#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Read-only checks of repository-local agent profiles and pinned Ponytail inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = "scripts/codex-cloud/ponytail-manifest.json"
PIN = "16352ac229eb08a9598dcae48fe9f2b342567d86"
ROLES = {
    "planning": "gpt-6-astra",
    "implementation": "gpt-6.1-sol",
    "correctness_review": "gpt-6.1-sol",
    "complexity_review": "gpt-6.1-sol",
}
SKILLS = ("ponytail", "ponytail-audit", "ponytail-debt", "ponytail-gain",
          "ponytail-help", "ponytail-review")
INPUTS = {f".agents/skills/{name}/SKILL.md" for name in SKILLS} | {
    ".agents/vendor/ponytail/LICENSE",
    ".agents/vendor/ponytail/README.md",
    ".agents/vendor/ponytail/benchmarks/agentic/README.md",
    ".agents/vendor/ponytail/benchmarks/results/2026-10-07-agentic.md",
}
POLICY = (
    "At most two subagents may be active for this project in addition to the lead.",
    "Do not start another worker while at the cap.",
    "Use Ponytail Full for each task.",
    "Preserve project instructions, approval gates, security, data integrity, and accessibility.",
    "Never merge to main.",
    "Do not push or open a PR without parent approval.",
)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def validate(root):
    """Return static errors; never install, execute tools, or inspect user homes."""
    root = Path(root)
    errors = []
    role_dir = root / ".codex/agents"
    expected_roles = {f"{role}.toml" for role in ROLES}
    if {path.name for path in role_dir.glob("*.toml")} != expected_roles:
        errors.append(".codex/agents: expected exactly the four named role TOMLs")
    for role, model in ROLES.items():
        path = role_dir / f"{role}.toml"
        try:
            if path.is_symlink():
                raise ValueError("role must be a regular repository file, not a symlink")
            data = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, ValueError) as exc:
            errors.append(f".codex/agents/{role}.toml: {exc}")
            continue
        for key, expected in (("name", role), ("model", model),
                              ("model_reasoning_effort", "high")):
            if data.get(key) != expected:
                errors.append(f"{role}: {key} must be {expected!r}")
        for key in ("description", "developer_instructions"):
            if not isinstance(data.get(key), str) or not data[key].strip():
                errors.append(f"{role}: {key} must be a nonempty string")
        instructions = data.get("developer_instructions", "")
        if isinstance(instructions, str):
            for directive in POLICY:
                if directive not in instructions:
                    errors.append(f"{role}: missing policy directive {directive!r}")
            if role == "planning" and (
                "Do not perform routine implementation or packaging" not in instructions
                or "gpt-6.1-sol/high implementation worker" not in instructions
            ):
                errors.append("planning: missing explicit Sol/high implementation handoff")

    try:
        manifest_path = root / MANIFEST
        if manifest_path.is_symlink():
            raise ValueError("manifest must not be a symlink")
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"),
                              object_pairs_hook=unique_object)
        if not isinstance(manifest, dict) or set(manifest) != {"version", "commit", "sha256"}:
            raise ValueError("expected version, commit, and sha256 fields")
        if manifest["version"] != "5.3.2" or manifest["commit"] != PIN:
            raise ValueError("expected Ponytail 5.3.2 at the fixed upstream commit")
        hashes = manifest["sha256"]
        if not isinstance(hashes, dict) or set(hashes) != INPUTS:
            raise ValueError("expected exactly six skill and four supporting input hashes")
        if any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value)
               for value in hashes.values()):
            raise ValueError("sha256 values must be 64 lowercase hexadecimal characters")
    except (OSError, UnicodeError, ValueError) as exc:
        errors.append(f"{MANIFEST}: {exc}")
        return errors

    actual = {path.relative_to(root).as_posix()
              for directory in (".agents/skills", ".agents/vendor/ponytail")
              for path in (root / directory).rglob("*") if path.is_file()}
    if actual != INPUTS:
        errors.append("Ponytail input inventory differs: "
                      f"missing={sorted(INPUTS - actual)}, extra={sorted(actual - INPUTS)}")
    for relative, expected in sorted(hashes.items()):
        path = root / relative
        try:
            if path.is_symlink():
                raise ValueError("pinned input must not be a symlink")
            if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
                raise ValueError("SHA-256 differs from the pinned manifest")
        except (OSError, ValueError) as exc:
            errors.append(f"{relative}: {exc}")
    return errors


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", nargs="?", type=Path, default=ROOT,
                        help="project root (defaults to this script's checkout)")
    args = parser.parse_args(argv)
    errors = validate(args.root)
    print("Static repository validation: " + ("FAIL" if errors else "PASS"))
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    print("Nested CLI diagnostics: not run")
    print("Hosted delegation admission/completion: not checked")
    print("Device/runtime checks: not checked")
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
