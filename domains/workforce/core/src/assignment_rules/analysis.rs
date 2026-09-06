#[path = "compiler_support.rs"]
pub(super) mod support;

use super::{
    AssignmentAnalysis, AssignmentConstructionIssue, AssignmentModelEstimate, AssignmentRuleError,
    AssignmentRuleLimit, PairRejection, RejectionCause, RequiredRulePartition,
    budget::{MAX_INSPECTED_PAIRS, OperationBudget, add, count, within},
    input::{AssignmentInput, ShiftDefinition}, intervals::availability_intervals,
};
use crate::{ids::ShiftId, model::*};
use eutheto_domain_api::DomainValidationReport;
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{CancellationToken, EntityId, PersonId, ResourceRef, RuleId, ScenarioDocument, ValidationIssue, ValidationSeverity};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use support::*;

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
    Headcount { definition: usize, shift: ShiftId, lower: u64, upper: u64, authored_upper: Option<u64> },
    Qualification { definition: usize, minimum: usize, shift: ShiftId, upper: u64 },
    Overlap { person: PersonId, first: ShiftId, second: ShiftId },
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
pub fn analyze_assignments(document: &ScenarioDocument, cancellation: Option<&CancellationToken>, limits: PlanningIrLimitsV1) -> Result<AssignmentAnalysis, AssignmentRuleError> {
    let mut budget = OperationBudget::analysis(cancellation, limits);
    let input = AssignmentInput::new(document, &mut budget)?;
    let (analysis, plan) = prepare(document, &input, &mut budget, limits)?;
    super::compiler::preflight(&analysis.candidates, &plan, &mut budget, limits)?;
    Ok(analysis)
}

pub(super) fn prepare(document: &ScenarioDocument, input: &AssignmentInput, budget: &mut OperationBudget<'_>, limits: PlanningIrLimitsV1) -> Result<(AssignmentAnalysis, Plan), AssignmentRuleError> {
    budget.check()?;
    let raw = count(input.people.len())?.checked_mul(count(input.shifts.len())?).ok_or(AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow))?;
    within(raw, MAX_INSPECTED_PAIRS, AssignmentRuleLimit::InspectedPairs)?;
    let obligations = obligations(input, budget)?;
    let mut result = AssignmentAnalysis {
        source_document_hash: String::new(), candidates: Vec::new(), rejections: Vec::new(),
        estimate: AssignmentModelEstimate { inspected_pairs: raw, ..AssignmentModelEstimate::default() },
        validation: DomainValidationReport::default(), obligations,
    };
    for id in &input.people {
        budget.step()?;
        let person = input.person(*id).ok_or_else(invalid)?;
        let active = active_interval(person, &document.settings)?;
        for shift in &input.shifts {
            budget.step()?;
            let pair = AssignmentPair { person_id: *id, shift_id: shift.id };
            let query = interval(shift);
            let metadata = input.metadata(shift)?;
            let mut activity_ok = true;
            let mut type_ok = true;
            let mut qualifications_ok = true;
            let mut availability_ok = true;
            if let Some(allowed) = active {
                if let Some(outside) = outside(query, allowed) {
                    activity_ok = false;
                    reject(&mut result, pair, RuleId::from_uuid(id.as_uuid()), RejectionCause::OutsideActiveRange { allowed, outside }, budget)?;
                }
            }
            for rule in input.domain.rules.values() {
                budget.step()?;
                let (binding, active, scope) = rule.header();
                if !active || !matches!(rule, WorkforceRule::Eligibility { .. } | WorkforceRule::Availability { .. }) { continue; }
                if !person_scope(scope, person, budget)? || !shift_scope(scope, shift, &metadata, budget)? { continue; }
                match rule {
                    WorkforceRule::Eligibility { .. } => {
                        budget.steps(count(person.eligible_assignment_type_ids.len())?)?;
                        if !person.eligible_assignment_type_ids.contains(&metadata.assignment_type.id) {
                            type_ok = false;
                            reject(&mut result, pair, binding, RejectionCause::AssignmentTypeNotAllowed { assignment_type_id: metadata.assignment_type.id }, budget)?;
                        }
                        if !expression(person, &metadata.assignment_type.qualifications, query, budget)? {
                            qualifications_ok = false;
                            reject(&mut result, pair, binding, RejectionCause::QualificationExpression { assignment_type_id: metadata.assignment_type.id }, budget)?;
                        }
                    }
                    WorkforceRule::Availability { .. } => {
                        if let Some(records) = input.availability_by_person.get(id) {
                            for availability_id in records {
                                budget.step()?;
                                let Some(WorkforceEntity::Availability(record)) = input.domain.entities.get(&availability_id.as_entity_id()) else { return Err(invalid()); };
                                if !matches!(record.availability_kind, AvailabilityKind::Unavailable | AvailabilityKind::AvailableOnly) || !availability_scope(record, &metadata, budget)? { continue; }
                                if let Some(cause) = availability_cause(record, query, document, budget)? {
                                    availability_ok = false;
                                    reject(&mut result, pair, binding, cause, budget)?;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Approved leave is independently owned and does not depend on an Availability rule.
            if let Some(records) = input.availability_by_person.get(id) {
                for availability_id in records {
                    budget.step()?;
                    let Some(WorkforceEntity::Availability(record)) = input.domain.entities.get(&availability_id.as_entity_id()) else { return Err(invalid()); };
                    if record.availability_kind != AvailabilityKind::ApprovedTimeOff || !availability_scope(record, &metadata, budget)? { continue; }
                    if let Some(cause) = availability_cause(record, query, document, budget)? {
                        availability_ok = false;
                        reject(&mut result, pair, RuleId::from_uuid(record.id.as_uuid()), cause, budget)?;
                    }
                }
            }
            if activity_ok { result.estimate.after_activity_pruning = add(result.estimate.after_activity_pruning, 1)?; }
            if activity_ok && type_ok { result.estimate.after_assignment_type_pruning = add(result.estimate.after_assignment_type_pruning, 1)?; }
            if activity_ok && type_ok && qualifications_ok { result.estimate.after_qualification_pruning = add(result.estimate.after_qualification_pruning, 1)?; }
            if activity_ok && type_ok && qualifications_ok && availability_ok {
                within(add(count(result.candidates.len())?, 1)?, budget.variable_limit(), AssignmentRuleLimit::Variables)?;
                budget.reserve(1, 2, budget.measure(&pair)?)?;
                result.candidates.push(pair);
            }
        }
    }
    result.candidates.sort_unstable();
    result.rejections.sort_unstable();
    result.estimate.after_availability_pruning = count(result.candidates.len())?;
    result.estimate.variables = count(result.candidates.len())?;
    result.estimate.rejection_facts = count(result.rejections.len())?;
    let plan = plan(input, &mut result, budget, limits)?;
    hard_lock_findings(input, &mut result, &plan, budget)?;
    result.validation.issues.sort_by(|a, b| (&a.code, &a.field_path, &a.message).cmp(&(&b.code, &b.field_path, &b.message)));
    result.validation.issues.dedup();
    // The hash was measured by AssignmentInput; this is the only retained additional copy.
    budget.reserve(0, 0, 64)?;
    result.source_document_hash.clone_from(&input.source_document_hash);
    budget.check()?;
    Ok((result, plan))
}

fn availability_cause(record: &Availability, query: super::InstantInterval, document: &ScenarioDocument, budget: &mut OperationBudget<'_>) -> Result<Option<RejectionCause>, AssignmentRuleError> {
    let windows = availability_intervals(record, query, &document.settings, budget)?;
    let Some(query) = windows.query else { return Ok(None); };
    match record.availability_kind {
        AvailabilityKind::AvailableOnly => Ok(uncovered(query, &windows.intervals, budget)?.map(|uncovered| RejectionCause::OutsideAvailableOnly { availability_id: record.id, uncovered })),
        AvailabilityKind::Unavailable => Ok(windows.intervals.first().copied().map(|overlap| RejectionCause::Unavailable { availability_id: record.id, overlap })),
        AvailabilityKind::ApprovedTimeOff => Ok(windows.intervals.first().copied().map(|overlap| RejectionCause::ApprovedTimeOff { availability_id: record.id, overlap })),
        AvailabilityKind::RequestedTimeOff => Ok(None),
    }
}

fn reject(result: &mut AssignmentAnalysis, pair: AssignmentPair, binding_id: RuleId, cause: RejectionCause, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    // The largest fixed cause has four timestamp endpoints plus three UUIDs. There are no
    // copied strings, expressions, grants or window lists in this bounded sidecar record.
    budget.reserve(1, 8, 512)?;
    result.rejections.push(PairRejection { pair, binding_id, cause });
    Ok(())
}

fn obligations(input: &AssignmentInput, budget: &mut OperationBudget<'_>) -> Result<RequiredRulePartition, AssignmentRuleError> {
    let mut result = RequiredRulePartition { handled: Vec::new(), remaining: Vec::new() };
    for rule in input.domain.rules.values() {
        budget.step()?;
        let (id, active, _) = rule.header();
        if !active { continue; }
        budget.reserve(0, 1, 16)?;
        if matches!(rule, WorkforceRule::Eligibility { .. } | WorkforceRule::Availability { .. } | WorkforceRule::Coverage { .. } | WorkforceRule::NoOverlap { .. }) { result.handled.push(id); }
        else { result.remaining.push(id); }
    }
    for entity in input.domain.entities.values() {
        budget.step()?;
        let id = match entity {
            WorkforceEntity::Person(person) => Some(RuleId::from_uuid(person.id.as_uuid())),
            WorkforceEntity::Availability(record) if record.availability_kind == AvailabilityKind::ApprovedTimeOff => Some(RuleId::from_uuid(record.id.as_uuid())),
            _ => None,
        };
        if let Some(id) = id { budget.reserve(0, 1, 16)?; result.handled.push(id); }
    }
    for lock in input.domain.locked_assignments.values() {
        budget.step()?;
        if matches!(lock.state, LockState::Hard {}) { budget.reserve(0, 1, 16)?; result.remaining.push(RuleId::from_uuid(lock.id.as_uuid())); }
    }
    result.handled.sort_unstable(); result.handled.dedup();
    result.remaining.sort_unstable(); result.remaining.dedup();
    Ok(result)
}

fn definition_owner(definition: ShiftDefinition) -> Owner {
    Owner { kind: match definition { ShiftDefinition::Template(_) => "shift_template", ShiftDefinition::Instance(_) => "shift_instance" }, id: definition.entity_id() }
}

fn plan(input: &AssignmentInput, result: &mut AssignmentAnalysis, budget: &mut OperationBudget<'_>, limits: PlanningIrLimitsV1) -> Result<Plan, AssignmentRuleError> {
    let mut plan = Plan { definitions: Vec::new(), constraints: Vec::new(), parents: BTreeMap::new() };
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
        if !active { continue; }
        match rule {
            WorkforceRule::Coverage { .. } => {
                for shift in &input.shifts {
                    budget.step()?;
                    let metadata = input.metadata(shift)?;
                    if !shift_scope(scope, shift, &metadata, budget)? { continue; }
                    let mut owners = Vec::new();
                    budget.reserve(1, 1, 32)?;
                    owners.push((definition_owner(metadata.definition), metadata.coverage));
                    for entity in input.domain.entities.values() {
                        budget.step()?;
                        if let WorkforceEntity::CoverageRequirement(requirement) = entity {
                            if requirement.active && requirement_scope(&requirement.scope, shift, &metadata, budget)? {
                                budget.reserve(1, 1, 32)?;
                                owners.push((Owner { kind: "coverage_requirement", id: requirement.id.as_entity_id() }, &requirement.coverage));
                            }
                        }
                    }
                    owners.sort_by_key(|(owner, _)| *owner);
                    for (owner, coverage) in owners {
                        budget.step()?;
                        let definition = if let Some(index) = definitions.get(&owner) { *index } else {
                            let index = plan.definitions.len();
                            let minima = canonical_minima(coverage, budget)?;
                            budget.reserve(2, 2, 64)?;
                            plan.definitions.push(Definition { owner, minima });
                            definitions.insert(owner, index);
                            index
                        };
                        let mut population = Vec::new();
                        if let Some(indices) = by_shift.get(&shift.id) {
                            for index in indices {
                                budget.step()?;
                                let person = input.person(result.candidates[*index].person_id).ok_or_else(invalid)?;
                                if person_scope(scope, person, budget)? { budget.reserve(0, 1, 8)?; population.push(*index); }
                            }
                        }
                        let (lower, authored_upper) = bounds(coverage);
                        let n = count(population.len())?;
                        let upper = authored_upper.unwrap_or(n).min(n);
                        if lower > n { finding(&mut result.validation, "candidate_shortage", id, owner, shift.id, budget)?; }
                        if lower > upper && lower <= n { finding(&mut result.validation, "contradictory_coverage_bounds", id, owner, shift.id, budget)?; }
                        let head_population = population;
                        for minimum in 0..plan.definitions[definition].minima.len() {
                            let key = &plan.definitions[definition].minima[minimum];
                            let mut qualified = Vec::new();
                            for index in &head_population {
                                budget.step()?;
                                let person = input.person(result.candidates[*index].person_id).ok_or_else(invalid)?;
                                if qualification_match(person, &key.all, &key.any, interval(shift), budget)? { budget.reserve(0, 1, 8)?; qualified.push(*index); }
                            }
                            let qualified_count = count(qualified.len())?;
                            if u64::from(key.minimum) > qualified_count { finding(&mut result.validation, "qualification_shortage", id, owner, shift.id, budget)?; }
                            if authored_upper.is_some_and(|upper| u64::from(key.minimum) > upper) { finding(&mut result.validation, "qualification_above_headcount", id, owner, shift.id, budget)?; }
                            let impossible = u64::from(key.minimum) > qualified_count;
                            append(&mut plan, PlannedConstraint { rule: id, predicate: Predicate::Qualification { definition, minimum, shift: shift.id, upper: qualified_count }, population: qualified, impossible }, budget)?;
                        }
                        append(&mut plan, PlannedConstraint { rule: id, predicate: Predicate::Headcount { definition, shift: shift.id, lower, upper, authored_upper }, population: head_population, impossible: lower > upper }, budget)?;
                    }
                }
            }
            WorkforceRule::NoOverlap { compatible_category_pairs, .. } => {
                for person_id in &input.people {
                    budget.step()?;
                    let person = input.person(*person_id).ok_or_else(invalid)?;
                    if !person_scope(scope, person, budget)? { continue; }
                    let mut chronological = Vec::new();
                    let start = result.candidates.partition_point(|pair| pair.person_id < *person_id);
                    for (offset, pair) in result.candidates[start..].iter().enumerate() {
                        budget.step()?;
                        if pair.person_id != *person_id { break; }
                        let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
                        if shift_scope(scope, shift, &input.metadata(shift)?, budget)? { budget.reserve(0, 1, 8)?; chronological.push(start + offset); }
                    }
                    chronological.sort_unstable_by_key(|index| {
                        let pair = result.candidates[*index];
                        (input.shift(pair.shift_id).map(|shift| shift.interval.starts_at.instant), pair.shift_id)
                    });
                    for (position, first_index) in chronological.iter().enumerate() {
                        let first = input.shift(result.candidates[*first_index].shift_id).ok_or_else(invalid)?;
                        for second_index in &chronological[position + 1..] {
                            budget.step()?;
                            let second = input.shift(result.candidates[*second_index].shift_id).ok_or_else(invalid)?;
                            if second.interval.starts_at.instant >= first.interval.ends_at.instant { break; }
                            if incompatible(input, first, second, compatible_category_pairs, budget)? {
                                budget.reserve(0, 2, 16)?;
                                append(&mut plan, PlannedConstraint { rule: id, predicate: Predicate::Overlap { person: *person_id, first: first.id.min(second.id), second: first.id.max(second.id) }, population: vec![*first_index, *second_index], impossible: false }, budget)?;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    coverage_bound_findings(&plan, &mut result.validation, budget)?;
    let variables = result.estimate.variables;
    let constraints = count(plan.constraints.len())?;
    let provenance = add(add(variables, constraints)?, count(plan.parents.len())?)?;
    within(provenance, budget.provenance_limit(), AssignmentRuleLimit::ProvenanceRecords)?;
    let mut references = variables.checked_mul(3).ok_or_else(invalid)?; // variable -> fact; fact -> person/shift
    for constraint in &plan.constraints {
        budget.step()?;
        let literal_count = if constraint.impossible { 0 } else { count(constraint.population.len())? };
        within(literal_count, limits.max_refs_per_node.min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node), AssignmentRuleLimit::PerRecord)?;
        let (entities, parameters) = predicate_shape(&constraint.predicate, &plan)?;
        within(entities, limits.max_entity_refs_per_record.min(PlanningIrLimitsV1::DEFAULT.max_entity_refs_per_record), AssignmentRuleLimit::PerRecord)?;
        within(parameters, limits.max_parameters_per_record.min(PlanningIrLimitsV1::DEFAULT.max_parameters_per_record), AssignmentRuleLimit::PerRecord)?;
        references = add(references, add(literal_count, add(2, add(entities, parameters)?)?)?)?;
    }
    within(references, limits.max_total_refs.min(PlanningIrLimitsV1::DEFAULT.max_total_refs), AssignmentRuleLimit::References)?;
    if variables > 0 {
        within(2, limits.max_entity_refs_per_record, AssignmentRuleLimit::PerRecord)?;
        within(1, limits.max_provenance_depth, AssignmentRuleLimit::PerRecord)?;
    }
    if constraints > 0 { within(2, limits.max_provenance_depth, AssignmentRuleLimit::PerRecord)?; }
    result.estimate.constraints = constraints;
    result.estimate.provenance_records = provenance;
    result.estimate.references = references;
    Ok(plan)
}

pub(super) fn predicate_shape(predicate: &Predicate, plan: &Plan) -> Result<(u64, u64), AssignmentRuleError> {
    Ok(match predicate {
        Predicate::Headcount { authored_upper, .. } => (2, 3 + u64::from(authored_upper.is_some())),
        Predicate::Qualification { definition, minimum, .. } => {
            let key = &plan.definitions[*definition].minima[*minimum];
            // all/any membership is retained as separately named typed Entity parameters.
            (2, add(1, count(key.all.len() + key.any.len())?)?)
        }
        Predicate::Overlap { .. } => (3, 0),
    })
}

fn append(plan: &mut Plan, constraint: PlannedConstraint, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    within(add(count(plan.constraints.len())?, 1)?, budget.constraint_limit(), AssignmentRuleLimit::Constraints)?;
    if !plan.parents.contains_key(&constraint.rule) {
        budget.reserve(1, 1, 64)?;
        plan.parents.insert(constraint.rule, if matches!(constraint.predicate, Predicate::Overlap { .. }) { "no_overlap" } else { "coverage" });
    }
    budget.reserve(1, 4, 128)?;
    plan.constraints.push(constraint);
    Ok(())
}

fn coverage_bound_findings(plan: &Plan, report: &mut DomainValidationReport, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    let mut by_shift = BTreeMap::<ShiftId, Vec<&PlannedConstraint>>::new();
    for constraint in &plan.constraints {
        budget.step()?;
        let shift = match constraint.predicate {
            Predicate::Headcount { shift, .. } | Predicate::Qualification { shift, .. } => shift,
            Predicate::Overlap { .. } => continue,
        };
        budget.reserve(1, 2, 24)?;
        by_shift.entry(shift).or_default().push(constraint);
    }
    for (shift, predicates) in by_shift {
        for lower in &predicates {
            let (definition, minimum, code) = match lower.predicate {
                Predicate::Headcount { definition, lower, .. } => (definition, lower, "contradictory_coverage_bounds"),
                Predicate::Qualification { definition, minimum, .. } => (definition, u64::from(plan.definitions[definition].minima[minimum].minimum), "qualification_above_headcount"),
                Predicate::Overlap { .. } => continue,
            };
            for upper in &predicates {
                budget.step()?;
                let Predicate::Headcount { definition: upper_definition, upper: maximum, .. } = upper.predicate else { continue; };
                if minimum <= maximum { continue; }
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
                    if upper.population.get(position) != Some(index) { subset = false; break; }
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

fn finding(report: &mut DomainValidationReport, code: &str, rule: RuleId, owner: Owner, shift: ShiftId, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    budget.reserve(1, 5, 768)?;
    report.issues.push(ValidationIssue {
        code: format!("official.workforce.{code}"), severity: ValidationSeverity::Error,
        message: format!("Required coverage has an obvious contradiction (owner {} {}, shift {shift}). This is not a solver feasibility result.", owner.kind, owner.id),
        field_path: Some(format!("domain.entities.{}.coverage", owner.id)), resource: Some(ResourceRef::Rule(rule)),
    });
    Ok(())
}

fn lock_finding(report: &mut DomainValidationReport, code: &str, lock: &AssignmentLock, other: Option<(&AssignmentLock, RuleId)>, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    budget.reserve(1, 5, 768)?;
    report.issues.push(ValidationIssue {
        code: format!("official.workforce.{code}"), severity: ValidationSeverity::Error,
        message: other.map_or_else(|| format!("Hard lock {} requires review for person {} and shift {}.", lock.id, lock.person_id, lock.shift_id), |(other, rule)| format!("Hard locks {} and {} conflict under active NoOverlap rule {rule}.", lock.id, other.id)),
        field_path: Some(format!("domain.lockedAssignments.{}", lock.id)), resource: Some(ResourceRef::Assignment(lock.id)),
    });
    Ok(())
}

fn hard_lock_findings(input: &AssignmentInput, result: &mut AssignmentAnalysis, plan: &Plan, budget: &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError> {
    let mut locks = Vec::new();
    let mut pairs = BTreeSet::new();
    for lock in input.domain.locked_assignments.values() {
        budget.step()?;
        if !matches!(lock.state, LockState::Hard {}) { continue; }
        budget.reserve(1, 3, 64)?;
        locks.push(lock);
        let pair = AssignmentPair { person_id: lock.person_id, shift_id: lock.shift_id };
        pairs.insert(pair);
        if input.shift(lock.shift_id).is_none() { lock_finding(&mut result.validation, "hard_lock_unresolved_shift", lock, None, budget)?; }
        else if result.candidates.binary_search(&pair).is_err() { lock_finding(&mut result.validation, "hard_lock_rejected_pair", lock, None, budget)?; }
    }
    locks.sort_unstable_by_key(|lock| (lock.person_id, input.shift(lock.shift_id).map(|shift| shift.interval.starts_at.instant), lock.shift_id, lock.id));
    for (position, first) in locks.iter().enumerate() {
        let Some(first_shift) = input.shift(first.shift_id) else { continue; };
        for second in &locks[position + 1..] {
            budget.step()?;
            if first.person_id != second.person_id { break; }
            if first.shift_id == second.shift_id { continue; }
            let Some(second_shift) = input.shift(second.shift_id) else { continue; };
            if second_shift.interval.starts_at.instant >= first_shift.interval.ends_at.instant { break; }
            let person = input.person(first.person_id).ok_or_else(invalid)?;
            for rule in input.domain.rules.values() {
                budget.step()?;
                if let WorkforceRule::NoOverlap { id, active: true, scope, compatible_category_pairs, .. } = rule {
                    if person_scope(scope, person, budget)? && shift_scope(scope, first_shift, &input.metadata(first_shift)?, budget)? && shift_scope(scope, second_shift, &input.metadata(second_shift)?, budget)? && incompatible(input, first_shift, second_shift, compatible_category_pairs, budget)? {
                        lock_finding(&mut result.validation, "hard_lock_overlap", first, Some((second, *id)), budget)?;
                    }
                }
            }
        }
    }
    for constraint in &plan.constraints {
        budget.step()?;
        let Predicate::Headcount { definition, shift, authored_upper: Some(upper), .. } = constraint.predicate else { continue; };
        let rule = input.domain.rules.get(&constraint.rule).ok_or_else(invalid)?;
        let (_, _, scope) = rule.header();
        let mut selected = 0;
        for pair in &pairs {
            budget.step()?;
            if pair.shift_id == shift && person_scope(scope, input.person(pair.person_id).ok_or_else(invalid)?, budget)? { selected = add(selected, 1)?; }
        }
        if selected > upper { finding(&mut result.validation, "hard_locked_coverage_excess", constraint.rule, plan.definitions[definition].owner, shift, budget)?; }
    }
    Ok(())
}
