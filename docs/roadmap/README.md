# eutheto implementation roadmap

This directory is the implementation source of truth for **eutheto**, a local-first, open-source constraint-optimization platform. It replaces the working project placeholder used by the source blueprint. The sequence is dependency-driven, not calendar-driven: a phase exits only when every stated gate passes. Architecture, data-integrity, security, accessibility, licensing, and compatibility requirements are release constraints rather than optional polish.

## Authority and unresolved product gates

The final project name and Rust/npm namespace are `eutheto`: Rust crates use `eutheto-*`, npm packages use `@eutheto/*`, repository paths and examples use `eutheto/`, and serialized project-owned media types use the `eutheto/...` namespace. The following are deliberately **not** inferred from the project name and must be resolved by an ADR before their first public use:

- working CLI executable name: `optimizer`;
- reverse-domain desktop application identifiers, including distinct stable/beta identifiers;
- portable project file extension (`.eutheto` is the current proposal, but is not final until its ADR closes);
- Git hosting organization and canonical repository URL;
- governance and private security-reporting contacts;
- code-signing/notarization identities, custody, rotation, protected environments, and release-signing/attestation choices.

Until each gate closes, documents may use `optimizer` only as an explicitly labelled working example and `.eutheto` only as a proposed extension. No public schema, installer, updater channel, shell integration, or file association may freeze them accidentally.

The source-of-truth order is:

1. security and data-integrity invariants in approved ADRs;
2. current scenario/protocol schemas and conformance tests;
3. this roadmap and domain specifications;
4. generated documentation;
5. incidental code comments.

Resolve conflicts by changing the authoritative source and its tests together. [assumptions.md](assumptions.md) records dated version/evidence decisions; it never replaces a requirement in a phase file.

Cross-cutting roadmap contracts complement the numbered phase files: [Performance and Solver UX Targets](performance-and-solver-ux-targets.md) defines provisional latency, budget, progress, and benchmark policy; [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md) defines implementation-independent bundle, import/restore, and privacy-filtered report boundaries. Each contract identifies the phase that owns implementation and release evidence.

[Collaborative Planning and Hosted Service](collaborative-planning-and-hosted-service.md) is the detailed post-MVP Branch-H workflow contract, with Branch-I/N/O integration, operations and portability ownership. Its approved product direction does not add collaboration production work or service-release gates to Phases00–12.

## Product vision

Build a general-purpose, human-friendly planning platform in which ordinary users describe what **must** happen, what they **prefer**, and which trade-offs matter, then receive understandable, verifiable, editable plans. The system is a headless optimization platform with an excellent desktop client, not optimization logic buried inside Tauri.

Every design must continue to pass both tests:

1. deleting the Tauri UI leaves an exceptionally useful headless framework;
2. a person who knows no constraint programming can solve a legitimately difficult problem in the desktop application.

The platform comprises:

- a fast Rust core for scenario commands, validation, routing, solve orchestration, independent verification, explanations, persistence, import/export, a reusable library API, and a first-class CLI;
- a Tauri 2 desktop client using Vue 3, TypeScript, and Vite;
- domain packs that translate workforce scheduling, individual-seat event seating, school timetabling, planned post-MVP household transportation, and future problem families into a solver-neutral planning IR;
- an optional provider-neutral AI interface that can act only through typed, validated, reviewable, reversible application commands.

The public MVP contains production workforce and event-seating packs, bundled OR-Tools CP-SAT workers for supported platforms, experimental Pumpkin only for proven-compatible subsets, local persistence, proposed `.eutheto` editable scenario export/import and full-backup/add-or-replace restore, immutable privacy-filtered offline HTML/PDF result sharing, undo/redo/repair/comparison/explanations, optional BYOK/local AI with preview-and-apply changes, cross-platform artifacts, examples, notices, SBOMs, checksums, and signed update metadata. School timetabling is the first major post-MVP pack and influences foundation contracts without delaying the MVP. [Phase 14](14-transportation-domain-pack.md) is the detailed proposed post-MVP plan for household transportation; it is not current product behavior and selects no calendar, routing, or transit provider.

Conversational planning follows **describe → clarify → preview → explore → compare → decide**, with typed domain meaning and independently verified evidence at every applicable step. Complete reviewed text assistance remains a Phase-10 MVP commitment. Conversational experiments and voice are prioritized early post-MVP deliveries in [Phase 13, Branch K](13-post-mvp-roadmap.md#branch-k--early-conversational-experiments-voice-and-optional-integrations), useful across workforce and seating rather than dependent on a new domain pack. External-assistant access and supported subscription runtimes are separate, gated extensions—not prerequisites for local scheduling or voice.

Optional organizational planning extends the same core through **collect → reconcile → solve → independently verify → explain → approve → publish → change → repair → republish**. The [bounded collaboration pilot](collaborative-planning-and-hosted-service.md#delivery-and-ownership) has its own post-Phase12 entry and complete-cycle exit; it is not a second optimizer or a reason to retrofit a universal organization/person/workflow model into the local MVP. Finish Phase05 and pause for user manual testing before starting Phase06.

## Product principles

- **Human language before solver language.** Say “Required,” “Preference,” and “minimum rest after call”; reserve solver terminology for an advanced inspector.
- **Progressive disclosure.** Guided choices and safe defaults lead; advanced modeling appears only when needed.
- **Deterministic core; optional AI.** Validation, compilation, solving, verification, persistence, scoring, and deterministic explanations are application functions. AI is neither source of truth nor solver.
- **Local-first.** Create, edit, solve, and inspect without accounts or cloud services. Local storage is the default.
- **Open software, explicit deployment support.** Official implemented software capabilities remain available locally/self-hosted with their stated runtime prerequisites, without commercial feature or population caps. Technical safety/resource limits remain; managed capacity and operating assurances are separate from software licensing. Semantic parity, data portability and supported deployment combinations are distinct [evidence-backed promises](collaborative-planning-and-hosted-service.md#open-software-and-deployment-parity).
- **Human control and plan stability.** AI writes are visible proposals; all applied changes use the normal command API, are undoable, and honor revisions. A user can hard-lock what must remain, express softer preferences, and reflow from an accepted plan while explicitly minimizing avoidable disruption. A hard lock is never silently weakened.
- **Verifiability over trust.** Backend claims never make a candidate publishable; original domain meaning is independently evaluated.
- **Explanations and recovery are product behavior.** “No solution” alone is insufficient. Report validation issues and, where evidence permits, sufficient conflicting required rules in domain language. Proposed repairs identify the exact compromise, require explicit review, and apply only through reversible commands; deterministic evidence remains useful without AI.
- **Open architecture without premature generalization.** Stable contracts first; no dynamic native plugins, arbitrary DSL, unsafe code loading, marketplace, or generic workflow engine in the MVP.
- **Comparison before judgment.** Compare accepted plans and revisions semantically—changed decisions, verified score/category deltas, lock preservation, affected entities, and proof status—rather than presenting a raw data diff or an unexplained universal quality score.
- **Preserve and share deliberately.** Editable scenarios, full-library backups, and immutable offline result capsules are distinct artifacts. Sharing starts from an accepted sanitized presentation model, shows an exact privacy preview, and remains useful without an account, server, or network.
- **Beautiful means clear.** Calm, legible, fast, accessible, restrained interfaces—not an unrelated-card dashboard.

## Goals

- One installer per supported platform; no separate Python, Java, Rust, Node.js, OR-Tools, or solver installation.
- Equivalent optimization through the Rust library, working CLI, and desktop application.
- Workforce scheduling with clinic/on-call coupling, eligibility, availability, coverage, overlap, rest, fixed/rolling hours, consecutive work, skill mix, travel, fairness, preferences, locks, and repair.
- Individual-seat event seating with deterministic integer geometry, table/seat adjacency, orientation, physical proximity and distance, accessibility, same/different table, exclusions, and locks.
- Planned post-MVP household transportation with calendar-derived commitments, vehicle/driver coordination, carpools, pickups, drop-offs, strict no-transit-first solving with per-person opt-in transit fallback, and independent verification.
- Solver-neutral planning representation and stable routing to OR-Tools, Pumpkin, native algorithms, and future reviewed backends.
- Independent verification and authoritative score recomputation for every accepted candidate.
- Domain-language explanations for validation, infeasibility, assignment, trade-off, comparison, and repair changes.
- Lock, prefer, move, and repair/reflow interactions that preserve an accepted-plan baseline, minimize unnecessary change, summarize what changed, and retain the prior accepted result.
- Previous-versus-current semantic comparison plus reliable pack-owned buffer/tightness indicators where the domain can calculate them without false precision.
- Optional provider-neutral AI with BYOK and compatible local endpoints.
- Domain packs independent of OR-Tools, Pumpkin, persistence, network providers, and desktop implementation.
- Permissive official distribution with complete third-party notices and release SBOMs.

## Explicit MVP non-goals

- multi-user real-time collaboration or a hosted SaaS backend;
- mobile applications;
- a dynamic third-party marketplace or untrusted native libraries;
- arbitrary user-authored solver code or a universal untrusted expression DSL;
- guaranteed global optimality for every size;
- automatic decomposition of tightly coupled models across solvers;
- unofficial consumer OAuth or browser-cookie reuse;
- payroll, clinical, legal, labor-law, or regulatory certification;
- automatic authoritative interpretation of regulations;
- complete school timetabling in the first public MVP.

## Success criteria

### Engineering

- Core and CLI build/test independently of Tauri.
- A UI-created scenario exports, solves in the CLI, and imports with equivalent meaning and results.
- No accepted solution violates any required rule under the independent verifier.
- Desktop forms, imports, CLI, and AI use the same transactional, revisioned command API and undo/redo journal.
- Worker absence, mismatch, malformed output, crash, timeout, or cancellation cannot corrupt scenarios or crash the desktop process.
- Unsupported backends are blocked by an exact compatibility report before solving.
- Database, bundle, Portable Scenario, Result, Share Result, pack, and protocol migrations preserve every supported format and reject unknown required semantics safely.
- Fresh-install full-backup restore, add/replace recovery, collision handling, malicious archive rejection, and atomic failure behavior pass permanent fixtures.
- One-file HTML and direct PDF are built only from accepted immutable results and the exact privacy preview; core meaning works from `file://` with no network and accessible list/table parity.
- Primary workforce and seating flows are keyboard-complete.
- The application remains fully useful with AI disabled.
- Representative warm and cold solves meet the calibrated [performance and solver UX targets](performance-and-solver-ux-targets.md), preserve UI responsiveness, and report limit/proof status accurately.

### Product

A first-time user can create a workforce scenario, add/import people, define work and coverage, add required rules and preferences through plain-language guided controls, validate and optimize, understand whether requirements passed and why an assignment occurred, recognize reliable pack-owned buffers or tight points, lock a decision they value, change another decision and reflow without needless reshuffling, compare the revised plan with the prior accepted plan, and receive actionable reviewed guidance when a requested plan cannot work. They can also distinguish editable export from full backup and immutable sharing, restore safely, and create a privacy-reviewed offline result capsule—without learning operations-research terminology.

## Approved architecture decisions

These decisions remain approved until superseded by a numbered ADR with context, alternatives, consequences, status, and supersession links.

| ID | Binding decision |
|---|---|
| ADR-001 | The optimization core is a reusable Rust library with a headless CLI; Tauri is a client. |
| ADR-002 | Desktop uses Vue 3, TypeScript, and Vite; Nuxt is reserved for the future docs/site app. |
| ADR-003 | Project code is Apache-2.0 and contributions initially use DCO sign-off, not a CLA. |
| ADR-004 | OR-Tools CP-SAT is the stable primary backend in a bundled native worker process. |
| ADR-005 | Pumpkin is an in-process Rust backend labelled experimental until compatibility, cancellation, packaging, and benchmark gates pass. |
| ADR-006 | Domain packs compile to solver-neutral planning IR and never construct backend objects. |
| ADR-007 | Every accepted solution is independently verified against the original domain scenario. |
| ADR-008 | MVP routing chooses one backend for a connected model and splits only mathematically and semantically independent components; cross-solver decomposition is post-MVP. |
| ADR-009 | Official MVP packs are compiled in. Future third-party packs use a sandboxed WASM/component model, not native dynamic libraries. |
| ADR-010 | SQLite is the local source of truth; credential values are stored only in the OS credential store. |
| ADR-011 | Scenario mutation uses typed commands, batches, inverses, periodic snapshots, and undo/redo—not distributed event sourcing. |
| ADR-012 | Tauri exposes coarse-grained commands and generated checked-in TypeScript DTOs; Rust owns authoritative scenario state. |
| ADR-013 | AI reads bounded context and proposes typed commands; it cannot bypass validation, write arbitrary files, run arbitrary commands, or generate solver source. |
| ADR-014 | BYOK and local endpoints are MVP authentication; OAuth is allowed only when a provider officially supports suitable third-party authorization. |
| ADR-015 | Nix Flakes are canonical for Linux/macOS development; native pinned Windows tooling/CI is authoritative for WebView2, sidecars, installers, and signing. |
| ADR-016 | Releases include checksums, SPDX SBOM, solver/version manifests, and third-party license notices. |
| ADR-017 | Time has an explicit scenario IANA zone and DST policy; solver quantities use checked integer units. |
| ADR-018 | The public external representation is eutheto's versioned scenario document/bundle, never an OR-Tools or MiniZinc model. |

## Temporary operating decision record

[Proposed ADR-019](../adr/019-phase06-integration-and-verification.md) records the explicit user-authorized Phase06 integration trial. It is not part of the approved ADR set and does not close governance appointment gates. The [contributor workflow](../contributors/git-workflow.md#temporary-phase06-integration) defines its exact branch scope, verification cadence, and expiry.

## System architecture and dependency direction

```text
Clients: Vue/Tauri | CLI | optional AI | future API
                         │
                 typed application API
                         │
Rust core: commands, persistence, domain registry, validation,
planning IR, routing, jobs, verification, explanations, import/export
             │                              │
 in-process reviewed backend       versioned worker protocol
 (Pumpkin/native algorithms)        OR-Tools CP-SAT worker
```

Layer order is presentation → thin Tauri adapter → application services → domain packs → planning core → backend adapters → infrastructure. Dependencies point inward:

- presentation knows application DTOs, never tables or solver APIs;
- Tauri knows application services; application services know domain/persistence interfaces;
- domain packs know pack API and planning IR, not solver implementations, Tauri, SQLite, credentials, or providers;
- backends know solver API and planning IR, not domain packs;
- AI knows typed commands/queries, not persistence or solver internals;
- verification knows original domain models and normalized solutions, never trusts backend status as rule evidence.

During an OR-Tools solve, the Tauri/Rust process owns Vue, core, SQLite, and optional in-process backends, and launches one worker child per solve. Per-solve lifetime supplies crash isolation, process-tree cancellation, version handshake, cleanup, and upgrade safety. A long-lived pool is post-MVP and must retain those properties.

Canonical solve flow:

1. capture immutable scenario revision $R$;
2. domain-validate $R$;
3. compile deterministic planning IR plus provenance/projection;
4. normalize and validate the IR;
5. fingerprint and route to a compatible backend;
6. consume candidate events;
7. project values to normalized domain assignments;
8. independently verify every required rule and recompute score;
9. expose only verified candidates;
10. atomically persist the best verified solution with revision, model hash, backend/adapter/protocol versions, seed, thread count, options, score vector, and verification report.

A failed solve never mutates the scenario. Worker missing/mismatch/early exit, malformed frame, unsupported primitive, timeout, cancellation, invalid model, crash, failed verification, stale scenario revision, or post-solve database write failure is a recoverable typed error. Solution persistence is a separate transaction after verification.

## Global invariants and stop conditions

Protect these invariants in every phase:

- Tauri is a client, not the optimizer; Rust owns authoritative state.
- Domain packs do not know backend APIs; backends do not know domain packs.
- Every mutation is typed, validated, transactional, revision-checked, durable, auditable, and reversible when semantics permit.
- Every solve is against an immutable revision; stale results are labelled and never silently applied.
- Every backend choice is capability-checked before execution.
- Every candidate remains untrusted until independent domain verification and authoritative score recomputation pass.
- “Optimal,” “feasible,” and conflict-minimality wording exactly matches evidence. A sufficient infeasibility set is never called minimal unless proven.
- User IDs are stable typed IDs, never display names, array positions, or database row IDs. Units, zones, DST policies, clocks, seeds, and resource limits are explicit.
- Canonical output and hashing have deterministic ordering; checked arithmetic rejects overflow before a backend.
- Unknown/newer schemas and protocols fail safely; unknown extension data is preserved where practical; public data is never silently lost.
- Secrets never enter Vue, logs, SQLite, exports, diagnostics, Nix derivations, repository files, or ordinary IPC responses.
- Imported files and bundles are untrusted and bounded; parses/migrations finish before one atomic commit.
- Portable data is a versioned implementation-independent schema, never a SQLite dump. Export writes the current schema; import migrates supported history and rejects unknown semantics before preview and one atomic commit.
- Editable scenario export/full backup and immutable result sharing are distinct. A self-contained report is built only from a privacy-filtered Share Result Model tied to an accepted result; it never defaults to the source scenario or requires a network/server.
- Every solve and nested fallback/diagnostic operation consumes one bounded parent budget; an unverified incumbent, provider call, AI response, or optimality proof never blocks access to an already verified result.
- AI is optional and cannot exceed typed command/query permissions or skip preview/apply.
- Official artifacts remain one-install and comply with the permissive license policy.
- Accessibility has equivalent keyboard/list paths; canvas is never the only representation.

Pause work and write an ADR before continuing if a rule cannot be independently verified; a backend needs domain-specific knowledge; UI needs direct database access; AI needs arbitrary file/code/shell access; a plugin needs native in-process loading; a dependency falls outside license policy; migration cannot preserve scenarios; decomposition cannot prove independence; packaging requires disabling a major security control; or correctness relies on undocumented provider/backend behavior.

## User manual checkpoint gates

Approved on **2026-09-09**, the following checkpoints are mandatory phase-exit and next-phase-entry gates, in addition to the existing Phase-05 manual checkpoint. They are not optional demonstrations or substitutes for the phase's automated, independent usability, accessibility, security, or release evidence.

| Boundary | User testing focus |
|---|---|
| Phase 06 → Phase 07 | Real desktop setup: people CSV mapping/preview/apply/rejected rows, work and availability, supported rule editing and truthful Required/Preference availability, validation and correction, keyboard/focus behavior, undo/redo, restart persistence, and scenario/backup recovery. Do not imply that Phase-07 solving or result screens already exist. |
| Phase 07 → Phase 08 | Complete Workforce workflow: create/import, optimize, inspect independently verified results and explanations, lock/edit/repair, compare, and export/share with accurate privacy and stale-result behavior. |
| Phase 09 → Phase 10 | Complete Seating workflow: guest/venue setup, equivalent canvas and accessible list/table editing, solve/explain, lock/repair, undo/persistence, and accepted-result export/share. |
| Phase 10 → Phase 11 | Focused AI trust and privacy: supported profile setup, bounded context and consent, draft versus applied proposals, explicit Apply and separate solve confirmation, stale/cancel/error recovery, one-step undo, retention/deletion controls, and the complete deterministic application with AI disabled. |
| Phase 11 → Phase 12 | Actual immutable release candidates, not a development build: installation, offline use, existing-project migration, backup/restore, update and recovery. Record the platforms and exact artifact digests personally tested; this does not replace the required supported-target matrix. |

For every checkpoint:

1. Complete the current phase's implementation and prerequisite verification, then **stop before starting the next phase's implementation**. Passing CI, merging a PR, or an earlier checkpoint does not clear this gate.
2. Provide a reproducible manual-test handoff: tested source commit and applicable artifact digests, actual launch/install commands, sample data and ordered workflows, expected results, known limitations, platform coverage, and safe handling of existing user data. Never invent commands or implemented behavior, or include credentials or captured private scenarios in evidence.
3. Wait for the user's manual testing and feedback. Fix failures in the current phase and rerun affected automated and manual checks; do not treat untested or blocked checks as passes.
4. Record the tested candidate, results, resolved feedback and the user's **explicit checkpoint acceptance and authorization to proceed**. Silence, automated checks, merge approval and agent confidence are not acceptance.

Read-only planning and investigation may continue while waiting; dependent production implementation may not. A changed candidate requires fresh evidence for affected behavior; release-candidate changes also obey Phase 12's exact-artifact evidence rules. Checkpoint acceptance does not authorize signing or public publication and cannot waive existing security, data-integrity or release gates.

There is **no additional broad user pause after Phase 08**: its required focused backend, routing and cancellation regressions remain mandatory. Phase 12 still performs the full formal release QA and publication gate; the Phase-11 checkpoint is its entry condition, not release approval. The already-completed Phase-05 checkpoint is not reopened by these new gates.

## Dependency graph and delivery strategy

```text
00 repository/tooling
  └─> 01 core types, persistence, commands, CLI/Tauri shell
        └─> 02 domain-pack API, planning IR, solver/verifier contracts
              └─> 03 OR-Tools worker/adapter
                    └─> 04 independent verifier/explanation evidence
                          └─> 05 workforce core vertical slice
                                [entry requires both 03 solving and 04 verification]
                                └─> 06 desktop design system/workforce setup
                                      └─> 07 workforce results/repair/export
                                            └─> 08 Pumpkin/router
                                                  [entry requires the Phase-07
                                                   workforce fixture/benchmark corpus]
                                                  └─> 09 seating domain/canvas
                                                        [entry also requires the established
                                                         solver, verifier, accepted-result,
                                                         repair, router, and desktop foundations]
                                                        └─> 10 typed AI tools/UI
                                                              [entry requires complete deterministic
                                                               workforce and seating flows]

00–10 all complete ──> 11 packaging/docs ──> 12 stabilization/release
                                               ├─> 13 independent school/platform/assistant/collaboration tracks
                                               └─> 14 post-MVP transportation
```

The arrows are phase-entry gates, not merely suggested sequencing: transitive prerequisites remain mandatory where a later node names only its immediate predecessor. Within an entered phase, genuinely independent work packages may proceed in parallel after the contracts they consume are stable, but parallel preparation never waives an entry gate or permits an integration/exit claim early.

Phase 13 and Phase 14 are sibling post-MVP branches entered directly from completed Phase 12. Neither branch requires completion of the other.

Within Phase 13, the early Branch-K experiment and voice deliveries require the completed Phase-12 public release and their own named prerequisites, not completion of school timetabling, transportation, the documentation site, or the entire Phase-13 immediate milestone. They may release independently; the original school, portfolio, explanation, and site commitments remain intact. General branching/reconciliation, remote service deployment, and organization administration retain their separate applicable gates.

The Branch-H collaboration pilot likewise enters from completed Phase12 plus its own H0/service/operating gates, not from completed school, Branch K or Phase14. Read-only pilot discovery may inform planning earlier but cannot authorize production expansion. Independent eligibility is not simultaneous staffing or an implicit priority change; H/N deployments inherit relevant security controls without requiring unrelated services to implement the entire intake pilot.

| Assistant-related work | Owning phase and boundary |
|---|---|
| Typed commands, revisions, undo, jobs, verification, evidence | Existing Phases 01–04 authorities; reuse them, do not create an AI-specific core. |
| Domain meaning, accessible editing, bounded catalogs and evidence | Phases 05–09; no production AI/audio work or future-domain prerequisite. |
| Named profiles, bounded text discussion, reviewed proposals, separately confirmed solve action | Phase 10; preserve native-only secrets, independent verification, and no AI export/privacy authority. |
| Applicable text-AI setup, recovery, security, accessibility and exact-candidate evidence | Phases 11–12; future voice, MCP, and runtime gates do not delay the MVP. |
| Isolated conversational experiments and staged dictation/realtime voice | Early Phase-13 Branch K; existing asynchronous jobs before optional measured reasoning delegation. |
| Restricted MCP server, conditional official runtime, remote/managed organization deployment | Separate Phase-13 gates; MCP is not an inference protocol, and a local runtime is not inherently hosted. |
| School and transportation assistant evaluations | Extend only after the respective production pack/capability gates; neither is required for workforce/seating AI acceptance. |

After contracts exist, prefer complete thin paths—one person, one shift, coverage → compile → solve → verify → display → edit → undo—over horizontal mock layers. Add each official rule with schema/migration analysis, command DTO/validation, plain-language editor, fast/full validation, planning compilation, backend capability/translation tests, independent verification, provenance/explanation, appropriate AI schema, CLI/document example, edge/infeasible fixtures, user limitations, and model/benchmark review. Solver formulation alone is never “done.”

Use trunk-based development, short-lived branches, protected `main`, and tags from tested commits. Add schema/protocol compatibility fixtures with the change, not at release time. Cargo features are sparse and explicit (`backend-pumpkin`, reviewed future adapters, diagnostics, platform integrations); required public packs are normal-build features. Frontend experiments use a typed application feature registry, never ad-hoc local-storage flags.

The remainder of Phase06 has one user-authorized, time-bounded [integration-branch exception](../contributors/git-workflow.md#temporary-phase06-integration). Package dependencies may consume exact verified phase commits; `main` stays the permanent trunk and all final-main/manual gates remain required.

## Shared definitions

| Term | Roadmap meaning |
|---|---|
| Scenario | User-owned planning project at a revision: entities, required rules, preferences, settings, and optional base solution. |
| Domain pack | One problem family's concepts, commands, validation, compilation, verification, views, explanations, imports/exports, metadata, fixtures, and optional official UI. |
| Domain model / Domain IR | Pack-specific normalized semantic representation after parsing/defaults. |
| Planning IR | Shared immutable solver-neutral Boolean/integer/interval representation. |
| Backend | Solver or exact specialized algorithm accepting supported planning IR and returning candidate values/evidence. |
| Worker | Separate native process isolating a backend such as OR-Tools. |
| Router | Deterministic capability/structure policy selecting a backend and execution plan. |
| Candidate | Raw backend values not yet independently verified. |
| Normalized solution | Backend-independent projected assignments using stable domain IDs. |
| Accepted/verified solution | Normalized solution whose required rules and authoritative score were recomputed successfully. |
| Organizational approval | Post-MVP governance decision bound to an exact independently verified result, input revision and policy; not solver acceptance or a local selection. |
| Published plan version | Post-MVP immutable, authorized recipient distribution with version/current-pointer/audit linkage; not a second assignment authority or a synonym for local file export. |
| Required rule | Condition every accepted solution must satisfy (“hard constraint” only in internal advanced terminology). |
| Preference | Explicit bounded objective contribution that may remain unmet. |
| Score vector | Ordered domain-owned objective levels with category breakdowns for comparing verified solutions. |
| Provenance | Stable chain from domain fact/rule through planning/backend records to evidence and localized explanation. |
| Assumption | Boolean switch a compatible solver uses to report a sufficient conflicting set. |
| Infeasibility core | Sufficient active assumption/rule set that cannot all hold; not necessarily minimum. |
| Lock | Absolute preservation (hard) or high-priority stability preference (soft). |
| Repair | Re-optimization after changes while minimizing differences from a base solution. |
| Counterfactual | Temporary diagnostic solve forcing/forbidding a choice to measure feasibility or score effect. |
| Domain command | Typed, validated, reversible scenario mutation. |
| View model | Purpose-built UI/CLI response, not authoritative storage. |
| Bundle | Versioned, bounded portable archive of scenario meaning and permitted assets; `.eutheto` is the proposed public extension pending its identity ADR. |
| Portable Scenario Model | Strict implementation-independent semantic representation used inside portable bundles; separately versioned from SQLite/internal persistence. |
| Result Model | Immutable accepted output tied to one scenario revision, verification checksum, score, and reproducibility metadata. |
| Share Result Model | Versioned privacy-filtered presentation representation built from an accepted Result Model, not the source Scenario Model. |
| Result capsule | Product term for an immutable privacy-reviewed offline artifact rendered from a Share Result Model, initially standalone HTML or direct PDF; it is not an editable scenario, backup, or additional source-of-truth format. |
| Scenario Compare | Semantic comparison of recorded inputs, locks/preferences, accepted decisions, verified score categories, affected entities, and proof state—not a raw JSON/database diff. |
| Plan Health | Pack-owned typed evidence about buffers, tightness, dependencies, or criticality; never an unexplained cross-domain score or unsupported probability. |
| Restore | Consequential backup operation that adds to or atomically replaces portable library data under explicit preview, confirmation, and safety-backup policy. |
| Model hash | Canonical digest of planning meaning, versions, compiler/adapter, and relevant options. |
| Optimal | Backend proved no better result for its exact encoded objective under its proof semantics, and the candidate verified. |
| Feasible | A verified required-rule-compliant solution exists, without a proof of optimality. |

## Definition of done

A core feature documents contracts, returns typed actionable errors, tests boundaries/property invariants, handles serialization/versioning, considers cancellation/resources, emits structured redacted logs, passes Nix/native builds, and updates relevant docs/ADRs.

A domain rule additionally completes the full rule checklist, states unambiguous required/preference semantics and safe empty/default scopes, agrees between compiler/verifier on exhaustive small cases, includes boundary and infeasible cases, has readable result/explanation UX, survives import/export, rejects invalid AI shapes, and measures model-size impact.

A backend has approved license/distribution, visible pinned version, implemented compatibility matrix, contract tests for every claimed primitive, normalized status/cancel/timeout behavior, verified candidate projection, crash/malformed-output handling, target packaging smoke tests, benchmark comparison, and accurate stability label.

A desktop flow includes normal, empty, loading, stale, error, and offline states; keyboard and accessible focus/name/announcement behavior; safe optimistic/transient state; conflict and undo/redo semantics; large-fixture profiling; representative usability evidence; and current screenshots/help.

An AI capability has a deterministic non-AI equivalent, typed allowlisted risk-classified tools, preview/apply writes, stale-revision and injection/malformed-call tests, documented secret/data scope, state-safe provider failures, inspectable deterministic evidence, and a fake-provider CI path.

## Cross-cutting roadmap specifications

| Specification | Contract |
|---|---|
| [Performance and Solver UX Targets](performance-and-solver-ux-targets.md) | Provisional end-to-end latency objectives, one-budget solve policy, responsive progress behavior, instrumentation, representative benchmark packs, and Phase-12 calibration. |
| [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md) | Proposed `.eutheto` bundle/version/migration contract, bounded import and atomic restore, immutable privacy-filtered HTML/PDF result sharing, security evidence, and phase ownership. |
| [Collaborative Planning and Hosted Service](collaborative-planning-and-hosted-service.md) | Post-MVP open deployment parity, scoped participant intake/authority, verified approval/publication, privacy/retention/recovery, complete pilot and managed-operation gates; preserves all source PRODUCT/ACCEPT identifiers without claiming implementation. |

## Phase navigation

| Phase | Outcome |
|---:|---|
| [00](00-repository-and-reproducible-tooling.md) | Reproducible, legally complete monorepo/toolchain and CI/generation skeleton. |
| [01](01-core-application-shell-and-persistence.md) | Durable typed application shell, command journal, project/scenario storage, CLI and real Tauri persistence. |
| [02](02-domain-pack-and-planning-ir-contracts.md) | Stable pack, planning IR, projection, provenance, score, backend compatibility, and router contracts. |
| [03](03-ortools-worker-vertical-slice.md) | Pinned isolated OR-Tools worker and protocol vertical slice. |
| [04](04-independent-verifier-and-explanations.md) | Independent verifier, authoritative scoring, and explanation evidence. |
| [05](05-workforce-core-vertical-slice.md) | Workforce entities, validation, compiler, verifier, and CLI vertical slice. |
| [06](06-desktop-design-system-and-workforce-setup.md) | Accessible desktop design system and guided workforce setup. |
| [07](07-workforce-solving-results-repair-and-export.md) | Workforce solve/results, explanations, repair, comparison, and export. |
| [08](08-pumpkin-backend-and-router.md) | Gated experimental Pumpkin adapter and deterministic routing. |
| [09](09-seating-domain-and-venue-experience.md) | Seating domain, deterministic geometry, accessible venue experience, solve/repair/export. |
| [10](10-ai-assistant-mvp.md) | Optional provider-neutral text AI with named profiles, native keyring credentials, privacy controls, and reviewed typed proposals. |
| [11](11-public-mvp-packaging-and-documentation.md) | Public-MVP packaging, updater, supply-chain artifacts, and documentation. |
| [12](12-stabilization-and-public-release-gate.md) | Correctness, usability, accessibility, security, performance, compliance, and public release gate. |
| [13](13-post-mvp-roadmap.md) | School/platform expansion plus independently gated early conversational experiments and voice; later external-assistant and managed-service extensions. |
| [14](14-transportation-domain-pack.md) | Proposed post-MVP household transportation pack with provider-neutral immutable snapshots, verified planning, and gated calendar/routing/transit adapters. |

## Complete blueprint coverage matrix

Every numbered source section and appendix is assigned below. Shared sections are intentionally carried into multiple phase gates.

| Source | Primary roadmap ownership |
|---|---|
| §1 Purpose | This index; all phase files inherit the contract-first intent. |
| §2 Executive summary | This index; phases 03, 08, 11 for backend/distribution details. |
| §3 Product vision | This index; phases 06, 07, 09, 10 for user-facing realization. |
| §4 Goals/non-goals/success | This index; phase 12 final verification. |
| §5 ADR-001–018 | This index; phase 00 creates ADR files; affected phases enforce them. |
| §6 Delivery phases | This index and phases 00–14. |
| §7 System architecture | This index; phases 01–04 enforce process/data/dependency boundaries. |
| §8 Monorepo layout | Phase 00. |
| §9 Core Rust architecture | Phase 01; solve/backend option contracts continue in phase 02. |
| §10 Commands, undo/redo, audit | Phase 01; AI batch use in phase 10. |
| §11 Persistence, files, migrations | Phase 01; domain migrations in phases 02/05/09/13/14 and release recovery in 12. |
| §12 Domain-pack architecture | Phase 02; official packs in phases 05/09/13/14. |
| §13 Domain model/planning IR | Phase 02; compiler implementations in 05/09/13/14. |
| §14 Solver backend/routing | Phase 02 contracts; phases 03/08 implementations, phase 13 portfolio work, and Phase 14's verified two-stage transportation solve. |
| §15 OR-Tools worker | Phase 03; packaging in 11 and stabilization in 12. |
| §16 Pumpkin/future backends | Phase 08; future backends in 13. |
| §17 Verification/scoring/explanations | Phase 04; domain realizations in 05/07/09/13/14. |
| §18 Workforce pack | Phases 05–07. |
| §19 Seating pack | Phase 09. |
| §20 School timetabling | Phase 13; foundation IR implications in phase 02. |
| Transportation domain-pack blueprint (2026-08-29) | [Phase 14](14-transportation-domain-pack.md); assumptions and provider/integration gates in `assumptions.md` K.9. |
| §21 Tauri/Vue architecture | Phase 06; shell/API foundation in 00–01; seating/AI surfaces in 09–10. |
| §22 UX specification | Phases 06–07, 09–10; final usability gates in 12. |
| §23 Optional AI | Phase 10; deterministic boundaries established in 01–02/04. |
| §24 Security/privacy/trust | Phases 00–01, 03, 06, 10–12; future plugin/server boundaries in 13. |
| §25 Nix Flakes | Phase 00; release use in 11–12. |
| §26 Testing/CI/quality/benchmarks | Phase 00 installs framework; each feature phase owns its layers; phase 12 executes release gates. |
| §27 Packaging/releases/updates | Phase 11; final clean-machine/release checks in 12. |
| §28 Licensing/governance/contribution | Phase 00 foundation; phases 11–12 exact-artifact compliance; phase 13 future dependency review. |
| §29 MVP implementation plan | Phases 00–12 respectively. |
| §30 Post-MVP roadmap | Phase 13. |
| §31 Sequencing/discipline/compatibility | This index and every phase; rule checklist primarily 05, 07, 09, 13, and 14. |
| §32 Risks/mitigations/stop conditions | This index and each phase risk section. |
| §33 Definitions of done | This index; each phase exit gate; public release proof in phase 12. |
| Appendix A Glossary | This index; phases 01–02 define serialized/API forms. |
| Appendix B Dependencies | Phase 00 version/role matrix; adopting phases own exact feature and license validation. |
| Appendix C CLI | Phase 01 base catalog; solve/solution/backend behavior in 02–09/13/14. |
| Appendix D Tauri API | Phase 01 base/generated boundary; domain/solve/solution/AI subsets in 06–10. |
| Appendix E Worker protocol | Phase 03. |
| Appendix F Workforce example | Phases 05–07. |
| Appendix G Seating example | Phase 09. |
| Appendix H Coding/architecture standards | Phase 00 configures enforcement; all phases comply. |
| Appendix I Backlog map | Phases 00–14 map their named work packages to the corresponding epics. |
| Appendix J Research references | Phase 00 evidence baseline; solver/desktop/AI/release owners re-verify current primary sources. |
| Appendix K Implementation gates | `assumptions.md` ledger plus relevant phase assumption/version gates; phase 12 closes release-readiness gates. |
| Appendix L Handoff | This index's dependency graph and phases 00–14. |
