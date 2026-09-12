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

| Role | Version | Compatibility and major-version implication |
|---|---:|---|
| Node.js | 24.20.0 LTS | Production LTS; satisfies Vite 8, pnpm 11, ESLint 10, Vitest 4. Do not move to Node 26 Current before its LTS/support gate. |
| pnpm | 11.24.0 | Current stable; replaces the blueprint's stale pnpm 10 assumption. Requires Node `>=22.13`. Update package-manager/CI/Nix pins as one cutover. |
| TypeScript | 6.0.3 | Newest compatible stable, not latest 7.0.2: `typescript-eslint` 8.68.0 declares `<6.1.0`. A TS 7 or 6.1+ upgrade waits for lint-parser support. |
| Vue | 3.5.42 | Composition API baseline. |
| vue-router | 5.3.0 | Breaking major from Router 4-era examples; requires Vue `^3.5.34`, Vite `^7.3 || ^8`, Pinia `^3.0.4 || ^4.0.2`. Use/test Router 5 hash-history, guards, typed route behavior; do not copy Router 4 APIs unreviewed. |
| Pinia | 4.0.3 | Breaking major, ESM-only, and requires `@vue/devtools-api` separately. It remains transient/view state only. |
| `@pinia/colada` | 1.4.2 | Query/server-state coordination only; Rust remains authoritative and mutations still cross the typed command/revision boundary. |
| `@vue/compiler-sfc` | 3.5.42 | Must exactly match the Vue 3.5.42 baseline for SFC compilation. |
| `@vue/devtools-api` | 8.2.1 | Required separate Pinia peer; pin explicitly rather than relying on an undeclared transitive copy. |
| Vite | 8.2.2 | Breaking major; engine `^20.19 || >=22.12`, satisfied by Node 24.20.0. |
| `@vitejs/plugin-vue` | 6.0.8 | Compatible with Vue 3 and Vite 8. |
| `@tauri-apps/api` | 2.11.1 | Keep coherent with the selected Tauri 2 Rust crates and generated API tests. |
| `@tauri-apps/cli` | 2.11.4 | Workspace development/build CLI; patch does not need to match JS API numerically, but the lock/manifests must be conformance-tested together. |
| `@tauri-apps/plugin-updater` | 2.10.1 | Release use is later; retain typed boundary and compatible Rust plugin/config. |
| `@tauri-apps/plugin-shell` | 2.3.5 | Minimize use; strict scope permits only exact bundled sidecar where needed. |
| Tailwind CSS / `@tailwindcss/vite` | 4.3.3 / 4.3.3 | Tailwind 4 is CSS-first and uses the Vite plugin. Audit v3 syntax; shadcn-vue uses `tw-animate-css`, and CSS variable utilities use `var(...)` semantics rather than stale v3 snippets. |
| shadcn-vue | 2.8.2 | Component generator/source, not runtime design authority. It migrated from Radix Vue to Reka; generated code is owned/reviewed by `eutheto`. |
| Reka UI | 2.10.4 | Current direct-registry version; use accessible headless behavior, not stale Radix Vue APIs. |
| `@lucide/vue` | 1.39.0 | Preserve the installed Phase-04 foundation pin; icons require visible/accessible labels where meaning is not decorative. |
| TanStack Vue Table | 9.2.4 | Breaking v9 API: use `useTable`, not v8 `useVueTable`, and configure features explicitly. |
| TanStack Vue Virtual | 3.13.36 | Stable v3 `useVirtualizer` line for measured large views. |
| Konva / vue-konva | 10.3.2 / 3.4.0 | Seating Phase 09; vue-konva supports Vue 3 and Konva `>7`. Treat Konva 10 as a breaking-major baseline. |
| ECharts / vue-echarts | 6.1.0 / 8.1.0 | Use only selected analytical views; vue-echarts 8 expects ECharts 6 and Vue 3. Do not copy v5 wrapper setup. |
| ESLint | 10.9.1 | Node 24 compatible; configure current flat/type-aware Vue rules. |
| `typescript-eslint` | 8.68.0 | Governs the TypeScript 6.0.3 ceiling (`>=4.8.4 <6.1.0`). |
| Vitest | 4.1.11 | Compatible with Vite 8 and Node 24. |
| Vue Test Utils | 2.5.0 | Vue 3 component tests. |
| Testing Library Vue | 8.1.0 | User-centric accessible queries. |
| axe-core | 4.13.0 | Automated accessibility aid; manual keyboard/screen-reader scripts remain required. |
| `@vitest/browser-playwright` / `playwright` | 4.1.11 / 1.62.1 | Real Chromium component interaction and axe checks; not native IPC, platform or installer evidence. |
| Native W3C WebDriver runner | Repository-owned | Existing dependency-free Node 24 runner; `just e2e` exercises Linux native unbundled Tauri/WebKit persistence across application-process restarts. |

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
scenario_get_history
scenario_migrate_preview

solve_get_backend_options
solve_estimate_model
solve_start
solve_cancel
solve_get_job
solve_list_runs
solve_get_diagnostics_summary
```

The `/settings` route uses the five `settings_*` endpoints above, `/about/licenses` uses `app_get_license_inventory`, and UI that shows local data/cache/log/export locations uses the bounded, nonsecret `app_get_paths_summary` result rather than reconstructing platform paths in Vue. Support-bundle and updater APIs are intentionally completed in [Phase 11](11-public-mvp-packaging-and-documentation.md), not improvised in this phase.

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

| Action | Shortcut |
|---|---|
| Undo/redo | Platform `Ctrl/Cmd+Z`, `Ctrl/Cmd+Shift+Z` |
| Save/export bundle | `Ctrl/Cmd+S` |
| Global command/search | `Ctrl/Cmd+K` |
| Optimize | configurable and collision-checked |
| Find active-view entity | `Ctrl/Cmd+F` |
| Toggle assistant | configurable |
| Open explanation/selected detail | `Enter` or explicit action |
| Lock/unlock selection | command palette and context menu |

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
- real Chromium component tests supplement but never replace native Tauri/WebKit coverage; `just e2e` currently exercises a Linux unbundled debug executable, not an installer. Extend the existing native W3C runner where supported and retain the later exact packaged-artifact/platform gates plus manual picker and screen-reader evidence.

### Phase exit gate

**User manual checkpoint:** Before this phase exits or Phase 07 implementation starts, hand off the real desktop setup workflow for user testing, resolve feedback, and obtain explicit acceptance and authorization to proceed under the [manual checkpoint policy](README.md#user-manual-checkpoint-gates). This is additional to the unfamiliar-user and accessibility evidence below; CI or a merged PR cannot satisfy it.

Phase 06 exits only when an unfamiliar user can create the small scenario through the first-time script without Advanced mode; all key setup actions work by keyboard; proposed `.eutheto` scenario inspect/import/export and full backup/add-or-replace restore use the completed Phase-01 preview/apply services with accurate inclusion, collision, reconnection, safety-backup and recovery states; the people CSV flow uses the completed Phase 05 Rust backend and applies as one undoable batch with rejected-row retrieval; Required/Preference language is consistent; every frontend mutation uses typed commands/revisions rather than local authoritative state; progress, accessibility, webview-responsiveness, performance, and error-state gates pass; and the current breaking-major stack is pinned and used without stale conventions. No Phase 07 domain importer or report renderer is required for this gate.

A desktop flow is done only when normal, empty, loading, stale, error, and offline states exist; keyboard and focus work; screen-reader names/announcements work; destructive actions confirm; revision conflicts recover; analytics/performance impact is measured where relevant; and usability evidence supports the task.

## Risks and failure handling

| Risk or failure | Required behavior |
|---|---|
| Router 5/Table 9/Pinia 4 examples use old APIs | Treat compile/type failures and behavior changes as migration defects; use only current APIs and remove compatibility layers. |
| Pinia duplicates domain state | Delete the duplication; query Rust view models and reconcile command deltas. |
| Tauri command accessible from unintended window | Register strict capability/command manifest and minimum window scope; do not rely on invoke handler alone. |
| Tailwind 3/shadcn stale CSS silently misstyles focus/theme | Audit generated source for Tailwind 4 variables/animation package and test both themes/focus/contrast. |
| Import identity ambiguity | Require explicit matching; never partially mutate or silently merge names. |
| Scenario changes while form/validation/solve is open | Detect revision mismatch, preserve edit buffer, refetch/review; never overwrite. |
| Large eligibility matrix stalls | Cursor/window query, stable keys, measured virtualization, bulk command rather than cell-by-cell IPC. |
| Calendar/drag excludes keyboard/screen-reader users | Supply equivalent list/form actions and logical grid semantics. |
| Error exposes sensitive/internal data | Show safe typed summary/diagnostic ID; sanitize copied report and logs. |
| AI/network unavailable | Core and all setup remain fully usable offline; no blocking prompt/account. |

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
