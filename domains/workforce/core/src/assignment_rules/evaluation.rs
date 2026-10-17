//! Independent original-domain predicates. This module never consumes candidate or IR decisions.
mod coverage;
mod fixed;
mod maximum_consecutive;
mod maximum_count;
mod maximum_hours;
mod maximum_weekends;
mod mutual;
mod predicates;
mod rest;
mod skill_mix;
mod transition;
mod workload_quota;
use super::{
    AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleEvaluation,
    AssignmentRuleLimit, HealthDirection, HealthObservation, HealthObservationKind, HealthUnits,
    InstantInterval, RequiredRulePartition,
    budget::{MAX_SELECTED_PAIRS, OperationBudget, add, count, within},
    input::AssignmentInput,
    intervals::date_range,
};
use crate::{
    ids::{ShiftId, WorkCalendarId, WorkloadBucketId},
    model::{
        ActiveRange, AssignmentPair, Availability, AvailabilityKind, CategoryPair, LockState,
        Person, Scope, WorkforceEntity, WorkforceRule,
    },
    temporal::ResolvedShift,
};
use eutheto_domain_ir::{
    DomainEntityId, DomainEntityKindId, DomainEntityRef, RuleEvaluation, VerificationFactId,
    VerificationValue,
};
use eutheto_types::{
    CancellationToken, EntityId, PersonId, Rfc3339Timestamp, RuleId, ScenarioDocument,
    ScenarioSettings,
};
use jiff::{SignedDuration, civil::Date};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const VIOLATIONS: &str = "official.workforce.fact.violation_count";
pub(super) const CHECKED: &str = "official.workforce.fact.checked_predicate_count";
const SUMMARY: &str = "official.workforce.evaluation.summary";
const REPAIR_ASSOCIATION_KIND: &str = "official.workforce.fact.repair_association_kind";

/// One bounded failed original-domain predicate and its changed source pairs.
pub(super) struct RepairObservation {
    pub evaluation: RuleEvaluation,
    pub affected_pairs: Vec<AssignmentPair>,
}

/// Evaluates Eligibility, Availability, Coverage, `RequiredSkillMix`, `FixedAssignment`, authored
/// Hard locks, `NoOverlap`, `MinimumRest`, `TransitionTime`, `MaximumAssignmentCount` and
/// `MaximumConsecutive`, plus unconditional activity/approved-leave facts.
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
    let control = cancellation
        .cloned()
        .map(eutheto_types::OperationControl::Cancellation);
    let mut budget = OperationBudget::evaluation(control.as_ref());
    budget.check()?;
    within(
        count(selected_pairs.len())?,
        MAX_SELECTED_PAIRS,
        AssignmentRuleLimit::SelectedPairs,
    )?;
    let input = AssignmentInput::new(document, &mut budget)?;
    input.validate_selection(selected_pairs, &mut budget)?;
    let (evaluations, obligations) = evaluate_validated_selection(
        document,
        &input,
        selected_pairs,
        &document.settings,
        &mut budget,
    )?;
    Ok(AssignmentRuleEvaluation {
        source_document_hash: input.source_document_hash,
        evaluations,
        obligations,
    })
}

/// Evaluates the same original predicates while retaining bounded observations for the native
/// accepted-result health view. Normal verification callers do not use this sidecar.
pub(crate) fn evaluate_assignment_rules_with_health(
    document: &ScenarioDocument,
    selected_pairs: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<(AssignmentRuleEvaluation, Vec<HealthObservation>), AssignmentRuleError> {
    budget.check()?;
    within(
        count(selected_pairs.len())?,
        MAX_SELECTED_PAIRS,
        AssignmentRuleLimit::SelectedPairs,
    )?;
    let input = AssignmentInput::new(document, budget)?;
    input.validate_selection(selected_pairs, budget)?;
    let selected = Selected::new(&input, selected_pairs, budget)?;
    let (evaluations, obligations, health, _) = evaluate_prepared_with_health(
        document,
        &input,
        &selected,
        &document.settings,
        budget,
        true,
        false,
        None,
    )?;
    Ok((
        AssignmentRuleEvaluation {
            source_document_hash: input.source_document_hash,
            evaluations,
            obligations,
        },
        health,
    ))
}

pub(super) fn evaluate_repair_selection(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    selected_pairs: &[AssignmentPair],
    association_pairs: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<
    (
        Vec<RuleEvaluation>,
        RequiredRulePartition,
        Vec<RepairObservation>,
    ),
    AssignmentRuleError,
> {
    budget.check()?;
    input.validate_selection(selected_pairs, budget)?;
    input.validate_selection(association_pairs, budget)?;
    let selected = Selected::with_associations(input, selected_pairs, association_pairs, budget)?;
    let (evaluations, obligations, _, observations) = evaluate_prepared_with_health(
        document,
        input,
        &selected,
        &document.settings,
        budget,
        false,
        true,
        None,
    )?;
    Ok((evaluations, obligations, observations))
}

/// Reuses prepared source data after the caller has checked pair identities, uniqueness and bounds.
pub(super) fn evaluate_validated_selection(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    selected_pairs: &[AssignmentPair],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<(Vec<RuleEvaluation>, RequiredRulePartition), AssignmentRuleError> {
    let selected = Selected::new(input, selected_pairs, budget)?;
    evaluate_prepared(document, input, &selected, settings, budget)
}
/// Observes only configured workload families during the original required evaluation.
/// Disabled collection retains ordinary traversal, allocations, error order and budget charges.
pub(super) fn evaluate_validated_selection_with_outcomes(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    pairs: &[AssignmentPair],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
    outcomes: Option<&mut super::alternative_outcomes::Collector>,
) -> Result<(Vec<RuleEvaluation>, RequiredRulePartition), AssignmentRuleError> {
    let selected = Selected::new(input, pairs, budget)?;
    if outcomes.is_none() {
        return evaluate_prepared(document, input, &selected, settings, budget);
    }
    let (evaluations, obligations, _, _) = evaluate_prepared_with_health(
        document, input, &selected, settings, budget, false, false, outcomes,
    )?;
    Ok((evaluations, obligations))
}

/// Rechecks this recorded decision only; never certifies aggregate coverage, overlap, rest or
/// maximum-count summaries.
pub(super) fn evaluate_pair(
    input: &AssignmentInput,
    pair: AssignmentPair,
    enabled: bool,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<RuleEvaluation>, AssignmentRuleError> {
    let person = input.person(pair.person_id).ok_or_else(invalid)?;
    let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
    let one = [shift];
    let shifts = if enabled { &one[..] } else { &[] };
    let pairs = if enabled {
        std::slice::from_ref(&pair)
    } else {
        &[]
    };
    let selected = Selected::new(input, pairs, budget)?;
    let mut evaluations = vec![
        activity(person, shifts, settings, false, budget)?
            .finish(RuleId::from_uuid(pair.person_id.as_uuid()), budget)?,
    ];
    if let Some(records) = input.availability_by_person.get(&pair.person_id) {
        for id in records {
            budget.step()?;
            let Some(WorkforceEntity::Availability(record)) =
                input.domain.entities.get(&id.as_entity_id())
            else {
                return Err(invalid());
            };
            if record.availability_kind == AvailabilityKind::ApprovedTimeOff {
                evaluations.push(
                    approved_leave(input, record, shifts, settings, false, budget)?.finish(
                        RuleId::from_uuid(record.id.as_entity_id().as_uuid()),
                        budget,
                    )?,
                );
            }
        }
    }
    for rule in input.domain.rules.values() {
        budget.step()?;
        let (id, active, _) = rule.header();
        if active
            && matches!(
                rule,
                WorkforceRule::Eligibility { .. } | WorkforceRule::Availability { .. }
            )
        {
            evaluations.push(
                pair_rule(input, &selected, rule, settings, false, budget)?.finish(id, budget)?,
            );
        }
    }
    budget.sort_work(evaluations.len())?;
    evaluations.sort_by_key(|evaluation| evaluation.rule_id);
    Ok(evaluations)
}

// The semantic phase takes only validated original data and the same operation budget. Keeping
// preparation separate also lets private tests target cancellation after decode/indexing completes.
fn evaluate_prepared(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    selected: &Selected<'_>,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<(Vec<RuleEvaluation>, RequiredRulePartition), AssignmentRuleError> {
    let (evaluations, obligations, _, _) = evaluate_prepared_with_health(
        document, input, selected, settings, budget, false, false, None,
    )?;
    Ok((evaluations, obligations))
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::type_complexity
)]
fn evaluate_prepared_with_health(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    selected: &Selected<'_>,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
    collect_health: bool,
    collect_repair: bool,
    mut outcomes: Option<&mut super::alternative_outcomes::Collector>,
) -> Result<
    (
        Vec<RuleEvaluation>,
        RequiredRulePartition,
        Vec<HealthObservation>,
        Vec<RepairObservation>,
    ),
    AssignmentRuleError,
> {
    let mut evaluations = Vec::new();
    let mut health = Vec::new();
    let mut observations = Vec::new();
    let mut obligations = RequiredRulePartition {
        handled: Vec::new(),
        remaining: Vec::new(),
    };
    fact_evaluations(
        input,
        selected,
        settings,
        &mut evaluations,
        &mut obligations.handled,
        &mut observations,
        collect_repair,
        budget,
    )?;
    for rule in input.domain.rules.values() {
        budget.step()?;
        let (id, active, scope) = rule.header();
        if !active {
            continue;
        }
        // Health re-runs Coverage over the full source, including when no authored rule exists.
        let mut summary =
            Summary::new(collect_health && !matches!(rule, WorkforceRule::Coverage { .. }));
        if collect_repair {
            summary.enable_repair();
        }
        summary.set_rule_id(id);
        if outcomes.is_some()
            && matches!(
                rule,
                WorkforceRule::WorkloadQuota(_)
                    | WorkforceRule::MaximumHours { .. }
                    | WorkforceRule::MaximumAssignmentCount { .. }
                    | WorkforceRule::MaximumConsecutive { .. }
                    | WorkforceRule::MaximumWorkedWeekends { .. }
            )
            && let Some(outcomes) = outcomes.as_deref_mut()
        {
            summary.alternatives = Some(std::mem::take(outcomes));
        }
        match rule {
            WorkforceRule::FixedAssignment {
                person_id,
                shift_id,
                assigned,
                ..
            } => {
                retain_id(&mut obligations.handled, id, budget)?;
                let summary = fixed::fixed_assignment(
                    input,
                    selected,
                    id,
                    scope,
                    AssignmentPair {
                        person_id: *person_id,
                        shift_id: *shift_id,
                    },
                    *assigned,
                    collect_repair,
                    budget,
                )?;
                evaluations.push(finish_summary(
                    summary,
                    id,
                    selected,
                    &mut observations,
                    budget,
                )?);
                continue;
            }
            WorkforceRule::Eligibility { .. } | WorkforceRule::Availability { .. } => {
                summary = pair_rule(input, selected, rule, settings, collect_repair, budget)?;
            }
            WorkforceRule::Coverage { .. } => {
                coverage::evaluate(input, selected, scope, &mut summary, budget)?;
            }
            WorkforceRule::NoOverlap {
                compatible_category_pairs,
                ..
            } => {
                overlap(
                    input,
                    selected,
                    scope,
                    compatible_category_pairs,
                    &mut summary,
                    budget,
                )?;
            }
            WorkforceRule::MinimumRest(rule) => {
                rest::evaluate(input, selected, rule, &mut summary, budget)?;
            }
            WorkforceRule::WorkloadQuota(_) => {
                workload_quota::evaluate(document, input, selected, rule, &mut summary, budget)?;
            }
            WorkforceRule::MaximumAssignmentCount { .. } => {
                maximum_count::evaluate(document, input, selected, rule, &mut summary, budget)?;
            }
            WorkforceRule::MaximumHours { .. } => {
                maximum_hours::evaluate(document, input, selected, rule, &mut summary, budget)?;
            }
            WorkforceRule::RequiredSkillMix { .. } => {
                skill_mix::evaluate(input, selected, rule, &mut summary, budget)?;
            }
            WorkforceRule::MutualAssignmentRestriction { .. } => {
                mutual::evaluate(input, selected, rule, &mut summary, budget)?;
            }
            WorkforceRule::TransitionTime { .. } => {
                transition::evaluate(input, selected, rule, &mut summary, budget)?;
            }
            WorkforceRule::MaximumConsecutive { .. } => {
                maximum_consecutive::evaluate(input, selected, rule, &mut summary, budget)?;
            }
            WorkforceRule::MaximumWorkedWeekends { .. } => {
                maximum_weekends::evaluate(input, selected, rule, settings, &mut summary, budget)?;
            }
        }
        if collect_health && matches!(rule, WorkforceRule::Coverage { .. }) && summary.checked == 0
        {
            budget.reserve(1, 0, 192)?;
            health.push(HealthObservation {
                kind: HealthObservationKind::CoverageHeadcount,
                rule_id: Some(id),
                coverage_owner: None,
                person_id: None,
                shift_id: None,
                other_shift_id: None,
                period: None,
                units: HealthUnits::People,
                direction: HealthDirection::Minimum,
                threshold: None,
                observed: None,
                satisfied: None,
                minimum_rank: None,
                affected_pairs: Vec::new(),
                affected_pair_count: 0,
                affected_pairs_truncated: false,
            });
        }
        if let Some(mut observations) = summary.take_health() {
            health.append(&mut observations);
        }
        if let Some(outcomes) = outcomes.as_deref_mut()
            && let Some(facts) = summary.alternatives.take()
        {
            *outcomes = facts;
        }
        retain_id(&mut obligations.handled, id, budget)?;
        evaluations.push(finish_summary(
            summary,
            id,
            selected,
            &mut observations,
            budget,
        )?);
    }
    if collect_health {
        let mut coverage_summary = Summary::new(true);
        coverage::evaluate_all(input, selected, &mut coverage_summary, budget)?;
        if let Some(mut observations) = coverage_summary.take_health() {
            health.append(&mut observations);
        }
    }
    append_lock_evaluations(
        input,
        selected,
        &mut evaluations,
        &mut obligations,
        &mut observations,
        collect_repair,
        budget,
    )?;
    budget.sort_work(obligations.handled.len())?;
    budget.sort_work(obligations.remaining.len())?;
    budget.sort_work(evaluations.len())?;
    obligations.handled.sort_unstable();
    obligations.remaining.sort_unstable();
    evaluations.sort_unstable_by_key(|evaluation| evaluation.rule_id);
    budget.check()?;
    Ok((evaluations, obligations, health, observations))
}

fn append_lock_evaluations(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    evaluations: &mut Vec<RuleEvaluation>,
    obligations: &mut RequiredRulePartition,
    observations: &mut Vec<RepairObservation>,
    collect_repair: bool,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for lock in input.domain.locked_assignments.values() {
        budget.step()?;
        let lock_id = RuleId::from_uuid(lock.id.as_uuid());
        match lock.state {
            LockState::Hard {} => {
                retain_id(&mut obligations.handled, lock_id, budget)?;
                let summary = fixed::hard_lock(
                    selected,
                    lock_id,
                    lock.person_id,
                    lock.shift_id,
                    collect_repair,
                    budget,
                )?;
                evaluations.push(finish_summary(
                    summary,
                    lock_id,
                    selected,
                    observations,
                    budget,
                )?);
            }
            LockState::Soft { .. } | LockState::Unlocked {} => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn fact_evaluations(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    settings: &ScenarioSettings,
    evaluations: &mut Vec<RuleEvaluation>,
    handled: &mut Vec<RuleId>,
    observations: &mut Vec<RepairObservation>,
    collect_repair: bool,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        let id = RuleId::from_uuid(person_id.as_uuid());
        retain_id(handled, id, budget)?;
        let shifts = selected
            .by_person
            .get(person_id)
            .map_or(&[][..], Vec::as_slice);
        let summary = activity(person, shifts, settings, collect_repair, budget)?;
        evaluations.push(finish_summary(summary, id, selected, observations, budget)?);
    }
    for entity in input.domain.entities.values() {
        budget.step()?;
        let WorkforceEntity::Availability(record) = entity else {
            continue;
        };
        if record.availability_kind != AvailabilityKind::ApprovedTimeOff {
            continue;
        }
        let id = RuleId::from_uuid(record.id.as_entity_id().as_uuid());
        retain_id(handled, id, budget)?;
        let shifts = selected
            .by_person
            .get(&record.person_id)
            .map_or(&[][..], Vec::as_slice);
        let summary = approved_leave(input, record, shifts, settings, collect_repair, budget)?;
        evaluations.push(finish_summary(summary, id, selected, observations, budget)?);
    }
    Ok(())
}

fn activity(
    person: &Person,
    shifts: &[&ResolvedShift],
    settings: &ScenarioSettings,
    collect_repair: bool,
    budget: &mut OperationBudget<'_>,
) -> Result<Summary, AssignmentRuleError> {
    let mut summary = if collect_repair {
        Summary::repair()
    } else {
        Summary::default()
    };
    if shifts.is_empty() {
        return Ok(summary);
    }
    let owner = EntityId::from_uuid(person.id.as_uuid());
    let allowed = match person.active_range {
        ActiveRange::Always {} => None,
        ActiveRange::DateRange(range) => Some(date_range(range, settings, owner)?),
    };
    for shift in shifts {
        budget.step()?;
        let query = predicates::interval(shift);
        let failed =
            allowed.is_some_and(|allowed| query.start < allowed.start || query.end > allowed.end);
        let mut witness = Witness::pair(
            AssignmentPair {
                person_id: person.id,
                shift_id: shift.id,
            },
            owner,
            "person",
            "outside_active_range",
        );
        if let Some(allowed) = allowed {
            witness.interval = if query.start < allowed.start {
                Some(InstantInterval {
                    start: query.start,
                    end: query.end.min(allowed.start),
                })
            } else if query.end > allowed.end {
                Some(InstantInterval {
                    start: query.start.max(allowed.end),
                    end: query.end,
                })
            } else {
                None
            };
        }
        summary.predicate(failed, &witness, budget)?;
    }
    Ok(summary)
}

fn approved_leave(
    input: &AssignmentInput,
    record: &Availability,
    shifts: &[&ResolvedShift],
    settings: &ScenarioSettings,
    collect_repair: bool,
    budget: &mut OperationBudget<'_>,
) -> Result<Summary, AssignmentRuleError> {
    let mut summary = if collect_repair {
        Summary::repair()
    } else {
        Summary::default()
    };
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
    collect_repair: bool,
    budget: &mut OperationBudget<'_>,
) -> Result<Summary, AssignmentRuleError> {
    let (_, _, scope) = rule.header();
    let mut summary = if collect_repair {
        Summary::repair()
    } else {
        Summary::default()
    };
    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !predicates::person_matches(person, scope, budget)? {
            continue;
        }
        for shift in shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !predicates::shift_matches(shift, &metadata, scope, budget)? {
                continue;
            }
            let pair = AssignmentPair {
                person_id: *person_id,
                shift_id: shift.id,
            };
            if matches!(rule, WorkforceRule::Eligibility { .. }) {
                let allowed = predicates::type_allowed(person, &metadata, budget)?;
                summary.predicate(
                    !allowed,
                    &predicates::pair_witness(input, pair, "assignment_type_not_allowed")?,
                    budget,
                )?;
                let qualified = predicates::qualified(
                    person,
                    &metadata.assignment_type.qualifications,
                    predicates::interval(shift),
                    budget,
                )?;
                summary.predicate(
                    !qualified,
                    &predicates::pair_witness(input, pair, "qualification_expression")?,
                    budget,
                )?;
            } else if let Some(records) = input.availability_by_person.get(person_id) {
                for record_id in records {
                    budget.step()?;
                    let Some(WorkforceEntity::Availability(record)) =
                        input.domain.entities.get(&record_id.as_entity_id())
                    else {
                        return Err(invalid());
                    };
                    if matches!(
                        record.availability_kind,
                        AvailabilityKind::Unavailable | AvailabilityKind::AvailableOnly
                    ) && predicates::availability_matches(record, &metadata, budget)?
                    {
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
        AssignmentPair {
            person_id: record.person_id,
            shift_id: shift.id,
        },
        record.id.as_entity_id(),
        "availability",
        reason,
    );
    witness.interval = failure;
    summary.predicate(failure.is_some(), &witness, budget)
}

struct Selected<'a> {
    by_person: BTreeMap<PersonId, Vec<&'a ResolvedShift>>,
    by_shift: BTreeMap<ShiftId, Vec<PersonId>>,
    associations: Option<BTreeSet<AssignmentPair>>,
}

impl<'a> Selected<'a> {
    fn contains(
        &self,
        pair: AssignmentPair,
        budget: &mut OperationBudget<'_>,
    ) -> Result<bool, AssignmentRuleError> {
        budget.step()?;
        let Some(people) = self.by_shift.get(&pair.shift_id) else {
            return Ok(false);
        };
        budget.steps(u64::from(usize::BITS - people.len().leading_zeros()))?;
        Ok(people.binary_search(&pair.person_id).is_ok())
    }

    fn new(
        input: &'a AssignmentInput,
        pairs: &[AssignmentPair],
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let mut selected = Self {
            by_person: BTreeMap::new(),
            by_shift: BTreeMap::new(),
            associations: None,
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

    fn with_associations(
        input: &'a AssignmentInput,
        pairs: &[AssignmentPair],
        associations: &[AssignmentPair],
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let mut selected = Self::new(input, pairs, budget)?;
        let mut values = BTreeSet::new();
        for pair in associations {
            budget.step()?;
            budget.reserve(0, 1, 48)?;
            values.insert(*pair);
        }
        selected.associations = Some(values);
        Ok(selected)
    }

    fn associations(&self) -> Option<&BTreeSet<AssignmentPair>> {
        self.associations.as_ref()
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
                    summary.failure(&witness, &witness, None, "direct", budget)?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Witness {
    person: Option<PersonId>,
    other_person: Option<PersonId>,
    shift: Option<ShiftId>,
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
    period: Option<(Rfc3339Timestamp, Rfc3339Timestamp)>,
    consecutive_start_date: Option<Date>,
    consecutive_end_date: Option<Date>,
    rest: Option<RestWitness>,
    transition: Option<TransitionWitness>,
    hours: Option<HoursWitness>,
    quota: Option<QuotaWitness>,
    quota_count: Option<QuotaCountWitness>,
}

#[derive(Clone, Copy)]
struct HoursWitness {
    maximum_minutes: u32,
    actual: SignedDuration,
    bucket: WorkloadBucketId,
    calendar: Option<WorkCalendarId>,
}

#[derive(Clone, Copy)]
struct QuotaWitness {
    minimum_minutes: u32,
    maximum_minutes: Option<u32>,
    actual: SignedDuration,
    bucket: WorkloadBucketId,
    calendar: WorkCalendarId,
}

#[derive(Clone, Copy)]
struct QuotaCountWitness {
    minimum: u32,
    maximum: Option<u32>,
    actual: u64,
    calendar: WorkCalendarId,
}
#[derive(Clone, Copy)]
struct RestWitness {
    minimum_minutes: u32,
    actual: SignedDuration,
}

#[derive(Clone, Copy)]
struct TransitionWitness {
    source_location: crate::ids::LocationId,
    target_location: crate::ids::LocationId,
    required_minutes: u32,
    actual: SignedDuration,
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
            other_person: None,
            shift: Some(pair.shift_id),
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
            period: None,
            consecutive_start_date: None,
            consecutive_end_date: None,
            rest: None,
            transition: None,
            hours: None,
            quota: None,
            quota_count: None,
        }
    }

    fn precedes(&self, other: &Self) -> bool {
        (
            self.person,
            self.other_person,
            self.period,
            self.shift,
            self.owner,
            self.reason,
            self.minimum_rank,
            self.other_shift,
        ) < (
            other.person,
            other.other_person,
            other.period,
            other.shift,
            other.owner,
            other.reason,
            other.minimum_rank,
            other.other_shift,
        )
    }
}

const MAX_HEALTH_AFFECTED_PAIRS: usize = 32;

struct RepairFailure {
    witness: Witness,
    rule_id: Option<RuleId>,
    association_kind: &'static str,
    members: Vec<AssignmentPair>,
}

fn exact_alternative_duration(duration: SignedDuration) -> Result<i128, AssignmentRuleError> {
    i128::from(duration.as_secs())
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i128::from(duration.subsec_nanos())))
        .ok_or_else(arithmetic)
}
#[derive(Default)]
struct Summary {
    checked: u64,
    violations: u64,
    first: Option<Witness>,
    source_rule: Option<RuleId>,
    health: Option<Vec<HealthObservation>>,
    repair: Option<Vec<RepairFailure>>,
    alternatives: Option<super::alternative_outcomes::Collector>,
}

impl Summary {
    fn new(collect_health: bool) -> Self {
        Self {
            health: collect_health.then(Vec::new),
            ..Self::default()
        }
    }

    fn repair() -> Self {
        Self {
            repair: Some(Vec::new()),
            ..Self::default()
        }
    }

    fn enable_repair(&mut self) {
        self.repair = Some(Vec::new());
    }

    fn collecting_repair(&self) -> bool {
        self.repair.is_some()
    }

    fn take_repair(&mut self) -> Option<Vec<RepairFailure>> {
        self.repair.take()
    }

    fn set_rule_id(&mut self, rule_id: RuleId) {
        self.source_rule = Some(rule_id);
    }

    fn collecting_health(&self) -> bool {
        self.health.is_some()
    }

    fn take_health(&mut self) -> Option<Vec<HealthObservation>> {
        self.health.take()
    }

    fn record_health(
        &mut self,
        mut observation: HealthObservation,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        let Some(health) = self.health.as_mut() else {
            return Ok(());
        };
        observation.rule_id = observation.rule_id.or(self.source_rule);
        if observation.affected_pairs.len() > MAX_HEALTH_AFFECTED_PAIRS {
            observation
                .affected_pairs
                .truncate(MAX_HEALTH_AFFECTED_PAIRS);
            observation.affected_pairs_truncated = true;
        }
        let items = count(observation.affected_pairs.len())?;
        let bytes = 192_u64
            .checked_add(items.checked_mul(48).ok_or_else(arithmetic)?)
            .ok_or_else(arithmetic)?;
        budget.reserve(1, items, bytes)?;
        health.push(observation);
        Ok(())
    }

    fn record_alternative(
        &mut self,
        witness: &Witness,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        use super::alternative_outcomes::{Family, Key};
        let Some(outcomes) = self.alternatives.as_mut() else {
            return Ok(());
        };
        let person = witness.person.ok_or_else(invalid)?;
        let rule = self.source_rule.ok_or_else(invalid)?;
        let (family, actual, maximum, period) = if let Some(hours) = witness.hours {
            (
                Family::MaximumHours,
                exact_alternative_duration(hours.actual)?,
                hours.calendar.is_none(),
                hours.calendar.and(witness.period),
            )
        } else if let Some(quota) = witness.quota {
            (
                Family::Quota,
                exact_alternative_duration(quota.actual)?,
                false,
                witness.period,
            )
        } else if let Some(quota) = witness.quota_count {
            (
                Family::Quota,
                i128::from(quota.actual),
                false,
                witness.period,
            )
        } else {
            let (family, maximum) = match witness.reason {
                "maximum_assignment_count" => (Family::MaximumCount, false),
                "maximum_consecutive" => (Family::MaximumConsecutive, true),
                "maximum_worked_weekends" => (Family::MaximumWeekends, true),
                _ => return Err(invalid()),
            };
            (
                family,
                i128::from(witness.actual.ok_or_else(invalid)?),
                maximum,
                if maximum { None } else { witness.period },
            )
        };
        let mut key = Key::person(family, rule, person);
        key.period = period;
        outcomes.amount(key, actual, maximum, budget)
    }
}

impl Summary {
    fn predicate(
        &mut self,
        failed: bool,
        witness: &Witness,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        self.predicate_with_members(failed, witness, None, "direct", budget)
    }

    fn predicate_with_members(
        &mut self,
        failed: bool,
        witness: &Witness,
        members: Option<Vec<AssignmentPair>>,
        association_kind: &'static str,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        self.predicate_with_observation(failed, witness, witness, members, association_kind, budget)
    }

    fn predicate_with_observation(
        &mut self,
        failed: bool,
        witness: &Witness,
        observation_witness: &Witness,
        members: Option<Vec<AssignmentPair>>,
        association_kind: &'static str,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        budget.step()?;
        self.record_alternative(witness, budget)?;
        self.checked = add(self.checked, 1)?;
        if failed {
            self.failure(
                witness,
                observation_witness,
                members,
                association_kind,
                budget,
            )?;
        }
        Ok(())
    }

    fn failure(
        &mut self,
        witness: &Witness,
        observation_witness: &Witness,
        members: Option<Vec<AssignmentPair>>,
        association_kind: &'static str,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        self.violations = add(self.violations, 1)?;
        if self
            .first
            .as_ref()
            .is_none_or(|first| witness.precedes(first))
        {
            self.first = Some(*witness);
        }
        if let Some(repair) = self.repair.as_mut() {
            let members = if let Some(members) = members {
                members
            } else {
                let mut members = Vec::new();
                if let (Some(person_id), Some(shift_id)) = (witness.person, witness.shift) {
                    budget.reserve(0, 1, 48)?;
                    members.push(AssignmentPair {
                        person_id,
                        shift_id,
                    });
                }
                if let Some(shift_id) = witness.other_shift
                    && let Some(person_id) = witness.other_person.or(witness.person)
                {
                    budget.reserve(0, 1, 48)?;
                    members.push(AssignmentPair {
                        person_id,
                        shift_id,
                    });
                }
                members
            };
            budget.reserve(1, 0, count(size_of::<RepairFailure>())?)?;
            repair.push(RepairFailure {
                witness: *observation_witness,
                rule_id: self.source_rule,
                association_kind,
                members,
            });
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
        if let Some(witness) = &self.first {
            witness_evidence(witness, &mut result, budget)?;
        }
        budget.sort_work(result.affected_entities.len())?;
        budget.steps(count(result.affected_entities.len())?)?;
        result.affected_entities.sort_unstable();
        result.affected_entities.dedup();
        result.validate().map_err(|error| match error {
            eutheto_domain_ir::DomainContractError::LimitExceeded(_) => {
                AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord)
            }
            _ => invalid(),
        })?;
        Ok(result)
    }
}

fn finish_summary(
    mut summary: Summary,
    rule_id: RuleId,
    selected: &Selected<'_>,
    observations: &mut Vec<RepairObservation>,
    budget: &mut OperationBudget<'_>,
) -> Result<RuleEvaluation, AssignmentRuleError> {
    let repair = summary.take_repair();
    let evaluation = summary.finish(rule_id, budget)?;
    if let Some(repair) = repair {
        append_repair_failures(repair, Some(rule_id), selected, observations, budget)?;
    }
    Ok(evaluation)
}

fn append_repair_failures(
    failures: Vec<RepairFailure>,
    fallback_rule: Option<RuleId>,
    selected: &Selected<'_>,
    observations: &mut Vec<RepairObservation>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let associations = selected.associations();
    for failure in failures {
        budget.step()?;
        let rule_id = failure
            .rule_id
            .or(fallback_rule)
            .unwrap_or_else(|| RuleId::from_uuid(failure.witness.owner.as_uuid()));
        let mut evaluation = Summary {
            checked: 1,
            violations: 1,
            first: Some(failure.witness),
            ..Summary::default()
        }
        .finish(rule_id, budget)?;
        text_fact(
            &mut evaluation.observed,
            REPAIR_ASSOCIATION_KIND,
            failure.association_kind,
            budget,
        )?;
        budget.step()?;
        evaluation.validate().map_err(|error| match error {
            eutheto_domain_ir::DomainContractError::LimitExceeded(_) => {
                AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord)
            }
            _ => invalid(),
        })?;
        let mut affected_pairs = Vec::new();
        if let Some(associations) = associations {
            for pair in failure.members {
                budget.step()?;
                if associations.contains(&pair) {
                    budget.reserve(0, 1, 48)?;
                    affected_pairs.push(pair);
                }
            }
        }
        budget.sort_work(affected_pairs.len())?;
        affected_pairs.sort_unstable();
        affected_pairs.dedup();
        budget.reserve(1, 0, count(size_of::<RepairObservation>())?)?;
        observations.push(RepairObservation {
            evaluation,
            affected_pairs,
        });
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn witness_evidence(
    witness: &Witness,
    result: &mut RuleEvaluation,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    text_fact(
        &mut result.observed,
        "official.workforce.fact.witness_reason",
        witness.reason,
        budget,
    )?;
    if let Some(person) = witness.person {
        entity(
            &mut result.affected_entities,
            "person",
            EntityId::from_uuid(person.as_uuid()),
            budget,
        )?;
    }
    if let Some(person) = witness.other_person {
        entity(
            &mut result.affected_entities,
            "person",
            EntityId::from_uuid(person.as_uuid()),
            budget,
        )?;
    }
    if let Some(shift) = witness.shift {
        entity(
            &mut result.affected_entities,
            "shift",
            shift.as_entity_id(),
            budget,
        )?;
    }
    if let Some(shift) = witness.other_shift {
        entity(
            &mut result.affected_entities,
            "shift",
            shift.as_entity_id(),
            budget,
        )?;
    }
    entity(
        &mut result.affected_entities,
        witness.owner_kind,
        witness.owner,
        budget,
    )?;
    if let Some(transition) = witness.transition {
        transition_evidence(witness, transition, result, budget)?;
    }
    if let Some(hours) = witness.hours {
        hours_evidence(hours, result, budget)?;
    }
    if let Some(quota) = witness.quota {
        quota_evidence(witness, quota, result, budget)?;
    }
    if let Some(quota) = witness.quota_count {
        quota_count_evidence(witness, quota, result, budget)?;
    }
    if witness.other_person.is_some() {
        mutual_pair_evidence(witness, result, budget)?;
    }
    for (key, value) in [
        ("official.workforce.fact.witness_minimum", witness.lower),
        ("official.workforce.fact.witness_maximum", witness.upper),
        ("official.workforce.fact.witness_count", witness.actual),
    ] {
        budget.step()?;
        if let Some(value) = value {
            fact(&mut result.observed, key, integer(value)?, budget)?;
        }
    }
    if let Some(rest) = witness.rest {
        rest_evidence(witness, rest, result, budget)?;
    }
    if let Some(hash) = witness.minimum_hash {
        let hash = blake3::Hash::from_bytes(hash).to_hex();
        text_fact(
            &mut result.observed,
            "official.workforce.fact.witness_minimum_key",
            hash.as_str(),
            budget,
        )?;
    }
    if let Some(interval) = witness.interval {
        // Formatting has a fixed timestamp bound; reserve before creating these strings.
        budget.reserve(0, 2, 128)?;
        text_fact(
            &mut result.observed,
            "official.workforce.fact.witness_start",
            &interval.start.to_string(),
            budget,
        )?;
        text_fact(
            &mut result.observed,
            "official.workforce.fact.witness_end",
            &interval.end.to_string(),
            budget,
        )?;
    }
    if let (Some(start), Some(end)) = (witness.consecutive_start_date, witness.consecutive_end_date)
    {
        budget.reserve(0, 2, 128)?;
        text_fact(
            &mut result.observed,
            "official.workforce.fact.consecutive_run_start_date",
            &start.to_string(),
            budget,
        )?;
        text_fact(
            &mut result.observed,
            "official.workforce.fact.consecutive_run_end_date",
            &end.to_string(),
            budget,
        )?;
    }
    Ok(())
}

fn hours_evidence(
    hours: HoursWitness,
    result: &mut RuleEvaluation,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    fact(
        &mut result.expected,
        "official.workforce.fact.maximum_hours_minutes",
        VerificationValue::Integer(i64::from(hours.maximum_minutes)),
        budget,
    )?;
    fact(
        &mut result.observed,
        "official.workforce.fact.actual_hours_seconds",
        VerificationValue::Integer(hours.actual.as_secs()),
        budget,
    )?;
    fact(
        &mut result.observed,
        "official.workforce.fact.actual_hours_subsecond_nanoseconds",
        VerificationValue::Integer(i64::from(hours.actual.subsec_nanos())),
        budget,
    )?;
    entity(
        &mut result.affected_entities,
        "workload_bucket",
        hours.bucket.as_entity_id(),
        budget,
    )?;
    if let Some(calendar) = hours.calendar {
        entity(
            &mut result.affected_entities,
            "calendar",
            calendar.as_entity_id(),
            budget,
        )?;
    }
    Ok(())
}

fn quota_evidence(
    witness: &Witness,
    quota: QuotaWitness,
    result: &mut RuleEvaluation,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    fact(
        &mut result.expected,
        "official.workforce.fact.workload_quota_minimum_minutes",
        VerificationValue::Integer(i64::from(quota.minimum_minutes)),
        budget,
    )?;
    if let Some(maximum) = quota.maximum_minutes {
        fact(
            &mut result.expected,
            "official.workforce.fact.workload_quota_maximum_minutes",
            VerificationValue::Integer(i64::from(maximum)),
            budget,
        )?;
    }
    fact(
        &mut result.observed,
        "official.workforce.fact.workload_quota_actual_seconds",
        VerificationValue::Integer(quota.actual.as_secs()),
        budget,
    )?;
    fact(
        &mut result.observed,
        "official.workforce.fact.workload_quota_actual_subsecond_nanoseconds",
        VerificationValue::Integer(i64::from(quota.actual.subsec_nanos())),
        budget,
    )?;
    entity(
        &mut result.affected_entities,
        "workload_bucket",
        quota.bucket.as_entity_id(),
        budget,
    )?;
    entity(
        &mut result.affected_entities,
        "calendar",
        quota.calendar.as_entity_id(),
        budget,
    )?;
    if let Some((start, end)) = witness.period {
        budget.reserve(0, 2, 128)?;
        text_fact(
            &mut result.observed,
            "official.workforce.fact.workload_quota_period_start",
            &start.to_string(),
            budget,
        )?;
        text_fact(
            &mut result.observed,
            "official.workforce.fact.workload_quota_period_end",
            &end.to_string(),
            budget,
        )?;
    }
    Ok(())
}

fn quota_count_evidence(
    witness: &Witness,
    quota: QuotaCountWitness,
    result: &mut RuleEvaluation,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    fact(
        &mut result.expected,
        "official.workforce.fact.workload_quota_minimum_assignments",
        VerificationValue::Integer(i64::from(quota.minimum)),
        budget,
    )?;
    if let Some(maximum) = quota.maximum {
        fact(
            &mut result.expected,
            "official.workforce.fact.workload_quota_maximum_assignments",
            VerificationValue::Integer(i64::from(maximum)),
            budget,
        )?;
    }
    fact(
        &mut result.observed,
        "official.workforce.fact.workload_quota_actual_assignments",
        integer(quota.actual)?,
        budget,
    )?;
    entity(
        &mut result.affected_entities,
        "calendar",
        quota.calendar.as_entity_id(),
        budget,
    )?;
    if let Some((start, end)) = witness.period {
        budget.reserve(0, 2, 128)?;
        text_fact(
            &mut result.observed,
            "official.workforce.fact.workload_quota_period_start",
            &start.to_string(),
            budget,
        )?;
        text_fact(
            &mut result.observed,
            "official.workforce.fact.workload_quota_period_end",
            &end.to_string(),
            budget,
        )?;
    }
    Ok(())
}

fn transition_evidence(
    witness: &Witness,
    transition: TransitionWitness,
    result: &mut RuleEvaluation,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let target_shift = witness.other_shift.ok_or_else(invalid)?;
    for (key, kind, id) in [
        (
            "official.workforce.fact.transition_source_shift",
            "shift",
            witness.shift.ok_or_else(invalid)?.as_entity_id(),
        ),
        (
            "official.workforce.fact.transition_target_shift",
            "shift",
            target_shift.as_entity_id(),
        ),
        (
            "official.workforce.fact.transition_source_location",
            "location",
            transition.source_location.as_entity_id(),
        ),
        (
            "official.workforce.fact.transition_target_location",
            "location",
            transition.target_location.as_entity_id(),
        ),
    ] {
        let reference = entity_ref(kind, id, budget)?;
        if kind == "location" {
            entity(&mut result.affected_entities, kind, id, budget)?;
        }
        fact(
            &mut result.observed,
            key,
            VerificationValue::Entity(reference),
            budget,
        )?;
    }
    fact(
        &mut result.expected,
        "official.workforce.fact.required_transition_minutes",
        VerificationValue::Integer(i64::from(transition.required_minutes)),
        budget,
    )?;
    for (key, value) in [
        (
            "official.workforce.fact.actual_transition_seconds",
            transition.actual.as_secs(),
        ),
        (
            "official.workforce.fact.actual_transition_subsecond_nanoseconds",
            i64::from(transition.actual.subsec_nanos()),
        ),
    ] {
        fact(
            &mut result.observed,
            key,
            VerificationValue::Integer(value),
            budget,
        )?;
    }
    Ok(())
}

fn mutual_pair_evidence(
    witness: &Witness,
    result: &mut RuleEvaluation,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let first_person = witness.person.ok_or_else(invalid)?;
    let second_person = witness.other_person.ok_or_else(invalid)?;
    let second_shift = witness.other_shift.ok_or_else(invalid)?;
    for (key, kind, id) in [
        (
            "official.workforce.fact.witness_first_person",
            "person",
            EntityId::from_uuid(first_person.as_uuid()),
        ),
        (
            "official.workforce.fact.witness_first_shift",
            "shift",
            witness.shift.ok_or_else(invalid)?.as_entity_id(),
        ),
        (
            "official.workforce.fact.witness_second_person",
            "person",
            EntityId::from_uuid(second_person.as_uuid()),
        ),
        (
            "official.workforce.fact.witness_second_shift",
            "shift",
            second_shift.as_entity_id(),
        ),
    ] {
        let entity = entity_ref(kind, id, budget)?;
        fact(
            &mut result.observed,
            key,
            VerificationValue::Entity(entity),
            budget,
        )?;
    }
    Ok(())
}

fn rest_evidence(
    witness: &Witness,
    rest: RestWitness,
    result: &mut RuleEvaluation,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for (key, shift) in [
        (
            "official.workforce.fact.rest_source_shift",
            witness.shift.ok_or_else(invalid)?,
        ),
        (
            "official.workforce.fact.rest_target_shift",
            witness.other_shift.ok_or_else(invalid)?,
        ),
    ] {
        let entity = entity_ref("shift", shift.as_entity_id(), budget)?;
        fact(
            &mut result.observed,
            key,
            VerificationValue::Entity(entity),
            budget,
        )?;
    }
    fact(
        &mut result.expected,
        "official.workforce.fact.required_rest_minutes",
        VerificationValue::Integer(i64::from(rest.minimum_minutes)),
        budget,
    )?;
    for (key, value) in [
        (
            "official.workforce.fact.actual_rest_seconds",
            rest.actual.as_secs(),
        ),
        (
            "official.workforce.fact.actual_rest_subsecond_nanoseconds",
            i64::from(rest.actual.subsec_nanos()),
        ),
    ] {
        fact(
            &mut result.observed,
            key,
            VerificationValue::Integer(value),
            budget,
        )?;
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
    entities.push(entity_ref(kind, id, budget)?);
    Ok(())
}

fn entity_ref(
    kind: &str,
    id: EntityId,
    budget: &mut OperationBudget<'_>,
) -> Result<DomainEntityRef, AssignmentRuleError> {
    budget.step()?;
    // Fixed namespace + UUID representations, independent of submitted names or text.
    budget.reserve(0, 1, 160)?;
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new(format!("official.workforce.{kind}"))
            .map_err(|_| invalid())?,
        id: DomainEntityId::new(format!("official.workforce.{id}")).map_err(|_| invalid())?,
    })
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

    use crate::test_support as support;

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
    fn pair(person: u32, shift: u32) -> TestResult<AssignmentPair> {
        Ok(AssignmentPair {
            person_id: support::id(person).parse()?,
            shift_id: support::id(shift).parse()?,
        })
    }

    fn repair_selection(
        document: &ScenarioDocument,
        selected: &[AssignmentPair],
        associations: &[AssignmentPair],
    ) -> TestResult<(
        Vec<RuleEvaluation>,
        RequiredRulePartition,
        Vec<RepairObservation>,
    )> {
        let mut budget = OperationBudget::evaluation(None);
        let input = AssignmentInput::new(document, &mut budget)?;
        Ok(evaluate_repair_selection(
            document,
            &input,
            selected,
            associations,
            &mut budget,
        )?)
    }

    fn observed_text<'a>(evaluation: &'a RuleEvaluation, key: &str) -> TestResult<&'a str> {
        match evaluation
            .observed
            .get(&VerificationFactId::new(key)?)
            .ok_or("missing observed text fact")?
        {
            VerificationValue::Text(value) => Ok(value),
            _ => Err("observed fact is not text".into()),
        }
    }

    fn add_shift_instance(value: &mut serde_json::Value, index: u32, day: u32) {
        let date = format!("2026-11-{day:02}");
        let mut shift = value["domain"]["entities"][support::id(8)].clone();
        shift["id"] = json!(support::id(index));
        shift["startsAt"] = json!({
            "instant": format!("{date}T06:30:00Z"),
            "local": format!("{date}T01:30:00"),
            "offsetSeconds": -18_000
        });
        shift["endsAt"] = json!({
            "instant": format!("{date}T07:30:00Z"),
            "local": format!("{date}T02:30:00"),
            "offsetSeconds": -18_000
        });
        value["domain"]["entities"][support::id(index)] = shift;
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

    fn leave_output(
        budget: &mut OperationBudget<'_>,
        left: (u64, u64, u64),
    ) -> Result<(), AssignmentRuleError> {
        let remaining = budget.remaining_output();
        budget.reserve(
            remaining.0 - left.0,
            remaining.1 - left.1,
            remaining.2 - left.2,
        )
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
            if left[dimension] == 0 {
                continue;
            }
            left[dimension] -= 1;
            let mut budget = OperationBudget::evaluation(None);
            leave_output(&mut budget, (left[0], left[1], left[2]))?;
            assert_eq!(
                build(&mut budget),
                Err(AssignmentRuleError::LimitExceeded(limit))
            );
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
            assert_eq!(
                facts.values().next(),
                Some(&VerificationValue::Text(text.to_owned()))
            );
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
            assert_eq!(
                entities[0].id.as_str(),
                format!("official.workforce.{person}")
            );
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
            let result = Summary {
                checked: 3,
                violations: 0,
                first: None,
                ..Summary::default()
            }
            .finish(id, budget)?;
            result.validate().map_err(|_| invalid())?;
            assert!(result.satisfied);
            assert_eq!(result.observed.len(), 2);
            Ok(())
        })
    }

    #[test]
    fn failing_summary_charges_every_nested_witness_before_retention() -> TestResult {
        let id = rule_id(20)?;
        let mut witness = Witness::pair(
            selected_pair()?,
            entity_id(30)?,
            "availability",
            "unavailable",
        );
        witness.other_shift = Some(support::id(7).parse()?);
        witness.lower = Some(2);
        witness.upper = Some(4);
        witness.actual = Some(1);
        witness.minimum_hash = Some([0; 32]);
        let start = "2026-11-01T05:30:00Z";
        let end = "2026-11-01T07:30:00Z";
        witness.interval = Some(InstantInterval {
            start: start.parse()?,
            end: end.parse()?,
        });
        let mut bytes = summary_bytes(id, 3, 1)? + 4 * 160 + 128;
        for (key, value) in [
            ("official.workforce.fact.witness_reason", witness.reason),
            (
                "official.workforce.fact.witness_minimum_key",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
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
            let result = Summary {
                checked: 3,
                violations: 1,
                first: Some(witness),
                ..Summary::default()
            }
            .finish(id, budget)?;
            result.validate().map_err(|_| invalid())?;
            assert!(!result.satisfied);
            assert_eq!(result.affected_entities.len(), 4);
            assert_eq!(result.observed.len(), 9);
            Ok(())
        })
    }

    #[test]
    fn malformed_generated_fact_is_not_resource_exhaustion() -> TestResult {
        let witness = Witness::pair(selected_pair()?, entity_id(30)?, "availability", "");
        let result = Summary {
            checked: 1,
            violations: 1,
            first: Some(witness),
            ..Summary::default()
        }
        .finish(rule_id(20)?, &mut OperationBudget::evaluation(None));
        assert_eq!(
            result,
            Err(AssignmentRuleError::InvalidConstruction(
                AssignmentConstructionIssue::InvalidRecord
            ))
        );
        Ok(())
    }

    fn rest_witness() -> TestResult<Witness> {
        let mut witness = Witness::pair(selected_pair()?, entity_id(8)?, "shift", "minimum_rest");
        witness.other_shift = Some(support::id(30).parse()?);
        witness.rest = Some(RestWitness {
            minimum_minutes: u32::MAX,
            actual: SignedDuration::new(-7200, -1),
        });
        Ok(witness)
    }

    fn rest_summary_bytes(
        id: RuleId,
        checked: i64,
        violations: i64,
        nanos: i64,
    ) -> TestResult<u64> {
        let mut bytes = summary_bytes(id, checked, violations)? + 6 * 160;
        bytes += encoded(&(
            "official.workforce.fact.witness_reason",
            "text",
            "minimum_rest",
        ))?;
        for (key, shift) in [
            ("official.workforce.fact.rest_source_shift", 8),
            ("official.workforce.fact.rest_target_shift", 30),
        ] {
            let entity = DomainEntityRef {
                kind: DomainEntityKindId::new("official.workforce.shift")?,
                id: DomainEntityId::new(format!("official.workforce.{}", support::id(shift)))?,
            };
            bytes += encoded(&(key, VerificationValue::Entity(entity)))?;
        }
        for (key, value) in [
            (
                "official.workforce.fact.required_rest_minutes",
                i64::from(u32::MAX),
            ),
            ("official.workforce.fact.actual_rest_seconds", -7200),
            (
                "official.workforce.fact.actual_rest_subsecond_nanoseconds",
                nanos,
            ),
        ] {
            bytes += encoded(&(key, VerificationValue::Integer(value)))?;
        }
        Ok(bytes)
    }

    #[test]
    fn rest_summary_enforces_exact_nested_fact_and_entity_output_limits() -> TestResult {
        let id = rule_id(20)?;
        let witness = rest_witness()?;
        boundaries((1, 24, rest_summary_bytes(id, 1, 1, -1)?), |budget| {
            let result = Summary {
                checked: 1,
                violations: 1,
                first: Some(witness),
                ..Summary::default()
            }
            .finish(id, budget)?;
            result.validate().map_err(|_| invalid())?;
            assert!(!result.satisfied);
            assert_eq!(result.affected_entities.len(), 3);
            assert_eq!(
                result.observed.get(
                    &VerificationFactId::new(
                        "official.workforce.fact.actual_rest_subsecond_nanoseconds"
                    )
                    .map_err(|_| invalid())?
                ),
                Some(&VerificationValue::Integer(-1)),
            );
            Ok(())
        })
    }

    fn dense_rest_document(count: u32) -> TestResult<(ScenarioDocument, Vec<AssignmentPair>)> {
        let mut document = document()?;
        document.domain.rules.insert(
            rule_id(20)?,
            json!({
                "id":support::id(20), "kind":"minimumRest", "active":true, "strength":"required",
                "scope":{"people":{"kind":"all"}}, "afterScope":{"people":{"kind":"all"}},
                "beforeScope":{"people":{"kind":"all"}}, "minimumMinutes":u32::MAX,
            }),
        );
        let prototype = document
            .domain
            .entities
            .get(&entity_id(8)?)
            .ok_or("shift")?
            .clone();
        let mut pairs = vec![selected_pair()?];
        for index in 30..30 + count - 1 {
            let mut shift = prototype.clone();
            shift["id"] = json!(support::id(index));
            document.domain.entities.insert(entity_id(index)?, shift);
            pairs.push(AssignmentPair {
                person_id: selected_pair()?.person_id,
                shift_id: support::id(index).parse()?,
            });
        }
        Ok((document, pairs))
    }

    #[test]
    fn rest_role_indices_enforce_exact_bounds_before_retention() -> TestResult {
        let (document, pairs) = dense_rest_document(2)?;
        let mut preparation = OperationBudget::evaluation(None);
        let input = AssignmentInput::new(&document, &mut preparation)?;
        input.validate_selection(&pairs, &mut preparation)?;
        let selected = Selected::new(&input, &pairs, &mut preparation)?;
        let Some(WorkforceRule::MinimumRest(rule)) = input.domain.rules.get(&rule_id(20)?) else {
            return Err("rest rule".into());
        };
        boundaries((0, 4, 64), |budget| {
            let mut summary = Summary::default();
            rest::evaluate(&input, &selected, rule, &mut summary, budget)?;
            assert_eq!((summary.checked, summary.violations), (2, 2));
            Ok(())
        })
    }

    #[test]
    fn complete_rest_evaluation_is_atomic_at_exact_aggregate_output_limits() -> TestResult {
        let (document, pairs) = dense_rest_document(2)?;
        let bytes = summary_bytes(rule_id(1)?, 2, 0)?
            + rest_summary_bytes(rule_id(20)?, 2, 2, 0)?
            + 64
            + 32; // Four role references and two handled obligation IDs.
        for (left, expected) in [
            ((2, 36, bytes), None),
            ((1, 36, bytes), Some(AssignmentRuleLimit::Records)),
            ((2, 35, bytes), Some(AssignmentRuleLimit::References)),
            ((2, 36, bytes - 1), Some(AssignmentRuleLimit::Bytes)),
        ] {
            let mut budget = OperationBudget::evaluation(None);
            let input = AssignmentInput::new(&document, &mut budget)?;
            input.validate_selection(&pairs, &mut budget)?;
            let selected = Selected::new(&input, &pairs, &mut budget)?;
            leave_output(&mut budget, left)?;
            let result = evaluate_prepared(
                &document,
                &input,
                &selected,
                &document.settings,
                &mut budget,
            );
            if let Some(limit) = expected {
                assert_eq!(result, Err(AssignmentRuleError::LimitExceeded(limit)));
            } else {
                let (evaluations, obligations) = result?;
                assert_eq!(evaluations.len(), 2);
                assert!(!evaluations[1].satisfied);
                assert_eq!(obligations.handled, [rule_id(1)?, rule_id(20)?]);
                assert!(obligations.remaining.is_empty());
                assert_eq!(budget.remaining_output(), (0, 0, 0));
            }
        }
        Ok(())
    }

    #[test]
    fn genuine_token_cancels_inside_dense_rest_pairs_after_validated_setup() -> TestResult {
        let (document, pairs) = dense_rest_document(65)?;
        let token = CancellationToken::new();
        let control = eutheto_types::OperationControl::Cancellation(token.clone());
        let mut budget = OperationBudget::evaluation(Some(&control));
        let input = AssignmentInput::new(&document, &mut budget)?;
        input.validate_selection(&pairs, &mut budget)?;
        let selected = Selected::new(&input, &pairs, &mut budget)?;
        let Some(WorkforceRule::MinimumRest(rule)) = input.domain.rules.get(&rule_id(20)?) else {
            return Err("rest rule".into());
        };
        let mut baseline = Summary::default();
        rest::evaluate(&input, &selected, rule, &mut baseline, &mut budget)?;
        assert_eq!((baseline.checked, baseline.violations), (4160, 4160));
        budget.cancel_after_steps(1000)?;
        let mut partial = Summary::default();
        assert_eq!(
            rest::evaluate(&input, &selected, rule, &mut partial, &mut budget),
            Err(AssignmentRuleError::Cancelled),
        );
        assert!(token.is_cancelled());
        // Scope indexing has finished and actual dense failures occurred before cancellation.
        assert!((1..4160).contains(&partial.violations));
        Ok(())
    }

    #[test]
    fn rest_dense_phase_preserves_the_cumulative_work_ceiling() -> TestResult {
        let (document, pairs) = dense_rest_document(65)?;
        let mut budget = OperationBudget::evaluation(None);
        let input = AssignmentInput::new(&document, &mut budget)?;
        input.validate_selection(&pairs, &mut budget)?;
        let selected = Selected::new(&input, &pairs, &mut budget)?;
        let Some(WorkforceRule::MinimumRest(rule)) = input.domain.rules.get(&rule_id(20)?) else {
            return Err("rest rule".into());
        };
        budget.steps(super::super::budget::MAX_WORK_STEPS - 100_000)?;
        let mut summary = Summary::default();
        let error = loop {
            match rest::evaluate(&input, &selected, rule, &mut summary, &mut budget) {
                Ok(()) => {}
                Err(error) => break error,
            }
        };
        assert_eq!(
            error,
            AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::WorkSteps)
        );
        assert!(summary.violations > 4160);
        Ok(())
    }

    #[test]
    fn genuine_token_cancels_during_rest_witness_fact_construction() -> TestResult {
        let (document, pairs) = dense_rest_document(2)?;
        let token = CancellationToken::new();
        let control = eutheto_types::OperationControl::Cancellation(token.clone());
        let mut budget = OperationBudget::evaluation(Some(&control));
        let input = AssignmentInput::new(&document, &mut budget)?;
        input.validate_selection(&pairs, &mut budget)?;
        let selected = Selected::new(&input, &pairs, &mut budget)?;
        let Some(WorkforceRule::MinimumRest(rule)) = input.domain.rules.get(&rule_id(20)?) else {
            return Err("rest rule".into());
        };
        let mut summary = Summary::default();
        rest::evaluate(&input, &selected, rule, &mut summary, &mut budget)?;
        assert_eq!((summary.checked, summary.violations), (2, 2));
        // After the role Entity facts, while constructing the signed numeric rest facts.
        budget.cancel_after_steps(18)?;
        assert_eq!(
            summary.finish(rule_id(20)?, &mut budget),
            Err(AssignmentRuleError::Cancelled)
        );
        assert!(token.is_cancelled());
        Ok(())
    }

    #[test]
    fn complete_semantic_phase_never_returns_an_output_truncated_at_aggregate_limits() -> TestResult
    {
        let mut document = document()?;
        let mut person = document
            .domain
            .entities
            .get(&entity_id(1)?)
            .ok_or("person")?
            .clone();
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
            let result = evaluate_prepared(
                &document,
                &input,
                &selected,
                &document.settings,
                &mut budget,
            );
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
            document.domain.entities.insert(
                entity_id(index)?,
                json!({
                    "kind":"qualification", "id":id, "name":"Qualification", "description":""
                }),
            );
            ids.push(id);
        }
        document
            .domain
            .entities
            .get_mut(&entity_id(1)?)
            .ok_or("person")?["qualificationGrants"] = json!([]);
        document
            .domain
            .entities
            .get_mut(&entity_id(4)?)
            .ok_or("type")?["qualifications"] =
            json!({"kind":"matches","allQualificationIds":ids,"anyQualificationIds":[]});
        document.domain.rules.insert(
            rule_id(20)?,
            json!({
                "kind":"eligibility","id":support::id(20),"active":true,"strength":"required",
                "scope":{"people":{"kind":"all"}}
            }),
        );
        let token = CancellationToken::new();
        let control = eutheto_types::OperationControl::Cancellation(token.clone());
        let mut budget = OperationBudget::evaluation(Some(&control));
        let input = AssignmentInput::new(&document, &mut budget)?;
        let selected = Selected::new(&input, &[selected_pair()?], &mut budget)?;
        let rule = input.domain.rules.get(&rule_id(20)?).ok_or("rule")?;
        let baseline = pair_rule(
            &input,
            &selected,
            rule,
            &document.settings,
            false,
            &mut budget,
        )?;
        assert_eq!((baseline.checked, baseline.violations), (2, 1));
        budget.cancel_after_steps(100)?;
        assert!(matches!(
            pair_rule(
                &input,
                &selected,
                rule,
                &document.settings,
                false,
                &mut budget
            ),
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
        document.domain.entities.insert(
            entity_id(30)?,
            json!({
                "kind":"availability","id":support::id(30),"personId":support::id(1),
                "availabilityKind":"availableOnly", "source":"", "note":"",
                "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
                "timeWindow":{"kind":"weekly","windows":windows}
            }),
        );
        let token = CancellationToken::new();
        let control = eutheto_types::OperationControl::Cancellation(token.clone());
        let mut budget = OperationBudget::evaluation(Some(&control));
        let input = AssignmentInput::new(&document, &mut budget)?;
        let Some(WorkforceEntity::Availability(record)) =
            input.domain.entities.get(&entity_id(30)?)
        else {
            return Err("availability".into());
        };
        let shift = input.shift(selected_pair()?.shift_id).ok_or("shift")?;
        let mut summary = Summary::default();
        check_availability(record, shift, &document.settings, &mut summary, &mut budget)?;
        assert_eq!((summary.checked, summary.violations), (1, 0));
        let records_before = budget.remaining_output().0;
        budget.cancel_after_steps(100)?;
        assert_eq!(
            check_availability(record, shift, &document.settings, &mut summary, &mut budget),
            Err(AssignmentRuleError::Cancelled)
        );
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
        let mut person = input
            .person(selected_pair()?.person_id)
            .ok_or("person")?
            .clone();
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
            let control = eutheto_types::OperationControl::Cancellation(token.clone());
            let mut budget = OperationBudget::evaluation(Some(&control));
            budget.cancel_after_steps(32)?;
            assert_eq!(
                predicates::person_matches(&person, &scope, &mut budget),
                Err(AssignmentRuleError::Cancelled)
            );
            assert!(token.is_cancelled());
        }
        Ok(())
    }

    #[test]
    fn active_empty_tag_populations_cannot_bypass_the_operation_work_ceiling() -> TestResult {
        let mut document = document()?;
        let mut prototype = document
            .domain
            .entities
            .get(&entity_id(1)?)
            .ok_or("person")?
            .clone();
        prototype["tags"] = json!([]);
        prototype
            .as_object_mut()
            .ok_or("person")?
            .remove("externalId");
        document
            .domain
            .entities
            .insert(entity_id(1)?, prototype.clone());
        let mut selected = vec![selected_pair()?];
        for index in 1000..1899 {
            let mut person = prototype.clone();
            person["id"] = json!(support::id(index));
            document.domain.entities.insert(entity_id(index)?, person);
            selected.push(AssignmentPair {
                person_id: support::id(index).parse()?,
                shift_id: selected_pair()?.shift_id,
            });
        }
        let tags: Vec<_> = (0..2500).map(|index| format!("tag{index:04}")).collect();
        for index in 2000..2010 {
            document.domain.rules.insert(rule_id(index)?, json!({
                "id":support::id(index),"kind":"eligibility","active":true,"strength":"required",
                "scope":{"people":{"kind":"filter","allTags":tags,"anyTags":[]}}
            }));
        }
        assert_eq!(
            evaluate_assignment_rules(&document, &selected, None),
            Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::WorkSteps
            ))
        );
        Ok(())
    }
    #[test]
    #[allow(clippy::too_many_lines)]
    fn repair_observations_keep_exact_pair_and_group_membership_without_changing_evaluations()
    -> TestResult {
        let mut source = serde_json::to_value(document()?)?;
        source["settings"]["horizon"]["end"] = json!("2026-11-03T05:00:00Z");
        add_shift_instance(&mut source, 18, 2);
        let mut second_person = source["domain"]["entities"][support::id(1)].clone();
        second_person["id"] = json!(support::id(14));
        second_person["name"] = json!("Riley");
        second_person["externalId"] = json!("staff-02");
        second_person["qualificationGrants"] = json!([{"qualificationId":support::id(12)}]);
        source["domain"]["entities"][support::id(14)] = second_person;
        source["domain"]["entities"][support::id(12)] = json!({
            "kind":"qualification","id":support::id(12),"name":"Specialist","description":""
        });
        source["domain"]["entities"][support::id(8)]["coverage"] = json!({
            "kind":"exact","count":2,
            "qualificationMinimums":[{
                "qualifications":{"allQualificationIds":[support::id(12)],"anyQualificationIds":[]},
                "minimum":1
            }]
        });
        source["domain"]["rules"][support::id(20)] = json!({
            "id":support::id(20), "kind":"fixedAssignment", "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}}, "personId":support::id(1),
            "shiftId":support::id(8), "assigned":false
        });
        source["domain"]["rules"][support::id(21)] = json!({
            "id":support::id(21), "kind":"workloadQuota", "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}}, "calendarId":support::id(2),
            "membership":"reportingDate", "minimum":0, "maximum":0,
            "measure":{"kind":"assignmentCount"}
        });
        source["domain"]["rules"][support::id(22)] = json!({
            "id":support::id(22), "kind":"coverage", "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}}
        });
        source["domain"]["rules"][support::id(23)] = json!({
            "id":support::id(23), "kind":"requiredSkillMix", "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}},
            "qualificationMinimums":[{
                "qualifications":{"allQualificationIds":[support::id(11)],"anyQualificationIds":[]},
                "minimum":2
            }]
        });
        let document: ScenarioDocument = serde_json::from_value(source)?;
        let next_day = pair(1, 18)?;
        let unqualified = pair(14, 8)?;
        let pair = pair(1, 8)?;
        let selected = [pair];
        let associations = [pair, next_day, unqualified];
        let ordinary = evaluate_assignment_rules(&document, &selected, None)?;
        let (evaluations, obligations, observations) =
            repair_selection(&document, &selected, &associations)?;
        assert_eq!(evaluations, ordinary.evaluations);
        assert_eq!(obligations, ordinary.obligations);
        let fixed_id = rule_id(20)?;
        let quota_id = rule_id(21)?;
        let skill_id = rule_id(23)?;

        let fixed = observations
            .iter()
            .find(|value| value.evaluation.rule_id == fixed_id)
            .ok_or("fixed observation missing")?;
        assert_eq!(
            observed_text(&fixed.evaluation, REPAIR_ASSOCIATION_KIND)?,
            "direct"
        );
        assert_eq!(fixed.affected_pairs, [pair]);

        let quota = observations
            .iter()
            .find(|value| value.evaluation.rule_id == quota_id)
            .ok_or("quota observation missing")?;
        assert_eq!(
            observed_text(&quota.evaluation, REPAIR_ASSOCIATION_KIND)?,
            "workload_quota_assignment_count"
        );
        assert_eq!(quota.affected_pairs, [pair]);

        let coverage_id = rule_id(22)?;
        let coverage = observations
            .iter()
            .find(|value| {
                value.evaluation.rule_id == coverage_id
                    && value.affected_pairs == [pair, unqualified]
            })
            .ok_or("shift-scoped coverage observation missing")?;
        assert_eq!(
            observed_text(&coverage.evaluation, REPAIR_ASSOCIATION_KIND)?,
            "coverage_headcount"
        );
        assert!(observations.iter().any(|value| {
            value.evaluation.rule_id == coverage_id && value.affected_pairs == [next_day]
        }));
        let coverage_skill = observations
            .iter()
            .find(|value| {
                value.evaluation.rule_id == coverage_id && value.affected_pairs == [unqualified]
            })
            .ok_or("qualification-scoped coverage observation missing")?;
        assert_eq!(
            observed_text(&coverage_skill.evaluation, REPAIR_ASSOCIATION_KIND)?,
            "coverage_qualification_minimum"
        );

        let skill = observations
            .iter()
            .find(|value| value.evaluation.rule_id == skill_id && value.affected_pairs == [pair])
            .ok_or("qualification-scoped skill observation missing")?;
        assert_eq!(
            observed_text(&skill.evaluation, REPAIR_ASSOCIATION_KIND)?,
            "required_skill_mix"
        );

        let mut health_budget = OperationBudget::evaluation(None);
        let (health_evaluation, _) =
            evaluate_assignment_rules_with_health(&document, &selected, &mut health_budget)?;
        assert_eq!(health_evaluation.evaluations, ordinary.evaluations);
        assert_eq!(health_evaluation.obligations, ordinary.obligations);
        Ok(())
    }

    #[test]
    fn repair_retains_each_approved_leave_and_exact_coverage_shift() -> TestResult {
        let mut source = serde_json::to_value(document()?)?;
        source["settings"]["horizon"]["end"] = json!("2026-11-04T05:00:00Z");
        add_shift_instance(&mut source, 18, 2);
        add_shift_instance(&mut source, 19, 3);
        for index in [30, 31] {
            source["domain"]["entities"][support::id(index)] = json!({
                "kind":"availability", "id":support::id(index), "personId":support::id(1),
                "availabilityKind":"approvedTimeOff",
                "timeWindow":{"kind":"instant","startsAt":"2026-11-01T05:00:00Z",
                    "endsAt":"2026-11-01T08:00:00Z"},
                "effectiveRange":{"startDate":"2026-01-01","endDateExclusive":"2027-01-01"},
                "source":"fixture","note":"fixture"
            });
        }
        source["domain"]["rules"][support::id(22)] = json!({
            "id":support::id(22), "kind":"coverage", "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}}
        });
        let document: ScenarioDocument = serde_json::from_value(source)?;
        let first = pair(1, 8)?;
        let second = pair(1, 18)?;
        let third = pair(1, 19)?;
        let (_, _, observations) = repair_selection(&document, &[first], &[first, second, third])?;
        for index in [30, 31] {
            let leave_id = rule_id(index)?;
            let leave = observations
                .iter()
                .find(|value| value.evaluation.rule_id == leave_id)
                .ok_or("approved-leave observation missing")?;
            assert_eq!(leave.affected_pairs, [first]);
        }
        let coverage_id = rule_id(22)?;
        for member in [second, third] {
            let coverage = observations
                .iter()
                .find(|value| {
                    value.evaluation.rule_id == coverage_id && value.affected_pairs == [member]
                })
                .ok_or("source-shift coverage observation missing")?;
            assert_eq!(
                observed_text(&coverage.evaluation, REPAIR_ASSOCIATION_KIND)?,
                "coverage_headcount"
            );
        }
        Ok(())
    }

    #[test]
    fn repair_errors_publish_no_partial_result_on_exhaustion_or_cancellation() -> TestResult {
        let document = document()?;
        let selected = [selected_pair()?];
        let mut exhausted = OperationBudget::evaluation(None);
        let input = AssignmentInput::new(&document, &mut exhausted)?;
        leave_output(&mut exhausted, (0, 0, 0))?;
        assert!(matches!(
            evaluate_repair_selection(&document, &input, &selected, &selected, &mut exhausted,),
            Err(AssignmentRuleError::LimitExceeded(_))
        ));

        let token = CancellationToken::new();
        let control = eutheto_types::OperationControl::Cancellation(token.clone());
        let mut cancelled = OperationBudget::evaluation(Some(&control));
        let input = AssignmentInput::new(&document, &mut cancelled)?;
        cancelled.cancel_after_steps(1)?;
        assert!(matches!(
            evaluate_repair_selection(&document, &input, &selected, &selected, &mut cancelled,),
            Err(AssignmentRuleError::Cancelled)
        ));
        assert!(token.is_cancelled());
        Ok(())
    }
    #[test]
    fn maximum_weekend_group_keeps_a_nonillustrative_contributor() -> TestResult {
        let mut source = serde_json::to_value(document()?)?;
        source["settings"]["horizon"]["end"] = json!("2026-11-08T05:00:00Z");
        add_shift_instance(&mut source, 18, 7);
        source["domain"]["rules"][support::id(24)] = json!({
            "id":support::id(24), "kind":"maximumWorkedWeekends", "active":true,
            "strength":"required", "scope":{"people":{"kind":"all"}}, "maximum":1
        });
        let document: ScenarioDocument = serde_json::from_value(source)?;
        let first_weekend = pair(1, 8)?;
        let later_weekend = pair(1, 18)?;
        let (_, _, observations) =
            repair_selection(&document, &[first_weekend, later_weekend], &[first_weekend])?;
        let weekend_id = rule_id(24)?;
        let observation = observations
            .iter()
            .find(|value| value.evaluation.rule_id == weekend_id)
            .ok_or("weekend observation missing")?;
        assert_eq!(observation.affected_pairs, [first_weekend]);
        assert_eq!(
            observed_text(&observation.evaluation, REPAIR_ASSOCIATION_KIND)?,
            "maximum_worked_weekends"
        );
        let later_shift = format!("official.workforce.{}", support::id(18));
        assert!(
            observation
                .evaluation
                .affected_entities
                .iter()
                .any(|entity| {
                    entity.kind.as_str() == "official.workforce.shift"
                        && entity.id.as_str() == later_shift.as_str()
                })
        );
        Ok(())
    }
}
