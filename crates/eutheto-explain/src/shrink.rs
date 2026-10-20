use eutheto_domain_ir::{
    ConflictGroupV1, ConflictMinimality, ConflictShrinkStopReason, ConflictShrinkSummaryV1,
    ConflictUnavailableReason, DomainContractError, InfeasibilityEvidenceV1,
    MAX_EXPLANATION_RECORDS,
};
use eutheto_types::SolveBudgetView;
use std::fmt;

/// Proof strength returned by one deletion trial.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictTrialOutcome {
    /// The candidate group set was proven infeasible.
    ProvenInfeasible,
    /// The candidate group set was proven feasible.
    ProvenFeasible,
    /// The trial established neither result.
    Inconclusive,
}

/// Invalid conflict input, an impossible summary count, or an invalid state transition.
#[derive(Debug)]
pub enum ConflictShrinkError {
    /// A conflict input must be non-empty, bounded, canonical, and contain valid groups.
    InvalidGroups,
    /// A domain conflict group violated its wire contract.
    InvalidGroup(DomainContractError),
    /// A platform conversion could not represent a contract count.
    CountOverflow,
    /// The state machine was advanced in an invalid order.
    InvalidTransition(ConflictShrinkTransition),
}

/// State-machine transition rejected by [`ConflictShrinker`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictShrinkTransition {
    /// A second trial was requested before the pending trial was completed.
    PendingTrial,
    /// A trial was completed after an earlier trial had already been completed.
    DoubleCompletion,
    /// A completion was requested before any trial had been issued.
    MissingTrial,
    /// A result was requested while trials could still be issued or completed.
    PrematureResult,
}

impl fmt::Display for ConflictShrinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGroups => formatter.write_str("conflict groups are not canonical"),
            Self::InvalidGroup(error) => write!(formatter, "invalid conflict group: {error}"),
            Self::CountOverflow => formatter.write_str("conflict count cannot be represented"),
            Self::InvalidTransition(transition) => {
                let message = match transition {
                    ConflictShrinkTransition::PendingTrial => {
                        "a trial is already pending completion"
                    }
                    ConflictShrinkTransition::DoubleCompletion => {
                        "the current trial was already completed"
                    }
                    ConflictShrinkTransition::MissingTrial => "no trial is pending completion",
                    ConflictShrinkTransition::PrematureResult => {
                        "the shrink result is not complete"
                    }
                };
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for ConflictShrinkError {}

struct PendingTrial {
    index: usize,
    removed: ConflictGroupV1,
}

/// Deterministically shrinks a sufficient conflict with bounded deletion trials.
///
/// The state machine owns one mutable group vector and exposes each deletion candidate by borrow.
/// The same shared budget is inspected immediately before every trial and again when that trial is
/// completed. A group is removed only when the caller reports that the conflict remains infeasible
/// without it. The first inconclusive trial stops shrinking, and cancellation, expiry, and trial
/// exhaustion retain the current sufficient conflict. If the empty set is proven infeasible, the
/// result is foundational unavailability rather than an invalid empty conflict.
///
/// # Errors
///
/// [`Self::new`] rejects a noncanonical input group set or an unrepresentable count. The state
/// methods reject pending, duplicate, missing, and premature transitions.
pub struct ConflictShrinker {
    remaining: Vec<ConflictGroupV1>,
    pending: Option<PendingTrial>,
    initial_group_count: u32,
    attempted_trials: u32,
    max_trials: u32,
    index: usize,
    budget: SolveBudgetView,
    stop_reason: Option<ConflictShrinkStopReason>,
    foundational: bool,
}

impl ConflictShrinker {
    /// Starts a bounded deletion pass by taking ownership of one canonical sufficient conflict.
    ///
    /// # Errors
    /// Rejects invalid/noncanonical groups or a group count that exceeds `u32`.
    pub fn new(
        groups: Vec<ConflictGroupV1>,
        max_trials: u32,
        budget: SolveBudgetView,
    ) -> Result<Self, ConflictShrinkError> {
        validate_groups(&groups)?;
        let initial_group_count =
            u32::try_from(groups.len()).map_err(|_| ConflictShrinkError::CountOverflow)?;
        Ok(Self {
            remaining: groups,
            pending: None,
            initial_group_count,
            attempted_trials: 0,
            max_trials,
            index: 0,
            budget,
            stop_reason: (max_trials == 0).then_some(ConflictShrinkStopReason::NotAttempted),
            foundational: false,
        })
    }

    /// Borrows the currently retained set after a trial completes.
    ///
    /// During a pending trial this is the proposed deletion set, not an admitted proof.
    #[must_use]
    pub fn retained_groups(&self) -> &[ConflictGroupV1] {
        &self.remaining
    }

    /// Returns the next deletion candidate, borrowing the in-place remaining groups.
    ///
    /// `None` means the pass has stopped or all deletion positions have been attempted. The
    /// caller must invoke [`Self::complete_trial`] exactly once for every `Some` result.
    ///
    /// # Errors
    /// Rejects starting a new trial while the previous trial is still pending.
    pub fn next_trial(&mut self) -> Result<Option<&[ConflictGroupV1]>, ConflictShrinkError> {
        if self.pending.is_some() {
            return Err(ConflictShrinkError::InvalidTransition(
                ConflictShrinkTransition::PendingTrial,
            ));
        }
        if self.stop_reason.is_some() || self.foundational {
            return Ok(None);
        }
        if self.max_trials == 0 {
            self.stop_reason = Some(ConflictShrinkStopReason::NotAttempted);
            return Ok(None);
        }
        if self.attempted_trials >= self.initial_group_count {
            self.stop_reason = Some(ConflictShrinkStopReason::Completed);
            return Ok(None);
        }
        if self.attempted_trials >= self.max_trials {
            self.stop_reason = Some(ConflictShrinkStopReason::TrialLimit);
            return Ok(None);
        }
        let snapshot = self.budget.snapshot();
        if snapshot.cancelled {
            self.stop_reason = Some(ConflictShrinkStopReason::Cancelled);
            return Ok(None);
        }
        if snapshot.expired {
            self.stop_reason = Some(ConflictShrinkStopReason::BudgetExpired);
            return Ok(None);
        }

        let removed = self.remaining.remove(self.index);
        self.attempted_trials += 1;
        self.pending = Some(PendingTrial {
            index: self.index,
            removed,
        });
        Ok(Some(&self.remaining))
    }

    /// Completes the pending deletion trial and advances or restores the in-place candidate.
    ///
    /// # Errors
    /// Rejects completion when no trial is pending.
    pub fn complete_trial(
        &mut self,
        outcome: ConflictTrialOutcome,
    ) -> Result<(), ConflictShrinkError> {
        let Some(pending) = self.pending.take() else {
            let transition = if self.attempted_trials == 0 {
                ConflictShrinkTransition::MissingTrial
            } else {
                ConflictShrinkTransition::DoubleCompletion
            };
            return Err(ConflictShrinkError::InvalidTransition(transition));
        };

        let snapshot = self.budget.snapshot();
        if snapshot.cancelled {
            self.remaining.insert(pending.index, pending.removed);
            self.stop_reason = Some(ConflictShrinkStopReason::Cancelled);
            return Ok(());
        }
        if snapshot.expired {
            self.remaining.insert(pending.index, pending.removed);
            self.stop_reason = Some(ConflictShrinkStopReason::BudgetExpired);
            return Ok(());
        }

        match outcome {
            ConflictTrialOutcome::ProvenInfeasible if self.remaining.is_empty() => {
                self.foundational = true;
            }
            ConflictTrialOutcome::ProvenInfeasible => {
                self.finish_after_trial();
            }
            ConflictTrialOutcome::ProvenFeasible => {
                self.remaining.insert(pending.index, pending.removed);
                self.index += 1;
                self.finish_after_trial();
            }
            ConflictTrialOutcome::Inconclusive => {
                self.remaining.insert(pending.index, pending.removed);
                self.stop_reason = Some(ConflictShrinkStopReason::Inconclusive);
            }
        }
        Ok(())
    }

    /// Finishes the state machine and assembles the validated immutable evidence result.
    ///
    /// # Errors
    /// Rejects an unfinished pass or invalid retained conflict evidence.
    pub fn into_result(self) -> Result<InfeasibilityEvidenceV1, ConflictShrinkError> {
        if self.pending.is_some() {
            return Err(ConflictShrinkError::InvalidTransition(
                ConflictShrinkTransition::PrematureResult,
            ));
        }
        if self.foundational {
            return Ok(InfeasibilityEvidenceV1::Unavailable {
                reason: ConflictUnavailableReason::FoundationalInfeasibility,
            });
        }
        let Some(stop_reason) = self.stop_reason else {
            return Err(ConflictShrinkError::InvalidTransition(
                ConflictShrinkTransition::PrematureResult,
            ));
        };
        conflict_result(
            self.remaining,
            self.initial_group_count,
            self.attempted_trials,
            self.max_trials,
            stop_reason,
        )
    }

    fn finish_after_trial(&mut self) {
        if self.attempted_trials >= self.initial_group_count {
            self.stop_reason = Some(ConflictShrinkStopReason::Completed);
        } else if self.attempted_trials >= self.max_trials {
            self.stop_reason = Some(ConflictShrinkStopReason::TrialLimit);
        }
    }
}

fn validate_groups(groups: &[ConflictGroupV1]) -> Result<(), ConflictShrinkError> {
    if groups.is_empty()
        || groups.len() > MAX_EXPLANATION_RECORDS
        || !groups.windows(2).all(|pair| pair[0] < pair[1])
    {
        return Err(ConflictShrinkError::InvalidGroups);
    }
    for group in groups {
        group
            .validate()
            .map_err(ConflictShrinkError::InvalidGroup)?;
    }
    Ok(())
}

fn conflict_result(
    groups: Vec<ConflictGroupV1>,
    initial_group_count: u32,
    attempted_trials: u32,
    max_trials: u32,
    stop_reason: ConflictShrinkStopReason,
) -> Result<InfeasibilityEvidenceV1, ConflictShrinkError> {
    let remaining_group_count =
        u32::try_from(groups.len()).map_err(|_| ConflictShrinkError::CountOverflow)?;
    let shrink = ConflictShrinkSummaryV1 {
        initial_group_count,
        remaining_group_count,
        attempted_trials,
        max_trials,
        stop_reason,
    };
    shrink
        .validate()
        .map_err(ConflictShrinkError::InvalidGroup)?;
    let minimality = if stop_reason == ConflictShrinkStopReason::Completed {
        ConflictMinimality::ProvenMinimal
    } else {
        ConflictMinimality::Sufficient
    };
    Ok(InfeasibilityEvidenceV1::Conflict {
        groups,
        minimality,
        shrink,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_domain_ir::AssumptionGroupId;
    use eutheto_types::{
        CancellationToken, DurationMillis, FixedMonotonicClock, MonotonicClock, ParentSolveBudget,
        RuleId, SolveBudgetView,
    };
    use std::sync::Arc;
    use std::time::Duration;

    fn group(index: u8) -> Result<ConflictGroupV1, Box<dyn std::error::Error>> {
        Ok(ConflictGroupV1 {
            group_id: AssumptionGroupId::new(format!("tests.group-{index}"))?,
            required_rules: vec![
                format!("01890a5d-ac96-7b64-9f74-bbfcf30f9f{index:02x}").parse::<RuleId>()?,
            ],
        })
    }

    fn budget(
        milliseconds: u64,
        cancellation: CancellationToken,
    ) -> Result<(Arc<FixedMonotonicClock>, SolveBudgetView), Box<dyn std::error::Error>> {
        let clock = Arc::new(FixedMonotonicClock::default());
        let shared: Arc<dyn MonotonicClock> = clock.clone();
        let parent =
            ParentSolveBudget::new(DurationMillis::new(milliseconds)?, shared, cancellation)?;
        Ok((clock, parent.phase_view()))
    }

    fn conflict_parts(
        evidence: InfeasibilityEvidenceV1,
    ) -> Result<
        (
            Vec<ConflictGroupV1>,
            ConflictMinimality,
            ConflictShrinkSummaryV1,
        ),
        Box<dyn std::error::Error>,
    > {
        match evidence {
            InfeasibilityEvidenceV1::Conflict {
                groups,
                minimality,
                shrink,
            } => Ok((groups, minimality, shrink)),
            InfeasibilityEvidenceV1::Unavailable { .. } => Err("expected conflict evidence".into()),
        }
    }

    #[test]
    fn removes_only_proven_redundant_groups_and_proves_minimal()
    -> Result<(), Box<dyn std::error::Error>> {
        let groups = vec![group(1)?, group(2)?, group(3)?];
        let (_, view) = budget(1_000, CancellationToken::new())?;
        let mut shrinker = ConflictShrinker::new(groups.clone(), 3, view)?;
        let mut trial = 0;
        while shrinker.next_trial()?.is_some() {
            trial += 1;
            shrinker.complete_trial(if trial == 1 {
                ConflictTrialOutcome::ProvenInfeasible
            } else {
                ConflictTrialOutcome::ProvenFeasible
            })?;
        }
        let (remaining, minimality, summary) = conflict_parts(shrinker.into_result()?)?;
        assert_eq!(remaining, groups[1..]);
        assert_eq!(minimality, ConflictMinimality::ProvenMinimal);
        assert_eq!(summary.attempted_trials, 3);
        assert_eq!(summary.stop_reason, ConflictShrinkStopReason::Completed);
        summary.validate()?;
        Ok(())
    }

    #[test]
    fn reports_zero_trials_and_trial_exhaustion_exactly() -> Result<(), Box<dyn std::error::Error>>
    {
        let groups = vec![group(1)?, group(2)?];
        let (_, view) = budget(1_000, CancellationToken::new())?;
        let mut zero = ConflictShrinker::new(groups.clone(), 0, view.phase_view())?;
        assert!(zero.next_trial()?.is_none());
        let (_, minimality, zero_summary) = conflict_parts(zero.into_result()?)?;
        assert_eq!(minimality, ConflictMinimality::Sufficient);
        assert_eq!(
            zero_summary.stop_reason,
            ConflictShrinkStopReason::NotAttempted
        );
        assert_eq!(zero_summary.attempted_trials, 0);

        let mut limited = ConflictShrinker::new(groups.clone(), 1, view)?;
        assert!(limited.next_trial()?.is_some());
        limited.complete_trial(ConflictTrialOutcome::ProvenInfeasible)?;
        let (remaining, minimality, limited_summary) = conflict_parts(limited.into_result()?)?;
        assert_eq!(remaining, vec![groups[1].clone()]);
        assert_eq!(minimality, ConflictMinimality::Sufficient);
        assert_eq!(
            limited_summary.stop_reason,
            ConflictShrinkStopReason::TrialLimit
        );
        assert_eq!(limited_summary.attempted_trials, 1);
        limited_summary.validate()?;
        Ok(())
    }

    #[test]
    fn distinguishes_shared_budget_expiry_cancellation_and_inconclusive()
    -> Result<(), Box<dyn std::error::Error>> {
        let groups = vec![group(1)?, group(2)?];
        let (clock, expired_view) = budget(5, CancellationToken::new())?;
        clock.advance(Duration::from_millis(5))?;
        let mut expired = ConflictShrinker::new(groups.clone(), 2, expired_view)?;
        assert!(expired.next_trial()?.is_none());
        let (_, _, expired_summary) = conflict_parts(expired.into_result()?)?;
        assert_eq!(
            expired_summary.stop_reason,
            ConflictShrinkStopReason::BudgetExpired
        );
        assert_eq!(expired_summary.attempted_trials, 0);

        let cancellation = CancellationToken::new();
        let (_, cancelled_view) = budget(1_000, cancellation.clone())?;
        cancellation.cancel();
        let mut cancelled = ConflictShrinker::new(groups.clone(), 2, cancelled_view)?;
        assert!(cancelled.next_trial()?.is_none());
        let (_, _, cancelled_summary) = conflict_parts(cancelled.into_result()?)?;
        assert_eq!(
            cancelled_summary.stop_reason,
            ConflictShrinkStopReason::Cancelled
        );
        assert_eq!(cancelled_summary.attempted_trials, 0);

        let (_, view) = budget(1_000, CancellationToken::new())?;
        let mut inconclusive = ConflictShrinker::new(groups.clone(), 2, view)?;
        assert!(inconclusive.next_trial()?.is_some());
        inconclusive.complete_trial(ConflictTrialOutcome::Inconclusive)?;
        let (remaining, minimality, summary) = conflict_parts(inconclusive.into_result()?)?;
        assert_eq!(remaining, groups);
        assert_eq!(minimality, ConflictMinimality::Sufficient);
        assert_eq!(summary.stop_reason, ConflictShrinkStopReason::Inconclusive);
        assert_eq!(summary.attempted_trials, 1);
        Ok(())
    }

    #[test]
    fn restores_pending_group_after_late_or_cancelled_trial()
    -> Result<(), Box<dyn std::error::Error>> {
        let groups = vec![group(1)?, group(2)?];
        let (clock, late_view) = budget(5, CancellationToken::new())?;
        let mut late = ConflictShrinker::new(groups.clone(), 2, late_view)?;
        let candidate = late.next_trial()?.ok_or("missing late candidate")?;
        assert_eq!(candidate, &groups[1..]);
        clock.advance(Duration::from_millis(5))?;
        late.complete_trial(ConflictTrialOutcome::ProvenInfeasible)?;
        let (remaining, _, late_summary) = conflict_parts(late.into_result()?)?;
        assert_eq!(remaining, groups);
        assert_eq!(
            late_summary.stop_reason,
            ConflictShrinkStopReason::BudgetExpired
        );

        let cancellation = CancellationToken::new();
        let (_, cancelled_view) = budget(1_000, cancellation.clone())?;
        let mut cancelled = ConflictShrinker::new(groups.clone(), 2, cancelled_view)?;
        assert!(cancelled.next_trial()?.is_some());
        cancellation.cancel();
        cancelled.complete_trial(ConflictTrialOutcome::ProvenInfeasible)?;
        let (remaining, _, cancelled_summary) = conflict_parts(cancelled.into_result()?)?;
        assert_eq!(remaining, groups);
        assert_eq!(
            cancelled_summary.stop_reason,
            ConflictShrinkStopReason::Cancelled
        );
        Ok(())
    }

    #[test]
    fn rejects_pending_double_missing_and_premature_transitions()
    -> Result<(), Box<dyn std::error::Error>> {
        let groups = vec![group(1)?, group(2)?];
        let (_, view) = budget(1_000, CancellationToken::new())?;
        let mut shrinker = ConflictShrinker::new(groups.clone(), 2, view)?;
        assert!(matches!(
            shrinker.complete_trial(ConflictTrialOutcome::ProvenFeasible),
            Err(ConflictShrinkError::InvalidTransition(
                ConflictShrinkTransition::MissingTrial
            ))
        ));
        assert!(shrinker.next_trial()?.is_some());
        assert!(matches!(
            shrinker.next_trial(),
            Err(ConflictShrinkError::InvalidTransition(
                ConflictShrinkTransition::PendingTrial
            ))
        ));
        shrinker.complete_trial(ConflictTrialOutcome::ProvenFeasible)?;
        assert!(matches!(
            shrinker.complete_trial(ConflictTrialOutcome::ProvenFeasible),
            Err(ConflictShrinkError::InvalidTransition(
                ConflictShrinkTransition::DoubleCompletion
            ))
        ));
        assert!(matches!(
            shrinker.into_result(),
            Err(ConflictShrinkError::InvalidTransition(
                ConflictShrinkTransition::PrematureResult
            ))
        ));
        Ok(())
    }

    #[test]
    fn converts_a_proven_empty_conflict_to_foundational_unavailability()
    -> Result<(), Box<dyn std::error::Error>> {
        let groups = vec![group(1)?];
        let (_, view) = budget(1_000, CancellationToken::new())?;
        let mut shrinker = ConflictShrinker::new(groups.clone(), 1, view)?;
        assert!(shrinker.next_trial()?.is_some());
        shrinker.complete_trial(ConflictTrialOutcome::ProvenInfeasible)?;
        assert_eq!(
            shrinker.into_result()?,
            InfeasibilityEvidenceV1::Unavailable {
                reason: ConflictUnavailableReason::FoundationalInfeasibility,
            }
        );
        Ok(())
    }
}
