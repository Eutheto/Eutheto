# SPDX-License-Identifier: Apache-2.0

set dotenv-load := false
fuzz_toolchain := "nightly-2026-08-28"
fuzz_dir := "crates/eutheto-import/fuzz"
fuzz_artifacts := env_var_or_default("FUZZ_ARTIFACTS", ".cache/fuzz-artifacts")
fuzz_corpus := env_var_or_default("FUZZ_CORPUS", ".cache/fuzz-corpus")
benchmark_artifacts := env_var_or_default("EUTHETO_BENCHMARK_ARTIFACTS", ".cache/benchmarks")
desktop_runtime := if os() == "linux" { "eutheto-desktop-runtime" } else { "" }

# Show the canonical repository commands.
default:
    @just --list

# Fetch and install every locked workspace dependency explicitly.
bootstrap: install

# Install locked Rust and JavaScript dependencies without updating lockfiles.
install:
    cargo fetch --locked
    pnpm install --frozen-lockfile --ignore-scripts

# Regenerate checked-in deterministic artifacts.
generate:
    cargo xtask generate

# Reject checked-in generated artifact drift.
generate-check:
    cargo xtask generate-check

# Verify the versioned worker-protocol assets.
protocol-check:
    cargo xtask protocol verify

# Validate every released Phase-01 fixture root and discovery boundary.
fixtures-check:
    cargo xtask fixtures validate

# Enforce workspace dependency direction and required architecture crates.
architecture-check:
    cargo xtask architecture verify

# Format the Vue/TypeScript frontend.
frontend-format:
    pnpm --filter @eutheto/desktop run format

# Check Vue/TypeScript formatting without changing files.
frontend-format-check:
    pnpm --filter @eutheto/desktop run format:check

# Run the strict frontend linter.
frontend-lint:
    pnpm --filter @eutheto/desktop run lint

# Type-check the Vue/TypeScript frontend.
frontend-typecheck:
    pnpm --filter @eutheto/desktop run typecheck

# Run the frontend unit and component tests.
frontend-test:
    pnpm --filter @eutheto/desktop run test


# Explicitly install the pinned headless Chromium used by browser component tests.
frontend-browser-install:
    pnpm --filter @eutheto/desktop exec playwright install --only-shell chromium

# Exercise real DOM, keyboard, focus, and accessibility behavior in Chromium.
frontend-browser-test:
    pnpm --filter @eutheto/desktop run test:browser
# Build the Vue/Vite frontend.
frontend-build:
    pnpm --filter @eutheto/desktop run build

# Run frontend formatting, lint, type, and Node component checks.
frontend-check: frontend-format-check frontend-lint frontend-typecheck frontend-test

# Format Rust, JavaScript/TypeScript, and Nix sources.
fmt: frontend-format
    cargo fmt --all
    for file in flake.nix nix/*.nix; do nixfmt "$file"; done

# Check Rust, JavaScript/TypeScript, and Nix formatting without changing files.
fmt-check: frontend-format-check
    cargo fmt --all -- --check
    for file in flake.nix nix/*.nix; do nixfmt --check "$file"; done

# Run the strict Rust and JavaScript/TypeScript linters.
lint: frontend-lint
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

# Type-check the JavaScript/TypeScript workspace.
typecheck: frontend-typecheck

# Run the focused Phase-01 core application tests.
test-core:
    cargo test --package eutheto-core --all-targets --locked

# Run the focused Phase-01 SQLite store tests.
test-store:
    cargo test --package eutheto-store --all-targets --locked

# Run the focused Phase-01 portable export/import tests.
test-portable:
    cargo test --package eutheto-export --package eutheto-import --all-targets --locked

# Run the focused Phase-01 CLI tests.
test-cli:
    cargo test --package eutheto-cli --all-targets --locked

# Run the focused Phase-01 native Tauri command tests without packaging.
test-tauri:
    cargo test --package eutheto-desktop --all-targets --locked

# Run the focused Phase-01 core/store/portable/CLI/Tauri test families.
test-phase01: test-core test-store test-portable test-cli test-tauri

# Check released fixture evidence and focused database/portable migration behavior.
migration-check: fixtures-check
    cargo test --package eutheto-store --test store --locked startup_applies_required_pragmas_schema_and_indexes
    cargo test --package eutheto-store --test store --locked newer_database_is_rejected_without_schema_mutation
    cargo test --package eutheto-store --test store --locked migration_failpoint_rolls_back_schema_and_registry
    cargo test --package eutheto-import --lib --locked unknown_newer_and_semantic_capability_are_rejected

# Run the Rust workspace tests with default-shell tools.
test-rust:
    cargo test --workspace --all-targets --locked

# Run the Rust workspace tests through nextest in the full shell.
test-rust-nextest:
    cargo nextest run --workspace --locked

# Run Rust documentation tests.
test-doc:
    cargo test --workspace --doc --locked

# Run the UI unit and component tests.
test-ui: frontend-test

# Run all Phase-01 Rust, documentation, and UI tests.
test: test-rust test-doc test-ui

# Produce the Rust workspace coverage report.
coverage:
    cargo llvm-cov nextest --workspace --locked

# Build the release PDF helper and its exact executable/renderer manifest.
pdf-worker-build:
    cargo xtask pdf build

# Stage the offline renderer independently of the solver sidecar.
pdf-worker-stage-desktop:
    cargo xtask pdf stage-desktop

# Build the pinned, finalized Nix PDF worker artifact.
pdf-worker-build-nix:
    nix build .#pdf-worker

# Verify final extracted package bytes using the independently retained build digest.
pdf-worker-verify executable manifest manifest_sha256:
    cargo xtask pdf verify --executable "{{ executable }}" --manifest "{{ manifest }}" --manifest-sha256 "{{ manifest_sha256 }}"

# Build the pinned Nix OR-Tools worker artifact.
worker-build-nix:
    nix build .#ortools-worker

# Build and verify the approved native Windows x86_64 worker.
worker-build-native:
    cargo xtask solver build-native

# Install the Nix-built worker once its manifest and license payload exist.
worker-install-from-nix:
    cargo xtask solver install-from-nix

# Build an unsigned desktop package with one trusted bundled-worker handoff.
worker-build-desktop:
    cargo xtask solver build-desktop

# Assemble the CLI executable beside its approved worker and manifest-bound resources.
worker-build-cli:
    cargo xtask solver build-cli

# Exercise a relocated CLI image, real file/stored solves, exports, and tamper rejection.
worker-smoke-cli image="target/cli-package":
    cargo xtask solver smoke-cli --image "{{ image }}"

# Validate one manifest-bound packaged OR-Tools worker/resource pair through a trivial solve.
worker-smoke executable resource_root manifest_sha256:
    cargo xtask solver smoke --executable "{{ executable }}" --resource-root "{{ resource_root }}" --manifest-sha256 "{{ manifest_sha256 }}"

# Prove rule contributions and complete Workforce compilation/projection against the approved worker.
workforce-rule-worker-test:
    cargo test -p eutheto-core --test workforce_assignment_rules --locked real_worker_ -- --ignored --test-threads=1


# Reject Workforce generation drift and validate all original-domain fixture semantics.
workforce-corpus-check:
    cargo xtask workforce check

# Generate a bounded synthetic portable scenario with explicit sizes and fixed seed zero.
workforce-corpus-generate people shifts output:
    cargo xtask workforce synthetic --people "{{ people }}" --shifts "{{ shifts }}" --output "{{ output }}"

# Measure real headless Workforce cases against one explicitly trusted worker artifact.
bench-workforce artifact_root manifest_sha256 output:
    cargo xtask workforce run --artifact-root "{{ artifact_root }}" --manifest-sha256 "{{ manifest_sha256 }}" --output "{{ output }}"

# Exercise the same corpus through an already assembled, manifest-bound optimizer image.
workforce-corpus-smoke-cli image manifest_sha256 output:
    cargo xtask workforce smoke-cli --image "{{ image }}" --manifest-sha256 "{{ manifest_sha256 }}" --output "{{ output }}"
# Run the real Phase-01 CLI status command.
cli:
    cargo run --locked --package eutheto-cli -- status

# Run the Vue/Vite development server without the native desktop shell.
ui-dev:
    pnpm --filter @eutheto/desktop run dev

# Run the Tauri development shell.
desktop-dev:
    {{ desktop_runtime }} pnpm --filter @eutheto/desktop run tauri dev

# Build the Phase-01 CLI.
cli-build:
    cargo build --locked --package eutheto-cli

# Build the Vue/Vite frontend.
ui-build: frontend-build

# Build the native Phase-01 desktop shell without release bundles.
desktop-build:
    pnpm --filter @eutheto/desktop run tauri build --no-bundle

# Verify the exact reviewed target and source-corpus contract.
_verify-fuzz-harness:
    python3 {{ fuzz_dir }}/ci/verify_targets.py {{ fuzz_dir }}

# Check the nested fuzz harness with the repository's stable Rust toolchain.
fuzz-harness-check: _verify-fuzz-harness
    cargo check --manifest-path {{ fuzz_dir }}/Cargo.toml --all-targets
# Fail clearly unless every command uses the pinned nightly's explicit binaries.
_require-fuzz-nightly:
    #!/usr/bin/env bash
    set -euo pipefail
    prerequisite="Install {{ fuzz_toolchain }} with rustup, then export FUZZ_CARGO, FUZZ_RUSTC, and FUZZ_RUSTDOC using 'rustup which --toolchain {{ fuzz_toolchain }} <binary>'."
    for variable in FUZZ_CARGO FUZZ_RUSTC FUZZ_RUSTDOC; do
        value="${!variable:-}"
        if [[ -z "$value" ]]; then
            printf 'error: %s is required. %s\n' "$variable" "$prerequisite" >&2
            exit 1
        fi
        if [[ "$value" != /* || ! -x "$value" ]]; then
            printf 'error: %s must be an executable absolute path, got %q. %s\n' "$variable" "$value" "$prerequisite" >&2
            exit 1
        fi
    done
    sysroot=$("$FUZZ_RUSTC" --print sysroot)
    toolchain_directory=${sysroot%/}
    toolchain_directory=${toolchain_directory##*/}
    if [[ "$toolchain_directory" != "{{ fuzz_toolchain }}" && "$toolchain_directory" != "{{ fuzz_toolchain }}-"* ]]; then
        printf 'error: FUZZ_RUSTC must use the {{ fuzz_toolchain }} rustup sysroot, got %q. %s\n' "$sysroot" "$prerequisite" >&2
        exit 1
    fi

# Build every required bounded portable-data fuzz target with the pinned nightly.
fuzz-build: _require-fuzz-nightly _verify-fuzz-harness
    env RUSTC="$FUZZ_RUSTC" RUSTDOC="$FUZZ_RUSTDOC" "$FUZZ_CARGO" fuzz build --fuzz-dir {{ fuzz_dir }}

# Run every reviewed target against scratch copies with deterministic resource bounds.
fuzz-check: fuzz-build
    #!/usr/bin/env bash
    set -euo pipefail
    fuzz_root=$(realpath -m "{{ fuzz_dir }}")
    workspace_root=$(realpath -m ".")
    scratch_corpus=$(realpath -m "{{ fuzz_corpus }}")
    scratch_artifacts=$(realpath -m "{{ fuzz_artifacts }}")
    for scratch in "$scratch_corpus" "$scratch_artifacts"; do
        if [[ "$scratch" == "/" || "$workspace_root" == "$scratch" || "$workspace_root" == "$scratch/"* || "$scratch" == "$fuzz_root" || "$scratch" == "$fuzz_root/"* ]]; then
            printf 'error: fuzz scratch paths must be safely outside %s, got %s\n' "$fuzz_root" "$scratch" >&2
            exit 1
        fi
    done
    targets=(scenario_envelope bundle migration_chain bundle_remap planning_ir integer_expression projection component_graph worker_frame)
    if [[ "$scratch_corpus" == "$scratch_artifacts" || "$scratch_corpus" == "$scratch_artifacts/"* || "$scratch_artifacts" == "$scratch_corpus/"* ]]; then
        printf 'error: fuzz corpus and artifact scratch paths must not overlap\n' >&2
        exit 1
    fi
    rm -rf "$scratch_corpus" "$scratch_artifacts"
    for target in "${targets[@]}"; do
        mkdir -p "$scratch_artifacts/$target" "$scratch_corpus/$target"
        cp -R "{{ fuzz_dir }}/corpus/$target/." "$scratch_corpus/$target/"
        env RUSTC="$FUZZ_RUSTC" RUSTDOC="$FUZZ_RUSTDOC" "$FUZZ_CARGO" fuzz run --fuzz-dir {{ fuzz_dir }} "$target" "$scratch_corpus/$target" -- -seed=0 -jobs=1 -workers=1 -timeout=5 -max_total_time=30 -rss_limit_mb=4096 -artifact_prefix="$scratch_artifacts/$target/"
    done

# Run Phase03, authoritative Workforce and packaged CLI evidence against one worker artifact.
bench:
    #!/usr/bin/env bash
    set -euo pipefail
    just workforce-corpus-check
    artifact_root="$(nix build --no-update-lock-file --no-link --print-out-paths .#ortools-worker)"
    if [[ ! -d "$artifact_root" ]]; then
        printf 'error: expected one native OR-Tools worker artifact, got %q\n' "$artifact_root" >&2
        exit 1
    fi
    manifest="$artifact_root/solver-manifest.json"
    digest_line="$(sha256sum "$manifest")"
    manifest_sha256="${digest_line%% *}"
    output_root="$(realpath -m "{{ benchmark_artifacts }}")"
    mkdir -p "$output_root"
    run_root="$(mktemp -d "$output_root/run-XXXXXXXX")"
    output="$run_root/phase03-primitives.json"
    cargo run --locked --release --package eutheto-phase03-benchmark -- \
        --artifact-root "$artifact_root" \
        --manifest-sha256 "$manifest_sha256" \
        --corpus benchmarks/corpus/solver/phase03-primitives.json \
        --output "$output"
    just bench-workforce "$artifact_root" "$manifest_sha256" "$run_root/workforce.json"
    cargo xtask solver build-cli --artifact-root "$artifact_root" --manifest-sha256 "$manifest_sha256"
    just workforce-corpus-smoke-cli target/cli-package "$manifest_sha256" "$run_root/workforce-cli.json"
    printf 'Benchmark evidence: %s\n' "$run_root"

# Build and exercise real Linux desktop persistence through WebKit WebDriver.
# `just e2e true` tests the unsigned, manifest-bound OR-Tools desktop bundle.
# `EUTHETO_E2E_CONFLICT_ONLY=1 just e2e true` runs the complete isolated conflict/copy/export flow.
# `EUTHETO_E2E_CONDITIONAL_ONLY=1 just e2e true` proves saved replacement pins, copy, Undo and Redo.
# `EUTHETO_E2E_ALTERNATIVES_ONLY=1 just e2e true` exercises sequential profiles and retained recovery.
# `EUTHETO_E2E_LARGE_RESULT_ONLY=1 just e2e true` solves large-supported with the bundled worker, verifies the result, and measures keyboard view/page/calendar transitions.
# `EUTHETO_E2E_RECIPIENT_EXPORT_ONLY=1 just e2e true` tests keyboard-operated accepted CSV/share reviews before publication checks.
# `EUTHETO_E2E_KEYBOARD_WORKFLOW_ONLY=1 just e2e true` creates, configures, solves, repairs, compares, and reviews export for a fictional project using UI keyboard actions.
# `EUTHETO_E2E_EXTERNAL_PLAN_ONLY=1 just e2e true` exercises unverified source editing, Repair and recovery.
# `EUTHETO_E2E_ASSIGNMENT_CSV_ONLY=1 just e2e true` exercises native assignment import, reports and recovery.
# `EUTHETO_E2E_ELIGIBILITY_CSV_ONLY=1 just e2e true` exercises sparse eligibility import and recovery.
# `EUTHETO_E2E_ELIGIBILITY_MATRIX_ONLY=1 just e2e true` exercises real pasted-matrix native review, reports and recovery.
# `EUTHETO_E2E_SHIFT_CSV_ONLY=1 just e2e true` exercises native shift/time mapping, reports and recovery.
# `EUTHETO_E2E_AVAILABILITY_CSV_ONLY=1 just e2e true` exercises native availability/time-off import and recovery.
# Set rebuild=false only to rerun against an existing artifact when application inputs are unchanged.
e2e bundled="false" rebuild="true":
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "$(uname -s)" != "Linux" ]]; then
      printf 'error: desktop WebDriver E2E is supported on Linux only\n' >&2
      exit 1
    fi
    if [[ "{{ bundled }}" != "true" && "{{ bundled }}" != "false" ]]; then
      printf 'error: e2e bundled must be true or false\n' >&2
      exit 1
    fi
    if [[ "{{ rebuild }}" != "true" && "{{ rebuild }}" != "false" ]]; then
      printf 'error: e2e rebuild must be true or false\n' >&2
      exit 1
    fi
    if [[ "{{ bundled }}" == "true" && "{{ rebuild }}" == "true" ]]; then
      just worker-build-desktop
    fi


    e2e_root="$(mktemp -d "${TMPDIR:-/tmp}/eutheto-e2e.XXXXXX")"
    trap 'rm -rf -- "$e2e_root"' EXIT
    export XDG_CACHE_HOME="$e2e_root/cache"
    export XDG_CONFIG_HOME="$e2e_root/config"
    export XDG_DATA_HOME="$e2e_root/data"
    export XDG_RUNTIME_DIR="$e2e_root/runtime"
    export XDG_STATE_HOME="$e2e_root/state"
    mkdir -p "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR" "$XDG_STATE_HOME"
    chmod 700 "$XDG_RUNTIME_DIR"

    if [[ "{{ bundled }}" == "true" ]]; then
      shopt -s nullglob
      bundles=(.cache/cargo-target/release/bundle/deb/*.deb)
      if [[ ${#bundles[@]} -ne 1 ]]; then
        printf 'error: expected exactly one unsigned desktop DEB bundle\n' >&2
        exit 1
      fi
      7z x -bd -y "-o$e2e_root/archive" "${bundles[0]}"
      mkdir -p "$e2e_root/application"
      tar -xf "$e2e_root/archive/data.tar" -C "$e2e_root/application"
      if [[ ! -x "$e2e_root/application/usr/bin/eutheto-desktop" ||
            ! -x "$e2e_root/application/usr/bin/ortools-worker" ||
            ! -x "$e2e_root/application/usr/bin/eutheto-pdf-worker" ]]; then
        printf 'error: unsigned desktop bundle is missing its application, solver, or PDF renderer\n' >&2
        exit 1
      fi
      mapfile -d '' -t pdf_manifests < <(find "$e2e_root/application" -type f -path '*/renderer/pdf/eutheto-pdf-worker.json' -print0)
      if [[ ${#pdf_manifests[@]} -ne 1 ]]; then
        printf 'error: expected exactly one packaged PDF renderer manifest\n' >&2
        exit 1
      fi
      pdf_digest_line="$(sha256sum apps/desktop/src-tauri/pdf-sidecar/resources/eutheto-pdf-worker.json)"
      cargo xtask pdf verify \
        --executable "$e2e_root/application/usr/bin/eutheto-pdf-worker" \
        --manifest "${pdf_manifests[0]}" \
        --manifest-sha256 "${pdf_digest_line%% *}"
      export EUTHETO_E2E_APPLICATION="$e2e_root/application/usr/bin/eutheto-desktop"
      export EUTHETO_E2E_PACKAGED_SOLVE=1
    elif [[ "{{ rebuild }}" == "true" ]]; then
      pnpm --filter @eutheto/desktop run tauri build --debug --no-bundle
    fi
    e2e_command=(
      "$(command -v env)"
      "EUTHETO_TAURI_DRIVER=$(command -v tauri-driver)"
      "EUTHETO_NATIVE_DRIVER=$(command -v WebKitWebDriver)"
      "EUTHETO_XDOTOOL=$(command -v xdotool)"
      "$(command -v xvfb-run)" -a --server-args="-screen 0 1600x1200x24"
      "$(command -v eutheto-desktop-runtime)"
      "$(command -v pnpm)" --filter @eutheto/desktop run e2e
    )
    unshare_bin="$(command -v unshare)"
    bash_bin="$(command -v bash)"
    ip_bin="$(command -v ip)"
    if [[ "${EUTHETO_E2E_PRIVILEGED_NETNS:-0}" == "1" ]]; then
      sudo_bin="$(command -v sudo)"
      setpriv_bin="$(command -v setpriv)"
      "$sudo_bin" -E "$unshare_bin" --net -- \
        "$bash_bin" -ceu \
          '"$1" link set lo up; exec "$2" --reuid "$SUDO_UID" --regid "$SUDO_GID" --clear-groups --no-new-privs --bounding-set=-all --inh-caps=-all --ambient-caps=-all -- "${@:3}"' \
          bash "$ip_bin" "$setpriv_bin" "${e2e_command[@]}"
    else
      "$unshare_bin" --user --map-root-user --net -- \
        "$bash_bin" -ceu '"$1" link set lo up; shift; exec "$@"' \
          bash "$ip_bin" "${e2e_command[@]}"
    fi

# Enforce Rust advisory policy, including the reviewed deny.toml exceptions.
rust-advisories:
    python3 -c 'from datetime import date, datetime, timezone; import sys; expiry = date.fromisoformat("2026-11-30"); today = datetime.now(timezone.utc).date(); sys.exit(f"RustSec exception review expired on {expiry}; update or remove each exception") if today > expiry else None'
    python3 scripts/check_rust_advisories.py
    cargo deny --locked check advisories licenses bans sources

# Exercise maintenance expiry, exact package scope, and vulnerability rejection.
rust-advisories-test:
    python3 tests/security/test_rust_advisories.py

# Generate and validate the Phase-01 third-party license inventory.
licenses:
    cargo xtask licenses generate

# Generate the Phase-01 software bill of materials.
sbom:
    cargo xtask sbom generate

# Evaluate all lightweight pinned Nix checks.
nix-check:
    nix flake check

# Exercise accepted and rejected repository DCO samples.
dco-self-test:
    python3 scripts/check_dco.py --self-test


# Exercise actual CI target selection and fail-closed terminal gates.
ci-policy-test:
    python3 tests/security/test_ci_policy.py
# Run the complete non-deferred Phase-01 repository suite.
check: generate-check protocol-check fixtures-check architecture-check dco-self-test ci-policy-test rust-advisories-test fmt-check lint typecheck test

# Run real clean-tree checks, then stop at the unresolved Phase-11 release gate.
release-preflight: check
    cargo xtask release verify-clean
    cargo xtask release assemble-manifest
