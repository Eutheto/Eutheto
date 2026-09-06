//! Independent original-domain predicates. This module never consumes candidate or IR decisions.
mod coverage;
mod predicates;

use super::{
    AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleEvaluation,
    AssignmentRuleLimit, InstantInterval, RequiredRulePartition,
    budget::{MAX_SELECTED_PAIRS, OperationBudget, add, count, within},
    input::AssignmentInput,
    intervals::date_range,
};
use crate::{
    ids::ShiftId,
    model::{
        ActiveRange, AssignmentPair, Availability, AvailabilityKind, CategoryPair, LockState, Person, Scope,
        WorkforceEntity, WorkforceRule,
    },
    temporal::ResolvedShift,
};
use eutheto_domain_ir::{
    DomainEntityId, DomainEntityKindId, DomainEntityRef, RuleEvaluation, VerificationFactId,
    VerificationValue,
};
use eutheto_types::{CancellationToken, EntityId, PersonId, RuleId, ScenarioDocument, ScenarioSettings};
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
    let (evaluations, obligations) = evaluate_prepared(&input, &selected, &document.settings, &mut budget)?;
    Ok(AssignmentRuleEvaluation { source_document_hash: input.source_document_hash, evaluations, obligations })
}

// The semantic phase takes only validated original data and the same operation budget. Keeping
// preparation separate also lets private tests target cancellation after decode/indexing completes.
fn evaluate_prepared(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<(Vec<RuleEvaluation>, RequiredRulePartition), AssignmentRuleError> {
    let mut evaluations = Vec::new();
    let mut obligations = RequiredRulePartition { handled: Vec::new(), remaining: Vec::new() };
    fact_evaluations(input, selected, settings, &mut evaluations, &mut obligations.handled, budget)?;
    for rule in input.domain.rules.values() {
        budget.step()?;
        let (id, active, scope) = rule.header();
        if !active { continue; }
        let mut summary = Summary::default();
        match rule {
            WorkforceRule::Eligibility { .. } | WorkforceRule::Availability { .. } => {
                summary = pair_rule(input, selected, rule, settings, budget)?;
            }
            WorkforceRule::Coverage { .. } => {
                coverage::evaluate(input, selected, scope, &mut summary, budget)?;
            }
            WorkforceRule::NoOverlap { compatible_category_pairs, .. } => {
                overlap(input, selected, scope, compatible_category_pairs, &mut summary, budget)?;
            }
            _ => {
                retain_id(&mut obligations.remaining, id, budget)?;
                continue;
            }
        }
        retain_id(&mut obligations.handled, id, budget)?;
        evaluations.push(summary.finish(id, budget)?);
    }
    for lock in input.domain.locked_assignments.values() {
        budget.step()?;
        if matches!(lock.state, LockState::Hard {}) {
            retain_id(&mut obligations.remaining, RuleId::from_uuid(lock.id.as_uuid()), budget)?;
        }
    }
    budget.sort_work(obligations.handled.len())?;
    budget.sort_work(obligations.remaining.len())?;
    budget.sort_work(evaluations.len())?;
    obligations.handled.sort_unstable();
    obligations.remaining.sort_unstable();
    evaluations.sort_unstable_by_key(|evaluation| evaluation.rule_id);
    budget.check()?;
    Ok((evaluations, obligations))
}

fn fact_evaluations(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    settings: &ScenarioSettings,
    evaluations: &mut Vec<RuleEvaluation>,
    handled: &mut Vec<RuleId>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        let id = RuleId::from_uuid(person_id.as_uuid());
        retain_id(handled, id, budget)?;
        let shifts = selected.by_person.get(person_id).map_or(&[][..], Vec::as_slice);
        evaluations.push(activity(person, shifts, settings, budget)?.finish(id, budget)?);
    }
    for entity in input.domain.entities.values() {
        budget.step()?;
        let WorkforceEntity::Availability(record) = entity else { continue; };
        if record.availability_kind != AvailabilityKind::ApprovedTimeOff { continue; }
        let id = RuleId::from_uuid(record.id.as_entity_id().as_uuid());
        retain_id(handled, id, budget)?;
        let shifts = selected.by_person.get(&record.person_id).map_or(&[][..], Vec::as_slice);
        evaluations.push(approved_leave(input, record, shifts, settings, budget)?.finish(id, budget)?);
    }
    Ok(())
}

fn activity(
    person: &Person,
    shifts: &[&ResolvedShift],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<Summary, AssignmentRuleError> {
    let mut summary = Summary::default();
    if shifts.is_empty() { return Ok(summary); }
    let owner = EntityId::from_uuid(person.id.as_uuid());
    let allowed = match person.active_range {
        ActiveRange::Always {} => None,
        ActiveRange::DateRange(range) => Some(date_range(range, settings, owner)?),
    };
    for shift in shifts {
        budget.step()?;
        let query = predicates::interval(shift);
        let failed = allowed.is_some_and(|allowed| query.start < allowed.start || query.end > allowed.end);
        let mut witness = Witness::pair(
            AssignmentPair { person_id: person.id, shift_id: shift.id },
            owner, "person", "outside_active_range",
        );
        if let Some(allowed) = allowed {
            witness.interval = if query.start < allowed.start {
                Some(InstantInterval { start: query.start, end: query.end.min(allowed.start) })
            } else if query.end > allowed.end {
                Some(InstantInterval { start: query.start.max(allowed.end), end: query.end })
            } else { None };
        }
        summary.predicate(failed, witness, budget)?;
    }
    Ok(summary)
}

fn approved_leave(
    input: &AssignmentInput,
    record: &Availability,
    shifts: &[&ResolvedShift],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<Summary, AssignmentRuleError> {
    let mut summary = Summary::default();
    for shift in shifts {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if predicates::availability_matches(record, &metadata, budget)? {
            check_availability(record, shift, settings, &mut summary, budget)?;
        }
    }
    Ok(summary)
}

fn pair_rule(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule: &WorkforceRule,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<Summary, AssignmentRuleError> {
    let (_, _, scope) = rule.header();
    let mut summary = Summary::default();
    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !predicates::person_matches(person, scope, budget)? { continue; }
        for shift in shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !predicates::shift_matches(shift, &metadata, scope, budget)? { continue; }
            let pair = AssignmentPair { person_id: *person_id, shift_id: shift.id };
            if matches!(rule, WorkforceRule::Eligibility { .. }) {
                let allowed = predicates::type_allowed(person, &metadata, budget)?;
                summary.predicate(!allowed, predicates::pair_witness(input, pair, "assignment_type_not_allowed")?, budget)?;
                let qualified = predicates::qualified(person, &metadata.assignment_type.qualifications, predicates::interval(shift), budget)?;
                summary.predicate(!qualified, predicates::pair_witness(input, pair, "qualification_expression")?, budget)?;
            } else if let Some(records) = input.availability_by_person.get(person_id) {
                for record_id in records {
                    budget.step()?;
                    let Some(WorkforceEntity::Availability(record)) = input.domain.entities.get(&record_id.as_entity_id()) else {
                        return Err(invalid());
                    };
                    if matches!(record.availability_kind, AvailabilityKind::Unavailable | AvailabilityKind::AvailableOnly)
                        && predicates::availability_matches(record, &metadata, budget)? {
                        check_availability(record, shift, settings, &mut summary, budget)?;
                    }
                }
            }
        }
    }
    Ok(summary)
}

fn check_availability(
    record: &Availability,
    shift: &ResolvedShift,
    settings: &ScenarioSettings,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let reason = match record.availability_kind {
        AvailabilityKind::AvailableOnly => "outside_available_only",
        AvailabilityKind::Unavailable => "unavailable",
        AvailabilityKind::ApprovedTimeOff => "approved_time_off",
        AvailabilityKind::RequestedTimeOff => return Err(invalid()),
    };
    let failure = predicates::availability_failure(record, shift, settings, budget)?;
    let mut witness = Witness::pair(
        AssignmentPair { person_id: record.person_id, shift_id: shift.id },
        record.id.as_entity_id(), "availability", reason,
    );
    witness.interval = failure;
    summary.predicate(failure.is_some(), witness, budget)
}

struct Selected<'a> {
    by_person: BTreeMap<PersonId, Vec<&'a ResolvedShift>>,
    by_shift: BTreeMap<ShiftId, Vec<PersonId>>,
}

impl<'a> Selected<'a> {
    fn new(
        input: &'a AssignmentInput,
        pairs: &[AssignmentPair],
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let mut selected = Self {
            by_person: BTreeMap::new(),
            by_shift: BTreeMap::new(),
        };
        for pair in pairs {
            budget.step()?;
            let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
            if !selected.by_person.contains_key(&pair.person_id) {
                budget.reserve(1, 1, 16)?;
            }
            if !selected.by_shift.contains_key(&pair.shift_id) {
                budget.reserve(1, 1, 16)?;
            }
            budget.reserve(0, 2, 32)?;
            selected
                .by_person
                .entry(pair.person_id)
                .or_default()
                .push(shift);
            selected
                .by_shift
                .entry(pair.shift_id)
                .or_default()
                .push(pair.person_id);
        }
        for shifts in selected.by_person.values_mut() {
            budget.step()?;
            budget.sort_work(shifts.len())?;
            shifts.sort_unstable_by_key(|shift| (shift.interval.starts_at.instant, shift.id));
        }
        for people in selected.by_shift.values_mut() {
            budget.step()?;
            budget.sort_work(people.len())?;
            people.sort_unstable();
        }
        Ok(selected)
    }
}

// Chronological start order proves every suffix beginning at/after an interval's end is
// nonoverlapping in bulk. Only genuinely overlapping pairs require category comparisons.
fn overlap(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
    compatible: &[CategoryPair],
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        if !predicates::person_matches(
            input.person(*person_id).ok_or_else(invalid)?,
            scope,
            budget,
        )? {
            continue;
        }
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
                if right.interval.starts_at.instant >= left.interval.ends_at.instant {
                    break;
                }
                let mut exempt = false;
                for categories in compatible {
                    budget.step()?;
                    exempt |= (categories.first_category == *left_category
                        && categories.second_category == *right_category)
                        || (categories.first_category == *right_category
                            && categories.second_category == *left_category);
                }
                if !exempt {
                    let first = left.id.min(right.id);
                    let second = left.id.max(right.id);
                    let mut witness = Witness::pair(
                        AssignmentPair {
                            person_id: *person_id,
                            shift_id: first,
                        },
                        first.as_entity_id(),
                        "shift",
                        "overlap",
                    );
                    witness.other_shift = Some(second);
                    witness.interval =
                        predicates::interval(left).intersection(predicates::interval(right));
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
    fn pair(
        pair: AssignmentPair,
        owner: EntityId,
        owner_kind: &'static str,
        reason: &'static str,
    ) -> Self {
        Self {
            person: Some(pair.person_id),
            shift: pair.shift_id,
            other_shift: None,
            owner,
            owner_kind,
            reason,
            minimum_rank: 0,
            minimum_hash: None,
            lower: None,
            upper: None,
            actual: None,
            interval: None,
        }
    }

    fn precedes(&self, other: &Self) -> bool {
        (
            self.person,
            self.shift,
            self.owner,
            self.reason,
            self.minimum_rank,
            self.other_shift,
        ) < (
            other.person,
            other.shift,
            other.owner,
            other.reason,
            other.minimum_rank,
            other.other_shift,
        )
    }
}

#[derive(Default)]
struct Summary {
    checked: u64,
    violations: u64,
    first: Option<Witness>,
}

impl Summary {
    fn predicate(
        &mut self,
        failed: bool,
        witness: Witness,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        budget.step()?;
        self.checked = add(self.checked, 1)?;
        if failed {
            self.failure(witness)?;
        }
        Ok(())
    }

    fn failure(&mut self, witness: Witness) -> Result<(), AssignmentRuleError> {
        self.violations = add(self.violations, 1)?;
        if self
            .first
            .as_ref()
            .is_none_or(|first| witness.precedes(first))
        {
            self.first = Some(witness);
        }
        Ok(())
    }

    fn finish(
        self,
        rule_id: RuleId,
        budget: &mut OperationBudget<'_>,
    ) -> Result<RuleEvaluation, AssignmentRuleError> {
        budget.step()?;
        let bytes = budget.measure(&(rule_id, self.violations == 0, SUMMARY))?;
        budget.reserve(1, 0, bytes)?;
        let mut result = RuleEvaluation {
            rule_id,
            satisfied: self.violations == 0,
            affected_entities: Vec::new(),
            message_key: SUMMARY.to_owned(),
            expected: BTreeMap::new(),
            observed: BTreeMap::new(),
            evidence: Vec::new(),
        };
        fact(
            &mut result.expected,
            VIOLATIONS,
            VerificationValue::Integer(0),
            budget,
        )?;
        fact(
            &mut result.observed,
            VIOLATIONS,
            integer(self.violations)?,
            budget,
        )?;
        fact(
            &mut result.observed,
            CHECKED,
            integer(self.checked)?,
            budget,
        )?;
        if let Some(witness) = self.first {
            witness_evidence(witness, &mut result, budget)?;
        }
        budget.sort_work(result.affected_entities.len())?;
        budget.steps(count(result.affected_entities.len())?)?;
        result.affected_entities.sort_unstable();
        result.affected_entities.dedup();
        result
            .validate()
            .map_err(|_| AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord))?;
        Ok(result)
    }
}

fn witness_evidence(
    witness: Witness,
    result: &mut RuleEvaluation,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    text_fact(&mut result.observed, "official.workforce.fact.witness_reason", witness.reason, budget)?;
    if let Some(person) = witness.person {
        entity(&mut result.affected_entities, "person", EntityId::from_uuid(person.as_uuid()), budget)?;
    }
    entity(&mut result.affected_entities, "shift", witness.shift.as_entity_id(), budget)?;
    if let Some(shift) = witness.other_shift {
        entity(&mut result.affected_entities, "shift", shift.as_entity_id(), budget)?;
    }
    entity(&mut result.affected_entities, witness.owner_kind, witness.owner, budget)?;
    for (key, value) in [
        ("official.workforce.fact.witness_minimum", witness.lower),
        ("official.workforce.fact.witness_maximum", witness.upper),
        ("official.workforce.fact.witness_count", witness.actual),
    ] {
        budget.step()?;
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
    Ok(())
}

fn fact(
    map: &mut BTreeMap<VerificationFactId, VerificationValue>,
    key: &str,
    value: VerificationValue,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    let bytes = budget.measure(&(key, &value))?;
    budget.reserve(0, 2, bytes)?;
    map.insert(VerificationFactId::new(key).map_err(|_| invalid())?, value);
    Ok(())
}

fn text_fact(
    map: &mut BTreeMap<VerificationFactId, VerificationValue>,
    key: &str,
    value: &str,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    // The borrowed tuple counts all owned text before either allocation.
    let bytes = budget.measure(&(key, "text", value))?;
    budget.reserve(0, 2, bytes)?;
    map.insert(
        VerificationFactId::new(key).map_err(|_| invalid())?,
        VerificationValue::Text(value.to_owned()),
    );
    Ok(())
}

fn entity(
    entities: &mut Vec<DomainEntityRef>,
    kind: &str,
    id: EntityId,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    // Fixed namespace + UUID representations, independent of submitted names or text.
    budget.reserve(0, 1, 160)?;
    entities.push(DomainEntityRef {
        kind: DomainEntityKindId::new(format!("official.workforce.{kind}"))
            .map_err(|_| invalid())?,
        id: DomainEntityId::new(format!("official.workforce.{id}")).map_err(|_| invalid())?,
    });
    Ok(())
}

fn integer(value: u64) -> Result<VerificationValue, AssignmentRuleError> {
    Ok(VerificationValue::Integer(
        i64::try_from(value).map_err(|_| arithmetic())?,
    ))
}

fn retain_id(
    ids: &mut Vec<RuleId>,
    id: RuleId,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
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


#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use serde_json::json;

    mod support {
        include!("../../tests/support/mod.rs");
    }

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn document() -> TestResult<ScenarioDocument> {
        let mut document = support::fixture()?;
        document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
        document.domain.locked_assignments.clear();
        Ok(document)
    }

    fn entity_id(index: u32) -> TestResult<EntityId> {
        Ok(support::id(index).parse()?)
    }

    fn rule_id(index: u32) -> TestResult<RuleId> {
        Ok(support::id(index).parse()?)
    }

    fn selected_pair() -> TestResult<AssignmentPair> {
        Ok(AssignmentPair {
            person_id: support::id(1).parse()?,
            shift_id: support::id(8).parse()?,
        })
    }

    fn encoded<T: Serialize>(value: &T) -> TestResult<u64> {
        Ok(u64::try_from(serde_json::to_vec(value)?.len())?)
    }

    fn summary_bytes(id: RuleId, checked: i64, violations: i64) -> TestResult<u64> {
        Ok(encoded(&(id, violations == 0, SUMMARY))?
            + encoded(&(VIOLATIONS, VerificationValue::Integer(0)))?
            + encoded(&(VIOLATIONS, VerificationValue::Integer(violations)))?
            + encoded(&(CHECKED, VerificationValue::Integer(checked)))?)
    }

    fn leave_output(budget: &mut OperationBudget<'_>, left: (u64, u64, u64)) -> Result<(), AssignmentRuleError> {
        let remaining = budget.remaining_output();
        budget.reserve(remaining.0 - left.0, remaining.1 - left.1, remaining.2 - left.2)
    }

    // Exercise each real builder at all applicable exact boundaries and with one fewer slot/byte.
    // Costs are independently specified, not learned by invoking the builder under test.
    fn boundaries(
        cost: (u64, u64, u64),
        mut build: impl FnMut(&mut OperationBudget<'_>) -> Result<(), AssignmentRuleError>,
    ) -> TestResult {
        let mut exact = OperationBudget::evaluation(None);
        leave_output(&mut exact, cost)?;
        build(&mut exact)?;
        assert_eq!(exact.remaining_output(), (0, 0, 0));
        for (dimension, limit) in [
            (0, AssignmentRuleLimit::Records),
            (1, AssignmentRuleLimit::References),
            (2, AssignmentRuleLimit::Bytes),
        ] {
            let mut left = [cost.0, cost.1, cost.2];
            if left[dimension] == 0 { continue; }
            left[dimension] -= 1;
            let mut budget = OperationBudget::evaluation(None);
            leave_output(&mut budget, (left[0], left[1], left[2]))?;
            assert_eq!(build(&mut budget), Err(AssignmentRuleError::LimitExceeded(limit)));
        }
        Ok(())
    }

    #[test]
    fn scalar_and_text_facts_enforce_exact_item_and_byte_boundaries() -> TestResult {
        let scalar = VerificationValue::Integer(17);
        let cost = (0, 2, encoded(&(CHECKED, &scalar))?);
        boundaries(cost, |budget| {
            let mut facts = BTreeMap::new();
            fact(&mut facts, CHECKED, scalar.clone(), budget)?;
            assert_eq!(facts.values().next(), Some(&scalar));
            Ok(())
        })?;
        let key = "official.workforce.fact.witness_reason";
        let text = "qualification_expression";
        boundaries((0, 2, encoded(&(key, "text", text))?), |budget| {
            let mut facts = BTreeMap::new();
            text_fact(&mut facts, key, text, budget)?;
            assert_eq!(facts.values().next(), Some(&VerificationValue::Text(text.to_owned())));
            Ok(())
        })
    }

    #[test]
    fn entity_and_obligation_retention_enforce_exact_boundaries() -> TestResult {
        let person = entity_id(1)?;
        boundaries((0, 1, 160), |budget| {
            let mut entities = Vec::new();
            entity(&mut entities, "person", person, budget)?;
            assert_eq!(entities.len(), 1);
            assert_eq!(entities[0].id.as_str(), format!("official.workforce.{person}"));
            Ok(())
        })?;
        let binding = rule_id(1)?;
        boundaries((0, 1, 16), |budget| {
            let mut handled = Vec::new();
            retain_id(&mut handled, binding, budget)?;
            assert_eq!(handled, [binding]);
            Ok(())
        })
    }

    #[test]
    fn aggregate_summary_enforces_exact_record_item_and_byte_boundaries() -> TestResult {
        let id = rule_id(20)?;
        boundaries((1, 6, summary_bytes(id, 3, 0)?), |budget| {
            let result = Summary { checked: 3, violations: 0, first: None }.finish(id, budget)?;
            result.validate().map_err(|_| invalid())?;
            assert!(result.satisfied);
            assert_eq!(result.observed.len(), 2);
            Ok(())
        })
    }

    #[test]
    fn failing_summary_charges_every_nested_witness_before_retention() -> TestResult {
        let id = rule_id(20)?;
        let mut witness = Witness::pair(selected_pair()?, entity_id(30)?, "availability", "unavailable");
        witness.other_shift = Some(support::id(7).parse()?);
        witness.lower = Some(2);
        witness.upper = Some(4);
        witness.actual = Some(1);
        witness.minimum_hash = Some([0; 32]);
        let start = "2026-11-01T05:30:00Z";
        let end = "2026-11-01T07:30:00Z";
        witness.interval = Some(InstantInterval { start: start.parse()?, end: end.parse()? });
        let mut bytes = summary_bytes(id, 3, 1)? + 4 * 160 + 128;
        for (key, value) in [
            ("official.workforce.fact.witness_reason", witness.reason),
            ("official.workforce.fact.witness_minimum_key", "0000000000000000000000000000000000000000000000000000000000000000"),
            ("official.workforce.fact.witness_start", start),
            ("official.workforce.fact.witness_end", end),
        ] {
            bytes += encoded(&(key, "text", value))?;
        }
        for (key, value) in [
            ("official.workforce.fact.witness_minimum", 2),
            ("official.workforce.fact.witness_maximum", 4),
            ("official.workforce.fact.witness_count", 1),
        ] {
            bytes += encoded(&(key, VerificationValue::Integer(value)))?;
        }
        boundaries((1, 26, bytes), |budget| {
            let result = Summary { checked: 3, violations: 1, first: Some(witness) }.finish(id, budget)?;
            result.validate().map_err(|_| invalid())?;
            assert!(!result.satisfied);
            assert_eq!(result.affected_entities.len(), 4);
            assert_eq!(result.observed.len(), 9);
            Ok(())
        })
    }

    #[test]
    fn complete_semantic_phase_never_returns_an_output_truncated_at_aggregate_limits() -> TestResult {
        let mut document = document()?;
        let mut person = document.domain.entities.get(&entity_id(1)?).ok_or("person")?.clone();
        person["id"] = json!(support::id(30));
        person.as_object_mut().ok_or("person")?.remove("externalId");
        document.domain.entities.insert(entity_id(30)?, person);
        let bytes = summary_bytes(rule_id(1)?, 0, 0)? + summary_bytes(rule_id(30)?, 0, 0)? + 32;
        for (left, expected) in [
            ((2, 14, bytes), None),
            ((1, 14, bytes), Some(AssignmentRuleLimit::Records)),
            ((2, 13, bytes), Some(AssignmentRuleLimit::References)),
            ((2, 14, bytes - 1), Some(AssignmentRuleLimit::Bytes)),
        ] {
            let mut budget = OperationBudget::evaluation(None);
            let input = AssignmentInput::new(&document, &mut budget)?;
            input.validate_selection(&[], &mut budget)?;
            let selected = Selected::new(&input, &[], &mut budget)?;
            // The same cumulative operation budget continues through semantic output.
            leave_output(&mut budget, left)?;
            let result = evaluate_prepared(&input, &selected, &document.settings, &mut budget);
            if let Some(limit) = expected {
                assert_eq!(result, Err(AssignmentRuleError::LimitExceeded(limit)));
            } else {
                let (evaluations, obligations) = result?;
                assert_eq!(evaluations.len(), 2);
                assert_eq!(obligations.handled, [rule_id(1)?, rule_id(30)?]);
                assert_eq!(budget.remaining_output(), (0, 0, 0));
            }
        }
        Ok(())
    }

    #[test]
    fn genuine_token_cancels_inside_dense_qualification_leaves_with_empty_grants() -> TestResult {
        let mut document = document()?;
        let mut ids = Vec::new();
        for index in 1000..2000 {
            let id = support::id(index);
            document.domain.entities.insert(entity_id(index)?, json!({
                "kind":"qualification", "id":id, "name":"Qualification", "description":""
            }));
            ids.push(id);
        }
        document.domain.entities.get_mut(&entity_id(1)?).ok_or("person")?["qualificationGrants"] = json!([]);
        document.domain.entities.get_mut(&entity_id(4)?).ok_or("type")?["qualifications"] =
            json!({"kind":"matches","allQualificationIds":ids,"anyQualificationIds":[]});
        document.domain.rules.insert(rule_id(20)?, json!({
            "kind":"eligibility","id":support::id(20),"active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}}
        }));
        let token = CancellationToken::new();
        let mut budget = OperationBudget::evaluation(Some(&token));
        let input = AssignmentInput::new(&document, &mut budget)?;
        let selected = Selected::new(&input, &[selected_pair()?], &mut budget)?;
        let rule = input.domain.rules.get(&rule_id(20)?).ok_or("rule")?;
        let baseline = pair_rule(&input, &selected, rule, &document.settings, &mut budget)?;
        assert_eq!((baseline.checked, baseline.violations), (2, 1));
        budget.cancel_after_steps(100)?;
        assert!(matches!(
            pair_rule(&input, &selected, rule, &document.settings, &mut budget),
            Err(AssignmentRuleError::Cancelled)
        ));
        assert!(token.is_cancelled());
        Ok(())
    }

    #[test]
    fn genuine_token_cancels_after_weekly_expansion_has_begun() -> TestResult {
        let mut document = document()?;
        let windows: Vec<_> = (0..300).map(|second| json!({
            "weekdays":["sunday"], "startTime":format!("00:{:02}:{:02}", second / 60, second % 60),
            "endTime":"03:00:00", "endDayOffset":0
        })).collect();
        document.domain.entities.insert(entity_id(30)?, json!({
            "kind":"availability","id":support::id(30),"personId":support::id(1),
            "availabilityKind":"availableOnly", "source":"", "note":"",
            "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
            "timeWindow":{"kind":"weekly","windows":windows}
        }));
        let token = CancellationToken::new();
        let mut budget = OperationBudget::evaluation(Some(&token));
        let input = AssignmentInput::new(&document, &mut budget)?;
        let Some(WorkforceEntity::Availability(record)) = input.domain.entities.get(&entity_id(30)?) else {
            return Err("availability".into());
        };
        let shift = input.shift(selected_pair()?.shift_id).ok_or("shift")?;
        let mut summary = Summary::default();
        check_availability(record, shift, &document.settings, &mut summary, &mut budget)?;
        assert_eq!((summary.checked, summary.violations), (1, 0));
        let records_before = budget.remaining_output().0;
        budget.cancel_after_steps(100)?;
        assert_eq!(check_availability(record, shift, &document.settings, &mut summary, &mut budget),
            Err(AssignmentRuleError::Cancelled));
        assert!(token.is_cancelled());
        let retained_windows = records_before - budget.remaining_output().0;
        // Cancellation must occur inside expansion, not later at sorting or summary construction.
        assert!((1..300).contains(&retained_windows));
        // A failed expansion did not become a vacuously satisfied or partial predicate.
        assert_eq!((summary.checked, summary.violations), (1, 0));
        Ok(())
    }

    #[test]
    fn empty_person_lists_still_charge_every_outer_scope_element() -> TestResult {
        let document = document()?;
        let mut preparation = OperationBudget::evaluation(None);
        let input = AssignmentInput::new(&document, &mut preparation)?;
        let mut person = input.person(selected_pair()?.person_id).ok_or("person")?.clone();
        person.tags.clear();
        person.team_ids.clear();
        let tags: Vec<_> = (0..100).map(|index| format!("tag{index}")).collect();
        let team_ids: Vec<_> = (1000..1100).map(support::id).collect();
        for scope in [
            json!({"people":{"kind":"filter","allTags":tags,"anyTags":[]}}),
            json!({"people":{"kind":"filter","allTags":[],"anyTags":tags}}),
            json!({"people":{"kind":"all"},"teamIds":team_ids}),
        ] {
            let scope: Scope = serde_json::from_value(scope)?;
            let token = CancellationToken::new();
            let mut budget = OperationBudget::evaluation(Some(&token));
            budget.cancel_after_steps(32)?;
            assert_eq!(predicates::person_matches(&person, &scope, &mut budget),
                Err(AssignmentRuleError::Cancelled));
            assert!(token.is_cancelled());
        }
        Ok(())
    }

    #[test]
    fn active_empty_tag_populations_cannot_bypass_the_operation_work_ceiling() -> TestResult {
        let mut document = document()?;
        let mut prototype = document.domain.entities.get(&entity_id(1)?).ok_or("person")?.clone();
        prototype["tags"] = json!([]);
        prototype.as_object_mut().ok_or("person")?.remove("externalId");
        document.domain.entities.insert(entity_id(1)?, prototype.clone());
        let mut selected = vec![selected_pair()?];
        for index in 1000..1099 {
            let mut person = prototype.clone();
            person["id"] = json!(support::id(index));
            document.domain.entities.insert(entity_id(index)?, person);
            selected.push(AssignmentPair { person_id: support::id(index).parse()?, shift_id: selected_pair()?.shift_id });
        }
        let tags: Vec<_> = (0..3000).map(|index| format!("tag{index:04}")).collect();
        for index in 2000..2100 {
            document.domain.rules.insert(rule_id(index)?, json!({
                "id":support::id(index),"kind":"eligibility","active":true,"strength":"required",
                "scope":{"people":{"kind":"filter","allTags":tags,"anyTags":[]}}
            }));
        }
        assert_eq!(evaluate_assignment_rules(&document, &selected, None),
            Err(AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::WorkSteps)));
        Ok(())
    }
}
