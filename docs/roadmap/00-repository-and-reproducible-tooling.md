# Phase 00 — Repository and reproducible tooling

[Roadmap index](README.md) · Next: [Phase 01](01-core-application-shell-and-persistence.md) · Evidence: [assumptions.md](assumptions.md)

## Outcome

A clean checkout of `eutheto` has one reproducible, legally complete monorepo; pinned Rust, Node, pnpm, Nix, native-worker, frontend, test, license, and release tools; Cargo/pnpm workspaces; a minimal real Vue/Vite/Tauri application; documented native Windows/macOS prerequisites; generated-code ownership; and CI that executes the same commands contributors use. Shell entry is fast and side-effect-free. This phase creates foundations only—it does not implement domain behavior or claim a shippable desktop application.

## Source coverage

Primary: blueprint §8, §25, §29 Phase 0; relevant §24.10–24.11, §26, §28, §31–33; Appendices B, H, I (`FOUND-001`–`FOUND-005`, `SEC-001` foundation), J, K.1–K.5/K.8, and L. Approved ADR-001–018 are copied into individual ADR files without changing their decisions.

## Dependencies

None. This phase is required by every later phase. It must not import future domain/backend implementation merely to make checks look complete.

## Decisions and invariants

- Project/repository/crate/npm namespace is `eutheto`; Rust crates are `eutheto-*`, npm packages `@eutheto/*`.
- The working CLI `optimizer`, reverse-domain application ID, public file extension, hosting organization, governance/security contacts, and signing/notarization plan remain explicit gates. Do not publish or register placeholders.
- Apache-2.0 covers core, CLI, desktop, official packs/adapters, project worker code, examples, and docs unless marked otherwise. DCO sign-off is required; no CLA initially.
- Cargo is Rust workspace authority; pnpm is JS/TS workspace authority; `Justfile` is the human command authority; Nix supplies development/release tools; `xtask` owns cross-platform generation/hashing/assembly logic.
- Commit `Cargo.lock`, `pnpm-lock.yaml`, `flake.lock`, exact CI action SHAs, worker source/hash metadata, and generated protocol descriptors/source hashes as applicable.
- Generated sources are checked in only when release builds consume them or onboarding materially benefits. They are never hand-edited; regeneration in a clean tree must prove no drift.
- GitHub Actions use full commit SHAs, read-only tokens by default, protected release environments, separate build/sign jobs, and artifact digest verification. Secrets never enter Nix derivations, default shell variables, repository `.env` files, PR jobs, logs, or caches.
- Nix is canonical and hermetic on Linux. Nix supplies language/build tooling on macOS while Xcode SDK/signing stays native. WSL may run core/CLI tests but is never authoritative for Windows WebView2, installers, signing, or sidecar behavior.
- Shell entry never installs packages, downloads dependencies, runs migrations, builds solvers, or writes source outputs. `pnpm install`, Cargo fetch, generation, and worker build occur only through explicit bootstrap commands.
- `unsafe` is forbidden in normal project crates. A narrowly scoped exception requires a safety module, documented invariants, dedicated tests, and maintainer review. Hidden global mutable state is forbidden. Library code uses typed errors with no user-triggerable panic/unwrap/expect, canonical work uses stable ordering, and dependency direction is enforced from the first crate.
- All public serialized formats have versions and compatibility policy; fields/tags are never reused with changed meaning; unknown-newer versions fail safely; centralized size/count/nesting limits receive tests.

## Repository layout

Create this complete shape, omitting only directories explicitly marked for later content—not renaming boundaries:

```text
eutheto/
├── .cargo/config.toml
├── .githooks/
├── .github/{ISSUE_TEMPLATE,workflows,CODEOWNERS,dependabot.yml}
├── apps/
│   ├── desktop/{src,public,src-tauri,package.json,vite.config.ts,components.json}
│   └── docs/                         # Nuxt reserved for post-MVP
├── crates/
│   ├── eutheto-types
│   ├── eutheto-domain-api
│   ├── eutheto-domain-ir
│   ├── eutheto-planning-ir
│   ├── eutheto-protocol
│   ├── eutheto-core
│   ├── eutheto-store
│   ├── eutheto-command
│   ├── eutheto-solver-api
│   ├── eutheto-solver-router
│   ├── eutheto-solver-ortools
│   ├── eutheto-solver-pumpkin
│   ├── eutheto-verify
│   ├── eutheto-explain
│   ├── eutheto-ai
│   ├── eutheto-import
│   ├── eutheto-export
│   └── eutheto-cli
├── domains/{workforce,seating,school}/{core,ui,fixtures,docs}
├── packages/{ui,frontend-api,frontend-core,test-fixtures}
├── protocol/{solver-worker.proto,version.json,golden}
├── workers/ortools/{CMakeLists.txt,cmake,src,tests,VERSION}
├── benchmarks/{corpus/{workforce,seating,school},expected,runner}
├── docs/{architecture,adr,contributors,domain-packs,releases.md,threat-model.md,roadmap}
├── examples/{clinic-and-call,wedding-seating,school-blocks}
├── migrations/
├── nix/{tooling.nix,dev-shell.nix,dev-shell-welcome.sh,release-tooling.nix,release-shell.nix,checks.nix,packages.nix,ortools-worker.nix,eutheto-cli.nix,source-filter.nix}
├── scripts/bootstrap-windows.ps1
├── tests/{integration,e2e,migration,protocol,security}
├── xtask/
├── .envrc
├── AGENTS.md
├── Cargo.lock
├── Cargo.toml
├── CODE_OF_CONDUCT.md
├── CONTRIBUTING.md
├── DCO.md
├── flake.lock
├── flake.nix
├── GOVERNANCE.md
├── Justfile
├── LICENSE
├── NOTICE
├── package.json
├── pnpm-lock.yaml
├── pnpm-workspace.yaml
├── README.md
├── rust-toolchain.toml
├── SECURITY.md
├── THIRD_PARTY_LICENSES/
├── THIRD_PARTY_NOTICES.md
└── TRADEMARKS.md
```

Workflows are `pr.yml`, `portable.yml`, `security.yml`, `benchmark.yml`, `fuzz.yml`, `release.yml`, and `dependency-update.yml`. The portable matrix owns the current unbundled desktop launch smoke; packaged desktop E2E remains a later release-phase workflow. `AGENTS.md` states dependency boundaries, canonical commands, generated-code rules, schema/protocol discipline, and tests expected from human and AI contributors. Platform conditionals live in `nix/tooling.nix` or package derivations, not throughout `flake.nix`.

## Current verified version baseline (2026-08-29)

These are bootstrap inputs, not floating ranges. Phase 0 records exact selections/integrities in lock/toolchain files. A lockfile may select transitive patches, but documentation and manifests must not claim stale blueprint majors.

### Toolchains, solvers, and native/release tools

| Component | Phase-0 baseline | Compatibility/gate |
|---|---:|---|
| Rust | **1.97.1** | Use until a stable newer than 1.98.0 fixes the known P-critical vtable miscompilation; 1.98.0 is forbidden for release/build baselines. |
| Node.js | **24.20.0 LTS** | Production LTS; root engine `>=24 <25`. |
| pnpm | **11.24.0** | Exact `packageManager` with integrity/Corepack metadata; root engine `>=11 <12`; supersedes blueprint pnpm 10. |
| TypeScript | **6.0.3** | Newest compatible stable because `typescript-eslint` 8.68.0 declares `<6.1.0`; do not take TS 7.0.2 yet. |
| OR-Tools | **9.15 candidate** | Pin only after target builds, benchmarks, source/hash, license, callback and assumption-core gates. Presolve issue #5141 affects assumptions in 9.14/9.15. |
| Protobuf/protoc | match pinned OR-Tools | Upstream 36.0 is not automatically correct; pin generator/runtime/protos as one tested contract. |
| Pumpkin | **0.5.0 candidate** | Phase 08 only after actual support matrix, dedicated-thread ownership, cooperative cancellation and benchmarks. |
| nixfmt / Just / Ninja / LLVM | 1.4.0 / 1.58.0 / 1.13.2 / 23.1.0 evidence | Exact derivations follow `flake.lock`; nixpkgs ≥25.11 uses `pkgs.nixfmt`, not the deprecated alias. |
| Syft / Cosign / SLSA verifier | 1.51.1 / 3.1.3 / 2.7.1 evidence | Release-shell derivations follow `flake.lock`; signing choices remain gates. |
| GitHub CLI / Docker Buildx | 2.98.0 / 0.36.1 evidence | Release shell only. |

### Rust production crates

| Role | Crate | Verified stable |
|---|---|---:|
| serialization/schema/DTO | `serde`, `serde_json`, `schemars`, `ts-rs` | 1.0.229, 1.0.151, 1.2.2, 12.0.1 |
| errors | `thiserror`, edge-only `anyhow` | 2.0.20, 1.0.104 |
| async/cancellation | `tokio`, `tokio-util`, `async-trait` | 1.53.1, 0.7.19, 0.1.92 |
| logging | `tracing`, `tracing-subscriber` | 0.1.44, 0.3.23 |
| IDs/time | `uuid`, `jiff` | 1.26.0, 0.2.35 |
| database/compression/bundle/hash | `rusqlite`, `zstd`, `zip`, `blake3` | 0.40.2, 0.13.3, 8.6.0, 1.8.7 |
| protocol | `prost`, `prost-build` | 0.14.4, 0.14.4; OR-Tools protos remain matched-gate |
| HTTP/URL/credentials | `reqwest`, `url`, `keyring`, `zeroize` | 0.13.4, 2.5.8, 4.1.6, 1.9.0 |
| compatibility/import/CLI/paths | `semver`, `csv`, `clap`, `directories` | 1.0.28, 1.4.0, 4.6.6, 6.0.0 |
| geometry candidate | `rstar` | 0.13.0; custom deterministic indexed pairs first |
| desktop | `tauri`, `tauri-build` | 2.11.5, 2.6.3 |

Roles are binding: bundled SQLite behind a dedicated service; strict project ZIP wrapper; BLAKE3 not password hashing; Rustls where provider/platform permits; checked deterministic ordered maps; reviewed calendar serializer; `anyhow` only at executable/application edges.

### Rust quality tools

`cargo-nextest` 0.9.143; `cargo-llvm-cov` 0.9.0; `proptest` 1.11.0; `insta`/`cargo-insta` 1.48.0; `criterion` 0.8.2; `tempfile` 3.27.0; `pretty_assertions` 1.4.1; `cargo-fuzz` 0.13.2; `libfuzzer-sys` 0.4.13; `cargo-deny` 0.20.2; `cargo-about` 0.9.2; `cargo-audit` 0.22.2; `cargo-cyclonedx` 0.5.9.

### Frontend runtime/build matrix

| Package | Verified version | Compatibility note |
|---|---:|---|
| `vue` | 3.5.42 | Composition API. |
| `@vue/compiler-sfc` | 3.5.42 | Mandatory direct build pin; exact patch matches `vue`. |
| `vue-router` | 5.3.0 | Vue 3.5/Vite 8/Pinia 4 compatible. |
| `pinia` | 4.0.3 | ESM-only; view state only. |
| `@pinia/colada` | 1.4.2 | Mandatory direct pin for server/async state; it does not become authoritative scenario state. |
| `@vue/devtools-api` | 8.2.1 | Mandatory direct Pinia peer pin; production enablement remains reviewed. |
| `vite` / `@vitejs/plugin-vue` | 8.2.2 / 6.0.8 | Node 24 compatible. |
| `@tauri-apps/api` / CLI | 2.11.1 / 2.11.4 | Match Rust Tauri 2.11.5 by tested lock. |
| updater / shell plugins | 2.10.1 / 2.3.5 | Capability-scoped; updater artifacts use `bundle.createUpdaterArtifacts`. |
| Tailwind / Vite plugin | 4.3.3 / 4.3.3 | CSS-first; `tw-animate-css`, not `tailwindcss-animate`. |
| shadcn-vue / Reka UI | 2.8.2 / 2.10.4 | Editable source; audit Tailwind 4 variable syntax. |
| `@lucide/vue` | 1.37.0 | Maintained Vue icon package; icons do not replace accessible names. |
| TanStack Table / Virtual | 9.2.4 / 3.13.36 | Table v9 `useTable`, not v8 examples. |
| Konva / vue-konva | 10.3.2 / 3.4.0 | Seating; accessible list equivalent required. |
| ECharts / vue-echarts | 6.1.0 / 8.1.0 | Selected explanatory views only. |
| animation/class stack | `tw-animate-css` 1.4.0; `tailwind-merge` 3.6.0; CVA 0.7.1; `clsx` 2.1.1 | One convention. |

### Frontend quality matrix

ESLint 10.9.1; `@vue/eslint-config-typescript` 14.9.0; `typescript-eslint` 8.68.0; `eslint-plugin-vue` 10.10.0; Prettier 3.9.6; Vitest 4.1.11; Vue Test Utils 2.5.0; Testing Library Vue 8.1.0; axe-core 4.13.0; WebDriverIO/CLI 9.31.4; Tauri service 1.3.0; Playwright/test 1.62.1. Playwright tests pure Vite only and does not replace packaged Tauri E2E.

Avoid initially: domain-relational ORM, duplicated frontend authority, generic workflow engine, Electron dependencies, Python production embedding, unofficial consumer OAuth/session libraries, GPL/AGPL solvers in official artifacts, native dynamic plugins, universal untrusted expression languages, and a generic heavy dashboard suite.

## Nix and native environment contract

The thin flake pins nixpkgs, default systems, and rust-overlay; uses `pkgs.rust-bin.fromRustupToolchainFile`; leaves `allowUnfree = false`; and evaluates `x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, `aarch64-darwin`. It exports `pkgs.nixfmt`, CLI and worker packages, default/full/release shells, and lightweight checks. Linux is fully hermetic; macOS needs Xcode SDK/tools; Windows uses native pinned tooling.

`rust-toolchain.toml` pins 1.97.1, minimal profile, Cargo, clippy, rust-analyzer, rust-src and rustfmt. Root `package.json` is private `eutheto-workspace`, pins pnpm 11.24.0 with integrity/Corepack metadata, and engines Node `>=24 <25`, pnpm `>=11 <12`. If nixpkgs lacks the exact pnpm package, explicit bootstrap prepares that Corepack artifact; never regress to pnpm 10 or float latest.

`nix/tooling.nix` supplies Rust tools; Node 24 and exact pnpm activation; Clang/libclang/CMake/Ninja/pkg-config/matched protobuf/OpenSSL/SQLite; Git/LFS, Just, jq/yq, tooling-only Python/Expect, archive tools; Linux GLib/GTK3/WebKitGTK 4.1/libsoup 3/librsvg/current Tauri AppIndicator dependency/patchelf/X server; Darwin cctools. `libayatana-appindicator` exists but is obsolete upstream; select Tauri's current requirement. Export deterministic Rust source/libclang/protoc/CMake paths and construct Linux runtime paths from declared libraries.

Default shell uses project-local caches and prints only commands. `.envrc` is exactly `use flake .`. It never fetches/builds/installs on entry. Release shell adds cargo-about/cyclonedx, Syft, Cosign, SLSA verifier, GitHub CLI and archive tools; secrets/identities stay in protected CI or OS keychains.

Phase 0 creates an OR-Tools derivation contract, but phase 03 supplies its approved pin. It must fetch one exact source/hash, build only needed CP-SAT/worker components, disable examples/wrappers/GLPK/proprietary/unrelated integrations, run executable checks where possible, and install only worker/runtime, licenses and solver manifest. Never guess CMake flags across releases or ship a dummy worker.

Native Windows bootstrap checks Visual Studio C++ workload, SDK, WebView2 plan, shared Rust pin, Node/pnpm, CMake/Ninja/matched protoc/Git/Just; it never silently performs privileged installs. Windows 10/Server/LTSC cannot be assumed to contain WebView2. macOS docs specify Xcode and runner architecture prerequisites.

## Human command and generation contract

`Justfile` delegates `bootstrap`, `install`, `generate`, `generate-check`, `fmt`, `fmt-check`, `lint`, `typecheck`, Rust/doc/UI/aggregate tests, coverage, worker Nix/native build and smoke, working CLI, desktop/UI dev, CLI/desktop build, bench, E2E, licenses, SBOM, Nix check, aggregate check and release preflight. Adapt nixfmt check syntax to the pinned formatter.

`xtask` owns:

```text
xtask generate
xtask solver build-native
xtask solver install-from-nix
xtask solver smoke
xtask protocol verify
xtask licenses generate
xtask sbom generate
xtask fixtures validate
xtask release verify-clean
xtask release assemble-manifest
```

Use it for cross-platform hashing, JSON, target triples, licenses and generated files instead of growing Bash. Generation twice must be byte-identical; checked-in TS DTOs, schemas, protocol outputs and notices are never hand-edited.

### Dependency-update workflow gate

`dependency-update.yml` is a required policy workflow implementing the immutable-action and routine-update isolation parts of blueprint §§24.11 and 25.17. It does not repeat build, generation, security, worker, or benchmark commands already owned by other workflows. Every automated or manual dependency-update PR must use frozen committed `flake.lock`, workspace `Cargo.lock`, `pnpm-lock.yaml`, CI-action SHAs, OR-Tools source/hash metadata when applicable, and generated protocol evidence, and must pass the applicable existing gates before merge:

1. `pr.yml` for `nix flake check` and the canonical full Rust and JavaScript suite;
2. `security.yml` for Rust and pnpm advisory/license/source policy, secret scanning, notices, and SBOM evidence;
3. `portable.yml` for native source build, test, and unbundled desktop-launch evidence;
4. deterministic generation and drift checks through the canonical `just check` path;
5. the real worker workflow and published official solver benchmark thresholds once their owning phases provide executable gates.

JavaScript packages with new or changed install scripts are blocked unless the exact script and package are explicitly reviewed and allowlisted with rationale. Routine update PRs isolate major Rust, Node, Tauri, OR-Tools, and schema migrations: one independently reviewable major family or migration per PR, never a combined major-update rollup. CI actions remain immutable-commit pinned, and dependency automation cannot create a license, security, generation, worker, benchmark, or review exception. `CODEOWNERS` remains the sensitive-path ownership map. Required independent approval is temporarily disabled while the repository has one active maintainer and must be restored when a second qualified maintainer is active.

## Ordered work packages

1. **FOUND-001 — Identity, workspaces, legal baseline.** Initialize Cargo/pnpm, normalized names, legal/governance/security/trademark/DCO files, CODEOWNERS, SPDX/REUSE policy and unresolved identity ADR without fake contacts.
2. **FOUND-002 — Nix.** Add thin flake/lock, tooling/default/full/release shells, packages/checks/source filter, `.envrc`, safe caches and no-side-effect welcome; correct nixfmt and pnpm assumptions.
3. **FOUND-003 — Pins, commands, generation.** Commit the current matrix, `Justfile`, `xtask`, deterministic DTO/schema/protocol/license workflows and the dependency-update policy, including the routine-PR migration-isolation rule above.
4. **FOUND-004 — CI/supply chain.** Add seven non-duplicative workflows with full action SHAs, least privilege, concurrency/cache controls, native prerequisites, digest handoff and sanitized artifacts.
5. **FOUND-005 — Real desktop boundary.** Minimal Vue/Vite/TS/Tauri shell, generated API, strict TS and no mock domain authority. Register custom commands through Tauri `AppManifest`/capabilities because `invoke_handler` alone permits all windows; minimum permissions/CSP; only `apps/desktop/src/api` imports invoke/event.
6. **Architecture/ADRs.** Materialize ADR-001–018, contributor boundaries, schema/protocol discipline, and automated dependency/architecture enforcement for both the normal-crate `unsafe` exception gate and the prohibition on hidden global mutable state.
7. **Worker/release scaffolds.** Worker manifest schema, explicitly gated derivation, release-tool inventory, license inputs and protected-signing checklist.
8. **Onboarding.** Execute/document Nix, no-direnv, native Windows, macOS/Xcode and core-only routes, cache fallback and WebKitGTK diagnostics.

## Test and acceptance plan

`nix flake check` checks required files, valid lock JSON, exact non-floating Rust/package-manager pins, Nix format, every required command under `/nix/store`, legal files and parseable workspaces. The full suite remains `just check`, not hidden in flake evaluation.

CI:

- `pr.yml`: pinned checkout/Nix, public cache, flake check, Nix-shell install/check, later worker smoke, sanitized reports;
- `portable.yml`: path-relevant Tier-1 Ubuntu, macOS arm64, and Windows checks on pull requests; macOS x86_64 additionally on sensitive pull requests, merge-group candidates, `main`, version tags, manual dispatches, and weekly validation; shared pins, native core and shell builds, and current unbundled shell-launch smoke;
- `security.yml`: deny/audit, pnpm lock/license/audit, secret scans, optional REUSE, SBOM smoke, capability diff;
- benchmark/fuzz are scheduled/path-gated and never run arbitrary PR scripts; release has protected separate build/sign jobs.

The user-authorized [temporary Phase06 integration trial](../contributors/git-workflow.md#temporary-phase06-integration) is the sole bounded cadence exception for exact phase-target PRs: locked Nix evaluation plus source/frontend-build/security checks on every package, risk-selected full matrices, and four named full integrated-phase checkpoints. It does not change protected-main, final phase, artifact, or release gates.

Fixed clock/time zone/locale/seed/thread count and temp directories are mandatory test context. Future coverage gates are established: verifier/migrations/protocol ≥90% branch; command/persistence/domain/compiler ≥80% line; frontend business logic ≥75% line; documented exceptions only, never excluding difficult code to meet a number.

### Exact phase exit evidence

1. `nix flake check` passes on each supported Nix system available in CI; runner gaps remain explicit.
2. `nix develop --command just check` runs a real trivial suite entirely from pinned tools.
3. Native Windows/macOS jobs build hello-world core and launch the actual Vue/Vite/Tauri shell using shared pins.
4. Frozen pnpm and locked Cargo install without lock mutation; double generation is byte-identical.
5. Format, clippy-deny, strict frontend lint/typecheck, license policy and basic tests pass.
6. The one shell API is generated/typed, capability-scoped through `AppManifest`, invoked only from the API layer and rendered without mock authoritative state.
7. Apache-2.0/DCO/governance/security/trademark files exist; sample DCO/license failures are detected; notice/SBOM smoke identifies exact inputs.
8. Native bootstrap verifies rather than silently installs; clean Nix/no-direnv onboarding matches docs.
9. No global language/tool dependency exists beyond Nix/direnv or documented native platform prerequisites.
10. CLI, reverse-domain IDs, extension, organization/contacts and signing choices remain visibly gated rather than fabricated.

## Risks and failure handling

- **Rust 1.98.0:** policy check pins 1.97.1 until fixed newer stable and full matrix.
- **Ambient/Nix drift:** pure CI plus command provenance and native matrix.
- **pnpm 11 nixpkgs gap:** exact Corepack bootstrap or exact locked package, never pnpm 10/floating latest.
- **OR-Tools/protobuf mismatch:** matched contract and phase-03 tests; no newest-protoc assumption.
- **Obsolete package names:** `pkgs.nixfmt`; current Tauri AppIndicator requirement checked at each lock update.
- **Tauri capability illusion:** build manifest registration, minimum per-window scope and diff review.
- **WebView2/AppImage surprises:** preserve release gates; no phase-0 support claim.
- **Supply-chain leakage:** separate digest-verified build/sign jobs; no PR/derivation/cache secrets.
- **License contamination:** allow/review/block policy, exact manifests, code-owner review, no GLPK/GPL/proprietary official bundle.
- **Unavailable Balanceframe reference:** preserve independently sound split-flake patterns without claiming verified provenance.

## Licensing and governance gate

Allowed by default: Apache-2.0, MIT, BSD-2/3-Clause, ISC, Unicode and reviewed equivalents. Review MPL-2.0, EPL-2.0, LGPL, attribution/source-offer/linking/data/model/trademark nuances, assets/fonts/datasets and custom/unknown terms. Block official GPL, AGPL, SSPL/source-available, noncommercial/no-derivatives and proprietary solver binaries unless policy changes after legal review; a process boundary is not automatic permission. Use cargo-deny/about, pnpm inventory, `xtask`, Syft/Cargo SPDX and generated diffs. Record every asset source/license; user-imported images remain user data.

## Deferred and non-goals

No domain behavior, solver, verifier, AI provider, full design system, updater/signing/installer, Nuxt desktop layer, marketplace, ORM/workflow engine, dynamic native packs, or Python runtime. Cache publication and full Linux desktop derivation wait for evidence. Future command scaffolds must not masquerade as working no-ops.

## Assumption and version gates

Record dated evidence in `assumptions.md`: final `eutheto` name and Rust `eutheto-*`/npm `@eutheto/*` namespaces closed; unresolved CLI, reverse-domain/application-channel IDs, public extension, exact crate/package inventory, organization/contacts and release gates open with owners; exact flake attributes/Darwin behavior; all pins/integrities/licenses and Rust/TS rationale; OR-Tools/Pumpkin remain candidates; matched protobuf; OS/WebView2/Wayland/X11/Linux artifact/macOS runner/updater/static-link/signing choices carried to phase 11; exact-artifact licensing/accessibility/clean-machine publication carried to phase 12.

## Exit gate

Phase 00 exits only when all ten exact evidence items pass and every version/identity gate has an explicit state, with no ambiguity hidden by an ambient installation. Then proceed to [Phase 01](01-core-application-shell-and-persistence.md).
