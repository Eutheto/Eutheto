use crate::{ComparisonContext, ComparisonRunManifests, compare_accepted_results};
use eutheto_domain_ir::{
    AcceptedResult, AcceptedResultRefV1, CounterfactualCompilationBindingV1,
    CounterfactualConclusionV1, CounterfactualConditionV1, CounterfactualFailureKind,
    CounterfactualJobErrorV1, CounterfactualJobRequestV1, CounterfactualResult,
    DomainContractError, RunInfeasibilityEvidenceV1, RunInput, RunManifest, RunTerminalOutcomeV1,
    counterfactual_condition_satisfied,
};
use eutheto_planning_ir::{
    CanonicalError, ConstraintRecord, MetadataKey, PlanningIrLimitsV1, PlanningProblem,
    ProvenanceParameter, ProvenanceRecord, ProvenanceSourceKind, ValidationError,
    canonical_ir_hash, validate,
};
use eutheto_types::SolveStatus;
use std::collections::BTreeSet;
use std::fmt;

/// The sole compile-metadata key authorized for temporary counterfactual compilation.
pub const COUNTERFACTUAL_CONDITION_HASH_METADATA_KEY: &str =
    "eutheto.counterfactual.condition_hash";

/// Failure of the pure baseline-preservation validator.
#[derive(Debug)]
pub enum CounterfactualValidationError {
    /// The base planning problem is invalid.
    InvalidBase(ValidationError),
    /// The derived planning problem is invalid.
    InvalidDerived(ValidationError),
    /// The condition contract is invalid.
    InvalidCondition(DomainContractError),
    /// A protected baseline field changed.
    BaselineMutation(&'static str),
    /// Compile metadata was not exactly the one authorized condition-hash addition.
    InvalidConditionMetadata,
    /// No constraint backed by newly derived provenance was added.
    MissingConditionConstraint,
    /// A newly added constraint did not use newly added derived provenance.
    InvalidConditionProvenance,
    /// Canonical model hashing failed.
    ModelHash(CanonicalError),
    /// The derived model is not distinct from the base model.
    UnchangedModelHash,
    /// The strict compilation binding constructor rejected an input hash.
    InvalidBinding(DomainContractError),
}

impl fmt::Display for CounterfactualValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBase(error) => write!(formatter, "invalid base problem: {error}"),
            Self::InvalidDerived(error) => write!(formatter, "invalid derived problem: {error}"),
            Self::InvalidCondition(error) => write!(formatter, "invalid condition: {error}"),
            Self::BaselineMutation(field) => {
                write!(formatter, "counterfactual changed baseline {field}")
            }
            Self::InvalidConditionMetadata => {
                formatter.write_str("counterfactual condition metadata is invalid")
            }
            Self::MissingConditionConstraint => {
                formatter.write_str("counterfactual added no condition constraint")
            }
            Self::InvalidConditionProvenance => {
                formatter.write_str("counterfactual condition provenance is invalid")
            }
            Self::ModelHash(error) => {
                write!(formatter, "counterfactual model hashing failed: {error}")
            }
            Self::UnchangedModelHash => {
                formatter.write_str("counterfactual model hash is unchanged")
            }
            Self::InvalidBinding(error) => write!(
                formatter,
                "invalid counterfactual compilation binding: {error}"
            ),
        }
    }
}

impl std::error::Error for CounterfactualValidationError {}

/// Validates that `derived` is only an additive temporary-condition compilation of `base`.
///
/// All protected planning semantics must be byte-for-byte equal at the typed-record level. Every
/// base constraint and provenance record must survive unchanged; new constraints must use newly
/// added `Derived` provenance. Compile metadata must contain exactly one additional entry whose key
/// and value bind the supplied condition checksum.
///
/// # Errors
/// Rejects either invalid problem, any baseline mutation/removal, unauthorized metadata, malformed
/// additive provenance, hashing failure, or an unchanged model hash.
pub fn validate_counterfactual_problem(
    base: &PlanningProblem,
    derived: &PlanningProblem,
    condition: &CounterfactualConditionV1,
    objective_policy_hash: &str,
    limits: PlanningIrLimitsV1,
) -> Result<CounterfactualCompilationBindingV1, CounterfactualValidationError> {
    validate(base, limits).map_err(CounterfactualValidationError::InvalidBase)?;
    validate(derived, limits).map_err(CounterfactualValidationError::InvalidDerived)?;
    condition
        .validate()
        .map_err(CounterfactualValidationError::InvalidCondition)?;

    require_equal(
        &base.schema_version,
        &derived.schema_version,
        "schema version",
    )?;
    require_equal(&base.variables, &derived.variables, "variables")?;
    require_equal(&base.objectives, &derived.objectives, "objectives")?;
    require_equal(&base.assumptions, &derived.assumptions, "assumptions")?;
    require_equal(&base.projections, &derived.projections, "projections")?;
    require_equal(
        &base.declared_capabilities,
        &derived.declared_capabilities,
        "declared capabilities",
    )?;
    require_equal(
        &base.split_authorization,
        &derived.split_authorization,
        "split authorization",
    )?;

    if base.metadata.pack_id != derived.metadata.pack_id
        || base.metadata.scenario_id != derived.metadata.scenario_id
        || base.metadata.scenario_revision != derived.metadata.scenario_revision
        || base.metadata.projection_version != derived.metadata.projection_version
        || base.metadata.compiler_id != derived.metadata.compiler_id
        || base.metadata.compiler_version != derived.metadata.compiler_version
        || base.metadata.display_text != derived.metadata.display_text
    {
        return Err(CounterfactualValidationError::BaselineMutation("metadata"));
    }
    validate_condition_metadata(base, derived, condition)?;

    let added_provenance = validate_provenance_additions(&base.provenance, &derived.provenance)?;
    let added_constraints =
        validate_constraint_additions(&base.constraints, &derived.constraints, &added_provenance)?;
    if added_constraints == 0 {
        return Err(CounterfactualValidationError::MissingConditionConstraint);
    }

    let base_model_hash =
        canonical_ir_hash(base, limits).map_err(CounterfactualValidationError::ModelHash)?;
    let derived_model_hash =
        canonical_ir_hash(derived, limits).map_err(CounterfactualValidationError::ModelHash)?;
    if base_model_hash == derived_model_hash {
        return Err(CounterfactualValidationError::UnchangedModelHash);
    }
    CounterfactualCompilationBindingV1::new(
        base_model_hash,
        condition.checksum.clone(),
        derived_model_hash,
        objective_policy_hash.to_owned(),
    )
    .map_err(CounterfactualValidationError::InvalidBinding)
}

fn require_equal<T: PartialEq + ?Sized>(
    base: &T,
    derived: &T,
    field: &'static str,
) -> Result<(), CounterfactualValidationError> {
    if base == derived {
        Ok(())
    } else {
        Err(CounterfactualValidationError::BaselineMutation(field))
    }
}

fn validate_condition_metadata(
    base: &PlanningProblem,
    derived: &PlanningProblem,
    condition: &CounterfactualConditionV1,
) -> Result<(), CounterfactualValidationError> {
    let key = MetadataKey::new(COUNTERFACTUAL_CONDITION_HASH_METADATA_KEY)
        .map_err(|_| CounterfactualValidationError::InvalidConditionMetadata)?;
    if base.metadata.compile_metadata.contains_key(&key) {
        return Err(CounterfactualValidationError::InvalidConditionMetadata);
    }
    let mut expected = base.metadata.compile_metadata.clone();
    expected.insert(key, ProvenanceParameter::Text(condition.checksum.clone()));
    if derived.metadata.compile_metadata != expected {
        return Err(CounterfactualValidationError::InvalidConditionMetadata);
    }
    Ok(())
}

fn validate_provenance_additions(
    base: &[ProvenanceRecord],
    derived: &[ProvenanceRecord],
) -> Result<BTreeSet<eutheto_planning_ir::ProvenanceId>, CounterfactualValidationError> {
    let mut base_index = 0;
    let mut derived_index = 0;
    let mut added = BTreeSet::new();
    while derived_index < derived.len() {
        if let Some(original) = base.get(base_index) {
            match original.id.cmp(&derived[derived_index].id) {
                std::cmp::Ordering::Less => {
                    return Err(CounterfactualValidationError::BaselineMutation(
                        "provenance",
                    ));
                }
                std::cmp::Ordering::Equal => {
                    if original != &derived[derived_index] {
                        return Err(CounterfactualValidationError::BaselineMutation(
                            "provenance",
                        ));
                    }
                    base_index += 1;
                    derived_index += 1;
                }
                std::cmp::Ordering::Greater => {
                    let record = &derived[derived_index];
                    if record.source_kind != ProvenanceSourceKind::Derived {
                        return Err(CounterfactualValidationError::InvalidConditionProvenance);
                    }
                    added.insert(record.id.clone());
                    derived_index += 1;
                }
            }
        } else {
            let record = &derived[derived_index];
            if record.source_kind != ProvenanceSourceKind::Derived {
                return Err(CounterfactualValidationError::InvalidConditionProvenance);
            }
            added.insert(record.id.clone());
            derived_index += 1;
        }
    }
    if base_index != base.len() {
        return Err(CounterfactualValidationError::BaselineMutation(
            "provenance",
        ));
    }
    Ok(added)
}

fn validate_constraint_additions(
    base: &[ConstraintRecord],
    derived: &[ConstraintRecord],
    added_provenance: &BTreeSet<eutheto_planning_ir::ProvenanceId>,
) -> Result<usize, CounterfactualValidationError> {
    let mut base_index = 0;
    let mut derived_index = 0;
    let mut additions = 0;
    while derived_index < derived.len() {
        if let Some(original) = base.get(base_index) {
            match original.id.cmp(&derived[derived_index].id) {
                std::cmp::Ordering::Less => {
                    return Err(CounterfactualValidationError::BaselineMutation(
                        "constraints",
                    ));
                }
                std::cmp::Ordering::Equal => {
                    if original != &derived[derived_index] {
                        return Err(CounterfactualValidationError::BaselineMutation(
                            "constraints",
                        ));
                    }
                    base_index += 1;
                    derived_index += 1;
                }
                std::cmp::Ordering::Greater => {
                    if !added_provenance.contains(&derived[derived_index].provenance) {
                        return Err(CounterfactualValidationError::InvalidConditionProvenance);
                    }
                    additions += 1;
                    derived_index += 1;
                }
            }
        } else {
            if !added_provenance.contains(&derived[derived_index].provenance) {
                return Err(CounterfactualValidationError::InvalidConditionProvenance);
            }
            additions += 1;
            derived_index += 1;
        }
    }
    if base_index != base.len() {
        return Err(CounterfactualValidationError::BaselineMutation(
            "constraints",
        ));
    }
    Ok(additions)
}

/// Pure terminal interpretation of a counterfactual run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CounterfactualInterpretation {
    /// A contract-valid proof or independently verified alternative is complete.
    Completed(Box<CounterfactualResult>),
    /// Work was explicitly cancelled and therefore carries no proof result.
    Cancelled,
    /// Started work ended without a terminal proof result.
    Interrupted,
    /// A safe typed failure, with no successful conclusion.
    Failed(CounterfactualJobErrorV1),
}

/// Interprets one terminal counterfactual run using only validated domain authority.
///
/// The function cross-binds request semantics, both run inputs/manifests, snapshot and model hashes,
/// the compilation condition, accepted-result and verifier checksums, and (when present) the
/// independently accepted alternative. Solver-native outcomes or claims are never consumed.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn interpret_counterfactual(
    request: &CounterfactualJobRequestV1,
    base_run_input: &RunInput,
    base_run_manifest: &RunManifest,
    base: &AcceptedResult,
    compilation: &CounterfactualCompilationBindingV1,
    derived_run_input: &RunInput,
    derived_run_manifest: &RunManifest,
    alternative: Option<&AcceptedResult>,
    infeasibility: Option<&RunInfeasibilityEvidenceV1>,
) -> CounterfactualInterpretation {
    if request.validate().is_err()
        || base_run_input.validate().is_err()
        || base_run_manifest.validate().is_err()
        || compilation.validate().is_err()
        || derived_run_input.validate().is_err()
        || derived_run_manifest.validate().is_err()
    {
        return failed(CounterfactualFailureKind::InvalidBinding);
    }
    if base.validate().is_err() {
        return failed(CounterfactualFailureKind::InvalidCandidate);
    }

    if request.semantics.scenario_id != base_run_input.scenario_id
        || request.semantics.scenario_revision != base_run_input.scenario_revision
        || request.semantics.scenario_id != base.solution.scenario_id
        || request.semantics.scenario_revision != base.solution.scenario_revision
        || derived_run_input.scenario_id != request.semantics.scenario_id
        || derived_run_input.scenario_revision != request.semantics.scenario_revision
    {
        return failed(CounterfactualFailureKind::StaleRevision);
    }
    if !base_bindings_match(request, base_run_input, base_run_manifest, base)
        || !derived_bindings_match(
            request,
            base_run_input,
            compilation,
            derived_run_input,
            derived_run_manifest,
        )
    {
        return failed(CounterfactualFailureKind::InvalidBinding);
    }
    if let Some(evidence) = infeasibility
        && (evidence
            .validate_binding(derived_run_input, derived_run_manifest)
            .is_err()
            || evidence.condition.as_ref() != Some(&request.condition))
    {
        return failed(CounterfactualFailureKind::InvalidBinding);
    }

    match &derived_run_manifest.outcome {
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible | SolveStatus::NoSolutionWithinLimit,
        } => {
            if alternative.is_some() {
                return failed(CounterfactualFailureKind::InvalidCandidate);
            }
            let conclusion = if infeasibility.is_some_and(|evidence| evidence.diagnostic.is_some())
            {
                CounterfactualConclusionV1::ProvenImpossible
            } else {
                CounterfactualConclusionV1::NotDistinguishedWithinBudget
            };
            completed(
                request,
                base_run_input,
                base_run_manifest,
                compilation,
                derived_run_input,
                derived_run_manifest,
                conclusion,
            )
        }
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Cancelled,
        } => CounterfactualInterpretation::Cancelled,
        RunTerminalOutcomeV1::Interrupted => CounterfactualInterpretation::Interrupted,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::BackendUnavailable,
        } => failed(CounterfactualFailureKind::BackendUnavailable),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::BackendFailed,
        } => failed(CounterfactualFailureKind::BackendFailed),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::InvalidModel | SolveStatus::Unbounded,
        } => failed(CounterfactualFailureKind::InvalidModel),
        RunTerminalOutcomeV1::VerificationAlarm { .. } => {
            failed(CounterfactualFailureKind::InvalidCandidate)
        }
        RunTerminalOutcomeV1::Accepted { .. } => verified_alternative(
            request,
            base_run_input,
            base_run_manifest,
            base,
            compilation,
            derived_run_input,
            derived_run_manifest,
            alternative,
        ),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Optimal | SolveStatus::Feasible,
        } => failed(CounterfactualFailureKind::InvalidBinding),
    }
}

fn base_bindings_match(
    request: &CounterfactualJobRequestV1,
    input: &RunInput,
    manifest: &RunManifest,
    accepted: &AcceptedResult,
) -> bool {
    let semantics = &request.semantics;
    let accepted_ref = AcceptedResultRefV1 {
        solution_id: accepted.solution.solution_id,
        result_checksum: accepted.checksum.clone(),
    };
    input.run_id == semantics.base_run_id
        && input.checksum == semantics.base_run_input_checksum
        && input.snapshot_id == semantics.snapshot_id
        && input.snapshot_document_hash == semantics.snapshot_document_hash
        && input.model_hash == semantics.base_model_hash
        && input.objective_policy_hash == semantics.objective_policy_hash
        && input.temporary_condition_hash.is_none()
        && input.pack_id == accepted.solution.pack_id
        && input.snapshot_document_hash == accepted.verification.document_hash
        && input.model_hash == accepted.verification.planning_model_hash
        && accepted_ref == semantics.base
        && manifest.run_id == input.run_id
        && manifest.run_input_checksum == input.checksum
        && matches!(
            &manifest.outcome,
            RunTerminalOutcomeV1::Accepted {
                solution_id,
                accepted_result_checksum,
                verification_checksum,
                ..
            } if solution_id == &accepted.solution.solution_id
                && accepted_result_checksum == &accepted.checksum
                && verification_checksum == &accepted.verification.checksum
        )
}

fn derived_bindings_match(
    request: &CounterfactualJobRequestV1,
    base_input: &RunInput,
    compilation: &CounterfactualCompilationBindingV1,
    input: &RunInput,
    manifest: &RunManifest,
) -> bool {
    compilation.base_model_hash == request.semantics.base_model_hash
        && compilation.base_model_hash != compilation.derived_model_hash
        && compilation.condition_checksum == request.condition.checksum
        && compilation.condition_checksum == request.semantics.condition_checksum
        && compilation.objective_policy_hash == request.semantics.objective_policy_hash
        && input.run_id != base_input.run_id
        && input.run_id == manifest.run_id
        && input.checksum == manifest.run_input_checksum
        && input.snapshot_id == request.semantics.snapshot_id
        && input.snapshot_document_hash == request.semantics.snapshot_document_hash
        && input.pack_id == base_input.pack_id
        && input.pack_schema_version == base_input.pack_schema_version
        && input.objective_context == base_input.objective_context
        && input.planning_ir_schema_version == base_input.planning_ir_schema_version
        && input.compiler_version == base_input.compiler_version
        && input.model_hash == compilation.derived_model_hash
        && input.objective_policy_hash == compilation.objective_policy_hash
        && input.temporary_condition_hash.as_ref() == Some(&compilation.condition_checksum)
}

#[allow(clippy::too_many_arguments)]
fn verified_alternative(
    request: &CounterfactualJobRequestV1,
    base_input: &RunInput,
    base_manifest: &RunManifest,
    base: &AcceptedResult,
    compilation: &CounterfactualCompilationBindingV1,
    derived_input: &RunInput,
    derived_manifest: &RunManifest,
    alternative: Option<&AcceptedResult>,
) -> CounterfactualInterpretation {
    let Some(alternative) = alternative else {
        return failed(CounterfactualFailureKind::InvalidCandidate);
    };
    if alternative.validate().is_err()
        || alternative.solution.scenario_id != base.solution.scenario_id
        || alternative.solution.scenario_revision != base.solution.scenario_revision
        || alternative.solution.pack_id != base.solution.pack_id
        || alternative.solution.projection_version != base.solution.projection_version
        || alternative.verification.document_hash != derived_input.snapshot_document_hash
        || alternative.verification.planning_model_hash != compilation.derived_model_hash
        || !counterfactual_condition_satisfied(&request.condition, &alternative.solution)
    {
        return failed(CounterfactualFailureKind::InvalidCandidate);
    }
    let RunTerminalOutcomeV1::Accepted {
        solution_id,
        accepted_result_checksum,
        verification_checksum,
        ..
    } = &derived_manifest.outcome
    else {
        return failed(CounterfactualFailureKind::InvalidBinding);
    };
    if solution_id != &alternative.solution.solution_id
        || accepted_result_checksum != &alternative.checksum
        || verification_checksum != &alternative.verification.checksum
    {
        return failed(CounterfactualFailureKind::InvalidCandidate);
    }

    let context = ComparisonContext {
        locks: &[],
        manifests: Some(ComparisonRunManifests {
            base: base_manifest,
            candidate: derived_manifest,
        }),
    };
    let Ok(comparison) = compare_accepted_results(base, alternative, Some(&context)) else {
        return failed(CounterfactualFailureKind::InvalidCandidate);
    };
    let ordering = comparison.ordering;
    completed(
        request,
        base_input,
        base_manifest,
        compilation,
        derived_input,
        derived_manifest,
        CounterfactualConclusionV1::VerifiedAlternative {
            alternative: AcceptedResultRefV1 {
                solution_id: alternative.solution.solution_id,
                result_checksum: alternative.checksum.clone(),
            },
            comparison: Box::new(comparison),
            ordering,
        },
    )
}

fn completed(
    request: &CounterfactualJobRequestV1,
    base_input: &RunInput,
    base_manifest: &RunManifest,
    compilation: &CounterfactualCompilationBindingV1,
    input: &RunInput,
    manifest: &RunManifest,
    conclusion: CounterfactualConclusionV1,
) -> CounterfactualInterpretation {
    match CounterfactualResult::new(
        request.clone(),
        base_input.clone(),
        base_manifest.clone(),
        compilation.clone(),
        input.clone(),
        manifest.clone(),
        conclusion,
    ) {
        Ok(result) => CounterfactualInterpretation::Completed(Box::new(result)),
        Err(_) => failed(CounterfactualFailureKind::InvalidBinding),
    }
}

const fn failed(kind: CounterfactualFailureKind) -> CounterfactualInterpretation {
    CounterfactualInterpretation::Failed(CounterfactualJobErrorV1 { kind })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_domain_ir::{
        AssignmentValue, COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION, ComparisonOrdering,
        CounterfactualConditionPayloadV1, CounterfactualRequestSemanticsV1, DomainAssignment,
        DomainAssignmentId, DomainEntityId, DomainEntityKindId, DomainEntityRef,
        NORMALIZED_SOLUTION_SCHEMA_VERSION, NormalizedSolution, OptimizationDirection,
        RunPhaseTimingsV1, ScoreLevelId, ScoreLevelValue, ScoreVector, VerificationContextV1,
        VerificationReport, blake3_hex,
    };
    use eutheto_planning_ir::{
        BoolVariable, BoolVariableId, Capability, CompilerId, Constraint, ConstraintRecord,
        ConstraintTag, ObjectiveLevel, ObjectivePlan, PLANNING_IR_SCHEMA_VERSION,
        PlanningConstraintId, PlanningMetadata, ProjectionExpression, ProjectionId, ProvenanceId,
        SolutionProjection, Variable,
    };
    use eutheto_types::{
        BackendId, BackendSelection, CounterfactualJobId, DurationMillis, ExplanationMode,
        IanaTimeZone, PackId, PreservationPolicy, ReproducibilityMode, RequestId, ResourceLimits,
        Rfc3339Timestamp, ScenarioId, ScenarioSnapshotId, SolutionId, SolveMode, SolveOptions,
        SolveRunId, WorkerThreadPolicy,
    };
    use std::collections::{BTreeMap, BTreeSet};

    fn hash(label: &str) -> String {
        blake3_hex(label.as_bytes())
    }

    fn base_problem() -> Result<PlanningProblem, Box<dyn std::error::Error>> {
        let variable = BoolVariableId::new("tests.value")?;
        let provenance = ProvenanceId::new("tests.base")?;
        Ok(PlanningProblem {
            schema_version: PLANNING_IR_SCHEMA_VERSION,
            variables: vec![Variable::Boolean(BoolVariable {
                id: variable.clone(),
                provenance: provenance.clone(),
            })],
            constraints: vec![ConstraintRecord {
                id: PlanningConstraintId::new("tests.baseline")?,
                body: Constraint::BoolAnd {
                    literals: vec![eutheto_planning_ir::Literal::positive(variable.clone())],
                },
                enforcement: Vec::new(),
                provenance: provenance.clone(),
                tags: Vec::new(),
            }],
            objectives: ObjectivePlan {
                levels: vec![ObjectiveLevel {
                    id: eutheto_planning_ir::ObjectiveLevelId::new("tests.primary")?,
                    direction: OptimizationDirection::Minimize,
                    lower_bound: 0,
                    upper_bound: 0,
                    terms: Vec::new(),
                    provenance: provenance.clone(),
                }],
            },
            assumptions: Vec::new(),
            projections: vec![SolutionProjection {
                id: ProjectionId::new("tests.value")?,
                assignment_id: DomainAssignmentId::new("tests.value")?,
                entity: DomainEntityRef {
                    kind: DomainEntityKindId::new("tests.person")?,
                    id: DomainEntityId::new("tests.person")?,
                },
                required: true,
                expression: ProjectionExpression::Boolean(variable),
                provenance: provenance.clone(),
            }],
            provenance: vec![ProvenanceRecord {
                id: provenance,
                source_kind: ProvenanceSourceKind::Projection,
                source_id: "tests.base".to_owned(),
                entity_refs: Vec::new(),
                message_key: "tests.base".to_owned(),
                parameters: BTreeMap::new(),
                parent: None,
            }],
            metadata: PlanningMetadata {
                pack_id: PackId::new("official.test")?,
                scenario_id: "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse::<ScenarioId>()?,
                scenario_revision: 7,
                projection_version: 1,
                compiler_id: CompilerId::new("official.test")?,
                compiler_version: "1.0.0".to_owned(),
                compile_metadata: BTreeMap::new(),
                display_text: BTreeMap::new(),
            },
            declared_capabilities: BTreeSet::from([
                Capability::BoolAnd,
                Capability::BooleanProjection,
            ]),
            split_authorization: None,
        })
    }

    fn condition() -> Result<CounterfactualConditionV1, Box<dyn std::error::Error>> {
        Ok(CounterfactualConditionV1::new(
            CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id: DomainAssignmentId::new("tests.value")?,
                value: AssignmentValue::Boolean(true),
            },
        )?)
    }

    fn derived_problem(
        base: &PlanningProblem,
        condition: &CounterfactualConditionV1,
    ) -> Result<PlanningProblem, Box<dyn std::error::Error>> {
        let mut derived = base.clone();
        let provenance = ProvenanceId::new("tests.condition")?;
        derived.constraints.push(ConstraintRecord {
            id: PlanningConstraintId::new("tests.condition")?,
            body: Constraint::BoolAnd {
                literals: vec![eutheto_planning_ir::Literal::positive(BoolVariableId::new(
                    "tests.value",
                )?)],
            },
            enforcement: Vec::new(),
            provenance: provenance.clone(),
            tags: Vec::new(),
        });
        derived.provenance.push(ProvenanceRecord {
            id: provenance,
            source_kind: ProvenanceSourceKind::Derived,
            source_id: "tests.condition".to_owned(),
            entity_refs: Vec::new(),
            message_key: "tests.condition".to_owned(),
            parameters: BTreeMap::new(),
            parent: Some(ProvenanceId::new("tests.base")?),
        });
        derived.metadata.compile_metadata.insert(
            MetadataKey::new(COUNTERFACTUAL_CONDITION_HASH_METADATA_KEY)?,
            ProvenanceParameter::Text(condition.checksum.clone()),
        );
        Ok(derived)
    }

    #[test]
    fn validates_only_additive_condition_compilation() -> Result<(), Box<dyn std::error::Error>> {
        let base = base_problem()?;
        let condition = condition()?;
        let derived = derived_problem(&base, &condition)?;
        let binding = validate_counterfactual_problem(
            &base,
            &derived,
            &condition,
            &hash("objective"),
            PlanningIrLimitsV1::DEFAULT,
        )?;
        assert_eq!(binding.condition_checksum, condition.checksum);
        assert_ne!(binding.base_model_hash, binding.derived_model_hash);

        let mut changed_objective = derived.clone();
        changed_objective.objectives.levels[0].direction = OptimizationDirection::Maximize;
        assert!(matches!(
            validate_counterfactual_problem(
                &base,
                &changed_objective,
                &condition,
                &hash("objective"),
                PlanningIrLimitsV1::DEFAULT,
            ),
            Err(CounterfactualValidationError::BaselineMutation(
                "objectives"
            ))
        ));
        let mut changed_projection = derived.clone();
        changed_projection.projections[0].required = false;
        assert!(matches!(
            validate_counterfactual_problem(
                &base,
                &changed_projection,
                &condition,
                &hash("objective"),
                PlanningIrLimitsV1::DEFAULT,
            ),
            Err(CounterfactualValidationError::BaselineMutation(
                "projections"
            ))
        ));
        let mut changed_constraint = derived;
        changed_constraint.constraints[0]
            .tags
            .push(ConstraintTag::new("tests.changed")?);
        assert!(matches!(
            validate_counterfactual_problem(
                &base,
                &changed_constraint,
                &condition,
                &hash("objective"),
                PlanningIrLimitsV1::DEFAULT,
            ),
            Err(CounterfactualValidationError::BaselineMutation(
                "constraints"
            ))
        ));
        Ok(())
    }

    fn options() -> Result<SolveOptions, Box<dyn std::error::Error>> {
        Ok(SolveOptions {
            backend: BackendSelection::Auto,
            mode: SolveMode::Balanced,
            time_limit_milliseconds: DurationMillis::new(5_000)?,
            memory_limit_bytes: None,
            worker_threads: WorkerThreadPolicy::Exact(1),
            random_seed: 1,
            solution_limit: Some(1),
            stop_after_first_feasible: true,
            collect_intermediate_solutions: false,
            explanation_mode: ExplanationMode::Standard,
            preserve_existing: PreservationPolicy::None,
            reproducibility: ReproducibilityMode::Deterministic,
            resource_limits: ResourceLimits {
                max_entities: 10,
                max_rules: 10,
                max_variables: 10,
                max_constraints: 10,
            },
        })
    }

    fn accepted(
        suffix: u8,
        score: i64,
        model_hash: &str,
    ) -> Result<AcceptedResult, Box<dyn std::error::Error>> {
        accepted_with_value(suffix, score, model_hash, true)
    }

    fn accepted_with_value(
        suffix: u8,
        score: i64,
        model_hash: &str,
        value: bool,
    ) -> Result<AcceptedResult, Box<dyn std::error::Error>> {
        let scenario_id = "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse::<ScenarioId>()?;
        let solution = NormalizedSolution {
            schema_version: NORMALIZED_SOLUTION_SCHEMA_VERSION,
            pack_id: PackId::new("official.test")?,
            scenario_id,
            scenario_revision: 7,
            projection_version: 1,
            solution_id: format!("01890a5d-ac96-7b64-9f74-bbfcf30f9f{suffix:02x}")
                .parse::<SolutionId>()?,
            assignments: vec![DomainAssignment {
                id: DomainAssignmentId::new("tests.value")?,
                entity: DomainEntityRef {
                    kind: DomainEntityKindId::new("tests.person")?,
                    id: DomainEntityId::new("tests.person")?,
                },
                value: AssignmentValue::Boolean(value),
                evidence: Vec::new(),
            }],
        };
        let context = VerificationContextV1::new(
            scenario_id,
            7,
            hash("snapshot"),
            model_hash.to_owned(),
            solution.canonical_hash()?,
            hash("scope"),
        )?;
        let report = VerificationReport::new(
            &context,
            Vec::new(),
            ScoreVector {
                feasibility: 0,
                levels: vec![ScoreLevelValue {
                    level_id: ScoreLevelId::new("tests.primary")?,
                    value: score,
                    direction: OptimizationDirection::Minimize,
                    category_breakdown: BTreeMap::new(),
                }],
            },
            Vec::new(),
            BTreeMap::new(),
        )?;
        Ok(AcceptedResult::new(solution, report)?)
    }
    #[test]
    fn condition_satisfaction_requires_target_and_exact_force_forbid_or_replace_semantics()
    -> Result<(), Box<dyn std::error::Error>> {
        let solution = accepted(0x11, 10, &hash("condition-model"))?.solution;
        let condition = |payload| CounterfactualConditionV1::new(payload);
        let target = DomainAssignmentId::new("tests.value")?;
        assert!(counterfactual_condition_satisfied(
            &condition(CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id: target.clone(),
                value: AssignmentValue::Boolean(true),
            })?,
            &solution,
        ));
        assert!(!counterfactual_condition_satisfied(
            &condition(CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id: target.clone(),
                value: AssignmentValue::Boolean(false),
            })?,
            &solution,
        ));
        assert!(!counterfactual_condition_satisfied(
            &condition(CounterfactualConditionPayloadV1::ForbidAssignmentValue {
                assignment_id: target,
                value: AssignmentValue::Boolean(true),
            })?,
            &solution,
        ));
        assert!(counterfactual_condition_satisfied(
            &condition(CounterfactualConditionPayloadV1::ForbidAssignmentValue {
                assignment_id: DomainAssignmentId::new("tests.value")?,
                value: AssignmentValue::Boolean(false),
            })?,
            &solution,
        ));
        assert!(!counterfactual_condition_satisfied(
            &condition(CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id: DomainAssignmentId::new("tests.missing")?,
                value: AssignmentValue::Boolean(true),
            })?,
            &solution,
        ));

        let target_id = DomainAssignmentId::new("tests.value")?;
        let replaced_id = DomainAssignmentId::new("tests.incumbent")?;
        let replace = || {
            condition(CounterfactualConditionPayloadV1::ReplaceAssignment {
                target_assignment_id: target_id.clone(),
                replaced_assignment_id: replaced_id.clone(),
            })
        };
        let solution_with_values =
            |target_value: Option<bool>,
             replaced_value: Option<bool>|
             -> Result<NormalizedSolution, Box<dyn std::error::Error>> {
                let mut solution = accepted_with_value(
                    0x11,
                    10,
                    &hash("condition-model"),
                    target_value.unwrap_or(false),
                )?
                .solution;
                let entity = solution.assignments[0].entity.clone();
                if target_value.is_none() {
                    solution.assignments.clear();
                }
                if let Some(value) = replaced_value {
                    solution.assignments.push(DomainAssignment {
                        id: replaced_id.clone(),
                        entity,
                        value: AssignmentValue::Boolean(value),
                        evidence: Vec::new(),
                    });
                }
                solution.canonicalize()?;
                Ok(solution)
            };

        assert!(!counterfactual_condition_satisfied(
            &replace()?,
            &solution_with_values(None, Some(false))?,
        ));
        assert!(!counterfactual_condition_satisfied(
            &replace()?,
            &solution_with_values(Some(true), None)?,
        ));
        assert!(!counterfactual_condition_satisfied(
            &replace()?,
            &solution_with_values(Some(false), Some(false))?,
        ));
        assert!(!counterfactual_condition_satisfied(
            &replace()?,
            &solution_with_values(Some(true), Some(true))?,
        ));
        assert!(!counterfactual_condition_satisfied(
            &replace()?,
            &solution_with_values(Some(false), Some(true))?,
        ));
        assert!(counterfactual_condition_satisfied(
            &replace()?,
            &solution_with_values(Some(true), Some(false))?,
        ));
        Ok(())
    }

    fn run_input(
        suffix: u8,
        model_hash: String,
        objective_hash: String,
        condition_hash: Option<String>,
    ) -> Result<RunInput, Box<dyn std::error::Error>> {
        run_input_at_revision(suffix, 7, model_hash, objective_hash, condition_hash)
    }

    fn run_input_at_revision(
        suffix: u8,
        revision: u64,
        model_hash: String,
        objective_hash: String,
        condition_hash: Option<String>,
    ) -> Result<RunInput, Box<dyn std::error::Error>> {
        Ok(RunInput::new(
            format!("01890a5d-ac96-7b64-9f74-bbfcf30f9f{suffix:02x}").parse::<SolveRunId>()?,
            format!("01890a5d-ac96-7b64-9f74-bbfcf30f9e{suffix:02x}").parse::<RequestId>()?,
            "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse::<ScenarioId>()?,
            revision,
            "01890a5d-ac96-7b64-9f74-bbfcf30f9f81".parse::<ScenarioSnapshotId>()?,
            hash("snapshot"),
            "2026-09-03T12:00:00Z".parse::<Rfc3339Timestamp>()?,
            PackId::new("official.test")?,
            1,
            PLANNING_IR_SCHEMA_VERSION,
            "1.0.0".to_owned(),
            "1.0.0".to_owned(),
            BackendId::new("test.backend")?,
            "1.0.0".to_owned(),
            "1.0.0".to_owned(),
            "1.0.0".to_owned(),
            "1.0.0".to_owned(),
            1,
            0,
            model_hash,
            objective_hash,
            options()?,
            "UTC".parse::<IanaTimeZone>()?,
            condition_hash,
            None,
        )?)
    }

    fn manifest(
        input: &RunInput,
        outcome: RunTerminalOutcomeV1,
    ) -> Result<RunManifest, Box<dyn std::error::Error>> {
        let accepted = matches!(&outcome, RunTerminalOutcomeV1::Accepted { .. });
        let interrupted = matches!(&outcome, RunTerminalOutcomeV1::Interrupted);
        Ok(RunManifest::new(
            input.run_id,
            input.checksum.clone(),
            outcome,
            "2026-09-03T12:00:00Z".parse::<Rfc3339Timestamp>()?,
            "2026-09-03T12:00:01Z".parse::<Rfc3339Timestamp>()?,
            (!interrupted).then_some(DurationMillis::new(1_000)?),
            accepted.then_some(DurationMillis::new(100)?),
            accepted.then_some(DurationMillis::new(200)?),
            RunPhaseTimingsV1::default(),
            Vec::new(),
        )?)
    }

    struct InterpretationFixture {
        request: CounterfactualJobRequestV1,
        base_input: RunInput,
        base_manifest: RunManifest,
        base: AcceptedResult,
        compilation: CounterfactualCompilationBindingV1,
        derived_input: RunInput,
    }

    fn interpretation_fixture() -> Result<InterpretationFixture, Box<dyn std::error::Error>> {
        let condition = condition()?;
        let objective_hash = hash("objective");
        let compilation = CounterfactualCompilationBindingV1::new(
            hash("base-model"),
            condition.checksum.clone(),
            hash("derived-model"),
            objective_hash.clone(),
        )?;
        let base = accepted(0x10, 10, &compilation.base_model_hash)?;
        let base_input = run_input(
            0x20,
            compilation.base_model_hash.clone(),
            objective_hash.clone(),
            None,
        )?;
        let base_manifest = manifest(
            &base_input,
            RunTerminalOutcomeV1::Accepted {
                status: SolveStatus::Feasible,
                solution_id: base.solution.solution_id,
                accepted_result_checksum: base.checksum.clone(),
                verification_checksum: base.verification.checksum.clone(),
            },
        )?;
        let derived_input = run_input(
            0x21,
            compilation.derived_model_hash.clone(),
            objective_hash.clone(),
            Some(condition.checksum.clone()),
        )?;
        let semantics = CounterfactualRequestSemanticsV1 {
            schema_version: COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION,
            scenario_id: base.solution.scenario_id,
            scenario_revision: 7,
            snapshot_id: base_input.snapshot_id,
            snapshot_document_hash: base_input.snapshot_document_hash.clone(),
            base: AcceptedResultRefV1 {
                solution_id: base.solution.solution_id,
                result_checksum: base.checksum.clone(),
            },
            base_run_id: base_input.run_id,
            base_run_input_checksum: base_input.checksum.clone(),
            base_model_hash: compilation.base_model_hash.clone(),
            objective_policy_hash: objective_hash,
            condition_checksum: condition.checksum.clone(),
            total_budget_milliseconds: DurationMillis::new(5_000)?,
        };
        let request = CounterfactualJobRequestV1::new(
            "01890a5d-ac96-7b64-9f74-bbfcf30f9f30".parse::<CounterfactualJobId>()?,
            "01890a5d-ac96-7b64-9f74-bbfcf30f9f31".parse::<RequestId>()?,
            semantics,
            condition,
            "2026-09-03T12:00:00Z".parse::<Rfc3339Timestamp>()?,
        )?;
        Ok(InterpretationFixture {
            request,
            base_input,
            base_manifest,
            base,
            compilation,
            derived_input,
        })
    }

    fn interpret_with_status(
        fixture: &InterpretationFixture,
        status: SolveStatus,
    ) -> Result<CounterfactualInterpretation, Box<dyn std::error::Error>> {
        let manifest = manifest(
            &fixture.derived_input,
            RunTerminalOutcomeV1::NoResult { status },
        )?;
        Ok(interpret_counterfactual(
            &fixture.request,
            &fixture.base_input,
            &fixture.base_manifest,
            &fixture.base,
            &fixture.compilation,
            &fixture.derived_input,
            &manifest,
            None,
            None,
        ))
    }

    #[test]
    fn backend_infeasibility_claim_stays_inconclusive() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = interpretation_fixture()?;
        assert!(matches!(
            interpret_with_status(&fixture, SolveStatus::Infeasible)?,
            CounterfactualInterpretation::Completed(result)
                if matches!(
                    result.conclusion,
                    CounterfactualConclusionV1::NotDistinguishedWithinBudget
                )
        ));
        Ok(())
    }

    #[test]
    fn maps_counterfactual_limits_and_failures() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = interpretation_fixture()?;
        assert!(matches!(
            interpret_with_status(&fixture, SolveStatus::NoSolutionWithinLimit)?,
            CounterfactualInterpretation::Completed(result)
                if matches!(
                    result.conclusion,
                    CounterfactualConclusionV1::NotDistinguishedWithinBudget
                )
        ));
        assert_eq!(
            interpret_with_status(&fixture, SolveStatus::Cancelled)?,
            CounterfactualInterpretation::Cancelled
        );
        for (status, kind) in [
            (
                SolveStatus::BackendUnavailable,
                CounterfactualFailureKind::BackendUnavailable,
            ),
            (
                SolveStatus::BackendFailed,
                CounterfactualFailureKind::BackendFailed,
            ),
            (
                SolveStatus::InvalidModel,
                CounterfactualFailureKind::InvalidModel,
            ),
            (
                SolveStatus::Unbounded,
                CounterfactualFailureKind::InvalidModel,
            ),
        ] {
            assert_eq!(
                interpret_with_status(&fixture, status)?,
                CounterfactualInterpretation::Failed(CounterfactualJobErrorV1 { kind })
            );
        }
        let interrupted = manifest(&fixture.derived_input, RunTerminalOutcomeV1::Interrupted)?;
        assert_eq!(
            interpret_counterfactual(
                &fixture.request,
                &fixture.base_input,
                &fixture.base_manifest,
                &fixture.base,
                &fixture.compilation,
                &fixture.derived_input,
                &interrupted,
                None,
                None,
            ),
            CounterfactualInterpretation::Interrupted
        );
        Ok(())
    }

    #[test]
    fn verifies_better_equivalent_and_worse_alternatives() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = interpretation_fixture()?;
        for (suffix, score, expected) in [
            (0x40, 5, ComparisonOrdering::Better),
            (0x41, 10, ComparisonOrdering::Equivalent),
            (0x42, 15, ComparisonOrdering::Worse),
        ] {
            let alternative = accepted(suffix, score, &fixture.compilation.derived_model_hash)?;
            let derived_manifest = manifest(
                &fixture.derived_input,
                RunTerminalOutcomeV1::Accepted {
                    status: SolveStatus::Feasible,
                    solution_id: alternative.solution.solution_id,
                    accepted_result_checksum: alternative.checksum.clone(),
                    verification_checksum: alternative.verification.checksum.clone(),
                },
            )?;
            let interpreted = interpret_counterfactual(
                &fixture.request,
                &fixture.base_input,
                &fixture.base_manifest,
                &fixture.base,
                &fixture.compilation,
                &fixture.derived_input,
                &derived_manifest,
                Some(&alternative),
                None,
            );
            assert!(matches!(
                interpreted,
                CounterfactualInterpretation::Completed(result)
                    if matches!(
                        result.conclusion,
                        CounterfactualConclusionV1::VerifiedAlternative {
                            ordering,
                            ..
                        } if ordering == expected
                    )
            ));
        }
        Ok(())
    }

    #[test]
    fn rejects_stale_and_invalid_candidates_with_typed_failures()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = interpretation_fixture()?;
        let stale_input = run_input_at_revision(
            0x22,
            8,
            fixture.compilation.derived_model_hash.clone(),
            fixture.compilation.objective_policy_hash.clone(),
            Some(fixture.compilation.condition_checksum.clone()),
        )?;
        let stale_manifest = manifest(
            &stale_input,
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::Infeasible,
            },
        )?;
        assert_eq!(
            interpret_counterfactual(
                &fixture.request,
                &fixture.base_input,
                &fixture.base_manifest,
                &fixture.base,
                &fixture.compilation,
                &stale_input,
                &stale_manifest,
                None,
                None,
            ),
            CounterfactualInterpretation::Failed(CounterfactualJobErrorV1 {
                kind: CounterfactualFailureKind::StaleRevision,
            })
        );

        let derived_manifest = manifest(
            &fixture.derived_input,
            RunTerminalOutcomeV1::VerificationAlarm {
                diagnostic_code: "verification.invalid-candidate".to_owned(),
            },
        )?;
        assert_eq!(
            interpret_counterfactual(
                &fixture.request,
                &fixture.base_input,
                &fixture.base_manifest,
                &fixture.base,
                &fixture.compilation,
                &fixture.derived_input,
                &derived_manifest,
                None,
                None,
            ),
            CounterfactualInterpretation::Failed(CounterfactualJobErrorV1 {
                kind: CounterfactualFailureKind::InvalidCandidate,
            })
        );

        let alternative =
            accepted_with_value(0x43, 5, &fixture.compilation.derived_model_hash, false)?;
        let mismatch_manifest = manifest(
            &fixture.derived_input,
            RunTerminalOutcomeV1::Accepted {
                status: SolveStatus::Feasible,
                solution_id: alternative.solution.solution_id,
                accepted_result_checksum: alternative.checksum.clone(),
                verification_checksum: alternative.verification.checksum.clone(),
            },
        )?;
        assert_eq!(
            interpret_counterfactual(
                &fixture.request,
                &fixture.base_input,
                &fixture.base_manifest,
                &fixture.base,
                &fixture.compilation,
                &fixture.derived_input,
                &mismatch_manifest,
                Some(&alternative),
                None,
            ),
            CounterfactualInterpretation::Failed(CounterfactualJobErrorV1 {
                kind: CounterfactualFailureKind::InvalidCandidate,
            })
        );
        Ok(())
    }
}
