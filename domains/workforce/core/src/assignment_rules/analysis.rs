#[path = "compiler_support.rs"]
pub(super) mod support;

#[path = "compiler_rest.rs"]
mod rest;

use super::{
    AssignmentAnalysis, AssignmentConstructionIssue, AssignmentModelEstimate, AssignmentRuleError,
    AssignmentRuleLimit, InstantInterval, PairRejection, RejectionCause, RequiredRulePartition,
    budget::{MAX_INSPECTED_PAIRS, OperationBudget, add, count, within},
    input::{AssignmentInput, ShiftDefinition, ShiftMetadata},
    intervals::availability_intervals,
};
use crate::{
    ids::ShiftId,
    model::{
        AssignmentLock, AssignmentPair, Availability, AvailabilityKind, CategoryPair, Coverage,
        LockState, Person, Scope, WorkforceEntity, WorkforceRule,
    },
    temporal::ResolvedShift,
};
use eutheto_domain_api::DomainValidationReport;
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{
    CancellationToken, EntityId, PersonId, ResourceRef, RuleId, ScenarioDocument, ValidationIssue,
    ValidationSeverity,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};
use support::{
    MinimumKey, active_interval, availability_scope, bounds, canonical_minima, expression,
    incompatible, interval, invalid, outside, person_scope, qualification_match, requirement_scope,
    shift_scope, uncovered,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(super) struct Owner {
    pub kind: &'static str,
    pub id: EntityId,
}

pub(super) struct Definition {
    pub owner: Owner,
    pub minima: Vec<MinimumKey>,
}

#[derive(Clone, Copy)]
pub(super) enum Predicate {
    Headcount {
        definition: usize,
        shift: ShiftId,
        lower: u64,
        upper: u64,
        authored_upper: Option<u64>,
    },
    Qualification {
        definition: usize,
        minimum: usize,
        shift: ShiftId,
        upper: u64,
    },
    Overlap {
        person: PersonId,
        first: ShiftId,
        second: ShiftId,
    },
    MinimumRest {
        person: PersonId,
        source: ShiftId,
        target: ShiftId,
        minimum_minutes: u32,
    },
}

pub(super) struct PlannedConstraint {
    pub rule: RuleId,
    pub predicate: Predicate,
    // Positions in the one analysis candidate vector, never duplicate owned candidate vectors.
    pub population: Vec<usize>,
    pub impossible: bool,
}

pub(super) struct Plan {
    pub definitions: Vec<Definition>,
    pub constraints: Vec<PlannedConstraint>,
    pub parents: BTreeMap<RuleId, &'static str>,
}

/// Analyze the original document without a solver, registration, or acceptance authority.
///
/// # Errors
/// Returns structural, temporal, cancellation, finite-work, or bounded-output failures atomically.
pub fn analyze_assignments(
    document: &ScenarioDocument,
    cancellation: Option<&CancellationToken>,
    limits: PlanningIrLimitsV1,
) -> Result<AssignmentAnalysis, AssignmentRuleError> {
    let mut budget = OperationBudget::analysis(cancellation, limits);
    let input = AssignmentInput::new(document, &mut budget)?;
    let (analysis, plan) = prepare(document, &input, &mut budget, limits)?;
    super::compiler::preflight(&analysis.candidates, &plan, &mut budget, limits)?;
    Ok(analysis)
}

pub(super) fn prepare(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(AssignmentAnalysis, Plan), AssignmentRuleError> {
    budget.check()?;
    let raw = count(input.people.len())?
        .checked_mul(count(input.shifts.len())?)
        .ok_or(AssignmentRuleError::InvalidConstruction(
            AssignmentConstructionIssue::ArithmeticOverflow,
        ))?;
    within(
        raw,
        MAX_INSPECTED_PAIRS,
        AssignmentRuleLimit::InspectedPairs,
    )?;
    let mut result = AssignmentAnalysis {
        source_document_hash: String::new(),
        candidates: Vec::new(),
        rejections: Vec::new(),
        estimate: AssignmentModelEstimate {
            inspected_pairs: raw,
            ..AssignmentModelEstimate::default()
        },
        validation: DomainValidationReport::default(),
        obligations: obligations(input, budget)?,
    };
    for id in &input.people {
        budget.step()?;
        let person = input.person(*id).ok_or_else(invalid)?;
        let active = active_interval(person, &document.settings)?;
        for shift in &input.shifts {
            let context = PairContext {
                document,
                input,
                person,
                shift,
                metadata: input.metadata(shift)?,
                pair: AssignmentPair {
                    person_id: *id,
                    shift_id: shift.id,
                },
            };
            context.analyze(active, &mut result, budget)?;
        }
    }
    budget.sort_work(result.candidates.len())?;
    result.candidates.sort_unstable();
    budget.sort_work(result.rejections.len())?;
    result.rejections.sort_unstable();
    result.estimate.after_availability_pruning = count(result.candidates.len())?;
    result.estimate.variables = count(result.candidates.len())?;
    result.estimate.rejection_facts = count(result.rejections.len())?;
    let plan = plan(input, &mut result, budget, limits)?;
    hard_lock_findings(input, &mut result, &plan, budget)?;
    budget.sort_work(result.validation.issues.len())?;
    result.validation.issues.sort_by(|a, b| {
        (&a.code, &a.field_path, &a.message).cmp(&(&b.code, &b.field_path, &b.message))
    });
    budget.steps(count(result.validation.issues.len())?)?;
    result.validation.issues.dedup();
    // The hash was measured by AssignmentInput; this is the only retained additional copy.
    budget.reserve(0, 0, 64)?;
    result
        .source_document_hash
        .clone_from(&input.source_document_hash);
    budget.check()?;
    Ok((result, plan))
}

struct PairContext<'a> {
    document: &'a ScenarioDocument,
    input: &'a AssignmentInput,
    person: &'a Person,
    shift: &'a ResolvedShift,
    metadata: ShiftMetadata<'a>,
    pair: AssignmentPair,
}

impl PairContext<'_> {
    fn analyze(
        &self,
        active: Option<InstantInterval>,
        result: &mut AssignmentAnalysis,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        budget.step()?;
        let mut activity_ok = true;
        let mut type_ok = true;
        let mut qualifications_ok = true;
        let mut availability_ok = true;
        if let Some(allowed) = active
            && let Some(outside) = outside(interval(self.shift), allowed)
        {
            activity_ok = false;
            reject(
                result,
                self.pair,
                RuleId::from_uuid(self.person.id.as_uuid()),
                RejectionCause::OutsideActiveRange { allowed, outside },
                budget,
            )?;
        }
        for rule in self.input.domain.rules.values() {
            budget.step()?;
            let (binding, active, scope) = rule.header();
            if !active
                || !matches!(
                    rule,
                    WorkforceRule::Eligibility { .. } | WorkforceRule::Availability { .. }
                )
            {
                continue;
            }
            if !person_scope(scope, self.person, budget)?
                || !shift_scope(scope, self.shift, &self.metadata, budget)?
            {
                continue;
            }
            match rule {
                WorkforceRule::Eligibility { .. } => {
                    let (membership, qualifications) = self.eligibility(binding, result, budget)?;
                    type_ok &= membership;
                    qualifications_ok &= qualifications;
                }
                WorkforceRule::Availability { .. } => {
                    availability_ok &= self.availability(Some(binding), result, budget)?;
                }
                _ => {}
            }
        }
        // Approved leave is independently owned, regardless of any Availability rule.
        availability_ok &= self.availability(None, result, budget)?;
        record_candidate(
            result,
            self.pair,
            (activity_ok, type_ok, qualifications_ok, availability_ok),
            budget,
        )
    }

    fn eligibility(
        &self,
        binding: RuleId,
        result: &mut AssignmentAnalysis,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(bool, bool), AssignmentRuleError> {
        budget.steps(count(self.person.eligible_assignment_type_ids.len())?)?;
        let membership = self
            .person
            .eligible_assignment_type_ids
            .contains(&self.metadata.assignment_type.id);
        if !membership {
            reject(
                result,
                self.pair,
                binding,
                RejectionCause::AssignmentTypeNotAllowed {
                    assignment_type_id: self.metadata.assignment_type.id,
                },
                budget,
            )?;
        }
        let qualifications = expression(
            self.person,
            &self.metadata.assignment_type.qualifications,
            interval(self.shift),
            budget,
        )?;
        if !qualifications {
            reject(
                result,
                self.pair,
                binding,
                RejectionCause::QualificationExpression {
                    assignment_type_id: self.metadata.assignment_type.id,
                },
                budget,
            )?;
        }
        Ok((membership, qualifications))
    }

    fn availability(
        &self,
        ordinary_rule: Option<RuleId>,
        result: &mut AssignmentAnalysis,
        budget: &mut OperationBudget<'_>,
    ) -> Result<bool, AssignmentRuleError> {
        let mut satisfied = true;
        let Some(records) = self.input.availability_by_person.get(&self.person.id) else {
            return Ok(true);
        };
        for availability_id in records {
            budget.step()?;
            let Some(WorkforceEntity::Availability(record)) = self
                .input
                .domain
                .entities
                .get(&availability_id.as_entity_id())
            else {
                return Err(invalid());
            };
            let applicable_kind = if ordinary_rule.is_some() {
                matches!(
                    record.availability_kind,
                    AvailabilityKind::Unavailable | AvailabilityKind::AvailableOnly
                )
            } else {
                record.availability_kind == AvailabilityKind::ApprovedTimeOff
            };
            if !applicable_kind || !availability_scope(record, &self.metadata, budget)? {
                continue;
            }
            if let Some(cause) =
                availability_cause(record, interval(self.shift), self.document, budget)?
            {
                satisfied = false;
                let binding = ordinary_rule
                    .unwrap_or_else(|| RuleId::from_uuid(record.id.as_entity_id().as_uuid()));
                reject(result, self.pair, binding, cause, budget)?;
            }
        }
        Ok(satisfied)
    }
}

fn record_candidate(
    result: &mut AssignmentAnalysis,
    pair: AssignmentPair,
    stages: (bool, bool, bool, bool),
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let (activity_ok, type_ok, qualifications_ok, availability_ok) = stages;
    if activity_ok {
        result.estimate.after_activity_pruning = add(result.estimate.after_activity_pruning, 1)?;
    }
    if activity_ok && type_ok {
        result.estimate.after_assignment_type_pruning =
            add(result.estimate.after_assignment_type_pruning, 1)?;
    }
    if activity_ok && type_ok && qualifications_ok {
        result.estimate.after_qualification_pruning =
            add(result.estimate.after_qualification_pruning, 1)?;
    }
    if activity_ok && type_ok && qualifications_ok && availability_ok {
        within(
            add(count(result.candidates.len())?, 1)?,
            budget.variable_limit(),
            AssignmentRuleLimit::Variables,
        )?;
        budget.reserve(1, 2, budget.measure(&pair)?)?;
        result.candidates.push(pair);
    }
    Ok(())
}

fn availability_cause(
    record: &Availability,
    query: InstantInterval,
    document: &ScenarioDocument,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<RejectionCause>, AssignmentRuleError> {
    let windows = availability_intervals(record, query, &document.settings, budget)?;
    let Some(query) = windows.query else {
        return Ok(None);
    };
    match record.availability_kind {
        AvailabilityKind::AvailableOnly => Ok(uncovered(query, &windows.intervals, budget)?.map(
            |uncovered| RejectionCause::OutsideAvailableOnly {
                availability_id: record.id,
                uncovered,
            },
        )),
        AvailabilityKind::Unavailable => {
            Ok(windows
                .intervals
                .first()
                .copied()
                .map(|overlap| RejectionCause::Unavailable {
                    availability_id: record.id,
                    overlap,
                }))
        }
        AvailabilityKind::ApprovedTimeOff => {
            Ok(windows
                .intervals
                .first()
                .copied()
                .map(|overlap| RejectionCause::ApprovedTimeOff {
                    availability_id: record.id,
                    overlap,
                }))
        }
        AvailabilityKind::RequestedTimeOff => Ok(None),
    }
}

fn reject(
    result: &mut AssignmentAnalysis,
    pair: AssignmentPair,
    binding_id: RuleId,
    cause: RejectionCause,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    // The largest fixed cause has four timestamp endpoints plus three UUIDs. There are no
    // copied strings, expressions, grants or window lists in this bounded sidecar record.
    budget.reserve(1, 8, 512)?;
    result.rejections.push(PairRejection {
        pair,
        binding_id,
        cause,
    });
    Ok(())
}

fn obligations(
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
) -> Result<RequiredRulePartition, AssignmentRuleError> {
    let mut result = RequiredRulePartition {
        handled: Vec::new(),
        remaining: Vec::new(),
    };
    for rule in input.domain.rules.values() {
        budget.step()?;
        let (id, active, _) = rule.header();
        if !active {
            continue;
        }
        budget.reserve(0, 1, 16)?;
        if matches!(
            rule,
            WorkforceRule::Eligibility { .. }
                | WorkforceRule::Availability { .. }
                | WorkforceRule::Coverage { .. }
                | WorkforceRule::NoOverlap { .. }
                | WorkforceRule::MinimumRest(_)
        ) {
            result.handled.push(id);
        } else {
            result.remaining.push(id);
        }
    }
    for entity in input.domain.entities.values() {
        budget.step()?;
        let id = match entity {
            WorkforceEntity::Person(person) => Some(RuleId::from_uuid(person.id.as_uuid())),
            WorkforceEntity::Availability(record)
                if record.availability_kind == AvailabilityKind::ApprovedTimeOff =>
            {
                Some(RuleId::from_uuid(record.id.as_entity_id().as_uuid()))
            }
            _ => None,
        };
        if let Some(id) = id {
            budget.reserve(0, 1, 16)?;
            result.handled.push(id);
        }
    }
    for lock in input.domain.locked_assignments.values() {
        budget.step()?;
        if matches!(lock.state, LockState::Hard {}) {
            budget.reserve(0, 1, 16)?;
            result.remaining.push(RuleId::from_uuid(lock.id.as_uuid()));
        }
    }
    budget.sort_work(result.handled.len())?;
    result.handled.sort_unstable();
    budget.steps(count(result.handled.len())?)?;
    result.handled.dedup();
    budget.sort_work(result.remaining.len())?;
    result.remaining.sort_unstable();
    budget.steps(count(result.remaining.len())?)?;
    result.remaining.dedup();
    Ok(result)
}

fn definition_owner(definition: ShiftDefinition) -> Owner {
    Owner {
        kind: match definition {
            ShiftDefinition::Template(_) => "shift_template",
            ShiftDefinition::Instance(_) => "shift_instance",
        },
        id: definition.entity_id(),
    }
}

fn plan(
    input: &AssignmentInput,
    result: &mut AssignmentAnalysis,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<Plan, AssignmentRuleError> {
    let mut plan = Plan {
        definitions: Vec::new(),
        constraints: Vec::new(),
        parents: BTreeMap::new(),
    };
    let mut definitions = BTreeMap::new();
    let mut by_shift = BTreeMap::<ShiftId, Vec<usize>>::new();
    for (index, pair) in result.candidates.iter().enumerate() {
        budget.step()?;
        budget.reserve(1, 2, 24)?;
        by_shift.entry(pair.shift_id).or_default().push(index);
    }
    for rule in input.domain.rules.values() {
        budget.step()?;
        let (id, active, scope) = rule.header();
        if !active {
            continue;
        }
        match rule {
            WorkforceRule::Coverage { .. } => {
                CoveragePlanner {
                    input,
                    result,
                    plan: &mut plan,
                    definitions: &mut definitions,
                    by_shift: &by_shift,
                }
                .add_rule(id, scope, budget)?;
            }
            WorkforceRule::NoOverlap {
                compatible_category_pairs,
                ..
            } => {
                plan_overlap(
                    input,
                    &result.candidates,
                    &mut plan,
                    id,
                    scope,
                    compatible_category_pairs,
                    budget,
                )?;
            }
            WorkforceRule::MinimumRest(rest) => rest::RestRule {
                id,
                scope,
                after_scope: &rest.after_scope,
                before_scope: &rest.before_scope,
                minimum_minutes: rest.minimum_minutes,
            }
            .plan(input, &result.candidates, &mut plan, budget)?,
            _ => {}
        }
    }
    coverage_bound_findings(&plan, &mut result.validation, budget)?;
    estimate_contribution(result, &plan, budget, limits)?;
    Ok(plan)
}

struct CoveragePlanner<'a> {
    input: &'a AssignmentInput,
    result: &'a mut AssignmentAnalysis,
    plan: &'a mut Plan,
    definitions: &'a mut BTreeMap<Owner, usize>,
    by_shift: &'a BTreeMap<ShiftId, Vec<usize>>,
}

impl CoveragePlanner<'_> {
    fn add_rule(
        &mut self,
        rule: RuleId,
        scope: &Scope,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        let input = self.input;
        for shift in &input.shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !shift_scope(scope, shift, &metadata, budget)? {
                continue;
            }
            for (owner, coverage) in coverage_owners(input, shift, &metadata, budget)? {
                budget.step()?;
                self.add_owner(rule, scope, shift, owner, coverage, budget)?;
            }
        }
        Ok(())
    }

    fn add_owner(
        &mut self,
        rule: RuleId,
        scope: &Scope,
        shift: &ResolvedShift,
        owner: Owner,
        coverage: &Coverage,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        let definition = match self.definitions.entry(owner) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let index = self.plan.definitions.len();
                let minima = canonical_minima(coverage, budget)?;
                budget.reserve(2, 2, 64)?;
                self.plan.definitions.push(Definition { owner, minima });
                entry.insert(index);
                index
            }
        };
        let mut population = Vec::new();
        if let Some(indices) = self.by_shift.get(&shift.id) {
            for index in indices {
                budget.step()?;
                let person = self
                    .input
                    .person(self.result.candidates[*index].person_id)
                    .ok_or_else(invalid)?;
                if person_scope(scope, person, budget)? {
                    budget.reserve(0, 1, 8)?;
                    population.push(*index);
                }
            }
        }
        let (lower, authored_upper) = bounds(coverage);
        let n = count(population.len())?;
        let upper = authored_upper.unwrap_or(n).min(n);
        if lower > n {
            finding(
                &mut self.result.validation,
                "candidate_shortage",
                rule,
                owner,
                shift.id,
                budget,
            )?;
        }
        if lower > upper && lower <= n {
            finding(
                &mut self.result.validation,
                "contradictory_coverage_bounds",
                rule,
                owner,
                shift.id,
                budget,
            )?;
        }
        self.add_minima(rule, shift, definition, &population, authored_upper, budget)?;
        append(
            self.plan,
            PlannedConstraint {
                rule,
                predicate: Predicate::Headcount {
                    definition,
                    shift: shift.id,
                    lower,
                    upper,
                    authored_upper,
                },
                population,
                impossible: lower > upper,
            },
            budget,
        )
    }

    fn add_minima(
        &mut self,
        rule: RuleId,
        shift: &ResolvedShift,
        definition: usize,
        head_population: &[usize],
        authored_upper: Option<u64>,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        let owner = self.plan.definitions[definition].owner;
        for minimum in 0..self.plan.definitions[definition].minima.len() {
            budget.step()?;
            let key = &self.plan.definitions[definition].minima[minimum];
            let mut qualified = Vec::new();
            for index in head_population {
                budget.step()?;
                let person = self
                    .input
                    .person(self.result.candidates[*index].person_id)
                    .ok_or_else(invalid)?;
                if qualification_match(person, &key.all, &key.any, interval(shift), budget)? {
                    budget.reserve(0, 1, 8)?;
                    qualified.push(*index);
                }
            }
            let qualified_count = count(qualified.len())?;
            if u64::from(key.minimum) > qualified_count {
                finding(
                    &mut self.result.validation,
                    "qualification_shortage",
                    rule,
                    owner,
                    shift.id,
                    budget,
                )?;
            }
            if authored_upper.is_some_and(|upper| u64::from(key.minimum) > upper) {
                finding(
                    &mut self.result.validation,
                    "qualification_above_headcount",
                    rule,
                    owner,
                    shift.id,
                    budget,
                )?;
            }
            let impossible = u64::from(key.minimum) > qualified_count;
            append(
                self.plan,
                PlannedConstraint {
                    rule,
                    predicate: Predicate::Qualification {
                        definition,
                        minimum,
                        shift: shift.id,
                        upper: qualified_count,
                    },
                    population: qualified,
                    impossible,
                },
                budget,
            )?;
        }
        Ok(())
    }
}

fn coverage_owners<'a>(
    input: &'a AssignmentInput,
    shift: &ResolvedShift,
    metadata: &ShiftMetadata<'a>,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<(Owner, &'a Coverage)>, AssignmentRuleError> {
    let mut owners = Vec::new();
    budget.reserve(1, 1, 32)?;
    owners.push((definition_owner(metadata.definition), metadata.coverage));
    for entity in input.domain.entities.values() {
        budget.step()?;
        if let WorkforceEntity::CoverageRequirement(requirement) = entity
            && requirement.active
            && requirement_scope(&requirement.scope, shift, metadata, budget)?
        {
            budget.reserve(1, 1, 32)?;
            owners.push((
                Owner {
                    kind: "coverage_requirement",
                    id: requirement.id.as_entity_id(),
                },
                &requirement.coverage,
            ));
        }
    }
    budget.sort_work(owners.len())?;
    owners.sort_by_key(|(owner, _)| *owner);
    Ok(owners)
}

fn scoped_chronological(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    person: PersonId,
    scope: &Scope,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<usize>, AssignmentRuleError> {
    let mut chronological = Vec::new();
    let start = candidates.partition_point(|pair| pair.person_id < person);
    for (offset, pair) in candidates[start..].iter().enumerate() {
        budget.step()?;
        if pair.person_id != person {
            break;
        }
        let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
        if shift_scope(scope, shift, &input.metadata(shift)?, budget)? {
            budget.reserve(0, 1, 8)?;
            chronological.push(start + offset);
        }
    }
    budget.sort_work(chronological.len())?;
    chronological.sort_unstable_by_key(|index| {
        let pair = candidates[*index];
        (
            input
                .shift(pair.shift_id)
                .map(|shift| shift.interval.starts_at.instant),
            pair.shift_id,
        )
    });
    Ok(chronological)
}

fn plan_overlap(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    plan: &mut Plan,
    rule: RuleId,
    scope: &Scope,
    compatibility: &[CategoryPair],
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !person_scope(scope, person, budget)? {
            continue;
        }
        let chronological = scoped_chronological(input, candidates, *person_id, scope, budget)?;
        for (position, first_index) in chronological.iter().enumerate() {
            budget.step()?;
            let first = input
                .shift(candidates[*first_index].shift_id)
                .ok_or_else(invalid)?;
            for second_index in &chronological[position + 1..] {
                budget.step()?;
                let second = input
                    .shift(candidates[*second_index].shift_id)
                    .ok_or_else(invalid)?;
                if second.interval.starts_at.instant >= first.interval.ends_at.instant {
                    break;
                }
                if incompatible(input, first, second, compatibility, budget)? {
                    budget.reserve(0, 2, 16)?;
                    append(
                        plan,
                        PlannedConstraint {
                            rule,
                            predicate: Predicate::Overlap {
                                person: *person_id,
                                first: first.id.min(second.id),
                                second: first.id.max(second.id),
                            },
                            population: vec![*first_index, *second_index],
                            impossible: false,
                        },
                        budget,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn estimate_contribution(
    result: &mut AssignmentAnalysis,
    plan: &Plan,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let variables = result.estimate.variables;
    let constraints = count(plan.constraints.len())?;
    let provenance = add(add(variables, constraints)?, count(plan.parents.len())?)?;
    within(
        provenance,
        budget.provenance_limit(),
        AssignmentRuleLimit::ProvenanceRecords,
    )?;
    let defaults = PlanningIrLimitsV1::DEFAULT;
    // variable -> fact; fact -> person/shift
    let mut references = variables.checked_mul(3).ok_or_else(invalid)?;
    for constraint in &plan.constraints {
        budget.step()?;
        let literals = if constraint.impossible {
            0
        } else {
            count(constraint.population.len())?
        };
        within(
            literals,
            limits.max_refs_per_node.min(defaults.max_refs_per_node),
            AssignmentRuleLimit::PerRecord,
        )?;
        let (entities, parameters) = predicate_shape(&constraint.predicate, plan)?;
        within(
            entities,
            limits
                .max_entity_refs_per_record
                .min(defaults.max_entity_refs_per_record),
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            parameters,
            limits
                .max_parameters_per_record
                .min(defaults.max_parameters_per_record),
            AssignmentRuleLimit::PerRecord,
        )?;
        references = add(
            references,
            add(literals, add(2, add(entities, parameters)?)?)?,
        )?;
    }
    within(
        references,
        limits.max_total_refs.min(defaults.max_total_refs),
        AssignmentRuleLimit::References,
    )?;
    if variables > 0 {
        within(
            2,
            limits.max_entity_refs_per_record,
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            1,
            limits.max_provenance_depth,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    if constraints > 0 {
        within(
            2,
            limits.max_provenance_depth,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    result.estimate.constraints = constraints;
    result.estimate.provenance_records = provenance;
    result.estimate.references = references;
    Ok(())
}

pub(super) fn predicate_shape(
    predicate: &Predicate,
    plan: &Plan,
) -> Result<(u64, u64), AssignmentRuleError> {
    Ok(match predicate {
        Predicate::Headcount { authored_upper, .. } => (2, 3 + u64::from(authored_upper.is_some())),
        Predicate::Qualification {
            definition,
            minimum,
            ..
        } => {
            let key = &plan.definitions[*definition].minima[*minimum];
            // all/any membership is retained as separately named typed Entity parameters.
            (2, add(1, count(key.all.len() + key.any.len())?)?)
        }
        Predicate::Overlap { .. } => (3, 0),
        Predicate::MinimumRest { .. } => (3, 3),
    })
}

fn append(
    plan: &mut Plan,
    constraint: PlannedConstraint,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    within(
        add(count(plan.constraints.len())?, 1)?,
        budget.constraint_limit(),
        AssignmentRuleLimit::Constraints,
    )?;
    if let Entry::Vacant(entry) = plan.parents.entry(constraint.rule) {
        budget.reserve(1, 1, 64)?;
        entry.insert(match constraint.predicate {
            Predicate::Overlap { .. } => "no_overlap",
            Predicate::MinimumRest { .. } => "minimum_rest",
            Predicate::Headcount { .. } | Predicate::Qualification { .. } => "coverage",
        });
    }
    budget.reserve(1, 4, 128)?;
    plan.constraints.push(constraint);
    Ok(())
}

fn coverage_bound_findings(
    plan: &Plan,
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut by_shift = BTreeMap::<ShiftId, Vec<&PlannedConstraint>>::new();
    for constraint in &plan.constraints {
        budget.step()?;
        let shift = match constraint.predicate {
            Predicate::Headcount { shift, .. } | Predicate::Qualification { shift, .. } => shift,
            Predicate::Overlap { .. } | Predicate::MinimumRest { .. } => continue,
        };
        budget.reserve(1, 2, 24)?;
        by_shift.entry(shift).or_default().push(constraint);
    }
    for (shift, predicates) in by_shift {
        for lower in &predicates {
            let (definition, minimum, code) = match lower.predicate {
                Predicate::Headcount {
                    definition, lower, ..
                } => (definition, lower, "contradictory_coverage_bounds"),
                Predicate::Qualification {
                    definition,
                    minimum,
                    ..
                } => (
                    definition,
                    u64::from(plan.definitions[definition].minima[minimum].minimum),
                    "qualification_above_headcount",
                ),
                Predicate::Overlap { .. } | Predicate::MinimumRest { .. } => continue,
            };
            for upper in &predicates {
                budget.step()?;
                let Predicate::Headcount {
                    definition: upper_definition,
                    upper: maximum,
                    ..
                } = upper.predicate
                else {
                    continue;
                };
                if minimum <= maximum {
                    continue;
                }
                // A lower-bounded subset cannot exceed the upper bound on its containing
                // population. A merge walk proves containment; partially overlapping groups
                // are not treated as the same population.
                let mut position = 0;
                let mut subset = true;
                for index in &lower.population {
                    budget.step()?;
                    while position < upper.population.len() && upper.population[position] < *index {
                        budget.step()?;
                        position += 1;
                    }
                    if upper.population.get(position) != Some(index) {
                        subset = false;
                        break;
                    }
                }
                if subset {
                    let owner = plan.definitions[definition].owner;
                    let other = plan.definitions[upper_definition].owner;
                    budget.reserve(1, 7, 1024)?;
                    report.issues.push(ValidationIssue {
                        code: format!("official.workforce.{code}"), severity: ValidationSeverity::Error,
                        message: format!("Rule {} owner {} {} requires at least {minimum}, exceeding {maximum} allowed by rule {} owner {} {} for shift {shift}. This is not a solver feasibility result.", lower.rule, owner.kind, owner.id, upper.rule, other.kind, other.id),
                        field_path: Some(format!("domain.entities.{}.coverage", owner.id)), resource: Some(ResourceRef::Rule(lower.rule)),
                    });
                }
            }
        }
    }
    Ok(())
}

fn finding(
    report: &mut DomainValidationReport,
    code: &str,
    rule: RuleId,
    owner: Owner,
    shift: ShiftId,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    budget.reserve(1, 5, 768)?;
    report.issues.push(ValidationIssue {
        code: format!("official.workforce.{code}"), severity: ValidationSeverity::Error,
        message: format!("Required coverage has an obvious contradiction (owner {} {}, shift {shift}). This is not a solver feasibility result.", owner.kind, owner.id),
        field_path: Some(format!("domain.entities.{}.coverage", owner.id)), resource: Some(ResourceRef::Rule(rule)),
    });
    Ok(())
}

fn lock_finding(
    report: &mut DomainValidationReport,
    code: &str,
    lock: &AssignmentLock,
    other: Option<(&AssignmentLock, RuleId)>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    budget.reserve(1, 5, 768)?;
    report.issues.push(ValidationIssue {
        code: format!("official.workforce.{code}"),
        severity: ValidationSeverity::Error,
        message: other.map_or_else(
            || {
                format!(
                    "Hard lock {} requires review for person {} and shift {}.",
                    lock.id, lock.person_id, lock.shift_id
                )
            },
            |(other, rule)| {
                format!(
                    "Hard locks {} and {} conflict under active NoOverlap rule {rule}.",
                    lock.id, other.id
                )
            },
        ),
        field_path: Some(format!("domain.lockedAssignments.{}", lock.id)),
        resource: Some(ResourceRef::Assignment(lock.id)),
    });
    Ok(())
}

fn hard_lock_findings(
    input: &AssignmentInput,
    result: &mut AssignmentAnalysis,
    plan: &Plan,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut locks = Vec::new();
    let mut pairs = BTreeSet::new();
    for lock in input.domain.locked_assignments.values() {
        budget.step()?;
        if !matches!(lock.state, LockState::Hard {}) {
            continue;
        }
        budget.reserve(1, 3, 64)?;
        locks.push(lock);
        let pair = AssignmentPair {
            person_id: lock.person_id,
            shift_id: lock.shift_id,
        };
        pairs.insert(pair);
        if input.shift(lock.shift_id).is_none() {
            lock_finding(
                &mut result.validation,
                "hard_lock_unresolved_shift",
                lock,
                None,
                budget,
            )?;
        } else if result.candidates.binary_search(&pair).is_err() {
            lock_finding(
                &mut result.validation,
                "hard_lock_rejected_pair",
                lock,
                None,
                budget,
            )?;
        }
    }
    budget.sort_work(locks.len())?;
    locks.sort_unstable_by_key(|lock| {
        (
            lock.person_id,
            input
                .shift(lock.shift_id)
                .map(|shift| shift.interval.starts_at.instant),
            lock.shift_id,
            lock.id,
        )
    });
    locked_overlap_findings(input, &locks, &mut result.validation, budget)?;
    rest::locked_rest_findings(input, &locks, &mut result.validation, budget)?;
    locked_coverage_findings(input, &pairs, plan, &mut result.validation, budget)
}

fn locked_overlap_findings(
    input: &AssignmentInput,
    locks: &[&AssignmentLock],
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut overlap_rules = Vec::new();
    for rule in input.domain.rules.values() {
        budget.step()?;
        if matches!(rule, WorkforceRule::NoOverlap { active: true, .. }) {
            budget.reserve(1, 1, 16)?;
            overlap_rules.push(rule);
        }
    }
    if overlap_rules.is_empty() {
        return Ok(());
    }
    for (position, first) in locks.iter().enumerate() {
        budget.step()?;
        let Some(first_shift) = input.shift(first.shift_id) else {
            continue;
        };
        for second in &locks[position + 1..] {
            budget.step()?;
            if first.person_id != second.person_id {
                break;
            }
            if first.shift_id == second.shift_id {
                continue;
            }
            let Some(second_shift) = input.shift(second.shift_id) else {
                continue;
            };
            if second_shift.interval.starts_at.instant >= first_shift.interval.ends_at.instant {
                break;
            }
            let person = input.person(first.person_id).ok_or_else(invalid)?;
            for &rule in &overlap_rules {
                budget.step()?;
                if let WorkforceRule::NoOverlap {
                    id,
                    active: true,
                    scope,
                    compatible_category_pairs,
                    ..
                } = rule
                    && person_scope(scope, person, budget)?
                    && shift_scope(scope, first_shift, &input.metadata(first_shift)?, budget)?
                    && shift_scope(scope, second_shift, &input.metadata(second_shift)?, budget)?
                    && incompatible(
                        input,
                        first_shift,
                        second_shift,
                        compatible_category_pairs,
                        budget,
                    )?
                {
                    lock_finding(
                        report,
                        "hard_lock_overlap",
                        first,
                        Some((second, *id)),
                        budget,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn locked_coverage_findings(
    input: &AssignmentInput,
    pairs: &BTreeSet<AssignmentPair>,
    plan: &Plan,
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for constraint in &plan.constraints {
        budget.step()?;
        let Predicate::Headcount {
            definition,
            shift,
            authored_upper: Some(upper),
            ..
        } = constraint.predicate
        else {
            continue;
        };
        let rule = input
            .domain
            .rules
            .get(&constraint.rule)
            .ok_or_else(invalid)?;
        let (_, _, scope) = rule.header();
        let mut selected = 0;
        for pair in pairs {
            budget.step()?;
            if pair.shift_id == shift
                && person_scope(
                    scope,
                    input.person(pair.person_id).ok_or_else(invalid)?,
                    budget,
                )?
            {
                selected = add(selected, 1)?;
            }
        }
        if selected > upper {
            finding(
                report,
                "hard_locked_coverage_excess",
                constraint.rule,
                plan.definitions[definition].owner,
                shift,
                budget,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AssignmentAnalysis, AssignmentInput, AssignmentModelEstimate, AssignmentPair,
        AssignmentRuleError, DomainValidationReport, OperationBudget, PairContext, obligations,
    };
    use crate::test_support::{fixture, id};
    use eutheto_planning_ir::PlanningIrLimitsV1;
    use eutheto_types::{CancellationToken, ScenarioDocument};
    use serde_json::json;

    fn leaf_document(leaf: &str) -> Result<ScenarioDocument, Box<dyn std::error::Error>> {
        let mut document = fixture()?;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        let mut references = Vec::new();
        for index in 0..1_000 {
            let reference = id(1_000 + index);
            if leaf.ends_with("QualificationIds") {
                document.domain.entities.insert(
                    reference.parse()?,
                    json!({"kind":"qualification","id":reference,"name":"Leaf","description":""}),
                );
            } else if leaf == "teamIds" {
                document.domain.entities.insert(
                    reference.parse()?,
                    json!({"kind":"team","id":reference,"name":"Leaf"}),
                );
            }
            references.push(reference);
        }
        let person = document
            .domain
            .entities
            .get_mut(&id(1).parse()?)
            .ok_or("person")?;
        person["qualificationGrants"] = json!([]);
        person["tags"] = json!([]);
        person["teamIds"] = json!([]);
        let mut scope = json!({"people":{"kind":"all"}});
        if leaf.ends_with("QualificationIds") {
            let assignment_type = document
                .domain
                .entities
                .get_mut(&id(4).parse()?)
                .ok_or("type")?;
            assignment_type["qualifications"] = json!({
                "kind":"matches","allQualificationIds":[],"anyQualificationIds":[],
            });
            assignment_type["qualifications"][leaf] = json!(references);
        } else if leaf == "teamIds" {
            scope["teamIds"] = json!(references);
        } else {
            scope["people"] = json!({"kind":"filter","allTags":[],"anyTags":[]});
            scope["people"][leaf] = json!(references);
        }
        document.domain.rules.insert(
            id(20).parse()?,
            json!({
                "kind":"eligibility","id":id(20),"active":true,"strength":"required","scope":scope,
            }),
        );
        Ok(document)
    }

    #[test]
    fn cancellation_inside_empty_inner_searches_requires_each_outer_leaf_checkpoint()
    -> Result<(), Box<dyn std::error::Error>> {
        for leaf in [
            "allQualificationIds",
            "anyQualificationIds",
            "allTags",
            "anyTags",
            "teamIds",
        ] {
            let document = leaf_document(leaf)?;
            let token = CancellationToken::new();
            let mut budget = OperationBudget::analysis(Some(&token), PlanningIrLimitsV1::DEFAULT);
            let input = AssignmentInput::new(&document, &mut budget)?;
            let mut result = AssignmentAnalysis {
                source_document_hash: String::new(),
                candidates: Vec::new(),
                rejections: Vec::new(),
                estimate: AssignmentModelEstimate::default(),
                validation: DomainValidationReport::default(),
                obligations: obligations(&input, &mut budget)?,
            };
            let person = input.person(id(1).parse()?).ok_or("resolved person")?;
            let shift = input.shifts.first().ok_or("resolved shift")?;
            let context = PairContext {
                document: &document,
                input: &input,
                person,
                shift,
                metadata: input.metadata(shift)?,
                pair: AssignmentPair {
                    person_id: person.id,
                    shift_id: shift.id,
                },
            };
            // Arm only after decode and pair setup. The other checkpoints for this one
            // pair total fewer than 128: deleting this leaf's checkpoint makes the
            // operation succeed without cancellation, so this regression then fails.
            budget.cancel_after_steps(128)?;
            assert_eq!(
                context.analyze(None, &mut result, &mut budget),
                Err(AssignmentRuleError::Cancelled)
            );
            assert!(token.is_cancelled());
        }
        Ok(())
    }
}
