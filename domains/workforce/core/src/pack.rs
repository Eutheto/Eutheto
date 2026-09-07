use crate::{
    assignment_rules::{
        AssignmentRuleError, build_workforce_share_result, compile_workforce, operation_error,
        project_workforce_candidate, render_workforce_evidence, score_workforce_solution,
        validate_input_bounds, validate_workforce_full, verify_workforce_solution,
        workforce_verification_scope,
    },
    commands,
    generated_workforce_pack_contract::{
        WORKFORCE_COMMAND_IDS, WORKFORCE_PACK_CONTRACT_JSON, WORKFORCE_PACK_ID,
        WORKFORCE_PACK_VERSION,
    },
    portable,
    validation::validate_document,
};
use eutheto_domain_api::{
    AiToolDescriptor, CommandDescriptor, CompileContext, CounterfactualCompileContext,
    DomainBatchCommand, DomainCapability, DomainCatalog, DomainMutation, DomainPack,
    DomainPackDescriptor, DomainPackError, DomainShareResult, DomainUiManifest,
    DomainValidationReport, DomainView, LicenseMetadata, LocalizedText, PortableImportContext,
    SchemaVersionDescriptor, ShareResultOptions,
};
use eutheto_domain_ir::{
    AcceptedResult, CounterfactualConditionV1, EvidenceRenderRequestV1, EvidenceRenderResultV1,
    ExplanationCapability, NormalizedSolution, ScoreVector, VerificationContextV1,
    VerificationReport, VerificationScope,
};
use eutheto_planning_ir::{CandidateValues, PlanningIrLimitsV1, PlanningProblem};
use eutheto_types::{
    PackId, PortableDomainDocument, ScenarioDocument, ScenarioDomain, SemanticCapability,
    SolutionId,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

/// Real Workforce domain hooks, available for conformance without production registration.
///
/// The current executable subset rejects active later-rule/preferences and hard/soft locks.
/// Views and the five later explanation kinds are explicitly unsupported, not fabricated.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkforcePack;

impl DomainPack for WorkforcePack {
    fn descriptor(&self) -> Result<DomainPackDescriptor, DomainPackError> {
        Ok(DomainPackDescriptor {
            id: PackId::new(WORKFORCE_PACK_ID).map_err(|_| generated_error())?,
            display_name: text("official.workforce.name", "Workforce"),
            description: text(
                "official.workforce.description",
                "Workforce assignment planning and independent verification.",
            ),
            pack_version: WORKFORCE_PACK_VERSION
                .parse()
                .map_err(|_| generated_error())?,
            scenario_versions: SchemaVersionDescriptor {
                latest: 1,
                migratable_from: BTreeSet::new(),
            },
            icon_id: "official.workforce.icon".to_owned(),
            capabilities: [
                DomainCapability::Commands,
                DomainCapability::Compilation,
                DomainCapability::Projection,
                DomainCapability::Verification,
                DomainCapability::Scoring,
                DomainCapability::PortableData,
                DomainCapability::ShareResult,
                DomainCapability::AiTools,
            ]
            .into_iter()
            .collect(),
            portable_versions: SchemaVersionDescriptor {
                latest: 1,
                migratable_from: BTreeSet::new(),
            },
            explanation_capabilities: [
                ExplanationCapability::Validation,
                ExplanationCapability::Assignment,
            ]
            .into_iter()
            .collect(),
            portable_capabilities: [SemanticCapability {
                id: "official.workforce.portable".to_owned(),
                version: 1,
            }]
            .into_iter()
            .collect(),
            share_result_schema_version: 1,
            documentation_url: None,
            license: LicenseMetadata {
                spdx_expression: "Apache-2.0".to_owned(),
                attribution: "Eutheto contributors".to_owned(),
            },
            synthetic_test_only: false,
        })
    }

    fn catalog(&self) -> Result<DomainCatalog, DomainPackError> {
        generated_catalog()
    }

    fn new_document(
        &self,
        mut shell: ScenarioDocument,
    ) -> Result<ScenarioDocument, DomainPackError> {
        validate_input_bounds(&shell)?;
        if shell.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
            return Err(DomainPackError::InvalidPayload {
                path: "domainPack.id".to_owned(),
                message: "expected the Workforce domain pack".to_owned(),
            });
        }
        if shell.domain_pack.schema_version != 1 {
            return Err(DomainPackError::UnsupportedVersion(
                shell.domain_pack.schema_version,
            ));
        }
        shell.domain = ScenarioDomain::default();
        validate_document(&shell)?;
        Ok(shell)
    }

    fn migrate_document(
        &self,
        document: ScenarioDocument,
    ) -> Result<ScenarioDocument, DomainPackError> {
        validate_input_bounds(&document)?;
        validate_document(&document)?;
        Ok(document)
    }

    fn validate_fast(&self, document: &ScenarioDocument) -> DomainValidationReport {
        match validate_input_bounds(document).and_then(|()| validate_document(document).map(|_| ()))
        {
            Ok(()) => DomainValidationReport::default(),
            Err(error) => DomainValidationReport {
                issues: vec![AssignmentRuleError::InvalidDocument(error).validation_issue()],
            },
        }
    }

    fn validate_full(&self, document: &ScenarioDocument) -> DomainValidationReport {
        validate_workforce_full(document)
    }

    fn apply_batch(
        &self,
        document: &ScenarioDocument,
        batch: &DomainBatchCommand,
    ) -> Result<DomainMutation, DomainPackError> {
        validate_input_bounds(document)?;
        validate_input_bounds(batch)?;
        commands::apply_batch(document, batch)
    }

    fn compile(
        &self,
        document: &ScenarioDocument,
        context: &CompileContext,
    ) -> Result<PlanningProblem, DomainPackError> {
        if context.cancellation.is_cancelled() {
            return Err(DomainPackError::Cancelled);
        }
        validate_input_bounds(document)?;
        validate_input_bounds(&context.semantic_metadata)?;
        compile_workforce(document, context)
            .map(|compiled| compiled.problem)
            .map_err(|error| operation_error(&error))
    }

    fn project(
        &self,
        problem: &PlanningProblem,
        candidate: &CandidateValues,
        solution_id: SolutionId,
    ) -> Result<NormalizedSolution, DomainPackError> {
        project_workforce_candidate(problem, candidate, solution_id, PlanningIrLimitsV1::DEFAULT)
    }

    fn verification_scope(
        &self,
        document: &ScenarioDocument,
        scenario_revision: u64,
    ) -> Result<VerificationScope, DomainPackError> {
        workforce_verification_scope(document, scenario_revision, None)
    }

    fn verify(
        &self,
        document: &ScenarioDocument,
        solution: &NormalizedSolution,
        context: &VerificationContextV1,
        authoritative_score: &ScoreVector,
    ) -> Result<VerificationReport, DomainPackError> {
        verify_workforce_solution(document, solution, context, authoritative_score, None)
    }

    fn score(
        &self,
        document: &ScenarioDocument,
        solution: &NormalizedSolution,
    ) -> Result<ScoreVector, DomainPackError> {
        score_workforce_solution(document, solution, None)
    }

    fn export_portable(
        &self,
        document: &ScenarioDocument,
    ) -> Result<PortableDomainDocument, DomainPackError> {
        validate_input_bounds(document)?;
        portable::export_portable(document)
    }

    fn migrate_portable_step(
        &self,
        document: PortableDomainDocument,
    ) -> Result<PortableDomainDocument, DomainPackError> {
        // No historical Workforce portable versions exist; even current v1 has no next step.
        Err(DomainPackError::UnsupportedVersion(document.schema_version))
    }

    fn import_portable(
        &self,
        document: &PortableDomainDocument,
        context: &PortableImportContext,
    ) -> Result<ScenarioDocument, DomainPackError> {
        validate_input_bounds(document)?;
        validate_input_bounds(&context.scenario_shell)?;
        portable::import_portable(document, context)
    }

    fn build_share_result(
        &self,
        document: &ScenarioDocument,
        accepted: &AcceptedResult,
        options: ShareResultOptions,
    ) -> Result<DomainShareResult, DomainPackError> {
        build_workforce_share_result(document, accepted, options)
    }

    fn build_view(
        &self,
        _document: &ScenarioDocument,
        _solution: Option<&NormalizedSolution>,
        _view_id: &str,
    ) -> Result<DomainView, DomainPackError> {
        Err(DomainPackError::InvalidPayload {
            path: "/viewId".to_owned(),
            message: "unknown Workforce view".to_owned(),
        })
    }

    fn render_evidence(
        &self,
        document: &ScenarioDocument,
        request: &EvidenceRenderRequestV1,
    ) -> Result<EvidenceRenderResultV1, DomainPackError> {
        render_workforce_evidence(document, request)
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

fn generated_catalog() -> Result<DomainCatalog, DomainPackError> {
    let generated: GeneratedContract =
        serde_json::from_str(WORKFORCE_PACK_CONTRACT_JSON).map_err(|_| generated_error())?;
    if generated.schema_version != 3
        || generated.pack.id != WORKFORCE_PACK_ID
        || generated.pack.pack_version != WORKFORCE_PACK_VERSION
        || generated.pack.latest_schema_version != 1
        || generated.pack.portable_schema_version != 1
        || generated.pack.share_result_schema_version != 1
        || !generated
            .commands
            .iter()
            .map(|command| command.id.as_str())
            .eq(WORKFORCE_COMMAND_IDS.iter().copied())
    {
        return Err(generated_error());
    }
    let ai_tools = generated
        .ai_tools
        .into_iter()
        .map(|tool| {
            let command = generated
                .commands
                .iter()
                .find(|command| command.id == tool.command_id)
                .ok_or_else(generated_error)?;
            Ok(AiToolDescriptor {
                command_id: tool.command_id,
                name: tool.name,
                description: tool.description,
                input_schema: command.payload_schema.clone(),
                valid_examples: command.valid_examples.clone(),
            })
        })
        .collect::<Result<Vec<_>, DomainPackError>>()?;
    Ok(DomainCatalog {
        pack_id: PackId::new(WORKFORCE_PACK_ID).map_err(|_| generated_error())?,
        scenario_schema_version: generated.pack.latest_schema_version,
        internal_schema: generated.internal_schema,
        portable_schema: generated.portable_schema,
        share_result_schema: generated.share_result_schema,
        commands: generated.commands,
        ai_tools,
        ui: generated.ui_manifest,
    })
}

fn generated_error() -> DomainPackError {
    DomainPackError::CatalogMismatch("generated Workforce contract".to_owned())
}

fn text(key: &str, default_text: &str) -> LocalizedText {
    LocalizedText {
        key: key.to_owned(),
        default_text: default_text.to_owned(),
    }
}
