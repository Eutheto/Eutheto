use super::super::types::AssignmentRuleError;
use crate::test_support::{fixture, id};
use eutheto_domain_api::{CompileContext, DomainPackError, InfeasibilityCompileContext};
use eutheto_domain_ir::{
    AssignmentValue, CounterfactualConditionPayloadV1, CounterfactualConditionV1,
    DomainAssignmentId,
};
use eutheto_planning_ir::{
    Constraint, MetadataKey, PlanningIrLimitsV1, PlanningProblem, ProvenanceParameter,
    ProvenanceSourceKind,
};
use eutheto_types::{
    CancellationToken, DurationMillis, OperationControl, ParentSolveBudget, ScenarioDocument,
    SystemMonotonicClock,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error, sync::Arc};

type Result<T = (), E = Box<dyn Error>> = std::result::Result<T, E>;

fn source() -> Result<ScenarioDocument> {
    let mut document = fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    document.domain.locked_assignments.clear();
    Ok(document)
}

fn context_with_limits(limits: PlanningIrLimitsV1) -> CompileContext {
    CompileContext {
        objective_context: None,
        scenario_revision: 1,
        semantic_metadata: BTreeMap::new(),
        control: OperationControl::Cancellation(CancellationToken::new()),
        planning_limits: limits,
    }
}

fn diagnostic(
    document: &ScenarioDocument,
    condition: Option<&CounterfactualConditionV1>,
) -> Result<PlanningProblem> {
    diagnostic_with_limits(document, condition, PlanningIrLimitsV1::DEFAULT)
}

fn diagnostic_with_limits(
    document: &ScenarioDocument,
    condition: Option<&CounterfactualConditionV1>,
    limits: PlanningIrLimitsV1,
) -> Result<PlanningProblem> {
    let compile_context = context_with_limits(limits);
    let compiled = super::super::compiler_model::compile_workforce(document, &compile_context)?;
    let mut base = compiled.problem;
    if let Some(condition) = condition {
        base.metadata.compile_metadata.insert(
            MetadataKey::new("eutheto.counterfactual.condition_hash")?,
            ProvenanceParameter::Text(condition.checksum.clone()),
        );
    }
    let parent = ParentSolveBudget::new(
        DurationMillis::new(30_000)?,
        Arc::new(SystemMonotonicClock::new()),
        CancellationToken::new(),
    )?;
    let compile = InfeasibilityCompileContext {
        base_problem: &base,
        compile_context: &compile_context,
        condition,
        budget: parent.phase_view(),
    };
    Ok(super::compile_infeasibility(document, &compile)?)
}

#[test]
fn diagnostic_compilation_observes_independent_compile_cancellation() -> Result {
    let document = source()?;
    let compile_context = context_with_limits(PlanningIrLimitsV1::DEFAULT);
    let base =
        super::super::compiler_model::compile_workforce(&document, &compile_context)?.problem;
    let OperationControl::Cancellation(token) = &compile_context.control else {
        return Err("fixture requires cancellation control".into());
    };
    token.cancel();
    let parent = ParentSolveBudget::new(
        DurationMillis::new(30_000)?,
        Arc::new(SystemMonotonicClock::new()),
        CancellationToken::new(),
    )?;
    let result = eutheto_domain_api::DomainPack::compile_infeasibility(
        &crate::WorkforcePack,
        &document,
        &InfeasibilityCompileContext {
            base_problem: &base,
            compile_context: &compile_context,
            condition: None,
            budget: parent.phase_view(),
        },
    );
    assert!(matches!(result, Err(DomainPackError::Cancelled)));
    Ok(())
}

fn authored_eligibility(document: &mut ScenarioDocument) -> Result {
    document.domain.rules.insert(
        id(20).parse()?,
        json!({
            "kind":"eligibility", "id":id(20), "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}}
        }),
    );
    Ok(())
}

fn fixed(document: &mut ScenarioDocument, assigned: bool, person: u32) -> Result {
    document.domain.rules.insert(
        id(20).parse()?,
        json!({
            "kind":"fixedAssignment", "id":id(20), "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}}, "personId":id(person), "shiftId":id(8),
            "assigned":assigned
        }),
    );
    Ok(())
}

fn pair_projection_count(problem: &PlanningProblem) -> usize {
    let pair = format!("{}.{}", id(1), id(8));
    problem
        .projections
        .iter()
        .filter(|projection| projection.entity.id.as_str() == pair)
        .count()
}

#[test]
fn authored_pruning_is_restored_but_candidate_rejection_stays_guarded() -> Result {
    let mut all_active = source()?;
    authored_eligibility(&mut all_active)?;
    let original = diagnostic(&all_active, None)?;
    assert_eq!(pair_projection_count(&original), 1);
    assert!(!original
        .provenance
        .iter()
        .any(|record| record.message_key == "official.workforce.diagnostic.candidate_rejection"));

    let mut one_drop = all_active;
    one_drop
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] = json!([]);
    let restored = diagnostic(&one_drop, None)?;
    assert_eq!(pair_projection_count(&restored), 1);
    let rejection = restored
        .provenance
        .iter()
        .find(|record| record.message_key == "official.workforce.diagnostic.candidate_rejection")
        .ok_or("missing restored candidate rejection")?;
    assert_eq!(rejection.source_id, id(20));
    let rejection_constraint = restored
        .constraints
        .iter()
        .find(|constraint| constraint.provenance == rejection.id)
        .ok_or("missing guarded rejection constraint")?;
    assert_eq!(rejection_constraint.enforcement.len(), 1);
    assert!(matches!(
        &rejection_constraint.body,
        Constraint::BoolOr { literals } if literals.len() == 1 && !literals[0].positive
    ));
    let required_rule = id(20).parse()?;
    assert!(
        restored
            .assumptions
            .iter()
            .any(|assumption| assumption.required_rules == vec![required_rule])
    );
    Ok(())
}

#[test]
fn activity_and_approved_leave_remain_facts_while_locks_remain_unconditional() -> Result {
    let mut activity = source()?;
    activity
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["activeRange"] = json!({
        "kind":"dateRange", "startDate":"2026-12-01", "endDateExclusive":"2026-12-02"
    });
    assert_eq!(pair_projection_count(&diagnostic(&activity, None)?), 0);
    let mut mixed_authored = activity.clone();
    authored_eligibility(&mut mixed_authored)?;
    mixed_authored
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] = json!([]);
    let mixed_problem = diagnostic(&mixed_authored, None)?;
    assert_eq!(pair_projection_count(&mixed_problem), 0);
    assert!(!mixed_problem
        .provenance
        .iter()
        .any(|record| record.message_key == "official.workforce.diagnostic.candidate_rejection"));

    let mut leave = source()?;
    leave.domain.entities.insert(
        id(30).parse()?,
        json!({
            "kind":"availability", "id":id(30), "personId":id(1),
            "availabilityKind":"approvedTimeOff",
            "timeWindow":{"kind":"instant","startsAt":"2026-11-01T05:00:00Z","endsAt":"2026-11-01T08:00:00Z"},
            "effectiveRange":{"startDate":"2026-01-01","endDateExclusive":"2027-01-01"},
            "source":"fixture", "note":"fixture"
        }),
    );
    assert_eq!(pair_projection_count(&diagnostic(&leave, None)?), 0);

    let mut locked = source()?;
    locked.domain.locked_assignments.insert(
        id(50).parse()?,
        json!({"id":id(50),"personId":id(1),"shiftId":id(8),"state":{"kind":"hard"}}),
    );
    let locked_problem = diagnostic(&locked, None)?;
    assert_eq!(pair_projection_count(&locked_problem), 1);
    let lock_fact = locked_problem
        .provenance
        .iter()
        .find(|record| {
            record.source_kind == ProvenanceSourceKind::Fact
                && record.message_key == "official.workforce.hard_lock"
        })
        .ok_or("missing lock fact")?;
    let lock_constraint = locked_problem
        .constraints
        .iter()
        .find(|constraint| {
            locked_problem
                .provenance
                .iter()
                .find(|record| record.id == constraint.provenance)
                .and_then(|record| record.parent.as_ref())
                == Some(&lock_fact.id)
        })
        .ok_or("missing lock constraint")?;
    assert!(lock_constraint.enforcement.is_empty());
    Ok(())
}

#[test]
fn false_fixed_assignment_is_vacuous_for_fact_pruned_pair_but_missing_target_is_rejected() -> Result
{
    let mut pruned = source()?;
    pruned
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["activeRange"] = json!({
        "kind":"dateRange", "startDate":"2026-12-01", "endDateExclusive":"2026-12-02"
    });
    fixed(&mut pruned, false, 1)?;
    let problem = diagnostic(&pruned, None)?;
    let fixed_fact = problem
        .provenance
        .iter()
        .find(|record| record.message_key == "official.workforce.fixed_assignment_excluded")
        .ok_or("missing false fixed fact")?;
    let fixed_constraint = problem
        .constraints
        .iter()
        .find(|constraint| constraint.provenance == fixed_fact.id)
        .ok_or("missing false fixed constraint")?;
    assert_eq!(fixed_constraint.enforcement.len(), 1);
    assert!(matches!(
        &fixed_constraint.body,
        Constraint::BoolAnd { literals } if literals.is_empty()
    ));

    for assigned in [false, true] {
        let mut missing = source()?;
        fixed(&mut missing, assigned, 99)?;
        let error = diagnostic(&missing, None)
            .err()
            .ok_or("missing-target diagnostic unexpectedly succeeded")?;
        assert!(
            matches!(
                error.downcast_ref::<AssignmentRuleError>(),
                Some(AssignmentRuleError::InvalidDocument(
                    DomainPackError::InvalidPayload { .. }
                ))
            ),
            "{error:?}"
        );
    }
    Ok(())
}

#[test]
fn exact_condition_pin_uses_admitted_projection_identity_and_source_checksum() -> Result {
    let document = source()?;
    let assignment_id =
        DomainAssignmentId::new(format!("official.workforce.assignment.{}.{}", id(1), id(8)))?;
    let condition =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(false),
        })?;
    let problem = diagnostic(&document, Some(&condition))?;
    let pin = problem
        .provenance
        .iter()
        .find(|record| record.message_key == "official.workforce.diagnostic.condition_pin")
        .ok_or("missing condition pin provenance")?;
    assert_eq!(pin.source_id, condition.checksum);
    let condition_fact = pin.parent.as_ref().ok_or("missing condition source fact")?;
    assert!(problem.provenance.iter().any(|record| {
        &record.id == condition_fact
            && record.message_key == "official.workforce.diagnostic.condition"
            && record.source_id == condition.checksum
    }));
    let constraint = problem
        .constraints
        .iter()
        .find(|constraint| constraint.provenance == pin.id)
        .ok_or("missing condition pin constraint")?;
    assert!(constraint.enforcement.is_empty());
    assert!(matches!(
        &constraint.body,
        Constraint::BoolOr { literals } if literals.len() == 1 && !literals[0].positive
    ));
    Ok(())
}

#[test]
fn diagnostic_preflight_counts_contribution_pair_indices_before_construction() -> Result {
    let mut document = source()?;
    document.domain.entities.remove(&id(6).parse()?);
    document
        .domain
        .entities
        .get_mut(&id(3).parse()?)
        .ok_or("workload bucket")?["overlappingContribution"] = json!("sum");
    document.domain.rules.insert(
        id(20).parse()?,
        json!({
            "kind":"maximumHours", "id":id(20), "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}}, "bucketId":id(3),
            "window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},
            "maximumMinutes":480
        }),
    );
    let ordinary = super::super::compiler_model::compile_workforce(
        &document,
        &context_with_limits(PlanningIrLimitsV1::DEFAULT),
    )?;
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_variables = u64::try_from(ordinary.problem.variables.len())?;
    let baseline =
        super::super::compiler_model::compile_workforce(&document, &context_with_limits(limits))?;
    assert_eq!(baseline.problem, ordinary.problem);
    let error = diagnostic_with_limits(&document, None, limits)
        .err()
        .ok_or("diagnostic variable preflight unexpectedly succeeded")?;
    assert!(matches!(
        error.downcast_ref::<DomainPackError>(),
        Some(DomainPackError::ResourceLimitExceeded)
    ));
    let admitted = diagnostic(&document, None)?;
    let rule = id(20).parse()?;
    let assumption = admitted
        .assumptions
        .iter()
        .find(|assumption| assumption.required_rules.as_slice() == [rule])
        .ok_or("missing hours assumption")?;
    let fact = admitted
        .provenance
        .iter()
        .find(|fact| fact.message_key == "official.workforce.maximum_hours")
        .ok_or("missing hours constraint fact")?;
    let parent = fact.parent.as_ref().ok_or("missing hours rule parent")?;
    assert!(admitted.provenance.iter().any(|fact| {
        &fact.id == parent
            && fact.source_kind == ProvenanceSourceKind::RequiredRule
            && fact.source_id == id(20)
    }));
    let constraint = admitted
        .constraints
        .iter()
        .find(|constraint| constraint.provenance == fact.id)
        .ok_or("missing hours comparison")?;
    assert!(constraint.enforcement.contains(&assumption.literal));
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn consecutive_diagnostic_guard_controls_full_model_feasibility_and_consumes_enforcement() -> Result
{
    use crate::model::AssignmentPair;
    use eutheto_planning_ir::{
        CandidateValues, ComparisonOp, Literal, ProjectionExpression, Variable,
    };

    let mut document = source()?;
    document.domain.entities.remove(&id(6).parse()?);
    let mut second = document
        .domain
        .entities
        .get(&id(8).parse()?)
        .ok_or("shift")?
        .clone();
    second["id"] = json!(id(22));
    document.domain.entities.insert(id(22).parse()?, second);
    document.domain.rules.insert(
        id(20).parse()?,
        json!({
            "kind":"maximumConsecutive", "id":id(20), "active":true,
            "strength":"required", "scope":{"people":{"kind":"all"}},
            "mode":{"kind":"assignments","breakMinutes":0}, "maximum":1
        }),
    );
    let selected = [8, 22]
        .map(|shift| {
            Ok(AssignmentPair {
                person_id: id(1).parse()?,
                shift_id: id(shift).parse()?,
            })
        })
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let evaluated = super::super::evaluate_assignment_rules(&document, &selected, None)?;
    assert!(evaluated.obligations.remaining.is_empty());
    assert!(evaluated.evaluations.iter().any(|rule| !rule.satisfied));

    let problem = diagnostic(&document, None)?;
    let rule = id(20).parse()?;
    let assumption = problem
        .assumptions
        .iter()
        .find(|assumption| assumption.required_rules.as_slice() == [rule])
        .ok_or("missing consecutive assumption")?;
    let parent = problem
        .provenance
        .iter()
        .find(|fact| fact.id == assumption.provenance)
        .ok_or("missing authored consecutive parent")?;
    assert_eq!(parent.source_kind, ProvenanceSourceKind::RequiredRule);
    assert_eq!(parent.source_id, id(20));
    assert!(!problem.constraints.is_empty());
    for constraint in &problem.constraints {
        let fact = problem
            .provenance
            .iter()
            .find(|fact| fact.id == constraint.provenance)
            .ok_or("missing consecutive constraint fact")?;
        assert_eq!(fact.source_id, id(20));
        assert_eq!(fact.parent.as_ref(), Some(&parent.id));
        assert!(constraint.enforcement.contains(&assumption.literal));
    }
    for variable in &problem.variables {
        let (provenance, helper) = match variable {
            Variable::Boolean(variable) => {
                let projected = problem.projections.iter().any(|projection| {
                    matches!(&projection.expression, ProjectionExpression::Boolean(id) if id == &variable.id)
                });
                (
                    &variable.provenance,
                    !projected && variable.id != assumption.literal.variable,
                )
            }
            Variable::Integer(variable) => (&variable.provenance, true),
            Variable::Interval(_) => return Err("unexpected diagnostic interval".into()),
        };
        let fact = problem
            .provenance
            .iter()
            .find(|fact| &fact.id == provenance)
            .ok_or("missing variable fact")?;
        if helper {
            assert_eq!(fact.source_id, id(20));
            assert_eq!(fact.parent.as_ref(), Some(&parent.id));
        }
    }
    let projected = problem
        .projections
        .iter()
        .map(|projection| {
            let (person, shift) = projection
                .entity
                .id
                .as_str()
                .split_once('.')
                .ok_or("pair identity")?;
            let pair = AssignmentPair {
                person_id: person.parse()?,
                shift_id: shift.parse()?,
            };
            assert!(selected.contains(&pair));
            let ProjectionExpression::Boolean(variable) = &projection.expression else {
                return Err("unexpected assignment projection".into());
            };
            Ok(variable.clone())
        })
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(projected.len(), selected.len());

    // Enumerate only these tiny declared domains and evaluate every enforced IR
    // primitive. No helper identity or prefix-state implementation is assumed.
    let feasible = |enabled: bool| -> Result<bool> {
        let domains = problem
            .variables
            .iter()
            .map(|variable| {
                let domain = match variable {
                    Variable::Boolean(variable) if variable.id == assumption.literal.variable => {
                        vec![i64::from(enabled == assumption.literal.positive)]
                    }
                    Variable::Boolean(variable) if projected.contains(&variable.id) => vec![1],
                    Variable::Boolean(_) => vec![0, 1],
                    Variable::Integer(variable) => {
                        let mut domain = Vec::new();
                        for range in &variable.domain.inclusive_ranges {
                            let width = i128::from(range.end) - i128::from(range.start) + 1;
                            assert!((1..=4096).contains(&width));
                            assert!(domain.len() + usize::try_from(width)? <= 4096);
                            domain.extend(range.start..=range.end);
                        }
                        domain
                    }
                    Variable::Interval(_) => return Err("unexpected diagnostic interval".into()),
                };
                Ok(domain)
            })
            .collect::<Result<Vec<_>>>()?;
        let combinations = domains.iter().try_fold(1_usize, |count, domain| {
            let count = count
                .checked_mul(domain.len())
                .ok_or("diagnostic domain product")?;
            assert!(count <= 4096, "only enumerate tiny diagnostic fixtures");
            Ok::<_, Box<dyn Error>>(count)
        })?;
        for mut index in 0..combinations {
            let mut values = CandidateValues::default();
            for (variable, domain) in problem.variables.iter().zip(&domains) {
                let value = domain[index % domain.len()];
                index /= domain.len();
                match variable {
                    Variable::Boolean(variable) => {
                        values.booleans.insert(variable.id.clone(), value != 0);
                    }
                    Variable::Integer(variable) => {
                        values.integers.insert(variable.id.clone(), value);
                    }
                    Variable::Interval(_) => return Err("unexpected diagnostic interval".into()),
                }
            }
            let literal = |literal: &Literal| -> Result<bool> {
                Ok(*values
                    .booleans
                    .get(&literal.variable)
                    .ok_or("missing Boolean")?
                    == literal.positive)
            };
            let mut accepted = true;
            for constraint in &problem.constraints {
                let mut enforced = true;
                for guard in &constraint.enforcement {
                    enforced &= literal(guard)?;
                }
                if !enforced {
                    continue;
                }
                let (literals, minimum, maximum) = match &constraint.body {
                    Constraint::LinearComparison(comparison) => {
                        let lhs = comparison.expression.terms.iter().try_fold(
                            i128::from(comparison.expression.constant),
                            |sum, term| -> Result<i128> {
                                Ok(sum
                                    + i128::from(term.coefficient)
                                        * i128::from(
                                            *values
                                                .integers
                                                .get(&term.variable)
                                                .ok_or("missing integer")?,
                                        ))
                            },
                        )?;
                        let rhs = i128::from(comparison.rhs);
                        accepted &= match comparison.op {
                            ComparisonOp::Equal => lhs == rhs,
                            ComparisonOp::LessOrEqual => lhs <= rhs,
                            ComparisonOp::GreaterOrEqual => lhs >= rhs,
                        };
                        continue;
                    }
                    Constraint::BoolOr { literals } => (literals, 1, u64::MAX),
                    Constraint::BoolAnd { literals } => {
                        (literals, u64::try_from(literals.len())?, u64::MAX)
                    }
                    Constraint::AtMostOne { literals } => (literals, 0, 1),
                    Constraint::ExactlyOne { literals } => (literals, 1, 1),
                    Constraint::CardinalityRange { literals, min, max } => (literals, *min, *max),
                    _ => return Err("unexpected diagnostic primitive".into()),
                };
                let mut count = 0;
                for item in literals {
                    count += u64::from(literal(item)?);
                }
                accepted &= minimum <= count && count <= maximum;
            }
            if accepted {
                return Ok(true);
            }
        }
        Ok(false)
    };
    assert!(
        !feasible(true)?,
        "enabled rule must exclude the source violation"
    );
    assert!(
        feasible(false)?,
        "disabled rule must permit the source violation"
    );

    let ordinary = super::super::compiler_model::compile_workforce(
        &document,
        &context_with_limits(PlanningIrLimitsV1::DEFAULT),
    )?
    .problem;
    let ordinary_enforcement = ordinary
        .constraints
        .iter()
        .map(|record| record.enforcement.len())
        .max()
        .ok_or("missing ordinary consecutive constraints")?;
    let guarded_enforcement = problem
        .constraints
        .iter()
        .map(|record| record.enforcement.len())
        .max()
        .ok_or("missing guarded consecutive constraints")?;
    assert_eq!(guarded_enforcement, ordinary_enforcement + 1);
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_enforcement_literals = u64::try_from(guarded_enforcement)?;
    assert_eq!(diagnostic_with_limits(&document, None, limits)?, problem);
    limits.max_enforcement_literals = u64::try_from(ordinary_enforcement)?;
    super::super::compiler_model::compile_workforce(&document, &context_with_limits(limits))?;
    let error = diagnostic_with_limits(&document, None, limits)
        .err()
        .ok_or("diagnostic authored guard escaped the enforcement budget")?;
    assert!(matches!(
        error.downcast_ref::<DomainPackError>(),
        Some(DomainPackError::ResourceLimitExceeded)
    ));
    Ok(())
}
