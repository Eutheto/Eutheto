use crate::TranslatedCpSatModel;
use eutheto_domain_ir::OptimizationDirection;
use eutheto_solver_api::{
    BackendAssumptionCoreEvidence, BackendCandidate, BackendObjectiveEvidence,
    BackendTerminationEvidence, BoundSummary, CandidateSubmission,
};
use eutheto_types::DurationMillis;
use thiserror::Error;

/// Stateful aggregation of parent-measured timing and exact adapter quality evidence.
///
/// The recorder retains only the first-incumbent time, latest exact objective vector,
/// and latest exact representable bound vector. It never stores candidate assignments
/// or worker floating-point timing/objective summaries.
#[derive(Clone, Debug)]
pub struct AdapterEvidenceRecorder {
    remaining_at_dispatch_milliseconds: DurationMillis,
    backend_limit_milliseconds: DurationMillis,
    objective_contracts: Vec<ObjectiveContract>,
    first_incumbent_milliseconds: Option<DurationMillis>,
    last_observed_milliseconds: Option<DurationMillis>,
    latest_objective: Option<BackendObjectiveEvidence>,
    latest_bound_values: Option<Vec<i64>>,
    assumption_core: Option<BackendAssumptionCoreEvidence>,
}

#[derive(Clone, Copy, Debug)]
struct ObjectiveContract {
    direction: OptimizationDirection,
    lower_bound: i64,
    upper_bound: i64,
}

impl AdapterEvidenceRecorder {
    /// Starts one backend invocation evidence record from the translated objective plan and frozen
    /// dispatch budget.
    #[must_use]
    pub fn new(
        translated: &TranslatedCpSatModel,
        remaining_at_dispatch_milliseconds: DurationMillis,
        backend_limit_milliseconds: DurationMillis,
    ) -> Self {
        Self::with_contracts(
            translated
                .objective_plan()
                .levels
                .iter()
                .map(|level| ObjectiveContract {
                    direction: level.direction,
                    lower_bound: level.lower_bound,
                    upper_bound: level.upper_bound,
                })
                .collect(),
            remaining_at_dispatch_milliseconds,
            backend_limit_milliseconds,
        )
    }

    fn with_contracts(
        objective_contracts: Vec<ObjectiveContract>,
        remaining_at_dispatch_milliseconds: DurationMillis,
        backend_limit_milliseconds: DurationMillis,
    ) -> Self {
        Self {
            remaining_at_dispatch_milliseconds,
            backend_limit_milliseconds,
            objective_contracts,
            first_incumbent_milliseconds: None,
            last_observed_milliseconds: None,
            latest_objective: None,
            latest_bound_values: None,
            assumption_core: None,
        }
    }

    /// Records an accepted backend candidate without retaining its assignment.
    ///
    /// # Errors
    ///
    /// Returns an error for non-monotonic parent timing, missing or unexpected objective evidence,
    /// an objective dimension mismatch, or a retained bound that contradicts the candidate.
    pub fn record_candidate(
        &mut self,
        candidate: &BackendCandidate,
    ) -> Result<(), AdapterEvidenceError> {
        self.record_candidate_evidence(
            candidate.objective.as_ref(),
            candidate.observed_after_milliseconds,
        )
    }

    /// Validates and records candidate evidence before the assignment crosses the bounded output
    /// mutation boundary.
    ///
    /// # Errors
    ///
    /// Returns the same evidence errors as [`Self::record_candidate`].
    pub fn record_submission(
        &mut self,
        candidate: &CandidateSubmission,
    ) -> Result<(), AdapterEvidenceError> {
        self.record_candidate_evidence(
            candidate.objective.as_ref(),
            candidate.observed_after_milliseconds,
        )
    }

    fn record_candidate_evidence(
        &mut self,
        objective: Option<&BackendObjectiveEvidence>,
        observed_after_milliseconds: DurationMillis,
    ) -> Result<(), AdapterEvidenceError> {
        let objective_values = match (objective, self.objective_contracts.is_empty()) {
            (None, true) => None,
            (None, false) => return Err(AdapterEvidenceError::MissingObjectiveEvidence),
            (Some(_), true) => return Err(AdapterEvidenceError::UnexpectedObjectiveEvidence),
            (Some(objective), false) => {
                if objective.objective_values.len() != self.objective_contracts.len() {
                    return Err(AdapterEvidenceError::ObjectiveDimensionMismatch);
                }
                if let Some(bound) = &self.latest_bound_values {
                    self.validate_bound_against_objective(bound, &objective.objective_values)?;
                }
                Some(objective.objective_values.clone())
            }
        };
        self.validate_time(observed_after_milliseconds)?;

        self.last_observed_milliseconds = Some(observed_after_milliseconds);
        if self.first_incumbent_milliseconds.is_none() {
            self.first_incumbent_milliseconds = Some(observed_after_milliseconds);
        }
        self.latest_objective =
            objective_values.map(|objective_values| BackendObjectiveEvidence { objective_values });
        Ok(())
    }

    /// Records one strictly improved exact bound without retaining worker-native floats.
    ///
    /// Returns `false` for an exact duplicate so callers do not spend progress capacity on the
    /// terminal frame repeating the last callback bound.
    ///
    /// # Errors
    ///
    /// Returns an error unless the adapter has one objective level and the bound is in range,
    /// directionally consistent with the latest candidate, strictly improves or equals prior
    /// bounds, and has non-decreasing parent timing.
    pub fn record_bound(&mut self, bound: &BoundSummary) -> Result<bool, AdapterEvidenceError> {
        let [contract] = self.objective_contracts.as_slice() else {
            return Err(AdapterEvidenceError::BoundUnavailableForObjectivePlan);
        };
        let [bound_value] = bound.bound_values.as_slice() else {
            return Err(AdapterEvidenceError::ObjectiveDimensionMismatch);
        };
        if !(contract.lower_bound..=contract.upper_bound).contains(bound_value) {
            return Err(AdapterEvidenceError::BoundOutsideDeclaredRange);
        }
        if let Some(previous) = self
            .latest_bound_values
            .as_ref()
            .and_then(|values| values.first())
        {
            if previous == bound_value {
                self.validate_time(bound.observed_after_milliseconds)?;
                self.last_observed_milliseconds = Some(bound.observed_after_milliseconds);
                return Ok(false);
            }
            if !bound_strictly_improves(contract.direction, *previous, *bound_value) {
                return Err(AdapterEvidenceError::BoundRegressed);
            }
        }
        if let Some(objective) = &self.latest_objective {
            self.validate_bound_against_objective(
                &bound.bound_values,
                &objective.objective_values,
            )?;
        }
        self.validate_time(bound.observed_after_milliseconds)?;

        self.last_observed_milliseconds = Some(bound.observed_after_milliseconds);
        self.latest_bound_values = Some(bound.bound_values.clone());
        Ok(true)
    }
    pub(crate) fn record_assumption_core(&mut self, core: BackendAssumptionCoreEvidence) {
        self.assumption_core = Some(core);
    }

    /// Finalizes shared backend termination evidence using parent-measured elapsed time.
    ///
    /// # Errors
    ///
    /// Returns an error when completion predates an already recorded observation.
    pub fn finish(
        self,
        elapsed_milliseconds: DurationMillis,
    ) -> Result<BackendTerminationEvidence, AdapterEvidenceError> {
        if self
            .last_observed_milliseconds
            .is_some_and(|observed| observed > elapsed_milliseconds)
        {
            return Err(AdapterEvidenceError::CompletionBeforeObservation);
        }
        Ok(BackendTerminationEvidence {
            remaining_at_dispatch_milliseconds: self.remaining_at_dispatch_milliseconds,
            backend_limit_milliseconds: self.backend_limit_milliseconds,
            elapsed_milliseconds,
            first_incumbent_milliseconds: self.first_incumbent_milliseconds,
            objective: self.latest_objective,
            best_bound_values: self.latest_bound_values,
            assumption_core: self.assumption_core,
            evidence_refs: Vec::new(),
            execution: None,
        })
    }

    fn validate_bound_against_objective(
        &self,
        bound_values: &[i64],
        objective_values: &[i64],
    ) -> Result<(), AdapterEvidenceError> {
        let [contract] = self.objective_contracts.as_slice() else {
            return Err(AdapterEvidenceError::BoundUnavailableForObjectivePlan);
        };
        let ([bound], [objective]) = (bound_values, objective_values) else {
            return Err(AdapterEvidenceError::ObjectiveDimensionMismatch);
        };
        let consistent = match contract.direction {
            OptimizationDirection::Minimize => bound <= objective,
            OptimizationDirection::Maximize => bound >= objective,
        };
        if consistent {
            Ok(())
        } else {
            Err(AdapterEvidenceError::BoundContradictsCandidate)
        }
    }

    fn validate_time(
        &self,
        observed_after_milliseconds: DurationMillis,
    ) -> Result<(), AdapterEvidenceError> {
        if self
            .last_observed_milliseconds
            .is_some_and(|previous| observed_after_milliseconds < previous)
        {
            Err(AdapterEvidenceError::NonMonotonicObservationTime)
        } else {
            Ok(())
        }
    }
}

const fn bound_strictly_improves(
    direction: OptimizationDirection,
    previous: i64,
    next: i64,
) -> bool {
    match direction {
        OptimizationDirection::Minimize => next > previous,
        OptimizationDirection::Maximize => next < previous,
    }
}

/// Invalid ordering, dimensionality, or semantics in adapter-produced termination evidence.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AdapterEvidenceError {
    #[error("adapter evidence observation time moved backwards")]
    NonMonotonicObservationTime,
    #[error("adapter candidate omitted required exact objective evidence")]
    MissingObjectiveEvidence,
    #[error("adapter candidate supplied objective evidence for a satisfaction model")]
    UnexpectedObjectiveEvidence,
    #[error("adapter objective and bound dimensions differ")]
    ObjectiveDimensionMismatch,
    #[error("an exact scalar bound is unavailable for this objective plan")]
    BoundUnavailableForObjectivePlan,
    #[error("adapter bound lies outside the declared objective range")]
    BoundOutsideDeclaredRange,
    #[error("adapter bound contradicts the latest candidate objective")]
    BoundContradictsCandidate,
    #[error("adapter bound regressed instead of improving")]
    BoundRegressed,
    #[error("adapter completion predates an observed progress record")]
    CompletionBeforeObservation,
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_planning_ir::CandidateValues;
    use std::error::Error;

    fn milliseconds(value: u64) -> Result<DurationMillis, Box<dyn Error>> {
        Ok(DurationMillis::new(value)?)
    }

    fn recorder(
        direction: OptimizationDirection,
    ) -> Result<AdapterEvidenceRecorder, Box<dyn Error>> {
        Ok(AdapterEvidenceRecorder::with_contracts(
            vec![ObjectiveContract {
                direction,
                lower_bound: 0,
                upper_bound: 10,
            }],
            milliseconds(900)?,
            milliseconds(800)?,
        ))
    }

    fn candidate(
        sequence: u32,
        observed_after_milliseconds: u64,
        objective_value: i64,
    ) -> Result<BackendCandidate, Box<dyn Error>> {
        Ok(BackendCandidate {
            sequence,
            values: CandidateValues::default(),
            observed_after_milliseconds: milliseconds(observed_after_milliseconds)?,
            objective: Some(BackendObjectiveEvidence {
                objective_values: vec![objective_value],
            }),
            evidence_refs: Vec::new(),
        })
    }

    #[test]
    fn recorder_retains_first_incumbent_and_latest_exact_quality_only() -> Result<(), Box<dyn Error>>
    {
        let mut recorder = recorder(OptimizationDirection::Minimize)?;
        recorder.record_bound(&BoundSummary {
            observed_after_milliseconds: milliseconds(10)?,
            bound_values: vec![0],
        })?;
        recorder.record_candidate(&candidate(1, 20, 8)?)?;
        assert!(recorder.record_bound(&BoundSummary {
            observed_after_milliseconds: milliseconds(30)?,
            bound_values: vec![3],
        })?);
        assert!(!recorder.record_bound(&BoundSummary {
            observed_after_milliseconds: milliseconds(35)?,
            bound_values: vec![3],
        })?);
        recorder.record_candidate(&candidate(2, 40, 6)?)?;

        assert_eq!(
            recorder.finish(milliseconds(50)?)?,
            BackendTerminationEvidence {
                remaining_at_dispatch_milliseconds: milliseconds(900)?,
                backend_limit_milliseconds: milliseconds(800)?,
                elapsed_milliseconds: milliseconds(50)?,
                first_incumbent_milliseconds: Some(milliseconds(20)?),
                objective: Some(BackendObjectiveEvidence {
                    objective_values: vec![6],
                }),
                best_bound_values: Some(vec![3]),
                assumption_core: None,
                evidence_refs: Vec::new(),
                execution: None,
            }
        );
        Ok(())
    }

    #[test]
    fn recorder_retains_a_bound_without_any_incumbent() -> Result<(), Box<dyn Error>> {
        let mut recorder = recorder(OptimizationDirection::Minimize)?;
        recorder.record_bound(&BoundSummary {
            observed_after_milliseconds: milliseconds(15)?,
            bound_values: vec![4],
        })?;

        let evidence = recorder.finish(milliseconds(20)?)?;
        assert_eq!(evidence.first_incumbent_milliseconds, None);
        assert_eq!(evidence.objective, None);
        assert_eq!(evidence.best_bound_values, Some(vec![4]));
        Ok(())
    }

    #[test]
    fn duplicate_bounds_validate_and_advance_the_observation_floor() -> Result<(), Box<dyn Error>> {
        let mut regressing = recorder(OptimizationDirection::Minimize)?;
        regressing.record_bound(&BoundSummary {
            observed_after_milliseconds: milliseconds(30)?,
            bound_values: vec![3],
        })?;
        assert_eq!(
            regressing.record_bound(&BoundSummary {
                observed_after_milliseconds: milliseconds(29)?,
                bound_values: vec![3],
            }),
            Err(AdapterEvidenceError::NonMonotonicObservationTime)
        );
        assert_eq!(
            regressing.finish(milliseconds(29)?),
            Err(AdapterEvidenceError::CompletionBeforeObservation)
        );

        let mut later = recorder(OptimizationDirection::Minimize)?;
        later.record_bound(&BoundSummary {
            observed_after_milliseconds: milliseconds(30)?,
            bound_values: vec![3],
        })?;
        assert!(!later.record_bound(&BoundSummary {
            observed_after_milliseconds: milliseconds(40)?,
            bound_values: vec![3],
        })?);
        assert_eq!(
            later.finish(milliseconds(39)?),
            Err(AdapterEvidenceError::CompletionBeforeObservation)
        );
        Ok(())
    }

    #[test]
    fn recorder_rejects_contradictory_and_regressing_bounds_without_mutation()
    -> Result<(), Box<dyn Error>> {
        let mut minimize = recorder(OptimizationDirection::Minimize)?;
        minimize.record_candidate(&candidate(1, 20, 5)?)?;
        assert_eq!(
            minimize.record_bound(&BoundSummary {
                observed_after_milliseconds: milliseconds(30)?,
                bound_values: vec![8],
            }),
            Err(AdapterEvidenceError::BoundContradictsCandidate)
        );
        minimize.record_bound(&BoundSummary {
            observed_after_milliseconds: milliseconds(30)?,
            bound_values: vec![3],
        })?;
        assert_eq!(
            minimize.record_bound(&BoundSummary {
                observed_after_milliseconds: milliseconds(40)?,
                bound_values: vec![2],
            }),
            Err(AdapterEvidenceError::BoundRegressed)
        );
        let evidence = minimize.finish(milliseconds(30)?)?;
        assert_eq!(
            evidence.objective,
            Some(BackendObjectiveEvidence {
                objective_values: vec![5],
            })
        );
        assert_eq!(evidence.best_bound_values, Some(vec![3]));

        let mut maximize = recorder(OptimizationDirection::Maximize)?;
        maximize.record_candidate(&candidate(1, 20, 5)?)?;
        assert_eq!(
            maximize.record_bound(&BoundSummary {
                observed_after_milliseconds: milliseconds(30)?,
                bound_values: vec![4],
            }),
            Err(AdapterEvidenceError::BoundContradictsCandidate)
        );
        maximize.record_bound(&BoundSummary {
            observed_after_milliseconds: milliseconds(30)?,
            bound_values: vec![8],
        })?;
        assert_eq!(
            maximize.record_bound(&BoundSummary {
                observed_after_milliseconds: milliseconds(40)?,
                bound_values: vec![9],
            }),
            Err(AdapterEvidenceError::BoundRegressed)
        );
        Ok(())
    }

    #[test]
    fn recorder_rejects_non_monotonic_or_dimensionally_invalid_evidence()
    -> Result<(), Box<dyn Error>> {
        let mut non_monotonic = recorder(OptimizationDirection::Minimize)?;
        non_monotonic.record_candidate(&candidate(1, 20, 1)?)?;
        assert_eq!(
            non_monotonic.record_bound(&BoundSummary {
                observed_after_milliseconds: milliseconds(19)?,
                bound_values: vec![0],
            }),
            Err(AdapterEvidenceError::NonMonotonicObservationTime)
        );

        let mut mismatched = recorder(OptimizationDirection::Minimize)?;
        assert_eq!(
            mismatched.record_bound(&BoundSummary {
                observed_after_milliseconds: milliseconds(10)?,
                bound_values: vec![0, 1],
            }),
            Err(AdapterEvidenceError::ObjectiveDimensionMismatch)
        );

        let mut late = recorder(OptimizationDirection::Minimize)?;
        late.record_candidate(&candidate(1, 20, 1)?)?;
        assert_eq!(
            late.finish(milliseconds(19)?),
            Err(AdapterEvidenceError::CompletionBeforeObservation)
        );
        Ok(())
    }
}
