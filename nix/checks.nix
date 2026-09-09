{
  pkgs,
  src,
  tooling,
}:
{
  required-files = pkgs.runCommand "eutheto-required-files" { } ''
    set -eu
    for path in \
      .envrc \
      AGENTS.md \
      Cargo.lock \
      Cargo.toml \
      crates/eutheto-protocol/src/generated/eutheto.worker.v1.rs \
      CODE_OF_CONDUCT.md \
      CONTRIBUTING.md \
      DCO.md \
      GOVERNANCE.md \
      Justfile \
      LICENSE \
      NOTICE \
      README.md \
      SECURITY.md \
      THIRD_PARTY_LICENSES \
      THIRD_PARTY_NOTICES.md \
      TRADEMARKS.md \
      flake.lock \
      flake.nix \
      package.json \
      pnpm-lock.yaml \
      pnpm-workspace.yaml \
      protocol/solver-worker.proto \
      protocol/version.json \
      protocol/generated/cpp/protocol-policy.h \
      protocol/generated/cpp/solver-worker.pb.cc \
      protocol/generated/cpp/solver-worker.pb.h \
      protocol/generated/eutheto.worker.v1.descriptor.pb \
      rust-toolchain.toml \
      nix/checks.nix \
      nix/dev-shell.nix \
      nix/dev-shell-welcome.sh \
      nix/eutheto-cli.nix \
      nix/ortools-worker.nix \
      nix/packages.nix \
      nix/release-shell.nix \
      nix/release-tooling.nix \
      nix/source-filter.nix \
      nix/tooling.nix
    do
      if ! test -e "${src}/$path"; then
        echo "required repository path is missing: $path" >&2
        exit 1
      fi
    done
    touch "$out"
  '';

  exact-pins-and-json = pkgs.runCommand "eutheto-exact-pins-and-json" { } ''
    set -eu
    test "$("${tooling.protobuf}/bin/protoc" --version)" = "libprotoc 33.1"
    ${pkgs.python3}/bin/python3 - "${src}" <<'PY'
    import json
    import pathlib
    import sys
    import tomllib

    root = pathlib.Path(sys.argv[1])

    json_files = sorted(root.rglob("*.json"))
    if not json_files:
        raise SystemExit("repository contains no JSON inputs")
    for path in json_files:
        with path.open("rb") as stream:
            json.load(stream)

    with (root / "rust-toolchain.toml").open("rb") as stream:
        toolchain = tomllib.load(stream)["toolchain"]
    assert toolchain["channel"] == "1.97.1"
    assert toolchain["profile"] == "minimal"
    assert sorted(toolchain["components"]) == sorted([
        "cargo",
        "clippy",
        "rust-analyzer",
        "rust-src",
        "rustfmt",
    ])

    with (root / "Cargo.toml").open("rb") as stream:
        cargo = tomllib.load(stream)
    assert cargo["workspace"]["members"] == [
        "crates/eutheto-types",
        "crates/eutheto-domain-ir",
        "crates/eutheto-planning-ir",
        "crates/eutheto-domain-api",
        "crates/eutheto-verify",
        "crates/eutheto-explain",
        "crates/eutheto-solver-api",
        "crates/eutheto-solver-router",
        "crates/eutheto-protocol",
        "crates/eutheto-solver-ortools",
        "crates/eutheto-core",
        "crates/eutheto-store",
        "crates/eutheto-command",
        "crates/eutheto-export",
        "crates/eutheto-import",
        "crates/eutheto-cli",
        "domains/workforce/core",
        "apps/desktop/src-tauri",
        "benchmarks/runner",
        "benchmarks/workforce-runner",
        "xtask",
    ]
    assert cargo["workspace"]["package"]["rust-version"] == "1.97.1"

    with (root / "package.json").open(encoding="utf-8") as stream:
        package = json.load(stream)
    assert package["private"] is True
    assert package["packageManager"] == "pnpm@11.24.0+sha512.bd27e345e976dcb0be0b7a1228217b049a817e21b1f355c90dbe7dc46671895a8bc1e6d06c24554505ea93ea0b45f489a27ec1bfbc8de6a9659fca0f16fa0000"
    assert package["engines"] == {
        "node": ">=24 <25",
        "pnpm": ">=11 <12",
    }
    PY
    touch "$out"
  '';

  envrc = pkgs.runCommand "eutheto-envrc" { } ''
    set -eu
    printf 'use flake .\n' > "$TMPDIR/expected-envrc"
    if ! ${pkgs.diffutils}/bin/cmp -s "$TMPDIR/expected-envrc" "${src}/.envrc"; then
      echo ".envrc must contain exactly: use flake ." >&2
      exit 1
    fi
    touch "$out"
  '';

  legal-baseline = pkgs.runCommand "eutheto-legal-baseline" { } ''
    set -eu
    ${pkgs.python3}/bin/python3 - "${src}" <<'PY'
    import pathlib
    import sys

    root = pathlib.Path(sys.argv[1])
    required = [
        "CODE_OF_CONDUCT.md",
        "CONTRIBUTING.md",
        "DCO.md",
        "GOVERNANCE.md",
        "LICENSE",
        "NOTICE",
        "SECURITY.md",
        "THIRD_PARTY_NOTICES.md",
        "TRADEMARKS.md",
    ]
    for relative in required:
        path = root / relative
        if not path.is_file() or not path.read_text(encoding="utf-8").strip():
            raise SystemExit(f"legal baseline file is missing or empty: {relative}")
    if not (root / "THIRD_PARTY_LICENSES").is_dir():
        raise SystemExit("THIRD_PARTY_LICENSES must be a directory")
    license_text = (root / "LICENSE").read_text(encoding="utf-8")
    if "Apache License" not in license_text or "Version 2.0, January 2004" not in license_text:
        raise SystemExit("LICENSE is not the Apache-2.0 license text")
    dco_text = (root / "DCO.md").read_text(encoding="utf-8")
    if "Developer Certificate of Origin" not in dco_text or "1.1" not in dco_text:
        raise SystemExit("DCO.md does not contain the DCO 1.1 baseline")
    PY
    touch "$out"
  '';

  protocol-contract = pkgs.runCommand "eutheto-protocol-contract" { } ''
    set -eu
    ${pkgs.python3}/bin/python3 - "${src}" <<'PY'
    import json
    import pathlib
    import sys

    root = pathlib.Path(sys.argv[1])
    protocol = root / "protocol"
    with (protocol / "version.json").open(encoding="utf-8") as stream:
        manifest = json.load(stream)
    assert manifest["protocol"] == "eutheto.solver-worker"
    assert manifest["package"] == "eutheto.worker.v1"
    assert manifest["version"] == {"major": 1, "minor": 1}
    assert manifest["applied_parameters_hash"] == {
        "algorithm": "sha256",
        "domain_separator": "eutheto.applied-solve-parameters.v1\0",
    }
    assert manifest["compatibility"]["accepted_protocol_majors"] == [1]
    assert manifest["compatibility"]["unknown_major_action"] == "typed_handshake_error_then_close"
    framing = manifest["framing"]
    assert framing == {
        "length_prefix_bytes": 4,
        "length_prefix_order": "big-endian",
        "min_payload_bytes": 1,
    }
    assert manifest["frame_classes"]["handshake"]["max_payload_bytes"] == 1024 * 1024
    assert manifest["frame_classes"]["solve_request"]["max_payload_bytes"] == 256 * 1024 * 1024
    assert manifest["frame_classes"]["worker_event"]["max_payload_bytes"] == 16 * 1024 * 1024
    assert manifest["limits"]["max_stderr_bytes"] == 4 * 1024 * 1024
    assert manifest["limits"] == {
        "events_per_second": 64,
        "events_per_session": 4096,
        "frames_per_session": 4099,
        "max_nesting_depth": 8,
        "max_repeated_field_items": 100000,
        "max_stderr_bytes": 4 * 1024 * 1024,
        "max_string_bytes": 4096,
        "max_worker_threads": 10000,
        "total_session_bytes": 512 * 1024 * 1024,
    }
    assert manifest["field_limits"]["eutheto.worker.v1.HandshakeRequest.expected_manifest_sha256"] == {
        "max_bytes": 32,
    }

    schema = (protocol / "solver-worker.proto").read_text(encoding="utf-8")
    if 'syntax = "proto3";' not in schema or "package eutheto.worker.v1;" not in schema:
        raise SystemExit("solver-worker.proto does not declare the authoritative v1 protocol package")
    for message in ("ParentFrame", "WorkerFrame", "HandshakeRequest", "SolveRequest", "Finished"):
        if f"message {message} " not in schema:
            raise SystemExit(f"solver-worker.proto is missing {message}")
    generated = protocol / "generated"
    expected_generated = {
        "generated/cpp/protocol-policy.h",
        "generated/cpp/solver-worker.pb.cc",
        "generated/cpp/solver-worker.pb.h",
        "generated/eutheto.worker.v1.descriptor.pb",
    }
    actual_generated = {
        path.relative_to(protocol).as_posix()
        for path in generated.rglob("*")
        if path.is_file()
    }
    if actual_generated != expected_generated:
        raise SystemExit("generated protocol file inventory does not match the authoritative set")
    rust_binding = root / "crates/eutheto-protocol/src/generated/eutheto.worker.v1.rs"
    if not rust_binding.is_file():
        raise SystemExit("generated Rust protocol binding is missing")

    golden = protocol / "golden"
    required = {
        "finished",
        "handshake-error",
        "handshake-request",
        "handshake-response",
        "incumbent",
        "progress",
        "solve-request",
        "started",
        "worker-error",
    }
    json_stems = {path.stem for path in golden.glob("*.json")}
    frame_stems = {
        path.name.removesuffix(".frame.hex")
        for path in golden.glob("*.frame.hex")
    }
    if json_stems != required or frame_stems != required:
        raise SystemExit("protocol golden pair inventory does not match the authoritative v1 sequence")

    for stem in sorted(required):
        with (golden / f"{stem}.json").open(encoding="utf-8") as stream:
            json.load(stream)
        encoded = "".join((golden / f"{stem}.frame.hex").read_text(encoding="ascii").split())
        try:
            frame = bytes.fromhex(encoded)
        except ValueError as error:
            raise SystemExit(f"invalid hexadecimal protocol frame {stem}: {error}") from error
        if len(frame) < 5:
            raise SystemExit(f"protocol frame is too short: {stem}")
        payload_length = int.from_bytes(frame[:4], byteorder="big")
        if payload_length != len(frame) - 4:
            raise SystemExit(f"protocol frame length prefix does not match payload: {stem}")
    PY
    touch "$out"
  '';

  workspace-manifests = pkgs.runCommand "eutheto-workspace-manifests" { } ''
    set -eu
    export CARGO_HOME="$TMPDIR/cargo-home"
    export CARGO_NET_OFFLINE=true
    ${pkgs.cargo}/bin/cargo metadata \
      --format-version 1 \
      --locked \
      --manifest-path "${src}/Cargo.toml" \
      --no-deps \
      > "$TMPDIR/cargo-metadata.json"
    ${pkgs.jq}/bin/jq -e '
      (.workspace_members | length) == 21 and
      ([.packages[].name] | sort) == ([
        "eutheto-cli",
        "eutheto-command",
        "eutheto-core",
        "eutheto-desktop",
        "eutheto-domain-api",
        "eutheto-domain-ir",
        "eutheto-explain",
        "eutheto-export",
        "eutheto-import",
        "eutheto-phase03-benchmark",
        "eutheto-planning-ir",
        "eutheto-protocol",
        "eutheto-solver-api",
        "eutheto-solver-ortools",
        "eutheto-solver-router",
        "eutheto-store",
        "eutheto-types",
        "eutheto-verify",
        "eutheto-workforce",
        "eutheto-workforce-benchmark",
        "xtask"
      ] | sort)
    ' "$TMPDIR/cargo-metadata.json" > /dev/null

    ${pkgs.yq-go}/bin/yq eval -o=json "${src}/pnpm-workspace.yaml" > "$TMPDIR/pnpm-workspace.json"
    ${pkgs.jq}/bin/jq -e '.packages == ["apps/*", "packages/*"]' "$TMPDIR/pnpm-workspace.json" > /dev/null
    ${pkgs.yq-go}/bin/yq eval -o=json "${src}/pnpm-lock.yaml" > "$TMPDIR/pnpm-lock.json"
    ${pkgs.jq}/bin/jq -e 'type == "object" and has("lockfileVersion")' "$TMPDIR/pnpm-lock.json" > /dev/null
    touch "$out"
  '';

  nix-format = pkgs.runCommand "eutheto-nix-format" { } ''
    set -eu
    for path in "${src}/flake.nix" "${src}"/nix/*.nix; do
      ${pkgs.nixfmt}/bin/nixfmt --check "$path"
    done
    touch "$out"
  '';
}
// pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
  desktop-runtime = tooling.desktopRuntime;
}
