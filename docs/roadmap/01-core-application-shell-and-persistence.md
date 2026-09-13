# Phase 01 — Core application shell and persistence

[Previous: Phase 00](00-repository-and-reproducible-tooling.md) · [Roadmap index](README.md) · Next: [Phase 02](02-domain-pack-and-planning-ir-contracts.md)

## Outcome

A real Rust application service owns typed IDs/time/units/errors/revisions, transactional SQLite persistence, versioned scenario documents, an implementation-independent portable model, safe proposed `.eutheto` scenario/full-backup bundles, migration/import/restore/recovery, command batches/journal/snapshots/undo/redo, and project lifecycle. The working CLI and generated Tauri/Vue client exercise the same service. A user can create, edit, close, reopen, duplicate, archive, delete, import/export a generic scenario, and create/restore a full portable backup without mock state or any solver/domain implementation.

## Source coverage

Blueprint §§9–11 and §29 Phase 1; relevant §§7, 24, 26, 27.11, 28, 31–33; Appendices A–D, H, I (`CORE-001`–`CORE-005`, `FOUND-005`, `SEC-001` persistence boundary), K.1/K.5/K.8, L; and the foundational slice of [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md). This phase owns the complete foundational CLI and Tauri API catalogs; later phases fill domain/solve/result/share/AI handlers without renaming them casually.

## Dependencies

[Phase 00](00-repository-and-reproducible-tooling.md) must have reproducible workspaces, generated DTO workflow, minimal Tauri capability boundary, current pins, and legal/CI skeleton. Phase 01 may define interfaces needed by phase 02, but must not depend on domain or backend implementations.

## Decisions and invariants

- `eutheto-core` is a reusable library: no CLI parsing, windows, Tauri types, ambient platform paths, or UI assumptions.
- SQLite is local authority. Authoritative pack data is one versioned document, not a domain-specific relational schema or frontend store. Never persist backend protos or frontend state.
- Every external identifier is a typed, sortable collision-resistant ID (UUIDv7 when the pinned crate implementation passes tests). Database row IDs, display names, and array offsets never escape.
- Time-based scenarios explicitly store IANA time zone, locale, horizon, DST gap/overlap policy, local template values, resolved instants, and display values. Never infer host zone. Solver duration is whole minutes in MVP.
- Every completed interaction is immediately durable through a typed command in one SQLite transaction. Transient invalid form text may remain in Vue but is not authoritative.
- All mutation sources—desktop, import, working CLI, and later AI—use one service and optimistic `expected_revision`.
- Failed writes/imports/migrations/solves cannot partially mutate projects. A failed solve never mutates a scenario; accepted-solution persistence is a later separate transaction.
- Unknown extension fields survive round trips where practical. Unknown-newer outer/database/pack versions are rejected safely; downgrades never open newer DBs silently.
- Database, portable container, portable schema, pack schema, and report/result schemas are independent version spaces. A portable bundle is never a SQLite dump, and database migrations cannot substitute for portable migrations.
- The MVP importer supports maintained historical portable versions through pure sequential migrations; the exporter always emits the current version and offers no semantic down-conversion.
- Credentials, OAuth tokens, API keys, raw secrets, unrelated paths, and logs never enter database tables or bundles.
- SQL is parameterized; foreign keys on; migrations forward-only, append-only after release, transactional and fixture-tested.
- Blocking database/file/compression work runs off async executor threads. Scenario mutation serializes per scenario; independent scenarios may operate concurrently under explicit limits.

## Core application contract

Construct dependencies explicitly:

```rust
pub struct EuthetoApp {
    store: Arc<dyn ScenarioStore>,
    domains: Arc<DomainRegistry>,
    solvers: Arc<SolverRegistry>,
    router: Arc<dyn SolverRouter>,
    verifier: Arc<VerificationService>,
    explainer: Arc<ExplanationService>,
    jobs: Arc<SolveJobManager>,
}

impl EuthetoApp {
    pub async fn execute(&self, command: AppCommand)
        -> Result<CommandResult, AppError>;
    pub async fn query(&self, query: AppQuery)
        -> Result<QueryResult, AppError>;
    pub async fn subscribe(&self, topic: EventTopic)
        -> Result<EventStream, AppError>;
}
```

Phase 01 may inject empty registries/services until their owning phases, but commands must return typed `Unsupported` rather than panic or lie. Commands/queries are versioned, coarse-grained protocol DTOs—not tables-as-API.

### Shared IDs and units

Define newtypes at minimum: `ScenarioId`, `PersonId`, `RuleId`, `AssignmentId`, `SolveRunId`, `SolutionId`, command/request IDs and pack/backend IDs. Integer-unit newtypes include `Minutes(i64)`, `Millimeters(i64)`, `Penalty(i64)`, and `Capacity(i64)`. Use checked arithmetic and reject overflow before persistence/compilation.

### Time contract

Each time scenario records IANA zone, locale, horizon, explicit ambiguity/gap policy. Recurrences use local wall time plus scenario zone; generated instances preserve UTC start/end and local display values. DST gaps explicitly reject, move forward, or use a documented pack rule; overlaps select earlier/later offset. Use `jiff` 0.2.35 rather than hand conversion. Include spring-forward/fall-back fixtures and fixed clocks.

### Typed error contract

```rust
pub enum AppError {
    Validation(ValidationReport),
    Conflict { expected_revision: u64, actual_revision: u64 },
    NotFound(ResourceRef),
    Unsupported(UnsupportedFeature),
    Solver(SolverFailure),
    Verification(VerificationFailure),
    Storage(StorageFailure),
    Protocol(ProtocolFailure),
    Ai(AiFailure),
    Internal { incident_id: Uuid },
}
```

User-safe domain/application messages are distinct from structured redacted diagnostics. Stable codes link validation issues to fields/entities/rules. Never show raw DB/solver/HTTP text as the primary message or expose Rust types/backtraces in normal DTOs.

### Jobs and resource options

Tokio runs asynchronous services. Scenario mutation serializes per scenario; independent solves may run concurrently subject to a global limit. Default later OR-Tools concurrency is one heavyweight job per installation. Each solve captures immutable revision; edits may continue, but results stay associated with that revision and become stale when current revision differs. In-process cancellation is cooperative; worker cancellation terminates process tree.

The shared options model must reconcile one canonical DTO used by phases 02–03:

```rust
pub struct SolveOptions {
    pub backend: BackendSelection,
    pub mode: SolveMode,
    pub time_limit: Duration,
    pub memory_limit_bytes: Option<u64>,
    pub worker_threads: WorkerThreadPolicy,
    pub random_seed: u64,
    pub solution_limit: Option<u32>,
    pub stop_after_first_feasible: bool,
    pub collect_intermediate_solutions: bool,
    pub explanation_mode: ExplanationMode,
    pub preserve_existing: PreservationPolicy,
    pub reproducibility: ReproducibilityMode,
    pub resource_limits: ResourceLimits,
}
```

Quick = short interactive search; Balanced/Standard = default; Deep = longer continued improvement; Advanced/Custom = explicit compatible backend/details with warnings. None implies optimality.

## Command, journal, undo, and audit contracts

### Command catalog

```rust
pub enum ScenarioCommand {
    AddEntity(AddEntity),
    UpdateEntity(UpdateEntity),
    RemoveEntity(RemoveEntity),
    AddRule(AddRule),
    UpdateRule(UpdateRule),
    RemoveRule(RemoveRule),
    SetPreference(SetPreference),
    LockAssignment(LockAssignment),
    UnlockAssignment(UnlockAssignment),
    ApplyDomainCommand(DomainCommandEnvelope),
    ApplyBatch(CommandBatch),
}

pub struct CommandEnvelope {
    pub command_id: Uuid,
    pub scenario_id: ScenarioId,
    pub expected_revision: u64,
    pub actor: ActorRef,
    pub source: CommandSource,
    pub command: ScenarioCommand,
}

pub struct CommandResult {
    pub new_revision: u64,
    pub change_set: ChangeSet,
    pub validation_delta: ValidationDelta,
    pub inverse: Option<ScenarioCommand>,
}
```

### Atomic execution

One transaction performs, in order: read/compare revision; validate envelope shape and authorization/capability; ask the pack to apply the command; run fast structural validation; persist document/delta; append journal entry and inverse; increment revision; commit. Any failure rolls back all eight effects.

Use a command journal with periodic snapshots, not distributed event sourcing:

- reversible commands store inverses;
- batch inverse reverses member order and applies atomically;
- later AI proposals apply as one named batch;
- undo executes stored inverse transactionally and moves history cursor; redo reapplies original;
- a command after undo truncates redo branch only with appropriate user confirmation;
- snapshot every configurable command count and before schema migration;
- large imports may store compressed before/after snapshots instead of thousands of inverses;
- journal reconstruction only replays pure commands from a known snapshot for repair, never arbitrary side effects.

Journal metadata: timestamp, actor/source, human summary, command type, revision before/after, import checksum, optional note, and later typed optional AI provenance containing proposal/profile identity plus provider/model identity—not credentials or conversation content. An applied AI proposal must retain its proposal/profile identity independently of the continued existence of its chat or profile configuration. Audit remains local and is excluded from bundles unless the user explicitly opts in.

[Phase 10, AI-004](10-ai-assistant-mvp.md#ordered-work-packages) owns that AI metadata extension to the existing journal, versioned DTO/schema generation and forward-only database migration when AI is implemented. Existing non-AI history remains valid without AI metadata; the applying user, base/applied revisions, batch identity/outcome, commands and inverses remain under the normal atomic journal contract. Prove migration/round-trip compatibility, apply/provenance rollback together, and history/undo after conversation or profile deletion. Do not add speculative AI columns or reopen Phase-01 production work now, and do not create a second audit/undo store.

## Persistence contract

Use `rusqlite` 0.40.2 with bundled SQLite. A dedicated actor owns the write connection; typed repository operations/closures are submitted to it. Add a read pool only after profiling proves it useful.

Connection settings:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA trusted_schema = OFF;
```

Use `FULL` for explicit backup/export durability. Database is in OS application data; logs, caches, temporary worker files, and user documents are separate platform-resolved directories.

### Initial schema

Implement at least these exact columns and constraints; add indexes for scenario recency, solve runs, accepted solutions, and conversation lookup:

```sql
CREATE TABLE app_metadata (
  key TEXT PRIMARY KEY, value TEXT NOT NULL
);
CREATE TABLE scenarios (
  id TEXT PRIMARY KEY, domain_pack_id TEXT NOT NULL,
  domain_schema_version INTEGER NOT NULL, title TEXT NOT NULL,
  description TEXT, revision INTEGER NOT NULL, document_json TEXT NOT NULL,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  last_opened_at TEXT, archived_at TEXT
);
CREATE TABLE scenario_snapshots (
  id TEXT PRIMARY KEY,
  scenario_id TEXT NOT NULL REFERENCES scenarios(id) ON DELETE CASCADE,
  revision INTEGER NOT NULL, document_json_zstd BLOB NOT NULL,
  created_at TEXT NOT NULL, reason TEXT NOT NULL,
  UNIQUE (scenario_id, revision)
);
CREATE TABLE command_journal (
  id TEXT PRIMARY KEY,
  scenario_id TEXT NOT NULL REFERENCES scenarios(id) ON DELETE CASCADE,
  revision_before INTEGER NOT NULL, revision_after INTEGER NOT NULL,
  command_type TEXT NOT NULL, command_json TEXT NOT NULL, inverse_json TEXT,
  actor_json TEXT NOT NULL, source TEXT NOT NULL, summary TEXT NOT NULL,
  created_at TEXT NOT NULL, history_sequence INTEGER NOT NULL,
  branch_generation INTEGER NOT NULL, UNIQUE (scenario_id, revision_after)
);
CREATE TABLE solve_runs (
  id TEXT PRIMARY KEY,
  scenario_id TEXT NOT NULL REFERENCES scenarios(id) ON DELETE CASCADE,
  scenario_revision INTEGER NOT NULL, input_hash TEXT NOT NULL,
  backend_id TEXT NOT NULL, backend_version TEXT NOT NULL,
  protocol_version INTEGER, status TEXT NOT NULL, options_json TEXT NOT NULL,
  model_summary_json TEXT, started_at TEXT NOT NULL, finished_at TEXT,
  elapsed_ms INTEGER, best_bound TEXT, backend_diagnostics_json TEXT,
  error_json TEXT
);
CREATE TABLE solutions (
  id TEXT PRIMARY KEY,
  solve_run_id TEXT NOT NULL REFERENCES solve_runs(id) ON DELETE CASCADE,
  scenario_id TEXT NOT NULL REFERENCES scenarios(id) ON DELETE CASCADE,
  scenario_revision INTEGER NOT NULL, status TEXT NOT NULL,
  accepted INTEGER NOT NULL, normalized_solution_json TEXT NOT NULL,
  score_json TEXT NOT NULL, verification_report_json TEXT NOT NULL,
  explanation_index_json TEXT, created_at TEXT NOT NULL
);
CREATE TABLE app_settings (
  key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE ai_conversations (
  id TEXT PRIMARY KEY,
  scenario_id TEXT REFERENCES scenarios(id) ON DELETE CASCADE,
  title TEXT NOT NULL, provider_id TEXT NOT NULL, model_id TEXT NOT NULL,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE ai_messages (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES ai_conversations(id) ON DELETE CASCADE,
  role TEXT NOT NULL, content_json TEXT NOT NULL, tool_activity_json TEXT,
  created_at TEXT NOT NULL
);
```

Timestamps are RFC 3339 UTC. Domain-local time remains in the document. No API key, credential, refresh token, or raw secret may be inserted.

### Scenario envelope

Canonical external JSON (format namespace normalized to eutheto):

```json
{
  "format": "eutheto/scenario",
  "formatVersion": 1,
  "scenarioId": "0195...",
  "domainPack": { "id": "official.workforce", "schemaVersion": 1 },
  "metadata": {
    "title": "September clinic and call",
    "description": "",
    "createdAt": "2026-08-28T23:00:00Z",
    "updatedAt": "2026-08-28T23:00:00Z"
  },
  "settings": {
    "timeZone": "America/Chicago",
    "locale": "en-US",
    "units": "us-customary"
  },
  "domain": {},
  "extensions": {}
}
```

`formatVersion` versions envelope; pack `schemaVersion` versions `domain`; IDs are stable; references are IDs; durations/geometry are documented integers. Canonical form is JSON. Working CLI may import YAML convenience but normalizes internally. Reject duplicate keys, non-finite numbers, excessive nesting, count and size violations. Unknown fields survive where practical.

### Portable model, bundle, import, backup, and restore

The binding cross-cutting contract is [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md). `.eutheto` is the current proposed one-file extension, pending the Phase-11 identity ADR; `.optplan` is no longer the working proposal. Phase 01 defines and publishes version 1 without freezing the extension/file association before that ADR.

The portable model is a strict implementation-independent representation converted explicitly to/from scenario documents and pack-owned semantic payloads. It uses stable typed UUID identities, canonical units/time zones/enums, explicit semantic capabilities, and a documented namespaced container only for preserve-through-round-trip nonsemantic extensions. Unknown Required-rule, availability, transportation, accessibility, location, unit, enum, or other semantic data blocks import.

The ZIP-compatible logical layout has fixed `manifest.json` and `checksums.json`, plus declared `scenarios/`, optional `results/`, `shared/`, `preferences/`, and permitted `assets/` payloads. The strict manifest independently declares `formatVersion`, `schemaVersion`, stable bundle ID, `scenario-export` or `full-backup` kind, UTC creation/application metadata, title/counts, compatibility/capabilities, and SHA-256 checksum-file reference. All declared payloads are checked; undeclared files are rejected except within a specifically versioned extension area. Checksums detect corruption, not sender authenticity.

Single-scenario export contains every required portable semantic fact and only referenced shared records/assets. Full backup defaults to all scenarios and revisions required by retained accepted results, referenced shared entities/templates, retained immutable results/evidence, share/report presets, portable settings, and permitted assets. Both structurally exclude credentials/tokens/keychain references, SQLite, unrelated scenarios, device paths/window state, logs, ephemeral/prohibited caches, executable content, and provider data lacking redistribution permission. Credential-free integration meaning survives as reconnection state; derived data carries provider/model/freshness/provenance and is included only when terms permit.

Every import is an untrusted staged pipeline:

1. open through the granted-file boundary and stream under centralized compressed/uncompressed/entry/path/JSON/string/count/nesting/asset/time/disk limits;
2. reject absolute/drive/UNC/traversal/NUL paths, duplicate normalized or case-colliding paths, links/devices, bombs/extreme ratios, checked-count overflow, undeclared files, malformed UTF-8 and executable/nested content not explicitly permitted;
3. validate fixed manifest, supported versions/kind/counts/capabilities and every checksum;
4. migrate outer format, then parse typed historical portable schema and apply every pure sequential offline migration;
5. validate current strict schema, stable IDs/references, units/time zones/ranges/enums/assets/capabilities and pack/domain semantics in staging;
6. show a preview with source/version/counts, migrations/warnings, reconnections, included/excluded data, preserved extensions, and all identity collisions;
7. resolve each collision as **Create a copy** with complete reference remap, **Replace existing**, or **Skip**; never merge semantic graphs automatically; and
8. commit the selected import in one SQLite transaction, retain bounded provenance, or leave the library unchanged.

Phase 01 publishes concrete cross-platform limits before the parser accepts public bundles. It prefers streaming validation to extraction into a user-writable directory. Preview identity binds file hash, versions, options, and local-library revision; change makes it stale.

Restore distinguishes **Add backup data** from **Replace current portable library**. Replace shows removal scope, requires explicit confirmation, attempts and verifies a timestamped pre-restore portable safety backup, then performs one atomic transaction/swap. If safety backup creation fails, the UI explains why and requires stronger confirmation. Fresh-install restore remains nearly one click after preview. Cancellation, I/O failure, validation failure, commit failure, or restart during restore preserves/recoverably restores the prior library.

Export/backup snapshots one consistent portable view, writes a temporary sibling, verifies declared bytes/checksums, flushes/closes, and atomically renames where supported. Failure/cancellation removes staging output. Automatic rotating backups, encryption, signatures, selected multi-scenario backup, semantic merge, and hosted storage are post-MVP.

### Autosave, startup, recovery, migrations

Completed interactions persist immediately. Long-form invalid text stays visibly unsaved in frontend state; valid text commits on submit, valid focus transition, or short idle debounce—never silently discarded.

Startup: open DB and proportionate integrity check; mark previously `running` solve runs `interrupted`; offer temp recovery only for valid manifests/checksums; never replay arbitrary side effects.

DB migrations are embedded append-only SQL, transactional, and update schema version only after success. Before potentially destructive change: create backup, record app/schema versions, retain old DB until new opens/validates, and report actionable failure plus backup path. Reject newer DB on downgrade.

```rust
pub trait DocumentMigration {
    fn from_version(&self) -> u32;
    fn to_version(&self) -> u32;
    fn migrate(&self, input: serde_json::Value)
        -> Result<serde_json::Value, MigrationError>;
}
```

Database migrations and portable migrations are separate registries. Document/pack/portable migrations are pure, deterministic, sequential, fixture-tested, offline, and retain original imported bytes until success. They do not fabricate domain facts; exact preservation is required for solver-relevant meaning, while documented historical ambiguity may produce a typed review warning. The exporter always writes the current portable schema; backward-version export is not an MVP option.

## Working CLI contract

Executable is still the unresolved working name `optimizer`. It runs without desktop, uses identical services, separates stderr diagnostics from stdout/data, never prints secrets, has stable exit codes, and accepts explicit paths where possible.

Global form and options:

```text
optimizer [--format human|json]
          [--log-level error|warn|info|debug|trace]
          [--no-color] [--data-dir <path>] [--config <path>]
          [--offline] [--version] [--help] <COMMAND>
```

JSON mode emits exactly one `eutheto/cli-result/v1` envelope to stdout. Progress is disabled unless `--progress jsonl`; progress goes to stderr or explicit destination.

Complete catalog:

```text
optimizer doctor
optimizer info
optimizer licenses
optimizer packs list
optimizer packs describe <pack-id>
optimizer projects list
optimizer projects create --pack <id> --title <title>
optimizer projects import <bundle> [--collision-plan <json>]
optimizer projects export <scenario-id> --output <bundle>
optimizer projects delete <scenario-id>
optimizer backup inspect <bundle>
optimizer backup create --output <bundle> [--exclude-results] [--exclude-large-assets]
optimizer backup restore <bundle> --mode add|replace [--collision-plan <json>]
optimizer scenario show <input>
optimizer scenario migrate <input> --output <path>
optimizer scenario validate <input>
optimizer scenario apply <input> --commands <json> --output <path>
optimizer scenario history <scenario-id>
optimizer solve <input> [--backend <id>] [--mode quick|balanced|deep]
  [--max-time <duration>] [--threads <count>] [--seed <integer>]
  [--first-feasible] [--repair-from <solution>] [--output <path>]
  [--include-diagnostics <directory>] [--progress human|jsonl|none]
optimizer solve cancel <job-id>                 # future service mode
optimizer solutions list <input-or-id>
optimizer solutions verify <scenario> <solution>
optimizer solutions compare <scenario> <solution-a> <solution-b>
optimizer solutions explain <scenario> <solution> [request options]
optimizer solutions export <scenario> <solution> --format csv|ics|svg|html|pdf|json
optimizer solvers list
optimizer solvers describe <solver-id>
optimizer solvers check <solver-id>
optimizer serve                                 # post-MVP
```

Solve sequence is load/migrate → validate → compile/route → solve → project/verify → write only accepted normalized solution → emit evidence/status/score. Infeasible is a successful execution with its specific exit code, not internal failure.

```json
{
  "apiVersion": "eutheto/cli-result/v1",
  "command": "solve",
  "ok": true,
  "status": "feasible",
  "result": {},
  "warnings": [],
  "diagnosticId": null
}
```

On error, `ok: false` adds stable safe `error.code`, `message`, details, optional diagnostic ID. Exit codes: `0` success/result; `1` unclassified app error; `2` usage; `3` validation; `4` proven infeasible; `5` no verified solution within limits; `6` backend unavailable/incompatible/failed; `7` verification correctness alarm; `8` file/bundle/DB/migration; `9` AI credential/provider command; `10` revision/state conflict; `130` user cancellation. Document shell signal transformations.

Scenario examples use the current proposed `.eutheto` extension only with a pending-identity notice until its ADR closes; never register a public association early. `optimizer serve` is post-MVP, loopback/token-authenticated by default, and is not raw Tauri API exposure.

## Tauri API contract and catalog

Every mutation includes:

```rust
pub struct MutationContextDto {
    pub scenario_id: String,
    pub expected_revision: u64,
    pub request_id: String,
}
```

Every response has request ID, applicable current revision, warnings, and stable DTO schema version. Errors:

```rust
pub struct ApiErrorDto {
    pub code: String,
    pub message: String,
    pub category: ApiErrorCategoryDto,
    pub retryable: bool,
    pub field_errors: Vec<FieldErrorDto>,
    pub details: Option<serde_json::Value>,
    pub diagnostic_id: Option<String>,
}
```

Complete stable catalog:

```text
app_get_info                    app_get_capabilities
app_get_paths_summary           app_open_data_folder
app_create_support_bundle_preview app_create_support_bundle
app_check_for_update            app_install_update
app_get_license_inventory

project_list                    project_get_metadata
project_create                  project_duplicate
project_archive                 project_unarchive
project_delete                  project_import_preview
project_import_apply            project_export_preview
project_export_create           project_backup_preview
project_backup_create           project_restore_preview
project_restore_apply           project_operation_cancel

scenario_get_summary            scenario_get_setup_status
scenario_get_view               scenario_get_entity
scenario_search_entities        scenario_get_rule_catalog
scenario_get_command_catalog    scenario_apply_command
scenario_apply_batch            scenario_validate
scenario_undo                   scenario_redo
scenario_get_history_page       scenario_migrate_preview

solve_get_backend_options       solve_estimate_model
solve_start                     solve_cancel
solve_get_job                   solve_list_runs
solve_get_diagnostics_summary

solution_list                   solution_get_summary
solution_get_view               solution_select
solution_verify                 solution_compare
solution_explain                solution_start_counterfactual
solution_cancel_counterfactual  solution_lock_assignment
solution_unlock_assignment      solution_create_repair_request
solution_export_preview         solution_export
solution_share_preview          solution_share_create
solution_export_cancel

ai_get_provider_catalog         ai_get_configuration
ai_store_credential             ai_delete_credential
ai_test_provider                ai_list_models
ai_list_conversations           ai_create_conversation
ai_get_conversation             ai_send_turn
ai_cancel_turn                  ai_get_proposal
ai_apply_proposal               ai_reject_proposal
ai_delete_conversation

settings_get                    settings_update
settings_reset_section          settings_export_nonsecret
settings_import_nonsecret
```

Phase 01 implements application/project/scenario/settings subsets; unowned calls return typed unavailable status until owning phases, never fake data. `ai_store_credential` eventually accepts a secret once, writes OS keyring, best-effort zeroizes, and returns only reference/status.

Stable events:

```text
solve://progress                solve://completed
scenario://changed              scenario://validation-changed
counterfactual://progress       ai://stream
ai://proposal-ready             ai://completed
update://available              app://notification
```

Every payload includes `eventVersion`, timestamp, request/job/scenario IDs and revision where applicable. Throttle later solver events. Only `apps/desktop/src/api` may import Tauri invoke/events, enforced by ESLint. Components use typed services/composables. Custom commands are registered for strict capability enforcement and minimum per-window scope.

## Ordered work packages

1. **CORE-001 — Values and platform interfaces.** IDs, units, checked arithmetic, fixed clock/random/filesystem/path/process/credential abstractions, time/DST model, revisions, DTO versions and typed errors.
2. **CORE-002 — SQLite service.** Dedicated write actor, pragmas, schema/indexes, typed repositories, atomic transactions, platform data path, structured redacted diagnostics.
3. **CORE-003 — Portable documents and bundles.** Internal→portable conversion seam, strict current/historical schemas, stable IDs/capabilities/extensions, version-1 manifest/layout/checksums/central limits, sequential portable migrations, current-only atomic export, staged inspect/preview/collision/atomic import, provenance, permanent fixtures.
4. **CORE-004 — Commands and history.** Envelope, batch, conflict check, pack application seam, fast validation seam, journal, inverses, snapshots, undo/redo/branch truncation and audit choices.
5. **CORE-005 — App/project/backup services.** List/create/open/archive/unarchive/delete/duplicate; consistent full portable snapshot; backup preview/create; add/replace restore preview/apply with attempted pre-restore safety backup and atomic recovery; startup integrity/interrupted jobs; DB migration/recovery registry and query view models.
6. **CLI skeleton.** Full Clap tree, global output contract/exit mapping, phase-01 real project/scenario commands, explicit not-yet-available solve behavior, examples.
7. **Generated Tauri client.** Rust DTO source, checked-in TS generation, command wrappers/event types, capability manifest, real Vue project home with conflict/empty/loading/error state.
8. **Security/recovery hardening.** Parser/fuzz limits, temp privacy/cleanup, archive path normalization, redacted support preview seam, failpoint tests and downgrade guards.

## Tests and acceptance

### Test layers

- Unit: typed IDs/units/overflow, error mapping, DST gap/overlap, canonical serialization, command inverse, parser/archive limits, path resolution, format/schema migration functions.
- Property: command + inverse restores canonical meaning/hash; current portable export/import semantic equivalence; historical import/current export/re-import equivalence; stable output independent of map/entity order; copy-mode reference remapping; batch inverse reversal; no partial transaction under injected failures.
- Permanent migration fixtures for every released DB/envelope/portable/pack version: sequential current upgrade, structured ambiguity warnings, unknown-newer/semantic refusal, nonsemantic extension preservation, interrupted rollback, backup retention, current export after migration.
- Archive adversarial fixtures: absolute/traversal/drive/UNC/backslash/NUL/Unicode normalization, duplicate/case-collision, symlink/hardlink/device, nested/executable content, bomb ratios, per-file/total/file-count/nesting/string/collection/asset/time/disk limits, checked overflow, checksum/undeclared/malformed manifest, UTF-8/content mismatch, temp cleanup.
- Import/restore: preview binding and staleness; create-copy/replace/skip; no name matching/merge; reconnection and restricted-data omission; transaction failpoints; consistent full snapshot; fresh-install restore; add/replace; attempted safety backup; restart recovery; absence of SQLite/credentials/device paths/prohibited provider data.
- Concurrency: same-scenario serialization, independent scenario progress, stale revisions/previews, busy timeout, crash between each transaction/restore-swap step, undo/new-command branch behavior.
- CLI contract: stdout/stderr separation, one JSON envelope, exact exit codes, inspect/import/export/backup/restore behavior, no secret/path leakage, explicit working paths, signal cancellation.
- Frontend/component: real project list/create/open/archive/unarchive/delete/duplicate; import/backup/restore previews and recovery; revision conflict; validation fields; keyboard/focus; empty/loading/error/offline states; no direct invoke outside API.
- Tauri E2E: first launch, persistent project across restart, command/undo/redo, current export/fresh import, full backup/fresh restore, collision choices, malformed bundle rejection, interrupted operation recovery.
- Fuzz: scenario envelope, ZIP/manifest/checksum/path normalization, current/historical portable schemas, migration chains, ID/reference remapping.

### Required scenario examples

1. Create generic `official.test` scenario at revision 0; apply add/update batch; close/reopen; assert revision/document equality and audit metadata.
2. Apply a command with stale expected revision; return conflict containing expected/actual; make no journal/document change.
3. Apply command then inverse and compare canonical document hash; undo/redo across restart; new command after undo follows explicit branch-truncation policy.
4. Export current portable scenario containing unknown declared nonsemantic `extensions.vendor.example`, selected accepted-solution metadata and optional audit disabled; import into a fresh DB and preserve semantic equivalence plus extension bytes/meaning.
5. Import the same identity through Create copy, Replace, and Skip; assert full internal-reference remap or exact replacement/omission, no name-based merge, and unchanged library after cancel/stale preview.
6. Attempt traversal/case-collision/duplicate/link/bomb/oversize/checksum/undeclared/newer-semantic failures; leave DB and destination unchanged and delete staging.
7. Create a full backup with retained result/shared records and explicit exclusions; restore to a fresh install; then test add/replace, pre-restore safety backup, injected swap/transaction failure, restart recovery, and absence of credentials/SQLite/prohibited data.
8. Inject failure after document write but before journal/revision and at database/portable migration steps; transaction rolls back, backup remains, prior app can still access retained data.
9. Spring-forward invalid local time and fall-back ambiguous time require explicit policy and yield deterministic instants independent of host zone.
10. Vue project home starts with empty real DB, creates project, reloads process, displays persisted project, handles archive/unarchive, import/backup/restore preview, and revision conflict without local authority.

### Exact exit criteria

- create/edit/reopen works through core, working CLI and real Tauri client;
- command then inverse restores canonical hash; batch/undo/redo survive restart;
- stale revision is detected before mutation;
- current portable bundle round trip preserves semantic data and declared nonsemantic extension fields; every maintained historical fixture migrates deterministically and re-exports current only;
- malicious path/link/collision/checksum/size/count/nesting/version/capability fixtures are rejected before mutation with bounded actionable errors;
- import preview and Create copy/Replace/Skip preserve stable identity/reference semantics; cancellation, staleness and injected failure leave the library unchanged;
- full backup/fresh restore and add/replace flows preserve all selected portable data, clearly exclude secrets/nonportable data, attempt a verified pre-restore safety backup, and recover atomically from every failpoint;
- DB migration backup/rollback/unknown-newer tests pass independently of portable migration tests;
- project list/open/archive/unarchive/delete/duplicate is real persistence, not mock state;
- generated Rust/TS DTOs and wrappers have no drift; API imports/capabilities obey boundary;
- startup marks interrupted jobs, validates recovery/staging artifacts and never replays side effects;
- no credentials, SQLite database, device-only path, or prohibited provider data exists in bundles/logs, verified by structural fixture scans.

## Risks and failure handling

| Failure | Handling |
|---|---|
| Partial command/write | Single transaction across document, journal, inverse, revision; failpoint proof. |
| UI/Rust divergence | Rust authority, revisioned mutation, purpose-built view models, refresh on event/conflict. |
| Bundle path/resource attack | Private temp, normalized-path/link rejection, bounded streaming, checksums, one commit, cleanup. |
| Migration destroys project | Pre-backup, retained old DB/original bytes, transactional sequential functions, downgrade refusal. |
| Async DB starvation | Dedicated blocking actor and cancellation-aware queue; profile before read pool. |
| DST/host dependence | Explicit zone/policy/local+instant storage and fixed context fixtures. |
| Unknown extensions lost | Round-trip retention tests and safe unopened export behavior. |
| Audit privacy | Local by default; explicit bundle inclusion; no secrets; preview. |
| CLI/API drift | One Rust DTO/catalog source, generated checked-in TS and contract tests. |
| Deletion mistake | Define archive/restore separately; destructive delete confirmation/atomic cascade/backup semantics. |

Stop and write an ADR if UI needs DB access, a migration cannot preserve scenarios, an API bypasses commands, platform paths leak into core, or a serialized field must be reused with changed semantics.

## Deferred and non-goals

- Domain entities/rules, planning compilation, routing, workers, verification, explanations and solve implementations belong to later phases.
- AI conversation tables exist for forward migration planning but no AI behavior/secret storage is implemented here.
- `.eutheto` is the proposed portable extension and `optimizer` remains a working CLI example; Phase 11 closes file associations/media types and public CLI stability.
- Long-lived worker/service mode, server APIs, collaboration, and cloud storage are post-MVP.
- Read pooling, event sourcing, ORM, and custom database normalization are not introduced without measured need.

## Assumption/version gates

- Confirm `rusqlite` 0.40.2 bundled feature/license and SQLite behavior on every target; record exact lock.
- Confirm `jiff` 0.2.35 DST APIs and UUID 1.26 UUIDv7 behavior with fixtures.
- Choose centralized numeric archive/JSON size, nesting, file-count, compression-ratio, support-log, snapshot interval and DB backup limits before accepting untrusted input; record them as versioned constants and API metadata. No unbounded default is permitted.
- Resolve public CLI/extension/reverse-domain only through the named gates. Serialized namespace is already `eutheto`.
- Confirm Tauri data/path APIs per target, minimum capability manifest, CSP, WebView2 strategy ownership and credential-store interface seam; secrets are not implemented here.
- Any schema released from this phase establishes migration fixtures and cannot be edited in place.

## Completion evidence

Phase 01 was locally revalidated on 2026-08-31 with the repository-owned commands:

- `just test-phase01` passed the core, store, command, import/export, CLI, and native Tauri Phase 01 suites.
- `just check` passed generated drift, protocol and fixture validation, DCO self-tests, formatting, linting, type checking, and all workspace tests.
- `nix flake check --no-update-lock-file` built all compatible local flake checks.
- `nix develop --no-update-lock-file --command just e2e` built the real unbundled Tauri application, created a project through the Vue client, started a new application process and WebDriver session, and loaded the project from the isolated persistent SQLite library.
- The four approved portable-data fuzz targets completed bounded 30-second runs with their checked-in seed corpora under `nightly-2026-08-28`.

These local runs establish the Phase 01 behavior described above. Hosted cross-platform checks for the change remain required before merge; local evidence is not a substitute for those protected-branch checks. Public CLI, extension, reverse-domain, governance/security-contact, signing, and publication identities remain unresolved at their named roadmap gates.

## Exit gate

Phase 01 exits only when all exact exit criteria and all ten scenario examples pass through real storage/client boundaries, with transactional evidence for every destructive failure. Proceed to [Phase 02](02-domain-pack-and-planning-ir-contracts.md).
