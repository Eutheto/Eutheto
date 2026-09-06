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
        cancellation: CancellationToken::new(),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    }
}

#[test]
fn full_ir_preflight_accounts_for_every_serialized_byte() -> Result {
    for rejected in [false, true] {
        let mut document = source()?;
        if rejected {
            document
                .domain
                .entities
                .get_mut(&id(1).parse()?)
                .ok_or("person")?["eligibleAssignmentTypeIds"] = json!([]);
        }
        let context = context();
        let limits = context.planning_limits;
        let mut budget = OperationBudget::analysis(Some(&context.cancellation), limits);
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
        let mut budget = OperationBudget::analysis(Some(&context.cancellation), limits);
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
        assert!(context.cancellation.is_cancelled());
    }
    Ok(())
}
