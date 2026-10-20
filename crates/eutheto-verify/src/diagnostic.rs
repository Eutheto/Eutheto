//! Unsaved partial witness review against the complete original source.
//! No diagnostic witness can construct an `AcceptedResult` or ordinary acceptance decision.

use super::{
    AcceptancePhaseTimings, AcceptanceReviewer, CorrectnessAlarm, CorrectnessAlarmCategory,
    DiagnosticStructuralFailure, elapsed, interrupted, pack_failure, validate_report_integrity,
    validate_structure_diagnostic,
};
use eutheto_domain_api::DomainPackError;
use eutheto_domain_ir::{
    CounterfactualConditionV1, VerificationContextV1, counterfactual_condition_satisfied,
};
use eutheto_planning_ir::{PlanningPreparationBudget, PlanningPreparationError};
use eutheto_solver_api::BackendCandidate;
use eutheto_types::{OperationControl, OperationInterruption, RuleId, SolutionId};
/// A diagnostic-only decision; never a selectable or persistable solution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticDecision {
    /// Every fact/lock and retained authored rule passed complete source review.
    Verified {
        timings: AcceptancePhaseTimings,
    },
    /// A fully checked original-source witness, never a late feasible deletion proof.
    ///
    /// With a locally retained original Infeasible observation, the driver must quarantine the
    /// contradiction before handling optional stop. No accepted result is constructed.
    OriginalWitness {
        optional_stop: Option<OperationInterruption>,
        timings: AcceptancePhaseTimings,
    },
    Quarantined {
        alarm: CorrectnessAlarm,
        timings: AcceptancePhaseTimings,
    },
    Interrupted {
        reason: OperationInterruption,
        timings: AcceptancePhaseTimings,
    },
    ResourceLimitExceeded {
        timings: AcceptancePhaseTimings,
    },
}

impl AcceptanceReviewer<'_> {
    /// Reviews a diagnostic candidate without objective reconciliation or acceptance authority.
    ///
    /// `retained_rules` must be canonical active genuine Required IDs. Their classification comes
    /// from the original pack evaluator, never compiler assumptions or provenance. All other
    /// original obligations remain mandatory unless the source classifies them as authored rules.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn review_diagnostic(
        &self,
        candidate: &BackendCandidate,
        solution_id: SolutionId,
        retained_rules: &[RuleId],
        condition: Option<&CounterfactualConditionV1>,
        control: &OperationControl,
        budget: &mut PlanningPreparationBudget,
    ) -> DiagnosticDecision {
        let mut timings = AcceptancePhaseTimings::default();
        if let Err(error) = budget.check() {
            return preparation_decision(error, timings);
        }
        if let Some(reason) = interrupted(control, None) {
            return DiagnosticDecision::Interrupted { reason, timings };
        }
        let projection_units = u64::try_from(self.problem.variables.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(self.problem.projections.len()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(candidate.values.booleans.len()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(candidate.values.integers.len()).unwrap_or(u64::MAX));
        let Some(projection_scratch) = projection_units
            .checked_mul(256)
            .and_then(|value| value.checked_add(8192))
        else {
            return DiagnosticDecision::ResourceLimitExceeded { timings };
        };
        if let Err(error) = budget.reserve_work(projection_units.saturating_mul(2)) {
            return preparation_decision(error, timings);
        }
        if let Err(error) = budget.reserve_transient(projection_scratch) {
            return preparation_decision(error, timings);
        }
        let started = self.clock.now_milliseconds();
        let projected = self
            .pack
            .project(self.problem, &candidate.values, solution_id, control);
        let projection_failure =
            pack_failure(&projected, CorrectnessAlarmCategory::ProjectionFailed);
        if let Err(error) = budget.release_transient(projection_scratch) {
            return preparation_decision(error, timings);
        }
        let projected_at = self.clock.now_milliseconds();
        let projection_elapsed = elapsed(started, projected_at);
        if let Some(duration) = projection_elapsed {
            timings.projection_milliseconds = duration;
        }
        if let Some(category) = projection_failure {
            return quarantine(category, timings);
        }
        if projection_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
        }
        if let Some(reason) = interrupted(control, projected.as_ref().err()) {
            return DiagnosticDecision::Interrupted { reason, timings };
        }
        let solution = match projected {
            Ok(solution) => solution,
            Err(DomainPackError::ResourceLimitExceeded) => {
                return DiagnosticDecision::ResourceLimitExceeded { timings };
            }
            Err(_) => return quarantine(CorrectnessAlarmCategory::ProjectionFailed, timings),
        };
        let structural = validate_structure_diagnostic(
            self.problem,
            &self.planning_model_hash,
            &candidate.values,
            solution_id,
            &solution,
            budget,
            control,
        );
        let structural_failure = match &structural {
            Err(DiagnosticStructuralFailure::Invalid) => {
                Some(CorrectnessAlarmCategory::StructuralValidationFailed)
            }
            _ => None,
        };
        let structural_at = self.clock.now_milliseconds();
        let structural_elapsed = elapsed(projected_at, structural_at);
        if let Some(duration) = structural_elapsed {
            timings.structural_validation_milliseconds = duration;
        }
        if let Some(category) = structural_failure {
            return quarantine(category, timings);
        }
        if structural_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
        }
        if let Some(reason) = interrupted(control, None) {
            return DiagnosticDecision::Interrupted { reason, timings };
        }
        let structural = match structural {
            Ok(structural) => structural,
            Err(DiagnosticStructuralFailure::Preparation(error)) => {
                return preparation_decision(error, timings);
            }
            Err(DiagnosticStructuralFailure::Invalid) => {
                return quarantine(
                    CorrectnessAlarmCategory::StructuralValidationFailed,
                    timings,
                );
            }
        };
        let score = self
            .pack
            .score(self.document, &solution, self.objective_context, control);
        let score_failure =
            pack_failure(&score, CorrectnessAlarmCategory::ScoreRecomputationFailed);
        let scored_at = self.clock.now_milliseconds();
        let score_elapsed = elapsed(structural_at, scored_at);
        if let Some(duration) = score_elapsed {
            timings.score_recomputation_milliseconds = duration;
        }
        if let Some(category) = score_failure {
            return quarantine(category, timings);
        }
        if score_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
        }
        if let Some(reason) = interrupted(control, score.as_ref().err()) {
            return DiagnosticDecision::Interrupted { reason, timings };
        }
        let score = match score {
            Ok(score) => score,
            Err(DomainPackError::ResourceLimitExceeded) => {
                return DiagnosticDecision::ResourceLimitExceeded { timings };
            }
            Err(_) => {
                return quarantine(CorrectnessAlarmCategory::ScoreRecomputationFailed, timings);
            }
        };
        let classification =
            self.pack
                .diagnostic_verification_scope(self.document, self.scenario_revision, control);
        let classification_failure = pack_failure(
            &classification,
            CorrectnessAlarmCategory::VerificationScopeFailed,
        );
        if let Some(category) = classification_failure {
            return quarantine(category, timings);
        }
        if let Some(reason) = interrupted(control, classification.as_ref().err()) {
            return DiagnosticDecision::Interrupted { reason, timings };
        }
        let classification = match classification {
            Ok(classification) => classification,
            Err(DomainPackError::ResourceLimitExceeded) => {
                return DiagnosticDecision::ResourceLimitExceeded { timings };
            }
            Err(_) => {
                return quarantine(CorrectnessAlarmCategory::VerificationScopeFailed, timings);
            }
        };
        let authored = classification.relaxable_required_rules();
        if retained_rules.len() > authored.len()
            || !retained_rules.windows(2).all(|pair| pair[0] < pair[1])
            || retained_rules
                .iter()
                .any(|rule| authored.binary_search(rule).is_err())
        {
            return quarantine(CorrectnessAlarmCategory::VerificationScopeFailed, timings);
        }
        let verification_scope = classification.verification_scope();
        let context = VerificationContextV1::new(
            self.document.scenario_id,
            self.scenario_revision,
            self.document_hash.clone(),
            self.planning_model_hash.clone(),
            structural.normalized_solution_hash,
            verification_scope.checksum.clone(),
        );
        let Ok(context) = context else {
            return quarantine(CorrectnessAlarmCategory::ReportBindingFailed, timings);
        };
        let report = self.pack.verify(
            self.document,
            &solution,
            self.objective_context,
            &context,
            &score,
            control,
        );
        let report_failure = pack_failure(
            &report,
            CorrectnessAlarmCategory::RequiredRuleVerificationFailed,
        );
        if let Some(category) = report_failure {
            return quarantine(category, timings);
        }
        if let Some(reason) = interrupted(control, report.as_ref().err()) {
            return DiagnosticDecision::Interrupted { reason, timings };
        }
        let report = match report {
            Ok(report) => report,
            Err(DomainPackError::ResourceLimitExceeded) => {
                return DiagnosticDecision::ResourceLimitExceeded { timings };
            }
            Err(_) => {
                return quarantine(
                    CorrectnessAlarmCategory::RequiredRuleVerificationFailed,
                    timings,
                );
            }
        };
        if let Err(category) =
            validate_report_integrity(verification_scope, &context, &report, &score)
        {
            return quarantine(category, timings);
        }
        if score.validate_current_shape().is_err() {
            return quarantine(CorrectnessAlarmCategory::ScoreIntegrityFailed, timings);
        }
        let all_original_satisfied = report.required_rule_results.iter().all(|row| row.satisfied);
        if report.accepted != (score.feasibility == 0) {
            return quarantine(CorrectnessAlarmCategory::ScoreIntegrityFailed, timings);
        }
        if report.required_rule_results.iter().any(|row| {
            !row.satisfied
                && (authored.binary_search(&row.rule_id).is_err()
                    || retained_rules.binary_search(&row.rule_id).is_ok())
        }) || condition.is_some_and(|question| {
            question.validate().is_err() || !counterfactual_condition_satisfied(question, &solution)
        }) {
            return quarantine(CorrectnessAlarmCategory::RequiredRuleRejected, timings);
        }
        let verified_at = self.clock.now_milliseconds();
        let verification_elapsed = elapsed(scored_at, verified_at);
        if let Some(duration) = verification_elapsed {
            timings.required_rule_verification_milliseconds = duration;
        }
        let optional_stop = interrupted(control, None);
        if all_original_satisfied {
            return DiagnosticDecision::OriginalWitness {
                optional_stop,
                timings,
            };
        }
        if verification_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
        }
        if let Some(reason) = optional_stop {
            return DiagnosticDecision::Interrupted { reason, timings };
        }
        // An incomplete or late partial review cannot establish a retained group's necessity.
        DiagnosticDecision::Verified { timings }
    }
}

fn preparation_decision(
    error: PlanningPreparationError,
    timings: AcceptancePhaseTimings,
) -> DiagnosticDecision {
    match error {
        PlanningPreparationError::Cancelled => DiagnosticDecision::Interrupted {
            reason: OperationInterruption::Cancelled,
            timings,
        },
        PlanningPreparationError::DeadlineExceeded => DiagnosticDecision::Interrupted {
            reason: OperationInterruption::DeadlineExceeded,
            timings,
        },
        PlanningPreparationError::WorkLimitExceeded
        | PlanningPreparationError::TransientLimitExceeded
        | PlanningPreparationError::SerializedLimitExceeded
        | PlanningPreparationError::ArithmeticOverflow
        | PlanningPreparationError::Serialization => {
            DiagnosticDecision::ResourceLimitExceeded { timings }
        }
    }
}

fn quarantine(
    category: CorrectnessAlarmCategory,
    timings: AcceptancePhaseTimings,
) -> DiagnosticDecision {
    DiagnosticDecision::Quarantined {
        alarm: CorrectnessAlarm::new(category),
        timings,
    }
}
