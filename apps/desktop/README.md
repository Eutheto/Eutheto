<!-- SPDX-License-Identifier: Apache-2.0 -->

# @eutheto/desktop — development desktop foundation

This package is the non-public Tauri/Vue development application. Its existing
[Phase 01 core shell and persistence](../../docs/roadmap/01-core-application-shell-and-persistence.md)
consumes the [Phase 06 foundation and shell/navigation package](../../docs/roadmap/06-desktop-design-system-and-workforce-setup.md).
It exercises the real local Rust application service; it is not a mock shell
or a released installer. The complete Workforce setup/results, public application
identity, and native manual accessibility gates remain open.

## Implemented development behavior

At startup, Tauri opens one `EuthetoApp` service backed by
`library.sqlite3` in the platform application-data directory. Rust and SQLite
are authoritative. The Vue application loads projections from that service and
sends typed requests back to it; browser state is not a second persistence
layer.

Hash routing exposes first launch, Workforce creation, the active/archived
project library, selected-project setup, People/supporting records, Work, history and
editable export, portable import, Settings, Backup and restore, offline About/licenses,
and unknown-route recovery.
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
substituting another project. The overview is not an editor or result workspace.
Preview does not commit; apply, history, undo/redo, and local settings retain
their existing application-service authority.

The separate People and supporting records route reads 50 native summaries at a
time and complete typed details on selection. People, qualifications, teams and
assignment types have explicit create/edit buffers and native command review;
deletion requires a successful native reference check. Person fields include
external identity, active dates, temporal qualification grants, eligible assignment
types, teams, existing home-location/workload-target references, rational weight,
tags and display metadata. Assignment types retain their complete policy fields.
The person form starts with the name and four summarized optional groups:
qualifications/eligible work, dates/teams/home location, workload/targets, and
tags/appearance. External import identity and record IDs are separate disclosures.
Opening a group preserves the same complete raw draft; invalid fields and
validation navigation open the relevant group before focusing the input. Review
still uses the complete native proposal, not only the expanded fields.

A revision or library-epoch change preserves raw drafts but invalidates approval.
Whole-field rebase carries independent current changes and requires explicit choices
for conflicts. Partial choices survive later revisions, but stale choices remain
disabled until a fresh explicit rebase. Inactive optional input and numeric spelling survive when the resolved
native field still has the draft's meaning. An unavailable record is not assumed
deleted; copying its draft requires a fresh identity. Root-owned write receipts and
unknown-outcome recovery survive route exit without replaying the command.

Bulk actions capture up to 50 people selected on the current native page. Set their
active dates, add/remove one team, or explicitly confirm deletion; native change
review, selected proposed-person inspection and validation precede one atomic,
undoable batch. A concurrent target-field change requires a whole-field choice,
including when the original action was a no-op. Partial choices survive unrelated
revisions; stale choices cannot be accepted before the complete selection is reread.

People links to a native CSV import route. The picker and raw snapshot remain in
Rust; the screen receives an opaque source ID, native encoding/delimiter evidence,
and bounded inert samples. Delimiter, header handling, physical columns, creation
defaults and exact reference tokens are explicit choices, never a browser CSV parser.
Names do not choose identity. Add allocates a stable draft ID; Update requires a
native exact match or an explicitly selected existing person; Skip is explicit.
Decisions remain repairable after a failed preview, including record-specific native
errors. Interpretation changes discard prior decisions; revision-only re-preview
retains them while invalidating approval.

Native row status, typed proposed people and draft validation precede one atomic
apply. A blocked review cannot be applied; an exact no-change import creates no
history entry. The screen never splits an oversized import into separate commits.
Source limits are 16 MiB, 64 columns and 10,000 data records, with at most 1,000
changed people per apply. Rejection reports contain bounded record numbers/codes,
not raw cells. Save them through the separate native chooser before replacing the
preview/source or leaving the route. A received import receipt remains authoritative
if report saving fails or is cancelled; uncertain writes use root history recovery,
not automatic replay.

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
mapping fields support these and the remaining ordered editor packages; they are
not mounted as a production playground or evidence of completed import/rule editors.
Their [native and accessibility contracts](../../docs/roadmap/06-desktop-design-system-and-workforce-setup.md#common-field-consumer-contracts)
also identify later-only components and the distinct Phase 07 Share Result gate.

Setup operations reserve a window/context-bound identity before work, use
bounded admission and an invocation-owned progress channel, and release
ownership on settlement or disposal. Cancellation acknowledgement is not a
terminal result. Full readiness distinguishes not-run, running, completed,
failed, cancelled, and stale input; an empty fast report is not full readiness.

The native Workforce overview supplies exact half-open planning dates and a
bounded initial work window. The stored-only `local_time_resolution` setup query
prepares one raw local endpoint against the captured scenario settings, including
outside the visible horizon; offset and timezone annotations are not accepted as
local input. Settings preparation and temporal failures return safe exact-field
findings, and overnight generation diagnostics retain both occurrence and endpoint
dates.

Open a Workforce project and choose **Work** to edit planning dates/time policy,
assignment types, workload buckets, locations, calendars, recurring shift templates,
stored shift instances and exact/minimum coverage with optional preferred/maximum
counts and qualification minima. The stored-record browser also reaches templates
without active occurrences and manual/detached instances outside the planning horizon.
Calendar and coverage collections retain the complete raw draft while mounting
only a bounded page of controls; native errors reveal and focus the affected page.

**Review changes** shows native command changes and generation additions, changes
and removals. Source inspection reads the complete saved or proposed record;
occurrence ledgers are not editable fields. The local-date display window and
review filters never narrow the command's effect. Apply is bound to the reviewed
source, revision and prospective hash. Changed local endpoints require fresh native
preparation; a concurrent time-policy change requires an explicit choice before
reinterpreting retained raw input. Native tables show original local intent, UTC
offset, instant, reporting date, and both wall-clock and elapsed durations.

Detachment, manual endpoint edits, regeneration and explicit reattachment use the
same reviewed command/history flow. Regeneration does not overwrite detached edits.
The editable **Clinic + on-call starter** proposes ordinary typed records in one
atomic batch; it is synthetic, adds no people or rules, and is not a clinical,
staffing, fairness or payroll recommendation. These screens do not optimize or
display accepted results.

**Eligibility** shows configured person/type membership, not qualification validity
or assignment feasibility. Native axes contain at most 128 people and 64 types;
search names or filter people by a recorded qualification grant, including future
or expired grants. The virtual matrix keeps logical row/column headers and active
cells, supports arrow/Home/End navigation, Space edits and Enter inspection, and
has an equivalent bounded paged table. Explicit row/column choices are reviewed
against a captured revision and applied as one atomic batch. Fresh review preserves
unselected memberships and other person fields. Capture is capped at 8 MiB of
compact person records and the command at 16 MiB; oversized work is refused, not split.

**Availability** separates authored records from configured occurrences, so records
outside the displayed dates remain reachable. It edits unavailable, available-only
and approved-time-off records with instant or weekly windows, effective dates,
optional assignment-type/location restrictions, and inert source/note text. Native
portable-data restrictions still apply. Rust resolves new local endpoints; editing
other fields preserves stored instants and fractional precision. The calendar has
keyboard creation controls and an exact interval list; its marks represent the
current native occurrence page, not proof of availability or rule enforcement.
The shared person–assignment inspector reads applied native blockers and identifies
Required rules not covered by its analysis. It does not establish whole-plan
feasibility.

**Required rules** exposes the five implemented kinds: eligibility, availability,
coverage, no overlap and minimum rest. Commands retain stable identities across
draft edits, require native before/after review, and show native affected counts.
Independent bounded people/assignment scope previews use the captured revision.
An active rule with an empty evaluated scope cannot be applied; explicitly inactive
rules remain supported. Preferences and later rule kinds are not presented as
implemented policies.

**Validation** separates saved-input fast feedback from an explicit full check of
the saved revision. Completed, stale, failed and cancelled outcomes remain distinct;
neither zero fast errors nor a completed full check proves solver feasibility.
Native severity groups and owner/field provenance drive supported editor navigation,
without mutating the revision. Repairs use the ordinary reviewed editor commands,
not guessed automatic fixes. Unsaved drafts are not included in saved-input checks.

The overview's **Optimize** section reads actual native command capability and
matching saved-revision validation. It gives read-only Quick, Balanced and Deep
descriptions only after a current, error-free full check; this is not a proof of
feasibility. Registered desktop solve start/cancel commands are unavailable, so
there are no executable mode, backend, time, repair or Advanced controls. A
capability-read failure has an explicit retry and does not erase accepted-result
summaries. Phase 07 owns live desktop solves and result/repair actions.

The root displays an operation panel after a 400 ms perceptual delay without
delaying native admission or the immediate exclusion of concurrent commands. Its
phase is the most recent actual native report; ordinary assistive announcements
coalesce to at most one per second, while cancellation and a real library refresh
announce promptly. The panel disappears after the operation and any library
refresh settle; a failed operation never claims it is refreshing the library.
Cancellation request and native terminal result remain separate.

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

## Phase 06 manual native checkpoint script

The maintainer explicitly approved the Phase 06 manual checkpoint items on
2026-09-24 for local candidate `cbfb6c746ca053fc2737d8124e762d91fd17f867`.
The maintainer also confirmed that the manual checkpoint was not run on this
candidate. Approval is a recorded decision, not evidence that the checks below
passed.

An [agent-operated native walkthrough](../../docs/roadmap/06-desktop-design-system-and-workforce-setup.md#assisted-native-walkthrough--24-september-2026)
records the checks actually exercised. The 25 September native runner on source
`d1470e0` covered the corrected invalid-time field focus and seven keyboard
route-to-heading transitions. That `cbfb6c7` approval did not record manual
test results or accept the later candidate; the maintainer's subsequent
checkpoint report and conditional acceptance for `e2c15f5` are below.
Unfamiliar-user research and spoken native screen-reader QA remain deferred to
beta, not passed.

The [26 September native zoom/focus follow-up](../../docs/roadmap/06-desktop-design-system-and-workforce-setup.md#native-zoom-and-focus-follow-up--26-september-2026)
records real 200%/400% Linux WebKit magnification, selected keyboard route and
dialog checks, forced reduced-motion persistence and bounded color-token contrast
calculations. It does not establish the full native validation-admission typing,
rendered contrast/state matrix, other platforms or checkpoint acceptance.

The first manual pass found setup overwhelming; the second found **Individual
Person Edit/Update** overwhelming. Its optional groups and the plain-language
labels across setup require fresh unprompted review at beta before claiming
unfamiliar-user acceptance; automation cannot measure comprehension.

**Maintainer-reported native checkpoint — 25 September 2026 (local).** The
maintainer reports running the applicable portions of all ten steps below on
local source `e2c15f5738bf0bf7bca3a08b75a2525c8930d68c` in the native
Linux Tauri/WebKitGTK application, with no failures reported. These are
maintainer observations, not additional agent-runner assertions:

1. **Pass** — offline project creation, four-day New York horizon and DST/locale settings.
2. **Pass** — People and Work edits, disclosures, draft/review/focus and repeated-hour shift.
3. **Pass** — eligibility matrix, equivalent table, keyboard selection and reviewed apply.
4. **Pass** — availability calendar/list, weekly restriction, approved time off and inspector.
5. **Pass** — scoped ten-hour Required rest rule and rejected empty scope.
6. **Pass** — full validation, finding navigation, cancellation, stale result and command-search typing with focus retained **after native validation was admitted**.
7. **Pass** — truthful Optimize status before validation, after validation and after an edit.
8. **Pass** — native CSV review, identity decisions, rejected rows, apply and one-step undo.
9. **Pass** — project export/import review, collision and stale cases, full-backup restore, and **safely observed safety-backup failure and recovery** in a disposable profile.
10. **Pass** — applicable route/editor keyboard and focus checks, Linux shortcuts, states, reduced motion, and light/dark inspection on every setup route at both 200% and 400% **native** zoom.

The maintainer used this host. Metadata collected **after** the report: Fedora
Linux 44 Workstation x86_64, shell locale `en_US.UTF-8`, GNOME automatic
display-scaling setting (`0`) and text scale `1.0`. The Nix development
`webkit2gtk-4.1` pkg-config version is `2.52.4`; the installed Fedora
`webkit2gtk4.1` package is `2.54.0`. Neither package query identifies the
exact library loaded during the maintainer's test. The tested display-scale
multiplier and executable digest were not supplied; do not substitute the
earlier automated runner's executable digest. The maintainer explicitly did
**not** run an independent solver-naive participant or a spoken native screen
reader. Their comprehension and spoken names/announcements within steps 2,
3, 5, 6, 8–10 remain **unverified** under the beta deferral, not Pass. This
report covers one native Linux host, not other platforms. The maintainer
explicitly accepted this Phase 06 checkpoint for `e2c15f5` and authorized
Phase 07 **only after** the remaining Phase 06 hosted, verification and
protected-main integration gates pass. This neither asserts Phase 06 exit now
nor authorizes a push, PR, merge, signing or public publication.

Run the actual `just desktop-dev` Tauri application on a supported platform
with a disposable local data profile and test files. The Vite-only page has
no native service. Record the platform, OS, webview, screen reader/version,
theme, locale, display scale, application revision, date, and each observed
failure. Do not use a personal project library for restore or replacement
exercises. Independent solver-naive participant research is deferred to beta;
record any comprehension failures directly observed in the current checkpoint
without leading someone through individual controls.

1. Start with an empty library and no account/network connection. From
   **Work schedule**, create a small project covering 31 October–3 November
   2026 in `America/New_York`. Expand the date/time settings with the
   keyboard and deliberately choose the gap and repeated-time policies.
   Check the project opens at its saved revision; changing display locale
   must not change its scenario time zone.
2. Follow the project setup's **People** and **Work** steps. Add two people, a
   qualification and one work type. For a new person, start with the name;
   check that the optional group summaries tell what is already configured.
   Use **Edit person** near the saved person's heading, then open each
   disclosure by keyboard to set qualifications and valid times, work types,
   active dates, team/home location, relative workload weight and target,
   tags/appearance, and an ID from another system. Ask the participant to
   explain, before prompting, what each choice will change and whether a
   reviewed record is already saved. Check that they understand a workload
   weight is a relative ratio, not a promised shift count, and that the
   "stops before" date is excluded. Close and reopen a group; values must
   remain. Deliberately leave a required field empty in a closed group and
   review: the invalid group must open and focus its field. Review the
   complete proposed record before saving the update. In Work, switch
   between saved records and the displayed shift dates; add a recurring
   shift with coverage of one and inspect its entered local time, resolved
   offset, instant and elapsed duration around the repeated hour. Check an
   invalid local time preserves raw input and exact focus. Change an
   independent record while a person draft is open; refresh/review must not
   erase unrelated or inactive input.
3. Open **Eligibility** from the People step or **More project tools**.
   Configure the people/type memberships by keyboard: arrows, Home/End,
   Space and Enter in the matrix, then the equivalent paged table. Check
   row/column headers and the person inspector are announced with their
   names. Review the before/desired values and apply once; a pending choice
   is not a shift-feasibility finding.
4. Open **Availability** from the People step or **More project tools**.
   Author a weekly unavailable window and an approved-time-off record for
   one person. Use keyboard date creation and the exact interval list as
   well as the visual calendar. Check the source/note appear as inert text,
   an unmarked date is not called available, and the inspector identifies
   the blocker on the actual shift.
5. Create an active ten-hour **Required** minimum-rest rule. Try an empty
   evaluated scope and observe refusal; then select a meaningful scope,
   review before/after and save. Ask the participant what Required versus
   Preference, who and what a rule covers, units, proposed versus saved
   changes, and validation versus a workable schedule mean. Check whether
   **Who can work each type**, **Availability**, CSV import and the rule
   coverage counts give the same understanding of work types and shift
   restrictions; record any mismatch, including unsupported choices.
6. Run explicit **Validation** on the saved revision. With the keyboard,
   traverse all severities and follow a finding to its actual owner field;
   check focus and raw input, return to the finding, and repair through a
   reviewed editor command. Interrupt a later validation and check the
   cancellation request is distinct from the terminal outcome; edit after
   a completed run and check it becomes stale. During admitted work, type
   into the command search and check typed input/focus remain responsive.
7. Expand **Optimization and accepted results** on the setup page to inspect
   **Optimize** before full validation, after an error-free current full run,
   and after a revision change. Its Quick/Balanced/Deep descriptions are
   information, not runnable choices or optimality promises: native
   start/cancel remain unavailable. Do not infer feasibility from zero fast
   errors or from a completed full check.
8. Prepare a disposable UTF-8 CSV with `Name,External ID` and rows such as
   `Alex,review-1` and `Blair,review-2`. Select it with the system file
   chooser. Ask the participant to match columns to fields and explain which
   people will be added, updated or skipped before reviewing the import;
   distinguish a CSV record number from a file line and an ID from another
   system from an Eutheto record ID. Inspect additions, duplicates and a
   deliberately malformed row, then apply the reviewed batch and undo it
   once. Saving a rejected-row report is a separate choice. Similar names
   must never silently choose an existing person's identity.
9. Use **More project tools** to review an editable `.eutheto` project
   export. Check the file without importing it, then import it in a separate
   review. Ask the participant which steps change the saved library and which
   only inspect or prepare a file. Exercise same-ID collision choices, stale
   review and reconnection warnings without treating a preview as a commit.
   In the disposable profile, review full-backup included/excluded sections,
   add-versus-replace restore, explicit confirmation, safety-backup
   failure/recovery and verified result. If a failure cannot be reproduced
   safely on the platform, mark that manual case unverified; do not damage
   a real library to manufacture evidence.
10. On each route, use Tab/Shift+Tab, Enter/Space, Escape and available
    Ctrl/Cmd shortcuts without a pointer. Check visible focus, named
    dialogs/menus, focus trapping and restoration, skip navigation,
    informative empty/loading/stale/error/cancelled states, and status
    announcements without repeated callback spam. Inspect light/dark
    contrast, reduced motion and reflow at 200% and 400% native zoom; on
    Linux use Ctrl+= to magnify and Ctrl+- to return to the original scale.
    Note any horizontal scrolling outside a bounded data grid. With a native
    screen reader (for example Orca, NVDA or VoiceOver on its supported
    platform), record actual field names, descriptions, logical headers,
    exact validation navigation and cancellation/completion announcements.

Automation covers selected native paths, but it does not supply human
understanding, platform editing accelerator, installer or spoken
assistive-technology evidence in this script. The maintainer deferred
unfamiliar-user research and spoken native screen-reader QA to beta; the
remaining typical interaction gaps in the [agent-operated record](../../docs/roadmap/06-desktop-design-system-and-workforce-setup.md#assisted-native-walkthrough--24-september-2026)
remain unverified. Record observed outcomes and failures, correct failures,
and repeat affected steps before treating Phase 06 exit or Phase 07 entry as
evidenced. Checkpoint approval alone does not authorize remote publication.

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
scoped scenario undo/redo, offline inventory, route recovery and restart persistence.
The People scenario exercises all four record kinds, distinct temporal grants,
existing location/target references, native target rejection and repair, active dates,
rational weight and display fields. It covers real concurrent writers,
stale-approval invalidation, partial whole-field choices across another revision,
retained inactive input, referenced-deletion refusal, fresh-identity recovery and keyboard focus
without stealing focus from an existing draft input. Screenshots are written only
under ignored `.cache/e2e`.
The bulk scenario covers native add/remove-team and date changes, invalid-date
refusal without mutation, initially no-op target conflicts, mixed field choices
across another revision, preservation of unrelated fields and references, selected
native proposal inspection, confirmed batch deletion and one-step undo/redo.
The CSV scenario adds native picker cancellation and unsupported encoding,
explicit column/identity review, inert hostile header text, a real Add-ID collision,
record-specific error focus, duplicate/rejected rows, selected sample/proposed-person
inspection, stale re-preview with stable Add IDs, atomic apply, independent report
save/cancellation, exact no-change apply, and one-step undo/redo after restart.
The eligibility/availability scenario exercises a real 100-person × 64-type matrix,
keyboard navigation and the equivalent table, concurrent unrelated edits, one-batch
apply and exact membership restoration through undo/redo and process restart.
It authors weekly and instant availability, type/location restrictions and literal
markup, checks exact nanosecond intervals, and inspects an approved-time-off blocker
on an actual shift. Matrix DOM-retention and WebDriver round-trip measurements are
written to `.cache/e2e/package7-matrix-measurements.json`; these are not heap-memory,
INP, release-build or cross-platform benchmarks.

The rule/validation scenario reviews and persists a ten-hour Required rest rule,
checks stable draft identity, live scope pages, empty-active-scope refusal and
explicit inactivity, and distinguishes completed, stale, failed and cancelled
validation attempts. It follows an authoritative standalone-coverage shortage to
the exact count field and reviews a manual repair. An America/New_York repeated-time
fixture demonstrates that zero fast errors need not imply full readiness and
navigates the full finding to the second authored weekly start without mutation.
The native runner also checks WebDriver keyboard input and focus while the UI
reports a pending full-validation request. That UI flag does not prove native
admission or entry into the computation loop; input during admitted native
validation remains unverified by this run.

The native Optimize handoff reads real `app_get_capabilities` and checks
unvalidated, current-full and stale saved revisions. Start/cancel remain unavailable;
the three mode descriptions are informational, no desktop solve action is exposed,
and viewing them does not change the scenario. The runner also records actual
operation-active/display spans under ignored
`.cache/e2e/package9-progress-measurements.json`: short operations must not flash
progress, and longer ones show only delayed native phases. These debug WebKit
measurements do not establish release-build responsiveness or screen-reader
announcement behavior.

Phase 06 performance evidence is generated by the same native runner at
`.cache/e2e/package10-performance-measurements.json`. It checks SHA-256 digests
from the versioned Workforce corpus, applies the tiny, initial and
large-supported fixtures through typed native commands in disposable projects,
and compares settings, entity counts, authored Required rules and resolved
shifts; new project identity/history means the scenarios are not byte-identical.
In one Linux debug Tauri/WebKit 60.5 run (24 September 2026; GPU acceleration
unavailable), Work's initial-window action-to-double-animation-frame spans
were 139/122/160 ms for tiny/initial/large-supported; full-horizon spans were
139/89/113 ms for 5/20/12 rows. Native `buildingView` to
`preparingResponse` timestamp differences were 45/28/38 ms for overview and
69/59/89 ms for the full Work query. Those backend phase spans exclude
admission, response encoding, IPC and webview rendering. Native batch-apply
request-to-settlement spans were 171/209/1554 ms; they include IPC and are
**not** backend-only durations.

The separate 100-person × 64-type eligibility stress case measured cold
keyboard/input/scroll double-animation-frame proxies of 12/5/20 ms and warm
proxies of 10/6/15 ms. Its native cancellation request was acknowledged in
4 ms; validation IPC rejected 126 ms after its initial request. The profiler
does not classify the rejection cause. WebKit's long-task observer reported
zero entries in the bounded sample; two live-region DOM mutations are an
announcement proxy, not screen-reader evidence. The source artifact records
fixture digests, platform, method and native backend phase boundaries. These
one-run debug figures are below the provisional 300–500 ms progress-display
threshold for sampled in-page interactions, not calibrated INP, paint,
accessibility, release-build or cross-platform performance budgets.

Chromium checks supplement this native surface with focus, keyboard and axe
coverage. Neither suite proves native screen-reader behavior, installers, or
other platforms. In the isolated GTK/WebKit run, an unprevented text-field
Ctrl+Z did not perform native editing undo; the runner verifies the shell leaves
editing shortcuts unconsumed and cannot route them to scenario history. Native
editing accelerator and assistive-technology behavior still require the later
manual/platform checkpoint; no shell shortcut override is installed.
