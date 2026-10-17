//! Descriptive, source-owned Alternatives equivalence, never acceptance or provenance authority.
use super::{
    AssignmentRuleError, authority,
    budget::{OperationBudget, count},
    input::AssignmentInput,
    profile,
};
use crate::model::{AssignmentPair, LockState};
use eutheto_domain_api::DomainPackError;
use eutheto_domain_ir::{AcceptedResult, RunObjectiveContext};
use eutheto_types::{EntityId, PersonId, Rfc3339Timestamp, RuleId, ScenarioDocument};
use std::collections::BTreeMap;

/// An opaque complete census. Only the cumulative admission comparator proves equality.
/// Source bytes and the captured baseline are private bindings, not public checksum authority.
#[derive(Debug)]
pub struct WorkforceAlternativeOutcomes {
    source: Vec<u8>,
    baseline: Vec<u8>,
    facts: Vec<(Key, Values)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Family {
    Metric,
    Baseline,
    Soft,
    Direct,
    AssignmentTarget,
    WeekendTarget,
    Consecutive,
    Pairing,
    Coverage,
    Workload,
    Quota,
    MaximumHours,
    MaximumCount,
    MaximumConsecutive,
    MaximumWeekends,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct Key {
    pub family: Family,
    pub rule: RuleId,
    pub person: Option<PersonId>,
    pub participant: Option<PersonId>,
    pub owner: Option<EntityId>,
    pub shift: Option<crate::ids::ShiftId>,
    pub period: Option<(Rfc3339Timestamp, Rfc3339Timestamp)>,
}
impl Key {
    pub fn person(family: Family, rule: RuleId, person: PersonId) -> Self {
        Self {
            family,
            rule,
            person: Some(person),
            participant: None,
            owner: None,
            shift: None,
            period: None,
        }
    }
}

// Each family owns the meaning of its exact scalar coordinates; no floats or rounded pages.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Values {
    Counts {
        examined: i128,
        matched: i128,
        mismatched: i128,
        cost: i128,
    },
    Target {
        actual: i128,
        target: i128,
        difference: i128,
        cost: i128,
    },
    Workload {
        actual: i128,
        deviation: i128,
        denominator: i128,
        cost: i128,
    },
    Amount(i128),
    Baseline {
        added: i128,
        removed: i128,
    },
    Soft {
        retained: bool,
        penalty: i128,
    },
}
#[derive(Default)]
pub(super) struct Collector {
    facts: BTreeMap<Key, Values>,
}
impl Collector {
    pub fn insert(
        &mut self,
        key: Key,
        value: Values,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        budget.step()?;
        budget.steps(u64::from(usize::BITS - self.facts.len().leading_zeros()))?;
        if self.facts.contains_key(&key) {
            return Err(AssignmentRuleError::InvalidConstruction(
                super::AssignmentConstructionIssue::InvalidRecord,
            ));
        }
        budget.reserve(1, 1, count(size_of::<(Key, Values)>() + 48)?)?;
        self.facts.insert(key, value);
        Ok(())
    }
    pub fn counts(
        &mut self,
        key: Key,
        examined: i128,
        matched: i128,
        mismatched: i128,
        cost: i128,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        budget.step()?;
        budget.steps(u64::from(usize::BITS - self.facts.len().leading_zeros()))?;
        if let Some(Values::Counts {
            examined: a,
            matched: b,
            mismatched: c,
            cost: d,
        }) = self.facts.get_mut(&key)
        {
            *a = a.checked_add(examined).ok_or_else(arithmetic)?;
            *b = b.checked_add(matched).ok_or_else(arithmetic)?;
            *c = c.checked_add(mismatched).ok_or_else(arithmetic)?;
            *d = d.checked_add(cost).ok_or_else(arithmetic)?;
            Ok(())
        } else {
            self.insert(
                key,
                Values::Counts {
                    examined,
                    matched,
                    mismatched,
                    cost,
                },
                budget,
            )
        }
    }
    pub fn amount(
        &mut self,
        key: Key,
        actual: i128,
        maximum: bool,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        budget.step()?;
        budget.steps(u64::from(usize::BITS - self.facts.len().leading_zeros()))?;
        if let Some(Values::Amount(previous)) = self.facts.get_mut(&key) {
            if !maximum {
                return Err(AssignmentRuleError::InvalidConstruction(
                    super::AssignmentConstructionIssue::InvalidRecord,
                ));
            }
            *previous = (*previous).max(actual);
            Ok(())
        } else {
            // The first real observation, including negative scheduled credits, initializes worst.
            self.insert(key, Values::Amount(actual), budget)
        }
    }
    fn baseline(
        &mut self,
        key: Key,
        added: i128,
        removed: i128,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        budget.step()?;
        budget.steps(u64::from(usize::BITS - self.facts.len().leading_zeros()))?;
        if let Some(Values::Baseline {
            added: a,
            removed: r,
        }) = self.facts.get_mut(&key)
        {
            *a = a.checked_add(added).ok_or_else(arithmetic)?;
            *r = r.checked_add(removed).ok_or_else(arithmetic)?;
            Ok(())
        } else {
            self.insert(key, Values::Baseline { added, removed }, budget)
        }
    }
    fn finish(
        self,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Vec<(Key, Values)>, AssignmentRuleError> {
        budget.sort_work(self.facts.len())?;
        budget.reserve(
            1,
            count(self.facts.len())?,
            count(self.facts.len())?
                .checked_mul(count(size_of::<(Key, Values)>())?)
                .ok_or_else(arithmetic)?,
        )?;
        let mut facts = Vec::with_capacity(self.facts.len());
        for (key, value) in self.facts {
            budget.step()?;
            // Absence is the canonical no-work zero amount for every configured workload family.
            if value != Values::Amount(0) {
                facts.push((key, value));
            }
        }
        Ok(facts)
    }
}
fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(super::AssignmentConstructionIssue::ArithmeticOverflow)
}
fn binding<T: serde::Serialize>(
    value: &T,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<u8>, DomainPackError> {
    let measured = super::boundary::preflight(value, budget, super::boundary::DOMAIN_LIMITS)
        .map_err(|error| authority::operation_error(&error))?;
    measured
        .reserve_json(budget, 1)
        .map_err(|error| authority::operation_error(&error))?;
    let bytes =
        serde_json::to_vec(value).map_err(|_| authority::contract("alternative_binding"))?;
    budget
        .check()
        .map_err(|error| authority::operation_error(&error))?;
    Ok(bytes)
}

pub(super) fn extract(
    document: &ScenarioDocument,
    accepted: &AcceptedResult,
    context: &RunObjectiveContext,
    budget: &mut OperationBudget<'_>,
) -> Result<WorkforceAlternativeOutcomes, DomainPackError> {
    let mut collector = Collector::default();
    authority::revalidate_workforce_accepted_with_outcomes(
        document,
        accepted,
        Some(context),
        budget,
        Some(&mut collector),
    )?;
    let source = binding(document, budget)?;
    let baseline = if let Some(external) = context.external_baseline.as_deref() {
        binding(external, budget)?
    } else {
        binding(&context.baseline.as_deref(), budget)?
    };
    // Verification metric values are profile neutral and non-rank; successful predicate populations
    // are intentionally not a new per-rule census.
    let metric = |name| -> Result<i128, DomainPackError> {
        let id = eutheto_domain_ir::MetricId::new(name)
            .map_err(|_| authority::contract("metric_identity"))?;
        match accepted.verification.metrics.get(&id) {
            Some(eutheto_domain_ir::MetricValue::Integer(value)) => Ok(i128::from(*value)),
            _ => Err(authority::contract("alternative_metric")),
        }
    };
    collector
        .insert(
            Key {
                family: Family::Metric,
                rule: RuleId::from_uuid(document.scenario_id.as_uuid()),
                person: None,
                participant: None,
                owner: None,
                shift: None,
                period: None,
            },
            Values::Counts {
                examined: metric(authority::CHECKED_METRIC)?,
                matched: metric(authority::SELECTED_METRIC)?,
                mismatched: metric(authority::VIOLATIONS_METRIC)?,
                cost: 0,
            },
            budget,
        )
        .map_err(|error| authority::operation_error(&error))?;
    let facts = collector
        .finish(budget)
        .map_err(|error| authority::operation_error(&error))?;
    Ok(WorkforceAlternativeOutcomes {
        source,
        baseline,
        facts,
    })
}
#[allow(clippy::too_many_lines)]
pub(super) fn collect_baseline_and_soft(
    input: &AssignmentInput,
    document: &ScenarioDocument,
    context: &RunObjectiveContext,
    selected: &[AssignmentPair],
    collector: &mut Collector,
    baseline: Option<&profile::SortedPairs<'_>>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    let resolved;
    let baseline = if let Some(baseline) = baseline {
        baseline
    } else {
        resolved =
            profile::sorted_baseline_pairs(input, document.scenario_id, Some(context), budget)?
                .ok_or_else(|| authority::contract("baseline_required"))?;
        &resolved
    };
    let mut pair_changes = 0_i128;
    let baseline_key = |person| {
        Key::person(
            Family::Baseline,
            RuleId::from_uuid(document.scenario_id.as_uuid()),
            person,
        )
    };
    for person in &input.people {
        collector
            .baseline(baseline_key(*person), 0, 0, budget)
            .map_err(|error| authority::operation_error(&error))?;
    }
    for pair in selected {
        budget
            .step()
            .map_err(|error| authority::operation_error(&error))?;
        if !baseline.contains(*pair, budget)? {
            collector
                .baseline(baseline_key(pair.person_id), 1, 0, budget)
                .map_err(|error| authority::operation_error(&error))?;
            pair_changes = pair_changes
                .checked_add(1)
                .ok_or_else(|| authority::contract("arithmetic"))?;
        }
    }
    for index in 0..baseline.len() {
        budget
            .step()
            .map_err(|error| authority::operation_error(&error))?;
        let pair = baseline.pair(index);
        budget
            .steps(u64::from(usize::BITS - selected.len().leading_zeros()))
            .map_err(|error| authority::operation_error(&error))?;
        if selected.binary_search(pair).is_err() {
            collector
                .baseline(baseline_key(pair.person_id), 0, 1, budget)
                .map_err(|error| authority::operation_error(&error))?;
            pair_changes = pair_changes
                .checked_add(1)
                .ok_or_else(|| authority::contract("arithmetic"))?;
        }
    }
    collector
        .insert(
            Key {
                family: Family::Baseline,
                rule: RuleId::from_uuid(document.scenario_id.as_uuid()),
                person: None,
                participant: None,
                owner: None,
                shift: None,
                period: None,
            },
            Values::Amount(pair_changes),
            budget,
        )
        .map_err(|error| authority::operation_error(&error))?;
    for lock in input.domain.locked_assignments.values() {
        budget
            .step()
            .map_err(|error| authority::operation_error(&error))?;
        if let LockState::Soft { stability_weight } = lock.state {
            let pair = AssignmentPair {
                person_id: lock.person_id,
                shift_id: lock.shift_id,
            };
            budget
                .steps(u64::from(usize::BITS - selected.len().leading_zeros()))
                .map_err(|error| authority::operation_error(&error))?;
            let retained = selected.binary_search(&pair).is_ok();
            let mut key = Key::person(
                Family::Soft,
                RuleId::from_uuid(lock.id.as_uuid()),
                lock.person_id,
            );
            key.shift = Some(lock.shift_id);
            collector
                .insert(
                    key,
                    Values::Soft {
                        retained,
                        penalty: if retained {
                            0
                        } else {
                            i128::from(stability_weight)
                        },
                    },
                    budget,
                )
                .map_err(|error| authority::operation_error(&error))?;
        }
    }
    Ok(())
}
pub(super) fn same(
    left: &WorkforceAlternativeOutcomes,
    right: &WorkforceAlternativeOutcomes,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, DomainPackError> {
    budget
        .check()
        .map_err(|error| authority::operation_error(&error))?;
    for (a, b) in [
        (&left.source, &right.source),
        (&left.baseline, &right.baseline),
    ] {
        if a.len() != b.len() {
            return Ok(false);
        }
        for (a, b) in a.iter().zip(b) {
            budget
                .step()
                .map_err(|error| authority::operation_error(&error))?;
            if a != b {
                return Ok(false);
            }
        }
    }
    if left.facts.len() != right.facts.len() {
        return Ok(false);
    }
    for (a, b) in left.facts.iter().zip(&right.facts) {
        budget
            .step()
            .map_err(|error| authority::operation_error(&error))?;
        if a != b {
            return Ok(false);
        }
    }
    budget
        .check()
        .map_err(|error| authority::operation_error(&error))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_types::{CancellationToken, OperationControl};

    #[test]
    fn rolling_worst_uses_real_signed_observation_and_zero_is_canonical()
    -> Result<(), AssignmentRuleError> {
        let mut budget = OperationBudget::evaluation(None);
        let mut collector = Collector::default();
        let key = Key::person(
            Family::MaximumHours,
            RuleId::from_uuid(uuid::Uuid::nil()),
            PersonId::from_uuid(uuid::Uuid::nil()),
        );
        collector.amount(key, -30_000_000_000, true, &mut budget)?;
        collector.amount(key, -15_000_000_000, true, &mut budget)?;
        let zero = Key {
            family: Family::MaximumCount,
            ..key
        };
        collector.amount(zero, 0, false, &mut budget)?;
        let facts = collector.finish(&mut budget)?;
        assert_eq!(facts, vec![(key, Values::Amount(-15_000_000_000))]);
        Ok(())
    }

    #[test]
    fn interruption_during_binding_comparison_and_canonicalization_is_not_equality()
    -> Result<(), AssignmentRuleError> {
        let token = CancellationToken::new();
        let control = OperationControl::Cancellation(token);
        let mut budget = OperationBudget::evaluation(Some(&control));
        budget.cancel_after_steps(4)?;
        let left = WorkforceAlternativeOutcomes {
            source: vec![1; 16],
            baseline: vec![],
            facts: vec![],
        };
        let right = WorkforceAlternativeOutcomes {
            source: vec![1; 16],
            baseline: vec![],
            facts: vec![],
        };
        assert!(matches!(
            same(&left, &right, &mut budget),
            Err(DomainPackError::Cancelled)
        ));
        let token = CancellationToken::new();
        let control = OperationControl::Cancellation(token);
        let mut budget = OperationBudget::evaluation(Some(&control));
        let mut collector = Collector::default();
        for index in 0..8 {
            collector.amount(
                Key::person(
                    Family::MaximumCount,
                    RuleId::from_uuid(uuid::Uuid::from_u128(index)),
                    PersonId::from_uuid(uuid::Uuid::nil()),
                ),
                1,
                false,
                &mut budget,
            )?;
        }
        budget.cancel_after_steps(2)?;
        assert!(matches!(
            collector.finish(&mut budget),
            Err(AssignmentRuleError::Cancelled)
        ));
        Ok(())
    }
}
