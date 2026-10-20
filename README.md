# eutheto

`eutheto` is a local-first, open-source constraint-optimization platform for building plans that people can understand, verify, edit, and trust.

The Rust core validates scenarios, translates supported Workforce requirements into a solver-neutral planning model, routes compatible work to the isolated OR-Tools backend, and independently verifies candidates against the original domain meaning. The complete desktop planning experience and additional domains remain roadmap work.

> **Project status:** [Phases 00–05](docs/roadmap/README.md) established the reproducible core, isolated OR-Tools worker, independent verification, registered Workforce domain, and headless exports. [Phase 06](docs/roadmap/06-desktop-design-system-and-workforce-setup.md) setup passed its applicable native checkpoint and protected-main checks and integrated through [PR #59](https://github.com/Eutheto/Eutheto/pull/59). Active [Phase 07](docs/roadmap/07-workforce-solving-results-repair-and-export.md) development includes fourteen Required rule kinds, twelve authored preference kinds, native Optimize, accepted-result calendars/tables and summaries, explanations, reviewed manual changes/Repair, Alternatives/comparison, CSV/calendar exports and privacy-reviewed offline HTML/PDF reports. A debug desktop without the bundled worker truthfully reports Optimize unavailable. Bundled desktop PDF acceptance, final Phase07 manual acceptance, solver-naive and spoken native screen-reader QA, Seating, AI and signed releases remain incomplete or unverified.

## Product direction

The public MVP is planned to include:

- a reusable Rust core and first-class headless CLI;
- a Tauri 2 desktop application using Vue 3, TypeScript, and Vite;
- production workforce-scheduling and individual-seat event-seating domain packs;
- a solver-neutral planning IR and deterministic backend routing;
- OR-Tools CP-SAT in an isolated, versioned native worker process;
- an experimental Pumpkin backend for explicitly proven-compatible subsets;
- independent verification and authoritative score recomputation for every accepted result;
- local SQLite persistence, revisioned commands, undo/redo, repair, comparison, explanations, and import/export;
- optional provider-neutral AI that can propose only typed, validated, reviewable, reversible application actions;
- accessible, keyboard-complete primary workflows and equivalent non-canvas representations;
- cross-platform artifacts with checksums, third-party notices, solver manifests, and SPDX SBOMs.

School timetabling is the first planned post-MVP domain. Its modeling needs constrain foundation contracts without delaying the MVP.

## Principles

- **Local-first:** core planning works without an account or cloud service.
- **Human language first:** use domain concepts such as `Required`, `Preference`, and `minimum rest`, not solver jargon.
- **Deterministic core; optional AI:** AI is neither the source of truth nor the solver.
- **Verifiability over trust:** backend status never makes a candidate publishable; original domain rules are evaluated independently.
- **Human control:** scenario changes are typed, previewable, revision-checked, atomic, and undoable where semantics permit.
- **Headless architecture:** removing the desktop client must leave a useful library and CLI.
- **Accessible by design:** keyboard, screen-reader, focus, non-color, and equivalent-list requirements are implementation gates.
- **Open without premature generalization:** stable contracts precede plugins, marketplaces, arbitrary DSLs, or distributed architecture.

## Target architecture

```text
Vue/Tauri desktop ─┐
Headless CLI ──────┼─> typed application API
Optional AI ──────┘            │
                                v
                     Rust application/core
                  commands · persistence · jobs
                  validation · import/export
                                │
              ┌─────────────────┴─────────────────┐
              v                                   v
       official domain packs             solver-neutral planning IR
 workforce · seating · school                        │
                                         deterministic capability router
                                              │              │
                                              v              v
                                    in-process reviewed   versioned worker
                                    backends/algorithms   OR-Tools CP-SAT
                                              │              │
                                              └──────┬───────┘
                                                     v
                                      projection and independent
                                      domain verification/scoring
```

The dependency direction is presentation → thin Tauri adapter → application services → domain packs → planning core → backend adapters and infrastructure. Domain packs do not construct solver-specific objects, and solver adapters do not depend on official domains.

The core accepts candidates only after projection, structural validation, independent evaluation of every required domain rule, and authoritative score recomputation.

## Roadmap

The implementation plan lives in [`docs/roadmap/`](docs/roadmap/README.md). Phases are dependency gates, not calendar estimates:

|                                                                Phase | Outcome                                                                                                                 |
| -------------------------------------------------------------------: | ----------------------------------------------------------------------------------------------------------------------- |
|         [00](docs/roadmap/00-repository-and-reproducible-tooling.md) | Repository, reproducible tooling, legal baseline, CI, and real desktop boundary                                         |
|      [01](docs/roadmap/01-core-application-shell-and-persistence.md) | Core types, commands, SQLite persistence, CLI/Tauri shell                                                               |
|       [02](docs/roadmap/02-domain-pack-and-planning-ir-contracts.md) | Domain-pack API, planning IR, command and solver/verifier contracts                                                     |
|               [03](docs/roadmap/03-ortools-worker-vertical-slice.md) | OR-Tools worker protocol and first real solver vertical slice                                                           |
|       [04](docs/roadmap/04-independent-verifier-and-explanations.md) | Pack-neutral independent verification and explanation foundations                                                       |
|               [05](docs/roadmap/05-workforce-core-vertical-slice.md) | Workforce domain core and first complete verified domain slice                                                          |
|   [06](docs/roadmap/06-desktop-design-system-and-workforce-setup.md) | Accessible desktop design system and workforce setup experience                                                         |
| [07](docs/roadmap/07-workforce-solving-results-repair-and-export.md) | Workforce solve, results, repair, explanations, import, and export                                                      |
|                  [08](docs/roadmap/08-pumpkin-backend-and-router.md) | Experimental Pumpkin adapter and deterministic backend router                                                           |
|         [09](docs/roadmap/09-seating-domain-and-venue-experience.md) | Seating domain, deterministic geometry, and accessible venue experience                                                 |
|                            [10](docs/roadmap/10-ai-assistant-mvp.md) | Optional provider-neutral AI proposal and review workflow                                                               |
|      [11](docs/roadmap/11-public-mvp-packaging-and-documentation.md) | Cross-platform packaging, updater, support data, and public documentation                                               |
|       [12](docs/roadmap/12-stabilization-and-public-release-gate.md) | Stabilization, conformance, and public release gate                                                                     |
|                            [13](docs/roadmap/13-post-mvp-roadmap.md) | School timetabling and post-MVP platform evolution                                                                      |
|                  [14](docs/roadmap/14-transportation-domain-pack.md) | Proposed post-MVP household transportation pack with provider-neutral snapshots and independently verified trajectories |

See [`docs/roadmap/assumptions.md`](docs/roadmap/assumptions.md) for dated package/tool evidence, compatibility exceptions, and unresolved product gates.

## Current repository contents

Phases 00–04 established a working, deliberately bounded optimization foundation:

- locked Nix development and release shells, reproducible Cargo and pnpm
  workspaces, pinned CI, and deterministic repository commands;
- typed application values and errors, transactional SQLite persistence,
  revisioned commands and history, portable import/export, backup/restore,
  a working CLI, a thin Tauri adapter, and a generated TypeScript API;
- statically compiled domain-pack contracts, solver-neutral Planning IR,
  deterministic capability routing, normalized candidate results, and
  generated schema and capability artifacts;
- a pinned OR-Tools CP-SAT worker isolated behind a bounded versioned protocol,
  with verified target manifests and unsigned package smoke on every MVP
  worker target;
- independent projection, domain verification, authoritative scoring,
  candidate quarantine, accepted-result persistence, explanation evidence,
  comparison, and bounded counterfactual application services;
- a Vue 3 explanation-component foundation with typed states, keyboard and
  screen-reader behavior, non-color status cues, and no direct Tauri access;
- conformance fixtures, compatibility and migration tests, fuzz targets,
  architecture checks, license inventories, SPDX SBOM generation, and
  cross-platform hosted validation.

Phase 05 adds the registered Workforce domain, independent original-domain
verification and scoring, and real headless file/stored optimization through the
approved OR-Tools worker. The CLI can fully validate scenarios, retain verified
results, explain recorded assignments, and export accepted JSON, assignment CSV,
Workforce person/shift summary CSV, or combined/per-person iCalendar.
This remains a bounded development planner: Phase07 adds the desktop flows and
recipient HTML/PDF reports below; final manual acceptance, Seating, AI and signed
releases remain roadmap work.

Phase 06 now provides first launch, Workforce project creation, a searchable
active/archived library, a revision-bound setup overview, a bounded change-history
view with one-step undo/redo, and reviewed portable inspection/import/export and
backup/restore/recovery screens. Application Settings
provides separate revision-checked drafts, reviewed nonsecret import/export, and
persisted appearance preferences. Linux native main-window Ctrl+= and Ctrl+-
zoom have automated 200%/400% coverage and maintainer light/dark inspection
across setup routes on the tested host; spoken native screen-reader QA is deferred.
About renders the bounded offline workspace license inventory and redacted
configured-location status—not exact installer
attribution or completed license clearance.

The People and supporting records route provides native record search/detail and
explicit create/edit/delete review for people, qualifications, teams and assignment
types. Concurrent changes invalidate approval; whole-field rebase preserves
unrelated current values and inactive raw input until an explicit choice replaces it.
Current-page bulk actions select up to 50 people for active-date changes, team
addition/removal or confirmed deletion, reviewed and applied as one undoable batch.

The Work route edits planning dates, IANA time zone and explicit daylight-saving
policies, workload buckets, locations, calendars, recurring templates, shift
instances and coverage. Rust prepares local endpoints and reviews complete command
and generation effects before revision/hash-bound apply. Native tables distinguish
wall-clock and elapsed durations; detached edits survive regeneration until an
explicit reviewed resolution. The editable Clinic + on-call starter is synthetic,
not a staffing recommendation, and adds no people or rules.

The generated setup API owns native operation lifetime, progress and cancellation.
The People CSV screen selects a Rust-owned immutable snapshot, requires explicit
delimiter/header/column mapping and identity decisions, and displays native proposed
people and draft validation before one atomic apply. Rejected-row reports can be
saved separately, including after apply; an exact no-change import creates no history
entry. Names are never used to guess identity.

**Eligibility → Import eligibility from CSV** applies sparse configured membership
edits. Explicitly choose comma/semicolon/tab, header and person/type/membership
columns; membership must be exactly `true` or `false`. Use typed IDs or explicit
exact-token mappings, never guessed names. Missing pairs remain unchanged, and
every unskipped invalid or duplicate pair blocks the whole import. Review native
before/after facts and approve one atomic, undoable batch; unrelated Person fields
are preserved and a no-change import creates no history. The source is limited to
16 MiB/10,000 data records, with at most 1,000 changed people. Save the complete
excluded-row report before replacing the source or leaving. Token mappings retain
the existing nonsecret-input policy; credential-like labels may be refused.

**Eligibility → Import eligibility from pasted matrix** accepts tab-separated text
that you explicitly paste into the bounded input. The first record contains an
empty or `personId` corner followed by assignment-type references; subsequent
records begin with a person reference and have exactly the same width. Use typed
IDs or separate explicit person/type token mappings. Membership cells are exactly
`true`, `false`, or empty; empty cells and absent pairs preserve membership.
This edits configured eligibility, not assignment feasibility.

The complete text is limited to 1 MiB UTF-8, 64 total columns and 10,000 membership
cells, including blanks and skipped cells. Oversized input is cleared in full,
never truncated. Ragged records are refused. Every included invalid or duplicate
typed pair blocks apply; explicitly skip individual native cells and re-review.
The review shows logical record numbers including the header and one-based display
columns. Native approval applies one undoable batch, preserving unrelated Person
fields and membership order. A no-change apply consumes the source without history.
Save the complete exclusion report before leaving or replacing the source.
Release the captured source before editing the paste; stale revisions require
re-review, and unknown apply outcomes must be checked in History, never replayed.

**Work → Import shifts from CSV** adds manual shift instances or updates stored
manual/detached instances; it never edits generated occurrences or templates.
Choose the delimiter, header, columns and exact reference mappings explicitly.
A supplied native shift ID matches only that stored instance; an unknown supplied
ID requires explicit Add using the same ID. Without an ID column/value, choose Add
or select one stored update target. Names and dates never establish identity.

New rows require assignment type, local start/end, Coverage JSON and reporting
attribution (`startLocalDate` or `endLocalDate`). For example, exact coverage is
`{"kind":"exact","count":1,"qualificationMinimums":[]}`; the complete `atLeast`
shape and qualification minima are also supported. Tags are a JSON array.
Times must be offset-free local date/times, such as `2030-01-01T23:00:00`, resolved
by Rust using the scenario's timezone and daylight-saving policy. Enter the actual
next-day end date for overnight work; offsets, zone annotations and implicit
rollover are not accepted. Review the original local intent, resolved instants,
offsets and complete before/after fields.

Updates preserve unmapped fields and equivalent raw values, including detached
origin. Empty mapped cells preserve values unless Clear is explicitly selected
for location or tags. Invalid or duplicate unskipped targets block the whole
import. The 16 MiB/10,000-row source and 1,000-change atomic batch limits apply;
no-change imports create no history. Save the complete excluded-row report before
leaving. A revision change requires re-review, not recapturing the source.

**Availability → Import availability from CSV** adds or updates stored availability
and time-off records. New records need a person, one native kind (`unavailable`,
`availableOnly`, `approvedTimeOff` or `requestedTimeOff`), and effective start/end
dates; the end date is exclusive. Supply either both offset-free local instant
endpoints or a complete `weeklyWindows` JSON array. Each weekly window has
`weekdays`, `startTime`, `endTime` and explicit `endDayOffset`; for example,
`[{"weekdays":["monday"],"startTime":"09:00:00","endTime":"17:00:00","endDayOffset":0}]`.
Weekly windows remain authored recurrence, not eagerly expanded occurrences.

Person, assignment-type and location tokens have separate explicit mappings.
Optional type/location restrictions are complete JSON arrays of references.
Updates preserve unmapped fields and equivalent raw spelling. Empty cells preserve
values by default; Clear is available only for restrictions, source and note.
Source/note whitespace is authored data. Switching from weekly to instant requires
both endpoints; mixing weekly and instant edits is refused. Review native endpoint
resolution facts even when an edit makes no change.

Approved time off independently blocks applicable overlaps. Unavailable and
available-only records require matching active Required rules; requested time off
only affects explicitly referencing preferences. Imports never create those rules
or preferences, and kind changes that invalidate even inactive references are
blocked. The same explicit identity, stale-review, atomic Undo/Redo and source/change
limits apply as for shift imports. Save the complete excluded-row report before
leaving; no-change imports create no history.

The Eligibility route edits configured person/type memberships through a bounded,
keyboard-accessible matrix or equivalent paged table. Explicit row/column choices
are reviewed and applied as one undoable batch; they are not assignment-feasibility
results. Availability provides authored records, a calendar and exact interval
list, instant/weekly windows, effective dates, type/location restrictions and inert
source/note fields. Rust resolves time and supplies the person–assignment blocking
inspector. Required-rule editors support fourteen implemented kinds, including
Maximum Assignment Count with a reporting-date calendar and nonnegative cap,
Maximum Consecutive with distinct consecutive reporting dates or consecutive
matching assignments joined by a configured elapsed break, Maximum Hours with
a native workload bucket, fixed calendar or rolling window, membership policy
and nonnegative minute cap, Required Qualification Mix with per-shift
qualification-expression minimums, Fixed Assignment with an exact named person
and shift whose active scope must include both targets, Mutual Assignment
Restriction for selected people on the same shift or on any overlapping shifts,
and directed Transition Time between distinct named locations with an authored
location-pair duration. A clinic shift requiring one Specialist and one Nurse
may count the same dual-qualified person toward both minimums, but each
minimum counts distinct assigned people. Native scope previews and reviewed
commands remain required.
Explicit full validation checks the saved revision, distinguishes stale, failed
and cancelled attempts, and navigates supported
findings to their owning editor fields; it does not prove plan feasibility. See
[desktop behavior and verification limits](apps/desktop/README.md), including
native editing-accelerator, accessibility and packaged-platform gates.
The headless workflow below remains available without the desktop. The setup
overview now offers a native **Optimize** action after current, completed,
error-free full validation when a supported worker is available. Quick, Balanced
and Deep are bounded modes, not optimality promises. Native progress and
cancellation remain separate from independently accepted results; the overview
lists current and historical accepted results, allows one verified terminal
result to be selected for viewing without changing the scenario revision, and
inspects bounded by-person, by-shift/location, UTC-ordered assignment timeline,
selected-assignment, fairness, and Plan Health pages in the original scenario's
time zone. Seven-day person and shift/location calendars show the original
scenario's civil dates with virtual or paged keyboard-accessible rows. Opening
a saved result requests a fresh native check against its original revision
before showing Required-rule, preference-score and exact Plan Health observations;
a recorded run status is not a fresh optimality proof. When two accepted
results are saved, **Compare latest two results** compares the immediately
previous result with the newest through native, independently checked source
evidence. Assignment, Required-rule, metric, protection/selection, affected-entity
and descriptive run changes remain available without comparable scores.
Score ordering and category deltas require recognized, compatible original
objective meaning: units, population/scope, weights, scaling, priority order
and score shape. Rust checks each result's own recorded policy against its
original source; different revisions or lowered policy hashes do not by
themselves make utility incomparable. Matching unknown metadata does not
establish scoring meaning, and historical absent objective context is not
inferred to mean current Authored context. Viewing or comparing does not change
the scenario revision, adopt a base schedule, or create a lock. Separately,
**Review as repair base**
binds one accepted result to a reviewed current revision: Rust re-verifies its
original source, saves its selected assignments as an undoable base schedule,
and requires fresh full validation before another solve. The reviewed
Adjacency and Base Stability preference editors can then select that saved
base; changing a preference also requires fresh validation and a new solve
before judging its effects. A result's **Changes from base** view compares
with the base recorded in that result's original scenario, not later edits;
a recorded source ID alone is not proof of acceptance.

**Manual changes and repair** provides native-reviewed add, remove, same-shift
replacement, and Hard/Soft/Unlocked protection changes. Review checks the
complete selection and Required rules, not just the visible page. A valid Apply
saves the selection and protection as one undoable command; removing a Hard
incumbent requires an explicit unlock in that same review. Original accepted
results stay immutable. Invalid changes retain their draft and offer related
rule/preference editing or a diagnostic against a chosen current accepted result.

After edits, **Repair plan** uses the latest complete manual selection. It
minimizes changed person–shift pairs first, Soft-lock penalties second, then
authored preferences and deterministic rank; Hard locks remain Required.
Run full validation for the new revision before solving. Authored **Optimize**
is unavailable while Soft locks are present. The CLI also supports explicit
original-result repair without adopting a base. The full Phase-07 acceptance
gate and complete result reporting remain unfinished.

**Alternatives** runs Balanced, Preference-focused, Minimal-changes, and
Fairness-first sequentially from one immutable current source: an explicitly
adopted base or selected unverified external starting plan, sharing one total time
limit. Each saved result is independently verified. Exact native outcome
equivalence suppresses duplicate suggestions,
not their History records. Choose two distinct suggestions to inspect assignment
and rule differences; incompatible objective profiles have no aggregate score
ranking. Viewing an alternative does not apply it or change the authored scenario.

Open **Inspect schedule → Share result** to review and save a participant-friendly,
single-file HTML or fixed-layout PDF report. Rust captures the complete accepted
result, not the visible page or filter. Person names, authored labels, staffing, changes and fairness actuals
are off by default; the preview lists included, excluded and unavailable information.
Generated aliases remain linkable, not anonymous. Older accepted results require
explicit consent and retain their original revision. Saving uses the native file
picker, never overwrites an existing file, and consumes that review even on
cancellation; prepare a fresh review to try again. Both formats work offline.
PDF uses a dedicated fixed Typst worker and the same captured report model and
privacy choices as HTML; its layout differs from browser printing. Packaged CLI
PDF export has been checked with full practitioner reports, stored stale results and
no-clobber publication. Bundled desktop PDF picker/lifecycle, assistive-technology,
supported-platform and final manual acceptance remain unverified. See the
[renderer contract](docs/architecture/recipient-pdf.md).

Use **Inspect schedule → Export CSV/calendar** for assignment CSV, people/shift
summary CSV, or a combined calendar. Each accepted-person row also offers a
calendar export, including people with no assignments and people on later pages.
Native capture always covers the complete selected source, not the current view.
The review shows source identities, the full artifact's byte count and SHA-256,
and an explicitly partial inert text sample of at most 8 KiB. These fixed formats
have their own field disclosures; HTML Share switches do not apply.
Saving publishes those exact bytes through the native picker without overwriting
an existing file. Older sources require consent. Assignment CSV V1 has no inline
revision field: the review and suggested filename identify its source revision;
summary CSV and ICS also carry historical revision labels inside the file.

Existing **unverified starting plans** can be selected, renamed and edited in
Setup, including an explicit empty plan. Review covers all pairs, not only the
displayed page; updates and removal are atomic History Undo/Redo steps.
Repair and Alternatives capture the selected saved source without adding
Required locks or accepted ancestry. Changing the next source does not retarget
an active group. Stale drafts require rebase and fresh review; unknown writes
are reconciled through History, never replayed.

To create one from **assignment CSV**, choose **Imported starting plan — unverified**
as the starting source in Setup, then **Import assignments from CSV**. Use strict
Eutheto assignments v1 or explicitly map comma/semicolon/tab columns and exact
endpoint tokens. Choose a new named plan or complete replacement of an existing
plan; every unskipped invalid or duplicate pair blocks the import. Empty plans
require explicit approval. The 16 MiB/10,000-data-record limit includes skipped
records. Review shows complete additions/removals; the separately savable native
exclusion report retains every omitted record, including skips. A changed revision
requires fresh review without discarding the raw mapping. Import is one undoable
command and never grants accepted-result export authority or selects a Repair source.

**Explain terminal status** reads a recorded run's original input and exact
manifest; accepted results are independently reverified against that saved
source. No-result runs have no verified candidate, and a backend-only
infeasible/unbounded status is not a mapped conflict or independent proof.
Explanations from older revisions remain historical, never current-source
validation. After **Run full validation** completes, selecting a finding in the
full report offers **Check this full validation finding**: Rust rechecks its
exact native issue and saved revision, and the Workforce pack renders the
matching native message. For a current `candidate_shortage` finding, it may
also show a sufficient source-level staffing contradiction: an independent
upper bound on possible people is below the authored Required coverage count.
The evidence lists the source Coverage rule and any active Eligibility or
Availability rules used to exclude people; unconditional activity and approved
leave remain source facts, not invented rules. Coverage scope and qualification
filters are omitted from that count, so it can only overestimate possible
staffing. This is not a minimal conflict, a solver result, or a proof about
other infeasible runs. Fast findings do not offer this evidence.

**Explain saved-change facts** reads two exact saved accepted sources without
changing the scenario. It preserves the accepted-plan comparison and separately
shows original-domain Repair facts against the candidate's recorded manual/base
selection when present. Native facts retain expected thresholds, observed totals,
group scope and assignment identities. A recorded baseline is not newly admitted
by this read; missing historical endpoints are explicit, and no unique cause or
universal fairness is claimed. **Explain recorded repair facts for chosen result**
instead binds A to the candidate's recorded original accepted plan, retaining
the separate manual B and accepted C scopes without applying C.

The OR-Tools adapter supports bounded assumption diagnostics in an objective-free,
single-worker satisfaction model with presolve disabled. The Workforce driver
validates complete signed cores before mapping source rule identities and performs
bounded deletion trials with independently checked original-domain witnesses.
Invalid cores never publish a partial mapping; a zero-assumption foundational
proof is distinct from an unavailable empty native core. These paths have local
approved-worker evidence, and the actual packaged desktop displays mapped source
groups. Native solve modes request bounded diagnostics by default; retries keep
their recorded explanation policy. The packaged desktop navigates to the exact
mapped source rule and retains the committed diagnostic-copy receipt through
same-revision refreshes. A copy's normal History Undo leaves the original
unchanged. Private diagnostic JSON uses the native save picker and refuses
to overwrite an existing destination.

Current native run manifests retain compact backend observations bound to the
recorded input, with independently optional objective and bound vectors.
Observed terminal runtime requires a completed-terminal parameter digest;
configured versions alone are not an observation. These remain backend claims,
not acceptance or independent proof. Private diagnostic JSON includes the
checksummed manifest; participant reports do not inherit this private metadata.

For a saved terminal accepted assignment, **Inspect evidence** rechecks
pair-local activity, approved leave, and active Eligibility/Availability against
the original source. On a selected, locally eligible pair it can list up to
128 matching active Time, Assignment Type, Location, Requested Time Off,
Adjacency, and Base Stability preference terms in rule order with authored
IDs. Each term includes its original policy objective level and the exact
pair-local penalty with that pair selected versus unselected; the latter is
not a feasible alternative or a whole-schedule score difference. Finite
resource limits can omit optional terms, so this is not a complete preference
inventory. Requested time off is not approved leave. These local terms do
not explain why the solver selected the pair, attribute fairness or aggregate
score changes, or revalidate it against later edits.

**Check an assignment change** reads native decisions from the same saved accepted
source before starting a 30-second diagnostic. Replace a person (select the target
and exclude the recorded incumbent on the same shift), require another person
without excluding the incumbent, or forbid the incumbent from working that shift.
All other assignments may change, while the original Required rules and score
policy remain intact. A replacement or newly required person must be modeled and
unselected; static exclusions show native source reasons without saving a
question, and an already-assigned target is rejected. Completed questions remain
readable from native per-result history. A verified alternative is retained as
another accepted result, not applied to the scenario, and shows assignment changes
and exact priority/category score differences. An observed worse alternative does
not prove every alternative is worse. Unmapped backend infeasibility remains
inconclusive. Workforce diagnostics support Boolean decisions, not arbitrary
integer, interval or absent-value conditions.

Mapped Workforce conflicts retain their immutable source revision, Required-rule
captions, fixed-fact/Hard-lock counts, and sufficient-versus-proven-minimal status.
Eligible authored Required groups can be reviewed for a new diagnostic copy:
the native transaction preserves the original and journals the copy for undo.
Diagnostic JSON captures a bounded native whitelist before destination selection,
uses private no-clobber publication, and may contain historical domain labels.
It is a local diagnostic, not a redacted public Share Result artifact. Historical
rule inspection cannot redirect an old rule into a newer revision's editor.

General native-operation status uses actual reported phases, delays its visible
panel for 400 ms, and coalesces assistive announcements.
Optimize delays its live progress display for 350 ms, so quick completions
show only their durable terminal status.
Cancellation acknowledgement is not a successful terminal result.
In a bundled desktop solve, the first candidate that passes independent
original-domain verification is committed as a separate local checkpoint and
can be inspected as a read-only schedule while the backend continues. Its
verified schedule remains available after cancellation or backend failure;
the checkpoint does not claim a terminal accepted result, optimality, selection,
or export authority. Later final proof and terminal status remain separate.
An actual native correctness alarm revokes that run's checkpoint authority even
if durable quarantine cleanup fails. Application-service checkpoint reads,
views, explanations, result exports, and exact-request retries then refuse that
source; ordinary cancellation alone does not revoke a verified checkpoint.
Independent verification covers candidate feasibility and score, not a second
global optimality certificate; a terminal `Optimal` status reflects the bundled
backend's checked proof-state report.

## Quick start

Enter the pinned development environment with `direnv`:

```sh
direnv allow
```

or enter it directly with Nix:

```sh
nix develop
```

Then use the repository's canonical `Justfile` commands:

```sh
just install
just check
just cli
just desktop-dev
```

`just cli` runs the non-final working CLI. `just desktop-dev` runs the persisted
desktop shell and implemented project, People/supporting-record, Work,
Eligibility/Availability, Required and Preference rules, Validation, portable-file,
Settings and About workflows. Desktop Optimize runs only when a compatible worker
is bundled; the unbundled debug desktop reports backend unavailability.
On Linux, `just e2e` builds the unbundled Tauri application and exercises native
settings/portable workflows, People editing and bulk actions with concurrent-change
recovery, native CSV import/report/no-change behavior, a 100-person membership batch
with undo/redo, availability authoring and blocking inspection, Required-rule review,
full validation and exact-field navigation, restart persistence,
safety-backup failure/recovery and deletion/history boundaries with isolated local
data and networking.

`just e2e true` additionally builds an unsigned manifest-bound OR-Tools
desktop bundle on Linux and exercises a real native accepted solve, persisted
timings and re-verified accepted schedule in a disposable WebKit run. It is
not a release-signed package. The packaged fixture alone does not prove
first-result-before-proof delivery; an approved-worker core regression holds
the backend open while reading the durable verified schedule and then cancels
the run without losing that checkpoint.

`just e2e true false` re-extracts and tests the existing unsigned bundle without
rebuilding it. Use this only after a successful bundle build when application
inputs are unchanged, for example while correcting E2E steps. The default still
rebuilds the selected application.

`EUTHETO_E2E_LARGE_RESULT_ONLY=1 just e2e true` exercises the versioned
101-person accepted-result fixture and writes bounded native/WebKit performance
evidence. It measures settled keyboard interactions, not INP, paint or heap usage;
the fixture has 1,212 candidate pairs but only 12 selected assignments.
First-transition and repeat samples do not flush system or webview caches.
See [desktop verification](apps/desktop/README.md) for focused modes and limits.

Run `just` to list every supported command. In particular,
`just generate-check`, `just protocol-check`, and `just fixtures-check` verify
checked-in generated and protocol artifacts, while `just licenses` and
`just sbom` produce the Phase 01 supply-chain inventories.

### Headless Workforce

The source-built CLI is deliberately unbundled. On an approved worker target,
assemble and exercise the manifest-bound image before solving:

```sh
just worker-build-cli
just worker-smoke-cli
target/cli-package/optimizer --help
```

Use `optimizer.exe` on Windows. Keep the executable, sibling worker, and
`solver/ortools` resources together when relocating the image. Runtime environment
variables cannot replace its build-bound worker identity. An unbundled build
reports backend unavailability rather than searching for or downloading a worker.

`projects create --pack official.workforce --title "Plan" --output ./scenario.json`
creates a standalone portable scenario without a local library. Omit `--output`
to create a stored project. `scenario show`, `validate`, `apply`, and `batch`
accept stored IDs or explicit files; mutations require `--expected-revision`,
and file mutations also require a new no-clobber `--output` path.
Bare UUIDv7 values always mean stored IDs; use `./name` for an extensionless or
UUID-shaped filename. File errors never fall back to SQLite. Read-only operations
also accept checked single-scenario bundles, not full-library backups.

Workforce portable exports and full backups preserve current editable data,
original result-source revisions, and archived result records. Migrated legacy
and unaccepted native records use a versioned inert archive; their recorded
status and flags do not grant native accepted-result authority. Copies retain
the original archival payload while remapping only its live ownership/source
bindings. Result-bearing export refuses missing exact sources or nonportable
payloads; `backup create --exclude-results` remains available for those cases.
Result-derived outputs still require fresh independent verification. Imported
external starting plans remain explicitly unverified after backup/restore.

`scenario apply` also accepts `setScenarioSettings` with complete `settings` and
`restoration: null`. Workforce timezone/DST-policy changes preserve every stored
manual or detached shift's instants and elapsed duration, re-expressing local
endpoints when required; recurring templates retain local intent. The command
uses the same revision checks and atomic history as other edits. Stored undo/redo
restores exact endpoint representations, including DST-gap intent and timestamp
spelling; inverse restoration payloads are pack-owned. The Work screen uses the
same native settings authority.

```sh
optimizer scenario validate ./scenario.json
optimizer solve ./scenario.json --output ./result.json --progress human
optimizer solutions verify ./scenario.json ./result.json
optimizer solutions explain ./scenario.json ./result.json --assignment-id "$assignment_id"
optimizer solutions export ./scenario.json ./result.json --format csv --output ./assignments.csv
optimizer solutions export ./scenario.json ./result.json --format summary-csv --output ./summary.csv
optimizer solutions export ./scenario.json ./result.json --format ics --output ./schedule.ics
optimizer solutions export ./scenario.json ./result.json --format ics --person-id "$person_id" --output ./person.ics
optimizer solutions export ./scenario.json ./result.json --format html --output ./shared-plan.html
optimizer solutions export "$scenario_id" "$solution_id" --format html --output ./shared-plan.html
optimizer --format json solutions export ./scenario.json ./result.json --format pdf --output ./shared-plan.pdf
```

For two stored accepted results, compare an explicit base and candidate under
the same scenario ID:

```sh
target/cli-package/optimizer --format json solutions compare "$scenario_id" "$base_solution_id" "$candidate_solution_id"
```

The CLI independently re-verifies both original source results before returning
the current V3 comparison envelope. Score ordering requires recognized,
compatible original objective meaning and ordered score shapes, with each side
checked against its own recorded policy. A changed revision or lowered model
policy alone does not prevent comparison. Unestablished scoring meaning still
returns changed assignments, Required-rule facts, metrics, protection/selection
and descriptive run bindings, with score comparison explicitly unavailable.

Workforce `--format summary-csv` emits every source person and resolved shift,
including unassigned rows, with selected assignment counts, source timezone,
solved/as-of revisions, and exact coverage margins. Shift rows retain local
endpoints and source-resolved UTC instants (including DST transitions); output is
bounded and re-verifies the accepted result before publication. CSV cells
containing untrusted display names are escaped for spreadsheet import.

Workforce iCalendar export is combined by default; `--person-id` is valid only with
`--format ics` and selects one stable person UUID. The export first independently
re-verifies the accepted result against the source scenario, then resolves shift
endpoints from that source. `DTSTART` and `DTEND` are UTC (`Z`) values;
`DTSTAMP` uses the export operation's local clock, not an unverified historical
run timestamp. `X-EUTHETO-SCENARIO-TIMEZONE` records the scenario's IANA timezone;
assignment endpoints with subsecond precision are rejected rather than truncated.
Output uses RFC 5545 CRLF content, line folding, and TEXT escaping, and excludes
notes, evidence, private solver provenance, and source scenario titles. A result
already known stale at the revision read labels its standard calendar name and
event summaries with the historical solved revision. Event UIDs are opaque and
distinct per accepted result, so a later export of history cannot replace a
different accepted result's events for the same person and shift. Custom headers
expose stable scenario/person identities, solved revision, the as-of observation,
scenario timezone, accepted/feasible status, and the unverified historical solver
proof. CLI result-envelope metadata for stored JSON, assignment CSV, Workforce
summary CSV, and iCalendar exports records the solved revision, the revision
observed as of the read/verification boundary, and stale-at-revision-read
metadata. A known-stale stored result is blocked before any file or raw output is
published with an explicit warning; repeat the command with `--allow-stale` to
authorize an output visibly labeled stale as of that revision observation. A
concurrent edit may occur after the observation and before no-clobber publication,
so these exports make no claim of being current at publication time. `--output`
never overwrites an existing file.

Workforce `--format html` and `--format pdf` publish the offline recipient report
from the same model as the desktop Share review. A named `--output` is required;
neither format is written to stdout. Person names, authored labels, staffing totals, assignment changes, and
configured fairness actuals are independently opt-in through
`--include-person-names`, `--include-authored-labels`, `--include-staffing`,
`--include-changes`, and `--include-fairness-actuals`. These flags are valid only
for HTML/PDF. Even with every option off, stable identities and schedule structure
remain linkable; this is not anonymization.

Stored HTML/PDF captures an immutable accepted source and checks the live head under
the publication lease; `--allow-stale` explicitly authorizes a historical source.
Explicit-file HTML/PDF independently verifies the complete supplied snapshot/result
without opening a library. Its live head remains unknown, and recorded original
and baseline comparisons do not establish native historical custody. Both paths
share one 30-second preparation/publication budget and preserve an existing
destination. JSON CLI receipts include `modelSha256` and the exact artifact digest
(`htmlSha256` or `pdfSha256`), source revisions and selected privacy options.
PDF requires the packaged renderer and its build-bound manifest; builds without
that manifest report PDF unavailable.

To require one qualified Physician on each scoped shift in the checked-in
synthetic `clinic-tiny` scenario, save this as `skill-mix-command.json`:

```json
{
  "type": "applyDomainCommand",
  "payload": {
    "commandType": "official.workforce.add_rule",
    "payload": {
      "rule": {
        "kind": "requiredSkillMix",
        "id": "01900000-0000-7000-8000-000000000043",
        "active": true,
        "strength": "required",
        "scope": { "people": { "kind": "all" } },
        "qualificationMinimums": [
          {
            "qualifications": {
              "allQualificationIds": ["01a04a99-f980-7d4f-ba87-36abc52c26e9"],
              "anyQualificationIds": []
            },
            "minimum": 1
          }
        ]
      }
    }
  }
}
```

From the repository root, with the packaged CLI image assembled:

```sh
target/cli-package/optimizer scenario apply domains/workforce/fixtures/v1/clinic-tiny.json \
  --commands ./skill-mix-command.json --expected-revision 0 \
  --output ./clinic-tiny-with-mix.json
target/cli-package/optimizer scenario validate ./clinic-tiny-with-mix.json
```

This edits a separate portable file; it does not assert that a clinic-and-call
workflow, live desktop solving, or practitioner-specific workload policy is complete.

To force Synthetic clinician 1 onto the reconciled 1 September clinic occurrence,
save this as `fixed-command.json`:

```json
{
  "type": "applyDomainCommand",
  "payload": {
    "commandType": "official.workforce.add_rule",
    "payload": {
      "rule": {
        "kind": "fixedAssignment",
        "id": "01900000-0000-7000-8000-000000000044",
        "active": true,
        "strength": "required",
        "scope": { "people": { "kind": "all" } },
        "personId": "01a04a99-f980-7d5f-a8da-39b1f78107f2",
        "shiftId": "01a04a99-f980-7578-b426-5976dc49020d"
      }
    }
  }
}
```

```sh
target/cli-package/optimizer scenario apply domains/workforce/fixtures/v1/clinic-tiny.json \
  --commands ./fixed-command.json --expected-revision 0 \
  --output ./clinic-tiny-with-fixed.json
target/cli-package/optimizer scenario validate ./clinic-tiny-with-fixed.json
```

The fixed rule forces that pair; an authored Hard lock also forces its named pair
without inferring an accepted base schedule. Repair scores Soft locks after
pair-change cost. Authored Optimize with Soft locks remains unavailable.

To keep Synthetic clinicians 1 and 2 apart on overlapping matching shifts,
save this as `mutual-command.json`:

```json
{
  "type": "applyDomainCommand",
  "payload": {
    "commandType": "official.workforce.add_rule",
    "payload": {
      "rule": {
        "kind": "mutualAssignmentRestriction",
        "id": "01900000-0000-7000-8000-000000000045",
        "active": true,
        "strength": "required",
        "scope": { "people": { "kind": "all" } },
        "personIds": [
          "01a04a99-f980-7d5f-a8da-39b1f78107f2",
          "01a04a99-f980-74fd-baef-690fcafd45ec"
        ],
        "mode": "overlappingShifts"
      }
    }
  }
}
```

```sh
target/cli-package/optimizer scenario apply domains/workforce/fixtures/v1/clinic-tiny.json \
  --commands ./mutual-command.json --expected-revision 0 \
  --output ./clinic-tiny-with-mutual.json
target/cli-package/optimizer scenario validate ./clinic-tiny-with-mutual.json
```

`sameShift` restricts listed people only on the same shift; `overlappingShifts`
also restricts different shifts whose instant intervals intersect. Neither mode
restricts two assignments to the same person by itself.

For a scenario with two located shifts and a source location whose `transitions`
contains `{"locationId":"01900000-0000-7000-8000-000000000052","minutes":30}`,
replace both example location IDs with that scenario's actual source and
destination IDs and save this as `travel-command.json`:

```json
{
  "type": "applyDomainCommand",
  "payload": {
    "commandType": "official.workforce.add_rule",
    "payload": {
      "rule": {
        "kind": "transitionTime",
        "id": "01900000-0000-7000-8000-000000000046",
        "active": true,
        "strength": "required",
        "scope": { "people": { "kind": "all" } },
        "locationIds": [
          "01900000-0000-7000-8000-000000000051",
          "01900000-0000-7000-8000-000000000052"
        ]
      }
    }
  }
}
```

```sh
target/cli-package/optimizer scenario apply ./located-workforce.json \
  --commands ./travel-command.json --expected-revision 0 \
  --output ./located-workforce-with-travel.json
target/cli-package/optimizer scenario validate ./located-workforce-with-travel.json
```

For one person assigned both matching shifts, a configured source-to-target
entry requires at least its elapsed minutes from the source's end to the
target's start. A missing or reverse-only entry does not infer a gap; No
overlap remains a separate rule. The desktop editor proves two effective
selected locations against native shift scope before newly activating this
rule. Previously saved active singleton selections are loadable no-ops.

To limit each clinician's elapsed work to 16 hours in any seven-day window,
save this as `hours-command.json` for the synthetic `clinic-tiny` bucket:

```json
{
  "type": "applyDomainCommand",
  "payload": {
    "commandType": "official.workforce.add_rule",
    "payload": {
      "rule": {
        "kind": "maximumHours",
        "id": "01900000-0000-7000-8000-000000000047",
        "active": true,
        "strength": "required",
        "scope": { "people": { "kind": "all" } },
        "bucketId": "01a04a99-f980-752b-aa27-15f8bfbe18bb",
        "window": {
          "kind": "rolling",
          "durationMinutes": 10080,
          "membership": "intersection"
        },
        "maximumMinutes": 960
      }
    }
  }
}
```

```sh
target/cli-package/optimizer scenario apply domains/workforce/fixtures/v1/clinic-tiny.json \
  --commands ./hours-command.json --expected-revision 0 \
  --output ./clinic-tiny-with-hours.json
target/cli-package/optimizer scenario validate ./clinic-tiny-with-hours.json
target/cli-package/optimizer solve ./clinic-tiny-with-hours.json \
  --output ./clinic-tiny-with-hours-result.json
```

The cap is a configured scheduling constraint, not a regulatory hours
assessment. The CLI accepts worker candidates only after independent
verification.

To cap each clinician at two consecutive worked reporting dates in the synthetic
`clinic-tiny` scenario, save this as `consecutive-command.json`:

```json
{
  "type": "applyDomainCommand",
  "payload": {
    "commandType": "official.workforce.add_rule",
    "payload": {
      "rule": {
        "kind": "maximumConsecutive",
        "id": "01900000-0000-7000-8000-000000000048",
        "active": true,
        "strength": "required",
        "scope": { "people": { "kind": "all" } },
        "mode": { "kind": "workedDays" },
        "maximum": 2
      }
    }
  }
}
```

```sh
target/cli-package/optimizer scenario apply domains/workforce/fixtures/v1/clinic-tiny.json \
  --commands ./consecutive-command.json --expected-revision 0 \
  --output ./clinic-tiny-with-consecutive.json
target/cli-package/optimizer scenario validate ./clinic-tiny-with-consecutive.json
target/cli-package/optimizer solve ./clinic-tiny-with-consecutive.json \
  --output ./clinic-tiny-with-consecutive-result.json
target/cli-package/optimizer solutions verify \
  ./clinic-tiny-with-consecutive.json ./clinic-tiny-with-consecutive-result.json
```

`workedDays` counts each reporting date once, even if several selected shifts
report it; unworked dates break the run. `assignments` instead orders matching
selected shifts by start instant and joins them when the signed gap from the
previous matching shift's end is at most `breakMinutes`. A nonmatching shift
does not interrupt that run. Neither mode is a legal work/rest compliance preset.

Full validation returns its complete report and exit 3 for errors; an obvious
coverage contradiction is not itself a solver feasibility result. Solve defaults
are Balanced, 30 seconds, automatic backend selection, one worker thread, and seed 1.
`--max-time` accepts a positive integer followed by `ms`, `s`, `m`, or `h`.
Progress goes to stderr while work runs; candidates are not accepted results.
File solves require an output destination. Stored solves retain their result in
the library first; a later optional output failure is a warning, not rollback.
Results always belong to their solved revision and are never silently applied.

Use `--repair-from` to select an exact original accepted result without changing
the input scenario, its revision/history, or that original result:

```sh
optimizer solve ./scenario.json --repair-from ./result.json --output ./repaired.json
optimizer solutions verify ./scenario.json ./repaired.json
optimizer solve "$scenario_id" --repair-from "$original_result_id"
```

Stored inputs accept a saved result ID or accepted-result JSON file. The exact
original revision must be current or retained in that scenario's history. A native admitted manual
selection takes precedence only when it belongs to the same original; incompatible
manual ancestry is refused. Exact request retries retain their recorded manual
generation rather than recapturing later edits.

File inputs accept an accepted-result JSON file, or an exact result ID inside a
single-scenario bundle. The exact original snapshot must be the current input or
retained in that bundle: after a call-out edit, a standalone changed JSON file
plus an old result is insufficient. Imported manual selections cannot establish
native receipt custody. Imported solver proof/history remains untrusted.

Without native manual state, repair retains the complete original pairs only in
the run's objective context. This captured form is limited to 512 KiB canonical
JSON; larger captures fail before solving, without truncation or fallback.
Existing in-snapshot/native manual capacity and whole-result/explanation limits
are unchanged. The repaired result can be independently reverified against its
unchanged current input.
Repair prioritizes changed person–shift pairs, then Soft-lock penalties, then
preference and rank objectives. A `feasible` status is not a claim of global
optimality.

For an existing unverified `externalStartingPlan` entity, select its stable entity
ID explicitly instead of an accepted result:

```sh
optimizer solve ./scenario.json --repair-external "$starting_plan_id" --output ./repaired.json
optimizer solve "$scenario_id" --repair-external "$starting_plan_id"
```

This source records complete current person–shift pairs, including an explicit
empty plan. Repair minimizes changes without turning those pairs into Required
locks or inventing accepted ancestry. Imported fingerprints remain descriptive;
the resulting candidate still requires independent verification. The two repair
source flags are mutually exclusive. Desktop CSV import creates these unverified
records; desktop selection and pair editing preserve their descriptive provenance.

Verification and export recheck the complete accepted result against its original
scenario. External solver history, optimality, and timing remain unverified source
metadata. JSON preserves the full accepted artifact, including false decisions;
CSV preserves only selected assignment identities in canonical order. Assignment
CSV v1 begins with `eutheto/assignments,1`, then
`assignment_id,person_id,shift_id`; it is bounded to 16 MiB, 100,000 rows, and
160 UTF-8 bytes per cell. CSV decoding is not an accepted-assignment import API.
The desktop starting-plan importer has the smaller 10,000-data-record limit above;
it creates unverified intent, not an accepted solution.

Without `--output`, human-format JSON, CSV and iCalendar exports write exact raw
bytes to stdout. HTML/PDF and global `--format json` always require an export
destination so data cannot mix with the one terminal result envelope.
Exits are 0 success, 2 usage, 3 validation,
4 backend-reported compiled-model infeasibility (without a mapped original-rule
conflict), 5 no verified result within limits, 6 unavailable/incompatible
backend or capability, 7 verification alarm, 8 file/storage failure, 10 revision
conflict, and 130 cancellation. Service cancellation, standalone migration,
configuration files for these new operations remain explicitly unavailable.

## Contributing

Read [`AGENTS.md`](AGENTS.md) before changing the repository. It defines source authority, phase discipline, architecture boundaries, generated-code rules, security and privacy constraints, and verification expectations for human and automated contributors.

Implementation now proceeds through Phase 07. Changes must preserve the
applicable roadmap issue IDs and exit gates and avoid claiming later-phase
production behavior. Contributions should prefer complete vertical paths over
mocks, stubs, or speculative infrastructure.

The project is licensed under the Apache License 2.0; see [`LICENSE`](LICENSE)
and [`NOTICE`](NOTICE). Contributions use DCO sign-off as described in
[`DCO.md`](DCO.md) and [`CONTRIBUTING.md`](CONTRIBUTING.md). Governance,
security, and conduct policies are also checked in.

## Name and unresolved public identifiers

The project, Rust crate, npm package, and project-owned media-type namespaces are fixed:

- project: `eutheto`;
- Rust crates: `eutheto-*`;
- npm packages: `@eutheto/*`;
- media types: `eutheto/...`.

The development CLI executable uses the working name `optimizer`, but its final
public name is unresolved. Reverse-domain desktop IDs, the portable project
extension, hosting organization, governance/security contacts, and signing
identities also remain explicit roadmap decisions. Development identifiers are
not public commitments.
