//! `SQLite`-backed scenario persistence.
//!
//! All access to a store is serialized through one dedicated thread which owns
//! the sole `SQLite` connection. Callers never receive a connection or a
//! transaction and therefore cannot accidentally bypass revision checks.

use eutheto_domain_ir::{
    AcceptedResult, AcceptedResultRefV1, ComparisonContext, ComparisonRunManifests,
    CounterfactualConclusionV1, CounterfactualFailureKind, CounterfactualJobErrorV1,
    CounterfactualJobRecordV1, CounterfactualJobRequestV1, CounterfactualJobState,
    CounterfactualResultV1, DomainEvidenceId, NormalizedSolution, PortableAcceptedResultV2,
    RUN_REQUEST_SEMANTICS_SCHEMA_VERSION, RunInputV1, RunManifestV1, RunPhaseTimingsV1,
    RunRequestSemanticsV1, RunTerminalOutcomeV1, ScoreVector, VerificationReport,
    VerificationValue, compare_accepted_results, counterfactual_condition_satisfied,
};
use eutheto_export::ApplicationMetadata;
use eutheto_import::{
    AppliedMigration, ImportProvenance, MigrationRegistryKind, MigrationSubject,
    PackMigrationVersionSpace, PreviewBinding, RestoreAuthorization, RestoreMode,
    SafetyBackupEvidence, StagedBackupRestore, StagedDisposition, StagedImport,
};
use eutheto_types::{
    ActorRef, BackendId, BackendSelection, BundleId, CancellationToken, CommandId, CommandSource,
    CounterfactualJobId, IanaTimeZone, MAX_SCENARIO_DOCUMENT_BYTES, PackId, PortableAsset,
    PortableJsonLimits, ProjectMetadataDto, RequestId, Revision, Rfc3339Timestamp,
    SafeDiagnosticValue, ScenarioDocument, ScenarioId, ScenarioRevisionReference,
    ScenarioSnapshotId, ScenarioSnapshotV1, SemanticCapability, SolutionId, SolveOptions,
    SolveRunId, SolveStatus, SupplementalIdentity, SupplementalSectionKind,
    collect_scenario_owned_uuids, collect_self_declared_uuids, extract_result_dependency,
    extract_result_id, extract_scenario_references, validate_nonsecret_portable_json,
    validate_scenario_owned_uuid_uniqueness,
};
use rusqlite::{
    Connection, MAIN_DB, OpenFlags, OptionalExtension, TransactionBehavior, limits::Limit, params,
    types::ValueRef,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{File, OpenOptions as FileOpenOptions};
use std::io::{self, Read};
use std::num::NonZeroU32;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::thread;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

const CURRENT_SCHEMA_VERSION: u32 = 5;
const INITIAL_MIGRATION: &str = include_str!("../../../migrations/V1_initial_schema.sql");
const INDEPENDENT_VERIFICATION_MIGRATION: &str =
    include_str!("../../../migrations/V2_independent_verification.sql");
const COUNTERFACTUAL_JOB_REQUESTS_MIGRATION: &str =
    include_str!("../../../migrations/V3_counterfactual_job_requests.sql");
const SELECTED_SOLUTION_MIGRATION: &str =
    include_str!("../../../migrations/V4_selected_solution.sql");
const COUNTERFACTUAL_RUN_OWNERSHIP_MIGRATION: &str =
    include_str!("../../../migrations/V5_counterfactual_run_ownership.sql");
const SQLITE_LENGTH_LIMIT_BYTES: i32 = 128 * 1024 * 1024;
// Allows one SQLite busy-timeout interval for worker cleanup and one for the
// owning process to persist its terminal timeout/cancellation outcome.
const SOLVE_TERMINAL_PERSISTENCE_GRACE_MILLISECONDS: u64 = 10_000;
const V1_MIGRATION_NAME: &str = "V1_initial_schema.sql";
const V2_MIGRATION_NAME: &str = "V2_independent_verification.sql";
const V3_MIGRATION_NAME: &str = "V3_counterfactual_job_requests.sql";
const V4_MIGRATION_NAME: &str = "V4_selected_solution.sql";
const V5_MIGRATION_NAME: &str = "V5_counterfactual_run_ownership.sql";
const PRE_V2_BACKUP_SUFFIX: &str = ".pre-v2-backup.sqlite3";
const MAX_SNAPSHOT_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;
const IMPORT_PROVENANCE_RETENTION_POLICY_VERSION: u32 = 1;
const MAX_IMPORT_PROVENANCE_ROWS: u64 = 128;
const MAX_IMPORT_PROVENANCE_BYTES: u64 = 4 * 1024 * 1024;
const LIBRARY_REVISION_KEY: &str = "portable_library_revision";
const MAX_SAFETY_BACKUP_FAILURE_RECEIPTS: u64 = 16;
const MAX_RECEIPT_PROOF_BYTES: usize = 1_024;
const MAX_RECEIPT_BINDING_BYTES: usize = 4 * 1_024;
const MAX_RECEIPT_SAFE_REASON_BYTES: usize = 4 * 1_024;
/// Maximum number of globally queued or running counterfactual diagnostics.
pub const MAX_ACTIVE_COUNTERFACTUAL_JOBS: u32 = 4;

/// Errors emitted by the persistence boundary. Database diagnostics are kept
/// as a source for structured logs; callers should map these to user-safe text.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("the database actor is unavailable")]
    ActorUnavailable,
    #[error("failed to start the database actor: {0}")]
    ActorStart(String),
    #[error("the operation was cancelled before commit")]
    OperationCancelled,
    #[error("database operation failed")]
    Database(#[source] rusqlite::Error),
    #[error("stored JSON is invalid")]
    Json(#[source] serde_json::Error),
    #[error("scenario {0} was not found")]
    ScenarioNotFound(ScenarioId),
    #[error("revision conflict")]
    Conflict {
        expected: Revision,
        actual: Revision,
    },
    #[error(
        "library revision conflict: expected {}, actual {}",
        .expected.value(),
        .actual.value()
    )]
    LibraryConflict {
        expected: Revision,
        actual: Revision,
    },
    #[error("scenario {0} already exists")]
    ScenarioAlreadyExists(ScenarioId),
    #[error("command application failed ({code}): {message}")]
    CommandApplication { code: String, message: String },
    #[error("staged library apply is invalid: {0}")]
    InvalidStagedApply(String),
    #[error("scenario identity graph is invalid: {0}")]
    InvalidScenarioIdentity(String),
    #[error("portable identity is already owned: {0}")]
    IdentityCollision(Uuid),
    #[error("safety-backup failure receipt is invalid: {0}")]
    InvalidSafetyBackupFailureReceipt(String),
    #[error("safety-backup failure receipt is missing or does not match the staged restore")]
    SafetyBackupFailureReceiptRejected,
    #[error("project duplicate identity mapping is invalid: {0}")]
    InvalidDuplicateMapping(String),
    #[error("the database schema version {found} is newer than supported version {supported}")]
    NewerSchema { found: u32, supported: u32 },
    #[error("released migration {version} does not match its embedded checksum")]
    MigrationChanged { version: u32 },
    #[error("database integrity check failed: {0}")]
    Integrity(String),
    #[error("snapshot policy is outside the supported bounds")]
    InvalidSnapshotPolicy,
    #[error("scenario document exceeds the snapshot input limit")]
    SnapshotTooLarge,
    #[error("scenario document exceeds the authoritative size limit")]
    ScenarioDocumentTooLarge,
    #[error("snapshot compression failed")]
    Compression(#[source] io::Error),
    #[error("undo is unavailable")]
    NoUndo,
    #[error("redo is unavailable")]
    NoRedo,
    #[error("the selected command is not reversible")]
    CommandNotReversible,
    #[error("a new command would discard redo history; explicit truncation is required")]
    RedoBranchRequiresTruncation,
    #[error("the backup destination already exists")]
    BackupDestinationExists,
    #[error("numeric database value is outside the supported range")]
    NumericRange,
    #[error("private storage path validation failed")]
    PrivatePath(#[source] io::Error),
    #[error("solve request {request_id} conflicts with an existing request")]
    SolveRequestIdConflict { request_id: RequestId },
    #[error("solve run {0} collides with an existing run")]
    SolveRunCollision(SolveRunId),
    #[error("solve run {0} was not found")]
    SolveRunNotFound(SolveRunId),
    #[error("solve run {0} is already terminal")]
    SolveRunTerminalConflict(SolveRunId),
    #[error("persisted solve run is invalid: {0}")]
    InvalidPersistedRun(String),
    #[error("persisted accepted result is invalid: {0}")]
    InvalidPersistedResult(String),
    #[error("accepted result {0} was not found")]
    AcceptedResultNotFound(SolutionId),
    #[error("persisted candidate diagnostics are invalid: {0}")]
    InvalidPersistedDiagnostic(String),
    #[error("scenario snapshot {0} does not match its solve input")]
    SnapshotMismatch(ScenarioSnapshotId),
    #[error("counterfactual request {request_id} conflicts with an existing request")]
    CounterfactualRequestIdConflict { request_id: RequestId },
    #[error("counterfactual job {0} collides with an existing job")]
    CounterfactualJobCollision(CounterfactualJobId),
    #[error("counterfactual job {0} was not found")]
    CounterfactualJobNotFound(CounterfactualJobId),
    #[error("counterfactual cancellation request {request_id} conflicts with persisted state")]
    CounterfactualCancelRequestIdConflict { request_id: RequestId },
    #[error("counterfactual job {0} cannot make the requested transition")]
    CounterfactualTransitionConflict(CounterfactualJobId),
    #[error("persisted counterfactual job is invalid: {0}")]
    InvalidPersistedCounterfactual(String),
    #[error("counterfactual diagnostic capacity is full (maximum {maximum})")]
    CounterfactualCapacityExceeded { maximum: u32 },
    #[cfg(debug_assertions)]
    #[error("injected persistence failure")]
    InjectedFailure,
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Bounded periodic snapshot configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotPolicy {
    interval: NonZeroU32,
    max_document_bytes: u64,
    compression_level: i32,
}

impl SnapshotPolicy {
    /// Creates a policy. Intervals are limited to 1..=10,000 commands,
    /// documents to 16 MiB..=64 MiB, and zstd levels to 1..=19. The lower
    /// document bound guarantees every legal authoritative document can be
    /// snapshotted when its interval is reached.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidSnapshotPolicy`] when any bound is violated.
    pub fn new(
        interval: NonZeroU32,
        max_document_bytes: u64,
        compression_level: i32,
    ) -> Result<Self, StoreError> {
        if interval.get() > 10_000
            || !(MAX_SCENARIO_DOCUMENT_BYTES..=MAX_SNAPSHOT_DOCUMENT_BYTES)
                .contains(&max_document_bytes)
            || !(1..=19).contains(&compression_level)
        {
            return Err(StoreError::InvalidSnapshotPolicy);
        }
        Ok(Self {
            interval,
            max_document_bytes,
            compression_level,
        })
    }

    /// Number of committed commands between snapshots.
    #[must_use]
    pub const fn interval(self) -> NonZeroU32 {
        self.interval
    }

    /// Maximum uncompressed scenario document accepted by the compressor.
    #[must_use]
    pub const fn max_document_bytes(self) -> u64 {
        self.max_document_bytes
    }
}

impl Default for SnapshotPolicy {
    fn default() -> Self {
        Self {
            interval: NonZeroU32::MIN.saturating_add(49),
            max_document_bytes: MAX_SCENARIO_DOCUMENT_BYTES,
            compression_level: 3,
        }
    }
}

/// Configuration applied before the actor starts accepting work.
#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    pub snapshot_policy: SnapshotPolicy,
    #[cfg(debug_assertions)]
    pub failpoint: Option<Failpoint>,
    #[cfg(debug_assertions)]
    v2_migration_begin_test_hook: Option<V2MigrationBeginTestHook>,
    #[cfg(debug_assertions)]
    v3_migration_begin_test_hook: Option<V3MigrationBeginTestHook>,
    #[cfg(debug_assertions)]
    v4_migration_begin_test_hook: Option<V4MigrationBeginTestHook>,
    #[cfg(debug_assertions)]
    command_commit_test_hook: Option<CommandCommitTestHook>,
    #[cfg(debug_assertions)]
    recovery_now: Option<jiff::Timestamp>,
}

/// One-shot test failure locations. This API is absent from optimized builds.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failpoint {
    AfterMigrationSql,
    AfterV2MigrationSql,
    AfterV3MigrationSql,
    AfterV4MigrationSql,
    AfterDocumentWrite,
    AfterSupplementalWrite,
    AfterSolveRunInsert,
    AfterAcceptedSolutionInsert,
    AfterQuarantineWrite,
    AfterCounterfactualJobInsert,
    AfterCounterfactualTransition,
    AfterCounterfactualCancelWrite,
    AfterSettingsWrite,
}

/// The command commit boundary at which a debug-only hook pauses once.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandCommitTestPhase {
    BeforeFinalCancellationCheck,
    AfterFinalCancellationCheck,
}

/// One-shot, per-store synchronization around the final command cancellation check.
#[cfg(debug_assertions)]
#[derive(Clone, Debug)]
pub struct CommandCommitTestHook {
    phase: CommandCommitTestPhase,
    armed: Arc<std::sync::atomic::AtomicBool>,
    reached: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
}

#[cfg(debug_assertions)]
impl CommandCommitTestHook {
    /// Pauses the first command reaching the selected phase, after all writes.
    #[must_use]
    pub fn new(phase: CommandCommitTestPhase) -> Self {
        Self {
            phase,
            armed: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            reached: Arc::new(std::sync::Barrier::new(2)),
            release: Arc::new(std::sync::Barrier::new(2)),
        }
    }

    /// Blocks the test thread until the actor reaches the selected phase.
    pub fn wait_until_reached(&self) {
        self.reached.wait();
    }

    /// Releases the paused actor.
    pub fn release(&self) {
        self.release.wait();
    }

    fn actor_pause(&self, phase: CommandCommitTestPhase) {
        if self.phase == phase && self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.reached.wait();
            self.release.wait();
        }
    }
}

/// Debug-only synchronization immediately before the V2 writer transaction.
#[cfg(debug_assertions)]
#[derive(Clone, Debug)]
pub struct V2MigrationBeginTestHook {
    reached: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
}

#[cfg(debug_assertions)]
impl V2MigrationBeginTestHook {
    /// Creates a hook shared by one test thread and the migration actor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            reached: Arc::new(std::sync::Barrier::new(2)),
            release: Arc::new(std::sync::Barrier::new(2)),
        }
    }

    /// Blocks until the migration actor is immediately before V2 `BEGIN`.
    pub fn wait_before_begin(&self) {
        self.reached.wait();
    }

    /// Releases the migration actor to acquire its writer lock.
    pub fn release(&self) {
        self.release.wait();
    }

    fn actor_wait_before_begin(&self) {
        self.reached.wait();
        self.release.wait();
    }
}

#[cfg(debug_assertions)]
impl Default for V2MigrationBeginTestHook {
    fn default() -> Self {
        Self::new()
    }
}

/// Debug-only synchronization immediately before the V3 writer transaction.
#[cfg(debug_assertions)]
#[derive(Clone, Debug)]
pub struct V3MigrationBeginTestHook {
    reached: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
}

#[cfg(debug_assertions)]
impl V3MigrationBeginTestHook {
    /// Creates a hook shared by one test thread and the migration actor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            reached: Arc::new(std::sync::Barrier::new(2)),
            release: Arc::new(std::sync::Barrier::new(2)),
        }
    }

    /// Blocks until the migration actor is immediately before V3 `BEGIN`.
    pub fn wait_before_begin(&self) {
        self.reached.wait();
    }

    /// Releases the migration actor to acquire its writer lock.
    pub fn release(&self) {
        self.release.wait();
    }

    fn actor_wait_before_begin(&self) {
        self.reached.wait();
        self.release.wait();
    }
}

#[cfg(debug_assertions)]
impl Default for V3MigrationBeginTestHook {
    fn default() -> Self {
        Self::new()
    }
}

/// Debug-only synchronization immediately before the V4 writer transaction.
#[cfg(debug_assertions)]
#[derive(Clone, Debug)]
pub struct V4MigrationBeginTestHook {
    reached: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
}

#[cfg(debug_assertions)]
impl V4MigrationBeginTestHook {
    /// Creates a hook shared by one test thread and the migration actor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            reached: Arc::new(std::sync::Barrier::new(2)),
            release: Arc::new(std::sync::Barrier::new(2)),
        }
    }

    /// Blocks until the migration actor is immediately before V4 `BEGIN`.
    pub fn wait_before_begin(&self) {
        self.reached.wait();
    }

    /// Releases the migration actor to acquire its writer lock.
    pub fn release(&self) {
        self.release.wait();
    }

    fn actor_wait_before_begin(&self) {
        self.reached.wait();
        self.release.wait();
    }
}

#[cfg(debug_assertions)]
impl Default for V4MigrationBeginTestHook {
    fn default() -> Self {
        Self::new()
    }
}
impl OpenOptions {
    /// Uses a validated snapshot policy with no injected failure.
    #[must_use]
    pub const fn new(snapshot_policy: SnapshotPolicy) -> Self {
        Self {
            snapshot_policy,
            #[cfg(debug_assertions)]
            failpoint: None,
            #[cfg(debug_assertions)]
            v2_migration_begin_test_hook: None,
            #[cfg(debug_assertions)]
            v3_migration_begin_test_hook: None,
            #[cfg(debug_assertions)]
            v4_migration_begin_test_hook: None,
            #[cfg(debug_assertions)]
            command_commit_test_hook: None,
            #[cfg(debug_assertions)]
            recovery_now: None,
        }
    }

    /// Arms an initialization failpoint in debug/test builds.
    #[cfg(debug_assertions)]
    #[must_use]
    pub const fn with_failpoint(mut self, failpoint: Failpoint) -> Self {
        self.failpoint = Some(failpoint);
        self
    }

    /// Overrides the startup recovery wall clock in debug/test builds.
    #[cfg(debug_assertions)]
    #[must_use]
    pub const fn with_recovery_now(mut self, now: jiff::Timestamp) -> Self {
        self.recovery_now = Some(now);
        self
    }

    /// Pauses an existing-V1 migration immediately before its V2 writer transaction.
    #[cfg(debug_assertions)]
    #[must_use]
    pub fn with_v2_migration_begin_test_hook(mut self, hook: V2MigrationBeginTestHook) -> Self {
        self.v2_migration_begin_test_hook = Some(hook);
        self
    }

    /// Pauses an existing-V2 migration immediately before its V3 writer transaction.
    #[cfg(debug_assertions)]
    #[must_use]
    pub fn with_v3_migration_begin_test_hook(mut self, hook: V3MigrationBeginTestHook) -> Self {
        self.v3_migration_begin_test_hook = Some(hook);
        self
    }

    /// Pauses an existing-V3 migration immediately before its V4 writer transaction.
    #[cfg(debug_assertions)]
    #[must_use]
    pub fn with_v4_migration_begin_test_hook(mut self, hook: V4MigrationBeginTestHook) -> Self {
        self.v4_migration_begin_test_hook = Some(hook);
        self
    }

    /// Pauses one command at its final cancellation/commit boundary in debug builds.
    #[cfg(debug_assertions)]
    #[must_use]
    pub fn with_command_commit_test_hook(mut self, hook: CommandCommitTestHook) -> Self {
        self.command_commit_test_hook = Some(hook);
        self
    }
}

/// Result of startup recovery work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializationOutcome {
    pub schema_version: u32,
    pub applied_migrations: Vec<u32>,
    pub integrity: IntegrityOutcome,
    pub retained_backup_path: Option<PathBuf>,
    pub recovery: RecoveryOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrityOutcome {
    Ok,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryOutcome {
    pub interrupted_solve_run_ids: Vec<SolveRunId>,
    pub recovered_counterfactual_job_ids: Vec<CounterfactualJobId>,
}

/// Authoritative document used to create one persisted scenario.
#[derive(Clone, Debug)]
pub struct NewProject {
    pub document: ScenarioDocument,
}

/// A typed persisted project row, intentionally narrower than the authoritative document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: ScenarioId,
    pub domain_pack_id: PackId,
    pub domain_schema_version: u32,
    pub title: String,
    pub description: Option<String>,
    pub revision: Revision,
    pub created_at: Rfc3339Timestamp,
    pub updated_at: Rfc3339Timestamp,
    pub last_opened_at: Option<Rfc3339Timestamp>,
    pub archived_at: Option<Rfc3339Timestamp>,
}

impl From<&ProjectSummary> for ProjectMetadataDto {
    fn from(summary: &ProjectSummary) -> Self {
        Self {
            scenario_id: summary.id,
            title: summary.title.clone(),
            description: summary.description.clone().unwrap_or_default(),
            domain_pack: eutheto_types::DomainPackRef {
                id: summary.domain_pack_id.clone(),
                schema_version: summary.domain_schema_version,
            },
            revision: summary.revision,
            created_at: summary.created_at,
            updated_at: summary.updated_at,
            archived_at: summary.archived_at,
        }
    }
}
/// Portable envelope values that are not part of the authoritative document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortableWrapperMetadata {
    pub required_capabilities: BTreeSet<SemanticCapability>,
    pub semantic_extensions: BTreeMap<String, Value>,
    pub extensions: BTreeMap<String, Value>,
}

/// Complete stored scenario view.
#[derive(Clone, Debug)]
pub struct StoredProject {
    pub summary: ProjectSummary,
    pub document: ScenarioDocument,
    pub portable: PortableWrapperMetadata,
}

/// Exact immutable scenario snapshot and complete independently accepted result.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredAcceptedResultV2 {
    pub document: ScenarioDocument,
    pub portable: PortableAcceptedResultV2,
}

/// Compact validated projection of one independently accepted result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAcceptedResultSummary {
    pub result: AcceptedResultRefV1,
    pub run_id: SolveRunId,
    pub scenario_id: ScenarioId,
    pub scenario_revision: Revision,
    pub current_revision: Revision,
    pub stale: bool,
    pub selected: bool,
    pub status: SolveStatus,
    pub score: ScoreVector,
    pub verification_checksum: String,
    pub finished_at: Rfc3339Timestamp,
}

/// Exact immutable portable scenario revision required by a retained result.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredScenarioRevision {
    pub scenario: ScenarioSnapshotV1,
}

/// Supplemental portable JSON bytes and inert assets with their exact declarations.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PortableSupplementalSections {
    pub results: BTreeMap<String, Vec<u8>>,
    pub shared_records: BTreeMap<String, Vec<u8>>,
    pub preferences: BTreeMap<String, Vec<u8>>,
    pub assets: BTreeMap<String, PortableAsset>,
}

/// Immutable caller-supplied solve request. Scenario-owned fields are derived
/// from the exact authoritative document inside the start transaction.
#[derive(Clone, Debug)]
pub struct NewSolveRunV1 {
    /// Stable identity assigned to this execution attempt.
    pub run_id: SolveRunId,
    /// Idempotency identity assigned to the semantic request.
    pub request_id: RequestId,
    /// Scenario to solve.
    pub scenario_id: ScenarioId,
    /// Exact scenario revision the caller intends to solve.
    pub expected_revision: Revision,
    /// Planning IR schema version.
    pub planning_ir_schema_version: u32,
    /// Domain compiler version.
    pub compiler_version: String,
    /// Application version.
    pub application_version: String,
    /// Selected backend identity.
    pub backend_id: BackendId,
    /// Selected backend version.
    pub backend_version: String,
    /// Backend adapter version.
    pub adapter_version: String,
    /// Isolated worker version.
    pub worker_version: String,
    /// Solver engine version.
    pub solver_version: String,
    /// Worker protocol major version.
    pub protocol_major: u32,
    /// Worker protocol minor version.
    pub protocol_minor: u32,
    /// Canonical planning model hash.
    pub model_hash: String,
    /// Canonical objective-policy hash.
    pub objective_policy_hash: String,
    /// Exact canonical solver options.
    pub solve_options: SolveOptions,
    /// Optional canonical temporary-condition-set hash.
    pub temporary_condition_hash: Option<String>,
    /// Parent-recorded execution start time.
    pub started_at: Rfc3339Timestamp,
}

/// Result of atomically creating or reusing an idempotent solve request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartedSolveRunV1 {
    /// Immutable typed input persisted for the run.
    pub input: RunInputV1,
    /// Original parent-recorded start time used for deadlines and manifests.
    pub started_at: Rfc3339Timestamp,
    /// Whether an existing request was reused.
    pub reused: bool,
}

/// Validated retained request state; reading it never starts or takes ownership of work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExistingSolveRunV1 {
    pub input: RunInputV1,
    pub started_at: Rfc3339Timestamp,
    /// Absent only for a still-running request.
    pub manifest: Option<RunManifestV1>,
}

/// Exact immutable scenario snapshot and typed input for backend execution.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedSolveInputV1 {
    /// Immutable typed input persisted for the run.
    pub input: RunInputV1,
    /// Exact scenario document referenced by the input snapshot.
    pub document: ScenarioDocument,
}

/// Result of atomically creating or reusing a counterfactual request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartedCounterfactualJobV1 {
    /// Exact persisted lifecycle record.
    pub record: CounterfactualJobRecordV1,
    /// Whether an existing semantically identical request was reused.
    pub reused: bool,
}

/// One legal requested counterfactual lifecycle transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CounterfactualJobTransitionV1 {
    /// Mark queued work as started.
    Running { started_at: Rfc3339Timestamp },
    /// Persist a locally authorized counterfactual result.
    Completed { result: Box<CounterfactualResultV1> },
    /// Persist a safe typed failure.
    Failed {
        finished_at: Rfc3339Timestamp,
        error: CounterfactualJobErrorV1,
    },
    /// Finish cleanup after a durable cancellation request.
    Cancelled { finished_at: Rfc3339Timestamp },
    /// Record that started work ended without a result.
    Interrupted { finished_at: Rfc3339Timestamp },
}
/// One atomic terminal disposition for a counterfactual-owned derived run and job.
#[derive(Clone, Debug, PartialEq)]
pub enum CounterfactualRunFinalizationV1 {
    /// Persist an independently accepted alternative and complete the job.
    CompletedAccepted {
        accepted_result: Box<AcceptedResult>,
        evidence: BTreeMap<DomainEvidenceId, VerificationValue>,
        manifest: RunManifestV1,
        result: Box<CounterfactualResultV1>,
    },
    /// Persist an infeasibility proof or post-run limit outcome and complete the job.
    CompletedNoResult {
        manifest: RunManifestV1,
        result: Box<CounterfactualResultV1>,
    },
    /// Quarantine an invalid candidate and fail the job with a typed error.
    Quarantined {
        manifest: RunManifestV1,
        candidate_diagnostics: CandidateDiagnosticsV1,
        error: CounterfactualJobErrorV1,
    },
    /// Persist a terminal no-result run and fail the job with a typed error.
    Failed {
        manifest: RunManifestV1,
        error: CounterfactualJobErrorV1,
    },
    /// Persist a cancelled derived run and cancel the job.
    Cancelled { manifest: RunManifestV1 },
    /// Persist an interrupted derived run and interrupt the job.
    Interrupted { manifest: RunManifestV1 },
}

/// Result of linearizing a cancellation request against terminal completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CounterfactualCancelOutcomeV1 {
    /// Cancellation was durably requested or the same request was reused.
    Requested {
        record: CounterfactualJobRecordV1,
        reused: bool,
    },
    /// A non-cancel terminal state won before this request and was not mutated.
    AlreadyTerminal { record: CounterfactualJobRecordV1 },
}

/// Redacted, nonsecret candidate diagnostics retained for a quarantined run.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateDiagnosticsV1 {
    /// Stable diagnostic identities mapped to bounded safe values.
    pub values: BTreeMap<String, SafeDiagnosticValue>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PersistedMigrationRegistry {
    Outer,
    Portable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedAppliedMigration {
    pub registry: PersistedMigrationRegistry,
    pub name: String,
    pub from_version: u32,
    pub to_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_space: Option<PackMigrationVersionSpace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<MigrationSubject>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PersistedDisposition {
    Create,
    CreateCopy,
    Replace,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedPreviewBinding {
    pub file_sha256: String,
    pub options_sha256: String,
    pub local_library_revision: Revision,
    pub format_version: u32,
    pub schema_version: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioImportSource {
    pub scenario_id: ScenarioId,
    pub original_scenario_id: ScenarioId,
    pub source_revision: Revision,
    pub disposition: PersistedDisposition,
    pub id_remap: BTreeMap<Uuid, Uuid>,
}

/// Provenance of one committed portable apply, including the source revision
/// even when the local replacement revision was advanced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedImportProvenance {
    pub source_bundle_id: BundleId,
    pub source_application: ApplicationMetadata,
    pub original_format_version: u32,
    pub original_schema_version: u32,
    pub source_file_sha256: String,
    pub applied_migrations: Vec<PersistedAppliedMigration>,
    pub scenario_sources: Vec<ScenarioImportSource>,
    pub binding: PersistedPreviewBinding,
    pub source_created_at: Rfc3339Timestamp,
    pub applied_at: Rfc3339Timestamp,
}

/// Transactionally consistent view used by export and import preview.
#[derive(Clone, Debug)]
pub struct LibrarySnapshot {
    pub revision: Revision,
    pub projects: Vec<StoredProject>,
    pub settings: BTreeMap<String, AppSetting<Value>>,
    pub scenario_revisions: Vec<StoredScenarioRevision>,
    pub scenario_revision_high_water: BTreeMap<ScenarioId, Revision>,
    pub scenario_identity_owners: BTreeMap<Uuid, ScenarioId>,
    pub manifest_extensions: BTreeMap<String, Value>,
    pub nonsemantic_extensions: BTreeSet<String>,
    pub sections: PortableSupplementalSections,
    pub provenance: Vec<PersistedImportProvenance>,
    pub supplemental_identities: BTreeSet<SupplementalIdentity>,
    pub supplemental_identity_owners: BTreeMap<Uuid, SupplementalIdentity>,
}

/// Minimal transactionally consistent library identity used by support flows
/// that must not deserialize user-authored content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LibraryMetadataSnapshot {
    pub revision: Revision,
    pub scenario_count: u64,
}

/// One owner-private authorization receipt recorded after a real safety-backup failure.
#[derive(Clone, Debug)]
pub struct SafetyBackupFailureReceipt {
    pub proof: String,
    pub binding: PreviewBinding,
    pub collision_plan_sha256: String,
    pub safe_reason: String,
    pub created_at: Rfc3339Timestamp,
}
/// Fully staged portable mutation accepted by one store transaction.
#[derive(Clone, Debug)]
pub enum StagedLibraryApply {
    Import(StagedImport),
    BackupRestore {
        restore: StagedBackupRestore,
        settings: BTreeMap<String, AppSetting<Value>>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LibraryApplyOutcome {
    pub library_revision: Revision,
    pub created: usize,
    pub replaced: usize,
    pub removed: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectListScope {
    Active,
    Archived,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RedoBranchPolicy {
    Reject,
    Truncate,
}

/// Typed audit material supplied by the pure command layer.
#[derive(Clone, Debug)]
pub struct JournalWrite {
    pub command_type: String,
    pub command: Value,
    pub command_id: CommandId,
    pub inverse: Option<Value>,
    pub actor: ActorRef,
    pub source: CommandSource,
    pub summary: String,
    pub created_at: Rfc3339Timestamp,
}

/// The value produced by a pure command callback and committed atomically.
#[derive(Clone, Debug)]
pub struct CommandWrite<T> {
    pub document: ScenarioDocument,
    pub journal: JournalWrite,
    pub output: T,
}

/// Result returned after the transaction commits.
#[derive(Clone, Debug)]
pub struct CommittedCommand<T> {
    pub new_revision: Revision,
    pub output: T,
}

/// A persisted history row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEntry {
    pub id: CommandId,
    pub revision_before: Revision,
    pub revision_after: Revision,
    pub command_type: String,
    pub command: Value,
    pub inverse: Option<Value>,
    pub actor: ActorRef,
    pub source: CommandSource,
    pub summary: String,
    pub created_at: Rfc3339Timestamp,
    pub history_sequence: u64,
    pub branch_generation: u64,
    pub applied: bool,
}

/// Maximum number of entries returned by a metadata history page.
pub const HISTORY_PAGE_MAX_ENTRIES: u32 = 100;
/// Maximum UTF-8 bytes returned for one recorded history summary.
pub const HISTORY_SUMMARY_MAX_BYTES: usize = 4 * 1024;

/// Bounded journal metadata, without command, inverse, or actor payloads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEntrySummary {
    pub id: CommandId,
    pub revision_before: Revision,
    pub revision_after: Revision,
    pub source: CommandSource,
    /// `None` means the recorded summary exceeds the display byte limit.
    pub summary: Option<String>,
    pub created_at: Rfc3339Timestamp,
    pub history_sequence: u64,
    pub branch_generation: u64,
    pub applied: bool,
}

/// One revision-consistent metadata page and current journal availability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryPage {
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub entries: Vec<HistoryEntrySummary>,
    pub next_before_sequence: Option<u64>,
    pub undo_available: bool,
    pub redo_available: bool,
}
/// Live connection settings and installed schema objects, used by startup
/// diagnostics without exposing `SQLite` access to callers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreDiagnostics {
    pub foreign_keys: bool,
    pub journal_mode: String,
    pub synchronous: u32,
    pub busy_timeout_ms: u32,
    pub trusted_schema: bool,
    pub sqlite_length_limit_bytes: i32,
    pub schema_version: u32,
    pub tables: Vec<String>,
    pub indexes: Vec<String>,
}

/// Input supplied to undo and redo callbacks.
#[derive(Clone, Debug)]
pub struct HistoryCommand {
    pub document: ScenarioDocument,
    pub command: Value,
    pub entry: HistoryEntry,
    pub target_document_updated_at: Rfc3339Timestamp,
}

/// App setting with its last update timestamp.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppSetting<T> {
    pub value: T,
    pub updated_at: Rfc3339Timestamp,
}

/// A consistent snapshot of the caller's trusted application-setting scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppSettingsSnapshot {
    pub library_revision: Revision,
    pub settings: BTreeMap<String, AppSetting<Value>>,
}

/// The outcome of an atomic application-settings replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsCommit {
    pub library_revision: Revision,
    pub changed: bool,
    pub settings: BTreeMap<String, AppSetting<Value>>,
}

type ActorOperation = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

struct StoreActor {
    sender: Option<mpsc::UnboundedSender<ActorOperation>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for StoreActor {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(handle) = self.thread.take()
            && handle.thread().id() != thread::current().id()
        {
            let _ignored = handle.join();
        }
    }
}

/// Async handle to a dedicated-thread `SQLite` actor. The handle is cheap to
/// clone and is suitable for sharing through [`Arc`].
#[derive(Clone)]
pub struct SqliteScenarioStore {
    actor: Arc<StoreActor>,
    snapshot_policy: SnapshotPolicy,
    #[cfg(debug_assertions)]
    failpoint: Arc<std::sync::Mutex<Option<Failpoint>>>,
    #[cfg(debug_assertions)]
    command_commit_test_hook: Option<CommandCommitTestHook>,
}

impl fmt::Debug for SqliteScenarioStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteScenarioStore")
            .field("snapshot_policy", &self.snapshot_policy)
            .finish_non_exhaustive()
    }
}

impl SqliteScenarioStore {
    /// Opens or creates a database and returns both the actor and startup
    /// recovery outcome.
    ///
    /// Application wiring must pass only the fixed owner-private database from
    /// `AppPaths`; this is authoritative application state, not an import
    /// surface. Foreign database bytes must be decoded through the inert
    /// portable import boundary instead of being opened here.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor thread cannot start or report its
    /// initialization result, if the database cannot be opened or initialized,
    /// or if its schema, migration checksum, or integrity check is invalid. In
    /// debug builds, an armed initialization failpoint also returns an error.
    pub async fn open(path: impl AsRef<Path>) -> Result<(Self, InitializationOutcome), StoreError> {
        Self::open_with_options(path, OpenOptions::default()).await
    }

    /// Opens or creates a database with explicit actor and snapshot options.
    ///
    /// Application wiring must select only the trusted owner-private `AppPaths`
    /// database. Foreign database files must enter through inert portable
    /// import and are never accepted as an authoritative store.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid snapshot policy, actor startup or
    /// availability failure, database open or initialization failure, an
    /// unsupported or invalid schema, a changed migration, or failed integrity
    /// checks. In debug builds, an armed initialization failpoint also returns
    /// an error.
    pub async fn open_with_options(
        path: impl AsRef<Path>,
        options: OpenOptions,
    ) -> Result<(Self, InitializationOutcome), StoreError> {
        // Re-run validation even though fields are private so a future policy
        // expansion cannot accidentally bypass the bounds.
        let policy = SnapshotPolicy::new(
            options.snapshot_policy.interval,
            options.snapshot_policy.max_document_bytes,
            options.snapshot_policy.compression_level,
        )?;
        let path = path.as_ref().to_path_buf();
        let (sender, mut receiver) = mpsc::unbounded_channel::<ActorOperation>();
        let (initialization_sender, initialization_receiver) = oneshot::channel();
        #[cfg(debug_assertions)]
        let failpoint = Arc::new(std::sync::Mutex::new(options.failpoint));
        #[cfg(debug_assertions)]
        let actor_failpoint = Arc::clone(&failpoint);
        #[cfg(debug_assertions)]
        let command_commit_test_hook = options.command_commit_test_hook.clone();

        let actor_thread = thread::Builder::new()
            .name("eutheto-sqlite-store".to_owned())
            .spawn(move || {
                let result = prepare_private_database_path(&path).and_then(|guard| {
                    preflight_schema_version(&guard.file)
                        .and_then(|()| open_connection(&path))
                        .and_then(|mut connection| {
                            verify_private_database_path(&path, &guard)?;
                            initialize_connection(
                                &mut connection,
                                &path,
                                &guard.file,
                                #[cfg(debug_assertions)]
                                &options,
                                #[cfg(debug_assertions)]
                                &actor_failpoint,
                            )
                            .map(|outcome| (connection, outcome))
                        })
                });
                match result {
                    Ok((mut connection, outcome)) => {
                        if initialization_sender.send(Ok(outcome)).is_err() {
                            return;
                        }
                        while let Some(operation) = receiver.blocking_recv() {
                            operation(&mut connection);
                        }
                    }
                    Err(error) => {
                        let _ignored = initialization_sender.send(Err(error));
                    }
                }
            })
            .map_err(|error| StoreError::ActorStart(error.to_string()))?;

        let outcome = initialization_receiver
            .await
            .map_err(|_| StoreError::ActorUnavailable)??;
        Ok((
            Self {
                actor: Arc::new(StoreActor {
                    sender: Some(sender),
                    thread: Some(actor_thread),
                }),
                snapshot_policy: policy,
                #[cfg(debug_assertions)]
                failpoint,
                #[cfg(debug_assertions)]
                command_commit_test_hook,
            },
            outcome,
        ))
    }

    async fn call<T, F>(&self, operation: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let (result_sender, result_receiver) = oneshot::channel();
        self.actor
            .sender
            .as_ref()
            .ok_or(StoreError::ActorUnavailable)?
            .send(Box::new(move |connection| {
                let result = operation(connection);
                let _ignored = result_sender.send(result);
            }))
            .map_err(|_| StoreError::ActorUnavailable)?;
        result_receiver
            .await
            .map_err(|_| StoreError::ActorUnavailable)?
    }

    /// Creates a project at revision zero.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario already exists, the document cannot be
    /// serialized, a numeric value is out of range, the storage transaction
    /// fails, or the database actor is unavailable.
    pub async fn create_project(&self, project: NewProject) -> Result<StoredProject, StoreError> {
        self.call(move |connection| {
            let scenario_id = project.document.scenario_id;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            ensure_scenario_id_unseen(&transaction, scenario_id)?;
            insert_project(
                &transaction,
                &project.document,
                Revision::INITIAL,
                None,
                &PortableWrapperMetadata::default(),
            )?;
            validate_global_identity_ownership(&transaction)?;
            increment_library_revision(&transaction)?;
            transaction.commit()?;
            load_project(connection, scenario_id)
        })
        .await
    }

    /// Lists project summaries in the requested archive scope.
    ///
    /// # Errors
    ///
    /// Returns an error if the database actor or query fails, or if persisted
    /// identifiers, timestamps, or numeric values are invalid.
    pub async fn list_projects(
        &self,
        scope: ProjectListScope,
    ) -> Result<Vec<ProjectSummary>, StoreError> {
        self.call(move |connection| {
            let predicate = match scope {
                ProjectListScope::Active => "archived_at IS NULL",
                ProjectListScope::Archived => "archived_at IS NOT NULL",
                ProjectListScope::All => "1 = 1",
            };
            let sql = format!("SELECT id, domain_pack_id, domain_schema_version, title, description, revision, created_at, updated_at, last_opened_at, archived_at FROM scenarios WHERE {predicate} ORDER BY updated_at DESC, id");
            let mut statement = connection.prepare(&sql)?;
            let mut rows = statement.query([])?;
            let mut summaries = Vec::new();
            while let Some(row) = rows.next()? {
                summaries.push(read_summary_row(row)?);
            }
            Ok(summaries)
        })
        .await
    }

    /// Reads only the portable-library revision and scenario count from one
    /// transaction, without loading user-authored documents or settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor or snapshot transaction fails, or if the
    /// persisted revision or count is outside supported numeric bounds.
    pub async fn library_metadata_snapshot(&self) -> Result<LibraryMetadataSnapshot, StoreError> {
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let revision = library_revision(&transaction)?;
            let scenario_count: i64 =
                transaction.query_row("SELECT COUNT(*) FROM scenarios", [], |row| row.get(0))?;
            let scenario_count = i64_to_u64(scenario_count)?;
            transaction.commit()?;
            Ok(LibraryMetadataSnapshot {
                revision,
                scenario_count,
            })
        })
        .await
    }

    /// Holds a cross-process `SQLite` write lease from revision validation through
    /// one caller-supplied portable publication commit point.
    ///
    /// The callback should perform only the already-prepared atomic filesystem
    /// commit. Expensive encoding, temporary-file writing, and verification
    /// belong before this lease.
    ///
    /// # Errors
    ///
    /// Returns a revision conflict, missing scenario, database, or actor error.
    /// Callback failures are returned as the inner result after the lease is
    /// released.
    pub async fn with_publication_revision_lease<T, E, F>(
        &self,
        expected_library_revision: Revision,
        expected_scenario: Option<(ScenarioId, Revision)>,
        publish: F,
    ) -> Result<Result<T, E>, StoreError>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce() -> Result<T, E> + Send + 'static,
    {
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let actual_library_revision = library_revision(&transaction)?;
            if actual_library_revision != expected_library_revision {
                return Err(StoreError::LibraryConflict {
                    expected: expected_library_revision,
                    actual: actual_library_revision,
                });
            }
            if let Some((scenario_id, expected_revision)) = expected_scenario {
                let actual_revision = scenario_revision(&transaction, scenario_id)?;
                ensure_revision(expected_revision, actual_revision.value())?;
            }
            let publication = publish();
            // This transaction is a read-only writer lease. Dropping it rolls
            // back no state and cannot obscure a completed filesystem commit.
            drop(transaction);
            Ok(publication)
        })
        .await
    }

    /// Reads the portable-library revision, authoritative documents and wrapper
    /// metadata, application settings, supplemental identities, JSON records,
    /// assets and their declarations, and import provenance from one snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor or snapshot transaction fails, or if any
    /// stored identifier, timestamp, numeric value, or JSON payload is invalid.
    pub async fn library_snapshot(&self) -> Result<LibrarySnapshot, StoreError> {
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let revision = library_revision(&transaction)?;
            let mut statement = transaction.prepare("SELECT id FROM scenarios ORDER BY id")?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            let mut projects = Vec::with_capacity(ids.len());
            for id in ids {
                let scenario_id = id.parse().map_err(|error| {
                    StoreError::Integrity(format!("invalid stored scenario id: {error}"))
                })?;
                projects.push(load_project(&transaction, scenario_id)?);
            }
            let scenario_revisions = load_retained_scenario_revisions(&transaction)?;
            let scenario_revision_high_water = load_scenario_revision_high_water(&transaction)?;
            let scenario_identity_owners = load_scenario_identity_owners(&transaction)?;
            let settings = load_all_settings(&transaction)?;
            let (manifest_extensions, nonsemantic_extensions) =
                load_portable_library_metadata(&transaction)?;
            let supplemental_identity_owners = load_supplemental_identity_owners(&transaction)?;
            let sections = load_portable_sections(&transaction)?;
            let supplemental_identities = portable_section_identities(&sections);
            let provenance = load_import_provenance(&transaction)?;
            transaction.commit()?;
            Ok(LibrarySnapshot {
                revision,
                projects,
                settings,
                scenario_revisions,
                scenario_revision_high_water,
                scenario_identity_owners,
                manifest_extensions,
                nonsemantic_extensions,
                sections,
                provenance,
                supplemental_identities,
                supplemental_identity_owners,
            })
        })
        .await
    }

    /// Applies a fully staged portable import or backup restore atomically.
    ///
    /// # Errors
    ///
    /// Returns an error when the staged input is invalid or stale, a referenced
    /// scenario is missing or already occupied, a document cannot be
    /// serialized, a numeric value is out of range, the storage transaction
    /// fails, cancellation wins before commit, or the database actor is unavailable.
    /// Committed success wins over later cancellation, including no-effect applies.
    pub async fn apply_staged_library(
        &self,
        staged: StagedLibraryApply,
        applied_at: Rfc3339Timestamp,
        cancellation: CancellationToken,
    ) -> Result<LibraryApplyOutcome, StoreError> {
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        #[cfg(debug_assertions)]
        let command_commit_test_hook = self.command_commit_test_hook.clone();
        self.call(move |connection| {
            apply_staged_library_transaction(
                connection,
                staged,
                applied_at,
                &cancellation,
                #[cfg(debug_assertions)]
                &failpoint,
                #[cfg(debug_assertions)]
                command_commit_test_hook.as_ref(),
            )
        })
        .await
    }

    /// Records one authorization receipt after a real safety-backup attempt fails.
    ///
    /// The plaintext proof is hashed before the database actor receives the
    /// operation and is never persisted.
    ///
    /// # Errors
    ///
    /// Returns an error when receipt fields violate their bounds, the proof was
    /// already recorded, or the database actor or transaction fails.
    pub async fn record_safety_backup_failure_receipt(
        &self,
        receipt: SafetyBackupFailureReceipt,
    ) -> Result<(), StoreError> {
        let SafetyBackupFailureReceipt {
            proof,
            binding,
            collision_plan_sha256,
            safe_reason,
            created_at,
        } = receipt;
        validate_failure_receipt_fields(&proof, &collision_plan_sha256, &safe_reason)?;
        let proof_sha256 = eutheto_export::sha256_hex(proof.as_bytes());
        let binding_json = eutheto_export::canonical_json(&binding)
            .map_err(|error| StoreError::InvalidSafetyBackupFailureReceipt(error.to_string()))?;
        if binding_json.len() > MAX_RECEIPT_BINDING_BYTES {
            return Err(StoreError::InvalidSafetyBackupFailureReceipt(
                "preview binding exceeds the receipt size limit".to_owned(),
            ));
        }
        if binding_json
            .windows(proof.len())
            .any(|window| window == proof.as_bytes())
        {
            return Err(StoreError::InvalidSafetyBackupFailureReceipt(
                "preview binding contains the plaintext proof".to_owned(),
            ));
        }
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM safety_backup_failure_receipts WHERE proof_sha256 = ?1)",
                [&proof_sha256],
                |row| row.get(0),
            )?;
            if exists {
                return Err(StoreError::InvalidSafetyBackupFailureReceipt(
                    "proof was already recorded".to_owned(),
                ));
            }
            transaction.execute(
                "INSERT INTO safety_backup_failure_receipts (proof_sha256, binding_json, collision_plan_sha256, safe_reason, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    proof_sha256,
                    binding_json,
                    collision_plan_sha256,
                    safe_reason,
                    created_at.to_string(),
                ],
            )?;
            transaction.execute(
                "DELETE FROM safety_backup_failure_receipts WHERE id NOT IN (SELECT id FROM safety_backup_failure_receipts ORDER BY id DESC LIMIT ?1)",
                [u64_to_i64(MAX_SAFETY_BACKUP_FAILURE_RECEIPTS)?],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    /// Returns the authoritative stored project.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario does not exist, the database actor or
    /// query fails, or its persisted metadata or document JSON is invalid.
    pub async fn get_project(&self, scenario_id: ScenarioId) -> Result<StoredProject, StoreError> {
        self.call(move |connection| load_project(connection, scenario_id))
            .await
    }

    /// Starts an immutable V2 solve run or reuses the run already bound to the
    /// same request identity and canonical request hash.
    ///
    /// # Errors
    ///
    /// Returns an error for an unavailable actor, invalid request or persisted
    /// input, request/run identity conflict, stale scenario revision, snapshot
    /// mismatch, serialization bound violation, or database failure.
    pub async fn start_solve_run(
        &self,
        request: NewSolveRunV1,
    ) -> Result<StartedSolveRunV1, StoreError> {
        let snapshot_policy = self.snapshot_policy;
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        self.call(move |connection| {
            start_solve_run_transaction(
                connection,
                request,
                snapshot_policy,
                #[cfg(debug_assertions)]
                &failpoint,
            )
        })
        .await
    }

    /// Loads the exact immutable document snapshot and typed V2 input for a run.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor or run is unavailable, the persisted input
    /// is invalid, the snapshot binding does not match, bounded decompression
    /// fails, or the exact scenario document is invalid.
    pub async fn load_solve_input(
        &self,
        run_id: SolveRunId,
    ) -> Result<LoadedSolveInputV1, StoreError> {
        self.call(move |connection| load_solve_input_row(connection, run_id))
            .await
    }

    /// Resolves idempotent request state before a caller reads moving-current scenario data.
    ///
    /// # Errors
    /// Rejects inconsistent persisted input, terminal bindings, timing columns or database errors.
    pub async fn load_solve_run_by_request(
        &self,
        request_id: RequestId,
    ) -> Result<Option<ExistingSolveRunV1>, StoreError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let row: Option<(String, Option<String>, Option<i64>)> = transaction
                .query_row(
                    "SELECT id, finished_at, elapsed_ms FROM solve_runs WHERE request_id = ?1",
                    [request_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((run_id, finished_at, elapsed_ms)) = row else {
                return Ok(None);
            };
            let run_id: SolveRunId = run_id.parse().map_err(|_| {
                StoreError::InvalidPersistedRun("invalid retained run ID".to_owned())
            })?;
            let (input, status, manifest_json, started_at) =
                load_run_input_record(&transaction, run_id)?;
            let manifest = manifest_json
                .map(|json| RunManifestV1::from_json(json.as_bytes()))
                .transpose()
                .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))?;
            let finished_at = finished_at
                .map(|value| parse_timestamp(&value, "solve-run finished_at"))
                .transpose()?;
            let elapsed_ms = elapsed_ms.map(i64_to_u64).transpose()?;
            let consistent = match &manifest {
                None => status == "running" && finished_at.is_none() && elapsed_ms.is_none(),
                Some(manifest) => {
                    manifest.run_id == input.run_id
                        && manifest.run_input_checksum == input.checksum
                        && manifest.started_at == started_at
                        && Some(manifest.finished_at) == finished_at
                        && manifest
                            .elapsed_milliseconds
                            .map(eutheto_types::DurationMillis::value)
                            == elapsed_ms
                        && terminal_status(&manifest.outcome)? == status
                }
            };
            if input.request_id != request_id || !consistent {
                return Err(StoreError::InvalidPersistedRun(
                    "retained request state disagrees with immutable run authority".to_owned(),
                ));
            }
            transaction.commit()?;
            Ok(Some(ExistingSolveRunV1 {
                input,
                started_at,
                manifest,
            }))
        })
        .await
    }
    /// Lists compact independently accepted results for a scenario, newest first.
    ///
    /// The list never exposes unverified, quarantined, or legacy rows. Every
    /// returned summary is reconstructed through the same local-authority path
    /// as [`Self::load_accepted_result`].
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ScenarioNotFound`] when the scenario is absent,
    /// or rejects invalid persisted authority and column drift.
    pub async fn list_accepted_results(
        &self,
        scenario_id: ScenarioId,
    ) -> Result<Vec<StoredAcceptedResultSummary>, StoreError> {
        self.call(move |connection| {
            let current_revision = scenario_revision(connection, scenario_id)?;
            let selected_solution_id = selected_solution_id(connection, scenario_id)?;
            let solution_ids = {
                let mut statement = connection.prepare(
                    "SELECT id FROM solutions WHERE scenario_id = ?1 AND accepted = 1 AND status = 'verified' ORDER BY created_at DESC, id",
                )?;
                statement
                    .query_map([scenario_id.to_string()], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let mut summaries = Vec::with_capacity(solution_ids.len());
            let mut selected_found = selected_solution_id.is_none();
            for solution_id in solution_ids {
                let solution_id: SolutionId = solution_id.parse().map_err(|error| {
                    StoreError::InvalidPersistedResult(format!("invalid solution ID: {error}"))
                })?;
                let portable = load_local_accepted_authority(connection, solution_id)?;
                if portable.scenario_id != scenario_id {
                    return Err(StoreError::InvalidPersistedResult(
                        "accepted result disagrees with its scenario list".to_owned(),
                    ));
                }
                let selected = selected_solution_id == Some(solution_id);
                selected_found |= selected;
                summaries.push(accepted_result_summary(
                    &portable,
                    current_revision,
                    selected,
                )?);
            }
            if !selected_found {
                return Err(StoreError::InvalidPersistedResult(
                    "selected solution is not accepted local authority".to_owned(),
                ));
            }
            Ok(summaries)
        })
        .await
    }

    /// Selects one independently accepted local result as scenario viewing state.
    ///
    /// Selection does not change the scenario document, scenario revision, or
    /// portable-library revision. Results from an older scenario revision remain
    /// selectable and are returned with `stale` set.
    ///
    /// # Errors
    ///
    /// Returns a revision conflict for stale scenario authority, not-found for
    /// missing, unaccepted, legacy, or cross-scenario results, and rejects
    /// invalid persisted accepted-result authority.
    pub async fn select_accepted_result(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        solution_id: SolutionId,
    ) -> Result<StoredAcceptedResultSummary, StoreError> {
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current_revision = scenario_revision(&transaction, scenario_id)?;
            ensure_revision(expected_revision, current_revision.value())?;
            let portable = load_local_accepted_authority(&transaction, solution_id)?;
            if portable.scenario_id != scenario_id {
                return Err(StoreError::AcceptedResultNotFound(solution_id));
            }
            let summary = accepted_result_summary(&portable, current_revision, true)?;
            transaction.execute(
                "UPDATE scenarios SET selected_solution_id = ?2 WHERE id = ?1",
                params![scenario_id.to_string(), solution_id.to_string()],
            )?;
            transaction.commit()?;
            Ok(summary)
        })
        .await
    }

    /// Reads the selected independently accepted local result for one scenario.
    ///
    /// # Errors
    ///
    /// Returns not-found when the scenario is absent and rejects invalid
    /// persisted selection or accepted-result authority.
    pub async fn selected_accepted_result(
        &self,
        scenario_id: ScenarioId,
    ) -> Result<Option<StoredAcceptedResultSummary>, StoreError> {
        self.call(move |connection| {
            let current_revision = scenario_revision(connection, scenario_id)?;
            let Some(solution_id) = selected_solution_id(connection, scenario_id)? else {
                return Ok(None);
            };
            let portable =
                load_local_accepted_authority(connection, solution_id).map_err(|error| {
                    if matches!(error, StoreError::AcceptedResultNotFound(_)) {
                        StoreError::InvalidPersistedResult(
                            "selected solution is not accepted local authority".to_owned(),
                        )
                    } else {
                        error
                    }
                })?;
            if portable.scenario_id != scenario_id {
                return Err(StoreError::InvalidPersistedResult(
                    "selected accepted result belongs to another scenario".to_owned(),
                ));
            }
            accepted_result_summary(&portable, current_revision, true).map(Some)
        })
        .await
    }

    /// Loads one exact immutable scenario snapshot and its complete validated
    /// independently accepted result.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::AcceptedResultNotFound`] when the identity is
    /// missing or not independently accepted, and
    /// [`StoreError::InvalidPersistedResult`] when stored authority is invalid.
    pub async fn load_accepted_result(
        &self,
        solution_id: SolutionId,
    ) -> Result<StoredAcceptedResultV2, StoreError> {
        self.call(move |connection| load_stored_accepted_result(connection, solution_id))
            .await
    }

    /// Atomically finalizes a running solve and persists its independently
    /// accepted canonical result.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor or run is unavailable, the run is already
    /// terminal, any accepted-result binding or bounded JSON is invalid, the
    /// solve deadline has passed, or the atomic database commit fails.
    pub async fn finalize_accepted_run(
        &self,
        accepted_result: AcceptedResult,
        manifest: RunManifestV1,
        evidence: BTreeMap<DomainEvidenceId, VerificationValue>,
    ) -> Result<(), StoreError> {
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        self.call(move |connection| {
            finalize_accepted_run_transaction(
                connection,
                &accepted_result,
                &manifest,
                &evidence,
                #[cfg(debug_assertions)]
                &failpoint,
            )
        })
        .await
    }

    /// Atomically quarantines a running solve after an independent
    /// verification alarm. No solution row is created.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor or run is unavailable, the run is already
    /// terminal, the manifest is not a verification alarm, diagnostics are
    /// invalid or unsafe, or the atomic database commit fails.
    pub async fn finalize_quarantined_run(
        &self,
        manifest: RunManifestV1,
        candidate_diagnostics: CandidateDiagnosticsV1,
    ) -> Result<(), StoreError> {
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        self.call(move |connection| {
            finalize_quarantined_run_transaction(
                connection,
                &manifest,
                &candidate_diagnostics,
                #[cfg(debug_assertions)]
                &failpoint,
            )
        })
        .await
    }

    /// Atomically finalizes a running solve with a non-result or interrupted
    /// terminal manifest. No solution row is created.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor or run is unavailable, the run is already
    /// terminal, the manifest is invalid or has an unsupported outcome, or the
    /// atomic database commit fails.
    pub async fn finalize_terminal_run(&self, manifest: RunManifestV1) -> Result<(), StoreError> {
        self.call(move |connection| finalize_terminal_run_transaction(connection, &manifest))
            .await
    }

    /// Atomically creates or reuses an exact counterfactual job request.
    ///
    /// # Errors
    ///
    /// Returns a typed identity, authority, persisted-data, revision, actor, or
    /// database error. No partial job or portable-library revision mutation is committed.
    pub async fn start_counterfactual_job(
        &self,
        request: CounterfactualJobRequestV1,
    ) -> Result<StartedCounterfactualJobV1, StoreError> {
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        self.call(move |connection| {
            start_counterfactual_job_transaction(
                connection,
                &request,
                #[cfg(debug_assertions)]
                &failpoint,
            )
        })
        .await
    }

    /// Atomically creates and binds the exact derived solve run owned by a running job.
    ///
    /// # Errors
    ///
    /// Returns a transition conflict unless the job is running, uncancelled, and
    /// either unbound or already bound to this exact run/request pair.
    pub async fn start_counterfactual_run(
        &self,
        job_id: CounterfactualJobId,
        request: NewSolveRunV1,
    ) -> Result<StartedSolveRunV1, StoreError> {
        let snapshot_policy = self.snapshot_policy;
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        self.call(move |connection| {
            start_counterfactual_run_transaction(
                connection,
                job_id,
                request,
                snapshot_policy,
                #[cfg(debug_assertions)]
                &failpoint,
            )
        })
        .await
    }

    /// Atomically finalizes a counterfactual-owned solve run and its owning job.
    ///
    /// # Errors
    ///
    /// Returns a transition or authority error when ownership, cancellation,
    /// terminal outcome, accepted authority, or result bindings do not match.
    pub async fn finalize_counterfactual_run(
        &self,
        job_id: CounterfactualJobId,
        finalization: CounterfactualRunFinalizationV1,
    ) -> Result<CounterfactualJobRecordV1, StoreError> {
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        self.call(move |connection| {
            finalize_counterfactual_run_transaction(
                connection,
                job_id,
                &finalization,
                false,
                #[cfg(debug_assertions)]
                &failpoint,
            )
        })
        .await
    }

    /// Loads and cross-checks one exact typed counterfactual lifecycle record.
    ///
    /// # Errors
    ///
    /// Returns a typed not-found error, or rejects invalid, oversized, secret-like,
    /// noncanonical, or column-inconsistent persisted data.
    pub async fn load_counterfactual_job(
        &self,
        job_id: CounterfactualJobId,
    ) -> Result<CounterfactualJobRecordV1, StoreError> {
        self.call(move |connection| load_counterfactual_job_row(connection, job_id))
            .await
    }

    /// Applies one legal counterfactual lifecycle transition using an explicit database CAS.
    ///
    /// # Errors
    ///
    /// Returns a typed not-found or transition conflict, rejects invalid local
    /// completion authority and persisted data, or returns an actor/database error.
    pub async fn transition_counterfactual_job(
        &self,
        job_id: CounterfactualJobId,
        transition: CounterfactualJobTransitionV1,
    ) -> Result<CounterfactualJobRecordV1, StoreError> {
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        self.call(move |connection| {
            transition_counterfactual_job_transaction(
                connection,
                job_id,
                &transition,
                #[cfg(debug_assertions)]
                &failpoint,
            )
        })
        .await
    }

    /// Linearizes a cancellation request against counterfactual completion.
    ///
    /// Queued jobs become cancelled immediately; running jobs retain the durable
    /// cancellation pair until cleanup transitions them to cancelled. A prior
    /// non-cancel terminal state is returned without mutation.
    ///
    /// # Errors
    ///
    /// Returns a typed not-found or cancel-identity conflict, rejects invalid
    /// timestamps and persisted data, or returns an actor/database error.
    pub async fn request_counterfactual_cancel(
        &self,
        job_id: CounterfactualJobId,
        cancel_request_id: RequestId,
        requested_at: Rfc3339Timestamp,
    ) -> Result<CounterfactualCancelOutcomeV1, StoreError> {
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        self.call(move |connection| {
            request_counterfactual_cancel_transaction(
                connection,
                job_id,
                cancel_request_id,
                requested_at,
                #[cfg(debug_assertions)]
                &failpoint,
            )
        })
        .await
    }

    /// Records that a project was opened and returns its authoritative view.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario does not exist, the database actor or
    /// transaction fails, the library revision overflows, or the authoritative
    /// project contains invalid persisted data.
    pub async fn open_project(
        &self,
        scenario_id: ScenarioId,
        opened_at: Rfc3339Timestamp,
    ) -> Result<StoredProject, StoreError> {
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let changed = transaction.execute(
                "UPDATE scenarios SET last_opened_at = ?2 WHERE id = ?1",
                params![scenario_id.to_string(), opened_at.to_string()],
            )?;
            if changed == 0 {
                return Err(StoreError::ScenarioNotFound(scenario_id));
            }
            increment_library_revision(&transaction)?;
            let project = load_project(&transaction, scenario_id)?;
            transaction.commit()?;
            Ok(project)
        })
        .await
    }

    /// Marks a project as archived.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario does not exist, its revision differs
    /// from `expected_revision`, the transaction fails, or the database actor
    /// is unavailable.
    pub async fn archive_project(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        archived_at: Rfc3339Timestamp,
    ) -> Result<(), StoreError> {
        update_archive(self, scenario_id, expected_revision, Some(archived_at)).await
    }

    /// Marks an archived project as active.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario does not exist, its revision differs
    /// from `expected_revision`, the transaction fails, or the database actor
    /// is unavailable.
    pub async fn unarchive_project(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
    ) -> Result<(), StoreError> {
        update_archive(self, scenario_id, expected_revision, None).await
    }

    /// Permanently deletes a project, its dependent rows, and supplemental JSON
    /// records that declare a reference to it. Shared inert assets are retained.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario does not exist, its revision differs
    /// from `expected_revision`, the transaction fails, or the database actor
    /// is unavailable.
    pub async fn delete_project(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
    ) -> Result<(), StoreError> {
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let actual_revision = scenario_revision(&transaction, scenario_id)?;
            ensure_revision(expected_revision, actual_revision.value())?;
            record_scenario_revision_high_water(&transaction, scenario_id, actual_revision)?;
            delete_scenario_referencing_supplemental(&transaction, scenario_id)?;
            transaction.execute(
                "DELETE FROM scenarios WHERE id = ?1",
                [scenario_id.to_string()],
            )?;
            synchronize_retained_scenario_revisions(&transaction, Vec::new())?;
            increment_library_revision(&transaction)?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    /// Duplicates only the project document. Journal, snapshots, runs, and
    /// solutions deliberately remain attached to the source project.
    ///
    /// # Errors
    ///
    /// Returns an error if the source scenario is missing, its revision differs
    /// from `expected_source_revision`, the destination already exists, the
    /// source document is malformed or cannot be serialized, a numeric value is
    /// out of range, the transaction fails, or the database actor is
    /// unavailable.
    pub async fn duplicate_project(
        &self,
        source_id: ScenarioId,
        expected_source_revision: Revision,
        new_id: ScenarioId,
        id_remap: BTreeMap<Uuid, Uuid>,
        title: String,
        created_at: Rfc3339Timestamp,
    ) -> Result<StoredProject, StoreError> {
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let source = load_project(&transaction, source_id)?;
            ensure_revision(expected_source_revision, source.summary.revision.value())?;
            let occupied = authoritative_occupied_uuids(&transaction)?;
            let (document, portable) = duplicate_project_contents(
                source, new_id, &id_remap, &occupied, &title, created_at,
            )?;
            insert_project(&transaction, &document, Revision::INITIAL, None, &portable)?;
            validate_global_identity_ownership(&transaction)?;
            increment_library_revision(&transaction)?;
            transaction.commit()?;
            load_project(connection, new_id)
        })
        .await
    }

    /// Runs a pure command callback after checking the current revision and
    /// commits the document, journal, cursor, revision, and optional snapshot
    /// in one immediate transaction.
    /// Cancellation observed inside the actor before the callback or at the final
    /// pre-commit check rolls back the entire transaction. After that final check,
    /// commit wins: later cancellation never changes a successful commit into an error.
    /// Dropping the awaiting future does not cancel actor work; cancel the token.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario is missing, its revision conflicts,
    /// redo history requires explicit truncation, the callback returns a typed
    /// command error, command data cannot be serialized, numeric bounds are
    /// exceeded, snapshot sizing or compression fails, the transaction or actor
    /// fails, cancellation is observed before commit, or a debug failpoint is triggered.
    pub async fn execute_command<T, F>(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        branch_policy: RedoBranchPolicy,
        cancellation: CancellationToken,
        apply: F,
    ) -> Result<CommittedCommand<T>, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&ScenarioDocument) -> Result<CommandWrite<T>, StoreError> + Send + 'static,
    {
        let snapshot_policy = self.snapshot_policy;
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        #[cfg(debug_assertions)]
        let command_commit_test_hook = self.command_commit_test_hook.clone();
        self.call(move |connection| {
            Self::check_command_cancelled(&cancellation)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let project = load_project(&transaction, scenario_id)?;
            retain_current_project_revision_if_required(&transaction, &project)?;
            let actual_revision = project.summary.revision.value();
            let document = project.document;
            let portable = project.portable;
            ensure_revision(expected_revision, actual_revision)?;
            let (cursor, mut generation, max_sequence) = history_state(&transaction, scenario_id)?;
            if cursor < max_sequence {
                match branch_policy {
                    RedoBranchPolicy::Reject => return Err(StoreError::RedoBranchRequiresTruncation),
                    RedoBranchPolicy::Truncate => {
                        transaction.execute(
                            "DELETE FROM command_journal WHERE scenario_id = ?1 AND history_sequence > ?2",
                            params![scenario_id.to_string(), u64_to_i64(cursor)?],
                        )?;
                        generation = generation.checked_add(1).ok_or(StoreError::NumericRange)?;
                    }
                }
            }
            Self::check_command_cancelled(&cancellation)?;
            let write = apply(&document)?;
            let new_revision = checked_revision(actual_revision)?
                .checked_next()
                .map_err(|_| StoreError::NumericRange)?;
            validate_project_owned_uuid_uniqueness(&write.document, new_revision, &portable)
                .map_err(StoreError::InvalidScenarioIdentity)?;
            record_scenario_identity_owners(
                &transaction,
                scenario_id,
                &collect_project_owned_uuids(
                    &write.document,
                    &portable.semantic_extensions,
                    &portable.extensions,
                ),
            )?;
            let sequence = cursor.checked_add(1).ok_or(StoreError::NumericRange)?;
            let document_json = serialize_document(&write.document)?;
            let projection = document_projection(&write.document);
            let command_json = serde_json::to_string(&write.journal.command)?;
            let inverse_json = write
                .journal
                .inverse
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            let actor_json = serde_json::to_string(&write.journal.actor)?;
            transaction.execute(
                "UPDATE scenarios SET domain_pack_id = ?2, domain_schema_version = ?3, title = ?4, description = ?5, document_json = ?6, revision = ?7, updated_at = ?8 WHERE id = ?1",
                params![
                    scenario_id.to_string(),
                    projection.domain_pack_id,
                    u32_to_i64(projection.domain_schema_version),
                    projection.title,
                    projection.description,
                    document_json,
                    u64_to_i64(new_revision.value())?,
                    projection.updated_at,
                ],
            )?;
            record_scenario_revision_high_water(
                &transaction,
                scenario_id,
                new_revision,
            )?;
            #[cfg(debug_assertions)]
            consume_failpoint(&failpoint, Failpoint::AfterDocumentWrite)?;
            transaction.execute(
                "INSERT INTO command_journal (id, scenario_id, revision_before, revision_after, command_type, command_json, inverse_json, actor_json, source, summary, created_at, history_sequence, branch_generation) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![write.journal.command_id.to_string(), scenario_id.to_string(), u64_to_i64(actual_revision)?, u64_to_i64(new_revision.value())?, write.journal.command_type, command_json, inverse_json, actor_json, command_source_name(write.journal.source), write.journal.summary, write.journal.created_at.to_string(), u64_to_i64(sequence)?, u64_to_i64(generation)?],
            )?;
            transaction.execute(
                "UPDATE scenario_history_state SET cursor_sequence = ?2, branch_generation = ?3 WHERE scenario_id = ?1",
                params![scenario_id.to_string(), u64_to_i64(sequence)?, u64_to_i64(generation)?],
            )?;
            maybe_snapshot(&transaction, scenario_id, new_revision.value(), sequence, &write.document, &write.journal.created_at, snapshot_policy)?;
            validate_global_identity_ownership(&transaction)?;
            increment_library_revision(&transaction)?;
            Self::commit_command(
                transaction,
                &cancellation,
                #[cfg(debug_assertions)]
                command_commit_test_hook.as_ref(),
            )?;
            Ok(CommittedCommand {
                new_revision,
                output: write.output,
            })
        })
        .await
    }

    fn check_command_cancelled(cancellation: &CancellationToken) -> Result<(), StoreError> {
        if cancellation.is_cancelled() {
            Err(StoreError::OperationCancelled)
        } else {
            Ok(())
        }
    }

    fn commit_command(
        transaction: rusqlite::Transaction<'_>,
        cancellation: &CancellationToken,
        #[cfg(debug_assertions)] hook: Option<&CommandCommitTestHook>,
    ) -> Result<(), StoreError> {
        #[cfg(debug_assertions)]
        if let Some(hook) = hook {
            hook.actor_pause(CommandCommitTestPhase::BeforeFinalCancellationCheck);
        }
        Self::check_command_cancelled(cancellation)?;
        #[cfg(debug_assertions)]
        if let Some(hook) = hook {
            hook.actor_pause(CommandCommitTestPhase::AfterFinalCancellationCheck);
        }
        transaction.commit()?;
        Ok(())
    }

    /// Applies the inverse of the current history entry.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario is missing, its revision conflicts,
    /// undo is unavailable, the command is not reversible, the callback returns
    /// a typed command error, stored history or JSON is invalid, numeric bounds
    /// are exceeded, or the transaction or actor fails.
    pub async fn undo<T, F>(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        applied_at: Rfc3339Timestamp,
        apply_inverse: F,
    ) -> Result<CommittedCommand<T>, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(HistoryCommand) -> Result<(ScenarioDocument, T), StoreError> + Send + 'static,
    {
        self.move_history(
            scenario_id,
            expected_revision,
            applied_at,
            HistoryDirection::Undo,
            apply_inverse,
        )
        .await
    }

    /// Reapplies the next command in the redo history.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario is missing, its revision conflicts,
    /// redo is unavailable, the callback returns a typed command error, stored
    /// history or JSON is invalid, numeric bounds are exceeded, or the
    /// transaction or actor fails.
    pub async fn redo<T, F>(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        applied_at: Rfc3339Timestamp,
        apply_command: F,
    ) -> Result<CommittedCommand<T>, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(HistoryCommand) -> Result<(ScenarioDocument, T), StoreError> + Send + 'static,
    {
        self.move_history(
            scenario_id,
            expected_revision,
            applied_at,
            HistoryDirection::Redo,
            apply_command,
        )
        .await
    }

    async fn move_history<T, F>(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        applied_at: Rfc3339Timestamp,
        direction: HistoryDirection,
        apply: F,
    ) -> Result<CommittedCommand<T>, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(HistoryCommand) -> Result<(ScenarioDocument, T), StoreError> + Send + 'static,
    {
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let project = load_project(&transaction, scenario_id)?;
            retain_current_project_revision_if_required(&transaction, &project)?;
            let actual_revision = project.summary.revision.value();
            let document = project.document;
            let portable = project.portable;
            ensure_revision(expected_revision, actual_revision)?;
            let (cursor, _generation, max_sequence) = history_state(&transaction, scenario_id)?;
            let target_sequence = match direction {
                HistoryDirection::Undo if cursor == 0 => return Err(StoreError::NoUndo),
                HistoryDirection::Undo => cursor,
                HistoryDirection::Redo if cursor >= max_sequence => return Err(StoreError::NoRedo),
                HistoryDirection::Redo => cursor.checked_add(1).ok_or(StoreError::NumericRange)?,
            };
            let entry = load_history_entry(&transaction, scenario_id, target_sequence, cursor)?;
            let command = match direction {
                HistoryDirection::Undo => entry.inverse.clone().ok_or(StoreError::CommandNotReversible)?,
                HistoryDirection::Redo => entry.command.clone(),
            };
            let new_cursor = match direction {
                HistoryDirection::Undo => cursor.checked_sub(1).ok_or(StoreError::NumericRange)?,
                HistoryDirection::Redo => target_sequence,
            };
            let target_document_updated_at = match direction {
                HistoryDirection::Undo if new_cursor == 0 => document.metadata.created_at,
                HistoryDirection::Undo => {
                    load_history_entry(&transaction, scenario_id, new_cursor, cursor)?.created_at
                }
                HistoryDirection::Redo => entry.created_at,
            };
            let (updated_document, output) = apply(HistoryCommand {
                document,
                command,
                entry,
                target_document_updated_at,
            })?;
            let new_revision = checked_revision(actual_revision)?
                .checked_next()
                .map_err(|_| StoreError::NumericRange)?;
            validate_project_owned_uuid_uniqueness(&updated_document, new_revision, &portable)
                .map_err(StoreError::InvalidScenarioIdentity)?;
            record_scenario_identity_owners(
                &transaction,
                scenario_id,
                &collect_project_owned_uuids(
                    &updated_document,
                    &portable.semantic_extensions,
                    &portable.extensions,
                ),
            )?;
            let projection = document_projection(&updated_document);
            transaction.execute(
                "UPDATE scenarios SET domain_pack_id = ?2, domain_schema_version = ?3, title = ?4, description = ?5, document_json = ?6, revision = ?7, updated_at = ?8 WHERE id = ?1",
                params![
                    scenario_id.to_string(),
                    projection.domain_pack_id,
                    u32_to_i64(projection.domain_schema_version),
                    projection.title,
                    projection.description,
                    serialize_document(&updated_document)?,
                    u64_to_i64(new_revision.value())?,
                    applied_at.to_string(),
                ],
            )?;
            record_scenario_revision_high_water(
                &transaction,
                scenario_id,
                new_revision,
            )?;
            transaction.execute(
                "UPDATE scenario_history_state SET cursor_sequence = ?2 WHERE scenario_id = ?1",
                params![scenario_id.to_string(), u64_to_i64(new_cursor)?],
            )?;
            validate_global_identity_ownership(&transaction)?;
            increment_library_revision(&transaction)?;
            transaction.commit()?;
            Ok(CommittedCommand {
                new_revision,
                output,
            })
        })
        .await
    }

    /// Returns the command history in history-sequence order.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario is missing, the database actor or query
    /// fails, or persisted journal identifiers, timestamps, numeric values, or
    /// serialized command and actor data are invalid.
    pub async fn history(&self, scenario_id: ScenarioId) -> Result<Vec<HistoryEntry>, StoreError> {
        self.call(move |connection| {
            let exists = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM scenarios WHERE id = ?1)",
                [scenario_id.to_string()],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(StoreError::ScenarioNotFound(scenario_id));
            }
            let cursor = connection.query_row(
                "SELECT cursor_sequence FROM scenario_history_state WHERE scenario_id = ?1",
                [scenario_id.to_string()],
                |row| row.get::<_, i64>(0),
            )?;
            let cursor = i64_to_u64(cursor)?;
            let mut statement = connection.prepare(
                "SELECT id, revision_before, revision_after, command_type, command_json, inverse_json, actor_json, source, summary, created_at, history_sequence, branch_generation FROM command_journal WHERE scenario_id = ?1 ORDER BY history_sequence, branch_generation",
            )?;
            let mut rows = statement.query([scenario_id.to_string()])?;
            let mut entries = Vec::new();
            while let Some(row) = rows.next()? {
                entries.push(parse_history_row(row, cursor)?);
            }
            Ok(entries)
        })
        .await
    }

    /// Reads bounded journal metadata in descending sequence order in one snapshot.
    ///
    /// # Errors
    ///
    /// Rejects invalid numeric bounds, missing scenarios, revision conflicts,
    /// inconsistent persisted history metadata, and actor or database failures.
    pub async fn history_page(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        limit: u32,
        before_sequence: Option<u64>,
    ) -> Result<HistoryPage, StoreError> {
        if !(1..=HISTORY_PAGE_MAX_ENTRIES).contains(&limit) {
            return Err(StoreError::NumericRange);
        }
        if let Some(sequence) = before_sequence {
            if sequence == 0 {
                return Err(StoreError::NumericRange);
            }
            checked_revision(sequence)?;
        }
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let scenario_key = scenario_id.to_string();
            let revision: Option<i64> = transaction
                .query_row(
                    "SELECT revision FROM scenarios WHERE id = ?1",
                    [&scenario_key],
                    |row| row.get(0),
                )
                .optional()?;
            let revision = checked_revision(i64_to_u64(
                revision.ok_or(StoreError::ScenarioNotFound(scenario_id))?,
            )?)?;
            ensure_revision(expected_revision, revision.value())?;
            let (cursor, generation, max_sequence) = history_state(&transaction, scenario_id)?;
            if cursor > max_sequence
                || max_sequence > revision.value()
                || generation > revision.value()
                || (max_sequence == 0 && generation != 0)
            {
                return Err(StoreError::Integrity("invalid history cursor state".to_owned()));
            }
            let (undo_available, redo_available) = history_page_availability(
                &transaction, &scenario_key, cursor, generation, max_sequence,
            )?;
            let mut entries = Vec::with_capacity(limit as usize);
            let mut next_before_sequence = None;
            {
                let mut statement = transaction.prepare(
                    "SELECT
                       CASE WHEN length(CAST(id AS BLOB)) <= 36 THEN id ELSE NULL END,
                       revision_before, revision_after,
                       CASE WHEN length(CAST(source AS BLOB)) <= 16 THEN source ELSE NULL END,
                       CASE WHEN length(CAST(summary AS BLOB)) <= ?4 THEN summary ELSE NULL END,
                       CASE WHEN length(CAST(created_at AS BLOB)) <= 64 THEN created_at ELSE NULL END,
                       history_sequence, branch_generation
                     FROM command_journal
                     WHERE scenario_id = ?1 AND (?2 IS NULL OR history_sequence < ?2)
                     ORDER BY history_sequence DESC LIMIT ?3",
                )?;
                let mut rows = statement.query(params![
                    &scenario_key,
                    before_sequence.map(u64_to_i64).transpose()?,
                    i64::from(limit) + 1,
                    i64::try_from(HISTORY_SUMMARY_MAX_BYTES).map_err(|_| StoreError::NumericRange)?,
                ])?;
                let mut expected_sequence =
                    before_sequence.map_or(max_sequence, |before| max_sequence.min(before - 1));
                while let Some(row) = rows.next()? {
                    let entry = parse_history_summary_row(row, cursor, generation, revision)?;
                    if entry.history_sequence != expected_sequence {
                        return Err(StoreError::Integrity(
                            "history page contains a missing or ambiguous sequence".to_owned(),
                        ));
                    }
                    expected_sequence -= 1;
                    if entries.len() == limit as usize {
                        next_before_sequence = entries.last().map(
                            |entry: &HistoryEntrySummary| entry.history_sequence,
                        );
                        break;
                    }
                    entries.push(entry);
                }
                if next_before_sequence.is_none() && expected_sequence != 0 {
                    return Err(StoreError::Integrity(
                        "history page ends before its expected sequence".to_owned(),
                    ));
                }
            }
            transaction.commit()?;
            Ok(HistoryPage {
                scenario_id,
                revision,
                entries,
                next_before_sequence,
                undo_available,
                redo_available,
            })
        })
        .await
    }

    /// Captures only the trusted setting keys and library revision in one read transaction.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor or transaction fails, or a requested stored
    /// setting or the library revision cannot be decoded.
    pub async fn settings_snapshot(
        &self,
        keys: &'static [&'static str],
    ) -> Result<AppSettingsSnapshot, StoreError> {
        self.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let library_revision = library_revision(&transaction)?;
            let settings = load_scoped_settings(&transaction, keys)?;
            transaction.commit()?;
            Ok(AppSettingsSnapshot {
                library_revision,
                settings,
            })
        })
        .await
    }

    /// Replaces the trusted setting scope after checking its reviewed revision and entries.
    ///
    /// Missing keys remove settings only within `keys`. Complete entries, including
    /// timestamps, determine whether the replacement changes the library. The caller
    /// remains responsible for validating setting keys and values semantically.
    ///
    /// # Errors
    ///
    /// Returns an error for cancellation before commit, a stale library revision or
    /// before-state, out-of-scope entries, invalid stored data, revision overflow,
    /// actor or transaction failure, or an injected debug failure. Every failure
    /// rolls back all writes; committed success wins over later cancellation.
    pub async fn replace_settings(
        &self,
        keys: &'static [&'static str],
        before: AppSettingsSnapshot,
        after: BTreeMap<String, AppSetting<Value>>,
        cancellation: CancellationToken,
    ) -> Result<SettingsCommit, StoreError> {
        #[cfg(debug_assertions)]
        let failpoint = Arc::clone(&self.failpoint);
        #[cfg(debug_assertions)]
        let command_commit_test_hook = self.command_commit_test_hook.clone();
        self.call(move |connection| {
            Self::check_command_cancelled(&cancellation)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let actual_revision = library_revision(&transaction)?;
            if before.library_revision != actual_revision {
                return Err(StoreError::LibraryConflict {
                    expected: before.library_revision,
                    actual: actual_revision,
                });
            }
            if before.settings.keys().chain(after.keys()).any(|key| !keys.contains(&key.as_str())) {
                return Err(StoreError::InvalidStagedApply(
                    "application settings replacement contains an out-of-scope key".to_owned(),
                ));
            }
            if load_scoped_settings(&transaction, keys)? != before.settings {
                return Err(StoreError::InvalidStagedApply(
                    "application settings before-state no longer matches".to_owned(),
                ));
            }
            let changed = before.settings != after;
            let library_revision = if changed {
                for key in before.settings.keys().filter(|key| !after.contains_key(*key)) {
                    transaction.execute("DELETE FROM app_settings WHERE key = ?1", [key])?;
                }
                for (key, setting) in &after {
                    if before.settings.get(key) != Some(setting) {
                        transaction.execute(
                            "INSERT INTO app_settings (key, value_json, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
                            params![key, serde_json::to_string(&setting.value)?, setting.updated_at.to_string()],
                        )?;
                    }
                }
                #[cfg(debug_assertions)]
                consume_failpoint(&failpoint, Failpoint::AfterSettingsWrite)?;
                increment_library_revision(&transaction)?
            } else {
                actual_revision
            };
            Self::commit_command(
                transaction,
                &cancellation,
                #[cfg(debug_assertions)]
                command_commit_test_hook.as_ref(),
            )?;
            Ok(SettingsCommit {
                library_revision,
                changed,
                settings: after,
            })
        })
        .await
    }

    /// Returns a typed application setting, if present.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor or query fails, the stored setting cannot
    /// be deserialized as `T`, or its persisted timestamp is invalid.
    pub async fn get_setting<T: DeserializeOwned + Send + 'static>(
        &self,
        key: String,
    ) -> Result<Option<AppSetting<T>>, StoreError> {
        self.call(move |connection| {
            let row = connection
                .query_row(
                    "SELECT value_json, updated_at FROM app_settings WHERE key = ?1",
                    [key],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            row.map(|(json, updated_at)| {
                let value = serde_json::from_str(&json)?;
                let updated_at = parse_timestamp(&updated_at, "app setting updated_at")?;
                Ok(AppSetting { value, updated_at })
            })
            .transpose()
        })
        .await
    }

    /// Returns bounded, non-sensitive database diagnostics.
    ///
    /// # Errors
    ///
    /// Returns an error if the database actor is unavailable or any diagnostic
    /// pragma or schema query fails.
    pub async fn diagnostics(&self) -> Result<StoreDiagnostics, StoreError> {
        self.call(move |connection| {
            let foreign_keys =
                connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
            let journal_mode =
                connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
            let synchronous =
                connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
            let busy_timeout_ms =
                connection.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
            let trusted_schema =
                connection.pragma_query_value(None, "trusted_schema", |row| row.get(0))?;
            let schema_version =
                connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
            let sqlite_length_limit_bytes =
                connection.limit(Limit::SQLITE_LIMIT_LENGTH)?;
            let mut statement = connection.prepare(
                "SELECT type, name FROM sqlite_schema WHERE type IN ('table', 'index') AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut tables = Vec::new();
            let mut indexes = Vec::new();
            for row in rows {
                let (kind, name) = row?;
                if kind == "table" {
                    tables.push(name);
                } else {
                    indexes.push(name);
                }
            }
            Ok(StoreDiagnostics {
                foreign_keys,
                journal_mode,
                synchronous,
                busy_timeout_ms,
                trusted_schema,
                sqlite_length_limit_bytes,
                schema_version,
                tables,
                indexes,
            })
        })
        .await
    }

    /// Creates a transactionally consistent `SQLite` safety backup without
    /// exposing the live connection. The destination must not exist.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::BackupDestinationExists`] if the destination
    /// already exists. Also returns an error if the actor is unavailable or
    /// changing durability settings, copying the database, or restoring the
    /// live connection settings fails.
    pub async fn safety_backup(&self, destination: impl AsRef<Path>) -> Result<(), StoreError> {
        let destination = destination.as_ref().to_path_buf();
        self.call(move |connection| {
            let guard = prepare_private_new_file(&destination)?;
            connection.pragma_update(None, "synchronous", "FULL")?;
            let backup_result = connection.backup(MAIN_DB, &destination, None);
            let restore_result = connection.pragma_update(None, "synchronous", "NORMAL");
            if let Err(error) = backup_result {
                remove_guarded_file(&destination, &guard);
                restore_result?;
                return Err(StoreError::Database(error));
            }
            restore_result?;
            ensure_same_file(&guard.file, &destination)?;
            restrict_private_file(&destination)?;
            Ok(())
        })
        .await
    }

    /// Arms a one-shot transaction failpoint in debug/test builds.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ActorUnavailable`] if the failpoint state lock has
    /// been poisoned.
    #[cfg(debug_assertions)]
    pub fn set_failpoint(&self, failpoint_value: Failpoint) -> Result<(), StoreError> {
        let mut guard = self
            .failpoint
            .lock()
            .map_err(|_| StoreError::ActorUnavailable)?;
        *guard = Some(failpoint_value);
        Ok(())
    }
}

fn solve_request_hash(
    request: &NewSolveRunV1,
    pack_id: &PackId,
    pack_schema_version: u32,
    scenario_timezone: &IanaTimeZone,
) -> Result<String, StoreError> {
    RunRequestSemanticsV1 {
        schema_version: RUN_REQUEST_SEMANTICS_SCHEMA_VERSION,
        scenario_id: request.scenario_id,
        scenario_revision: request.expected_revision.value(),
        pack_id: pack_id.clone(),
        pack_schema_version,
        planning_ir_schema_version: request.planning_ir_schema_version,
        compiler_version: request.compiler_version.clone(),
        application_version: request.application_version.clone(),
        backend_id: request.backend_id.clone(),
        backend_version: request.backend_version.clone(),
        adapter_version: request.adapter_version.clone(),
        worker_version: request.worker_version.clone(),
        solver_version: request.solver_version.clone(),
        protocol_major: request.protocol_major,
        protocol_minor: request.protocol_minor,
        model_hash: request.model_hash.clone(),
        objective_policy_hash: request.objective_policy_hash.clone(),
        solve_options: request.solve_options.clone(),
        scenario_timezone: scenario_timezone.clone(),
        temporary_condition_hash: request.temporary_condition_hash.clone(),
    }
    .canonical_hash()
    .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))
}

// Request idempotency, exact snapshot selection, and run insertion deliberately
// remain one cohesive transaction so no backend can observe a partial start.
#[allow(clippy::too_many_lines)]
fn start_solve_run_transaction(
    connection: &mut Connection,
    request: NewSolveRunV1,
    snapshot_policy: SnapshotPolicy,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<StartedSolveRunV1, StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let started = start_solve_run_in_transaction(
        &transaction,
        request,
        snapshot_policy,
        #[cfg(debug_assertions)]
        failpoint,
    )?;
    transaction.commit()?;
    Ok(started)
}

#[allow(clippy::too_many_lines)]
fn start_solve_run_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    request: NewSolveRunV1,
    snapshot_policy: SnapshotPolicy,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<StartedSolveRunV1, StoreError> {
    let existing: Option<(String, String, Option<String>)> = transaction
        .query_row(
            "SELECT id, input_hash, run_input_json FROM solve_runs WHERE request_id = ?1",
            [request.request_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((run_id_text, stored_hash, input_json)) = existing {
        if input_json.is_none() {
            return Err(StoreError::InvalidPersistedRun(
                "idempotent request has no V2 run input".to_owned(),
            ));
        }
        let run_id: SolveRunId = run_id_text
            .parse()
            .map_err(|error| StoreError::InvalidPersistedRun(format!("invalid run ID: {error}")))?;
        let (input, _, _, started_at) = load_run_input_record(transaction, run_id)?;
        let request_hash = solve_request_hash(
            &request,
            &input.pack_id,
            input.pack_schema_version,
            &input.scenario_timezone,
        )?;
        if stored_hash != request_hash {
            return Err(StoreError::SolveRequestIdConflict {
                request_id: request.request_id,
            });
        }
        if input.request_id != request.request_id || input.request_hash != request_hash {
            return Err(StoreError::InvalidPersistedRun(
                "idempotent request columns disagree with the typed input".to_owned(),
            ));
        }
        return Ok(StartedSolveRunV1 {
            input,
            started_at,
            reused: true,
        });
    }
    let project = load_project(transaction, request.scenario_id)?;
    let request_hash = solve_request_hash(
        &request,
        &project.document.domain_pack.id,
        project.document.domain_pack.schema_version,
        &project.document.settings.time_zone,
    )?;
    ensure_revision(request.expected_revision, project.summary.revision.value())?;
    let run_collision: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM solve_runs WHERE id = ?1)",
        [request.run_id.to_string()],
        |row| row.get(0),
    )?;
    if run_collision {
        return Err(StoreError::SolveRunCollision(request.run_id));
    }
    let document_json = serialize_document(&project.document)?;
    let document_hash = blake3::hash(document_json.as_bytes()).to_hex().to_string();
    let (snapshot_id, snapshot_created_at) = ensure_solve_snapshot(
        transaction,
        request.scenario_id,
        request.expected_revision,
        document_json.as_bytes(),
        request.started_at,
        snapshot_policy,
    )?;
    let input = RunInputV1::new(
        request.run_id,
        request.request_id,
        request.scenario_id,
        request.expected_revision.value(),
        snapshot_id,
        document_hash,
        snapshot_created_at,
        project.document.domain_pack.id.clone(),
        project.document.domain_pack.schema_version,
        request.planning_ir_schema_version,
        request.compiler_version,
        request.application_version,
        request.backend_id,
        request.backend_version,
        request.adapter_version,
        request.worker_version,
        request.solver_version,
        request.protocol_major,
        request.protocol_minor,
        request.model_hash,
        request.objective_policy_hash,
        request.solve_options.clone(),
        project.document.settings.time_zone.clone(),
        request.temporary_condition_hash,
    )
    .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))?;
    if input.request_hash != request_hash {
        return Err(StoreError::InvalidPersistedRun(
            "constructed run input disagrees with canonical request semantics".to_owned(),
        ));
    }
    let input_json = serialize_checked_json(&input, "solve-run input")
        .map_err(StoreError::InvalidPersistedRun)?;
    let options_json = serialize_checked_json(&request.solve_options, "solve options")
        .map_err(StoreError::InvalidPersistedRun)?;
    transaction.execute(
        "INSERT INTO solve_runs (id, request_id, scenario_id, scenario_revision, input_hash, backend_id, backend_version, protocol_version, status, options_json, started_at, run_input_json, run_manifest_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'running', ?9, ?10, ?11, NULL)",
        params![
            request.run_id.to_string(),
            request.request_id.to_string(),
            request.scenario_id.to_string(),
            u64_to_i64(request.expected_revision.value())?,
            request_hash,
            input.backend_id.to_string(),
            input.backend_version,
            u32_to_i64(input.protocol_major),
            options_json,
            request.started_at.to_string(),
            input_json,
        ],
    )?;
    validate_global_identity_ownership(transaction)?;
    #[cfg(debug_assertions)]
    consume_failpoint(failpoint, Failpoint::AfterSolveRunInsert)?;
    Ok(StartedSolveRunV1 {
        input,
        started_at: request.started_at,
        reused: false,
    })
}

fn ensure_solve_snapshot(
    transaction: &rusqlite::Transaction<'_>,
    scenario_id: ScenarioId,
    revision: Revision,
    document_bytes: &[u8],
    created_at: Rfc3339Timestamp,
    snapshot_policy: SnapshotPolicy,
) -> Result<(ScenarioSnapshotId, Rfc3339Timestamp), StoreError> {
    let existing: Option<(String, Vec<u8>, String)> = transaction
        .query_row(
            "SELECT id, document_json_zstd, created_at FROM scenario_snapshots WHERE scenario_id = ?1 AND revision = ?2",
            params![scenario_id.to_string(), u64_to_i64(revision.value())?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((snapshot_id, compressed, stored_created_at)) = existing {
        let snapshot_id: ScenarioSnapshotId = snapshot_id.parse().map_err(|error| {
            StoreError::InvalidPersistedRun(format!("invalid snapshot ID: {error}"))
        })?;
        if bounded_decompress_snapshot(&compressed)? != document_bytes {
            return Err(StoreError::SnapshotMismatch(snapshot_id));
        }
        let snapshot_created_at = parse_timestamp(&stored_created_at, "snapshot created_at")?;
        if snapshot_created_at > created_at {
            return Err(StoreError::InvalidPersistedRun(
                "solve start precedes its immutable snapshot".to_owned(),
            ));
        }
        return Ok((snapshot_id, snapshot_created_at));
    }
    if u64::try_from(document_bytes.len()).map_err(|_| StoreError::NumericRange)?
        > snapshot_policy.max_document_bytes
    {
        return Err(StoreError::SnapshotTooLarge);
    }
    let compressed = zstd::stream::encode_all(document_bytes, snapshot_policy.compression_level)
        .map_err(StoreError::Compression)?;
    let snapshot_id = ScenarioSnapshotId::from_uuid(Uuid::now_v7());
    transaction.execute(
        "INSERT INTO scenario_snapshots (id, scenario_id, revision, document_json_zstd, created_at, reason) VALUES (?1, ?2, ?3, ?4, ?5, 'solve-input')",
        params![
            snapshot_id.to_string(),
            scenario_id.to_string(),
            u64_to_i64(revision.value())?,
            compressed,
            created_at.to_string(),
        ],
    )?;
    Ok((snapshot_id, created_at))
}

fn bounded_decompress_snapshot(compressed: &[u8]) -> Result<Vec<u8>, StoreError> {
    let decoder = zstd::stream::read::Decoder::new(compressed).map_err(StoreError::Compression)?;
    let mut bytes = Vec::new();
    decoder
        .take(MAX_SCENARIO_DOCUMENT_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(StoreError::Compression)?;
    if u64::try_from(bytes.len()).map_err(|_| StoreError::NumericRange)?
        > MAX_SCENARIO_DOCUMENT_BYTES
    {
        return Err(StoreError::SnapshotTooLarge);
    }
    Ok(bytes)
}

fn parse_run_input(bytes: &[u8]) -> Result<RunInputV1, StoreError> {
    if u64::try_from(bytes.len()).map_err(|_| StoreError::NumericRange)?
        > eutheto_export::PORTABLE_LIMITS.max_json_bytes
    {
        return Err(StoreError::InvalidPersistedRun(
            "run input exceeds the JSON byte limit".to_owned(),
        ));
    }
    RunInputV1::from_json(bytes).map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))
}

struct PersistedRunRow {
    run_id: String,
    request_id: Option<String>,
    scenario_id: String,
    scenario_revision: i64,
    request_hash: String,
    status: String,
    manifest_json: Option<String>,
    started_at: String,
    input_json: Option<String>,
}

fn load_run_input_record(
    connection: &Connection,
    run_id: SolveRunId,
) -> Result<(RunInputV1, String, Option<String>, Rfc3339Timestamp), StoreError> {
    let row: Option<PersistedRunRow> = connection
        .query_row(
            "SELECT id, request_id, scenario_id, scenario_revision, input_hash, status, run_manifest_json, started_at, run_input_json FROM solve_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| {
                Ok(PersistedRunRow {
                    run_id: row.get(0)?,
                    request_id: row.get(1)?,
                    scenario_id: row.get(2)?,
                    scenario_revision: row.get(3)?,
                    request_hash: row.get(4)?,
                    status: row.get(5)?,
                    manifest_json: row.get(6)?,
                    started_at: row.get(7)?,
                    input_json: row.get(8)?,
                })
            },
        )
        .optional()?;
    let Some(PersistedRunRow {
        run_id: stored_run_id,
        request_id,
        scenario_id,
        scenario_revision,
        request_hash,
        status,
        manifest_json,
        started_at,
        input_json,
    }) = row
    else {
        return Err(StoreError::SolveRunNotFound(run_id));
    };
    let request_id = request_id.ok_or_else(|| {
        StoreError::InvalidPersistedRun("V2 solve run has no request ID".to_owned())
    })?;
    let input_json = input_json.ok_or_else(|| {
        StoreError::InvalidPersistedRun("V2 solve run has no run input".to_owned())
    })?;
    let input = parse_run_input(input_json.as_bytes())?;
    let stored_request_id: RequestId = request_id
        .parse()
        .map_err(|error| StoreError::InvalidPersistedRun(format!("invalid request ID: {error}")))?;
    let stored_scenario_id: ScenarioId = scenario_id.parse().map_err(|error| {
        StoreError::InvalidPersistedRun(format!("invalid scenario ID: {error}"))
    })?;
    let started_at = parse_timestamp(&started_at, "solve-run started_at")?;
    if stored_run_id != run_id.to_string()
        || input.run_id != run_id
        || input.request_id != stored_request_id
        || input.scenario_id != stored_scenario_id
        || input.scenario_revision != i64_to_u64(scenario_revision)?
        || input.request_hash != request_hash
        || input.snapshot_created_at > started_at
    {
        return Err(StoreError::InvalidPersistedRun(
            "run columns disagree with the typed input".to_owned(),
        ));
    }
    Ok((input, status, manifest_json, started_at))
}

fn load_solve_input_row(
    connection: &Connection,
    run_id: SolveRunId,
) -> Result<LoadedSolveInputV1, StoreError> {
    let (input, _, _, _) = load_run_input_record(connection, run_id)?;
    let row: Option<(String, i64, Vec<u8>, String)> = connection
        .query_row(
            "SELECT scenario_id, revision, document_json_zstd, created_at FROM scenario_snapshots WHERE id = ?1",
            [input.snapshot_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((scenario_id, revision, compressed, snapshot_created_at)) = row else {
        return Err(StoreError::SnapshotMismatch(input.snapshot_id));
    };
    let document_bytes = bounded_decompress_snapshot(&compressed)?;
    let document_value: Value = serde_json::from_slice(&document_bytes)
        .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))?;
    validate_stored_portable_json(&document_value, "solve snapshot document")
        .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))?;
    let document: ScenarioDocument = serde_json::from_value(document_value)
        .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))?;
    let snapshot_created_at = parse_timestamp(&snapshot_created_at, "snapshot created_at")?;
    if scenario_id != input.scenario_id.to_string()
        || i64_to_u64(revision)? != input.scenario_revision
        || document.scenario_id != input.scenario_id
        || blake3::hash(&document_bytes).to_hex().to_string() != input.snapshot_document_hash
        || document.domain_pack.id != input.pack_id
        || document.domain_pack.schema_version != input.pack_schema_version
        || document.settings.time_zone != input.scenario_timezone
        || snapshot_created_at != input.snapshot_created_at
        || serde_json::to_vec(&document)? != document_bytes
    {
        return Err(StoreError::SnapshotMismatch(input.snapshot_id));
    }
    Ok(LoadedSolveInputV1 { input, document })
}

fn serialize_checked_json<T: Serialize>(value: &T, context: &str) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        > eutheto_export::PORTABLE_LIMITS.max_json_bytes
    {
        return Err(format!("{context} exceeds the JSON byte limit"));
    }
    let parsed: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    validate_nonsecret_portable_json(
        &parsed,
        &PortableJsonLimits {
            max_depth: eutheto_export::PORTABLE_LIMITS.max_json_depth,
            max_string_bytes: eutheto_export::PORTABLE_LIMITS.max_string_bytes,
            max_collection_items: eutheto_export::PORTABLE_LIMITS.max_collection_items,
        },
    )
    .map_err(|error| format!("{context} violates the nonsecret policy: {error}"))?;
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

fn terminal_status(outcome: &RunTerminalOutcomeV1) -> Result<&'static str, StoreError> {
    match outcome {
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Optimal,
            ..
        } => Ok("optimal"),
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            ..
        } => Ok("feasible"),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible,
        } => Ok("infeasible"),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Unbounded,
        } => Ok("unbounded"),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::NoSolutionWithinLimit,
        } => Ok("no_solution_within_limit"),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Cancelled,
        } => Ok("cancelled"),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::InvalidModel,
        } => Ok("invalid_model"),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::BackendUnavailable,
        } => Ok("backend_unavailable"),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::BackendFailed,
        } => Ok("backend_failed"),
        RunTerminalOutcomeV1::VerificationAlarm { .. } => Ok("quarantined"),
        RunTerminalOutcomeV1::Interrupted => Ok("interrupted"),
        RunTerminalOutcomeV1::Accepted { .. } | RunTerminalOutcomeV1::NoResult { .. } => {
            Err(StoreError::InvalidPersistedRun(
                "terminal outcome contains an invalid status".to_owned(),
            ))
        }
    }
}

fn solve_deadline(input: &RunInputV1, started_at: Rfc3339Timestamp) -> Option<jiff::Timestamp> {
    started_at
        .as_timestamp()
        .checked_add(std::time::Duration::from_millis(
            input.solve_options.time_limit_milliseconds.value(),
        ))
        .ok()
}

fn solve_recovery_deadline(
    input: &RunInputV1,
    started_at: Rfc3339Timestamp,
) -> Option<jiff::Timestamp> {
    solve_deadline(input, started_at).and_then(|deadline| {
        deadline
            .checked_add(std::time::Duration::from_millis(
                SOLVE_TERMINAL_PERSISTENCE_GRACE_MILLISECONDS,
            ))
            .ok()
    })
}

fn validate_terminal_manifest(
    connection: &Connection,
    manifest: &RunManifestV1,
) -> Result<RunInputV1, StoreError> {
    manifest
        .validate()
        .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))?;
    let (input, status, stored_manifest, started_at) =
        load_run_input_record(connection, manifest.run_id)?;
    if status != "running" || stored_manifest.is_some() {
        return Err(StoreError::SolveRunTerminalConflict(manifest.run_id));
    }
    if input.checksum != manifest.run_input_checksum || started_at != manifest.started_at {
        return Err(StoreError::InvalidPersistedRun(
            "terminal manifest does not bind the stored run input".to_owned(),
        ));
    }
    Ok(input)
}

fn reject_counterfactual_owned_run(
    connection: &Connection,
    run_id: SolveRunId,
) -> Result<(), StoreError> {
    let job_id: Option<String> = connection
        .query_row(
            "SELECT id FROM counterfactual_jobs WHERE derived_run_id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(job_id) = job_id {
        let job_id = job_id.parse().map_err(|error| {
            invalid_counterfactual(format!("invalid owning job identity: {error}"))
        })?;
        return Err(StoreError::CounterfactualTransitionConflict(job_id));
    }
    Ok(())
}

fn finalize_accepted_run_transaction(
    connection: &mut Connection,
    accepted_result: &AcceptedResult,
    manifest: &RunManifestV1,
    evidence: &BTreeMap<DomainEvidenceId, VerificationValue>,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    reject_counterfactual_owned_run(&transaction, manifest.run_id)?;
    finalize_accepted_run_in_transaction(
        &transaction,
        accepted_result,
        manifest,
        evidence,
        #[cfg(debug_assertions)]
        failpoint,
    )?;
    transaction.commit()?;
    Ok(())
}

fn finalize_accepted_run_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    accepted_result: &AcceptedResult,
    manifest: &RunManifestV1,
    evidence: &BTreeMap<DomainEvidenceId, VerificationValue>,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<(), StoreError> {
    let input = validate_terminal_manifest(transaction, manifest)?;
    if !matches!(&manifest.outcome, RunTerminalOutcomeV1::Accepted { .. }) {
        return Err(StoreError::InvalidPersistedResult(
            "accepted finalization requires an accepted manifest".to_owned(),
        ));
    }
    let deadline = solve_deadline(&input, manifest.started_at);
    let elapsed = manifest.elapsed_milliseconds.ok_or_else(|| {
        StoreError::InvalidPersistedResult(
            "accepted finalization requires a measured elapsed duration".to_owned(),
        )
    })?;
    if deadline.is_some_and(|deadline| manifest.finished_at.as_timestamp() > deadline)
        || elapsed > input.solve_options.time_limit_milliseconds
    {
        return Err(StoreError::InvalidPersistedResult(
            "accepted finalization exceeds the immutable solve deadline".to_owned(),
        ));
    }
    if solve_recovery_deadline(&input, manifest.started_at)
        .is_some_and(|deadline| jiff::Timestamp::now() >= deadline)
    {
        return Err(StoreError::InvalidPersistedResult(
            "accepted finalization is past the terminal persistence cutoff".to_owned(),
        ));
    }
    let wrapper = PortableAcceptedResultV2::new(
        input,
        manifest.clone(),
        accepted_result.clone(),
        evidence.clone(),
    )
    .map_err(|error| StoreError::InvalidPersistedResult(error.to_string()))?;
    let manifest_json = serialize_checked_json(manifest, "run manifest")
        .map_err(StoreError::InvalidPersistedRun)?;
    let solution_json = serialize_checked_json(&accepted_result.solution, "normalized solution")
        .map_err(StoreError::InvalidPersistedResult)?;
    let report_json = serialize_checked_json(&accepted_result.verification, "verification report")
        .map_err(StoreError::InvalidPersistedResult)?;
    let score_json = serialize_checked_json(&accepted_result.verification.score, "score")
        .map_err(StoreError::InvalidPersistedResult)?;
    let evidence_json = serialize_checked_json(evidence, "result evidence")
        .map_err(StoreError::InvalidPersistedResult)?;
    wrapper
        .validate()
        .map_err(|error| StoreError::InvalidPersistedResult(error.to_string()))?;
    transaction.execute(
        "INSERT INTO solutions (id, solve_run_id, scenario_id, scenario_revision, status, accepted, normalized_solution_json, score_json, verification_report_json, evidence_json, created_at) VALUES (?1, ?2, ?3, ?4, 'verified', 1, ?5, ?6, ?7, ?8, ?9)",
        params![
            accepted_result.solution.solution_id.to_string(),
            manifest.run_id.to_string(),
            accepted_result.solution.scenario_id.to_string(),
            u64_to_i64(accepted_result.solution.scenario_revision)?,
            solution_json,
            score_json,
            report_json,
            evidence_json,
            manifest.finished_at.to_string(),
        ],
    )?;
    #[cfg(debug_assertions)]
    consume_failpoint(failpoint, Failpoint::AfterAcceptedSolutionInsert)?;
    let changed = transaction.execute(
        "UPDATE solve_runs SET status = ?2, finished_at = ?3, elapsed_ms = ?4, run_manifest_json = ?5, backend_diagnostics_json = NULL, error_json = NULL WHERE id = ?1 AND status = 'running' AND run_manifest_json IS NULL",
        params![
            manifest.run_id.to_string(),
            terminal_status(&manifest.outcome)?,
            manifest.finished_at.to_string(),
            manifest
                .elapsed_milliseconds
                .map(|elapsed| u64_to_i64(elapsed.value()))
                .transpose()?,
            manifest_json,
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::SolveRunTerminalConflict(manifest.run_id));
    }
    validate_global_identity_ownership(transaction)?;
    increment_library_revision(transaction)?;
    Ok(())
}

fn contains_unsafe_diagnostic_text(text: &str) -> bool {
    text.chars()
        .any(|character| matches!(character, '\n' | '\r' | '/' | '\\'))
        || text.contains("://")
        || text.split_whitespace().any(|part| {
            part.contains('.')
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        })
}

fn validate_candidate_diagnostics(
    diagnostics: &CandidateDiagnosticsV1,
) -> Result<String, StoreError> {
    if diagnostics.values.len() > eutheto_domain_ir::MAX_PORTABLE_RESULT_EVIDENCE_RECORDS {
        return Err(StoreError::InvalidPersistedDiagnostic(
            "candidate diagnostics exceed the record limit".to_owned(),
        ));
    }
    for (key, value) in &diagnostics.values {
        if key.is_empty()
            || key.len() > eutheto_domain_ir::MAX_RUN_DIAGNOSTIC_CODE_BYTES
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(StoreError::InvalidPersistedDiagnostic(
                "candidate diagnostic identity is invalid".to_owned(),
            ));
        }
        if let SafeDiagnosticValue::Text(text) = value
            && contains_unsafe_diagnostic_text(text)
        {
            return Err(StoreError::InvalidPersistedDiagnostic(
                "candidate diagnostics contain log, domain, or path-like text".to_owned(),
            ));
        }
    }
    serialize_checked_json(diagnostics, "candidate diagnostics")
        .map_err(StoreError::InvalidPersistedDiagnostic)
}

fn finalize_quarantined_run_transaction(
    connection: &mut Connection,
    manifest: &RunManifestV1,
    diagnostics: &CandidateDiagnosticsV1,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    reject_counterfactual_owned_run(&transaction, manifest.run_id)?;
    finalize_quarantined_run_in_transaction(
        &transaction,
        manifest,
        diagnostics,
        #[cfg(debug_assertions)]
        failpoint,
    )?;
    transaction.commit()?;
    Ok(())
}

fn finalize_quarantined_run_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    manifest: &RunManifestV1,
    diagnostics: &CandidateDiagnosticsV1,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<(), StoreError> {
    validate_terminal_manifest(transaction, manifest)?;
    let RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code } = &manifest.outcome else {
        return Err(StoreError::InvalidPersistedDiagnostic(
            "quarantine requires a verification-alarm manifest".to_owned(),
        ));
    };
    let diagnostics_json = validate_candidate_diagnostics(diagnostics)?;
    let error_json = serialize_checked_json(
        &serde_json::json!({"code": diagnostic_code}),
        "verification alarm",
    )
    .map_err(StoreError::InvalidPersistedDiagnostic)?;
    let manifest_json = serialize_checked_json(manifest, "run manifest")
        .map_err(StoreError::InvalidPersistedRun)?;
    let changed = transaction.execute(
        "UPDATE solve_runs SET status = 'quarantined', finished_at = ?2, elapsed_ms = ?3, run_manifest_json = ?4, backend_diagnostics_json = ?5, error_json = ?6 WHERE id = ?1 AND status = 'running' AND run_manifest_json IS NULL",
        params![
            manifest.run_id.to_string(),
            manifest.finished_at.to_string(),
            manifest
                .elapsed_milliseconds
                .map(|elapsed| u64_to_i64(elapsed.value()))
                .transpose()?,
            manifest_json,
            diagnostics_json,
            error_json,
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::SolveRunTerminalConflict(manifest.run_id));
    }
    synchronize_retained_scenario_revisions(transaction, Vec::new())?;
    #[cfg(debug_assertions)]
    consume_failpoint(failpoint, Failpoint::AfterQuarantineWrite)?;
    Ok(())
}

fn finalize_terminal_run_transaction(
    connection: &mut Connection,
    manifest: &RunManifestV1,
) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    reject_counterfactual_owned_run(&transaction, manifest.run_id)?;
    finalize_terminal_run_in_transaction(&transaction, manifest)?;
    transaction.commit()?;
    Ok(())
}

fn finalize_terminal_run_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    manifest: &RunManifestV1,
) -> Result<(), StoreError> {
    validate_terminal_manifest(transaction, manifest)?;
    if !matches!(
        &manifest.outcome,
        RunTerminalOutcomeV1::NoResult { .. } | RunTerminalOutcomeV1::Interrupted
    ) {
        return Err(StoreError::InvalidPersistedRun(
            "generic finalization permits only no-result or interrupted outcomes".to_owned(),
        ));
    }
    let manifest_json = serialize_checked_json(manifest, "run manifest")
        .map_err(StoreError::InvalidPersistedRun)?;
    let changed = transaction.execute(
        "UPDATE solve_runs SET status = ?2, finished_at = ?3, elapsed_ms = ?4, run_manifest_json = ?5, backend_diagnostics_json = NULL, error_json = NULL WHERE id = ?1 AND status = 'running' AND run_manifest_json IS NULL",
        params![
            manifest.run_id.to_string(),
            terminal_status(&manifest.outcome)?,
            manifest.finished_at.to_string(),
            manifest
                .elapsed_milliseconds
                .map(|elapsed| u64_to_i64(elapsed.value()))
                .transpose()?,
            manifest_json,
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::SolveRunTerminalConflict(manifest.run_id));
    }
    synchronize_retained_scenario_revisions(transaction, Vec::new())?;
    Ok(())
}

struct PersistedCounterfactualRow {
    id: String,
    request_id: String,
    request_hash: String,
    cancel_request_id: Option<String>,
    scenario_id: String,
    scenario_revision: i64,
    snapshot_id: String,
    base_solution_id: String,
    base_result_checksum: String,
    condition_json: String,
    total_budget_ms: i64,
    state: String,
    cancel_requested_at: Option<String>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    result_json: Option<String>,
    evidence_json: Option<String>,
    error_json: Option<String>,
    request_json: String,
}

fn invalid_counterfactual(message: impl Into<String>) -> StoreError {
    StoreError::InvalidPersistedCounterfactual(message.into())
}

fn parse_counterfactual_state(value: &str) -> Result<CounterfactualJobState, StoreError> {
    match value {
        "queued" => Ok(CounterfactualJobState::Queued),
        "running" => Ok(CounterfactualJobState::Running),
        "completed" => Ok(CounterfactualJobState::Completed),
        "failed" => Ok(CounterfactualJobState::Failed),
        "cancelled" => Ok(CounterfactualJobState::Cancelled),
        "interrupted" => Ok(CounterfactualJobState::Interrupted),
        _ => Err(invalid_counterfactual("invalid lifecycle state")),
    }
}

fn counterfactual_state_name(state: CounterfactualJobState) -> &'static str {
    match state {
        CounterfactualJobState::Queued => "queued",
        CounterfactualJobState::Running => "running",
        CounterfactualJobState::Completed => "completed",
        CounterfactualJobState::Failed => "failed",
        CounterfactualJobState::Cancelled => "cancelled",
        CounterfactualJobState::Interrupted => "interrupted",
    }
}

fn parse_checked_json<T: DeserializeOwned>(bytes: &[u8], context: &str) -> Result<T, StoreError> {
    if u64::try_from(bytes.len()).map_err(|_| StoreError::NumericRange)?
        > eutheto_export::PORTABLE_LIMITS.max_json_bytes
    {
        return Err(invalid_counterfactual(format!(
            "{context} exceeds the JSON byte limit"
        )));
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| invalid_counterfactual(error.to_string()))?;
    validate_stored_portable_json(&value, context)
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    serde_json::from_value(value).map_err(|error| invalid_counterfactual(error.to_string()))
}

fn parse_counterfactual_request(bytes: &[u8]) -> Result<CounterfactualJobRequestV1, StoreError> {
    if u64::try_from(bytes.len()).map_err(|_| StoreError::NumericRange)?
        > eutheto_export::PORTABLE_LIMITS.max_json_bytes
    {
        return Err(invalid_counterfactual(
            "counterfactual request exceeds the JSON byte limit",
        ));
    }
    CounterfactualJobRequestV1::from_json(bytes)
        .map_err(|error| invalid_counterfactual(error.to_string()))
}

fn parse_counterfactual_result(bytes: &[u8]) -> Result<CounterfactualResultV1, StoreError> {
    if u64::try_from(bytes.len()).map_err(|_| StoreError::NumericRange)?
        > eutheto_export::PORTABLE_LIMITS.max_json_bytes
    {
        return Err(invalid_counterfactual(
            "counterfactual result exceeds the JSON byte limit",
        ));
    }
    CounterfactualResultV1::from_json(bytes)
        .map_err(|error| invalid_counterfactual(error.to_string()))
}

#[allow(clippy::too_many_lines)]
fn load_counterfactual_job_row(
    connection: &Connection,
    job_id: CounterfactualJobId,
) -> Result<CounterfactualJobRecordV1, StoreError> {
    let row = connection
        .query_row(
            "SELECT id, request_id, request_hash, cancel_request_id, scenario_id, scenario_revision, snapshot_id, base_solution_id, base_result_checksum, condition_json, total_budget_ms, state, cancel_requested_at, created_at, started_at, finished_at, result_json, evidence_json, error_json, request_json FROM counterfactual_jobs WHERE id = ?1",
            [job_id.to_string()],
            |row| {
                Ok(PersistedCounterfactualRow {
                    id: row.get(0)?,
                    request_id: row.get(1)?,
                    request_hash: row.get(2)?,
                    cancel_request_id: row.get(3)?,
                    scenario_id: row.get(4)?,
                    scenario_revision: row.get(5)?,
                    snapshot_id: row.get(6)?,
                    base_solution_id: row.get(7)?,
                    base_result_checksum: row.get(8)?,
                    condition_json: row.get(9)?,
                    total_budget_ms: row.get(10)?,
                    state: row.get(11)?,
                    cancel_requested_at: row.get(12)?,
                    created_at: row.get(13)?,
                    started_at: row.get(14)?,
                    finished_at: row.get(15)?,
                    result_json: row.get(16)?,
                    evidence_json: row.get(17)?,
                    error_json: row.get(18)?,
                    request_json: row.get(19)?,
                })
            },
        )
        .optional()?
        .ok_or(StoreError::CounterfactualJobNotFound(job_id))?;
    if row.request_json.is_empty() {
        return Err(invalid_counterfactual("counterfactual request is missing"));
    }
    if row.evidence_json.is_some() {
        return Err(invalid_counterfactual(
            "counterfactual evidence has no typed persistence contract",
        ));
    }
    let request = parse_counterfactual_request(row.request_json.as_bytes())?;
    let condition =
        eutheto_domain_ir::CounterfactualConditionV1::from_json(row.condition_json.as_bytes())
            .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let stored_job_id: CounterfactualJobId = row
        .id
        .parse()
        .map_err(|error| invalid_counterfactual(format!("invalid job identity: {error}")))?;
    let request_id: RequestId = row
        .request_id
        .parse()
        .map_err(|error| invalid_counterfactual(format!("invalid request identity: {error}")))?;
    let scenario_id: ScenarioId = row
        .scenario_id
        .parse()
        .map_err(|error| invalid_counterfactual(format!("invalid scenario identity: {error}")))?;
    let snapshot_id: ScenarioSnapshotId = row
        .snapshot_id
        .parse()
        .map_err(|error| invalid_counterfactual(format!("invalid snapshot identity: {error}")))?;
    let base_solution_id: SolutionId = row.base_solution_id.parse().map_err(|error| {
        invalid_counterfactual(format!("invalid base solution identity: {error}"))
    })?;
    let cancel_request_id = row
        .cancel_request_id
        .as_deref()
        .map(str::parse)
        .transpose()
        .map_err(|error| invalid_counterfactual(format!("invalid cancel identity: {error}")))?;
    let created_at = parse_timestamp(&row.created_at, "counterfactual created_at")
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let started_at = row
        .started_at
        .as_deref()
        .map(|value| parse_timestamp(value, "counterfactual started_at"))
        .transpose()
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let finished_at = row
        .finished_at
        .as_deref()
        .map(|value| parse_timestamp(value, "counterfactual finished_at"))
        .transpose()
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let cancel_requested_at = row
        .cancel_requested_at
        .as_deref()
        .map(|value| parse_timestamp(value, "counterfactual cancel_requested_at"))
        .transpose()
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let result = row
        .result_json
        .as_deref()
        .map(|value| parse_counterfactual_result(value.as_bytes()))
        .transpose()?;
    let error = row
        .error_json
        .as_deref()
        .map(|value| parse_checked_json(value.as_bytes(), "counterfactual error"))
        .transpose()?;
    let state = parse_counterfactual_state(&row.state)?;
    let record = CounterfactualJobRecordV1::new(
        request,
        state,
        started_at,
        finished_at,
        cancel_request_id,
        cancel_requested_at,
        result,
        error,
    )
    .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let semantics = &record.request.semantics;
    if stored_job_id != job_id
        || record.request.job_id != stored_job_id
        || record.request.request_id != request_id
        || record.request.request_hash != row.request_hash
        || semantics.scenario_id != scenario_id
        || semantics.scenario_revision != i64_to_u64(row.scenario_revision)?
        || semantics.snapshot_id != snapshot_id
        || semantics.base.solution_id != base_solution_id
        || semantics.base.result_checksum != row.base_result_checksum
        || record.request.condition != condition
        || semantics.total_budget_milliseconds.value() != i64_to_u64(row.total_budget_ms)?
        || record.request.created_at != created_at
        || counterfactual_state_name(record.state) != row.state
        || serialize_checked_json(&record.request, "counterfactual request")
            .map_err(invalid_counterfactual)?
            != row.request_json
        || serialize_checked_json(&record.request.condition, "counterfactual condition")
            .map_err(invalid_counterfactual)?
            != row.condition_json
        || record
            .result
            .as_ref()
            .map(|value| serialize_checked_json(value, "counterfactual result"))
            .transpose()
            .map_err(invalid_counterfactual)?
            != row.result_json
        || record
            .error
            .as_ref()
            .map(|value| serialize_checked_json(value, "counterfactual error"))
            .transpose()
            .map_err(invalid_counterfactual)?
            != row.error_json
    {
        return Err(invalid_counterfactual(
            "counterfactual columns disagree with the typed record",
        ));
    }
    if let Some(ownership) = load_counterfactual_run_ownership(connection, job_id)? {
        let (input, _, _, run_started_at) = load_run_input_record(connection, ownership.run_id)
            .map_err(|error| invalid_counterfactual(error.to_string()))?;
        if record.state == CounterfactualJobState::Queued
            || record.started_at != Some(run_started_at)
            || input.request_id != ownership.request_id
            || input.run_id == semantics.base_run_id
            || input.scenario_id != semantics.scenario_id
            || input.scenario_revision != semantics.scenario_revision
            || input.snapshot_id != semantics.snapshot_id
            || input.snapshot_document_hash != semantics.snapshot_document_hash
            || input.objective_policy_hash != semantics.objective_policy_hash
            || input.temporary_condition_hash.as_ref() != Some(&semantics.condition_checksum)
            || input.solve_options.time_limit_milliseconds != semantics.total_budget_milliseconds
        {
            return Err(invalid_counterfactual(
                "derived run ownership disagrees with counterfactual job",
            ));
        }
    }
    Ok(record)
}

fn invalid_result(message: impl Into<String>) -> StoreError {
    StoreError::InvalidPersistedResult(message.into())
}

fn parse_accepted_json<T: DeserializeOwned>(bytes: &[u8], context: &str) -> Result<T, StoreError> {
    if u64::try_from(bytes.len()).map_err(|_| StoreError::NumericRange)?
        > eutheto_export::PORTABLE_LIMITS.max_json_bytes
    {
        return Err(invalid_result(format!(
            "{context} exceeds the JSON byte limit"
        )));
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| invalid_result(error.to_string()))?;
    validate_stored_portable_json(&value, context)
        .map_err(|error| invalid_result(error.to_string()))?;
    serde_json::from_value(value).map_err(|error| invalid_result(error.to_string()))
}

struct PersistedAcceptedAuthorityRow {
    run_id: String,
    scenario_id: String,
    scenario_revision: i64,
    solution_json: String,
    score_json: String,
    report_json: String,
    evidence_json: Option<String>,
    created_at: String,
}

fn accepted_result_summary(
    portable: &PortableAcceptedResultV2,
    current_revision: Revision,
    selected: bool,
) -> Result<StoredAcceptedResultSummary, StoreError> {
    let scenario_revision = checked_revision(portable.scenario_revision)
        .map_err(|error| invalid_result(error.to_string()))?;
    if scenario_revision > current_revision {
        return Err(invalid_result(
            "accepted result revision is newer than its current scenario",
        ));
    }
    let &RunTerminalOutcomeV1::Accepted { status, .. } = &portable.run_manifest.outcome else {
        return Err(invalid_result(
            "accepted result does not have an accepted terminal outcome",
        ));
    };
    let result = AcceptedResultRefV1::from_result(&portable.accepted_result)
        .map_err(|error| invalid_result(error.to_string()))?;
    Ok(StoredAcceptedResultSummary {
        result,
        run_id: portable.run_input.run_id,
        scenario_id: portable.scenario_id,
        scenario_revision,
        current_revision,
        stale: scenario_revision != current_revision,
        selected,
        status,
        score: portable.accepted_result.verification.score.clone(),
        verification_checksum: portable.accepted_result.verification.checksum.clone(),
        finished_at: portable.run_manifest.finished_at,
    })
}

#[allow(clippy::too_many_lines)]
fn load_local_accepted_authority(
    connection: &Connection,
    solution_id: SolutionId,
) -> Result<PortableAcceptedResultV2, StoreError> {
    let row: Option<PersistedAcceptedAuthorityRow> = connection
        .query_row(
            "SELECT solve_run_id, scenario_id, scenario_revision, normalized_solution_json, score_json, verification_report_json, evidence_json, created_at FROM solutions WHERE id = ?1 AND accepted = 1 AND status = 'verified'",
            [solution_id.to_string()],
            |row| {
                Ok(PersistedAcceptedAuthorityRow {
                    run_id: row.get(0)?,
                    scenario_id: row.get(1)?,
                    scenario_revision: row.get(2)?,
                    solution_json: row.get(3)?,
                    score_json: row.get(4)?,
                    report_json: row.get(5)?,
                    evidence_json: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Err(StoreError::AcceptedResultNotFound(solution_id));
    };
    let run_id: SolveRunId = row
        .run_id
        .parse()
        .map_err(|error| invalid_result(format!("invalid solve-run identity: {error}")))?;
    let scenario_id: ScenarioId = row
        .scenario_id
        .parse()
        .map_err(|error| invalid_result(format!("invalid scenario identity: {error}")))?;
    let (input, status, manifest_json, started_at) = load_run_input_record(connection, run_id)
        .map_err(|error| invalid_result(error.to_string()))?;
    let manifest_json =
        manifest_json.ok_or_else(|| invalid_result("accepted run manifest is missing"))?;
    let manifest = RunManifestV1::from_json(manifest_json.as_bytes())
        .map_err(|error| invalid_result(error.to_string()))?;
    let solution = NormalizedSolution::from_json(row.solution_json.as_bytes())
        .map_err(|error| invalid_result(error.to_string()))?;
    let report = VerificationReport::from_json(row.report_json.as_bytes())
        .map_err(|error| invalid_result(error.to_string()))?;
    let accepted_result =
        AcceptedResult::new(solution, report).map_err(|error| invalid_result(error.to_string()))?;
    let score: ScoreVector = parse_accepted_json(row.score_json.as_bytes(), "accepted score")?;
    let evidence_json = row
        .evidence_json
        .ok_or_else(|| invalid_result("accepted result evidence is missing"))?;
    let evidence: BTreeMap<DomainEvidenceId, VerificationValue> =
        parse_accepted_json(evidence_json.as_bytes(), "accepted evidence")?;
    let created_at = Rfc3339Timestamp::parse(&row.created_at)
        .map_err(|error| invalid_result(format!("invalid accepted-result created_at: {error}")))?;
    let (run_finished_at, run_elapsed_milliseconds): (Option<String>, Option<i64>) = connection
        .query_row(
            "SELECT finished_at, elapsed_ms FROM solve_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| invalid_result("accepted run terminal columns are missing"))?;
    let run_finished_at =
        run_finished_at.ok_or_else(|| invalid_result("accepted run finished_at is missing"))?;
    let run_finished_at = Rfc3339Timestamp::parse(&run_finished_at)
        .map_err(|error| invalid_result(format!("invalid accepted-run finished_at: {error}")))?;
    let run_elapsed_milliseconds = run_elapsed_milliseconds
        .map(i64_to_u64)
        .transpose()
        .map_err(|error| invalid_result(error.to_string()))?;
    if accepted_result.solution.solution_id != solution_id
        || accepted_result.solution.scenario_id != scenario_id
        || accepted_result.solution.scenario_revision
            != i64_to_u64(row.scenario_revision)
                .map_err(|error| invalid_result(error.to_string()))?
        || accepted_result.verification.score != score
        || manifest.run_id != run_id
        || manifest.started_at != started_at
        || manifest.finished_at != created_at
        || manifest.finished_at != run_finished_at
        || manifest
            .elapsed_milliseconds
            .map(eutheto_types::DurationMillis::value)
            != run_elapsed_milliseconds
        || status
            != terminal_status(&manifest.outcome)
                .map_err(|error| invalid_result(error.to_string()))?
    {
        return Err(invalid_result(
            "accepted solution columns disagree with local authority",
        ));
    }
    PortableAcceptedResultV2::new(input, manifest, accepted_result, evidence)
        .map_err(|error| invalid_result(error.to_string()))
}

fn load_stored_accepted_result(
    connection: &Connection,
    solution_id: SolutionId,
) -> Result<StoredAcceptedResultV2, StoreError> {
    let portable = load_local_accepted_authority(connection, solution_id)?;
    let loaded = load_solve_input_row(connection, portable.run_input.run_id)
        .map_err(|error| invalid_result(error.to_string()))?;
    if loaded.input != portable.run_input {
        return Err(invalid_result("accepted run input changed while loading"));
    }
    Ok(StoredAcceptedResultV2 {
        document: loaded.document,
        portable,
    })
}

fn validate_counterfactual_base_authority(
    connection: &Connection,
    request: &CounterfactualJobRequestV1,
) -> Result<(), StoreError> {
    let semantics = &request.semantics;
    let base = load_stored_accepted_result(connection, semantics.base.solution_id)
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let base = &base.portable;
    if base.accepted_result.checksum != semantics.base.result_checksum
        || base.run_input.run_id != semantics.base_run_id
        || base.run_input.checksum != semantics.base_run_input_checksum
        || base.run_input.scenario_id != semantics.scenario_id
        || base.run_input.scenario_revision != semantics.scenario_revision
        || base.run_input.snapshot_id != semantics.snapshot_id
        || base.run_input.snapshot_document_hash != semantics.snapshot_document_hash
        || base.run_input.model_hash != semantics.base_model_hash
        || base.run_input.objective_policy_hash != semantics.objective_policy_hash
        || base.run_input.temporary_condition_hash.is_some()
    {
        return Err(invalid_counterfactual(
            "counterfactual request disagrees with local base authority",
        ));
    }
    Ok(())
}

fn validate_counterfactual_capacity(connection: &Connection) -> Result<(), StoreError> {
    let active_jobs: u32 = connection.query_row(
        "SELECT COUNT(*) FROM counterfactual_jobs WHERE state IN ('queued', 'running')",
        [],
        |row| row.get(0),
    )?;
    if active_jobs >= MAX_ACTIVE_COUNTERFACTUAL_JOBS {
        return Err(StoreError::CounterfactualCapacityExceeded {
            maximum: MAX_ACTIVE_COUNTERFACTUAL_JOBS,
        });
    }
    Ok(())
}

fn start_counterfactual_job_transaction(
    connection: &mut Connection,
    request: &CounterfactualJobRequestV1,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<StartedCounterfactualJobV1, StoreError> {
    request
        .validate()
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let request_json = serialize_checked_json(request, "counterfactual request")
        .map_err(invalid_counterfactual)?;
    let condition_json = serialize_checked_json(&request.condition, "counterfactual condition")
        .map_err(invalid_counterfactual)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing_job: Option<String> = transaction
        .query_row(
            "SELECT id FROM counterfactual_jobs WHERE request_id = ?1",
            [request.request_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing_job) = existing_job {
        let existing_job_id = existing_job
            .parse()
            .map_err(|error| invalid_counterfactual(format!("invalid job identity: {error}")))?;
        let record = load_counterfactual_job_row(&transaction, existing_job_id)?;
        if record.request.request_hash != request.request_hash
            || record.request.semantics != request.semantics
            || record.request.condition != request.condition
        {
            return Err(StoreError::CounterfactualRequestIdConflict {
                request_id: request.request_id,
            });
        }
        transaction.commit()?;
        return Ok(StartedCounterfactualJobV1 {
            record,
            reused: true,
        });
    }
    validate_counterfactual_capacity(&transaction)?;
    let job_collision: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM counterfactual_jobs WHERE id = ?1)",
        [request.job_id.to_string()],
        |row| row.get(0),
    )?;
    if job_collision {
        return Err(StoreError::CounterfactualJobCollision(request.job_id));
    }
    let occupied = authoritative_occupied_uuids(&transaction)?;
    for identity in [request.job_id.as_uuid(), request.request_id.as_uuid()] {
        if occupied.contains(&identity) {
            return Err(StoreError::IdentityCollision(identity));
        }
    }
    validate_counterfactual_base_authority(&transaction, request)?;
    let queued = CounterfactualJobRecordV1::new(
        request.clone(),
        CounterfactualJobState::Queued,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let semantics = &request.semantics;
    transaction.execute(
        "INSERT INTO counterfactual_jobs (id, request_id, request_hash, cancel_request_id, scenario_id, scenario_revision, snapshot_id, base_solution_id, base_result_checksum, condition_json, total_budget_ms, state, cancel_requested_at, created_at, started_at, finished_at, result_json, evidence_json, error_json, request_json) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'queued', NULL, ?11, NULL, NULL, NULL, NULL, NULL, ?12)",
        params![
            request.job_id.to_string(),
            request.request_id.to_string(),
            request.request_hash,
            semantics.scenario_id.to_string(),
            u64_to_i64(semantics.scenario_revision)?,
            semantics.snapshot_id.to_string(),
            semantics.base.solution_id.to_string(),
            semantics.base.result_checksum,
            condition_json,
            u64_to_i64(semantics.total_budget_milliseconds.value())?,
            request.created_at.to_string(),
            request_json,
        ],
    )?;
    let stored = load_counterfactual_job_row(&transaction, request.job_id)?;
    if stored != queued {
        return Err(invalid_counterfactual(
            "inserted counterfactual record changed before commit",
        ));
    }
    validate_global_identity_ownership(&transaction)?;
    #[cfg(debug_assertions)]
    consume_failpoint(failpoint, Failpoint::AfterCounterfactualJobInsert)?;
    transaction.commit()?;
    Ok(StartedCounterfactualJobV1 {
        record: stored,
        reused: false,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CounterfactualRunOwnership {
    run_id: SolveRunId,
    request_id: RequestId,
}

fn load_counterfactual_run_ownership(
    connection: &Connection,
    job_id: CounterfactualJobId,
) -> Result<Option<CounterfactualRunOwnership>, StoreError> {
    let row: Option<(Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT derived_run_id, derived_request_id FROM counterfactual_jobs WHERE id = ?1",
            [job_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((run_id, request_id)) = row else {
        return Err(StoreError::CounterfactualJobNotFound(job_id));
    };
    let (run_id, request_id) = match (run_id, request_id) {
        (None, None) => return Ok(None),
        (Some(run_id), Some(request_id)) => (run_id, request_id),
        _ => {
            return Err(invalid_counterfactual(
                "derived run ownership is only partially persisted",
            ));
        }
    };
    let run_id: SolveRunId = run_id.parse().map_err(|error| {
        invalid_counterfactual(format!("invalid derived run identity: {error}"))
    })?;
    let request_id: RequestId = request_id.parse().map_err(|error| {
        invalid_counterfactual(format!("invalid derived request identity: {error}"))
    })?;
    let (input, _, _, _) = load_run_input_record(connection, run_id)
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    if input.request_id != request_id {
        return Err(invalid_counterfactual(
            "derived run ownership disagrees with solve authority",
        ));
    }
    Ok(Some(CounterfactualRunOwnership { run_id, request_id }))
}

fn validate_counterfactual_derived_input(
    connection: &Connection,
    current: &CounterfactualJobRecordV1,
    input: &RunInputV1,
    started_at: Rfc3339Timestamp,
) -> Result<(), StoreError> {
    let semantics = &current.request.semantics;
    let job_started_at = current
        .started_at
        .ok_or_else(|| invalid_counterfactual("running job has no start time"))?;
    let base = load_stored_accepted_result(connection, semantics.base.solution_id)
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let base_input = &base.portable.run_input;
    let mut expected_options = base_input.solve_options.clone();
    expected_options.backend = BackendSelection::Specific(base_input.backend_id.clone());
    expected_options.time_limit_milliseconds = semantics.total_budget_milliseconds;
    if started_at != job_started_at
        || input.run_id == semantics.base_run_id
        || input.scenario_id != semantics.scenario_id
        || input.scenario_revision != semantics.scenario_revision
        || input.snapshot_id != semantics.snapshot_id
        || input.snapshot_document_hash != semantics.snapshot_document_hash
        || input.pack_id != base_input.pack_id
        || input.pack_schema_version != base_input.pack_schema_version
        || input.planning_ir_schema_version != base_input.planning_ir_schema_version
        || input.compiler_version != base_input.compiler_version
        || input.backend_id != base_input.backend_id
        || input.backend_version != base_input.backend_version
        || input.adapter_version != base_input.adapter_version
        || input.worker_version != base_input.worker_version
        || input.solver_version != base_input.solver_version
        || input.protocol_major != base_input.protocol_major
        || input.protocol_minor != base_input.protocol_minor
        || input.objective_policy_hash != semantics.objective_policy_hash
        || input.temporary_condition_hash.as_ref() != Some(&semantics.condition_checksum)
        || input.solve_options != expected_options
    {
        return Err(invalid_counterfactual(
            "derived solve input disagrees with counterfactual authority",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn start_counterfactual_run_transaction(
    connection: &mut Connection,
    job_id: CounterfactualJobId,
    request: NewSolveRunV1,
    snapshot_policy: SnapshotPolicy,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<StartedSolveRunV1, StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_counterfactual_job_row(&transaction, job_id)?;
    if current.state != CounterfactualJobState::Running
        || current.cancel_request_id.is_some()
        || current.cancel_requested_at.is_some()
        || current.finished_at.is_some()
    {
        return Err(StoreError::CounterfactualTransitionConflict(job_id));
    }
    if request.scenario_id != current.request.semantics.scenario_id
        || request.expected_revision.value() != current.request.semantics.scenario_revision
        || request.started_at
            != current.started_at.ok_or_else(|| {
                invalid_counterfactual("running counterfactual job has no start time")
            })?
    {
        return Err(invalid_counterfactual(
            "derived solve request disagrees with counterfactual job",
        ));
    }
    if let Some(ownership) = load_counterfactual_run_ownership(&transaction, job_id)? {
        if ownership.run_id != request.run_id || ownership.request_id != request.request_id {
            return Err(StoreError::CounterfactualTransitionConflict(job_id));
        }
        let started = start_solve_run_in_transaction(
            &transaction,
            request,
            snapshot_policy,
            #[cfg(debug_assertions)]
            failpoint,
        )?;
        if started.input.run_id != ownership.run_id || !started.reused {
            return Err(invalid_counterfactual(
                "idempotent derived run binding changed",
            ));
        }
        let (_, status, stored_manifest, stored_started_at) =
            load_run_input_record(&transaction, ownership.run_id)?;
        if status != "running"
            || stored_manifest.is_some()
            || stored_started_at != started.started_at
        {
            return Err(StoreError::CounterfactualTransitionConflict(job_id));
        }
        validate_counterfactual_derived_input(
            &transaction,
            &current,
            &started.input,
            started.started_at,
        )?;
        transaction.commit()?;
        return Ok(started);
    }
    let started = start_solve_run_in_transaction(
        &transaction,
        request,
        snapshot_policy,
        #[cfg(debug_assertions)]
        failpoint,
    )?;
    if started.reused {
        return Err(StoreError::CounterfactualTransitionConflict(job_id));
    }
    validate_counterfactual_derived_input(
        &transaction,
        &current,
        &started.input,
        started.started_at,
    )?;
    let changed = transaction.execute(
        "UPDATE counterfactual_jobs SET derived_run_id = ?2, derived_request_id = ?3 WHERE id = ?1 AND state = 'running' AND cancel_request_id IS NULL AND cancel_requested_at IS NULL AND finished_at IS NULL AND derived_run_id IS NULL AND derived_request_id IS NULL",
        params![
            job_id.to_string(),
            started.input.run_id.to_string(),
            started.input.request_id.to_string(),
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::CounterfactualTransitionConflict(job_id));
    }
    let ownership = load_counterfactual_run_ownership(&transaction, job_id)?;
    if ownership
        != Some(CounterfactualRunOwnership {
            run_id: started.input.run_id,
            request_id: started.input.request_id,
        })
    {
        return Err(invalid_counterfactual(
            "persisted derived run ownership changed before commit",
        ));
    }
    transaction.commit()?;
    Ok(started)
}

fn load_local_terminal_run(
    connection: &Connection,
    run_id: SolveRunId,
) -> Result<(RunInputV1, RunManifestV1), StoreError> {
    let loaded = load_solve_input_row(connection, run_id)
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let (input, status, manifest_json, started_at) = load_run_input_record(connection, run_id)
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let manifest_json =
        manifest_json.ok_or_else(|| invalid_counterfactual("derived run is not terminal"))?;
    let manifest = RunManifestV1::from_json(manifest_json.as_bytes())
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    if loaded.input != input
        || manifest.run_id != run_id
        || manifest.run_input_checksum != input.checksum
        || manifest.started_at != started_at
        || status != terminal_status(&manifest.outcome)?
        || status == "quarantined"
    {
        return Err(invalid_counterfactual(
            "derived run columns disagree with local terminal authority",
        ));
    }
    Ok((input, manifest))
}

fn validate_counterfactual_completion_authority(
    connection: &Connection,
    result: &CounterfactualResultV1,
) -> Result<(), StoreError> {
    result
        .validate()
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let base = load_stored_accepted_result(connection, result.request.semantics.base.solution_id)
        .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let base = &base.portable;
    if base.run_input != result.base_run_input
        || base.run_manifest != result.base_run_manifest
        || base.accepted_result.solution.solution_id != result.request.semantics.base.solution_id
        || base.accepted_result.checksum != result.request.semantics.base.result_checksum
        || base.run_input.checksum != result.request.semantics.base_run_input_checksum
    {
        return Err(invalid_counterfactual(
            "counterfactual result disagrees with local base authority",
        ));
    }
    let (derived_input, derived_manifest) =
        load_local_terminal_run(connection, result.run_input.run_id)?;
    if derived_input != result.run_input || derived_manifest != result.run_manifest {
        return Err(invalid_counterfactual(
            "counterfactual result disagrees with local derived-run authority",
        ));
    }
    match &result.conclusion {
        CounterfactualConclusionV1::VerifiedAlternative {
            alternative,
            comparison,
            ..
        } => {
            let accepted = load_stored_accepted_result(connection, alternative.solution_id)
                .map_err(|error| invalid_counterfactual(error.to_string()))?;
            let accepted = &accepted.portable;
            if accepted.run_input != result.run_input
                || accepted.run_manifest != result.run_manifest
                || accepted.accepted_result.solution.solution_id != alternative.solution_id
                || accepted.accepted_result.checksum != alternative.result_checksum
            {
                return Err(invalid_counterfactual(
                    "counterfactual alternative disagrees with local acceptance authority",
                ));
            }
            if !counterfactual_condition_satisfied(
                &result.request.condition,
                &accepted.accepted_result.solution,
            ) {
                return Err(invalid_counterfactual(
                    "counterfactual alternative violates its requested condition",
                ));
            }
            let expected = compare_accepted_results(
                &base.accepted_result,
                &accepted.accepted_result,
                Some(&ComparisonContext {
                    locks: &[],
                    manifests: Some(ComparisonRunManifests {
                        base: &base.run_manifest,
                        candidate: &accepted.run_manifest,
                    }),
                }),
            )
            .map_err(|error| invalid_counterfactual(error.to_string()))?;
            if comparison.as_ref() != &expected {
                return Err(invalid_counterfactual(
                    "counterfactual comparison disagrees with local accepted results",
                ));
            }
        }
        CounterfactualConclusionV1::ProvenImpossible => {
            return Err(invalid_counterfactual(
                "counterfactual impossibility requires independent typed proof authority",
            ));
        }
        CounterfactualConclusionV1::NotDistinguishedWithinBudget => {
            let has_accepted_solution: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM solutions WHERE solve_run_id = ?1 AND accepted = 1 AND status = 'verified')",
                [result.run_input.run_id.to_string()],
                |row| row.get(0),
            )?;
            if has_accepted_solution {
                return Err(invalid_counterfactual(
                    "no-result counterfactual run has accepted-solution authority",
                ));
            }
        }
    }
    Ok(())
}
fn validate_counterfactual_normal_completion_timing(
    input: &RunInputV1,
    manifest: &RunManifestV1,
) -> Result<(), StoreError> {
    let elapsed = manifest.elapsed_milliseconds.ok_or_else(|| {
        StoreError::InvalidPersistedResult(
            "counterfactual completion requires a measured elapsed duration".to_owned(),
        )
    })?;
    if solve_deadline(input, manifest.started_at)
        .is_some_and(|deadline| manifest.finished_at.as_timestamp() > deadline)
        || elapsed > input.solve_options.time_limit_milliseconds
    {
        return Err(StoreError::InvalidPersistedResult(
            "counterfactual completion exceeds the immutable solve deadline".to_owned(),
        ));
    }
    if solve_recovery_deadline(input, manifest.started_at)
        .is_some_and(|deadline| jiff::Timestamp::now() >= deadline)
    {
        return Err(StoreError::InvalidPersistedResult(
            "counterfactual completion is past the terminal persistence cutoff".to_owned(),
        ));
    }
    Ok(())
}

fn finalize_counterfactual_run_transaction(
    connection: &mut Connection,
    job_id: CounterfactualJobId,
    finalization: &CounterfactualRunFinalizationV1,
    recover_cancelled_job: bool,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<CounterfactualJobRecordV1, StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let stored = finalize_counterfactual_run_in_transaction(
        &transaction,
        job_id,
        finalization,
        recover_cancelled_job,
        #[cfg(debug_assertions)]
        failpoint,
    )?;
    transaction.commit()?;
    Ok(stored)
}

#[allow(clippy::too_many_lines)]
fn finalize_counterfactual_run_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    job_id: CounterfactualJobId,
    finalization: &CounterfactualRunFinalizationV1,
    recover_cancelled_job: bool,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<CounterfactualJobRecordV1, StoreError> {
    let current = load_counterfactual_job_row(transaction, job_id)?;
    let ownership = load_counterfactual_run_ownership(transaction, job_id)?
        .ok_or(StoreError::CounterfactualTransitionConflict(job_id))?;
    if recover_cancelled_job
        && !matches!(
            finalization,
            CounterfactualRunFinalizationV1::Interrupted { .. }
        )
    {
        return Err(StoreError::CounterfactualTransitionConflict(job_id));
    }
    let manifest = match finalization {
        CounterfactualRunFinalizationV1::CompletedAccepted { manifest, .. }
        | CounterfactualRunFinalizationV1::CompletedNoResult { manifest, .. }
        | CounterfactualRunFinalizationV1::Quarantined { manifest, .. }
        | CounterfactualRunFinalizationV1::Failed { manifest, .. }
        | CounterfactualRunFinalizationV1::Cancelled { manifest }
        | CounterfactualRunFinalizationV1::Interrupted { manifest } => manifest,
    };
    if current.state != CounterfactualJobState::Running
        || current.finished_at.is_some()
        || manifest.run_id != ownership.run_id
    {
        return Err(StoreError::CounterfactualTransitionConflict(job_id));
    }
    let (input, status, stored_manifest, started_at) =
        load_run_input_record(transaction, ownership.run_id)
            .map_err(|error| invalid_counterfactual(error.to_string()))?;
    if input.request_id != ownership.request_id
        || status != "running"
        || stored_manifest.is_some()
        || manifest.started_at != started_at
    {
        return Err(StoreError::CounterfactualTransitionConflict(job_id));
    }
    validate_counterfactual_derived_input(transaction, &current, &input, started_at)?;

    let transition = match finalization {
        CounterfactualRunFinalizationV1::CompletedAccepted {
            accepted_result,
            evidence,
            manifest,
            result,
        } => {
            if current.cancel_request_id.is_some()
                || current.cancel_requested_at.is_some()
                || result.request != current.request
                || result.run_input != input
                || &result.run_manifest != manifest
                || !matches!(&manifest.outcome, RunTerminalOutcomeV1::Accepted { .. })
            {
                return Err(StoreError::CounterfactualTransitionConflict(job_id));
            }
            finalize_accepted_run_in_transaction(
                transaction,
                accepted_result,
                manifest,
                evidence,
                #[cfg(debug_assertions)]
                failpoint,
            )?;
            CounterfactualJobTransitionV1::Completed {
                result: result.clone(),
            }
        }
        CounterfactualRunFinalizationV1::CompletedNoResult { manifest, result } => {
            if current.cancel_request_id.is_some()
                || current.cancel_requested_at.is_some()
                || result.request != current.request
                || result.run_input != input
                || &result.run_manifest != manifest
                || !matches!(
                    &manifest.outcome,
                    RunTerminalOutcomeV1::NoResult {
                        status: SolveStatus::Infeasible | SolveStatus::NoSolutionWithinLimit
                    }
                )
            {
                return Err(StoreError::CounterfactualTransitionConflict(job_id));
            }
            validate_counterfactual_normal_completion_timing(&input, manifest)?;
            finalize_terminal_run_in_transaction(transaction, manifest)?;
            CounterfactualJobTransitionV1::Completed {
                result: result.clone(),
            }
        }
        CounterfactualRunFinalizationV1::Quarantined {
            manifest,
            candidate_diagnostics,
            error,
        } => {
            if current.cancel_request_id.is_some()
                || current.cancel_requested_at.is_some()
                || error.kind != CounterfactualFailureKind::InvalidCandidate
                || !matches!(
                    &manifest.outcome,
                    RunTerminalOutcomeV1::VerificationAlarm { .. }
                )
            {
                return Err(StoreError::CounterfactualTransitionConflict(job_id));
            }
            finalize_quarantined_run_in_transaction(
                transaction,
                manifest,
                candidate_diagnostics,
                #[cfg(debug_assertions)]
                failpoint,
            )?;
            CounterfactualJobTransitionV1::Failed {
                finished_at: manifest.finished_at,
                error: *error,
            }
        }
        CounterfactualRunFinalizationV1::Failed { manifest, error } => {
            let valid_failure = matches!(
                (&manifest.outcome, error.kind),
                (
                    RunTerminalOutcomeV1::NoResult {
                        status: SolveStatus::InvalidModel | SolveStatus::Unbounded
                    },
                    CounterfactualFailureKind::InvalidModel
                ) | (
                    RunTerminalOutcomeV1::NoResult {
                        status: SolveStatus::BackendUnavailable
                    },
                    CounterfactualFailureKind::BackendUnavailable
                ) | (
                    RunTerminalOutcomeV1::NoResult {
                        status: SolveStatus::BackendFailed
                    },
                    CounterfactualFailureKind::BackendFailed
                        | CounterfactualFailureKind::InvalidBinding
                )
            );
            if current.cancel_request_id.is_some()
                || current.cancel_requested_at.is_some()
                || !valid_failure
            {
                return Err(StoreError::CounterfactualTransitionConflict(job_id));
            }
            finalize_terminal_run_in_transaction(transaction, manifest)?;
            CounterfactualJobTransitionV1::Failed {
                finished_at: manifest.finished_at,
                error: *error,
            }
        }
        CounterfactualRunFinalizationV1::Cancelled { manifest } => {
            if current.cancel_request_id.is_none()
                || current.cancel_requested_at.is_none()
                || !matches!(
                    &manifest.outcome,
                    RunTerminalOutcomeV1::NoResult {
                        status: SolveStatus::Cancelled
                    }
                )
            {
                return Err(StoreError::CounterfactualTransitionConflict(job_id));
            }
            finalize_terminal_run_in_transaction(transaction, manifest)?;
            CounterfactualJobTransitionV1::Cancelled {
                finished_at: manifest.finished_at,
            }
        }
        CounterfactualRunFinalizationV1::Interrupted { manifest } => {
            let cancellation_marker_is_complete =
                current.cancel_request_id.is_some() && current.cancel_requested_at.is_some();
            if !matches!(&manifest.outcome, RunTerminalOutcomeV1::Interrupted)
                || (recover_cancelled_job && !cancellation_marker_is_complete)
                || (!recover_cancelled_job
                    && (current.cancel_request_id.is_some()
                        || current.cancel_requested_at.is_some()))
            {
                return Err(StoreError::CounterfactualTransitionConflict(job_id));
            }
            finalize_terminal_run_in_transaction(transaction, manifest)?;
            if recover_cancelled_job {
                CounterfactualJobTransitionV1::Cancelled {
                    finished_at: manifest.finished_at,
                }
            } else {
                CounterfactualJobTransitionV1::Interrupted {
                    finished_at: manifest.finished_at,
                }
            }
        }
    };
    let stored = transition_counterfactual_job_in_transaction(
        transaction,
        job_id,
        &transition,
        true,
        #[cfg(debug_assertions)]
        failpoint,
    )?;
    Ok(stored)
}

// Keeping the exhaustive lifecycle matrix together makes legal edges and idempotency auditable.
#[allow(clippy::too_many_lines)]
fn build_counterfactual_transition_target(
    connection: &Connection,
    current: &CounterfactualJobRecordV1,
    transition: &CounterfactualJobTransitionV1,
) -> Result<CounterfactualJobRecordV1, StoreError> {
    let target = match transition {
        CounterfactualJobTransitionV1::Running { started_at } => {
            if current.state == CounterfactualJobState::Running
                && current.started_at == Some(*started_at)
            {
                return Ok(current.clone());
            }
            if current.state != CounterfactualJobState::Queued {
                return Err(StoreError::CounterfactualTransitionConflict(
                    current.request.job_id,
                ));
            }
            CounterfactualJobRecordV1::new(
                current.request.clone(),
                CounterfactualJobState::Running,
                Some(*started_at),
                None,
                None,
                None,
                None,
                None,
            )
        }
        CounterfactualJobTransitionV1::Completed { result } => {
            if current.state != CounterfactualJobState::Running
                && current.state != CounterfactualJobState::Completed
            {
                return Err(StoreError::CounterfactualTransitionConflict(
                    current.request.job_id,
                ));
            }
            if current.cancel_request_id.is_some() {
                return Err(StoreError::CounterfactualTransitionConflict(
                    current.request.job_id,
                ));
            }
            validate_counterfactual_completion_authority(connection, result)?;
            CounterfactualJobRecordV1::new(
                current.request.clone(),
                CounterfactualJobState::Completed,
                current.started_at,
                Some(result.run_manifest.finished_at),
                None,
                None,
                Some(result.as_ref().clone()),
                None,
            )
        }
        CounterfactualJobTransitionV1::Failed { finished_at, error } => {
            if current.state != CounterfactualJobState::Queued
                && current.state != CounterfactualJobState::Running
                && current.state != CounterfactualJobState::Failed
            {
                return Err(StoreError::CounterfactualTransitionConflict(
                    current.request.job_id,
                ));
            }
            if current.state == CounterfactualJobState::Queued
                && !matches!(
                    error.kind,
                    CounterfactualFailureKind::BudgetExhausted
                        | CounterfactualFailureKind::StaleRevision
                        | CounterfactualFailureKind::BackendUnavailable
                        | CounterfactualFailureKind::InvalidModel
                        | CounterfactualFailureKind::CompilationFailed
                        | CounterfactualFailureKind::InvalidBinding
                )
            {
                return Err(StoreError::CounterfactualTransitionConflict(
                    current.request.job_id,
                ));
            }
            if current.cancel_request_id.is_some() {
                return Err(StoreError::CounterfactualTransitionConflict(
                    current.request.job_id,
                ));
            }
            CounterfactualJobRecordV1::new(
                current.request.clone(),
                CounterfactualJobState::Failed,
                current.started_at,
                Some(*finished_at),
                None,
                None,
                None,
                Some(*error),
            )
        }
        CounterfactualJobTransitionV1::Cancelled { finished_at } => {
            if current.state != CounterfactualJobState::Running
                && current.state != CounterfactualJobState::Cancelled
            {
                return Err(StoreError::CounterfactualTransitionConflict(
                    current.request.job_id,
                ));
            }
            if current.cancel_request_id.is_none() {
                return Err(StoreError::CounterfactualTransitionConflict(
                    current.request.job_id,
                ));
            }
            CounterfactualJobRecordV1::new(
                current.request.clone(),
                CounterfactualJobState::Cancelled,
                current.started_at,
                Some(*finished_at),
                current.cancel_request_id,
                current.cancel_requested_at,
                None,
                None,
            )
        }
        CounterfactualJobTransitionV1::Interrupted { finished_at } => {
            if current.state != CounterfactualJobState::Running
                && current.state != CounterfactualJobState::Interrupted
            {
                return Err(StoreError::CounterfactualTransitionConflict(
                    current.request.job_id,
                ));
            }
            if current.cancel_request_id.is_some() {
                return Err(StoreError::CounterfactualTransitionConflict(
                    current.request.job_id,
                ));
            }
            CounterfactualJobRecordV1::new(
                current.request.clone(),
                CounterfactualJobState::Interrupted,
                current.started_at,
                Some(*finished_at),
                None,
                None,
                None,
                None,
            )
        }
    }
    .map_err(|error| invalid_counterfactual(error.to_string()))?;
    if current.state == target.state {
        if current == &target {
            return Ok(current.clone());
        }
        return Err(StoreError::CounterfactualTransitionConflict(
            current.request.job_id,
        ));
    }
    Ok(target)
}

fn transition_counterfactual_job_transaction(
    connection: &mut Connection,
    job_id: CounterfactualJobId,
    transition: &CounterfactualJobTransitionV1,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<CounterfactualJobRecordV1, StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let stored = transition_counterfactual_job_in_transaction(
        &transaction,
        job_id,
        transition,
        false,
        #[cfg(debug_assertions)]
        failpoint,
    )?;
    transaction.commit()?;
    Ok(stored)
}

fn transition_counterfactual_job_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    job_id: CounterfactualJobId,
    transition: &CounterfactualJobTransitionV1,
    allow_derived_ownership: bool,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<CounterfactualJobRecordV1, StoreError> {
    let current = load_counterfactual_job_row(transaction, job_id)?;
    if !allow_derived_ownership
        && load_counterfactual_run_ownership(transaction, job_id)?.is_some()
        && !matches!(transition, CounterfactualJobTransitionV1::Running { .. })
    {
        return Err(StoreError::CounterfactualTransitionConflict(job_id));
    }
    let target = build_counterfactual_transition_target(transaction, &current, transition)?;
    if target == current {
        return Ok(current);
    }
    let result_json = target
        .result
        .as_ref()
        .map(|value| serialize_checked_json(value, "counterfactual result"))
        .transpose()
        .map_err(invalid_counterfactual)?;
    let error_json = target
        .error
        .as_ref()
        .map(|value| serialize_checked_json(value, "counterfactual error"))
        .transpose()
        .map_err(invalid_counterfactual)?;
    let changed = match target.state {
        CounterfactualJobState::Running
        | CounterfactualJobState::Completed
        | CounterfactualJobState::Failed
        | CounterfactualJobState::Interrupted => transaction.execute(
            "UPDATE counterfactual_jobs SET state = ?2, started_at = ?3, finished_at = ?4, result_json = ?5, evidence_json = NULL, error_json = ?6 WHERE id = ?1 AND state = ?7 AND cancel_request_id IS NULL AND cancel_requested_at IS NULL",
            params![
                job_id.to_string(),
                counterfactual_state_name(target.state),
                target.started_at.map(|value| value.to_string()),
                target.finished_at.map(|value| value.to_string()),
                result_json,
                error_json,
                counterfactual_state_name(current.state),
            ],
        )?,
        CounterfactualJobState::Cancelled => transaction.execute(
            "UPDATE counterfactual_jobs SET state = 'cancelled', started_at = ?2, finished_at = ?3, result_json = NULL, evidence_json = NULL, error_json = NULL WHERE id = ?1 AND state = 'running' AND cancel_request_id = ?4 AND cancel_requested_at = ?5",
            params![
                job_id.to_string(),
                target.started_at.map(|value| value.to_string()),
                target.finished_at.map(|value| value.to_string()),
                target.cancel_request_id.map(|value| value.to_string()),
                target.cancel_requested_at.map(|value| value.to_string()),
            ],
        )?,
        CounterfactualJobState::Queued => 0,
    };
    if changed != 1 {
        return Err(StoreError::CounterfactualTransitionConflict(job_id));
    }
    let stored = load_counterfactual_job_row(transaction, job_id)?;
    if stored != target {
        return Err(invalid_counterfactual(
            "transitioned counterfactual record changed before commit",
        ));
    }
    #[cfg(debug_assertions)]
    consume_failpoint(failpoint, Failpoint::AfterCounterfactualTransition)?;
    Ok(stored)
}

#[allow(clippy::too_many_lines)]
fn request_counterfactual_cancel_transaction(
    connection: &mut Connection,
    job_id: CounterfactualJobId,
    cancel_request_id: RequestId,
    requested_at: Rfc3339Timestamp,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<CounterfactualCancelOutcomeV1, StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_counterfactual_job_row(&transaction, job_id)?;
    if matches!(
        current.state,
        CounterfactualJobState::Completed
            | CounterfactualJobState::Failed
            | CounterfactualJobState::Interrupted
    ) {
        transaction.commit()?;
        return Ok(CounterfactualCancelOutcomeV1::AlreadyTerminal { record: current });
    }
    if let Some(recorded_id) = current.cancel_request_id {
        if recorded_id == cancel_request_id {
            transaction.commit()?;
            return Ok(CounterfactualCancelOutcomeV1::Requested {
                record: current,
                reused: true,
            });
        }
        return Err(StoreError::CounterfactualCancelRequestIdConflict {
            request_id: cancel_request_id,
        });
    }
    if current.state == CounterfactualJobState::Running
        && current
            .started_at
            .is_some_and(|started_at| requested_at < started_at)
    {
        return Err(invalid_counterfactual(
            "cancellation request precedes counterfactual start",
        ));
    }
    if authoritative_occupied_uuids(&transaction)?.contains(&cancel_request_id.as_uuid()) {
        return Err(StoreError::CounterfactualCancelRequestIdConflict {
            request_id: cancel_request_id,
        });
    }
    let target = match current.state {
        CounterfactualJobState::Queued => CounterfactualJobRecordV1::new(
            current.request.clone(),
            CounterfactualJobState::Cancelled,
            None,
            Some(requested_at),
            Some(cancel_request_id),
            Some(requested_at),
            None,
            None,
        ),
        CounterfactualJobState::Running => CounterfactualJobRecordV1::new(
            current.request.clone(),
            CounterfactualJobState::Running,
            current.started_at,
            None,
            Some(cancel_request_id),
            Some(requested_at),
            None,
            None,
        ),
        CounterfactualJobState::Cancelled => {
            return Err(StoreError::CounterfactualCancelRequestIdConflict {
                request_id: cancel_request_id,
            });
        }
        CounterfactualJobState::Completed
        | CounterfactualJobState::Failed
        | CounterfactualJobState::Interrupted => unreachable!(),
    }
    .map_err(|error| invalid_counterfactual(error.to_string()))?;
    let changed = match current.state {
        CounterfactualJobState::Queued => transaction.execute(
            "UPDATE counterfactual_jobs SET cancel_request_id = ?2, cancel_requested_at = ?3, state = 'cancelled', finished_at = ?3 WHERE id = ?1 AND state = 'queued' AND cancel_request_id IS NULL AND cancel_requested_at IS NULL AND started_at IS NULL AND finished_at IS NULL",
            params![
                job_id.to_string(),
                cancel_request_id.to_string(),
                requested_at.to_string()
            ],
        )?,
        CounterfactualJobState::Running => transaction.execute(
            "UPDATE counterfactual_jobs SET cancel_request_id = ?2, cancel_requested_at = ?3 WHERE id = ?1 AND state = 'running' AND cancel_request_id IS NULL AND cancel_requested_at IS NULL AND finished_at IS NULL",
            params![
                job_id.to_string(),
                cancel_request_id.to_string(),
                requested_at.to_string()
            ],
        )?,
        _ => 0,
    };
    if changed != 1 {
        return Err(StoreError::CounterfactualTransitionConflict(job_id));
    }
    let stored = load_counterfactual_job_row(&transaction, job_id)?;
    if stored != target {
        return Err(invalid_counterfactual(
            "cancelled counterfactual record changed before commit",
        ));
    }
    validate_global_identity_ownership(&transaction)?;
    #[cfg(debug_assertions)]
    consume_failpoint(failpoint, Failpoint::AfterCounterfactualCancelWrite)?;
    transaction.commit()?;
    Ok(CounterfactualCancelOutcomeV1::Requested {
        record: stored,
        reused: false,
    })
}

struct StagedApplyParts {
    import: StagedImport,
    remove_scenario_ids: BTreeSet<ScenarioId>,
    settings: BTreeMap<String, AppSetting<Value>>,
    authorization: Option<RestoreAuthorization>,
}

struct StagedScenarioOutcome {
    created: usize,
    replaced: usize,
    sources: Vec<ScenarioImportSource>,
}

#[derive(Clone, Copy)]
enum HistoryDirection {
    Undo,
    Redo,
}

struct PrivateFileGuard {
    file: File,
}

fn private_path_error(message: &'static str) -> StoreError {
    StoreError::PrivatePath(io::Error::other(message))
}

fn path_is_indirection(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    false
}

/// Creates or validates an owner-private application directory.
///
/// Every existing path component is checked for symlink/reparse indirection.
/// The requested directory is created with owner-only access when absent and
/// hardened and revalidated when present.
///
/// # Errors
///
/// Returns [`StoreError::PrivatePath`] when the path is root-like, indirect,
/// not a directory, has an unsafe ancestor, cannot be created, or cannot be
/// restricted to the current owner.
pub fn ensure_private_application_directory(path: impl AsRef<Path>) -> Result<(), StoreError> {
    ensure_private_parent(&path.as_ref().join(".eutheto-private-directory-guard"))
}

fn ensure_private_parent(path: &Path) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if parent.file_name().is_none() && parent.has_root() {
        return Err(private_path_error(
            "private storage requires a dedicated parent directory",
        ));
    }
    if path.file_name().is_none() {
        return Err(private_path_error(
            "private storage path requires a regular file name",
        ));
    }
    let mut current = PathBuf::new();
    let components = parent.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {
                if current.as_os_str().is_empty() {
                    current.push(".");
                }
            }
            Component::ParentDir => {
                return Err(private_path_error(
                    "private storage paths may not contain parent-directory components",
                ));
            }
            Component::Normal(name) => {
                current.push(name);
                match std::fs::symlink_metadata(&current) {
                    Ok(metadata) => {
                        if path_is_indirection(&metadata) || !metadata.is_dir() {
                            return Err(private_path_error(
                                "private storage path contains an indirect or non-directory component",
                            ));
                        }
                        if index + 1 != components.len() {
                            ensure_safe_ancestor_directory(&current, &metadata)?;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        create_private_directory(&current)?;
                        restrict_private_directory(&current)?;
                    }
                    Err(error) => return Err(StoreError::PrivatePath(error)),
                }
            }
        }
        if index + 1 == components.len() {
            restrict_private_directory(&current)?;
        }
    }
    if components.is_empty() {
        restrict_private_directory(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_safe_ancestor_directory(
    _path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = metadata.permissions().mode();
    if mode & 0o022 != 0 && mode & 0o1000 == 0 {
        return Err(private_path_error(
            "private storage path has an ambient writable ancestor",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_safe_ancestor_directory(
    path: &Path,
    _metadata: &std::fs::Metadata,
) -> Result<(), StoreError> {
    use std::process::{Command, Stdio};
    let script = r#"
$ErrorActionPreference = 'Stop'
$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
$allowed = [System.Collections.Generic.HashSet[string]]::new()
$allowed.Add($identity.User.Value) | Out-Null
$allowed.Add('S-1-5-18') | Out-Null
$allowed.Add('S-1-5-32-544') | Out-Null
$writeMask = [System.Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles
$acl = Get-Acl -LiteralPath $env:EUTHETO_PRIVATE_PATH
$rules = @($acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
foreach ($rule in $rules) {
  if (($rule.PropagationFlags -band [System.Security.AccessControl.PropagationFlags]::InheritOnly) -ne 0) {
    continue
  }
  if ($rule.AccessControlType -eq [System.Security.AccessControl.AccessControlType]::Allow -and
      -not $allowed.Contains($rule.IdentityReference.Value) -and
      (($rule.FileSystemRights -band $writeMask) -ne 0)) {
    throw "ambient writable ancestor '$($env:EUTHETO_PRIVATE_PATH)': SID=$($rule.IdentityReference.Value), rights=$($rule.FileSystemRights), inheritance=$($rule.InheritanceFlags), propagation=$($rule.PropagationFlags)"
  }
}
"#;
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env_remove("PSModulePath")
        .env("EUTHETO_PRIVATE_PATH", path)
        .stdin(Stdio::null())
        .output()
        .map_err(StoreError::PrivatePath)?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(StoreError::PrivatePath(io::Error::other(
            if detail.is_empty() {
                "private storage path has an ambient writable ancestor".to_owned()
            } else {
                detail
            },
        )));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn ensure_safe_ancestor_directory(
    _path: &Path,
    _metadata: &std::fs::Metadata,
) -> Result<(), StoreError> {
    Err(private_path_error(
        "private storage is unsupported on this platform",
    ))
}

fn prepare_private_database_path(path: &Path) -> Result<PrivateFileGuard, StoreError> {
    ensure_private_parent(path)?;
    for sidecar in sqlite_sidecar_paths(path) {
        match std::fs::symlink_metadata(&sidecar) {
            Ok(metadata) => {
                ensure_regular_direct_file(&metadata)?;
                #[cfg(windows)]
                ensure_path_has_no_windows_hard_links(&sidecar)?;
                restrict_private_file(&sidecar)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(StoreError::PrivatePath(error)),
        }
    }
    prepare_private_file(path, false)
}

fn prepare_private_new_file(path: &Path) -> Result<PrivateFileGuard, StoreError> {
    ensure_private_parent(path)?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure_regular_direct_file(&metadata)?;
            #[cfg(windows)]
            ensure_path_has_no_windows_hard_links(path)?;
            return Err(StoreError::BackupDestinationExists);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(StoreError::PrivatePath(error)),
    }
    prepare_private_file(path, true)
}

fn prepare_private_file(path: &Path, create_new: bool) -> Result<PrivateFileGuard, StoreError> {
    let mut options = FileOpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|error| {
        if create_new && error.kind() == io::ErrorKind::AlreadyExists {
            StoreError::BackupDestinationExists
        } else {
            StoreError::PrivatePath(error)
        }
    })?;
    let metadata = file.metadata().map_err(StoreError::PrivatePath)?;
    ensure_regular_direct_file(&metadata)?;
    #[cfg(windows)]
    ensure_path_has_no_windows_hard_links(path)?;
    restrict_private_file(path)?;
    ensure_same_file(&file, path)?;
    Ok(PrivateFileGuard { file })
}

fn remove_guarded_file(path: &Path, guard: &PrivateFileGuard) {
    if ensure_same_file(&guard.file, path).is_ok() {
        let _ignored = std::fs::remove_file(path);
    }
}

fn ensure_regular_direct_file(metadata: &std::fs::Metadata) -> Result<(), StoreError> {
    if path_is_indirection(metadata) || !metadata.is_file() {
        return Err(private_path_error(
            "private storage path is indirect or is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(private_path_error(
                "private storage files may not have additional hard links",
            ));
        }
    }
    Ok(())
}

fn verify_private_database_path(path: &Path, guard: &PrivateFileGuard) -> Result<(), StoreError> {
    ensure_same_file(&guard.file, path)?;
    restrict_private_file(path)?;
    for sidecar in sqlite_sidecar_paths(path) {
        match std::fs::symlink_metadata(&sidecar) {
            Ok(metadata) => {
                ensure_regular_direct_file(&metadata)?;
                restrict_private_file(&sidecar)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(StoreError::PrivatePath(error)),
        }
    }
    Ok(())
}

fn sqlite_sidecar_paths(path: &Path) -> [PathBuf; 3] {
    let raw = path.as_os_str().to_string_lossy();
    [
        PathBuf::from(format!("{raw}-wal")),
        PathBuf::from(format!("{raw}-shm")),
        PathBuf::from(format!("{raw}-journal")),
    ]
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path).map_err(StoreError::PrivatePath)
}

#[cfg(windows)]
fn create_private_directory(path: &Path) -> Result<(), StoreError> {
    use std::process::{Command, Stdio};
    let script = r#"
$ErrorActionPreference = 'Stop'
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = [System.Security.AccessControl.DirectorySecurity]::new()
$acl.SetAccessRuleProtection($true, $false)
$acl.SetOwner($sid)
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
  $sid,
  [System.Security.AccessControl.FileSystemRights]::FullControl,
  [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
  [System.Security.AccessControl.PropagationFlags]::None,
  [System.Security.AccessControl.AccessControlType]::Allow)
$acl.AddAccessRule($rule)
[System.IO.Directory]::CreateDirectory($env:EUTHETO_PRIVATE_PATH, $acl) | Out-Null
"#;
    let status = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env_remove("PSModulePath")
        .env("EUTHETO_PRIVATE_PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(StoreError::PrivatePath)?;
    if !status.success() {
        return Err(private_path_error(
            "owner-private directory creation failed",
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn create_private_directory(_path: &Path) -> Result<(), StoreError> {
    Err(private_path_error(
        "private storage is unsupported on this platform",
    ))
}

#[cfg(windows)]
fn ensure_path_has_no_windows_hard_links(path: &Path) -> Result<(), StoreError> {
    use std::process::{Command, Stdio};
    let script = r#"
$ErrorActionPreference = 'Stop'
$item = Get-Item -Force -LiteralPath $env:EUTHETO_PRIVATE_PATH
if ($item.LinkType -eq 'HardLink') {
  throw 'private storage files may not have additional hard links'
}
"#;
    let status = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env_remove("PSModulePath")
        .env("EUTHETO_PRIVATE_PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(StoreError::PrivatePath)?;
    if !status.success() {
        return Err(private_path_error(
            "private storage files may not have additional hard links",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_private_directory(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = std::fs::symlink_metadata(path).map_err(StoreError::PrivatePath)?;
    if path_is_indirection(&metadata)
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(private_path_error(
            "private storage directory ownership is invalid",
        ));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(StoreError::PrivatePath)?;
    let metadata = std::fs::symlink_metadata(path).map_err(StoreError::PrivatePath)?;
    if path_is_indirection(&metadata)
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(private_path_error(
            "private storage directory ownership or permissions are invalid",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_private_file(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = std::fs::symlink_metadata(path).map_err(StoreError::PrivatePath)?;
    ensure_regular_direct_file(&metadata)?;
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(private_path_error(
            "private storage file ownership is invalid",
        ));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(StoreError::PrivatePath)?;
    let metadata = std::fs::symlink_metadata(path).map_err(StoreError::PrivatePath)?;
    if path_is_indirection(&metadata)
        || !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(private_path_error(
            "private storage file ownership or permissions are invalid",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn restrict_private_directory(path: &Path) -> Result<(), StoreError> {
    restrict_windows_acl(path, true)
}

#[cfg(windows)]
fn restrict_private_file(path: &Path) -> Result<(), StoreError> {
    restrict_windows_acl(path, false)
}

#[cfg(windows)]
fn restrict_windows_acl(path: &Path, directory: bool) -> Result<(), StoreError> {
    use std::process::{Command, Stdio};
    let script = r#"
$ErrorActionPreference = 'Stop'
$path = $env:EUTHETO_PRIVATE_PATH
$isDirectory = $env:EUTHETO_PRIVATE_KIND -eq 'directory'
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$trustedOwners = [System.Collections.Generic.HashSet[string]]::new()
$trustedOwners.Add($sid.Value) | Out-Null
$trustedOwners.Add('S-1-5-18') | Out-Null
$trustedOwners.Add('S-1-5-32-544') | Out-Null
$item = Get-Item -Force -LiteralPath $path
if (-not $isDirectory -and $item.LinkType -eq 'HardLink') {
  throw 'private storage files may not have additional hard links'
}
$existingAcl = Get-Acl -LiteralPath $path
$owner = $existingAcl.GetOwner([System.Security.Principal.SecurityIdentifier])
if (-not $trustedOwners.Contains($owner.Value)) {
  throw 'private storage ownership is invalid'
}
if ($isDirectory) {
  $acl = [System.Security.AccessControl.DirectorySecurity]::new()
  $inheritance = [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
} else {
  $acl = [System.Security.AccessControl.FileSecurity]::new()
  $inheritance = [System.Security.AccessControl.InheritanceFlags]::None
}
$acl.SetAccessRuleProtection($true, $false)
$acl.SetOwner($sid)
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
  $sid,
  [System.Security.AccessControl.FileSystemRights]::FullControl,
  $inheritance,
  [System.Security.AccessControl.PropagationFlags]::None,
  [System.Security.AccessControl.AccessControlType]::Allow)
$acl.AddAccessRule($rule)
Set-Acl -LiteralPath $path -AclObject $acl
$verified = Get-Acl -LiteralPath $path
$rules = @($verified.GetAccessRules($true, $false, [System.Security.Principal.SecurityIdentifier]))
if (-not $verified.AreAccessRulesProtected -or $rules.Count -ne 1 -or
    $rules[0].IdentityReference -ne $sid -or
    $rules[0].AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow -or
    (($rules[0].FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -ne
      [System.Security.AccessControl.FileSystemRights]::FullControl)) {
  throw 'owner-private ACL verification failed'
}
"#;
    let status = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env_remove("PSModulePath")
        .env("EUTHETO_PRIVATE_PATH", path)
        .env(
            "EUTHETO_PRIVATE_KIND",
            if directory { "directory" } else { "file" },
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(StoreError::PrivatePath)?;
    if !status.success() {
        return Err(private_path_error(
            "current-user-only ACL creation or verification failed",
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn restrict_private_directory(_path: &Path) -> Result<(), StoreError> {
    Err(private_path_error(
        "private storage is unsupported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn restrict_private_file(_path: &Path) -> Result<(), StoreError> {
    Err(private_path_error(
        "private storage is unsupported on this platform",
    ))
}

fn ensure_same_file(file: &File, path: &Path) -> Result<(), StoreError> {
    let selected = std::fs::symlink_metadata(path).map_err(StoreError::PrivatePath)?;
    ensure_regular_direct_file(&selected)?;
    let opened = same_file::Handle::from_file(file.try_clone().map_err(StoreError::PrivatePath)?)
        .map_err(StoreError::PrivatePath)?;
    let selected = same_file::Handle::from_path(path).map_err(StoreError::PrivatePath)?;
    if opened != selected {
        return Err(private_path_error(
            "private storage file changed during validation",
        ));
    }
    Ok(())
}

fn preflight_schema_version(file: &File) -> Result<(), StoreError> {
    let mut file = file.try_clone().map_err(|error| {
        StoreError::Integrity(format!("database schema preflight failed: {error}"))
    })?;
    if file
        .metadata()
        .map_err(|error| {
            StoreError::Integrity(format!("database schema preflight failed: {error}"))
        })?
        .len()
        < 64
    {
        return Ok(());
    }
    let mut header = [0_u8; 64];
    file.read_exact(&mut header).map_err(|error| {
        StoreError::Integrity(format!("database schema preflight failed: {error}"))
    })?;
    if &header[..16] != b"SQLite format 3\0" {
        return Ok(());
    }
    let found = u32::from_be_bytes(header[60..64].try_into().map_err(|_| {
        StoreError::Integrity("database schema preflight header is invalid".to_owned())
    })?);
    if found > CURRENT_SCHEMA_VERSION {
        return Err(StoreError::NewerSchema {
            found,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    Ok(())
}

fn open_connection(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, SQLITE_LENGTH_LIMIT_BYTES)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    Ok(connection)
}

fn initialize_connection(
    connection: &mut Connection,
    database_path: &Path,
    authoritative_file: &File,
    #[cfg(debug_assertions)] options: &OpenOptions,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<InitializationOutcome, StoreError> {
    let (applied_migrations, retained_backup_path) = initialize_schema(
        connection,
        database_path,
        authoritative_file,
        #[cfg(debug_assertions)]
        options.v2_migration_begin_test_hook.as_ref(),
        #[cfg(debug_assertions)]
        options.v3_migration_begin_test_hook.as_ref(),
        #[cfg(debug_assertions)]
        options.v4_migration_begin_test_hook.as_ref(),
        #[cfg(debug_assertions)]
        failpoint,
    )?;

    let provenance_policy: String = connection
        .query_row(
            "SELECT value FROM app_metadata WHERE key = 'portable_import_provenance_retention_policy'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| {
            StoreError::Integrity(
                "portable import provenance retention policy is missing".to_owned(),
            )
        })?;
    if provenance_policy.parse::<u32>().ok() != Some(IMPORT_PROVENANCE_RETENTION_POLICY_VERSION) {
        return Err(StoreError::Integrity(
            "portable import provenance retention policy is unsupported".to_owned(),
        ));
    }
    initialize_scenario_revision_high_water(connection)?;
    initialize_scenario_identity_owners(connection)?;
    let quick_check: String =
        connection.pragma_query_value(None, "quick_check", |row| row.get(0))?;
    if quick_check != "ok" {
        return Err(StoreError::Integrity(quick_check));
    }
    let mut interrupted_solve_run_ids = recover_running_v2_runs(connection)?;
    let recovery_now = {
        #[cfg(debug_assertions)]
        {
            options.recovery_now.unwrap_or_else(jiff::Timestamp::now)
        }
        #[cfg(not(debug_assertions))]
        {
            jiff::Timestamp::now()
        }
    };
    let (recovered_counterfactual_job_ids, interrupted_counterfactual_run_ids) =
        recover_expired_counterfactual_jobs(connection, recovery_now)?;
    interrupted_solve_run_ids.extend(interrupted_counterfactual_run_ids);
    Ok(InitializationOutcome {
        schema_version: CURRENT_SCHEMA_VERSION,
        applied_migrations,
        integrity: IntegrityOutcome::Ok,
        retained_backup_path,
        recovery: RecoveryOutcome {
            interrupted_solve_run_ids,
            recovered_counterfactual_job_ids,
        },
    })
}

fn initialize_scenario_revision_high_water(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut statement = transaction.prepare(
        "SELECT scenario_id, MAX(revision) FROM (
            SELECT id AS scenario_id, revision FROM scenarios
            UNION ALL
            SELECT scenario_id, revision FROM retained_scenario_revisions
            UNION ALL
            SELECT scenario_id, revision FROM scenario_snapshots
            UNION ALL
            SELECT scenario_id, revision_after AS revision FROM command_journal
        ) GROUP BY scenario_id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (scenario_id, revision) in rows {
        let scenario_id = scenario_id.parse().map_err(|error| {
            StoreError::Integrity(format!(
                "invalid scenario identity while backfilling revision high-water: {error}"
            ))
        })?;
        record_scenario_revision_high_water(
            &transaction,
            scenario_id,
            checked_revision(i64_to_u64(revision)?)?,
        )?;
    }
    load_scenario_revision_high_water(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn initialize_scenario_identity_owners(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (scenario_id, _) in load_scenario_revision_high_water(&transaction)? {
        record_scenario_identity_owners(
            &transaction,
            scenario_id,
            &BTreeSet::from([scenario_id.as_uuid()]),
        )?;
    }
    for scenario_id in scenario_revisions(&transaction)?.keys().copied() {
        let project = load_project(&transaction, scenario_id)?;
        record_scenario_identity_owners(
            &transaction,
            scenario_id,
            &collect_project_owned_uuids(
                &project.document,
                &project.portable.semantic_extensions,
                &project.portable.extensions,
            ),
        )?;
    }
    for scenario in read_retained_scenario_revision_rows(&transaction)?.into_values() {
        record_scenario_identity_owners(
            &transaction,
            scenario.document.scenario_id,
            &collect_project_owned_uuids(
                &scenario.document,
                &scenario.semantic_extensions,
                &scenario.extensions,
            ),
        )?;
    }
    load_scenario_identity_owners(&transaction)?;
    transaction.commit()?;
    Ok(())
}

// Each version's under-lock reread, registry verification, SQL, and commit stay visibly coupled.
#[allow(clippy::too_many_lines)]
fn initialize_schema(
    connection: &mut Connection,
    database_path: &Path,
    authoritative_file: &File,
    #[cfg(debug_assertions)] v2_migration_begin_test_hook: Option<&V2MigrationBeginTestHook>,
    #[cfg(debug_assertions)] v3_migration_begin_test_hook: Option<&V3MigrationBeginTestHook>,
    #[cfg(debug_assertions)] v4_migration_begin_test_hook: Option<&V4MigrationBeginTestHook>,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
) -> Result<(Vec<u32>, Option<PathBuf>), StoreError> {
    let initial_version: u32 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if initial_version > CURRENT_SCHEMA_VERSION {
        return Err(StoreError::NewerSchema {
            found: initial_version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    let checksums = [
        blake3::hash(INITIAL_MIGRATION.as_bytes())
            .to_hex()
            .to_string(),
        blake3::hash(INDEPENDENT_VERIFICATION_MIGRATION.as_bytes())
            .to_hex()
            .to_string(),
        blake3::hash(COUNTERFACTUAL_JOB_REQUESTS_MIGRATION.as_bytes())
            .to_hex()
            .to_string(),
        blake3::hash(SELECTED_SOLUTION_MIGRATION.as_bytes())
            .to_hex()
            .to_string(),
        blake3::hash(COUNTERFACTUAL_RUN_OWNERSHIP_MIGRATION.as_bytes())
            .to_hex()
            .to_string(),
    ];
    let mut applied = Vec::new();
    let mut retained_backup_path = None;
    loop {
        let observed_version: u32 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if observed_version > CURRENT_SCHEMA_VERSION {
            return Err(StoreError::NewerSchema {
                found: observed_version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        if observed_version == CURRENT_SCHEMA_VERSION {
            verify_migration_registry(connection, observed_version, &checksums)?;
            break;
        }
        #[cfg(debug_assertions)]
        if observed_version == 1
            && let Some(hook) = v2_migration_begin_test_hook
        {
            hook.actor_wait_before_begin();
        }
        #[cfg(debug_assertions)]
        if observed_version == 2
            && let Some(hook) = v3_migration_begin_test_hook
        {
            hook.actor_wait_before_begin();
        }
        #[cfg(debug_assertions)]
        if observed_version == 3
            && let Some(hook) = v4_migration_begin_test_hook
        {
            hook.actor_wait_before_begin();
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let locked_version: u32 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if locked_version > CURRENT_SCHEMA_VERSION {
            return Err(StoreError::NewerSchema {
                found: locked_version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        if locked_version == 0 {
            let has_registry: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'schema_migrations')",
                [],
                |row| row.get(0),
            )?;
            if has_registry {
                return Err(StoreError::Integrity(
                    "database migration registry disagrees with user_version".to_owned(),
                ));
            }
        } else {
            verify_migration_registry(&transaction, locked_version, &checksums)?;
        }
        if locked_version == CURRENT_SCHEMA_VERSION {
            transaction.commit()?;
            break;
        }
        let next_version = locked_version.saturating_add(1);
        match next_version {
            1 => {
                transaction.execute_batch(
                    "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, checksum TEXT NOT NULL, applied_at TEXT NOT NULL) STRICT;",
                )?;
                transaction.execute_batch(INITIAL_MIGRATION)?;
                #[cfg(debug_assertions)]
                consume_failpoint(failpoint, Failpoint::AfterMigrationSql)?;
            }
            2 => {
                if initial_version == 1 {
                    let source =
                        open_validated_read_only_source(database_path, authoritative_file)?;
                    retained_backup_path =
                        Some(ensure_pre_v2_backup(&source, database_path, &checksums[0])?);
                }
                transaction.execute_batch(INDEPENDENT_VERIFICATION_MIGRATION)?;
                #[cfg(debug_assertions)]
                consume_failpoint(failpoint, Failpoint::AfterV2MigrationSql)?;
            }
            3 => {
                transaction.execute_batch(COUNTERFACTUAL_JOB_REQUESTS_MIGRATION)?;
                #[cfg(debug_assertions)]
                consume_failpoint(failpoint, Failpoint::AfterV3MigrationSql)?;
            }
            4 => {
                transaction.execute_batch(SELECTED_SOLUTION_MIGRATION)?;
                #[cfg(debug_assertions)]
                consume_failpoint(failpoint, Failpoint::AfterV4MigrationSql)?;
            }
            5 => {
                transaction.execute_batch(COUNTERFACTUAL_RUN_OWNERSHIP_MIGRATION)?;
            }
            _ => {
                return Err(StoreError::Integrity(
                    "database migration sequence is invalid".to_owned(),
                ));
            }
        }
        let migration_name = match next_version {
            1 => V1_MIGRATION_NAME,
            2 => V2_MIGRATION_NAME,
            3 => V3_MIGRATION_NAME,
            4 => V4_MIGRATION_NAME,
            5 => V5_MIGRATION_NAME,
            _ => unreachable!(),
        };
        transaction.execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                next_version,
                migration_name,
                checksums[usize::try_from(locked_version).map_err(|_| StoreError::NumericRange)?],
            ],
        )?;
        transaction.pragma_update(None, "user_version", next_version)?;
        transaction.commit()?;
        applied.push(next_version);
    }
    verify_migration_registry(connection, CURRENT_SCHEMA_VERSION, &checksums)?;
    Ok((applied, retained_backup_path))
}

fn verify_migration_registry(
    connection: &Connection,
    pragma_version: u32,
    checksums: &[String; 5],
) -> Result<(), StoreError> {
    let has_registry: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    if !has_registry {
        return Err(StoreError::Integrity(
            "database schema migration registry is missing".to_owned(),
        ));
    }
    let mut statement = connection
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if let Some((found, _, _)) = rows
        .iter()
        .find(|(version, _, _)| *version > CURRENT_SCHEMA_VERSION)
    {
        return Err(StoreError::NewerSchema {
            found: *found,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    if rows.len() != usize::try_from(pragma_version).map_err(|_| StoreError::NumericRange)?
        || rows
            .last()
            .map(|(version, _, _)| *version)
            .unwrap_or_default()
            != pragma_version
    {
        return Err(StoreError::Integrity(
            "database migration registry disagrees with user_version".to_owned(),
        ));
    }
    let names = [
        V1_MIGRATION_NAME,
        V2_MIGRATION_NAME,
        V3_MIGRATION_NAME,
        V4_MIGRATION_NAME,
        V5_MIGRATION_NAME,
    ];
    for (index, (version, name, checksum)) in rows.iter().enumerate() {
        let expected_version = u32::try_from(index + 1).map_err(|_| StoreError::NumericRange)?;
        if *version != expected_version {
            return Err(StoreError::Integrity(
                "database migration registry is not contiguous".to_owned(),
            ));
        }
        if name != names[index] {
            return Err(StoreError::Integrity(
                "database migration registry name is invalid".to_owned(),
            ));
        }
        if checksum != &checksums[index] {
            return Err(StoreError::MigrationChanged { version: *version });
        }
    }
    Ok(())
}
fn pre_v2_backup_path(database_path: &Path) -> PathBuf {
    let mut backup_path = database_path.as_os_str().to_owned();
    backup_path.push(PRE_V2_BACKUP_SUFFIX);
    PathBuf::from(backup_path)
}

fn open_validated_read_only_source(
    database_path: &Path,
    authoritative_file: &File,
) -> Result<Connection, StoreError> {
    ensure_same_file(authoritative_file, database_path)?;
    let source = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    source.set_limit(Limit::SQLITE_LIMIT_LENGTH, SQLITE_LENGTH_LIMIT_BYTES)?;
    source.pragma_update(None, "trusted_schema", "OFF")?;
    ensure_same_file(authoritative_file, database_path)?;
    Ok(source)
}

fn ensure_pre_v2_backup(
    connection: &Connection,
    database_path: &Path,
    v1_checksum: &str,
) -> Result<PathBuf, StoreError> {
    let backup_path = pre_v2_backup_path(database_path);
    match std::fs::symlink_metadata(&backup_path) {
        Ok(_) => {
            validate_v1_backup(&backup_path, v1_checksum)?;
            validate_v1_backup_matches_live(connection, &backup_path, v1_checksum)?;
            Ok(backup_path)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let guard = prepare_private_new_file(&backup_path)?;
            if let Err(error) = connection.backup(MAIN_DB, &backup_path, None) {
                remove_guarded_file(&backup_path, &guard);
                return Err(StoreError::Database(error));
            }
            ensure_same_file(&guard.file, &backup_path)?;
            restrict_private_file(&backup_path)?;
            validate_v1_backup(&backup_path, v1_checksum)?;
            Ok(backup_path)
        }
        Err(error) => Err(StoreError::PrivatePath(error)),
    }
}

fn validate_v1_backup_matches_live(
    live: &Connection,
    backup_path: &Path,
    v1_checksum: &str,
) -> Result<(), StoreError> {
    let mut temporary_name = backup_path.as_os_str().to_owned();
    temporary_name.push(format!(".compare-{}.sqlite3", Uuid::now_v7()));
    let temporary_path = PathBuf::from(temporary_name);
    let guard = prepare_private_new_file(&temporary_path)?;
    let result = (|| {
        live.backup(MAIN_DB, &temporary_path, None)?;
        ensure_same_file(&guard.file, &temporary_path)?;
        restrict_private_file(&temporary_path)?;
        validate_v1_backup(&temporary_path, v1_checksum)?;
        if sqlite_logical_digest(&temporary_path)? != sqlite_logical_digest(backup_path)? {
            return Err(StoreError::Integrity(
                "retained pre-V2 backup does not match the live V1 database".to_owned(),
            ));
        }
        Ok(())
    })();
    for sidecar in sqlite_sidecar_paths(&temporary_path) {
        if let Ok(sidecar_guard) = prepare_private_file(&sidecar, false) {
            remove_guarded_file(&sidecar, &sidecar_guard);
        }
    }
    remove_guarded_file(&temporary_path, &guard);
    result
}

fn hash_logical_field(hasher: &mut blake3::Hasher, tag: u8, bytes: &[u8]) {
    hasher.update(&[tag]);
    hasher.update(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(bytes);
}

fn hash_sql_value(hasher: &mut blake3::Hasher, value: ValueRef<'_>) {
    match value {
        ValueRef::Null => hash_logical_field(hasher, 0, &[]),
        ValueRef::Integer(value) => hash_logical_field(hasher, 1, &value.to_le_bytes()),
        ValueRef::Real(value) => {
            hash_logical_field(hasher, 2, &value.to_bits().to_le_bytes());
        }
        ValueRef::Text(value) => hash_logical_field(hasher, 3, value),
        ValueRef::Blob(value) => hash_logical_field(hasher, 4, value),
    }
}

fn sqlite_logical_digest(path: &Path) -> Result<blake3::Hash, StoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, SQLITE_LENGTH_LIMIT_BYTES)?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    let mut hasher = blake3::Hasher::new();
    hash_logical_field(&mut hasher, 0x10, b"eutheto.sqlite.logical.v1");
    {
        let mut statement = connection.prepare(
            "SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_schema ORDER BY type, name, tbl_name, sql",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            hash_logical_field(&mut hasher, 0x11, &[]);
            for column in 0..4 {
                hash_sql_value(&mut hasher, row.get_ref(column)?);
            }
        }
    }
    let table_names = {
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for table_name in table_names {
        hash_logical_field(&mut hasher, 0x20, table_name.as_bytes());
        let quoted_name = table_name.replace('"', "\"\"");
        let projection = format!("SELECT * FROM \"{quoted_name}\"");
        let column_count = connection.prepare(&projection)?.column_count();
        hash_logical_field(
            &mut hasher,
            0x21,
            &u64::try_from(column_count)
                .map_err(|_| StoreError::NumericRange)?
                .to_le_bytes(),
        );
        let order = (1..=column_count)
            .map(|column| column.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!("{projection} ORDER BY {order}");
        let mut statement = connection.prepare(&query)?;
        for name in statement.column_names() {
            hash_logical_field(&mut hasher, 0x22, name.as_bytes());
        }
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            hash_logical_field(&mut hasher, 0x23, &[]);
            for column in 0..column_count {
                hash_sql_value(&mut hasher, row.get_ref(column)?);
            }
        }
    }
    Ok(hasher.finalize())
}

fn validate_v1_backup(path: &Path, v1_checksum: &str) -> Result<(), StoreError> {
    ensure_private_parent(path)?;
    let guard = prepare_private_file(path, false)?;
    ensure_same_file(&guard.file, path)?;
    for sidecar in sqlite_sidecar_paths(path) {
        if std::fs::symlink_metadata(&sidecar).is_ok() {
            return Err(StoreError::Integrity(
                "retained pre-V2 backup has a SQLite sidecar".to_owned(),
            ));
        }
    }
    let backup = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    backup.set_limit(Limit::SQLITE_LIMIT_LENGTH, SQLITE_LENGTH_LIMIT_BYTES)?;
    backup.pragma_update(None, "trusted_schema", "OFF")?;
    let version: u32 = backup.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 1 {
        return Err(StoreError::Integrity(
            "retained pre-V2 backup is not a V1 database".to_owned(),
        ));
    }
    let quick_check: String = backup.pragma_query_value(None, "quick_check", |row| row.get(0))?;
    if quick_check != "ok" {
        return Err(StoreError::Integrity(
            "retained pre-V2 backup failed quick_check".to_owned(),
        ));
    }
    let row: Option<(String, String)> = backup
        .query_row(
            "SELECT name, checksum FROM schema_migrations WHERE version = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if row.as_ref().map(|(name, _)| name.as_str()) != Some(V1_MIGRATION_NAME)
        || row.as_ref().map(|(_, checksum)| checksum.as_str()) != Some(v1_checksum)
    {
        return Err(StoreError::Integrity(
            "retained pre-V2 backup has an invalid V1 registry".to_owned(),
        ));
    }
    let count: u32 = backup.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
        row.get(0)
    })?;
    if count != 1 {
        return Err(StoreError::Integrity(
            "retained pre-V2 backup has an invalid migration registry".to_owned(),
        ));
    }
    Ok(())
}

fn recover_running_v2_runs(connection: &mut Connection) -> Result<Vec<SolveRunId>, StoreError> {
    let candidates = {
        let mut statement = connection.prepare(
            "SELECT id FROM solve_runs WHERE status = 'running' AND run_manifest_json IS NULL AND NOT EXISTS (SELECT 1 FROM counterfactual_jobs WHERE derived_run_id = solve_runs.id) ORDER BY id",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let now = jiff::Timestamp::now();
    let finished_at = Rfc3339Timestamp::from_timestamp(now);
    let mut interrupted = Vec::new();
    for run_id_text in candidates {
        let Ok(run_id) = run_id_text.parse::<SolveRunId>() else {
            continue;
        };
        let (input, status, stored_manifest, started_at) =
            match load_run_input_record(connection, run_id) {
                Ok(record) => record,
                Err(StoreError::InvalidPersistedRun(_) | StoreError::SolveRunNotFound(_)) => {
                    continue;
                }
                Err(error) => return Err(error),
            };
        let Some(recovery_deadline) = solve_recovery_deadline(&input, started_at) else {
            continue;
        };
        if status != "running" || stored_manifest.is_some() || now < recovery_deadline {
            continue;
        }
        let Ok(manifest) = RunManifestV1::new(
            run_id,
            input.checksum,
            RunTerminalOutcomeV1::Interrupted,
            started_at,
            finished_at,
            None,
            None,
            None,
            RunPhaseTimingsV1::default(),
            Vec::new(),
        ) else {
            continue;
        };
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        finalize_terminal_run_in_transaction(&transaction, &manifest)?;
        transaction.commit()?;
        interrupted.push(run_id);
    }
    Ok(interrupted)
}

fn counterfactual_recovery_deadline(
    record: &CounterfactualJobRecordV1,
) -> Result<Option<jiff::Timestamp>, StoreError> {
    let started_at = record
        .started_at
        .ok_or_else(|| invalid_counterfactual("running job has no start time"))?;
    Ok(started_at
        .as_timestamp()
        .checked_add(std::time::Duration::from_millis(
            record.request.semantics.total_budget_milliseconds.value(),
        ))
        .ok()
        .and_then(|deadline| {
            deadline
                .checked_add(std::time::Duration::from_millis(
                    SOLVE_TERMINAL_PERSISTENCE_GRACE_MILLISECONDS,
                ))
                .ok()
        }))
}

fn recover_expired_counterfactual_jobs(
    connection: &mut Connection,
    now: jiff::Timestamp,
) -> Result<(Vec<CounterfactualJobId>, Vec<SolveRunId>), StoreError> {
    let candidates = {
        let mut statement = connection
            .prepare("SELECT id FROM counterfactual_jobs WHERE state = 'running' ORDER BY id")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let finished_at = Rfc3339Timestamp::from_timestamp(now);
    let mut recovered = Vec::new();
    let mut interrupted_runs = Vec::new();
    #[cfg(debug_assertions)]
    let recovery_failpoint = Arc::new(std::sync::Mutex::new(None));
    for job_id_text in candidates {
        let job_id: CounterfactualJobId = job_id_text.parse().map_err(|error| {
            invalid_counterfactual(format!("invalid recovery job identity: {error}"))
        })?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load_counterfactual_job_row(&transaction, job_id)?;
        if record.state != CounterfactualJobState::Running || record.finished_at.is_some() {
            continue;
        }
        let Some(recovery_deadline) = counterfactual_recovery_deadline(&record)? else {
            continue;
        };
        if now < recovery_deadline {
            continue;
        }
        let ownership = load_counterfactual_run_ownership(&transaction, job_id)?;
        if let Some(ownership) = ownership {
            let (input, status, stored_manifest, started_at) =
                load_run_input_record(&transaction, ownership.run_id)
                    .map_err(|error| invalid_counterfactual(error.to_string()))?;
            if input.request_id != ownership.request_id
                || status != "running"
                || stored_manifest.is_some()
            {
                return Err(invalid_counterfactual(
                    "expired job derived run is not recoverably running",
                ));
            }
            validate_counterfactual_derived_input(&transaction, &record, &input, started_at)?;
            let cancelled = record.cancel_request_id.is_some();
            let manifest = RunManifestV1::new(
                input.run_id,
                input.checksum,
                RunTerminalOutcomeV1::Interrupted,
                started_at,
                finished_at,
                None,
                None,
                None,
                RunPhaseTimingsV1::default(),
                Vec::new(),
            )
            .map_err(|error| invalid_counterfactual(error.to_string()))?;
            let finalization = CounterfactualRunFinalizationV1::Interrupted { manifest };
            finalize_counterfactual_run_in_transaction(
                &transaction,
                job_id,
                &finalization,
                cancelled,
                #[cfg(debug_assertions)]
                &recovery_failpoint,
            )?;
            transaction.commit()?;
            interrupted_runs.push(ownership.run_id);
        } else {
            let transition = if record.cancel_request_id.is_some() {
                CounterfactualJobTransitionV1::Cancelled { finished_at }
            } else {
                CounterfactualJobTransitionV1::Interrupted { finished_at }
            };
            transition_counterfactual_job_in_transaction(
                &transaction,
                job_id,
                &transition,
                true,
                #[cfg(debug_assertions)]
                &recovery_failpoint,
            )?;
            transaction.commit()?;
        }
        recovered.push(job_id);
    }
    Ok((recovered, interrupted_runs))
}

async fn update_archive(
    store: &SqliteScenarioStore,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    archived_at: Option<Rfc3339Timestamp>,
) -> Result<(), StoreError> {
    store
        .call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let actual_revision = scenario_revision(&transaction, scenario_id)?;
            ensure_revision(expected_revision, actual_revision.value())?;
            transaction.execute(
                "UPDATE scenarios SET archived_at = ?2 WHERE id = ?1",
                params![
                    scenario_id.to_string(),
                    archived_at.map(|timestamp| timestamp.to_string())
                ],
            )?;
            increment_library_revision(&transaction)?;
            transaction.commit()?;
            Ok(())
        })
        .await
}

fn scenario_revision(
    connection: &Connection,
    scenario_id: ScenarioId,
) -> Result<Revision, StoreError> {
    let revision = connection
        .query_row(
            "SELECT revision FROM scenarios WHERE id = ?1",
            [scenario_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .ok_or(StoreError::ScenarioNotFound(scenario_id))?;
    checked_revision(i64_to_u64(revision)?)
}

fn selected_solution_id(
    connection: &Connection,
    scenario_id: ScenarioId,
) -> Result<Option<SolutionId>, StoreError> {
    let selected: Option<String> = connection
        .query_row(
            "SELECT selected_solution_id FROM scenarios WHERE id = ?1",
            [scenario_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(StoreError::ScenarioNotFound(scenario_id))?;
    selected
        .map(|value| {
            value.parse().map_err(|error| {
                StoreError::InvalidPersistedResult(format!(
                    "invalid selected solution identity: {error}"
                ))
            })
        })
        .transpose()
}

fn scenario_archived_at(
    connection: &Connection,
    scenario_id: ScenarioId,
) -> Result<Option<Rfc3339Timestamp>, StoreError> {
    let archived_at = connection
        .query_row(
            "SELECT archived_at FROM scenarios WHERE id = ?1",
            [scenario_id.to_string()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    parse_optional_timestamp(archived_at, "scenario archived_at")
}

fn load_project(
    connection: &Connection,
    scenario_id: ScenarioId,
) -> Result<StoredProject, StoreError> {
    let row = connection
        .query_row(
            "SELECT id, domain_pack_id, domain_schema_version, title, description, revision, created_at, updated_at, last_opened_at, archived_at, document_json, portable_required_capabilities_json, portable_semantic_extensions_json, portable_extensions_json FROM scenarios WHERE id = ?1",
            [scenario_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                ))
            },
        )
        .optional()?;
    let Some((
        id,
        pack,
        schema,
        title,
        description,
        revision,
        created_at,
        updated_at,
        last_opened_at,
        archived_at,
        document_json,
        required_capabilities_json,
        semantic_extensions_json,
        extensions_json,
    )) = row
    else {
        return Err(StoreError::ScenarioNotFound(scenario_id));
    };
    let id = id
        .parse()
        .map_err(|error| StoreError::Integrity(format!("invalid stored scenario id: {error}")))?;
    let domain_pack_id = PackId::new(&pack)
        .map_err(|error| StoreError::Integrity(format!("invalid stored pack id: {error}")))?;
    let document_value: Value = serde_json::from_str(&document_json)?;
    validate_stored_portable_json(&document_value, "scenario document")?;
    let semantic_extensions_value: Value = serde_json::from_str(&semantic_extensions_json)?;
    validate_stored_portable_json(&semantic_extensions_value, "scenario semantic extensions")?;
    let extensions_value: Value = serde_json::from_str(&extensions_json)?;
    validate_stored_portable_json(&extensions_value, "scenario extensions")?;
    let project = StoredProject {
        summary: ProjectSummary {
            id,
            domain_pack_id,
            domain_schema_version: i64_to_u32(schema)?,
            title,
            description,
            revision: checked_revision(i64_to_u64(revision)?)?,
            created_at: parse_timestamp(&created_at, "scenario created_at")?,
            updated_at: parse_timestamp(&updated_at, "scenario updated_at")?,
            last_opened_at: parse_optional_timestamp(last_opened_at, "scenario last_opened_at")?,
            archived_at: parse_optional_timestamp(archived_at, "scenario archived_at")?,
        },
        document: serde_json::from_value(document_value)?,
        portable: PortableWrapperMetadata {
            required_capabilities: serde_json::from_str(&required_capabilities_json)?,
            semantic_extensions: serde_json::from_value(semantic_extensions_value)?,
            extensions: serde_json::from_value(extensions_value)?,
        },
    };
    validate_project_owned_uuid_uniqueness(
        &project.document,
        project.summary.revision,
        &project.portable,
    )
    .map_err(StoreError::Integrity)?;
    Ok(project)
}

fn load_retained_scenario_revisions(
    connection: &Connection,
) -> Result<Vec<StoredScenarioRevision>, StoreError> {
    let result_dependencies = retained_result_dependencies(connection)?;
    let internal_dependencies = retained_scenario_dependencies(connection)?;
    let current = scenario_revisions(connection)?;
    let retained = read_retained_scenario_revision_rows(connection)?;
    for identity in retained.keys() {
        if !internal_dependencies.contains(identity) {
            return Err(StoreError::Integrity(
                "an immutable scenario revision has no retention dependency".to_owned(),
            ));
        }
    }
    let mut historical = Vec::new();
    for dependency in result_dependencies {
        if current.get(&dependency.scenario_id) == Some(&dependency.scenario_revision) {
            continue;
        }
        let scenario = retained.get(&dependency).cloned().ok_or_else(|| {
            StoreError::Integrity(format!(
                "retained result requires unavailable scenario {} revision {}",
                dependency.scenario_id,
                dependency.scenario_revision.value()
            ))
        })?;
        historical.push(StoredScenarioRevision { scenario });
    }
    Ok(historical)
}

fn synchronize_retained_scenario_revisions(
    transaction: &rusqlite::Transaction<'_>,
    staged: Vec<ScenarioSnapshotV1>,
) -> Result<(), StoreError> {
    let mut candidates = BTreeMap::new();
    for scenario in staged {
        validate_scenario_owned_uuid_uniqueness(&scenario)
            .map_err(|error| StoreError::InvalidStagedApply(error.to_string()))?;
        serialize_document(&scenario.document)?;
        validate_historical_scenario(&scenario).map_err(StoreError::InvalidStagedApply)?;
        let identity = ScenarioRevisionReference {
            scenario_id: scenario.document.scenario_id,
            scenario_revision: scenario.revision,
        };
        if let Some(existing) = candidates.insert(identity, scenario.clone())
            && existing != scenario
        {
            return Err(StoreError::InvalidStagedApply(
                "staged input contains conflicting immutable scenario revisions".to_owned(),
            ));
        }
    }
    for scenario in candidates.values() {
        insert_retained_scenario_revision(transaction, scenario)?;
    }

    let dependencies = retained_scenario_dependencies(transaction)?;
    let retained = read_retained_scenario_revision_rows(transaction)?;
    for dependency in &dependencies {
        if retained.contains_key(dependency) {
            continue;
        }
        let project =
            load_project(transaction, dependency.scenario_id).map_err(|error| match error {
                StoreError::ScenarioNotFound(_) => StoreError::InvalidStagedApply(format!(
                    "retained result requires missing scenario {}",
                    dependency.scenario_id
                )),
                other => other,
            })?;
        if project.summary.revision != dependency.scenario_revision {
            return Err(StoreError::InvalidStagedApply(format!(
                "retained result requires unavailable scenario {} revision {}",
                dependency.scenario_id,
                dependency.scenario_revision.value()
            )));
        }
        let scenario = historical_scenario_from_project(project);
        insert_retained_scenario_revision(transaction, &scenario)?;
    }

    let retained = read_retained_scenario_revision_rows(transaction)?;
    let mut delete = transaction.prepare(
        "DELETE FROM retained_scenario_revisions WHERE scenario_id = ?1 AND revision = ?2",
    )?;
    for identity in retained.keys() {
        if !dependencies.contains(identity) {
            delete.execute(params![
                identity.scenario_id.to_string(),
                u64_to_i64(identity.scenario_revision.value())?
            ])?;
        }
    }
    Ok(())
}

fn retain_current_project_revision_if_required(
    transaction: &rusqlite::Transaction<'_>,
    project: &StoredProject,
) -> Result<(), StoreError> {
    let identity = ScenarioRevisionReference {
        scenario_id: project.summary.id,
        scenario_revision: project.summary.revision,
    };
    if retained_scenario_dependencies(transaction)?.contains(&identity) {
        insert_retained_scenario_revision(
            transaction,
            &historical_scenario_from_project(project.clone()),
        )?;
    }
    Ok(())
}

fn historical_scenario_from_project(project: StoredProject) -> ScenarioSnapshotV1 {
    let mut scenario = ScenarioSnapshotV1::current(
        project.summary.revision,
        project.document,
        project.portable.required_capabilities,
    );
    scenario.semantic_extensions = project.portable.semantic_extensions;
    scenario.extensions = project.portable.extensions;
    scenario
}

fn validate_historical_scenario(scenario: &ScenarioSnapshotV1) -> Result<(), String> {
    if scenario.project.is_some() {
        return Err("immutable scenario revisions may not contain project metadata".to_owned());
    }
    eutheto_export::validate_scenario_snapshot(scenario)
        .map_err(|error| format!("immutable scenario revision is invalid: {error}"))
}

fn insert_retained_scenario_revision(
    connection: &Connection,
    scenario: &ScenarioSnapshotV1,
) -> Result<(), StoreError> {
    let scenario_id = scenario.document.scenario_id;
    let revision = scenario.revision;
    let serialized = serde_json::to_string(scenario)?;
    let existing = connection
        .query_row(
            "SELECT scenario_json FROM retained_scenario_revisions WHERE scenario_id = ?1 AND revision = ?2",
            params![scenario_id.to_string(), u64_to_i64(revision.value())?],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        let existing: ScenarioSnapshotV1 = serde_json::from_str(&existing)?;
        if &existing != scenario {
            return Err(StoreError::InvalidStagedApply(format!(
                "immutable scenario {} revision {} conflicts with retained content",
                scenario_id,
                revision.value()
            )));
        }
        record_scenario_revision_high_water(connection, scenario_id, revision)?;
        return Ok(());
    }
    connection.execute(
        "INSERT INTO retained_scenario_revisions (scenario_id, revision, scenario_json) VALUES (?1, ?2, ?3)",
        params![
            scenario_id.to_string(),
            u64_to_i64(revision.value())?,
            serialized
        ],
    )?;
    record_scenario_revision_high_water(connection, scenario_id, revision)?;
    Ok(())
}

fn read_retained_scenario_revision_rows(
    connection: &Connection,
) -> Result<BTreeMap<ScenarioRevisionReference, ScenarioSnapshotV1>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT scenario_id, revision, scenario_json FROM retained_scenario_revisions ORDER BY scenario_id, revision",
    )?;
    let mut rows = statement.query([])?;
    let mut revisions = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let scenario_id_text: String = row.get(0)?;
        let scenario_id = scenario_id_text.parse().map_err(|error| {
            StoreError::Integrity(format!(
                "invalid retained scenario revision identity: {error}"
            ))
        })?;
        let revision = checked_revision(i64_to_u64(row.get(1)?)?)?;
        let json: String = row.get(2)?;
        let scenario: ScenarioSnapshotV1 = serde_json::from_str(&json)?;
        validate_historical_scenario(&scenario).map_err(StoreError::Integrity)?;
        if scenario.document.scenario_id != scenario_id || scenario.revision != revision {
            return Err(StoreError::Integrity(
                "retained scenario revision columns disagree with its document".to_owned(),
            ));
        }
        revisions.insert(
            ScenarioRevisionReference {
                scenario_id,
                scenario_revision: revision,
            },
            scenario,
        );
    }
    Ok(revisions)
}

fn retained_result_dependencies(
    connection: &Connection,
) -> Result<BTreeSet<ScenarioRevisionReference>, StoreError> {
    let mut statement = connection
        .prepare("SELECT value FROM portable_sections WHERE section = ?1 ORDER BY key")?;
    let mut rows = statement.query([SupplementalSectionKind::Results.as_str()])?;
    let mut dependencies = BTreeSet::new();
    while let Some(row) = rows.next()? {
        let bytes: Vec<u8> = row.get(0)?;
        let value: Value = serde_json::from_slice(&bytes)?;
        let dependency = extract_result_dependency(&value).map_err(|error| {
            StoreError::Integrity(format!(
                "stored retained result has an invalid scenario revision reference: {error}"
            ))
        })?;
        dependencies.insert(dependency);
    }
    drop(rows);
    drop(statement);
    let mut statement = connection.prepare(
        "SELECT r.run_input_json FROM solutions s
         JOIN solve_runs r ON r.id = s.solve_run_id
         WHERE s.accepted = 1 AND s.status = 'verified'
         ORDER BY r.run_input_json",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let json: String = row.get(0)?;
        let input = parse_run_input(json.as_bytes())?;
        dependencies.insert(ScenarioRevisionReference {
            scenario_id: input.scenario_id,
            scenario_revision: checked_revision(input.scenario_revision)?,
        });
    }
    Ok(dependencies)
}

fn retained_scenario_dependencies(
    connection: &Connection,
) -> Result<BTreeSet<ScenarioRevisionReference>, StoreError> {
    let mut dependencies = retained_result_dependencies(connection)?;
    let mut statement = connection.prepare(
        "SELECT run_input_json FROM solve_runs
         WHERE status = 'running' AND run_input_json IS NOT NULL
         ORDER BY run_input_json",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let json: String = row.get(0)?;
        let input = parse_run_input(json.as_bytes())?;
        dependencies.insert(ScenarioRevisionReference {
            scenario_id: input.scenario_id,
            scenario_revision: checked_revision(input.scenario_revision)?,
        });
    }
    Ok(dependencies)
}

fn read_summary_row(row: &rusqlite::Row<'_>) -> Result<ProjectSummary, StoreError> {
    let id_text: String = row.get(0)?;
    let id = id_text
        .parse()
        .map_err(|error| StoreError::Integrity(format!("invalid stored scenario id: {error}")))?;
    let pack_text: String = row.get(1)?;
    let domain_pack_id = PackId::new(&pack_text)
        .map_err(|error| StoreError::Integrity(format!("invalid stored pack id: {error}")))?;
    let created_at: String = row.get(6)?;
    let updated_at: String = row.get(7)?;
    let last_opened_at: Option<String> = row.get(8)?;
    let archived_at: Option<String> = row.get(9)?;
    Ok(ProjectSummary {
        id,
        domain_pack_id,
        domain_schema_version: i64_to_u32(row.get(2)?)?,
        title: row.get(3)?,
        description: row.get(4)?,
        revision: checked_revision(i64_to_u64(row.get(5)?)?)?,
        created_at: parse_timestamp(&created_at, "scenario created_at")?,
        updated_at: parse_timestamp(&updated_at, "scenario updated_at")?,
        last_opened_at: parse_optional_timestamp(last_opened_at, "scenario last_opened_at")?,
        archived_at: parse_optional_timestamp(archived_at, "scenario archived_at")?,
    })
}

fn history_state(
    transaction: &rusqlite::Transaction<'_>,
    scenario_id: ScenarioId,
) -> Result<(u64, u64, u64), StoreError> {
    let (cursor, generation) = transaction.query_row(
        "SELECT cursor_sequence, branch_generation FROM scenario_history_state WHERE scenario_id = ?1",
        [scenario_id.to_string()],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let max_sequence: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(history_sequence), 0) FROM command_journal WHERE scenario_id = ?1",
        [scenario_id.to_string()],
        |row| row.get(0),
    )?;
    Ok((
        i64_to_u64(cursor)?,
        i64_to_u64(generation)?,
        i64_to_u64(max_sequence)?,
    ))
}

fn load_history_entry(
    transaction: &rusqlite::Transaction<'_>,
    scenario_id: ScenarioId,
    sequence: u64,
    cursor: u64,
) -> Result<HistoryEntry, StoreError> {
    let mut statement = transaction.prepare(
        "SELECT id, revision_before, revision_after, command_type, command_json, inverse_json, actor_json, source, summary, created_at, history_sequence, branch_generation FROM command_journal WHERE scenario_id = ?1 AND history_sequence = ?2 ORDER BY branch_generation DESC LIMIT 1",
    )?;
    let mut rows = statement.query(params![scenario_id.to_string(), u64_to_i64(sequence)?])?;
    let Some(row) = rows.next()? else {
        return Err(StoreError::Integrity(
            "history cursor references a missing journal entry".to_owned(),
        ));
    };
    parse_history_row(row, cursor)
}

/// Checks retained-tail/cursor links without loading journal payloads.
fn history_page_availability(
    transaction: &rusqlite::Transaction<'_>,
    scenario_key: &str,
    cursor: u64,
    generation: u64,
    max_sequence: u64,
) -> Result<(bool, bool), StoreError> {
    let (tail_count, tail_generation): (i64, Option<i64>) = transaction.query_row(
        "SELECT COUNT(*), MAX(branch_generation) FROM command_journal
         WHERE scenario_id = ?1 AND history_sequence = ?2",
        params![scenario_key, u64_to_i64(max_sequence)?],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if tail_count != i64::from(max_sequence > 0)
        || tail_generation.map(i64_to_u64).transpose()? != (max_sequence > 0).then_some(generation)
    {
        return Err(StoreError::Integrity(
            "history branch does not match its retained tail".to_owned(),
        ));
    }
    let (cursor_count, reversible): (i64, bool) = transaction.query_row(
        "SELECT COUNT(*), COALESCE(MAX(inverse_json IS NOT NULL), 0)
         FROM command_journal WHERE scenario_id = ?1 AND history_sequence = ?2",
        params![scenario_key, u64_to_i64(cursor)?],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let redo_count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM command_journal
         WHERE scenario_id = ?1 AND history_sequence = ?2",
        params![scenario_key, u64_to_i64(cursor + 1)?],
        |row| row.get(0),
    )?;
    if cursor_count != i64::from(cursor > 0) || redo_count != i64::from(cursor < max_sequence) {
        return Err(StoreError::Integrity(
            "history cursor references missing or ambiguous journal entries".to_owned(),
        ));
    }
    Ok((cursor > 0 && reversible, cursor < max_sequence))
}

fn parse_history_summary_row(
    row: &rusqlite::Row<'_>,
    cursor: u64,
    generation: u64,
    revision: Revision,
) -> Result<HistoryEntrySummary, StoreError> {
    let sequence = i64_to_u64(row.get(6)?)?;
    let branch_generation = i64_to_u64(row.get(7)?)?;
    let revision_before = checked_revision(i64_to_u64(row.get(1)?)?)?;
    let revision_after = checked_revision(i64_to_u64(row.get(2)?)?)?;
    if sequence == 0
        || sequence > revision_after.value()
        || branch_generation > generation
        || revision_before >= revision_after
        || revision_after > revision
    {
        return Err(StoreError::Integrity(
            "invalid history entry numbers".to_owned(),
        ));
    }
    let id_text: String = row.get(0)?;
    let id = id_text
        .parse()
        .map_err(|error| StoreError::Integrity(format!("invalid stored command id: {error}")))?;
    let source: String = row.get(3)?;
    let created_at: String = row.get(5)?;
    Ok(HistoryEntrySummary {
        id,
        revision_before,
        revision_after,
        source: parse_command_source(&source)?,
        summary: row.get(4)?,
        created_at: parse_timestamp(&created_at, "journal created_at")?,
        history_sequence: sequence,
        branch_generation,
        applied: sequence <= cursor,
    })
}

fn parse_history_row(row: &rusqlite::Row<'_>, cursor: u64) -> Result<HistoryEntry, StoreError> {
    let sequence = i64_to_u64(row.get(10)?)?;
    let id_text: String = row.get(0)?;
    let id = id_text
        .parse()
        .map_err(|error| StoreError::Integrity(format!("invalid stored command id: {error}")))?;
    let source_text: String = row.get(7)?;
    let created_at: String = row.get(9)?;
    Ok(HistoryEntry {
        id,
        revision_before: checked_revision(i64_to_u64(row.get(1)?)?)?,
        revision_after: checked_revision(i64_to_u64(row.get(2)?)?)?,
        command_type: row.get(3)?,
        command: serde_json::from_str(&row.get::<_, String>(4)?)?,
        inverse: row
            .get::<_, Option<String>>(5)?
            .map(|json| serde_json::from_str(&json))
            .transpose()?,
        actor: serde_json::from_str(&row.get::<_, String>(6)?)?,
        source: parse_command_source(&source_text)?,
        summary: row.get(8)?,
        created_at: parse_timestamp(&created_at, "journal created_at")?,
        history_sequence: sequence,
        branch_generation: i64_to_u64(row.get(11)?)?,
        applied: sequence <= cursor,
    })
}

fn maybe_snapshot(
    transaction: &rusqlite::Transaction<'_>,
    scenario_id: ScenarioId,
    revision: u64,
    sequence: u64,
    document: &ScenarioDocument,
    created_at: &Rfc3339Timestamp,
    policy: SnapshotPolicy,
) -> Result<(), StoreError> {
    if !sequence.is_multiple_of(u64::from(policy.interval.get())) {
        return Ok(());
    }
    let bytes = serde_json::to_vec(document)?;
    if u64::try_from(bytes.len()).map_err(|_| StoreError::NumericRange)? > policy.max_document_bytes
    {
        return Err(StoreError::SnapshotTooLarge);
    }
    let compressed = zstd::stream::encode_all(bytes.as_slice(), policy.compression_level)
        .map_err(StoreError::Compression)?;
    transaction.execute(
        "INSERT INTO scenario_snapshots (id, scenario_id, revision, document_json_zstd, created_at, reason) VALUES (?1, ?2, ?3, ?4, ?5, 'periodic')",
        params![Uuid::now_v7().to_string(), scenario_id.to_string(), u64_to_i64(revision)?, compressed, created_at.to_string()],
    )?;
    Ok(())
}

struct DocumentProjection {
    domain_pack_id: String,
    domain_schema_version: u32,
    title: String,
    description: Option<String>,
    created_at: String,
    updated_at: String,
}

fn document_projection(document: &ScenarioDocument) -> DocumentProjection {
    DocumentProjection {
        domain_pack_id: document.domain_pack.id.to_string(),
        domain_schema_version: document.domain_pack.schema_version,
        title: document.metadata.title.clone(),
        description: Some(document.metadata.description.clone()),
        created_at: document.metadata.created_at.to_string(),
        updated_at: document.metadata.updated_at.to_string(),
    }
}

fn duplicate_project_contents(
    source: StoredProject,
    new_id: ScenarioId,
    mapping: &BTreeMap<Uuid, Uuid>,
    occupied: &BTreeSet<Uuid>,
    title: &str,
    timestamp: Rfc3339Timestamp,
) -> Result<(ScenarioDocument, PortableWrapperMetadata), StoreError> {
    let owned = collect_project_owned_uuids(
        &source.document,
        &source.portable.semantic_extensions,
        &source.portable.extensions,
    );
    let mapped_keys = mapping.keys().copied().collect::<BTreeSet<_>>();
    if mapped_keys != owned {
        return Err(StoreError::InvalidDuplicateMapping(
            "mapping keys do not exactly match source-owned identities".to_owned(),
        ));
    }
    if mapping.get(&source.document.scenario_id.as_uuid()) != Some(&new_id.as_uuid()) {
        return Err(StoreError::InvalidDuplicateMapping(
            "source scenario identity does not map to the requested destination".to_owned(),
        ));
    }
    let mapped_values = mapping.values().copied().collect::<BTreeSet<_>>();
    if mapped_values.len() != mapping.len() {
        return Err(StoreError::InvalidDuplicateMapping(
            "mapping target identities are not unique".to_owned(),
        ));
    }
    if mapped_values
        .iter()
        .any(|identity| identity.get_version_num() != 7)
    {
        return Err(StoreError::InvalidDuplicateMapping(
            "mapping target identities must be UUIDv7".to_owned(),
        ));
    }
    if mapped_values
        .iter()
        .any(|identity| occupied.contains(identity))
    {
        return Err(StoreError::InvalidDuplicateMapping(
            "mapping target identity is already occupied".to_owned(),
        ));
    }

    let mut domain = serde_json::to_value(&source.document.domain)?;
    let mut semantic_extensions = serde_json::to_value(&source.portable.semantic_extensions)?;
    let mut document_extensions = serde_json::to_value(&source.document.extensions)?;
    let mut portable_extensions = serde_json::to_value(&source.portable.extensions)?;
    rewrite_domain_definitions(&mut domain, mapping)?;
    rewrite_self_declared_definitions(&mut domain, mapping)?;
    rewrite_self_declared_definitions(&mut semantic_extensions, mapping)?;
    rewrite_self_declared_definitions(&mut document_extensions, mapping)?;
    rewrite_self_declared_definitions(&mut portable_extensions, mapping)?;
    rewrite_declared_references(&mut domain, mapping)?;
    rewrite_declared_references(&mut semantic_extensions, mapping)?;
    rewrite_declared_references(&mut document_extensions, mapping)?;
    rewrite_declared_references(&mut portable_extensions, mapping)?;
    reject_stale_declared_references(&domain, mapping)?;
    reject_stale_declared_references(&semantic_extensions, mapping)?;
    reject_stale_declared_references(&document_extensions, mapping)?;
    reject_stale_declared_references(&portable_extensions, mapping)?;

    let mut document = source.document;
    document.scenario_id = new_id;
    title.clone_into(&mut document.metadata.title);
    document.metadata.created_at = timestamp;
    document.metadata.updated_at = timestamp;
    document.domain = serde_json::from_value(domain)?;
    document.extensions = serde_json::from_value(document_extensions)?;
    let mut portable = source.portable;
    portable.semantic_extensions = serde_json::from_value(semantic_extensions)?;
    portable.extensions = serde_json::from_value(portable_extensions)?;
    Ok((document, portable))
}

fn collect_project_owned_uuids(
    document: &ScenarioDocument,
    semantic_extensions: &BTreeMap<String, Value>,
    extensions: &BTreeMap<String, Value>,
) -> BTreeSet<Uuid> {
    let mut owned = BTreeSet::from([document.scenario_id.as_uuid()]);
    owned.extend(
        document
            .domain
            .entities
            .keys()
            .map(|id| id.as_uuid())
            .chain(document.domain.rules.keys().map(|id| id.as_uuid()))
            .chain(document.domain.preferences.keys().map(|id| id.as_uuid()))
            .chain(
                document
                    .domain
                    .locked_assignments
                    .keys()
                    .map(|id| id.as_uuid()),
            ),
    );
    for value in document
        .domain
        .entities
        .values()
        .chain(document.domain.rules.values())
        .chain(document.domain.preferences.values())
        .chain(document.domain.locked_assignments.values())
        .chain(semantic_extensions.values())
        .chain(document.extensions.values())
        .chain(extensions.values())
    {
        owned.extend(collect_self_declared_uuids(value));
    }
    owned
}

fn authoritative_occupied_uuids(connection: &Connection) -> Result<BTreeSet<Uuid>, StoreError> {
    let mut occupied = BTreeSet::new();
    occupied.extend(load_scenario_identity_owners(connection)?.keys().copied());
    occupied.extend(
        load_scenario_revision_high_water(connection)?
            .keys()
            .map(|scenario_id| scenario_id.as_uuid()),
    );
    for scenario_id in scenario_revisions(connection)?.keys() {
        let project = load_project(connection, *scenario_id)?;
        occupied.extend(collect_project_owned_uuids(
            &project.document,
            &project.portable.semantic_extensions,
            &project.portable.extensions,
        ));
    }
    for scenario in read_retained_scenario_revision_rows(connection)?.into_values() {
        occupied.extend(collect_project_owned_uuids(
            &scenario.document,
            &scenario.semantic_extensions,
            &scenario.extensions,
        ));
    }
    occupied.extend(load_authoritative_solve_identities(connection)?);
    occupied.extend(
        load_supplemental_identity_owners(connection)?
            .keys()
            .copied(),
    );
    Ok(occupied)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StoredIdentityOwner {
    ScenarioFamily(ScenarioId),
    SolveState,
    Supplemental(SupplementalIdentity),
}

fn register_stored_identity(
    owners: &mut BTreeMap<Uuid, StoredIdentityOwner>,
    identity: Uuid,
    owner: StoredIdentityOwner,
) -> Result<(), StoreError> {
    if let Some(existing) = owners.get(&identity) {
        if matches!(
            (existing, &owner),
            (
                StoredIdentityOwner::ScenarioFamily(left),
                StoredIdentityOwner::ScenarioFamily(right)
            ) if left == right
        ) {
            return Ok(());
        }
        return Err(StoreError::IdentityCollision(identity));
    }
    owners.insert(identity, owner);
    Ok(())
}

fn load_authoritative_solve_identities(
    connection: &Connection,
) -> Result<BTreeSet<Uuid>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT id FROM scenario_snapshots
         UNION ALL SELECT id FROM solve_runs
         UNION ALL SELECT request_id FROM solve_runs WHERE request_id IS NOT NULL
         UNION ALL SELECT id FROM solutions
         UNION ALL SELECT id FROM counterfactual_jobs
         UNION ALL SELECT request_id FROM counterfactual_jobs
         UNION ALL SELECT cancel_request_id FROM counterfactual_jobs WHERE cancel_request_id IS NOT NULL",
    )?;
    let mut identities = BTreeSet::new();
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for value in rows {
        let identity = Uuid::parse_str(&value).map_err(|error| {
            StoreError::Integrity(format!("invalid authoritative solve identity: {error}"))
        })?;
        if !identities.insert(identity) {
            return Err(StoreError::IdentityCollision(identity));
        }
    }
    Ok(identities)
}

fn validate_global_identity_ownership(connection: &Connection) -> Result<(), StoreError> {
    let mut owners = BTreeMap::new();
    for (identity, scenario_id) in load_scenario_identity_owners(connection)? {
        register_stored_identity(
            &mut owners,
            identity,
            StoredIdentityOwner::ScenarioFamily(scenario_id),
        )?;
    }
    for scenario_id in load_scenario_revision_high_water(connection)?
        .keys()
        .copied()
    {
        register_stored_identity(
            &mut owners,
            scenario_id.as_uuid(),
            StoredIdentityOwner::ScenarioFamily(scenario_id),
        )?;
    }
    for scenario_id in scenario_revisions(connection)?.keys().copied() {
        let project = load_project(connection, scenario_id)?;
        for identity in collect_project_owned_uuids(
            &project.document,
            &project.portable.semantic_extensions,
            &project.portable.extensions,
        ) {
            register_stored_identity(
                &mut owners,
                identity,
                StoredIdentityOwner::ScenarioFamily(scenario_id),
            )?;
        }
    }
    for scenario in read_retained_scenario_revision_rows(connection)?.into_values() {
        let scenario_id = scenario.document.scenario_id;
        for identity in collect_project_owned_uuids(
            &scenario.document,
            &scenario.semantic_extensions,
            &scenario.extensions,
        ) {
            register_stored_identity(
                &mut owners,
                identity,
                StoredIdentityOwner::ScenarioFamily(scenario_id),
            )?;
        }
    }
    for identity in load_authoritative_solve_identities(connection)? {
        register_stored_identity(&mut owners, identity, StoredIdentityOwner::SolveState)?;
    }
    for (identity, supplemental) in load_supplemental_identity_owners(connection)? {
        register_stored_identity(
            &mut owners,
            identity,
            StoredIdentityOwner::Supplemental(supplemental),
        )?;
    }
    Ok(())
}

fn rewrite_domain_definitions(
    domain: &mut Value,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<(), StoreError> {
    let object = domain
        .as_object_mut()
        .ok_or_else(|| StoreError::Integrity("scenario domain is not an object".to_owned()))?;
    for section in ["entities", "rules", "preferences", "lockedAssignments"] {
        let records = object
            .get_mut(section)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "scenario domain section {section} is not an object"
                ))
            })?;
        let taken = std::mem::take(records);
        for (key, mut value) in taken {
            let old = Uuid::parse_str(&key).map_err(|_| {
                StoreError::Integrity(format!("definition key {key} is not a UUID"))
            })?;
            let new = mapping.get(&old).ok_or_else(|| {
                StoreError::Integrity(format!("definition key {key} was not mapped"))
            })?;
            if let Some(record_id) = value.get_mut("id")
                && record_id.as_str() == Some(key.as_str())
            {
                *record_id = Value::String(new.to_string());
            }
            if records.insert(new.to_string(), value).is_some() {
                return Err(StoreError::Integrity(
                    "duplicate identity produced while copying scenario".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn rewrite_self_declared_definitions(
    value: &mut Value,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<(), StoreError> {
    match value {
        Value::Array(values) => {
            for value in values {
                rewrite_self_declared_definitions(value, mapping)?;
            }
        }
        Value::Object(values) => {
            let taken = std::mem::take(values);
            for (key, mut value) in taken {
                rewrite_self_declared_definitions(&mut value, mapping)?;
                let rewritten_key = Uuid::parse_str(&key)
                    .ok()
                    .and_then(|old| mapping.get(&old))
                    .filter(|_| value.get("id").and_then(Value::as_str) == Some(key.as_str()))
                    .map_or_else(
                        || key.clone(),
                        |new| {
                            if let Some(id) = value.get_mut("id") {
                                *id = Value::String(new.to_string());
                            }
                            new.to_string()
                        },
                    );
                if values.insert(rewritten_key.clone(), value).is_some() {
                    return Err(StoreError::Integrity(format!(
                        "definition rewrite produced duplicate key {rewritten_key}"
                    )));
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum DeclaredReferenceKind {
    Scalar,
    List,
    External,
}

fn rewrite_declared_references(
    value: &mut Value,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<(), StoreError> {
    match value {
        Value::Array(values) => {
            for value in values {
                rewrite_declared_references(value, mapping)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                match declared_reference_kind(key) {
                    Some(DeclaredReferenceKind::Scalar) => {
                        rewrite_uuid_string(value, mapping, key)?;
                    }
                    Some(DeclaredReferenceKind::List) => {
                        let items = value.as_array_mut().ok_or_else(|| {
                            StoreError::Integrity(format!(
                                "declared internal reference list {key} must be an array"
                            ))
                        })?;
                        for item in items {
                            rewrite_uuid_string(item, mapping, key)?;
                        }
                    }
                    Some(DeclaredReferenceKind::External) => {}
                    None => rewrite_declared_references(value, mapping)?,
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn rewrite_uuid_string(
    value: &mut Value,
    mapping: &BTreeMap<Uuid, Uuid>,
    field: &str,
) -> Result<(), StoreError> {
    let text = value.as_str().ok_or_else(|| {
        StoreError::Integrity(format!(
            "declared internal reference {field} is not a string"
        ))
    })?;
    let old = Uuid::parse_str(text).map_err(|_| {
        StoreError::Integrity(format!("declared internal reference {field} is not a UUID"))
    })?;
    if let Some(new) = mapping.get(&old) {
        *value = Value::String(new.to_string());
    }
    Ok(())
}

fn declared_reference_kind(key: &str) -> Option<DeclaredReferenceKind> {
    let normalized = key
        .chars()
        .filter(|character| !matches!(character, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if matches!(
        normalized.as_str(),
        "externalid"
            | "externalids"
            | "providerexternalid"
            | "providerexternalids"
            | "sourceexternalid"
            | "sourceexternalids"
            | "legacyexternalid"
            | "legacyexternalids"
    ) {
        return Some(DeclaredReferenceKind::External);
    }
    match reference_field_tokens(key).last().map(String::as_str) {
        Some("id") => Some(DeclaredReferenceKind::Scalar),
        Some("ids") => Some(DeclaredReferenceKind::List),
        _ => None,
    }
}

fn reference_field_tokens(field: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut previous = None;
    let mut characters = field.chars().peekable();
    while let Some(character) = characters.next() {
        if !character.is_ascii_alphanumeric() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            previous = None;
            continue;
        }
        let boundary = !token.is_empty()
            && character.is_ascii_uppercase()
            && previous.is_some_and(|previous: char| {
                previous.is_ascii_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_ascii_uppercase()
                        && characters.peek().is_some_and(char::is_ascii_lowercase))
            });
        if boundary {
            tokens.push(std::mem::take(&mut token));
        }
        token.push(character.to_ascii_lowercase());
        previous = Some(character);
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn reject_stale_declared_references(
    value: &Value,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<(), StoreError> {
    match value {
        Value::Array(values) => {
            for value in values {
                reject_stale_declared_references(value, mapping)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                match declared_reference_kind(key) {
                    Some(DeclaredReferenceKind::Scalar) => {
                        reject_stale_uuid(value, mapping, key)?;
                    }
                    Some(DeclaredReferenceKind::List) => {
                        let items = value.as_array().ok_or_else(|| {
                            StoreError::Integrity(format!(
                                "declared internal reference list {key} must be an array"
                            ))
                        })?;
                        for item in items {
                            reject_stale_uuid(item, mapping, key)?;
                        }
                    }
                    Some(DeclaredReferenceKind::External) => {}
                    None => reject_stale_declared_references(value, mapping)?,
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn reject_stale_uuid(
    value: &Value,
    mapping: &BTreeMap<Uuid, Uuid>,
    field: &str,
) -> Result<(), StoreError> {
    let text = value.as_str().ok_or_else(|| {
        StoreError::Integrity(format!(
            "declared internal reference {field} is not a string"
        ))
    })?;
    let id = Uuid::parse_str(text).map_err(|_| {
        StoreError::Integrity(format!("declared internal reference {field} is not a UUID"))
    })?;
    if mapping.contains_key(&id) {
        return Err(StoreError::Integrity(format!(
            "declared internal reference {field} retained an old identity"
        )));
    }
    Ok(())
}

fn ensure_revision(expected: Revision, actual: u64) -> Result<(), StoreError> {
    let actual = checked_revision(actual)?;
    if expected != actual {
        return Err(StoreError::Conflict { expected, actual });
    }
    Ok(())
}

fn validate_stored_portable_json(value: &Value, context: &str) -> Result<(), StoreError> {
    validate_nonsecret_portable_json(
        value,
        &PortableJsonLimits {
            max_depth: eutheto_export::PORTABLE_LIMITS.max_json_depth,
            max_string_bytes: eutheto_export::PORTABLE_LIMITS.max_string_bytes,
            max_collection_items: eutheto_export::PORTABLE_LIMITS.max_collection_items,
        },
    )
    .map_err(|error| {
        StoreError::Integrity(format!(
            "stored {context} violates the nonsecret portable policy: {error}"
        ))
    })
}

fn parse_timestamp(value: &str, field: &str) -> Result<Rfc3339Timestamp, StoreError> {
    Rfc3339Timestamp::parse(value)
        .map_err(|error| StoreError::Integrity(format!("invalid stored {field}: {error}")))
}

fn parse_optional_timestamp(
    value: Option<String>,
    field: &str,
) -> Result<Option<Rfc3339Timestamp>, StoreError> {
    value
        .map(|timestamp| parse_timestamp(&timestamp, field))
        .transpose()
}
fn validate_failure_receipt_fields(
    proof: &str,
    collision_plan_sha256: &str,
    safe_reason: &str,
) -> Result<(), StoreError> {
    if proof.trim().is_empty() || proof.len() > MAX_RECEIPT_PROOF_BYTES {
        return Err(StoreError::InvalidSafetyBackupFailureReceipt(
            "proof is empty or exceeds the receipt size limit".to_owned(),
        ));
    }
    if collision_plan_sha256 == proof || safe_reason.contains(proof) {
        return Err(StoreError::InvalidSafetyBackupFailureReceipt(
            "receipt metadata contains the plaintext proof".to_owned(),
        ));
    }
    if collision_plan_sha256.len() != 64
        || !collision_plan_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(StoreError::InvalidSafetyBackupFailureReceipt(
            "collision-plan digest is not SHA-256".to_owned(),
        ));
    }
    if safe_reason.trim().is_empty() || safe_reason.len() > MAX_RECEIPT_SAFE_REASON_BYTES {
        return Err(StoreError::InvalidSafetyBackupFailureReceipt(
            "safe reason is empty or exceeds the receipt size limit".to_owned(),
        ));
    }
    Ok(())
}
fn authorize_staged_restore(
    transaction: &rusqlite::Transaction<'_>,
    binding: &PreviewBinding,
    mode: RestoreMode,
    authorization: Option<&RestoreAuthorization>,
) -> Result<(), StoreError> {
    match mode {
        RestoreMode::ImportScenario => {
            if authorization.is_some() {
                return Err(StoreError::InvalidStagedApply(
                    "ordinary import carries restore authorization".to_owned(),
                ));
            }
        }
        RestoreMode::AddBackup => {
            let authorization = authorization.ok_or_else(|| {
                StoreError::InvalidStagedApply(
                    "add-backup restore lacks authorization metadata".to_owned(),
                )
            })?;
            if authorization.prospective_failure_receipt_token.is_some()
                || authorization.collision_plan_sha256.is_some()
            {
                return Err(StoreError::InvalidStagedApply(
                    "add-backup restore carries failure-receipt metadata".to_owned(),
                ));
            }
        }
        RestoreMode::ReplaceLibrary => {
            let authorization = authorization.ok_or_else(|| {
                StoreError::InvalidStagedApply(
                    "replace-library restore lacks authorization metadata".to_owned(),
                )
            })?;
            if authorization.prospective_failure_receipt_token.is_some() {
                return Err(StoreError::InvalidStagedApply(
                    "replace-library restore carries an unnormalized receipt token".to_owned(),
                ));
            }
            if let SafetyBackupEvidence::FailedWithStrongConfirmation { proof } =
                &authorization.safety_backup
            {
                let collision_plan_sha256 = authorization
                    .collision_plan_sha256
                    .as_deref()
                    .ok_or(StoreError::SafetyBackupFailureReceiptRejected)?;
                if proof.trim().is_empty()
                    || proof.len() > MAX_RECEIPT_PROOF_BYTES
                    || collision_plan_sha256.len() != 64
                    || !collision_plan_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(StoreError::SafetyBackupFailureReceiptRejected);
                }
                let proof_sha256 = eutheto_export::sha256_hex(proof.as_bytes());
                let binding_json = eutheto_export::canonical_json(binding)
                    .map_err(|_| StoreError::SafetyBackupFailureReceiptRejected)?;
                let consumed = transaction.execute(
                    "DELETE FROM safety_backup_failure_receipts WHERE proof_sha256 = ?1 AND binding_json = ?2 AND collision_plan_sha256 = ?3",
                    params![proof_sha256, binding_json, collision_plan_sha256],
                )?;
                if consumed != 1 {
                    return Err(StoreError::SafetyBackupFailureReceiptRejected);
                }
            }
        }
    }
    Ok(())
}

fn command_source_name(source: CommandSource) -> &'static str {
    match source {
        CommandSource::Desktop => "desktop",
        CommandSource::Cli => "cli",
        CommandSource::Import => "import",
        CommandSource::System => "system",
        CommandSource::Undo => "undo",
        CommandSource::Redo => "redo",
    }
}

fn parse_command_source(value: &str) -> Result<CommandSource, StoreError> {
    serde_json::from_value(Value::String(value.to_owned()))
        .map_err(|error| StoreError::Integrity(format!("invalid stored command source: {error}")))
}

fn authorize_staged_apply(
    transaction: &rusqlite::Transaction<'_>,
    binding: &PreviewBinding,
    mode: RestoreMode,
    authorization: Option<&RestoreAuthorization>,
) -> Result<Revision, StoreError> {
    let actual_revision = library_revision(transaction)?;
    let expected_revision = binding.local_library_revision;
    if expected_revision != actual_revision {
        return Err(StoreError::LibraryConflict {
            expected: expected_revision,
            actual: actual_revision,
        });
    }
    authorize_staged_restore(transaction, binding, mode, authorization)?;
    Ok(actual_revision)
}

fn store_opaque_staged_results(
    transaction: &rusqlite::Transaction<'_>,
    results: BTreeMap<String, Vec<u8>>,
    replacements: &BTreeSet<SupplementalIdentity>,
) -> Result<(), StoreError> {
    let canonical = load_canonical_accepted_results(transaction)?;
    let mut canonical_by_id = BTreeMap::new();
    for (key, bytes) in canonical {
        let id = Uuid::parse_str(&key).map_err(|error| {
            StoreError::InvalidPersistedResult(format!("invalid canonical result ID: {error}"))
        })?;
        canonical_by_id.insert(id, bytes);
    }
    let mut statement = transaction
        .prepare("SELECT key, value FROM portable_sections WHERE section = 'results'")?;
    let stored = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut stored_by_id = BTreeMap::new();
    let mut stored_by_key = BTreeMap::new();
    for (key, bytes) in stored {
        let value: Value = serde_json::from_slice(&bytes)?;
        let id = extract_result_id(&value).map_err(|error| {
            StoreError::Integrity(format!("stored result has invalid identity: {error}"))
        })?;
        if stored_by_id.insert(id, bytes.clone()).is_some() {
            return Err(StoreError::IdentityCollision(id));
        }
        if !replacements.contains(&SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key: key.clone(),
        }) {
            stored_by_key.insert(key, id);
        }
    }

    let mut accepted = BTreeMap::new();
    let mut incoming_by_id = BTreeMap::new();
    for (key, bytes) in results {
        let value: Value = serde_json::from_slice(&bytes)?;
        validate_stored_portable_json(&value, "staged retained result")?;
        let id = extract_result_id(&value)
            .map_err(|error| StoreError::InvalidStagedApply(error.to_string()))?;
        if let Some(existing) = canonical_by_id.get(&id) {
            if existing != &bytes {
                return Err(StoreError::IdentityCollision(id));
            }
            continue;
        }
        if let Some(existing) = stored_by_id.get(&id) {
            if existing != &bytes {
                return Err(StoreError::IdentityCollision(id));
            }
            continue;
        }
        if let Some(existing) = incoming_by_id.get(&id) {
            if existing != &bytes {
                return Err(StoreError::IdentityCollision(id));
            }
            continue;
        }
        if let Some(existing_id) = stored_by_key.get(&key)
            && *existing_id != id
        {
            return Err(StoreError::IdentityCollision(id));
        }
        incoming_by_id.insert(id, bytes.clone());
        accepted.insert(key, bytes);
    }
    upsert_portable_section(transaction, SupplementalSectionKind::Results, accepted)
}

fn apply_staged_library_transaction(
    connection: &mut Connection,
    staged: StagedLibraryApply,
    applied_at: Rfc3339Timestamp,
    cancellation: &CancellationToken,
    #[cfg(debug_assertions)] failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
    #[cfg(debug_assertions)] command_commit_test_hook: Option<&CommandCommitTestHook>,
) -> Result<LibraryApplyOutcome, StoreError> {
    SqliteScenarioStore::check_command_cancelled(cancellation)?;
    let StagedApplyParts {
        import,
        remove_scenario_ids,
        settings,
        authorization,
    } = staged_apply_parts(staged)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let actual_revision = authorize_staged_apply(
        &transaction,
        &import.binding,
        import.mode,
        authorization.as_ref(),
    )?;

    let mode = import.mode;
    ensure_supplemental_replacements(
        &transaction,
        mode,
        &import.results,
        &import.shared_records,
        &import.preferences,
        &import.assets,
        &import.supplemental_replacements,
    )?;
    let no_effect = mode != RestoreMode::ReplaceLibrary
        && remove_scenario_ids.is_empty()
        && import.scenarios.is_empty()
        && import.results.is_empty()
        && import.shared_records.is_empty()
        && import.preferences.is_empty()
        && import.assets.is_empty()
        && settings.is_empty();
    if no_effect {
        SqliteScenarioStore::commit_command(
            transaction,
            cancellation,
            #[cfg(debug_assertions)]
            command_commit_test_hook,
        )?;
        return Ok(LibraryApplyOutcome {
            library_revision: actual_revision,
            created: 0,
            replaced: 0,
            removed: 0,
        });
    }

    let (outcome, retained_candidates) = replace_staged_scenarios(
        &transaction,
        mode,
        import.scenarios,
        import.scenario_revisions,
        &remove_scenario_ids,
    )?;
    store_opaque_staged_results(
        &transaction,
        import.results,
        &import.supplemental_replacements,
    )?;
    upsert_portable_section(
        &transaction,
        SupplementalSectionKind::SharedRecords,
        import.shared_records,
    )?;
    upsert_portable_section(
        &transaction,
        SupplementalSectionKind::Preferences,
        import.preferences,
    )?;
    upsert_portable_assets(&transaction, import.assets)?;
    synchronize_retained_scenario_revisions(&transaction, retained_candidates)?;
    upsert_settings(&transaction, settings)?;
    persist_portable_library_metadata(
        &transaction,
        mode,
        import.manifest_extensions,
        import.nonsemantic_extensions,
    )?;
    validate_global_identity_ownership(&transaction)?;
    #[cfg(debug_assertions)]
    consume_failpoint(failpoint, Failpoint::AfterSupplementalWrite)?;
    insert_import_provenance(
        &transaction,
        import.binding,
        import.provenance,
        &outcome.sources,
        applied_at,
    )?;
    let library_revision = increment_library_revision(&transaction)?;
    SqliteScenarioStore::commit_command(
        transaction,
        cancellation,
        #[cfg(debug_assertions)]
        command_commit_test_hook,
    )?;
    Ok(LibraryApplyOutcome {
        library_revision,

        created: outcome.created,
        replaced: outcome.replaced,
        removed: remove_scenario_ids.len(),
    })
}
fn validate_staged_scenario_revision_targets(
    connection: &Connection,
    scenarios: &[eutheto_import::StagedScenario],
    staged_history: &[ScenarioSnapshotV1],
) -> Result<(), StoreError> {
    let mut high_water = load_scenario_revision_high_water(connection)?;
    for historical in staged_history {
        validate_scenario_owned_uuid_uniqueness(historical)
            .map_err(|error| StoreError::InvalidStagedApply(error.to_string()))?;
        record_scenario_identity_owners(
            connection,
            historical.document.scenario_id,
            &collect_scenario_owned_uuids(historical),
        )?;
        high_water
            .entry(historical.document.scenario_id)
            .and_modify(|revision| *revision = (*revision).max(historical.revision))
            .or_insert(historical.revision);
    }
    for staged in scenarios {
        validate_scenario_owned_uuid_uniqueness(&staged.scenario)
            .map_err(|error| StoreError::InvalidStagedApply(error.to_string()))?;
        let scenario_id = staged.scenario.document.scenario_id;
        if let Some(highest_revision) = high_water.get(&scenario_id)
            && staged.scenario.revision <= *highest_revision
        {
            return Err(StoreError::InvalidStagedApply(format!(
                "staged scenario {scenario_id} revision does not exceed its durable high-water mark"
            )));
        }
    }
    Ok(())
}

fn replace_staged_scenarios(
    transaction: &rusqlite::Transaction<'_>,
    mode: RestoreMode,
    scenarios: Vec<eutheto_import::StagedScenario>,
    mut retained_candidates: Vec<ScenarioSnapshotV1>,
    remove_scenario_ids: &BTreeSet<ScenarioId>,
) -> Result<(StagedScenarioOutcome, Vec<ScenarioSnapshotV1>), StoreError> {
    let current_revisions = scenario_revisions(transaction)?;
    let current_ids = current_revisions.keys().copied().collect::<BTreeSet<_>>();
    if mode == RestoreMode::ReplaceLibrary && remove_scenario_ids != &current_ids {
        return Err(StoreError::InvalidStagedApply(
            "replace-library removal set no longer matches the library".to_owned(),
        ));
    }
    validate_staged_scenario_revision_targets(transaction, &scenarios, &retained_candidates)?;
    if mode != RestoreMode::ReplaceLibrary {
        retained_candidates
            .extend(read_retained_scenario_revision_rows(transaction)?.into_values());
    }
    retained_candidates.extend(scenarios.iter().map(|staged| {
        let mut scenario = staged.scenario.clone();
        scenario.project = None;
        scenario
    }));
    if mode == RestoreMode::ReplaceLibrary {
        transaction.execute("DELETE FROM portable_sections", [])?;
        transaction.execute("DELETE FROM portable_import_provenance", [])?;
        transaction.execute("DELETE FROM app_settings", [])?;
        transaction.execute("DELETE FROM retained_scenario_revisions", [])?;
    }
    for (scenario_id, revision) in &current_revisions {
        record_scenario_revision_high_water(transaction, *scenario_id, *revision)?;
    }
    for scenario_id in remove_scenario_ids {
        transaction.execute(
            "DELETE FROM scenarios WHERE id = ?1",
            [scenario_id.to_string()],
        )?;
    }
    let mut occupied = current_ids;
    for scenario_id in remove_scenario_ids {
        occupied.remove(scenario_id);
    }
    let outcome =
        apply_staged_scenarios(transaction, mode, scenarios, remove_scenario_ids, occupied)?;
    Ok((outcome, retained_candidates))
}

fn apply_staged_scenarios(
    transaction: &rusqlite::Transaction<'_>,
    mode: RestoreMode,
    scenarios: Vec<eutheto_import::StagedScenario>,
    remove_scenario_ids: &BTreeSet<ScenarioId>,
    mut occupied: BTreeSet<ScenarioId>,
) -> Result<StagedScenarioOutcome, StoreError> {
    let mut created = 0;
    let mut replaced = 0;
    let mut staged_ids = BTreeSet::new();
    let mut sources = Vec::with_capacity(scenarios.len());
    for staged_scenario in scenarios {
        let source_revision = staged_scenario.source_revision;
        let target_revision = staged_scenario.scenario.revision;
        let document = staged_scenario.scenario.document;
        let scenario_id = document.scenario_id;
        if !staged_ids.insert(scenario_id) {
            return Err(StoreError::InvalidStagedApply(
                "staged input contains a duplicate scenario identity".to_owned(),
            ));
        }
        sources.push(ScenarioImportSource {
            scenario_id,
            original_scenario_id: staged_scenario.original_id,
            source_revision,
            disposition: persisted_disposition(staged_scenario.disposition),
            id_remap: staged_scenario.id_remap,
        });
        let archived_at = if mode == RestoreMode::ImportScenario {
            if staged_scenario.disposition == StagedDisposition::Replace {
                scenario_archived_at(transaction, scenario_id)?
            } else {
                None
            }
        } else {
            staged_scenario
                .scenario
                .project
                .as_ref()
                .and_then(|project| project.archived_at)
        };
        let portable = PortableWrapperMetadata {
            required_capabilities: staged_scenario.scenario.required_capabilities,
            semantic_extensions: staged_scenario.scenario.semantic_extensions,
            extensions: staged_scenario.scenario.extensions,
        };
        match staged_scenario.disposition {
            StagedDisposition::Create | StagedDisposition::CreateCopy => {
                if !occupied.insert(scenario_id) {
                    return Err(StoreError::ScenarioAlreadyExists(scenario_id));
                }
                insert_project(
                    transaction,
                    &document,
                    target_revision,
                    archived_at,
                    &portable,
                )?;
                created += 1;
            }
            StagedDisposition::Replace => {
                let revision = target_revision;
                if remove_scenario_ids.contains(&scenario_id) {
                    if !occupied.insert(scenario_id) {
                        return Err(StoreError::InvalidStagedApply(
                            "staged input contains a duplicate scenario identity".to_owned(),
                        ));
                    }
                } else if occupied.contains(&scenario_id) {
                    let current = load_project(transaction, scenario_id)?;
                    retain_current_project_revision_if_required(transaction, &current)?;
                    replace_project_in_place(
                        transaction,
                        &document,
                        revision,
                        archived_at,
                        &portable,
                    )?;
                } else {
                    return Err(StoreError::ScenarioNotFound(scenario_id));
                }
                if remove_scenario_ids.contains(&scenario_id) {
                    insert_project(transaction, &document, revision, archived_at, &portable)?;
                }
                replaced += 1;
            }
        }
    }
    sources.sort_by_key(|source| source.scenario_id);
    Ok(StagedScenarioOutcome {
        created,
        replaced,
        sources,
    })
}

fn staged_apply_parts(staged: StagedLibraryApply) -> Result<StagedApplyParts, StoreError> {
    match staged {
        StagedLibraryApply::Import(import) => {
            if import.mode != RestoreMode::ImportScenario {
                return Err(StoreError::InvalidStagedApply(
                    "ordinary import must use import-scenario mode".to_owned(),
                ));
            }
            Ok(StagedApplyParts {
                import,
                remove_scenario_ids: BTreeSet::new(),
                settings: BTreeMap::new(),
                authorization: None,
            })
        }
        StagedLibraryApply::BackupRestore { restore, settings } => {
            let StagedBackupRestore {
                import,
                remove_scenario_ids,
                authorization,
            } = restore;
            match import.mode {
                RestoreMode::ImportScenario => Err(StoreError::InvalidStagedApply(
                    "backup restore cannot use import-scenario mode".to_owned(),
                )),
                RestoreMode::AddBackup => {
                    if !remove_scenario_ids.is_empty()
                        || authorization.safety_backup != SafetyBackupEvidence::NotRequired
                    {
                        return Err(StoreError::InvalidStagedApply(
                            "add-backup restore has destructive metadata".to_owned(),
                        ));
                    }
                    Ok(StagedApplyParts {
                        import,
                        remove_scenario_ids,
                        settings,
                        authorization: Some(authorization),
                    })
                }
                RestoreMode::ReplaceLibrary => {
                    if !authorization.destructive_action_confirmed
                        || authorization.safety_backup == SafetyBackupEvidence::NotRequired
                    {
                        return Err(StoreError::InvalidStagedApply(
                            "replace-library restore lacks destructive authorization".to_owned(),
                        ));
                    }
                    Ok(StagedApplyParts {
                        import,
                        remove_scenario_ids,
                        settings,
                        authorization: Some(authorization),
                    })
                }
            }
        }
    }
}

fn scenario_revisions(
    connection: &Connection,
) -> Result<BTreeMap<ScenarioId, Revision>, StoreError> {
    let mut statement = connection.prepare("SELECT id, revision FROM scenarios ORDER BY id")?;
    let mut rows = statement.query([])?;
    let mut revisions = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let value: String = row.get(0)?;
        let id = value.parse().map_err(|error| {
            StoreError::Integrity(format!("invalid stored scenario id: {error}"))
        })?;
        let revision = checked_revision(i64_to_u64(row.get(1)?)?)?;
        revisions.insert(id, revision);
    }
    Ok(revisions)
}

fn persisted_disposition(disposition: StagedDisposition) -> PersistedDisposition {
    match disposition {
        StagedDisposition::Create => PersistedDisposition::Create,
        StagedDisposition::CreateCopy => PersistedDisposition::CreateCopy,
        StagedDisposition::Replace => PersistedDisposition::Replace,
    }
}

fn persisted_migration(migration: AppliedMigration) -> PersistedAppliedMigration {
    let registry = match migration.registry {
        MigrationRegistryKind::Outer => PersistedMigrationRegistry::Outer,
        MigrationRegistryKind::Portable => PersistedMigrationRegistry::Portable,
    };
    PersistedAppliedMigration {
        registry,
        name: migration.name,
        from_version: migration.from_version,
        to_version: migration.to_version,
        version_space: migration.version_space,
        subject: migration.subject,
    }
}

fn delete_scenario_referencing_supplemental(
    transaction: &rusqlite::Transaction<'_>,
    scenario_id: ScenarioId,
) -> Result<(), StoreError> {
    let mut statement = transaction.prepare(
        "SELECT section, key, value FROM portable_sections WHERE section <> ?1 ORDER BY section, key",
    )?;
    let mut rows = statement.query([SupplementalSectionKind::Assets.as_str()])?;
    let mut referencing_records = Vec::new();
    while let Some(row) = rows.next()? {
        let section: String = row.get(0)?;
        let key: String = row.get(1)?;
        let bytes: Vec<u8> = row.get(2)?;
        let value: Value = serde_json::from_slice(&bytes)?;
        let scenario_references = extract_scenario_references(&value).map_err(|error| {
            StoreError::Integrity(format!(
                "stored supplemental record has invalid scenario references: {error}"
            ))
        })?;
        if scenario_references.contains(&scenario_id) {
            referencing_records.push((section, key));
        }
    }
    drop(rows);
    drop(statement);
    let mut delete =
        transaction.prepare("DELETE FROM portable_sections WHERE section = ?1 AND key = ?2")?;
    for (section, key) in referencing_records {
        delete.execute(params![section, key])?;
    }
    Ok(())
}

fn ensure_supplemental_replacements(
    transaction: &rusqlite::Transaction<'_>,
    mode: RestoreMode,
    results: &BTreeMap<String, Vec<u8>>,
    shared_records: &BTreeMap<String, Vec<u8>>,
    preferences: &BTreeMap<String, Vec<u8>>,
    assets: &BTreeMap<String, PortableAsset>,
    authorized: &BTreeSet<SupplementalIdentity>,
) -> Result<(), StoreError> {
    if mode == RestoreMode::ReplaceLibrary {
        return Ok(());
    }
    let current = load_supplemental_identities(transaction)?;
    let staged = supplemental_identities_from_keys(results, shared_records, preferences, assets);
    let collisions = current
        .intersection(&staged)
        .cloned()
        .collect::<BTreeSet<_>>();
    if collisions != *authorized {
        return Err(StoreError::InvalidStagedApply(
            "supplemental replacement authorizations do not exactly match current collisions"
                .to_owned(),
        ));
    }
    Ok(())
}

fn supplemental_identities_from_keys(
    results: &BTreeMap<String, Vec<u8>>,
    shared_records: &BTreeMap<String, Vec<u8>>,
    preferences: &BTreeMap<String, Vec<u8>>,
    assets: &BTreeMap<String, PortableAsset>,
) -> BTreeSet<SupplementalIdentity> {
    let mut identities = BTreeSet::new();
    extend_supplemental_identities(
        &mut identities,
        SupplementalSectionKind::Results,
        results.keys(),
    );
    extend_supplemental_identities(
        &mut identities,
        SupplementalSectionKind::SharedRecords,
        shared_records.keys(),
    );
    extend_supplemental_identities(
        &mut identities,
        SupplementalSectionKind::Preferences,
        preferences.keys(),
    );
    extend_supplemental_identities(
        &mut identities,
        SupplementalSectionKind::Assets,
        assets.keys(),
    );
    identities
}

fn canonical_result_ids(connection: &Connection) -> Result<Vec<Uuid>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT id FROM solutions WHERE accepted = 1 AND status = 'verified' ORDER BY id",
    )?;
    statement
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|row| {
            let id = row?;
            Uuid::parse_str(&id).map_err(|error| {
                StoreError::InvalidPersistedResult(format!("invalid solution ID: {error}"))
            })
        })
        .collect()
}

fn load_canonical_accepted_results(
    connection: &Connection,
) -> Result<BTreeMap<String, Vec<u8>>, StoreError> {
    let solution_ids = canonical_result_ids(connection)?;
    let mut results = BTreeMap::new();
    for solution_uuid in solution_ids {
        let solution_id = SolutionId::from_uuid(solution_uuid);
        let stored = load_stored_accepted_result(connection, solution_id)?;
        let bytes = serialize_checked_json(&stored.portable, "portable accepted result")
            .map_err(StoreError::InvalidPersistedResult)?
            .into_bytes();
        if results.insert(solution_id.to_string(), bytes).is_some() {
            return Err(StoreError::IdentityCollision(solution_id.as_uuid()));
        }
    }
    Ok(results)
}

fn portable_section_identities(
    sections: &PortableSupplementalSections,
) -> BTreeSet<SupplementalIdentity> {
    supplemental_identities_from_keys(
        &sections.results,
        &sections.shared_records,
        &sections.preferences,
        &sections.assets,
    )
}

fn extend_supplemental_identities<'a>(
    identities: &mut BTreeSet<SupplementalIdentity>,
    section: SupplementalSectionKind,
    keys: impl Iterator<Item = &'a String>,
) {
    identities.extend(keys.map(|key| SupplementalIdentity {
        section,
        key: key.clone(),
    }));
}

fn load_supplemental_identities(
    connection: &Connection,
) -> Result<BTreeSet<SupplementalIdentity>, StoreError> {
    let mut statement = connection
        .prepare("SELECT section, key, value FROM portable_sections ORDER BY section, key")?;
    let mut rows = statement.query([])?;
    let mut identities = BTreeSet::new();
    while let Some(row) = rows.next()? {
        let section = supplemental_section_kind(&row.get::<_, String>(0)?)?;
        let key: String = row.get(1)?;
        identities.insert(SupplementalIdentity { section, key });
    }
    drop(rows);
    drop(statement);
    for result_id in canonical_result_ids(connection)? {
        identities.insert(SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key: result_id.to_string(),
        });
    }
    Ok(identities)
}

fn load_supplemental_identity_owners(
    connection: &Connection,
) -> Result<BTreeMap<Uuid, SupplementalIdentity>, StoreError> {
    let mut statement = connection.prepare("SELECT section, key, value FROM portable_sections")?;
    let mut rows = statement.query([])?;
    let mut owners = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let section = supplemental_section_kind(&row.get::<_, String>(0)?)?;
        let key: String = row.get(1)?;
        let bytes: Vec<u8> = row.get(2)?;
        let owner = SupplementalIdentity {
            section,
            key: key.clone(),
        };
        let value = if section == SupplementalSectionKind::Assets {
            None
        } else {
            Some(serde_json::from_slice::<Value>(&bytes)?)
        };
        let exact = if section == SupplementalSectionKind::Results {
            let result = value.as_ref().ok_or_else(|| {
                StoreError::Integrity("stored result is missing its JSON value".to_owned())
            })?;
            Some(extract_result_id(result).map_err(|error| {
                StoreError::Integrity(format!("stored result has invalid identity: {error}"))
            })?)
        } else {
            Uuid::parse_str(key.split('.').next().unwrap_or(&key)).ok()
        };
        if let Some(identity) = exact {
            register_supplemental_identity_owner(&mut owners, identity, &owner)?;
        }
        if let Some(value) = value {
            for identity in collect_self_declared_uuids(&value) {
                register_supplemental_identity_owner(&mut owners, identity, &owner)?;
            }
        }
    }
    drop(rows);
    for (key, bytes) in load_canonical_accepted_results(connection)? {
        let value: Value = serde_json::from_slice(&bytes)?;
        let owner = SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key,
        };
        for identity in collect_self_declared_uuids(&value) {
            register_supplemental_identity_owner(&mut owners, identity, &owner)?;
        }
    }
    drop(statement);
    Ok(owners)
}

fn register_supplemental_identity_owner(
    owners: &mut BTreeMap<Uuid, SupplementalIdentity>,
    identity: Uuid,
    owner: &SupplementalIdentity,
) -> Result<(), StoreError> {
    if let Some(existing) = owners.get(&identity) {
        if existing == owner {
            return Ok(());
        }
        return Err(StoreError::IdentityCollision(identity));
    }
    owners.insert(identity, owner.clone());
    Ok(())
}

fn supplemental_section_kind(section: &str) -> Result<SupplementalSectionKind, StoreError> {
    match section {
        "results" => Ok(SupplementalSectionKind::Results),
        "shared_records" => Ok(SupplementalSectionKind::SharedRecords),
        "preferences" => Ok(SupplementalSectionKind::Preferences),
        "assets" => Ok(SupplementalSectionKind::Assets),
        _ => Err(StoreError::Integrity(format!(
            "invalid stored portable section: {section}"
        ))),
    }
}

fn upsert_portable_section(
    transaction: &rusqlite::Transaction<'_>,
    section: SupplementalSectionKind,
    values: BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    let mut statement = transaction.prepare(
        "INSERT INTO portable_sections (section, key, value, asset_media_type, asset_redistribution_permitted) VALUES (?1, ?2, ?3, NULL, NULL) ON CONFLICT(section, key) DO UPDATE SET value = excluded.value, asset_media_type = NULL, asset_redistribution_permitted = NULL",
    )?;
    for (key, value) in values {
        let parsed: Value = serde_json::from_slice(&value)?;
        validate_stored_portable_json(&parsed, "staged supplemental portable record")?;
        statement.execute(params![section.as_str(), key, value])?;
    }
    Ok(())
}

fn upsert_portable_assets(
    transaction: &rusqlite::Transaction<'_>,
    assets: BTreeMap<String, PortableAsset>,
) -> Result<(), StoreError> {
    let mut statement = transaction.prepare(
        "INSERT INTO portable_sections (section, key, value, asset_media_type, asset_redistribution_permitted) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(section, key) DO UPDATE SET value = excluded.value, asset_media_type = excluded.asset_media_type, asset_redistribution_permitted = excluded.asset_redistribution_permitted",
    )?;
    for (key, asset) in assets {
        statement.execute(params![
            SupplementalSectionKind::Assets.as_str(),
            key,
            asset.bytes,
            asset.media_type,
            i64::from(asset.redistribution_permitted),
        ])?;
    }
    Ok(())
}

fn upsert_settings(
    transaction: &rusqlite::Transaction<'_>,
    settings: BTreeMap<String, AppSetting<Value>>,
) -> Result<(), StoreError> {
    let mut statement = transaction.prepare(
        "INSERT INTO app_settings (key, value_json, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )?;
    for (key, setting) in settings {
        validate_stored_portable_json(&setting.value, "staged application setting")?;
        statement.execute(params![
            key,
            serde_json::to_string(&setting.value)?,
            setting.updated_at.to_string(),
        ])?;
    }
    Ok(())
}

fn insert_import_provenance(
    transaction: &rusqlite::Transaction<'_>,
    binding: PreviewBinding,
    provenance: ImportProvenance,
    scenario_sources: &[ScenarioImportSource],
    applied_at: Rfc3339Timestamp,
) -> Result<(), StoreError> {
    let binding = PersistedPreviewBinding {
        file_sha256: binding.file_sha256,
        options_sha256: binding.options_sha256,
        local_library_revision: binding.local_library_revision,
        format_version: binding.format_version,
        schema_version: binding.schema_version,
    };
    let applied_migrations = provenance
        .applied_migrations
        .into_iter()
        .map(persisted_migration)
        .collect::<Vec<_>>();
    let source_bundle_id = provenance.source_bundle_id.to_string();
    let source_application = serde_json::to_string(&provenance.source_application)?;
    let applied_migrations = serde_json::to_string(&applied_migrations)?;
    let binding = serde_json::to_string(&binding)?;
    let scenario_sources = serde_json::to_string(scenario_sources)?;
    let source_created_at = provenance.source_created_at.to_string();
    let applied_at = applied_at.to_string();
    let row_bytes = [
        source_bundle_id.len(),
        source_application.len(),
        provenance.source_file_sha256.len(),
        applied_migrations.len(),
        binding.len(),
        scenario_sources.len(),
        source_created_at.len(),
        applied_at.len(),
    ]
    .into_iter()
    .try_fold(16_u64, |total, size| {
        total
            .checked_add(u64::try_from(size).map_err(|_| StoreError::NumericRange)?)
            .ok_or(StoreError::NumericRange)
    })?;
    if row_bytes > MAX_IMPORT_PROVENANCE_BYTES {
        return Err(StoreError::InvalidStagedApply(
            "portable import provenance exceeds the retention byte policy".to_owned(),
        ));
    }
    transaction.execute(
        "INSERT INTO portable_import_provenance (source_bundle_id, source_application_json, original_format_version, original_schema_version, source_file_sha256, applied_migrations_json, binding_json, scenario_sources_json, source_created_at, applied_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            source_bundle_id,
            source_application,
            u32_to_i64(provenance.original_format_version),
            u32_to_i64(provenance.original_schema_version),
            provenance.source_file_sha256,
            applied_migrations,
            binding,
            scenario_sources,
            source_created_at,
            applied_at,
        ],
    )?;
    prune_import_provenance(transaction)
}

fn prune_import_provenance(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection.prepare(
        "SELECT id, 16 + length(CAST(source_bundle_id AS BLOB)) + length(CAST(source_application_json AS BLOB)) + length(CAST(source_file_sha256 AS BLOB)) + length(CAST(applied_migrations_json AS BLOB)) + length(CAST(binding_json AS BLOB)) + length(CAST(scenario_sources_json AS BLOB)) + length(CAST(source_created_at AS BLOB)) + length(CAST(applied_at AS BLOB)) FROM portable_import_provenance ORDER BY id",
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut total_bytes = rows.iter().try_fold(0_u64, |total, (_, bytes)| {
        total
            .checked_add(i64_to_u64(*bytes)?)
            .ok_or(StoreError::NumericRange)
    })?;
    let mut retained_count = u64::try_from(rows.len()).map_err(|_| StoreError::NumericRange)?;
    for (id, bytes) in rows {
        if retained_count <= MAX_IMPORT_PROVENANCE_ROWS
            && total_bytes <= MAX_IMPORT_PROVENANCE_BYTES
        {
            break;
        }
        connection.execute("DELETE FROM portable_import_provenance WHERE id = ?1", [id])?;
        retained_count = retained_count
            .checked_sub(1)
            .ok_or(StoreError::NumericRange)?;
        total_bytes = total_bytes
            .checked_sub(i64_to_u64(bytes)?)
            .ok_or(StoreError::NumericRange)?;
    }
    Ok(())
}

fn load_portable_library_metadata(
    connection: &Connection,
) -> Result<(BTreeMap<String, Value>, BTreeSet<String>), StoreError> {
    let row = connection
        .query_row(
            "SELECT manifest_extensions_json, nonsemantic_extensions_json FROM portable_library_metadata WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .ok_or_else(|| {
            StoreError::Integrity("portable library metadata is missing".to_owned())
        })?;
    let manifest_extensions: Value = serde_json::from_str(&row.0)?;
    validate_stored_portable_json(&manifest_extensions, "manifest extensions")?;
    Ok((
        serde_json::from_value(manifest_extensions)?,
        serde_json::from_str(&row.1)?,
    ))
}

fn persist_portable_library_metadata(
    connection: &Connection,
    mode: RestoreMode,
    incoming_manifest_extensions: BTreeMap<String, Value>,
    incoming_nonsemantic_extensions: BTreeSet<String>,
) -> Result<(), StoreError> {
    let (mut manifest_extensions, mut nonsemantic_extensions) =
        if mode == RestoreMode::ReplaceLibrary {
            (BTreeMap::new(), BTreeSet::new())
        } else {
            load_portable_library_metadata(connection)?
        };
    manifest_extensions.extend(incoming_manifest_extensions);
    nonsemantic_extensions.extend(incoming_nonsemantic_extensions);
    let manifest_value = serde_json::to_value(&manifest_extensions)?;
    validate_stored_portable_json(&manifest_value, "staged manifest extensions")?;
    connection.execute(
        "UPDATE portable_library_metadata SET manifest_extensions_json = ?1, nonsemantic_extensions_json = ?2 WHERE singleton = 1",
        params![
            serde_json::to_string(&manifest_value)?,
            serde_json::to_string(&nonsemantic_extensions)?
        ],
    )?;
    Ok(())
}
fn load_scoped_settings(
    connection: &Connection,
    keys: &[&str],
) -> Result<BTreeMap<String, AppSetting<Value>>, StoreError> {
    let mut statement =
        connection.prepare("SELECT value_json, updated_at FROM app_settings WHERE key = ?1")?;
    let mut settings = BTreeMap::new();
    for key in keys {
        let row = statement
            .query_row([key], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .optional()?;
        if let Some((json, updated_at)) = row {
            settings.insert(
                (*key).to_owned(),
                AppSetting {
                    value: serde_json::from_str(&json)?,
                    updated_at: parse_timestamp(&updated_at, "app setting updated_at")?,
                },
            );
        }
    }
    Ok(settings)
}

fn load_all_settings(
    connection: &Connection,
) -> Result<BTreeMap<String, AppSetting<Value>>, StoreError> {
    let mut statement =
        connection.prepare("SELECT key, value_json, updated_at FROM app_settings ORDER BY key")?;
    let mut rows = statement.query([])?;
    let mut settings = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let json: String = row.get(1)?;
        let value: Value = serde_json::from_str(&json)?;
        validate_stored_portable_json(&value, "application setting")?;
        let updated_at: String = row.get(2)?;
        settings.insert(
            key,
            AppSetting {
                value,
                updated_at: parse_timestamp(&updated_at, "app setting updated_at")?,
            },
        );
    }
    Ok(settings)
}

fn load_portable_sections(
    connection: &Connection,
) -> Result<PortableSupplementalSections, StoreError> {
    let mut statement = connection.prepare(
        "SELECT section, key, value, asset_media_type, asset_redistribution_permitted FROM portable_sections ORDER BY section, key",
    )?;
    let mut rows = statement.query([])?;
    let mut sections = PortableSupplementalSections::default();
    while let Some(row) = rows.next()? {
        let raw_section: String = row.get(0)?;
        let section = supplemental_section_kind(&raw_section)?;
        let key: String = row.get(1)?;
        let value: Vec<u8> = row.get(2)?;
        let media_type: Option<String> = row.get(3)?;
        let redistribution_permitted: Option<i64> = row.get(4)?;
        if section != SupplementalSectionKind::Assets {
            let parsed: Value = serde_json::from_slice(&value)?;
            validate_stored_portable_json(&parsed, "supplemental portable record")?;
        }
        match section {
            SupplementalSectionKind::Results => {
                ensure_absent_asset_metadata(
                    &raw_section,
                    media_type.as_deref(),
                    redistribution_permitted,
                )?;
                sections.results.insert(key, value);
            }
            SupplementalSectionKind::SharedRecords => {
                ensure_absent_asset_metadata(
                    &raw_section,
                    media_type.as_deref(),
                    redistribution_permitted,
                )?;
                sections.shared_records.insert(key, value);
            }
            SupplementalSectionKind::Preferences => {
                ensure_absent_asset_metadata(
                    &raw_section,
                    media_type.as_deref(),
                    redistribution_permitted,
                )?;
                sections.preferences.insert(key, value);
            }
            SupplementalSectionKind::Assets => {
                let media_type = media_type.ok_or_else(|| {
                    StoreError::Integrity("stored asset is missing its media type".to_owned())
                })?;
                let redistribution_permitted = match redistribution_permitted {
                    Some(0) => false,
                    Some(1) => true,
                    _ => {
                        return Err(StoreError::Integrity(
                            "stored asset has invalid redistribution permission".to_owned(),
                        ));
                    }
                };
                sections.assets.insert(
                    key,
                    PortableAsset {
                        bytes: value,
                        media_type,
                        redistribution_permitted,
                    },
                );
            }
        }
    }
    drop(rows);
    drop(statement);
    let mut opaque_ids = BTreeSet::new();
    for bytes in sections.results.values() {
        let value: Value = serde_json::from_slice(bytes)?;
        let result_id = extract_result_id(&value).map_err(|error| {
            StoreError::Integrity(format!("stored result has invalid identity: {error}"))
        })?;
        if !opaque_ids.insert(result_id) {
            return Err(StoreError::IdentityCollision(result_id));
        }
    }
    for (key, bytes) in load_canonical_accepted_results(connection)? {
        let result_id = Uuid::parse_str(&key).map_err(|error| {
            StoreError::InvalidPersistedResult(format!("invalid result identity: {error}"))
        })?;
        if opaque_ids.contains(&result_id) || sections.results.insert(key, bytes).is_some() {
            return Err(StoreError::IdentityCollision(result_id));
        }
    }
    Ok(sections)
}

fn ensure_absent_asset_metadata(
    section: &str,
    media_type: Option<&str>,
    redistribution_permitted: Option<i64>,
) -> Result<(), StoreError> {
    if media_type.is_some() || redistribution_permitted.is_some() {
        return Err(StoreError::Integrity(format!(
            "stored {section} record has asset metadata"
        )));
    }
    Ok(())
}

fn load_import_provenance(
    connection: &Connection,
) -> Result<Vec<PersistedImportProvenance>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT source_bundle_id, source_application_json, original_format_version, original_schema_version, source_file_sha256, applied_migrations_json, binding_json, scenario_sources_json, source_created_at, applied_at FROM portable_import_provenance ORDER BY id",
    )?;
    let mut rows = statement.query([])?;
    let mut all = Vec::new();
    while let Some(row) = rows.next()? {
        let source_bundle_id: String = row.get(0)?;
        let source_bundle_id = source_bundle_id.parse().map_err(|error| {
            StoreError::Integrity(format!("invalid stored source bundle id: {error}"))
        })?;
        let source_application_json: String = row.get(1)?;
        let original_format_version = i64_to_u32(row.get(2)?)?;
        let original_schema_version = i64_to_u32(row.get(3)?)?;
        let source_file_sha256: String = row.get(4)?;
        let applied_migrations_json: String = row.get(5)?;
        let binding_json: String = row.get(6)?;
        let scenario_sources_json: String = row.get(7)?;
        let source_created_at: String = row.get(8)?;
        let applied_at: String = row.get(9)?;
        all.push(PersistedImportProvenance {
            source_bundle_id,
            source_application: serde_json::from_str(&source_application_json)?,
            original_format_version,
            original_schema_version,
            source_file_sha256,
            applied_migrations: serde_json::from_str(&applied_migrations_json)?,
            scenario_sources: serde_json::from_str(&scenario_sources_json)?,
            binding: serde_json::from_str(&binding_json)?,
            source_created_at: parse_timestamp(
                &source_created_at,
                "import provenance source_created_at",
            )?,
            applied_at: parse_timestamp(&applied_at, "import provenance applied_at")?,
        });
    }
    Ok(all)
}

fn library_revision(connection: &Connection) -> Result<Revision, StoreError> {
    let value = connection
        .query_row(
            "SELECT value FROM app_metadata WHERE key = ?1",
            [LIBRARY_REVISION_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_else(|| "0".to_owned());
    let parsed = value.parse().map_err(|error| {
        StoreError::Integrity(format!("invalid portable-library revision: {error}"))
    })?;
    Revision::try_new(parsed).map_err(|_| {
        StoreError::Integrity("portable-library revision exceeds the version-1 bound".to_owned())
    })
}

fn load_scenario_revision_high_water(
    connection: &Connection,
) -> Result<BTreeMap<ScenarioId, Revision>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT scenario_id, highest_revision FROM scenario_revision_high_water ORDER BY scenario_id",
    )?;
    let mut rows = statement.query([])?;
    let mut high_water = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let scenario_id: String = row.get(0)?;
        let scenario_id = scenario_id.parse().map_err(|error| {
            StoreError::Integrity(format!(
                "invalid scenario revision high-water identity: {error}"
            ))
        })?;
        let revision = checked_revision(i64_to_u64(row.get(1)?)?)?;
        high_water.insert(scenario_id, revision);
    }
    Ok(high_water)
}

fn record_scenario_revision_high_water(
    connection: &Connection,
    scenario_id: ScenarioId,
    revision: Revision,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO scenario_revision_high_water (scenario_id, highest_revision) VALUES (?1, ?2) ON CONFLICT(scenario_id) DO UPDATE SET highest_revision = excluded.highest_revision WHERE excluded.highest_revision > scenario_revision_high_water.highest_revision",
        params![scenario_id.to_string(), u64_to_i64(revision.value())?],
    )?;
    Ok(())
}

fn load_scenario_identity_owners(
    connection: &Connection,
) -> Result<BTreeMap<Uuid, ScenarioId>, StoreError> {
    let mut statement = connection
        .prepare("SELECT identity, scenario_id FROM scenario_identity_owners ORDER BY identity")?;
    let mut rows = statement.query([])?;
    let mut owners = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let identity: String = row.get(0)?;
        let identity = Uuid::parse_str(&identity).map_err(|error| {
            StoreError::Integrity(format!("invalid reserved portable identity: {error}"))
        })?;
        let scenario_id: String = row.get(1)?;
        let scenario_id = scenario_id.parse().map_err(|error| {
            StoreError::Integrity(format!("invalid portable identity owner: {error}"))
        })?;
        if owners.insert(identity, scenario_id).is_some() {
            return Err(StoreError::Integrity(
                "reserved portable identity is duplicated".to_owned(),
            ));
        }
    }
    Ok(owners)
}

fn record_scenario_identity_owners(
    connection: &Connection,
    scenario_id: ScenarioId,
    identities: &BTreeSet<Uuid>,
) -> Result<(), StoreError> {
    for identity in identities {
        let existing: Option<String> = connection
            .query_row(
                "SELECT scenario_id FROM scenario_identity_owners WHERE identity = ?1",
                [identity.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            let existing: ScenarioId = existing.parse().map_err(|error| {
                StoreError::Integrity(format!("invalid portable identity owner: {error}"))
            })?;
            if existing != scenario_id {
                return Err(StoreError::IdentityCollision(*identity));
            }
            continue;
        }
        connection.execute(
            "INSERT INTO scenario_identity_owners (identity, scenario_id) VALUES (?1, ?2)",
            params![identity.to_string(), scenario_id.to_string()],
        )?;
    }
    Ok(())
}

fn ensure_scenario_id_unseen(
    connection: &Connection,
    scenario_id: ScenarioId,
) -> Result<(), StoreError> {
    let seen: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM scenario_revision_high_water WHERE scenario_id = ?1)",
        [scenario_id.to_string()],
        |row| row.get(0),
    )?;
    if seen {
        return Err(StoreError::ScenarioAlreadyExists(scenario_id));
    }
    Ok(())
}

fn increment_library_revision(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Revision, StoreError> {
    let revision = library_revision(transaction)?
        .checked_next()
        .map_err(|_| StoreError::NumericRange)?;
    transaction.execute(
        "INSERT INTO app_metadata (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![LIBRARY_REVISION_KEY, revision.value().to_string()],
    )?;
    Ok(revision)
}

fn validate_project_owned_uuid_uniqueness(
    document: &ScenarioDocument,
    revision: Revision,
    portable: &PortableWrapperMetadata,
) -> Result<(), String> {
    let mut scenario = ScenarioSnapshotV1::current(
        revision,
        document.clone(),
        portable.required_capabilities.clone(),
    );
    scenario.semantic_extensions = portable.semantic_extensions.clone();
    scenario.extensions = portable.extensions.clone();
    validate_scenario_owned_uuid_uniqueness(&scenario).map_err(|error| error.to_string())
}
fn insert_project(
    transaction: &rusqlite::Transaction<'_>,
    document: &ScenarioDocument,
    revision: Revision,
    archived_at: Option<Rfc3339Timestamp>,
    portable: &PortableWrapperMetadata,
) -> Result<(), StoreError> {
    validate_project_owned_uuid_uniqueness(document, revision, portable)
        .map_err(StoreError::InvalidScenarioIdentity)?;
    record_scenario_identity_owners(
        transaction,
        document.scenario_id,
        &collect_project_owned_uuids(
            document,
            &portable.semantic_extensions,
            &portable.extensions,
        ),
    )?;
    validate_stored_portable_json(
        &serde_json::to_value(&portable.semantic_extensions)?,
        "scenario semantic extensions",
    )?;
    validate_stored_portable_json(
        &serde_json::to_value(&portable.extensions)?,
        "scenario extensions",
    )?;
    let revision_i64 = u64_to_i64(revision.value())?;
    let document_json = serialize_document(document)?;
    let required_capabilities_json = serde_json::to_string(&portable.required_capabilities)?;
    let semantic_extensions_json = serde_json::to_string(&portable.semantic_extensions)?;
    let extensions_json = serde_json::to_string(&portable.extensions)?;
    let projection = document_projection(document);
    let scenario_id = document.scenario_id;
    transaction.execute(
        "INSERT INTO scenarios (id, domain_pack_id, domain_schema_version, title, description, revision, document_json, portable_required_capabilities_json, portable_semantic_extensions_json, portable_extensions_json, created_at, updated_at, archived_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            scenario_id.to_string(),

            projection.domain_pack_id,
            u32_to_i64(projection.domain_schema_version),
            projection.title,
            projection.description,
            revision_i64,
            document_json,
            required_capabilities_json,
            semantic_extensions_json,
            extensions_json,
            projection.created_at,
            projection.updated_at,
            archived_at.map(|value| value.to_string()),
        ],
    )?;
    record_scenario_revision_high_water(transaction, scenario_id, revision)?;
    transaction.execute(
        "INSERT INTO scenario_history_state (scenario_id, cursor_sequence, branch_generation) VALUES (?1, 0, 0)",
        [scenario_id.to_string()],
    )?;
    Ok(())
}

fn replace_project_in_place(
    transaction: &rusqlite::Transaction<'_>,
    document: &ScenarioDocument,
    revision: Revision,
    archived_at: Option<Rfc3339Timestamp>,
    portable: &PortableWrapperMetadata,
) -> Result<(), StoreError> {
    validate_project_owned_uuid_uniqueness(document, revision, portable)
        .map_err(StoreError::InvalidScenarioIdentity)?;
    validate_stored_portable_json(
        &serde_json::to_value(&portable.semantic_extensions)?,
        "scenario semantic extensions",
    )?;
    validate_stored_portable_json(
        &serde_json::to_value(&portable.extensions)?,
        "scenario extensions",
    )?;
    let scenario_id = document.scenario_id;
    let projection = document_projection(document);
    let changed = transaction.execute(
        "UPDATE scenarios SET domain_pack_id = ?2, domain_schema_version = ?3, title = ?4, description = ?5, revision = ?6, document_json = ?7, portable_required_capabilities_json = ?8, portable_semantic_extensions_json = ?9, portable_extensions_json = ?10, created_at = ?11, updated_at = ?12, archived_at = ?13 WHERE id = ?1",
        params![
            scenario_id.to_string(),
            projection.domain_pack_id,
            u32_to_i64(projection.domain_schema_version),
            projection.title,
            projection.description,
            u64_to_i64(revision.value())?,
            serialize_document(document)?,
            serde_json::to_string(&portable.required_capabilities)?,
            serde_json::to_string(&portable.semantic_extensions)?,
            serde_json::to_string(&portable.extensions)?,
            projection.created_at,
            projection.updated_at,
            archived_at.map(|value| value.to_string()),
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::ScenarioNotFound(scenario_id));
    }
    transaction.execute(
        "DELETE FROM command_journal WHERE scenario_id = ?1",
        [scenario_id.to_string()],
    )?;
    transaction.execute(
        "UPDATE scenario_history_state SET cursor_sequence = 0, branch_generation = 0 WHERE scenario_id = ?1",
        [scenario_id.to_string()],
    )?;
    transaction.execute(
        "DELETE FROM ai_conversations WHERE scenario_id = ?1",
        [scenario_id.to_string()],
    )?;
    record_scenario_identity_owners(
        transaction,
        scenario_id,
        &collect_project_owned_uuids(
            document,
            &portable.semantic_extensions,
            &portable.extensions,
        ),
    )?;
    record_scenario_revision_high_water(transaction, scenario_id, revision)?;
    Ok(())
}

fn serialize_document(document: &ScenarioDocument) -> Result<String, StoreError> {
    let json = serde_json::to_string(document)?;
    if u64::try_from(json.len()).map_err(|_| StoreError::NumericRange)?
        > MAX_SCENARIO_DOCUMENT_BYTES
    {
        return Err(StoreError::ScenarioDocumentTooLarge);
    }
    let value: Value = serde_json::from_str(&json)?;
    validate_stored_portable_json(&value, "scenario document")?;
    Ok(json)
}

fn u32_to_i64(value: u32) -> i64 {
    i64::from(value)
}

fn u64_to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::NumericRange)
}

fn i64_to_u64(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::NumericRange)
}

fn i64_to_u32(value: i64) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| StoreError::NumericRange)
}

fn checked_revision(value: u64) -> Result<Revision, StoreError> {
    Revision::try_new(value).map_err(|_| StoreError::NumericRange)
}
#[cfg(debug_assertions)]
fn consume_failpoint(
    failpoint: &Arc<std::sync::Mutex<Option<Failpoint>>>,
    expected: Failpoint,
) -> Result<(), StoreError> {
    let mut guard = failpoint.lock().map_err(|_| StoreError::ActorUnavailable)?;
    if *guard == Some(expected) {
        *guard = None;
        return Err(StoreError::InjectedFailure);
    }
    Ok(())
}
