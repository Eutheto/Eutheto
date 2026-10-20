//! Capability, feature-usage, cost-summary, objective, and component analysis.

use crate::budget::{PlanningPreparationBudget, PlanningPreparationError, TransientReservation};
use crate::ids::{BoolVariableId, ComponentId, IntVariableId, IntervalVariableId};
use crate::model::{
    Capability, Constraint, LinearExpression, ObjectivePlan, ObjectiveTermKind, PlanningProblem,
    ProjectionExpression, Variable,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;

/// Rich deterministic feature inventory used before routing.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeatureUsageManifest {
    /// Count by required capability.
    pub capability_counts: BTreeMap<Capability, u64>,
    /// Count of optional intervals.
    pub optional_interval_count: u64,
    /// Count of global interval constraints.
    pub global_interval_constraint_count: u64,
    /// Count of table rows.
    pub table_row_count: u64,
    /// Count of table cells.
    pub table_cell_count: u64,
    /// Number of linear terms across constraints, objectives, and projections.
    pub linear_term_count: u64,
    /// Number of Boolean literal references.
    pub literal_reference_count: u64,
}

impl FeatureUsageManifest {
    fn add(&mut self, capability: Capability) {
        let entry = self.capability_counts.entry(capability).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    /// Exact set of capabilities used by the problem.
    #[must_use]
    pub fn required_capabilities(&self) -> BTreeSet<Capability> {
        self.capability_counts.keys().copied().collect()
    }
}

/// One mathematical connected component.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MathematicalComponent {
    /// Deterministic component identity derived from its first node.
    pub id: ComponentId,
    /// Typed variable IDs encoded as `bool:...`, `int:...`, or `interval:...`.
    pub variable_nodes: Vec<String>,
}

/// Component hypergraph result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentAnalysis {
    /// Canonical components sorted by their first node.
    pub components: Vec<MathematicalComponent>,
    /// Conservative number of pairwise union edges produced from hyperedges.
    pub edge_count: u64,
    /// BLAKE3 of the canonical components and edge count.
    pub component_hash: String,
}

/// Component analysis whose allocation remains charged to its preparation phase.
#[derive(Debug)]
pub struct BudgetedComponentAnalysis {
    analysis: ComponentAnalysis,
    reservation: Option<TransientReservation>,
}

impl BudgetedComponentAnalysis {
    /// Borrows the unchanged component-analysis payload.
    #[must_use]
    pub fn analysis(&self) -> &ComponentAnalysis {
        &self.analysis
    }

    pub(crate) fn unbudgeted(analysis: ComponentAnalysis) -> Self {
        Self {
            analysis,
            reservation: None,
        }
    }

    pub(crate) fn into_parts(self) -> (ComponentAnalysis, Option<TransientReservation>) {
        (self.analysis, self.reservation)
    }
}

/// Safe exact scalarization weights or required multipass solving.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    tag = "strategy",
    content = "value",
    deny_unknown_fields
)]
pub enum LexicographicStrategy {
    /// Each weight corresponds to the objective level at the same vector position.
    ExactScalarization { weights: Vec<i64> },
    /// Exact priority cannot be represented safely in `i64`.
    Multipass,
}

/// Deterministic redacted size and cost summary. Contains no scenario display text.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanningProblemSummary {
    pub schema_version: u32,
    pub variable_count: u64,
    pub bool_variable_count: u64,
    pub int_variable_count: u64,
    pub interval_variable_count: u64,
    pub constraint_count: u64,
    pub assumption_count: u64,
    pub objective_level_count: u64,
    pub objective_term_count: u64,
    pub lexicographic_strategy: LexicographicStrategy,
    pub projection_count: u64,
    pub provenance_count: u64,
    pub domain_range_count: u64,
    pub total_reference_count: u64,
    pub min_coefficient: Option<i64>,
    pub max_coefficient: Option<i64>,
    pub manifest: FeatureUsageManifest,
    pub components: ComponentAnalysis,
    pub canonical_ir_hash: String,
}

/// Typed variable node for graph and reference analysis.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum VariableNode<'a> {
    Bool(&'a BoolVariableId),
    Int(&'a IntVariableId),
    Interval(&'a IntervalVariableId),
}

impl VariableNode<'_> {
    fn encoded(&self) -> String {
        match self {
            Self::Bool(id) => format!("bool:{id}"),
            Self::Int(id) => format!("int:{id}"),
            Self::Interval(id) => format!("interval:{id}"),
        }
    }
}

/// Extracts the full feature inventory.
///
/// # Panics
/// Panics only if an internal invariant introduces a preparation failure without a budget.
#[must_use]
#[allow(
    clippy::expect_used,
    reason = "Every failure requires an actual preparation budget"
)]
pub fn feature_usage(problem: &PlanningProblem) -> FeatureUsageManifest {
    feature_usage_inner(problem, None).expect("uncontrolled feature analysis cannot fail")
}

/// Extracts the full feature inventory while inheriting preparation control/work accounting.
///
/// # Errors
/// Returns the inherited preparation interruption or finite-work admission failure.
pub fn feature_usage_with_budget(
    problem: &PlanningProblem,
    budget: &mut PlanningPreparationBudget,
) -> Result<FeatureUsageManifest, PlanningPreparationError> {
    feature_usage_inner(problem, Some(budget))
}

#[allow(clippy::too_many_lines)]
fn feature_usage_inner(
    problem: &PlanningProblem,
    mut budget: Option<&mut PlanningPreparationBudget>,
) -> Result<FeatureUsageManifest, PlanningPreparationError> {
    let mut usage = FeatureUsageManifest::default();
    for variable in &problem.variables {
        visit(&mut budget)?;
        if let Variable::Interval(interval) = variable
            && interval.presence.is_some()
        {
            usage.add(Capability::OptionalIntervals);
            usage.optional_interval_count = usage.optional_interval_count.saturating_add(1);
            usage.literal_reference_count = usage.literal_reference_count.saturating_add(1);
        }
    }
    for record in &problem.constraints {
        visit(&mut budget)?;
        usage.literal_reference_count = usage
            .literal_reference_count
            .saturating_add(record.enforcement.len() as u64);
        let capability = constraint_capability(&record.body);
        usage.add(capability);
        match &record.body {
            Constraint::BoolOr { literals }
            | Constraint::BoolAnd { literals }
            | Constraint::AtMostOne { literals }
            | Constraint::ExactlyOne { literals }
            | Constraint::CardinalityRange { literals, .. } => {
                usage.literal_reference_count = usage
                    .literal_reference_count
                    .saturating_add(literals.len() as u64);
            }
            Constraint::Implication { .. } | Constraint::Equivalence { .. } => {
                usage.literal_reference_count = usage.literal_reference_count.saturating_add(2);
            }
            Constraint::LinearComparison(comparison) => {
                usage.linear_term_count = usage
                    .linear_term_count
                    .saturating_add(comparison.expression.terms.len() as u64);
            }
            Constraint::ReifiedLinearComparison { comparison, .. } => {
                usage.literal_reference_count = usage.literal_reference_count.saturating_add(1);
                usage.linear_term_count = usage
                    .linear_term_count
                    .saturating_add(comparison.expression.terms.len() as u64);
            }
            Constraint::AllowedTable { variables, rows }
            | Constraint::ForbiddenTable { variables, rows } => {
                usage.table_row_count = usage.table_row_count.saturating_add(rows.len() as u64);
                usage.table_cell_count = usage
                    .table_cell_count
                    .saturating_add((variables.len() as u64).saturating_mul(rows.len() as u64));
            }
            Constraint::NoOverlap { .. } | Constraint::Cumulative { .. } => {
                usage.global_interval_constraint_count =
                    usage.global_interval_constraint_count.saturating_add(1);
            }
            Constraint::AllDifferent { .. }
            | Constraint::Element { .. }
            | Constraint::Min { .. }
            | Constraint::Max { .. }
            | Constraint::Equality { .. }
            | Constraint::AbsDifference { .. } => {}
        }
    }
    for level in &problem.objectives.levels {
        visit(&mut budget)?;
        for term in &level.terms {
            visit(&mut budget)?;
            usage.add(match term.kind {
                ObjectiveTermKind::Penalty => Capability::ObjectivePenalty,
                ObjectiveTermKind::Reward => Capability::ObjectiveReward,
            });
            usage.linear_term_count = usage
                .linear_term_count
                .saturating_add(term.expression.terms.len() as u64);
        }
    }
    if !problem.assumptions.is_empty() {
        visit(&mut budget)?;
        usage.add(Capability::Assumptions);
        usage.literal_reference_count = usage
            .literal_reference_count
            .saturating_add(problem.assumptions.len() as u64);
    }
    for projection in &problem.projections {
        visit(&mut budget)?;
        match &projection.expression {
            ProjectionExpression::Boolean(_) => usage.add(Capability::BooleanProjection),
            ProjectionExpression::Integer(_) => usage.add(Capability::IntegerProjection),
            ProjectionExpression::Linear(expression) => {
                usage.add(Capability::IntegerProjection);
                usage.linear_term_count = usage
                    .linear_term_count
                    .saturating_add(expression.terms.len() as u64);
            }
            ProjectionExpression::Interval(_) => usage.add(Capability::IntervalProjection),
            ProjectionExpression::Constant(value) => usage.add(match value {
                eutheto_domain_ir::AssignmentValue::Boolean(_) => Capability::BooleanProjection,
                eutheto_domain_ir::AssignmentValue::Integer(_) => Capability::IntegerProjection,
                eutheto_domain_ir::AssignmentValue::Interval(_) => Capability::IntervalProjection,
                eutheto_domain_ir::AssignmentValue::Absent => Capability::AbsentProjection,
            }),
        }
    }
    Ok(usage)
}

fn visit(
    budget: &mut Option<&mut PlanningPreparationBudget>,
) -> Result<(), PlanningPreparationError> {
    if let Some(budget) = budget.as_deref_mut() {
        budget.reserve_work(1)?;
    }
    Ok(())
}

fn constraint_capability(constraint: &Constraint) -> Capability {
    match constraint {
        Constraint::BoolOr { .. } => Capability::BoolOr,
        Constraint::BoolAnd { .. } => Capability::BoolAnd,
        Constraint::Implication { .. } => Capability::Implication,
        Constraint::Equivalence { .. } => Capability::Equivalence,
        Constraint::AtMostOne { .. } => Capability::AtMostOne,
        Constraint::ExactlyOne { .. } => Capability::ExactlyOne,
        Constraint::CardinalityRange { .. } => Capability::CardinalityRange,
        Constraint::LinearComparison(_) => Capability::LinearComparison,
        Constraint::ReifiedLinearComparison { .. } => Capability::ReifiedLinearComparison,
        Constraint::AllDifferent { .. } => Capability::AllDifferent,
        Constraint::AllowedTable { .. } => Capability::AllowedTable,
        Constraint::ForbiddenTable { .. } => Capability::ForbiddenTable,
        Constraint::Element { .. } => Capability::Element,
        Constraint::Min { .. } => Capability::Min,
        Constraint::Max { .. } => Capability::Max,
        Constraint::Equality { .. } => Capability::Equality,
        Constraint::AbsDifference { .. } => Capability::AbsDifference,
        Constraint::NoOverlap { .. } => Capability::NoOverlap,
        Constraint::Cumulative { .. } => Capability::Cumulative,
    }
}

/// Proves exact scalarization or selects exact multipass semantics.
#[must_use]
pub fn lexicographic_strategy(plan: &ObjectivePlan) -> LexicographicStrategy {
    if plan.levels.is_empty() {
        return LexicographicStrategy::ExactScalarization {
            weights: Vec::new(),
        };
    }
    let mut weights = vec![1_i64; plan.levels.len()];
    let mut lower_span = 1_i64;
    for index in (0..plan.levels.len()).rev() {
        weights[index] = lower_span;
        let level = &plan.levels[index];
        let Some(span) = level.upper_bound.checked_sub(level.lower_bound) else {
            return LexicographicStrategy::Multipass;
        };
        let Some(radix) = span.checked_add(1) else {
            return LexicographicStrategy::Multipass;
        };
        let Some(next) = lower_span.checked_mul(radix) else {
            return LexicographicStrategy::Multipass;
        };
        lower_span = next;
    }
    LexicographicStrategy::ExactScalarization { weights }
}

fn declared_nodes<'a>(
    problem: &'a PlanningProblem,
    budget: &mut Option<&mut PlanningPreparationBudget>,
) -> Result<(Option<TransientReservation>, Vec<VariableNode<'a>>), PlanningPreparationError> {
    let reservation = if let Some(budget) = budget.as_deref_mut() {
        let mut scratch = 4096_u64;
        for variable in &problem.variables {
            budget.reserve_work(1)?;
            let id_bytes = u64::try_from(variable.canonical_id().len())
                .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?;
            scratch = scratch
                .checked_add(
                    id_bytes
                        .checked_add(128)
                        .ok_or(PlanningPreparationError::ArithmeticOverflow)?,
                )
                .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        }
        let count = u64::try_from(problem.variables.len())
            .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?;
        budget.reserve_work(count)?;
        Some(budget.reserve_transient_owned(scratch)?)
    } else {
        None
    };
    let mut nodes: Vec<_> = problem
        .variables
        .iter()
        .map(|variable| match variable {
            Variable::Boolean(value) => VariableNode::Bool(&value.id),
            Variable::Integer(value) => VariableNode::Int(&value.id),
            Variable::Interval(value) => VariableNode::Interval(&value.id),
        })
        .collect();
    nodes.sort_unstable();
    nodes.dedup();
    Ok((reservation, nodes))
}

/// Computes mathematical components and a hash binding the exact result.
///
/// Constraints, interval structure, every objective level containing more than one variable,
/// projection expressions, and assumptions contribute hyperedges. A split is still forbidden
/// unless a validated [`crate::model::SplitAuthorization`] is present.
///
/// # Panics
/// Panics only if an internal invariant introduces a preparation failure without a budget.
#[must_use]
#[allow(
    clippy::expect_used,
    reason = "Every failure requires an actual preparation budget"
)]
pub fn analyze_components(problem: &PlanningProblem) -> ComponentAnalysis {
    analyze_components_inner(problem, None)
        .expect("uncontrolled component analysis cannot fail")
        .analysis
}

/// Computes components while inheriting preparation control/work accounting.
///
/// # Errors
/// Returns the inherited preparation interruption or finite-work admission failure.
pub fn analyze_components_with_budget(
    problem: &PlanningProblem,
    budget: &mut PlanningPreparationBudget,
) -> Result<BudgetedComponentAnalysis, PlanningPreparationError> {
    analyze_components_inner(problem, Some(budget))
}

#[allow(clippy::too_many_lines)]
fn analyze_components_inner(
    problem: &PlanningProblem,
    mut budget: Option<&mut PlanningPreparationBudget>,
) -> Result<BudgetedComponentAnalysis, PlanningPreparationError> {
    visit(&mut budget)?;
    let (declared_reservation, ordered) = declared_nodes(problem, &mut budget)?;
    let node_count = if budget.is_some() {
        u64::try_from(ordered.len()).map_err(|_| PlanningPreparationError::ArithmeticOverflow)?
    } else {
        ordered.len() as u64
    };

    let (index_reservation, indexes) = if let Some(budget) = budget.as_deref_mut() {
        let scratch = node_count
            .checked_mul(192)
            .and_then(|value| value.checked_add(4096))
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        budget.reserve_work(node_count)?;
        let reservation = budget.reserve_transient_owned(scratch)?;
        let indexes = ordered
            .iter()
            .copied()
            .enumerate()
            .map(|(index, node)| (node, index))
            .collect();
        (Some(reservation), indexes)
    } else {
        let indexes = ordered
            .iter()
            .copied()
            .enumerate()
            .map(|(index, node)| (node, index))
            .collect();
        (None, indexes)
    };
    let parent_reservation = if let Some(budget) = budget.as_deref_mut() {
        let usize_bytes = u64::try_from(size_of::<usize>())
            .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?;
        let scratch = node_count
            .checked_mul(usize_bytes)
            .and_then(|value| value.checked_mul(2))
            .and_then(|value| value.checked_add(4096))
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        budget.reserve_work(
            node_count
                .checked_mul(2)
                .ok_or(PlanningPreparationError::ArithmeticOverflow)?,
        )?;
        Some(budget.reserve_transient_owned(scratch)?)
    } else {
        None
    };
    let mut parent: Vec<usize> = (0..ordered.len()).collect();
    let mut size = vec![1_usize; ordered.len()];
    let mut edge_count = 0_u64;

    for variable in &problem.variables {
        visit(&mut budget)?;
        if let Variable::Interval(interval) = variable {
            let edge_len = if interval.presence.is_some() { 5 } else { 4 };
            let edge_reservation = reserve_edge(&mut budget, || Ok(edge_len))?;
            let edge = if budget.is_some() {
                let mut edge = Vec::with_capacity(edge_len);
                visit(&mut budget)?;
                edge.push(VariableNode::Interval(&interval.id));
                visit(&mut budget)?;
                edge.push(VariableNode::Int(&interval.start));
                visit(&mut budget)?;
                edge.push(VariableNode::Int(&interval.duration));
                visit(&mut budget)?;
                edge.push(VariableNode::Int(&interval.end));
                if let Some(literal) = &interval.presence {
                    visit(&mut budget)?;
                    edge.push(VariableNode::Bool(&literal.variable));
                }
                edge
            } else {
                let mut edge = vec![
                    VariableNode::Interval(&interval.id),
                    VariableNode::Int(&interval.start),
                    VariableNode::Int(&interval.duration),
                    VariableNode::Int(&interval.end),
                ];
                if let Some(literal) = &interval.presence {
                    edge.push(VariableNode::Bool(&literal.variable));
                }
                edge
            };
            union_edge(
                &edge,
                &indexes,
                &mut parent,
                &mut size,
                &mut edge_count,
                &mut budget,
            )?;
            drop(edge);
            drop(edge_reservation);
        }
    }
    for record in &problem.constraints {
        visit(&mut budget)?;
        let edge_len = if budget.is_some() {
            Some(
                constraint_node_capacity(&record.body)?
                    .checked_add(record.enforcement.len())
                    .ok_or(PlanningPreparationError::ArithmeticOverflow)?,
            )
        } else {
            None
        };
        let edge_reservation = reserve_edge(&mut budget, || Ok(edge_len.unwrap_or_default()))?;
        let mut edge = if let Some(capacity) = edge_len {
            let mut nodes = Vec::with_capacity(capacity);
            constraint_nodes_into_with_budget(&record.body, &mut nodes, &mut budget)?;
            nodes
        } else {
            constraint_nodes(&record.body)
        };
        for literal in &record.enforcement {
            visit(&mut budget)?;
            edge.push(VariableNode::Bool(&literal.variable));
        }
        union_edge(
            &edge,
            &indexes,
            &mut parent,
            &mut size,
            &mut edge_count,
            &mut budget,
        )?;
        drop(edge);
        drop(edge_reservation);
    }
    for level in &problem.objectives.levels {
        visit(&mut budget)?;
        let edge_len = if budget.is_some() {
            let mut edge_len = 0_usize;
            for term in &level.terms {
                visit(&mut budget)?;
                edge_len = edge_len
                    .checked_add(term.expression.terms.len())
                    .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
            }
            Some(edge_len)
        } else {
            None
        };
        let edge_reservation = reserve_edge(&mut budget, || Ok(edge_len.unwrap_or_default()))?;
        let mut edge = if let Some(capacity) = edge_len {
            Vec::with_capacity(capacity)
        } else {
            Vec::new()
        };
        for term in &level.terms {
            visit(&mut budget)?;
            if budget.is_some() {
                for expression_term in &term.expression.terms {
                    visit(&mut budget)?;
                    edge.push(VariableNode::Int(&expression_term.variable));
                }
            } else {
                edge.extend(expression_nodes(&term.expression));
            }
        }
        union_edge(
            &edge,
            &indexes,
            &mut parent,
            &mut size,
            &mut edge_count,
            &mut budget,
        )?;
        drop(edge);
        drop(edge_reservation);
    }
    for projection in &problem.projections {
        visit(&mut budget)?;
        let edge_len = if budget.is_some() {
            Some(projection_node_capacity(&projection.expression))
        } else {
            None
        };
        let edge_reservation = reserve_edge(&mut budget, || Ok(edge_len.unwrap_or_default()))?;
        let edge = projection_nodes_with_budget(&projection.expression, &mut budget)?;
        union_edge(
            &edge,
            &indexes,
            &mut parent,
            &mut size,
            &mut edge_count,
            &mut budget,
        )?;
        drop(edge);
        drop(edge_reservation);
    }
    for assumption in &problem.assumptions {
        visit(&mut budget)?;
        let edge_reservation = reserve_edge(&mut budget, || Ok(1))?;
        let edge = [VariableNode::Bool(&assumption.literal.variable)];
        union_edge(
            &edge,
            &indexes,
            &mut parent,
            &mut size,
            &mut edge_count,
            &mut budget,
        )?;
        drop(edge_reservation);
    }
    drop(indexes);
    drop(index_reservation);

    let (output_reservation, grouping_reservation) = if let Some(budget) = budget.as_deref_mut() {
        // Retained storage includes bounded encoded IDs, component IDs and Vec spare
        // capacity (up to four String slots for a singleton), not just node text.
        let output_bytes = node_count
            .checked_mul(512)
            .and_then(|value| value.checked_add(8192))
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        let grouping_bytes = node_count
            .checked_mul(128)
            .and_then(|value| value.checked_add(4096))
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        budget.reserve_work(node_count)?;
        (
            Some(budget.reserve_transient_owned(output_bytes)?),
            Some(budget.reserve_transient_owned(grouping_bytes)?),
        )
    } else {
        (None, None)
    };
    let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (index, node) in ordered.iter().enumerate() {
        visit(&mut budget)?;
        let root = find_root(&mut parent, index, &mut budget)?;
        grouped.entry(root).or_default().push(node.encoded());
    }
    drop(parent);
    drop(size);
    drop(parent_reservation);
    drop(ordered);
    drop(declared_reservation);

    let mut raw_groups: Vec<Vec<String>> = grouped.into_values().collect();
    for group in &mut raw_groups {
        visit(&mut budget)?;
        group.sort();
    }
    if let Some(budget) = budget.as_deref_mut() {
        budget.reserve_work(
            u64::try_from(raw_groups.len())
                .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?,
        )?;
    }
    raw_groups.sort();
    let components: Vec<_> = raw_groups
        .into_iter()
        .enumerate()
        .filter_map(|(index, variable_nodes)| {
            ComponentId::new(format!("component.c{index}"))
                .ok()
                .map(|id| MathematicalComponent { id, variable_nodes })
        })
        .collect();
    drop(grouping_reservation);
    visit(&mut budget)?;
    let component_hash = hash_components(&components, edge_count, &mut budget)?;
    Ok(BudgetedComponentAnalysis {
        analysis: ComponentAnalysis {
            components,
            edge_count,
            component_hash,
        },
        reservation: output_reservation,
    })
}

fn edge_scratch_bytes(edge_len: usize) -> Result<u64, PlanningPreparationError> {
    u64::try_from(edge_len)
        .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?
        .checked_mul(192)
        .and_then(|value| value.checked_add(1024))
        .ok_or(PlanningPreparationError::ArithmeticOverflow)
}

fn reserve_edge(
    budget: &mut Option<&mut PlanningPreparationBudget>,
    capacity: impl FnOnce() -> Result<usize, PlanningPreparationError>,
) -> Result<Option<TransientReservation>, PlanningPreparationError> {
    let Some(budget) = budget.as_deref_mut() else {
        return Ok(None);
    };
    let edge_len = capacity()?;
    let scratch = edge_scratch_bytes(edge_len)?;
    budget.reserve_work(
        u64::try_from(edge_len).map_err(|_| PlanningPreparationError::ArithmeticOverflow)?,
    )?;
    Ok(Some(budget.reserve_transient_owned(scratch)?))
}

fn constraint_node_capacity(constraint: &Constraint) -> Result<usize, PlanningPreparationError> {
    let capacity = match constraint {
        Constraint::BoolOr { literals }
        | Constraint::BoolAnd { literals }
        | Constraint::AtMostOne { literals }
        | Constraint::ExactlyOne { literals }
        | Constraint::CardinalityRange { literals, .. } => literals.len(),
        Constraint::Implication { .. }
        | Constraint::Equivalence { .. }
        | Constraint::Element { .. }
        | Constraint::Equality { .. } => 2,
        Constraint::LinearComparison(comparison) => comparison.expression.terms.len(),
        Constraint::ReifiedLinearComparison {
            literal: _,
            comparison,
        } => comparison
            .expression
            .terms
            .len()
            .checked_add(1)
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?,
        Constraint::AllDifferent { variables }
        | Constraint::AllowedTable { variables, .. }
        | Constraint::ForbiddenTable { variables, .. } => variables.len(),
        Constraint::Min { inputs, .. } | Constraint::Max { inputs, .. } => inputs
            .len()
            .checked_add(1)
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?,
        Constraint::AbsDifference { .. } => 3,
        Constraint::NoOverlap { intervals } | Constraint::Cumulative { intervals, .. } => {
            intervals.len()
        }
    };
    Ok(capacity)
}

fn projection_node_capacity(expression: &ProjectionExpression) -> usize {
    match expression {
        ProjectionExpression::Boolean(_)
        | ProjectionExpression::Integer(_)
        | ProjectionExpression::Interval(_) => 1,
        ProjectionExpression::Linear(expression) => expression.terms.len(),
        ProjectionExpression::Constant(_) => 0,
    }
}

fn hash_components(
    components: &[MathematicalComponent],
    edge_count: u64,
    budget: &mut Option<&mut PlanningPreparationBudget>,
) -> Result<String, PlanningPreparationError> {
    let hash_reservation = if let Some(budget) = budget.as_deref_mut() {
        Some(budget.reserve_transient_owned(4096)?)
    } else {
        None
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(&edge_count.to_le_bytes());
    for component in components {
        visit(budget)?;
        let component_len = if budget.is_some() {
            u64::try_from(component.variable_nodes.len())
                .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?
        } else {
            component.variable_nodes.len() as u64
        };
        hasher.update(&component_len.to_le_bytes());
        for node in &component.variable_nodes {
            visit(budget)?;
            let node_len = if budget.is_some() {
                u64::try_from(node.len())
                    .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?
            } else {
                node.len() as u64
            };
            hasher.update(&node_len.to_le_bytes());
            hasher.update(node.as_bytes());
        }
    }
    let hash = hasher.finalize().to_hex().to_string();
    drop(hasher);
    drop(hash_reservation);
    Ok(hash)
}

fn union_edge<'a>(
    edge: &[VariableNode<'a>],
    indexes: &BTreeMap<VariableNode<'a>, usize>,
    parent: &mut [usize],
    size: &mut [usize],
    edge_count: &mut u64,
    budget: &mut Option<&mut PlanningPreparationBudget>,
) -> Result<(), PlanningPreparationError> {
    let unique_scratch = reserve_edge(budget, || Ok(edge.len()))?;
    let result = (|| {
        let unique: BTreeSet<_> = edge.iter().collect();
        let mut positions = unique
            .into_iter()
            .filter_map(|node| indexes.get(node).copied());
        let Some(first) = positions.next() else {
            return Ok(());
        };
        for position in positions {
            union(parent, size, first, position, budget)?;
            *edge_count = if budget.is_some() {
                (*edge_count)
                    .checked_add(1)
                    .ok_or(PlanningPreparationError::ArithmeticOverflow)?
            } else {
                edge_count.saturating_add(1)
            };
        }
        Ok(())
    })();
    drop(unique_scratch);
    result
}

fn find_root(
    parent: &mut [usize],
    mut node: usize,
    budget: &mut Option<&mut PlanningPreparationBudget>,
) -> Result<usize, PlanningPreparationError> {
    while parent[node] != node {
        visit(budget)?;
        let grandparent = parent[parent[node]];
        parent[node] = grandparent;
        node = grandparent;
    }
    Ok(node)
}

// Size linking doubles an attached component, bounding depth by floor(log2(node_count)).
// The existing path-halving find therefore visits at most ceil(depth / 2) ancestors.
fn union(
    parent: &mut [usize],
    size: &mut [usize],
    left: usize,
    right: usize,
    budget: &mut Option<&mut PlanningPreparationBudget>,
) -> Result<(), PlanningPreparationError> {
    if let Some(budget) = budget.as_deref_mut() {
        budget.reserve_work(6)?;
    }
    let left_root = find_root(parent, left, budget)?;
    let right_root = find_root(parent, right, budget)?;
    if left_root != right_root {
        let combined = size[left_root]
            .checked_add(size[right_root])
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        let (winner, loser) = if size[left_root] > size[right_root]
            || (size[left_root] == size[right_root] && left_root < right_root)
        {
            (left_root, right_root)
        } else {
            (right_root, left_root)
        };
        parent[loser] = winner;
        size[winner] = combined;
    }
    Ok(())
}

pub(crate) fn expression_nodes(expression: &LinearExpression) -> Vec<VariableNode<'_>> {
    expression
        .terms
        .iter()
        .map(|term| VariableNode::Int(&term.variable))
        .collect()
}

pub(crate) fn projection_nodes(expression: &ProjectionExpression) -> Vec<VariableNode<'_>> {
    match expression {
        ProjectionExpression::Boolean(id) => vec![VariableNode::Bool(id)],
        ProjectionExpression::Integer(id) => vec![VariableNode::Int(id)],
        ProjectionExpression::Linear(expression) => expression_nodes(expression),
        ProjectionExpression::Interval(id) => vec![VariableNode::Interval(id)],
        ProjectionExpression::Constant(_) => Vec::new(),
    }
}

pub(crate) fn constraint_nodes(constraint: &Constraint) -> Vec<VariableNode<'_>> {
    match constraint {
        Constraint::BoolOr { literals }
        | Constraint::BoolAnd { literals }
        | Constraint::AtMostOne { literals }
        | Constraint::ExactlyOne { literals }
        | Constraint::CardinalityRange { literals, .. } => literals
            .iter()
            .map(|literal| VariableNode::Bool(&literal.variable))
            .collect(),
        Constraint::Implication {
            antecedent,
            consequent,
        } => vec![
            VariableNode::Bool(&antecedent.variable),
            VariableNode::Bool(&consequent.variable),
        ],
        Constraint::Equivalence { left, right } => vec![
            VariableNode::Bool(&left.variable),
            VariableNode::Bool(&right.variable),
        ],
        Constraint::LinearComparison(comparison) => expression_nodes(&comparison.expression),
        Constraint::ReifiedLinearComparison {
            literal,
            comparison,
        } => {
            let mut nodes = expression_nodes(&comparison.expression);
            nodes.push(VariableNode::Bool(&literal.variable));
            nodes
        }
        Constraint::AllDifferent { variables }
        | Constraint::AllowedTable { variables, .. }
        | Constraint::ForbiddenTable { variables, .. } => {
            variables.iter().map(VariableNode::Int).collect()
        }
        Constraint::Min { target, inputs } | Constraint::Max { target, inputs } => {
            let mut nodes = Vec::with_capacity(inputs.len().saturating_add(1));
            nodes.push(VariableNode::Int(target));
            nodes.extend(inputs.iter().map(VariableNode::Int));
            nodes
        }
        Constraint::Element { index, target, .. } => {
            vec![VariableNode::Int(index), VariableNode::Int(target)]
        }
        Constraint::Equality { left, right } => {
            vec![VariableNode::Int(left), VariableNode::Int(right)]
        }
        Constraint::AbsDifference {
            target,
            left,
            right,
        } => vec![
            VariableNode::Int(target),
            VariableNode::Int(left),
            VariableNode::Int(right),
        ],
        Constraint::NoOverlap { intervals } | Constraint::Cumulative { intervals, .. } => {
            intervals.iter().map(VariableNode::Interval).collect()
        }
    }
}

fn projection_nodes_with_budget<'a>(
    expression: &'a ProjectionExpression,
    budget: &mut Option<&mut PlanningPreparationBudget>,
) -> Result<Vec<VariableNode<'a>>, PlanningPreparationError> {
    let nodes = projection_nodes(expression);
    for _ in &nodes {
        visit(budget)?;
    }
    Ok(nodes)
}

fn constraint_nodes_into_with_budget<'a>(
    constraint: &'a Constraint,
    nodes: &mut Vec<VariableNode<'a>>,
    budget: &mut Option<&mut PlanningPreparationBudget>,
) -> Result<(), PlanningPreparationError> {
    match constraint {
        Constraint::BoolOr { literals }
        | Constraint::BoolAnd { literals }
        | Constraint::AtMostOne { literals }
        | Constraint::ExactlyOne { literals }
        | Constraint::CardinalityRange { literals, .. } => {
            for literal in literals {
                visit(budget)?;
                nodes.push(VariableNode::Bool(&literal.variable));
            }
        }
        Constraint::Implication {
            antecedent,
            consequent,
        } => {
            for literal in [antecedent, consequent] {
                visit(budget)?;
                nodes.push(VariableNode::Bool(&literal.variable));
            }
        }
        Constraint::Equivalence { left, right } => {
            for literal in [left, right] {
                visit(budget)?;
                nodes.push(VariableNode::Bool(&literal.variable));
            }
        }
        Constraint::LinearComparison(comparison) => {
            for term in &comparison.expression.terms {
                visit(budget)?;
                nodes.push(VariableNode::Int(&term.variable));
            }
        }
        Constraint::ReifiedLinearComparison {
            literal,
            comparison,
        } => {
            for term in &comparison.expression.terms {
                visit(budget)?;
                nodes.push(VariableNode::Int(&term.variable));
            }
            visit(budget)?;
            nodes.push(VariableNode::Bool(&literal.variable));
        }
        Constraint::AllDifferent { variables }
        | Constraint::AllowedTable { variables, .. }
        | Constraint::ForbiddenTable { variables, .. } => {
            for variable in variables {
                visit(budget)?;
                nodes.push(VariableNode::Int(variable));
            }
        }
        Constraint::Min { target, inputs } | Constraint::Max { target, inputs } => {
            visit(budget)?;
            nodes.push(VariableNode::Int(target));
            for input in inputs {
                visit(budget)?;
                nodes.push(VariableNode::Int(input));
            }
        }
        Constraint::Element { index, target, .. }
        | Constraint::Equality {
            left: index,
            right: target,
        } => {
            for variable in [index, target] {
                visit(budget)?;
                nodes.push(VariableNode::Int(variable));
            }
        }
        Constraint::AbsDifference {
            target,
            left,
            right,
        } => {
            for variable in [target, left, right] {
                visit(budget)?;
                nodes.push(VariableNode::Int(variable));
            }
        }
        Constraint::NoOverlap { intervals } | Constraint::Cumulative { intervals, .. } => {
            for interval in intervals {
                visit(budget)?;
                nodes.push(VariableNode::Interval(interval));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{find_root, union};
    use crate::{PlanningIrLimitsV1, PlanningPreparationBudget, PlanningPreparationError};

    #[test]
    fn size_linking_prevents_descending_union_chains() -> Result<(), PlanningPreparationError> {
        let count = 65;
        let mut parent: Vec<usize> = (0..count).collect();
        let mut size = vec![1; count];
        let mut budget = PlanningPreparationBudget::uncontrolled(PlanningIrLimitsV1::DEFAULT);
        for left in (0..count - 1).rev() {
            union(
                &mut parent,
                &mut size,
                left,
                left + 1,
                &mut Some(&mut budget),
            )?;
        }
        // The first equal-size merge chooses the lower root; later larger components
        // win even against a lower root index instead of building the old linear chain.
        assert_eq!(size[count - 2], count);
        for node in 0..count {
            let before = budget.charged_work();
            assert_eq!(
                find_root(&mut parent, node, &mut Some(&mut budget))?,
                count - 2
            );
            assert!(budget.charged_work() - before <= 1);
        }
        assert_eq!(
            parent
                .iter()
                .enumerate()
                .filter(|(index, root)| *index == **root)
                .map(|(root, _)| size[root])
                .sum::<usize>(),
            count
        );
        Ok(())
    }

    #[test]
    fn balanced_size_merges_bound_halving_visits_and_redundant_unions()
    -> Result<(), PlanningPreparationError> {
        for count in [0_usize, 1, 16, 17, 32, 33] {
            let mut parent: Vec<usize> = (0..count).collect();
            let mut size = vec![1; count];
            let mut width = 1;
            while width < count {
                for left in (0..count).step_by(2 * width) {
                    let right = left + width;
                    if right < count {
                        // Reverse arguments to protect the lower-root equal-size tie rule.
                        union(&mut parent, &mut size, right, left, &mut None)?;
                    }
                }
                width *= 2;
            }
            let bound = if count < 2 {
                0
            } else {
                u64::from(count.ilog2()).div_ceil(2)
            };
            let mut budget = PlanningPreparationBudget::uncontrolled(PlanningIrLimitsV1::DEFAULT);
            let mut deepest_visits = 0;
            for node in 0..count {
                let mut uncompressed = parent.clone();
                for repetition in 0..3 {
                    let before = budget.charged_work();
                    assert_eq!(
                        find_root(&mut uncompressed, node, &mut Some(&mut budget))?,
                        0
                    );
                    let visits = budget.charged_work() - before;
                    assert!(visits <= bound);
                    if repetition == 0 {
                        deepest_visits = deepest_visits.max(visits);
                    }
                }
            }
            // These balanced power/non-power trees attain the rounding boundary before
            // compression, rather than merely passing a loose bound on shallow trees.
            assert_eq!(deepest_visits, bound);
            assert_eq!(size.first().copied().unwrap_or(0), count);
            if count != 0 {
                let before_sizes = size.clone();
                let before_work = budget.charged_work();
                union(&mut parent, &mut size, 0, 0, &mut Some(&mut budget))?;
                assert_eq!(size, before_sizes);
                assert_eq!(budget.charged_work() - before_work, 6);
            }
        }
        Ok(())
    }

    #[test]
    fn size_merge_overflow_precedes_parent_and_size_mutation() {
        let mut parent = [0, 1];
        let mut size = [usize::MAX, 1];
        assert_eq!(
            union(&mut parent, &mut size, 0, 1, &mut None),
            Err(PlanningPreparationError::ArithmeticOverflow)
        );
        assert_eq!(parent, [0, 1]);
        assert_eq!(size, [usize::MAX, 1]);
    }
}
