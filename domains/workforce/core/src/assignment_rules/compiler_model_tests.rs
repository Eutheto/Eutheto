use super::*;
use crate::test_support::{fixture, id};
use eutheto_domain_ir::{ObjectiveProfileId, RunObjectiveOperation};
use eutheto_planning_ir::Constraint;
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

fn late_date_source() -> Result<ScenarioDocument> {
    let mut document = source()?;
    document.domain.entities.remove(&id(6).parse()?);
    document.settings.time_zone = "Pacific/Kiritimati".parse()?;
    document.settings.horizon.start = "9999-12-29T10:00:00Z".parse()?;
    document.settings.horizon.end = "9999-12-30T10:00:00Z".parse()?;
    let shift = document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or("shift")?;
    shift["startsAt"] = json!({
        "instant":"9999-12-30T08:00:00Z",
        "local":"9999-12-30T22:00:00",
        "offsetSeconds":50400
    });
    shift["endsAt"] = json!({
        "instant":"9999-12-30T09:00:00Z",
        "local":"9999-12-30T23:00:00",
        "offsetSeconds":50400
    });
    document.domain.preferences.insert(
        id(31).parse()?,
        json!({
            "kind":"time", "id":id(31), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal",
            "weight":1, "direction":"avoid",
            "timeWindow":{"kind":"weekly","windows":[
                {"weekdays":["sunday"], "startTime":"00:00:00",
                 "endTime":"01:00:00", "endDayOffset":0},
                {"weekdays":["friday"], "startTime":"00:00:00",
                 "endTime":"01:00:00", "endDayOffset":0}
            ]}
        }),
    );
    Ok(document)
}

fn scalarized_preference_source(entries_per_level: u32) -> Result<ScenarioDocument> {
    let mut document = source()?;
    document.domain.entities.remove(&id(6).parse()?);
    let policy = document
        .domain
        .entities
        .get_mut(&id(9).parse()?)
        .ok_or("score policy")?;
    policy["levels"] = json!([
        {"levelKey":"high","label":"High"},
        {"levelKey":"low","label":"Low"}
    ]);
    policy["priorityMapping"] = json!([
        {"priority":"low","levelKey":"low","scale":1},
        {"priority":"normal","levelKey":"high","scale":1},
        {"priority":"high","levelKey":"high","scale":1},
        {"priority":"veryHigh","levelKey":"low","scale":1}
    ]);
    for index in 0..entries_per_level {
        let high = 10_000 + index * 2;
        let low = high + 1;
        for (id_number, priority, direction) in [(high, "normal", "prefer"), (low, "low", "avoid")]
        {
            document.domain.preferences.insert(
                id(id_number).parse()?,
                json!({
                    "kind":"assignmentType", "id":id(id_number), "active":true,
                    "scope":{"people":{"kind":"all"}}, "priority":priority,
                    "weight":1_000_000, "direction":direction,
                    "assignmentTypeIds":[id(4)]
                }),
            );
        }
    }
    Ok(document)
}

#[test]
fn weekly_time_preference_accepts_a_late_date_with_no_matching_occurrence() -> Result {
    let compiled = compile_workforce(&late_date_source()?, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].upper_bound, 0);
    assert_eq!(compiled.problem.objectives.levels[1].upper_bound, 1);
    Ok(())
}

#[test]
fn scalarized_preference_limit_matches_native_signed_objective_safety() -> Result {
    use super::super::AssignmentRuleLimit;

    let supported = scalarized_preference_source(1_500)?;
    let compiled = compile_workforce(&supported, &context())?;
    assert_eq!(
        compiled.problem.objectives.levels[0].upper_bound,
        1_500_000_000
    );
    assert_eq!(
        compiled.problem.objectives.levels[1].upper_bound,
        1_500_000_000
    );
    assert_eq!(compiled.problem.objectives.levels[2].upper_bound, 1);

    let overlarge = scalarized_preference_source(1_600)?;
    assert!(matches!(
        compile_workforce(&overlarge, &context()),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::PerRecord
        ))
    ));
    Ok(())
}

fn context() -> CompileContext {
    CompileContext {
        objective_context: None,
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
#[allow(clippy::too_many_lines)]
fn full_ir_preflight_accounts_for_every_serialized_byte() -> Result {
    for (extra_shifts, rejected, consecutive) in [
        (0, false, None),
        (10, false, None),
        (0, true, None),
        (10, false, Some(1)),
        (10, false, Some(2)),
    ] {
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
            if consecutive.is_some() {
                shift["startsAt"] = json!({
                    "instant":format!("2026-11-01T{:02}:00:00Z", 8 + index),
                    "local":format!("2026-11-01T{:02}:00:00", 3 + index),
                    "offsetSeconds":-18000
                });
                shift["endsAt"] = json!({
                    "instant":format!("2026-11-01T{:02}:00:00Z", 9 + index),
                    "local":format!("2026-11-01T{:02}:00:00", 4 + index),
                    "offsetSeconds":-18000
                });
            }
            document
                .domain
                .entities
                .insert(id(200 + index).parse()?, shift);
        }
        if let Some(maximum) = consecutive {
            document.domain.rules.insert(
                id(33).parse()?,
                json!({
                    "kind":"maximumConsecutive","id":id(33),"active":true,
                    "strength":"required","scope":{"people":{"kind":"all"}},
                    "mode":{"kind":"assignments","breakMinutes":0},"maximum":maximum
                }),
            );
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
        let policy = require_supported(
            &input,
            document.scenario_id,
            context.objective_context.as_ref(),
            &mut budget,
        )?;
        check_metadata(&context, limits, &mut budget)?;
        let (analysis, plan) = prepare(&document, &input, &mut budget, limits)?;
        let preference_plan = prepare_preferences(
            &document,
            &input,
            context.objective_context.as_ref(),
            limits,
            &mut budget,
        )?;
        let ranks = RankIndex::new(&input, &mut budget)?;
        let model = ModelInput {
            document: &document,
            context: &context,
            assignment: &input,
            policy,
            analysis: &analysis,
            plan: &plan,
            ranks: &ranks,
            preferences: &preference_plan,
            baseline_pairs: None,
            limits,
        };
        let header = preflight::prepare(&model, &mut budget)?;
        let mut problem = construct(&model, header, &mut budget)?;
        problem.canonicalize()?;
        let reserved = budget.reserved_ir_bytes();
        let serialized = serde_json::to_vec(&problem)?;
        assert_eq!(
            serde_json::from_slice::<PlanningProblem>(&serialized)?,
            problem
        );
        let actual = count(serialized.len())?;
        if consecutive.is_some() {
            // Borrowed literal sizing uses `false`, one byte larger than `true`.
            // Only that established polarity allowance may exceed emitted bytes.
            let polarity_slack = problem
                .constraints
                .iter()
                .try_fold(0_u64, |slack, record| {
                    let body = match &record.body {
                        Constraint::BoolOr { literals } => {
                            literals.iter().filter(|literal| literal.positive).count()
                        }
                        _ => 0,
                    };
                    let enforcement = record
                        .enforcement
                        .iter()
                        .filter(|literal| literal.positive)
                        .count();
                    super::super::budget::add(slack, count(body + enforcement)?)
                })?;
            assert!(
                reserved >= actual && reserved - actual <= polarity_slack,
                "maximum={consecutive:?}: reserved={reserved}, actual={actual}, polarity_slack={polarity_slack}"
            );
        } else {
            assert_eq!(reserved, actual);
        }
    }
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn dense_consecutive_helpers_honor_actual_caller_admission_boundaries() -> Result {
    use super::super::AssignmentRuleLimit;

    let mut document = source()?;
    let prototype = document
        .domain
        .entities
        .get(&id(8).parse()?)
        .ok_or("shift")?
        .clone();
    for index in 0..10 {
        let mut shift = prototype.clone();
        shift["id"] = json!(id(200 + index));
        document
            .domain
            .entities
            .insert(id(200 + index).parse()?, shift);
    }
    document.domain.rules.insert(
        id(33).parse()?,
        json!({
            "kind":"maximumConsecutive","id":id(33),"active":true,
            "strength":"required","scope":{"people":{"kind":"all"}},
            "mode":{"kind":"assignments","breakMinutes":0},"maximum":1
        }),
    );
    let baseline = compile_workforce(&document, &context())?.problem;
    let mut limited = context();
    limited.planning_limits.max_variables = count(baseline.variables.len())?;
    assert_eq!(compile_workforce(&document, &limited)?.problem, baseline);
    limited.planning_limits.max_variables -= 1;
    assert!(matches!(
        compile_workforce(&document, &limited),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::Variables
        ))
    ));

    // Source/scratch retention also consumes references and bytes. Discover the
    // complete admission boundary, not an incidental current helper-record count.
    for (cap, maximum, expected) in [
        (
            "value",
            u64::try_from(PlanningIrLimitsV1::DEFAULT.max_abs_value)?,
            AssignmentRuleLimit::PerRecord,
        ),
        (
            "node references",
            PlanningIrLimitsV1::DEFAULT.max_refs_per_node,
            AssignmentRuleLimit::PerRecord,
        ),
        (
            "references",
            PlanningIrLimitsV1::DEFAULT.max_total_refs,
            AssignmentRuleLimit::References,
        ),
        (
            "enforcement",
            PlanningIrLimitsV1::DEFAULT.max_enforcement_literals,
            AssignmentRuleLimit::PerRecord,
        ),
        (
            "bytes",
            PlanningIrLimitsV1::DEFAULT.max_ir_bytes,
            AssignmentRuleLimit::Bytes,
        ),
    ] {
        let mut lower = 0;
        let mut upper = maximum;
        let mut limited = context();
        while lower < upper {
            let middle = lower + (upper - lower) / 2;
            match cap {
                "value" => limited.planning_limits.max_abs_value = i64::try_from(middle)?,
                "node references" => limited.planning_limits.max_refs_per_node = middle,
                "references" => limited.planning_limits.max_total_refs = middle,
                "enforcement" => limited.planning_limits.max_enforcement_literals = middle,
                "bytes" => limited.planning_limits.max_ir_bytes = middle,
                _ => unreachable!(),
            }
            match compile_workforce(&document, &limited) {
                Ok(compiled) => {
                    assert_eq!(compiled.problem, baseline, "{cap}");
                    upper = middle;
                }
                Err(AssignmentRuleError::LimitExceeded(actual)) => {
                    assert_eq!(actual, expected, "{cap}");
                    lower = middle + 1;
                }
                Err(error) => return Err(error.into()),
            }
        }
        assert!(lower > 0 && lower < maximum, "{cap}");
        for (value, admitted) in [(lower, true), (lower - 1, false)] {
            match cap {
                "value" => limited.planning_limits.max_abs_value = i64::try_from(value)?,
                "node references" => limited.planning_limits.max_refs_per_node = value,
                "references" => limited.planning_limits.max_total_refs = value,
                "enforcement" => limited.planning_limits.max_enforcement_literals = value,
                "bytes" => limited.planning_limits.max_ir_bytes = value,
                _ => unreachable!(),
            }
            let result = compile_workforce(&document, &limited);
            if admitted {
                assert_eq!(result?.problem, baseline, "{cap}");
            } else {
                assert!(
                    matches!(result, Err(AssignmentRuleError::LimitExceeded(actual)) if actual == expected),
                    "{cap}"
                );
            }
        }
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
        let policy = require_supported(
            &input,
            document.scenario_id,
            context.objective_context.as_ref(),
            &mut budget,
        )?;
        let (analysis, plan) = prepare(&document, &input, &mut budget, limits)?;
        let preference_plan = prepare_preferences(
            &document,
            &input,
            context.objective_context.as_ref(),
            limits,
            &mut budget,
        )?;
        let ranks = RankIndex::new(&input, &mut budget)?;
        let model = ModelInput {
            document: &document,
            context: &context,
            assignment: &input,
            policy,
            analysis: &analysis,
            plan: &plan,
            ranks: &ranks,
            preferences: &preference_plan,
            baseline_pairs: None,
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
#[allow(clippy::too_many_lines)]
fn active_unsupported_obligations_cannot_disappear_from_complete_compilation() -> Result {
    let mut document = source()?;
    let context = context();
    let preference_id = id(31).parse()?;
    let lock_id = id(14).parse()?;
    document.domain.preferences.insert(
        preference_id,
        json!({
            "kind":"assignmentTarget", "id":id(31), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":1,
            "calendarId":id(2), "targetCount":1
        }),
    );
    document.domain.locked_assignments = fixture()?.domain.locked_assignments;
    let target = compile_workforce(&document, &context)?;
    assert!(
        target
            .problem
            .constraints
            .iter()
            .any(|record| { matches!(&record.body, Constraint::AbsDifference { .. }) })
    );

    document.domain.entities.insert(
        id(30).parse()?,
        json!({
            "kind":"baseSchedule", "id":id(30), "sourceSolutionId":id(31),
            "sourceRevision":1, "assignments":[{"personId":id(1),"shiftId":id(7)}]
        }),
    );
    document.domain.entities.insert(
        id(40).parse()?,
        json!({
            "kind":"coverageRequirement", "id":id(40), "active":true,
            "scope":{"kind":"all"},
            "coverage":{"kind":"atLeast","minimum":1,"preferredCount":1,"maximumCount":1,"qualificationMinimums":[]}
        }),
    );
    for supported in [
        json!({
            "kind":"baseStability", "id":id(31), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":1,
            "baseScheduleId":id(30)
        }),
        json!({
            "kind":"togetherSeparate", "id":id(31), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":1,
            "direction":"together", "personIds":[id(1)], "mode":"sameShift"
        }),
        json!({
            "kind":"preferredCoverage", "id":id(31), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":1,
            "coverageRequirementIds":[id(40)]
        }),
    ] {
        document
            .domain
            .preferences
            .insert(preference_id, supported.clone());
        let outcome = compile_workforce(&document, &context);
        assert!(
            outcome.is_ok(),
            "supported {} did not compile: {outcome:?}",
            supported["kind"]
        );
    }
    document.domain.entities.remove(&id(30).parse()?);
    document.domain.entities.remove(&id(40).parse()?);
    document.domain.preferences.insert(
        preference_id,
        json!({
            "kind":"assignmentTarget", "id":id(31), "active":false,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":1,
            "calendarId":id(2), "targetCount":1
        }),
    );
    let hard = compile_workforce(&document, &context)?;
    assert!(
        hard.problem
            .constraints
            .iter()
            .any(|record| { matches!(&record.body, Constraint::BoolOr { .. }) })
    );
    document
        .domain
        .locked_assignments
        .get_mut(&lock_id)
        .ok_or("lock")?["state"] = json!({"kind":"soft","stabilityWeight":1});
    assert!(matches!(
        compile_workforce(&document, &context),
        Err(AssignmentRuleError::UnsupportedObligation(
            UnsupportedWorkforceObligation::SoftLock(actual)
        )) if actual == lock_id
    ));
    let mut authored_context = context.clone();
    authored_context.objective_context = Some(RunObjectiveContext::authored(
        document.domain_pack.id.clone(),
    )?);
    assert!(matches!(
        compile_workforce(&document, &authored_context),
        Err(AssignmentRuleError::InvalidDocument(DomainPackError::InvalidPayload {
            path,
            ..
        })) if path == "objectiveContext"
    ));
    document
        .domain
        .locked_assignments
        .get_mut(&lock_id)
        .ok_or("lock")?["state"] = json!({"kind":"unlocked"});
    let compiled = compile_workforce(&document, &context)?;
    assert_eq!(compiled.problem.objectives.levels[0].upper_bound, 3);
    assert_eq!(compiled.problem.objectives.levels.len(), 1);
    Ok(())
}

#[test]
fn compiler_entry_rejects_unsupported_objective_contexts() -> Result {
    let document = source()?;
    let authored = RunObjectiveContext::authored(document.domain_pack.id.clone())?;
    let mut unknown_profile = authored.clone();
    unknown_profile.profile_id = ObjectiveProfileId::new("eutheto.objective.unknown")?;
    let mut wrong_pack = authored.clone();
    wrong_pack.pack_id = PackId::new("official.other")?;
    let mut unknown_semantics = authored.clone();
    unknown_semantics.semantic_version += 1;
    let mut repair = authored.clone();
    repair.operation = RunObjectiveOperation::Repair;
    let mut unknown_version = authored;
    unknown_version.schema_version = u32::MAX;

    for objective_context in [unknown_profile, wrong_pack, unknown_semantics, repair] {
        let mut compile_context = context();
        compile_context.objective_context = Some(objective_context);
        assert!(matches!(
            compile_workforce(&document, &compile_context),
            Err(AssignmentRuleError::InvalidDocument(DomainPackError::InvalidPayload {
                path,
                ..
            })) if path == "objectiveContext"
        ));
        assert!(matches!(
            super::super::compile_assignment_rules(&document, &compile_context),
            Err(AssignmentRuleError::InvalidDocument(DomainPackError::InvalidPayload {
                path,
                ..
            })) if path == "objectiveContext"
        ));
    }
    let mut compile_context = context();
    compile_context.objective_context = Some(unknown_version);
    assert!(matches!(
        compile_workforce(&document, &compile_context),
        Err(AssignmentRuleError::InvalidDocument(
            DomainPackError::UnsupportedVersion(_)
        ))
    ));
    assert!(matches!(
        super::super::compile_assignment_rules(&document, &compile_context),
        Err(AssignmentRuleError::InvalidDocument(
            DomainPackError::UnsupportedVersion(_)
        ))
    ));
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
        let policy = require_supported(
            &input,
            document.scenario_id,
            context.objective_context.as_ref(),
            &mut budget,
        )?;
        let (analysis, plan) = prepare(&document, &input, &mut budget, limits)?;
        let preference_plan = prepare_preferences(
            &document,
            &input,
            context.objective_context.as_ref(),
            limits,
            &mut budget,
        )?;
        let ranks = RankIndex::new(&input, &mut budget)?;
        let model = ModelInput {
            document: &document,
            context: &context,
            assignment: &input,
            policy,
            analysis: &analysis,
            plan: &plan,
            ranks: &ranks,
            preferences: &preference_plan,
            baseline_pairs: None,
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
