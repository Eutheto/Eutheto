<!-- SPDX-License-Identifier: Apache-2.0 -->

# Workforce `core` boundary

The `eutheto-workforce` workspace crate owns the official Workforce domain implementation. WF-001 currently provides pack-owned UUIDv7 identities, the typed four-map model for current and reserved MVP semantics, generated record/command schemas and editor/AI metadata, bounded document decoding and structural/reference validation, explicit score-policy shape checks, inclusive local planning-date derivation from the host horizon, pure typed CRUD commands with exact inverses, and current portable-v1 conversion. It remains **unregistered**: these APIs do not expose Workforce projects through application or CLI dispatch and provide no solver or verifier.

## Boundary

It may use the domain API, stable value types, solver-neutral planning IR, and normalized verification contracts. It must not depend on Tauri, Vue, SQLite, credential stores, network providers, OR-Tools, Pumpkin, or backend-native objects.

Bounded CSV review remains WF-001 work. Recurrence, complete rule semantics, compilation and independent verification follow the ordered Phase05 packages. Production registration requires WF-007 and complete result contracts. No partial `DomainPack` implementation or synthetic solver authority is permitted.

`validation::decode_document` bounds raw input before typed decoding; `validate_document` also handles an existing host document without changing it. Structural checks include normalized owned-identity uniqueness, expected reference kinds, explicit scope safety, exact external-ID uniqueness, temporal coherence, coverage count ordering, and workload peer/target agreement. Valid-but-contradictory coverage remains editable. Missing score policy is an incomplete draft, not a decoding error. Known later semantics are retained data, not executable capability.

Document limits reuse the shared contract bounds: 16 MiB serialized bytes, depth 32, 16 KiB strings and 100,000 aggregate items/nodes. Each host map and reference list is capped at 10,000 entries; occurrence ledgers are capped at 4,096 per template and 32,768 per document. Policy limits are 15 explicit score levels plus the reserved final rank level, 256 workload definitions and 32 piecewise segments. Cross-field and whole-document bounds can make a particular combination smaller than these individual ceilings.

`commands::apply_batch` applies at most 1,000 typed operations on a private working copy. Every prefix must remain structurally valid; deletes never silently cascade references. Updates retain kind, untouched records retain exact JSON, and reversed ordinary command inverses restore original field presence and spelling. Both forward and inverse batches must fit the shared 16 MiB serialized command bound. Returned changes carry record paths and exact before/after values; persistence, revisions and production routing remain host-owned.

`portable::export_portable` and `import_portable` preserve the four raw record maps and declared `nonsemantic.*` extensions under Workforce portable version 1, requiring exactly `official.workforce.portable` at version 1. Import replaces only the explicit host shell's domain and extensions; host identity, metadata, settings and versions remain unchanged and are validated with the resulting document. Unknown semantic requirements, record variants and older/newer Workforce versions fail without a fabricated migration. Depth/safety checks precede recursive serialization or cloning of caller-built payloads. Global portable version 2, internal snapshots and their genuine historical migrations remain host-owned.

## Generated contracts

`schemas/domain-packs/workforce.contract.json` is the source-v3 authoring contract. `just generate` expands its bounded local definitions into the existing runtime schema vocabulary, emits the Rust command identities consumed by dispatch, and produces the internal/portable schema artifacts, twelve command contracts, TypeScript payload types, editor manifest and explicit AI-tool subset. Source version 3 does not change the Workforce internal or portable version 1.

Both document ingress paths and command dispatch reject raw JSON representations outside those schemas before accepting records. Each operation owns its parsed, validated schemas and reuses them across batch prefixes; no global schema cache is used. Accepted records are not normalized, so inverse replay retains their exact spelling.

The portable artifact is checked against actual pure conversion, not application import/export authority. Score/provenance/result/transfer metadata remains empty and Share Result is absent. Editor and AI metadata describes current typed data/proposal shapes, not an implemented desktop or assistant flow.
