# eutheto

`eutheto` is a local-first, open-source constraint-optimization platform for building plans that people can understand, verify, edit, and trust.

The Rust core validates scenarios, translates supported Workforce requirements into a solver-neutral planning model, routes compatible work to the isolated OR-Tools backend, and independently verifies candidates against the original domain meaning. The complete desktop planning experience and additional domains remain roadmap work.

> **Project status:** [Phases 00–05](docs/roadmap/README.md) established the reproducible core, the isolated OR-Tools worker, independent verification, the registered Workforce domain, and headless solving and JSON/CSV export. [Phase 06](docs/roadmap/06-desktop-design-system-and-workforce-setup.md) Workforce setup passed its applicable native checkpoint and protected-main checks and integrated through [PR #59](https://github.com/Eutheto/Eutheto/pull/59). Development screens provide People, Work, eligibility, availability, six Required rule kinds, and explicit full validation. Phase 07 has added Maximum Assignment Count, including a reviewed calendar/cap editor and headless compile–solve–verify support; live desktop solves, results and repair remain unavailable. Independent solver-naive and spoken native screen-reader QA are deferred and unverified. [Phase 07](docs/roadmap/07-workforce-solving-results-repair-and-export.md) is active; Seating, AI and signed releases remain roadmap work.

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
results, explain recorded assignments, and export accepted JSON or assignment CSV.
This remains a bounded development planner: the complete Workforce desktop flow,
Seating, repair/comparison product flows, AI, and signed releases remain roadmap work.

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

The Eligibility route edits configured person/type memberships through a bounded,
keyboard-accessible matrix or equivalent paged table. Explicit row/column choices
are reviewed and applied as one undoable batch; they are not assignment-feasibility
results. Availability provides authored records, a calendar and exact interval
list, instant/weekly windows, effective dates, type/location restrictions and inert
source/note fields. Rust resolves time and supplies the person–assignment blocking
inspector. Required-rule editors support twelve implemented kinds, including
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
Use the headless Workforce workflow below for the available end-to-end
optimization path.

The setup overview offers a read-only **Optimize** handoff only for a current,
completed, error-free full validation. Quick, Balanced and Deep describe intended
budgets, not runnable desktop choices or optimality promises. Native capability
still reports desktop solve start/cancel unavailable; the headless CLI remains the
working optimization path. General native-operation status uses actual reported
phases, delays its visible panel for 400 ms, and coalesces assistive announcements.
Cancellation acknowledgement is not a successful terminal result.

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
Eligibility/Availability, Required rules, Validation, portable-file, Settings and
About workflows; live desktop solving remains unavailable.
On Linux, `just e2e` builds the unbundled Tauri application and exercises native
settings/portable workflows, People editing and bulk actions with concurrent-change
recovery, native CSV import/report/no-change behavior, a 100-person membership batch
with undo/redo, availability authoring and blocking inspection, Required-rule review,
full validation and exact-field navigation, restart persistence,
safety-backup failure/recovery and deletion/history boundaries with isolated local
data and networking.

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
```

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
without inferring an accepted base schedule. Soft-lock scoring and interactive
repair are not yet available.

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

Verification and export recheck the complete accepted result against its original
scenario. External solver history, optimality, and timing remain unverified source
metadata. JSON preserves the full accepted artifact, including false decisions;
CSV preserves only selected assignment identities in canonical order. Assignment
CSV v1 begins with `eutheto/assignments,1`, then
`assignment_id,person_id,shift_id`; it is bounded to 16 MiB, 100,000 rows, and
160 UTF-8 bytes per cell. CSV decoding is not an accepted-assignment import API.

Without `--output`, human-format export writes exact raw bytes to stdout.
Global `--format json` requires an export destination so data cannot mix with
the one terminal result envelope. Exits are 0 success, 2 usage, 3 validation,
4 proven infeasible, 5 no verified result within limits, 6 unavailable/incompatible
backend or capability, 7 verification alarm, 8 file/storage failure, 10 revision
conflict, and 130 cancellation. Repair, service cancellation, standalone migration,
configuration files for these new operations, and non-JSON/CSV result exports
remain explicitly unavailable.

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
