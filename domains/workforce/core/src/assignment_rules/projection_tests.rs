use super::*;
use crate::assignment_rules::compile_workforce;
use crate::test_support::{fixture, id};
use eutheto_domain_api::CompileContext;
use eutheto_domain_ir::{DomainAssignmentId, DomainEntityId, DomainEntityKindId};
use eutheto_planning_ir::{BoolVariableId, IntVariableId, validate};
use eutheto_types::{CancellationToken, OverlapPolicy, PackId};
use std::collections::BTreeMap;
use std::error::Error;

type Result<T = (), E = Box<dyn Error>> = std::result::Result<T, E>;

fn model() -> Result<(PlanningProblem, CandidateValues)> {
    let mut document = fixture()?;
    document.settings.overlap_policy = OverlapPolicy::Earlier;
    document.domain.locked_assignments.clear();
    let context = CompileContext {
        scenario_revision: 7,
        semantic_metadata: BTreeMap::new(),
        control: OperationControl::Cancellation(CancellationToken::new()),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    };
    let problem = compile_workforce(&document, &context)?.problem;
    let mut candidate = CandidateValues::default();
    for variable in &problem.variables {
        if let Variable::Boolean(boolean) = variable {
            candidate.booleans.insert(boolean.id.clone(), false);
        }
    }
    Ok((problem, candidate))
}

fn solution_id() -> Result<SolutionId> {
    Ok(id(200).parse()?)
}

fn reject(problem: &PlanningProblem, candidate: &CandidateValues, code: &'static str) -> Result {
    assert_eq!(
        project_workforce_candidate(
            problem,
            candidate,
            solution_id()?,
            PlanningIrLimitsV1::DEFAULT,
            &OperationControl::Cancellation(CancellationToken::new())
        ),
        Err(contract(code)),
    );
    Ok(())
}

fn first_integer(problem: &PlanningProblem) -> Result<IntVariableId> {
    problem
        .variables
        .iter()
        .find_map(|variable| match variable {
            Variable::Integer(integer) => Some(integer.id.clone()),
            Variable::Boolean(_) | Variable::Interval(_) => None,
        })
        .ok_or_else(|| "fixture lacks a rank channel integer".into())
}

#[test]
fn required_false_values_survive_with_original_binding_and_no_auxiliary_values() -> Result {
    let (problem, mut candidate) = model()?;
    let selected_pair = format!("{}.{}", id(1), id(8));
    let projection = problem
        .projections
        .iter()
        .find(|projection| projection.entity.id.as_str() == selected_pair)
        .ok_or("fixture lacks the manual-shift projection")?;
    let ProjectionExpression::Boolean(boolean) = &projection.expression else {
        return Err("fixture projection is not Boolean".into());
    };
    candidate.booleans.insert(boolean.clone(), true);
    let solution_id = solution_id()?;
    let solution = project_workforce_candidate(
        &problem,
        &candidate,
        solution_id,
        PlanningIrLimitsV1::DEFAULT,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_eq!(solution.scenario_id, id(100).parse()?);
    assert_eq!(solution.scenario_revision, 7);
    assert_eq!(solution.solution_id, solution_id);
    let decoded = solution
        .assignments
        .iter()
        .map(decode_workforce_assignment)
        .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
    assert_eq!(
        decoded,
        BTreeMap::from([
            (
                AssignmentPair {
                    person_id: id(1).parse()?,
                    shift_id: id(7).parse()?
                },
                false
            ),
            (
                AssignmentPair {
                    person_id: id(1).parse()?,
                    shift_id: id(8).parse()?
                },
                true
            ),
        ])
    );
    // Workforce checks must not rewrite generic identities, evidence, or canonical order.
    assert_eq!(
        solution,
        project_candidate(
            &problem,
            &candidate,
            solution_id,
            PlanningIrLimitsV1::DEFAULT,
        )?
    );
    Ok(())
}

#[test]
fn missing_required_boolean_is_not_an_unselected_assignment() -> Result {
    let (problem, mut candidate) = model()?;
    candidate
        .booleans
        .pop_first()
        .ok_or("fixture has no decision")?;
    reject(
        &problem,
        &candidate,
        "official.workforce.projection.missing_value",
    )
}

#[test]
fn supplied_unknown_variable_is_rejected_even_when_all_required_values_exist() -> Result {
    let (problem, mut candidate) = model()?;
    candidate
        .booleans
        .insert(BoolVariableId::new("test.unknown")?, false);
    reject(
        &problem,
        &candidate,
        "official.workforce.projection.unknown_candidate",
    )
}

#[test]
fn supplied_auxiliary_integer_must_still_obey_its_domain() -> Result {
    let (problem, mut candidate) = model()?;
    candidate.integers.insert(first_integer(&problem)?, 2);
    reject(
        &problem,
        &candidate,
        "official.workforce.projection.out_of_domain",
    )
}

#[test]
fn every_assignment_is_decoded_not_just_the_first() -> Result {
    let (mut problem, candidate) = model()?;
    let projection = problem
        .projections
        .iter_mut()
        .max_by(|left, right| left.assignment_id.cmp(&right.assignment_id))
        .ok_or("fixture has no projection")?;
    projection.entity.kind = DomainEntityKindId::new("official.other.assignment")?;
    reject(
        &problem,
        &candidate,
        "official.workforce.projection.entity_kind",
    )
}

#[test]
fn assignment_namespace_and_entity_pair_must_agree() -> Result {
    let (problem, candidate) = model()?;
    let mut wrong_namespace = problem.clone();
    wrong_namespace.projections[0].assignment_id = DomainAssignmentId::new(format!(
        "official.other.assignment.{}",
        wrong_namespace.projections[0].entity.id
    ))?;
    reject(
        &wrong_namespace,
        &candidate,
        "official.workforce.projection.assignment_id",
    )?;
    let mut mismatch = problem;
    mismatch.projections[0].entity.id = DomainEntityId::new(format!("{}.{}", id(2), id(8)))?;
    reject(
        &mismatch,
        &candidate,
        "official.workforce.projection.entity_mismatch",
    )
}

#[test]
fn pairs_require_exact_canonical_typed_uuidv7_encoding() -> Result {
    let (problem, candidate) = model()?;
    let person = id(1);
    let shift = id(7);
    // Domain IDs already reject uppercase and missing namespace separators. These cases
    // reach projection and distinguish permissive spelling, untyped UUIDs, and pair splitting.
    // Both identity references agree in every malformed case.
    let malformed = [
        format!("{}.{}", person.replacen('a', "g", 1), shift),
        format!("{}.{}", person, shift.replace('-', "")),
        format!("{}.{}", person.replacen("-7000-", "-4000-", 1), shift),
        format!("{}.{}", person, shift.replacen("-7000-", "-4000-", 1)),
        format!("{person}.{shift}.extra"),
    ];
    for pair in malformed {
        let mut malformed = problem.clone();
        malformed.projections[0].assignment_id =
            DomainAssignmentId::new(format!("{ASSIGNMENT_KIND}.{pair}"))?;
        malformed.projections[0].entity.id = DomainEntityId::new(pair)?;
        reject(&malformed, &candidate, "official.workforce.projection.pair")?;
    }
    Ok(())
}

#[test]
fn projected_integer_cannot_masquerade_as_boolean_selection() -> Result {
    let (mut problem, mut candidate) = model()?;
    let integer = first_integer(&problem)?;
    candidate.integers.insert(integer.clone(), 0);
    problem.projections[0].expression = ProjectionExpression::Integer(integer);
    problem.canonicalize()?;
    validate(&problem, PlanningIrLimitsV1::DEFAULT)?;
    reject(
        &problem,
        &candidate,
        "official.workforce.projection.value_kind",
    )
}

#[test]
fn optional_absence_cannot_escape_as_an_unselected_pair() -> Result {
    let (mut problem, mut candidate) = model()?;
    let projection = &mut problem.projections[0];
    projection.required = false;
    let ProjectionExpression::Boolean(boolean) = &projection.expression else {
        return Err("fixture projection is not Boolean".into());
    };
    candidate.booleans.remove(boolean);
    reject(
        &problem,
        &candidate,
        "official.workforce.projection.value_kind",
    )
}

#[test]
fn foreign_pack_and_projection_version_fail_before_candidate_processing() -> Result {
    let (problem, _) = model()?;
    let empty = CandidateValues::default();
    let mut foreign = problem.clone();
    foreign.metadata.pack_id = PackId::new("official.other")?;
    reject(&foreign, &empty, "official.workforce.projection.pack")?;
    let mut future = problem;
    future.metadata.projection_version = 2;
    reject(&future, &empty, "official.workforce.projection.version")
}

#[test]
fn aggregate_metadata_bytes_are_bounded_before_generic_indexes() -> Result {
    let (mut problem, candidate) = model()?;
    let baseline_bytes = u64::try_from(serde_json::to_vec(&problem)?.len())?;
    for index in 0..8 {
        problem
            .metadata
            .display_text
            .insert(format!("test.metadata.{index}"), "x".repeat(4096));
    }
    let limits = PlanningIrLimitsV1 {
        max_ir_bytes: baseline_bytes + 16 * 1024,
        ..PlanningIrLimitsV1::DEFAULT
    };
    // Each field is allowed and the model is otherwise valid: generic validation alone
    // deliberately does not enforce aggregate serialized bytes.
    validate(&problem, limits)?;
    assert_eq!(
        project_workforce_candidate(
            &problem,
            &candidate,
            solution_id()?,
            limits,
            &OperationControl::Cancellation(CancellationToken::new())
        ),
        Err(contract("official.workforce.limit.bytes"))
    );
    // Exhausting index/reference quota as well must not move it ahead of the byte gate.
    let no_indexes = PlanningIrLimitsV1 {
        max_total_refs: 0,
        ..limits
    };
    assert_eq!(
        project_workforce_candidate(
            &problem,
            &candidate,
            solution_id()?,
            no_indexes,
            &OperationControl::Cancellation(CancellationToken::new())
        ),
        Err(contract("official.workforce.limit.bytes"))
    );
    Ok(())
}

#[test]
fn generic_scratch_exhaustion_returns_no_partial_solution_and_does_not_poison_retry() -> Result {
    let (problem, candidate) = model()?;
    let limits = PlanningIrLimitsV1 {
        max_total_refs: 64,
        ..PlanningIrLimitsV1::DEFAULT
    };
    // The source model fits, but cumulative generic validation/projection scratch does not.
    validate(&problem, limits)?;
    assert_eq!(
        project_workforce_candidate(
            &problem,
            &candidate,
            solution_id()?,
            limits,
            &OperationControl::Cancellation(CancellationToken::new())
        ),
        Err(contract("official.workforce.limit.references"))
    );
    let solution = project_workforce_candidate(
        &problem,
        &candidate,
        solution_id()?,
        PlanningIrLimitsV1::DEFAULT,
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let values = solution
        .assignments
        .iter()
        .map(decode_workforce_assignment)
        .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
    assert_eq!(
        values,
        BTreeMap::from([
            (
                AssignmentPair {
                    person_id: id(1).parse()?,
                    shift_id: id(7).parse()?
                },
                false
            ),
            (
                AssignmentPair {
                    person_id: id(1).parse()?,
                    shift_id: id(8).parse()?
                },
                false
            ),
        ])
    );
    Ok(())
}

#[test]
fn caller_cannot_expand_supported_generic_limits() -> Result {
    let (mut problem, candidate) = model()?;
    problem.metadata.compiler_version = "x".repeat(4097);
    let limits = PlanningIrLimitsV1 {
        max_metadata_text_bytes: 8192,
        ..PlanningIrLimitsV1::DEFAULT
    };
    validate(&problem, limits)?;
    assert_eq!(
        project_workforce_candidate(
            &problem,
            &candidate,
            solution_id()?,
            limits,
            &OperationControl::Cancellation(CancellationToken::new())
        ),
        Err(contract("official.workforce.projection.invalid_problem"))
    );
    Ok(())
}

#[test]
fn borrowed_text_gate_covers_every_public_unbounded_string_before_serialization() -> Result {
    use eutheto_planning_ir::{MetadataKey, ProvenanceParameter, SplitAuthorization};
    let (problem, _) = model()?;
    let defaults = PlanningIrLimitsV1::DEFAULT;
    let baseline = precharge_text(
        &problem,
        defaults,
        &mut OperationBudget::analysis(None, defaults),
    )?;
    let mut spent = OperationBudget::analysis(None, defaults);
    spent.steps(super::super::budget::MAX_WORK_STEPS - baseline + 1)?;
    assert_eq!(
        precharge_text(&problem, defaults, &mut spent),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::WorkSteps
        ))
    );
    let limits = PlanningIrLimitsV1 {
        max_ir_bytes: baseline + 1,
        ..defaults
    };
    for field in 0..10 {
        let mut malformed = problem.clone();
        let text = "x".repeat(usize::try_from(baseline + 2)?);
        match field {
            0 => malformed.metadata.compiler_version = text,
            1 => malformed.provenance[0].source_id = text,
            2 => malformed.provenance[0].message_key = text,
            3 => {
                malformed.provenance[0]
                    .parameters
                    .insert(text, ProvenanceParameter::Boolean(false));
            }
            4 => {
                malformed.provenance[0]
                    .parameters
                    .insert("text".to_owned(), ProvenanceParameter::Text(text));
            }
            5 => {
                malformed.metadata.compile_metadata.insert(
                    MetadataKey::new("test.text")?,
                    ProvenanceParameter::Text(text),
                );
            }
            6 => {
                malformed.metadata.display_text.insert(text, String::new());
            }
            7 => {
                malformed
                    .metadata
                    .display_text
                    .insert("text".to_owned(), text);
            }
            _ => {
                malformed.split_authorization = Some(SplitAuthorization {
                    component_hash: if field == 8 {
                        text.clone()
                    } else {
                        String::new()
                    },
                    domain_merge_contract: if field == 9 { text } else { String::new() },
                    projection_independent: false,
                });
            }
        }
        assert_eq!(
            precharge_text(
                &malformed,
                limits,
                &mut OperationBudget::analysis(None, limits)
            ),
            Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::Bytes
            ))
        );
    }
    Ok(())
}

#[test]
fn running_deadline_interrupts_projection_validation_and_pack_compilation_preflight() -> Result {
    use eutheto_domain_api::DomainPack;
    use eutheto_types::{DurationMillis, MonotonicClock, ParentSolveBudget};
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::Duration;
    struct AdvancingClock(AtomicU64);
    impl MonotonicClock for AdvancingClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.fetch_add(1, Ordering::Relaxed))
        }
    }
    fn control() -> Result<OperationControl> {
        let parent = ParentSolveBudget::new(
            DurationMillis::new(3)?,
            Arc::new(AdvancingClock(AtomicU64::new(0))),
            CancellationToken::new(),
        )?;
        let control = OperationControl::Solve(parent.phase_view());
        assert_eq!(control.check(), Ok(()));
        Ok(control)
    }
    let (problem, candidate) = model()?;
    assert_eq!(
        project_workforce_candidate(
            &problem,
            &candidate,
            solution_id()?,
            PlanningIrLimitsV1::DEFAULT,
            &control()?
        ),
        Err(DomainPackError::BudgetExpired),
    );
    let mut document = fixture()?;
    assert_eq!(
        crate::WorkforcePack.validate_full(&document, &control()?),
        Err(DomainPackError::BudgetExpired),
    );
    // An uncontrolled preflight would return this bounds error before the compiler's next
    // deadline check. Interruption must be observed inside the pack-facade traversal instead.
    document.metadata.title =
        "x".repeat(eutheto_domain_api::ContractJsonLimits::DEFAULT.max_string_bytes + 1);
    let context = CompileContext {
        scenario_revision: 1,
        semantic_metadata: BTreeMap::new(),
        control: control()?,
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    };
    assert_eq!(
        crate::WorkforcePack.compile(&document, &context),
        Err(DomainPackError::BudgetExpired),
    );
    Ok(())
}
