use super::*;
use crate::{
    assignment_rules::{
        score_workforce_solution, verify_workforce_solution, workforce_baseline_reference,
        workforce_manual_selection_checksum, workforce_pair_content_hash,
        workforce_verification_scope,
    },
    model::{ManualSelection, WorkforceEntity},
    test_support::{fixture, id},
};
use eutheto_domain_ir::{
    AcceptedResult, AcceptedResultRefV1, AssignmentProtectionStateV1, AssignmentValue,
    DomainAssignment, NormalizedSolution, ObjectiveProfileId, PortableAcceptedResult,
    RunBaselineBinding, RunBaselineOriginal, RunBaselineSelection, RunInput, RunManifest,
    RunObjectiveOperation, RunPhaseTimingsV1, RunTerminalOutcomeV1, ScoreComparisonV2,
    VerificationContextV1, blake3_hex,
};
use eutheto_types::{
    BackendId, BackendSelection, DurationMillis, ExplanationMode, PreservationPolicy,
    ReproducibilityMode, ResourceLimits, Revision, SolveMode, SolveOptions, SolveRunId,
    SolveStatus, WorkerThreadPolicy,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn source() -> Result<ScenarioDocument> {
    let mut document = fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    document.domain.locked_assignments.clear();
    Ok(document)
}

fn pair(person: u32, shift: u32) -> Result<AssignmentPair> {
    Ok(AssignmentPair {
        person_id: id(person).parse()?,
        shift_id: id(shift).parse()?,
    })
}

fn result_context(document: &ScenarioDocument, profile: &str) -> Result<RunObjectiveContext> {
    Ok(RunObjectiveContext {
        schema_version: 1,
        pack_id: document.domain_pack.id.clone(),
        profile_id: ObjectiveProfileId::new(profile)?,
        semantic_version: 1,
        operation: RunObjectiveOperation::Optimize,
        baseline: None,
        external_baseline: None,
    })
}

fn solution_assignment(pair: AssignmentPair, selected: bool) -> Result<DomainAssignment> {
    let pair_id = format!("{}.{}", pair.person_id, pair.shift_id);
    Ok(DomainAssignment {
        id: DomainAssignmentId::new(format!("{ASSIGNMENT_KIND}.{pair_id}"))?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new(ASSIGNMENT_KIND)?,
            id: DomainEntityId::new(pair_id)?,
        },
        value: AssignmentValue::Boolean(selected),
        evidence: Vec::new(),
    })
}

#[allow(clippy::too_many_lines)]
fn accepted(
    document: &ScenarioDocument,
    selected: &[AssignmentPair],
    revision: u64,
    objective_context: Option<RunObjectiveContext>,
    identity: u32,
    compiler_version: &str,
) -> Result<PortableAcceptedResult> {
    let mut budget = OperationBudget::evaluation(None);
    let input = AssignmentInput::new(document, &mut budget)?;
    let selected: BTreeSet<_> = selected.iter().copied().collect();
    let mut assignments = Vec::new();
    for person in &input.people {
        for shift in &input.shifts {
            let pair = AssignmentPair {
                person_id: *person,
                shift_id: shift.id,
            };
            assignments.push(solution_assignment(pair, selected.contains(&pair))?);
        }
    }
    assignments.sort_by(|left, right| left.id.cmp(&right.id));
    let solution = NormalizedSolution {
        schema_version: NORMALIZED_SOLUTION_SCHEMA_VERSION,
        pack_id: document.domain_pack.id.clone(),
        scenario_id: document.scenario_id,
        scenario_revision: revision,
        projection_version: WORKFORCE_PROJECTION_VERSION,
        solution_id: id(identity).parse()?,
        assignments,
    };
    let document_hash = blake3_hex(&serde_json::to_vec(document)?);
    let model_hash = blake3_hex(b"comparison fixture model");
    let verification_scope = workforce_verification_scope(document, revision, None)?;
    let verification_context = VerificationContextV1::new(
        document.scenario_id,
        revision,
        document_hash.clone(),
        model_hash.clone(),
        solution.canonical_hash()?,
        verification_scope.checksum,
    )?;
    let score = score_workforce_solution(document, &solution, objective_context.as_ref(), None)?;
    let verification = verify_workforce_solution(
        document,
        &solution,
        objective_context.as_ref(),
        &verification_context,
        &score,
        None,
    )?;
    let accepted = AcceptedResult::new(solution.clone(), verification.clone())?;
    let run_id: SolveRunId = id(identity + 1).parse()?;
    let run_input = RunInput::new(
        run_id,
        id(identity + 2).parse()?,
        document.scenario_id,
        revision,
        id(identity + 3).parse()?,
        document_hash,
        "2026-11-01T00:00:00Z".parse()?,
        document.domain_pack.id.clone(),
        document.domain_pack.schema_version,
        PLANNING_IR_SCHEMA_VERSION,
        compiler_version.to_owned(),
        "0.1.0".to_owned(),
        BackendId::new("ortools.cp-sat")?,
        "9.15.6755".to_owned(),
        "0.1.0".to_owned(),
        "0.1.0".to_owned(),
        "9.15.6755".to_owned(),
        1,
        0,
        model_hash,
        blake3_hex(b"comparison fixture objective policy"),
        solve_options()?,
        "America/New_York".parse()?,
        None,
        objective_context,
    )?;
    let manifest = RunManifest::new(
        run_id,
        run_input.checksum.clone(),
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: verification.checksum.clone(),
        },
        "2026-11-01T00:00:00Z".parse()?,
        "2026-11-01T00:00:01Z".parse()?,
        Some(DurationMillis::new(1_000)?),
        Some(DurationMillis::new(100)?),
        Some(DurationMillis::new(500)?),
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    Ok(PortableAcceptedResult::new(
        run_input,
        manifest,
        accepted,
        BTreeMap::new(),
    )?)
}

fn solve_options() -> Result<SolveOptions> {
    Ok(SolveOptions {
        backend: BackendSelection::Auto,
        mode: SolveMode::Balanced,
        time_limit_milliseconds: DurationMillis::new(30_000)?,
        memory_limit_bytes: Some(64 * 1024 * 1024),
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 7,
        solution_limit: Some(1),
        stop_after_first_feasible: true,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::Standard,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: 1_000,
            max_rules: 1_000,
            max_variables: 10_000,
            max_constraints: 10_000,
        },
    })
}

fn source_enrichment<'a>(
    base_document: &ScenarioDocument,
    base: &'a PortableAcceptedResult,
    candidate_document: &ScenarioDocument,
    candidate: &'a PortableAcceptedResult,
) -> Result<(PreparedComparisonV2<'a>, ComparisonEnrichmentV2<'a>)> {
    let prepared = eutheto_domain_ir::prepare_comparison_v2(base, candidate, None)?;
    let enrichment = workforce_comparison_enrichment(
        base_document,
        base,
        candidate_document,
        candidate,
        &prepared,
        None,
    )?;
    Ok((prepared, enrichment))
}

fn semantic_hash(
    document: &ScenarioDocument,
    context: Option<&RunObjectiveContext>,
) -> Result<String> {
    let mut budget = OperationBudget::evaluation(None);
    let input = AssignmentInput::new(document, &mut budget)?.with_objective_context(
        context,
        document.scenario_id,
        &mut budget,
    )?;
    Ok(scoring_semantic_hash(
        document,
        &input,
        context,
        &mut budget,
    )?)
}

fn add_person(document: &mut ScenarioDocument) -> Result<()> {
    let mut person = document
        .domain
        .entities
        .get(&id(1).parse()?)
        .cloned()
        .ok_or("fixture person")?;
    person["id"] = json!(id(20));
    person["externalId"] = json!("staff-20");
    person["name"] = json!("Sky");
    document.domain.entities.insert(id(20).parse()?, person);
    Ok(())
}

fn lock(
    document: &mut ScenarioDocument,
    lock_id: u32,
    person: u32,
    shift: u32,
    state: Value,
) -> Result<()> {
    let mut record = json!({"id":id(lock_id),"personId":id(person),"shiftId":id(shift)});
    record["state"] = state;
    document
        .domain
        .locked_assignments
        .insert(id(lock_id).parse()?, record);
    Ok(())
}

fn protection_for(
    protections: &[AssignmentProtectionComparisonV1],
    person: u32,
    shift: u32,
) -> Result<&AssignmentProtectionComparisonV1> {
    let id = assignment_id(pair(person, shift)?)?;
    protections
        .iter()
        .find(|record| record.assignment_id == id)
        .ok_or("missing protection".into())
}

#[test]
fn semantic_context_ignores_revision_metadata_and_selected_pair_changes() -> Result {
    let base_document = source()?;
    let mut candidate_document = base_document.clone();
    candidate_document.metadata.title = "Renamed scenario".to_owned();
    candidate_document.metadata.updated_at = "2026-10-01T00:00:00Z".parse()?;
    let context = result_context(
        &base_document,
        eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID,
    )?;
    let base = accepted(
        &base_document,
        &[pair(1, 8)?],
        1,
        Some(context.clone()),
        100,
        env!("CARGO_PKG_VERSION"),
    )?;
    let candidate = accepted(
        &candidate_document,
        &[pair(1, 7)?],
        2,
        Some(context),
        110,
        env!("CARGO_PKG_VERSION"),
    )?;
    let (prepared, enrichment) =
        source_enrichment(&base_document, &base, &candidate_document, &candidate)?;
    let (before, after) = enrichment
        .scoring_contexts
        .as_ref()
        .ok_or("current context")?;
    assert_eq!(before.semantic_hash, after.semantic_hash);
    let comparison = prepared.finish(enrichment)?;
    assert!(matches!(
        comparison.score,
        ScoreComparisonV2::Comparable { .. }
    ));
    Ok(())
}

#[test]
fn absent_context_and_unknown_compiler_keep_source_evidence_but_decline_score_projection() -> Result
{
    let document = source()?;
    let absent = accepted(
        &document,
        &[pair(1, 8)?],
        1,
        None,
        120,
        env!("CARGO_PKG_VERSION"),
    )?;
    let authored_context =
        result_context(&document, eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID)?;
    let authored = accepted(
        &document,
        &[pair(1, 7)?],
        2,
        Some(authored_context),
        130,
        env!("CARGO_PKG_VERSION"),
    )?;
    let (prepared, enrichment) = source_enrichment(&document, &absent, &document, &authored)?;
    assert!(enrichment.scoring_contexts.is_none());
    let comparison = prepared.finish(enrichment)?;
    assert!(matches!(
        comparison.score,
        ScoreComparisonV2::Unavailable {
            reason: eutheto_domain_ir::ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch
        }
    ));

    let context = result_context(&document, eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID)?;
    let old_base = accepted(
        &document,
        &[pair(1, 8)?],
        1,
        Some(context.clone()),
        140,
        "unknown",
    )?;
    let old_candidate = accepted(&document, &[pair(1, 7)?], 2, Some(context), 150, "unknown")?;
    let (prepared, enrichment) =
        source_enrichment(&document, &old_base, &document, &old_candidate)?;
    assert!(enrichment.scoring_contexts.is_none());
    let comparison = prepared.finish(enrichment)?;
    assert!(matches!(
        comparison.score,
        ScoreComparisonV2::Unavailable { .. }
    ));
    Ok(())
}

#[test]
fn preference_population_window_weight_and_scale_change_cost_fingerprint() -> Result {
    let context = result_context(&source()?, eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID)?;
    let mut base = source()?;
    base.domain.preferences.insert(
        id(30).parse()?,
        json!({
            "kind":"time", "id":id(30), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":1,
            "direction":"prefer",
            "timeWindow":{"kind":"weekly","windows":[{
                "weekdays":["sunday"],"startTime":"01:00:00","endTime":"02:00:00","endDayOffset":0
            }]}
        }),
    );
    let base_hash = semantic_hash(&base, Some(&context))?;
    let mut changed = base.clone();
    changed
        .domain
        .preferences
        .get_mut(&id(30).parse()?)
        .ok_or("preference")?["weight"] = json!(2);
    assert_ne!(base_hash, semantic_hash(&changed, Some(&context))?);
    let mut changed_window = base.clone();
    changed_window
        .domain
        .preferences
        .get_mut(&id(30).parse()?)
        .ok_or("preference")?["timeWindow"]["windows"][0]["weekdays"] = json!(["monday"]);
    assert_ne!(base_hash, semantic_hash(&changed_window, Some(&context))?);

    let mut changed_scale = base.clone();
    changed_scale
        .domain
        .entities
        .get_mut(&id(9).parse()?)
        .ok_or("score policy")?["priorityMapping"][1]["scale"] = json!(20);
    assert_ne!(base_hash, semantic_hash(&changed_scale, Some(&context))?);
    let mut changed_population = base.clone();
    add_person(&mut changed_population)?;
    assert_ne!(
        base_hash,
        semantic_hash(&changed_population, Some(&context))?
    );
    let mut changed_shape = base.clone();
    let policy = changed_shape
        .domain
        .entities
        .get_mut(&id(9).parse()?)
        .ok_or("score policy")?;
    policy["levels"][0]["levelKey"] = json!("renamed-preferences");
    for mapping in policy["priorityMapping"]
        .as_array_mut()
        .ok_or("priority mapping")?
    {
        mapping["levelKey"] = json!("renamed-preferences");
    }
    assert_ne!(base_hash, semantic_hash(&changed_shape, Some(&context))?);

    let mut workload = source()?;
    workload.domain.preferences.insert(
        id(31).parse()?,
        json!({
            "kind":"workloadBalance", "id":id(31), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":1,
            "workloadPolicyId":id(10)
        }),
    );
    let workload_context =
        result_context(&workload, eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID)?;
    let workload_hash = semantic_hash(&workload, Some(&workload_context))?;
    let mut scaled = workload.clone();
    scaled
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["workloadTarget"]["target"] = json!(240);
    assert_ne!(
        workload_hash,
        semantic_hash(&scaled, Some(&workload_context))?
    );
    Ok(())
}

fn baseline_binding(
    document: &mut ScenarioDocument,
    pairs: &[AssignmentPair],
    manual: bool,
) -> Result<RunBaselineBinding> {
    let content_hash = workforce_pair_content_hash(pairs, None)?;
    let base_id = id(40).parse()?;
    let source_solution_id = id(41).parse()?;
    document.domain.entities.insert(
        base_id,
        json!({
            "kind":"baseSchedule","id":id(40),"sourceSolutionId":id(41),"sourceRevision":0,
            "assignments":pairs
        }),
    );
    let original = RunBaselineOriginal {
        entity: workforce_baseline_reference(base_id, false)?,
        pair_content_hash: content_hash.clone(),
        accepted_result: AcceptedResultRefV1 {
            solution_id: source_solution_id,
            result_checksum: "a".repeat(64),
        },
        source_scenario_id: document.scenario_id,
        source_revision: 0,
        source_document_hash: "b".repeat(64),
        source_run_id: id(42).parse()?,
        source_run_input_checksum: "c".repeat(64),
        source_run_manifest_checksum: "d".repeat(64),
    };
    if !manual {
        return Ok(RunBaselineBinding {
            captured_original: None,
            original,
            selection: RunBaselineSelection::OriginalAccepted {},
            selected_pair_content_hash: content_hash,
        });
    }
    let mut record = ManualSelection {
        schema_version: 1,
        id: id(50).parse()?,
        original: original.clone(),
        admission_source_scenario_id: document.scenario_id,
        admission_source_revision: Revision::INITIAL,
        admission_source_document_hash: "e".repeat(64),
        admission_command_id: id(51).parse()?,
        assignments: pairs.to_vec(),
        checksum: String::new(),
    };
    record.checksum = workforce_manual_selection_checksum(&record, None)?;
    document.domain.entities.insert(
        record.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(record.clone()))?,
    );
    Ok(RunBaselineBinding {
        captured_original: None,
        original,
        selection: RunBaselineSelection::ManualSelection {
            entity: workforce_baseline_reference(record.id, true)?,
            record_checksum: record.checksum,
            admission_source_scenario_id: record.admission_source_scenario_id,
            admission_source_revision: record.admission_source_revision.value(),
            admission_source_document_hash: record.admission_source_document_hash,
        },
        selected_pair_content_hash: content_hash,
    })
}

fn repair_context(
    document: &ScenarioDocument,
    binding: RunBaselineBinding,
) -> Result<RunObjectiveContext> {
    Ok(RunObjectiveContext {
        schema_version: 1,
        pack_id: document.domain_pack.id.clone(),
        profile_id: ObjectiveProfileId::new("official.workforce.objective.minimal-changes")?,
        semantic_version: 1,
        operation: RunObjectiveOperation::Repair,
        baseline: Some(Box::new(binding)),
        external_baseline: None,
    })
}

#[test]
fn pair_change_fingerprint_uses_baseline_pairs_not_receipt_identity() -> Result {
    let reference_pair = pair(1, 8)?;
    let changed_pair = pair(1, 7)?;
    let mut original_doc = source()?;
    let original_binding = baseline_binding(&mut original_doc, &[reference_pair], false)?;
    let original_context = repair_context(&original_doc, original_binding)?;
    let mut manual_doc = source()?;
    let manual_binding = baseline_binding(&mut manual_doc, &[reference_pair], true)?;
    let manual_context = repair_context(&manual_doc, manual_binding)?;
    let original_hash = semantic_hash(&original_doc, Some(&original_context))?;
    assert_eq!(
        original_hash,
        semantic_hash(&manual_doc, Some(&manual_context))?
    );

    let mut changed_doc = source()?;
    let changed_binding = baseline_binding(&mut changed_doc, &[changed_pair], false)?;
    let changed_context = repair_context(&changed_doc, changed_binding)?;
    assert_ne!(
        original_hash,
        semantic_hash(&changed_doc, Some(&changed_context))?
    );
    let authored = result_context(
        &original_doc,
        eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID,
    )?;
    assert_ne!(
        original_hash,
        semantic_hash(&original_doc, Some(&authored))?
    );
    Ok(())
}

#[test]
fn pair_preference_fingerprint_is_independent_of_temporal_shift_order() -> Result {
    let mut document = source()?;
    document.domain.preferences.insert(
        id(30).parse()?,
        json!({
            "kind":"assignmentType", "id":id(30), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":1,
            "direction":"prefer", "assignmentTypeIds":[id(4)]
        }),
    );
    let context = result_context(&document, eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID)?;
    let expected = semantic_hash(&document, Some(&context))?;

    let mut reordered = document.clone();
    reordered
        .domain
        .entities
        .get_mut(&id(6).parse()?)
        .ok_or("shift template")?["timing"]["startTime"] = json!("03:30:00");
    assert_eq!(expected, semantic_hash(&reordered, Some(&context))?);
    Ok(())
}

#[test]
fn preferred_coverage_fingerprint_ignores_inactive_and_zero_count_references() -> Result {
    let mut document = source()?;
    document.domain.entities.insert(
        id(40).parse()?,
        json!({
            "kind":"coverageRequirement", "id":id(40), "active":true,
            "scope":{"kind":"all"},
            "coverage":{"kind":"atLeast","minimum":0,"preferredCount":1,
                "maximumCount":1,"qualificationMinimums":[]}
        }),
    );
    document.domain.preferences.insert(
        id(30).parse()?,
        json!({
            "kind":"preferredCoverage", "id":id(30), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":1,
            "coverageRequirementIds":[id(40)]
        }),
    );
    let context = result_context(&document, eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID)?;
    let expected = semantic_hash(&document, Some(&context))?;

    let mut inactive = document.clone();
    inactive.domain.entities.insert(
        id(41).parse()?,
        json!({
            "kind":"coverageRequirement", "id":id(41), "active":false,
            "scope":{"kind":"all"},
            "coverage":{"kind":"atLeast","minimum":0,"preferredCount":1,
                "maximumCount":1,"qualificationMinimums":[]}
        }),
    );
    inactive
        .domain
        .preferences
        .get_mut(&id(30).parse()?)
        .ok_or("preference")?["coverageRequirementIds"] = json!([id(40), id(41)]);
    assert_eq!(expected, semantic_hash(&inactive, Some(&context))?);

    let mut zero_count = document;
    zero_count.domain.entities.insert(
        id(42).parse()?,
        json!({
            "kind":"coverageRequirement", "id":id(42), "active":true,
            "scope":{"kind":"all"},
            "coverage":{"kind":"atLeast","minimum":0,"preferredCount":0,
                "maximumCount":1,"qualificationMinimums":[]}
        }),
    );
    zero_count
        .domain
        .preferences
        .get_mut(&id(30).parse()?)
        .ok_or("preference")?["coverageRequirementIds"] = json!([id(40), id(42)]);
    assert_eq!(expected, semantic_hash(&zero_count, Some(&context))?);
    Ok(())
}

#[test]
fn affected_team_labels_are_resolved_from_both_sources() -> Result {
    let mut base_document = source()?;
    base_document.domain.entities.insert(
        id(30).parse()?,
        json!({"kind":"team","id":id(30),"name":"Blue"}),
    );
    base_document.domain.entities.insert(
        id(31).parse()?,
        json!({"kind":"team","id":id(31),"name":"Gold"}),
    );
    base_document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["teamIds"] = json!([id(30)]);
    lock(&mut base_document, 60, 1, 7, json!({"kind":"unlocked"}))?;
    let mut candidate_document = base_document.clone();
    candidate_document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["teamIds"] = json!([id(31)]);
    candidate_document
        .domain
        .locked_assignments
        .remove(&id(60).parse()?);
    let context = result_context(&base_document, "official.workforce.objective.balanced")?;
    let base = accepted(
        &base_document,
        &[],
        1,
        Some(context.clone()),
        190,
        env!("CARGO_PKG_VERSION"),
    )?;
    let candidate = accepted(
        &candidate_document,
        &[],
        2,
        Some(context),
        200,
        env!("CARGO_PKG_VERSION"),
    )?;
    let (prepared, enrichment) =
        source_enrichment(&base_document, &base, &candidate_document, &candidate)?;
    let comparison = prepared.finish(enrichment)?;
    for (team_id, label) in [(30, "Blue"), (31, "Gold")] {
        let reference = entity_ref("team", id(team_id).parse()?)?;
        let observation = comparison
            .entity_observations
            .iter()
            .find(|record| record.entity == reference)
            .ok_or("team observation")?;
        assert_eq!(observation.before_label.as_deref(), Some(label));
        assert_eq!(observation.after_label.as_deref(), Some(label));
    }
    Ok(())
}

#[test]
fn portable_preflight_bounds_evidence_and_honors_cancellation() -> Result {
    let document = source()?;
    let context = result_context(&document, eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID)?;
    let mut oversized = accepted(
        &document,
        &[],
        1,
        Some(context.clone()),
        210,
        env!("CARGO_PKG_VERSION"),
    )?;
    for index in 0..4_000 {
        oversized.evidence.insert(
            DomainEvidenceId::new(format!("official.workforce.evidence-{index}"))?,
            eutheto_domain_ir::VerificationValue::Text("x".repeat(3_500)),
        );
    }
    let mut budget = OperationBudget::evaluation(None);
    assert!(matches!(
        validate_portable(&oversized, &mut budget),
        Err(DomainPackError::ResourceLimitExceeded)
    ));

    let valid = accepted(
        &document,
        &[],
        1,
        Some(context),
        220,
        env!("CARGO_PKG_VERSION"),
    )?;

    let mut unsupported = valid.clone();
    unsupported.schema_version = 4;
    let mut unsupported_budget = OperationBudget::evaluation(None);
    assert!(matches!(
        validate_portable(&unsupported, &mut unsupported_budget),
        Err(DomainPackError::UnsupportedVersion(4))
    ));

    let cancellation = eutheto_types::CancellationToken::new();
    let control = OperationControl::Cancellation(cancellation.clone());
    let mut cancellation_budget = OperationBudget::evaluation(Some(&control));
    cancellation_budget.cancel_after_steps(1)?;
    assert!(matches!(
        validate_portable(&valid, &mut cancellation_budget),
        Err(DomainPackError::Cancelled)
    ));
    assert!(cancellation.is_cancelled());
    Ok(())
}

#[test]
fn frozen_label_union_reserves_only_the_missing_source_label() -> Result {
    let document = source()?;
    let mut budget = OperationBudget::evaluation(None);
    let input = AssignmentInput::new(&document, &mut budget)?;
    let name = input
        .person(id(1).parse()?)
        .ok_or("fixture person")?
        .name
        .clone();
    let entity = entity_ref("person", id(1).parse()?)?;
    let mut observations = BTreeMap::from([(entity.clone(), [Some(name.clone()), None])]);
    let missing_label_bytes = count(name.len() + 8)?;
    budget.reserve(0, 0, budget.remaining_output().2 - missing_label_bytes)?;

    resolve_source_labels(&input, &input, &mut observations, &mut budget)?;
    let labels = observations.get(&entity).ok_or("lost affected person")?;
    assert_eq!(labels[0].as_deref(), Some(name.as_str()));
    assert_eq!(labels[1].as_deref(), Some(name.as_str()));
    assert_eq!(budget.remaining_output().2, 0);
    Ok(())
}

// One two-source fixture checks independent protection states and retained/missing native labels.
#[allow(clippy::too_many_lines)]
#[test]
fn protections_preserve_hard_soft_unlocked_absent_and_side_labels() -> Result {
    let mut base_document = source()?;
    add_person(&mut base_document)?;
    lock(&mut base_document, 60, 1, 7, json!({"kind":"hard"}))?;
    lock(
        &mut base_document,
        61,
        1,
        8,
        json!({"kind":"soft","stabilityWeight":2}),
    )?;
    lock(&mut base_document, 62, 20, 8, json!({"kind":"unlocked"}))?;
    let mut candidate_document = base_document.clone();
    candidate_document.domain.locked_assignments.clear();
    lock(
        &mut candidate_document,
        63,
        1,
        7,
        json!({"kind":"soft","stabilityWeight":4}),
    )?;
    lock(
        &mut candidate_document,
        64,
        1,
        8,
        json!({"kind":"soft","stabilityWeight":3}),
    )?;
    candidate_document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["name"] = json!("River, revised");
    let context = result_context(&base_document, "official.workforce.objective.balanced")?;
    let base = accepted(
        &base_document,
        &[pair(1, 7)?],
        1,
        Some(context.clone()),
        160,
        env!("CARGO_PKG_VERSION"),
    )?;
    let candidate = accepted(
        &candidate_document,
        &[pair(1, 7)?],
        2,
        Some(context.clone()),
        170,
        env!("CARGO_PKG_VERSION"),
    )?;
    let (prepared, enrichment) =
        source_enrichment(&base_document, &base, &candidate_document, &candidate)?;
    let hard = protection_for(&enrichment.protections, 1, 7)?;
    assert_eq!(hard.before, Some(AssignmentProtectionStateV1::Hard));
    assert_eq!(
        hard.after,
        Some(AssignmentProtectionStateV1::Soft { weight: 4 })
    );
    let soft = protection_for(&enrichment.protections, 1, 8)?;
    assert_eq!(
        soft.before,
        Some(AssignmentProtectionStateV1::Soft { weight: 2 })
    );
    assert_eq!(
        soft.after,
        Some(AssignmentProtectionStateV1::Soft { weight: 3 })
    );
    let unlocked = protection_for(&enrichment.protections, 20, 8)?;
    assert_eq!(unlocked.before, Some(AssignmentProtectionStateV1::Unlocked));
    assert_eq!(unlocked.after, None);
    assert!(!unlocked.before_selected && !unlocked.after_selected);
    let comparison = prepared.finish(enrichment)?;
    let person_ref = DomainEntityRef {
        kind: DomainEntityKindId::new("official.workforce.person")?,
        id: DomainEntityId::new(format!("official.workforce.{}", id(1)))?,
    };
    let observation = comparison
        .entity_observations
        .iter()
        .find(|record| record.entity == person_ref)
        .ok_or("person observation")?;
    assert_eq!(observation.before_label.as_deref(), Some("River"));
    assert_eq!(observation.after_label.as_deref(), Some("River, revised"));
    let mut removed_document = base_document.clone();
    removed_document.domain.entities.remove(&id(1).parse()?);
    removed_document.domain.locked_assignments.clear();
    let removed = accepted(
        &removed_document,
        &[],
        3,
        Some(context),
        180,
        env!("CARGO_PKG_VERSION"),
    )?;
    let (prepared, enrichment) =
        source_enrichment(&base_document, &base, &removed_document, &removed)?;
    let comparison = prepared.finish(enrichment)?;
    let removed_observation = comparison
        .entity_observations
        .iter()
        .find(|record| record.entity == person_ref)
        .ok_or("removed person observation")?;
    assert_eq!(removed_observation.before_label.as_deref(), Some("River"));
    assert_eq!(removed_observation.after_label, None);
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn repair_messages_bind_complete_sources_and_preserve_thresholds_and_recorded_frames() -> Result {
    use eutheto_domain_api::DomainPack;
    use eutheto_domain_ir::{
        EvidenceRenderRequestV2, ExplanationEvidencePayloadV2, ExplanationEvidenceV2,
        RepairCausalityV1, RepairEvidenceV2, VerificationFactId, VerificationValue,
    };
    let control = OperationControl::Cancellation(eutheto_types::CancellationToken::new());
    for manual in [false, true] {
        let base_document = source()?;
        let original_pair = pair(1, if manual { 7 } else { 8 })?;
        let base = accepted(
            &base_document,
            &[original_pair],
            1,
            Some(result_context(
                &base_document,
                eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID,
            )?),
            200,
            env!("CARGO_PKG_VERSION"),
        )?;
        let mut candidate_document = base_document.clone();
        let context = if manual {
            let binding = baseline_binding(&mut candidate_document, &[pair(1, 8)?], true)?;
            repair_context(&candidate_document, binding)?
        } else {
            result_context(
                &candidate_document,
                eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID,
            )?
        };
        candidate_document.domain.rules.insert(
            id(60).parse()?,
            json!({
                "id":id(60),"kind":"maximumHours","active":true,"strength":"required",
                "scope":{"people":{"kind":"all"}},"bucketId":id(3),
                "window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},
                "maximumMinutes":60
            }),
        );
        let candidate = accepted(
            &candidate_document,
            &[pair(1, 7)?],
            2,
            Some(context),
            210,
            env!("CARGO_PKG_VERSION"),
        )?;
        let (prepared, enrichment) =
            source_enrichment(&base_document, &base, &candidate_document, &candidate)?;
        let comparison = prepared.finish(enrichment)?;
        if manual {
            // A and C are identical selections. B-only changes must not vanish from facts.
            assert!(comparison.assignments.is_empty());
        }
        let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
            ExplanationEvidencePayloadV2::Repair {
                repair: Box::new(RepairEvidenceV2 {
                    comparison,
                    causality: RepairCausalityV1::NotEstablished,
                }),
            },
        )?)?;
        let rendered = crate::WorkforcePack.render_repair_evidence(
            &base_document,
            &base,
            &candidate_document,
            &candidate,
            &request,
            &control,
        )?;
        assert_eq!(
            rendered.messages[0]
                .parameters
                .get(&VerificationFactId::new(
                    "official.workforce.fact.repair_frame",
                )?),
            Some(&VerificationValue::Text(
                "acceptedBaseToCandidate".to_owned()
            ))
        );
        let hours_rule: eutheto_types::RuleId = id(60).parse()?;
        let message = rendered
            .messages
            .iter()
            .find(|message| message.rules.contains(&hours_rule))
            .ok_or("missing full-source hours observation")?;
        assert_eq!(
            message.parameters.get(&VerificationFactId::new(
                "expected.official.workforce.fact.maximum_hours_minutes",
            )?),
            Some(&VerificationValue::Integer(60))
        );
        assert_eq!(
            message.parameters.get(&VerificationFactId::new(
                "observed.official.workforce.fact.actual_hours_seconds",
            )?),
            Some(&VerificationValue::Integer(7200))
        );
        assert_eq!(
            message.parameters.get(&VerificationFactId::new(
                "official.workforce.fact.repair_frame",
            )?),
            Some(&VerificationValue::Text(
                if manual {
                    "recordedBaselineToCandidate"
                } else {
                    "acceptedBaseToCandidate"
                }
                .to_owned()
            ))
        );
        assert_eq!(
            message.parameters.get(&VerificationFactId::new(
                "official.workforce.fact.baseline_admission",
            )?),
            Some(&VerificationValue::Text("notEstablished".to_owned()))
        );
        assert!(message.assignments.contains(&assignment_id(pair(1, 8)?)?));
        rendered.validate()?;
        let mut wrong_document = candidate_document.clone();
        wrong_document.metadata.title.push_str(" wrong source");
        assert!(
            crate::WorkforcePack
                .render_repair_evidence(
                    &base_document,
                    &base,
                    &wrong_document,
                    &candidate,
                    &request,
                    &control,
                )
                .is_err()
        );
        assert!(matches!(
            crate::WorkforcePack.render_evidence(
                &candidate_document,
                &request,
                candidate.run_input.objective_context.as_ref(),
                &control,
            ),
            Err(DomainPackError::UnsupportedExplanationCapability(
                eutheto_domain_ir::ExplanationCapability::Repair,
            ))
        ));
    }
    Ok(())
}

#[test]
fn repair_missing_historical_endpoint_disables_complete_aggregate_scope() -> Result {
    use eutheto_domain_api::DomainPack;
    use eutheto_domain_ir::{
        EvidenceRenderRequestV2, ExplanationEvidencePayloadV2, ExplanationEvidenceV2,
        RepairCausalityV1, RepairEvidenceV2, VerificationFactId, VerificationValue,
    };
    let base_document = source()?;
    let context = result_context(
        &base_document,
        eutheto_domain_ir::AUTHORED_OBJECTIVE_PROFILE_ID,
    )?;
    let base = accepted(
        &base_document,
        &[pair(1, 8)?],
        1,
        Some(context.clone()),
        230,
        env!("CARGO_PKG_VERSION"),
    )?;
    let mut candidate_document = base_document.clone();
    candidate_document.domain.entities.remove(&id(1).parse()?);
    let candidate = accepted(
        &candidate_document,
        &[],
        2,
        Some(context),
        240,
        env!("CARGO_PKG_VERSION"),
    )?;
    let (prepared, enrichment) =
        source_enrichment(&base_document, &base, &candidate_document, &candidate)?;
    let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Repair {
            repair: Box::new(RepairEvidenceV2 {
                comparison: prepared.finish(enrichment)?,
                causality: RepairCausalityV1::NotEstablished,
            }),
        },
    )?)?;
    let control = OperationControl::Cancellation(eutheto_types::CancellationToken::new());
    let rendered = crate::WorkforcePack.render_repair_evidence(
        &base_document,
        &base,
        &candidate_document,
        &candidate,
        &request,
        &control,
    )?;
    assert_eq!(
        rendered.messages[0]
            .parameters
            .get(&VerificationFactId::new(
                "official.workforce.fact.repair_aggregate_scope",
            )?),
        Some(&VerificationValue::Text(
            "unavailableHistoricalEndpoints".to_owned()
        ))
    );
    let prior_assignment = assignment_id(pair(1, 8)?)?;
    let endpoint = rendered
        .messages
        .iter()
        .find(|message| message.assignments.contains(&prior_assignment))
        .ok_or("missing historical endpoint evidence")?;
    assert_eq!(
        endpoint.parameters.get(&VerificationFactId::new(
            "official.workforce.fact.person_resolvable",
        )?),
        Some(&VerificationValue::Boolean(false))
    );
    rendered.validate()?;
    Ok(())
}

// Model a public caller resealing a structurally valid but untrusted contract.
fn reseal_contract_json(value: &mut Value) -> Result {
    value
        .as_object_mut()
        .ok_or("contract object")?
        .remove("checksum")
        .ok_or("contract checksum")?;
    let checksum = blake3_hex(&serde_json::to_vec(value)?);
    value
        .as_object_mut()
        .ok_or("contract object")?
        .insert("checksum".to_owned(), json!(checksum));
    Ok(())
}

#[test]
fn repair_rejects_resealed_body_omissions_and_pack_schema_mismatches() -> Result {
    use eutheto_domain_api::DomainPack;
    use eutheto_domain_ir::{
        ComparisonEnrichmentV2, EvidenceRenderRequestV2, ExplanationEvidencePayloadV2,
        ExplanationEvidenceV2, RepairCausalityV1, RepairEvidenceV2, SolutionComparisonV2,
        prepare_comparison_v2,
    };
    let document = source()?;
    let base = accepted(
        &document,
        &[pair(1, 8)?],
        1,
        None,
        250,
        env!("CARGO_PKG_VERSION"),
    )?;
    let candidate = accepted(
        &document,
        &[pair(1, 7)?],
        2,
        None,
        260,
        env!("CARGO_PKG_VERSION"),
    )?;
    let (prepared, enrichment) = source_enrichment(&document, &base, &document, &candidate)?;
    let mut comparison = prepared.finish(enrichment)?;
    assert!(!comparison.assignments.is_empty());
    comparison.assignments.clear();
    let mut forged = serde_json::to_value(comparison)?;
    reseal_contract_json(&mut forged)?;
    let comparison: SolutionComparisonV2 = serde_json::from_value(forged)?;
    comparison.validate()?;
    let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Repair {
            repair: Box::new(RepairEvidenceV2 {
                comparison,
                causality: RepairCausalityV1::NotEstablished,
            }),
        },
    )?)?;
    let control = OperationControl::Cancellation(eutheto_types::CancellationToken::new());
    assert!(
        crate::WorkforcePack
            .render_repair_evidence(&document, &base, &document, &candidate, &request, &control,)
            .is_err()
    );

    let mut mismatched = serde_json::to_value(&candidate)?;
    let mut mismatched_input = candidate.run_input.clone();
    mismatched_input.pack_schema_version = document.domain_pack.schema_version + 1;
    mismatched_input.request_hash = mismatched_input.request_semantics().canonical_hash()?;
    mismatched["runInput"] = serde_json::to_value(mismatched_input)?;
    reseal_contract_json(&mut mismatched["runInput"])?;
    mismatched["runManifest"]["runInputChecksum"] = mismatched["runInput"]["checksum"].clone();
    reseal_contract_json(&mut mismatched["runManifest"])?;
    reseal_contract_json(&mut mismatched)?;
    let mismatched: PortableAcceptedResult = serde_json::from_value(mismatched)?;
    mismatched.validate()?;
    let prepared = prepare_comparison_v2(&base, &mismatched, Some(&control))?;
    assert!(
        workforce_comparison_enrichment(
            &document,
            &base,
            &document,
            &mismatched,
            &prepared,
            Some(&control),
        )
        .is_err()
    );
    let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Repair {
            repair: Box::new(RepairEvidenceV2 {
                comparison: prepared.finish(ComparisonEnrichmentV2::default())?,
                causality: RepairCausalityV1::NotEstablished,
            }),
        },
    )?)?;
    assert!(
        crate::WorkforcePack
            .render_repair_evidence(&document, &base, &document, &mismatched, &request, &control,)
            .is_err()
    );
    Ok(())
}

#[test]
fn repair_preserves_withheld_score_authority_and_complete_recorded_baseline_facts() -> Result {
    use eutheto_domain_api::DomainPack;
    use eutheto_domain_ir::{
        EvidenceRenderRequestV2, ExplanationEvidencePayloadV2, ExplanationEvidenceV2,
        RepairCausalityV1, RepairEvidenceV2, ScoreComparisonUnavailableReasonV1, ScoreComparisonV2,
        VerificationFactId, VerificationValue,
    };
    let mut document = source()?;
    let binding = baseline_binding(&mut document, &[pair(1, 8)?], true)?;
    let context = repair_context(&document, binding)?;
    document.domain.rules.insert(
        id(60).parse()?,
        json!({
            "id":id(60),"kind":"maximumHours","active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}},"bucketId":id(3),
            "window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},
            "maximumMinutes":60
        }),
    );
    let base = accepted(
        &document,
        &[pair(1, 7)?],
        1,
        Some(context.clone()),
        270,
        env!("CARGO_PKG_VERSION"),
    )?;
    let candidate = accepted(
        &document,
        &[pair(1, 7)?],
        2,
        Some(context),
        280,
        env!("CARGO_PKG_VERSION"),
    )?;
    let (prepared, enrichment) = source_enrichment(&document, &base, &document, &candidate)?;
    assert!(matches!(
        prepared.finish(enrichment)?.score,
        ScoreComparisonV2::Comparable { .. }
    ));
    let (prepared, mut enrichment) = source_enrichment(&document, &base, &document, &candidate)?;
    // Core can withhold comparability when the recorded policy cannot be freshly established.
    enrichment.scoring_contexts = None;
    let comparison = prepared.finish(enrichment)?;
    assert!(matches!(
        comparison.score,
        ScoreComparisonV2::Unavailable {
            reason: ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch,
        }
    ));
    let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Repair {
            repair: Box::new(RepairEvidenceV2 {
                comparison,
                causality: RepairCausalityV1::NotEstablished,
            }),
        },
    )?)?;
    let control = OperationControl::Cancellation(eutheto_types::CancellationToken::new());
    let rendered = crate::WorkforcePack
        .render_repair_evidence(&document, &base, &document, &candidate, &request, &control)?;
    let hours_rule: eutheto_types::RuleId = id(60).parse()?;
    let hours = rendered
        .messages
        .iter()
        .find(|message| message.rules.contains(&hours_rule))
        .ok_or("withheld score dropped recorded baseline facts")?;
    assert_eq!(
        hours.parameters.get(&VerificationFactId::new(
            "observed.official.workforce.fact.actual_hours_seconds",
        )?),
        Some(&VerificationValue::Integer(7200))
    );
    assert_eq!(
        hours.parameters.get(&VerificationFactId::new(
            "official.workforce.fact.repair_frame",
        )?),
        Some(&VerificationValue::Text(
            "recordedBaselineToCandidate".to_owned()
        ))
    );
    assert!(hours.assignments.contains(&assignment_id(pair(1, 8)?)?));
    rendered.validate()?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[test]
fn captured_and_snapshot_context_versions_compare_by_exact_pair_cost_not_wire_version() -> Result {
    use eutheto_domain_api::DomainPack;
    use eutheto_domain_ir::{
        EvidenceRenderRequestV2, ExplanationEvidencePayloadV2, ExplanationEvidenceV2,
        RepairCausalityV1, RepairEvidenceV2, VerificationFactId, VerificationValue,
    };
    let mut historical_document = source()?;
    let binding = baseline_binding(&mut historical_document, &[pair(1, 8)?], false)?;
    let historical = repair_context(&historical_document, binding)?;
    let mut document = historical_document.clone();
    let mut captured = historical.clone();
    let mut raw = document
        .domain
        .entities
        .remove(&id(40).parse()?)
        .ok_or("base")?;
    let binding = captured.baseline.as_mut().ok_or("baseline")?;
    let entity_id = EntityId::from_uuid(binding.original.accepted_result.solution_id.as_uuid());
    raw["id"] = json!(entity_id);
    binding.original.entity = workforce_baseline_reference(entity_id, false)?;
    binding.captured_original = Some(std::sync::Arc::new(raw));
    captured.schema_version = 2;
    let before = serde_json::to_vec(&document)?;
    assert_eq!(
        semantic_hash(&historical_document, Some(&historical))?,
        semantic_hash(&document, Some(&captured))?
    );
    let base = accepted(
        &historical_document,
        &[pair(1, 8)?],
        1,
        Some(historical),
        800,
        env!("CARGO_PKG_VERSION"),
    )?;
    let candidate = accepted(
        &document,
        &[pair(1, 7)?],
        2,
        Some(captured.clone()),
        810,
        env!("CARGO_PKG_VERSION"),
    )?;
    let (prepared, enrichment) =
        source_enrichment(&historical_document, &base, &document, &candidate)?;
    assert_eq!(
        enrichment
            .scoring_contexts
            .as_ref()
            .ok_or("scoring")?
            .0
            .semantic_hash,
        enrichment
            .scoring_contexts
            .as_ref()
            .ok_or("scoring")?
            .1
            .semantic_hash
    );
    assert!(matches!(
        prepared.finish(enrichment)?.score,
        ScoreComparisonV2::Comparable { .. }
    ));
    let binding = captured.baseline.as_mut().ok_or("baseline")?;
    std::sync::Arc::make_mut(binding.captured_original.as_mut().ok_or("capture")?)["assignments"] =
        json!([pair(999, 999)?]);
    binding.original.pair_content_hash = workforce_pair_content_hash(&[pair(999, 999)?], None)?;
    binding.selected_pair_content_hash = binding.original.pair_content_hash.clone();
    assert_ne!(
        semantic_hash(
            &historical_document,
            base.run_input.objective_context.as_ref()
        )?,
        semantic_hash(&document, Some(&captured))?
    );
    let candidate = accepted(
        &document,
        &[pair(1, 7)?],
        2,
        Some(captured),
        820,
        env!("CARGO_PKG_VERSION"),
    )?;
    let (prepared, enrichment) =
        source_enrichment(&historical_document, &base, &document, &candidate)?;
    let comparison = prepared.finish(enrichment)?;
    assert!(matches!(
        comparison.score,
        ScoreComparisonV2::Unavailable { .. }
    ));
    let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Repair {
            repair: Box::new(RepairEvidenceV2 {
                comparison,
                causality: RepairCausalityV1::NotEstablished,
            }),
        },
    )?)?;
    let rendered = crate::WorkforcePack.render_repair_evidence(
        &historical_document,
        &base,
        &document,
        &candidate,
        &request,
        &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
    )?;
    let historical_assignment = assignment_id(pair(999, 999)?)?;
    let endpoint = rendered
        .messages
        .iter()
        .find(|message| message.assignments.contains(&historical_assignment))
        .ok_or("captured historical endpoint evidence")?;
    assert_eq!(
        endpoint.parameters.get(&VerificationFactId::new(
            "official.workforce.fact.person_resolvable",
        )?),
        Some(&VerificationValue::Boolean(false))
    );
    assert_eq!(
        endpoint.parameters.get(&VerificationFactId::new(
            "official.workforce.fact.repair_frame",
        )?),
        Some(&VerificationValue::Text(
            "recordedBaselineToCandidate".to_owned()
        ))
    );
    assert_eq!(serde_json::to_vec(&document)?, before);
    Ok(())
}

#[test]
fn external_repair_observations_identify_unverified_pairs_without_accepted_ancestry() -> Result {
    use eutheto_domain_api::DomainPack;
    use eutheto_domain_ir::{
        EvidenceRenderRequestV2, ExplanationEvidencePayloadV2, ExplanationEvidenceV2,
        RepairCausalityV1, RepairEvidenceV2, RunExternalBaselineBinding, VerificationFactId,
        VerificationValue,
    };
    let mut document = source()?;
    document.domain.entities.insert(
        id(50).parse()?,
        json!({
            "kind":"externalStartingPlan","schemaVersion":1,"id":id(50),"name":"Unverified import",
            "assignments":[pair(1, 8)?],
            "originalImport":{"sourceBlake3":"a".repeat(64),"pairContentHash":"b".repeat(64)}
        }),
    );
    document.domain.rules.insert(
        id(60).parse()?,
        json!({
            "id":id(60),"kind":"maximumHours","active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}},"bucketId":id(3),
            "window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},
            "maximumMinutes":60
        }),
    );
    let binding = RunExternalBaselineBinding {
        entity: crate::assignment_rules::workforce_external_baseline_reference(id(50).parse()?)?,
        selected_pair_content_hash: workforce_pair_content_hash(&[pair(1, 8)?], None)?,
    };
    let context = RunObjectiveContext {
        schema_version: 3,
        pack_id: document.domain_pack.id.clone(),
        profile_id: ObjectiveProfileId::new("official.workforce.objective.minimal-changes")?,
        semantic_version: 1,
        operation: RunObjectiveOperation::Repair,
        baseline: None,
        external_baseline: Some(Box::new(binding.clone())),
    };
    let version = env!("CARGO_PKG_VERSION");
    let base = accepted(&document, &[pair(1, 7)?], 1, None, 830, version)?;
    let candidate = accepted(&document, &[pair(1, 7)?], 1, Some(context), 840, version)?;
    let (prepared, enrichment) = source_enrichment(&document, &base, &document, &candidate)?;
    let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
        ExplanationEvidencePayloadV2::Repair {
            repair: Box::new(RepairEvidenceV2 {
                comparison: prepared.finish(enrichment)?,
                causality: RepairCausalityV1::NotEstablished,
            }),
        },
    )?)?;
    let rendered = crate::WorkforcePack.render_repair_evidence(
        &document,
        &base,
        &document,
        &candidate,
        &request,
        &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
    )?;
    let hours_rule: eutheto_types::RuleId = id(60).parse()?;
    let observation = rendered
        .messages
        .iter()
        .find(|message| message.rules.contains(&hours_rule))
        .ok_or("external baseline hours observation")?;
    assert_eq!(
        observation.parameters.get(&VerificationFactId::new(
            "observed.official.workforce.fact.actual_hours_seconds"
        )?),
        Some(&VerificationValue::Integer(7200))
    );
    for (name, value) in [
        ("repair_frame", "recordedBaselineToCandidate"),
        ("baseline_admission", "notEstablished"),
        ("baseline_selection_kind", "externalStartingPlan"),
        ("baseline_selection_entity", binding.entity.id.as_str()),
        (
            "baseline_pair_content_hash",
            binding.selected_pair_content_hash.as_str(),
        ),
    ] {
        assert_eq!(
            observation.parameters.get(&VerificationFactId::new(format!(
                "official.workforce.fact.{name}"
            ))?),
            Some(&VerificationValue::Text(value.to_owned())),
            "{name}"
        );
    }
    assert!(!observation.parameters.keys().any(|key| {
        key.as_str()
            .starts_with("official.workforce.fact.baseline_original_")
            || key
                .as_str()
                .starts_with("official.workforce.fact.baseline_record")
    }));
    Ok(())
}
