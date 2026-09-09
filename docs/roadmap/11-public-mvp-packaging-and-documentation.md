# Phase 11 — Public-MVP Packaging and Documentation

## Outcome

Produce reviewable public-MVP release candidates for `eutheto` that install, launch, solve, export, update, and operate offline on every supported target without requiring users to install Rust, Node.js, pnpm, Nix, Python, C++, Java, OR-Tools, or another solver. Every candidate is signed where the platform provides code signing, carries exact license and supply-chain evidence, preserves user data, exposes a redacted/previewable support-bundle flow, and is accompanied by complete user, administrator, contributor, architecture, security, privacy, recovery, CLI, rule, and limitations documentation.

This phase prepares artifacts and documentation. [Phase 12](12-stabilization-and-public-release-gate.md) decides whether those artifacts may become the public release. Version and decision evidence is maintained in [the assumptions ledger](assumptions.md).

This is the local desktop/CLI release-candidate gate, not a collaboration-service launch gate. Core planning and accepted-result output remain offline and account-free, with no hidden network dependency; optional provider and updater networking retains its existing explicit boundaries. Local selection or export is not organization approval or publication. Later collaboration and hosted releases follow the separate [delivery and ownership gates](collaborative-planning-and-hosted-service.md#delivery-and-ownership), supplying applicable identity/authorization, privacy/retention/recovery, deployment, and operations evidence for their actual supported modes rather than adding those prerequisites to this MVP.

## Source coverage

This phase is the implementation source of truth for blueprint Sections 24 and 27–28; the release, security, quality, and documentation portions of Section 26; Phase 11; the release/documentation slices of Appendices B, H, I, J, and K; the public-MVP contents in Section 6.3; candidate documentation/evidence for [Performance and Solver UX Targets](performance-and-solver-ux-targets.md); and public format/identity/documentation closure for [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md). It owns backlog items `SEC-001`, `REL-001`, `REL-002`, and `DOC-001`, and supplies the exact-artifact inputs to `QA-001` and `MVP-001`. It does not take full ownership of Appendices B, H, or L: the complete dependency baseline/toolchain still belongs to Phase 00, complete standards to [README.md](README.md), and the dependency graph to the roadmap index.

## Dependencies

- Phases 0–10 have produced the authoritative Rust core, desktop application, working CLI, schema migrations, official workforce and seating packs, OR-Tools worker, optional experimental Pumpkin path, exports, explanation/repair flows, and optional AI adapters.
- All serialized formats and protocols have explicit versions and compatibility policies; unknown-newer versions fail safely.
- Every accepted solver candidate passes the independent verifier.
- Release automation, capabilities, worker manifests, license aggregation, and documentation are implemented from the same source commit and lockfiles.
- Phase 12 consumes immutable artifact digests from this phase; it does not rebuild a different candidate and call it equivalent.
- The user has accepted the Phase-10 AI trust/privacy and AI-disabled checkpoint and explicitly authorized Phase-11 implementation under the [manual checkpoint policy](README.md#user-manual-checkpoint-gates).

## Decisions and invariants

### Product and release identity

- The project name is final: `eutheto`.
- Apache-2.0 and Developer Certificate of Origin sign-off are approved and must not be reopened by this phase.
- The working CLI name `optimizer` remains provisional. `.eutheto` is the current portable-file extension proposal, not a public decision. The final CLI name, crate namespace/prefix, reverse-domain application ID, portable extension/media type/file association, Git hosting organization, governance/security contacts, stable/beta application identifiers, updater endpoint, and signing/key-custody choices are explicit release gates in [the assumptions ledger](assumptions.md); no package may silently invent them.
- Overall releases use semantic versioning: patch for compatible bug/security fixes, minor for backward-compatible features/rules/backends/packs, and major for incompatible public CLI/API/document changes.
- The application release manifest records the application, core API, scenario envelope, every domain schema, planning IR, worker protocol, OR-Tools, backend-adapter, AI policy/tool-catalog, benchmark-corpus, performance-policy, and report/portable-format versions together even where they are independently versioned.
- Channels are stable, beta, and nightly/development. Stable is signed and fully tested; beta is explicit opt-in with schema compatibility tested; nightly artifacts are not automatically trusted for production. Stable never moves to beta without explicit user action.

### Trust boundaries and data authority

Treat every boundary as untrusted input:

```text
Vue webview
    ↕ typed Tauri IPC
Rust application/core
    ↕ length-delimited worker protocol
Bundled OR-Tools worker

Rust application/core
    ↕ HTTPS or explicitly configured local endpoint
External AI provider

Rust application/core
    ↕ strict parsers
Imported files, portable bundles, and standalone report data
```

- Rust remains authoritative for scenario data. The webview never receives credentials and never directly performs provider HTTP calls.
- No dynamic native plug-ins ship in the MVP. Official workforce and seating packs are compiled and explicitly registered. Untrusted `.dll`, `.dylib`, and `.so` loading is forbidden.
- Every potentially hostile input is bounded before allocation or execution: project archives, JSON, CSV, images, worker frames, updater metadata, provider responses, URLs, and support-bundle paths.
- A signature establishes artifact identity, not semantic correctness; parsers, protocol validation, independent verification, migration safety, and resource limits still apply.

### Tauri and webview security

- Use minimum permissions per window. There is no general shell permission, no unrestricted filesystem or HTTP access, and no generic sidecar permission.
- Sidecar permission names only the exact bundled solver worker; file access flows through explicit dialogs and scoped paths; clipboard access exists only where a demonstrated flow needs it; updater permissions are isolated to the update UI.
- Production debug/devtools behavior follows the selected platform policy. Generated capability files and `build.rs` command manifests are security-sensitive reviewed source; do not rely on a broad `invoke_handler` default.
- The production CSP permits scripts and styles only from bundled resources, forbids arbitrary remote scripts and `eval`/dynamic code generation unless a dependency-specific review proves necessity, restricts images to bundled/local-converted resources and reviewed blob/data uses, and disables webview network connections by default.
- Render user, scenario, import, provider, and AI content as plain text or restricted Markdown with raw HTML disabled. Sanitize any supported rich text.

### Parsing and process integrity

- Text is strict UTF-8 by default. CSV encoding detection requires explicit review before conversion.
- CSV parsing streams and enforces byte, row, column, and field limits before full materialization. JSON applies centralized byte, nesting, string, collection, and allocation limits. Images apply encoded-size, dimension, pixel-count, and decoded-memory limits.
- Project archives reject absolute paths, parent traversal, unsafe links, duplicate/conflicting entries, excessive files, compression-ratio bombs, and expansion beyond declared budgets. Import is staged and transactional; malformed bundles never partially mutate authoritative data.
- Spreadsheet imports do not execute macros or active content. Templates never execute arbitrary code.
- The application validates the expected solver-manifest location and worker executable location. The manifest includes worker hash, worker version, target triple/architecture, OR-Tools version, license metadata, and worker-protocol version.
- Worker output remains length-delimited, versioned, request-ID matched, size bounded, and independently parsed. Packaged worker hash, executable bit, architecture, handshake, license payload, and forbidden linked dependencies are checked.
- The packaged app never searches `PATH` for its default solver. Future user-provided external solvers use an explicit configuration flow.

### Secrets, logs, diagnostics, and privacy

- Credentials live only in the operating-system credential store. They never enter SQLite, project files, exports, the webview, child-process environments, panic reports, or support bundles.
- After storage, Tauri IPC returns only an opaque credential reference/status, never the secret. Replacement and deletion are explicit. Short-lived in-memory buffers and `zeroize` are best-effort defenses, not absolute guarantees.
- Structured tracing uses redaction wrappers. Production logs may contain lifecycle events, job IDs, normalized status, bounded durations, and version metadata. Diagnostic mode may add model summaries and bounded solver logs.
- Do not default-log complete scenarios, person/guest names, notes, chat content, provider payloads, authorization headers, query keys, or imported rows. Logs rotate and have a storage cap.
- Public MVP has no required telemetry. All features, including offline work, remain functional without telemetry.
- Compliance presets are starting templates, not legal authority. Documentation and in-product text state that users must validate current laws, contracts, accreditation requirements, and organizational policy; AI output is not legal/professional advice; “verified” means the candidate satisfies configured rules, not every possible real-world rule. No preset is called “legally compliant” without a separately maintained jurisdiction-specific review process.

## Exhaustive scope and technical details

### 1. Public-MVP target matrix

| Platform | Architecture | Desktop artifact | CLI artifact | Bundled solver worker | Required release evidence |
|---|---|---|---|---|---|
| Windows | x86_64 | signed installer | ZIP containing executable and notices | target-matched `.exe` | Authenticode/timestamp verification, WebView2 strategy, install/update/uninstall smoke |
| macOS | arm64 | signed and notarized DMG or app bundle | tarball | target-matched signed binary | Developer ID, hardened-runtime review, notarization, stapling, Gatekeeper smoke |
| macOS | x86_64 | signed and notarized DMG or app bundle | tarball | target-matched signed binary | runner/support confirmation plus the same macOS evidence |
| Linux | x86_64 | AppImage and/or deb, selected by clean-machine evidence | tarball | target-matched executable | checksum, signature/attestation, SBOM/provenance, Wayland/X11 and package smoke |

Linux arm64 becomes an MVP target only when CI and end-user clean-machine testing are reliable; otherwise it remains a future target while the development shell supports it. Windows arm64 is post-MVP unless demand plus Tauri/OR-Tools evidence closes its gate. An rpm is optional and depends on clean-machine results. Separate macOS architecture bundles are acceptable; a universal bundle is used only when both application and sidecar packaging are reliable.

Minimum supported OS releases, the exact AppImage/deb/rpm mix, Windows WebView2 Evergreen bootstrap/runtime mode, macOS x86_64 runner availability, and updater endpoint/key lifecycle must be recorded before release.

### 2. One-install bundle contract

Every desktop bundle contains exactly the target-appropriate:

- Tauri application binary and bundled Vue/Vite web assets;
- OR-Tools worker copied/renamed using Tauri's target-triple external-binary convention;
- worker/solver manifest and update public-key configuration;
- required migration resources;
- official workforce/seating domain assets, templates, presets, and example projects;
- Apache-2.0 license, project NOTICE, generated third-party notices, and solver/native license payloads.

Pumpkin and approved native algorithms are linked according to Cargo features and visibly identified by version, stability, capability, and license. The About screen and provisional `optimizer solvers` command expose solver distribution/license metadata. No GPL, AGPL, SSPL/source-available, noncommercial, no-derivatives, proprietary solver binary, or otherwise blocked library may be linked or bundled.

### 3. Release artifact contract

Each candidate/release publishes or stages all of the following for every applicable target:

1. desktop installer/bundle;
2. CLI archive;
3. target-specific bundled worker and its manifest within the exact bundle;
4. cryptographic checksum manifest;
5. detached signatures and/or verifiable attestations;
6. SPDX JSON SBOM for each artifact or platform bundle;
7. third-party notices and license corpus generated from exact contents;
8. source archive and immutable source tag;
9. migration notes, release notes, known limitations, and supported schema/version matrix;
10. provenance statement with source commit, workflow identity, lockfiles/Nix inputs, toolchains, flags, and artifact digest;
11. whole-pipeline benchmark summary whenever compiler, verifier, router, backend, solver build, renderer, or performance-sensitive rule behavior changes, including exact corpus/reference environment, warm/cold state, total budget, first verified feasible, authoritative quality, phase timing, and UI responsiveness rather than solver time alone;
12. signed updater metadata for each enabled channel.

Reproducibility records include exact source commit, `Cargo.lock`, `pnpm-lock.yaml`, `flake.lock`, pinned toolchain files, build flags, artifact digests, and `SOURCE_DATE_EPOCH` where supported. Claims must distinguish reproducible core/CLI/worker builds from platform bundles altered by timestamps, signing, or notarization.

### 4. Signing, notarization, updater, and channel behavior

#### macOS

- Sign the application and bundled worker with Developer ID Application credentials.
- Review hardened-runtime entitlements for child-process execution; no broad entitlement is accepted merely to make packaging work.
- Notarize and staple. Verify on a clean machine with Gatekeeper enabled.

#### Windows

- Authenticode-sign the application, installer, and worker where appropriate and timestamp signatures.
- Protect keys in the selected CI signing service/environment. Test under normal-user permissions.
- Do not assume WebView2 exists on every supported Windows image. Implement the recorded bootstrap/runtime strategy and test offline/managed variants covered by support policy.

#### Linux

- Publish checksums, signatures/attestations, SBOMs, and provenance. If a package repository is later operated, repository metadata/packages also require signing.

#### Updater

- Use Tauri 2 signed updater artifacts (`bundle.createUpdaterArtifacts` in current configuration semantics), never unsigned downloads or arbitrary URLs.
- Verify package signature before install; display version and release notes; allow defer/skip where safe; keep automatic checks configurable; continue to work indefinitely offline.
- Stable and beta metadata/endpoints are separated, and updater metadata is hosted only through a transparent static endpoint or GitHub Releases. Channel changes are explicit.
- Never update during an active solve, export, or migration.
- Before the first launch that performs a database migration, create a recoverable backup. Updates preserve user data.
- The signing public key embedded in the app, private-key custody, rotation/revocation response, endpoint ownership, and stable/beta identifier continuity are written and tested before enabling updates.

### 5. Protected release pipeline

1. Validate version, immutable source tag, changelog/release notes, schema matrix, and working/final CLI/application identities.
2. Run Phase 12's complete correctness, migration, E2E, accessibility, security, benchmark, usability, and license gates against the source candidate.
3. Build unsigned artifacts in isolated target-specific jobs.
4. Generate target manifests and hashes; include the worker digest and exact dependency graph.
5. Transfer by digest to protected signing environments; no pull-request/fork context receives signing credentials.
6. Verify digest, then sign, notarize, staple, and timestamp as applicable.
7. Verify signatures and install exact artifacts on clean runners/machines.
8. Generate final notices/SBOMs against exact assembled bundles, not an approximate build graph.
9. Generate provenance/attestations tied to exact digests.
10. Create a draft release and updater metadata without publishing it.
11. Maintainers review the entire artifact matrix, migration/recovery notes, license deltas, SBOMs, benchmark deltas, supported OS statements, and limitations.
12. Phase 12 authorizes publication; protected automation publishes the release and channel metadata.
13. Perform post-publication clean-machine install/solve/export/update/offline smoke tests using downloaded artifacts and verify published checksums/signatures.

Build and signing are separate jobs. CI actions are pinned by full commit SHA, default tokens are read-only, release rights exist only in protected environments, and artifacts are digest-verified before signing. Provenance and SBOM generation occurs after exact assembly. Pull requests cannot execute arbitrary scenario/benchmark scripts with privileged tokens.

### 6. Dependency, license, asset, and supply-chain contract

#### Project licensing and contribution provenance

Apache-2.0 applies to the Rust core/CLI, desktop application, official domain packs, official solver adapters, project-owned worker code, examples, and documentation unless specifically marked otherwise. Preserve all required dependency notices. Every contribution uses DCO sign-off (`git commit -s`); no CLA is required unless a later publicly reviewed governance/legal decision adopts one.

Required repository legal/governance files are:

```text
LICENSE
NOTICE
THIRD_PARTY_LICENSES/
CONTRIBUTING.md
CODE_OF_CONDUCT.md
SECURITY.md
GOVERNANCE.md
TRADEMARKS.md
DCO.md or a maintained DCO link/reference
```

Use SPDX identifiers in source or REUSE-compatible metadata. If REUSE is adopted, include `LICENSES/Apache-2.0.txt` and metadata for non-commentable files, and make `reuse lint` a gate.

#### Dependency policy

- Default allowlist: Apache-2.0, MIT, BSD-2-Clause, BSD-3-Clause, ISC, Unicode licenses, and similarly permissive licenses after review.
- Mandatory review: MPL-2.0, EPL-2.0, LGPL variants, licenses with attribution/source-offer/linking/data/model/trademark nuances, non-code assets/fonts/datasets, and unknown/custom licenses.
- Block from official linked/bundled artifacts absent an intentional policy change and legal review: GPL, AGPL, SSPL/source-available non-open-source, noncommercial, no-derivatives, and proprietary solver binaries.
- A process boundary is not assumed to eliminate obligations. User-provided external integration still receives specific legal/security review.

Solver distribution classes are: Pumpkin/native algorithms built in after policy gates; OR-Tools shipped as the reviewed worker; HiGHS/SCIP/permissive workers only as separately reviewed post-MVP adapters; Gurobi/CPLEX/Xpress user-provided only; GLPK/GPL engines never officially bundled under current policy. MiniZinc and its bundled solvers are reviewed component-by-component, not as one license unit.

#### Automation and assets

- Lock Cargo and pnpm dependencies; commit `flake.lock`; disallow JavaScript install scripts unless explicitly reviewed.
- Run `cargo-deny` for licenses/bans/sources/advisories, `cargo-audit` as an additional advisory signal, `cargo-about` for Rust notices, a pnpm license inventory, an `xtask` aggregator for Rust/frontend/native worker inventories, and Syft and/or Cargo SBOM tooling.
- Diff generated notices and `THIRD_PARTY_LICENSES`; a human reviews every new/changed license. Automated output is evidence, not authority.
- Review cryptography, parsers, updater, keyring, worker protocol, and Tauri-permission changes with security-sensitive code ownership.
- Prefer system fonts or explicitly permissive distributable fonts. Record source and license for every bundled font, icon, illustration, sample image, template, domain asset, and example dataset. Do not redistribute fonts merely because a developer machine has them.
- User-imported venue images remain user data. Tauri, solver, and provider trademarks/logos remain their owners' marks and do not become eutheto identity.

### 7. Support-bundle and redaction contract

The user explicitly creates a support bundle; the application never uploads it automatically. Before export, show an itemized preview with inclusion state, byte size, sensitivity warning, and destination. The user can remove optional sections.

Default eligible contents are bounded application/version/platform metadata, release/worker manifest, configuration with secrets and personal fields removed, structured lifecycle/status logs, bounded diagnostic solver logs only when diagnostic mode was explicitly enabled, migration history/status without scenario contents, and recent typed error codes. Full scenarios, imported rows, names, notes, AI conversations/provider payloads, credentials, authorization/query tokens, database files, raw crash dumps, and arbitrary local files are excluded by default.

Redaction is structural at production time, not a post-hoc regular expression alone. Export writes to a fresh staging directory, uses safe generated filenames, rejects symlinks/traversal, enforces per-file and total limits, emits a manifest/checksums, and cleans temporary data on success/failure. The preview and final manifest must agree. A regression fixture plants representative secrets and personally identifying fields and proves they do not appear in the archive. Logs rotate and remain storage-bounded even if export is never used.

### 8. Portable data and standalone report release contract

Before candidate creation, freeze and document the MVP Portable Scenario Model, immutable Result Model, privacy-filtered Share Result Model, bundle manifest/checksum layout, current export versions, historical migration matrix, supported import range, and size/security limits from the cross-cutting specification. The release identity ADR closes the proposed `.eutheto` extension, media types, OS file association, icon, and CLI/desktop ownership together; installer associations and public examples cannot precede that decision.

Release evidence proves scenario export/import, full-backup creation, add restore, replace restore with attempted pre-restore safety backup, collision choices, integration reconnection, and fresh-install recovery across every supported target. Current versions export only current canonical portable schemas; every supported historical version migrates sequentially; unknown required semantics or newer versions fail without mutation. Bundles never contain SQLite files, credentials, device-specific paths, disposable caches, or provider content whose terms prohibit redistribution.

One-file HTML and direct PDF are public MVP result outputs. HTML must open from `file://`, require zero server/account/storage and zero network request for core meaning, preserve keyboard/screen-reader/list-table access, render user strings inertly under the restrictive standalone-report policy, print cleanly, and remain an immutable snapshot after source edits. PDF uses the same exact previewed Share Result payload and controlled local renderer. Exact privacy-preview equality, accepted-result provenance, generated-file versions, supported browsers, print/PDF limitations, and sensitive-field defaults are release-visible contracts—not implementation detail.

### 9. Documentation contract

All public claims are checked against the exact candidate. Documentation includes:

#### User and operations documentation

- quick start: install, create/open a project, configure, validate, solve, interpret status, independently verified meaning, export, save, and back up;
- workforce guide and exhaustive rule reference with semantics, required/preference classification, scope/default behavior, units/time-zone/DST limits, examples, infeasible/boundary cases, scoring/fairness interpretation, import/export behavior, and model-size implications;
- seating guide and exhaustive rule/geometry reference with canvas and accessible list workflows, proximity classifications, overlays, keyboard operation, examples, limits, repair, and export;
- locks, repair, comparison, alternatives, explanations, sufficient-versus-minimal conflict wording, counterfactual limitations, and accurate feasible/optimal/unknown status language;
- AI/privacy/provider guide covering named profiles; native-only credential entry and keyring recovery; synthetic opt-in connection tests and capability limitations; exact destination and billing route; local endpoint versus genuinely offline inference; context disclosure, aliases and notes; safe profile switching without transferring earlier private context; opt-in local conversation retention/deletion; typed proposals, command-derived preview/apply/undo and separately confirmed solve actions; usage uncertainty/cancellation; deterministic non-AI equivalents; and AI-disabled operation;
- intent-led portable-data guide that distinguishes **Export editable scenario**, **Back up everything**, and **Share Result**; documents proposed/final extension status, manifest/current-export/historical-import policy, stable identity, exclusions, bounded inspection, migrations, Create copy/Replace/Skip, integration reconnection, add/replace restore, pre-restore safety backup, atomicity, fresh-install recovery, CLI inspection, limits, and failure handling;
- standalone-result guide covering accepted-result provenance, exact privacy preview/defaults, immutable snapshots, one-file offline `file://` HTML, direct PDF/print behavior, accessibility/list alternatives, no-network guarantee, safe external-link behavior, supported browsers, status/version metadata, and limitations;
- installer/update/uninstall/offline behavior by platform, supported OS/architecture matrix, WebView2 requirements, Gatekeeper/SmartScreen guidance, and checksum/signature verification;
- security/privacy statement, no-required-telemetry statement, threat model summary, data locations/retention, support-bundle contents/redaction/preview, security reporting path, and update trust model;
- limitations/non-certification statement: presets are starting points and verified output only proves configured rules;
- example workforce and seating projects with expected status/invariants, license/source, anonymized data, and walkthroughs.
- performance and solver UX guidance explaining provisional versus calibrated expectations, Quick/Balanced/Deep budgets, first verified feasible versus proof, exact status/limit wording, truthful progress/cancellation behavior, reference-machine scope, supported model envelope, and how to read benchmark evidence without treating it as a universal guarantee.

#### CLI documentation

The published CLI reference must reproduce the complete [Phase-01 working CLI contract](01-core-application-shell-and-persistence.md#working-cli-contract) and Appendix C.1–C.7: the final selected command name (with every unresolved example explicitly marked with the working name `optimizer`); all global options; stdout/data versus stderr diagnostics behavior and JSONL progress behavior; the complete command catalog; solve options and the load/migrate → validate → compile/route → solve → project/verify → accepted-result-write sequence; the `eutheto/cli-result/v1` JSON envelope; every stable exit code and shell-signal transformation; and CLI examples. The reference also documents offline behavior, schema compatibility, cancellation, error codes, backend/solver listing, and solver license metadata. Do not publish a provisional binary name as final by accident.

#### Developer/contributor/architecture documentation

- native Nix and Windows setup, exact pinned toolchains, generation/lockfile workflow, worker build, Tauri prerequisites, tests, benchmarks, fuzzing, packaging, signing boundaries, and common failures;
- headless Rust authority, package/crate boundaries, typed Tauri API, persistence/command journal, scenario envelope/database migrations, separate Portable Scenario/Result/Share Result models and migrations, canonical bundle/checksum/import/restore staging contracts, domain-pack and Planning-IR contracts, solver routing/worker protocol, independent verifier, standalone report trust boundary, explanations, optional AI trust boundary, and release flow;
- all approved ADRs under `docs/adr`, each containing context, decision, alternatives, consequences, status, and supersession links:
  `0001-headless-rust-core.md`, `0002-vue-vite-tauri.md`, `0003-ortools-worker-boundary.md`, `0004-domain-and-planning-ir.md`, `0005-independent-verifier.md`, `0006-sqlite-document-persistence.md`, `0007-ai-tool-approval-model.md`, `0008-apache-license-and-dco.md`, and `0009-nix-development-environment.md`;
- public API and non-obvious invariants in code; generated DTOs/protobufs are not hand-edited; schema/protocol changes include compatibility/migration notes; solver changes include benchmark impact; material UI-flow changes include accessibility/usability evidence; dependency additions state purpose and license;
- contribution workflow: issue/design discussion for new domains/backends/major flows, focused DCO-signed PRs, tests/docs/migration/license/benchmark impact, code-owner review for verifier/protocol/security/licensing/migration/solver changes, and changelog attribution.

#### Governance and support documentation

`GOVERNANCE.md` defines maintainers (release authority, roadmap, security response, governance), domain maintainers (semantics/fixtures), solver maintainers (version, capability matrix, benchmarks, packaging), UI/accessibility maintainers (design and accessibility gates), and contributors. Major architecture, licensing, governance, or compatibility changes require a public ADR/RFC review proportionate to impact.

`SECURITY.md` supplies a private reporting channel, supported versions, realistic response expectations, and coordinated disclosure guidance. Security fixes use a private branch/advisory flow and signed artifacts. `TRADEMARKS.md` reserves the eutheto name/logo while permitting truthful compatibility statements and forbidding implied endorsement or materially modified official-name distributions without permission. Hosting organization, governance contacts, and private security contact remain unresolved gates rather than fabricated addresses.

## Ordered work packages

1. **REL-IDENTITY — close release identity gates.** Record final CLI/crate namespace, reverse-domain ID, proposed `.eutheto` decision, media types/file associations/icon, hosting organization, governance/security contacts, stable/beta IDs, supported OS minimums, artifact mix, updater endpoint, key custody/rotation, and platform signing approach in the assumptions ledger, ADR, release configuration, installers, CLI, and docs.
2. **SEC-BOUNDARIES — harden desktop/import/report/worker boundaries.** Review Tauri capability manifests, `build.rs` command allowlisting, app/report CSPs, inert standalone rendering, zero-required-network evidence, Rust-only provider network path, parser/archive/decompression/image limits, worker location/manifest/protocol verification, logging redaction, and no-telemetry default.
3. **SEC-SUPPORT — implement diagnostics and support bundle.** Add bounded structured logging, diagnostic opt-in, itemized preview, structural redaction, safe staging/archive, checksums, cleanup, and planted-secret regression fixtures.
4. **REL-WORKERS — produce target workers.** Build OR-Tools 9.15 only after its platform/build/benchmark/assumption-core gates, match protobuf/protoc to the pinned OR-Tools contract, disable GLPK/wrappers/examples/unrelated/proprietary integrations, inspect dependencies, produce manifest/SBOM/license payload, and test target-triple resolution.
5. **REL-BUNDLES — assemble one-install packages and freeze public data formats.** Embed web assets, worker, manifests, portable/report schemas and migration fixtures, presets, examples, keys, notices, and licenses; register only the decided file association; prove no external runtime or `PATH` solver lookup.
6. **REL-SIGN — establish protected signing.** Separate build/sign jobs, digest transfers, macOS Developer ID/notarization/stapling, Windows Authenticode/timestamping, Linux signatures/attestations, key protection, and verification.
7. **REL-UPDATE — implement stable/beta updater.** Configure signed metadata, channel separation, defer/skip/configurable checks, active-operation exclusion, offline behavior, pre-migration backup, and endpoint/key lifecycle.
8. **LEGAL — complete compliance/governance corpus.** Generate and review exact-artifact notices/SBOMs, apply allow/review/block policy, verify asset/font/data provenance, add legal/governance/security/trademark/DCO files, and expose solver metadata.
9. **DOC-USER — author and fixture-check user materials.** Complete quick start, workforce/seating/rules, status/explanation, solver budgets/performance limitations, portable import/export/full-backup/add-or-replace restore/recovery, Share Result privacy/offline HTML/PDF, AI/privacy, platform, support, examples, and non-certification content.
10. **DOC-DEV — author contributor and architecture materials.** Complete Nix/native Windows, build/test/release, portable/result/share schemas and migration/renderer trust boundaries, contracts, ADRs, governance, security response, CLI, compatibility matrices, and generated-code policies.
11. **REL-DRAFT — stage immutable candidates.** Run protected build/sign/assembly steps, draft complete releases/updater metadata, and hand exact digests plus evidence to Phase 12.

## Tests and acceptance evidence

Phase 11 supplies candidate-level evidence; Phase 12 repeats/expands release-gate review.

- Clean-machine install, launch, create/open, workforce solve, seating solve, independent verification, export, close/reopen, updater, uninstall, and offline-first-launch smoke passes for every declared target.
- Every target's bundled worker architecture, filename/executable bit, manifest hash, version, protocol handshake, license payload, solve, cancellation, and failure handling pass; no runtime/toolchain/solver installation or `PATH` lookup occurs.
- macOS signatures/notarization/stapling/Gatekeeper, Windows Authenticode/timestamp/SmartScreen/WebView2/normal-user behavior, and Linux Wayland/X11/package behavior are recorded for exact artifacts.
- Stable/beta channel isolation, signature rejection, forged/stale metadata rejection, defer/skip/configurable checks, active-solve/export/migration exclusion, offline behavior, and pre-migration backup pass.
- Updates preserve data; uninstall does not silently delete projects; open-data-folder, export-all-projects, delete-local-data, backup/restore, and CLI recovery paths match documentation.
- Exact-artifact license/SBOM checks pass; generated notice diffs are reviewed; no blocked solver/dependency is linked or bundled; every asset/font/example has recorded source/license.
- Support-bundle tests prove itemized preview equals final manifest, planted credentials/personal content are excluded, logs are bounded, archive/path limits hold, and failed exports clean staging data.
- Portable-data fixtures prove canonical current export, semantic current/historical round trip for every official pack, unknown semantic/newer-version rejection without mutation, nonsemantic extension preservation, malicious ZIP/path/decompression/checksum rejection, Create copy/Replace/Skip, cancellation/stale-preview behavior, no SQLite/secrets/restricted caches, full-backup fresh-install restore, add/replace semantics, safety-backup reporting, atomic failure recovery, and cross-platform/CLI-desktop compatibility.
- Standalone-report fixtures prove accepted-result and exact privacy-preview gates, default sensitive-field exclusion, inert malicious text, immutable source snapshots, one-file `file://` operation with zero required network requests, supported-browser/keyboard/screen-reader/list-table behavior, deterministic schematic/data agreement, print/PDF provenance/readability, atomic cleanup, and large-report responsiveness.
- Security review proves minimum capabilities, restrictive CSP, Rust-only provider network path, safe rendering, strict parser limits, worker integrity validation, credential non-disclosure, structured redaction, and no required telemetry.
- Documentation walkthroughs are executed against the candidate: quick start, workforce, seating, required/preference distinction, infeasibility wording, lock/repair, export, offline, update, backup/recovery, checksum/signature verification, support bundle, and AI-disabled operation.
- Packaged text-AI walkthroughs cover a tested API profile and compatible local profile using synthetic data, native credential entry/replace/delete and unavailable-store recovery, workforce and seating clarification/proposal/apply/undo, stale revisions, profile-switch privacy, conversation-retention choice, and failure without damage to the scenario. Recorded/fake-provider paths are reproducible without paid inference; any live conformance evidence is separately opt-in and budgeted.
- candidate benchmark summaries identify exact corpus/build/reference environment, warm/cold state, full operation boundary and provisional status; public copy never turns a backend microbenchmark or one best run into a universal latency guarantee.
- Migrations and backups from every previous beta build fixture pass.
- Public text accurately states supported targets, experimental features, feasible/optimal/unknown meaning, explanation limits, privacy boundaries, and non-certification.

## Risks and failure handling

| Failure | Required response |
|---|---|
| Target worker cannot build/package reliably | Do not claim the target; fix the worker/package path or explicitly remove the target before publication. Never fall back to a `PATH` solver. |
| Platform packaging requires disabling a major security control | Stop and write an ADR; the feature/target does not proceed under an undocumented exemption. |
| Signature, notarization, timestamp, updater, or key-custody flow is incomplete | Candidate remains unpublished; do not ship unsigned “temporary” stable artifacts. |
| Worker differs from tested digest | Reject and rebuild from the approved source; never sign it. |
| WebView2/AppImage/macOS runner assumption fails | Revisit bootstrap/artifact/architecture support and documentation; clean-machine evidence controls. |
| License is outside policy or artifact inventory is uncertain | Stop bundling, obtain review, or remove the component; process separation is not an automatic exemption. |
| Support bundle leaks sensitive data or preview disagrees | Disable export and block release until structural redaction and fixtures pass. |
| Update/migration can lose projects | Block updater/release; preserve transactional migration, backup, rollback/recovery behavior. |
| Bundle migration/restore loses meaning or mutates on failure | Block candidate; preserve source/backup, fix authoritative schema/migration/staging code, and rerun permanent historical/malicious/fresh-restore fixtures. |
| Share preview differs from output, sensitive fields leak, or report needs network | Disable result export and block candidate until exact payload, inert rendering, privacy and offline browser evidence pass. |
| Documentation contradicts behavior | Treat as release-blocking, correct docs or implementation, and repeat walkthrough. |
| Users could read “verified” as legal/optimal certainty | Correct UI/docs before release; never weaken the configured-rules/non-certification and feasible/optimal language. |
| Maintainer/security contact is not staffed | Do not fabricate it; resolve governance/contact gate before public reporting instructions are published. |

## Exit gate

Phase 11 exits only when:

- the user has tested the actual immutable release candidates for install/offline/migration/backup/restore/update/recovery behavior, feedback is resolved, and explicit checkpoint acceptance and authorization to enter Phase 12 are recorded under the [manual checkpoint policy](README.md#user-manual-checkpoint-gates);
- all declared target-specific, one-install, exact-artifact, signing, update, license, SBOM, notices, provenance, migration, offline, and support-bundle contracts above have evidence;
- clean-machine install/open/solve/verify/export/update/uninstall smokes pass per target and no external runtime is required;
- current/historical portable fixtures, scenario/full-backup import, add/replace restore, safety backup, atomic recovery, CLI/desktop parity, extension/media type/file association, and malicious-archive gates pass on every declared target;
- one-file offline HTML and direct PDF pass exact privacy-preview, accepted-result provenance, inert-data/CSP, zero-required-network, supported-browser, accessibility/list-table, print, immutability, atomic-cleanup, and large-report gates;
- exact artifacts contain no blocked dependency/solver and their notices/SBOMs/licenses/checksums/signatures/provenance are complete;
- documentation is complete, walkthrough-tested, and accurate about portability/recovery/share privacy, limitations, performance scope, solver budgets, first-verified-feasible/proof status, security, experimental features, and non-certification;
- every supported historical database and portable version plus previous-beta backup/restore fixture passes;
- every unresolved identity, target, updater, governance, and signing gate has a recorded decision rather than an implicit default;
- immutable candidate digests and evidence are handed to [Phase 12](12-stabilization-and-public-release-gate.md).
- Phase-10 named-profile, privacy, retention, native-credential, proposal and recovery documentation matches the enabled adapter contracts and exact packaged behavior; credentials and conversation/provider payloads remain absent from portable/share data.

Phase completion does not authorize public publication; Phase 12 does.

## Deferred and non-goals

- Linux arm64 and Windows arm64 release artifacts remain future targets unless their explicit gates close.
- rpm/repository distribution, universal macOS bundles, hosted package repositories, and automatic telemetry are not required for the MVP.
- No third-party native plug-in ABI, marketplace, arbitrary native code loading, or dynamic official pack loading.
- No bundled GPL/AGPL solver, proprietary solver binary, or user-installed solver discovery in the MVP.
- No mandatory account, hosted service, remote solve, or telemetry; local/offline operation is first-class.
- No claim of bit-for-bit reproducible signed/notarized platform bundles when timestamps/platform services prevent it.
- No “legally compliant,” certified, guaranteed optimal, or minimal-conflict claim unless separately proven.
- Post-MVP telemetry, sandboxed packs, additional solvers, collaboration, server mode, hosted services, and new targets belong to [Phase 13](13-post-mvp-roadmap.md); the proposed household-transportation pack has its separate sibling plan in [Phase 14](14-transportation-domain-pack.md).
- Early post-MVP conversational experiments, dictation/realtime voice, MCP access, delegated reasoning, and conditional subscription-runtime integrations belong to Phase-13 Branch K. They do not add audio runtimes, microphone permissions, server listeners, provider login promises, or new candidate gates to this MVP. Each later release must repeat the applicable packaging, license, credential, privacy, accessibility, and documentation evidence for its actual supported modes.
- Collaborative planning and managed operations follow [open-software and deployment parity](collaborative-planning-and-hosted-service.md#open-software-and-deployment-parity), not commercial software feature unlocks. The later narrow Workforce pilot includes a documented self-hostable reference deployment; managed operations use the same open capabilities. Applicable security controls carry across collaboration/service deployments without imposing the entire intake pilot on every deployment. This does not change existing technical safety/resource limits, add hosted capacity to local packaging, or merge school, voice, and transportation release gates.

## Assumption and version gates

The authoritative values and evidence are in [the assumptions ledger](assumptions.md). Phase 11 applies these specific gates:

- Rust 1.97.1 is the release toolchain until a newer stable fixes Rust 1.98.0's known P-critical miscompilation and passes the matrix.
- Node 24.20.0 LTS, pnpm 11.24.0, and TypeScript 6.0.3 are the current supported recommendations. TypeScript is capped below 6.1 because `typescript-eslint` 8.68.0 declares `<6.1.0`; exact lockfile pins and integrity are Phase-0 records.
- Current direct-registry frontend versions are Vue 3.5.42, vue-router 5.3.0, Pinia 4.0.3, Vite 8.2.2, `@vitejs/plugin-vue` 6.0.8, Tauri npm packages recorded in the ledger, Tailwind 4.3.3, shadcn-vue 2.8.2, Reka 2.10.4, TanStack Table 9.2.4, TanStack Virtual 3.13.36, Konva 10.3.2/vue-konva 3.4.0, ECharts 6.1.0/vue-echarts 8.1.0, ESLint 10.9.1, and Vitest 4.1.11.
- Tauri Rust crates are `tauri` 2.11.5 and `tauri-build` 2.6.3; npm package versions are independently pinned exactly as the ledger records.
- OR-Tools 9.15 is eligible only after all K.3 platform/build/benchmark/protobuf/assumption-core gates. The known v9.14/v9.15 presolve assumption-core issue must be characterized; diagnostic cores remain sufficient, never claimed minimal.
- Protobuf/protoc must match the pinned OR-Tools protocol/build contract rather than blindly using current protobuf 36.0.
- Pumpkin 0.5.0 remains experimental until support-matrix, dedicated-thread/cooperative-cancellation, timeout, verifier, and benchmark gates pass.
- `pkgs.nixfmt` replaces the deprecated `pkgs.nixfmt-rfc-style`; every Nix attribute is verified against the committed nixpkgs revision. App-indicator choice remains gated because the legacy library is marked obsolete upstream.
- WebView2 bootstrap/runtime, Wayland/X11, AppImage/deb/rpm mix, macOS x86_64 runners, updater endpoint, signing key lifecycle, supported OS minimums, and Linux package scope are unresolved K.5/K.8 release decisions.
- SPDX 3.0.1 is the current stable specification, but Syft output format support is verified before claiming SPDX 3 output; SPDX 2.3 remains the interoperable fallback. CI/release tool/action versions are exact and SHA-pinned from the ledger at implementation.
