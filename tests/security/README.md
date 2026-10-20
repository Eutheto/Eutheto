<!-- SPDX-License-Identifier: Apache-2.0 -->

# Security tests

Reserved for repository-level tests at implemented trust boundaries: untrusted imports and archives, protocol frames, filesystem and temporary-state handling, desktop command/capability/CSP scope, credentials and redaction, dependency policy, and release artifact handoff. Future cases must exercise both accepted and rejected paths, enforce centralized bounds, and confirm that secrets, sensitive paths, and unsanitized data do not enter logs, exports, diagnostics, fixtures, or reports.

Adversarial fixtures must be synthetic, versioned, minimal, and safe to inspect or redistribute. Phase 00 contains no fabricated exploit corpus, credential, security result, or passing fixture.

`just rust-advisories-test` exercises the actual maintenance gate with synthetic
audit findings: the inclusive UTC expiry, exact version/source/checksum scope,
changed classifications, new findings, protected vulnerability IDs and audit
infrastructure failures. The security workflow runs it before `just rust-advisories`;
the latter fetches current advisory evidence and enforces the real gate.
