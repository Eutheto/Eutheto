<!-- SPDX-License-Identifier: Apache-2.0 -->

# `eutheto-import`

Bounded inspection, migration and collision staging for untrusted portable scenarios and bundles. Inspection does not mutate a library or authorize a solver result.

## Boundary

- `inspect_bundle` validates archive structure, exact checksums, declared capabilities, transformed size limits, owned identities and dependency closure. Current and historical scenario payloads must reference represented scenario families; asset references require included payloads or valid omission metadata.
- `inspect_scenario` shares strict JSON parsing, source-before-migration requirements, caller-supplied migration registries, domain decoding and post-decoding host checks with bundle inspection.
- `validate_standalone_scenario` uses the same closure validator with one scenario and no supplemental assets. Standalone JSON preserves declared nonsemantic data but cannot silently depend on an absent asset or another scenario.
- Unknown newer versions and unsupported semantic capabilities fail safely. Migration provenance is reported; historical versions are supported only by genuine registered migrations.

The application owns atomic commit, exact-revision checks and user-safe error mapping. A decoded historical accepted-result record remains archival input until the application freshly verifies its original-domain bindings, required rules and score.

## Verification

`just test-portable` runs the existing portable package checks. Host-specific opened-handle, link/reparse and file-selection behavior remains the host adapter's responsibility; byte inspection alone does not establish those guarantees.
