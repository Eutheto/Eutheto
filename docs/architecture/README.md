<!-- SPDX-License-Identifier: Apache-2.0 -->

# Architecture

This directory summarizes the approved architecture. Phases 00–05 have implemented reproducible tooling, transactional local persistence, domain-pack and solver-neutral Planning IR contracts, the isolated OR-Tools worker, independent verification, and the bounded Workforce core with real CLI solving and export. Automated acceptance and the maintainer's manual-testing pass are complete; [Phase 06](../roadmap/06-desktop-design-system-and-workforce-setup.md) desktop design-system and Workforce setup work is the active scope, not completed behavior. Pumpkin, remaining domain features, AI, the updater, and a production release remain roadmap work. The [roadmap index](../roadmap/README.md) owns delivery order and detailed acceptance gates.

## System shape

```text
Vue presentation ──> generated desktop API ──> thin Tauri adapter
                                                   │
CLI ───────────────────────────────────────────────┤
                                                   v
                                       application services
                                  commands · queries · persistence
                                             │           │
                                             v           v
                                      domain packs   infrastructure
                                             │
                                             v
                                      planning core / IR
                                      │             │
                                      v             v
                              in-process adapter   worker-protocol adapter
                               (future Pumpkin)      (OR-Tools child)
                                      │             │
                                      └──── candidates ────┐
                                                           v
                                      projection and independent verifier
```

Dependency direction and ownership are normative in [dependency boundaries](dependency-boundaries.md). The central decisions are [ADR-001](../adr/001-core-library-and-cli.md), [ADR-006](../adr/006-solver-neutral-planning-ir.md), [ADR-007](../adr/007-independent-solution-verification.md), [ADR-012](../adr/012-tauri-api-and-generated-dtos.md), and [ADR-018](../adr/018-public-scenario-representation.md).

## Authority

Rust application services own authoritative scenario state, validation, transactions, persistence, routing, solving, projection, independent verification, scoring, explanations, and import/export. Tauri and the CLI are clients of those services. Vue/Pinia/query state is a presentation cache. A backend produces candidates, never trusted domain results. Optional AI can propose typed commands but is never persistence, solver, verifier, or routing authority.

## Solve data flow

The Workforce CLI implements this path for the Phase 05 supported rule set; later domain and desktop flows must preserve the same authority boundaries:

1. capture immutable scenario revision $R$;
2. domain-validate $R$;
3. deterministically compile solver-neutral planning IR plus provenance and projection;
4. normalize and validate the IR;
5. fingerprint it and route to an exactly compatible backend;
6. treat every backend event and candidate as untrusted;
7. project typed values to normalized domain assignments;
8. independently evaluate every required rule and recompute the authoritative score;
9. expose only verified candidates; and
10. atomically persist an accepted solution with its revision and reproducibility evidence.

A failed or cancelled solve never mutates the scenario. Persisting an accepted solution is a separate transaction after verification. [ADR-004](../adr/004-ortools-worker.md) defines worker isolation, and [ADR-008](../adr/008-mvp-solver-routing.md) defines routing and decomposition.

## State and external formats

SQLite is the implemented local authority; credential values remain restricted to OS credential storage ([ADR-010](../adr/010-local-state-and-credentials.md)), not a claim that provider credential workflows already exist. Every mutation uses the typed command/journal contract ([ADR-011](../adr/011-command-journal-and-undo.md)). The public interchange contract is the versioned eutheto scenario document/bundle, never a backend model ([ADR-018](../adr/018-public-scenario-representation.md)).

All public scenario, bundle, database, command, event, schema, and worker-protocol formats require explicit versions and compatibility rules. Unknown newer versions fail safely. Canonical serialization and hashing require stable ordering and checked arithmetic. Contributor rules are in [generated code and contracts](../contributors/generated-code-and-contracts.md).

## Security boundaries

Imported documents/bundles, archive entries, worker frames, provider data, URLs, and future pack components are untrusted bounded inputs. Credentials never cross into Vue, SQLite, logs, exports, diagnostics, Nix derivations, or ordinary IPC. Tauri capabilities, the webview CSP, filesystem access, worker processes, OS credential storage, and release build/sign separation are explicit trust boundaries. See the [Phase-00 threat model](../threat-model.md).

## Change discipline

The numbered ADRs are approved decisions. They are not changed in place when the decision changes: a new numbered ADR must supersede the old record and add reciprocal links. Work pauses for an ADR when a backend needs domain knowledge, UI needs direct database access, AI needs arbitrary file/code/shell access, a plugin needs native in-process loading, decomposition cannot prove independence, migration cannot preserve supported scenarios, or packaging requires disabling a major security control.
