# Phase 12 — Stabilization and Public Release Gate

## Outcome

Turn the immutable Phase-11 release candidates into a public release only after correctness, data integrity, usability, accessibility, security/privacy, packaging, licensing, performance, migration/recovery, documentation, and operational evidence all pass. Publish defensible benchmark baselines and triage release-candidate feedback. Isolate or remove any experimental feature that cannot meet its advertised stability contract. “No known release blocker” means no unresolved severity accepted implicitly: every issue is fixed, explicitly downgraded with evidence and maintainer sign-off, or the affected feature/target is removed from the release.

Candidate construction and artifact contracts are defined in [Phase 11](11-public-mvp-packaging-and-documentation.md). Version and unresolved-decision evidence is in [the assumptions ledger](assumptions.md). Work intentionally outside the public MVP enters the applicable post-MVP branch: the platform and school roadmap in [Phase 13](13-post-mvp-roadmap.md) or the proposed household-transportation pack in [Phase 14](14-transportation-domain-pack.md).

Approval here authorizes the local desktop/CLI software release, not organization approval of a schedule or launch of a collaboration/hosted service. The MVP retains offline, no-account, no-hidden-network operation within the existing optional-provider/updater boundaries, and accepted-only solution-derived outputs. Later collaboration/service releases require separate applicable identity/authorization, privacy/retention/recovery, deployment, and operational evidence under [delivery and ownership](collaborative-planning-and-hosted-service.md#delivery-and-ownership); organization approval and publication follow their own [verified-result contract](collaborative-planning-and-hosted-service.md#verified-approval-and-publication) and can never waive independent verification.

## Source coverage

This phase is the implementation source of truth for blueprint Sections 22.18, 26, 32, and 33; Phase 12; Appendix K.8; the public-MVP scope in Section 6.3; the release-gate portions of Sections 24, 27, and 28; calibration/release approval of [Performance and Solver UX Targets](performance-and-solver-ux-targets.md); and final release evidence for [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md). It closes `QA-001` and `MVP-001` against the exact `SEC-001`, `REL-001`, `REL-002`, and `DOC-001` outputs from Phase 11.

## Dependencies

- All phases 0–11 have exited and supplied candidate source commit, lockfiles, immutable artifact digests, manifests, exact SBOMs/notices, migration fixtures, target support statements, documentation, known limitations, and test/benchmark tooling.
- Release candidates are not rebuilt during approval. Any source, lockfile, build flag, capability, worker, dependency, asset, migration, or documentation change creates a new candidate and invalidates affected evidence.
- Official workforce and seating behavior, independent verification, persistence, CLI, desktop flows, optional AI, worker supervision, updater, backup/recovery, and support-bundle contracts are feature-complete.
- The public release remains fully functional offline and with AI disabled.
- The user has accepted the Phase-11 release-candidate checkpoint against recorded immutable artifact digests and explicitly authorized Phase-12 formal QA under the [manual checkpoint policy](README.md#user-manual-checkpoint-gates). This is not authorization to publish and does not replace any gate below.

## Decisions and invariants

### Evidence hierarchy

1. An independently verified candidate is the only user-visible solution.
2. Domain semantics are tested against exhaustive/small truth sets, not merely against backend status.
3. Compiler and verifier are independent enough for mutation tests to demonstrate detection of compiler defects.
4. Migration and import tests prove transactional behavior and recovery, not just successful happy paths.
5. Packaged-app tests inspect the exact bundled worker and artifact, not an unpackaged development binary.
6. Coverage is diagnostic; it cannot substitute for semantic, differential, metamorphic, accessibility, or manual evidence.
7. Performance gates detect meaningful regression on controlled runners; they do not promise universal wall-clock times.
8. Accessibility, security, data integrity, licensing, and documentation accuracy are release requirements, not optional polish.
9. A time-limited feasible result is never called optimal. A sufficient conflict is never called minimal unless minimality is proven.
10. Tests never depend on current time/date, host time zone/locale, nondeterministic entropy, machine core count, or a live provider. They receive fixed clocks, explicit IANA zones, fixed seeds, temporary directories, and credential/provider fakes.
11. Portable import/restore never commits until archive, checksums, versions, migrations, domain validation, collisions, reconnections, capacity, and destination are fully staged and reviewed. Current builds export only current canonical portable schemas.
12. The exact privacy-preview Share Result payload is the only data accepted by HTML/PDF renderers. A report requiring a network request for core meaning is release-blocking.

### Highest-risk failures

Testing effort is weighted toward these failures:

- a scenario compiles to the wrong mathematical meaning;
- a solver candidate violating a required rule is accepted;
- a migration corrupts or loses a project;
- an infeasibility explanation overstates certainty;
- a packaged worker differs from the tested worker;
- a large model freezes the UI or exhausts memory/log/process limits;
- a portable import/restore drops meaning, imports device state/secrets, or partially mutates on failure;
- a share preview omits a disclosed field, output leaks excluded data, or offline report rendering executes untrusted content;
- an AI tool mutates authoritative state without explicit user approval.

## Exhaustive scope and technical details

### 1. Test layers

#### Pure unit tests

Cover IDs and value objects; explicit units; time-zone, DST ambiguity, interval, duration, and rolling-window calculations; shift-window generation; geometry transforms and classifications; score arithmetic, lexicographic comparison, and bounds; command inverses; strict parser/resource limits; migration functions; router compatibility/capability/policy; stable error codes; redaction; and updater/channel state.

#### Property-based tests

Using `proptest`, prove:

- command followed by inverse restores equivalent authoritative state;
- serialization round trips preserve canonical meaning;
- score comparison is transitive;
- geometry distance is symmetric;
- normalized domains contain exactly represented values;
- generated shift windows cover intended dates without duplicates;
- verifier results do not depend on entity ordering;
- proven-independent component decomposition plus merge is semantically equal to whole-problem solving;
- archive/path normalization cannot escape the staging root;
- version comparisons and stable/beta channel selection obey policy.

#### Domain validation tests

Every workforce and seating rule type has valid, invalid, edge, empty-scope, DST where applicable, unit/boundary, import/export, required/preference, and unsafe-default fixtures. Defaults and legal/regulatory templates remain non-authoritative. AI-produced command shapes pass the same validators.

#### Compiler golden tests

Small scenarios serialize a stable diagnostic Planning-IR form containing semantic IDs and normalized expressions, never unstable memory/debug representations. Snapshot changes require explicit reviewer confirmation that domain meaning is unchanged or intentionally migrated.

#### Backend contract tests

Run the same Planning-IR fixtures against every backend claiming support. Assert capability checking, normalized status, candidate projection, score vector/evidence, cancellation/timeout, and independent verification. An unsupported primitive fails before solve; it is never silently approximated.

#### Compiler/verifier differential and mutation tests

For generated very small instances:

1. enumerate all assignments;
2. independently evaluate each assignment;
3. derive the true feasible and best sets;
4. compare each backend/compiler result to truth;
5. inject deliberate compiler mutations in test-only builds and prove the harness rejects them.

This suite covers official rule interactions, not only one-rule fixtures, and is a mandatory correctness gate.

#### Metamorphic tests

Prove at least:

- adding an irrelevant inactive person cannot turn a feasible model infeasible;
- increasing a required minimum cannot enlarge the feasible set;
- converting a preference to required cannot create new feasible assignments;
- renaming/reordering entities cannot change feasibility or score;
- translating or rotating an entire seating venue preserves distance-based feasibility;
- duplicated solutions that differ only in display metadata verify and score identically;
- adding a disabled rule cannot change results;
- changing stable/beta update metadata cannot cross channels without explicit selection.

#### Migration and persistence tests

Maintain fixtures for every public database, scenario-envelope, domain-schema, planning/solution, and settings version. Test sequential upgrade to current, every supported direct path, unknown-newer rejection, forward-only migration policy, interrupted migration rollback, backup creation/restoration, export after migration, application restart, command journal/undo/redo preservation, and downgrade guard. Released migration files never change; a new migration carries before/after fixture tests. Malformed import/project bundles cannot partially mutate data.

#### Protocol, worker, and process tests

Fuzz and test frame parsing, size/output/log limits, version negotiation, malformed/truncated/unknown output, request-ID mismatch/staleness, worker target/manifest/hash mismatch, executable resolution, process startup failure, crash, cancellation escalation, timeout, exit codes, incumbent events, and restart/recovery. Every projected candidate is independently verified; a worker's status is never accepted as verification.

#### Frontend unit and component tests

Use Vitest 4.1.11, Vue Test Utils 2.5.0, Testing Library Vue 8.1.0 where useful, and `axe-core` 4.13.0 directly through the project test harness. Cover component logic, validation, typed command payloads, stale revision conflicts, proposal diffs, virtualized selection/focus, accessible labels/focus/announcements, status/optimality/explanation language, error/empty/loading/offline states, updater state, and support-bundle preview. Avoid broad generated-markup snapshots; test behavior and accessible output.

#### Desktop end-to-end tests

Use WebDriverIO 9.31.4 and `@wdio/tauri-service` 1.3.0 on supported Windows, Linux, and macOS configurations. Cover:

- first launch and project creation;
- workforce fixture creation/import/validation/solve/verified-result/export;
- seating fixture solve plus synchronized canvas/list interactions;
- cancellation;
- undo/redo;
- export/import round trip;
- lock and repair;
- infeasible-result explanation;
- AI proposal through a fake provider, preview/apply/undo, and AI-disabled path;
- backend crash recovery;
- offline and updater-disabled behavior;
- support-bundle preview/export;
- backup/recovery after migration.

Run a small smoke suite on each pull request and the broad suite on main/release workflows. Playwright 1.62.1 may test the pure Vite UI but never replaces packaged Tauri E2E.

#### Manual release QA

Execute a versioned script against every exact candidate covering:

- all keyboard-only flows, visible focus, focus restoration, shortcuts, and escape/cancel behavior;
- screen-reader basics, names/roles/states, live status/progress/error announcements, table/grid navigation, and a complete non-canvas seating workflow;
- zoom/text scaling, high-DPI, multi-monitor movement, and resizing;
- light, dark, high-contrast where supported, reduced-motion, and non-color state/error cues;
- installer, update, migration backup, restore, uninstall, and local-data retention/deletion behavior;
- macOS signature/notarization/stapling/Gatekeeper;
- Windows Authenticode/timestamp/SmartScreen/WebView2 and normal-user permissions;
- Linux Wayland/X11 and exact AppImage/deb/rpm artifacts selected by Phase 11;
- large representative workforce and seating scenarios;
- first launch and normal use with no network;
- OS credential-store integration, replacement/deletion, absent-store error UX, and no secret disclosure;
- checksum/signature verification, About/solver license metadata, and support-bundle redaction/preview;
- proposed/final `.eutheto` scenario import/export, full backup, add/replace restore, attempted safety-backup failure, collision/reconnection review, cancellation, and fresh-install recovery;
- exact privacy-preview one-file HTML opened from `file://`, direct PDF/print, no-network evidence, malicious-text inertness, immutable snapshot behavior, and accessible list/table alternatives.

### 2. Usability research gate

Recruit representative people who have not used constraint solvers. Each participant must attempt, with the release candidate and public docs:

1. create a small schedule from scratch;
2. import people;
3. express one required rest rule and one preference;
4. understand an infeasible result without mistaking a sufficient conflict for a proven minimal conflict;
5. lock an assignment and repair the schedule;
6. create a seating separation rule based on physical proximity;
7. choose the correct action among Export editable scenario, Back up everything, and Share Result;
8. inspect privacy choices and share an immutable one-file HTML/PDF result offline; and
9. restore a full backup on a fresh installation, distinguishing Add from Replace and understanding the safety-backup outcome.

### 3. Fuzzing and sanitizers

Create and maintain `cargo-fuzz` 0.13.2 / `libfuzzer-sys` 0.4.13 targets for:

- scenario JSON envelope;
- every domain-document parser;
- project-bundle manifest, ZIP path normalization, entry/aggregate/decompression limits, and checksums;
- every Portable Scenario/Result/Share Result schema and sequential migration;
- standalone-report Share Result decoder and inert-data boundary;
- CSV import mapping;
- worker frame decoder;
- normalized-solution parser;
- database and portable migrations;
- custom endpoint URL validation.

Run bounded scheduled CI campaigns plus sanitizer builds; run longer campaigns before major/public releases. Store minimized regression inputs under `fuzz/regressions`, classify crashes/timeouts/OOMs, and add deterministic regression coverage. Fuzzing's nightly Rust toolchain is separately pinned; it does not change the release compiler.

### 4. Coverage policy

Coverage gates are:

- verifier, migrations, and worker protocol: at least **90% branch coverage**;
- core command, persistence, domain, and compiler crates: at least **80% line coverage**, with reviewed gaps;
- frontend business logic and composables: at least **75% line coverage**;
- visual components are judged by behavior, accessibility, packaged E2E, and manual QA rather than markup coverage.

Generated bindings or platform-only code may receive a documented exception. Difficult code is not excluded simply to raise percentages. Coverage artifacts are sanitized and never contain scenario/provider/credential data.

### 5. Benchmark corpus and reproducibility

Maintain versioned fixtures at:

```text
benchmarks/
├── workforce/
│   ├── small/
│   ├── medium/
│   ├── large/
│   └── generated/
├── seating/
├── school/
├── manifests/
└── expected-invariants/
```

School fixtures may exist before the school UI, but do not imply Phase-13 delivery. Every manifest records fixture hash and generator seed, scenario/domain schema/compiler/backend versions, scenario and model-size metrics, candidate counts before/after pruning, warm/cold and cache/enrichment preconditions, expected status when known, required-rule and authoritative-score invariants, permitted backends, total and backend budgets, thread policy, and historical baseline ranges. Real donated scenarios require permission and de-identification; personally identifying data is never committed.

Measure separately:

- scenario load and migration;
- validation;
- candidate generation and deterministic pruning;
- domain normalization;
- local transportation/snapshot lookup and separately disclosed network enrichment;
- Planning-IR compilation;
- backend translation;
- worker startup;
- raw backend first incumbent;
- first independently verified feasible candidate;
- best authoritative score vector within fixed budgets;
- backend solve and termination/proof/bound;
- projection and verification;
- explanation shrinking/counterfactual;
- result post-processing;
- UI initial render and interaction latency;
- memory high-water mark;
- cache hit/miss and fallback/transit stage; and
- model size.

Use a fixed runner image/machine class, fixed clocks/seeds/budgets, explicit thread/core policy, and recorded toolchain/backend versions. Keep raw results as artifacts and publish the reviewed baseline/summary. A solver with fast search but slow compilation is not presented as universally faster.

Performance gates require:

- no regression beyond the reviewed threshold in compile or verify time on stable small/medium fixtures;
- no loss of verified feasibility on any fixture within its established budget;
- no authoritative-score regression on deterministic fixed-budget profiles beyond reviewed tolerance;
- no material unexplained model-size explosion after compiler/rule changes;
- no UI long-task regression beyond defined interaction budgets;
- no unbounded worker logs or memory growth.

Threshold values/tolerances and interaction budgets must be defined from Phase-12 baseline evidence, committed with rationale, and applied consistently; an undefined “reviewed percentage” cannot authorize release.

### Initial target calibration

Begin with the cross-cutting hypotheses rather than inventing a different release metric: <500 ms end to end for approved small warm fixtures; target <1 second warm and usually <3 seconds cold for approved typical fixtures; p95 <5 seconds over the defined normal corpus; <5 seconds for the large majority of moderately complex cases and <10 seconds expected ceiling; and bounded, accurately labelled outcomes for stress/pathological cases. The initial Balanced profile tests approximately 2–3 seconds of CP-SAT time inside a 3–5 second end-to-end interactive budget.

Calibrate on an exact ordinary consumer machine class approximating 4–8 CPU cores and 16 GiB RAM, with no dedicated GPU requirement, then record the concrete machine. A target becomes a release gate only after the corpus, cache state, operation boundary, sample count, percentile method, variance policy, power mode, and regression tolerance are committed. If evidence requires changing a target, record the measured reason and update the cross-cutting document, product copy, and baseline together; never silently redefine the denominator or publish a universal hardware promise.

Map the corpus to four reviewed classes: Pack A small; Pack B common workforce/household/group-equivalent load; Pack C moderately complex with significant preferences/transportation dependencies where applicable; and Pack D difficult/stress. Transportation Phase 14 adds manual, warm-snapshot, cold-enrichment, traffic-bucket, and transit-fallback variants after MVP without changing the public-MVP corpus retroactively.

Performance acceptance also requires no webview freeze or unbounded long task; progress appears only after approximately 300–500 ms and only from real phases; cancellation/focus/screen-reader behavior remains responsive; raw incumbents are never announced as valid; and an accepted result is not withheld by optimality proof, optional explanation, or AI provider work.

### 6. CI workflow and required checks

The workflow set is:

```text
.github/workflows/
├── pr.yml
├── portable.yml
├── desktop-e2e.yml
├── security.yml
├── benchmark.yml
├── fuzz.yml
├── release.yml
└── dependency-update.yml
```

#### `pr.yml` — canonical Linux/Nix suite

Use SHA-pinned checkout and Nix actions, the public binary cache, `nix flake check`, `nix develop --command just install`, `nix develop --command just check`, a Nix OR-Tools worker build/smoke, and sanitized failure/coverage artifacts. It mirrors the local shell and is the canonical full suite.

#### `portable.yml` — native matrix

Run core tests and supported platform packaging smokes on `ubuntu-latest`, a supported macOS x86_64 runner (subject to the recorded gate), an available macOS arm64 runner, and `windows-latest`, using pinned Rust, Node/pnpm, native prerequisites, and native workers.

#### `desktop-e2e.yml`

Run packaged Tauri smoke/E2E on all supported Windows, Linux, and macOS targets; Linux uses a virtual display when required. Embedded/external driver limitations are recorded rather than silently reducing coverage.

#### `security.yml`

Run `cargo deny check`, `cargo audit`, pnpm lock/license/advisory checks, CodeQL or equivalent analysis, secret scanning, `reuse lint` if adopted, exact-input SBOM smoke, and a reviewable Tauri capability/CSP diff artifact.

#### `benchmark.yml`

Run on main, compiler/solver/rule/router-sensitive changes, release candidates, and a schedule. Use fixed seeds/budgets and retain raw/baseline deltas. Expensive benchmarks do not block unrelated documentation-only changes.

#### `fuzz.yml`

Run scheduled short fuzz jobs and sanitizer builds; longer release campaigns may use trusted dedicated/donated runners.

#### Required pull-request checks

- Nix structural checks;
- formatting;
- clippy with warnings denied;
- frontend lint and typecheck;
- generated DTO/bindings/protobuf/license outputs clean;
- Rust and frontend tests;
- solver-worker contract tests;
- license policy;
- supported-platform core builds;
- security-sensitive code-owner review where applicable.

All actions are pinned by immutable SHA. Tokens default read-only. Release permissions exist only in protected environments. Signing secrets are unavailable to pull requests and untrusted forks. Build/sign jobs are separated and exchange digest-addressed artifacts; the signer verifies the digest. Provenance and SBOMs describe the final exact assembly. Untrusted PRs cannot run arbitrary scenario/benchmark scripts in privileged contexts.

### 7. Security, privacy, accessibility, and packaging review

The release review replays the Phase-11 threat model: malicious/corrupt bundles; oversized CSV/JSON/images; malformed/compromised worker; prompt injection; malicious endpoints; log-secret leakage; supply-chain compromise; unsafe plugins; path traversal/symlinks; stale/forged updater metadata; and compliance overclaiming.

Evidence includes minimum Tauri capabilities and restrictive CSP, strict parser/archive/image/resource limits, worker manifest/hash/location/protocol checks, credential isolation, structural log/support-bundle redaction, no required telemetry, signed update/channel behavior, exact-artifact dependency/license review, and non-certification language.

Accessibility evidence includes automated `axe-core`, accessible component/unit behavior, packaged E2E, keyboard-only and screen-reader manual QA, reduced motion, zoom/high-DPI, non-color cues, synchronized accessible alternatives for canvas, and agreed severity criteria. The severity policy and any accepted non-blocking finding are documented; an undefined “agreed severity” is not sufficient.

### 8. Release-candidate feedback and experimental isolation

- Publish candidate limitations and supported paths to testers; collect issues without default telemetry.
- Triage every report into correctness, data integrity, security/privacy, accessibility, packaging, licensing, performance, usability, documentation, or non-blocking enhancement.
- Reproduce against the candidate when practical; root-cause and retest any fix. A changed candidate repeats affected gates.
- Experimental Pumpkin routing remains explicit/advanced and capability-gated. If cancellation, support matrix, performance, packaging, or messaging is inadequate, disable it in public artifacts without weakening OR-Tools or verifier behavior.
- AI remains optional and provider adapters can be individually excluded if conformance/maintenance quality is insufficient. Removing an adapter does not remove deterministic workflows.
- Nightly/development features must not leak into stable configuration, docs, updater metadata, or schema dependencies.

## Definitions of done

These definitions apply cumulatively; meeting the release gate cannot waive a feature-level definition.

### Core feature

A core feature is done only when its public/internal contract is documented; errors are typed/actionable; unit/property tests cover boundaries; serialization/version effects are handled; cancellation/resource limits are considered; logs are structured/redacted; Nix and native builds pass; and relevant documentation/ADRs are updated.

### Domain rule

A domain rule is done only when all 13 rule-implementation items are complete:

1. stable rule ID/schema and migration impact;
2. command DTO and validation;
3. plain-language UI/editor;
4. fast and full domain validation;
5. Planning-IR compilation;
6. backend capability and translation tests;
7. independent-verifier evaluation;
8. provenance and deterministic explanation;
9. AI tool/schema exposure when appropriate;
10. a CLI/document-format example;
11. fixtures including edge and infeasible cases;
12. user documentation and limitations;
13. benchmark and model-size impact review.

In addition, required/preference semantics must be unambiguous; empty/default scopes must be safe; compiler and verifier must agree on exhaustive small fixtures; infeasible and boundary examples must exist; result/explanation UI must be human-readable; import/export round trips must preserve the rule; AI must not be able to create an invalid shape; and model-size impact must be measured.

### Backend

A backend is done only when license/distribution are approved; exact version is pinned and visible; capability matrix is implemented; every claimed Planning-IR primitive has contract tests; status/cancellation/timeouts are normalized; candidate projection and independent verification pass; crash/malformed-output handling passes; packaging/target smokes pass; benchmark comparison is documented; and user-visible stability labels are accurate.

### Desktop flow

A desktop flow is done only when normal, empty, loading, stale, error, and offline states are designed; the keyboard path works; labels/focus/announcements pass automated and manual checks; transient/optimistic state cannot corrupt Rust authority; revision conflicts are handled; undo/redo is defined; a large fixture is profiled; representative users complete the task; and screenshots/docs match the candidate.

### Portable data and restore flow

A portable-data flow is done only when internal storage is not the interchange schema; current export is canonical; every supported historical migration is permanent and sequential; IDs/references/units/extensions obey the public contract; bounds/checksums/archive hazards and unknown semantics fail before mutation; preview exactly describes migrations, collisions, reconnections, inclusions/exclusions and destination; Create copy/Replace/Skip plus add/replace restore are unambiguous; destructive replace attempts and truthfully reports a pre-restore safety backup; commit is atomic; cancellation/failure preserves source and authoritative data; and desktop/CLI/fresh-install fixtures agree.

### Standalone result flow

A standalone result flow is done only when one accepted immutable result builds one versioned privacy-filtered Share Result payload; preview and output fields agree exactly; HTML opens as one `file://` file with no required network/server/account/storage; user strings remain inert under the report CSP; key interactions are keyboard/screen-reader operable and every visual has a list/table equivalent; print/PDF reuse the same payload; generated output retains status/provenance/version/privacy metadata and cannot change with the source; output staging is atomic/cancellable; and supported-browser, malicious-data, large-report, offline, and no-network evidence pass.

### AI capability

An AI capability is done only when a deterministic non-AI equivalent exists; the tool is typed/allowlisted; read/write risk is classified; writes produce a diff preview and require explicit apply; stale revision behavior and prompt-injection/malformed-call tests pass; secret/data scope is documented; provider failure preserves state; deterministic evidence is inspectable; and a fake provider covers it in CI.

For Phase 10, exercise both workforce and event seating with synthetic, versioned conversations and strict expected command/evidence outcomes. Include ambiguous identities, Required-versus-Preference interpretation, stale/partial proposals, duplicate or delayed tool events, and a real evidence citation paired with an unsupported claim. Switching from local-only to an external profile must not upload prior private messages, summaries, cached tool results, or retained context without a newly approved disclosure. Default conversations are ephemeral; explicit retention and deletion leave applied commands and minimal durable provenance intact. Native credential canaries and keyring-unavailable recovery preserve the same no-secret-in-Vue boundary.

Manual acceptance checks that users distinguish a draft proposal, applied scenario, and accepted result; can correct a wrong interpretation; and understand scope, provider destination, usage uncertainty, and unresolved search. Schema validity or a plausible explanation alone is not evidence of useful or truthful assistance. Record correction effort and observed confusion without introducing telemetry.

## Public-MVP release gate

Every item is mandatory unless the affected target/feature is explicitly removed and all docs/manifests are updated.

### Correctness

- All official workforce and seating fixtures pass.
- Every accepted candidate is independently verified.
- Exhaustive/differential small-model and compiler-mutation suites pass.
- No known release-blocking compiler/verifier discrepancy remains.
- Feasible, optimal, infeasible, unknown, bounds, time-limit, and sufficient-conflict language is accurate.

### Data integrity

- Database, scenario-envelope, domain, settings, Portable Scenario, Result, and Share Result migrations pass every permanent historical fixture.
- Current export is canonical/current-only; historical import migrates sequentially; unknown required semantics/newer versions reject before mutation while declared nonsemantic extensions preserve.
- Scenario export/import and full-backup creation plus add/replace restore pass semantic round trip, stable identity/reference/unit, collision/reconnection, cross-platform, CLI/desktop, fresh-install, attempted safety-backup, cancellation, interruption, and recovery fixtures.
- Malformed, checksum-invalid, traversal/symlink/duplicate/path-conflicting/decompression-hostile or over-limit bundles cannot partially mutate authoritative data or leave trusted staging.
- Bundles contain no SQLite/database file, credential, device-specific path, disposable cache, unauthorized provider content, AI conversation/provider payload, or undisclosed excluded data.

### UX and accessibility

- Every first-time usability task passes with representative new users; repeated confusion is resolved and retested.
- Keyboard workflows pass.
- Automated and manual accessibility checks pass the documented severity policy.
- Large workforce and seating views meet recorded responsiveness/memory budgets.
- Calibrated small/typical/normal-corpus solve targets pass on the recorded reference machine, stress cases remain bounded, and raw artifacts prove each end-to-end phase rather than solver time alone.
- Progress threshold, truthful phases, cancellation/focus/announcement behavior, first-verified-feasible delivery, and optional-explanation/AI isolation pass on packaged targets.
- One-file HTML and direct PDF pass exact privacy-preview, accepted-result provenance, immutable-snapshot, `file://` offline/no-network, safe interaction, keyboard/screen-reader/list-table, print/grayscale/page-context, supported-browser, cancellation, and large-report responsiveness gates.
- Representative users distinguish editable export, full backup, and immutable result sharing; they can complete privacy review and add/replace recovery without unsafe assumptions.
- AI is optional, visibly bounded, preview/apply controlled, and all deterministic paths work without it.

### Security and privacy

- Threat model is reviewed against exact artifacts.
- Tauri capabilities, application CSP, standalone-report CSP, safe inert-data rendering, archive/report parser limits, output staging, and Rust-only network path are reviewed.
- Secrets and excluded/source-only data remain outside webview, logs, database, portable bundles, HTML/PDF/other exports, diagnostics, child environments, and support bundles.
- Dependency/advisory/secret/static scans pass or every non-blocking exception has owner, scope, rationale, expiration/review point, and maintainer approval.
- Signed update/channel/key flows reject invalid metadata/packages and pass clean-machine tests.
- Support bundle is structurally redacted, bounded, user-created, previewable, and manifest-consistent.
- No required telemetry exists.

### Packaging

- Every declared target installs and launches on a clean supported OS.
- The exact bundled worker validates its manifest/hash/target, handshakes, solves, cancels, and fails safely.
- No external runtime, toolchain, language, or solver installation is required.
- Update, migration backup, uninstall, data retention/deletion, restore, and offline behavior pass.
- The final portable extension/media types/file associations open through bounded inspect/preview rather than direct mutation, and all registered cross-platform behavior matches the identity ADR.
- Platform signing/notarization/timestamp/checksum/attestation evidence verifies for exact digests.

### Open-source compliance

- Apache-2.0, NOTICE, DCO, contribution, governance, security, code-of-conduct, trademark, and chosen SPDX/REUSE files are complete.
- Exact-artifact third-party notices and SBOMs are generated and reviewed.
- No blocked solver/dependency is linked or bundled; OR-Tools build excludes GLPK/proprietary integrations and unwanted components according to the pinned build evidence.
- Source tag/archive, checksums, signatures/attestations, provenance, migration notes, release notes, and license evidence are ready to publish together.

### Documentation

- User quick start;
- workforce guide;
- seating guide;
- exhaustive rule semantics, examples, required/preference behavior, limitations, time/geometry/fairness interpretation, and non-certification;
- final CLI reference and stable exit/error behavior;
- developer guide for Nix and native Windows setup;
- architecture and approved ADRs;
- security, privacy, AI, provider, credential, updater, support-bundle, standalone-report CSP/no-network, and exact share-preview guidance;
- Portable Scenario/Result/Share Result compatibility matrices; editable export versus backup versus sharing; migration/collision/reconnection; add/replace restore, safety backup, recovery, data deletion/retention, update, uninstall, and offline instructions;
- final extension/media types/file associations, supported OS/target/browser matrix, artifact/checksum/signature verification, stability labels, and known limitations.
- named-profile setup, capability/billing limitations, native credential recovery, context/profile-switch disclosure, opt-in conversation retention/deletion, separately confirmed solve actions, and workforce/seating proposal walkthroughs match the implemented Phase-10 contract.

All documentation walkthroughs must match the exact candidate.

### Quality, performance, and release operations

- All required PR/main/release CI workflows pass with immutable action pins and protected signing boundaries.
- Coverage thresholds pass or only documented generated/platform-only exceptions remain.
- Scheduled/release fuzz campaigns and sanitizer builds have no untriaged crash, hang, or uncontrolled resource growth.
- Fixed-runner benchmark baselines and raw evidence are published; no unapproved feasibility, score, model-size, compile/verify, UI-latency, memory, or log-growth regression remains.
- Manual platform/accessibility/security QA passes.
- Every release-candidate report is triaged and every release-blocking issue is closed.
- Experimental features are isolated, accurately labeled, or removed.

## Ordered work packages

1. **QA-FREEZE — freeze candidate and evidence index.** Record source, locks, toolchains, flags, manifests, digests, target matrix, expected tests, docs, and prior-beta migration corpus.
2. **QA-CORRECT — run semantic correctness layers.** Unit/property/domain/golden/backend/differential/mutation/metamorphic suites; investigate every discrepancy at the compiler/verifier/domain cause.
3. **QA-DATA — run persistence/portable/migration/import/recovery gates.** Include permanent database and Portable Scenario/Result/Share Result fixtures, current-only export, semantic round trips, malicious/oversize archives, unknown semantics/extensions, collisions/reconnections, add/replace and fresh-install restore, safety backup, interruption/cancellation, no-forbidden-content checks, and atomic recovery.
4. **QA-PROCESS — run protocol/worker/fuzz/sanitizer gates.** Exercise malformed worker frames, ZIP manifests/paths/checksums/decompression, portable/share decoders and migrations, cancellation, crashes, limits, manifests, packaged workers, and regression corpus.
5. **QA-UI — run frontend, report-browser, and packaged E2E.** Cover required workflows, distinct editable/backup/share intents, exact privacy preview, import/restore review/recovery, HTML `file://` no-network/inert-data/immutability, PDF/print, stale/conflict/error/offline states, large views/reports, support bundle, fake AI, and exact artifacts.
6. **QA-RESEARCH — conduct usability studies.** Capture and resolve every required task's repeated confusion, then retest affected flows with fresh users.
7. **QA-A11Y — complete accessibility audit.** Automated, keyboard, screen-reader, zoom/high-DPI, reduced-motion, non-color, canvas-alternative, and severity review.
8. **QA-PERF — establish and enforce baseline.** Lock reference machine/runner, Pack A–D manifests, warm/cold/cache preconditions, budgets, sample/percentile method and tolerances; run the corpus; inspect raw end-to-end phase/model/cache/quality metrics; publish baseline and deltas; calibrate Quick/Balanced/Deep and supported envelopes without overstating guarantees.
9. **QA-SECLEGAL — review threat model and exact artifacts.** Capabilities/application-and-report CSPs, inert rendering, archive/report parsers, portable/share privacy and prohibited content, secrets/support/updater, scans, notices/SBOM/licenses/assets, no blocked dependencies, and security reporting readiness.
10. **QA-PLATFORM — complete clean-machine manual QA.** Installer/signing/update/uninstall/offline/credential-store/platform display behavior for every declared target.
11. **QA-RC — triage candidate feedback and isolate experiments.** Fix root causes, regenerate candidates, and repeat affected evidence until no blocker remains.
12. **MVP-APPROVE — maintainer release review.** Check every definition/gate line, authorize protected Phase-11 publication, and verify downloaded public artifacts after publication.

## Risks and mitigations

| Risk | Consequence | Required mitigation |
|---|---|---|
| Prematurely generic domain system | Slow progress, unusable UI | Public MVP keeps compiled official packs and stable internal traits, not a marketplace. |
| OR-Tools build/packaging complexity | Broken installers/contributor friction | Pinned worker boundary, Nix/native builds, exact manifest, sidecar and clean-machine smoke. |
| Compiler bug | Invalid/missing assignments | Independent verifier plus exhaustive differential and mutation tests. |
| Verifier shares compiler error | False confidence | Separate rule evaluators, limited shared utilities, mutation evidence. |
| Solver returns feasible but not optimal | User assumes “best possible” | Accurate normalized status, bound and time-limit wording. |
| Infeasibility core is large/nonminimal | Misleading explanation | Bounded shrinking and “sufficient,” never “minimal” without proof. |
| Scheduling rule/model explosion | Slow/OOM solve | Candidate filtering, conflict graphs/global constraints, model summaries and benchmarks. |
| Seating pair explosion | Excessive constraints | Relevant-pair precomputation, table variables, symmetry breaking, warnings. |
| DST/time semantics error | Unsafe schedule | Explicit IANA zone/policy, instant calculations, boundary fixtures. |
| Fairness underspecified | Results feel wrong | Explicit target/peer-group configuration and raw distributions. |
| Frontend owns domain state | Divergence/data loss | Rust authority, revisioned commands, generated API, transient-only stores. |
| UI complexity | Product failure | Progressive disclosure, guided flows, plain language, usability gate. |
| Canvas excludes users | Accessibility failure | Synchronized keyboard/screen-reader list/editor and non-color cues. |
| AI hallucination/injection | Bad config/data leak | Typed allowlist, preview/apply, deterministic evidence, Rust network boundary. |
| Provider churn | Broken optional AI | Current official contracts, recorded conformance fixtures, per-adapter isolation, AI optional. |
| Secret leakage | Account compromise | OS keyring, no webview secrets, structural redaction, redirect and endpoint controls. |
| Malicious project bundle | Traversal/exhaustion/data mutation | Strict manifest/checksums, limits, safe staging, transactional import. |
| Portable migration/restore silently loses meaning | Irrecoverable trust/data loss | Permanent historical semantic fixtures, strict unknown-semantic rejection, exact preview, staged validation, atomic commit, safety backup and fresh-install recovery. |
| Standalone report leaks or executes scenario data | Privacy or code-execution failure | Exact preview payload, purpose-built Share Result, inert encoding/rendering, restrictive report CSP, zero-required-network and malicious-data browser fixtures. |
| Untrusted plug-in | Code execution | No native dynamic MVP plug-ins; future no-ambient-authority WASM only. |
| License contamination | Distribution restriction | Allow/review/block policy, exact notices/SBOM, minimal OR-Tools build. |
| Proprietary solver redistribution | Legal violation | User-provided only; never bundled. |
| Cross-platform drift | Broken OS | Native matrix, packaged E2E, platform ownership, clean-machine QA. |
| Nix-only assumptions | Windows friction | Canonical Nix plus native Windows docs and CI. |
| Update/migration loses projects | Severe trust failure | Pre-migration backups, transactions, version fixtures, downgrade guard, recovery test. |
| Presets imply legal compliance | Operational/legal harm | Explicit non-authoritative disclaimers and no certified claims. |
| Maintainer overload | Stalled release/project | Modular ownership, conformance suites, scoped gate, governance. |

### Architectural stop conditions

Pause and write an ADR before proceeding if a domain rule cannot be independently verified; a backend needs domain-specific knowledge; UI requires direct database access; AI requires arbitrary code/file/shell access; a plug-in requires native in-process loading; a dependency license falls outside policy; a migration cannot preserve scenarios; decomposition cannot prove independence; packaging needs a major security control disabled; or correctness depends on undocumented provider/backend behavior.

## Failure handling

- A failing semantic test is investigated as a possible contract/compiler/verifier defect; do not suppress, weaken, or mark flaky without root-cause evidence.
- A candidate change produces a new digest and invalidates all affected signing, SBOM, packaging, benchmark, migration, docs, and platform evidence.
- A platform/feature unable to pass is removed from the supported matrix/build/docs or release is delayed; unsupported fallback behavior is forbidden.
- A security/license exception requires explicit evidence, owner, impact, expiry/review point, and maintainer approval; blockers in exact artifacts cannot be waived casually.
- Benchmark noise triggers reruns on the same controlled environment and statistical review; no ad hoc threshold moving after seeing results.
- User research findings are resolved in the product or text and retested, not explained away as user error.
- Published-artifact smoke failure pauses updater/release promotion, removes/withholds affected metadata where safe, and follows documented security/recovery communication.

## Exit gate

Phase 12 and public MVP are complete only when every definition of done and every gate above is evidenced against identical artifact digests; every Appendix K.8 item is closed; no release-blocking issue remains in correctness, portable-data integrity, security/privacy, accessibility, packaging, licensing, performance, usability, migration/backup/restore/report recovery, or documentation; final extension/media-type/file-association and supported-browser decisions are recorded; all experimental paths are isolated/accurately labeled; authorized maintainers approve publication; and post-publication downloaded-artifact smokes verify the public source tag, checksums, signatures/attestations, updater metadata, notices, SBOMs, migration notes, release notes, portable import/restore, and offline HTML/PDF result output.

## Deferred and non-goals

- This phase does not add school timetabling, new backends, sandboxed packs, collaboration/server mode, hosted services, telemetry, or new release architectures to make the release appear broader.
- It does not chase arbitrary global coverage percentages beyond the specified risk-weighted gates.
- It does not claim universal solve-time guarantees or bit-for-bit reproducibility for signed/notarized bundles.
- It does not use live paid provider calls as deterministic CI evidence.
- It does not convert experimental Pumpkin into a stable default without its support/cancellation/benchmark gates.
- It does not publish merely because a date/tag was planned; evidence controls publication.
- Conversational experiments, voice, MCP servers, delegated reasoning, and conditional subscription-runtime support have their own post-MVP Phase-13 gates. Audio/remote-host evaluations do not become public-MVP prerequisites. When those capabilities ship later, apply the same exact-artifact, security, privacy, accessibility, recovery, and evidence discipline to their supported modes; school and transportation are not prerequisites for the MVP assistant corpus.
- [Collaborative Planning and Hosted Service](collaborative-planning-and-hosted-service.md#status-authority-and-scope) is Branch-H workflow authority, not another numbered phase or an MVP gate. H0 read-only reuse/pilot-charter work may inform planning without production expansion; H1 enters only after Phase 12 and its relevant ADR/operating gates. Managed collaboration operations use the same open capabilities, with applicable security controls inherited by each deployment rather than the entire intake-pilot checklist. Existing school, voice, and transportation gates remain independent; eligibility does not imply simultaneous staffing or priority.

## Assumption and version gates

- Use the direct-registry/API values in [the assumptions ledger](assumptions.md), resolving report conflicts in favor of that evidence.
- Rust release builds use 1.97.1 until a newer stable fixes the 1.98.0 P-critical issue and passes all target suites. Fuzzing uses a separately pinned compatible nightly.
- Node 24.20.0 LTS, pnpm 11.24.0, TypeScript 6.0.3, ESLint 10.9.1, Vitest 4.1.11, Vue Test Utils 2.5.0, Testing Library Vue 8.1.0, `axe-core` 4.13.0, WebDriverIO/`@wdio/cli` 9.31.4, `@wdio/tauri-service` 1.3.0, and Playwright 1.62.1 are the current verified test/tool values; exact lock pins are Phase-0 records.
- TypeScript remains below 6.1 because `typescript-eslint` 8.68.0 excludes `>=6.1`; there is no nonexistent `@axe-core/vue` dependency.
- OR-Tools 9.15 cannot pass the backend/release gate until K.3 closes, including platform builds, CMake flags, source/proto hashes, linkage, dependency loading, SBOM/license manifest, benchmark, and assumption-core behavior. Its known presolve issue prevents unqualified core claims.
- Pumpkin 0.5.0 stays experimental until actual API support matrix, dedicated-thread/cooperative-cancellation/time-limit behavior, contract/verifier suites, and fixed-budget benchmarks pass.
- K.8 explicitly requires exact lockfile/compiled-artifact license verification; proof that no GPL/proprietary solver is linked/bundled; completed manual accessibility audit; clean-machine installer/update/uninstall evidence; and publication of source, notices, SBOM, checksums, signatures/attestations, and migration notes.
- Signing, updater endpoint/key lifecycle, supported OS minimums, WebView2 mode, Wayland/X11, artifact mix, macOS x86_64 support, final CLI/app IDs, hosting organization, governance contacts, and release-signing choices must be resolved before approval.
