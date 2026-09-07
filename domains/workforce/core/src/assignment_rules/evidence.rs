use super::{
    analysis::analyze_with_budget,
    authority::{
        CHECKED_METRIC, SELECTED_METRIC, VIOLATIONS_METRIC, contract, evaluation_counts,
        operation_error, original_obligations, original_rank, validate_pair,
    },
    boundary::{DOMAIN_LIMITS, preflight},
    budget::OperationBudget,
    evaluation::evaluate_pair,
    identity::{IdentityKind, PlanningIdentities, RANK_CATEGORY, RANK_LEVEL},
    input::AssignmentInput,
    projection::decode_workforce_assignment,
    sharing::validate_source_provenance,
};
use eutheto_domain_api::{ContractJsonLimits, DomainPackError, DomainValidationReport};
use eutheto_domain_ir::{
    AssignmentEvidenceV1, AssignmentLockStateV1, DomainEvidenceId, EvidenceMessageV1,
    EvidenceRenderRequestV1, EvidenceRenderResultV1, ExplanationCapability,
    ExplanationEvidencePayloadV1, ExplanationKind, ExplanationValidationSeverity,
    MAX_EVIDENCE_MESSAGES, MetricValue, RuleEvaluation, ValidationIssueEvidenceV1,
    VerificationFactId, VerificationValue,
};
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{ScenarioDocument, ValidationSeverity};
use std::collections::BTreeMap;

/// Renders current code-level validation or rederived pair-local assignment facts.
///
/// This cannot establish historical execution, head revision, full-schedule feasibility or
/// optimality. The host owns document/result association. Caller prose is never rendered.
///
/// # Errors
/// Rejects unsupported kinds, stale/forged local facts, unsupported initial semantics and bounds.
pub fn render_workforce_evidence(
    document: &ScenarioDocument,
    request: &EvidenceRenderRequestV1,
) -> Result<EvidenceRenderResultV1, DomainPackError> {
    if request.kind != request.evidence.kind() {
        return Err(contract("evidence_kind"));
    }
    let unsupported = match request.kind {
        ExplanationKind::Validation | ExplanationKind::Assignment => None,
        ExplanationKind::Infeasibility => Some(ExplanationCapability::Infeasibility),
        ExplanationKind::Counterfactual => Some(ExplanationCapability::Counterfactual),
        ExplanationKind::SolutionDifference => Some(ExplanationCapability::SolutionDifference),
        ExplanationKind::Repair => Some(ExplanationCapability::Repair),
        ExplanationKind::OptimalityStatus => Some(ExplanationCapability::OptimalityStatus),
    };
    if let Some(capability) = unsupported {
        return Err(DomainPackError::UnsupportedExplanationCapability(
            capability,
        ));
    }
    let mut budget = OperationBudget::evaluation(None);
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
    let messages = match &request.evidence.evidence {
        ExplanationEvidencePayloadV1::Validation { issue } => {
            vec![render_validation(document, issue, &mut budget)?]
        }
        ExplanationEvidencePayloadV1::Assignment { assignment } => {
            render_assignment(document, assignment, &mut budget)?
        }
        _ => return Err(contract("evidence_kind")),
    };
    let output = preflight(&messages, &mut budget, DOMAIN_LIMITS)
        .map_err(|error| operation_error(&error))?;
    output
        .reserve_json(&mut budget, 1)
        .map_err(|error| operation_error(&error))?;
    budget
        .reserve(1, 6, 1024)
        .map_err(|error| operation_error(&error))?;
    let result =
        EvidenceRenderResultV1::new(request, messages).map_err(|_| contract("evidence_result"))?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(result)
}

pub(crate) fn validate_workforce_full(document: &ScenarioDocument) -> DomainValidationReport {
    let mut budget = OperationBudget::evaluation(None);
    let result = preflight(document, &mut budget, ContractJsonLimits::DEFAULT)
        .and_then(|measured| measured.reserve_json(&mut budget, 1))
        .and_then(|()| analyze_with_budget(document, &mut budget, PlanningIrLimitsV1::DEFAULT));
    match result {
        Ok(analysis) => analysis.validation,
        Err(error) => DomainValidationReport {
            issues: vec![error.validation_issue()],
        },
    }
}

fn render_validation(
    document: &ScenarioDocument,
    issue: &ValidationIssueEvidenceV1,
    budget: &mut OperationBudget<'_>,
) -> Result<EvidenceMessageV1, DomainPackError> {
    if issue.issue_id.as_str() != issue.message_key
        || !issue.parameters.is_empty()
        || issue.field_path.is_some()
        || issue.entity.is_some()
        || issue.rule_id.is_some()
    {
        return Err(contract("validation_summary"));
    }
    let report = match analyze_with_budget(document, budget, PlanningIrLimitsV1::DEFAULT) {
        Ok(analysis) => analysis.validation,
        Err(error) => DomainValidationReport {
            issues: vec![error.validation_issue()],
        },
    };
    let current = report
        .issues
        .iter()
        .find(|current| {
            current.code == issue.message_key && severity(current.severity) == issue.severity
        })
        .ok_or_else(|| contract("validation_not_current"))?;
    // The ID deliberately identifies a current code/severity summary, not a historical occurrence.
    // Only freshly generated readiness text is used; no submitted path, parameter or prose is copied.
    budget
        .reserve(1, 6, 2048)
        .map_err(|error| operation_error(&error))?;
    let mut parameters = BTreeMap::new();
    parameters.insert(
        fact("summary")?,
        VerificationValue::Text(format!(
            "Current validation finding; not historical occurrence evidence. {}",
            current.message
        )),
    );
    Ok(EvidenceMessageV1 {
        message_key: current.code.clone(),
        parameters,
        entities: Vec::new(),
        rules: Vec::new(),
        assignments: Vec::new(),
        evidence: Vec::new(),
    })
}

fn render_assignment(
    document: &ScenarioDocument,
    evidence: &AssignmentEvidenceV1,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<EvidenceMessageV1>, DomainPackError> {
    let input = AssignmentInput::new(document, budget).map_err(|error| operation_error(&error))?;
    // Also rejects ambiguous original obligation IDs and unsupported initial semantics.
    original_obligations(&input, budget)?;
    let (pair, selected) = decode_workforce_assignment(&evidence.assignment)?;
    validate_pair(&input, pair)?;
    let mut identities = PlanningIdentities::default();
    let source_evidence =
        validate_source_provenance(&evidence.assignment, pair, &mut identities, budget)?;
    if evidence.score_contributions.len() > 1
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
    budget
        .reserve(1, 12, 2048)
        .map_err(|error| operation_error(&error))?;
    let mut parameters = BTreeMap::from([
        (fact("summary")?, VerificationValue::Text("Recorded assignment decision; only pair-local checks are recomputed. This is not full-schedule, current-revision, historical-execution or optimality proof.".to_owned())),
        (fact("selected")?, VerificationValue::Boolean(selected)),
        (fact("violations")?, VerificationValue::Integer(violations)),
        (fact("checked_predicates")?, VerificationValue::Integer(checked)),
    ]);
    if let Some(contribution) = evidence.score_contributions.first() {
        let pairs = if selected {
            std::slice::from_ref(&pair)
        } else {
            &[]
        };
        let rank = original_rank(&input, pairs, budget)?;
        let expected = DomainEvidenceId::new(
            identities
                .derive(
                    IdentityKind::Provenance,
                    &("assignment_rank", pair.person_id, pair.shift_id),
                    budget,
                )
                .map_err(|error| operation_error(&error))?,
        )
        .map_err(|_| contract("evidence_identity"))?;
        if contribution.evidence_id != expected
            || contribution.level_id.as_str() != RANK_LEVEL
            || contribution
                .category_id
                .as_ref()
                .is_some_and(|id| id.as_str() != RANK_CATEGORY)
            || contribution.value != rank
        {
            return Err(contract("pair_rank_contribution"));
        }
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
    for evaluation in evaluations {
        messages.push(render_rule_evaluation(evaluation, budget)?);
    }
    Ok(messages)
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
