use crate::*;
use eutheto_domain_api::*;
use eutheto_domain_ir::*;
use eutheto_planning_ir::*;
use eutheto_solver_api::{BackendCandidate, BackendObjectiveEvidence};
use eutheto_types::*;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};

const SCENARIO_ID: &str = "01890a5d-ac96-7b64-9f74-bbfcf30f9f80";
const OTHER_SCENARIO_ID: &str = "01890a5d-ac96-7b64-9f74-bbfcf30f9f81";
const SOLUTION_ID: &str = "01890a5d-ac96-7b64-9f74-bbfcf30f9f82";
const OTHER_SOLUTION_ID: &str = "01890a5d-ac96-7b64-9f74-bbfcf30f9f85";
const REQUIRED_RULE_ID: &str = "01890a5d-ac96-7b64-9f74-bbfcf30f9f83";
const EXTRA_RULE_ID: &str = "01890a5d-ac96-7b64-9f74-bbfcf30f9f84";
const REVISION: u64 = 4;
const AUTHORITATIVE_SCORE: i64 = 4;

#[derive(Clone, Copy, Debug, Default)]
enum ProjectionMutation {
    #[default]
    None,
    Reject,
    MissingAssignment,
    ExtraAssignment,
    WrongValueKind,
    WrongSameKindValue,
    StaleScenario,
    StaleRevision,
    StaleProjection,
    WrongEntity,
    WrongSolutionId,
    AdditionalEvidence,
}

#[derive(Clone, Copy, Debug, Default)]
enum ScoreMutation {
    #[default]
    None,
    Reject,
    NonzeroFeasibility,
    WrongLevelId,
    WrongDirection,
    OutOfBounds,
    UndeclaredCategory,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ReportMutation {
    #[default]
    None,
    Reject,
    OmitRequiredRule,
    DuplicateRequiredRule,
    ExtraRequiredRule,
    FailedRequiredRule,
    ReplaceAuthoritativeScore,
    WrongModelBinding,
    WrongScopeBinding,
    InvalidChecksum,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InterruptStage {
    Projection,
    Scope,
    Score,
    Verification,
}

#[derive(Default)]
#[allow(clippy::struct_field_names)]
struct TestPack {
    projection_mutation: ProjectionMutation,
    score_mutation: ScoreMutation,
    report_mutation: ReportMutation,
    fail_scope: bool,
    interrupt_at: Option<(InterruptStage, DomainPackError)>,
}

impl TestPack {
    fn interrupt(&self, stage: InterruptStage) -> Result<(), DomainPackError> {
        match &self.interrupt_at {
            Some((configured, error)) if *configured == stage => Err(error.clone()),
            _ => Ok(()),
        }
    }
}

fn contract(error: impl std::fmt::Display) -> DomainPackError {
    DomainPackError::Contract(error.to_string())
}

fn unsupported<T>() -> Result<T, DomainPackError> {
    Err(DomainPackError::Contract(
        "operation is not part of the verifier fixture".to_owned(),
    ))
}

fn explanation_capability(kind: ExplanationKind) -> ExplanationCapability {
    match kind {
        ExplanationKind::Validation => ExplanationCapability::Validation,
        ExplanationKind::Infeasibility => ExplanationCapability::Infeasibility,
        ExplanationKind::Assignment => ExplanationCapability::Assignment,
        ExplanationKind::Counterfactual => ExplanationCapability::Counterfactual,
        ExplanationKind::SolutionDifference => ExplanationCapability::SolutionDifference,
        ExplanationKind::Repair => ExplanationCapability::Repair,
        ExplanationKind::OptimalityStatus => ExplanationCapability::OptimalityStatus,
    }
}

impl DomainPack for TestPack {
    fn descriptor(&self) -> Result<DomainPackDescriptor, DomainPackError> {
        unsupported()
    }

    fn catalog(&self) -> Result<DomainCatalog, DomainPackError> {
        unsupported()
    }

    fn new_document(&self, _shell: ScenarioDocument) -> Result<ScenarioDocument, DomainPackError> {
        unsupported()
    }

    fn migrate_document(
        &self,
        _document: ScenarioDocument,
    ) -> Result<ScenarioDocument, DomainPackError> {
        unsupported()
    }

    fn validate_fast(&self, _document: &ScenarioDocument) -> DomainValidationReport {
        DomainValidationReport::default()
    }

    fn validate_full(
        &self,
        _document: &ScenarioDocument,
        control: &OperationControl,
    ) -> Result<DomainValidationReport, DomainPackError> {
        control.check()?;
        Ok(DomainValidationReport::default())
    }

    fn apply_batch(
        &self,
        _document: &ScenarioDocument,
        _batch: &DomainBatchCommand,
        _cancellation: &CancellationToken,
    ) -> Result<DomainMutation, DomainPackError> {
        unsupported()
    }

    fn compile(
        &self,
        _document: &ScenarioDocument,
        _context: &CompileContext,
    ) -> Result<PlanningProblem, DomainPackError> {
        unsupported()
    }

    fn project(
        &self,
        problem: &PlanningProblem,
        candidate: &CandidateValues,
        solution_id: SolutionId,
        control: &OperationControl,
    ) -> Result<NormalizedSolution, DomainPackError> {
        control.check()?;
        self.interrupt(InterruptStage::Projection)?;
        let mut solution =
            project_candidate(problem, candidate, solution_id, PlanningIrLimitsV1::DEFAULT)
                .map_err(contract)?;
        match self.projection_mutation {
            ProjectionMutation::Reject => {
                return Err(contract("injected projection failure"));
            }
            ProjectionMutation::None => {}
            ProjectionMutation::MissingAssignment => solution.assignments.clear(),
            ProjectionMutation::ExtraAssignment => {
                solution.assignments.push(DomainAssignment {
                    id: DomainAssignmentId::new("assignment.extra").map_err(contract)?,
                    entity: entity("entity.extra")?,
                    value: AssignmentValue::Integer(1),
                    evidence: Vec::new(),
                });
                solution.canonicalize().map_err(contract)?;
            }
            ProjectionMutation::WrongValueKind => {
                let assignment = solution.assignments.first_mut().ok_or_else(|| {
                    DomainPackError::Contract("missing fixture assignment".to_owned())
                })?;
                assignment.value = AssignmentValue::Boolean(true);
            }
            ProjectionMutation::WrongSameKindValue => {
                let assignment = solution.assignments.first_mut().ok_or_else(|| {
                    DomainPackError::Contract("missing fixture assignment".to_owned())
                })?;
                let AssignmentValue::Integer(value) = &mut assignment.value else {
                    return Err(contract("fixture assignment is not integer-valued"));
                };
                *value += 1;
            }
            ProjectionMutation::StaleScenario => {
                solution.scenario_id = OTHER_SCENARIO_ID.parse().map_err(contract)?;
            }
            ProjectionMutation::StaleRevision => solution.scenario_revision -= 1,
            ProjectionMutation::StaleProjection => solution.projection_version += 1,
            ProjectionMutation::WrongEntity => {
                let assignment = solution.assignments.first_mut().ok_or_else(|| {
                    DomainPackError::Contract("missing fixture assignment".to_owned())
                })?;
                assignment.entity = entity("entity.wrong")?;
            }
            ProjectionMutation::WrongSolutionId => {
                solution.solution_id = OTHER_SOLUTION_ID.parse().map_err(contract)?;
            }
            ProjectionMutation::AdditionalEvidence => {
                let assignment = solution.assignments.first_mut().ok_or_else(|| {
                    DomainPackError::Contract("missing fixture assignment".to_owned())
                })?;
                assignment
                    .evidence
                    .push(DomainEvidenceId::new("evidence.pack.detail").map_err(contract)?);
                solution.canonicalize().map_err(contract)?;
            }
        }
        Ok(solution)
    }

    fn verification_scope(
        &self,
        document: &ScenarioDocument,
        scenario_revision: u64,
        control: &OperationControl,
    ) -> Result<VerificationScope, DomainPackError> {
        control.check()?;
        self.interrupt(InterruptStage::Scope)?;
        if self.fail_scope {
            return Err(contract("injected verification scope failure"));
        }
        VerificationScope::new(
            document.scenario_id,
            scenario_revision,
            vec![RequiredRuleBinding {
                rule_id: required_rule_id()?,
                semantic_hash: blake3_hex(b"fixture required rule"),
            }],
        )
        .map_err(contract)
    }

    fn verify(
        &self,
        _document: &ScenarioDocument,
        _solution: &NormalizedSolution,
        context: &VerificationContextV1,
        authoritative_score: &ScoreVector,
        control: &OperationControl,
    ) -> Result<VerificationReport, DomainPackError> {
        control.check()?;
        self.interrupt(InterruptStage::Verification)?;
        if self.report_mutation == ReportMutation::Reject {
            return Err(contract("injected verifier failure"));
        }
        let mut report_context = context.clone();
        match self.report_mutation {
            ReportMutation::WrongModelBinding => {
                report_context.planning_model_hash = blake3_hex(b"wrong planning model");
            }
            ReportMutation::WrongScopeBinding => {
                report_context.verification_scope_checksum = blake3_hex(b"wrong scope");
            }
            _ => {}
        }

        let satisfied = self.report_mutation != ReportMutation::FailedRequiredRule;
        let required = evaluation(required_rule_id()?, satisfied);
        let evaluations = match self.report_mutation {
            ReportMutation::OmitRequiredRule => Vec::new(),
            ReportMutation::ExtraRequiredRule => {
                vec![required, evaluation(extra_rule_id()?, true)]
            }
            _ => vec![required],
        };
        let mut report_score = authoritative_score.clone();
        if self.report_mutation == ReportMutation::ReplaceAuthoritativeScore {
            let level = report_score.levels.first_mut().ok_or_else(|| {
                DomainPackError::Contract("missing fixture score level".to_owned())
            })?;
            level.value += 1;
        }
        let mut report = VerificationReport::new(
            &report_context,
            evaluations,
            report_score,
            Vec::new(),
            BTreeMap::new(),
        )
        .map_err(contract)?;
        if self.report_mutation == ReportMutation::DuplicateRequiredRule {
            let duplicate = report
                .required_rule_results
                .first()
                .cloned()
                .ok_or_else(|| {
                    DomainPackError::Contract("missing fixture rule report".to_owned())
                })?;
            report.required_rule_results.push(duplicate);
        }
        if self.report_mutation == ReportMutation::InvalidChecksum {
            report.checksum = blake3_hex(b"tampered verification report");
        }
        Ok(report)
    }

    fn score(
        &self,
        _document: &ScenarioDocument,
        solution: &NormalizedSolution,
        control: &OperationControl,
    ) -> Result<ScoreVector, DomainPackError> {
        control.check()?;
        self.interrupt(InterruptStage::Score)?;
        let value = match solution
            .assignments
            .first()
            .map(|assignment| &assignment.value)
        {
            Some(AssignmentValue::Integer(value)) => *value,
            _ => {
                return Err(DomainPackError::Contract(
                    "fixture score input is invalid".to_owned(),
                ));
            }
        };
        let mut score = ScoreVector {
            feasibility: 0,
            levels: vec![ScoreLevelValue {
                level_id: ScoreLevelId::new("objective.primary").map_err(contract)?,
                value,
                direction: OptimizationDirection::Minimize,
                category_breakdown: BTreeMap::from([(
                    ScoreCategoryId::new("score.value").map_err(contract)?,
                    -7,
                )]),
            }],
        };
        match self.score_mutation {
            ScoreMutation::Reject => return Err(contract("injected score failure")),
            ScoreMutation::None => {}
            ScoreMutation::NonzeroFeasibility => score.feasibility = 1,
            ScoreMutation::WrongLevelId => {
                score.levels[0].level_id =
                    ScoreLevelId::new("objective.wrong").map_err(contract)?;
            }
            ScoreMutation::WrongDirection => {
                score.levels[0].direction = OptimizationDirection::Maximize;
            }
            ScoreMutation::OutOfBounds => score.levels[0].value = 11,
            ScoreMutation::UndeclaredCategory => {
                score.levels[0].category_breakdown.insert(
                    ScoreCategoryId::new("score.undeclared").map_err(contract)?,
                    1,
                );
            }
        }
        Ok(score)
    }

    fn export_portable(
        &self,
        _document: &ScenarioDocument,
    ) -> Result<PortableDomainDocument, DomainPackError> {
        unsupported()
    }

    fn migrate_portable_step(
        &self,
        _document: PortableDomainDocument,
    ) -> Result<PortableDomainDocument, DomainPackError> {
        unsupported()
    }

    fn import_portable(
        &self,
        _document: &PortableDomainDocument,
        _context: &PortableImportContext,
    ) -> Result<ScenarioDocument, DomainPackError> {
        unsupported()
    }

    fn build_share_result(
        &self,
        _document: &ScenarioDocument,
        _accepted: &AcceptedResult,
        _options: ShareResultOptions,
    ) -> Result<DomainShareResult, DomainPackError> {
        unsupported()
    }

    fn build_view(
        &self,
        _document: &ScenarioDocument,
        _solution: Option<&NormalizedSolution>,
        _view_id: &str,
    ) -> Result<DomainView, DomainPackError> {
        unsupported()
    }

    fn render_evidence(
        &self,
        _document: &ScenarioDocument,
        request: &EvidenceRenderRequestV1,
    ) -> Result<EvidenceRenderResultV1, DomainPackError> {
        Err(DomainPackError::UnsupportedExplanationCapability(
            explanation_capability(request.kind),
        ))
    }

    fn compile_counterfactual(
        &self,
        _document: &ScenarioDocument,
        _condition: &CounterfactualConditionV1,
        _context: &CounterfactualCompileContext<'_>,
    ) -> Result<PlanningProblem, DomainPackError> {
        Err(DomainPackError::UnsupportedExplanationCapability(
            ExplanationCapability::Counterfactual,
        ))
    }
}

struct SequenceClock {
    times: Vec<DurationMillis>,
    next: AtomicUsize,
}

impl SequenceClock {
    fn new(values: &[u64]) -> Result<Self, Box<dyn Error>> {
        let times = values
            .iter()
            .copied()
            .map(DurationMillis::new)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            times,
            next: AtomicUsize::new(0),
        })
    }
}

impl VerificationClock for SequenceClock {
    fn now_milliseconds(&self) -> DurationMillis {
        let index = self.next.fetch_add(1, Ordering::Relaxed);
        self.times
            .get(index)
            .copied()
            .unwrap_or(DurationMillis::MAX)
    }
}

fn entity(id: &str) -> Result<DomainEntityRef, DomainPackError> {
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new("entity.fixture").map_err(contract)?,
        id: DomainEntityId::new(id).map_err(contract)?,
    })
}

fn required_rule_id() -> Result<RuleId, DomainPackError> {
    REQUIRED_RULE_ID.parse().map_err(contract)
}

fn extra_rule_id() -> Result<RuleId, DomainPackError> {
    EXTRA_RULE_ID.parse().map_err(contract)
}

fn evaluation(rule_id: RuleId, satisfied: bool) -> RuleEvaluation {
    RuleEvaluation {
        rule_id,
        satisfied,
        affected_entities: Vec::new(),
        message_key: "fixture.rule.evaluation".to_owned(),
        expected: BTreeMap::new(),
        observed: BTreeMap::new(),
        evidence: Vec::new(),
    }
}

fn planning_problem() -> Result<PlanningProblem, Box<dyn Error>> {
    let provenance = ProvenanceId::new("provenance.fixture")?;
    let value_id = IntVariableId::new("int.value")?;
    let mut problem = PlanningProblem {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables: vec![Variable::Integer(IntVariable {
            id: value_id.clone(),
            domain: IntDomain::new(vec![InclusiveRange { start: 0, end: 10 }])?,
            provenance: provenance.clone(),
        })],
        constraints: Vec::new(),
        objectives: ObjectivePlan {
            levels: vec![ObjectiveLevel {
                id: ObjectiveLevelId::new("objective.primary")?,
                direction: OptimizationDirection::Minimize,
                lower_bound: 0,
                upper_bound: 10,
                terms: vec![ObjectiveTerm {
                    id: ObjectiveTermId::new("objective.term.value")?,
                    expression: LinearExpression::new(
                        vec![LinearTerm {
                            variable: value_id.clone(),
                            coefficient: 1,
                        }],
                        0,
                    )?,
                    kind: ObjectiveTermKind::Penalty,
                    category: ScoreCategoryId::new("score.value")?,
                    provenance: provenance.clone(),
                }],
                provenance: provenance.clone(),
            }],
        },
        assumptions: Vec::new(),
        projections: vec![SolutionProjection {
            id: ProjectionId::new("projection.value")?,
            assignment_id: DomainAssignmentId::new("assignment.value")?,
            entity: entity("entity.value").map_err(io::Error::other)?,
            required: true,
            expression: ProjectionExpression::Integer(value_id),
            provenance: provenance.clone(),
        }],
        provenance: vec![ProvenanceRecord {
            id: provenance,
            source_kind: ProvenanceSourceKind::Fact,
            source_id: "fixture.value".to_owned(),
            entity_refs: Vec::new(),
            message_key: "fixture.value".to_owned(),
            parameters: BTreeMap::new(),
            parent: None,
        }],
        metadata: PlanningMetadata {
            pack_id: PackId::new("official.verify-fixture")?,
            scenario_id: SCENARIO_ID.parse()?,
            scenario_revision: REVISION,
            projection_version: PROJECTION_SCHEMA_VERSION,
            compiler_id: CompilerId::new("compiler.verify-fixture")?,
            compiler_version: "1.0.0".to_owned(),
            compile_metadata: BTreeMap::new(),
            display_text: BTreeMap::new(),
        },
        declared_capabilities: BTreeSet::from([Capability::ObjectivePenalty]),
        split_authorization: None,
    };
    problem.canonicalize()?;
    Ok(problem)
}

fn document() -> Result<ScenarioDocument, Box<dyn Error>> {
    let start: Rfc3339Timestamp = "2026-09-01T00:00:00Z".parse()?;
    let end: Rfc3339Timestamp = "2026-09-02T00:00:00Z".parse()?;
    Ok(ScenarioDocument::new(
        SCENARIO_ID.parse()?,
        DomainPackRef {
            id: PackId::new("official.verify-fixture")?,
            schema_version: 1,
        },
        ScenarioMetadata {
            title: "Verifier fixture".to_owned(),
            description: String::new(),
            created_at: start,
            updated_at: start,
        },
        ScenarioSettings {
            time_zone: "UTC".parse()?,
            locale: "en-US".parse()?,
            units: UnitSystem::Metric,
            horizon: Horizon::new(start, end)?,
            gap_policy: GapPolicy::Reject,
            overlap_policy: OverlapPolicy::Earlier,
        },
        ScenarioDomain::default(),
        BTreeMap::new(),
    ))
}

fn candidate(objective: Option<Vec<i64>>) -> Result<BackendCandidate, Box<dyn Error>> {
    Ok(BackendCandidate {
        sequence: 1,
        values: CandidateValues {
            booleans: BTreeMap::new(),
            integers: BTreeMap::from([(IntVariableId::new("int.value")?, AUTHORITATIVE_SCORE)]),
        },
        observed_after_milliseconds: DurationMillis::new(2)?,
        objective: objective.map(|objective_values| BackendObjectiveEvidence {
            objective_values,
            best_bound_values: None,
        }),
        evidence_refs: Vec::new(),
    })
}

fn review(
    pack: &TestPack,
    objective: Option<Vec<i64>>,
    clock_values: &[u64],
) -> Result<AcceptanceDecision, Box<dyn Error>> {
    let document = document()?;
    let problem = planning_problem()?;
    let clock = SequenceClock::new(clock_values)?;
    let reviewer = AcceptanceReviewer::new(pack, &document, REVISION, &problem, &clock)
        .map_err(|alarm| io::Error::other(format!("reviewer construction failed: {alarm:?}")))?;
    Ok(reviewer.review(
        &candidate(objective)?,
        SOLUTION_ID.parse()?,
        &OperationControl::Cancellation(CancellationToken::new()),
    ))
}

fn accepted(
    decision: AcceptanceDecision,
) -> Result<
    (
        AcceptedResult,
        BackendObjectiveReconciliation,
        AcceptancePhaseTimings,
    ),
    io::Error,
> {
    match decision {
        AcceptanceDecision::Accepted {
            result,
            objective_reconciliation,
            timings,
        } => Ok((*result, objective_reconciliation, timings)),
        other => Err(io::Error::other(format!(
            "expected accepted candidate, got {other:?}"
        ))),
    }
}

fn quarantined(
    decision: AcceptanceDecision,
    category: CorrectnessAlarmCategory,
) -> Result<AcceptancePhaseTimings, io::Error> {
    match decision {
        AcceptanceDecision::Quarantined { alarm, timings } => {
            assert_eq!(alarm.category, category);
            assert_eq!(alarm.diagnostic_code, category.code());
            Ok(timings)
        }
        other => Err(io::Error::other(format!(
            "expected {category:?} quarantine, got {other:?}"
        ))),
    }
}

fn assert_quarantined(
    decision: AcceptanceDecision,
    category: CorrectnessAlarmCategory,
) -> Result<(), io::Error> {
    quarantined(decision, category).map(drop)
}

#[test]
fn valid_candidate_uses_authoritative_score_and_parent_stage_timings() -> Result<(), Box<dyn Error>>
{
    let (result, reconciliation, timings) = accepted(review(
        &TestPack::default(),
        Some(vec![AUTHORITATIVE_SCORE]),
        &[10, 13, 18, 25, 36],
    )?)?;

    assert_eq!(reconciliation, BackendObjectiveReconciliation::Matched);
    assert_eq!(result.verification.score.feasibility, 0);
    assert_eq!(
        result.verification.score.levels[0].value,
        AUTHORITATIVE_SCORE
    );
    assert_eq!(
        timings,
        AcceptancePhaseTimings {
            projection_milliseconds: DurationMillis::new(3)?,
            structural_validation_milliseconds: DurationMillis::new(5)?,
            score_recomputation_milliseconds: DurationMillis::new(7)?,
            required_rule_verification_milliseconds: DurationMillis::new(11)?,
        }
    );
    Ok(())
}

#[test]
fn backend_objective_evidence_is_reconciled_without_gating_or_persistence()
-> Result<(), Box<dyn Error>> {
    for (objective, expected) in [
        (None, BackendObjectiveReconciliation::Missing),
        (Some(vec![99]), BackendObjectiveReconciliation::Mismatch),
    ] {
        let (result, reconciliation, _) =
            accepted(review(&TestPack::default(), objective, &[0, 1, 2, 3, 4])?)?;
        assert_eq!(reconciliation, expected);
        assert_eq!(
            result.verification.score.levels[0].value,
            AUTHORITATIVE_SCORE
        );
        assert_eq!(
            result.verification.score.levels[0]
                .category_breakdown
                .get(&ScoreCategoryId::new("score.value")?),
            Some(&-7)
        );
        let persisted = serde_json::to_value(&result)?;
        assert!(
            !persisted.to_string().contains("objectiveValues"),
            "accepted results must not persist raw backend objective evidence"
        );
    }
    Ok(())
}

#[test]
fn structural_projection_mutations_are_quarantined() -> Result<(), Box<dyn Error>> {
    for projection_mutation in [
        ProjectionMutation::MissingAssignment,
        ProjectionMutation::ExtraAssignment,
        ProjectionMutation::WrongValueKind,
        ProjectionMutation::WrongSameKindValue,
        ProjectionMutation::StaleScenario,
        ProjectionMutation::StaleRevision,
        ProjectionMutation::StaleProjection,
        ProjectionMutation::WrongEntity,
        ProjectionMutation::WrongSolutionId,
    ] {
        let pack = TestPack {
            projection_mutation,
            ..TestPack::default()
        };
        assert_quarantined(
            review(&pack, Some(vec![AUTHORITATIVE_SCORE]), &[0, 1, 2])?,
            CorrectnessAlarmCategory::StructuralValidationFailed,
        )?;
    }
    Ok(())
}

#[test]
fn non_authoritative_projection_evidence_does_not_block_acceptance() -> Result<(), Box<dyn Error>> {
    let pack = TestPack {
        projection_mutation: ProjectionMutation::AdditionalEvidence,
        ..TestPack::default()
    };
    let (result, _, _) = accepted(review(
        &pack,
        Some(vec![AUTHORITATIVE_SCORE]),
        &[0, 1, 2, 3, 4],
    )?)?;
    assert_eq!(
        result.solution.assignments[0].evidence,
        vec![
            DomainEvidenceId::new("evidence.pack.detail")?,
            DomainEvidenceId::new("provenance.fixture")?,
        ]
    );
    Ok(())
}

#[test]
fn failed_stages_record_parent_observed_elapsed_time() -> Result<(), Box<dyn Error>> {
    let projection = quarantined(
        review(
            &TestPack {
                projection_mutation: ProjectionMutation::Reject,
                ..TestPack::default()
            },
            None,
            &[10, 14],
        )?,
        CorrectnessAlarmCategory::ProjectionFailed,
    )?;
    assert_eq!(projection.projection_milliseconds, DurationMillis::new(4)?);

    let structural = quarantined(
        review(
            &TestPack {
                projection_mutation: ProjectionMutation::WrongSameKindValue,
                ..TestPack::default()
            },
            None,
            &[10, 11, 15],
        )?,
        CorrectnessAlarmCategory::StructuralValidationFailed,
    )?;
    assert_eq!(
        structural.structural_validation_milliseconds,
        DurationMillis::new(4)?
    );

    let score = quarantined(
        review(
            &TestPack {
                score_mutation: ScoreMutation::Reject,
                ..TestPack::default()
            },
            None,
            &[10, 11, 12, 18],
        )?,
        CorrectnessAlarmCategory::ScoreRecomputationFailed,
    )?;
    assert_eq!(
        score.score_recomputation_milliseconds,
        DurationMillis::new(6)?
    );

    let scope_timing = quarantined(
        review(
            &TestPack {
                fail_scope: true,
                ..TestPack::default()
            },
            None,
            &[10, 11, 12, 13, 21],
        )?,
        CorrectnessAlarmCategory::VerificationScopeFailed,
    )?;
    assert_eq!(
        scope_timing.required_rule_verification_milliseconds,
        DurationMillis::new(8)?
    );

    let verification = quarantined(
        review(
            &TestPack {
                report_mutation: ReportMutation::Reject,
                ..TestPack::default()
            },
            None,
            &[10, 11, 12, 13, 22],
        )?,
        CorrectnessAlarmCategory::RequiredRuleVerificationFailed,
    )?;
    assert_eq!(
        verification.required_rule_verification_milliseconds,
        DurationMillis::new(9)?
    );
    Ok(())
}

#[test]
fn required_rule_report_mutations_are_quarantined_by_stable_category() -> Result<(), Box<dyn Error>>
{
    for (report_mutation, category) in [
        (
            ReportMutation::OmitRequiredRule,
            CorrectnessAlarmCategory::RequiredRuleCoverageFailed,
        ),
        (
            ReportMutation::DuplicateRequiredRule,
            CorrectnessAlarmCategory::ReportBindingFailed,
        ),
        (
            ReportMutation::ExtraRequiredRule,
            CorrectnessAlarmCategory::RequiredRuleCoverageFailed,
        ),
        (
            ReportMutation::FailedRequiredRule,
            CorrectnessAlarmCategory::RequiredRuleRejected,
        ),
    ] {
        let pack = TestPack {
            report_mutation,
            ..TestPack::default()
        };
        assert_quarantined(
            review(&pack, Some(vec![AUTHORITATIVE_SCORE]), &[0, 1, 2, 3, 4])?,
            category,
        )?;
    }
    Ok(())
}

#[test]
fn score_shape_and_feasibility_mutations_are_quarantined() -> Result<(), Box<dyn Error>> {
    for score_mutation in [
        ScoreMutation::NonzeroFeasibility,
        ScoreMutation::WrongLevelId,
        ScoreMutation::WrongDirection,
        ScoreMutation::OutOfBounds,
        ScoreMutation::UndeclaredCategory,
    ] {
        let pack = TestPack {
            score_mutation,
            ..TestPack::default()
        };
        assert_quarantined(
            review(&pack, Some(vec![AUTHORITATIVE_SCORE]), &[0, 1, 2, 3, 4])?,
            CorrectnessAlarmCategory::ScoreIntegrityFailed,
        )?;
    }
    Ok(())
}

#[test]
fn verifier_cannot_replace_the_authoritative_recomputed_score() -> Result<(), Box<dyn Error>> {
    let pack = TestPack {
        report_mutation: ReportMutation::ReplaceAuthoritativeScore,
        ..TestPack::default()
    };
    assert_quarantined(
        review(&pack, Some(vec![AUTHORITATIVE_SCORE]), &[0, 1, 2, 3, 4])?,
        CorrectnessAlarmCategory::ScoreIntegrityFailed,
    )?;
    Ok(())
}

#[test]
fn report_checksum_and_immutable_bindings_are_quarantined() -> Result<(), Box<dyn Error>> {
    for report_mutation in [
        ReportMutation::WrongModelBinding,
        ReportMutation::WrongScopeBinding,
        ReportMutation::InvalidChecksum,
    ] {
        let pack = TestPack {
            report_mutation,
            ..TestPack::default()
        };
        assert_quarantined(
            review(&pack, Some(vec![AUTHORITATIVE_SCORE]), &[0, 1, 2, 3, 4])?,
            CorrectnessAlarmCategory::ReportBindingFailed,
        )?;
    }
    Ok(())
}

#[test]
fn reviewer_rejects_nonportable_revision_before_candidate_work() -> Result<(), Box<dyn Error>> {
    let document = document()?;
    let mut problem = planning_problem()?;
    problem.metadata.scenario_revision = REVISION_MAX_V1 + 1;
    let clock = SequenceClock::new(&[0])?;
    let pack = TestPack::default();
    let reviewer = AcceptanceReviewer::new(&pack, &document, REVISION_MAX_V1 + 1, &problem, &clock);
    let Err(alarm) = reviewer else {
        return Err("nonportable revision unexpectedly constructed an acceptance reviewer".into());
    };
    assert_eq!(
        alarm.category,
        CorrectnessAlarmCategory::InvalidPlanningProblem
    );
    Ok(())
}

#[test]
fn backward_parent_clock_is_quarantined() -> Result<(), Box<dyn Error>> {
    assert_quarantined(
        review(
            &TestPack::default(),
            Some(vec![AUTHORITATIVE_SCORE]),
            &[10, 9],
        )?,
        CorrectnessAlarmCategory::ClockFailed,
    )?;
    Ok(())
}

#[test]
fn pack_interruption_at_each_acceptance_stage_never_becomes_a_correctness_alarm()
-> Result<(), Box<dyn Error>> {
    for stage in [
        InterruptStage::Projection,
        InterruptStage::Scope,
        InterruptStage::Score,
        InterruptStage::Verification,
    ] {
        for (error, expected) in [
            (DomainPackError::Cancelled, OperationInterruption::Cancelled),
            (
                DomainPackError::BudgetExpired,
                OperationInterruption::DeadlineExceeded,
            ),
        ] {
            let pack = TestPack {
                interrupt_at: Some((stage, error)),
                ..TestPack::default()
            };
            let decision = review(
                &pack,
                Some(vec![AUTHORITATIVE_SCORE]),
                &[10, 13, 18, 25, 36],
            )?;
            let AcceptanceDecision::Interrupted { reason, timings } = decision else {
                return Err(format!("{stage:?}: interruption became {decision:?}").into());
            };
            assert_eq!(reason, expected);
            let expected_timings = match stage {
                InterruptStage::Projection => [3, 0, 0, 0],
                InterruptStage::Score => [3, 5, 7, 0],
                InterruptStage::Scope | InterruptStage::Verification => [3, 5, 7, 11],
            };
            assert_eq!(
                [
                    timings.projection_milliseconds.value(),
                    timings.structural_validation_milliseconds.value(),
                    timings.score_recomputation_milliseconds.value(),
                    timings.required_rule_verification_milliseconds.value(),
                ],
                expected_timings,
                "{stage:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn expiry_during_acceptance_prevents_an_accepted_result() -> Result<(), Box<dyn Error>> {
    use std::sync::Arc;
    use std::time::Duration;
    struct ExpiringClock {
        calls: AtomicUsize,
        stop_on: usize,
        parent_clock: FixedMonotonicClock,
    }
    impl VerificationClock for ExpiringClock {
        fn now_milliseconds(&self) -> DurationMillis {
            let call = self.calls.fetch_add(1, Ordering::Relaxed);
            if call == self.stop_on {
                assert!(self.parent_clock.advance(Duration::from_secs(1)).is_ok());
            }
            DurationMillis::ZERO
        }
    }
    let document = document()?;
    let problem = planning_problem()?;
    for stop_on in 0..5 {
        let parent_clock = FixedMonotonicClock::default();
        let budget = ParentSolveBudget::new(
            DurationMillis::new(1000)?,
            Arc::new(parent_clock.clone()),
            CancellationToken::new(),
        )?;
        let clock = ExpiringClock {
            calls: AtomicUsize::new(0),
            stop_on,
            parent_clock,
        };
        let pack = TestPack::default();
        let reviewer = AcceptanceReviewer::new(&pack, &document, REVISION, &problem, &clock)
            .map_err(|alarm| io::Error::other(format!("{alarm:?}")))?;
        let decision = reviewer.review(
            &candidate(Some(vec![AUTHORITATIVE_SCORE]))?,
            SOLUTION_ID.parse()?,
            &OperationControl::Solve(budget.phase_view()),
        );
        assert!(
            matches!(
                decision,
                AcceptanceDecision::Interrupted {
                    reason: OperationInterruption::DeadlineExceeded,
                    ..
                }
            ),
            "stage boundary {stop_on}: {decision:?}"
        );
    }
    Ok(())
}
