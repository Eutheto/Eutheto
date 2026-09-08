<!-- SPDX-License-Identifier: Apache-2.0 -->

# `eutheto-export`

Canonical current portable scenario and bundle serialization, bounded nonsecret JSON preparation, and atomic no-clobber publication. Callers supply consistent typed application data; this crate does not open SQLite or act as solver/verifier authority.

## Boundary

- `assemble_scenario_export` and `assemble_full_backup` validate represented identities, dependencies and assets before producing deterministic bundle bytes.
- `prepare_bundle_atomic_cancellable` verifies bundle bytes before shared owner-private sibling staging.
- `prepare_json_atomic_controlled` checks bounded canonical JSON and portable-data safety, not domain semantics or accepted-result authority. The application must validate the typed value before calling it.
- `PreparedPublication::publish_controlled` uses explicit cancellation or the original solve deadline. Staging writes, flushes, syncs, reopens and hashes exact bytes before the atomic no-clobber commit. Existing destinations are never overwritten; precommit interruption removes staging, and successful publication wins over later cancellation.

Persisted solve output is secondary to the database commit. The core service publishes only the reloaded store-owned portable result; output failure cannot roll back or relabel that committed result.

## Verification

`just test-portable` exercises serialization, malformed input and publication boundaries. Native packaging and host-specific file-selection guarantees require their separate platform evidence.
