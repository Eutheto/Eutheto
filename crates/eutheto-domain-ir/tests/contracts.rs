use eutheto_domain_ir::*;
use eutheto_types::{
    BackendId, BackendSelection, DurationMillis, ExplanationMode, IanaTimeZone, PackId,
    PreservationPolicy, REVISION_MAX_V1, ReproducibilityMode, RequestId, ResourceLimits,
    ResourceRef, Rfc3339Timestamp, RuleId, ScenarioId, ScenarioSnapshotId, SolutionId, SolveMode,
    SolveOptions, SolveRunId, SolveStatus, ValidationIssue, ValidationSeverity, WorkerThreadPolicy,
    extract_result_dependency, extract_result_id,
};
use proptest::prelude::*;
use serde_json::json;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::error::Error;

fn score(value: i64) -> Result<ScoreVector, Box<dyn Error>> {
    Ok(ScoreVector {
        feasibility: 0,
        levels: vec![ScoreLevelValue {
            level_id: ScoreLevelId::new("score.preference")?,
            value,
            direction: OptimizationDirection::Minimize,
            category_breakdown: BTreeMap::new(),
        }],
    })
}

fn solution(revision: u64) -> Result<NormalizedSolution, Box<dyn Error>> {
    Ok(NormalizedSolution {
        schema_version: NORMALIZED_SOLUTION_SCHEMA_VERSION,
        pack_id: PackId::new("official.synthetic")?,
        scenario_id: "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse::<ScenarioId>()?,
        scenario_revision: revision,
        projection_version: 1,
        solution_id: "01890a5d-ac96-7b64-9f74-bbfcf30f9f81".parse::<SolutionId>()?,
        assignments: Vec::new(),
    })
}

fn rule(suffix: u8) -> Result<RuleId, Box<dyn Error>> {
    Ok(format!("01890a5d-ac96-7b64-9f74-bbfcf30f9f{suffix:02x}").parse::<RuleId>()?)
}

fn entity(suffix: &str) -> Result<DomainEntityRef, Box<dyn Error>> {
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new("school.person")?,
        id: DomainEntityId::new(format!("school.{suffix}"))?,
    })
}

fn binding(rule_id: RuleId, meaning: &[u8]) -> RequiredRuleBinding {
    RequiredRuleBinding {
        rule_id,
        semantic_hash: blake3_hex(meaning),
    }
}

fn evaluation(rule_id: RuleId, satisfied: bool) -> Result<RuleEvaluation, Box<dyn Error>> {
    Ok(RuleEvaluation {
        rule_id,
        satisfied,
        affected_entities: vec![entity("person-b")?, entity("person-a")?],
        message_key: "official.synthetic.required".to_owned(),
        expected: BTreeMap::from([(
            VerificationFactId::new("fact.required")?,
            VerificationValue::Boolean(true),
        )]),
        observed: BTreeMap::from([(
            VerificationFactId::new("fact.required")?,
            VerificationValue::Boolean(satisfied),
        )]),
        evidence: vec![
            DomainEvidenceId::new("evidence.second")?,
            DomainEvidenceId::new("evidence.first")?,
        ],
    })
}

fn warning(suffix: &str) -> Result<VerificationWarning, Box<dyn Error>> {
    Ok(VerificationWarning {
        id: VerificationWarningId::new(format!("warning.{suffix}"))?,
        message_key: "official.synthetic.warning".to_owned(),
        affected_entities: vec![entity("person-b")?, entity("person-a")?],
        facts: BTreeMap::from([(
            VerificationFactId::new("fact.detail")?,
            VerificationValue::Text("bounded detail".to_owned()),
        )]),
    })
}

fn scope(revision: u64) -> Result<VerificationScope, Box<dyn Error>> {
    let scenario_id = solution(revision)?.scenario_id;
    Ok(VerificationScope::new(
        scenario_id,
        revision,
        vec![binding(rule(2)?, b"second"), binding(rule(1)?, b"first")],
    )?)
}

fn context(
    solution: &NormalizedSolution,
    scope: &VerificationScope,
) -> Result<VerificationContextV1, Box<dyn Error>> {
    Ok(VerificationContextV1::new(
        solution.scenario_id,
        solution.scenario_revision,
        blake3_hex(b"document"),
        blake3_hex(b"planning-model"),
        solution.canonical_hash()?,
        scope.checksum.clone(),
    )?)
}

fn report(
    solution: &NormalizedSolution,
    feasibility: i64,
    satisfied: bool,
) -> Result<VerificationReport, Box<dyn Error>> {
    let scope = scope(solution.scenario_revision)?;
    let context = context(solution, &scope)?;
    let mut authoritative_score = score(4)?;
    authoritative_score.feasibility = feasibility;
    Ok(VerificationReport::new(
        &context,
        vec![
            evaluation(rule(2)?, satisfied)?,
            evaluation(rule(1)?, true)?,
        ],
        authoritative_score,
        vec![warning("second")?, warning("first")?],
        BTreeMap::from([
            (
                MetricId::new("metric.ratio")?,
                MetricValue::Ratio {
                    numerator: 1,
                    denominator: 2,
                },
            ),
            (MetricId::new("metric.total")?, MetricValue::Integer(4)),
        ]),
    )?)
}

fn solve_options() -> Result<SolveOptions, Box<dyn Error>> {
    Ok(SolveOptions {
        backend: BackendSelection::Auto,
        mode: SolveMode::Balanced,
        time_limit_milliseconds: DurationMillis::new(30_000)?,
        memory_limit_bytes: Some(64 * 1024 * 1024),
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 7,
        solution_limit: Some(1),
        stop_after_first_feasible: true,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::Standard,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: 1_000,
            max_rules: 1_000,
            max_variables: 10_000,
            max_constraints: 10_000,
        },
    })
}

fn run_id(suffix: u8) -> Result<SolveRunId, Box<dyn Error>> {
    Ok(format!("01890a5d-ac96-7b64-9f74-bbfcf30f9f{suffix:02x}").parse()?)
}

fn run_input_at(
    accepted: &AcceptedResult,
    run_id: SolveRunId,
    snapshot_document_hash: String,
    model_hash: String,
    snapshot_created_at: Rfc3339Timestamp,
) -> Result<RunInput, Box<dyn Error>> {
    Ok(RunInput::new(
        run_id,
        "01890a5d-ac96-7b64-9f74-bbfcf30f9f83".parse::<RequestId>()?,
        accepted.solution.scenario_id,
        accepted.solution.scenario_revision,
        "01890a5d-ac96-7b64-9f74-bbfcf30f9f84".parse::<ScenarioSnapshotId>()?,
        snapshot_document_hash,
        snapshot_created_at,
        accepted.solution.pack_id.clone(),
        1,
        1,
        "1.2.3".to_owned(),
        "0.1.0".to_owned(),
        BackendId::new("ortools.cp-sat")?,
        "9.15.6755".to_owned(),
        "0.1.0".to_owned(),
        "0.1.0".to_owned(),
        "9.15.6755".to_owned(),
        1,
        0,
        model_hash,
        blake3_hex(b"objective-policy"),
        solve_options()?,
        "America/Chicago".parse::<IanaTimeZone>()?,
        None,
        Some(RunObjectiveContext::authored(
            accepted.solution.pack_id.clone(),
        )?),
    )?)
}

fn run_input_for(
    accepted: &AcceptedResult,
    run_id: SolveRunId,
    snapshot_document_hash: String,
    model_hash: String,
) -> Result<RunInput, Box<dyn Error>> {
    run_input_at(
        accepted,
        run_id,
        snapshot_document_hash,
        model_hash,
        "2026-09-03T12:00:00Z".parse()?,
    )
}

fn accepted_contract() -> Result<AcceptedResult, Box<dyn Error>> {
    let solution = solution(7)?;
    Ok(AcceptedResult::new(
        solution.clone(),
        report(&solution, 0, true)?,
    )?)
}

fn manifest_for(
    run_id: SolveRunId,
    run_input_checksum: String,
    outcome: RunTerminalOutcomeV1,
    first_verified_feasible_milliseconds: Option<DurationMillis>,
    verification_warnings: Vec<VerificationWarning>,
) -> Result<RunManifest, Box<dyn Error>> {
    Ok(RunManifest::new(
        run_id,
        run_input_checksum,
        outcome,
        "2026-09-03T12:00:01Z".parse::<Rfc3339Timestamp>()?,
        "2026-09-03T12:00:02Z".parse::<Rfc3339Timestamp>()?,
        Some(DurationMillis::new(1_000)?),
        Some(DurationMillis::new(100)?),
        first_verified_feasible_milliseconds,
        RunPhaseTimingsV1 {
            compile_milliseconds: Some(DurationMillis::new(10)?),
            backend_milliseconds: Some(DurationMillis::new(400)?),
            projection_milliseconds: Some(DurationMillis::new(20)?),
            structural_validation_milliseconds: Some(DurationMillis::new(15)?),
            score_recomputation_milliseconds: Some(DurationMillis::new(15)?),
            required_rule_verification_milliseconds: Some(DurationMillis::new(30)?),
            evidence_persistence_milliseconds: Some(DurationMillis::new(10)?),
            optional_explanation_milliseconds: None,
        },
        verification_warnings,
    )?)
}

fn accepted_outcome(accepted: &AcceptedResult) -> RunTerminalOutcomeV1 {
    RunTerminalOutcomeV1::Accepted {
        status: SolveStatus::Feasible,
        solution_id: accepted.solution.solution_id,
        accepted_result_checksum: accepted.checksum.clone(),
        verification_checksum: accepted.verification.checksum.clone(),
    }
}

fn accepted_manifest(
    input: &RunInput,
    accepted: &AcceptedResult,
) -> Result<RunManifest, Box<dyn Error>> {
    manifest_for(
        input.run_id,
        input.checksum.clone(),
        accepted_outcome(accepted),
        Some(DurationMillis::new(500)?),
        accepted.verification.warnings.clone(),
    )
}

fn portable_result() -> Result<PortableAcceptedResult, Box<dyn Error>> {
    let accepted = accepted_contract()?;
    let input = run_input_for(
        &accepted,
        run_id(0x82)?,
        accepted.verification.document_hash.clone(),
        accepted.verification.planning_model_hash.clone(),
    )?;
    let manifest = accepted_manifest(&input, &accepted)?;
    Ok(PortableAcceptedResult::new(
        input,
        manifest,
        accepted,
        BTreeMap::from([(
            DomainEvidenceId::new("evidence.first")?,
            VerificationValue::Integer(1),
        )]),
    )?)
}

#[test]
fn ids_accept_required_grammar_and_boundaries() -> Result<(), Box<dyn Error>> {
    let id = DomainAssignmentId::new("school_2.section-a")?;
    assert_eq!(id.as_str(), "school_2.section-a");
    assert!(DomainAssignmentId::new("School.section").is_err());
    assert!(DomainAssignmentId::new("school").is_err());
    assert!(DomainAssignmentId::new(format!("aa.{}", "b".repeat(158))).is_err());
    assert!(VerificationFactId::new("fact.required").is_ok());
    assert!(MetricId::new("metric.total").is_ok());
    Ok(())
}

#[test]
fn interval_and_absent_are_distinct_and_checked() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        AssignedInterval::new(5, 0, 5)?,
        AssignedInterval {
            start: 5,
            duration: 0,
            end: 5
        }
    );
    assert!(AssignedInterval::new(5, -1, 4).is_err());
    assert!(AssignedInterval::new(i64::MAX, 1, i64::MIN).is_err());
    assert_ne!(AssignmentValue::Absent, AssignmentValue::Boolean(false));
    Ok(())
}

#[test]
fn normalized_solution_v1_is_unchanged_and_hashes_canonically() -> Result<(), Box<dyn Error>> {
    let mut value = solution(3)?;
    let assignment = DomainAssignment {
        id: DomainAssignmentId::new("school.second")?,
        entity: entity("section-a")?,
        value: AssignmentValue::Integer(1),
        evidence: vec![
            DomainEvidenceId::new("evidence.z")?,
            DomainEvidenceId::new("evidence.a")?,
        ],
    };
    value.assignments = vec![assignment];
    assert_eq!(
        value.validate(),
        Err(DomainContractError::NonCanonicalEvidence)
    );
    value.canonicalize()?;
    let first_hash = value.canonical_hash()?;
    assert_eq!(first_hash.len(), 64);
    assert!(
        first_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    let json = serde_json::to_value(&value)?;
    assert!(json.get("checksum").is_none());
    assert_eq!(value.schema_version, 1);
    let duplicate = value.assignments[0].clone();
    value.assignments.push(duplicate);
    assert!(matches!(
        value.canonicalize(),
        Err(DomainContractError::DuplicateAssignment(_))
    ));
    Ok(())
}

#[test]
fn scope_sorts_bindings_rejects_duplicates_and_binds_revision() -> Result<(), Box<dyn Error>> {
    let first = scope(7)?;
    assert_eq!(first.required_rules[0].rule_id, rule(1)?);
    assert_eq!(first.required_rules[1].rule_id, rule(2)?);
    first.validate()?;

    let reordered = VerificationScope::new(
        first.scenario_id,
        7,
        vec![binding(rule(1)?, b"first"), binding(rule(2)?, b"second")],
    )?;
    assert_eq!(first.checksum, reordered.checksum);
    assert_ne!(first.checksum, scope(8)?.checksum);
    assert_eq!(
        VerificationScope::new(
            first.scenario_id,
            7,
            vec![binding(rule(1)?, b"first"), binding(rule(1)?, b"duplicate")]
        ),
        Err(DomainContractError::DuplicateRequiredRule(rule(1)?))
    );
    assert_eq!(
        VerificationScope::new(
            first.scenario_id,
            7,
            vec![RequiredRuleBinding {
                rule_id: rule(1)?,
                semantic_hash: "A".repeat(64)
            }]
        ),
        Err(DomainContractError::InvalidBlake3)
    );
    Ok(())
}

#[test]
fn report_constructor_canonicalizes_and_hashes_typed_content() -> Result<(), Box<dyn Error>> {
    let solution = solution(7)?;
    let report = report(&solution, 0, true)?;
    assert!(report.accepted);
    assert_eq!(report.schema_version, 2);
    assert_eq!(report.required_rule_results[0].rule_id, rule(1)?);
    assert_eq!(
        report.warnings[0].id,
        VerificationWarningId::new("warning.first")?
    );
    assert_eq!(
        report.required_rule_results[0].affected_entities[0],
        entity("person-a")?
    );
    assert_eq!(
        report.required_rule_results[0].evidence[0],
        DomainEvidenceId::new("evidence.first")?
    );
    report.validate()?;

    let encoded = serde_json::to_value(&report)?;
    assert!(encoded["metrics"]["metric.total"].get("type").is_some());
    assert!(serde_json::from_value::<VerificationValue>(json!({"untyped": true})).is_err());
    Ok(())
}

#[test]
fn failed_rule_remains_visible_and_controls_report_acceptance() -> Result<(), Box<dyn Error>> {
    let solution = solution(7)?;
    let report = report(&solution, 0, false)?;
    assert!(!report.accepted);
    assert_eq!(report.required_rule_results.len(), 2);
    assert!(
        report
            .required_rule_results
            .iter()
            .any(|result| !result.satisfied)
    );
    report.validate()?;
    assert_eq!(
        AcceptedResult::new(solution, report),
        Err(DomainContractError::NotVerifiedFeasible)
    );
    Ok(())
}

#[test]
fn zero_feasibility_is_enforced_only_at_acceptance_boundary() -> Result<(), Box<dyn Error>> {
    let solution = solution(7)?;
    let inconsistent = report(&solution, 1, true)?;
    assert!(inconsistent.accepted);
    inconsistent.validate()?;
    assert_eq!(
        AcceptedResult::new(solution.clone(), inconsistent),
        Err(DomainContractError::NotVerifiedFeasible)
    );

    let accepted_report = report(&solution, 0, true)?;
    let accepted = AcceptedResult::new(solution, accepted_report)?;
    accepted.validate()?;
    assert_eq!(accepted.schema_version, 2);
    assert_eq!(accepted.checksum.len(), 64);
    Ok(())
}

#[test]
fn accepted_result_rejects_every_solution_report_binding_mismatch() -> Result<(), Box<dyn Error>> {
    let original = solution(7)?;
    let report = report(&original, 0, true)?;
    assert!(AcceptedResult::new(original.clone(), report.clone()).is_ok());

    let mut wrong_revision = original.clone();
    wrong_revision.scenario_revision = 8;
    assert_eq!(
        AcceptedResult::new(wrong_revision, report.clone()),
        Err(DomainContractError::VerificationBindingMismatch)
    );

    let mut wrong_solution = original;
    wrong_solution.solution_id = "01890a5d-ac96-7b64-9f74-bbfcf30f9f99".parse()?;
    assert_eq!(
        AcceptedResult::new(wrong_solution, report),
        Err(DomainContractError::VerificationBindingMismatch)
    );
    Ok(())
}

#[test]
fn current_result_contracts_reject_nonportable_revisions() -> Result<(), Box<dyn Error>> {
    let invalid_revision = REVISION_MAX_V1 + 1;
    let invalid_solution = solution(invalid_revision)?;
    assert_eq!(
        invalid_solution.validate(),
        Err(DomainContractError::LimitExceeded("scenario revision"))
    );

    let scenario_id = solution(7)?.scenario_id;
    assert_eq!(
        VerificationScope::new(scenario_id, invalid_revision, Vec::new()),
        Err(DomainContractError::LimitExceeded("scenario revision"))
    );

    let valid_scope = scope(7)?;
    assert_eq!(
        VerificationContextV1::new(
            scenario_id,
            invalid_revision,
            blake3_hex(b"document"),
            blake3_hex(b"planning-model"),
            blake3_hex(b"solution"),
            valid_scope.checksum,
        ),
        Err(DomainContractError::LimitExceeded("scenario revision"))
    );

    let valid_solution = solution(7)?;
    let mut invalid_report = report(&valid_solution, 0, true)?;
    invalid_report.evaluated_revision = invalid_revision;
    assert_eq!(
        invalid_report.validate(),
        Err(DomainContractError::LimitExceeded("scenario revision"))
    );
    Ok(())
}

#[test]
fn checksum_mutation_is_rejected_at_every_checksummed_boundary() -> Result<(), Box<dyn Error>> {
    let mut scope = scope(7)?;
    scope.required_rules[0].semantic_hash = blake3_hex(b"mutated");
    assert_eq!(scope.validate(), Err(DomainContractError::ChecksumMismatch));

    let solution = solution(7)?;
    let mut mutated_report = report(&solution, 0, true)?;
    mutated_report.planning_model_hash = blake3_hex(b"different-model");
    assert_eq!(
        mutated_report.validate(),
        Err(DomainContractError::ChecksumMismatch)
    );

    let mut accepted = AcceptedResult::new(solution.clone(), report(&solution, 0, true)?)?;
    accepted.checksum = blake3_hex(b"forged-result");
    assert_eq!(
        accepted.validate(),
        Err(DomainContractError::ChecksumMismatch)
    );
    Ok(())
}

#[test]
fn deserialized_noncanonical_collections_are_rejected_before_checksum() -> Result<(), Box<dyn Error>>
{
    let solution = solution(7)?;
    let mut noncanonical_rules = report(&solution, 0, true)?;
    noncanonical_rules.required_rule_results.reverse();
    assert_eq!(
        noncanonical_rules.validate(),
        Err(DomainContractError::NonCanonicalRuleEvaluations)
    );

    let mut noncanonical_warnings = report(&solution, 0, true)?;
    noncanonical_warnings.warnings.reverse();
    assert_eq!(
        noncanonical_warnings.validate(),
        Err(DomainContractError::NonCanonicalVerificationWarnings)
    );

    let scope = scope(7)?;
    let context = context(&solution, &scope)?;
    let duplicate = evaluation(rule(1)?, true)?;
    assert_eq!(
        VerificationReport::new(
            &context,
            vec![duplicate.clone(), duplicate],
            score(0)?,
            Vec::new(),
            BTreeMap::new()
        ),
        Err(DomainContractError::DuplicateRuleEvaluation(rule(1)?))
    );
    Ok(())
}

#[test]
fn typed_values_and_metrics_enforce_bounds() -> Result<(), Box<dyn Error>> {
    let solution = solution(7)?;
    let scope = scope(7)?;
    let context = context(&solution, &scope)?;
    let mut invalid_text = evaluation(rule(1)?, true)?;
    invalid_text.expected.insert(
        VerificationFactId::new("fact.text")?,
        VerificationValue::Text(String::new()),
    );
    assert_eq!(
        VerificationReport::new(
            &context,
            vec![invalid_text],
            score(0)?,
            Vec::new(),
            BTreeMap::new()
        ),
        Err(DomainContractError::InvalidVerificationText)
    );
    assert_eq!(
        VerificationReport::new(
            &context,
            Vec::new(),
            score(0)?,
            Vec::new(),
            BTreeMap::from([(
                MetricId::new("metric.ratio")?,
                MetricValue::Ratio {
                    numerator: 1,
                    denominator: 0
                }
            )])
        ),
        Err(DomainContractError::InvalidMetricRatio)
    );
    Ok(())
}

#[test]
fn strict_json_rejects_unknown_fields_and_unknown_versions() -> Result<(), Box<dyn Error>> {
    let solution = solution(1)?;
    let mut json = serde_json::to_value(&solution)?;
    json.as_object_mut()
        .ok_or("solution JSON is not an object")?
        .insert("futureField".to_owned(), serde_json::Value::Bool(true));
    assert!(matches!(
        NormalizedSolution::from_json(&serde_json::to_vec(&json)?),
        Err(DomainContractError::MalformedJson(_))
    ));

    let mut report = report(&solution, 0, true)?;
    report.schema_version = 3;
    assert_eq!(
        report.validate(),
        Err(DomainContractError::UnsupportedVersion(3))
    );

    let mut context = context(&solution, &scope(1)?)?;
    context.document_hash = "ABC".to_owned();
    assert_eq!(context.validate(), Err(DomainContractError::InvalidBlake3));
    Ok(())
}

#[test]
fn legacy_v1_decodes_but_cannot_validate_as_current() -> Result<(), Box<dyn Error>> {
    let legacy_report = LegacyVerificationReportV1 {
        schema_version: 1,
        scenario_revision: 7,
        feasible: true,
        issues: vec![LegacyVerificationIssueV1 {
            id: VerificationIssueId::new("legacy.issue")?,
            severity: LegacyVerificationSeverityV1::Warning,
            message_key: "legacy.warning".to_owned(),
            entities: Vec::new(),
            evidence: Vec::new(),
        }],
        score: Some(score(0)?),
    };
    let legacy = LegacyAcceptedResultV1 {
        schema_version: 1,
        solution: solution(7)?,
        verification: legacy_report,
    };
    let bytes = serde_json::to_vec(&legacy)?;
    let decoded = LegacyAcceptedResultV1::from_json(&bytes)?;
    assert_eq!(decoded, legacy);
    assert!(matches!(
        AcceptedResult::from_json(&bytes),
        Err(DomainContractError::MalformedJson(_))
    ));
    let legacy_report_bytes = serde_json::to_vec(&legacy.verification)?;
    assert!(matches!(
        VerificationReport::from_json(&legacy_report_bytes),
        Err(DomainContractError::MalformedJson(_))
    ));
    let mut wide_legacy = legacy.clone();
    let wide_score = wide_legacy
        .verification
        .score
        .as_mut()
        .ok_or_else(|| std::io::Error::other("legacy fixture score missing"))?;
    wide_score.levels.clear();
    for index in 0..=MAX_SCORE_LEVELS {
        wide_score.levels.push(ScoreLevelValue {
            level_id: ScoreLevelId::new(format!("legacy.score.level.{index}"))?,
            value: i64::try_from(index)?,
            direction: OptimizationDirection::Minimize,
            category_breakdown: BTreeMap::new(),
        });
    }
    assert!(wide_score.validate_shape().is_ok());
    assert_eq!(
        wide_score.validate_current_shape(),
        Err(DomainContractError::LimitExceeded("score levels"))
    );
    assert_eq!(
        LegacyAcceptedResultV1::from_json(&serde_json::to_vec(&wide_legacy)?)?,
        wide_legacy
    );
    let mut category_legacy = legacy.clone();
    let category_score = category_legacy
        .verification
        .score
        .as_mut()
        .ok_or_else(|| std::io::Error::other("legacy fixture score missing"))?;
    for index in 0..=MAX_SCORE_CATEGORIES_PER_LEVEL {
        category_score.levels[0].category_breakdown.insert(
            ScoreCategoryId::new(format!("legacy.score.category.{index}"))?,
            i64::try_from(index)?,
        );
    }
    assert!(category_score.validate_shape().is_ok());
    assert_eq!(
        category_score.validate_current_shape(),
        Err(DomainContractError::LimitExceeded("score categories"))
    );
    assert_eq!(
        LegacyAcceptedResultV1::from_json(&serde_json::to_vec(&category_legacy)?)?,
        category_legacy
    );
    let mut newer = legacy;
    newer.schema_version = 9;
    assert_eq!(
        LegacyAcceptedResultV1::from_json(&serde_json::to_vec(&newer)?),
        Err(DomainContractError::UnsupportedVersion(9))
    );
    Ok(())
}

#[test]
fn run_and_portable_contracts_round_trip_with_outer_extractors() -> Result<(), Box<dyn Error>> {
    let portable = portable_result()?;

    let bytes = serde_json::to_vec(&portable)?;
    assert_eq!(PortableAcceptedResult::from_json(&bytes)?, portable);
    let json = serde_json::to_value(&portable)?;
    assert_eq!(json["runInput"]["backendId"], json!("ortools.cp-sat"));
    assert!(json["runInput"].get("snapshotId").is_some());
    assert!(json["runInput"].get("scenarioSnapshotId").is_none());
    assert_eq!(extract_result_id(&json)?, portable.result_id.as_uuid());
    let dependency = extract_result_dependency(&json)?;
    assert_eq!(dependency.scenario_id, portable.scenario_id);
    assert_eq!(
        dependency.scenario_revision.value(),
        portable.scenario_revision
    );
    Ok(())
}
#[test]
fn verified_checkpoint_binds_first_feasible_result_without_claiming_terminal_proof()
-> Result<(), Box<dyn Error>> {
    let portable = portable_result()?;
    let checkpoint = VerifiedCheckpoint::new(
        portable.run_input,
        DurationMillis::new(100)?,
        DurationMillis::new(500)?,
        portable.run_manifest.started_at,
        portable.accepted_result,
        portable.evidence,
    )?;
    assert_eq!(
        VerifiedCheckpoint::from_json(&serde_json::to_vec(&checkpoint)?)?,
        checkpoint
    );

    let mut invalid = checkpoint.clone();
    invalid.schema_version += 1;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::UnsupportedVersion(
            VERIFIED_CHECKPOINT_SCHEMA_VERSION + 1
        ))
    );
    invalid = checkpoint.clone();
    invalid.verified_after_milliseconds = DurationMillis::new(99)?;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::InvalidRunTiming)
    );
    invalid = checkpoint.clone();
    invalid.evidence.clear();
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::ChecksumMismatch)
    );

    let mut unknown = serde_json::to_value(&checkpoint)?;
    unknown
        .as_object_mut()
        .ok_or("checkpoint must serialize as an object")?
        .insert("backendProof".to_owned(), json!(true));
    assert!(matches!(
        VerifiedCheckpoint::from_json(&serde_json::to_vec(&unknown)?),
        Err(DomainContractError::MalformedJson(_))
    ));
    Ok(())
}

#[test]
// One mutation table keeps every immutable input field under the same strict parser contract.
#[allow(clippy::too_many_lines)]
fn run_input_rejects_checksum_version_hash_and_numeric_bounds() -> Result<(), Box<dyn Error>> {
    let accepted = accepted_contract()?;
    let input = run_input_for(
        &accepted,
        run_id(0x82)?,
        accepted.verification.document_hash.clone(),
        accepted.verification.planning_model_hash.clone(),
    )?;
    assert_eq!(RunInput::from_json(&serde_json::to_vec(&input)?)?, input);
    assert_eq!(
        input.request_semantics().canonical_hash()?,
        input.request_hash
    );
    let semantics = input.request_semantics();
    assert_eq!(
        semantics.schema_version,
        RUN_REQUEST_SEMANTICS_SCHEMA_VERSION
    );

    let mut invalid = input.clone();
    invalid.protocol_minor += 1;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::RequestHashMismatch)
    );
    invalid = input.clone();
    invalid.solve_options.backend = BackendSelection::Specific(BackendId::new("other.backend")?);
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::RunInputBackendMismatch)
    );
    invalid = input.clone();
    invalid.solve_options.memory_limit_bytes = Some(0);
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::InvalidSolveOptions)
    );

    invalid = input.clone();
    invalid.schema_version = RUN_INPUT_SCHEMA_VERSION + 1;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::UnsupportedVersion(
            RUN_INPUT_SCHEMA_VERSION + 1
        ))
    );
    invalid = input.clone();
    invalid.scenario_revision = REVISION_MAX_V1 + 1;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::LimitExceeded("scenario revision"))
    );
    invalid = input.clone();
    invalid.request_hash = "A".repeat(64);
    assert_eq!(invalid.validate(), Err(DomainContractError::InvalidBlake3));
    invalid = input.clone();
    invalid.temporary_condition_hash = Some("0".repeat(63));
    assert_eq!(invalid.validate(), Err(DomainContractError::InvalidBlake3));
    invalid = input.clone();
    invalid.compiler_version.clear();
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::InvalidRunVersion("compiler version"))
    );
    invalid = input.clone();
    invalid.solver_version = "x".repeat(MAX_RUN_VERSION_BYTES + 1);
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::InvalidRunVersion("solver version"))
    );
    invalid = input.clone();
    invalid.pack_schema_version = 0;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::ZeroVersion("pack schema version"))
    );
    invalid = input.clone();
    invalid.planning_ir_schema_version = 0;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::ZeroVersion(
            "planning IR schema version"
        ))
    );
    invalid = input.clone();
    invalid.protocol_major = 0;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::ZeroVersion("protocol major"))
    );
    invalid = input.clone();
    invalid.checksum = blake3_hex(b"mutated-checksum");
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::ChecksumMismatch)
    );

    let mut unknown = serde_json::to_value(&input)?;
    unknown
        .as_object_mut()
        .ok_or("run input must serialize as object")?
        .insert("unknown".to_owned(), json!(true));
    assert!(matches!(
        RunInput::from_json(&serde_json::to_vec(&unknown)?),
        Err(DomainContractError::MalformedJson(_))
    ));
    assert!(matches!(
        RunInput::from_json(br#"{"schemaVersion":"one"}"#),
        Err(DomainContractError::MalformedJson(_))
    ));
    assert!(matches!(
        RunInput::from_json(br#"{"schemaVersion":1,"schemaVersion":1}"#),
        Err(DomainContractError::PortableJsonViolation(_))
    ));
    Ok(())
}

#[test]
// Every terminal status and timing partition stays together so the state matrix is auditable.
#[allow(clippy::too_many_lines)]
fn terminal_outcome_status_and_timing_partitions_are_strict() -> Result<(), Box<dyn Error>> {
    let accepted = accepted_contract()?;
    for status in [
        SolveStatus::Optimal,
        SolveStatus::Feasible,
        SolveStatus::Infeasible,
        SolveStatus::Unbounded,
        SolveStatus::NoSolutionWithinLimit,
        SolveStatus::Cancelled,
        SolveStatus::InvalidModel,
        SolveStatus::BackendUnavailable,
        SolveStatus::BackendFailed,
    ] {
        let outcome = RunTerminalOutcomeV1::Accepted {
            status,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        };
        let wire = serde_json::to_value(&outcome)?;
        assert_eq!(
            wire.pointer("/solutionId"),
            Some(&json!(accepted.solution.solution_id))
        );
        assert_eq!(
            wire.pointer("/acceptedResultChecksum"),
            Some(&json!(accepted.checksum))
        );
        assert_eq!(
            wire.pointer("/verificationChecksum"),
            Some(&json!(accepted.verification.checksum))
        );
        assert!(wire.pointer("/solution_id").is_none());
        assert!(wire.pointer("/accepted_result_checksum").is_none());
        assert!(wire.pointer("/verification_checksum").is_none());
        assert_eq!(
            outcome.validate().is_ok(),
            matches!(status, SolveStatus::Optimal | SolveStatus::Feasible)
        );
        let no_result = RunTerminalOutcomeV1::NoResult { status };
        assert_eq!(
            no_result.validate().is_ok(),
            !matches!(status, SolveStatus::Optimal | SolveStatus::Feasible)
        );
    }
    let verification_alarm = RunTerminalOutcomeV1::VerificationAlarm {
        diagnostic_code: "verification.score_mismatch".to_owned(),
    };
    assert!(verification_alarm.validate().is_ok());
    let wire = serde_json::to_value(&verification_alarm)?;
    assert_eq!(
        wire.pointer("/diagnosticCode"),
        Some(&json!("verification.score_mismatch"))
    );
    assert!(wire.pointer("/diagnostic_code").is_none());
    for diagnostic_code in [
        "Verification.bad".to_owned(),
        "verification".to_owned(),
        format!("verification.{}", "x".repeat(MAX_RUN_DIAGNOSTIC_CODE_BYTES)),
    ] {
        assert_eq!(
            RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code }.validate(),
            Err(DomainContractError::InvalidRunDiagnosticCode)
        );
    }

    let input = run_input_for(
        &accepted,
        run_id(0x82)?,
        accepted.verification.document_hash.clone(),
        accepted.verification.planning_model_hash.clone(),
    )?;
    assert!(matches!(
        manifest_for(
            input.run_id,
            input.checksum.clone(),
            accepted_outcome(&accepted),
            None,
            Vec::new(),
        ),
        Err(error) if error.downcast_ref::<DomainContractError>()
            == Some(&DomainContractError::InvalidRunTiming)
    ));
    let no_result = RunTerminalOutcomeV1::NoResult {
        status: SolveStatus::NoSolutionWithinLimit,
    };
    assert!(
        manifest_for(
            input.run_id,
            input.checksum.clone(),
            no_result.clone(),
            None,
            Vec::new(),
        )
        .is_ok()
    );
    assert!(matches!(
        manifest_for(
            input.run_id,
            input.checksum.clone(),
            no_result,
            Some(DurationMillis::ZERO),
            Vec::new(),
        ),
        Err(error) if error.downcast_ref::<DomainContractError>()
            == Some(&DomainContractError::InvalidRunTiming)
    ));
    let interrupted = RunManifest::new(
        input.run_id,
        input.checksum.clone(),
        RunTerminalOutcomeV1::Interrupted,
        "2026-09-03T12:00:01Z".parse()?,
        "2026-09-03T12:00:02Z".parse()?,
        None,
        None,
        None,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    assert!(interrupted.validate().is_ok());
    for invalid in [
        {
            let mut value = interrupted.clone();
            value.elapsed_milliseconds = Some(DurationMillis::ZERO);
            value
        },
        {
            let mut value = interrupted.clone();
            value.first_incumbent_milliseconds = Some(DurationMillis::ZERO);
            value
        },
        {
            let mut value = interrupted.clone();
            value.first_verified_feasible_milliseconds = Some(DurationMillis::ZERO);
            value
        },
        {
            let mut value = interrupted.clone();
            value.phase_timings.compile_milliseconds = Some(DurationMillis::ZERO);
            value
        },
        {
            let mut value = interrupted.clone();
            value.verification_warnings.push(warning("interrupted")?);
            value
        },
    ] {
        assert_eq!(
            invalid.validate(),
            Err(DomainContractError::InvalidRunTiming)
        );
    }
    assert_eq!(
        RunManifest::new(
            input.run_id,
            input.checksum.clone(),
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::Cancelled,
            },
            "2026-09-03T12:00:01Z".parse()?,
            "2026-09-03T12:00:02Z".parse()?,
            None,
            None,
            None,
            RunPhaseTimingsV1::default(),
            Vec::new(),
        ),
        Err(DomainContractError::InvalidRunTiming)
    );
    assert!(matches!(
        manifest_for(
            input.run_id,
            input.checksum,
            RunTerminalOutcomeV1::Interrupted,
            None,
            vec![warning("bounded")?; MAX_VERIFICATION_WARNINGS + 1],
        ),
        Err(error) if error.downcast_ref::<DomainContractError>()
            == Some(&DomainContractError::LimitExceeded("verification warnings"))
    ));
    Ok(())
}

#[test]
// One parser fixture covers checksum, unknown-field, and numeric boundary rejection together.
#[allow(clippy::too_many_lines)]
fn run_manifest_rejects_checksum_unknown_fields_and_duration_bounds() -> Result<(), Box<dyn Error>>
{
    let portable = portable_result()?;
    let manifest = portable.run_manifest;
    assert_eq!(
        RunManifest::from_json(&serde_json::to_vec(&manifest)?)?,
        manifest
    );

    let mut invalid = manifest.clone();
    invalid.checksum = blake3_hex(b"other-manifest");
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::ChecksumMismatch)
    );
    invalid = manifest.clone();
    invalid.finished_at = "2026-09-03T11:59:59Z".parse()?;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::InvalidRunTiming)
    );
    invalid = manifest.clone();
    invalid.first_incumbent_milliseconds = Some(DurationMillis::new(1_001)?);
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::InvalidRunTiming)
    );
    invalid = manifest.clone();
    invalid.first_verified_feasible_milliseconds = Some(DurationMillis::new(1_001)?);
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::InvalidRunTiming)
    );
    invalid = manifest.clone();
    invalid.first_incumbent_milliseconds = Some(DurationMillis::new(600)?);
    invalid.first_verified_feasible_milliseconds = Some(DurationMillis::new(500)?);
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::InvalidRunTiming)
    );
    invalid = manifest.clone();
    invalid.phase_timings = RunPhaseTimingsV1 {
        compile_milliseconds: Some(DurationMillis::new(600)?),
        backend_milliseconds: Some(DurationMillis::new(500)?),
        ..RunPhaseTimingsV1::default()
    };
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::InvalidRunTiming)
    );
    let over_elapsed = Some(DurationMillis::new(1_001)?);
    for phase_timings in [
        RunPhaseTimingsV1 {
            compile_milliseconds: over_elapsed,
            ..RunPhaseTimingsV1::default()
        },
        RunPhaseTimingsV1 {
            backend_milliseconds: over_elapsed,
            ..RunPhaseTimingsV1::default()
        },
        RunPhaseTimingsV1 {
            projection_milliseconds: over_elapsed,
            ..RunPhaseTimingsV1::default()
        },
        RunPhaseTimingsV1 {
            structural_validation_milliseconds: over_elapsed,
            ..RunPhaseTimingsV1::default()
        },
        RunPhaseTimingsV1 {
            score_recomputation_milliseconds: over_elapsed,
            ..RunPhaseTimingsV1::default()
        },
        RunPhaseTimingsV1 {
            required_rule_verification_milliseconds: over_elapsed,
            ..RunPhaseTimingsV1::default()
        },
        RunPhaseTimingsV1 {
            evidence_persistence_milliseconds: over_elapsed,
            ..RunPhaseTimingsV1::default()
        },
        RunPhaseTimingsV1 {
            optional_explanation_milliseconds: over_elapsed,
            ..RunPhaseTimingsV1::default()
        },
    ] {
        invalid = manifest.clone();
        invalid.phase_timings = phase_timings;
        assert_eq!(
            invalid.validate(),
            Err(DomainContractError::InvalidRunTiming)
        );
    }
    let mut unknown = serde_json::to_value(&manifest)?;
    unknown
        .as_object_mut()
        .ok_or("manifest must serialize as object")?
        .insert("nativeLog".to_owned(), json!("forbidden"));
    assert!(matches!(
        RunManifest::from_json(&serde_json::to_vec(&unknown)?),
        Err(DomainContractError::MalformedJson(_))
    ));
    let mut oversized = serde_json::to_value(&manifest)?;
    oversized["elapsedMilliseconds"] = json!(9_007_199_254_740_992_u64);
    assert!(matches!(
        RunManifest::from_json(&serde_json::to_vec(&oversized)?),
        Err(DomainContractError::MalformedJson(_))
    ));
    Ok(())
}

#[test]
// One mutation table proves every portable-result cross-binding independently rejects.
#[allow(clippy::too_many_lines)]
fn portable_result_rejects_every_cross_contract_binding() -> Result<(), Box<dyn Error>> {
    let portable = portable_result()?;
    let mut invalid = portable.clone();
    invalid.result_id = "01890a5d-ac96-7b64-9f74-bbfcf30f9f85".parse()?;
    assert!(matches!(
        invalid.validate(),
        Err(DomainContractError::PortableResultBindingMismatch(
            "result ID"
        ))
    ));
    invalid = portable.clone();
    invalid.scenario_id = "01890a5d-ac96-7b64-9f74-bbfcf30f9f86".parse()?;
    assert!(matches!(
        invalid.validate(),
        Err(DomainContractError::PortableResultBindingMismatch(
            "scenario ID"
        ))
    ));
    invalid = portable.clone();
    invalid.scenario_revision += 1;
    assert!(matches!(
        invalid.validate(),
        Err(DomainContractError::PortableResultBindingMismatch(
            "scenario revision"
        ))
    ));

    let accepted = portable.accepted_result.clone();
    let other_input = run_input_for(
        &accepted,
        run_id(0x85)?,
        accepted.verification.document_hash.clone(),
        accepted.verification.planning_model_hash.clone(),
    )?;
    invalid = portable.clone();
    invalid.run_manifest = accepted_manifest(&other_input, &accepted)?;
    assert!(matches!(
        invalid.validate(),
        Err(DomainContractError::PortableResultBindingMismatch("run ID"))
    ));
    invalid = portable.clone();
    invalid.run_manifest = manifest_for(
        invalid.run_input.run_id,
        blake3_hex(b"other-run-input"),
        accepted_outcome(&accepted),
        Some(DurationMillis::new(500)?),
        accepted.verification.warnings.clone(),
    )?;
    assert!(matches!(
        invalid.validate(),
        Err(DomainContractError::PortableResultBindingMismatch(
            "run-input checksum"
        ))
    ));

    let bad_outcomes = [
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: "01890a5d-ac96-7b64-9f74-bbfcf30f9f85".parse()?,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: blake3_hex(b"other-result"),
            verification_checksum: accepted.verification.checksum.clone(),
        },
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: blake3_hex(b"other-report"),
        },
    ];
    for outcome in bad_outcomes {
        invalid = portable.clone();
        invalid.run_manifest = manifest_for(
            invalid.run_input.run_id,
            invalid.run_input.checksum.clone(),
            outcome,
            Some(DurationMillis::new(500)?),
            accepted.verification.warnings.clone(),
        )?;
        assert!(matches!(
            invalid.validate(),
            Err(DomainContractError::PortableResultBindingMismatch(
                "accepted outcome"
            ))
        ));
    }
    invalid = portable.clone();
    invalid.run_manifest = manifest_for(
        invalid.run_input.run_id,
        invalid.run_input.checksum.clone(),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible,
        },
        None,
        accepted.verification.warnings.clone(),
    )?;
    assert!(matches!(
        invalid.validate(),
        Err(DomainContractError::PortableResultBindingMismatch(
            "terminal outcome"
        ))
    ));
    invalid = portable.clone();
    invalid.run_manifest = manifest_for(
        invalid.run_input.run_id,
        invalid.run_input.checksum.clone(),
        accepted_outcome(&accepted),
        Some(DurationMillis::new(500)?),
        Vec::new(),
    )?;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::PortableResultBindingMismatch(
            "verification warnings"
        ))
    );

    let late_snapshot_input = run_input_at(
        &accepted,
        portable.run_input.run_id,
        accepted.verification.document_hash.clone(),
        accepted.verification.planning_model_hash.clone(),
        "2026-09-03T12:00:02Z".parse()?,
    )?;
    invalid = portable.clone();
    invalid.run_manifest = accepted_manifest(&late_snapshot_input, &accepted)?;
    invalid.run_input = late_snapshot_input;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::PortableResultBindingMismatch(
            "snapshot creation time"
        ))
    );
    Ok(())
}

#[test]
fn portable_result_binds_pack_document_model_and_nested_checksums() -> Result<(), Box<dyn Error>> {
    let portable = portable_result()?;
    let accepted = portable.accepted_result.clone();

    for (document_hash, model_hash, expected) in [
        (
            blake3_hex(b"other-document"),
            accepted.verification.planning_model_hash.clone(),
            "snapshot document hash",
        ),
        (
            accepted.verification.document_hash.clone(),
            blake3_hex(b"other-model"),
            "planning model hash",
        ),
    ] {
        let input = run_input_for(
            &accepted,
            portable.run_input.run_id,
            document_hash,
            model_hash,
        )?;
        let manifest = accepted_manifest(&input, &accepted)?;
        let invalid = PortableAcceptedResult {
            run_input: input,
            run_manifest: manifest,
            ..portable.clone()
        };
        assert_eq!(
            invalid.validate(),
            Err(DomainContractError::PortableResultBindingMismatch(expected))
        );
    }

    let mut other_solution = accepted.solution.clone();
    other_solution.pack_id = PackId::new("official.other")?;
    let other_report = report(&other_solution, 0, true)?;
    let other_accepted = AcceptedResult::new(other_solution, other_report)?;
    let manifest = manifest_for(
        portable.run_input.run_id,
        portable.run_input.checksum.clone(),
        accepted_outcome(&other_accepted),
        Some(DurationMillis::new(500)?),
        other_accepted.verification.warnings.clone(),
    )?;
    let invalid = PortableAcceptedResult {
        accepted_result: other_accepted,
        run_manifest: manifest,
        ..portable.clone()
    };
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::PortableResultBindingMismatch(
            "pack ID"
        ))
    );

    let mut invalid = portable.clone();
    invalid.run_input.checksum = blake3_hex(b"bad-input-checksum");
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::ChecksumMismatch)
    );
    invalid = portable.clone();
    invalid.run_manifest.checksum = blake3_hex(b"bad-manifest-checksum");
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::ChecksumMismatch)
    );
    invalid = portable.clone();
    invalid.evidence.insert(
        DomainEvidenceId::new("evidence.first")?,
        VerificationValue::Integer(2),
    );
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::ChecksumMismatch)
    );
    invalid = portable;
    invalid.checksum = blake3_hex(b"bad-portable-checksum");
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::ChecksumMismatch)
    );
    Ok(())
}

#[test]
fn portable_result_rejects_newer_unknown_and_evidence_bounds() -> Result<(), Box<dyn Error>> {
    let portable = portable_result()?;
    let mut invalid = portable.clone();
    invalid.schema_version = PORTABLE_ACCEPTED_RESULT_SCHEMA_VERSION + 1;
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::UnsupportedVersion(
            PORTABLE_ACCEPTED_RESULT_SCHEMA_VERSION + 1
        ))
    );

    let mut unknown = serde_json::to_value(&portable)?;
    unknown
        .as_object_mut()
        .ok_or("portable result must serialize as object")?
        .insert("backendAuthority".to_owned(), json!(true));
    assert!(matches!(
        PortableAcceptedResult::from_json(&serde_json::to_vec(&unknown)?),
        Err(DomainContractError::MalformedJson(_))
    ));

    invalid = portable.clone();
    invalid.evidence.insert(
        DomainEvidenceId::new("evidence.text")?,
        VerificationValue::Text("x".repeat(MAX_VERIFICATION_TEXT_BYTES + 1)),
    );
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::InvalidVerificationText)
    );

    invalid = portable.clone();
    invalid.evidence.insert(
        DomainEvidenceId::new("evidence.orphan")?,
        VerificationValue::Integer(1),
    );
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::PortableResultBindingMismatch(
            "evidence reference"
        ))
    );

    invalid = portable;
    invalid.evidence.clear();
    for index in 0..=MAX_PORTABLE_RESULT_EVIDENCE_RECORDS {
        invalid.evidence.insert(
            DomainEvidenceId::new(format!("evidence.item-{index}"))?,
            VerificationValue::Integer(0),
        );
    }
    assert_eq!(
        invalid.validate(),
        Err(DomainContractError::LimitExceeded(
            "portable result evidence"
        ))
    );
    Ok(())
}
#[test]
fn every_json_entrypoint_rejects_oversized_input_before_deserialization() {
    let bytes = vec![b' '; MAX_DOMAIN_CONTRACT_JSON_BYTES + 1];
    macro_rules! assert_oversized {
        ($contract:ty) => {
            assert_eq!(
                <$contract>::from_json(&bytes),
                Err(DomainContractError::LimitExceeded(
                    "domain contract JSON bytes"
                ))
            );
        };
    }
    assert_oversized!(NormalizedSolution);
    assert_oversized!(VerificationScope);
    assert_oversized!(VerificationContextV1);
    assert_oversized!(VerificationReport);
    assert_oversized!(AcceptedResult);
    assert_oversized!(RunInput);
    assert_oversized!(RunManifest);
    assert_oversized!(PortableAcceptedResult);
    assert_oversized!(LegacyVerificationReportV1);
    assert_oversized!(LegacyAcceptedResultV1);
}

#[test]
fn portable_ingress_applies_shared_nonsecret_policy_before_serde() -> Result<(), Box<dyn Error>> {
    let portable = portable_result()?;
    let mut input = serde_json::to_value(&portable.run_input)?;
    input
        .as_object_mut()
        .ok_or("run input must serialize as object")?
        .insert("secretToken".to_owned(), json!("not-retained"));
    assert!(matches!(
        RunInput::from_json(&serde_json::to_vec(&input)?),
        Err(DomainContractError::PortableJsonViolation(_))
    ));

    let mut accepted = serde_json::to_value(&portable.accepted_result)?;
    accepted["verification"]["warnings"][0]["facts"]["fact.detail"] = json!("/tmp/native.log");
    assert!(matches!(
        AcceptedResult::from_json(&serde_json::to_vec(&accepted)?),
        Err(DomainContractError::PortableJsonViolation(_))
    ));
    let deep = format!(
        "{}0{}",
        "[".repeat(MAX_DOMAIN_CONTRACT_JSON_DEPTH + 1),
        "]".repeat(MAX_DOMAIN_CONTRACT_JSON_DEPTH + 1)
    );
    assert!(matches!(
        NormalizedSolution::from_json(deep.as_bytes()),
        Err(DomainContractError::PortableJsonViolation(_))
    ));

    let long_string = serde_json::to_vec(&"x".repeat(MAX_VERIFICATION_TEXT_BYTES + 1))?;
    assert!(matches!(
        NormalizedSolution::from_json(&long_string),
        Err(DomainContractError::PortableJsonViolation(_))
    ));

    let too_many_items = format!("[{}0]", "0,".repeat(MAX_DOMAIN_CONTRACT_JSON_ITEMS));
    assert!(matches!(
        NormalizedSolution::from_json(too_many_items.as_bytes()),
        Err(DomainContractError::PortableJsonViolation(_))
    ));
    Ok(())
}

fn mixed_score(
    feasibility: i64,
    minimized: i64,
    maximized: i64,
) -> Result<ScoreVector, Box<dyn Error>> {
    Ok(ScoreVector {
        feasibility,
        levels: vec![
            ScoreLevelValue {
                level_id: ScoreLevelId::new("score.primary")?,
                value: minimized,
                direction: OptimizationDirection::Minimize,
                category_breakdown: BTreeMap::new(),
            },
            ScoreLevelValue {
                level_id: ScoreLevelId::new("score.secondary")?,
                value: maximized,
                direction: OptimizationDirection::Maximize,
                category_breakdown: BTreeMap::new(),
            },
        ],
    })
}

#[test]
fn score_shape_is_bounded_and_category_breakdowns_are_optional() -> Result<(), Box<dyn Error>> {
    let mut invalid = score(0)?;
    invalid.feasibility = -1;
    assert_eq!(
        invalid.validate_shape(),
        Err(DomainContractError::NegativeFeasibility)
    );

    let level = score(0)?.levels.remove(0);
    let mut too_many_levels = ScoreVector {
        feasibility: 0,
        levels: vec![level.clone(); MAX_SCORE_LEVELS + 1],
    };
    for (index, score_level) in too_many_levels.levels.iter_mut().enumerate() {
        score_level.level_id = ScoreLevelId::new(format!("score.level.{index}"))?;
    }
    assert_eq!(
        too_many_levels.validate_current_shape(),
        Err(DomainContractError::LimitExceeded("score levels"))
    );
    assert_eq!(
        too_many_levels.compare(&too_many_levels),
        Err(DomainContractError::LimitExceeded("score levels"))
    );

    let mut categories = BTreeMap::new();
    for index in 0..MAX_SCORE_CATEGORIES_PER_LEVEL {
        categories.insert(
            ScoreCategoryId::new(format!("score.category.{index}"))?,
            i64::try_from(index)?,
        );
    }
    let mut bounded = score(7)?;
    bounded.levels[0].category_breakdown = categories;
    assert!(bounded.validate_current_shape().is_ok());
    bounded.levels[0]
        .category_breakdown
        .insert(ScoreCategoryId::new("score.category.overflow")?, -1);
    assert_eq!(
        bounded.validate_current_shape(),
        Err(DomainContractError::LimitExceeded("score categories"))
    );
    assert_eq!(
        bounded.compare(&bounded),
        Err(DomainContractError::LimitExceeded("score categories"))
    );

    let mut duplicate = mixed_score(0, 1, 2)?;
    duplicate.levels[1].level_id = duplicate.levels[0].level_id.clone();
    assert_eq!(
        duplicate.validate_shape(),
        Err(DomainContractError::DuplicateScoreLevel)
    );
    assert!(score(7)?.validate_shape().is_ok());
    Ok(())
}

#[test]
fn score_uses_feasibility_then_mixed_lexicographic_direction() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        mixed_score(0, 2, i64::MIN)?.compare(&mixed_score(0, 3, i64::MAX)?)?,
        Ordering::Less
    );
    assert_eq!(
        mixed_score(0, 2, 5)?.compare(&mixed_score(0, 2, 4)?)?,
        Ordering::Less
    );
    assert_eq!(
        mixed_score(1, i64::MIN, i64::MAX)?.compare(&mixed_score(0, i64::MAX, i64::MIN)?)?,
        Ordering::Greater
    );

    let mut mismatch = mixed_score(0, 2, 5)?;
    mismatch.levels[1].direction = OptimizationDirection::Minimize;
    assert_eq!(
        mixed_score(0, 2, 5)?.compare(&mismatch),
        Err(DomainContractError::ScoreShapeMismatch)
    );

    // A different first value or feasibility cannot bypass a later score-shape mismatch.
    let mut late_identity_mismatch = mixed_score(0, 3, 5)?;
    late_identity_mismatch.levels[1].level_id = ScoreLevelId::new("score.other")?;
    assert_eq!(
        mixed_score(0, 2, 5)?.compare(&late_identity_mismatch),
        Err(DomainContractError::ScoreShapeMismatch)
    );
    let mut late_direction_mismatch = mixed_score(0, 2, 5)?;
    late_direction_mismatch.levels[1].direction = OptimizationDirection::Minimize;
    assert_eq!(
        mixed_score(1, 2, 5)?.compare(&late_direction_mismatch),
        Err(DomainContractError::ScoreShapeMismatch)
    );
    Ok(())
}

#[test]
fn score_serialization_is_deterministic_and_breakdowns_need_not_sum() -> Result<(), Box<dyn Error>>
{
    let first = ScoreCategoryId::new("score.category.first")?;
    let second = ScoreCategoryId::new("score.category.second")?;
    let mut left = score(42)?;
    left.levels[0].category_breakdown = BTreeMap::from([(second.clone(), -7), (first.clone(), 99)]);
    let mut right = score(42)?;
    right.levels[0].category_breakdown = BTreeMap::from([(first, 99), (second, -7)]);

    assert!(left.validate_shape().is_ok());
    assert_eq!(left.compare(&right)?, Ordering::Equal);
    assert_eq!(serde_json::to_vec(&left)?, serde_json::to_vec(&right)?);
    Ok(())
}

proptest! {
    #[test]
    fn mixed_direction_score_comparison_is_a_total_preorder(
        left in (0_i64..100, any::<i64>(), any::<i64>()),
        middle in (0_i64..100, any::<i64>(), any::<i64>()),
        right in (0_i64..100, any::<i64>(), any::<i64>()),
    ) {
        let Ok(left) = mixed_score(left.0, left.1, left.2) else {
            return Ok(());
        };
        let Ok(middle) = mixed_score(middle.0, middle.1, middle.2) else {
            return Ok(());
        };
        let Ok(right) = mixed_score(right.0, right.1, right.2) else {
            return Ok(());
        };

        prop_assert_eq!(left.compare(&left), Ok(Ordering::Equal));
        let Ok(left_middle) = left.compare(&middle) else {
            return Ok(());
        };
        let Ok(middle_left) = middle.compare(&left) else {
            return Ok(());
        };
        let Ok(middle_right) = middle.compare(&right) else {
            return Ok(());
        };
        prop_assert_eq!(left_middle, middle_left.reverse());
        if left_middle != Ordering::Greater && middle_right != Ordering::Greater {
            prop_assert_ne!(left.compare(&right), Ok(Ordering::Greater));
        }
        if left_middle == Ordering::Equal {
            prop_assert_eq!(left.feasibility, middle.feasibility);
            prop_assert!(left
                .levels
                .iter()
                .zip(&middle.levels)
                .all(|(left, right)| left.value == right.value));
        }
    }
}

#[test]
fn validation_issue_identity_distinguishes_same_code_and_field() -> Result<(), Box<dyn Error>> {
    let first = ValidationIssue {
        code: "official.workforce.same_code".to_owned(),
        severity: ValidationSeverity::Error,
        message: "first native message".to_owned(),
        field_path: Some("/domain/entities/example".to_owned()),
        resource: None,
    };
    let second = ValidationIssue {
        message: "second native message".to_owned(),
        ..first.clone()
    };
    let first_id = validation_issue_identity(&first)?;
    let second_id = validation_issue_identity(&second)?;
    let mut severity_changed = first.clone();
    severity_changed.severity = ValidationSeverity::Warning;
    let mut resource_changed = first.clone();
    resource_changed.resource = Some(ResourceRef::Scenario(
        "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse()?,
    ));
    let severity_id = validation_issue_identity(&severity_changed)?;
    let resource_id = validation_issue_identity(&resource_changed)?;
    assert_ne!(first_id, second_id);
    assert_ne!(first_id, severity_id);
    assert_ne!(first_id, resource_id);
    assert!(first_id.as_str().starts_with("validation.issue."));
    assert_eq!(first_id.as_str().len(), "validation.issue.".len() + 64);
    assert_eq!(validation_issue_identity(&first)?, first_id);
    Ok(())
}

// Frozen from the actual Rust V1/V2 writers before the objective-profile cutover.
// These are historical inputs, not fixtures regenerated from current constructors.
#[test]
fn historical_run_wire_preserves_checksums_and_request_semantics() -> Result<(), Box<dyn Error>> {
    let request_bytes = include_bytes!("fixtures/request_semantics_v1.json");
    let portable_bytes = include_bytes!("fixtures/portable_accepted_v2.json");
    let checkpoint_bytes = include_bytes!("fixtures/verified_checkpoint_v1.json");
    let semantics: RunRequestSemantics = serde_json::from_slice(request_bytes)?;
    let portable = PortableAcceptedResult::from_json(portable_bytes)?;
    let checkpoint = VerifiedCheckpoint::from_json(checkpoint_bytes)?;

    assert_eq!(semantics, portable.run_input.request_semantics());
    assert_eq!(semantics.canonical_hash()?, portable.run_input.request_hash);
    assert_eq!(checkpoint.run_input, portable.run_input);
    assert_eq!(checkpoint.accepted_result, portable.accepted_result);
    assert_eq!(serde_json::to_vec(&semantics)?, request_bytes);
    assert_eq!(serde_json::to_vec(&portable)?, portable_bytes);
    assert_eq!(serde_json::to_vec(&checkpoint)?, checkpoint_bytes);
    Ok(())
}

#[test]
fn legacy_objective_context_wire_preserves_request_bytes_and_hashes() -> Result<(), Box<dyn Error>>
{
    // Explicit V1/V2 wire specimens, not values emitted by the current context constructor.
    // These generic references establish serialization compatibility, not native acceptance.
    const BASELINE: &str = concat!(
        r#"{"original":{"entity":{"kind":"school.person","id":"school.base"},"pairContentHash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","#,
        r#""acceptedResult":{"solutionId":"01890a5d-ac96-7b64-9f74-bbfcf30f9f81","resultChecksum":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"#,
        r#""sourceScenarioId":"01890a5d-ac96-7b64-9f74-bbfcf30f9f80","sourceRevision":7,"sourceDocumentHash":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","#,
        r#""sourceRunId":"01890a5d-ac96-7b64-9f74-bbfcf30f9f82","sourceRunInputChecksum":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","#,
        r#""sourceRunManifestChecksum":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"},"selection":{"kind":"originalAccepted"},"selectedPairContentHash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa""#,
    );
    let request_fields = include_str!("fixtures/request_semantics_v1.json")
        .strip_prefix(r#"{"schemaVersion":1,"#)
        .and_then(|fields| fields.strip_suffix('}'))
        .ok_or("frozen request must have the expected version and object boundary")?;
    for (version, captured) in [(1, ""), (2, r#","capturedOriginal":{}"#)] {
        let mut wire = format!(
            "{{\"schemaVersion\":{version},\"packId\":\"official.synthetic\",\"profileId\":\"official.synthetic.objective.minimum\",\"semanticVersion\":1,\"operation\":\"repair\",\"baseline\":{BASELINE}"
        );
        wire.push_str(captured);
        wire.push_str("}}");
        let context: RunObjectiveContext = serde_json::from_str(&wire)?;
        context.validate_for(
            &PackId::new("official.synthetic")?,
            "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse()?,
        )?;
        assert_eq!(serde_json::to_vec(&context)?, wire.as_bytes());
        let request =
            format!("{{\"schemaVersion\":2,{request_fields},\"objectiveContext\":{wire}}}");
        let semantics: RunRequestSemantics = serde_json::from_str(&request)?;
        assert_eq!(serde_json::to_vec(&semantics)?, request.as_bytes());
        assert_eq!(semantics.canonical_hash()?, blake3_hex(request.as_bytes()));
    }
    Ok(())
}

// One baseline fixture covers the legacy/current matrix and complete captured carriers.
#[allow(clippy::too_many_lines)]
#[test]
fn captured_baseline_preserves_legacy_context_and_enforces_version_and_size()
-> Result<(), Box<dyn Error>> {
    let portable = portable_result()?;
    let input = &portable.run_input;
    let binding = RunBaselineBinding {
        original: RunBaselineOriginal {
            entity: entity("base")?,
            pair_content_hash: "a".repeat(64),
            accepted_result: AcceptedResultRefV1::from_result(&portable.accepted_result)?,
            source_scenario_id: input.scenario_id,
            source_revision: input.scenario_revision,
            source_document_hash: input.snapshot_document_hash.clone(),
            source_run_id: input.run_id,
            source_run_input_checksum: input.checksum.clone(),
            source_run_manifest_checksum: portable.run_manifest.checksum.clone(),
        },
        selection: RunBaselineSelection::OriginalAccepted {},
        selected_pair_content_hash: "a".repeat(64),
        captured_original: None,
    };
    let legacy = serde_json::to_value(&binding)?;
    assert!(legacy.get("capturedOriginal").is_none());
    let mut nullable = legacy.clone();
    nullable["capturedOriginal"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<RunBaselineBinding>(nullable.clone()).is_err());
    assert!(serde_json::from_slice::<RunBaselineBinding>(&serde_json::to_vec(&nullable)?).is_err());
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<RunBaselineBinding>(
            legacy.clone()
        )?)?,
        legacy,
    );
    let mut wire_prefix = serde_json::to_string(&legacy)?;
    wire_prefix.pop();
    wire_prefix.push_str(",\"capturedOriginal\":");
    for invalid in [
        r#"{"duplicate":0,"duplicate":1}"#,
        r#"{"duplicate":0,"\u0064uplicate":1}"#,
        "[1,]",
        r#"{"key":}"#,
        "NaN",
        "1e999",
    ] {
        assert!(
            serde_json::from_str::<RunBaselineBinding>(&format!("{wire_prefix}{invalid}}}"))
                .is_err(),
            "accepted invalid capture: {invalid}",
        );
    }
    assert!(
        serde_json::from_str::<RunBaselineBinding>(&format!(
            "{wire_prefix}{{}},\"capturedOriginal\":{{}}}}",
        ))
        .is_err(),
    );

    // Canonical escaping, number formatting and every container delimiter count,
    // while even a pretty-printed wire larger than the cap is admissible.
    for payload in [
        json!([]),
        json!({}),
        serde_json::from_str(
            r#"{"\"\\\u0000\n\té":["\\\"\b\f\r\n\t\u001f",null,true,false,-9223372036854775808,18446744073709551615,-0.0,1e0,1e-5,1e30]}"#,
        )?,
    ] {
        let mut capture = json!({"payload": payload, "padding": ""});
        let overhead = serde_json::to_vec(&capture)?.len();
        for extra in [0, 1] {
            capture["padding"] = json!("x".repeat(MAX_CAPTURED_BASELINE_BYTES - overhead + extra));
            let mut wire_value = legacy.clone();
            wire_value["capturedOriginal"] = capture.clone();
            let wire = serde_json::to_vec_pretty(&wire_value)?;
            assert!(wire.len() > MAX_CAPTURED_BASELINE_BYTES);
            assert_eq!(
                serde_json::from_slice::<RunBaselineBinding>(&wire).is_ok(),
                extra == 0,
            );
            assert_eq!(
                serde_json::from_value::<RunBaselineBinding>(wire_value).is_ok(),
                extra == 0,
            );
        }
    }

    for extra in [0, 1] {
        // Alternate wire escaping is not itself part of the canonical-byte cap.
        let wire = format!(
            "{wire_prefix}\"{}\"}}",
            "\\u0078".repeat(MAX_CAPTURED_BASELINE_BYTES - 2 + extra),
        );
        assert_eq!(
            serde_json::from_str::<RunBaselineBinding>(&wire).is_ok(),
            extra == 0,
        );
    }

    // A reader proves refusal while still inside the over-limit collection,
    // without reading or retaining the large remaining member.
    let zeros = "0,".repeat(MAX_CAPTURED_BASELINE_BYTES / 2);
    let mut entries = String::new();
    for index in 0..MAX_CAPTURED_BASELINE_BYTES / 8 {
        use std::fmt::Write as _;
        write!(entries, "\"{index}\":0,")?;
    }
    let unread = "unread".repeat(MAX_CAPTURED_BASELINE_BYTES);
    for (opening, members, closing) in [
        ("[", zeros.as_str(), "]"),
        ("{\"values\":[", zeros.as_str(), "]}"),
        ("{", entries.as_str(), "}"),
    ] {
        let overflowing_prefix = format!("{wire_prefix}{opening}{members}");
        let refusal_boundary = overflowing_prefix.len();
        let next_member = if opening == "{" { "\"unread\":" } else { "" };
        let wire = format!("{overflowing_prefix}{next_member}\"{unread}\"{closing}}}");
        let mut reader = std::io::Cursor::new(wire.as_bytes());
        let Err(error) = serde_json::from_reader::<_, RunBaselineBinding>(&mut reader) else {
            return Err("oversized collection survived streaming admission".into());
        };
        assert!(error.to_string().contains("captured baseline JSON bytes"));
        // serde_json may peek one byte for error positioning, but not the string contents.
        assert!(reader.position() <= u64::try_from(refusal_boundary + 1)?);
    }

    for version in [1, 2, 3] {
        for captured in [false, true] {
            let mut baseline = binding.clone();
            baseline.captured_original = captured.then(|| std::sync::Arc::new(json!({})));
            let context = RunObjectiveContext {
                schema_version: version,
                pack_id: input.pack_id.clone(),
                profile_id: ObjectiveProfileId::new("official.synthetic.objective.minimum")?,
                semantic_version: 1,
                operation: RunObjectiveOperation::Repair,
                baseline: Some(Box::new(baseline)),
                external_baseline: None,
            };
            assert_eq!(
                context
                    .validate_for(&input.pack_id, input.scenario_id)
                    .is_ok(),
                matches!((version, captured), (1, false) | (2, true)),
            );
        }
    }

    let external = RunExternalBaselineBinding {
        entity: entity("external")?,
        selected_pair_content_hash: "c".repeat(64),
    };
    let external_context = RunObjectiveContext {
        schema_version: 3,
        pack_id: input.pack_id.clone(),
        profile_id: ObjectiveProfileId::new("official.synthetic.objective.minimum")?,
        semantic_version: 1,
        operation: RunObjectiveOperation::Repair,
        baseline: None,
        external_baseline: Some(Box::new(external)),
    };
    for version in [1, 2, 3, 4] {
        for accepted in [false, true] {
            for selected_external in [false, true] {
                let mut context = external_context.clone();
                context.schema_version = version;
                context.baseline = accepted.then(|| Box::new(binding.clone()));
                if !selected_external {
                    context.external_baseline = None;
                }
                assert_eq!(
                    context
                        .validate_for(&input.pack_id, input.scenario_id)
                        .is_ok(),
                    matches!(
                        (version, accepted, selected_external),
                        (1, true, false) | (3, false, true)
                    ),
                );
            }
        }
    }
    let external_wire = serde_json::to_vec(&external_context)?;
    assert_eq!(
        serde_json::from_slice::<RunObjectiveContext>(&external_wire)?,
        external_context,
    );
    let mut invalid = serde_json::to_value(&external_context)?;
    invalid["externalBaseline"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<RunObjectiveContext>(invalid).is_err());
    let mut authored_external = external_context.clone();
    authored_external.profile_id = ObjectiveProfileId::new(AUTHORED_OBJECTIVE_PROFILE_ID)?;
    authored_external.operation = RunObjectiveOperation::Optimize;
    assert!(
        authored_external
            .validate_for(&input.pack_id, input.scenario_id)
            .is_err()
    );
    let mut semantics = input.request_semantics();
    semantics.objective_context = Some(external_context);
    let original_request_hash = semantics.canonical_hash()?;
    semantics
        .objective_context
        .as_mut()
        .ok_or("missing external context")?
        .external_baseline
        .as_mut()
        .ok_or("missing external baseline")?
        .entity = entity("other-external")?;
    assert_ne!(semantics.canonical_hash()?, original_request_hash);
    semantics
        .objective_context
        .as_mut()
        .ok_or("missing external context")?
        .external_baseline
        .as_mut()
        .ok_or("missing external baseline")?
        .selected_pair_content_hash = "not-a-digest".to_owned();
    assert!(semantics.validate().is_err());
    let mut captured = binding;
    for bytes in [
        MAX_CAPTURED_BASELINE_BYTES - 1,
        MAX_CAPTURED_BASELINE_BYTES,
        MAX_CAPTURED_BASELINE_BYTES + 1,
    ] {
        // Two quote bytes: the generic carrier bounds opaque JSON; the pack checks its shape.
        captured.captured_original = Some(std::sync::Arc::new(json!("x".repeat(bytes - 2))));
        assert_eq!(
            captured.validate_for(input.scenario_id).is_ok(),
            bytes <= MAX_CAPTURED_BASELINE_BYTES,
        );
        let wire = serde_json::to_vec(&captured)?;
        assert_eq!(
            serde_json::from_slice::<RunBaselineBinding>(&wire).is_ok(),
            bytes <= MAX_CAPTURED_BASELINE_BYTES,
        );
        assert_eq!(
            serde_json::from_value::<RunBaselineBinding>(serde_json::to_value(&captured)?).is_ok(),
            bytes <= MAX_CAPTURED_BASELINE_BYTES,
        );
    }
    let mut chunks = vec!["x".repeat(1021); 512];
    chunks[511].pop();
    captured.captured_original = Some(std::sync::Arc::new(json!(chunks)));
    let context = RunObjectiveContext {
        schema_version: 2,
        pack_id: input.pack_id.clone(),
        profile_id: ObjectiveProfileId::new("official.synthetic.objective.minimum")?,
        semantic_version: 1,
        operation: RunObjectiveOperation::Repair,
        baseline: Some(Box::new(captured.clone())),
        external_baseline: None,
    };
    let captured_input = RunInput::new(
        input.run_id,
        input.request_id,
        input.scenario_id,
        input.scenario_revision,
        input.snapshot_id,
        input.snapshot_document_hash.clone(),
        input.snapshot_created_at,
        input.pack_id.clone(),
        input.pack_schema_version,
        input.planning_ir_schema_version,
        input.compiler_version.clone(),
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
        None,
        Some(context),
    )?;
    let manifest = accepted_manifest(&captured_input, &portable.accepted_result)?;
    let captured_portable = PortableAcceptedResult::new(
        captured_input,
        manifest,
        portable.accepted_result.clone(),
        portable.evidence.clone(),
    )?;
    assert_eq!(
        PortableAcceptedResult::from_json(&serde_json::to_vec(&captured_portable)?)?,
        captured_portable,
    );
    captured.captured_original = Some(std::sync::Arc::new(json!({})));
    captured.selection = RunBaselineSelection::ManualSelection {
        entity: entity("manual")?,
        record_checksum: "b".repeat(64),
        admission_source_scenario_id: input.scenario_id,
        admission_source_revision: input.scenario_revision,
        admission_source_document_hash: input.snapshot_document_hash.clone(),
    };
    assert!(captured.validate_for(input.scenario_id).is_err());
    Ok(())
}

#[test]
fn objective_context_is_versioned_nonnull_and_bound_to_request() -> Result<(), Box<dyn Error>> {
    let portable = portable_result()?;
    let input = portable.run_input;
    let authored = input
        .objective_context
        .as_ref()
        .ok_or("missing current context")?;
    assert!(
        authored
            .validate_for(&input.pack_id, input.scenario_id)
            .is_ok()
    );
    let mut other = input.request_semantics();
    other
        .objective_context
        .as_mut()
        .ok_or("missing context")?
        .profile_id = ObjectiveProfileId::new("official.test.objective.other")?;
    assert_ne!(other.canonical_hash()?, input.request_hash);

    for (version, context) in [
        (1, serde_json::to_value(authored)?),
        (2, serde_json::Value::Null),
    ] {
        let mut bytes = serde_json::to_value(&input)?;
        bytes["schemaVersion"] = json!(version);
        bytes["objectiveContext"] = context;
        assert!(RunInput::from_json(&serde_json::to_vec(&bytes)?).is_err());
    }
    let mut absent = serde_json::to_value(&input)?;
    absent
        .as_object_mut()
        .ok_or("input object")?
        .remove("objectiveContext");
    assert!(RunInput::from_json(&serde_json::to_vec(&absent)?).is_err());

    let mut invalid = authored.clone();
    invalid.operation = RunObjectiveOperation::Repair;
    assert!(
        invalid
            .validate_for(&input.pack_id, input.scenario_id)
            .is_err()
    );
    invalid = authored.clone();
    invalid.pack_id = PackId::new("official.other")?;
    assert!(
        invalid
            .validate_for(&input.pack_id, input.scenario_id)
            .is_err()
    );
    invalid = authored.clone();
    invalid.semantic_version = 2;
    assert!(
        invalid
            .validate_for(&input.pack_id, input.scenario_id)
            .is_err()
    );
    let mut nullable = serde_json::to_value(authored)?;
    nullable["baseline"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<RunObjectiveContext>(nullable).is_err());

    // Old wrappers cannot smuggle current input semantics; current wrappers can retain old inputs.
    let current = portable_result()?;
    let mut old_wrapper = current.clone();
    old_wrapper.schema_version = 2;
    assert!(matches!(
        old_wrapper.validate(),
        Err(DomainContractError::PortableResultBindingMismatch(_))
    ));
    let legacy =
        PortableAcceptedResult::from_json(include_bytes!("fixtures/portable_accepted_v2.json"))?;
    let wrapped = PortableAcceptedResult::new(
        legacy.run_input,
        legacy.run_manifest,
        legacy.accepted_result,
        legacy.evidence,
    )?;
    assert_eq!(wrapped.run_input.schema_version, 1);
    wrapped.validate()?;
    Ok(())
}

mod complete_comparison {
    use super::*;

    fn source(
        revision: u64,
        value: i64,
        assignments: Vec<DomainAssignment>,
    ) -> Result<PortableAcceptedResult, Box<dyn Error>> {
        let mut solution = solution(revision)?;
        solution.solution_id = format!("01890a5d-ac96-7b64-9f74-{revision:012x}").parse()?;
        solution.assignments = assignments;
        solution.canonicalize()?;
        let context = VerificationContextV1::new(
            solution.scenario_id,
            revision,
            blake3_hex(format!("source-{revision}").as_bytes()),
            blake3_hex(format!("model-{revision}").as_bytes()),
            solution.canonical_hash()?,
            scope(revision)?.checksum,
        )?;
        let mut score = score(value)?;
        score.levels[0].category_breakdown =
            BTreeMap::from([(ScoreCategoryId::new("category.preference")?, value)]);
        let report = VerificationReport::new(
            &context,
            vec![evaluation(rule(1)?, true)?, evaluation(rule(2)?, true)?],
            score,
            Vec::new(),
            BTreeMap::new(),
        )?;
        let accepted = AcceptedResult::new(solution, report)?;
        let input = run_input_for(
            &accepted,
            run_id(
                u8::try_from(revision)?
                    .checked_add(0x90)
                    .ok_or("run suffix")?,
            )?,
            accepted.verification.document_hash.clone(),
            accepted.verification.planning_model_hash.clone(),
        )?;
        let manifest = accepted_manifest(&input, &accepted)?;
        Ok(PortableAcceptedResult::new(
            input,
            manifest,
            accepted,
            BTreeMap::new(),
        )?)
    }

    fn scoring_context<'a>(
        source: &'a PortableAcceptedResult,
        meaning: &[u8],
    ) -> Result<ComparisonScoringContext<'a>, Box<dyn Error>> {
        Ok(ComparisonScoringContext {
            source,
            producer_id: DomainEvidenceId::new("tests.scoring-context.v1")?,
            semantic_hash: blake3_hex(meaning),
        })
    }

    fn comparable<'a>(
        base: &'a PortableAcceptedResult,
        candidate: &'a PortableAcceptedResult,
    ) -> Result<ComparisonEnrichmentV2<'a>, Box<dyn Error>> {
        Ok(ComparisonEnrichmentV2 {
            scoring_contexts: Some((
                scoring_context(base, b"same units, population and weights")?,
                scoring_context(candidate, b"same units, population and weights")?,
            )),
            ..ComparisonEnrichmentV2::default()
        })
    }

    #[test]
    fn cross_revision_compatible_scores_keep_exact_category_deltas_and_source_proof()
    -> Result<(), Box<dyn Error>> {
        let base = source(7, 50, Vec::new())?;
        let candidate = source(8, 20, Vec::new())?;
        assert_ne!(
            base.accepted_result
                .verification
                .verification_scope_checksum,
            candidate
                .accepted_result
                .verification
                .verification_scope_checksum
        );
        assert_ne!(
            base.run_input.snapshot_document_hash,
            candidate.run_input.snapshot_document_hash
        );
        let comparison = prepare_comparison_v2(&base, &candidate, None)?
            .finish(comparable(&base, &candidate)?)?;
        let ScoreComparisonV2::Comparable {
            base_score,
            candidate_score,
            score_levels,
            ordering,
        } = &comparison.score
        else {
            return Err("revision identity must not suppress compatible scoring semantics".into());
        };
        assert_eq!(base_score.levels[0].value, 50);
        assert_eq!(candidate_score.levels[0].value, 20);
        assert_eq!(*ordering, ComparisonOrdering::Better);
        assert_eq!(score_levels[0].delta, -30);
        assert_eq!(score_levels[0].categories[0].delta, Some(-30));
        assert_eq!(comparison.base.scenario_revision, 7);
        assert_eq!(comparison.candidate.scenario_revision, 8);
        assert_eq!(comparison.runs.base.run_id, base.run_input.run_id);
        assert_eq!(comparison.runs.candidate.run_id, candidate.run_input.run_id);
        let encoded = serde_json::to_vec(&comparison)?;
        assert_eq!(SolutionComparisonV2::from_json(&encoded)?, comparison);
        Ok(())
    }

    #[test]
    fn unavailable_scores_retain_assignment_protection_and_affected_party_changes()
    -> Result<(), Box<dyn Error>> {
        let old_assignment = DomainAssignment {
            id: DomainAssignmentId::new("tests.changed-pair")?,
            entity: entity("changed-person")?,
            value: AssignmentValue::Boolean(true),
            evidence: Vec::new(),
        };
        let base = source(7, 50, vec![old_assignment.clone()])?;
        let candidate = source(8, 20, Vec::new())?;
        let protection = AssignmentProtectionComparisonV1::new(
            old_assignment.id.clone(),
            Some(AssignmentProtectionStateV1::Hard),
            Some(AssignmentProtectionStateV1::Soft { weight: 9 }),
            true,
            false,
        )?;
        let person = entity("changed-person")?;
        let mut enrichment = comparable(&base, &candidate)?;
        if let Some((_, after)) = &mut enrichment.scoring_contexts {
            after.semantic_hash = blake3_hex(b"different weights");
        }
        enrichment.protections.push(protection);
        enrichment
            .affected_parties
            .push(ComparisonEntityObservationV1 {
                entity: person.clone(),
                before_label: Some("Original person".to_owned()),
                after_label: Some("Renamed person".to_owned()),
            });
        let prepared = prepare_comparison_v2(&base, &candidate, None)?;
        assert_eq!(
            prepared.assignments(),
            &[AssignmentComparisonV1::Removed {
                before: old_assignment
            }]
        );
        let comparison = prepared.finish(enrichment)?;
        assert_eq!(
            comparison.score,
            ScoreComparisonV2::Unavailable {
                reason: ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch,
            }
        );
        assert!(!comparison.protections[0].state_preserved);
        assert!(!comparison.protections[0].assignment_preserved);
        assert_eq!(comparison.affected_entities, vec![person]);
        assert_eq!(
            comparison.entity_observations[0].before_label.as_deref(),
            Some("Original person")
        );
        assert_eq!(
            comparison.entity_observations[0].after_label.as_deref(),
            Some("Renamed person")
        );
        comparison.validate()?;
        Ok(())
    }

    #[test]
    fn protection_presence_strength_and_selection_are_distinct() -> Result<(), Box<dyn Error>> {
        let id = DomainAssignmentId::new("tests.protected-pair")?;
        for (before, after, selected, state_preserved) in [
            (
                None,
                Some(AssignmentProtectionStateV1::Unlocked),
                false,
                false,
            ),
            (
                Some(AssignmentProtectionStateV1::Soft { weight: 2 }),
                Some(AssignmentProtectionStateV1::Soft { weight: 3 }),
                true,
                false,
            ),
            (
                Some(AssignmentProtectionStateV1::Hard),
                Some(AssignmentProtectionStateV1::Hard),
                true,
                true,
            ),
        ] {
            let evidence = AssignmentProtectionComparisonV1::new(
                id.clone(),
                before,
                after,
                selected,
                selected,
            )?;
            assert_eq!(evidence.state_preserved, state_preserved);
            assert!(evidence.assignment_preserved);
        }
        for (before, after, before_selected, after_selected) in [
            (None, None, false, false),
            (
                Some(AssignmentProtectionStateV1::Soft { weight: 0 }),
                None,
                true,
                false,
            ),
            (None, Some(AssignmentProtectionStateV1::Hard), false, false),
        ] {
            assert!(
                AssignmentProtectionComparisonV1::new(
                    id.clone(),
                    before,
                    after,
                    before_selected,
                    after_selected,
                )
                .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn protection_only_changes_can_affect_more_than_one_verification_record_limit()
    -> Result<(), Box<dyn Error>> {
        let base = source(7, 0, Vec::new())?;
        let candidate = source(8, 0, Vec::new())?;
        let mut enrichment = ComparisonEnrichmentV2::default();
        for index in 0..300 {
            enrichment
                .protections
                .push(AssignmentProtectionComparisonV1::new(
                    DomainAssignmentId::new(format!("tests.pair-{index:03}"))?,
                    Some(AssignmentProtectionStateV1::Soft { weight: 2 }),
                    Some(AssignmentProtectionStateV1::Unlocked),
                    false,
                    false,
                )?);
            enrichment
                .affected_parties
                .push(ComparisonEntityObservationV1 {
                    entity: entity(&format!("person-{index:03}"))?,
                    before_label: Some(format!("Person {index}")),
                    after_label: Some(format!("Person {index}")),
                });
        }
        let expected_entities: Vec<_> = enrichment
            .affected_parties
            .iter()
            .map(|party| party.entity.clone())
            .collect();
        let comparison = prepare_comparison_v2(&base, &candidate, None)?.finish(enrichment)?;
        assert!(comparison.assignments.is_empty());
        assert_eq!(comparison.affected_entities, expected_entities);
        assert!(
            comparison
                .protections
                .iter()
                .all(|item| !item.state_preserved && item.assignment_preserved)
        );
        assert!(matches!(
            comparison.score,
            ScoreComparisonV2::Unavailable { .. }
        ));
        comparison.validate()?;
        Ok(())
    }

    #[test]
    fn wrong_source_projection_proof_and_missing_affected_identity_fail_closed()
    -> Result<(), Box<dyn Error>> {
        let selected = DomainAssignment {
            id: DomainAssignmentId::new("tests.selected-pair")?,
            entity: entity("selected-person")?,
            value: AssignmentValue::Boolean(true),
            evidence: Vec::new(),
        };
        let base = source(7, 50, vec![selected])?;
        let candidate = source(8, 20, Vec::new())?;
        let mut wrong = comparable(&base, &candidate)?;
        if let Some((before, _)) = &mut wrong.scoring_contexts {
            before.source = &candidate;
        }
        assert!(matches!(
            prepare_comparison_v2(&base, &candidate, None)?.finish(wrong),
            Err(ComparisonError::InvalidComparison(
                DomainContractError::InvalidExplanationContract("comparison scoring source")
            ))
        ));
        let mut comparison = prepare_comparison_v2(&base, &candidate, None)?
            .finish(comparable(&base, &candidate)?)?;
        comparison.runs.base.outcome = candidate.run_manifest.outcome.clone();
        assert!(matches!(
            comparison.validate(),
            Err(DomainContractError::InvalidExplanationContract(
                "comparison accepted run binding"
            ))
        ));
        comparison.runs.base.outcome = base.run_manifest.outcome.clone();
        comparison.affected_entities.clear();
        assert!(matches!(
            comparison.validate(),
            Err(DomainContractError::InvalidExplanationContract(
                "missing comparison affected entity"
            ))
        ));
        Ok(())
    }

    #[test]
    fn scoring_facts_are_bound_to_exact_run_input_and_manifest_not_only_result()
    -> Result<(), Box<dyn Error>> {
        let base = source(7, 0, Vec::new())?;
        let input = run_input_for(
            &base.accepted_result,
            run_id(0x99)?,
            base.run_input.snapshot_document_hash.clone(),
            base.run_input.model_hash.clone(),
        )?;
        let changed_input = PortableAcceptedResult::new(
            input.clone(),
            accepted_manifest(&input, &base.accepted_result)?,
            base.accepted_result.clone(),
            BTreeMap::new(),
        )?;
        let changed_manifest = PortableAcceptedResult::new(
            base.run_input.clone(),
            manifest_for(
                base.run_input.run_id,
                base.run_input.checksum.clone(),
                accepted_outcome(&base.accepted_result),
                Some(DurationMillis::new(600)?),
                Vec::new(),
            )?,
            base.accepted_result.clone(),
            BTreeMap::new(),
        )?;
        for changed in [&changed_input, &changed_manifest] {
            assert_eq!(changed.accepted_result, base.accepted_result);
            changed.validate()?;
            assert!(matches!(
                prepare_comparison_v2(&base, changed, None)?.finish(comparable(&base, &base)?),
                Err(ComparisonError::InvalidComparison(
                    DomainContractError::InvalidExplanationContract("comparison scoring source")
                ))
            ));
        }
        Ok(())
    }

    #[test]
    fn typed_sources_cannot_hide_excessive_unchanged_evidence_behind_a_small_diff()
    -> Result<(), Box<dyn Error>> {
        let mut assignments = Vec::with_capacity(100_000);
        for index in 0..100_000 {
            assignments.push(DomainAssignment {
                id: DomainAssignmentId::new(format!("p.x{index:05}"))?,
                entity: DomainEntityRef {
                    kind: DomainEntityKindId::new("p.p")?,
                    id: DomainEntityId::new("p.p")?,
                },
                value: AssignmentValue::Boolean(true),
                evidence: vec![DomainEvidenceId::new("e.p")?, DomainEvidenceId::new("e.q")?],
            });
        }
        let oversized = source(7, 0, assignments)?;
        assert!(matches!(
            prepare_comparison_v2(&oversized, &oversized, None),
            Err(ComparisonError::InvalidAcceptedResult {
                side: ComparisonSide::Base,
                source: DomainContractError::PortableJsonViolation(_),
            })
        ));
        Ok(())
    }

    #[test]
    fn established_legacy_semantics_are_distinct_from_an_unknown_projection()
    -> Result<(), Box<dyn Error>> {
        let legacy = PortableAcceptedResult::from_json(include_bytes!(
            "fixtures/portable_accepted_v2.json"
        ))?;
        let unknown = prepare_comparison_v2(&legacy, &legacy, None)?
            .finish(ComparisonEnrichmentV2::default())?;
        assert_eq!(
            unknown.score,
            ScoreComparisonV2::Unavailable {
                reason: ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch,
            }
        );
        let established =
            prepare_comparison_v2(&legacy, &legacy, None)?.finish(comparable(&legacy, &legacy)?)?;
        let ScoreComparisonV2::Comparable { score_levels, .. } = established.score else {
            return Err("established historical semantics were discarded".into());
        };
        assert!(score_levels.iter().all(|level| level.delta == 0));
        assert!(legacy.run_input.objective_context.is_none());
        Ok(())
    }

    #[test]
    fn legacy_absence_does_not_become_current_authored_comparability() -> Result<(), Box<dyn Error>>
    {
        let legacy = PortableAcceptedResult::from_json(include_bytes!(
            "fixtures/portable_accepted_v2.json"
        ))?;
        let current = source(7, 4, Vec::new())?;
        let comparison = prepare_comparison_v2(&legacy, &current, None)?
            .finish(comparable(&legacy, &current)?)?;
        assert_eq!(
            comparison.score,
            ScoreComparisonV2::Unavailable {
                reason: ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch,
            }
        );
        assert_eq!(legacy.run_input.schema_version, 1);
        assert!(legacy.run_input.objective_context.is_none());
        assert_eq!(
            current
                .run_input
                .objective_context
                .as_ref()
                .map(|context| context.profile_id.as_str()),
            Some(AUTHORED_OBJECTIVE_PROFILE_ID)
        );
        Ok(())
    }

    #[test]
    fn protection_observations_reject_changed_and_unchanged_source_contradictions()
    -> Result<(), Box<dyn Error>> {
        let assignment = DomainAssignment {
            id: DomainAssignmentId::new("tests.protected-pair")?,
            entity: entity("protected-person")?,
            value: AssignmentValue::Boolean(true),
            evidence: Vec::new(),
        };
        let base = source(7, 0, vec![assignment.clone()])?;
        for (value, observed) in [
            (AssignmentValue::Boolean(false), true),
            (AssignmentValue::Boolean(true), false),
            (AssignmentValue::Integer(1), true),
        ] {
            let mut after = assignment.clone();
            after.value = value;
            let candidate = source(8, 0, vec![after])?;
            let protection = AssignmentProtectionComparisonV1::new(
                assignment.id.clone(),
                Some(AssignmentProtectionStateV1::Soft { weight: 1 }),
                Some(AssignmentProtectionStateV1::Soft { weight: 1 }),
                true,
                observed,
            )?;
            assert!(matches!(
                prepare_comparison_v2(&base, &candidate, None)?.finish(ComparisonEnrichmentV2 {
                    protections: vec![protection],
                    ..ComparisonEnrichmentV2::default()
                }),
                Err(ComparisonError::InvalidComparison(
                    DomainContractError::InvalidExplanationContract(
                        "assignment protection selection binding"
                    )
                ))
            ));
        }
        Ok(())
    }

    #[test]
    fn wire_protections_reject_boolean_and_non_boolean_delta_contradictions()
    -> Result<(), Box<dyn Error>> {
        let before = DomainAssignment {
            id: DomainAssignmentId::new("tests.protected-pair")?,
            entity: entity("protected-person")?,
            value: AssignmentValue::Boolean(true),
            evidence: Vec::new(),
        };
        let base = source(7, 0, vec![before.clone()])?;
        for value in [AssignmentValue::Boolean(false), AssignmentValue::Integer(1)] {
            let mut after = before.clone();
            after.value = value;
            let candidate = source(8, 0, vec![after])?;
            let mut comparison = prepare_comparison_v2(&base, &candidate, None)?
                .finish(ComparisonEnrichmentV2::default())?;
            comparison
                .protections
                .push(AssignmentProtectionComparisonV1::new(
                    before.id.clone(),
                    Some(AssignmentProtectionStateV1::Soft { weight: 1 }),
                    Some(AssignmentProtectionStateV1::Soft { weight: 1 }),
                    true,
                    true,
                )?);
            assert!(matches!(
                comparison.validate(),
                Err(DomainContractError::InvalidExplanationContract(
                    "assignment protection selection binding"
                ))
            ));
        }
        Ok(())
    }

    #[test]
    fn aggregate_parser_budget_rejects_oversized_producer_evidence() -> Result<(), Box<dyn Error>> {
        let base = source(7, 0, Vec::new())?;
        let candidate = source(8, 0, Vec::new())?;
        let mut enrichment = ComparisonEnrichmentV2::default();
        for index in 0..48_000 {
            enrichment
                .protections
                .push(AssignmentProtectionComparisonV1::new(
                    DomainAssignmentId::new(format!("p.x{index:05}"))?,
                    Some(AssignmentProtectionStateV1::Soft { weight: 1 }),
                    Some(AssignmentProtectionStateV1::Soft { weight: 2 }),
                    false,
                    false,
                )?);
            enrichment
                .affected_parties
                .push(ComparisonEntityObservationV1 {
                    entity: DomainEntityRef {
                        kind: DomainEntityKindId::new("p.p")?,
                        id: DomainEntityId::new(format!("p.x{index:05}"))?,
                    },
                    before_label: Some("P".to_owned()),
                    after_label: Some("P".to_owned()),
                });
        }
        assert!(matches!(
            prepare_comparison_v2(&base, &candidate, None)?.finish(enrichment),
            Err(ComparisonError::InvalidComparison(
                DomainContractError::PortableJsonViolation(_)
            ))
        ));
        Ok(())
    }

    #[test]
    fn inherited_cancellation_survives_the_pack_enrichment_boundary() -> Result<(), Box<dyn Error>>
    {
        let base = source(7, 50, Vec::new())?;
        let candidate = source(8, 20, Vec::new())?;
        let cancellation = eutheto_types::CancellationToken::new();
        let control = eutheto_types::OperationControl::Cancellation(cancellation.clone());
        let prepared = prepare_comparison_v2(&base, &candidate, Some(&control))?;
        cancellation.cancel();
        assert!(matches!(
            prepared.finish(comparable(&base, &candidate)?),
            Err(ComparisonError::Interrupted(
                eutheto_types::OperationInterruption::Cancelled
            ))
        ));
        assert!(matches!(
            prepare_comparison_v2(&base, &candidate, Some(&control)),
            Err(ComparisonError::Interrupted(
                eutheto_types::OperationInterruption::Cancelled
            ))
        ));
        Ok(())
    }
}
