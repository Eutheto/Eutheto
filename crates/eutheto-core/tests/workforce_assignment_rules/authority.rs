use super::{TestResult, context, fixture_value, id, pair, selections, workforce_fixture};
use eutheto_domain_api::{
    ContractJsonLimits, DomainPack, DomainPackError, ShareResultOptions, validate_contract_value,
};
use eutheto_domain_ir::{
    AcceptedResult, AssignmentEvidenceV1, AssignmentValue, DomainAssignment, DomainEntityRef,
    EvidenceRenderRequestV2, ExplanationEvidencePayloadV2, ExplanationEvidenceV2,
    ExplanationValidationSeverity, MetricValue, NormalizedSolution, ScoreContributionV1,
    ValidationIssueEvidenceV1, VerificationContextV1, VerificationReport, VerificationValue,
    blake3_hex, validation_issue_identity,
};
use eutheto_planning_ir::{
    CandidateValues, Constraint, PlanningIrLimitsV1, PlanningProblem, Variable, feature_usage,
};
use eutheto_solver_api::{BackendCandidate, BackendObjectiveEvidence};
use eutheto_types::{CancellationToken, DurationMillis, OperationControl, ScenarioDocument};
use eutheto_verify::{
    AcceptanceDecision, AcceptanceReviewer, BackendObjectiveReconciliation,
    CorrectnessAlarmCategory, SystemVerificationClock,
};
use eutheto_workforce::{
    WorkforcePack,
    assignment_rules::{
        AssignmentRuleError, build_workforce_share_result, render_workforce_evidence,
        score_workforce_solution, verify_workforce_solution, workforce_verification_scope,
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
    let authoritative = score_workforce_solution(document, &solution, None, None)?;
    Ok(verify_workforce_solution(
        document,
        &solution,
        None,
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
    let reviewer = AcceptanceReviewer::new(&WorkforcePack, document, 1, &problem, None, &clock)
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

fn assignment_request(evidence: AssignmentEvidenceV1) -> Result<EvidenceRenderRequestV2> {
    Ok(EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Assignment {
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
            assert!(score_workforce_solution(&document, &solution, None, None).is_err());
        }
        let mut solution = source_solution(&document, &[pair(1, 8)?])?;
        let mut duplicate = solution.assignments[0].clone();
        duplicate.value = AssignmentValue::Boolean(selected);
        solution.assignments.push(duplicate);
        assert!(score_workforce_solution(&document, &solution, None, None).is_err());
    }
    let mut solution = source_solution(&document, &[pair(1, 8)?])?;
    solution.assignments[0].entity.kind = "official.workforce.person".parse()?;
    assert!(score_workforce_solution(&document, &solution, None, None).is_err());
    solution = source_solution(&document, &[pair(1, 8)?])?;
    solution.assignments[0].value = AssignmentValue::Integer(1);
    assert!(score_workforce_solution(&document, &solution, None, None).is_err());
    for (field, value) in [
        ("packId", json!("official.foreign")),
        ("scenarioId", json!(id(999))),
        ("projectionVersion", json!(2)),
        ("schemaVersion", json!(2)),
    ] {
        let mut foreign = serde_json::to_value(source_solution(&document, &[pair(1, 8)?])?)?;
        foreign[field] = value;
        assert!(
            score_workforce_solution(&document, &serde_json::from_value(foreign)?, None, None)
                .is_err()
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
                None,
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
        verify_workforce_solution(&document, &accepted.solution, None, &context, &forged, None)
            .is_err()
    );
    Ok(())
}

#[test]
fn real_reviewer_quarantines_bad_candidates_and_ignores_backend_rank_authority() -> TestResult {
    let document = small_document()?;
    let (problem, mut candidate, accepted) = accepted_fixture(&document, true)?;
    let clock = SystemVerificationClock::default();
    let reviewer = AcceptanceReviewer::new(&WorkforcePack, &document, 1, &problem, None, &clock)
        .map_err(|alarm| alarm.diagnostic_code)?;
    candidate.objective = Some(BackendObjectiveEvidence {
        objective_values: vec![999],
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
    let reviewer = AcceptanceReviewer::new(&WorkforcePack, &document, 1, &problem, None, &clock)
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
            None,
            ShareResultOptions {
                include_evidence_references,
            },
        )?;
        let serialized = serde_json::to_string(&share)?;
        assert_eq!(share.schema_version, 2);
        assert_eq!(share.payload["schemaVersion"], 2);
        assert!(share.payload.get("score").is_none());
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
        newer["schemaVersion"] = json!(3);
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
            None,
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
            None,
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
            None,
            ShareResultOptions {
                include_evidence_references: false
            }
        )
        .is_err()
    );
    let share = build_workforce_share_result(
        &document,
        &accepted,
        None,
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
    let rendered = render_workforce_evidence(
        &document,
        &request,
        None,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
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
    assert!(
        render_workforce_evidence(
            &changed,
            &assignment_request(stale_evidence)?,
            None,
            &OperationControl::Cancellation(CancellationToken::new())
        )
        .is_err()
    );
    let aggregate_rule = id(30).parse()?;
    evidence.related_rules = accepted
        .verification
        .required_rule_results
        .iter()
        .filter(|rule| rule.rule_id == aggregate_rule)
        .cloned()
        .collect();
    assert!(
        render_workforce_evidence(
            &document,
            &assignment_request(evidence.clone())?,
            None,
            &OperationControl::Cancellation(CancellationToken::new())
        )
        .is_err()
    );
    evidence.related_rules.clear();
    evidence.score_contributions[0].value = 99;
    assert!(
        render_workforce_evidence(
            &document,
            &assignment_request(evidence.clone())?,
            None,
            &OperationControl::Cancellation(CancellationToken::new())
        )
        .is_err()
    );
    evidence.score_contributions.clear();
    evidence.metrics.insert(
        "official.workforce.metric.selected_assignments".parse()?,
        MetricValue::Integer(99),
    );
    assert!(
        render_workforce_evidence(
            &document,
            &assignment_request(evidence)?,
            None,
            &OperationControl::Cancellation(CancellationToken::new())
        )
        .is_err()
    );
    Ok(())
}

fn with_pair_preferences(mut document: ScenarioDocument) -> Result<ScenarioDocument> {
    let mut other_person = document
        .domain
        .entities
        .get(&id(1).parse()?)
        .ok_or("source person")?
        .clone();
    other_person["id"] = json!(id(60));
    other_person["name"] = json!("Other clinician");
    other_person["externalId"] = json!("staff-60");
    document
        .domain
        .entities
        .insert(id(60).parse()?, other_person);
    document.domain.entities.insert(id(50).parse()?, json!({
        "kind":"availability","id":id(50),"personId":id(1),
        "availabilityKind":"requestedTimeOff",
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T05:30:00Z","endsAt":"2026-11-01T07:30:00Z"},
        "source":"private-request-source","note":"PRIVATE_LEAVE_NOTE"
    }));
    document.domain.entities.insert(
        id(45).parse()?,
        json!({
            "kind":"baseSchedule","id":id(45),"sourceSolutionId":id(47),
            "sourceRevision":1,"assignments":[{"personId":id(1),"shiftId":id(8)}]
        }),
    );
    document.domain.preferences.insert(
        id(40).parse()?,
        json!({
            "kind":"assignmentType","id":id(40),"active":true,
            "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
            "direction":"prefer","assignmentTypeIds":[id(4)]
        }),
    );
    document.domain.preferences.insert(
        id(41).parse()?,
        json!({
            "kind":"location","id":id(41),"active":true,
            "scope":{"people":{"kind":"all"}},"priority":"low","weight":1,
            "direction":"avoid","locationIds":[id(5)]
        }),
    );
    document.domain.preferences.insert(
        id(42).parse()?,
        json!({
            "kind":"requestedTimeOff","id":id(42),"active":true,
            "scope":{"people":{"kind":"all"}},"priority":"high","weight":1,
            "availabilityIds":[id(50)]
        }),
    );
    document.domain.preferences.insert(
        id(46).parse()?,
        json!({
            "kind":"baseStability","id":id(46),"active":true,
            "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
            "baseScheduleId":id(45)
        }),
    );
    document.domain.preferences.insert(
        id(43).parse()?,
        json!({
            "kind":"location","id":id(43),"active":false,
            "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
            "direction":"avoid","locationIds":[id(5)]
        }),
    );
    document.domain.preferences.insert(
        id(44).parse()?,
        json!({
            "kind":"assignmentType","id":id(44),"active":true,
            "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
            "direction":"prefer","assignmentTypeIds":[id(4)]
        }),
    );
    let preference = document
        .domain
        .preferences
        .get_mut(&id(44).parse()?)
        .ok_or("unmatched preference")?;
    preference["scope"]["people"] =
        json!({"kind":"filter","allTags":["not-a-clinician"],"anyTags":[]});
    Ok(document)
}

#[test]
fn selected_assignment_reports_only_matching_source_preference_facts_without_global_score_claims()
-> TestResult {
    let document = small_document()?;
    let (_, _, accepted) = accepted_fixture(&document, true)?;
    let document = with_pair_preferences(document)?;
    assert!(verify_selection(&document, &[pair(1, 8)?])?.accepted);
    let rendered = render_workforce_evidence(
        &document,
        &assignment_request(assignment_evidence(&accepted))?,
        None,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let preference_facts = rendered
        .messages
        .iter()
        .filter(|message| {
            message
                .message_key
                .starts_with("official.workforce.assignment.preference.")
        })
        .collect::<Vec<_>>();
    let expected_rules = [
        id(40).parse()?,
        id(41).parse()?,
        id(42).parse()?,
        id(46).parse()?,
    ];
    assert_eq!(preference_facts.len(), expected_rules.len());
    for (message, rule) in preference_facts.iter().zip(expected_rules) {
        assert_eq!(message.rules.as_slice(), &[rule]);
    }
    assert_eq!(
        preference_facts
            .iter()
            .map(|message| message.message_key.as_str())
            .collect::<Vec<_>>(),
        [
            "official.workforce.assignment.preference.matched_prefer",
            "official.workforce.assignment.preference.matched_avoid",
            "official.workforce.assignment.preference.requested_time_off",
            "official.workforce.assignment.preference.base_stability"
        ]
    );
    for (message, (selected, unselected)) in
        preference_facts
            .iter()
            .zip([(0, 10), (1, 0), (100, 0), (0, 10)])
    {
        assert_eq!(
            message
                .parameters
                .get(&"official.workforce.fact.selected_pair_penalty".parse()?),
            Some(&VerificationValue::Integer(selected))
        );
        assert_eq!(
            message
                .parameters
                .get(&"official.workforce.fact.unselected_pair_penalty".parse()?),
            Some(&VerificationValue::Integer(unselected))
        );
        assert_eq!(
            message
                .parameters
                .get(&"official.workforce.fact.objective_level".parse()?),
            Some(&VerificationValue::Text(
                "official.workforce.objective.preference.preferences".to_owned()
            ))
        );
    }
    let text = serde_json::to_string(&rendered)?;
    assert!(!text.contains("PRIVATE_LEAVE_NOTE"));
    assert!(!text.contains("private-request-source"));
    assert!(!text.contains("fairness contribution"));
    Ok(())
}

#[test]
fn selected_pair_not_in_source_base_has_the_exact_stability_penalty() -> TestResult {
    let document = small_document()?;
    let (_, _, accepted) = accepted_fixture(&document, true)?;
    let mut document = with_pair_preferences(document)?;
    document
        .domain
        .entities
        .get_mut(&id(45).parse()?)
        .ok_or("base schedule")?["assignments"] = json!([{"personId":id(60),"shiftId":id(8)}]);
    assert!(verify_selection(&document, &[pair(1, 8)?])?.accepted);
    let rendered = render_workforce_evidence(
        &document,
        &assignment_request(assignment_evidence(&accepted))?,
        None,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let stability_rule = id(46).parse()?;
    let stability = rendered
        .messages
        .iter()
        .find(|message| message.rules.as_slice() == std::slice::from_ref(&stability_rule))
        .ok_or("base stability missing")?;
    assert_eq!(
        stability
            .parameters
            .get(&"official.workforce.fact.selected_pair_penalty".parse()?),
        Some(&VerificationValue::Integer(10))
    );
    assert_eq!(
        stability
            .parameters
            .get(&"official.workforce.fact.unselected_pair_penalty".parse()?),
        Some(&VerificationValue::Integer(0))
    );
    Ok(())
}

#[test]
fn optional_preference_volume_preserves_recorded_pair_evidence_and_labels_the_bound() -> TestResult
{
    let mut document = small_document()?;
    let (_, _, accepted) = accepted_fixture(&document, true)?;
    for index in 1_000..2_000 {
        document.domain.preferences.insert(
            id(index).parse()?,
            json!({
                "kind":"assignmentType","id":id(index),"active":true,
                "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
                "direction":"prefer","assignmentTypeIds":[id(4)]
            }),
        );
    }
    let rendered = render_workforce_evidence(
        &document,
        &assignment_request(assignment_evidence(&accepted))?,
        None,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_eq!(
        rendered.messages[0].message_key,
        "official.workforce.assignment.recorded_decision"
    );
    let preferences = rendered
        .messages
        .iter()
        .filter(|message| {
            message
                .message_key
                .starts_with("official.workforce.assignment.preference.")
        })
        .collect::<Vec<_>>();
    assert_eq!(preferences.len(), 129);
    assert_eq!(preferences[0].rules.as_slice(), &[id(1_000).parse()?]);
    assert_eq!(preferences[127].rules.as_slice(), &[id(1_127).parse()?]);
    assert_eq!(
        preferences[128].message_key,
        "official.workforce.assignment.preference.limit"
    );
    assert!(preferences[128].rules.is_empty());
    Ok(())
}

#[test]
fn validation_rendering_requires_exact_current_native_issue_and_rejects_submitted_prose()
-> TestResult {
    let mut document = small_document()?;
    document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or("shift")?["coverage"]["count"] = json!(3);
    let findings = WorkforcePack.validate_full(
        &document,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let current = findings
        .issues
        .first()
        .ok_or("expected coverage readiness finding")?;
    let issue = ValidationIssueEvidenceV1 {
        issue_id: validation_issue_identity(current)?,
        severity: ExplanationValidationSeverity::MustFix,
        message_key: current.code.clone(),
        parameters: BTreeMap::new(),
        field_path: None,
        entity: None,
        rule_id: None,
    };
    let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Validation {
            issue: issue.clone(),
        },
    )?)?;
    let rendered = render_workforce_evidence(
        &document,
        &request,
        None,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_eq!(rendered.evidence_checksum, request.evidence.checksum);
    let mut forged = issue;
    forged.parameters.insert(
        "test.submitted.prose".parse()?,
        VerificationValue::Text("SYNTHETIC_PRIVATE_PROSE".to_owned()),
    );
    let forged = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Validation { issue: forged },
    )?)?;
    let Err(error) = render_workforce_evidence(
        &document,
        &forged,
        None,
        &OperationControl::Cancellation(CancellationToken::new()),
    ) else {
        return Err("submitted prose must not render".into());
    };
    assert!(!error.to_string().contains("SYNTHETIC_PRIVATE"));
    document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or("shift")?["coverage"]["count"] = json!(1);
    assert!(
        render_workforce_evidence(
            &document,
            &request,
            None,
            &OperationControl::Cancellation(CancellationToken::new())
        )
        .is_err()
    );
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
    let control = OperationControl::Cancellation(token);
    assert_eq!(
        workforce_verification_scope(&document, u64::MAX, Some(&control)),
        Err(DomainPackError::Cancelled)
    );
    assert_eq!(
        score_workforce_solution(&document, solution, None, Some(&control)),
        Err(DomainPackError::Cancelled)
    );
    let context = report_context(&accepted.verification, solution)?;
    assert_eq!(
        verify_workforce_solution(
            &document,
            solution,
            None,
            &context,
            &accepted.verification.score,
            Some(&control)
        ),
        Err(DomainPackError::Cancelled)
    );
    let limit = DomainPackError::ResourceLimitExceeded;
    document.metadata.description = "\0".repeat(ContractJsonLimits::DEFAULT.max_string_bytes + 1);
    assert_eq!(
        score_workforce_solution(&document, solution, None, None),
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
        score_workforce_solution(&document, solution, None, None),
        Err(limit.clone())
    );
    document.extensions.remove("nonsemantic.deep");
    let mut request = assignment_request(assignment_evidence(&accepted))?;
    let ExplanationEvidencePayloadV2::Assignment { assignment } = &mut request.evidence.evidence
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
    assert_eq!(
        render_workforce_evidence(
            &document,
            &request,
            None,
            &OperationControl::Cancellation(CancellationToken::new())
        ),
        Err(limit)
    );
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
fn time_and_type_preferences_rescore_only_statically_feasible_pairs() -> TestResult {
    let mut value = serde_json::to_value(small_document()?)?;
    let mut ineligible = value["domain"]["entities"][id(1)].clone();
    ineligible["id"] = json!(id(20));
    ineligible["externalId"] = json!("staff-20");
    ineligible["qualificationGrants"] = json!([]);
    value["domain"]["entities"][id(20)] = ineligible;
    value["domain"]["rules"][id(33)] = json!({
        "id":id(33),"kind":"eligibility","active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}}
    });
    value["domain"]["preferences"][id(31)] = json!({
        "kind":"time","id":id(31),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":2,
        "direction":"prefer",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T05:30:00Z",
                      "endsAt":"2026-11-01T05:31:00Z"}
    });
    value["domain"]["preferences"][id(32)] = json!({
        "kind":"assignmentType","id":id(32),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"high","weight":1,
        "direction":"avoid","assignmentTypeIds":[id(4)]
    });
    let document: ScenarioDocument = serde_json::from_value(value.clone())?;
    let idle = score_workforce_solution(&document, &source_solution(&document, &[])?, None, None)?;
    let assigned = score_workforce_solution(
        &document,
        &source_solution(&document, &[pair(1, 8)?])?,
        None,
        None,
    )?;
    assert_eq!(idle.levels[0].value, 20);
    assert_eq!(assigned.levels[0].value, 100);
    let verified = verify_selection(&document, &[pair(1, 8)?])?;
    assert!(verified.accepted);
    assert_eq!(verified.score, assigned);

    // The time window ends exactly at the shift's start, so the half-open
    // intervals no longer overlap. Avoiding the selected type still costs 100.
    value["domain"]["preferences"][id(31)]["timeWindow"] = json!({"kind":"instant","startsAt":"2026-11-01T05:29:00Z",
               "endsAt":"2026-11-01T05:30:00Z"});
    let boundary: ScenarioDocument = serde_json::from_value(value)?;
    let missed =
        score_workforce_solution(&boundary, &source_solution(&boundary, &[])?, None, None)?;
    assert_eq!(missed.levels[0].value, 0);
    Ok(())
}

#[test]
fn soft_locks_require_base_context_and_scoring_requires_valid_policy() -> TestResult {
    let baseline = small_document()?;
    let mut value = serde_json::to_value(&baseline)?;
    value["domain"]["lockedAssignments"][id(40)] = json!({"id":id(40),"personId":id(1),"shiftId":id(8),"state":{"kind":"soft","stabilityWeight":1}});
    let document = serde_json::from_value(value.clone())?;
    assert!(WorkforcePack.validate_fast(&document).issues.is_empty());
    assert!(
        score_workforce_solution(&document, &source_solution(&document, &[])?, None, None).is_err()
    );
    value["domain"]["lockedAssignments"][id(40)]["state"] = json!({"kind":"unlocked"});
    let inactive = serde_json::from_value(value)?;
    let baseline_score =
        score_workforce_solution(&baseline, &source_solution(&baseline, &[])?, None, None)?;
    assert_eq!(
        score_workforce_solution(&inactive, &source_solution(&inactive, &[])?, None, None)?,
        baseline_score
    );
    workforce_verification_scope(&inactive, 1, None)?;
    let mut missing = baseline.clone();
    missing.domain.entities.remove(&id(9).parse()?);
    assert!(WorkforcePack.validate_fast(&missing).issues.is_empty());
    let expected =
        DomainPackError::Contract(AssignmentRuleError::MissingScorePolicy.code().to_owned());
    assert_eq!(
        score_workforce_solution(&missing, &source_solution(&missing, &[])?, None, None),
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
        score_workforce_solution(&malformed, &source_solution(&malformed, &[])?, None, None)
            .is_err()
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
    let expected = DomainPackError::ResourceLimitExceeded;
    assert_eq!(
        build_workforce_share_result(
            &document,
            &excessive,
            None,
            ShareResultOptions {
                include_evidence_references: false
            }
        ),
        Err(expected),
    );
    Ok(())
}

struct DiagnosticFinalStopClock {
    samples: std::sync::atomic::AtomicUsize,
    stop_at_sample: usize,
    deadline: bool,
    expired: std::sync::atomic::AtomicBool,
    cancellation: CancellationToken,
    limit: DurationMillis,
}

impl eutheto_types::MonotonicClock for DiagnosticFinalStopClock {
    fn now(&self) -> std::time::Duration {
        if self.expired.load(std::sync::atomic::Ordering::SeqCst) {
            self.limit.to_duration()
        } else {
            std::time::Duration::ZERO
        }
    }
}

impl eutheto_verify::VerificationClock for DiagnosticFinalStopClock {
    fn now_milliseconds(&self) -> DurationMillis {
        if self
            .samples
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == self.stop_at_sample
        {
            if self.deadline {
                self.expired
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            } else {
                self.cancellation.cancel();
            }
        }
        if self.expired.load(std::sync::atomic::Ordering::SeqCst) {
            self.limit
        } else {
            DurationMillis::ZERO
        }
    }
}

fn diagnostic_stop_decision(
    deadline: bool,
    stop_at_sample: usize,
) -> Result<eutheto_verify::DiagnosticDecision> {
    let document = small_document()?;
    let compile_context = context();
    let base = WorkforcePack.compile(&document, &compile_context)?;
    let clock = std::sync::Arc::new(DiagnosticFinalStopClock {
        samples: std::sync::atomic::AtomicUsize::new(0),
        stop_at_sample,
        deadline,
        expired: std::sync::atomic::AtomicBool::new(false),
        cancellation: CancellationToken::new(),
        limit: DurationMillis::new(1_000)?,
    });
    let parent = eutheto_types::ParentSolveBudget::new(
        clock.limit,
        clock.clone(),
        clock.cancellation.clone(),
    )?;
    let problem = WorkforcePack.compile_infeasibility(
        &document,
        &eutheto_domain_api::InfeasibilityCompileContext {
            base_problem: &base,
            compile_context: &compile_context,
            condition: None,
            budget: parent.phase_view(),
        },
    )?;
    let candidate = tiny_candidate(&problem, true)?;
    let control = OperationControl::Solve(parent.phase_view());
    let scope = WorkforcePack.diagnostic_verification_scope(&document, 1, &control)?;
    let mut preparation = eutheto_planning_ir::PlanningPreparationBudget::new(
        control.clone(),
        compile_context.planning_limits,
    );
    let prepared = eutheto_planning_ir::PreparedPlanningModel::prepare(
        std::sync::Arc::new(problem),
        compile_context.planning_limits,
        &mut preparation,
    )
    .map_err(|_| "diagnostic fixture model preparation failed")?;
    let reviewer = AcceptanceReviewer::new_diagnostic(
        &WorkforcePack,
        &document,
        1,
        &prepared,
        None,
        clock.as_ref(),
        &mut preparation,
        &control,
    )
    .map_err(|_| "diagnostic fixture reviewer preparation failed")?;
    let decision = reviewer.review_diagnostic(
        &candidate,
        id(700).parse()?,
        scope.relaxable_required_rules(),
        None,
        &control,
        &mut preparation,
    );
    reviewer
        .release_diagnostic(&mut preparation)
        .map_err(|_| "diagnostic fixture reviewer release failed")?;
    prepared
        .release(&mut preparation)
        .map_err(|_| "diagnostic fixture model release failed")?;
    Ok(decision)
}

#[test]
fn diagnostic_complete_original_witness_survives_optional_cancel_or_expiry() -> TestResult {
    for deadline in [false, true] {
        // The last phase-clock sample occurs after complete source/report/condition checks.
        let decision = diagnostic_stop_decision(deadline, 4)?;
        let eutheto_verify::DiagnosticDecision::OriginalWitness { optional_stop, .. } = decision
        else {
            return Err(format!("complete original witness was erased: {decision:?}").into());
        };
        assert_eq!(
            optional_stop,
            Some(if deadline {
                eutheto_types::OperationInterruption::DeadlineExceeded
            } else {
                eutheto_types::OperationInterruption::Cancelled
            })
        );
    }
    Ok(())
}

#[test]
fn diagnostic_incomplete_source_review_cannot_become_an_original_witness() -> TestResult {
    for deadline in [false, true] {
        let decision = diagnostic_stop_decision(deadline, 3)?;
        assert!(matches!(
            decision,
            eutheto_verify::DiagnosticDecision::Interrupted { .. }
        ));
    }
    Ok(())
}
