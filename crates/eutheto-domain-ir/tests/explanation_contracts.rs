use eutheto_domain_ir::*;
use eutheto_types::{
    BackendId, BackendSelection, CounterfactualJobId, DurationMillis, ExplanationMode,
    IanaTimeZone, PackId, PreservationPolicy, ReproducibilityMode, RequestId, ResourceLimits,
    Rfc3339Timestamp, ScenarioId, ScenarioSnapshotId, SolutionId, SolveMode, SolveOptions,
    SolveRunId, SolveStatus, WorkerThreadPolicy,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::error::Error;

fn uuid(suffix: u8) -> String {
    format!("01890a5d-ac96-7b64-9f74-bbfcf30f9f{suffix:02x}")
}

fn hash(label: &str) -> String {
    blake3_hex(label.as_bytes())
}

fn result_ref(suffix: u8) -> Result<AcceptedResultRefV1, Box<dyn Error>> {
    Ok(AcceptedResultRefV1 {
        solution_id: uuid(suffix).parse::<SolutionId>()?,
        result_checksum: hash(&format!("result-{suffix}")),
    })
}

fn score(value: i64) -> Result<ScoreVector, Box<dyn Error>> {
    Ok(ScoreVector {
        feasibility: 0,
        levels: vec![ScoreLevelValue {
            level_id: ScoreLevelId::new("score.primary")?,
            value,
            direction: OptimizationDirection::Minimize,
            category_breakdown: BTreeMap::new(),
        }],
    })
}

fn comparison() -> Result<SolutionComparisonV1, Box<dyn Error>> {
    let base = result_ref(1)?;
    let candidate = result_ref(2)?;
    let pack_id = PackId::new("official.synthetic")?;
    let scenario_id = uuid(0x80).parse::<ScenarioId>()?;
    let side = |revision, document: &str, accepted_result| ComparisonBindingV1 {
        pack_id: pack_id.clone(),
        scenario_id,
        scenario_revision: revision,
        document_hash: hash(document),
        projection_version: 1,
        verification_scope_checksum: hash(&format!("scope-{revision}")),
        accepted_result,
    };
    Ok(SolutionComparisonV1::new(
        side(7, "base-document", base),
        side(8, "candidate-document", candidate),
        score(4)?,
        score(4)?,
        Vec::new(),
        Vec::new(),
        vec![ScoreLevelComparisonV1 {
            level_id: ScoreLevelId::new("score.primary")?,
            direction: OptimizationDirection::Minimize,
            before: 4,
            after: 4,
            delta: 0,
            categories: Vec::new(),
        }],
        Vec::new(),
        Vec::new(),
        Some(RunComparisonV1 {
            base: RunComparisonSideV1 {
                run_id: uuid(3).parse()?,
                run_manifest_checksum: hash("base-manifest"),
                outcome: RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::InvalidModel,
                },
                certainty: ExplanationCertainty::Deterministic,
            },
            candidate: RunComparisonSideV1 {
                run_id: uuid(4).parse()?,
                run_manifest_checksum: hash("candidate-manifest"),
                outcome: RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::InvalidModel,
                },
                certainty: ExplanationCertainty::Deterministic,
            },
        }),
        Vec::new(),
        ComparisonOrdering::Equivalent,
    )?)
}
fn comparison_v2() -> Result<SolutionComparisonV2, Box<dyn Error>> {
    let source =
        PortableAcceptedResult::from_json(include_bytes!("fixtures/portable_accepted_v2.json"))?;
    let producer_id = DomainEvidenceId::new("tests.comparison.producer")?;
    let semantic_hash = hash("same-source-scoring-semantics");
    Ok(
        prepare_comparison_v2(&source, &source, None)?.finish(ComparisonEnrichmentV2 {
            scoring_contexts: Some((
                ComparisonScoringContext {
                    source: &source,
                    producer_id: producer_id.clone(),
                    semantic_hash: semantic_hash.clone(),
                },
                ComparisonScoringContext {
                    source: &source,
                    producer_id,
                    semantic_hash,
                },
            )),
            ..ComparisonEnrichmentV2::default()
        })?,
    )
}

fn solve_options() -> Result<SolveOptions, Box<dyn Error>> {
    Ok(SolveOptions {
        backend: BackendSelection::Auto,
        mode: SolveMode::Balanced,
        time_limit_milliseconds: DurationMillis::new(5_000)?,
        memory_limit_bytes: Some(64 * 1024 * 1024),
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 9,
        solution_limit: Some(1),
        stop_after_first_feasible: true,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::Standard,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: 100,
            max_rules: 100,
            max_variables: 1_000,
            max_constraints: 1_000,
        },
    })
}

fn rebuild_run_input(
    input: &RunInput,
    run_id: SolveRunId,
    compiler_version: String,
) -> Result<RunInput, DomainContractError> {
    RunInput::new(
        run_id,
        input.request_id,
        input.scenario_id,
        input.scenario_revision,
        input.snapshot_id,
        input.snapshot_document_hash.clone(),
        input.snapshot_created_at,
        input.pack_id.clone(),
        input.pack_schema_version,
        input.planning_ir_schema_version,
        compiler_version,
        input.application_version.clone(),
        input.backend_id.clone(),
        input.backend_version.clone(),
        input.adapter_version.clone(),
        input.worker_version.clone(),
        input.solver_version.clone(),
        input.protocol_major,
        input.protocol_minor,
        input.model_hash.clone(),
        input.objective_policy_hash.clone(),
        input.solve_options.clone(),
        input.scenario_timezone.clone(),
        input.temporary_condition_hash.clone(),
        input.objective_context.clone(),
    )
}

fn infeasible_manifest(input: &RunInput) -> Result<RunManifest, Box<dyn Error>> {
    Ok(RunManifest::new(
        input.run_id,
        input.checksum.clone(),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible,
        },
        "2026-09-03T12:00:01Z".parse()?,
        "2026-09-03T12:00:02Z".parse()?,
        Some(DurationMillis::new(1_000)?),
        None,
        None,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?)
}

struct CounterfactualFixture {
    result: CounterfactualResult,
    request: CounterfactualJobRequestV1,
}

fn counterfactual_fixture() -> Result<CounterfactualFixture, Box<dyn Error>> {
    counterfactual_fixture_with_capture(false)
}

#[allow(clippy::too_many_lines)]
fn counterfactual_fixture_with_capture(
    captured: bool,
) -> Result<CounterfactualFixture, Box<dyn Error>> {
    let condition =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: DomainAssignmentId::new("assignment.target")?,
            value: AssignmentValue::Boolean(true),
        })?;
    let base_model_hash = hash("base-model");
    let derived_model_hash = hash("derived-model");
    let objective_policy_hash = hash("objective-policy");
    let compilation = CounterfactualCompilationBindingV1::new(
        base_model_hash.clone(),
        condition.checksum.clone(),
        derived_model_hash.clone(),
        objective_policy_hash.clone(),
    )?;
    let scenario_id = uuid(0x80).parse::<ScenarioId>()?;
    let snapshot_id = uuid(0x11).parse::<ScenarioSnapshotId>()?;
    let snapshot_document_hash = hash("snapshot");
    let pack_id = PackId::new("official.synthetic")?;
    let base_run_id = uuid(0x14).parse::<SolveRunId>()?;
    let mut base_run_input = RunInput::new(
        base_run_id,
        uuid(0x16).parse::<RequestId>()?,
        scenario_id,
        7,
        snapshot_id,
        snapshot_document_hash.clone(),
        "2026-09-03T11:59:00Z".parse::<Rfc3339Timestamp>()?,
        pack_id.clone(),
        1,
        2,
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        BackendId::new("ortools.cp-sat")?,
        "9.15.6755".to_owned(),
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        "9.15.6755".to_owned(),
        1,
        0,
        base_model_hash.clone(),
        objective_policy_hash.clone(),
        solve_options()?,
        "UTC".parse::<IanaTimeZone>()?,
        None,
        None,
    )?;
    let base = result_ref(1)?;
    if captured {
        let mut chunks = vec!["x".repeat(1021); 512];
        chunks[511].pop();
        base_run_input.objective_context = Some(RunObjectiveContext {
            schema_version: 2,
            pack_id: pack_id.clone(),
            profile_id: ObjectiveProfileId::new("official.synthetic.objective.minimum")?,
            semantic_version: 1,
            operation: RunObjectiveOperation::Repair,
            external_baseline: None,
            baseline: Some(Box::new(RunBaselineBinding {
                original: RunBaselineOriginal {
                    entity: DomainEntityRef {
                        kind: DomainEntityKindId::new("synthetic.base")?,
                        id: DomainEntityId::new("synthetic.original")?,
                    },
                    pair_content_hash: hash("original-pairs"),
                    accepted_result: base.clone(),
                    source_scenario_id: scenario_id,
                    source_revision: 7,
                    source_document_hash: snapshot_document_hash.clone(),
                    source_run_id: base_run_id,
                    source_run_input_checksum: base_run_input.checksum.clone(),
                    source_run_manifest_checksum: hash("original-manifest"),
                },
                selection: RunBaselineSelection::OriginalAccepted {},
                selected_pair_content_hash: hash("original-pairs"),
                captured_original: Some(std::sync::Arc::new(json!(chunks))),
            })),
        });
        base_run_input = rebuild_run_input(&base_run_input, base_run_id, "1.0.0".to_owned())?;
    }
    let base_run_manifest = RunManifest::new(
        base_run_id,
        base_run_input.checksum.clone(),
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: base.solution_id,
            accepted_result_checksum: base.result_checksum.clone(),
            verification_checksum: hash("base-verification"),
        },
        "2026-09-03T11:59:01Z".parse()?,
        "2026-09-03T11:59:02Z".parse()?,
        Some(DurationMillis::new(1_000)?),
        Some(DurationMillis::new(500)?),
        Some(DurationMillis::new(800)?),
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    let run_id = uuid(0x10).parse::<SolveRunId>()?;
    let run_input = RunInput::new(
        run_id,
        uuid(0x12).parse::<RequestId>()?,
        scenario_id,
        7,
        snapshot_id,
        snapshot_document_hash.clone(),
        "2026-09-03T12:00:00Z".parse::<Rfc3339Timestamp>()?,
        pack_id,
        1,
        2,
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        BackendId::new("ortools.cp-sat")?,
        "9.15.6755".to_owned(),
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        "9.15.6755".to_owned(),
        1,
        0,
        derived_model_hash,
        objective_policy_hash.clone(),
        solve_options()?,
        "UTC".parse::<IanaTimeZone>()?,
        Some(condition.checksum.clone()),
        base_run_input.objective_context.clone(),
    )?;
    let run_manifest = RunManifest::new(
        run_id,
        run_input.checksum.clone(),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible,
        },
        "2026-09-03T12:00:01Z".parse()?,
        "2026-09-03T12:00:02Z".parse()?,
        Some(DurationMillis::new(1_000)?),
        None,
        None,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    let job_id = uuid(0x13).parse::<CounterfactualJobId>()?;
    let semantics = CounterfactualRequestSemanticsV1 {
        schema_version: COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION,
        scenario_id,
        scenario_revision: 7,
        snapshot_id,
        snapshot_document_hash,
        base,
        base_run_id,
        base_run_input_checksum: base_run_input.checksum.clone(),
        base_model_hash,
        objective_policy_hash,
        condition_checksum: condition.checksum.clone(),
        total_budget_milliseconds: DurationMillis::new(5_000)?,
    };
    let request = CounterfactualJobRequestV1::new(
        job_id,
        uuid(0x15).parse()?,
        semantics,
        condition,
        "2026-09-03T12:00:00Z".parse()?,
    )?;
    let result = CounterfactualResult::new(
        request.clone(),
        base_run_input,
        base_run_manifest,
        compilation,
        run_input,
        run_manifest,
        CounterfactualConclusionV1::ProvenImpossible,
    )?;
    Ok(CounterfactualFixture { result, request })
}

fn counterfactual_comparison(
    result: &CounterfactualResult,
    base: AcceptedResultRefV1,
    candidate: AcceptedResultRefV1,
) -> Result<SolutionComparisonV1, Box<dyn Error>> {
    let scope = hash("counterfactual-scope");
    let binding = |accepted_result| ComparisonBindingV1 {
        pack_id: result.run_input.pack_id.clone(),
        scenario_id: result.request.semantics.scenario_id,
        scenario_revision: result.request.semantics.scenario_revision,
        document_hash: result.request.semantics.snapshot_document_hash.clone(),
        projection_version: 1,
        verification_scope_checksum: scope.clone(),
        accepted_result,
    };
    Ok(SolutionComparisonV1::new(
        binding(base),
        binding(candidate),
        score(4)?,
        score(4)?,
        Vec::new(),
        Vec::new(),
        vec![ScoreLevelComparisonV1 {
            level_id: ScoreLevelId::new("score.primary")?,
            direction: OptimizationDirection::Minimize,
            before: 4,
            after: 4,
            delta: 0,
            categories: Vec::new(),
        }],
        Vec::new(),
        Vec::new(),
        None,
        Vec::new(),
        ComparisonOrdering::Equivalent,
    )?)
}

fn validation_issue(
    severity: ExplanationValidationSeverity,
) -> Result<ValidationIssueEvidenceV1, Box<dyn Error>> {
    Ok(ValidationIssueEvidenceV1 {
        issue_id: VerificationIssueId::new("validation.issue")?,
        severity,
        message_key: "validation.issue.message".to_owned(),
        parameters: BTreeMap::from([(
            VerificationFactId::new("fact.count")?,
            VerificationValue::Integer(2),
        )]),
        field_path: Some(vec![VerificationFactId::new("field.assignments")?]),
        entity: None,
        rule_id: None,
    })
}

fn assignment_evidence() -> Result<AssignmentEvidenceV1, Box<dyn Error>> {
    Ok(AssignmentEvidenceV1 {
        assignment: DomainAssignment {
            id: DomainAssignmentId::new("assignment.target")?,
            entity: DomainEntityRef {
                kind: DomainEntityKindId::new("entity.person")?,
                id: DomainEntityId::new("person.alice")?,
            },
            value: AssignmentValue::Boolean(true),
            evidence: Vec::new(),
        },
        related_rules: Vec::new(),
        score_contributions: vec![ScoreContributionV1 {
            evidence_id: DomainEvidenceId::new("contribution.primary")?,
            level_id: ScoreLevelId::new("score.primary")?,
            category_id: None,
            value: 4,
        }],
        metrics: BTreeMap::new(),
        lock_state: Some(AssignmentLockStateV1::Locked {
            value: AssignmentValue::Boolean(true),
        }),
    })
}

fn all_evidence() -> Result<Vec<ExplanationEvidenceV2>, Box<dyn Error>> {
    let counterfactual = counterfactual_fixture()?.result;
    let optimality_status = OptimalityStatusEvidence {
        schema_version: Some(2),
        run_input: counterfactual.run_input.clone(),
        run_manifest: counterfactual.run_manifest.clone(),
        result: None,
    };
    let comparison = comparison_v2()?;
    Ok(vec![
        ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Validation {
            issue: validation_issue(ExplanationValidationSeverity::MustFix)?,
        })?,
        ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Infeasibility {
            infeasibility: InfeasibilityEvidenceV1::Unavailable {
                reason: ConflictUnavailableReason::ConflictNotReturned,
            },
            condition: None,
        })?,
        ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Assignment {
            assignment: assignment_evidence()?,
        })?,
        ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Counterfactual {
            result: Box::new(counterfactual),
        })?,
        ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::SolutionDifference {
            comparison: Box::new(comparison.clone()),
        })?,
        ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Repair {
            repair: Box::new(RepairEvidenceV2 {
                comparison,
                causality: RepairCausalityV1::NotEstablished,
            }),
        })?,
        ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::OptimalityStatus {
            status: Box::new(optimality_status),
        })?,
    ])
}

#[test]
fn exact_seven_request_subjects_round_trip_strictly() -> Result<(), Box<dyn Error>> {
    let accepted = result_ref(1)?;
    let subjects = vec![
        ExplanationRequestSubjectV1::Validation { issue_id: None },
        ExplanationRequestSubjectV1::Infeasibility {
            solve_run_id: uuid(1).parse()?,
            run_manifest_checksum: hash("manifest"),
            conflict_id: Some(DomainEvidenceId::new("conflict.retained")?),
        },
        ExplanationRequestSubjectV1::Assignment {
            result: accepted.clone(),
            assignment_id: DomainAssignmentId::new("assignment.target")?,
        },
        ExplanationRequestSubjectV1::Counterfactual {
            job_id: uuid(2).parse()?,
            base: accepted.clone(),
        },
        ExplanationRequestSubjectV1::SolutionDifference {
            left: accepted.clone(),
            right: result_ref(2)?,
        },
        ExplanationRequestSubjectV1::Repair {
            current: result_ref(2)?,
            base: accepted.clone(),
        },
        ExplanationRequestSubjectV1::OptimalityStatus {
            solve_run_id: uuid(3).parse()?,
            run_manifest_checksum: hash("status-manifest"),
            result: Some(accepted),
        },
    ];
    let expected = [
        ExplanationKind::Validation,
        ExplanationKind::Infeasibility,
        ExplanationKind::Assignment,
        ExplanationKind::Counterfactual,
        ExplanationKind::SolutionDifference,
        ExplanationKind::Repair,
        ExplanationKind::OptimalityStatus,
    ];
    assert_eq!(subjects.len(), expected.len());
    for (subject, kind) in subjects.into_iter().zip(expected) {
        let request = ExplanationRequestV1::new(subject)?;
        assert_eq!(request.schema_version, EXPLANATION_REQUEST_SCHEMA_VERSION);
        assert_eq!(request.subject.kind(), kind);
        let bytes = serde_json::to_vec(&request)?;
        let wire = String::from_utf8(bytes.clone())?;
        for snake_case in [
            "issue_id",
            "solve_run_id",
            "run_manifest_checksum",
            "conflict_id",
            "assignment_id",
            "job_id",
        ] {
            assert!(!wire.contains(snake_case), "wire leaked `{snake_case}`");
        }
        match kind {
            ExplanationKind::Validation => assert!(wire.contains("\"issueId\"")),
            ExplanationKind::Infeasibility => {
                assert!(wire.contains("\"solveRunId\""));
                assert!(wire.contains("\"runManifestChecksum\""));
                assert!(wire.contains("\"conflictId\""));
            }
            ExplanationKind::Assignment => assert!(wire.contains("\"assignmentId\"")),
            ExplanationKind::Counterfactual => assert!(wire.contains("\"jobId\"")),
            ExplanationKind::SolutionDifference | ExplanationKind::Repair => {}
            ExplanationKind::OptimalityStatus => {
                assert!(wire.contains("\"solveRunId\""));
                assert!(wire.contains("\"runManifestChecksum\""));
            }
        }
        assert_eq!(ExplanationRequestV1::from_json(&bytes)?, request);
    }
    Ok(())
}

#[test]
fn exact_seven_evidence_and_result_discriminants_round_trip() -> Result<(), Box<dyn Error>> {
    assert_eq!(EXPLANATION_EVIDENCE_V2_SCHEMA_VERSION, 2);
    assert_eq!(EVIDENCE_RENDER_REQUEST_V2_SCHEMA_VERSION, 2);
    assert_eq!(EXPLANATION_RESULT_V2_SCHEMA_VERSION, 2);
    assert_eq!(EVIDENCE_RENDER_RESULT_SCHEMA_VERSION, 1);
    let expected = [
        ExplanationKind::Validation,
        ExplanationKind::Infeasibility,
        ExplanationKind::Assignment,
        ExplanationKind::Counterfactual,
        ExplanationKind::SolutionDifference,
        ExplanationKind::Repair,
        ExplanationKind::OptimalityStatus,
    ];
    for (evidence, kind) in all_evidence()?.into_iter().zip(expected) {
        assert_eq!(evidence.kind(), kind);
        assert_eq!(
            evidence.schema_version,
            EXPLANATION_EVIDENCE_V2_SCHEMA_VERSION
        );
        assert_eq!(
            ExplanationEvidenceV2::from_json(&serde_json::to_vec(&evidence)?)?,
            evidence
        );
        let request = EvidenceRenderRequestV2::new(evidence.clone())?;
        assert_eq!(
            request.schema_version,
            EVIDENCE_RENDER_REQUEST_V2_SCHEMA_VERSION
        );
        let rendered = EvidenceRenderResultV1::new(
            &request,
            vec![EvidenceMessageV1 {
                message_key: "evidence.summary".to_owned(),
                parameters: BTreeMap::new(),
                entities: Vec::new(),
                rules: Vec::new(),
                assignments: Vec::new(),
                evidence: Vec::new(),
            }],
        )?;
        assert_eq!(
            rendered.schema_version,
            EVIDENCE_RENDER_RESULT_SCHEMA_VERSION
        );
        let result = ExplanationResultV2::new(evidence, rendered)?;
        assert_eq!(result.schema_version, EXPLANATION_RESULT_V2_SCHEMA_VERSION);
        if kind == ExplanationKind::Validation {
            let mut tampered = result.clone();
            tampered.checksum = hash("tampered-result");
            assert_eq!(
                tampered.validate(),
                Err(DomainContractError::ChecksumMismatch)
            );
            let mut old_version = serde_json::to_value(&result)?;
            old_version["schemaVersion"] = json!(1);
            assert!(matches!(
                ExplanationResultV2::from_json(&serde_json::to_vec(&old_version)?),
                Err(DomainContractError::UnsupportedVersion(1))
            ));
        }
        assert_eq!(
            ExplanationResultV2::from_json(&serde_json::to_vec(&result)?)?,
            result
        );
    }
    Ok(())
}

#[test]
fn render_result_rejects_same_kind_evidence_substitution() -> Result<(), Box<dyn Error>> {
    let first = ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Validation {
        issue: validation_issue(ExplanationValidationSeverity::MustFix)?,
    })?;
    let second = ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Validation {
        issue: validation_issue(ExplanationValidationSeverity::LikelyProblem)?,
    })?;
    let request = EvidenceRenderRequestV2::new(first)?;
    let rendered = EvidenceRenderResultV1::new(
        &request,
        vec![EvidenceMessageV1 {
            message_key: "evidence.summary".to_owned(),
            parameters: BTreeMap::new(),
            entities: Vec::new(),
            rules: Vec::new(),
            assignments: Vec::new(),
            evidence: Vec::new(),
        }],
    )?;
    assert!(matches!(
        ExplanationResultV2::new(second, rendered),
        Err(DomainContractError::InvalidExplanationContract(
            "explanation result evidence binding"
        ))
    ));
    Ok(())
}

#[test]
fn optimality_status_rejects_detached_run_authority() -> Result<(), Box<dyn Error>> {
    let fixture = counterfactual_fixture()?;
    let status = OptimalityStatusEvidence {
        schema_version: Some(2),
        run_input: fixture.result.base_run_input,
        run_manifest: fixture.result.run_manifest,
        result: None,
    };
    assert!(matches!(
        status.validate(),
        Err(DomainContractError::InvalidExplanationContract(
            "optimality run binding"
        ))
    ));
    Ok(())
}

#[test]
fn maximum_captured_original_survives_complete_status_and_counterfactual_carriers()
-> Result<(), Box<dyn Error>> {
    let fixture = counterfactual_fixture_with_capture(true)?;
    let status = ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::OptimalityStatus {
        status: Box::new(OptimalityStatusEvidence {
            schema_version: Some(2),
            run_input: fixture.result.base_run_input.clone(),
            run_manifest: fixture.result.base_run_manifest.clone(),
            result: Some(fixture.request.semantics.base.clone()),
        }),
    })?;
    assert_eq!(
        ExplanationEvidenceV2::from_json(&serde_json::to_vec(&status)?)?,
        status
    );
    let counterfactual =
        ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Counterfactual {
            result: Box::new(fixture.result),
        })?;
    assert_eq!(
        ExplanationEvidenceV2::from_json(&serde_json::to_vec(&counterfactual)?)?,
        counterfactual,
    );
    Ok(())
}

#[test]
fn bounded_captures_do_not_exempt_the_complete_counterfactual_carrier_limit()
-> Result<(), Box<dyn Error>> {
    let fixture = counterfactual_fixture_with_capture(true)?;
    let result = fixture.result;
    let enlarge = |manifest: RunManifest| -> Result<RunManifest, Box<dyn Error>> {
        let warnings = (0..2000)
            .map(|index| {
                Ok(VerificationWarning {
                    id: VerificationWarningId::new(format!("warning.large-{index:04}"))?,
                    message_key: "synthetic.warning.large".to_owned(),
                    affected_entities: Vec::new(),
                    facts: BTreeMap::from([(
                        VerificationFactId::new("fact.large")?,
                        VerificationValue::Text("x".repeat(MAX_VERIFICATION_TEXT_BYTES)),
                    )]),
                })
            })
            .collect::<Result<Vec<_>, DomainIdError>>()?;
        Ok(RunManifest::new(
            manifest.run_id,
            manifest.run_input_checksum,
            manifest.outcome,
            manifest.started_at,
            manifest.finished_at,
            manifest.elapsed_milliseconds,
            manifest.first_incumbent_milliseconds,
            manifest.first_verified_feasible_milliseconds,
            manifest.phase_timings,
            warnings,
        )?)
    };
    // Each manifest remains legal independently; only the combined carrier is too large.
    let base_manifest = enlarge(result.base_run_manifest)?;
    let derived_manifest = enlarge(result.run_manifest)?;
    assert!(matches!(
        CounterfactualResult::new(
            result.request,
            result.base_run_input,
            base_manifest,
            result.compilation,
            result.run_input,
            derived_manifest,
            result.conclusion,
        ),
        Err(DomainContractError::LimitExceeded(
            "domain contract JSON bytes"
        )),
    ));
    Ok(())
}

#[test]
fn resealed_recorded_optimal_status_cannot_acquire_current_backend_proof()
-> Result<(), Box<dyn Error>> {
    let mut recorded =
        PortableAcceptedResult::from_json(include_bytes!("fixtures/portable_accepted_v2.json"))?;
    // Resealing a recorded status claim does not authenticate a backend proof.
    let mut manifest = serde_json::to_value(&recorded.run_manifest)?;
    manifest["outcome"]["status"] = json!("optimal");
    manifest
        .as_object_mut()
        .ok_or("manifest is not an object")?
        .remove("checksum");
    let checksum = blake3_hex(&serde_json::to_vec(&manifest)?);
    manifest["checksum"] = json!(checksum);
    recorded.run_manifest = RunManifest::from_json(&serde_json::to_vec(&manifest)?)?;
    let result = AcceptedResultRefV1::from_result(&recorded.accepted_result)?;
    let evidence = ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::OptimalityStatus {
        status: Box::new(OptimalityStatusEvidence {
            schema_version: Some(2),
            run_input: recorded.run_input,
            run_manifest: recorded.run_manifest,
            result: Some(result),
        }),
    })?;
    assert_eq!(
        evidence.certainty,
        ExplanationCertainty::IndependentlyVerified
    );
    ExplanationEvidenceV2::from_json(&serde_json::to_vec(&evidence)?)?;

    let mut claimed_proof = serde_json::to_value(evidence)?;
    claimed_proof["certainty"] = json!("backendProof");
    claimed_proof
        .as_object_mut()
        .ok_or("evidence is not an object")?
        .remove("checksum");
    let checksum = blake3_hex(&serde_json::to_vec(&claimed_proof)?);
    claimed_proof["checksum"] = json!(checksum);
    assert!(matches!(
        ExplanationEvidenceV2::from_json(&serde_json::to_vec(&claimed_proof)?),
        Err(DomainContractError::InvalidExplanationContract(
            "evidence certainty"
        ))
    ));
    Ok(())
}

#[test]
fn explanation_ingress_rejects_unknowns_and_certainty_kind_checksum_tampering()
-> Result<(), Box<dyn Error>> {
    let evidence = all_evidence()?.remove(0);
    let mut value = serde_json::to_value(&evidence)?;
    value["unknown"] = json!(true);
    assert!(ExplanationEvidenceV2::from_json(&serde_json::to_vec(&value)?).is_err());

    let mut value = serde_json::to_value(&evidence)?;
    value["schemaVersion"] = json!(1);
    assert!(matches!(
        ExplanationEvidenceV2::from_json(&serde_json::to_vec(&value)?),
        Err(DomainContractError::UnsupportedVersion(1))
    ));

    let mut value = serde_json::to_value(&evidence)?;
    value["certainty"] = json!("backendProof");
    assert!(matches!(
        ExplanationEvidenceV2::from_json(&serde_json::to_vec(&value)?),
        Err(DomainContractError::InvalidExplanationContract(
            "evidence certainty"
        ))
    ));

    let request = EvidenceRenderRequestV2::new(evidence.clone())?;
    let mut value = serde_json::to_value(&request)?;
    value["schemaVersion"] = json!(1);
    assert!(matches!(
        EvidenceRenderRequestV2::from_json(&serde_json::to_vec(&value)?),
        Err(DomainContractError::UnsupportedVersion(1))
    ));
    let mut value = serde_json::to_value(&request)?;
    value["kind"] = json!("assignment");
    assert!(matches!(
        EvidenceRenderRequestV2::from_json(&serde_json::to_vec(&value)?),
        Err(DomainContractError::InvalidExplanationContract(
            "render request kind"
        ))
    ));

    let mut value = serde_json::to_value(&evidence)?;
    value["checksum"] = json!(hash("tampered"));
    assert!(matches!(
        ExplanationEvidenceV2::from_json(&serde_json::to_vec(&value)?),
        Err(DomainContractError::ChecksumMismatch)
    ));
    Ok(())
}

#[test]
fn explanation_v2_rejects_nested_comparison_checksum_tampering() -> Result<(), Box<dyn Error>> {
    let comparison = all_evidence()?
        .into_iter()
        .find(|evidence| evidence.kind() == ExplanationKind::SolutionDifference)
        .ok_or("missing solution difference evidence")?;
    let mut value = serde_json::to_value(comparison)?;
    value["evidence"]["comparison"]["checksum"] = json!(hash("tampered-comparison"));
    assert_eq!(
        ExplanationEvidenceV2::from_json(&serde_json::to_vec(&value)?),
        Err(DomainContractError::ChecksumMismatch)
    );
    Ok(())
}

#[test]
fn evidence_references_are_canonical_bounded_and_text_is_safe() -> Result<(), Box<dyn Error>> {
    let mut message = EvidenceMessageV1 {
        message_key: "evidence.message".to_owned(),
        parameters: BTreeMap::new(),
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: vec![
            DomainAssignmentId::new("assignment.second")?,
            DomainAssignmentId::new("assignment.first")?,
        ],
        evidence: Vec::new(),
    };
    assert!(matches!(
        message.validate(),
        Err(DomainContractError::NonCanonicalExplanationCollection(
            "message assignments"
        ))
    ));
    message.assignments = (0..=MAX_EVIDENCE_MESSAGE_REFERENCES)
        .map(|index| DomainAssignmentId::new(format!("assignment.a{index:04}")))
        .collect::<Result<_, _>>()?;
    assert!(matches!(
        message.validate(),
        Err(DomainContractError::LimitExceeded("message assignments"))
    ));
    message.assignments.clear();
    message.parameters.insert(
        VerificationFactId::new("fact.unsafe")?,
        VerificationValue::Text(String::new()),
    );
    assert!(matches!(
        message.validate(),
        Err(DomainContractError::InvalidVerificationText)
    ));

    let mut assignment = assignment_evidence()?;
    let mut duplicate = assignment.score_contributions[0].clone();
    duplicate.value = 5;
    assignment.score_contributions.push(duplicate);
    assert!(matches!(
        assignment.validate(),
        Err(DomainContractError::NonCanonicalExplanationCollection(
            "score contributions"
        ))
    ));
    Ok(())
}

#[test]
fn all_four_validation_severities_are_distinct_and_strict() -> Result<(), Box<dyn Error>> {
    for severity in [
        ExplanationValidationSeverity::MustFix,
        ExplanationValidationSeverity::LikelyProblem,
        ExplanationValidationSeverity::ReviewSuggested,
        ExplanationValidationSeverity::Information,
    ] {
        let issue = validation_issue(severity)?;
        issue.validate()?;
        assert_eq!(
            serde_json::from_slice::<ValidationIssueEvidenceV1>(&serde_json::to_vec(&issue)?)?,
            issue
        );
    }
    assert!(serde_json::from_value::<ExplanationValidationSeverity>(json!("warning")).is_err());
    Ok(())
}

#[test]
fn force_and_forbid_conditions_round_trip_with_checksums() -> Result<(), Box<dyn Error>> {
    for condition in [
        CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: DomainAssignmentId::new("assignment.target")?,
            value: AssignmentValue::Integer(4),
        },
        CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id: DomainAssignmentId::new("assignment.target")?,
            value: AssignmentValue::Integer(4),
        },
    ] {
        let condition = CounterfactualConditionV1::new(condition)?;
        let wire = serde_json::to_value(&condition)?;
        assert_eq!(
            wire.pointer("/condition/assignmentId"),
            Some(&json!("assignment.target"))
        );
        assert!(wire.pointer("/condition/assignment_id").is_none());
        assert_eq!(
            CounterfactualConditionV1::from_json(&serde_json::to_vec(&wire)?)?,
            condition
        );
        let mut tampered = serde_json::to_value(&condition)?;
        tampered["checksum"] = json!(hash("wrong"));
        assert!(matches!(
            CounterfactualConditionV1::from_json(&serde_json::to_vec(&tampered)?),
            Err(DomainContractError::ChecksumMismatch)
        ));
    }
    Ok(())
}

#[test]
fn replacement_condition_round_trips_and_binds_distinct_assignments() -> Result<(), Box<dyn Error>>
{
    let replacement =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: DomainAssignmentId::new("assignment.target")?,
            replaced_assignment_id: DomainAssignmentId::new("assignment.incumbent")?,
        })?;
    let wire = serde_json::to_value(&replacement)?;
    assert_eq!(
        wire.pointer("/condition/type"),
        Some(&json!("replaceAssignment"))
    );
    assert_eq!(
        wire.pointer("/condition/targetAssignmentId"),
        Some(&json!("assignment.target"))
    );
    assert_eq!(
        wire.pointer("/condition/replacedAssignmentId"),
        Some(&json!("assignment.incumbent"))
    );
    assert!(wire.pointer("/condition/assignmentId").is_none());
    assert_eq!(
        CounterfactualConditionV1::from_json(&serde_json::to_vec(&wire)?)?,
        replacement
    );

    let mut changed_id = serde_json::to_value(&replacement)?;
    changed_id["condition"]["targetAssignmentId"] = json!("assignment.alternative");
    assert!(matches!(
        CounterfactualConditionV1::from_json(&serde_json::to_vec(&changed_id)?),
        Err(DomainContractError::ChecksumMismatch)
    ));
    let mut tampered = serde_json::to_value(&replacement)?;
    tampered["checksum"] = json!(hash("wrong"));
    assert!(matches!(
        CounterfactualConditionV1::from_json(&serde_json::to_vec(&tampered)?),
        Err(DomainContractError::ChecksumMismatch)
    ));
    assert!(matches!(
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: DomainAssignmentId::new("assignment.same")?,
            replaced_assignment_id: DomainAssignmentId::new("assignment.same")?,
        },),
        Err(DomainContractError::InvalidExplanationContract(
            "counterfactual replacement assignments must be distinct"
        ))
    ));
    assert!(
        serde_json::from_value::<CounterfactualConditionPayloadV1>(json!({
            "type": "replaceAssignment",
            "targetAssignmentId": "",
            "replacedAssignmentId": "assignment.incumbent"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<CounterfactualConditionPayloadV1>(json!({
            "type": "replaceAssignment",
            "targetAssignmentId": "Assignment.Target",
            "replacedAssignmentId": "assignment.incumbent"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<CounterfactualConditionPayloadV1>(json!({
            "type": "relaxRequiredRule",
            "ruleId": uuid(1)
        }))
        .is_err()
    );
    let unchanged_hash = hash("unchanged-model");
    assert!(matches!(
        CounterfactualCompilationBindingV1::new(
            unchanged_hash.clone(),
            hash("condition"),
            unchanged_hash,
            hash("objective"),
        ),
        Err(DomainContractError::InvalidExplanationContract(
            "unchanged counterfactual model"
        ))
    ));
    Ok(())
}

#[test]
fn counterfactual_request_hash_excludes_job_request_ids_and_timestamps()
-> Result<(), Box<dyn Error>> {
    let fixture = counterfactual_fixture()?;
    let second = CounterfactualJobRequestV1::new(
        uuid(0x30).parse()?,
        uuid(0x31).parse()?,
        fixture.request.semantics.clone(),
        fixture.request.condition.clone(),
        "2027-01-01T00:00:00Z".parse()?,
    )?;
    assert_eq!(fixture.request.request_hash, second.request_hash);
    assert_ne!(fixture.request.job_id, second.job_id);
    assert_ne!(fixture.request.request_id, second.request_id);
    assert_ne!(fixture.request.created_at, second.created_at);

    let mismatched =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id: DomainAssignmentId::new("assignment.target")?,
            value: AssignmentValue::Boolean(true),
        })?;
    assert!(matches!(
        CounterfactualJobRequestV1::new(
            uuid(0x32).parse()?,
            uuid(0x33).parse()?,
            fixture.request.semantics.clone(),
            mismatched,
            "2027-01-01T00:00:00Z".parse()?,
        ),
        Err(DomainContractError::InvalidExplanationContract(
            "counterfactual request condition binding"
        ))
    ));
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn counterfactual_authority_substitution_is_rejected() -> Result<(), Box<dyn Error>> {
    let fixture = counterfactual_fixture()?;

    let replacement_request = CounterfactualJobRequestV1::new(
        uuid(0x34).parse()?,
        uuid(0x35).parse()?,
        fixture.request.semantics.clone(),
        fixture.request.condition.clone(),
        "2026-09-03T12:00:00Z".parse()?,
    )?;
    assert!(matches!(
        CounterfactualJobRecordV1::new(
            replacement_request,
            CounterfactualJobState::Completed,
            Some("2026-09-03T12:00:01Z".parse()?),
            Some("2026-09-03T12:00:03Z".parse()?),
            None,
            None,
            Some(fixture.result.clone()),
            None,
            None,
        ),
        Err(DomainContractError::InvalidExplanationContract(
            "counterfactual job result binding"
        ))
    ));

    let mut detached_base = fixture.result.clone();
    detached_base.base_run_input = detached_base.run_input.clone();
    assert!(matches!(
        detached_base.validate(),
        Err(DomainContractError::InvalidExplanationContract(
            "counterfactual base run binding"
        ))
    ));

    let reused_run_input = rebuild_run_input(
        &fixture.result.run_input,
        fixture.result.base_run_input.run_id,
        fixture.result.run_input.compiler_version.clone(),
    )?;
    let reused_run_manifest = infeasible_manifest(&reused_run_input)?;
    assert!(matches!(
        CounterfactualResult::new(
            fixture.request.clone(),
            fixture.result.base_run_input.clone(),
            fixture.result.base_run_manifest.clone(),
            fixture.result.compilation.clone(),
            reused_run_input,
            reused_run_manifest,
            CounterfactualConclusionV1::ProvenImpossible,
        ),
        Err(DomainContractError::InvalidExplanationContract(
            "counterfactual derived run binding"
        ))
    ));

    let changed_compiler_input = rebuild_run_input(
        &fixture.result.run_input,
        fixture.result.run_input.run_id,
        "2.0.0".to_owned(),
    )?;
    let changed_compiler_manifest = infeasible_manifest(&changed_compiler_input)?;
    assert!(matches!(
        CounterfactualResult::new(
            fixture.request.clone(),
            fixture.result.base_run_input.clone(),
            fixture.result.base_run_manifest.clone(),
            fixture.result.compilation.clone(),
            changed_compiler_input,
            changed_compiler_manifest,
            CounterfactualConclusionV1::ProvenImpossible,
        ),
        Err(DomainContractError::InvalidExplanationContract(
            "counterfactual derived run binding"
        ))
    ));

    let alternative = result_ref(2)?;
    let accepted_manifest = RunManifest::new(
        fixture.result.run_input.run_id,
        fixture.result.run_input.checksum.clone(),
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: alternative.solution_id,
            accepted_result_checksum: alternative.result_checksum.clone(),
            verification_checksum: hash("alternative-verification"),
        },
        "2026-09-03T12:00:01Z".parse()?,
        "2026-09-03T12:00:02Z".parse()?,
        Some(DurationMillis::new(1_000)?),
        Some(DurationMillis::new(500)?),
        Some(DurationMillis::new(800)?),
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    let comparison = counterfactual_comparison(
        &fixture.result,
        fixture.request.semantics.base.clone(),
        alternative.clone(),
    )?;
    let verified = CounterfactualResult::new(
        fixture.request.clone(),
        fixture.result.base_run_input.clone(),
        fixture.result.base_run_manifest.clone(),
        fixture.result.compilation.clone(),
        fixture.result.run_input.clone(),
        accepted_manifest.clone(),
        CounterfactualConclusionV1::VerifiedAlternative {
            alternative: alternative.clone(),
            comparison: Box::new(comparison),
            ordering: ComparisonOrdering::Equivalent,
        },
    )?;
    verified.validate()?;

    let substituted_result = result_ref(3)?;
    let substituted_result_comparison = counterfactual_comparison(
        &fixture.result,
        fixture.request.semantics.base.clone(),
        substituted_result.clone(),
    )?;
    assert!(matches!(
        CounterfactualResult::new(
            fixture.request.clone(),
            fixture.result.base_run_input.clone(),
            fixture.result.base_run_manifest.clone(),
            fixture.result.compilation.clone(),
            fixture.result.run_input.clone(),
            accepted_manifest.clone(),
            CounterfactualConclusionV1::VerifiedAlternative {
                alternative: substituted_result,
                comparison: Box::new(substituted_result_comparison),
                ordering: ComparisonOrdering::Equivalent,
            },
        ),
        Err(DomainContractError::InvalidExplanationContract(
            "counterfactual alternative binding"
        ))
    ));

    let substituted_comparison =
        counterfactual_comparison(&fixture.result, result_ref(4)?, alternative.clone())?;
    assert!(matches!(
        CounterfactualResult::new(
            fixture.request.clone(),
            fixture.result.base_run_input.clone(),
            fixture.result.base_run_manifest.clone(),
            fixture.result.compilation.clone(),
            fixture.result.run_input.clone(),
            accepted_manifest,
            CounterfactualConclusionV1::VerifiedAlternative {
                alternative,
                comparison: Box::new(substituted_comparison),
                ordering: ComparisonOrdering::Equivalent,
            },
        ),
        Err(DomainContractError::InvalidExplanationContract(
            "counterfactual alternative binding"
        ))
    ));
    Ok(())
}

#[test]
fn every_valid_counterfactual_job_state_timestamp_matrix_is_accepted() -> Result<(), Box<dyn Error>>
{
    let fixture = counterfactual_fixture()?;
    let started = "2026-09-03T12:00:01Z".parse()?;
    let finished = "2026-09-03T12:00:03Z".parse()?;
    let valid = vec![
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Queued,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Running,
            Some(started),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Running,
            Some(started),
            None,
            Some(uuid(0x32).parse()?),
            Some(started),
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Completed,
            Some(started),
            Some(finished),
            None,
            None,
            Some(fixture.result.clone()),
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Failed,
            None,
            Some(finished),
            None,
            None,
            None,
            Some(CounterfactualJobErrorV1 {
                kind: CounterfactualFailureKind::CompilationFailed,
            }),
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Cancelled,
            None,
            Some(finished),
            Some(uuid(0x33).parse()?),
            Some(started),
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request,
            CounterfactualJobState::Interrupted,
            Some(started),
            Some(finished),
            None,
            None,
            None,
            None,
            None,
        ),
    ];
    for record in valid {
        let record = record?;
        assert_eq!(
            CounterfactualJobRecordV1::from_json(&serde_json::to_vec(&record)?)?,
            record
        );
    }
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn every_invalid_counterfactual_job_state_or_timestamp_shape_is_rejected()
-> Result<(), Box<dyn Error>> {
    let fixture = counterfactual_fixture()?;
    let started = "2026-09-03T12:00:01Z".parse()?;
    let finished = "2026-09-03T12:00:03Z".parse()?;
    let error = CounterfactualJobErrorV1 {
        kind: CounterfactualFailureKind::BackendFailed,
    };
    let mut over_budget_semantics = fixture.request.semantics.clone();
    over_budget_semantics.total_budget_milliseconds = DurationMillis::new(4_999)?;
    let over_budget_request = CounterfactualJobRequestV1::new(
        fixture.request.job_id,
        fixture.request.request_id,
        over_budget_semantics,
        fixture.request.condition.clone(),
        fixture.request.created_at,
    )?;
    let invalid = vec![
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Queued,
            Some(started),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Queued,
            None,
            None,
            Some(uuid(0x41).parse()?),
            Some(started),
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            over_budget_request,
            CounterfactualJobState::Completed,
            Some(started),
            Some(finished),
            None,
            None,
            Some(fixture.result.clone()),
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Running,
            Some(started),
            Some(finished),
            None,
            None,
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Completed,
            Some(started),
            Some(finished),
            None,
            None,
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Completed,
            Some(started),
            Some(finished),
            Some(uuid(0x42).parse()?),
            Some(started),
            Some(fixture.result.clone()),
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Failed,
            None,
            Some(finished),
            None,
            None,
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Failed,
            None,
            Some(finished),
            Some(uuid(0x43).parse()?),
            Some(started),
            None,
            Some(error),
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Cancelled,
            None,
            Some(finished),
            None,
            None,
            None,
            Some(error),
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Interrupted,
            None,
            Some(finished),
            None,
            None,
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Interrupted,
            Some(started),
            Some(finished),
            Some(uuid(0x44).parse()?),
            Some(started),
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Cancelled,
            None,
            Some(finished),
            Some(uuid(0x40).parse()?),
            None,
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request.clone(),
            CounterfactualJobState::Cancelled,
            None,
            Some(finished),
            None,
            None,
            None,
            None,
            None,
        ),
        CounterfactualJobRecordV1::new(
            fixture.request,
            CounterfactualJobState::Running,
            Some("2026-09-02T12:00:00Z".parse()?),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
    ];
    assert!(invalid.into_iter().all(|record| record.is_err()));
    Ok(())
}

#[test]
fn conflict_minimality_and_group_identity_are_derived_and_validated() -> Result<(), Box<dyn Error>>
{
    let group = ConflictGroupV1 {
        group_id: AssumptionGroupId::new("assumption.required")?,
        required_rules: vec![uuid(1).parse()?],
    };
    let evidence = ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Infeasibility {
        infeasibility: InfeasibilityEvidenceV1::Conflict {
            groups: vec![group],
            minimality: ConflictMinimality::ProvenMinimal,
            shrink: ConflictShrinkSummaryV1 {
                initial_group_count: 1,
                remaining_group_count: 1,
                attempted_trials: 1,
                max_trials: 4,
                stop_reason: ConflictShrinkStopReason::Completed,
            },
        },
        condition: None,
    })?;
    assert_eq!(
        evidence.certainty,
        ExplanationCertainty::ProvenMinimalConflict
    );

    let invalid_core = ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Infeasibility {
        infeasibility: InfeasibilityEvidenceV1::Unavailable {
            reason: ConflictUnavailableReason::InvalidAssumptionCore,
        },
        condition: None,
    })?;
    assert_eq!(invalid_core.certainty, ExplanationCertainty::Unavailable);
    assert_eq!(
        ExplanationEvidenceV2::from_json(&serde_json::to_vec(&invalid_core)?)?,
        invalid_core
    );
    assert_eq!(
        serde_json::to_value(ConflictUnavailableReason::InvalidAssumptionCore)?,
        json!("invalidAssumptionCore")
    );

    let invalid = InfeasibilityEvidenceV1::Conflict {
        groups: Vec::new(),
        minimality: ConflictMinimality::Sufficient,
        shrink: ConflictShrinkSummaryV1 {
            initial_group_count: 1,
            remaining_group_count: 1,
            attempted_trials: 0,
            max_trials: 0,
            stop_reason: ConflictShrinkStopReason::NotAttempted,
        },
    };
    assert!(
        ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Infeasibility {
            infeasibility: invalid,
            condition: None,
        })
        .is_err()
    );
    Ok(())
}

#[test]
fn cross_revision_comparison_retains_both_bindings_and_rejects_tampering()
-> Result<(), Box<dyn Error>> {
    let comparison = comparison()?;
    assert_eq!(comparison.base.scenario_revision, 7);
    assert_eq!(comparison.candidate.scenario_revision, 8);
    assert_ne!(
        comparison.base.document_hash,
        comparison.candidate.document_hash
    );
    comparison.validate()?;

    let mut tampered: Value = serde_json::to_value(&comparison)?;
    tampered["ordering"] = json!("better");

    let mut same_revision_wrong_scope = comparison.clone();
    same_revision_wrong_scope.candidate.scenario_revision =
        same_revision_wrong_scope.base.scenario_revision;
    assert!(matches!(
        same_revision_wrong_scope.validate(),
        Err(DomainContractError::InvalidExplanationContract(
            "same-revision verification scope"
        ))
    ));
    assert!(matches!(
        SolutionComparisonV1::from_json(&serde_json::to_vec(&tampered)?),
        Err(DomainContractError::InvalidExplanationContract(
            "comparison ordering"
        ))
    ));
    Ok(())
}

fn retained_diagnostic_fixture()
-> Result<(RunInput, RunManifest, RunInfeasibilityEvidenceV1), Box<dyn Error>> {
    let fixture = counterfactual_fixture()?;
    let input = fixture.result.run_input;
    let manifest = infeasible_manifest(&input)?;
    let group = ConflictGroupV1 {
        group_id: "test.diagnostic.group".parse()?,
        required_rules: vec![uuid(0x42).parse()?],
    };
    let original = OriginalInfeasibilityObservationV1 {
        run_input_checksum: input.checksum.clone(),
        model_hash: input.model_hash.clone(),
        worker_manifest_sha256: Some(hash("approved-worker")),
        observed_at: "2026-09-03T12:00:01.100Z".parse()?,
        elapsed_milliseconds: DurationMillis::new(100)?,
    };
    let diagnostic = DiagnosticInfeasibilityProofV1 {
        diagnostic_started_after_milliseconds: DurationMillis::new(200)?,
        initial: DiagnosticInfeasibilityObservationV1 {
            binding: DiagnosticSolveBindingV1 {
                model_hash: hash("diagnostic-model"),
                routed_fingerprint: hash("diagnostic-route"),
                submitted_assumptions: vec![DiagnosticSubmittedAssumptionV1 {
                    variable_id: "test.guard".parse()?,
                    value: true,
                    group_id: group.group_id.clone(),
                    required_rule_id: group.required_rules[0],
                }],
                native_profile: DiagnosticNativeProfileV1::NonemptyAssumptionSatisfaction,
                parameter_digest: hash("actual-parameters"),
                worker_manifest_sha256: hash("approved-worker"),
                time_limit_milliseconds: DurationMillis::new(500)?,
            },
            dispatched_after_milliseconds: DurationMillis::new(200)?,
            observed_after_milliseconds: DurationMillis::new(300)?,
        },
        initial_core: vec![group.clone()],
        last_proven_infeasible: None,
        attempted_trials: 0,
        max_trials: 0,
    };
    let infeasibility = InfeasibilityEvidenceV1::Conflict {
        groups: vec![group],
        minimality: ConflictMinimality::Sufficient,
        shrink: ConflictShrinkSummaryV1 {
            initial_group_count: 1,
            remaining_group_count: 1,
            attempted_trials: 0,
            max_trials: 0,
            stop_reason: ConflictShrinkStopReason::NotAttempted,
        },
    };
    let evidence = RunInfeasibilityEvidenceV1::new(
        &input,
        &manifest,
        original,
        Some(fixture.request.condition),
        Some(diagnostic),
        infeasibility,
    )?;
    Ok((input, manifest, evidence))
}

#[test]
fn retained_diagnostic_roundtrip_rejects_condition_and_final_manifest_substitution()
-> Result<(), Box<dyn Error>> {
    let (input, manifest, evidence) = retained_diagnostic_fixture()?;
    let decoded = RunInfeasibilityEvidenceV1::from_json(&serde_json::to_vec(&evidence)?)?;
    assert_eq!(decoded, evidence);
    decoded.validate_binding(&input, &manifest)?;
    let mut later = manifest.clone();
    later.finished_at = "2026-09-03T12:00:03Z".parse()?;
    later.elapsed_milliseconds = Some(DurationMillis::new(2_000)?);
    let later = RunManifest::new(
        later.run_id,
        later.run_input_checksum,
        later.outcome,
        later.started_at,
        later.finished_at,
        later.elapsed_milliseconds,
        None,
        None,
        later.phase_timings,
        later.verification_warnings,
    )?;
    assert!(decoded.validate_binding(&input, &later).is_err());
    let wrong =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: "assignment.other".parse()?,
            value: AssignmentValue::Boolean(true),
        })?;
    assert!(
        RunInfeasibilityEvidenceV1::new(
            &input,
            &manifest,
            evidence.original.clone(),
            Some(wrong),
            evidence.diagnostic.clone(),
            evidence.infeasibility.clone()
        )
        .is_err()
    );
    let mut unknown = serde_json::to_value(evidence)?;
    unknown["schemaVersion"] = json!(2);
    assert!(matches!(
        RunInfeasibilityEvidenceV1::from_json(&serde_json::to_vec(&unknown)?),
        Err(DomainContractError::UnsupportedVersion(2))
    ));
    Ok(())
}

#[test]
fn empty_core_is_not_foundational_without_explicit_zero_assumption_request()
-> Result<(), Box<dyn Error>> {
    let (input, manifest, evidence) = retained_diagnostic_fixture()?;
    let mut diagnostic = evidence
        .diagnostic
        .clone()
        .ok_or("missing diagnostic fixture")?;
    diagnostic.initial_core.clear();
    let foundational = InfeasibilityEvidenceV1::Unavailable {
        reason: ConflictUnavailableReason::FoundationalInfeasibility,
    };
    assert!(
        RunInfeasibilityEvidenceV1::new(
            &input,
            &manifest,
            evidence.original.clone(),
            evidence.condition.clone(),
            Some(diagnostic.clone()),
            foundational.clone()
        )
        .is_err()
    );
    diagnostic.initial.binding.submitted_assumptions.clear();
    assert!(
        RunInfeasibilityEvidenceV1::new(
            &input,
            &manifest,
            evidence.original.clone(),
            evidence.condition.clone(),
            Some(diagnostic.clone()),
            foundational.clone()
        )
        .is_err()
    );
    diagnostic.initial.binding.native_profile =
        DiagnosticNativeProfileV1::ZeroAssumptionSatisfaction;
    let foundation = RunInfeasibilityEvidenceV1::new(
        &input,
        &manifest,
        evidence.original,
        evidence.condition,
        Some(diagnostic),
        foundational,
    )?;
    foundation.validate_binding(&input, &manifest)?;
    assert!(
        RunInfeasibilityEvidenceV1::new(
            &input,
            &manifest,
            foundation.original.clone(),
            foundation.condition.clone(),
            None,
            foundation.infeasibility
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn retained_proof_rejects_boundary_late_and_restarted_dispatch_budget() -> Result<(), Box<dyn Error>>
{
    let (input, manifest, evidence) = retained_diagnostic_fixture()?;
    let diagnostic = evidence
        .diagnostic
        .clone()
        .ok_or("missing diagnostic fixture")?;
    let mut late = diagnostic.clone();
    late.initial.observed_after_milliseconds = DurationMillis::new(700)?;
    assert!(
        RunInfeasibilityEvidenceV1::new(
            &input,
            &manifest,
            evidence.original.clone(),
            evidence.condition.clone(),
            Some(late),
            evidence.infeasibility.clone()
        )
        .is_err()
    );
    let mut restarted = diagnostic;
    restarted.diagnostic_started_after_milliseconds = DurationMillis::new(4_500)?;
    restarted.initial.dispatched_after_milliseconds = DurationMillis::new(4_700)?;
    restarted.initial.observed_after_milliseconds = DurationMillis::new(4_800)?;
    // Observation is timely, but request duration would cross the inherited parent deadline.
    let final_manifest = RunManifest::new(
        input.run_id,
        input.checksum.clone(),
        manifest.outcome,
        manifest.started_at,
        "2026-09-03T12:00:06Z".parse()?,
        Some(DurationMillis::new(5_000)?),
        None,
        None,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    assert!(
        RunInfeasibilityEvidenceV1::new(
            &input,
            &final_manifest,
            evidence.original,
            evidence.condition,
            Some(restarted),
            evidence.infeasibility
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn native_source_asset_custody_keeps_restrictions_and_rejects_wrapper_or_byte_drift()
-> Result<(), Box<dyn Error>> {
    let (input, _, _) = retained_diagnostic_fixture()?;
    let wrapper_hash = hash("full-source-wrapper");
    let custody = RunSourceAssetBindingsV1::new(
        &input,
        wrapper_hash.clone(),
        RunSourceAssetCaptureV1::Available {
            assets: vec![RunSourceAssetRefV1 {
                reference: "licensed/reference.bin".to_owned(),
                sha256: hash("opaque-bytes"),
                media_type: "application/octet-stream".to_owned(),
                redistribution_permitted: false,
                byte_length: 3,
            }],
        },
    )?;
    let decoded = RunSourceAssetBindingsV1::from_json(&serde_json::to_vec(&custody)?)?;
    assert_eq!(decoded, custody);
    decoded.validate_source(&input, &wrapper_hash)?;
    assert!(
        decoded
            .validate_source(&input, &hash("different-wrapper-extension"))
            .is_err()
    );
    let mut changed = decoded;
    let RunSourceAssetCaptureV1::Available { assets } = &mut changed.capture else {
        return Err("expected captured source assets".into());
    };
    assets[0].byte_length = 4;
    assert!(matches!(
        changed.validate(),
        Err(DomainContractError::ChecksumMismatch)
    ));
    Ok(())
}

#[test]
fn terminal_cancellation_requires_closed_matching_local_authority_reference()
-> Result<(), Box<dyn Error>> {
    let fixture = counterfactual_fixture()?;
    let authority = CounterfactualTerminalAuthorityV1::TimelyInfeasibility {
        run_id: fixture.result.run_input.run_id,
        run_manifest_checksum: fixture.result.run_manifest.checksum.clone(),
        evidence_checksum: hash("retained-evidence"),
    };
    let mut record = CounterfactualJobRecordV1::new(
        fixture.request,
        CounterfactualJobState::Completed,
        Some(fixture.result.run_manifest.started_at),
        Some(fixture.result.run_manifest.finished_at),
        Some(uuid(0x43).parse()?),
        Some(fixture.result.run_manifest.started_at),
        Some(fixture.result),
        None,
        Some(authority),
    )?;
    record.validate()?;
    record.terminal_authority = None;
    assert!(record.validate().is_err());
    record.terminal_authority = Some(CounterfactualTerminalAuthorityV1::VerificationAlarm {
        run_id: uuid(0x66).parse()?,
        run_manifest_checksum: hash("alarm-manifest"),
        clock_failure: None,
    });
    assert!(record.validate().is_err());
    Ok(())
}

#[test]
fn terminal_authority_wire_uses_closed_camel_case_fields_for_both_variants()
-> Result<(), Box<dyn Error>> {
    let run_id: SolveRunId = uuid(0x60).parse()?;
    for wire in [
        json!({"kind":"timelyInfeasibility","runId":run_id,"runManifestChecksum":hash("manifest"),"evidenceChecksum":hash("evidence")}),
        json!({"kind":"verificationAlarm","runId":run_id,"runManifestChecksum":hash("manifest")}),
    ] {
        let authority: CounterfactualTerminalAuthorityV1 = serde_json::from_value(wire.clone())?;
        authority.validate()?;
        assert_eq!(serde_json::to_value(authority)?, wire);
    }
    assert!(
        serde_json::from_value::<CounterfactualTerminalAuthorityV1>(json!({
            "kind":"verificationAlarm","run_id":run_id,"run_manifest_checksum":hash("manifest")
        }))
        .is_err()
    );
    Ok(())
}

#[test]
fn small_core_cannot_be_deleted_to_claim_a_foundational_proof() -> Result<(), Box<dyn Error>> {
    let (input, manifest, evidence) = retained_diagnostic_fixture()?;
    let mut proof = evidence
        .diagnostic
        .clone()
        .ok_or("missing diagnostic fixture")?;
    let mut last = proof.initial.clone();
    last.binding.model_hash = hash("zero-assumption-model");
    last.binding.routed_fingerprint = hash("zero-assumption-request");
    last.binding.native_profile = DiagnosticNativeProfileV1::ZeroAssumptionSatisfaction;
    last.binding.submitted_assumptions.clear();
    last.dispatched_after_milliseconds = DurationMillis::new(310)?;
    last.observed_after_milliseconds = DurationMillis::new(320)?;
    proof.last_proven_infeasible = Some(last);
    proof.attempted_trials = 1;
    proof.max_trials = 1;
    assert!(
        RunInfeasibilityEvidenceV1::new(
            &input,
            &manifest,
            evidence.original,
            evidence.condition,
            Some(proof),
            InfeasibilityEvidenceV1::Unavailable {
                reason: ConflictUnavailableReason::FoundationalInfeasibility,
            }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn retained_deletion_count_tracks_initial_core_not_all_submitted_assumptions()
-> Result<(), Box<dyn Error>> {
    let (input, manifest, evidence) = retained_diagnostic_fixture()?;
    let mut proof = evidence
        .diagnostic
        .clone()
        .ok_or("missing diagnostic fixture")?;
    proof.initial.binding.submitted_assumptions.clear();
    proof.initial_core.clear();
    for number in 0..18 {
        let assumption = DiagnosticSubmittedAssumptionV1 {
            variable_id: format!("test.guard.{number:03}").parse()?,
            value: true,
            group_id: format!("test.group.{number:03}").parse()?,
            required_rule_id: uuid(0x60 + number).parse()?,
        };
        if number < 17 {
            proof.initial_core.push(ConflictGroupV1 {
                group_id: assumption.group_id.clone(),
                required_rules: vec![assumption.required_rule_id],
            });
        }
        proof.initial.binding.submitted_assumptions.push(assumption);
    }
    let mut last = proof.initial.clone();
    last.binding.model_hash = hash("last-subset-model");
    last.binding.routed_fingerprint = hash("last-subset-request");
    last.binding.submitted_assumptions.truncate(15);
    last.dispatched_after_milliseconds = DurationMillis::new(310)?;
    last.observed_after_milliseconds = DurationMillis::new(320)?;
    proof.last_proven_infeasible = Some(last);
    proof.attempted_trials = 1;
    proof.max_trials = 32;
    let mut conflict = InfeasibilityEvidenceV1::Conflict {
        groups: proof.initial_core[..15].to_vec(),
        minimality: ConflictMinimality::Sufficient,
        shrink: ConflictShrinkSummaryV1 {
            initial_group_count: 17,
            remaining_group_count: 15,
            attempted_trials: 1,
            max_trials: 32,
            stop_reason: ConflictShrinkStopReason::Inconclusive,
        },
    };
    assert!(
        RunInfeasibilityEvidenceV1::new(
            &input,
            &manifest,
            evidence.original.clone(),
            evidence.condition.clone(),
            Some(proof.clone()),
            conflict.clone()
        )
        .is_err()
    );
    let mut last = proof
        .last_proven_infeasible
        .take()
        .ok_or("missing last request fixture")?;
    last.binding.submitted_assumptions = proof.initial.binding.submitted_assumptions[..16].to_vec();
    proof.last_proven_infeasible = Some(last);
    if let InfeasibilityEvidenceV1::Conflict { groups, shrink, .. } = &mut conflict {
        *groups = proof.initial_core[..16].to_vec();
        shrink.remaining_group_count = 16;
    }
    // Discarding one non-core submitted assumption is not a deletion trial.
    let valid = RunInfeasibilityEvidenceV1::new(
        &input,
        &manifest,
        evidence.original,
        evidence.condition,
        Some(proof),
        conflict,
    )?;
    valid.validate_binding(&input, &manifest)?;
    Ok(())
}
