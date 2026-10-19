# AGENTS.md

## Project

`eutheto` is a local-first, open-source constraint-optimization platform with a working Rust core, headless CLI, and Tauri/Vue development desktop. Implementation proceeds through the dependency-gated phases in [`docs/roadmap/`](docs/roadmap/README.md); consult the roadmap for the active phase and [`README.md`](README.md) for verified user-facing capabilities and limitations.

Do not claim, document, or test behavior that has not been implemented. Do not create mock authority in the desktop client to make an incomplete vertical slice appear functional.

## Source of truth

Use this authority order when requirements conflict:

1. approved security and data-integrity ADRs;
2. current scenario, bundle, and worker-protocol schemas plus conformance tests;
3. [`docs/roadmap/README.md`](docs/roadmap/README.md) and the active phase document;
4. generated documentation;
5. code comments.

[`docs/roadmap/assumptions.md`](docs/roadmap/assumptions.md) records dated package and tooling evidence. Reverify mutable external contracts before adopting or updating them. Do not silently replace a roadmap requirement with an assumption.

## Delivery discipline

- Work only within the current phase and its completed prerequisites. A later phase may influence a contract fixture, but its production feature does not move earlier.
- Preserve issue IDs, acceptance criteria, exit gates, non-goals, and compatibility requirements from the applicable phase document.
- Build complete vertical behavior rather than disconnected scaffolds or placeholders.
- Make the smallest coherent change. Do not add speculative abstractions, dependencies, configuration, fallbacks, or feature flags.
- Update every affected caller, test, schema, generated artifact, and document in the same change. Use clean cutovers; do not leave deprecated aliases unless a published compatibility contract requires them.
- Never hand-edit generated files. Change their authoritative inputs and regenerate them through the repository command.
- Before a PR is merged into `main`, review and make needed updates to root `README.md` and `AGENTS.md`. Keep verified capabilities, setup/usage, limitations, and documentation navigation in README; keep durable contributor rules, architecture boundaries, command authority, and verification guidance here. Do not duplicate transient task status or detailed phase checklists in either file.
- Respect explicit manual-testing checkpoints in the roadmap. Hosted checks, a merged PR, or agent confidence do not substitute for maintainer checkpoint acceptance or authorize dependent production work.

## Agent orchestration

Use the repository-local [Ponytail skills](.agents/README.md) directly when automatic discovery is unavailable. Ponytail Full is the default for each task. A reasoned escalation to Ultra is task-local and ends with that task; explicit user choices take precedence. Modes are behavioral instructions, separate from model reasoning effort. Keep vendored skills unchanged.

The lead, implementation, correctness_review and complexity_review use explicitly requested `gpt-6.1-sol` with `high` reasoning. Use `gpt-6-astra` / `high` for difficult technical planning or consequential decisions; hand routine implementation and review back to Sol/high. Use the four roles in [`.codex/agents/`](.codex/agents/) and give each worker a concrete scope, ownership, dependencies and verification expectations. At most two subagents may be active per project in addition to the lead. This is orchestration policy; hosted enforcement has not been established. Check existing workers before launching another.

Named TOML profiles remain available for local clients that support them. Their presence does not register hosted roles. In hosted execution, pass role instructions and explicit model/effort settings directly through the platform delegation tool when supported, and report this handoff distinction. Model-setting admission and task completion are separate evidence; neither independently attests the backend model. Do not describe requested settings as verified runtime identity. File edits do not reconfigure running workers.

Keep static configuration checks, optional nested CLI diagnostics, actual hosted delegation smoke and device/runtime checks separate. `just codex-static-check` verifies repository inputs only. Do not require nested `codex app-server` startup for hosted readiness. Managed read-only `CODEX_HOME` or project-trust failures in a nested CLI do not establish hosted delegation failure; retain those diagnostic errors when that diagnostic is requested. Do not claim checks not run or weaken validators to obtain success.

Never modify protected home, relocate or replace `CODEX_HOME`, inject `BASH_ENV`, grant hook trust, or execute untrusted hooks to activate this setup. Preserve existing pinned Nix/Docker startup. Do not change project packages, architecture, security, credentials, network or billing without explicit approval. Do not push, create a PR, merge, deploy or publish saved-environment settings without approval. Repository configuration changes do not update saved-environment settings. See [hosted/local verification and publication guidance](docs/contributors/codex-cloud.md).

## Architecture boundaries

Dependency direction is presentation → thin Tauri adapter → application services → domain packs → planning core → backend adapters/infrastructure.

- Rust owns authoritative scenario state, validation, persistence, routing, solving, verification, scoring, explanations, and import/export.
- Tauri is a client boundary, not the optimizer. Only the desktop API layer may invoke Tauri commands or subscribe to Tauri events.
- Use the generated desktop API and its operation lifecycle rather than adding a parallel invoke/event wrapper. Bind work to the actual native window and immutable scenario/revision context; keep resource cleanup distinct from cancellation and durable mutation outcomes.
- Work editors retain raw intent; Rust resolves local endpoints, recurrence and reconciliation. Bind approval to the immutable source, revision and prospective hash. Display windows do not limit command scope, and occurrence ledgers are never editable presentation fields.
- Validation navigation uses native owner and field provenance through closed editor adapters. Preserve scenario, revision, library epoch, draft ownership and post-await focus checks; wait for initial reads without reopening a handled request on later background refreshes. Never infer an endpoint from a record-level finding or treat a resource reference as a substitute for its explicit owner path.
- Domain packs depend on the domain API and solver-neutral planning IR. They never construct backend objects or depend on Tauri, SQLite, credentials, network providers, OR-Tools, or Pumpkin.
- Solver adapters depend on planning IR and solver APIs, never official domain packs.
- AI may read bounded context and propose typed application commands. It cannot bypass validation, mutate persistence directly, access arbitrary files, execute shell/code, or act as solver/verifier authority.
- Every accepted candidate is projected and independently verified against the original domain scenario, with authoritative score recomputation.
- Hidden global mutable state is forbidden.
- `unsafe` is forbidden in normal crates. A narrow exception requires an isolated safety module, documented invariants, dedicated tests, and maintainer review.

## Data and protocol rules

- Mutations are typed, validated, transactional, revision-checked, durable, and reversible when semantics permit.
- Solves operate on immutable scenario revisions. Stale results remain explicit and cannot be silently applied.
- Stable typed IDs identify entities; never use display names, collection positions, or database row IDs as identity.
- Public scenario, bundle, database, command, event, and worker-protocol formats are versioned. Never reuse a field or tag with different meaning.
- Unknown newer versions fail safely. Preserve unknown extension data where the format contract requires forward compatibility.
- Canonical serialization and hashing use stable ordering and checked arithmetic.
- Imported files, bundles, worker frames, provider responses, and URLs are untrusted, bounded inputs. Parse and validate before one atomic commit.
- Generated TypeScript guards are not native admission checks. Bound untrusted native request data before additional typed allocation or queued retention, then preserve authoritative application/domain validation.
- OR-Tools communicates through the versioned worker protocol and remains isolated from the desktop process. Worker failure must not corrupt scenario state.

## Security and privacy

- Secrets never enter Vue/JavaScript state, logs, SQLite, exports, diagnostics, Nix derivations, repository files, or normal IPC payloads.
- Credentials are entered through a Rust/native-owned secure surface and stored only in the operating-system credential store; Vue receives opaque references and status.
- Native-selected CSV paths and raw snapshots stay in Rust. Vue receives opaque source IDs, bounded review DTOs, and explicitly selected inert samples, not filesystem authority or a second CSV parser.
- Standalone settings import is a library-revision-bound complete replacement of only `appearance`, `locale`, and `units`, including entry timestamps and reviewed removals. Keep picker paths native, excluded device/credential state untouched, and active review custody charged until its owner settles.
- Use least-privilege Tauri commands, capabilities, CSP, filesystem grants, CI permissions, and release jobs.
- Do not add telemetry or network access by default. AI and provider integrations remain optional and explicit.
- Never commit `.env` files, credentials, signing material, local databases, captured user scenarios, or unsanitized support bundles.
- Treat dependency licenses, install scripts, advisories, SBOM inputs, updater metadata, and signing custody as release constraints.

## User interface and accessibility

- Use human domain language before solver terminology: `Required`, `Preference`, `Optimize`, and `Repair plan`.
- Rust remains authoritative; Pinia and query state are presentation caches only.
- Full-record editor rebases preserve independent current fields and inactive raw input. Replace a raw field only when adopting different native meaning; resolve collections and nested objects as whole fields, not hidden element merges.
- Every primary flow is keyboard-complete, has correct focus behavior and screen-reader names/announcements, and does not rely on color alone.
- Canvas and charts require an equivalent accessible list/table representation.
- Implement normal, empty, loading, stale, error, cancellation, and offline-capable states as applicable.

## Commands and generated artifacts

`Justfile` is the human command authority; `xtask` owns cross-platform generation, hashing, solver assembly, licenses, SBOMs, fixtures, and release manifests. Prefer those commands over ad hoc scripts. Start with `just install`, `just check`, and the focused commands listed by `just`; README documents development and packaged-CLI entry points.

Use `just generate` after changing authoritative generators, schemas, or contract inputs, and `just generate-check` to verify drift. Desktop API generation is owned by `xtask/src/generate.rs` and `xtask/src/phase02_generate.rs`; generated TypeScript and native command catalogs travel with those inputs. Only document commands that exist, and distinguish checks actually run from future release gates.

Commit `Cargo.lock`, `pnpm-lock.yaml`, `flake.lock`, exact CI action SHAs, worker source/hash metadata, and required generated protocol/DTO/schema artifacts. Build output, local caches, test reports, local databases, and secrets remain ignored.

## Verification

- Bugs: reproduce the failure, fix its causal mechanism, and rerun the reproducer.
- Features and contract changes: exercise the observable path and run the narrowest existing checks that prove it.
- Schema/protocol/migration changes: run compatibility, round-trip, unknown-version, generated-drift, and malformed-input fixtures.
- Solver changes: run capability, translation, cancellation, worker-failure, projection, independent-verifier, score, deterministic-fixture, and applicable benchmark checks.
- Desktop changes: run focused type/lint/component checks and exercise the packaged Tauri surface where the changed behavior depends on native integration.
- Security-sensitive changes: test both request and response boundaries and confirm diagnostics/support artifacts remain redacted.
- Report only checks actually run. A unit test does not prove packaging, and a successful build does not prove product acceptance.

## Repository identity

Rust crates use `eutheto-*`; npm packages use `@eutheto/*`; project-owned media types use the `eutheto/...` namespace. The CLI executable, reverse-domain application IDs, portable project extension, hosting organization, governance/security contacts, and signing identities remain explicit roadmap gates. Do not publish placeholders as final identifiers.
