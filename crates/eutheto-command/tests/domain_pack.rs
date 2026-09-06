use eutheto_command::{OFFICIAL_TEST_PACK_ID, OfficialTestPack};
use eutheto_domain_api::{
    CompileContext, ContractJsonLimits, CounterfactualCompileContext, DOMAIN_BATCH_SCHEMA_VERSION,
    DomainBatchCommand, DomainPack, DomainPackError, DomainPackRegistry, DomainShareResult,
    PortableImportContext, ShareResultOptions, validate_contract_value,
};
use eutheto_domain_ir::{
    AcceptedResult, AssignmentValue, CounterfactualConditionPayloadV1, CounterfactualConditionV1,
    DomainAssignmentId, EvidenceRenderRequestV1, ExplanationCapability,
    ExplanationEvidencePayloadV1, ExplanationEvidenceV1, ExplanationValidationSeverity,
    NormalizedSolution, ValidationIssueEvidenceV1, VerificationContextV1, VerificationIssueId,
    VerificationValue, blake3_hex,
};
use eutheto_planning_ir::{
    CandidateValues, Capability, ComparisonOp, Constraint, MetadataKey, PlanningIrLimitsV1,
    PlanningProblem, ProjectionExpression, ProvenanceParameter, ProvenanceSourceKind, Variable,
    canonical_ir_hash, summarize, validate,
};
use eutheto_types::{
    AssignmentId, CancellationToken, DomainCommandEnvelope, DurationMillis, EntityId,
    FixedMonotonicClock, PackId, ParentSolveBudget, PortableDomainDocument, RuleId,
    ScenarioDocument, SolutionId,
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

fn context() -> CompileContext {
    CompileContext {
        scenario_revision: 7,
        semantic_metadata: BTreeMap::new(),
        cancellation: CancellationToken::new(),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    }
}
fn verification_context(
    pack: &dyn DomainPack,
    document: &ScenarioDocument,
    problem: &PlanningProblem,
    solution: &NormalizedSolution,
) -> Result<VerificationContextV1, Box<dyn Error>> {
    let scope = pack.verification_scope(document, solution.scenario_revision)?;
    Ok(VerificationContextV1::new(
        document.scenario_id,
        solution.scenario_revision,
        blake3_hex(&serde_json::to_vec(document)?),
        canonical_ir_hash(problem, PlanningIrLimitsV1::DEFAULT)?,
        solution.canonical_hash()?,
        scope.checksum,
    )?)
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
    let evidence = ExplanationEvidenceV1::new(ExplanationEvidencePayloadV1::Validation {
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
    let request = EvidenceRenderRequestV1::new(evidence)?;
    let rendered = OfficialTestPack.render_evidence(&document()?, &request)?;
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
        pack.apply_batch(&source, &unknown_version),
        Err(DomainPackError::UnsupportedVersion(2))
    ));

    let unknown_command = DomainCommandEnvelope {
        command_type: "official.test.future".to_owned(),
        payload: json!({}),
    };
    assert!(matches!(
        pack.apply_batch(&source, &batch(vec![unknown_command])?),
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
        pack.apply_batch(&source, &two),
        Err(DomainPackError::InvalidPayload { .. })
    ));
    assert_eq!(source, document()?);
    Ok(())
}

#[test]
fn batch_is_atomic_and_inverse_restores_exact_document() -> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let source = document()?;
    let applied = pack.apply_batch(&source, &batch(vec![command(true, 3), command(true, 4)])?)?;
    assert_eq!(applied.results.len(), 2);
    assert_eq!(applied.changes.len(), 2);
    assert_eq!(applied.inverse.commands.len(), 2);
    let restored = pack.apply_batch(&applied.document, &applied.inverse)?;
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

    let problem = pack.compile(&source, &context())?;
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
    let solution = pack.project(&problem, &candidate, SolutionId::from_str(SOLUTION_ID)?)?;
    let authoritative_score = pack.score(&source, &solution)?;
    let verification_context = verification_context(&pack, &source, &problem, &solution)?;
    let verification = pack.verify(
        &source,
        &solution,
        &verification_context,
        &authoritative_score,
    )?;
    let accepted = AcceptedResult::new(solution, verification)?;
    source.extensions.insert(
        "nonsemantic.source-only".to_owned(),
        json!({"private": "must-not-be-shared"}),
    );
    let shared = pack.build_share_result(&source, &accepted, ShareResultOptions::default())?;
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
        pack.build_share_result(&source, &unaccepted, ShareResultOptions::default())
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
    candidate
}

#[test]
// This contract test keeps compilation, projection, verification, and score determinism together.
#[allow(clippy::too_many_lines)]
fn compile_project_verify_and_score_are_deterministic() -> Result<(), Box<dyn Error>> {
    let pack = OfficialTestPack;
    let source = document()?;
    let limits = PlanningIrLimitsV1::DEFAULT;
    let verification_scope = pack.verification_scope(&source, context().scenario_revision)?;
    assert_eq!(
        verification_scope,
        pack.verification_scope(&source, context().scenario_revision)?
    );
    assert_eq!(verification_scope.required_rules.len(), 1);
    assert_eq!(
        verification_scope.required_rules[0].rule_id,
        RuleId::from_uuid(EntityId::from_str(ENTITY_ID)?.as_uuid())
    );
    let first = pack.compile(&source, &context())?;
    let second = pack.compile(&source, &context())?;
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
    let renamed = pack.compile(&renamed_source, &context())?;
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
    let solution = pack.project(&first, &candidate, SolutionId::from_str(SOLUTION_ID)?)?;
    let projection_evidence = format!("official_test.projection.{ENTITY_ID}");
    assert!(solution.assignments.iter().all(|assignment| {
        assignment.evidence.len() == 1 && assignment.evidence[0].as_str() == projection_evidence
    }));
    let authoritative_score = pack.score(&source, &solution)?;
    let context_for_solution = verification_context(&pack, &source, &first, &solution)?;
    let mut unsatisfied = solution.clone();
    let target = unsatisfied
        .assignments
        .iter_mut()
        .find(|assignment| assignment.id.as_str().contains(".target."))
        .ok_or("target assignment missing")?;
    target.value = AssignmentValue::Integer(4);
    let unsatisfied_score = pack.score(&source, &unsatisfied)?;
    let unsatisfied_context = verification_context(&pack, &source, &first, &unsatisfied)?;
    let report = pack.verify(
        &source,
        &solution,
        &context_for_solution,
        &authoritative_score,
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
        pack.verify(&source, &solution, &context_for_solution, &report.score)?
    );

    let unsatisfied_report = pack.verify(
        &source,
        &unsatisfied,
        &unsatisfied_context,
        &unsatisfied_score,
    )?;
    assert!(!unsatisfied_report.accepted);
    assert_eq!(unsatisfied_report.required_rule_results.len(), 1);
    assert!(!unsatisfied_report.required_rule_results[0].satisfied);
    assert!(unsatisfied_report.warnings.is_empty());
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
    assert!(pack.compile(&source, &context()).is_err());

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
    let compile_context = context();
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
    let compile_context = context();
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
    let compile_context = context();
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
