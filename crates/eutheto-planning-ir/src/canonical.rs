//! Canonical normalization, serialization, hashing, and redacted summaries.

use crate::analysis::{
    PlanningProblemSummary, analyze_components, feature_usage, lexicographic_strategy,
};
use crate::model::{
    Constraint, IntDomain, LinearExpression, ModelError, ObjectivePlan, PlanningProblem,
    ProjectionExpression, SolveFingerprintInput, Variable,
};
use crate::validation::{PlanningIrLimitsV1, ValidationError, validate};
use std::fmt;

const SOLVE_FINGERPRINT_DOMAIN: &[u8] = b"eutheto.routed-solve-fingerprint.v1";
const OBJECTIVE_POLICY_DOMAIN: &[u8] = b"eutheto/planning-objective-policy/v1\0";

impl PlanningProblem {
    /// Canonicalizes every order-insensitive collection while preserving objective-level
    /// vector precedence.
    ///
    /// # Errors
    /// Returns a model error if a domain or linear expression cannot be normalized safely.
    pub fn canonicalize(&mut self) -> Result<(), ModelError> {
        for variable in &mut self.variables {
            if let Variable::Integer(integer) = variable {
                integer.domain = IntDomain::new(integer.domain.inclusive_ranges.clone())?;
            }
        }
        self.variables
            .sort_by(|left, right| left.canonical_id().cmp(right.canonical_id()));
        for record in &mut self.constraints {
            record.canonicalize();
            canonicalize_constraint_expressions(&mut record.body)?;
        }
        self.constraints
            .sort_by(|left, right| left.id.cmp(&right.id));
        canonicalize_objectives(&mut self.objectives)?;
        for assumption in &mut self.assumptions {
            assumption.required_rules.sort();
        }
        self.assumptions
            .sort_by(|left, right| left.id.cmp(&right.id));
        self.projections
            .sort_by(|left, right| left.id.cmp(&right.id));
        for projection in &mut self.projections {
            if let ProjectionExpression::Linear(expression) = &mut projection.expression {
                *expression = LinearExpression::new(expression.terms.clone(), expression.constant)?;
            }
        }
        self.provenance
            .sort_by(|left, right| left.id.cmp(&right.id));
        for record in &mut self.provenance {
            record.entity_refs.sort();
            record.entity_refs.dedup();
        }
        self.declared_capabilities = feature_usage(self).required_capabilities();
        Ok(())
    }
}

fn canonicalize_objectives(objectives: &mut ObjectivePlan) -> Result<(), ModelError> {
    for level in &mut objectives.levels {
        for term in &mut level.terms {
            term.expression =
                LinearExpression::new(term.expression.terms.clone(), term.expression.constant)?;
        }
        level.terms.sort_by(|left, right| left.id.cmp(&right.id));
    }
    Ok(())
}

fn canonicalize_constraint_expressions(constraint: &mut Constraint) -> Result<(), ModelError> {
    match constraint {
        Constraint::LinearComparison(comparison)
        | Constraint::ReifiedLinearComparison { comparison, .. } => {
            comparison.expression = LinearExpression::new(
                comparison.expression.terms.clone(),
                comparison.expression.constant,
            )?;
        }
        Constraint::BoolOr { .. }
        | Constraint::BoolAnd { .. }
        | Constraint::Implication { .. }
        | Constraint::Equivalence { .. }
        | Constraint::AtMostOne { .. }
        | Constraint::ExactlyOne { .. }
        | Constraint::CardinalityRange { .. }
        | Constraint::AllDifferent { .. }
        | Constraint::AllowedTable { .. }
        | Constraint::ForbiddenTable { .. }
        | Constraint::Element { .. }
        | Constraint::Min { .. }
        | Constraint::Max { .. }
        | Constraint::Equality { .. }
        | Constraint::AbsDifference { .. }
        | Constraint::NoOverlap { .. }
        | Constraint::Cumulative { .. } => {}
    }
    Ok(())
}

/// Canonical serialization/hash failure.
#[derive(Debug)]
pub enum CanonicalError {
    /// Normalization failed.
    Model(ModelError),
    /// Strict validation failed.
    Validation(ValidationError),
    /// Serialization failed.
    Serialization(serde_json::Error),
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "canonical planning IR error: {self:?}")
    }
}

impl std::error::Error for CanonicalError {}

impl From<ModelError> for CanonicalError {
    fn from(value: ModelError) -> Self {
        Self::Model(value)
    }
}

impl From<ValidationError> for CanonicalError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

impl From<serde_json::Error> for CanonicalError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value)
    }
}

/// Computes the deterministic post-route solve fingerprint.
///
/// The encoding is domain-separated and length-prefixes every field, so distinct backend IDs,
/// backend/adapter versions, models, and canonical option byte sequences cannot be ambiguous.
#[must_use]
pub fn routed_solve_fingerprint(input: &SolveFingerprintInput) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SOLVE_FINGERPRINT_DOMAIN);
    update_fingerprint_field(&mut hasher, input.canonical_ir_hash.as_bytes());
    update_fingerprint_field(&mut hasher, input.backend_id.as_bytes());
    update_fingerprint_field(&mut hasher, input.backend_version.as_bytes());
    update_fingerprint_field(&mut hasher, input.adapter_version.as_bytes());
    update_fingerprint_field(&mut hasher, &input.canonical_options);
    hasher.finalize().to_hex().to_string()
}

fn update_fingerprint_field(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_be_bytes());
    hasher.update(value);
}

/// Produces canonical JSON from strictly validated input.
///
/// Raw input is validated before the idempotent normalization pass so malformed declarations
/// cannot be repaired by canonicalization.
///
/// # Errors
/// Returns strict validation, normalization, or serialization failure.
pub fn canonical_json(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
) -> Result<Vec<u8>, CanonicalError> {
    validate(problem, limits)?;
    let mut canonical = problem.clone();
    canonical.canonicalize()?;
    validate(&canonical, limits)?;
    serde_json::to_vec(&canonical).map_err(CanonicalError::from)
}

/// Computes the pre-routing BLAKE3 model hash.
///
/// The hash covers schema v2, canonical variables/constraints/objectives/assumptions,
/// projections/provenance, compiler ID/version, and explicit compile metadata. It excludes
/// display text, split authorization, backend, adapter, and solve options. Post-route identity
/// belongs to [`SolveFingerprintInput`].
///
/// # Errors
/// Returns normalization, strict validation, or serialization failure.
pub fn canonical_ir_hash(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
) -> Result<String, CanonicalError> {
    validate(problem, limits)?;
    let mut semantic = problem.clone();
    semantic.canonicalize()?;
    semantic.metadata.display_text.clear();
    semantic.split_authorization = None;
    validate(&semantic, limits)?;
    let bytes = serde_json::to_vec(&semantic)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

/// Hashes the canonical objective policy for newly produced ordinary runs.
///
/// Level order is significant; term collection order is not. Nonobjective model fields
/// are validated but excluded from this identity. Historical retained policy hashes are
/// opaque bindings and must not be reinterpreted using this producer.
///
/// # Errors
/// Rejects invalid model/objective references, normalization errors, or serialization failure.
pub fn canonical_objective_policy_hash(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
) -> Result<String, CanonicalError> {
    validate(problem, limits)?;
    let mut objectives = problem.objectives.clone();
    canonicalize_objectives(&mut objectives)?;
    let bytes = serde_json::to_vec(&objectives)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(OBJECTIVE_POLICY_DOMAIN);
    hasher.update(&bytes);
    Ok(hasher.finalize().to_hex().to_string())
}

/// Builds a deterministic redacted model summary.
///
/// # Errors
/// Rejects invalid input or canonical hashing failure.
pub fn summarize(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
) -> Result<PlanningProblemSummary, CanonicalError> {
    validate(problem, limits)?;
    let mut canonical = problem.clone();
    canonical.canonicalize()?;
    validate(&canonical, limits)?;
    let manifest = feature_usage(&canonical);
    let components = analyze_components(&canonical);
    let canonical_ir_hash = canonical_ir_hash(&canonical, limits)?;
    let mut bool_variable_count = 0_u64;
    let mut int_variable_count = 0_u64;
    let mut interval_variable_count = 0_u64;
    let mut domain_range_count = 0_u64;
    for variable in &canonical.variables {
        match variable {
            Variable::Boolean(_) => bool_variable_count += 1,
            Variable::Integer(value) => {
                int_variable_count += 1;
                domain_range_count =
                    domain_range_count.saturating_add(value.domain.inclusive_ranges.len() as u64);
            }
            Variable::Interval(_) => interval_variable_count += 1,
        }
    }
    let mut objective_term_count = 0_u64;
    let mut coefficients = Vec::new();
    for record in &canonical.constraints {
        match &record.body {
            Constraint::LinearComparison(comparison)
            | Constraint::ReifiedLinearComparison { comparison, .. } => {
                coefficients.extend(
                    comparison
                        .expression
                        .terms
                        .iter()
                        .map(|term| term.coefficient),
                );
            }
            Constraint::BoolOr { .. }
            | Constraint::BoolAnd { .. }
            | Constraint::Implication { .. }
            | Constraint::Equivalence { .. }
            | Constraint::AtMostOne { .. }
            | Constraint::ExactlyOne { .. }
            | Constraint::CardinalityRange { .. }
            | Constraint::AllDifferent { .. }
            | Constraint::AllowedTable { .. }
            | Constraint::ForbiddenTable { .. }
            | Constraint::Element { .. }
            | Constraint::Min { .. }
            | Constraint::Max { .. }
            | Constraint::Equality { .. }
            | Constraint::AbsDifference { .. }
            | Constraint::NoOverlap { .. }
            | Constraint::Cumulative { .. } => {}
        }
    }
    for level in &canonical.objectives.levels {
        objective_term_count = objective_term_count.saturating_add(level.terms.len() as u64);
        coefficients.extend(
            level
                .terms
                .iter()
                .flat_map(|term| term.expression.terms.iter())
                .map(|term| term.coefficient),
        );
    }
    let min_coefficient = coefficients.iter().copied().min();
    let max_coefficient = coefficients.iter().copied().max();
    let total_reference_count = manifest
        .literal_reference_count
        .saturating_add(manifest.linear_term_count)
        .saturating_add(manifest.table_cell_count)
        .saturating_add(components.edge_count);
    Ok(PlanningProblemSummary {
        schema_version: canonical.schema_version,
        variable_count: canonical.variables.len() as u64,
        bool_variable_count,
        int_variable_count,
        interval_variable_count,
        constraint_count: canonical.constraints.len() as u64,
        assumption_count: canonical.assumptions.len() as u64,
        objective_level_count: canonical.objectives.levels.len() as u64,
        lexicographic_strategy: lexicographic_strategy(&canonical.objectives),
        objective_term_count,
        projection_count: canonical.projections.len() as u64,
        provenance_count: canonical.provenance.len() as u64,
        domain_range_count,
        total_reference_count,
        min_coefficient,
        max_coefficient,
        manifest,
        components,
        canonical_ir_hash,
    })
}
