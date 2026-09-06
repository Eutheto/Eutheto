//! Independent original-domain predicates. This module never consumes candidate or IR decisions.
mod coverage;
mod predicates;

use super::{
    AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleEvaluation, AssignmentRuleLimit,
    InstantInterval, RequiredRulePartition,
    budget::{MAX_SELECTED_PAIRS, OperationBudget, add, count, within},
    input::AssignmentInput,
    intervals::date_range,
};
use crate::{
    ids::ShiftId,
    model::{ActiveRange, AssignmentPair, AvailabilityKind, CategoryPair, LockState, Scope, WorkforceEntity, WorkforceRule},
    temporal::ResolvedShift,
};
use eutheto_domain_ir::{DomainEntityId, DomainEntityKindId, DomainEntityRef, RuleEvaluation, VerificationFactId, VerificationValue};
use eutheto_types::{CancellationToken, EntityId, PersonId, RuleId, ScenarioDocument};
use std::collections::BTreeMap;

const VIOLATIONS: &str = "official.workforce.fact.violation_count";
const CHECKED: &str = "official.workforce.fact.checked_predicate_count";
const SUMMARY: &str = "official.workforce.evaluation.summary";

/// Evaluates the four assignment-rule families and unconditional activity/approved-leave facts.
///
/// This is a bounded contribution, not complete verification or feasibility authority. Identified
/// but inadmissible selections remain in the population and yield original-domain violations.
///
/// # Errors
/// Rejects malformed documents/selections, temporal review, cancellation and finite-work/output
/// limits. An error never returns a partial evaluation or obligation partition.
pub fn evaluate_assignment_rules(
    document: &ScenarioDocument,
    selected_pairs: &[AssignmentPair],
    cancellation: Option<&CancellationToken>,
) -> Result<AssignmentRuleEvaluation, AssignmentRuleError> {
    let mut budget = OperationBudget::evaluation(cancellation);
    budget.check()?;
    within(count(selected_pairs.len())?, MAX_SELECTED_PAIRS, AssignmentRuleLimit::SelectedPairs)?;
    let input = AssignmentInput::new(document, &mut budget)?;
    input.validate_selection(selected_pairs, &mut budget)?;
    let selected = Selected::new(&input, selected_pairs, &mut budget)?;
    let mut evaluations = Vec::new();
    let mut obligations = RequiredRulePartition { handled: Vec::new(), remaining: Vec::new() };

    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        let id = RuleId::from_uuid(person_id.as_uuid());
        retain_id(&mut obligations.handled, id, &mut budget)?;
        let mut summary = Summary::default();
        if let Some(shifts) = selected.by_person.get(person_id) {
            let allowed = match person.active_range {
                ActiveRange::Always {} => None,
                ActiveRange::DateRange(range) => Some(date_range(range, &document.settings, EntityId::from_uuid(person_id.as_uuid()))?),
            };
            for shift in shifts {
                budget.step()?;
                let query = predicates::interval(shift);
                let failed = allowed.is_some_and(|allowed| query.start < allowed.start || query.end > allowed.end);
                let mut witness = Witness::pair(AssignmentPair { person_id: *person_id, shift_id: shift.id }, EntityId::from_uuid(person_id.as_uuid()), "person", "outside_active_range");
                if let Some(allowed) = allowed {
                    witness.interval = if query.start < allowed.start {
                        Some(InstantInterval { start: query.start, end: query.end.min(allowed.start) })
                    } else if query.end > allowed.end {
                        Some(InstantInterval { start: query.start.max(allowed.end), end: query.end })
                    } else { None };
                }
                summary.predicate(failed, witness, &mut budget)?;
            }
        }
        evaluations.push(summary.finish(id, &mut budget)?);
    }

    for entity in input.domain.entities.values() {
        budget.step()?;
        let WorkforceEntity::Availability(record) = entity else { continue; };
        if record.availability_kind != AvailabilityKind::ApprovedTimeOff { continue; }
        let id = RuleId::from_uuid(record.id.as_uuid());
        retain_id(&mut obligations.handled, id, &mut budget)?;
        let mut summary = Summary::default();
        if let Some(shifts) = selected.by_person.get(&record.person_id) {
            for shift in shifts {
                budget.step()?;
                let metadata = input.metadata(shift)?;
                if predicates::availability_matches(record, &metadata, &mut budget)? {
                    let failure = predicates::availability_failure(record, shift, &document.settings, &mut budget)?;
                    let mut witness = Witness::pair(AssignmentPair { person_id: record.person_id, shift_id: shift.id }, record.id.as_entity_id(), "availability", "approved_time_off");
                    witness.interval = failure;
                    summary.predicate(failure.is_some(), witness, &mut budget)?;
                }
            }
        }
        evaluations.push(summary.finish(id, &mut budget)?);
    }

    for rule in input.domain.rules.values() {
        budget.step()?;
        let (id, active, scope) = rule.header();
        if !active { continue; }
        let mut summary = Summary::default();
        match rule {
            WorkforceRule::Eligibility { .. } | WorkforceRule::Availability { .. } => {
                for (person_id, shifts) in &selected.by_person {
                    budget.step()?;
                    let person = input.person(*person_id).ok_or_else(invalid)?;
                    if !predicates::person_matches(person, scope, &mut budget)? { continue; }
                    for shift in shifts {
                        budget.step()?;
                        let metadata = input.metadata(shift)?;
                        if !predicates::shift_matches(shift, &metadata, scope, &mut budget)? { continue; }
                        let pair = AssignmentPair { person_id: *person_id, shift_id: shift.id };
                        if matches!(rule, WorkforceRule::Eligibility { .. }) {
                            let allowed = predicates::type_allowed(person, &metadata, &mut budget)?;
                            summary.predicate(!allowed, predicates::pair_witness(&input, pair, "assignment_type_not_allowed")?, &mut budget)?;
                            let qualified = predicates::qualified(person, &metadata.assignment_type.qualifications, predicates::interval(shift), &mut budget)?;
                            summary.predicate(!qualified, predicates::pair_witness(&input, pair, "qualification_expression")?, &mut budget)?;
                        } else if let Some(records) = input.availability_by_person.get(person_id) {
                            for record_id in records {
                                budget.step()?;
                                let Some(WorkforceEntity::Availability(record)) = input.domain.entities.get(&record_id.as_entity_id()) else { return Err(invalid()); };
                                if !matches!(record.availability_kind, AvailabilityKind::Unavailable | AvailabilityKind::AvailableOnly)
                                    || !predicates::availability_matches(record, &metadata, &mut budget)? { continue; }
                                let failure = predicates::availability_failure(record, shift, &document.settings, &mut budget)?;
                                let reason = if record.availability_kind == AvailabilityKind::AvailableOnly { "outside_available_only" } else { "unavailable" };
                                let mut witness = Witness::pair(pair, record.id.as_entity_id(), "availability", reason);
                                witness.interval = failure;
                                summary.predicate(failure.is_some(), witness, &mut budget)?;
                            }
                        }
                    }
                }
            }
            WorkforceRule::Coverage { .. } => coverage::evaluate(&input, &selected, scope, &mut summary, &mut budget)?,
            WorkforceRule::NoOverlap { compatible_category_pairs, .. } => overlap(&input, &selected, scope, compatible_category_pairs, &mut summary, &mut budget)?,
            _ => {
                retain_id(&mut obligations.remaining, id, &mut budget)?;
                continue;
            }
        }
        retain_id(&mut obligations.handled, id, &mut budget)?;
        evaluations.push(summary.finish(id, &mut budget)?);
    }
    for lock in input.domain.locked_assignments.values() {
        budget.step()?;
        if matches!(lock.state, LockState::Hard {}) {
            retain_id(&mut obligations.remaining, RuleId::from_uuid(lock.id.as_uuid()), &mut budget)?;
        }
    }
    sort_work(obligations.handled.len(), &mut budget)?;
    sort_work(obligations.remaining.len(), &mut budget)?;
    sort_work(evaluations.len(), &mut budget)?;
    obligations.handled.sort_unstable();
    obligations.remaining.sort_unstable();
    evaluations.sort_unstable_by_key(|evaluation| evaluation.rule_id);
    budget.check()?;
    Ok(AssignmentRuleEvaluation { source_document_hash: input.source_document_hash, evaluations, obligations })
}

struct Selected<'a> {
    by_person: BTreeMap<PersonId, Vec<&'a ResolvedShift>>,
    by_shift: BTreeMap<ShiftId, Vec<PersonId>>,
}

impl<'a> Selected<'a> {
    fn new(input: &'a AssignmentInput, pairs: &[AssignmentPair], budget: &mut OperationBudget<'_>) -> Result<Self, AssignmentRuleError> {
        let mut selected = Self { by_person: BTreeMap::new(), by_shift: BTreeMap::new() };
        for pair in pairs {
            budget.step()?;
            let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
            if !selected.by_person.contains_key(&pair.person_id) { budget.reserve(1, 1, 16)?; }
            if !selected.by_shift.contains_key(&pair.shift_id) { budget.reserve(1, 1, 16)?; }
            budget.reserve(0, 2, 32)?;
            selected.by_person.entry(pair.person_id).or_default().push(shift);
            selected.by_shift.entry(pair.shift_id).or_default().push(pair.person_id);
        }
        for shifts in selected.by_person.values_mut() {
            budget.step()?;
            sort_work(shifts.len(), budget)?;
            shifts.sort_unstable_by_key(|shift| (shift.interval.starts_at.instant, shift.id));
        }
        for people in selected.by_shift.values_mut() {
            budget.step()?;
            sort_work(people.len(), budget)?;
            people.sort_unstable();
        }
        Ok(selected)
    }
}

// Chronological start order proves every suffix beginning at/after an interval's end is
// nonoverlapping in bulk. Only genuinely overlapping pairs require category comparisons.
fn overlap(input: &AssignmentInput, selected: &Selected<'_>, scope: &Scope, compatible: &[CategoryPair], summary: &mut Summary, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        if !predicates::person_matches(input.person(*person_id).ok_or_else(invalid)?, scope, budget)? { continue; }
        let mut scoped = Vec::new();
        for shift in shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if predicates::shift_matches(shift, &metadata, scope, budget)? {
                budget.reserve(0, 1, 16)?;
                scoped.push((*shift, metadata.assignment_type.category.as_str()));
            }
        }
        let n = count(scoped.len())?;
        let pairs = n.checked_mul(n.saturating_sub(1)).ok_or_else(arithmetic)? / 2;
        summary.checked = add(summary.checked, pairs)?;
        for (index, (left, left_category)) in scoped.iter().enumerate() {
            budget.step()?;
            for (right, right_category) in &scoped[index + 1..] {
                budget.step()?;
                if right.interval.starts_at.instant >= left.interval.ends_at.instant { break; }
                let mut exempt = false;
                for categories in compatible {
                    budget.step()?;
                    exempt |= (categories.first_category == *left_category && categories.second_category == *right_category)
                        || (categories.first_category == *right_category && categories.second_category == *left_category);
                }
                if !exempt {
                    let first = left.id.min(right.id);
                    let second = left.id.max(right.id);
                    let mut witness = Witness::pair(AssignmentPair { person_id: *person_id, shift_id: first }, first.as_entity_id(), "shift", "overlap");
                    witness.other_shift = Some(second);
                    witness.interval = predicates::interval(left).intersection(predicates::interval(right));
                    summary.failure(witness)?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Witness {
    person: Option<PersonId>,
    shift: ShiftId,
    other_shift: Option<ShiftId>,
    owner: EntityId,
    owner_kind: &'static str,
    reason: &'static str,
    minimum_rank: u64,
    minimum_hash: Option<[u8; 32]>,
    lower: Option<u64>,
    upper: Option<u64>,
    actual: Option<u64>,
    interval: Option<InstantInterval>,
}

impl Witness {
    fn pair(pair: AssignmentPair, owner: EntityId, owner_kind: &'static str, reason: &'static str) -> Self {
        Self { person: Some(pair.person_id), shift: pair.shift_id, other_shift: None, owner, owner_kind, reason, minimum_rank: 0, minimum_hash: None, lower: None, upper: None, actual: None, interval: None }
    }

    fn precedes(&self, other: &Self) -> bool {
        (self.person, self.shift, self.owner, self.reason, self.minimum_rank, self.other_shift)
            < (other.person, other.shift, other.owner, other.reason, other.minimum_rank, other.other_shift)
    }
}

#[derive(Default)]
struct Summary {
    checked: u64,
    violations: u64,
    first: Option<Witness>,
}

impl Summary {
    fn predicate(&mut self, failed: bool, witness: Witness, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
        budget.step()?;
        self.checked = add(self.checked, 1)?;
        if failed { self.failure(witness)?; }
        Ok(())
    }

    fn failure(&mut self, witness: Witness) -> Result<(), AssignmentRuleError> {
        self.violations = add(self.violations, 1)?;
        if self.first.as_ref().is_none_or(|first| witness.precedes(first)) { self.first = Some(witness); }
        Ok(())
    }

    fn finish(self, rule_id: RuleId, budget: &mut OperationBudget<'_>) -> Result<RuleEvaluation, AssignmentRuleError> {
        budget.step()?;
        let bytes = budget.measure(&(rule_id, self.violations == 0, SUMMARY))?;
        budget.reserve(1, 0, bytes)?;
        let mut result = RuleEvaluation { rule_id, satisfied: self.violations == 0, affected_entities: Vec::new(), message_key: SUMMARY.to_owned(), expected: BTreeMap::new(), observed: BTreeMap::new(), evidence: Vec::new() };
        fact(&mut result.expected, VIOLATIONS, VerificationValue::Integer(0), budget)?;
        fact(&mut result.observed, VIOLATIONS, integer(self.violations)?, budget)?;
        fact(&mut result.observed, CHECKED, integer(self.checked)?, budget)?;
        if let Some(witness) = self.first {
            text_fact(&mut result.observed, "official.workforce.fact.witness_reason", witness.reason, budget)?;
            if let Some(person) = witness.person {
                entity(&mut result.affected_entities, "person", EntityId::from_uuid(person.as_uuid()), budget)?;
            }
            entity(&mut result.affected_entities, "shift", witness.shift.as_entity_id(), budget)?;
            if let Some(shift) = witness.other_shift { entity(&mut result.affected_entities, "shift", shift.as_entity_id(), budget)?; }
            entity(&mut result.affected_entities, witness.owner_kind, witness.owner, budget)?;
            for (key, value) in [
                ("official.workforce.fact.witness_minimum", witness.lower),
                ("official.workforce.fact.witness_maximum", witness.upper),
                ("official.workforce.fact.witness_count", witness.actual),
            ] {
                if let Some(value) = value { fact(&mut result.observed, key, integer(value)?, budget)?; }
            }
            if let Some(hash) = witness.minimum_hash {
                let hash = blake3::Hash::from_bytes(hash).to_hex();
                text_fact(&mut result.observed, "official.workforce.fact.witness_minimum_key", hash.as_str(), budget)?;
            }
            if let Some(interval) = witness.interval {
                // Formatting has a fixed timestamp bound; reserve before creating these strings.
                budget.reserve(0, 2, 128)?;
                text_fact(&mut result.observed, "official.workforce.fact.witness_start", &interval.start.to_string(), budget)?;
                text_fact(&mut result.observed, "official.workforce.fact.witness_end", &interval.end.to_string(), budget)?;
            }
        }
        sort_work(result.affected_entities.len(), budget)?;
        result.affected_entities.sort_unstable();
        result.affected_entities.dedup();
        result.validate().map_err(|_| AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord))?;
        Ok(result)
    }
}

fn fact(map: &mut BTreeMap<VerificationFactId, VerificationValue>, key: &str, value: VerificationValue, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    let bytes = budget.measure(&(key, &value))?;
    budget.reserve(0, 2, bytes)?;
    map.insert(VerificationFactId::new(key).map_err(|_| invalid())?, value);
    Ok(())
}

fn text_fact(map: &mut BTreeMap<VerificationFactId, VerificationValue>, key: &str, value: &str, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    // The borrowed tuple counts all owned text before either allocation.
    let bytes = budget.measure(&(key, "text", value))?;
    budget.reserve(0, 2, bytes)?;
    map.insert(VerificationFactId::new(key).map_err(|_| invalid())?, VerificationValue::Text(value.to_owned()));
    Ok(())
}

fn entity(entities: &mut Vec<DomainEntityRef>, kind: &str, id: EntityId, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    // Fixed namespace + UUID representations, independent of submitted names or text.
    budget.reserve(0, 1, 160)?;
    entities.push(DomainEntityRef {
        kind: DomainEntityKindId::new(format!("official.workforce.{kind}")).map_err(|_| invalid())?,
        id: DomainEntityId::new(format!("official.workforce.entity.{id}")).map_err(|_| invalid())?,
    });
    Ok(())
}

fn integer(value: u64) -> Result<VerificationValue, AssignmentRuleError> {
    Ok(VerificationValue::Integer(i64::try_from(value).map_err(|_| arithmetic())?))
}

fn retain_id(ids: &mut Vec<RuleId>, id: RuleId, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    budget.reserve(0, 1, 16)?;
    ids.push(id);
    Ok(())
}

fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}

fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow)
}

fn sort_work(length: usize, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    let n = count(length)?;
    let levels = u64::from(u64::BITS - n.leading_zeros());
    budget.steps(n.checked_mul(levels).ok_or_else(arithmetic)?)
}
