# Phase 06 — Desktop design system and workforce setup

## Outcome

Deliver a calm, accessible Tauri/Vue desktop setup experience over the authoritative headless core: first launch and local project navigation; intent-led editable-scenario import/export and full-backup/restore UI over Phase 01; semantic light/dark themes; application-owned accessible components; typed Tauri API boundary; workforce people/import/work/eligibility/availability/rule/validation editors; and complete keyboard/focus/error-state behavior. The people CSV UI consumes the completed Phase 05 detect/map/preview/validate/apply/undo/rejected-row backend. A first-time user must create and validate a small schedule without an account, AI, solver terminology, or Advanced mode.

This phase edits scenarios only through the Phase 01 command/revision boundary and the [Phase 05 workforce pack](05-workforce-core-vertical-slice.md). It consumes validation evidence from [Phase 04](04-independent-verifier-and-explanations.md). Solve/result/repair/export screens are completed in [Phase 07](07-workforce-solving-results-repair-and-export.md); seating canvas components are completed in [Phase 09](09-seating-domain-and-venue-experience.md).

This remains a local, no-account MVP setup surface: no campaign, organization, invitation, or login UI is added. Later collaborative intake is governed by [Collaborative Planning and Hosted Service — scope](collaborative-planning-and-hosted-service.md#status-authority-and-scope) and [reuse of existing authorities](collaborative-planning-and-hosted-service.md#reuse-of-existing-authorities); it must reuse these typed editors and command/revision semantics rather than introduce a second constraint editor or database. This clarification does not bypass the mandatory user pause after Phase 05 before entering Phase 06.

## Source coverage

This phase incorporates blueprint Sections 21 and 22; Phase 6; frontend dependencies in Appendix B; Tauri API standards in Appendix D; TypeScript/Vue/schema/error standards in Appendix H; UI backlog items in Appendix I; Tauri/Vue references in Appendix J; repository/desktop/domain gates in Appendix K; frontend/accessibility/E2E tests in Section 26; desktop-flow definition of done in Section 33.4; the responsive progress contract in [Performance and Solver UX Targets](performance-and-solver-ux-targets.md); and import/backup/share-preview UI foundations from [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md).

## Dependencies

- Phase 00: Vue/Vite/Tauri shell, Node/pnpm workspace, checked-in generated-contract workflow, strict lint/type/test setup, CSP/capability baseline, and exact dependency locks.
- Phase 01: project/application services, persisted settings, scenario revisions, command batches, undo/redo/history, granted-file and bounded import infrastructure, and typed API errors.
- Phase 02: pack descriptor, setup checklist, UI manifest, command/rule catalog, typed view requests, validation DTOs, and generated TS contracts.
- Phase 04: validation severity/evidence, conflict/explanation DTO foundations, verification-safe status language.
- Phase 05: workforce schema, people/qualifications/types/templates/instances, eligibility/availability/initial rules, fast/full validation and view models, plus the completed people CSV detection, mapping, preview, proposed-state validation, atomic apply, undo/redo, and rejected-row services.
- Vue never imports a domain repository or solver crate. `apps/desktop/src-tauri` is a thin adapter to `crates/eutheto-core`.

Phase 06 entry does not depend on Phase 07. Eligibility, shift, availability/time-off, and existing-assignment import formats remain accurately unavailable until Phase 07; the Phase 06 people CSV flow is fully backed by Phase 05.

## Current compatible frontend baseline

Registry/API evidence was verified **2026-08-29**. Exact non-UI lockfile pins remain Phase 00 repository actions. Phase 04 owns the minimum Tailwind/shadcn-vue/Reka/Lucide foundation needed by its explanation components; Phase 06 consumes and extends that same foundation. Implementation must use this coherent set unless a newer stable set is re-verified together:

UI001's adopted runtime/browser subset was reverified on **2026-09-09**; exact adoption evidence and the narrow declaration-only dependency repairs are recorded in [the assumptions ledger](assumptions.md). Later-phase or not-yet-consumed entries below remain adoption gates, not claims that those packages or features are installed.

| Role                                        |          Version | Compatibility and major-version implication                                                                                                                                             |
| ------------------------------------------- | ---------------: | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Node.js                                     |      24.20.0 LTS | Production LTS; satisfies Vite 8, pnpm 11, ESLint 10, Vitest 4. Do not move to Node 26 Current before its LTS/support gate.                                                             |
| pnpm                                        |          11.24.0 | Current stable; replaces the blueprint's stale pnpm 10 assumption. Requires Node `>=22.13`. Update package-manager/CI/Nix pins as one cutover.                                          |
| TypeScript                                  |            6.0.3 | Newest compatible stable, not latest 7.0.2: `typescript-eslint` 8.68.0 declares `<6.1.0`. A TS 7 or 6.1+ upgrade waits for lint-parser support.                                         |
| Vue                                         |           3.5.42 | Composition API baseline.                                                                                                                                                               |
| vue-router                                  |            5.3.0 | Breaking major from Router 4-era examples; requires Vue `^3.5.34`, Vite `^7.3                                                                                                           |     | ^8`, Pinia `^3.0.4                   |     | ^4.0.2`. Use/test Router 5 hash-history, guards, typed route behavior; do not copy Router 4 APIs unreviewed. |
| Pinia                                       |            4.0.3 | Breaking major, ESM-only, and requires `@vue/devtools-api` separately. It remains transient/view state only.                                                                            |
| `@pinia/colada`                             |            1.4.2 | Query/server-state coordination only; Rust remains authoritative and mutations still cross the typed command/revision boundary.                                                         |
| `@vue/compiler-sfc`                         |           3.5.42 | Must exactly match the Vue 3.5.42 baseline for SFC compilation.                                                                                                                         |
| `@vue/devtools-api`                         |            8.2.1 | Required separate Pinia peer; pin explicitly rather than relying on an undeclared transitive copy.                                                                                      |
| Vite                                        |            8.2.2 | Breaking major; engine `^20.19                                                                                                                                                          |     | >=22.12`, satisfied by Node 24.20.0. |
| `@vitejs/plugin-vue`                        |            6.0.8 | Compatible with Vue 3 and Vite 8.                                                                                                                                                       |
| `@tauri-apps/api`                           |           2.11.1 | Keep coherent with the selected Tauri 2 Rust crates and generated API tests.                                                                                                            |
| `@tauri-apps/cli`                           |           2.11.4 | Workspace development/build CLI; patch does not need to match JS API numerically, but the lock/manifests must be conformance-tested together.                                           |
| `@tauri-apps/plugin-updater`                |           2.10.1 | Release use is later; retain typed boundary and compatible Rust plugin/config.                                                                                                          |
| `@tauri-apps/plugin-shell`                  |            2.3.5 | Minimize use; strict scope permits only exact bundled sidecar where needed.                                                                                                             |
| Tailwind CSS / `@tailwindcss/vite`          |    4.3.3 / 4.3.3 | Tailwind 4 is CSS-first and uses the Vite plugin. Audit v3 syntax; shadcn-vue uses `tw-animate-css`, and CSS variable utilities use `var(...)` semantics rather than stale v3 snippets. |
| shadcn-vue                                  |            2.8.2 | Component generator/source, not runtime design authority. It migrated from Radix Vue to Reka; generated code is owned/reviewed by `eutheto`.                                            |
| Reka UI                                     |           2.10.4 | Current direct-registry version; use accessible headless behavior, not stale Radix Vue APIs.                                                                                            |
| `@lucide/vue`                               |           1.39.0 | Preserve the installed Phase-04 foundation pin; icons require visible/accessible labels where meaning is not decorative.                                                                |
| TanStack Vue Table                          |            9.2.4 | Breaking v9 API: use `useTable`, not v8 `useVueTable`, and configure features explicitly.                                                                                               |
| TanStack Vue Virtual                        |          3.13.36 | Stable v3 `useVirtualizer` line for measured large views.                                                                                                                               |
| Konva / vue-konva                           |   10.3.2 / 3.4.0 | Seating Phase 09; vue-konva supports Vue 3 and Konva `>7`. Treat Konva 10 as a breaking-major baseline.                                                                                 |
| ECharts / vue-echarts                       |    6.1.0 / 8.1.0 | Use only selected analytical views; vue-echarts 8 expects ECharts 6 and Vue 3. Do not copy v5 wrapper setup.                                                                            |
| ESLint                                      |           10.9.1 | Node 24 compatible; configure current flat/type-aware Vue rules.                                                                                                                        |
| `typescript-eslint`                         |           8.68.0 | Governs the TypeScript 6.0.3 ceiling (`>=4.8.4 <6.1.0`).                                                                                                                                |
| Vitest                                      |           4.1.11 | Compatible with Vite 8 and Node 24.                                                                                                                                                     |
| Vue Test Utils                              |            2.5.0 | Vue 3 component tests.                                                                                                                                                                  |
| Testing Library Vue                         |            8.1.0 | User-centric accessible queries.                                                                                                                                                        |
| axe-core                                    |           4.13.0 | Automated accessibility aid; manual keyboard/screen-reader scripts remain required.                                                                                                     |
| `@vitest/browser-playwright` / `playwright` |  4.1.11 / 1.62.1 | Real Chromium component interaction and axe checks; not native IPC, platform or installer evidence.                                                                                     |
| Native W3C WebDriver runner                 | Repository-owned | Existing dependency-free Node 24 runner; `just e2e` exercises Linux native unbundled Tauri/WebKit persistence across application-process restarts.                                      |

`@pinia/colada` **1.4.2**, `@vue/compiler-sfc` **3.5.42**, `@vue/devtools-api` **8.2.1**, and `@lucide/vue` **1.39.0** are mandatory direct dependencies at these exact versions, not undeclared transitive dependencies.

Tailwind CSS, `@tailwindcss/vite`, shadcn-vue, Reka UI, `@lucide/vue`, `tw-animate-css`, `tailwind-merge`, `class-variance-authority`, and `clsx` are locked and minimally configured by Phase 04. Phase 06 must consume and extend those exact pins, application-owned wrappers, and mapped semantic tokens rather than re-locking dependencies or introducing a parallel design convention.

Tauri remains major **2**. The Rust crate patch/version set is pinned alongside npm packages after checking current compatible releases. Breaking-major adoption is clean-cut: no compatibility aliases or mixed old/new APIs in examples, generated code, configuration, or tests.

WebDriverIO and `@wdio/tauri-service` were not adopted. Extend the existing native runner rather than adding a second automation stack. Use `just frontend-browser-install` and `just frontend-browser-test` for the supplemental Chromium suite; keep manual native picker/screen-reader and later exact packaged-artifact/platform evidence distinct.

## Authority and trust boundary

### Rust owns

- scenario document and current revision;
- structural/full validation results;
- command journal, undo/redo, audit history;
- persisted settings;
- solve jobs and backend availability;
- solutions and verification reports;
- AI conversations and applied proposals.

### Vue owns

- current route/workspace tab;
- selected entity/assignment/seat;
- panel sizes/collapsed state;
- canvas viewport/zoom/temporary drag state;
- unsubmitted form text/edit buffers;
- table sort/filter state;
- transient notifications;
- accessibility focus restoration.

Never create a mutable long-lived Pinia copy of the scenario. Query a typed view, submit a command with `expectedRevision`, then reconcile the returned change/view delta. A watcher must not cause hidden scenario mutation. Completed interactions commit promptly; form typing may remain transient until the explicit commit boundary.

## Desktop architecture

### Source ownership

The following is an ownership illustration, not a required directory scaffold. Preserve existing entry files and consumers; add a feature path only when its owning phase implements real behavior. Project-generated frontend contracts remain under `src/api`, not a second `src/generated` tree.

```text
apps/desktop/
├── src/
│   ├── app/                 App.vue, router.ts, shortcuts.ts, error-boundary.ts
│   ├── api/                 client.ts, commands.ts, queries.ts, events.ts, errors.ts
│   ├── components/
│   │   ├── ui/              reviewed application-owned shadcn-vue source
│   │   ├── planner/
│   │   ├── workforce/
│   │   └── seating/
│   ├── features/            projects, scenario-editor, validation, solving,
│   │                        explanations, ai-assistant, import-export, settings
│   ├── views/
│   ├── stores/
│   ├── composables/
│   ├── styles/
│   ├── i18n/
│   └── test/
├── src-tauri/
│   ├── capabilities/
│   ├── icons/
│   ├── src/                 commands/, events.rs, app_state.rs, lib.rs
│   ├── tauri.conf.json
│   └── Cargo.toml
├── public/
├── package.json
├── vite.config.ts
└── vitest.config.ts
```

Business logic stays in `crates/application`/domain crates. `src-tauri` adapts typed commands, state, events, file-picker grants, and capabilities.

### API conventions

Mutations include:

```rust
pub struct MutationContextDto {
    pub scenario_id: String,
    pub expected_revision: u64,
    pub request_id: String,
}
```

Every response echoes request ID, current revision where applicable, warnings, and stable DTO schema version. Errors use stable code, safe message, typed category, retryability, field errors, optional safe details, and optional diagnostic ID; they do not expose Rust names/backtraces.

Coarse-grained endpoints include:

```text
app_get_paths_summary
app_get_license_inventory

settings_get
settings_update
settings_reset_section
settings_export_nonsecret
settings_import_nonsecret

project_list
project_open
project_get_metadata
project_create
project_duplicate
project_archive
project_unarchive
project_delete
project_import_preview
project_import_apply
project_export_preview
project_export_create
project_backup_preview
project_backup_create
project_restore_preview
project_restore_apply
project_operation_cancel

scenario_get_summary
scenario_get_setup_status
scenario_get_view
scenario_get_entity
scenario_search_entities
scenario_get_rule_catalog
scenario_get_command_catalog
scenario_apply_command
scenario_apply_batch
scenario_validate
scenario_undo
scenario_redo
scenario_get_history_page
scenario_migrate_preview

solve_get_backend_options
solve_estimate_model
solve_start
solve_cancel
solve_get_job
solve_list_runs
solve_get_diagnostics_summary
```

The `/settings` route uses the five `settings_*` endpoints above, and `/about/licenses` uses the bounded offline `app_get_license_inventory` result. `app_get_paths_summary` exposes configured/unconfigured categories, not device paths or proof that directories exist, are accessible, or are writable; Vue must not reconstruct paths from these flags. Support-bundle and updater APIs are intentionally completed in [Phase 11](11-public-mvp-packaging-and-documentation.md), not improvised in this phase.

Project creation, listing, and opening use closed V1 requests. Workforce creation accepts first and last included civil dates; Rust resolves exact local midnights, checks the exclusive end date, and rejects shifted or skipped boundaries before committing. The versioned list/open projection adds `lastOpenedAt`; listing is read-only, and opening records its timestamp only if loading succeeds. The CLI retains its RFC3339 creation arguments and original project-list JSON shape.

Local settings reads return complete nullable `appearance`, `locale`, and `units` entries at one library revision. Updates and resets require `expectedLibraryRevision` and return the exact committed snapshot and `changed` result. Stale writes fail without retry; an absent-key reset neither advances the revision nor emits a change notification. Local values remain readable under local validation even when stricter portable policy prohibits exporting them.

Portable import, export, backup, restore, unopened-bundle inspection, and exact re-export use closed V1 requests through operation admission. Reviews bind to the owning window, creator request/operation, exact purpose, and captured library/scenario revisions as applicable. The generated `PortableReviewFlow` owns retained reviews and actual operation settlement; a cancellation acknowledgement is not a terminal result. Native custody retains quota charges during active consumption and cleanup, and creator/window teardown rejects late publication of a review. Compact portable payloads remain bounded at 64 MiB, with a separate fixed wire bound; these limits do not widen other IPC contracts.

Restore retry authority comes from the actually retained core review after a safety-backup failure. The native boundary reports the typed `portablePreviewRetained` evidence only after successful handback; clients must not infer it from an error code. Committed restore receipts distinguish `notRequired`, `createdAndVerified` with the actual artifact basename, and `confirmedBypass`. Recovery selection accepts only `userSelected` or `safetyBackups` origin, never a renderer-provided path. Cancellation before the final commit check aborts mutation; cancellation after commit preserves the committed outcome. A verified safety backup published before a later cancellation or conflict remains available.

Commands reject arbitrary SQL, shell strings, ungranted paths, and unallowlisted solver parameters. File operations use an explicit picker/grant result. Request/response uses commands; bounded progress/change streams use channels/events. Every event includes `eventVersion`, timestamp, request/job/scenario IDs, and revision where applicable. Phase 06 consumes `scenario://changed`, `scenario://validation-changed`, `solve://progress`, `solve://completed`, and `app://notification`.

Only `apps/desktop/src/api` may import Tauri invoke/event APIs; ESLint import restrictions enforce this. Components call typed composables/services. Tauri capabilities use minimum permissions per window and explicit custom-command manifest registration; `invoke_handler` alone must not be mistaken for strict window scoping.

### Generated contracts

Rust application DTOs and approved pack contracts remain authoritative. Extend the existing `xtask` generation pipeline and its checked-in products:

```text
apps/desktop/src/api/generated.ts
apps/desktop/src/api/generated-domain-pack-contracts.ts
```

The existing command/client/event/version projection and pack-contract generators own these products; do not introduce a parallel generated directory or a new generator merely to match an illustrative layout. Seating contracts are added only with their Phase-09 authority. Generated files are never hand-edited; `just generate` regenerates and `just generate-check` rejects drift. Untrusted boundary values are `unknown` and validated; application code avoids `any`. Tauri-owned `src-tauri/gen/schemas/` and `permissions/autogenerated/` are ignored native build output, not checked-in `xtask` DTO products; see [generated artifact ownership](../architecture/generated-artifacts.md).

### Routes

Use Vue Router 5 in Tauri-compatible hash SPA mode unless a tested memory mode is required:

```text
/
/projects
/project/:scenarioId/setup
/project/:scenarioId/people
/project/:scenarioId/work
/project/:scenarioId/rules
/project/:scenarioId/results/:solutionId?
/project/:scenarioId/history
/settings
/settings/backup-restore
/about/licenses
```

Seating later maps equivalent workspace routes to `venue`, `guests`, `relationships`, and `arrangement`. Route guards handle unsubmitted temporary form text; they never block already committed commands. Unknown/missing/stale scenario routes have recovery states.

### Pinia boundaries

- `workspaceStore`: open tabs, selected scenario, recent projects;
- `viewStateStore`: panels, selection, zoom, filters;
- `solveUiStore`: active job IDs and throttled progress presentation;
- `notificationStore`: transient notices;
- `settingsViewStore`: edit buffers before settings commit.

There is no universal `scenarioStore`. Query composables refresh invalidated views after commands.

### Purpose-built views

```rust
pub enum DomainViewRequest {
    Overview,
    EntityPage { kind: String, cursor: Option<String>, filter: String },
    ScheduleWindow { start: Instant, end: Instant, people: Vec<PersonId> },
    RuleList { category: Option<String> },
    SeatingViewport { bounds: RectMm, zoom_bucket: u8 },
    SolutionSummary { solution_id: SolutionId },
}
```

A command response includes changed entity/rule IDs and invalidated view keys. Reload complete small views first; add deltas only when profiling shows value. Large pages are cursor/window based and virtualized.

## Design system

### Semantic foundations

Define before domain pages:

```text
color.background
color.surface
color.surfaceRaised
color.text
color.textMuted
color.border
color.focus
color.required
color.preference
color.success
color.warning
color.error
color.info
color.selection
```

Also define typography, spacing, radii, elevation/shadows, motion durations, grid sizing, and chart/canvas semantic styles. Light/dark themes share tokens. Meet WCAG AA for normal text and important controls; required/preference/status is never hue-only. Prefer one accent plus restrained semantic colors, borders only for useful grouping, and avoid cards nested in cards. Use platform-appropriate chrome/density without pretending web controls are native toolkit widgets.

shadcn-vue output under `components/ui` is application source: remove unused variants, replace stale Tailwind syntax, style only through semantic tokens, verify Reka behavior, and keep one class-composition convention.

### Reusable component catalog

Build in Phase 06 where used, or establish typed/accessibility contracts for later phases:

```text
EntityPicker
EntityMultiPicker
RuleCard
RuleStrengthControl
RuleScopeBuilder
DurationField
DateTimeRangeField
EligibilityMatrix
AvailabilityCalendar
ValidationSummary
ConflictCard
SolveModePicker
SolveProgress
SolutionStatus
ScoreBreakdown
ExplanationPanel
AssignmentInspector
LockControl
ChangeSetPreview
UndoHistory
ImportMappingTable
EmptyState
ErrorRecoveryPanel
```

Domain components are `WorkforceScheduleGrid`, `PersonTimeline`, `CoverageInspector`, `FairnessDistribution`, `VenueCanvas`, `TableEditor`, `SeatInspector`, `GuestRelationshipEditor`, and `GeometryOverlayLegend`. Phase 06 implements setup-facing components; later phases implement result/canvas behavior without creating a second design convention.

#### Common-field consumer contracts

Package 4 supplies controlled presentation fields, not a production editor playground. The first live consumers are the setup overview's fast `ValidationSummary`, the workspace `History` route, and empty library/history states. People/import, work/shift, eligibility/availability, and rule-editor bindings remain ordered packages 5–8.

- `EntityPicker` / `EntityMultiPicker` use kind-qualified `DomainEntityRef` identity and separate retained labels. Options are one caller-owned native page, at most 200 items. Both captured query and per-invocation request key must match; typing invalidates old options before parent echo. The key covers scenario/revision, complete source/query, kind, limit and continuation. The caller owns native operation cancellation and admission; no browser entity creation or unbounded option accumulation.
- `DurationField` emits one atomic `DurationDraft`: raw text/unit and empty, invalid, or valid whole minutes. Only valid drafts carry minutes. Decimal hours convert exactly within the native U32 range; no rounding or retained last-valid value. `DateTimeRangeField` likewise emits one atomic `TemporalDraft`, preserving local interval precision and the full local-window day-offset range 0–255. A complete candidate is only ready for native validation; Rust resolves syntax, timezone and DST.
- `RuleStrengthControl` emits Required/Preference intent constrained by the native catalog, with the generated preference-priority enum. It does not implicitly convert a saved rule. `RuleScopeBuilder` edits `WorkforceScope` immutably, preserving exact tag entries and absent versus explicitly empty filters. Only a matching native `WorkforceSetupScopeInspection` supplies counts/population. Its request key includes immutable scenario/revision, complete stored/command-preview source, rule, part, axis, limit and continuation; every edit or invocation invalidates the captured preview synchronously.
- `ImportMappingTable` edits `PeopleCsvColumn` values by zero-based physical index, not header text. It preserves existing invalid rows and blank policies until explicitly repaired. Local structural feedback covers width 1–64, at most 10 mappings, duplicate/out-of-range indices, duplicate fields, and disallowed Clear policies; native `invalidMapping` remains summary-only because it has no column field path. Inputs are bounded native header metadata and explicitly selected inert samples, never paths, raw files or a browser CSV parser. Package 5 owns dialect/defaults/reference and identity review, native preview, apply and report lifecycle.
- `UndoHistory` consumes `HistoryPageDtoV1` from `getScenarioHistoryPage`: 50 entries per UI page, hard limit 100, 4 KiB request and exact 4 MiB response ceilings. Metadata excludes command/inverse/actor payloads; a null summary explicitly means it exceeded 4096 UTF-8 bytes, distinct from an empty recorded summary. Revision-bound continuations page retained journal entries, not an archive of discarded branches. One-step Undo/Redo intent flows through `ProjectWorkspace` to the existing root operation owner, never arbitrary entry replay. Busy, archived and unsupported contexts disable mutation; revision/scenario/library-epoch changes and disposal invalidate pending reads. Core/CLI full history remains a separate existing consumer.
- Preserve all eight explanation components: `SolutionStatus`, `ScoreBreakdown`, `ExplanationPanel`, `AssignmentInspector`, `ConflictCard`, `ChangeSetPreview`, `ValidationSummary`, and `ErrorRecoveryPanel`. Fast validation shows native total/displayed/omitted counts without inventing full-validation findings; static findings are not buttons. Conflict and comparison components retain their infeasibility/solution evidence contracts rather than receiving fabricated setup data. Error recovery actions require explicit capabilities; a diagnostic ID alone does not authorize download or support export.

Every field preserves linked label/help/error semantics, keyboard interaction, read-only/disabled behavior and meaningful focus after removal, paging or recovery. Shared field types and messages live in `apps/desktop/src/components/planner`; generated DTOs remain native-authoritative. These contracts do not close the phase-wide accessibility/performance or maintainer manual-testing gates.

#### Later component contracts, not mounted placeholders

| Component / first owner                                                      | Typed producer and interaction contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | Accessibility and lifecycle                                                                                                                                                                                                                                                                                                                                                                                                                 |
| ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `RuleCard` — package 8                                                       | Consume generated `WorkforceSetupRuleCatalogResult`, `WorkforceSetupRuleDetailResult` and stable `{class, ruleId}` references. `official.workforce.setup.rule_scope_summary` reports effective authored populations for implemented Required rules: minimum-rest people intersect main, before and after; each shift role intersects main with that role. Compose strength/scope fields; emit explicit edit, preview, enable/disable and delete intents through typed native commands, not local feasibility or implicit class conversion. | Named expand/edit controls, Required/Preference and inactive status in text, linked validation with exact field focus, focus returned to the edited rule or adjacent item after deletion. Prevent active empty populations from current exact-command evidence; preserve native rejection of literal-empty active selections. Captured revision/source governs preview and apply; loading, stale and native error states cannot look saved. |
| `EligibilityMatrix` — package 7                                              | Consume `WorkforceSetupEligibilityMatrixResult` from `official.workforce.setup.eligibility_matrix` using stable person/assignment-type identities for configured membership, not feasibility. `official.workforce.setup.people_page` adds native name and recorded-qualification-grant filtering; actual person/shift blocking evidence comes from assignment inspection. Bulk changes are reviewed typed commands, never a browser eligibility calculation.                                                                               | Keyboard row/cell navigation with an equivalent accessible table/list, explicit row/column names and state beyond color. Preserve focus by identity across bounded pages; invalidate stale cells and bulk previews, expose cancellation/error/empty states.                                                                                                                                                                                 |
| `AvailabilityCalendar` — package 7                                           | Consume `WorkforceSetupAvailabilityWindowResult` from `official.workforce.setup.availability_window`; use `official.workforce.setup.availability_records` for the per-person authored list, including records outside the visible dates or without occurrences. Edit generated local temporal drafts and typed commands. Rust resolves intervals and DST; scenario timezone is explicit.                                                                                                                                                   | Equivalent list editor for every calendar operation, keyboard add/edit/remove, named intervals, linked native errors and restored focus. Draft/source changes invalidate previews; cancellation cannot undo a committed mutation.                                                                                                                                                                                                           |
| `SolveModePicker` / `SolveProgress` — package 9 shell, Phase 07 live solving | Bind native backend/mode capability, versioned requests and invocation/job events when the solve adapter is available. The current registered-but-unavailable solve endpoints are not usable producers. Exact enabled modes and progress DTO bindings must be settled with that adapter, not replaced by simulated progress.                                                                                                                                                                                                               | Keyboard-complete named mode controls, textual limits and unsupported reasons, truthful coarse phases, 300–500 ms perceptual threshold, bounded announcements and explicit cancellation. Keep cancellation request separate from terminal outcome; filter by job/scenario/input revision and retain stale-result labeling.                                                                                                                  |
| `LockControl` — Phase 07 repair                                              | Bind accepted base-solution/assignment identity and native hard/soft/unlocked semantics. Emit reviewed typed lock/unlock intents; convenience endpoints must remain ordinary revisioned scenario commands. Their repair UI/adapter is not implemented by this package.                                                                                                                                                                                                                                                                     | Named keyboard choices with textual consequences, equivalent list actions, conflict/error feedback and focus restoration. No saved-lock claim until authoritative commit; stale base or revision requires fresh review.                                                                                                                                                                                                                     |
| Share Result inclusion/privacy summary — Phase 07 export                     | The existing pack hook takes `ShareResultOptions.includeEvidenceReferences` (default false). Its current Workforce payload contains immutable scenario/revision/solution/checksum and verification provenance, selected assignment identities, authoritative score and optional evidence references. IDs are pseudonymous, not anonymous. The richer recipient profiles, names/aliases/addresses/notes/section switches and desktop preview/create DTOs remain unresolved Phase 07 production contracts.                                   | Keep Share Result separate from editable scenario/backup. Present the exact native preview, inclusions/exclusions and revision/verification basis; choices are named keyboard controls, sensitive additions require explicit choice, and generation has cancellation/error/focus behavior. Never infer privacy from the portable summary or claim a participant-ready report from the current identity-only contribution.                   |

`PortableEvidence` continues to render actual portable import/export/backup inclusion and exclusion evidence only. It neither satisfies nor mounts the future Share Result preview. Phase 07 must meet its [Share Result model, privacy, offline renderer and export gates](07-workforce-solving-results-repair-and-export.md#share-result-model-and-renderer) without broadening portable authority or inventing frontend-only export capabilities.

## Workforce setup UX

### First launch and project home

First launch asks “What would you like to plan?” with **Work schedule**, **Event seating**, and **Open an existing project**. School scheduling may be visibly “Coming later” only as non-clickable discovery. No account, analytics consent, or AI setup is required.

Project home shows title/domain, last opened, horizon/event date, validation/solution status, and clear local-storage indicator. Actions are open, duplicate, **Export editable scenario**, archive, and delete. Deletion has explicit confirmation, uses OS trash/recovery when feasible, and offers current portable export before permanent deletion.

**Open an existing project** inspects the proposed `.eutheto` bundle before mutation and shows kind/source/version/counts, migrations/review warnings, integration reconnections, included/excluded data, and explicit Create copy/Replace/Skip collisions. **Backup & Restore** offers **Back up everything**, lists portable inclusions and credential/device/cache exclusions, then distinguishes **Add backup data** from **Replace current portable library**. Replace summarizes removal, requires confirmation, reports the attempted pre-restore safety backup, and never implies recovery exists when its creation failed. Every preview is stale-detected, cancellable, keyboard complete, screen-reader announced without spam, and followed by a success/recovery action.

### Guided but non-rigid setup

Workforce checklist:

```text
1. People                 Complete
2. Work to cover          Complete
3. Eligibility            2 issues
4. Required rules         Complete
5. Preferences            Optional
6. Validate               Not run
```

Creation initially guides horizon/timezone, people, work, eligibility, availability/time off, required rules, preferences/fairness, and validation. After creation users can navigate freely; it is not a rigid wizard. Status comes from Rust pack queries and updates after manual or AI-issued commands.

### People and import

People editor covers stable/external ID, name, active range, qualifications, eligible assignment types, optional home location/contract target, teams/tags, and display-only metadata. Bulk and keyboard workflows are first-class.

Import always performs: choose file; safe format/encoding detection; column mapping or bundle summary; preview additions/updates/duplicates/rejected rows; resolve identity matching; validate proposed changes; apply one atomic command batch; show report with downloadable rejected-row details. Similar names are never silently merged. Cancellation before apply leaves scenario untouched; apply is one undo step.

Detection, parsing, identity resolution, proposed-state validation, preview binding, atomic batch creation, and rejected-row report generation run in the Phase 05 Rust service. Phase 06 owns only file selection/grant, mapping and identity-decision UI, preview/report presentation, and calls through the generated typed API. Apply carries `expectedRevision` and `previewId`; stale or changed previews must be regenerated rather than reconstructed in Vue.

### Work and calendar editor

Choose horizon and IANA timezone, explicit DST review policy, preset such as `Clinic + on-call`, assignment types, recurring templates, location, local start/end including next-day end, exact/min/max coverage, and qualification slots. Show generated instances and regeneration diff; detached manual instances are not overwritten silently. Ambiguous/nonexistent local times link to exact template/instance fields and show wall/elapsed duration when different.

Settings changes preserve the exact instants and elapsed durations of all stored manual and detached instances, including those outside the current horizon; local values and offsets are re-expressed only when required by the new timezone/DST policy. Recurring templates retain local-time intent. Rust owns preparation, preview, atomic command application, and exact inverse capture, so undo restores original endpoint representations rather than reconstructing lost gap-local intent from an instant. The setup UI must review these effects explicitly; changing a timezone is not permission to move existing stored work.

### Eligibility and availability

`EligibilityMatrix` supports large-data virtualization, sticky/semantic row/column headers, bulk actions with preview, searchable people/types/qualifications, keyboard cell actions, and detail inspector. It does not duplicate the scenario in Pinia.

`AvailabilityCalendar` plus list editor supports unavailable intervals, recurring weekly rules, available-only windows, approved time off, effective dates, type/location restrictions, source/note, and accessible non-calendar editing. Drag creation has keyboard equivalents. Users can inspect which availability rule blocks a person/shift pair.

### Plain-language rule builder

Start from intent categories:

```text
Availability
Rest between work
Hours and workload
Coverage
Eligibility and qualifications
Consecutive work
Fairness
Assignments together or apart
Location or travel
Other advanced rule
```

The selected rule becomes a sentence form, for example:

```text
[ Everyone eligible for both ] needs at least [ 10 ] [ hours ]
after [ Overnight call ] before [ Clinic ].

[ Required ▼ ]
```

Below it, preview all currently scoped people and assignment types. An empty scope cannot save accidentally; the only alternative is an explicit inactive-state choice. `Required` means the app will not accept a solution that breaks it. `Preference` means optimization tries to honor it and reports tradeoffs. Explain these labels on first use/help; “hard/soft” stays in developer diagnostics.

Preferences retain the established `Low`, `Normal`, `High`, and `Very high` priority labels and recorded score-policy mapping from [Phase 05](05-workforce-core-vertical-slice.md#complete-mvp-preference-catalog-completed-in-phase-07-unless-needed-by-a-phase-05-fixture); Phase 07 completes the later catalog rather than becoming a prerequisite for this editor. Later participant-requested treatment and coordinator review are collaboration-layer decisions, not new rule strengths or a replacement for Required/Preference; accepted effects use the existing typed commands and scenario revisions under the [identity, input-authority, and policy contract](collaborative-planning-and-hosted-service.md#identity-input-authority-and-policy).

Generate the authoritative preview from validated typed commands, not free-form prose: show before/after values, affected identities/counts, explicit units, effective dates and scenario timezone, Required/Preference behavior, and unresolved choices. A name is not identity, and a shift-sequence restriction is not interchangeable with elapsed minimum rest. Keep draft edits, the applied scenario revision, validation state, and later accepted results visibly distinct. Phase 10 reuses these semantics for proposal cards; this phase does not build a chat panel or microphone controls.

Only Phase 05-complete rule types are enabled as executable normal-flow choices. Later catalog items may appear only with accurate unavailable status; never save a rule the core will ignore.

The [Phase 07 practitioner scheduling decision ledger](07-workforce-solving-results-repair-and-export.md#practitioner-scheduling-clarifications) also governs editor language. Distinguish Required exact/minimum/maximum/ranged totals from preferred targets; distinct weekends worked from weekend shifts/hours; grouping newly selected work from adjacency to a base schedule; and predefined partial shifts from optimizer-selected linked alternatives. Show units, period/membership, population, strength, and unresolved policy choices explicitly. Phase 06 must not invent defaults, speculative controls or replacement schemas for unanswered requirements. Once the corresponding Rust contract and capability land in Phase 07, extend these same accessible generated editors; do not create a parallel editor or enable unsupported choices.

### Validation experience

Group issues as **Must fix before optimizing**, **Likely problem**, **Review suggested**, and **Information**. Every issue uses plain language, names affected entities, links to exact editor/field, offers only deterministic safe bulk fixes, and distinguishes data validation from solver-proven infeasibility.

Example semantics: “Tuesday PM clinic requires two pediatric-qualified clinicians, but only one eligible person is available,” with **Review people** and **Review coverage** actions. Validation state binds to a scenario revision; stale results are discarded/refetched after commands.

### Optimize boundary

Phase 06 may expose the shared `Optimize` entry/status shell once full validation passes: revision/status, Quick/Balanced/Deep, optional time, repair indicator, backend only in Advanced. It never says “Run solver” in normal flow, implies Deep guarantees optimality, or labels a backend incumbent as a valid plan. Phase 07 completes results/repair.

Edits during a solve cancel it or leave it bound to its recorded revision and clearly mark the eventual result stale. Operations completing below the perceptual threshold do not flash a spinner. At approximately 300–500 ms, a stable live region shows only real coarse phases—validation, candidate preparation, transportation when applicable, model build, optimization, verification, result preparation—plus elapsed time when useful and cancellation. It never invents percentages, cycles fake steps, floods assistive technology with callbacks, or announces a valid plan before independent verification.

### Advanced mode

Advanced mode is a deliberately separate diagnostics and expert-control surface; it is never mixed into first launch, guided setup, or the normal Optimize task flow. It may expose:

- backend selection;
- time, thread, and seed settings;
- model summary;
- planning-IR and backend diagnostics;
- objective-level details;
- bounded, redacted solver logs;
- assumption-core diagnostics, with sufficient-versus-minimal and capability limitations stated accurately;
- export of previewable, sanitized model artifacts.

Advanced controls show clear backend, compatibility, reproducibility, privacy, and performance warnings where applicable, apply only allowlisted values through typed APIs, and provide one action to reset every advanced control to safe defaults.

## Accessibility and keyboard contract

- Every function is keyboard reachable with a visible focus indicator.
- Dialog/popover/route transitions restore focus predictably; validation links focus the exact field.
- Controls have semantic names/descriptions; icon-only controls get accessible names.
- Validation and solve completion use screen-reader announcements without noisy event spam.
- Reduced motion is respected.
- Drag-and-drop always has keyboard actions.
- Calendar/canvas interactions always have list/table alternatives.
- Schedule/matrix grids expose logical row/column headers and detail inspector.
- Important content is never hover-only or color-only.
- Automated checks are supplemented by manual keyboard and screen-reader scripts.

Default keyboard model:

| Action                           | Shortcut                                  |
| -------------------------------- | ----------------------------------------- |
| Undo/redo                        | Platform `Ctrl/Cmd+Z`, `Ctrl/Cmd+Shift+Z` |
| Save/export bundle               | `Ctrl/Cmd+S`                              |
| Global command/search            | `Ctrl/Cmd+K`                              |
| Optimize                         | configurable and collision-checked        |
| Find active-view entity          | `Ctrl/Cmd+F`                              |
| Toggle assistant                 | configurable                              |
| Open explanation/selected detail | `Enter` or explicit action                |
| Lock/unlock selection            | command palette and context menu          |

Never bind destructive actions to one unmodified letter. Shortcut help is visible; future customization is allowed without changing command semantics.

## Performance contract

- Virtualize large rows and columns based on measured need; use TanStack Table 9 and Virtual 3 current APIs.
- Debounce view-only filtering, not committed commands indefinitely.
- Move parsing, compilation, geometry, solver, report, import, and export work to Rust/application jobs; never block the webview thread.
- Bound and coalesce solve events; keep stable component keys and avoid rebuilding full trees on selection.
- Update selection immediately, then fetch heavier detail.
- Record action-to-result render spans separately from backend time and measure webview long tasks, input responsiveness, scroll, screen-reader announcement volume, cancellation latency, and result-view render.
- Profile the versioned small/typical/stress workforce fixtures, including representative 100-person schedules; retain the 500-guest canvas benchmark for Phase 09.
- Future canvas layers remain background/tables/seats/overlays/selection and cache geometry by layout hash.

## Error, empty, loading, stale, and offline states

Typed errors map to user input/validation, revision conflict, unsupported feature/backend, import/migration, solver status, internal defect, credential/provider/network, permission/path, and update/release. Internal error UI includes redacted diagnostic ID, safe summary, retry/recovery actions, and copy-sanitized-report; no raw Rust debug/provider response by default.

Every applicable major screen designs:

- no entities or results yet;
- solution stale after scenario changes;
- active/cancelled solve;
- backend unavailable;
- import inspection, migration/review warning, reconnection, collision review, stale preview, cancelled import, and rejected rows;
- backup in progress, explicit exclusions, permission/space failure, cancellation, and verified completion;
- restore add/replace choice, safety-backup success/failure, confirmation, interrupted/recoverable failure, and fresh-install success;
- AI unavailable while core remains fully usable;
- old solution from an earlier revision;
- internal verification failure;
- loading with an action/status description; and
- local/offline operation with no account/network dependency.

No indefinite spinner lacks text, cancellation, or timeout behavior. Revision conflicts offer refresh/reapply review rather than overwriting authoritative state.

## Localization readiness

English is sufficient for MVP, but all user strings use message keys; explanations/validation retain typed parameters; date/number/unit formatting is locale-aware; scenario timezone is separate from locale; translatable strings are not concatenated from fragments; technical solver logs remain English diagnostics.

## Ordered work packages

For the remainder of this phase, follow the user-authorized [temporary integration workflow](../contributors/git-workflow.md#temporary-phase06-integration): coherent package PRs target protected `phase/06-desktop`, consume exact verified integrated phase commits, and receive full integrated-phase checkpoints after packages 3, 6, 9, and 10 below. This changes integration cadence, not package order, issue IDs, acceptance criteria, manual checkpoint, or final-main authority.

1. **UI-001 — versioned foundation:** lock the compatible stack, migrate breaking-major APIs cleanly, configure Tailwind 4/Vite, copy/review minimal shadcn-vue/Reka components, and define semantic tokens/themes/motion/typography.
2. **Generated API boundary:** generate DTOs/commands/events, implement the only Tauri client layer, strict import restrictions, typed errors/revision handling, event cleanup, and minimal capabilities.
3. **UI-002 — shell/navigation and portability:** first launch, project home, proposed `.eutheto` inspect/import/export, Backup & Restore route, intent-led preview/collision/add/replace/safety-backup/recovery flows, workspace/view stores, command palette/shortcuts, focus restoration, empty/error boundaries.
4. **Common planner fields:** pickers, duration/date-time fields, rule strength/scope, import mapping, portable/share inclusion and privacy summaries, validation summary, empty/error recovery, and undo history with accessible contracts.
5. **UI-003 — people/import:** people editor, qualification/type/team/contract fields, and the UI/client for the completed Phase 05 people CSV service: granted-file detection, mapping/identity review, additions/updates/duplicates/rejections preview, atomic apply, one-step undo, and rejected-row download.
6. **Work/shift editor:** horizon/timezone/DST policy, presets/types/templates/instances, coverage/qualification slots, regeneration diff, detached-instance review.
7. **Eligibility/availability:** virtualized matrix, bulk preview, calendar and accessible list editor, blocking-rule inspector.
8. **UI-004 — rule/validation:** intent catalog, sentence builder, live scope preview, Required/Preference guidance, fast/full validation navigation and safe fixes.
9. **Solve/status handoff:** shared mode/progress/status/error shell driven by versioned events, 300–500 ms perceptual threshold, coarse truthful phase mapping, bounded announcements, cancellation, stale revision behavior, then handoff to Phase 07.
10. **Accessibility/performance hardening:** keyboard/screen-reader/reduced-motion/contrast scripts, small/typical/100-person profiling, webview long-task/input/render evidence, designed state matrix, localization-key review.

**Remaining implementation-branch grouping:** Deliver packages **5–6** together, **7–8** together, then **9** and **10** separately under the user-approved [temporary Phase06 integration workflow](../contributors/git-workflow.md#temporary-phase06-integration). Preserve the ordered implementation dependencies and every package's acceptance criteria; a shared branch does not authorize skipping a prerequisite. Full integrated-phase checkpoints remain after packages **6, 9 and 10** (the package-3 checkpoint is already complete), followed by the existing manual-testing gate. This consolidation grants no additional remote publication or final-main authority.

## Tests and acceptance

### API/state tests

- generated Rust→TS DTO/command/event output is checked in and drift-detected;
- only API modules import Tauri invoke/event functions;
- mutation request carries expected revision/request ID; conflict never overwrites current state;
- Pinia contains only declared transient/view state; route/store refresh follows invalidated view keys;
- event listeners filter stale job/scenario/revision and clean up on unmount/navigation;
- strict `unknown` boundary validation and stable safe error mapping.

### Component and accessibility tests

- Vitest/Vue Test Utils/Testing Library exercise user behavior with accessible queries, not class selectors;
- Reka dialogs/popovers/menus have names, keyboard operation, focus trap/restore, escape/cancel, and reduced-motion behavior;
- token contrast meets WCAG AA; status/required/preference remains understandable without color;
- matrix/calendar/rule/import flows work without pointer/drag and expose semantic headers/list alternatives;
- screen-reader announcements cover validation change and completion without solver-callback flooding;
- axe checks supplement manual keyboard and screen-reader scripts.

### Setup and import behavior

- first-time script creates a small workforce scenario without Advanced mode/account/AI;
- user sets timezone/DST, adds people/qualifications/work/coverage, configures eligibility/availability, creates the 10-hour Required rest rule, and runs validation;
- every key action is keyboard operable and focus returns to a meaningful control;
- rule empty scope cannot save accidentally; unsupported rule cannot appear enforced;
- CSV mapping through the generated typed client previews additions/updates/duplicates/rejections from the Phase 05 service, never guesses similar-name identity, rejects stale previews, applies atomically, retrieves rejected-row details, and undoes as one batch;
- recurrence regeneration shows diff and protects detached edits;
- opening a proposed `.eutheto` bundle shows exact inspect/migration/reconnection/collision preview before mutation; Create copy/Replace/Skip, cancellation and stale preview map to Phase-01 typed services without frontend authority;
- practitioner-ledger distinctions are not collapsed into misleading target/fairness labels; unanswered or unsupported choices remain accurately unavailable, and later enabled controls use the same typed preview, keyboard, validation and revision contracts;
- scenario export is labelled editable; full backup lists included/excluded data; add versus replace restore is unmistakable; destructive replace requires confirmation and truthfully reports pre-restore safety-backup failure/recovery;
- all validation severities link to exact editors/fields and distinguish data issues from infeasibility.
- users can distinguish draft from applied configuration and explain the proposed scope, units, and Required/Preference meaning before committing; ambiguous identity or time interpretation is resolved explicitly rather than silently guessed.

### State/performance matrix

- normal, empty, loading, stale, error, cancelled, offline, unavailable, migration-review, old-solution, import-rejection, and verification-failure states render meaningful text/recovery;
- no indefinite spinner; cancellation/timeouts are visible;
- representative 100-person matrices remain usable under measured render/input/scroll budgets with current Table 9/Virtual 3 APIs;
- sub-threshold operations avoid progress flicker; longer operations show only real phases, remain cancellable, keep input/focus responsive, and coalesce screen-reader announcements;
- raw backend incumbents never trigger verified-result copy, and optional later explanation work never blocks the already accepted result shell.
- Real Chromium component tests supplement but never replace native Tauri/WebKit coverage. `just e2e` exercises the Linux unbundled debug shell, native settings and portable file workflows, real safety-backup failure/bypass/recovery, deletion/history boundaries, offline inventory, route recovery and restart persistence; see [desktop behavior and verification limits](../../apps/desktop/README.md). This is not installer, native screen-reader, or cross-platform acceptance. Keep the later exact packaged-artifact/platform gates and manual picker/screen-reader evidence; unconsumed native editing shortcuts still need platform binding verification.

### Assisted native walkthrough — 24 September 2026

This is **agent-operated automation**, not the user manual checkpoint or unfamiliar-user acceptance. The application source is based on `cbfb6c746ca053fc2737d8124e762d91fd17f867` plus documentation commit `ff8dc2959e1d0cbb9ccee1ca2ffa5ce6ae4fb9a4`; the passing walkthrough used the corrected native runner whose SHA-256 is `e930d43d8d473bd381df53beaf60b52150d7f266c129895c1e3622434f46533c`. On Fedora Linux x86_64, `just e2e` built and drove the real unbundled debug Tauri/WebKit application under an offline Xvfb display (1600 × 1200) with disposable XDG/SQLite data. Its WebDriver interactions use native GTK file pickers and keyboard input; screenshots are local, ignored `.cache/e2e/` artifacts. `just frontend-browser-test` also passed 56 Chromium browser tests in 12 files, including selected axe checks; those are not native screen-reader tests. The exact [ten-step manual script](../../apps/desktop/README.md#phase-06-manual-native-checkpoint-script) remains the acceptance procedure.

The complete native suite passed again on local commit `5d7e0d4eee31b00e481fad731c7ec8aab814cd87`, which contains that exact runner revision. No application behavior changed between that run and the earlier one; this documentation follow-up records the reproducible tested source.

| Script step                       | Directly observed in the native automated walk                                                                                                                                                                                                                                                                                                                                                                                                                         | Not established by this walk                                                                                                                                                                                                        |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1. First launch                   | Offline empty-library project creation, keyboard expansion of date settings, saved Workforce project and revision, and scenario locale persistence after restart. The main fixture used January 2030 in UTC; a separate New York repeated-time fixture covered 1 November 2026.                                                                                                                                                                                        | The requested four-day 31 October–3 November project with deliberate gap/repeated-time policy selection.                                                                                                                            |
| 2. People and Work                | Created and edited a person, opened optional groups with configured-value summaries, reviewed native changes and retained draft fields. `people-native-simple-person.png` and `people-native-optional-fields.png` show the revised layout.                                                                                                                                                                                                                             | An unprompted person's explanation of each field and saved versus draft state; the complete two-person Work-editor exercise, recurring repeated-hour shift, invalid local-time focus and cross-record draft rebase.                 |
| 3. Eligibility                    | Native 100-person × 64-type matrix, keyboard navigation, paged equivalent, membership preview and one-batch apply. `package7-matrix.png` shows the rendered grid.                                                                                                                                                                                                                                                                                                      | Spoken row/column headers and inspector names, and the exact two-person manual setup.                                                                                                                                               |
| 4. Availability                   | Authored weekly and instant unavailability, reviewed an approved-time-off blocker against a saved shift and inspected the visual/list representation.                                                                                                                                                                                                                                                                                                                  | The entire manual keyboard date-entry script and a participant's interpretation of an unmarked date.                                                                                                                                |
| 5. Rules                          | Reviewed and saved a ten-hour Required rest rule, checked empty active-scope refusal and coverage counts. `package8-native-people-scope.png` shows the scope preview.                                                                                                                                                                                                                                                                                                  | A solver-naive participant's understanding of Required/Preference, units, saved versus proposed state and consistency across all related views.                                                                                     |
| 6. Validation                     | Explicit full validation, finding-to-field navigation, stale status, a typed resource-limit failure, WebDriver keyboard input and focus while the UI reported a pending validation request, separate native cancellation, and a New York repeated-hour finding navigated to the exact weekly start. `package8-input-during-native-validation.png`, `package8-native-validation-cancelled.png` and `package8-temporal-exact-weekly-row.png` show these distinct states. | The complete manual keyboard traversal of every severity, human interpretation of validation versus feasibility, and input responsiveness **after confirmed native admission**. The pending UI flag alone does not prove admission. |
| 7. Optimize                       | Saw informational Quick/Balanced/Deep modes and unavailable start/cancel controls on unvalidated, current-full and stale revisions; `package9-optimize-current.png` shows the current-full state.                                                                                                                                                                                                                                                                      | A participant explaining why validation and mode text do not create a workable schedule.                                                                                                                                            |
| 8. CSV import                     | Drove GTK chooser, column/identity and proposed-person review, duplicate/rejected rows, atomic apply, report handling and undo; `people-csv-native-review.png` shows the review.                                                                                                                                                                                                                                                                                       | Unprompted field mapping and identity reasoning for the exact two-row manual file.                                                                                                                                                  |
| 9. Portable files                 | Drove editable export, inspect/import, full backup and add/replace restore with a real safety-backup failure/recovery in the disposable library; `portable-preview.png` shows included/excluded data.                                                                                                                                                                                                                                                                  | A participant distinguishing file preparation from library mutation and any platform-specific manual chooser behavior.                                                                                                              |
| 10. Accessibility and performance | Observed keyboard focus on selected screens, native matrix/list views, light/dark screens and automated browser axe checks. The full native runner recorded its debug WebKit corpus, input and render samples in ignored `.cache/e2e/package10-performance-measurements.json`.                                                                                                                                                                                         | Full route-by-route keyboard/focus and contrast audit, 200%/400% reflow, reduced motion, installer/platform shortcuts and spoken screen-reader names/announcements.                                                                 |

**Reconciliation of the 24 September run:** Native automation established selected offline setup, validation, CSV, portability, backup/restore, keyboard, and performance paths, not the whole first-time script. At that point the exact four-day, two-person recurring-Work scenario, full keyboard traversal of key setup routes, input responsiveness after confirmed native validation admission, and route-wide zoom/contrast/state checks were unverified. The follow-up below closes only some of those gaps. The beta deferral applies only to unfamiliar-user research and spoken native screen-reader QA; it does not waive typical-interaction gaps. Hosted target checks and protected integration are not evidenced for this local candidate. Phase 06 exit and Phase 07 production remain blocked pending missing evidence, review of resulting defects, and explicit user checkpoint acceptance and authorization.

The first two `just e2e` attempts stopped at the 200-shift validation workload: one captured command-search text and focus only **after** the operation had failed; the other waited for the transient “Validating” label after opening search and never saw it. A third run captured WebDriver keyboard input while the UI reported the validation request as running, but failed because the delayed progress label had not appeared. The test now checks the pending UI state at the input event, not whether a delayed visual phase happened to render. This is evidence of retained input and focus during a pending request, **not** proof that the native validator was already admitted at that instant. With this test-only correction, the fourth `just e2e` run passed the full native suite, including validation cancellation, DST finding navigation, restart persistence and bounded debug performance sampling. The expected resource-limit error was displayed as failure, never readiness. This is not installer, release-build, cross-platform, or human usability evidence.

The fourth debug run's 100-person × 64-type membership preview/apply took 54,774/25,990 ms; its 200-shift fixture admission took 60,357 ms. On the committed-source repeat, these took 31,747/16,420 ms and 37,509 ms respectively. In the repeat's local performance artifact, the large-supported Work initial/full-window action-to-double-animation-frame proxies were 179/109 ms; 100 × 64 eligibility cold input/scroll proxies were 4/16 ms and warm proxies 5/13 ms. These are different operation boundaries on unaccelerated Linux debug WebKit, not INP, typical two-person setup measurements, release-build service-level guarantees, or proof that large-batch edits feel fast.

No spoken screen-reader observation was obtained: the initial Orca invocation could not load the Atspi namespace, and the isolated app reported an inaccessible accessibility bus. Setting `GI_TYPELIB_PATH=/usr/lib64/girepository-1.0` made `orca --help` work, but did not establish spoken app behavior. No solver-naive human participant was available. On 24 September 2026 the maintainer chose to **defer unfamiliar-user research and spoken native screen-reader testing to beta**, not to count unrun tests as passes. Phase 12's [usability research and accessibility QA](12-stabilization-and-public-release-gate.md) remain release gates. The Phase 06 user checkpoint is not yet accepted on the results of this walkthrough; Phase 07 production remains gated.

### Follow-up native typical-interaction checks — 25 September 2026

The local Linux WebKit/Tauri `just e2e` run passed after extending `apps/desktop/e2e/project-home.persistence.mjs` with two disposable four-day New York Workforce projects. Keyboard expansion of creation settings and explicit gap/repeated-hour **reject** policy selection in one project produced the native planning horizon from 31 October through 3 November 2026 (exclusive 4 November); its existing full-validation finding-to-field check also passed. In the second project, the UI selected gap rejection and the **earlier** repeated-hour policy. Through People and Work editors it saved two eligible people, one Work type and a reviewed Sunday recurring shift at 01:30–02:30. The authoritative resolved 1 November occurrence starts at `05:30Z` (UTC−04:00), ends at `07:30Z` (UTC−05:00), and lasts 120 minutes; the native shift table rendered it. Policy selection was verified; actual nonexistent spring-forward time rejection was not exercised.

The passing final runner has SHA-256 `432a367f51632e0252d36dab0991ea3ca32d86f186098e03f29d05e3d40d3aec`. One intermediate rerun stopped at a native “Stay here” route-leave dialog. The final runner accepts no dirty-draft dialog: for a clean operation guard it stays in place, waits for settlement and retries the keyboard route without cancelling or discarding.

The same native run established these bounded interaction results and limitations:

- On editing the saved recurring template, raw invalid start time `25:61` remained in the draft, review did not open, and the scenario revision did not advance. **The exact-field focus criterion failed:** focus moved to `#work-editor-heading`, not the invalid time field, and the displayed native error was a generic invalid command payload at `/command`. The Workforce command deserializer emits a general `/payload` failure; single-command error conversion drops payload-path provenance. This remains a product defect, not a passing manual checkpoint. No product code was changed in this check pass.
- Keyboard Enter navigated directly to People, Work, Rules, Validation and Setup and focused each route heading. Eligibility and Availability through More project tools, complete route-by-route keyboard traversal, and all editor/dialog transitions were not established.
- At a 1360-pixel native window, injected CSS zoom at 200% and 400% left the **Work view** without page-wide horizontal overflow (`scrollWidth <= clientWidth`). Screenshots in ignored `.cache/e2e/package8-work-css-zoom-{2,4}x.png` capture the surface; this does not establish actual native zoom, route-wide reflow or contrast.
- The existing pending-validation keyboard check still does not establish responsiveness **after native admission**. Full route-wide zoom/contrast/states, hosted target checks, feedback resolution and explicit maintainer checkpoint acceptance remain outstanding. The unfamiliar-user and spoken screen-reader deferrals are unchanged. Phase 07 production is not authorized.

### Exact local-time field correction — 25 September 2026

The prior invalid-time focus failure is corrected in the local candidate. On a failed Workforce template decode, Rust checks the authored start/end time with its native time deserializer and preserves that payload leaf through command, store, application and desktop preview errors. Only a directly authored single domain command can promote the leaf to an input field; one-child batches and derived reconciliation keep a general command error rather than misattribute a field. The repeated full Linux WebKit/Tauri `just e2e` run passed with an assertion that raw `25:61` stays in the start-time input, the exact input has `aria-invalid="true"` and active focus, no approvable review opens, and the saved revision is unchanged. The runner source SHA-256 is `07bac191836b447c69728e8e82a4a575235a6c01b724d272dc8c8b9e2d8df6fd`. Focused Rust checks covered both authored time leaves, direct versus one-child batch preview/mutation attribution, and unchanged state. Independent code and security review found no remaining issue after the batch attribution correction. This closes that specific observed defect, **not** the unproven route-wide keyboard/zoom, post-admission responsiveness, hosted targets, or explicit maintainer manual checkpoint; Phase 07 production remains gated.

The local `just check` passed generated drift, fixture/protocol and architecture validation, formatting, workspace Clippy and tests, desktop type/lint/component checks, and doc tests. This does not establish hosted-platform or maintainer checkpoint acceptance.

### Additional native keyboard routes — 25 September 2026

The subsequent full Linux WebKit/Tauri `just e2e` run passed with keyboard Space expanding **More project tools** and Enter navigating to Eligibility and Availability; focus landed on each route heading, in addition to the previously exercised five direct setup routes. The runner SHA-256 is `f455f6afd89edcf7997896251cf40e873e99e8aa87a30289bd7a220157d4666e`. This establishes those seven transitions only: it does not prove complete Tab/Shift+Tab traversal, editor/dialog focus restoration, native 200%/400% zoom and contrast across routes, or responsiveness after **confirmed** native validation admission. Those checks and the explicit maintainer checkpoint remain outstanding.

### Native zoom and focus follow-up — 26 September 2026

The complete `just e2e` run passed on Fedora Linux x86_64 with WebKit user agent `Version/60.5` in the unbundled debug shell and disposable XDG storage. The tested source commit is `b991416e1b09cf2a941927c7ad1791d57816d475`; its native runner SHA-256 is `17b3e67cc3f525971419185687460752e1bad1b3cfa4b24e887c1757131efb5a`, and the locally built unsigned debug executable SHA-256 is `57c78670c451103c2dd3eed1010efbdd30d423002dea557b19388194a52e800b`. These digests identify this local check, not a packaged or supported-platform release candidate.

The previous Work-only CSS zoom check was not native zoom. In the unbundled Linux Tauri/WebKit window, Ctrl+= initially left the device-pixel ratio and viewport unchanged. The main window now enables Tauri zoom hotkeys with `core:webview:allow-set-webview-zoom` confined to the existing local `main` capability; no new application command or remote window grant was added. A disposable native run used those keys to reach actual 200% and 400% zoom at a 1360 × 1000 window, checking increased device-pixel ratio and reduced CSS viewport width. At each scale it navigated by keyboard to loaded Setup, People, Work, Rules, Validation, Eligibility and Availability routes, checked heading focus plus a forward and reverse Tab with visible focus outlines, opened the populated Work shifts table and selected an Availability person. Each route's document had no page-wide horizontal overflow (`scrollWidth <= clientWidth`); ignored `package8-*-native-zoom-{2,4}x.png` screenshots retain the rendered samples. This is selected route traversal and a root-width reflow check, **not** complete keyboard traversal or a whole-page visual/contrast audit.

The 400% command dialog exposed two real focus failures in this isolated WebKit run. Native X11 Shift+Tab arrived as `key="Unidentified", code="Tab"`; Reka's Tab-only focus trap left focus on the first search input instead of wrapping to the last dialog button. After that wrap was repaired, forward Tab focused the search input while it remained scrolled out of view. The native regression failed before the visibility correction and passed after the shared dialog handler restored boundary focus/scroll visibility. The test checks the dialog's visible accessible heading, physical-like Shift+Tab to the last button, Tab back to the visible search field, Escape and restoration of the previous focus. The 400% screenshot `package8-native-modal-zoom-4x.png` shows the focused search control on-screen. No physical keyboard, spoken screen reader or other desktop platform was exercised.

The full native run also exercised a changed-revision deletion review: its confirmation button was disabled, and reverse Tab from the first enabled dialog button wrapped to the last **enabled** button rather than the disabled confirmation. This checks the dialog fallback against an actual disabled control, not just the all-enabled command palette. The 400% dialog screenshot SHA-256 is `1725fe317d7afca646504f98354119f30a1450a7fe6babe3b58872a032c9708f`.

The native settings run saved dark appearance and forced reduced motion, observed the root preferences and computed button transition durations no greater than 1 ms, then reset and restored the same appearance through reviewed settings import. A separate calculation from the semantic CSS colors found light/dark ink-on-canvas ratios 13.50/15.44, muted-on-canvas 6.45/10.31, accent-on-canvas 6.84/9.96, and danger-on-danger-soft 6.52/4.75; these selected token pairs exceed the 4.5:1 normal-text threshold but do **not** prove every rendered text/background pair, focus treatment or route meets WCAG. The isolated native accessibility bus remained unavailable, and the prior deferral of unfamiliar-user and spoken screen-reader checks to beta remains unchanged.

The 200-shift full-validation keystroke assertion still observes a **pending UI request**, not confirmed native admission. An attempted long eligibility-review progress probe did not establish a continuous admitted operation at the keystroke and was removed rather than reported as a pass. Full route-by-route editor/dialog keyboard traversal, rendered light/dark contrast/state matrices, platform-specific editing shortcuts and explicit maintainer review/acceptance are still unverified; do not infer Phase 06 exit or Phase 07 entry.

### Complete command-dialog Tab path at native zoom — 26 September 2026

The subsequent full Linux WebKit/Tauri `just e2e` run passed at tested source `5abe6aaffff4ebec07234b86f99b87f0b2ef0754` (runner SHA-256 `8263704ca7cf75ae9cb3c669eec659e5d21728ce13c3c8127643ddeb973c1d73`; unsigned debug executable SHA-256 `57c78670c451103c2dd3eed1010efbdd30d423002dea557b19388194a52e800b`). At 400% native zoom, successive WebDriver Tabs from the command search focused every enabled button in DOM order, with each button vertically visible inside the dialog and viewport; the next Tab wrapped to the visible search field. Physical-like reverse Tab, forward return and Escape focus restoration also passed. This is complete **command-dialog** traversal, not complete traversal of every route and editor. It does not establish rendered light/dark contrast, other-platform shortcuts, or input responsiveness after confirmed native full-validation admission.

### Maintainer-reported native checkpoint — 25 September 2026 local

For local source `e2c15f5738bf0bf7bca3a08b75a2525c8930d68c`, the maintainer reports **Pass** on the applicable portions of all ten [native checkpoint steps](../../apps/desktop/README.md#phase-06-manual-native-checkpoint-script) on this Fedora Linux/WebKitGTK host, with no failures reported. They specifically confirmed typing and retained focus after native full-validation admission, every setup route inspected in light and dark at 200% and 400% native zoom, and safely observed safety-backup failure and recovery. These are user-reported observations on the current local source; the earlier agent-operated pending-request keystroke and limited route/theme coverage in this document remain correctly scoped to their own runs.

The maintainer did not run independent solver-naive participant research or spoken native screen-reader QA. Those cases within the script remain **unverified**, deferred to beta/Phase 12, not Pass. Host package/version, locale, scaling and exact-runtime limitations are recorded alongside the per-step results in the checkpoint script. One Linux host does not establish other native platform results. The maintainer explicitly accepted this checkpoint for `e2c15f5` and authorized Phase 07 work **only after** the remaining Phase 06 hosted, verification and protected-main integration gates pass. This conditional acceptance does not itself establish Phase 06 exit or authorize push, PR, merge, signing or public publication.

### Phase exit gate

**User manual checkpoint:** Before this phase exits or Phase 07 implementation starts, hand off the real desktop setup workflow for user review, resolve feedback, and obtain explicit acceptance and authorization to proceed under the [manual checkpoint policy](README.md#user-manual-checkpoint-gates). Agent-operated typical interaction checks do not themselves grant acceptance. The maintainer's dated Phase 06 exception defers independent unfamiliar-user research and spoken screen-reader testing to beta/Phase 12; it does not call those tests passed, waive keyboard/semantic accessibility requirements, or authorize Phase 07, remote publication or release.

Phase 06 exits only when the small scenario can be created through the first-time script without Advanced mode; all key setup actions work by keyboard; proposed `.eutheto` scenario inspect/import/export and full backup/add-or-replace restore use the completed Phase-01 preview/apply services with accurate inclusion, collision, reconnection, safety-backup and recovery states; the people CSV flow uses the completed Phase 05 Rust backend and applies as one undoable batch with rejected-row retrieval; Required/Preference language is consistent; every frontend mutation uses typed commands/revisions rather than local authoritative state; typical native mouse/keyboard flows, semantic accessibility checks, webview-responsiveness, performance, and error-state gates pass; and the current breaking-major stack is pinned and used without stale conventions. Unfamiliar-user research and spoken native screen-reader QA remain **unverified until beta** under the dated exception above; no Phase 07 domain importer or report renderer is required for this gate.

A desktop flow is done for this phase only when normal, empty, loading, stale, error, and offline states exist; keyboard and focus work; semantic names and status announcements have automated coverage; destructive actions confirm; revision conflicts recover; analytics/performance impact is measured where relevant; and typical-workflow evidence supports the task. Native spoken screen-reader and independent usability acceptance are deferred as stated above, not inferred from automation.

## Risks and failure handling

| Risk or failure                                            | Required behavior                                                                                                             |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| Router 5/Table 9/Pinia 4 examples use old APIs             | Treat compile/type failures and behavior changes as migration defects; use only current APIs and remove compatibility layers. |
| Pinia duplicates domain state                              | Delete the duplication; query Rust view models and reconcile command deltas.                                                  |
| Tauri command accessible from unintended window            | Register strict capability/command manifest and minimum window scope; do not rely on invoke handler alone.                    |
| Tailwind 3/shadcn stale CSS silently misstyles focus/theme | Audit generated source for Tailwind 4 variables/animation package and test both themes/focus/contrast.                        |
| Import identity ambiguity                                  | Require explicit matching; never partially mutate or silently merge names.                                                    |
| Scenario changes while form/validation/solve is open       | Detect revision mismatch, preserve edit buffer, refetch/review; never overwrite.                                              |
| Large eligibility matrix stalls                            | Cursor/window query, stable keys, measured virtualization, bulk command rather than cell-by-cell IPC.                         |
| Calendar/drag excludes keyboard/screen-reader users        | Supply equivalent list/form actions and logical grid semantics.                                                               |
| Error exposes sensitive/internal data                      | Show safe typed summary/diagnostic ID; sanitize copied report and logs.                                                       |
| AI/network unavailable                                     | Core and all setup remain fully usable offline; no blocking prompt/account.                                                   |

## Deferred and non-goals

- Full workforce solving/result grids, explanations, alternatives, repair, locks, and exports are Phase 07.
- Seating canvas/geometry is Phase 09, although its components/tokens/accessibility contracts are reserved.
- AI assistant implementation is Phase 10; Phase 06 does not require AI setup or network.
- Nuxt is not used in the desktop app; a later documentation website may use it.
- No universal form library, chart/dashboard suite, mutable scenario store, raw HTML from scenario/AI, Electron-specific dependency, or hidden mutation watcher unless a demonstrated requirement justifies it.
- Shortcut customization and richer motion are optional later work; accessible defaults are required now.

## Assumption and version gates

- Pin the exact compatible versions in this document and their transitive lockfiles. A proposed update is evaluated as a coherent stack, not package-by-package wishful upgrading.
- TypeScript remains **6.0.3** while `typescript-eslint` 8.68.0 excludes `>=6.1`; moving to 7.0.2 is blocked until current lint tooling supports it.
- Node remains **24.20.0 LTS** and pnpm **11.24.0**; update stale Nix/package metadata that still assumes pnpm 10.
- Verify Tauri 2 Rust/npm/plugin compatibility, strict capabilities, Linux prerequisites, and packaged WebDriver behavior against exact locks. Windows WebView2, minimum OS versions, and Linux/macOS target packaging remain Phase 11 gates.
- Pinia 4 ESM-only plus separate devtools API, TanStack Table 9 `useTable`/feature configuration, Tailwind 4 CSS-first/shadcn animation-variable changes, Router 5, Vite 8, Konva 10, and ECharts 6 are explicit breaking-major adoption work—not documentation-only version bumps.
- Review workforce defaults/fairness language with practitioners and validate the first-time workflow with solver-naive users. Repeated confusion about Required vs Preference, rule scope, import identity, or validation vs infeasibility blocks public MVP.
- The final project name is `eutheto`. Reverse-domain application ID, CLI name, hosting/governance contacts, release identifiers, and signing/updater choices remain explicit gates. `.eutheto` is a proposed extension until the Phase-11 identity ADR closes; UI copy may label the proposal but cannot register a public association early.
