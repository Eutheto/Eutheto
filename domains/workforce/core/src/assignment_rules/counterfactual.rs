use super::{
    budget::{OperationBudget, effective_limits},
    compiler_rank::RankIds,
    identity::{ASSIGNMENT_KIND, PlanningIdentities},
    ir_cost::{precharge_canonicalization, precharge_validation},
    operation_error,
};
use crate::{ids::ShiftId, model::AssignmentPair};
use eutheto_domain_api::{CounterfactualCompileContext, DomainPackError};
use eutheto_domain_ir::{
    AssignmentValue, CounterfactualConditionPayloadV1, CounterfactualConditionV1, DomainEntityRef,
};
use eutheto_planning_ir::{
    ComparisonOp, Constraint, ConstraintRecord, LinearComparison, LinearExpression, LinearTerm,
    MetadataKey, PlanningConstraintId, PlanningProblem, ProjectionExpression, ProvenanceId,
    ProvenanceParameter, ProvenanceRecord, ProvenanceSourceKind, canonical_ir_hash, validate,
};
use eutheto_types::{PersonId, ScenarioDocument};
use std::collections::BTreeMap;

fn invalid(path: &str, message: &str) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

fn check(context: &CounterfactualCompileContext<'_>) -> Result<(), DomainPackError> {
    context.compile_context.control.check()?;
    if context.budget.is_cancelled() {
        Err(DomainPackError::Cancelled)
    } else if context.budget.is_expired() {
        Err(DomainPackError::BudgetExpired)
    } else {
        Ok(())
    }
}

fn rank_id(
    base: &PlanningProblem,
    assignment_id: &eutheto_domain_ir::DomainAssignmentId,
    budget: &mut OperationBudget<'_>,
    identities: &mut PlanningIdentities,
) -> Result<(ShiftId, eutheto_planning_ir::IntVariableId, DomainEntityRef), DomainPackError> {
    budget
        .steps(base.projections.len() as u64)
        .map_err(|error| operation_error(&error))?;
    let projection = base
        .projections
        .iter()
        .find(|entry| &entry.assignment_id == assignment_id)
        .ok_or_else(|| {
            invalid(
                "/condition",
                "The assignment is not a supported candidate in this schedule.",
            )
        })?;
    if projection.entity.kind.as_str() != ASSIGNMENT_KIND {
        return Err(invalid(
            "/condition",
            "The assignment is not a Boolean Workforce candidate.",
        ));
    }
    let ProjectionExpression::Boolean(decision_id) = &projection.expression else {
        return Err(invalid(
            "/condition",
            "The assignment is not a Boolean Workforce candidate.",
        ));
    };
    let pair_text = projection.entity.id.as_str();
    let (person, shift) = pair_text
        .split_once('.')
        .ok_or_else(|| invalid("/condition", "Invalid Workforce assignment identity."))?;
    let pair = AssignmentPair {
        person_id: person
            .parse::<PersonId>()
            .map_err(|_| invalid("/condition", "Invalid Workforce person identity."))?,
        shift_id: shift
            .parse::<ShiftId>()
            .map_err(|_| invalid("/condition", "Invalid Workforce shift identity."))?,
    };
    if assignment_id
        .as_str()
        .strip_prefix(ASSIGNMENT_KIND)
        .and_then(|suffix| suffix.strip_prefix('.'))
        != Some(pair_text)
    {
        return Err(invalid(
            "/condition",
            "The assignment does not match its source pair.",
        ));
    }
    let ids = RankIds::derive(pair, identities, budget).map_err(|error| operation_error(&error))?;
    // Ordinary Workforce compilation binds this 0/1 rank to the selected-pair Boolean.
    // Requiring both original channels prevents a new equality from constraining a detached rank.
    for (id, selected) in ids.channels.iter().zip([true, false]) {
        let record = base
            .constraints
            .binary_search_by(|record| record.id.cmp(id))
            .ok()
            .and_then(|index| base.constraints.get(index))
            .ok_or_else(|| {
                invalid(
                    "/context/baseProblem",
                    "Missing original Workforce rank channel.",
                )
            })?;
        if record.enforcement.len() != 1
            || record.enforcement[0].variable != *decision_id
            || record.enforcement[0].positive != selected
            || !matches!(&record.body, Constraint::LinearComparison(comparison)
                if comparison.op == ComparisonOp::Equal
                && comparison.rhs == i64::from(selected)
                && comparison.expression.constant == 0
                && comparison.expression.terms.len() == 1
                && comparison.expression.terms[0].variable == ids.integer
                && comparison.expression.terms[0].coefficient == 1)
        {
            return Err(invalid(
                "/context/baseProblem",
                "The Workforce rank channel does not match its assignment.",
            ));
        }
    }
    Ok((pair.shift_id, ids.integer, projection.entity.clone()))
}

/// Pins one original Boolean decision, or replaces the incumbent of the same shift.
/// Other assignments stay free to change under the original rules and score policy.
///
/// # Errors
/// Rejects unsupported values, missing or cross-shift pairs, altered base compilation,
/// model limits, or cancellation/deadline expiry; no derived model is returned on failure.
pub(crate) fn compile_counterfactual(
    document: &ScenarioDocument,
    condition: &CounterfactualConditionV1,
    context: &CounterfactualCompileContext<'_>,
) -> Result<PlanningProblem, DomainPackError> {
    check(context)?;
    super::authority::check_objective_context(
        context.compile_context.objective_context.as_ref(),
        document.scenario_id,
    )?;
    condition
        .validate()
        .map_err(|_| invalid("/condition", "The diagnostic condition is invalid."))?;
    let (target_assignment_id, target_selected, replaced_assignment_id) = match &condition.condition
    {
        CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(selected),
        } => (assignment_id, *selected, None),
        CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(selected),
        } => (assignment_id, !*selected, None),
        CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id,
            replaced_assignment_id,
        } => (target_assignment_id, true, Some(replaced_assignment_id)),
        _ => {
            return Err(DomainPackError::UnsupportedExplanationCapability(
                eutheto_domain_ir::ExplanationCapability::Counterfactual,
            ));
        }
    };
    let limits = effective_limits(context.compile_context.planning_limits)
        .map_err(|error| operation_error(&error))?;
    let mut budget = OperationBudget::analysis(Some(&context.compile_context.control), limits);
    precharge_validation(context.base_problem, &mut budget)
        .map_err(|error| operation_error(&error))?;
    validate(context.base_problem, limits).map_err(|_| {
        invalid(
            "/context/baseProblem",
            "The original planning model is invalid.",
        )
    })?;
    let mut identities = PlanningIdentities::default();
    let (target_shift, target_rank, target_entity) = rank_id(
        context.base_problem,
        target_assignment_id,
        &mut budget,
        &mut identities,
    )?;
    let replaced_pin = if let Some(assignment_id) = replaced_assignment_id {
        let (shift, rank, entity) = rank_id(
            context.base_problem,
            assignment_id,
            &mut budget,
            &mut identities,
        )?;
        if target_shift != shift {
            return Err(invalid(
                "/condition",
                "A replacement must concern the same shift.",
            ));
        }
        Some((rank, entity))
    } else {
        None
    };
    let recompiled = recompile_original_model(document, context, &mut budget)?;
    let added_constraints = if replaced_pin.is_some() { 2 } else { 1 };
    if (context.base_problem.constraints.len() as u64)
        .checked_add(added_constraints)
        .is_none_or(|count| count > limits.max_constraints)
        || (context.base_problem.provenance.len() as u64) >= limits.max_provenance_records
    {
        return Err(DomainPackError::ResourceLimitExceeded);
    }
    let mut derived = recompiled;
    // These non-hashed fields still belong to the exact baseline passed by the host.
    derived
        .metadata
        .display_text
        .clone_from(&context.base_problem.metadata.display_text);
    derived
        .split_authorization
        .clone_from(&context.base_problem.split_authorization);
    add_condition_pins(
        &mut derived,
        condition,
        target_rank,
        target_selected,
        target_entity,
        replaced_pin,
    )?;
    precharge_validation(&derived, &mut budget).map_err(|error| operation_error(&error))?;
    validate(&derived, limits)
        .map_err(|_| invalid("/condition", "The diagnostic model is invalid."))?;
    check(context)?;
    Ok(derived)
}

fn recompile_original_model(
    document: &ScenarioDocument,
    context: &CounterfactualCompileContext<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<PlanningProblem, DomainPackError> {
    check(context)?;
    let limits = effective_limits(context.compile_context.planning_limits)
        .map_err(|error| operation_error(&error))?;
    let recompiled = super::compile_workforce(document, context.compile_context)
        .map_err(|error| operation_error(&error))?
        .problem;
    check(context)?;
    for original in [&recompiled, context.base_problem] {
        // canonical_ir_hash validates twice, normalizes a copy, and serializes it.
        precharge_validation(original, budget).map_err(|error| operation_error(&error))?;
        precharge_validation(original, budget).map_err(|error| operation_error(&error))?;
        precharge_canonicalization(original, budget).map_err(|error| operation_error(&error))?;
        let bytes = budget
            .measure_ir(original)
            .map_err(|error| operation_error(&error))?;
        budget
            .reserve(
                0,
                0,
                bytes
                    .checked_mul(2)
                    .ok_or(DomainPackError::ResourceLimitExceeded)?,
            )
            .map_err(|error| operation_error(&error))?;
    }
    check(context)?;
    let recompiled_hash = canonical_ir_hash(&recompiled, limits)
        .map_err(|_| invalid("/context/baseProblem", "Invalid recompiled model."))?;
    check(context)?;
    let original_hash = canonical_ir_hash(context.base_problem, limits)
        .map_err(|_| invalid("/context/baseProblem", "Invalid original model."))?;
    check(context)?;
    if recompiled_hash != original_hash {
        return Err(invalid(
            "/context/baseProblem",
            "The original model differs from deterministic recompilation.",
        ));
    }
    Ok(recompiled)
}

fn add_condition_pins(
    derived: &mut PlanningProblem,
    condition: &CounterfactualConditionV1,
    target_rank: eutheto_planning_ir::IntVariableId,
    target_selected: bool,
    target_entity: DomainEntityRef,
    replaced: Option<(eutheto_planning_ir::IntVariableId, DomainEntityRef)>,
) -> Result<(), DomainPackError> {
    let provenance_id = ProvenanceId::new(format!(
        "official.workforce.counterfactual.provenance.{}",
        condition.checksum
    ))
    .map_err(|_| invalid("/condition", "Invalid diagnostic provenance identity."))?;
    let metadata_key = MetadataKey::new("eutheto.counterfactual.condition_hash")
        .map_err(|_| invalid("/condition", "Invalid diagnostic metadata identity."))?;
    if derived
        .metadata
        .compile_metadata
        .insert(
            metadata_key,
            ProvenanceParameter::Text(condition.checksum.clone()),
        )
        .is_some()
    {
        return Err(invalid(
            "/context/baseProblem",
            "The original model already contains diagnostic metadata.",
        ));
    }
    let mut entity_refs = Vec::with_capacity(if replaced.is_some() { 2 } else { 1 });
    entity_refs.push(target_entity);
    let replaced_rank = replaced.map(|(rank, entity)| {
        entity_refs.push(entity);
        rank
    });
    let source_id = match condition.condition {
        CounterfactualConditionPayloadV1::ForceAssignmentValue { .. } => {
            "official.workforce.counterfactual.force"
        }
        CounterfactualConditionPayloadV1::ForbidAssignmentValue { .. } => {
            "official.workforce.counterfactual.forbid"
        }
        CounterfactualConditionPayloadV1::ReplaceAssignment { .. } => {
            "official.workforce.counterfactual.replace"
        }
    };
    entity_refs.sort();
    entity_refs.dedup();
    let Err(provenance_index) = derived
        .provenance
        .binary_search_by(|record| record.id.cmp(&provenance_id))
    else {
        return Err(invalid(
            "/context/baseProblem",
            "Diagnostic provenance identity already exists.",
        ));
    };
    derived.provenance.insert(
        provenance_index,
        ProvenanceRecord {
            id: provenance_id.clone(),
            source_kind: ProvenanceSourceKind::Derived,
            source_id: source_id.to_owned(),
            entity_refs,
            message_key: source_id.to_owned(),
            parameters: BTreeMap::new(),
            parent: None,
        },
    );
    let pins = [
        Some(("target", target_rank, i64::from(target_selected))),
        replaced_rank.map(|rank| ("replaced", rank, 0)),
    ];
    for (label, rank, selected) in pins.into_iter().flatten() {
        insert_condition_pin(
            derived,
            &condition.checksum,
            label,
            rank,
            selected,
            &provenance_id,
        )?;
    }
    Ok(())
}

fn insert_condition_pin(
    derived: &mut PlanningProblem,
    condition_checksum: &str,
    label: &str,
    rank: eutheto_planning_ir::IntVariableId,
    selected: i64,
    provenance: &ProvenanceId,
) -> Result<(), DomainPackError> {
    let id = PlanningConstraintId::new(format!(
        "official.workforce.counterfactual.constraint.{label}.{condition_checksum}"
    ))
    .map_err(|_| invalid("/condition", "Invalid diagnostic constraint identity."))?;
    let Err(constraint_index) = derived
        .constraints
        .binary_search_by(|record| record.id.cmp(&id))
    else {
        return Err(invalid(
            "/context/baseProblem",
            "Diagnostic constraint identity already exists.",
        ));
    };
    derived.constraints.insert(
        constraint_index,
        ConstraintRecord {
            id,
            body: Constraint::LinearComparison(LinearComparison {
                expression: LinearExpression {
                    constant: 0,
                    terms: vec![LinearTerm {
                        variable: rank,
                        coefficient: 1,
                    }],
                },
                op: ComparisonOp::Equal,
                rhs: selected,
            }),
            enforcement: Vec::new(),
            provenance: provenance.clone(),
            tags: Vec::new(),
        },
    );
    Ok(())
}
