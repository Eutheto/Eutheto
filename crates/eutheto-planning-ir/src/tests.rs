use crate::test_evaluator::evaluate_record;
use crate::*;
use eutheto_domain_ir::{
    AssignmentValue, AssumptionGroupId, DomainAssignmentId, DomainEntityId, DomainEntityKindId,
    DomainEntityRef, DomainEvidenceId, OptimizationDirection, ScoreCategoryId,
};
use eutheto_types::{
    CancellationToken, DurationMillis, FixedMonotonicClock, OperationControl, PackId,
    ParentSolveBudget, RuleId, ScenarioId, SolutionId,
};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

const TRANSIENT_TEST_CEILING: u64 = 64 * 1024 * 1024;

fn bool_id(name: &str) -> Result<BoolVariableId, PlanningIdError> {
    BoolVariableId::new(format!("bool.{name}"))
}

fn int_id(name: &str) -> Result<IntVariableId, PlanningIdError> {
    IntVariableId::new(format!("int.{name}"))
}

fn interval_id(name: &str) -> Result<IntervalVariableId, PlanningIdError> {
    IntervalVariableId::new(format!("interval.{name}"))
}

fn provenance_id() -> Result<ProvenanceId, PlanningIdError> {
    ProvenanceId::new("provenance.root")
}

fn domain(minimum: i64, maximum: i64) -> Result<IntDomain, ModelError> {
    IntDomain::new(vec![InclusiveRange {
        start: minimum,
        end: maximum,
    }])
}

fn provenance() -> Result<ProvenanceRecord, Box<dyn Error>> {
    Ok(ProvenanceRecord {
        id: provenance_id()?,
        source_kind: ProvenanceSourceKind::Fact,
        source_id: "school.fixture".to_owned(),
        entity_refs: vec![DomainEntityRef {
            kind: DomainEntityKindId::new("school.section")?,
            id: DomainEntityId::new("school.section-a")?,
        }],
        message_key: "school.fixture".to_owned(),
        parameters: BTreeMap::new(),
        parent: None,
    })
}

fn rule_id(index: u8) -> Result<RuleId, Box<dyn Error>> {
    Ok(format!("01890a5d-ac96-7b64-9f74-bbfcf30f9f{index:02x}").parse()?)
}

fn required_rule_provenance() -> Result<ProvenanceRecord, Box<dyn Error>> {
    Ok(ProvenanceRecord {
        id: ProvenanceId::new("provenance.required-rule")?,
        source_kind: ProvenanceSourceKind::RequiredRule,
        source_id: rule_id(1)?.to_string(),
        entity_refs: Vec::new(),
        message_key: "school.required-rule".to_owned(),
        parameters: BTreeMap::new(),
        parent: None,
    })
}

fn assumption(
    name: &str,
    literal: Literal,
    required_rules: Vec<RuleId>,
    provenance: ProvenanceId,
) -> Result<Assumption, Box<dyn Error>> {
    Ok(Assumption {
        id: AssumptionGroupId::new(format!("assumption.{name}"))?,
        literal,
        required_rules,
        provenance,
    })
}

fn assumption_problem(assumptions: Vec<Assumption>) -> Result<PlanningProblem, Box<dyn Error>> {
    let mut problem = base_problem()?;
    problem.assumptions = assumptions;
    problem.provenance.push(required_rule_provenance()?);
    problem.canonicalize()?;
    Ok(problem)
}

// This fixture stays cohesive so every shared declaration is initialized in one canonical model.
#[allow(clippy::too_many_lines)]
fn base_problem() -> Result<PlanningProblem, Box<dyn Error>> {
    let provenance_id = provenance_id()?;
    let mut variables = vec![
        Variable::Boolean(BoolVariable {
            id: bool_id("a")?,
            provenance: provenance_id.clone(),
        }),
        Variable::Boolean(BoolVariable {
            id: bool_id("b")?,
            provenance: provenance_id.clone(),
        }),
        Variable::Integer(IntVariable {
            id: int_id("duration_a")?,
            domain: domain(0, 10)?,
            provenance: provenance_id.clone(),
        }),
        Variable::Integer(IntVariable {
            id: int_id("duration_b")?,
            domain: domain(0, 10)?,
            provenance: provenance_id.clone(),
        }),
        Variable::Integer(IntVariable {
            id: int_id("end_a")?,
            domain: domain(-10, 20)?,
            provenance: provenance_id.clone(),
        }),
        Variable::Integer(IntVariable {
            id: int_id("end_b")?,
            domain: domain(-10, 20)?,
            provenance: provenance_id.clone(),
        }),
        Variable::Integer(IntVariable {
            id: int_id("index")?,
            domain: domain(-1, 3)?,
            provenance: provenance_id.clone(),
        }),
        Variable::Integer(IntVariable {
            id: int_id("start_a")?,
            domain: domain(-10, 20)?,
            provenance: provenance_id.clone(),
        }),
        Variable::Integer(IntVariable {
            id: int_id("start_b")?,
            domain: domain(-10, 20)?,
            provenance: provenance_id.clone(),
        }),
        Variable::Integer(IntVariable {
            id: int_id("x")?,
            domain: domain(-10, 10)?,
            provenance: provenance_id.clone(),
        }),
        Variable::Integer(IntVariable {
            id: int_id("y")?,
            domain: domain(-10, 10)?,
            provenance: provenance_id.clone(),
        }),
        Variable::Integer(IntVariable {
            id: int_id("z")?,
            domain: domain(0, 20)?,
            provenance: provenance_id.clone(),
        }),
        Variable::Interval(IntervalVariable {
            id: interval_id("a")?,
            start: int_id("start_a")?,
            duration: int_id("duration_a")?,
            end: int_id("end_a")?,
            presence: None,
            provenance: provenance_id.clone(),
        }),
        Variable::Interval(IntervalVariable {
            id: interval_id("b")?,
            start: int_id("start_b")?,
            duration: int_id("duration_b")?,
            end: int_id("end_b")?,
            presence: Some(Literal::positive(bool_id("b")?)),
            provenance: provenance_id,
        }),
    ];
    variables.sort_by(|left, right| left.canonical_id().cmp(right.canonical_id()));
    let mut problem = PlanningProblem {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables,
        constraints: Vec::new(),
        objectives: ObjectivePlan::default(),
        assumptions: Vec::new(),
        projections: Vec::new(),
        provenance: vec![provenance()?],
        metadata: PlanningMetadata {
            pack_id: PackId::new("official.synthetic")?,
            scenario_id: "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse::<ScenarioId>()?,
            scenario_revision: 4,
            projection_version: PROJECTION_SCHEMA_VERSION,
            compiler_id: CompilerId::new("compiler.synthetic")?,
            compiler_version: "1.0.0".to_owned(),
            compile_metadata: BTreeMap::new(),
            display_text: BTreeMap::new(),
        },
        declared_capabilities: BTreeSet::new(),
        split_authorization: None,
    };
    problem.canonicalize()?;
    Ok(problem)
}

fn candidate() -> Result<CandidateValues, Box<dyn Error>> {
    Ok(CandidateValues {
        booleans: BTreeMap::from([(bool_id("a")?, true), (bool_id("b")?, true)]),
        integers: BTreeMap::from([
            (int_id("duration_a")?, 2),
            (int_id("duration_b")?, 2),
            (int_id("end_a")?, 2),
            (int_id("end_b")?, 4),
            (int_id("index")?, 1),
            (int_id("start_a")?, 0),
            (int_id("start_b")?, 2),
            (int_id("x")?, 2),
            (int_id("y")?, 3),
            (int_id("z")?, 5),
        ]),
    })
}

fn record(id: &str, body: Constraint) -> Result<ConstraintRecord, Box<dyn Error>> {
    Ok(ConstraintRecord {
        id: PlanningConstraintId::new(format!("constraint.{id}"))?,
        body,
        enforcement: Vec::new(),
        provenance: provenance_id()?,
        tags: Vec::new(),
    })
}

fn evaluate(body: Constraint, candidate: &CandidateValues) -> Result<bool, Box<dyn Error>> {
    Ok(evaluate_record(
        &record("evaluation", body)?,
        &base_problem()?,
        candidate,
    )?)
}

#[test]
fn id_and_domain_exact_normalization_boundaries() -> Result<(), Box<dyn Error>> {
    assert!(BoolVariableId::new("bool.valid_name-2").is_ok());
    assert!(BoolVariableId::new("Bool.invalid").is_err());
    let normalized = IntDomain::new(vec![
        InclusiveRange { start: 4, end: 5 },
        InclusiveRange { start: 1, end: 2 },
        InclusiveRange { start: 3, end: 4 },
    ])?;
    assert_eq!(
        normalized.inclusive_ranges,
        vec![InclusiveRange { start: 1, end: 5 }]
    );
    assert!(normalized.contains(1));
    assert!(normalized.contains(5));
    assert!(!normalized.contains(6));
    assert!(IntDomain::new(Vec::new()).is_err());
    assert!(IntDomain::new(vec![InclusiveRange { start: 2, end: 1 }]).is_err());
    Ok(())
}

#[test]
fn boolean_primitive_empty_single_and_invalid_semantics() -> Result<(), Box<dyn Error>> {
    let values = candidate()?;
    assert!(!evaluate(Constraint::bool_or(Vec::new()), &values)?);
    assert!(evaluate(Constraint::bool_and(Vec::new()), &values)?);
    assert!(evaluate(Constraint::at_most_one(Vec::new()), &values)?);
    assert!(!evaluate(Constraint::exactly_one(Vec::new()), &values)?);
    assert!(evaluate(
        Constraint::bool_or(vec![Literal::positive(bool_id("a")?)]),
        &values
    )?);
    assert!(!evaluate(
        Constraint::AtMostOne {
            literals: vec![
                Literal::positive(bool_id("a")?),
                Literal::positive(bool_id("b")?)
            ],
        },
        &values,
    )?);
    assert!(evaluate(
        Constraint::Implication {
            antecedent: Literal::positive(bool_id("a")?),
            consequent: Literal::positive(bool_id("b")?),
        },
        &values,
    )?);
    assert!(evaluate(
        Constraint::Equivalence {
            left: Literal::positive(bool_id("a")?),
            right: Literal::positive(bool_id("b")?),
        },
        &values,
    )?);
    assert_eq!(
        Constraint::cardinality(Vec::new(), 0, 0)?,
        Constraint::CardinalityRange {
            literals: Vec::new(),
            min: 0,
            max: 0
        }
    );
    assert!(Constraint::cardinality(Vec::new(), 0, 1).is_err());
    Ok(())
}

#[test]
fn integer_and_reified_primitives_cover_true_false_and_overflow() -> Result<(), Box<dyn Error>> {
    let values = candidate()?;
    let sum = LinearExpression::new(
        vec![
            LinearTerm {
                variable: int_id("y")?,
                coefficient: 1,
            },
            LinearTerm {
                variable: int_id("x")?,
                coefficient: 1,
            },
        ],
        0,
    )?;
    let comparison = LinearComparison {
        expression: sum,
        op: ComparisonOp::Equal,
        rhs: 5,
    };
    assert!(evaluate(
        Constraint::LinearComparison(comparison.clone()),
        &values
    )?);
    assert!(evaluate(
        Constraint::ReifiedLinearComparison {
            literal: Literal::positive(bool_id("a")?),
            comparison,
        },
        &values,
    )?);
    assert!(evaluate(Constraint::all_different(Vec::new()), &values)?);
    assert!(evaluate(
        Constraint::all_different(vec![int_id("x")?]),
        &values
    )?);
    assert!(evaluate(
        Constraint::all_different(vec![int_id("x")?, int_id("y")?]),
        &values,
    )?);
    let mut same = values.clone();
    same.integers.insert(int_id("y")?, 2);
    assert!(!evaluate(
        Constraint::all_different(vec![int_id("x")?, int_id("y")?]),
        &same,
    )?);
    assert!(
        LinearExpression::new(
            vec![
                LinearTerm {
                    variable: int_id("x")?,
                    coefficient: i64::MAX
                },
                LinearTerm {
                    variable: int_id("x")?,
                    coefficient: 1
                },
            ],
            0,
        )
        .is_err()
    );
    Ok(())
}

fn expression_position_problems(
    expression: &LinearExpression,
) -> Result<Vec<PlanningProblem>, Box<dyn Error>> {
    let comparison = LinearComparison {
        expression: expression.clone(),
        op: ComparisonOp::Equal,
        rhs: 5,
    };
    let mut plain = base_problem()?;
    plain.constraints.push(record(
        "plain-expression",
        Constraint::LinearComparison(comparison.clone()),
    )?);
    let mut reified = base_problem()?;
    reified.constraints.push(record(
        "reified-expression",
        Constraint::ReifiedLinearComparison {
            literal: Literal::positive(bool_id("a")?),
            comparison,
        },
    )?);
    let mut objective = base_problem()?;
    objective.objectives.levels = vec![ObjectiveLevel {
        id: ObjectiveLevelId::new("level.expression")?,
        direction: OptimizationDirection::Minimize,
        lower_bound: i64::MIN,
        upper_bound: i64::MAX,
        terms: vec![ObjectiveTerm {
            id: ObjectiveTermId::new("objective.expression")?,
            expression: expression.clone(),
            kind: ObjectiveTermKind::Penalty,
            category: ScoreCategoryId::new("score.expression")?,
            provenance: provenance_id()?,
        }],
        provenance: provenance_id()?,
    }];
    let mut projection = base_problem()?;
    projection.projections.push(SolutionProjection {
        id: ProjectionId::new("projection.expression")?,
        assignment_id: DomainAssignmentId::new("assignment.expression")?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new("school.summary")?,
            id: DomainEntityId::new("school.summary")?,
        },
        required: true,
        expression: ProjectionExpression::Linear(expression.clone()),
        provenance: provenance_id()?,
    });
    Ok(vec![plain, reified, objective, projection])
}

#[test]
fn canonicalize_expression_positions_match_constructor_normalization() -> Result<(), Box<dyn Error>>
{
    let cases: &[&[(&str, i64)]] = &[
        &[],
        &[("x", 7)],
        &[("x", i64::MIN), ("y", i64::MAX)],
        &[("y", 2), ("x", 3)],
        &[("x", 2), ("x", 3)],
        &[("y", 0), ("x", 2), ("x", -2), ("y", 3), ("y", 4), ("z", 0)],
    ];
    for terms in cases {
        let expression = LinearExpression {
            terms: terms
                .iter()
                .map(|(name, coefficient)| {
                    Ok(LinearTerm {
                        variable: int_id(name)?,
                        coefficient: *coefficient,
                    })
                })
                .collect::<Result<_, PlanningIdError>>()?,
            constant: -7,
        };
        let normalized = LinearExpression::new(expression.terms.clone(), expression.constant)?;
        let raw_positions = expression_position_problems(&expression)?;
        let expected_positions = expression_position_problems(&normalized)?;
        for (mut problem, mut expected) in raw_positions.into_iter().zip(expected_positions) {
            problem.canonicalize()?;
            expected.canonicalize()?;
            assert_eq!(problem.constraints, expected.constraints);
            assert_eq!(problem.objectives, expected.objectives);
            assert_eq!(problem.projections, expected.projections);
            let canonical_bytes = serde_json::to_vec(&problem)?;
            assert_eq!(canonical_bytes, serde_json::to_vec(&expected)?);
            problem.canonicalize()?;
            assert_eq!(serde_json::to_vec(&problem)?, canonical_bytes);
        }
    }
    Ok(())
}

#[test]
fn canonicalize_expression_overflow_preserves_each_failing_position() -> Result<(), Box<dyn Error>>
{
    for coefficients in [[i64::MAX, 1, -1], [i64::MIN, -1, 1]] {
        let expression = LinearExpression {
            terms: coefficients
                .into_iter()
                .map(|coefficient| {
                    Ok(LinearTerm {
                        variable: int_id("x")?,
                        coefficient,
                    })
                })
                .collect::<Result<_, PlanningIdError>>()?,
            constant: 11,
        };
        // The eventual mathematical sum fits, but the authored combination order overflows.
        assert_eq!(
            LinearExpression::new(expression.terms.clone(), expression.constant),
            Err(ModelError::ArithmeticOverflow)
        );
        for mut problem in expression_position_problems(&expression)? {
            let before = serde_json::to_vec(&(
                &problem.constraints,
                &problem.objectives,
                &problem.projections,
            ))?;
            assert_eq!(problem.canonicalize(), Err(ModelError::ArithmeticOverflow));
            assert_eq!(
                serde_json::to_vec(&(
                    &problem.constraints,
                    &problem.objectives,
                    &problem.projections,
                ))?,
                before
            );
        }
    }
    Ok(())
}

#[test]
fn table_semantics_include_zero_arity_and_exact_rows() -> Result<(), Box<dyn Error>> {
    let values = candidate()?;
    assert!(!evaluate(
        Constraint::allowed_table(vec![int_id("x")?], Vec::new())?,
        &values
    )?);
    assert!(evaluate(
        Constraint::forbidden_table(vec![int_id("x")?], Vec::new())?,
        &values
    )?);
    assert!(evaluate(
        Constraint::allowed_table(Vec::new(), vec![Vec::new()])?,
        &values
    )?);
    assert!(!evaluate(
        Constraint::forbidden_table(Vec::new(), vec![Vec::new()])?,
        &values
    )?);
    assert!(evaluate(
        Constraint::allowed_table(vec![int_id("x")?, int_id("y")?], vec![vec![2, 3]])?,
        &values,
    )?);
    assert!(Constraint::allowed_table(vec![int_id("x")?], vec![vec![1, 2]]).is_err());
    Ok(())
}

#[test]
fn element_min_max_equality_and_abs_have_exact_semantics() -> Result<(), Box<dyn Error>> {
    let values = candidate()?;
    assert!(evaluate(
        Constraint::Element {
            index: int_id("index")?,
            values: vec![1, 5],
            target: int_id("z")?
        },
        &values,
    )?);
    let mut outside = values.clone();
    outside.integers.insert(int_id("index")?, -1);
    assert!(!evaluate(
        Constraint::Element {
            index: int_id("index")?,
            values: vec![1, 5],
            target: int_id("z")?
        },
        &outside,
    )?);
    assert!(evaluate(
        Constraint::min(int_id("x")?, vec![int_id("x")?, int_id("y")?])?,
        &values
    )?);
    assert!(evaluate(
        Constraint::max(int_id("y")?, vec![int_id("x")?, int_id("y")?])?,
        &values
    )?);
    assert!(Constraint::min(int_id("x")?, Vec::new()).is_err());
    assert!(Constraint::max(int_id("x")?, Vec::new()).is_err());
    assert!(evaluate(
        Constraint::Equality {
            left: int_id("z")?,
            right: int_id("z")?
        },
        &values
    )?);
    assert!(evaluate(
        Constraint::AbsDifference {
            target: int_id("x")?,
            left: int_id("z")?,
            right: int_id("y")?
        },
        &values
    )?);
    Ok(())
}

#[test]
fn interval_globals_use_presence_half_open_and_checked_demand() -> Result<(), Box<dyn Error>> {
    let values = candidate()?;
    assert!(evaluate(
        Constraint::no_overlap(vec![interval_id("a")?, interval_id("b")?]),
        &values,
    )?);
    assert!(evaluate(
        Constraint::cumulative(vec![interval_id("a")?, interval_id("b")?], vec![2, 2], 2,)?,
        &values,
    )?);
    let mut overlap = values.clone();
    overlap.integers.insert(int_id("start_b")?, 1);
    overlap.integers.insert(int_id("end_b")?, 3);
    assert!(!evaluate(
        Constraint::no_overlap(vec![interval_id("a")?, interval_id("b")?]),
        &overlap,
    )?);
    assert!(!evaluate(
        Constraint::cumulative(vec![interval_id("a")?, interval_id("b")?], vec![2, 2], 3,)?,
        &overlap,
    )?);
    overlap.booleans.insert(bool_id("b")?, false);
    assert!(evaluate(
        Constraint::no_overlap(vec![interval_id("a")?, interval_id("b")?]),
        &overlap,
    )?);
    assert!(Constraint::cumulative(vec![interval_id("a")?], vec![-1], 1).is_err());
    assert!(Constraint::cumulative(vec![interval_id("a")?], Vec::new(), 1).is_err());
    Ok(())
}

#[test]
fn interval_domains_require_a_checked_coherent_present_value() -> Result<(), Box<dyn Error>> {
    fn set_domain(
        problem: &mut PlanningProblem,
        id: &IntVariableId,
        replacement: IntDomain,
    ) -> Result<(), Box<dyn Error>> {
        let variable = problem
            .variables
            .iter_mut()
            .find_map(|variable| match variable {
                Variable::Integer(value) if &value.id == id => Some(value),
                _ => None,
            })
            .ok_or("missing interval component")?;
        variable.domain = replacement;
        Ok(())
    }

    assert!(validate(&base_problem()?, PlanningIrLimitsV1::DEFAULT).is_ok());

    let mut boundary = base_problem()?;
    set_domain(
        &mut boundary,
        &int_id("start_a")?,
        domain(i64::MAX, i64::MAX)?,
    )?;
    set_domain(&mut boundary, &int_id("duration_a")?, domain(0, 0)?)?;
    set_domain(
        &mut boundary,
        &int_id("end_a")?,
        domain(i64::MAX, i64::MAX)?,
    )?;
    let wide_limits = PlanningIrLimitsV1 {
        max_abs_value: i64::MAX,
        ..PlanningIrLimitsV1::DEFAULT
    };
    assert!(validate(&boundary, wide_limits).is_ok());

    let mut overflow = boundary.clone();
    set_domain(&mut overflow, &int_id("duration_a")?, domain(1, 1)?)?;
    assert!(matches!(
        validate(&overflow, wide_limits),
        Err(ValidationError {
            code: ValidationCode::ArithmeticOverflow,
            ..
        })
    ));

    let mut mandatory_incoherent = base_problem()?;
    set_domain(
        &mut mandatory_incoherent,
        &int_id("start_a")?,
        domain(0, 0)?,
    )?;
    set_domain(
        &mut mandatory_incoherent,
        &int_id("duration_a")?,
        domain(2, 2)?,
    )?;
    set_domain(&mut mandatory_incoherent, &int_id("end_a")?, domain(1, 1)?)?;
    assert!(matches!(
        validate(&mandatory_incoherent, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::InvalidDomain,
            ..
        })
    ));

    let mut optional_incoherent = base_problem()?;
    set_domain(&mut optional_incoherent, &int_id("start_b")?, domain(0, 0)?)?;
    set_domain(
        &mut optional_incoherent,
        &int_id("duration_b")?,
        domain(-2, -1)?,
    )?;
    set_domain(&mut optional_incoherent, &int_id("end_b")?, domain(0, 0)?)?;
    assert!(matches!(
        validate(&optional_incoherent, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::InvalidDomain,
            ..
        })
    ));
    Ok(())
}

#[test]
fn enforcement_is_conjunctive_and_empty_is_active() -> Result<(), Box<dyn Error>> {
    let values = candidate()?;
    let mut false_body = record("enforced", Constraint::bool_or(Vec::new()))?;
    assert!(!evaluate_record(&false_body, &base_problem()?, &values)?);
    false_body.enforcement = vec![Literal::negative(bool_id("a")?)];
    assert!(evaluate_record(&false_body, &base_problem()?, &values)?);
    false_body.enforcement = vec![
        Literal::positive(bool_id("a")?),
        Literal::negative(bool_id("b")?),
    ];
    assert!(evaluate_record(&false_body, &base_problem()?, &values)?);
    Ok(())
}

#[test]
fn builder_output_validates_and_strict_validator_rejects_noncanonical() -> Result<(), Box<dyn Error>>
{
    let mut problem = base_problem()?;
    problem.constraints.push(record(
        "or",
        Constraint::bool_or(vec![
            Literal::positive(bool_id("b")?),
            Literal::positive(bool_id("a")?),
            Literal::positive(bool_id("a")?),
        ]),
    )?);
    problem.canonicalize()?;
    assert!(validate(&problem, PlanningIrLimitsV1::DEFAULT).is_ok());
    if let Constraint::BoolOr { literals } = &mut problem.constraints[0].body {
        literals.reverse();
    }
    assert!(matches!(
        validate(&problem, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::NonCanonical,
            ..
        })
    ));
    Ok(())
}

#[test]
fn objective_bounds_scalarization_and_overflow_are_exact() -> Result<(), Box<dyn Error>> {
    let mut problem = base_problem()?;
    let term = ObjectiveTerm {
        id: ObjectiveTermId::new("objective.x")?,
        expression: LinearExpression::new(
            vec![LinearTerm {
                variable: int_id("x")?,
                coefficient: 1,
            }],
            10,
        )?,
        kind: ObjectiveTermKind::Penalty,
        category: ScoreCategoryId::new("score.preference")?,
        provenance: provenance_id()?,
    };
    problem.objectives.levels = vec![ObjectiveLevel {
        id: ObjectiveLevelId::new("level.preference")?,
        direction: OptimizationDirection::Minimize,
        lower_bound: 0,
        upper_bound: 20,
        terms: vec![term],
        provenance: provenance_id()?,
    }];
    problem.canonicalize()?;
    assert!(validate(&problem, PlanningIrLimitsV1::DEFAULT).is_ok());
    assert!(matches!(
        lexicographic_strategy(&problem.objectives),
        LexicographicStrategy::ExactScalarization { .. }
    ));

    for kind in [ObjectiveTermKind::Penalty, ObjectiveTermKind::Reward] {
        let mut negative_contribution = base_problem()?;
        negative_contribution.objectives.levels = vec![ObjectiveLevel {
            id: ObjectiveLevelId::new("level.preference")?,
            direction: OptimizationDirection::Minimize,
            lower_bound: 0,
            upper_bound: 20,
            terms: vec![
                ObjectiveTerm {
                    id: ObjectiveTermId::new("objective.negative")?,
                    expression: LinearExpression::new(
                        vec![LinearTerm {
                            variable: int_id("x")?,
                            coefficient: 1,
                        }],
                        0,
                    )?,
                    kind,
                    category: ScoreCategoryId::new("score.preference")?,
                    provenance: provenance_id()?,
                },
                ObjectiveTerm {
                    id: ObjectiveTermId::new("objective.offset")?,
                    expression: LinearExpression::new(Vec::new(), 10)?,
                    kind,
                    category: ScoreCategoryId::new("score.preference")?,
                    provenance: provenance_id()?,
                },
            ],
            provenance: provenance_id()?,
        }];
        negative_contribution.canonicalize()?;
        assert!(matches!(
            validate(&negative_contribution, PlanningIrLimitsV1::DEFAULT),
            Err(ValidationError {
                code: ValidationCode::InvalidObjectiveBounds,
                ..
            })
        ));
    }

    let mut levels = Vec::new();
    for index in 0..3 {
        levels.push(ObjectiveLevel {
            id: ObjectiveLevelId::new(format!("level.l{index}"))?,
            direction: OptimizationDirection::Minimize,
            lower_bound: -1_000_000_000_000_000,
            upper_bound: 1_000_000_000_000_000,
            terms: Vec::new(),
            provenance: provenance_id()?,
        });
    }
    assert_eq!(
        lexicographic_strategy(&ObjectivePlan { levels }),
        LexicographicStrategy::Multipass
    );
    problem.objectives.levels[0].upper_bound = 19;
    assert!(matches!(
        validate(&problem, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::InvalidObjectiveBounds,
            ..
        })
    ));
    Ok(())
}

#[test]
fn projection_rejects_unknown_missing_type_domain_and_handles_absence() -> Result<(), Box<dyn Error>>
{
    let mut problem = base_problem()?;
    let entity = DomainEntityRef {
        kind: DomainEntityKindId::new("school.occurrence")?,
        id: DomainEntityId::new("school.occurrence-a")?,
    };
    problem.projections = vec![
        SolutionProjection {
            id: ProjectionId::new("projection.interval")?,
            assignment_id: DomainAssignmentId::new("assignment.interval")?,
            entity: entity.clone(),
            required: true,
            expression: ProjectionExpression::Interval(interval_id("b")?),
            provenance: provenance_id()?,
        },
        SolutionProjection {
            id: ProjectionId::new("projection.optional")?,
            assignment_id: DomainAssignmentId::new("assignment.optional")?,
            entity,
            required: false,
            expression: ProjectionExpression::Integer(int_id("x")?),
            provenance: provenance_id()?,
        },
    ];
    problem.canonicalize()?;
    let solution_id = "01890a5d-ac96-7b64-9f74-bbfcf30f9f81".parse::<SolutionId>()?;
    let mut values = candidate()?;
    values.booleans.insert(bool_id("b")?, false);
    values.integers.remove(&int_id("x")?);
    let projected = project_candidate(&problem, &values, solution_id, PlanningIrLimitsV1::DEFAULT)?;
    assert_eq!(projected.assignments[0].value, AssignmentValue::Absent);
    assert_eq!(projected.assignments[1].value, AssignmentValue::Absent);
    let projection_evidence = DomainEvidenceId::new(provenance_id()?.as_str())?;
    assert!(projected.assignments.iter().all(|assignment| {
        assignment.evidence.as_slice() == std::slice::from_ref(&projection_evidence)
    }));
    let mut unknown = values.clone();
    unknown.booleans.insert(bool_id("unknown")?, true);
    let solution_id = "01890a5d-ac96-7b64-9f74-bbfcf30f9f82".parse::<SolutionId>()?;
    assert!(matches!(
        project_candidate(&problem, &unknown, solution_id, PlanningIrLimitsV1::DEFAULT),
        Err(ProjectionError::UnknownCandidateValue(_))
    ));
    let mut out = candidate()?;
    out.integers.insert(int_id("x")?, 11);
    let solution_id = "01890a5d-ac96-7b64-9f74-bbfcf30f9f83".parse::<SolutionId>()?;
    assert!(matches!(
        project_candidate(&problem, &out, solution_id, PlanningIrLimitsV1::DEFAULT),
        Err(ProjectionError::OutOfDomain(_))
    ));
    Ok(())
}

#[test]
fn provenance_graph_missing_references_and_capability_mismatch_reject() -> Result<(), Box<dyn Error>>
{
    let mut problem = base_problem()?;
    problem.provenance[0].parent = Some(provenance_id()?);
    assert!(matches!(
        validate(&problem, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::ProvenanceCycle,
            ..
        })
    ));
    problem.provenance[0].parent = None;
    problem.constraints.push(record(
        "missing",
        Constraint::bool_or(vec![Literal::positive(bool_id("missing")?)]),
    )?);
    problem.canonicalize()?;
    assert!(matches!(
        validate(&problem, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::MissingReference,
            ..
        })
    ));
    problem.constraints.clear();
    problem.canonicalize()?;
    problem.declared_capabilities.clear();
    assert!(matches!(
        validate(&problem, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::UndeclaredCapability,
            ..
        })
    ));
    Ok(())
}

#[test]
fn provenance_rejects_orphans_but_retains_referenced_ancestors() -> Result<(), Box<dyn Error>> {
    let mut problem = base_problem()?;
    let parent_id = ProvenanceId::new("provenance.parent")?;
    problem.provenance[0].parent = Some(parent_id.clone());
    problem.provenance.push(ProvenanceRecord {
        id: parent_id,
        source_kind: ProvenanceSourceKind::Derived,
        source_id: "school.parent".to_owned(),
        entity_refs: Vec::new(),
        message_key: "school.parent".to_owned(),
        parameters: BTreeMap::new(),
        parent: None,
    });
    problem.canonicalize()?;
    assert!(validate(&problem, PlanningIrLimitsV1::DEFAULT).is_ok());

    problem.provenance.push(ProvenanceRecord {
        id: ProvenanceId::new("provenance.orphan")?,
        source_kind: ProvenanceSourceKind::Fact,
        source_id: "school.orphan".to_owned(),
        entity_refs: Vec::new(),
        message_key: "school.orphan".to_owned(),
        parameters: BTreeMap::new(),
        parent: None,
    });
    problem.canonicalize()?;
    assert!(matches!(
        validate(&problem, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::OrphanProvenance,
            ..
        })
    ));
    Ok(())
}

fn provenance_forest(
    parents: &[(&str, Option<&str>)],
    used: &[&str],
) -> Result<PlanningProblem, Box<dyn Error>> {
    let mut problem = base_problem()?;
    problem.variables = used
        .iter()
        .enumerate()
        .map(|(index, name)| {
            Ok(Variable::Boolean(BoolVariable {
                id: bool_id(&format!("leaf-{index}"))?,
                provenance: ProvenanceId::new(format!("provenance.{name}"))?,
            }))
        })
        .collect::<Result<_, PlanningIdError>>()?;
    problem.provenance = parents
        .iter()
        .map(|(name, parent)| {
            let mut record = provenance()?;
            record.id = ProvenanceId::new(format!("provenance.{name}"))?;
            record.parent = parent
                .map(|name| ProvenanceId::new(format!("provenance.{name}")))
                .transpose()?;
            Ok(record)
        })
        .collect::<Result<_, Box<dyn Error>>>()?;
    problem.canonicalize()?;
    Ok(problem)
}

fn assert_provenance_error(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
    code: ValidationCode,
    path: &str,
) -> Result<(), Box<dyn Error>> {
    let expected = ValidationError {
        code,
        path: path.to_owned(),
    };
    assert_eq!(validate(problem, limits), Err(expected.clone()));
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    let Err(BudgetedValidationError::Invalid(actual)) =
        validate_with_budget(problem, limits, &mut budget)
    else {
        return Err(
            "budgeted provenance validation did not return the expected semantic error".into(),
        );
    };
    assert_eq!(actual, expected);
    Ok(())
}

#[test]
fn provenance_coverage_preserves_root_and_reference_failures() -> Result<(), Box<dyn Error>> {
    let limits = PlanningIrLimitsV1::DEFAULT;
    for problem in [
        provenance_forest(&[], &[])?,
        provenance_forest(&[("root", None)], &["root"])?,
        provenance_forest(&[("leaf", Some("root")), ("root", None)], &["leaf", "leaf"])?,
    ] {
        assert_eq!(validate(&problem, limits), Ok(()));
        let mut budget = PlanningPreparationBudget::uncontrolled(limits);
        drop(validate_with_budget(&problem, limits, &mut budget)?);
    }
    let mut problem = provenance_forest(&[("orphan", None), ("root", None)], &["root"])?;
    assert_provenance_error(
        &problem,
        limits,
        ValidationCode::OrphanProvenance,
        "provenance",
    )?;
    problem.provenance[0].message_key.clear();
    assert_provenance_error(
        &problem,
        limits,
        ValidationCode::LimitExceeded,
        "provenance.messageKey",
    )?;
    let missing = provenance_forest(&[("orphan", None)], &["missing"])?;
    assert_provenance_error(
        &missing,
        limits,
        ValidationCode::MissingProvenance,
        "variables[0]",
    )?;
    let mut duplicate = provenance_forest(&[("root", None)], &["root"])?;
    duplicate.provenance.push(duplicate.provenance[0].clone());
    assert_provenance_error(
        &duplicate,
        limits,
        ValidationCode::NonCanonical,
        "provenance",
    )?;
    Ok(())
}

#[test]
fn provenance_memoized_suffixes_preserve_exact_depth_and_ordering() -> Result<(), Box<dyn Error>> {
    let limits = PlanningIrLimitsV1 {
        max_provenance_depth: 3,
        ..PlanningIrLimitsV1::DEFAULT
    };
    // Root and tail cache before both leaves: reaching cached depth2 through prefix1
    // must admit depth3, not double-count the cached current record as depth4.
    let problem = provenance_forest(
        &[
            ("a-root", None),
            ("b-tail", Some("a-root")),
            ("c-leaf", Some("b-tail")),
            ("z-leaf", Some("b-tail")),
        ],
        &["c-leaf", "z-leaf"],
    )?;
    assert_eq!(validate(&problem, limits), Ok(()));
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    drop(validate_with_budget(&problem, limits, &mut budget)?);
    assert_provenance_error(
        &problem,
        PlanningIrLimitsV1 {
            max_provenance_depth: 2,
            ..limits
        },
        ValidationCode::LimitExceeded,
        "provenance.depth",
    )?;
    let mut unsorted = problem.clone();
    unsorted.provenance.reverse();
    assert_provenance_error(
        &unsorted,
        limits,
        ValidationCode::NonCanonical,
        "provenance",
    )?;

    // Both leaves sort before the shared tail, exercising a previously completed
    // ancestor whose own metadata is not checked until its later outer turn.
    let mut later = provenance_forest(
        &[
            ("a-leaf", Some("z-tail")),
            ("b-leaf", Some("z-tail")),
            ("z-root", None),
            ("z-tail", Some("z-root")),
        ],
        &["a-leaf", "b-leaf"],
    )?;
    assert_eq!(validate(&later, limits), Ok(()));
    later
        .provenance
        .last_mut()
        .ok_or("tail")?
        .message_key
        .clear();
    assert_provenance_error(
        &later,
        limits,
        ValidationCode::LimitExceeded,
        "provenance.messageKey",
    )?;
    later.provenance[1].source_id.clear();
    assert_provenance_error(
        &later,
        limits,
        ValidationCode::LimitExceeded,
        "provenance.sourceId",
    )?;
    Ok(())
}

#[test]
fn provenance_cycles_missing_parents_and_zero_depth_preserve_precedence()
-> Result<(), Box<dyn Error>> {
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    let self_cycle = provenance_forest(&[("a", Some("a"))], &["a"])?;
    limits.max_provenance_depth = 1;
    assert_provenance_error(
        &self_cycle,
        limits,
        ValidationCode::ProvenanceCycle,
        "provenance.parent",
    )?;
    let cycle = provenance_forest(&[("a", Some("b")), ("b", Some("a"))], &["a"])?;
    limits.max_provenance_depth = 2;
    assert_provenance_error(
        &cycle,
        limits,
        ValidationCode::ProvenanceCycle,
        "provenance.parent",
    )?;
    limits.max_provenance_depth = 1;
    assert_provenance_error(
        &cycle,
        limits,
        ValidationCode::LimitExceeded,
        "provenance.depth",
    )?;
    let missing = provenance_forest(&[("a", Some("missing"))], &["a"])?;
    assert_provenance_error(
        &missing,
        limits,
        ValidationCode::LimitExceeded,
        "provenance.depth",
    )?;
    limits.max_provenance_depth = 2;
    assert_provenance_error(
        &missing,
        limits,
        ValidationCode::MissingReference,
        "provenance.parent",
    )?;
    let root = provenance_forest(&[("a", None)], &["a"])?;
    limits.max_provenance_depth = 1;
    assert_eq!(validate(&root, limits), Ok(()));
    limits.max_provenance_depth = 0;
    assert_provenance_error(
        &root,
        limits,
        ValidationCode::LimitExceeded,
        "provenance.depth",
    )?;
    let empty = provenance_forest(&[], &[])?;
    assert_eq!(validate(&empty, limits), Ok(()));
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    drop(validate_with_budget(&empty, limits, &mut budget)?);
    Ok(())
}

#[test]
fn provenance_memo_storage_is_admitted_and_released_on_failure() -> Result<(), Box<dyn Error>> {
    let problem = provenance_forest(&[("a-root", None), ("b-leaf", Some("a-root"))], &["b-leaf"])?;
    let limits = PlanningIrLimitsV1 {
        max_ir_bytes: 1_000_000,
        max_provenance_depth: 2,
        ..PlanningIrLimitsV1::DEFAULT
    };
    let scratch = 4096 + 128 * u64::try_from(problem.variables.len() + problem.provenance.len())?;
    let memo = 2 * u64::try_from(size_of::<Option<u64>>() + size_of::<usize>())?;
    let preexisting = TRANSIENT_TEST_CEILING - (scratch + memo - 1);
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    budget.reserve_transient(preexisting)?;
    assert!(matches!(
        validate_with_budget(&problem, limits, &mut budget),
        Err(BudgetedValidationError::Preparation(
            PlanningPreparationError::TransientLimitExceeded
        ))
    ));
    budget.reserve_transient(TRANSIENT_TEST_CEILING - preexisting)?;
    budget.release_transient(TRANSIENT_TEST_CEILING - preexisting)?;
    budget.release_transient(preexisting)?;
    drop(validate_with_budget(&problem, limits, &mut budget)?);
    budget.reserve_transient(TRANSIENT_TEST_CEILING)?;
    budget.release_transient(TRANSIENT_TEST_CEILING)?;
    Ok(())
}

#[test]
fn assumption_group_accepts_multiple_rules_and_counts_one_literal() -> Result<(), Box<dyn Error>> {
    let provenance = ProvenanceId::new("provenance.required-rule")?;
    let mut problem = assumption_problem(vec![assumption(
        "multi",
        Literal::positive(bool_id("a")?),
        vec![rule_id(2)?, rule_id(1)?],
        provenance,
    )?])?;

    assert_eq!(
        problem.assumptions[0].required_rules,
        vec![rule_id(1)?, rule_id(2)?]
    );
    assert_eq!(
        feature_usage(&problem).literal_reference_count,
        feature_usage(&base_problem()?).literal_reference_count + 1
    );
    validate(&problem, PlanningIrLimitsV1::DEFAULT)?;

    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_refs_per_node = 2;
    assert_eq!(
        validate(&problem, limits),
        Err(ValidationError {
            code: ValidationCode::LimitExceeded,
            path: "assumptions[0].references".to_owned(),
        })
    );
    problem.assumptions[0].required_rules.pop();
    assert!(validate(&problem, PlanningIrLimitsV1::DEFAULT).is_ok());
    Ok(())
}

#[test]
fn assumption_rule_order_is_canonical_and_hash_stable() -> Result<(), Box<dyn Error>> {
    let provenance = ProvenanceId::new("provenance.required-rule")?;
    let first = assumption_problem(vec![assumption(
        "stable",
        Literal::positive(bool_id("a")?),
        vec![rule_id(1)?, rule_id(2)?],
        provenance.clone(),
    )?])?;
    let second = assumption_problem(vec![assumption(
        "stable",
        Literal::positive(bool_id("a")?),
        vec![rule_id(2)?, rule_id(1)?],
        provenance,
    )?])?;

    assert_eq!(first.assumptions, second.assumptions);
    assert_eq!(
        canonical_ir_hash(&first, PlanningIrLimitsV1::DEFAULT)?,
        canonical_ir_hash(&second, PlanningIrLimitsV1::DEFAULT)?
    );

    let json = serde_json::to_vec(&first)?;
    let round_trip: PlanningProblem = serde_json::from_slice(&json)?;
    assert_eq!(
        round_trip.assumptions[0].id,
        AssumptionGroupId::new("assumption.stable")?
    );
    assert_eq!(
        canonical_ir_hash(&round_trip, PlanningIrLimitsV1::DEFAULT)?,
        canonical_ir_hash(&first, PlanningIrLimitsV1::DEFAULT)?
    );

    let mut renamed = round_trip;
    renamed.assumptions[0].id = AssumptionGroupId::new("assumption.renamed")?;
    assert_ne!(
        canonical_ir_hash(&renamed, PlanningIrLimitsV1::DEFAULT)?,
        canonical_ir_hash(&first, PlanningIrLimitsV1::DEFAULT)?
    );
    Ok(())
}

#[test]
fn assumption_group_rejects_empty_and_duplicate_rule_lists() -> Result<(), Box<dyn Error>> {
    let provenance = ProvenanceId::new("provenance.required-rule")?;
    let empty = assumption_problem(vec![assumption(
        "empty",
        Literal::positive(bool_id("a")?),
        Vec::new(),
        provenance.clone(),
    )?])?;
    assert_eq!(
        validate(&empty, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::InvalidShape,
            path: "assumptions[0].requiredRules".to_owned(),
        })
    );

    let duplicate = assumption_problem(vec![assumption(
        "duplicate",
        Literal::positive(bool_id("a")?),
        vec![rule_id(1)?, rule_id(1)?],
        provenance,
    )?])?;
    assert_eq!(
        validate(&duplicate, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::NonCanonical,
            path: "assumptions[0].requiredRules".to_owned(),
        })
    );
    Ok(())
}

#[test]
fn assumption_groups_reject_reused_rule_and_literal_variable() -> Result<(), Box<dyn Error>> {
    let provenance = ProvenanceId::new("provenance.required-rule")?;
    let reused_rule = assumption_problem(vec![
        assumption(
            "a",
            Literal::positive(bool_id("a")?),
            vec![rule_id(1)?],
            provenance.clone(),
        )?,
        assumption(
            "b",
            Literal::positive(bool_id("b")?),
            vec![rule_id(1)?],
            provenance.clone(),
        )?,
    ])?;
    assert_eq!(
        validate(&reused_rule, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::DuplicateId,
            path: "assumptions[1].requiredRules[0]".to_owned(),
        })
    );

    let reused_variable = assumption_problem(vec![
        assumption(
            "a",
            Literal::positive(bool_id("a")?),
            vec![rule_id(1)?],
            provenance.clone(),
        )?,
        assumption(
            "b",
            Literal::negative(bool_id("a")?),
            vec![rule_id(2)?],
            provenance,
        )?,
    ])?;
    assert_eq!(
        validate(&reused_variable, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::DuplicateId,
            path: "assumptions[1].literal.variable".to_owned(),
        })
    );
    Ok(())
}

#[test]
fn assumption_group_requires_existing_literal_and_provenance() -> Result<(), Box<dyn Error>> {
    let provenance = ProvenanceId::new("provenance.required-rule")?;
    let valid = assumption_problem(vec![assumption(
        "references",
        Literal::positive(bool_id("a")?),
        vec![rule_id(1)?],
        provenance,
    )?])?;
    let mut missing_literal = valid.clone();
    missing_literal.assumptions[0].literal = Literal::positive(bool_id("missing")?);
    assert_eq!(
        validate(&missing_literal, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::MissingReference,
            path: "assumptions[0].literal".to_owned(),
        })
    );

    let mut missing_provenance = valid;
    missing_provenance.assumptions[0].provenance = ProvenanceId::new("provenance.missing")?;
    assert_eq!(
        validate(&missing_provenance, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::MissingProvenance,
            path: "assumptions[0].provenance".to_owned(),
        })
    );
    Ok(())
}

#[test]
fn assumption_group_rejects_non_required_rule_provenance() -> Result<(), Box<dyn Error>> {
    let mut problem = base_problem()?;
    problem.assumptions.push(assumption(
        "wrong-provenance",
        Literal::positive(bool_id("a")?),
        vec![rule_id(1)?],
        provenance_id()?,
    )?);
    problem.canonicalize()?;

    assert_eq!(
        validate(&problem, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::InvalidShape,
            path: "assumptions[0].provenance".to_owned(),
        })
    );
    Ok(())
}

#[test]
fn serialization_hash_is_canonical_context_sensitive_and_display_independent()
-> Result<(), Box<dyn Error>> {
    let mut first = base_problem()?;
    first.constraints = vec![
        record("b", Constraint::bool_and(Vec::new()))?,
        record("a", Constraint::bool_or(Vec::new()))?,
    ];
    let mut second = first.clone();
    second.constraints.reverse();
    first.canonicalize()?;
    second.canonicalize()?;
    let first_json = canonical_json(&first, PlanningIrLimitsV1::DEFAULT)?;
    let second_json = canonical_json(&second, PlanningIrLimitsV1::DEFAULT)?;
    assert_eq!(first_json, second_json);
    let hash = canonical_ir_hash(&first, PlanningIrLimitsV1::DEFAULT)?;
    second
        .metadata
        .display_text
        .insert("title".to_owned(), "changed".to_owned());
    assert_eq!(
        hash,
        canonical_ir_hash(&second, PlanningIrLimitsV1::DEFAULT)?
    );
    second.metadata.compiler_version = "1.0.1".to_owned();
    assert_ne!(
        hash,
        canonical_ir_hash(&second, PlanningIrLimitsV1::DEFAULT)?
    );
    second.metadata.compiler_version = "1.0.0".to_owned();
    second.metadata.compile_metadata.insert(
        MetadataKey::new("compile.seed")?,
        ProvenanceParameter::Integer(1),
    );
    assert_ne!(
        hash,
        canonical_ir_hash(&second, PlanningIrLimitsV1::DEFAULT)?
    );
    Ok(())
}

#[test]
fn objective_policy_hash_binds_priority_but_not_unrelated_model_structure()
-> Result<(), Box<dyn Error>> {
    let mut problem = base_problem()?;
    for name in ["x", "y"] {
        problem.objectives.levels.push(ObjectiveLevel {
            id: ObjectiveLevelId::new(format!("level.{name}"))?,
            direction: OptimizationDirection::Minimize,
            lower_bound: 0,
            upper_bound: 20,
            terms: vec![ObjectiveTerm {
                id: ObjectiveTermId::new(format!("objective.{name}"))?,
                expression: LinearExpression::new(
                    vec![LinearTerm {
                        variable: int_id(name)?,
                        coefficient: 1,
                    }],
                    10,
                )?,
                kind: ObjectiveTermKind::Penalty,
                category: ScoreCategoryId::new("score.preference")?,
                provenance: provenance_id()?,
            }],
            provenance: provenance_id()?,
        });
    }
    problem.canonicalize()?;
    let hash = canonical_objective_policy_hash(&problem, PlanningIrLimitsV1::DEFAULT)?;
    problem
        .constraints
        .push(record("extra", Constraint::bool_and(Vec::new()))?);
    problem.metadata.scenario_revision += 1;
    problem.canonicalize()?;
    assert_eq!(
        hash,
        canonical_objective_policy_hash(&problem, PlanningIrLimitsV1::DEFAULT)?
    );
    problem.objectives.levels.reverse();
    assert_ne!(
        hash,
        canonical_objective_policy_hash(&problem, PlanningIrLimitsV1::DEFAULT)?
    );
    problem.objectives.levels.reverse();
    problem.objectives.levels[0].terms[0].expression.terms[0].coefficient = 0;
    assert!(canonical_objective_policy_hash(&problem, PlanningIrLimitsV1::DEFAULT).is_err());
    problem.objectives.levels[0].terms[0].expression.terms[0].coefficient = 1;
    problem.objectives.levels[0].direction = OptimizationDirection::Maximize;
    assert_ne!(
        hash,
        canonical_objective_policy_hash(&problem, PlanningIrLimitsV1::DEFAULT)?
    );
    Ok(())
}

#[test]
fn strict_parser_rejects_unsupported_versions_and_field() -> Result<(), Box<dyn Error>> {
    let problem = base_problem()?;
    let mut value = serde_json::to_value(&problem)?;
    if let Some(object) = value.as_object_mut() {
        object.insert("future".to_owned(), serde_json::Value::Bool(true));
    }
    assert!(matches!(
        parse_and_validate(&serde_json::to_vec(&value)?, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::MalformedJson,
            ..
        })
    ));
    for schema_version in [1, PLANNING_IR_SCHEMA_VERSION + 1] {
        let mut unsupported = problem.clone();
        unsupported.schema_version = schema_version;
        assert!(matches!(
            parse_and_validate(
                &serde_json::to_vec(&unsupported)?,
                PlanningIrLimitsV1::DEFAULT
            ),
            Err(ValidationError {
                code: ValidationCode::UnsupportedVersion,
                ..
            })
        ));
    }
    let tiny = PlanningIrLimitsV1 {
        max_ir_bytes: 1,
        ..PlanningIrLimitsV1::DEFAULT
    };
    assert!(matches!(
        parse_and_validate(b"{}", tiny),
        Err(ValidationError {
            code: ValidationCode::LimitExceeded,
            ..
        })
    ));
    Ok(())
}

// One evolving model demonstrates every component-joining source and binds the resulting hash.
#[allow(clippy::too_many_lines)]
#[test]
fn components_are_joined_by_objective_projection_and_split_hash_is_bound()
-> Result<(), Box<dyn Error>> {
    let mut problem = base_problem()?;
    let x = int_id("x")?;
    let y = int_id("y")?;
    let z = int_id("z")?;
    problem.variables.retain(
        |variable| {
            matches!(variable, Variable::Integer(value) if value.id == x || value.id == y || value.id == z)
        },
    );
    problem.canonicalize()?;
    let independent = analyze_components(&problem);
    assert_eq!(independent.components.len(), 3);
    problem.constraints = vec![record(
        "min-connects-target",
        Constraint::Min {
            target: z.clone(),
            inputs: vec![x.clone(), y.clone()],
        },
    )?];
    problem.canonicalize()?;
    assert_eq!(analyze_components(&problem).components.len(), 1);
    problem.constraints = vec![record(
        "max-connects-target",
        Constraint::Max {
            target: z,
            inputs: vec![x.clone(), y.clone()],
        },
    )?];
    problem.canonicalize()?;
    assert_eq!(analyze_components(&problem).components.len(), 1);
    problem.constraints.clear();
    problem.variables.retain(
        |variable| matches!(variable, Variable::Integer(value) if value.id == x || value.id == y),
    );
    problem.objectives.levels = vec![ObjectiveLevel {
        id: ObjectiveLevelId::new("level.shared")?,
        direction: OptimizationDirection::Minimize,
        lower_bound: 0,
        upper_bound: 40,
        terms: ["x", "y"]
            .into_iter()
            .map(|name| {
                Ok(ObjectiveTerm {
                    id: ObjectiveTermId::new(format!("objective.{name}"))?,
                    expression: LinearExpression::new(
                        vec![LinearTerm {
                            variable: int_id(name)?,
                            coefficient: 1,
                        }],
                        10,
                    )?,
                    kind: ObjectiveTermKind::Penalty,
                    category: ScoreCategoryId::new("score.shared")?,
                    provenance: provenance_id()?,
                })
            })
            .collect::<Result<_, Box<dyn Error>>>()?,
        provenance: provenance_id()?,
    }];
    problem.canonicalize()?;
    assert_eq!(analyze_components(&problem).components.len(), 1);
    let mut budget = PlanningPreparationBudget::uncontrolled(PlanningIrLimitsV1::DEFAULT);
    let objective_joined = analyze_components_with_budget(&problem, &mut budget)?;
    assert_eq!(objective_joined.analysis(), &analyze_components(&problem));
    assert_eq!(objective_joined.analysis().edge_count, 1);
    drop(objective_joined);
    problem.objectives.levels.clear();
    problem.projections = vec![SolutionProjection {
        id: ProjectionId::new("projection.shared")?,
        assignment_id: DomainAssignmentId::new("assignment.shared")?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new("school.summary")?,
            id: DomainEntityId::new("school.summary")?,
        },
        required: true,
        expression: ProjectionExpression::Linear(LinearExpression::new(
            vec![
                LinearTerm {
                    variable: int_id("x")?,
                    coefficient: 1,
                },
                LinearTerm {
                    variable: int_id("y")?,
                    coefficient: 1,
                },
            ],
            0,
        )?),
        provenance: provenance_id()?,
    }];
    problem.canonicalize()?;
    let joined = analyze_components(&problem);
    assert_eq!(joined.components.len(), 1);
    let projection_joined = analyze_components_with_budget(&problem, &mut budget)?;
    assert_eq!(projection_joined.analysis(), &joined);
    assert_eq!(projection_joined.analysis().edge_count, 1);
    drop(projection_joined);
    problem.split_authorization = Some(SplitAuthorization {
        component_hash: "wrong".to_owned(),
        domain_merge_contract: "school.merge-v1".to_owned(),
        projection_independent: true,
    });
    assert!(matches!(
        validate(&problem, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::InvalidSplitAuthorization,
            ..
        })
    ));
    problem.split_authorization = Some(SplitAuthorization {
        component_hash: joined.component_hash,
        domain_merge_contract: "school.merge-v1".to_owned(),
        projection_independent: true,
    });
    assert!(validate(&problem, PlanningIrLimitsV1::DEFAULT).is_ok());
    Ok(())
}

proptest! {
    #[test]
    fn normalized_domain_represents_exact_values(mut points in proptest::collection::vec(-100_i64..100, 1..40)) {
        points.sort_unstable();
        points.dedup();
        let ranges: Vec<_> = points.iter().map(|point| InclusiveRange { start: *point, end: *point }).collect();
        let Ok(domain) = IntDomain::new(ranges) else { return Ok(()); };
        for value in -100_i64..100 {
            prop_assert_eq!(domain.contains(value), points.binary_search(&value).is_ok());
        }
    }
}

#[test]
fn component_analysis_rejects_exhausted_transient_scratch() -> Result<(), Box<dyn Error>> {
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_ir_bytes = 1;
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    budget.reserve_transient(64 * 1024 * 1024)?;
    assert!(matches!(
        analyze_components_with_budget(&base_problem()?, &mut budget),
        Err(PlanningPreparationError::TransientLimitExceeded)
    ));
    Ok(())
}

#[test]
fn component_analysis_honors_preexisting_cancellation() -> Result<(), Box<dyn Error>> {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut budget = PlanningPreparationBudget::new(
        OperationControl::Cancellation(cancellation),
        PlanningIrLimitsV1::DEFAULT,
    );
    let problem = base_problem()?;
    assert!(matches!(
        analyze_components_with_budget(&problem, &mut budget),
        Err(PlanningPreparationError::Cancelled)
    ));
    Ok(())
}

fn one_boolean_problem() -> Result<PlanningProblem, Box<dyn Error>> {
    let mut problem = base_problem()?;
    let boolean = bool_id("a")?;
    problem
        .variables
        .retain(|variable| matches!(variable, Variable::Boolean(value) if value.id == boolean));
    Ok(problem)
}

#[test]
fn duplicate_typed_declarations_keep_the_historical_component_hash() -> Result<(), Box<dyn Error>> {
    let one = one_boolean_problem()?;
    let expected_node = "bool:bool.a";
    let mut hasher = blake3::Hasher::new();
    hasher.update(&0_u64.to_le_bytes());
    hasher.update(&1_u64.to_le_bytes());
    hasher.update(&u64::try_from(expected_node.len())?.to_le_bytes());
    hasher.update(expected_node.as_bytes());
    let expected = ComponentAnalysis {
        components: vec![MathematicalComponent {
            id: ComponentId::new("component.c0")?,
            variable_nodes: vec![expected_node.to_owned()],
        }],
        edge_count: 0,
        component_hash: hasher.finalize().to_hex().to_string(),
    };
    assert_eq!(analyze_components(&one), expected);

    let mut duplicate = one.clone();
    duplicate.variables.push(duplicate.variables[0].clone());
    assert_eq!(analyze_components(&duplicate), expected);

    let mut connected = base_problem()?;
    let connected_expected = analyze_components(&connected);
    let boolean_b = connected
        .variables
        .iter()
        .find(|variable| {
            matches!(variable, Variable::Boolean(value) if value.id.as_str() == "bool.b")
        })
        .ok_or("base fixture is missing bool.b")?
        .clone();
    connected.variables.push(boolean_b);
    assert_eq!(analyze_components(&connected), connected_expected);
    connected.variables.reverse();
    assert_eq!(analyze_components(&connected), connected_expected);

    let ordinary = summarize(&base_problem()?, PlanningIrLimitsV1::DEFAULT)?;
    let mut budget = PlanningPreparationBudget::uncontrolled(PlanningIrLimitsV1::DEFAULT);
    let controlled =
        summarize_with_budget(&base_problem()?, PlanningIrLimitsV1::DEFAULT, &mut budget)?;
    assert_eq!(controlled.summary(), &ordinary);
    assert_eq!(
        serde_json::to_vec(controlled.summary())?,
        serde_json::to_vec(&ordinary)?
    );
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn borrowed_component_nodes_preserve_typed_membership_edges_hash_and_owned_output()
-> Result<(), Box<dyn Error>> {
    let mut problem = base_problem()?;
    problem.variables.clear();
    for name in [
        "node.shared",
        "node.peer",
        "node.enforced",
        "node.presence",
        "node.single",
    ] {
        problem.variables.push(Variable::Boolean(BoolVariable {
            id: BoolVariableId::new(name)?,
            provenance: provenance_id()?,
        }));
    }
    let shared_int = IntVariableId::new("node.shared")?;
    problem.variables.push(Variable::Integer(IntVariable {
        id: shared_int.clone(),
        domain: domain(0, 1)?,
        provenance: provenance_id()?,
    }));
    problem.variables.push(Variable::Interval(IntervalVariable {
        id: IntervalVariableId::new("node.shared")?,
        start: shared_int.clone(),
        duration: shared_int.clone(),
        end: shared_int,
        presence: Some(Literal::positive(BoolVariableId::new("node.presence")?)),
        provenance: provenance_id()?,
    }));
    // Direct component APIs historically accept duplicate/malformed declarations and
    // unknown references; validation remains the separate authority for model shape.
    problem.variables.push(problem.variables[0].clone());
    let repeated = vec![
        Literal::positive(BoolVariableId::new("node.shared")?),
        Literal::positive(BoolVariableId::new("node.peer")?),
        Literal::negative(BoolVariableId::new("node.shared")?),
        Literal::positive(BoolVariableId::new("node.unknown")?),
    ];
    for name in ["repeated-first", "repeated-second"] {
        problem.constraints.push(record(
            name,
            Constraint::BoolOr {
                literals: repeated.clone(),
            },
        )?);
    }
    let mut enforced = record(
        "enforced",
        Constraint::BoolOr {
            literals: vec![Literal::positive(BoolVariableId::new("node.shared")?)],
        },
    )?;
    enforced
        .enforcement
        .push(Literal::positive(BoolVariableId::new("node.enforced")?));
    problem.constraints.push(enforced);
    problem.constraints.push(record(
        "empty",
        Constraint::BoolAnd {
            literals: Vec::new(),
        },
    )?);
    problem.constraints.push(record(
        "singleton-with-unknown",
        Constraint::BoolOr {
            literals: vec![
                Literal::positive(BoolVariableId::new("node.single")?),
                Literal::positive(BoolVariableId::new("node.unknown")?),
            ],
        },
    )?);
    problem.assumptions.push(assumption(
        "single",
        Literal::positive(BoolVariableId::new("node.single")?),
        vec![rule_id(1)?],
        provenance_id()?,
    )?);
    let expected_groups = [
        vec!["bool:node.enforced", "bool:node.peer", "bool:node.shared"],
        vec![
            "bool:node.presence",
            "int:node.shared",
            "interval:node.shared",
        ],
        vec!["bool:node.single"],
    ];
    // Interval structure produces two known-node edges; each repeated hyperedge
    // still counts one, and enforcement adds one. Empty/unknown/singleton add none.
    let mut hasher = blake3::Hasher::new();
    hasher.update(&5_u64.to_le_bytes());
    let mut components = Vec::new();
    for (index, group) in expected_groups.into_iter().enumerate() {
        hasher.update(&u64::try_from(group.len())?.to_le_bytes());
        for node in &group {
            hasher.update(&u64::try_from(node.len())?.to_le_bytes());
            hasher.update(node.as_bytes());
        }
        components.push(MathematicalComponent {
            id: ComponentId::new(format!("component.c{index}"))?,
            variable_nodes: group.into_iter().map(str::to_owned).collect(),
        });
    }
    let expected = ComponentAnalysis {
        components,
        edge_count: 5,
        component_hash: hasher.finalize().to_hex().to_string(),
    };
    let mut budget = PlanningPreparationBudget::uncontrolled(PlanningIrLimitsV1::DEFAULT);
    for _ in 0..2 {
        assert_eq!(analyze_components(&problem), expected);
        let controlled = analyze_components_with_budget(&problem, &mut budget)?;
        assert_eq!(controlled.analysis(), &expected);
        problem.variables.reverse();
    }
    let ordinary = analyze_components(&problem);
    let controlled = analyze_components_with_budget(&problem, &mut budget)?;
    drop(problem);
    assert_eq!(ordinary, expected);
    assert_eq!(controlled.analysis(), &expected);
    drop(controlled);

    let mut empty = one_boolean_problem()?;
    empty.variables.clear();
    let expected_empty = ComponentAnalysis {
        components: Vec::new(),
        edge_count: 0,
        component_hash: blake3::hash(&0_u64.to_le_bytes()).to_hex().to_string(),
    };
    assert_eq!(analyze_components(&empty), expected_empty);
    assert_eq!(
        analyze_components_with_budget(&empty, &mut budget)?.analysis(),
        &expected_empty
    );
    Ok(())
}

#[test]
fn component_and_summary_output_stay_charged_until_dropped() -> Result<(), Box<dyn Error>> {
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_ir_bytes = 1_000_000;
    let problem = base_problem()?;

    let mut uncharged = PlanningPreparationBudget::uncontrolled(limits);
    let output = analyze_components_with_budget(&problem, &mut uncharged)?;
    assert!(matches!(
        uncharged.release_transient(1),
        Err(PlanningPreparationError::ArithmeticOverflow)
    ));
    drop(output);
    uncharged.reserve_transient(TRANSIENT_TEST_CEILING)?;
    uncharged.release_transient(TRANSIENT_TEST_CEILING)?;

    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    budget.reserve_transient(32)?;
    let analysis = analyze_components_with_budget(&problem, &mut budget)?;
    assert!(matches!(
        budget.reserve_transient(TRANSIENT_TEST_CEILING - 32),
        Err(PlanningPreparationError::TransientLimitExceeded)
    ));
    budget.release_transient(32)?;
    assert!(matches!(
        budget.reserve_transient(TRANSIENT_TEST_CEILING - 1),
        Err(PlanningPreparationError::TransientLimitExceeded)
    ));
    drop(analysis);
    budget.reserve_transient(TRANSIENT_TEST_CEILING)?;
    budget.release_transient(TRANSIENT_TEST_CEILING)?;

    let first = analyze_components_with_budget(&problem, &mut budget)?;
    let second = analyze_components_with_budget(&problem, &mut budget)?;
    let gate = Arc::new(std::sync::Barrier::new(3));
    let first_gate = Arc::clone(&gate);
    let second_gate = Arc::clone(&gate);
    let first_drop = std::thread::spawn(move || {
        first_gate.wait();
        drop(first);
    });
    let second_drop = std::thread::spawn(move || {
        second_gate.wait();
        drop(second);
    });
    gate.wait();
    first_drop
        .join()
        .map_err(|_| "first output retirement thread panicked")?;
    second_drop
        .join()
        .map_err(|_| "second output retirement thread panicked")?;
    budget.reserve_transient(TRANSIENT_TEST_CEILING)?;
    budget.release_transient(TRANSIENT_TEST_CEILING)?;

    let summary = summarize_with_budget(&problem, limits, &mut budget)?;
    assert!(matches!(
        budget.reserve_transient(TRANSIENT_TEST_CEILING),
        Err(PlanningPreparationError::TransientLimitExceeded)
    ));
    drop(summary);
    budget.reserve_transient(TRANSIENT_TEST_CEILING)?;
    budget.release_transient(TRANSIENT_TEST_CEILING)?;
    Ok(())
}

#[test]
fn retained_singleton_capacity_remains_admitted_until_drop() -> Result<(), Box<dyn Error>> {
    let mut problem = one_boolean_problem()?;
    problem.variables = (0..512)
        .map(|index| {
            Ok(Variable::Boolean(BoolVariable {
                id: BoolVariableId::new(format!("bool.{index:03}.{}", "x".repeat(151)))?,
                provenance: provenance_id()?,
            }))
        })
        .collect::<Result<_, PlanningIdError>>()?;
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_ir_bytes = 1_000_000;
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    let output = analyze_components_with_budget(&problem, &mut budget)?;
    let components = &output.analysis().components;
    let retained_bytes = components.capacity() * size_of::<MathematicalComponent>()
        + components
            .iter()
            .map(|component| {
                component.id.as_str().len()
                    + component.variable_nodes.capacity() * size_of::<String>()
                    + component
                        .variable_nodes
                        .iter()
                        .map(String::capacity)
                        .sum::<usize>()
            })
            .sum::<usize>();
    // This lower bound uses actual retained capacities, excluding allocator overhead
    // and any spare capacity hidden by the typed component ID.
    let overlapping_bytes = TRANSIENT_TEST_CEILING - u64::try_from(retained_bytes)? + 1;
    assert!(matches!(
        budget.reserve_transient(overlapping_bytes),
        Err(PlanningPreparationError::TransientLimitExceeded)
    ));
    drop(output);
    budget.reserve_transient(TRANSIENT_TEST_CEILING)?;
    budget.release_transient(TRANSIENT_TEST_CEILING)?;
    Ok(())
}

#[test]
fn component_analysis_unwinds_declaration_index_and_parent_admission() -> Result<(), Box<dyn Error>>
{
    let problem = one_boolean_problem()?;
    let variable_bytes = u64::try_from(problem.variables[0].canonical_id().len())?;
    let declared = 4096 + 128 + variable_bytes;
    let index = 4096 + 192;
    let parent = 4096 + 2 * u64::try_from(size_of::<usize>())?;
    let needed = declared + index + parent;
    let preexisting = TRANSIENT_TEST_CEILING - (needed - 1);
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_ir_bytes = 1_000_000;
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    budget.reserve_transient(preexisting)?;

    assert!(matches!(
        analyze_components_with_budget(&problem, &mut budget),
        Err(PlanningPreparationError::TransientLimitExceeded)
    ));
    let remaining = TRANSIENT_TEST_CEILING - preexisting;
    budget.reserve_transient(remaining)?;
    budget.release_transient(remaining)?;
    budget.release_transient(preexisting)?;
    drop(analyze_components_with_budget(&problem, &mut budget)?);
    Ok(())
}

#[test]
fn component_analysis_unwinds_edge_and_output_admission() -> Result<(), Box<dyn Error>> {
    let mut problem = one_boolean_problem()?;
    let mut record = record(
        "one-node-edge",
        Constraint::BoolOr {
            literals: vec![
                Literal::positive(bool_id("a")?),
                Literal::positive(bool_id("a")?),
                Literal::positive(bool_id("a")?),
            ],
        },
    )?;
    record.enforcement.push(Literal::positive(bool_id("a")?));
    problem.constraints.push(record);
    let variable_bytes = u64::try_from(problem.variables[0].canonical_id().len())?;
    let declared = 4096 + 128 + variable_bytes;
    let index = 4096 + 192;
    let parent = 4096 + 2 * u64::try_from(size_of::<usize>())?;
    let edge = 4 * 192 + 1024;
    let preexisting = TRANSIENT_TEST_CEILING - (declared + index + parent + 2 * edge - 1);
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_ir_bytes = 1_000_000;
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    budget.reserve_transient(preexisting)?;
    assert!(matches!(
        analyze_components_with_budget(&problem, &mut budget),
        Err(PlanningPreparationError::TransientLimitExceeded)
    ));
    budget.reserve_transient(TRANSIENT_TEST_CEILING - preexisting)?;
    budget.release_transient(TRANSIENT_TEST_CEILING - preexisting)?;
    budget.release_transient(preexisting)?;

    let problem = one_boolean_problem()?;
    let variable_bytes = u64::try_from(problem.variables[0].canonical_id().len())?;
    let declared = 4096 + 128 + variable_bytes;
    let parent = 4096 + 2 * u64::try_from(size_of::<usize>())?;
    let output = 256 + 8192;
    let grouping = 128 + 4096;
    let preexisting = TRANSIENT_TEST_CEILING - (declared + parent + output + grouping - 1);
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    budget.reserve_transient(preexisting)?;
    assert!(matches!(
        analyze_components_with_budget(&problem, &mut budget),
        Err(PlanningPreparationError::TransientLimitExceeded)
    ));
    budget.reserve_transient(TRANSIENT_TEST_CEILING - preexisting)?;
    budget.release_transient(TRANSIENT_TEST_CEILING - preexisting)?;
    budget.release_transient(preexisting)?;
    drop(analyze_components_with_budget(&problem, &mut budget)?);
    Ok(())
}

#[test]
fn validation_failures_retire_scratch_and_component_output() -> Result<(), Box<dyn Error>> {
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_ir_bytes = 1_000_000;
    let mut duplicate = one_boolean_problem()?;
    duplicate.variables.push(duplicate.variables[0].clone());
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    assert!(matches!(
        validate_with_budget(&duplicate, limits, &mut budget),
        Err(BudgetedValidationError::Invalid(ValidationError {
            code: ValidationCode::NonCanonical,
            ..
        }))
    ));
    budget.reserve_transient(TRANSIENT_TEST_CEILING - 1)?;
    budget.release_transient(TRANSIENT_TEST_CEILING - 1)?;

    let mut split = base_problem()?;
    split.split_authorization = Some(SplitAuthorization {
        component_hash: "historically-wrong".to_owned(),
        domain_merge_contract: "school.merge-v1".to_owned(),
        projection_independent: true,
    });
    assert!(matches!(
        validate_with_budget(&split, limits, &mut budget),
        Err(BudgetedValidationError::Invalid(ValidationError {
            code: ValidationCode::InvalidSplitAuthorization,
            ..
        }))
    ));
    budget.reserve_transient(TRANSIENT_TEST_CEILING - 1)?;
    budget.release_transient(TRANSIENT_TEST_CEILING - 1)?;
    Ok(())
}

#[test]
fn controlled_table_shape_admission_precedes_row_ordering() -> Result<(), Box<dyn Error>> {
    let mut problem = base_problem()?;
    let mut descending = vec![0_i64; 4096];
    descending.push(0);
    let mut preceding = vec![0_i64; 4096];
    preceding.push(1);
    problem.constraints.push(record(
        "wide-table",
        Constraint::AllowedTable {
            variables: vec![int_id("duration_a")?],
            rows: vec![preceding, descending],
        },
    )?);

    assert!(matches!(
        validate(&problem, PlanningIrLimitsV1::DEFAULT),
        Err(ValidationError {
            code: ValidationCode::NonCanonical,
            ..
        })
    ));
    let limits = PlanningIrLimitsV1::DEFAULT;
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    assert!(matches!(
        validate_with_budget(&problem, limits, &mut budget),
        Err(BudgetedValidationError::Invalid(ValidationError {
            code: ValidationCode::InvalidShape,
            ..
        }))
    ));
    Ok(())
}

#[test]
fn discarded_hash_and_validation_outputs_do_not_exhaust_transient_capacity()
-> Result<(), Box<dyn Error>> {
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_ir_bytes = 1_000_000;
    let problem = base_problem()?;
    let expected_hash = canonical_ir_hash(&problem, limits)?;
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    for _ in 0..2 {
        assert_eq!(
            canonical_ir_hash_with_budget(&problem, limits, &mut budget)?,
            expected_hash
        );
        let _ = validate_with_budget(&problem, limits, &mut budget)?;
    }
    budget.reserve_transient(TRANSIENT_TEST_CEILING - 1)?;
    budget.release_transient(TRANSIENT_TEST_CEILING - 1)?;
    Ok(())
}

#[test]
fn constant_objective_terms_consume_sizing_and_materialization_work() -> Result<(), Box<dyn Error>>
{
    let constant_terms = |count: usize| -> Result<PlanningProblem, Box<dyn Error>> {
        let mut problem = one_boolean_problem()?;
        problem.variables.clear();
        let mut terms = Vec::with_capacity(count);
        for index in 0..count {
            terms.push(ObjectiveTerm {
                id: ObjectiveTermId::new(format!("objective.constant-{index}"))?,
                expression: LinearExpression::new(Vec::new(), 1)?,
                kind: ObjectiveTermKind::Penalty,
                category: ScoreCategoryId::new("score.constant")?,
                provenance: provenance_id()?,
            });
        }
        problem.objectives.levels = vec![ObjectiveLevel {
            id: ObjectiveLevelId::new("level.constants")?,
            direction: OptimizationDirection::Minimize,
            lower_bound: 0,
            upper_bound: i64::try_from(count)?,
            terms,
            provenance: provenance_id()?,
        }];
        Ok(problem)
    };

    for (term_count, max_total_refs) in [(100, 19), (200, 26)] {
        let problem = constant_terms(term_count)?;
        let mut limits = PlanningIrLimitsV1::DEFAULT;
        limits.max_total_refs = max_total_refs;
        limits.max_ir_bytes = 1_000_000;
        let mut budget = PlanningPreparationBudget::uncontrolled(limits);
        assert!(matches!(
            analyze_components_with_budget(&problem, &mut budget),
            Err(PlanningPreparationError::WorkLimitExceeded)
        ));
        let charged_work = budget.charged_work();
        assert!(charged_work > 0);
        budget.reserve_transient(TRANSIENT_TEST_CEILING)?;
        budget.release_transient(TRANSIENT_TEST_CEILING)?;
        assert!(matches!(
            analyze_components_with_budget(&problem, &mut budget),
            Err(PlanningPreparationError::WorkLimitExceeded)
        ));
        assert_eq!(budget.charged_work(), charged_work);
    }
    Ok(())
}

#[test]
fn validation_sizing_is_admitted_and_inherited_expiry_precedes_it() -> Result<(), Box<dyn Error>> {
    let mut problem = base_problem()?;
    for index in 0..20 {
        problem.constraints.push(record(
            &format!("sizing-{index}"),
            Constraint::BoolAnd {
                literals: Vec::new(),
            },
        )?);
    }
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_total_refs = 1;
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    assert!(matches!(
        validate_with_budget(&problem, limits, &mut budget),
        Err(BudgetedValidationError::Preparation(
            PlanningPreparationError::WorkLimitExceeded
        ))
    ));
    let work = budget.charged_work();
    assert!(work > 0);

    let clock = Arc::new(FixedMonotonicClock::default());
    let parent = ParentSolveBudget::new(
        DurationMillis::new(1)?,
        clock.clone(),
        CancellationToken::new(),
    )?;
    clock.advance(Duration::from_millis(1))?;
    let mut expired =
        PlanningPreparationBudget::new(OperationControl::Solve(parent.phase_view()), limits);
    assert!(matches!(
        validate_with_budget(&problem, limits, &mut expired),
        Err(BudgetedValidationError::Preparation(
            PlanningPreparationError::DeadlineExceeded
        ))
    ));
    assert_eq!(expired.charged_work(), 0);
    Ok(())
}

#[test]
fn prepared_reservations_check_identity_and_retire_with_the_last_thread_owner()
-> Result<(), Box<dyn Error>> {
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_ir_bytes = 1_000_000;
    let mut budget = PlanningPreparationBudget::uncontrolled(limits);
    budget.reserve_transient(17)?;
    let prepared = PreparedPlanningModel::prepare(Arc::new(base_problem()?), limits, &mut budget)?;
    let clone = prepared.clone();
    let outstanding = prepared.clone();
    let mut wrong_budget = PlanningPreparationBudget::uncontrolled(limits);
    wrong_budget.reserve_transient(31)?;
    assert!(matches!(
        prepared.release(&mut wrong_budget),
        Err(PlanningPreparationError::ArithmeticOverflow)
    ));
    assert!(matches!(
        clone.release(&mut budget),
        Err(PlanningPreparationError::ArithmeticOverflow)
    ));
    assert!(matches!(
        budget.reserve_transient(TRANSIENT_TEST_CEILING - 17),
        Err(PlanningPreparationError::TransientLimitExceeded)
    ));
    let wrong_remaining = TRANSIENT_TEST_CEILING - 31;
    wrong_budget.reserve_transient(wrong_remaining)?;
    wrong_budget.release_transient(wrong_remaining)?;
    wrong_budget.release_transient(31)?;

    std::thread::spawn(move || drop(outstanding))
        .join()
        .map_err(|_| "final prepared owner retirement thread panicked")?;
    budget.reserve_transient(TRANSIENT_TEST_CEILING - 17)?;
    budget.release_transient(TRANSIENT_TEST_CEILING - 17)?;
    budget.release_transient(17)?;

    let prepared = PreparedPlanningModel::prepare(Arc::new(base_problem()?), limits, &mut budget)?;
    prepared.release(&mut budget)?;
    budget.reserve_transient(TRANSIENT_TEST_CEILING - 1)?;
    budget.release_transient(TRANSIENT_TEST_CEILING - 1)?;

    let prepared = PreparedPlanningModel::prepare(Arc::new(base_problem()?), limits, &mut budget)?;
    drop(prepared);
    budget.reserve_transient(TRANSIENT_TEST_CEILING - 1)?;
    budget.release_transient(TRANSIENT_TEST_CEILING - 1)?;
    Ok(())
}
