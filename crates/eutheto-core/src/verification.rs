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
    CorrectnessAlarm, CorrectnessAlarmCategory, VerificationClock,
};
use std::cmp::Ordering as ScoreOrdering;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;
const SOLUTION_ID_FAILURE_CODE: &str = "verification.solution_id_generation_failed";
type FirstAcceptedCallback<'a> =
    dyn FnMut(&AcceptedResult, DurationMillis, DurationMillis) + Send + 'a;
type AfterReviewCallback<'a> = dyn FnMut(&AcceptanceDecision) + Send + 'a;

/// Application-layer bridge from router candidates to independent acceptance verification.
///
/// The router receives only the terminal disposition. The complete accepted result or bounded
/// correctness alarm remains available through [`Self::decision`] for authoritative persistence.
pub struct RouterCandidateReviewer<'a> {
    acceptance: AcceptanceReviewer<'a>,
    clock: &'a dyn VerificationClock,
    id_generator: &'a dyn IdGenerator,
    decision: Option<AcceptanceDecision>,
    established_alarm: Option<CorrectnessAlarm>,
    review_timings: Result<Option<AcceptancePhaseTimings>, ()>,
    before_review: Option<Box<dyn FnMut() + Send + 'a>>,
    after_review: Option<Box<AfterReviewCallback<'a>>>,
    on_first_accepted: Option<Box<FirstAcceptedCallback<'a>>>,
    first_accepted_sent: bool,
    control: OperationControl,
}

impl<'a> RouterCandidateReviewer<'a> {
    /// Binds candidate review to one immutable scenario revision and planning problem.
    ///
    /// # Errors
    ///
    /// Returns a bounded correctness alarm when the planning problem or immutable bindings are
    /// invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pack: &'a dyn DomainPack,
        document: &'a ScenarioDocument,
        scenario_revision: u64,
        problem: &'a PlanningProblem,
        objective_context: Option<&'a eutheto_domain_ir::RunObjectiveContext>,
        clock: &'a dyn VerificationClock,
        id_generator: &'a dyn IdGenerator,
        control: &OperationControl,
    ) -> Result<Self, CorrectnessAlarm> {
        Ok(Self {
            acceptance: AcceptanceReviewer::new(
                pack,
                document,
                scenario_revision,
                problem,
                objective_context,
                clock,
            )?,
            clock,
            id_generator,
            decision: None,
            established_alarm: None,
            review_timings: Ok(None),
            before_review: None,
            after_review: None,
            on_first_accepted: None,
            first_accepted_sent: false,
            control: control.clone(),
        })
    }

    /// Returns the first trusted non-clock correctness alarm independently of later accounting.
    #[must_use]
    pub(crate) fn established_alarm(&self) -> Option<&CorrectnessAlarm> {
        self.established_alarm.as_ref()
    }

    /// Invokes the callback immediately before each independent candidate review.
    #[must_use]
    pub fn with_before_review(mut self, before_review: impl FnMut() + Send + 'a) -> Self {
        self.before_review = Some(Box::new(before_review));
        self
    }

    /// Observes independent decisions and later integrity alarms before control returns to the router.
    #[must_use]
    pub(crate) fn with_after_review(
        mut self,
        after_review: impl FnMut(&AcceptanceDecision) + Send + 'a,
    ) -> Self {
        self.after_review = Some(Box::new(after_review));
        self
    }

    /// Notifies the owner once after the first complete independent acceptance. The callback
    /// must only hand the result to bounded asynchronous custody; it must not perform I/O.
    #[must_use]
    pub fn with_first_accepted(
        mut self,
        on_first_accepted: impl FnMut(&AcceptedResult, DurationMillis, DurationMillis) + Send + 'a,
    ) -> Self {
        self.on_first_accepted = Some(Box::new(on_first_accepted));
        self
    }

    /// Returns the best independently accepted decision, or the last terminal disposition.
    #[must_use]
    pub const fn decision(&self) -> Option<&AcceptanceDecision> {
        self.decision.as_ref()
    }

    /// Transfers the completed decision without cloning an accepted solution or its report.
    #[must_use]
    pub fn into_decision(self) -> Option<AcceptanceDecision> {
        self.decision
    }

    /// Total independent-review work across every candidate, including earlier accepted
    /// incumbents that were replaced by a better terminal candidate.
    pub(crate) fn total_review_timings(&self) -> Result<Option<AcceptancePhaseTimings>, ()> {
        self.review_timings
    }

    fn reconcile_accepted_decision(
        &mut self,
        decision: &AcceptanceDecision,
    ) -> Option<CandidateReview> {
        if let (
            Some(AcceptanceDecision::Accepted {
                result: earlier, ..
            }),
            AcceptanceDecision::Accepted { result: later, .. },
        ) = (&self.decision, decision)
        {
            match later
                .verification
                .score
                .compare(&earlier.verification.score)
            {
                Ok(ScoreOrdering::Greater) => {
                    return Some(CandidateReview::AwaitingIndependentVerification);
                }
                Err(_) => {
                    let category = CorrectnessAlarmCategory::ScoreIntegrityFailed;
                    let diagnostic_code = category.code().to_owned();
                    let alarm = CorrectnessAlarm {
                        category,
                        diagnostic_code: diagnostic_code.clone(),
                    };
                    self.established_alarm = Some(alarm.clone());
                    self.decision = Some(AcceptanceDecision::Quarantined {
                        alarm,
                        timings: decision_timings(decision),
                    });
                    if let (Some(after_review), Some(decision)) =
                        (&mut self.after_review, &self.decision)
                    {
                        after_review(decision);
                    }
                    return Some(CandidateReview::VerificationFailed { diagnostic_code });
                }
                Ok(ScoreOrdering::Less | ScoreOrdering::Equal) => {}
            }
        }
        None
    }
}

impl CandidateReviewer for RouterCandidateReviewer<'_> {
    fn review(&mut self, _backend_id: &BackendId, candidate: &BackendCandidate) -> CandidateReview {
        if let Some(alarm) = &self.established_alarm {
            return CandidateReview::VerificationFailed {
                diagnostic_code: alarm.diagnostic_code.clone(),
            };
        }
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
        let observed = (!self.first_accepted_sent && self.on_first_accepted.is_some())
            .then(|| self.clock.now_milliseconds());
        let decision = self
            .acceptance
            .review(candidate, solution_id, &self.control);
        if let AcceptanceDecision::Quarantined { alarm, .. } = &decision
            && alarm.category != CorrectnessAlarmCategory::ClockFailed
        {
            self.established_alarm.get_or_insert_with(|| alarm.clone());
        }
        if let Some(after_review) = &mut self.after_review {
            after_review(&decision);
        }
        self.review_timings = self.review_timings.and_then(|total| {
            let current = decision_timings(&decision);
            total
                .map_or(Some(current), |previous| checked_sum(previous, current))
                .map(Some)
                .ok_or(())
        });
        if self.review_timings.is_err() {
            return CandidateReview::VerificationFailed {
                diagnostic_code: "verification.phase_timing_overflow".to_owned(),
            };
        }
        let verified = observed.map(|_| self.clock.now_milliseconds());
        if let AcceptanceDecision::Accepted { result, .. } = &decision
            && let (Some(observed), Some(verified), Some(callback)) =
                (observed, verified, &mut self.on_first_accepted)
            && self.control.check().is_ok()
        {
            callback(result, observed, verified);
            self.first_accepted_sent = true;
        }
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
        if let Some(review) = self.reconcile_accepted_decision(&decision) {
            return review;
        }
        self.decision = Some(decision);
        disposition
    }
}

/// Adapts the original operation clock and retains trustworthy bounds after invalid observations.
pub(crate) struct ParentVerificationClock {
    budget: ParentSolveBudget,
    observations: Mutex<ParentClockObservations>,
}

#[derive(Default)]
struct ParentClockObservations {
    latest_elapsed: Option<Duration>,
    failed: bool,
}

impl ParentVerificationClock {
    pub(crate) fn new(budget: &ParentSolveBudget) -> Self {
        Self {
            budget: budget.clone(),
            observations: Mutex::new(ParentClockObservations::default()),
        }
    }

    /// Records an elapsed observation from another trusted native timing source.
    ///
    /// Router observations arrive after the review samples they enclose, so these bounds are
    /// accumulated by maximum rather than treated as a newly ordered clock read.
    pub(crate) fn retain_elapsed_bound(&self, elapsed: Duration) {
        let mut observations = self
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !representable_elapsed(elapsed) {
            observations.failed = true;
            return;
        }
        observations.latest_elapsed = Some(
            observations
                .latest_elapsed
                .map_or(elapsed, |previous| previous.max(elapsed)),
        );
    }

    /// Takes and retains one actual monotonic sample, rejecting regressions against every
    /// previously trusted source observation.
    pub(crate) fn observe_elapsed(&self) -> Result<Duration, ()> {
        let Some(elapsed) = self.budget.checked_elapsed() else {
            self.mark_failed();
            return Err(());
        };
        if !representable_elapsed(elapsed) {
            self.mark_failed();
            return Err(());
        }
        let mut observations = self
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if observations
            .latest_elapsed
            .is_some_and(|previous| elapsed < previous)
        {
            observations.failed = true;
            return Err(());
        }
        observations.latest_elapsed = Some(elapsed);
        Ok(elapsed)
    }

    pub(crate) fn latest_elapsed_bound(&self) -> Option<Duration> {
        self.observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest_elapsed
    }

    pub(crate) fn has_failed(&self) -> bool {
        self.observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failed
    }

    fn mark_failed(&self) {
        self.observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failed = true;
    }
}

impl VerificationClock for ParentVerificationClock {
    fn now_milliseconds(&self) -> DurationMillis {
        self.observe_elapsed()
            .and_then(|elapsed| {
                u64::try_from(elapsed.as_millis())
                    .ok()
                    .and_then(|milliseconds| DurationMillis::new(milliseconds).ok())
                    .ok_or(())
            })
            .unwrap_or(DurationMillis::MAX)
    }
}

fn representable_elapsed(elapsed: Duration) -> bool {
    u64::try_from(elapsed.as_millis())
        .ok()
        .and_then(|milliseconds| DurationMillis::new(milliseconds).ok())
        .is_some()
}

pub(crate) fn terminal_phase_timings(
    review_timings: Option<AcceptancePhaseTimings>,
    compile_elapsed: DurationMillis,
    backend_elapsed: Option<DurationMillis>,
) -> RunPhaseTimingsV1 {
    RunPhaseTimingsV1 {
        compile_milliseconds: Some(compile_elapsed),
        backend_milliseconds: backend_elapsed,
        projection_milliseconds: review_timings.map(|timings| timings.projection_milliseconds),
        structural_validation_milliseconds: review_timings
            .map(|timings| timings.structural_validation_milliseconds),
        score_recomputation_milliseconds: review_timings
            .map(|timings| timings.score_recomputation_milliseconds),
        required_rule_verification_milliseconds: review_timings
            .map(|timings| timings.required_rule_verification_milliseconds),
        evidence_persistence_milliseconds: None,
        optional_explanation_milliseconds: None,
    }
}

fn checked_sum(
    previous: AcceptancePhaseTimings,
    current: AcceptancePhaseTimings,
) -> Option<AcceptancePhaseTimings> {
    let add = |left: DurationMillis, right: DurationMillis| {
        left.value()
            .checked_add(right.value())
            .and_then(|sum| DurationMillis::new(sum).ok())
    };
    Some(AcceptancePhaseTimings {
        projection_milliseconds: add(
            previous.projection_milliseconds,
            current.projection_milliseconds,
        )?,
        structural_validation_milliseconds: add(
            previous.structural_validation_milliseconds,
            current.structural_validation_milliseconds,
        )?,
        score_recomputation_milliseconds: add(
            previous.score_recomputation_milliseconds,
            current.score_recomputation_milliseconds,
        )?,
        required_rule_verification_milliseconds: add(
            previous.required_rule_verification_milliseconds,
            current.required_rule_verification_milliseconds,
        )?,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_types::{CancellationToken, MonotonicClock};

    #[test]
    fn terminal_phases_include_each_review_and_keep_backend_time_separate()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = AcceptancePhaseTimings {
            projection_milliseconds: DurationMillis::new(4)?,
            structural_validation_milliseconds: DurationMillis::new(3)?,
            score_recomputation_milliseconds: DurationMillis::new(2)?,
            required_rule_verification_milliseconds: DurationMillis::new(1)?,
        };
        let last = AcceptancePhaseTimings {
            projection_milliseconds: DurationMillis::new(1)?,
            structural_validation_milliseconds: DurationMillis::new(2)?,
            score_recomputation_milliseconds: DurationMillis::new(3)?,
            required_rule_verification_milliseconds: DurationMillis::new(4)?,
        };
        let review = checked_sum(first, last)
            .ok_or_else(|| std::io::Error::other("both review phases must be finite"))?;
        let phases = terminal_phase_timings(
            Some(review),
            DurationMillis::new(7)?,
            Some(DurationMillis::new(11)?),
        );
        assert_eq!(phases.compile_milliseconds, Some(DurationMillis::new(7)?));
        assert_eq!(phases.backend_milliseconds, Some(DurationMillis::new(11)?));
        assert_eq!(
            phases.projection_milliseconds,
            Some(DurationMillis::new(5)?)
        );
        assert_eq!(
            phases.structural_validation_milliseconds,
            Some(DurationMillis::new(5)?)
        );
        assert_eq!(
            phases.score_recomputation_milliseconds,
            Some(DurationMillis::new(5)?)
        );
        assert_eq!(
            phases.required_rule_verification_milliseconds,
            Some(DurationMillis::new(5)?)
        );

        assert!(
            checked_sum(
                AcceptancePhaseTimings {
                    projection_milliseconds: DurationMillis::MAX,
                    ..first
                },
                last
            )
            .is_none()
        );
        assert_eq!(
            terminal_phase_timings(None, DurationMillis::ZERO, None).projection_milliseconds,
            None
        );
        Ok(())
    }
    struct TestMonotonicClock(std::sync::atomic::AtomicU64);

    impl MonotonicClock for TestMonotonicClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(std::sync::atomic::Ordering::Relaxed))
        }
    }

    #[test]
    fn invalid_verification_sample_preserves_bounds_and_later_valid_recovery()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw = std::sync::Arc::new(TestMonotonicClock(std::sync::atomic::AtomicU64::new(100)));
        let budget = ParentSolveBudget::new(
            DurationMillis::new(1_000)?,
            raw.clone(),
            CancellationToken::new(),
        )?;
        let clock = ParentVerificationClock::new(&budget);

        assert_eq!(clock.now_milliseconds(), DurationMillis::ZERO);
        raw.0.store(99, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(clock.now_milliseconds(), DurationMillis::MAX);
        assert!(clock.has_failed());
        assert_eq!(clock.latest_elapsed_bound(), Some(Duration::ZERO));

        raw.0.store(110, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(clock.now_milliseconds(), DurationMillis::new(10)?);
        assert_eq!(clock.observe_elapsed(), Ok(Duration::from_millis(10)));
        assert_eq!(
            clock.latest_elapsed_bound(),
            Some(Duration::from_millis(10))
        );
        assert!(clock.has_failed());
        Ok(())
    }

    #[test]
    fn retained_router_bound_rejects_actual_sample_below_last_trusted_observation()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw = std::sync::Arc::new(TestMonotonicClock(std::sync::atomic::AtomicU64::new(100)));
        let budget = ParentSolveBudget::new(
            DurationMillis::new(1_000)?,
            raw.clone(),
            CancellationToken::new(),
        )?;
        let clock = ParentVerificationClock::new(&budget);
        clock.retain_elapsed_bound(Duration::from_millis(20));
        raw.0.store(115, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(clock.observe_elapsed(), Err(()));
        assert!(clock.has_failed());
        assert_eq!(
            clock.latest_elapsed_bound(),
            Some(Duration::from_millis(20))
        );
        raw.0.store(120, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(clock.observe_elapsed(), Ok(Duration::from_millis(20)));
        Ok(())
    }
}
