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

The root owns ordinary workspace operation lifetime, cancellation,
authoritative refresh and terminal outcomes across navigation. The Optimize
observer owns only its solve presentation; releasing or unmounting it detaches
observation and never silently cancels the durable solve. Route-local review
generations prevent late responses from resurrecting discarded views. Dirty
routes offer Stay/Discard; cancel-and-leave still waits for native settlement
at the root. A committed mutation or publication wins over late cancellation.

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
substituting another project. The overview is not a general-purpose editor;
accepted-result inspection is read-only unless the user separately reviews an
explicit base capture. Preview does not commit; apply, history, undo/redo, and
local settings retain their existing application-service authority.

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
staffing, fairness or payroll recommendation. The starter editor itself does
not optimize or display accepted results; the setup overview owns the native
Optimize action and accepted-result list.

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

**Required rules** exposes fourteen implemented kinds: eligibility,
availability, coverage, no overlap, minimum rest, Maximum Assignment Count,
Maximum Hours, Maximum Consecutive, Maximum Worked Weekends, Required Workload
Quota, Required Qualification Mix, Fixed Assignment, Mutual Assignment
Restriction and Transition Time. The count cap applies to matching
person–shift assignments by the shift's reporting date in a selected calendar
period; zero forbids any matching assignment. Maximum Consecutive applies only
to matching assignments in each person's native scope: Worked Days counts
distinct reporting dates and joins only successive civil dates, while
Assignments orders matching shifts by start instant and joins adjacent matches
when their elapsed break is at most the configured minutes; nonmatching selected
shifts do not break a matching run, and overlaps join.
Maximum Hours caps a named workload bucket for each matching person in a fixed
calendar period or every rolling window; Reporting Date belongs to calendar
periods only, while Start Instant and Intersection select by native instants.
Elapsed intersections are clipped to the window; `sum` adds overlapping work
and `union` counts elapsed overlaps once.
A Scheduled Minutes bucket counts signed whole-shift local duration and cannot
use Intersection.
Required Workload Quota applies each person's authored assignment-count or
named-bucket minute range in each owned calendar period. Maximum Worked
Weekends caps distinct local Saturday-to-Monday windows touched by selected
source shifts, including overnight tails, without inferring an adjacent
rotation or guaranteeing any free weekend.
A clinic shift requiring one Specialist and one Nurse can count the same
dual-qualified person toward both minimums, but each minimum counts distinct
assigned people on that shift.
Qualification mix uses whole-shift qualification grants and does not prorate
partial shifts. Fixed Assignment chooses a native person and shift, allows
browsing a bounded date window, and requires the exact named pair to belong
to the active rule's reviewed scope. Mutual Assignment Restriction chooses
native people and either limits them to one on each matching shift or
forbids distinct selected people on matching half-open overlapping shifts
(including the same shift). New active rules need two selected people inside
their reviewed native scope; previously saved singleton or empty active
rules remain readable no-ops.
Transition Time chooses named native locations and applies only the directed
source-to-target minutes authored on the source location. Both distinct
shifts must match the rule scope and belong to the same person; an absent
or reverse-only matrix entry adds no travel gap. Native prospective review
proves that two selected locations occur in effective matching shifts
before new activation or changing an active selection/scope. Existing
active singleton rules remain readable no-ops and can be deactivated.
Commands retain stable identities across draft edits, require native
before/after review, and show native affected counts. Independent bounded
people/assignment scope previews use the captured revision. An active rule
with an empty evaluated scope cannot be applied; explicitly inactive rules
remain supported. The Preference editor offers all twelve authored kinds:
Time, Assignment Type, Location, Requested Time Off, Assignment Target,
Worked-weekend target, Workload Balance, Consecutive Work, Adjacency, Base
Stability, Together or Separate, and Preferred Coverage. The Score policy
screen creates the native singleton when absent and edits its saved levels,
priority mapping and explicit workload policies through reviewed,
revision-bound commands; an
unconfigured policy never implies fairness. Together or Separate evaluates
only pairs who are both assigned and never creates Required work. Preferences
guide the accepted objective; they never turn a valid plan into a
Required-rule violation. Soft locks require Repair plan or a named Alternatives
profile; ordinary Authored Optimize does not support them.

**Validation** separates saved-input fast feedback from an explicit full check of
the saved revision. Completed, stale, failed and cancelled outcomes remain distinct;
neither zero fast errors nor a completed full check proves solver feasibility.
Native severity groups and owner/field provenance drive supported editor navigation,
without mutating the revision. Repairs use the ordinary reviewed editor commands,
not guessed automatic fixes. Unsaved drafts are not included in saved-input checks.

The overview's **Optimize** section reads actual native command and backend
capability and matching saved-revision validation. After a current, error-free
full check it offers executable Quick, Balanced and Deep modes for the exact
saved revision. Command availability and compatible backend names/versions sit
behind closed technical disclosures, not in the default controls. Live phases
appear only after a 350 ms display delay;
quick completions show their durable terminal status without a transient running
announcement. The active native window owns explicit cancellation: a view restored
after navigation or a same-scenario edit can cancel the live solve through its
original request/revision-bound handle even if current validation is stale; a
restarted app has no live handle and never fabricates one. Detaching
observation never cancels work. Durable `get`/`list` reads restore running and
terminal states after navigation, same-scenario edits, or restart; an active
solve continues polling across validation refreshes even if native events are
lost, and its newly started run can be discovered without an event. Once the
first candidate passes independent verification, a separate committed checkpoint
exposes its read-only schedule
without waiting for backend proof. It remains inspectable if the run is later
cancelled or fails, but is not a terminal accepted solution or export authority.
The bundled OR-Tools worker exposes the first intermediate candidate and its
final candidate, not every improvement made during proof search.
The accepted list can select a terminal result for viewing through the native
`solution_select` contract. The returned result identity and revision are checked
before the view marks it selected; an interrupted selection requires a fresh
native list before retry. Selection changes neither the scenario revision nor
the base schedule, locks, or accepted-result verification.
The results panel compares the newest two retained accepted results in native
list order, independent of the selected-for-viewing row. Its owned
`SolutionExplain` operation binds the immutable newest-result revision and both
exact result/checksum/run identities. Visible-head revision, library epoch,
pair generation and actual native window are separate presentation fences; a
refresh that replaces the pair discards a pending comparison. Cancellation and
resource release are distinct. Rust re-verifies both original sources before
returning the read-only difference explanation.
The comparison shows bounded, keyboard-accessible pages of assignments,
Required-rule statuses and typed facts/evidence, metrics, original side labels,
affected identities, independent protection/selection states and descriptive
recorded runs. Score ranking and category deltas require recognized compatible
original objective meaning: units, population/scope, weights, scaling, priority
order and score shape. Each result's own policy is checked against its original
source; different revisions or lowered policy hashes alone are not a rejection.
Matching unknown metadata or absent-versus-Authored objective context does not
establish utility. Other accepted pairs retain semantic differences and an
explicit score-unavailable reason. Selecting or comparing does not adopt or
restore a base schedule, modify locks, or mutate the scenario revision.

**Explain saved-change facts** uses the same owned operation and immutable
candidate anchor for a read-only Repair explanation. The accepted comparison is
A→C; original-domain facts use recorded B→C when the candidate has a baseline,
otherwise A→C. B remains explicitly `notEstablished`, not a freshly admitted
result. The dialog pages native messages, distinct expected/observed parameters
and assignment references without decoding assignment IDs in JavaScript.
Complete original-domain groups are evaluated under one inherited budget;
unresolvable historical endpoints make aggregate scope explicitly unavailable,
not a filtered proof. Closing the dialog cancels pending work, releases its
operation and restores focus; changed pair, head, library or window ownership
cannot publish a late response. Reading facts does not apply a repair.

An explicit **Review as repair base** action is separate from viewing and
comparison. The review binds a particular retained result ID/checksum, original
revision and current scenario revision; the native application re-verifies the
accepted source, saves selected person–shift pairs as a BaseSchedule in one
undoable revision, and requires explicit redo-branch consent when applicable.
An interrupted write is reconciled from history, not replayed automatically.
Older accepted results may be used only while their selected people and shifts
still exist in the current scenario. The Adjacency and Base Stability
preference editors become available once the base is saved and select its
stable ID through native review; neither capturing it nor authoring a
preference proves the revised scenario has a feasible solve. Full validation
must be rerun for the new revision before Optimize becomes available. A result's
**Changes from base** page compares its assignments to the base in that
result's original scenario,
not to subsequent edits. It distinguishes no recorded base from an unchanged
base and pages added/removed person–shift pairs; a base record's source ID alone
does not verify that source.

**Manual changes and repair** reads the complete native selection B while keeping
its original accepted plan A immutable. Add/remove, same-shift replacement and
Hard/Soft/Unlocked protection changes retain raw local intent until explicit
**Review manual change**. Rust resolves recurring work, checks every Required
obligation against the prospective complete selection, and binds approval to
the exact revision/source and proposed meaning. Display pages never restrict
command scope. Historical missing endpoints remain visible and can be removed;
new assignments and protection require current endpoints.

A valid **Apply reviewed manual change** commits the selection and protection
together as one normal History Undo/Redo step. A Hard incumbent cannot be
removed implicitly: replacement requires an explicit unlock in the same review.
Edits, revision/library changes and return from another editor require fresh
review. Cancellation before commit does not save; a confirmed native commit
remains committed despite later cleanup or presentation failure. Unknown
outcomes are reconciled through History rather than replayed automatically.

Invalid review keeps the draft and shows native expected/observed findings and
exact supported editor owners. Alternatives include a native time-preference
preview/apply and a replacement diagnostic against the explicitly chosen
current accepted result C, not historical A or unaccepted B. Diagnostics preserve
the source rules and leave other assignments free; only their actual evidence
can enable a diagnostic copy. If C is stale, solve the current revision first.

After a committed edit, **Repair plan** is primary and uses complete recorded B:
pair-change cost precedes Soft-lock penalties, authored preferences and rank.
Hard locks remain Required. Full validation is required at the new revision;
Authored Optimize is disabled with Soft locks. The chosen C's recorded repair
diff retains unchanged participants and exact added/removed pairs.
**Explain recorded repair facts for chosen result** binds its immutable A/C
sources and separately renders recorded B→C facts; B remains `notEstablished`.
Neither solve nor inspection silently applies a candidate.

Choose **Alternatives** as the next solve intent after selecting an adopted
accepted base or an existing unverified external starting plan and completing
full validation. Its four sequential profiles share one immutable source and
total time limit: Balanced, Preference-focused, Minimal-changes, and
Fairness-first. Each accepted result is independently
verified. Native equivalence compares exact configured non-rank and per-person
outcomes; duplicate suggestions remain recorded in History.

Select two distinct suggestions for the existing accepted-result comparison.
Cross-profile score utility may be unavailable even when assignment and rule
differences can be shown. **View** only inspects the selected accepted result;
neither viewing nor comparison changes the source or adopts a new base. A
settled group is rediscovered on return without rerunning its profiles.

**Starting source for Repair and Alternatives** explicitly separates accepted
result/manual-edit custody from an unverified external plan. The external editor
loads the complete saved pair set, pages its display, and reviews complete
rename/pair changes or removal through one ordinary undoable command. Current-pair
edits preserve the original descriptive import fingerprints. An explicit empty
plan remains a valid source; deleting it never falls back to an accepted source.
Raw drafts survive revision refreshes and require explicit rebase/fresh review.
A lost write receipt blocks replay until native History reconciliation.

The active Alternatives group retains its captured source even if the next
source selector changes. Recorded external baseline differences remain available
after source edits/removal and do not claim accepted A or manual-edit receipt
authority. Assignment CSV import creates or completely replaces an explicitly
unverified external starting plan through native review and atomic apply; it does
not fabricate accepted ancestry or Required locks.
`EUTHETO_E2E_EXTERNAL_PLAN_ONLY=1 just e2e true` exercises the packaged native
editor, complete off-page updates, stale rebase, Undo/Redo, independently verified
Repair, four-profile source capture, explicit empty source, lost-receipt recovery
and reversible removal.

The independently refreshed accepted-result list establishes terminal accepted
results. Inspecting a terminal result or a checkpoint-only schedule separately
requests a fresh native verification report bound to its original scenario,
revision, result, and checksum. The result header shows passed Required-rule
count, a count of recorded preference-priority scores with zero penalty,
category penalties, and verifier warnings from that report; zero penalty is not
a claim that every authored preference applied. The saved run conclusion is
labeled recorded, not a new optimality proof.

The Optimize panel's **Explain terminal status** action reads the saved run
input and exact terminal-manifest checksum from native diagnostics. Accepted
results bind their exact recorded reference and are independently reverified
against the original scenario; no-result runs bind no accepted candidate.
Run ID, original revision and document hash, manifest checksum and evidence
certainty remain visible. An older source is labeled historical, and a
backend-only infeasible or unbounded status does not establish an independent
infeasibility proof or mapped Required-rule conflict. Pending reads are
discarded if the displayed run, scenario or revision changes; explaining a
status never starts a solve or mutates the scenario.

**Check this full validation finding** is available only for an issue selected
from the completed full report on the current saved revision. Native full
validation supplies an exact issue identity; the explanation rechecks that
finding against the saved source and renders the matching native message.
For a current Required coverage `candidate_shortage`, a second native message
may independently count every original person after unconditional activity,
approved leave and active Eligibility/Availability checks. If even this
overestimate is below the authored Required coverage minimum, the panel shows
the count and the real Coverage and excluding rule IDs; source activity and
leave facts have no invented rule identity. This is a sufficient source-level
contradiction, not a minimal core or a solver diagnosis. Fast findings are not
full-report evidence. Source changes or replacement of the displayed report
discard the selection; this evidence does not fix the input or certify an
accepted schedule.

The accepted-assignment inspector's **Check an assignment change** controls
query the saved native person/shift decision in every mode; Vue never constructs
an assignment identity. A 30-second diagnostic can replace a person (require the
target and exclude the incumbent on the same shift), require another person
without excluding the incumbent, or forbid the incumbent from working that shift.
All other assignments remain free, and the original Required rules and objectives
are preserved. Native static exclusions save no question, and an already-selected
replacement or newly required person is rejected without dispatching a solve.

Questions are bound to the base result, immutable revision and condition checksum.
Native get/list APIs reload their durable results; history pages and changed
assignment/category displays are bounded. Starting a question does not revise
the scenario. Independently verified alternatives appear as retained accepted
results but are not automatically selected or applied; latest-two comparison
therefore follows retained-result order, not the viewed result. The panel shows
the observed exact score differences without claiming every possible alternative
is worse. History belongs to the immutable saved result and labels each recorded
condition independently of the current draft. Backend-only infeasibility remains
inconclusive rather than being presented as original-rule impossibility. Workforce
conditions are Boolean only; integer, interval and absent values and general
mapped infeasibility remain unsupported.

By-person, by-shift/location, chronological timeline, selected-assignment,
fairness, and Plan Health pages are native-bounded semantic tables in the
original time zone, including exact DST offsets. Seven-day person and
shift/location calendars retain original civil dates and selected-assignment
samples in a virtualized grid with an equivalent keyboard-friendly paged view.
Location and person drilldowns query the original accepted source, not just
the visible page. Shift rows distinguish authored staffing bounds and
preferences from independently verified Required rules; being below an
authored minimum does not itself imply a Required-rule failure. Coverage margin
is an exact signed difference: direct pack JSON carries a bounded integer,
while native IPC can quote its `i64` as canonical decimal text. The generated
desktop guard accepts both representations without numeric coercion. Plan
Health shows pack-owned source coverage plus applicable Required rest, hours,
and directed travel observations with exact signed margins, typed units,
affected-assignment counts, and explicit no-applicable work. These are
original-result facts, not a new compliance score or validation of later edits.

Selecting a terminal assignment opens independently loaded, pair-local
recorded evidence with native typed facts: rederived original-source checks
for activity, approved leave, and active Eligibility/Availability. For a
selected locally eligible pair, it can list up to 128 matching active Time,
Assignment Type, Location, Requested Time Off, Adjacency, and Base Stability
terms with authored rule IDs and their original policy objective levels.
Selected versus unselected pair-local penalties are exact terms, not a
feasible alternative or whole-plan score difference. Optional terms may be
omitted on finite resource limits; this is not a complete preference inventory.
Requested time off is not approved leave. They do not establish why the
solver selected the assignment, fairness or aggregate score changes, or
validity after later edits. A surviving checkpoint does not invent recorded
evidence. The remaining explanation categories are still Phase-07 work.

A terminal run's advanced diagnostics read its scenario-owned persisted
manifest timings and safe verification code, not raw worker logs; the advanced
disclosure lists recorded phase durations and omits phases not measured.
Backend phase time excludes measured in-flight independent review rather than
truncating it to fit total time. Projection, structure, scoring, and Required
rule durations include every independently reviewed candidate, not only the
terminal choice. A checkpoint retains parent-clock verification entry and
completion; its entry can be later than first-incumbent admission.
Unsupported builds keep the CLI fallback and no desktop solve action. A
capability or backend read failure has an explicit retry and does not erase
accepted-result summaries.

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

Accepted-solution listing, detail, bounded views, selection, verification and
counterfactual requests retain their V1 native boundaries. Current
`solution_compare` uses request/response V3 with `SolutionComparisonV2`; no older
comparison aliases remain. Native comparison responses reject sizes above
512 KiB rather than truncating evidence. Owned explanation requests retain
their operation-request V2 and subject-request V1, while the current explanation
DTO and general evidence/render payloads are V2. Explanation responses are
bounded at 2 MiB. Current recorded terminal statuses are descriptive, not fresh
backend optimality proof; accepted outcomes require independent verification.
Exact historical absent-context statuses, stored counterfactual comparisons V1
and diagnostic reports V2 retain their established formats. A retained
diagnostic's original evidence checksum is distinct from its derived general
explanation envelope checksum. Retained accepted document/solution pairing
remains authoritative; nested accepted-result and verification formats retain
their own V2 versions. The setup overview
inspects accepted Workforce schedules in 50-row person, shift, or assignment
pages; each page includes unassigned people and uncovered shifts where
applicable, and displays local instants in the accepted revision's time zone.
Stale results stay visibly stale. Comparison does not adopt or restore a base
schedule or perform repair. Reviewed base adoption and guided manual Repair are
implemented separately; full Phase-07 acceptance and complete reporting remain pending.
The native Optimize action is available only when the current project passes
full validation and a supported backend is installed; AI commands remain
unavailable.

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
7. Inspect the visible **Optimize** section and expand
   **Optimization and accepted results** on the setup page before full validation,
   after an error-free current full run, and after a revision change.
   The valid state keeps backend names behind an advanced disclosure and offers
   keyboard-accessible Quick/Balanced/Deep mode controls. Start a solve, observe
   real queued/compiling/solving/verifying phases for longer runs (short runs
   show terminal status without a flashing progress announcement), request explicit
   cancellation, and wait for its terminal status. Edit the same scenario while
   search runs: new validation becomes stale, but cancellation still targets
   the original solve revision and the durable terminal status must refresh
   without an event. Open the terminal run's diagnostics and distinguish its
   backend-candidate time from verified-feasible time. Leave and return to
   confirm durable get/list state; only the independently refreshed
   accepted-result list establishes acceptance. Do not infer feasibility
   or optimality from zero fast errors, a full check, a progress phase or a terminal
   status alone.
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

## Phase 07 non-PDF walkthrough preparation

This prepares the non-PDF portion of the next manual checkpoint; it does **not**
open that gate or record maintainer acceptance. CEF/direct PDF is paused by the
maintainer and must be completed, with dependent final verification, before the
Phase 07 manual walkthrough. Spoken native AT and human comprehension are not
established by the automated keyboard journey.

Use an isolated fictional project, not a real roster. The currently exercised
unsigned Linux DEB is
`.cache/cargo-target/release/bundle/deb/eutheto Phase 01 development shell_0.1.0_amd64.deb`,
SHA-256 `dfbc0f469b7d4b32738b4a0d266f605d7ecb73ccdad3080191562791d13f18f9`.
It contains the manifest-bound worker; ordinary `just desktop-dev` does not
automatically provide the bundled-worker build. Final checkpoint artifact
identities must be refreshed after PDF integration.

Once the checkpoint is opened, launch that existing local package from the
repository root in the development environment and a graphical desktop session
without installing it system-wide. This follows `just e2e`'s extraction path;
keep the desktop session's runtime directory so Wayland/X11 remains available:

```sh
manual_root="$(mktemp -d "${TMPDIR:-/tmp}/eutheto-phase07-manual.XXXXXX")"
7z x -bd -y "-o$manual_root/archive" \
  ".cache/cargo-target/release/bundle/deb/eutheto Phase 01 development shell_0.1.0_amd64.deb"
mkdir -p "$manual_root/application" "$manual_root/cache" "$manual_root/config" \
  "$manual_root/data" "$manual_root/state"
tar -xf "$manual_root/archive/data.tar" -C "$manual_root/application"
printf 'Keep this isolated project directory: %s\n' "$manual_root"
env XDG_CACHE_HOME="$manual_root/cache" XDG_CONFIG_HOME="$manual_root/config" \
  XDG_DATA_HOME="$manual_root/data" XDG_STATE_HOME="$manual_root/state" \
  "$manual_root/application/usr/bin/eutheto-desktop"
```

The private directory is deliberately retained after the application exits;
reuse it to revisit the fictional project. This is a Linux development-package
procedure, not a supported-platform packaging or final PDF acceptance claim.

1. **Create a fictional Workforce project.** Use January 1–2, 2030 inclusive
   and `America/Chicago`. In People, create `Fictional Worker`. Create the work
   type `Fictional Shift`, category `fictional`, duration `480` with the unit
   set to minutes, unconstrained qualifications, no location and local-wall-clock
   time behavior. Review and save each complete record.
2. **Create work and explicit eligibility.** In Work, choose Saved shift and
   create January 1 `09:00–17:00`, selecting that work type, local-start-date
   reporting and AtLeast coverage with minimum `1`. Review and Apply. In
   Eligibility, check the person's work-type choice, review it, then Save
   reviewed work type choices.
3. **Author the Required rule and score policy.** In Rules, choose the catalog's
   Coverage button; its editor is titled Meet required coverage. Keep it active
   for all people; Review changes, then Save reviewed change. In Score policy,
   create a policy with profile key `keyboard-sample`; keep the single Preferences
   level, four scale-one priority mappings, stable-rank tie-break and no workload
   policies. Review and Apply. Ordinary Authored Optimize requires a score policy;
   structural validation alone does not supply one.
4. **Validate and Optimize.** Run full validation, return to Setup, then Start
   optimization. Inspect the accepted schedule and its fresh native independent
   check. Confirm the one person/shift assignment and original local endpoints,
   not just a recorded Optimal/Feasible label.
5. **Inspect explanation and base capture.** Open assignment evidence from
   Timeline; close it and check focus returns to the exact trigger. Review this
   accepted result as repair base and explicitly save it. That changes the head:
   the accepted original remains immutable and becomes an earlier-revision source.
   Reopen Inspect schedule after the refresh; its check must not claim to validate
   the new head.
6. **Review protection and Repair.** Refresh current selection. Change protection
   for the exact native pair to Hard, then Review manual change. Confirm the
   unlocked-to-Hard before/after facts and that nothing changes before explicit
   Apply. Apply, rerun full validation and choose Repair plan. Inspect the newly
   accepted current-source result. This single-assignment example cannot show a
   different roster; it proves preserving the required protected pair, not utility
   improvement or unique causality.
7. **Compare without mutating.** Choose Compare latest two results. Check both
   original result IDs, source revisions and protection facts. An unavailable
   cross-profile score comparison is not a failed verification or permission to
   invent a better score. Comparison must not change the scenario.
8. **Review recipient outputs.** From the repaired result, prepare the assignments
   CSV review and Share result HTML review. Check exact result ID/checksum,
   scope and privacy disclosures. Optional Share sections start off; generated
   identities remain linkable, not anonymous. Close and release each review and
   check focus returns to its trigger. The keyboard automation proves review,
   not keyboard operation of the OS destination picker; publication and opening
   the saved files remain explicit manual actions.

For broader call-out, overnight, DST, infeasible, stale and Alternatives behavior,
use the [Phase 07 acceptance evidence and corpus](../../docs/roadmap/07-workforce-solving-results-repair-and-export.md).
Raw `eutheto/scenario` fixture JSON is not a GUI-importable portable bundle.
Keep unavailable PDF, spoken AT, supported-platform and human-usability evidence
distinct from the non-PDF automated paths above.

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
| Exercise bundled-worker Linux solve and accepted schedule          | `just e2e true`                    | Same isolated native WebKit runner; unsigned manifest-bound DEB                |
| Build the Vue frontend                                             | `just ui-build`                    | `pnpm --filter @eutheto/desktop run build`                                     |
| Build the native desktop executable without bundles                | `just desktop-build`               | `pnpm --filter @eutheto/desktop run tauri build --no-bundle`                   |

API generation is Rust-owned: use `just generate` and
`just generate-check`, which invoke the corresponding `cargo xtask` commands.

The native development application requires the pinned Rust toolchain and
platform prerequisites described in
[development setup](../../docs/contributors/development.md). The Vite-only
server does not provide the native Tauri/Rust service.

The native runner's default mode uses the real unbundled debug Tauri/WebKit
application, isolated SQLite/XDG data and network namespace, GTK file pickers,
and hardware keyboard input via the existing Nix `xdotool` tool. It exercises
settings conflicts/import/export, portable inspection/re-export/import,
backup/add/replace, real safety-backup I/O failure and explicit bypass/recovery,
deletion review, scoped scenario undo/redo, offline inventory, route recovery
and restart persistence.
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
fixed-period and rolling Maximum Hours rules, and directed Transition Time
between two native-located shifts. Maximum Hours uses native workload-bucket
and calendar picks; an invalid rolling duration preserves the draft and focuses
the field before correction, with both windows restored after process restart.
The scenario refuses a newly active singleton location selection without
mutation, saves the two-location rule after native shift-scope review, and
verifies the same location IDs after restart. It checks stable draft identity,
live scope pages, empty-active-scope refusal and explicit inactivity, and
distinguishes completed, stale, failed and cancelled validation attempts.
It follows an authoritative standalone-coverage shortage to the exact count
field and reviews a manual repair. An America/New_York repeated-time fixture
demonstrates that zero fast errors need not imply full readiness and navigates
the full finding to the second authored weekly start without mutation.
The native runner also checks WebDriver keyboard input and focus while the UI
reports a pending full-validation request. That UI flag does not prove native
admission or entry into the computation loop; input during admitted native
validation remains unverified by this run.

The native Optimize path reads real `app_get_capabilities` and exact
revision-scoped backend options after current full validation. It starts the
selected bounded Quick/Balanced/Deep solve through the generated operation
scope, forwards actual native phases, sends explicit `solve_cancel`, and
refreshes accepted results only after terminal settlement. Release and unmount
detach the observer without cancelling the durable solve. The default `just e2e`
debug build checks unavailable-solver messaging and the native model estimate;
Chromium component checks cover cancellation, delayed progress, recovery and
stale/no-result states without claiming packaged-native acceptance.
`just e2e true` builds an unsigned, manifest-bound OR-Tools DEB, extracts it
into a private runtime, then runs the same real Tauri/WebKit setup through a
native Deep solve. It reads the persisted run and scenario-owned diagnostic
timings, opens the accepted schedule through a separately re-verified native
view, and stops before the debug suite's independent cancellation/restart
checks. These bounded fixture runs do not prove first-result-before-proof
delivery, a calibrated performance distribution, or packaged native cancellation.
The approved-worker `headless_worker` regression holds backend completion
after a real incumbent, reads the committed checkpoint and independently
re-verified schedule before terminal proof, then cancels and reads it again.

`EUTHETO_E2E_RECIPIENT_EXPORT_ONLY=1 just e2e true` runs the focused Phase 07
recipient-export journey. It solves the versioned tiny fixture with the bundled
worker, checks native assignment/summary CSV and combined ICS publication against
the reviewed byte count and SHA-256, cancels an HTML picker and requires a new
review, then checks the published HTML's exact accepted assignment set and private
default participant population. It also checks that no scenario revision changed.
The full packaged run includes this journey. `just e2e true false` reuses the
existing DEB only when application inputs have not changed; such a run proves that
image, not a fresh source build, cross-platform packaging or screen-reader support.

`EUTHETO_E2E_LARGE_RESULT_ONLY=1 just e2e true` runs the focused packaged
large-supported accepted-result performance journey, also included in the full
packaged run. It applies the existing versioned v1 fixture, validates it, runs the
real bundled worker and independently verifies the accepted result without editing
the source revision. Native reads must identify **101 people, 12 resolved shifts,
1,212 candidate pairs and 12 selected assignments**. This is large population /
candidate evidence, **not** thousands of selected or rendered assignment rows.

The journey writes a `large-supported-accepted-result` case to the existing
`.cache/e2e/package10-performance-measurements.json` artifact (each run replaces
that artifact; packaged result evidence and debug setup evidence are separate runs).
It binds fixture and corpus/expectation SHA-256 digests, native scenario/revision,
run/solution IDs, accepted-result/verification/run-manifest checksums, application
and bundled-worker binary SHA-256 digests, release/package identity and host/WebKit
metadata. No participant names, schedule payloads or assignment details are recorded.
It repeats 11 result-view/page/calendar-date transitions twice. The measured actions
use Tab to reach the selected view radio or requested button, native arrow keys to
change views, and Enter to activate page/calendar controls; they do not programmatically
focus or click the measured controls. The cold/warm labels mean first measured
transition and same-session repetition;
Each sample measures in-page **keydown to expected settled native content plus
two animation frames**, excluding WebDriver round trips. It reports native total
and returned item counts separately from rendered table rows/grid cells and retained
element nodes within the accepted-schedule component. The weekly calendar's native
50-item source page is distinct from its virtualized or eight-row rendered page.
This is an interaction/DOM proxy, **not INP, a paint timestamp, backend-only time
or heap-memory evidence**; no latency pass budget is asserted. The existing bounded
32-interaction/128-long-task/1,024-live-region limits and overflow flags apply; an
unsupported long-task observer retains the explicitly non-equivalent event-loop
lag fallback. Fresh measured results must come from the runner, not these docs.
Rebuild with the command above after application/native changes; use
`EUTHETO_E2E_LARGE_RESULT_ONLY=1 just e2e true false` only for repeat runs against
the same unchanged package.

The large-result path also saves keyboard-operated calendar and assignment-detail
screenshots under ignored `.cache/e2e`: native calendar view-control focus, arrow-key view
navigation, opening the assignment explanation with Tab/Enter, then closing and
checking focus returns to its trigger. The focused recipient-export journey
(`EUTHETO_E2E_RECIPIENT_EXPORT_ONLY=1 just e2e true`) adds bounded Tab/Space/Enter
review-only navigation for accepted CSV and Share Result and checks modal focus
restoration, source identity and unchanged revision before its separate publication
checks.
The focused `EUTHETO_E2E_KEYBOARD_WORKFLOW_ONLY=1 just e2e true` journey is also
included in the full packaged run. It creates a fictional one-person Workforce
project using Tab/Enter and ordinary WebDriver key input, creates an assignment type
and shift, an active Required coverage rule and a reviewed authored score policy,
applies eligibility through its review, validates and optimizes with the bundled worker,
opens and closes assignment evidence, reviews
and explicitly applies Hard protection, validates and runs Repair, compares saved
results, and opens review-only CSV and Share flows. Scenario edits, solve admission,
and review/apply decisions in this named journey use UI keyboard events; native reads
are observation only. Screenshots go to ignored `.cache/e2e`. This is a bounded
single-scenario workflow, not a claim that every E2E path or every picker/system
dialog is keyboard-only. The other persistence and fixture journeys still include
non-keyboard actions; none of these tests establish exhaustive focus order, manual
usability, non-color coverage of every state, or spoken assistive-technology output.
Screenshots and DOM/focus assertions are not Orca or screen-reader evidence.

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
