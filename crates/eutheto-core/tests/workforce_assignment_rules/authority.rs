use super::{TestResult, context, fixture_value, id, pair, selections, workforce_fixture};
use eutheto_domain_api::{
    ContractJsonLimits, DomainPack, DomainPackError, ShareResultOptions, validate_contract_value,
};
use eutheto_domain_ir::{
    AcceptedResult, AssignmentEvidenceV1, AssignmentValue, DomainAssignment, DomainEntityRef,
    EvidenceRenderRequestV1, ExplanationEvidencePayloadV1, ExplanationEvidenceV1,
    ExplanationValidationSeverity, MetricValue, NormalizedSolution, ScoreContributionV1,
    ValidationIssueEvidenceV1, VerificationContextV1, VerificationReport, VerificationValue,
    blake3_hex,
};
use eutheto_planning_ir::{
    CandidateValues, Constraint, PlanningIrLimitsV1, PlanningProblem, Variable, feature_usage,
};
use eutheto_solver_api::{BackendCandidate, BackendObjectiveEvidence};
use eutheto_types::{CancellationToken, DurationMillis, ScenarioDocument};
use eutheto_verify::{
    AcceptanceDecision, AcceptanceReviewer, BackendObjectiveReconciliation,
    CorrectnessAlarmCategory, SystemVerificationClock,
};
use eutheto_workforce::{
    WorkforcePack,
    assignment_rules::{
        AssignmentRuleError, AssignmentRuleLimit, build_workforce_share_result,
        render_workforce_evidence, score_workforce_solution, verify_workforce_solution,
        workforce_verification_scope,
    },
    model::AssignmentPair,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn source_solution(
    document: &ScenarioDocument,
    selected: &[AssignmentPair],
) -> Result<NormalizedSolution> {
    let mut assignments = selected
        .iter()
        .map(|pair| {
            let identity = format!("{}.{}", pair.person_id, pair.shift_id);
            Ok(DomainAssignment {
                id: format!("official.workforce.assignment.{identity}").parse()?,
                entity: DomainEntityRef {
                    kind: "official.workforce.assignment".parse()?,
                    id: identity.parse()?,
                },
                value: AssignmentValue::Boolean(true),
                evidence: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    assignments.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(NormalizedSolution {
        schema_version: 1,
        pack_id: document.domain_pack.id.clone(),
        scenario_id: document.scenario_id,
        scenario_revision: 1,
        projection_version: 1,
        solution_id: id(700).parse()?,
        assignments,
    })
}

/// Source semantics only. The syntactic model hash here is not model/acceptance evidence;
/// the real-reviewer tests below establish that separate binding.
pub(super) fn verify_selection(
    document: &ScenarioDocument,
    selected: &[AssignmentPair],
) -> Result<VerificationReport> {
    let solution = source_solution(document, selected)?;
    let scope = workforce_verification_scope(document, 1, None)?;
    let context = VerificationContextV1::new(
        document.scenario_id,
        1,
        blake3_hex(&serde_json::to_vec(document)?),
        "0".repeat(64),
        solution.canonical_hash()?,
        scope.checksum,
    )?;
    let authoritative = score_workforce_solution(document, &solution, None)?;
    Ok(verify_workforce_solution(
        document,
        &solution,
        &context,
        &authoritative,
        None,
    )?)
}

fn small_document() -> Result<ScenarioDocument> {
    let mut document = workforce_fixture::fixture()?;
    document.domain.entities.remove(&id(6).parse()?);
    document.domain.locked_assignments.clear();
    document.domain.rules.insert(id(30).parse()?, json!({
        "id":id(30),"kind":"coverage","active":true,"strength":"required","scope":{"people":{"kind":"all"}}
    }));
    Ok(document)
}

fn tiny_candidate(problem: &PlanningProblem, selected: bool) -> Result<BackendCandidate> {
    let mut values = CandidateValues::default();
    for variable in &problem.variables {
        match variable {
            Variable::Boolean(variable) => {
                values.booleans.insert(variable.id.clone(), selected);
            }
            Variable::Integer(variable) => {
                values
                    .integers
                    .insert(variable.id.clone(), i64::from(selected));
            }
            Variable::Interval(_) => {
                return Err("the tiny source fixture has no interval variables".into());
            }
        }
    }
    Ok(BackendCandidate {
        sequence: 1,
        values,
        observed_after_milliseconds: DurationMillis::ZERO,
        objective: None,
        evidence_refs: Vec::new(),
    })
}

fn accepted_fixture(
    document: &ScenarioDocument,
    selected: bool,
) -> Result<(PlanningProblem, BackendCandidate, AcceptedResult)> {
    let problem = WorkforcePack.compile(document, &context())?;
    let candidate = tiny_candidate(&problem, selected)?;
    let clock = SystemVerificationClock::default();
    let reviewer = AcceptanceReviewer::new(&WorkforcePack, document, 1, &problem, &clock)
        .map_err(|alarm| alarm.diagnostic_code)?;
    match reviewer.review(&candidate, id(700).parse()?, &context().control) {
        AcceptanceDecision::Accepted { result, .. } => Ok((problem, candidate, *result)),
        other => Err(format!("expected real acceptance, got {other:?}").into()),
    }
}

fn report_context(
    report: &VerificationReport,
    solution: &NormalizedSolution,
) -> Result<VerificationContextV1> {
    Ok(VerificationContextV1::new(
        report.scenario_id,
        report.evaluated_revision,
        report.document_hash.clone(),
        report.planning_model_hash.clone(),
        solution.canonical_hash()?,
        report.verification_scope_checksum.clone(),
    )?)
}

fn assignment_request(evidence: AssignmentEvidenceV1) -> Result<EvidenceRenderRequestV1> {
    Ok(EvidenceRenderRequestV1::new(ExplanationEvidenceV1::new(
        ExplanationEvidencePayloadV1::Assignment {
            assignment: evidence,
        },
    )?)?)
}

fn assignment_evidence(accepted: &AcceptedResult) -> AssignmentEvidenceV1 {
    AssignmentEvidenceV1 {
        assignment: accepted.solution.assignments[0].clone(),
        related_rules: Vec::new(),
        score_contributions: Vec::new(),
        metrics: BTreeMap::new(),
        lock_state: None,
    }
}

#[test]
fn frozen_source_truth_and_rank_reject_coherent_verifier_omissions() -> TestResult {
    let document: ScenarioDocument = serde_json::from_value(fixture_value()?)?;
    let expected_scope = [1, 20, 21, 30, 31, 32, 33]
        .into_iter()
        .map(|value| id(value).parse())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let scope = workforce_verification_scope(&document, 1, None)?;
    assert_eq!(
        scope
            .required_rules
            .iter()
            .map(|binding| binding.rule_id)
            .collect::<Vec<_>>(),
        expected_scope
    );
    for (mask, selected) in selections()?.iter().enumerate() {
        let report = verify_selection(&document, selected)?;
        // Frozen source meaning, not evaluator output or compiled constraints:
        // one qualified person per shift, person20 unavailable for shift8, person21
        // unqualified, and one person cannot cover both overlapping shifts.
        assert_eq!(report.accepted, mask == 9, "source selection mask {mask}");
        assert_eq!(report.score.feasibility == 0, mask == 9);
        let rank: i64 = (0..6)
            .filter(|bit| mask & (1 << bit) != 0)
            .map(|bit| i64::from(bit + 1))
            .sum();
        assert_eq!(report.score.levels[0].value, rank);
        assert_eq!(
            report
                .required_rule_results
                .iter()
                .map(|rule| rule.rule_id)
                .collect::<Vec<_>>(),
            expected_scope
        );
    }
    Ok(())
}

#[test]
fn unconditional_activity_and_approved_leave_do_not_depend_on_authored_rules() -> TestResult {
    let mut value = fixture_value()?;
    value["domain"]["rules"] = json!({});
    value["domain"]["entities"][id(1)]["activeRange"] =
        json!({"kind":"dateRange","startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    let mut leave = value["domain"]["entities"][id(23)].clone();
    leave["id"] = json!(id(25));
    leave["availabilityKind"] = json!("approvedTimeOff");
    leave["timeWindow"] =
        json!({"kind":"instant","startsAt":"2026-11-01T10:00:00Z","endsAt":"2026-11-01T11:00:00Z"});
    value["domain"]["entities"][id(25)] = leave;
    let document = serde_json::from_value(value)?;
    let inactive = verify_selection(&document, &[pair(1, 8)?])?;
    assert_eq!((inactive.accepted, inactive.score.feasibility), (false, 1));
    let on_leave = verify_selection(&document, &[pair(20, 22)?])?;
    assert_eq!((on_leave.accepted, on_leave.score.feasibility), (false, 1));
    let both = verify_selection(&document, &[pair(1, 8)?, pair(20, 22)?])?;
    assert_eq!((both.accepted, both.score.feasibility), (false, 2));
    assert!(verify_selection(&document, &[pair(20, 8)?])?.accepted);
    assert!(verify_selection(&document, &[])?.accepted);
    Ok(())
}

#[test]
fn rank_counts_inactive_non_candidates_and_generated_source_shifts() -> TestResult {
    let mut value = fixture_value()?;
    let mut inactive = value["domain"]["entities"][id(1)].clone();
    inactive["id"] = json!(id(16));
    inactive["externalId"] = json!("synthetic-inactive");
    inactive["activeRange"] =
        json!({"kind":"dateRange","startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    value["domain"]["entities"][id(16)] = inactive;
    value["domain"]["entities"][id(6)] =
        workforce_fixture::fixture()?.domain.entities[&id(6).parse()?].clone();
    let document = serde_json::from_value(value)?;
    let report = verify_selection(&document, &[pair(1, 7)?, pair(1, 8)?, pair(20, 22)?])?;
    assert!(report.accepted);
    // Source people [1,16,20,21], shifts [7,8,22]: 1 + 2 + 9, not retained-variable ranks.
    assert_eq!(report.score.levels[0].value, 12);
    Ok(())
}

#[test]
fn every_true_and_false_decision_requires_unique_original_typed_references() -> TestResult {
    let document = small_document()?;
    for selected in [false, true] {
        for (person, shift) in [(999, 8), (11, 8), (1, 999), (1, 1)] {
            let mut solution = source_solution(&document, &[pair(person, shift)?])?;
            solution.assignments[0].value = AssignmentValue::Boolean(selected);
            assert!(score_workforce_solution(&document, &solution, None).is_err());
        }
        let mut solution = source_solution(&document, &[pair(1, 8)?])?;
        let mut duplicate = solution.assignments[0].clone();
        duplicate.value = AssignmentValue::Boolean(selected);
        solution.assignments.push(duplicate);
        assert!(score_workforce_solution(&document, &solution, None).is_err());
    }
    let mut solution = source_solution(&document, &[pair(1, 8)?])?;
    solution.assignments[0].entity.kind = "official.workforce.person".parse()?;
    assert!(score_workforce_solution(&document, &solution, None).is_err());
    solution = source_solution(&document, &[pair(1, 8)?])?;
    solution.assignments[0].value = AssignmentValue::Integer(1);
    assert!(score_workforce_solution(&document, &solution, None).is_err());
    for (field, value) in [
        ("packId", json!("official.foreign")),
        ("scenarioId", json!(id(999))),
        ("projectionVersion", json!(2)),
        ("schemaVersion", json!(2)),
    ] {
        let mut foreign = serde_json::to_value(source_solution(&document, &[pair(1, 8)?])?)?;
        foreign[field] = value;
        assert!(
            score_workforce_solution(&document, &serde_json::from_value(foreign)?, None).is_err()
        );
    }
    Ok(())
}

#[test]
fn stale_contexts_and_forged_scores_cannot_be_verified() -> TestResult {
    let document = small_document()?;
    let (_, _, accepted) = accepted_fixture(&document, true)?;
    let context = report_context(&accepted.verification, &accepted.solution)?;
    for field in [
        "documentHash",
        "normalizedSolutionHash",
        "verificationScopeChecksum",
        "scenarioId",
        "evaluatedRevision",
    ] {
        let mut value = serde_json::to_value(&context)?;
        value[field] = match field {
            "scenarioId" => json!(id(999)),
            "evaluatedRevision" => json!(2),
            _ => json!("f".repeat(64)),
        };
        let stale = serde_json::from_value(value)?;
        assert!(
            verify_workforce_solution(
                &document,
                &accepted.solution,
                &stale,
                &accepted.verification.score,
                None
            )
            .is_err(),
            "{field}"
        );
    }
    let mut forged = accepted.verification.score.clone();
    forged.levels[0].value = 99;
    *forged.levels[0]
        .category_breakdown
        .values_mut()
        .next()
        .ok_or("rank category")? = 99;
    assert!(
        verify_workforce_solution(&document, &accepted.solution, &context, &forged, None).is_err()
    );
    Ok(())
}

#[test]
fn real_reviewer_quarantines_bad_candidates_and_ignores_backend_rank_authority() -> TestResult {
    let document = small_document()?;
    let (problem, mut candidate, accepted) = accepted_fixture(&document, true)?;
    let clock = SystemVerificationClock::default();
    let reviewer = AcceptanceReviewer::new(&WorkforcePack, &document, 1, &problem, &clock)
        .map_err(|alarm| alarm.diagnostic_code)?;
    candidate.objective = Some(BackendObjectiveEvidence {
        objective_values: vec![999],
        best_bound_values: None,
    });
    let AcceptanceDecision::Accepted {
        result,
        objective_reconciliation,
        ..
    } = reviewer.review(&candidate, id(700).parse()?, &context().control)
    else {
        return Err("backend objective mismatch must not replace source score".into());
    };
    assert_eq!(
        objective_reconciliation,
        BackendObjectiveReconciliation::Mismatch
    );
    assert_eq!(result.verification.score, accepted.verification.score);
    let bad = tiny_candidate(&problem, false)?;
    let AcceptanceDecision::Quarantined { alarm, .. } =
        reviewer.review(&bad, id(701).parse()?, &context().control)
    else {
        return Err("infeasible original assignment was not quarantined".into());
    };
    assert_eq!(
        alarm.category,
        CorrectnessAlarmCategory::RequiredRuleRejected
    );
    Ok(())
}

#[test]
fn real_reviewer_rejects_a_validly_encoded_missing_coverage_compiler_mutant() -> TestResult {
    let document = small_document()?;
    let mut problem = WorkforcePack.compile(&document, &context())?;
    weaken_coverage(&mut problem)?;
    let clock = SystemVerificationClock::default();
    let reviewer = AcceptanceReviewer::new(&WorkforcePack, &document, 1, &problem, &clock)
        .map_err(|alarm| alarm.diagnostic_code)?;
    let AcceptanceDecision::Quarantined { alarm, .. } = reviewer.review(
        &tiny_candidate(&problem, false)?,
        id(701).parse()?,
        &context().control,
    ) else {
        return Err("weakened compiler model became feasibility authority".into());
    };
    assert_eq!(
        alarm.category,
        CorrectnessAlarmCategory::RequiredRuleRejected
    );
    Ok(())
}

pub(super) fn weaken_coverage(problem: &mut PlanningProblem) -> TestResult {
    let mut weakened = false;
    for constraint in &mut problem.constraints {
        match &mut constraint.body {
            Constraint::ExactlyOne { literals } => {
                constraint.body = Constraint::CardinalityRange {
                    literals: literals.clone(),
                    min: 0,
                    max: 1,
                };
                weakened = true;
            }
            Constraint::CardinalityRange { min, .. } if *min > 0 => {
                *min = 0;
                weakened = true;
            }
            _ => {}
        }
    }
    assert!(weakened, "fixture must exercise a real coverage mutation");
    problem.declared_capabilities = feature_usage(problem).required_capabilities();
    eutheto_planning_ir::validate(problem, PlanningIrLimitsV1::DEFAULT)?;
    Ok(())
}

#[test]
fn accepted_share_is_closed_identity_only_and_rejects_freshly_checksummed_lies() -> TestResult {
    let mut document = small_document()?;
    document.metadata.description = "SYNTHETIC_PRIVATE_NOTE".to_owned();
    document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["name"] = json!("SYNTHETIC_PRIVATE_NAME");
    let (_, _, accepted) = accepted_fixture(&document, true)?;
    let catalog = WorkforcePack.catalog()?;
    for include_evidence_references in [false, true] {
        let share = build_workforce_share_result(
            &document,
            &accepted,
            ShareResultOptions {
                include_evidence_references,
            },
        )?;
        let serialized = serde_json::to_string(&share)?;
        assert!(!serialized.contains("SYNTHETIC_PRIVATE"));
        assert_eq!(share.payload["assignments"][0]["personId"], id(1));
        assert_eq!(share.payload["assignments"][0]["shiftId"], id(8));
        assert_eq!(
            share.payload.get("evidenceReferences").is_some(),
            include_evidence_references
        );
        validate_contract_value(
            &catalog.share_result_schema,
            &share.payload,
            ContractJsonLimits::DEFAULT,
        )?;
        let mut excessive = share.payload.clone();
        excessive["notes"] = json!("SYNTHETIC_PRIVATE_NOTE");
        assert!(
            validate_contract_value(
                &catalog.share_result_schema,
                &excessive,
                ContractJsonLimits::DEFAULT
            )
            .is_err()
        );
        let mut newer = share.payload;
        newer["schemaVersion"] = json!(2);
        assert!(
            validate_contract_value(
                &catalog.share_result_schema,
                &newer,
                ContractJsonLimits::DEFAULT
            )
            .is_err()
        );
    }
    let context = report_context(&accepted.verification, &accepted.solution)?;
    let missing = VerificationReport::new(
        &context,
        Vec::new(),
        accepted.verification.score.clone(),
        Vec::new(),
        accepted.verification.metrics.clone(),
    )?;
    let forged = AcceptedResult::new(accepted.solution.clone(), missing)?;
    forged.validate()?;
    assert!(
        build_workforce_share_result(
            &document,
            &forged,
            ShareResultOptions {
                include_evidence_references: false
            }
        )
        .is_err()
    );
    let mut stale = document.clone();
    stale.metadata.description = "different synthetic source snapshot".to_owned();
    assert!(
        build_workforce_share_result(
            &stale,
            &accepted,
            ShareResultOptions {
                include_evidence_references: false
            }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn even_unselected_hidden_share_evidence_must_come_from_the_original_pair() -> TestResult {
    let mut document = small_document()?;
    document.domain.rules.clear();
    let (_, _, accepted) = accepted_fixture(&document, false)?;
    let mut solution = accepted.solution.clone();
    solution.assignments[0].evidence = vec!["test.foreign.evidence".parse()?];
    let context = report_context(&accepted.verification, &solution)?;
    let report = VerificationReport::new(
        &context,
        accepted.verification.required_rule_results.clone(),
        accepted.verification.score.clone(),
        Vec::new(),
        accepted.verification.metrics.clone(),
    )?;
    let forged = AcceptedResult::new(solution, report)?;
    forged.validate()?;
    assert!(
        build_workforce_share_result(
            &document,
            &forged,
            ShareResultOptions {
                include_evidence_references: false
            }
        )
        .is_err()
    );
    let share = build_workforce_share_result(
        &document,
        &accepted,
        ShareResultOptions {
            include_evidence_references: true,
        },
    )?;
    assert_eq!(share.payload["assignments"], json!([]));
    assert_eq!(share.payload["evidenceReferences"], json!([]));
    Ok(())
}

#[test]
fn assignment_rendering_rederives_pair_facts_and_rejects_aggregate_or_forged_context() -> TestResult
{
    let document = small_document()?;
    let (problem, _, accepted) = accepted_fixture(&document, true)?;
    let mut evidence = assignment_evidence(&accepted);
    let term = &problem.objectives.levels[0].terms[0];
    evidence.score_contributions = vec![ScoreContributionV1 {
        evidence_id: term.provenance.as_str().parse()?,
        level_id: "official.workforce.objective.assignment.rank".parse()?,
        category_id: Some(term.category.clone()),
        value: 1,
    }];
    let request = assignment_request(evidence.clone())?;
    let rendered = render_workforce_evidence(&document, &request)?;
    assert_eq!(rendered.evidence_checksum, request.evidence.checksum);
    assert_eq!(
        rendered.messages[0].parameters[&"official.workforce.fact.rank_contribution".parse()?],
        VerificationValue::Integer(1)
    );
    let mut stale_evidence = evidence.clone();
    let person_rule = id(1).parse()?;
    stale_evidence.related_rules = accepted
        .verification
        .required_rule_results
        .iter()
        .filter(|rule| rule.rule_id == person_rule)
        .cloned()
        .collect();
    let mut changed = document.clone();
    changed
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["activeRange"] =
        json!({"kind":"dateRange","startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    assert!(render_workforce_evidence(&changed, &assignment_request(stale_evidence)?).is_err());
    let aggregate_rule = id(30).parse()?;
    evidence.related_rules = accepted
        .verification
        .required_rule_results
        .iter()
        .filter(|rule| rule.rule_id == aggregate_rule)
        .cloned()
        .collect();
    assert!(render_workforce_evidence(&document, &assignment_request(evidence.clone())?).is_err());
    evidence.related_rules.clear();
    evidence.score_contributions[0].value = 99;
    assert!(render_workforce_evidence(&document, &assignment_request(evidence.clone())?).is_err());
    evidence.score_contributions.clear();
    evidence.metrics.insert(
        "official.workforce.metric.selected_assignments".parse()?,
        MetricValue::Integer(99),
    );
    assert!(render_workforce_evidence(&document, &assignment_request(evidence)?).is_err());
    Ok(())
}

#[test]
fn validation_rendering_is_current_code_summary_not_submitted_prose_or_stale_occurrence()
-> TestResult {
    let mut document = small_document()?;
    document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or("shift")?["coverage"]["count"] = json!(3);
    let findings = WorkforcePack.validate_full(
        &document,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let current = findings
        .issues
        .first()
        .ok_or("expected coverage readiness finding")?;
    let issue = ValidationIssueEvidenceV1 {
        issue_id: current.code.parse()?,
        severity: ExplanationValidationSeverity::MustFix,
        message_key: current.code.clone(),
        parameters: BTreeMap::new(),
        field_path: None,
        entity: None,
        rule_id: None,
    };
    let request = EvidenceRenderRequestV1::new(ExplanationEvidenceV1::new(
        ExplanationEvidencePayloadV1::Validation {
            issue: issue.clone(),
        },
    )?)?;
    let rendered = render_workforce_evidence(&document, &request)?;
    assert_eq!(rendered.evidence_checksum, request.evidence.checksum);
    let mut forged = issue;
    forged.parameters.insert(
        "test.submitted.prose".parse()?,
        VerificationValue::Text("SYNTHETIC_PRIVATE_PROSE".to_owned()),
    );
    let forged = EvidenceRenderRequestV1::new(ExplanationEvidenceV1::new(
        ExplanationEvidencePayloadV1::Validation { issue: forged },
    )?)?;
    let Err(error) = render_workforce_evidence(&document, &forged) else {
        return Err("submitted prose must not render".into());
    };
    assert!(!error.to_string().contains("SYNTHETIC_PRIVATE"));
    document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or("shift")?["coverage"]["count"] = json!(1);
    assert!(render_workforce_evidence(&document, &request).is_err());
    Ok(())
}

#[test]
fn typed_input_bounds_precede_generic_checksums_and_cancellation_precedes_semantics() -> TestResult
{
    let mut document = small_document()?;
    let (_, _, accepted) = accepted_fixture(&document, true)?;
    let solution = &accepted.solution;
    let token = CancellationToken::new();
    token.cancel();
    let control = eutheto_types::OperationControl::Cancellation(token);
    assert_eq!(
        workforce_verification_scope(&document, u64::MAX, Some(&control)),
        Err(DomainPackError::Cancelled)
    );
    assert_eq!(
        score_workforce_solution(&document, solution, Some(&control)),
        Err(DomainPackError::Cancelled)
    );
    let context = report_context(&accepted.verification, solution)?;
    assert_eq!(
        verify_workforce_solution(
            &document,
            solution,
            &context,
            &accepted.verification.score,
            Some(&control)
        ),
        Err(DomainPackError::Cancelled)
    );
    let limit = DomainPackError::Contract(
        AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord)
            .code()
            .to_owned(),
    );
    document.metadata.description = "\0".repeat(ContractJsonLimits::DEFAULT.max_string_bytes + 1);
    assert_eq!(
        score_workforce_solution(&document, solution, None),
        Err(limit.clone())
    );
    document.metadata.description.clear();
    let mut nested = json!(0);
    for _ in 0..40 {
        nested = json!([nested]);
    }
    document
        .extensions
        .insert("nonsemantic.deep".to_owned(), nested);
    assert_eq!(
        score_workforce_solution(&document, solution, None),
        Err(limit.clone())
    );
    document.extensions.remove("nonsemantic.deep");
    let mut request = assignment_request(assignment_evidence(&accepted))?;
    let ExplanationEvidencePayloadV1::Assignment { assignment } = &mut request.evidence.evidence
    else {
        return Err("assignment fixture".into());
    };
    let mut rule = accepted.verification.required_rule_results[0].clone();
    rule.observed.insert(
        "test.oversized".parse()?,
        VerificationValue::Text("x".repeat(eutheto_domain_ir::MAX_VERIFICATION_TEXT_BYTES + 1)),
    );
    assignment.related_rules.push(rule);
    // Deliberately stale checksum: nested resource classification must win before hashing.
    assert_eq!(render_workforce_evidence(&document, &request), Err(limit));
    Ok(())
}

#[test]
fn every_applicable_available_only_predicate_is_required() -> TestResult {
    let mut value = fixture_value()?;
    let mut available = value["domain"]["entities"][id(23)].clone();
    available["id"] = json!(id(26));
    available["personId"] = json!(id(1));
    available["availabilityKind"] = json!("availableOnly");
    available["timeWindow"] =
        json!({"kind":"instant","startsAt":"2026-11-01T08:00:00Z","endsAt":"2026-11-01T10:00:00Z"});
    value["domain"]["entities"][id(26)] = available.clone();
    let selected = [pair(1, 8)?, pair(20, 22)?];
    assert!(verify_selection(&serde_json::from_value(value.clone())?, &selected)?.accepted);
    available["id"] = json!(id(27));
    available["timeWindow"] =
        json!({"kind":"instant","startsAt":"2026-11-01T09:00:00Z","endsAt":"2026-11-01T11:00:00Z"});
    value["domain"]["entities"][id(27)] = available;
    assert!(!verify_selection(&serde_json::from_value(value)?, &selected)?.accepted);
    Ok(())
}

#[test]
fn editable_reserved_data_never_silently_becomes_supported_execution() -> TestResult {
    let baseline = small_document()?;
    for (collection, record) in [
        (
            "rules",
            json!({"kind":"maximumAssignmentCount","id":id(40),"active":true,"strength":"required","scope":{"people":{"kind":"all"}},"calendarId":id(2),"maximum":2}),
        ),
        (
            "preferences",
            json!({"kind":"assignmentType","id":id(40),"active":true,"scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,"direction":"prefer","assignmentTypeIds":[id(4)]}),
        ),
        (
            "lockedAssignments",
            json!({"id":id(40),"personId":id(1),"shiftId":id(8),"state":{"kind":"hard"}}),
        ),
        (
            "lockedAssignments",
            json!({"id":id(40),"personId":id(1),"shiftId":id(8),"state":{"kind":"soft","stabilityWeight":1}}),
        ),
    ] {
        let mut value = serde_json::to_value(&baseline)?;
        value["domain"][collection][id(40)] = record;
        let document = serde_json::from_value(value.clone())?;
        assert!(WorkforcePack.validate_fast(&document).issues.is_empty());
        assert!(workforce_verification_scope(&document, 1, None).is_err());
        assert!(
            score_workforce_solution(&document, &source_solution(&document, &[])?, None).is_err()
        );
        if collection == "lockedAssignments" {
            value["domain"][collection][id(40)]["state"] = json!({"kind":"unlocked"});
        } else {
            value["domain"][collection][id(40)]["active"] = json!(false);
        }
        let inactive = serde_json::from_value(value)?;
        let baseline_score =
            score_workforce_solution(&baseline, &source_solution(&baseline, &[])?, None)?;
        assert_eq!(
            score_workforce_solution(&inactive, &source_solution(&inactive, &[])?, None)?,
            baseline_score
        );
        workforce_verification_scope(&inactive, 1, None)?;
    }
    let mut missing = baseline.clone();
    missing.domain.entities.remove(&id(9).parse()?);
    assert!(WorkforcePack.validate_fast(&missing).issues.is_empty());
    let expected =
        DomainPackError::Contract(AssignmentRuleError::MissingScorePolicy.code().to_owned());
    assert_eq!(
        workforce_verification_scope(&missing, 1, None),
        Err(expected.clone())
    );
    assert_eq!(
        score_workforce_solution(&missing, &source_solution(&missing, &[])?, None),
        Err(expected)
    );
    let mut malformed = baseline;
    malformed
        .domain
        .entities
        .get_mut(&id(9).parse()?)
        .ok_or("score policy")?["priorityMapping"][0]["scale"] = json!(u64::MAX);
    assert!(!WorkforcePack.validate_fast(&malformed).issues.is_empty());
    assert!(
        score_workforce_solution(&malformed, &source_solution(&malformed, &[])?, None).is_err()
    );
    Ok(())
}

#[test]
fn aggregate_small_report_fields_are_bounded_before_stale_checksum_validation() -> TestResult {
    let document = small_document()?;
    let (_, _, mut excessive) = accepted_fixture(&document, true)?;
    for index in 0..50_000 {
        excessive.verification.metrics.insert(
            format!("test.workforce.large.metric.m{index:05}").parse()?,
            MetricValue::Integer(0),
        );
    }
    let expected = DomainPackError::Contract(
        AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::Bytes)
            .code()
            .to_owned(),
    );
    assert_eq!(
        build_workforce_share_result(
            &document,
            &excessive,
            ShareResultOptions {
                include_evidence_references: false
            }
        ),
        Err(expected),
    );
    Ok(())
}
