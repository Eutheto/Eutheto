# Assumptions, Decisions, and Current-Version Ledger

## Ledger purpose and authority

This dated ledger records implementation-time facts, corrections, unresolved gates, and evidence. It does **not** replace requirements in the phase roadmaps. Requirements remain in the applicable phase file; this ledger answers “what was verified, what value controls, who closes the gate, and from which evidence?”

- **Blueprint source:** `open-source-constraint-optimizer-development-mvp-post-mvp-blueprint.md`
- **Blueprint SHA-256:** `ed094402135dc8e7a1c66b640484b4a4643e631024439016e48b895def00d13e`
- **Verification date:** 2026-08-29
- **Transportation blueprint source:** `eutheto-transportation-domain-pack-mvp-post-mvp.md`
- **Transportation blueprint SHA-256:** `9e269a173c217fcd6d08d4afaab9d82da3da43acda428b21c4c081cb353783e0`
- **Transportation blueprint date:** 2026-08-29
- **Performance/solver UX source:** `eutheto-performance-ux-targets.md`
- **Performance/solver UX SHA-256:** `f5ad3479f76e22a16ca355abd8f3323add9f853003d9590c97d3080c2a3c389a`
- **Performance/solver UX date:** 2026-08-29
- **Export/import/backup/sharing source:** `eutheto-export-import-backup-sharing-spec.md`
- **Export/import/backup/sharing SHA-256:** `679efcf2beb4c27a60ba36fae28870d51900d28180c34a17665c57dd0c7e8181`
- **Export/import/backup/sharing date:** 2026-08-29
- **Final project name:** `eutheto`
- **Version policy:** use the latest stable supported version. If newest is incompatible or has a known blocker, use the newest compatible/safe stable and record the reason. Exact dependency/tool/action pins and integrity hashes are committed in Phase 0 lockfiles/toolchain files.
- **Conflict rule:** direct npm registry, crates.io API, and GitHub releases API evidence recorded in the dated ledgers below controls over narrative agent reports. Conflicting values are not retained as alternatives.

Related delivery files: [Performance and Solver UX Targets](performance-and-solver-ux-targets.md), [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md), [Phase 11 packaging/documentation](11-public-mvp-packaging-and-documentation.md), [Phase 12 stabilization/release](12-stabilization-and-public-release-gate.md), [Phase 13 post-MVP](13-post-mvp-roadmap.md), and [Phase 14 transportation](14-transportation-domain-pack.md).

## Final and unresolved identity decisions

| Decision | Status/value | Owner/gate |
|---|---|---|
| Project name | **Resolved: `eutheto`** | Project-wide; Phase 0 records it everywhere |
| License/contribution | **Resolved: Apache-2.0 and DCO; no CLA initially** | Phase 0 legal files; Phase 11/12 compliance |
| Working CLI name | **Unresolved: `optimizer` is provisional, not final** | K.1 / Phase 0; must close before stable CLI docs and service mode |
| Crate/package namespace and prefixes | **Resolved: `eutheto` namespace prefix**; Rust crates use `eutheto-*` and npm packages use `@eutheto/*`. The exact crate/package inventory names remain Phase-0 initialization decisions. | K.1 / Phase 0 |
| Reverse-domain application ID | **Unresolved** | K.1 / Phase 0 and Phase 11 signing/updater |
| Portable file extension/media type/association | **Unresolved; `.eutheto` is the current proposal, not a public decision.** `.optplan` is no longer the working proposal. | Phase 11 identity ADR and Phase 12 cross-platform open/inspect/package evidence |
| Git hosting organization | **Unresolved** | K.1 / Phase 0 and Phase 11 release URLs |
| Governance/security contacts | **Unresolved**; do not fabricate addresses | K.1 / Phase 11 governance/security |
| Stable/beta application identifiers | **Unresolved** | K.1 / Phase 11 updater/signing continuity |
| Release signing/notarization/key custody | **Unresolved choices; required before release** | K.1/K.5/K.8 / Phase 11–12 |

## Cross-cutting proposal decisions

| Decision | Current status/value | Owner/gate |
|---|---|---|
| Performance targets | **Provisional engineering objectives, not public guarantees:** small warm <500 ms; typical warm target <1 s and usually <3 s cold; p95 normal <5 s; moderate majority <5 s and expected <10 s; bounded stress/pathological behavior. | [Performance and Solver UX Targets](performance-and-solver-ux-targets.md); Phase 12 calibrates public-MVP corpus/reference machine and Phase 14 recalibrates transportation. |
| Interactive solver budget | **Initial experiment:** approximately 2–3 s of CP-SAT within a 3–5 s end-to-end Balanced budget; Quick/Deep remain bounded and never imply proof. | Phases 02–08 implement one parent budget/status semantics; Phase 12 selects released defaults from evidence. |
| Performance reference hardware | **Provisional class:** ordinary 4–8-core, 16 GiB consumer machine, no dedicated GPU requirement. | Phase 12 records the exact machine, power mode, runner image, sample/percentile policy, corpus, cache state, and regression thresholds. |
| Portable compatibility policy | **MVP proposal:** internal SQLite/document storage is not interchange; current builds export only current canonical Portable Scenario/Result/Share Result schemas; released historical inputs migrate sequentially; unknown required semantics/newer versions fail before mutation; declared nonsemantic extensions preserve. | [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md); Phases 01–02 implement contracts, 05/07/09 specialize official packs, 11–12 freeze/release. |
| MVP portable scope | **Scenario bundle plus full-library backup/restore:** stable IDs and canonical units; Create copy/Replace/Skip scenario collisions; Add/Replace full restore with attempted pre-restore safety backup; no semantic merge, encryption, automatic backup, signatures, or hosted service in MVP. | Phases 01/06 own services/UI; Phase 12 owns permanent migration, malicious-archive, atomicity, and fresh-install recovery evidence. |
| MVP result sharing | **One privacy-filtered immutable Share Result Model:** one-file offline `file://` HTML is default; direct PDF is secondary but required; exact preview drives both; zero required network; accessible list/table parity; accepted-result provenance. | Phases 07/09 implement shared renderer and official-pack payloads; Phases 11–12 close browser, CSP, privacy, print/PDF, and release evidence. |

## Controlling recommendations

| Item | Latest observed | Controlling recommendation | Reason and owner |
|---|---:|---:|---|
| nixpkgs | `nixos-unstable` snapshot identifies as 26.11 | **`nixos-25.11`** | The 26.11 unstable snapshot dropped `x86_64-darwin`, which Phase 00 still requires alongside the other three flake systems. `nixos-25.11` is the newest line verified compatible with that four-system evaluation contract, not a claim about the newest nixpkgs generally. Re-evaluate when a newer supported line restores `x86_64-darwin` or project support policy closes that target. |
| nixGL | `b6105297e6f0cd041670c3e8628394d4ee247ed5` | **Locked flake input following repository nixpkgs** | Non-NixOS Linux cannot use Nix-built WebKitGTK reliably with ambient Mesa/NVIDIA userspace. The canonical desktop recipe applies the locked Mesa wrapper automatically and performs host-version NVIDIA selection only when the explicit launch command runs. Phase 0 owns the development wrapper; Phase 11/12 separately owns packaged runtime evidence. |
| Rust | 1.98.0 | **1.97.1** | Rust 1.98.0 has a P-critical trait-object vtable miscompilation; re-evaluate when 1.98.1 or newer fixed stable exists and passes Phase 0/12 target suites. [Rust releases](https://blog.rust-lang.org/releases/) and [issue #161441](https://github.com/rust-lang/rust/issues/161441). |
| Node.js | 26.8.1 Current | **24.20.0 LTS** | Production uses current LTS; re-evaluate Node 26 when it becomes LTS. Phase 0 owns engines/Nix. [Node distributions](https://nodejs.org/dist/). |
| pnpm | 11.24.0 | **11.24.0** | Direct registry current stable supports Node `>=22.13`; replaces blueprint/report pnpm-10 guidance. Phase 0 owns lock/integrity and Nix availability. [npm](https://www.npmjs.com/package/pnpm). |
| TypeScript | 7.0.2 | **6.0.3** | `typescript-eslint` 8.68.0 declares TypeScript `>=4.8.4 <6.1.0`; newest supported stable is 6.0.3. Phase 0 owns exact lock. [npm TypeScript](https://www.npmjs.com/package/typescript), [typescript-eslint](https://www.npmjs.com/package/typescript-eslint). |
| protobuf/protoc | 36.0 | **33.1, matched to OR-Tools 9.15** | OR-Tools tag `v9.15` pins protobuf `v33.1`; the locked four-system Nix toolchain exposes `protobuf_33` 33.1 and `libprotoc 33.1`. Do not blindly use newest protoc. Phase 3 owns generated-binding and C++ runtime conformance. [protobuf v33.1](https://github.com/protocolbuffers/protobuf/releases/tag/v33.1), [OR-Tools v9.15 dependency declaration](https://github.com/google/or-tools/blob/v9.15/cmake/dependencies/CMakeLists.txt). |
| OR-Tools | 9.15 | **9.15.6755 source input and bundled Phase-03 backend approved** | The generated source contract binds the exact source/protobuf/common-CMake/static-linkage decision to immutable approval evidence `d74be8614150ad8474dd78f59ffe0dc761fd742f`. Native unsigned package smoke now validates the bundled worker, trusted manifest, license/NOTICE payload, and SPDX 2.3 SBOM on every MVP target. Backend output remains an untrusted candidate until Phase 04 independently verifies it. Phase 11/12 owns exact signed release artifacts. [release](https://github.com/google/or-tools/releases/tag/v9.15). |
| Pumpkin | 0.5.0 | **0.5.0 after K.4 gates** | Actual support matrix, dedicated-thread ownership, cooperative cancellation/time limits, verifier/contracts, and benchmarks precede auto-routing. Phase 8 owns. [crates.io](https://crates.io/crates/pumpkin-solver). |

### Linux graphics runtime provenance

The `nixGL` input is locked to revision `b6105297e6f0cd041670c3e8628394d4ee247ed5` and follows the repository's `nixpkgs` input. Its verified wrapper contract is: `nixGLIntel` supplies Mesa for Intel, AMD, and Nouveau; `auto.nixGLDefault` chooses the proprietary NVIDIA wrapper when `/proc/driver/nvidia/version` identifies a loaded NVIDIA module and otherwise chooses Mesa. The automatic NVIDIA derivation is necessarily impure because its userspace version must exactly match the host kernel module; `just desktop-dev` defers that selection until the explicit launch so shell entry and non-desktop commands remain pure and side-effect-free.

The upstream NVIDIA download path is x86-64-only. The locked flake therefore supports the Mesa path on both Linux systems and proprietary NVIDIA only on `x86_64-linux`; a proprietary NVIDIA `aarch64-linux` desktop is not claimed. Sources reverified on 2026-08-29: [`nixGL` README](https://github.com/nix-community/nixGL/blob/b6105297e6f0cd041670c3e8628394d4ee247ed5/README.md), [`flake.nix`](https://github.com/nix-community/nixGL/blob/b6105297e6f0cd041670c3e8628394d4ee247ed5/flake.nix), and [`nixGL.nix`](https://github.com/nix-community/nixGL/blob/b6105297e6f0cd041670c3e8628394d4ee247ed5/nixGL.nix).

### Node 24.20.0 Nix provenance

The locked `nixos-25.11` graph exposes Node 24.18.0, while `nixos-26.05`—the last release line exposing `x86_64-darwin`—exposes Node 24.19.0. Neither satisfies the exact Phase-00 Node 24.20.0 contract. Until nixpkgs catches up without dropping a supported system, `nix/tooling.nix` uses the official Node binary archives as fixed-output inputs and patches only the Linux ELF interpreter/runtime references through Nix.

The controlling hashes are the entries in Node's official [`v24.20.0/SHASUMS256.txt`](https://nodejs.org/dist/v24.20.0/SHASUMS256.txt), verified on 2026-08-29:

| Nix system | Official archive | SHA-256 |
|---|---|---|
| `x86_64-linux` | `node-v24.20.0-linux-x64.tar.xz` | `2f2c0da162318f0de47665410c7c8c2ed3d36c8f3105de4bbc61176c70a7cbf2` |
| `aarch64-linux` | `node-v24.20.0-linux-arm64.tar.xz` | `5f4ddab610c1ab2016b3c227cebdbf6d9495161487e4739c7b90090595f465f7` |
| `x86_64-darwin` | `node-v24.20.0-darwin-x64.tar.xz` | `26fc30891004603d094eed11de5efcd03bbd2efbc35c177fc72648d5d7a7701b` |
| `aarch64-darwin` | `node-v24.20.0-darwin-arm64.tar.xz` | `b7bf7707070b950ba1ec5f1af3bb6de0f2b1962c5033973d94068ab021ef3014` |

Re-verify the filename/hash pairs against that official manifest whenever the selected Node patch changes. Re-evaluate and remove the binary-archive derivation when the locked nixpkgs graph supplies the exact selected Node version on all four supported systems; do not accept an older patch merely because it is the newest package in a compatible nixpkgs line.

### cargo-llvm-cov availability in locked nixpkgs

The locked `nixos-25.11` graph resolves `pkgs.cargo-llvm-cov` to `cargo-llvm-cov-0.6.20` and marks that derivation broken on both `x86_64-darwin` and `aarch64-darwin`. Nix therefore refuses to evaluate either macOS shell containing it. [`nix/tooling.nix`](../../nix/tooling.nix) excludes only that package on Darwin; both Linux systems retain it, including the Linux full shell used by CI. The default shell is unchanged.

The direct quality-tool selection remains `cargo-llvm-cov` 0.9.0. The portable CI matrix configures native locked-flake evaluation and default-shell realization for all four supported systems, with the macOS coverage omission above. Configuration alone does not prove exact-version parity, completed native macOS runners, or macOS coverage. Until the locked graph supplies the selected version on every supported system, this remains an explicit Phase-00 quality-tool exception rather than satisfying the exact 0.9.0 four-system tool contract. Linux CI is configured to run the canonical `just check` recipe, and macOS contributors can run coverage with the separately installed selected tool after completing the [native Xcode/SDK prerequisites](../contributors/macos.md#xcode-and-sdk-prerequisite).

Remove the Darwin omission only when a newly locked nixpkgs graph provides a non-broken `cargo-llvm-cov` at the selected version on all four systems. Re-run the all-system flake evaluation plus native coverage on each affected target before closing the exception.

## Complete direct npm-registry ledger

All values below are from `https://registry.npmjs.org/<package>/latest` on the verification date unless a runtime-major-compatible line is explicitly recorded. “Use” means current candidate for exact Phase-0 lockfile pin unless the controlling recommendation above overrides it.

| Package | Direct version | Compatibility/decision note | Owning phase |
|---|---:|---|---|
| [`pnpm`](https://www.npmjs.com/package/pnpm) | 11.24.0 | Node `>=22.13`; use | 0 |
| [`corepack`](https://www.npmjs.com/package/corepack) | 0.36.0 | Node `^22.22.2 || ^24.15.0 || >=26`; separately pin if used | 0 |
| [`typescript`](https://www.npmjs.com/package/typescript) | 7.0.2 | discovery latest; use **6.0.3** due typescript-eslint `<6.1` | 0/6 |
| [`@types/node`](https://www.npmjs.com/package/@types/node) | 24.13.3 | newest Node-24 types line verified on 2026-08-29; compatible with and pinned to the Node 24 runtime; use | 0/6/12 |
| [`vue`](https://www.npmjs.com/package/vue) | 3.5.42 | use | 0/6 |
| [`@vue/compiler-sfc`](https://www.npmjs.com/package/@vue/compiler-sfc) | 3.5.42 | mandatory direct build pin; exact patch matches Vue | 0/6 |
| [`vue-router`](https://www.npmjs.com/package/vue-router) | 5.3.0 | peers Vue `^3.5.34`, Vite 7/8, Pinia 3/4; use | 0/6 |
| [`pinia`](https://www.npmjs.com/package/pinia) | 4.0.3 | peer Vue `^3.5.11`, TS `>=5.6`; ESM-only caveat reviewed | 0/6 |
| [`@pinia/colada`](https://www.npmjs.com/package/@pinia/colada) | 1.4.2 | mandatory direct pin for server/async state; never authoritative scenario state | 0/6 |
| [`@vue/devtools-api`](https://www.npmjs.com/package/@vue/devtools-api) | 8.2.1 | mandatory direct Pinia peer pin; production enablement reviewed | 0/6 |
| [`vite`](https://www.npmjs.com/package/vite) | 8.2.2 | Node `^20.19 || >=22.12`; use | 0/6 |
| [`@vitejs/plugin-vue`](https://www.npmjs.com/package/@vitejs/plugin-vue) | 6.0.8 | supports Vite through 8; use | 0/6 |
| [`@tauri-apps/api`](https://www.npmjs.com/package/@tauri-apps/api) | 2.11.1 | use; independent of Rust crate patch | 0/6/11 |
| [`@tauri-apps/cli`](https://www.npmjs.com/package/@tauri-apps/cli) | 2.11.4 | use | 0/6/11 |
| [`@tauri-apps/plugin-updater`](https://www.npmjs.com/package/@tauri-apps/plugin-updater) | 2.10.1 | use; signed updater gate remains | 11 |
| [`@tauri-apps/plugin-shell`](https://www.npmjs.com/package/@tauri-apps/plugin-shell) | 2.3.5 | exact-sidecar permission only | 3/11 |
| [`tailwindcss`](https://www.npmjs.com/package/tailwindcss) | 4.3.3 | use | 4/6 |
| [`@tailwindcss/vite`](https://www.npmjs.com/package/@tailwindcss/vite) | 4.3.3 | supports Vite 5–8; use | 4/6 |
| [`shadcn-vue`](https://www.npmjs.com/package/shadcn-vue) | 2.8.2 | editable component source; use | 4/6 |
| [`reka-ui`](https://www.npmjs.com/package/reka-ui) | 2.10.4 | Vue `>=3.4`; use | 4/6 |
| [`@lucide/vue`](https://www.npmjs.com/package/@lucide/vue) | 1.37.0 | maintained Vue package; use; icons retain accessible names | 4/6 |
| [`@tanstack/vue-table`](https://www.npmjs.com/package/@tanstack/vue-table) | 9.2.4 | Node `>=20`, Vue `>=3.2`; use v9 API | 6/7/9 |
| [`@tanstack/vue-virtual`](https://www.npmjs.com/package/@tanstack/vue-virtual) | 3.13.36 | use | 6/7/9 |
| [`konva`](https://www.npmjs.com/package/konva) | 10.3.2 | optional canvas peers are not browser runtime requirements; use | 9 |
| [`vue-konva`](https://www.npmjs.com/package/vue-konva) | 3.4.0 | peers Vue 3 and Konva `>7`; use | 9 |
| [`echarts`](https://www.npmjs.com/package/echarts) | 6.1.0 | selected analytical views only | 7/13 |
| [`vue-echarts`](https://www.npmjs.com/package/vue-echarts) | 8.1.0 | peers Vue `^3.3`, ECharts `^6`; use | 7/13 |
| [`eslint`](https://www.npmjs.com/package/eslint) | 10.9.1 | supports Node 24; use | 0/12 |
| [`@vue/eslint-config-typescript`](https://www.npmjs.com/package/@vue/eslint-config-typescript) | 14.9.0 | supports ESLint 9/10 | 0/12 |
| [`typescript-eslint`](https://www.npmjs.com/package/typescript-eslint) | 8.68.0 | supports ESLint 8/9/10; TypeScript `<6.1` controls TS recommendation | 0/12 |
| [`eslint-plugin-vue`](https://www.npmjs.com/package/eslint-plugin-vue) | 10.10.0 | supports ESLint 8/9/10 | 0/12 |
| [`prettier`](https://www.npmjs.com/package/prettier) | 3.9.6 | use if selected formatter | 0 |
| [`vitest`](https://www.npmjs.com/package/vitest) | 4.1.11 | Node 20/22/24+, Vite 6/7/8; use | 0/12 |
| [`@vue/test-utils`](https://www.npmjs.com/package/@vue/test-utils) | 2.5.0 | Vue 3 peers; use | 0/12 |
| [`@testing-library/vue`](https://www.npmjs.com/package/@testing-library/vue) | 8.1.0 | use where user-centric tests help | 12 |
| [`axe-core`](https://www.npmjs.com/package/axe-core) | 4.13.0 | use directly; `@axe-core/vue` does not exist | 12 |
| [`webdriverio`](https://www.npmjs.com/package/webdriverio) | 9.31.4 | Node `>=18.20`; use | 12 |
| [`@wdio/cli`](https://www.npmjs.com/package/@wdio/cli) | 9.31.4 | align with WebDriverIO | 12 |
| [`@wdio/tauri-service`](https://www.npmjs.com/package/@wdio/tauri-service) | 1.3.0 | peers WebDriverIO 9; packaged Tauri E2E | 12 |
| [`playwright`](https://www.npmjs.com/package/playwright) | 1.62.1 | pure Vite UI only, not Tauri replacement | 12 |
| [`@playwright/test`](https://www.npmjs.com/package/@playwright/test) | 1.62.1 | align with Playwright | 12 |
| [`tw-animate-css`](https://www.npmjs.com/package/tw-animate-css) | 1.4.0 | Tailwind-4/shadcn animation utility | 4/6 |
| [`tailwind-merge`](https://www.npmjs.com/package/tailwind-merge) | 3.6.0 | one class-composition convention | 4/6 |
| [`class-variance-authority`](https://www.npmjs.com/package/class-variance-authority) | 0.7.1 | shadcn variant utility | 4/6 |
| [`clsx`](https://www.npmjs.com/package/clsx) | 2.1.1 | shadcn class utility | 4/6 |

## Complete direct crates.io ledger

Values are maximum stable releases reported by `https://crates.io/api/v1/crates/<crate>` on the verification date. Exact Cargo pins/features/licenses/MSRV are Phase-0 decisions; role-specific adoption remains gated by the owning phase.

| Crate/tool | Stable | Decision/note | Owner |
|---|---:|---|---|
| [`serde`](https://crates.io/crates/serde) | 1.0.229 | use, explicit features | 0–2 |
| [`serde_json`](https://crates.io/crates/serde_json) | 1.0.151 | use with strict boundaries | 0–2 |
| [`schemars`](https://crates.io/crates/schemars) | 1.2.2 | 1.x generated-schema review | 0–2/10 |
| [`ts-rs`](https://crates.io/crates/ts-rs) | 12.0.1 | generated DTO output checked in/reviewed | 0/6 |
| [`thiserror`](https://crates.io/crates/thiserror) | 2.0.20 | typed library errors | 0–13 |
| [`anyhow`](https://crates.io/crates/anyhow) | 1.0.104 | binary/application edges only | 0–13 |
| [`tokio`](https://crates.io/crates/tokio) | 1.53.1 | explicit features; blocking work off executor | 0–13 |
| [`tokio-util`](https://crates.io/crates/tokio-util) | 0.7.19 | cancellation tokens | 0–13 |
| [`async-trait`](https://crates.io/crates/async-trait) | 0.1.92 | avoid when native async traits meet object-safety/design needs | 0/10 |
| [`tracing`](https://crates.io/crates/tracing) | 0.1.44 | structured/redacted logs | 0/11 |
| [`tracing-subscriber`](https://crates.io/crates/tracing-subscriber) | 0.3.23 | bounded production appender | 0/11 |
| [`uuid`](https://crates.io/crates/uuid) | 1.26.0 | typed ID wrappers; evaluate v7 feature | 0–2 |
| [`jiff`](https://crates.io/crates/jiff) | 0.2.35 | pre-1.0; IANA/DST fixtures | 0/5/13 |
| [`rusqlite`](https://crates.io/crates/rusqlite) | 0.40.2 | bundled SQLite; dedicated service | 1 |
| [`zstd`](https://crates.io/crates/zstd) | 0.13.3 | bounded compression | 1/11 |
| [`zip`](https://crates.io/crates/zip) | 8.6.0 | newest stable; 9.0.0-pre3 excluded; strict wrapper | 1/11 |
| [`image`](https://crates.io/crates/image) | 0.25.10 | bounded full-stream PNG/JPEG decode only; default features disabled | 1 |
| [`same-file`](https://crates.io/crates/same-file) | 1.0.6 | stable opened-file/path identity checks for private storage | 1 |
| [`rustix`](https://crates.io/crates/rustix) | 1.1.4 | safe effective-user ownership checks; `process` feature only | 1 |
| [`libc`](https://crates.io/crates/libc) | 0.2.189 | Unix `O_NOFOLLOW`/`O_NONBLOCK`; 1.0.0-alpha excluded | 1 |
| [`blake3`](https://crates.io/crates/blake3) | 1.8.7 | content/model hashes, not passwords | 1–4/11 |
| [`prost`](https://crates.io/crates/prost) | 0.14.4 | match `prost-build` and OR-Tools proto contract | 2/3 |
| [`prost-build`](https://crates.io/crates/prost-build) | 0.14.4 | match prost/protoc gate | 2/3 |
| [`reqwest`](https://crates.io/crates/reqwest) | 0.13.4 | Rust-only provider HTTP, strict redirects/timeouts | 10 |
| [`url`](https://crates.io/crates/url) | 2.5.8 | endpoint/resource validation | 10/12 |
| [`keyring`](https://crates.io/crates/keyring) | 4.1.6 | OS store; Linux behavior gate | 10/12 |
| [`semver`](https://crates.io/crates/semver) | 1.0.28 | pack/backend/protocol compatibility | 2/11/13 |
| [`csv`](https://crates.io/crates/csv) | 1.4.0 | bounded streaming imports | 5/9/13 |
| [`clap`](https://crates.io/crates/clap) | 4.6.6 | final CLI name still unresolved | 1/11 |
| [`directories`](https://crates.io/crates/directories) | 6.0.0 | preferred active option | 1 |
| [`directories-next`](https://crates.io/crates/directories-next) | 2.0.0 | archived; do not adopt over `directories` | none |
| [`zeroize`](https://crates.io/crates/zeroize) | 1.9.0 | best-effort secret buffers | 10/11 |
| [`rstar`](https://crates.io/crates/rstar) | 0.13.0 | only after profiling vs custom indexed pairs | 9/13 |
| [`cargo-nextest`](https://crates.io/crates/cargo-nextest) | 0.9.143 | test tool | 0/12 |
| [`cargo-llvm-cov`](https://crates.io/crates/cargo-llvm-cov) | 0.9.0 | coverage tool; LLVM compatibility gate | 0/12 |
| [`proptest`](https://crates.io/crates/proptest) | 1.11.0 | property tests | 0/12 |
| [`insta`](https://crates.io/crates/insta) | 1.48.0 | reviewed semantic snapshots | 0/12 |
| [`cargo-insta`](https://crates.io/crates/cargo-insta) | 1.48.0 | snapshot-review companion | 0/12 |
| [`criterion`](https://crates.io/crates/criterion) | 0.8.2 | benchmarks plus project runner | 0/12 |
| [`tempfile`](https://crates.io/crates/tempfile) | 3.27.0 | isolated tests/staging | 0/12 |
| [`pretty_assertions`](https://crates.io/crates/pretty_assertions) | 1.4.1 | optional readable diffs | 0/12 |
| [`cargo-fuzz`](https://crates.io/crates/cargo-fuzz) | 0.13.2 | separately pinned nightly | 0/12 |
| [`libfuzzer-sys`](https://crates.io/crates/libfuzzer-sys) | 0.4.13 | fuzz harness | 0/12 |
| [`cargo-deny`](https://crates.io/crates/cargo-deny) | 0.20.2 | license/ban/source/advisory gate | 0/11/12 |
| [`cargo-about`](https://crates.io/crates/cargo-about) | 0.9.2 | Rust notices | 0/11/12 |
| [`cargo-audit`](https://crates.io/crates/cargo-audit) | 0.22.2 | additional RustSec signal | 0/11/12 |
| [`cargo-cyclonedx`](https://crates.io/crates/cargo-cyclonedx) | 0.5.9 | SBOM option | 0/11/12 |
| [`pumpkin-solver`](https://crates.io/crates/pumpkin-solver) | 0.5.0 | K.4-gated | 8/13 |
| [`pumpkin-core`](https://crates.io/crates/pumpkin-core) | 0.5.0 | match solver; cooperative termination | 8/13 |
| [`tauri`](https://crates.io/crates/tauri) | 2.11.5 | align through tested Tauri contract, not patch-number assumptions | 0/6/11 |
| [`tauri-plugin-dialog`](https://crates.io/crates/tauri-plugin-dialog) | 2.7.2 | exact desktop pin; its `tauri = "2.10"` requirement admits the selected Tauri 2.11 line | 1/6/11 |
| [`tauri-build`](https://crates.io/crates/tauri-build) | 2.6.3 | direct maximum stable; ignore unrelated prerelease-like newest metadata | 0/6/11 |

### Phase-01 implementation limits and dependency evidence

The following implementation evidence was recorded on **2026-08-29** and controls Phase 01 until the named constants or dependency pins change:

- `apps/desktop/src-tauri/Cargo.toml` pins `tauri = 2.11.5` and `tauri-plugin-dialog = 2.7.2`, and the root `Cargo.lock` resolves that exact pair. The upstream [`dialog-v2.7.2` workspace manifest](https://github.com/tauri-apps/plugins-workspace/blob/dialog-v2.7.2/Cargo.toml) declares `tauri = "2.10"`, whose Cargo-compatible 2.x range admits Tauri 2.11. This is controlling manifest/lock compatibility evidence, not packaged cross-platform dialog execution evidence.
- `.eutheto` remains only the proposed portable extension. The working `optimizer` CLI name, reverse-domain application ID, file association/media type, and other public identities remain unresolved; the implemented bundle formats and limits do not settle them.

`eutheto_export::PORTABLE_LIMITS` is the one production limit set shared by export and untrusted import:

| Boundary | Controlling value |
|---|---:|
| Archive bytes | 64 MiB (`64 * 1024 * 1024`) |
| Total uncompressed bytes | 64 MiB (`64 * 1024 * 1024`) |
| One entry | 16 MiB (`16 * 1024 * 1024`) |
| Entries | 4,096 |
| Compression ratio | 200:1 |
| UTF-8 path bytes | 240 |
| One JSON document | 16 MiB (`16 * 1024 * 1024`) |
| JSON nesting depth | 128 |
| One JSON string | 1 MiB (`1024 * 1024`) |
| Items in one JSON collection | 1,000,000 |

The CLI bounds command-file JSON reads at 16 MiB and bundle reads at the same centralized 64 MiB production archive ceiling. Desktop granted-file reads apply the same 64 MiB ceiling with one-byte overflow detection, require an opened regular file, and reject links and special files. Unix opens use `O_NOFOLLOW | O_NONBLOCK` plus device/inode comparison. Windows resolves the selected path once with `FILE_FLAG_OPEN_REPARSE_POINT`, treats the returned handle as authoritative, rejects a final entry carrying `FILE_ATTRIBUTE_REPARSE_POINT`, and reads that same handle without reopening the path. Intermediate directory reparse points participate in Windows path resolution and are not independently rejected; binding a native-dialog selection to a specific file across later path replacement would require the dialog boundary to return an open handle rather than only a path. The store's default periodic snapshot policy is every 50 committed commands, at most 16 MiB of document JSON, with zstd level 3. Accepted configured snapshot bounds are 1–10,000 commands, 1 byte–64 MiB per document, and zstd levels 1–19. Every SQLite connection runs `foreign_keys = ON` and a 5-second busy timeout, enables WAL plus `synchronous = FULL` during initialization, and uses `BEGIN IMMEDIATE` for mutation, migration, and restore transactions. Private database, WAL, SHM, safety-backup, and containing-directory paths are rejected if they traverse symlinks or Windows reparse points. On Unix they are restricted to the effective user with mode `0600`/`0700`; on Windows the owner SID must already be the current user, LocalSystem, or BUILTIN Administrators, after which inheritance is disabled, the owner is set to the current user, the DACL is replaced with one current-user `FullControl` rule, and that ACL is read back and verified. Existing private files must have one link on Unix and must not report `HardLink` on Windows. Existing ancestors are inspected without link traversal; Unix rejects ambient group/other write unless the sticky bit is set, while Windows rejects non-current-user, non-LocalSystem, non-Administrators effective allow rules granting `DeleteSubdirectoriesAndFiles`, ignoring inheritance-only ACEs that do not apply to the ancestor itself. Unsupported platforms fail closed. The Windows implementation uses non-interactive PowerShell/.NET security APIs and therefore requires Windows PowerShell compatibility. Blocking SQLite work runs through bounded `spawn_blocking` calls, and the application service serializes all store access. On successful startup, the application opens the configured database, verifies or initializes the schema, validates the audit chain plus current materialized document/revision state, verifies snapshots and rolls back only to a valid current-revision snapshot before otherwise failing closed, rebuilds authoritative projections, increments `startup_epoch`, writes `last_clean_shutdown = false`, records the exact `StartupRecoveryOutcome`, and exposes that outcome through application state for one native startup notification before acknowledgment. Graceful shutdown sets `last_clean_shutdown = true`. An unclean marker without corruption is retained in startup state and remains a notice rather than a destructive automatic repair.

Phase-01 portable previews are retained in native memory only, capped at three pending previews and 64 MiB of retained inspected data in total. Ordinary import accepts only `scenario-export`; add/replace restore accepts only `full-backup`; typed portable application settings and every supplemental collision identity are validated before a preview is retained. Apply, stale/conflicting apply, terminal failure, explicit cancellation, and eviction consume the preview without mutation. The sole exception is the first safe replace-library safety-backup failure: that failure retains the same preview and collision choices, exposes only its user-safe reason, and enables a second request with the exact phrase `REPLACE WITHOUT BACKUP`; a prospective phrase is rejected and consumed. Replace preview discloses exact project and supplemental removal scope at the bound library revision. The safety backup is assembled, published without clobbering, reopened, and verified before replacement. Scenario export and full backup are each assembled from one store snapshot, preserve portable capability/semantic/nonsemantic wrapper metadata, selected supplemental JSON, and exact inert asset media-type/redistribution declarations, and reject retained results whose exact scenario revision is not represented. Scenario export scopes retained records/assets to declared references for its one scenario. Both paths structurally omit unsupported or secret-bearing application settings; the public settings boundary accepts only typed `appearance`, `locale`, and `units` values. Every committed create, duplicate, scenario command, lifecycle action, portable apply/removal, and setting mutation publishes a request-correlated post-commit event; lag is retryable, Tauri requests authoritative refresh, and forwarding continues.

The `zip = 8.6.0` reader can collapse duplicate raw filenames before ordinary indexed inspection. The controlling defense is therefore to validate the raw ZIP32 central directory before constructing `ZipArchive`: reject duplicate raw names and duplicate normalized paths, inconsistent EOCD/count/offset/size records, ZIP64, multi-disk, encryption flags/fields, malformed extra fields, and then retain normalized and case-collision checks during bounded entry reads. Dependency-reader behavior alone is not the duplicate-path trust boundary.

Stable harness compilation and fuzz execution are separate evidence. On 2026-08-29, `cargo check --manifest-path crates/eutheto-import/fuzz/Cargo.toml --all-targets` passed, proving that stable Cargo can compile all targets in the nested harness manifest; that check alone does **not** prove a libFuzzer run. Separately, `nix develop .#full -c just fuzz-check` completed successfully with explicit rustup `nightly-2026-08-28` `FUZZ_CARGO`, `FUZZ_RUSTC`, and `FUZZ_RUSTDOC`. With `-seed=0 -jobs=1 -workers=1 -timeout=5 -max_total_time=30 -rss_limit_mb=4096`, `scenario_envelope` completed 145,840 runs in 31 seconds and `bundle` completed 2,644,047 runs in 31 seconds, with no crashes. The scenario target rejects inputs over 64 KiB; the raw-bundle target rejects inputs over 512 KiB and further caps total uncompressed data at 1 MiB, one entry at 256 KiB, entries at 128, compression ratio at 32:1, JSON at 256 KiB/depth 64/string 64 KiB/collection 4,096, and paths at 240 bytes. On 2026-08-31, the same bounded command completed all eight checked-in targets—`scenario_envelope`, `bundle`, `migration_chain`, `bundle_remap`, `planning_ir`, `integer_expression`, `projection`, and `component_graph`—for 30 seconds each against their checked-in seed corpora, with no crashes.

## Complete direct GitHub latest-release ledger

| Repository/tool | Direct latest release | Use/gate | Owner |
|---|---|---|---|
| [google/or-tools](https://github.com/google/or-tools/releases/tag/v9.15) | v9.15 (2026-01-12) | K.3-gated pin | 3/11/12 |
| [ConSol-Lab/Pumpkin](https://github.com/ConSol-Lab/Pumpkin/releases/tag/pumpkin-checker-v0.5.0) | pumpkin-checker-v0.5.0 (2026-08-05) | crates 0.5.0 control adapter; K.4 | 8/13 |
| [ERGO-Code/HiGHS](https://github.com/ERGO-Code/HiGHS/releases/tag/v1.15.1) | v1.15.1 (2026-07-02) | post-MVP discovery; reverify/license/benchmark | 13 |
| [scipopt/scip](https://github.com/scipopt/scip/releases/tag/v10.0.3) | v10.0.3 (2026-07-06) | post-MVP; require >=8.0.3 and exact components | 13 |
| [MiniZinc/libminizinc](https://github.com/MiniZinc/libminizinc/releases/tag/2.10.0) | 2.10.0 (2026-07-23) | research adapter; mixed-license bundle review | 13 |
| [NixOS/nix](https://github.com/NixOS/nix/releases) | **Direct query returned HTTP 404** | unresolved by this inventory; pin Nix through committed flake/installer evidence, never invent a version | 0 |
| [NixOS/nixfmt](https://github.com/NixOS/nixfmt/releases/tag/v1.4.0) | v1.4.0 (2026-07-07) | use `pkgs.nixfmt` attribute after nixpkgs verification | 0 |
| [casey/just](https://github.com/casey/just/releases/tag/1.58.0) | 1.58.0 (2026-08-03) | Phase-0 tool pin | 0 |
| [ninja-build/ninja](https://github.com/ninja-build/ninja/releases/tag/v1.13.2) | v1.13.2 (2025-11-20) | match pinned native build | 0/3 |
| [llvm/llvm-project](https://github.com/llvm/llvm-project/releases/tag/llvmorg-23.1.0) | llvmorg-23.1.0 (2026-08-25) | discovery only; use pinned nixpkgs compiler proven with OR-Tools | 0/3 |
| [protocolbuffers/protobuf](https://github.com/protocolbuffers/protobuf/releases/tag/v36.0) | v36.0 (2026-08-20) | discovery; match OR-Tools instead of blind upgrade | 2/3 |
| [anchore/syft](https://github.com/anchore/syft/releases/tag/v1.51.1) | v1.51.1 (2026-08-27) | exact SBOM tool pin; verify output format | 0/11/12 |
| [sigstore/cosign](https://github.com/sigstore/cosign/releases/tag/v3.1.3) | v3.1.3 (2026-08-06) | signing choice/key model still gated; verify bundle compatibility | 11/12 |
| [slsa-framework/slsa-verifier](https://github.com/slsa-framework/slsa-verifier/releases/tag/v2.7.1) | v2.7.1 (2025-06-27) | exact provenance verifier pin | 0/11/12 |
| [cli/cli](https://github.com/cli/cli/releases/tag/v2.98.0) | v2.98.0 (2026-08-20) | release-tooling pin if adopted | 0/11 |
| [docker/buildx](https://github.com/docker/buildx/releases/tag/v0.36.1) | v0.36.1 (2026-08-04) | Linux release tooling only if adopted | 0/11 |

## Verified corrections and qualifications

| Finding | Controlling resolution | Owner |
|---|---|---|
| Rust 1.98.0 was reported as the pin | Corrected to **1.97.1** until the P-critical issue is fixed by a newer stable and tested | 0/12 |
| Blueprint/reports retained pnpm 10 | Corrected to direct current **pnpm 11.24.0** with Node 24 compatibility; exact Nix/package-manager integrity must be proven | 0 |
| Desktop reports supplied many swapped/stale npm values | Replaced by the complete direct npm table: vue-router 5.3.0, Pinia 4.0.3, Tauri API 2.11.1/CLI 2.11.4/updater 2.10.1/shell 2.3.5, shadcn-vue 2.8.2, Reka 2.10.4, Table 9.2.4, Virtual 3.13.36, ECharts 6.1.0, vue-echarts 8.1.0, ESLint 10.9.1, Vitest 4.1.11, Vue Test Utils 2.5.0 | 0/6/9/11/12 |
| TypeScript report recommended 7.0.2 | Corrected to **6.0.3** because typescript-eslint 8.68.0 excludes TypeScript >=6.1 | 0/12 |
| Pumpkin report said 0.4.0 | Corrected to direct crates.io **0.5.0** | 8/13 |
| Syft reports said 1.51.0 | Corrected to direct GitHub latest **1.51.1** | 0/11/12 |
| protobuf report said 35.1 | Direct current is **36.0**, but neither is automatically selected; match OR-Tools 9.15 | 2/3 |
| `nixfmt-rfc-style` attribute | Use current `pkgs.nixfmt`; old name is deprecated after nixpkgs 25.11. Directory invocation caveat remains; verify check invocation | 0 |
| Linux app-indicator | Legacy `libayatana-appindicator` exists but is obsolete upstream; confirm Tauri requirement or current replacement in locked nixpkgs | 0/6 |
| Tauri capabilities | Custom commands need explicit manifest/capability treatment; broad invoke registration is not sufficient for least privilege | 6/11/12 |
| Tauri updater | Current configuration uses `bundle.createUpdaterArtifacts`; verify signed updater contract and endpoint/key lifecycle | 11/12 |
| WebView2 | Not guaranteed on every clean Windows 10/Server/LTSC image; bootstrap/runtime strategy is a release gate | 11/12 |
| AppImage | Supported but WebKitGTK/linuxdeploy behavior requires exact clean-machine evidence; “AppImage and/or deb” remains deliberate | 11/12 |
| OR-Tools assumption core | v9.14/v9.15 issue #5141 may return a presolve literal outside assumptions; cores are sufficient, not necessarily minimal; pin diagnostic settings and characterize behavior | 3/4/12 |
| Pumpkin cancellation | Cooperative polling can overrun nominal timeout during long propagation; solver is owned on a dedicated thread | 8/12/13 |
| SCIP licensing | Only versions >=8.0.3 are Apache-2.0; exact assembled components still reviewed | 13 |
| MiniZinc licensing | libminizinc MPL-2.0 does not make its bundled solver/UI distribution one license unit | 13 |
| `keyring-rs` link | Canonical repository is [open-source-cooperative/keyring-rs](https://github.com/open-source-cooperative/keyring-rs); Linux Secret Service/fallback UX remains gated | 10/12 |
| `directories-next` | Archived; prefer active `directories` 6.0.0 | 0/1 |
| `async-trait` | Native async traits exist; use crate only where actual dispatch/object-safety design requires it | 0/10 |
| Accessibility package | `@axe-core/vue` does not exist; use `axe-core` directly through the Vue test harness | 12 |
| shadcn/Tailwind 4 | Use `tw-animate-css`; audit Tailwind-4 CSS-variable syntax | 0/6 |
| Balanceframe references | Architecture patterns may be adapted, but repository provenance was not independently discoverable by the report; never make access to it a build requirement | 0 |

## Appendix K gate ledger

Every K item is represented below. “Version selected” does not mean the build/release evidence gate is closed.

### K.1 — Repository initialization

| Gate | Status on 2026-08-29 | Owner/closure evidence |
|---|---|---|
| Final project name | **Closed: eutheto** | Phase 0 workspace/package/docs normalization |
| Rust/npm namespaces and prefixes | **Closed: Rust `eutheto-*`; npm `@eutheto/*`** | Final project-wide namespace decision; Phase 0 normalizes workspace/package manifests |
| CLI executable, reverse-domain app ID, exact crate/package inventory names, portable extension/media types/file associations | **Open; `.eutheto` is the portable extension proposal** | Phase 0 committed names where needed; Phase 11 identity ADR/installer/CLI/docs; Phase 12 cross-platform open/inspect and migration/update compatibility |
| Hosting organization and governance contacts | **Open** | Phase 0/11 public repository, GOVERNANCE/SECURITY contacts |
| Exact Rust | **Selected and Nix-pinned: 1.97.1; four native CI default-shell assertions configured, run evidence pending** | `rust-toolchain.toml`, flake shell, and successful target-suite runs; revisit fixed stable |
| Exact Node/pnpm | **Selected and Nix-pinned: Node 24.20.0 and pnpm 11.24.0; four native CI default-shell assertions configured, run evidence pending** | official fixed-output hashes/package-manager integrity, Nix shell, and successful CI smoke |
| Tauri/Vue/Tailwind/shadcn/Reka/Lucide pins | **Explanation foundation, maintained 2026-09-04:** Tauri 2.11.5, Vue 3.5.42, Tailwind CSS and `@tailwindcss/vite` 4.3.3, shadcn-vue 2.8.2, Reka UI 2.10.4, and `@lucide/vue` 1.39.0; the direct support set is class-variance-authority 0.7.1, clsx 2.1.1, tailwind-merge 3.6.0, and tw-animate-css 1.4.0 | `apps/desktop/package.json`, `pnpm-lock.yaml`, generated license/SBOM inventories; the dated dependency-maintenance entry below supersedes the initial Lucide 1.37.0 pin without changing the Phase-04 component contract; Phase 06 owns any justified extension |
| RustSec exceptions in the locked Tauri Linux graph | **Accepted until 2026-11-30:** sixteen exact unmaintained advisories in Tauri 2.11.5's GTK3/urlpattern transitives have no compatible maintained replacement; `RUSTSEC-2024-0429` is accepted only because the locked dependency sources contain no call to the affected `glib::VariantStrIter` path | `@Eutheto/maintainers`; exact IDs, rationale, and a UTC fail-closed expiry guard in `deny.toml`/`just rust-advisories`; re-review on every Tauri/lock change, any relevant advisory, by the expiry, and before Phase 11/public release |
| Stable/beta IDs | **Open** | Phase 11 updater/signing path continuity |
| Signing/notarization plan and protected environments | **Open** | Phase 11 documented key custody/rotation and protected-workflow evidence |

### K.2 — Nix

| Gate | Status | Owner/closure evidence |
|---|---|---|
| Verify `webkitgtk_4_1`, `libsoup_3`, app indicator, pnpm 11, exact Node 24.20.0, Syft, Cosign, and SLSA verifier | **Node/pnpm derivations and official Node hashes committed; native locked-flake/default-shell CI is configured for all four systems, with run evidence and remaining locked attributes still open**; app indicator qualified obsolete | Phase 0 `flake.lock`, native CI runs, and shells |
| Darwin shell/Xcode tools | **Native Intel and Apple Silicon shell lanes configured; successful run evidence pending** | Phase 0 macOS shell smoke on `macos-15-intel` and `macos-15` |
| nixfmt check invocation | **Configured: `nix flake check --no-update-lock-file` on every native Nix lane; successful run evidence pending** | Phase 0 portable workflow runs |
| Binary-cache plan | **Configured for the public `cache.nixos.org` substituter only; no project cache is claimed and run evidence is pending** | Phase 0 portable workflow and later project-cache ownership/trust decision |
| Fully packaged Linux desktop derivation vs dev/worker/CLI only | **Open** | Phase 0/11 artifact scope decision |

### K.3 — OR-Tools

| Gate | Status | Owner/closure evidence |
|---|---|---|
| Exact release/commit | **Approved bundled backend: v9.15 at `551ad10d94835c99e5e1e684500d3db398c0e345`, semantic build version `9.15.6755`.** Generated `workers/ortools/source-contract.json` binds approval evidence `docs/roadmap/assumptions.md@d74be8614150ad8474dd78f59ffe0dc761fd742f`. Native target builds, installed manifests, licenses, SBOMs, worker behavior, unsigned package smoke, and target loading passed. Phase 04 still owns independent candidate acceptance; Phase 11/12 owns exact signed release artifacts. | Target matrix runs `33577785073`, `33582692427`, `33587339469`, `33590499388`, and `33595985834`; finalized Windows worker run `33674502944`; Phase-03 package run `33749723466` at `b6c0211` |
| Source hash and proto checksums | **Recorded:** OR-Tools raw GitHub tag-archive SHA-256 `6395a00a97ff30af878ee8d7fd5ad0ab1c7844f7219182c6d71acbee1b5f3026` and unpacked Nix hash `sha256-9+tvgP/+/VY6wu7lzTdP4xfiJIgPSLVR9lEdZjQCZkE=`; repository patch SHA-256 `3ab9c8c45d76aab2416195bc97266718986a395a37fcd6c8d6e6fa5322ecf6a6`; protobuf 33.1 official release archive `protobuf-33.1.tar.gz` SHA-256 `fda132cb0c86400381c0af1fe98bd0f775cb566cb247cdcc105e344e00acc30e`; official Windows x64 `protoc-33.1-win64.zip` SHA-256 `d7a207fb6eec0e4b1b6613be3b7d11905375b6fd1147a071116eb8e9f24ac53b` reverified from the release asset digest on 2026-09-02; `cp_model.proto` SHA-256 `c967180600fab5db4fc8b7477fef56c3c6d3c0714b1f0355697f96d147f77d96`; `sat_parameters.proto` SHA-256 `9a5e08486a63414191870bd9953f9e561af221e165654913a33662bf1b674308`.** | Run `33595985834` hash-verified and forced the official protobuf archive into OR-Tools FetchContent on all four targets; Phase 3 derivation, generated-proto, and manifest checks |
| Exact CMake disable flags | **Approved as 26 sorted cross-target cache entries:** Release mode, static OR-Tools, C++ plus dependency install enabled, BOP/GLOP enabled, and language wrappers, examples/samples/tests/docs, FlatZinc, MathOpt, Coin-OR, SCIP, GLPK, HiGHS, Gurobi, CPLEX, Xpress, and PDLP disabled. | Runs `33577785073`, `33590499388`, and `33595985834`; target-specific compiler/generator/install/package paths, verified fetched-source override paths, and `EIGEN_MPL2_ONLY` spelling remain build-owned and must be recorded in each installed manifest |
| CP-SAT worker-thread ceiling | **Confirmed: 10,000** | Pinned OR-Tools 9.15 [`parameters_validation.cc`](https://github.com/google/or-tools/blob/v9.15/ortools/sat/parameters_validation.cc) rejects `num_search_workers` above 10,000; `protocol/version.json` is the runtime authority and may impose a lower product limit later |
| Pre-pin primitive benchmarks | **Passed on every MVP target:** nine outcome-sensitive fixtures cover the Phase 03 Boolean, cardinality, integer-linear, reification/enforcement, bounded penalty/reward objective, and projection encodings under seed 1, one worker, a 2-second per-run budget, and three repetitions. The 768-variable/1,408-constraint mixed fixture proved objective and bound `2688`; raw-solver medians were 7.7–17.6 ms and in-process worker medians were 8.4–19.4 ms. | Run `33582692427` at `d8c4317`; evidence is explicitly prebuilt-`CpModelProto`, in-process, excludes Rust translation/process spawn/full-adapter timing, and makes no product SLA claim |
| Static/dynamic linkage per target | **Selected: static OR-Tools on Linux x86_64, Windows x86_64, macOS arm64, and macOS x86_64.** Against shared OR-Tools, static reduced the measured worker-plus-installed-runtime-library payload by 33.3% on Linux, 23.7% on macOS arm64, and 25.9% on macOS x86_64, with equal or lower median process startup/loading samples and one fewer runtime file. Windows shared compilation fails in the pinned source at generated `assignment.pb.cc` with MSVC C2491; static is the viable mode. | Comparison run `33587339469` at `3a742b4`; selected-policy run `33590499388` at `a54df49`. This is static OR-Tools with the exact dependent-library set, not a claim that the complete worker process has no dynamic dependencies. |
| Target dependent-library loading | **Phase-03 unsigned package-smoke closure confirmed on every MVP target; Phase 11/12 signed release and clean-machine closure remain open.** Windows x86_64, macOS arm64, macOS x86_64, and Linux x86_64 passed architecture/dependency inspection, manifest-bound worker handshake and solve under sanitized runtime lookup, and packaged-app launch. Linux launches the exact deb payload and separately verifies the generated RPM inventory contains one desktop, worker, and trusted solver manifest. | Run `33749723466` at `b6c0211`; no package was uploaded, signed, notarized, or published |
| Exact linked dependency licenses | **Build-artifact closure confirmed:** OR-Tools and Abseil `Apache-2.0`; protobuf `BSD-3-Clause`; bundled utf8_range `MIT`; RE2 `BSD-3-Clause`; zlib `Zlib`; bzip2 permissive `bzip2`. Eigen 3.4.0 is fetched under `EIGEN_MPL2_ONLY` but not compiled or linked with `USE_PDLP=OFF`. No GLPK, GPL/LGPL/AGPL, proprietary solver, or Python, Java, or .NET runtime dependency is compiled or linked. Finalized artifacts install eight source-hashed license files and a deterministic NOTICE; the Windows NOTICE additionally records app-local MSVC runtime redistribution terms. | Candidate run `33577785073` inspected every installed runtime binary plus the worker and native test executable on all four targets; bzip2 resolved exactly to `66c46b8c9436613fd81bc5d03f63a61933a4dcc3`. Finalized Linux and Windows evidence is recorded in the worker-manifest row below; Phase 11/12 still owns exact assembled-bundle review. |
| Worker SBOM/license manifest | **Phase-03 build and unsigned-package gates passed.** Deterministic manifests inventory and hash the worker, runtime closure, license corpus, NOTICE, and SPDX 2.3 SBOM; validation binds source/protobuf/protocol approvals, target-specific CMake evidence, capabilities, linkage, and runtime files. The native package matrix validates the exact bundled resource closure before worker smoke. | Commit `b6c0211`: run `33749723466` passed every MVP target. Earlier local Nix manifest digest `6c85e599895f60efb44583b843b9eb610f147ca5daf30709fd1d57324f889cd7` and Windows manifest digest `f4fb1815a26d21fa087d2733a18144b4da9b5c06be46aa95e2cc2a497e5f2c3f` remain source-stage evidence. Phase 11/12 owns exact signed release-bundle inspection and publication. |
| Assumption/core APIs and callbacks | **Callback gate passed; sufficient-assumption capability remains disabled.** Every selected target emitted one fixed-feasible incumbent, zero infeasible incumbents, three strictly improving optimization incumbents, two strictly improving finite bounds ending at `2.6`, and one callback-stopped incumbent with `Feasible` termination. Issue #5141 remains open and the upstream keep-all workaround still reproduces. | Run `33590499388` at `a54df49`; callback evidence uses the direct OR-Tools API with seed 1 and one worker, and does not claim multi-worker behavior or deterministic worker-protocol event counts. Canonical handshake omits the assumption capability; reject any returned assumption evidence unless a later exact-build gate enables it. |

### Phase-03 closure evidence (2026-09-03)

- Phase-03 implementation evidence head: `b6c0211ebb5be9d929527a50bdeb30c642955538`.
- Canonical locked-Nix validation passed in run `33749724748` at that head. The native unsigned package matrix passed Windows x86_64, macOS arm64, macOS x86_64, and Linux x86_64 in run `33749723466` at the same head.
- Portable native/source-build coverage passed in run `33744017797` at `1489fd2`; deterministic benchmark discovery and execution passed in run `33744015141` at `1489fd2`. The only later implementation change, `b6c0211`, changes Linux package-payload verification in the OR-Tools workflow and does not change either covered workflow or product source.
- Focused local closure checks passed: `cargo test -p eutheto-solver-ortools --locked` (64 tests), `cargo test -p eutheto-protocol --locked` (53 tests), exact extracted Linux package worker smoke, and `just check`.
- All package evidence is unsigned development evidence. No executable, installer, signature, notarization, release artifact, or update metadata was uploaded or published. Phase 04 owns authoritative acceptance and quarantine; Phase 11/12 owns signed exact-release artifacts, clean-machine support claims, and publication.

### Phase-04 closure evidence (2026-09-04)

- Phase-04 core implementation evidence head: `fdd77f3786b5f6e25a3231b64a366578dbd4364a`. The stacked explanation-UI and dependency evidence head is `b179f65fdf4a2a4ede745667611959765d5dd047`.
- Core validation passed canonical locked-Nix run `33879622431`, Portable run `33879622259` on macOS arm64/x86_64, Ubuntu arm64/x86_64, and Windows arm64/x86_64, OR-Tools unsigned package run `33879622266` on macOS arm64/x86_64, Linux x86_64, and Windows x86_64, Security run `33879622338`, and dependency-policy run `33879622267`.
- The composed UI tree passed canonical locked-Nix run `33880458720`, the same six-target Portable matrix in run `33880458283`, the same four-target unsigned worker/package matrix in run `33880458545`, Security run `33880458547`, and dependency-policy run `33880458331`. The candidate-source matrix entry was correctly skipped because neither stack changed the approved OR-Tools source contract.
- Focused closure evidence covered deliberate invalid-candidate quarantine; projection, verifier, and score mutation sensitivity; exhaustive small-model compiler/verifier agreement; valid and invalid assumption mapping; verifier-owned score/category breakdown; acceptance timing; optional-explanation independence; backend-contract verification; metamorphic invariants; counterfactual execution/cancellation; compact evidence round trips; stable CLI/Tauri errors and events; and accepted versus quarantined UI states. Local `just check`, release CLI smoke, and Chromium keyboard/visual smoke passed on the composed product tree.
- DeepReview, security re-review, and UI review reported no remaining blockers. The synthetic conformance pack is excluded from release builds, so this evidence does not claim a production domain or accepted production result. All package evidence is unsigned development evidence; Phase 11/12 still owns signed exact-release artifacts, clean-machine claims, and publication.

### Phase-05 portable-v2 prerequisite local evidence (2026-09-05)

- Local implementation branch: `feat/portable-schema-v2`, based on merged prerequisite PR #20 at `7ee59344c79988bea1cbd41a6165cf9e115f8686`. This is working-tree evidence, not an exact-candidate hosted or protected-main completion claim. Workforce production implementation remains gated on integration.
- Current export writes global portable schema V2 through the owning pack for both current and retained revisions. Outer ZIP format V1, internal document/snapshot V1 and SQLite migration identities/checksums remain unchanged. The synthetic pack's portable payload is V2; its internal schema remains V1. Workforce's approved initial version remains V1 without invented history.
- Actual old/new CLI workflows exercised genuine V1 import, process reopen, V2 export and fresh-library import, full backup, reviewed replace restore, CreateCopy remapping, and restoration of the actual pre-replace safety backup. No-remap paths preserved exact retained-row and canonical archived-result bytes; copy preserved checksum fields while rewriting declared identities. Imported result records remained inert and accepted-result lists stayed empty.
- The retained baseline binary read and exported databases written by the new binary, including subject/version-space provenance from an actual global-V2 import with sequential synthetic portable V0→V1→V2 migration. A deterministic internal-migration fixture separately proves source checksum preservation despite changed domain documents and no promotion into accepted-result APIs. The retained result source uses the existing synthetic store acceptance fixture, not a live backend or production-domain solve.
- `just check` passed on Linux: generation drift (39 products), worker protocol verification, permanent fixture discovery, architecture, DCO self-tests, Rust/frontend formatting and strict linting, frontend type checking, workspace/default-feature Rust and documentation tests, and all 95 frontend tests. `nix flake check --no-update-lock-file` passed the local x86_64-linux checks; other systems were not executed by that command.
- `nix build .#eutheto-cli --no-link --no-update-lock-file --print-out-paths` passed after separating debug-only integration fixture checks from the installed release build. The actual Nix-installed release executable ran `packs list` successfully and returned an empty catalog, confirming that the synthetic conformance pack was not shipped as production authority.
- `just fuzz-harness-check` passed its reviewed inventory check (nine targets, 19 seed files) and stable-toolchain compilation. Focused portable/core regressions cover sequential conversion, scope/version rejection, unchanged historical fixtures, aggregate transformed-input and retained-preview bounds, and atomic failures.
- `just e2e` built the native Tauri application and proved project persistence across independent WebKit application sessions. Chromium visual inspection separately exercised the real import/restore migration-list component with presentation-only fixtures: global steps and distinct pack/scenario/revision/version-space rows remained distinguishable. That visual check does not claim a native migration-dialog workflow.
- Independent implementation and security reviews closed their identified conversion/provenance and resource-accounting findings. Required exact-candidate hosted matrices and protected-main integration remain open; no signed release, production Workforce behavior or public-release compatibility certification is claimed.
- Hosted prerequisite closure (2026-09-05): [PR #21](https://github.com/Eutheto/eutheto/pull/21), head `bb2ebb4e8ff7fb6320c40dd745ecc47c52b63e6e`, passed canonical locked Nix, security/inventories, dependency policy, all six native targets, and all four unsigned packaged-worker targets. The exact protected merge-group candidate `9b0b55ecb622a75b90e2bd97f30c0a223e6c3eb6` then passed [canonical](https://github.com/Eutheto/eutheto/actions/runs/33970228360), [security](https://github.com/Eutheto/eutheto/actions/runs/33970228393), [dependency policy](https://github.com/Eutheto/eutheto/actions/runs/33970228374), [portable](https://github.com/Eutheto/eutheto/actions/runs/33970228361), and [packaged worker](https://github.com/Eutheto/eutheto/actions/runs/33970228434) workflows and became the protected-main merge commit. This closes the portability prerequisite only; Workforce implementation and release signing/publication remain separate gates.

### K.4 — Pumpkin

| Gate | Status | Owner/closure evidence |
|---|---|---|
| Exact version | **Candidate 0.5.0; exact Cargo lock open** | Phase 8 |
| Actual API support matrix | **Open** | Generated capability/primitive tests |
| Cancellation/time limit | **Open; known cooperative polling** | Dedicated-thread cancellation/overrun/crash tests |
| Auto-routing benchmarks | **Open** | Compatible-subset fixed-budget results plus verifier |

### K.5 — Desktop release targets

| Gate | Status | Owner/closure evidence |
|---|---|---|
| Minimum Windows/macOS/Linux versions | **Open** | Phase 11/12 support statement and clean-machine matrix |
| Windows WebView2 strategy | **Open/critical** | Bootstrap/runtime/offline/managed-host tests |
| Wayland/X11 | **Open** | Phase 11/12 Linux manual/E2E |
| AppImage/deb/rpm mix | **Open** | Clean-machine results control |
| macOS x86_64 runner/support | **Tier 2 through the first public MVP:** native `macos-15-intel` source and unsigned-package lanes run for sensitive pull requests, every merge-group candidate, `main`, version tags, manual dispatches, and weekly validation; final signed-package support remains open | Runner availability plus Phase 11/12 signed package smoke; retire when the declared macOS security-support window closes or required tooling no longer supports the target proportionately |
| Updater endpoint/signing-key lifecycle | **Open** | Protected endpoint/key custody/rotation/revocation tests |
| Standalone HTML supported browsers and PDF renderer | **Open**; one-file `file://`, zero-required-network, accessible interaction/list parity and controlled local PDF are required | Phase 07/09 implementation evidence; Phase 11 support statement; Phase 12 exact-candidate browser/platform/print/PDF gate |

### K.6 — AI adapters

| Gate | Status | Owner/closure evidence |
|---|---|---|
| Current official provider APIs | **Mutable/open at implementation and each release** | Phase 10 conformance against official OpenAI Responses/Chat compatibility, Anthropic Messages, and current Gemini contract |
| Tool schema/stream/event formats | **Open** | Recorded provider fixtures, normalization, malformed/stream tests |
| Local OpenAI-compatible differences | **Open** | Capability detection/warnings for strict mode, streaming, parallel calls, endpoint shapes |
| OS keyring on supported desktops/Linux fallback | **Open** | Credential create/read/replace/delete plus absent-daemon UX |
| First-public provider set | **Open** | Enable only adapters meeting maintenance/test quality; AI remains optional |

### K.7 — Domain semantics

| Gate | Status | Owner/closure evidence |
|---|---|---|
| Workforce defaults with practitioners | **Open until review** | Phase 5/12 research evidence |
| Fairness presets/weights | **Open until usability evidence** | Phase 5/7/12 |
| DST, rolling-hours, repair semantics | **Open until domain fixtures/review** | Phase 5/7/12 |
| Seating back-to-back classification | **Open until representative layouts** | Phase 9/12 |
| Accessibility seat metadata without unnecessary sensitive data | **Open until privacy/accessibility review** | Phase 9/12 |
| Non-authoritative legal/regulatory templates | **Permanent invariant; wording review open** | Phase 5/9/11/12 and future packs |

### K.8 — Release readiness

| Gate | Status before Phase 12 | Owner/closure evidence |
|---|---|---|
| Exact dependency licenses from locks/compiled artifacts | **Open** | Phase 11 exact notices/SBOM; Phase 12 review |
| No GPL/proprietary solver linked/bundled | **Open** | Exact binary/dependency inspection and solver manifests |
| Manual accessibility audit | **Open** | Phase 12 automated + keyboard/screen-reader/manual report |
| Clean-machine installer/update/uninstall | **Open** | Every declared target, exact digests |
| Publish source, notices, SBOM, checksums, signatures/attestations, migration notes | **Open until authorized release** | Phase 11 staging; Phase 12 publication/post-publish verification |

### K.9 — Transportation

The proposed official pack ID `official.transportation` is reserved for planning only and is not registered or implemented. Phase 14 is entered from completed Phase 12 independently of Phase 13 completion. Pack compilation and verification remain network-free and provider-neutral; only reviewed Rust application/infrastructure adapters may produce bounded immutable local snapshots. No calendar, routing, traffic, or transit provider is selected by this ledger.

| Gate | Status before Phase 14 implementation | Owner/closure evidence |
|---|---|---|
| ADRs for the external-data boundary, immutable snapshot schemas/versioning, network ownership, persistence/export semantics, and two-stage solve policy | **Open** | Phase 14 T0 approved ADRs and conformance contracts |
| Reference calendar, routing, and transit adapters plus each provider's API, authentication, licensing, rate, caching, retention, persistence, export, redistribution, and attribution terms | **Open; no provider selected** | Phase 14 T0 policy gates and T2/T3/T5 exact-provider reviews with bounded recorded conformance fixtures before an adapter is enabled |
| Least-privilege authentication/scopes, credential custody, revocation, account/tenant separation, and disclosed data access | **Open** | Phase 14 T0 threat model, authorization matrix, credential lifecycle tests, and consent UX |
| Routing capability for historical day/time traffic rather than live-current traffic alone, including provenance, freshness, determinism, and honest fallback labels | **Open** | Phase 14 T3 capability evidence and fixed snapshot fixtures; unsupported estimates are not advertised |
| Transit schedule/realtime data licensing, caching, redistribution, attribution, retention, and derived-snapshot rights | **Open** | Phase 14 T0/T5 exact-feed/provider legal review before transit support is enabled |
| Privacy threat model for calendars, precise locations, movement patterns, household relationships, minors, retention, logs, diagnostics, redaction, deletion, and export | **Open** | Phase 14 T0 security/privacy review and data-flow inventory |
| Proof that no-transit solve followed by opt-in transit fallback is bounded, cancellation-safe, never silently weakens required rules, and independently verifies every candidate accepted at either stage | **Open** | Phase 14 T5 deterministic orchestration tests, authoritative verifier evidence, and one shared budget |
| Practitioner review of terminology, required/default semantics, scoring priorities, transit opt-in, accessibility, explanations, and privacy-safe defaults | **Open** | Phase 14 T1/T7 recorded household/user research and approved default matrix |
| Candidate and benchmark limits for people, vehicles, commitments, locations, travel options, transit alternatives, horizon, solve stages, snapshot size/age, and global time/memory budget | **Open** | Phase 14 T0/T4/T7 published supported envelope, adversarial fixtures, and fixed-budget benchmark evidence |
| Manual entry and offline/stale-snapshot fallback, including explicit freshness/provenance, usable no-account behavior, safe refresh failure, and user-controlled snapshot persistence/export/deletion | **Open** | Phase 14 T2/T3/T6/T7 offline and provider-failure scenarios; core planning remains usable without a provider |

## Additional current specifications and mutable contracts

These verified findings supplement, but do not override, the direct inventory.

| Item | Current finding | Decision/owner | Official evidence |
|---|---|---|---|
| SPDX specification | 3.0.1 stable; 3.1-RC is not production | Phase 11 selects output supported by tooling; Syft may default to SPDX JSON 2.3 | [SPDX specifications](https://spdx.dev/use/specifications/) |
| REUSE specification/tool | Spec 3.3; reported tool 6.2.0 | If adopted, pin tool and make `reuse lint` exact | [REUSE spec](https://reuse.software/spec/) |
| OpenSSF Scorecard | reported CLI 5.5.0/action 2.4.4 | SHA-pin action and least privileges if adopted | [Scorecard releases](https://github.com/ossf/scorecard/releases) |
| OpenAI | `/v1/responses` preferred for new integration; Chat Completions may be needed for local compatibility | Phase 10 supports only tested subsets and normalizes tool calls | [Responses](https://platform.openai.com/docs/api-reference/responses), [function calling](https://platform.openai.com/docs/guides/function-calling) |
| Anthropic | `/v1/messages`, `anthropic-version: 2023-06-01`, tool_use/tool_result | Reconfirm strict-schema/stream fixtures | [Messages](https://docs.anthropic.com/en/api/messages), [tool use](https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/overview) |
| Gemini | report conflict between v1 Interactions and v1beta Interactions remains mutable | Do not freeze narrative value; Phase 10 must use current official endpoint/schema fixtures | [function calling](https://ai.google.dev/gemini-api/docs/function-calling), [Interactions overview](https://ai.google.dev/gemini-api/docs/interactions-overview) |
| Apache-2.0 | approved project license/SPDX identifier | Preserve notices/patent grant and exact artifact obligations | [license](https://www.apache.org/licenses/LICENSE-2.0), [SPDX Apache-2.0](https://spdx.org/licenses/Apache-2.0.html) |
| DCO | approved contribution sign-off | Document `git commit -s`; no CLA initially | [Developer Certificate of Origin](https://developercertificate.org/) |
| Tauri 2 | sidecars, capabilities, updater, WebDriver/signing architecture verified at major-line level | Exact package/config/target behavior remains Phase 6/11/12 tested | [sidecars](https://v2.tauri.app/develop/sidecar/), [capabilities](https://v2.tauri.app/security/capabilities/), [updater](https://v2.tauri.app/plugin/updater/), [WebDriver](https://v2.tauri.app/develop/tests/webdriver/), [macOS signing](https://v2.tauri.app/distribute/sign/macos/) |

## Compatible Vue dependency maintenance — 2026-09-04

- Adopted the compatible version proposals from [Dependabot PR #18](https://github.com/Eutheto/eutheto/pull/18): `@lucide/vue` 1.37.0 → 1.39.0, `vue-component-type-helpers` 3.0.3 → 3.3.11, and `vue-tsc` 3.2.6 → 3.3.11. The lockfile also moves `@vue/language-core` 3.2.6 → 3.3.11; Node 24, TypeScript 6.0.3, and the Rust dependencies remain unchanged.
- Reviewed the [Lucide 1.39.0 release](https://github.com/lucide-icons/lucide/releases/tag/1.39.0) and [Vue language-tools 3.3.11 release](https://github.com/vuejs/language-tools/releases/tag/v3.3.11). Installed package metadata declares ISC for Lucide and MIT for the three Vue tooling packages, with no `preinstall`, `install`, or `postinstall` scripts in these four updated packages. Lucide declares Vue `>=3.0.1`; `vue-tsc` declares TypeScript `>=5.0.0`. These are metadata/compatibility observations, not final shipped-artifact license conclusions.
- Updated exact pins and the lockfile through pnpm with lifecycle scripts disabled; regenerated notices, license inventory, and SBOM through `just licenses sbom`. `just frontend-check frontend-build` passed formatting, lint, type checking, all 93 existing frontend tests, and the production Vite build.
- The reviewed replacement uses a maintainer-authored, DCO-signed commit rather than weakening the author/sign-off identity check for bots. TypeScript 7 remains deferred under the support trigger below; `@types/node` stays on the Node 24 line to match the supported runtime.

## Revalidation triggers

Re-run the relevant direct registry/API and official-contract checks before changing a lockfile/toolchain, starting a gated backend/provider/target branch, cutting beta/RC/stable, or after an upstream security/advisory notice. A revalidation updates this ledger and the owning phase evidence, never silently changes a recommendation. In particular:

- move from Rust 1.97.1 only after a fixed stable exists and passes affected builds/tests;
- move TypeScript to >=6.1 only after the selected typescript-eslint release declares support and lint/typecheck pass;
- change OR-Tools/protobuf only as one tested worker/protocol unit;
- change Pumpkin only with regenerated capability/cancellation/benchmark evidence;
- change provider contracts only with recorded conformance fixtures;
- change target/signing/updater assumptions only with clean-machine and key-lifecycle evidence;
- publish only when every K.8 row is closed against exact artifact digests.
