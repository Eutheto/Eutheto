use eutheto_domain_api::DomainPack;
use eutheto_domain_ir::{AcceptedResult, DomainEvidenceId, RunPhaseTimingsV1, VerificationValue};
use eutheto_planning_ir::PlanningProblem;
use eutheto_solver_api::{BackendCandidate, BackendRuntimeIdentity};
use eutheto_solver_router::{CandidateReview, CandidateReviewer, RouterExecutionRecord};
use eutheto_types::{
    BackendId, DurationMillis, IdGenerator, OperationControl, ParentSolveBudget, ScenarioDocument,
    SolutionId, SolveOptions,
};
use eutheto_verify::{
    AcceptanceDecision, AcceptancePhaseTimings, AcceptanceReviewer, BackendObjectiveReconciliation,
    CorrectnessAlarm, VerificationClock,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

const SOLUTION_ID_FAILURE_CODE: &str = "verification.solution_id_generation_failed";

/// Application-layer bridge from router candidates to independent acceptance verification.
///
/// The router receives only the terminal disposition. The complete accepted result or bounded
/// correctness alarm remains available through [`Self::decision`] for authoritative persistence.
pub struct RouterCandidateReviewer<'a> {
    acceptance: AcceptanceReviewer<'a>,
    id_generator: &'a dyn IdGenerator,
    decision: Option<AcceptanceDecision>,
    before_review: Option<Box<dyn FnMut() + Send + 'a>>,
    control: OperationControl,
}

impl<'a> RouterCandidateReviewer<'a> {
    /// Binds candidate review to one immutable scenario revision and planning problem.
    ///
    /// # Errors
    ///
    /// Returns a bounded correctness alarm when the planning problem or immutable bindings are
    /// invalid.
    pub fn new(
        pack: &'a dyn DomainPack,
        document: &'a ScenarioDocument,
        scenario_revision: u64,
        problem: &'a PlanningProblem,
        clock: &'a dyn VerificationClock,
        id_generator: &'a dyn IdGenerator,
        control: &OperationControl,
    ) -> Result<Self, CorrectnessAlarm> {
        Ok(Self {
            acceptance: AcceptanceReviewer::new(pack, document, scenario_revision, problem, clock)?,
            id_generator,
            decision: None,
            before_review: None,
            control: control.clone(),
        })
    }

    /// Invokes the callback immediately before each independent candidate review.
    #[must_use]
    pub fn with_before_review(mut self, before_review: impl FnMut() + Send + 'a) -> Self {
        self.before_review = Some(Box::new(before_review));
        self
    }

    /// Returns the most recent complete acceptance decision.
    #[must_use]
    pub const fn decision(&self) -> Option<&AcceptanceDecision> {
        self.decision.as_ref()
    }

    /// Transfers the completed decision without cloning an accepted solution or its report.
    #[must_use]
    pub fn into_decision(self) -> Option<AcceptanceDecision> {
        self.decision
    }
}

impl CandidateReviewer for RouterCandidateReviewer<'_> {
    fn review(&mut self, _backend_id: &BackendId, candidate: &BackendCandidate) -> CandidateReview {
        if let Err(reason) = self.control.check() {
            return CandidateReview::Interrupted(reason);
        }
        let Ok(solution_id) = SolutionId::new(self.id_generator) else {
            return CandidateReview::VerificationFailed {
                diagnostic_code: SOLUTION_ID_FAILURE_CODE.to_owned(),
            };
        };
        if let Some(before_review) = &mut self.before_review {
            before_review();
        }
        let decision = self
            .acceptance
            .review(candidate, solution_id, &self.control);
        let disposition = match &decision {
            AcceptanceDecision::Awaiting => CandidateReview::AwaitingIndependentVerification,
            AcceptanceDecision::Accepted {
                objective_reconciliation,
                ..
            } => CandidateReview::Verified {
                objective_matches: *objective_reconciliation
                    == BackendObjectiveReconciliation::Matched,
            },
            AcceptanceDecision::Interrupted { reason, .. } => CandidateReview::Interrupted(*reason),
            AcceptanceDecision::ResourceLimitExceeded { .. } => {
                CandidateReview::ResourceLimitExceeded
            }
            AcceptanceDecision::Quarantined { alarm, .. } => CandidateReview::VerificationFailed {
                diagnostic_code: alarm.diagnostic_code.clone(),
            },
        };
        self.decision = Some(decision);
        disposition
    }
}

/// Adapts the original operation clock and retains any invalid observation across later recovery.
pub(crate) struct ParentVerificationClock<'a> {
    budget: &'a ParentSolveBudget,
    failed: AtomicBool,
}

impl<'a> ParentVerificationClock<'a> {
    pub(crate) fn new(budget: &'a ParentSolveBudget) -> Self {
        Self {
            budget,
            failed: AtomicBool::new(false),
        }
    }

    pub(crate) fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

impl VerificationClock for ParentVerificationClock<'_> {
    fn now_milliseconds(&self) -> DurationMillis {
        let observed = self
            .budget
            .checked_elapsed()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .and_then(|milliseconds| DurationMillis::new(milliseconds).ok());
        if let Some(elapsed) = observed {
            elapsed
        } else {
            self.failed.store(true, Ordering::Relaxed);
            DurationMillis::MAX
        }
    }
}

pub(crate) fn terminal_phase_timings(
    decision: Option<&AcceptanceDecision>,
    compile_elapsed: DurationMillis,
    backend_elapsed: Option<DurationMillis>,
) -> RunPhaseTimingsV1 {
    let acceptance = decision.map(decision_timings).unwrap_or_default();
    RunPhaseTimingsV1 {
        compile_milliseconds: Some(compile_elapsed),
        backend_milliseconds: backend_elapsed,
        projection_milliseconds: decision.map(|_| acceptance.projection_milliseconds),
        structural_validation_milliseconds: decision
            .map(|_| acceptance.structural_validation_milliseconds),
        score_recomputation_milliseconds: decision
            .map(|_| acceptance.score_recomputation_milliseconds),
        required_rule_verification_milliseconds: decision
            .map(|_| acceptance.required_rule_verification_milliseconds),
        evidence_persistence_milliseconds: None,
        optional_explanation_milliseconds: None,
    }
}
fn decision_timings(decision: &AcceptanceDecision) -> AcceptancePhaseTimings {
    match decision {
        AcceptanceDecision::Accepted { timings, .. }
        | AcceptanceDecision::Quarantined { timings, .. }
        | AcceptanceDecision::Interrupted { timings, .. }
        | AcceptanceDecision::ResourceLimitExceeded { timings } => *timings,
        AcceptanceDecision::Awaiting => AcceptancePhaseTimings::default(),
    }
}

pub(crate) fn runtime_evidence_matches(
    record: &RouterExecutionRecord,
    identity: &BackendRuntimeIdentity,
    options: &SolveOptions,
) -> bool {
    let Some(attempt) = record.attempts.last() else {
        return record.invocation_count == 0;
    };
    let Some(outcome) = &attempt.outcome else {
        return false;
    };
    let Some(execution) = &outcome.evidence.execution else {
        return false;
    };
    let reproducibility = &execution.reproducibility;
    attempt.backend_id == *identity.backend_id()
        && attempt.backend_version == identity.backend_version()
        && attempt.adapter_version == identity.adapter_version()
        && reproducibility.backend_version == identity.backend_version()
        && reproducibility.adapter_version == identity.adapter_version()
        && reproducibility.worker_version == identity.worker_version()
        && reproducibility.engine_version == identity.solver_version()
        && reproducibility.protocol_major == identity.protocol_major()
        && reproducibility.protocol_minor == identity.protocol_minor()
        && reproducibility.applied_options == *options
}
pub(crate) fn accepted_evidence(
    accepted: &AcceptedResult,
) -> BTreeMap<DomainEvidenceId, VerificationValue> {
    accepted
        .solution
        .assignments
        .iter()
        .flat_map(|assignment| assignment.evidence.iter())
        .chain(
            accepted
                .verification
                .required_rule_results
                .iter()
                .flat_map(|rule| rule.evidence.iter()),
        )
        .cloned()
        .map(|id| (id, VerificationValue::Boolean(true)))
        .collect()
}
