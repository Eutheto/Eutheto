//! Canonical normalization, serialization, hashing, and redacted summaries.

use crate::analysis::{
    BudgetedComponentAnalysis, PlanningProblemSummary, analyze_components, feature_usage,
    lexicographic_strategy,
};
use crate::budget::{PlanningPreparationBudget, PlanningPreparationError, TransientReservation};
use crate::model::{
    Assumption, Capability, Constraint, ConstraintRecord, IntDomain, LinearExpression, ModelError,
    ObjectivePlan, PlanningProblem, ProjectionExpression, ProvenanceParameter, ProvenanceRecord,
    SolutionProjection, SolveFingerprintInput, SplitAuthorization, Variable,
};
use crate::validation::{
    BudgetedValidationError, PlanningIrLimitsV1, ValidationError, validate, validate_with_budget,
};
use eutheto_types::{PackId, ScenarioId};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;

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
                canonicalize_expression(expression)?;
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

fn canonicalize_expression(expression: &mut LinearExpression) -> Result<(), ModelError> {
    if expression.terms.iter().all(|term| term.coefficient != 0)
        && expression
            .terms
            .windows(2)
            .all(|pair| pair[0].variable < pair[1].variable)
    {
        return Ok(());
    }
    *expression = LinearExpression::new(expression.terms.clone(), expression.constant)?;
    Ok(())
}

fn canonicalize_objectives(objectives: &mut ObjectivePlan) -> Result<(), ModelError> {
    for level in &mut objectives.levels {
        for term in &mut level.terms {
            canonicalize_expression(&mut term.expression)?;
        }
        level.terms.sort_by(|left, right| left.id.cmp(&right.id));
    }
    Ok(())
}

fn canonicalize_constraint_expressions(constraint: &mut Constraint) -> Result<(), ModelError> {
    match constraint {
        Constraint::LinearComparison(comparison)
        | Constraint::ReifiedLinearComparison { comparison, .. } => {
            canonicalize_expression(&mut comparison.expression)?;
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
    /// Cooperative preparation admission or control failed.
    Preparation(PlanningPreparationError),
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
impl From<BudgetedValidationError> for CanonicalError {
    fn from(value: BudgetedValidationError) -> Self {
        match value {
            BudgetedValidationError::Invalid(error) => Self::Validation(error),
            BudgetedValidationError::Preparation(error) => Self::Preparation(error),
        }
    }
}
impl From<PlanningPreparationError> for CanonicalError {
    fn from(value: PlanningPreparationError) -> Self {
        Self::Preparation(value)
    }
}

impl From<serde_json::Error> for CanonicalError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value)
    }
}

/// A planning summary whose owned output remains charged to its preparation phase.
#[derive(Debug)]
pub struct BudgetedPlanningSummary {
    summary: PlanningProblemSummary,
    component_reservation: Option<TransientReservation>,
    summary_reservation: Option<TransientReservation>,
}

impl BudgetedPlanningSummary {
    /// Borrows the unchanged serialized summary payload.
    #[must_use]
    pub fn summary(&self) -> &PlanningProblemSummary {
        &self.summary
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        PlanningProblemSummary,
        Option<TransientReservation>,
        Option<TransientReservation>,
    ) {
        (
            self.summary,
            self.component_reservation,
            self.summary_reservation,
        )
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
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    let encoded_bytes = budget.precharge_serialized(problem, limits.max_ir_bytes)?;
    let scratch = PlanningPreparationBudget::serialized_scratch_bytes(encoded_bytes)?;
    let scratch_reservation = budget.reserve_transient_owned(scratch)?;
    let result = (|| {
        validate(problem, limits)?;
        let mut canonical = problem.clone();
        canonical.canonicalize()?;
        validate(&canonical, limits)?;
        budget.reserve_work(encoded_bytes / 64 + 1)?;
        budget.check()?;
        serde_json::to_vec(&canonical).map_err(CanonicalError::from)
    })();
    drop(scratch_reservation);
    result
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
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    let encoded_bytes = budget.precharge_serialized(problem, limits.max_ir_bytes)?;
    let scratch = PlanningPreparationBudget::serialized_scratch_bytes(encoded_bytes)?;
    let scratch_reservation = budget.reserve_transient_owned(scratch)?;
    let result = (|| {
        budget.reserve_work(encoded_bytes / 64 + 1)?;
        validate(problem, limits)?;
        let mut semantic = problem.clone();
        semantic.canonicalize()?;
        semantic.metadata.display_text.clear();
        semantic.split_authorization = None;
        validate(&semantic, limits)?;
        hash_canonical_problem_with_budget(&semantic, &semantic.declared_capabilities, &mut budget)
    })();
    drop(scratch_reservation);
    result
}

/// Computes the hash of an already canonical, strictly validated planning model while charging
/// borrowed serialization and inherited control through `budget`.
///
/// Unlike [`canonical_ir_hash`], this path never clones or normalizes the graph.  It is intended
/// for compiler-owned models and trial models whose only mutation is a canonical assumption edit.
///
/// # Errors
/// Returns validation, preparation-admission, or serialization failure.
pub fn canonical_ir_hash_with_budget(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
    budget: &mut PlanningPreparationBudget,
) -> Result<String, CanonicalError> {
    let encoded_bytes = budget.precharge_serialized(problem, limits.max_ir_bytes)?;
    let scratch = PlanningPreparationBudget::serialized_scratch_bytes(encoded_bytes)?;
    let scratch_reservation = budget.reserve_transient_owned(scratch)?;
    let result = (|| {
        let (manifest, components) = validate_with_budget(problem, limits, budget)?;
        drop(components);
        budget.check()?;
        let capability_reservation = budget.reserve_transient_owned(4096)?;
        let declared_capabilities = manifest.required_capabilities();
        let hash = hash_canonical_problem_with_budget(problem, &declared_capabilities, budget)?;
        drop(declared_capabilities);
        drop(capability_reservation);
        Ok(hash)
    })();
    drop(scratch_reservation);
    result
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
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    let encoded_bytes = budget.precharge_serialized(problem, limits.max_ir_bytes)?;
    let scratch = PlanningPreparationBudget::serialized_scratch_bytes(encoded_bytes)?;
    let scratch_reservation = budget.reserve_transient_owned(scratch)?;
    let result = (|| {
        validate(problem, limits)?;
        let mut canonical = problem.clone();
        canonical.canonicalize()?;
        validate(&canonical, limits)?;
        let manifest = feature_usage(&canonical);
        let components = analyze_components(&canonical);
        let declared_capabilities = manifest.required_capabilities();
        let canonical_ir_hash =
            hash_canonical_problem_with_budget(&canonical, &declared_capabilities, &mut budget)?;
        summary_from_canonical(&canonical, manifest, components, canonical_ir_hash)
            .map_err(CanonicalError::from)
    })();
    drop(scratch_reservation);
    result
}

/// Builds a deterministic summary from canonical compiler output under inherited admission.
///
/// The model must already use canonical collection ordering. Strict validation still runs before
/// the summary is exposed; callers cannot manufacture an admitted summary by opting out of it.
/// No graph clone or normalization is performed.
///
/// # Errors
/// Returns strict validation, preparation-admission, or serialization failure.
pub fn summarize_with_budget(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
    budget: &mut PlanningPreparationBudget,
) -> Result<BudgetedPlanningSummary, CanonicalError> {
    let encoded_bytes = budget.precharge_serialized(problem, limits.max_ir_bytes)?;
    let scratch = PlanningPreparationBudget::serialized_scratch_bytes(encoded_bytes)?;
    let scratch_reservation = budget.reserve_transient_owned(scratch)?;
    let result = (|| -> Result<BudgetedPlanningSummary, CanonicalError> {
        let (manifest, components) = validate_with_budget(problem, limits, budget)?;
        budget.check()?;
        let capability_reservation = budget.reserve_transient_owned(4096)?;
        let declared_capabilities = manifest.required_capabilities();
        let canonical_ir_hash =
            hash_canonical_problem_with_budget(problem, &declared_capabilities, budget)?;
        drop(declared_capabilities);
        drop(capability_reservation);
        summary_from_canonical_with_budget(problem, manifest, components, canonical_ir_hash, budget)
            .map_err(CanonicalError::from)
    })();
    drop(scratch_reservation);
    result
}

fn summary_from_canonical(
    canonical: &PlanningProblem,
    manifest: crate::FeatureUsageManifest,
    components: crate::ComponentAnalysis,
    canonical_ir_hash: String,
) -> Result<PlanningProblemSummary, PlanningPreparationError> {
    let summary = summary_from_canonical_inner(
        canonical,
        manifest,
        BudgetedComponentAnalysis::unbudgeted(components),
        canonical_ir_hash,
        None,
    )?;
    Ok(summary.summary)
}

fn summary_from_canonical_with_budget(
    canonical: &PlanningProblem,
    manifest: crate::FeatureUsageManifest,
    components: BudgetedComponentAnalysis,
    canonical_ir_hash: String,
    budget: &mut PlanningPreparationBudget,
) -> Result<BudgetedPlanningSummary, PlanningPreparationError> {
    summary_from_canonical_inner(
        canonical,
        manifest,
        components,
        canonical_ir_hash,
        Some(budget),
    )
}

fn summary_output_bytes(
    canonical: &PlanningProblem,
    manifest: &crate::FeatureUsageManifest,
) -> Result<u64, PlanningPreparationError> {
    let mut units = 0_u64;
    for count in [
        canonical.variables.len(),
        canonical.constraints.len(),
        canonical.assumptions.len(),
        canonical.objectives.levels.len(),
        canonical.projections.len(),
        canonical.provenance.len(),
    ] {
        units = units
            .checked_add(
                u64::try_from(count).map_err(|_| PlanningPreparationError::ArithmeticOverflow)?,
            )
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
    }
    for count in [
        manifest.literal_reference_count,
        manifest.linear_term_count,
        manifest.table_cell_count,
    ] {
        units = units
            .checked_add(count)
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
    }
    units
        .checked_mul(256)
        .and_then(|value| value.checked_add(8192))
        .ok_or(PlanningPreparationError::ArithmeticOverflow)
}

// Keep fallible summary construction and its ownership transfer in one scope.
#[allow(clippy::too_many_lines)]
fn summary_from_canonical_inner(
    canonical: &PlanningProblem,
    manifest: crate::FeatureUsageManifest,
    components: BudgetedComponentAnalysis,
    canonical_ir_hash: String,
    mut budget: Option<&mut PlanningPreparationBudget>,
) -> Result<BudgetedPlanningSummary, PlanningPreparationError> {
    let summary_reservation = if let Some(budget) = budget.as_deref_mut() {
        Some(budget.reserve_transient_owned(summary_output_bytes(canonical, &manifest)?)?)
    } else {
        None
    };
    if let Some(budget) = budget.as_deref_mut() {
        budget.reserve_work(1)?;
    }
    let mut bool_variable_count = 0_u64;
    let mut int_variable_count = 0_u64;
    let mut interval_variable_count = 0_u64;
    let mut domain_range_count = 0_u64;
    for variable in &canonical.variables {
        if let Some(budget) = budget.as_deref_mut() {
            budget.reserve_work(1)?;
        }
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
    let mut min_coefficient: Option<i64> = None;
    let mut max_coefficient: Option<i64> = None;
    for record in &canonical.constraints {
        if let Some(budget) = budget.as_deref_mut() {
            budget.reserve_work(1)?;
        }
        if let Constraint::LinearComparison(comparison)
        | Constraint::ReifiedLinearComparison { comparison, .. } = &record.body
        {
            for term in &comparison.expression.terms {
                if let Some(budget) = budget.as_deref_mut() {
                    budget.reserve_work(1)?;
                }
                min_coefficient = Some(
                    min_coefficient.map_or(term.coefficient, |value| value.min(term.coefficient)),
                );
                max_coefficient = Some(
                    max_coefficient.map_or(term.coefficient, |value| value.max(term.coefficient)),
                );
            }
        }
    }
    for level in &canonical.objectives.levels {
        if let Some(budget) = budget.as_deref_mut() {
            budget.reserve_work(1)?;
        }
        objective_term_count = objective_term_count.saturating_add(level.terms.len() as u64);
        for term in &level.terms {
            if let Some(budget) = budget.as_deref_mut() {
                budget.reserve_work(1)?;
            }
            for coefficient in term.expression.terms.iter().map(|term| term.coefficient) {
                if let Some(budget) = budget.as_deref_mut() {
                    budget.reserve_work(1)?;
                }
                min_coefficient =
                    Some(min_coefficient.map_or(coefficient, |value| value.min(coefficient)));
                max_coefficient =
                    Some(max_coefficient.map_or(coefficient, |value| value.max(coefficient)));
            }
        }
    }
    let component_edge_count = components.analysis().edge_count;
    let total_reference_count = manifest
        .literal_reference_count
        .saturating_add(manifest.linear_term_count)
        .saturating_add(manifest.table_cell_count)
        .saturating_add(component_edge_count);
    let (components, component_reservation) = components.into_parts();
    Ok(BudgetedPlanningSummary {
        summary: PlanningProblemSummary {
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
        },
        component_reservation,
        summary_reservation,
    })
}
/// An opaque model/summary pair admitted by the generic planning kernel.
///
/// The fields are private so a caller cannot forge a summary that bypasses canonical validation or
/// later pair a summary with a different model.  Holding the `Arc` here also prevents mutation
/// while the pair is in flight.
#[derive(Debug)]
struct PreparedReservation {
    _summary: Arc<PlanningProblemSummary>,
    retained: TransientReservation,
    component_reservation: Option<TransientReservation>,
    summary_reservation: Option<TransientReservation>,
}

#[derive(Clone, Debug)]
pub struct PreparedPlanningModel {
    problem: Arc<PlanningProblem>,
    summary: Arc<PlanningProblemSummary>,
    limits: PlanningIrLimitsV1,
    reservation: Arc<PreparedReservation>,
}

impl PreparedPlanningModel {
    /// Admits one canonical model and computes its deterministic summary exactly once.
    ///
    /// # Errors
    /// Returns strict validation, preparation-admission, or serialization failure.
    pub fn prepare(
        problem: Arc<PlanningProblem>,
        limits: PlanningIrLimitsV1,
        budget: &mut PlanningPreparationBudget,
    ) -> Result<Self, CanonicalError> {
        let mut units = 0_u64;
        for count in [
            problem.variables.len(),
            problem.constraints.len(),
            problem.assumptions.len(),
            problem.projections.len(),
            problem.provenance.len(),
        ] {
            units = units
                .checked_add(
                    u64::try_from(count)
                        .map_err(|_| PlanningPreparationError::ArithmeticOverflow)?,
                )
                .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        }
        let retained = units
            .checked_mul(384)
            .and_then(|value| value.checked_add(4096))
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        let retained_reservation = budget.reserve_transient_owned(retained)?;
        let summary = summarize_with_budget(&problem, limits, budget)?;
        let (summary, component_reservation, summary_reservation) = summary.into_parts();
        let summary = Arc::new(summary);
        Ok(Self {
            problem,
            summary: Arc::clone(&summary),
            limits,
            reservation: Arc::new(PreparedReservation {
                _summary: summary,
                retained: retained_reservation,
                component_reservation,
                summary_reservation,
            }),
        })
    }

    /// Releases the retained reservation after every request clone is dropped.
    ///
    /// # Errors
    /// Returns an accounting error if another owner remains live or the budget is not the origin.
    pub fn release(
        self,
        budget: &mut PlanningPreparationBudget,
    ) -> Result<(), PlanningPreparationError> {
        let reservation = &self.reservation;
        let owns_budget = reservation.retained.belongs_to(budget)
            && reservation
                .component_reservation
                .as_ref()
                .is_some_and(|guard| guard.belongs_to(budget))
            && reservation
                .summary_reservation
                .as_ref()
                .is_some_and(|guard| guard.belongs_to(budget));
        if Arc::strong_count(&self.reservation) != 1 || !owns_budget {
            return Err(PlanningPreparationError::ArithmeticOverflow);
        }
        drop(self);
        Ok(())
    }

    #[must_use]
    pub fn problem(&self) -> &Arc<PlanningProblem> {
        &self.problem
    }

    #[must_use]
    pub fn summary(&self) -> &PlanningProblemSummary {
        self.summary.as_ref()
    }

    #[must_use]
    pub const fn limits(&self) -> PlanningIrLimitsV1 {
        self.limits
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CanonicalHashProblem<'a> {
    schema_version: u32,
    variables: &'a [Variable],
    constraints: &'a [ConstraintRecord],
    objectives: &'a ObjectivePlan,
    assumptions: &'a [Assumption],
    projections: &'a [SolutionProjection],
    provenance: &'a [ProvenanceRecord],
    metadata: CanonicalHashMetadata<'a>,
    declared_capabilities: &'a BTreeSet<Capability>,
    split_authorization: Option<&'a SplitAuthorization>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CanonicalHashMetadata<'a> {
    pack_id: &'a PackId,
    scenario_id: &'a ScenarioId,
    scenario_revision: u64,
    projection_version: u32,
    compiler_id: &'a crate::CompilerId,
    compiler_version: &'a str,
    compile_metadata: &'a BTreeMap<crate::MetadataKey, ProvenanceParameter>,
    display_text: &'a BTreeMap<String, String>,
}

static EMPTY_DISPLAY_TEXT: BTreeMap<String, String> = BTreeMap::new();

fn canonical_hash_view<'a>(
    problem: &'a PlanningProblem,
    declared_capabilities: &'a BTreeSet<Capability>,
) -> CanonicalHashProblem<'a> {
    CanonicalHashProblem {
        schema_version: problem.schema_version,
        variables: &problem.variables,
        constraints: &problem.constraints,
        objectives: &problem.objectives,
        assumptions: &problem.assumptions,
        projections: &problem.projections,
        provenance: &problem.provenance,
        metadata: CanonicalHashMetadata {
            pack_id: &problem.metadata.pack_id,
            scenario_id: &problem.metadata.scenario_id,
            scenario_revision: problem.metadata.scenario_revision,
            projection_version: problem.metadata.projection_version,
            compiler_id: &problem.metadata.compiler_id,
            compiler_version: &problem.metadata.compiler_version,
            compile_metadata: &problem.metadata.compile_metadata,
            display_text: &EMPTY_DISPLAY_TEXT,
        },
        declared_capabilities,
        split_authorization: None,
    }
}

fn hash_canonical_problem_with_budget(
    problem: &PlanningProblem,
    declared_capabilities: &BTreeSet<Capability>,
    budget: &mut PlanningPreparationBudget,
) -> Result<String, CanonicalError> {
    let mut hasher = blake3::Hasher::new();
    let mut writer = ControlledHashWriter {
        hasher: &mut hasher,
        budget,
        failure: None,
    };
    let serialized = serde_json::to_writer(
        &mut writer,
        &canonical_hash_view(problem, declared_capabilities),
    );
    let failure = writer.failure;
    if let Some(error) = failure {
        return Err(error.into());
    }
    serialized?;
    Ok(hasher.finalize().to_hex().to_string())
}

struct ControlledHashWriter<'a> {
    hasher: &'a mut blake3::Hasher,
    budget: &'a mut PlanningPreparationBudget,
    failure: Option<PlanningPreparationError>,
}

impl Write for ControlledHashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length =
            u64::try_from(bytes.len()).map_err(|_| io::Error::other("hash length overflow"))?;
        if let Err(error) = self.budget.reserve_work(length / 64 + 1) {
            self.failure = Some(error);
            return Err(io::Error::other("planning preparation admission failed"));
        }
        self.hasher.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.budget
            .check()
            .map_err(|_| io::Error::other("planning preparation interrupted"))
    }
}
