use super::super::{
    AssignmentRuleLimit,
    analysis::Predicate,
    budget::{add, count, within},
    compiler::{BOOL_ID, CONSTRAINT_ID, PROVENANCE_ID, preflight, reserve_fact},
    compiler_rank::emit_rank,
};
use super::{
    AssignmentAnalysis, AssignmentRuleError, COMPILER_ID, CompileContext, ModelInput,
    OperationBudget, PlanningIrLimitsV1, RANK_LEVEL, RESERVED_METADATA, RankIds, RankIndex,
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
    pub upper_bound: i64,
    pub capabilities: BTreeSet<Capability>,
}

pub(super) fn prepare(
    input: &ModelInput<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<Header, AssignmentRuleError> {
    let ModelInput {
        document,
        context,
        policy,
        analysis,
        plan,
        ranks,
        limits,
    } = *input;
    let capabilities = validate_shape(input, budget)?;
    let pairs = count(analysis.candidates.len())?;
    let variables = mul(pairs, 2)?;
    let constraints = add(count(plan.constraints.len())?, variables)?;
    let provenance = add(add(analysis.estimate.provenance_records, pairs)?, 1)?;
    // This set predicts serialized size only. Shared canonicalization derives the actual
    // declaration from the finished model; the compiler never declares backend support.
    let mut upper_bound = 0_i64;
    for &pair in &analysis.candidates {
        let rank = ranks.rank(pair, limits, budget)?;
        upper_bound = upper_bound.checked_add(rank).ok_or_else(invalid)?;
        if upper_bound > limits.max_abs_value {
            return Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord,
            ));
        }
    }
    preflight(&analysis.candidates, plan, budget, limits)?;
    // Contribution preflight measures bare Boolean records. Complete variables have a
    // schema-v2 enum wrapper; the root below contributes all punctuation and empty arrays.
    let boolean_wrappers = mul(pairs, count(r#"{"type":"boolean","value":}"#.len())?)?;
    let separators = [variables, constraints, provenance, pairs, pairs]
        .into_iter()
        .try_fold(0, |sum, length| add(sum, length.saturating_sub(1)))?;
    let root = EmptyRoot {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables: [],
        constraints: [],
        assumptions: [],
        projections: [],
        provenance: [],
        objectives: EmptyObjectives {
            levels: [EmptyLevel {
                id: RANK_LEVEL,
                direction: OptimizationDirection::Minimize,
                lower_bound: 0,
                upper_bound,
                terms: [],
                provenance: PROVENANCE_ID,
            }],
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
        add(8, count(context.semantic_metadata.len())?)?,
        add(root_bytes, add(boolean_wrappers, separators)?)?,
    )?;
    budget.reserve(1, 0, 512)?;
    reserve_fact(
        &level_fact(
            policy,
            ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
        ),
        budget,
        limits,
    )?;
    preflight_ranks(analysis, ranks, limits, budget)?;
    budget.check()?;
    Ok(Header {
        upper_bound,
        capabilities,
    })
}

fn validate_shape(
    input: &ModelInput<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<BTreeSet<Capability>, AssignmentRuleError> {
    let ModelInput {
        analysis,
        plan,
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
    within(
        count(env!("CARGO_PKG_VERSION").len())?,
        limits.max_metadata_text_bytes,
        AssignmentRuleLimit::PerRecord,
    )?;
    let pairs = count(analysis.candidates.len())?;
    let variables = mul(pairs, 2)?;
    let constraints = add(count(plan.constraints.len())?, variables)?;
    let provenance = add(add(analysis.estimate.provenance_records, pairs)?, 1)?;
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
    for (value, limit) in [
        (1, limits.max_objective_levels),
        (pairs, limits.max_objective_terms),
        (pairs, limits.max_projections),
        (variables, limits.max_component_nodes),
        (1, limits.max_provenance_depth),
    ] {
        within(value, limit, AssignmentRuleLimit::PerRecord)?;
    }
    if pairs > 0 {
        for (value, limit) in [
            (1, limits.max_domain_ranges),
            (1, limits.max_enforcement_literals),
            (2, limits.max_refs_per_node),
            (1, limits.max_parameters_per_record),
            (2, limits.max_provenance_depth),
            (1, limits.max_projection_expression_depth),
            (
                1,
                u64::try_from(limits.max_abs_value).map_err(|_| invalid())?,
            ),
        ] {
            within(value, limit, AssignmentRuleLimit::PerRecord)?;
        }
    }
    let mut edges = add(variables, pairs.saturating_sub(1))?;
    budget.reserve(0, 6, 512)?;
    let mut capabilities = BTreeSet::new();
    if pairs > 0 {
        capabilities.extend([
            Capability::LinearComparison,
            Capability::ObjectivePenalty,
            Capability::BooleanProjection,
        ]);
    }
    for planned in &plan.constraints {
        budget.step()?;
        let capability = if planned.impossible {
            Capability::BoolOr
        } else {
            edges = add(edges, count(planned.population.len())?.saturating_sub(1))?;
            match planned.predicate {
                Predicate::Headcount { .. } | Predicate::Qualification { .. } => {
                    Capability::CardinalityRange
                }
                Predicate::Overlap { .. }
                | Predicate::OverlapClique { .. }
                | Predicate::MinimumRest { .. } => Capability::AtMostOne,
            }
        };
        capabilities.insert(capability);
    }
    within(
        edges,
        limits.max_component_edges,
        AssignmentRuleLimit::PerRecord,
    )?;
    Ok(capabilities)
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
    levels: [EmptyLevel<'a>; 1],
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
