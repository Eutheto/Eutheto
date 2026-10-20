use super::{
    AssignmentAnalysis, AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleLimit,
    AssignmentVariable,
    analysis::{Plan, Predicate, consecutive, hours, prepare_infeasibility},
    authority::{check_objective_context, workforce_diagnostic_verification_scope},
    budget::{OperationBudget, add, count, effective_limits, within},
    compiler::{
        DecisionVariables, compile_constraint, materialize_hard_lock_parents,
        materialize_rule_parents, preflight, variable_fact,
    },
    compiler_rank::{emit_projection, projection_id},
    compiler_weekend, decode_workforce_assignment_id,
    identity::{ASSIGNMENT_KIND, IdentityKind, PlanningIdentities, WORKFORCE_PROJECTION_VERSION},
    input::AssignmentInput,
    ir_cost::{precharge_canonicalization, precharge_validation},
};
use crate::{generated_workforce_pack_contract::WORKFORCE_PACK_ID, model::AssignmentPair};
use eutheto_domain_api::{DomainPackError, DomainValidationReport, InfeasibilityCompileContext};
use eutheto_domain_ir::{
    AssignmentValue, AssumptionGroupId, CounterfactualConditionPayloadV1,
    CounterfactualConditionV1, DomainAssignmentId,
};
use eutheto_planning_ir::{
    Assumption, BoolVariable, BoolVariableId, Capability, CompilerId, Constraint, ConstraintRecord,
    Literal, MetadataKey, ObjectivePlan, PlanningConstraintId, PlanningIrLimitsV1,
    PlanningMetadata, PlanningProblem, ProjectionExpression, ProvenanceId, ProvenanceParameter,
    ProvenanceRecord, ProvenanceSourceKind, Variable, validate,
};
use eutheto_types::{OperationControl, PackId, RuleId, ScenarioDocument};
use std::collections::{BTreeMap, BTreeSet};

const DIAGNOSTIC_COMPILER_ID: &str = "official.workforce.infeasibility.compiler";
const CONDITION_METADATA_KEY: &str = "eutheto.counterfactual.condition_hash";

fn map_operation(error: &AssignmentRuleError) -> DomainPackError {
    super::authority::operation_error(error)
}

// Keep canonical graph lowering, ownership guards, stable ordering and validation on one budgeted path.
/// Compile the original Workforce Required semantics without objectives or rank artifacts.
///
/// The source authority supplies the independent set of genuine authored Required rules. Their
/// positive assumptions are the only relaxable controls; facts, hard locks and exact condition
/// pins remain unconditional.
///
/// # Errors
/// Returns an invalid-payload, interruption, finite-resource or canonical-model failure.
#[allow(clippy::too_many_lines)]
pub(crate) fn compile_infeasibility(
    document: &ScenarioDocument,
    context: &InfeasibilityCompileContext<'_>,
) -> Result<PlanningProblem, DomainPackError> {
    context.compile_context.control.check()?;
    let limits = effective_limits(context.compile_context.planning_limits)
        .map_err(|error| map_operation(&error))?;
    let control = OperationControl::Solve(context.budget.clone());
    let mut budget = OperationBudget::analysis(Some(&control), limits);
    check_context(document, context, &mut budget, limits)?;

    let classified = workforce_diagnostic_verification_scope(
        document,
        context.compile_context.scenario_revision,
        &control,
    )?;
    let relaxable = classified
        .relaxable_required_rules()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let input = AssignmentInput::new(document, &mut budget)
        .map_err(|error| map_operation(&error))?
        .with_objective_context(
            context.compile_context.objective_context.as_ref(),
            document.scenario_id,
            &mut budget,
        )?;
    super::profile::snapshot_pairs(
        &input,
        document.scenario_id,
        context.compile_context.objective_context.as_ref(),
        &mut budget,
    )?;
    let (analysis, mut plan) =
        prepare_infeasibility(document, &input, &mut budget, limits, &relaxable)
            .map_err(|error| map_operation(&error))?;
    reject_invalid_fixed_targets(&analysis.validation)?;

    // Every active genuine Required rule receives a parent even when its original population is
    // empty. This is source ownership, not a compiler-derived role guess.
    for &rule in classified.relaxable_required_rules() {
        budget.step().map_err(|error| map_operation(&error))?;
        plan.parents
            .entry(rule)
            .or_insert("diagnostic_required_rule");
    }
    let condition_pins = resolve_condition(
        context.base_problem,
        context.condition,
        &analysis.candidates,
        &input,
        &mut budget,
    )?;
    preflight_diagnostic(
        &analysis,
        &plan,
        &relaxable,
        classified.relaxable_required_rules().len(),
        condition_pins.len(),
        limits,
        &mut budget,
    )?;

    let mut identities = PlanningIdentities::default();
    let pairs = &analysis.candidates;
    let mut assignment_variables = Vec::with_capacity(pairs.len());
    let mut variables = Vec::with_capacity(pairs.len());
    let mut projections = Vec::with_capacity(pairs.len());
    let mut provenance = Vec::new();
    for pair in pairs {
        budget.step().map_err(|error| map_operation(&error))?;
        let key = ("assignment", pair.person_id, pair.shift_id);
        let decision = BoolVariable {
            id: BoolVariableId::new(
                identities
                    .derive(IdentityKind::Boolean, &key, &mut budget)
                    .map_err(|error| map_operation(&error))?,
            )
            .map_err(|_| invalid("/document", "Invalid Workforce Boolean identity."))?,
            provenance: ProvenanceId::new(
                identities
                    .derive(IdentityKind::Provenance, &key, &mut budget)
                    .map_err(|error| map_operation(&error))?,
            )
            .map_err(|_| invalid("/document", "Invalid Workforce provenance identity."))?,
        };
        provenance.push(
            variable_fact(*pair, decision.provenance.clone(), &mut budget)
                .map_err(|error| map_operation(&error))?,
        );
        projections.push(
            emit_projection(
                *pair,
                &decision,
                projection_id(*pair, &mut identities, &mut budget)
                    .map_err(|error| map_operation(&error))?,
                &mut budget,
            )
            .map_err(|error| map_operation(&error))?,
        );
        variables.push(Variable::Boolean(decision.clone()));
        assignment_variables.push(AssignmentVariable {
            pair: *pair,
            variable: decision,
        });
    }

    let parents = materialize_rule_parents(&plan, &mut identities, &mut provenance, &mut budget)
        .map_err(|error| map_operation(&error))?;
    let hard_lock_parents =
        materialize_hard_lock_parents(&plan, &mut identities, &mut provenance, &mut budget)
            .map_err(|error| map_operation(&error))?;
    let mut constraints = Vec::new();
    let mut auxiliary_variables = Vec::new();
    for planned in &plan.constraints {
        let parent = match planned.predicate {
            Predicate::HardLock { assignment, .. } => hard_lock_parents
                .get(&assignment)
                .ok_or_else(|| invalid("/document", "Missing Workforce lock provenance."))?
                .clone(),
            _ => parents
                .get(&planned.rule)
                .ok_or_else(|| invalid("/document", "Missing Workforce rule provenance."))?
                .clone(),
        };
        if compiler_weekend::is(planned) {
            compiler_weekend::emit(
                planned,
                &plan,
                DecisionVariables::Contribution(&assignment_variables),
                parent,
                &mut identities,
                &mut auxiliary_variables,
                &mut constraints,
                &mut provenance,
                &mut budget,
                limits,
            )
            .map_err(|error| map_operation(&error))?;
        } else if consecutive::is(planned) {
            consecutive::emit(
                planned,
                &plan,
                DecisionVariables::Contribution(&assignment_variables),
                parent,
                &mut identities,
                &mut auxiliary_variables,
                &mut constraints,
                &mut provenance,
                &mut budget,
                limits,
            )
            .map_err(|error| map_operation(&error))?;
        } else {
            let (record, fact) = compile_constraint(
                planned,
                &plan,
                DecisionVariables::Contribution(&assignment_variables),
                parent,
                &mut identities,
                &mut budget,
            )
            .map_err(|error| map_operation(&error))?;
            constraints.push(record);
            provenance.push(fact);
        }
    }
    let hour_auxiliaries = hours::prepare_contribution(
        &plan,
        pairs,
        &assignment_variables,
        &mut identities,
        &mut budget,
    )
    .map_err(|error| map_operation(&error))?;
    auxiliary_variables.extend(hour_auxiliaries.variables);
    constraints.extend(hour_auxiliaries.constraints);
    provenance.extend(hour_auxiliaries.provenance);
    hours::compile(
        &plan,
        &hour_auxiliaries.pair_ints,
        &parents,
        &mut identities,
        &mut auxiliary_variables,
        &mut constraints,
        &mut provenance,
        &mut budget,
        limits,
    )
    .map_err(|error| map_operation(&error))?;

    variables.extend(auxiliary_variables);
    let guards = add_assumptions(
        classified.relaxable_required_rules(),
        &parents,
        &mut identities,
        &mut variables,
        &mut provenance,
        &mut budget,
    )
    .map_err(|error| map_operation(&error))?;
    add_authored_rejection_constraints(
        &analysis,
        &relaxable,
        &assignment_variables,
        &guards,
        &parents,
        &mut identities,
        &mut constraints,
        &mut provenance,
        &mut budget,
    )
    .map_err(|error| map_operation(&error))?;
    add_condition_constraints(
        &condition_pins,
        context.condition,
        &assignment_variables,
        &mut identities,
        &mut constraints,
        &mut provenance,
        &mut budget,
    )
    .map_err(|error| map_operation(&error))?;
    append_rule_guards(
        &mut constraints,
        &provenance,
        &guards,
        &parents,
        limits,
        &mut budget,
    )
    .map_err(|error| map_operation(&error))?;

    let metadata = diagnostic_metadata(document, context, &mut budget)?;
    let mut assumptions = build_assumptions(
        classified.relaxable_required_rules(),
        &guards,
        &parents,
        &mut budget,
    )
    .map_err(|error| map_operation(&error))?;
    variables.sort_unstable_by(|left, right| left.canonical_id().cmp(right.canonical_id()));
    constraints.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    provenance.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    projections.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    assumptions.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    budget
        .sort_work(variables.len())
        .map_err(|error| map_operation(&error))?;
    budget
        .sort_work(constraints.len())
        .map_err(|error| map_operation(&error))?;
    budget
        .sort_work(provenance.len())
        .map_err(|error| map_operation(&error))?;
    budget
        .sort_work(projections.len())
        .map_err(|error| map_operation(&error))?;
    budget
        .sort_work(assumptions.len())
        .map_err(|error| map_operation(&error))?;

    let mut problem = PlanningProblem {
        schema_version: eutheto_planning_ir::PLANNING_IR_SCHEMA_VERSION,
        variables,
        constraints,
        objectives: ObjectivePlan::default(),
        assumptions,
        projections,
        provenance,
        metadata,
        declared_capabilities: diagnostic_capabilities(),
        split_authorization: None,
    };
    precharge_canonicalization(&problem, &mut budget).map_err(|error| map_operation(&error))?;
    budget.check().map_err(|error| map_operation(&error))?;
    problem
        .canonicalize()
        .map_err(|_| invalid("/document", "Invalid diagnostic planning model."))?;
    precharge_validation(&problem, &mut budget).map_err(|error| map_operation(&error))?;
    validate(&problem, limits)
        .map_err(|_| invalid("/document", "Invalid diagnostic planning model."))?;
    budget.check().map_err(|error| map_operation(&error))?;
    Ok(problem)
}

fn check_context(
    document: &ScenarioDocument,
    context: &InfeasibilityCompileContext<'_>,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), DomainPackError> {
    budget.check().map_err(|error| map_operation(&error))?;
    check_objective_context(
        context.compile_context.objective_context.as_ref(),
        document.scenario_id,
    )?;
    if context.base_problem.metadata.scenario_id != document.scenario_id
        || context.base_problem.metadata.scenario_revision
            != context.compile_context.scenario_revision
    {
        return Err(invalid(
            "/context/baseProblem",
            "The diagnostic baseline does not belong to this scenario revision.",
        ));
    }
    precharge_validation(context.base_problem, budget).map_err(|error| map_operation(&error))?;
    validate(context.base_problem, limits).map_err(|_| {
        invalid(
            "/context/baseProblem",
            "The diagnostic baseline planning model is invalid.",
        )
    })?;
    if let Some(condition) = context.condition {
        condition
            .validate()
            .map_err(|_| invalid("/condition", "The diagnostic condition is invalid."))?;
        let key = MetadataKey::new(CONDITION_METADATA_KEY)
            .map_err(|_| invalid("/condition", "Invalid condition metadata identity."))?;
        match context.base_problem.metadata.compile_metadata.get(&key) {
            Some(ProvenanceParameter::Text(checksum)) if checksum == &condition.checksum => {}
            _ => {
                return Err(invalid(
                    "/context/baseProblem",
                    "The diagnostic condition does not match its admitted baseline.",
                ));
            }
        }
    } else if context
        .base_problem
        .metadata
        .compile_metadata
        .keys()
        .any(|key| key.as_str() == CONDITION_METADATA_KEY)
    {
        return Err(invalid(
            "/context/baseProblem",
            "An unconditional diagnostic cannot use a conditional baseline.",
        ));
    }
    Ok(())
}

fn reject_invalid_fixed_targets(report: &DomainValidationReport) -> Result<(), DomainPackError> {
    if report.issues.iter().any(|issue| {
        issue.code == "official.workforce.fixed_assignment_unresolved_person"
            || issue.code == "official.workforce.fixed_assignment_unresolved_shift"
            || issue.code == "official.workforce.fixed_assignment_out_of_scope"
    }) {
        return Err(invalid(
            "/document/domain/rules",
            "A FixedAssignment target is missing or outside its authored scope.",
        ));
    }
    Ok(())
}

fn resolve_condition(
    base: &PlanningProblem,
    condition: Option<&CounterfactualConditionV1>,
    candidates: &[AssignmentPair],
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<(AssignmentPair, bool)>, DomainPackError> {
    let Some(condition) = condition else {
        return Ok(Vec::new());
    };
    let projection = |assignment_id: &DomainAssignmentId| {
        let pair_text = assignment_id
            .as_str()
            .strip_prefix(ASSIGNMENT_KIND)
            .and_then(|suffix| suffix.strip_prefix('.'))?;
        base.projections.iter().find(|entry| {
            &entry.assignment_id == assignment_id
                && entry.required
                && matches!(&entry.expression, ProjectionExpression::Boolean(_))
                && entry.entity.kind.as_str() == ASSIGNMENT_KIND
                && entry.entity.id.as_str() == pair_text
        })
    };
    let pair = |assignment_id: &DomainAssignmentId| -> Result<AssignmentPair, DomainPackError> {
        if projection(assignment_id).is_none() {
            return Err(invalid(
                "/condition",
                "The condition assignment is not an admitted original projection.",
            ));
        }
        let pair = decode_workforce_assignment_id(assignment_id)?;
        if !candidates.contains(&pair) || input.person(pair.person_id).is_none() {
            return Err(invalid(
                "/condition",
                "The condition assignment is not present in the diagnostic source universe.",
            ));
        }
        Ok(pair)
    };
    let pins = match &condition.condition {
        CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(value),
        } => vec![(pair(assignment_id)?, *value)],
        CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(value),
        } => vec![(pair(assignment_id)?, !*value)],
        CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id,
            replaced_assignment_id,
        } => {
            let target = pair(target_assignment_id)?;
            let replaced = pair(replaced_assignment_id)?;
            if target.shift_id != replaced.shift_id {
                return Err(invalid(
                    "/condition",
                    "A replacement must concern the same shift.",
                ));
            }
            vec![(target, true), (replaced, false)]
        }
        _ => {
            return Err(invalid(
                "/condition",
                "Only Boolean Workforce conditions are supported.",
            ));
        }
    };
    budget
        .steps(count(pins.len()).map_err(|_| invalid("/condition", "Condition is too large."))?)
        .map_err(|error| map_operation(&error))?;
    Ok(pins)
}

// Shape admission and each corresponding budget reservation stay together before graph allocation.
#[allow(clippy::too_many_lines)]
fn preflight_diagnostic(
    analysis: &AssignmentAnalysis,
    plan: &Plan,
    relaxable: &BTreeSet<RuleId>,
    assumption_count: usize,
    condition_count: usize,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    preflight(&analysis.candidates, plan, budget, limits).map_err(|error| map_operation(&error))?;
    let hours = hours::checked_shape(plan, limits).map_err(|error| map_operation(&error))?;
    let consecutive = consecutive::shape(plan, limits).map_err(|error| map_operation(&error))?;
    let weekend = compiler_weekend::shape(plan, limits).map_err(|error| map_operation(&error))?;
    let generic_constraints = plan
        .constraints
        .iter()
        .filter(|planned| !consecutive::is(planned) && !compiler_weekend::is(planned))
        .count();
    let mut rejection_keys = BTreeSet::new();
    let candidate_search_work = count(analysis.candidates.len())?;
    let candidate_search_steps = u64::from(u64::BITS - candidate_search_work.leading_zeros());
    for rejection in &analysis.rejections {
        budget.step().map_err(|error| map_operation(&error))?;
        budget
            .steps(candidate_search_steps)
            .map_err(|error| map_operation(&error))?;
        if relaxable.contains(&rejection.binding_id)
            && analysis.candidates.binary_search(&rejection.pair).is_ok()
            && matches!(
                rejection.cause,
                super::RejectionCause::AssignmentTypeNotAllowed { .. }
                    | super::RejectionCause::QualificationExpression { .. }
                    | super::RejectionCause::Unavailable { .. }
                    | super::RejectionCause::OutsideAvailableOnly { .. }
            )
        {
            rejection_keys.insert((rejection.binding_id, rejection.pair));
        }
    }
    let rejection_count = rejection_keys.len();
    let generic_constraints = count(generic_constraints).map_err(|error| map_operation(&error))?;
    let rejection_count_ir = count(rejection_count).map_err(|error| map_operation(&error))?;
    let condition_count_ir = count(condition_count).map_err(|error| map_operation(&error))?;
    let constraints = generic_constraints
        .checked_add(hours.constraints)
        .and_then(|value| value.checked_add(consecutive.constraints))
        .and_then(|value| value.checked_add(weekend.constraints))
        .and_then(|value| value.checked_add(rejection_count_ir))
        .and_then(|value| value.checked_add(condition_count_ir))
        .ok_or_else(|| invalid("/document", "Diagnostic shape arithmetic overflow."))?;
    within(
        constraints,
        limits.max_constraints,
        AssignmentRuleLimit::Constraints,
    )
    .map_err(|error| map_operation(&error))?;
    let candidate_count =
        count(analysis.candidates.len()).map_err(|error| map_operation(&error))?;
    let assumptions = count(assumption_count).map_err(|error| map_operation(&error))?;
    let variables = candidate_count
        .checked_add(hours.pair_indices)
        .and_then(|value| value.checked_add(hours.helper_variables))
        .and_then(|value| value.checked_add(consecutive.variables))
        .and_then(|value| value.checked_add(weekend.variables))
        .and_then(|value| value.checked_add(assumptions))
        .ok_or_else(|| invalid("/document", "Diagnostic shape arithmetic overflow."))?;
    within(
        variables,
        limits.max_variables,
        AssignmentRuleLimit::Variables,
    )
    .map_err(|error| map_operation(&error))?;
    within(
        assumptions,
        limits.max_assumptions,
        AssignmentRuleLimit::Records,
    )
    .map_err(|error| map_operation(&error))?;
    // Guard literals, Boolean-only projections, assumptions, restored rejection records and
    // condition pins are all charged before their corresponding vectors are allocated.
    let hours_channels = hours
        .pair_indices
        .checked_mul(2)
        .ok_or_else(|| invalid("/document", "Diagnostic guard shape overflow."))?;
    let hours_guarded = hours
        .constraints
        .checked_sub(hours_channels)
        .ok_or_else(|| invalid("/document", "Diagnostic guard shape overflow."))?;
    let generic_guard_count = plan
        .constraints
        .iter()
        .filter(|planned| {
            !matches!(planned.predicate, Predicate::HardLock { .. })
                && relaxable.contains(&planned.rule)
                && !consecutive::is(planned)
                && !compiler_weekend::is(planned)
        })
        .count();
    let guarded_constraints = count(generic_guard_count)
        .map_err(|error| map_operation(&error))?
        .checked_add(consecutive.constraints)
        .and_then(|value| value.checked_add(weekend.constraints))
        .and_then(|value| value.checked_add(hours_guarded))
        .and_then(|value| value.checked_add(rejection_count_ir))
        .ok_or_else(|| invalid("/document", "Diagnostic guard shape overflow."))?;
    for _ in 0..analysis.candidates.len() {
        budget
            .reserve_ir(1, 4, 768)
            .map_err(|error| map_operation(&error))?;
    }
    for _ in 0..assumption_count {
        budget
            .reserve_ir(1, 4, 768)
            .map_err(|error| map_operation(&error))?;
    }
    for _ in 0..rejection_count {
        budget
            .reserve_ir(1, 4, 768)
            .map_err(|error| map_operation(&error))?;
    }
    for _ in 0..condition_count {
        budget
            .reserve_ir(1, 3, 768)
            .map_err(|error| map_operation(&error))?;
    }
    for _ in 0..guarded_constraints {
        budget
            .reserve_ir(0, 1, 64)
            .map_err(|error| map_operation(&error))?;
    }
    budget.check().map_err(|error| map_operation(&error))
}

fn add_assumptions(
    rules: &[RuleId],
    parents: &BTreeMap<RuleId, ProvenanceId>,
    identities: &mut PlanningIdentities,
    variables: &mut Vec<Variable>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
) -> Result<BTreeMap<RuleId, Literal>, AssignmentRuleError> {
    let map_count = count(rules.len())?;
    budget.reserve(
        0,
        map_count,
        map_count.checked_mul(1024).ok_or_else(arithmetic)?,
    )?;
    let mut guards = BTreeMap::new();
    for rule in rules {
        budget.step()?;
        let parent = parents.get(rule).ok_or_else(invalid_rule)?;
        let key = ("diagnostic_assumption", rule);
        let bool_id =
            BoolVariableId::new(identities.derive(IdentityKind::Boolean, &key, budget)?)
                .map_err(|_| invalid_rule())?;
        let literal = Literal::positive(bool_id.clone());
        let fact_id = ProvenanceId::new(identities.derive(
            IdentityKind::Provenance,
            &("diagnostic_assumption_fact", rule),
            budget,
        )?)
        .map_err(|_| invalid_rule())?;
        budget.reserve(1, 4, 768)?;
        provenance.push(ProvenanceRecord {
            id: fact_id.clone(),
            source_kind: ProvenanceSourceKind::Derived,
            source_id: rule.to_string(),
            entity_refs: Vec::new(),
            message_key: "official.workforce.diagnostic.assumption".to_owned(),
            parameters: BTreeMap::new(),
            parent: Some(parent.clone()),
        });
        variables.push(Variable::Boolean(BoolVariable {
            id: bool_id,
            provenance: fact_id,
        }));
        if guards.insert(*rule, literal).is_some() {
            return Err(invalid_rule());
        }
    }
    Ok(guards)
}

fn build_assumptions(
    rules: &[RuleId],
    guards: &BTreeMap<RuleId, Literal>,
    parents: &BTreeMap<RuleId, ProvenanceId>,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<Assumption>, AssignmentRuleError> {
    rules
        .iter()
        .map(|rule| {
            budget.step()?;
            let literal = guards.get(rule).ok_or_else(invalid_rule)?.clone();
            let provenance = parents.get(rule).ok_or_else(invalid_rule)?.clone();
            let id =
                AssumptionGroupId::new(format!("official.workforce.assumption.required.{rule}"))
                    .map_err(|_| invalid_rule())?;
            budget.reserve(1, 2, 512)?;
            Ok(Assumption {
                id,
                literal,
                required_rules: vec![*rule],
                provenance,
            })
        })
        .collect()
}

// Keep rejection identity, ownership, guard and paired IR emission in one source-bound operation.
#[allow(clippy::too_many_arguments)]
fn add_authored_rejection_constraints(
    analysis: &AssignmentAnalysis,
    relaxable: &BTreeSet<RuleId>,
    variables: &[AssignmentVariable],
    guards: &BTreeMap<RuleId, Literal>,
    parents: &BTreeMap<RuleId, ProvenanceId>,
    identities: &mut PlanningIdentities,
    constraints: &mut Vec<ConstraintRecord>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut seen = BTreeSet::new();
    let variable_search_work = count(variables.len())?;
    let variable_search_steps = u64::from(u64::BITS - variable_search_work.leading_zeros());
    for rejection in &analysis.rejections {
        budget.step()?;
        if !relaxable.contains(&rejection.binding_id)
            || !matches!(
                rejection.cause,
                super::RejectionCause::AssignmentTypeNotAllowed { .. }
                    | super::RejectionCause::QualificationExpression { .. }
                    | super::RejectionCause::Unavailable { .. }
                    | super::RejectionCause::OutsideAvailableOnly { .. }
            )
        {
            continue;
        }
        budget.steps(variable_search_steps)?;
        let variable = match variables.binary_search_by_key(&rejection.pair, |entry| entry.pair) {
            Ok(index) => &variables[index],
            Err(_) => continue,
        };
        if !seen.insert((rejection.binding_id, rejection.pair)) {
            continue;
        }
        let guard = guards.get(&rejection.binding_id).ok_or_else(invalid_rule)?;
        let parent = parents
            .get(&rejection.binding_id)
            .ok_or_else(invalid_rule)?;
        let key = (
            "diagnostic_candidate_rejection",
            rejection.binding_id,
            rejection.pair,
        );
        let id =
            PlanningConstraintId::new(identities.derive(IdentityKind::Constraint, &key, budget)?)
                .map_err(|_| invalid_rule())?;
        let fact_id =
            ProvenanceId::new(identities.derive(IdentityKind::Provenance, &key, budget)?)
                .map_err(|_| invalid_rule())?;
        budget.reserve(1, 5, 768)?;
        constraints.push(ConstraintRecord {
            id,
            body: Constraint::bool_or(vec![Literal::negative(variable.variable.id.clone())]),
            enforcement: vec![guard.clone()],
            provenance: fact_id.clone(),
            tags: Vec::new(),
        });
        provenance.push(ProvenanceRecord {
            id: fact_id,
            source_kind: ProvenanceSourceKind::Derived,
            source_id: rejection.binding_id.to_string(),
            entity_refs: Vec::new(),
            message_key: "official.workforce.diagnostic.candidate_rejection".to_owned(),
            parameters: BTreeMap::new(),
            parent: Some(parent.clone()),
        });
    }
    Ok(())
}

fn add_condition_constraints(
    pins: &[(AssignmentPair, bool)],
    condition: Option<&CounterfactualConditionV1>,
    variables: &[AssignmentVariable],
    identities: &mut PlanningIdentities,
    constraints: &mut Vec<ConstraintRecord>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let Some(condition) = condition else {
        return Ok(());
    };
    let fact_id = ProvenanceId::new(identities.derive(
        IdentityKind::Provenance,
        &("diagnostic_condition", &condition.checksum),
        budget,
    )?)
    .map_err(|_| invalid_rule())?;
    budget.reserve(1, 1, 512)?;
    provenance.push(ProvenanceRecord {
        id: fact_id.clone(),
        source_kind: ProvenanceSourceKind::Fact,
        source_id: condition.checksum.clone(),
        entity_refs: Vec::new(),
        message_key: "official.workforce.diagnostic.condition".to_owned(),
        parameters: BTreeMap::new(),
        parent: None,
    });
    for (index, (pair, value)) in pins.iter().enumerate() {
        let variable = variables
            .iter()
            .find(|entry| entry.pair == *pair)
            .ok_or_else(invalid_rule)?;
        let key = (
            "diagnostic_condition_pin",
            &condition.checksum,
            index,
            pair,
            value,
        );
        let id =
            PlanningConstraintId::new(identities.derive(IdentityKind::Constraint, &key, budget)?)
                .map_err(|_| invalid_rule())?;
        let pin_fact =
            ProvenanceId::new(identities.derive(IdentityKind::Provenance, &key, budget)?)
                .map_err(|_| invalid_rule())?;
        budget.reserve(1, 4, 768)?;
        constraints.push(ConstraintRecord {
            id,
            body: Constraint::bool_or(vec![if *value {
                Literal::positive(variable.variable.id.clone())
            } else {
                Literal::negative(variable.variable.id.clone())
            }]),
            enforcement: Vec::new(),
            provenance: pin_fact.clone(),
            tags: Vec::new(),
        });
        provenance.push(ProvenanceRecord {
            id: pin_fact,
            source_kind: ProvenanceSourceKind::Derived,
            source_id: condition.checksum.clone(),
            entity_refs: Vec::new(),
            message_key: "official.workforce.diagnostic.condition_pin".to_owned(),
            parameters: BTreeMap::new(),
            parent: Some(fact_id.clone()),
        });
    }
    Ok(())
}

fn append_rule_guards(
    constraints: &mut [ConstraintRecord],
    provenance: &[ProvenanceRecord],
    guards: &BTreeMap<RuleId, Literal>,
    parents: &BTreeMap<RuleId, ProvenanceId>,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let provenance_count = count(provenance.len())?;
    let parent_count = count(parents.len())?;
    let index_items = add(provenance_count, parent_count)?;
    budget.reserve(
        0,
        index_items,
        index_items.checked_mul(512).ok_or_else(arithmetic)?,
    )?;
    let index_work = u64::from(u64::BITS - index_items.leading_zeros());
    let mut fact_index = BTreeMap::<ProvenanceId, usize>::new();
    for (index, record) in provenance.iter().enumerate() {
        budget.step()?;
        budget.steps(index_work)?;
        if fact_index.insert(record.id.clone(), index).is_some() {
            return Err(invalid_rule());
        }
    }
    let mut guard_by_parent = BTreeMap::<ProvenanceId, Option<Literal>>::new();
    for (rule, parent) in parents {
        budget.step()?;
        budget.steps(index_work)?;
        if guard_by_parent
            .insert(parent.clone(), guards.get(rule).cloned())
            .is_some()
        {
            return Err(invalid_rule());
        }
    }
    for constraint in constraints {
        budget.step()?;
        budget.steps(index_work)?;
        let Some(&fact_index) = fact_index.get(&constraint.provenance) else {
            return Err(invalid_rule());
        };
        let fact = provenance.get(fact_index).ok_or_else(invalid_rule)?;
        let Some(guard) = fact
            .parent
            .as_ref()
            .and_then(|parent| guard_by_parent.get(parent))
            .and_then(|guard| guard.as_ref())
        else {
            continue;
        };
        budget.steps(count(constraint.enforcement.len())?)?;
        if constraint.enforcement.contains(guard) {
            continue;
        }
        let next = count(constraint.enforcement.len())?
            .checked_add(1)
            .ok_or_else(arithmetic)?;
        within(
            next,
            limits
                .max_enforcement_literals
                .min(PlanningIrLimitsV1::DEFAULT.max_enforcement_literals),
            AssignmentRuleLimit::PerRecord,
        )?;
        budget.reserve(0, 1, 64)?;
        constraint.enforcement.push(guard.clone());
    }
    Ok(())
}

fn diagnostic_metadata(
    document: &ScenarioDocument,
    context: &InfeasibilityCompileContext<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<PlanningMetadata, DomainPackError> {
    let mut compile_metadata = BTreeMap::new();
    for (key, value) in &context.compile_context.semantic_metadata {
        budget.step().map_err(|error| map_operation(&error))?;
        let key = MetadataKey::new(key.clone())
            .map_err(|_| invalid("/context/semanticMetadata", "Invalid metadata key."))?;
        compile_metadata.insert(key, ProvenanceParameter::Text(value.clone()));
    }
    if let Some(condition) = context.condition {
        let key = MetadataKey::new(CONDITION_METADATA_KEY)
            .map_err(|_| invalid("/condition", "Invalid condition metadata identity."))?;
        compile_metadata.insert(key, ProvenanceParameter::Text(condition.checksum.clone()));
    }
    budget
        .reserve(1, compile_metadata.len() as u64, 1024)
        .map_err(|error| map_operation(&error))?;
    Ok(PlanningMetadata {
        pack_id: PackId::new(WORKFORCE_PACK_ID)
            .map_err(|_| invalid("/document", "Invalid Workforce pack identity."))?,
        scenario_id: document.scenario_id,
        scenario_revision: context.compile_context.scenario_revision,
        projection_version: WORKFORCE_PROJECTION_VERSION,
        compiler_id: CompilerId::new(DIAGNOSTIC_COMPILER_ID)
            .map_err(|_| invalid("/document", "Invalid diagnostic compiler identity."))?,
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
        compile_metadata,
        display_text: BTreeMap::new(),
    })
}

fn diagnostic_capabilities() -> BTreeSet<Capability> {
    BTreeSet::from([
        Capability::AtMostOne,
        Capability::Assumptions,
        Capability::BoolAnd,
        Capability::BoolOr,
        Capability::BooleanProjection,
        Capability::CardinalityRange,
        Capability::LinearComparison,
    ])
}

fn invalid(path: &str, message: &str) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

fn invalid_rule() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}

fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow)
}

#[cfg(test)]
#[path = "compiler_diagnostic_tests.rs"]
mod tests;
