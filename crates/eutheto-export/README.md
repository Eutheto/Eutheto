<!-- SPDX-License-Identifier: Apache-2.0 -->

# `eutheto-export`

Canonical current portable scenario and bundle serialization, bounded nonsecret JSON preparation, and atomic no-clobber publication. Callers supply consistent typed application data; this crate does not open SQLite or act as solver/verifier authority.

## Boundary

- `assemble_scenario_export` and `assemble_full_backup` validate represented identities, dependencies and assets before producing deterministic bundle bytes.
- `prepare_bundle_atomic_cancellable` verifies bundle bytes before shared owner-private sibling staging.
- `prepare_json_atomic_controlled` checks bounded canonical JSON and portable-data safety, not domain semantics or accepted-result authority. The application must validate the typed value before calling it.
- `PreparedPublication::publish_controlled` uses explicit cancellation or the original solve deadline. Staging writes, flushes, syncs, reopens and hashes exact bytes before the atomic no-clobber commit. Existing destinations are never overwritten; precommit interruption removes staging, and successful publication wins over later cancellation.

Persisted solve output is secondary to the database commit. The core service publishes only the reloaded store-owned portable result; output failure cannot roll back or relabel that committed result.

Native unaccepted/legacy results use the closed `eutheto/archival-result` V1
envelope and require host capability `eutheto.portable.archival-result` version1.
The root `resultId`, `scenarioId` and `scenarioRevision` bind live ownership and
the exact source. `archive` preserves original identities, recorded status/flag,
timestamp and solution/score/verification JSON, with optional explanation/evidence
fields absent for SQL NULL and present for JSON null. These facts confer no
acceptance or verification authority.

Copy remaps only live roots; original archival identities and JSON remain inert.
Content, secret, depth, byte and asset checks still traverse the entire record.
Selected results require an exact current or retained historical source. An
unavailable native-only source or unsafe native payload refuses result-bearing
export rather than dropping data; excluding results avoids native payload
projection. Older readers must reject the required host capability, not deep-remap
the unknown archive. No Workforce schema or accepted-result version is redefined.

## Verification

`just test-portable` exercises serialization, malformed input and publication boundaries. Native packaging and host-specific file-selection guarantees require their separate platform evidence.
