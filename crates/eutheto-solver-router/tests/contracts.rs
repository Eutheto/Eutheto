use eutheto_domain_ir::{
    DomainAssignmentId, DomainEntityId, DomainEntityKindId, DomainEntityRef, OptimizationDirection,
    ScoreCategoryId,
};
use eutheto_planning_ir::{
    CandidateValues, ComparisonOp, CompilerId, Constraint, ConstraintRecord, InclusiveRange,
    IntDomain, IntVariable, IntVariableId, LinearComparison, LinearExpression, LinearTerm,
    MathematicalComponent, ObjectiveLevel, ObjectiveLevelId, ObjectivePlan, ObjectiveTerm,
    ObjectiveTermId, ObjectiveTermKind, PLANNING_IR_SCHEMA_VERSION, PlanningConstraintId,
    PlanningIrLimitsV1, PlanningMetadata, PlanningProblem, PlanningProblemSummary,
    ProjectionExpression, ProjectionId, ProvenanceId, ProvenanceRecord, ProvenanceSourceKind,
    SolutionProjection, SplitAuthorization, Variable, analyze_components, validate,
};
use eutheto_solver_api::*;
use eutheto_solver_router::*;
use eutheto_types::{
    BackendId, BackendSelection, CancellationToken, DurationMillis, ExplanationMode,
    FixedMonotonicClock, PackId, ParentSolveBudget, PreservationPolicy, ReproducibilityMode,
    ResourceLimits, ScenarioId, SolveMode, SolveOptions, SolveStatus, WorkerThreadPolicy,
};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Clone)]
enum Behavior {
    Outcome(BackendTerminationReason, bool, u64, u64),
    Error(bool, u64),
    AssignmentFlood,
    LateCandidate(BackendTerminationReason, u64, u64),
    Never,
    Proof {
        termination: BackendTerminationReason,
        candidates: Vec<i64>,
        final_value: i64,
        bound: Option<i64>,
    },
}

struct TestBackend {
    descriptor: SolverDescriptor,
    runtime_identity: BackendRuntimeIdentity,
    matrix: CapabilityMatrix,
    behavior: Behavior,
    clock: FixedMonotonicClock,
}

impl SolverBackend for TestBackend {
    fn descriptor(&self) -> &SolverDescriptor {
        &self.descriptor
    }

    fn runtime_identity(&self) -> &BackendRuntimeIdentity {
        &self.runtime_identity
    }

    fn compatibility(
        &self,
        summary: &PlanningProblemSummary,
        options: &SolveOptions,
    ) -> CompatibilityReport {
        compatibility_for(&self.matrix, &self.descriptor.id, summary, options).unwrap_or_else(
            |_| CompatibilityReport {
                level: CompatibilityLevel::Unsupported,
                unsupported_features: Vec::new(),
                warnings: Vec::new(),
                estimated_translation_cost: None,
            },
        )
    }

    // This one scripted fake keeps every backend return shape and timing adversary auditable.
    #[allow(clippy::too_many_lines)]
    fn solve<'a>(
        &'a self,
        request: &'a SolveRequest,
        output: &'a mut dyn BackendOutputSink,
    ) -> BackendSolveFuture<'a> {
        Box::pin(async move {
            if matches!(self.behavior, Behavior::Never) {
                std::future::pending::<()>().await;
            }
            if let Behavior::LateCandidate(termination, elapsed, advance) = &self.behavior {
                self.clock
                    .advance(Duration::from_millis(*advance))
                    .map_err(|_| BackendError::from(OutputError::UnsafeDiagnosticLine))?;
                output.submit_candidate(CandidateSubmission {
                    values: candidate_values()?,
                    observed_after_milliseconds: millis(1)?,
                    objective: None,
                    evidence_refs: Vec::new(),
                })?;
                return Ok(BackendSolveOutcome {
                    backend_id: self.descriptor.id.clone(),
                    model_hash: request.model_hash().to_owned(),
                    solve_fingerprint: request.solve_fingerprint().to_owned(),
                    termination: *termination,
                    evidence: BackendTerminationEvidence {
                        remaining_at_dispatch_milliseconds: request
                            .dispatch_budget()
                            .remaining_at_dispatch(),
                        backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
                        elapsed_milliseconds: millis(*elapsed)?,
                        first_incumbent_milliseconds: Some(millis(1)?),
                        objective: None,
                        evidence_refs: Vec::new(),
                        execution: None,
                    },
                });
            }
            if let Behavior::Proof {
                termination,
                candidates,
                final_value,
                bound,
            } = &self.behavior
            {
                for value in candidates {
                    let mut values = candidate_values()?;
                    values.integers.insert(
                        IntVariableId::new("tests.left")
                            .map_err(|_| OutputError::UnsafeDiagnosticLine)?,
                        *value,
                    );
                    output.submit_candidate(CandidateSubmission {
                        values,
                        observed_after_milliseconds: millis(1)?,
                        objective: Some(BackendObjectiveEvidence {
                            objective_values: vec![*value],
                            best_bound_values: None,
                        }),
                        evidence_refs: Vec::new(),
                    })?;
                }
                return Ok(BackendSolveOutcome {
                    backend_id: self.descriptor.id.clone(),
                    model_hash: request.model_hash().to_owned(),
                    solve_fingerprint: request.solve_fingerprint().to_owned(),
                    termination: *termination,
                    evidence: BackendTerminationEvidence {
                        remaining_at_dispatch_milliseconds: request
                            .dispatch_budget()
                            .remaining_at_dispatch(),
                        backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
                        elapsed_milliseconds: if *termination == BackendTerminationReason::TimeLimit
                        {
                            request.dispatch_budget().backend_limit()
                        } else {
                            millis(3)?
                        },
                        first_incumbent_milliseconds: Some(millis(1)?),
                        objective: Some(BackendObjectiveEvidence {
                            objective_values: vec![*final_value],
                            best_bound_values: bound.map(|value| vec![value]),
                        }),
                        evidence_refs: Vec::new(),
                        execution: None,
                    },
                });
            }
            let (candidate, advance) = match &self.behavior {
                Behavior::Outcome(_, candidate, _, advance)
                | Behavior::Error(candidate, advance) => (*candidate, *advance),
                Behavior::AssignmentFlood => (false, 0),
                Behavior::Proof { .. } | Behavior::LateCandidate(_, _, _) | Behavior::Never => {
                    unreachable!("handled before ordinary backend behavior")
                }
            };
            if matches!(self.behavior, Behavior::AssignmentFlood) {
                let submission = CandidateSubmission {
                    values: candidate_values()?,
                    observed_after_milliseconds: millis(1)?,
                    objective: None,
                    evidence_refs: Vec::new(),
                };
                output.submit_candidate(submission.clone())?;
                let _ = output.submit_candidate(submission);
                return Ok(BackendSolveOutcome {
                    backend_id: self.descriptor.id.clone(),
                    model_hash: request.model_hash().to_owned(),
                    solve_fingerprint: request.solve_fingerprint().to_owned(),
                    termination: BackendTerminationReason::CandidateFound,
                    evidence: BackendTerminationEvidence {
                        remaining_at_dispatch_milliseconds: request
                            .dispatch_budget()
                            .remaining_at_dispatch(),
                        backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
                        elapsed_milliseconds: millis(1)?,
                        first_incumbent_milliseconds: Some(millis(1)?),
                        objective: None,
                        evidence_refs: Vec::new(),
                        execution: None,
                    },
                });
            }
            if candidate {
                output.submit_candidate(CandidateSubmission {
                    values: candidate_values()?,
                    observed_after_milliseconds: millis(1)?,
                    objective: None,
                    evidence_refs: Vec::new(),
                })?;
            }
            self.clock
                .advance(Duration::from_millis(advance))
                .map_err(|_| BackendError::from(OutputError::UnsafeDiagnosticLine))?;
            match &self.behavior {
                Behavior::Error(_, _) => Err(BackendError::new(
                    "tests.crash",
                    "redacted backend failure",
                )?),
                Behavior::Outcome(termination, candidate, elapsed, _) => Ok(BackendSolveOutcome {
                    backend_id: self.descriptor.id.clone(),
                    model_hash: request.model_hash().to_owned(),
                    solve_fingerprint: request.solve_fingerprint().to_owned(),
                    termination: *termination,
                    evidence: BackendTerminationEvidence {
                        remaining_at_dispatch_milliseconds: request
                            .dispatch_budget()
                            .remaining_at_dispatch(),
                        backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
                        elapsed_milliseconds: millis(*elapsed)?,
                        first_incumbent_milliseconds: if *candidate {
                            Some(millis(1)?)
                        } else {
                            None
                        },
                        objective: None,
                        evidence_refs: Vec::new(),
                        execution: None,
                    },
                }),
                Behavior::AssignmentFlood => unreachable!("handled before backend outcome"),
                Behavior::Proof { .. } | Behavior::LateCandidate(_, _, _) | Behavior::Never => {
                    unreachable!("handled before ordinary backend outcome")
                }
            }
        })
    }
}

fn millis(value: u64) -> Result<DurationMillis, BackendError> {
    DurationMillis::new(value).map_err(|_| BackendError::from(OutputError::UnsafeDiagnosticLine))
}

struct Progress;
impl ProgressSink for Progress {
    fn emit(&mut self, _event: SolveProgressEvent) -> Result<(), OutputError> {
        Ok(())
    }
}
struct Quarantine;
impl CandidateReviewer for Quarantine {
    fn review(&mut self, _backend: &BackendId, _candidate: &BackendCandidate) -> CandidateReview {
        CandidateReview::VerificationFailed {
            diagnostic_code: "verification.rejected".to_owned(),
        }
    }
}

struct Verify;
impl CandidateReviewer for Verify {
    fn review(&mut self, _backend: &BackendId, _candidate: &BackendCandidate) -> CandidateReview {
        CandidateReview::Verified {
            objective_matches: false,
        }
    }
}

struct ExpiringReview {
    clock: FixedMonotonicClock,
}
impl CandidateReviewer for ExpiringReview {
    fn review(&mut self, _backend: &BackendId, _candidate: &BackendCandidate) -> CandidateReview {
        if self.clock.advance(Duration::from_secs(1)).is_err() {
            return CandidateReview::VerificationFailed {
                diagnostic_code: "verification.clock_overflow".to_owned(),
            };
        }
        CandidateReview::Verified {
            objective_matches: false,
        }
    }
}

fn features() -> Result<Vec<SupportFeature>, SupportMatrixError> {
    SUPPORT_FEATURES
        .iter()
        .map(|(id, category, gate)| {
            Ok(SupportFeature {
                id: SupportFeatureId::new(*id)?,
                category: match *category {
                    "primitive" => SupportFeatureCategory::Primitive,
                    "objective" => SupportFeatureCategory::Objective,
                    "projection" => SupportFeatureCategory::Projection,
                    "solve" => SupportFeatureCategory::Solve,
                    value => {
                        return Err(SupportMatrixError::UnknownGeneratedCategory(
                            value.to_owned(),
                        ));
                    }
                },
                gate: if *gate == "unconditional" {
                    SupportFeatureGate::Unconditional
                } else {
                    SupportFeatureGate::Enabled((*gate).to_owned())
                },
            })
        })
        .collect()
}

fn registry(
    specs: Vec<(&str, Behavior, bool)>,
    clock: &FixedMonotonicClock,
) -> Result<SolverRegistry, Box<dyn Error>> {
    let feature_list = features()?;
    let mut descriptors = Vec::new();
    let mut columns = Vec::new();
    for (id, _, incompatible) in &specs {
        let backend_id = BackendId::new(id)?;
        let mut supported = BTreeSet::new();
        let cells = feature_list
            .iter()
            .map(|feature| {
                let cell = if *incompatible && feature.id.as_str() == "solve.deterministic-mode" {
                    SupportCell::Unsupported {
                        reason: "deterministic mode unavailable".to_owned(),
                        remediation: "choose another backend".to_owned(),
                        fixture_id: "fixture.unsupported".to_owned(),
                    }
                } else {
                    supported.insert(feature.id.clone());
                    SupportCell::Supported {
                        fixture_id: format!("fixture.{}", feature.id.as_str().replace('.', "-")),
                    }
                };
                (feature.id.clone(), cell)
            })
            .collect();
        let descriptor = SolverDescriptor {
            id: backend_id.clone(),
            display_name: format!("Test {id}"),
            version: "1.0.0".to_owned(),
            adapter_version: "1.0.0".to_owned(),
            distribution: SolverDistribution::BuiltIn,
            license: LicenseMetadata {
                spdx_expression: "Apache-2.0".to_owned(),
                license_name: "Apache License 2.0".to_owned(),
                source_url: None,
            },
            stability: BackendStability::Experimental,
            capabilities: SolverCapabilities {
                supported,
                degraded: BTreeSet::new(),
            },
        };
        columns.push(BackendSupportColumn {
            backend_id,
            backend_version: descriptor.version.clone(),
            adapter_version: descriptor.adapter_version.clone(),
            cells,
        });
        descriptors.push(descriptor);
    }
    let matrix = CapabilityMatrix::new(
        SUPPORT_MATRIX_SCHEMA_VERSION,
        SUPPORT_MATRIX_IR_SCHEMA_VERSION,
        feature_list,
        columns,
        Vec::new(),
    )?;
    let backends = specs
        .into_iter()
        .zip(descriptors)
        .map(
            |((_, behavior, _), descriptor)| -> Result<Arc<dyn SolverBackend>, Box<dyn Error>> {
                let runtime_identity = BackendRuntimeIdentity::new(
                    descriptor.id.clone(),
                    descriptor.version.clone(),
                    descriptor.adapter_version.clone(),
                    "1.0.0".to_owned(),
                    "1.0.0".to_owned(),
                    1,
                    0,
                )?;
                Ok(Arc::new(TestBackend {
                    descriptor,
                    runtime_identity,
                    matrix: matrix.clone(),
                    behavior,
                    clock: clock.clone(),
                }) as Arc<dyn SolverBackend>)
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SolverRegistry::new(matrix, backends)?)
}

fn problem() -> Result<PlanningProblem, Box<dyn Error>> {
    let provenance = ProvenanceId::new("tests.provenance")?;
    let mut problem = PlanningProblem {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables: ["tests.left", "tests.right"]
            .into_iter()
            .map(|id| -> Result<Variable, Box<dyn Error>> {
                Ok(Variable::Integer(IntVariable {
                    id: IntVariableId::new(id)?,
                    domain: IntDomain::new(vec![InclusiveRange { start: 1, end: 3 }])?,
                    provenance: provenance.clone(),
                }))
            })
            .collect::<Result<Vec<_>, _>>()?,
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
            scenario_revision: 1,
            projection_version: 1,
            compiler_id: CompilerId::new("tests.compiler")?,
            compiler_version: "1.0.0".to_owned(),
            compile_metadata: BTreeMap::new(),
            display_text: BTreeMap::new(),
        },
        declared_capabilities: BTreeSet::new(),
        split_authorization: None,
    };
    problem.canonicalize()?;
    Ok(problem)
}

fn candidate_values() -> Result<CandidateValues, OutputError> {
    Ok(CandidateValues {
        booleans: BTreeMap::new(),
        integers: BTreeMap::from([
            (
                IntVariableId::new("tests.left").map_err(|_| OutputError::UnsafeDiagnosticLine)?,
                1,
            ),
            (
                IntVariableId::new("tests.right").map_err(|_| OutputError::UnsafeDiagnosticLine)?,
                2,
            ),
        ]),
    })
}

fn options(backend: BackendSelection) -> Result<SolveOptions, Box<dyn Error>> {
    Ok(SolveOptions {
        backend,
        mode: SolveMode::Custom,
        time_limit_milliseconds: DurationMillis::new(1_000)?,
        memory_limit_bytes: Some(1024),
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

async fn run(
    registry: &SolverRegistry,
    problem: PlanningProblem,
    clock: &FixedMonotonicClock,
    reviewer: &mut dyn CandidateReviewer,
) -> Result<RouterExecutionRecord, Box<dyn Error>> {
    let parent = ParentSolveBudget::new(
        DurationMillis::new(1_000)?,
        Arc::new(clock.clone()),
        CancellationToken::new(),
    )?;
    let mut progress = Progress;
    Ok(SolverRouter::new(registry)
        .execute(
            Arc::new(problem),
            options(BackendSelection::Auto)?,
            &parent,
            &mut progress,
            reviewer,
        )
        .await)
}

#[test]
fn profiles_and_production_registry_are_versioned_and_safe() -> TestResult {
    assert_eq!(ROUTING_PROFILE_VERSION, 1);
    assert_eq!(RoutingProfile::QUICK_V1.backend_cap_milliseconds, 1_000);
    assert_eq!(RoutingProfile::BALANCED_V1.backend_cap_milliseconds, 3_000);
    assert_eq!(RoutingProfile::DEEP_V1.mode, SolveMode::Deep);
    assert!(RoutingProfile::for_mode(SolveMode::Custom).is_none());
    let production = production_registry()?;
    assert!(production.is_empty());
    assert_eq!(production.descriptors().count(), 0);
    Ok(())
}

#[test]
fn auto_override_and_component_authorization_are_exact_and_deterministic() -> TestResult {
    let clock = FixedMonotonicClock::default();
    let behavior = Behavior::Outcome(BackendTerminationReason::InfeasibilityClaimed, false, 1, 0);
    let registry = registry(
        vec![
            ("tests.z", behavior.clone(), false),
            ("tests.a", behavior, false),
        ],
        &clock,
    )?;
    let base = problem()?;
    let auto = SolverRouter::new(&registry).decide(&base, &options(BackendSelection::Auto)?);
    assert_eq!(auto.chosen_backend, Some(BackendId::new("tests.a")?));
    assert_eq!(auto.fallback_order, vec![BackendId::new("tests.z")?]);
    assert!(
        auto.considered_backends
            .iter()
            .all(|entry| entry.matrix_report.is_some() && entry.backend_report.is_some())
    );
    let specific = SolverRouter::new(&registry).decide(
        &base,
        &options(BackendSelection::Specific(BackendId::new("tests.z")?))?,
    );
    assert_eq!(specific.chosen_backend, Some(BackendId::new("tests.z")?));
    assert!(specific.fallback_order.is_empty());
    let analysis = analyze_components(&base);
    assert_eq!(analysis.components.len(), 2);
    assert_eq!(auto.split, SplitDisposition::MissingAuthorization);
    for (authorization, expected) in [
        (
            SplitAuthorization {
                component_hash: "0".repeat(64),
                domain_merge_contract: "tests.merge-v1".to_owned(),
                projection_independent: true,
            },
            0_u8,
        ),
        (
            SplitAuthorization {
                component_hash: analysis.component_hash.clone(),
                domain_merge_contract: String::new(),
                projection_independent: true,
            },
            1,
        ),
        (
            SplitAuthorization {
                component_hash: analysis.component_hash.clone(),
                domain_merge_contract: "tests.merge-v1".to_owned(),
                projection_independent: false,
            },
            2,
        ),
        (
            SplitAuthorization {
                component_hash: analysis.component_hash.clone(),
                domain_merge_contract: "tests.merge-v1".to_owned(),
                projection_independent: true,
            },
            3,
        ),
    ] {
        let mut candidate = base.clone();
        candidate.split_authorization = Some(authorization);
        let split = SolverRouter::new(&registry)
            .decide(&candidate, &options(BackendSelection::Auto)?)
            .split;
        assert!(matches!(
            (expected, split),
            (0..=2, SplitDisposition::InvalidModel)
                | (3, SplitDisposition::AuthorizedWholeModelOnly { .. })
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConformanceSolution {
    assignments: BTreeMap<IntVariableId, i64>,
    objective_values: BTreeMap<String, i64>,
    ordered_score: Vec<i64>,
    projections: BTreeMap<String, i64>,
}

fn component_conformance_problem() -> Result<PlanningProblem, Box<dyn Error>> {
    let mut fixture = problem()?;
    let provenance = fixture.provenance[0].id.clone();
    let left = IntVariableId::new("tests.left")?;
    let right = IntVariableId::new("tests.right")?;
    fixture.constraints = vec![
        ConstraintRecord {
            id: PlanningConstraintId::new("tests.constraint.left")?,
            body: Constraint::LinearComparison(LinearComparison {
                expression: LinearExpression::new(
                    vec![LinearTerm {
                        variable: left.clone(),
                        coefficient: 1,
                    }],
                    0,
                )?,
                op: ComparisonOp::GreaterOrEqual,
                rhs: 2,
            }),
            enforcement: Vec::new(),
            provenance: provenance.clone(),
            tags: Vec::new(),
        },
        ConstraintRecord {
            id: PlanningConstraintId::new("tests.constraint.right")?,
            body: Constraint::LinearComparison(LinearComparison {
                expression: LinearExpression::new(
                    vec![LinearTerm {
                        variable: right.clone(),
                        coefficient: 1,
                    }],
                    0,
                )?,
                op: ComparisonOp::LessOrEqual,
                rhs: 2,
            }),
            enforcement: Vec::new(),
            provenance: provenance.clone(),
            tags: Vec::new(),
        },
    ];
    fixture.objectives = ObjectivePlan {
        levels: [("left", left.clone()), ("right", right.clone())]
            .into_iter()
            .map(
                |(name, variable)| -> Result<ObjectiveLevel, Box<dyn Error>> {
                    Ok(ObjectiveLevel {
                        id: ObjectiveLevelId::new(format!("tests.level.{name}"))?,
                        direction: OptimizationDirection::Minimize,
                        lower_bound: 1,
                        upper_bound: 3,
                        terms: vec![ObjectiveTerm {
                            id: ObjectiveTermId::new(format!("tests.objective.{name}"))?,
                            expression: LinearExpression::new(
                                vec![LinearTerm {
                                    variable,
                                    coefficient: 1,
                                }],
                                0,
                            )?,
                            kind: ObjectiveTermKind::Penalty,
                            category: ScoreCategoryId::new(format!("tests.score.{name}"))?,
                            provenance: provenance.clone(),
                        }],
                        provenance: provenance.clone(),
                    })
                },
            )
            .collect::<Result<Vec<_>, _>>()?,
    };
    fixture.projections = [("left", left), ("right", right)]
        .into_iter()
        .map(
            |(name, variable)| -> Result<SolutionProjection, Box<dyn Error>> {
                Ok(SolutionProjection {
                    id: ProjectionId::new(format!("tests.projection.{name}"))?,
                    assignment_id: DomainAssignmentId::new(format!("tests.assignment.{name}"))?,
                    entity: DomainEntityRef {
                        kind: DomainEntityKindId::new("tests.component")?,
                        id: DomainEntityId::new(format!("tests.component.{name}"))?,
                    },
                    required: true,
                    expression: ProjectionExpression::Integer(variable),
                    provenance: provenance.clone(),
                })
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
    fixture.canonicalize()?;
    let analysis = analyze_components(&fixture);
    fixture.split_authorization = Some(SplitAuthorization {
        component_hash: analysis.component_hash,
        domain_merge_contract: "tests.merge-v1".to_owned(),
        projection_independent: true,
    });
    validate(&fixture, PlanningIrLimitsV1::DEFAULT)?;
    Ok(fixture)
}

fn expression_value(
    expression: &LinearExpression,
    assignments: &BTreeMap<IntVariableId, i64>,
) -> Result<i64, Box<dyn Error>> {
    expression
        .terms
        .iter()
        .try_fold(expression.constant, |value, term| {
            let assigned = assignments
                .get(&term.variable)
                .copied()
                .ok_or("missing conformance assignment")?;
            value
                .checked_add(
                    term.coefficient
                        .checked_mul(assigned)
                        .ok_or("conformance multiplication overflow")?,
                )
                .ok_or("conformance addition overflow")
        })
        .map_err(Into::into)
}

// One bounded exhaustive solver keeps component extraction, feasibility, scoring, and projection
// evaluation identical for the whole-model and split/merge conformance paths.
#[allow(clippy::too_many_lines)]
fn solve_component_fixture_bounded(
    problem: &PlanningProblem,
) -> Result<ConformanceSolution, Box<dyn Error>> {
    const MAX_ENUMERATED_ASSIGNMENTS: usize = 64;
    let mut assignments = vec![BTreeMap::new()];
    for variable in &problem.variables {
        let Variable::Integer(variable) = variable else {
            return Err("component conformance fixture supports integer variables only".into());
        };
        let mut next = Vec::new();
        for assignment in &assignments {
            for range in &variable.domain.inclusive_ranges {
                for value in range.start..=range.end {
                    if next.len() >= MAX_ENUMERATED_ASSIGNMENTS {
                        return Err("component conformance enumeration limit exceeded".into());
                    }
                    let mut candidate = assignment.clone();
                    candidate.insert(variable.id.clone(), value);
                    next.push(candidate);
                }
            }
        }
        assignments = next;
    }
    let mut best: Option<ConformanceSolution> = None;
    for assignments in assignments {
        let feasible = problem
            .constraints
            .iter()
            .try_fold(true, |feasible, record| {
                if !record.enforcement.is_empty() {
                    return Err("component conformance fixture forbids enforcement literals");
                }
                let Constraint::LinearComparison(comparison) = &record.body else {
                    return Err("component conformance fixture supports linear comparisons only");
                };
                let value = expression_value(&comparison.expression, &assignments)
                    .map_err(|_| "component conformance expression failed")?;
                let satisfied = match comparison.op {
                    ComparisonOp::Equal => value == comparison.rhs,
                    ComparisonOp::LessOrEqual => value <= comparison.rhs,
                    ComparisonOp::GreaterOrEqual => value >= comparison.rhs,
                };
                Ok(feasible && satisfied)
            })?;
        if !feasible {
            continue;
        }
        let mut objective_values = BTreeMap::new();
        let mut ordered_score = Vec::new();
        for level in &problem.objectives.levels {
            if level.direction != OptimizationDirection::Minimize
                || level
                    .terms
                    .iter()
                    .any(|term| term.kind != ObjectiveTermKind::Penalty)
            {
                return Err(
                    "component conformance fixture supports minimizing penalties only".into(),
                );
            }
            let mut value = 0_i64;
            for term in &level.terms {
                value = value
                    .checked_add(expression_value(&term.expression, &assignments)?)
                    .ok_or("component conformance objective overflow")?;
            }
            ordered_score.push(value);
            objective_values.insert(level.id.to_string(), value);
        }
        let mut projections = BTreeMap::new();
        for projection in &problem.projections {
            let value = match &projection.expression {
                ProjectionExpression::Integer(variable) => assignments
                    .get(variable)
                    .copied()
                    .ok_or("missing projected conformance assignment")?,
                ProjectionExpression::Linear(expression) => {
                    expression_value(expression, &assignments)?
                }
                _ => return Err("unsupported component conformance projection".into()),
            };
            if projections
                .insert(projection.assignment_id.to_string(), value)
                .is_some()
            {
                return Err("duplicate conformance projection".into());
            }
        }
        let candidate = ConformanceSolution {
            assignments,
            objective_values,
            ordered_score,
            projections,
        };
        if best.as_ref().is_none_or(|current| {
            candidate.ordered_score < current.ordered_score
                || (candidate.ordered_score == current.ordered_score
                    && candidate.assignments < current.assignments)
        }) {
            best = Some(candidate);
        }
    }
    best.ok_or_else(|| "component conformance fixture is infeasible".into())
}

fn extract_authorized_component(
    whole: &PlanningProblem,
    component: &MathematicalComponent,
) -> Result<PlanningProblem, Box<dyn Error>> {
    let authorization = whole
        .split_authorization
        .as_ref()
        .ok_or("missing component authorization")?;
    let analysis = analyze_components(whole);
    if !analysis.components.iter().any(|actual| actual == component)
        || authorization.component_hash != analysis.component_hash
        || authorization.domain_merge_contract != "tests.merge-v1"
        || !authorization.projection_independent
    {
        return Err("component authorization proof mismatch".into());
    }
    let nodes: BTreeSet<_> = component.variable_nodes.iter().cloned().collect();
    let owns = |variable: &IntVariableId| nodes.contains(&format!("int:{variable}"));
    let mut extracted = whole.clone();
    extracted
        .variables
        .retain(|variable| matches!(variable, Variable::Integer(variable) if owns(&variable.id)));
    extracted.constraints.retain(|record| {
        matches!(
            &record.body,
            Constraint::LinearComparison(comparison)
                if comparison.expression.terms.iter().all(|term| owns(&term.variable))
        )
    });
    extracted.objectives.levels.retain(|level| {
        level.terms.iter().all(|term| {
            term.expression
                .terms
                .iter()
                .all(|term| owns(&term.variable))
        })
    });
    extracted
        .projections
        .retain(|projection| match &projection.expression {
            ProjectionExpression::Integer(variable) => owns(variable),
            ProjectionExpression::Linear(expression) => {
                expression.terms.iter().all(|term| owns(&term.variable))
            }
            _ => false,
        });
    extracted.split_authorization = None;
    extracted.canonicalize()?;
    validate(&extracted, PlanningIrLimitsV1::DEFAULT)?;
    Ok(extracted)
}

#[test]
fn authorized_components_are_actually_solved_and_merged_equivalently() -> TestResult {
    let whole = component_conformance_problem()?;
    let analysis = analyze_components(&whole);
    assert_eq!(analysis.components.len(), 2);
    let whole_solution = solve_component_fixture_bounded(&whole)?;
    let mut merged = ConformanceSolution {
        assignments: BTreeMap::new(),
        objective_values: BTreeMap::new(),
        ordered_score: Vec::new(),
        projections: BTreeMap::new(),
    };
    let mut extracted_constraints = 0;
    let mut extracted_objectives = 0;
    let mut extracted_projections = 0;
    for component in &analysis.components {
        let extracted = extract_authorized_component(&whole, component)?;
        extracted_constraints += extracted.constraints.len();
        extracted_objectives += extracted.objectives.levels.len();
        extracted_projections += extracted.projections.len();
        let solved = solve_component_fixture_bounded(&extracted)?;
        for (variable, value) in solved.assignments {
            if merged.assignments.insert(variable, value).is_some() {
                return Err("component assignment collision".into());
            }
        }
        for (level, value) in solved.objective_values {
            if merged.objective_values.insert(level, value).is_some() {
                return Err("component objective collision".into());
            }
        }
        for (assignment, value) in solved.projections {
            if merged.projections.insert(assignment, value).is_some() {
                return Err("component projection collision".into());
            }
        }
    }
    merged.ordered_score = whole
        .objectives
        .levels
        .iter()
        .map(|level| {
            merged
                .objective_values
                .get(&level.id.to_string())
                .copied()
                .ok_or("missing merged objective")
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(extracted_constraints, whole.constraints.len());
    assert_eq!(extracted_objectives, whole.objectives.levels.len());
    assert_eq!(extracted_projections, whole.projections.len());
    assert_eq!(merged, whole_solution);
    Ok(())
}

fn assert_invalid_model_without_invocations(record: &RouterExecutionRecord) {
    assert_eq!(record.invocation_count, 0);
    assert_eq!(record.terminal_status, SolveStatus::InvalidModel);
    assert!(record.decision.component_analysis.components.is_empty());
    assert_eq!(record.decision.split, SplitDisposition::InvalidModel);
}

#[tokio::test]
async fn invalid_ir_and_unsupported_override_have_zero_invocations() -> TestResult {
    let clock = FixedMonotonicClock::default();
    let registry = registry(
        vec![(
            "tests.unsupported",
            Behavior::Outcome(BackendTerminationReason::InfeasibilityClaimed, false, 1, 0),
            true,
        )],
        &clock,
    )?;
    let parent = ParentSolveBudget::new(
        DurationMillis::new(1_000)?,
        Arc::new(clock.clone()),
        CancellationToken::new(),
    )?;
    let mut progress = Progress;
    let mut reviewer = RequireIndependentVerification;
    let unsupported = SolverRouter::new(&registry)
        .execute(
            Arc::new(problem()?),
            options(BackendSelection::Specific(BackendId::new(
                "tests.unsupported",
            )?))?,
            &parent,
            &mut progress,
            &mut reviewer,
        )
        .await;
    assert_eq!(unsupported.invocation_count, 0);
    let mut invalid = problem()?;
    invalid.variables.push(invalid.variables[0].clone());
    let invalid = SolverRouter::new(&registry)
        .execute(
            Arc::new(invalid),
            options(BackendSelection::Auto)?,
            &parent,
            &mut progress,
            &mut reviewer,
        )
        .await;
    assert_invalid_model_without_invocations(&invalid);
    assert_eq!(invalid.decision.component_analysis.edge_count, 0);
    assert!(
        invalid
            .decision
            .component_analysis
            .component_hash
            .is_empty()
    );
    let mut undeclared = problem()?;
    undeclared.constraints.push(ConstraintRecord {
        id: PlanningConstraintId::new("tests.all-different")?,
        body: Constraint::AllDifferent {
            variables: vec![
                IntVariableId::new("tests.left")?,
                IntVariableId::new("tests.right")?,
            ],
        },
        enforcement: Vec::new(),
        provenance: undeclared.provenance[0].id.clone(),
        tags: Vec::new(),
    });
    let undeclared = SolverRouter::new(&registry)
        .execute(
            Arc::new(undeclared),
            options(BackendSelection::Auto)?,
            &parent,
            &mut progress,
            &mut reviewer,
        )
        .await;
    assert_invalid_model_without_invocations(&undeclared);

    let mut noncanonical = problem()?;
    let Variable::Integer(integer) = &mut noncanonical.variables[0] else {
        return Err("expected integer fixture".into());
    };
    integer.domain = IntDomain {
        inclusive_ranges: vec![
            InclusiveRange { start: 1, end: 2 },
            InclusiveRange { start: 2, end: 3 },
        ],
    };
    let noncanonical = SolverRouter::new(&registry)
        .execute(
            Arc::new(noncanonical),
            options(BackendSelection::Auto)?,
            &parent,
            &mut progress,
            &mut reviewer,
        )
        .await;
    assert_invalid_model_without_invocations(&noncanonical);
    Ok(())
}

#[tokio::test]
async fn fallback_is_candidate_aware_and_shares_parent_deadline() -> TestResult {
    let clock = FixedMonotonicClock::default();
    let fallback_registry = registry(
        vec![
            (
                "tests.a",
                Behavior::Outcome(BackendTerminationReason::Unavailable, false, 600, 600),
                false,
            ),
            (
                "tests.b",
                Behavior::Outcome(BackendTerminationReason::TimeLimit, false, 400, 400),
                false,
            ),
        ],
        &clock,
    )?;
    let mut reviewer = RequireIndependentVerification;
    let result = run(&fallback_registry, problem()?, &clock, &mut reviewer).await?;
    assert_eq!(result.invocation_count, 2);
    assert!(result.attempts[0].fallback_taken);
    assert_eq!(
        result.attempts[1]
            .remaining_at_dispatch_milliseconds
            .value(),
        400
    );
    assert!(!result.attempts[1].fallback_eligible);
    assert!(!result.attempts[1].fallback_taken);
    assert_eq!(result.terminal_status, SolveStatus::NoSolutionWithinLimit);

    for behavior in [
        Behavior::Outcome(BackendTerminationReason::InvalidModel, false, 1, 0),
        Behavior::Outcome(BackendTerminationReason::InfeasibilityClaimed, false, 1, 0),
        Behavior::Error(true, 0),
    ] {
        let clock = FixedMonotonicClock::default();
        let registry = registry(
            vec![
                ("tests.a", behavior, false),
                (
                    "tests.b",
                    Behavior::Outcome(BackendTerminationReason::Unavailable, false, 1, 0),
                    false,
                ),
            ],
            &clock,
        )?;
        let result = run(&registry, problem()?, &clock, &mut reviewer).await?;
        assert_eq!(result.invocation_count, 1);
        assert!(!result.attempts[0].fallback_taken);
    }
    Ok(())
}

#[tokio::test]
async fn late_empty_proof_claims_cannot_outlive_the_parent_deadline() -> TestResult {
    for claimed in [
        BackendTerminationReason::InfeasibilityClaimed,
        BackendTerminationReason::UnboundedClaimed,
    ] {
        let clock = FixedMonotonicClock::default();
        let registry = registry(
            vec![
                (
                    "tests.a",
                    Behavior::Outcome(claimed, false, 1, 1_000),
                    false,
                ),
                (
                    "tests.b",
                    Behavior::Outcome(BackendTerminationReason::InfeasibilityClaimed, false, 1, 0),
                    false,
                ),
            ],
            &clock,
        )?;
        let mut reviewer = RequireIndependentVerification;
        let result = run(&registry, problem()?, &clock, &mut reviewer).await?;
        assert_eq!(result.invocation_count, 1);
        assert_eq!(result.terminal_status, SolveStatus::NoSolutionWithinLimit);
        assert_eq!(
            result.terminal_reason,
            ExecutionTerminalReason::ParentDeadlineExceeded
        );
        assert_eq!(
            result.attempts[0].termination,
            AttemptTermination::ParentDeadlineExceeded
        );
        assert!(!result.attempts[0].fallback_eligible);
        assert!(!result.attempts[0].fallback_taken);
        assert!(result.attempts[0].outcome.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn shorter_backend_cap_rejects_late_proof_and_candidate() -> TestResult {
    for behavior in [
        Behavior::Outcome(
            BackendTerminationReason::InfeasibilityClaimed,
            false,
            1,
            1_500,
        ),
        Behavior::LateCandidate(BackendTerminationReason::OptimalityClaimed, 1, 1_500),
    ] {
        let clock = FixedMonotonicClock::default();
        let registry = registry(vec![("tests.a", behavior, false)], &clock)?;
        let parent = ParentSolveBudget::new(
            DurationMillis::new(2_500)?,
            Arc::new(clock),
            CancellationToken::new(),
        )?;
        let mut solve_options = options(BackendSelection::Auto)?;
        solve_options.mode = SolveMode::Quick;
        solve_options.time_limit_milliseconds = DurationMillis::new(2_500)?;
        let mut progress = Progress;
        let mut reviewer = Verify;
        let result = SolverRouter::new(&registry)
            .execute(
                Arc::new(problem()?),
                solve_options,
                &parent,
                &mut progress,
                &mut reviewer,
            )
            .await;
        assert_eq!(result.invocation_count, 1);
        assert_eq!(result.terminal_status, SolveStatus::NoSolutionWithinLimit);
        assert_eq!(
            result.attempts[0].termination,
            AttemptTermination::BackendOutcome(BackendTerminationReason::TimeLimit)
        );
        assert!(result.attempts[0].outcome.is_none());
        assert!(result.selected_candidate.is_none());
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn nonreturning_backend_is_bounded_by_router_timeout() -> TestResult {
    let clock = FixedMonotonicClock::default();
    let registry = registry(vec![("tests.a", Behavior::Never, false)], &clock)?;
    let parent = ParentSolveBudget::new(
        DurationMillis::new(1)?,
        Arc::new(clock),
        CancellationToken::new(),
    )?;
    let mut solve_options = options(BackendSelection::Auto)?;
    solve_options.time_limit_milliseconds = DurationMillis::new(1)?;
    let mut progress = Progress;
    let mut reviewer = Verify;
    let result = SolverRouter::new(&registry)
        .execute(
            Arc::new(problem()?),
            solve_options,
            &parent,
            &mut progress,
            &mut reviewer,
        )
        .await;
    assert_eq!(result.invocation_count, 1);
    assert_eq!(result.terminal_status, SolveStatus::NoSolutionWithinLimit);
    assert_eq!(
        result.attempts[0].termination,
        AttemptTermination::BackendOutcome(BackendTerminationReason::TimeLimit)
    );
    Ok(())
}

#[tokio::test]
async fn cumulative_assignment_overflow_is_a_bounded_resource_terminal() -> TestResult {
    let clock = FixedMonotonicClock::default();
    let registry = registry(vec![("tests.a", Behavior::AssignmentFlood, false)], &clock)?;
    let parent = ParentSolveBudget::new(
        DurationMillis::new(1_000)?,
        Arc::new(clock),
        CancellationToken::new(),
    )?;
    let limits = SolverApiLimits {
        max_candidate_assignments: 3,
        ..SolverApiLimits::DEFAULT
    };
    let mut progress = Progress;
    let mut reviewer = Verify;
    let result = SolverRouter::new(&registry)
        .with_limits(limits)?
        .execute(
            Arc::new(problem()?),
            options(BackendSelection::Auto)?,
            &parent,
            &mut progress,
            &mut reviewer,
        )
        .await;
    let expected = OutputError::CandidateAssignmentLimitExceeded.to_string();
    assert_eq!(result.invocation_count, 1);
    assert_eq!(result.terminal_status, SolveStatus::BackendFailed);
    assert_eq!(
        result.terminal_reason,
        ExecutionTerminalReason::SharedOutputLimitExhausted
    );
    assert_eq!(
        result.attempts[0].termination,
        AttemptTermination::BackendError {
            code: expected.clone()
        }
    );
    assert_eq!(
        result.attempts[0].backend_failure_code.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(result.attempts[0].candidate_count, 0);
    assert!(result.selected_candidate.is_none());
    Ok(())
}

#[tokio::test]
async fn unavailable_and_crash_before_candidate_fallback_but_quarantine_never_does() -> TestResult {
    for first in [
        Behavior::Outcome(BackendTerminationReason::Unavailable, false, 1, 0),
        Behavior::Error(false, 0),
    ] {
        let clock = FixedMonotonicClock::default();
        let registry = registry(
            vec![
                ("tests.a", first, false),
                (
                    "tests.b",
                    Behavior::Outcome(BackendTerminationReason::InfeasibilityClaimed, false, 1, 0),
                    false,
                ),
            ],
            &clock,
        )?;
        let mut reviewer = RequireIndependentVerification;
        let result = run(&registry, problem()?, &clock, &mut reviewer).await?;
        assert_eq!(result.invocation_count, 2);
        assert_eq!(result.terminal_status, SolveStatus::Infeasible);
    }
    let clock = FixedMonotonicClock::default();
    let registry = registry(
        vec![
            (
                "tests.a",
                Behavior::Outcome(BackendTerminationReason::CandidateFound, true, 5, 0),
                false,
            ),
            (
                "tests.b",
                Behavior::Outcome(BackendTerminationReason::InfeasibilityClaimed, false, 1, 0),
                false,
            ),
        ],
        &clock,
    )?;
    let mut reviewer = Quarantine;
    let result = run(&registry, problem()?, &clock, &mut reviewer).await?;
    assert_eq!(result.invocation_count, 1);
    assert_eq!(
        result.terminal_reason,
        ExecutionTerminalReason::VerificationQuarantined
    );
    assert!(result.selected_candidate.is_none());
    assert!(!result.attempts[0].fallback_eligible);
    Ok(())
}

#[tokio::test]
async fn time_limit_falls_back_only_with_no_candidate_and_parent_time_remaining() -> TestResult {
    let clock = FixedMonotonicClock::default();
    let registry = registry(
        vec![
            (
                "tests.a",
                Behavior::Outcome(BackendTerminationReason::TimeLimit, false, 1_000, 1_000),
                false,
            ),
            (
                "tests.b",
                Behavior::Outcome(BackendTerminationReason::InfeasibilityClaimed, false, 1, 0),
                false,
            ),
        ],
        &clock,
    )?;
    let parent = ParentSolveBudget::new(
        DurationMillis::new(2_500)?,
        Arc::new(clock.clone()),
        CancellationToken::new(),
    )?;
    let mut solve_options = options(BackendSelection::Auto)?;
    solve_options.mode = SolveMode::Quick;
    solve_options.time_limit_milliseconds = DurationMillis::new(2_500)?;
    let mut progress = Progress;
    let mut reviewer = RequireIndependentVerification;
    let result = SolverRouter::new(&registry)
        .execute(
            Arc::new(problem()?),
            solve_options,
            &parent,
            &mut progress,
            &mut reviewer,
        )
        .await;
    assert_eq!(result.invocation_count, 2);
    assert!(result.attempts[0].fallback_taken);
    assert_eq!(
        result.attempts[1]
            .remaining_at_dispatch_milliseconds
            .value(),
        1_500
    );
    assert_eq!(result.terminal_status, SolveStatus::Infeasible);
    Ok(())
}

#[tokio::test]
async fn retained_candidate_is_reviewed_after_backend_error_and_failure_is_preserved() -> TestResult
{
    let clock = FixedMonotonicClock::default();
    let registry = registry(
        vec![
            ("tests.a", Behavior::Error(true, 7), false),
            (
                "tests.b",
                Behavior::Outcome(BackendTerminationReason::InfeasibilityClaimed, false, 1, 0),
                false,
            ),
        ],
        &clock,
    )?;
    let mut reviewer = Verify;
    let result = run(&registry, problem()?, &clock, &mut reviewer).await?;
    assert_eq!(result.invocation_count, 1);
    assert_eq!(result.terminal_status, SolveStatus::Feasible);
    assert_eq!(
        result.attempts[0].backend_failure_code.as_deref(),
        Some("tests.crash")
    );
    assert!(!result.attempts[0].fallback_taken);
    assert_eq!(result.attempts[0].backend_version, "1.0.0");
    assert_eq!(result.attempts[0].adapter_version, "1.0.0");
    assert_eq!(result.attempts[0].solve_fingerprint.len(), 64);
    assert!(result.attempts[0].outcome.is_none());
    assert_eq!(
        result.first_verified_feasible_milliseconds,
        Some(DurationMillis::new(7)?)
    );
    Ok(())
}

#[tokio::test]
async fn backend_optimality_without_independent_objective_match_stays_feasible() -> TestResult {
    let clock = FixedMonotonicClock::default();
    let registry = registry(
        vec![(
            "tests.a",
            Behavior::Outcome(BackendTerminationReason::OptimalityClaimed, true, 5, 5),
            false,
        )],
        &clock,
    )?;
    let mut reviewer = Verify;
    let result = run(&registry, problem()?, &clock, &mut reviewer).await?;
    assert_eq!(result.terminal_status, SolveStatus::Feasible);
    assert_eq!(
        result.first_verified_feasible_milliseconds,
        Some(DurationMillis::new(5)?)
    );
    Ok(())
}

#[tokio::test]
// Keep the distinct proof-binding, candidate-selection and limit cases in one auditable matrix.
#[allow(clippy::too_many_lines)]
async fn optimal_status_requires_the_selected_verified_final_candidate_and_equal_bound()
-> TestResult {
    struct Review {
        skip_first: bool,
        matches: bool,
    }
    impl CandidateReviewer for Review {
        fn review(&mut self, _: &BackendId, candidate: &BackendCandidate) -> CandidateReview {
            if self.skip_first && candidate.sequence == 1 {
                CandidateReview::AwaitingIndependentVerification
            } else {
                CandidateReview::Verified {
                    objective_matches: self.matches,
                }
            }
        }
    }
    use BackendTerminationReason::{OptimalityClaimed, SolutionLimit, TimeLimit};
    for (termination, candidates, final_value, bound, skip_first, matched, expected) in [
        (
            OptimalityClaimed,
            vec![1],
            1,
            Some(1),
            false,
            true,
            SolveStatus::Optimal,
        ),
        (
            OptimalityClaimed,
            vec![2, 1],
            1,
            Some(1),
            false,
            true,
            SolveStatus::Feasible,
        ),
        (
            OptimalityClaimed,
            vec![2, 1],
            1,
            Some(1),
            true,
            true,
            SolveStatus::Optimal,
        ),
        (
            OptimalityClaimed,
            vec![1],
            1,
            None,
            false,
            true,
            SolveStatus::Feasible,
        ),
        (
            OptimalityClaimed,
            vec![1],
            2,
            Some(2),
            false,
            true,
            SolveStatus::Feasible,
        ),
        (
            OptimalityClaimed,
            vec![2],
            2,
            Some(1),
            false,
            true,
            SolveStatus::Feasible,
        ),
        (
            OptimalityClaimed,
            vec![1],
            1,
            Some(1),
            false,
            false,
            SolveStatus::Feasible,
        ),
        (
            TimeLimit,
            vec![1],
            1,
            Some(1),
            false,
            true,
            SolveStatus::Feasible,
        ),
        (
            SolutionLimit,
            vec![1],
            1,
            Some(1),
            false,
            true,
            SolveStatus::Feasible,
        ),
    ] {
        let clock = FixedMonotonicClock::default();
        let registry = registry(
            vec![(
                "tests.a",
                Behavior::Proof {
                    termination,
                    candidates,
                    final_value,
                    bound,
                },
                false,
            )],
            &clock,
        )?;
        let mut fixture = problem()?;
        let provenance = ProvenanceId::new("tests.provenance")?;
        fixture.objectives.levels.push(ObjectiveLevel {
            id: ObjectiveLevelId::new("tests.level")?,
            direction: OptimizationDirection::Minimize,
            lower_bound: 1,
            upper_bound: 3,
            terms: vec![ObjectiveTerm {
                id: ObjectiveTermId::new("tests.objective")?,
                expression: LinearExpression::new(
                    vec![LinearTerm {
                        variable: IntVariableId::new("tests.left")?,
                        coefficient: 1,
                    }],
                    0,
                )?,
                kind: ObjectiveTermKind::Penalty,
                category: ScoreCategoryId::new("tests.score")?,
                provenance: provenance.clone(),
            }],
            provenance,
        });
        fixture.canonicalize()?;
        let result = run(
            &registry,
            fixture,
            &clock,
            &mut Review {
                skip_first,
                matches: matched,
            },
        )
        .await?;
        assert_eq!(
            result.terminal_status, expected,
            "{termination:?}, skip_first={skip_first}, matched={matched}, bound={bound:?}"
        );
        assert!(result.selected_candidate.is_some());
    }
    Ok(())
}

#[tokio::test]
async fn candidate_review_obeys_parent_deadline_before_acceptance() -> TestResult {
    let clock = FixedMonotonicClock::default();
    let registry = registry(
        vec![(
            "tests.a",
            Behavior::Outcome(BackendTerminationReason::CandidateFound, true, 1, 0),
            false,
        )],
        &clock,
    )?;
    let mut reviewer = ExpiringReview {
        clock: clock.clone(),
    };
    let result = run(&registry, problem()?, &clock, &mut reviewer).await?;
    assert_eq!(
        result.terminal_reason,
        ExecutionTerminalReason::ParentDeadlineExceeded
    );
    assert_eq!(result.terminal_status, SolveStatus::NoSolutionWithinLimit);
    assert!(result.selected_candidate.is_none());
    assert!(result.first_verified_feasible_milliseconds.is_none());
    assert_eq!(
        result.attempts[0].termination,
        AttemptTermination::ReviewDeadlineExceeded
    );
    Ok(())
}

#[tokio::test]
async fn execution_record_persists_deterministic_routed_request_and_outcome_evidence() -> TestResult
{
    let behavior = Behavior::Outcome(BackendTerminationReason::InfeasibilityClaimed, false, 1, 0);
    let first_clock = FixedMonotonicClock::default();
    let first_registry = registry(vec![("tests.a", behavior.clone(), false)], &first_clock)?;
    let second_clock = FixedMonotonicClock::default();
    let second_registry = registry(vec![("tests.a", behavior, false)], &second_clock)?;
    let mut first_reviewer = RequireIndependentVerification;
    let first = run(
        &first_registry,
        problem()?,
        &first_clock,
        &mut first_reviewer,
    )
    .await?;
    let mut second_reviewer = RequireIndependentVerification;
    let second = run(
        &second_registry,
        problem()?,
        &second_clock,
        &mut second_reviewer,
    )
    .await?;

    let first_json = serde_json::to_vec(&first)?;
    assert_eq!(first_json, serde_json::to_vec(&second)?);
    let round_trip: RouterExecutionRecord = serde_json::from_slice(&first_json)?;
    assert_eq!(round_trip, first);
    assert_eq!(first.solve_options.backend, BackendSelection::Auto);
    assert_eq!(first.attempts.len(), 1);
    let attempt = &first.attempts[0];
    assert_eq!(attempt.backend_version, "1.0.0");
    assert_eq!(attempt.adapter_version, "1.0.0");
    assert_eq!(attempt.solve_fingerprint.len(), 64);
    let outcome = attempt
        .outcome
        .as_ref()
        .ok_or("validated backend outcome must be retained")?;
    assert_eq!(outcome.solve_fingerprint, attempt.solve_fingerprint);
    assert_eq!(
        outcome.model_hash,
        first
            .decision
            .summary
            .as_ref()
            .ok_or("valid decision summary must be retained")?
            .canonical_ir_hash
    );
    assert_eq!(outcome.evidence.first_incumbent_milliseconds, None);
    Ok(())
}
