use super::{
    AssignmentAnalysis, AssignmentConstructionIssue, AssignmentModelEstimate, AssignmentRuleError,
    UnsupportedWorkforceObligation, WorkforceCompilation,
    analysis::{Plan, prepare, supported_rule},
    budget::{OperationBudget, count, effective_limits},
    compiler::{DecisionVariables, compile_constraint, rule_fact, variable_fact},
    compiler_rank::{RANK_LEVEL, RankIds, RankIndex, WORKFORCE_PROJECTION_VERSION, emit_rank},
    identity::{IdentityKind, PlanningIdentities},
    input::AssignmentInput,
    ir_cost::{precharge_canonicalization, precharge_validation},
};
use crate::model::{LockState, WorkforceEntity};
use eutheto_domain_api::CompileContext;
use eutheto_domain_ir::OptimizationDirection;
use eutheto_planning_ir::{
    BoolVariable, BoolVariableId, CompilerId, MetadataKey, ObjectiveLevel, ObjectiveLevelId,
    ObjectivePlan, PLANNING_IR_SCHEMA_VERSION, PlanningIrLimitsV1, PlanningMetadata,
    PlanningProblem, ProvenanceId, ProvenanceParameter, ProvenanceRecord, ProvenanceSourceKind,
    Variable, validate,
};
use eutheto_types::{EntityId, PackId, ScenarioDocument};
use std::collections::BTreeMap;

#[path = "compiler_model_preflight.rs"]
mod preflight;

#[cfg(test)]
#[path = "compiler_model_tests.rs"]
mod tests;

pub(super) const COMPILER_ID: &str = "official.workforce.compiler";
pub(super) const RESERVED_METADATA: [&str; 7] = [
    "official.workforce.rank.version",
    "official.workforce.candidates.inspected",
    "official.workforce.candidates.after.activity",
    "official.workforce.candidates.after.assignment.type",
    "official.workforce.candidates.after.qualification",
    "official.workforce.candidates.after.availability",
    "official.workforce.rejection.facts",
];

#[derive(Clone, Copy)]
pub(super) struct ModelInput<'a> {
    document: &'a ScenarioDocument,
    context: &'a CompileContext,
    policy: EntityId,
    analysis: &'a AssignmentAnalysis,
    plan: &'a Plan,
    ranks: &'a RankIndex<'a>,
    limits: PlanningIrLimitsV1,
}

/// Compile the complete supported Workforce subset without registering or accepting a pack.
/// Rejections remain source-bound diagnostics beside, not orphan provenance inside, the IR.
///
/// # Errors
/// Rejects invalid drafts, missing score policy, active unsupported obligations, reserved or
/// malformed metadata, cancellation and cumulative construction/resource failures atomically.
pub fn compile_workforce(
    document: &ScenarioDocument,
    context: &CompileContext,
) -> Result<WorkforceCompilation, AssignmentRuleError> {
    if context.cancellation.is_cancelled() {
        return Err(AssignmentRuleError::Cancelled);
    }
    let limits = effective_limits(context.planning_limits)?;
    let mut budget = OperationBudget::analysis(Some(&context.cancellation), limits);
    let input = AssignmentInput::new(document, &mut budget)?;
    let policy = require_supported(&input, &mut budget)?;
    check_metadata(context, limits, &mut budget)?;
    let (analysis, plan) = prepare(document, &input, &mut budget, limits)?;
    let ranks = RankIndex::new(&input, &mut budget)?;
    let model_input = ModelInput {
        document,
        context,
        policy,
        analysis: &analysis,
        plan: &plan,
        ranks: &ranks,
        limits,
    };
    let header = preflight::prepare(&model_input, &mut budget)?;
    let mut problem = construct(&model_input, header, &mut budget)?;
    // These generic routines are finite, separately precharged indivisible phases.
    precharge_canonicalization(&problem, &mut budget)?;
    budget.check()?;
    problem.canonicalize().map_err(|_| invalid())?;
    budget.check()?;
    precharge_validation(&problem, &mut budget)?;
    budget.check()?;
    validate(&problem, limits).map_err(|_| invalid())?;
    budget.check()?;
    Ok(WorkforceCompilation {
        problem,
        source_document_hash: analysis.source_document_hash,
        rejections: analysis.rejections,
        validation: analysis.validation,
    })
}

fn require_supported(
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
) -> Result<EntityId, AssignmentRuleError> {
    let mut policy = None;
    for entity in input.domain.entities.values() {
        budget.step()?;
        if let WorkforceEntity::ScorePolicy(value) = entity {
            policy = Some(value.id);
        }
    }
    let policy = policy.ok_or(AssignmentRuleError::MissingScorePolicy)?;
    // Enum order fixes category precedence; typed IDs fix order within a category.
    // This does not reinterpret Hard-lock AssignmentIds as Required RuleIds.
    let mut unsupported: Option<UnsupportedWorkforceObligation> = None;
    for rule in input.domain.rules.values() {
        budget.step()?;
        let (id, active, _) = rule.header();
        if active && !supported_rule(rule) {
            let value = UnsupportedWorkforceObligation::RequiredRule(id);
            unsupported = Some(unsupported.map_or(value, |prior| prior.min(value)));
        }
    }
    for preference in input.domain.preferences.values() {
        budget.step()?;
        let (id, active, ..) = preference.header();
        if active {
            let value = UnsupportedWorkforceObligation::Preference(id);
            unsupported = Some(unsupported.map_or(value, |prior| prior.min(value)));
        }
    }
    for lock in input.domain.locked_assignments.values() {
        budget.step()?;
        let value = match lock.state {
            LockState::Hard {} => UnsupportedWorkforceObligation::HardLock(lock.id),
            LockState::Soft { .. } => UnsupportedWorkforceObligation::SoftLock(lock.id),
            LockState::Unlocked {} => continue,
        };
        unsupported = Some(unsupported.map_or(value, |prior| prior.min(value)));
    }
    if let Some(value) = unsupported {
        return Err(AssignmentRuleError::UnsupportedObligation(value));
    }
    Ok(policy)
}

fn check_metadata(
    context: &CompileContext,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for (key, value) in &context.semantic_metadata {
        budget.step()?;
        if let Some(reserved) = RESERVED_METADATA
            .iter()
            .find(|reserved| key.as_str() == **reserved)
        {
            return Err(AssignmentRuleError::ReservedSemanticMetadata(reserved));
        }
        if count(key.len())? > limits.max_id_bytes
            || count(value.len())? > limits.max_metadata_text_bytes
        {
            return Err(AssignmentRuleError::InvalidSemanticMetadata);
        }
        budget.reserve(0, 1, count(key.len())?)?;
        MetadataKey::new(key.clone()).map_err(|_| AssignmentRuleError::InvalidSemanticMetadata)?;
    }
    Ok(())
}

pub(super) fn metadata_values(estimate: &AssignmentModelEstimate) -> [u64; 7] {
    [
        1,
        estimate.inspected_pairs,
        estimate.after_activity_pruning,
        estimate.after_assignment_type_pruning,
        estimate.after_qualification_pruning,
        estimate.after_availability_pruning,
        estimate.rejection_facts,
    ]
}

fn metadata(
    document: &ScenarioDocument,
    context: &CompileContext,
    estimate: &AssignmentModelEstimate,
    budget: &mut OperationBudget<'_>,
) -> Result<PlanningMetadata, AssignmentRuleError> {
    let mut compile_metadata = BTreeMap::new();
    for (key, value) in &context.semantic_metadata {
        budget.step()?;
        compile_metadata.insert(
            MetadataKey::new(key.clone()).map_err(|_| invalid())?,
            ProvenanceParameter::Text(value.clone()),
        );
    }
    for (key, value) in RESERVED_METADATA.into_iter().zip(metadata_values(estimate)) {
        budget.step()?;
        compile_metadata.insert(
            MetadataKey::new(key).map_err(|_| invalid())?,
            ProvenanceParameter::Integer(i64::try_from(value).map_err(|_| invalid())?),
        );
    }
    Ok(PlanningMetadata {
        pack_id: PackId::new("official.workforce").map_err(|_| invalid())?,
        scenario_id: document.scenario_id,
        scenario_revision: context.scenario_revision,
        projection_version: WORKFORCE_PROJECTION_VERSION,
        compiler_id: CompilerId::new(COMPILER_ID).map_err(|_| invalid())?,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        compile_metadata,
        display_text: BTreeMap::new(),
    })
}

pub(super) fn level_fact(policy: EntityId, id: ProvenanceId) -> ProvenanceRecord {
    ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Fact,
        source_id: policy.to_string(),
        entity_refs: Vec::new(),
        message_key: "official.workforce.assignment.rank.policy".to_owned(),
        parameters: BTreeMap::new(),
        parent: None,
    }
}

fn construct(
    input: &ModelInput<'_>,
    header: preflight::Header,
    budget: &mut OperationBudget<'_>,
) -> Result<PlanningProblem, AssignmentRuleError> {
    let ModelInput {
        document,
        context,
        policy,
        analysis,
        plan,
        ranks,
        limits,
    } = *input;
    let mut identities = PlanningIdentities::default();
    let level_provenance = ProvenanceId::new(identities.derive(
        IdentityKind::Provenance,
        &("assignment_rank_level", policy),
        budget,
    )?)
    .map_err(|_| invalid())?;
    let variable_count = analysis
        .candidates
        .len()
        .checked_mul(2)
        .ok_or_else(invalid)?;
    let mut variables = Vec::with_capacity(variable_count);
    let mut constraints = Vec::new();
    let mut projections = Vec::with_capacity(analysis.candidates.len());
    let mut provenance = vec![level_fact(policy, level_provenance.clone())];
    let mut terms = Vec::with_capacity(analysis.candidates.len());
    for &pair in &analysis.candidates {
        budget.step()?;
        let key = ("assignment", pair.person_id, pair.shift_id);
        let decision = BoolVariable {
            id: BoolVariableId::new(identities.derive(IdentityKind::Boolean, &key, budget)?)
                .map_err(|_| invalid())?,
            provenance: ProvenanceId::new(identities.derive(
                IdentityKind::Provenance,
                &key,
                budget,
            )?)
            .map_err(|_| invalid())?,
        };
        provenance.push(variable_fact(pair, decision.provenance.clone(), budget)?);
        let rank = ranks.rank(pair, limits, budget)?;
        let ids = RankIds::derive(pair, &mut identities, budget)?;
        let emitted = emit_rank(pair, rank, &decision, ids, budget)?;
        variables.push(Variable::Boolean(decision));
        variables.push(emitted.variable);
        constraints.extend(emitted.channels);
        terms.push(emitted.term);
        projections.push(emitted.projection);
        provenance.push(emitted.fact);
    }
    let mut parents = BTreeMap::new();
    for (rule, kind) in &plan.parents {
        budget.step()?;
        let id = ProvenanceId::new(identities.derive(
            IdentityKind::Provenance,
            &("rule", rule, kind),
            budget,
        )?)
        .map_err(|_| invalid())?;
        budget.reserve(1, 1, 128)?;
        parents.insert(*rule, id.clone());
        provenance.push(rule_fact(*rule, kind, id));
    }
    for planned in &plan.constraints {
        let parent = parents.get(&planned.rule).ok_or_else(invalid)?.clone();
        let (record, fact) = compile_constraint(
            planned,
            plan,
            DecisionVariables::Complete {
                pairs: &analysis.candidates,
                variables: &variables,
            },
            parent,
            &mut identities,
            budget,
        )?;
        constraints.push(record);
        provenance.push(fact);
    }
    Ok(PlanningProblem {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables,
        constraints,
        objectives: ObjectivePlan {
            levels: vec![ObjectiveLevel {
                id: ObjectiveLevelId::new(RANK_LEVEL).map_err(|_| invalid())?,
                direction: OptimizationDirection::Minimize,
                lower_bound: 0,
                upper_bound: header.upper_bound,
                terms,
                provenance: level_provenance,
            }],
        },
        assumptions: Vec::new(),
        projections,
        provenance,
        metadata: metadata(document, context, &analysis.estimate, budget)?,
        declared_capabilities: header.capabilities,
        split_authorization: None,
    })
}

fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}
