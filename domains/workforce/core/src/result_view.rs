mod calendar;
pub mod recipient_report;

use crate::assignment_rules::{
    HealthDirection, HealthObservation, HealthObservationKind, HealthUnits,
    WORKFORCE_PROJECTION_VERSION, budget::OperationBudget, check_objective_context,
    decode_workforce_assignment, evaluate_assignment_rules_with_health, operation_error,
    validate_workforce_objective_snapshot,
};
use crate::ids::{LocationId, ShiftId, WorkCalendarId, WorkloadBucketId, WorkloadPolicyId};
use crate::model::{
    AssignmentPair, Coverage, DateRange, OverlappingContribution, Person, PersonSelection,
    PreferencePriority, Scope, WindowMembership, WorkforceDomainV1, WorkforceEntity,
    WorkforcePreference, WorkforceScorePolicy, WorkloadMeasurement, WorkloadPolicy,
    WorkloadTargetMode, planning_dates,
};
use crate::temporal::{
    CalendarWindow, ResolvedShift, ResolvedShiftOrigin, TemporalError,
    reporting_window_for_calendar, resolve_calendar_for_calendar, weekday,
};
use crate::validation::validate_document_controlled;
use eutheto_domain_api::{
    DomainPackError, DomainViewInput, DomainViewOutput, SetupContinuationV1, bounded_json_size,
};
use eutheto_domain_ir::{
    NormalizedSolution, RunBaselineBinding, RunBaselineOriginal, RunBaselineSelection,
    RunExternalBaselineBinding, RunObjectiveContext,
};
use eutheto_types::{
    EntityId, Horizon, IanaTimeZone, OperationControl, PersonId, ResolvedLocalTime,
    Rfc3339Timestamp, RuleId,
};
use jiff::{Span, civil::Date};
use serde::{Deserialize, Serialize, ser::SerializeSeq};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const RESULT_VIEW_ID: &str = "official.workforce.result.schedule";
const QUERY_BYTES: usize = 64 * 1024;
const MAX_PAGE_SIZE: usize = 100;
const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_DATA_BYTES: usize = 256 * 1024;
const MAX_CURSOR_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
enum BaseDiffChange {
    Added,
    Removed,
}
type BaseDiffEntry = (AssignmentPair, BaseDiffChange);

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ResultViewKind {
    #[default]
    People,
    Shifts,
    Assignments,
    Timeline,
    Decision,
    Fairness,
    Health,
    #[serde(rename = "baseDiff")]
    BaseDiff,
    #[serde(rename = "repairDiff")]
    RepairDiff,
    #[serde(rename = "calendarPeople")]
    CalendarPeople,
    #[serde(rename = "calendarShifts")]
    CalendarShifts,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResultViewParameters {
    #[serde(default)]
    kind: ResultViewKind,
    #[serde(default)]
    person_id: Option<PersonId>,
    #[serde(default)]
    shift_id: Option<ShiftId>,
    #[serde(default)]
    location_id: Option<LocationId>,
    #[serde(default)]
    start_date: Option<Date>,
    #[serde(default)]
    local_date: Option<Date>,
    #[serde(default = "default_page_size")]
    limit: usize,
}

fn default_page_size() -> usize {
    DEFAULT_PAGE_SIZE
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ResultPosition {
    Person {
        person_id: PersonId,
    },
    Shift {
        shift_id: ShiftId,
    },
    Assignment {
        person_id: PersonId,
        shift_id: ShiftId,
    },
    Timeline {
        shift_id: ShiftId,
        person_id: PersonId,
    },
    Fairness {
        policy_id: WorkloadPolicyId,
        preference_id: RuleId,
        period_start: Rfc3339Timestamp,
        person_id: PersonId,
    },
    Health {
        index: u32,
    },
    BaseDiff {
        person_id: PersonId,
        shift_id: ShiftId,
        change: BaseDiffChange,
    },
    RepairDiff {
        shift_id: ShiftId,
    },
    CalendarPerson {
        person_id: PersonId,
    },
    CalendarShift {
        shift_id: ShiftId,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultEnvelope<'a> {
    schema_version: u32,
    time_zone: &'a IanaTimeZone,
    result: ResultData<'a>,
}

// One response per request, never an array of variants; boxing adds an allocation solely for layout.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum ResultData<'a> {
    Decision {
        person_id: PersonId,
        shift_id: ShiftId,
        assignment_id: Option<String>,
        selected: Option<bool>,
    },
    People {
        total_items: u32,
        items: Vec<PersonRow>,
        continuation: Option<SetupContinuationV1>,
    },
    Shifts {
        total_items: u32,
        items: Vec<ShiftRow>,
        continuation: Option<SetupContinuationV1>,
    },
    Assignments {
        total_items: u32,
        items: Vec<AssignmentRow>,
        continuation: Option<SetupContinuationV1>,
    },
    Timeline {
        total_items: u32,
        items: Vec<AssignmentRow>,
        continuation: Option<SetupContinuationV1>,
    },
    Fairness {
        total_items: u32,
        items: Vec<FairnessRow>,
        continuation: Option<SetupContinuationV1>,
    },
    BaseDiff {
        base_schedule: Option<BaseScheduleMetadata>,
        total_items: u32,
        items: Vec<BaseDiffRow>,
        continuation: Option<SetupContinuationV1>,
    },
    RepairDiff {
        availability: RepairDiffAvailability,
        source: RepairDiffSource,
        baseline: Option<RepairDiffBaseline<'a>>,
        counts: Option<RepairDiffCounts>,
        total_items: Option<u32>,
        items: Vec<RepairDiffRow>,
        continuation: Option<SetupContinuationV1>,
    },
    Health {
        total_items: u32,
        items: Vec<HealthRow>,
        continuation: Option<SetupContinuationV1>,
    },
    CalendarPeople {
        planning_dates: DateRange,
        window: DateRange,
        total_items: u32,
        items: Vec<calendar::PersonCalendarRow>,
        continuation: Option<SetupContinuationV1>,
    },
    CalendarShifts {
        planning_dates: DateRange,
        window: DateRange,
        total_items: u32,
        items: Vec<calendar::ShiftCalendarRow>,
        continuation: Option<SetupContinuationV1>,
    },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultEnvelopeBorrowed<'a, T> {
    schema_version: u32,
    time_zone: &'a IanaTimeZone,
    result: ResultDataBorrowed<'a, T>,
}

#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum ResultDataBorrowed<'a, T> {
    People {
        total_items: u32,
        items: RowsRef<'a, T>,
        continuation: Option<SetupContinuationV1>,
    },
    Shifts {
        total_items: u32,
        items: RowsRef<'a, T>,
        continuation: Option<SetupContinuationV1>,
    },
    Assignments {
        total_items: u32,
        items: RowsRef<'a, T>,
        continuation: Option<SetupContinuationV1>,
    },
    Timeline {
        total_items: u32,
        items: RowsRef<'a, T>,
        continuation: Option<SetupContinuationV1>,
    },
    Fairness {
        total_items: u32,
        items: RowsRef<'a, T>,
        continuation: Option<SetupContinuationV1>,
    },
    BaseDiff {
        base_schedule: Option<BaseScheduleMetadata>,
        total_items: u32,
        items: RowsRef<'a, T>,
        continuation: Option<SetupContinuationV1>,
    },
    RepairDiff {
        availability: RepairDiffAvailability,
        source: &'a RepairDiffSource,
        baseline: Option<&'a RepairDiffBaseline<'a>>,
        counts: Option<RepairDiffCounts>,
        total_items: Option<u32>,
        items: RowsRef<'a, T>,
        continuation: Option<SetupContinuationV1>,
    },
    Health {
        total_items: u32,
        items: RowsRef<'a, T>,
        continuation: Option<SetupContinuationV1>,
    },
    CalendarPeople {
        planning_dates: DateRange,
        window: DateRange,
        total_items: u32,
        items: RowsRef<'a, T>,
        continuation: Option<SetupContinuationV1>,
    },
    CalendarShifts {
        planning_dates: DateRange,
        window: DateRange,
        total_items: u32,
        items: RowsRef<'a, T>,
        continuation: Option<SetupContinuationV1>,
    },
}
#[derive(Clone, Copy)]
enum PageKind<'a> {
    People,
    Shifts,
    Assignments,
    Timeline,
    Fairness,
    BaseDiff {
        base_schedule: Option<BaseScheduleMetadata>,
    },
    RepairDiff {
        source: &'a RepairDiffSource,
        baseline: &'a RepairDiffBaseline<'a>,
        counts: RepairDiffCounts,
        control: &'a OperationControl,
    },
    Health,
    CalendarPeople {
        planning_dates: DateRange,
        window: DateRange,
    },
    CalendarShifts {
        planning_dates: DateRange,
        window: DateRange,
    },
}

struct RowsRef<'a, T>(&'a [(T, ResultPosition)]);

impl<T: Serialize> Serialize for RowsRef<'_, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for (row, _) in self.0 {
            sequence.serialize_element(row)?;
        }
        sequence.end()
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BaseScheduleMetadata {
    id: EntityId,
    source_solution_id: eutheto_types::SolutionId,
    source_revision: eutheto_types::Revision,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BaseDiffRow {
    change: BaseDiffChange,
    person_id: PersonId,
    shift_id: ShiftId,
    person_name: String,
    shift_label: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum RepairDiffAvailability {
    Available,
    Absent,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepairDiffSource {
    scenario_id: eutheto_types::ScenarioId,
    source_revision: eutheto_types::Revision,
    solution_id: eutheto_types::SolutionId,
    normalized_solution_hash: String,
    source_document_hash: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum RepairBaselineVerificationStatus {
    NotEstablished,
}

// Frozen descriptive provenance is not an independent acceptance of either selected source.
#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum RepairDiffBaseline<'a> {
    Accepted {
        original: &'a RunBaselineOriginal,
        selection: &'a RunBaselineSelection,
        selected_pair_content_hash: &'a str,
        verification_status: RepairBaselineVerificationStatus,
    },
    External {
        binding: &'a RunExternalBaselineBinding,
        verification_status: RepairBaselineVerificationStatus,
    },
}

// Wire counts name the pair unit explicitly, distinct from people or grouped work rows.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepairDiffCounts {
    removed_pairs: u32,
    added_pairs: u32,
    unchanged_pairs: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum RepairDiffChange {
    Unchanged,
    Replacement,
    NewlyUnfilled,
    NewlyAssigned,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepairDiffMember {
    person_id: PersonId,
    person_name: Option<String>,
    missing_person: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepairDiffRow {
    change: RepairDiffChange,
    shift_id: ShiftId,
    shift_label: Option<String>,
    missing_shift: bool,
    old_members: Vec<RepairDiffMember>,
    before_member_count: u32,
    after_member_count: u32,
    new_members: Vec<RepairDiffMember>,
    unchanged_participants: Vec<RepairDiffMember>,
    removed_pairs: u32,
    added_pairs: u32,
}

#[derive(Clone, Copy, Default)]
struct RepairMembership {
    old: bool,
    new: bool,
}

type RepairDiffIndex = BTreeMap<ShiftId, BTreeMap<PersonId, RepairMembership>>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersonRow {
    person_id: PersonId,
    name: String,
    selected_count: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShiftRow {
    shift_id: ShiftId,
    assignment_type_id: crate::ids::AssignmentTypeId,
    assignment_type_name: Option<String>,
    location_id: Option<LocationId>,
    location_name: Option<String>,
    starts_at: ResolvedLocalTime,
    ends_at: ResolvedLocalTime,
    required: Option<u32>,
    minimum: u32,
    preferred: Option<u32>,
    maximum: Option<u32>,
    selected_count: u32,
    margin: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssignmentRow {
    assignment_id: String,
    person_id: PersonId,
    person_name: String,
    shift_id: ShiftId,
    assignment_type_id: crate::ids::AssignmentTypeId,
    assignment_type_name: Option<String>,
    location_id: Option<LocationId>,
    location_name: Option<String>,
    starts_at: ResolvedLocalTime,
    ends_at: ResolvedLocalTime,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExactResultValue {
    numerator: String,
    denominator: String,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum HealthCheckKind {
    MinimumRest,
    MaximumHours,
    TransitionTime,
    CoverageHeadcount,
    CoverageQualification,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum HealthStatus {
    Satisfied,
    Violated,
    NoApplicable,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum HealthResultUnits {
    Minutes,
    People,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum CoverageOwnerKind {
    ShiftTemplate,
    ShiftInstance,
    CoverageRequirement,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthRow {
    check: HealthCheckKind,
    status: HealthStatus,
    units: HealthResultUnits,
    direction: HealthDirection,
    source_rule_id: Option<RuleId>,
    coverage_owner_id: Option<EntityId>,
    coverage_owner_kind: Option<CoverageOwnerKind>,
    person_id: Option<PersonId>,
    shift_id: Option<ShiftId>,
    other_shift_id: Option<ShiftId>,
    window_starts_at: Option<Rfc3339Timestamp>,
    window_ends_at: Option<Rfc3339Timestamp>,
    minimum_rank: Option<u32>,
    threshold: Option<ExactResultValue>,
    observed: Option<ExactResultValue>,
    margin: Option<ExactResultValue>,
    affected_assignment_ids: Vec<String>,
    affected_assignment_count: u32,
    affected_assignments_truncated: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum FairnessTargetMode {
    Explicit,
    PersonTargets,
    WeightedEqualShare,
    EqualShare,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum FairnessUnits {
    Assignments,
    Minutes,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FairnessRow {
    score_policy_id: EntityId,
    policy_id: WorkloadPolicyId,
    preference_id: RuleId,
    bucket_id: WorkloadBucketId,
    bucket_name: String,
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    target_mode: FairnessTargetMode,
    measure: WorkloadMeasurement,
    units: FairnessUnits,
    preference_priority: PreferencePriority,
    preference_weight: u32,
    period_starts_at: ResolvedLocalTime,
    period_ends_at: ResolvedLocalTime,
    peer_group_size: u32,
    person_id: PersonId,
    person_name: String,
    target: ExactResultValue,
    actual: ExactResultValue,
    deviation: ExactResultValue,
    group_actual_minimum: ExactResultValue,
    group_actual_maximum: ExactResultValue,
    group_spread: ExactResultValue,
    group_deviation_minimum: ExactResultValue,
    group_deviation_maximum: ExactResultValue,
    group_deviation_spread: ExactResultValue,
}

#[derive(Clone, Copy, Debug)]
struct ShiftMetadata<'a> {
    assignment_type_id: crate::ids::AssignmentTypeId,
    location_id: Option<LocationId>,
    coverage: &'a Coverage,
}
type SelectedAssignments<'solution> = (
    BTreeMap<AssignmentPair, &'solution str>,
    BTreeMap<ShiftId, u32>,
    BTreeMap<PersonId, u32>,
);

// Keep query validation, snapshot preparation, and page dispatch together so every page
// shares the same bounded source and cancellation checks.
#[allow(clippy::too_many_lines)]
pub(crate) fn build_view(
    input: DomainViewInput<'_>,
    control: &OperationControl,
) -> Result<DomainViewOutput, DomainPackError> {
    let DomainViewInput::AcceptedSolution {
        document,
        solution,
        objective_context,
        view_id,
        query,
        context,
    } = input
    else {
        return Err(invalid("/source", "accepted solution is required"));
    };
    control.check()?;
    check_objective_context(objective_context, document.scenario_id)?;
    bounded_json_size(solution, eutheto_domain_ir::MAX_DOMAIN_CONTRACT_JSON_BYTES)
        .map_err(|_| invalid("/solution", "solution exceeds its byte bound"))?;
    solution
        .validate()
        .map_err(|_| invalid("/solution", "invalid normalized solution"))?;
    if solution.pack_id.as_str() != crate::generated_workforce_pack_contract::WORKFORCE_PACK_ID
        || solution.scenario_id != document.scenario_id
        || solution.scenario_revision != context.revision.value()
        || solution.projection_version != WORKFORCE_PROJECTION_VERSION
    {
        return Err(invalid(
            "/solution",
            "solution source binding does not match",
        ));
    }
    if view_id != RESULT_VIEW_ID || query.view_id != RESULT_VIEW_ID {
        return Err(invalid("/viewId", "unknown Workforce result view"));
    }
    bounded_json_size(query, QUERY_BYTES)
        .map_err(|_| invalid("/query", "result view query exceeds its byte bound"))?;
    if query.schema_version != 1 {
        return Err(invalid(
            "/query/schemaVersion",
            "unsupported result view query version",
        ));
    }
    let parameters: ResultViewParameters = serde_json::from_value(query.parameters.clone())
        .map_err(|_| invalid("/query/parameters", "invalid result view parameters"))?;
    if !(1..=MAX_PAGE_SIZE).contains(&parameters.limit) {
        return Err(invalid(
            "/query/parameters/limit",
            "result view page limit must be between 1 and 100",
        ));
    }
    if (parameters.person_id.is_some()
        && !matches!(
            parameters.kind,
            ResultViewKind::Assignments | ResultViewKind::Timeline | ResultViewKind::Decision
        ))
        || (parameters.shift_id.is_some()
            && !matches!(
                parameters.kind,
                ResultViewKind::Assignments | ResultViewKind::Decision
            ))
    {
        return Err(invalid(
            "/query/parameters",
            "personId and shiftId are only valid for their source assignment views",
        ));
    }
    if parameters.location_id.is_some()
        && !matches!(
            parameters.kind,
            ResultViewKind::Shifts | ResultViewKind::Timeline | ResultViewKind::CalendarShifts
        )
    {
        return Err(invalid(
            "/query/parameters/locationId",
            "locationId filter is only valid for shift, timeline, or shift calendar pages",
        ));
    }
    if parameters.kind == ResultViewKind::Assignments
        && parameters.person_id.is_some()
        && parameters.shift_id.is_some()
    {
        return Err(invalid(
            "/query/parameters",
            "assignment pages accept at most one source filter",
        ));
    }

    if parameters.start_date.is_some()
        && !matches!(
            parameters.kind,
            ResultViewKind::CalendarPeople | ResultViewKind::CalendarShifts
        )
    {
        return Err(invalid(
            "/query/parameters/startDate",
            "startDate is only valid for calendar pages",
        ));
    }
    if parameters.local_date.is_some() && parameters.kind != ResultViewKind::Assignments {
        return Err(invalid(
            "/query/parameters/localDate",
            "localDate is only valid for assignment pages",
        ));
    }
    let cursor = parse_cursor(query.continuation.as_ref(), document, context)?;
    if parameters.kind == ResultViewKind::Decision
        && (parameters.person_id.is_none() || parameters.shift_id.is_none() || cursor.is_some())
    {
        return Err(invalid(
            "/query/parameters",
            "a decision requires a person and shift with no continuation",
        ));
    }
    let domain = validate_document_controlled(document, Some(control))?;
    let mut objective_budget = OperationBudget::evaluation(Some(control));
    let captured_original = objective_context
        .and_then(|context| context.baseline.as_deref())
        .map(|binding| {
            crate::assignment_rules::baseline::decode_captured_original(
                binding,
                document.scenario_id,
                &mut objective_budget,
            )
        })
        .transpose()?
        .flatten();
    if parameters.kind == ResultViewKind::RepairDiff {
        let data = build_repair_diff_page(
            document,
            &domain,
            captured_original.as_ref(),
            solution,
            objective_context,
            cursor.as_ref(),
            parameters.limit,
            context,
            &mut objective_budget,
        )?;
        let bytes = objective_budget
            .measure(&data)
            .map_err(|error| operation_error(&error))?;
        objective_budget
            .reserve(0, 0, bytes)
            .map_err(|error| operation_error(&error))?;
        control.check()?;
        let data = serde_json::to_value(data)
            .map_err(|_| invalid("/view/data", "view encoding failed"))?;
        bounded_json_size(&data, MAX_DATA_BYTES)
            .map_err(|_| DomainPackError::ResourceLimitExceeded)?;
        control.check()?;
        return Ok(DomainViewOutput {
            view: eutheto_domain_api::DomainView {
                view_id: RESULT_VIEW_ID.to_owned(),
                data,
            },
            reconciliation: None,
        });
    }
    validate_workforce_objective_snapshot(
        &domain,
        captured_original.as_ref(),
        document.scenario_id,
        objective_context,
        &mut objective_budget,
    )?;
    let mut resolved =
        crate::temporal::resolve_validated_shifts(&domain, &document.settings, &mut |_| {
            control
                .check()
                .map_err(|reason| TemporalError::from(DomainPackError::from(reason)))
        })
        .map_err(temporal_error)?;
    control.check()?;
    resolved.sort_unstable_by_key(|shift| (shift.interval.starts_at.instant, shift.id));

    let people = people_by_id(&domain);
    let locations = locations_by_id(&domain);
    if parameters
        .location_id
        .is_some_and(|location_id| !locations.contains_key(&location_id))
    {
        return Err(invalid(
            "/query/parameters/locationId",
            "location filter is not in source",
        ));
    }
    let assignment_types = assignment_types_by_id(&domain);
    let shifts = shift_metadata(&domain, &resolved)?;
    let (selected, selected_by_shift, selected_by_person) =
        selected_assignments(solution, &people, &shifts, control)?;
    let health_observations = if parameters.kind == ResultViewKind::Health {
        let selected_pairs = selected.keys().copied().collect::<Vec<_>>();
        let mut budget = OperationBudget::evaluation(Some(control));
        let (_, mut observations) =
            evaluate_assignment_rules_with_health(document, &selected_pairs, &mut budget)
                .map_err(|error| operation_error(&error))?;
        append_health_no_applicable(&domain, &mut observations, &mut budget)?;
        Some(observations)
    } else {
        None
    };
    let data = match parameters.kind {
        ResultViewKind::RepairDiff => {
            return Err(invalid("/view/data", "repair diff dispatch is unavailable"));
        }
        ResultViewKind::Decision => {
            let person_id = parameters
                .person_id
                .ok_or_else(|| invalid("/query/parameters/personId", "missing person"))?;
            let shift_id = parameters
                .shift_id
                .ok_or_else(|| invalid("/query/parameters/shiftId", "missing shift"))?;
            if !people.contains_key(&person_id) || !shifts.contains_key(&shift_id) {
                return Err(invalid(
                    "/query/parameters",
                    "person or shift is not in the original source",
                ));
            }
            let mut decision = None;
            for assignment in &solution.assignments {
                control.check()?;
                let (pair, selected) = decode_workforce_assignment(assignment)?;
                if pair.person_id == person_id && pair.shift_id == shift_id {
                    decision = Some((assignment.id.as_str().to_owned(), selected));
                    break;
                }
            }
            let (assignment_id, selected) = match decision {
                Some((assignment_id, selected)) => (Some(assignment_id), Some(selected)),
                None => (None, None),
            };
            ResultEnvelope {
                schema_version: 1,
                time_zone: &document.settings.time_zone,
                result: ResultData::Decision {
                    person_id,
                    shift_id,
                    assignment_id,
                    selected,
                },
            }
        }
        ResultViewKind::People => build_people_page(
            &people,
            &selected_by_person,
            cursor.as_ref(),
            parameters.limit,
            document,
            context,
            control,
        )?,
        ResultViewKind::Shifts => build_shift_page(
            &resolved,
            &shifts,
            &assignment_types,
            &locations,
            &selected_by_shift,
            parameters.location_id,
            cursor.as_ref(),
            parameters.limit,
            document,
            context,
            control,
        )?,
        ResultViewKind::Assignments => build_assignment_page(
            &selected,
            &resolved,
            &shifts,
            &assignment_types,
            &locations,
            &people,
            parameters.person_id,
            parameters.shift_id,
            parameters.local_date,
            cursor.as_ref(),
            parameters.limit,
            document,
            context,
            control,
        )?,
        ResultViewKind::Timeline => {
            let mut selected_by_shift_person: BTreeMap<ShiftId, BTreeMap<PersonId, &str>> =
                BTreeMap::new();
            for (pair, assignment_id) in &selected {
                control.check()?;
                selected_by_shift_person
                    .entry(pair.shift_id)
                    .or_default()
                    .insert(pair.person_id, *assignment_id);
            }
            build_timeline_page(
                &selected_by_shift_person,
                &resolved,
                &shifts,
                &assignment_types,
                &locations,
                &people,
                parameters.person_id,
                parameters.location_id,
                cursor.as_ref(),
                parameters.limit,
                document,
                context,
                control,
            )?
        }
        ResultViewKind::Fairness => build_fairness_page(
            document,
            &domain,
            &people,
            &assignment_types,
            &resolved,
            &shifts,
            &selected,
            cursor.as_ref(),
            parameters.limit,
            context,
            control,
        )?,
        ResultViewKind::Health => build_health_page(
            health_observations
                .as_deref()
                .ok_or_else(|| invalid("/view/data", "health observations are unavailable"))?,
            &selected,
            cursor.as_ref(),
            parameters.limit,
            document,
            context,
            control,
        )?,
        ResultViewKind::BaseDiff => build_base_diff_page(
            document,
            &domain,
            &people,
            &selected,
            cursor.as_ref(),
            parameters.limit,
            context,
            control,
        )?,
        ResultViewKind::CalendarPeople => calendar::build_people_calendar(
            &people,
            &selected_by_person,
            &selected,
            &resolved,
            parameters.start_date,
            cursor.as_ref(),
            parameters.limit,
            document,
            context,
            control,
        )?,
        ResultViewKind::CalendarShifts => calendar::build_shift_calendar(
            &resolved,
            &shifts,
            &assignment_types,
            &locations,
            &selected_by_shift,
            &selected,
            parameters.location_id,
            parameters.start_date,
            cursor.as_ref(),
            parameters.limit,
            document,
            context,
            control,
        )?,
    };
    let data =
        serde_json::to_value(data).map_err(|_| invalid("/view/data", "view encoding failed"))?;
    bounded_json_size(&data, MAX_DATA_BYTES).map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    Ok(DomainViewOutput {
        view: eutheto_domain_api::DomainView {
            view_id: RESULT_VIEW_ID.to_owned(),
            data,
        },
        reconciliation: None,
    })
}

fn people_by_id(domain: &WorkforceDomainV1) -> BTreeMap<PersonId, &Person> {
    domain
        .entities
        .values()
        .filter_map(|entity| match entity {
            WorkforceEntity::Person(person) => Some((person.id, person)),
            _ => None,
        })
        .collect()
}

fn locations_by_id(domain: &WorkforceDomainV1) -> BTreeMap<LocationId, &crate::model::Location> {
    domain
        .entities
        .values()
        .filter_map(|entity| match entity {
            WorkforceEntity::Location(location) => Some((location.id, location)),
            _ => None,
        })
        .collect()
}

fn assignment_types_by_id(
    domain: &WorkforceDomainV1,
) -> BTreeMap<crate::ids::AssignmentTypeId, &crate::model::AssignmentType> {
    domain
        .entities
        .values()
        .filter_map(|entity| match entity {
            WorkforceEntity::AssignmentType(assignment_type) => {
                Some((assignment_type.id, assignment_type))
            }
            _ => None,
        })
        .collect()
}

fn shift_metadata<'a>(
    domain: &'a WorkforceDomainV1,
    shifts: &[ResolvedShift],
) -> Result<BTreeMap<ShiftId, ShiftMetadata<'a>>, DomainPackError> {
    shifts
        .iter()
        .map(|shift| {
            let metadata = if let Some(WorkforceEntity::ShiftInstance(instance)) =
                domain.entities.get(&shift.id.as_entity_id())
            {
                ShiftMetadata {
                    assignment_type_id: instance.assignment_type_id,
                    location_id: instance.location_id,
                    coverage: &instance.coverage,
                }
            } else {
                let template_id = match shift.origin {
                    ResolvedShiftOrigin::Generated { template_id, .. }
                    | ResolvedShiftOrigin::Detached { template_id, .. } => template_id,
                    ResolvedShiftOrigin::Manual => {
                        return Err(invalid("/domain", "resolved shift source is missing"));
                    }
                };
                let Some(WorkforceEntity::ShiftTemplate(template)) =
                    domain.entities.get(&template_id.as_entity_id())
                else {
                    return Err(invalid("/domain", "resolved shift template is missing"));
                };
                ShiftMetadata {
                    assignment_type_id: template.assignment_type_id,
                    location_id: template.location_id,
                    coverage: &template.coverage,
                }
            };
            Ok((shift.id, metadata))
        })
        .collect()
}

fn selected_assignments<'solution>(
    solution: &'solution NormalizedSolution,
    people: &BTreeMap<PersonId, &Person>,
    shifts: &BTreeMap<ShiftId, ShiftMetadata<'_>>,
    control: &OperationControl,
) -> Result<SelectedAssignments<'solution>, DomainPackError> {
    let mut selected = BTreeMap::new();
    let mut selected_by_shift: BTreeMap<ShiftId, u32> = BTreeMap::new();
    let mut selected_by_person: BTreeMap<PersonId, u32> = BTreeMap::new();
    for assignment in &solution.assignments {
        control.check()?;
        let (pair, is_selected) = decode_workforce_assignment(assignment)?;
        if !people.contains_key(&pair.person_id) || !shifts.contains_key(&pair.shift_id) {
            return Err(invalid(
                "/solution/assignments",
                "accepted assignment source is missing",
            ));
        }
        if is_selected {
            if selected.insert(pair, assignment.id.as_str()).is_some() {
                return Err(invalid(
                    "/solution/assignments",
                    "accepted assignment source is duplicated",
                ));
            }
            let shift_count = selected_by_shift.entry(pair.shift_id).or_insert(0);
            *shift_count = shift_count
                .checked_add(1)
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            let person_count = selected_by_person.entry(pair.person_id).or_insert(0);
            *person_count = person_count
                .checked_add(1)
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
        }
    }
    Ok((selected, selected_by_shift, selected_by_person))
}

fn parse_cursor(
    continuation: Option<&SetupContinuationV1>,
    document: &eutheto_types::ScenarioDocument,
    context: eutheto_domain_api::SetupViewContext,
) -> Result<Option<ResultPosition>, DomainPackError> {
    let Some(continuation) = continuation else {
        return Ok(None);
    };
    if continuation.schema_version != 1 {
        return Err(invalid(
            "/query/continuation/schemaVersion",
            "unsupported cursor version",
        ));
    }
    if continuation.scenario_id != document.scenario_id {
        return Err(invalid(
            "/query/continuation/scenarioId",
            "cursor scenario does not match",
        ));
    }
    if continuation.revision != context.revision {
        return Err(invalid(
            "/query/continuation/revision",
            "cursor source revision does not match",
        ));
    }
    if continuation.query_fingerprint != context.query_fingerprint {
        return Err(invalid(
            "/query/continuation/queryFingerprint",
            "cursor query does not match",
        ));
    }
    bounded_json_size(&continuation.position, MAX_CURSOR_BYTES).map_err(|_| {
        invalid(
            "/query/continuation/position",
            "cursor position is too large",
        )
    })?;
    serde_json::from_value(continuation.position.clone())
        .map(Some)
        .map_err(|_| invalid("/query/continuation/position", "invalid cursor position"))
}

// The selected baseline is resolved only from C's immutable snapshot or explicit run-only capture.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_repair_diff_page<'doc>(
    document: &'doc eutheto_types::ScenarioDocument,
    domain: &WorkforceDomainV1,
    captured_original: Option<&crate::model::BaseSchedule>,
    solution: &NormalizedSolution,
    objective_context: Option<&'doc RunObjectiveContext>,
    cursor: Option<&ResultPosition>,
    limit: usize,
    context: eutheto_domain_api::SetupViewContext,
    budget: &mut OperationBudget<'_>,
) -> Result<ResultEnvelope<'doc>, DomainPackError> {
    let source = RepairDiffSource {
        scenario_id: document.scenario_id,
        source_revision: context.revision,
        solution_id: solution.solution_id,
        normalized_solution_hash: repair_source_hash(solution, budget)?,
        source_document_hash: repair_source_hash(document, budget)?,
    };
    let Some(objective) = objective_context.filter(|context| context.has_baseline()) else {
        if cursor.is_some() {
            return Err(invalid(
                "/query/continuation/position",
                "repair diff has no recorded baseline",
            ));
        }
        return Ok(ResultEnvelope {
            schema_version: 1,
            time_zone: &document.settings.time_zone,
            result: ResultData::RepairDiff {
                availability: RepairDiffAvailability::Absent,
                source,
                baseline: None,
                counts: None,
                total_items: None,
                items: Vec::new(),
                continuation: None,
            },
        });
    };
    let baseline_pairs = crate::assignment_rules::baseline::selected_baseline_pairs(
        domain,
        captured_original,
        document.scenario_id,
        Some(objective),
        budget,
    )?
    .ok_or_else(|| invalid("/view", "missing recorded baseline"))?;
    let index = repair_diff_index(baseline_pairs, solution, budget)?;
    let cursor_shift = match cursor {
        None => None,
        Some(ResultPosition::RepairDiff { shift_id }) if index.contains_key(shift_id) => {
            Some(*shift_id)
        }
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor shift is not in the recorded repair diff",
            ));
        }
    };

    // Charge the complete source-label census before retaining these borrowed indexes.
    for entity in domain.entities.values() {
        budget.step().map_err(|error| operation_error(&error))?;
        match entity {
            WorkforceEntity::Person(_) | WorkforceEntity::ShiftInstance(_) => {
                budget
                    .reserve(1, 1, 64)
                    .map_err(|error| operation_error(&error))?;
            }
            WorkforceEntity::ShiftTemplate(template) => {
                for _ in template.occurrence_identities.keys() {
                    budget.step().map_err(|error| operation_error(&error))?;
                    budget
                        .reserve(1, 1, 64)
                        .map_err(|error| operation_error(&error))?;
                }
            }
            _ => {}
        }
    }
    budget.check().map_err(|error| operation_error(&error))?;
    let people = people_by_id(domain);
    let labels = base_diff_shift_labels(
        domain,
        budget
            .control()
            .ok_or_else(|| invalid("/view", "missing operation control"))?,
    )?;
    let baseline = match (&objective.baseline, &objective.external_baseline) {
        (Some(binding), None) => repair_diff_baseline(binding, budget)?,
        (None, Some(binding)) => {
            let bytes = budget
                .measure(binding.as_ref())
                .map_err(|error| operation_error(&error))?;
            budget
                .reserve(1, 0, bytes)
                .map_err(|error| operation_error(&error))?;
            RepairDiffBaseline::External {
                binding: binding.as_ref(),
                verification_status: RepairBaselineVerificationStatus::NotEstablished,
            }
        }
        _ => return Err(invalid("/view", "invalid recorded baseline source")),
    };
    let total = u32::try_from(index.len()).map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    let mut counts = RepairDiffCounts::default();
    let mut rows = Vec::new();
    let mut has_more = false;
    for (shift_id, members) in &index {
        budget.step().map_err(|error| operation_error(&error))?;
        let mut row_counts = RepairDiffCounts::default();
        let mut before_member_count = 0;
        let mut after_member_count = 0;
        for member in members.values() {
            budget.step().map_err(|error| operation_error(&error))?;
            before_member_count += u32::from(member.old);
            after_member_count += u32::from(member.new);
            match (member.old, member.new) {
                (true, true) => row_counts.unchanged_pairs += 1,
                (true, false) => row_counts.removed_pairs += 1,
                (false, true) => row_counts.added_pairs += 1,
                (false, false) => {}
            }
        }
        counts.removed_pairs += row_counts.removed_pairs;
        counts.added_pairs += row_counts.added_pairs;
        counts.unchanged_pairs += row_counts.unchanged_pairs;
        if cursor_shift.is_some_and(|cursor_shift| *shift_id <= cursor_shift) {
            continue;
        }
        if rows.len() == limit {
            has_more = true;
            continue;
        }
        let label = labels.get(shift_id).copied();
        budget
            .reserve(1, 0, 128 + label.map_or(0, |value| value.len() as u64))
            .map_err(|error| operation_error(&error))?;
        let mut row = RepairDiffRow {
            change: match (row_counts.removed_pairs > 0, row_counts.added_pairs > 0) {
                (false, false) => RepairDiffChange::Unchanged,
                (true, true) => RepairDiffChange::Replacement,
                (true, false) => RepairDiffChange::NewlyUnfilled,
                (false, true) => RepairDiffChange::NewlyAssigned,
            },
            shift_id: *shift_id,
            shift_label: label.map(str::to_owned),
            missing_shift: label.is_none(),
            before_member_count,
            after_member_count,
            old_members: Vec::new(),
            new_members: Vec::new(),
            unchanged_participants: Vec::new(),
            removed_pairs: row_counts.removed_pairs,
            added_pairs: row_counts.added_pairs,
        };
        for (person_id, membership) in members {
            budget.step().map_err(|error| operation_error(&error))?;
            if membership.old {
                row.old_members
                    .push(repair_diff_member(*person_id, &people, budget)?);
            }
            if membership.new {
                row.new_members
                    .push(repair_diff_member(*person_id, &people, budget)?);
            }
            if membership.old && membership.new {
                row.unchanged_participants
                    .push(repair_diff_member(*person_id, &people, budget)?);
            }
        }
        rows.push((
            row,
            ResultPosition::RepairDiff {
                shift_id: *shift_id,
            },
        ));
    }
    budget.check().map_err(|error| operation_error(&error))?;
    let (items, continuation) = fit_page(
        rows,
        total,
        has_more,
        document.scenario_id,
        context,
        &document.settings.time_zone,
        PageKind::RepairDiff {
            source: &source,
            baseline: &baseline,
            counts,
            control: budget
                .control()
                .ok_or_else(|| invalid("/view", "missing operation control"))?,
        },
    )?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(ResultEnvelope {
        schema_version: 1,
        time_zone: &document.settings.time_zone,
        result: ResultData::RepairDiff {
            availability: RepairDiffAvailability::Available,
            source,
            baseline: Some(baseline),
            counts: Some(counts),
            total_items: Some(total),
            items,
            continuation,
        },
    })
}

fn repair_source_hash<T: Serialize>(
    value: &T,
    budget: &mut OperationBudget<'_>,
) -> Result<String, DomainPackError> {
    let bytes = budget
        .measure(value)
        .map_err(|error| operation_error(&error))?;
    budget
        .steps(bytes)
        .map_err(|error| operation_error(&error))?;
    budget
        .reserve(0, 0, 64)
        .map_err(|error| operation_error(&error))?;
    let mut hasher = blake3::Hasher::new();
    serde_json::to_writer(&mut hasher, value)
        .map_err(|_| invalid("/view/source", "source hashing failed"))?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn repair_diff_baseline<'a>(
    binding: &'a RunBaselineBinding,
    budget: &mut OperationBudget<'_>,
) -> Result<RepairDiffBaseline<'a>, DomainPackError> {
    let bytes = budget
        .measure(&(
            &binding.original,
            &binding.selection,
            &binding.selected_pair_content_hash,
        ))
        .map_err(|error| operation_error(&error))?;
    budget
        .reserve(1, 0, bytes)
        .map_err(|error| operation_error(&error))?;
    Ok(RepairDiffBaseline::Accepted {
        original: &binding.original,
        selection: &binding.selection,
        selected_pair_content_hash: &binding.selected_pair_content_hash,
        verification_status: RepairBaselineVerificationStatus::NotEstablished,
    })
}

fn repair_diff_index(
    baseline: &[AssignmentPair],
    solution: &NormalizedSolution,
    budget: &mut OperationBudget<'_>,
) -> Result<RepairDiffIndex, DomainPackError> {
    let mut index = BTreeMap::new();
    for pair in baseline {
        budget.step().map_err(|error| operation_error(&error))?;
        repair_diff_membership(&mut index, *pair, budget)?.old = true;
    }
    let mut selected_count = 0_u64;
    for assignment in &solution.assignments {
        budget.step().map_err(|error| operation_error(&error))?;
        let (pair, selected) = decode_workforce_assignment(assignment)?;
        if !selected {
            continue;
        }
        selected_count += 1;
        if selected_count > crate::assignment_rules::MAX_SELECTED_PAIRS {
            return Err(DomainPackError::ResourceLimitExceeded);
        }
        let membership = repair_diff_membership(&mut index, pair, budget)?;
        if membership.new {
            return Err(invalid(
                "/solution/assignments",
                "repair diff pair is duplicated",
            ));
        }
        membership.new = true;
    }
    Ok(index)
}

fn repair_diff_membership<'a>(
    index: &'a mut RepairDiffIndex,
    pair: AssignmentPair,
    budget: &mut OperationBudget<'_>,
) -> Result<&'a mut RepairMembership, DomainPackError> {
    budget
        .steps(3 * u64::from(usize::BITS - index.len().leading_zeros()))
        .map_err(|error| operation_error(&error))?;
    if !index.contains_key(&pair.shift_id) {
        budget
            .reserve(1, 1, 64)
            .map_err(|error| operation_error(&error))?;
    }
    let members = index.entry(pair.shift_id).or_default();
    budget
        .steps(2 * u64::from(usize::BITS - members.len().leading_zeros()))
        .map_err(|error| operation_error(&error))?;
    if !members.contains_key(&pair.person_id) {
        budget
            .reserve(0, 1, 64)
            .map_err(|error| operation_error(&error))?;
    }
    Ok(members.entry(pair.person_id).or_default())
}

fn repair_diff_member(
    person_id: PersonId,
    people: &BTreeMap<PersonId, &Person>,
    budget: &mut OperationBudget<'_>,
) -> Result<RepairDiffMember, DomainPackError> {
    let name = people.get(&person_id).map(|person| person.name.as_str());
    budget
        .reserve(0, 1, 64 + name.map_or(0, |value| value.len() as u64))
        .map_err(|error| operation_error(&error))?;
    Ok(RepairDiffMember {
        person_id,
        person_name: name.map(str::to_owned),
        missing_person: name.is_none(),
    })
}

#[allow(clippy::too_many_arguments)]
fn build_base_diff_page<'doc>(
    document: &'doc eutheto_types::ScenarioDocument,
    domain: &WorkforceDomainV1,
    people: &BTreeMap<PersonId, &Person>,
    selected: &BTreeMap<AssignmentPair, &str>,
    cursor: Option<&ResultPosition>,
    limit: usize,
    context: eutheto_domain_api::SetupViewContext,
    control: &OperationControl,
) -> Result<ResultEnvelope<'doc>, DomainPackError> {
    let base = domain.entities.values().find_map(|entity| match entity {
        WorkforceEntity::BaseSchedule(value) => Some(value),
        _ => None,
    });
    let Some(base) = base else {
        if cursor.is_some() {
            return Err(invalid(
                "/query/continuation/position",
                "base diff has no base schedule",
            ));
        }
        return Ok(ResultEnvelope {
            schema_version: 1,
            time_zone: &document.settings.time_zone,
            result: ResultData::BaseDiff {
                base_schedule: None,
                total_items: 0,
                items: Vec::new(),
                continuation: None,
            },
        });
    };

    let base_schedule = Some(BaseScheduleMetadata {
        id: base.id,
        source_solution_id: base.source_solution_id,
        source_revision: base.source_revision,
    });
    let (candidates, cursor_key) =
        base_diff_candidates(&base.assignments, selected, cursor, control)?;

    let shift_labels = base_diff_shift_labels(domain, control)?;
    let total =
        u32::try_from(candidates.len()).map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    let mut rows = Vec::new();
    let mut has_more = false;
    for (pair, change) in candidates {
        control.check()?;
        if cursor_key.is_some_and(|cursor_key| (pair, change) <= cursor_key) {
            continue;
        }
        if rows.len() < limit {
            let person = people
                .get(&pair.person_id)
                .ok_or_else(|| invalid("/domain", "base diff person source is missing"))?;
            let shift_label = *shift_labels
                .get(&pair.shift_id)
                .ok_or_else(|| invalid("/domain", "base diff shift source is missing"))?;
            rows.push((
                BaseDiffRow {
                    change,
                    person_id: pair.person_id,
                    shift_id: pair.shift_id,
                    person_name: person.name.clone(),
                    shift_label: shift_label.to_owned(),
                },
                ResultPosition::BaseDiff {
                    person_id: pair.person_id,
                    shift_id: pair.shift_id,
                    change,
                },
            ));
        } else {
            has_more = true;
        }
    }
    let (items, continuation) = fit_page(
        rows,
        total,
        has_more,
        document.scenario_id,
        context,
        &document.settings.time_zone,
        PageKind::BaseDiff { base_schedule },
    )?;
    Ok(ResultEnvelope {
        schema_version: 1,
        time_zone: &document.settings.time_zone,
        result: ResultData::BaseDiff {
            base_schedule,
            total_items: total,
            items,
            continuation,
        },
    })
}

fn base_diff_candidates(
    base_assignments: &[AssignmentPair],
    selected: &BTreeMap<AssignmentPair, &str>,
    cursor: Option<&ResultPosition>,
    control: &OperationControl,
) -> Result<(Vec<BaseDiffEntry>, Option<BaseDiffEntry>), DomainPackError> {
    let base_pairs = base_assignments.iter().copied().collect::<BTreeSet<_>>();
    let mut candidates = Vec::new();
    for pair in selected.keys() {
        control.check()?;
        if !base_pairs.contains(pair) {
            candidates.push((*pair, BaseDiffChange::Added));
        }
    }
    for pair in &base_pairs {
        control.check()?;
        if !selected.contains_key(pair) {
            candidates.push((*pair, BaseDiffChange::Removed));
        }
    }
    candidates.sort_unstable_by_key(|(pair, change)| (*pair, *change));

    let cursor_key = match cursor {
        None => None,
        Some(ResultPosition::BaseDiff {
            person_id,
            shift_id,
            change,
        }) => {
            let key = (
                AssignmentPair {
                    person_id: *person_id,
                    shift_id: *shift_id,
                },
                *change,
            );
            if candidates
                .binary_search_by_key(&key, |(pair, change)| (*pair, *change))
                .is_err()
            {
                return Err(invalid(
                    "/query/continuation/position",
                    "cursor change is not in the base diff",
                ));
            }
            Some(key)
        }
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor kind does not match base diff page",
            ));
        }
    };
    Ok((candidates, cursor_key))
}

// Generated occurrences inherit their authored template name; stored/manual shifts use
// their source assignment type name, matching the pack's other source-derived projections.
fn base_diff_shift_labels<'a>(
    domain: &'a WorkforceDomainV1,
    control: &OperationControl,
) -> Result<BTreeMap<ShiftId, &'a str>, DomainPackError> {
    let mut labels = BTreeMap::new();
    for entity in domain.entities.values() {
        control.check()?;
        match entity {
            WorkforceEntity::ShiftInstance(instance) => {
                let assignment_type = domain
                    .entities
                    .get(&instance.assignment_type_id.as_entity_id())
                    .and_then(|entity| match entity {
                        WorkforceEntity::AssignmentType(value) => Some(value),
                        _ => None,
                    })
                    .ok_or_else(|| invalid("/domain", "base diff assignment type is missing"))?;
                if labels
                    .insert(instance.id, assignment_type.name.as_str())
                    .is_some()
                {
                    return Err(invalid("/domain", "base diff shift identity is duplicated"));
                }
            }
            WorkforceEntity::ShiftTemplate(template) => {
                for shift_id in template.occurrence_identities.keys() {
                    control.check()?;
                    if labels.insert(*shift_id, template.name.as_str()).is_some() {
                        return Err(invalid("/domain", "base diff shift identity is duplicated"));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(labels)
}

fn build_people_page<'a>(
    people: &BTreeMap<PersonId, &Person>,
    selected_by_person: &BTreeMap<PersonId, u32>,
    cursor: Option<&ResultPosition>,
    limit: usize,
    document: &'a eutheto_types::ScenarioDocument,
    context: eutheto_domain_api::SetupViewContext,
    control: &OperationControl,
) -> Result<ResultEnvelope<'a>, DomainPackError> {
    let cursor_id = match cursor {
        None => None,
        Some(ResultPosition::Person { person_id }) => {
            if !people.contains_key(person_id) {
                return Err(invalid(
                    "/query/continuation/position",
                    "cursor person is not in source",
                ));
            }
            Some(*person_id)
        }
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor kind does not match page",
            ));
        }
    };
    let mut rows = Vec::new();
    let mut total = 0_u32;
    let mut has_more = false;
    for (person_id, person) in people {
        control.check()?;
        total = total
            .checked_add(1)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if cursor_id.is_some_and(|cursor_id| *person_id <= cursor_id) {
            continue;
        }
        if rows.len() < limit {
            rows.push((
                PersonRow {
                    person_id: *person_id,
                    name: person.name.clone(),
                    selected_count: selected_by_person.get(person_id).copied().unwrap_or(0),
                },
                ResultPosition::Person {
                    person_id: *person_id,
                },
            ));
        } else {
            has_more = true;
        }
    }
    let (items, continuation) = fit_page(
        rows,
        total,
        has_more,
        document.scenario_id,
        context,
        &document.settings.time_zone,
        PageKind::People,
    )?;
    Ok(ResultEnvelope {
        schema_version: 1,
        time_zone: &document.settings.time_zone,
        result: ResultData::People {
            total_items: total,
            items,
            continuation,
        },
    })
}

// Keep the independently sourced row dimensions explicit; a one-use context would add
// state without reducing the page's linear pagination flow.
#[allow(clippy::too_many_arguments)]
fn build_shift_page<'doc>(
    resolved: &[ResolvedShift],
    shifts: &BTreeMap<ShiftId, ShiftMetadata<'_>>,
    assignment_types: &BTreeMap<crate::ids::AssignmentTypeId, &crate::model::AssignmentType>,
    locations: &BTreeMap<LocationId, &crate::model::Location>,
    selected_by_shift: &BTreeMap<ShiftId, u32>,
    location_filter: Option<LocationId>,
    cursor: Option<&ResultPosition>,
    limit: usize,
    document: &'doc eutheto_types::ScenarioDocument,
    context: eutheto_domain_api::SetupViewContext,
    control: &OperationControl,
) -> Result<ResultEnvelope<'doc>, DomainPackError> {
    let cursor_key = match cursor {
        None => None,
        Some(ResultPosition::Shift { shift_id }) => {
            let shift = resolved
                .iter()
                .find(|shift| shift.id == *shift_id)
                .ok_or_else(|| {
                    invalid(
                        "/query/continuation/position",
                        "cursor shift is not in source",
                    )
                })?;
            let metadata = shifts
                .get(shift_id)
                .ok_or_else(|| invalid("/domain", "resolved shift metadata is missing"))?;
            if location_filter.is_some_and(|filter| metadata.location_id != Some(filter)) {
                return Err(invalid(
                    "/query/continuation/position",
                    "cursor does not match location filter",
                ));
            }
            Some((shift.interval.starts_at.instant, *shift_id))
        }
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor kind does not match page",
            ));
        }
    };
    let mut rows = Vec::new();
    let mut total = 0_u32;
    let mut has_more = false;
    for shift in resolved {
        control.check()?;
        let metadata = shifts
            .get(&shift.id)
            .ok_or_else(|| invalid("/domain", "resolved shift metadata is missing"))?;
        if location_filter.is_some_and(|filter| metadata.location_id != Some(filter)) {
            continue;
        }
        total = total
            .checked_add(1)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        let key = (shift.interval.starts_at.instant, shift.id);
        if cursor_key.is_some_and(|cursor_key| key <= cursor_key) {
            continue;
        }
        if rows.len() < limit {
            let selected_count = selected_by_shift.get(&shift.id).copied().unwrap_or(0);
            let (required, minimum, preferred, maximum, margin) =
                coverage_values(metadata.coverage, selected_count);
            rows.push((
                ShiftRow {
                    shift_id: shift.id,
                    assignment_type_id: metadata.assignment_type_id,
                    assignment_type_name: assignment_types
                        .get(&metadata.assignment_type_id)
                        .map(|value| value.name.clone()),
                    location_id: metadata.location_id,
                    location_name: metadata
                        .location_id
                        .and_then(|id| locations.get(&id).map(|value| value.name.clone())),
                    starts_at: shift.interval.starts_at,
                    ends_at: shift.interval.ends_at,
                    required,
                    minimum,
                    preferred,
                    maximum,
                    selected_count,
                    margin,
                },
                ResultPosition::Shift { shift_id: shift.id },
            ));
        } else {
            has_more = true;
        }
    }
    let (items, continuation) = fit_page(
        rows,
        total,
        has_more,
        document.scenario_id,
        context,
        &document.settings.time_zone,
        PageKind::Shifts,
    )?;
    Ok(ResultEnvelope {
        schema_version: 1,
        time_zone: &document.settings.time_zone,
        result: ResultData::Shifts {
            total_items: total,
            items,
            continuation,
        },
    })
}

// Keep source validation, filtering, and row construction in one auditable linear page pass.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_assignment_page<'doc>(
    selected: &BTreeMap<AssignmentPair, &str>,
    resolved: &[ResolvedShift],
    shifts: &BTreeMap<ShiftId, ShiftMetadata<'_>>,
    assignment_types: &BTreeMap<crate::ids::AssignmentTypeId, &crate::model::AssignmentType>,
    locations: &BTreeMap<LocationId, &crate::model::Location>,
    people: &BTreeMap<PersonId, &Person>,
    person_filter: Option<PersonId>,
    shift_filter: Option<ShiftId>,
    local_date: Option<Date>,
    cursor: Option<&ResultPosition>,
    limit: usize,
    document: &'doc eutheto_types::ScenarioDocument,
    context: eutheto_domain_api::SetupViewContext,
    control: &OperationControl,
) -> Result<ResultEnvelope<'doc>, DomainPackError> {
    let resolved_by_id: BTreeMap<ShiftId, &ResolvedShift> =
        resolved.iter().map(|shift| (shift.id, shift)).collect();
    if person_filter.is_some_and(|person_id| !people.contains_key(&person_id)) {
        return Err(invalid(
            "/query/parameters/personId",
            "person filter is not in source",
        ));
    }
    if shift_filter.is_some_and(|shift_id| !resolved_by_id.contains_key(&shift_id)) {
        return Err(invalid(
            "/query/parameters/shiftId",
            "shift filter is not in source",
        ));
    }
    if let Some(date) = local_date {
        let dates = planning_dates(&document.settings)?;
        if date < dates.first_date || date > dates.last_date {
            return Err(invalid(
                "/query/parameters/localDate",
                "localDate is outside the accepted source planning dates",
            ));
        }
    }
    let cursor_pair = match cursor {
        None => None,
        Some(ResultPosition::Assignment {
            person_id,
            shift_id,
        }) => {
            let pair = AssignmentPair {
                person_id: *person_id,
                shift_id: *shift_id,
            };
            if !selected.contains_key(&pair) {
                return Err(invalid(
                    "/query/continuation/position",
                    "cursor assignment is not selected source data",
                ));
            }
            if person_filter.is_some_and(|filter| filter != *person_id)
                || shift_filter.is_some_and(|filter| filter != *shift_id)
            {
                return Err(invalid(
                    "/query/continuation/position",
                    "cursor does not match assignment filter",
                ));
            }
            if local_date.is_some_and(|date| {
                resolved_by_id
                    .get(shift_id)
                    .is_none_or(|shift| shift.interval.starts_at.local.as_datetime().date() != date)
            }) {
                return Err(invalid(
                    "/query/continuation/position",
                    "cursor does not match local date filter",
                ));
            }
            Some(pair)
        }
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor kind does not match page",
            ));
        }
    };
    let mut rows = Vec::new();
    let mut total = 0_u32;
    let mut has_more = false;
    for (pair, assignment_id) in selected {
        control.check()?;
        if person_filter.is_some_and(|filter| filter != pair.person_id)
            || shift_filter.is_some_and(|filter| filter != pair.shift_id)
        {
            continue;
        }
        if let Some(date) = local_date {
            let shift = resolved_by_id
                .get(&pair.shift_id)
                .ok_or_else(|| invalid("/domain", "selected shift source is missing"))?;
            if shift.interval.starts_at.local.as_datetime().date() != date {
                continue;
            }
        }
        total = total
            .checked_add(1)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if cursor_pair.is_some_and(|cursor_pair| *pair <= cursor_pair) {
            continue;
        }
        if rows.len() < limit {
            let person = people
                .get(&pair.person_id)
                .ok_or_else(|| invalid("/domain", "selected person source is missing"))?;
            let shift = resolved_by_id
                .get(&pair.shift_id)
                .ok_or_else(|| invalid("/domain", "selected shift source is missing"))?;
            let metadata = shifts
                .get(&pair.shift_id)
                .ok_or_else(|| invalid("/domain", "selected shift metadata is missing"))?;
            rows.push((
                AssignmentRow {
                    assignment_id: (*assignment_id).to_owned(),
                    person_id: pair.person_id,
                    person_name: person.name.clone(),
                    shift_id: pair.shift_id,
                    assignment_type_id: metadata.assignment_type_id,
                    assignment_type_name: assignment_types
                        .get(&metadata.assignment_type_id)
                        .map(|value| value.name.clone()),
                    location_id: metadata.location_id,
                    location_name: metadata
                        .location_id
                        .and_then(|id| locations.get(&id).map(|value| value.name.clone())),
                    starts_at: shift.interval.starts_at,
                    ends_at: shift.interval.ends_at,
                },
                ResultPosition::Assignment {
                    person_id: pair.person_id,
                    shift_id: pair.shift_id,
                },
            ));
        } else {
            has_more = true;
        }
    }
    let (items, continuation) = fit_page(
        rows,
        total,
        has_more,
        document.scenario_id,
        context,
        &document.settings.time_zone,
        PageKind::Assignments,
    )?;
    Ok(ResultEnvelope {
        schema_version: 1,
        time_zone: &document.settings.time_zone,
        result: ResultData::Assignments {
            total_items: total,
            items,
            continuation,
        },
    })
}

// Preserve the assignment row shape while walking resolved shifts by UTC start instant.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_timeline_page<'doc>(
    selected_by_shift_person: &BTreeMap<ShiftId, BTreeMap<PersonId, &str>>,
    resolved: &[ResolvedShift],
    shifts: &BTreeMap<ShiftId, ShiftMetadata<'_>>,
    assignment_types: &BTreeMap<crate::ids::AssignmentTypeId, &crate::model::AssignmentType>,
    locations: &BTreeMap<LocationId, &crate::model::Location>,
    people: &BTreeMap<PersonId, &Person>,
    person_filter: Option<PersonId>,
    location_filter: Option<LocationId>,
    cursor: Option<&ResultPosition>,
    limit: usize,
    document: &'doc eutheto_types::ScenarioDocument,
    context: eutheto_domain_api::SetupViewContext,
    control: &OperationControl,
) -> Result<ResultEnvelope<'doc>, DomainPackError> {
    if person_filter.is_some_and(|person_id| !people.contains_key(&person_id)) {
        return Err(invalid(
            "/query/parameters/personId",
            "person filter is not in source",
        ));
    }
    let cursor_key = match cursor {
        None => None,
        Some(ResultPosition::Timeline {
            shift_id,
            person_id,
        }) => {
            let shift = resolved
                .iter()
                .find(|shift| shift.id == *shift_id)
                .ok_or_else(|| {
                    invalid(
                        "/query/continuation/position",
                        "cursor shift is not in source",
                    )
                })?;
            let metadata = shifts
                .get(shift_id)
                .ok_or_else(|| invalid("/domain", "resolved shift metadata is missing"))?;
            let selected_for_shift = selected_by_shift_person.get(shift_id).ok_or_else(|| {
                invalid(
                    "/query/continuation/position",
                    "cursor assignment is not selected source data",
                )
            })?;
            if !selected_for_shift.contains_key(person_id) {
                return Err(invalid(
                    "/query/continuation/position",
                    "cursor assignment is not selected source data",
                ));
            }
            if person_filter.is_some_and(|filter| filter != *person_id)
                || location_filter.is_some_and(|filter| metadata.location_id != Some(filter))
            {
                return Err(invalid(
                    "/query/continuation/position",
                    "cursor does not match timeline filter",
                ));
            }
            Some((shift.interval.starts_at.instant, *shift_id, *person_id))
        }
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor kind does not match page",
            ));
        }
    };
    let mut rows = Vec::new();
    let mut total = 0_u32;
    let mut has_more = false;
    for shift in resolved {
        control.check()?;
        let metadata = shifts
            .get(&shift.id)
            .ok_or_else(|| invalid("/domain", "resolved shift metadata is missing"))?;
        if location_filter.is_some_and(|filter| metadata.location_id != Some(filter)) {
            continue;
        }
        let Some(selected_for_shift) = selected_by_shift_person.get(&shift.id) else {
            continue;
        };
        for (person_id, assignment_id) in selected_for_shift {
            control.check()?;
            if person_filter.is_some_and(|filter| filter != *person_id) {
                continue;
            }
            total = total
                .checked_add(1)
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            let key = (shift.interval.starts_at.instant, shift.id, *person_id);
            if cursor_key.is_some_and(|cursor_key| key <= cursor_key) {
                continue;
            }
            if rows.len() < limit {
                let person = people
                    .get(person_id)
                    .ok_or_else(|| invalid("/domain", "selected person source is missing"))?;
                rows.push((
                    AssignmentRow {
                        assignment_id: (*assignment_id).to_owned(),
                        person_id: *person_id,
                        person_name: person.name.clone(),
                        shift_id: shift.id,
                        assignment_type_id: metadata.assignment_type_id,
                        assignment_type_name: assignment_types
                            .get(&metadata.assignment_type_id)
                            .map(|value| value.name.clone()),
                        location_id: metadata.location_id,
                        location_name: metadata
                            .location_id
                            .and_then(|id| locations.get(&id).map(|value| value.name.clone())),
                        starts_at: shift.interval.starts_at,
                        ends_at: shift.interval.ends_at,
                    },
                    ResultPosition::Timeline {
                        shift_id: shift.id,
                        person_id: *person_id,
                    },
                ));
            } else {
                has_more = true;
            }
        }
    }
    let (items, continuation) = fit_page(
        rows,
        total,
        has_more,
        document.scenario_id,
        context,
        &document.settings.time_zone,
        PageKind::Timeline,
    )?;
    Ok(ResultEnvelope {
        schema_version: 1,
        time_zone: &document.settings.time_zone,
        result: ResultData::Timeline {
            total_items: total,
            items,
            continuation,
        },
    })
}

#[allow(clippy::too_many_lines)]
fn append_health_no_applicable(
    domain: &WorkforceDomainV1,
    observations: &mut Vec<HealthObservation>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    let mut observed_rules = BTreeSet::new();
    let mut observed_owners = BTreeSet::new();
    for observation in observations.iter() {
        budget.step().map_err(|error| operation_error(&error))?;
        if let Some(rule_id) = observation.rule_id {
            observed_rules.insert(rule_id);
        }
        if let Some((owner_id, _)) = observation.coverage_owner {
            observed_owners.insert(owner_id);
        }
    }
    for rule in domain.rules.values() {
        budget.step().map_err(|error| operation_error(&error))?;
        let (rule_id, active, _) = rule.header();
        if !active {
            continue;
        }
        match rule {
            crate::model::WorkforceRule::MinimumRest(rule)
                if !observed_rules.contains(&rule.id) =>
            {
                budget
                    .reserve(1, 0, 192)
                    .map_err(|error| operation_error(&error))?;
                observations.push(HealthObservation {
                    kind: HealthObservationKind::MinimumRest,
                    rule_id: Some(rule.id),
                    coverage_owner: None,
                    person_id: None,
                    shift_id: None,
                    other_shift_id: None,
                    period: None,
                    units: HealthUnits::Minutes,
                    direction: HealthDirection::Minimum,
                    threshold: Some(
                        i128::from(rule.minimum_minutes)
                            .checked_mul(NANOS_PER_MINUTE)
                            .ok_or(DomainPackError::ResourceLimitExceeded)?,
                    ),
                    observed: None,
                    satisfied: None,
                    minimum_rank: None,
                    affected_pairs: Vec::new(),
                    affected_pair_count: 0,
                    affected_pairs_truncated: false,
                });
            }
            crate::model::WorkforceRule::MaximumHours {
                maximum_minutes, ..
            } if !observed_rules.contains(&rule_id) => {
                budget
                    .reserve(1, 0, 192)
                    .map_err(|error| operation_error(&error))?;
                observations.push(HealthObservation {
                    kind: HealthObservationKind::MaximumHours,
                    rule_id: Some(rule_id),
                    coverage_owner: None,
                    person_id: None,
                    shift_id: None,
                    other_shift_id: None,
                    period: None,
                    units: HealthUnits::Minutes,
                    direction: HealthDirection::Maximum,
                    threshold: Some(
                        i128::from(*maximum_minutes)
                            .checked_mul(NANOS_PER_MINUTE)
                            .ok_or(DomainPackError::ResourceLimitExceeded)?,
                    ),
                    observed: None,
                    satisfied: None,
                    minimum_rank: None,
                    affected_pairs: Vec::new(),
                    affected_pair_count: 0,
                    affected_pairs_truncated: false,
                });
            }
            crate::model::WorkforceRule::TransitionTime { .. }
                if !observed_rules.contains(&rule_id) =>
            {
                budget
                    .reserve(1, 0, 192)
                    .map_err(|error| operation_error(&error))?;
                observations.push(HealthObservation {
                    kind: HealthObservationKind::TransitionTime,
                    rule_id: Some(rule_id),
                    coverage_owner: None,
                    person_id: None,
                    shift_id: None,
                    other_shift_id: None,
                    period: None,
                    units: HealthUnits::Minutes,
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
            _ => {}
        }
    }
    for entity in domain.entities.values() {
        budget.step().map_err(|error| operation_error(&error))?;
        let (owner, owner_kind, coverage) = match entity {
            WorkforceEntity::ShiftTemplate(value) => {
                (value.id.as_entity_id(), "shift_template", &value.coverage)
            }
            WorkforceEntity::ShiftInstance(value) => {
                (value.id.as_entity_id(), "shift_instance", &value.coverage)
            }
            WorkforceEntity::CoverageRequirement(value) if value.active => (
                value.id.as_entity_id(),
                "coverage_requirement",
                &value.coverage,
            ),
            _ => continue,
        };
        if observed_owners.contains(&owner) {
            continue;
        }
        append_no_applicable_coverage(observations, owner, owner_kind, coverage, budget)?;
    }
    budget
        .sort_work(observations.len())
        .map_err(|error| operation_error(&error))?;
    observations.sort_unstable_by_key(|observation| {
        (
            observation.kind,
            observation.rule_id,
            observation.coverage_owner.map(|(id, _)| id),
            observation.person_id,
            observation.period.map(|(start, _)| start),
            observation.shift_id,
            observation.other_shift_id,
            observation.minimum_rank,
            observation.direction,
            observation.satisfied,
        )
    });
    Ok(())
}

fn append_no_applicable_coverage(
    observations: &mut Vec<HealthObservation>,
    owner: EntityId,
    owner_kind: &'static str,
    coverage: &Coverage,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    let (lower, upper, minimums) = match coverage {
        Coverage::Exact {
            count,
            qualification_minimums,
        } => (
            i128::from(*count),
            Some(i128::from(*count)),
            qualification_minimums,
        ),
        Coverage::AtLeast {
            minimum,
            maximum_count,
            qualification_minimums,
            ..
        } => (
            i128::from(*minimum),
            maximum_count.map(i128::from),
            qualification_minimums,
        ),
    };
    let direction = if upper == Some(lower) {
        HealthDirection::Exact
    } else {
        HealthDirection::Minimum
    };
    let observation = HealthObservation {
        kind: HealthObservationKind::CoverageHeadcount,
        rule_id: None,
        coverage_owner: Some((owner, owner_kind)),
        person_id: None,
        shift_id: None,
        other_shift_id: None,
        period: None,
        units: HealthUnits::People,
        direction,
        threshold: Some(lower),
        observed: None,
        satisfied: None,
        minimum_rank: None,
        affected_pairs: Vec::new(),
        affected_pair_count: 0,
        affected_pairs_truncated: false,
    };
    if let Some(upper) = upper.filter(|upper| *upper != lower) {
        budget
            .reserve(1, 0, 192)
            .map_err(|error| operation_error(&error))?;
        observations.push(HealthObservation {
            direction: HealthDirection::Maximum,
            threshold: Some(upper),
            ..observation.clone()
        });
    }
    budget
        .reserve(1, 0, 192)
        .map_err(|error| operation_error(&error))?;
    observations.push(observation);
    for (rank, minimum) in minimums.iter().enumerate() {
        budget.step().map_err(|error| operation_error(&error))?;
        budget
            .reserve(1, 0, 192)
            .map_err(|error| operation_error(&error))?;
        observations.push(HealthObservation {
            kind: HealthObservationKind::CoverageQualification,
            rule_id: None,
            coverage_owner: Some((owner, owner_kind)),
            person_id: None,
            shift_id: None,
            other_shift_id: None,
            period: None,
            units: HealthUnits::People,
            direction: HealthDirection::Minimum,
            threshold: Some(i128::from(minimum.minimum)),
            observed: None,
            satisfied: None,
            minimum_rank: Some(
                u64::try_from(rank).map_err(|_| DomainPackError::ResourceLimitExceeded)?,
            ),
            affected_pairs: Vec::new(),
            affected_pair_count: 0,
            affected_pairs_truncated: false,
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_health_page<'doc>(
    observations: &[HealthObservation],
    selected: &BTreeMap<AssignmentPair, &str>,
    cursor: Option<&ResultPosition>,
    limit: usize,
    document: &'doc eutheto_types::ScenarioDocument,
    context: eutheto_domain_api::SetupViewContext,
    control: &OperationControl,
) -> Result<ResultEnvelope<'doc>, DomainPackError> {
    let cursor_index = match cursor {
        None => None,
        Some(ResultPosition::Health { index }) => {
            if usize::try_from(*index)
                .ok()
                .is_none_or(|index| index >= observations.len())
            {
                return Err(invalid(
                    "/query/continuation/position",
                    "cursor health row is not in source",
                ));
            }
            Some(usize::try_from(*index).map_err(|_| {
                invalid(
                    "/query/continuation/position",
                    "cursor health row is not in source",
                )
            })?)
        }
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor kind does not match page",
            ));
        }
    };
    let total =
        u32::try_from(observations.len()).map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    let mut rows = Vec::with_capacity(limit.min(observations.len()));
    let mut has_more = false;
    for (index, observation) in observations.iter().enumerate() {
        control.check()?;
        if cursor_index.is_some_and(|cursor| index <= cursor) {
            continue;
        }
        if rows.len() < limit {
            rows.push((
                health_row(observation, selected)?,
                ResultPosition::Health {
                    index: u32::try_from(index)
                        .map_err(|_| DomainPackError::ResourceLimitExceeded)?,
                },
            ));
        } else {
            has_more = true;
        }
    }
    let (items, continuation) = fit_page(
        rows,
        total,
        has_more,
        document.scenario_id,
        context,
        &document.settings.time_zone,
        PageKind::Health,
    )?;
    Ok(ResultEnvelope {
        schema_version: 1,
        time_zone: &document.settings.time_zone,
        result: ResultData::Health {
            total_items: total,
            items,
            continuation,
        },
    })
}

fn health_row(
    observation: &HealthObservation,
    selected: &BTreeMap<AssignmentPair, &str>,
) -> Result<HealthRow, DomainPackError> {
    let mut affected_assignment_ids = Vec::new();
    for pair in &observation.affected_pairs {
        let assignment_id = selected.get(pair).ok_or_else(|| {
            invalid(
                "/solution/assignments",
                "health observation assignment source is missing",
            )
        })?;
        affected_assignment_ids.push((*assignment_id).to_owned());
    }
    let affected_assignment_count = u32::try_from(observation.affected_pair_count)
        .map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    let status = match observation.satisfied {
        Some(true) => HealthStatus::Satisfied,
        Some(false) => HealthStatus::Violated,
        None => HealthStatus::NoApplicable,
    };
    let margin = match (observation.threshold, observation.observed) {
        (Some(threshold), Some(observed)) => Some(
            health_margin(
                observation.direction,
                observation.units,
                threshold,
                observed,
            )?
            .into_result_value(),
        ),
        _ => None,
    };
    Ok(HealthRow {
        check: health_check_kind(observation.kind),
        status,
        units: health_units(observation.units),
        direction: observation.direction,
        source_rule_id: observation.rule_id,
        coverage_owner_id: observation.coverage_owner.map(|(id, _)| id),
        coverage_owner_kind: observation
            .coverage_owner
            .map(|(_, kind)| coverage_owner_kind(kind))
            .transpose()?,
        person_id: observation.person_id,
        shift_id: observation.shift_id,
        other_shift_id: observation.other_shift_id,
        window_starts_at: observation.period.map(|(start, _)| start),
        window_ends_at: observation.period.map(|(_, end)| end),
        minimum_rank: observation
            .minimum_rank
            .map(|rank| u32::try_from(rank).map_err(|_| DomainPackError::ResourceLimitExceeded))
            .transpose()?,
        threshold: observation
            .threshold
            .map(|value| health_value(observation.units, value))
            .transpose()?,
        observed: observation
            .observed
            .map(|value| health_value(observation.units, value))
            .transpose()?,
        margin,
        affected_assignment_ids,
        affected_assignment_count,
        affected_assignments_truncated: observation.affected_pairs_truncated,
    })
}

fn health_check_kind(kind: HealthObservationKind) -> HealthCheckKind {
    match kind {
        HealthObservationKind::MinimumRest => HealthCheckKind::MinimumRest,
        HealthObservationKind::MaximumHours => HealthCheckKind::MaximumHours,
        HealthObservationKind::TransitionTime => HealthCheckKind::TransitionTime,
        HealthObservationKind::CoverageHeadcount => HealthCheckKind::CoverageHeadcount,
        HealthObservationKind::CoverageQualification => HealthCheckKind::CoverageQualification,
    }
}

fn health_units(units: HealthUnits) -> HealthResultUnits {
    match units {
        HealthUnits::Minutes => HealthResultUnits::Minutes,
        HealthUnits::People => HealthResultUnits::People,
    }
}

fn coverage_owner_kind(kind: &'static str) -> Result<CoverageOwnerKind, DomainPackError> {
    match kind {
        "shift_template" => Ok(CoverageOwnerKind::ShiftTemplate),
        "shift_instance" => Ok(CoverageOwnerKind::ShiftInstance),
        "coverage_requirement" => Ok(CoverageOwnerKind::CoverageRequirement),
        _ => Err(invalid("/view/data", "unknown coverage owner kind")),
    }
}

fn health_value(units: HealthUnits, value: i128) -> Result<ExactResultValue, DomainPackError> {
    match units {
        HealthUnits::Minutes => {
            ExactFraction::new(value, NANOS_PER_MINUTE).map(ExactFraction::into_result_value)
        }
        HealthUnits::People => ExactFraction::new(value, 1).map(ExactFraction::into_result_value),
    }
}

fn health_margin(
    direction: HealthDirection,
    units: HealthUnits,
    threshold: i128,
    observed: i128,
) -> Result<ExactFraction, DomainPackError> {
    let margin = match direction {
        HealthDirection::Minimum => observed
            .checked_sub(threshold)
            .ok_or(DomainPackError::ResourceLimitExceeded)?,
        HealthDirection::Maximum => threshold
            .checked_sub(observed)
            .ok_or(DomainPackError::ResourceLimitExceeded)?,
        HealthDirection::Exact => {
            if observed == threshold {
                0
            } else {
                -i128::try_from(observed.abs_diff(threshold))
                    .map_err(|_| DomainPackError::ResourceLimitExceeded)?
            }
        }
    };
    let denominator = match units {
        HealthUnits::Minutes => NANOS_PER_MINUTE,
        HealthUnits::People => 1,
    };
    ExactFraction::new(margin, denominator)
}

const MAX_FAIRNESS_ROWS: usize = 100_000;
// Source limits permit many selected shifts to contribute to each retained row.
const MAX_FAIRNESS_INDEX_STEPS: usize = 10_000_000;
const MAX_FAIRNESS_UNION_INTERVALS: usize = 1_000_000;
const NANOS_PER_MINUTE: i128 = 60_000_000_000;

#[derive(Clone, Copy)]
struct FairnessPlan<'a> {
    preference_id: RuleId,
    priority: PreferencePriority,
    weight: u32,
    scope: &'a Scope,
    policy: &'a WorkloadPolicy,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FairnessPeriodKey {
    calendar_id: WorkCalendarId,
    start: Rfc3339Timestamp,
    end: Rfc3339Timestamp,
}

struct FairnessPeriods {
    windows: Vec<CalendarWindow>,
    shift_periods: BTreeMap<ShiftId, Vec<FairnessPeriodKey>>,
}
struct FairnessActualIndex {
    amounts: BTreeMap<(FairnessPeriodKey, PersonId), ExactFraction>,
    union_intervals:
        BTreeMap<(FairnessPeriodKey, PersonId), Vec<(Rfc3339Timestamp, Rfc3339Timestamp)>>,
}
type FairnessCursorKey = (WorkloadPolicyId, RuleId, Rfc3339Timestamp, PersonId);

struct FairnessPageRows {
    rows: Vec<(FairnessRow, ResultPosition)>,
    total: u32,
    has_more: bool,
    cursor_found: bool,
}
#[derive(Clone, Copy, Debug)]
struct ExactFraction {
    numerator: i128,
    denominator: i128,
}

#[allow(clippy::too_many_arguments)]
fn build_fairness_page<'doc>(
    document: &'doc eutheto_types::ScenarioDocument,
    domain: &WorkforceDomainV1,
    people: &BTreeMap<PersonId, &Person>,
    assignment_types: &BTreeMap<crate::ids::AssignmentTypeId, &crate::model::AssignmentType>,
    resolved: &[ResolvedShift],
    shifts: &BTreeMap<ShiftId, ShiftMetadata<'_>>,
    selected: &BTreeMap<AssignmentPair, &str>,
    cursor: Option<&ResultPosition>,
    limit: usize,
    context: eutheto_domain_api::SetupViewContext,
    control: &OperationControl,
) -> Result<ResultEnvelope<'doc>, DomainPackError> {
    let cursor_key = match cursor {
        None => None,
        Some(ResultPosition::Fairness {
            policy_id,
            preference_id,
            period_start,
            person_id,
        }) => Some((*policy_id, *preference_id, *period_start, *person_id)),
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor kind does not match page",
            ));
        }
    };
    let page = build_fairness_rows(
        document,
        domain,
        people,
        assignment_types,
        resolved,
        shifts,
        selected,
        cursor_key,
        limit,
        control,
    )?;
    if cursor_key.is_some() && !page.cursor_found {
        return Err(invalid(
            "/query/continuation/position",
            "cursor fairness row is not in source",
        ));
    }
    let (items, continuation) = fit_page(
        page.rows,
        page.total,
        page.has_more,
        document.scenario_id,
        context,
        &document.settings.time_zone,
        PageKind::Fairness,
    )?;
    Ok(ResultEnvelope {
        schema_version: 1,
        time_zone: &document.settings.time_zone,
        result: ResultData::Fairness {
            total_items: page.total,
            items,
            continuation,
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn build_fairness_rows(
    document: &eutheto_types::ScenarioDocument,
    domain: &WorkforceDomainV1,
    people: &BTreeMap<PersonId, &Person>,
    assignment_types: &BTreeMap<crate::ids::AssignmentTypeId, &crate::model::AssignmentType>,
    resolved: &[ResolvedShift],
    shifts: &BTreeMap<ShiftId, ShiftMetadata<'_>>,
    selected: &BTreeMap<AssignmentPair, &str>,
    cursor_key: Option<FairnessCursorKey>,
    limit: usize,
    control: &OperationControl,
) -> Result<FairnessPageRows, DomainPackError> {
    let mut page = FairnessPageRows {
        rows: Vec::with_capacity(limit),
        total: 0,
        has_more: false,
        cursor_found: false,
    };
    let mut plans = active_fairness_plans(domain)?;
    plans.sort_unstable_by_key(|plan| (plan.policy.id, plan.preference_id));
    let Some(score_policy) = domain.entities.values().find_map(|entity| match entity {
        WorkforceEntity::ScorePolicy(policy) => Some(policy),
        _ => None,
    }) else {
        return Ok(page);
    };
    for plan in plans {
        control.check()?;
        build_fairness_plan_rows(
            document,
            domain,
            score_policy,
            plan,
            people,
            assignment_types,
            resolved,
            shifts,
            selected,
            cursor_key,
            limit,
            &mut page,
            control,
        )?;
    }
    Ok(page)
}

fn active_fairness_plans(
    domain: &WorkforceDomainV1,
) -> Result<Vec<FairnessPlan<'_>>, DomainPackError> {
    let has_active = domain.preferences.values().any(|preference| {
        matches!(
            preference,
            WorkforcePreference::WorkloadBalance { active: true, .. }
        )
    });
    let Some(score_policy) = domain.entities.values().find_map(|entity| match entity {
        WorkforceEntity::ScorePolicy(policy) => Some(policy),
        _ => None,
    }) else {
        if has_active {
            return Err(invalid(
                "/domain/preferences",
                "active workload balance requires a score policy",
            ));
        }
        return Ok(Vec::new());
    };
    let mut plans = Vec::new();
    for preference in domain.preferences.values() {
        let WorkforcePreference::WorkloadBalance {
            id,
            active: true,
            priority,
            scope,
            weight,
            workload_policy_id,
        } = preference
        else {
            continue;
        };
        let policy = score_policy
            .workload_policies
            .get(workload_policy_id)
            .ok_or_else(|| invalid("/domain/preferences", "workload policy is missing"))?;
        plans.push(FairnessPlan {
            preference_id: *id,
            priority: *priority,
            weight: *weight,
            scope,
            policy,
        });
    }
    Ok(plans)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_fairness_plan_rows(
    document: &eutheto_types::ScenarioDocument,
    domain: &WorkforceDomainV1,
    score_policy: &WorkforceScorePolicy,
    plan: FairnessPlan<'_>,
    people: &BTreeMap<PersonId, &Person>,
    assignment_types: &BTreeMap<crate::ids::AssignmentTypeId, &crate::model::AssignmentType>,
    resolved: &[ResolvedShift],
    shifts: &BTreeMap<ShiftId, ShiftMetadata<'_>>,
    selected: &BTreeMap<AssignmentPair, &str>,
    cursor_key: Option<FairnessCursorKey>,
    limit: usize,
    page: &mut FairnessPageRows,
    control: &OperationControl,
) -> Result<(), DomainPackError> {
    let mut checkpoint = |_, _| control.check().map_err(DomainPackError::from);
    let mut peers = Vec::new();
    for (id, person) in people {
        if fairness_peer_matches(
            &plan.policy.peer_group.people,
            plan.policy.peer_group.team_ids.as_deref(),
            person,
            &mut checkpoint,
        )? {
            peers.push(*id);
        }
    }
    if peers.is_empty() {
        return Err(invalid(
            "/domain/scorePolicy/workloadPolicies",
            "workload peer group is empty",
        ));
    }
    let peer_group_size =
        u32::try_from(peers.len()).map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    let Some(WorkforceEntity::WorkloadBucket(bucket)) =
        domain.entities.get(&plan.policy.bucket_id.as_entity_id())
    else {
        return Err(invalid(
            "/domain/scorePolicy/workloadPolicies",
            "workload bucket is missing",
        ));
    };
    let bucket_name = bucket.name.clone();
    let bucket_measurement = bucket.measurement;
    let bucket_overlap = bucket.overlapping_contribution;
    if (bucket_measurement == WorkloadMeasurement::AssignmentCount
        && bucket_overlap == OverlappingContribution::Union)
        || (bucket_measurement == WorkloadMeasurement::ScheduledMinutes
            && (bucket_overlap == OverlappingContribution::Union
                || plan.policy.membership == WindowMembership::Intersection))
    {
        return Err(invalid(
            "/domain/scorePolicy/workloadPolicies",
            "workload policy has unsupported measurement or membership",
        ));
    }
    let mut matching_shifts = Vec::new();
    for shift in resolved {
        let Some(metadata) = shifts.get(&shift.id) else {
            continue;
        };
        let Some(assignment_type) = assignment_types.get(&metadata.assignment_type_id) else {
            continue;
        };
        checkpoint(
            FairnessStep::Inspect,
            assignment_type.workload_bucket_ids.len(),
        )?;
        if assignment_type
            .workload_bucket_ids
            .contains(&plan.policy.bucket_id)
            && fairness_shift_matches(
                plan.scope,
                shift,
                metadata,
                assignment_type,
                &mut checkpoint,
            )?
        {
            matching_shifts.push(shift);
        }
    }
    let FairnessPeriods {
        windows: periods,
        shift_periods,
        ..
    } = fairness_periods(
        document,
        domain,
        plan.policy,
        &matching_shifts,
        &mut checkpoint,
    )?;
    let target_mode = fairness_target_mode(plan.policy);
    let units = match bucket_measurement {
        WorkloadMeasurement::AssignmentCount => FairnessUnits::Assignments,
        WorkloadMeasurement::ElapsedMinutes | WorkloadMeasurement::ScheduledMinutes => {
            FairnessUnits::Minutes
        }
    };
    let plan_row_count = periods
        .len()
        .checked_mul(peers.len())
        .ok_or(DomainPackError::ResourceLimitExceeded)?;
    if plan_row_count > MAX_FAIRNESS_ROWS {
        return Err(DomainPackError::ResourceLimitExceeded);
    }
    let weighted_total = if matches!(
        &plan.policy.target_mode,
        WorkloadTargetMode::WeightedEqualShare {}
    ) {
        Some(
            peers
                .iter()
                .try_fold(ExactFraction::zero(), |sum, person_id| {
                    let person = people.get(person_id).ok_or_else(|| {
                        invalid("/domain/entities", "workload peer source is missing")
                    })?;
                    sum.add(ExactFraction::new(
                        i128::from(person.workload_weight.numerator),
                        i128::from(person.workload_weight.denominator),
                    )?)
                })?,
        )
    } else {
        None
    };
    let mut actual_index = build_fairness_actual_index(
        &matching_shifts,
        &shift_periods,
        &peers,
        plan.policy.membership,
        bucket_measurement,
        bucket_overlap,
        selected,
        &mut checkpoint,
    )?;
    for period in periods {
        control.check()?;
        let period_key = FairnessPeriodKey {
            calendar_id: period.calendar_id,
            start: period.interval.starts_at.instant,
            end: period.interval.ends_at.instant,
        };
        let mut actuals = Vec::with_capacity(peers.len());
        for person_id in &peers {
            control.check()?;
            actuals.push(actual_index.actual(
                period_key,
                *person_id,
                bucket_measurement,
                bucket_overlap,
                &mut checkpoint,
            )?);
        }
        let total = actuals
            .iter()
            .try_fold(ExactFraction::zero(), |sum, actual| sum.add(*actual))?;
        let targets = actuals
            .iter()
            .enumerate()
            .map(|(index, _)| {
                fairness_target(
                    plan.policy,
                    &peers,
                    peers[index],
                    total,
                    people,
                    weighted_total,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let deviations = actuals
            .iter()
            .zip(&targets)
            .map(|(actual, target)| actual.sub(*target))
            .collect::<Result<Vec<_>, _>>()?;
        let actual_minimum = fraction_minimum(&actuals)?;
        let actual_maximum = fraction_maximum(&actuals)?;
        let deviation_minimum = fraction_minimum(&deviations)?;
        let deviation_maximum = fraction_maximum(&deviations)?;
        let group_spread = actual_maximum.sub(actual_minimum)?;
        let group_deviation_spread = deviation_maximum.sub(deviation_minimum)?;
        for (index, person_id) in peers.iter().enumerate() {
            control.check()?;
            let position = ResultPosition::Fairness {
                policy_id: plan.policy.id,
                preference_id: plan.preference_id,
                period_start: period.interval.starts_at.instant,
                person_id: *person_id,
            };
            if !record_fairness_position(page, cursor_key, limit, &position)? {
                continue;
            }
            let person = people
                .get(person_id)
                .ok_or_else(|| invalid("/domain/entities", "workload peer source is missing"))?;
            let actual = actuals[index];
            let target = targets[index];
            let deviation = deviations[index];
            let row = FairnessRow {
                score_policy_id: score_policy.id,
                policy_id: plan.policy.id,
                preference_id: plan.preference_id,
                bucket_id: plan.policy.bucket_id,
                bucket_name: bucket_name.clone(),
                calendar_id: plan.policy.calendar_id,
                membership: plan.policy.membership,
                target_mode,
                measure: bucket_measurement,
                units,
                preference_priority: plan.priority,
                preference_weight: plan.weight,
                period_starts_at: period.interval.starts_at,
                period_ends_at: period.interval.ends_at,
                peer_group_size,
                person_id: *person_id,
                person_name: person.name.clone(),
                target: target.into_result_value(),
                actual: actual.into_result_value(),
                deviation: deviation.into_result_value(),
                group_actual_minimum: actual_minimum.into_result_value(),
                group_actual_maximum: actual_maximum.into_result_value(),
                group_spread: group_spread.into_result_value(),
                group_deviation_minimum: deviation_minimum.into_result_value(),
                group_deviation_maximum: deviation_maximum.into_result_value(),
                group_deviation_spread: group_deviation_spread.into_result_value(),
            };
            page.rows.push((row, position));
        }
    }
    Ok(())
}

fn record_fairness_position(
    page: &mut FairnessPageRows,
    cursor_key: Option<FairnessCursorKey>,
    limit: usize,
    position: &ResultPosition,
) -> Result<bool, DomainPackError> {
    if usize::try_from(page.total)
        .ok()
        .and_then(|count| count.checked_add(1))
        .is_none_or(|count| count > MAX_FAIRNESS_ROWS)
    {
        return Err(DomainPackError::ResourceLimitExceeded);
    }
    page.total = page
        .total
        .checked_add(1)
        .ok_or(DomainPackError::ResourceLimitExceeded)?;
    let key = match position {
        ResultPosition::Fairness {
            policy_id,
            preference_id,
            period_start,
            person_id,
        } => (*policy_id, *preference_id, *period_start, *person_id),
        _ => unreachable!("fairness row has fairness cursor position"),
    };
    if cursor_key.is_some_and(|cursor| cursor == key) {
        page.cursor_found = true;
    }
    if cursor_key.is_some_and(|cursor| key <= cursor) {
        return Ok(false);
    }
    if page.rows.len() >= limit {
        page.has_more = true;
        return Ok(false);
    }
    Ok(true)
}
fn fairness_peer_matches(
    selection: &PersonSelection,
    team_ids: Option<&[crate::ids::TeamId]>,
    person: &Person,
    checkpoint: &mut impl FnMut(FairnessStep, usize) -> Result<(), DomainPackError>,
) -> Result<bool, DomainPackError> {
    let people_match = match selection {
        PersonSelection::All {} => true,
        PersonSelection::Selected { person_ids } => {
            checkpoint(FairnessStep::Inspect, person_ids.len())?;
            person_ids.contains(&person.id)
        }
        PersonSelection::Filter { all_tags, any_tags } => {
            for tag in all_tags {
                checkpoint(FairnessStep::Inspect, 1)?;
                checkpoint(FairnessStep::Inspect, person.tags.len())?;
                if !person.tags.contains(tag) {
                    return Ok(false);
                }
            }
            let mut any = any_tags.is_empty();
            for tag in any_tags {
                checkpoint(FairnessStep::Inspect, 1)?;
                checkpoint(FairnessStep::Inspect, person.tags.len())?;
                if person.tags.contains(tag) {
                    any = true;
                    break;
                }
            }
            any
        }
    };
    if !people_match {
        return Ok(false);
    }
    if let Some(ids) = team_ids {
        for team_id in ids {
            checkpoint(FairnessStep::Inspect, 1)?;
            checkpoint(FairnessStep::Inspect, person.team_ids.len())?;
            if person.team_ids.contains(team_id) {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    Ok(true)
}

fn fairness_shift_matches(
    scope: &Scope,
    shift: &ResolvedShift,
    metadata: &ShiftMetadata<'_>,
    assignment_type: &crate::model::AssignmentType,
    checkpoint: &mut impl FnMut(FairnessStep, usize) -> Result<(), DomainPackError>,
) -> Result<bool, DomainPackError> {
    if let Some(ids) = &scope.assignment_type_ids {
        checkpoint(FairnessStep::Inspect, ids.len())?;
        if !ids.contains(&metadata.assignment_type_id) {
            return Ok(false);
        }
    }
    if let Some(categories) = &scope.categories {
        checkpoint(FairnessStep::Inspect, categories.len())?;
        if !categories.contains(&assignment_type.category) {
            return Ok(false);
        }
    }
    if let Some(days) = &scope.weekdays {
        checkpoint(FairnessStep::Inspect, days.len())?;
        if !days.contains(&weekday(shift.reporting_date)) {
            return Ok(false);
        }
    }
    if let Some(ids) = &scope.location_ids {
        checkpoint(FairnessStep::Inspect, ids.len())?;
        if !metadata.location_id.is_some_and(|id| ids.contains(&id)) {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Copy)]
enum FairnessStep {
    Inspect,
    RetainWindow,
    RetainIndex,
    UnionInterval,
    Sort,
}

fn fairness_periods(
    document: &eutheto_types::ScenarioDocument,
    domain: &WorkforceDomainV1,
    policy: &WorkloadPolicy,
    matching_shifts: &[&ResolvedShift],
    checkpoint: &mut impl FnMut(FairnessStep, usize) -> Result<(), DomainPackError>,
) -> Result<FairnessPeriods, DomainPackError> {
    let Some(WorkforceEntity::Calendar(calendar)) =
        domain.entities.get(&policy.calendar_id.as_entity_id())
    else {
        return Err(invalid(
            "/domain/scorePolicy/workloadPolicies",
            "workload calendar is missing",
        ));
    };
    let horizon = Horizon::new(
        document.settings.horizon.start,
        document.settings.horizon.end,
    )
    .map_err(|_| invalid("/domain/settings/horizon", "invalid planning horizon"))?;
    let base_windows = resolve_calendar_for_calendar(
        document,
        policy.calendar_id,
        calendar,
        horizon,
        &mut |step| fairness_calendar_step(step, checkpoint),
    )
    .map_err(temporal_error)?;
    let mut periods = BTreeMap::new();
    let mut shift_periods = BTreeMap::new();
    for window in base_windows {
        checkpoint(FairnessStep::RetainIndex, 1)?;
        let key = FairnessPeriodKey {
            calendar_id: window.calendar_id,
            start: window.interval.starts_at.instant,
            end: window.interval.ends_at.instant,
        };
        periods.insert(key, window);
    }
    for shift in matching_shifts {
        checkpoint(FairnessStep::Inspect, 1)?;
        let mut period_keys = Vec::new();
        match policy.membership {
            WindowMembership::ReportingDate => {
                if let Some(window) = reporting_window_for_calendar(
                    document,
                    policy.calendar_id,
                    calendar,
                    shift.reporting_date,
                    &mut |step| fairness_calendar_step(step, checkpoint),
                )
                .map_err(temporal_error)?
                {
                    checkpoint(FairnessStep::RetainIndex, 1)?;
                    let key = FairnessPeriodKey {
                        calendar_id: window.calendar_id,
                        start: window.interval.starts_at.instant,
                        end: window.interval.ends_at.instant,
                    };
                    periods.entry(key).or_insert(window);
                    period_keys.push(key);
                }
            }
            WindowMembership::StartInstant | WindowMembership::Intersection => {
                let range = Horizon::new(
                    shift.interval.starts_at.instant,
                    shift.interval.ends_at.instant,
                )
                .map_err(|_| invalid("/domain", "resolved shift interval is invalid"))?;
                let windows = resolve_calendar_for_calendar(
                    document,
                    policy.calendar_id,
                    calendar,
                    range,
                    &mut |step| fairness_calendar_step(step, checkpoint),
                )
                .map_err(temporal_error)?;
                for window in windows {
                    checkpoint(FairnessStep::Inspect, 1)?;
                    if policy.membership == WindowMembership::StartInstant
                        && !(window.interval.starts_at.instant <= shift.interval.starts_at.instant
                            && shift.interval.starts_at.instant < window.interval.ends_at.instant)
                    {
                        continue;
                    }
                    checkpoint(FairnessStep::RetainIndex, 1)?;
                    let key = FairnessPeriodKey {
                        calendar_id: window.calendar_id,
                        start: window.interval.starts_at.instant,
                        end: window.interval.ends_at.instant,
                    };
                    periods.entry(key).or_insert(window);
                    period_keys.push(key);
                }
            }
        }
        if !period_keys.is_empty() {
            checkpoint(FairnessStep::RetainIndex, 1)?;
            shift_periods.insert(shift.id, period_keys);
        }
    }
    // The final contiguous window vector is a second retained representation.
    checkpoint(FairnessStep::RetainWindow, periods.len())?;
    Ok(FairnessPeriods {
        windows: periods.into_values().collect(),
        shift_periods,
    })
}

fn fairness_calendar_step(
    step: crate::temporal::CalendarStep,
    checkpoint: &mut impl FnMut(FairnessStep, usize) -> Result<(), DomainPackError>,
) -> Result<(), TemporalError> {
    checkpoint(
        match step {
            crate::temporal::CalendarStep::Inspect => FairnessStep::Inspect,
            crate::temporal::CalendarStep::RetainWindow => FairnessStep::RetainWindow,
        },
        1,
    )
    .map_err(TemporalError::from)
}

#[allow(clippy::too_many_arguments)]
fn build_fairness_actual_index(
    matching_shifts: &[&ResolvedShift],
    shift_periods: &BTreeMap<ShiftId, Vec<FairnessPeriodKey>>,
    peers: &[PersonId],
    membership: WindowMembership,
    measurement: WorkloadMeasurement,
    overlap: OverlappingContribution,
    selected: &BTreeMap<AssignmentPair, &str>,
    checkpoint: &mut impl FnMut(FairnessStep, usize) -> Result<(), DomainPackError>,
) -> Result<FairnessActualIndex, DomainPackError> {
    if (measurement == WorkloadMeasurement::AssignmentCount
        && overlap == OverlappingContribution::Union)
        || (measurement == WorkloadMeasurement::ScheduledMinutes
            && (overlap == OverlappingContribution::Union
                || membership == WindowMembership::Intersection))
    {
        return Err(invalid(
            "/domain/scorePolicy/workloadPolicies",
            "workload policy has unsupported measurement or membership",
        ));
    }
    let mut matching_by_id = BTreeMap::new();
    for shift in matching_shifts {
        checkpoint(FairnessStep::RetainIndex, 1)?;
        matching_by_id.insert(shift.id, *shift);
    }
    let mut index = FairnessActualIndex {
        amounts: BTreeMap::new(),
        union_intervals: BTreeMap::new(),
    };
    let mut index_steps = 0usize;
    let mut union_interval_count = 0usize;
    for pair in selected.keys() {
        checkpoint(FairnessStep::Inspect, 1)?;
        if peers.binary_search(&pair.person_id).is_err() {
            continue;
        }
        let Some(shift) = matching_by_id.get(&pair.shift_id).copied() else {
            continue;
        };
        let Some(period_keys) = shift_periods.get(&shift.id) else {
            continue;
        };
        for period_key in period_keys {
            checkpoint(FairnessStep::Inspect, 1)?;
            let (start, end) = if membership == WindowMembership::Intersection {
                let start = shift.interval.starts_at.instant.max(period_key.start);
                let end = shift.interval.ends_at.instant.min(period_key.end);
                if start >= end {
                    continue;
                }
                (start, end)
            } else {
                (
                    shift.interval.starts_at.instant,
                    shift.interval.ends_at.instant,
                )
            };
            index_steps = index_steps
                .checked_add(1)
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            if index_steps > MAX_FAIRNESS_INDEX_STEPS {
                return Err(DomainPackError::ResourceLimitExceeded);
            }
            if measurement == WorkloadMeasurement::ElapsedMinutes
                && overlap == OverlappingContribution::Union
            {
                union_interval_count = union_interval_count
                    .checked_add(1)
                    .ok_or(DomainPackError::ResourceLimitExceeded)?;
                if union_interval_count > MAX_FAIRNESS_UNION_INTERVALS {
                    return Err(DomainPackError::ResourceLimitExceeded);
                }
                checkpoint(FairnessStep::UnionInterval, 1)?;
                index
                    .union_intervals
                    .entry((*period_key, pair.person_id))
                    .or_default()
                    .push((start, end));
                continue;
            }
            let contribution = match measurement {
                WorkloadMeasurement::AssignmentCount => ExactFraction::new(1, 1)?,
                WorkloadMeasurement::ElapsedMinutes => {
                    ExactFraction::new(timestamp_duration_nanos(start, end)?, NANOS_PER_MINUTE)?
                }
                WorkloadMeasurement::ScheduledMinutes => {
                    ExactFraction::new(local_duration_nanos(shift)?, NANOS_PER_MINUTE)?
                }
            };
            checkpoint(FairnessStep::RetainIndex, 1)?;
            let amount = index
                .amounts
                .entry((*period_key, pair.person_id))
                .or_insert_with(ExactFraction::zero);
            *amount = (*amount).add(contribution)?;
        }
    }
    Ok(index)
}

impl FairnessActualIndex {
    fn actual(
        &mut self,
        period: FairnessPeriodKey,
        person_id: PersonId,
        measurement: WorkloadMeasurement,
        overlap: OverlappingContribution,
        checkpoint: &mut impl FnMut(FairnessStep, usize) -> Result<(), DomainPackError>,
    ) -> Result<ExactFraction, DomainPackError> {
        checkpoint(FairnessStep::Inspect, 1)?;
        if measurement == WorkloadMeasurement::ElapsedMinutes
            && overlap == OverlappingContribution::Union
        {
            let Some(intervals) = self.union_intervals.get_mut(&(period, person_id)) else {
                return Ok(ExactFraction::zero());
            };
            return ExactFraction::new(
                union_duration_nanos(intervals, checkpoint)?,
                NANOS_PER_MINUTE,
            );
        }
        if overlap == OverlappingContribution::Union {
            return Err(invalid(
                "/domain/scorePolicy/workloadPolicies",
                "workload policy has unsupported measurement or membership",
            ));
        }
        Ok(self
            .amounts
            .get(&(period, person_id))
            .copied()
            .unwrap_or_else(ExactFraction::zero))
    }
}

fn timestamp_duration_nanos(
    start: Rfc3339Timestamp,
    end: Rfc3339Timestamp,
) -> Result<i128, DomainPackError> {
    let duration = start.as_timestamp().duration_until(end.as_timestamp());
    i128::from(duration.as_secs())
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i128::from(duration.subsec_nanos())))
        .ok_or(DomainPackError::ResourceLimitExceeded)
}

fn local_duration_nanos(shift: &ResolvedShift) -> Result<i128, DomainPackError> {
    let duration = shift
        .interval
        .starts_at
        .local
        .as_datetime()
        .duration_until(shift.interval.ends_at.local.as_datetime());
    i128::from(duration.as_secs())
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i128::from(duration.subsec_nanos())))
        .ok_or(DomainPackError::ResourceLimitExceeded)
}

fn union_duration_nanos(
    intervals: &mut [(Rfc3339Timestamp, Rfc3339Timestamp)],
    checkpoint: &mut impl FnMut(FairnessStep, usize) -> Result<(), DomainPackError>,
) -> Result<i128, DomainPackError> {
    checkpoint(FairnessStep::Sort, intervals.len())?;
    intervals.sort_unstable_by_key(|(start, end)| (*start, *end));
    checkpoint(FairnessStep::Inspect, 1)?;
    let Some((mut start, mut end)) = intervals.first().copied() else {
        return Ok(0);
    };
    let mut total = 0_i128;
    for (next_start, next_end) in intervals.iter().copied().skip(1) {
        checkpoint(FairnessStep::Inspect, 1)?;
        if next_start <= end {
            end = end.max(next_end);
        } else {
            total = total
                .checked_add(timestamp_duration_nanos(start, end)?)
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            start = next_start;
            end = next_end;
        }
    }
    total
        .checked_add(timestamp_duration_nanos(start, end)?)
        .ok_or(DomainPackError::ResourceLimitExceeded)
}

fn fairness_target(
    policy: &WorkloadPolicy,
    peers: &[PersonId],
    person_id: PersonId,
    total: ExactFraction,
    people: &BTreeMap<PersonId, &Person>,
    weighted_total: Option<ExactFraction>,
) -> Result<ExactFraction, DomainPackError> {
    match &policy.target_mode {
        WorkloadTargetMode::Explicit { targets } => {
            let target = targets
                .iter()
                .find(|target| target.person_id == person_id)
                .ok_or_else(|| {
                    invalid(
                        "/domain/scorePolicy/workloadPolicies",
                        "explicit target is missing",
                    )
                })?;
            ExactFraction::new(i128::from(target.target), 1)
        }
        WorkloadTargetMode::PersonTargets {} => {
            let person = people
                .get(&person_id)
                .ok_or_else(|| invalid("/domain/entities", "workload peer source is missing"))?;
            let target = person
                .workload_target
                .ok_or_else(|| invalid("/domain/entities", "person workload target is missing"))?;
            if target.bucket_id != policy.bucket_id
                || target.calendar_id != policy.calendar_id
                || target.membership != policy.membership
            {
                return Err(invalid(
                    "/domain/entities",
                    "person workload target does not match policy",
                ));
            }
            ExactFraction::new(i128::from(target.target), 1)
        }
        WorkloadTargetMode::EqualShare {} => {
            if total.numerator == 0 {
                ExactFraction::new(0, 1)
            } else {
                total.div(ExactFraction::new(
                    i128::try_from(peers.len())
                        .map_err(|_| DomainPackError::ResourceLimitExceeded)?,
                    1,
                )?)
            }
        }
        WorkloadTargetMode::WeightedEqualShare {} => {
            if total.numerator == 0 {
                return ExactFraction::new(0, 1);
            }
            let person = people
                .get(&person_id)
                .ok_or_else(|| invalid("/domain/entities", "workload peer source is missing"))?;
            let weight = ExactFraction::new(
                i128::from(person.workload_weight.numerator),
                i128::from(person.workload_weight.denominator),
            )?;
            let total_weight = weighted_total.ok_or_else(|| {
                invalid(
                    "/domain/scorePolicy/workloadPolicies",
                    "weighted target total is missing",
                )
            })?;
            total.mul(weight)?.div(total_weight)
        }
    }
}

fn fairness_target_mode(policy: &WorkloadPolicy) -> FairnessTargetMode {
    match &policy.target_mode {
        WorkloadTargetMode::Explicit { .. } => FairnessTargetMode::Explicit,
        WorkloadTargetMode::PersonTargets {} => FairnessTargetMode::PersonTargets,
        WorkloadTargetMode::WeightedEqualShare {} => FairnessTargetMode::WeightedEqualShare,
        WorkloadTargetMode::EqualShare {} => FairnessTargetMode::EqualShare,
    }
}

fn fraction_minimum(values: &[ExactFraction]) -> Result<ExactFraction, DomainPackError> {
    let Some(first) = values.first().copied() else {
        return Err(invalid("/view/data", "fairness peer group is empty"));
    };
    values
        .iter()
        .copied()
        .skip(1)
        .try_fold(first, |minimum, value| {
            Ok(if value.cmp(minimum)? == Ordering::Less {
                value
            } else {
                minimum
            })
        })
}

fn fraction_maximum(values: &[ExactFraction]) -> Result<ExactFraction, DomainPackError> {
    let Some(first) = values.first().copied() else {
        return Err(invalid("/view/data", "fairness peer group is empty"));
    };
    values
        .iter()
        .copied()
        .skip(1)
        .try_fold(first, |maximum, value| {
            Ok(if value.cmp(maximum)? == Ordering::Greater {
                value
            } else {
                maximum
            })
        })
}

impl ExactFraction {
    fn zero() -> Self {
        Self {
            numerator: 0,
            denominator: 1,
        }
    }

    fn new(numerator: i128, denominator: i128) -> Result<Self, DomainPackError> {
        if denominator == 0 {
            return Err(invalid(
                "/view/data",
                "fairness rational denominator is zero",
            ));
        }
        let sign = if denominator < 0 { -1 } else { 1 };
        let numerator = numerator
            .checked_mul(sign)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        let denominator = denominator
            .checked_mul(sign)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        let gcd = gcd_u128(numerator.unsigned_abs(), denominator.unsigned_abs());
        let gcd = i128::try_from(gcd).map_err(|_| DomainPackError::ResourceLimitExceeded)?;
        Ok(Self {
            numerator: numerator
                .checked_div(gcd)
                .ok_or(DomainPackError::ResourceLimitExceeded)?,
            denominator: denominator
                .checked_div(gcd)
                .ok_or(DomainPackError::ResourceLimitExceeded)?,
        })
    }

    fn add(self, other: Self) -> Result<Self, DomainPackError> {
        let left = self
            .numerator
            .checked_mul(other.denominator)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        let right = other
            .numerator
            .checked_mul(self.denominator)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        Self::new(
            left.checked_add(right)
                .ok_or(DomainPackError::ResourceLimitExceeded)?,
            self.denominator
                .checked_mul(other.denominator)
                .ok_or(DomainPackError::ResourceLimitExceeded)?,
        )
    }

    fn sub(self, other: Self) -> Result<Self, DomainPackError> {
        self.add(Self::new(
            other
                .numerator
                .checked_neg()
                .ok_or(DomainPackError::ResourceLimitExceeded)?,
            other.denominator,
        )?)
    }

    fn mul(self, other: Self) -> Result<Self, DomainPackError> {
        Self::new(
            self.numerator
                .checked_mul(other.numerator)
                .ok_or(DomainPackError::ResourceLimitExceeded)?,
            self.denominator
                .checked_mul(other.denominator)
                .ok_or(DomainPackError::ResourceLimitExceeded)?,
        )
    }

    fn div(self, other: Self) -> Result<Self, DomainPackError> {
        if other.numerator == 0 {
            return Err(invalid("/view/data", "fairness rational divisor is zero"));
        }
        Self::new(
            self.numerator
                .checked_mul(other.denominator)
                .ok_or(DomainPackError::ResourceLimitExceeded)?,
            self.denominator
                .checked_mul(other.numerator)
                .ok_or(DomainPackError::ResourceLimitExceeded)?,
        )
    }

    fn cmp(self, other: Self) -> Result<Ordering, DomainPackError> {
        let left = self
            .numerator
            .checked_mul(other.denominator)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        let right = other
            .numerator
            .checked_mul(self.denominator)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        Ok(left.cmp(&right))
    }

    fn into_result_value(self) -> ExactResultValue {
        ExactResultValue {
            numerator: self.numerator.to_string(),
            denominator: self.denominator.to_string(),
        }
    }
}

fn gcd_u128(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[allow(clippy::too_many_lines)]
fn fit_page<T>(
    mut rows: Vec<(T, ResultPosition)>,
    total: u32,
    mut has_more: bool,
    scenario_id: eutheto_types::ScenarioId,
    context: eutheto_domain_api::SetupViewContext,
    time_zone: &IanaTimeZone,
    kind: PageKind<'_>,
) -> Result<(Vec<T>, Option<SetupContinuationV1>), DomainPackError>
where
    T: Serialize,
{
    let had_rows = !rows.is_empty();
    loop {
        if let PageKind::RepairDiff { control, .. } = kind {
            control.check()?;
        }
        let continuation = if has_more {
            rows.last()
                .map(|(_, position)| {
                    Ok::<SetupContinuationV1, DomainPackError>(SetupContinuationV1 {
                        schema_version: 1,
                        scenario_id,
                        revision: context.revision,
                        query_fingerprint: context.query_fingerprint,
                        position: serde_json::to_value(position).map_err(|_| {
                            invalid("/view/data/continuation", "cursor encoding failed")
                        })?,
                    })
                })
                .transpose()?
        } else {
            None
        };
        let envelope = match kind {
            PageKind::People => ResultEnvelopeBorrowed {
                schema_version: 1,
                time_zone,
                result: ResultDataBorrowed::People {
                    total_items: total,
                    items: RowsRef(&rows),
                    continuation: continuation.clone(),
                },
            },
            PageKind::Shifts => ResultEnvelopeBorrowed {
                schema_version: 1,
                time_zone,
                result: ResultDataBorrowed::Shifts {
                    total_items: total,
                    items: RowsRef(&rows),
                    continuation: continuation.clone(),
                },
            },
            PageKind::Assignments => ResultEnvelopeBorrowed {
                schema_version: 1,
                time_zone,
                result: ResultDataBorrowed::Assignments {
                    total_items: total,
                    items: RowsRef(&rows),
                    continuation: continuation.clone(),
                },
            },
            PageKind::Timeline => ResultEnvelopeBorrowed {
                schema_version: 1,
                time_zone,
                result: ResultDataBorrowed::Timeline {
                    total_items: total,
                    items: RowsRef(&rows),
                    continuation: continuation.clone(),
                },
            },
            PageKind::Fairness => ResultEnvelopeBorrowed {
                schema_version: 1,
                time_zone,
                result: ResultDataBorrowed::Fairness {
                    total_items: total,
                    items: RowsRef(&rows),
                    continuation: continuation.clone(),
                },
            },
            PageKind::BaseDiff { base_schedule } => ResultEnvelopeBorrowed {
                schema_version: 1,
                time_zone,
                result: ResultDataBorrowed::BaseDiff {
                    base_schedule,
                    total_items: total,
                    items: RowsRef(&rows),
                    continuation: continuation.clone(),
                },
            },
            PageKind::RepairDiff {
                source,
                baseline,
                counts,
                ..
            } => ResultEnvelopeBorrowed {
                schema_version: 1,
                time_zone,
                result: ResultDataBorrowed::RepairDiff {
                    availability: RepairDiffAvailability::Available,
                    source,
                    baseline: Some(baseline),
                    counts: Some(counts),
                    total_items: Some(total),
                    items: RowsRef(&rows),
                    continuation: continuation.clone(),
                },
            },
            PageKind::Health => ResultEnvelopeBorrowed {
                schema_version: 1,
                time_zone,
                result: ResultDataBorrowed::Health {
                    total_items: total,
                    items: RowsRef(&rows),
                    continuation: continuation.clone(),
                },
            },
            PageKind::CalendarPeople {
                planning_dates,
                window,
            } => ResultEnvelopeBorrowed {
                schema_version: 1,
                time_zone,
                result: ResultDataBorrowed::CalendarPeople {
                    planning_dates,
                    window,
                    total_items: total,
                    items: RowsRef(&rows),
                    continuation: continuation.clone(),
                },
            },
            PageKind::CalendarShifts {
                planning_dates,
                window,
            } => ResultEnvelopeBorrowed {
                schema_version: 1,
                time_zone,
                result: ResultDataBorrowed::CalendarShifts {
                    planning_dates,
                    window,
                    total_items: total,
                    items: RowsRef(&rows),
                    continuation: continuation.clone(),
                },
            },
        };
        if bounded_json_size(&envelope, MAX_DATA_BYTES).is_ok() {
            let items = rows.into_iter().map(|(row, _)| row).collect();
            return Ok((items, continuation));
        }
        if rows.pop().is_none() {
            return Err(DomainPackError::ResourceLimitExceeded);
        }
        has_more = true;
        if rows.is_empty() && had_rows {
            return Err(DomainPackError::ResourceLimitExceeded);
        }
    }
}

fn coverage_values(
    coverage: &Coverage,
    selected_count: u32,
) -> (Option<u32>, u32, Option<u32>, Option<u32>, Option<i64>) {
    match coverage {
        Coverage::Exact { count, .. } => {
            let required = u32::from(*count);
            (
                Some(required),
                required,
                None,
                Some(required),
                Some(i64::from(selected_count) - i64::from(required)),
            )
        }
        Coverage::AtLeast {
            minimum,
            preferred_count,
            maximum_count,
            ..
        } => {
            let minimum = u32::from(*minimum);
            (
                None,
                minimum,
                preferred_count.map(u32::from),
                maximum_count.map(u32::from),
                Some(i64::from(selected_count) - i64::from(minimum)),
            )
        }
    }
}

fn temporal_error(error: TemporalError) -> DomainPackError {
    match error {
        TemporalError::InvalidDocument(error) => error,
        TemporalError::Cancelled => DomainPackError::Cancelled,
        TemporalError::Issue(_) => {
            invalid("/domain", "accepted schedule source could not be resolved")
        }
    }
}

fn invalid(path: &str, message: &str) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assignment_rules::{
        workforce_baseline_reference, workforce_manual_selection_checksum,
        workforce_pair_content_hash,
    };
    use crate::model::ManualSelection;
    use crate::test_support::{fixture, id};
    use eutheto_domain_api::{DomainSetupQueryV1, SetupViewContext};
    use eutheto_domain_ir::{
        AcceptedResultRefV1, AssignmentValue, DomainAssignment, DomainAssignmentId, DomainEntityId,
        DomainEntityKindId, DomainEntityRef, ObjectiveProfileId, RunObjectiveOperation,
    };
    use eutheto_types::{CancellationToken, Revision, ScenarioDocument};
    use serde_json::Value;
    use serde_json::json;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn union_actuals_share_work_admission_before_sorting() -> TestResult {
        let interval = |start: &str, end: &str| -> TestResult<_> {
            Ok((
                start.parse::<Rfc3339Timestamp>()?,
                end.parse::<Rfc3339Timestamp>()?,
            ))
        };
        let source = [
            interval("2026-01-01T00:05:00Z", "2026-01-01T00:05:30.001Z")?,
            interval("2026-01-01T00:01:00Z", "2026-01-01T00:03:00Z")?,
            interval("2026-01-01T00:00:00Z", "2026-01-01T00:02:00Z")?,
            interval("2026-01-01T00:03:00Z", "2026-01-01T00:04:00Z")?,
        ];
        let mut budget = OperationBudget::evaluation(None);
        budget.steps(crate::assignment_rules::budget::MAX_WORK_STEPS - 20)?;
        let mut checkpoint = |step, count| {
            match step {
                FairnessStep::Sort => budget.sort_work(count),
                _ => budget.steps(count as u64),
            }
            .map_err(|error| operation_error(&error))
        };
        let mut first = source;
        assert_eq!(
            union_duration_nanos(&mut first, &mut checkpoint)?,
            270_001_000_000,
        );
        let mut second = source;
        assert!(matches!(
            union_duration_nanos(&mut second, &mut checkpoint),
            Err(DomainPackError::ResourceLimitExceeded)
        ));
        assert_eq!(second, source);

        let cancellation = CancellationToken::new();
        let control = OperationControl::Cancellation(cancellation);
        let mut budget = OperationBudget::evaluation(Some(&control));
        budget.cancel_after_steps(14)?;
        let mut checkpoint = |step, count| {
            match step {
                FairnessStep::Sort => budget.sort_work(count),
                _ => budget.steps(count as u64),
            }
            .map_err(|error| operation_error(&error))
        };
        assert!(matches!(
            union_duration_nanos(&mut first, &mut checkpoint),
            Err(DomainPackError::Cancelled)
        ));
        Ok(())
    }

    fn repair_pairs(pairs: &[(u32, u32)]) -> TestResult<Vec<AssignmentPair>> {
        let mut pairs = pairs
            .iter()
            .map(|(person, shift)| {
                Ok(AssignmentPair {
                    person_id: id(*person).parse()?,
                    shift_id: id(*shift).parse()?,
                })
            })
            .collect::<TestResult<Vec<_>>>()?;
        pairs.sort_unstable();
        Ok(pairs)
    }

    #[allow(clippy::too_many_lines)]
    fn repair_fixture(
        original: &[(u32, u32)],
        manual: Option<&[(u32, u32)]>,
        candidate: &[(u32, u32)],
    ) -> TestResult<(ScenarioDocument, NormalizedSolution, RunObjectiveContext)> {
        let mut document = fixture()?;
        document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
        document.domain.locked_assignments.clear();
        for person in [12, 13, 15] {
            let mut record = document.domain.entities[&id(1).parse()?].clone();
            record["id"] = json!(id(person));
            record["name"] = json!(format!("Person {person}"));
            record["externalId"] = json!(format!("repair-person-{person}"));
            document.domain.entities.insert(id(person).parse()?, record);
        }
        for shift in [16, 17, 18, 19] {
            let mut record = document.domain.entities[&id(8).parse()?].clone();
            record["id"] = json!(id(shift));
            document.domain.entities.insert(id(shift).parse()?, record);
        }
        let original_pairs = repair_pairs(original)?;
        let base_id = id(40).parse()?;
        let original_hash = workforce_pair_content_hash(&original_pairs, None)?;
        let original = RunBaselineOriginal {
            entity: workforce_baseline_reference(base_id, false)?,
            pair_content_hash: original_hash.clone(),
            accepted_result: AcceptedResultRefV1 {
                solution_id: id(41).parse()?,
                result_checksum: "a".repeat(64),
            },
            source_scenario_id: document.scenario_id,
            source_revision: 0,
            source_document_hash: "b".repeat(64),
            source_run_id: id(42).parse()?,
            source_run_input_checksum: "c".repeat(64),
            source_run_manifest_checksum: "d".repeat(64),
        };
        document.domain.entities.insert(
            base_id,
            json!({
                "kind":"baseSchedule", "id":base_id,
                "sourceSolutionId":original.accepted_result.solution_id,
                "sourceRevision":0, "assignments":original_pairs
            }),
        );
        let binding = if let Some(manual) = manual {
            let assignments = repair_pairs(manual)?;
            let selected_pair_content_hash = workforce_pair_content_hash(&assignments, None)?;
            let mut record = ManualSelection {
                schema_version: 1,
                id: id(50).parse()?,
                original: original.clone(),
                admission_source_scenario_id: document.scenario_id,
                admission_source_revision: Revision::INITIAL,
                admission_source_document_hash: "e".repeat(64),
                admission_command_id: id(51).parse()?,
                assignments,
                checksum: String::new(),
            };
            record.checksum = workforce_manual_selection_checksum(&record, None)?;
            let selection = RunBaselineSelection::ManualSelection {
                entity: workforce_baseline_reference(record.id, true)?,
                record_checksum: record.checksum.clone(),
                admission_source_scenario_id: record.admission_source_scenario_id,
                admission_source_revision: record.admission_source_revision.value(),
                admission_source_document_hash: record.admission_source_document_hash.clone(),
            };
            document.domain.entities.insert(
                record.id,
                serde_json::to_value(WorkforceEntity::ManualSelection(record))?,
            );
            RunBaselineBinding {
                captured_original: None,
                original,
                selection,
                selected_pair_content_hash,
            }
        } else {
            RunBaselineBinding {
                captured_original: None,
                original,
                selection: RunBaselineSelection::OriginalAccepted {},
                selected_pair_content_hash: original_hash,
            }
        };
        let assignments = repair_pairs(candidate)?
            .into_iter()
            .map(|pair| {
                let identity = format!("{}.{}", pair.person_id, pair.shift_id);
                Ok(DomainAssignment {
                    id: DomainAssignmentId::new(format!(
                        "official.workforce.assignment.{identity}"
                    ))?,
                    entity: DomainEntityRef {
                        kind: DomainEntityKindId::new("official.workforce.assignment")?,
                        id: DomainEntityId::new(identity)?,
                    },
                    value: AssignmentValue::Boolean(true),
                    evidence: Vec::new(),
                })
            })
            .collect::<TestResult<Vec<_>>>()?;
        let solution = NormalizedSolution {
            schema_version: 1,
            pack_id: document.domain_pack.id.clone(),
            scenario_id: document.scenario_id,
            scenario_revision: 0,
            projection_version: WORKFORCE_PROJECTION_VERSION,
            solution_id: id(700).parse()?,
            assignments,
        };
        let objective = RunObjectiveContext {
            schema_version: 1,
            pack_id: document.domain_pack.id.clone(),
            profile_id: ObjectiveProfileId::new("official.workforce.objective.minimal-changes")?,
            semantic_version: 1,
            operation: RunObjectiveOperation::Repair,
            baseline: Some(Box::new(binding)),
            external_baseline: None,
        };
        Ok((document, solution, objective))
    }

    fn repair_test_context() -> SetupViewContext {
        SetupViewContext {
            revision: Revision::INITIAL,
            query_fingerprint: [7; 32],
        }
    }

    fn repair_page(
        document: &ScenarioDocument,
        solution: &NormalizedSolution,
        objective: Option<&RunObjectiveContext>,
        kind: &str,
        limit: usize,
        continuation: Option<SetupContinuationV1>,
    ) -> TestResult<Value> {
        let query = DomainSetupQueryV1 {
            schema_version: 1,
            view_id: RESULT_VIEW_ID.to_owned(),
            parameters: json!({"kind":kind, "limit":limit}),
            continuation,
        };
        Ok(build_view(
            DomainViewInput::AcceptedSolution {
                document,
                solution,
                objective_context: objective,
                view_id: RESULT_VIEW_ID,
                query: &query,
                context: repair_test_context(),
            },
            &OperationControl::Cancellation(CancellationToken::new()),
        )?
        .view
        .data)
    }

    #[test]
    // Keep the distinct A/B/C members and all grouped classifications together.
    #[allow(clippy::too_many_lines)]
    fn recorded_repair_diff_uses_manual_b_even_when_original_a_equals_c() -> TestResult {
        let candidate = [
            (1, 7),
            (1, 8),
            (15, 8),
            (13, 17),
            (1, 18),
            (1, 19),
            (13, 19),
        ];
        let manual = [
            (1, 7),
            (1, 8),
            (12, 8),
            (13, 8),
            (12, 16),
            (1, 18),
            (12, 18),
            (1, 19),
        ];
        let (document, solution, objective) =
            repair_fixture(&candidate, Some(&manual), &candidate)?;
        let data = repair_page(
            &document,
            &solution,
            Some(&objective),
            "repairDiff",
            100,
            None,
        )?;
        let result = &data["result"];
        let binding = objective.baseline.as_deref().ok_or("missing baseline")?;
        assert_ne!(
            binding.selected_pair_content_hash,
            binding.original.pair_content_hash
        );
        assert_eq!(result["baseline"]["selection"]["kind"], "manualSelection");
        assert_eq!(result["baseline"]["verificationStatus"], "notEstablished");
        assert_eq!(
            result["baseline"]["original"]["acceptedResult"],
            serde_json::to_value(&binding.original.accepted_result)?
        );
        assert_eq!(result["source"]["solutionId"], id(700));
        assert_eq!(
            result["source"]["normalizedSolutionHash"],
            solution.canonical_hash()?
        );
        assert_eq!(
            result["source"]["sourceDocumentHash"],
            eutheto_domain_ir::blake3_hex(&serde_json::to_vec(&document)?)
        );
        assert_eq!(
            result["counts"],
            json!({"removedPairs":4,"addedPairs":3,"unchangedPairs":4})
        );
        assert_eq!(result["totalItems"], 6);
        let rows = result["items"].as_array().ok_or("missing rows")?;
        assert_eq!(
            rows.iter()
                .map(|row| row["change"].as_str())
                .collect::<Vec<_>>(),
            vec![
                Some("unchanged"),
                Some("replacement"),
                Some("newlyUnfilled"),
                Some("newlyAssigned"),
                Some("newlyUnfilled"),
                Some("newlyAssigned")
            ]
        );
        assert_eq!(
            rows[1]["oldMembers"]
                .as_array()
                .ok_or("missing old members")?
                .len(),
            3
        );
        assert_eq!(
            rows[1]["newMembers"]
                .as_array()
                .ok_or("missing new members")?
                .len(),
            2
        );
        assert_eq!(
            rows[1]["oldMembers"]
                .as_array()
                .ok_or("missing old members")?
                .iter()
                .map(|member| member["personId"].clone())
                .collect::<Vec<_>>(),
            vec![json!(id(1)), json!(id(12)), json!(id(13))],
        );
        assert_eq!(
            rows[1]["newMembers"]
                .as_array()
                .ok_or("missing new members")?
                .iter()
                .map(|member| member["personId"].clone())
                .collect::<Vec<_>>(),
            vec![json!(id(1)), json!(id(15))],
        );
        assert_eq!(rows[0]["oldMembers"], rows[0]["newMembers"]);
        assert_eq!(rows[2]["newMembers"], json!([]));
        assert_eq!(rows[3]["oldMembers"], json!([]));
        assert_eq!(rows[1]["unchangedParticipants"][0]["personId"], id(1));
        assert_eq!(rows[1]["removedPairs"], 2);
        assert_eq!(rows[1]["addedPairs"], 1);
        assert_eq!(rows[2]["afterMemberCount"], 0);
        assert_eq!(rows[4]["afterMemberCount"], 1);
        assert_eq!(rows[5]["beforeMemberCount"], 1);
        assert_eq!(rows[0]["shiftLabel"], "Sunday Clinic");
        assert_eq!(rows[1]["shiftLabel"], "Clinic");
        assert_eq!(rows[1]["oldMembers"][0]["personName"], "River");
        let original_diff = repair_page(
            &document,
            &solution,
            Some(&objective),
            "baseDiff",
            100,
            None,
        )?;
        assert_eq!(original_diff["result"]["totalItems"], 0);
        Ok(())
    }

    #[test]
    fn recorded_repair_diff_pages_traverse_exact_union_and_reject_rebound_cursor() -> TestResult {
        let candidate = [(1, 7), (1, 8), (13, 17), (1, 18)];
        let manual = [(1, 7), (12, 8), (12, 16), (1, 18)];
        let (document, solution, objective) =
            repair_fixture(&candidate, Some(&manual), &candidate)?;
        let expected = repair_page(
            &document,
            &solution,
            Some(&objective),
            "repairDiff",
            100,
            None,
        )?;
        let mut continuation = None;
        let mut traversed = Vec::new();
        let mut saved_cursor = None;
        loop {
            let data = repair_page(
                &document,
                &solution,
                Some(&objective),
                "repairDiff",
                1,
                continuation,
            )?;
            assert_eq!(data["result"]["counts"], expected["result"]["counts"]);
            assert_eq!(
                data["result"]["totalItems"],
                expected["result"]["totalItems"]
            );
            let rows = data["result"]["items"].as_array().ok_or("missing page")?;
            assert_eq!(rows.len(), 1);
            traversed.extend(rows.iter().cloned());
            let cursor = &data["result"]["continuation"];
            if cursor.is_null() {
                break;
            }
            let cursor: SetupContinuationV1 = serde_json::from_value(cursor.clone())?;
            assert_eq!(cursor.position["kind"], "repairDiff");
            saved_cursor = Some(cursor.clone());
            continuation = Some(cursor);
        }
        assert_eq!(Value::Array(traversed), expected["result"]["items"]);
        let mut stale = saved_cursor.ok_or("missing continuation")?;
        stale.query_fingerprint = [8; 32];
        assert!(
            repair_page(
                &document,
                &solution,
                Some(&objective),
                "repairDiff",
                1,
                Some(stale)
            )
            .is_err()
        );
        let foreign_kind = SetupContinuationV1 {
            schema_version: 1,
            scenario_id: document.scenario_id,
            revision: Revision::INITIAL,
            query_fingerprint: [7; 32],
            position: json!({"kind":"shift", "shiftId":id(7)}),
        };
        assert!(
            repair_page(
                &document,
                &solution,
                Some(&objective),
                "repairDiff",
                1,
                Some(foreign_kind)
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn recorded_repair_diff_rejects_replaced_snapshot_b_without_current_head_fallback() -> TestResult
    {
        let (document, solution, objective) =
            repair_fixture(&[(1, 7)], Some(&[(1, 8)]), &[(1, 7)])?;
        let captured = repair_page(
            &document,
            &solution,
            Some(&objective),
            "repairDiff",
            100,
            None,
        )?;
        let mut changed_head = document.clone();
        let manual_id: EntityId = id(50).parse()?;
        let WorkforceEntity::ManualSelection(mut record) = serde_json::from_value::<WorkforceEntity>(
            changed_head.domain.entities[&manual_id].clone(),
        )?
        else {
            return Err("missing manual selection".into());
        };
        record.assignments = repair_pairs(&[(1, 7)])?;
        record.checksum = workforce_manual_selection_checksum(&record, None)?;
        changed_head.domain.entities.insert(
            manual_id,
            serde_json::to_value(WorkforceEntity::ManualSelection(record))?,
        );
        assert!(
            repair_page(
                &changed_head,
                &solution,
                Some(&objective),
                "repairDiff",
                100,
                None
            )
            .is_err()
        );
        assert_eq!(
            repair_page(
                &document,
                &solution,
                Some(&objective),
                "repairDiff",
                100,
                None
            )?,
            captured,
        );
        Ok(())
    }

    #[test]
    fn recorded_repair_diff_preserves_missing_historical_endpoints() -> TestResult {
        let candidate = [(1, 7), (1, 8)];
        let manual = [(90, 8), (12, 91)];
        let (document, solution, objective) =
            repair_fixture(&candidate, Some(&manual), &candidate)?;
        let data = repair_page(
            &document,
            &solution,
            Some(&objective),
            "repairDiff",
            100,
            None,
        )?;
        let rows = data["result"]["items"].as_array().ok_or("missing rows")?;
        assert_eq!(
            data["result"]["counts"],
            json!({"removedPairs":2,"addedPairs":2,"unchangedPairs":0})
        );
        assert_eq!(
            rows[1]["oldMembers"][0],
            json!({
                "personId":id(90), "personName":null, "missingPerson":true
            })
        );
        assert_eq!(rows[2]["shiftId"], id(91));
        assert_eq!(rows[2]["shiftLabel"], Value::Null);
        assert_eq!(rows[2]["missingShift"], true);
        assert_eq!(rows[2]["oldMembers"][0]["personId"], id(12));
        assert_eq!(rows[2]["afterMemberCount"], 0);
        Ok(())
    }

    #[test]
    fn recorded_repair_diff_original_and_absent_baselines_are_distinct() -> TestResult {
        let (document, solution, objective) = repair_fixture(&[(1, 7)], None, &[(1, 8)])?;
        let available = repair_page(
            &document,
            &solution,
            Some(&objective),
            "repairDiff",
            100,
            None,
        )?;
        assert_eq!(available["result"]["availability"], "available");
        assert_eq!(
            available["result"]["baseline"]["selection"]["kind"],
            "originalAccepted"
        );
        assert_eq!(
            available["result"]["counts"],
            json!({"removedPairs":1,"addedPairs":1,"unchangedPairs":0})
        );
        let absent = repair_page(&document, &solution, None, "repairDiff", 100, None)?;
        assert_eq!(absent["result"]["availability"], "absent");
        for field in ["baseline", "counts", "totalItems", "continuation"] {
            assert_eq!(absent["result"][field], Value::Null);
        }
        assert_eq!(absent["result"]["items"], json!([]));
        let (document, solution, objective) = repair_fixture(&[], Some(&[]), &[])?;
        let empty = repair_page(
            &document,
            &solution,
            Some(&objective),
            "repairDiff",
            100,
            None,
        )?;
        assert_eq!(empty["result"]["availability"], "available");
        assert_eq!(empty["result"]["totalItems"], 0);
        assert_eq!(
            empty["result"]["baseline"]["selection"]["kind"],
            "manualSelection"
        );
        Ok(())
    }

    #[test]
    fn recorded_repair_diff_charges_false_decision_census_and_preallocation() -> TestResult {
        let (_, mut solution, _) = repair_fixture(&[], None, &[(1, 7), (1, 8)])?;
        solution.assignments[0].value = AssignmentValue::Boolean(false);
        let control = OperationControl::Cancellation(CancellationToken::new());
        let mut budget = OperationBudget::evaluation(Some(&control));
        let before = budget.remaining_output();
        budget.cancel_after_steps(2)?;
        assert!(matches!(
            repair_diff_index(&[], &solution, &mut budget),
            Err(DomainPackError::Cancelled)
        ));
        assert_eq!(before, budget.remaining_output());
        solution.assignments[0].value = AssignmentValue::Boolean(true);
        let mut budget = OperationBudget::evaluation(None);
        budget.reserve(0, 0, 16 * 1024 * 1024 - 63)?;
        assert!(matches!(
            repair_diff_index(&[], &solution, &mut budget),
            Err(DomainPackError::ResourceLimitExceeded)
        ));
        Ok(())
    }

    #[test]
    fn recorded_repair_diff_response_cap_does_not_split_one_shift_group() -> TestResult {
        let (document, solution, objective) = repair_fixture(&[(1, 7)], None, &[(1, 7)])?;
        let control = OperationControl::Cancellation(CancellationToken::new());
        let mut budget = OperationBudget::evaluation(Some(&control));
        let baseline = repair_diff_baseline(
            objective.baseline.as_deref().ok_or("missing baseline")?,
            &mut budget,
        )?;
        let source = RepairDiffSource {
            scenario_id: document.scenario_id,
            source_revision: Revision::INITIAL,
            solution_id: solution.solution_id,
            normalized_solution_hash: solution.canonical_hash()?,
            source_document_hash: "b".repeat(64),
        };
        assert!(matches!(
            fit_page(
                vec![(
                    json!({"largeHistoricalLabel":"x".repeat(MAX_DATA_BYTES)}),
                    ResultPosition::RepairDiff {
                        shift_id: id(7).parse()?
                    }
                )],
                1,
                false,
                document.scenario_id,
                repair_test_context(),
                &document.settings.time_zone,
                PageKind::RepairDiff {
                    source: &source,
                    baseline: &baseline,
                    counts: RepairDiffCounts::default(),
                    control: &control
                },
            ),
            Err(DomainPackError::ResourceLimitExceeded)
        ));
        Ok(())
    }

    #[test]
    fn no_applicable_health_charges_shared_retention_and_cancellation()
    -> Result<(), Box<dyn std::error::Error>> {
        let owner: EntityId = "01900000-0000-7000-8000-000000000030".parse()?;
        let minimum = json!({
            "qualifications":{
                "allQualificationIds":["01900000-0000-7000-8000-000000000031"],
                "anyQualificationIds":[]
            },
            "minimum":1
        });
        let mut domain = WorkforceDomainV1::default();
        domain.entities.insert(
            owner,
            serde_json::from_value(json!({
                "kind":"coverageRequirement",
                "id":owner,
                "active":true,
                "scope":{
                    "kind":"filter",
                    "assignmentTypeIds":["01900000-0000-7000-8000-000000000032"]
                },
                "coverage":{
                    "kind":"atLeast",
                    "minimum":1,
                    "qualificationMinimums":vec![minimum;16]
                }
            }))?,
        );

        let mut budget = OperationBudget::evaluation(None);
        budget.reserve(0, 0, 16 * 1024 * 1024 - 8 * 192)?;
        let mut observations = Vec::new();
        assert!(matches!(
            append_health_no_applicable(&domain, &mut observations, &mut budget),
            Err(DomainPackError::ResourceLimitExceeded)
        ));
        assert_eq!(observations.len(), 8);

        let cancellation = CancellationToken::new();
        let control = OperationControl::Cancellation(cancellation);
        let mut budget = OperationBudget::evaluation(Some(&control));
        budget.cancel_after_steps(5)?;
        let mut observations = Vec::new();
        assert!(matches!(
            append_health_no_applicable(&domain, &mut observations, &mut budget),
            Err(DomainPackError::Cancelled)
        ));
        assert!(observations.len() < 17);
        Ok(())
    }

    #[test]
    fn captured_original_repair_diff_preserves_deleted_endpoints_and_source_hash() -> TestResult {
        let (mut document, solution, mut objective) =
            repair_fixture(&[(1, 8), (12, 16)], None, &[(1, 8)])?;
        let mut raw = document
            .domain
            .entities
            .remove(&id(40).parse()?)
            .ok_or("base")?;
        let binding = objective.baseline.as_mut().ok_or("baseline")?;
        let entity_id = EntityId::from_uuid(binding.original.accepted_result.solution_id.as_uuid());
        raw["id"] = json!(entity_id);
        binding.original.entity = workforce_baseline_reference(entity_id, false)?;
        binding.captured_original = Some(std::sync::Arc::new(raw));
        objective.schema_version = 2;
        document.domain.entities.remove(&id(12).parse()?);
        document.domain.entities.remove(&id(16).parse()?);
        let before = serde_json::to_vec(&document)?;
        let page = repair_page(
            &document,
            &solution,
            Some(&objective),
            "repairDiff",
            100,
            None,
        )?;
        let result = &page["result"];
        assert_eq!(
            result["counts"],
            json!({"removedPairs":1,"addedPairs":0,"unchangedPairs":1})
        );
        assert_eq!(result["totalItems"], 2);
        assert_eq!(
            result["source"]["sourceDocumentHash"],
            eutheto_domain_ir::blake3_hex(&before)
        );
        assert_eq!(result["baseline"]["selection"]["kind"], "originalAccepted");
        assert_eq!(result["baseline"]["verificationStatus"], "notEstablished");
        assert!(result["baseline"].get("capturedOriginal").is_none());
        let removed = &result["items"][1];
        assert_eq!(removed["shiftId"], id(16));
        assert_eq!(removed["missingShift"], true);
        assert_eq!(removed["oldMembers"][0]["personId"], id(12));
        assert_eq!(removed["oldMembers"][0]["missingPerson"], true);
        assert_eq!(removed["change"], "newlyUnfilled");
        let base_diff = repair_page(
            &document,
            &solution,
            Some(&objective),
            "baseDiff",
            100,
            None,
        )?;
        assert!(base_diff["result"]["baseSchedule"].is_null());
        assert_eq!(serde_json::to_vec(&document)?, before);
        Ok(())
    }
}
