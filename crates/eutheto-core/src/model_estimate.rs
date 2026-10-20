use std::sync::Arc;

use super::{
    EuthetoApp, available_pack, domain_interruption, operation_interrupted, protocol_error,
    resource_limit_error, store_error, validation_error,
};
use eutheto_domain_api::{CompileContext, DomainPackError, DomainPackRegistry};
use eutheto_planning_ir::{
    MetadataKey, PLANNING_IR_SCHEMA_VERSION, PlanningIrLimitsV1, PlanningProblem,
    ProvenanceParameter, ValidationCode, validate,
};
use eutheto_types::{
    AppError, OperationControl, ParentSolveBudget, Revision, ScenarioId, UnsupportedFeature,
};
use serde::{Deserialize, Serialize};
/// Versioned, read-only request for a bounded model estimate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolveEstimateModelRequestV1 {
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
}

/// Exact counts from one validated compiled planning model. This is not a solve result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolveEstimateModelResultV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub scenario_revision: Revision,
    pub current_revision: Revision,
    pub stale: bool,
    /// Always true: the returned counts are from a completed model compilation.
    pub compiled: bool,
    /// Always false: this operation never invokes a backend or establishes feasibility.
    pub solved: bool,
    /// Workforce supplies this exact pack-owned pruning count; other packs are not applicable.
    pub candidate_count: Option<u64>,
    pub variable_count: u64,
    pub constraint_count: u64,
    pub projection_count: u64,
    pub objective_term_count: u64,
}

pub const SOLVE_ESTIMATE_MODEL_API_SCHEMA_VERSION: u32 = 1;
const WORKFORCE_PACK_ID: &str = "official.workforce";
const WORKFORCE_CANDIDATE_METADATA_KEY: &str = "official.workforce.candidates.after.availability";

impl EuthetoApp {
    /// Compiles an immutable stored scenario snapshot without invoking a solver or mutating
    /// durable state. The supplied parent budget is created before admission by the native
    /// operation registry and remains the one deadline for capture, compilation, and response.
    ///
    /// # Errors
    /// Returns a conflict for a stale expected revision, an unsupported error for an unavailable
    /// pack/schema, a validation error for an invalid model, or bounded cancellation/deadline and
    /// resource failures. No source document or pack-owned display data is returned.
    pub async fn estimate_stored_model(
        &self,
        request: SolveEstimateModelRequestV1,
        budget: ParentSolveBudget,
    ) -> Result<SolveEstimateModelResultV1, AppError> {
        let control = OperationControl::Solve(budget.phase_view());
        control.check().map_err(operation_interrupted)?;

        // Capture the authoritative document and revision while holding the same mutation lock as
        // scenario commands. The lock is released before any CPU-heavy pack compilation.
        let (scenario_revision, document, pack_id) = {
            let mutation = self.scenario_lock(request.scenario_id).await;
            let _guard = mutation.lock().await;
            control.check().map_err(operation_interrupted)?;
            let project = self
                .store
                .get_project(request.scenario_id)
                .await
                .map_err(store_error)?;
            if project.summary.revision != request.expected_revision {
                return Err(AppError::Conflict {
                    expected_revision: request.expected_revision,
                    actual_revision: project.summary.revision,
                });
            }
            if available_pack(&project.document, &self.pack_registry).is_none() {
                return Err(unsupported_estimate_pack(&project.document.domain_pack.id));
            }
            let pack_id = project.document.domain_pack.id.clone();
            (project.summary.revision, project.document, pack_id)
        };

        control.check().map_err(operation_interrupted)?;
        let registry = Arc::clone(&self.pack_registry);
        let worker_control = control.clone();
        let worker_pack_id = pack_id.clone();
        let limits = PlanningIrLimitsV1::DEFAULT;
        let worker_document = document;
        let problem = tokio::task::spawn_blocking(move || {
            worker_control.check().map_err(operation_interrupted)?;
            compile_estimate_problem(
                registry.as_ref(),
                &worker_document,
                &worker_pack_id,
                scenario_revision,
                &worker_control,
                limits,
            )
        })
        .await
        .map_err(|_| {
            protocol_error(
                "solve.estimate_failed",
                "The model estimate could not finish safely.",
                false,
            )
        })??;

        control.check().map_err(operation_interrupted)?;
        let counts = exact_counts(&problem, &pack_id)?;
        control.check().map_err(operation_interrupted)?;
        let current_revision = self.current_scenario_revision(request.scenario_id).await?;
        control.check().map_err(operation_interrupted)?;

        Ok(SolveEstimateModelResultV1 {
            schema_version: SOLVE_ESTIMATE_MODEL_API_SCHEMA_VERSION,
            scenario_id: request.scenario_id,
            scenario_revision,
            current_revision,
            stale: current_revision != scenario_revision,
            compiled: true,
            solved: false,
            candidate_count: counts.candidate,
            variable_count: counts.variables,
            constraint_count: counts.constraints,
            projection_count: counts.projections,
            objective_term_count: counts.objective_terms,
        })
    }
}

fn compile_estimate_problem(
    registry: &DomainPackRegistry,
    document: &eutheto_types::ScenarioDocument,
    pack_id: &eutheto_types::PackId,
    scenario_revision: Revision,
    control: &OperationControl,
    limits: PlanningIrLimitsV1,
) -> Result<PlanningProblem, AppError> {
    let pack = registry
        .require(pack_id)
        .map_err(|error| map_compile_error(&error, pack_id))?;
    let context = CompileContext {
        objective_context: Some(
            eutheto_domain_ir::RunObjectiveContext::authored(pack_id.clone())
                .map_err(|_| invalid_model())?,
        ),
        scenario_revision: scenario_revision.value(),
        semantic_metadata: std::collections::BTreeMap::new(),
        control: control.clone(),
        planning_limits: limits,
    };
    let problem = pack
        .compile(document, &context)
        .map_err(|error| map_compile_error(&error, pack_id))?;
    control.check().map_err(operation_interrupted)?;
    validate(&problem, limits).map_err(|error| match error.code {
        ValidationCode::LimitExceeded => resource_limit_error(),
        _ => invalid_model(),
    })?;
    if problem.schema_version != PLANNING_IR_SCHEMA_VERSION
        || &problem.metadata.pack_id != pack_id
        || problem.metadata.scenario_id != document.scenario_id
        || problem.metadata.scenario_revision != scenario_revision.value()
    {
        return Err(invalid_model());
    }
    Ok(problem)
}

struct ExactCounts {
    candidate: Option<u64>,
    variables: u64,
    constraints: u64,
    projections: u64,
    objective_terms: u64,
}

fn exact_counts(
    problem: &PlanningProblem,
    pack_id: &eutheto_types::PackId,
) -> Result<ExactCounts, AppError> {
    let variable_count = u64::try_from(problem.variables.len()).map_err(|_| invalid_model())?;
    let constraint_count = u64::try_from(problem.constraints.len()).map_err(|_| invalid_model())?;
    let projection_count = u64::try_from(problem.projections.len()).map_err(|_| invalid_model())?;
    let objective_term_count =
        problem
            .objectives
            .levels
            .iter()
            .try_fold(0_u64, |total, level| {
                let count = u64::try_from(level.terms.len()).map_err(|_| invalid_model())?;
                total.checked_add(count).ok_or_else(invalid_model)
            })?;
    let candidate = if pack_id.as_str() == WORKFORCE_PACK_ID {
        let key =
            MetadataKey::new(WORKFORCE_CANDIDATE_METADATA_KEY).map_err(|_| invalid_model())?;
        match problem.metadata.compile_metadata.get(&key) {
            Some(ProvenanceParameter::Integer(value)) if *value >= 0 => {
                Some(u64::try_from(*value).map_err(|_| invalid_model())?)
            }
            _ => return Err(invalid_model()),
        }
    } else {
        None
    };
    Ok(ExactCounts {
        candidate,
        variables: variable_count,
        constraints: constraint_count,
        projections: projection_count,
        objective_terms: objective_term_count,
    })
}

fn map_compile_error(error: &DomainPackError, pack_id: &eutheto_types::PackId) -> AppError {
    if let Some(interruption) = domain_interruption(error) {
        return interruption;
    }
    match error {
        DomainPackError::PackUnavailable(_) | DomainPackError::UnsupportedVersion(_) => {
            unsupported_estimate_pack(pack_id)
        }
        DomainPackError::ResourceLimitExceeded => resource_limit_error(),
        _ => invalid_model(),
    }
}

fn unsupported_estimate_pack(pack_id: &eutheto_types::PackId) -> AppError {
    AppError::Unsupported(UnsupportedFeature {
        code: "solve.model_estimate_unsupported".to_owned(),
        capability: format!("Domain pack {pack_id}"),
    })
}

fn invalid_model() -> AppError {
    validation_error(
        "solve.model_invalid",
        "/model",
        "The scenario could not be compiled into a supported model.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_workforce_candidate_count_is_explicitly_not_applicable()
    -> Result<(), Box<dyn std::error::Error>> {
        let pack_id = eutheto_types::PackId::new("fixture.other")?;
        // This behavior is intentionally independent of IR variable count or metadata shape.
        let problem = PlanningProblem {
            schema_version: PLANNING_IR_SCHEMA_VERSION,
            variables: Vec::new(),
            constraints: Vec::new(),
            objectives: eutheto_planning_ir::ObjectivePlan::default(),
            assumptions: Vec::new(),
            projections: Vec::new(),
            provenance: Vec::new(),
            metadata: eutheto_planning_ir::PlanningMetadata {
                pack_id: pack_id.clone(),
                scenario_id: ScenarioId::from_uuid(uuid::Uuid::now_v7()),
                scenario_revision: 0,
                projection_version: 1,
                compiler_id: eutheto_planning_ir::CompilerId::new("fixture.compiler")?,
                compiler_version: "fixture".to_owned(),
                compile_metadata: std::collections::BTreeMap::new(),
                display_text: std::collections::BTreeMap::new(),
            },
            declared_capabilities: std::collections::BTreeSet::new(),
            split_authorization: None,
        };
        let counts = exact_counts(&problem, &pack_id)
            .map_err(|error| std::io::Error::other(format!("other pack count: {error:?}")))?;
        assert_eq!(counts.candidate, None);
        Ok(())
    }

    #[test]
    fn workforce_candidate_count_requires_exact_integer_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let pack_id = eutheto_types::PackId::new(WORKFORCE_PACK_ID)?;
        let mut problem = PlanningProblem {
            schema_version: PLANNING_IR_SCHEMA_VERSION,
            variables: Vec::new(),
            constraints: Vec::new(),
            objectives: eutheto_planning_ir::ObjectivePlan::default(),
            assumptions: Vec::new(),
            projections: Vec::new(),
            provenance: Vec::new(),
            metadata: eutheto_planning_ir::PlanningMetadata {
                pack_id: pack_id.clone(),
                scenario_id: ScenarioId::from_uuid(uuid::Uuid::now_v7()),
                scenario_revision: 0,
                projection_version: 1,
                compiler_id: eutheto_planning_ir::CompilerId::new("fixture.compiler")?,
                compiler_version: "fixture".to_owned(),
                compile_metadata: std::collections::BTreeMap::new(),
                display_text: std::collections::BTreeMap::new(),
            },
            declared_capabilities: std::collections::BTreeSet::new(),
            split_authorization: None,
        };
        let key = MetadataKey::new(WORKFORCE_CANDIDATE_METADATA_KEY)?;
        problem
            .metadata
            .compile_metadata
            .insert(key.clone(), ProvenanceParameter::Integer(17));
        let counts = exact_counts(&problem, &pack_id)
            .map_err(|error| std::io::Error::other(format!("workforce metadata: {error:?}")))?;
        assert_eq!(counts.candidate, Some(17));
        problem
            .metadata
            .compile_metadata
            .insert(key, ProvenanceParameter::Text("17".to_owned()));
        assert!(matches!(
            exact_counts(&problem, &pack_id),
            Err(AppError::Validation(_))
        ));
        Ok(())
    }
}
