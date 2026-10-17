use super::{
    AssignmentAnalysis, AssignmentConstructionIssue, AssignmentModelEstimate, AssignmentRuleError,
    UnsupportedWorkforceObligation, WorkforceCompilation,
    analysis::{Plan, consecutive, hours, prepare, supported_rule},
    authority::check_objective_context,
    compiler::{
        DecisionVariables, compile_constraint, materialize_hard_lock_parents,
        materialize_rule_parents, variable_fact,
    },
    compiler_rank::{RankIds, RankIndex, emit_rank},
    compiler_weekend,
    identity::{IdentityKind, PlanningIdentities, RANK_LEVEL, WORKFORCE_PROJECTION_VERSION},
    input::AssignmentInput,
    ir_cost::{precharge_canonicalization, precharge_validation},
    preference::{self, PreferencePlan, emit as emit_preference, prepare as prepare_preferences},
    preference_consecutive, profile,
};
use crate::{
    assignment_rules::budget::{OperationBudget, count, effective_limits},
    model::{LockState, WorkforceEntity},
};
use eutheto_domain_api::{CompileContext, DomainPackError};
use eutheto_domain_ir::{OptimizationDirection, RunObjectiveContext};
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
pub(super) struct ModelInput<'a, 'p> {
    document: &'a ScenarioDocument,
    context: &'a CompileContext,
    assignment: &'a AssignmentInput,
    policy: Option<EntityId>,
    analysis: &'a AssignmentAnalysis,
    plan: &'a Plan,
    ranks: &'a RankIndex<'a>,
    preferences: &'p PreferencePlan<'a>,
    baseline_pairs: Option<&'p profile::SortedPairs<'a>>,
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
    context.control.check().map_err(|reason| match reason {
        eutheto_types::OperationInterruption::Cancelled => AssignmentRuleError::Cancelled,
        eutheto_types::OperationInterruption::DeadlineExceeded => {
            AssignmentRuleError::BudgetExpired
        }
    })?;
    let limits = effective_limits(context.planning_limits)?;
    let mut budget = OperationBudget::analysis(Some(&context.control), limits);
    check_objective_context(context.objective_context.as_ref(), document.scenario_id)
        .map_err(AssignmentRuleError::InvalidDocument)?;
    let input = AssignmentInput::new(document, &mut budget)?
        .with_objective_context(
            context.objective_context.as_ref(),
            document.scenario_id,
            &mut budget,
        )
        .map_err(AssignmentRuleError::InvalidDocument)?;
    let policy = require_supported(
        &input,
        document.scenario_id,
        context.objective_context.as_ref(),
        &mut budget,
    )?;
    check_metadata(context, limits, &mut budget)?;
    let (analysis, plan) = prepare(document, &input, &mut budget, limits)?;
    let preference_plan = prepare_preferences(
        document,
        &input,
        context.objective_context.as_ref(),
        limits,
        &mut budget,
    )?;
    let baseline_pairs = if preference_plan
        .layout
        .changes(context.objective_context.as_ref())
    {
        Some(
            profile::sorted_baseline_pairs(
                &input,
                document.scenario_id,
                context.objective_context.as_ref(),
                &mut budget,
            )
            .map_err(AssignmentRuleError::InvalidDocument)?
            .ok_or_else(|| {
                AssignmentRuleError::InvalidDocument(DomainPackError::Contract(
                    "official.workforce.authority.baseline_required".to_owned(),
                ))
            })?,
        )
    } else {
        profile::snapshot_pairs(
            &input,
            document.scenario_id,
            context.objective_context.as_ref(),
            &mut budget,
        )
        .map_err(AssignmentRuleError::InvalidDocument)?;
        None
    };
    let ranks = RankIndex::new(&input, &mut budget)?;
    let model_input = ModelInput {
        document,
        context,
        assignment: &input,
        policy,
        analysis: &analysis,
        plan: &plan,
        ranks: &ranks,
        preferences: &preference_plan,
        baseline_pairs: baseline_pairs.as_ref(),
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
    scenario_id: eutheto_types::ScenarioId,
    objective_context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<EntityId>, AssignmentRuleError> {
    let mut policy = None;
    for entity in input.domain.entities.values() {
        budget.step()?;
        if let WorkforceEntity::ScorePolicy(value) = entity {
            policy = Some(value.id);
        }
    }
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
    let mut active_preference = false;
    for preference in input.domain.preferences.values() {
        budget.step()?;
        let (id, active, ..) = preference.header();
        active_preference |= active;
        if active
            && !(matches!(
                preference,
                crate::model::WorkforcePreference::Time { .. }
                    | crate::model::WorkforcePreference::AssignmentType { .. }
                    | crate::model::WorkforcePreference::Location { .. }
                    | crate::model::WorkforcePreference::RequestedTimeOff { .. }
                    | crate::model::WorkforcePreference::AssignmentTarget { .. }
                    | crate::model::WorkforcePreference::WeekendTarget { .. }
                    | crate::model::WorkforcePreference::WorkloadBalance { .. }
                    | crate::model::WorkforcePreference::Adjacency { .. }
                    | crate::model::WorkforcePreference::BaseStability { .. }
                    | crate::model::WorkforcePreference::TogetherSeparate { .. }
                    | crate::model::WorkforcePreference::PreferredCoverage { .. }
            ) || matches!(
                preference,
                crate::model::WorkforcePreference::ConsecutiveWork { mode, .. }
                    if preference_consecutive::supports(mode)
            ))
        {
            let value = UnsupportedWorkforceObligation::Preference(id);
            unsupported = Some(unsupported.map_or(value, |prior| prior.min(value)));
        }
    }
    let kind = if policy.is_none() {
        profile::ProfileKind::parse(objective_context, scenario_id)
            .map_err(AssignmentRuleError::InvalidDocument)?
    } else {
        None
    };
    let policy = match policy {
        Some(policy) => Some(policy),
        None if kind.is_some_and(profile::ProfileKind::named) && !active_preference => None,
        None => return Err(AssignmentRuleError::MissingScorePolicy),
    };
    for lock in input.domain.locked_assignments.values() {
        budget.step()?;
        if matches!(lock.state, LockState::Soft { .. }) {
            if profile::supports_soft(objective_context) {
                continue;
            }
            if objective_context.is_some() {
                return Err(AssignmentRuleError::InvalidDocument(
                    DomainPackError::InvalidPayload {
                        path: "objectiveContext".to_owned(),
                        message:
                            "Soft locks require a named objective profile; Authored cannot score them"
                                .to_owned(),
                    },
                ));
            }
            let value = UnsupportedWorkforceObligation::SoftLock(lock.id);
            unsupported = Some(unsupported.map_or(value, |prior| prior.min(value)));
        }
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
        if value.is_empty()
            || count(key.len())? > limits.max_id_bytes
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

pub(super) fn level_fact(
    policy: Option<EntityId>,
    kind: Option<profile::ProfileKind>,
    id: ProvenanceId,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    let (source_id, message_key) = match policy {
        Some(policy) => (
            policy.to_string(),
            "official.workforce.assignment.rank.policy",
        ),
        None => (
            kind.filter(|kind| kind.named())
                .ok_or_else(invalid)?
                .id()
                .to_owned(),
            "official.workforce.assignment.rank.profile",
        ),
    };
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Fact,
        source_id,
        entity_refs: Vec::new(),
        message_key: message_key.to_owned(),
        parameters: BTreeMap::new(),
        parent: None,
    })
}

// Keep the model's objective, projections and source-bound constraint wiring in one construction.
#[allow(clippy::too_many_lines)]
fn construct(
    input: &ModelInput<'_, '_>,
    header: preflight::Header,
    budget: &mut OperationBudget<'_>,
) -> Result<PlanningProblem, AssignmentRuleError> {
    let ModelInput {
        document,
        context,
        assignment,
        policy,
        analysis,
        plan,
        ranks,
        preferences,
        baseline_pairs,
        limits,
    } = *input;
    let mut identities = PlanningIdentities::default();
    if header
        .profile
        .tiers
        .iter()
        .any(|tier| tier.tier == profile::PrefixTier::PairChange)
        != baseline_pairs.is_some()
    {
        return Err(invalid());
    }
    let rank_level_identity = if let Some(policy) = policy {
        identities.derive(
            IdentityKind::Provenance,
            &("assignment_rank_level", policy),
            budget,
        )?
    } else {
        let kind = preferences
            .layout
            .kind
            .filter(|kind| kind.named())
            .ok_or_else(invalid)?;
        identities.derive(
            IdentityKind::Provenance,
            &("assignment_rank_profile_level", kind.id()),
            budget,
        )?
    };
    let level_provenance = ProvenanceId::new(rank_level_identity).map_err(|_| invalid())?;
    let rank_fact = level_fact(policy, preferences.layout.kind, level_provenance.clone())?;
    let mut provenance = vec![rank_fact];
    let mut profile_level_provenance = Vec::with_capacity(header.profile.tiers.len());
    let mut profile_terms = Vec::with_capacity(header.profile.tiers.len());
    if !header.profile.tiers.is_empty() {
        let kind = preferences
            .layout
            .kind
            .filter(|kind| kind.named())
            .ok_or_else(invalid)?;
        for tier in &header.profile.tiers {
            let fact = profile::prefix_level_fact(kind, tier.tier, &mut identities, budget)?;
            profile_level_provenance.push(fact.id.clone());
            provenance.push(fact);
            profile_terms.push(Vec::with_capacity(tier.terms.len()));
        }
    }
    let mut preference_level_provenance = Vec::with_capacity(preferences.levels.len());
    for level in &preferences.levels {
        let policy = policy.ok_or_else(invalid)?;
        let id = ProvenanceId::new(identities.derive(
            IdentityKind::Provenance,
            &("preference_level", policy, &level.key),
            budget,
        )?)
        .map_err(|_| invalid())?;
        preference_level_provenance.push(id.clone());
        provenance.push(preference::level_fact(policy, level, id));
    }
    let variable_count = analysis
        .candidates
        .len()
        .checked_mul(2)
        .ok_or_else(invalid)?;
    let mut variables = Vec::with_capacity(variable_count);
    let mut weekend_variables = Vec::new();
    let mut constraints = Vec::new();
    let mut consecutive_variables = Vec::new();
    let mut projections = Vec::with_capacity(analysis.candidates.len());
    let mut terms = Vec::with_capacity(analysis.candidates.len());
    let mut preference_terms = (0..preferences.levels.len())
        .map(|_| Vec::new())
        .collect::<Vec<Vec<eutheto_planning_ir::ObjectiveTerm>>>();
    let mut decision_info = Vec::with_capacity(analysis.candidates.len());
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
        let integer = match &emitted.variable {
            Variable::Integer(variable) => variable.id.clone(),
            _ => return Err(invalid()),
        };
        decision_info.push((pair, decision.provenance.clone(), integer));
        variables.push(Variable::Boolean(decision));
        variables.push(emitted.variable);
        constraints.extend(emitted.channels);
        terms.push(emitted.term);
        projections.push(emitted.projection);
        provenance.push(emitted.fact);
    }
    for entry in &preferences.entries {
        for (pair, decision_provenance, integer) in &decision_info {
            budget.step()?;
            if !preference::matches(assignment, *entry, *pair, &document.settings, budget)? {
                continue;
            }
            let ids = preference::PreferenceIds::derive(*entry, *pair, &mut identities, budget)?;
            let emitted = emit_preference(
                *entry,
                integer.clone(),
                decision_provenance.clone(),
                ids,
                budget,
            )?;
            preference_terms
                .get_mut(entry.level)
                .ok_or_else(invalid)?
                .push(emitted.term);
            provenance.push(emitted.fact);
        }
    }
    for pruned in &header.preference.pruned_base {
        budget.step()?;
        let entry = *preferences.entries.get(pruned.entry).ok_or_else(invalid)?;
        let parent = preference_level_provenance
            .get(entry.level)
            .ok_or_else(invalid)?
            .clone();
        let ids = preference::PreferenceIds::derive(entry, pruned.pair, &mut identities, budget)?;
        let emitted = preference::emit_pruned_base(entry, parent, ids, budget)?;
        preference_terms
            .get_mut(entry.level)
            .ok_or_else(invalid)?
            .push(emitted.term);
        provenance.push(emitted.fact);
    }
    for target_shape in &header.preference.targets {
        budget.step()?;
        let target = preferences
            .targets
            .get(target_shape.target)
            .ok_or_else(invalid)?;
        let period = target
            .periods
            .get(target_shape.period)
            .ok_or_else(invalid)?;
        let parent = preference_level_provenance
            .get(target.level)
            .ok_or_else(invalid)?
            .clone();
        if target_shape.population.is_empty() {
            let ids =
                preference::TargetIds::derive_constant(target, period, &mut identities, budget)?;
            let emitted = preference::emit_constant_target(target, period, parent, ids, budget)?;
            preference_terms
                .get_mut(target.level)
                .ok_or_else(invalid)?
                .push(emitted.term);
            provenance.push(emitted.fact);
            continue;
        }
        let population = target_shape
            .population
            .iter()
            .map(|index| {
                budget.step()?;
                decision_info
                    .get(*index)
                    .map(|(_, _, integer)| integer.clone())
                    .ok_or_else(invalid)
            })
            .collect::<Result<Vec<_>, AssignmentRuleError>>()?;
        let ids = preference::TargetIds::derive(target, period, &mut identities, budget)?;
        let emitted = preference::emit_target(target, period, &population, parent, ids, budget)?;
        variables.extend(emitted.variables);
        constraints.extend(emitted.constraints);
        preference_terms
            .get_mut(target.level)
            .ok_or_else(invalid)?
            .push(emitted.term);
        provenance.push(emitted.fact);
    }
    for weekend_shape in &header.preference.weekend_targets {
        budget.step()?;
        let target = preferences
            .weekend_targets
            .get(weekend_shape.target)
            .ok_or_else(invalid)?;
        let person = target
            .people
            .get(weekend_shape.person)
            .ok_or_else(invalid)?;
        let parent = preference_level_provenance
            .get(target.level)
            .ok_or_else(invalid)?
            .clone();
        if weekend_shape.weekends.is_empty() {
            let ids = preference::WeekendTargetIds::derive_constant(
                target,
                person,
                &mut identities,
                budget,
            )?;
            let emitted =
                preference::emit_constant_weekend_target(target, person, parent, ids, budget)?;
            preference_terms
                .get_mut(target.level)
                .ok_or_else(invalid)?
                .push(emitted.term);
            provenance.push(emitted.fact);
            continue;
        }
        let populations = weekend_shape
            .weekends
            .iter()
            .map(|weekend| {
                weekend
                    .population
                    .iter()
                    .map(|index| {
                        budget.step()?;
                        decision_info
                            .get(*index)
                            .map(|(_, _, integer)| integer.clone())
                            .ok_or_else(invalid)
                    })
                    .collect::<Result<Vec<_>, AssignmentRuleError>>()
            })
            .collect::<Result<Vec<_>, AssignmentRuleError>>()?;
        let ids = preference::WeekendTargetIds::derive(target, person, &mut identities, budget)?;
        let emitted = preference::emit_weekend_target(
            target,
            person,
            weekend_shape,
            &populations,
            parent,
            ids,
            &mut identities,
            budget,
        )?;
        variables.extend(emitted.variables);
        constraints.extend(emitted.constraints);
        preference_terms
            .get_mut(target.level)
            .ok_or_else(invalid)?
            .push(emitted.term);
        provenance.push(emitted.fact);
    }
    for group_shape in &header.preference.groups {
        budget.step()?;
        let group = preferences
            .groups
            .get(group_shape.plan)
            .ok_or_else(invalid)?;
        let parent = preference_level_provenance
            .get(group.level)
            .ok_or_else(invalid)?
            .clone();
        for pair_shape in &group_shape.pairs {
            budget.step()?;
            let first = analysis
                .candidates
                .get(pair_shape.first)
                .copied()
                .ok_or_else(invalid)?;
            let second = analysis
                .candidates
                .get(pair_shape.second)
                .copied()
                .ok_or_else(invalid)?;
            let first_integer = decision_info
                .get(pair_shape.first)
                .map(|(_, _, integer)| integer)
                .ok_or_else(invalid)?;
            let second_integer = decision_info
                .get(pair_shape.second)
                .map(|(_, _, integer)| integer)
                .ok_or_else(invalid)?;
            let ids = preference::GroupIds::derive(group, first, second, &mut identities, budget)?;
            let emitted = preference::emit_group(
                group,
                first,
                second,
                first_integer,
                second_integer,
                parent.clone(),
                ids,
                budget,
            )?;
            variables.push(emitted.variable);
            constraints.extend(emitted.constraints);
            preference_terms
                .get_mut(group.level)
                .ok_or_else(invalid)?
                .push(emitted.term);
            provenance.push(emitted.fact);
        }
    }
    for coverage_shape in &header.preference.coverage {
        budget.step()?;
        let coverage_plan = preferences
            .coverage
            .get(coverage_shape.plan)
            .ok_or_else(invalid)?;
        let parent = preference_level_provenance
            .get(coverage_plan.level)
            .ok_or_else(invalid)?
            .clone();
        let population = coverage_shape
            .population
            .iter()
            .map(|index| {
                budget.step()?;
                decision_info
                    .get(*index)
                    .map(|(_, _, integer)| integer.clone())
                    .ok_or_else(invalid)
            })
            .collect::<Result<Vec<_>, AssignmentRuleError>>()?;
        let ids = preference::CoverageIds::derive(
            coverage_plan,
            coverage_shape,
            &mut identities,
            budget,
        )?;
        let emitted = preference::emit_coverage(
            coverage_plan,
            coverage_shape,
            &population,
            parent,
            ids,
            budget,
        )?;
        variables.extend(emitted.variables);
        constraints.extend(emitted.constraints);
        preference_terms
            .get_mut(coverage_plan.level)
            .ok_or_else(invalid)?
            .push(emitted.term);
        provenance.push(emitted.fact);
    }
    for period_shape in &header.preference.workloads.periods {
        budget.step()?;
        let workload = preferences
            .workloads
            .get(period_shape.workload)
            .ok_or_else(invalid)?;
        let period = workload
            .periods
            .get(period_shape.period)
            .ok_or_else(invalid)?;
        let parent = preference_level_provenance
            .get(workload.level)
            .ok_or_else(invalid)?
            .clone();
        let candidate_integers = decision_info
            .iter()
            .map(|(_, _, integer)| integer.clone())
            .collect::<Vec<_>>();
        let mut ids = Vec::with_capacity(period_shape.peers.len());
        for peer_shape in &period_shape.peers {
            budget.step()?;
            ids.push(preference::WorkloadIds::derive(
                workload,
                period,
                peer_shape,
                &mut identities,
                budget,
            )?);
        }
        let peer_actuals = ids
            .iter()
            .map(|value| value.actual.clone())
            .collect::<Vec<_>>();
        for (peer_shape, ids) in period_shape.peers.iter().zip(ids) {
            let emitted = preference::emit_workload(
                workload,
                period,
                peer_shape,
                &peer_actuals,
                &candidate_integers,
                parent.clone(),
                ids,
                budget,
            )?;
            variables.extend(emitted.variables);
            constraints.extend(emitted.constraints);
            preference_terms
                .get_mut(workload.level)
                .ok_or_else(invalid)?
                .push(emitted.term);
            provenance.push(emitted.fact);
        }
    }
    let candidate_booleans = (0..analysis.candidates.len())
        .map(|index| {
            budget.step()?;
            match variables.get(index.checked_mul(2).ok_or_else(invalid)?) {
                Some(Variable::Boolean(variable)) => Ok(variable.id.clone()),
                _ => Err(invalid()),
            }
        })
        .collect::<Result<Vec<_>, AssignmentRuleError>>()?;
    for window_shape in &header.preference.consecutives.windows {
        budget.step()?;
        let consecutive = preferences
            .consecutives
            .get(window_shape.plan)
            .ok_or_else(invalid)?;
        let parent = preference_level_provenance
            .get(consecutive.level)
            .ok_or_else(invalid)?
            .clone();
        let ids = preference_consecutive::ids(consecutive, window_shape, &mut identities, budget)?;
        let emitted = preference_consecutive::emit(
            consecutive,
            window_shape,
            ids,
            &candidate_booleans,
            parent,
            budget,
        )?;
        variables.extend(emitted.variables);
        constraints.extend(emitted.constraints);
        preference_terms
            .get_mut(consecutive.level)
            .ok_or_else(invalid)?
            .push(emitted.term);
        provenance.push(emitted.fact);
    }
    let parents = materialize_rule_parents(plan, &mut identities, &mut provenance, budget)?;
    let hard_lock_parents =
        materialize_hard_lock_parents(plan, &mut identities, &mut provenance, budget)?;
    for planned in &plan.constraints {
        let parent = match planned.predicate {
            super::analysis::Predicate::HardLock { assignment, .. } => hard_lock_parents
                .get(&assignment)
                .ok_or_else(invalid)?
                .clone(),
            _ => parents.get(&planned.rule).ok_or_else(invalid)?.clone(),
        };
        if compiler_weekend::is(planned) {
            compiler_weekend::emit(
                planned,
                plan,
                DecisionVariables::Complete {
                    pairs: &analysis.candidates,
                    variables: &variables,
                },
                parent,
                &mut identities,
                &mut weekend_variables,
                &mut constraints,
                &mut provenance,
                budget,
                limits,
            )?;
        } else if consecutive::is(planned) {
            consecutive::emit(
                planned,
                plan,
                DecisionVariables::Complete {
                    pairs: &analysis.candidates,
                    variables: &variables,
                },
                parent,
                &mut identities,
                &mut consecutive_variables,
                &mut constraints,
                &mut provenance,
                budget,
                limits,
            )?;
        } else {
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
    }
    variables.extend(weekend_variables);
    variables.extend(consecutive_variables);
    let pair_ints = (0..analysis.candidates.len())
        .map(|index| {
            let position = index.checked_mul(2).and_then(|value| value.checked_add(1));
            match position.and_then(|value| variables.get(value)) {
                Some(Variable::Integer(variable)) => Ok((index, variable.id.clone())),
                _ => Err(invalid()),
            }
        })
        .collect::<Result<BTreeMap<_, _>, AssignmentRuleError>>()?;
    hours::compile(
        plan,
        &pair_ints,
        &parents,
        &mut identities,
        &mut variables,
        &mut constraints,
        &mut provenance,
        budget,
        limits,
    )?;
    for (tier_index, tier_shape) in header.profile.tiers.iter().enumerate() {
        let parent = profile_level_provenance
            .get(tier_index)
            .ok_or_else(invalid)?
            .clone();
        for term_shape in &tier_shape.terms {
            budget.step()?;
            let integer = if let Some(candidate_index) = term_shape.candidate_index {
                let (pair, _, integer) = decision_info.get(candidate_index).ok_or_else(invalid)?;
                if *pair != term_shape.pair {
                    return Err(invalid());
                }
                Some(integer.clone())
            } else {
                None
            };
            let (term, fact) = profile::emit_term(
                *term_shape,
                integer,
                parent.clone(),
                &mut identities,
                budget,
            )?;
            profile_terms
                .get_mut(tier_index)
                .ok_or_else(invalid)?
                .push(term);
            provenance.push(fact);
        }
    }
    let level_count = header
        .profile
        .tiers
        .len()
        .checked_add(preferences.levels.len())
        .and_then(|value| value.checked_add(1))
        .ok_or_else(invalid)?;
    let mut levels = Vec::with_capacity(level_count);
    for (index, tier) in header.profile.tiers.iter().enumerate() {
        levels.push(ObjectiveLevel {
            id: ObjectiveLevelId::new(tier.tier.id()).map_err(|_| invalid())?,
            direction: OptimizationDirection::Minimize,
            lower_bound: tier.lower_bound,
            upper_bound: tier.upper_bound,
            terms: std::mem::take(profile_terms.get_mut(index).ok_or_else(invalid)?),
            provenance: profile_level_provenance
                .get(index)
                .ok_or_else(invalid)?
                .clone(),
        });
    }
    for (index, level) in preferences.levels.iter().enumerate() {
        levels.push(ObjectiveLevel {
            id: ObjectiveLevelId::new(level.id.clone()).map_err(|_| invalid())?,
            direction: OptimizationDirection::Minimize,
            lower_bound: *header
                .preference
                .level_lower_bounds
                .get(index)
                .ok_or_else(invalid)?,
            upper_bound: *header
                .preference
                .level_upper_bounds
                .get(index)
                .ok_or_else(invalid)?,
            terms: std::mem::take(preference_terms.get_mut(index).ok_or_else(invalid)?),
            provenance: preference_level_provenance
                .get(index)
                .ok_or_else(invalid)?
                .clone(),
        });
    }
    levels.push(ObjectiveLevel {
        id: ObjectiveLevelId::new(RANK_LEVEL).map_err(|_| invalid())?,
        direction: OptimizationDirection::Minimize,
        lower_bound: 0,
        upper_bound: header.rank_upper_bound,
        terms,
        provenance: level_provenance,
    });
    Ok(PlanningProblem {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables,
        constraints,
        objectives: ObjectivePlan { levels },
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
