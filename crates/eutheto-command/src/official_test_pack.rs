use crate::generated_official_test_pack_contract::{
    CONFIGURE_ENTITY, OFFICIAL_TEST_COMMAND_IDS, OFFICIAL_TEST_PACK_CONTRACT_JSON,
    OFFICIAL_TEST_PACK_ID, OFFICIAL_TEST_PACK_VERSION,
};
use eutheto_domain_api::{
    AiToolDescriptor, CommandDescriptor, CompileContext, ContractJsonLimits,
    CounterfactualCompileContext, DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand,
    DomainCapability, DomainCatalog, DomainChange, DomainMutation, DomainPack,
    DomainPackDescriptor, DomainPackError, DomainShareResult, DomainUiManifest,
    DomainValidationReport, LicenseMetadata, LocalizedText, PortableImportContext,
    SchemaVersionDescriptor, ShareResultOptions, ValidatedContractSchema, validate_contract_value,
};
use eutheto_domain_ir::{
    AcceptedResult, AssignmentValue, CounterfactualConditionPayloadV1, CounterfactualConditionV1,
    DomainAssignmentId, DomainEntityId, DomainEntityKindId, DomainEntityRef, DomainEvidenceId,
    EvidenceMessageV1, EvidenceRenderRequestV1, EvidenceRenderResultV1, ExplanationCapability,
    ExplanationKind, NormalizedSolution, OptimizationDirection, RequiredRuleBinding,
    RuleEvaluation, ScoreCategoryId, ScoreLevelId, ScoreLevelValue, ScoreVector,
    VerificationContextV1, VerificationFactId, VerificationReport, VerificationScope,
    VerificationValue, blake3_hex,
};
use eutheto_planning_ir::{
    BoolVariable, BoolVariableId, CandidateValues, Capability, ComparisonOp, CompilerId,
    Constraint, ConstraintRecord, ConstraintTag, InclusiveRange, IntDomain, IntVariable,
    IntVariableId, LinearComparison, LinearExpression, LinearTerm, MetadataKey, ObjectiveLevel,
    ObjectiveLevelId, ObjectivePlan, ObjectiveTerm, ObjectiveTermId, ObjectiveTermKind,
    PLANNING_IR_SCHEMA_VERSION, PROJECTION_SCHEMA_VERSION, PlanningConstraintId, PlanningMetadata,
    PlanningProblem, ProjectionExpression, ProjectionId, ProvenanceId, ProvenanceParameter,
    ProvenanceRecord, ProvenanceSourceKind, SolutionProjection, Variable, canonical_ir_hash,
    project_candidate, validate,
};
use eutheto_types::{
    AssignmentId, CancellationToken, DomainCommandEnvelope, DomainPackRef, EntityId, PackId,
    PortableDomainDocument, RuleId, ScenarioDocument, ScenarioDomain, SemanticCapability,
    SolutionId, ValidationIssue, ValidationSeverity,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

const PORTABLE_CAPABILITY: &str = "official.test.portable";
const HISTORICAL_PORTABLE_CAPABILITY_V1: &str = "official.test.portable-v1";
const ENTITY_KIND: &str = "official.test.entity";
const SCORE_LEVEL: &str = "official.test.score.target";
const SCORE_CATEGORY: &str = "official.test.score.target-total";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeneratedContract {
    schema_version: u32,
    pack: GeneratedPack,
    commands: Vec<CommandDescriptor>,
    internal_schema: Value,
    portable_schema: Value,
    share_result_schema: Value,
    ai_tools: Vec<GeneratedAiTool>,
    ui_manifest: DomainUiManifest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeneratedPack {
    id: String,
    pack_version: String,
    latest_schema_version: u32,
    portable_schema_version: u32,
    share_result_schema_version: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeneratedAiTool {
    command_id: String,
    name: String,
    description: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TestEntity {
    id: EntityId,
    enabled: bool,
    target: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigureEntity {
    entity_id: EntityId,
    enabled: bool,
    target: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortableEntity {
    enabled: bool,
    target: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HistoricalPortablePayloadV1 {
    schema_version: u32,
    entities: BTreeMap<String, PortableEntity>,
    extensions: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HistoricalPortablePayloadV0 {
    schema_version: u32,
    entities: BTreeMap<String, PortableEntity>,
    #[serde(default)]
    extensions: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortablePayloadV2 {
    schema_version: u32,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    entities: BTreeMap<EntityId, Value>,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    rules: BTreeMap<RuleId, Value>,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    preferences: BTreeMap<RuleId, Value>,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    locked_assignments: BTreeMap<AssignmentId, Value>,
    extensions: BTreeMap<String, Value>,
}

impl DomainPack for crate::OfficialTestPack {
    fn descriptor(&self) -> Result<DomainPackDescriptor, DomainPackError> {
        Ok(DomainPackDescriptor {
            id: PackId::new(OFFICIAL_TEST_PACK_ID).map_err(contract)?,
            display_name: text("official.test.name", "Official synthetic test pack"),
            description: text(
                "official.test.description",
                "Synthetic Phase-02 domain-pack conformance fixture.",
            ),
            pack_version: Version::parse(OFFICIAL_TEST_PACK_VERSION).map_err(contract)?,
            scenario_versions: SchemaVersionDescriptor {
                latest: 1,
                migratable_from: BTreeSet::new(),
            },
            icon_id: "official.test.icon".to_owned(),
            capabilities: [
                DomainCapability::Commands,
                DomainCapability::Compilation,
                DomainCapability::Projection,
                DomainCapability::Verification,
                DomainCapability::Scoring,
                DomainCapability::PortableData,
                DomainCapability::ShareResult,
                DomainCapability::UiManifest,
                DomainCapability::AiTools,
            ]
            .into_iter()
            .collect(),
            portable_versions: SchemaVersionDescriptor {
                latest: 2,
                migratable_from: [0, 1].into_iter().collect(),
            },
            explanation_capabilities: [
                ExplanationCapability::Validation,
                ExplanationCapability::Infeasibility,
                ExplanationCapability::Assignment,
                ExplanationCapability::Counterfactual,
                ExplanationCapability::SolutionDifference,
                ExplanationCapability::Repair,
                ExplanationCapability::OptimalityStatus,
            ]
            .into_iter()
            .collect(),
            portable_capabilities: [historical_portable_capability_v1(), portable_capability()]
                .into_iter()
                .collect(),
            share_result_schema_version: 1,
            documentation_url: None,
            license: LicenseMetadata {
                spdx_expression: "Apache-2.0".to_owned(),
                attribution: "Eutheto contributors".to_owned(),
            },
            synthetic_test_only: true,
        })
    }

    fn catalog(&self) -> Result<DomainCatalog, DomainPackError> {
        generated_catalog()
    }

    fn new_document(
        &self,
        mut shell: ScenarioDocument,
    ) -> Result<ScenarioDocument, DomainPackError> {
        require_pack(&shell)?;
        shell.domain = ScenarioDomain::default();
        shell.domain_pack.schema_version = 1;
        Ok(shell)
    }

    fn migrate_document(
        &self,
        document: ScenarioDocument,
    ) -> Result<ScenarioDocument, DomainPackError> {
        require_pack(&document)?;
        parse_entities(&document)?;
        Ok(document)
    }

    fn validate_fast(&self, document: &ScenarioDocument) -> DomainValidationReport {
        validation_report(document)
    }

    fn validate_full(
        &self,
        document: &ScenarioDocument,
        control: &eutheto_types::OperationControl,
    ) -> Result<DomainValidationReport, DomainPackError> {
        control.check()?;
        let report = validation_report(document);
        control.check()?;
        Ok(report)
    }

    fn apply_batch(
        &self,
        document: &ScenarioDocument,
        batch: &DomainBatchCommand,
        cancellation: &CancellationToken,
    ) -> Result<DomainMutation, DomainPackError> {
        if cancellation.is_cancelled() {
            return Err(DomainPackError::Cancelled);
        }
        require_pack(document)?;
        validate_batch(document, batch)?;
        let descriptor = generated_catalog()?
            .commands
            .into_iter()
            .find(|command| command.id == CONFIGURE_ENTITY)
            .ok_or_else(|| DomainPackError::UnknownCommand(CONFIGURE_ENTITY.to_owned()))?;
        let payload_schema = ValidatedContractSchema::new(descriptor.payload_schema)?;
        let result_schema = ValidatedContractSchema::new(descriptor.result_schema)?;
        let change_schema = ValidatedContractSchema::new(descriptor.change_schema)?;
        let mut working = document.clone();
        let mut results = Vec::with_capacity(batch.commands.len());
        let mut changes = Vec::with_capacity(batch.commands.len());
        let mut inverses = Vec::with_capacity(batch.commands.len());
        for (index, envelope) in batch.commands.iter().enumerate() {
            if cancellation.is_cancelled() {
                return Err(DomainPackError::Cancelled);
            }
            payload_schema.validate(&envelope.payload, ContractJsonLimits::DEFAULT)?;
            let command: ConfigureEntity = serde_json::from_value(envelope.payload.clone())
                .map_err(|error| payload("/commands/payload", error))?;
            let (change, inverse) = configure_entity(&mut working, &command)?;
            let result = json!({ "entityId": command.entity_id });
            result_schema.validate(&result, ContractJsonLimits::DEFAULT)?;
            change_schema.validate(&change, ContractJsonLimits::DEFAULT)?;
            results.push(result);
            changes.push(DomainChange {
                command_index: u32::try_from(index)
                    .map_err(|_| DomainPackError::MutationOutputLimit)?,
                value: change,
            });
            inverses.push(inverse);
        }
        inverses.reverse();
        if cancellation.is_cancelled() {
            return Err(DomainPackError::Cancelled);
        }
        let mutation = DomainMutation {
            document: working,
            results,
            changes,
            inverse: DomainBatchCommand {
                schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
                pack_id: batch.pack_id.clone(),
                scenario_schema_version: batch.scenario_schema_version,
                label: batch.label.clone(),
                commands: inverses,
            },
        };
        mutation.inverse.validate_inverse_bounds()?;
        Ok(mutation)
    }

    fn compile(
        &self,
        document: &ScenarioDocument,
        context: &CompileContext,
    ) -> Result<PlanningProblem, DomainPackError> {
        context.control.check()?;
        require_pack(document)?;
        build_problem(document, context, &parse_entities(document)?)
    }

    fn project(
        &self,
        problem: &PlanningProblem,
        candidate: &CandidateValues,
        solution_id: SolutionId,
        control: &eutheto_types::OperationControl,
    ) -> Result<NormalizedSolution, DomainPackError> {
        control.check()?;
        if problem.metadata.pack_id.as_str() != OFFICIAL_TEST_PACK_ID {
            return Err(DomainPackError::Contract(
                "planning problem pack mismatch".to_owned(),
            ));
        }
        project_candidate(
            problem,
            candidate,
            solution_id,
            eutheto_planning_ir::PlanningIrLimitsV1::DEFAULT,
        )
        .map_err(contract)
    }

    fn verification_scope(
        &self,
        document: &ScenarioDocument,
        scenario_revision: u64,
        control: &eutheto_types::OperationControl,
    ) -> Result<VerificationScope, DomainPackError> {
        control.check()?;
        let required_rules = parse_entities(document)?
            .into_values()
            .map(|entity| {
                control.check()?;
                Ok(RequiredRuleBinding {
                    rule_id: RuleId::from_uuid(entity.id.as_uuid()),
                    semantic_hash: blake3_hex(
                        &serde_json::to_vec(&("official.test.required_target.v1", &entity))
                            .map_err(|error| payload("/domain/entities", error))?,
                    ),
                })
            })
            .collect::<Result<Vec<_>, DomainPackError>>()?;
        VerificationScope::new(document.scenario_id, scenario_revision, required_rules)
            .map_err(contract)
    }

    fn verify(
        &self,
        document: &ScenarioDocument,
        solution: &NormalizedSolution,
        context: &VerificationContextV1,
        authoritative_score: &ScoreVector,
        control: &eutheto_types::OperationControl,
    ) -> Result<VerificationReport, DomainPackError> {
        control.check()?;
        validate_verification_context(*self, document, solution, context, control)?;
        let evaluations = evaluate_required_rules(document, solution, control)?;
        VerificationReport::new(
            context,
            evaluations,
            authoritative_score.clone(),
            Vec::new(),
            BTreeMap::new(),
        )
        .map_err(contract)
    }

    fn score(
        &self,
        document: &ScenarioDocument,
        solution: &NormalizedSolution,
        control: &eutheto_types::OperationControl,
    ) -> Result<ScoreVector, DomainPackError> {
        control.check()?;
        authoritative_score(document, solution, control)
    }

    fn export_portable(
        &self,
        document: &ScenarioDocument,
    ) -> Result<PortableDomainDocument, DomainPackError> {
        require_pack(document)?;
        crate::validate_document_shape(document).map_err(contract)?;
        if document
            .extensions
            .keys()
            .any(|namespace| !eutheto_types::is_portable_namespace(namespace))
        {
            return Err(contract("invalid portable extension namespace"));
        }
        let payload_value = json!({
            "schemaVersion": 2,
            "entities": &document.domain.entities,
            "rules": &document.domain.rules,
            "preferences": &document.domain.preferences,
            "lockedAssignments": &document.domain.locked_assignments,
            "extensions": &document.extensions,
        });
        validate_contract_value(
            &generated_catalog()?.portable_schema,
            &payload_value,
            ContractJsonLimits::DEFAULT,
        )?;
        Ok(PortableDomainDocument {
            pack_id: document.domain_pack.id.clone(),
            schema_version: 2,
            required_capabilities: [portable_capability()].into_iter().collect(),
            payload: payload_value,
        })
    }

    fn migrate_portable_step(
        &self,
        document: PortableDomainDocument,
    ) -> Result<PortableDomainDocument, DomainPackError> {
        if document.pack_id.as_str() != OFFICIAL_TEST_PACK_ID {
            return Err(DomainPackError::PackUnavailable(
                document.pack_id.to_string(),
            ));
        }
        let (schema_version, capability, payload_value) = match document.schema_version {
            0 => {
                if !document.required_capabilities.is_empty() {
                    return Err(contract("unknown historical semantic portable capability"));
                }
                let historical: HistoricalPortablePayloadV0 =
                    serde_json::from_value(document.payload)
                        .map_err(|error| payload("/portable", error))?;
                if historical.schema_version != 0 {
                    return Err(DomainPackError::UnsupportedVersion(
                        historical.schema_version,
                    ));
                }
                validate_portable_extensions(&historical.extensions)?;
                let value = serde_json::to_value(HistoricalPortablePayloadV1 {
                    schema_version: 1,
                    entities: historical.entities,
                    extensions: historical.extensions,
                })
                .map_err(|error| payload("/portable", error))?;
                (1, historical_portable_capability_v1(), value)
            }
            1 => {
                if document.required_capabilities
                    != [historical_portable_capability_v1()].into_iter().collect()
                {
                    return Err(contract(
                        "unknown or missing historical semantic portable capability",
                    ));
                }
                let historical: HistoricalPortablePayloadV1 =
                    serde_json::from_value(document.payload)
                        .map_err(|error| payload("/portable", error))?;
                if historical.schema_version != 1 {
                    return Err(DomainPackError::UnsupportedVersion(
                        historical.schema_version,
                    ));
                }
                validate_portable_extensions(&historical.extensions)?;
                let mut entities = BTreeMap::new();
                for (raw_id, entity) in historical.entities {
                    let id = EntityId::from_str(&raw_id).map_err(contract)?;
                    check_target(entity.target)?;
                    let std::collections::btree_map::Entry::Vacant(entry) = entities.entry(id)
                    else {
                        return Err(contract("duplicate typed record identity"));
                    };
                    entry.insert(
                        serde_json::to_value(TestEntity {
                            id,
                            enabled: entity.enabled,
                            target: entity.target,
                        })
                        .map_err(|error| payload("/portable/entities", error))?,
                    );
                }
                let value = json!({
                    "schemaVersion": 2,
                    "entities": entities,
                    "rules": {},
                    "preferences": {},
                    "lockedAssignments": {},
                    "extensions": historical.extensions,
                });
                validate_contract_value(
                    &generated_catalog()?.portable_schema,
                    &value,
                    ContractJsonLimits::DEFAULT,
                )?;
                (2, portable_capability(), value)
            }
            version => return Err(DomainPackError::UnsupportedVersion(version)),
        };
        Ok(PortableDomainDocument {
            pack_id: document.pack_id,
            schema_version,
            required_capabilities: [capability].into_iter().collect(),
            payload: payload_value,
        })
    }

    fn import_portable(
        &self,
        document: &PortableDomainDocument,
        context: &PortableImportContext,
    ) -> Result<ScenarioDocument, DomainPackError> {
        if document.pack_id.as_str() != OFFICIAL_TEST_PACK_ID {
            return Err(DomainPackError::PackUnavailable(
                document.pack_id.to_string(),
            ));
        }
        if document.schema_version != 2 {
            return Err(DomainPackError::UnsupportedVersion(document.schema_version));
        }
        if document.required_capabilities != [portable_capability()].into_iter().collect() {
            return Err(contract("unknown or missing semantic portable capability"));
        }
        validate_contract_value(
            &generated_catalog()?.portable_schema,
            &document.payload,
            ContractJsonLimits::DEFAULT,
        )?;
        let portable: PortablePayloadV2 = serde_json::from_value(document.payload.clone())
            .map_err(|error| payload("/portable", error))?;
        if portable.schema_version != document.schema_version {
            return Err(DomainPackError::UnsupportedVersion(portable.schema_version));
        }
        let mut result = context.scenario_shell.clone();
        require_pack(&result)?;
        result.domain = ScenarioDomain {
            entities: portable.entities,
            rules: portable.rules,
            preferences: portable.preferences,
            locked_assignments: portable.locked_assignments,
        };
        result.domain_pack = DomainPackRef {
            id: document.pack_id.clone(),
            schema_version: 1,
        };
        result.extensions = portable.extensions;
        if result
            .extensions
            .keys()
            .any(|namespace| !eutheto_types::is_portable_namespace(namespace))
        {
            return Err(contract("invalid portable extension namespace"));
        }
        crate::validate_document_shape(&result).map_err(contract)?;
        Ok(result)
    }

    fn build_share_result(
        &self,
        document: &ScenarioDocument,
        accepted: &AcceptedResult,
        options: ShareResultOptions,
    ) -> Result<DomainShareResult, DomainPackError> {
        require_pack(document)?;
        accepted.validate().map_err(contract)?;
        if accepted.solution.pack_id.as_str() != OFFICIAL_TEST_PACK_ID
            || accepted.solution.scenario_id != document.scenario_id
        {
            return Err(DomainPackError::Contract(
                "accepted result scenario mismatch".to_owned(),
            ));
        }
        let _ = options;
        let assignments: Result<Vec<_>, DomainPackError> = accepted
            .solution
            .assignments
            .iter()
            .map(|assignment| {
                let value = match assignment.value {
                    AssignmentValue::Boolean(value) => Value::Bool(value),
                    AssignmentValue::Integer(value) => Value::from(value),
                    AssignmentValue::Absent => Value::Null,
                    AssignmentValue::Interval(_) => {
                        return Err(DomainPackError::Contract(
                            "test Share Result does not support interval assignments".to_owned(),
                        ));
                    }
                };
                Ok(json!({ "assignmentId": assignment.id.as_str(), "value": value }))
            })
            .collect();
        let payload_value = json!({ "schemaVersion": 1, "assignments": assignments? });
        validate_contract_value(
            &generated_catalog()?.share_result_schema,
            &payload_value,
            ContractJsonLimits::DEFAULT,
        )?;
        Ok(DomainShareResult {
            pack_id: document.domain_pack.id.clone(),
            schema_version: 1,
            payload: payload_value,
        })
    }

    fn build_view(
        &self,
        document: &ScenarioDocument,
        solution: Option<&NormalizedSolution>,
        view_id: &str,
    ) -> Result<eutheto_domain_api::DomainView, DomainPackError> {
        require_pack(document)?;
        if !generated_catalog()?
            .ui
            .result_views
            .iter()
            .any(|view| view.id == view_id)
        {
            return Err(DomainPackError::InvalidPayload {
                path: "/viewId".to_owned(),
                message: format!("unknown view {view_id}"),
            });
        }
        Ok(eutheto_domain_api::DomainView {
            view_id: view_id.to_owned(),
            data: json!({
                "entityCount": document.domain.entities.len(),
                "assignmentCount": solution.map_or(0, |value| value.assignments.len()),
            }),
        })
    }

    fn render_evidence(
        &self,
        document: &ScenarioDocument,
        request: &EvidenceRenderRequestV1,
    ) -> Result<EvidenceRenderResultV1, DomainPackError> {
        parse_entities(document)?;
        request.validate().map_err(contract)?;
        require_explanation_capability(explanation_capability(request.kind))?;
        EvidenceRenderResultV1::new(request, vec![evidence_message(request)?]).map_err(contract)
    }

    fn compile_counterfactual(
        &self,
        document: &ScenarioDocument,
        condition: &CounterfactualConditionV1,
        context: &CounterfactualCompileContext<'_>,
    ) -> Result<PlanningProblem, DomainPackError> {
        require_explanation_capability(ExplanationCapability::Counterfactual)?;
        compile_counterfactual_problem(document, condition, context)
    }
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

fn require_explanation_capability(
    capability: ExplanationCapability,
) -> Result<(), DomainPackError> {
    if crate::OfficialTestPack
        .descriptor()?
        .explanation_capabilities
        .contains(&capability)
    {
        Ok(())
    } else {
        Err(DomainPackError::UnsupportedExplanationCapability(
            capability,
        ))
    }
}

fn evidence_message(
    request: &EvidenceRenderRequestV1,
) -> Result<EvidenceMessageV1, DomainPackError> {
    evidence_message_for_kind(request.kind, &request.evidence.checksum)
}

fn evidence_message_for_kind(
    kind: ExplanationKind,
    evidence_checksum: &str,
) -> Result<EvidenceMessageV1, DomainPackError> {
    let (message_key, kind_name) = match kind {
        ExplanationKind::Validation => ("official.test.explanation.validation", "validation"),
        ExplanationKind::Infeasibility => {
            ("official.test.explanation.infeasibility", "infeasibility")
        }
        ExplanationKind::Assignment => ("official.test.explanation.assignment", "assignment"),
        ExplanationKind::Counterfactual => {
            ("official.test.explanation.counterfactual", "counterfactual")
        }
        ExplanationKind::SolutionDifference => (
            "official.test.explanation.solution_difference",
            "solutionDifference",
        ),
        ExplanationKind::Repair => ("official.test.explanation.repair", "repair"),
        ExplanationKind::OptimalityStatus => (
            "official.test.explanation.optimality_status",
            "optimalityStatus",
        ),
    };
    let parameters = [
        (
            VerificationFactId::new("official.test.explanation.kind").map_err(contract)?,
            VerificationValue::Text(kind_name.to_owned()),
        ),
        (
            VerificationFactId::new("official.test.explanation.evidence_checksum")
                .map_err(contract)?,
            VerificationValue::Text(evidence_checksum.to_owned()),
        ),
    ]
    .into_iter()
    .collect();
    Ok(EvidenceMessageV1 {
        message_key: message_key.to_owned(),
        parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: Vec::new(),
        evidence: Vec::new(),
    })
}

fn check_counterfactual_budget(
    context: &CounterfactualCompileContext<'_>,
) -> Result<(), DomainPackError> {
    if context.budget.is_cancelled() {
        Err(DomainPackError::Cancelled)
    } else if context.budget.is_expired() {
        Err(DomainPackError::BudgetExpired)
    } else {
        Ok(())
    }
}

fn counterfactual_payload_error(path: &str, message: &str) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

#[allow(clippy::too_many_lines)]
fn compile_counterfactual_problem(
    document: &ScenarioDocument,
    condition: &CounterfactualConditionV1,
    context: &CounterfactualCompileContext<'_>,
) -> Result<PlanningProblem, DomainPackError> {
    check_counterfactual_budget(context)?;
    context.compile_context.control.check()?;
    let entities = parse_entities(document)?;
    condition.validate().map_err(contract)?;
    validate(
        context.base_problem,
        context.compile_context.planning_limits,
    )
    .map_err(contract)?;

    let (assignment_id, value, force) = match &condition.condition {
        CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value,
        } => (assignment_id, value, true),
        CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id,
            value,
        } => (assignment_id, value, false),
    };
    let projection = context
        .base_problem
        .projections
        .iter()
        .find(|projection| &projection.assignment_id == assignment_id)
        .ok_or_else(|| {
            counterfactual_payload_error(
                "/condition/assignmentId",
                "assignment projection is not supported by official.test",
            )
        })?;
    check_counterfactual_budget(context)?;

    let body = match (&projection.expression, value, force) {
        (ProjectionExpression::Boolean(variable), AssignmentValue::Boolean(value), force) => {
            let required = if force { *value } else { !*value };
            Constraint::bool_and(vec![if required {
                eutheto_planning_ir::Literal::positive(variable.clone())
            } else {
                eutheto_planning_ir::Literal::negative(variable.clone())
            }])
        }
        (ProjectionExpression::Integer(variable), AssignmentValue::Integer(value), true) => {
            Constraint::LinearComparison(LinearComparison {
                expression: target_expression(variable)?,
                op: ComparisonOp::Equal,
                rhs: *value,
            })
        }
        (ProjectionExpression::Integer(variable), AssignmentValue::Integer(value), false) => {
            Constraint::forbidden_table(vec![variable.clone()], vec![vec![*value]])
                .map_err(contract)?
        }
        (ProjectionExpression::Boolean(_) | ProjectionExpression::Integer(_), _, _) => {
            return Err(counterfactual_payload_error(
                "/condition/value",
                "condition value kind does not match the assignment projection",
            ));
        }
        (
            ProjectionExpression::Linear(_)
            | ProjectionExpression::Interval(_)
            | ProjectionExpression::Constant(_),
            _,
            _,
        ) => {
            return Err(counterfactual_payload_error(
                "/condition/assignmentId",
                "projection kind is not supported by official.test counterfactual compilation",
            ));
        }
    };
    let recompiled = build_problem(document, context.compile_context, &entities)?;
    check_counterfactual_budget(context)?;
    let base_hash = canonical_ir_hash(
        context.base_problem,
        context.compile_context.planning_limits,
    )
    .map_err(contract)?;
    let recompiled_hash = canonical_ir_hash(&recompiled, context.compile_context.planning_limits)
        .map_err(contract)?;
    if base_hash != recompiled_hash {
        return Err(counterfactual_payload_error(
            "/context/baseProblem",
            "base planning problem does not match deterministic recompilation",
        ));
    }

    let metadata_key =
        MetadataKey::new("eutheto.counterfactual.condition_hash").map_err(contract)?;
    let provenance_id = ProvenanceId::new(format!(
        "official_test.counterfactual.provenance.{}",
        condition.checksum
    ))
    .map_err(contract)?;
    let constraint_id = PlanningConstraintId::new(format!(
        "official_test.counterfactual.constraint.{}",
        condition.checksum
    ))
    .map_err(contract)?;
    let mut derived = context.base_problem.clone();
    if derived
        .metadata
        .compile_metadata
        .insert(
            metadata_key,
            ProvenanceParameter::Text(condition.checksum.clone()),
        )
        .is_some()
    {
        return Err(counterfactual_payload_error(
            "/context/baseProblem/metadata/compileMetadata",
            "base planning problem already contains counterfactual condition metadata",
        ));
    }
    derived.provenance.push(ProvenanceRecord {
        id: provenance_id.clone(),
        source_kind: ProvenanceSourceKind::Derived,
        source_id: "official.test.counterfactual.condition".to_owned(),
        entity_refs: vec![projection.entity.clone()],
        message_key: "official.test.provenance.counterfactual_condition".to_owned(),
        parameters: BTreeMap::from([(
            "conditionChecksum".to_owned(),
            ProvenanceParameter::Text(condition.checksum.clone()),
        )]),
        parent: Some(projection.provenance.clone()),
    });
    derived.constraints.push(ConstraintRecord {
        id: constraint_id,
        body,
        enforcement: Vec::new(),
        provenance: provenance_id,
        tags: vec![ConstraintTag::new("official_test.counterfactual.condition").map_err(contract)?],
    });
    derived.canonicalize().map_err(contract)?;
    derived
        .declared_capabilities
        .insert(Capability::ForbiddenTable);
    validate(&derived, context.compile_context.planning_limits).map_err(contract)?;
    check_counterfactual_budget(context)?;
    Ok(derived)
}

fn generated_catalog() -> Result<DomainCatalog, DomainPackError> {
    let generated: GeneratedContract =
        serde_json::from_str(OFFICIAL_TEST_PACK_CONTRACT_JSON).map_err(contract)?;
    validate_generated_contract(&generated)?;
    let GeneratedContract {
        pack,
        commands,
        internal_schema,
        portable_schema,
        share_result_schema,
        ai_tools,
        ui_manifest,
        ..
    } = generated;
    let ai_tools = ai_tool_descriptors(ai_tools, &commands)?;
    Ok(DomainCatalog {
        pack_id: PackId::new(OFFICIAL_TEST_PACK_ID).map_err(contract)?,
        scenario_schema_version: pack.latest_schema_version,
        internal_schema,
        portable_schema,
        share_result_schema,
        commands,
        ai_tools,
        ui: ui_manifest,
    })
}

fn validate_generated_contract(generated: &GeneratedContract) -> Result<(), DomainPackError> {
    let generated_ids: Vec<_> = generated
        .commands
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    if generated.schema_version == 3
        && generated.pack.id == OFFICIAL_TEST_PACK_ID
        && generated.pack.pack_version == OFFICIAL_TEST_PACK_VERSION
        && generated.pack.latest_schema_version == 1
        && generated.pack.portable_schema_version == 2
        && generated.pack.share_result_schema_version == 1
        && generated_ids.as_slice() == OFFICIAL_TEST_COMMAND_IDS
    {
        Ok(())
    } else {
        Err(DomainPackError::CatalogMismatch(
            "generated official-test constants".to_owned(),
        ))
    }
}

fn ai_tool_descriptors(
    tools: Vec<GeneratedAiTool>,
    commands: &[CommandDescriptor],
) -> Result<Vec<AiToolDescriptor>, DomainPackError> {
    tools
        .into_iter()
        .map(|tool| {
            let command = commands
                .iter()
                .find(|command| command.id == tool.command_id)
                .ok_or_else(|| DomainPackError::UnknownCommand(tool.command_id.clone()))?;
            Ok(AiToolDescriptor {
                command_id: tool.command_id,
                name: tool.name,
                description: tool.description,
                input_schema: command.payload_schema.clone(),
                valid_examples: command.valid_examples.clone(),
            })
        })
        .collect()
}

fn text(key: &str, default_text: &str) -> LocalizedText {
    LocalizedText {
        key: key.to_owned(),
        default_text: default_text.to_owned(),
    }
}

fn require_pack(document: &ScenarioDocument) -> Result<(), DomainPackError> {
    if document.domain_pack.id.as_str() != OFFICIAL_TEST_PACK_ID {
        return Err(DomainPackError::PackUnavailable(
            document.domain_pack.id.to_string(),
        ));
    }
    if document.domain_pack.schema_version != 1 {
        return Err(DomainPackError::UnsupportedVersion(
            document.domain_pack.schema_version,
        ));
    }
    Ok(())
}

fn portable_capability() -> SemanticCapability {
    SemanticCapability {
        id: PORTABLE_CAPABILITY.to_owned(),
        version: 2,
    }
}

fn historical_portable_capability_v1() -> SemanticCapability {
    SemanticCapability {
        // This is the exact historical requirement, not a parsed version suffix.
        id: HISTORICAL_PORTABLE_CAPABILITY_V1.to_owned(),
        version: 1,
    }
}

fn validate_portable_extensions(
    extensions: &BTreeMap<String, Value>,
) -> Result<(), DomainPackError> {
    if let Some(extension) = extensions
        .keys()
        .find(|key| !key.starts_with("nonsemantic."))
    {
        return Err(DomainPackError::Contract(format!(
            "unknown semantic extension {extension}"
        )));
    }
    Ok(())
}

fn parse_entities(
    document: &ScenarioDocument,
) -> Result<BTreeMap<EntityId, TestEntity>, DomainPackError> {
    require_pack(document)?;
    let mut entities = BTreeMap::new();
    for (id, value) in &document.domain.entities {
        let entity: TestEntity = serde_json::from_value(value.clone())
            .map_err(|error| payload(&format!("/domain/entities/{id}"), error))?;
        if entity.id != *id {
            return Err(DomainPackError::Contract(format!(
                "entity {id} map key/id mismatch"
            )));
        }
        check_target(entity.target)?;
        entities.insert(*id, entity);
    }
    Ok(entities)
}

fn check_target(target: i64) -> Result<(), DomainPackError> {
    if (0..=10).contains(&target) {
        Ok(())
    } else {
        Err(DomainPackError::InvalidPayload {
            path: "/target".to_owned(),
            message: "target must be between 0 and 10".to_owned(),
        })
    }
}

fn validation_report(document: &ScenarioDocument) -> DomainValidationReport {
    let issues = match parse_entities(document) {
        Err(error) => vec![ValidationIssue {
            code: "official.test.invalid_document".to_owned(),
            severity: ValidationSeverity::Error,
            message: error.to_string(),
            field_path: Some("/domain/entities".to_owned()),
            resource: None,
        }],
        Ok(entities) => entities
            .into_iter()
            .filter(|(_, entity)| entity.enabled != (entity.target >= 1))
            .map(|(id, _)| ValidationIssue {
                code: "official.test.inconsistent_values".to_owned(),
                severity: ValidationSeverity::Error,
                message: "enabled must equal target >= 1".to_owned(),
                field_path: Some(format!("/domain/entities/{id}")),
                resource: None,
            })
            .collect(),
    };
    DomainValidationReport { issues }
}

fn validate_batch(
    document: &ScenarioDocument,
    batch: &DomainBatchCommand,
) -> Result<(), DomainPackError> {
    batch.validate_bounds()?;
    if batch.pack_id != document.domain_pack.id {
        return Err(DomainPackError::PackUnavailable(batch.pack_id.to_string()));
    }
    if batch.scenario_schema_version != document.domain_pack.schema_version {
        return Err(DomainPackError::UnsupportedVersion(
            batch.scenario_schema_version,
        ));
    }
    if let Some(command) = batch
        .commands
        .iter()
        .find(|command| command.command_type != CONFIGURE_ENTITY)
    {
        return Err(DomainPackError::UnknownCommand(
            command.command_type.clone(),
        ));
    }
    Ok(())
}

struct ProblemParts {
    variables: Vec<Variable>,
    constraints: Vec<ConstraintRecord>,
    objective_terms: Vec<ObjectiveTerm>,
    projections: Vec<SolutionProjection>,
    provenance: Vec<ProvenanceRecord>,
}

impl ProblemParts {
    fn with_capacity(entity_count: usize) -> Self {
        Self {
            variables: Vec::with_capacity(entity_count * 2),
            constraints: Vec::with_capacity(entity_count * 3),
            objective_terms: Vec::with_capacity(entity_count),
            projections: Vec::with_capacity(entity_count * 2),
            provenance: Vec::with_capacity(entity_count * 3 + 1),
        }
    }

    fn add_entity(
        &mut self,
        id: &EntityId,
        entity: &TestEntity,
        preference_provenance: &ProvenanceId,
    ) -> Result<(), DomainPackError> {
        let symbols = EntityProblemSymbols::new(id)?;
        self.variables.extend(entity_variables(&symbols)?);
        self.constraints
            .extend(entity_constraints(&symbols, entity)?);
        self.objective_terms
            .push(entity_objective(&symbols, preference_provenance)?);
        self.projections.extend(entity_projections(&symbols)?);
        self.provenance.extend(entity_provenance(symbols));
        Ok(())
    }
}

struct EntityProblemSymbols {
    suffix: String,
    bool_id: BoolVariableId,
    int_id: IntVariableId,
    fact: ProvenanceId,
    rule: ProvenanceId,
    projection: ProvenanceId,
    entity_ref: DomainEntityRef,
}

impl EntityProblemSymbols {
    fn new(id: &EntityId) -> Result<Self, DomainPackError> {
        let suffix = id.to_string();
        Ok(Self {
            bool_id: BoolVariableId::new(format!("official_test.enabled.{suffix}"))
                .map_err(contract)?,
            int_id: IntVariableId::new(format!("official_test.target.{suffix}"))
                .map_err(contract)?,
            fact: ProvenanceId::new(format!("official_test.fact.{suffix}")).map_err(contract)?,
            rule: ProvenanceId::new(format!("official_test.rule.{suffix}")).map_err(contract)?,
            projection: ProvenanceId::new(format!("official_test.projection.{suffix}"))
                .map_err(contract)?,
            entity_ref: DomainEntityRef {
                kind: DomainEntityKindId::new(ENTITY_KIND).map_err(contract)?,
                id: DomainEntityId::new(format!("official.test.entity.{suffix}"))
                    .map_err(contract)?,
            },
            suffix,
        })
    }
}

fn entity_variables(symbols: &EntityProblemSymbols) -> Result<[Variable; 2], DomainPackError> {
    let domain = IntDomain::new(vec![InclusiveRange { start: 0, end: 10 }]).map_err(contract)?;
    Ok([
        Variable::Boolean(BoolVariable {
            id: symbols.bool_id.clone(),
            provenance: symbols.fact.clone(),
        }),
        Variable::Integer(IntVariable {
            id: symbols.int_id.clone(),
            domain,
            provenance: symbols.fact.clone(),
        }),
    ])
}

fn entity_constraints(
    symbols: &EntityProblemSymbols,
    entity: &TestEntity,
) -> Result<[ConstraintRecord; 3], DomainPackError> {
    let required_tag = || ConstraintTag::new("official_test.required").map_err(contract);
    Ok([
        ConstraintRecord {
            id: PlanningConstraintId::new(format!(
                "official_test.fixed_enabled.{}",
                symbols.suffix
            ))
            .map_err(contract)?,
            body: Constraint::bool_and(vec![if entity.enabled {
                eutheto_planning_ir::Literal::positive(symbols.bool_id.clone())
            } else {
                eutheto_planning_ir::Literal::negative(symbols.bool_id.clone())
            }]),
            enforcement: Vec::new(),
            provenance: symbols.fact.clone(),
            tags: vec![required_tag()?],
        },
        ConstraintRecord {
            id: PlanningConstraintId::new(format!("official_test.fixed_target.{}", symbols.suffix))
                .map_err(contract)?,
            body: Constraint::LinearComparison(LinearComparison {
                expression: target_expression(&symbols.int_id)?,
                op: ComparisonOp::Equal,
                rhs: entity.target,
            }),
            enforcement: Vec::new(),
            provenance: symbols.fact.clone(),
            tags: vec![required_tag()?],
        },
        ConstraintRecord {
            id: PlanningConstraintId::new(format!(
                "official_test.required_target.{}",
                symbols.suffix
            ))
            .map_err(contract)?,
            body: Constraint::ReifiedLinearComparison {
                literal: eutheto_planning_ir::Literal::positive(symbols.bool_id.clone()),
                comparison: LinearComparison {
                    expression: target_expression(&symbols.int_id)?,
                    op: ComparisonOp::GreaterOrEqual,
                    rhs: 1,
                },
            },
            enforcement: Vec::new(),
            provenance: symbols.rule.clone(),
            tags: vec![required_tag()?],
        },
    ])
}

fn target_expression(int_id: &IntVariableId) -> Result<LinearExpression, DomainPackError> {
    LinearExpression::new(
        vec![LinearTerm {
            variable: int_id.clone(),
            coefficient: 1,
        }],
        0,
    )
    .map_err(contract)
}

fn entity_objective(
    symbols: &EntityProblemSymbols,
    preference_provenance: &ProvenanceId,
) -> Result<ObjectiveTerm, DomainPackError> {
    Ok(ObjectiveTerm {
        id: ObjectiveTermId::new(format!("official_test.target_penalty.{}", symbols.suffix))
            .map_err(contract)?,
        expression: target_expression(&symbols.int_id)?,
        kind: ObjectiveTermKind::Penalty,
        category: ScoreCategoryId::new(SCORE_CATEGORY).map_err(contract)?,
        provenance: preference_provenance.clone(),
    })
}

fn entity_projections(
    symbols: &EntityProblemSymbols,
) -> Result<[SolutionProjection; 2], DomainPackError> {
    Ok([
        SolutionProjection {
            id: ProjectionId::new(format!("official_test.project_enabled.{}", symbols.suffix))
                .map_err(contract)?,
            assignment_id: DomainAssignmentId::new(format!(
                "official.test.assignment.enabled.{}",
                symbols.suffix
            ))
            .map_err(contract)?,
            entity: symbols.entity_ref.clone(),
            required: true,
            expression: ProjectionExpression::Boolean(symbols.bool_id.clone()),
            provenance: symbols.projection.clone(),
        },
        SolutionProjection {
            id: ProjectionId::new(format!("official_test.project_target.{}", symbols.suffix))
                .map_err(contract)?,
            assignment_id: DomainAssignmentId::new(format!(
                "official.test.assignment.target.{}",
                symbols.suffix
            ))
            .map_err(contract)?,
            entity: symbols.entity_ref.clone(),
            required: true,
            expression: ProjectionExpression::Integer(symbols.int_id.clone()),
            provenance: symbols.projection.clone(),
        },
    ])
}

fn entity_provenance(symbols: EntityProblemSymbols) -> [ProvenanceRecord; 3] {
    [
        provenance_record(
            symbols.fact,
            ProvenanceSourceKind::Fact,
            format!("official.test.entity.{}", symbols.suffix),
            symbols.entity_ref.clone(),
        ),
        provenance_record(
            symbols.rule,
            ProvenanceSourceKind::RequiredRule,
            symbols.suffix.clone(),
            symbols.entity_ref.clone(),
        ),
        provenance_record(
            symbols.projection,
            ProvenanceSourceKind::Projection,
            format!("official.test.projection.{}", symbols.suffix),
            symbols.entity_ref,
        ),
    ]
}

fn compile_metadata(
    context: &CompileContext,
) -> Result<BTreeMap<MetadataKey, ProvenanceParameter>, DomainPackError> {
    context
        .semantic_metadata
        .iter()
        .map(|(key, value)| {
            context.control.check()?;
            Ok((
                MetadataKey::new(key.clone()).map_err(contract)?,
                ProvenanceParameter::Text(value.clone()),
            ))
        })
        .collect()
}

fn build_problem(
    document: &ScenarioDocument,
    context: &CompileContext,
    entities: &BTreeMap<EntityId, TestEntity>,
) -> Result<PlanningProblem, DomainPackError> {
    let preference_provenance =
        ProvenanceId::new("official_test.preference.target").map_err(contract)?;
    let mut parts = ProblemParts::with_capacity(entities.len());
    for (id, entity) in entities {
        context.control.check()?;
        parts.add_entity(id, entity, &preference_provenance)?;
    }
    parts.provenance.push(ProvenanceRecord {
        id: preference_provenance.clone(),
        source_kind: ProvenanceSourceKind::Preference,
        source_id: "official.test.preference.target".to_owned(),
        entity_refs: Vec::new(),
        message_key: "official.test.provenance.target_preference".to_owned(),
        parameters: BTreeMap::new(),
        parent: None,
    });
    let upper_bound = i64::try_from(entities.len())
        .ok()
        .and_then(|count| count.checked_mul(10))
        .ok_or_else(|| DomainPackError::Contract("objective bound overflow".to_owned()))?;
    let mut problem = PlanningProblem {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables: parts.variables,
        constraints: parts.constraints,
        objectives: ObjectivePlan {
            levels: vec![ObjectiveLevel {
                id: ObjectiveLevelId::new(SCORE_LEVEL).map_err(contract)?,
                direction: OptimizationDirection::Minimize,
                lower_bound: 0,
                upper_bound,
                terms: parts.objective_terms,
                provenance: preference_provenance,
            }],
        },
        assumptions: Vec::new(),
        projections: parts.projections,
        provenance: parts.provenance,
        metadata: PlanningMetadata {
            pack_id: document.domain_pack.id.clone(),
            scenario_id: document.scenario_id,
            scenario_revision: context.scenario_revision,
            projection_version: PROJECTION_SCHEMA_VERSION,
            compiler_id: CompilerId::new("official_test.compiler").map_err(contract)?,
            compiler_version: OFFICIAL_TEST_PACK_VERSION.to_owned(),
            compile_metadata: compile_metadata(context)?,
            display_text: BTreeMap::new(),
        },
        declared_capabilities: [
            Capability::ReifiedLinearComparison,
            Capability::BoolAnd,
            Capability::LinearComparison,
            Capability::ObjectivePenalty,
            Capability::BooleanProjection,
            Capability::IntegerProjection,
        ]
        .into_iter()
        .collect(),
        split_authorization: None,
    };
    problem.canonicalize().map_err(contract)?;
    context.control.check()?;
    problem
        .declared_capabilities
        .insert(Capability::ForbiddenTable);
    validate(&problem, context.planning_limits).map_err(contract)?;
    context.control.check()?;
    Ok(problem)
}

fn provenance_record(
    id: ProvenanceId,
    source_kind: ProvenanceSourceKind,
    source_id: String,
    entity: DomainEntityRef,
) -> ProvenanceRecord {
    ProvenanceRecord {
        id,
        source_kind,
        source_id,
        entity_refs: vec![entity],
        message_key: "official.test.provenance.record".to_owned(),
        parameters: BTreeMap::new(),
        parent: None,
    }
}

fn validate_verification_context(
    pack: crate::OfficialTestPack,
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    context: &VerificationContextV1,
    control: &eutheto_types::OperationControl,
) -> Result<(), DomainPackError> {
    context.validate().map_err(contract)?;
    let document_hash =
        blake3_hex(&serde_json::to_vec(document).map_err(|error| payload("/document", error))?);
    let normalized_solution_hash = solution.canonical_hash().map_err(contract)?;
    let scope = pack.verification_scope(document, solution.scenario_revision, control)?;
    if context.scenario_id != document.scenario_id
        || context.evaluated_revision != solution.scenario_revision
        || context.document_hash != document_hash
        || context.normalized_solution_hash != normalized_solution_hash
        || context.verification_scope_checksum != scope.checksum
    {
        return Err(contract("verification context binding mismatch"));
    }
    Ok(())
}

fn evaluate_required_rules(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    control: &eutheto_types::OperationControl,
) -> Result<Vec<RuleEvaluation>, DomainPackError> {
    require_pack(document)?;
    solution.validate().map_err(contract)?;
    if solution.pack_id != document.domain_pack.id || solution.scenario_id != document.scenario_id {
        return Err(DomainPackError::Contract(
            "solution scenario mismatch".to_owned(),
        ));
    }
    let entities = parse_entities(document)?;
    let assignments: BTreeMap<_, _> = solution
        .assignments
        .iter()
        .map(|assignment| (assignment.id.as_str().to_owned(), assignment))
        .collect();
    let expected_assignment_ids: BTreeSet<_> = entities
        .keys()
        .flat_map(|id| {
            let suffix = id.to_string();
            [
                format!("official.test.assignment.enabled.{suffix}"),
                format!("official.test.assignment.target.{suffix}"),
            ]
        })
        .collect();
    if assignments
        .keys()
        .any(|id| !expected_assignment_ids.contains(id))
    {
        return Err(DomainPackError::Contract(
            "solution contains an unknown domain assignment".to_owned(),
        ));
    }

    let enabled_fact = VerificationFactId::new("official.test.fact.enabled").map_err(contract)?;
    let target_fact = VerificationFactId::new("official.test.fact.target").map_err(contract)?;
    let mut evaluations = Vec::with_capacity(entities.len());
    for (id, entity) in &entities {
        control.check()?;
        let suffix = id.to_string();
        let entity_ref = DomainEntityRef {
            kind: DomainEntityKindId::new(ENTITY_KIND).map_err(contract)?,
            id: DomainEntityId::new(format!("official.test.entity.{suffix}")).map_err(contract)?,
        };
        let enabled_assignment =
            assignments.get(&format!("official.test.assignment.enabled.{suffix}"));
        let target_assignment =
            assignments.get(&format!("official.test.assignment.target.{suffix}"));
        if enabled_assignment
            .into_iter()
            .chain(target_assignment)
            .any(|assignment| assignment.entity != entity_ref)
        {
            return Err(contract("solution assignment entity mismatch"));
        }
        let enabled = enabled_assignment.map(|assignment| &assignment.value);
        let target = target_assignment.map(|assignment| &assignment.value);
        let satisfied = matches!(
            (enabled, target),
            (
                Some(AssignmentValue::Boolean(enabled)),
                Some(AssignmentValue::Integer(target))
            ) if *enabled == entity.enabled
                && *target == entity.target
                && *enabled == (*target >= 1)
        );
        evaluations.push(RuleEvaluation {
            rule_id: RuleId::from_uuid(id.as_uuid()),
            satisfied,
            affected_entities: vec![entity_ref],
            message_key: "official.test.verify.required_target".to_owned(),
            expected: [
                (
                    enabled_fact.clone(),
                    VerificationValue::Boolean(entity.enabled),
                ),
                (
                    target_fact.clone(),
                    VerificationValue::Integer(entity.target),
                ),
            ]
            .into_iter()
            .collect(),
            observed: [
                (enabled_fact.clone(), assignment_verification_value(enabled)),
                (target_fact.clone(), assignment_verification_value(target)),
            ]
            .into_iter()
            .collect(),
            evidence: vec![
                DomainEvidenceId::new(format!("official_test.rule.{suffix}")).map_err(contract)?,
            ],
        });
    }
    control.check()?;
    Ok(evaluations)
}

fn authoritative_score(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    control: &eutheto_types::OperationControl,
) -> Result<ScoreVector, DomainPackError> {
    require_pack(document)?;
    solution.validate().map_err(contract)?;
    if solution.pack_id != document.domain_pack.id || solution.scenario_id != document.scenario_id {
        return Err(DomainPackError::Contract(
            "solution scenario mismatch".to_owned(),
        ));
    }
    let entities = parse_entities(document)?;
    let assignments: BTreeMap<_, _> = solution
        .assignments
        .iter()
        .map(|assignment| (assignment.id.as_str().to_owned(), &assignment.value))
        .collect();
    let expected_assignment_ids: BTreeSet<_> = entities
        .keys()
        .flat_map(|id| {
            let suffix = id.to_string();
            [
                format!("official.test.assignment.enabled.{suffix}"),
                format!("official.test.assignment.target.{suffix}"),
            ]
        })
        .collect();
    if assignments
        .keys()
        .any(|id| !expected_assignment_ids.contains(id))
    {
        return Err(contract("solution contains an unknown domain assignment"));
    }

    let mut feasibility = 0_i64;
    let mut total = 0_i64;
    for (id, entity) in &entities {
        control.check()?;
        let suffix = id.to_string();
        let enabled = assignments.get(&format!("official.test.assignment.enabled.{suffix}"));
        let target = assignments.get(&format!("official.test.assignment.target.{suffix}"));
        let satisfied = matches!(
            (enabled, target),
            (
                Some(AssignmentValue::Boolean(enabled)),
                Some(AssignmentValue::Integer(target))
            ) if *enabled == entity.enabled
                && *target == entity.target
                && *enabled == (*target >= 1)
        );
        if !satisfied {
            feasibility = feasibility
                .checked_add(1)
                .ok_or_else(|| contract("score feasibility overflow"))?;
        }
        if let Some(AssignmentValue::Integer(value)) = target {
            total = total
                .checked_add(*value)
                .ok_or_else(|| contract("score overflow"))?;
        }
    }
    control.check()?;
    Ok(ScoreVector {
        feasibility,
        levels: vec![ScoreLevelValue {
            level_id: ScoreLevelId::new(SCORE_LEVEL).map_err(contract)?,
            value: total,
            direction: OptimizationDirection::Minimize,
            category_breakdown: [(
                ScoreCategoryId::new(SCORE_CATEGORY).map_err(contract)?,
                total,
            )]
            .into_iter()
            .collect(),
        }],
    })
}

fn assignment_verification_value(value: Option<&AssignmentValue>) -> VerificationValue {
    match value {
        Some(AssignmentValue::Boolean(value)) => VerificationValue::Boolean(*value),
        Some(AssignmentValue::Integer(value)) => VerificationValue::Integer(*value),
        Some(AssignmentValue::Interval(value)) => VerificationValue::Text(format!(
            "interval:{}:{}:{}",
            value.start, value.duration, value.end
        )),
        Some(AssignmentValue::Absent) => VerificationValue::Text("absent".to_owned()),
        None => VerificationValue::Text("missing".to_owned()),
    }
}

fn payload(path: &str, error: impl std::fmt::Display) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: path.to_owned(),
        message: error.to_string(),
    }
}

fn contract(error: impl std::fmt::Display) -> DomainPackError {
    DomainPackError::Contract(error.to_string())
}

fn configure_entity(
    working: &mut ScenarioDocument,
    command: &ConfigureEntity,
) -> Result<(Value, DomainCommandEnvelope), DomainPackError> {
    check_target(command.target)?;
    if command.enabled != (command.target >= 1) {
        return Err(DomainPackError::InvalidPayload {
            path: "/commands/payload".to_owned(),
            message: "enabled must equal target >= 1".to_owned(),
        });
    }
    let previous_value = working
        .domain
        .entities
        .get(&command.entity_id)
        .cloned()
        .ok_or_else(|| DomainPackError::InvalidPayload {
            path: format!("/domain/entities/{}", command.entity_id),
            message: "configured entity does not exist".to_owned(),
        })?;
    let previous: TestEntity = serde_json::from_value(previous_value.clone())
        .map_err(|error| payload("/domain/entities", error))?;
    if previous.id != command.entity_id {
        return Err(DomainPackError::Contract(
            "entity map key/id mismatch".to_owned(),
        ));
    }
    let next = TestEntity {
        id: command.entity_id,
        enabled: command.enabled,
        target: command.target,
    };
    let next_value =
        serde_json::to_value(&next).map_err(|error| payload("/domain/entities", error))?;
    working
        .domain
        .entities
        .insert(command.entity_id, next_value.clone());
    let change = json!({
        "path": format!("/domain/entities/{}", command.entity_id),
        "before": previous_value,
        "after": next_value,
    });
    let inverse = DomainCommandEnvelope {
        command_type: CONFIGURE_ENTITY.to_owned(),
        payload: serde_json::to_value(ConfigureEntity {
            entity_id: previous.id,
            enabled: previous.enabled,
            target: previous.target,
        })
        .map_err(|error| payload("/inverse", error))?,
    };
    Ok((change, inverse))
}
#[cfg(test)]
mod tests {
    use super::{DomainPackError, ExplanationKind, VerificationValue, evidence_message_for_kind};
    use std::collections::BTreeSet;

    #[test]
    fn all_seven_evidence_kinds_render_to_distinct_typed_messages() -> Result<(), DomainPackError> {
        let kinds = [
            ExplanationKind::Validation,
            ExplanationKind::Infeasibility,
            ExplanationKind::Assignment,
            ExplanationKind::Counterfactual,
            ExplanationKind::SolutionDifference,
            ExplanationKind::Repair,
            ExplanationKind::OptimalityStatus,
        ];
        let mut keys = BTreeSet::new();
        for kind in kinds {
            let message = evidence_message_for_kind(kind, &"a".repeat(64))?;
            message.validate().map_err(super::contract)?;
            assert!(keys.insert(message.message_key));
            assert!(
                message
                    .parameters
                    .values()
                    .all(|value| matches!(value, VerificationValue::Text(_)))
            );
        }
        Ok(())
    }
}
