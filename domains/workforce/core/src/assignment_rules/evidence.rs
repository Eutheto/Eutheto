use super::{
    analysis::{analyze_source_with_budget, candidate_shortage_source},
    authority::{
        CHECKED_METRIC, SELECTED_METRIC, VIOLATIONS_METRIC, check_objective_context,
        check_objective_support, contract, evaluation_counts, operation_error,
        original_obligations, original_obligations_with_policy, original_rank, validate_pair,
    },
    boundary::{DOMAIN_LIMITS, preflight},
    budget::OperationBudget,
    evaluation::{RepairObservation, evaluate_pair, evaluate_repair_selection},
    identity::{IdentityKind, PlanningIdentities, RANK_CATEGORY, RANK_LEVEL},
    input::AssignmentInput,
    preference,
    projection::decode_workforce_assignment,
    sharing::validate_source_provenance,
};
use crate::model::{AssignmentPair, PreferenceDirection, WorkforcePreference, WorkforceRule};
use eutheto_domain_api::{ContractJsonLimits, DomainPackError, DomainValidationReport};
use eutheto_domain_ir::{
    AssignmentEvidenceV1, AssignmentLockStateV1, AssignmentValue, ComparisonOrdering,
    ConflictMinimality, ConflictShrinkStopReason, ConflictUnavailableReason,
    CounterfactualConclusionV1, CounterfactualConditionPayloadV1, CounterfactualConditionV1,
    CounterfactualResult, DomainEvidenceId, EvidenceMessageV1, EvidenceRenderRequestV2,
    EvidenceRenderResultV1, ExplanationEvidencePayloadV2, ExplanationKind,
    ExplanationValidationSeverity, InfeasibilityEvidenceV1, MAX_DIAGNOSTIC_GROUPS,
    MAX_EVIDENCE_MESSAGE_REFERENCES, MAX_EVIDENCE_MESSAGES, MetricValue, OptimalityStatusEvidence,
    PortableAcceptedResult, RuleEvaluation, RunBaselineSelection, RunObjectiveContext,
    RunTerminalOutcomeV1, ScoreCategoryId, ScoreComparisonUnavailableReasonV1, ScoreComparisonV2,
    ScoreContributionV1, ScoreLevelId, SolutionComparisonV2, ValidationIssueEvidenceV1,
    VerificationFactId, VerificationValue, blake3_hex,
};
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{OperationControl, RuleId, ScenarioDocument, SolveStatus, ValidationSeverity};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SELECTED_PREFERENCE_FACTS: usize = 128;
const MAX_INFEASIBILITY_CAPTION_BYTES: usize = 512;

/// Renders current validation, rederived pair-local assignment facts, terminal run status, or
/// immutable mapped infeasibility evidence already supplied by the host.
///
/// Validation and assignment branches cannot establish historical execution, head revision,
/// full-schedule feasibility or optimality. Infeasibility rendering validates source ownership
/// against the supplied document but does not independently confer run, worker, or proof authority.
///
/// # Errors
/// Rejects unsupported kinds, stale/forged local facts, unsupported initial semantics and bounds.
pub fn render_workforce_evidence(
    document: &ScenarioDocument,
    request: &EvidenceRenderRequestV2,
    objective_context: Option<&RunObjectiveContext>,
    control: &OperationControl,
) -> Result<EvidenceRenderResultV1, DomainPackError> {
    let mut budget = OperationBudget::evaluation(Some(control));

    let request_size =
        preflight(request, &mut budget, DOMAIN_LIMITS).map_err(|error| operation_error(&error))?;
    let document_size = preflight(document, &mut budget, ContractJsonLimits::DEFAULT)
        .map_err(|error| operation_error(&error))?;
    // Request validation is repeated by the generic result constructor; both checksum passes count.
    request_size
        .reserve_json(&mut budget, 2)
        .map_err(|error| operation_error(&error))?;
    document_size
        .reserve_json(&mut budget, 1)
        .map_err(|error| operation_error(&error))?;
    request
        .validate()
        .map_err(|_| contract("evidence_request"))?;
    check_objective_context(objective_context, document.scenario_id)?;
    if request.kind == ExplanationKind::Validation && objective_context.is_some() {
        return Err(DomainPackError::InvalidPayload {
            path: "objectiveContext".to_owned(),
            message: "runless Validation cannot use a recorded run objective context".to_owned(),
        });
    }
    if request.kind != ExplanationKind::Assignment
        && objective_context.is_some_and(RunObjectiveContext::has_baseline)
        && !matches!(
            request.kind,
            ExplanationKind::SolutionDifference | ExplanationKind::Repair
        )
    {
        let input = AssignmentInput::new(document, &mut budget)
            .map_err(|error| operation_error(&error))?
            .with_objective_context(objective_context, document.scenario_id, &mut budget)?;
        super::profile::snapshot_pairs(
            &input,
            document.scenario_id,
            objective_context,
            &mut budget,
        )?;
    }
    let messages = match &request.evidence.evidence {
        ExplanationEvidencePayloadV2::Validation { issue } => {
            render_validation(document, issue, &mut budget)?
        }
        ExplanationEvidencePayloadV2::Assignment { assignment } => {
            render_assignment(document, assignment, objective_context, &mut budget)?
        }
        ExplanationEvidencePayloadV2::Infeasibility {
            infeasibility,
            condition,
        } => render_infeasibility(document, infeasibility, condition.as_ref(), &mut budget)?,
        ExplanationEvidencePayloadV2::OptimalityStatus { status } => {
            vec![render_optimality_status(
                document,
                status,
                objective_context,
                &mut budget,
            )?]
        }
        ExplanationEvidencePayloadV2::Counterfactual { result } => {
            vec![render_counterfactual(
                document,
                result,
                objective_context,
                &mut budget,
            )?]
        }
        ExplanationEvidencePayloadV2::SolutionDifference { comparison } => {
            vec![render_comparison(comparison, false, &mut budget)?]
        }
        ExplanationEvidencePayloadV2::Repair { .. } => {
            return Err(DomainPackError::UnsupportedExplanationCapability(
                eutheto_domain_ir::ExplanationCapability::Repair,
            ));
        }
    };
    reserve_rendered_output(&messages, &mut budget)?;
    let result =
        EvidenceRenderResultV1::new(request, messages).map_err(|_| contract("evidence_result"))?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(result)
}

/// Derives bounded current-source Repair observations, not historical baseline admission.
#[allow(clippy::too_many_lines)]
pub(crate) fn render_workforce_repair(
    base_document: &ScenarioDocument,
    base: &PortableAcceptedResult,
    candidate_document: &ScenarioDocument,
    candidate: &PortableAcceptedResult,
    request: &EvidenceRenderRequestV2,
    control: &OperationControl,
) -> Result<EvidenceRenderResultV1, DomainPackError> {
    let mut budget = OperationBudget::evaluation(Some(control));
    preflight(request, &mut budget, DOMAIN_LIMITS)
        .map_err(|error| operation_error(&error))?
        .reserve_json(&mut budget, 2)
        .map_err(|error| operation_error(&error))?;
    request.validate().map_err(|_| contract("repair.request"))?;
    let ExplanationEvidencePayloadV2::Repair { repair } = &request.evidence.evidence else {
        return Err(contract("repair.kind"));
    };
    for document in [base_document, candidate_document] {
        preflight(document, &mut budget, ContractJsonLimits::DEFAULT)
            .map_err(|error| operation_error(&error))?
            .reserve_json(&mut budget, 1)
            .map_err(|error| operation_error(&error))?;
    }
    for source in [base, candidate] {
        // Generic preparation validates nested checksums; reserve its scratch first.
        preflight(source, &mut budget, DOMAIN_LIMITS)
            .map_err(|error| operation_error(&error))?
            .reserve_json(&mut budget, 8)
            .map_err(|error| operation_error(&error))?;
    }
    let prepared = eutheto_domain_ir::prepare_comparison_v2(base, candidate, Some(control))
        .map_err(|error| match error {
            eutheto_domain_ir::ComparisonError::Interrupted(reason) => reason.into(),
            _ => contract("repair.comparison_sources"),
        })?;
    let base_input = AssignmentInput::new(base_document, &mut budget)
        .map_err(|error| operation_error(&error))?
        .with_objective_context(
            base.run_input.objective_context.as_ref(),
            base_document.scenario_id,
            &mut budget,
        )?;
    let input = AssignmentInput::new(candidate_document, &mut budget)
        .map_err(|error| operation_error(&error))?
        .with_objective_context(
            candidate.run_input.objective_context.as_ref(),
            candidate_document.scenario_id,
            &mut budget,
        )?;
    super::comparison::validate_source_binding(base, base_document, &base_input)?;
    super::comparison::validate_source_binding(candidate, candidate_document, &input)?;
    let comparison = &repair.comparison;
    let base_selected = super::comparison::selected_pairs(base, &base_input, &mut budget)?;
    let candidate_selected = super::comparison::selected_pairs(candidate, &input, &mut budget)?;
    let mut enrichment = super::comparison::enrich_prepared(
        base_document,
        base,
        &base_input,
        &base_selected,
        candidate_document,
        candidate,
        &input,
        &candidate_selected,
        &prepared,
        &mut budget,
    )?;
    // Native admission may conservatively withhold score authority even when the
    // recorded source policies match. Never upgrade that unavailable comparison.
    if matches!(
        &comparison.score,
        ScoreComparisonV2::Unavailable {
            reason: ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch,
        }
    ) {
        enrichment.scoring_contexts = None;
    }
    let expected = prepared.finish(enrichment).map_err(|error| match error {
        eutheto_domain_ir::ComparisonError::Interrupted(reason) => reason.into(),
        _ => contract("repair.comparison_sources"),
    })?;
    if comparison != &expected {
        return Err(contract("repair.comparison_body"));
    }
    let context = candidate.run_input.objective_context.as_ref();
    check_objective_context(context, candidate_document.scenario_id)?;
    let recorded = super::profile::snapshot_pairs(
        &input,
        candidate_document.scenario_id,
        context,
        &mut budget,
    )?;
    let prior = if let Some(pairs) = recorded {
        if pairs.is_sorted() {
            std::borrow::Cow::Borrowed(pairs)
        } else {
            reserve_repair_pairs(pairs.len(), &mut budget)?;
            budget
                .sort_work(pairs.len())
                .map_err(|error| operation_error(&error))?;
            let mut ordered = pairs.to_vec();
            ordered.sort_unstable();
            std::borrow::Cow::Owned(ordered)
        }
    } else {
        reserve_repair_pairs(base_selected.len(), &mut budget)?;
        std::borrow::Cow::Owned(base_selected.into_iter().collect::<Vec<_>>())
    };
    let mut changed = Vec::new();
    for pair in prior.iter() {
        budget
            .steps(search_work(candidate_selected.len()))
            .map_err(|error| operation_error(&error))?;
        if !candidate_selected.contains(pair) {
            reserve_repair_pairs(1, &mut budget)?;
            changed.push(*pair);
        }
    }
    for pair in &candidate_selected {
        budget
            .steps(search_work(prior.len()))
            .map_err(|error| operation_error(&error))?;
        if prior.binary_search(pair).is_err() {
            reserve_repair_pairs(1, &mut budget)?;
            changed.push(*pair);
        }
    }
    budget
        .sort_work(changed.len())
        .map_err(|error| operation_error(&error))?;
    changed.sort_unstable();
    let frame = if recorded.is_some() {
        "recordedBaselineToCandidate"
    } else {
        "acceptedBaseToCandidate"
    };
    let mut summary = render_comparison(comparison, true, &mut budget)?;
    append_repair_scope(
        &mut summary.parameters,
        "acceptedBaseToCandidate",
        candidate,
        &mut budget,
    )?;
    let mut messages = vec![summary];
    let mut complete = true;
    for pair in prior.iter() {
        budget.step().map_err(|error| operation_error(&error))?;
        if input
            .domain
            .entities
            .get(&eutheto_types::EntityId::from_uuid(
                pair.person_id.as_uuid(),
            ))
            .is_some_and(|entity| !matches!(entity, crate::model::WorkforceEntity::Person(_)))
        {
            return Err(contract("repair.historical_person_kind"));
        }
        let person = input.person(pair.person_id).is_some();
        let shift = input.shift(pair.shift_id).is_some();
        if person && shift {
            continue;
        }
        complete = false;
        reserve_repair_message_slot(&messages, &mut budget)?;
        messages.push(repair_endpoint_message(
            *pair,
            person,
            shift,
            frame,
            candidate,
            &mut budget,
        )?);
    }
    if complete {
        let (evaluations, partition, observations) =
            evaluate_repair_selection(candidate_document, &input, &prior, &changed, &mut budget)
                .map_err(|error| operation_error(&error))?;
        let obligations = original_obligations(&input, &mut budget)?;
        if !partition.remaining.is_empty()
            || partition.handled.len() != obligations.len()
            || evaluations.len() != obligations.len()
            || partition
                .handled
                .iter()
                .zip(&obligations)
                .any(|(id, (original, _))| id != original)
            || evaluations
                .iter()
                .zip(&obligations)
                .any(|(evaluation, (id, _))| evaluation.rule_id != *id)
        {
            return Err(contract("repair.incomplete_evaluation"));
        }
        for observation in observations {
            reserve_repair_message_slot(&messages, &mut budget)?;
            messages.push(render_repair_observation(
                observation,
                frame,
                candidate,
                &mut budget,
            )?);
        }
    } else {
        // Missing historical endpoints prevent complete aggregate proof. Pair-local checks are
        // still meaningful, but must not promote the reduced population to a complete selection.
        messages[0].parameters.insert(
            fact("repair_aggregate_scope")?,
            VerificationValue::Text("unavailableHistoricalEndpoints".to_owned()),
        );
        for pair in prior.iter().copied() {
            if input.person(pair.person_id).is_none() || input.shift(pair.shift_id).is_none() {
                continue;
            }
            for evaluation in evaluate_pair(
                &input,
                pair,
                true,
                &candidate_document.settings,
                &mut budget,
            )
            .map_err(|error| operation_error(&error))?
            {
                if evaluation.satisfied {
                    continue;
                }
                reserve_repair_message_slot(&messages, &mut budget)?;
                reserve_repair_pairs(1, &mut budget)?;
                let affected_pairs = if changed.binary_search(&pair).is_ok() {
                    vec![pair]
                } else {
                    Vec::new()
                };
                let message = render_repair_observation(
                    RepairObservation {
                        evaluation,
                        affected_pairs,
                    },
                    frame,
                    candidate,
                    &mut budget,
                )?;
                messages.push(message);
            }
        }
    }
    reserve_rendered_output(&messages, &mut budget)?;
    let result =
        EvidenceRenderResultV1::new(request, messages).map_err(|_| contract("repair.result"))?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(result)
}

fn reserve_repair_pairs(
    length: usize,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    let length = super::budget::count(length).map_err(|error| operation_error(&error))?;
    budget
        .reserve(
            0,
            length,
            length
                .checked_mul(32)
                .ok_or_else(|| contract("repair.arithmetic"))?,
        )
        .map_err(|error| operation_error(&error))
}

fn search_work(length: usize) -> u64 {
    1 + u64::from(usize::BITS - length.leading_zeros())
}

#[allow(clippy::too_many_lines)]
fn append_repair_scope(
    parameters: &mut BTreeMap<VerificationFactId, VerificationValue>,
    frame: &str,
    candidate: &PortableAcceptedResult,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    budget
        .reserve(1, 24, 4096)
        .map_err(|error| operation_error(&error))?;
    parameters.insert(
        fact("repair_frame")?,
        VerificationValue::Text(frame.to_owned()),
    );
    parameters.insert(
        fact("baseline_admission")?,
        VerificationValue::Text("notEstablished".to_owned()),
    );
    parameters.insert(
        fact("repair_source_revision")?,
        VerificationValue::Integer(
            i64::try_from(candidate.scenario_revision).map_err(|_| contract("repair.revision"))?,
        ),
    );
    parameters.insert(
        fact("repair_source_document_hash")?,
        VerificationValue::Text(candidate.run_input.snapshot_document_hash.clone()),
    );
    if let Some(binding) = candidate
        .run_input
        .objective_context
        .as_ref()
        .and_then(|context| context.baseline.as_deref())
    {
        parameters.insert(
            fact("baseline_pair_content_hash")?,
            VerificationValue::Text(binding.selected_pair_content_hash.clone()),
        );
        parameters.insert(
            fact("baseline_original_solution")?,
            VerificationValue::Text(binding.original.accepted_result.solution_id.to_string()),
        );
        parameters.insert(
            fact("baseline_original_revision")?,
            VerificationValue::Integer(
                i64::try_from(binding.original.source_revision)
                    .map_err(|_| contract("repair.revision"))?,
            ),
        );
        parameters.insert(
            fact("baseline_original_result_checksum")?,
            VerificationValue::Text(binding.original.accepted_result.result_checksum.clone()),
        );
        parameters.insert(
            fact("baseline_original_document_hash")?,
            VerificationValue::Text(binding.original.source_document_hash.clone()),
        );
        parameters.insert(
            fact("baseline_original_run")?,
            VerificationValue::Text(binding.original.source_run_id.to_string()),
        );
        parameters.insert(
            fact("baseline_original_input_checksum")?,
            VerificationValue::Text(binding.original.source_run_input_checksum.clone()),
        );
        parameters.insert(
            fact("baseline_original_manifest_checksum")?,
            VerificationValue::Text(binding.original.source_run_manifest_checksum.clone()),
        );
        let (kind, entity) = match &binding.selection {
            RunBaselineSelection::OriginalAccepted {} => {
                ("originalAccepted", &binding.original.entity)
            }
            RunBaselineSelection::ManualSelection {
                entity,
                record_checksum,
                admission_source_revision,
                admission_source_document_hash,
                ..
            } => {
                parameters.insert(
                    fact("baseline_record_checksum")?,
                    VerificationValue::Text(record_checksum.clone()),
                );
                parameters.insert(
                    fact("baseline_recorded_admission_revision")?,
                    VerificationValue::Integer(
                        i64::try_from(*admission_source_revision)
                            .map_err(|_| contract("repair.revision"))?,
                    ),
                );
                parameters.insert(
                    fact("baseline_recorded_admission_document_hash")?,
                    VerificationValue::Text(admission_source_document_hash.clone()),
                );
                ("manualSelection", entity)
            }
        };
        parameters.insert(
            fact("baseline_selection_kind")?,
            VerificationValue::Text(kind.to_owned()),
        );
        parameters.insert(
            fact("baseline_selection_entity")?,
            VerificationValue::Text(entity.id.as_str().to_owned()),
        );
    } else if let Some(binding) = candidate
        .run_input
        .objective_context
        .as_ref()
        .and_then(|context| context.external_baseline.as_deref())
    {
        parameters.insert(
            fact("baseline_selection_kind")?,
            VerificationValue::Text("externalStartingPlan".to_owned()),
        );
        parameters.insert(
            fact("baseline_selection_entity")?,
            VerificationValue::Text(binding.entity.id.as_str().to_owned()),
        );
        parameters.insert(
            fact("baseline_pair_content_hash")?,
            VerificationValue::Text(binding.selected_pair_content_hash.clone()),
        );
    }
    Ok(())
}

fn render_repair_observation(
    observation: RepairObservation,
    frame: &str,
    candidate: &PortableAcceptedResult,
    budget: &mut OperationBudget<'_>,
) -> Result<EvidenceMessageV1, DomainPackError> {
    let RepairObservation {
        evaluation,
        affected_pairs,
    } = observation;
    if evaluation.satisfied {
        return Err(contract("repair.observation"));
    }
    if affected_pairs.len() > MAX_EVIDENCE_MESSAGE_REFERENCES {
        return Err(DomainPackError::ResourceLimitExceeded);
    }
    let mut parameters = BTreeMap::new();
    for (prefix, facts) in [
        ("expected", evaluation.expected),
        ("observed", evaluation.observed),
    ] {
        for (key, value) in facts {
            let bytes = budget
                .measure(&value)
                .map_err(|error| operation_error(&error))?;
            budget
                .reserve(
                    0,
                    1,
                    bytes
                        .checked_add(160)
                        .ok_or_else(|| contract("repair.arithmetic"))?,
                )
                .map_err(|error| operation_error(&error))?;
            let key = VerificationFactId::new(format!("{prefix}.{}", key.as_str()))
                .map_err(|_| contract("repair.fact_key"))?;
            parameters.insert(key, value);
        }
    }
    append_repair_scope(&mut parameters, frame, candidate, budget)?;
    budget
        .reserve(1, 4, 1024)
        .map_err(|error| operation_error(&error))?;
    parameters.insert(fact("summary")?, VerificationValue::Text(
        "The recorded prior selection violates this condition in the candidate's immutable source. Referenced changes are participants or members of the affected group; this does not prove that a specific replacement was uniquely required.".to_owned(),
    ));
    reserve_repair_pairs(affected_pairs.len(), budget)?;
    let mut assignments = Vec::new();
    for pair in affected_pairs {
        budget
            .reserve(0, 1, 128)
            .map_err(|error| operation_error(&error))?;
        assignments.push(super::comparison::assignment_id(pair)?);
    }
    budget
        .sort_work(assignments.len())
        .map_err(|error| operation_error(&error))?;
    assignments.sort_unstable();
    assignments.dedup();
    Ok(EvidenceMessageV1 {
        message_key: "official.workforce.repair.source_condition".to_owned(),
        parameters,
        entities: evaluation.affected_entities,
        rules: vec![evaluation.rule_id],
        assignments,
        evidence: evaluation.evidence,
    })
}

fn repair_endpoint_message(
    pair: AssignmentPair,
    person: bool,
    shift: bool,
    frame: &str,
    candidate: &PortableAcceptedResult,
    budget: &mut OperationBudget<'_>,
) -> Result<EvidenceMessageV1, DomainPackError> {
    let mut parameters = BTreeMap::new();
    append_repair_scope(&mut parameters, frame, candidate, budget)?;
    budget
        .reserve(1, 5, 1024)
        .map_err(|error| operation_error(&error))?;
    parameters.insert(fact("summary")?, VerificationValue::Text(
        "A prior assignment endpoint is absent from the candidate's resolved immutable source. Complete aggregate evaluation of the historical selection is unavailable; this does not establish whether the endpoint was deleted or excluded.".to_owned(),
    ));
    parameters.insert(
        fact("person_resolvable")?,
        VerificationValue::Boolean(person),
    );
    parameters.insert(fact("shift_resolvable")?, VerificationValue::Boolean(shift));
    Ok(EvidenceMessageV1 {
        message_key: "official.workforce.repair.endpoint_unavailable".to_owned(),
        parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: vec![super::comparison::assignment_id(pair)?],
        evidence: Vec::new(),
    })
}

fn reserve_repair_message_slot(
    messages: &[EvidenceMessageV1],
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    if messages.len() >= MAX_EVIDENCE_MESSAGES {
        return Err(DomainPackError::ResourceLimitExceeded);
    }
    budget
        .reserve(1, 1, 192)
        .map_err(|error| operation_error(&error))?;
    Ok(())
}

fn reserve_rendered_output(
    messages: &[EvidenceMessageV1],
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    let output =
        preflight(messages, budget, DOMAIN_LIMITS).map_err(|error| operation_error(&error))?;
    output
        .reserve_json(budget, 1)
        .map_err(|error| operation_error(&error))?;
    budget
        .reserve(1, 6, 1024)
        .map_err(|error| operation_error(&error))
}

/// Trial retention is discarded; every output traversal uses the inherited work allowance.
fn check_rendered_output(
    messages: &[EvidenceMessageV1],
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    let output =
        preflight(messages, budget, DOMAIN_LIMITS).map_err(|error| operation_error(&error))?;
    let mut prospective = budget.clone();
    output
        .reserve_json(&mut prospective, 1)
        .and_then(|()| prospective.reserve(1, 6, 1024))
        .map_err(|error| operation_error(&error))
}

fn render_counterfactual(
    document: &ScenarioDocument,
    result: &CounterfactualResult,
    objective_context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<EvidenceMessageV1, DomainPackError> {
    budget.step().map_err(|error| operation_error(&error))?;
    if result.base_run_input.objective_context.as_ref() != objective_context
        || result.run_input.objective_context.as_ref() != objective_context
    {
        return Err(contract("objective_context_mismatch"));
    }
    let condition = &result.request.condition.condition;
    let (mut assignments, condition_summary) = match condition {
        CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id,
            replaced_assignment_id,
        } => (
            vec![target_assignment_id.clone(), replaced_assignment_id.clone()],
            "Require the replacement and exclude the recorded incumbent on the same shift.",
        ),
        CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(selected),
        } => (
            vec![assignment_id.clone()],
            if *selected {
                "Require this assignment to be selected."
            } else {
                "Require this assignment not to be selected."
            },
        ),
        CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(selected),
        } => (
            vec![assignment_id.clone()],
            if *selected {
                "Forbid this assignment from being selected."
            } else {
                "Forbid the unselected value: this assignment must be selected."
            },
        ),
        _ => return Err(contract("counterfactual_condition")),
    };
    if document.scenario_id != result.request.semantics.scenario_id
        || document.domain_pack.id != result.run_input.pack_id
        || document.domain_pack.schema_version != result.run_input.pack_schema_version
        || serde_json::to_vec(document)
            .map(|bytes| blake3_hex(&bytes))
            .map_err(|_| contract("counterfactual_document"))?
            != result.request.semantics.snapshot_document_hash
    {
        return Err(contract("counterfactual_document"));
    }
    let (key, summary) = match result.conclusion {
        CounterfactualConclusionV1::ProvenImpossible => (
            "official.workforce.counterfactual.proven_impossible",
            "The saved run proved that no schedule satisfies this recorded temporary condition under the original rules. The current scenario may have changed.",
        ),
        CounterfactualConclusionV1::VerifiedAlternative { .. } => (
            "official.workforce.counterfactual.verified_alternative",
            "The saved run found an independently verified alternative satisfying this temporary condition. Other assignments may also change. Compare the full results before acting; this result has not been applied.",
        ),
        CounterfactualConclusionV1::NotDistinguishedWithinBudget => (
            "official.workforce.counterfactual.inconclusive",
            "The saved run did not settle this temporary condition within its budget. This is not proof that the condition is impossible.",
        ),
    };
    let mut parameters = BTreeMap::new();
    parameters.insert(
        fact("summary")?,
        VerificationValue::Text(summary.to_owned()),
    );
    parameters.insert(
        fact("condition")?,
        VerificationValue::Text(condition_summary.to_owned()),
    );
    parameters.insert(
        fact("source_revision")?,
        VerificationValue::Text(result.request.semantics.scenario_revision.to_string()),
    );
    assignments.sort();
    Ok(EvidenceMessageV1 {
        message_key: key.to_owned(),
        parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments,
        evidence: Vec::new(),
    })
}

fn render_comparison(
    comparison: &SolutionComparisonV2,
    repair: bool,
    budget: &mut OperationBudget<'_>,
) -> Result<EvidenceMessageV1, DomainPackError> {
    let mut protection_state_changes = 0;
    let mut protected_selection_changes = 0;
    for protection in &comparison.protections {
        budget.step().map_err(|error| operation_error(&error))?;
        if !protection.state_preserved {
            protection_state_changes += 1;
        }
        if !protection.assignment_preserved {
            protected_selection_changes += 1;
        }
    }
    budget
        .reserve(1, 20, 4_096)
        .map_err(|error| operation_error(&error))?;

    let mut parameters = BTreeMap::from([
        (
            fact("assignment_changes")?,
            comparison_count(comparison.assignments.len())?,
        ),
        (
            fact("required_rule_changes")?,
            comparison_count(comparison.rules.len())?,
        ),
        (
            fact("metric_changes")?,
            comparison_count(comparison.metrics.len())?,
        ),
        (
            fact("affected_entity_count")?,
            comparison_count(comparison.affected_entities.len())?,
        ),
        (
            fact("protected_assignment_count")?,
            comparison_count(comparison.protections.len())?,
        ),
        (
            fact("protection_state_changes")?,
            comparison_count(protection_state_changes)?,
        ),
        (
            fact("protected_selection_changes")?,
            comparison_count(protected_selection_changes)?,
        ),
        (
            fact("base_run_status")?,
            VerificationValue::Text(status_label(&comparison.runs.base.outcome).to_owned()),
        ),
        (
            fact("candidate_run_status")?,
            VerificationValue::Text(status_label(&comparison.runs.candidate.outcome).to_owned()),
        ),
    ]);
    let score_status = insert_comparison_score(&comparison.score, &mut parameters, budget)?;
    parameters.insert(
        fact("score_status")?,
        VerificationValue::Text(score_status.to_owned()),
    );
    parameters.insert(
        fact("summary")?,
        VerificationValue::Text(
            comparison_summary(repair, score_status == "comparable").to_owned(),
        ),
    );
    if repair {
        parameters.insert(
            fact("repair_causality")?,
            VerificationValue::Text("notEstablished".to_owned()),
        );
    }
    Ok(EvidenceMessageV1 {
        message_key: if repair {
            "official.workforce.explanation.repair"
        } else {
            "official.workforce.explanation.solution_difference"
        }
        .to_owned(),
        parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: Vec::new(),
        evidence: Vec::new(),
    })
}

fn insert_comparison_score(
    score: &ScoreComparisonV2,
    parameters: &mut BTreeMap<VerificationFactId, VerificationValue>,
    budget: &mut OperationBudget<'_>,
) -> Result<&'static str, DomainPackError> {
    match score {
        ScoreComparisonV2::Comparable {
            score_levels,
            ordering,
            ..
        } => {
            let mut categories = 0_usize;
            for level in score_levels {
                budget.step().map_err(|error| operation_error(&error))?;
                categories = categories
                    .checked_add(level.categories.len())
                    .ok_or_else(|| contract("comparison_count"))?;
                for _ in &level.categories {
                    budget.step().map_err(|error| operation_error(&error))?;
                }
            }
            parameters.insert(
                fact("score_ordering")?,
                VerificationValue::Text(ordering_label(*ordering).to_owned()),
            );
            parameters.insert(
                fact("score_levels_compared")?,
                comparison_count(score_levels.len())?,
            );
            parameters.insert(
                fact("score_categories_compared")?,
                comparison_count(categories)?,
            );
            Ok("comparable")
        }
        ScoreComparisonV2::Unavailable { reason } => {
            parameters.insert(
                fact("score_unavailable_reason")?,
                VerificationValue::Text(score_unavailable_reason_label(*reason).to_owned()),
            );
            Ok("unavailable")
        }
    }
}

fn comparison_count(value: usize) -> Result<VerificationValue, DomainPackError> {
    i64::try_from(value)
        .map(VerificationValue::Integer)
        .map_err(|_| contract("comparison_count"))
}

const fn comparison_summary(repair: bool, score_comparable: bool) -> &'static str {
    match (repair, score_comparable) {
        (true, true) => {
            "Assignment, Required-rule, metric, affected-entity and protection differences are recorded. Exact score/category deltas and candidate objective ordering are available under compatible recorded scoring contexts. Recorded run status does not authenticate historical backend proof. Repair causality and universal fairness are not established."
        }
        (true, false) => {
            "Assignment, Required-rule, metric, affected-entity and protection differences are recorded. Aggregate scores and ordering are unavailable; no score order is claimed. Recorded run status does not authenticate historical backend proof. Repair causality and universal fairness are not established."
        }
        (false, true) => {
            "Assignment, Required-rule, metric, affected-entity and protection differences are recorded. Exact score/category deltas and candidate objective ordering are available under compatible recorded scoring contexts. Recorded run status does not authenticate historical backend proof. This does not establish causality or universal fairness."
        }
        (false, false) => {
            "Assignment, Required-rule, metric, affected-entity and protection differences are recorded. Aggregate scores and ordering are unavailable; no score order is claimed. Recorded run status does not authenticate historical backend proof. This does not establish causality or universal fairness."
        }
    }
}

const fn ordering_label(ordering: ComparisonOrdering) -> &'static str {
    match ordering {
        ComparisonOrdering::Better => "better",
        ComparisonOrdering::Equivalent => "equivalent",
        ComparisonOrdering::Worse => "worse",
    }
}

const fn score_unavailable_reason_label(
    reason: ScoreComparisonUnavailableReasonV1,
) -> &'static str {
    match reason {
        ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch => "policyOrScopeMismatch",
        ScoreComparisonUnavailableReasonV1::ScoreShapeMismatch => "scoreShapeMismatch",
    }
}

#[allow(clippy::too_many_lines)]
fn render_optimality_status(
    document: &ScenarioDocument,
    status: &OptimalityStatusEvidence,
    objective_context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<EvidenceMessageV1, DomainPackError> {
    budget.step().map_err(|error| operation_error(&error))?;
    if status.run_input.objective_context.as_ref() != objective_context {
        return Err(contract("objective_context_mismatch"));
    }
    if document.scenario_id != status.run_input.scenario_id
        || document.domain_pack.id != status.run_input.pack_id
        || document.domain_pack.schema_version != status.run_input.pack_schema_version
    {
        return Err(contract("status_document"));
    }
    let document_hash = serde_json::to_vec(document)
        .map(|bytes| blake3_hex(&bytes))
        .map_err(|_| contract("status_document"))?;
    if document_hash != status.run_input.snapshot_document_hash {
        return Err(contract("status_document"));
    }

    let (message_key, summary) = match &status.run_manifest.outcome {
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Optimal,
            ..
        } => (
            "official.workforce.optimality_status.backend_recorded_proof",
            "Historical backend record reports an optimal terminal status. The accepted candidate was independently verified feasible; this explanation does not claim optimality for the current scenario source.",
        ),
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            ..
        } => (
            "official.workforce.optimality_status.verified_feasible",
            "Historical backend record reports a feasible terminal status. The accepted candidate was independently verified feasible; optimality is not claimed and this does not establish a current-source result.",
        ),
        RunTerminalOutcomeV1::Accepted { .. } => return Err(contract("status_outcome")),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible | SolveStatus::Unbounded,
        } => (
            "official.workforce.optimality_status.backend_recorded_status",
            "Historical run recorded no accepted result and a backend terminal status indicating infeasibility or unboundedness. This records the backend status only; it is not independent proof about the original rules or current source.",
        ),
        RunTerminalOutcomeV1::NoResult { .. } => (
            "official.workforce.optimality_status.unresolved",
            "Historical run recorded no accepted result. The terminal status is retained for review; no feasibility or optimality claim is made.",
        ),
        RunTerminalOutcomeV1::VerificationAlarm { .. } => (
            "official.workforce.optimality_status.unresolved",
            "Historical run recorded no accepted result because independent verification raised a correctness alarm. No feasibility or optimality claim is made.",
        ),
        RunTerminalOutcomeV1::Interrupted => (
            "official.workforce.optimality_status.interrupted",
            "Historical run was interrupted before a terminal outcome was recorded. No feasibility or optimality claim is made.",
        ),
    };

    budget
        .reserve(1, 12, 4096)
        .map_err(|error| operation_error(&error))?;
    let mut parameters = BTreeMap::new();
    parameters.insert(
        fact("summary")?,
        VerificationValue::Text(summary.to_owned()),
    );
    parameters.insert(
        fact("solve_run_id")?,
        VerificationValue::Text(status.run_input.run_id.to_string()),
    );
    parameters.insert(
        fact("scenario_revision")?,
        VerificationValue::Text(status.run_input.scenario_revision.to_string()),
    );
    parameters.insert(
        fact("run_input_checksum")?,
        VerificationValue::Text(status.run_input.checksum.clone()),
    );
    parameters.insert(
        fact("run_manifest_checksum")?,
        VerificationValue::Text(status.run_manifest.checksum.clone()),
    );
    parameters.insert(
        fact("status")?,
        VerificationValue::Text(status_label(&status.run_manifest.outcome).to_owned()),
    );
    if let Some(result) = &status.result {
        parameters.insert(
            fact("accepted_solution_id")?,
            VerificationValue::Text(result.solution_id.to_string()),
        );
        parameters.insert(
            fact("accepted_result_checksum")?,
            VerificationValue::Text(result.result_checksum.clone()),
        );
    }
    if let RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code } =
        &status.run_manifest.outcome
    {
        parameters.insert(
            fact("diagnostic_code")?,
            VerificationValue::Text(diagnostic_code.clone()),
        );
    }
    Ok(EvidenceMessageV1 {
        message_key: message_key.to_owned(),
        parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: Vec::new(),
        evidence: Vec::new(),
    })
}

fn status_label(outcome: &RunTerminalOutcomeV1) -> &'static str {
    match outcome {
        RunTerminalOutcomeV1::Accepted { status, .. }
        | RunTerminalOutcomeV1::NoResult { status } => match status {
            SolveStatus::Optimal => "optimal",
            SolveStatus::Feasible => "feasible",
            SolveStatus::Infeasible => "infeasible",
            SolveStatus::Unbounded => "unbounded",
            SolveStatus::NoSolutionWithinLimit => "noSolutionWithinLimit",
            SolveStatus::Cancelled => "cancelled",
            SolveStatus::InvalidModel => "invalidModel",
            SolveStatus::BackendUnavailable => "backendUnavailable",
            SolveStatus::BackendFailed => "backendFailed",
        },
        RunTerminalOutcomeV1::VerificationAlarm { .. } => "verificationAlarm",
        RunTerminalOutcomeV1::Interrupted => "interrupted",
    }
}

#[allow(clippy::too_many_lines)]
fn render_infeasibility(
    document: &ScenarioDocument,
    infeasibility: &InfeasibilityEvidenceV1,
    condition: Option<&CounterfactualConditionV1>,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<EvidenceMessageV1>, DomainPackError> {
    budget.step().map_err(|error| operation_error(&error))?;
    let group_count = infeasibility_group_count(infeasibility, budget)?;
    reserve_infeasibility_context(infeasibility, group_count, budget)?;

    let input = AssignmentInput::new(document, budget).map_err(|error| operation_error(&error))?;
    let obligations = original_obligations(&input, budget)?;
    let mut required_rules = BTreeSet::new();
    let mut fixed_fact_count = 0_u64;
    let mut hard_lock_count = 0_u64;
    for (rule_id, role) in obligations {
        budget.step().map_err(|error| operation_error(&error))?;
        match role {
            "required_rule" => {
                required_rules.insert(rule_id);
            }
            "person_activity" | "approved_leave" => {
                fixed_fact_count = fixed_fact_count
                    .checked_add(1)
                    .ok_or_else(|| contract("infeasibility_count"))?;
            }
            "hard_lock" => {
                hard_lock_count = hard_lock_count
                    .checked_add(1)
                    .ok_or_else(|| contract("infeasibility_count"))?;
            }
            _ => return Err(contract("infeasibility_source_role")),
        }
    }
    budget.check().map_err(|error| operation_error(&error))?;

    let mut messages = Vec::with_capacity(
        group_count
            .checked_add(2)
            .ok_or_else(|| contract("infeasibility_count"))?,
    );
    let condition_assignments = condition_assignment_refs(condition);
    let InfeasibilityStatus {
        status,
        summary,
        reason,
        shrink,
    } = infeasibility_status(infeasibility, group_count)?;
    let mut status_parameters = BTreeMap::new();
    status_parameters.insert(
        fact("summary")?,
        VerificationValue::Text(summary.to_owned()),
    );
    status_parameters.insert(fact("status")?, VerificationValue::Text(status.to_owned()));
    status_parameters.insert(
        fact("fixed_fact_count")?,
        VerificationValue::Integer(
            i64::try_from(fixed_fact_count).map_err(|_| contract("infeasibility_count"))?,
        ),
    );
    status_parameters.insert(
        fact("hard_lock_count")?,
        VerificationValue::Integer(
            i64::try_from(hard_lock_count).map_err(|_| contract("infeasibility_count"))?,
        ),
    );
    if let Some(reason) = reason {
        status_parameters.insert(
            fact("reason")?,
            VerificationValue::Text(unavailable_reason_label(reason).to_owned()),
        );
    }
    if let Some(shrink) = shrink {
        status_parameters.insert(
            fact("initial_group_count")?,
            VerificationValue::Integer(i64::from(shrink.initial_group_count)),
        );
        status_parameters.insert(
            fact("remaining_group_count")?,
            VerificationValue::Integer(i64::from(shrink.remaining_group_count)),
        );
        status_parameters.insert(
            fact("attempted_trials")?,
            VerificationValue::Integer(i64::from(shrink.attempted_trials)),
        );
        status_parameters.insert(
            fact("max_trials")?,
            VerificationValue::Integer(i64::from(shrink.max_trials)),
        );
        status_parameters.insert(
            fact("stop_reason")?,
            VerificationValue::Text(shrink_stop_reason_label(shrink.stop_reason).to_owned()),
        );
    }
    append_condition_parameters(&mut status_parameters, condition)?;
    messages.push(EvidenceMessageV1 {
        message_key: "official.workforce.infeasibility.status".to_owned(),
        parameters: status_parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: condition_assignments,
        evidence: Vec::new(),
    });

    let mut fixed_parameters = BTreeMap::new();
    fixed_parameters.insert(
        fact("summary")?,
        VerificationValue::Text(
            "Fixed activity and approved-leave facts, together with hard locks, remain enforced and are outside mapped conflict groups."
                .to_owned(),
        ),
    );
    fixed_parameters.insert(
        fact("fixed_fact_count")?,
        VerificationValue::Integer(
            i64::try_from(fixed_fact_count).map_err(|_| contract("infeasibility_count"))?,
        ),
    );
    fixed_parameters.insert(
        fact("hard_lock_count")?,
        VerificationValue::Integer(
            i64::try_from(hard_lock_count).map_err(|_| contract("infeasibility_count"))?,
        ),
    );
    messages.push(EvidenceMessageV1 {
        message_key: "official.workforce.infeasibility.fixed_scope".to_owned(),
        parameters: fixed_parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: Vec::new(),
        evidence: Vec::new(),
    });

    if let InfeasibilityEvidenceV1::Conflict { groups, .. } = infeasibility {
        for group in groups {
            budget.step().map_err(|error| operation_error(&error))?;
            let [rule_id] = group.required_rules.as_slice() else {
                return Err(contract("infeasibility_source_group"));
            };
            if !required_rules.contains(rule_id) || input.domain.preferences.contains_key(rule_id) {
                return Err(contract("infeasibility_source_rule"));
            }
            let Some(rule) = input.domain.rules.get(rule_id) else {
                return Err(contract("infeasibility_source_rule"));
            };
            let (actual_id, active, _) = rule.header();
            if actual_id != *rule_id || !active {
                return Err(contract("infeasibility_source_rule"));
            }
            let (source_kind_key, _) = source_kind(rule);
            let source_caption = source_caption(&input, rule_id, rule)?;
            let mut parameters = BTreeMap::new();
            parameters.insert(
                fact("summary")?,
                VerificationValue::Text(
                    "This complete mapped source group contributes to the recorded infeasibility evidence; it is not a unique explanation."
                        .to_owned(),
                ),
            );
            parameters.insert(
                fact("source_kind")?,
                VerificationValue::Text(format!("official.workforce.rule.{source_kind_key}")),
            );
            parameters.insert(
                fact("group_id")?,
                VerificationValue::Text(group.group_id.to_string()),
            );
            parameters.insert(
                fact("source_caption")?,
                VerificationValue::Text(source_caption),
            );
            parameters.insert(
                fact("source_rule_count")?,
                VerificationValue::Integer(
                    i64::try_from(group.required_rules.len())
                        .map_err(|_| contract("infeasibility_count"))?,
                ),
            );
            messages.push(EvidenceMessageV1 {
                message_key: "official.workforce.infeasibility.conflict_group".to_owned(),
                parameters,
                entities: Vec::new(),
                rules: group.required_rules.clone(),
                assignments: Vec::new(),
                evidence: Vec::new(),
            });
        }
    }
    Ok(messages)
}

fn infeasibility_group_count(
    infeasibility: &InfeasibilityEvidenceV1,
    budget: &mut OperationBudget<'_>,
) -> Result<usize, DomainPackError> {
    budget.step().map_err(|error| operation_error(&error))?;
    match infeasibility {
        InfeasibilityEvidenceV1::Conflict { groups, .. } => {
            if groups.len() > MAX_DIAGNOSTIC_GROUPS {
                return Err(DomainPackError::ResourceLimitExceeded);
            }
            Ok(groups.len())
        }
        InfeasibilityEvidenceV1::Unavailable { .. } => Ok(0),
    }
}

fn reserve_infeasibility_context(
    infeasibility: &InfeasibilityEvidenceV1,
    group_count: usize,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    let mut items = 24_u64;
    let mut bytes = 4_096_u64;
    if let InfeasibilityEvidenceV1::Conflict { groups, .. } = infeasibility {
        for group in groups {
            budget.step().map_err(|error| operation_error(&error))?;
            if group.required_rules.len() > MAX_EVIDENCE_MESSAGE_REFERENCES {
                return Err(DomainPackError::ResourceLimitExceeded);
            }
            items = items
                .checked_add(
                    u64::try_from(group.required_rules.len())
                        .map_err(|_| DomainPackError::ResourceLimitExceeded)?,
                )
                .and_then(|value| value.checked_add(8))
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            let rule_bytes = group
                .required_rules
                .iter()
                .try_fold(0_u64, |total, _rule_id| {
                    total
                        .checked_add(36)
                        .and_then(|value| value.checked_add(1))
                        .ok_or(DomainPackError::ResourceLimitExceeded)
                })?;
            let caption_bytes = u64::try_from(group.required_rules.len())
                .map_err(|_| DomainPackError::ResourceLimitExceeded)?
                .checked_mul(
                    u64::try_from(MAX_INFEASIBILITY_CAPTION_BYTES)
                        .map_err(|_| DomainPackError::ResourceLimitExceeded)?,
                )
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            let max_text_bytes = u64::try_from(eutheto_domain_ir::MAX_VERIFICATION_TEXT_BYTES)
                .map_err(|_| DomainPackError::ResourceLimitExceeded)?;
            bytes = bytes
                .checked_add(
                    rule_bytes
                        .checked_add(caption_bytes)
                        .and_then(|value| value.checked_add(max_text_bytes))
                        .and_then(|value| value.checked_add(768))
                        .ok_or(DomainPackError::ResourceLimitExceeded)?,
                )
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
        }
    }
    let records = u64::try_from(
        group_count
            .checked_add(2)
            .ok_or(DomainPackError::ResourceLimitExceeded)?,
    )
    .map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    budget
        .reserve(records, items, bytes)
        .map_err(|error| operation_error(&error))
}

struct InfeasibilityStatus<'a> {
    status: &'static str,
    summary: &'static str,
    reason: Option<ConflictUnavailableReason>,
    shrink: Option<&'a eutheto_domain_ir::ConflictShrinkSummaryV1>,
}

fn infeasibility_status(
    infeasibility: &InfeasibilityEvidenceV1,
    group_count: usize,
) -> Result<InfeasibilityStatus<'_>, DomainPackError> {
    match infeasibility {
        InfeasibilityEvidenceV1::Conflict {
            minimality, shrink, ..
        } => {
            if shrink.remaining_group_count as usize != group_count {
                return Err(contract("infeasibility_group_count"));
            }
            match minimality {
                ConflictMinimality::Sufficient => Ok(InfeasibilityStatus {
                    status: "sufficient",
                    summary: "The recorded mapped conflict is sufficient for the historical infeasibility result. It is not a unique or minimal explanation.",
                    reason: None,
                    shrink: Some(shrink),
                }),
                ConflictMinimality::ProvenMinimal => Ok(InfeasibilityStatus {
                    status: "minimal",
                    summary: "The recorded mapped conflict is proven minimal by the completed bounded deletion trials. Minimality does not imply uniqueness.",
                    reason: None,
                    shrink: Some(shrink),
                }),
            }
        }
        InfeasibilityEvidenceV1::Unavailable { reason } => {
            if *reason == ConflictUnavailableReason::FoundationalInfeasibility {
                Ok(InfeasibilityStatus {
                    status: "foundational",
                    summary: "The historical diagnostic record attributes infeasibility to fixed foundational constraints; no mapped Required-rule conflict is claimed.",
                    reason: Some(*reason),
                    shrink: None,
                })
            } else {
                Ok(InfeasibilityStatus {
                    status: "unavailable",
                    summary: "A complete mapped conflict was not retained for this historical run. No sufficient, minimal, or unique explanation is claimed.",
                    reason: Some(*reason),
                    shrink: None,
                })
            }
        }
    }
}

fn unavailable_reason_label(reason: ConflictUnavailableReason) -> &'static str {
    match reason {
        ConflictUnavailableReason::AssumptionsUnavailable => "assumptionsUnavailable",
        ConflictUnavailableReason::FoundationalInfeasibility => "foundationalInfeasibility",
        ConflictUnavailableReason::ConflictNotReturned => "conflictNotReturned",
        ConflictUnavailableReason::InvalidAssumptionCore => "invalidAssumptionCore",
        ConflictUnavailableReason::NotRequested => "notRequested",
        ConflictUnavailableReason::NotRecorded => "notRecorded",
        ConflictUnavailableReason::UnsupportedDomainPack => "unsupportedDomainPack",
        ConflictUnavailableReason::BudgetExpired => "budgetExpired",
        ConflictUnavailableReason::Cancelled => "cancelled",
        ConflictUnavailableReason::ResourceLimitExceeded => "resourceLimitExceeded",
        ConflictUnavailableReason::DiagnosticFailed => "diagnosticFailed",
    }
}

fn shrink_stop_reason_label(reason: ConflictShrinkStopReason) -> &'static str {
    match reason {
        ConflictShrinkStopReason::Completed => "completed",
        ConflictShrinkStopReason::NotAttempted => "notAttempted",
        ConflictShrinkStopReason::TrialLimit => "trialLimit",
        ConflictShrinkStopReason::BudgetExpired => "budgetExpired",
        ConflictShrinkStopReason::Cancelled => "cancelled",
        ConflictShrinkStopReason::Inconclusive => "inconclusive",
    }
}

fn append_condition_parameters(
    parameters: &mut BTreeMap<VerificationFactId, VerificationValue>,
    condition: Option<&CounterfactualConditionV1>,
) -> Result<(), DomainPackError> {
    let Some(condition) = condition else {
        return Ok(());
    };
    parameters.insert(
        fact("condition_checksum")?,
        VerificationValue::Text(condition.checksum.clone()),
    );
    match &condition.condition {
        CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value,
        } => {
            parameters.insert(
                fact("condition_kind")?,
                VerificationValue::Text("force".to_owned()),
            );
            parameters.insert(
                fact("condition_assignment_id")?,
                VerificationValue::Text(assignment_id.to_string()),
            );
            append_condition_value(parameters, value)?;
        }
        CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id,
            value,
        } => {
            parameters.insert(
                fact("condition_kind")?,
                VerificationValue::Text("forbid".to_owned()),
            );
            parameters.insert(
                fact("condition_assignment_id")?,
                VerificationValue::Text(assignment_id.to_string()),
            );
            append_condition_value(parameters, value)?;
        }
        CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id,
            replaced_assignment_id,
        } => {
            parameters.insert(
                fact("condition_kind")?,
                VerificationValue::Text("replace".to_owned()),
            );
            parameters.insert(
                fact("condition_target_assignment_id")?,
                VerificationValue::Text(target_assignment_id.to_string()),
            );
            parameters.insert(
                fact("condition_replaced_assignment_id")?,
                VerificationValue::Text(replaced_assignment_id.to_string()),
            );
            parameters.insert(
                fact("condition_target_value")?,
                VerificationValue::Boolean(true),
            );
            parameters.insert(
                fact("condition_replaced_value")?,
                VerificationValue::Boolean(false),
            );
        }
    }
    Ok(())
}

fn append_condition_value(
    parameters: &mut BTreeMap<VerificationFactId, VerificationValue>,
    value: &AssignmentValue,
) -> Result<(), DomainPackError> {
    let (value_type, rendered) = match value {
        AssignmentValue::Boolean(value) => ("boolean", VerificationValue::Boolean(*value)),
        AssignmentValue::Integer(value) => ("integer", VerificationValue::Integer(*value)),
        AssignmentValue::Interval(value) => (
            "interval",
            VerificationValue::Text(format!("{}:{}:{}", value.start, value.duration, value.end)),
        ),
        AssignmentValue::Absent => ("absent", VerificationValue::Text("absent".to_owned())),
    };
    parameters.insert(
        fact("condition_value_type")?,
        VerificationValue::Text(value_type.to_owned()),
    );
    parameters.insert(fact("condition_value")?, rendered);
    let value_json =
        serde_json::to_string(value).map_err(|_| contract("infeasibility_condition"))?;
    parameters.insert(
        fact("condition_value_json")?,
        VerificationValue::Text(value_json),
    );
    Ok(())
}

fn condition_assignment_refs(
    condition: Option<&CounterfactualConditionV1>,
) -> Vec<eutheto_domain_ir::DomainAssignmentId> {
    let Some(condition) = condition else {
        return Vec::new();
    };
    match &condition.condition {
        CounterfactualConditionPayloadV1::ForceAssignmentValue { assignment_id, .. }
        | CounterfactualConditionPayloadV1::ForbidAssignmentValue { assignment_id, .. } => {
            vec![assignment_id.clone()]
        }
        CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id,
            replaced_assignment_id,
        } => {
            let mut assignments =
                vec![target_assignment_id.clone(), replaced_assignment_id.clone()];
            assignments.sort();
            assignments
        }
    }
}

fn source_caption(
    input: &AssignmentInput,
    rule_id: &RuleId,
    rule: &WorkforceRule,
) -> Result<String, DomainPackError> {
    let (kind_key, kind_title) = source_kind(rule);
    let caption = if let WorkforceRule::FixedAssignment {
        person_id,
        shift_id,
        assigned,
        ..
    } = rule
    {
        let person_name = input
            .person(*person_id)
            .map_or("unknown person", |person| person.name.as_str());
        let shift_name = source_shift_name(input, *shift_id);
        if *assigned {
            format!(
                "{kind_title}: require {person_name} ({person_id}) on exact shift {shift_name} ({shift_id})"
            )
        } else {
            format!(
                "{kind_title}: exclude exactly {person_name} ({person_id}) from exact shift {shift_name} ({shift_id})"
            )
        }
    } else {
        format!("{kind_title} ({kind_key}) rule {rule_id}")
    };
    truncate_caption(&caption)
}

fn source_kind(rule: &WorkforceRule) -> (&'static str, &'static str) {
    match rule {
        WorkforceRule::Eligibility { .. } => ("eligibility", "Eligibility"),
        WorkforceRule::Availability { .. } => ("availability", "Availability"),
        WorkforceRule::Coverage { .. } => ("coverage", "Coverage"),
        WorkforceRule::NoOverlap { .. } => ("no-overlap", "No overlap"),
        WorkforceRule::MinimumRest(_) => ("minimum-rest", "Minimum rest"),
        WorkforceRule::MaximumAssignmentCount { .. } => {
            ("maximum-assignment-count", "Maximum assignment count")
        }
        WorkforceRule::WorkloadQuota(_) => ("workload-quota", "Workload quota"),
        WorkforceRule::MaximumHours { .. } => ("maximum-hours", "Maximum hours"),
        WorkforceRule::MaximumConsecutive { .. } => ("maximum-consecutive", "Maximum consecutive"),
        WorkforceRule::MaximumWorkedWeekends { .. } => {
            ("maximum-worked-weekends", "Maximum worked weekends")
        }
        WorkforceRule::RequiredSkillMix { .. } => ("required-skill-mix", "Required skill mix"),
        WorkforceRule::FixedAssignment { .. } => ("fixed-assignment", "Fixed assignment"),
        WorkforceRule::MutualAssignmentRestriction { .. } => (
            "mutual-assignment-restriction",
            "Mutual assignment restriction",
        ),
        WorkforceRule::TransitionTime { .. } => ("transition-time", "Transition time"),
    }
}

fn source_shift_name(input: &AssignmentInput, shift_id: crate::ids::ShiftId) -> String {
    let fallback = shift_id.to_string();
    let Some(shift) = input.shift(shift_id) else {
        return fallback;
    };
    let entity_id = match shift.origin {
        crate::temporal::ResolvedShiftOrigin::Generated { template_id, .. }
        | crate::temporal::ResolvedShiftOrigin::Detached { template_id, .. } => {
            template_id.as_entity_id()
        }
        crate::temporal::ResolvedShiftOrigin::Manual => shift_id.as_entity_id(),
    };
    match input.domain.entities.get(&entity_id) {
        Some(crate::model::WorkforceEntity::ShiftTemplate(template)) => template.name.clone(),
        _ => fallback,
    }
}

fn truncate_caption(value: &str) -> Result<String, DomainPackError> {
    if value.is_empty() {
        return Err(contract("infeasibility_source_caption"));
    }
    if value.len() <= MAX_INFEASIBILITY_CAPTION_BYTES {
        return Ok(value.to_owned());
    }
    let marker = "…";
    let limit = MAX_INFEASIBILITY_CAPTION_BYTES
        .checked_sub(marker.len())
        .ok_or_else(|| contract("infeasibility_caption"))?;
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end = end
            .checked_sub(1)
            .ok_or_else(|| contract("infeasibility_caption"))?;
    }
    let mut truncated = String::with_capacity(MAX_INFEASIBILITY_CAPTION_BYTES);
    truncated.push_str(&value[..end]);
    truncated.push_str(marker);
    Ok(truncated)
}
pub(crate) fn validate_workforce_full(
    document: &ScenarioDocument,
    control: &OperationControl,
) -> Result<DomainValidationReport, DomainPackError> {
    let mut budget = OperationBudget::evaluation(Some(control));
    let result = preflight(document, &mut budget, ContractJsonLimits::DEFAULT)
        .and_then(|measured| measured.reserve_json(&mut budget, 1))
        .and_then(|()| {
            analyze_source_with_budget(document, &mut budget, PlanningIrLimitsV1::DEFAULT)
        });
    budget.check().map_err(|error| operation_error(&error))?;
    match result {
        Ok(analysis) => Ok(analysis.validation),
        Err(
            error @ (super::AssignmentRuleError::Cancelled
            | super::AssignmentRuleError::BudgetExpired
            | super::AssignmentRuleError::LimitExceeded(_)
            | super::AssignmentRuleError::InvalidDocument(
                DomainPackError::ResourceLimitExceeded,
            )),
        ) => Err(operation_error(&error)),
        Err(error) => Ok(DomainValidationReport {
            issues: vec![error.validation_issue(document)],
        }),
    }
}

fn render_validation(
    document: &ScenarioDocument,
    issue: &ValidationIssueEvidenceV1,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<EvidenceMessageV1>, DomainPackError> {
    if !issue.parameters.is_empty()
        || issue.field_path.is_some()
        || issue.entity.is_some()
        || issue.rule_id.is_some()
    {
        return Err(contract("validation_source"));
    }
    let report = match analyze_source_with_budget(document, budget, PlanningIrLimitsV1::DEFAULT) {
        Ok(analysis) => analysis.validation,
        Err(
            error @ (super::AssignmentRuleError::Cancelled
            | super::AssignmentRuleError::BudgetExpired
            | super::AssignmentRuleError::LimitExceeded(_)
            | super::AssignmentRuleError::InvalidDocument(
                DomainPackError::ResourceLimitExceeded,
            )),
        ) => return Err(operation_error(&error)),
        Err(error) => DomainValidationReport {
            issues: vec![error.validation_issue(document)],
        },
    };
    let mut selected = None;
    for current in &report.issues {
        budget.step().map_err(|error| operation_error(&error))?;
        let current_id = eutheto_domain_ir::validation_issue_identity(current)
            .map_err(|_| contract("validation_identity"))?;
        if current_id == issue.issue_id && selected.replace(current).is_some() {
            return Err(contract("validation_ambiguous"));
        }
    }
    let current = selected.ok_or_else(|| contract("validation_not_current"))?;
    if current.code != issue.message_key || severity(current.severity) != issue.severity {
        return Err(contract("validation_not_current"));
    }
    budget
        .reserve(1, 6, 2048)
        .map_err(|error| operation_error(&error))?;
    let mut parameters = BTreeMap::new();
    parameters.insert(
        fact("summary")?,
        VerificationValue::Text(current.message.clone()),
    );
    let mut messages = vec![EvidenceMessageV1 {
        message_key: current.code.clone(),
        parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: Vec::new(),
        evidence: Vec::new(),
    }];
    if current.code == "official.workforce.candidate_shortage"
        && current.severity == ValidationSeverity::Error
    {
        let proof = render_candidate_shortage_proof(document, current, budget);
        match proof {
            Ok(Some(proof)) => {
                messages.push(proof);
                match check_rendered_output(&messages, budget) {
                    Ok(()) => {}
                    Err(DomainPackError::ResourceLimitExceeded) => {
                        messages.pop();
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(None) | Err(DomainPackError::ResourceLimitExceeded) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(messages)
}

#[allow(clippy::too_many_lines)]
fn render_candidate_shortage_proof(
    document: &ScenarioDocument,
    issue: &eutheto_types::ValidationIssue,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<EvidenceMessageV1>, DomainPackError> {
    if issue.code != "official.workforce.candidate_shortage"
        || issue.severity != ValidationSeverity::Error
    {
        return Ok(None);
    }
    let input = match AssignmentInput::new(document, budget) {
        Ok(input) => input,
        Err(
            error @ (super::AssignmentRuleError::Cancelled
            | super::AssignmentRuleError::BudgetExpired
            | super::AssignmentRuleError::LimitExceeded(_)
            | super::AssignmentRuleError::InvalidDocument(
                DomainPackError::ResourceLimitExceeded,
            )),
        ) => return Err(operation_error(&error)),
        Err(_) => return Ok(None),
    };
    let source = match candidate_shortage_source(&input, issue, budget) {
        Ok(Some(source)) => source,
        Err(
            error @ (super::AssignmentRuleError::Cancelled
            | super::AssignmentRuleError::BudgetExpired
            | super::AssignmentRuleError::LimitExceeded(_)
            | super::AssignmentRuleError::InvalidDocument(
                DomainPackError::ResourceLimitExceeded,
            )),
        ) => return Err(operation_error(&error)),
        Ok(None) | Err(_) => return Ok(None),
    };
    let mut rules = BTreeSet::new();
    budget
        .reserve(0, 1, 64)
        .map_err(|error| operation_error(&error))?;
    rules.insert(source.rule_id);
    let mut possible_people = 0_u64;
    let mut foundational_block = false;
    for person_id in &input.people {
        budget.step().map_err(|error| operation_error(&error))?;
        let pair = AssignmentPair {
            person_id: *person_id,
            shift_id: source.shift_id,
        };
        let evaluations = match evaluate_pair(&input, pair, true, &document.settings, budget) {
            Ok(evaluations) => evaluations,
            Err(
                error @ (super::AssignmentRuleError::Cancelled
                | super::AssignmentRuleError::BudgetExpired
                | super::AssignmentRuleError::LimitExceeded(_)
                | super::AssignmentRuleError::InvalidDocument(
                    DomainPackError::ResourceLimitExceeded,
                )),
            ) => return Err(operation_error(&error)),
            Err(_) => return Ok(None),
        };
        let mut possible = true;
        for evaluation in evaluations {
            budget.step().map_err(|error| operation_error(&error))?;
            if evaluation.satisfied {
                continue;
            }
            possible = false;
            let foundational = evaluation.rule_id == RuleId::from_uuid(person_id.as_uuid())
                || input
                    .availability_by_person
                    .get(person_id)
                    .is_some_and(|availability_ids| {
                        availability_ids.iter().any(|availability_id| {
                            evaluation.rule_id
                                == RuleId::from_uuid(availability_id.as_entity_id().as_uuid())
                        })
                    });
            let active_pair_rule =
                input
                    .domain
                    .rules
                    .get(&evaluation.rule_id)
                    .is_some_and(|rule| {
                        let (id, active, _) = rule.header();
                        id == evaluation.rule_id
                            && active
                            && matches!(
                                rule,
                                WorkforceRule::Eligibility { .. }
                                    | WorkforceRule::Availability { .. }
                            )
                    });
            if foundational {
                foundational_block = true;
                continue;
            }
            if !active_pair_rule {
                return Ok(None);
            }
            budget
                .reserve(0, 1, 64)
                .map_err(|error| operation_error(&error))?;
            rules.insert(evaluation.rule_id);
        }
        if possible {
            possible_people = possible_people
                .checked_add(1)
                .ok_or_else(|| contract("validation_count"))?;
        }
    }
    if source.required_count <= possible_people || rules.len() > MAX_EVIDENCE_MESSAGE_REFERENCES {
        return Ok(None);
    }
    let mut rule_ids = Vec::new();
    for rule_id in rules {
        budget
            .reserve(0, 1, 32)
            .map_err(|error| operation_error(&error))?;
        rule_ids.push(rule_id);
    }
    let rule_count = u64::try_from(rule_ids.len()).map_err(|_| contract("validation_count"))?;
    let output_items = 3_u64
        .checked_add(rule_count)
        .ok_or_else(|| contract("validation_count"))?;
    let output_bytes = 2048_u64
        .checked_add(
            rule_count
                .checked_mul(64)
                .ok_or_else(|| contract("validation_count"))?,
        )
        .ok_or_else(|| contract("validation_count"))?;
    budget
        .reserve(1, output_items, output_bytes)
        .map_err(|error| operation_error(&error))?;
    let foundational_sentence = if foundational_block {
        " Foundational activity or approved-leave predicates also excluded at least one person; no invented RuleId represents those source facts."
    } else {
        ""
    };
    let summary = format!(
        "Independent source evaluation found {possible_people} possible people for this shift after unconditional activity, approved-leave, and active Eligibility/Availability checks; this is below the Required Coverage lower bound of {}. Coverage scope and coverage qualification filters are not applied, so this is a safe upper-bound source count contradiction, not a solver status.{foundational_sentence}",
        source.required_count,
    );
    let mut parameters = BTreeMap::new();
    parameters.insert(fact("summary")?, VerificationValue::Text(summary));
    parameters.insert(
        fact("required_count")?,
        VerificationValue::Integer(
            i64::try_from(source.required_count).map_err(|_| contract("validation_count"))?,
        ),
    );
    parameters.insert(
        fact("possible_people")?,
        VerificationValue::Integer(
            i64::try_from(possible_people).map_err(|_| contract("validation_count"))?,
        ),
    );
    Ok(Some(EvidenceMessageV1 {
        message_key: "official.workforce.validation.coverage_shortage_proof".to_owned(),
        parameters,
        entities: Vec::new(),
        rules: rule_ids,
        assignments: Vec::new(),
        evidence: Vec::new(),
    }))
}

#[allow(clippy::too_many_lines)]
fn render_assignment(
    document: &ScenarioDocument,
    evidence: &AssignmentEvidenceV1,
    objective_context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<EvidenceMessageV1>, DomainPackError> {
    let input = AssignmentInput::new(document, budget)
        .map_err(|error| operation_error(&error))?
        .with_objective_context(objective_context, document.scenario_id, budget)?;
    let objective_baseline =
        super::profile::snapshot_pairs(&input, document.scenario_id, objective_context, budget)?;
    let (_, has_score_policy) = original_obligations_with_policy(&input, budget)?;
    check_objective_support(
        &input,
        document.scenario_id,
        objective_context,
        has_score_policy,
        budget,
    )?;
    let (pair, selected) = decode_workforce_assignment(&evidence.assignment)?;
    validate_pair(&input, pair)?;
    let mut identities = PlanningIdentities::default();
    let source_evidence =
        validate_source_provenance(&evidence.assignment, pair, &mut identities, budget)?;
    let named_profile =
        super::profile::ProfileKind::parse(objective_context, document.scenario_id)?
            .is_some_and(super::profile::ProfileKind::named);
    if (!named_profile && evidence.score_contributions.len() > 1)
        || matches!(
            evidence.lock_state,
            Some(AssignmentLockStateV1::Locked { .. })
        )
    {
        return Err(contract("assignment_context"));
    }
    let evaluations = evaluate_pair(&input, pair, selected, &document.settings, budget)
        .map_err(|error| operation_error(&error))?;
    if evaluations.len() >= MAX_EVIDENCE_MESSAGES {
        return Err(contract("evidence_messages"));
    }
    for supplied in &evidence.related_rules {
        budget.step().map_err(|error| operation_error(&error))?;
        let index = evaluations
            .binary_search_by_key(&supplied.rule_id, |value| value.rule_id)
            .map_err(|_| contract("pair_local_rule"))?;
        if evaluations[index] != *supplied {
            return Err(contract("pair_local_rule"));
        }
    }
    let (violations, checked) = evaluation_counts(&evaluations, budget)?;
    for (id, value) in &evidence.metrics {
        budget.step().map_err(|error| operation_error(&error))?;
        let expected = match id.as_str() {
            SELECTED_METRIC => i64::from(selected),
            CHECKED_METRIC => checked,
            VIOLATIONS_METRIC => violations,
            _ => return Err(contract("pair_local_metric")),
        };
        if value != &MetricValue::Integer(expected) {
            return Err(contract("pair_local_metric"));
        }
    }
    let (rank_contribution, profile_messages) = validate_score_contributions(
        document,
        &input,
        objective_context,
        objective_baseline,
        evidence,
        pair,
        selected,
        &mut identities,
        budget,
    )?;
    budget
        .reserve(1, 12, 2048)
        .map_err(|error| operation_error(&error))?;
    let mut parameters = BTreeMap::from([
        (fact("summary")?, VerificationValue::Text("Recorded assignment decision; only pair-local checks are recomputed. This is not full-schedule, current-revision, historical-execution or optimality proof.".to_owned())),
        (fact("selected")?, VerificationValue::Boolean(selected)),
        (fact("violations")?, VerificationValue::Integer(violations)),
        (fact("checked_predicates")?, VerificationValue::Integer(checked)),
    ]);
    if let Some(rank) = rank_contribution {
        parameters.insert(fact("rank_contribution")?, VerificationValue::Integer(rank));
    }
    let mut messages = vec![EvidenceMessageV1 {
        message_key: "official.workforce.assignment.recorded_decision".to_owned(),
        parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: vec![evidence.assignment.id.clone()],
        evidence: vec![source_evidence],
    }];
    messages.extend(profile_messages);
    for evaluation in evaluations {
        messages.push(render_rule_evaluation(evaluation, budget)?);
    }
    if selected && violations == 0 && !input.domain.preferences.is_empty() {
        append_optional_selected_preferences(
            &input,
            document,
            objective_context,
            pair,
            &evidence.assignment.id,
            &mut messages,
            budget,
        )?;
    }
    Ok(messages)
}
#[allow(clippy::too_many_arguments)]
fn validate_score_contributions(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    objective_context: Option<&RunObjectiveContext>,
    objective_baseline: Option<&[AssignmentPair]>,
    evidence: &AssignmentEvidenceV1,
    pair: AssignmentPair,
    selected: bool,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<(Option<i64>, Vec<EvidenceMessageV1>), DomainPackError> {
    let profile = super::profile::ProfileKind::parse(objective_context, document.scenario_id)?;
    let named_profile = profile.is_some_and(super::profile::ProfileKind::named);
    if evidence.score_contributions.is_empty() {
        if !named_profile {
            return Ok((None, Vec::new()));
        }
        let contributions = derive_profile_contributions(
            document,
            input,
            objective_context,
            objective_baseline,
            pair,
            selected,
            identities,
            budget,
        )?;
        let messages = contributions
            .iter()
            .map(|contribution| render_profile_contribution(contribution, &evidence.assignment.id))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok((None, messages));
    }
    let rank_id = DomainEvidenceId::new(
        identities
            .derive(
                IdentityKind::Provenance,
                &("assignment_rank", pair.person_id, pair.shift_id),
                budget,
            )
            .map_err(|error| operation_error(&error))?,
    )
    .map_err(|_| contract("evidence_identity"))?;
    let mut rank = None;
    let mut non_rank = Vec::new();
    for contribution in &evidence.score_contributions {
        budget.step().map_err(|error| operation_error(&error))?;
        if contribution.evidence_id == rank_id {
            let pairs = if selected {
                std::slice::from_ref(&pair)
            } else {
                &[]
            };
            let expected = original_rank(input, pairs, budget)?;
            if rank.replace(expected).is_some()
                || contribution.level_id.as_str() != RANK_LEVEL
                || contribution
                    .category_id
                    .as_ref()
                    .is_some_and(|id| id.as_str() != RANK_CATEGORY)
                || contribution.value != expected
            {
                return Err(contract("pair_rank_contribution"));
            }
        } else {
            non_rank.push(contribution);
        }
    }
    if non_rank.is_empty() {
        return Ok((rank, Vec::new()));
    }
    if !named_profile {
        return Err(contract("pair_rank_contribution"));
    }
    let expected = derive_profile_contributions(
        document,
        input,
        objective_context,
        objective_baseline,
        pair,
        selected,
        identities,
        budget,
    )?;
    let mut messages = Vec::with_capacity(non_rank.len());
    for contribution in non_rank {
        budget.step().map_err(|error| operation_error(&error))?;
        if !expected
            .iter()
            .any(|expected| profile_contribution_matches(contribution, expected))
        {
            return Err(contract("profile_score_contribution"));
        }
        messages.push(render_profile_contribution(
            contribution,
            &evidence.assignment.id,
        )?);
    }
    Ok((rank, messages))
}

#[allow(clippy::too_many_arguments)]
fn derive_profile_contributions(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    objective_context: Option<&RunObjectiveContext>,
    objective_baseline: Option<&[AssignmentPair]>,
    pair: AssignmentPair,
    selected: bool,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<ScoreContributionV1>, DomainPackError> {
    let plan = preference::prepare(
        document,
        input,
        objective_context,
        PlanningIrLimitsV1::DEFAULT,
        budget,
    )
    .map_err(|error| operation_error(&error))?;
    let analysis = analyze_source_with_budget(document, budget, PlanningIrLimitsV1::DEFAULT)
        .map_err(|error| operation_error(&error))?;
    let candidate = analysis.candidates.binary_search(&pair).is_ok();
    let baseline = if plan.layout.changes(objective_context) {
        Some(super::profile::sort_pairs(
            objective_baseline.ok_or_else(|| contract("baseline_required"))?,
            budget,
        )?)
    } else {
        None
    };
    let mut contributions = Vec::new();
    append_profile_prefix_contributions(
        input,
        &plan.layout,
        baseline.as_ref(),
        candidate,
        pair,
        selected,
        identities,
        budget,
        &mut contributions,
    )?;
    append_direct_preference_contributions(
        document,
        input,
        &plan,
        candidate,
        pair,
        selected,
        identities,
        budget,
        &mut contributions,
    )?;
    contributions.sort_by(|left, right| left.evidence_id.cmp(&right.evidence_id));
    Ok(contributions)
}

#[allow(clippy::too_many_arguments)]
fn append_profile_prefix_contributions(
    input: &AssignmentInput,
    layout: &super::profile::Layout,
    baseline: Option<&super::profile::SortedPairs<'_>>,
    candidate: bool,
    pair: AssignmentPair,
    selected: bool,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
    contributions: &mut Vec<ScoreContributionV1>,
) -> Result<(), DomainPackError> {
    for tier in &layout.prefix {
        match tier {
            super::profile::PrefixTier::PairChange => {
                let baseline = baseline.ok_or_else(|| contract("baseline_required"))?;
                let baseline_member = baseline
                    .contains(pair, budget)
                    .map_err(|error| operation_error(&error))?;
                if candidate || baseline_member {
                    let id = super::profile::pair_change_fact_id(pair, identities, budget)
                        .map_err(|error| operation_error(&error))?;
                    contributions.push(source_score_contribution(
                        id.as_str(),
                        tier.id(),
                        tier.category(),
                        i64::from(selected != baseline_member),
                    )?);
                }
            }
            super::profile::PrefixTier::SoftLocks => {
                for (lock_id, lock) in &input.domain.locked_assignments {
                    budget.step().map_err(|error| operation_error(&error))?;
                    if lock.person_id != pair.person_id || lock.shift_id != pair.shift_id {
                        continue;
                    }
                    let crate::model::LockState::Soft { stability_weight } = lock.state else {
                        continue;
                    };
                    if stability_weight == 0 {
                        continue;
                    }
                    let id = super::profile::soft_lock_fact_id(
                        *lock_id,
                        pair,
                        stability_weight,
                        identities,
                        budget,
                    )
                    .map_err(|error| operation_error(&error))?;
                    contributions.push(source_score_contribution(
                        id.as_str(),
                        tier.id(),
                        tier.category(),
                        if selected {
                            0
                        } else {
                            i64::from(stability_weight)
                        },
                    )?);
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_direct_preference_contributions(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    plan: &preference::PreferencePlan<'_>,
    candidate: bool,
    pair: AssignmentPair,
    selected: bool,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
    contributions: &mut Vec<ScoreContributionV1>,
) -> Result<(), DomainPackError> {
    for entry in &plan.entries {
        budget.step().map_err(|error| operation_error(&error))?;
        if !candidate && !matches!(entry.variant, preference::PreferenceVariant::BaseStability) {
            continue;
        }
        if !preference::matches(input, *entry, pair, &document.settings, budget)
            .map_err(|error| operation_error(&error))?
        {
            continue;
        }
        let ids = preference::PreferenceIds::derive(*entry, pair, identities, budget)
            .map_err(|error| operation_error(&error))?;
        if !candidate && !ids.base_member {
            continue;
        }
        let value = preference::pair_penalty(*entry, selected && candidate, ids.base_member)
            .map_err(|error| operation_error(&error))?;
        let level = plan
            .levels
            .get(entry.level)
            .ok_or_else(|| contract("profile_score_contribution"))?;
        contributions.push(source_score_contribution(
            ids.fact.as_str(),
            &level.id,
            preference::category(entry.variant),
            value,
        )?);
    }
    Ok(())
}

fn source_score_contribution(
    evidence_id: &str,
    level_id: &str,
    category: &str,
    value: i64,
) -> Result<ScoreContributionV1, DomainPackError> {
    Ok(ScoreContributionV1 {
        evidence_id: DomainEvidenceId::new(evidence_id)
            .map_err(|_| contract("evidence_identity"))?,
        level_id: ScoreLevelId::new(level_id).map_err(|_| contract("score_identity"))?,
        category_id: Some(ScoreCategoryId::new(category).map_err(|_| contract("score_identity"))?),
        value,
    })
}

fn profile_contribution_matches(
    contribution: &ScoreContributionV1,
    expected: &ScoreContributionV1,
) -> bool {
    contribution.evidence_id == expected.evidence_id
        && contribution.level_id == expected.level_id
        && contribution
            .category_id
            .as_ref()
            .is_none_or(|category| Some(category) == expected.category_id.as_ref())
        && contribution.value == expected.value
}

fn render_profile_contribution(
    contribution: &ScoreContributionV1,
    assignment: &eutheto_domain_ir::DomainAssignmentId,
) -> Result<EvidenceMessageV1, DomainPackError> {
    let mut parameters = BTreeMap::from([
        (
            fact("objective_level")?,
            VerificationValue::Text(contribution.level_id.as_str().to_owned()),
        ),
        (
            fact("objective_value")?,
            VerificationValue::Integer(contribution.value),
        ),
    ]);
    if let Some(category) = &contribution.category_id {
        parameters.insert(
            fact("objective_category")?,
            VerificationValue::Text(category.as_str().to_owned()),
        );
    }
    Ok(EvidenceMessageV1 {
        message_key: "official.workforce.assignment.objective_contribution".to_owned(),
        parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: vec![assignment.clone()],
        evidence: vec![contribution.evidence_id.clone()],
    })
}

/// Optional preference facts share the decision's finite work and retention allowance.
fn append_optional_selected_preferences(
    input: &AssignmentInput,
    document: &ScenarioDocument,
    objective_context: Option<&RunObjectiveContext>,
    pair: AssignmentPair,
    assignment: &eutheto_domain_ir::DomainAssignmentId,
    messages: &mut Vec<EvidenceMessageV1>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    let extra = render_selected_preferences(
        input,
        document,
        objective_context,
        pair,
        assignment,
        &messages[0].evidence[0],
        budget,
    );
    match extra {
        Ok(mut facts) => {
            let base_len = messages.len();
            messages.append(&mut facts);
            while messages.len() > base_len {
                if messages.len() > MAX_EVIDENCE_MESSAGES {
                    messages.pop();
                    continue;
                }
                match check_rendered_output(messages, budget) {
                    Ok(()) => break,
                    Err(DomainPackError::ResourceLimitExceeded) => {
                        messages.pop();
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        Err(DomainPackError::ResourceLimitExceeded) => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

/// Only eligible source-scoped selected-pair preference terms; no full-schedule score or causality.
#[allow(clippy::too_many_lines)]
fn render_selected_preferences(
    input: &AssignmentInput,
    document: &ScenarioDocument,
    objective_context: Option<&RunObjectiveContext>,
    pair: AssignmentPair,
    assignment: &eutheto_domain_ir::DomainAssignmentId,
    source_evidence: &DomainEvidenceId,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<EvidenceMessageV1>, DomainPackError> {
    let plan = preference::prepare(
        document,
        input,
        objective_context,
        PlanningIrLimitsV1::DEFAULT,
        budget,
    )
    .map_err(|error| operation_error(&error))?;
    let mut messages = Vec::new();
    for entry in &plan.entries {
        budget.step().map_err(|error| operation_error(&error))?;
        let (key, kind, summary) = match entry.preference {
            WorkforcePreference::Time { direction, .. } => {
                directional_preference_fact(*direction, "Time")
            }
            WorkforcePreference::AssignmentType { direction, .. } => {
                directional_preference_fact(*direction, "Assignment type")
            }
            WorkforcePreference::Location { direction, .. } => {
                directional_preference_fact(*direction, "Location")
            }
            WorkforcePreference::Adjacency { direction, .. } => {
                directional_preference_fact(*direction, "Base adjacency")
            }
            WorkforcePreference::RequestedTimeOff { .. } => (
                "official.workforce.assignment.preference.requested_time_off",
                "Requested time off",
                "This selected pair matches a source requested-time-off preference. Requested time off is not approved leave; this does not prove any whole-schedule outcome.",
            ),
            WorkforcePreference::BaseStability { .. } => (
                "official.workforce.assignment.preference.base_stability",
                "Base stability",
                "This selected pair is covered by a source Base stability preference.",
            ),
            _ => continue,
        };
        if !preference::matches(input, *entry, pair, &document.settings, budget)
            .map_err(|error| operation_error(&error))?
        {
            continue;
        }
        if messages.len() == MAX_SELECTED_PREFERENCE_FACTS {
            budget
                .reserve(1, 8, 1024)
                .map_err(|error| operation_error(&error))?;
            messages.push(EvidenceMessageV1 {
                message_key: "official.workforce.assignment.preference.limit".to_owned(),
                parameters: BTreeMap::from([(
                    fact("summary")?,
                    VerificationValue::Text(
                        "Only the first 128 matching active source preference conditions are shown in rule order. Additional matches may exist; this is not a complete preference inventory.".to_owned(),
                    ),
                )]),
                entities: Vec::new(),
                rules: Vec::new(),
                assignments: vec![assignment.clone()],
                evidence: vec![source_evidence.clone()],
            });
            break;
        }
        let base_member = preference::base_contains(*entry, pair, budget)
            .map_err(|error| operation_error(&error))?;
        let selected_penalty = preference::pair_penalty(*entry, true, base_member)
            .map_err(|error| operation_error(&error))?;
        let unselected_penalty = preference::pair_penalty(*entry, false, base_member)
            .map_err(|error| operation_error(&error))?;
        let level = plan
            .levels
            .get(entry.level)
            .ok_or_else(|| contract("preference_level"))?;
        let summary = format!(
            "{summary} At objective level {}, this pair-local term contributes {selected_penalty} penalty units when selected and {unselected_penalty} if unselected. This is not a feasible alternative, full-schedule score difference, or causal proof.",
            level.id
        );
        budget
            .reserve(1, 16, 2560)
            .map_err(|error| operation_error(&error))?;
        messages.push(EvidenceMessageV1 {
            message_key: key.to_owned(),
            parameters: BTreeMap::from([
                (fact("summary")?, VerificationValue::Text(summary)),
                (fact("kind")?, VerificationValue::Text(kind.to_owned())),
                (
                    fact("objective_level")?,
                    VerificationValue::Text(level.id.clone()),
                ),
                (
                    fact("selected_pair_penalty")?,
                    VerificationValue::Integer(selected_penalty),
                ),
                (
                    fact("unselected_pair_penalty")?,
                    VerificationValue::Integer(unselected_penalty),
                ),
            ]),
            entities: Vec::new(),
            rules: vec![entry.id],
            assignments: vec![assignment.clone()],
            evidence: vec![source_evidence.clone()],
        });
    }
    Ok(messages)
}

fn directional_preference_fact(
    direction: PreferenceDirection,
    kind: &'static str,
) -> (&'static str, &'static str, &'static str) {
    match direction {
        PreferenceDirection::Prefer => (
            "official.workforce.assignment.preference.matched_prefer",
            kind,
            "This selected pair matches an active Prefer condition. This is a pair-local fact, not a whole-schedule preference or optimality claim.",
        ),
        PreferenceDirection::Avoid => (
            "official.workforce.assignment.preference.matched_avoid",
            kind,
            "This selected pair matches an active Avoid condition. This is a pair-local fact, not a whole-schedule preference or optimality claim.",
        ),
    }
}

fn render_rule_evaluation(
    evaluation: RuleEvaluation,
    budget: &mut OperationBudget<'_>,
) -> Result<EvidenceMessageV1, DomainPackError> {
    budget.step().map_err(|error| operation_error(&error))?;
    budget
        .reserve(1, 4, 512)
        .map_err(|error| operation_error(&error))?;
    let mut parameters = evaluation.observed;
    parameters.insert(
        fact("summary")?,
        VerificationValue::Text(
            if evaluation.satisfied {
                "No violation of this Required condition was found for this recorded decision."
            } else {
                "This recorded decision violates this Required condition."
            }
            .to_owned(),
        ),
    );
    Ok(EvidenceMessageV1 {
        message_key: evaluation.message_key,
        parameters,
        entities: evaluation.affected_entities,
        rules: vec![evaluation.rule_id],
        assignments: Vec::new(),
        evidence: evaluation.evidence,
    })
}

fn fact(name: &'static str) -> Result<VerificationFactId, DomainPackError> {
    VerificationFactId::new(format!("official.workforce.fact.{name}"))
        .map_err(|_| contract("fact_identity"))
}

const fn severity(value: ValidationSeverity) -> ExplanationValidationSeverity {
    match value {
        ValidationSeverity::Info => ExplanationValidationSeverity::Information,
        ValidationSeverity::Warning => ExplanationValidationSeverity::ReviewSuggested,
        ValidationSeverity::Error => ExplanationValidationSeverity::MustFix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_domain_api::DomainPack;
    use eutheto_domain_ir::{
        AssumptionGroupId, ComparisonEnrichmentV2, ComparisonScoringContext, ConflictGroupV1,
        ConflictMinimality, ConflictShrinkStopReason, ConflictShrinkSummaryV1,
        ConflictUnavailableReason, CounterfactualConditionPayloadV1, CounterfactualConditionV1,
        DomainAssignmentId, EvidenceRenderRequestV2, ExplanationEvidencePayloadV2,
        ExplanationEvidenceV2, OptimalityStatusEvidence, PortableAcceptedResult, RepairCausalityV1,
        RepairEvidenceV2, RunInput, RunManifest, RunPhaseTimingsV1, SolutionComparisonV2,
        prepare_comparison_v2,
    };
    use eutheto_types::{
        BackendId, BackendSelection, DurationMillis, ExplanationMode, IanaTimeZone,
        PreservationPolicy, ReproducibilityMode, RequestId, ResourceLimits, ScenarioSnapshotId,
        SolveMode, SolveOptions, SolveRunId, WorkerThreadPolicy,
    };
    use std::collections::BTreeMap;
    #[test]
    fn incomplete_validation_is_not_rendered_as_a_current_finding()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = crate::test_support::fixture()?;
        let code = "official.workforce.limit.work_steps";
        let issue = ValidationIssueEvidenceV1 {
            issue_id: eutheto_domain_ir::VerificationIssueId::new(code)?,
            severity: ExplanationValidationSeverity::MustFix,
            message_key: code.to_owned(),
            parameters: BTreeMap::new(),
            field_path: None,
            entity: None,
            rule_id: None,
        };
        let mut budget = OperationBudget::evaluation(None);
        budget.steps(super::super::budget::MAX_WORK_STEPS)?;
        assert!(matches!(
            render_validation(&document, &issue, &mut budget),
            Err(DomainPackError::ResourceLimitExceeded)
        ));
        Ok(())
    }

    #[test]
    fn optional_preference_work_preserves_inherited_cancellation_checkpoint()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
        document.domain.preferences.insert(
            crate::test_support::id(33).parse()?,
            serde_json::json!({
                "kind":"location","id":crate::test_support::id(33),"active":true,
                "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
                "direction":"avoid","locationIds":[crate::test_support::id(5)]
            }),
        );
        let token = eutheto_types::CancellationToken::new();
        let control = OperationControl::Cancellation(token.clone());
        let mut budget = OperationBudget::evaluation(Some(&control));
        let input = AssignmentInput::new(&document, &mut budget)?;
        let pair = AssignmentPair {
            person_id: crate::test_support::id(1).parse()?,
            shift_id: crate::test_support::id(8).parse()?,
        };
        let assignment = DomainAssignmentId::new(format!(
            "official.workforce.assignment.{}.{}",
            pair.person_id, pair.shift_id,
        ))?;
        let mut messages = vec![EvidenceMessageV1 {
            message_key: "official.workforce.assignment.recorded_decision".to_owned(),
            parameters: BTreeMap::new(),
            entities: Vec::new(),
            rules: Vec::new(),
            assignments: vec![assignment.clone()],
            evidence: vec![DomainEvidenceId::new("tests.optional.source")?],
        }];
        let original = messages.clone();
        budget.cancel_after_steps(1)?;
        assert_eq!(
            append_optional_selected_preferences(
                &input,
                &document,
                None,
                pair,
                &assignment,
                &mut messages,
                &mut budget,
            ),
            Err(DomainPackError::Cancelled),
        );
        assert!(token.is_cancelled());
        assert_eq!(messages, original);
        Ok(())
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn validation_rendering_requires_exact_native_issue_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document.domain.locked_assignments.clear();
        document
            .domain
            .entities
            .remove(&crate::test_support::id(6).parse()?);
        document
            .domain
            .entities
            .get_mut(&crate::test_support::id(8).parse()?)
            .ok_or("shift")?["coverage"] = serde_json::json!({
            "kind": "exact",
            "count": 3,
            "qualificationMinimums": []
        });
        document.domain.rules.insert(
            crate::test_support::id(22).parse()?,
            serde_json::json!({
                "id": crate::test_support::id(22),
                "kind": "coverage",
                "active": true,
                "strength": "required",
                "scope": {"people": {"kind": "all"}}
            }),
        );
        let report = validate_workforce_full(
            &document,
            &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
        )?;
        let native = report.issues.first().ok_or("expected native finding")?;
        let issue = ValidationIssueEvidenceV1 {
            issue_id: eutheto_domain_ir::validation_issue_identity(native)?,
            severity: severity(native.severity),
            message_key: native.code.clone(),
            parameters: BTreeMap::new(),
            field_path: None,
            entity: None,
            rule_id: None,
        };
        let other = eutheto_types::ValidationIssue {
            message: format!("{} (different native message)", native.message),
            ..native.clone()
        };
        let other_id = eutheto_domain_ir::validation_issue_identity(&other)?;
        assert_ne!(issue.issue_id, other_id);

        let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
            ExplanationEvidencePayloadV2::Validation {
                issue: issue.clone(),
            },
        )?)?;
        let authored = RunObjectiveContext::authored(document.domain_pack.id.clone())?;
        assert!(matches!(
            crate::WorkforcePack.render_evidence(
                &document,
                &request,
                Some(&authored),
                &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
            ),
            Err(DomainPackError::InvalidPayload { path, .. }) if path == "objectiveContext"
        ));
        let current = crate::WorkforcePack.render_evidence(
            &document,
            &request,
            None,
            &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
        )?;
        assert_eq!(
            current
                .messages
                .first()
                .map(|message| message.message_key.as_str()),
            Some(issue.message_key.as_str())
        );
        let mut budget = OperationBudget::evaluation(None);
        let rendered = render_validation(&document, &issue, &mut budget)?;
        let summary = rendered.first().ok_or("missing summary")?;
        assert_eq!(summary.message_key, native.code);
        assert_eq!(
            summary.parameters.get(&fact("summary")?),
            Some(&VerificationValue::Text(native.message.clone()))
        );
        assert!(summary.entities.is_empty());
        assert!(summary.rules.is_empty());
        assert_eq!(rendered.len(), 2);
        let proof = rendered.get(1).ok_or("missing shortage proof")?;
        assert_eq!(
            proof.message_key,
            "official.workforce.validation.coverage_shortage_proof"
        );
        assert_eq!(
            proof.parameters.get(&fact("required_count")?),
            Some(&VerificationValue::Integer(3))
        );
        assert_eq!(
            proof.parameters.get(&fact("possible_people")?),
            Some(&VerificationValue::Integer(1))
        );
        assert_eq!(proof.rules, vec![crate::test_support::id(22).parse()?]);
        assert!(matches!(
            proof.parameters.get(&fact("summary")?),
            Some(VerificationValue::Text(summary)) if summary.contains("not a solver status")
        ));

        let forged = ValidationIssueEvidenceV1 {
            issue_id: other_id,
            ..issue.clone()
        };
        let mut budget = OperationBudget::evaluation(None);
        assert!(render_validation(&document, &forged, &mut budget).is_err());
        let mut tampered_code = issue.clone();
        tampered_code.message_key = "official.workforce.other".to_owned();
        let mut budget = OperationBudget::evaluation(None);
        assert!(render_validation(&document, &tampered_code, &mut budget).is_err());
        let mut tampered_path = issue.clone();
        tampered_path.field_path = Some(vec![fact("tampered")?]);
        let mut budget = OperationBudget::evaluation(None);
        assert!(render_validation(&document, &tampered_path, &mut budget).is_err());
        let mut tampered_parameters = issue.clone();
        tampered_parameters
            .parameters
            .insert(fact("tampered")?, VerificationValue::Boolean(true));
        let mut budget = OperationBudget::evaluation(None);
        assert!(render_validation(&document, &tampered_parameters, &mut budget).is_err());
        let mut warning = native.clone();
        warning.severity = ValidationSeverity::Warning;
        let mut budget = OperationBudget::evaluation(None);
        assert!(render_candidate_shortage_proof(&document, &warning, &mut budget)?.is_none());
        let mut other_finding = native.clone();
        other_finding.code = "official.workforce.other".to_owned();
        let mut budget = OperationBudget::evaluation(None);
        assert!(render_candidate_shortage_proof(&document, &other_finding, &mut budget)?.is_none());
        Ok(())
    }
    #[test]
    fn validation_shortage_maps_the_active_eligibility_rule_that_excludes_a_person()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document.domain.locked_assignments.clear();
        document
            .domain
            .entities
            .remove(&crate::test_support::id(6).parse()?);
        document
            .domain
            .entities
            .get_mut(&crate::test_support::id(8).parse()?)
            .ok_or("shift")?["coverage"]["count"] = serde_json::json!(2);
        let mut second = document
            .domain
            .entities
            .get(&crate::test_support::id(1).parse()?)
            .ok_or("person")?
            .clone();
        second["id"] = serde_json::json!(crate::test_support::id(40));
        second["externalId"] = serde_json::json!("staff-40");
        second["eligibleAssignmentTypeIds"] = serde_json::json!([]);
        document
            .domain
            .entities
            .insert(crate::test_support::id(40).parse()?, second);
        document.domain.rules.insert(
            crate::test_support::id(20).parse()?,
            serde_json::json!({
                "id": crate::test_support::id(20),
                "kind": "eligibility",
                "active": true,
                "strength": "required",
                "scope": {"people": {"kind": "all"}}
            }),
        );
        document.domain.rules.insert(
            crate::test_support::id(22).parse()?,
            serde_json::json!({
                "id": crate::test_support::id(22),
                "kind": "coverage",
                "active": true,
                "strength": "required",
                "scope": {"people": {"kind": "all"}}
            }),
        );
        let report = validate_workforce_full(
            &document,
            &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
        )?;
        let native = report
            .issues
            .iter()
            .find(|item| item.code == "official.workforce.candidate_shortage")
            .ok_or("expected shortage with active eligibility")?;
        let issue = ValidationIssueEvidenceV1 {
            issue_id: eutheto_domain_ir::validation_issue_identity(native)?,
            severity: severity(native.severity),
            message_key: native.code.clone(),
            parameters: BTreeMap::new(),
            field_path: None,
            entity: None,
            rule_id: None,
        };
        let mut budget = OperationBudget::evaluation(None);
        let rendered = render_validation(&document, &issue, &mut budget)?;
        assert_eq!(rendered.len(), 2);
        let proof = rendered.get(1).ok_or("missing eligibility proof")?;
        assert_eq!(
            proof.parameters.get(&fact("possible_people")?),
            Some(&VerificationValue::Integer(1))
        );
        assert_eq!(
            proof.rules,
            vec![
                crate::test_support::id(20).parse()?,
                crate::test_support::id(22).parse()?
            ]
        );

        document
            .domain
            .rules
            .get_mut(&crate::test_support::id(20).parse()?)
            .ok_or("eligibility rule")?["active"] = serde_json::json!(false);
        let corrected = validate_workforce_full(
            &document,
            &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
        )?;
        assert!(
            corrected
                .issues
                .iter()
                .all(|item| item.code != "official.workforce.candidate_shortage")
        );
        Ok(())
    }

    #[test]
    fn validation_renderer_binds_shortage_proof_to_standalone_coverage_owner()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document.domain.locked_assignments.clear();
        document
            .domain
            .entities
            .remove(&crate::test_support::id(6).parse()?);
        document.domain.entities.insert(
            crate::test_support::id(30).parse()?,
            serde_json::json!({
                "kind": "coverageRequirement",
                "id": crate::test_support::id(30),
                "active": true,
                "scope": {"kind": "all"},
                "coverage": {
                    "kind": "exact",
                    "count": 1000,
                    "qualificationMinimums": []
                }
            }),
        );
        document.domain.rules.insert(
            crate::test_support::id(22).parse()?,
            serde_json::json!({
                "id": crate::test_support::id(22),
                "kind": "coverage",
                "active": true,
                "strength": "required",
                "scope": {"people": {"kind": "all"}}
            }),
        );
        let report = validate_workforce_full(
            &document,
            &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
        )?;
        let native = report
            .issues
            .iter()
            .find(|native| native.code == "official.workforce.candidate_shortage")
            .ok_or("expected standalone candidate shortage")?;
        assert_eq!(
            native.field_path,
            Some(format!(
                "domain.entities.{}.coverage.count",
                crate::test_support::id(30)
            ))
        );
        let issue = ValidationIssueEvidenceV1 {
            issue_id: eutheto_domain_ir::validation_issue_identity(native)?,
            severity: severity(native.severity),
            message_key: native.code.clone(),
            parameters: BTreeMap::new(),
            field_path: None,
            entity: None,
            rule_id: None,
        };
        let mut budget = OperationBudget::evaluation(None);
        let rendered = render_validation(&document, &issue, &mut budget)?;
        assert_eq!(rendered.len(), 2);
        let proof = rendered.get(1).ok_or("missing standalone proof")?;
        assert_eq!(
            proof.parameters.get(&fact("required_count")?),
            Some(&VerificationValue::Integer(1000))
        );
        assert_eq!(
            proof.parameters.get(&fact("possible_people")?),
            Some(&VerificationValue::Integer(1))
        );
        assert_eq!(proof.rules, vec![crate::test_support::id(22).parse()?]);
        Ok(())
    }

    #[test]
    fn validation_renderer_keeps_summary_when_two_people_pass_upper_bound()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document.domain.locked_assignments.clear();
        document
            .domain
            .entities
            .remove(&crate::test_support::id(6).parse()?);
        document
            .domain
            .entities
            .get_mut(&crate::test_support::id(8).parse()?)
            .ok_or("shift")?["coverage"] = serde_json::json!({
            "kind": "exact",
            "count": 2,
            "qualificationMinimums": []
        });
        document.domain.entities.insert(
            crate::test_support::id(12).parse()?,
            serde_json::json!({
                "kind": "person",
                "id": crate::test_support::id(12),
                "name": "Sky",
                "externalId": "staff-02",
                "activeRange": {"kind": "always"},
                "qualificationGrants": [
                    {"qualificationId": crate::test_support::id(11)}
                ],
                "eligibleAssignmentTypeIds": [crate::test_support::id(4)],
                "workloadWeight": {"numerator": 1, "denominator": 1},
                "workloadTarget": {
                    "bucketId": crate::test_support::id(3),
                    "calendarId": crate::test_support::id(2),
                    "membership": "reportingDate",
                    "target": 120
                },
                "tags": ["night"],
                "teamIds": []
            }),
        );
        document.domain.rules.insert(
            crate::test_support::id(22).parse()?,
            serde_json::json!({
                "id": crate::test_support::id(22),
                "kind": "coverage",
                "active": true,
                "strength": "required",
                "scope": {
                    "people": {
                        "kind": "selected",
                        "personIds": [crate::test_support::id(1)]
                    }
                }
            }),
        );
        let report = validate_workforce_full(
            &document,
            &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
        )?;
        let native = report
            .issues
            .iter()
            .find(|native| native.code == "official.workforce.candidate_shortage")
            .ok_or("expected scoped candidate shortage")?;
        let issue = ValidationIssueEvidenceV1 {
            issue_id: eutheto_domain_ir::validation_issue_identity(native)?,
            severity: severity(native.severity),
            message_key: native.code.clone(),
            parameters: BTreeMap::new(),
            field_path: None,
            entity: None,
            rule_id: None,
        };
        let mut budget = OperationBudget::evaluation(None);
        let rendered = render_validation(&document, &issue, &mut budget)?;
        assert_eq!(rendered.len(), 1);
        Ok(())
    }

    fn terminal_status_request(
        document: &ScenarioDocument,
    ) -> Result<EvidenceRenderRequestV2, Box<dyn std::error::Error>> {
        let run_id: SolveRunId = crate::test_support::id(201).parse()?;
        let document_hash = blake3_hex(&serde_json::to_vec(document)?);
        let input = RunInput::new(
            run_id,
            crate::test_support::id(202).parse::<RequestId>()?,
            document.scenario_id,
            1,
            crate::test_support::id(203).parse::<ScenarioSnapshotId>()?,
            document_hash,
            "2026-11-01T00:00:00Z".parse()?,
            document.domain_pack.id.clone(),
            document.domain_pack.schema_version,
            1,
            "1.2.3".to_owned(),
            "0.1.0".to_owned(),
            BackendId::new("ortools.cp-sat")?,
            "9.15.6755".to_owned(),
            "0.1.0".to_owned(),
            "0.1.0".to_owned(),
            "9.15.6755".to_owned(),
            1,
            0,
            blake3_hex(b"planning"),
            blake3_hex(b"objective"),
            SolveOptions {
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
            },
            "America/New_York".parse::<IanaTimeZone>()?,
            None,
            None,
        )?;
        let manifest = RunManifest::new(
            run_id,
            input.checksum.clone(),
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::NoSolutionWithinLimit,
            },
            "2026-11-01T00:00:00Z".parse()?,
            "2026-11-01T00:00:01Z".parse()?,
            Some(DurationMillis::new(1_000)?),
            None,
            None,
            RunPhaseTimingsV1::default(),
            Vec::new(),
        )?;
        let evidence =
            ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::OptimalityStatus {
                status: Box::new(OptimalityStatusEvidence {
                    schema_version: Some(2),
                    run_input: input,
                    run_manifest: manifest,
                    result: None,
                }),
            })?;
        let request = EvidenceRenderRequestV2::new(evidence)?;
        Ok(request)
    }

    #[test]
    fn terminal_status_renderer_keeps_no_result_unresolved()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = crate::test_support::fixture()?;
        let request = terminal_status_request(&document)?;
        let objective_context = RunObjectiveContext::authored(document.domain_pack.id.clone())?;
        assert!(matches!(
            crate::WorkforcePack.render_evidence(
                &document,
                &request,
                Some(&objective_context),
                &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
            ),
            Err(DomainPackError::Contract(_))
        ));
        let rendered = crate::WorkforcePack.render_evidence(
            &document,
            &request,
            None,
            &OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
        )?;
        assert_eq!(rendered.messages.len(), 1);
        let message = &rendered.messages[0];
        assert_eq!(
            message.message_key,
            "official.workforce.optimality_status.unresolved"
        );
        assert_eq!(
            message.parameters.get(&fact("status")?),
            Some(&VerificationValue::Text("noSolutionWithinLimit".to_owned()))
        );
        assert!(
            !message
                .parameters
                .contains_key(&fact("accepted_solution_id")?)
        );
        rendered.validate()?;
        Ok(())
    }
    #[test]
    fn infeasibility_render_navigates_only_active_required_rules()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
        document.domain.rules.insert(
            crate::test_support::id(30).parse()?,
            serde_json::json!({
                "id": crate::test_support::id(30),
                "kind": "coverage",
                "active": true,
                "strength": "required",
                "scope": {"people": {"kind": "all"}}
            }),
        );
        let mut source_budget = OperationBudget::evaluation(None);
        let input = AssignmentInput::new(&document, &mut source_budget)?;
        let rule_id = original_obligations(&input, &mut source_budget)?
            .into_iter()
            .find_map(|(id, role)| (role == "required_rule").then_some(id))
            .ok_or("fixture required rule")?;
        let evidence = InfeasibilityEvidenceV1::Conflict {
            groups: vec![ConflictGroupV1 {
                group_id: AssumptionGroupId::new("tests.infeasibility.group")?,
                required_rules: vec![rule_id],
            }],
            minimality: ConflictMinimality::Sufficient,
            shrink: ConflictShrinkSummaryV1 {
                initial_group_count: 1,
                remaining_group_count: 1,
                attempted_trials: 0,
                max_trials: 0,
                stop_reason: ConflictShrinkStopReason::NotAttempted,
            },
        };
        let mut budget = OperationBudget::evaluation(None);
        let messages = render_infeasibility(&document, &evidence, None, &mut budget)?;
        assert_eq!(messages.len(), 3);
        assert_eq!(
            messages[0].parameters.get(&fact("status")?),
            Some(&VerificationValue::Text("sufficient".to_owned()))
        );
        let group = messages.last().ok_or("group message")?;
        assert_eq!(group.rules, vec![rule_id]);
        Ok(())
    }

    #[test]
    fn infeasibility_render_rejects_fact_or_lock_source_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
        let mut source_budget = OperationBudget::evaluation(None);
        let input = AssignmentInput::new(&document, &mut source_budget)?;
        let fact_id = original_obligations(&input, &mut source_budget)?
            .into_iter()
            .find_map(|(id, role)| (role == "person_activity").then_some(id))
            .ok_or("fixture activity fact")?;
        let evidence = InfeasibilityEvidenceV1::Conflict {
            groups: vec![ConflictGroupV1 {
                group_id: AssumptionGroupId::new("tests.infeasibility.fact")?,
                required_rules: vec![fact_id],
            }],
            minimality: ConflictMinimality::Sufficient,
            shrink: ConflictShrinkSummaryV1 {
                initial_group_count: 1,
                remaining_group_count: 1,
                attempted_trials: 0,
                max_trials: 0,
                stop_reason: ConflictShrinkStopReason::NotAttempted,
            },
        };
        let mut budget = OperationBudget::evaluation(None);
        assert!(matches!(
            render_infeasibility(&document, &evidence, None, &mut budget),
            Err(DomainPackError::Contract(_))
        ));
        Ok(())
    }

    #[test]
    fn infeasibility_status_preserves_degraded_reason_and_exact_condition()
    -> Result<(), Box<dyn std::error::Error>> {
        let unavailable = InfeasibilityEvidenceV1::Unavailable {
            reason: ConflictUnavailableReason::Cancelled,
        };
        let InfeasibilityStatus {
            status,
            reason,
            shrink,
            ..
        } = infeasibility_status(&unavailable, 0)?;
        assert_eq!(status, "unavailable");
        assert_eq!(reason, Some(ConflictUnavailableReason::Cancelled));
        assert!(shrink.is_none());
        let minimal = InfeasibilityEvidenceV1::Conflict {
            groups: vec![ConflictGroupV1 {
                group_id: AssumptionGroupId::new("tests.infeasibility.minimal")?,
                required_rules: vec![crate::test_support::id(20).parse()?],
            }],
            minimality: ConflictMinimality::ProvenMinimal,
            shrink: ConflictShrinkSummaryV1 {
                initial_group_count: 1,
                remaining_group_count: 1,
                attempted_trials: 1,
                max_trials: 1,
                stop_reason: ConflictShrinkStopReason::Completed,
            },
        };
        let InfeasibilityStatus { status, reason, .. } = infeasibility_status(&minimal, 1)?;
        assert_eq!(status, "minimal");
        assert!(reason.is_none());
        let foundation = InfeasibilityEvidenceV1::Unavailable {
            reason: ConflictUnavailableReason::FoundationalInfeasibility,
        };
        let InfeasibilityStatus { status, reason, .. } = infeasibility_status(&foundation, 0)?;
        assert_eq!(status, "foundational");
        assert_eq!(
            reason,
            Some(ConflictUnavailableReason::FoundationalInfeasibility)
        );

        let condition = CounterfactualConditionV1::new(
            CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id: DomainAssignmentId::new("tests.assignment.1")?,
                value: AssignmentValue::Boolean(false),
            },
        )?;
        let mut parameters = BTreeMap::new();
        append_condition_parameters(&mut parameters, Some(&condition))?;
        assert_eq!(
            parameters.get(&fact("condition_kind")?),
            Some(&VerificationValue::Text("force".to_owned()))
        );
        assert_eq!(
            parameters.get(&fact("condition_value")?),
            Some(&VerificationValue::Boolean(false))
        );
        assert_eq!(
            parameters.get(&fact("condition_checksum")?),
            Some(&VerificationValue::Text(condition.checksum.clone()))
        );
        Ok(())
    }

    #[test]
    fn infeasibility_render_is_cancellable_and_rejects_oversized_mapped_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = crate::test_support::fixture()?;
        let unavailable = InfeasibilityEvidenceV1::Unavailable {
            reason: ConflictUnavailableReason::BudgetExpired,
        };
        let token = eutheto_types::CancellationToken::new();
        token.cancel();
        let control = OperationControl::Cancellation(token);
        let mut budget = OperationBudget::evaluation(Some(&control));
        assert_eq!(
            render_infeasibility(&document, &unavailable, None, &mut budget),
            Err(DomainPackError::Cancelled)
        );

        let rule_id = crate::test_support::id(20).parse()?;
        let oversized_group_count = u32::try_from(MAX_DIAGNOSTIC_GROUPS + 1)?;
        let mut groups = Vec::new();
        for index in 0..=MAX_DIAGNOSTIC_GROUPS {
            groups.push(ConflictGroupV1 {
                group_id: AssumptionGroupId::new(format!("tests.oversized.{index}"))?,
                required_rules: vec![rule_id],
            });
        }
        let oversized = InfeasibilityEvidenceV1::Conflict {
            groups,
            minimality: ConflictMinimality::Sufficient,
            shrink: ConflictShrinkSummaryV1 {
                initial_group_count: oversized_group_count,
                remaining_group_count: oversized_group_count,
                attempted_trials: 0,
                max_trials: 0,
                stop_reason: ConflictShrinkStopReason::NotAttempted,
            },
        };
        let mut budget = OperationBudget::evaluation(None);
        assert_eq!(
            infeasibility_group_count(&oversized, &mut budget),
            Err(DomainPackError::ResourceLimitExceeded)
        );
        Ok(())
    }

    #[test]
    fn source_captions_are_bounded_at_utf8_byte_limit() -> Result<(), Box<dyn std::error::Error>> {
        let caption = truncate_caption(&"é".repeat(400))?;
        assert!(caption.len() <= MAX_INFEASIBILITY_CAPTION_BYTES);
        assert!(caption.ends_with('…'));
        Ok(())
    }

    fn portable_comparison(
        comparable: bool,
    ) -> Result<SolutionComparisonV2, Box<dyn std::error::Error>> {
        let source = PortableAcceptedResult::from_json(include_bytes!(
            "../../../../../crates/eutheto-domain-ir/tests/fixtures/portable_accepted_v2.json"
        ))?;
        let scoring_contexts = if comparable {
            let producer_id = DomainEvidenceId::new("tests.comparison.renderer")?;
            let semantic_hash = blake3_hex(b"same units, population and weights");
            Some((
                ComparisonScoringContext {
                    source: &source,
                    producer_id: producer_id.clone(),
                    semantic_hash: semantic_hash.clone(),
                },
                ComparisonScoringContext {
                    source: &source,
                    producer_id,
                    semantic_hash,
                },
            ))
        } else {
            None
        };
        Ok(
            prepare_comparison_v2(&source, &source, None)?.finish(ComparisonEnrichmentV2 {
                scoring_contexts,
                ..ComparisonEnrichmentV2::default()
            })?,
        )
    }

    #[test]
    fn repair_comparison_summary_never_claims_unavailable_score_order_or_causality()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = EvidenceRenderRequestV2::new(ExplanationEvidenceV2::new(
            ExplanationEvidencePayloadV2::Repair {
                repair: Box::new(RepairEvidenceV2 {
                    comparison: portable_comparison(false)?,
                    causality: RepairCausalityV1::NotEstablished,
                }),
            },
        )?)?;
        let control = OperationControl::Cancellation(eutheto_types::CancellationToken::new());
        let mut budget = OperationBudget::evaluation(Some(&control));
        let ExplanationEvidencePayloadV2::Repair { repair } = &request.evidence.evidence else {
            return Err("wrong fixture kind".into());
        };
        let message = render_comparison(&repair.comparison, true, &mut budget)?;
        assert_eq!(message.message_key, "official.workforce.explanation.repair");
        assert_eq!(
            message.parameters.get(&fact("score_status")?),
            Some(&VerificationValue::Text("unavailable".to_owned()))
        );
        assert_eq!(
            message.parameters.get(&fact("score_unavailable_reason")?),
            Some(&VerificationValue::Text("policyOrScopeMismatch".to_owned()))
        );
        assert!(!message.parameters.contains_key(&fact("score_ordering")?));
        assert!(
            !message
                .parameters
                .contains_key(&fact("score_levels_compared")?)
        );
        assert!(
            !message
                .parameters
                .contains_key(&fact("score_categories_compared")?)
        );
        assert_eq!(
            message.parameters.get(&fact("repair_causality")?),
            Some(&VerificationValue::Text("notEstablished".to_owned()))
        );
        message.validate()?;
        Ok(())
    }
}
