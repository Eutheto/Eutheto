use crate::test_evaluator::evaluate_record;
use crate::*;
use eutheto_domain_ir::{
    AssignmentValue, AssumptionGroupId, DomainAssignmentId, DomainEntityId, DomainEntityKindId,
    DomainEntityRef, DomainEvidenceId, OptimizationDirection, ScoreCategoryId,
};
use eutheto_types::{PackId, RuleId, ScenarioId, SolutionId};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;

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
        terms: vec![ObjectiveTerm {
            id: ObjectiveTermId::new("objective.shared")?,
            expression: LinearExpression::new(
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
                20,
            )?,
            kind: ObjectiveTermKind::Penalty,
            category: ScoreCategoryId::new("score.shared")?,
            provenance: provenance_id()?,
        }],
        provenance: provenance_id()?,
    }];
    problem.canonicalize()?;
    assert_eq!(analyze_components(&problem).components.len(), 1);
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
