//! Strict candidate-value projection into normalized domain solutions.

use crate::ids::{BoolVariableId, IntVariableId, IntervalVariableId, ProvenanceId};
use crate::model::{IntervalVariable, PlanningProblem, ProjectionExpression, Variable};
use crate::validation::{PlanningIrLimitsV1, ValidationError, validate};
use eutheto_domain_ir::{
    AssignedInterval, AssignmentValue, DomainAssignment, DomainContractError, DomainEvidenceId,
    NORMALIZED_SOLUTION_SCHEMA_VERSION, NormalizedSolution,
};
use eutheto_types::SolutionId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Typed candidate values returned by an adapter. Interval values are reconstructed from their
/// declared integer components and presence literal; adapters cannot inject domain assignments.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateValues {
    /// Boolean planning assignments.
    pub booleans: BTreeMap<BoolVariableId, bool>,
    /// Integer planning assignments.
    pub integers: BTreeMap<IntVariableId, i64>,
}

/// Projection failure is a compiler/adapter contract error, never user infeasibility.
#[derive(Debug)]
pub enum ProjectionError {
    InvalidProblem(ValidationError),
    UnknownCandidateValue(String),
    OutOfDomain(IntVariableId),
    MissingRequiredValue(String),
    InvalidInterval(IntervalVariableId),
    ArithmeticOverflow,
    InvalidEvidence(ProvenanceId),
    DomainContract(DomainContractError),
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "planning projection error: {self:?}")
    }
}

impl std::error::Error for ProjectionError {}

/// Projects a complete/partial typed candidate according to explicit required/optional rules.
///
/// Unknown candidate IDs and out-of-domain values are always rejected. A missing input to a
/// required projection is rejected; a missing input to an optional projection becomes
/// [`AssignmentValue::Absent`]. A false optional-interval presence literal also becomes absent
/// and leaves its integer components unconstrained. A present interval requires all components,
/// non-negative duration, and checked `start + duration == end`. Every emitted assignment carries
/// the projection provenance ID as a non-authoritative evidence reference.
///
/// # Errors
/// Returns a strict validation, unknown/missing/type/domain, evidence, arithmetic, or interval
/// error.
pub fn project_candidate(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
    solution_id: SolutionId,
    limits: PlanningIrLimitsV1,
) -> Result<NormalizedSolution, ProjectionError> {
    project_candidate_inner(problem, candidate, solution_id, limits, true)
}

/// Projects an assignment through an already admitted immutable planning model.
///
/// The caller must retain the [`PreparedPlanningModel`](crate::PreparedPlanningModel) that
/// authenticated the model hash. This path preserves candidate/domain checks but does not repeat
/// whole-IR validation while reviewing a diagnostic witness.
///
/// # Errors
/// Returns candidate, projection, arithmetic, or output-limit failures.
pub fn project_candidate_admitted(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
    solution_id: SolutionId,
    limits: PlanningIrLimitsV1,
) -> Result<NormalizedSolution, ProjectionError> {
    project_candidate_inner(problem, candidate, solution_id, limits, false)
}

fn project_candidate_inner(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
    solution_id: SolutionId,
    limits: PlanningIrLimitsV1,
    validate_problem: bool,
) -> Result<NormalizedSolution, ProjectionError> {
    if validate_problem {
        validate(problem, limits).map_err(ProjectionError::InvalidProblem)?;
    }
    let bool_ids: BTreeSet<_> = problem
        .variables
        .iter()
        .filter_map(|variable| match variable {
            Variable::Boolean(value) => Some(&value.id),
            Variable::Integer(_) | Variable::Interval(_) => None,
        })
        .collect();
    let int_domains: BTreeMap<_, _> = problem
        .variables
        .iter()
        .filter_map(|variable| match variable {
            Variable::Integer(value) => Some((&value.id, &value.domain)),
            Variable::Boolean(_) | Variable::Interval(_) => None,
        })
        .collect();
    let interval_values: BTreeMap<_, _> = problem
        .variables
        .iter()
        .filter_map(|variable| match variable {
            Variable::Interval(value) => Some((&value.id, value)),
            Variable::Boolean(_) | Variable::Integer(_) => None,
        })
        .collect();
    for id in candidate.booleans.keys() {
        if !bool_ids.contains(id) {
            return Err(ProjectionError::UnknownCandidateValue(id.to_string()));
        }
    }
    for (id, value) in &candidate.integers {
        let Some(domain) = int_domains.get(id) else {
            return Err(ProjectionError::UnknownCandidateValue(id.to_string()));
        };
        if !domain.contains(*value) {
            return Err(ProjectionError::OutOfDomain(id.clone()));
        }
    }

    let mut assignments = Vec::with_capacity(problem.projections.len());
    for projection in &problem.projections {
        let value = evaluate_projection(
            &projection.expression,
            projection.required,
            candidate,
            &interval_values,
        )?;
        let evidence = DomainEvidenceId::new(projection.provenance.as_str())
            .map_err(|_| ProjectionError::InvalidEvidence(projection.provenance.clone()))?;
        assignments.push(DomainAssignment {
            id: projection.assignment_id.clone(),
            entity: projection.entity.clone(),
            value,
            evidence: vec![evidence],
        });
    }
    assignments.sort_by(|left, right| left.id.cmp(&right.id));
    let mut solution = NormalizedSolution {
        schema_version: NORMALIZED_SOLUTION_SCHEMA_VERSION,
        pack_id: problem.metadata.pack_id.clone(),
        scenario_id: problem.metadata.scenario_id,
        scenario_revision: problem.metadata.scenario_revision,
        projection_version: problem.metadata.projection_version,
        solution_id,
        assignments,
    };
    solution
        .canonicalize()
        .map_err(ProjectionError::DomainContract)?;
    Ok(solution)
}

fn evaluate_projection(
    expression: &ProjectionExpression,
    required: bool,
    candidate: &CandidateValues,
    intervals: &BTreeMap<&IntervalVariableId, &IntervalVariable>,
) -> Result<AssignmentValue, ProjectionError> {
    match expression {
        ProjectionExpression::Boolean(id) => candidate
            .booleans
            .get(id)
            .copied()
            .map(AssignmentValue::Boolean)
            .map_or_else(|| missing(required, id.as_str()), Ok),
        ProjectionExpression::Integer(id) => candidate
            .integers
            .get(id)
            .copied()
            .map(AssignmentValue::Integer)
            .map_or_else(|| missing(required, id.as_str()), Ok),
        ProjectionExpression::Linear(expression) => {
            let mut value = expression.constant;
            for term in &expression.terms {
                let Some(variable) = candidate.integers.get(&term.variable) else {
                    return missing(required, term.variable.as_str());
                };
                let product = term
                    .coefficient
                    .checked_mul(*variable)
                    .ok_or(ProjectionError::ArithmeticOverflow)?;
                value = value
                    .checked_add(product)
                    .ok_or(ProjectionError::ArithmeticOverflow)?;
            }
            Ok(AssignmentValue::Integer(value))
        }
        ProjectionExpression::Interval(id) => {
            let interval = intervals
                .get(id)
                .copied()
                .ok_or_else(|| ProjectionError::UnknownCandidateValue(id.to_string()))?;
            if let Some(presence) = &interval.presence {
                let Some(boolean) = candidate.booleans.get(&presence.variable) else {
                    return missing(required, presence.variable.as_str());
                };
                if *boolean != presence.positive {
                    return Ok(AssignmentValue::Absent);
                }
            }
            let Some(start) = candidate.integers.get(&interval.start) else {
                return missing(required, interval.start.as_str());
            };
            let Some(duration) = candidate.integers.get(&interval.duration) else {
                return missing(required, interval.duration.as_str());
            };
            let Some(end) = candidate.integers.get(&interval.end) else {
                return missing(required, interval.end.as_str());
            };
            AssignedInterval::new(*start, *duration, *end)
                .map(AssignmentValue::Interval)
                .map_err(|_| ProjectionError::InvalidInterval(id.clone()))
        }
        ProjectionExpression::Constant(value) => Ok(value.clone()),
    }
}

fn missing(required: bool, id: &str) -> Result<AssignmentValue, ProjectionError> {
    if required {
        Err(ProjectionError::MissingRequiredValue(id.to_owned()))
    } else {
        Ok(AssignmentValue::Absent)
    }
}
