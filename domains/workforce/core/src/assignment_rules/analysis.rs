#[path = "compiler_support.rs"]
pub(crate) mod support;

#[path = "compiler_rest.rs"]
mod rest;

#[path = "compiler_consecutive.rs"]
pub(super) mod consecutive;

#[path = "compiler_count.rs"]
mod count;
#[path = "compiler_mutual.rs"]
mod mutual;
#[path = "compiler_quota.rs"]
mod quota;

#[path = "compiler_overlap.rs"]
mod overlap;

#[path = "compiler_transition.rs"]
mod transition;

#[path = "compiler_hours.rs"]
pub(super) mod hours;

use super::compiler_weekend;
use super::weekends::WeekendPlan;
use super::{
    AssignmentAnalysis, AssignmentConstructionIssue, AssignmentModelEstimate, AssignmentRuleError,
    AssignmentRuleLimit, InstantInterval, PairRejection, RejectionCause, RequiredRulePartition,
    SelectionIssueKind,
    budget::{MAX_INSPECTED_PAIRS, OperationBudget, add, count, within},
    input::{AssignmentInput, ShiftDefinition, ShiftMetadata},
    intervals::availability_intervals,
    reporting::ReportingPeriod,
};
use crate::{
    ids::{LocationId, ShiftId, WorkCalendarId},
    model::{
        AssignmentLock, AssignmentPair, Availability, AvailabilityKind, CategoryPair, Coverage,
        LockState, PairingMode, Person, QualificationMinimum, RequiredStrength, Scope,
        WorkforceEntity, WorkforceRule, WorkloadQuotaMeasure,
    },
    temporal::ResolvedShift,
};
use eutheto_domain_api::DomainValidationReport;
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{
    AssignmentId, CancellationToken, EntityId, PersonId, ResourceRef, RuleId, ScenarioDocument,
    ValidationIssue, ValidationSeverity,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};
use support::{
    MinimumKey, active_interval, authored_source_rows, authored_source_rows_values,
    availability_scope, bounds, canonical_minima, canonical_minima_values, expression,
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
    /// Evidence-only mapping from canonical minimum index to the first authored
    /// `qualificationMinimums` row index. Never affects `MinimumKey` identity.
    pub source_rows: Vec<usize>,
    pub consecutive: Option<consecutive::ConsecutivePlan>,
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
    SkillMixQualification {
        definition: usize,
        minimum: usize,
        shift: ShiftId,
        upper: u64,
    },
    MaximumAssignmentCount {
        person: PersonId,
        calendar_id: WorkCalendarId,
        period: ReportingPeriod,
        maximum: u32,
    },
    WorkloadQuota {
        person: PersonId,
        calendar_id: WorkCalendarId,
        period: ReportingPeriod,
        minimum: u32,
        maximum: Option<u32>,
        measure: WorkloadQuotaMeasure,
    },
    Overlap {
        person: PersonId,
        first: ShiftId,
        second: ShiftId,
    },
    OverlapClique {
        person: PersonId,
    },
    MutualSameShift {
        shift: ShiftId,
    },
    MutualOverlap {
        first_person: PersonId,
        first_shift: ShiftId,
        second_person: PersonId,
        second_shift: ShiftId,
    },
    MinimumRest {
        person: PersonId,
        source: ShiftId,
        target: ShiftId,
        minimum_minutes: u32,
    },
    TransitionTime {
        person: PersonId,
        source_location: LocationId,
        target_location: LocationId,
        source: ShiftId,
        target: ShiftId,
        required_minutes: u32,
    },
    FixedAssignment {
        person: PersonId,
        shift: ShiftId,
        assigned: bool,
    },
    MaximumConsecutive {
        person: PersonId,
        definition: usize,
    },
    MaximumWorkedWeekends {
        person: PersonId,
        definition: usize,
    },
    HardLock {
        assignment: AssignmentId,
        person: PersonId,
        shift: ShiftId,
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
    pub weekends: Vec<WeekendPlan>,
    hours: Vec<hours::HoursPlan>,
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
    let control = cancellation
        .cloned()
        .map(eutheto_types::OperationControl::Cancellation);
    let mut budget = OperationBudget::analysis(control.as_ref(), limits);
    analyze_with_budget(document, &mut budget, limits)
}

pub(crate) fn analyze_with_budget(
    document: &ScenarioDocument,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<AssignmentAnalysis, AssignmentRuleError> {
    let input = AssignmentInput::new(document, budget)?;
    let (analysis, plan) = prepare(document, &input, budget, limits)?;
    super::compiler::preflight(&analysis.candidates, &plan, budget, limits)?;
    Ok(analysis)
}

/// Complete source analysis for report consumers, without measuring unused emitted IR.
pub(crate) fn analyze_source_with_budget(
    document: &ScenarioDocument,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<AssignmentAnalysis, AssignmentRuleError> {
    let input = AssignmentInput::new(document, budget)?;
    let (analysis, _) = prepare(document, &input, budget, limits)?;
    Ok(analysis)
}

/// Source identity and lower bound for one native `candidate_shortage` finding.
///
/// The renderer uses this only after the complete native issue has been selected by identity.
/// Matching the owning coverage source, field path, resource and message here prevents a
/// same-rule finding for another shift or coverage owner from being reused as provenance.
pub(crate) struct CandidateShortageSource {
    pub rule_id: RuleId,
    pub shift_id: ShiftId,
    pub required_count: u64,
}

pub(crate) fn candidate_shortage_source(
    input: &AssignmentInput,
    issue: &ValidationIssue,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<CandidateShortageSource>, AssignmentRuleError> {
    if issue.code != "official.workforce.candidate_shortage"
        || issue.severity != ValidationSeverity::Error
    {
        return Ok(None);
    }
    let Some(field_path) = issue.field_path.as_deref() else {
        return Ok(None);
    };
    let Some(ResourceRef::Rule(source_rule)) = issue.resource.as_ref() else {
        return Ok(None);
    };
    let mut found = None;
    for authored in input.domain.rules.values() {
        budget.step()?;
        let WorkforceRule::Coverage {
            id,
            active,
            strength: RequiredStrength::Required,
            scope,
            ..
        } = authored
        else {
            continue;
        };
        if !*active || id != source_rule {
            continue;
        }
        for shift in &input.shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !shift_scope(scope, shift, &metadata, budget)? {
                continue;
            }
            for (owner, coverage) in coverage_owners(input, shift, &metadata, budget)? {
                budget.step()?;
                let (required_count, _) = bounds(coverage);
                let count_field = match coverage {
                    Coverage::Exact { .. } => "count",
                    Coverage::AtLeast { .. } => "minimum",
                };
                // Bound temporary source-comparison strings as well as the eventual message.
                budget.reserve(0, 1, 128)?;
                let expected_path = format!("domain.entities.{}.coverage.{count_field}", owner.id);
                if field_path != expected_path.as_str() {
                    continue;
                }
                budget.reserve(0, 1, 512)?;
                let expected_message = finding_message(owner, shift.id);
                if issue.message != expected_message.as_str() {
                    continue;
                }
                if found.is_some() {
                    // A native issue identity should select exactly one source. Refuse an
                    // ambiguous provenance match rather than choosing one arbitrarily.
                    return Ok(None);
                }
                found = Some(CandidateShortageSource {
                    rule_id: *id,
                    shift_id: shift.id,
                    required_count,
                });
            }
        }
    }
    Ok(found)
}

pub(super) fn prepare(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(AssignmentAnalysis, Plan), AssignmentRuleError> {
    prepare_with_restoration(document, input, budget, limits, None)
}

/// Prepare the diagnostic candidate graph. Authored Eligibility/Availability rejections are
/// restored before any rule populations are planned; activity and approved-leave facts remain
/// structural exclusions.
pub(super) fn prepare_infeasibility(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
    relaxable: &BTreeSet<RuleId>,
) -> Result<(AssignmentAnalysis, Plan), AssignmentRuleError> {
    prepare_with_restoration(document, input, budget, limits, Some(relaxable))
}

fn prepare_with_restoration(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
    relaxable: Option<&BTreeSet<RuleId>>,
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
    if let Some(relaxable) = relaxable {
        restore_authored_candidates(&mut result, input, relaxable, budget)?;
    }
    budget.sort_work(result.candidates.len())?;
    result.candidates.sort_unstable();
    budget.sort_work(result.rejections.len())?;
    result.rejections.sort_unstable();
    result.estimate.after_availability_pruning = count(result.candidates.len())?;
    result.estimate.variables = count(result.candidates.len())?;
    result.estimate.rejection_facts = count(result.rejections.len())?;
    let plan = plan(document, input, &mut result, budget, limits)?;
    hard_lock_findings(document, input, &mut result, &plan, budget)?;
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

fn restore_authored_candidates(
    result: &mut AssignmentAnalysis,
    input: &AssignmentInput,
    relaxable: &BTreeSet<RuleId>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut authored = BTreeSet::new();
    let mut facts = BTreeSet::new();
    for rejection in &result.rejections {
        budget.step()?;
        let authored_rejection = relaxable.contains(&rejection.binding_id)
            && matches!(
                rejection.cause,
                RejectionCause::AssignmentTypeNotAllowed { .. }
                    | RejectionCause::QualificationExpression { .. }
                    | RejectionCause::Unavailable { .. }
                    | RejectionCause::OutsideAvailableOnly { .. }
            );
        if authored_rejection {
            budget.reserve(0, 1, 16)?;
            authored.insert(rejection.pair);
        } else {
            // Activity and approved leave are intentionally not relaxable. Any other
            // rejection is likewise retained as a structural fact rather than guessed.
            budget.reserve(0, 1, 16)?;
            facts.insert(rejection.pair);
        }
    }
    for pair in authored {
        budget.step()?;
        if facts.contains(&pair) || result.candidates.contains(&pair) {
            continue;
        }
        if input.person(pair.person_id).is_none() || input.shift(pair.shift_id).is_none() {
            return Err(AssignmentRuleError::InvalidSelection {
                pair,
                kind: SelectionIssueKind::UnresolvedShift,
            });
        }
        budget.reserve(1, 2, budget.measure(&pair)?)?;
        result.candidates.push(pair);
    }
    Ok(())
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

pub(crate) fn supported_rule(rule: &WorkforceRule) -> bool {
    matches!(
        rule,
        WorkforceRule::Eligibility { .. }
            | WorkforceRule::Availability { .. }
            | WorkforceRule::Coverage { .. }
            | WorkforceRule::NoOverlap { .. }
            | WorkforceRule::MinimumRest(_)
            | WorkforceRule::WorkloadQuota(_)
            | WorkforceRule::MaximumAssignmentCount { .. }
            | WorkforceRule::MaximumConsecutive { .. }
            | WorkforceRule::MaximumWorkedWeekends { .. }
            | WorkforceRule::MaximumHours { .. }
            | WorkforceRule::RequiredSkillMix { .. }
            | WorkforceRule::FixedAssignment { .. }
            | WorkforceRule::MutualAssignmentRestriction { .. }
            | WorkforceRule::TransitionTime { .. }
    )
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
        if supported_rule(rule) {
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
            result.handled.push(RuleId::from_uuid(lock.id.as_uuid()));
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

// One candidate graph and its ordered rule plans share the same bounded source analysis.
#[allow(clippy::too_many_lines)]
fn plan(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    result: &mut AssignmentAnalysis,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<Plan, AssignmentRuleError> {
    let mut plan = Plan {
        definitions: Vec::new(),
        constraints: Vec::new(),
        weekends: Vec::new(),
        hours: Vec::new(),
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
            WorkforceRule::RequiredSkillMix {
                qualification_minimums,
                ..
            } => {
                SkillMixPlanner {
                    input,
                    result,
                    plan: &mut plan,
                    definitions: &mut definitions,
                    by_shift: &by_shift,
                }
                .add_rule(id, scope, qualification_minimums, budget)?;
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
            WorkforceRule::MutualAssignmentRestriction { .. } => mutual::plan(
                input,
                &result.candidates,
                &by_shift,
                &mut plan,
                rule,
                budget,
                limits,
            )?,
            WorkforceRule::MinimumRest(rest) => rest::RestRule {
                id,
                scope,
                after_scope: &rest.after_scope,
                before_scope: &rest.before_scope,
                minimum_minutes: rest.minimum_minutes,
            }
            .plan(input, &result.candidates, &mut plan, budget)?,
            WorkforceRule::TransitionTime { .. } => {
                transition::plan(input, &result.candidates, &mut plan, rule, budget)?;
            }
            WorkforceRule::WorkloadQuota(_) => {
                quota::plan(
                    document,
                    input,
                    &result.candidates,
                    rule,
                    &mut plan,
                    budget,
                    limits,
                )?;
            }
            WorkforceRule::MaximumAssignmentCount { .. } => {
                count::plan(document, input, &result.candidates, rule, &mut plan, budget)?;
            }
            WorkforceRule::MaximumConsecutive { .. } => {
                consecutive::plan(input, &result.candidates, &mut plan, rule, budget, limits)?;
            }
            WorkforceRule::MaximumWorkedWeekends { .. } => {
                compiler_weekend::plan(
                    input,
                    &result.candidates,
                    rule,
                    &mut plan,
                    &document.settings,
                    budget,
                )?;
            }
            WorkforceRule::MaximumHours { .. } => {
                hours::plan(
                    document,
                    input,
                    &result.candidates,
                    rule,
                    &mut plan,
                    budget,
                    limits,
                )?;
            }
            WorkforceRule::FixedAssignment { .. } => plan_fixed(
                input,
                &result.candidates,
                &mut plan,
                &mut result.validation,
                rule,
                budget,
            )?,
            _ => {}
        }
    }
    plan_hard_locks(input, &result.candidates, &mut plan, budget)?;
    coverage_bound_findings(&plan, &mut result.validation, budget)?;
    estimate_contribution(result, &plan, budget, limits)?;
    Ok(plan)
}

fn plan_hard_locks(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    plan: &mut Plan,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for lock in input.domain.locked_assignments.values() {
        budget.step()?;
        if !matches!(lock.state, LockState::Hard {}) {
            continue;
        }
        let pair = AssignmentPair {
            person_id: lock.person_id,
            shift_id: lock.shift_id,
        };
        budget.reserve(1, 3, 64)?;
        let candidate = candidates.binary_search(&pair);
        let (population, impossible) = match candidate {
            Ok(index) => {
                budget.reserve(0, 1, 8)?;
                (vec![index], false)
            }
            Err(_) => (Vec::new(), true),
        };
        append(
            plan,
            PlannedConstraint {
                rule: RuleId::from_uuid(lock.id.as_uuid()),
                predicate: Predicate::HardLock {
                    assignment: lock.id,
                    person: lock.person_id,
                    shift: lock.shift_id,
                },
                population,
                impossible,
            },
            budget,
        )?;
    }
    Ok(())
}

fn plan_fixed(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    plan: &mut Plan,
    report: &mut DomainValidationReport,
    fixed: &WorkforceRule,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::FixedAssignment {
        id: rule,
        scope,
        person_id,
        shift_id,
        assigned,
        ..
    } = fixed
    else {
        return Err(invalid());
    };
    let (rule, person_id, shift_id, assigned) = (*rule, *person_id, *shift_id, *assigned);
    budget.step()?;
    let pair = AssignmentPair {
        person_id,
        shift_id,
    };
    let person = input.person(person_id);
    let shift = input.shift(shift_id);
    let missing = if person.is_none() {
        Some(("fixed_assignment_unresolved_person", "personId"))
    } else if shift.is_none() {
        Some(("fixed_assignment_unresolved_shift", "shiftId"))
    } else {
        None
    };
    if let Some((code, field)) = missing {
        fixed_finding(
            report,
            code,
            rule,
            pair,
            &format!("domain.rules.{rule}.{field}"),
            budget,
        )?;
        return append(
            plan,
            PlannedConstraint {
                rule,
                predicate: Predicate::FixedAssignment {
                    person: person_id,
                    shift: shift_id,
                    assigned,
                },
                population: Vec::new(),
                impossible: assigned,
            },
            budget,
        );
    }
    let person = person.ok_or_else(invalid)?;
    let shift = shift.ok_or_else(invalid)?;
    let metadata = input.metadata(shift)?;
    let in_scope =
        person_scope(scope, person, budget)? && shift_scope(scope, shift, &metadata, budget)?;
    let (population, impossible) = if in_scope {
        if let Ok(index) = candidates.binary_search(&pair) {
            budget.reserve(0, 1, 8)?;
            (vec![index], false)
        } else {
            if assigned {
                fixed_finding(
                    report,
                    "fixed_assignment_rejected_pair",
                    rule,
                    pair,
                    &format!("domain.rules.{rule}"),
                    budget,
                )?;
            }
            (Vec::new(), assigned)
        }
    } else {
        fixed_finding(
            report,
            "fixed_assignment_out_of_scope",
            rule,
            pair,
            &format!("domain.rules.{rule}.scope"),
            budget,
        )?;
        (Vec::new(), assigned)
    };
    append(
        plan,
        PlannedConstraint {
            rule,
            predicate: Predicate::FixedAssignment {
                person: person_id,
                shift: shift_id,
                assigned,
            },
            population,
            impossible,
        },
        budget,
    )
}

fn fixed_finding(
    report: &mut DomainValidationReport,
    code: &str,
    rule: RuleId,
    pair: AssignmentPair,
    field_path: &str,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    budget.reserve(1, 5, 768)?;
    report.issues.push(ValidationIssue {
        code: format!("official.workforce.{code}"),
        severity: ValidationSeverity::Error,
        message: format!(
            "Fixed assignment rule {rule} requires person {} on shift {} but its authored target is not a viable assignment. Review the named target, active scope, eligibility, and availability. This is not a solver feasibility result.",
            pair.person_id, pair.shift_id
        ),
        field_path: Some(field_path.to_owned()),
        resource: Some(ResourceRef::Rule(rule)),
    });
    Ok(())
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
                let source_rows = authored_source_rows(coverage, &minima, budget)?;
                budget.reserve(2, 2, 64)?;
                self.plan.definitions.push(Definition {
                    owner,
                    minima,
                    source_rows,
                    consecutive: None,
                });
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
            let count_field = match coverage {
                Coverage::Exact { .. } => "count",
                Coverage::AtLeast { .. } => "minimum",
            };
            finding(
                &mut self.result.validation,
                "candidate_shortage",
                rule,
                owner,
                shift.id,
                &format!("domain.entities.{}.coverage.{count_field}", owner.id),
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
                &format!("domain.entities.{}.coverage", owner.id),
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
            let source_row = self.plan.definitions[definition].source_rows[minimum];
            if u64::from(key.minimum) > qualified_count {
                finding(
                    &mut self.result.validation,
                    "qualification_shortage",
                    rule,
                    owner,
                    shift.id,
                    &format!(
                        "domain.entities.{}.coverage.qualificationMinimums.{source_row}.minimum",
                        owner.id
                    ),
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
                    &format!(
                        "domain.entities.{}.coverage.qualificationMinimums.{source_row}.minimum",
                        owner.id
                    ),
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
struct SkillMixPlanner<'a> {
    input: &'a AssignmentInput,
    result: &'a mut AssignmentAnalysis,
    plan: &'a mut Plan,
    definitions: &'a mut BTreeMap<Owner, usize>,
    by_shift: &'a BTreeMap<ShiftId, Vec<usize>>,
}

impl SkillMixPlanner<'_> {
    fn add_rule(
        &mut self,
        rule: RuleId,
        scope: &Scope,
        values: &[QualificationMinimum],
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        let owner = Owner {
            kind: "required_skill_mix",
            id: EntityId::from_uuid(rule.as_uuid()),
        };
        let definition = match self.definitions.entry(owner) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let index = self.plan.definitions.len();
                let minima = canonical_minima_values(values, budget)?;
                let source_rows = authored_source_rows_values(values, &minima, budget)?;
                budget.reserve(2, 2, 64)?;
                self.plan.definitions.push(Definition {
                    owner,
                    minima,
                    source_rows,
                    consecutive: None,
                });
                entry.insert(index);
                index
            }
        };
        let input = self.input;
        for shift in &input.shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !shift_scope(scope, shift, &metadata, budget)? {
                continue;
            }
            let mut population = Vec::new();
            if let Some(indices) = self.by_shift.get(&shift.id) {
                for index in indices {
                    budget.step()?;
                    let person = input
                        .person(self.result.candidates[*index].person_id)
                        .ok_or_else(invalid)?;
                    if person_scope(scope, person, budget)? {
                        budget.reserve(0, 1, 8)?;
                        population.push(*index);
                    }
                }
            }
            for minimum in 0..self.plan.definitions[definition].minima.len() {
                budget.step()?;
                let key = &self.plan.definitions[definition].minima[minimum];
                let mut qualified = Vec::new();
                for index in &population {
                    budget.step()?;
                    let person = input
                        .person(self.result.candidates[*index].person_id)
                        .ok_or_else(invalid)?;
                    if qualification_match(person, &key.all, &key.any, interval(shift), budget)? {
                        budget.reserve(0, 1, 8)?;
                        qualified.push(*index);
                    }
                }
                let qualified_count = count(qualified.len())?;
                let source_row = self.plan.definitions[definition].source_rows[minimum];
                if u64::from(key.minimum) > qualified_count {
                    finding(
                        &mut self.result.validation,
                        "qualification_shortage",
                        rule,
                        owner,
                        shift.id,
                        &format!("domain.rules.{rule}.qualificationMinimums.{source_row}.minimum"),
                        budget,
                    )?;
                }
                append(
                    self.plan,
                    PlannedConstraint {
                        rule,
                        predicate: Predicate::SkillMixQualification {
                            definition,
                            minimum,
                            shift: shift.id,
                            upper: qualified_count,
                        },
                        population: qualified,
                        impossible: u64::from(key.minimum) > qualified_count,
                    },
                    budget,
                )?;
            }
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

fn scoped_candidates(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    person: PersonId,
    scope: &Scope,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<usize>, AssignmentRuleError> {
    let mut scoped = Vec::new();
    budget.steps(u64::from(usize::BITS - candidates.len().leading_zeros()))?;
    let start = candidates.partition_point(|pair| pair.person_id < person);
    for (offset, pair) in candidates[start..].iter().enumerate() {
        budget.step()?;
        if pair.person_id != person {
            break;
        }
        let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
        if shift_scope(scope, shift, &input.metadata(shift)?, budget)? {
            budget.reserve(0, 1, 8)?;
            scoped.push(start + offset);
        }
    }
    Ok(scoped)
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
        let mut chronological = scoped_candidates(input, candidates, *person_id, scope, budget)?;
        if compatibility.is_empty() {
            overlap::plan_cliques(
                input,
                candidates,
                &chronological,
                plan,
                rule,
                *person_id,
                budget,
            )?;
            continue;
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

// Retain every contribution's counter update in the same auditable estimate.
#[allow(clippy::too_many_lines)]
fn estimate_contribution(
    result: &mut AssignmentAnalysis,
    plan: &Plan,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let base_variables = result.estimate.variables;
    let mut consecutive_shape = consecutive::Shape::default();
    let mut generic_constraints = 0_u64;
    for constraint in &plan.constraints {
        budget.step()?;
        if consecutive::is(constraint) {
            consecutive_shape =
                consecutive_shape.add(consecutive::shape_constraint(constraint, plan, limits)?)?;
        } else if !compiler_weekend::is(constraint) {
            generic_constraints = add(generic_constraints, 1)?;
        }
    }
    let weekend_shape = compiler_weekend::shape(plan, limits)?;
    let base_constraints = add(
        add(generic_constraints, consecutive_shape.constraints)?,
        weekend_shape.constraints,
    )?;
    let mut hard_lock_parents = 0;
    for constraint in &plan.constraints {
        budget.step()?;
        if matches!(constraint.predicate, Predicate::HardLock { .. }) {
            hard_lock_parents = add(hard_lock_parents, 1)?;
        }
    }
    let base_provenance = add(
        add(base_variables, generic_constraints)?,
        add(count(plan.parents.len())?, hard_lock_parents)?,
    )?;
    let defaults = PlanningIrLimitsV1::DEFAULT;
    // variable -> fact; fact -> person/shift
    let mut base_references = base_variables.checked_mul(3).ok_or_else(invalid)?;
    base_references = add(
        base_references,
        hard_lock_parents.checked_mul(3).ok_or_else(invalid)?,
    )?;
    for constraint in &plan.constraints {
        budget.step()?;
        if consecutive::is(constraint) || compiler_weekend::is(constraint) {
            continue;
        }
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
        base_references = add(
            base_references,
            add(literals, add(2, add(entities, parameters)?)?)?,
        )?;
    }
    base_references = add(
        add(base_references, consecutive_shape.references)?,
        weekend_shape.references,
    )?;
    let hour_shape = hours::checked_shape(plan, limits)?;
    let variables = add(
        add(
            add(
                add(base_variables, consecutive_shape.variables)?,
                weekend_shape.variables,
            )?,
            hour_shape.pair_indices,
        )?,
        hour_shape.helper_variables,
    )?;
    let constraints = add(base_constraints, hour_shape.constraints)?;
    let provenance = add(
        add(
            add(base_provenance, consecutive_shape.provenance)?,
            weekend_shape.provenance,
        )?,
        hour_shape.provenance,
    )?;
    let references = add(base_references, hour_shape.references)?;
    within(
        variables,
        limits.max_variables.min(defaults.max_variables),
        AssignmentRuleLimit::Variables,
    )?;
    within(
        constraints,
        limits.max_constraints.min(defaults.max_constraints),
        AssignmentRuleLimit::Constraints,
    )?;
    within(
        provenance,
        limits
            .max_provenance_records
            .min(defaults.max_provenance_records),
        AssignmentRuleLimit::ProvenanceRecords,
    )?;
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
    if hard_lock_parents > 0 {
        within(
            3,
            limits
                .max_entity_refs_per_record
                .min(defaults.max_entity_refs_per_record),
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
    result.estimate.variables = variables;
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
        Predicate::SkillMixQualification {
            definition,
            minimum,
            ..
        } => {
            let key = &plan.definitions[*definition].minima[*minimum];
            (2, add(2, count(key.all.len() + key.any.len())?)?)
        }
        Predicate::MaximumAssignmentCount { .. } => (2, 3),
        Predicate::MaximumWorkedWeekends { .. } => (1, 2),
        Predicate::WorkloadQuota {
            maximum,
            measure: WorkloadQuotaMeasure::BucketMinutes { .. },
            ..
        } => (3, 4 + u64::from(maximum.is_some())),
        Predicate::WorkloadQuota { maximum, .. } => (2, 4 + u64::from(maximum.is_some())),
        Predicate::MaximumConsecutive { .. } => (1, 3),
        Predicate::Overlap { .. } => (3, 0),
        Predicate::OverlapClique { .. } | Predicate::MutualSameShift { .. } => (1, 0),
        Predicate::MutualOverlap {
            first_shift,
            second_shift,
            ..
        } => (3 + u64::from(first_shift != second_shift), 0),
        Predicate::MinimumRest { .. } => (3, 3),
        Predicate::TransitionTime { .. } => (5, 5),
        Predicate::FixedAssignment { .. } | Predicate::HardLock { .. } => (2, 0),
    })
}

#[derive(Clone, Copy)]
pub(super) struct AppendReservation {
    add_parent: bool,
}

pub(super) fn reserve_append(
    plan: &Plan,
    rule: RuleId,
    predicate: Predicate,
    budget: &mut OperationBudget<'_>,
) -> Result<AppendReservation, AssignmentRuleError> {
    budget.step()?;
    within(
        add(count(plan.constraints.len())?, 1)?,
        budget.constraint_limit(),
        AssignmentRuleLimit::Constraints,
    )?;
    let add_parent =
        !matches!(predicate, Predicate::HardLock { .. }) && !plan.parents.contains_key(&rule);
    if add_parent {
        budget.reserve(1, 1, 64)?;
    }
    budget.reserve(1, 4, 128)?;
    Ok(AppendReservation { add_parent })
}

pub(super) fn commit_append(
    plan: &mut Plan,
    constraint: PlannedConstraint,
    reservation: AppendReservation,
) {
    if reservation.add_parent {
        plan.parents.insert(
            constraint.rule,
            match constraint.predicate {
                Predicate::Overlap { .. } | Predicate::OverlapClique { .. } => "no_overlap",
                Predicate::MutualSameShift { .. } | Predicate::MutualOverlap { .. } => {
                    "mutual_assignment_restriction"
                }
                Predicate::MinimumRest { .. } => "minimum_rest",
                Predicate::TransitionTime { .. } => "transition_time",
                Predicate::MaximumAssignmentCount { .. } => "maximum_assignment_count",
                Predicate::WorkloadQuota { .. } => "workload_quota",
                Predicate::MaximumConsecutive { .. } => "maximum_consecutive",
                Predicate::MaximumWorkedWeekends { .. } => "maximum_worked_weekends",
                Predicate::Headcount { .. } | Predicate::Qualification { .. } => "coverage",
                Predicate::SkillMixQualification { .. } => "required_skill_mix",
                Predicate::FixedAssignment { .. } => "fixed_assignment",
                Predicate::HardLock { .. } => unreachable!(),
            },
        );
    }
    plan.constraints.push(constraint);
}

pub(super) fn append(
    plan: &mut Plan,
    constraint: PlannedConstraint,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let reservation = reserve_append(plan, constraint.rule, constraint.predicate, budget)?;
    commit_append(plan, constraint, reservation);
    Ok(())
}

// Keep source-bound pairing, subset proof, and the owning finding in one auditable path.
#[allow(clippy::too_many_lines)]
fn coverage_bound_findings(
    plan: &Plan,
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut by_shift = BTreeMap::<ShiftId, Vec<&PlannedConstraint>>::new();
    for constraint in &plan.constraints {
        budget.step()?;
        let shift = match constraint.predicate {
            Predicate::Headcount { shift, .. }
            | Predicate::Qualification { shift, .. }
            | Predicate::SkillMixQualification { shift, .. } => shift,
            Predicate::MaximumAssignmentCount { .. }
            | Predicate::MaximumWorkedWeekends { .. }
            | Predicate::WorkloadQuota { .. }
            | Predicate::MaximumConsecutive { .. }
            | Predicate::Overlap { .. }
            | Predicate::OverlapClique { .. }
            | Predicate::MutualSameShift { .. }
            | Predicate::MutualOverlap { .. }
            | Predicate::MinimumRest { .. }
            | Predicate::TransitionTime { .. }
            | Predicate::FixedAssignment { .. }
            | Predicate::HardLock { .. } => continue,
        };
        budget.reserve(1, 2, 24)?;
        by_shift.entry(shift).or_default().push(constraint);
    }
    for (shift, predicates) in by_shift {
        for lower in &predicates {
            let (definition, minimum, code, source_row) = match lower.predicate {
                Predicate::Headcount {
                    definition, lower, ..
                } => (definition, lower, "contradictory_coverage_bounds", None),
                Predicate::Qualification {
                    definition,
                    minimum,
                    ..
                }
                | Predicate::SkillMixQualification {
                    definition,
                    minimum,
                    ..
                } => {
                    let sr = plan.definitions[definition].source_rows[minimum];
                    (
                        definition,
                        u64::from(plan.definitions[definition].minima[minimum].minimum),
                        "qualification_above_headcount",
                        Some(sr),
                    )
                }
                Predicate::MaximumAssignmentCount { .. }
                | Predicate::WorkloadQuota { .. }
                | Predicate::MaximumWorkedWeekends { .. }
                | Predicate::MaximumConsecutive { .. }
                | Predicate::Overlap { .. }
                | Predicate::OverlapClique { .. }
                | Predicate::MutualSameShift { .. }
                | Predicate::MutualOverlap { .. }
                | Predicate::MinimumRest { .. }
                | Predicate::TransitionTime { .. }
                | Predicate::FixedAssignment { .. }
                | Predicate::HardLock { .. } => continue,
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
                    let field_path = match (owner.kind, source_row) {
                        ("required_skill_mix", Some(row)) => format!(
                            "domain.rules.{}.qualificationMinimums.{row}.minimum",
                            owner.id
                        ),
                        (_, Some(row)) => format!(
                            "domain.entities.{}.coverage.qualificationMinimums.{row}.minimum",
                            owner.id
                        ),
                        (_, None) => format!("domain.entities.{}.coverage", owner.id),
                    };
                    budget.reserve(1, 7, 1024)?;
                    report.issues.push(ValidationIssue {
                        code: format!("official.workforce.{code}"), severity: ValidationSeverity::Error,
                        message: format!("Rule {} owner {} {} requires at least {minimum}, exceeding {maximum} allowed by rule {} owner {} {} for shift {shift}. This is not a solver feasibility result.", lower.rule, owner.kind, owner.id, upper.rule, other.kind, other.id),
                        field_path: Some(field_path), resource: Some(ResourceRef::Rule(lower.rule)),
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
    field_path: &str,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    budget.reserve(1, 5, 768)?;
    report.issues.push(ValidationIssue {
        code: format!("official.workforce.{code}"),
        severity: ValidationSeverity::Error,
        message: finding_message(owner, shift),
        field_path: Some(field_path.to_owned()),
        resource: Some(ResourceRef::Rule(rule)),
    });
    Ok(())
}

fn finding_message(owner: Owner, shift: ShiftId) -> String {
    format!(
        "{} has an obvious contradiction (owner {} {}, shift {shift}). This is not a solver feasibility result.",
        if owner.kind == "required_skill_mix" {
            "Required skill mix"
        } else {
            "Required coverage"
        },
        owner.kind,
        owner.id
    )
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
    document: &ScenarioDocument,
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
    mutual::locked_findings(input, &locks, &mut result.validation, budget)?;
    rest::locked_rest_findings(input, &locks, &mut result.validation, budget)?;
    transition::locked_findings(input, &locks, &mut result.validation, budget)?;
    count::locked_findings(document, input, &locks, &mut result.validation, budget)?;
    hours::locked_findings(document, input, &locks, &mut result.validation, budget)?;
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
            let owner = plan.definitions[definition].owner;
            finding(
                report,
                "hard_locked_coverage_excess",
                constraint.rule,
                owner,
                shift,
                &format!("domain.entities.{}.coverage", owner.id),
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
        AssignmentRuleError, AssignmentRuleLimit, DomainValidationReport, OperationBudget,
        PairContext, analyze_assignments, analyze_source_with_budget, analyze_with_budget,
        obligations,
    };
    use crate::{
        assignment_rules::{analysis::prepare, budget::MAX_WORK_STEPS},
        test_support::{fixture, id},
    };
    use eutheto_planning_ir::PlanningIrLimitsV1;
    use eutheto_types::{CancellationToken, ScenarioDocument};
    use serde_json::json;

    #[test]
    fn source_analysis_matches_full_analysis_and_retains_preparation_charges()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = fixture()?;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        let mut rejected_person = document
            .domain
            .entities
            .get(&id(1).parse()?)
            .ok_or("person fixture")?
            .clone();
        rejected_person["id"] = json!(id(12));
        rejected_person["externalId"] = json!("staff-02");
        rejected_person["eligibleAssignmentTypeIds"] = json!([]);
        document
            .domain
            .entities
            .insert(id(12).parse()?, rejected_person);
        document
            .domain
            .entities
            .get_mut(&id(8).parse()?)
            .ok_or("shift fixture")?["coverage"] =
            json!({"kind":"exact","count":2,"qualificationMinimums":[]});
        for (index, kind) in [(20, "eligibility"), (22, "coverage")] {
            document.domain.rules.insert(
                id(index).parse()?,
                json!({"id":id(index),"kind":kind,"active":true,"strength":"required",
                       "scope":{"people":{"kind":"all"}}}),
            );
        }
        for limits in [
            PlanningIrLimitsV1::DEFAULT,
            PlanningIrLimitsV1 {
                max_variables: 100,
                max_constraints: 100,
                max_provenance_records: 1_000,
                max_total_refs: 10_000,
                max_ir_bytes: 1024 * 1024,
                ..PlanningIrLimitsV1::DEFAULT
            },
        ] {
            for mut source_budget in [
                OperationBudget::analysis(None, limits),
                OperationBudget::evaluation(None),
            ] {
                source_budget.reserve(1, 1, 128)?;
                source_budget.steps(7)?;
                let mut full_budget = source_budget.clone();
                let mut preparation_budget = source_budget.clone();
                let input = AssignmentInput::new(&document, &mut preparation_budget)?;
                prepare(&document, &input, &mut preparation_budget, limits)?;
                let source = analyze_source_with_budget(&document, &mut source_budget, limits)?;
                let full = analyze_with_budget(&document, &mut full_budget, limits)?;
                // AssignmentAnalysis equality covers the hash, complete pair partition,
                // validation, obligations and every estimate field, including pruning stages.
                assert_eq!(source, full);
                assert!(source.obligations.handled.contains(&id(20).parse()?));
                assert_eq!(
                    source_budget.remaining_output(),
                    preparation_budget.remaining_output()
                );
                assert_eq!(source_budget.reserved_ir_bytes(), 0);
                assert!(full_budget.reserved_ir_bytes() > 0);
            }
        }
        Ok(())
    }

    #[test]
    fn source_analysis_does_not_measure_unused_derived_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        // No hours rule: its source checked_shape may legitimately enforce derived-ID limits.
        let mut document = fixture()?;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        let limits = PlanningIrLimitsV1 {
            max_id_bytes: 1,
            ..PlanningIrLimitsV1::DEFAULT
        };
        let expected = analyze_assignments(&document, None, PlanningIrLimitsV1::DEFAULT)?;
        let mut budget = OperationBudget::analysis(None, limits);
        assert_eq!(
            analyze_source_with_budget(&document, &mut budget, limits)?,
            expected
        );
        assert_eq!(
            analyze_assignments(&document, None, limits),
            Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord
            ))
        );
        Ok(())
    }

    #[test]
    fn source_analysis_preserves_inherited_cancellation() -> Result<(), Box<dyn std::error::Error>>
    {
        let document = fixture()?;
        let token = CancellationToken::new();
        let control = eutheto_types::OperationControl::Cancellation(token.clone());
        let mut budget = OperationBudget::evaluation(Some(&control));
        budget.cancel_after_steps(1)?;
        assert_eq!(
            analyze_source_with_budget(&document, &mut budget, PlanningIrLimitsV1::DEFAULT),
            Err(AssignmentRuleError::Cancelled)
        );
        assert!(token.is_cancelled());
        Ok(())
    }

    #[test]
    fn source_analysis_preserves_inherited_source_budget_exhaustion()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = fixture()?;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        let limits = PlanningIrLimitsV1::DEFAULT;
        let mut budget = OperationBudget::evaluation(None);
        assert!(analyze_source_with_budget(&document, &mut budget, limits).is_ok());

        let mut bytes_budget = OperationBudget::evaluation(None);
        let (_, _, remaining_bytes) = bytes_budget.remaining_output();
        bytes_budget.reserve(0, 0, remaining_bytes - 1)?;
        assert_eq!(
            analyze_source_with_budget(&document, &mut bytes_budget, limits),
            Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::Bytes
            ))
        );
        assert_eq!(bytes_budget.reserved_ir_bytes(), 0);

        let mut work_budget = OperationBudget::evaluation(None);
        work_budget.steps(MAX_WORK_STEPS)?;
        assert_eq!(
            analyze_source_with_budget(&document, &mut work_budget, limits),
            Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::WorkSteps
            ))
        );
        assert_eq!(work_budget.reserved_ir_bytes(), 0);
        Ok(())
    }

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
            let control = eutheto_types::OperationControl::Cancellation(token.clone());
            let mut budget = OperationBudget::analysis(Some(&control), PlanningIrLimitsV1::DEFAULT);
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

    // ── Coverage source-row mapping regression ──────────────────────────────

    use super::support::{authored_source_rows, canonical_minima};
    use crate::model::{Coverage, QualificationMatch, QualificationMinimum};
    use std::str::FromStr;

    fn qid(index: u32) -> Result<crate::ids::QualificationId, Box<dyn std::error::Error>> {
        Ok(crate::ids::QualificationId::from_str(&id(index))?)
    }

    fn qual_min(
        all: Vec<u32>,
        any: Vec<u32>,
        minimum: u16,
    ) -> Result<QualificationMinimum, Box<dyn std::error::Error>> {
        Ok(QualificationMinimum {
            qualifications: QualificationMatch {
                all_qualification_ids: all.into_iter().map(qid).collect::<Result<_, _>>()?,
                any_qualification_ids: any.into_iter().map(qid).collect::<Result<_, _>>()?,
            },
            minimum,
        })
    }

    #[test]
    fn authored_source_rows_reversed_order() -> Result<(), Box<dyn std::error::Error>> {
        // Authored order: [B, A]. Canonical sort reverses to [A, B].
        // Source mapping must resolve each canonical key to its first authored row.
        let coverage = Coverage::Exact {
            count: 1,
            qualification_minimums: vec![
                qual_min(vec![20, 10], vec![], 1)?, // authored row 0: {10,20}
                qual_min(vec![10], vec![], 1)?,     // authored row 1: {10}
            ],
        };
        let mut budget = OperationBudget::analysis(None, PlanningIrLimitsV1::DEFAULT);
        let minima = canonical_minima(&coverage, &mut budget)?;
        // Canonical: [{10}, {10,20}] after sort+dedup (two distinct keys)
        assert_eq!(minima.len(), 2);
        assert_eq!(minima[0].all, vec![qid(10)?]);
        assert_eq!(minima[1].all, vec![qid(10)?, qid(20)?]);

        let source_rows = authored_source_rows(&coverage, &minima, &mut budget)?;
        assert_eq!(source_rows.len(), 2);
        // Canonical [{10}] matches authored row 1 (first match)
        assert_eq!(source_rows[0], 1);
        // Canonical [{10,20}] matches authored row 0 (first match)
        assert_eq!(source_rows[1], 0);
        Ok(())
    }

    #[test]
    fn authored_source_rows_duplicate_equivalent() -> Result<(), Box<dyn std::error::Error>> {
        // Two identical authored rows. Canonical dedup merges them.
        // Both canonical keys map to the first authored row (index 0).
        let coverage = Coverage::Exact {
            count: 2,
            qualification_minimums: vec![
                qual_min(vec![10], vec![], 1)?, // authored row 0
                qual_min(vec![10], vec![], 1)?, // authored row 1 (duplicate)
            ],
        };
        let mut budget = OperationBudget::analysis(None, PlanningIrLimitsV1::DEFAULT);
        let minima = canonical_minima(&coverage, &mut budget)?;
        assert_eq!(minima.len(), 1); // deduped

        let source_rows = authored_source_rows(&coverage, &minima, &mut budget)?;
        assert_eq!(source_rows.len(), 1);
        assert_eq!(source_rows[0], 0); // first authored row
        Ok(())
    }

    #[test]
    fn candidate_shortage_exact_targets_count_field() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = fixture()?;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        // Exact coverage requiring 2 but only 1 person available
        document
            .domain
            .entities
            .get_mut(&id(8).parse()?)
            .ok_or("shift fixture")?["coverage"] =
            json!({"kind":"exact","count":2,"qualificationMinimums":[]});
        // Add a coverage rule
        document.domain.rules.insert(
            id(22).parse()?,
            json!({"id":id(22),"kind":"coverage","active":true,"strength":"required",
                   "scope":{"people":{"kind":"all"}}}),
        );
        let mut budget = OperationBudget::analysis(None, PlanningIrLimitsV1::DEFAULT);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let (analysis, _) = prepare(&document, &input, &mut budget, PlanningIrLimitsV1::DEFAULT)?;
        let shortage = analysis
            .validation
            .issues
            .iter()
            .find(|i| i.code == "official.workforce.candidate_shortage")
            .ok_or("expected candidate_shortage")?;
        assert_eq!(
            shortage.field_path,
            Some(format!("domain.entities.{}.coverage.count", id(8)))
        );
        Ok(())
    }

    #[test]
    fn candidate_shortage_at_least_targets_minimum_field() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut document = fixture()?;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        // AtLeast coverage requiring minimum 2 but only 1 person available
        document
            .domain
            .entities
            .get_mut(&id(8).parse()?)
            .ok_or("shift fixture")?["coverage"] =
            json!({"kind":"atLeast","minimum":2,"qualificationMinimums":[]});
        document.domain.rules.insert(
            id(22).parse()?,
            json!({"id":id(22),"kind":"coverage","active":true,"strength":"required",
                   "scope":{"people":{"kind":"all"}}}),
        );
        let mut budget = OperationBudget::analysis(None, PlanningIrLimitsV1::DEFAULT);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let (analysis, _) = prepare(&document, &input, &mut budget, PlanningIrLimitsV1::DEFAULT)?;
        let shortage = analysis
            .validation
            .issues
            .iter()
            .find(|i| i.code == "official.workforce.candidate_shortage")
            .ok_or("expected candidate_shortage")?;
        assert_eq!(
            shortage.field_path,
            Some(format!("domain.entities.{}.coverage.minimum", id(8)))
        );
        Ok(())
    }

    #[test]
    fn qualification_shortage_targets_correct_authored_row()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = fixture()?;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        // Add a second qualification entity
        document.domain.entities.insert(
            id(12).parse()?,
            json!({"kind":"qualification","id":id(12),"name":"Second","description":""}),
        );
        // Person has qualification 11 but not 12
        // Two qualification minimums in authored order: [row0={id(11)}, row1={id(12)}]
        // Person lacks id(12), so row1 should be short.
        document
            .domain
            .entities
            .get_mut(&id(8).parse()?)
            .ok_or("shift fixture")?["coverage"] = json!({
            "kind":"exact","count":1,
            "qualificationMinimums":[
                {"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":1},
                {"qualifications":{"allQualificationIds":[id(12)],"anyQualificationIds":[]},"minimum":1}
            ]
        });
        document.domain.rules.insert(
            id(22).parse()?,
            json!({"id":id(22),"kind":"coverage","active":true,"strength":"required",
                   "scope":{"people":{"kind":"all"}}}),
        );
        let mut budget = OperationBudget::analysis(None, PlanningIrLimitsV1::DEFAULT);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let (analysis, _) = prepare(&document, &input, &mut budget, PlanningIrLimitsV1::DEFAULT)?;
        let shortage = analysis
            .validation
            .issues
            .iter()
            .find(|i| i.code == "official.workforce.qualification_shortage")
            .ok_or("expected qualification_shortage")?;
        // The field path should target the specific authored row that's short.
        // Authored row 0 has allQualificationIds=[id(11)] which person has.
        // Authored row 1 has allQualificationIds=[id(12)] which person lacks.
        assert_eq!(
            shortage.field_path,
            Some(format!(
                "domain.entities.{}.coverage.qualificationMinimums.1.minimum",
                id(8)
            ))
        );
        Ok(())
    }

    #[test]
    fn standalone_coverage_owner_field_path() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = fixture()?;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        // Standalone CoverageRequirement requiring 2 but only 1 person
        document.domain.entities.insert(
            id(30).parse()?,
            json!({"kind":"coverageRequirement","id":id(30),"active":true,
                   "scope":{"kind":"all"},
                   "coverage":{"kind":"exact","count":2,"qualificationMinimums":[]}}),
        );
        // Need a coverage rule for the shift template too
        document.domain.rules.insert(
            id(22).parse()?,
            json!({"id":id(22),"kind":"coverage","active":true,"strength":"required",
                   "scope":{"people":{"kind":"all"}}}),
        );
        let mut budget = OperationBudget::analysis(None, PlanningIrLimitsV1::DEFAULT);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let (analysis, _) = prepare(&document, &input, &mut budget, PlanningIrLimitsV1::DEFAULT)?;
        let shortage = analysis
            .validation
            .issues
            .iter()
            .find(|i| i.code == "official.workforce.candidate_shortage")
            .ok_or("expected candidate_shortage from coverage requirement")?;
        // Must target the standalone coverage requirement entity, not the shift template
        assert_eq!(
            shortage.field_path,
            Some(format!("domain.entities.{}.coverage.count", id(30)))
        );
        Ok(())
    }
}
