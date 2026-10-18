<!-- SPDX-License-Identifier: Apache-2.0 -->

# Dependency Boundaries

The canonical layer order is:

```text
presentation → thin Tauri adapter → application services → domain packs
             → planning core → backend adapters → infrastructure
```

Arrows describe allowed knowledge, not blanket permission to import every layer to the right. Interfaces remain at the inward boundary; infrastructure implements application-owned ports. The reusable Rust core and headless CLI rule is [ADR-001](../adr/001-core-library-and-cli.md).

## Layer rules

| Layer | May know | Must not know or do |
|---|---|---|
| Vue presentation | generated application DTOs, the desktop API facade, presentation-only state | Tauri primitives outside the API facade; SQLite/tables; credentials; domain mutation authority; solver or worker APIs |
| Thin Tauri adapter | generated DTOs, application-service interfaces, narrowly scoped native facilities | domain rules; planning compilation; solver construction; independent verification; broad filesystem/shell authority |
| CLI adapter | application-service interfaces, CLI serialization and exit policy | desktop/Tauri types; direct persistence mutation; backend-specific public formats |
| Application services | typed commands/queries, domain registry interfaces, persistence/process/credential ports | Vue components; ambient global mutable state; unvalidated direct provider/backend authority |
| Domain packs | domain API, stable value types, solver-neutral planning IR, normalized solution/verifier contracts | Tauri; SQLite; credential stores; network providers; OR-Tools; Pumpkin; backend objects; arbitrary HTML |
| Planning core and IR | typed solver-neutral primitives, capability/provenance/projection metadata | official domain-pack implementations; UI; database; provider clients; backend-native objects |
| Backend adapters | solver API, validated planning IR, worker/native solver interface | official domain packs; authoritative domain score or feasibility claims; persistence mutation |
| Independent verifier | original normalized domain scenario, normalized assignments, primitive independently tested facts | backend status as rule evidence; compiled backend constraints as the sole oracle |
| Infrastructure | application-owned persistence, filesystem, clock, process, network, and credential ports | domain policy; callers bypassing validation/transactions; secrets in ordinary responses |
| Optional AI adapter | bounded typed context, typed proposal/query contracts | database or credential access; arbitrary files, shell, code execution; solver/verifier/routing authority |

## Process and trust separation

The planned OR-Tools path crosses a versioned worker protocol into one supervised child per solve. The worker receives numeric/stable model identifiers rather than names, notes, credentials, AI content, or unrelated paths. Frames are length/count bounded and validated before use. A worker exit, mismatch, malformed response, timeout, or cancellation returns a typed failure and cannot mutate scenario state. See [ADR-004](../adr/004-ortools-worker.md).

The recipient PDF helper is a separate rendering boundary. It consumes only the validated recipient disclosure model, including the labels the user selected for sharing. It cannot establish solution acceptance or publication authority. Core retains those decisions and the exact rendered bytes. Typst belongs only in the helper; the CLI, desktop adapter and Core must not link the renderer. See [recipient PDF rendering](recipient-pdf.md).

A future in-process Pumpkin adapter is explicitly experimental and must satisfy dedicated-thread, cancellation, panic, compatibility, packaging, and benchmark gates before its label can change ([ADR-005](../adr/005-pumpkin-experimental-backend.md)). A future third-party pack boundary is sandboxed WASM/components, never native dynamic libraries ([ADR-009](../adr/009-domain-pack-loading.md)). Neither is a Phase-00 capability.

## Data ownership rules

- Stable typed IDs are identity; display names, array positions, and database row IDs are not.
- Scenario mutation is typed, validated, transactional, revision-checked, durable, and reversible when semantics permit.
- Solves read immutable revisions. Results retain their source revision and are explicitly stale after relevant edits.
- Public formats are versioned and do not reuse fields or tags with changed meaning.
- Imported content is parsed and migrated fully before one atomic commit.
- Canonical work uses stable ordering and checked integer arithmetic.
- Hidden global mutable state is forbidden.
- `unsafe` is forbidden in normal project crates. A narrow exception requires an isolated safety module, documented invariants, dedicated tests, and maintainer review.

## Enforcement contract

Phase 00 establishes these checks as repository policy; automation must fail closed as the relevant crates and packages appear:

- Rust dependency policy rejects layer reversals and cyclic crate dependencies.
- Frontend linting rejects Tauri invoke/event imports outside `apps/desktop/src/api`.
- The generation drift check rejects hand-edited or stale DTO/schema/protocol outputs.
- Source policy rejects `unsafe` in normal crates unless the documented exception is reviewed.
- Architecture checks reject hidden global mutable state patterns rather than allowing undocumented exceptions.
- CODEOWNERS review applies to parsers/untrusted inputs, worker protocol, credentials, updater/signing, cryptography, and Tauri capabilities.

An absent future crate is not proof that its behavior exists; a check becomes executable with the owning phase and must not be replaced by a no-op. The [roadmap](../roadmap/README.md#global-invariants-and-stop-conditions) remains the delivery authority.
