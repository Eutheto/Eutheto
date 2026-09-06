//! Stable Phase-01 documents, commands, errors, views, and event contracts.

use crate::budget::DurationMillis;
use crate::ids::{
    AssignmentId, BackendId, CommandId, CounterfactualJobId, EntityId, PackId, PersonId, RequestId,
    RuleId, ScenarioId, SolutionId, SolveRunId,
};
use crate::values::{
    GapPolicy, Horizon, IanaTimeZone, LocaleTag, OverlapPolicy, Revision, Rfc3339Timestamp,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

/// Current scenario envelope format version.
pub const SCENARIO_FORMAT_VERSION: u32 = 1;

/// Maximum serialized size of one authoritative or portable scenario document.
pub const MAX_SCENARIO_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;

/// Current redacted support-preview schema version.
pub const SUPPORT_PREVIEW_SCHEMA_VERSION: u32 = 1;

/// Stable namespace of a scenario document.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ScenarioFormat {
    /// The canonical Eutheto scenario envelope.
    #[serde(rename = "eutheto/scenario")]
    EuthetoScenario,
}

/// Reference to a domain pack and its independent schema version.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainPackRef {
    /// Stable pack identity.
    pub id: PackId,
    /// Pack-owned domain schema version.
    pub schema_version: u32,
}

/// Human metadata and authoritative timestamps for a scenario.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioMetadata {
    /// Human-readable title.
    pub title: String,
    /// Optional prose represented as an empty string when absent.
    pub description: String,
    /// Creation instant.
    pub created_at: Rfc3339Timestamp,
    /// Last authoritative update instant.
    pub updated_at: Rfc3339Timestamp,
}

/// Display unit system selected by the scenario.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnitSystem {
    /// Metric display units.
    Metric,
    /// United States customary display units.
    UsCustomary,
}

/// Explicit scenario-wide settings that must not come from the host machine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioSettings {
    /// IANA time-zone identifier.
    pub time_zone: IanaTimeZone,
    /// Locale used for display and explicit locale-sensitive pack behavior.
    pub locale: LocaleTag,
    /// Display unit system.
    pub units: UnitSystem,
    /// Half-open planning horizon.
    pub horizon: Horizon,
    /// Nonexistent-local-time policy.
    pub gap_policy: GapPolicy,
    /// Repeated-local-time policy.
    pub overlap_policy: OverlapPolicy,
}

/// Generic Phase-01 domain payload with stable typed map keys.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioDomain {
    /// Entity records keyed by stable identity.
    #[serde(deserialize_with = "crate::deserialize_unique_id_map")]
    pub entities: BTreeMap<EntityId, Value>,
    /// Rule records keyed by stable identity.
    #[serde(deserialize_with = "crate::deserialize_unique_id_map")]
    pub rules: BTreeMap<RuleId, Value>,
    /// Preference records keyed by stable identity.
    #[serde(deserialize_with = "crate::deserialize_unique_id_map")]
    pub preferences: BTreeMap<RuleId, Value>,
    /// Locked assignment records keyed by stable identity.
    #[serde(deserialize_with = "crate::deserialize_unique_id_map")]
    pub locked_assignments: BTreeMap<AssignmentId, Value>,
}

impl ScenarioDomain {
    /// Checks the fixed host-owned shape of a pack's internal JSON Schema.
    ///
    /// Record schemas and keyword validity remain the pack catalog's responsibility.
    #[must_use]
    pub fn has_schema_shape(schema: &Value) -> bool {
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return false;
        };
        let Some(required) = schema.get("required").and_then(Value::as_array) else {
            return false;
        };
        let fields = ["entities", "rules", "preferences", "lockedAssignments"];
        schema.get("type").and_then(Value::as_str) == Some("object")
            && schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
            && properties.len() == fields.len()
            && required.len() == fields.len()
            && fields.iter().all(|field| {
                let Some(map) = properties.get(*field) else {
                    return false;
                };
                map.get("type").and_then(Value::as_str) == Some("object")
                    && map.get("properties").is_none()
                    && map
                        .get("additionalProperties")
                        .and_then(|record| record.get("type"))
                        .and_then(Value::as_str)
                        == Some("object")
                    && required.iter().any(|item| item.as_str() == Some(*field))
            })
    }
}

/// Strict version-1 scenario envelope.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioDocument {
    /// Stable format namespace.
    pub format: ScenarioFormat,
    /// Envelope schema version, independent from the pack schema.
    pub format_version: u32,
    /// Stable scenario identity.
    pub scenario_id: ScenarioId,
    /// Selected domain pack and domain schema version.
    pub domain_pack: DomainPackRef,
    /// Human metadata.
    pub metadata: ScenarioMetadata,
    /// Explicit time, locale, and unit settings.
    pub settings: ScenarioSettings,
    /// Pack-independent Phase-01 domain maps.
    pub domain: ScenarioDomain,
    /// Preserve-through-round-trip nonsemantic extension values.
    pub extensions: BTreeMap<String, Value>,
}

impl ScenarioDocument {
    /// Creates a current-version scenario document.
    #[must_use]
    pub fn new(
        scenario_id: ScenarioId,
        domain_pack: DomainPackRef,
        metadata: ScenarioMetadata,
        settings: ScenarioSettings,
        domain: ScenarioDomain,
        extensions: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            format: ScenarioFormat::EuthetoScenario,
            format_version: SCENARIO_FORMAT_VERSION,
            scenario_id,
            domain_pack,
            metadata,
            settings,
            domain,
            extensions,
        }
    }
}

impl<'de> Deserialize<'de> for ScenarioDocument {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct SerializedScenarioDocument {
            format: ScenarioFormat,
            format_version: u32,
            scenario_id: ScenarioId,
            domain_pack: DomainPackRef,
            metadata: ScenarioMetadata,
            settings: ScenarioSettings,
            domain: ScenarioDomain,
            extensions: BTreeMap<String, Value>,
        }

        let value = SerializedScenarioDocument::deserialize(deserializer)?;
        if value.format_version != SCENARIO_FORMAT_VERSION {
            return Err(serde::de::Error::custom(format_args!(
                "unsupported scenario format version {}; expected {}",
                value.format_version, SCENARIO_FORMAT_VERSION
            )));
        }
        Ok(Self {
            format: value.format,
            format_version: value.format_version,
            scenario_id: value.scenario_id,
            domain_pack: value.domain_pack,
            metadata: value.metadata,
            settings: value.settings,
            domain: value.domain,
            extensions: value.extensions,
        })
    }
}

/// Actor metadata stored in the local command journal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorRef {
    /// Stable application-owned actor reference when one exists.
    pub actor_id: Option<String>,
    /// User-safe display label.
    pub display_name: String,
}

/// Source through which a command entered the application service.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandSource {
    /// Desktop application interaction.
    Desktop,
    /// Headless command-line interaction.
    Cli,
    /// Portable import operation.
    Import,
    /// Internal application operation.
    System,
    /// Journal undo operation.
    Undo,
    /// Journal redo operation.
    Redo,
}

/// Adds a generic entity.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddEntity {
    /// New entity identity.
    pub entity_id: EntityId,
    /// Pack-owned entity value.
    pub value: Value,
}

/// Replaces a generic entity value.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateEntity {
    /// Existing entity identity.
    pub entity_id: EntityId,
    /// Replacement pack-owned value.
    pub value: Value,
}

/// Removes a generic entity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoveEntity {
    /// Existing entity identity.
    pub entity_id: EntityId,
}

/// Adds a generic rule.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddRule {
    /// New rule identity.
    pub rule_id: RuleId,
    /// Pack-owned rule value.
    pub value: Value,
}

/// Replaces a generic rule value.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateRule {
    /// Existing rule identity.
    pub rule_id: RuleId,
    /// Replacement pack-owned value.
    pub value: Value,
}

/// Removes a generic rule.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoveRule {
    /// Existing rule identity.
    pub rule_id: RuleId,
}

/// Inserts, replaces, or removes a preference.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetPreference {
    /// Preference identity in the rule identity space.
    pub preference_id: RuleId,
    /// New value, or `None` to remove the preference.
    pub value: Option<Value>,
}

/// Creates or replaces an assignment lock.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockAssignment {
    /// Assignment identity.
    pub assignment_id: AssignmentId,
    /// Pack-owned lock value.
    pub value: Value,
}

/// Removes an assignment lock.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnlockAssignment {
    /// Locked assignment identity.
    pub assignment_id: AssignmentId,
}

/// Namespaced pack-owned command payload.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainCommandEnvelope {
    /// Stable pack-owned command type.
    pub command_type: String,
    /// Pack-owned command payload.
    pub payload: Value,
}

/// Atomic ordered command batch.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandBatch {
    /// Optional user-safe journal label.
    pub label: Option<String>,
    /// Commands applied in this order.
    pub commands: Vec<ScenarioCommand>,
}

/// Canonical Phase-01 scenario mutation catalog.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "payload")]
pub enum ScenarioCommand {
    /// Add an entity.
    AddEntity(AddEntity),
    /// Update an entity.
    UpdateEntity(UpdateEntity),
    /// Remove an entity.
    RemoveEntity(RemoveEntity),
    /// Add a rule.
    AddRule(AddRule),
    /// Update a rule.
    UpdateRule(UpdateRule),
    /// Remove a rule.
    RemoveRule(RemoveRule),
    /// Set or clear a preference.
    SetPreference(SetPreference),
    /// Lock an assignment.
    LockAssignment(LockAssignment),
    /// Unlock an assignment.
    UnlockAssignment(UnlockAssignment),
    /// Apply a pack-owned command.
    ApplyDomainCommand(DomainCommandEnvelope),
    /// Apply an ordered batch atomically.
    ApplyBatch(CommandBatch),
}

/// Complete optimistic-concurrency command request.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandEnvelope {
    /// Unique command identity.
    pub command_id: CommandId,
    /// Target scenario.
    pub scenario_id: ScenarioId,
    /// Revision the caller observed.
    pub expected_revision: Revision,
    /// Journal actor metadata.
    pub actor: ActorRef,
    /// Mutation entry point.
    pub source: CommandSource,
    /// Pure scenario mutation.
    pub command: ScenarioCommand,
}

/// Kind of one observable document change.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    /// A value was added.
    Added,
    /// A value was updated.
    Updated,
    /// A value was removed.
    Removed,
    /// An assignment was locked.
    Locked,
    /// An assignment was unlocked.
    Unlocked,
}

/// One redaction-safe structural document change.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    /// Change classification.
    pub kind: ChangeKind,
    /// Stable JSON-pointer-like field path.
    pub path: String,
    /// Previous value when applicable.
    pub before: Option<Value>,
    /// New value when applicable.
    pub after: Option<Value>,
}

/// Ordered changes produced by one command.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeSet {
    /// Individual changes in deterministic order.
    pub changes: Vec<Change>,
}

/// User-visible validation severity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationSeverity {
    /// Informational finding.
    Info,
    /// Reviewable warning.
    Warning,
    /// Blocking validation error.
    Error,
}

/// One stable, field-addressable validation finding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationIssue {
    /// Stable machine-readable issue code.
    pub code: String,
    /// Finding severity.
    pub severity: ValidationSeverity,
    /// User-safe message.
    pub message: String,
    /// Stable field path when applicable.
    pub field_path: Option<String>,
    /// Related typed resource when applicable.
    pub resource: Option<ResourceRef>,
}

/// Complete validation state for a document or operation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationReport {
    /// Findings in deterministic display order.
    pub issues: Vec<ValidationIssue>,
}

/// Validation changes caused by one mutation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationDelta {
    /// Newly present findings.
    pub added: Vec<ValidationIssue>,
    /// Stable issue codes no longer present.
    pub resolved: Vec<String>,
}

/// Result of atomically applying a scenario command.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandResult {
    /// Revision after the command commits.
    pub new_revision: Revision,
    /// Deterministic structural changes.
    pub change_set: ChangeSet,
    /// Validation findings added and resolved.
    pub validation_delta: ValidationDelta,
    /// Exact inverse when the command is reversible.
    pub inverse: Option<ScenarioCommand>,
}

/// Typed resource reference that contains no filesystem or secret material.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "id")]
pub enum ResourceRef {
    /// Scenario resource.
    Scenario(ScenarioId),
    /// Person resource.
    Person(PersonId),
    /// Rule or preference resource.
    Rule(RuleId),
    /// Assignment resource.
    Assignment(AssignmentId),
    /// Solve-run resource.
    SolveRun(SolveRunId),
    /// Solution resource.
    Solution(SolutionId),
    /// Domain pack resource.
    Pack(PackId),
    /// Solver backend resource.
    Backend(BackendId),
}

/// Unsupported capability reported without pretending it is available.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnsupportedFeature {
    /// Stable machine-readable code.
    pub code: String,
    /// User-safe capability label.
    pub capability: String,
}

macro_rules! safe_failure {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name {
            /// Stable machine-readable failure code.
            pub code: String,
            /// User-safe localized-or-localizable message.
            pub message: String,
            /// Whether retrying without changing input may succeed.
            pub retryable: bool,
            /// Correlation identifier for separately retained redacted diagnostics.
            pub diagnostic_id: Option<RequestId>,
        }
    };
}

safe_failure!(SolverFailure, "User-safe solver subsystem failure.");
safe_failure!(
    VerificationFailure,
    "User-safe independent verification failure."
);
safe_failure!(
    StorageFailure,
    "User-safe storage failure with no raw database text or path."
);
safe_failure!(
    ProtocolFailure,
    "User-safe protocol failure with no raw transport payload."
);
safe_failure!(
    AiFailure,
    "User-safe AI subsystem failure with no provider secret or response."
);

/// Typed application failure serialized with stable category tagging.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "category", content = "details")]
pub enum AppError {
    /// Input or document validation failed.
    Validation(ValidationReport),
    /// Optimistic revision did not match.
    Conflict {
        /// Revision supplied by the caller.
        expected_revision: Revision,
        /// Current durable revision.
        actual_revision: Revision,
    },
    /// A typed resource does not exist.
    NotFound(ResourceRef),
    /// A known capability is not available.
    Unsupported(UnsupportedFeature),
    /// Solver subsystem failed safely.
    Solver(SolverFailure),
    /// Independent verification detected a failure.
    Verification(VerificationFailure),
    /// Persistence failed safely.
    Storage(StorageFailure),
    /// API or worker protocol failed safely.
    Protocol(ProtocolFailure),
    /// AI subsystem failed safely.
    Ai(AiFailure),
    /// Unexpected failure; diagnostics are only available by incident identity.
    Internal {
        /// Redacted diagnostic correlation identity.
        incident_id: RequestId,
    },
}

/// Selection of an automatic or explicit solver backend.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    tag = "kind",
    content = "backendId",
    deny_unknown_fields
)]
pub enum BackendSelection {
    /// Let the deterministic router choose.
    Auto,
    /// Require one backend.
    Specific(BackendId),
}

/// User-facing solve effort mode; no variant promises optimality.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SolveMode {
    /// Short interactive search.
    Quick,
    /// Default quality/responsiveness balance.
    Balanced,
    /// Longer continued improvement.
    Deep,
    /// Explicit advanced settings.
    Custom,
}

/// Worker thread allocation policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    tag = "kind",
    content = "count",
    deny_unknown_fields
)]
pub enum WorkerThreadPolicy {
    /// Let the application apply its bounded default.
    Auto,
    /// Use exactly this positive number of threads.
    Exact(u16),
}

/// Requested explanation work.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExplanationMode {
    /// Do not request explanation work.
    None,
    /// Produce normal user-facing evidence.
    Standard,
    /// Produce bounded detailed diagnostics.
    Detailed,
}

/// Existing-solution preservation policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreservationPolicy {
    /// Do not preserve an existing solution.
    None,
    /// Prefer existing choices when compatible.
    PreferExisting,
    /// Treat preserved choices as required locks.
    RequireExisting,
}

/// Reproducibility trade-off selected by the caller.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReproducibilityMode {
    /// Permit supported performance-oriented nondeterminism.
    Performance,
    /// Require deterministic supported execution settings.
    Deterministic,
}

/// Canonical normalized outcome of a solve attempt.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SolveStatus {
    /// Optimality was proven for the configured model and objective.
    Optimal,
    /// At least one independently verified feasible solution exists.
    Feasible,
    /// Infeasibility was proven.
    Infeasible,
    /// The model was proven unbounded.
    Unbounded,
    /// No independently verified solution was found within the limit.
    NoSolutionWithinLimit,
    /// The solve was cancelled.
    Cancelled,
    /// Model validation failed before backend execution.
    InvalidModel,
    /// The selected backend was unavailable.
    BackendUnavailable,
    /// The backend failed without a trusted result.
    BackendFailed,
}

/// Stable coarse phase used by solver progress events.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SolvePhase {
    /// Waiting for bounded execution capacity.
    Queued,
    /// Validating and compiling the immutable scenario revision.
    Compiling,
    /// Running the selected backend.
    Solving,
    /// Projecting and independently verifying a candidate.
    Verifying,
    /// Producing bounded explanation evidence.
    Explaining,
}

/// Stable coarse phase used by counterfactual progress events.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CounterfactualProgressPhase {
    /// Waiting for the job to begin execution.
    Queued,
    /// Compiling the base and temporary counterfactual models.
    Compiling,
    /// Running the selected backend.
    Solving,
    /// Independently reviewing a candidate.
    Verifying,
    /// Persisting the authoritative terminal outcome.
    Finalizing,
    /// The job committed a completed result.
    Completed,
    /// The job committed a failure.
    Failed,
    /// The job committed cancellation.
    Cancelled,
    /// The job was interrupted before a normal terminal outcome.
    Interrupted,
}

/// Bounded resources shared by compile, solve, projection, and verification.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceLimits {
    /// Maximum generic entities accepted in one operation.
    pub max_entities: u32,
    /// Maximum rules accepted in one operation.
    pub max_rules: u32,
    /// Maximum generated model variables.
    pub max_variables: u64,
    /// Maximum generated model constraints.
    pub max_constraints: u64,
}

/// Canonical options carried end to end through a solve.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolveOptions {
    /// Backend routing request.
    pub backend: BackendSelection,
    /// User-facing effort mode.
    pub mode: SolveMode,
    /// One whole-millisecond end-to-end parent deadline budget.
    pub time_limit_milliseconds: DurationMillis,
    /// Optional process memory ceiling.
    pub memory_limit_bytes: Option<u64>,
    /// Worker thread policy.
    pub worker_threads: WorkerThreadPolicy,
    /// Explicit random seed.
    pub random_seed: u64,
    /// Optional accepted-solution count ceiling.
    pub solution_limit: Option<u32>,
    /// Stop after the first independently verified feasible result.
    pub stop_after_first_feasible: bool,
    /// Retain bounded intermediate verified solutions.
    pub collect_intermediate_solutions: bool,
    /// Explanation work request.
    pub explanation_mode: ExplanationMode,
    /// Existing-solution preservation policy.
    pub preserve_existing: PreservationPolicy,
    /// Reproducibility selection.
    pub reproducibility: ReproducibilityMode,
    /// Shared generation and execution limits.
    pub resource_limits: ResourceLimits,
}

impl SolveOptions {
    /// Validates nonzero optional limits and exact worker allocation.
    ///
    /// # Errors
    /// Rejects a zero memory limit, exact worker count, or solution limit.
    pub fn validate(&self) -> Result<(), SolveOptionsError> {
        if self.memory_limit_bytes == Some(0) {
            return Err(SolveOptionsError::ZeroMemoryLimit);
        }
        if matches!(self.worker_threads, WorkerThreadPolicy::Exact(0)) {
            return Err(SolveOptionsError::ZeroWorkerThreads);
        }
        if self.solution_limit == Some(0) {
            return Err(SolveOptionsError::ZeroSolutionLimit);
        }
        Ok(())
    }
}

/// Invalid canonical solve options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SolveOptionsError {
    /// A present process memory ceiling is zero.
    ZeroMemoryLimit,
    /// An exact worker allocation is zero.
    ZeroWorkerThreads,
    /// A present accepted-solution ceiling is zero.
    ZeroSolutionLimit,
}

impl fmt::Display for SolveOptionsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid solve options: {self:?}")
    }
}

impl std::error::Error for SolveOptionsError {}

/// Project list item returned by application APIs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSummaryDto {
    /// Scenario identity backing the project.
    pub scenario_id: ScenarioId,
    /// Project title.
    pub title: String,
    /// Pack identity.
    pub domain_pack_id: PackId,
    /// Current durable revision.
    pub revision: Revision,
    /// Last update time.
    pub updated_at: Rfc3339Timestamp,
    /// Whether the project is archived.
    pub archived: bool,
}

/// Detailed project metadata view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectMetadataDto {
    /// Scenario identity backing the project.
    pub scenario_id: ScenarioId,
    /// Project title.
    pub title: String,
    /// Project description.
    pub description: String,
    /// Pack identity and schema.
    pub domain_pack: DomainPackRef,
    /// Current durable revision.
    pub revision: Revision,
    /// Creation instant.
    pub created_at: Rfc3339Timestamp,
    /// Last update instant.
    pub updated_at: Rfc3339Timestamp,
    /// Archive instant when archived.
    pub archived_at: Option<Rfc3339Timestamp>,
}

/// Lightweight scenario state view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioSummaryDto {
    /// Scenario identity.
    pub scenario_id: ScenarioId,
    /// Current revision.
    pub revision: Revision,
    /// Scenario title.
    pub title: String,
    /// Current validation report.
    pub validation: ValidationReport,
}

/// Complete Phase-01 scenario API view.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioViewDto {
    /// Current durable revision.
    pub revision: Revision,
    /// Current authoritative document.
    pub document: ScenarioDocument,
    /// Current validation report.
    pub validation: ValidationReport,
}

/// Coarse status for a private application directory.
///
/// This label deliberately carries no path, filename, error detail, or environment value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DirectoryAvailabilityLabel {
    /// The expected directory is currently available.
    Available,
    /// The expected directory is absent, inaccessible, or is not a directory.
    Unavailable,
}

/// Redaction-safe application identity included in a support preview.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupportApplicationMetadataDto {
    /// Compile-time application name.
    pub name: String,
    /// Compile-time application version.
    pub version: String,
}

/// Version metadata for persisted formats, without schema internals.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupportSchemaMetadataDto {
    /// Current scenario-envelope schema version.
    pub scenario_format_version: u32,
    /// Schema version observed during storage initialization.
    pub storage_schema_version: u32,
}

/// Aggregate local-library metadata containing no scenario identity or content.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupportLibraryMetadataDto {
    /// Current durable library revision.
    pub revision: Revision,
    /// Number of stored scenarios, including archived scenarios.
    pub scenario_count: u64,
    /// Number of active solve runs.
    ///
    /// Phase 01 cannot start solve work, and startup recovery transitions every
    /// formerly active run to interrupted before the application is constructed.
    pub active_solve_run_count: u64,
    /// Number of formerly active solve runs interrupted during this startup.
    pub interrupted_recovery_count: u64,
}

/// Availability of private application directories, represented without their paths.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupportDirectoryMetadataDto {
    /// Availability of the directory containing the local application database.
    pub application_data: DirectoryAvailabilityLabel,
    /// Availability of the private safety-backup directory.
    pub safety_backups: DirectoryAvailabilityLabel,
}

/// Versioned allowlist used to preview safe support metadata.
///
/// By construction this type has no fields for paths, filenames, documents,
/// logs, credentials, secrets, database bytes, or environment values.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupportPreviewDto {
    /// Schema version for this support-preview payload.
    pub schema_version: u32,
    /// Time at which the preview was generated, supplied by the application clock.
    pub generated_at: Rfc3339Timestamp,
    /// Safe compile-time application identity.
    pub application: SupportApplicationMetadataDto,
    /// Safe persisted-format versions.
    pub schemas: SupportSchemaMetadataDto,
    /// Aggregate library state.
    pub library: SupportLibraryMetadataDto,
    /// Coarse private-directory availability.
    pub directories: SupportDirectoryMetadataDto,
}

/// Required correlation and optimistic-concurrency fields for mutations.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutationContextDto {
    /// Target scenario.
    pub scenario_id: ScenarioId,
    /// Revision observed by the caller.
    pub expected_revision: Revision,
    /// Request correlation identity.
    pub request_id: RequestId,
}

/// API error category independent from Rust implementation types.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiErrorCategoryDto {
    /// Validation failure.
    Validation,
    /// Revision or state conflict.
    Conflict,
    /// Missing resource.
    NotFound,
    /// Unavailable capability.
    Unsupported,
    /// Solver failure.
    Solver,
    /// Verification correctness alarm.
    Verification,
    /// Storage or portable-data failure.
    Storage,
    /// Protocol failure.
    Protocol,
    /// AI provider or credential failure.
    Ai,
    /// Unexpected internal failure.
    Internal,
}

/// Field-addressable API error detail.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldErrorDto {
    /// Stable field path.
    pub field: String,
    /// Stable error code.
    pub code: String,
    /// User-safe message.
    pub message: String,
}

/// Bounded scalar value allowed in serialized diagnostic details.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum SafeDiagnosticValue {
    /// Redacted, user-safe text.
    Text(String),
    /// Integer fact.
    Integer(i64),
    /// Boolean fact.
    Boolean(bool),
}

/// Stable API error containing only user-safe data and diagnostic correlation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiErrorDto {
    /// Stable machine-readable error code.
    pub code: String,
    /// User-safe message.
    pub message: String,
    /// Stable broad category.
    pub category: ApiErrorCategoryDto,
    /// Whether an unchanged retry may succeed.
    pub retryable: bool,
    /// Field-level findings.
    pub field_errors: Vec<FieldErrorDto>,
    /// Bounded redacted scalar details; never raw paths, secrets, SQL, or backtraces.
    pub details: Option<BTreeMap<String, SafeDiagnosticValue>>,
    /// Correlation identity for separately retained diagnostics.
    pub diagnostic_id: Option<RequestId>,
}

/// Generic successful API response envelope.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiResponseDto<T> {
    /// Stable DTO schema version.
    pub schema_version: u32,
    /// Request correlation identity.
    pub request_id: RequestId,
    /// Current revision when the response is scenario-specific.
    pub current_revision: Option<Revision>,
    /// Nonblocking user-safe warnings.
    pub warnings: Vec<ValidationIssue>,
    /// Operation-specific result.
    pub result: T,
}

/// Stable application event topic names.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EventTopic {
    /// Solver progress event.
    #[serde(rename = "solve://progress")]
    SolveProgress,
    /// Solver completion event.
    #[serde(rename = "solve://completed")]
    SolveCompleted,
    /// Scenario changed event.
    #[serde(rename = "scenario://changed")]
    ScenarioChanged,
    /// Scenario validation changed event.
    #[serde(rename = "scenario://validation-changed")]
    ScenarioValidationChanged,
    /// Counterfactual progress event.
    #[serde(rename = "counterfactual://progress")]
    CounterfactualProgress,
    /// AI streaming event.
    #[serde(rename = "ai://stream")]
    AiStream,
    /// AI proposal readiness event.
    #[serde(rename = "ai://proposal-ready")]
    AiProposalReady,
    /// AI completion event.
    #[serde(rename = "ai://completed")]
    AiCompleted,
    /// Application update availability event.
    #[serde(rename = "update://available")]
    UpdateAvailable,
    /// Generic application notification.
    #[serde(rename = "app://notification")]
    AppNotification,
}

/// Common identity and ordering fields on every application event.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventContext {
    /// Event payload schema version.
    pub event_version: u32,
    /// Event creation instant.
    pub timestamp: Rfc3339Timestamp,
    /// Related request when applicable.
    pub request_id: Option<RequestId>,
    /// Related scenario when applicable.
    pub scenario_id: Option<ScenarioId>,
    /// Related revision when applicable.
    pub revision: Option<Revision>,
    /// Related solve run when applicable.
    pub solve_run_id: Option<SolveRunId>,
}

/// Basic Phase-01 event payloads; later phases add topic-specific detail versions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    content = "payload",
    deny_unknown_fields
)]
pub enum EventPayload {
    /// Bounded solver progress.
    SolveProgress {
        /// Common event metadata.
        context: EventContext,
        /// Current coarse solve phase.
        phase: SolvePhase,
        /// Optional integer percentage in the inclusive range 0–100.
        percent: Option<u8>,
    },
    /// Coarse progress for one counterfactual job.
    CounterfactualProgress {
        /// Common event metadata.
        context: EventContext,
        /// Stable counterfactual job identity.
        job_id: CounterfactualJobId,
        /// Current truthful coarse phase.
        phase: CounterfactualProgressPhase,
    },
    /// Final normalized solve outcome.
    SolveCompleted {
        /// Common event metadata.
        context: EventContext,
        /// Normalized solve status.
        status: SolveStatus,
        /// Independently verified solution when one exists.
        solution_id: Option<SolutionId>,
    },
    /// A committed scenario mutation.
    ScenarioChanged {
        /// Common event metadata.
        context: EventContext,
        /// Structural changes.
        change_set: ChangeSet,
    },
    /// Validation state changed.
    ScenarioValidationChanged {
        /// Common event metadata.
        context: EventContext,
        /// Validation changes.
        validation_delta: ValidationDelta,
    },
    /// User-safe application notification.
    AppNotification {
        /// Common event metadata.
        context: EventContext,
        /// Stable notification code.
        code: String,
        /// User-safe message.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        CounterfactualProgressPhase, EventPayload, SCENARIO_FORMAT_VERSION, ScenarioCommand,
        ScenarioDocument, SolveOptions, SolveOptionsError,
    };
    use serde_json::{Value, json};

    fn scenario_json() -> Value {
        json!({
            "format": "eutheto/scenario",
            "formatVersion": 1,
            "scenarioId": "018f47f2-e880-7000-8000-000000000001",
            "domainPack": {
                "id": "official.test",
                "schemaVersion": 1
            },
            "metadata": {
                "title": "Deterministic fixture",
                "description": "",
                "createdAt": "2026-08-28T23:00:00Z",
                "updatedAt": "2026-08-28T23:00:00Z"
            },
            "settings": {
                "timeZone": "America/Chicago",
                "locale": "en-US",
                "units": "us-customary",
                "horizon": {
                    "start": "2026-09-01T05:00:00Z",
                    "end": "2026-10-01T05:00:00Z"
                },
                "gapPolicy": "reject",
                "overlapPolicy": "earlier"
            },
            "domain": {
                "entities": {
                    "018f47f2-e880-7000-8000-000000000002": {"name": "Ada"}
                },
                "rules": {},
                "preferences": {},
                "lockedAssignments": {}
            },
            "extensions": {
                "vendor.example": {
                    "opaque": [1, "two", true]
                }
            }
        })
    }

    fn solve_options_json() -> Value {
        json!({
            "backend": {"kind": "auto"},
            "mode": "balanced",
            "timeLimitMilliseconds": 750,
            "memoryLimitBytes": null,
            "workerThreads": {"kind": "auto"},
            "randomSeed": 17,
            "solutionLimit": null,
            "stopAfterFirstFeasible": false,
            "collectIntermediateSolutions": false,
            "explanationMode": "standard",
            "preserveExisting": "none",
            "reproducibility": "deterministic",
            "resourceLimits": {
                "maxEntities": 100,
                "maxRules": 100,
                "maxVariables": 1_000,
                "maxConstraints": 2_000
            }
        })
    }

    #[test]
    fn scenario_envelope_round_trips_extensions() -> Result<(), serde_json::Error> {
        let input = scenario_json();
        let document: ScenarioDocument = serde_json::from_value(input.clone())?;
        assert_eq!(document.format_version, SCENARIO_FORMAT_VERSION);
        assert_eq!(serde_json::to_value(document)?, input);
        Ok(())
    }

    #[test]
    fn scenario_envelope_rejects_aliased_duplicate_record_ids() {
        let lower = "018f47f2-e880-7000-8000-0000000000ab";
        let upper = lower.to_uppercase();
        for section in ["entities", "rules", "preferences", "lockedAssignments"] {
            let mut input = scenario_json();
            input["domain"][section] = json!({
                lower: {"name": "first record"},
                upper.as_str(): {"name": "replacement record"}
            });
            assert!(
                serde_json::from_value::<ScenarioDocument>(input).is_err(),
                "aliased identities in {section} must not overwrite records"
            );
        }
    }

    #[test]
    fn document_owned_uuids_reject_cross_map_collisions() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut document: ScenarioDocument = serde_json::from_value(scenario_json())?;
        let identity = "018f47f2-e880-7000-8000-000000000002".parse()?;
        document.domain.rules.insert(
            crate::RuleId::from_uuid(identity),
            json!({"name": "conflicting rule"}),
        );
        assert_eq!(
            crate::validate_document_owned_uuid_uniqueness(&document),
            Err(crate::OwnedIdentityError {
                duplicate_uuid: identity
            }),
        );
        Ok(())
    }

    #[test]
    fn document_owned_uuids_collect_and_reject_nested_collisions()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document: ScenarioDocument = serde_json::from_value(scenario_json())?;
        let entity_id = "018f47f2-e880-7000-8000-000000000002".parse()?;
        let nested_id = "018f47f2-e880-7000-8000-000000000003".parse::<uuid::Uuid>()?;
        let definition = json!({
            (nested_id.to_string()): {"id": nested_id},
        });
        document.domain.entities.insert(
            crate::EntityId::from_uuid(entity_id),
            json!({"children": [definition.clone()]}),
        );
        crate::validate_document_owned_uuid_uniqueness(&document)?;
        assert_eq!(
            crate::collect_document_owned_uuids(&document),
            std::collections::BTreeSet::from([
                document.scenario_id.as_uuid(),
                entity_id,
                nested_id,
            ]),
        );
        document
            .extensions
            .insert("vendor.owned".to_owned(), json!({"children": [definition]}));
        assert_eq!(
            crate::validate_document_owned_uuid_uniqueness(&document),
            Err(crate::OwnedIdentityError {
                duplicate_uuid: nested_id
            }),
        );
        Ok(())
    }

    #[test]
    fn scenario_envelope_rejects_unknown_outer_fields() {
        let mut input = scenario_json();
        input["unexpected"] = json!(true);
        assert!(serde_json::from_value::<ScenarioDocument>(input).is_err());
    }

    #[test]
    fn command_catalog_uses_stable_adjacent_tags() -> Result<(), serde_json::Error> {
        let command: ScenarioCommand = serde_json::from_value(json!({
            "type": "removeEntity",
            "payload": {
                "entityId": "018f47f2-e880-7000-8000-000000000002"
            }
        }))?;
        assert_eq!(
            serde_json::to_value(command)?,
            json!({
                "type": "removeEntity",
                "payload": {
                    "entityId": "018f47f2-e880-7000-8000-000000000002"
                }
            })
        );
        Ok(())
    }

    #[test]
    fn solve_options_use_only_strict_millisecond_time_limit() -> Result<(), serde_json::Error> {
        let input = solve_options_json();
        let options: SolveOptions = serde_json::from_value(input.clone())?;
        assert_eq!(options.time_limit_milliseconds.value(), 750);
        let serialized = serde_json::to_value(options)?;
        assert_eq!(serialized, input);
        assert!(serialized.get("timeLimit").is_none());

        let mut old_only = solve_options_json();
        let Some(object) = old_only.as_object_mut() else {
            return Err(serde_json::Error::io(std::io::Error::other(
                "solve options fixture must be an object",
            )));
        };
        object.remove("timeLimitMilliseconds");
        object.insert("timeLimit".to_owned(), json!(5));
        assert!(serde_json::from_value::<SolveOptions>(old_only).is_err());

        let mut unknown_old_field = solve_options_json();
        unknown_old_field["timeLimit"] = json!(5);
        assert!(serde_json::from_value::<SolveOptions>(unknown_old_field).is_err());
        Ok(())
    }

    #[test]
    fn solve_options_reject_zero_optional_limits_and_exact_workers() -> Result<(), serde_json::Error>
    {
        let mut options: SolveOptions = serde_json::from_value(solve_options_json())?;
        options.memory_limit_bytes = Some(0);
        assert_eq!(options.validate(), Err(SolveOptionsError::ZeroMemoryLimit));

        options.memory_limit_bytes = None;
        options.worker_threads = super::WorkerThreadPolicy::Exact(0);
        assert_eq!(
            options.validate(),
            Err(SolveOptionsError::ZeroWorkerThreads)
        );

        options.worker_threads = super::WorkerThreadPolicy::Auto;
        options.solution_limit = Some(0);
        assert_eq!(
            options.validate(),
            Err(SolveOptionsError::ZeroSolutionLimit)
        );
        Ok(())
    }

    #[test]
    fn solve_option_tagged_enums_reject_unknown_fields() {
        let mut backend = solve_options_json();
        backend["backend"]["unexpected"] = json!(true);
        assert!(serde_json::from_value::<SolveOptions>(backend).is_err());

        let mut workers = solve_options_json();
        workers["workerThreads"] = json!({"kind": "exact", "count": 1, "unexpected": true});
        assert!(serde_json::from_value::<SolveOptions>(workers).is_err());
    }
    #[test]
    fn counterfactual_progress_event_has_strict_stable_json()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = json!({
            "type": "counterfactualProgress",
            "payload": {
                "context": {
                    "eventVersion": 1,
                    "timestamp": "2026-09-04T12:34:56Z",
                    "requestId": "018f47f2-e880-7000-8000-000000000010",
                    "scenarioId": "018f47f2-e880-7000-8000-000000000001",
                    "revision": 42,
                    "solveRunId": null
                },
                "jobId": "018f47f2-e880-7000-8000-000000000020",
                "phase": "finalizing"
            }
        });
        let event: EventPayload = serde_json::from_value(input.clone())?;

        let EventPayload::CounterfactualProgress {
            context,
            job_id,
            phase,
        } = &event
        else {
            return Err("counterfactual progress JSON decoded as the wrong event variant".into());
        };
        assert_eq!(context.event_version, 1);
        assert_eq!(context.timestamp.to_string(), "2026-09-04T12:34:56Z");
        assert_eq!(
            context
                .request_id
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("018f47f2-e880-7000-8000-000000000010")
        );
        assert_eq!(
            context
                .scenario_id
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("018f47f2-e880-7000-8000-000000000001")
        );
        assert_eq!(context.revision, Some(super::Revision::new(42)));
        assert_eq!(context.solve_run_id, None);
        assert_eq!(job_id.to_string(), "018f47f2-e880-7000-8000-000000000020");
        assert_eq!(*phase, CounterfactualProgressPhase::Finalizing);
        assert_eq!(serde_json::to_value(&event)?, input);

        let mut unknown_outer = input.clone();
        unknown_outer["unexpected"] = json!(true);
        assert!(serde_json::from_value::<EventPayload>(unknown_outer).is_err());

        let mut unknown_payload = input.clone();
        unknown_payload["payload"]["unexpected"] = json!(true);
        assert!(serde_json::from_value::<EventPayload>(unknown_payload).is_err());

        let mut unknown_context = input;
        unknown_context["payload"]["context"]["unexpected"] = json!(true);
        assert!(serde_json::from_value::<EventPayload>(unknown_context).is_err());
        Ok(())
    }
}
