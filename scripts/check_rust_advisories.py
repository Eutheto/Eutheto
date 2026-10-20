#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Enforce the exact, temporary Phase 07 maintenance dispositions before deny."""

from datetime import date, datetime, timezone
import json
from pathlib import Path
import subprocess
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[1]
EXPIRY = date(2026, 11, 16)  # Inclusive UTC; no automatic renewal.
SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
# Owner: Phase07maintainer. Re-review conditions are in CONTRIBUTING.md.
MAINTENANCE = {
    "RUSTSEC-2025-0141": ("bincode", "1.3.3", "b1f45e9417d87227c7a56d22e471c6206462cba514c7590c09aff4cf6d1ddcad"),
    "RUSTSEC-2024-0436": ("paste", "1.0.15", "57c0d7b74b563b49d38dae00a0c37d4d6de9b432382b2892f0574ddcae73fd0a"),
    "RUSTSEC-2026-0206": ("rustybuzz", "0.20.1", "fd3c7c96f8a08ee34eff8857b11b49b07d71d1c3f4e88f8a88d4c9e9f90b1702"),
    "RUSTSEC-2026-0192": ("ttf-parser", "0.25.1", "d2df906b07856748fa3f6e0ad0cbaa047052d4a7dd609e231c4f72cee8c36f31"),
    "RUSTSEC-2024-0449": ("unsafe-libyaml", "0.2.11", "673aac59facbab8a9007c7f6108d11f63b603f7cabff99fabf650fea5c32b861"),
    "RUSTSEC-2024-0320": ("yaml-rust", "0.4.5", "56c1936c4cc7a1c9ab21a1ebb602eb942ba868cbd44a99cb7cdc5892335e1c85"),
}
PROTECTED = {
    "RUSTSEC-2026-0194", "RUSTSEC-2026-0195", "RUSTSEC-2026-0367",
    "RUSTSEC-2026-0368", "RUSTSEC-2026-0369",
}
LEGACY_IGNORES = {
    "RUSTSEC-2024-0370", "RUSTSEC-2024-0411", "RUSTSEC-2024-0412",
    "RUSTSEC-2024-0413", "RUSTSEC-2024-0414", "RUSTSEC-2024-0415",
    "RUSTSEC-2024-0416", "RUSTSEC-2024-0417", "RUSTSEC-2024-0418",
    "RUSTSEC-2024-0419", "RUSTSEC-2024-0420", "RUSTSEC-2024-0429",
    "RUSTSEC-2025-0075", "RUSTSEC-2025-0080", "RUSTSEC-2025-0081",
    "RUSTSEC-2025-0098", "RUSTSEC-2025-0100",
}


def check_ignored_ids(ignored):
    if ignored != MAINTENANCE.keys() | LEGACY_IGNORES:
        raise ValueError("advisory ignores differ from the six approved maintenance IDs and existing GTK policy")


def matches(package, expected):
    name, version, checksum = expected
    return (package.get("name"), package.get("version"), package.get("source"),
            package.get("checksum")) == (name, version, SOURCE, checksum)


def check_scope(lock, ignored, today):
    if today > EXPIRY:
        raise ValueError(f"Phase 07 maintenance dispositions expired after {EXPIRY} UTC; no automatic renewal")
    if PROTECTED & ignored or not MAINTENANCE.keys() <= ignored:
        raise ValueError("deny.toml must contain the six approved maintenance IDs and no protected vulnerability IDs")
    check_ignored_ids(ignored)
    for advisory_id, expected in MAINTENANCE.items():
        packages = [p for p in lock["package"] if p["name"] == expected[0]]
        if len(packages) != 1 or not matches(packages[0], expected):
            raise ValueError(f"{advisory_id}: locked package/version/source/checksum changed; re-review required")


def check_report(report, ignored):
    check_ignored_ids(ignored)
    # An ignored raw report cannot prove that the maintenance IDs remain informational.
    settings = report["settings"]
    if (report.get("error") or settings["ignore"] or settings["target_arch"]
            or settings["target_os"] or settings["severity"] is not None
            or set(settings["informational_warnings"]) != {"unmaintained", "unsound", "notice"}):
        raise ValueError("cargo audit must return an unfiltered, successful report")
    if report["database"]["advisory-count"] <= 0:
        raise ValueError("cargo audit returned an empty advisory database")
    vulnerabilities = report["vulnerabilities"]
    if (vulnerabilities["count"] != len(vulnerabilities["list"])
            or vulnerabilities["found"] != bool(vulnerabilities["list"])):
        raise ValueError("inconsistent cargo audit vulnerability report")
    findings = [("vulnerability", row) for row in vulnerabilities["list"]]
    findings += [(kind, row) for kind, rows in report["warnings"].items() for row in rows]
    seen = set()
    for kind, row in findings:
        advisory = row.get("advisory") or {}
        advisory_id = advisory.get("id")
        if advisory_id in PROTECTED:
            raise ValueError(f"{advisory_id}: vulnerability disposition is forbidden")
        if advisory_id in MAINTENANCE:
            if (kind != "unmaintained" or row.get("kind") != "unmaintained"
                    or advisory.get("informational") != "unmaintained"
                    or advisory.get("package") != MAINTENANCE[advisory_id][0]
                    or not matches(row["package"], MAINTENANCE[advisory_id])
                    or advisory_id in seen):
                raise ValueError(f"{advisory_id}: maintenance classification or exact package scope changed")
            seen.add(advisory_id)
        elif advisory_id not in ignored:
            raise ValueError(f"unreviewed {kind}: {advisory_id or row.get('package', {}).get('name', 'unknown')}")
    if seen != MAINTENANCE.keys():
        raise ValueError("expected maintenance notices missing; re-review required")
    return bool(findings)


def main():
    try:
        lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
        policy = tomllib.loads((ROOT / "deny.toml").read_text())
        ignored = {item["id"] for item in policy["advisories"]["ignore"]}
        check_scope(lock, ignored, datetime.now(timezone.utc).date())
        result = subprocess.run(
            ["cargo", "audit", "--json", "--deny", "warnings"],
            cwd=ROOT, text=True, stdout=subprocess.PIPE, check=False,
        )
        if result.returncode not in (0, 1):
            raise ValueError(f"cargo audit failed with exit {result.returncode}")
        findings = check_report(json.loads(result.stdout), ignored)
        if result.returncode and not findings:
            raise ValueError("cargo audit failed without advisory findings")
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Rust advisory gate failed: {error}", file=sys.stderr)
        return 1
    print(f"Rust advisory gate: six exact maintenance dispositions valid through {EXPIRY} UTC")
    return 0


if __name__ == "__main__":
    sys.exit(main())
