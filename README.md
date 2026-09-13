# eutheto

`eutheto` is a local-first, open-source constraint-optimization platform for building plans that people can understand, verify, edit, and trust.

The Rust core validates scenarios, translates supported Workforce requirements into a solver-neutral planning model, routes compatible work to the isolated OR-Tools backend, and independently verifies candidates against the original domain meaning. The complete desktop planning experience and additional domains remain roadmap work.

> **Project status:** [Phases 00–05](docs/roadmap/README.md) have established reproducible tooling, transactional local persistence, domain-pack and solver-neutral Planning IR contracts, the isolated OR-Tools worker, independent verification, and the registered Workforce core. The bounded Workforce slice includes its initial five Required rules, the headless people CSV service, and verified CLI solving and JSON/CSV export. Automated acceptance and the maintainer's manual-testing pass are complete. [Phase 06](docs/roadmap/06-desktop-design-system-and-workforce-setup.md), desktop design system and Workforce setup, is the active implementation scope; its editors are not yet implemented. Remaining Workforce rules and result/repair screens, Seating, AI, and signed releases remain roadmap work.

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

| Phase | Outcome |
|---:|---|
| [00](docs/roadmap/00-repository-and-reproducible-tooling.md) | Repository, reproducible tooling, legal baseline, CI, and real desktop boundary |
| [01](docs/roadmap/01-core-application-shell-and-persistence.md) | Core types, commands, SQLite persistence, CLI/Tauri shell |
| [02](docs/roadmap/02-domain-pack-and-planning-ir-contracts.md) | Domain-pack API, planning IR, command and solver/verifier contracts |
| [03](docs/roadmap/03-ortools-worker-vertical-slice.md) | OR-Tools worker protocol and first real solver vertical slice |
| [04](docs/roadmap/04-independent-verifier-and-explanations.md) | Pack-neutral independent verification and explanation foundations |
| [05](docs/roadmap/05-workforce-core-vertical-slice.md) | Workforce domain core and first complete verified domain slice |
| [06](docs/roadmap/06-desktop-design-system-and-workforce-setup.md) | Accessible desktop design system and workforce setup experience |
| [07](docs/roadmap/07-workforce-solving-results-repair-and-export.md) | Workforce solve, results, repair, explanations, import, and export |
| [08](docs/roadmap/08-pumpkin-backend-and-router.md) | Experimental Pumpkin adapter and deterministic backend router |
| [09](docs/roadmap/09-seating-domain-and-venue-experience.md) | Seating domain, deterministic geometry, and accessible venue experience |
| [10](docs/roadmap/10-ai-assistant-mvp.md) | Optional provider-neutral AI proposal and review workflow |
| [11](docs/roadmap/11-public-mvp-packaging-and-documentation.md) | Cross-platform packaging, updater, support data, and public documentation |
| [12](docs/roadmap/12-stabilization-and-public-release-gate.md) | Stabilization, conformance, and public release gate |
| [13](docs/roadmap/13-post-mvp-roadmap.md) | School timetabling and post-MVP platform evolution |
| [14](docs/roadmap/14-transportation-domain-pack.md) | Proposed post-MVP household transportation pack with provider-neutral snapshots and independently verified trajectories |

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
persisted appearance preferences. About renders the bounded offline workspace
license inventory and redacted configured-location status—not exact installer
attribution or completed license clearance.

The generated setup API owns native operation lifetime, progress and cancellation.
Native people-CSV commands support picker-owned immutable snapshots, reviewed
atomic imports and separate rejected-row report saves; their editor/import screens
and the remaining Workforce editors are still roadmap work. See
[desktop behavior and verification limits](apps/desktop/README.md), including
native editing-accelerator, accessibility and packaged-platform gates.
Use the headless Workforce workflow below for the available end-to-end
optimization path.

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
desktop shell and implemented project, portable-file, Settings and About workflows;
it does not provide Workforce editors or live solving. On Linux, `just e2e` builds
the unbundled Tauri application and exercises native settings/portable workflows,
real safety-backup failure/recovery, deletion/history boundaries and restart
persistence with isolated local data and networking.

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
spelling; inverse restoration payloads are pack-owned. This command authority
does not yet provide the Phase-06 Workforce calendar-settings editor.

```sh
optimizer scenario validate ./scenario.json
optimizer solve ./scenario.json --output ./result.json --progress human
optimizer solutions verify ./scenario.json ./result.json
optimizer solutions explain ./scenario.json ./result.json --assignment-id "$assignment_id"
optimizer solutions export ./scenario.json ./result.json --format csv --output ./assignments.csv
```

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

Implementation now proceeds through Phase 06. Changes must preserve the
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
