//! Privacy-filtered Workforce facts. Sources are descriptive inputs, never acceptance tokens.
//! Core owns host/run binding, authenticated manual journals and native original promotion.

use super::{
    FairnessPeriodKey, FairnessStep, ShiftMetadata, build_fairness_actual_index,
    fairness_peer_matches, fairness_periods, fairness_shift_matches,
};
use crate::assignment_rules::{
    baseline::{baseline_sources, selected_baseline_pairs},
    budget::OperationBudget,
    input::AssignmentInput,
    operation_error, recipient_source,
};
use crate::ids::ShiftId;
use crate::model::{
    AssignmentPair, OverlappingContribution, WindowMembership, WorkforceEntity,
    WorkforcePreference, WorkloadMeasurement,
};
use crate::temporal::ResolvedInterval;
use eutheto_domain_api::DomainPackError;
use eutheto_domain_ir::{AcceptedResult, RunBaselineSelection, RunObjectiveContext};
use eutheto_types::{EntityId, OperationControl, PersonId, Revision, ScenarioDocument};
use eutheto_types::{
    MAX_RECIPIENT_REPORT_BYTES, MAX_RECIPIENT_REPORT_ROWS, RecipientActualGroupV1,
    RecipientActualPeriodV1, RecipientActualV1, RecipientAssignmentV1, RecipientChangeBasisV1,
    RecipientChangeRowV1, RecipientChangesV1, RecipientEndpointV1, RecipientEntityV1,
    RecipientFractionV1, RecipientHistoricalEndpointV1, RecipientHistoricalFactsV1,
    RecipientIntervalV1, RecipientLabelV1, RecipientMembershipV1, RecipientOriginalAdmissionV1,
    RecipientOriginalV1, RecipientOverlapV1, RecipientPairChangeV1, RecipientPersonV1,
    RecipientReportOptionsV1, RecipientShiftV1, RecipientStaffingV1, RecipientWorkloadMeasureV1,
    WorkforceRecipientV1,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

/// Exact accepted A source, supplied only after Core checks the complete native original tuple.
/// This public value does not assert custody. Its pairs and document are independently checked here.
#[derive(Clone, Copy)]
pub struct RecipientAcceptedSource<'a> {
    pub document: &'a ScenarioDocument,
    pub accepted: &'a AcceptedResult,
    pub objective_context: Option<&'a RunObjectiveContext>,
}

/// Exact pre-edit B source. Core must authenticate the journal transition before supplying it.
#[derive(Clone, Copy)]
pub struct RecipientManualSource<'a> {
    pub document: &'a ScenarioDocument,
    pub revision: Revision,
}

type Selected<'a> = BTreeMap<AssignmentPair, &'a str>;

// Source assessment keeps its existing limits. All optional projection contexts share these
// separate, cumulative work/row/union counters and the same inherited operation control.
struct ReportBudget<'a> {
    scratch: OperationBudget<'a>,
    work: usize,
    rows: usize,
    unions: usize,
    strings: usize,
}

impl<'a> ReportBudget<'a> {
    fn new(control: &'a OperationControl) -> Self {
        Self {
            scratch: OperationBudget::evaluation(Some(control)),
            work: 0,
            rows: 0,
            unions: 0,
            strings: 0,
        }
    }

    fn work(&mut self, amount: usize) -> Result<(), DomainPackError> {
        self.scratch
            .check()
            .map_err(|error| operation_error(&error))?;
        self.work = self
            .work
            .checked_add(amount)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if self.work > 10_000_000 {
            return Err(DomainPackError::ResourceLimitExceeded);
        }
        self.scratch
            .steps(amount as u64)
            .map_err(|error| operation_error(&error))
    }

    fn retain(&mut self, amount: usize, bytes: usize) -> Result<(), DomainPackError> {
        self.work(amount)?;
        let bytes = amount
            .checked_mul(bytes)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        self.scratch
            .reserve(0, amount as u64, bytes as u64)
            .map_err(|error| operation_error(&error))
    }

    fn rows<T>(&mut self, amount: usize) -> Result<(), DomainPackError> {
        self.rows = self
            .rows
            .checked_add(amount)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if self.rows > MAX_RECIPIENT_REPORT_ROWS {
            return Err(DomainPackError::ResourceLimitExceeded);
        }
        // Vec growth and inline owned capacities are admitted conservatively before retention.
        self.retain(amount, size_of::<T>().saturating_mul(2))
    }

    fn text(&mut self, text: &str) -> Result<String, DomainPackError> {
        self.work(text.len())?;
        let encoded = text.bytes().try_fold(2usize, |sum, byte| {
            sum.checked_add(match byte {
                b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 8 | 12 => 2,
                0..=31 => 6,
                _ => 1,
            })
            .ok_or(DomainPackError::ResourceLimitExceeded)
        })?;
        self.strings = self
            .strings
            .checked_add(encoded)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if self.strings > MAX_RECIPIENT_REPORT_BYTES {
            return Err(DomainPackError::ResourceLimitExceeded);
        }
        self.retain(1, text.len())?;
        Ok(text.to_owned())
    }

    fn label(
        &mut self,
        kind: &str,
        identity: impl std::fmt::Display,
        authored: &str,
        include: bool,
    ) -> Result<RecipientLabelV1, DomainPackError> {
        if include {
            return self.text(authored).map(RecipientLabelV1::Authored);
        }
        // Full native identity makes aliases stable across privacy/reference-closure changes.
        let length = kind.len() + 39;
        self.strings = self
            .strings
            .checked_add(length)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if self.strings > MAX_RECIPIENT_REPORT_BYTES {
            return Err(DomainPackError::ResourceLimitExceeded);
        }
        self.retain(1, length)?;
        Ok(RecipientLabelV1::Generated(format!("{kind} {identity}")))
    }

    fn fairness(&mut self, step: FairnessStep, amount: usize) -> Result<(), DomainPackError> {
        match step {
            FairnessStep::Inspect => self.work(amount),
            FairnessStep::Sort => {
                let levels = usize::BITS - amount.leading_zeros();
                self.work(
                    amount
                        .checked_mul(levels as usize)
                        .ok_or(DomainPackError::ResourceLimitExceeded)?,
                )
            }
            FairnessStep::RetainWindow => self.retain(amount, 512),
            FairnessStep::RetainIndex => self.retain(amount, 256),
            FairnessStep::UnionInterval => {
                self.unions = self
                    .unions
                    .checked_add(amount)
                    .ok_or(DomainPackError::ResourceLimitExceeded)?;
                if self.unions > 1_000_000 {
                    return Err(DomainPackError::ResourceLimitExceeded);
                }
                self.retain(amount, 256)
            }
        }
    }

    fn source(
        &mut self,
        document: &ScenarioDocument,
        accepted: Option<&AcceptedResult>,
    ) -> Result<(), DomainPackError> {
        for count in [
            document.domain.entities.len(),
            document.domain.rules.len(),
            document.domain.preferences.len(),
            document.domain.locked_assignments.len(),
            accepted.map_or(0, |value| value.solution.assignments.len()),
        ] {
            self.work(count)?;
        }
        Ok(())
    }
}

fn missing() -> DomainPackError {
    super::invalid(
        "/recipientReport/source",
        "recipient historical source or pair census does not match",
    )
}

fn interval(value: ResolvedInterval) -> Result<RecipientIntervalV1, DomainPackError> {
    Ok(RecipientIntervalV1 {
        starts_at: RecipientEndpointV1::from_resolved(value.starts_at).map_err(|_| missing())?,
        ends_at: RecipientEndpointV1::from_resolved(value.ends_at).map_err(|_| missing())?,
    })
}

fn insert<T: Ord + Copy>(
    set: &mut BTreeSet<T>,
    value: T,
    budget: &mut ReportBudget<'_>,
) -> Result<(), DomainPackError> {
    budget.work(1)?;
    if !set.contains(&value) {
        budget.retain(1, 128)?;
        set.insert(value);
    }
    Ok(())
}

/// Projects a complete selected schedule and the opted-in actual-only censuses.
/// A is always returned as `NotEstablished`; only Core may promote original admission after
/// checking authenticated run bindings. Contradictory supplied history refuses the entire report.
///
/// # Errors
/// Refuses fresh-verification/provenance/source mismatches, aggregate limits and interruption.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn build_workforce_recipient_report(
    document: &ScenarioDocument,
    accepted: &AcceptedResult,
    objective_context: Option<&RunObjectiveContext>,
    original: Option<RecipientAcceptedSource<'_>>,
    manual: Option<RecipientManualSource<'_>>,
    options: RecipientReportOptionsV1,
    control: &OperationControl,
) -> Result<WorkforceRecipientV1, DomainPackError> {
    let mut source_budget = OperationBudget::evaluation(Some(control));
    let mut budget = ReportBudget::new(control);
    budget.source(document, Some(accepted))?;
    let (input, selected) =
        recipient_source(document, accepted, objective_context, &mut source_budget)?;
    let mut people = BTreeSet::new();
    let mut shifts = BTreeSet::new();
    let mut content = WorkforceRecipientV1 {
        people: Vec::new(),
        assignment_types: Vec::new(),
        locations: Vec::new(),
        shifts: Vec::new(),
        assignments: Vec::new(),
        staffing: options.include_staffing.then(Vec::new),
        changes: options.include_changes.then(Vec::new),
        fairness_actuals: options.include_fairness_actuals.then(Vec::new),
    };
    for (pair, id) in &selected {
        budget.rows::<RecipientAssignmentV1>(1)?;
        insert(&mut people, pair.person_id, &mut budget)?;
        insert(&mut shifts, pair.shift_id, &mut budget)?;
        content.assignments.push(RecipientAssignmentV1 {
            assignment_id: budget.text(id)?,
            person_id: pair.person_id,
            shift_id: pair.shift_id.as_entity_id(),
        });
    }
    if let Some(staffing) = &mut content.staffing {
        let mut counts = BTreeMap::<ShiftId, u32>::new();
        for pair in selected.keys() {
            budget.retain(1, 128)?;
            let count = counts.entry(pair.shift_id).or_default();
            *count = count
                .checked_add(1)
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
        }
        for shift in &input.shifts {
            budget.rows::<RecipientStaffingV1>(1)?;
            insert(&mut shifts, shift.id, &mut budget)?;
            staffing.push(RecipientStaffingV1 {
                shift_id: shift.id.as_entity_id(),
                assigned_count: counts.get(&shift.id).copied().unwrap_or(0),
            });
        }
    }
    // Supplied historical sources are checked even when changes are not disclosed.
    let changes = changes(
        document,
        objective_context,
        &input,
        &selected,
        original,
        manual,
        options,
        &mut source_budget,
        &mut budget,
    )?;
    if options.include_changes {
        content.changes = Some(changes);
    }
    if let Some(groups) = &mut content.fairness_actuals {
        actuals(
            document,
            &input,
            &selected,
            groups,
            &mut people,
            &mut shifts,
            &mut budget,
        )?;
    }
    let mut types = BTreeSet::new();
    let mut locations = BTreeSet::new();
    for id in shifts {
        budget.rows::<RecipientShiftV1>(1)?;
        let shift = input.shift(id).ok_or_else(missing)?;
        let metadata = input
            .metadata(shift)
            .map_err(|error| operation_error(&error))?;
        insert(&mut types, metadata.assignment_type.id, &mut budget)?;
        if let Some(location) = metadata.location_id {
            insert(&mut locations, location, &mut budget)?;
        }
        budget.retain(1, 10)?;
        content.shifts.push(RecipientShiftV1 {
            shift_id: id.as_entity_id(),
            assignment_type_id: metadata.assignment_type.id.as_entity_id(),
            location_id: metadata
                .location_id
                .map(crate::ids::LocationId::as_entity_id),
            interval: interval(shift.interval)?,
            reporting_date: shift.reporting_date.to_string(),
        });
    }
    // Every presentation view consumes this native chronology, including exact fractional instants.
    budget.fairness(FairnessStep::Sort, content.shifts.len())?;
    content
        .shifts
        .sort_unstable_by_key(|shift| (shift.interval.starts_at.resolved.instant, shift.shift_id));
    for id in people {
        budget.rows::<RecipientPersonV1>(1)?;
        let person = input.person(id).ok_or_else(missing)?;
        content.people.push(RecipientPersonV1 {
            person_id: id,
            label: budget.label(
                "Participant",
                id,
                &person.name,
                options.include_person_names,
            )?,
        });
    }
    for id in types {
        budget.rows::<RecipientEntityV1>(1)?;
        let entity = input
            .assignment_type(id)
            .map_err(|error| operation_error(&error))?;
        content.assignment_types.push(RecipientEntityV1 {
            entity_id: id.as_entity_id(),
            label: budget.label("Work", id, &entity.name, options.include_authored_labels)?,
        });
    }
    for id in locations {
        budget.rows::<RecipientEntityV1>(1)?;
        let Some(WorkforceEntity::Location(entity)) = input.domain.entities.get(&id.as_entity_id())
        else {
            return Err(missing());
        };
        content.locations.push(RecipientEntityV1 {
            entity_id: id.as_entity_id(),
            label: budget.label(
                "Location",
                id,
                &entity.name,
                options.include_authored_labels,
            )?,
        });
    }
    // No JSON tree or duplicate canonical byte buffer: stream into a bounded counting writer.
    let mut writer = CanonicalCount {
        budget: &mut budget,
        bytes: 0,
        failure: None,
    };
    if serde_json::to_writer(&mut writer, &content).is_err() {
        return Err(writer.failure.unwrap_or_else(missing));
    }
    control.check().map_err(DomainPackError::from)?;
    Ok(content)
}

struct CanonicalCount<'a, 'control> {
    budget: &'a mut ReportBudget<'control>,
    bytes: usize,
    failure: Option<DomainPackError>,
}
impl Write for CanonicalCount<'_, '_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let check = self.budget.work(bytes.len()).and_then(|()| {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            if self.bytes > MAX_RECIPIENT_REPORT_BYTES {
                return Err(DomainPackError::ResourceLimitExceeded);
            }
            Ok(())
        });
        if let Err(error) = check {
            self.failure = Some(error);
            return Err(io::Error::other("recipient report refused"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn actuals(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    selected: &Selected<'_>,
    groups: &mut Vec<RecipientActualGroupV1>,
    people: &mut BTreeSet<PersonId>,
    shifts: &mut BTreeSet<ShiftId>,
    budget: &mut ReportBudget<'_>,
) -> Result<(), DomainPackError> {
    let policy = input
        .domain
        .entities
        .values()
        .find_map(|entity| match entity {
            WorkforceEntity::ScorePolicy(policy) => Some(policy),
            _ => None,
        });
    for preference in input.domain.preferences.values() {
        budget.work(1)?;
        let WorkforcePreference::WorkloadBalance {
            active: true,
            scope,
            workload_policy_id,
            ..
        } = preference
        else {
            continue;
        };
        let plan = policy
            .and_then(|policy| policy.workload_policies.get(workload_policy_id))
            .ok_or_else(missing)?;
        let Some(WorkforceEntity::WorkloadBucket(bucket)) =
            input.domain.entities.get(&plan.bucket_id.as_entity_id())
        else {
            return Err(missing());
        };
        budget.rows::<RecipientActualGroupV1>(1)?;
        let mut peers = Vec::new();
        for id in &input.people {
            budget.work(1)?;
            let person = input.person(*id).ok_or_else(missing)?;
            if fairness_peer_matches(
                &plan.peer_group.people,
                plan.peer_group.team_ids.as_deref(),
                person,
                &mut |step, count| budget.fairness(step, count),
            )? {
                budget.rows::<PersonId>(1)?;
                peers.push(*id);
                insert(people, *id, budget)?;
            }
        }
        let mut matching = Vec::new();
        for shift in &input.shifts {
            budget.work(1)?;
            let metadata = input
                .metadata(shift)
                .map_err(|error| operation_error(&error))?;
            let parent_metadata = ShiftMetadata {
                assignment_type_id: metadata.assignment_type.id,
                location_id: metadata.location_id,
                coverage: metadata.coverage,
            };
            budget.work(metadata.assignment_type.workload_bucket_ids.len())?;
            if metadata
                .assignment_type
                .workload_bucket_ids
                .contains(&plan.bucket_id)
                && fairness_shift_matches(
                    scope,
                    shift,
                    &parent_metadata,
                    metadata.assignment_type,
                    &mut |step, count| budget.fairness(step, count),
                )?
            {
                budget.retain(1, size_of::<&crate::temporal::ResolvedShift>() * 2)?;
                matching.push(shift);
            }
        }
        let periods = fairness_periods(
            document,
            &input.domain,
            plan,
            &matching,
            &mut |step, count| budget.fairness(step, count),
        )?;
        let mut index = build_fairness_actual_index(
            &matching,
            &periods.shift_periods,
            &peers,
            plan.membership,
            bucket.measurement,
            bucket.overlapping_contribution,
            selected,
            &mut |step, count| budget.fairness(step, count),
        )?;
        let mut contributing = BTreeSet::new();
        for pair in selected.keys() {
            budget.work(1)?;
            if peers.binary_search(&pair.person_id).is_ok()
                && periods.shift_periods.contains_key(&pair.shift_id)
            {
                insert(&mut contributing, pair.shift_id, budget)?;
            }
        }
        let mut qualifying = Vec::new();
        // Only actual selected peer contributions, not an unassigned raw selector population.
        for id in contributing {
            budget.rows::<EntityId>(1)?;
            insert(shifts, id, budget)?;
            qualifying.push(id.as_entity_id());
        }
        let mut output_periods = Vec::new();
        for window in periods.windows {
            budget.rows::<RecipientActualPeriodV1>(1)?;
            budget.rows::<RecipientActualV1>(peers.len())?;
            let key = FairnessPeriodKey {
                calendar_id: window.calendar_id,
                start: window.interval.starts_at.instant,
                end: window.interval.ends_at.instant,
            };
            let mut values = Vec::with_capacity(peers.len());
            for id in &peers {
                let value = index.actual(
                    key,
                    *id,
                    bucket.measurement,
                    bucket.overlapping_contribution,
                    &mut |step, count| budget.fairness(step, count),
                )?;
                // i128 decimal representation has a fixed maximum of 40 bytes per component.
                budget.retain(2, 40)?;
                values.push(RecipientActualV1 {
                    person_id: *id,
                    actual: RecipientFractionV1 {
                        numerator: value.numerator.to_string(),
                        denominator: value.denominator.to_string(),
                    },
                });
            }
            output_periods.push(RecipientActualPeriodV1 {
                interval: interval(window.interval)?,
                actuals: values,
            });
        }
        groups.push(RecipientActualGroupV1 {
            measure: match bucket.measurement {
                WorkloadMeasurement::AssignmentCount => RecipientWorkloadMeasureV1::AssignmentCount,
                WorkloadMeasurement::ElapsedMinutes => RecipientWorkloadMeasureV1::ElapsedMinutes,
                WorkloadMeasurement::ScheduledMinutes => {
                    RecipientWorkloadMeasureV1::ScheduledMinutes
                }
            },
            membership: match plan.membership {
                WindowMembership::ReportingDate => RecipientMembershipV1::ReportingDate,
                WindowMembership::StartInstant => RecipientMembershipV1::StartInstant,
                WindowMembership::Intersection => RecipientMembershipV1::Intersection,
            },
            overlap: match bucket.overlapping_contribution {
                OverlappingContribution::Sum => RecipientOverlapV1::Sum,
                OverlappingContribution::Union => RecipientOverlapV1::Union,
            },
            people: peers,
            qualifying_shifts: qualifying,
            periods: output_periods,
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn changes(
    document: &ScenarioDocument,
    context: Option<&RunObjectiveContext>,
    input: &AssignmentInput,
    selected: &Selected<'_>,
    original: Option<RecipientAcceptedSource<'_>>,
    manual: Option<RecipientManualSource<'_>>,
    options: RecipientReportOptionsV1,
    source_budget: &mut OperationBudget<'_>,
    budget: &mut ReportBudget<'_>,
) -> Result<Vec<RecipientChangesV1>, DomainPackError> {
    let mut output = Vec::new();
    let Some(binding) = context.and_then(|context| context.baseline.as_deref()) else {
        if original.is_some() || manual.is_some() {
            return Err(missing());
        }
        if let Some(pairs) = selected_baseline_pairs(
            &input.domain,
            input.captured_original.as_ref(),
            document.scenario_id,
            context,
            source_budget,
        )? && options.include_changes
        {
            output.push(comparison(
                RecipientChangeBasisV1::ExternalStartingPlan,
                None,
                pairs,
                selected,
                None,
                options,
                budget,
            )?);
        }
        return Ok(output);
    };
    let (base, before) = baseline_sources(
        &input.domain,
        input.captured_original.as_ref(),
        document.scenario_id,
        binding,
        source_budget,
    )?;
    let historical_a = if let Some(source) = original {
        budget.source(source.document, Some(source.accepted))?;
        let reference = &binding.original;
        if source.document.scenario_id != reference.source_scenario_id
            || source.accepted.solution.scenario_id != reference.source_scenario_id
            || source.accepted.solution.scenario_revision != reference.source_revision
            || source.accepted.solution.solution_id != reference.accepted_result.solution_id
            || source.accepted.checksum != reference.accepted_result.result_checksum
            || source.accepted.verification.document_hash != reference.source_document_hash
        {
            return Err(missing());
        }
        let (historical, pairs) = recipient_source(
            source.document,
            source.accepted,
            source.objective_context,
            source_budget,
        )?;
        bind_original_pairs(&base.assignments, &pairs, budget)?;
        Some(historical)
    } else {
        None
    };
    let historical_b = match (&binding.selection, manual) {
        (
            RunBaselineSelection::ManualSelection {
                admission_source_scenario_id,
                admission_source_revision,
                admission_source_document_hash,
                ..
            },
            Some(source),
        ) => {
            budget.source(source.document, None)?;
            if source.document.scenario_id != *admission_source_scenario_id
                || source.revision.value() != *admission_source_revision
            {
                return Err(missing());
            }
            let historical = AssignmentInput::new(source.document, source_budget)
                .map_err(|error| operation_error(&error))?;
            if historical.source_document_hash != *admission_source_document_hash {
                return Err(missing());
            }
            Some(historical)
        }
        (RunBaselineSelection::OriginalAccepted {}, Some(_)) => return Err(missing()),
        (_, None) => None,
    };
    if options.include_changes {
        budget.retain(1, 64)?;
        let original = RecipientOriginalV1 {
            scenario_id: binding.original.source_scenario_id,
            scenario_revision: base.source_revision,
            solution_id: binding.original.accepted_result.solution_id,
            accepted_result_checksum: budget
                .text(&binding.original.accepted_result.result_checksum)?,
            admission: RecipientOriginalAdmissionV1::NotEstablished,
        };
        output.push(comparison(
            RecipientChangeBasisV1::RecordedOriginal,
            Some(original),
            &base.assignments,
            selected,
            historical_a.as_ref(),
            options,
            budget,
        )?);
        if matches!(
            binding.selection,
            RunBaselineSelection::ManualSelection { .. }
        ) {
            output.push(comparison(
                RecipientChangeBasisV1::RecordedManual,
                None,
                before,
                selected,
                historical_b.as_ref(),
                options,
                budget,
            )?);
        }
    }
    Ok(output)
}

fn bind_original_pairs(
    recorded: &[AssignmentPair],
    selected: &Selected<'_>,
    budget: &mut ReportBudget<'_>,
) -> Result<(), DomainPackError> {
    if recorded.len() != selected.len() {
        return Err(missing());
    }
    let mut seen = BTreeSet::new();
    for pair in recorded {
        budget.retain(1, 128)?;
        if !selected.contains_key(pair) || !seen.insert(*pair) {
            return Err(missing());
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn comparison(
    basis: RecipientChangeBasisV1,
    original: Option<RecipientOriginalV1>,
    before: &[AssignmentPair],
    selected: &Selected<'_>,
    historical: Option<&AssignmentInput>,
    options: RecipientReportOptionsV1,
    budget: &mut ReportBudget<'_>,
) -> Result<RecipientChangesV1, DomainPackError> {
    budget.rows::<RecipientChangesV1>(1)?;
    let mut old = BTreeSet::new();
    for pair in before {
        budget.retain(1, 128)?;
        if !old.insert(*pair) {
            return Err(missing());
        }
    }
    let mut rows = Vec::new();
    // BTree merge retains every old and new pair without a second complete union allocation.
    let mut previous_pairs = old.iter().peekable();
    let mut current_pairs = selected.keys().peekable();
    loop {
        let pair = match (previous_pairs.peek(), current_pairs.peek()) {
            (Some(previous), Some(current)) => match previous.cmp(current) {
                std::cmp::Ordering::Less => previous_pairs.next(),
                std::cmp::Ordering::Greater => current_pairs.next(),
                std::cmp::Ordering::Equal => {
                    current_pairs.next();
                    previous_pairs.next()
                }
            },
            (Some(_), None) => previous_pairs.next(),
            (None, Some(_)) => current_pairs.next(),
            (None, None) => break,
        }
        .ok_or_else(missing)?;
        budget.rows::<RecipientChangeRowV1>(1)?;
        let previous = if old.contains(pair) {
            Some(historical_facts(historical, *pair, options, budget)?)
        } else {
            None
        };
        rows.push(RecipientChangeRowV1 {
            person_id: pair.person_id,
            shift_id: pair.shift_id.as_entity_id(),
            previous,
            membership: match (old.contains(pair), selected.contains_key(pair)) {
                (true, true) => RecipientPairChangeV1::Retained,
                (true, false) => RecipientPairChangeV1::Removed,
                _ => RecipientPairChangeV1::Added,
            },
        });
    }
    Ok(RecipientChangesV1 {
        basis,
        original,
        rows,
    })
}

fn historical_facts(
    input: Option<&AssignmentInput>,
    pair: AssignmentPair,
    options: RecipientReportOptionsV1,
    budget: &mut ReportBudget<'_>,
) -> Result<RecipientHistoricalEndpointV1, DomainPackError> {
    let Some(input) = input else {
        return Ok(RecipientHistoricalEndpointV1::Unavailable);
    };
    let (Some(person), Some(shift)) = (input.person(pair.person_id), input.shift(pair.shift_id))
    else {
        return Ok(RecipientHistoricalEndpointV1::Unavailable);
    };
    let metadata = input
        .metadata(shift)
        .map_err(|error| operation_error(&error))?;
    let location = if let Some(id) = metadata.location_id {
        let Some(WorkforceEntity::Location(entity)) = input.domain.entities.get(&id.as_entity_id())
        else {
            return Ok(RecipientHistoricalEndpointV1::Unavailable);
        };
        Some(RecipientEntityV1 {
            entity_id: id.as_entity_id(),
            label: budget.label(
                "Location",
                id,
                &entity.name,
                options.include_authored_labels,
            )?,
        })
    } else {
        None
    };
    budget.retain(1, 10)?;
    Ok(RecipientHistoricalEndpointV1::Available(
        RecipientHistoricalFactsV1 {
            person_label: budget.label(
                "Participant",
                person.id,
                &person.name,
                options.include_person_names,
            )?,
            assignment_type: RecipientEntityV1 {
                entity_id: metadata.assignment_type.id.as_entity_id(),
                label: budget.label(
                    "Work",
                    metadata.assignment_type.id,
                    &metadata.assignment_type.name,
                    options.include_authored_labels,
                )?,
            },
            location,
            interval: interval(shift.interval)?,
            reporting_date: shift.reporting_date.to_string(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assignment_rules::{
        compile_assignment_rules, score_workforce_solution, verify_workforce_solution,
        workforce_baseline_reference, workforce_external_baseline_reference,
        workforce_manual_selection_checksum, workforce_pair_content_hash,
        workforce_verification_scope,
    };
    use crate::model::ManualSelection;
    use crate::test_support::{fixture, id};
    use eutheto_domain_api::CompileContext;
    use eutheto_domain_ir::{
        AcceptedResultRefV1, AssignmentValue, DomainAssignment, DomainAssignmentId, DomainEntityId,
        DomainEntityKindId, DomainEntityRef, NormalizedSolution, ObjectiveProfileId,
        RunBaselineBinding, RunBaselineOriginal, RunExternalBaselineBinding, RunObjectiveOperation,
        VerificationContextV1, blake3_hex,
    };
    use eutheto_planning_ir::PlanningIrLimitsV1;
    use eutheto_types::{CancellationToken, OverlapPolicy};
    use serde_json::json;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn control() -> OperationControl {
        OperationControl::Cancellation(CancellationToken::new())
    }

    fn pair(person: u32, shift: u32) -> TestResult<AssignmentPair> {
        Ok(AssignmentPair {
            person_id: id(person).parse()?,
            shift_id: id(shift).parse()?,
        })
    }

    fn source() -> TestResult<ScenarioDocument> {
        let mut document = fixture()?;
        document.settings.overlap_policy = OverlapPolicy::Earlier;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        let shift = document
            .domain
            .entities
            .get_mut(&id(8).parse()?)
            .ok_or("shift")?;
        shift["coverage"] = json!({"kind":"atLeast","minimum":0,"qualificationMinimums":[]});
        shift["endsAt"] = json!({"instant":"2026-11-01T05:30:30.5Z",
            "local":"2026-11-01T01:30:30.5","offsetSeconds":-14400});
        let mut second = document.domain.entities[&id(1).parse()?].clone();
        second["id"] = json!(id(20));
        second["name"] = json!("Private second participant");
        second["externalId"] = json!("staff-20");
        document.domain.entities.insert(id(20).parse()?, second);
        let mut zero_shift = document.domain.entities[&id(8).parse()?].clone();
        zero_shift["id"] = json!(id(18));
        document.domain.entities.insert(id(18).parse()?, zero_shift);
        Ok(document)
    }

    fn accepted(
        document: &ScenarioDocument,
        objective: Option<&RunObjectiveContext>,
        decisions: &[(u32, u32, bool)],
    ) -> TestResult<AcceptedResult> {
        let compiled = compile_assignment_rules(
            document,
            &CompileContext {
                objective_context: objective.cloned(),
                scenario_revision: 1,
                semantic_metadata: BTreeMap::new(),
                control: control(),
                planning_limits: PlanningIrLimitsV1::DEFAULT,
            },
        )?;
        let mut assignments = Vec::new();
        for (person, shift, selected) in decisions {
            let pair = pair(*person, *shift)?;
            let variable = compiled
                .variables
                .iter()
                .find(|entry| entry.pair == pair)
                .ok_or("compiled native pair")?;
            let identity = format!("{}.{}", pair.person_id, pair.shift_id);
            assignments.push(DomainAssignment {
                id: DomainAssignmentId::new(format!("official.workforce.assignment.{identity}"))?,
                entity: DomainEntityRef {
                    kind: DomainEntityKindId::new("official.workforce.assignment")?,
                    id: DomainEntityId::new(identity)?,
                },
                value: AssignmentValue::Boolean(*selected),
                evidence: vec![variable.variable.provenance.as_str().parse()?],
            });
        }
        let solution = NormalizedSolution {
            schema_version: 1,
            pack_id: document.domain_pack.id.clone(),
            scenario_id: document.scenario_id,
            scenario_revision: 1,
            projection_version: 1,
            solution_id: id(700).parse()?,
            assignments,
        };
        let authoritative_score = score_workforce_solution(document, &solution, objective, None)?;
        let scope = workforce_verification_scope(document, 1, None)?;
        let context = VerificationContextV1::new(
            document.scenario_id,
            1,
            blake3_hex(&serde_json::to_vec(document)?),
            "a".repeat(64),
            solution.canonical_hash()?,
            scope.checksum,
        )?;
        let report = verify_workforce_solution(
            document,
            &solution,
            objective,
            &context,
            &authoritative_score,
            None,
        )?;
        Ok(AcceptedResult::new(solution, report)?)
    }

    fn options() -> RecipientReportOptionsV1 {
        RecipientReportOptionsV1 {
            include_staffing: true,
            include_changes: true,
            include_fairness_actuals: true,
            ..RecipientReportOptionsV1::default()
        }
    }

    fn baseline(
        document: &mut ScenarioDocument,
        original: &AcceptedResult,
        recorded: &[AssignmentPair],
    ) -> TestResult<RunObjectiveContext> {
        let hash = workforce_pair_content_hash(recorded, None)?;
        let reference = RunBaselineOriginal {
            entity: workforce_baseline_reference(id(40).parse()?, false)?,
            pair_content_hash: hash.clone(),
            accepted_result: AcceptedResultRefV1 {
                solution_id: original.solution.solution_id,
                result_checksum: original.checksum.clone(),
            },
            source_scenario_id: original.solution.scenario_id,
            source_revision: original.solution.scenario_revision,
            source_document_hash: original.verification.document_hash.clone(),
            source_run_id: id(42).parse()?,
            source_run_input_checksum: "c".repeat(64),
            source_run_manifest_checksum: "d".repeat(64),
        };
        document.domain.entities.insert(
            id(40).parse()?,
            json!({
                "kind":"baseSchedule","id":id(40),"sourceSolutionId":original.solution.solution_id,
                "sourceRevision":1,"assignments":recorded
            }),
        );
        Ok(RunObjectiveContext {
            schema_version: 1,
            pack_id: document.domain_pack.id.clone(),
            profile_id: ObjectiveProfileId::new("official.workforce.objective.minimal-changes")?,
            semantic_version: 1,
            operation: RunObjectiveOperation::Repair,
            baseline: Some(Box::new(RunBaselineBinding {
                original: reference,
                selection: RunBaselineSelection::OriginalAccepted {},
                selected_pair_content_hash: hash,
                captured_original: None,
            })),
            external_baseline: None,
        })
    }

    #[test]
    fn schedule_uses_exact_instant_order_instead_of_shift_identity() -> TestResult {
        let mut document = source()?;
        document
            .domain
            .entities
            .get_mut(&id(8).parse()?)
            .ok_or("shift")?["startsAt"] = json!({"instant":"2026-11-01T05:30:00.25Z",
                "local":"2026-11-01T01:30:00.25","offsetSeconds":-14400});
        let accepted = accepted(&document, None, &[(1, 8, true), (20, 18, true)])?;
        let report = build_workforce_recipient_report(
            &document,
            &accepted,
            None,
            None,
            None,
            RecipientReportOptionsV1::default(),
            &control(),
        )?;
        assert_eq!(
            report
                .shifts
                .iter()
                .map(|shift| shift.shift_id)
                .collect::<Vec<_>>(),
            vec![id(18).parse()?, id(8).parse()?]
        );
        Ok(())
    }

    #[test]
    // Keep the full disclosure matrix and deterministic census assertions on one accepted source.
    #[allow(clippy::too_many_lines)]
    fn complete_optional_censuses_are_private_fractional_and_deterministic() -> TestResult {
        let mut document = source()?;
        for preference in [30, 31] {
            document.domain.preferences.insert(
                id(preference).parse()?,
                json!({
                    "kind":"workloadBalance","id":id(preference),"active":true,"priority":"normal",
                    "weight":1,"scope":{"people":{"kind":"all"}},"workloadPolicyId":id(10)
                }),
            );
        }
        let accepted = accepted(&document, None, &[(1, 8, true), (20, 8, false)])?;
        let report = build_workforce_recipient_report(
            &document,
            &accepted,
            None,
            None,
            None,
            options(),
            &control(),
        )?;
        assert_eq!(
            report,
            build_workforce_recipient_report(
                &document,
                &accepted,
                None,
                None,
                None,
                options(),
                &control()
            )?
        );
        assert_eq!(
            report.assignments[0].assignment_id,
            accepted.solution.assignments[0].id.as_str()
        );
        assert_eq!(
            report
                .staffing
                .as_ref()
                .ok_or("staffing")?
                .iter()
                .map(|row| row.assigned_count)
                .sum::<u32>(),
            1
        );
        assert_eq!(report.staffing.as_ref().ok_or("staffing")?.len(), 2);
        let groups = report.fairness_actuals.as_ref().ok_or("actuals")?;
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].people.len(), 2);
        assert_eq!(
            groups[0].qualifying_shifts,
            vec![pair(1, 8)?.shift_id.as_entity_id()]
        );
        assert_eq!(
            groups[0].periods[0].actuals[0].actual,
            RecipientFractionV1 {
                numerator: "61".into(),
                denominator: "120".into()
            }
        );
        assert_eq!(groups[0].periods[0].actuals[1].actual.numerator, "0");
        let encoded = serde_json::to_string(&report)?;
        for forbidden in [
            "River",
            "Private second participant",
            "Clinic",
            "North",
            "Days",
            "workloadPolicy",
            "target",
            "deviation",
            "qualification",
            "externalId",
        ] {
            assert!(!encoded.contains(forbidden), "{forbidden}");
        }
        let named = build_workforce_recipient_report(
            &document,
            &accepted,
            None,
            None,
            None,
            RecipientReportOptionsV1 {
                include_person_names: true,
                include_authored_labels: true,
                ..options()
            },
            &control(),
        )?;
        assert!(
            matches!(&named.people[0].label, RecipientLabelV1::Authored(name) if name == "River")
        );
        assert!(
            matches!(&named.assignment_types[0].label, RecipientLabelV1::Authored(name) if name == "Clinic")
        );
        let default = build_workforce_recipient_report(
            &document,
            &accepted,
            None,
            None,
            None,
            RecipientReportOptionsV1::default(),
            &control(),
        )?;
        assert_eq!(default.people.len(), 1);
        assert_eq!(default.shifts.len(), 1);
        assert!(
            default.staffing.is_none()
                && default.changes.is_none()
                && default.fairness_actuals.is_none()
        );
        assert_eq!(default.people[0].label, report.people[0].label);
        Ok(())
    }

    #[test]
    // One exact A/B/C chain exercises metadata separation and a freshly checksummed counterfeit.
    #[allow(clippy::too_many_lines)]
    fn exact_original_and_manual_history_never_borrow_current_metadata() -> TestResult {
        let a_document = source()?;
        let a = accepted(&a_document, None, &[(1, 8, true)])?;
        let mut b_document = source()?;
        b_document
            .domain
            .entities
            .get_mut(&id(1).parse()?)
            .ok_or("person")?["name"] = json!("Manual person");
        b_document
            .domain
            .entities
            .get_mut(&id(8).parse()?)
            .ok_or("shift")?["endsAt"] = json!({"instant":"2026-11-01T05:31:30.5Z",
                "local":"2026-11-01T01:31:30.5","offsetSeconds":-14400});
        let b_hash = blake3_hex(&serde_json::to_vec(&b_document)?);
        let mut c_document = source()?;
        c_document
            .domain
            .entities
            .get_mut(&id(1).parse()?)
            .ok_or("person")?["name"] = json!("Current person");
        c_document
            .domain
            .entities
            .get_mut(&id(8).parse()?)
            .ok_or("shift")?["endsAt"] = json!({"instant":"2026-11-01T05:32:30.5Z",
                "local":"2026-11-01T01:32:30.5","offsetSeconds":-14400});
        let mut objective = baseline(&mut c_document, &a, &[pair(1, 8)?])?;
        let mut record = ManualSelection {
            schema_version: 1,
            id: id(50).parse()?,
            original: objective
                .baseline
                .as_ref()
                .ok_or("baseline")?
                .original
                .clone(),
            admission_source_scenario_id: b_document.scenario_id,
            admission_source_revision: Revision::new(1),
            admission_source_document_hash: b_hash.clone(),
            admission_command_id: id(51).parse()?,
            assignments: vec![pair(1, 8)?],
            checksum: String::new(),
        };
        record.checksum = workforce_manual_selection_checksum(&record, None)?;
        let binding = objective.baseline.as_mut().ok_or("baseline")?;
        binding.selection = RunBaselineSelection::ManualSelection {
            entity: workforce_baseline_reference(record.id, true)?,
            record_checksum: record.checksum.clone(),
            admission_source_scenario_id: b_document.scenario_id,
            admission_source_revision: 1,
            admission_source_document_hash: b_hash,
        };
        c_document.domain.entities.insert(
            record.id,
            serde_json::to_value(WorkforceEntity::ManualSelection(record))?,
        );
        let c = accepted(&c_document, Some(&objective), &[(1, 8, true)])?;
        let privacy = RecipientReportOptionsV1 {
            include_person_names: true,
            ..options()
        };
        let original = Some(RecipientAcceptedSource {
            document: &a_document,
            accepted: &a,
            objective_context: None,
        });
        let manual = Some(RecipientManualSource {
            document: &b_document,
            revision: Revision::new(1),
        });
        let report = build_workforce_recipient_report(
            &c_document,
            &c,
            Some(&objective),
            original,
            manual,
            privacy,
            &control(),
        )?;
        let changes = report.changes.ok_or("changes")?;
        assert_eq!(changes.len(), 2);
        assert_eq!(
            changes[0].original.as_ref().ok_or("original")?.admission,
            RecipientOriginalAdmissionV1::NotEstablished
        );
        for (comparison, (expected, expected_end)) in changes.iter().zip([
            ("River", "2026-11-01T05:30:30.5Z"),
            ("Manual person", "2026-11-01T05:31:30.5Z"),
        ]) {
            let Some(RecipientHistoricalEndpointV1::Available(facts)) =
                &comparison.rows[0].previous
            else {
                return Err("history".into());
            };
            assert_eq!(
                facts.person_label,
                RecipientLabelV1::Authored(expected.into())
            );
            assert_eq!(
                facts.interval.ends_at.resolved.instant.to_string(),
                expected_end
            );
        }
        let missing = build_workforce_recipient_report(
            &c_document,
            &c,
            Some(&objective),
            None,
            None,
            privacy,
            &control(),
        )?;
        for comparison in missing.changes.ok_or("changes")? {
            assert_eq!(comparison.rows.len(), 1);
            assert_eq!(
                comparison.rows[0].previous,
                Some(RecipientHistoricalEndpointV1::Unavailable)
            );
        }
        assert!(
            build_workforce_recipient_report(
                &c_document,
                &c,
                Some(&objective),
                original,
                Some(RecipientManualSource {
                    document: &a_document,
                    revision: Revision::new(1)
                }),
                privacy,
                &control()
            )
            .is_err()
        );
        let counterfeit_pairs = vec![pair(20, 8)?];
        let counterfeit_hash = workforce_pair_content_hash(&counterfeit_pairs, None)?;
        c_document
            .domain
            .entities
            .get_mut(&id(40).parse()?)
            .ok_or("base")?["assignments"] = json!(counterfeit_pairs);
        let binding = objective.baseline.as_mut().ok_or("baseline")?;
        binding.original.pair_content_hash = counterfeit_hash;
        let WorkforceEntity::ManualSelection(mut manual_record) =
            serde_json::from_value(c_document.domain.entities[&id(50).parse()?].clone())?
        else {
            return Err("manual selection".into());
        };
        manual_record.original = binding.original.clone();
        manual_record.checksum = workforce_manual_selection_checksum(&manual_record, None)?;
        if let RunBaselineSelection::ManualSelection {
            record_checksum, ..
        } = &mut binding.selection
        {
            *record_checksum = manual_record.checksum.clone();
        }
        c_document.domain.entities.insert(
            manual_record.id,
            serde_json::to_value(WorkforceEntity::ManualSelection(manual_record))?,
        );
        let counterfeit = accepted(&c_document, Some(&objective), &[(1, 8, true)])?;
        assert!(
            build_workforce_recipient_report(
                &c_document,
                &counterfeit,
                Some(&objective),
                original,
                manual,
                privacy,
                &control()
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn original_pair_contradictions_reject_embedded_and_captured_sources() -> TestResult {
        let a_document = source()?;
        let a = accepted(&a_document, None, &[(1, 8, true)])?;
        for captured in [false, true] {
            for counterfeit in [false, true] {
                let mut document = source()?;
                let pairs = vec![pair(if counterfeit { 20 } else { 1 }, 8)?];
                let mut objective = baseline(&mut document, &a, &pairs)?;
                if captured {
                    objective.schema_version = 2;
                    let binding = objective.baseline.as_mut().ok_or("binding")?;
                    let carrier_id = EntityId::from_uuid(a.solution.solution_id.as_uuid());
                    binding.original.entity = workforce_baseline_reference(carrier_id, false)?;
                    binding.captured_original = Some(std::sync::Arc::new(json!({
                        "kind":"baseSchedule","id":carrier_id,"sourceSolutionId":a.solution.solution_id,
                        "sourceRevision":1,"assignments":pairs
                    })));
                    document.domain.entities.remove(&id(40).parse()?);
                }
                let c = accepted(&document, Some(&objective), &[(1, 8, true)])?;
                let result = build_workforce_recipient_report(
                    &document,
                    &c,
                    Some(&objective),
                    Some(RecipientAcceptedSource {
                        document: &a_document,
                        accepted: &a,
                        objective_context: None,
                    }),
                    None,
                    options(),
                    &control(),
                );
                assert_eq!(result.is_err(), counterfeit);
                if let Ok(report) = result {
                    assert_eq!(report.changes.ok_or("changes")?.len(), 1);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn external_history_is_unavailable_and_false_assignment_provenance_is_checked() -> TestResult {
        let mut document = source()?;
        let before = vec![pair(20, 8)?];
        document.domain.entities.insert(id(50).parse()?, json!({
            "kind":"externalStartingPlan","schemaVersion":1,"id":id(50),"name":"Secret old plan",
            "assignments":before,"originalImport":{"sourceBlake3":"a".repeat(64),"pairContentHash":"b".repeat(64)}
        }));
        let objective = RunObjectiveContext {
            schema_version: 3,
            pack_id: document.domain_pack.id.clone(),
            profile_id: ObjectiveProfileId::new("official.workforce.objective.minimal-changes")?,
            semantic_version: 1,
            operation: RunObjectiveOperation::Repair,
            baseline: None,
            external_baseline: Some(Box::new(RunExternalBaselineBinding {
                entity: workforce_external_baseline_reference(id(50).parse()?)?,
                selected_pair_content_hash: workforce_pair_content_hash(&before, None)?,
            })),
        };
        let c = accepted(&document, Some(&objective), &[(1, 8, true), (20, 8, false)])?;
        let report = build_workforce_recipient_report(
            &document,
            &c,
            Some(&objective),
            None,
            None,
            options(),
            &control(),
        )?;
        let changes = report.changes.ok_or("changes")?;
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].basis,
            RecipientChangeBasisV1::ExternalStartingPlan
        );
        assert!(changes[0].original.is_none());
        assert_eq!(changes[0].rows.len(), 2);
        assert_eq!(
            changes[0].rows[1].previous,
            Some(RecipientHistoricalEndpointV1::Unavailable)
        );
        let mut solution = c.solution;
        solution.assignments[1].evidence = solution.assignments[0].evidence.clone();
        let scope = workforce_verification_scope(&document, 1, None)?;
        let context = VerificationContextV1::new(
            document.scenario_id,
            1,
            blake3_hex(&serde_json::to_vec(&document)?),
            "a".repeat(64),
            solution.canonical_hash()?,
            scope.checksum,
        )?;
        let authoritative_score =
            score_workforce_solution(&document, &solution, Some(&objective), None)?;
        let verification = verify_workforce_solution(
            &document,
            &solution,
            Some(&objective),
            &context,
            &authoritative_score,
            None,
        )?;
        let forged = AcceptedResult::new(solution, verification)?;
        assert!(
            build_workforce_recipient_report(
                &document,
                &forged,
                Some(&objective),
                None,
                None,
                RecipientReportOptionsV1::default(),
                &control()
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn actual_only_helpers_keep_union_and_unavailable_history() -> TestResult {
        let mut document = source()?;
        document.domain.preferences.insert(
            id(30).parse()?,
            json!({
                "kind":"workloadBalance","id":id(30),"active":true,"priority":"normal",
                "weight":1,"scope":{"people":{"kind":"all"}},"workloadPolicyId":id(10)
            }),
        );
        let control = control();
        let mut source_budget = OperationBudget::evaluation(Some(&control));
        let input = AssignmentInput::new(&document, &mut source_budget)?;
        let selected = BTreeMap::from([(pair(1, 8)?, "native-8"), (pair(1, 18)?, "native-18")]);
        let mut groups = Vec::new();
        actuals(
            &document,
            &input,
            &selected,
            &mut groups,
            &mut BTreeSet::new(),
            &mut BTreeSet::new(),
            &mut ReportBudget::new(&control),
        )?;
        assert_eq!(
            groups[0].periods[0].actuals[0].actual,
            RecipientFractionV1 {
                numerator: "61".into(),
                denominator: "120".into()
            }
        );
        let mut missing_document = source()?;
        missing_document.domain.entities.remove(&id(20).parse()?);
        missing_document.domain.entities.remove(&id(18).parse()?);
        let missing_input = AssignmentInput::new(&missing_document, &mut source_budget)?;
        for pair in [pair(20, 8)?, pair(1, 18)?] {
            assert_eq!(
                historical_facts(
                    Some(&missing_input),
                    pair,
                    options(),
                    &mut ReportBudget::new(&control)
                )?,
                RecipientHistoricalEndpointV1::Unavailable
            );
        }
        Ok(())
    }

    #[test]
    fn scheduled_actuals_preserve_signed_fractional_fold_time() -> TestResult {
        let mut document = source()?;
        document
            .domain
            .entities
            .get_mut(&id(3).parse()?)
            .ok_or("bucket")?["measurement"] = json!("scheduledMinutes");
        document
            .domain
            .entities
            .get_mut(&id(3).parse()?)
            .ok_or("bucket")?["overlappingContribution"] = json!("sum");
        document
            .domain
            .entities
            .get_mut(&id(8).parse()?)
            .ok_or("shift")?["endsAt"] = json!({
            "instant":"2026-11-01T06:29:30.5Z","local":"2026-11-01T01:29:30.5","offsetSeconds":-18000
        });
        document.domain.preferences.insert(
            id(30).parse()?,
            json!({
                "kind":"workloadBalance","id":id(30),"active":true,"priority":"normal",
                "weight":1,"scope":{"people":{"kind":"all"}},"workloadPolicyId":id(10)
            }),
        );
        let accepted = accepted(&document, None, &[(1, 8, true)])?;
        let report = build_workforce_recipient_report(
            &document,
            &accepted,
            None,
            None,
            None,
            options(),
            &control(),
        )?;
        let group = &report.fairness_actuals.as_ref().ok_or("actuals")?[0];
        assert_eq!(group.measure, RecipientWorkloadMeasureV1::ScheduledMinutes);
        assert_eq!(
            group.periods[0].actuals[0].actual,
            RecipientFractionV1 {
                numerator: "-59".into(),
                denominator: "120".into()
            }
        );
        Ok(())
    }

    #[test]
    fn optional_groups_share_work_and_calendar_interruption() -> TestResult {
        let mut document = source()?;
        let preference = json!({
            "kind":"workloadBalance","id":id(30),"active":true,"priority":"normal",
            "weight":1,"scope":{"people":{"kind":"all"}},"workloadPolicyId":id(10)
        });
        document
            .domain
            .preferences
            .insert(id(30).parse()?, preference.clone());
        let control = control();
        let input =
            AssignmentInput::new(&document, &mut OperationBudget::evaluation(Some(&control)))?;
        let selected = BTreeMap::from([(pair(1, 8)?, "native-8")]);
        let mut budget = ReportBudget::new(&control);
        actuals(
            &document,
            &input,
            &selected,
            &mut Vec::new(),
            &mut BTreeSet::new(),
            &mut BTreeSet::new(),
            &mut budget,
        )?;
        let one_group_work = budget.work;
        let mut second = preference;
        second["id"] = json!(id(31));
        document.domain.preferences.insert(id(31).parse()?, second);
        let input =
            AssignmentInput::new(&document, &mut OperationBudget::evaluation(Some(&control)))?;
        let mut budget = ReportBudget::new(&control);
        budget.work = 10_000_000 - one_group_work;
        let mut groups = Vec::new();
        assert!(matches!(
            actuals(
                &document,
                &input,
                &selected,
                &mut groups,
                &mut BTreeSet::new(),
                &mut BTreeSet::new(),
                &mut budget
            ),
            Err(DomainPackError::ResourceLimitExceeded)
        ));
        assert_eq!(groups.len(), 1);

        let cancellation = CancellationToken::new();
        let inherited = OperationControl::Cancellation(cancellation);
        let mut budget = ReportBudget::new(&inherited);
        budget.scratch.cancel_after_steps(20)?;
        assert!(matches!(
            actuals(
                &document,
                &input,
                &selected,
                &mut Vec::new(),
                &mut BTreeSet::new(),
                &mut BTreeSet::new(),
                &mut budget
            ),
            Err(DomainPackError::Cancelled)
        ));

        let clock = std::sync::Arc::new(eutheto_types::FixedMonotonicClock::default());
        let parent = eutheto_types::ParentSolveBudget::new(
            eutheto_types::DurationMillis::new(1)?,
            clock.clone(),
            CancellationToken::new(),
        )?;
        let inherited = OperationControl::Solve(parent.phase_view());
        clock.set(std::time::Duration::from_millis(1))?;
        assert!(matches!(
            actuals(
                &document,
                &input,
                &selected,
                &mut Vec::new(),
                &mut BTreeSet::new(),
                &mut BTreeSet::new(),
                &mut ReportBudget::new(&inherited)
            ),
            Err(DomainPackError::BudgetExpired)
        ));
        Ok(())
    }

    #[test]
    fn nested_selectors_exhaust_the_real_report_and_observe_inherited_control() -> TestResult {
        let mut document = source()?;
        let mut tags = (0..999)
            .map(|index| format!("person-tag-{index}"))
            .collect::<Vec<_>>();
        tags.push("night".into());
        for person in [1, 20] {
            document
                .domain
                .entities
                .get_mut(&id(person).parse()?)
                .ok_or("person")?["tags"] = json!(tags);
        }
        let mut selector = (0..999)
            .map(|index| format!("missing-tag-{index}"))
            .collect::<Vec<_>>();
        selector.push("night".into());
        document
            .domain
            .entities
            .get_mut(&id(9).parse()?)
            .ok_or("policy")?["workloadPolicies"][id(10)]["peerGroup"]["people"] =
            json!({"kind":"filter","allTags":[],"anyTags":selector});
        for preference in 30..36 {
            document.domain.preferences.insert(
                id(preference).parse()?,
                json!({
                    "kind":"workloadBalance","id":id(preference),"active":true,"priority":"normal",
                    "weight":1,"scope":{"people":{"kind":"all"}},"workloadPolicyId":id(10)
                }),
            );
        }
        let accepted = accepted(&document, None, &[(1, 8, true)])?;
        // Fresh source verification has its independent 20m allowance. Only disclosed actuals
        // repeat these scans under the report's shared 10m allowance. The old outer-only checks
        // admitted all six groups although 2 people × 1000 selector tags × 1000 source tags
        // × 6 contexts inspect 12m tag candidates, with the sole match at each vector's tail.
        build_workforce_recipient_report(
            &document,
            &accepted,
            None,
            None,
            None,
            RecipientReportOptionsV1::default(),
            &control(),
        )?;
        assert!(matches!(
            build_workforce_recipient_report(
                &document,
                &accepted,
                None,
                None,
                None,
                RecipientReportOptionsV1 {
                    include_fairness_actuals: true,
                    ..RecipientReportOptionsV1::default()
                },
                &control()
            ),
            Err(DomainPackError::ResourceLimitExceeded)
        ));

        let inherited = OperationControl::Cancellation(CancellationToken::new());
        let input = AssignmentInput::new(
            &document,
            &mut OperationBudget::evaluation(Some(&inherited)),
        )?;
        let mut budget = ReportBudget::new(&inherited);
        budget.scratch.cancel_after_steps(500)?;
        let mut groups = Vec::new();
        assert!(matches!(
            actuals(
                &document,
                &input,
                &BTreeMap::from([(pair(1, 8)?, "native-8")]),
                &mut groups,
                &mut BTreeSet::new(),
                &mut BTreeSet::new(),
                &mut budget
            ),
            Err(DomainPackError::Cancelled)
        ));
        assert!(groups.is_empty());
        Ok(())
    }

    #[test]
    fn actual_groups_cumulatively_charge_rows_and_union_intervals() -> TestResult {
        let mut document = source()?;
        for preference in [30, 31] {
            document.domain.preferences.insert(
                id(preference).parse()?,
                json!({
                    "kind":"workloadBalance","id":id(preference),"active":true,"priority":"normal",
                    "weight":1,"scope":{"people":{"kind":"all"}},"workloadPolicyId":id(10)
                }),
            );
        }
        let control = control();
        let input =
            AssignmentInput::new(&document, &mut OperationBudget::evaluation(Some(&control)))?;
        let selected = BTreeMap::from([(pair(1, 8)?, "native-8")]);
        let mut measured = ReportBudget::new(&control);
        actuals(
            &document,
            &input,
            &selected,
            &mut Vec::new(),
            &mut BTreeSet::new(),
            &mut BTreeSet::new(),
            &mut measured,
        )?;
        assert_eq!(measured.unions, 2);
        for row_limit in [true, false] {
            let mut budget = ReportBudget::new(&control);
            if row_limit {
                budget.rows = MAX_RECIPIENT_REPORT_ROWS - measured.rows / 2;
            } else {
                budget.unions = 1_000_000 - 1;
            }
            let mut groups = Vec::new();
            // The first complete native context fits; the second must refuse through the real
            // period/actual/index consumer, rather than exercising the counters in isolation.
            assert!(matches!(
                actuals(
                    &document,
                    &input,
                    &selected,
                    &mut groups,
                    &mut BTreeSet::new(),
                    &mut BTreeSet::new(),
                    &mut budget
                ),
                Err(DomainPackError::ResourceLimitExceeded)
            ));
            assert_eq!(groups.len(), 1);
        }
        Ok(())
    }

    #[test]
    fn escaped_strings_and_cancelled_source_are_refused() -> TestResult {
        let control = control();
        let mut budget = ReportBudget::new(&control);
        assert!(
            budget
                .text(&"\u{1}".repeat(MAX_RECIPIENT_REPORT_BYTES / 6 + 1))
                .is_err()
        );
        let cancellation = CancellationToken::new();
        let cancelled = OperationControl::Cancellation(cancellation.clone());
        let document = source()?;
        let accepted = accepted(&document, None, &[(1, 8, true)])?;
        cancellation.cancel();
        assert!(matches!(
            build_workforce_recipient_report(
                &document,
                &accepted,
                None,
                None,
                None,
                options(),
                &cancelled
            ),
            Err(DomainPackError::Cancelled)
        ));
        Ok(())
    }
}
