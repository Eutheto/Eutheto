//! Strict bounded validation for the current untrusted planning IR schema.

use crate::analysis::{
    BudgetedComponentAnalysis, analyze_components, analyze_components_with_budget, feature_usage,
    feature_usage_with_budget,
};
use crate::budget::{PlanningPreparationBudget, PlanningPreparationError};
use crate::ids::{BoolVariableId, IntVariableId, IntervalVariableId, ProvenanceId};
use crate::model::{
    Constraint, IntDomain, LinearComparison, LinearExpression, Literal, PLANNING_IR_SCHEMA_VERSION,
    PlanningMetadata, PlanningProblem, ProjectionExpression, ProvenanceParameter, ProvenanceRecord,
    ProvenanceSourceKind, Variable,
};
use eutheto_domain_ir::{AssignmentValue, MAX_SCORE_LEVELS};
use eutheto_types::ResourceLimits;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::mem::size_of;

/// Independent planning-IR safety limits. Scenario document limits remain separate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanningIrLimitsV1 {
    pub max_ir_bytes: u64,
    pub max_variables: u64,
    pub max_constraints: u64,
    pub max_assumptions: u64,
    pub max_objective_levels: u64,
    pub max_objective_terms: u64,
    pub max_provenance_records: u64,
    pub max_provenance_depth: u64,
    pub max_parameters_per_record: u64,
    pub max_parameter_text_bytes: u64,
    pub max_entity_refs_per_record: u64,
    pub max_projections: u64,
    pub max_projection_expression_depth: u64,
    pub max_domain_ranges: u64,
    pub max_refs_per_node: u64,
    pub max_total_refs: u64,
    pub max_table_rows: u64,
    pub max_table_arity: u64,
    pub max_table_cells: u64,
    pub max_intervals_per_global: u64,
    pub max_enforcement_literals: u64,
    pub max_tags: u64,
    pub max_component_nodes: u64,
    pub max_component_edges: u64,
    pub max_id_bytes: u64,
    pub max_metadata_text_bytes: u64,
    pub max_abs_coefficient: i64,
    pub max_abs_value: i64,
}

impl PlanningIrLimitsV1 {
    /// Fixed schema-v1 limits.
    pub const DEFAULT: Self = Self {
        max_ir_bytes: 64 * 1024 * 1024,
        max_variables: 100_000,
        max_constraints: 500_000,
        max_assumptions: 100_000,
        max_objective_levels: MAX_SCORE_LEVELS as u64,
        max_objective_terms: 100_000,
        max_provenance_records: 500_000,
        max_provenance_depth: 32,
        max_parameters_per_record: 64,
        max_parameter_text_bytes: 1024,
        max_entity_refs_per_record: 1024,
        max_projections: 100_000,
        max_projection_expression_depth: 16,
        max_domain_ranges: 64,
        max_refs_per_node: 100_000,
        max_total_refs: 5_000_000,
        max_table_rows: 100_000,
        max_table_arity: 64,
        max_table_cells: 1_000_000,
        max_intervals_per_global: 100_000,
        max_enforcement_literals: 1024,
        max_tags: 32,
        max_component_nodes: 100_000,
        max_component_edges: 5_000_000,
        max_id_bytes: 160,
        max_metadata_text_bytes: 4 * 1024,
        max_abs_coefficient: 1_000_000_000_000,
        max_abs_value: 1_000_000_000_000_000,
    };

    /// Applies caller resource caps only where the shared contract permits tightening.
    #[must_use]
    pub const fn tightened_by(mut self, caller: ResourceLimits) -> Self {
        self.max_variables = min_u64(self.max_variables, caller.max_variables);
        self.max_constraints = min_u64(self.max_constraints, caller.max_constraints);
        self
    }
}

const fn min_u64(left: u64, right: u64) -> u64 {
    if left < right { left } else { right }
}

/// Stable validation code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationCode {
    UnsupportedVersion,
    MalformedJson,
    LimitExceeded,
    DuplicateId,
    MissingReference,
    MissingProvenance,
    NonCanonical,
    InvalidDomain,
    InvalidShape,
    ArithmeticOverflow,
    ValueOutOfBounds,
    InvalidObjectiveBounds,
    InvalidProjection,
    ProvenanceCycle,
    OrphanProvenance,
    UndeclaredCapability,
    InvalidSplitAuthorization,
}

/// Bounded, non-secret validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationError {
    pub code: ValidationCode,
    pub path: String,
}

impl ValidationError {
    fn new(code: ValidationCode, path: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "planning IR {:?} at {}", self.code, self.path)
    }
}

impl std::error::Error for ValidationError {}
/// Validation failure carrying inherited cooperative-preparation admission.
#[derive(Debug)]
pub enum BudgetedValidationError {
    /// The immutable IR violates a schema or canonical invariant.
    Invalid(ValidationError),
    /// The inherited preparation budget stopped the traversal.
    Preparation(PlanningPreparationError),
}

impl fmt::Display for BudgetedValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => error.fmt(formatter),
            Self::Preparation(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for BudgetedValidationError {}

impl From<ValidationError> for BudgetedValidationError {
    fn from(error: ValidationError) -> Self {
        Self::Invalid(error)
    }
}

impl From<PlanningPreparationError> for BudgetedValidationError {
    fn from(error: PlanningPreparationError) -> Self {
        Self::Preparation(error)
    }
}
type BudgetedResult<T> = Result<T, BudgetedValidationError>;

struct ValidationContext<'a> {
    budget: Option<&'a mut PlanningPreparationBudget>,
}

impl<'a> ValidationContext<'a> {
    fn uncontrolled() -> Self {
        Self { budget: None }
    }

    fn with_budget(budget: &'a mut PlanningPreparationBudget) -> Self {
        Self {
            budget: Some(budget),
        }
    }

    fn visit(&mut self) -> BudgetedResult<()> {
        if let Some(budget) = self.budget.as_deref_mut() {
            budget.reserve_work(1)?;
        }
        Ok(())
    }
}

/// Parses unknown input only after enforcing the byte ceiling, then validates every invariant.
///
/// # Errors
/// Rejects oversized/malformed JSON, unknown fields/versions, and any validation failure.
pub fn parse_and_validate(
    bytes: &[u8],
    limits: PlanningIrLimitsV1,
) -> Result<PlanningProblem, ValidationError> {
    if count(bytes.len())? > limits.max_ir_bytes {
        return Err(ValidationError::new(ValidationCode::LimitExceeded, "bytes"));
    }
    let problem: PlanningProblem = serde_json::from_slice(bytes)
        .map_err(|_| ValidationError::new(ValidationCode::MalformedJson, "json"))?;
    validate(&problem, limits)?;
    Ok(problem)
}

/// Strictly validates canonical current-schema IR.
///
/// # Errors
/// Returns the first deterministic validation failure.
pub fn validate(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
) -> Result<(), ValidationError> {
    let mut context = ValidationContext::uncontrolled();
    let (_, total_refs) =
        validate_core(problem, limits, &mut context).map_err(|error| match error {
            BudgetedValidationError::Invalid(error) => error,
            BudgetedValidationError::Preparation(_) => {
                unreachable!("uncontrolled validation has no preparation failure")
            }
        })?;
    validate_features_and_components(problem, total_refs, limits)?;
    Ok(())
}

fn validate_core<'a>(
    problem: &'a PlanningProblem,
    limits: PlanningIrLimitsV1,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<(VariableReferences<'a>, u64)> {
    validate_root(problem, limits, context)?;
    let (references, mut total_refs) = validate_variables(problem, limits, context)?;
    validate_constraints(problem, &references, limits, &mut total_refs, context)?;
    validate_objectives(
        problem,
        &references.ints,
        &references.provenance,
        limits,
        &mut total_refs,
        context,
    )?;
    validate_assumptions(
        problem,
        &references.bools,
        &references.provenance,
        &mut total_refs,
        limits,
        context,
    )?;
    validate_projections(
        problem,
        &references.bools,
        &references.ints,
        &references.intervals,
        &references.provenance,
        &mut total_refs,
        limits,
        context,
    )?;
    validate_provenance(problem, limits, context)?;
    validate_metadata(&problem.metadata, limits, context)?;
    Ok((references, total_refs))
}

/// Strictly validates canonical IR while charging nested graph traversal to inherited admission.
///
/// The ordinary [`validate`] API remains unchanged for callers that have no operation budget.
/// Preparation callers use this seam so cancellation and finite work checks occur while walking
/// source-controlled vectors and the actual validation/index helpers.
///
/// # Errors
/// Returns either the first validation failure or inherited preparation admission failure.
pub fn validate_with_budget(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
    budget: &mut PlanningPreparationBudget,
) -> Result<
    (
        crate::analysis::FeatureUsageManifest,
        BudgetedComponentAnalysis,
    ),
    BudgetedValidationError,
> {
    budget.check()?;
    validate_root_admission(problem, limits)?;
    let scratch = validation_scratch_bytes(problem, budget)?;
    let scratch_reservation = budget.reserve_transient_owned(scratch)?;
    let total_refs = {
        let mut context = ValidationContext::with_budget(budget);
        validate_core(problem, limits, &mut context).map(|(_, total_refs)| total_refs)?
    };
    drop(scratch_reservation);
    let manifest = feature_usage_with_budget(problem, budget)?;
    let components = analyze_components_with_budget(problem, budget)?;
    validate_features_and_components_with_analysis(
        problem,
        total_refs,
        limits,
        &manifest,
        components.analysis(),
    )?;
    budget.check()?;
    Ok((manifest, components))
}

fn validation_scratch_bytes(
    problem: &PlanningProblem,
    budget: &mut PlanningPreparationBudget,
) -> Result<u64, PlanningPreparationError> {
    let mut units = 0_u64;
    for value in [
        problem.variables.len(),
        problem.constraints.len(),
        problem.assumptions.len(),
        problem.objectives.levels.len(),
        problem.projections.len(),
        problem.provenance.len(),
    ] {
        budget.reserve_work(1)?;
        units = units
            .checked_add(
                u64::try_from(value).map_err(|_| PlanningPreparationError::ArithmeticOverflow)?,
            )
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
    }
    for record in &problem.constraints {
        budget.reserve_work(1)?;
        for count in [
            constraint_node_count(&record.body),
            record.enforcement.len(),
            record.tags.len(),
        ] {
            units = units
                .checked_add(
                    u64::try_from(count)
                        .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?,
                )
                .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        }
    }
    for assumption in &problem.assumptions {
        budget.reserve_work(1)?;
        units = units
            .checked_add(
                u64::try_from(assumption.required_rules.len())
                    .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?,
            )
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
    }
    for projection in &problem.projections {
        budget.reserve_work(1)?;
        units = units
            .checked_add(
                u64::try_from(projection_node_count(&projection.expression))
                    .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?,
            )
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
    }
    for level in &problem.objectives.levels {
        budget.reserve_work(1)?;
        units = units
            .checked_add(
                u64::try_from(level.terms.len())
                    .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?,
            )
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        for term in &level.terms {
            budget.reserve_work(1)?;
            units = units
                .checked_add(
                    u64::try_from(term.expression.terms.len())
                        .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?,
                )
                .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        }
    }
    units
        .checked_mul(128)
        .and_then(|value| value.checked_add(4096))
        .ok_or(PlanningPreparationError::ArithmeticOverflow)
}

fn validate_root_admission(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
) -> Result<(), ValidationError> {
    if problem.schema_version != PLANNING_IR_SCHEMA_VERSION {
        return Err(ValidationError::new(
            ValidationCode::UnsupportedVersion,
            "schemaVersion",
        ));
    }
    check_limit(problem.variables.len(), limits.max_variables, "variables")?;
    check_limit(
        problem.constraints.len(),
        limits.max_constraints,
        "constraints",
    )?;
    check_limit(
        problem.assumptions.len(),
        limits.max_assumptions,
        "assumptions",
    )?;
    check_limit(
        problem.objectives.levels.len(),
        limits.max_objective_levels,
        "objectives.levels",
    )?;
    check_limit(
        problem.projections.len(),
        limits.max_projections,
        "projections",
    )?;
    check_limit(
        problem.provenance.len(),
        limits.max_provenance_records,
        "provenance",
    )?;
    Ok(())
}

fn validate_root(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    context.visit()?;
    validate_root_admission(problem, limits)?;
    validate_root_order(problem, context)?;
    Ok(())
}

struct VariableReferences<'a> {
    provenance: BTreeSet<&'a ProvenanceId>,
    bools: BTreeSet<&'a BoolVariableId>,
    ints: BTreeMap<&'a IntVariableId, &'a IntDomain>,
    intervals: BTreeSet<&'a IntervalVariableId>,
}

fn validate_variables<'a>(
    problem: &'a PlanningProblem,
    limits: PlanningIrLimitsV1,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<(VariableReferences<'a>, u64)> {
    let mut provenance = BTreeSet::new();
    for record in &problem.provenance {
        context.visit()?;
        provenance.insert(&record.id);
    }
    let mut bools = BTreeSet::new();
    let mut ints = BTreeMap::new();
    let mut intervals = BTreeSet::new();
    let mut all_ids = BTreeSet::new();
    let mut total_refs = 0_u64;
    for (index, variable) in problem.variables.iter().enumerate() {
        context.visit()?;
        let (id, provenance_id) = match variable {
            Variable::Boolean(value) => {
                if !bools.insert(&value.id) {
                    return duplicate(format!("variables[{index}].id")).map_err(Into::into);
                }
                (value.id.as_str(), &value.provenance)
            }
            Variable::Integer(value) => {
                validate_domain(
                    &value.domain,
                    limits,
                    &format!("variables[{index}].domain"),
                    context,
                )?;
                if ints.insert(&value.id, &value.domain).is_some() {
                    return duplicate(format!("variables[{index}].id")).map_err(Into::into);
                }
                (value.id.as_str(), &value.provenance)
            }
            Variable::Interval(value) => {
                if !intervals.insert(&value.id) {
                    return duplicate(format!("variables[{index}].id")).map_err(Into::into);
                }
                total_refs = add_refs(total_refs, 4, limits)?;
                (value.id.as_str(), &value.provenance)
            }
        };
        if !all_ids.insert(id) {
            return duplicate(format!("variables[{index}].id")).map_err(Into::into);
        }
        require_provenance(provenance_id, &provenance, &format!("variables[{index}]"))?;
    }
    let references = VariableReferences {
        provenance,
        bools,
        ints,
        intervals,
    };
    validate_interval_variables(problem, &references, context)?;
    Ok((references, total_refs))
}

fn validate_interval_variables(
    problem: &PlanningProblem,
    references: &VariableReferences<'_>,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    for (index, variable) in problem.variables.iter().enumerate() {
        context.visit()?;
        if let Variable::Interval(interval) = variable {
            let start = require_int(
                &interval.start,
                &references.ints,
                &format!("variables[{index}].start"),
            )?;
            let duration = require_int(
                &interval.duration,
                &references.ints,
                &format!("variables[{index}].duration"),
            )?;
            let end = require_int(
                &interval.end,
                &references.ints,
                &format!("variables[{index}].end"),
            )?;
            if let Some(literal) = &interval.presence {
                require_literal(
                    literal,
                    &references.bools,
                    &format!("variables[{index}].presence"),
                )?;
            } else if duration
                .inclusive_ranges
                .iter()
                .any(|range| range.start < 0)
            {
                return Err(ValidationError::new(
                    ValidationCode::InvalidDomain,
                    format!("variables[{index}].duration"),
                )
                .into());
            }
            validate_interval_equation(start, duration, end, index, context)?;
        }
    }
    Ok(())
}

fn validate_interval_equation(
    start: &IntDomain,
    duration: &IntDomain,
    end: &IntDomain,
    index: usize,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    let mut has_representable_sum = false;
    let has_nonnegative_duration = duration.inclusive_ranges.iter().any(|range| range.end >= 0);
    for start_range in &start.inclusive_ranges {
        context.visit()?;
        for duration_range in &duration.inclusive_ranges {
            context.visit()?;
            let duration_minimum = duration_range.start.max(0);
            if duration_minimum > duration_range.end {
                continue;
            }
            let Some(sum_minimum) = start_range.start.checked_add(duration_minimum) else {
                continue;
            };
            has_representable_sum = true;
            let sum_maximum = start_range
                .end
                .checked_add(duration_range.end)
                .unwrap_or(i64::MAX);
            if domain_intersects_range(end, sum_minimum, sum_maximum) {
                return Ok(());
            }
        }
    }
    let path = format!("variables[{index}]");
    Err(ValidationError::new(
        if !has_nonnegative_duration || has_representable_sum {
            ValidationCode::InvalidDomain
        } else {
            ValidationCode::ArithmeticOverflow
        },
        path,
    )
    .into())
}

fn domain_intersects_range(domain: &IntDomain, minimum: i64, maximum: i64) -> bool {
    let first_after_maximum = domain
        .inclusive_ranges
        .partition_point(|range| range.start <= maximum);
    first_after_maximum != 0
        && domain.inclusive_ranges[..first_after_maximum]
            .last()
            .is_some_and(|range| range.end >= minimum)
}
fn constraint_node_count(constraint: &Constraint) -> usize {
    match constraint {
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
        Constraint::ReifiedLinearComparison { comparison, .. } => {
            comparison.expression.terms.len().saturating_add(1)
        }
        Constraint::AllDifferent { variables }
        | Constraint::AllowedTable { variables, .. }
        | Constraint::ForbiddenTable { variables, .. } => variables.len(),
        Constraint::Min { inputs, .. } | Constraint::Max { inputs, .. } => {
            inputs.len().saturating_add(1)
        }
        Constraint::AbsDifference { .. } => 3,
        Constraint::NoOverlap { intervals } | Constraint::Cumulative { intervals, .. } => {
            intervals.len()
        }
    }
}
fn projection_node_count(expression: &ProjectionExpression) -> usize {
    match expression {
        ProjectionExpression::Boolean(_)
        | ProjectionExpression::Integer(_)
        | ProjectionExpression::Interval(_) => 1,
        ProjectionExpression::Linear(expression) => expression.terms.len(),
        ProjectionExpression::Constant(_) => 0,
    }
}

fn validate_constraints(
    problem: &PlanningProblem,
    references: &VariableReferences<'_>,
    limits: PlanningIrLimitsV1,
    total_refs: &mut u64,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    let mut constraint_ids = BTreeSet::new();
    for (index, record) in problem.constraints.iter().enumerate() {
        context.visit()?;
        if !constraint_ids.insert(&record.id) {
            return duplicate(format!("constraints[{index}].id")).map_err(Into::into);
        }
        require_provenance(
            &record.provenance,
            &references.provenance,
            &format!("constraints[{index}]"),
        )?;
        check_limit(
            record.enforcement.len(),
            limits.max_enforcement_literals,
            &format!("constraints[{index}].enforcement"),
        )?;
        check_limit(
            record.tags.len(),
            limits.max_tags,
            &format!("constraints[{index}].tags"),
        )?;
        strict_with_context(
            &record.enforcement,
            &format!("constraints[{index}].enforcement"),
            context,
        )?;
        strict_with_context(&record.tags, &format!("constraints[{index}].tags"), context)?;
        for literal in &record.enforcement {
            context.visit()?;
            require_literal(
                literal,
                &references.bools,
                &format!("constraints[{index}].enforcement"),
            )?;
        }
        let refs = constraint_node_count(&record.body);
        check_limit(
            refs,
            limits.max_refs_per_node,
            &format!("constraints[{index}].body"),
        )?;
        *total_refs = add_refs(*total_refs, count(refs)?, limits)?;
        validate_constraint(
            &record.body,
            &references.bools,
            &references.ints,
            &references.intervals,
            limits,
            &format!("constraints[{index}].body"),
            context,
        )?;
    }
    Ok(())
}

fn validate_features_and_components(
    problem: &PlanningProblem,
    total_refs: u64,
    limits: PlanningIrLimitsV1,
) -> Result<(), ValidationError> {
    let usage = feature_usage(problem);
    let components = analyze_components(problem);
    validate_features_and_components_with_analysis(problem, total_refs, limits, &usage, &components)
}

fn validate_features_and_components_with_analysis(
    problem: &PlanningProblem,
    total_refs: u64,
    limits: PlanningIrLimitsV1,
    usage: &crate::analysis::FeatureUsageManifest,
    components: &crate::analysis::ComponentAnalysis,
) -> Result<(), ValidationError> {
    if usage
        .capability_counts
        .keys()
        .any(|capability| !problem.declared_capabilities.contains(capability))
    {
        return Err(ValidationError::new(
            ValidationCode::UndeclaredCapability,
            "declaredCapabilities",
        ));
    }
    if usage.table_row_count > limits.max_table_rows
        || usage.table_cell_count > limits.max_table_cells
    {
        return Err(ValidationError::new(
            ValidationCode::LimitExceeded,
            "tables",
        ));
    }
    if total_refs > limits.max_total_refs {
        return Err(ValidationError::new(
            ValidationCode::LimitExceeded,
            "totalReferences",
        ));
    }
    check_limit(
        components.components.len(),
        limits.max_component_nodes,
        "components.nodes",
    )?;
    if components.edge_count > limits.max_component_edges {
        return Err(ValidationError::new(
            ValidationCode::LimitExceeded,
            "components.edges",
        ));
    }
    if let Some(authorization) = &problem.split_authorization
        && (authorization.component_hash != components.component_hash
            || authorization.domain_merge_contract.is_empty()
            || count(authorization.domain_merge_contract.len())? > limits.max_metadata_text_bytes
            || !authorization.projection_independent)
    {
        return Err(ValidationError::new(
            ValidationCode::InvalidSplitAuthorization,
            "splitAuthorization",
        ));
    }
    Ok(())
}

fn validate_root_order(
    problem: &PlanningProblem,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    strict_by_with_context(
        &problem.variables,
        |variable| variable.canonical_id(),
        "variables",
        context,
    )?;
    strict_by_with_context(
        &problem.constraints,
        |record| &record.id,
        "constraints",
        context,
    )?;
    strict_by_with_context(
        &problem.assumptions,
        |record| &record.id,
        "assumptions",
        context,
    )?;
    strict_by_with_context(
        &problem.projections,
        |record| &record.id,
        "projections",
        context,
    )?;
    strict_by_with_context(
        &problem.provenance,
        |record| &record.id,
        "provenance",
        context,
    )?;
    Ok(())
}

fn validate_domain(
    domain: &IntDomain,
    limits: PlanningIrLimitsV1,
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    check_limit(
        domain.inclusive_ranges.len(),
        limits.max_domain_ranges,
        path,
    )?;
    for _ in &domain.inclusive_ranges {
        context.visit()?;
    }
    let normalized = IntDomain::new(domain.inclusive_ranges.clone())
        .map_err(|_| ValidationError::new(ValidationCode::InvalidDomain, path))?;
    if normalized != *domain {
        return noncanonical(path).map_err(Into::into);
    }
    for range in &domain.inclusive_ranges {
        context.visit()?;
        check_value(range.start, limits, path)?;
        check_value(range.end, limits, path)?;
    }
    Ok(())
}

fn validate_constraint(
    constraint: &Constraint,
    bools: &BTreeSet<&BoolVariableId>,
    ints: &BTreeMap<&IntVariableId, &IntDomain>,
    intervals: &BTreeSet<&IntervalVariableId>,
    limits: PlanningIrLimitsV1,
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    match constraint {
        Constraint::BoolOr { literals }
        | Constraint::BoolAnd { literals }
        | Constraint::AtMostOne { literals }
        | Constraint::ExactlyOne { literals } => {
            validate_literals(literals, bools, path, context)?;
        }
        Constraint::CardinalityRange { literals, min, max } => {
            validate_literals(literals, bools, path, context)?;
            let len = count(literals.len())?;
            if min > max || *max > len {
                return shape(path).map_err(Into::into);
            }
        }
        Constraint::Implication {
            antecedent,
            consequent,
        } => {
            context.visit()?;
            require_literal(antecedent, bools, path)?;
            require_literal(consequent, bools, path)?;
        }
        Constraint::Equivalence { left, right } => {
            context.visit()?;
            require_literal(left, bools, path)?;
            require_literal(right, bools, path)?;
        }
        Constraint::LinearComparison(comparison) => {
            validate_comparison(comparison, ints, limits, path, context)?;
        }
        Constraint::ReifiedLinearComparison {
            literal,
            comparison,
        } => {
            context.visit()?;
            require_literal(literal, bools, path)?;
            validate_comparison(comparison, ints, limits, path, context)?;
        }
        Constraint::AllDifferent { variables } => {
            strict_with_context(variables, path, context)?;
            for variable in variables {
                context.visit()?;
                require_int(variable, ints, path)?;
            }
        }
        Constraint::AllowedTable { variables, rows }
        | Constraint::ForbiddenTable { variables, rows } => {
            validate_table(variables, rows, ints, limits, path, context)?;
        }
        Constraint::Element {
            index,
            values,
            target,
        } => {
            require_int(index, ints, path)?;
            require_int(target, ints, path)?;
            for value in values {
                context.visit()?;
                check_value(*value, limits, path)?;
            }
        }
        Constraint::Min { target, inputs } | Constraint::Max { target, inputs } => {
            validate_integer_aggregate(target, inputs, ints, path, context)?;
        }
        Constraint::Equality { left, right } => {
            context.visit()?;
            require_int(left, ints, path)?;
            require_int(right, ints, path)?;
        }
        Constraint::AbsDifference {
            target,
            left,
            right,
        } => {
            context.visit()?;
            require_int(target, ints, path)?;
            let left_domain = require_int(left, ints, path)?;
            let right_domain = require_int(right, ints, path)?;
            prove_abs_bounds(left_domain, right_domain, path)?;
        }
        Constraint::NoOverlap { intervals: values } => {
            strict_with_context(values, path, context)?;
            check_limit(values.len(), limits.max_intervals_per_global, path)?;
            for interval in values {
                context.visit()?;
                require_interval(interval, intervals, path)?;
            }
        }
        Constraint::Cumulative {
            intervals: values,
            demands,
            capacity,
        } => {
            validate_cumulative(values, demands, *capacity, intervals, limits, path, context)?;
        }
    }
    Ok(())
}

fn validate_integer_aggregate(
    target: &IntVariableId,
    inputs: &[IntVariableId],
    ints: &BTreeMap<&IntVariableId, &IntDomain>,
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    require_int(target, ints, path)?;
    if inputs.is_empty() {
        return shape(path).map_err(Into::into);
    }
    strict_with_context(inputs, path, context)?;
    for input in inputs {
        context.visit()?;
        require_int(input, ints, path)?;
    }
    Ok(())
}

fn validate_table(
    variables: &[IntVariableId],
    rows: &[Vec<i64>],
    ints: &BTreeMap<&IntVariableId, &IntDomain>,
    limits: PlanningIrLimitsV1,
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    check_limit(variables.len(), limits.max_table_arity, path)?;
    check_limit(rows.len(), limits.max_table_rows, path)?;
    let arity = variables.len();
    if context.budget.is_some() {
        for variable in variables {
            context.visit()?;
            require_int(variable, ints, path)?;
        }
        let mut cells = 0_u64;
        for row in rows {
            context.visit()?;
            if row.len() != arity {
                return shape(path).map_err(Into::into);
            }
            cells = cells
                .checked_add(count(row.len())?)
                .ok_or_else(|| ValidationError::new(ValidationCode::ArithmeticOverflow, path))?;
        }
        if cells > limits.max_table_cells {
            return Err(ValidationError::new(ValidationCode::LimitExceeded, path).into());
        }
        strict_table_rows_with_context(rows, path, context)?;
    } else {
        strict_with_context(rows, path, context)?;
        for variable in variables {
            context.visit()?;
            require_int(variable, ints, path)?;
        }
        if rows.iter().any(|row| row.len() != arity) {
            return shape(path).map_err(Into::into);
        }
        let cells = count(rows.len())?
            .checked_mul(count(arity)?)
            .ok_or_else(|| ValidationError::new(ValidationCode::ArithmeticOverflow, path))?;
        if cells > limits.max_table_cells {
            return Err(ValidationError::new(ValidationCode::LimitExceeded, path).into());
        }
    }
    for row in rows {
        context.visit()?;
        for value in row {
            context.visit()?;
            check_value(*value, limits, path)?;
        }
    }
    Ok(())
}

fn strict_table_rows_with_context(
    rows: &[Vec<i64>],
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    for pair in rows.windows(2) {
        context.visit()?;
        let mut ordering = None;
        for (left, right) in pair[0].iter().zip(&pair[1]) {
            context.visit()?;
            if left != right {
                ordering = Some(left.cmp(right));
                break;
            }
        }
        let ordering = ordering.unwrap_or_else(|| pair[0].len().cmp(&pair[1].len()));
        if ordering != std::cmp::Ordering::Less {
            return noncanonical(path).map_err(Into::into);
        }
    }
    Ok(())
}

fn validate_cumulative(
    values: &[IntervalVariableId],
    demands: &[i64],
    capacity: i64,
    intervals: &BTreeSet<&IntervalVariableId>,
    limits: PlanningIrLimitsV1,
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    if values.len() != demands.len() || capacity < 0 {
        return shape(path).map_err(Into::into);
    }
    strict_with_context(values, path, context)?;
    check_limit(values.len(), limits.max_intervals_per_global, path)?;
    check_value(capacity, limits, path)?;
    let mut total = 0_i64;
    for (interval, demand) in values.iter().zip(demands) {
        context.visit()?;
        require_interval(interval, intervals, path)?;
        if *demand < 0 {
            return shape(path).map_err(Into::into);
        }
        check_value(*demand, limits, path)?;
        total = total
            .checked_add(*demand)
            .ok_or_else(|| ValidationError::new(ValidationCode::ArithmeticOverflow, path))?;
    }
    Ok(())
}

fn validate_objectives(
    problem: &PlanningProblem,
    ints: &BTreeMap<&IntVariableId, &IntDomain>,
    provenance: &BTreeSet<&ProvenanceId>,
    limits: PlanningIrLimitsV1,
    total_refs: &mut u64,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    let mut level_ids = BTreeSet::new();
    let mut term_count = 0_u64;
    for (level_index, level) in problem.objectives.levels.iter().enumerate() {
        context.visit()?;
        if !level_ids.insert(&level.id) {
            return duplicate(format!("objectives.levels[{level_index}].id")).map_err(Into::into);
        }
        require_provenance(
            &level.provenance,
            provenance,
            &format!("objectives.levels[{level_index}]"),
        )?;
        if level.lower_bound > level.upper_bound {
            return Err(ValidationError::new(
                ValidationCode::InvalidObjectiveBounds,
                format!("objectives.levels[{level_index}]"),
            )
            .into());
        }
        check_value(level.lower_bound, limits, "objectives.lowerBound")?;
        check_value(level.upper_bound, limits, "objectives.upperBound")?;
        strict_by_with_context(&level.terms, |term| &term.id, "objectives.terms", context)?;
        let mut exact_min = 0_i64;
        let mut exact_max = 0_i64;
        for term in &level.terms {
            context.visit()?;
            term_count = term_count.checked_add(1).ok_or_else(|| {
                ValidationError::new(ValidationCode::ArithmeticOverflow, "objectives.terms")
            })?;
            require_provenance(&term.provenance, provenance, "objectives.term")?;
            let (min, max) =
                expression_bounds(&term.expression, ints, limits, "objectives.term", context)?;
            if min < 0 {
                return Err(ValidationError::new(
                    ValidationCode::InvalidObjectiveBounds,
                    format!("objectives.levels[{level_index}].terms"),
                )
                .into());
            }
            exact_min = exact_min.checked_add(min).ok_or_else(|| {
                ValidationError::new(ValidationCode::ArithmeticOverflow, "objectives.lowerBound")
            })?;
            exact_max = exact_max.checked_add(max).ok_or_else(|| {
                ValidationError::new(ValidationCode::ArithmeticOverflow, "objectives.upperBound")
            })?;
            *total_refs = add_refs(*total_refs, count(term.expression.terms.len())?, limits)?;
        }
        if exact_min != level.lower_bound || exact_max != level.upper_bound {
            return Err(ValidationError::new(
                ValidationCode::InvalidObjectiveBounds,
                format!("objectives.levels[{level_index}]"),
            )
            .into());
        }
    }
    if term_count > limits.max_objective_terms {
        return Err(ValidationError::new(ValidationCode::LimitExceeded, "objectives.terms").into());
    }
    Ok(())
}

fn validate_assumptions(
    problem: &PlanningProblem,
    bools: &BTreeSet<&BoolVariableId>,
    provenance: &BTreeSet<&ProvenanceId>,
    total_refs: &mut u64,
    limits: PlanningIrLimitsV1,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    if problem.assumptions.is_empty() {
        return Ok(());
    }
    let mut provenance_records = BTreeMap::new();
    for record in &problem.provenance {
        context.visit()?;
        provenance_records.insert(&record.id, record);
    }
    let mut rule_ids = BTreeSet::new();
    let mut literal_variables = BTreeSet::new();
    for (index, assumption) in problem.assumptions.iter().enumerate() {
        context.visit()?;
        let path = format!("assumptions[{index}]");
        if assumption.required_rules.is_empty() {
            return shape(format!("{path}.requiredRules")).map_err(Into::into);
        }
        let refs = assumption
            .required_rules
            .len()
            .checked_add(1)
            .ok_or_else(|| {
                ValidationError::new(
                    ValidationCode::ArithmeticOverflow,
                    format!("{path}.references"),
                )
            })?;
        check_limit(
            refs,
            limits.max_refs_per_node,
            &format!("{path}.references"),
        )?;
        *total_refs = add_refs(*total_refs, count(refs)?, limits)?;
        strict_with_context(
            &assumption.required_rules,
            &format!("{path}.requiredRules"),
            context,
        )?;
        if !literal_variables.insert(&assumption.literal.variable) {
            return duplicate(format!("{path}.literal.variable")).map_err(Into::into);
        }
        require_literal(&assumption.literal, bools, &format!("{path}.literal"))?;
        require_provenance(
            &assumption.provenance,
            provenance,
            &format!("{path}.provenance"),
        )?;
        if provenance_records
            .get(&assumption.provenance)
            .is_none_or(|record| record.source_kind != ProvenanceSourceKind::RequiredRule)
        {
            return shape(format!("{path}.provenance")).map_err(Into::into);
        }
        for (rule_index, rule_id) in assumption.required_rules.iter().enumerate() {
            context.visit()?;
            if !rule_ids.insert(rule_id) {
                return duplicate(format!("{path}.requiredRules[{rule_index}]"))
                    .map_err(Into::into);
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_projections(
    problem: &PlanningProblem,
    bools: &BTreeSet<&BoolVariableId>,
    ints: &BTreeMap<&IntVariableId, &IntDomain>,
    intervals: &BTreeSet<&IntervalVariableId>,
    provenance: &BTreeSet<&ProvenanceId>,
    total_refs: &mut u64,
    limits: PlanningIrLimitsV1,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    let mut assignments = BTreeSet::new();
    for projection in &problem.projections {
        context.visit()?;
        if !assignments.insert(&projection.assignment_id) {
            return duplicate("projections.assignmentId").map_err(Into::into);
        }
        require_provenance(&projection.provenance, provenance, "projections")?;
        match &projection.expression {
            ProjectionExpression::Boolean(id) => {
                require_bool(id, bools, "projections.expression")?;
            }
            ProjectionExpression::Integer(id) => {
                require_int(id, ints, "projections.expression")?;
            }
            ProjectionExpression::Linear(expression) => {
                expression_bounds(expression, ints, limits, "projections.expression", context)?;
                for _ in &expression.terms {
                    context.visit()?;
                }
            }
            ProjectionExpression::Interval(id) => {
                require_interval(id, intervals, "projections.expression")?;
            }
            ProjectionExpression::Constant(AssignmentValue::Interval(interval)) => {
                interval.validate().map_err(|_| {
                    ValidationError::new(
                        ValidationCode::InvalidProjection,
                        "projections.expression",
                    )
                })?;
            }
            ProjectionExpression::Constant(_) => {}
        }
        let refs = projection_node_count(&projection.expression);
        check_limit(refs, limits.max_refs_per_node, "projections.expression")?;
        *total_refs = add_refs(*total_refs, count(refs)?, limits)?;
        if limits.max_projection_expression_depth < 1 {
            return Err(ValidationError::new(
                ValidationCode::LimitExceeded,
                "projections.expressionDepth",
            )
            .into());
        }
    }
    Ok(())
}

fn validate_provenance(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    let record_count = count(problem.provenance.len())?;
    let path_capacity =
        usize::try_from(record_count.min(limits.max_provenance_depth)).map_err(|_| {
            ValidationError::new(ValidationCode::ArithmeticOverflow, "provenance.depth")
        })?;
    let memo_reservation = if let Some(budget) = context.budget.as_deref_mut() {
        let bytes = record_count
            .checked_mul(size_of::<Option<u64>>() as u64)
            .and_then(|bytes| {
                (path_capacity as u64)
                    .checked_mul(size_of::<usize>() as u64)
                    .and_then(|path_bytes| bytes.checked_add(path_bytes))
            })
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        let reservation = budget.reserve_transient_owned(bytes)?;
        budget.reserve_work(record_count)?;
        Some(reservation)
    } else {
        None
    };
    let mut completed = vec![None; problem.provenance.len()];
    let mut path = Vec::with_capacity(path_capacity);
    for record in &problem.provenance {
        context.visit()?;
        check_limit(
            record.entity_refs.len(),
            limits.max_entity_refs_per_record,
            "provenance.entityRefs",
        )?;
        check_limit(
            record.parameters.len(),
            limits.max_parameters_per_record,
            "provenance.parameters",
        )?;
        strict_with_context(&record.entity_refs, "provenance.entityRefs", context)?;
        check_text(
            &record.source_id,
            limits.max_metadata_text_bytes,
            "provenance.sourceId",
        )?;
        check_text(
            &record.message_key,
            limits.max_metadata_text_bytes,
            "provenance.messageKey",
        )?;
        for (key, value) in &record.parameters {
            context.visit()?;
            check_text(
                key,
                limits.max_parameter_text_bytes,
                "provenance.parameters.key",
            )?;
            if let ProvenanceParameter::Text(text) = value {
                check_text(
                    text,
                    limits.max_parameter_text_bytes,
                    "provenance.parameters.text",
                )?;
            }
        }
        validate_provenance_chain(
            &record.id,
            &problem.provenance,
            &mut completed,
            &mut path,
            limits.max_provenance_depth,
            context,
        )?;
    }
    drop(path);
    drop(completed);
    drop(memo_reservation);
    validate_provenance_coverage(problem, context)?;
    Ok(())
}

fn validate_provenance_chain(
    start: &ProvenanceId,
    records: &[ProvenanceRecord],
    completed: &mut [Option<u64>],
    path: &mut Vec<usize>,
    depth_limit: u64,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    let mut current = Some(start);
    let mut tail_depth = 0_u64;
    while let Some(id) = current {
        if let Some(budget) = context.budget.as_deref_mut() {
            budget.reserve_work(8)?;
        }
        // Preserve cycle-before-depth-before-lookup precedence, including a repeated
        // current ID reached exactly at the authored depth boundary.
        for index in path.iter() {
            context.visit()?;
            if &records[*index].id == id {
                return Err(ValidationError::new(
                    ValidationCode::ProvenanceCycle,
                    "provenance.parent",
                )
                .into());
            }
        }
        let prefix_depth = count(path.len())?;
        let partial_depth = prefix_depth.checked_add(1).ok_or_else(|| {
            ValidationError::new(ValidationCode::ArithmeticOverflow, "provenance.depth")
        })?;
        if partial_depth > depth_limit {
            return Err(
                ValidationError::new(ValidationCode::LimitExceeded, "provenance.depth").into(),
            );
        }
        if let Some(budget) = context.budget.as_deref_mut() {
            // Standard binary search includes its final equality comparison.
            budget.reserve_work(u64::from(usize::BITS - records.len().leading_zeros()) + 1)?;
        }
        let index = records
            .binary_search_by(|record| record.id.cmp(id))
            .map_err(|_| {
                ValidationError::new(ValidationCode::MissingReference, "provenance.parent")
            })?;
        if let Some(depth) = completed[index] {
            // The cached depth already includes the current record: k+d, not k+1+d.
            let total_depth = prefix_depth.checked_add(depth).ok_or_else(|| {
                ValidationError::new(ValidationCode::ArithmeticOverflow, "provenance.depth")
            })?;
            if total_depth > depth_limit {
                return Err(ValidationError::new(
                    ValidationCode::LimitExceeded,
                    "provenance.depth",
                )
                .into());
            }
            tail_depth = depth;
            break;
        }
        path.push(index);
        current = records[index].parent.as_ref();
    }
    // Only a fully admitted suffix is cached; exact depths are assigned while unwinding.
    while let Some(index) = path.pop() {
        context.visit()?;
        tail_depth = tail_depth.checked_add(1).ok_or_else(|| {
            ValidationError::new(ValidationCode::ArithmeticOverflow, "provenance.depth")
        })?;
        completed[index] = Some(tail_depth);
    }
    Ok(())
}

fn validate_provenance_coverage(
    problem: &PlanningProblem,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    let mut used = BTreeSet::new();
    for variable in &problem.variables {
        context.visit()?;
        used.insert(variable.provenance());
    }
    for constraint in &problem.constraints {
        context.visit()?;
        used.insert(&constraint.provenance);
    }
    for level in &problem.objectives.levels {
        context.visit()?;
        used.insert(&level.provenance);
        for term in &level.terms {
            context.visit()?;
            used.insert(&term.provenance);
        }
    }
    for assumption in &problem.assumptions {
        context.visit()?;
        used.insert(&assumption.provenance);
    }
    for projection in &problem.projections {
        context.visit()?;
        used.insert(&projection.provenance);
    }
    let mut pending = Vec::new();
    for id in &used {
        context.visit()?;
        pending.push(*id);
    }
    while let Some(id) = pending.pop() {
        context.visit()?;
        if let Some(budget) = context.budget.as_deref_mut() {
            budget.reserve_work(
                u64::from(usize::BITS - problem.provenance.len().leading_zeros()) + 1,
            )?;
        }
        let index = problem
            .provenance
            .binary_search_by(|record| record.id.cmp(id))
            .map_err(|_| ValidationError::new(ValidationCode::MissingProvenance, "provenance"))?;
        let record = &problem.provenance[index];
        if let Some(parent) = &record.parent
            && used.insert(parent)
        {
            pending.push(parent);
        }
    }
    if used.len() != problem.provenance.len() {
        return Err(ValidationError::new(ValidationCode::OrphanProvenance, "provenance").into());
    }
    Ok(())
}

fn validate_metadata(
    metadata: &PlanningMetadata,
    limits: PlanningIrLimitsV1,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    check_text(
        &metadata.compiler_version,
        limits.max_metadata_text_bytes,
        "metadata.compilerVersion",
    )?;
    for (key, value) in &metadata.compile_metadata {
        context.visit()?;
        check_text(
            key.as_str(),
            limits.max_id_bytes,
            "metadata.compileMetadata.key",
        )?;
        if let ProvenanceParameter::Text(text) = value {
            check_text(
                text,
                limits.max_metadata_text_bytes,
                "metadata.compileMetadata.text",
            )?;
        }
    }
    for (key, value) in &metadata.display_text {
        context.visit()?;
        check_text(
            key,
            limits.max_metadata_text_bytes,
            "metadata.displayText.key",
        )?;
        check_text(
            value,
            limits.max_metadata_text_bytes,
            "metadata.displayText.value",
        )?;
    }
    Ok(())
}

fn validate_comparison(
    comparison: &LinearComparison,
    ints: &BTreeMap<&IntVariableId, &IntDomain>,
    limits: PlanningIrLimitsV1,
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    expression_bounds(&comparison.expression, ints, limits, path, context)?;
    check_value(comparison.rhs, limits, path)?;
    Ok(())
}

fn expression_bounds(
    expression: &LinearExpression,
    ints: &BTreeMap<&IntVariableId, &IntDomain>,
    limits: PlanningIrLimitsV1,
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<(i64, i64)> {
    strict_by_with_context(&expression.terms, |term| &term.variable, path, context)?;
    check_value(expression.constant, limits, path)?;
    let mut minimum = expression.constant;
    let mut maximum = expression.constant;
    for term in &expression.terms {
        context.visit()?;
        if term.coefficient == 0
            || term.coefficient.unsigned_abs() > limits.max_abs_coefficient.cast_unsigned()
        {
            return Err(ValidationError::new(ValidationCode::ValueOutOfBounds, path).into());
        }
        let domain = require_int(&term.variable, ints, path)?;
        let (lower, upper) = domain
            .bounds()
            .ok_or_else(|| ValidationError::new(ValidationCode::InvalidDomain, path))?;
        let first = term
            .coefficient
            .checked_mul(lower)
            .ok_or_else(|| ValidationError::new(ValidationCode::ArithmeticOverflow, path))?;
        let second = term
            .coefficient
            .checked_mul(upper)
            .ok_or_else(|| ValidationError::new(ValidationCode::ArithmeticOverflow, path))?;
        minimum = minimum
            .checked_add(first.min(second))
            .ok_or_else(|| ValidationError::new(ValidationCode::ArithmeticOverflow, path))?;
        maximum = maximum
            .checked_add(first.max(second))
            .ok_or_else(|| ValidationError::new(ValidationCode::ArithmeticOverflow, path))?;
    }
    Ok((minimum, maximum))
}

fn prove_abs_bounds(
    left: &IntDomain,
    right: &IntDomain,
    path: &str,
) -> Result<(), ValidationError> {
    let (left_min, left_max) = left
        .bounds()
        .ok_or_else(|| ValidationError::new(ValidationCode::InvalidDomain, path))?;
    let (right_min, right_max) = right
        .bounds()
        .ok_or_else(|| ValidationError::new(ValidationCode::InvalidDomain, path))?;
    left_min
        .checked_sub(right_max)
        .and_then(i64::checked_abs)
        .ok_or_else(|| ValidationError::new(ValidationCode::ArithmeticOverflow, path))?;
    left_max
        .checked_sub(right_min)
        .and_then(i64::checked_abs)
        .ok_or_else(|| ValidationError::new(ValidationCode::ArithmeticOverflow, path))?;
    Ok(())
}

fn validate_literals(
    literals: &[Literal],
    bools: &BTreeSet<&BoolVariableId>,
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    strict_with_context(literals, path, context)?;
    for literal in literals {
        context.visit()?;
        require_literal(literal, bools, path)?;
    }
    Ok(())
}

fn require_literal(
    literal: &Literal,
    bools: &BTreeSet<&BoolVariableId>,
    path: &str,
) -> Result<(), ValidationError> {
    require_bool(&literal.variable, bools, path)
}

fn require_bool(
    id: &BoolVariableId,
    bools: &BTreeSet<&BoolVariableId>,
    path: &str,
) -> Result<(), ValidationError> {
    if bools.contains(id) {
        Ok(())
    } else {
        Err(ValidationError::new(ValidationCode::MissingReference, path))
    }
}

fn require_int<'a>(
    id: &IntVariableId,
    ints: &BTreeMap<&'a IntVariableId, &'a IntDomain>,
    path: &str,
) -> Result<&'a IntDomain, ValidationError> {
    ints.get(id)
        .copied()
        .ok_or_else(|| ValidationError::new(ValidationCode::MissingReference, path))
}

fn require_interval(
    id: &IntervalVariableId,
    intervals: &BTreeSet<&IntervalVariableId>,
    path: &str,
) -> Result<(), ValidationError> {
    if intervals.contains(id) {
        Ok(())
    } else {
        Err(ValidationError::new(ValidationCode::MissingReference, path))
    }
}

fn require_provenance(
    id: &ProvenanceId,
    provenance: &BTreeSet<&ProvenanceId>,
    path: &str,
) -> Result<(), ValidationError> {
    if provenance.contains(id) {
        Ok(())
    } else {
        Err(ValidationError::new(
            ValidationCode::MissingProvenance,
            path,
        ))
    }
}

fn strict_with_context<T: Ord>(
    values: &[T],
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    for pair in values.windows(2) {
        context.visit()?;
        if pair[0] >= pair[1] {
            return noncanonical(path).map_err(Into::into);
        }
    }
    Ok(())
}

fn strict_by_with_context<T, K: Ord + ?Sized>(
    values: &[T],
    key: impl Fn(&T) -> &K,
    path: &str,
    context: &mut ValidationContext<'_>,
) -> BudgetedResult<()> {
    for pair in values.windows(2) {
        context.visit()?;
        if key(&pair[0]) >= key(&pair[1]) {
            return noncanonical(path).map_err(Into::into);
        }
    }
    Ok(())
}

fn check_value(value: i64, limits: PlanningIrLimitsV1, path: &str) -> Result<(), ValidationError> {
    if value.unsigned_abs() > limits.max_abs_value.cast_unsigned() {
        Err(ValidationError::new(ValidationCode::ValueOutOfBounds, path))
    } else {
        Ok(())
    }
}

fn check_text(value: &str, maximum: u64, path: &str) -> Result<(), ValidationError> {
    if value.is_empty() || count(value.len())? > maximum {
        Err(ValidationError::new(ValidationCode::LimitExceeded, path))
    } else {
        Ok(())
    }
}

fn check_limit(value: usize, maximum: u64, path: &str) -> Result<(), ValidationError> {
    if count(value)? > maximum {
        Err(ValidationError::new(ValidationCode::LimitExceeded, path))
    } else {
        Ok(())
    }
}

fn add_refs(
    total: u64,
    additional: u64,
    limits: PlanningIrLimitsV1,
) -> Result<u64, ValidationError> {
    let value = total.checked_add(additional).ok_or_else(|| {
        ValidationError::new(ValidationCode::ArithmeticOverflow, "totalReferences")
    })?;
    if value > limits.max_total_refs {
        Err(ValidationError::new(
            ValidationCode::LimitExceeded,
            "totalReferences",
        ))
    } else {
        Ok(value)
    }
}

fn count(value: usize) -> Result<u64, ValidationError> {
    u64::try_from(value)
        .map_err(|_| ValidationError::new(ValidationCode::ArithmeticOverflow, "count"))
}

fn duplicate<T>(path: impl Into<String>) -> Result<T, ValidationError> {
    Err(ValidationError::new(ValidationCode::DuplicateId, path))
}

fn noncanonical<T>(path: impl Into<String>) -> Result<T, ValidationError> {
    Err(ValidationError::new(ValidationCode::NonCanonical, path))
}

fn shape<T>(path: impl Into<String>) -> Result<T, ValidationError> {
    Err(ValidationError::new(ValidationCode::InvalidShape, path))
}
#[cfg(test)]
mod validation_work_tests {
    use super::*;
    use eutheto_types::{
        CancellationToken, DurationMillis, MonotonicClock, OperationControl, ParentSolveBudget,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn chain_record(
        index: usize,
        parent: Option<usize>,
    ) -> Result<ProvenanceRecord, crate::PlanningIdError> {
        Ok(ProvenanceRecord {
            id: ProvenanceId::new(format!("provenance.{index:03}"))?,
            source_kind: ProvenanceSourceKind::Fact,
            source_id: "test.source".to_owned(),
            entity_refs: Vec::new(),
            message_key: "test.message".to_owned(),
            parameters: BTreeMap::new(),
            parent: parent
                .map(|index| ProvenanceId::new(format!("provenance.{index:03}")))
                .transpose()?,
        })
    }

    #[test]
    fn shared_provenance_tail_work_is_bounded_and_failed_paths_are_not_cached()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut records = (0..80)
            .map(|index| {
                chain_record(
                    index,
                    if index < 64 {
                        Some(64)
                    } else if index < 79 {
                        Some(index + 1)
                    } else {
                        None
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut completed = vec![None; records.len()];
        let mut path = Vec::with_capacity(17);
        let mut budget = PlanningPreparationBudget::uncontrolled(PlanningIrLimitsV1::DEFAULT);
        for record in &records {
            validate_provenance_chain(
                &record.id,
                &records,
                &mut completed,
                &mut path,
                17,
                &mut ValidationContext::with_budget(&mut budget),
            )?;
        }
        for (index, depth) in completed.iter().enumerate() {
            assert_eq!(
                *depth,
                Some(if index < 64 {
                    17
                } else {
                    u64::try_from(80 - index)?
                })
            );
        }
        assert!(path.is_empty());
        let count = u64::try_from(records.len())?;
        let levels = u64::from(usize::BITS - records.len().leading_zeros());
        let bound = 2 * count + (2 * count + 1) * (17 + levels + 9);
        assert!(budget.charged_work() <= bound);

        records[0].parent = Some(ProvenanceId::new("provenance.missing")?);
        completed.fill(None);
        let mut budget = PlanningPreparationBudget::uncontrolled(PlanningIrLimitsV1::DEFAULT);
        assert!(matches!(
            validate_provenance_chain(
                &records[0].id,
                &records,
                &mut completed,
                &mut path,
                17,
                &mut ValidationContext::with_budget(&mut budget),
            ),
            Err(BudgetedValidationError::Invalid(ValidationError {
                code: ValidationCode::MissingReference,
                ..
            }))
        ));
        assert!(completed.iter().all(Option::is_none));
        assert!(budget.charged_work() <= bound);
        Ok(())
    }

    #[test]
    fn provenance_traversal_and_unwind_check_inherited_cancellation()
    -> Result<(), Box<dyn std::error::Error>> {
        struct CancelOnCheck {
            checks: AtomicUsize,
            cancel_at: usize,
            cancellation: CancellationToken,
        }
        impl MonotonicClock for CancelOnCheck {
            fn now(&self) -> Duration {
                if self.checks.fetch_add(1, Ordering::Relaxed) + 1 == self.cancel_at {
                    self.cancellation.cancel();
                }
                Duration::ZERO
            }
        }
        let records = [chain_record(0, Some(1))?, chain_record(1, None)?];
        // Check2 is the first binary lookup; check6 is the first unwind/cache write.
        // Parent construction observes the injected clock once before these checkpoints.
        for checkpoint in [2, 6] {
            let cancellation = CancellationToken::new();
            let clock = Arc::new(CancelOnCheck {
                checks: AtomicUsize::new(0),
                cancel_at: checkpoint + 1,
                cancellation: cancellation.clone(),
            });
            let parent =
                ParentSolveBudget::new(DurationMillis::new(1000)?, clock, cancellation.clone())?;
            let mut budget = PlanningPreparationBudget::new(
                OperationControl::Solve(parent.phase_view()),
                PlanningIrLimitsV1::DEFAULT,
            );
            let mut completed = [None; 2];
            let mut path = Vec::with_capacity(2);
            assert!(matches!(
                validate_provenance_chain(
                    &records[0].id,
                    &records,
                    &mut completed,
                    &mut path,
                    2,
                    &mut ValidationContext::with_budget(&mut budget),
                ),
                Err(BudgetedValidationError::Preparation(
                    PlanningPreparationError::Cancelled
                ))
            ));
            assert!(cancellation.is_cancelled());
            assert_eq!(completed, [None; 2]);
        }
        Ok(())
    }

    #[test]
    fn table_row_common_prefix_comparison_consumes_controlled_work() {
        let mut limits = PlanningIrLimitsV1::DEFAULT;
        limits.max_total_refs = 2;
        let mut budget = PlanningPreparationBudget::uncontrolled(limits);
        let mut context = ValidationContext::with_budget(&mut budget);
        let mut last = vec![0_i64; 64];
        last[63] = 1;
        let rows = vec![vec![0_i64; 64], last];

        assert!(matches!(
            strict_table_rows_with_context(&rows, "table.rows", &mut context),
            Err(BudgetedValidationError::Preparation(
                PlanningPreparationError::WorkLimitExceeded
            ))
        ));
    }
}
