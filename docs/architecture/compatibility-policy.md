<!-- SPDX-License-Identifier: Apache-2.0 -->

# Compatibility Policy

## Scope and phase boundary

This policy applies once introduced to every public scenario document, portable bundle, database schema, application command/event/DTO, CLI machine-readable output, domain-pack document, persisted or exchanged planning/projection record, worker protocol, release manifest, and updater metadata format. [ADR-018](../adr/018-public-scenario-representation.md) makes the versioned eutheto scenario document/bundle—not a backend model—the public interchange authority. [ADR-012](../adr/012-tauri-api-and-generated-dtos.md) governs generated desktop DTOs, and [ADR-004](../adr/004-ortools-worker.md) governs the isolated worker protocol.

[Phase 00](../roadmap/00-repository-and-reproducible-tooling.md) establishes these rules and generation scaffolding only. It does not claim a released schema or a working solver peer. Phase 01 implements scenario, bundle, database, command, event, and DTO compatibility; Phase 02 introduces domain/planning contracts; Phase 03 implements and proves the worker protocol with an approved matched solver/protobuf toolchain; Phase 11 assembles release matrices; and Phase 12 requires every-version migration and protocol evidence before publication.

## Version contract

Every public surface must have:

- one authoritative version declaration, not a version inferred from application code or package metadata;
- documented current writer version and exact reader support window;
- a compatibility classification for each change;
- typed `malformed`, `unsupported-version`, `incompatible`, and `resource-limit` failures as applicable;
- centralized byte, item-count, nesting, string, frame, and decompression limits; and
- canonical serialization rules where bytes are hashed, signed, compared, or used as fixtures.

Writers emit only the current version. Readers support exactly the documented window—never an accidental range accepted by a permissive parser. An application release manifest records independently versioned application, core API, scenario envelope, domain schema, planning IR, worker protocol, solver, adapter, and applicable policy/catalog versions together without pretending those versions are interchangeable.

## Change classification

A change is **compatible** only when all supported readers retain the documented meaning and safety properties. Additive fields are not automatically compatible: their absence/default semantics, unknown-field behavior, canonical form, bounds, and old-reader behavior must be specified and tested.

A change is **migration-requiring** when supported older data can be deterministically transformed to the current form without losing promised meaning. Migration logic lands before current writers depend on the new form. Imports parse and validate into staging, complete all required migrations, and commit once atomically.

A change is **breaking** when it changes existing meaning, removes required supported meaning, violates the documented reader window, or cannot preserve supported data. It requires the format's declared incompatible-version transition, an overall major release where the public CLI/API/document contract is incompatible, updated support policy, migrations or explicit refusal, and release notes. A clean internal cutover does not excuse breaking a released external contract.

## Unknown-newer behavior

An unknown newer outer envelope, bundle, database, domain-pack, persisted-record, updater-metadata, or protocol version is rejected safely before mutation, migration, worker execution, update installation, or other authoritative side effect. The error must identify the unsupported surface and version without exposing untrusted payload details. There is no best-effort downgrade, silent default to the current version, or partial import.

Unknown data inside an otherwise supported version follows that format's explicit rule:

- designated extension data is preserved losslessly through read/write and migration when forward-compatible preservation is part of the contract;
- unknown enum or tagged-union values are never coerced to a known default;
- non-extension unknown fields may be rejected when the schema is closed; and
- ignored protocol fields or messages are permitted only when the negotiated protocol version explicitly defines that behavior and no safety, lifecycle, or authority decision depends on them.

Preserving opaque extension data does not make it trusted or executable. It remains bounded and cannot influence validation, routing, solving, credentials, filesystem access, or UI rendering without a recognized schema.

## Permanent field and tag identity

Once released, a field name, numeric field tag, enum numeric value, tagged-union discriminator, command/event name, stable error code, or protocol message kind permanently retains its meaning. Removal retires the identifier; it never makes the identifier available for a different meaning. Protobuf field numbers and enum values must be declared reserved when removed, and generated bindings must retain that reservation.

Renaming while reusing the same identifier for changed semantics is a breaking change, not a migration shortcut. A new meaning receives a new identifier and versioned migration. Aliases, dual writers, deprecated duplicate fields, and silent fallbacks are forbidden unless the published compatibility contract specifically requires a bounded read path. Internal callers migrate in the same change and obsolete write paths are removed.

## Portable-v2 prerequisite compatibility

The Phase-05 prerequisite separates the current portable wire document from the frozen internal
snapshot. It advances only the global portable schema and the synthetic pack's portable payload:

| Surface | Current writer | Supported reader |
|---|---:|---|
| Outer ZIP bundle format | V1 | V1 |
| Global portable scenario schema | V2 | V1 through the explicit V1→V2 migration; V2 directly |
| Internal `ScenarioDocument.formatVersion` | V1 | V1, unchanged |
| Internal `ScenarioSnapshotV1.schemaVersion` | V1 | V1, with unchanged retained SQLite representation |
| Synthetic `official.test` internal schema | V1 | V1 |
| Synthetic `official.test` portable payload | V2 | V0→V1→V2 and V1→V2 sequential migration; V2 directly |

Workforce has no implemented schema in this prerequisite; its approved initial internal and portable
versions remain V1, without fabricated historical support. The prerequisite introduces no database
migration and no internal snapshot rewrite.

Current wire `PortableScenario` contains host-owned identity, metadata, settings, revision and wrapper
fields plus exactly one pack-owned `PortableDomainDocument`. The pack owns its domain records and
scenario-document nonsemantic extensions. Current and retained-revision exports call the registered
pack encoder; imports decode and structurally validate every revision before collision remapping,
preview retention or transaction staging. Solver feasibility is not a persistence requirement.

Every source entry must declare the same global schema version as its manifest. Source manifest
requirements equal the exact union of host and domain declarations. Host capabilities cannot be
authorized by a pack descriptor; domain capabilities are checked against the owning pack. Trusted
migration may introduce recorded requirements, after which the effective manifest is rebuilt from
the converted output. Missing source declarations are not repaired implicitly.

The global V1→V2 step is recorded once per bundle. Pack migration records include optional
`versionSpace` and a `subject` identifying pack, scenario and revision. Existing persisted `registry`
values remain unchanged, so older binaries can still read that provenance. Unopened metadata exposes
`internalPackSchemaVersion` and `portablePackSchemaVersion` separately; an unavailable pack permits
only bounded inspection and exact-byte re-export of the original archive, without codec execution.

No-remap, current-internal round trips preserve canonical scenario and archived result bytes,
retained snapshot bytes and existing checksums. Copy remapping and internal migration preserve source
checksum fields without recomputing them, but their result records remain inert portable sections:
they are not accepted results and transformed wrappers need not pass current result validation.
Only fresh local independent verification can grant acceptance authority.

Historical `tests/migration/fixtures/portable_v1_scenario.json` remains unchanged. The current
`portable_v2_scenario.json` fixture is generated by `just generate`; unknown-newer coverage uses V3.
Archive limits bound actual decompressor output, and transformed JSON, decoded snapshots and retained
preview copies share aggregate byte, depth, collection and memory limits.

## Phase 04 result-contract compatibility

| Surface | Current writer | Current reader | Retained legacy reader |
|---|---:|---:|---|
| `NormalizedSolution` | V1 | V1 only | None; the V1 shape remains current |
| `VerificationScope` | V1 | V1 only | None |
| `VerificationContextV1` | V1 | V1 only | None |
| `VerificationReport` | V2 | V2 only | Exact V1 decoding through `LegacyVerificationReportV1`; inert and never accepted |
| `AcceptedResult` | V2 | V2 only | Exact V1 decoding through `LegacyAcceptedResultV1`; inert and never upgraded in place |

All current result contracts are closed to unknown fields, reject unknown versions, use canonical
ordering where checksums depend on serialized bytes, and reject scenario revisions above
`9_007_199_254_740_991`, the largest integer exactly representable by supported JSON/TypeScript
clients. Current writers never emit V1 report/result data. Legacy readers neither construct a V2
result nor grant acceptance authority.

## Phase 06 settings and library-operation compatibility

| Surface | Current writer | Supported reader |
|---|---:|---|
| Standalone `eutheto/application-settings` document | V1 | V1 only; no legacy reader or extensions |
| Settings import/export requests and results | V1 | The matching co-bundled desktop client |
| Operation prepare/progress | V1 | The matching co-bundled desktop client |
| API response envelope | V1, unchanged | The matching co-bundled desktop client |
| Build-owned locked-workspace license inventory | V2 | V2 only, with explicit `lockedWorkspace` scope |

The desktop Rust adapter and generated TypeScript client are one co-generated, co-bundled
reader/writer pair. Independently deployed or mixed-version desktop clients are not supported.
The additive library operation context, settings purposes and `publishingFile` progress phase
do not change existing scenario context encoding. Unknown tags and outer versions fail safely;
old clients cannot initiate library operations. This is not a claim that every older V1 decoder
accepts the new tags.

`ApiResponseDto.currentRevision` identifies the authoritative context revision, whether scenario
or library, matching existing backup responses. Settings preview/export report their captured
library revision; apply reports its committed or unchanged library revision. Unrevisioned reads
and discard report null. Scenario response meaning is unchanged.

Standalone settings are a complete replacement of only `appearance`, `locale`, and `units`.
Entries contain both `value` and `updatedAt`; omitted keys mean reviewed removal, and an empty
settings map means clear that scope. Device settings, credentials and unrelated library data
are excluded. Unknown keys/fields/versions, duplicate source keys, invalid timestamps/values
and oversized documents are rejected. Local validation and the existing nonsecret portable
policy both apply: an unportable local value makes the whole export fail, never silently omit
a key. A review binds exact before/after entries and library revision; valid apply ownership
is one-use, including after failure. No database migration or backup-restore mode is introduced.

Source documents and compact settings request/result families are bounded at64KiB; encoded
settings envelopes allow136KiB. The offline inventory is bounded at2MiB compact/2,368KiB wire,
with at most4,096packages. It reports the locked workspace, not exactly linked installer
dependencies or completed license clearance; `NOASSERTION` retains its original meaning.

## Worker protocol compatibility

The authoritative protocol source, protocol version declaration, generator and runtime, matched upstream protobuf inputs, generated bindings, worker, desktop adapter, golden frames, hashes, and manifests are one reviewed contract. Peers negotiate before accepting model/request traffic. An unsupported version, capability, target, source/hash, or lifecycle contract produces a typed incompatibility and starts no solve.

A protocol change must define request and response limits, request-ID matching, lifecycle and terminal states, cancellation, unknown-message behavior, and failure recovery. Malformed, truncated, oversized, duplicate, stale, out-of-order, or unknown worker output cannot mutate scenario state or crash the desktop process. The exact OR-Tools/protobuf pin, source hash, and build flags remain a Phase-03 gate; this Phase-00 policy supplies none of those values.

## Required migration and compatibility evidence

A public schema or protocol change is complete only when the authoritative input, generated products, every reader/writer/caller, compatibility declaration, documentation, and focused evidence agree. The evidence set includes, as applicable:

1. immutable fixtures for every supported released version and the expected canonical current form;
2. sequential migration from each older version plus every direct path the support policy promises;
3. canonical read/write round trips, deterministic ordering and hash stability;
4. lossless round trips and migrations for designated unknown extension data;
5. safe unknown-newer refusal and downgrade guards;
6. malformed, duplicate-key/tag, unknown-enum/union, size/count/nesting/decompression, overflow, and checksum cases;
7. interrupted migration rollback, backup/restore, restart, export-after-migration, and one-atomic-commit behavior;
8. old/new protocol handshake and capability matrices, golden frames, truncation, oversized/malformed frames, unknown tags/messages, request-ID mismatch, duplicate/out-of-order lifecycle messages, early exit, timeout, and cancellation;
9. proof that incompatibility or malformed input cannot partially mutate authoritative state; and
10. deterministic generation and clean-tree generated-artifact drift checks.

Released migration files and compatibility fixtures are immutable evidence. A new migration is additive and carries before/after fixtures; prior files are not rewritten to make history appear current. [Generated artifact policy](generated-artifacts.md) governs regeneration, ownership, source hashes, and checked-in outputs. Phase 12 accepts compatibility claims only when this evidence is bound to the identical release-candidate digests.
