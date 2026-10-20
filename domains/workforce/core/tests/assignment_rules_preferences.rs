#![forbid(unsafe_code)]

mod support;

use eutheto_domain_api::{CompileContext, DomainPackError};
use eutheto_domain_ir::{
    AcceptedResultRefV1, AssignmentEvidenceV1, AssignmentValue, DomainAssignment,
    DomainAssignmentId, DomainEntityId, DomainEntityKindId, DomainEntityRef, EvidenceMessageV1,
    EvidenceRenderRequestV2, ExplanationEvidencePayloadV2, ExplanationEvidenceV2,
    NormalizedSolution, ObjectiveProfileId, RunBaselineBinding, RunBaselineOriginal,
    RunBaselineSelection, RunExternalBaselineBinding, RunObjectiveContext, RunObjectiveOperation,
    ScoreContributionV1, VerificationContextV1, VerificationValue, blake3_hex,
};
use eutheto_planning_ir::{PlanningIrLimitsV1, canonical_ir_hash};
use eutheto_types::{
    CancellationToken, OperationControl, PackId, Revision, RuleId, ScenarioDocument,
};
use eutheto_workforce::{
    assignment_rules::{
        AssignmentRuleError, AssignmentRuleLimit, compile_workforce, render_workforce_evidence,
        score_workforce_solution, verify_workforce_solution, workforce_baseline_reference,
        workforce_external_baseline_reference, workforce_manual_selection_checksum,
        workforce_pair_content_hash, workforce_verification_scope,
    },
    model::{AssignmentPair, ManualSelection, WorkforceEntity},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
};

const ASSIGNMENT_KIND: &str = "official.workforce.assignment";

type Result<T = (), E = Box<dyn Error>> = std::result::Result<T, E>;

fn id(index: u32) -> String {
    support::id(index)
}

fn base() -> Result<Value> {
    let mut value = serde_json::to_value(support::fixture()?)?;
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(6));
    value["domain"]["lockedAssignments"] = json!({});
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    let shift = &mut value["domain"]["entities"][id(8)];
    shift["startsAt"] = json!({
        "instant":"2026-11-01T08:00:00Z",
        "local":"2026-11-01T08:00:00",
        "offsetSeconds":0
    });
    shift["endsAt"] = json!({
        "instant":"2026-11-01T10:00:00Z",
        "local":"2026-11-01T10:00:00",
        "offsetSeconds":0
    });
    Ok(value)
}

fn weekend_base() -> Result<Value> {
    let mut value = base()?;
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-17T00:00:00Z"});
    value["domain"]["entities"][id(8)]["startsAt"] = json!({
        "instant":"2026-11-02T08:00:00Z",
        "local":"2026-11-02T08:00:00",
        "offsetSeconds":0
    });
    value["domain"]["entities"][id(8)]["endsAt"] = json!({
        "instant":"2026-11-02T10:00:00Z",
        "local":"2026-11-02T10:00:00",
        "offsetSeconds":0
    });
    Ok(value)
}
fn availability(value: &mut Value, index: u32, owner: u32, kind: &str, start: &str, end: &str) {
    value["domain"]["entities"][id(index)] = json!({
        "kind":"availability",
        "id":id(index),
        "personId":id(owner),
        "availabilityKind":kind,
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "timeWindow":{"kind":"instant","startsAt":start,"endsAt":end},
        "source":"synthetic",
        "note":""
    });
}

fn requested_preference(value: &mut Value, index: u32, availability_ids: &[u32], scope: &Value) {
    value["domain"]["preferences"][id(index)] = json!({
        "kind":"requestedTimeOff",
        "id":id(index),
        "active":true,
        "scope":scope,
        "priority":"normal",
        "weight":1,
        "availabilityIds":availability_ids.iter().map(|entry| id(*entry)).collect::<Vec<_>>()
    });
}

fn location_preference(value: &mut Value, index: u32, direction: &str, weight: u32) {
    value["domain"]["preferences"][id(index)] = json!({
        "kind":"location",
        "id":id(index),
        "active":true,
        "scope":{"people":{"kind":"all"}},
        "priority":"normal",
        "weight":weight,
        "direction":direction,
        "locationIds":[id(5)]
    });
}
fn adjacency_preference(
    value: &mut Value,
    index: u32,
    direction: &str,
    maximum_gap_minutes: u32,
    mode: &str,
    base_schedule: u32,
) {
    value["domain"]["preferences"][id(index)] = json!({
        "kind":"adjacency",
        "id":id(index),
        "active":true,
        "scope":{"people":{"kind":"all"}},
        "priority":"normal",
        "weight":1,
        "direction":direction,
        "baseScheduleId":id(base_schedule),
        "maximumGapMinutes":maximum_gap_minutes,
        "mode":mode
    });
}
fn workload_preference(value: &mut Value, index: u32, weight: u32, policy: u32) {
    value["domain"]["preferences"][id(index)] = json!({
        "kind":"workloadBalance",
        "id":id(index),
        "active":true,
        "scope":{"people":{"kind":"all"}},
        "priority":"low",
        "weight":weight,
        "workloadPolicyId":id(policy)
    });
}
fn weekend_target_preference(
    value: &mut Value,
    index: u32,
    target_count: u32,
    scope: &Value,
    priority: &str,
    weight: u32,
) {
    value["domain"]["preferences"][id(index)] = json!({
        "kind":"weekendTarget",
        "id":id(index),
        "active":true,
        "scope":scope,
        "priority":priority,
        "weight":weight,
        "targetCount":target_count
    });
}

fn add_shift(value: &mut Value, index: u32, start: &str, end: &str) {
    let mut shift = value["domain"]["entities"][id(8)].clone();
    shift["id"] = json!(id(index));
    shift["startsAt"] = json!({
        "instant":start,
        "local":start.trim_end_matches('Z'),
        "offsetSeconds":0
    });
    shift["endsAt"] = json!({
        "instant":end,
        "local":end.trim_end_matches('Z'),
        "offsetSeconds":0
    });
    value["domain"]["entities"][id(index)] = shift;
}

fn add_shift_with_offsets(
    value: &mut Value,
    index: u32,
    start: (&str, &str, i32),
    end: (&str, &str, i32),
) {
    let mut shift = value["domain"]["entities"][id(8)].clone();
    shift["id"] = json!(id(index));
    shift["startsAt"] = json!({
        "instant":start.0,
        "local":start.1,
        "offsetSeconds":start.2
    });
    shift["endsAt"] = json!({
        "instant":end.0,
        "local":end.1,
        "offsetSeconds":end.2
    });
    value["domain"]["entities"][id(index)] = shift;
}

fn add_night_peer(value: &mut Value) {
    let mut person = value["domain"]["entities"][id(1)].clone();
    person["id"] = json!(id(20));
    person["externalId"] = json!("staff-20");
    person["name"] = json!("Sky");
    person["workloadWeight"] = json!({"numerator":2,"denominator":1});
    value["domain"]["entities"][id(20)] = person;
}

fn decode_document(value: Value) -> Result<ScenarioDocument> {
    Ok(serde_json::from_value(value)?)
}

fn context() -> CompileContext {
    CompileContext {
        objective_context: None,
        scenario_revision: 1,
        semantic_metadata: BTreeMap::new(),
        control: OperationControl::Cancellation(CancellationToken::new()),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    }
}

fn assignment(person: u32, shift: u32, selected: bool) -> Result<DomainAssignment> {
    let pair = format!("{}.{}", id(person), id(shift));
    Ok(DomainAssignment {
        id: DomainAssignmentId::new(format!("{ASSIGNMENT_KIND}.{pair}"))?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new(ASSIGNMENT_KIND)?,
            id: DomainEntityId::new(pair)?,
        },
        value: AssignmentValue::Boolean(selected),
        evidence: Vec::new(),
    })
}

fn evidence_assignment(person: u32, shift: u32, selected: bool) -> Result<DomainAssignment> {
    // Obtain the native pair fact from the original eligible source, including pruned decisions.
    let mut source = base()?;
    add_shift(
        &mut source,
        18,
        "2026-11-01T10:00:00Z",
        "2026-11-01T12:00:00Z",
    );
    let compiled = eutheto_workforce::assignment_rules::compile_assignment_rules(
        &decode_document(source)?,
        &context(),
    )?;
    let source_pair = pair(person, shift)?;
    let variable = compiled
        .variables
        .iter()
        .find(|entry| entry.pair == source_pair)
        .ok_or("original source pair")?;
    let mut assignment = assignment(person, shift, selected)?;
    assignment
        .evidence
        .push(variable.variable.provenance.as_str().parse()?);
    Ok(assignment)
}

fn solution(
    document: &ScenarioDocument,
    assignments: Vec<DomainAssignment>,
) -> Result<NormalizedSolution> {
    Ok(NormalizedSolution {
        schema_version: 1,
        pack_id: PackId::new("official.workforce")?,
        scenario_id: document.scenario_id,
        scenario_revision: 1,
        projection_version: 1,
        solution_id: id(700).parse()?,
        assignments,
    })
}

fn pair(person: u32, shift: u32) -> Result<AssignmentPair> {
    Ok(AssignmentPair {
        person_id: id(person).parse()?,
        shift_id: id(shift).parse()?,
    })
}

fn document_with_original_baseline(
    mut value: Value,
    assignments: &[AssignmentPair],
) -> Result<(ScenarioDocument, RunBaselineBinding)> {
    let hash = workforce_pair_content_hash(assignments, None)?;
    value["domain"]["entities"][id(20)] = json!({
        "kind":"baseSchedule",
        "id":id(20),
        "sourceSolutionId":id(21),
        "sourceRevision":0,
        "assignments":assignments,
    });
    let document = decode_document(value)?;
    let original = RunBaselineOriginal {
        entity: workforce_baseline_reference(id(20).parse()?, false)?,
        pair_content_hash: hash.clone(),
        accepted_result: AcceptedResultRefV1 {
            solution_id: id(21).parse()?,
            result_checksum: "a".repeat(64),
        },
        source_scenario_id: document.scenario_id,
        source_revision: 0,
        source_document_hash: "b".repeat(64),
        source_run_id: id(22).parse()?,
        source_run_input_checksum: "c".repeat(64),
        source_run_manifest_checksum: "d".repeat(64),
    };
    Ok((
        document,
        RunBaselineBinding {
            captured_original: None,
            original,
            selection: RunBaselineSelection::OriginalAccepted {},
            selected_pair_content_hash: hash,
        },
    ))
}

#[test]
fn direct_contribution_and_non_assignment_evidence_reject_missing_baseline_source() -> Result {
    let (mut document, binding) = document_with_original_baseline(base()?, &[pair(1, 8)?])?;
    let objective = objective_context(
        &document,
        "official.workforce.objective.minimal-changes",
        RunObjectiveOperation::Repair,
        Some(binding),
    )?;
    let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Infeasibility {
            infeasibility: eutheto_domain_ir::InfeasibilityEvidenceV1::Unavailable {
                reason: eutheto_domain_ir::ConflictUnavailableReason::BudgetExpired,
            },
            condition: None,
        },
    )?)?;
    let control = OperationControl::Cancellation(CancellationToken::new());
    let valid = render_workforce_evidence(&document, &request, Some(&objective), &control)?;
    assert!(
        valid
            .messages
            .iter()
            .any(|message| { message.message_key == "official.workforce.infeasibility.status" })
    );
    document.domain.entities.remove(&id(20).parse()?);
    assert!(matches!(
        render_workforce_evidence(&document, &request, Some(&objective), &control),
        Err(DomainPackError::Contract(reason))
            if reason == "official.workforce.authority.baseline_original_missing"
    ));
    let mut compile_context = context();
    compile_context.objective_context = Some(objective);
    assert!(matches!(
        eutheto_workforce::assignment_rules::compile_assignment_rules(&document, &compile_context),
        Err(AssignmentRuleError::InvalidDocument(
            DomainPackError::Contract(reason)
        )) if reason == "official.workforce.authority.baseline_original_missing"
    ));
    Ok(())
}

fn objective_context(
    document: &ScenarioDocument,
    profile_id: &str,
    operation: RunObjectiveOperation,
    baseline: Option<RunBaselineBinding>,
) -> Result<RunObjectiveContext> {
    Ok(RunObjectiveContext {
        schema_version: 1,
        pack_id: document.domain_pack.id.clone(),
        profile_id: ObjectiveProfileId::new(profile_id)?,
        semantic_version: 1,
        operation,
        baseline: baseline.map(Box::new),
        external_baseline: None,
    })
}

#[test]
fn base_stability_charges_each_changed_person_shift_pair_in_both_directions() -> Result {
    let mut value = base()?;
    add_night_peer(&mut value);
    value["domain"]["entities"][id(40)] = json!({
        "kind":"baseSchedule","id":id(40),"sourceSolutionId":id(41),"sourceRevision":1,
        "assignments":[{"personId":id(1),"shiftId":id(8)}]
    });
    value["domain"]["preferences"][id(42)] = json!({
        "kind":"baseStability","id":id(42),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "baseScheduleId":id(40)
    });
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let terms = &compiled.problem.objectives.levels[0].terms;
    assert_eq!(terms.len(), 2);
    assert!(terms.iter().any(|term| {
        term.expression.constant == 10 && term.expression.terms[0].coefficient == -10
    }));
    assert!(terms.iter().any(|term| {
        term.expression.constant == 0 && term.expression.terms[0].coefficient == 10
    }));
    for (selected, expected) in [
        (vec![], 10),
        (vec![assignment(1, 8, true)?], 0),
        (vec![assignment(20, 8, true)?], 20),
    ] {
        let score =
            score_workforce_solution(&document, &solution(&document, selected)?, None, None)?;
        assert_eq!(score.levels[0].value, expected);
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[test]
fn base_stability_keeps_constant_penalty_when_old_assignment_is_pruned() -> Result {
    let mut value = base()?;
    availability(
        &mut value,
        35,
        1,
        "approvedTimeOff",
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
    value["domain"]["entities"][id(40)] = json!({
        "kind":"baseSchedule","id":id(40),"sourceSolutionId":id(41),"sourceRevision":1,
        "assignments":[{"personId":id(1),"shiftId":id(8)}]
    });
    value["domain"]["preferences"][id(42)] = json!({
        "kind":"baseStability","id":id(42),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "baseScheduleId":id(40)
    });
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let objective = &compiled.problem.objectives.levels[0];
    assert_eq!(objective.lower_bound, 10);
    assert_eq!(objective.upper_bound, 10);
    assert_eq!(objective.terms.len(), 1);
    assert_eq!(objective.terms[0].expression.constant, 10);
    assert!(objective.terms[0].expression.terms.is_empty());
    let score = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(score.levels[0].value, 10);
    let mut limited = context();
    limited.planning_limits.max_parameters_per_record = 1;
    assert_eq!(
        compile_workforce(&document, &limited)?
            .problem
            .objectives
            .levels[0]
            .terms
            .len(),
        1
    );
    for profile in [
        "official.workforce.objective.balanced",
        "official.workforce.objective.preference-focused",
        "official.workforce.objective.fairness-first",
    ] {
        let objective =
            objective_context(&document, profile, RunObjectiveOperation::Optimize, None)?;
        let mut compile_context = context();
        compile_context.objective_context = Some(objective.clone());
        let model = compile_workforce(&document, &compile_context)?.problem;
        let (level, term) = model
            .objectives
            .levels
            .iter()
            .find_map(|level| {
                level
                    .terms
                    .iter()
                    .find(|term| {
                        term.category.as_str()
                            == "official.workforce.score.preference.base_stability"
                    })
                    .map(|term| (level, term))
            })
            .ok_or("pruned stability term")?;
        let contribution = ScoreContributionV1 {
            evidence_id: term.provenance.as_str().parse()?,
            level_id: level.id.as_str().parse()?,
            category_id: Some("official.workforce.score.preference.base_stability".parse()?),
            value: 10,
        };
        for (selected, rows) in [
            (false, Vec::new()),
            (false, vec![contribution.clone()]),
            (true, Vec::new()),
            (true, vec![contribution.clone()]),
        ] {
            let evidence = AssignmentEvidenceV1 {
                assignment: evidence_assignment(1, 8, selected)?,
                related_rules: Vec::new(),
                score_contributions: rows,
                metrics: BTreeMap::new(),
                lock_state: None,
            };
            let request = assignment_evidence_request(evidence.clone())?;
            let rendered = render_workforce_evidence(
                &document,
                &request,
                Some(&objective),
                &OperationControl::Cancellation(CancellationToken::new()),
            )?;
            assert_rendered_profile_contributions(
                &rendered.messages,
                std::slice::from_ref(&contribution),
                &evidence.assignment.id,
            )?;
        }
        let source_score = score_workforce_solution(
            &document,
            &solution(&document, Vec::new())?,
            Some(&objective),
            None,
        )?;
        assert_eq!(
            source_score
                .levels
                .iter()
                .find(|score| score.level_id == contribution.level_id)
                .ok_or("stability score level")?
                .value,
            10,
        );
    }
    Ok(())
}

#[test]
fn base_stability_ignores_base_assignments_outside_the_planning_horizon() -> Result {
    let mut value = base()?;
    value["settings"]["horizon"]["start"] = json!("2026-11-02T00:00:00Z");
    value["settings"]["horizon"]["end"] = json!("2026-11-03T00:00:00Z");
    add_shift(
        &mut value,
        19,
        "2026-11-02T08:00:00Z",
        "2026-11-02T09:00:00Z",
    );
    value["domain"]["entities"][id(40)] = json!({
        "kind":"baseSchedule","id":id(40),"sourceSolutionId":id(41),"sourceRevision":1,
        "assignments":[{"personId":id(1),"shiftId":id(8)}]
    });
    value["domain"]["preferences"][id(42)] = json!({
        "kind":"baseStability","id":id(42),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "baseScheduleId":id(40)
    });
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].upper_bound, 10);
    assert_eq!(compiled.problem.objectives.levels[0].lower_bound, 0);
    let score = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(score.levels[0].value, 0);
    Ok(())
}

#[test]
fn together_separate_scores_only_selected_pairs_with_the_authored_relation() -> Result {
    for (direction, mode, second_start, expected) in [
        ("together", "sameShift", "2026-11-01T10:00:00Z", 10),
        ("separate", "overlappingShifts", "2026-11-01T10:00:00Z", 0),
        ("separate", "overlappingShifts", "2026-11-01T09:00:00Z", 10),
        ("together", "overlappingShifts", "2026-11-01T09:00:00Z", 0),
    ] {
        let mut value = base()?;
        add_night_peer(&mut value);
        add_shift(&mut value, 30, second_start, "2026-11-01T12:00:00Z");
        value["domain"]["preferences"][id(40)] = json!({
            "kind":"togetherSeparate","id":id(40),"active":true,
            "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
            "direction":direction,"personIds":[id(1),id(20)],"mode":mode
        });
        let document = decode_document(value)?;
        let compiled = compile_workforce(&document, &context())?;
        assert!(compiled.problem.objectives.levels[0].upper_bound >= expected);
        let selected = vec![assignment(1, 8, true)?, assignment(20, 30, true)?];
        let score =
            score_workforce_solution(&document, &solution(&document, selected)?, None, None)?;
        assert_eq!(
            score.levels[0].value, expected,
            "{direction} {mode} {second_start}"
        );
        let one = score_workforce_solution(
            &document,
            &solution(&document, vec![assignment(1, 8, true)?])?,
            None,
            None,
        )?;
        assert_eq!(one.levels[0].value, 0);
    }
    Ok(())
}

#[test]
fn preferred_coverage_penalizes_only_deficit_of_active_requirement() -> Result {
    let mut value = base()?;
    add_night_peer(&mut value);
    value["domain"]["entities"][id(8)]["coverage"] = json!({
        "kind":"atLeast","minimum":0,"preferredCount":2,
        "maximumCount":2,"qualificationMinimums":[]
    });
    value["domain"]["entities"][id(40)] = json!({
        "kind":"coverageRequirement","id":id(40),"active":true,
        "scope":{"kind":"all"},
        "coverage":{"kind":"atLeast","minimum":0,"preferredCount":2,
            "maximumCount":2,"qualificationMinimums":[]}
    });
    value["domain"]["preferences"][id(41)] = json!({
        "kind":"preferredCoverage","id":id(41),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "coverageRequirementIds":[id(40)]
    });
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let objective = &compiled.problem.objectives.levels[0];
    assert_eq!(objective.upper_bound, 20);
    for (selected, expected) in [
        (vec![], 20),
        (vec![assignment(1, 8, true)?], 10),
        (vec![assignment(1, 8, true)?, assignment(20, 8, true)?], 0),
    ] {
        let score =
            score_workforce_solution(&document, &solution(&document, selected)?, None, None)?;
        assert_eq!(score.levels[0].value, expected);
    }
    Ok(())
}

#[test]
fn adjacency_prefer_scores_each_matching_pair_once_and_accepts_zero_gap() -> Result {
    let mut value = base()?;
    add_shift(
        &mut value,
        30,
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
    add_shift(
        &mut value,
        31,
        "2026-11-01T10:30:00Z",
        "2026-11-01T12:30:00Z",
    );
    add_shift(
        &mut value,
        32,
        "2026-11-01T10:00:00Z",
        "2026-11-01T12:00:00Z",
    );
    value["domain"]["entities"][id(40)] = json!({
        "kind":"baseSchedule",
        "id":id(40),
        "sourceSolutionId":id(41),
        "sourceRevision":1,
        "assignments":[{"personId":id(1),"shiftId":id(30)}]
    });
    adjacency_preference(&mut value, 41, "prefer", 0, "either", 40);
    let document = decode_document(value)?;

    let compiled = compile_workforce(&document, &context())?;
    let preference_level = &compiled.problem.objectives.levels[0];
    assert_eq!(preference_level.upper_bound, 10);
    assert_eq!(preference_level.terms.len(), 1);
    assert!(preference_level.terms.iter().all(|term| {
        term.expression.constant == 10 && term.expression.terms[0].coefficient == -10
    }));

    let empty = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(empty.levels[0].value, 10);
    assert_eq!(
        empty.levels[0]
            .category_breakdown
            .get(&eutheto_domain_ir::ScoreCategoryId::new(
                "official.workforce.score.preference.adjacency",
            )?),
        Some(&10)
    );
    let selected = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 32, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected.levels[0].value, 0);
    Ok(())
}

#[test]
fn inactive_adjacency_with_unknown_source_does_not_block_compilation() -> Result {
    let mut value = base()?;
    adjacency_preference(&mut value, 41, "prefer", 0, "either", 40);
    value["domain"]["preferences"][id(41)]["active"] = json!(false);
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels.len(), 1);
    Ok(())
}

#[test]
fn adjacency_avoid_after_only_penalizes_matching_candidate() -> Result {
    let mut value = base()?;
    add_shift(
        &mut value,
        30,
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
    add_shift(
        &mut value,
        31,
        "2026-11-01T10:00:00Z",
        "2026-11-01T12:00:00Z",
    );
    add_shift(
        &mut value,
        32,
        "2026-11-01T06:00:00Z",
        "2026-11-01T08:00:00Z",
    );
    value["domain"]["entities"][id(40)] = json!({
        "kind":"baseSchedule",
        "id":id(40),
        "sourceSolutionId":id(41),
        "sourceRevision":1,
        "assignments":[{"personId":id(1),"shiftId":id(30)}]
    });
    adjacency_preference(&mut value, 41, "avoid", 0, "after", 40);
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let preference_level = &compiled.problem.objectives.levels[0];
    assert_eq!(preference_level.upper_bound, 10);
    assert_eq!(preference_level.terms.len(), 1);
    assert_eq!(preference_level.terms[0].expression.constant, 0);
    assert_eq!(
        preference_level.terms[0].expression.terms[0].coefficient,
        10
    );

    let empty = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(empty.levels[0].value, 0);
    let selected_after = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 31, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected_after.levels[0].value, 10);
    let selected_before = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 32, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected_before.levels[0].value, 0);
    Ok(())
}

#[test]
fn location_prefer_and_requested_time_off_share_static_pair_and_category_accounting() -> Result {
    let mut value = base()?;
    availability(
        &mut value,
        31,
        1,
        "requestedTimeOff",
        "2026-11-01T08:00:00Z",
        "2026-11-01T09:00:00Z",
    );
    availability(
        &mut value,
        32,
        1,
        "requestedTimeOff",
        "2026-11-01T08:30:00Z",
        "2026-11-01T09:30:00Z",
    );
    requested_preference(&mut value, 33, &[31, 32], &json!({"people":{"kind":"all"}}));
    location_preference(&mut value, 34, "prefer", 2);
    let document = decode_document(value)?;

    let compiled = compile_workforce(&document, &context())?;
    let preference_level = &compiled.problem.objectives.levels[0];
    assert_eq!(preference_level.terms.len(), 2);
    assert_eq!(preference_level.upper_bound, 30);
    assert!(preference_level.terms.iter().any(|term| {
        term.expression.constant == 20 && term.expression.terms[0].coefficient == -20
    }));
    assert!(preference_level.terms.iter().any(|term| {
        term.expression.constant == 0 && term.expression.terms[0].coefficient == 10
    }));

    let empty = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(empty.feasibility, 0);
    assert_eq!(empty.levels[0].value, 20);
    assert_eq!(
        empty.levels[0]
            .category_breakdown
            .get(&eutheto_domain_ir::ScoreCategoryId::new(
                "official.workforce.score.preference.location",
            )?),
        Some(&20)
    );
    assert!(!empty.levels[0].category_breakdown.contains_key(
        &eutheto_domain_ir::ScoreCategoryId::new(
            "official.workforce.score.preference.requested_time_off",
        )?
    ));

    let selected = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 8, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected.feasibility, 0);
    assert_eq!(selected.levels[0].value, 10);
    assert_eq!(
        selected.levels[0]
            .category_breakdown
            .get(&eutheto_domain_ir::ScoreCategoryId::new(
                "official.workforce.score.preference.requested_time_off",
            )?),
        Some(&10)
    );
    assert!(!selected.levels[0].category_breakdown.contains_key(
        &eutheto_domain_ir::ScoreCategoryId::new("official.workforce.score.preference.location",)?
    ));
    Ok(())
}

#[test]
fn requested_time_off_is_half_open_and_filters_owner_type_and_effective_range() -> Result {
    let mut value = base()?;
    let entities = value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?;
    let mut other_person = entities[&id(1)].clone();
    other_person["id"] = json!(id(20));
    other_person["externalId"] = json!("staff-20");
    entities.insert(id(20), other_person);
    let mut other_type = entities[&id(4)].clone();
    other_type["id"] = json!(id(12));
    other_type["name"] = json!("Other");
    entities.insert(id(12), other_type);

    availability(
        &mut value,
        31,
        20,
        "requestedTimeOff",
        "2026-11-01T08:00:00Z",
        "2026-11-01T09:00:00Z",
    );
    availability(
        &mut value,
        32,
        1,
        "requestedTimeOff",
        "2026-11-01T08:00:00Z",
        "2026-11-01T09:00:00Z",
    );
    value["domain"]["entities"][id(32)]["assignmentTypeIds"] = json!([id(12)]);
    availability(
        &mut value,
        33,
        1,
        "requestedTimeOff",
        "2026-11-01T08:00:00Z",
        "2026-11-01T09:00:00Z",
    );
    value["domain"]["entities"][id(33)]["effectiveRange"] =
        json!({"startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    availability(
        &mut value,
        34,
        1,
        "requestedTimeOff",
        "2026-11-01T10:00:00Z",
        "2026-11-01T11:00:00Z",
    );
    requested_preference(
        &mut value,
        35,
        &[31, 32, 33, 34],
        &json!({"people":{"kind":"selected","personIds":[id(1)]}}),
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 0);
    let score = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 8, true)?])?,
        None,
        None,
    )?;
    assert_eq!(score.feasibility, 0);
    assert_eq!(score.levels[0].value, 0);

    let mut touching = serde_json::to_value(&document)?;
    touching["domain"]["entities"][id(34)]["timeWindow"]["startsAt"] =
        json!("2026-11-01T09:00:00Z");
    touching["domain"]["entities"][id(34)]["timeWindow"]["endsAt"] = json!("2026-11-01T10:00:00Z");
    let touching = decode_document(touching)?;
    let compiled = compile_workforce(&touching, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 1);
    let score = score_workforce_solution(
        &touching,
        &solution(&touching, vec![assignment(1, 8, true)?])?,
        None,
        None,
    )?;
    assert_eq!(score.levels[0].value, 10);
    Ok(())
}

#[test]
fn rejected_static_pair_has_no_preference_term_and_authority_quarantines_selection() -> Result {
    let mut value = base()?;
    availability(
        &mut value,
        31,
        1,
        "requestedTimeOff",
        "2026-11-01T08:00:00Z",
        "2026-11-01T09:00:00Z",
    );
    availability(
        &mut value,
        32,
        1,
        "unavailable",
        "2026-11-01T08:00:00Z",
        "2026-11-01T09:00:00Z",
    );
    requested_preference(&mut value, 33, &[31], &json!({"people":{"kind":"all"}}));
    value["domain"]["rules"][id(34)] = json!({
        "kind":"availability",
        "id":id(34),
        "active":true,
        "strength":"required",
        "scope":{"people":{"kind":"all"}}
    });
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert!(compiled.problem.projections.is_empty());
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 0);

    let score = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 8, true)?])?,
        None,
        None,
    )?;
    assert_eq!(score.feasibility, 1);
    assert_eq!(score.levels[0].value, 0);
    Ok(())
}

#[test]
fn assignment_target_retains_constant_shortfall_when_every_scoped_pair_is_infeasible() -> Result {
    let mut value = base()?;
    let mut available = value["domain"]["entities"][id(1)].clone();
    available["id"] = json!(id(20));
    available["externalId"] = json!("staff-20");
    available["name"] = json!("Sky");
    value["domain"]["entities"][id(20)] = available;
    availability(
        &mut value,
        31,
        1,
        "unavailable",
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
    value["domain"]["rules"][id(32)] = json!({
        "kind":"availability","id":id(32),"active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}}
    });
    value["domain"]["preferences"][id(33)] = json!({
        "kind":"assignmentTarget","id":id(33),"active":true,
        "scope":{"people":{"kind":"selected","personIds":[id(1)]}},"priority":"normal","weight":1,
        "calendarId":id(2),"targetCount":2
    });
    let document = decode_document(value)?;
    let compiled =
        compile_workforce(&document, &context()).map_err(|error| format!("compile: {error:?}"))?;
    let level = &compiled.problem.objectives.levels[0];
    assert_eq!(level.terms.len(), 1);
    assert_eq!(level.terms[0].expression.constant, 20);
    let score = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(20, 8, true)?])?,
        None,
        None,
    )
    .map_err(|error| format!("source score: {error:?}"))?;
    assert_eq!(score.feasibility, 0);
    assert_eq!(score.levels[0].value, 20);
    Ok(())
}

#[test]
fn assignment_target_charges_an_owned_calendar_period_without_matching_shifts() -> Result {
    let mut value = base()?;
    value["settings"]["horizon"]["end"] = json!("2026-11-03T00:00:00Z");
    value["domain"]["preferences"][id(33)] = json!({
        "kind":"assignmentTarget","id":id(33),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "calendarId":id(2),"targetCount":2
    });
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let level = &compiled.problem.objectives.levels[0];
    assert_eq!(level.terms.len(), 2);
    assert_eq!((level.lower_bound, level.upper_bound), (20, 40));
    let selected = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 8, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected.feasibility, 0);
    assert_eq!(selected.levels[0].value, 30);
    Ok(())
}

#[test]
fn independent_constant_target_levels_use_spans_for_exact_scalarization() -> Result {
    let mut value = base()?;
    availability(
        &mut value,
        31,
        1,
        "unavailable",
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
    value["domain"]["rules"][id(32)] = json!({
        "kind":"availability","id":id(32),"active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}}
    });
    value["domain"]["entities"][id(9)]["levels"] = json!([
        {"levelKey":"preferences","label":"Preferences"},
        {"levelKey":"secondary","label":"Secondary"}
    ]);
    for mapping in value["domain"]["entities"][id(9)]["priorityMapping"]
        .as_array_mut()
        .ok_or("priority mapping")?
    {
        if mapping["priority"] == "high" {
            mapping["levelKey"] = json!("secondary");
        }
    }
    for (index, priority) in [(33, "normal"), (34, "high")] {
        value["domain"]["preferences"][id(index)] = json!({
            "kind":"assignmentTarget","id":id(index),"active":true,
            "scope":{"people":{"kind":"all"}},"priority":priority,"weight":1_000_000,
            "calendarId":id(2),"targetCount":100_000
        });
    }
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let levels = &compiled.problem.objectives.levels;
    assert_eq!(
        (levels[0].lower_bound, levels[0].upper_bound),
        (1_000_000_000_000, 1_000_000_000_000)
    );
    assert_eq!(
        (levels[1].lower_bound, levels[1].upper_bound),
        (10_000_000_000_000, 10_000_000_000_000)
    );
    let score = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(score.levels[0].value, levels[0].lower_bound);
    assert_eq!(score.levels[1].value, levels[1].lower_bound);
    Ok(())
}

#[test]
fn preference_objective_budget_is_enforced() -> Result {
    let mut value = base()?;
    location_preference(&mut value, 33, "avoid", 1);
    let document = decode_document(value)?;
    let mut limited = context();
    limited.planning_limits.max_objective_terms = 1;
    assert!(matches!(
        compile_workforce(&document, &limited),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::PerRecord
        ))
    ));
    Ok(())
}

#[test]
fn workload_balance_keeps_empty_and_source_tail_periods_in_the_objective() -> Result {
    let mut value = base()?;
    value["settings"]["horizon"]["end"] = json!("2026-11-03T00:00:00Z");
    add_shift(
        &mut value,
        18,
        "2026-11-02T23:00:00Z",
        "2026-11-03T01:00:00Z",
    );
    value["domain"]["entities"][id(18)]["reportingAttribution"] = json!("endLocalDate");
    workload_preference(&mut value, 33, 1, 10);
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 3);

    let empty = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(empty.levels[0].value, 360);
    let selected = score_workforce_solution(
        &document,
        &solution(
            &document,
            vec![assignment(1, 8, true)?, assignment(1, 18, true)?],
        )?,
        None,
        None,
    )?;
    assert_eq!(selected.levels[0].value, 120);
    Ok(())
}

fn two_peer_workload(mode: &str) -> Result<ScenarioDocument> {
    let mut value = base()?;
    add_night_peer(&mut value);
    add_shift(
        &mut value,
        18,
        "2026-11-01T10:00:00Z",
        "2026-11-01T11:00:00Z",
    );
    value["domain"]["entities"][id(9)]["workloadPolicies"][id(10)]["targetMode"] =
        json!({"kind":mode});
    workload_preference(&mut value, 33, 1, 10);
    decode_document(value)
}

fn has_coefficients(
    compiled: &eutheto_planning_ir::PlanningProblem,
    left: i64,
    right: i64,
) -> bool {
    compiled.constraints.iter().any(|record| {
        let eutheto_planning_ir::Constraint::LinearComparison(comparison) = &record.body else {
            return false;
        };
        comparison
            .expression
            .terms
            .iter()
            .any(|term| term.coefficient == left)
            && comparison
                .expression
                .terms
                .iter()
                .any(|term| term.coefficient == right)
    })
}

#[test]
fn equal_share_source_score_and_ir_use_all_peers() -> Result {
    let document = two_peer_workload("equalShare")?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 2);
    assert!(has_coefficients(&compiled.problem, -1, 1));
    let score = score_workforce_solution(
        &document,
        &solution(
            &document,
            vec![assignment(1, 8, true)?, assignment(20, 18, true)?],
        )?,
        None,
        None,
    )?;
    assert_eq!(score.levels[0].value, 120);
    Ok(())
}

#[test]
fn weighted_equal_share_changes_target_with_peer_weight() -> Result {
    let document = two_peer_workload("weightedEqualShare")?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 2);
    assert!(has_coefficients(&compiled.problem, -2, 1));
    assert!(has_coefficients(&compiled.problem, -1, 2));
    let score = score_workforce_solution(
        &document,
        &solution(
            &document,
            vec![assignment(1, 8, true)?, assignment(20, 18, true)?],
        )?,
        None,
        None,
    )?;
    assert_eq!(score.levels[0].value, 360);
    Ok(())
}

#[test]
fn workload_preference_shift_filter_preserves_policy_peer_group() -> Result {
    let mut value = serde_json::to_value(two_peer_workload("equalShare")?)?;
    value["domain"]["preferences"][id(33)]["scope"]["weekdays"] = json!(["monday"]);
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 2);
    let selected = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 8, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected.levels[0].value, 0);
    Ok(())
}

#[test]
fn elapsed_workload_preserves_fractional_minutes_in_compiler_and_source_score() -> Result {
    let mut value = base()?;
    value["domain"]["entities"][id(8)]["endsAt"]["instant"] =
        json!("2026-11-01T10:00:00.000000001Z");
    value["domain"]["entities"][id(8)]["endsAt"]["local"] = json!("2026-11-01T10:00:00.000000001");
    workload_preference(&mut value, 33, 1, 10);
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].lower_bound, 0);
    assert_eq!(
        compiled.problem.objectives.levels[0].upper_bound,
        120 * 60_000_000_000
    );
    let selected = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 8, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected.levels[0].value, 1);
    Ok(())
}

#[test]
fn fractional_workload_normalizes_unrelated_preferences_in_same_priority_level() -> Result {
    let mut value = base()?;
    value["domain"]["entities"][id(8)]["endsAt"]["instant"] =
        json!("2026-11-01T10:00:00.000000001Z");
    value["domain"]["entities"][id(8)]["endsAt"]["local"] = json!("2026-11-01T10:00:00.000000001");
    workload_preference(&mut value, 33, 1, 10);
    location_preference(&mut value, 34, "avoid", 1);
    let document = decode_document(value)?;
    let solution = solution(&document, vec![assignment(1, 8, true)?])?;
    let selected = score_workforce_solution(&document, &solution, None, None)?;
    assert_eq!(selected.levels[0].value, 600_000_000_001);
    let focused = objective_context(
        &document,
        "official.workforce.objective.preference-focused",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let focused_score = score_workforce_solution(&document, &solution, Some(&focused), None)?;
    assert_eq!(
        focused_score.levels[1].value + focused_score.levels[2].value,
        selected.levels[0].value
    );
    // The nanosecond-separated lexicographic spans exceed the native signed scalar range.
    // Exercise compiler partition scaling with a half-minute quantum that fits that range.
    let mut bounded = serde_json::to_value(&document)?;
    bounded["domain"]["entities"][id(8)]["endsAt"]["instant"] = json!("2026-11-01T10:00:30Z");
    bounded["domain"]["entities"][id(8)]["endsAt"]["local"] = json!("2026-11-01T10:00:30");
    let bounded = decode_document(bounded)?;
    let mut focused_context = context();
    focused_context.objective_context = Some(focused);
    let focused_problem = compile_workforce(&bounded, &focused_context)?.problem;
    let legacy_problem = compile_workforce(&bounded, &context())?.problem;
    let split_terms = focused_problem.objectives.levels[1..3]
        .iter()
        .flat_map(|level| &level.terms)
        .collect::<Vec<_>>();
    assert_eq!(
        split_terms.len(),
        legacy_problem.objectives.levels[0].terms.len()
    );
    for term in split_terms {
        let legacy = legacy_problem.objectives.levels[0]
            .terms
            .iter()
            .find(|legacy| legacy.id == term.id)
            .ok_or("legacy preference term")?;
        assert_eq!(term.expression, legacy.expression);
        assert_eq!(term.category, legacy.category);
    }
    Ok(())
}

#[test]
fn nonconvex_weighted_share_and_unrelated_preference_keep_exact_common_level_units() -> Result {
    let mut value = serde_json::to_value(two_peer_workload("weightedEqualShare")?)?;
    value["domain"]["entities"][id(18)]["endsAt"] = json!({
        "instant":"2026-11-01T10:30:00Z",
        "local":"2026-11-01T10:30:00",
        "offsetSeconds":0
    });
    value["domain"]["entities"][id(9)]["workloadPolicies"][id(10)]["penalty"] = json!({"kind":"piecewise","breakpoints":[
        {"fromDeviation":0,"slope":3},{"fromDeviation":60,"slope":1}
    ]});
    location_preference(&mut value, 34, "avoid", 1);
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert!(compiled.problem.objectives.levels[0].terms.len() >= 3);
    let selected = score_workforce_solution(
        &document,
        &solution(
            &document,
            vec![assignment(1, 8, true)?, assignment(20, 18, true)?],
        )?,
        None,
        None,
    )?;
    // 120 and 30 minutes target 50 and 100: two 70-minute deviations cost
    // 3*60 + 1*10 each, scaled by the rational peer denominator 3. Location
    // Avoid's two contributions are scaled by that same priority denominator.
    assert_eq!(selected.levels[0].value, 1_200);
    Ok(())
}

#[test]
fn signed_scheduled_workload_preserves_fall_back_credit_in_compiler_and_score() -> Result {
    let mut value = base()?;
    value["settings"]["timeZone"] = json!("America/New_York");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T04:00:00Z","end":"2026-11-02T05:00:00Z"});
    value["domain"]["entities"][id(3)]["measurement"] = json!("scheduledMinutes");
    value["domain"]["entities"][id(3)]["overlappingContribution"] = json!("sum");
    value["domain"]["entities"][id(8)]["startsAt"] = json!({
        "instant":"2026-11-01T05:45:00Z",
        "local":"2026-11-01T01:45:00",
        "offsetSeconds":-14400
    });
    value["domain"]["entities"][id(8)]["endsAt"] = json!({
        "instant":"2026-11-01T06:15:00Z",
        "local":"2026-11-01T01:15:00",
        "offsetSeconds":-18000
    });
    value["domain"]["entities"][id(9)]["workloadPolicies"][id(10)]["targetMode"] =
        json!({"kind":"explicit","targets":[{"personId":id(1),"target":0}]});
    workload_preference(&mut value, 33, 1, 10);
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].lower_bound, 0);
    let empty = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(empty.levels[0].value, 0);
    let selected = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 8, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected.levels[0].value, 30);
    Ok(())
}

#[test]
fn consecutive_reporting_days_charge_windows_not_assignments_and_break_on_gaps() -> Result {
    let mut value = base()?;
    value["settings"]["horizon"]["end"] = json!("2026-11-05T00:00:00Z");
    add_shift(
        &mut value,
        18,
        "2026-11-01T10:00:00Z",
        "2026-11-01T11:00:00Z",
    );
    add_shift(
        &mut value,
        19,
        "2026-11-02T08:00:00Z",
        "2026-11-02T09:00:00Z",
    );
    add_shift(
        &mut value,
        20,
        "2026-11-03T08:00:00Z",
        "2026-11-03T09:00:00Z",
    );
    value["domain"]["preferences"][id(33)] = json!({
        "kind":"consecutiveWork","id":id(33),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "mode":{"kind":"reportingDays"},"assignmentTypeIds":[id(4)],"threshold":1
    });
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 2);
    let score = |selected: &[u32]| -> Result<i64> {
        let selected = selected
            .iter()
            .map(|shift| assignment(1, *shift, true))
            .collect::<Result<Vec<_>>>()?;
        Ok(
            score_workforce_solution(&document, &solution(&document, selected)?, None, None)?
                .levels[0]
                .value,
        )
    };
    assert_eq!(score(&[8, 18, 19, 20])?, 20);
    assert_eq!(score(&[8, 18, 19])?, 10);
    assert_eq!(score(&[8, 20])?, 0);
    assert_eq!(score(&[8])?, 0);
    Ok(())
}

#[test]
fn consecutive_weekends_use_week_calendar_and_actual_authored_weekday_overlap() -> Result {
    let mut value = base()?;
    value["settings"]["horizon"]["end"] = json!("2026-11-23T00:00:00Z");
    value["domain"]["entities"][id(2)]["period"] =
        json!({"kind":"week","anchorDate":"2026-11-02","startTime":"00:00:00"});
    add_shift(
        &mut value,
        18,
        "2026-11-07T08:00:00Z",
        "2026-11-07T09:00:00Z",
    );
    add_shift(
        &mut value,
        19,
        "2026-11-08T08:00:00Z",
        "2026-11-08T09:00:00Z",
    );
    add_shift(
        &mut value,
        20,
        "2026-11-14T08:00:00Z",
        "2026-11-14T09:00:00Z",
    );
    add_shift(
        &mut value,
        21,
        "2026-11-21T08:00:00Z",
        "2026-11-21T09:00:00Z",
    );
    add_shift(
        &mut value,
        22,
        "2026-11-07T10:00:00Z",
        "2026-11-07T11:00:00Z",
    );
    value["domain"]["preferences"][id(33)] = json!({
        "kind":"consecutiveWork","id":id(33),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "mode":{"kind":"weekends","calendarId":id(2),"weekdays":["saturday"]},
        "assignmentTypeIds":[id(4)],"threshold":1
    });
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 2);
    let score = |selected: &[u32]| -> Result<i64> {
        let selected = selected
            .iter()
            .map(|shift| assignment(1, *shift, true))
            .collect::<Result<Vec<_>>>()?;
        Ok(
            score_workforce_solution(&document, &solution(&document, selected)?, None, None)?
                .levels[0]
                .value,
        )
    };
    assert_eq!(score(&[18, 20, 21])?, 20);
    assert_eq!(score(&[18, 19, 20, 22])?, 10);
    assert_eq!(score(&[19, 20])?, 0);
    assert_eq!(score(&[18, 21])?, 0);
    Ok(())
}

#[test]
fn consecutive_zero_threshold_scores_each_terminal_source_date_once() -> Result {
    let mut value = base()?;
    value["settings"]["horizon"]["end"] = json!("2026-11-03T00:00:00Z");
    add_shift(
        &mut value,
        19,
        "2026-11-02T08:00:00Z",
        "2026-11-02T09:00:00Z",
    );
    value["domain"]["preferences"][id(33)] = json!({
        "kind":"consecutiveWork","id":id(33),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "mode":{"kind":"reportingDays"},"assignmentTypeIds":[id(4)],"threshold":0
    });
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 2);
    let selected = solution(
        &document,
        vec![assignment(1, 8, true)?, assignment(1, 19, true)?],
    )?;
    let score = score_workforce_solution(&document, &selected, None, None)?;
    assert_eq!(score.levels[0].value, 20);
    Ok(())
}

#[test]
fn consecutive_discarded_window_does_not_reject_ineligible_day_references() -> Result {
    let mut value = base()?;
    value["settings"]["horizon"]["end"] = json!("2026-11-03T00:00:00Z");
    for (id_value, start, end) in [
        (18, "2026-11-01T10:00:00Z", "2026-11-01T11:00:00Z"),
        (19, "2026-11-01T11:00:00Z", "2026-11-01T12:00:00Z"),
        (20, "2026-11-02T08:00:00Z", "2026-11-02T09:00:00Z"),
    ] {
        add_shift(&mut value, id_value, start, end);
    }
    value["domain"]["entities"][id(1)]["activeRange"] = json!({
        "kind":"dateRange","startDate":"2026-11-01","endDateExclusive":"2026-11-02"
    });
    value["domain"]["preferences"][id(33)] = json!({
        "kind":"consecutiveWork","id":id(33),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "mode":{"kind":"reportingDays"},"assignmentTypeIds":[id(4)],"threshold":1
    });
    let document = decode_document(value)?;
    let mut limits = context();
    limits.planning_limits.max_refs_per_node = 3;
    let compiled = compile_workforce(&document, &limits)?;
    assert!(compiled.problem.objectives.levels[0].terms.is_empty());
    let selected = solution(&document, vec![assignment(1, 8, true)?])?;
    let score = score_workforce_solution(&document, &selected, None, None)?;
    assert_eq!(score.levels[0].value, 0);
    Ok(())
}

#[test]
fn weekend_target_counts_distinct_source_weekends_and_nested_shift_tails() -> Result {
    let mut value = weekend_base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-07T08:00:00Z",
        "2026-11-07T09:00:00Z",
    );
    add_shift(
        &mut value,
        19,
        "2026-11-07T23:30:00Z",
        "2026-11-08T01:30:00Z",
    );
    add_shift(
        &mut value,
        20,
        "2026-11-14T08:00:00Z",
        "2026-11-14T09:00:00Z",
    );
    weekend_target_preference(
        &mut value,
        33,
        2,
        &json!({"people":{"kind":"all"}}),
        "normal",
        1,
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let level = &compiled.problem.objectives.levels[0];
    assert_eq!(level.terms.len(), 1);
    assert_eq!((level.lower_bound, level.upper_bound), (0, 20));

    let score = |selected: &[u32]| -> Result<i64> {
        let assignments = selected
            .iter()
            .map(|shift| assignment(1, *shift, true))
            .collect::<Result<Vec<_>>>()?;
        Ok(
            score_workforce_solution(&document, &solution(&document, assignments)?, None, None)?
                .levels[0]
                .value,
        )
    };
    assert_eq!(score(&[])?, 20);
    assert_eq!(score(&[18])?, 10);
    assert_eq!(score(&[19, 20])?, 0);
    assert_eq!(score(&[18, 19, 20])?, 0);
    Ok(())
}

#[test]
fn weekend_target_scores_each_scoped_person_independently() -> Result {
    let mut value = weekend_base()?;
    add_night_peer(&mut value);
    add_shift(
        &mut value,
        18,
        "2026-11-07T08:00:00Z",
        "2026-11-07T09:00:00Z",
    );
    add_shift(
        &mut value,
        21,
        "2026-11-14T08:00:00Z",
        "2026-11-14T09:00:00Z",
    );
    weekend_target_preference(
        &mut value,
        33,
        2,
        &json!({"people":{"kind":"all"}}),
        "normal",
        1,
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 2);
    let empty = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(empty.levels[0].value, 40);
    let one_person = score_workforce_solution(
        &document,
        &solution(
            &document,
            vec![assignment(1, 18, true)?, assignment(1, 21, true)?],
        )?,
        None,
        None,
    )?;
    assert_eq!(one_person.levels[0].value, 20);
    let both_people = score_workforce_solution(
        &document,
        &solution(
            &document,
            vec![
                assignment(1, 18, true)?,
                assignment(1, 21, true)?,
                assignment(20, 18, true)?,
                assignment(20, 21, true)?,
            ],
        )?,
        None,
        None,
    )?;
    assert_eq!(both_people.levels[0].value, 0);
    Ok(())
}

#[test]
fn weekend_target_zero_has_no_objective_when_no_weekend_is_reachable() -> Result {
    let mut value = weekend_base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-07T08:00:00Z",
        "2026-11-07T09:00:00Z",
    );
    weekend_target_preference(
        &mut value,
        33,
        0,
        &json!({"people":{"kind":"all"},"weekdays":["monday"]}),
        "normal",
        1,
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert!(compiled.problem.objectives.levels[0].terms.is_empty());
    let empty = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(empty.levels[0].value, 0);
    assert!(empty.levels[0].category_breakdown.is_empty());

    let mut reachable = weekend_base()?;
    add_shift(
        &mut reachable,
        18,
        "2026-11-07T08:00:00Z",
        "2026-11-07T09:00:00Z",
    );
    weekend_target_preference(
        &mut reachable,
        33,
        0,
        &json!({"people":{"kind":"all"}}),
        "normal",
        1,
    );
    let reachable = decode_document(reachable)?;
    let compiled = compile_workforce(&reachable, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 1);
    let selected = score_workforce_solution(
        &reachable,
        &solution(&reachable, vec![assignment(1, 18, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected.levels[0].value, 10);
    assert_eq!(
        selected.levels[0]
            .category_breakdown
            .values()
            .copied()
            .collect::<Vec<_>>(),
        vec![10]
    );
    Ok(())
}

#[test]
fn weekend_target_source_score_counts_a_pruned_source_weekend() -> Result {
    let mut value = weekend_base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-07T08:00:00Z",
        "2026-11-07T09:00:00Z",
    );
    add_shift(
        &mut value,
        20,
        "2026-11-14T08:00:00Z",
        "2026-11-14T09:00:00Z",
    );
    availability(
        &mut value,
        31,
        1,
        "unavailable",
        "2026-11-07T08:00:00Z",
        "2026-11-07T09:00:00Z",
    );
    value["domain"]["entities"][id(31)]["effectiveRange"] =
        json!({"startDate":"2026-11-07","endDateExclusive":"2026-11-08"});
    value["domain"]["rules"][id(32)] = json!({
        "kind":"availability","id":id(32),"active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}}
    });
    weekend_target_preference(
        &mut value,
        33,
        2,
        &json!({"people":{"kind":"all"}}),
        "normal",
        1,
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 1);
    let selected = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 18, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected.feasibility, 1);
    assert_eq!(selected.levels[0].value, 10);
    Ok(())
}

#[test]
fn weekend_target_uses_positive_elapsed_overlap_at_the_half_open_boundary() -> Result {
    let mut value = weekend_base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-06T23:00:00Z",
        "2026-11-07T00:00:00Z",
    );
    add_shift(
        &mut value,
        19,
        "2026-11-06T23:00:00Z",
        "2026-11-07T00:00:00.000000001Z",
    );
    weekend_target_preference(
        &mut value,
        33,
        1,
        &json!({"people":{"kind":"all"}}),
        "normal",
        1,
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 1);
    let empty = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(empty.levels[0].value, 10);
    let exact_boundary = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 18, true)?])?,
        None,
        None,
    )?;
    assert_eq!(exact_boundary.levels[0].value, 10);
    let positive_overlap = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 19, true)?])?,
        None,
        None,
    )?;
    assert_eq!(positive_overlap.levels[0].value, 0);
    Ok(())
}

#[test]
fn weekend_target_uses_local_windows_for_overnights_and_both_dst_transitions() -> Result {
    let mut value = weekend_base()?;
    value["settings"]["timeZone"] = json!("America/New_York");
    value["settings"]["horizon"] =
        json!({"start":"2026-03-01T05:00:00Z","end":"2026-11-10T05:00:00Z"});
    add_shift_with_offsets(
        &mut value,
        8,
        ("2026-11-02T13:00:00Z", "2026-11-02T08:00:00", -18000),
        ("2026-11-02T15:00:00Z", "2026-11-02T10:00:00", -18000),
    );
    add_shift_with_offsets(
        &mut value,
        18,
        ("2026-03-08T04:30:00Z", "2026-03-07T23:30:00", -18000),
        ("2026-03-08T07:30:00Z", "2026-03-08T03:30:00", -14400),
    );
    add_shift_with_offsets(
        &mut value,
        19,
        ("2026-11-01T03:30:00Z", "2026-10-31T23:30:00", -14400),
        ("2026-11-01T08:30:00Z", "2026-11-01T03:30:00", -18000),
    );
    add_shift_with_offsets(
        &mut value,
        20,
        ("2026-11-09T04:00:00Z", "2026-11-08T23:00:00", -18000),
        ("2026-11-09T06:00:00Z", "2026-11-09T01:00:00", -18000),
    );
    weekend_target_preference(
        &mut value,
        33,
        3,
        &json!({"people":{"kind":"all"}}),
        "normal",
        1,
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let level = &compiled.problem.objectives.levels[0];
    assert_eq!(level.terms.len(), 1);
    assert_eq!((level.lower_bound, level.upper_bound), (0, 30));
    let selected = score_workforce_solution(
        &document,
        &solution(
            &document,
            vec![
                assignment(1, 18, true)?,
                assignment(1, 19, true)?,
                assignment(1, 20, true)?,
            ],
        )?,
        None,
        None,
    )?;
    assert_eq!(selected.levels[0].value, 0);
    let missing_one = score_workforce_solution(
        &document,
        &solution(
            &document,
            vec![assignment(1, 18, true)?, assignment(1, 19, true)?],
        )?,
        None,
        None,
    )?;
    assert_eq!(missing_one.levels[0].value, 10);
    Ok(())
}

#[test]
fn weekend_target_counts_one_shift_in_each_weekend_it_spans() -> Result {
    let mut value = weekend_base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-07T23:00:00Z",
        "2026-11-15T01:00:00Z",
    );
    weekend_target_preference(
        &mut value,
        33,
        2,
        &json!({"people":{"kind":"all"}}),
        "normal",
        1,
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let level = &compiled.problem.objectives.levels[0];
    assert_eq!(level.terms.len(), 1);
    assert_eq!((level.lower_bound, level.upper_bound), (0, 20));
    let selected = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 18, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected.levels[0].value, 0);
    Ok(())
}

#[test]
fn weekend_target_scope_uses_reporting_weekday_for_a_friday_shift_tail() -> Result {
    let mut value = weekend_base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-06T23:00:00Z",
        "2026-11-07T01:00:00Z",
    );
    add_shift(
        &mut value,
        19,
        "2026-11-07T08:00:00Z",
        "2026-11-07T09:00:00Z",
    );
    weekend_target_preference(
        &mut value,
        33,
        1,
        &json!({
            "people":{"kind":"all"},
            "weekdays":["friday"]
        }),
        "normal",
        1,
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(compiled.problem.objectives.levels[0].terms.len(), 1);
    let friday_tail = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 18, true)?])?,
        None,
        None,
    )?;
    assert_eq!(friday_tail.levels[0].value, 0);
    let saturday_shift = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 19, true)?])?,
        None,
        None,
    )?;
    assert_eq!(saturday_shift.levels[0].value, 10);
    Ok(())
}

#[test]
fn weekend_target_uses_priority_scale_and_weight_in_source_and_ir() -> Result {
    let mut value = weekend_base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-07T08:00:00Z",
        "2026-11-07T09:00:00Z",
    );
    weekend_target_preference(
        &mut value,
        33,
        1,
        &json!({"people":{"kind":"all"}}),
        "high",
        2,
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let level = &compiled.problem.objectives.levels[0];
    assert_eq!((level.lower_bound, level.upper_bound), (0, 200));
    let empty = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(empty.levels[0].value, 200);
    assert_eq!(
        empty.levels[0]
            .category_breakdown
            .get(&eutheto_domain_ir::ScoreCategoryId::new(
                "official.workforce.score.preference.weekend_target",
            )?),
        Some(&200)
    );
    let selected = score_workforce_solution(
        &document,
        &solution(&document, vec![assignment(1, 18, true)?])?,
        None,
        None,
    )?;
    assert_eq!(selected.levels[0].value, 0);
    Ok(())
}

#[test]
fn weekend_target_has_no_penalty_for_an_empty_person_scope() -> Result {
    let mut value = weekend_base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-07T08:00:00Z",
        "2026-11-07T09:00:00Z",
    );
    weekend_target_preference(
        &mut value,
        33,
        2,
        &json!({
            "people":{"kind":"filter","allTags":["not-in-this-fixture"],"anyTags":[]}
        }),
        "normal",
        1,
    );
    let document = decode_document(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert!(compiled.problem.objectives.levels[0].terms.is_empty());
    let score = score_workforce_solution(&document, &solution(&document, Vec::new())?, None, None)?;
    assert_eq!(score.levels[0].value, 0);
    assert!(score.levels[0].category_breakdown.is_empty());
    Ok(())
}

#[test]
fn authored_objective_admission_preserves_legacy_soft_lock_boundary() -> Result {
    let ordinary = decode_document(base()?)?;
    let authored = RunObjectiveContext::authored(ordinary.domain_pack.id.clone())?;
    let ordinary_solution = solution(&ordinary, Vec::new())?;
    assert_eq!(
        score_workforce_solution(&ordinary, &ordinary_solution, None, None)?,
        score_workforce_solution(&ordinary, &ordinary_solution, Some(&authored), None)?,
    );
    let mut unknown_profile = authored.clone();
    unknown_profile.profile_id = ObjectiveProfileId::new("eutheto.objective.unknown")?;
    assert!(matches!(
        score_workforce_solution(
            &ordinary,
            &ordinary_solution,
            Some(&unknown_profile),
            None
        ),
        Err(DomainPackError::InvalidPayload { path, .. }) if path == "objectiveContext"
    ));

    let mut value = base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-01T10:00:00Z",
        "2026-11-01T12:00:00Z",
    );
    value["domain"]["lockedAssignments"][id(40)] = json!({
        "id":id(40),
        "personId":id(1),
        "shiftId":id(18),
        "state":{"kind":"soft","stabilityWeight":1}
    });
    let document = decode_document(value)?;
    let solution = solution(&document, Vec::new())?;
    let lock_id: RuleId = id(40).parse()?;
    let authored = RunObjectiveContext::authored(document.domain_pack.id.clone())?;

    assert!(matches!(
        compile_workforce(&document, &context()),
        Err(AssignmentRuleError::UnsupportedObligation(_))
    ));
    let mut current_compile_context = context();
    current_compile_context.objective_context = Some(authored.clone());
    assert!(matches!(
        compile_workforce(&document, &current_compile_context),
        Err(AssignmentRuleError::InvalidDocument(DomainPackError::InvalidPayload {
            path,
            ..
        })) if path == "objectiveContext"
    ));
    let scope = workforce_verification_scope(&document, 1, None)?;
    assert!(
        !scope
            .required_rules
            .iter()
            .any(|binding| binding.rule_id == lock_id)
    );
    assert!(matches!(
        score_workforce_solution(&document, &solution, None, None),
        Err(DomainPackError::Contract(code))
            if code == "official.workforce.compile.unsupported_obligation"
    ));
    assert!(matches!(
        score_workforce_solution(&document, &solution, Some(&authored), None),
        Err(DomainPackError::InvalidPayload { path, .. }) if path == "objectiveContext"
    ));
    Ok(())
}

fn four_mapped_levels(value: &mut Value) {
    let mut levels = vec![
        json!({"levelKey":"zeta","label":"Zeta"}),
        json!({"levelKey":"alpha","label":"Alpha"}),
        json!({"levelKey":"omega","label":"Omega"}),
        json!({"levelKey":"beta","label":"Beta"}),
    ];
    for index in 0..11 {
        levels.push(json!({
            "levelKey":format!("unmapped_{index}"),
            "label":"Unmapped"
        }));
    }
    value["domain"]["entities"][id(9)]["levels"] = Value::Array(levels);
    value["domain"]["entities"][id(9)]["priorityMapping"] = json!([
        {"priority":"low","levelKey":"zeta","scale":1},
        {"priority":"normal","levelKey":"alpha","scale":10},
        {"priority":"high","levelKey":"omega","scale":100},
        {"priority":"veryHigh","levelKey":"beta","scale":1000}
    ]);
}

fn named_profile_fixture() -> Result<(Value, ScenarioDocument, RunBaselineBinding)> {
    let mut value = base()?;
    four_mapped_levels(&mut value);
    add_shift(
        &mut value,
        18,
        "2026-11-01T10:00:00Z",
        "2026-11-01T12:00:00Z",
    );
    location_preference(&mut value, 33, "prefer", 1);
    workload_preference(&mut value, 34, 1, 10);
    value["domain"]["lockedAssignments"][id(40)] = json!({
        "id":id(40),
        "personId":id(1),
        "shiftId":id(18),
        "state":{"kind":"soft","stabilityWeight":3}
    });
    let (document, baseline) = document_with_original_baseline(value.clone(), &[pair(1, 8)?])?;
    Ok((value, document, baseline))
}

fn manual_baseline_fixture(
    value: Value,
    assignments: Vec<AssignmentPair>,
) -> Result<(ScenarioDocument, RunBaselineBinding)> {
    let (mut document, original) = document_with_original_baseline(value, &[pair(1, 8)?])?;
    let mut manual = ManualSelection {
        schema_version: 1,
        id: id(50).parse()?,
        original: original.original.clone(),
        admission_source_scenario_id: document.scenario_id,
        admission_source_revision: Revision::INITIAL,
        admission_source_document_hash: "e".repeat(64),
        admission_command_id: id(51).parse()?,
        assignments,
        checksum: String::new(),
    };
    manual.checksum = workforce_manual_selection_checksum(&manual, None)?;
    document.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(manual.clone()))?,
    );
    let binding = RunBaselineBinding {
        captured_original: None,
        original: manual.original.clone(),
        selection: RunBaselineSelection::ManualSelection {
            entity: workforce_baseline_reference(manual.id, true)?,
            record_checksum: manual.checksum.clone(),
            admission_source_scenario_id: manual.admission_source_scenario_id,
            admission_source_revision: manual.admission_source_revision.value(),
            admission_source_document_hash: manual.admission_source_document_hash.clone(),
        },
        selected_pair_content_hash: workforce_pair_content_hash(&manual.assignments, None)?,
    };
    Ok((document, binding))
}

fn compile_level_ids(
    document: &ScenarioDocument,
    objective: &RunObjectiveContext,
) -> Result<Vec<String>> {
    let mut compile_context = context();
    compile_context.objective_context = Some(objective.clone());
    Ok(compile_workforce(document, &compile_context)?
        .problem
        .objectives
        .levels
        .into_iter()
        .map(|level| level.id.as_str().to_owned())
        .collect())
}

fn split_profile_level(profile: &str, partition: &str, key: &str) -> String {
    let digest = blake3::hash(key.as_bytes()).to_hex();
    format!("official.workforce.objective.preference.{profile}.{partition}.{digest}")
}

#[test]
fn named_profiles_keep_declared_tiers_and_eleven_level_capacity() -> Result {
    let (_, document, baseline) = named_profile_fixture()?;
    let balanced = objective_context(
        &document,
        "official.workforce.objective.balanced",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let focused = objective_context(
        &document,
        "official.workforce.objective.preference-focused",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let fairness = objective_context(
        &document,
        "official.workforce.objective.fairness-first",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let minimal = objective_context(
        &document,
        "official.workforce.objective.minimal-changes",
        RunObjectiveOperation::Optimize,
        Some(baseline.clone()),
    )?;
    let repair = objective_context(
        &document,
        "official.workforce.objective.preference-focused",
        RunObjectiveOperation::Repair,
        Some(baseline.clone()),
    )?;
    let balanced_ids = compile_level_ids(&document, &balanced)?;
    assert_eq!(
        balanced_ids,
        [
            "official.workforce.objective.soft_lock",
            "official.workforce.objective.preference.zeta",
            "official.workforce.objective.preference.alpha",
            "official.workforce.objective.preference.omega",
            "official.workforce.objective.preference.beta",
            "official.workforce.objective.assignment.rank",
        ]
    );
    let focused_ids = compile_level_ids(&document, &focused)?;
    let focused_halves = [
        split_profile_level("preference_focused", "non_workload", "zeta"),
        split_profile_level("preference_focused", "non_workload", "alpha"),
        split_profile_level("preference_focused", "non_workload", "omega"),
        split_profile_level("preference_focused", "non_workload", "beta"),
        split_profile_level("preference_focused", "workload_balance", "zeta"),
        split_profile_level("preference_focused", "workload_balance", "alpha"),
        split_profile_level("preference_focused", "workload_balance", "omega"),
        split_profile_level("preference_focused", "workload_balance", "beta"),
    ];
    assert_eq!(&focused_ids[1..9], focused_halves);
    assert_eq!(focused_halves.into_iter().collect::<BTreeSet<_>>().len(), 8);
    let fairness_ids = compile_level_ids(&document, &fairness)?;
    assert_eq!(
        &fairness_ids[1..9],
        [
            split_profile_level("fairness_first", "workload_balance", "zeta"),
            split_profile_level("fairness_first", "workload_balance", "alpha"),
            split_profile_level("fairness_first", "workload_balance", "omega"),
            split_profile_level("fairness_first", "workload_balance", "beta"),
            split_profile_level("fairness_first", "non_workload", "zeta"),
            split_profile_level("fairness_first", "non_workload", "alpha"),
            split_profile_level("fairness_first", "non_workload", "omega"),
            split_profile_level("fairness_first", "non_workload", "beta"),
        ]
    );
    assert_eq!(compile_level_ids(&document, &minimal)?.len(), 7);
    assert_eq!(compile_level_ids(&document, &repair)?.len(), 11);
    let minimal_repair = objective_context(
        &document,
        "official.workforce.objective.minimal-changes",
        RunObjectiveOperation::Repair,
        Some(baseline),
    )?;
    let minimal_repair_ids = compile_level_ids(&document, &minimal_repair)?;
    assert_eq!(
        minimal_repair_ids[0],
        "official.workforce.objective.pair_change"
    );
    assert_eq!(minimal_repair_ids.len(), 7);
    assert_eq!(
        minimal_repair_ids
            .iter()
            .filter(|id| id.as_str() == "official.workforce.objective.pair_change")
            .count(),
        1
    );
    Ok(())
}

#[test]
fn named_profile_score_metadata_matches_compiled_level_order() -> Result {
    let (_, document, baseline) = named_profile_fixture()?;
    let balanced = objective_context(
        &document,
        "official.workforce.objective.balanced",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let focused = objective_context(
        &document,
        "official.workforce.objective.preference-focused",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let fairness = objective_context(
        &document,
        "official.workforce.objective.fairness-first",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let minimal = objective_context(
        &document,
        "official.workforce.objective.minimal-changes",
        RunObjectiveOperation::Optimize,
        Some(baseline.clone()),
    )?;
    let repair = objective_context(
        &document,
        "official.workforce.objective.preference-focused",
        RunObjectiveOperation::Repair,
        Some(baseline),
    )?;
    let empty = solution(&document, Vec::new())?;
    for objective in [&balanced, &focused, &fairness, &minimal, &repair] {
        named_profile_score_matches_model(&document, objective, &empty)?;
    }
    Ok(())
}

#[test]
fn named_profile_soft_change_scores_and_source_binding_are_exact() -> Result {
    let (_, document, baseline) = named_profile_fixture()?;
    let balanced = objective_context(
        &document,
        "official.workforce.objective.balanced",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let repair = objective_context(
        &document,
        "official.workforce.objective.preference-focused",
        RunObjectiveOperation::Repair,
        Some(baseline),
    )?;
    let empty = solution(&document, Vec::new())?;
    let selected = solution(&document, vec![assignment(1, 18, true)?])?;
    assert_eq!(
        score_workforce_solution(&document, &empty, Some(&balanced), None)?.levels[0].value,
        3
    );
    assert_eq!(
        score_workforce_solution(&document, &selected, Some(&balanced), None)?.levels[0].value,
        0
    );
    assert_eq!(
        score_workforce_solution(&document, &selected, Some(&repair), None)?.levels[0].value,
        2
    );
    let mut changed_value = serde_json::to_value(&document)?;
    changed_value["domain"]["entities"][id(20)]["assignments"] = json!([pair(1, 18)?]);
    let changed_document = decode_document(changed_value)?;
    assert!(score_workforce_solution(&changed_document, &selected, Some(&repair), None).is_err());

    Ok(())
}

#[test]
fn named_profile_empty_and_missing_baselines_remain_distinct() -> Result {
    let (value, _, _) = named_profile_fixture()?;
    let (empty_document, empty_binding) = document_with_original_baseline(value, &[])?;
    let explicit_empty = objective_context(
        &empty_document,
        "official.workforce.objective.minimal-changes",
        RunObjectiveOperation::Optimize,
        Some(empty_binding),
    )?;
    let selected = solution(&empty_document, vec![assignment(1, 18, true)?])?;
    assert_eq!(
        score_workforce_solution(&empty_document, &selected, Some(&explicit_empty), None)?.levels
            [0]
        .value,
        1
    );
    let missing = objective_context(
        &empty_document,
        "official.workforce.objective.minimal-changes",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let mut compile_context = context();
    compile_context.objective_context = Some(missing.clone());
    assert!(compile_workforce(&empty_document, &compile_context).is_err());
    assert!(score_workforce_solution(&empty_document, &selected, Some(&missing), None).is_err());

    Ok(())
}

#[test]
fn external_starting_plan_distance_uses_current_pairs_and_rejects_stale_or_missing_sources()
-> Result {
    let (mut value, _, _) = named_profile_fixture()?;
    let imported = json!({
        "sourceBlake3": "a".repeat(64),
        "pairContentHash": "b".repeat(64)
    });
    value["domain"]["entities"][id(50)] = json!({
        "kind":"externalStartingPlan", "schemaVersion":1, "id":id(50),
        "name":"Imported plan", "assignments":[pair(1, 8)?],
        "originalImport":imported
    });
    let document = decode_document(value.clone())?;
    let mut objective = objective_context(
        &document,
        "official.workforce.objective.minimal-changes",
        RunObjectiveOperation::Repair,
        None,
    )?;
    objective.schema_version = 3;
    objective.external_baseline = Some(Box::new(RunExternalBaselineBinding {
        entity: workforce_external_baseline_reference(id(50).parse()?)?,
        selected_pair_content_hash: workforce_pair_content_hash(&[pair(1, 8)?], None)?,
    }));
    let mut compile_context = context();
    compile_context.objective_context = Some(objective.clone());
    for (assignments, expected) in [
        (vec![assignment(1, 8, true)?], 0),
        (Vec::new(), 1),
        (vec![assignment(1, 18, true)?], 2),
    ] {
        let candidate = solution(&document, assignments)?;
        assert_eq!(
            score_workforce_solution(&document, &candidate, Some(&objective), None)?.levels[0]
                .value,
            expected
        );
    }
    value["domain"]["entities"][id(50)]["assignments"] = json!([]);
    let edited = decode_document(value.clone())?;
    assert!(compile_workforce(&edited, &compile_context).is_err());
    let empty = solution(&edited, Vec::new())?;
    assert!(score_workforce_solution(&edited, &empty, Some(&objective), None).is_err());
    objective
        .external_baseline
        .as_mut()
        .ok_or("external binding")?
        .selected_pair_content_hash = workforce_pair_content_hash(&[], None)?;
    assert_eq!(
        score_workforce_solution(&edited, &empty, Some(&objective), None)?.levels[0].value,
        0
    );
    assert_eq!(
        edited.domain.entities[&id(50).parse()?]["originalImport"],
        imported
    );
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(50));
    let missing = decode_document(value)?;
    assert!(score_workforce_solution(&missing, &empty, Some(&objective), None).is_err());
    Ok(())
}

#[test]
fn manual_and_original_profiles_bind_distinct_complete_pair_snapshots() -> Result {
    let (value, _, original_binding) = named_profile_fixture()?;
    let original_context = objective_context(
        &decode_document(value.clone())?,
        "official.workforce.objective.minimal-changes",
        RunObjectiveOperation::Optimize,
        Some(original_binding),
    )?;
    let (manual_document, manual_binding) = manual_baseline_fixture(value, vec![pair(1, 18)?])?;
    let manual_context = objective_context(
        &manual_document,
        "official.workforce.objective.minimal-changes",
        RunObjectiveOperation::Optimize,
        Some(manual_binding.clone()),
    )?;
    let original_document = decode_document(serde_json::to_value(&manual_document)?)?;
    let selected = solution(&manual_document, vec![assignment(1, 18, true)?])?;
    assert_eq!(
        score_workforce_solution(&manual_document, &selected, Some(&manual_context), None)?.levels
            [0]
        .value,
        0
    );
    assert_eq!(
        score_workforce_solution(&original_document, &selected, Some(&original_context), None)?
            .levels[0]
            .value,
        2
    );
    let mut stale_manual = manual_document.clone();
    let raw = stale_manual
        .domain
        .entities
        .get_mut(&id(50).parse()?)
        .ok_or("manual selection")?;
    let WorkforceEntity::ManualSelection(mut record) = serde_json::from_value(raw.clone())? else {
        return Err("manual selection kind".into());
    };
    record.assignments.clear();
    record.checksum = workforce_manual_selection_checksum(&record, None)?;
    let updated_checksum = record.checksum.clone();
    *raw = serde_json::to_value(WorkforceEntity::ManualSelection(record))?;
    let mut stale_context = manual_context.clone();
    let baseline = stale_context.baseline.as_mut().ok_or("manual baseline")?;
    let RunBaselineSelection::ManualSelection {
        record_checksum, ..
    } = &mut baseline.selection
    else {
        return Err("expected manual baseline".into());
    };
    *record_checksum = updated_checksum;
    assert!(
        score_workforce_solution(&stale_manual, &selected, Some(&stale_context), None).is_err()
    );
    Ok(())
}

fn named_profile_score_matches_model(
    document: &ScenarioDocument,
    objective: &RunObjectiveContext,
    solution: &NormalizedSolution,
) -> Result<eutheto_domain_ir::ScoreVector> {
    let mut compile_context = context();
    compile_context.objective_context = Some(objective.clone());
    let problem = compile_workforce(document, &compile_context)?.problem;
    let score = score_workforce_solution(document, solution, Some(objective), None)?;
    assert_eq!(score.levels.len(), problem.objectives.levels.len());
    for (score_level, objective_level) in score.levels.iter().zip(&problem.objectives.levels) {
        assert_eq!(score_level.level_id.as_str(), objective_level.id.as_str());
        assert_eq!(score_level.direction, objective_level.direction);
    }
    let document_bytes = serde_json::to_vec(document)?;
    let verification_scope = workforce_verification_scope(document, 1, None)?;
    let verification_context = VerificationContextV1::new(
        document.scenario_id,
        1,
        blake3_hex(&document_bytes),
        canonical_ir_hash(&problem, PlanningIrLimitsV1::DEFAULT)?,
        solution.canonical_hash()?,
        verification_scope.checksum,
    )?;
    verify_workforce_solution(
        document,
        solution,
        Some(objective),
        &verification_context,
        &score,
        None,
    )?;
    Ok(score)
}

#[test]
fn named_profiles_keep_inactive_candidate_and_zero_score_shapes_in_sync() -> Result {
    let mut inactive_value = base()?;
    let entities = inactive_value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?;
    assert!(entities.remove(&id(9)).is_some());
    let inactive_document = decode_document(inactive_value.clone())?;
    let objective = objective_context(
        &inactive_document,
        "official.workforce.objective.balanced",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let (minimal_document, baseline) =
        document_with_original_baseline(inactive_value.clone(), &[pair(1, 8)?])?;
    let repair = objective_context(
        &minimal_document,
        "official.workforce.objective.preference-focused",
        RunObjectiveOperation::Repair,
        Some(baseline.clone()),
    )?;
    let repaired = named_profile_score_matches_model(
        &minimal_document,
        &repair,
        &solution(&minimal_document, Vec::new())?,
    )?;
    assert_eq!(
        repaired.levels[0].level_id.as_str(),
        "official.workforce.objective.pair_change"
    );
    assert_eq!(repaired.levels[0].value, 1);
    let minimal = named_profile_score_matches_model(
        &minimal_document,
        &objective_context(
            &minimal_document,
            "official.workforce.objective.minimal-changes",
            RunObjectiveOperation::Optimize,
            Some(baseline),
        )?,
        &solution(&minimal_document, Vec::new())?,
    )?;
    assert_eq!(
        minimal.levels[0].level_id.as_str(),
        "official.workforce.objective.pair_change"
    );
    assert_eq!(minimal.levels[0].value, 1);
    let empty = solution(&inactive_document, Vec::new())?;
    let inactive = named_profile_score_matches_model(&inactive_document, &objective, &empty)?;
    assert!(
        inactive.levels.iter().all(|level| level.value == 0)
            && inactive.levels[0].level_id.as_str() == "official.workforce.objective.soft_lock"
    );

    availability(
        &mut inactive_value,
        42,
        1,
        "unavailable",
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
    let no_candidate_document = decode_document(inactive_value)?;
    let no_candidates = named_profile_score_matches_model(
        &no_candidate_document,
        &objective_context(
            &no_candidate_document,
            "official.workforce.objective.balanced",
            RunObjectiveOperation::Optimize,
            None,
        )?,
        &solution(&no_candidate_document, Vec::new())?,
    )?;
    assert!(
        no_candidates.levels.iter().all(|level| level.value == 0)
            && no_candidates.levels[0].level_id.as_str()
                == "official.workforce.objective.soft_lock"
    );

    let mut active_value = base()?;
    location_preference(&mut active_value, 33, "prefer", 1);
    let active_document = decode_document(active_value)?;
    let active_objective = objective_context(
        &active_document,
        "official.workforce.objective.balanced",
        RunObjectiveOperation::Optimize,
        None,
    )?;
    let active = named_profile_score_matches_model(
        &active_document,
        &active_objective,
        &solution(&active_document, vec![assignment(1, 8, true)?])?,
    )?;
    assert!(active.levels.iter().any(|level| level.level_id.as_str()
        == "official.workforce.objective.preference.preferences"
        && level.value == 0));
    Ok(())
}

fn profile_evidence_fixture() -> Result<(ScenarioDocument, RunObjectiveContext)> {
    let mut value = base()?;
    four_mapped_levels(&mut value);
    add_shift(
        &mut value,
        18,
        "2026-11-01T10:00:00Z",
        "2026-11-01T12:00:00Z",
    );
    availability(
        &mut value,
        42,
        1,
        "unavailable",
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
    location_preference(&mut value, 33, "prefer", 1);
    value["domain"]["lockedAssignments"][id(40)] = json!({
        "id":id(40),"personId":id(1),"shiftId":id(18),
        "state":{"kind":"soft","stabilityWeight":3}
    });
    let (document, baseline) = document_with_original_baseline(value, &[pair(1, 8)?])?;
    let objective = objective_context(
        &document,
        "official.workforce.objective.preference-focused",
        RunObjectiveOperation::Repair,
        Some(baseline),
    )?;
    Ok((document, objective))
}

fn compiled_profile_contributions(
    document: &ScenarioDocument,
    objective: &RunObjectiveContext,
) -> Result<Vec<ScoreContributionV1>> {
    let mut compile_context = context();
    compile_context.objective_context = Some(objective.clone());
    let problem = compile_workforce(document, &compile_context)?.problem;
    let pair_change = problem
        .objectives
        .levels
        .iter()
        .find(|level| level.id.as_str() == "official.workforce.objective.pair_change")
        .ok_or("pair-change level")?;
    let pair_change_term = pair_change
        .terms
        .iter()
        .find(|term| {
            term.expression
                .terms
                .first()
                .is_some_and(|term| term.coefficient == 1)
        })
        .ok_or("new pair-change term")?;
    let variable = pair_change_term.expression.terms[0].variable.clone();
    let contribution = |level_id: &str, term: &eutheto_planning_ir::ObjectiveTerm| {
        let value = term
            .expression
            .terms
            .iter()
            .filter(|term| term.variable == variable)
            .try_fold(term.expression.constant, |sum, term| {
                sum.checked_add(term.coefficient)
            })
            .ok_or("score contribution overflow")?;
        Ok::<_, Box<dyn Error>>(ScoreContributionV1 {
            evidence_id: term.provenance.as_str().parse()?,
            level_id: level_id.parse()?,
            category_id: Some(term.category.clone()),
            value,
        })
    };
    let soft = problem
        .objectives
        .levels
        .iter()
        .find(|level| level.id.as_str() == "official.workforce.objective.soft_lock")
        .ok_or("soft-lock level")?;
    let location = problem
        .objectives
        .levels
        .iter()
        .find(|level| {
            level.id.as_str().contains("non_workload")
                && level.terms.iter().any(|term| {
                    term.expression
                        .terms
                        .iter()
                        .any(|term| term.variable == variable)
                })
        })
        .ok_or("preference-focused location level")?;
    let mut contributions = vec![
        contribution(pair_change.id.as_str(), pair_change_term)?,
        contribution(
            soft.id.as_str(),
            soft.terms
                .iter()
                .find(|term| {
                    term.expression
                        .terms
                        .iter()
                        .any(|term| term.variable == variable)
                })
                .ok_or("soft-lock term")?,
        )?,
        contribution(
            location.id.as_str(),
            location
                .terms
                .iter()
                .find(|term| {
                    term.expression
                        .terms
                        .iter()
                        .any(|term| term.variable == variable)
                })
                .ok_or("location term")?,
        )?,
    ];
    contributions.sort_by(|left, right| left.evidence_id.cmp(&right.evidence_id));
    Ok(contributions)
}

fn assignment_evidence_request(evidence: AssignmentEvidenceV1) -> Result<EvidenceRenderRequestV2> {
    Ok(EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Assignment {
            assignment: evidence,
        },
    )?)?)
}

fn profile_assignment_evidence(
    contributions: Vec<ScoreContributionV1>,
) -> Result<AssignmentEvidenceV1> {
    Ok(AssignmentEvidenceV1 {
        assignment: evidence_assignment(1, 18, true)?,
        related_rules: Vec::new(),
        score_contributions: contributions,
        metrics: BTreeMap::new(),
        lock_state: None,
    })
}

fn assert_rendered_profile_contributions(
    messages: &[EvidenceMessageV1],
    contributions: &[ScoreContributionV1],
    assignment_id: &DomainAssignmentId,
) -> Result {
    let rendered = messages
        .iter()
        .filter(|message| {
            message.message_key == "official.workforce.assignment.objective_contribution"
        })
        .collect::<Vec<_>>();
    assert_eq!(rendered.len(), contributions.len());
    let level_fact = "official.workforce.fact.objective_level".parse()?;
    let category_fact = "official.workforce.fact.objective_category".parse()?;
    let value_fact = "official.workforce.fact.objective_value".parse()?;
    for (message, contribution) in rendered.iter().zip(contributions) {
        assert_eq!(message.assignments, vec![assignment_id.clone()]);
        assert_eq!(message.evidence, vec![contribution.evidence_id.clone()]);
        assert_eq!(
            message.parameters.get(&level_fact),
            Some(&VerificationValue::Text(
                contribution.level_id.as_str().to_owned()
            ))
        );
        assert_eq!(
            message.parameters.get(&category_fact),
            contribution
                .category_id
                .as_ref()
                .map(|category| VerificationValue::Text(category.as_str().to_owned()))
                .as_ref()
        );
        assert_eq!(
            message.parameters.get(&value_fact),
            Some(&VerificationValue::Integer(contribution.value))
        );
    }
    Ok(())
}

#[test]
fn native_assignment_render_derives_typed_profile_contributions() -> Result {
    let (document, objective) = profile_evidence_fixture()?;
    let expected = compiled_profile_contributions(&document, &objective)?;
    let evidence = profile_assignment_evidence(Vec::new())?;
    let request = assignment_evidence_request(evidence)?;
    let rendered = render_workforce_evidence(
        &document,
        &request,
        Some(&objective),
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_rendered_profile_contributions(
        &rendered.messages,
        &expected,
        &assignment(1, 18, true)?.id,
    )?;
    Ok(())
}

#[test]
fn supplied_profile_contributions_are_source_checked() -> Result {
    let (document, objective) = profile_evidence_fixture()?;
    let contributions = compiled_profile_contributions(&document, &objective)?;
    let evidence = profile_assignment_evidence(contributions.clone())?;
    let request = assignment_evidence_request(evidence.clone())?;
    let rendered = render_workforce_evidence(
        &document,
        &request,
        Some(&objective),
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_rendered_profile_contributions(
        &rendered.messages,
        &contributions,
        &evidence.assignment.id,
    )?;

    let mut forged = evidence;
    forged.score_contributions[0].value += 1;
    let forged = assignment_evidence_request(forged)?;
    assert!(
        render_workforce_evidence(
            &document,
            &forged,
            Some(&objective),
            &OperationControl::Cancellation(CancellationToken::new()),
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn historical_workforce_objectives_and_source_scores_replay_exactly() -> Result {
    // Captured from the original Rust writers; do not regenerate from current constructors.
    let captures: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/pre_profile_semantics.json"))?;
    for capture in &captures {
        let text = |key: &str| -> Result<&str> {
            capture[key]
                .as_str()
                .ok_or_else(|| "invalid historic fixture field".into())
        };
        let document = decode_document(serde_json::from_str(text("documentJson")?)?)?;
        let normalized_solution: NormalizedSolution = serde_json::from_str(text("solutionJson")?)?;
        let compiled = compile_workforce(&document, &context())?;
        let model_hash = canonical_ir_hash(&compiled.problem, PlanningIrLimitsV1::DEFAULT)?;
        let objective_levels = compiled
            .problem
            .objectives
            .levels
            .iter()
            .map(|level| json!({"id":level.id.as_str(),"direction":level.direction}))
            .collect::<Vec<_>>();
        assert_eq!(model_hash, text("modelHash")?, "{}", text("case")?);
        assert_eq!(json!(objective_levels), capture["objectiveLevels"]);

        let document_bytes = serde_json::to_vec(&document)?;
        assert_eq!(document_bytes, text("documentJson")?.as_bytes());
        let score = score_workforce_solution(&document, &normalized_solution, None, None)?;
        let score_bytes = serde_json::to_vec(&score)?;
        assert_eq!(
            score_bytes,
            text("scoreJson")?.as_bytes(),
            "{}",
            text("case")?
        );
        assert_eq!(blake3_hex(&score_bytes), text("scoreChecksum")?);

        let required_scope = workforce_verification_scope(&document, 1, None)?;
        let verification_context = VerificationContextV1::new(
            document.scenario_id,
            1,
            blake3_hex(&document_bytes),
            model_hash,
            normalized_solution.canonical_hash()?,
            required_scope.checksum,
        )?;
        let verification = verify_workforce_solution(
            &document,
            &normalized_solution,
            None,
            &verification_context,
            &score,
            None,
        )?;
        assert_eq!(verification.checksum, text("requiredVerificationChecksum")?);
        assert_eq!(
            serde_json::to_vec(&verification)?,
            text("requiredVerificationReportJson")?.as_bytes(),
            "{}",
            text("case")?
        );
    }
    Ok(())
}

fn capture_test_original(
    document: &mut ScenarioDocument,
    objective: &mut RunObjectiveContext,
) -> Result {
    let binding = objective.baseline.as_mut().ok_or("baseline")?;
    let mut raw = document
        .domain
        .entities
        .remove(&id(20).parse()?)
        .ok_or("original")?;
    let entity_id =
        eutheto_types::EntityId::from_uuid(binding.original.accepted_result.solution_id.as_uuid());
    raw["id"] = json!(entity_id);
    binding.original.entity = workforce_baseline_reference(entity_id, false)?;
    binding.captured_original = Some(std::sync::Arc::new(raw));
    objective.schema_version = 2;
    Ok(())
}

#[test]
fn captured_original_pair_costs_compile_score_and_verify_without_mutating_source() -> Result {
    let mut value = base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-01T10:00:00Z",
        "2026-11-01T12:00:00Z",
    );
    for (baseline, selected, expected) in [
        (vec![], vec![], 0),
        (vec![], vec![assignment(1, 18, true)?], 1),
        (vec![pair(1, 8)?], vec![assignment(1, 8, true)?], 0),
        (vec![pair(1, 8)?], vec![], 1),
        (vec![pair(1, 8)?], vec![assignment(1, 18, true)?], 2),
        // Both historical endpoints were deleted; neither may be filtered out of the census.
        (
            vec![pair(999, 998)?, pair(999, 999)?],
            vec![assignment(1, 18, true)?],
            3,
        ),
    ] {
        let (mut document, binding) = document_with_original_baseline(value.clone(), &[])?;
        let mut objective = objective_context(
            &document,
            "official.workforce.objective.minimal-changes",
            RunObjectiveOperation::Repair,
            Some(binding),
        )?;
        capture_test_original(&mut document, &mut objective)?;
        let binding = objective.baseline.as_mut().ok_or("baseline")?;
        std::sync::Arc::make_mut(binding.captured_original.as_mut().ok_or("capture")?)["assignments"] =
            json!(baseline);
        binding.original.pair_content_hash = workforce_pair_content_hash(&baseline, None)?;
        binding.selected_pair_content_hash = binding.original.pair_content_hash.clone();
        let before = serde_json::to_vec(&document)?;
        let normalized = solution(&document, selected)?;
        let score = named_profile_score_matches_model(&document, &objective, &normalized)?;
        assert_eq!(score.levels[0].value, expected);
        let mut compile_context = context();
        compile_context.objective_context = Some(objective.clone());
        let problem = compile_workforce(&document, &compile_context)?.problem;
        let change = &problem.objectives.levels[0];
        assert_eq!(
            change.id.as_str(),
            "official.workforce.objective.pair_change"
        );
        assert_eq!(
            change
                .terms
                .iter()
                .map(|term| term.expression.constant)
                .sum::<i64>(),
            i64::try_from(baseline.len())?
        );
        assert_eq!(serde_json::to_vec(&document)?, before);
    }
    Ok(())
}

#[test]
fn captured_original_version_matrix_is_enforced_by_compiler_and_scorer() -> Result {
    let (mut document, binding) = document_with_original_baseline(base()?, &[pair(1, 8)?])?;
    let mut original = objective_context(
        &document,
        "official.workforce.objective.minimal-changes",
        RunObjectiveOperation::Repair,
        Some(binding),
    )?;
    let historical = original.clone();
    let historical_document = document.clone();
    capture_test_original(&mut document, &mut original)?;
    let empty = solution(&document, vec![])?;
    assert_eq!(
        score_workforce_solution(&historical_document, &empty, Some(&historical), None)?,
        score_workforce_solution(&document, &empty, Some(&original), None)?,
    );
    for (schema_version, captured) in [(1, true), (2, false), (3, true)] {
        let mut bad = original.clone();
        bad.schema_version = schema_version;
        if !captured {
            bad.baseline.as_mut().ok_or("baseline")?.captured_original = None;
        }
        let mut compile_context = context();
        compile_context.objective_context = Some(bad.clone());
        assert!(compile_workforce(&document, &compile_context).is_err());
        assert!(score_workforce_solution(&document, &empty, Some(&bad), None).is_err());
    }
    Ok(())
}

#[test]
fn captured_original_assignment_and_nonassignment_evidence_use_recorded_census() -> Result {
    let (mut document, mut objective) = profile_evidence_fixture()?;
    let expected = compiled_profile_contributions(&document, &objective)?;
    capture_test_original(&mut document, &mut objective)?;
    let before = serde_json::to_vec(&document)?;
    let request = assignment_evidence_request(profile_assignment_evidence(Vec::new())?)?;
    let control = OperationControl::Cancellation(CancellationToken::new());
    let rendered = render_workforce_evidence(&document, &request, Some(&objective), &control)?;
    assert_rendered_profile_contributions(
        &rendered.messages,
        &expected,
        &assignment(1, 18, true)?.id,
    )?;
    let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Infeasibility {
            infeasibility: eutheto_domain_ir::InfeasibilityEvidenceV1::Unavailable {
                reason: eutheto_domain_ir::ConflictUnavailableReason::BudgetExpired,
            },
            condition: None,
        },
    )?)?;
    render_workforce_evidence(&document, &request, Some(&objective), &control)?;
    assert_eq!(serde_json::to_vec(&document)?, before);
    Ok(())
}

fn alternative_context(
    value: Value,
    pairs: &[AssignmentPair],
) -> Result<(ScenarioDocument, RunObjectiveContext)> {
    let (document, baseline) = document_with_original_baseline(value, pairs)?;
    let objective = objective_context(
        &document,
        "official.workforce.objective.balanced",
        RunObjectiveOperation::Optimize,
        Some(baseline),
    )?;
    Ok((document, objective))
}

fn verified_alternative(
    document: &ScenarioDocument,
    objective: &RunObjectiveContext,
    pairs: &[(u32, u32)],
) -> Result<eutheto_domain_ir::AcceptedResult> {
    let selected = pairs
        .iter()
        .map(|(person, shift)| assignment(*person, *shift, true))
        .collect::<Result<Vec<_>>>()?;
    let normalized = solution(document, selected)?;
    let mut compile_context = context();
    compile_context.objective_context = Some(objective.clone());
    let problem = compile_workforce(document, &compile_context)?.problem;
    let score = score_workforce_solution(document, &normalized, Some(objective), None)?;
    let verification_scope = workforce_verification_scope(document, 1, None)?;
    let verification_context = VerificationContextV1::new(
        document.scenario_id,
        1,
        blake3_hex(&serde_json::to_vec(document)?),
        canonical_ir_hash(&problem, PlanningIrLimitsV1::DEFAULT)?,
        normalized.canonical_hash()?,
        verification_scope.checksum,
    )?;
    let report = verify_workforce_solution(
        document,
        &normalized,
        Some(objective),
        &verification_context,
        &score,
        None,
    )?;
    assert!(report.accepted);
    Ok(eutheto_domain_ir::AcceptedResult::new(normalized, report)?)
}

fn outcome_peer(value: &mut Value, index: u32) {
    let mut person = value["domain"]["entities"][id(1)].clone();
    person["id"] = json!(id(index));
    person["externalId"] = json!(format!("staff-{index}"));
    value["domain"]["entities"][id(index)] = person;
}

fn outcome_equivalent(
    document: &ScenarioDocument,
    objective: &RunObjectiveContext,
    left: &[(u32, u32)],
    right: &[(u32, u32)],
) -> Result<bool> {
    let left = verified_alternative(document, objective, left)?;
    let right = verified_alternative(document, objective, right)?;
    let mut budget = eutheto_workforce::assignment_rules::WorkforceAdmissionBudget::new(None);
    let left = budget.alternative_outcomes(document, &left, objective)?;
    let right = budget.alternative_outcomes(document, &right, objective)?;
    Ok(budget.same_alternative_outcomes(&left, &right)?)
}

#[test]
fn alternative_direct_sequence_and_target_families_preserve_personal_tradeoffs() -> Result {
    // All direct families, authored stability, targets, sequence and workload use one source-owned
    // per-person census rather than their equal scalar total. Capture-baseline additions are equal.
    let definitions = [
        json!({"kind":"time","direction":"avoid","timeWindow":{"kind":"instant","startsAt":"2026-11-01T00:00:00Z","endsAt":"2026-11-03T00:00:00Z"}}),
        json!({"kind":"assignmentType","direction":"avoid","assignmentTypeIds":[id(4)]}),
        json!({"kind":"location","direction":"avoid","locationIds":[id(5)]}),
        json!({"kind":"requestedTimeOff","availabilityIds":[id(45),id(46)]}),
        json!({"kind":"assignmentTarget","calendarId":id(2),"targetCount":1}),
        json!({"kind":"weekendTarget","targetCount":1}),
        json!({"kind":"consecutiveWork","mode":{"kind":"reportingDays"},"assignmentTypeIds":[id(4)],"threshold":0}),
        json!({"kind":"adjacency","direction":"avoid","baseScheduleId":id(20),"maximumGapMinutes":1000,"mode":"either"}),
        json!({"kind":"baseStability","baseScheduleId":id(20)}),
        json!({"kind":"workloadBalance","workloadPolicyId":id(10)}),
    ];
    for mut definition in definitions {
        let mut value = base()?;
        value["settings"]["horizon"]["end"] = json!("2026-11-03T00:00:00Z");
        outcome_peer(&mut value, 25);
        add_shift(
            &mut value,
            18,
            "2026-11-02T08:00:00Z",
            "2026-11-02T10:00:00Z",
        );
        availability(
            &mut value,
            45,
            1,
            "requestedTimeOff",
            "2026-11-01T00:00:00Z",
            "2026-11-02T00:00:00Z",
        );
        availability(
            &mut value,
            46,
            25,
            "requestedTimeOff",
            "2026-11-01T00:00:00Z",
            "2026-11-02T00:00:00Z",
        );
        let kind = definition["kind"]
            .as_str()
            .ok_or("preference kind")?
            .to_owned();
        definition["id"] = json!(id(40));
        definition["active"] = json!(true);
        definition["scope"] = json!({"people":{"kind":"all"},"weekdays":["sunday"]});
        definition["priority"] = json!("normal");
        definition["weight"] = json!(1);
        value["domain"]["preferences"][id(40)] = definition;
        // Authored stability and adjacency have actual opportunities against the common base.
        let base_pairs = match kind.as_str() {
            "baseStability" => vec![pair(1, 8)?, pair(25, 8)?],
            "adjacency" => {
                add_shift(
                    &mut value,
                    30,
                    "2026-11-01T06:00:00Z",
                    "2026-11-01T08:00:00Z",
                );
                vec![pair(1, 30)?, pair(25, 30)?]
            }
            _ => vec![],
        };
        let (document, objective) = alternative_context(value, &base_pairs)?;
        let left = verified_alternative(&document, &objective, &[(1, 8), (25, 18)])?;
        let right = verified_alternative(&document, &objective, &[(1, 18), (25, 8)])?;
        assert_eq!(
            left.verification.score.levels[1].value, right.verification.score.levels[1].value,
            "{kind}"
        );
        let mut budget = eutheto_workforce::assignment_rules::WorkforceAdmissionBudget::new(None);
        let left = budget.alternative_outcomes(&document, &left, &objective)?;
        let right = budget.alternative_outcomes(&document, &right, &objective)?;
        assert!(!budget.same_alternative_outcomes(&left, &right)?, "{kind}");
    }
    Ok(())
}

#[test]
fn alternative_placement_and_profile_rank_do_not_define_equality() -> Result {
    let mut value = base()?;
    add_shift(
        &mut value,
        18,
        "2026-11-01T12:00:00Z",
        "2026-11-01T14:00:00Z",
    );
    location_preference(&mut value, 40, "prefer", 1);
    workload_preference(&mut value, 41, 1, 10);
    value["domain"]["rules"][id(50)] = json!({"kind":"maximumHours","id":id(50),"active":true,"strength":"required","scope":{"people":{"kind":"all"}},"bucketId":id(3),"window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},"maximumMinutes":1000});
    value["domain"]["rules"][id(51)] = json!({"kind":"maximumAssignmentCount","id":id(51),"active":true,"strength":"required","scope":{"people":{"kind":"all"}},"calendarId":id(2),"maximum":10});
    let (document, objective) = alternative_context(value, &[])?;
    assert!(outcome_equivalent(
        &document,
        &objective,
        &[(1, 8)],
        &[(1, 18)]
    )?);
    let left = verified_alternative(&document, &objective, &[(1, 8)])?;
    let mut other = objective.clone();
    other.profile_id = ObjectiveProfileId::new("official.workforce.objective.fairness-first")?;
    let right = verified_alternative(&document, &other, &[(1, 18)])?;
    assert_ne!(
        left.verification.score.levels.last(),
        right.verification.score.levels.last()
    );
    let mut budget = eutheto_workforce::assignment_rules::WorkforceAdmissionBudget::new(None);
    let left = budget.alternative_outcomes(&document, &left, &objective)?;
    let right = budget.alternative_outcomes(&document, &right, &other)?;
    assert!(budget.same_alternative_outcomes(&left, &right)?);
    Ok(())
}

#[test]
fn alternative_pairing_coverage_and_soft_outcomes_are_identity_specific() -> Result {
    let mut value = base()?;
    outcome_peer(&mut value, 25);
    outcome_peer(&mut value, 26);
    add_shift(
        &mut value,
        18,
        "2026-11-01T12:00:00Z",
        "2026-11-01T14:00:00Z",
    );
    value["domain"]["preferences"][id(40)] = json!({"kind":"togetherSeparate","id":id(40),"active":true,"scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,"direction":"together","personIds":[id(1),id(25),id(26)],"mode":"sameShift"});
    let (document, objective) = alternative_context(value.clone(), &[])?;
    assert!(!outcome_equivalent(
        &document,
        &objective,
        &[(1, 8), (25, 8), (26, 18)],
        &[(1, 8), (25, 18), (26, 8)]
    )?);
    value["domain"]["preferences"] = json!({});
    for preferred in [0, 2] {
        value["domain"]["entities"][id(45)] = json!({"kind":"coverageRequirement","id":id(45),"active":true,"scope":{"kind":"all"},"coverage":{"kind":"atLeast","minimum":0,"preferredCount":preferred,"maximumCount":3,"qualificationMinimums":[]}});
        value["domain"]["preferences"][id(40)] = json!({"kind":"preferredCoverage","id":id(40),"active":true,"scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,"coverageRequirementIds":[id(45)]});
        let (document, objective) = alternative_context(value.clone(), &[])?;
        assert!(
            !outcome_equivalent(&document, &objective, &[(1, 8)], &[(1, 18)])?,
            "preferred={preferred}"
        );
    }
    value["domain"]["preferences"] = json!({});
    for (lock, shift) in [(40, 8), (41, 18)] {
        value["domain"]["lockedAssignments"][id(lock)] = json!({"id":id(lock),"personId":id(1),"shiftId":id(shift),"state":{"kind":"soft","stabilityWeight":3}});
    }
    let (document, objective) = alternative_context(value, &[])?;
    assert!(!outcome_equivalent(
        &document,
        &objective,
        &[(1, 8)],
        &[(1, 18)]
    )?);
    Ok(())
}

#[test]
fn alternative_all_configured_workload_families_and_zero_are_complete() -> Result {
    let definitions = [
        json!({"kind":"maximumHours","bucketId":id(3),"window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},"maximumMinutes":1000}),
        json!({"kind":"maximumHours","bucketId":id(3),"window":{"kind":"rolling","durationMinutes":1440,"membership":"startInstant"},"maximumMinutes":1000}),
        json!({"kind":"maximumAssignmentCount","calendarId":id(2),"maximum":10}),
        json!({"kind":"maximumConsecutive","mode":{"kind":"workedDays"},"maximum":10}),
        json!({"kind":"maximumConsecutive","mode":{"kind":"assignments","breakMinutes":1000},"maximum":10}),
        json!({"kind":"maximumWorkedWeekends","maximum":10}),
        json!({"kind":"workloadQuota","calendarId":id(2),"membership":"reportingDate","minimum":0,"maximum":1000,"measure":{"kind":"assignmentCount"}}),
        json!({"kind":"workloadQuota","calendarId":id(2),"membership":"intersection","minimum":0,"maximum":1000,"measure":{"kind":"bucketMinutes","bucketId":id(3)}}),
    ];
    for mut rule in definitions {
        let mut value = base()?;
        outcome_peer(&mut value, 25);
        add_shift(
            &mut value,
            18,
            "2026-11-01T12:00:00Z",
            "2026-11-01T13:00:00Z",
        );
        rule["id"] = json!(id(40));
        rule["active"] = json!(true);
        rule["strength"] = json!("required");
        rule["scope"] = json!({"people":{"kind":"all"}});
        value["domain"]["rules"][id(40)] = rule.clone();
        let (document, objective) = alternative_context(value, &[])?;
        assert!(
            !outcome_equivalent(
                &document,
                &objective,
                &[(1, 8), (1, 18)],
                &[(25, 8), (25, 18)]
            )?,
            "{rule}"
        );
        assert!(outcome_equivalent(&document, &objective, &[], &[])?);
        if rule["kind"] == "maximumHours" {
            // Same number of pairs added per person, unequal hours: baseline counts cannot mask it.
            assert!(!outcome_equivalent(
                &document,
                &objective,
                &[(1, 8), (25, 18)],
                &[(1, 18), (25, 8)]
            )?);
        }
    }
    Ok(())
}

#[test]
fn alternative_symmetric_target_sides_do_not_collapse() -> Result {
    let mut value = base()?;
    outcome_peer(&mut value, 25);
    add_shift(
        &mut value,
        18,
        "2026-11-01T12:00:00Z",
        "2026-11-01T14:00:00Z",
    );
    value["domain"]["preferences"][id(40)] = json!({"kind":"assignmentTarget","id":id(40),"active":true,"scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,"calendarId":id(2),"targetCount":1});
    let (document, objective) = alternative_context(value, &[])?;
    let left = verified_alternative(&document, &objective, &[(1, 8), (1, 18)])?;
    let right = verified_alternative(&document, &objective, &[(25, 8), (25, 18)])?;
    assert_eq!(
        left.verification.score.levels[1].value,
        right.verification.score.levels[1].value
    );
    assert!(!outcome_equivalent(
        &document,
        &objective,
        &[(1, 8), (1, 18)],
        &[(25, 8), (25, 18)]
    )?);
    Ok(())
}

#[test]
fn alternative_fractional_and_signed_dst_workloads_remain_exact() -> Result {
    let mut value = base()?;
    value["domain"]["entities"][id(8)]["endsAt"]["instant"] =
        json!("2026-11-01T10:00:00.000000001Z");
    value["domain"]["entities"][id(8)]["endsAt"]["local"] = json!("2026-11-01T10:00:00.000000001");
    add_shift(
        &mut value,
        18,
        "2026-11-01T12:00:00Z",
        "2026-11-01T14:00:00Z",
    );
    value["domain"]["rules"][id(40)] = json!({"kind":"maximumHours","id":id(40),"active":true,"strength":"required","scope":{"people":{"kind":"all"}},"bucketId":id(3),"window":{"kind":"calendar","calendarId":id(2),"membership":"intersection"},"maximumMinutes":1000});
    let (document, objective) = alternative_context(value, &[])?;
    assert!(!outcome_equivalent(
        &document,
        &objective,
        &[(1, 8)],
        &[(1, 18)]
    )?);
    let mut value = base()?;
    value["settings"]["timeZone"] = json!("America/New_York");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T04:00:00Z","end":"2026-11-02T05:00:00Z"});
    value["domain"]["entities"][id(3)]["measurement"] = json!("scheduledMinutes");
    value["domain"]["entities"][id(3)]["overlappingContribution"] = json!("sum");
    add_shift_with_offsets(
        &mut value,
        8,
        ("2026-11-01T05:45:00Z", "2026-11-01T01:45:00", -14400),
        ("2026-11-01T06:15:00Z", "2026-11-01T01:15:00", -18000),
    );
    add_shift_with_offsets(
        &mut value,
        18,
        ("2026-11-01T05:40:00Z", "2026-11-01T01:40:00", -14400),
        ("2026-11-01T06:25:00Z", "2026-11-01T01:25:00", -18000),
    );
    value["domain"]["rules"][id(40)] = json!({"kind":"maximumHours","id":id(40),"active":true,"strength":"required","scope":{"people":{"kind":"all"}},"bucketId":id(3),"window":{"kind":"rolling","durationMinutes":120,"membership":"startInstant"},"maximumMinutes":1000});
    let (document, objective) = alternative_context(value, &[])?;
    assert!(!outcome_equivalent(
        &document,
        &objective,
        &[(1, 8)],
        &[(1, 18)]
    )?);
    Ok(())
}

#[test]
fn alternative_census_and_comparison_never_refill_or_swallow_interruption() -> Result {
    use eutheto_workforce::assignment_rules::{MAX_WORK_STEPS, WorkforceAdmissionBudget};
    let (document, objective) = alternative_context(base()?, &[])?;
    let accepted = verified_alternative(&document, &objective, &[(1, 8)])?;
    let token = CancellationToken::new();
    let control = OperationControl::Cancellation(token.clone());
    let mut budget = WorkforceAdmissionBudget::new(Some(&control));
    let left = budget.alternative_outcomes(&document, &accepted, &objective)?;
    let right = budget.alternative_outcomes(&document, &accepted, &objective)?;
    token.cancel();
    assert!(matches!(
        budget.same_alternative_outcomes(&left, &right),
        Err(DomainPackError::Cancelled)
    ));
    assert!(matches!(
        budget.alternative_outcomes(&document, &accepted, &objective),
        Err(DomainPackError::Cancelled)
    ));
    let mut exhausted = WorkforceAdmissionBudget::new(None);
    exhausted.steps(MAX_WORK_STEPS)?;
    assert!(
        exhausted
            .alternative_outcomes(&document, &accepted, &objective)
            .is_err()
    );
    let mut exhausted = WorkforceAdmissionBudget::new(None);
    exhausted.steps(MAX_WORK_STEPS)?;
    assert!(exhausted.same_alternative_outcomes(&left, &right).is_err());
    let mut retention = WorkforceAdmissionBudget::new(None);
    retention.reserve(100_000, 0, 0)?;
    assert!(
        retention
            .alternative_outcomes(&document, &accepted, &objective)
            .is_err()
    );
    let mut wrong = accepted.clone();
    wrong
        .verification
        .score
        .levels
        .last_mut()
        .ok_or("rank")?
        .value += 1;
    let mut budget = WorkforceAdmissionBudget::new(None);
    assert!(
        budget
            .alternative_outcomes(&document, &wrong, &objective)
            .is_err()
    );
    Ok(())
}

#[test]
fn alternative_baseline_changes_keep_added_and_removed_personal_counts() -> Result {
    let mut value = base()?;
    outcome_peer(&mut value, 25);
    add_shift(
        &mut value,
        18,
        "2026-11-01T12:00:00Z",
        "2026-11-01T14:00:00Z",
    );
    let (document, objective) = alternative_context(value, &[pair(1, 8)?, pair(25, 18)?])?;
    assert!(!outcome_equivalent(
        &document,
        &objective,
        &[(1, 8), (25, 8)],
        &[(1, 18), (25, 18)]
    )?);
    Ok(())
}
