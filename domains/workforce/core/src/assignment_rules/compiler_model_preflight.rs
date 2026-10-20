use super::super::{
    AssignmentRuleLimit,
    analysis::{Plan, PlannedConstraint, Predicate, consecutive, hours},
    budget::{add, count, within},
    compiler::{BOOL_ID, CONSTRAINT_ID, PROVENANCE_ID, preflight, reserve_fact},
    compiler_rank::{RankIds, RankIndex, emit_rank},
    compiler_weekend,
    identity::PlanningIdentities,
    preference::{
        self, CoverageIds, GroupIds, PreferenceIds, PreferencePlan, PreferenceShape, TargetIds,
        WeekendTargetIds, emit as emit_preference, emit_constant_target,
        emit_constant_weekend_target, emit_coverage, emit_group, emit_target, emit_weekend_target,
        shape as preference_shape,
    },
    preference_consecutive, profile,
};
use super::{
    AssignmentAnalysis, AssignmentRuleError, COMPILER_ID, CompileContext, ModelInput,
    OperationBudget, PlanningIrLimitsV1, RANK_LEVEL, RESERVED_METADATA,
    WORKFORCE_PROJECTION_VERSION, invalid, level_fact, metadata_values,
};
use eutheto_domain_ir::OptimizationDirection;
use eutheto_planning_ir::{
    BoolVariable, BoolVariableId, Capability, IntVariableId, ObjectiveTermId,
    PLANNING_IR_SCHEMA_VERSION, PlanningConstraintId, ProjectionId, ProvenanceId,
    ProvenanceParameter,
};
use serde::{Serialize, Serializer, ser::SerializeMap};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Header {
    pub rank_upper_bound: i64,
    pub profile: profile::Shape,
    pub preference: PreferenceShape,
    pub capabilities: BTreeSet<Capability>,
}

#[allow(clippy::too_many_lines)]
pub(super) fn prepare(
    input: &ModelInput<'_, '_>,
    budget: &mut OperationBudget<'_>,
) -> Result<Header, AssignmentRuleError> {
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
    let pairs = count(analysis.candidates.len())?;
    let mut rank_upper_bound = 0_i64;
    for &pair in &analysis.candidates {
        let rank = ranks.rank(pair, limits, budget)?;
        rank_upper_bound = rank_upper_bound.checked_add(rank).ok_or_else(invalid)?;
        if rank_upper_bound > limits.max_abs_value {
            return Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord,
            ));
        }
    }
    let preference = preference_shape(
        assignment,
        preferences,
        &analysis.candidates,
        &document.settings,
        limits,
        budget,
    )?;
    let objective_profile = profile::shape(
        &preferences.layout,
        &assignment.domain,
        &analysis.candidates,
        baseline_pairs,
        limits,
        budget,
    )?;
    let (capabilities, variables, constraints, provenance) = validate_shape(
        input,
        &objective_profile,
        &preference,
        rank_upper_bound,
        budget,
    )?;
    let objective_terms = add(
        add(pairs, preference.term_count)?,
        objective_profile.term_count,
    )?;
    let objective_levels = add(
        add(
            count(objective_profile.tiers.len())?,
            count(preferences.levels.len())?,
        )?,
        1,
    )?;
    preflight(&analysis.candidates, plan, budget, limits)?;
    // Consecutive preflight already includes each helper's complete tagged declaration.
    let helper_variable_bytes = compiler_weekend::helper_variable_bytes(plan, budget)?;
    // schema-v2 enum wrapper; the root below contributes all punctuation and empty arrays.
    let boolean_wrappers = mul(pairs, count(r#"{"type":"boolean","value":}"#.len())?)?;
    let separators = [variables, constraints, provenance, objective_terms, pairs]
        .into_iter()
        .try_fold(0, |sum, length| add(sum, length.saturating_sub(1)))?;
    let mut root_levels =
        Vec::with_capacity(usize::try_from(objective_levels).map_err(|_| invalid())?);
    for tier in &objective_profile.tiers {
        root_levels.push(EmptyLevel {
            id: tier.tier.id(),
            direction: OptimizationDirection::Minimize,
            lower_bound: tier.lower_bound,
            upper_bound: tier.upper_bound,
            terms: [],
            provenance: PROVENANCE_ID,
        });
    }
    for (index, level) in preferences.levels.iter().enumerate() {
        root_levels.push(EmptyLevel {
            id: &level.id,
            direction: OptimizationDirection::Minimize,
            lower_bound: *preference
                .level_lower_bounds
                .get(index)
                .ok_or_else(invalid)?,
            upper_bound: *preference
                .level_upper_bounds
                .get(index)
                .ok_or_else(invalid)?,
            terms: [],
            provenance: PROVENANCE_ID,
        });
    }
    root_levels.push(EmptyLevel {
        id: RANK_LEVEL,
        direction: OptimizationDirection::Minimize,
        lower_bound: 0,
        upper_bound: rank_upper_bound,
        terms: [],
        provenance: PROVENANCE_ID,
    });
    let root = EmptyRoot {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables: [],
        constraints: [],
        assumptions: [],
        projections: [],
        provenance: [],
        objectives: EmptyObjectives {
            levels: root_levels,
        },
        metadata: BorrowedMetadata {
            pack_id: "official.workforce",
            scenario_id: document.scenario_id,
            scenario_revision: context.scenario_revision,
            projection_version: WORKFORCE_PROJECTION_VERSION,
            compiler_id: COMPILER_ID,
            compiler_version: env!("CARGO_PKG_VERSION"),
            compile_metadata: MetadataEntries {
                context,
                estimate: &analysis.estimate,
            },
            display_text: BTreeMap::new(),
        },
        declared_capabilities: &capabilities,
        split_authorization: None,
    };
    let root_bytes = budget.measure_ir(&root)?;
    budget.reserve_ir(
        1,
        add(
            8,
            add(count(context.semantic_metadata.len())?, objective_levels)?,
        )?,
        add(
            root_bytes,
            add(add(boolean_wrappers, helper_variable_bytes)?, separators)?,
        )?,
    )?;
    budget.reserve(1, 0, 512)?;
    reserve_fact(
        &level_fact(
            policy,
            preferences.layout.kind,
            ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
        )?,
        budget,
        limits,
    )?;
    for level in &preferences.levels {
        let policy = policy.ok_or_else(invalid)?;
        reserve_fact(
            &preference::level_fact(
                policy,
                level,
                ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
            ),
            budget,
            limits,
        )?;
    }
    preflight_ranks(analysis, ranks, limits, budget)?;
    preflight_preferences(input, preferences, &preference, limits, budget)?;
    preflight_profile(input, &objective_profile, limits, budget)?;
    hours::preflight_complete_ir(plan, budget, limits)?;

    budget.reserve_ir_retention()?;
    budget.check()?;
    Ok(Header {
        rank_upper_bound,
        profile: objective_profile,
        preference,
        capabilities,
    })
}

fn workload_counts(
    plan: &PreferencePlan<'_>,
    preference: &PreferenceShape,
) -> Result<(u64, u64), AssignmentRuleError> {
    let mut variables = 0_u64;
    let mut constraints = 0_u64;
    for period in &preference.workloads.periods {
        let workload = plan.workloads.get(period.workload).ok_or_else(invalid)?;
        let piecewise = match &workload.policy.penalty {
            crate::model::DeviationPenalty::Absolute {} => 0,
            crate::model::DeviationPenalty::Piecewise { breakpoints } => breakpoints.len(),
        };
        for peer in &period.peers {
            variables = variables
                .checked_add(4)
                .and_then(|value| value.checked_add(count(peer.segments.len()).ok()?))
                .and_then(|value| value.checked_add(count(piecewise).ok()?))
                .and_then(|value| {
                    value.checked_add(if peer.split_bounds.is_some() { 2 } else { 0 })
                })
                .and_then(|value| value.checked_add(u64::from(piecewise > 0)))
                .ok_or_else(arithmetic)?;
            let segment_constraints = peer.segments.iter().try_fold(0_u64, |sum, segment| {
                sum.checked_add(
                    count(segment.population.len())?
                        .checked_add(1)
                        .ok_or_else(arithmetic)?,
                )
                .ok_or_else(arithmetic)
            })?;
            let piecewise_constraints = if piecewise > 0 {
                count(piecewise)?.checked_mul(3).ok_or_else(arithmetic)?
            } else {
                0
            };
            constraints = constraints
                .checked_add(3)
                .and_then(|value| value.checked_add(segment_constraints))
                .and_then(|value| {
                    value.checked_add(if peer.split_bounds.is_some() { 2 } else { 0 })
                })
                .and_then(|value| value.checked_add(piecewise_constraints))
                .ok_or_else(arithmetic)?;
        }
    }
    Ok((variables, constraints))
}
fn weekend_target_counts(preference: &PreferenceShape) -> Result<(u64, u64), AssignmentRuleError> {
    let mut variables = 0_u64;
    let mut constraints = 0_u64;
    for shape in &preference.weekend_targets {
        if shape.weekends.is_empty() {
            continue;
        }
        variables = variables
            .checked_add(count(shape.weekends.len())?)
            .and_then(|value| value.checked_add(3))
            .ok_or_else(arithmetic)?;
        let indicator_constraints = shape.weekends.iter().try_fold(0_u64, |sum, weekend| {
            sum.checked_add(
                count(weekend.population.len())?
                    .checked_add(1)
                    .ok_or_else(arithmetic)?,
            )
            .ok_or_else(arithmetic)
        })?;
        constraints = constraints
            .checked_add(indicator_constraints)
            .and_then(|value| value.checked_add(2))
            .ok_or_else(arithmetic)?;
    }
    Ok((variables, constraints))
}

fn complete_shape(
    analysis: &AssignmentAnalysis,
    plan: &Plan,
    pairs: u64,
    preferences: &PreferencePlan<'_>,
    profile: &profile::Shape,
    preference: &PreferenceShape,
    limits: PlanningIrLimitsV1,
) -> Result<(hours::HoursShape, u64, u64, u64), AssignmentRuleError> {
    let hour_shape = hours::checked_shape(plan, limits)?;
    let consecutive_shape = consecutive::shape(plan, limits)?;
    let weekend_shape = compiler_weekend::shape(plan, limits)?;
    let target_terms = count(
        preference
            .targets
            .iter()
            .filter(|target| !target.population.is_empty())
            .count(),
    )?;
    let target_variables = mul(target_terms, 3)?;
    let target_constraints = mul(target_terms, 2)?;
    let (weekend_variables, weekend_constraints) = weekend_target_counts(preference)?;
    let group_terms = preference.groups.iter().try_fold(0_u64, |sum, group| {
        sum.checked_add(count(group.pairs.len())?)
            .ok_or_else(arithmetic)
    })?;
    let group_variables = group_terms;
    let group_constraints = mul(group_terms, 3)?;
    let coverage_variables = count(
        preference
            .coverage
            .iter()
            .filter(|shape| !shape.population.is_empty())
            .count(),
    )?;
    let coverage_constraints = coverage_variables;
    let (workload_variables, workload_constraints) = workload_counts(preferences, preference)?;
    let consecutive_preference_shape = &preference.consecutives;
    let preference_variables = add(
        add(add(target_variables, weekend_variables)?, group_variables)?,
        add(coverage_variables, workload_variables)?,
    )?;
    let variables = add(
        add(
            add(
                add(mul(pairs, 2)?, hour_shape.helper_variables)?,
                consecutive_shape.variables,
            )?,
            weekend_shape.variables,
        )?,
        add(preference_variables, consecutive_preference_shape.variables)?,
    )?;
    let hour_channel_constraints = hour_shape.pair_indices.checked_mul(2).ok_or_else(invalid)?;
    let hour_constraints = hour_shape
        .constraints
        .checked_sub(hour_channel_constraints)
        .ok_or_else(invalid)?;
    let generic_constraints = count(plan.constraints.len())?
        .checked_sub(consecutive_shape.planned_constraints)
        .and_then(|value| value.checked_sub(weekend_shape.planned_constraints))
        .ok_or_else(invalid)?;
    let preference_constraints = add(
        add(
            add(target_constraints, weekend_constraints)?,
            group_constraints,
        )?,
        add(coverage_constraints, workload_constraints)?,
    )?;
    let constraints = add(
        add(
            add(
                add(generic_constraints, consecutive_shape.constraints)?,
                weekend_shape.constraints,
            )?,
            mul(pairs, 2)?,
        )?,
        add(
            hour_constraints,
            add(
                preference_constraints,
                consecutive_preference_shape.constraints,
            )?,
        )?,
    )?;
    let base_provenance = analysis
        .estimate
        .provenance_records
        .checked_sub(hour_shape.provenance)
        .ok_or_else(invalid)?;
    let profile_levels = count(profile.tiers.len())?;
    let preference_levels = count(preference.level_upper_bounds.len())?;
    let hour_provenance = hour_shape
        .provenance
        .checked_sub(hour_shape.pair_indices)
        .ok_or_else(invalid)?;
    let provenance = add(
        add(base_provenance, pairs)?,
        add(
            add(1, hour_provenance)?,
            add(
                add(preference.term_count, preference_levels)?,
                add(profile.term_count, profile_levels)?,
            )?,
        )?,
    )?;
    Ok((hour_shape, variables, constraints, provenance))
}

#[allow(clippy::too_many_lines)]
fn validate_shape(
    input: &ModelInput<'_, '_>,
    profile: &profile::Shape,
    preference: &PreferenceShape,
    rank_upper_bound: i64,
    budget: &mut OperationBudget<'_>,
) -> Result<(BTreeSet<Capability>, u64, u64, u64), AssignmentRuleError> {
    let ModelInput {
        analysis,
        plan,
        preferences,
        limits,
        ..
    } = *input;
    for id in [PROVENANCE_ID, RANK_LEVEL, COMPILER_ID]
        .into_iter()
        .chain(RESERVED_METADATA)
    {
        within(
            count(id.len())?,
            limits.max_id_bytes,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for tier in &profile.tiers {
        within(
            count(tier.tier.id().len())?,
            limits.max_id_bytes,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for level in &preferences.levels {
        within(
            count(level.id.len())?,
            limits.max_id_bytes,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    within(
        count(env!("CARGO_PKG_VERSION").len())?,
        limits.max_metadata_text_bytes,
        AssignmentRuleLimit::PerRecord,
    )?;
    let pairs = count(analysis.candidates.len())?;
    let (hour_shape, variables, constraints, provenance) = complete_shape(
        analysis,
        plan,
        pairs,
        preferences,
        profile,
        preference,
        limits,
    )?;
    let consecutive_shape = consecutive::shape(plan, limits)?;
    let weekend_shape = compiler_weekend::shape(plan, limits)?;
    let objective_levels = add(
        add(
            count(profile.tiers.len())?,
            count(preference.level_upper_bounds.len())?,
        )?,
        1,
    )?;
    let objective_terms = add(add(pairs, preference.term_count)?, profile.term_count)?;
    within(
        variables,
        limits.max_variables,
        AssignmentRuleLimit::Variables,
    )?;
    within(
        constraints,
        limits.max_constraints,
        AssignmentRuleLimit::Constraints,
    )?;
    within(
        provenance,
        limits.max_provenance_records,
        AssignmentRuleLimit::ProvenanceRecords,
    )?;
    within(
        objective_levels,
        limits.max_objective_levels,
        AssignmentRuleLimit::PerRecord,
    )?;
    within(
        objective_terms,
        limits.max_objective_terms,
        AssignmentRuleLimit::PerRecord,
    )?;
    within(
        pairs,
        limits.max_projections,
        AssignmentRuleLimit::PerRecord,
    )?;
    within(
        variables,
        limits.max_component_nodes,
        AssignmentRuleLimit::PerRecord,
    )?;
    within(
        1,
        limits.max_provenance_depth,
        AssignmentRuleLimit::PerRecord,
    )?;
    if pairs > 0 {
        for (value, limit) in [
            (1, limits.max_domain_ranges),
            (1, limits.max_enforcement_literals),
            (2, limits.max_refs_per_node),
            (1, limits.max_parameters_per_record),
            (2, limits.max_provenance_depth),
            (1, limits.max_projection_expression_depth),
        ] {
            within(value, limit, AssignmentRuleLimit::PerRecord)?;
        }
    }
    if preference.term_count > 0 || profile.term_count > 0 {
        for (value, limit) in [
            (1, limits.max_refs_per_node),
            (2, limits.max_provenance_depth),
        ] {
            within(value, limit, AssignmentRuleLimit::PerRecord)?;
        }
    }
    for tier in &profile.tiers {
        for term in &tier.terms {
            if let profile::TermSource::SoftLock { weight, .. } = term.source {
                within(
                    u64::from(weight),
                    u64::try_from(limits.max_abs_coefficient).map_err(|_| invalid())?,
                    AssignmentRuleLimit::PerRecord,
                )?;
            }
        }
    }
    for window in &preference.consecutives.windows {
        let consecutive = preferences
            .consecutives
            .get(window.plan)
            .ok_or_else(invalid)?;
        within(
            u64::try_from(consecutive.coefficient).map_err(|_| invalid())?,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| invalid())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        for day in &window.days {
            within(
                count(day.population.len())?
                    .checked_add(1)
                    .ok_or_else(invalid)?,
                limits.max_refs_per_node,
                AssignmentRuleLimit::PerRecord,
            )?;
        }
        within(
            count(window.days.len())?
                .checked_add(1)
                .ok_or_else(invalid)?,
            limits.max_refs_per_node,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            4,
            limits.max_parameters_per_record,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            1,
            limits.max_entity_refs_per_record,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            2,
            limits.max_provenance_depth,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for target in &preference.targets {
        if !target.population.is_empty() {
            within(
                count(target.population.len())?
                    .checked_add(1)
                    .ok_or_else(invalid)?,
                limits.max_refs_per_node,
                AssignmentRuleLimit::PerRecord,
            )?;
            within(3, limits.max_refs_per_node, AssignmentRuleLimit::PerRecord)?;
        }
        within(
            u64::try_from(target.difference_upper).map_err(|_| invalid())?,
            u64::try_from(limits.max_abs_value).map_err(|_| invalid())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            5,
            limits.max_parameters_per_record,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            2,
            limits.max_entity_refs_per_record,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for weekend_shape in &preference.weekend_targets {
        let target = preferences
            .weekend_targets
            .get(weekend_shape.target)
            .ok_or_else(invalid)?;
        target
            .people
            .get(weekend_shape.person)
            .ok_or_else(invalid)?;
        within(
            u64::try_from(target.coefficient).map_err(|_| invalid())?,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| invalid())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            u64::from(target.target_count),
            u64::try_from(limits.max_abs_value).map_err(|_| invalid())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            u64::try_from(weekend_shape.difference_upper).map_err(|_| invalid())?,
            u64::try_from(limits.max_abs_value).map_err(|_| invalid())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            count(weekend_shape.weekends.len())?
                .checked_add(1)
                .ok_or_else(arithmetic)?,
            limits.max_refs_per_node,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(3, limits.max_refs_per_node, AssignmentRuleLimit::PerRecord)?;
        within(
            3,
            limits.max_parameters_per_record,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            1,
            limits.max_entity_refs_per_record,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            2,
            limits.max_provenance_depth,
            AssignmentRuleLimit::PerRecord,
        )?;
        if !weekend_shape.weekends.is_empty() {
            within(1, limits.max_domain_ranges, AssignmentRuleLimit::PerRecord)?;
            within(
                1,
                u64::try_from(limits.max_abs_coefficient).map_err(|_| invalid())?,
                AssignmentRuleLimit::PerRecord,
            )?;
        }
        for weekend in &weekend_shape.weekends {
            within(2, limits.max_refs_per_node, AssignmentRuleLimit::PerRecord)?;
            within(
                count(weekend.population.len())?
                    .checked_add(1)
                    .ok_or_else(arithmetic)?,
                limits.max_refs_per_node,
                AssignmentRuleLimit::PerRecord,
            )?;
        }
    }
    for group_shape in &preference.groups {
        let group = preferences
            .groups
            .get(group_shape.plan)
            .ok_or_else(invalid)?;
        within(
            u64::try_from(group.coefficient).map_err(|_| invalid())?,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| invalid())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        for _pair in &group_shape.pairs {
            within(3, limits.max_refs_per_node, AssignmentRuleLimit::PerRecord)?;
            within(
                3,
                limits.max_parameters_per_record,
                AssignmentRuleLimit::PerRecord,
            )?;
            within(
                2,
                limits.max_provenance_depth,
                AssignmentRuleLimit::PerRecord,
            )?;
        }
    }
    for coverage_shape in &preference.coverage {
        let _coverage = preferences
            .coverage
            .get(coverage_shape.plan)
            .ok_or_else(invalid)?;
        within(
            u64::try_from(coverage_shape.difference_upper).map_err(|_| invalid())?,
            u64::try_from(limits.max_abs_value).map_err(|_| invalid())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            5,
            limits.max_parameters_per_record,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            2,
            limits.max_provenance_depth,
            AssignmentRuleLimit::PerRecord,
        )?;
        if !coverage_shape.population.is_empty() {
            within(
                count(coverage_shape.population.len())?
                    .checked_add(1)
                    .ok_or_else(arithmetic)?,
                limits.max_refs_per_node,
                AssignmentRuleLimit::PerRecord,
            )?;
        }
    }
    for period in &preference.workloads.periods {
        let workload = preferences
            .workloads
            .get(period.workload)
            .ok_or_else(invalid)?;
        for peer in &period.peers {
            within(
                u64::try_from(peer.difference_upper).map_err(|_| invalid())?,
                u64::try_from(limits.max_abs_value).map_err(|_| invalid())?,
                AssignmentRuleLimit::PerRecord,
            )?;
            within(
                count(peer.segments.len())?
                    .checked_add(2)
                    .ok_or_else(invalid)?,
                limits.max_refs_per_node,
                AssignmentRuleLimit::PerRecord,
            )?;
            if peer.split_bounds.is_some() {
                let components = if peer.segments.is_empty() {
                    peer.terms.len()
                } else {
                    peer.segments.len()
                };
                within(
                    count(components)?.checked_add(2).ok_or_else(arithmetic)?,
                    limits.max_refs_per_node,
                    AssignmentRuleLimit::PerRecord,
                )?;
            }
            let piecewise = match &workload.policy.penalty {
                crate::model::DeviationPenalty::Absolute {} => 0,
                crate::model::DeviationPenalty::Piecewise { breakpoints } => breakpoints.len(),
            };
            if piecewise > 0 {
                within(
                    1,
                    limits.max_enforcement_literals,
                    AssignmentRuleLimit::PerRecord,
                )?;
                within(
                    count(piecewise)?.checked_add(1).ok_or_else(invalid)?,
                    limits.max_refs_per_node,
                    AssignmentRuleLimit::PerRecord,
                )?;
            }
        }
    }
    for upper in profile
        .tiers
        .iter()
        .map(|tier| tier.upper_bound)
        .chain(preference.level_upper_bounds.iter().copied())
        .chain(std::iter::once(rank_upper_bound))
    {
        if upper < 0 {
            return Err(invalid());
        }
        within(
            u64::try_from(upper).map_err(|_| invalid())?,
            u64::try_from(limits.max_abs_value).map_err(|_| invalid())?,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    scalarization_bounds(
        profile
            .tiers
            .iter()
            .map(|tier| (tier.lower_bound, tier.upper_bound))
            .chain(
                preference
                    .level_lower_bounds
                    .iter()
                    .copied()
                    .zip(preference.level_upper_bounds.iter().copied()),
            )
            .chain(std::iter::once((0, rank_upper_bound))),
    )?;
    let target_references = preference.targets.iter().try_fold(0_u64, |sum, target| {
        sum.checked_add(
            count(target.population.len())?
                .checked_add(if target.population.is_empty() { 2 } else { 4 })
                .ok_or_else(invalid)?,
        )
        .ok_or_else(arithmetic)
    })?;
    let weekend_target_references =
        preference
            .weekend_targets
            .iter()
            .try_fold(0_u64, |sum, shape| {
                let value = if shape.weekends.is_empty() {
                    2
                } else {
                    let indicator = shape.weekends.iter().try_fold(0_u64, |sum, weekend| {
                        sum.checked_add(
                            count(weekend.population.len())?
                                .checked_mul(4)
                                .and_then(|value| value.checked_add(2))
                                .ok_or_else(arithmetic)?,
                        )
                        .ok_or_else(arithmetic)
                    })?;
                    indicator
                        .checked_add(count(shape.weekends.len())?)
                        .and_then(|value| value.checked_add(6))
                        .ok_or_else(arithmetic)?
                };
                sum.checked_add(value).ok_or_else(arithmetic)
            })?;
    let group_references = preference.groups.iter().try_fold(0_u64, |sum, group| {
        sum.checked_add(
            count(group.pairs.len())?
                .checked_mul(6)
                .ok_or_else(arithmetic)?,
        )
        .ok_or_else(arithmetic)
    })?;
    let coverage_references = preference.coverage.iter().try_fold(0_u64, |sum, shape| {
        sum.checked_add(
            count(shape.population.len())?
                .checked_add(3)
                .ok_or_else(invalid)?,
        )
        .ok_or_else(arithmetic)
    })?;
    let workload_references =
        preference
            .workloads
            .periods
            .iter()
            .try_fold(0_u64, |sum, period| {
                period.peers.iter().try_fold(sum, |sum, peer| {
                    let segments = peer.segments.iter().try_fold(0_u64, |sum, segment| {
                        sum.checked_add(count(segment.population.len())?)
                            .ok_or_else(arithmetic)
                    })?;
                    let split_refs = if peer.split_bounds.is_some() {
                        let components = if peer.segments.is_empty() {
                            peer.terms.len()
                        } else {
                            peer.segments.len()
                        };
                        count(components)?
                            .checked_add(2)
                            .and_then(|value| value.checked_mul(2))
                            .ok_or_else(arithmetic)?
                    } else {
                        0
                    };
                    sum.checked_add(4_u64.checked_add(segments).ok_or_else(arithmetic)?)
                        .and_then(|value| value.checked_add(split_refs))
                        .ok_or_else(arithmetic)
                })
            })?;
    let profile_references = profile.term_count.checked_mul(2).ok_or_else(arithmetic)?;
    let references = add(
        add(analysis.estimate.references, pairs)?,
        add(
            add(preference.term_count, profile_references)?,
            pairs
                .checked_add(target_references)
                .and_then(|value| value.checked_add(weekend_target_references))
                .and_then(|value| value.checked_add(group_references))
                .and_then(|value| value.checked_add(coverage_references))
                .and_then(|value| value.checked_add(workload_references))
                .and_then(|value| value.checked_add(preference.consecutives.references))
                .ok_or_else(invalid)?,
        )?,
    )?;
    within(
        references,
        limits.max_total_refs,
        AssignmentRuleLimit::References,
    )?;
    let mut edges = add(
        add(
            add(
                add(
                    add(variables, pairs.saturating_sub(1))?,
                    hour_shape.component_edges,
                )?,
                consecutive_shape.component_edges,
            )?,
            weekend_shape.component_edges,
        )?,
        preference.consecutives.component_edges,
    )?;
    edges = add(edges, preference.term_count)?;
    edges = add(edges, profile.term_count)?;
    let target_edges = preference.targets.iter().try_fold(0_u64, |sum, target| {
        sum.checked_add(if target.population.is_empty() {
            0
        } else {
            count(target.population.len())?
                .checked_add(2)
                .ok_or_else(invalid)?
        })
        .ok_or_else(arithmetic)
    })?;
    edges = add(edges, target_edges)?;
    let weekend_target_edges =
        preference
            .weekend_targets
            .iter()
            .try_fold(0_u64, |sum, shape| {
                if shape.weekends.is_empty() {
                    return Ok(sum);
                }
                let indicators = shape.weekends.iter().try_fold(0_u64, |sum, weekend| {
                    sum.checked_add(
                        count(weekend.population.len())?
                            .checked_mul(2)
                            .ok_or_else(arithmetic)?,
                    )
                    .ok_or_else(arithmetic)
                })?;
                let weekend_count = count(shape.weekends.len())?;
                sum.checked_add(indicators)
                    .and_then(|value| value.checked_add(weekend_count))
                    .and_then(|value| value.checked_add(2))
                    .ok_or_else(arithmetic)
            })?;
    edges = add(edges, weekend_target_edges)?;
    let group_edges = preference.groups.iter().try_fold(0_u64, |sum, group| {
        sum.checked_add(
            count(group.pairs.len())?
                .checked_mul(3)
                .ok_or_else(arithmetic)?,
        )
        .ok_or_else(arithmetic)
    })?;
    let coverage_edges = preference.coverage.iter().try_fold(0_u64, |sum, shape| {
        sum.checked_add(
            count(shape.population.len())?
                .checked_add(1)
                .ok_or_else(invalid)?,
        )
        .ok_or_else(arithmetic)
    })?;
    edges = add(edges, group_edges)?;
    edges = add(edges, coverage_edges)?;
    budget.reserve(0, 6, 512)?;
    let mut capabilities = BTreeSet::new();
    if !preference.groups.is_empty()
        || preference
            .coverage
            .iter()
            .any(|shape| !shape.population.is_empty())
    {
        capabilities.insert(Capability::LinearComparison);
    }
    if pairs > 0 || preference.term_count > 0 || profile.term_count > 0 {
        capabilities.extend([
            Capability::LinearComparison,
            Capability::ObjectivePenalty,
            Capability::BooleanProjection,
        ]);
    }
    if !preference.consecutives.windows.is_empty() {
        capabilities.insert(Capability::BoolOr);
    }
    if preference
        .targets
        .iter()
        .any(|target| !target.population.is_empty())
    {
        capabilities.insert(Capability::AbsDifference);
    }
    if preference
        .weekend_targets
        .iter()
        .any(|target| !target.weekends.is_empty())
    {
        capabilities.insert(Capability::AbsDifference);
        capabilities.insert(Capability::LinearComparison);
    }
    if preference
        .workloads
        .periods
        .iter()
        .any(|period| !period.peers.is_empty())
    {
        capabilities.extend([
            Capability::AbsDifference,
            Capability::LinearComparison,
            Capability::ObjectivePenalty,
        ]);
        if preference.workloads.periods.iter().any(|period| {
            preferences
                .workloads
                .get(period.workload)
                .is_some_and(|workload| {
                    matches!(
                        &workload.policy.penalty,
                        crate::model::DeviationPenalty::Piecewise { .. }
                    )
                })
        }) {
            capabilities.insert(Capability::ExactlyOne);
        }
    }
    let workload_edges = preference
        .workloads
        .periods
        .iter()
        .try_fold(0_u64, |sum, period| {
            period.peers.iter().try_fold(sum, |sum, peer| {
                let segments = peer.segments.iter().try_fold(0_u64, |sum, segment| {
                    sum.checked_add(count(segment.population.len())?.saturating_add(1))
                        .ok_or_else(arithmetic)
                })?;
                let split = if peer.split_bounds.is_some() {
                    let components = if peer.segments.is_empty() {
                        peer.terms.len()
                    } else {
                        peer.segments.len()
                    };
                    count(components)?
                        .checked_add(1)
                        .and_then(|value| value.checked_mul(2))
                        .ok_or_else(arithmetic)?
                } else {
                    0
                };
                sum.checked_add(segments.saturating_add(2))
                    .and_then(|value| value.checked_add(split))
                    .ok_or_else(arithmetic)
            })
        })?;
    edges = add(edges, workload_edges)?;
    for planned in &plan.constraints {
        budget.step()?;
        if consecutive::is(planned) {
            consecutive::capabilities(planned, plan, &mut capabilities)?;
            continue;
        }
        if compiler_weekend::is(planned) {
            capabilities.insert(Capability::CardinalityRange);
            continue;
        }
        if !planned.impossible {
            edges = add(edges, count(planned.population.len())?.saturating_sub(1))?;
        }
        capabilities.insert(constraint_capability(planned));
    }
    within(
        edges,
        limits.max_component_edges,
        AssignmentRuleLimit::PerRecord,
    )?;
    Ok((capabilities, variables, constraints, provenance))
}
fn scalarization_bounds(
    bounds: impl DoubleEndedIterator<Item = (i64, i64)>,
) -> Result<(), AssignmentRuleError> {
    let mut radix = 1_i128;
    let mut native = 0_i128;
    let native_limit = i128::from(i64::MAX / 2);
    for (lower, upper) in bounds.rev() {
        if lower < 0 || upper < lower {
            return Err(invalid());
        }
        let span = upper.checked_sub(lower).ok_or_else(arithmetic)?;
        native = native
            .checked_add(
                radix
                    .checked_mul(i128::from(upper))
                    .ok_or_else(arithmetic)?,
            )
            .ok_or_else(arithmetic)?;
        radix = radix
            .checked_mul(i128::from(span) + 1)
            .ok_or_else(arithmetic)?;
        // Lexicographic weights use each level's variable span; fixed target shortfalls do
        // not multiply earlier weights. The native bound still includes their full offsets.
        // Prefer terms may have positive constants and negative decision coefficients, so
        // conservatively charge each level's full upper bound before native translation.
        if radix > i128::from(i64::MAX) || native > native_limit {
            return Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord,
            ));
        }
    }
    Ok(())
}
fn workload_constraint_refs(
    constraint: &eutheto_planning_ir::ConstraintRecord,
) -> Result<u64, AssignmentRuleError> {
    let body = match &constraint.body {
        eutheto_planning_ir::Constraint::LinearComparison(comparison) => {
            count(comparison.expression.terms.len())?
        }
        eutheto_planning_ir::Constraint::AbsDifference { .. } => 3,
        eutheto_planning_ir::Constraint::ExactlyOne { literals } => count(literals.len())?,
        _ => return Err(invalid()),
    };
    body.checked_add(count(constraint.enforcement.len())?)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(arithmetic)
}

#[allow(clippy::too_many_lines)]
fn preflight_preferences(
    input: &ModelInput<'_, '_>,
    preferences: &PreferencePlan<'_>,
    shape: &PreferenceShape,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    if !preferences.active() {
        return Ok(());
    }
    let ModelInput {
        assignment,
        analysis,
        document,
        ..
    } = *input;
    let integer = IntVariableId::new(
        "official.workforce.int.0000000000000000000000000000000000000000000000000000000000000000",
    )
    .map_err(|_| invalid())?;
    let decision_provenance = ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?;
    let mut identities = PlanningIdentities::default();
    let mut measured = 0_u64;
    for entry in &preferences.entries {
        for pair in &analysis.candidates {
            budget.step()?;
            if !preference::matches(assignment, *entry, *pair, &document.settings, budget)? {
                continue;
            }
            measured = measured.checked_add(1).ok_or_else(arithmetic)?;
            let ids = PreferenceIds::derive(*entry, *pair, &mut identities, budget)?;
            let emitted = emit_preference(
                *entry,
                integer.clone(),
                decision_provenance.clone(),
                ids,
                budget,
            )?;
            within(
                count(emitted.term.id.as_str().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, 2, budget.measure_ir(&emitted.term)?)?;
            reserve_fact(&emitted.fact, budget, limits)?;
        }
    }
    for pruned in &shape.pruned_base {
        budget.step()?;
        let entry = *preferences.entries.get(pruned.entry).ok_or_else(invalid)?;
        let ids = PreferenceIds::derive(entry, pruned.pair, &mut identities, budget)?;
        let emitted =
            preference::emit_pruned_base(entry, decision_provenance.clone(), ids, budget)?;
        within(
            count(emitted.term.id.as_str().len())?,
            limits.max_id_bytes,
            AssignmentRuleLimit::PerRecord,
        )?;
        budget.reserve_ir(1, 1, budget.measure_ir(&emitted.term)?)?;
        reserve_fact(&emitted.fact, budget, limits)?;
        measured = measured.checked_add(1).ok_or_else(arithmetic)?;
    }
    for target_shape in &shape.targets {
        budget.step()?;
        let target = preferences
            .targets
            .get(target_shape.target)
            .ok_or_else(invalid)?;
        let period = target
            .periods
            .get(target_shape.period)
            .ok_or_else(invalid)?;
        if target_shape.population.is_empty() {
            let ids = TargetIds::derive_constant(target, period, &mut identities, budget)?;
            let emitted = emit_constant_target(
                target,
                period,
                ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
                ids,
                budget,
            )?;
            within(
                count(emitted.term.id.as_str().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, 1, budget.measure_ir(&emitted.term)?)?;
            reserve_fact(&emitted.fact, budget, limits)?;
            measured = measured.checked_add(1).ok_or_else(arithmetic)?;
            continue;
        }
        let population = (0..target_shape.population.len())
            .map(|index| {
                budget.step()?;
                let id = format!("official.workforce.int.{index:064x}");
                budget.reserve(0, 1, count(id.len())?)?;
                IntVariableId::new(id).map_err(|_| invalid())
            })
            .collect::<Result<Vec<_>, AssignmentRuleError>>()?;
        let ids = TargetIds::derive(target, period, &mut identities, budget)?;
        let emitted = emit_target(
            target,
            period,
            &population,
            ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
            ids,
            budget,
        )?;
        for variable in &emitted.variables {
            within(
                count(variable.canonical_id().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, 1, budget.measure_ir(variable)?)?;
        }
        let count_refs = count(population.len())?
            .checked_add(2)
            .ok_or_else(invalid)?;
        for (index, constraint) in emitted.constraints.iter().enumerate() {
            within(
                count(constraint.id.as_str().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            let refs = if index == 0 { count_refs } else { 4 };
            budget.reserve_ir(1, refs, budget.measure_ir(constraint)?)?;
        }
        within(
            count(emitted.term.id.as_str().len())?,
            limits.max_id_bytes,
            AssignmentRuleLimit::PerRecord,
        )?;
        budget.reserve_ir(1, 2, budget.measure_ir(&emitted.term)?)?;
        reserve_fact(&emitted.fact, budget, limits)?;
        measured = measured.checked_add(1).ok_or_else(arithmetic)?;
    }
    for weekend_shape in &shape.weekend_targets {
        budget.step()?;
        let target = preferences
            .weekend_targets
            .get(weekend_shape.target)
            .ok_or_else(invalid)?;
        let person = target
            .people
            .get(weekend_shape.person)
            .ok_or_else(invalid)?;
        if weekend_shape.weekends.is_empty() {
            let ids = WeekendTargetIds::derive_constant(target, person, &mut identities, budget)?;
            let emitted = emit_constant_weekend_target(
                target,
                person,
                ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
                ids,
                budget,
            )?;
            within(
                count(emitted.term.id.as_str().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, 1, budget.measure_ir(&emitted.term)?)?;
            reserve_fact(&emitted.fact, budget, limits)?;
            measured = measured.checked_add(1).ok_or_else(arithmetic)?;
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
                        let id = format!("official.workforce.int.{index:064x}");
                        budget.reserve(0, 1, count(id.len())?)?;
                        IntVariableId::new(id).map_err(|_| invalid())
                    })
                    .collect::<Result<Vec<_>, AssignmentRuleError>>()
            })
            .collect::<Result<Vec<_>, AssignmentRuleError>>()?;
        let ids = WeekendTargetIds::derive(target, person, &mut identities, budget)?;
        let emitted = emit_weekend_target(
            target,
            person,
            weekend_shape,
            &populations,
            ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
            ids,
            &mut identities,
            budget,
        )?;
        for variable in &emitted.variables {
            within(
                count(variable.canonical_id().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, 1, budget.measure_ir(variable)?)?;
        }
        for constraint in &emitted.constraints {
            within(
                count(constraint.id.as_str().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            let refs = workload_constraint_refs(constraint)?;
            within(
                refs,
                limits.max_refs_per_node,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, refs, budget.measure_ir(constraint)?)?;
        }
        within(
            count(emitted.term.id.as_str().len())?,
            limits.max_id_bytes,
            AssignmentRuleLimit::PerRecord,
        )?;
        budget.reserve_ir(1, 2, budget.measure_ir(&emitted.term)?)?;
        reserve_fact(&emitted.fact, budget, limits)?;
        measured = measured.checked_add(1).ok_or_else(arithmetic)?;
    }
    for group_shape in &shape.groups {
        budget.step()?;
        let group = preferences
            .groups
            .get(group_shape.plan)
            .ok_or_else(invalid)?;
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
            let first_integer =
                IntVariableId::new(format!("official.workforce.int.{:064x}", pair_shape.first))
                    .map_err(|_| invalid())?;
            let second_integer =
                IntVariableId::new(format!("official.workforce.int.{:064x}", pair_shape.second))
                    .map_err(|_| invalid())?;
            let ids = GroupIds::derive(group, first, second, &mut identities, budget)?;
            let emitted = emit_group(
                group,
                first,
                second,
                &first_integer,
                &second_integer,
                ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
                ids,
                budget,
            )?;
            within(
                count(emitted.variable.canonical_id().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, 2, budget.measure_ir(&emitted.variable)?)?;
            for constraint in &emitted.constraints {
                within(
                    count(constraint.id.as_str().len())?,
                    limits.max_id_bytes,
                    AssignmentRuleLimit::PerRecord,
                )?;
                budget.reserve_ir(1, 3, budget.measure_ir(constraint)?)?;
            }
            within(
                count(emitted.term.id.as_str().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, 3, budget.measure_ir(&emitted.term)?)?;
            reserve_fact(&emitted.fact, budget, limits)?;
            measured = measured.checked_add(1).ok_or_else(arithmetic)?;
        }
    }
    for coverage_shape in &shape.coverage {
        budget.step()?;
        let coverage_plan = preferences
            .coverage
            .get(coverage_shape.plan)
            .ok_or_else(invalid)?;
        let population = coverage_shape
            .population
            .iter()
            .map(|index| {
                IntVariableId::new(format!("official.workforce.int.{index:064x}"))
                    .map_err(|_| invalid())
            })
            .collect::<Result<Vec<_>, AssignmentRuleError>>()?;
        let ids = CoverageIds::derive(coverage_plan, coverage_shape, &mut identities, budget)?;
        let emitted = emit_coverage(
            coverage_plan,
            coverage_shape,
            &population,
            ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
            ids,
            budget,
        )?;
        for variable in &emitted.variables {
            within(
                count(variable.canonical_id().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, 2, budget.measure_ir(variable)?)?;
        }
        for constraint in &emitted.constraints {
            within(
                count(constraint.id.as_str().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            let refs = count(population.len())?
                .checked_add(2)
                .ok_or_else(invalid)?;
            budget.reserve_ir(1, refs, budget.measure_ir(constraint)?)?;
        }
        within(
            count(emitted.term.id.as_str().len())?,
            limits.max_id_bytes,
            AssignmentRuleLimit::PerRecord,
        )?;
        budget.reserve_ir(1, 2, budget.measure_ir(&emitted.term)?)?;
        reserve_fact(&emitted.fact, budget, limits)?;
        measured = measured.checked_add(1).ok_or_else(arithmetic)?;
    }
    for period_shape in &shape.workloads.periods {
        budget.step()?;
        let workload = preferences
            .workloads
            .get(period_shape.workload)
            .ok_or_else(invalid)?;
        let period = workload
            .periods
            .get(period_shape.period)
            .ok_or_else(invalid)?;
        let candidate_integers = (0..analysis.candidates.len())
            .map(|index| {
                IntVariableId::new(format!("official.workforce.int.{index:064x}"))
                    .map_err(|_| invalid())
            })
            .collect::<Result<Vec<_>, AssignmentRuleError>>()?;
        let mut ids = Vec::with_capacity(period_shape.peers.len());
        for peer in &period_shape.peers {
            ids.push(preference::WorkloadIds::derive(
                workload,
                period,
                peer,
                &mut identities,
                budget,
            )?);
        }
        let actuals = ids
            .iter()
            .map(|value| value.actual.clone())
            .collect::<Vec<_>>();
        for (peer, ids) in period_shape.peers.iter().zip(ids) {
            let emitted = preference::emit_workload(
                workload,
                period,
                peer,
                &actuals,
                &candidate_integers,
                ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
                ids,
                budget,
            )?;
            for variable in &emitted.variables {
                within(
                    count(variable.canonical_id().len())?,
                    limits.max_id_bytes,
                    AssignmentRuleLimit::PerRecord,
                )?;
                budget.reserve_ir(1, 2, budget.measure_ir(variable)?)?;
            }
            for constraint in &emitted.constraints {
                within(
                    count(constraint.id.as_str().len())?,
                    limits.max_id_bytes,
                    AssignmentRuleLimit::PerRecord,
                )?;
                let refs = workload_constraint_refs(constraint)?;
                within(
                    refs,
                    limits.max_refs_per_node,
                    AssignmentRuleLimit::PerRecord,
                )?;
                budget.reserve_ir(1, refs, budget.measure_ir(constraint)?)?;
            }
            within(
                count(emitted.term.id.as_str().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, 2, budget.measure_ir(&emitted.term)?)?;
            reserve_fact(&emitted.fact, budget, limits)?;
            measured = measured.checked_add(1).ok_or_else(arithmetic)?;
        }
    }
    let candidate_booleans = (0..analysis.candidates.len())
        .map(|index| {
            BoolVariableId::new(format!("official.workforce.bool.{index:064x}"))
                .map_err(|_| invalid())
        })
        .collect::<Result<Vec<_>, AssignmentRuleError>>()?;
    for window_shape in &shape.consecutives.windows {
        budget.step()?;
        let consecutive = preferences
            .consecutives
            .get(window_shape.plan)
            .ok_or_else(invalid)?;
        let ids = preference_consecutive::ids(consecutive, window_shape, &mut identities, budget)?;
        let emitted = preference_consecutive::emit(
            consecutive,
            window_shape,
            ids,
            &candidate_booleans,
            ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
            budget,
        )?;
        for variable in &emitted.variables {
            within(
                count(variable.canonical_id().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, 2, budget.measure_ir(variable)?)?;
        }
        for constraint in &emitted.constraints {
            within(
                count(constraint.id.as_str().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            let refs = match &constraint.body {
                eutheto_planning_ir::Constraint::BoolOr { literals } => count(literals.len())?,
                eutheto_planning_ir::Constraint::LinearComparison(comparison) => {
                    count(comparison.expression.terms.len())?
                }
                _ => return Err(invalid()),
            }
            .checked_add(count(constraint.enforcement.len())?)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(arithmetic)?;
            within(
                refs,
                limits.max_refs_per_node,
                AssignmentRuleLimit::PerRecord,
            )?;
            budget.reserve_ir(1, refs, budget.measure_ir(constraint)?)?;
        }
        within(
            count(emitted.term.id.as_str().len())?,
            limits.max_id_bytes,
            AssignmentRuleLimit::PerRecord,
        )?;
        budget.reserve_ir(1, 2, budget.measure_ir(&emitted.term)?)?;
        reserve_fact(&emitted.fact, budget, limits)?;
        measured = measured.checked_add(1).ok_or_else(arithmetic)?;
    }
    if measured != shape.term_count {
        return Err(invalid());
    }
    budget.check()
}

fn preflight_profile(
    input: &ModelInput<'_, '_>,
    shape: &profile::Shape,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    if shape.tiers.is_empty() {
        return if shape.term_count == 0 {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    let ModelInput { preferences, .. } = *input;
    let kind = preferences
        .layout
        .kind
        .filter(|kind| kind.named())
        .ok_or_else(invalid)?;
    let mut identities = PlanningIdentities::default();
    let mut parents = Vec::with_capacity(shape.tiers.len());
    for tier in &shape.tiers {
        let fact = profile::prefix_level_fact(kind, tier.tier, &mut identities, budget)?;
        parents.push((tier.tier, fact.id.clone()));
        reserve_fact(&fact, budget, limits)?;
    }
    let mut measured = 0_u64;
    for tier in &shape.tiers {
        let parent = parents
            .iter()
            .find(|(source, _)| *source == tier.tier)
            .map(|(_, id)| id.clone())
            .ok_or_else(invalid)?;
        for term_shape in &tier.terms {
            budget.step()?;
            let integer = term_shape
                .candidate_index
                .map(|index| {
                    IntVariableId::new(format!("official.workforce.int.{index:064x}"))
                        .map_err(|_| invalid())
                })
                .transpose()?;
            let (term, fact) = profile::emit_term(
                *term_shape,
                integer,
                parent.clone(),
                &mut identities,
                budget,
            )?;
            within(
                count(term.id.as_str().len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
            let references = count(term.expression.terms.len())?
                .checked_add(1)
                .ok_or_else(arithmetic)?;
            budget.reserve_ir(1, references, budget.measure_ir(&term)?)?;
            reserve_fact(&fact, budget, limits)?;
            measured = measured.checked_add(1).ok_or_else(arithmetic)?;
        }
    }
    if measured != shape.term_count {
        return Err(invalid());
    }
    budget.check()
}

fn constraint_capability(planned: &PlannedConstraint) -> Capability {
    if planned.impossible {
        return match planned.predicate {
            Predicate::FixedAssignment {
                assigned: false, ..
            } => Capability::BoolAnd,
            _ => Capability::BoolOr,
        };
    }
    match planned.predicate {
        Predicate::Headcount { .. }
        | Predicate::Qualification { .. }
        | Predicate::SkillMixQualification { .. }
        | Predicate::MaximumAssignmentCount { .. }
        | Predicate::WorkloadQuota {
            measure: crate::model::WorkloadQuotaMeasure::AssignmentCount {},
            ..
        }
        | Predicate::MaximumWorkedWeekends { .. } => Capability::CardinalityRange,
        Predicate::WorkloadQuota { .. } | Predicate::FixedAssignment { assigned: true, .. } => {
            Capability::BoolOr
        }
        Predicate::FixedAssignment {
            assigned: false, ..
        } => Capability::BoolAnd,
        Predicate::HardLock { .. } => Capability::BoolOr,
        Predicate::MaximumConsecutive { .. } => unreachable!(),
        Predicate::Overlap { .. }
        | Predicate::OverlapClique { .. }
        | Predicate::MutualSameShift { .. }
        | Predicate::MutualOverlap { .. }
        | Predicate::MinimumRest { .. }
        | Predicate::TransitionTime { .. } => Capability::AtMostOne,
    }
}

fn preflight_ranks(
    analysis: &AssignmentAnalysis,
    ranks: &RankIndex<'_>,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    if let Some(&pair) = analysis.candidates.first() {
        // One fixed-size measurement template, never a per-candidate temporary graph.
        // UUIDs and planning digests have fixed ASCII spelling; only the numeric rank's
        // serialized length varies. Reuse the template and update those two integers.
        budget.reserve(6, 24, 4096)?;
        let decision = BoolVariable {
            id: BoolVariableId::new(BOOL_ID).map_err(|_| invalid())?,
            provenance: ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
        };
        let ids = RankIds {
            integer: IntVariableId::new("official.workforce.int.0000000000000000000000000000000000000000000000000000000000000000").map_err(|_| invalid())?,
            channels: [PlanningConstraintId::new(CONSTRAINT_ID).map_err(|_| invalid())?, PlanningConstraintId::new(CONSTRAINT_ID).map_err(|_| invalid())?],
            term: ObjectiveTermId::new("official.workforce.objective.0000000000000000000000000000000000000000000000000000000000000000").map_err(|_| invalid())?,
            projection: ProjectionId::new("official.workforce.projection.0000000000000000000000000000000000000000000000000000000000000000").map_err(|_| invalid())?,
            fact: ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
        };
        let mut template = emit_rank(pair, 1, &decision, ids, budget)?;
        for id in [
            template.term.id.as_str(),
            template.projection.id.as_str(),
            template.projection.assignment_id.as_str(),
            template.projection.entity.id.as_str(),
        ] {
            within(
                count(id.len())?,
                limits.max_id_bytes,
                AssignmentRuleLimit::PerRecord,
            )?;
        }
        for &pair in &analysis.candidates {
            budget.steps(16)?;
            let rank = ranks.rank(pair, limits, budget)?;
            template.term.expression.terms[0].coefficient = rank;
            *template
                .fact
                .parameters
                .get_mut("rank")
                .ok_or_else(invalid)? = ProvenanceParameter::Integer(rank);
            budget.reserve_ir(1, 1, budget.measure_ir(&template.variable)?)?;
            for channel in &template.channels {
                budget.reserve_ir(1, 3, budget.measure_ir(channel)?)?;
            }
            budget.reserve_ir(1, 2, budget.measure_ir(&template.term)?)?;
            budget.reserve_ir(1, 3, budget.measure_ir(&template.projection)?)?;
            reserve_fact(&template.fact, budget, limits)?;
        }
    }
    budget.check()
}

fn mul(left: u64, right: u64) -> Result<u64, AssignmentRuleError> {
    left.checked_mul(right).ok_or_else(invalid)
}

fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(super::AssignmentConstructionIssue::ArithmeticOverflow)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmptyRoot<'a> {
    schema_version: u32,
    variables: [(); 0],
    constraints: [(); 0],
    objectives: EmptyObjectives<'a>,
    assumptions: [(); 0],
    projections: [(); 0],
    provenance: [(); 0],
    metadata: BorrowedMetadata<'a>,
    declared_capabilities: &'a BTreeSet<Capability>,
    split_authorization: Option<()>,
}
#[derive(Serialize)]
struct EmptyObjectives<'a> {
    levels: Vec<EmptyLevel<'a>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmptyLevel<'a> {
    id: &'a str,
    direction: OptimizationDirection,
    lower_bound: i64,
    upper_bound: i64,
    terms: [(); 0],
    provenance: &'a str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BorrowedMetadata<'a> {
    pack_id: &'a str,
    scenario_id: eutheto_types::ScenarioId,
    scenario_revision: u64,
    projection_version: u32,
    compiler_id: &'a str,
    compiler_version: &'a str,
    compile_metadata: MetadataEntries<'a>,
    display_text: BTreeMap<&'a str, &'a str>,
}
struct MetadataEntries<'a> {
    context: &'a CompileContext,
    estimate: &'a super::AssignmentModelEstimate,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
enum BorrowedParameter<'a> {
    Text(&'a str),
}
impl Serialize for MetadataEntries<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let length = self
            .context
            .semantic_metadata
            .len()
            .checked_add(RESERVED_METADATA.len())
            .ok_or_else(|| serde::ser::Error::custom("metadata count overflow"))?;
        let mut map = serializer.serialize_map(Some(length))?;
        for (key, value) in &self.context.semantic_metadata {
            map.serialize_entry(key, &BorrowedParameter::Text(value))?;
        }
        for (key, value) in RESERVED_METADATA
            .into_iter()
            .zip(metadata_values(self.estimate))
        {
            let integer = i64::try_from(value)
                .map_err(|_| serde::ser::Error::custom("metadata count overflow"))?;
            map.serialize_entry(key, &ProvenanceParameter::Integer(integer))?;
        }
        map.end()
    }
}
