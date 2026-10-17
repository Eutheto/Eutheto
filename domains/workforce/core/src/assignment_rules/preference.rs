use super::{
    AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleLimit, InstantInterval,
    analysis::support,
    budget::{OperationBudget, count, within},
    identity::{
        ADJACENCY_PREFERENCE_CATEGORY, ASSIGNMENT_TARGET_PREFERENCE_CATEGORY,
        ASSIGNMENT_TYPE_PREFERENCE_CATEGORY, BASE_STABILITY_PREFERENCE_CATEGORY, IdentityKind,
        LOCATION_PREFERENCE_CATEGORY, PREFERRED_COVERAGE_PREFERENCE_CATEGORY, PlanningIdentities,
        REQUESTED_TIME_OFF_PREFERENCE_CATEGORY, TIME_PREFERENCE_CATEGORY,
        TOGETHER_SEPARATE_PREFERENCE_CATEGORY, WEEKEND_TARGET_PREFERENCE_CATEGORY,
        WORKLOAD_BALANCE_PREFERENCE_CATEGORY,
    },
    input::AssignmentInput,
    intervals::availability_intervals,
    preference_consecutive,
    profile::{self, Layout, PreferenceLevel},
    reporting::{ReportingPeriod, ReportingWindows},
    weekends::{self, WorkedWeekend},
};
use crate::{
    model::{
        AdjacencyMode, AssignmentPair, AvailabilityKind, Coverage, DeviationPenalty,
        GroupDirection, OverlappingContribution, PairingMode, PeerGroup, PersonSelection,
        PreferenceDirection, PreferencePriority, Scope, ShiftScope, TimeWindow, WindowMembership,
        WorkforceEntity, WorkforcePreference, WorkforceRule, WorkforceScorePolicy,
        WorkloadMeasurement, WorkloadPolicy, WorkloadTargetMode,
    },
    temporal::{
        ResolvedShift, potentially_intersects, resolve_calendar_for_calendar, resolve_interval,
        weekday,
    },
};
use eutheto_domain_ir::{
    DomainEntityId, DomainEntityKindId, DomainEntityRef, RunObjectiveContext,
    RunObjectiveOperation, ScoreCategoryId,
};
use eutheto_planning_ir::{
    BoolVariable, BoolVariableId, ComparisonOp, Constraint, ConstraintRecord, InclusiveRange,
    IntDomain, IntVariable, IntVariableId, LinearComparison, LinearExpression, LinearTerm, Literal,
    ObjectiveTerm, ObjectiveTermId, ObjectiveTermKind, PlanningConstraintId, PlanningIrLimitsV1,
    ProvenanceId, ProvenanceParameter, ProvenanceRecord, ProvenanceSourceKind, Variable,
};
use eutheto_types::{EntityId, Horizon, PersonId, RuleId, ScenarioDocument, ScenarioSettings};
use jiff::{
    Span,
    civil::Date,
    tz::{Offset, TimeZone},
};
use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    mem::size_of,
};

const NANOS_PER_MINUTE: i64 = 60_000_000_000;

/// Stable explanatory categories for the supported headless preference variants.
pub(super) const TIME_CATEGORY: &str = TIME_PREFERENCE_CATEGORY;
pub(super) const ASSIGNMENT_TYPE_CATEGORY: &str = ASSIGNMENT_TYPE_PREFERENCE_CATEGORY;
pub(super) const LOCATION_CATEGORY: &str = LOCATION_PREFERENCE_CATEGORY;
pub(super) const REQUESTED_TIME_OFF_CATEGORY: &str = REQUESTED_TIME_OFF_PREFERENCE_CATEGORY;
pub(super) const ASSIGNMENT_TARGET_CATEGORY: &str = ASSIGNMENT_TARGET_PREFERENCE_CATEGORY;
pub(super) const WEEKEND_TARGET_CATEGORY: &str = WEEKEND_TARGET_PREFERENCE_CATEGORY;
pub(super) const CONSECUTIVE_WORK_CATEGORY: &str = preference_consecutive::CATEGORY;
pub(super) const WORKLOAD_BALANCE_CATEGORY: &str = WORKLOAD_BALANCE_PREFERENCE_CATEGORY;
pub(super) const ADJACENCY_CATEGORY: &str = ADJACENCY_PREFERENCE_CATEGORY;
pub(super) const BASE_STABILITY_CATEGORY: &str = BASE_STABILITY_PREFERENCE_CATEGORY;
pub(super) const TOGETHER_SEPARATE_CATEGORY: &str = TOGETHER_SEPARATE_PREFERENCE_CATEGORY;
pub(super) const PREFERRED_COVERAGE_CATEGORY: &str = PREFERRED_COVERAGE_PREFERENCE_CATEGORY;

pub(super) fn requirement_shift_matches(
    scope: &ShiftScope,
    shift: &ResolvedShift,
    metadata: &super::input::ShiftMetadata<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    match scope {
        ShiftScope::All {} => Ok(true),
        ShiftScope::Selected { shift_ids } => {
            budget.steps(count(shift_ids.len())?)?;
            Ok(shift_ids.contains(&shift.id))
        }
        ShiftScope::Filter {
            assignment_type_ids,
            start_date_range,
            location_ids,
        } => {
            let date = shift.interval.starts_at.local.as_datetime().date();
            let date_matches = start_date_range
                .as_ref()
                .is_none_or(|range| range.start_date <= date && date < range.end_date_exclusive);
            let assignment_type_matches = assignment_type_ids
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|id| id == &metadata.assignment_type.id));
            let location_matches = location_ids
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|id| Some(*id) == metadata.location_id));
            budget.steps(
                count(assignment_type_ids.as_ref().map_or(0, Vec::len))?
                    .checked_add(count(location_ids.as_ref().map_or(0, Vec::len))?)
                    .ok_or_else(arithmetic)?,
            )?;
            Ok(date_matches && assignment_type_matches && location_matches)
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum PreferenceVariant {
    Time,
    AssignmentType,
    Location,
    RequestedTimeOff,
    Adjacency,
    BaseStability,
}

#[derive(Clone, Copy)]
pub(super) struct PreferenceEntry<'a> {
    pub id: RuleId,
    pub preference: &'a WorkforcePreference,
    pub level: usize,
    pub coefficient: i64,
    pub direction: Option<PreferenceDirection>,
    pub variant: PreferenceVariant,
    /// Source-owned base assignments used to charge the scoped symmetric difference.
    pub base_assignments: Option<&'a [AssignmentPair]>,
}

#[derive(Clone, Copy)]
pub(super) struct GroupPlan<'a> {
    pub id: RuleId,
    pub level: usize,
    pub coefficient: i64,
    pub scope: &'a Scope,
    pub direction: GroupDirection,
    pub person_ids: &'a [PersonId],
    pub mode: PairingMode,
}

#[derive(Clone, Copy)]
pub(super) struct CoveragePlan<'a> {
    pub id: RuleId,
    pub level: usize,
    pub coefficient: i64,
    pub scope: &'a Scope,
    pub coverage_requirement_ids: &'a [crate::ids::CoverageRequirementId],
}

#[derive(Clone, Debug)]
pub(super) struct TargetPeriod {
    pub person: PersonId,
    pub period: ReportingPeriod,
    /// Every source shift matching the authored target scope, including statically infeasible
    /// pairs. The source score intentionally does not consume the compiler candidate graph.
    pub pairs: Vec<AssignmentPair>,
}

#[derive(Clone)]
pub(super) struct PreferenceTarget {
    pub id: RuleId,
    pub level: usize,
    pub coefficient: i64,
    pub target_count: u32,
    pub periods: Vec<TargetPeriod>,
}
#[derive(Clone, Debug)]
pub(super) struct WeekendTargetWeekend {
    pub date: Date,
    /// Every source pair matching this preference's person and shift scope. The compiler later
    /// intersects this source population with candidate pairs.
    pub pairs: Vec<AssignmentPair>,
}

#[derive(Clone, Debug)]
pub(super) struct WeekendTargetPerson {
    pub person: PersonId,
    pub weekends: Vec<WeekendTargetWeekend>,
}

#[derive(Clone, Debug)]
pub(super) struct WeekendTargetPlan {
    pub id: RuleId,
    pub level: usize,
    pub coefficient: i64,
    pub target_count: u32,
    pub people: Vec<WeekendTargetPerson>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WorkloadTargetKind {
    Explicit,
    PersonTargets,
    WeightedEqualShare,
    EqualShare,
}

#[derive(Clone, Debug)]
pub(super) struct WorkloadTargetPlan {
    pub kind: WorkloadTargetKind,
    /// The denominator of the exact target rational, after reducing common factors.
    pub denominator: i64,
    pub targets: BTreeMap<PersonId, i64>,
    pub weights: BTreeMap<PersonId, i64>,
    pub peers: Vec<PersonId>,
}

#[derive(Clone, Debug)]
pub(super) struct WorkloadPeerPeriod {
    pub person: PersonId,
    /// Every source shift matching the preference scope and bucket. The compiler later
    /// intersects this source population with its own candidate vector.
    pub pairs: Vec<AssignmentPair>,
}

#[derive(Clone, Debug)]
pub(super) struct WorkloadPeriod {
    pub period: ReportingPeriod,
    pub peers: Vec<WorkloadPeerPeriod>,
}

#[derive(Clone)]
pub(super) struct WorkloadPlan<'a> {
    pub id: RuleId,
    pub level: usize,
    pub coefficient: i64,
    /// The greatest exact common quantum of all source contributions; no fractional
    /// elapsed or signed scheduled workload is rounded.
    pub nanoseconds_per_unit: i64,
    pub units_per_target_unit: i64,
    /// Exact rational target denominator in the native integer bucket units.
    pub score_denominator: i64,
    pub policy: &'a WorkloadPolicy,
    pub target: WorkloadTargetPlan,
    pub periods: Vec<WorkloadPeriod>,
}

pub(super) struct PreferencePlan<'a> {
    pub levels: Vec<PreferenceLevel>,
    pub layout: Layout,
    pub entries: Vec<PreferenceEntry<'a>>,
    pub targets: Vec<PreferenceTarget>,
    pub weekend_targets: Vec<WeekendTargetPlan>,
    pub workloads: Vec<WorkloadPlan<'a>>,
    pub consecutives: Vec<preference_consecutive::Plan>,
    pub groups: Vec<GroupPlan<'a>>,
    pub coverage: Vec<CoveragePlan<'a>>,
}

impl PreferencePlan<'_> {
    pub fn active(&self) -> bool {
        !self.entries.is_empty()
            || !self.targets.is_empty()
            || !self.workloads.is_empty()
            || !self.consecutives.is_empty()
            || !self.groups.is_empty()
            || !self.coverage.is_empty()
            || !self.weekend_targets.is_empty()
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PrunedBaseShape {
    pub entry: usize,
    pub pair: AssignmentPair,
}

#[derive(Clone, Debug)]
pub(super) struct TargetShape {
    pub target: usize,
    pub period: usize,
    pub population: Vec<usize>,
    pub difference_upper: i64,
}
#[derive(Clone, Debug)]
pub(super) struct WeekendTargetWeekendShape {
    pub date: Date,
    pub population: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct WeekendTargetShape {
    pub target: usize,
    pub person: usize,
    pub weekends: Vec<WeekendTargetWeekendShape>,
    pub difference_upper: i64,
}

#[derive(Clone, Debug)]
pub(super) struct WorkloadSegmentShape {
    pub units: i64,
    pub population: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct WorkloadPeerShape {
    pub person: PersonId,
    pub terms: Vec<(usize, i64)>,
    pub segments: Vec<WorkloadSegmentShape>,
    /// Whole authored minutes and residual exact units when the quantum is finer
    /// than one minute; no segment contributes an out-of-bounds IR coefficient.
    pub split_bounds: Option<(i64, i64, i64)>,
    pub actual_lower: i64,
    pub actual_upper: i64,
    pub delta_lower: i64,
    pub delta_upper: i64,
    pub difference_upper: i64,
    pub cost_upper: i64,
}

#[derive(Clone, Debug)]
pub(super) struct WorkloadPeriodShape {
    pub workload: usize,
    pub period: usize,
    pub peers: Vec<WorkloadPeerShape>,
}

#[derive(Clone, Debug)]
pub(super) struct WorkloadShape {
    pub periods: Vec<WorkloadPeriodShape>,
}

#[derive(Clone, Debug)]
pub(super) struct GroupPairShape {
    pub first: usize,
    pub second: usize,
}

#[derive(Clone, Debug)]
pub(super) struct GroupShape {
    pub plan: usize,
    pub pairs: Vec<GroupPairShape>,
}

#[derive(Clone, Debug)]
pub(super) struct CoverageShape {
    pub plan: usize,
    pub requirement_id: crate::ids::CoverageRequirementId,
    pub shift_id: crate::ids::ShiftId,
    pub population: Vec<usize>,
    pub preferred_count: u16,
    pub difference_upper: i64,
}

#[derive(Clone, Debug)]
pub(super) struct PreferenceShape {
    pub term_count: u64,
    pub pruned_base: Vec<PrunedBaseShape>,
    pub level_lower_bounds: Vec<i64>,
    pub level_upper_bounds: Vec<i64>,
    pub targets: Vec<TargetShape>,
    pub weekend_targets: Vec<WeekendTargetShape>,
    pub workloads: WorkloadShape,
    pub consecutives: preference_consecutive::Shape,
    pub groups: Vec<GroupShape>,
    pub coverage: Vec<CoverageShape>,
}

#[derive(Clone, Debug)]
pub(super) struct PreferenceLevelScore {
    pub value: i64,
    pub time: i64,
    pub assignment_type: i64,
    pub location: i64,
    pub requested_time_off: i64,
    pub assignment_target: i64,
    pub weekend_target: i64,
    pub workload_balance: i64,
    pub consecutive_work: i64,
    pub adjacency: i64,
    pub base_stability: i64,
    pub together_separate: i64,
    pub preferred_coverage: i64,
}

pub(super) struct PreferenceScore {
    pub levels: Vec<PreferenceLevelScore>,
}

/// Build the active supported preference subset once. Inactive and later variants are intentionally
/// absent; their active forms are rejected by the complete compiler and source authority gates.
#[allow(clippy::too_many_lines)]
pub(super) fn prepare<'a>(
    document: &ScenarioDocument,
    input: &'a AssignmentInput,
    objective_context: Option<&RunObjectiveContext>,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<PreferencePlan<'a>, AssignmentRuleError> {
    let mut active_supported = false;
    for preference in input.domain.preferences.values() {
        budget.step()?;
        let (_, active, ..) = preference.header();
        active_supported |= active
            && (matches!(
                preference,
                WorkforcePreference::Time { .. }
                    | WorkforcePreference::AssignmentType { .. }
                    | WorkforcePreference::Location { .. }
                    | WorkforcePreference::RequestedTimeOff { .. }
                    | WorkforcePreference::AssignmentTarget { .. }
                    | WorkforcePreference::WeekendTarget { .. }
                    | WorkforcePreference::WorkloadBalance { .. }
                    | WorkforcePreference::Adjacency { .. }
                    | WorkforcePreference::BaseStability { .. }
                    | WorkforcePreference::TogetherSeparate { .. }
                    | WorkforcePreference::PreferredCoverage { .. }
            ) || matches!(
                preference,
                WorkforcePreference::ConsecutiveWork { mode, .. }
                    if preference_consecutive::supports(mode)
            ));
    }
    let kind = profile::ProfileKind::parse(objective_context, document.scenario_id)
        .map_err(AssignmentRuleError::InvalidDocument)?;
    if !active_supported && !kind.is_some_and(profile::ProfileKind::named) {
        return Ok(PreferencePlan {
            levels: Vec::new(),
            layout: Layout::empty(kind),
            entries: Vec::new(),
            targets: Vec::new(),
            weekend_targets: Vec::new(),
            workloads: Vec::new(),
            consecutives: Vec::new(),
            groups: Vec::new(),
            coverage: Vec::new(),
        });
    }
    let policy = match score_policy(input) {
        Ok(policy) => policy,
        Err(AssignmentRuleError::MissingScorePolicy)
            if !active_supported && kind.is_some_and(profile::ProfileKind::named) =>
        {
            let kind = kind.ok_or_else(invalid)?;
            let operation = objective_context
                .map_or(RunObjectiveOperation::Optimize, |context| context.operation);
            let layout = Layout::without_policy(kind, operation, limits, budget)?;
            return Ok(PreferencePlan {
                levels: Vec::new(),
                layout,
                entries: Vec::new(),
                targets: Vec::new(),
                weekend_targets: Vec::new(),
                workloads: Vec::new(),
                consecutives: Vec::new(),
                groups: Vec::new(),
                coverage: Vec::new(),
            });
        }
        Err(error) => return Err(error),
    };
    let layout = Layout::new(
        policy,
        objective_context,
        document.scenario_id,
        active_supported,
        limits,
        budget,
    )?;
    if !active_supported {
        return Ok(PreferencePlan {
            levels: layout.levels.clone(),
            layout,
            entries: Vec::new(),
            targets: Vec::new(),
            weekend_targets: Vec::new(),
            workloads: Vec::new(),
            consecutives: Vec::new(),
            groups: Vec::new(),
            coverage: Vec::new(),
        });
    }
    let mut entries = Vec::new();
    let mut targets = Vec::new();
    let mut weekend_targets = Vec::new();
    let mut source_weekends = BTreeMap::<crate::ids::ShiftId, Vec<WorkedWeekend>>::new();
    let mut workloads = Vec::new();
    let mut consecutives = Vec::new();
    let mut groups = Vec::new();
    let mut coverage = Vec::new();
    for preference in input.domain.preferences.values() {
        budget.step()?;
        let (id, active, scope, priority, weight) = preference.header();
        if !active {
            continue;
        }
        let (level, scale) = mapping(policy, priority)?;
        let coefficient = u64::from(weight)
            .checked_mul(u64::from(scale))
            .ok_or_else(arithmetic)?;
        within(
            coefficient,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        let coefficient = i64::try_from(coefficient).map_err(|_| arithmetic())?;
        match preference {
            WorkforcePreference::Time { direction, .. } => entries.push(PreferenceEntry {
                id,
                preference,
                level,
                coefficient,
                direction: Some(*direction),
                variant: PreferenceVariant::Time,
                base_assignments: None,
            }),
            WorkforcePreference::AssignmentType { direction, .. } => {
                entries.push(PreferenceEntry {
                    id,
                    preference,
                    level,
                    coefficient,
                    direction: Some(*direction),
                    variant: PreferenceVariant::AssignmentType,
                    base_assignments: None,
                });
            }
            WorkforcePreference::Location { direction, .. } => entries.push(PreferenceEntry {
                id,
                preference,
                level,
                coefficient,
                direction: Some(*direction),
                variant: PreferenceVariant::Location,
                base_assignments: None,
            }),
            WorkforcePreference::RequestedTimeOff { .. } => entries.push(PreferenceEntry {
                id,
                preference,
                level,
                coefficient,
                direction: None,
                variant: PreferenceVariant::RequestedTimeOff,
                base_assignments: None,
            }),
            WorkforcePreference::Adjacency { direction, .. } => {
                entries.push(PreferenceEntry {
                    id,
                    preference,
                    level,
                    coefficient,
                    direction: Some(*direction),
                    variant: PreferenceVariant::Adjacency,
                    base_assignments: None,
                });
            }
            WorkforcePreference::BaseStability {
                base_schedule_id, ..
            } => {
                let Some(WorkforceEntity::BaseSchedule(base)) =
                    input.domain.entities.get(base_schedule_id)
                else {
                    return Err(invalid());
                };
                entries.push(PreferenceEntry {
                    id,
                    preference,
                    level,
                    coefficient,
                    direction: None,
                    variant: PreferenceVariant::BaseStability,
                    base_assignments: Some(base.assignments.as_slice()),
                });
            }
            WorkforcePreference::TogetherSeparate {
                direction,
                person_ids,
                mode,
                ..
            } => groups.push(GroupPlan {
                id,
                level,
                coefficient,
                scope,
                direction: *direction,
                person_ids,
                mode: *mode,
            }),
            WorkforcePreference::PreferredCoverage {
                coverage_requirement_ids,
                ..
            } => coverage.push(CoveragePlan {
                id,
                level,
                coefficient,
                scope,
                coverage_requirement_ids,
            }),
            WorkforcePreference::AssignmentTarget {
                calendar_id,
                target_count,
                ..
            } => {
                within(
                    u64::from(*target_count),
                    u64::try_from(limits.max_abs_value).map_err(|_| arithmetic())?,
                    AssignmentRuleLimit::PerRecord,
                )?;
                targets.push(PreferenceTarget {
                    id,
                    level,
                    coefficient,
                    target_count: *target_count,
                    periods: target_periods(
                        document,
                        input,
                        scope,
                        *calendar_id,
                        *target_count,
                        budget,
                    )?,
                });
            }
            WorkforcePreference::WeekendTarget { target_count, .. } => {
                within(
                    u64::from(*target_count),
                    u64::try_from(limits.max_abs_value).map_err(|_| arithmetic())?,
                    AssignmentRuleLimit::PerRecord,
                )?;
                weekend_targets.push(WeekendTargetPlan {
                    id,
                    level,
                    coefficient,
                    target_count: *target_count,
                    people: weekend_target_people(
                        input,
                        scope,
                        &document.settings,
                        &mut source_weekends,
                        budget,
                    )?,
                });
            }
            WorkforcePreference::WorkloadBalance {
                workload_policy_id, ..
            } => {
                let workload_policy = policy
                    .workload_policies
                    .get(workload_policy_id)
                    .ok_or_else(invalid)?;
                workloads.push(workload_plan(
                    document,
                    input,
                    scope,
                    workload_policy,
                    id,
                    level,
                    coefficient,
                    budget,
                )?);
            }
            WorkforcePreference::ConsecutiveWork {
                mode,
                assignment_type_ids,
                threshold,
                ..
            } => {
                if let Some(plan) = preference_consecutive::plan(
                    document,
                    input,
                    preference,
                    id,
                    level,
                    coefficient,
                    scope,
                    mode,
                    assignment_type_ids,
                    *threshold,
                    limits,
                    budget,
                )? {
                    consecutives.push(plan);
                }
            }
        }
    }
    for workload in &workloads {
        for target in workload.target.targets.values() {
            within(
                u64::try_from(*target).map_err(|_| arithmetic())?,
                u64::try_from(limits.max_abs_value).map_err(|_| arithmetic())?,
                AssignmentRuleLimit::PerRecord,
            )?;
        }
    }
    for level in &layout.levels {
        within(
            count(level.id.len())?,
            limits.max_id_bytes,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    let mut level_scales = vec![1_i64; policy.levels.len()];
    for workload in &workloads {
        let scale = level_scales.get_mut(workload.level).ok_or_else(invalid)?;
        *scale = lcm_i64(*scale, workload.score_denominator)?;
    }
    for entry in &mut entries {
        let scale = *level_scales.get(entry.level).ok_or_else(invalid)?;
        entry.coefficient = entry
            .coefficient
            .checked_mul(scale)
            .ok_or_else(arithmetic)?;
        within(
            u64::try_from(entry.coefficient).map_err(|_| arithmetic())?,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for target in &mut targets {
        let scale = *level_scales.get(target.level).ok_or_else(invalid)?;
        target.coefficient = target
            .coefficient
            .checked_mul(scale)
            .ok_or_else(arithmetic)?;
        within(
            u64::try_from(target.coefficient).map_err(|_| arithmetic())?,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for weekend_target in &mut weekend_targets {
        let scale = *level_scales.get(weekend_target.level).ok_or_else(invalid)?;
        weekend_target.coefficient = weekend_target
            .coefficient
            .checked_mul(scale)
            .ok_or_else(arithmetic)?;
        within(
            u64::try_from(weekend_target.coefficient).map_err(|_| arithmetic())?,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for group in &mut groups {
        let scale = *level_scales.get(group.level).ok_or_else(invalid)?;
        group.coefficient = group
            .coefficient
            .checked_mul(scale)
            .ok_or_else(arithmetic)?;
        within(
            u64::try_from(group.coefficient).map_err(|_| arithmetic())?,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for coverage_plan in &mut coverage {
        let scale = *level_scales.get(coverage_plan.level).ok_or_else(invalid)?;
        coverage_plan.coefficient = coverage_plan
            .coefficient
            .checked_mul(scale)
            .ok_or_else(arithmetic)?;
        within(
            u64::try_from(coverage_plan.coefficient).map_err(|_| arithmetic())?,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for consecutive in &mut consecutives {
        let scale = *level_scales.get(consecutive.level).ok_or_else(invalid)?;
        consecutive.coefficient = consecutive
            .coefficient
            .checked_mul(scale)
            .ok_or_else(arithmetic)?;
        within(
            u64::try_from(consecutive.coefficient).map_err(|_| arithmetic())?,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for workload in &mut workloads {
        let level_scale = *level_scales.get(workload.level).ok_or_else(invalid)?;
        let scale = level_scale
            .checked_div(workload.score_denominator)
            .ok_or_else(arithmetic)?;
        workload.coefficient = workload
            .coefficient
            .checked_mul(scale)
            .ok_or_else(arithmetic)?;
        within(
            u64::try_from(workload.coefficient).map_err(|_| arithmetic())?,
            u64::try_from(limits.max_abs_coefficient).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for entry in &mut entries {
        entry.level = layout
            .preference_level(entry.level, false)
            .ok_or_else(invalid)?;
    }
    for target in &mut targets {
        target.level = layout
            .preference_level(target.level, false)
            .ok_or_else(invalid)?;
    }
    for target in &mut weekend_targets {
        target.level = layout
            .preference_level(target.level, false)
            .ok_or_else(invalid)?;
    }
    for workload in &mut workloads {
        workload.level = layout
            .preference_level(workload.level, true)
            .ok_or_else(invalid)?;
    }
    for consecutive in &mut consecutives {
        consecutive.level = layout
            .preference_level(consecutive.level, false)
            .ok_or_else(invalid)?;
    }
    for group in &mut groups {
        group.level = layout
            .preference_level(group.level, false)
            .ok_or_else(invalid)?;
    }
    for coverage_plan in &mut coverage {
        coverage_plan.level = layout
            .preference_level(coverage_plan.level, false)
            .ok_or_else(invalid)?;
    }
    within(
        count(
            layout
                .prefix
                .len()
                .checked_add(layout.levels.len())
                .ok_or_else(arithmetic)?,
        )?
        .checked_add(1)
        .ok_or_else(arithmetic)?,
        limits.max_objective_levels,
        AssignmentRuleLimit::PerRecord,
    )?;
    Ok(PreferencePlan {
        levels: layout.levels.clone(),
        layout,
        entries,
        targets,
        weekend_targets,
        workloads,
        consecutives,
        groups,
        coverage,
    })
}

/// Build each scoped person's distinct local worked weekends from source shifts before any
/// candidate intersection. A scoped person with no matching source shift is retained so a positive
/// target still has its authoritative constant shortfall.
fn weekend_target_people(
    input: &AssignmentInput,
    scope: &Scope,
    settings: &ScenarioSettings,
    source_weekends: &mut BTreeMap<crate::ids::ShiftId, Vec<WorkedWeekend>>,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<WeekendTargetPerson>, AssignmentRuleError> {
    let mut people = Vec::new();
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !support::person_scope(scope, person, budget)? {
            continue;
        }
        let mut grouped = BTreeMap::<Date, Vec<AssignmentPair>>::new();
        for shift in &input.shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !support::shift_scope(scope, shift, &metadata, budget)? {
                continue;
            }
            let windows = match source_weekends.entry(shift.id) {
                Entry::Occupied(existing) => existing.into_mut(),
                Entry::Vacant(empty) => {
                    let windows = weekends::for_shift(shift, settings, budget)?;
                    budget.reserve(1, 1, 64)?;
                    empty.insert(windows)
                }
            };
            for weekend in windows {
                budget.step()?;
                let entry = match grouped.entry(weekend.date) {
                    Entry::Occupied(existing) => existing.into_mut(),
                    Entry::Vacant(empty) => {
                        budget.reserve(0, 1, 64)?;
                        empty.insert(Vec::new())
                    }
                };
                budget.reserve(0, 1, 32)?;
                entry.push(AssignmentPair {
                    person_id: *person_id,
                    shift_id: shift.id,
                });
            }
        }
        let mut weekends = Vec::with_capacity(grouped.len());
        for (date, mut pairs) in grouped {
            budget.step()?;
            budget.sort_work(pairs.len())?;
            pairs.sort_unstable();
            budget.steps(count(pairs.len())?)?;
            pairs.dedup();
            budget.reserve(1, 1, budget.measure(&(date, pairs.len()))?)?;
            weekends.push(WeekendTargetWeekend { date, pairs });
        }
        budget.reserve(1, 1, budget.measure(&(person_id, weekends.len()))?)?;
        people.push(WeekendTargetPerson {
            person: *person_id,
            weekends,
        });
    }
    Ok(people)
}

fn target_periods(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    scope: &Scope,
    calendar_id: crate::ids::WorkCalendarId,
    target_count: u32,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<TargetPeriod>, AssignmentRuleError> {
    let mut windows = ReportingWindows::new();
    let mut grouped = BTreeMap::<(PersonId, ReportingPeriod), Vec<AssignmentPair>>::new();
    if target_count > 0 {
        let dates = crate::model::planning_dates(&document.settings).map_err(|_| invalid())?;
        let mut date = dates.first_date;
        loop {
            budget.step()?;
            if let Some(period) = windows.resolve(document, input, calendar_id, date, budget)? {
                for person_id in &input.people {
                    budget.step()?;
                    let person = input.person(*person_id).ok_or_else(invalid)?;
                    if !support::person_scope(scope, person, budget)? {
                        continue;
                    }
                    if let Entry::Vacant(empty) = grouped.entry((*person_id, period)) {
                        budget.reserve(0, 1, 64)?;
                        empty.insert(Vec::new());
                    }
                }
            }
            if date == dates.last_date {
                break;
            }
            date = date
                .checked_add(Span::new().days(1))
                .map_err(|_| invalid())?;
        }
    }
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !support::person_scope(scope, person, budget)? {
            continue;
        }
        for shift in &input.shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !support::shift_scope(scope, shift, &metadata, budget)? {
                continue;
            }
            let Some(period) =
                windows.resolve(document, input, calendar_id, shift.reporting_date, budget)?
            else {
                // A custom reporting date without an owning window represents no period.
                continue;
            };
            let pairs = match grouped.entry((*person_id, period)) {
                Entry::Occupied(existing) => existing.into_mut(),
                Entry::Vacant(empty) => {
                    budget.reserve(0, 1, 64)?;
                    empty.insert(Vec::new())
                }
            };
            budget.reserve(0, 1, 32)?;
            pairs.push(AssignmentPair {
                person_id: *person_id,
                shift_id: shift.id,
            });
        }
    }
    let mut periods = Vec::with_capacity(grouped.len());
    for ((person, period), mut pairs) in grouped {
        budget.step()?;
        budget.sort_work(pairs.len())?;
        pairs.sort_unstable();
        budget.reserve(1, 1, 128)?;
        periods.push(TargetPeriod {
            person,
            period,
            pairs,
        });
    }
    Ok(periods)
}

#[allow(clippy::too_many_arguments)]
fn workload_plan<'a>(
    document: &ScenarioDocument,
    input: &'a AssignmentInput,
    scope: &Scope,
    policy: &'a WorkloadPolicy,
    id: RuleId,
    level: usize,
    coefficient: i64,
    budget: &mut OperationBudget<'_>,
) -> Result<WorkloadPlan<'a>, AssignmentRuleError> {
    let Some(WorkforceEntity::WorkloadBucket(bucket)) =
        input.domain.entities.get(&policy.bucket_id.as_entity_id())
    else {
        return Err(invalid());
    };
    if (bucket.measurement == WorkloadMeasurement::AssignmentCount
        && bucket.overlapping_contribution == OverlappingContribution::Union)
        || (bucket.measurement == WorkloadMeasurement::ScheduledMinutes
            && (bucket.overlapping_contribution == OverlappingContribution::Union
                || policy.membership == WindowMembership::Intersection))
    {
        return Err(invalid());
    }
    let target = workload_targets(input, &policy.peer_group, policy, budget)?;
    let periods = workload_periods(document, input, scope, policy, &target.peers, budget)?;
    let nanoseconds_per_unit =
        workload_nanoseconds_per_unit(input, bucket, policy, &periods, budget)?;
    let units_per_target_unit = if bucket.measurement == WorkloadMeasurement::AssignmentCount {
        1
    } else {
        NANOS_PER_MINUTE / nanoseconds_per_unit
    };
    let score_denominator = target
        .denominator
        .checked_mul(units_per_target_unit)
        .ok_or(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::PerRecord,
        ))?;
    Ok(WorkloadPlan {
        id,
        level,
        coefficient,
        nanoseconds_per_unit,
        units_per_target_unit,
        score_denominator,
        policy,
        target,
        periods,
    })
}

fn workload_nanoseconds_per_unit(
    input: &AssignmentInput,
    bucket: &crate::model::WorkloadBucket,
    policy: &WorkloadPolicy,
    periods: &[WorkloadPeriod],
    budget: &mut OperationBudget<'_>,
) -> Result<i64, AssignmentRuleError> {
    if bucket.measurement == WorkloadMeasurement::AssignmentCount {
        return Ok(1);
    }
    let mut quantum = NANOS_PER_MINUTE;
    for period in periods {
        budget.step()?;
        for peer in &period.peers {
            let mut origin = None;
            for pair in &peer.pairs {
                budget.step()?;
                let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
                if bucket.overlapping_contribution == OverlappingContribution::Union {
                    if let Some((start, end)) =
                        workload_interval(shift, period.period, policy.membership)
                    {
                        let first = *origin.get_or_insert(start);
                        for instant in [start, end] {
                            let remainder = duration_nanos(first, instant)?
                                .rem_euclid(i128::from(NANOS_PER_MINUTE));
                            quantum = gcd_i64(
                                quantum,
                                i64::try_from(remainder).map_err(|_| arithmetic())?,
                            );
                        }
                    }
                } else {
                    let amount = workload_amount(shift, period.period, policy.membership, bucket)?;
                    quantum = gcd_i64(quantum, amount.rem_euclid(NANOS_PER_MINUTE));
                }
            }
        }
    }
    Ok(quantum)
}

fn workload_targets(
    input: &AssignmentInput,
    group: &PeerGroup,
    policy: &WorkloadPolicy,
    budget: &mut OperationBudget<'_>,
) -> Result<WorkloadTargetPlan, AssignmentRuleError> {
    let mut peers = Vec::new();
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if peer_matches(group, person, budget)? {
            peers.push(*person_id);
        }
    }
    if peers.is_empty() {
        return Err(invalid());
    }
    let mut targets = BTreeMap::new();
    let mut weights = BTreeMap::new();
    let (kind, denominator) = match &policy.target_mode {
        WorkloadTargetMode::Explicit { targets: values } => {
            for value in values {
                budget.step()?;
                targets.insert(value.person_id, i64::from(value.target));
            }
            if values.len() != peers.len() || peers.iter().any(|id| !targets.contains_key(id)) {
                return Err(invalid());
            }
            (WorkloadTargetKind::Explicit, 1)
        }
        WorkloadTargetMode::PersonTargets {} => {
            for person_id in &peers {
                budget.step()?;
                let person = input.person(*person_id).ok_or_else(invalid)?;
                let Some(target) = person.workload_target else {
                    return Err(invalid());
                };
                if target.bucket_id != policy.bucket_id
                    || target.calendar_id != policy.calendar_id
                    || target.membership != policy.membership
                {
                    return Err(invalid());
                }
                targets.insert(*person_id, i64::from(target.target));
            }
            (WorkloadTargetKind::PersonTargets, 1)
        }
        WorkloadTargetMode::EqualShare {} => (
            WorkloadTargetKind::EqualShare,
            i64::try_from(peers.len()).map_err(|_| arithmetic())?,
        ),
        WorkloadTargetMode::WeightedEqualShare {} => {
            let mut common = 1_u128;
            for person_id in &peers {
                budget.step()?;
                let person = input.person(*person_id).ok_or_else(invalid)?;
                let denominator = u128::from(person.workload_weight.denominator);
                if denominator == 0 {
                    return Err(invalid());
                }
                common = lcm_u128(common, denominator)?;
            }
            for person_id in &peers {
                budget.step()?;
                let person = input.person(*person_id).ok_or_else(invalid)?;
                let factor = common
                    .checked_div(u128::from(person.workload_weight.denominator))
                    .ok_or_else(arithmetic)?;
                let value = u128::from(person.workload_weight.numerator)
                    .checked_mul(factor)
                    .ok_or_else(arithmetic)?;
                weights.insert(*person_id, i64::try_from(value).map_err(|_| arithmetic())?);
            }
            let common_factor = weights.values().copied().fold(0_i64, gcd_i64);
            if common_factor > 1 {
                for value in weights.values_mut() {
                    *value = value.checked_div(common_factor).ok_or_else(arithmetic)?;
                }
            }
            let denominator = weights.values().try_fold(0_i64, |sum, value| {
                sum.checked_add(*value).ok_or_else(arithmetic)
            })?;
            if denominator <= 0 {
                return Err(invalid());
            }
            (WorkloadTargetKind::WeightedEqualShare, denominator)
        }
    };
    Ok(WorkloadTargetPlan {
        kind,
        denominator,
        targets,
        weights,
        peers,
    })
}

fn peer_matches(
    group: &PeerGroup,
    person: &crate::model::Person,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let selected = match &group.people {
        PersonSelection::All {} => true,
        PersonSelection::Selected { person_ids } => {
            budget.steps(count(person_ids.len())?)?;
            person_ids.contains(&person.id)
        }
        PersonSelection::Filter { all_tags, any_tags } => {
            let mut all = true;
            let mut any = any_tags.is_empty();
            for tag in all_tags {
                budget.step()?;
                budget.steps(count(person.tags.len())?)?;
                all &= person.tags.contains(tag);
            }
            for tag in any_tags {
                budget.step()?;
                budget.steps(count(person.tags.len())?)?;
                any |= person.tags.contains(tag);
            }
            all && any
        }
    };
    let team = group.team_ids.as_ref().is_none_or(|team_ids| {
        team_ids
            .iter()
            .any(|team_id| person.team_ids.contains(team_id))
    });
    if let Some(team_ids) = &group.team_ids {
        budget.steps(count(team_ids.len())?)?;
        budget.steps(count(person.team_ids.len())?)?;
    }
    Ok(selected && team)
}
fn insert_workload_period(
    periods: &mut Vec<WorkloadPeriod>,
    positions: &mut BTreeMap<ReportingPeriod, usize>,
    period: ReportingPeriod,
    peers: &[PersonId],
    budget: &mut OperationBudget<'_>,
) -> Result<usize, AssignmentRuleError> {
    if let Some(index) = positions.get(&period) {
        return Ok(*index);
    }
    reserve_workload_periods(1, peers.len(), budget)?;
    let index = periods.len();
    periods.push(WorkloadPeriod {
        period,
        peers: peers
            .iter()
            .map(|person| WorkloadPeerPeriod {
                person: *person,
                pairs: Vec::new(),
            })
            .collect(),
    });
    positions.insert(period, index);
    Ok(index)
}

fn reserve_workload_periods(
    periods: usize,
    peers: usize,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let period_count = count(periods)?;
    let peer_count = count(peers)?;
    let records_per_period = peer_count.checked_add(2).ok_or_else(arithmetic)?;
    let bytes_per_period = count(size_of::<WorkloadPeriod>())?
        .checked_add(
            peer_count
                .checked_mul(count(size_of::<WorkloadPeerPeriod>())?)
                .ok_or_else(arithmetic)?,
        )
        .and_then(|bytes| bytes.checked_add(size_of::<(ReportingPeriod, usize)>() as u64))
        .ok_or_else(arithmetic)?;
    budget.reserve(
        period_count
            .checked_mul(records_per_period)
            .ok_or_else(arithmetic)?,
        period_count
            .checked_mul(peer_count.checked_add(1).ok_or_else(arithmetic)?)
            .ok_or_else(arithmetic)?,
        period_count
            .checked_mul(bytes_per_period)
            .ok_or_else(arithmetic)?,
    )
}

#[allow(clippy::too_many_lines)]
fn workload_periods(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    scope: &Scope,
    policy: &WorkloadPolicy,
    peers: &[PersonId],
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<WorkloadPeriod>, AssignmentRuleError> {
    let Some(WorkforceEntity::Calendar(calendar)) = input
        .domain
        .entities
        .get(&policy.calendar_id.as_entity_id())
    else {
        return Err(invalid());
    };
    let horizon = Horizon::new(
        document.settings.horizon.start,
        document.settings.horizon.end,
    )
    .map_err(|_| invalid())?;
    let windows = resolve_calendar_for_calendar(
        document,
        policy.calendar_id,
        calendar,
        horizon,
        &mut |step| budget.calendar_step(step),
    )?;
    budget.steps(count(windows.len())?)?;
    reserve_workload_periods(windows.len(), peers.len(), budget)?;
    let mut periods = windows
        .iter()
        .map(|window| WorkloadPeriod {
            period: ReportingPeriod {
                calendar_id: window.calendar_id,
                start: window.interval.starts_at.instant,
                end: window.interval.ends_at.instant,
            },
            peers: peers
                .iter()
                .map(|person| WorkloadPeerPeriod {
                    person: *person,
                    pairs: Vec::new(),
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let mut period_positions = BTreeMap::new();
    for (index, period) in periods.iter().enumerate() {
        budget.step()?;
        period_positions.insert(period.period, index);
    }
    let mut reporting = ReportingWindows::new();
    for (peer_index, person_id) in peers.iter().enumerate() {
        budget.step()?;
        for shift in &input.shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !support::shift_scope(scope, shift, &metadata, budget)?
                || !metadata
                    .assignment_type
                    .workload_bucket_ids
                    .contains(&policy.bucket_id)
            {
                continue;
            }
            let pair = AssignmentPair {
                person_id: *person_id,
                shift_id: shift.id,
            };
            match policy.membership {
                WindowMembership::ReportingDate => {
                    let Some(period) = reporting.resolve(
                        document,
                        input,
                        policy.calendar_id,
                        shift.reporting_date,
                        budget,
                    )?
                    else {
                        continue;
                    };
                    let index = insert_workload_period(
                        &mut periods,
                        &mut period_positions,
                        period,
                        peers,
                        budget,
                    )?;
                    budget.reserve(0, 1, count(size_of::<AssignmentPair>())?)?;
                    periods[index].peers[peer_index].pairs.push(pair);
                }
                WindowMembership::StartInstant => {
                    let start = shift.interval.starts_at.instant;
                    for period in &mut periods {
                        budget.step()?;
                        if period.period.start <= start && start < period.period.end {
                            budget.reserve(0, 1, count(size_of::<AssignmentPair>())?)?;
                            period.peers[peer_index].pairs.push(pair);
                        }
                    }
                }
                WindowMembership::Intersection => {
                    let range = Horizon::new(
                        shift.interval.starts_at.instant,
                        shift.interval.ends_at.instant,
                    )
                    .map_err(|_| invalid())?;
                    let windows = resolve_calendar_for_calendar(
                        document,
                        policy.calendar_id,
                        calendar,
                        range,
                        &mut |step| budget.calendar_step(step),
                    )?;
                    budget.steps(count(windows.len())?)?;
                    for window in windows {
                        let period = ReportingPeriod {
                            calendar_id: window.calendar_id,
                            start: window.interval.starts_at.instant,
                            end: window.interval.ends_at.instant,
                        };
                        let index = insert_workload_period(
                            &mut periods,
                            &mut period_positions,
                            period,
                            peers,
                            budget,
                        )?;
                        budget.reserve(0, 1, count(size_of::<AssignmentPair>())?)?;
                        periods[index].peers[peer_index].pairs.push(pair);
                    }
                }
            }
        }
    }
    periods.sort_by_key(|period| period.period);
    for period in &mut periods {
        budget.step()?;
        for peer in &mut period.peers {
            budget.sort_work(peer.pairs.len())?;
            peer.pairs.sort_unstable();
        }
    }
    Ok(periods)
}

fn lcm_u128(left: u128, right: u128) -> Result<u128, AssignmentRuleError> {
    if left == 0 || right == 0 {
        return Ok(0);
    }
    left.checked_div(gcd_u128(left, right))
        .and_then(|value| value.checked_mul(right))
        .ok_or_else(arithmetic)
}

fn gcd_u128(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn gcd_i64(mut left: i64, mut right: i64) -> i64 {
    left = left.unsigned_abs().cast_signed();
    right = right.unsigned_abs().cast_signed();
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn lcm_i64(left: i64, right: i64) -> Result<i64, AssignmentRuleError> {
    if left <= 0 || right <= 0 {
        return Err(invalid());
    }
    i128::from(left)
        .checked_div(i128::from(gcd_i64(left, right)))
        .and_then(|value| value.checked_mul(i128::from(right)))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(arithmetic)
}

fn score_policy(input: &AssignmentInput) -> Result<&WorkforceScorePolicy, AssignmentRuleError> {
    input
        .domain
        .entities
        .values()
        .find_map(|entity| match entity {
            WorkforceEntity::ScorePolicy(policy) => Some(policy),
            _ => None,
        })
        .ok_or(AssignmentRuleError::MissingScorePolicy)
}

fn mapping(
    policy: &WorkforceScorePolicy,
    priority: PreferencePriority,
) -> Result<(usize, u32), AssignmentRuleError> {
    let mapping = policy
        .priority_mapping
        .iter()
        .find(|mapping| mapping.priority == priority)
        .ok_or_else(invalid)?;
    let level = policy
        .levels
        .iter()
        .position(|level| level.level_key == mapping.level_key)
        .ok_or_else(invalid)?;
    Ok((level, mapping.scale))
}

pub(super) fn category(variant: PreferenceVariant) -> &'static str {
    match variant {
        PreferenceVariant::Time => TIME_CATEGORY,
        PreferenceVariant::AssignmentType => ASSIGNMENT_TYPE_CATEGORY,
        PreferenceVariant::Location => LOCATION_CATEGORY,
        PreferenceVariant::RequestedTimeOff => REQUESTED_TIME_OFF_CATEGORY,
        PreferenceVariant::Adjacency => ADJACENCY_CATEGORY,
        PreferenceVariant::BaseStability => BASE_STABILITY_CATEGORY,
    }
}

pub(super) fn category_id(
    variant: PreferenceVariant,
) -> Result<ScoreCategoryId, AssignmentRuleError> {
    ScoreCategoryId::new(category(variant)).map_err(|_| invalid())
}
pub(super) fn target_category_id() -> Result<ScoreCategoryId, AssignmentRuleError> {
    ScoreCategoryId::new(ASSIGNMENT_TARGET_CATEGORY).map_err(|_| invalid())
}
pub(super) fn weekend_target_category_id() -> Result<ScoreCategoryId, AssignmentRuleError> {
    ScoreCategoryId::new(WEEKEND_TARGET_CATEGORY).map_err(|_| invalid())
}

pub(super) fn matches(
    input: &AssignmentInput,
    entry: PreferenceEntry<'_>,
    pair: AssignmentPair,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    matches_source_preference(input, entry.preference, pair, settings, budget)
}

/// Uses the same scoped per-pair predicate for scoring and source-owned explanations.
pub(super) fn matches_source_preference(
    input: &AssignmentInput,
    preference: &WorkforcePreference,
    pair: AssignmentPair,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let person = input.person(pair.person_id).ok_or_else(invalid)?;
    let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
    let metadata = input.metadata(shift)?;
    let scope = preference.header().2;
    if !support::person_scope(scope, person, budget)?
        || !support::shift_scope(scope, shift, &metadata, budget)?
    {
        return Ok(false);
    }
    match preference {
        WorkforcePreference::Time { time_window, .. } => time_window_matches(
            time_window,
            shift,
            settings,
            EntityId::from_uuid(preference.header().0.as_uuid()),
            budget,
        ),
        WorkforcePreference::AssignmentType {
            assignment_type_ids,
            ..
        } => {
            budget.steps(count(assignment_type_ids.len())?)?;
            Ok(assignment_type_ids.contains(&metadata.assignment_type.id))
        }
        WorkforcePreference::Location { location_ids, .. } => {
            budget.steps(count(location_ids.len())?)?;
            Ok(metadata
                .location_id
                .is_some_and(|id| location_ids.contains(&id)))
        }
        WorkforcePreference::RequestedTimeOff {
            availability_ids, ..
        } => requested_time_off_matches(
            input,
            pair,
            shift,
            &metadata,
            availability_ids,
            settings,
            budget,
        ),
        WorkforcePreference::Adjacency { .. } => adjacency_matches(input, pair, preference, budget),
        WorkforcePreference::BaseStability { .. } => Ok(true),
        _ => Ok(false),
    }
}

fn requested_time_off_matches(
    input: &AssignmentInput,
    pair: AssignmentPair,
    shift: &ResolvedShift,
    metadata: &super::input::ShiftMetadata<'_>,
    availability_ids: &[crate::ids::AvailabilityId],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.steps(count(availability_ids.len())?)?;
    for availability_id in availability_ids {
        let Some(WorkforceEntity::Availability(record)) =
            input.domain.entities.get(&availability_id.as_entity_id())
        else {
            return Err(invalid());
        };
        if record.person_id != pair.person_id
            || record.availability_kind != AvailabilityKind::RequestedTimeOff
            || !support::availability_scope(record, metadata, budget)?
        {
            continue;
        }
        let expanded = availability_intervals(record, support::interval(shift), settings, budget)?;
        if !expanded.intervals.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Adjacency is a source-owned, half-open interval relation. For each scoped candidate pair,
/// the candidate and a source assignment must belong to the same person and be distinct pairs.
/// `Before` means `candidate.end <= source.start`; `After` means `source.end <= candidate.start`;
/// `Either` accepts either direction. The elapsed non-overlap gap is measured exactly in instants,
/// and is at most `maximumGapMinutes`; therefore a bound of zero accepts touching endpoints but
/// never overlapping intervals. A candidate is one opportunity even when several source shifts
/// match it, and source assignments are resolved from the current horizon's typed shift set rather
/// than inferred beyond that horizon.
fn adjacency_matches(
    input: &AssignmentInput,
    pair: AssignmentPair,
    preference: &WorkforcePreference,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let WorkforcePreference::Adjacency {
        base_schedule_id,
        maximum_gap_minutes,
        mode,
        ..
    } = preference
    else {
        return Err(invalid());
    };
    let Some(WorkforceEntity::BaseSchedule(base)) = input.domain.entities.get(base_schedule_id)
    else {
        return Err(invalid());
    };
    let candidate = input.shift(pair.shift_id).ok_or_else(invalid)?;
    let candidate = support::interval(candidate);
    let maximum_gap = jiff::SignedDuration::from_mins(i64::from(*maximum_gap_minutes));
    for source_pair in &base.assignments {
        budget.step()?;
        if source_pair.person_id != pair.person_id || *source_pair == pair {
            continue;
        }
        // Validation establishes the identity; a known occurrence outside this horizon is not
        // retained by `AssignmentInput` and therefore cannot supply inferred work here.
        let Some(source) = input.shift(source_pair.shift_id) else {
            continue;
        };
        let source = support::interval(source);
        let before = candidate.end <= source.start
            && candidate
                .end
                .as_timestamp()
                .duration_until(source.start.as_timestamp())
                <= maximum_gap;
        let after = source.end <= candidate.start
            && source
                .end
                .as_timestamp()
                .duration_until(candidate.start.as_timestamp())
                <= maximum_gap;
        let matched = match mode {
            AdjacencyMode::Before => before,
            AdjacencyMode::After => after,
            AdjacencyMode::Either => before || after,
        };
        if matched {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Original source static feasibility, independent of the analysis candidate graph. Dynamic
/// coverage, overlap, rest, locks and all other inter-assignment constraints are deliberately not
/// consulted here.
pub(super) fn statically_feasible(
    input: &AssignmentInput,
    pair: AssignmentPair,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let person = input.person(pair.person_id).ok_or_else(invalid)?;
    let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
    let metadata = input.metadata(shift)?;
    let query = support::interval(shift);
    if let Some(active) = support::active_interval(person, settings)?
        && support::outside(query, active).is_some()
    {
        return Ok(false);
    }
    for rule in input.domain.rules.values() {
        budget.step()?;
        let (_id, active, scope) = rule.header();
        if !active
            || !support::person_scope(scope, person, budget)?
            || !support::shift_scope(scope, shift, &metadata, budget)?
        {
            continue;
        }
        match rule {
            WorkforceRule::Eligibility { .. } => {
                budget.steps(count(person.eligible_assignment_type_ids.len())?)?;
                if !person
                    .eligible_assignment_type_ids
                    .contains(&metadata.assignment_type.id)
                    || !support::expression(
                        person,
                        &metadata.assignment_type.qualifications,
                        query,
                        budget,
                    )?
                {
                    return Ok(false);
                }
            }
            WorkforceRule::Availability { .. }
                if !records_allow(input, person.id, shift, &metadata, true, settings, budget)? =>
            {
                return Ok(false);
            }
            _ => {}
        }
    }
    if !records_allow(input, person.id, shift, &metadata, false, settings, budget)? {
        return Ok(false);
    }
    Ok(true)
}

fn records_allow(
    input: &AssignmentInput,
    person_id: PersonId,
    shift: &ResolvedShift,
    metadata: &super::input::ShiftMetadata<'_>,
    ordinary: bool,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let Some(records) = input.availability_by_person.get(&person_id) else {
        return Ok(true);
    };
    for availability_id in records {
        budget.step()?;
        let Some(WorkforceEntity::Availability(record)) =
            input.domain.entities.get(&availability_id.as_entity_id())
        else {
            return Err(invalid());
        };
        let applicable = if ordinary {
            matches!(
                record.availability_kind,
                AvailabilityKind::Unavailable | AvailabilityKind::AvailableOnly
            )
        } else {
            record.availability_kind == AvailabilityKind::ApprovedTimeOff
        };
        if !applicable || !support::availability_scope(record, metadata, budget)? {
            continue;
        }
        let expanded = availability_intervals(record, support::interval(shift), settings, budget)?;
        let Some(query) = expanded.query else {
            continue;
        };
        let failed = match record.availability_kind {
            AvailabilityKind::AvailableOnly => {
                support::uncovered(query, &expanded.intervals, budget)?.is_some()
            }
            AvailabilityKind::Unavailable | AvailabilityKind::ApprovedTimeOff => {
                !expanded.intervals.is_empty()
            }
            AvailabilityKind::RequestedTimeOff => false,
        };
        if failed {
            return Ok(false);
        }
    }
    Ok(true)
}

fn time_window_matches(
    window: &TimeWindow,
    shift: &ResolvedShift,
    settings: &ScenarioSettings,
    owner: EntityId,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let query = support::interval(shift);
    match window {
        TimeWindow::Instant { starts_at, ends_at } => Ok(query
            .intersection(InstantInterval {
                start: *starts_at,
                end: *ends_at,
            })
            .is_some()),
        TimeWindow::Weekly { windows } => {
            let zone = TimeZone::get(settings.time_zone.as_str()).map_err(|_| invalid())?;
            for weekly in windows {
                budget.step()?;
                let (first, last) = weekly_bounds(query, weekly.end_day_offset, owner)?;
                let mut date = first;
                while date <= last {
                    budget.step()?;
                    budget.steps(count(weekly.weekdays.len())?)?;
                    if weekly.weekdays.contains(&weekday(date)) {
                        let end_date = date
                            .checked_add(Span::new().days(i64::from(weekly.end_day_offset)))
                            .map_err(|_| invalid())?;
                        let start = date.to_datetime(weekly.start_time);
                        let end = end_date.to_datetime(weekly.end_time);
                        if potentially_intersects(
                            start,
                            end,
                            &zone,
                            Horizon {
                                start: query.start,
                                end: query.end,
                            },
                            owner,
                        )? {
                            let resolved = resolve_interval(start, end, settings, owner)?;
                            if query
                                .intersection(InstantInterval {
                                    start: resolved.starts_at.instant,
                                    end: resolved.ends_at.instant,
                                })
                                .is_some()
                            {
                                return Ok(true);
                            }
                        }
                    }
                    if date == last {
                        break;
                    }
                    date = date
                        .checked_add(Span::new().days(1))
                        .map_err(|_| invalid())?;
                }
            }
            Ok(false)
        }
    }
}

fn weekly_bounds(
    query: InstantInterval,
    end_day_offset: u8,
    owner: EntityId,
) -> Result<(Date, Date), AssignmentRuleError> {
    let first = Offset::UTC
        .to_datetime(query.start.as_timestamp())
        .checked_add(Span::new().seconds(i64::from(Offset::MIN.seconds())))
        .map_err(|_| temporal_overflow(owner))?
        .date()
        .checked_sub(Span::new().days(i64::from(end_day_offset)))
        .map_err(|_| temporal_overflow(owner))?;
    let last = Offset::UTC
        .to_datetime(query.end.as_timestamp())
        .checked_add(Span::new().seconds(i64::from(Offset::MAX.seconds())))
        .map_err(|_| temporal_overflow(owner))?
        .date();
    Ok((first, last))
}

pub(super) fn base_contains(
    entry: PreferenceEntry<'_>,
    pair: AssignmentPair,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let Some(assignments) = entry.base_assignments else {
        return Ok(false);
    };
    for candidate in assignments {
        budget.step()?;
        if *candidate == pair {
            return Ok(true);
        }
    }
    Ok(false)
}
/// The exact source preference term at one eligible pair; not a whole-schedule score change.
pub(super) fn pair_penalty(
    entry: PreferenceEntry<'_>,
    selected: bool,
    base_member: bool,
) -> Result<i64, AssignmentRuleError> {
    match entry.variant {
        PreferenceVariant::BaseStability => {
            if selected == base_member {
                Ok(0)
            } else {
                Ok(entry.coefficient)
            }
        }
        _ => match (entry.variant, entry.direction, selected) {
            (PreferenceVariant::RequestedTimeOff, None, true)
            | (_, Some(PreferenceDirection::Prefer), false)
            | (_, Some(PreferenceDirection::Avoid), true) => Ok(entry.coefficient),
            (PreferenceVariant::RequestedTimeOff, None, false)
            | (_, Some(PreferenceDirection::Prefer), true)
            | (_, Some(PreferenceDirection::Avoid), false) => Ok(0),
            _ => Err(invalid()),
        },
    }
}

/// membership is logarithmic and no per-opportunity set allocation is needed.
#[allow(clippy::too_many_lines)]
pub(super) fn score(
    input: &AssignmentInput,
    plan: &PreferencePlan<'_>,
    selected: &[AssignmentPair],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<PreferenceScore>, AssignmentRuleError> {
    score_with_outcomes(input, plan, selected, settings, budget, None)
}

#[allow(clippy::too_many_lines)]
pub(super) fn score_with_outcomes(
    input: &AssignmentInput,
    plan: &PreferencePlan<'_>,
    selected: &[AssignmentPair],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
    mut outcomes: Option<&mut super::alternative_outcomes::Collector>,
) -> Result<Option<PreferenceScore>, AssignmentRuleError> {
    use super::alternative_outcomes::{Family, Key, Values};
    if !plan.active() && plan.levels.is_empty() {
        return Ok(None);
    }
    let length = count(plan.levels.len())?;
    let bytes = length
        .checked_mul(count(size_of::<PreferenceLevelScore>())?)
        .ok_or_else(arithmetic)?;
    budget.reserve(1, length, bytes)?;
    let mut levels = (0..plan.levels.len())
        .map(|_| PreferenceLevelScore {
            value: 0,
            time: 0,
            assignment_type: 0,
            location: 0,
            requested_time_off: 0,
            assignment_target: 0,
            weekend_target: 0,
            workload_balance: 0,
            consecutive_work: 0,
            adjacency: 0,
            base_stability: 0,
            together_separate: 0,
            preferred_coverage: 0,
        })
        .collect::<Vec<_>>();
    for entry in &plan.entries {
        for person_id in &input.people {
            budget.step()?;
            for shift in &input.shifts {
                budget.step()?;
                let pair = AssignmentPair {
                    person_id: *person_id,
                    shift_id: shift.id,
                };
                if !matches(input, *entry, pair, settings, budget)? {
                    continue;
                }
                let base_member = if matches!(entry.variant, PreferenceVariant::BaseStability) {
                    base_contains(*entry, pair, budget)?
                } else {
                    false
                };
                let feasible = statically_feasible(input, pair, settings, budget)?;
                if !feasible && !base_member {
                    continue;
                }
                let selected_pair = if feasible {
                    budget.steps(selection_search_steps(selected.len()))?;
                    selected.binary_search(&pair).is_ok()
                } else {
                    false
                };
                let penalty = pair_penalty(*entry, selected_pair, base_member)?;
                if let Some(outcomes) = outcomes.as_deref_mut() {
                    let mismatched = match entry.variant {
                        PreferenceVariant::BaseStability => selected_pair != base_member,
                        _ => matches!(
                            (entry.variant, entry.direction, selected_pair),
                            (PreferenceVariant::RequestedTimeOff, None, true)
                                | (_, Some(PreferenceDirection::Prefer), false)
                                | (_, Some(PreferenceDirection::Avoid), true)
                        ),
                    };
                    outcomes.counts(
                        Key::person(Family::Direct, entry.id, *person_id),
                        1,
                        i128::from(selected_pair),
                        i128::from(mismatched),
                        i128::from(penalty),
                        budget,
                    )?;
                }
                let level = levels.get_mut(entry.level).ok_or_else(invalid)?;
                level.value = level.value.checked_add(penalty).ok_or_else(arithmetic)?;
                match entry.variant {
                    PreferenceVariant::Time => {
                        level.time = level.time.checked_add(penalty).ok_or_else(arithmetic)?;
                    }
                    PreferenceVariant::AssignmentType => {
                        level.assignment_type = level
                            .assignment_type
                            .checked_add(penalty)
                            .ok_or_else(arithmetic)?;
                    }
                    PreferenceVariant::Location => {
                        level.location =
                            level.location.checked_add(penalty).ok_or_else(arithmetic)?;
                    }
                    PreferenceVariant::RequestedTimeOff => {
                        level.requested_time_off = level
                            .requested_time_off
                            .checked_add(penalty)
                            .ok_or_else(arithmetic)?;
                    }
                    PreferenceVariant::Adjacency => {
                        level.adjacency = level
                            .adjacency
                            .checked_add(penalty)
                            .ok_or_else(arithmetic)?;
                    }
                    PreferenceVariant::BaseStability => {
                        level.base_stability = level
                            .base_stability
                            .checked_add(penalty)
                            .ok_or_else(arithmetic)?;
                    }
                }
            }
        }
    }
    for target in &plan.targets {
        for period in &target.periods {
            budget.step()?;
            let mut actual = 0_u64;
            for pair in &period.pairs {
                budget.step()?;
                if selected.binary_search(pair).is_ok() {
                    actual = actual.checked_add(1).ok_or_else(arithmetic)?;
                }
                budget.steps(selection_search_steps(selected.len()))?;
            }
            let difference = actual.abs_diff(u64::from(target.target_count));
            let penalty = i64::try_from(difference)
                .map_err(|_| arithmetic())?
                .checked_mul(target.coefficient)
                .ok_or_else(arithmetic)?;
            if let Some(outcomes) = outcomes.as_deref_mut() {
                let mut key = Key::person(Family::AssignmentTarget, target.id, period.person);
                key.period = Some((period.period.start, period.period.end));
                outcomes.insert(
                    key,
                    Values::Target {
                        actual: i128::from(actual),
                        target: i128::from(target.target_count),
                        difference: i128::from(actual) - i128::from(target.target_count),
                        cost: i128::from(penalty),
                    },
                    budget,
                )?;
            }
            let level = levels.get_mut(target.level).ok_or_else(invalid)?;
            level.value = level.value.checked_add(penalty).ok_or_else(arithmetic)?;
            level.assignment_target = level
                .assignment_target
                .checked_add(penalty)
                .ok_or_else(arithmetic)?;
        }
    }
    for target in &plan.weekend_targets {
        for person in &target.people {
            budget.step()?;
            let mut actual = 0_u64;
            for weekend in &person.weekends {
                budget.step()?;
                let mut worked = false;
                for pair in &weekend.pairs {
                    budget.step()?;
                    budget.steps(selection_search_steps(selected.len()))?;
                    if selected.binary_search(pair).is_ok() {
                        worked = true;
                        break;
                    }
                }
                if worked {
                    actual = actual.checked_add(1).ok_or_else(arithmetic)?;
                }
            }
            let difference = actual.abs_diff(u64::from(target.target_count));
            let penalty = i64::try_from(difference)
                .map_err(|_| arithmetic())?
                .checked_mul(target.coefficient)
                .ok_or_else(arithmetic)?;
            if let Some(outcomes) = outcomes.as_deref_mut() {
                outcomes.insert(
                    Key::person(Family::WeekendTarget, target.id, person.person),
                    Values::Target {
                        actual: i128::from(actual),
                        target: i128::from(target.target_count),
                        difference: i128::from(actual) - i128::from(target.target_count),
                        cost: i128::from(penalty),
                    },
                    budget,
                )?;
            }
            let level = levels.get_mut(target.level).ok_or_else(invalid)?;
            level.value = level.value.checked_add(penalty).ok_or_else(arithmetic)?;
            level.weekend_target = level
                .weekend_target
                .checked_add(penalty)
                .ok_or_else(arithmetic)?;
        }
    }
    for group in &plan.groups {
        let penalty = score_group(
            input,
            group,
            selected,
            settings,
            budget,
            outcomes.as_deref_mut(),
        )?;
        let level = levels.get_mut(group.level).ok_or_else(invalid)?;
        level.value = level.value.checked_add(penalty).ok_or_else(arithmetic)?;
        level.together_separate = level
            .together_separate
            .checked_add(penalty)
            .ok_or_else(arithmetic)?;
    }
    for coverage_plan in &plan.coverage {
        let penalty = score_preferred_coverage(
            input,
            coverage_plan,
            selected,
            settings,
            budget,
            outcomes.as_deref_mut(),
        )?;
        let level = levels.get_mut(coverage_plan.level).ok_or_else(invalid)?;
        level.value = level.value.checked_add(penalty).ok_or_else(arithmetic)?;
        level.preferred_coverage = level
            .preferred_coverage
            .checked_add(penalty)
            .ok_or_else(arithmetic)?;
    }
    for workload in &plan.workloads {
        for period in &workload.periods {
            budget.step()?;
            if outcomes.is_some() {
                let length = count(period.peers.len())?;
                budget.reserve(
                    1,
                    length,
                    length
                        .checked_mul(count(size_of::<i128>())?)
                        .ok_or_else(arithmetic)?,
                )?;
            }
            let mut actuals = Vec::with_capacity(period.peers.len());
            for peer in &period.peers {
                budget.step()?;
                actuals.push(source_workload_actual(
                    input,
                    workload,
                    period.period,
                    peer,
                    selected,
                    budget,
                )?);
            }
            let total = actuals.iter().try_fold(0_i128, |sum, value| {
                sum.checked_add(*value).ok_or_else(arithmetic)
            })?;
            for (index, peer) in period.peers.iter().enumerate() {
                budget.step()?;
                let actual = *actuals.get(index).ok_or_else(invalid)?;
                let (numerator, _) =
                    workload_numerator(workload, peer.person, actual, total, &actuals)?;
                let cost = piecewise_cost(
                    i128::try_from(numerator.unsigned_abs()).map_err(|_| arithmetic())?,
                    i128::from(workload.score_denominator),
                    &workload.policy.penalty,
                )?;
                let penalty = i64::try_from(cost)
                    .map_err(|_| arithmetic())?
                    .checked_mul(workload.coefficient)
                    .ok_or_else(arithmetic)?;
                if let Some(outcomes) = outcomes.as_deref_mut() {
                    let (_, denominator) =
                        workload_numerator(workload, peer.person, actual, total, &actuals)?;
                    let mut key = Key::person(Family::Workload, workload.id, peer.person);
                    key.period = Some((period.period.start, period.period.end));
                    outcomes.insert(
                        key,
                        Values::Workload {
                            actual,
                            deviation: numerator,
                            denominator: i128::from(denominator),
                            cost: i128::from(penalty),
                        },
                        budget,
                    )?;
                }
                let level = levels.get_mut(workload.level).ok_or_else(invalid)?;
                level.value = level.value.checked_add(penalty).ok_or_else(arithmetic)?;
                level.workload_balance = level
                    .workload_balance
                    .checked_add(penalty)
                    .ok_or_else(arithmetic)?;
            }
        }
    }
    for consecutive in &plan.consecutives {
        let penalty = preference_consecutive::score_with_outcomes(
            input,
            consecutive,
            selected,
            settings,
            budget,
            outcomes.as_deref_mut(),
        )?;
        let level = levels.get_mut(consecutive.level).ok_or_else(invalid)?;
        level.value = level.value.checked_add(penalty).ok_or_else(arithmetic)?;
        level.consecutive_work = level
            .consecutive_work
            .checked_add(penalty)
            .ok_or_else(arithmetic)?;
    }
    Ok(Some(PreferenceScore { levels }))
}

pub(super) fn relation_matches(
    input: &AssignmentInput,
    first: AssignmentPair,
    second: AssignmentPair,
    mode: PairingMode,
) -> Result<bool, AssignmentRuleError> {
    if matches!(mode, PairingMode::SameShift) {
        return Ok(first.shift_id == second.shift_id);
    }
    let first_shift = input.shift(first.shift_id).ok_or_else(invalid)?;
    let second_shift = input.shift(second.shift_id).ok_or_else(invalid)?;
    Ok(support::interval(first_shift)
        .intersection(support::interval(second_shift))
        .is_some())
}

pub(super) fn group_opportunities(
    input: &AssignmentInput,
    plan: &GroupPlan<'_>,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<AssignmentPair>, AssignmentRuleError> {
    let mut opportunities = Vec::new();
    for person_id in &input.people {
        budget.step()?;
        if !plan.person_ids.contains(person_id) {
            budget.steps(count(plan.person_ids.len())?)?;
            continue;
        }
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !support::person_scope(plan.scope, person, budget)? {
            continue;
        }
        for shift in &input.shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !support::shift_scope(plan.scope, shift, &metadata, budget)? {
                continue;
            }
            let pair = AssignmentPair {
                person_id: *person_id,
                shift_id: shift.id,
            };
            if statically_feasible(input, pair, settings, budget)? {
                budget.reserve(0, 1, 16)?;
                opportunities.push(pair);
            }
        }
    }
    Ok(opportunities)
}

fn score_group(
    input: &AssignmentInput,
    plan: &GroupPlan<'_>,
    selected: &[AssignmentPair],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
    mut outcomes: Option<&mut super::alternative_outcomes::Collector>,
) -> Result<i64, AssignmentRuleError> {
    let opportunities = group_opportunities(input, plan, settings, budget)?;
    let mut penalty = 0_i64;
    for first in 0..opportunities.len() {
        budget.step()?;
        for second in first + 1..opportunities.len() {
            budget.step()?;
            let first_pair = *opportunities.get(first).ok_or_else(invalid)?;
            let second_pair = *opportunities.get(second).ok_or_else(invalid)?;
            if first_pair.person_id == second_pair.person_id {
                continue;
            }
            let first_selected = selected.binary_search(&first_pair).is_ok();
            let second_selected = selected.binary_search(&second_pair).is_ok();
            budget.steps(selection_search_steps(selected.len()).saturating_mul(2))?;
            if !first_selected || !second_selected {
                continue;
            }
            let relation = relation_matches(input, first_pair, second_pair, plan.mode)?;
            let mismatch = match plan.direction {
                GroupDirection::Together => !relation,
                GroupDirection::Separate => relation,
            };
            if let Some(outcomes) = outcomes.as_deref_mut() {
                use super::alternative_outcomes::{Family, Key};
                let mut key = Key::person(
                    Family::Pairing,
                    plan.id,
                    first_pair.person_id.min(second_pair.person_id),
                );
                key.participant = Some(first_pair.person_id.max(second_pair.person_id));
                outcomes.counts(
                    key,
                    1,
                    i128::from(!mismatch),
                    i128::from(mismatch),
                    if mismatch {
                        i128::from(plan.coefficient)
                    } else {
                        0
                    },
                    budget,
                )?;
            }
            if mismatch {
                penalty = penalty
                    .checked_add(plan.coefficient)
                    .ok_or_else(arithmetic)?;
            }
        }
    }
    Ok(penalty)
}

fn score_preferred_coverage(
    input: &AssignmentInput,
    plan: &CoveragePlan<'_>,
    selected: &[AssignmentPair],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
    mut outcomes: Option<&mut super::alternative_outcomes::Collector>,
) -> Result<i64, AssignmentRuleError> {
    let mut penalty = 0_i64;
    for requirement_id in plan.coverage_requirement_ids {
        budget.step()?;
        let Some(WorkforceEntity::CoverageRequirement(requirement)) =
            input.domain.entities.get(&requirement_id.as_entity_id())
        else {
            return Err(invalid());
        };
        if !requirement.active {
            continue;
        }
        let preferred_count = match &requirement.coverage {
            Coverage::AtLeast {
                preferred_count: Some(value),
                ..
            } => u64::from(*value),
            Coverage::AtLeast {
                preferred_count: None,
                ..
            }
            | Coverage::Exact { .. } => return Err(invalid()),
        };
        if preferred_count == 0 && outcomes.is_none() {
            continue;
        }
        for shift in &input.shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !requirement_shift_matches(&requirement.scope, shift, &metadata, budget)?
                || !support::shift_scope(plan.scope, shift, &metadata, budget)?
            {
                continue;
            }
            let mut actual = 0_u64;
            for person_id in &input.people {
                budget.step()?;
                let person = input.person(*person_id).ok_or_else(invalid)?;
                if !support::person_scope(plan.scope, person, budget)? {
                    continue;
                }
                let pair = AssignmentPair {
                    person_id: *person_id,
                    shift_id: shift.id,
                };
                if selected.binary_search(&pair).is_err() {
                    budget.steps(selection_search_steps(selected.len()))?;
                    continue;
                }
                budget.steps(selection_search_steps(selected.len()))?;
                if statically_feasible(input, pair, settings, budget)? {
                    actual = actual.checked_add(1).ok_or_else(arithmetic)?;
                }
            }
            let deficit = preferred_count.saturating_sub(actual);
            if let Some(outcomes) = outcomes.as_deref_mut() {
                use super::alternative_outcomes::{Family, Key, Values};
                let key = Key {
                    family: Family::Coverage,
                    rule: plan.id,
                    person: None,
                    participant: None,
                    owner: Some(requirement_id.as_entity_id()),
                    shift: Some(shift.id),
                    period: None,
                };
                outcomes.insert(
                    key,
                    Values::Target {
                        actual: i128::from(actual),
                        target: i128::from(preferred_count),
                        difference: i128::from(deficit),
                        cost: i128::from(deficit)
                            .checked_mul(i128::from(plan.coefficient))
                            .ok_or_else(arithmetic)?,
                    },
                    budget,
                )?;
            }
            let deficit = i64::try_from(deficit).map_err(|_| arithmetic())?;
            penalty = penalty
                .checked_add(
                    deficit
                        .checked_mul(plan.coefficient)
                        .ok_or_else(arithmetic)?,
                )
                .ok_or_else(arithmetic)?;
        }
    }
    Ok(penalty)
}

fn source_workload_actual(
    input: &AssignmentInput,
    workload: &WorkloadPlan<'_>,
    period: ReportingPeriod,
    peer: &WorkloadPeerPeriod,
    selected: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<i128, AssignmentRuleError> {
    let bucket = workload_bucket(input, workload.policy.bucket_id)?;
    let mut intervals = Vec::new();
    let mut total = 0_i128;
    for pair in &peer.pairs {
        budget.step()?;
        if selected.binary_search(pair).is_err() {
            budget.steps(selection_search_steps(selected.len()))?;
            continue;
        }
        let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
        if bucket.overlapping_contribution == OverlappingContribution::Union {
            if let Some((start, end)) = workload_interval(shift, period, workload.policy.membership)
            {
                budget.reserve(
                    0,
                    1,
                    count(size_of::<(
                        eutheto_types::Rfc3339Timestamp,
                        eutheto_types::Rfc3339Timestamp,
                    )>())?,
                )?;
                intervals.push((start, end));
            }
        } else {
            total = total
                .checked_add(i128::from(workload_amount(
                    shift,
                    period,
                    workload.policy.membership,
                    bucket,
                )?))
                .ok_or_else(arithmetic)?;
        }
        budget.steps(selection_search_steps(selected.len()))?;
    }
    if bucket.overlapping_contribution == OverlappingContribution::Union {
        total = union_duration_nanos(&mut intervals, budget)?;
    }
    exact_units(total, workload.nanoseconds_per_unit)
}

fn exact_units(amount: i128, quantum: i64) -> Result<i128, AssignmentRuleError> {
    if quantum <= 0 || amount % i128::from(quantum) != 0 {
        return Err(invalid());
    }
    Ok(amount / i128::from(quantum))
}

fn union_duration_nanos(
    intervals: &mut [(
        eutheto_types::Rfc3339Timestamp,
        eutheto_types::Rfc3339Timestamp,
    )],
    budget: &mut OperationBudget<'_>,
) -> Result<i128, AssignmentRuleError> {
    budget.sort_work(intervals.len())?;
    intervals.sort_unstable_by_key(|(start, end)| (*start, *end));
    let Some((mut start, mut end)) = intervals.first().copied() else {
        return Ok(0);
    };
    let mut total = 0_i128;
    for (next_start, next_end) in intervals.iter().copied().skip(1) {
        budget.step()?;
        if next_start <= end {
            end = end.max(next_end);
        } else {
            total = total
                .checked_add(duration_nanos(start, end)?)
                .ok_or_else(arithmetic)?;
            start = next_start;
            end = next_end;
        }
    }
    total
        .checked_add(duration_nanos(start, end)?)
        .ok_or_else(arithmetic)
}

fn duration_nanos(
    start: eutheto_types::Rfc3339Timestamp,
    end: eutheto_types::Rfc3339Timestamp,
) -> Result<i128, AssignmentRuleError> {
    let duration = start.as_timestamp().duration_until(end.as_timestamp());
    i128::from(duration.as_secs())
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i128::from(duration.subsec_nanos())))
        .ok_or_else(arithmetic)
}

fn workload_numerator(
    workload: &WorkloadPlan<'_>,
    person: PersonId,
    actual: i128,
    total: i128,
    actuals: &[i128],
) -> Result<(i128, i64), AssignmentRuleError> {
    match workload.target.kind {
        WorkloadTargetKind::Explicit | WorkloadTargetKind::PersonTargets => Ok((
            actual
                .checked_sub(
                    i128::from(*workload.target.targets.get(&person).ok_or_else(invalid)?)
                        .checked_mul(i128::from(workload.units_per_target_unit))
                        .ok_or_else(arithmetic)?,
                )
                .ok_or_else(arithmetic)?,
            1,
        )),
        WorkloadTargetKind::EqualShare => {
            let count = i128::try_from(actuals.len()).map_err(|_| arithmetic())?;
            Ok((
                count
                    .checked_mul(actual)
                    .and_then(|value| value.checked_sub(total))
                    .ok_or_else(arithmetic)?,
                i64::try_from(count).map_err(|_| arithmetic())?,
            ))
        }
        WorkloadTargetKind::WeightedEqualShare => {
            let denominator = workload.target.denominator;
            let weight = *workload.target.weights.get(&person).ok_or_else(invalid)?;
            let weighted_target = i128::from(weight)
                .checked_mul(total)
                .ok_or_else(arithmetic)?;
            let numerator = i128::from(denominator)
                .checked_mul(actual)
                .and_then(|value| value.checked_sub(weighted_target))
                .ok_or_else(arithmetic)?;
            Ok((numerator, denominator))
        }
    }
}

fn selection_search_steps(length: usize) -> u64 {
    let length = u64::try_from(length).unwrap_or(u64::MAX);
    u64::from(u64::BITS - length.saturating_add(1).leading_zeros())
}

#[allow(clippy::too_many_lines)]
fn workload_shape(
    input: &AssignmentInput,
    plan: &PreferencePlan<'_>,
    candidates: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<WorkloadShape, AssignmentRuleError> {
    let mut periods = Vec::new();
    for (workload_index, workload) in plan.workloads.iter().enumerate() {
        budget.step()?;
        let bucket = workload_bucket(input, workload.policy.bucket_id)?;
        for (period_index, period) in workload.periods.iter().enumerate() {
            budget.step()?;
            let mut peers = Vec::with_capacity(period.peers.len());
            for source_peer in &period.peers {
                budget.step()?;
                let mut terms = Vec::new();
                let mut intervals = Vec::new();
                for (index, pair) in candidates.iter().enumerate() {
                    budget.step()?;
                    if pair.person_id != source_peer.person
                        || source_peer.pairs.binary_search(pair).is_err()
                    {
                        continue;
                    }
                    let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
                    if bucket.overlapping_contribution == OverlappingContribution::Union {
                        if let Some((start, end)) =
                            workload_interval(shift, period.period, workload.policy.membership)
                        {
                            intervals.push((index, start, end));
                        }
                    } else {
                        let amount = workload_amount(
                            shift,
                            period.period,
                            workload.policy.membership,
                            bucket,
                        )?;
                        let units = exact_units(i128::from(amount), workload.nanoseconds_per_unit)?;
                        terms.push((
                            index,
                            i64::try_from(units).map_err(|_| {
                                AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord)
                            })?,
                        ));
                    }
                    budget.reserve(0, 1, 16)?;
                }
                let segments = if bucket.overlapping_contribution == OverlappingContribution::Union
                {
                    union_segments(&intervals, workload.nanoseconds_per_unit, budget)?
                } else {
                    Vec::new()
                };
                let (actual_lower, actual_upper) = if segments.is_empty() {
                    let mut lower = 0_i64;
                    let mut upper = 0_i64;
                    for (_, amount) in &terms {
                        if *amount < 0 {
                            lower = lower.checked_add(*amount).ok_or_else(arithmetic)?;
                        } else {
                            upper = upper.checked_add(*amount).ok_or_else(arithmetic)?;
                        }
                    }
                    (lower, upper)
                } else {
                    let upper = segments.iter().try_fold(0_i64, |sum, segment| {
                        sum.checked_add(segment.units).ok_or_else(arithmetic)
                    })?;
                    (0, upper)
                };
                let split_bounds = if workload.units_per_target_unit > 1 {
                    let mut whole_lower = 0_i64;
                    let mut whole_upper = 0_i64;
                    let mut remainder_upper = 0_i64;
                    for amount in segments
                        .iter()
                        .map(|segment| segment.units)
                        .chain(terms.iter().map(|(_, amount)| *amount))
                    {
                        budget.step()?;
                        let whole = amount.div_euclid(workload.units_per_target_unit);
                        let remainder = amount.rem_euclid(workload.units_per_target_unit);
                        if whole < 0 {
                            whole_lower = whole_lower.checked_add(whole).ok_or_else(arithmetic)?;
                        } else {
                            whole_upper = whole_upper.checked_add(whole).ok_or_else(arithmetic)?;
                        }
                        remainder_upper = remainder_upper
                            .checked_add(remainder)
                            .ok_or_else(arithmetic)?;
                    }
                    Some((whole_lower, whole_upper, remainder_upper))
                } else {
                    None
                };
                peers.push(WorkloadPeerShape {
                    person: source_peer.person,
                    terms,
                    segments,
                    actual_lower,
                    actual_upper,
                    split_bounds,
                    delta_lower: 0,
                    delta_upper: 0,
                    difference_upper: 0,
                    cost_upper: 0,
                });
            }
            // Equal/weighted targets depend on every peer in this period. Recompute the
            // conservative delta bounds after the peer vector is complete.
            let bounds = peers
                .iter()
                .map(|peer| (peer.actual_lower, peer.actual_upper))
                .collect::<Vec<_>>();
            for peer in &mut peers {
                let (lower, upper) = workload_delta_bounds_from_bounds(
                    workload,
                    peer.person,
                    peer.actual_lower,
                    peer.actual_upper,
                    &period.peers,
                    &bounds,
                )?;
                peer.delta_lower = lower;
                peer.delta_upper = upper;
                let difference_upper =
                    i64::try_from(lower.unsigned_abs().max(upper.unsigned_abs()))
                        .map_err(|_| arithmetic())?;
                peer.difference_upper = difference_upper;
                peer.cost_upper = i64::try_from(piecewise_cost(
                    i128::from(difference_upper),
                    i128::from(workload.score_denominator),
                    &workload.policy.penalty,
                )?)
                .map_err(|_| arithmetic())?;
            }
            periods.push(WorkloadPeriodShape {
                workload: workload_index,
                period: period_index,
                peers,
            });
        }
    }
    Ok(WorkloadShape { periods })
}

fn workload_bucket(
    input: &AssignmentInput,
    bucket_id: crate::ids::WorkloadBucketId,
) -> Result<&crate::model::WorkloadBucket, AssignmentRuleError> {
    match input.domain.entities.get(&bucket_id.as_entity_id()) {
        Some(WorkforceEntity::WorkloadBucket(bucket)) => Ok(bucket),
        _ => Err(invalid()),
    }
}

pub(super) fn workload_interval(
    shift: &ResolvedShift,
    period: ReportingPeriod,
    membership: WindowMembership,
) -> Option<(
    eutheto_types::Rfc3339Timestamp,
    eutheto_types::Rfc3339Timestamp,
)> {
    let start = shift.interval.starts_at.instant;
    let end = shift.interval.ends_at.instant;
    match membership {
        WindowMembership::Intersection => {
            let start = start.max(period.start);
            let end = end.min(period.end);
            (start < end).then_some((start, end))
        }
        WindowMembership::ReportingDate | WindowMembership::StartInstant => Some((start, end)),
    }
}

pub(super) fn workload_amount(
    shift: &ResolvedShift,
    period: ReportingPeriod,
    membership: WindowMembership,
    bucket: &crate::model::WorkloadBucket,
) -> Result<i64, AssignmentRuleError> {
    if bucket.measurement == WorkloadMeasurement::AssignmentCount {
        return Ok(1);
    }
    let interval = workload_interval(shift, period, membership).ok_or_else(invalid)?;
    let duration = if bucket.measurement == WorkloadMeasurement::ElapsedMinutes {
        interval
            .0
            .as_timestamp()
            .duration_until(interval.1.as_timestamp())
    } else {
        if membership == WindowMembership::Intersection {
            return Err(invalid());
        }
        shift.interval.scheduled_duration()
    };
    let nanos = i128::from(duration.as_secs())
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i128::from(duration.subsec_nanos())))
        .ok_or_else(arithmetic)?;
    i64::try_from(nanos)
        .map_err(|_| AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord))
}

fn union_segments(
    intervals: &[(
        usize,
        eutheto_types::Rfc3339Timestamp,
        eutheto_types::Rfc3339Timestamp,
    )],
    nanoseconds_per_unit: i64,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<WorkloadSegmentShape>, AssignmentRuleError> {
    let endpoint_count = count(intervals.len())?
        .checked_mul(2)
        .ok_or_else(arithmetic)?;
    let bytes_per_endpoint = count(size_of::<eutheto_types::Rfc3339Timestamp>())?
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(size_of::<usize>() as u64))
        .ok_or_else(arithmetic)?;
    budget.reserve(
        0,
        endpoint_count.checked_mul(2).ok_or_else(arithmetic)?,
        endpoint_count
            .checked_mul(bytes_per_endpoint)
            .ok_or_else(arithmetic)?,
    )?;
    let mut endpoints = BTreeSet::new();
    for (_, start, end) in intervals {
        budget.step()?;
        endpoints.insert(*start);
        endpoints.insert(*end);
    }
    let endpoints = endpoints.into_iter().collect::<Vec<_>>();
    let mut segments = Vec::new();
    for pair in endpoints.windows(2) {
        budget.step()?;
        let [start, end] = *pair else {
            continue;
        };
        if start >= end {
            continue;
        }
        let mut population = Vec::new();
        for (index, interval_start, interval_end) in intervals {
            budget.step()?;
            if *interval_start <= start && end <= *interval_end {
                budget.reserve(0, 1, count(size_of::<usize>())?)?;
                population.push(*index);
            }
        }
        if population.is_empty() {
            continue;
        }
        budget.reserve(1, 1, count(size_of::<WorkloadSegmentShape>())?)?;
        segments.push(WorkloadSegmentShape {
            units: i64::try_from(exact_units(
                duration_nanos(start, end)?,
                nanoseconds_per_unit,
            )?)
            .map_err(|_| AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord))?,
            population,
        });
    }
    Ok(segments)
}

fn workload_delta_bounds_from_bounds(
    workload: &WorkloadPlan<'_>,
    person: PersonId,
    actual_lower: i64,
    actual_upper: i64,
    periods: &[WorkloadPeerPeriod],
    bounds: &[(i64, i64)],
) -> Result<(i64, i64), AssignmentRuleError> {
    let index = periods
        .iter()
        .position(|peer| peer.person == person)
        .ok_or_else(invalid)?;
    let mut terms = Vec::new();
    let mut constant = 0_i128;
    match workload.target.kind {
        WorkloadTargetKind::Explicit | WorkloadTargetKind::PersonTargets => {
            constant = -i128::from(*workload.target.targets.get(&person).ok_or_else(invalid)?)
                .checked_mul(i128::from(workload.units_per_target_unit))
                .ok_or_else(arithmetic)?;
            terms.push((index, 1_i128));
        }
        WorkloadTargetKind::EqualShare => {
            let count = i128::try_from(bounds.len()).map_err(|_| arithmetic())?;
            terms.push((index, count - 1));
            for (other, _) in bounds.iter().enumerate() {
                if other != index {
                    terms.push((other, -1));
                }
            }
        }
        WorkloadTargetKind::WeightedEqualShare => {
            let denominator = i128::from(workload.target.denominator);
            let weight = i128::from(*workload.target.weights.get(&person).ok_or_else(invalid)?);
            terms.push((index, denominator - weight));
            for (other, _) in periods.iter().enumerate() {
                if other != index {
                    terms.push((other, -weight));
                }
            }
        }
    }
    let mut lower = constant;
    let mut upper = constant;
    for (position, coefficient) in terms {
        let (minimum, maximum) = if position == index {
            (actual_lower, actual_upper)
        } else {
            *bounds.get(position).ok_or_else(invalid)?
        };
        if coefficient >= 0 {
            lower = lower
                .checked_add(coefficient * i128::from(minimum))
                .ok_or_else(arithmetic)?;
            upper = upper
                .checked_add(coefficient * i128::from(maximum))
                .ok_or_else(arithmetic)?;
        } else {
            lower = lower
                .checked_add(coefficient * i128::from(maximum))
                .ok_or_else(arithmetic)?;
            upper = upper
                .checked_add(coefficient * i128::from(minimum))
                .ok_or_else(arithmetic)?;
        }
    }
    Ok((
        i64::try_from(lower).map_err(|_| arithmetic())?,
        i64::try_from(upper).map_err(|_| arithmetic())?,
    ))
}

fn piecewise_cost(
    difference: i128,
    denominator: i128,
    penalty: &DeviationPenalty,
) -> Result<i128, AssignmentRuleError> {
    if denominator <= 0 || difference < 0 {
        return Err(invalid());
    }
    let breakpoints = match penalty {
        DeviationPenalty::Absolute {} => return Ok(difference),
        DeviationPenalty::Piecewise { breakpoints } => breakpoints,
    };
    let mut segment = 0;
    for (index, point) in breakpoints.iter().enumerate() {
        if difference >= i128::from(point.from_deviation) * denominator {
            segment = index;
        } else {
            break;
        }
    }
    let mut base = 0_i128;
    for pair in breakpoints[..=segment].windows(2) {
        let span = i128::from(pair[1].from_deviation - pair[0].from_deviation);
        base = base
            .checked_add(span * i128::from(pair[0].slope))
            .ok_or_else(arithmetic)?;
    }
    let point = breakpoints.get(segment).ok_or_else(invalid)?;
    let offset = difference
        .checked_sub(i128::from(point.from_deviation) * denominator)
        .ok_or_else(arithmetic)?;
    denominator
        .checked_mul(base)
        .and_then(|value| value.checked_add(i128::from(point.slope) * offset))
        .ok_or_else(arithmetic)
}

#[allow(clippy::too_many_lines)]
pub(super) fn shape(
    input: &AssignmentInput,
    plan: &PreferencePlan<'_>,
    candidates: &[AssignmentPair],
    settings: &ScenarioSettings,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<PreferenceShape, AssignmentRuleError> {
    if !plan.active() {
        return Ok(PreferenceShape {
            term_count: 0,
            pruned_base: Vec::new(),
            level_lower_bounds: vec![0; plan.levels.len()],
            level_upper_bounds: vec![0; plan.levels.len()],
            targets: Vec::new(),
            weekend_targets: Vec::new(),
            workloads: WorkloadShape {
                periods: Vec::new(),
            },
            consecutives: preference_consecutive::Shape::default(),
            groups: Vec::new(),
            coverage: Vec::new(),
        });
    }
    let mut level_upper_bounds = vec![0_i64; plan.levels.len()];
    let mut level_lower_bounds = vec![0_i64; plan.levels.len()];
    let mut term_count = 0_u64;
    let mut pruned_base = Vec::new();
    for entry in &plan.entries {
        for pair in candidates {
            budget.step()?;
            if !matches(input, *entry, *pair, settings, budget)? {
                continue;
            }
            term_count = term_count.checked_add(1).ok_or_else(arithmetic)?;
            let upper = level_upper_bounds
                .get_mut(entry.level)
                .ok_or_else(invalid)?;
            *upper = upper
                .checked_add(entry.coefficient)
                .ok_or_else(arithmetic)?;
        }
    }
    for (entry_index, entry) in plan.entries.iter().enumerate() {
        let Some(base_assignments) = entry.base_assignments else {
            continue;
        };
        for &pair in base_assignments {
            budget.step()?;
            budget.steps(selection_search_steps(candidates.len()))?;
            if candidates.binary_search(&pair).is_ok()
                || input.shift(pair.shift_id).is_none()
                || !matches(input, *entry, pair, settings, budget)?
            {
                continue;
            }
            budget.reserve(0, 1, count(size_of::<PrunedBaseShape>())?)?;
            pruned_base.push(PrunedBaseShape {
                entry: entry_index,
                pair,
            });
            term_count = term_count.checked_add(1).ok_or_else(arithmetic)?;
            let lower = level_lower_bounds
                .get_mut(entry.level)
                .ok_or_else(invalid)?;
            *lower = lower
                .checked_add(entry.coefficient)
                .ok_or_else(arithmetic)?;
            let upper = level_upper_bounds
                .get_mut(entry.level)
                .ok_or_else(invalid)?;
            *upper = upper
                .checked_add(entry.coefficient)
                .ok_or_else(arithmetic)?;
        }
    }
    let groups = group_shapes(input, plan, candidates, budget)?;
    for group in &groups {
        let preference = plan.groups.get(group.plan).ok_or_else(invalid)?;
        term_count = term_count
            .checked_add(count(group.pairs.len())?)
            .ok_or_else(arithmetic)?;
        let upper = preference
            .coefficient
            .checked_mul(i64::try_from(group.pairs.len()).map_err(|_| arithmetic())?)
            .ok_or_else(arithmetic)?;
        let level = level_upper_bounds
            .get_mut(preference.level)
            .ok_or_else(invalid)?;
        *level = level.checked_add(upper).ok_or_else(arithmetic)?;
    }
    let coverage = coverage_shapes(input, plan, candidates, budget)?;
    for shape in &coverage {
        let preference = plan.coverage.get(shape.plan).ok_or_else(invalid)?;
        term_count = term_count.checked_add(1).ok_or_else(arithmetic)?;
        let upper = preference
            .coefficient
            .checked_mul(shape.difference_upper)
            .ok_or_else(arithmetic)?;
        let level = level_upper_bounds
            .get_mut(preference.level)
            .ok_or_else(invalid)?;
        *level = level.checked_add(upper).ok_or_else(arithmetic)?;
        if shape.population.is_empty() {
            let lower = level_lower_bounds
                .get_mut(preference.level)
                .ok_or_else(invalid)?;
            *lower = lower.checked_add(upper).ok_or_else(arithmetic)?;
        }
    }
    let mut targets = Vec::new();
    for (target_index, target) in plan.targets.iter().enumerate() {
        for (period_index, period) in target.periods.iter().enumerate() {
            let mut population = Vec::new();
            for (index, pair) in candidates.iter().enumerate() {
                budget.step()?;
                if pair.person_id != period.person || period.pairs.binary_search(pair).is_err() {
                    continue;
                }
                budget.reserve(0, 1, 8)?;
                population.push(index);
            }
            if population.is_empty() && target.target_count == 0 {
                // An unreachable zero target contributes neither a penalty nor an objective term.
                continue;
            }
            let difference = target_difference_upper(target.target_count, population.len())?;
            let upper = difference
                .checked_mul(target.coefficient)
                .ok_or_else(arithmetic)?;
            term_count = term_count.checked_add(1).ok_or_else(arithmetic)?;
            if population.is_empty() {
                let lower = level_lower_bounds
                    .get_mut(target.level)
                    .ok_or_else(invalid)?;
                *lower = lower.checked_add(upper).ok_or_else(arithmetic)?;
            }
            let level_upper = level_upper_bounds
                .get_mut(target.level)
                .ok_or_else(invalid)?;
            *level_upper = level_upper.checked_add(upper).ok_or_else(arithmetic)?;
            targets.push(TargetShape {
                target: target_index,
                period: period_index,
                population,
                difference_upper: difference,
            });
        }
    }
    let weekend_targets = weekend_target_shapes(plan, candidates, budget)?;
    for shape in &weekend_targets {
        let target = plan.weekend_targets.get(shape.target).ok_or_else(invalid)?;
        term_count = term_count.checked_add(1).ok_or_else(arithmetic)?;
        let upper = target
            .coefficient
            .checked_mul(shape.difference_upper)
            .ok_or_else(arithmetic)?;
        let level_upper = level_upper_bounds
            .get_mut(target.level)
            .ok_or_else(invalid)?;
        *level_upper = level_upper.checked_add(upper).ok_or_else(arithmetic)?;
        if shape.weekends.is_empty() {
            let level_lower = level_lower_bounds
                .get_mut(target.level)
                .ok_or_else(invalid)?;
            *level_lower = level_lower.checked_add(upper).ok_or_else(arithmetic)?;
        }
    }
    let workloads = workload_shape(input, plan, candidates, budget)?;
    for period in &workloads.periods {
        budget.step()?;
        let workload = plan.workloads.get(period.workload).ok_or_else(invalid)?;
        let level = level_upper_bounds
            .get_mut(workload.level)
            .ok_or_else(invalid)?;
        for peer in &period.peers {
            budget.step()?;
            term_count = term_count.checked_add(1).ok_or_else(arithmetic)?;
            let upper = workload
                .coefficient
                .checked_mul(peer.cost_upper)
                .ok_or_else(arithmetic)?;
            *level = level.checked_add(upper).ok_or_else(arithmetic)?;
        }
    }
    let consecutives =
        preference_consecutive::shape(&plan.consecutives, candidates, limits, budget)?;
    term_count = term_count
        .checked_add(consecutives.term_count)
        .ok_or_else(arithmetic)?;
    for window in &consecutives.windows {
        budget.step()?;
        let consecutive = plan.consecutives.get(window.plan).ok_or_else(invalid)?;
        let level = level_upper_bounds
            .get_mut(consecutive.level)
            .ok_or_else(invalid)?;
        *level = level
            .checked_add(consecutive.coefficient)
            .ok_or_else(arithmetic)?;
    }
    Ok(PreferenceShape {
        term_count,
        pruned_base,
        level_lower_bounds,
        level_upper_bounds,
        targets,
        weekend_targets,
        workloads,
        consecutives,
        groups,
        coverage,
    })
}
fn weekend_target_shapes(
    plan: &PreferencePlan<'_>,
    candidates: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<WeekendTargetShape>, AssignmentRuleError> {
    let mut shapes = Vec::new();
    for (target_index, target) in plan.weekend_targets.iter().enumerate() {
        budget.step()?;
        for (person_index, person) in target.people.iter().enumerate() {
            budget.step()?;
            let mut weekends = Vec::new();
            for source_weekend in &person.weekends {
                budget.step()?;
                let mut population = Vec::new();
                for (index, pair) in candidates.iter().enumerate() {
                    budget.step()?;
                    if pair.person_id != person.person {
                        continue;
                    }
                    budget.steps(selection_search_steps(source_weekend.pairs.len()))?;
                    if source_weekend.pairs.binary_search(pair).is_err() {
                        continue;
                    }
                    budget.reserve(0, 1, 8)?;
                    population.push(index);
                }
                if population.is_empty() {
                    continue;
                }
                budget.reserve(1, 1, budget.measure(&population.len())?)?;
                weekends.push(WeekendTargetWeekendShape {
                    date: source_weekend.date,
                    population,
                });
            }
            if weekends.is_empty() && target.target_count == 0 {
                continue;
            }
            let difference = target_difference_upper(target.target_count, weekends.len())?;
            budget.reserve(1, 1, budget.measure(&(person.person, weekends.len()))?)?;
            shapes.push(WeekendTargetShape {
                target: target_index,
                person: person_index,
                weekends,
                difference_upper: difference,
            });
        }
    }
    Ok(shapes)
}

fn group_shapes(
    input: &AssignmentInput,
    plan: &PreferencePlan<'_>,
    candidates: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<GroupShape>, AssignmentRuleError> {
    let mut shapes = Vec::new();
    for (plan_index, group) in plan.groups.iter().enumerate() {
        budget.step()?;
        let mut scoped = Vec::new();
        for (index, pair) in candidates.iter().enumerate() {
            budget.step()?;
            if !group.person_ids.contains(&pair.person_id) {
                budget.steps(count(group.person_ids.len())?)?;
                continue;
            }
            let person = input.person(pair.person_id).ok_or_else(invalid)?;
            let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
            let metadata = input.metadata(shift)?;
            if support::person_scope(group.scope, person, budget)?
                && support::shift_scope(group.scope, shift, &metadata, budget)?
            {
                budget.reserve(0, 1, 8)?;
                scoped.push(index);
            }
        }
        let mut pairs = Vec::new();
        for first in 0..scoped.len() {
            budget.step()?;
            for second in first + 1..scoped.len() {
                budget.step()?;
                let first_index = *scoped.get(first).ok_or_else(invalid)?;
                let second_index = *scoped.get(second).ok_or_else(invalid)?;
                let first_pair = *candidates.get(first_index).ok_or_else(invalid)?;
                let second_pair = *candidates.get(second_index).ok_or_else(invalid)?;
                if first_pair.person_id == second_pair.person_id {
                    continue;
                }
                let relation = relation_matches(input, first_pair, second_pair, group.mode)?;
                let mismatch = match group.direction {
                    GroupDirection::Together => !relation,
                    GroupDirection::Separate => relation,
                };
                if mismatch {
                    budget.reserve(0, 1, 16)?;
                    pairs.push(GroupPairShape {
                        first: first_index,
                        second: second_index,
                    });
                }
            }
        }
        if !pairs.is_empty() {
            shapes.push(GroupShape {
                plan: plan_index,
                pairs,
            });
        }
    }
    Ok(shapes)
}

fn coverage_shapes(
    input: &AssignmentInput,
    plan: &PreferencePlan<'_>,
    candidates: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<CoverageShape>, AssignmentRuleError> {
    let mut shapes = Vec::new();
    for (plan_index, coverage_plan) in plan.coverage.iter().enumerate() {
        budget.step()?;
        for requirement_id in coverage_plan.coverage_requirement_ids {
            budget.step()?;
            let Some(WorkforceEntity::CoverageRequirement(requirement)) =
                input.domain.entities.get(&requirement_id.as_entity_id())
            else {
                return Err(invalid());
            };
            if !requirement.active {
                continue;
            }
            let preferred_count = match &requirement.coverage {
                Coverage::AtLeast {
                    preferred_count: Some(value),
                    ..
                } => *value,
                Coverage::AtLeast {
                    preferred_count: None,
                    ..
                }
                | Coverage::Exact { .. } => return Err(invalid()),
            };
            if preferred_count == 0 {
                continue;
            }
            for shift in &input.shifts {
                budget.step()?;
                let metadata = input.metadata(shift)?;
                if !requirement_shift_matches(&requirement.scope, shift, &metadata, budget)?
                    || !support::shift_scope(coverage_plan.scope, shift, &metadata, budget)?
                {
                    continue;
                }
                let mut population = Vec::new();
                for (index, pair) in candidates.iter().enumerate() {
                    budget.step()?;
                    if pair.shift_id != shift.id {
                        continue;
                    }
                    let person = input.person(pair.person_id).ok_or_else(invalid)?;
                    if support::person_scope(coverage_plan.scope, person, budget)? {
                        budget.reserve(0, 1, 8)?;
                        population.push(index);
                    }
                }
                shapes.push(CoverageShape {
                    plan: plan_index,
                    requirement_id: *requirement_id,
                    shift_id: shift.id,
                    population,
                    preferred_count,
                    difference_upper: i64::from(preferred_count),
                });
            }
        }
    }
    Ok(shapes)
}

fn target_difference_upper(
    target_count: u32,
    population: usize,
) -> Result<i64, AssignmentRuleError> {
    let target = u64::from(target_count);
    let population = count(population)?;
    let difference = target.max(population.abs_diff(target));
    i64::try_from(difference).map_err(|_| arithmetic())
}

pub(super) struct PreferenceIds {
    pub term: ObjectiveTermId,
    pub fact: ProvenanceId,
    pub base_member: bool,
}

impl PreferenceIds {
    pub fn derive(
        entry: PreferenceEntry<'_>,
        pair: AssignmentPair,
        identities: &mut PlanningIdentities,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let key = ("preference", entry.id, pair);
        let base_member = if matches!(entry.variant, PreferenceVariant::BaseStability) {
            base_contains(entry, pair, budget)?
        } else {
            false
        };
        Ok(Self {
            term: ObjectiveTermId::new(identities.derive(
                IdentityKind::ObjectiveTerm,
                &key,
                budget,
            )?)
            .map_err(|_| invalid())?,
            fact: ProvenanceId::new(identities.derive(IdentityKind::Provenance, &key, budget)?)
                .map_err(|_| invalid())?,
            base_member,
        })
    }
}

pub(super) struct PreferenceEmission {
    pub term: ObjectiveTerm,
    pub fact: ProvenanceRecord,
}

pub(super) fn emit(
    entry: PreferenceEntry<'_>,
    integer: IntVariableId,
    decision_provenance: ProvenanceId,
    ids: PreferenceIds,
    budget: &mut OperationBudget<'_>,
) -> Result<PreferenceEmission, AssignmentRuleError> {
    budget.steps(8)?;
    let (constant, coefficient) = match entry.variant {
        PreferenceVariant::BaseStability if ids.base_member => {
            (entry.coefficient, -entry.coefficient)
        }
        PreferenceVariant::BaseStability => (0, entry.coefficient),
        _ => match (entry.variant, entry.direction) {
            (_, Some(PreferenceDirection::Prefer)) => (entry.coefficient, -entry.coefficient),
            (PreferenceVariant::RequestedTimeOff, None) | (_, Some(PreferenceDirection::Avoid)) => {
                (0, entry.coefficient)
            }
            _ => return Err(invalid()),
        },
    };
    let expression = LinearExpression {
        constant,
        terms: vec![LinearTerm {
            variable: integer,
            coefficient,
        }],
    };
    let category = category_id(entry.variant)?;
    let fact = preference_fact(entry, decision_provenance, ids.fact.clone());
    Ok(PreferenceEmission {
        term: ObjectiveTerm {
            id: ids.term,
            expression,
            kind: ObjectiveTermKind::Penalty,
            category,
            provenance: fact.id.clone(),
        },
        fact,
    })
}

pub(super) fn emit_pruned_base(
    entry: PreferenceEntry<'_>,
    parent: ProvenanceId,
    ids: PreferenceIds,
    budget: &mut OperationBudget<'_>,
) -> Result<PreferenceEmission, AssignmentRuleError> {
    budget.steps(8)?;
    if !matches!(entry.variant, PreferenceVariant::BaseStability) || !ids.base_member {
        return Err(invalid());
    }
    let fact = preference_fact(entry, parent, ids.fact.clone());
    Ok(PreferenceEmission {
        term: ObjectiveTerm {
            id: ids.term,
            expression: LinearExpression {
                constant: entry.coefficient,
                terms: Vec::new(),
            },
            kind: ObjectiveTermKind::Penalty,
            category: category_id(entry.variant)?,
            provenance: fact.id.clone(),
        },
        fact,
    })
}

fn preference_fact(
    entry: PreferenceEntry<'_>,
    parent: ProvenanceId,
    id: ProvenanceId,
) -> ProvenanceRecord {
    let message_key = match entry.variant {
        PreferenceVariant::Time => "official.workforce.preference.time",
        PreferenceVariant::AssignmentType => "official.workforce.preference.assignment_type",
        PreferenceVariant::Location => "official.workforce.preference.location",
        PreferenceVariant::RequestedTimeOff => "official.workforce.preference.requested_time_off",
        PreferenceVariant::Adjacency => "official.workforce.preference.adjacency",
        PreferenceVariant::BaseStability => "official.workforce.preference.base_stability",
    };
    let mut parameters = BTreeMap::from([(
        "coefficient".to_owned(),
        ProvenanceParameter::Integer(entry.coefficient),
    )]);
    if let Some(direction) = entry.direction {
        parameters.insert(
            "direction".to_owned(),
            ProvenanceParameter::Text(
                match direction {
                    PreferenceDirection::Prefer => "prefer",
                    PreferenceDirection::Avoid => "avoid",
                }
                .to_owned(),
            ),
        );
    }
    ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Preference,
        source_id: entry.id.to_string(),
        entity_refs: Vec::new(),
        message_key: message_key.to_owned(),
        parameters,
        parent: Some(parent),
    }
}
pub(super) struct GroupIds {
    pub helper: IntVariableId,
    pub first_constraint: PlanningConstraintId,
    pub second_constraint: PlanningConstraintId,
    pub lower_constraint: PlanningConstraintId,
    pub term: ObjectiveTermId,
    pub fact: ProvenanceId,
}

impl GroupIds {
    pub fn derive(
        plan: &GroupPlan<'_>,
        first: AssignmentPair,
        second: AssignmentPair,
        identities: &mut PlanningIdentities,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let key = ("preference_group", plan.id, first, second);
        let derive_int = |suffix: &str,
                          identities: &mut PlanningIdentities,
                          budget: &mut OperationBudget<'_>|
         -> Result<IntVariableId, AssignmentRuleError> {
            IntVariableId::new(identities.derive(IdentityKind::Integer, &(suffix, key), budget)?)
                .map_err(|_| invalid())
        };
        let derive_constraint = |suffix: &str,
                                 identities: &mut PlanningIdentities,
                                 budget: &mut OperationBudget<'_>|
         -> Result<PlanningConstraintId, AssignmentRuleError> {
            PlanningConstraintId::new(identities.derive(
                IdentityKind::Constraint,
                &(suffix, key),
                budget,
            )?)
            .map_err(|_| invalid())
        };
        Ok(Self {
            helper: derive_int("preference_group_helper", identities, budget)?,
            first_constraint: derive_constraint("preference_group_first", identities, budget)?,
            second_constraint: derive_constraint("preference_group_second", identities, budget)?,
            lower_constraint: derive_constraint("preference_group_lower", identities, budget)?,
            term: ObjectiveTermId::new(identities.derive(
                IdentityKind::ObjectiveTerm,
                &("preference_group_term", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
            fact: ProvenanceId::new(identities.derive(
                IdentityKind::Provenance,
                &("preference_group_fact", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
        })
    }
}

pub(super) struct GroupEmission {
    pub variable: Variable,
    pub constraints: [ConstraintRecord; 3],
    pub term: ObjectiveTerm,
    pub fact: ProvenanceRecord,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_group(
    plan: &GroupPlan<'_>,
    first: AssignmentPair,
    second: AssignmentPair,
    first_integer: &IntVariableId,
    second_integer: &IntVariableId,
    parent: ProvenanceId,
    ids: GroupIds,
    budget: &mut OperationBudget<'_>,
) -> Result<GroupEmission, AssignmentRuleError> {
    budget.steps(24)?;
    let fact = group_fact(plan, first, second, ids.fact.clone(), parent, budget)?;
    let helper = ids.helper.clone();
    let linear =
        |id, coefficients: [i64; 3], op, rhs| -> Result<ConstraintRecord, AssignmentRuleError> {
            let terms = [
                (&helper, &coefficients[0]),
                (first_integer, &coefficients[1]),
                (second_integer, &coefficients[2]),
            ]
            .into_iter()
            .filter(|(_, coefficient)| **coefficient != 0)
            .map(|(variable, coefficient)| LinearTerm {
                variable: variable.clone(),
                coefficient: *coefficient,
            })
            .collect();
            Ok(ConstraintRecord {
                id,
                body: Constraint::LinearComparison(LinearComparison {
                    expression: LinearExpression::new(terms, 0).map_err(|_| invalid())?,
                    op,
                    rhs,
                }),
                enforcement: Vec::new(),
                provenance: fact.id.clone(),
                tags: Vec::new(),
            })
        };
    let constraints = [
        linear(
            ids.first_constraint,
            [1, -1, 0],
            ComparisonOp::LessOrEqual,
            0,
        )?,
        linear(
            ids.second_constraint,
            [1, 0, -1],
            ComparisonOp::LessOrEqual,
            0,
        )?,
        linear(
            ids.lower_constraint,
            [1, -1, -1],
            ComparisonOp::GreaterOrEqual,
            -1,
        )?,
    ];
    Ok(GroupEmission {
        variable: Variable::Integer(IntVariable {
            id: helper.clone(),
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange { start: 0, end: 1 }],
            },
            provenance: fact.id.clone(),
        }),
        constraints,
        term: ObjectiveTerm {
            id: ids.term,
            expression: LinearExpression::new(
                vec![LinearTerm {
                    variable: helper,
                    coefficient: plan.coefficient,
                }],
                0,
            )
            .map_err(|_| invalid())?,
            kind: ObjectiveTermKind::Penalty,
            category: ScoreCategoryId::new(TOGETHER_SEPARATE_CATEGORY).map_err(|_| invalid())?,
            provenance: fact.id.clone(),
        },
        fact,
    })
}

fn group_fact(
    plan: &GroupPlan<'_>,
    first: AssignmentPair,
    second: AssignmentPair,
    id: ProvenanceId,
    parent: ProvenanceId,
    budget: &mut OperationBudget<'_>,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    budget.reserve(0, 8, 384)?;
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Preference,
        source_id: plan.id.to_string(),
        entity_refs: Vec::new(),
        message_key: "official.workforce.preference.together_separate".to_owned(),
        parameters: BTreeMap::from([
            (
                "coefficient".to_owned(),
                ProvenanceParameter::Integer(plan.coefficient),
            ),
            (
                "direction".to_owned(),
                ProvenanceParameter::Text(
                    match plan.direction {
                        GroupDirection::Together => "together",
                        GroupDirection::Separate => "separate",
                    }
                    .to_owned(),
                ),
            ),
            (
                "mode".to_owned(),
                ProvenanceParameter::Text(
                    match plan.mode {
                        PairingMode::SameShift => "sameShift",
                        PairingMode::OverlappingShifts => "overlappingShifts",
                    }
                    .to_owned(),
                ),
            ),
            (
                "first_person".to_owned(),
                ProvenanceParameter::Text(first.person_id.to_string()),
            ),
            (
                "first_shift".to_owned(),
                ProvenanceParameter::Text(first.shift_id.to_string()),
            ),
            (
                "second_person".to_owned(),
                ProvenanceParameter::Text(second.person_id.to_string()),
            ),
            (
                "second_shift".to_owned(),
                ProvenanceParameter::Text(second.shift_id.to_string()),
            ),
        ]),
        parent: Some(parent),
    })
}

pub(super) struct CoverageIds {
    pub difference: IntVariableId,
    pub constraint: PlanningConstraintId,
    pub term: ObjectiveTermId,
    pub fact: ProvenanceId,
}

impl CoverageIds {
    pub fn derive(
        plan: &CoveragePlan<'_>,
        shape: &CoverageShape,
        identities: &mut PlanningIdentities,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let key = (
            "preference_coverage",
            plan.id,
            shape.requirement_id,
            shape.shift_id,
        );
        Ok(Self {
            difference: IntVariableId::new(identities.derive(
                IdentityKind::Integer,
                &("preference_coverage_difference", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
            constraint: PlanningConstraintId::new(identities.derive(
                IdentityKind::Constraint,
                &("preference_coverage_constraint", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
            term: ObjectiveTermId::new(identities.derive(
                IdentityKind::ObjectiveTerm,
                &("preference_coverage_term", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
            fact: ProvenanceId::new(identities.derive(
                IdentityKind::Provenance,
                &("preference_coverage_fact", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
        })
    }
}

pub(super) struct CoverageEmission {
    pub variables: Vec<Variable>,
    pub constraints: Vec<ConstraintRecord>,
    pub term: ObjectiveTerm,
    pub fact: ProvenanceRecord,
}

pub(super) fn emit_coverage(
    plan: &CoveragePlan<'_>,
    shape: &CoverageShape,
    population: &[IntVariableId],
    parent: ProvenanceId,
    ids: CoverageIds,
    budget: &mut OperationBudget<'_>,
) -> Result<CoverageEmission, AssignmentRuleError> {
    budget.steps(
        count(population.len())?
            .checked_add(16)
            .ok_or_else(arithmetic)?,
    )?;
    let preferred = i64::from(shape.preferred_count);
    let fact = coverage_fact(
        plan,
        shape,
        population.len(),
        ids.fact.clone(),
        parent,
        budget,
    )?;
    if population.is_empty() {
        return Ok(CoverageEmission {
            variables: Vec::new(),
            constraints: Vec::new(),
            term: ObjectiveTerm {
                id: ids.term,
                expression: LinearExpression {
                    constant: plan
                        .coefficient
                        .checked_mul(preferred)
                        .ok_or_else(arithmetic)?,
                    terms: Vec::new(),
                },
                kind: ObjectiveTermKind::Penalty,
                category: ScoreCategoryId::new(PREFERRED_COVERAGE_CATEGORY)
                    .map_err(|_| invalid())?,
                provenance: fact.id.clone(),
            },
            fact,
        });
    }
    let mut terms = vec![LinearTerm {
        variable: ids.difference.clone(),
        coefficient: 1,
    }];
    terms.extend(population.iter().cloned().map(|variable| LinearTerm {
        variable,
        coefficient: 1,
    }));
    let constraint = ConstraintRecord {
        id: ids.constraint,
        body: Constraint::LinearComparison(LinearComparison {
            expression: LinearExpression::new(terms, 0).map_err(|_| invalid())?,
            op: ComparisonOp::GreaterOrEqual,
            rhs: preferred,
        }),
        enforcement: Vec::new(),
        provenance: fact.id.clone(),
        tags: Vec::new(),
    };
    Ok(CoverageEmission {
        variables: vec![Variable::Integer(IntVariable {
            id: ids.difference.clone(),
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange {
                    start: 0,
                    end: preferred,
                }],
            },
            provenance: fact.id.clone(),
        })],
        constraints: vec![constraint],
        term: ObjectiveTerm {
            id: ids.term,
            expression: LinearExpression {
                constant: 0,
                terms: vec![LinearTerm {
                    variable: ids.difference,
                    coefficient: plan.coefficient,
                }],
            },
            kind: ObjectiveTermKind::Penalty,
            category: ScoreCategoryId::new(PREFERRED_COVERAGE_CATEGORY).map_err(|_| invalid())?,
            provenance: fact.id.clone(),
        },
        fact,
    })
}

fn coverage_fact(
    plan: &CoveragePlan<'_>,
    shape: &CoverageShape,
    population: usize,
    id: ProvenanceId,
    parent: ProvenanceId,
    budget: &mut OperationBudget<'_>,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    budget.reserve(0, 7, 384)?;
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Preference,
        source_id: plan.id.to_string(),
        entity_refs: Vec::new(),
        message_key: "official.workforce.preference.preferred_coverage".to_owned(),
        parameters: BTreeMap::from([
            (
                "coefficient".to_owned(),
                ProvenanceParameter::Integer(plan.coefficient),
            ),
            (
                "requirement_id".to_owned(),
                ProvenanceParameter::Text(shape.requirement_id.to_string()),
            ),
            (
                "shift_id".to_owned(),
                ProvenanceParameter::Text(shape.shift_id.to_string()),
            ),
            (
                "preferred_count".to_owned(),
                ProvenanceParameter::Integer(i64::from(shape.preferred_count)),
            ),
            (
                "selectable_count".to_owned(),
                ProvenanceParameter::Integer(i64::try_from(population).map_err(|_| arithmetic())?),
            ),
        ]),
        parent: Some(parent),
    })
}

struct WorkloadSplitIds {
    whole: IntVariableId,
    remainder: IntVariableId,
    whole_constraint: PlanningConstraintId,
    remainder_constraint: PlanningConstraintId,
}

pub(super) struct WorkloadIds {
    pub actual: IntVariableId,
    pub delta: IntVariableId,
    pub difference: IntVariableId,
    pub zero: IntVariableId,
    pub cost: IntVariableId,
    split: Option<WorkloadSplitIds>,
    pub segments: Vec<IntVariableId>,
    pub selectors: Vec<BoolVariableId>,
    pub actual_constraint: PlanningConstraintId,
    pub delta_constraint: PlanningConstraintId,
    pub difference_constraint: PlanningConstraintId,
    pub union_constraints: Vec<PlanningConstraintId>,
    pub exactly_one: Option<PlanningConstraintId>,
    pub lower_constraints: Vec<PlanningConstraintId>,
    pub upper_constraints: Vec<PlanningConstraintId>,
    pub relation_constraints: Vec<PlanningConstraintId>,
    pub term: ObjectiveTermId,
    pub fact: ProvenanceId,
}

impl WorkloadIds {
    #[allow(clippy::too_many_lines)]
    pub fn derive(
        workload: &WorkloadPlan<'_>,
        period: &WorkloadPeriod,
        peer: &WorkloadPeerShape,
        identities: &mut PlanningIdentities,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let key = ("workload_balance", workload.id, period.period, peer.person);
        macro_rules! integer {
            ($suffix:expr) => {{
                IntVariableId::new(identities.derive(
                    IdentityKind::Integer,
                    &($suffix, key),
                    budget,
                )?)
                .map_err(|_| invalid())
            }};
        }
        macro_rules! boolean {
            ($index:expr) => {{
                BoolVariableId::new(identities.derive(
                    IdentityKind::Boolean,
                    &("workload_selector", key, $index),
                    budget,
                )?)
                .map_err(|_| invalid())
            }};
        }
        macro_rules! constraint {
            ($suffix:expr) => {{
                PlanningConstraintId::new(identities.derive(
                    IdentityKind::Constraint,
                    &($suffix, key),
                    budget,
                )?)
                .map_err(|_| invalid())
            }};
            ($suffix:expr, $index:expr) => {{
                PlanningConstraintId::new(identities.derive(
                    IdentityKind::Constraint,
                    &($suffix, key, $index),
                    budget,
                )?)
                .map_err(|_| invalid())
            }};
        }
        let actual = integer!("workload_actual")?;
        let delta = integer!("workload_delta")?;
        let difference = integer!("workload_difference")?;
        let zero = integer!("workload_zero")?;
        let cost = integer!("workload_cost")?;
        let split = peer
            .split_bounds
            .map(|_| {
                Ok::<_, AssignmentRuleError>(WorkloadSplitIds {
                    whole: integer!("workload_whole_minutes")?,
                    remainder: integer!("workload_minute_remainder")?,
                    whole_constraint: constraint!("workload_whole_minutes_constraint")?,
                    remainder_constraint: constraint!("workload_minute_remainder_constraint")?,
                })
            })
            .transpose()?;
        let mut segments = Vec::new();
        let mut union_constraints = Vec::new();
        for (index, segment) in peer.segments.iter().enumerate() {
            budget.step()?;
            segments.push(
                IntVariableId::new(identities.derive(
                    IdentityKind::Integer,
                    &("workload_union_segment", key, index),
                    budget,
                )?)
                .map_err(|_| invalid())?,
            );
            for member in 0..segment.population.len().saturating_add(1) {
                union_constraints.push(
                    PlanningConstraintId::new(identities.derive(
                        IdentityKind::Constraint,
                        &("workload_union", key, index, member),
                        budget,
                    )?)
                    .map_err(|_| invalid())?,
                );
            }
        }
        let breakpoints = match &workload.policy.penalty {
            DeviationPenalty::Absolute {} => 0,
            DeviationPenalty::Piecewise { breakpoints } => breakpoints.len(),
        };
        let mut selectors = Vec::new();
        let mut lower_constraints = Vec::new();
        let mut upper_constraints = Vec::new();
        let mut relation_constraints = Vec::new();
        for index in 0..breakpoints {
            selectors.push(boolean!(index)?);
            lower_constraints.push(constraint!("workload_piecewise_lower", index)?);
            relation_constraints.push(constraint!("workload_piecewise_relation", index)?);
            if index + 1 < breakpoints {
                upper_constraints.push(constraint!("workload_piecewise_upper", index)?);
            }
        }
        Ok(Self {
            actual,
            delta,
            difference,
            zero,
            cost,
            split,
            segments,
            selectors,
            actual_constraint: constraint!("workload_actual_constraint")?,
            delta_constraint: constraint!("workload_delta_constraint")?,
            difference_constraint: constraint!("workload_difference_constraint")?,
            union_constraints,
            exactly_one: (breakpoints > 0)
                .then(|| constraint!("workload_piecewise_exactly_one"))
                .transpose()?,
            lower_constraints,
            upper_constraints,
            relation_constraints,
            term: ObjectiveTermId::new(identities.derive(
                IdentityKind::ObjectiveTerm,
                &("workload_term", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
            fact: ProvenanceId::new(identities.derive(
                IdentityKind::Provenance,
                &("workload_fact", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
        })
    }
}

pub(super) struct WorkloadEmission {
    pub variables: Vec<Variable>,
    pub constraints: Vec<ConstraintRecord>,
    pub term: ObjectiveTerm,
    pub fact: ProvenanceRecord,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn emit_workload(
    workload: &WorkloadPlan<'_>,
    period: &WorkloadPeriod,
    peer: &WorkloadPeerShape,
    peer_actuals: &[IntVariableId],
    candidate_integers: &[IntVariableId],
    parent: ProvenanceId,
    ids: WorkloadIds,
    budget: &mut OperationBudget<'_>,
) -> Result<WorkloadEmission, AssignmentRuleError> {
    budget.steps(
        count(peer.terms.len())?
            .checked_add(
                count(peer.segments.len())?
                    .checked_mul(4)
                    .ok_or_else(arithmetic)?,
            )
            .and_then(|value| value.checked_add(16))
            .ok_or_else(arithmetic)?,
    )?;
    let int_variable = |id: IntVariableId, start: i64, end: i64| {
        Variable::Integer(IntVariable {
            id,
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange { start, end }],
            },
            provenance: ids.fact.clone(),
        })
    };
    let mut variables = vec![
        int_variable(ids.actual.clone(), peer.actual_lower, peer.actual_upper),
        int_variable(ids.delta.clone(), peer.delta_lower, peer.delta_upper),
        int_variable(ids.difference.clone(), 0, peer.difference_upper),
        int_variable(ids.zero.clone(), 0, 0),
    ];
    for segment in &ids.segments {
        variables.push(int_variable(segment.clone(), 0, 1));
    }
    let mut constraints = Vec::new();
    let actual_expression =
        if let (Some(split), Some((whole_lower, whole_upper, remainder_upper))) =
            (&ids.split, peer.split_bounds)
        {
            variables.push(int_variable(split.whole.clone(), whole_lower, whole_upper));
            variables.push(int_variable(split.remainder.clone(), 0, remainder_upper));
            for (variable, constraint_id, component) in [
                (
                    &split.whole,
                    &split.whole_constraint,
                    WorkloadComponent::Whole,
                ),
                (
                    &split.remainder,
                    &split.remainder_constraint,
                    WorkloadComponent::Remainder,
                ),
            ] {
                constraints.push(ConstraintRecord {
                    id: constraint_id.clone(),
                    body: Constraint::LinearComparison(LinearComparison {
                        expression: LinearExpression::new(
                            workload_actual_terms(
                                peer,
                                &ids,
                                candidate_integers,
                                variable,
                                component,
                                workload.units_per_target_unit,
                            )?,
                            0,
                        )
                        .map_err(|_| invalid())?,
                        op: ComparisonOp::Equal,
                        rhs: 0,
                    }),
                    enforcement: Vec::new(),
                    provenance: ids.fact.clone(),
                    tags: Vec::new(),
                });
            }
            LinearExpression::new(
                vec![
                    LinearTerm {
                        variable: ids.actual.clone(),
                        coefficient: 1,
                    },
                    LinearTerm {
                        variable: split.whole.clone(),
                        coefficient: -workload.units_per_target_unit,
                    },
                    LinearTerm {
                        variable: split.remainder.clone(),
                        coefficient: -1,
                    },
                ],
                0,
            )
            .map_err(|_| invalid())?
        } else {
            LinearExpression::new(
                workload_actual_terms(
                    peer,
                    &ids,
                    candidate_integers,
                    &ids.actual,
                    WorkloadComponent::Native,
                    1,
                )?,
                0,
            )
            .map_err(|_| invalid())?
        };
    let fact = workload_fact(
        workload,
        period,
        peer.person,
        ids.fact.clone(),
        parent,
        budget,
    )?;
    constraints.push(ConstraintRecord {
        id: ids.actual_constraint.clone(),
        body: Constraint::LinearComparison(LinearComparison {
            expression: actual_expression,
            op: ComparisonOp::Equal,
            rhs: 0,
        }),
        enforcement: Vec::new(),
        provenance: ids.fact.clone(),
        tags: Vec::new(),
    });
    for (segment_index, segment) in peer.segments.iter().enumerate() {
        let helper = ids.segments.get(segment_index).ok_or_else(invalid)?;
        let mut population = Vec::new();
        for index in &segment.population {
            population.push(candidate_integers.get(*index).ok_or_else(invalid)?.clone());
        }
        for (member, integer) in population.iter().enumerate() {
            let constraint_id = ids
                .union_constraints
                .get(segment_index)
                .and_then(|_| {
                    let offset = peer.segments[..segment_index]
                        .iter()
                        .map(|value| value.population.len().saturating_add(1))
                        .sum::<usize>();
                    ids.union_constraints.get(offset + member)
                })
                .ok_or_else(invalid)?
                .clone();
            constraints.push(ConstraintRecord {
                id: constraint_id,
                body: Constraint::LinearComparison(LinearComparison {
                    expression: LinearExpression::new(
                        vec![
                            LinearTerm {
                                variable: helper.clone(),
                                coefficient: 1,
                            },
                            LinearTerm {
                                variable: integer.clone(),
                                coefficient: -1,
                            },
                        ],
                        0,
                    )
                    .map_err(|_| invalid())?,
                    op: ComparisonOp::GreaterOrEqual,
                    rhs: 0,
                }),
                enforcement: Vec::new(),
                provenance: ids.fact.clone(),
                tags: Vec::new(),
            });
        }
        let offset = peer.segments[..segment_index]
            .iter()
            .map(|value| value.population.len().saturating_add(1))
            .sum::<usize>();
        constraints.push(ConstraintRecord {
            id: ids
                .union_constraints
                .get(offset + segment.population.len())
                .ok_or_else(invalid)?
                .clone(),
            body: Constraint::LinearComparison(LinearComparison {
                expression: LinearExpression::new(
                    std::iter::once(LinearTerm {
                        variable: helper.clone(),
                        coefficient: 1,
                    })
                    .chain(population.into_iter().map(|variable| LinearTerm {
                        variable,
                        coefficient: -1,
                    }))
                    .collect(),
                    0,
                )
                .map_err(|_| invalid())?,
                op: ComparisonOp::LessOrEqual,
                rhs: 0,
            }),
            enforcement: Vec::new(),
            provenance: ids.fact.clone(),
            tags: Vec::new(),
        });
    }
    let delta_expression = workload_delta_expression(workload, peer, peer_actuals, &ids.delta)?;
    constraints.push(ConstraintRecord {
        id: ids.delta_constraint.clone(),
        body: Constraint::LinearComparison(delta_expression),
        enforcement: Vec::new(),
        provenance: ids.fact.clone(),
        tags: Vec::new(),
    });
    constraints.push(ConstraintRecord {
        id: ids.difference_constraint.clone(),
        body: Constraint::AbsDifference {
            target: ids.difference.clone(),
            left: ids.delta.clone(),
            right: ids.zero.clone(),
        },
        enforcement: Vec::new(),
        provenance: ids.fact.clone(),
        tags: Vec::new(),
    });
    if let DeviationPenalty::Piecewise { breakpoints } = &workload.policy.penalty {
        variables.extend(ids.selectors.iter().cloned().map(|id| {
            Variable::Boolean(BoolVariable {
                id,
                provenance: ids.fact.clone(),
            })
        }));
        variables.push(int_variable(ids.cost.clone(), 0, peer.cost_upper));
        let literals = ids
            .selectors
            .iter()
            .cloned()
            .map(Literal::positive)
            .collect::<Vec<_>>();
        constraints.push(ConstraintRecord {
            id: ids.exactly_one.clone().ok_or_else(invalid)?,
            body: Constraint::ExactlyOne { literals },
            enforcement: Vec::new(),
            provenance: ids.fact.clone(),
            tags: Vec::new(),
        });
        for (index, point) in breakpoints.iter().enumerate() {
            let selector = Literal::positive(ids.selectors.get(index).ok_or_else(invalid)?.clone());
            let lower = i64::from(point.from_deviation)
                .checked_mul(workload.score_denominator)
                .ok_or_else(arithmetic)?;
            constraints.push(ConstraintRecord {
                id: ids
                    .lower_constraints
                    .get(index)
                    .ok_or_else(invalid)?
                    .clone(),
                body: Constraint::LinearComparison(LinearComparison {
                    expression: LinearExpression::new(
                        vec![LinearTerm {
                            variable: ids.difference.clone(),
                            coefficient: 1,
                        }],
                        0,
                    )
                    .map_err(|_| invalid())?,
                    op: ComparisonOp::GreaterOrEqual,
                    rhs: lower,
                }),
                enforcement: vec![selector.clone()],
                provenance: ids.fact.clone(),
                tags: Vec::new(),
            });
            if let Some(next) = breakpoints.get(index + 1) {
                let upper = i64::from(next.from_deviation)
                    .checked_mul(workload.score_denominator)
                    .and_then(|value| value.checked_sub(1))
                    .ok_or_else(arithmetic)?;
                constraints.push(ConstraintRecord {
                    id: ids
                        .upper_constraints
                        .get(index)
                        .ok_or_else(invalid)?
                        .clone(),
                    body: Constraint::LinearComparison(LinearComparison {
                        expression: LinearExpression::new(
                            vec![LinearTerm {
                                variable: ids.difference.clone(),
                                coefficient: 1,
                            }],
                            0,
                        )
                        .map_err(|_| invalid())?,
                        op: ComparisonOp::LessOrEqual,
                        rhs: upper,
                    }),
                    enforcement: vec![selector.clone()],
                    provenance: ids.fact.clone(),
                    tags: Vec::new(),
                });
            }
            let base = piecewise_base(breakpoints, index)?;
            let constant = i128::from(workload.score_denominator)
                .checked_mul(base - i128::from(point.slope) * i128::from(point.from_deviation))
                .ok_or_else(arithmetic)?;
            let constant = i64::try_from(constant).map_err(|_| arithmetic())?;
            constraints.push(ConstraintRecord {
                id: ids
                    .relation_constraints
                    .get(index)
                    .ok_or_else(invalid)?
                    .clone(),
                body: Constraint::LinearComparison(LinearComparison {
                    expression: LinearExpression::new(
                        vec![
                            LinearTerm {
                                variable: ids.cost.clone(),
                                coefficient: 1,
                            },
                            LinearTerm {
                                variable: ids.difference.clone(),
                                coefficient: -i64::from(point.slope),
                            },
                        ],
                        0,
                    )
                    .map_err(|_| invalid())?,
                    op: ComparisonOp::Equal,
                    rhs: constant,
                }),
                enforcement: vec![selector],
                provenance: ids.fact.clone(),
                tags: Vec::new(),
            });
        }
    }
    let (objective_variable, objective_coefficient) = match &workload.policy.penalty {
        DeviationPenalty::Absolute {} => (ids.difference.clone(), workload.coefficient),
        DeviationPenalty::Piecewise { .. } => (ids.cost.clone(), workload.coefficient),
    };
    Ok(WorkloadEmission {
        variables,
        constraints,
        term: ObjectiveTerm {
            id: ids.term,
            expression: LinearExpression {
                constant: 0,
                terms: vec![LinearTerm {
                    variable: objective_variable,
                    coefficient: objective_coefficient,
                }],
            },
            kind: ObjectiveTermKind::Penalty,
            category: workload_category_id()?,
            provenance: fact.id.clone(),
        },
        fact,
    })
}

#[derive(Clone, Copy)]
enum WorkloadComponent {
    Native,
    Whole,
    Remainder,
}

fn contribution_units(amount: i64, component: WorkloadComponent, minute_units: i64) -> i64 {
    match component {
        WorkloadComponent::Native => amount,
        WorkloadComponent::Whole => amount.div_euclid(minute_units),
        WorkloadComponent::Remainder => amount.rem_euclid(minute_units),
    }
}

fn workload_actual_terms(
    peer: &WorkloadPeerShape,
    ids: &WorkloadIds,
    candidate_integers: &[IntVariableId],
    variable: &IntVariableId,
    component: WorkloadComponent,
    minute_units: i64,
) -> Result<Vec<LinearTerm>, AssignmentRuleError> {
    let mut terms = vec![LinearTerm {
        variable: variable.clone(),
        coefficient: 1,
    }];
    if peer.segments.is_empty() {
        for (index, amount) in &peer.terms {
            let coefficient = contribution_units(*amount, component, minute_units);
            if coefficient != 0 {
                terms.push(LinearTerm {
                    variable: candidate_integers.get(*index).ok_or_else(invalid)?.clone(),
                    coefficient: -coefficient,
                });
            }
        }
    } else {
        for (segment, amount) in ids
            .segments
            .iter()
            .zip(peer.segments.iter().map(|segment| segment.units))
        {
            let coefficient = contribution_units(amount, component, minute_units);
            if coefficient != 0 {
                terms.push(LinearTerm {
                    variable: segment.clone(),
                    coefficient: -coefficient,
                });
            }
        }
    }
    Ok(terms)
}

fn workload_delta_expression(
    workload: &WorkloadPlan<'_>,
    peer: &WorkloadPeerShape,
    actuals: &[IntVariableId],
    delta: &IntVariableId,
) -> Result<LinearComparison, AssignmentRuleError> {
    let index = workload
        .target
        .peers
        .iter()
        .position(|person| *person == peer.person)
        .ok_or_else(invalid)?;
    let mut terms = vec![LinearTerm {
        variable: delta.clone(),
        coefficient: 1,
    }];
    let mut rhs = 0_i64;
    match workload.target.kind {
        WorkloadTargetKind::Explicit | WorkloadTargetKind::PersonTargets => {
            terms.push(LinearTerm {
                variable: actuals.get(index).ok_or_else(invalid)?.clone(),
                coefficient: -1,
            });
            rhs = -workload
                .target
                .targets
                .get(&peer.person)
                .copied()
                .ok_or_else(invalid)?
                .checked_mul(workload.units_per_target_unit)
                .ok_or(AssignmentRuleError::LimitExceeded(
                    AssignmentRuleLimit::PerRecord,
                ))?;
        }
        WorkloadTargetKind::EqualShare => {
            let count = i64::try_from(actuals.len()).map_err(|_| arithmetic())?;
            terms.push(LinearTerm {
                variable: actuals.get(index).ok_or_else(invalid)?.clone(),
                coefficient: -(count - 1),
            });
            for (other, actual) in actuals.iter().enumerate() {
                if other != index {
                    terms.push(LinearTerm {
                        variable: actual.clone(),
                        coefficient: 1,
                    });
                }
            }
        }
        WorkloadTargetKind::WeightedEqualShare => {
            let denominator = workload.target.denominator;
            let weight = *workload
                .target
                .weights
                .get(&peer.person)
                .ok_or_else(invalid)?;
            terms.push(LinearTerm {
                variable: actuals.get(index).ok_or_else(invalid)?.clone(),
                coefficient: -(denominator - weight),
            });
            for (other, actual) in actuals.iter().enumerate() {
                if other != index {
                    terms.push(LinearTerm {
                        variable: actual.clone(),
                        coefficient: weight,
                    });
                }
            }
        }
    }
    Ok(LinearComparison {
        expression: LinearExpression::new(terms, 0).map_err(|_| invalid())?,
        op: ComparisonOp::Equal,
        rhs,
    })
}

fn piecewise_base(
    breakpoints: &[crate::model::PenaltyBreakpoint],
    segment: usize,
) -> Result<i128, AssignmentRuleError> {
    let mut base = 0_i128;
    for pair in breakpoints[..=segment].windows(2) {
        base = base
            .checked_add(
                i128::from(pair[1].from_deviation - pair[0].from_deviation)
                    * i128::from(pair[0].slope),
            )
            .ok_or_else(arithmetic)?;
    }
    Ok(base)
}

fn workload_category_id() -> Result<ScoreCategoryId, AssignmentRuleError> {
    ScoreCategoryId::new(WORKLOAD_BALANCE_CATEGORY).map_err(|_| invalid())
}

fn workload_fact(
    workload: &WorkloadPlan<'_>,
    period: &WorkloadPeriod,
    person: PersonId,
    id: ProvenanceId,
    parent: ProvenanceId,
    budget: &mut OperationBudget<'_>,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    budget.reserve(0, 10, 640)?;
    let ref_entity = |kind: &str, entity_id: EntityId| {
        Ok::<_, AssignmentRuleError>(DomainEntityRef {
            kind: DomainEntityKindId::new(kind).map_err(|_| invalid())?,
            id: DomainEntityId::new(format!("official.workforce.{entity_id}"))
                .map_err(|_| invalid())?,
        })
    };
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Preference,
        source_id: workload.id.to_string(),
        entity_refs: vec![
            ref_entity(
                "official.workforce.person",
                EntityId::from_uuid(person.as_uuid()),
            )?,
            ref_entity(
                "official.workforce.calendar",
                period.period.calendar_id.as_entity_id(),
            )?,
            ref_entity(
                "official.workforce.workload_bucket",
                workload.policy.bucket_id.as_entity_id(),
            )?,
        ],
        message_key: "official.workforce.preference.workload_balance".to_owned(),
        parameters: BTreeMap::from([
            (
                "coefficient".to_owned(),
                ProvenanceParameter::Integer(workload.coefficient),
            ),
            (
                "denominator".to_owned(),
                ProvenanceParameter::Integer(workload.target.denominator),
            ),
            (
                "nanoseconds_per_unit".to_owned(),
                ProvenanceParameter::Integer(workload.nanoseconds_per_unit),
            ),
            (
                "score_denominator".to_owned(),
                ProvenanceParameter::Integer(workload.score_denominator),
            ),
            (
                "period_start".to_owned(),
                ProvenanceParameter::Text(period.period.start.to_string()),
            ),
            (
                "period_end".to_owned(),
                ProvenanceParameter::Text(period.period.end.to_string()),
            ),
        ]),
        parent: Some(parent),
    })
}

pub(super) struct TargetIds {
    pub count: IntVariableId,
    pub target: IntVariableId,
    pub difference: IntVariableId,
    pub count_constraint: PlanningConstraintId,
    pub difference_constraint: PlanningConstraintId,
    pub term: ObjectiveTermId,
    pub fact: ProvenanceId,
}

impl TargetIds {
    pub fn derive(
        entry: &PreferenceTarget,
        period: &TargetPeriod,
        identities: &mut PlanningIdentities,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let key = (
            "preference_target",
            entry.id,
            period.person,
            period.period,
            entry.target_count,
        );
        let derive_int = |suffix: &str,
                          identities: &mut PlanningIdentities,
                          budget: &mut OperationBudget<'_>|
         -> Result<IntVariableId, AssignmentRuleError> {
            IntVariableId::new(identities.derive(IdentityKind::Integer, &(suffix, key), budget)?)
                .map_err(|_| invalid())
        };
        let derive_constraint = |suffix: &str,
                                 identities: &mut PlanningIdentities,
                                 budget: &mut OperationBudget<'_>|
         -> Result<PlanningConstraintId, AssignmentRuleError> {
            PlanningConstraintId::new(identities.derive(
                IdentityKind::Constraint,
                &(suffix, key),
                budget,
            )?)
            .map_err(|_| invalid())
        };
        let count = derive_int("preference_target_count", identities, budget)?;
        let target = derive_int("preference_target_constant", identities, budget)?;
        let difference = derive_int("preference_target_difference", identities, budget)?;
        let count_constraint =
            derive_constraint("preference_target_count_constraint", identities, budget)?;
        let difference_constraint = derive_constraint(
            "preference_target_difference_constraint",
            identities,
            budget,
        )?;
        let common = Self::derive_constant(entry, period, identities, budget)?;
        Ok(Self {
            count,
            target,
            difference,
            count_constraint,
            difference_constraint,
            term: common.term,
            fact: common.fact,
        })
    }

    pub fn derive_constant(
        entry: &PreferenceTarget,
        period: &TargetPeriod,
        identities: &mut PlanningIdentities,
        budget: &mut OperationBudget<'_>,
    ) -> Result<PreferenceIds, AssignmentRuleError> {
        let key = (
            "preference_target",
            entry.id,
            period.person,
            period.period,
            entry.target_count,
        );
        Ok(PreferenceIds {
            term: ObjectiveTermId::new(identities.derive(
                IdentityKind::ObjectiveTerm,
                &("preference_target_term", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
            fact: ProvenanceId::new(identities.derive(
                IdentityKind::Provenance,
                &("preference_target_fact", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
            base_member: false,
        })
    }
}

pub(super) struct TargetEmission {
    pub variables: [Variable; 3],
    pub constraints: [ConstraintRecord; 2],
    pub term: ObjectiveTerm,
    pub fact: ProvenanceRecord,
}

pub(super) fn emit_target(
    entry: &PreferenceTarget,
    period: &TargetPeriod,
    population: &[IntVariableId],
    parent: ProvenanceId,
    ids: TargetIds,
    budget: &mut OperationBudget<'_>,
) -> Result<TargetEmission, AssignmentRuleError> {
    budget.steps(
        count(population.len())?
            .checked_add(16)
            .ok_or_else(arithmetic)?,
    )?;
    let target_count = i64::from(entry.target_count);
    let population_count = i64::try_from(population.len()).map_err(|_| arithmetic())?;
    let difference = target_difference_upper(entry.target_count, population.len())?;
    let mut count_terms = Vec::with_capacity(population.len().saturating_add(1));
    count_terms.push(LinearTerm {
        variable: ids.count.clone(),
        coefficient: 1,
    });
    count_terms.extend(population.iter().cloned().map(|variable| LinearTerm {
        variable,
        coefficient: -1,
    }));
    let count_expression = LinearExpression::new(count_terms, 0).map_err(|_| invalid())?;
    let count_constraint = ConstraintRecord {
        id: ids.count_constraint,
        body: Constraint::LinearComparison(LinearComparison {
            expression: count_expression,
            op: ComparisonOp::Equal,
            rhs: 0,
        }),
        enforcement: Vec::new(),
        provenance: ids.fact.clone(),
        tags: Vec::new(),
    };
    let difference_constraint = ConstraintRecord {
        id: ids.difference_constraint,
        body: Constraint::AbsDifference {
            target: ids.difference.clone(),
            left: ids.count.clone(),
            right: ids.target.clone(),
        },
        enforcement: Vec::new(),
        provenance: ids.fact.clone(),
        tags: Vec::new(),
    };
    let fact = target_fact(
        entry,
        period,
        population.len(),
        ids.fact.clone(),
        parent,
        budget,
    )?;
    let variables = [
        Variable::Integer(IntVariable {
            id: ids.count.clone(),
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange {
                    start: 0,
                    end: population_count,
                }],
            },
            provenance: ids.fact.clone(),
        }),
        Variable::Integer(IntVariable {
            id: ids.target.clone(),
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange {
                    start: target_count,
                    end: target_count,
                }],
            },
            provenance: ids.fact.clone(),
        }),
        Variable::Integer(IntVariable {
            id: ids.difference.clone(),
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange {
                    start: 0,
                    end: difference,
                }],
            },
            provenance: ids.fact.clone(),
        }),
    ];
    Ok(TargetEmission {
        variables,
        constraints: [count_constraint, difference_constraint],
        term: ObjectiveTerm {
            id: ids.term,
            expression: LinearExpression {
                constant: 0,
                terms: vec![LinearTerm {
                    variable: ids.difference,
                    coefficient: entry.coefficient,
                }],
            },
            kind: ObjectiveTermKind::Penalty,
            category: target_category_id()?,
            provenance: fact.id.clone(),
        },
        fact,
    })
}

pub(super) fn emit_constant_target(
    entry: &PreferenceTarget,
    period: &TargetPeriod,
    parent: ProvenanceId,
    ids: PreferenceIds,
    budget: &mut OperationBudget<'_>,
) -> Result<PreferenceEmission, AssignmentRuleError> {
    budget.steps(4)?;
    let constant = entry
        .coefficient
        .checked_mul(i64::from(entry.target_count))
        .ok_or_else(arithmetic)?;
    let fact = target_fact(entry, period, 0, ids.fact, parent, budget)?;
    Ok(PreferenceEmission {
        term: ObjectiveTerm {
            id: ids.term,
            expression: LinearExpression {
                constant,
                terms: Vec::new(),
            },
            kind: ObjectiveTermKind::Penalty,
            category: target_category_id()?,
            provenance: fact.id.clone(),
        },
        fact,
    })
}

pub(super) struct WeekendTargetIds {
    pub count: IntVariableId,
    pub target: IntVariableId,
    pub difference: IntVariableId,
    pub count_constraint: PlanningConstraintId,
    pub difference_constraint: PlanningConstraintId,
    pub term: ObjectiveTermId,
    pub fact: ProvenanceId,
}

impl WeekendTargetIds {
    pub fn derive(
        entry: &WeekendTargetPlan,
        person: &WeekendTargetPerson,
        identities: &mut PlanningIdentities,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        let key = (
            "preference_weekend_target",
            entry.id,
            person.person,
            entry.target_count,
        );
        let derive_int = |suffix: &str,
                          identities: &mut PlanningIdentities,
                          budget: &mut OperationBudget<'_>|
         -> Result<IntVariableId, AssignmentRuleError> {
            IntVariableId::new(identities.derive(IdentityKind::Integer, &(suffix, key), budget)?)
                .map_err(|_| invalid())
        };
        let derive_constraint = |suffix: &str,
                                 identities: &mut PlanningIdentities,
                                 budget: &mut OperationBudget<'_>|
         -> Result<PlanningConstraintId, AssignmentRuleError> {
            PlanningConstraintId::new(identities.derive(
                IdentityKind::Constraint,
                &(suffix, key),
                budget,
            )?)
            .map_err(|_| invalid())
        };
        Ok(Self {
            count: derive_int("preference_weekend_target_count", identities, budget)?,
            target: derive_int("preference_weekend_target_constant", identities, budget)?,
            difference: derive_int("preference_weekend_target_difference", identities, budget)?,
            count_constraint: derive_constraint(
                "preference_weekend_target_count_constraint",
                identities,
                budget,
            )?,
            difference_constraint: derive_constraint(
                "preference_weekend_target_difference_constraint",
                identities,
                budget,
            )?,
            term: ObjectiveTermId::new(identities.derive(
                IdentityKind::ObjectiveTerm,
                &("preference_weekend_target_term", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
            fact: ProvenanceId::new(identities.derive(
                IdentityKind::Provenance,
                &("preference_weekend_target_fact", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
        })
    }

    pub fn derive_constant(
        entry: &WeekendTargetPlan,
        person: &WeekendTargetPerson,
        identities: &mut PlanningIdentities,
        budget: &mut OperationBudget<'_>,
    ) -> Result<PreferenceIds, AssignmentRuleError> {
        let key = (
            "preference_weekend_target",
            entry.id,
            person.person,
            entry.target_count,
        );
        Ok(PreferenceIds {
            term: ObjectiveTermId::new(identities.derive(
                IdentityKind::ObjectiveTerm,
                &("preference_weekend_target_term", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
            fact: ProvenanceId::new(identities.derive(
                IdentityKind::Provenance,
                &("preference_weekend_target_fact", key),
                budget,
            )?)
            .map_err(|_| invalid())?,
            base_member: false,
        })
    }
}

fn weekend_target_indicator_id(
    entry: &WeekendTargetPlan,
    person: &WeekendTargetPerson,
    date: Date,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<IntVariableId, AssignmentRuleError> {
    IntVariableId::new(identities.derive(
        IdentityKind::Integer,
        &(
            "preference_weekend_target_indicator",
            entry.id,
            person.person,
            date,
        ),
        budget,
    )?)
    .map_err(|_| invalid())
}

fn weekend_target_constraint_id(
    entry: &WeekendTargetPlan,
    person: &WeekendTargetPerson,
    date: Date,
    kind: &'static str,
    index: usize,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<PlanningConstraintId, AssignmentRuleError> {
    PlanningConstraintId::new(identities.derive(
        IdentityKind::Constraint,
        &(kind, entry.id, person.person, date, index),
        budget,
    )?)
    .map_err(|_| invalid())
}

pub(super) struct WeekendTargetEmission {
    pub variables: Vec<Variable>,
    pub constraints: Vec<ConstraintRecord>,
    pub term: ObjectiveTerm,
    pub fact: ProvenanceRecord,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn emit_weekend_target(
    entry: &WeekendTargetPlan,
    person: &WeekendTargetPerson,
    shape: &WeekendTargetShape,
    populations: &[Vec<IntVariableId>],
    parent: ProvenanceId,
    ids: WeekendTargetIds,
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<WeekendTargetEmission, AssignmentRuleError> {
    if shape.weekends.len() != populations.len() {
        return Err(invalid());
    }
    budget.steps(
        count(populations.len())?
            .checked_add(16)
            .ok_or_else(arithmetic)?,
    )?;
    let fact = weekend_target_fact(
        entry,
        person,
        shape.weekends.len(),
        ids.fact.clone(),
        parent,
        budget,
    )?;
    let mut variables =
        Vec::with_capacity(populations.len().checked_add(3).ok_or_else(arithmetic)?);
    let mut constraints = Vec::new();
    let mut count_terms = Vec::with_capacity(populations.len().saturating_add(1));
    count_terms.push(LinearTerm {
        variable: ids.count.clone(),
        coefficient: 1,
    });
    for (weekend, population) in shape.weekends.iter().zip(populations) {
        if population.is_empty() {
            return Err(invalid());
        }
        let indicator =
            weekend_target_indicator_id(entry, person, weekend.date, identities, budget)?;
        variables.push(Variable::Integer(IntVariable {
            id: indicator.clone(),
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange { start: 0, end: 1 }],
            },
            provenance: fact.id.clone(),
        }));
        count_terms.push(LinearTerm {
            variable: indicator.clone(),
            coefficient: -1,
        });
        for (index, candidate) in population.iter().enumerate() {
            let lower_id = weekend_target_constraint_id(
                entry,
                person,
                weekend.date,
                "preference_weekend_target_indicator_lower",
                index,
                identities,
                budget,
            )?;
            let lower = ConstraintRecord {
                id: lower_id,
                body: Constraint::LinearComparison(LinearComparison {
                    expression: LinearExpression::new(
                        vec![
                            LinearTerm {
                                variable: indicator.clone(),
                                coefficient: 1,
                            },
                            LinearTerm {
                                variable: candidate.clone(),
                                coefficient: -1,
                            },
                        ],
                        0,
                    )
                    .map_err(|_| invalid())?,
                    op: ComparisonOp::GreaterOrEqual,
                    rhs: 0,
                }),
                enforcement: Vec::new(),
                provenance: fact.id.clone(),
                tags: Vec::new(),
            };
            constraints.push(lower);
        }
        let upper_id = weekend_target_constraint_id(
            entry,
            person,
            weekend.date,
            "preference_weekend_target_indicator_upper",
            0,
            identities,
            budget,
        )?;
        let mut upper_terms = Vec::with_capacity(population.len().saturating_add(1));
        upper_terms.push(LinearTerm {
            variable: indicator,
            coefficient: 1,
        });
        upper_terms.extend(population.iter().cloned().map(|variable| LinearTerm {
            variable,
            coefficient: -1,
        }));
        constraints.push(ConstraintRecord {
            id: upper_id,
            body: Constraint::LinearComparison(LinearComparison {
                expression: LinearExpression::new(upper_terms, 0).map_err(|_| invalid())?,
                op: ComparisonOp::LessOrEqual,
                rhs: 0,
            }),
            enforcement: Vec::new(),
            provenance: fact.id.clone(),
            tags: Vec::new(),
        });
    }
    let count_constraint = ConstraintRecord {
        id: ids.count_constraint,
        body: Constraint::LinearComparison(LinearComparison {
            expression: LinearExpression::new(count_terms, 0).map_err(|_| invalid())?,
            op: ComparisonOp::Equal,
            rhs: 0,
        }),
        enforcement: Vec::new(),
        provenance: ids.fact.clone(),
        tags: Vec::new(),
    };
    constraints.push(count_constraint);
    constraints.push(ConstraintRecord {
        id: ids.difference_constraint,
        body: Constraint::AbsDifference {
            target: ids.difference.clone(),
            left: ids.count.clone(),
            right: ids.target.clone(),
        },
        enforcement: Vec::new(),
        provenance: ids.fact.clone(),
        tags: Vec::new(),
    });
    let count_upper = i64::try_from(populations.len()).map_err(|_| arithmetic())?;
    let variables_tail = [
        (ids.count.clone(), 0, count_upper),
        (
            ids.target.clone(),
            i64::from(entry.target_count),
            i64::from(entry.target_count),
        ),
        (ids.difference.clone(), 0, shape.difference_upper),
    ];
    for (id, start, end) in variables_tail {
        variables.push(Variable::Integer(IntVariable {
            id,
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange { start, end }],
            },
            provenance: ids.fact.clone(),
        }));
    }
    Ok(WeekendTargetEmission {
        variables,
        constraints,
        term: ObjectiveTerm {
            id: ids.term,
            expression: LinearExpression {
                constant: 0,
                terms: vec![LinearTerm {
                    variable: ids.difference,
                    coefficient: entry.coefficient,
                }],
            },
            kind: ObjectiveTermKind::Penalty,
            category: weekend_target_category_id()?,
            provenance: fact.id.clone(),
        },
        fact,
    })
}

pub(super) fn emit_constant_weekend_target(
    entry: &WeekendTargetPlan,
    person: &WeekendTargetPerson,
    parent: ProvenanceId,
    ids: PreferenceIds,
    budget: &mut OperationBudget<'_>,
) -> Result<PreferenceEmission, AssignmentRuleError> {
    budget.steps(4)?;
    let constant = entry
        .coefficient
        .checked_mul(i64::from(entry.target_count))
        .ok_or_else(arithmetic)?;
    let fact = weekend_target_fact(entry, person, 0, ids.fact, parent, budget)?;
    Ok(PreferenceEmission {
        term: ObjectiveTerm {
            id: ids.term,
            expression: LinearExpression {
                constant,
                terms: Vec::new(),
            },
            kind: ObjectiveTermKind::Penalty,
            category: weekend_target_category_id()?,
            provenance: fact.id.clone(),
        },
        fact,
    })
}

fn weekend_target_fact(
    entry: &WeekendTargetPlan,
    person: &WeekendTargetPerson,
    selectable_count: usize,
    id: ProvenanceId,
    parent: ProvenanceId,
    budget: &mut OperationBudget<'_>,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    budget.reserve(0, 7, 512)?;
    let person_ref = DomainEntityRef {
        kind: DomainEntityKindId::new("official.workforce.person").map_err(|_| invalid())?,
        id: DomainEntityId::new(format!(
            "official.workforce.{}",
            EntityId::from_uuid(person.person.as_uuid())
        ))
        .map_err(|_| invalid())?,
    };
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Preference,
        source_id: entry.id.to_string(),
        entity_refs: vec![person_ref],
        message_key: "official.workforce.preference.weekend_target".to_owned(),
        parameters: BTreeMap::from([
            (
                "target_count".to_owned(),
                ProvenanceParameter::Integer(i64::from(entry.target_count)),
            ),
            (
                "coefficient".to_owned(),
                ProvenanceParameter::Integer(entry.coefficient),
            ),
            (
                "selectable_count".to_owned(),
                ProvenanceParameter::Integer(
                    i64::try_from(selectable_count).map_err(|_| arithmetic())?,
                ),
            ),
        ]),
        parent: Some(parent),
    })
}

fn target_fact(
    entry: &PreferenceTarget,
    period: &TargetPeriod,
    population: usize,
    id: ProvenanceId,
    parent: ProvenanceId,
    budget: &mut OperationBudget<'_>,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    budget.reserve(0, 7, 512)?;
    let person = DomainEntityRef {
        kind: DomainEntityKindId::new("official.workforce.person").map_err(|_| invalid())?,
        id: DomainEntityId::new(format!(
            "official.workforce.{}",
            EntityId::from_uuid(period.person.as_uuid())
        ))
        .map_err(|_| invalid())?,
    };
    let calendar = DomainEntityRef {
        kind: DomainEntityKindId::new("official.workforce.calendar").map_err(|_| invalid())?,
        id: DomainEntityId::new(format!(
            "official.workforce.{}",
            period.period.calendar_id.as_entity_id()
        ))
        .map_err(|_| invalid())?,
    };
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Preference,
        source_id: entry.id.to_string(),
        entity_refs: vec![person, calendar],
        message_key: "official.workforce.preference.assignment_target".to_owned(),
        parameters: BTreeMap::from([
            (
                "target_count".to_owned(),
                ProvenanceParameter::Integer(i64::from(entry.target_count)),
            ),
            (
                "coefficient".to_owned(),
                ProvenanceParameter::Integer(entry.coefficient),
            ),
            (
                "period_start".to_owned(),
                ProvenanceParameter::Text(period.period.start.to_string()),
            ),
            (
                "period_end".to_owned(),
                ProvenanceParameter::Text(period.period.end.to_string()),
            ),
            (
                "selectable_count".to_owned(),
                ProvenanceParameter::Integer(i64::try_from(population).map_err(|_| arithmetic())?),
            ),
        ]),
        parent: Some(parent),
    })
}

pub(super) fn level_fact(
    policy: EntityId,
    level: &PreferenceLevel,
    id: ProvenanceId,
) -> ProvenanceRecord {
    ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Fact,
        source_id: policy.to_string(),
        entity_refs: Vec::new(),
        message_key: "official.workforce.preference.level".to_owned(),
        parameters: BTreeMap::from([(
            "level".to_owned(),
            ProvenanceParameter::Text(level.key.clone()),
        )]),
        parent: None,
    }
}

fn temporal_overflow(owner: EntityId) -> AssignmentRuleError {
    AssignmentRuleError::Temporal(super::super::temporal::TemporalIssue {
        kind: super::super::temporal::TemporalIssueKind::DateOverflow,
        entity_id: Some(owner),
        local_date: None,
        endpoint: None,
        occurrence_date: None,
        origin: None,
    })
}

fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}

fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow)
}
