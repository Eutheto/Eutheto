<!-- SPDX-License-Identifier: Apache-2.0 -->

# @eutheto/desktop — development desktop foundation

This package is the non-public Tauri/Vue development application. Its existing
[Phase 01 core shell and persistence](../../docs/roadmap/01-core-application-shell-and-persistence.md)
consumes the [Phase 06 foundation and shell/navigation package](../../docs/roadmap/06-desktop-design-system-and-workforce-setup.md).
It exercises the real local Rust application service; it is not a mock shell,
a released installer, or evidence that Workforce editors/result screens or the
public application identity are complete.

## Implemented development behavior

At startup, Tauri opens one `EuthetoApp` service backed by
`library.sqlite3` in the platform application-data directory. Rust and SQLite
are authoritative. The Vue application loads projections from that service and
sends typed requests back to it; browser state is not a second persistence
layer.

Hash routing exposes first launch, Workforce creation, the active/archived
project library, selected-project setup, history and editable export, portable import,
Settings, Backup and restore, offline About/licenses, and unknown-route recovery.
Pinia retains transient selection and deletion-review context; Colada caches
native list projections. Neither owns scenario data or replays native writes.

- First launch offers Work schedule and Open existing without an account or
  network. Event seating is explicitly unavailable.
- Creation submits title, locale, time zone, first/last included dates, units,
  and daylight-saving gap/overlap policies to the registered Workforce pack.
- The searchable library supports explicit open, duplicate, archive/unarchive,
  and revision-checked permanent deletion. Deletion offers archive and editable
  export first; returning from export never authorizes deletion and a changed
  revision requires another review.
- Portable routes expose actual inspection, explicit collision choices,
  editable-scenario export, full-library backup, additive/replacement restore,
  verified safety-backup outcomes and recovery, and byte-preserving unopened
  bundle re-export. Native previews supply inclusion/exclusion, migration,
  removal, and reconnection evidence; the client does not invent it.

The root owns operation lifetime, cancellation, authoritative refresh and
terminal outcomes across navigation. Route-local review generations prevent late
responses from resurrecting discarded views. Dirty routes offer Stay/Discard;
cancel-and-leave still waits for native settlement at the root. A committed
mutation or publication wins over late cancellation.

The command palette and shortcut help expose implemented actions only.
Ctrl/Cmd+K opens the palette, Ctrl/Cmd+F focuses the active search, Ctrl/Cmd+S
opens editable export in a valid project context, and Ctrl/Cmd+Z/Shift+Z invokes
scenario history only outside text editing. Input/contenteditable events remain
unconsumed. Route headings, dialogs, and aborted navigation restore meaningful
focus; destructive single-letter shortcuts are absent.

Project creation, listing, and opening use closed V1 native requests. Rust
resolves the included calendar dates to exact local-midnight boundaries and
rejects skipped midnights rather than shifting the requested horizon. The CLI
retains its explicit RFC3339 creation arguments and six-field project-list JSON.
The list/open projection includes `lastOpenedAt`; listing is read-only, while
`project_open` records a successful opening atomically with loading the project.

The native setup boundary exposes V2 summary/readiness, bounded Workforce
views, entity detail/search, rule catalogs, command previews, and explicit full
validation. Rust owns each projection and its immutable input revision.
The setup overview combines matching-revision summary, pack overview and
accepted-only solution projections. It shows saved calendar/count facts, actual
fast/full readiness, and current/stale/selected accepted-result state. Counts do
not imply readiness; an empty accepted list does not imply that solving never
ran. Missing, deleted, unsupported and stale contexts recover explicitly without
substituting another project. This is not a Workforce editor or result workspace.
Preview does not commit; apply, history, undo/redo, and local settings retain
their existing application-service authority.

The workspace History route reads revision-bound metadata pages through
`getScenarioHistoryPage`, not command/inverse/actor payloads. It shows 50 entries
at a time (native maximum 100); request and response ceilings are 4 KiB and 4 MiB.
Summaries exceeding 4096 UTF-8 bytes are explicitly omitted, distinct from an
empty recorded summary. Older/newest paging does not replay individual entries
or expose discarded branches. Undo/Redo uses the existing root operation owner
and native availability; archived, unsupported and busy contexts cannot mutate.
Pending reads are invalidated on mutation, revision/scenario/library-epoch change
or navigation. A conflict refreshes the library rather than applying stale history.

The setup overview now reuses `ValidationSummary` for bounded fast findings and
native total/displayed/omitted counts; full-validation status remains separate.
Empty library/history states use the same named `EmptyState`. Shared controlled
pickers, exact duration/local-time drafts, rule strength/scope and physical CSV
mapping fields are available for the ordered editor packages; they are not mounted
as a production playground and do not implement people/import or rule editors.
Their [native and accessibility contracts](../../docs/roadmap/06-desktop-design-system-and-workforce-setup.md#common-field-consumer-contracts)
also identify later-only components and the distinct Phase 07 Share Result gate.

Setup operations reserve a window/context-bound identity before work, use
bounded admission and an invocation-owned progress channel, and release
ownership on settlement or disposal. Cancellation acknowledgement is not a
terminal result. Full readiness distinguishes not-run, running, completed,
failed, cancelled, and stale input; an empty fast report is not full readiness.

`/settings` exposes separate appearance, locale, and units drafts backed by
native get/update/reset commands. Each write requires the captured library
revision and returns its exact committed snapshot. Dirty drafts survive external
updates as explicit conflicts; reload/discard is intentional, and a section's own
commit preserves unrelated drafts. Persisted theme/motion preferences affect the
shell. Conflicts are not retried, and an absent-key reset is silent and unchanged.
Local validation remains distinct from stricter portable-export policy.
`SettingsImportFlow` and `LibraryOperationScope` provide native picker preview,
one-use approval, atomic apply, and explicit review disposal.
The standalone V1 `eutheto/application-settings` document contains only
`appearance`, `locale`, and `units` as complete `{value, updatedAt}` entries.
Missing keys mean reviewed removal within that scope; an empty map clears it.
Device/credential settings and unrelated library data are never imported.
An unportable local value makes the entire export fail rather than omit a key.
The screen supports reviewed settings import/removal, export, cancellation,
and cleanup through those same native contracts.

Preview/apply/export use library-scoped progress and cancellation. Preview
cleanup covers lost responses and late native settlement; three native slots
include creating, ready, active, and closing reviews. A committed apply wins
over late cancellation. `exportNonsecretSettings` captures after native
destination selection and uses private atomic no-clobber publication: even
picker replacement confirmation does not authorize overwriting an existing file.
Source/compact settings payloads are limited to64KiB and wire envelopes to136KiB.

`getLicenseInventory` reads embedded schema-V2 locked-workspace metadata offline,
bounded to2MiB compact/2,368KiB wire and4,096packages. `NOASSERTION` remains
unknown; this is not exact linked-installer attribution or completed Phase-11
license notices. `getAppPathsSummary` exposes only the three configured-location
booleans, never platform paths.
`/about/licenses` renders that bounded inventory with search and pagination,
plus configured/unconfigured category labels. These labels do not claim
filesystem existence, accessibility, or writability.

Accepted-solution listing, detail, views, selection, verification, comparison,
explanations, and counterfactual requests are implemented at the existing V1
native boundary. Their retained accepted document/solution pairing remains
authoritative; nested accepted-result and verification formats retain their
own V2 versions. These APIs do not imply completed result screens or ordinary
live-solving controls.

Live solve, deferred solution-control/export, and AI command names remain
registered but unavailable where `app_get_capabilities` reports them so;
those calls return typed `unsupported` errors. The desktop does not advertise
live solving or AI availability.

## Rust-authoritative flow

```text
App.vue / route components
    │
    ▼
project-home.ts / application-settings.ts / portable-workspace.ts
    │ typed generated functions
    ▼
src/api/generated.ts
    │ Tauri invoke
    ▼
src-tauri/src/lib.rs
    │
    ▼
EuthetoApp ── SQLite library
```

The application-data database is the durable project authority. Portable source
and destination paths are selected by native dialogs and never enter Vue state.
Automatic pre-restore safety backups use the private application backup area.

`PortableReviewFlow` owns versioned import, restore, backup, scenario export, and
unopened-bundle operations. Library applies bind the reviewed library revision;
scenario exports retain both scenario and library revisions. Native custody
binds the invoking window, creator, review kind, and exact reviewed revisions.
Core-backed and prepared-output reviews each have three native slots; prepared
archive bytes remain charged during active publication. Compact portable
metadata is limited to64MiB, with a128MiB+128KiB client wire allowance.

Cancellation acknowledgement does not mean work has stopped. Owners await real
settlement; a committed mutation or successful publication remains successful
after late cancellation. A blocking native chooser may still need to be closed.
Cleanup also covers lost responses and window teardown without requiring a
current library revision.

A restore reports its actual safety-backup result: not required,
created and verified with the real artifact basename, or an explicitly confirmed
bypass after a real bound failure. Only an actually retained native failure review
can offer the stronger confirmation. Advisory refreshes preserve the displayed
committed outcome. The Backup and restore screen's safety-recovery entry uses
the API's `safetyBackups` picker origin, private backup area, and normal
review/apply pipeline.
A published safety backup may remain if a later restore step is cancelled or
conflicts; cancellation does not imply that no file was created.

## Generated API-only Tauri imports

[`ADR-012`](../../docs/adr/012-tauri-api-and-generated-dtos.md) defines the IPC
boundary. In production frontend source,
`src/api/generated.ts` is the only file that imports
`@tauri-apps/api/core`. Vue components and controllers consume its typed
functions instead of importing `invoke` directly.

`src/api/generated.ts` is generated from the Rust-owned command and DTO
contract and is checked in. Do not edit it by hand:

1. Change the authoritative Rust DTO/command and the corresponding template
   or catalog in `xtask/src/generate.rs`; change pack-owned query contracts at
   their schema authority.
2. Run `just generate`.
3. Review the generated diff.
4. Run `just generate-check` to reject drift.

The generated client parses IPC responses, errors, and events from `unknown`,
checks the declared schema, identities, revision correlation, discriminants,
and collection bounds, and preserves lossless integer representations.
Malformed responses do not trigger automatic command replay. Native command
dispatch and the build manifest consume the same generated command catalog;
the permission and local-window configuration are checked against that boundary.

See
[generated code discipline](../../docs/contributors/generated-code-and-contracts.md)
and [generated artifacts](../../docs/architecture/generated-artifacts.md).

Tauri's `src-tauri/gen/schemas/` and `src-tauri/permissions/autogenerated/` are
separate, ignored native build outputs. Change the checked-in native inputs and
run the native build; do not commit or hand-edit those generated files.

## Capability and release boundary

The local `main` window receives the `allow-phase-01-api` permission. The
permission admits the registered protocol catalog, while
`app_get_capabilities` remains the authority for which registered commands are
implemented in this development phase. No shell or broad filesystem permission
is granted to the webview.

The configured content security policy permits local application resources and
Tauri IPC; it does not permit remote scripts or remote navigation. Tauri
bundling is disabled, and the repository desktop build uses `--no-bundle`.
There is no supported packaged-desktop E2E command.

## Development identity and portable-extension notices

These values are explicit development values, not public compatibility or
release commitments:

| Value                                | Current use                                     | Status                                                                                                  |
| ------------------------------------ | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| `dev.eutheto.phase01.desktop`        | Tauri application identifier                    | Development-only; not the approved public reverse-domain identifier                                     |
| `eutheto Phase 01 development shell` | Tauri product name and main-window title        | Development-only; not a release brand                                                                   |
| `0.1.0`                              | Workspace and Tauri version                     | Development version; not a released desktop version                                                     |
| `.eutheto`                           | Portable scenario and backup artifact extension | Proposed and reported by the API as `provisional-development-only`; not a final public file association |

The stable/beta application identifiers, final portable extension, signing
identities, updater trust, and packaged-release evidence remain open gates.
See [identity gates](../../docs/architecture/identity-gates.md).

## Development commands

Run these commands from the repository root. The `just` recipes are the
canonical repository entry points.

| Task                                                               | `just` command                     | Direct pnpm command                                                            |
| ------------------------------------------------------------------ | ---------------------------------- | ------------------------------------------------------------------------------ |
| Install locked dependencies                                        | `just install`                     | `pnpm install --frozen-lockfile --ignore-scripts`                              |
| Run the native Tauri development application                       | `just desktop-dev`                 | `pnpm --filter @eutheto/desktop run tauri dev`                                 |
| Run only the Vue/Vite development server                           | `just ui-dev`                      | `pnpm --filter @eutheto/desktop run dev`                                       |
| Type-check the frontend                                            | `just typecheck`                   | `pnpm --filter @eutheto/desktop run typecheck`                                 |
| Run desktop ESLint                                                 | `just lint` (repository-wide)      | `pnpm --filter @eutheto/desktop run lint`                                      |
| Check desktop Prettier output                                      | `just fmt-check` (repository-wide) | `pnpm --filter @eutheto/desktop run format:check`                              |
| Run the UI unit/component tests                                    | `just test-ui`                     | `pnpm --filter @eutheto/desktop run test`                                      |
| Install the pinned browser explicitly                              | `just frontend-browser-install`    | `pnpm --filter @eutheto/desktop exec playwright install --only-shell chromium` |
| Run real browser dialog/keyboard/accessibility checks              | `just frontend-browser-test`       | `pnpm --filter @eutheto/desktop run test:browser`                              |
| Exercise Linux native shell/portable flows and restart persistence | `just e2e`                         | Use the recipe's isolated data/runtime/network environment                     |
| Build the Vue frontend                                             | `just ui-build`                    | `pnpm --filter @eutheto/desktop run build`                                     |
| Build the native desktop executable without bundles                | `just desktop-build`               | `pnpm --filter @eutheto/desktop run tauri build --no-bundle`                   |

API generation is Rust-owned: use `just generate` and
`just generate-check`, which invoke the corresponding `cargo xtask` commands.

The native development application requires the pinned Rust toolchain and
platform prerequisites described in
[development setup](../../docs/contributors/development.md). The Vite-only
server does not provide the native Tauri/Rust service.

The native runner uses the real unbundled debug Tauri/WebKit application,
isolated SQLite/XDG data and network namespace, GTK file pickers, and hardware
keyboard input via the existing Nix `xdotool` tool. It exercises settings
conflicts/import/export, portable inspection/re-export/import, backup/add/replace,
real safety-backup I/O failure and explicit bypass/recovery, deletion review,
scoped scenario undo/redo, offline inventory, route recovery and restart
persistence. Screenshots are written only under ignored `.cache/e2e`.

Chromium checks supplement this native surface with focus, keyboard and axe
coverage. Neither suite proves native screen-reader behavior, installers, or
other platforms. In the isolated GTK/WebKit run, an unprevented text-field
Ctrl+Z did not perform native editing undo; the runner verifies the shell leaves
editing shortcuts unconsumed and cannot route them to scenario history. Native
editing accelerator and assistive-technology behavior still require the later
manual/platform checkpoint; no shell shortcut override is installed.
