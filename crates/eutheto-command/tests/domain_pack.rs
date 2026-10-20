use eutheto_command::{OFFICIAL_TEST_PACK_ID, OfficialTestPack};
use eutheto_domain_api::{
    CompileContext, ContractJsonLimits, CounterfactualCompileContext, DOMAIN_BATCH_SCHEMA_VERSION,
    DomainBatchCommand, DomainPack, DomainPackError, DomainPackRegistry, DomainShareResult,
    InfeasibilityCompileContext, PortableImportContext, ShareResultOptions,
    validate_contract_value,
};
use eutheto_domain_ir::{
    AcceptedResult, AssignmentValue, ComparisonEnrichmentV2, CounterfactualConditionPayloadV1,
    CounterfactualConditionV1, DomainAssignment, DomainAssignmentId, DomainEntityId,
    DomainEntityKindId, DomainEntityRef, EvidenceRenderRequestV2, ExplanationCapability,
    ExplanationEvidencePayloadV2, ExplanationEvidenceV2, ExplanationKind,
    ExplanationValidationSeverity, NormalizedSolution, ObjectiveProfileId,
    OptimalityStatusEvidence, PortableAcceptedResult, RepairCausalityV1, RepairEvidenceV2,
    RunInput, RunManifest, RunObjectiveContext, RunObjectiveOperation, RunPhaseTimingsV1,
    RunTerminalOutcomeV1, SolutionComparisonV2, ValidationIssueEvidenceV1, VerificationContextV1,
    VerificationFactId, VerificationIssueId, VerificationValue, VerifiedCheckpoint, blake3_hex,
    prepare_comparison_v2,
};
use eutheto_planning_ir::{
    CandidateValues, Capability, ComparisonOp, Constraint, MetadataKey, PlanningIrLimitsV1,
    PlanningProblem, ProjectionExpression, ProvenanceParameter, ProvenanceSourceKind, Variable,
    canonical_ir_hash, summarize, validate,
};
use eutheto_types::{
    AssignmentId, CancellationToken, DomainCommandEnvelope, DurationMillis, EntityId,
    FixedMonotonicClock, PackId, ParentSolveBudget, PortableDomainDocument, RequestId, RuleId,
    ScenarioDocument, ScenarioSnapshotId, SolutionId, SolveRunId, SolveStatus,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

const SCENARIO_ID: &str = "0195a5e4-7c00-7000-8000-000000000001";
const ENTITY_ID: &str = "018f25a7-8b3c-7d11-8000-000000000001";
const SOLUTION_ID: &str = "0195a5e4-7c00-7000-8000-000000000002";
const COMMAND_ID: &str = "official.test.configure_entity";

fn document() -> Result<ScenarioDocument, serde_json::Error> {
    serde_json::from_value(json!({
        "format": "eutheto/scenario",
        "formatVersion": 1,
        "scenarioId": SCENARIO_ID,
        "domainPack": { "id": OFFICIAL_TEST_PACK_ID, "schemaVersion": 1 },
        "metadata": {
            "title": "Pack conformance",
            "description": "",
            "createdAt": "2026-08-29T12:00:00Z",
            "updatedAt": "2026-08-29T12:00:00Z"
        },
        "settings": {
            "timeZone": "UTC",
            "locale": "en-US",
            "units": "metric",
            "horizon": {
                "start": "2026-08-29T12:00:00Z",
                "end": "2026-09-05T12:00:00Z"
            },
            "gapPolicy": "reject",
            "overlapPolicy": "earlier"
        },
        "domain": {
            "entities": {
                (ENTITY_ID): { "id": ENTITY_ID, "enabled": true, "target": 3 }
            },
            "rules": {},
            "preferences": {},
            "lockedAssignments": {}
        },
        "extensions": {}
    }))
}

fn command(enabled: bool, target: i64) -> DomainCommandEnvelope {
    DomainCommandEnvelope {
        command_type: COMMAND_ID.to_owned(),
        payload: json!({ "entityId": ENTITY_ID, "enabled": enabled, "target": target }),
    }
}

fn batch(commands: Vec<DomainCommandEnvelope>) -> Result<DomainBatchCommand, Box<dyn Error>> {
    Ok(DomainBatchCommand {
        schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: PackId::new(OFFICIAL_TEST_PACK_ID)?,
        scenario_schema_version: 1,
        label: Some("Conformance batch".to_owned()),
        commands,
    })
}

fn authored_objective_context() -> Result<RunObjectiveContext, Box<dyn Error>> {
    Ok(RunObjectiveContext::authored(PackId::new(
        OFFICIAL_TEST_PACK_ID,
    )?)?)
}

fn context() -> Result<CompileContext, Box<dyn Error>> {
    Ok(CompileContext {
        objective_context: Some(authored_objective_context()?),
        scenario_revision: 7,
        semantic_metadata: BTreeMap::new(),
        control: eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    })
}

fn optimality_status_request(
    source: &ScenarioDocument,
    objective_context: Option<RunObjectiveContext>,
) -> Result<EvidenceRenderRequestV2, Box<dyn Error>> {
    let checkpoint = VerifiedCheckpoint::from_json(include_bytes!(
        "../../eutheto-domain-ir/tests/fixtures/verified_checkpoint_v1.json"
    ))?;
    let template = checkpoint.run_input;
    let run_input = RunInput::new(
        template.run_id,
        template.request_id,
        source.scenario_id,
        template.scenario_revision,
        template.snapshot_id,
        template.snapshot_document_hash,
        template.snapshot_created_at,
        source.domain_pack.id.clone(),
        source.domain_pack.schema_version,
        template.planning_ir_schema_version,
        template.compiler_version,
        template.application_version,
        template.backend_id,
        template.backend_version,
        template.adapter_version,
        template.worker_version,
        template.solver_version,
        template.protocol_major,
        template.protocol_minor,
        template.model_hash,
        template.objective_policy_hash,
        template.solve_options,
        template.scenario_timezone,
        template.temporary_condition_hash,
        objective_context,
    )?;
    let run_manifest = RunManifest::new(
        run_input.run_id,
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
    Ok(EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::OptimalityStatus {
            status: Box::new(OptimalityStatusEvidence {
                schema_version: Some(2),
                run_input,
                run_manifest,
                result: None,
            }),
        },
    )?)?)
}

fn verification_context(
    pack: &dyn DomainPack,
    document: &ScenarioDocument,
    problem: &PlanningProblem,
    solution: &NormalizedSolution,
) -> Result<VerificationContextV1, Box<dyn Error>> {
    let scope = pack.verification_scope(
        document,
        solution.scenario_revision,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    Ok(VerificationContextV1::new(
        document.scenario_id,
        solution.scenario_revision,
        blake3_hex(&serde_json::to_vec(document)?),
        canonical_ir_hash(problem, PlanningIrLimitsV1::DEFAULT)?,
        solution.canonical_hash()?,
        scope.checksum,
    )?)
}
#[allow(clippy::too_many_arguments)]
fn accepted_source(
    document: &ScenarioDocument,
    revision: u64,
    solution_id: &str,
    run_ids: (&str, &str, &str),
    target: i64,
    compiled_context: Option<RunObjectiveContext>,
    recorded_context: Option<RunObjectiveContext>,
    pack_schema_version: u32,
) -> Result<PortableAcceptedResult, Box<dyn Error>> {
    let pack = OfficialTestPack;
    let compilation = CompileContext {
        scenario_revision: revision,
        objective_context: compiled_context,
        ..context()?
    };
    let objective_context = compilation.objective_context.clone();
    let problem = pack.compile(document, &compilation)?;
    let solution = pack.project(
        &problem,
        &satisfying_candidate_for_target(&problem, target),
        SolutionId::from_str(solution_id)?,
        &compilation.control,
    )?;
    let score = pack.score(
        document,
        &solution,
        objective_context.as_ref(),
        &compilation.control,
    )?;
    let verification_context = verification_context(&pack, document, &problem, &solution)?;
    let verification = pack.verify(
        document,
        &solution,
        objective_context.as_ref(),
        &verification_context,
        &score,
        &compilation.control,
    )?;
    let accepted = AcceptedResult::new(solution, verification)?;
    let template = VerifiedCheckpoint::from_json(include_bytes!(
        "../../eutheto-domain-ir/tests/fixtures/verified_checkpoint_v1.json"
    ))?
    .run_input;
    let run_input = RunInput::new(
        SolveRunId::from_str(run_ids.0)?,
        RequestId::from_str(run_ids.1)?,
        document.scenario_id,
        revision,
        ScenarioSnapshotId::from_str(run_ids.2)?,
        blake3_hex(&serde_json::to_vec(document)?),
        template.snapshot_created_at,
        document.domain_pack.id.clone(),
        pack_schema_version,
        template.planning_ir_schema_version,
        template.compiler_version,
        template.application_version,
        template.backend_id,
        template.backend_version,
        template.adapter_version,
        template.worker_version,
        template.solver_version,
        template.protocol_major,
        template.protocol_minor,
        verification_context.planning_model_hash,
        template.objective_policy_hash,
        template.solve_options,
        template.scenario_timezone,
        template.temporary_condition_hash,
        recorded_context,
    )?;
    let run_manifest = RunManifest::new(
        run_input.run_id,
        run_input.checksum.clone(),
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
        "2026-09-03T12:00:01Z".parse()?,
        "2026-09-03T12:00:02Z".parse()?,
        Some(DurationMillis::new(1_000)?),
        None,
        Some(DurationMillis::new(1_000)?),
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    Ok(PortableAcceptedResult::new(
        run_input,
        run_manifest,
        accepted,
        BTreeMap::new(),
    )?)
}

fn repair_request(
    comparison: SolutionComparisonV2,
) -> Result<EvidenceRenderRequestV2, Box<dyn Error>> {
    Ok(EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Repair {
            repair: Box::new(RepairEvidenceV2 {
                comparison,
                causality: RepairCausalityV1::NotEstablished,
            }),
        },
    )?)?)
}

#[test]
fn descriptor_declares_exact_explanation_capabilities() -> Result<(), Box<dyn Error>> {
    let descriptor = OfficialTestPack.descriptor()?;
    let expected: BTreeSet<_> = [
        ExplanationCapability::Validation,
        ExplanationCapability::Infeasibility,
        ExplanationCapability::Assignment,
        ExplanationCapability::Counterfactual,
        ExplanationCapability::SolutionDifference,
        ExplanationCapability::Repair,
        ExplanationCapability::OptimalityStatus,
    ]
    .into_iter()
    .collect();
    assert_eq!(descriptor.explanation_capabilities, expected);
    assert_eq!(
        serde_json::to_value(&descriptor)?["explanationCapabilities"],
        json!([
            "validation",
            "infeasibility",
            "assignment",
            "counterfactual",
            "solutionDifference",
            "repair",
            "optimalityStatus"
        ])
    );
    Ok(())
}
#[test]
fn rendering_validates_and_returns_typed_message_parameters() -> Result<(), Box<dyn Error>> {
    let evidence = ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Validation {
        issue: ValidationIssueEvidenceV1 {
            issue_id: VerificationIssueId::new("official.test.issue.validation")?,
            severity: ExplanationValidationSeverity::MustFix,
            message_key: "official.test.validation.target".to_owned(),
            parameters: BTreeMap::new(),
            field_path: None,
            entity: None,
            rule_id: None,
        },
    })?;
    let request = EvidenceRenderRequestV2::new(evidence)?;
    let objective_context = authored_objective_context()?;
    let rendered = OfficialTestPack.render_evidence(
        &document()?,
        &request,
        Some(&objective_context),
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_eq!(rendered.kind, request.kind);
    assert_eq!(rendered.messages.len(), 1);
    assert_eq!(
        rendered.messages[0].message_key,
        "official.test.explanation.validation"
    );
    assert!(
        rendered.messages[0]
            .parameters
            .values()
            .all(|value| matches!(value, VerificationValue::Text(_)))
    );
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn synthetic_pack_requires_complete_sources_for_repair() -> Result<(), Box<dyn Error>> {
    let base_document = document()?;
    let mut candidate_document = base_document.clone();
    candidate_document
        .domain
        .entities
        .get_mut(&EntityId::from_str(ENTITY_ID)?)
        .ok_or("candidate entity is missing")?["target"] = json!(4);
    let base = accepted_source(
        &base_document,
        7,
        SOLUTION_ID,
        (
            "0195a5e4-7c00-7000-8000-000000000011",
            "0195a5e4-7c00-7000-8000-000000000012",
            "0195a5e4-7c00-7000-8000-000000000013",
        ),
        3,
        None,
        None,
        1,
    )?;
    let candidate = accepted_source(
        &candidate_document,
        8,
        "0195a5e4-7c00-7000-8000-000000000014",
        (
            "0195a5e4-7c00-7000-8000-000000000015",
            "0195a5e4-7c00-7000-8000-000000000016",
            "0195a5e4-7c00-7000-8000-000000000017",
        ),
        4,
        Some(authored_objective_context()?),
        Some(authored_objective_context()?),
        1,
    )?;
    let comparison =
        prepare_comparison_v2(&base, &candidate, None)?.finish(ComparisonEnrichmentV2::default())?;
    let control = eutheto_types::OperationControl::Cancellation(CancellationToken::new());
    let difference_request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::SolutionDifference {
            comparison: Box::new(comparison.clone()),
        },
    )?)?;
    let difference =
        OfficialTestPack.render_evidence(&base_document, &difference_request, None, &control)?;
    assert_eq!(difference.kind, ExplanationKind::SolutionDifference);
    assert_eq!(
        difference.messages[0].message_key,
        "official.test.explanation.solution_difference"
    );

    let request = repair_request(comparison)?;
    assert!(matches!(
        OfficialTestPack.render_evidence(&base_document, &request, None, &control),
        Err(DomainPackError::UnsupportedExplanationCapability(
            ExplanationCapability::Repair
        ))
    ));
    let rendered = OfficialTestPack.render_repair_evidence(
        &base_document,
        &base,
        &candidate_document,
        &candidate,
        &request,
        &control,
    )?;
    rendered.validate()?;
    let summary = rendered
        .messages
        .first()
        .ok_or("Repair summary is missing")?;
    assert_eq!(
        summary.parameters.get(&VerificationFactId::new(
            "official.test.fact.baseline_admission"
        )?),
        Some(&VerificationValue::Text("notEstablished".to_owned()))
    );
    let fact = rendered
        .messages
        .iter()
        .find(|message| message.message_key == "official.test.verify.required_target")
        .ok_or("the baseline selection's failed current rule was not rendered")?;
    assert_eq!(
        fact.parameters.get(&VerificationFactId::new(
            "expected.official.test.fact.target"
        )?),
        Some(&VerificationValue::Integer(4))
    );
    assert_eq!(
        fact.parameters.get(&VerificationFactId::new(
            "observed.official.test.fact.target"
        )?),
        Some(&VerificationValue::Integer(3))
    );
    assert_eq!(
        fact.parameters
            .get(&VerificationFactId::new("official.test.fact.repair_frame")?),
        Some(&VerificationValue::Text(
            "acceptedBaseToCandidate".to_owned()
        ))
    );
    assert_eq!(
        fact.assignments,
        vec![DomainAssignmentId::new(format!(
            "official.test.assignment.target.{ENTITY_ID}"
        ))?]
    );
    Ok(())
}

#[test]
fn repair_rejects_a_resealed_comparison_body_with_unchanged_source_bindings()
-> Result<(), Box<dyn Error>> {
    let base_document = document()?;
    let mut candidate_document = base_document.clone();
    candidate_document
        .domain
        .entities
        .get_mut(&EntityId::from_str(ENTITY_ID)?)
        .ok_or("candidate entity is missing")?["target"] = json!(4);
    let base = accepted_source(
        &base_document,
        7,
        SOLUTION_ID,
        (
            "0195a5e4-7c00-7000-8000-000000000021",
            "0195a5e4-7c00-7000-8000-000000000022",
            "0195a5e4-7c00-7000-8000-000000000023",
        ),
        3,
        None,
        None,
        1,
    )?;
    let candidate = accepted_source(
        &candidate_document,
        8,
        "0195a5e4-7c00-7000-8000-000000000024",
        (
            "0195a5e4-7c00-7000-8000-000000000025",
            "0195a5e4-7c00-7000-8000-000000000026",
            "0195a5e4-7c00-7000-8000-000000000027",
        ),
        4,
        Some(authored_objective_context()?),
        Some(authored_objective_context()?),
        1,
    )?;
    let mut body = serde_json::to_value(
        prepare_comparison_v2(&base, &candidate, None)?.finish(ComparisonEnrichmentV2::default())?,
    )?;
    let first_change = body["assignments"]
        .as_array_mut()
        .and_then(|changes| changes.first_mut())
        .ok_or("comparison has no changed assignments")?;
    if first_change["type"] != "changed" {
        return Err("comparison fixture did not produce a changed assignment".into());
    }
    first_change["after"]["value"]["value"] = json!(5);
    body.as_object_mut()
        .ok_or("comparison must serialize as an object")?
        .remove("checksum")
        .ok_or("comparison checksum is missing")?;
    let checksum = blake3_hex(&serde_json::to_vec(&body)?);
    body.as_object_mut()
        .ok_or("comparison must serialize as an object")?
        .insert("checksum".to_owned(), Value::String(checksum));
    let tampered = serde_json::from_value::<SolutionComparisonV2>(body)?;
    let request = repair_request(tampered)?;
    let control = eutheto_types::OperationControl::Cancellation(CancellationToken::new());
    assert!(matches!(
        OfficialTestPack.render_repair_evidence(
            &base_document,
            &base,
            &candidate_document,
            &candidate,
            &request,
            &control,
        ),
        Err(DomainPackError::Contract(message))
            if message.contains("does not match its complete accepted sources")
    ));
    Ok(())
}

#[test]
fn repair_rejects_pack_schema_and_unsupported_objective_contexts() -> Result<(), Box<dyn Error>> {
    let base_document = document()?;
    let mut candidate_document = base_document.clone();
    candidate_document
        .domain
        .entities
        .get_mut(&EntityId::from_str(ENTITY_ID)?)
        .ok_or("candidate entity is missing")?["target"] = json!(4);
    let base = accepted_source(
        &base_document,
        7,
        SOLUTION_ID,
        (
            "0195a5e4-7c00-7000-8000-000000000031",
            "0195a5e4-7c00-7000-8000-000000000032",
            "0195a5e4-7c00-7000-8000-000000000033",
        ),
        3,
        None,
        None,
        1,
    )?;
    let control = eutheto_types::OperationControl::Cancellation(CancellationToken::new());

    let wrong_schema = accepted_source(
        &candidate_document,
        8,
        "0195a5e4-7c00-7000-8000-000000000034",
        (
            "0195a5e4-7c00-7000-8000-000000000035",
            "0195a5e4-7c00-7000-8000-000000000036",
            "0195a5e4-7c00-7000-8000-000000000037",
        ),
        4,
        Some(authored_objective_context()?),
        Some(authored_objective_context()?),
        2,
    )?;
    let wrong_schema_request = repair_request(
        prepare_comparison_v2(&base, &wrong_schema, None)?
            .finish(ComparisonEnrichmentV2::default())?,
    )?;
    assert!(matches!(
        OfficialTestPack.render_repair_evidence(
            &base_document,
            &base,
            &candidate_document,
            &wrong_schema,
            &wrong_schema_request,
            &control,
        ),
        Err(DomainPackError::Contract(_))
    ));

    let mut unsupported_context = authored_objective_context()?;
    unsupported_context.profile_id = ObjectiveProfileId::new("official.test.objective.other")?;
    let unsupported = accepted_source(
        &candidate_document,
        8,
        "0195a5e4-7c00-7000-8000-000000000038",
        (
            "0195a5e4-7c00-7000-8000-000000000039",
            "0195a5e4-7c00-7000-8000-000000000040",
            "0195a5e4-7c00-7000-8000-000000000041",
        ),
        4,
        Some(authored_objective_context()?),
        Some(unsupported_context),
        1,
    )?;
    let unsupported_request = repair_request(
        prepare_comparison_v2(&base, &unsupported, None)?
            .finish(ComparisonEnrichmentV2::default())?,
    )?;
    assert!(matches!(
        OfficialTestPack.render_repair_evidence(
            &base_document,
            &base,
            &candidate_document,
            &unsupported,
            &unsupported_request,
            &control,
        ),
        Err(DomainPackError::Contract(message))
            if message.contains("unsupported objective context")
    ));
    Ok(())
}

#[test]
fn repair_reports_removed_historical_endpoints_as_unavailable() -> Result<(), Box<dyn Error>> {
    let base_document = document()?;
    let mut candidate_document = base_document.clone();
    candidate_document.domain.entities.clear();
    let base = accepted_source(
        &base_document,
        7,
        SOLUTION_ID,
        (
            "0195a5e4-7c00-7000-8000-000000000051",
            "0195a5e4-7c00-7000-8000-000000000052",
            "0195a5e4-7c00-7000-8000-000000000053",
        ),
        3,
        None,
        None,
        1,
    )?;
    let candidate = accepted_source(
        &candidate_document,
        8,
        "0195a5e4-7c00-7000-8000-000000000054",
        (
            "0195a5e4-7c00-7000-8000-000000000055",
            "0195a5e4-7c00-7000-8000-000000000056",
            "0195a5e4-7c00-7000-8000-000000000057",
        ),
        3,
        Some(authored_objective_context()?),
        Some(authored_objective_context()?),
        1,
    )?;
    let request = repair_request(
        prepare_comparison_v2(&base, &candidate, None)?.finish(ComparisonEnrichmentV2::default())?,
    )?;
    let control = eutheto_types::OperationControl::Cancellation(CancellationToken::new());
    let rendered = OfficialTestPack.render_repair_evidence(
        &base_document,
        &base,
        &candidate_document,
        &candidate,
        &request,
        &control,
    )?;
    assert_eq!(rendered.messages.len(), 3);
    let unavailable: BTreeSet<String> = rendered.messages[1..]
        .iter()
        .map(|message| message.assignments[0].as_str().to_owned())
        .collect();
    assert_eq!(
        unavailable,
        [
            format!("official.test.assignment.enabled.{ENTITY_ID}"),
            format!("official.test.assignment.target.{ENTITY_ID}"),
        ]
        .into_iter()
        .collect()
    );
    for message in rendered.messages.iter().skip(1) {
        assert_eq!(
            message.message_key,
            "official.test.explanation.repair.endpoint_unavailable"
        );
        assert_eq!(
            message.entities[0].id.as_str(),
            format!("official.test.entity.{ENTITY_ID}").as_str()
        );
        assert_eq!(
            message.parameters.get(&VerificationFactId::new(
                "official.test.fact.current_endpoint"
            )?),
            Some(&VerificationValue::Text("unavailable".to_owned()))
        );
        assert_eq!(
            message.parameters.get(&VerificationFactId::new(
                "official.test.fact.endpoint_reason"
            )?),
            Some(&VerificationValue::Text(
                "entityAbsentFromCandidateDocument".to_owned()
            ))
        );
    }
    rendered.validate()?;
    Ok(())
}

#[test]
fn ordinary_verification_still_rejects_unknown_assignment_endpoints() -> Result<(), Box<dyn Error>>
{
    let document = document()?;
    let objective_context = authored_objective_context()?;
    let source = accepted_source(
        &document,
        7,
        SOLUTION_ID,
        (
            "0195a5e4-7c00-7000-8000-000000000061",
            "0195a5e4-7c00-7000-8000-000000000062",
            "0195a5e4-7c00-7000-8000-000000000063",
        ),
        3,
        Some(objective_context.clone()),
        Some(objective_context.clone()),
        1,
    )?;
    let mut solution = source.accepted_result.solution.clone();
    let unknown = "018f25a7-8b3c-7d11-8000-000000000099";
    solution.assignments.push(DomainAssignment {
        id: DomainAssignmentId::new(format!("official.test.assignment.enabled.{unknown}"))?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new("official.test.entity")?,
            id: DomainEntityId::new(format!("official.test.entity.{unknown}"))?,
        },
        value: AssignmentValue::Boolean(true),
        evidence: Vec::new(),
    });
    solution.canonicalize()?;
    let pack = OfficialTestPack;
    let compile_context = CompileContext {
        scenario_revision: solution.scenario_revision,
        ..context()?
    };
    let problem = pack.compile(&document, &compile_context)?;
    let verification_context = verification_context(&pack, &document, &problem, &solution)?;
    assert!(matches!(
        pack.verify(
            &document,
            &solution,
            Some(&objective_context),
            &verification_context,
            &source.accepted_result.verification.score,
            &compile_context.control,
        ),
        Err(DomainPackError::Contract(message))
            if message.contains("solution contains an unknown domain assignment")
    ));
    Ok(())
}

#[test]
fn registry_rejects_duplicates_and_missing_metadata() -> Result<(), Box<dyn Error>> {
    let duplicate = DomainPackRegistry::builder()
        .register(OfficialTestPack)
        .register(OfficialTestPack)
        .build();
    assert!(matches!(duplicate, Err(DomainPackError::DuplicatePack(_))));

    let mut descriptor = DomainPack::descriptor(&OfficialTestPack)?;
    descriptor.display_name.default_text.clear();
    assert!(matches!(
        descriptor.validate(),
        Err(DomainPackError::MissingMetadata(_))
    ));
    Ok(())
}

#[test]
fn catalog_rejects_internal_schemas_incompatible_with_domain_storage() -> Result<(), Box<dyn Error>>
{
    let catalog = OfficialTestPack.catalog()?;
    catalog.validate()?;
    validate_contract_value(
        &catalog.internal_schema,
        &serde_json::to_value(document()?.domain)?,
        ContractJsonLimits::DEFAULT,
    )?;

    let mut missing_map = catalog.internal_schema.clone();
    missing_map["properties"]
        .as_object_mut()
        .ok_or("schema properties must be an object")?
        .remove("lockedAssignments");
    missing_map["required"]
        .as_array_mut()
        .ok_or("schema required must be an array")?
        .retain(|field| field.as_str() != Some("lockedAssignments"));
    let mut array_map = catalog.internal_schema.clone();
    array_map["properties"]["entities"] = json!({"type": "array"});
    let mut unrestricted_records = catalog.internal_schema.clone();
    unrestricted_records["properties"]["entities"]["additionalProperties"] = json!(true);
    let mut array_records = catalog.internal_schema.clone();
    array_records["properties"]["entities"]["additionalProperties"] = json!({"type": "array"});
    let mut overridden_record = catalog.internal_schema.clone();
    overridden_record["properties"]["entities"]["properties"] =
        json!({"00000000-0000-7000-8000-000000000001": {"type": "array"}});
    let mut host_field = catalog.internal_schema.clone();
    host_field["properties"]["schemaVersion"] = json!({"const": 1});
    host_field["required"]
        .as_array_mut()
        .ok_or("schema required must be an array")?
        .push(json!("schemaVersion"));
    for schema in [
        missing_map,
        array_map,
        unrestricted_records,
        array_records,
        overridden_record,
        host_field,
    ] {
        let mut incompatible = catalog.clone();
        incompatible.internal_schema = schema;
        assert!(matches!(
            incompatible.validate(),
            Err(DomainPackError::CatalogMismatch(_))
        ));
    }
    Ok(())
}

#[test]
fn command_validation_rejects_unknown_version_command_and_field_atomically()
-> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let source = document()?;

    let mut unknown_version = batch(vec![command(true, 3)])?;
    unknown_version.scenario_schema_version = 2;
    assert!(matches!(
        pack.apply_batch(&source, &unknown_version, &CancellationToken::new()),
        Err(DomainPackError::UnsupportedVersion(2))
    ));

    let unknown_command = DomainCommandEnvelope {
        command_type: "official.test.future".to_owned(),
        payload: json!({}),
    };
    assert!(matches!(
        pack.apply_batch(
            &source,
            &batch(vec![unknown_command])?,
            &CancellationToken::new()
        ),
        Err(DomainPackError::UnknownCommand(_))
    ));

    let invalid_field = DomainCommandEnvelope {
        command_type: COMMAND_ID.to_owned(),
        payload: json!({
            "entityId": ENTITY_ID,
            "enabled": true,
            "target": 3,
            "future": true
        }),
    };
    let two = batch(vec![command(true, 3), invalid_field])?;
    assert!(matches!(
        pack.apply_batch(&source, &two, &CancellationToken::new()),
        Err(DomainPackError::InvalidPayload { .. })
    ));
    assert_eq!(source, document()?);
    Ok(())
}

#[test]
fn batch_is_atomic_and_inverse_restores_exact_document() -> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let source = document()?;
    let applied = pack.apply_batch(
        &source,
        &batch(vec![command(true, 3), command(true, 4)])?,
        &CancellationToken::new(),
    )?;
    assert_eq!(applied.results.len(), 2);
    assert_eq!(applied.changes.len(), 2);
    assert_eq!(applied.inverse.commands.len(), 2);
    let restored = pack.apply_batch(
        &applied.document,
        &applied.inverse,
        &CancellationToken::new(),
    )?;
    assert_eq!(restored.document, source);
    Ok(())
}

#[test]
fn portable_v2_preserves_all_editable_records_without_solver_validation()
-> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let mut source = document()?;
    source.domain.entities.insert(
        EntityId::from_str(ENTITY_ID)?,
        json!({"name": "Unfinished generic entity", "details": {"values": [1, true, null]}}),
    );
    source.domain.rules.insert(
        RuleId::from_str("0195a5e4-7c00-7000-8000-000000000003")?,
        json!({"label": "Unfinished rule"}),
    );
    source.domain.preferences.insert(
        RuleId::from_str("0195a5e4-7c00-7000-8000-000000000004")?,
        json!({"label": "Unfinished preference"}),
    );
    source.domain.locked_assignments.insert(
        AssignmentId::from_str("0195a5e4-7c00-7000-8000-000000000005")?,
        json!({"entityId": ENTITY_ID, "notes": ["preserve"]}),
    );
    source.extensions.insert(
        "vendor.example".to_owned(),
        json!({"nested": {"value": "preserve"}, "array": [false, 1]}),
    );
    let portable = pack.export_portable(&source)?;
    let mut shell = document()?;
    shell.domain = eutheto_types::ScenarioDomain::default();
    shell.extensions.clear();
    let imported = pack.import_portable(
        &portable,
        &PortableImportContext {
            scenario_shell: shell,
        },
    )?;
    assert_eq!(imported, source);
    Ok(())
}

#[test]
fn portable_and_share_contracts_round_trip() -> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let mut source = document()?;
    let portable = pack.export_portable(&source)?;
    let serialized = serde_json::to_vec(&portable)?;
    let decoded = serde_json::from_slice(&serialized)?;
    let imported = pack.import_portable(
        &decoded,
        &PortableImportContext {
            scenario_shell: source.clone(),
        },
    )?;
    assert_eq!(imported.domain, source.domain);
    let migrated = pack.migrate_portable_step(PortableDomainDocument {
        pack_id: PackId::new(OFFICIAL_TEST_PACK_ID)?,
        schema_version: 0,
        required_capabilities: BTreeSet::default(),
        payload: json!({
            "schemaVersion": 0,
            "entities": {
                (ENTITY_ID): { "enabled": true, "target": 3 }
            }
        }),
    })?;
    assert_eq!(migrated.schema_version, 1);
    assert_eq!(migrated.payload["schemaVersion"], 1);
    let migrated = pack.migrate_portable_step(migrated)?;
    assert_eq!(migrated.payload, portable.payload);

    let compile_context = context()?;
    let objective_context = compile_context.objective_context.as_ref();
    let problem = pack.compile(&source, &compile_context)?;
    let mut candidate = CandidateValues::default();
    for variable in &problem.variables {
        match variable {
            Variable::Boolean(value) => {
                candidate.booleans.insert(value.id.clone(), true);
            }
            Variable::Integer(value) => {
                candidate.integers.insert(value.id.clone(), 3);
            }
            Variable::Interval(_) => {}
        }
    }
    let solution = pack.project(
        &problem,
        &candidate,
        SolutionId::from_str(SOLUTION_ID)?,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let authoritative_score = pack.score(
        &source,
        &solution,
        objective_context,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let verification_context = verification_context(&pack, &source, &problem, &solution)?;
    let verification = pack.verify(
        &source,
        &solution,
        objective_context,
        &verification_context,
        &authoritative_score,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let accepted = AcceptedResult::new(solution, verification)?;
    source.extensions.insert(
        "nonsemantic.source-only".to_owned(),
        json!({"private": "must-not-be-shared"}),
    );
    let shared = pack.build_share_result(
        &source,
        &accepted,
        objective_context,
        ShareResultOptions::default(),
    )?;
    let share_bytes = serde_json::to_vec(&shared)?;
    let share_round_trip: DomainShareResult = serde_json::from_slice(&share_bytes)?;
    assert_eq!(share_round_trip, shared);
    validate_contract_value(
        &pack.catalog()?.share_result_schema,
        &shared.payload,
        ContractJsonLimits::DEFAULT,
    )?;
    assert!(shared.payload.get("source").is_none());
    assert!(shared.payload.get("extensions").is_none());
    assert!(!serde_json::to_string(&shared.payload)?.contains("must-not-be-shared"));
    let mut unaccepted = accepted.clone();
    unaccepted.verification.accepted = false;
    assert!(
        pack.build_share_result(
            &source,
            &unaccepted,
            objective_context,
            ShareResultOptions::default(),
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn portable_nonsemantic_extensions_survive_current_and_historical_round_trips()
-> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let extensions = BTreeMap::from([
        (
            "nonsemantic.example.alpha".to_owned(),
            json!({
                "nested": [1, "two", true, null],
                "object": {"z": 3, "a": "exact"}
            }),
        ),
        (
            "nonsemantic.example.empty".to_owned(),
            json!({"array": [], "object": {}}),
        ),
    ]);
    let mut source = document()?;
    source.extensions = extensions.clone();

    let current = pack.export_portable(&source)?;
    assert_eq!(
        current.payload["extensions"],
        serde_json::to_value(&extensions)?
    );
    let imported = pack.import_portable(
        &current,
        &PortableImportContext {
            scenario_shell: document()?,
        },
    )?;
    assert_eq!(imported.domain, source.domain);
    assert_eq!(imported.extensions, extensions);
    assert_eq!(pack.export_portable(&imported)?.payload, current.payload);

    let migrated = pack.migrate_portable_step(PortableDomainDocument {
        pack_id: PackId::new(OFFICIAL_TEST_PACK_ID)?,
        schema_version: 0,
        required_capabilities: BTreeSet::default(),
        payload: json!({
            "schemaVersion": 0,
            "entities": {
                (ENTITY_ID): { "enabled": true, "target": 3 }
            },
            "extensions": extensions
        }),
    })?;
    let migrated = pack.migrate_portable_step(migrated)?;
    assert_eq!(
        migrated.payload["extensions"],
        current.payload["extensions"]
    );
    let migrated_import = pack.import_portable(
        &migrated,
        &PortableImportContext {
            scenario_shell: document()?,
        },
    )?;
    assert_eq!(migrated_import.domain, source.domain);
    assert_eq!(migrated_import.extensions, source.extensions);
    assert_eq!(
        pack.export_portable(&migrated_import)?.payload,
        migrated.payload
    );
    Ok(())
}

#[test]
fn historical_portable_semantic_extensions_are_rejected() -> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    assert!(matches!(
        pack.migrate_portable_step(PortableDomainDocument {
            pack_id: PackId::new(OFFICIAL_TEST_PACK_ID)?,
            schema_version: 0,
            required_capabilities: BTreeSet::default(),
            payload: json!({
                "schemaVersion": 0,
                "entities": {
                    (ENTITY_ID): { "enabled": true, "target": 3 }
                },
                "extensions": {
                    "semantic.required-rule": {"meaning": "must not be ignored"}
                }
            }),
        }),
        Err(DomainPackError::Contract(_))
    ));
    Ok(())
}

fn satisfying_candidate(problem: &PlanningProblem) -> CandidateValues {
    satisfying_candidate_for_target(problem, 3)
}

fn satisfying_candidate_for_target(problem: &PlanningProblem, target: i64) -> CandidateValues {
    let mut candidate = CandidateValues::default();
    for variable in &problem.variables {
        match variable {
            Variable::Boolean(value) => {
                candidate.booleans.insert(value.id.clone(), true);
            }
            Variable::Integer(value) => {
                candidate.integers.insert(value.id.clone(), target);
            }
            Variable::Interval(_) => {}
        }
    }
    candidate
}

#[test]
// This contract test keeps compilation, projection, verification, and score determinism together.
#[allow(clippy::too_many_lines)]
fn compile_project_verify_and_score_are_deterministic() -> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let source = document()?;
    let limits = PlanningIrLimitsV1::DEFAULT;
    let verification_scope = pack.verification_scope(
        &source,
        context()?.scenario_revision,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_eq!(
        verification_scope,
        pack.verification_scope(&source, context()?.scenario_revision, &context()?.control)?
    );
    assert_eq!(verification_scope.required_rules.len(), 1);
    assert_eq!(
        verification_scope.required_rules[0].rule_id,
        RuleId::from_uuid(EntityId::from_str(ENTITY_ID)?.as_uuid())
    );
    let compile_context = context()?;
    let objective_context = compile_context.objective_context.as_ref();
    let first = pack.compile(&source, &compile_context)?;
    let second = pack.compile(&source, &context()?)?;
    validate(&first, limits)?;
    validate(&second, limits)?;
    let first_summary = summarize(&first, limits)?;
    let second_summary = summarize(&second, limits)?;
    assert_eq!(first, second);
    assert_eq!(first_summary, second_summary);
    assert_eq!(first.variables.len(), 2);
    assert_eq!(first.constraints.len(), 3);
    assert_eq!(first.projections.len(), 2);
    assert_eq!(first.provenance.len(), 4);
    assert_eq!(first_summary.bool_variable_count, 1);
    assert_eq!(first_summary.int_variable_count, 1);
    assert_eq!(first_summary.constraint_count, 3);
    assert_eq!(first_summary.objective_term_count, 1);
    assert_eq!(first_summary.projection_count, 2);
    assert_eq!(first_summary.provenance_count, 4);
    let required_provenance = first
        .provenance
        .iter()
        .find(|record| record.source_kind == ProvenanceSourceKind::RequiredRule)
        .ok_or("required-rule provenance missing")?;
    assert_eq!(
        required_provenance.source_id,
        verification_scope.required_rules[0].rule_id.to_string()
    );
    assert!(
        first_summary
            .manifest
            .required_capabilities()
            .is_subset(&first.declared_capabilities)
    );
    assert!(
        first
            .declared_capabilities
            .contains(&Capability::ForbiddenTable)
    );
    assert_eq!(first.objectives.levels[0].lower_bound, 0);
    assert_eq!(first.objectives.levels[0].upper_bound, 10);
    let mut renamed_source = source.clone();
    renamed_source.metadata.title = "Renamed display-only scenario".to_owned();
    let renamed = pack.compile(&renamed_source, &context()?)?;
    assert_eq!(
        summarize(&renamed, limits)?.canonical_ir_hash,
        first_summary.canonical_ir_hash
    );

    let mut display_variant = first.clone();
    display_variant.metadata.display_text.insert(
        "official.test.display.label".to_owned(),
        "A different localized display label".to_owned(),
    );
    validate(&display_variant, limits)?;
    assert_eq!(
        summarize(&display_variant, limits)?.canonical_ir_hash,
        first_summary.canonical_ir_hash
    );

    let candidate = satisfying_candidate(&first);
    let solution = pack.project(
        &first,
        &candidate,
        SolutionId::from_str(SOLUTION_ID)?,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let projection_evidence = format!("official_test.projection.{ENTITY_ID}");
    assert!(solution.assignments.iter().all(|assignment| {
        assignment.evidence.len() == 1 && assignment.evidence[0].as_str() == projection_evidence
    }));
    let authoritative_score = pack.score(
        &source,
        &solution,
        objective_context,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let context_for_solution = verification_context(&pack, &source, &first, &solution)?;
    let mut unsatisfied = solution.clone();
    let target = unsatisfied
        .assignments
        .iter_mut()
        .find(|assignment| assignment.id.as_str().contains(".target."))
        .ok_or("target assignment missing")?;
    target.value = AssignmentValue::Integer(4);
    let unsatisfied_score = pack.score(
        &source,
        &unsatisfied,
        objective_context,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let unsatisfied_context = verification_context(&pack, &source, &first, &unsatisfied)?;
    let report = pack.verify(
        &source,
        &solution,
        objective_context,
        &context_for_solution,
        &authoritative_score,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert!(report.accepted);
    assert_eq!(report.required_rule_results.len(), 1);
    assert!(
        report
            .required_rule_results
            .iter()
            .all(|evaluation| evaluation.satisfied)
    );
    assert_eq!(authoritative_score.feasibility, 0);
    assert_eq!(authoritative_score.levels[0].value, 3);
    assert_eq!(report.score, authoritative_score);
    assert_eq!(
        report.verification_scope_checksum,
        verification_scope.checksum
    );
    assert_eq!(
        report.required_rule_results[0].rule_id,
        verification_scope.required_rules[0].rule_id
    );
    assert_eq!(report.required_rule_results[0].evidence.len(), 1);
    assert_eq!(
        report.required_rule_results[0].evidence[0].as_str(),
        format!("official_test.rule.{ENTITY_ID}")
    );
    assert_eq!(
        report.required_rule_results[0].evidence[0].as_str(),
        required_provenance.id.as_str()
    );
    assert_eq!(
        report,
        pack.verify(
            &source,
            &solution,
            objective_context,
            &context_for_solution,
            &report.score,
            &context()?.control
        )?
    );

    let unsatisfied_report = pack.verify(
        &source,
        &unsatisfied,
        objective_context,
        &unsatisfied_context,
        &unsatisfied_score,
        &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert!(!unsatisfied_report.accepted);
    assert_eq!(unsatisfied_report.required_rule_results.len(), 1);
    assert!(!unsatisfied_report.required_rule_results[0].satisfied);
    assert!(unsatisfied_report.warnings.is_empty());
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn direct_pack_paths_reject_unimplemented_objective_contexts() -> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let source = document()?;
    let compile_context = context()?;
    let objective_context = compile_context
        .objective_context
        .as_ref()
        .ok_or("current compile context is missing its objective")?;
    let problem = pack.compile(&source, &compile_context)?;
    let solution = pack.project(
        &problem,
        &satisfying_candidate(&problem),
        SolutionId::from_str(SOLUTION_ID)?,
        &compile_context.control,
    )?;
    let authoritative_score = pack.score(
        &source,
        &solution,
        Some(objective_context),
        &compile_context.control,
    )?;
    let verification_context = verification_context(&pack, &source, &problem, &solution)?;
    let report = pack.verify(
        &source,
        &solution,
        Some(objective_context),
        &verification_context,
        &authoritative_score,
        &compile_context.control,
    )?;
    let accepted = AcceptedResult::new(solution.clone(), report)?;

    let mut unimplemented = authored_objective_context()?;
    unimplemented.profile_id = ObjectiveProfileId::new("official.test.objective.other")?;
    assert!(
        unimplemented
            .validate_for(&source.domain_pack.id, source.scenario_id)
            .is_ok()
    );
    let unsupported_compile_context = CompileContext {
        objective_context: Some(unimplemented.clone()),
        ..compile_context.clone()
    };
    assert!(matches!(
        pack.compile(&source, &unsupported_compile_context),
        Err(DomainPackError::Contract(_))
    ));

    let mut malformed_contexts = Vec::new();
    let mut wrong_pack = authored_objective_context()?;
    wrong_pack.pack_id = PackId::new("official.test.other")?;
    malformed_contexts.push(wrong_pack);
    let mut wrong_schema = authored_objective_context()?;
    wrong_schema.schema_version += 1;
    malformed_contexts.push(wrong_schema);
    let mut wrong_semantics = authored_objective_context()?;
    wrong_semantics.semantic_version += 1;
    malformed_contexts.push(wrong_semantics);
    let mut repair = authored_objective_context()?;
    repair.operation = RunObjectiveOperation::Repair;
    malformed_contexts.push(repair);
    for invalid in malformed_contexts {
        let invalid_compile_context = CompileContext {
            objective_context: Some(invalid),
            ..compile_context.clone()
        };
        assert!(pack.compile(&source, &invalid_compile_context).is_err());
    }

    let projection = problem
        .projections
        .iter()
        .find(|projection| matches!(&projection.expression, ProjectionExpression::Boolean(_)))
        .ok_or("Boolean projection missing")?;
    let condition =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: projection.assignment_id.clone(),
            value: AssignmentValue::Boolean(false),
        })?;
    let budget = parent_budget(
        1_000,
        Arc::new(FixedMonotonicClock::default()),
        CancellationToken::new(),
    )?;
    assert!(matches!(
        pack.compile_counterfactual(
            &source,
            &condition,
            &CounterfactualCompileContext {
                base_problem: &problem,
                compile_context: &unsupported_compile_context,
                budget: budget.phase_view(),
            },
        ),
        Err(DomainPackError::Contract(_))
    ));
    assert!(matches!(
        pack.compile_infeasibility(
            &source,
            &InfeasibilityCompileContext {
                base_problem: &problem,
                compile_context: &unsupported_compile_context,
                condition: None,
                budget: budget.phase_view(),
            },
        ),
        Err(DomainPackError::Contract(_))
    ));

    let control = eutheto_types::OperationControl::Cancellation(CancellationToken::new());
    assert!(matches!(
        pack.score(&source, &solution, Some(&unimplemented), &control),
        Err(DomainPackError::Contract(_))
    ));
    assert!(matches!(
        pack.verify(
            &source,
            &solution,
            Some(&unimplemented),
            &verification_context,
            &authoritative_score,
            &control,
        ),
        Err(DomainPackError::Contract(_))
    ));
    assert!(matches!(
        pack.build_share_result(
            &source,
            &accepted,
            Some(&unimplemented),
            ShareResultOptions::default(),
        ),
        Err(DomainPackError::Contract(_))
    ));

    let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Validation {
            issue: ValidationIssueEvidenceV1 {
                issue_id: VerificationIssueId::new("official.test.issue.validation")?,
                severity: ExplanationValidationSeverity::MustFix,
                message_key: "official.test.validation.target".to_owned(),
                parameters: BTreeMap::new(),
                field_path: None,
                entity: None,
                rule_id: None,
            },
        },
    )?)?;
    assert!(matches!(
        pack.render_evidence(&source, &request, Some(&unimplemented), &control,),
        Err(DomainPackError::Contract(_))
    ));
    let query = eutheto_domain_api::DomainSetupQueryV1 {
        schema_version: 1,
        view_id: "official.test.result.summary".to_owned(),
        parameters: json!({}),
        continuation: None,
    };
    assert!(matches!(
        pack.build_view(
            eutheto_domain_api::DomainViewInput::AcceptedSolution {
                document: &source,
                solution: &solution,
                objective_context: Some(&unimplemented),
                view_id: &query.view_id,
                query: &query,
                context: eutheto_domain_api::SetupViewContext {
                    revision: eutheto_types::Revision::INITIAL,
                    query_fingerprint: [0; 32],
                },
            },
            &control,
        ),
        Err(DomainPackError::Contract(_))
    ));
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn legacy_none_context_preserves_model_and_report_semantics() -> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let source = document()?;
    let current_context = context()?;
    let objective_context = current_context
        .objective_context
        .as_ref()
        .ok_or("current compile context is missing its objective")?;
    let legacy_context = CompileContext {
        objective_context: None,
        ..current_context.clone()
    };
    let current_problem = pack.compile(&source, &current_context)?;
    let legacy_problem = pack.compile(&source, &legacy_context)?;
    assert_eq!(legacy_problem, current_problem);

    let solution = pack.project(
        &current_problem,
        &satisfying_candidate(&current_problem),
        SolutionId::from_str(SOLUTION_ID)?,
        &current_context.control,
    )?;
    let current_score = pack.score(
        &source,
        &solution,
        Some(objective_context),
        &current_context.control,
    )?;
    let legacy_score = pack.score(&source, &solution, None, &current_context.control)?;
    assert_eq!(legacy_score, current_score);
    let verification_context = verification_context(&pack, &source, &current_problem, &solution)?;
    let current_report = pack.verify(
        &source,
        &solution,
        Some(objective_context),
        &verification_context,
        &current_score,
        &current_context.control,
    )?;
    let legacy_report = pack.verify(
        &source,
        &solution,
        None,
        &verification_context,
        &legacy_score,
        &current_context.control,
    )?;
    assert_eq!(legacy_report, current_report);
    let accepted = AcceptedResult::new(solution.clone(), legacy_report)?;
    assert_eq!(
        pack.build_share_result(&source, &accepted, None, ShareResultOptions::default(),)?,
        pack.build_share_result(
            &source,
            &accepted,
            Some(objective_context),
            ShareResultOptions::default(),
        )?
    );

    let legacy_request = optimality_status_request(&source, None)?;
    let current_request = optimality_status_request(&source, Some((*objective_context).clone()))?;
    let legacy_rendered =
        pack.render_evidence(&source, &legacy_request, None, &current_context.control)?;
    assert_eq!(legacy_rendered.kind, legacy_request.kind);
    assert_eq!(
        legacy_rendered.evidence_checksum,
        legacy_request.evidence.checksum
    );
    let current_rendered = pack.render_evidence(
        &source,
        &current_request,
        Some(objective_context),
        &current_context.control,
    )?;
    assert_eq!(current_rendered.kind, current_request.kind);
    assert_eq!(
        current_rendered.evidence_checksum,
        current_request.evidence.checksum
    );
    assert!(matches!(
        pack.render_evidence(&source, &current_request, None, &current_context.control,),
        Err(DomainPackError::Contract(_))
    ));
    assert!(matches!(
        pack.render_evidence(
            &source,
            &legacy_request,
            Some(objective_context),
            &current_context.control,
        ),
        Err(DomainPackError::Contract(_))
    ));

    let query = eutheto_domain_api::DomainSetupQueryV1 {
        schema_version: 1,
        view_id: "official.test.result.summary".to_owned(),
        parameters: json!({}),
        continuation: None,
    };
    let view_context = eutheto_domain_api::SetupViewContext {
        revision: eutheto_types::Revision::INITIAL,
        query_fingerprint: [0; 32],
    };
    let current_view = pack.build_view(
        eutheto_domain_api::DomainViewInput::AcceptedSolution {
            document: &source,
            solution: &solution,
            objective_context: Some(objective_context),
            view_id: &query.view_id,
            query: &query,
            context: view_context,
        },
        &current_context.control,
    )?;
    let legacy_view = pack.build_view(
        eutheto_domain_api::DomainViewInput::AcceptedSolution {
            document: &source,
            solution: &solution,
            objective_context: None,
            view_id: &query.view_id,
            query: &query,
            context: view_context,
        },
        &current_context.control,
    )?;
    assert_eq!(legacy_view.view, current_view.view);

    let projection = current_problem
        .projections
        .iter()
        .find(|projection| matches!(&projection.expression, ProjectionExpression::Boolean(_)))
        .ok_or("Boolean projection missing")?;
    let condition =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: projection.assignment_id.clone(),
            value: AssignmentValue::Boolean(false),
        })?;
    let budget = parent_budget(
        1_000,
        Arc::new(FixedMonotonicClock::default()),
        CancellationToken::new(),
    )?;
    assert!(matches!(
        pack.compile_infeasibility(
            &source,
            &InfeasibilityCompileContext {
                base_problem: &current_problem,
                compile_context: &legacy_context,
                condition: None,
                budget: budget.phase_view(),
            },
        ),
        Err(DomainPackError::UnsupportedDomainCapability(
            eutheto_domain_api::DomainCapability::InfeasibilityDiagnostics
        ))
    ));

    assert_eq!(
        pack.compile_counterfactual(
            &source,
            &condition,
            &CounterfactualCompileContext {
                base_problem: &current_problem,
                compile_context: &legacy_context,
                budget: budget.phase_view(),
            },
        )?,
        pack.compile_counterfactual(
            &source,
            &condition,
            &CounterfactualCompileContext {
                base_problem: &current_problem,
                compile_context: &current_context,
                budget: budget.phase_view(),
            },
        )?
    );
    Ok(())
}

#[test]
fn unknown_internal_and_portable_fields_are_rejected() -> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let mut source = document()?;
    source.domain.entities.insert(
        EntityId::from_str(ENTITY_ID)?,
        json!({ "id": ENTITY_ID, "enabled": true, "target": 3, "future": true }),
    );
    assert!(!pack.validate_fast(&source).issues.is_empty());
    assert!(pack.compile(&source, &context()?).is_err());

    let clean = document()?;
    let mut portable = pack.export_portable(&clean)?;
    portable.payload["future"] = Value::Bool(true);
    assert!(
        pack.import_portable(
            &portable,
            &PortableImportContext {
                scenario_shell: clean
            }
        )
        .is_err()
    );
    Ok(())
}

fn parent_budget(
    milliseconds: u64,
    clock: Arc<FixedMonotonicClock>,
    cancellation: CancellationToken,
) -> Result<ParentSolveBudget, Box<dyn Error>> {
    Ok(ParentSolveBudget::new(
        DurationMillis::new(milliseconds)?,
        clock,
        cancellation,
    )?)
}

#[test]
#[allow(clippy::too_many_lines)]
fn counterfactual_compilation_adds_only_typed_condition_semantics() -> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let source = document()?;
    let compile_context = context()?;
    let base = pack.compile(&source, &compile_context)?;
    let bool_projection = base
        .projections
        .iter()
        .find(|projection| matches!(&projection.expression, ProjectionExpression::Boolean(_)))
        .ok_or("Boolean projection missing")?;
    let int_projection = base
        .projections
        .iter()
        .find(|projection| matches!(&projection.expression, ProjectionExpression::Integer(_)))
        .ok_or("integer projection missing")?;
    let clock = Arc::new(FixedMonotonicClock::default());
    let budget = parent_budget(1_000, clock, CancellationToken::new())?;
    let compile_counterfactual = |condition: &CounterfactualConditionV1| {
        pack.compile_counterfactual(
            &source,
            condition,
            &CounterfactualCompileContext {
                base_problem: &base,
                compile_context: &compile_context,
                budget: budget.phase_view(),
            },
        )
    };

    let force_boolean =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: bool_projection.assignment_id.clone(),
            value: AssignmentValue::Boolean(false),
        })?;
    let forced = compile_counterfactual(&force_boolean)?;
    validate(&forced, compile_context.planning_limits)?;
    assert_eq!(
        forced.metadata.compile_metadata.len(),
        base.metadata.compile_metadata.len() + 1
    );
    assert!(
        base.metadata
            .compile_metadata
            .iter()
            .all(|(key, value)| forced.metadata.compile_metadata.get(key) == Some(value))
    );
    assert_eq!(forced.variables, base.variables);
    assert_eq!(forced.objectives, base.objectives);
    assert_eq!(forced.assumptions, base.assumptions);
    assert_eq!(forced.projections, base.projections);
    assert_eq!(forced.constraints.len(), base.constraints.len() + 1);
    assert_eq!(forced.provenance.len(), base.provenance.len() + 1);
    assert!(
        base.constraints
            .iter()
            .all(|constraint| forced.constraints.contains(constraint))
    );
    assert!(
        base.provenance
            .iter()
            .all(|provenance| forced.provenance.contains(provenance))
    );
    assert_eq!(
        forced
            .metadata
            .compile_metadata
            .get(&MetadataKey::new("eutheto.counterfactual.condition_hash")?),
        Some(&ProvenanceParameter::Text(force_boolean.checksum.clone()))
    );
    let forced_constraint = forced
        .constraints
        .iter()
        .find(|constraint| !base.constraints.contains(constraint))
        .ok_or("counterfactual constraint missing")?;
    let Constraint::BoolAnd { literals } = &forced_constraint.body else {
        return Err("Boolean force did not add a Boolean condition".into());
    };
    assert_eq!(literals.len(), 1);
    assert!(!literals[0].positive);
    let derived_provenance = forced
        .provenance
        .iter()
        .find(|record| !base.provenance.contains(record))
        .ok_or("counterfactual provenance missing")?;
    assert_eq!(
        derived_provenance.source_kind,
        ProvenanceSourceKind::Derived
    );
    assert_eq!(
        derived_provenance.parent.as_ref(),
        Some(&bool_projection.provenance)
    );
    assert_eq!(forced_constraint.provenance, derived_provenance.id);

    let forbid_boolean =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id: bool_projection.assignment_id.clone(),
            value: AssignmentValue::Boolean(false),
        })?;
    let forbidden = compile_counterfactual(&forbid_boolean)?;
    let forbidden_constraint = forbidden
        .constraints
        .iter()
        .find(|constraint| !base.constraints.contains(constraint))
        .ok_or("Boolean forbid constraint missing")?;
    let Constraint::BoolAnd { literals } = &forbidden_constraint.body else {
        return Err("Boolean forbid did not add a Boolean condition".into());
    };
    assert_eq!(literals.len(), 1);
    assert!(literals[0].positive);

    let force_integer =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: int_projection.assignment_id.clone(),
            value: AssignmentValue::Integer(4),
        })?;
    let integer = compile_counterfactual(&force_integer)?;
    let integer_constraint = integer
        .constraints
        .iter()
        .find(|constraint| !base.constraints.contains(constraint))
        .ok_or("integer force constraint missing")?;
    let Constraint::LinearComparison(comparison) = &integer_constraint.body else {
        return Err("integer force did not add an integer condition".into());
    };
    assert_eq!(comparison.op, ComparisonOp::Equal);
    assert_eq!(comparison.rhs, 4);
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn counterfactual_compilation_rejects_mismatch_and_unsupported_conditions()
-> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let source = document()?;
    let compile_context = context()?;
    let base = pack.compile(&source, &compile_context)?;
    let bool_projection = base
        .projections
        .iter()
        .find(|projection| matches!(&projection.expression, ProjectionExpression::Boolean(_)))
        .ok_or("Boolean projection missing")?;
    let int_projection = base
        .projections
        .iter()
        .find(|projection| matches!(&projection.expression, ProjectionExpression::Integer(_)))
        .ok_or("integer projection missing")?;
    let condition = |payload| CounterfactualConditionV1::new(payload);
    let compile = |base_problem: &PlanningProblem, condition: &CounterfactualConditionV1| {
        let clock = Arc::new(FixedMonotonicClock::default());
        let budget = parent_budget(1_000, clock, CancellationToken::new())?;
        Ok::<_, Box<dyn Error>>(pack.compile_counterfactual(
            &source,
            condition,
            &CounterfactualCompileContext {
                base_problem,
                compile_context: &compile_context,
                budget: budget.phase_view(),
            },
        ))
    };

    let integer_forbid = condition(CounterfactualConditionPayloadV1::ForbidAssignmentValue {
        assignment_id: int_projection.assignment_id.clone(),
        value: AssignmentValue::Integer(3),
    })?;
    let integer_forbidden = compile(&base, &integer_forbid)??;
    validate(&integer_forbidden, compile_context.planning_limits)?;
    assert_eq!(
        integer_forbidden
            .metadata
            .compile_metadata
            .get(&MetadataKey::new("eutheto.counterfactual.condition_hash")?),
        Some(&ProvenanceParameter::Text(integer_forbid.checksum.clone()))
    );
    let forbidden_constraint = integer_forbidden
        .constraints
        .iter()
        .find(|constraint| !base.constraints.contains(constraint))
        .ok_or("integer forbid constraint missing")?;
    let Constraint::ForbiddenTable { variables, rows } = &forbidden_constraint.body else {
        return Err("integer forbid did not add a forbidden table".into());
    };
    let ProjectionExpression::Integer(projected_variable) = &int_projection.expression else {
        return Err("integer projection changed kind".into());
    };
    assert_eq!(variables, &vec![projected_variable.clone()]);
    assert_eq!(rows, &vec![vec![3]]);
    let forbidden_provenance = integer_forbidden
        .provenance
        .iter()
        .find(|record| !base.provenance.contains(record))
        .ok_or("integer forbid provenance missing")?;
    assert_eq!(forbidden_constraint.provenance, forbidden_provenance.id);
    assert_eq!(
        forbidden_provenance.parent.as_ref(),
        Some(&int_projection.provenance)
    );

    let wrong_value = condition(CounterfactualConditionPayloadV1::ForceAssignmentValue {
        assignment_id: bool_projection.assignment_id.clone(),
        value: AssignmentValue::Integer(1),
    })?;
    assert_eq!(
        compile(&base, &wrong_value)?,
        Err(DomainPackError::InvalidPayload {
            path: "/condition/value".to_owned(),
            message: "condition value kind does not match the assignment projection".to_owned(),
        })
    );

    let unknown_projection = condition(CounterfactualConditionPayloadV1::ForceAssignmentValue {
        assignment_id: DomainAssignmentId::new("official.test.assignment.unknown")?,
        value: AssignmentValue::Boolean(true),
    })?;
    assert_eq!(
        compile(&base, &unknown_projection)?,
        Err(DomainPackError::InvalidPayload {
            path: "/condition/assignmentId".to_owned(),
            message: "assignment projection is not supported by official.test".to_owned(),
        })
    );

    let mut unsupported_base = base.clone();
    unsupported_base
        .projections
        .iter_mut()
        .find(|projection| projection.assignment_id == bool_projection.assignment_id)
        .ok_or("Boolean projection missing from clone")?
        .expression = ProjectionExpression::Constant(AssignmentValue::Boolean(true));
    unsupported_base.canonicalize()?;
    let unsupported_kind = condition(CounterfactualConditionPayloadV1::ForceAssignmentValue {
        assignment_id: bool_projection.assignment_id.clone(),
        value: AssignmentValue::Boolean(true),
    })?;
    assert_eq!(
        compile(&unsupported_base, &unsupported_kind)?,
        Err(DomainPackError::InvalidPayload {
            path: "/condition/assignmentId".to_owned(),
            message: "projection kind is not supported by official.test counterfactual compilation"
                .to_owned(),
        })
    );

    let mut mismatched_base = base.clone();
    mismatched_base.metadata.compiler_version = "mismatched-test-compiler".to_owned();
    let mismatch = condition(CounterfactualConditionPayloadV1::ForceAssignmentValue {
        assignment_id: bool_projection.assignment_id.clone(),
        value: AssignmentValue::Boolean(true),
    })?;
    assert_eq!(
        compile(&mismatched_base, &mismatch)?,
        Err(DomainPackError::InvalidPayload {
            path: "/context/baseProblem".to_owned(),
            message: "base planning problem does not match deterministic recompilation".to_owned(),
        })
    );
    Ok(())
}

#[test]
fn counterfactual_compilation_observes_shared_cancellation_and_expiry() -> Result<(), Box<dyn Error>>
{
    let pack = OfficialTestPack;
    let source = document()?;
    let compile_context = context()?;
    let base = pack.compile(&source, &compile_context)?;
    let assignment_id = base
        .projections
        .iter()
        .find(|projection| matches!(&projection.expression, ProjectionExpression::Boolean(_)))
        .ok_or("Boolean projection missing")?
        .assignment_id
        .clone();
    let condition =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(true),
        })?;

    let cancellation = CancellationToken::new();
    let cancelled_clock = Arc::new(FixedMonotonicClock::default());
    let cancelled_budget = parent_budget(1_000, cancelled_clock, cancellation.clone())?;
    cancellation.cancel();
    assert_eq!(
        pack.compile_counterfactual(
            &source,
            &condition,
            &CounterfactualCompileContext {
                base_problem: &base,
                compile_context: &compile_context,
                budget: cancelled_budget.phase_view(),
            },
        ),
        Err(DomainPackError::Cancelled)
    );

    let expired_clock = Arc::new(FixedMonotonicClock::default());
    let expired_budget = parent_budget(1, Arc::clone(&expired_clock), CancellationToken::new())?;
    expired_clock.advance(Duration::from_millis(1))?;
    assert_eq!(
        pack.compile_counterfactual(
            &source,
            &condition,
            &CounterfactualCompileContext {
                base_problem: &base,
                compile_context: &compile_context,
                budget: expired_budget.phase_view(),
            },
        ),
        Err(DomainPackError::BudgetExpired)
    );
    Ok(())
}

#[test]
fn accepted_result_views_cannot_be_requested_with_setup_subjects() -> Result<(), Box<dyn Error>> {
    let document = document()?;
    let query = eutheto_domain_api::DomainSetupQueryV1 {
        schema_version: 1,
        view_id: "official.test.result.summary".to_owned(),
        parameters: json!({}),
        continuation: None,
    };
    let context = eutheto_domain_api::SetupViewContext {
        revision: eutheto_types::Revision::INITIAL,
        query_fingerprint: [0; 32],
    };
    let command = eutheto_types::ScenarioCommand::SetScenarioSettings(Box::new(
        eutheto_types::SetScenarioSettings {
            settings: document.settings.clone(),
            restoration: None,
        },
    ));
    for input in [
        eutheto_domain_api::DomainViewInput::StoredSetup {
            document: &document,
            query: &query,
            context,
        },
        eutheto_domain_api::DomainViewInput::CommandPreviewSetup {
            original: &document,
            prospective: &document,
            command: &command,
            changes: &[],
            query: &query,
            context,
        },
    ] {
        assert!(matches!(
            OfficialTestPack.build_view(
                input,
                &eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
            ),
            Err(DomainPackError::InvalidPayload { path, .. }) if path == "/source"
        ));
    }
    Ok(())
}

#[test]
fn catalog_cannot_advertise_a_mutation_identifier_as_read_only() -> Result<(), Box<dyn Error>> {
    let mut catalog = OfficialTestPack.catalog()?;
    let command = &catalog.commands[0];
    catalog
        .setup_queries
        .push(eutheto_domain_api::SetupQueryDescriptor {
            id: command.id.clone(),
            title: command.title.clone(),
            description: command.description.clone(),
            sources: [eutheto_domain_api::SetupQuerySource::Stored]
                .into_iter()
                .collect(),
            supports_continuation: false,
            parameter_schema: command.payload_schema.clone(),
            result_schema: command.result_schema.clone(),
            valid_examples: command.valid_examples.clone(),
            invalid_examples: command.invalid_examples.clone(),
        });
    assert!(catalog.validate().is_err());
    Ok(())
}
