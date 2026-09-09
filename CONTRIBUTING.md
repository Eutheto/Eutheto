<!-- SPDX-License-Identifier: Apache-2.0 -->

# Contributing to eutheto

Thank you for helping build `eutheto`.

## Current project state and authority

`eutheto` has completed Phase 05's bounded Workforce core vertical slice and the maintainer's manual-testing pass. The registered Workforce pack supports its initial five Required rules, headless people CSV review/apply/undo, and independently verified CLI solving and JSON/CSV export. Phase 06 desktop design-system and Workforce setup work is now authorized. Workforce desktop editors, remaining rules and result/repair screens, Seating, AI, the updater, and a shippable desktop release remain unimplemented. Contributions must not imply that deferred behavior exists.

Work is dependency-gated rather than date-gated. The [roadmap index](docs/roadmap/README.md) defines phase order and repository-wide authority; the [active Phase 06 document](docs/roadmap/06-desktop-design-system-and-workforce-setup.md) defines the current scope, acceptance evidence, non-goals, and exit gate. A later phase may inform a contract, but its production feature cannot be implemented early. When sources conflict, use the authority order in [AGENTS.md](AGENTS.md):

1. approved security and data-integrity ADRs;
2. current schemas and conformance tests;
3. the roadmap index and active phase;
4. generated documentation;
5. code comments.

Change an authoritative source and its tests together rather than documenting around a conflict.

## Before proposing a change

1. Identify the active roadmap work package and acceptance criterion served by the change.
2. Confirm that all phase prerequisites are complete. Preserve named issue IDs, compatibility requirements, gates, and non-goals.
3. Keep the change to the smallest complete behavior. Do not add speculative abstractions, dummy workers, mock authority, no-op commands, or placeholders that resemble production capability.
4. For a decision that changes an approved invariant, public contract, trust boundary, dependency direction, license policy, identity, or release/security posture, follow the ADR process in [GOVERNANCE.md](GOVERNANCE.md) before implementation.
5. Reverify mutable package, platform, and tool assumptions against primary sources and record dated evidence in `docs/roadmap/assumptions.md`; an assumption cannot replace a roadmap requirement.

Use short-lived branches and focused commits. A change is ready for review only when affected callers, tests, schemas, generated products, and documentation move together.

## Development commands

The Phase 00 contract makes Nix the canonical Linux environment and the language/tool provider on macOS, `Justfile` the human command authority, Cargo the Rust workspace authority, pnpm the JavaScript/TypeScript workspace authority, and `xtask` the owner of cross-platform generation, hashing, assembly, fixtures, licenses, and release manifests. Native pinned Windows tooling remains authoritative for Windows-specific behavior.

The following are the repository's canonical target entry points, not a claim that every deferred product capability exists:

```text
nix develop
nix develop --command just bootstrap
nix develop --command just install
nix develop --command just generate
nix develop --command just generate-check
nix develop --command just fmt-check
nix develop --command just lint
nix develop --command just typecheck
nix develop --command just check
nix flake check
```

Before running one, inspect the checked-in `Justfile` and `just --list`; a recipe is supported only when it exists there. Do not invent a successful invocation in an issue, pull request, or document. When a recipe exists, use it instead of an ad hoc script so local and CI behavior stay aligned. Shell entry must remain side-effect-free: entering `nix develop` or direnv must not install packages, fetch dependencies, run migrations, build solvers, or generate source files. Bootstrap, installation, generation, and native-worker work require explicit recipes.

Domain-pack generation and drift checking also require the locked JavaScript dependencies from `just install`: `xtask` invokes the workspace-pinned Prettier and checked-in desktop configuration for the combined TypeScript contract product. It verifies the exact formatter version and does not install dependencies or exempt generated TypeScript from formatting.

UI001 browser component checks are a separate, explicit surface: run `just frontend-browser-install` once to provision the lockfile-matched headless Chromium, then `just frontend-browser-test`. `just check` runs the Node-based frontend checks; the PR workflow also runs the browser scenarios. Browser focus/keyboard/axe evidence does not replace the dependency-free W3C `just e2e` native persistence check, manual assistive-technology review, or platform/installer acceptance. Exact dependency declaration repairs and their removal conditions are recorded in the [assumption ledger](docs/roadmap/assumptions.md#ui001-exact-stack-adoption-and-declaration-repairs--2026-09-09).

Report exactly what you ran and what it proved. A build does not prove product acceptance, a unit test does not prove packaging, and repository checks do not prove deferred product behavior.

## Architecture and security boundaries

All contributions must preserve these boundaries:

- dependencies point presentation → thin Tauri adapter → application services → domain packs → planning core → backend adapters/infrastructure;
- Rust owns authoritative scenario state and application behavior; frontend state is a presentation cache;
- only the desktop API layer may invoke Tauri commands or subscribe to Tauri events;
- domain packs depend on the domain API and solver-neutral planning IR, not Tauri, persistence, credentials, providers, or solver implementations;
- solver adapters depend on planning IR and solver APIs, never official domain packs;
- AI, when its phase is entered, may only propose typed, validated application commands and is never persistence, solver, or verification authority;
- imported content, protocol frames, URLs, and provider output are bounded untrusted input;
- secrets must never enter frontend state, logs, SQLite, exports, diagnostics, Nix derivations, repository files, ordinary IPC, caches, or pull-request jobs;
- hidden global mutable state is forbidden; `unsafe` in a normal crate requires the narrow, reviewed exception defined by the roadmap.

Read [SECURITY.md](SECURITY.md) before handling a suspected vulnerability. The private reporting channel is an unresolved gate, so never put vulnerability details in a public issue, discussion, commit, or pull request.

## Generated code

Never hand-edit a generated file. Find and change its authoritative input, then use the checked-in generation recipe. Checked-in generated outputs must be deterministic, and regeneration in a clean tree must produce no diff. A generated-file header, repository generation manifest, or `xtask` implementation determines ownership; if ownership is unclear, resolve that contract before changing the output.

Changes to schemas, DTOs, protocol outputs, worker metadata, notices, SBOM inputs, fixtures, or release manifests must include their required compatibility or drift evidence. Fields and tags are never reused with changed meaning, and unknown newer versions fail safely.

## Commits, licensing, and DCO

Unless a path says otherwise, contributions are offered under Apache-2.0. Add SPDX identifiers where the file format supports them and preserve existing notices. Do not add dependencies, assets, fonts, datasets, examples, or generated material without recording their source and license and satisfying the repository license policy.

Every commit must carry a Developer Certificate of Origin sign-off. Create the sign-off from your configured Git name and email with `git commit -s`. The sign-off certifies the [Developer Certificate of Origin](DCO.md); it is not a cosmetic footer and it must identify the contributor making the certification. If another person materially authored a commit, preserve authorship and obtain that person's own sign-off rather than signing for them. Fix a missing sign-off by amending or rebasing the affected commit as appropriate. `eutheto` requires DCO sign-off and does not require a Contributor License Agreement.

## Review and merge

Reviewers evaluate roadmap fit, correctness, architecture and trust boundaries, compatibility, generated drift, tests, licensing, accessibility where relevant, and whether documentation describes only observed capability. Authors must respond to material review findings or explain why the governing contract leads elsewhere.

Approval and passing automation do not waive an active phase gate. Merge authority, required ownership review, ADR decisions, recusals, and governance changes follow [GOVERNANCE.md](GOVERNANCE.md). Security-sensitive changes require review of both request and response boundaries and evidence that logs and support artifacts remain redacted.

By participating, contributors agree to follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). The governance, private reporting, public CLI, extension, reverse-domain, and signing identities remain deliberately unresolved public-identity gates; do not infer an organization, address, or final identity from the project name or development identifiers.