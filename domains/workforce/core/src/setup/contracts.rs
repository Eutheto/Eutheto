//! Frozen Workforce setup-query wire contracts, separate from stored Workforce data.
//!
//! These DTOs and named limits do not implement projections or enforce runtime bounds.
//! The query boundary must validate versions, parameters, source/cursor compatibility,
//! work budgets, cancellation, exact duration invariants and serialized output limits.
//! Existing model payloads retain their authoritative complete record representations.

use crate::ids::{AssignmentTypeId, AvailabilityId, ShiftId, ShiftTemplateId};
use crate::model::{
    AvailabilityKind, DateRange, PreferencePriority, WorkforceEntity, WorkforcePreference,
    WorkforceRule,
};
use eutheto_domain_api::{DomainSetupQueryV1, KindDescriptor, SetupContinuationV1};
use eutheto_types::{Change, EntityId, PersonId, Rfc3339Timestamp, RuleId, ScenarioSettings};
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

pub const QUERY_BYTES: usize = 64 * 1024;
pub const QUERY_STRING_BYTES: usize = 256;
pub const DEFAULT_PAGE: u16 = 50;

const fn default_page_limit() -> u16 {
    DEFAULT_PAGE
}

pub const MAX_PAGE: u16 = 200;
pub const MAX_WINDOW_DAYS: u16 = 366;
pub const MAX_MATRIX_PEOPLE: usize = 128;
pub const MAX_MATRIX_TYPES: usize = 64;
pub const MAX_MATRIX_CELLS: usize = 8192;
pub const ORDINARY_DATA_BYTES: usize = 2 * 1024 * 1024;
pub const DETAIL_DATA_BYTES: usize = 16 * 1024 * 1024;
pub const PREVIEW_DATA_BYTES: usize = 32 * 1024 * 1024;
pub const RESPONSE_FRAME_BYTES: usize = 64 * 1024;
pub const CURSOR_POSITION_BYTES: usize = 512;
pub const MAX_PROJECTION_VISITS: u32 = 100_000;

// Named structural OUTPUT limits, distinct from ContractJsonLimits::DEFAULT.
// The generic command boundary already permits depth128/1MiB strings/1000000 items;
// 16 wrapper levels preserve those values inside a bounded change-page response.
// DTO schemas retain their tighter field/array/model limits. Select max_serialized_bytes
// per result family (ordinary2MiB/detail16MiB/preview32MiB), then check outer framing too.
pub const SETUP_OUTPUT_LIMITS: eutheto_domain_api::ContractJsonLimits =
    eutheto_domain_api::ContractJsonLimits {
        max_serialized_bytes: PREVIEW_DATA_BYTES,
        max_depth: 144,
        max_string_bytes: 1024 * 1024,
        max_collection_items: 1_000_000,
    };

// This closed enum/constant mapping IS the setup allowlist; no setup ID enters
// DomainUiManifest.result_views. The accepted subject alone uses that existing allowlist.
// Decode DomainSetupQueryV1 into this enum by its stable view ID, without cloning Value.
// Each result discriminator below must match this query's variant exactly.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "viewId", content = "parameters", deny_unknown_fields)]
pub enum WorkforceSetupQueryV1 {
    #[serde(rename = "official.workforce.setup.overview")]
    Overview(EmptyParametersV1),
    #[serde(rename = "official.workforce.setup.settings_preparation")]
    SettingsPreparation(SettingsPreparationParametersV1),
    #[serde(rename = "eutheto.setup.entity_page")]
    EntityPage(EntityPageParametersV1),
    #[serde(rename = "eutheto.setup.entity_detail")]
    EntityDetail(EntityDetailParametersV1),
    #[serde(rename = "eutheto.setup.rule_catalog")]
    RuleCatalog(EmptyParametersV1),
    #[serde(rename = "official.workforce.setup.rule_page")]
    RulePage(RulePageParametersV1),
    #[serde(rename = "official.workforce.setup.rule_detail")]
    RuleDetail(RuleDetailParametersV1),
    #[serde(rename = "official.workforce.setup.rule_scope")]
    RuleScope(RuleScopeParametersV1),
    #[serde(rename = "official.workforce.setup.work_window")]
    WorkWindow(WorkWindowParametersV1),
    #[serde(rename = "official.workforce.setup.work_detail")]
    WorkDetail(WorkDetailParametersV1),
    #[serde(rename = "official.workforce.setup.generation_review")]
    GenerationReview(GenerationReviewParametersV1),
    #[serde(rename = "official.workforce.setup.eligibility_matrix")]
    EligibilityMatrix(EligibilityMatrixParametersV1),
    #[serde(rename = "official.workforce.setup.availability_window")]
    AvailabilityWindow(AvailabilityWindowParametersV1),
    #[serde(rename = "official.workforce.setup.assignment_inspection")]
    AssignmentInspection(AssignmentInspectionParametersV1),
    #[serde(rename = "eutheto.setup.command_changes")]
    CommandChanges(PageParametersV1),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmptyParametersV1 {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageParametersV1 {
    #[serde(default = "default_page_limit")]
    pub limit: u16,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum WorkforceEntityKindV1 {
    Person,
    Qualification,
    Team,
    Location,
    WorkloadBucket,
    Calendar,
    AssignmentType,
    ShiftTemplate,
    ShiftInstance,
    Availability,
    CoverageRequirement,
    BaseSchedule,
    ScorePolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityPageParametersV1 {
    pub entity_kind: WorkforceEntityKindV1,
    // Empty matches all. Literal Unicode-lowercase substring of the stored name;
    // not a regex/expression. Unnamed kinds support empty search only.
    pub search: String,
    #[serde(default = "default_page_limit")]
    pub limit: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDetailParametersV1 {
    pub entity_kind: WorkforceEntityKindV1,
    pub entity_id: EntityId,
}

#[derive(Clone, Copy, Debug, Eq, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum RuleClassV1 {
    Required,
    Preference,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleReferenceV1 {
    pub class: RuleClassV1,
    pub rule_id: RuleId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RulePageParametersV1 {
    pub class: RuleClassV1,
    // None means all kinds in that class. Some must name an existing catalog kind.
    pub kind_id: Option<String>,
    #[serde(default = "default_page_limit")]
    pub limit: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleDetailParametersV1 {
    pub rule: RuleReferenceV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ScopeAxisV1 {
    People,
    Shifts,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ScopePartV1 {
    Main,
    MinimumRestBefore,
    MinimumRestAfter,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleScopeParametersV1 {
    pub rule: RuleReferenceV1,
    pub part: ScopePartV1,
    pub axis: ScopeAxisV1,
    #[serde(default = "default_page_limit")]
    pub limit: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkWindowParametersV1 {
    pub dates: DateRange,
    #[serde(default = "default_page_limit")]
    pub limit: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkDetailParametersV1 {
    pub shift_id: ShiftId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationReviewParametersV1 {
    pub dates: Option<DateRange>,
    pub changes_only: bool,
    #[serde(default = "default_page_limit")]
    pub limit: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityMatrixParametersV1 {
    pub person_ids: Vec<PersonId>,
    pub assignment_type_ids: Vec<AssignmentTypeId>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailabilityWindowParametersV1 {
    pub person_id: PersonId,
    pub dates: DateRange,
    #[serde(default = "default_page_limit")]
    pub limit: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentInspectionParametersV1 {
    pub person_id: PersonId,
    pub shift_id: ShiftId,
    #[serde(default = "default_page_limit")]
    pub limit: u16,
}

// Position meanings are variant-specific, with unknown/wrong positions rejected.
// Entity and rule pages: exclusive previous ID. Work: exclusive (instant, ShiftId).
// Generation: exclusive ShiftId. Scope: exclusive PersonId or ShiftId for its axis.
// Ordered command changes, availability occurrences and pair rejections: next absolute ordinal.
// Valid next ordinals span 0..=total; total yields an exhausted empty page.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkforcePositionV1 {
    Entity {
        entity_id: EntityId,
    },
    Rule {
        rule_id: RuleId,
    },
    Person {
        person_id: PersonId,
    },
    Shift {
        shift_id: ShiftId,
    },
    TimedShift {
        starts_at: Rfc3339Timestamp,
        shift_id: ShiftId,
    },
    Ordinal {
        next_ordinal: u32,
    },
}

// total_items is the exact number matching this query BEFORE continuation, not just
// the visited suffix. No lower-bound estimate. A byte ceiling may shorten a page.
// Empty result => total_items=0/items=[]/continuation=None. An exhausted valid page has
// continuation=None; inability to fit a first remaining complete row is a resource error.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetupPageV1<T> {
    pub total_items: u32,
    pub items: Vec<T>,
    pub continuation: Option<SetupContinuationV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkforceSetupResultV1 {
    pub schema_version: u32,
    pub result: WorkforceSetupViewDataV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum WorkforceSetupViewDataV1 {
    Overview(WorkforceSetupFactsV1),
    SettingsPreparation(ScenarioSettings),
    EntityPage(SetupPageV1<EntitySummaryV1>),
    EntityDetail(Box<WorkforceEntity>),
    RuleCatalog(RuleCatalogV1),
    RulePage(SetupPageV1<RuleSummaryV1>),
    RuleDetail(Box<RuleRecordV1>),
    RuleScope(ScopeInspectionV1),
    WorkWindow(SetupPageV1<WorkShiftV1>),
    WorkDetail(Box<WorkShiftDetailV1>),
    GenerationReview(GenerationReviewV1),
    EligibilityMatrix(EligibilityMatrixV1),
    AvailabilityWindow(SetupPageV1<AvailabilityOccurrenceV1>),
    AssignmentInspection(AssignmentInspectionV1),
    CommandChanges(SetupPageV1<CommandChangeV1>),
}

// Facts, not a completeness/feasibility boolean. Exactly one entry for each of the13
// entity kinds, including zero. Active counts mean stored active=true, not applicability.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkforceSetupFactsV1 {
    pub settings: ScenarioSettings,
    pub entities: Vec<EntityKindCountV1>,
    pub required_rules: u32,
    pub active_required_rules: u32,
    pub preferences: u32,
    pub active_preferences: u32,
    pub locked_assignments: u32,
    pub configured_type_memberships: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityKindCountV1 {
    pub kind: WorkforceEntityKindV1,
    pub count: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntitySummaryV1 {
    pub entity_id: EntityId,
    pub kind: WorkforceEntityKindV1,
    // Full stored name where the existing kind has one; null for unnamed kinds.
    // No notes, qualification arrays, ledgers, or synthesized identity-by-name.
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleCatalogV1 {
    pub required: Vec<RuleCatalogEntryV1>,
    pub preferences: Vec<RuleCatalogEntryV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleCatalogEntryV1 {
    pub descriptor: KindDescriptor,
    // Implementation support from existing compilation authority, not installed backend state.
    pub support: RuleSupportV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum RuleSupportV1 {
    Implemented,
    NotImplemented,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleSummaryV1 {
    pub rule: RuleReferenceV1,
    pub kind_id: String,
    pub active: bool,
    pub priority: Option<PreferencePriority>,
    pub weight: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "class",
    content = "record",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum RuleRecordV1 {
    Required(WorkforceRule),
    Preference(WorkforcePreference),
}

// These are populations selected by existing person_scope/shift_scope helpers, not
// constraint counts or proof of feasibility. counts cover the full captured horizon.
// Minimum-rest before/after intersect the main and selected sub-scope on both axes.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeInspectionV1 {
    pub rule: RuleReferenceV1,
    pub part: ScopePartV1,
    pub people_count: u32,
    pub shift_count: u32,
    pub cartesian_pair_count: u32,
    pub population: ScopePopulationV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "axis",
    content = "page",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum ScopePopulationV1 {
    People(SetupPageV1<PersonSummaryV1>),
    Shifts(SetupPageV1<WorkShiftV1>),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonSummaryV1 {
    pub person_id: PersonId,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedIntervalV1 {
    pub starts_at: eutheto_types::ResolvedLocalTime,
    pub ends_at: eutheto_types::ResolvedLocalTime,
}

// Exact display duration, not a new solver unit. seconds is a bounded canonical i64
// decimal string; nanoseconds is -999999999..=999999999, same sign as nonzero seconds.
// This preserves legal subminute input for review rather than silently rounding it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisplayDurationV1 {
    pub seconds: String,
    pub nanoseconds: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ShiftOriginV1 {
    Generated {
        template_id: ShiftTemplateId,
        occurrence_date: Date,
    },
    Detached {
        template_id: ShiftTemplateId,
        occurrence_date: Date,
    },
    Manual,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoverageSummaryV1 {
    pub minimum: u16,
    pub preferred: Option<u16>,
    pub maximum: Option<u16>,
    pub qualification_minimum_count: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkShiftV1 {
    pub shift_id: ShiftId,
    pub origin: ShiftOriginV1,
    pub assignment_type_id: AssignmentTypeId,
    pub assignment_type_name: String,
    pub template_name: Option<String>,
    pub location_id: Option<crate::ids::LocationId>,
    pub location_name: Option<String>,
    pub coverage: CoverageSummaryV1,
    pub interval: ResolvedIntervalV1,
    pub reporting_date: Date,
    pub elapsed: DisplayDurationV1,
    pub scheduled: DisplayDurationV1,
}

// The source is the full typed template or stored instance. Generated occurrences are
// never presented as directly editable stored entity records; detach is an existing command.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkShiftDetailV1 {
    pub shift: WorkShiftV1,
    pub source: WorkShiftSourceV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "record",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum WorkShiftSourceV1 {
    Template(crate::model::ShiftTemplate),
    Instance(crate::model::ShiftInstance),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationReviewV1 {
    // Existing whole-document generation authority computes reconciliation first.
    pub prospective_hash: [u8; 32],
    pub reconciliation_required: bool,
    pub total_added: u32,
    pub total_changed: u32,
    pub total_removed: u32,
    pub page: SetupPageV1<GenerationRowV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationRowV1 {
    pub shift_id: ShiftId,
    pub before: Option<PriorWorkShiftV1>,
    pub after: Option<WorkShiftV1>,
    pub change: Option<ShiftChangeKindV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum PriorWorkShiftV1 {
    Resolved(Box<WorkShiftV1>),
    Unresolved(PriorUnresolvedShiftV1),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PriorUnresolvedShiftV1 {
    pub origin: ShiftOriginV1,
    // Strict mapped TemporalIssueKind, not Debug text. The separate source enum remains
    // nonserializable; the wire mapping includes its exact resolution subtype.
    pub issue: TemporalIssueCodeV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "resolution",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum TemporalIssueCodeV1 {
    Resolution(eutheto_types::TimeResolutionFailureKind),
    DateOverflow,
    InvalidInterval,
    OutsideHorizon,
    UnreconciledIdentity,
    IdentityCollision,
    IdentityTransition,
    OccurrenceLimit,
    OutputLimit,
    CalendarLimit,
    CalendarOverlap,
    CalendarOrder,
    AmbiguousReportingDate,
    UnknownCalendar,
    InvalidQuery,
    DifferentScenario,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ShiftChangeKindV1 {
    Added,
    Changed,
    Removed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityMatrixV1 {
    // Echo unique axes in request order. Reject missing/wrong-kind IDs, never omit cells.
    pub person_ids: Vec<PersonId>,
    pub assignment_type_ids: Vec<AssignmentTypeId>,
    // Exactly people.len rows, each exactly types.len booleans. Configured membership only.
    pub configured_memberships: Vec<Vec<bool>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstantIntervalV1 {
    pub start: Rfc3339Timestamp,
    pub end: Rfc3339Timestamp,
}

// Expand with the existing availability_intervals authority, clip to the requested
// instant window/effective range, deduplicate within each availability record, then
// order (AvailabilityId, start, end). This describes configured records, not their rule effect.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailabilityOccurrenceV1 {
    pub ordinal: u32,
    pub availability_id: AvailabilityId,
    pub availability_kind: AvailabilityKind,
    pub interval: InstantIntervalV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentInspectionV1 {
    pub person_id: PersonId,
    pub shift_id: ShiftId,
    // Existing analyze_assignments candidate membership, NOT whole-model feasibility.
    pub candidate_in_implemented_assignment_graph: bool,
    pub remaining_required_rule_count: u32,
    pub rejections: SetupPageV1<PairRejectionV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairRejectionV1 {
    pub ordinal: u32,
    pub binding_id: RuleId,
    pub cause: RejectionCauseV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RejectionCauseV1 {
    OutsideActiveRange {
        allowed: InstantIntervalV1,
        outside: InstantIntervalV1,
    },
    AssignmentTypeNotAllowed {
        assignment_type_id: AssignmentTypeId,
    },
    QualificationExpression {
        assignment_type_id: AssignmentTypeId,
    },
    Unavailable {
        availability_id: AvailabilityId,
        overlap: InstantIntervalV1,
    },
    OutsideAvailableOnly {
        availability_id: AvailabilityId,
        uncovered: InstantIntervalV1,
    },
    ApprovedTimeOff {
        availability_id: AvailabilityId,
        overlap: InstantIntervalV1,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandChangeV1 {
    // Zero-based original change index. Repeated edits of the same path retain distinct rows.
    pub ordinal: u32,
    pub change: Change,
}

// Stored-only, no continuation. Rust resolves local midnight boundaries and checks the
// existing Workforce planning_dates contract; Vue only wraps the result in SetScenarioSettings.
// dates is the desired entire horizon, NOT a presentation window: no366-day view cap.
// Existing horizon validation and generation/command work budgets still apply.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPreparationParametersV1 {
    pub time_zone: eutheto_types::IanaTimeZone,
    pub dates: DateRange,
    pub locale: eutheto_types::LocaleTag,
    pub units: eutheto_types::UnitSystem,
    pub gap_policy: eutheto_types::GapPolicy,
    pub overlap_policy: eutheto_types::OverlapPolicy,
}

impl WorkforceSetupQueryV1 {
    /// Decodes the closed query variant directly from borrowed parameters.
    ///
    /// This checks the query version and wire shape, not runtime parameter bounds,
    /// continuation identity or whether the selected source supports this query.
    ///
    /// # Errors
    /// Returns an error for an unsupported version, unknown view ID or invalid parameters.
    pub fn decode(query: &DomainSetupQueryV1) -> Result<Self, serde_json::Error> {
        if query.schema_version != 1 {
            return Err(serde::de::Error::custom(
                "unsupported Workforce setup query version",
            ));
        }
        match query.view_id.as_str() {
            "official.workforce.setup.overview" => {
                Deserialize::deserialize(&query.parameters).map(Self::Overview)
            }
            "official.workforce.setup.settings_preparation" => {
                Deserialize::deserialize(&query.parameters).map(Self::SettingsPreparation)
            }
            "eutheto.setup.entity_page" => {
                Deserialize::deserialize(&query.parameters).map(Self::EntityPage)
            }
            "eutheto.setup.entity_detail" => {
                Deserialize::deserialize(&query.parameters).map(Self::EntityDetail)
            }
            "eutheto.setup.rule_catalog" => {
                Deserialize::deserialize(&query.parameters).map(Self::RuleCatalog)
            }
            "official.workforce.setup.rule_page" => {
                Deserialize::deserialize(&query.parameters).map(Self::RulePage)
            }
            "official.workforce.setup.rule_detail" => {
                Deserialize::deserialize(&query.parameters).map(Self::RuleDetail)
            }
            "official.workforce.setup.rule_scope" => {
                Deserialize::deserialize(&query.parameters).map(Self::RuleScope)
            }
            "official.workforce.setup.work_window" => {
                Deserialize::deserialize(&query.parameters).map(Self::WorkWindow)
            }
            "official.workforce.setup.work_detail" => {
                Deserialize::deserialize(&query.parameters).map(Self::WorkDetail)
            }
            "official.workforce.setup.generation_review" => {
                Deserialize::deserialize(&query.parameters).map(Self::GenerationReview)
            }
            "official.workforce.setup.eligibility_matrix" => {
                Deserialize::deserialize(&query.parameters).map(Self::EligibilityMatrix)
            }
            "official.workforce.setup.availability_window" => {
                Deserialize::deserialize(&query.parameters).map(Self::AvailabilityWindow)
            }
            "official.workforce.setup.assignment_inspection" => {
                Deserialize::deserialize(&query.parameters).map(Self::AssignmentInspection)
            }
            "eutheto.setup.command_changes" => {
                Deserialize::deserialize(&query.parameters).map(Self::CommandChanges)
            }
            _ => Err(serde::de::Error::custom("unknown Workforce setup view ID")),
        }
    }

    /// Stable setup-query identity, never an accepted-solution result-view identity.
    #[must_use]
    pub const fn view_id(&self) -> &'static str {
        match self {
            Self::Overview(_) => "official.workforce.setup.overview",
            Self::SettingsPreparation(_) => "official.workforce.setup.settings_preparation",
            Self::EntityPage(_) => "eutheto.setup.entity_page",
            Self::EntityDetail(_) => "eutheto.setup.entity_detail",
            Self::RuleCatalog(_) => "eutheto.setup.rule_catalog",
            Self::RulePage(_) => "official.workforce.setup.rule_page",
            Self::RuleDetail(_) => "official.workforce.setup.rule_detail",
            Self::RuleScope(_) => "official.workforce.setup.rule_scope",
            Self::WorkWindow(_) => "official.workforce.setup.work_window",
            Self::WorkDetail(_) => "official.workforce.setup.work_detail",
            Self::GenerationReview(_) => "official.workforce.setup.generation_review",
            Self::EligibilityMatrix(_) => "official.workforce.setup.eligibility_matrix",
            Self::AvailabilityWindow(_) => "official.workforce.setup.availability_window",
            Self::AssignmentInspection(_) => "official.workforce.setup.assignment_inspection",
            Self::CommandChanges(_) => "eutheto.setup.command_changes",
        }
    }
}

impl WorkforceSetupViewDataV1 {
    /// Stable query identity corresponding exactly to this result discriminator.
    #[must_use]
    pub const fn view_id(&self) -> &'static str {
        match self {
            Self::Overview(_) => "official.workforce.setup.overview",
            Self::SettingsPreparation(_) => "official.workforce.setup.settings_preparation",
            Self::EntityPage(_) => "eutheto.setup.entity_page",
            Self::EntityDetail(_) => "eutheto.setup.entity_detail",
            Self::RuleCatalog(_) => "eutheto.setup.rule_catalog",
            Self::RulePage(_) => "official.workforce.setup.rule_page",
            Self::RuleDetail(_) => "official.workforce.setup.rule_detail",
            Self::RuleScope(_) => "official.workforce.setup.rule_scope",
            Self::WorkWindow(_) => "official.workforce.setup.work_window",
            Self::WorkDetail(_) => "official.workforce.setup.work_detail",
            Self::GenerationReview(_) => "official.workforce.setup.generation_review",
            Self::EligibilityMatrix(_) => "official.workforce.setup.eligibility_matrix",
            Self::AvailabilityWindow(_) => "official.workforce.setup.availability_window",
            Self::AssignmentInspection(_) => "official.workforce.setup.assignment_inspection",
            Self::CommandChanges(_) => "eutheto.setup.command_changes",
        }
    }
}
