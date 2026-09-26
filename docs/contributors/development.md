<!-- SPDX-License-Identifier: Apache-2.0 -->

# Development Environment

The repository has two supported environment routes:

- **Linux and macOS:** the locked Nix flake is the toolchain authority. `direnv` is optional.
- **Native Windows:** use the documented native prerequisites and the read-only verifier in [`windows.md`](windows.md). Nix under WSL is useful only for the core/CLI route; it is not Windows desktop evidence.

The [`Justfile`](../../Justfile) is the human command authority. Run its recipes rather than copying the underlying Cargo, pnpm, Nix, or `xtask` command into automation. `xtask` owns deterministic generation, protocol/fixture work, licenses, SBOMs, hashes, and release manifests.

## Nix with direnv

Install Nix with flakes enabled and install `direnv` through the method approved for your workstation. From the repository root, authorize the committed environment once:

```console
direnv allow
```

The `.envrc` contains only `use flake .`, so entering the directory selects `devShells.default`. Shell entry sets paths, redirects development caches, and prints suggested `just` recipes. It does **not** install JS dependencies, fetch Cargo dependencies, generate sources, run migrations, build a worker, or write source output.

After the shell has entered, perform the explicit locked bootstrap and then run the aggregate Phase-00 check:

```console
just bootstrap
just check
```

`just bootstrap` is the explicit network/write boundary for locked dependency preparation. Review unexpected lockfile changes rather than accepting them. `just check` covers implemented Phase-00 checks; it does not convert a deferred solver, packaged E2E, or release gate into a successful no-op.

If `.envrc` changes, inspect the diff before authorizing it again. Do not place secrets or machine-specific exports in `.envrc`.

## Nix without direnv

Enter exactly the same default shell directly:

```console
nix develop
```

You may also run one canonical recipe without keeping an interactive shell open:

```console
nix develop --command just bootstrap
nix develop --command just check
```

The default shell is sufficient for `just check`. Its `just test-rust` recipe uses locked `cargo test`, so the default check does not require nextest. Use the full shell only for optional additional Rust quality recipes such as `just test-rust-nextest` and `just coverage`:

```console
nix develop .#full
```

### macOS coverage exception

The locked `nixos-25.11` package set exposes `cargo-llvm-cov-0.6.20` but marks it broken on both `x86_64-darwin` and `aarch64-darwin`. The flake therefore omits only `cargo-llvm-cov` from both macOS full and release shells; all release-specific tools and all other quality tools remain present, and the default shell is unchanged. Both Linux full shells retain the Nix package. In particular, the Linux full shell used by CI still provides `cargo llvm-cov`, so Linux CI can run the canonical `just coverage` recipe.

On a native Mac, first complete the [Xcode and SDK prerequisite checks](macos.md#xcode-and-sdk-prerequisite). To run coverage locally despite the Nix package exception, enter the full shell, install the selected `cargo-llvm-cov` 0.9.0 into the shell's repository-local Cargo home, expose that manual install for the current shell, and use the canonical recipe:

```console
nix develop .#full
cargo install cargo-llvm-cov --version 0.9.0 --locked
export PATH="$CARGO_HOME/bin:$PATH"
just coverage
```

This is an explicit native workaround, not a shell hook, a flake-provided package, or evidence of four-system tool parity. The [assumptions ledger](../roadmap/assumptions.md#cargo-llvm-cov-availability-in-locked-nixpkgs) records the version gap and the condition for removing the exception.

The release shell is deliberately separate:

```console
nix develop .#release
```

It adds release-only inventory and signing tools, but entering it still performs no release, signing, upload, install, or dependency fetch. Signing identities and secrets are not shell variables or Nix inputs.

## Pinned Node and pnpm

The Nix shells provide Node **24.20.0** and pnpm **11.24.0** on `x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, and `aarch64-darwin`. The compatible nixpkgs line currently lags the required Node patch, so [`nix/tooling.nix`](../../nix/tooling.nix) selects the matching official `node-v24.20.0-<platform>.tar.xz` fixed-output input for each system. Its SHA-256 values and re-verification source are recorded in the [assumptions ledger](../roadmap/assumptions.md#node-24200-nix-provenance).

Linux builds use Nix's ELF patching hook and Nix-store runtime libraries; Darwin keeps the official archive's native system linkage. The packaged `npm`, `npx`, and `corepack` launch through that same store-pinned Node, and the pnpm wrapper also names that exact Node derivation. None of these tools falls back to a global Node installation.

Fetching and patching happen while Nix realizes the derivation, not in `shellHook`. Entering an already-realized shell therefore has no Node/Corepack/pnpm download or install side effect; explicit dependency fetching remains the responsibility of `just bootstrap`.

## Native Nix CI matrix

The portable workflow is configured to realize the locked flake and default shell independently on the four supported Nix systems:

| Nix system | GitHub-hosted runner | Phase-00 shell check |
|---|---|---|
| `x86_64-linux` | `ubuntu-24.04` | `just check` |
| `aarch64-linux` | `ubuntu-24.04-arm` | `just check` |
| `x86_64-darwin` | `macos-15-intel` | `just generate-check`, followed by the native core, CLI, UI, and Tauri source-build checks |
| `aarch64-darwin` | `macos-15` | `just generate-check`, followed by the native core, CLI, UI, and Tauri source-build checks |

Each lane first checks its native architecture and Nix system, runs `nix flake check --no-update-lock-file`, and enters `devShells.default` to assert Rust **1.97.1**, Node **24.20.0**, and pnpm **11.24.0** exactly. The macOS lanes use the narrower non-mutating generated-contract check because of hosted-runner capacity; all lanes still run the applicable native source tests, unbundled Tauri build, and launch smoke from that same job. The workflow's `setup-node` and `rustup` steps are restricted to native Windows and are not authority for any Nix lane.

Ordinary pull requests run the path-relevant Tier-1 matrix and defer the two `macos-15-intel` source/package lanes. A pull request runs those Intel lanes immediately when it changes native desktop packaging, Nix or Cargo platform configuration, the worker build, the Rust toolchain, or either target-selection workflow. The Intel lanes always run for merge-group candidates, pushes to `main`, version-tag candidates, manual dispatches, and the staggered weekly schedules. Stable Portable and OR-Tools gate jobs turn skipped irrelevant matrices into explicit successful checks and require every selected matrix job otherwise; the protected `main` merge queue requires those gates before integration.

For PRs targeting exactly `phase/06-desktop`, the user-authorized [temporary integration trial](git-workflow.md#temporary-phase06-integration) instead runs source/security/dependency checks on every package, defers native/package matrices only for the documented source-only path allowance, and requires full exact-phase checkpoints at the four named package boundaries. All other PR and non-PR policies above remain unchanged. An explicitly deferred policy gate is not native or packaged-artifact evidence.

This is configured CI coverage, not a claim that a workflow run has completed. A queued, unavailable, or failed hosted-runner lane remains an explicit evidence gap for that architecture. Successful flake evaluation and shell entry prove the pinned environment on that runner, while the unbundled Tauri build remains source-build evidence rather than installer, signed-package, notarization, or clean-machine launch evidence.

## Caches and fallback behavior

The development shell redirects writable tool caches into the checkout:

| Tool | Repository-local location |
|---|---|
| Cargo registry/config | `.cache/cargo` |
| Cargo build output | `.cache/cargo-target` |
| Native Windows worker output | `.cache/ortools-native/windows-x86_64/current` |
| Corepack | `.cache/corepack` |
| npm | `.cache/npm` |
| pnpm store/home | `.cache/pnpm/store` and `.cache/pnpm/home` |
| general XDG cache | `.cache` |

Shell entry only exports these paths. Directories and downloads appear when an explicit command such as `just bootstrap` uses them. They are disposable local state and must not be committed.

No project binary cache is claimed in Phase 00. Nix first uses the substituters configured by the contributor or CI. On a cache miss it builds the locked derivation locally; it must not fall back to an ambient compiler, Node, pnpm, or unpinned package. If the network and required Nix-store/local-cache entries are both unavailable, the explicit command fails. Restore access or pre-populate the approved store/cache; do not work around the failure with globally installed floating tools. Cache credentials, if an organization later provisions any, must stay outside the repository and Nix derivations.

## Common commands

The repository command set includes:

```console
just fmt-check
just lint
just typecheck
just test-rust
just test-rust-nextest
just test-doc
just test-ui
just test
just generate-check
just licenses
just sbom
just nix-check
just worker-build-nix
just worker-build-native
just check
```

`just check` calls the non-mutating `generate-check` recipe, which validates the existing checked-in generated outputs, including license and SBOM files, without regenerating them. Use `just generate` only after changing an authoritative generator input; generated files are never hand-edited. Run `just licenses` or `just sbom` when you explicitly intend to regenerate the corresponding supply-chain outputs. The optional `just test-rust-nextest` and `just coverage` recipes require the full shell; the default `just test-rust` recipe does not.

`just worker-build-nix` builds the approved source contract on x86_64 Linux and x86_64/aarch64 macOS. `just worker-build-native` requires native Windows x86_64 plus an activated x64 Visual Studio developer environment; it performs the equivalent fixed-source MSVC/Ninja build with the dynamic `MultiThreadedDLL` runtime required across the shared-Protobuf boundary, copies the transitively imported app-local x64 MSVC runtime files, finalizes the exact license/NOTICE/SPDX/manifest payload, and replaces `.cache/ortools-native/windows-x86_64/current` only after staging validation while preserving the last-good result on earlier failure. Both commands prove only their buildable worker artifact, native tests, runtime-dependency inspection, installed EOF smoke, and build-level evidence payload. Manifest-validated installation, packaged E2E, backend availability, and release preflight still fail closed until their later work packages.

## Core-only route

A contributor who is not working on the desktop boundary can use the Nix default shell on Linux/macOS, or Nix in WSL, and stay on the implemented core/CLI path:

```console
just bootstrap
just cli-build
just cli
```

This exercises the current internal working CLI without claiming the unresolved public CLI name, installer, sidecar, or desktop runtime. WSL results can support portable Rust/core work, but they do not prove native Windows MSVC behavior, WebView2, desktop launch, sidecars, installers, or signing.

The aggregate Rust workspace contains the Tauri crate, so `just test-rust` may require the platform's desktop development libraries. Use the core-only route above when those native desktop prerequisites are intentionally out of scope; CI remains responsible for the full matrix.

## Platform notes

- [Linux](linux.md): WebKitGTK, Wayland/X11, and virtual-display diagnostics.
- [macOS](macos.md): native Xcode/SDK requirements and runner architecture rules.
- [Windows](windows.md): native tool authority, verifier behavior, and the WebView2 clean-machine gate.

A successful shell entry proves only that the selected development environment was constructed. Report only commands and native surfaces actually exercised.