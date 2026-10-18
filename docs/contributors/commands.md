<!-- SPDX-License-Identifier: Apache-2.0 -->

# Contributor commands

The root `Justfile` is the human command authority. Run `just --list` to inspect the commands in the checkout, and use those recipes rather than reproducing their Cargo, pnpm, Nix, or `xtask` internals in local scripts or CI.

Nix is the canonical Linux environment and supplies the language and native tools on macOS. Native Windows development uses the pinned prerequisites documented for Windows. Entering `nix develop` or activating direnv is intentionally side-effect-free: it does not install packages, fetch dependencies, generate files, run migrations, or build the solver worker. Those operations begin only when an explicit recipe is run.

## First checkout

From the repository root:

```console
nix develop
just bootstrap
just check
```

`just bootstrap` delegates to `just install`. Installation runs `cargo fetch --locked` and `pnpm install --frozen-lockfile --ignore-scripts`; it may populate project-local and tool caches, but it cannot update either lockfile or execute dependency lifecycle scripts. The committed `supportedArchitectures` matrix materializes optional packages for every supported OS/CPU so the security workflow can inspect their manifests. Run `just install` directly when dependencies alone need refreshing.

On a native platform route, first satisfy that platform's documented prerequisites, then run the same `just bootstrap` and applicable portable recipes. Nix-only recipes require Nix.

## Command reference

### Dependencies and generated artifacts

| Recipe | Behavior |
| --- | --- |
| `just bootstrap` | Explicit first-checkout entry point; installs the locked Rust and pnpm dependency graphs. |
| `just install` | Fetches Cargo dependencies with `--locked` and installs pnpm dependencies with `--frozen-lockfile`. |
| `just rust-advisories` | Enforces UTC exception expiry and exact Phase 07 maintenance scope against raw audit findings, then checks locked Cargo advisories, licenses, bans and sources. See the [maintenance policy](../../CONTRIBUTING.md#temporary-phase-07-maintenance-dispositions). |
| `just rust-advisories-test` | Runs offline regression tests for expiry, exact package/source identity and rejection of vulnerability exemptions. |
| `just generate` | Delegates deterministic checked-in artifact generation to `cargo xtask generate`. |
| `just generate-check` | Rejects drift between authoritative inputs and checked-in generated artifacts. |
| `just protocol-check` | Verifies the versioned worker-protocol assets through `xtask`. |
| `just fixtures-check` | Validates repository fixture discovery and filesystem boundaries through `xtask`. |

Never hand-edit a generated product. Change its authoritative input, run `just generate`, inspect the result, and run `just generate-check`.

### Static checks

| Recipe | Behavior |
| --- | --- |
| `just fmt` | Formats the Rust workspace, pnpm workspace, `flake.nix`, and the Nix modules. |
| `just fmt-check` | Checks the same sources without modifying them. The pinned `nixfmt` is invoked with its supported `--check` file form. |
| `just lint` | Runs strict workspace Clippy and the pnpm lint command. |
| `just typecheck` | Runs the pnpm workspace TypeScript checks. |
| `just nix-check` | Runs the lightweight flake checks with `nix flake check`. It is separate from the full suite. |
| `just check` | Runs non-mutating generated-output drift checks through `generate-check`, including the existing checked-in license/SBOM outputs, plus protocol and fixture verification, formatting, linting, type checking, and all Phase-00 tests. It does not regenerate supply-chain files. Deferred worker, packaged E2E, and release assembly gates are deliberately not hidden in this passing Phase-00 suite. |

`nix flake check` validates the lightweight reproducibility contract; it does not replace `just check`.

### Tests, coverage, and benchmarks

| Recipe | Behavior |
| --- | --- |
| `just test-rust` | Runs all Rust workspace targets with locked `cargo test`; it is the default-shell Rust test recipe. |
| `just test-rust-nextest` | Optionally runs the Rust workspace through `cargo nextest` when using the full shell. |
| `just test-doc` | Runs Rust documentation tests. |
| `just test-ui` | Runs pnpm UI unit and component tests. |
| `just test` | Runs the Rust, documentation, and UI test recipes. |
| `just coverage` | Runs the Rust workspace through `cargo llvm-cov nextest` with the full-shell tools. Coverage is diagnostic and does not replace behavioral tests. |
| `just bench` | Builds the exact pinned native OR-Tools worker, runs the checked-in Phase-03 primitive Planning IR corpus through the production adapter under fixed seed/thread/budget settings, writes versioned raw evidence under `.cache/benchmarks/`, and prints it. The evidence is not a product SLA and grants no candidate acceptance. |
| `just e2e` | Runs the Linux native Tauri/WebKit WebDriver path in a separate network namespace, using isolated disposable application directories. `just e2e true` builds and tests the bundled application; `just e2e true false` reuses an unchanged existing bundle. Requires the native drivers, virtual display, and permitted namespace creation. See the [Phase 07 PDF validation handoff](phase07-pdf-validation.md) for current coverage and environment blockers. |

### Applications and builds

| Recipe | Behavior |
| --- | --- |
| `just cli` | Runs `optimizer status`, the real but explicitly non-final Phase-00 CLI behavior. |
| `just ui-dev` | Starts the Vue/Vite development server without the native shell. |
| `just desktop-dev` | Starts the Tauri development shell. Linux launches through the pinned automatic graphics wrapper; macOS and native Windows use their platform-native WebView paths. |
| `just cli-build` | Builds the locked `eutheto-cli` package. |
| `just ui-build` | Type-checks and builds the Vue/Vite frontend through its package script. |
| `just desktop-build` | Builds the native Phase-00 Tauri shell with release bundling disabled. This does not claim installer, updater, signing, or public application-identity readiness. |

The CLI name and desktop identifier used by Phase 00 are development surfaces, not a decision about public naming or registration.

### Solver worker

| Recipe | Behavior in Phase 03 |
| --- | --- |
| `just worker-build-nix` | Builds the approved OR-Tools 9.15.6755 source contract in the Nix sandbox, runs native worker tests, postprocesses the executable/runtime closure, installs the exact reviewed license and NOTICE payload, generates the artifact-specific SPDX SBOM and solver manifest, validates the complete nonsymlink inventory, and verifies installed EOF startup. It does not assemble a Tauri package. |
| `just worker-build-native` | On native Windows x86_64, downloads the fixed-hash source set, builds with MSVC/Ninja and the approved production contract, runs native tests, installs the recursive x64 project DLL closure, rejects unexpected runtime imports, verifies installed EOF startup, finalizes the same license/SBOM/manifest contract, and replaces `.cache/ortools-native/windows-x86_64/current` only after validation. A failed pre-publication rerun preserves the last-good result. It does not create a clean-machine package or release artifact. |
| `just worker-install-from-nix` | Evaluates and builds the exact pinned `.#ortools-worker` output without changing links, requires the current Nix-created `result` symlink to resolve to that output, validates the artifact, then atomically stages its target-suffixed worker and complete manifest/runtime/compliance resources under the ignored Tauri sidecar input directory. It prints the trusted desktop-build wrapper command; this is unsigned packaging input, not a release artifact. |
| `just worker-build-desktop` | Re-evaluates the pinned worker artifact (or builds the validated native Windows artifact), stages it, carries the independently validated manifest digest directly into the Tauri build, disables signing, and fails if staged metadata differs. Phase-03 smoke inputs are explicitly limited to Linux `deb,rpm`, macOS `app`, and Windows `msi`; AppImage rewriting and final release packaging remain later gates. |
| `just worker-smoke <executable> <resource-root> <manifest-sha256>` | Accepts the exact packaged `ortools-worker` executable (`.exe` on Windows), packaged `solver/ortools` resource directory produced by the split Tauri layout, and the trusted manifest digest carried from that desktop build or a release manifest—not recomputed from the package under test. It rejects links, target/architecture/name/hash or manifest-authority drift, incomplete runtime/license/SBOM payloads, unsafe protocol output, diagnostics, and non-clean exit; launches a private validated snapshot of those exact bytes with a cleared environment and only the packaged runtime-library path when required; and completes one bounded manifest-bound handshake plus fixed trivial solve through clean EOF. This proves the unsigned packaged worker bytes launch and negotiate on the current target, not application UI, installer behavior, signing/publication, or Phase-04 candidate verification. |

The Nix build is available on x86_64 Linux and x86_64/aarch64 macOS; the equivalent native build is available only from an x64 Visual Studio developer environment on Windows x86_64. Both builders share the installed-manifest, license, NOTICE, and SPDX finalizer contract. Validated builders stage the external-binary/resource layout consumed by the opt-in desktop `bundled-ortools` build; pass the package's exact `ortools-worker` executable path, `solver/ortools` directory, and independently retained build manifest digest to `worker-smoke`. Final release packaging remains a later gate.

### Supply chain and release

| Recipe | Behavior |
| --- | --- |
| `just licenses` | Explicitly regenerates and validates the Phase-00 Cargo and pnpm license inventory and notices through `xtask`. |
| `just sbom` | Explicitly regenerates the Phase-00 Cargo and pnpm SBOM outputs through `xtask`. |
| `just release-preflight` | Runs the complete Phase-00 check, verifies that tracked release inputs are clean, then fails at release-manifest assembly until the Phase-11 public identity, artifact, and signing gates are resolved. |

Successful `just licenses` or `just sbom` runs prove that the current Phase-00 workspace inventory was regenerated; they do not approve a future solver bundle or release artifact. The non-mutating `just check` path validates these existing outputs through `generate-check` instead. `release-preflight` is intentionally not a successful release command in Phase 00.

## Reporting verification

Record the exact recipes and relevant platform used in a pull request. State only what each run proves: unit tests do not prove packaging, a successful build does not prove runtime behavior, and the Phase-00 shell does not prove deferred solver or release capability.
