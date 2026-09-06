//! Bounded, unregistered Workforce projection; not original-scenario verification.

use super::budget::{OperationBudget, add, count, effective_limits, within};
use super::compiler_rank::{ASSIGNMENT_KIND, WORKFORCE_PROJECTION_VERSION};
use super::ir_cost::precharge_validation;
use super::{AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleLimit};
use crate::generated_workforce_pack_contract::WORKFORCE_PACK_ID;
use crate::{ids::ShiftId, model::AssignmentPair};
use eutheto_domain_api::DomainPackError;
use eutheto_domain_ir::{AssignmentValue, DomainAssignment, DomainEvidenceId, NormalizedSolution};
use eutheto_planning_ir::{
    CandidateValues, PlanningIrLimitsV1, PlanningProblem, ProjectionError, ProjectionExpression,
    Variable, project_candidate,
};
use eutheto_types::{PersonId, SolutionId};
use std::mem::size_of;

/// Projects a candidate under Workforce's V1 typed-pair assignment contract.
///
/// Both selected and unselected required decisions must be supplied. The generic projector
/// remains the model/domain validation authority; decoding checks every returned assignment,
/// not whether its pair belongs to an immutable original scenario. This tokenless API applies
/// finite cumulative resource limits, but does not claim interruptible cancellation.
///
/// # Errors
/// Returns bounded projection contract codes or the existing Workforce budget error conversion.
/// No partial solution is published on any failure.
pub fn project_workforce_candidate(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
    solution_id: SolutionId,
    limits: PlanningIrLimitsV1,
) -> Result<NormalizedSolution, DomainPackError> {
    if problem.metadata.pack_id.as_str() != WORKFORCE_PACK_ID {
        return Err(contract("official.workforce.projection.pack"));
    }
    if problem.metadata.projection_version != WORKFORCE_PROJECTION_VERSION {
        return Err(contract("official.workforce.projection.version"));
    }
    let limits = effective_limits(limits)?;
    let mut budget = OperationBudget::analysis(None, limits);
    let raw_text_bytes = precharge_text(problem, limits, &mut budget)?;
    // Generic validate does not enforce whole-model bytes. Measure the borrowed model before
    // its indexes, candidate processing, or output allocations; never serialize into a Vec.
    let bytes = budget.measure_ir(problem)?;
    budget.steps(bytes.checked_sub(raw_text_bytes).ok_or(
        AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow),
    )?)?;
    precharge_validation(problem, &mut budget)?;
    precharge_projection(problem, candidate, &mut budget)?;
    budget.check()?;
    let solution = project_candidate(problem, candidate, solution_id, limits)
        .map_err(|error| projection_error(&error))?;
    budget.check()?;
    for assignment in &solution.assignments {
        decode_workforce_assignment(assignment)?;
    }
    Ok(solution)
}

// serde_json scans an entire string for escapes before the first fragment reaches the
// counting writer. Bound every public raw String/map key by borrowed lengths first.
// Private typed IDs are constructor-bounded, so their per-fragment scans are already finite.
fn precharge_text(
    problem: &PlanningProblem,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<u64, AssignmentRuleError> {
    use eutheto_planning_ir::ProvenanceParameter;
    let mut bytes = 0;
    let mut charge = |text: &str| {
        bytes = add(bytes, count(text.len())?)?;
        within(bytes, limits.max_ir_bytes, AssignmentRuleLimit::Bytes)?;
        budget.steps(add(1, count(text.len())?)?)
    };
    charge(&problem.metadata.compiler_version)?;
    for record in &problem.provenance {
        charge(&record.source_id)?;
        charge(&record.message_key)?;
        for (key, value) in &record.parameters {
            charge(key)?;
            if let ProvenanceParameter::Text(text) = value {
                charge(text)?;
            }
        }
    }
    for (key, value) in &problem.metadata.compile_metadata {
        charge(key.as_str())?;
        if let ProvenanceParameter::Text(text) = value {
            charge(text)?;
        }
    }
    for (key, value) in &problem.metadata.display_text {
        charge(key)?;
        charge(value)?;
    }
    if let Some(authorization) = &problem.split_authorization {
        charge(&authorization.component_hash)?;
        charge(&authorization.domain_merge_contract)?;
    }
    Ok(bytes)
}

/// Decodes the canonical V1 Workforce pair identity and its Boolean selection.
///
/// Success is allocation-free. This validates the encoding only: it does not establish that
/// the person/shift exists in a source scenario, or that selecting it satisfies any rule.
///
/// # Errors
/// Rejects non-Workforce entity kinds, non-Boolean values (including absence), malformed or
/// noncanonical typed `UUIDv7` pairs, and disagreement between assignment and entity identity.
pub fn decode_workforce_assignment(
    assignment: &DomainAssignment,
) -> Result<(AssignmentPair, bool), DomainPackError> {
    if assignment.entity.kind.as_str() != ASSIGNMENT_KIND {
        return Err(contract("official.workforce.projection.entity_kind"));
    }
    let AssignmentValue::Boolean(selected) = assignment.value else {
        return Err(contract("official.workforce.projection.value_kind"));
    };
    let pair_text = assignment
        .id
        .as_str()
        .strip_prefix(ASSIGNMENT_KIND)
        .and_then(|suffix| suffix.strip_prefix('.'))
        .ok_or_else(|| contract("official.workforce.projection.assignment_id"))?;
    if pair_text != assignment.entity.id.as_str() {
        return Err(contract("official.workforce.projection.entity_mismatch"));
    }
    let (person, shift) = pair_text
        .split_once('.')
        .ok_or_else(|| contract("official.workforce.projection.pair"))?;
    if !canonical_uuid(person) || !canonical_uuid(shift) {
        return Err(contract("official.workforce.projection.pair"));
    }
    // Host and Workforce typed parsers enforce UUIDv7. The preceding spelling check prevents
    // the permissive UUID parser from accepting uppercase, compact, braced, or URN forms.
    let person_id: PersonId = person
        .parse()
        .map_err(|_| contract("official.workforce.projection.pair"))?;
    let shift_id: ShiftId = shift
        .parse()
        .map_err(|_| contract("official.workforce.projection.pair"))?;
    Ok((
        AssignmentPair {
            person_id,
            shift_id,
        },
        selected,
    ))
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

fn contract(code: &'static str) -> DomainPackError {
    DomainPackError::Contract(code.to_owned())
}

fn projection_error(error: &ProjectionError) -> DomainPackError {
    contract(match error {
        ProjectionError::InvalidProblem(_) => "official.workforce.projection.invalid_problem",
        ProjectionError::UnknownCandidateValue(_) => {
            "official.workforce.projection.unknown_candidate"
        }
        ProjectionError::OutOfDomain(_) => "official.workforce.projection.out_of_domain",
        ProjectionError::MissingRequiredValue(_) => "official.workforce.projection.missing_value",
        ProjectionError::InvalidInterval(_) => "official.workforce.projection.invalid_interval",
        ProjectionError::ArithmeticOverflow => "official.workforce.projection.arithmetic_overflow",
        ProjectionError::InvalidEvidence(_) => "official.workforce.projection.invalid_evidence",
        ProjectionError::DomainContract(_) => "official.workforce.projection.domain_contract",
    })
}

fn mul(left: u64, right: u64) -> Result<u64, AssignmentRuleError> {
    left.checked_mul(right)
        .ok_or(AssignmentRuleError::InvalidConstruction(
            AssignmentConstructionIssue::ArithmeticOverflow,
        ))
}

// Reuse the same pinned-standard-library comparison bound as generic IR precharges.
fn tree_work(
    budget: &mut OperationBudget<'_>,
    operations: u64,
    length: u64,
) -> Result<(), AssignmentRuleError> {
    super::ir_cost::tree_work(budget, operations, length, 1)
}

/// Only cost accounting. Generic projection handles unknown IDs, domains, missing values,
/// all expression variants and normalized-solution invariants after the complete precharge.
fn precharge_projection(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let variables = count(problem.variables.len())?;
    let booleans = count(candidate.booleans.len())?;
    let integers = count(candidate.integers.len())?;
    let candidates = add(booleans, integers)?;
    let projections = count(problem.projections.len())?;
    let word = count(size_of::<usize>())?;
    let assignment_bytes = count(size_of::<DomainAssignment>())?;
    let evidence_bytes = count(size_of::<DomainEvidenceId>())?;

    // Three variable scans, one retained borrowed index entry per variable (a set key or
    // key/value pair plus four logical tree links), then candidate traversal and lookups.
    budget.steps(add(mul(3, variables)?, candidates)?)?;
    budget.reserve(0, variables, mul(variables, mul(6, word)?)?)?;
    tree_work(budget, add(variables, candidates)?, variables)?;
    let mut max_ranges = 0;
    for variable in &problem.variables {
        budget.step()?;
        if let Variable::Integer(integer) = variable {
            max_ranges = max_ranges.max(count(integer.domain.inclusive_ranges.len())?);
        }
    }
    // IntDomain::contains scans ranges. This borrowed upper bound also covers malformed
    // models; no second candidate/domain index or semantic validator is built here.
    budget.steps(mul(integers, max_ranges)?)?;

    // Result shell and its cloned pack ID, bounded error payloads, and one allocation-free
    // decoder's typed-pair workspace. These remain charged even if an earlier phase fails.
    budget.reserve(
        1,
        2,
        add(
            add(
                count(size_of::<NormalizedSolution>())?,
                count(problem.metadata.pack_id.as_str().len())?,
            )?,
            add(512, count(size_of::<AssignmentPair>())?)?,
        )?,
    )?;
    budget.reserve(
        projections,
        projections,
        mul(projections, assignment_bytes)?,
    )?;
    // Generic projection sorts once, then NormalizedSolution::canonicalize sorts again.
    // Stable sorts may allocate a moved-element buffer, not cloned string contents.
    for _ in 0..2 {
        budget.sort_work(problem.projections.len())?;
        budget.reserve(0, projections, mul(projections, assignment_bytes)?)?;
    }
    budget.steps(mul(4, projections)?)?; // duplicate/order checks and normalized validation
    for projection in &problem.projections {
        budget.step()?;
        let text_bytes = add(
            add(
                count(projection.assignment_id.as_str().len())?,
                count(projection.entity.kind.as_str().len())?,
            )?,
            add(
                count(projection.entity.id.as_str().len())?,
                count(projection.provenance.as_str().len())?,
            )?,
        )?;
        // Cloned assignment/entity strings plus the one owned evidence ID and its sort
        // workspace. Evidence construction, text copying, and UUID decoding are bounded.
        budget.reserve(0, 2, add(text_bytes, mul(2, evidence_bytes)?)?)?;
        budget.steps(mul(4, text_bytes)?)?;
        budget.sort_work(1)?;
        match &projection.expression {
            ProjectionExpression::Boolean(_) => tree_work(budget, 1, booleans)?,
            ProjectionExpression::Integer(_) => tree_work(budget, 1, integers)?,
            ProjectionExpression::Linear(expression) => {
                let terms = count(expression.terms.len())?;
                budget.steps(mul(3, terms)?)?;
                tree_work(budget, terms, integers)?;
            }
            ProjectionExpression::Interval(_) => {
                tree_work(budget, 1, variables)?;
                tree_work(budget, 1, booleans)?;
                tree_work(budget, 3, integers)?;
                budget.steps(8)?;
            }
            ProjectionExpression::Constant(_) => budget.step()?,
        }
    }
    budget.check()
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
