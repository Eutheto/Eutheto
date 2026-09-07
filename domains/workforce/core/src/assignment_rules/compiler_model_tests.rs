use super::*;
use crate::test_support::{fixture, id};
use eutheto_types::{CancellationToken, OverlapPolicy};
use serde_json::json;
use std::error::Error;

type Result<T = (), E = Box<dyn Error>> = std::result::Result<T, E>;

fn source() -> Result<ScenarioDocument> {
    let mut document = fixture()?;
    document.settings.overlap_policy = OverlapPolicy::Earlier;
    document.domain.locked_assignments.clear();
    for (number, kind) in [(20, "eligibility"), (21, "coverage")] {
        document.domain.rules.insert(
            id(number).parse()?,
            json!({
                "kind":kind,"id":id(number),"active":true,"strength":"required",
                "scope":{"people":{"kind":"all"}}
            }),
        );
    }
    Ok(document)
}

fn context() -> CompileContext {
    CompileContext {
        scenario_revision: 1,
        semantic_metadata: BTreeMap::from([(
            "test.payload".to_owned(),
            "quotes \" slash \\ café\n".to_owned(),
        )]),
        control: eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    }
}

#[test]
fn full_ir_preflight_accounts_for_every_serialized_byte() -> Result {
    for (extra_shifts, rejected) in [(0, false), (10, false), (0, true)] {
        let mut document = source()?;
        let prototype = document
            .domain
            .entities
            .get(&id(8).parse()?)
            .ok_or("shift")?
            .clone();
        for index in 0..extra_shifts {
            let mut shift = prototype.clone();
            shift["id"] = json!(id(200 + index));
            document
                .domain
                .entities
                .insert(id(200 + index).parse()?, shift);
        }
        if rejected {
            document
                .domain
                .entities
                .get_mut(&id(1).parse()?)
                .ok_or("person")?["eligibleAssignmentTypeIds"] = json!([]);
        }
        let context = context();
        let limits = context.planning_limits;
        let mut budget = OperationBudget::analysis(Some(&context.control), limits);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let policy = require_supported(&input, &mut budget)?;
        check_metadata(&context, limits, &mut budget)?;
        let (analysis, plan) = prepare(&document, &input, &mut budget, limits)?;
        let ranks = RankIndex::new(&input, &mut budget)?;
        let model = ModelInput {
            document: &document,
            context: &context,
            policy,
            analysis: &analysis,
            plan: &plan,
            ranks: &ranks,
            limits,
        };
        let header = preflight::prepare(&model, &mut budget)?;
        let mut problem = construct(&model, header, &mut budget)?;
        problem.canonicalize()?;
        assert_eq!(
            budget.reserved_ir_bytes(),
            count(serde_json::to_vec(&problem)?.len())?
        );
    }
    Ok(())
}

#[test]
fn construction_and_generic_phases_observe_the_same_cancellation_token() -> Result {
    for phase in 0..4 {
        let document = source()?;
        let context = context();
        let limits = context.planning_limits;
        let mut budget = OperationBudget::analysis(Some(&context.control), limits);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let policy = require_supported(&input, &mut budget)?;
        let (analysis, plan) = prepare(&document, &input, &mut budget, limits)?;
        let ranks = RankIndex::new(&input, &mut budget)?;
        let model = ModelInput {
            document: &document,
            context: &context,
            policy,
            analysis: &analysis,
            plan: &plan,
            ranks: &ranks,
            limits,
        };
        if phase == 0 {
            budget.cancel_after_steps(1)?;
            assert!(matches!(
                preflight::prepare(&model, &mut budget),
                Err(AssignmentRuleError::Cancelled)
            ));
            continue;
        }
        let header = preflight::prepare(&model, &mut budget)?;
        if phase == 1 {
            budget.cancel_after_steps(1)?;
            assert!(matches!(
                construct(&model, header, &mut budget),
                Err(AssignmentRuleError::Cancelled)
            ));
            continue;
        }
        let mut problem = construct(&model, header, &mut budget)?;
        if phase == 2 {
            budget.cancel_after_steps(1)?;
            assert_eq!(
                precharge_canonicalization(&problem, &mut budget),
                Err(AssignmentRuleError::Cancelled)
            );
        } else {
            precharge_canonicalization(&problem, &mut budget)?;
            problem.canonicalize()?;
            budget.cancel_after_steps(1)?;
            assert_eq!(
                precharge_validation(&problem, &mut budget),
                Err(AssignmentRuleError::Cancelled)
            );
        }
        assert_eq!(
            context.control.check(),
            Err(eutheto_types::OperationInterruption::Cancelled)
        );
    }
    Ok(())
}

#[test]
fn policy_free_drafts_are_not_given_an_implicit_objective() -> Result {
    let mut document = source()?;
    document.domain.entities.remove(&id(9).parse()?);
    // The existing partial API still permits a draft with no scoring policy.
    super::super::compile_assignment_rules(&document, &context())?;
    assert!(matches!(
        compile_workforce(&document, &context()),
        Err(AssignmentRuleError::MissingScorePolicy)
    ));
    Ok(())
}

#[test]
fn active_unsupported_obligations_cannot_disappear_from_complete_compilation() -> Result {
    let mut document = source()?;
    let context = context();
    let rule_id = id(30).parse()?;
    let preference_id = id(31).parse()?;
    let lock_id = id(14).parse()?;
    document.domain.rules.insert(
        rule_id,
        json!({
            "kind":"maximumAssignmentCount", "id":id(30), "active":true,
            "strength":"required", "scope":{"people":{"kind":"all"}},
            "calendarId":id(2), "maximum":2
        }),
    );
    document.domain.preferences.insert(
        preference_id,
        json!({
            "kind":"assignmentType", "id":id(31), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":1,
            "direction":"prefer", "assignmentTypeIds":[id(4)]
        }),
    );
    document.domain.locked_assignments = fixture()?.domain.locked_assignments;
    for expected in [
        UnsupportedWorkforceObligation::RequiredRule(rule_id),
        UnsupportedWorkforceObligation::Preference(preference_id),
        UnsupportedWorkforceObligation::HardLock(lock_id),
        UnsupportedWorkforceObligation::SoftLock(lock_id),
    ] {
        assert!(matches!(
            compile_workforce(&document, &context),
            Err(AssignmentRuleError::UnsupportedObligation(actual)) if actual == expected
        ));
        match expected {
            UnsupportedWorkforceObligation::RequiredRule(_) => {
                document.domain.rules.get_mut(&rule_id).ok_or("rule")?["active"] = json!(false);
            }
            UnsupportedWorkforceObligation::Preference(_) => {
                document
                    .domain
                    .preferences
                    .get_mut(&preference_id)
                    .ok_or("preference")?["active"] = json!(false);
            }
            UnsupportedWorkforceObligation::HardLock(_) => {
                document
                    .domain
                    .locked_assignments
                    .get_mut(&lock_id)
                    .ok_or("lock")?["state"] = json!({"kind":"soft","stabilityWeight":1});
            }
            UnsupportedWorkforceObligation::SoftLock(_) => {
                document
                    .domain
                    .locked_assignments
                    .get_mut(&lock_id)
                    .ok_or("lock")?["state"] = json!({"kind":"unlocked"});
            }
        }
    }
    let compiled = compile_workforce(&document, &context)?;
    assert_eq!(compiled.problem.objectives.levels[0].upper_bound, 3);
    assert_eq!(compiled.problem.objectives.levels.len(), 1);
    Ok(())
}

#[test]
fn semantic_metadata_cannot_override_compiler_facts_and_affects_the_hash() -> Result {
    use eutheto_planning_ir::canonical_ir_hash;
    let document = source()?;
    for key in RESERVED_METADATA {
        let mut context = context();
        context
            .semantic_metadata
            .insert(key.to_owned(), "spoofed".to_owned());
        assert!(matches!(
            compile_workforce(&document, &context),
            Err(AssignmentRuleError::ReservedSemanticMetadata(actual)) if actual == key
        ));
    }
    let mut context = context();
    let first = compile_workforce(&document, &context)?;
    let value = "different caller semantics";
    context
        .semantic_metadata
        .insert("test.payload".to_owned(), value.to_owned());
    let second = compile_workforce(&document, &context)?;
    assert_eq!(
        second
            .problem
            .metadata
            .compile_metadata
            .get(&MetadataKey::new("test.payload")?),
        Some(&ProvenanceParameter::Text(value.to_owned()))
    );
    assert_ne!(
        canonical_ir_hash(&first.problem, context.planning_limits)?,
        canonical_ir_hash(&second.problem, context.planning_limits)?
    );
    Ok(())
}

#[test]
fn later_model_phases_cannot_refill_consumed_retention() -> Result {
    use super::super::AssignmentRuleLimit;
    for before_construction in [true, false] {
        let document = source()?;
        let context = context();
        let limits = context.planning_limits;
        let mut budget = OperationBudget::analysis(Some(&context.control), limits);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let policy = require_supported(&input, &mut budget)?;
        let (analysis, plan) = prepare(&document, &input, &mut budget, limits)?;
        let ranks = RankIndex::new(&input, &mut budget)?;
        let model = ModelInput {
            document: &document,
            context: &context,
            policy,
            analysis: &analysis,
            plan: &plan,
            ranks: &ranks,
            limits,
        };
        if before_construction {
            budget.reserve(0, 0, budget.remaining_output().2)?;
            assert!(matches!(
                preflight::prepare(&model, &mut budget),
                Err(AssignmentRuleError::LimitExceeded(
                    AssignmentRuleLimit::Bytes
                ))
            ));
        } else {
            let header = preflight::prepare(&model, &mut budget)?;
            let problem = construct(&model, header, &mut budget)?;
            budget.reserve(0, 0, budget.remaining_output().2)?;
            assert!(matches!(
                precharge_canonicalization(&problem, &mut budget),
                Err(AssignmentRuleError::LimitExceeded(
                    AssignmentRuleLimit::Bytes
                ))
            ));
        }
    }
    Ok(())
}

#[test]
fn complete_model_honors_channel_count_and_rank_sum_limits() -> Result {
    use super::super::AssignmentRuleLimit;
    let document = source()?;
    let mut context = context();
    context.planning_limits.max_variables = 4;
    context.planning_limits.max_abs_value = 3;
    let problem = compile_workforce(&document, &context)?.problem;
    assert_eq!(problem.objectives.levels[0].upper_bound, 3);
    context.planning_limits.max_variables = 3;
    assert!(matches!(
        compile_workforce(&document, &context),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::Variables
        ))
    ));
    context.planning_limits.max_variables = 4;
    context.planning_limits.max_abs_value = 2;
    assert!(matches!(
        compile_workforce(&document, &context),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::PerRecord
        ))
    ));
    Ok(())
}

#[test]
fn complete_ir_and_retained_scratch_share_the_callers_byte_allowance() -> Result {
    use super::super::AssignmentRuleLimit;
    let document = source()?;
    let mut context = context();
    for index in 0..64 {
        context
            .semantic_metadata
            .insert(format!("test.payload.{index}"), "x".repeat(4096));
    }
    let problem = compile_workforce(&document, &context)?.problem;
    context.planning_limits.max_ir_bytes = count(serde_json::to_vec(&problem)?.len())?;
    // Serialized IR alone fits exactly. Source analysis, diagnostic sidecar and scratch
    // are retained by the same operation, so there is no room to publish this full model.
    assert!(matches!(
        compile_workforce(&document, &context),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::Bytes
        ))
    ));
    Ok(())
}

#[test]
fn empty_caller_semantic_text_is_rejected_before_model_construction() -> Result {
    let mut context = context();
    context
        .semantic_metadata
        .insert("test.empty".to_owned(), String::new());
    assert!(matches!(
        compile_workforce(&source()?, &context),
        Err(AssignmentRuleError::InvalidSemanticMetadata)
    ));
    Ok(())
}
