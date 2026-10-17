use crate::{
    CandidateDecodeError, OrToolsDescriptorError, TranslatedCpSatModel, decode_projected_candidate,
    ortools_descriptor,
};
use eutheto_domain_ir::OptimizationDirection;
use eutheto_planning_ir::ObjectiveLevel;
use eutheto_protocol::wire::{Progress, ProgressKind, ProjectedCandidate};
use eutheto_solver_api::{
    BackendCandidate, BackendObjectiveEvidence, BoundSummary, CandidateSubmission,
    IncumbentSummary, SolveProgressEvent,
};
use eutheto_types::DurationMillis;
use thiserror::Error;

/// Emits the exact backend identity after a validated worker `Started` frame.
///
/// # Errors
///
/// Returns an error if the reviewed bundled-worker descriptor is invalid.
pub fn backend_started_progress() -> Result<SolveProgressEvent, OrToolsDescriptorError> {
    Ok(SolveProgressEvent::BackendStarted {
        backend: ortools_descriptor()?.id,
    })
}

/// Decodes one projected worker candidate into bounded backend submission evidence.
///
/// The supplied elapsed time is measured by the parent adapter. Worker floating-point
/// objective and timing summaries are deliberately not accepted as exact values.
///
/// # Errors
///
/// Returns an error when the untrusted projection is incomplete, duplicated, ill-typed,
/// out of domain, or cannot reproduce the exact integer objective levels.
pub fn candidate_submission(
    translated: &TranslatedCpSatModel,
    candidate: &ProjectedCandidate,
    observed_after_milliseconds: DurationMillis,
) -> Result<CandidateSubmission, CandidateDecodeError> {
    let decoded = decode_projected_candidate(translated, candidate)?;
    let objective = (!decoded.objective_values.is_empty()).then_some(BackendObjectiveEvidence {
        objective_values: decoded.objective_values,
    });
    Ok(CandidateSubmission {
        values: decoded.values,
        observed_after_milliseconds,
        objective,
        evidence_refs: Vec::new(),
    })
}

/// Builds an incumbent progress event only after the shared output boundary accepted the candidate.
#[must_use]
pub fn candidate_progress(candidate: &BackendCandidate) -> SolveProgressEvent {
    SolveProgressEvent::IncumbentFound(IncumbentSummary {
        sequence: candidate.sequence,
        observed_after_milliseconds: candidate.observed_after_milliseconds,
        objective: candidate.objective.clone(),
    })
}

/// Converts a protocol-validated worker progress frame when it carries exact representable facts.
///
/// Presolve and generic search frames contain no reduction facts in protocol v1.1, so they produce
/// no event. A scalar CP-SAT bound is emitted only for a single objective level when it maps exactly
/// through the retained scalarization weight and direction into the declared integer bounds.
/// Multi-level scalar bounds cannot truthfully reconstruct a lexicographic vector and are omitted.
///
/// # Errors
///
/// Returns an error when the progress-kind discriminant is unknown or unspecified. All other
/// unrepresentable worker summaries are safely omitted instead of rounded or fabricated.
pub fn bound_progress(
    translated: &TranslatedCpSatModel,
    progress: &Progress,
    observed_after_milliseconds: DurationMillis,
) -> Result<Option<SolveProgressEvent>, OrToolsProgressError> {
    let kind = ProgressKind::try_from(progress.kind)
        .map_err(|_| OrToolsProgressError::InvalidProgressKind(progress.kind))?;
    match kind {
        ProgressKind::Presolve | ProgressKind::Search => Ok(None),
        ProgressKind::BoundImproved => {
            Ok(
                exact_single_level_bound(translated, &progress.best_bound_values).map(|bound| {
                    SolveProgressEvent::BoundImproved(BoundSummary {
                        observed_after_milliseconds,
                        bound_values: vec![bound],
                    })
                }),
            )
        }
        ProgressKind::Unspecified => Err(OrToolsProgressError::InvalidProgressKind(progress.kind)),
    }
}

fn exact_single_level_bound(
    translated: &TranslatedCpSatModel,
    native_bounds: &[f64],
) -> Option<i64> {
    let [level] = translated.objective_plan().levels.as_slice() else {
        return None;
    };
    let [native_bound] = native_bounds else {
        return None;
    };
    let weight = translated.objective_weight(&level.id)?;
    exact_level_bound(level, weight, *native_bound)
}

#[allow(clippy::cast_possible_truncation)]
fn exact_level_bound(level: &ObjectiveLevel, weight: i64, native_bound: f64) -> Option<i64> {
    const MAX_SAFE_F64_INTEGER: f64 = 9_007_199_254_740_991.0;

    if weight <= 0
        || !native_bound.is_finite()
        || native_bound.fract() != 0.0
        || !(-MAX_SAFE_F64_INTEGER..=MAX_SAFE_F64_INTEGER).contains(&native_bound)
    {
        return None;
    }
    // The finite, integral, safe-integer range checks make this cast exact and unambiguous.
    let native_bound = native_bound as i64;
    let signed = match level.direction {
        OptimizationDirection::Minimize => native_bound,
        OptimizationDirection::Maximize => native_bound.checked_neg()?,
    };
    if signed % weight != 0 {
        return None;
    }
    let bound = signed / weight;
    (level.lower_bound..=level.upper_bound)
        .contains(&bound)
        .then_some(bound)
}

/// Invalid worker progress evidence that cannot be classified safely.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum OrToolsProgressError {
    #[error("worker emitted invalid progress kind {0}")]
    InvalidProgressKind(i32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_domain_ir::{OptimizationDirection, ScoreCategoryId};
    use eutheto_planning_ir::{
        Capability, CompilerId, InclusiveRange, IntDomain, IntVariable, IntVariableId,
        LinearExpression, LinearTerm, ObjectiveLevelId, ObjectivePlan, ObjectiveTerm,
        ObjectiveTermId, ObjectiveTermKind, PLANNING_IR_SCHEMA_VERSION, PROJECTION_SCHEMA_VERSION,
        PlanningIrLimitsV1, PlanningMetadata, PlanningProblem, ProvenanceId, ProvenanceRecord,
        ProvenanceSourceKind, Variable,
    };
    use eutheto_types::{PackId, ScenarioId};
    use std::collections::{BTreeMap, BTreeSet};
    use std::error::Error;

    fn level(direction: OptimizationDirection) -> Result<ObjectiveLevel, Box<dyn Error>> {
        let provenance = ProvenanceId::new("progress.provenance")?;
        Ok(ObjectiveLevel {
            id: ObjectiveLevelId::new("progress.level")?,
            direction,
            lower_bound: -10,
            upper_bound: 10,
            terms: vec![ObjectiveTerm {
                id: ObjectiveTermId::new("progress.term")?,
                expression: LinearExpression::new(Vec::new(), 0)?,
                kind: ObjectiveTermKind::Penalty,
                category: ScoreCategoryId::new("progress.category")?,
                provenance: provenance.clone(),
            }],
            provenance,
        })
    }
    fn translated_model() -> Result<TranslatedCpSatModel, Box<dyn Error>> {
        let provenance = ProvenanceId::new("progress.provenance")?;
        let variable = IntVariableId::new("progress.value")?;
        let problem = PlanningProblem {
            schema_version: PLANNING_IR_SCHEMA_VERSION,
            variables: vec![Variable::Integer(IntVariable {
                id: variable.clone(),
                domain: IntDomain::new(vec![InclusiveRange { start: 0, end: 10 }])?,
                provenance: provenance.clone(),
            })],
            constraints: Vec::new(),
            objectives: ObjectivePlan {
                levels: vec![ObjectiveLevel {
                    id: ObjectiveLevelId::new("progress.level")?,
                    direction: OptimizationDirection::Minimize,
                    lower_bound: 0,
                    upper_bound: 10,
                    terms: vec![ObjectiveTerm {
                        id: ObjectiveTermId::new("progress.term")?,
                        expression: LinearExpression::new(
                            vec![LinearTerm {
                                variable,
                                coefficient: 1,
                            }],
                            0,
                        )?,
                        kind: ObjectiveTermKind::Penalty,
                        category: ScoreCategoryId::new("progress.category")?,
                        provenance: provenance.clone(),
                    }],
                    provenance: provenance.clone(),
                }],
            },
            assumptions: Vec::new(),
            projections: Vec::new(),
            provenance: vec![ProvenanceRecord {
                id: provenance,
                source_kind: ProvenanceSourceKind::Fact,
                source_id: "progress.fixture".to_owned(),
                entity_refs: Vec::new(),
                message_key: "progress.fixture".to_owned(),
                parameters: BTreeMap::new(),
                parent: None,
            }],
            metadata: PlanningMetadata {
                pack_id: PackId::new("official.synthetic")?,
                scenario_id: "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse::<ScenarioId>()?,
                scenario_revision: 1,
                projection_version: PROJECTION_SCHEMA_VERSION,
                compiler_id: CompilerId::new("compiler.progress")?,
                compiler_version: "1.0.0".to_owned(),
                compile_metadata: BTreeMap::new(),
                display_text: BTreeMap::new(),
            },
            declared_capabilities: BTreeSet::from([Capability::ObjectivePenalty]),
            split_authorization: None,
        };
        Ok(crate::translate_supported_model(
            &problem,
            PlanningIrLimitsV1::DEFAULT,
        )?)
    }

    #[test]
    fn bound_progress_emits_only_exact_single_level_evidence() -> Result<(), Box<dyn Error>> {
        let translated = translated_model()?;
        let observed_after_milliseconds = DurationMillis::new(23)?;
        let exact = Progress {
            request_id: "request-1".to_owned(),
            kind: ProgressKind::BoundImproved as i32,
            objective_values: Vec::new(),
            best_bound_values: vec![6.0],
            wall_time_seconds: None,
            deterministic_time: None,
        };
        assert_eq!(
            bound_progress(&translated, &exact, observed_after_milliseconds)?,
            Some(SolveProgressEvent::BoundImproved(BoundSummary {
                observed_after_milliseconds,
                bound_values: vec![6],
            }))
        );

        let mut fractional = exact;
        fractional.best_bound_values = vec![6.5];
        assert_eq!(
            bound_progress(&translated, &fractional, observed_after_milliseconds)?,
            None
        );
        let search = Progress {
            kind: ProgressKind::Search as i32,
            ..fractional
        };
        assert_eq!(
            bound_progress(&translated, &search, observed_after_milliseconds)?,
            None
        );
        Ok(())
    }

    #[test]
    fn scalar_bounds_are_emitted_only_when_exact_and_in_range() -> Result<(), Box<dyn Error>> {
        let minimize = level(OptimizationDirection::Minimize)?;
        assert_eq!(exact_level_bound(&minimize, 2, 6.0), Some(3));
        assert_eq!(exact_level_bound(&minimize, 2, 5.0), None);
        assert_eq!(exact_level_bound(&minimize, 2, 22.0), None);
        assert_eq!(exact_level_bound(&minimize, 0, 6.0), None);
        assert_eq!(exact_level_bound(&minimize, 2, 6.5), None);
        assert_eq!(exact_level_bound(&minimize, 2, f64::NAN), None);
        let mut wide = minimize.clone();
        wide.lower_bound = -9_007_199_254_740_992;
        wide.upper_bound = 9_007_199_254_740_992;
        assert_eq!(
            exact_level_bound(&wide, 1, 9_007_199_254_740_991.0),
            Some(9_007_199_254_740_991)
        );
        assert_eq!(exact_level_bound(&wide, 1, 9_007_199_254_740_992.0), None);

        let maximize = level(OptimizationDirection::Maximize)?;
        assert_eq!(exact_level_bound(&maximize, 2, -8.0), Some(4));
        assert_eq!(exact_level_bound(&wide, 1, -9_007_199_254_740_992.0), None);
        Ok(())
    }

    #[test]
    fn accepted_candidate_progress_preserves_sequence_time_and_exact_objective()
    -> Result<(), Box<dyn Error>> {
        let observed_after_milliseconds = DurationMillis::new(17)?;
        let candidate = BackendCandidate {
            sequence: 3,
            values: eutheto_planning_ir::CandidateValues::default(),
            observed_after_milliseconds,
            objective: Some(BackendObjectiveEvidence {
                objective_values: vec![4, 9],
            }),
            evidence_refs: Vec::new(),
        };
        assert_eq!(
            candidate_progress(&candidate),
            SolveProgressEvent::IncumbentFound(IncumbentSummary {
                sequence: 3,
                observed_after_milliseconds,
                objective: candidate.objective,
            })
        );
        Ok(())
    }

    #[test]
    fn started_progress_uses_the_reviewed_backend_identity() -> Result<(), Box<dyn Error>> {
        assert_eq!(
            backend_started_progress()?,
            SolveProgressEvent::BackendStarted {
                backend: ortools_descriptor()?.id,
            }
        );
        Ok(())
    }
}
