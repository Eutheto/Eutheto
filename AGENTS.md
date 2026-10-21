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

## Architecture boundaries

Dependency direction is presentation → thin Tauri adapter → application services → domain packs → planning core → backend adapters/infrastructure.

- Rust owns authoritative scenario state, validation, persistence, routing, solving, verification, scoring, explanations, and import/export.
- Tauri is a client boundary, not the optimizer. Only the desktop API layer may invoke Tauri commands or subscribe to Tauri events.
- Use the generated desktop API and its operation lifecycle rather than adding a parallel invoke/event wrapper. Bind work to the actual native window and immutable scenario/revision context; keep resource cleanup distinct from cancellation and durable mutation outcomes.
- Explanation reads bind the immutable accepted-result anchor separately from the visible head, library epoch, pair identity and native window. Difference anchors to the candidate; Repair anchors to the current result. Check an exact native anchor revision before admitting its alarm. Retained diagnostic custody uses the original record checksum, not the separately checksummed general evidence envelope.
- Repair facts consume complete captured accepted A/C sources, not diff rows or the visible head. Preserve accepted A→C comparison separately from recorded B→C baseline facts; B remains `notEstablished`, never authenticated by a public checksum. Check the full original obligation census under one inherited budget, retain distinct expected/observed facts and exact native group members, and mark unresolved historical endpoints unavailable rather than publishing a filtered aggregate proof. Preserve conservative score unavailability and non-unique causality.
- Explicit repair originals are immutable source selectors, never implicit base adoption. Resolve exact original revisions and preserve native custody even for JSON operands; retries bind accepted/source/run identity and retain recorded B. Run-only captures use context2 with a 512 KiB canonical ceiling checked before tree retention or pair projection; Authored/in-snapshot writers remain context1. Decode captured pairs once per operation, retain aggregate carrier/IPC limits, and keep file reads and native verification waits under the original solve budget.
- External starting plans are complete, explicitly unverified pair sets, including empty sets; never synthesize accepted ancestry or Required locks. Select accepted versus external sources explicitly, without fallback. Context3 binds the current external pair hash; preserve V1/V2 accepted-source bytes. Original-import fingerprints are descriptive and survive pair-only edits/copy remapping. Retries and admitted Alternatives members retain their original capture; source revision changes refuse new members rather than recapturing. Keep editor raw intent/receipts alive across presentation refreshes while native approval remains revision-bound.
- Manual edits keep accepted A immutable and authenticate complete B through native receipt custody. Bind review approval to the whole source, revision and prospective meaning; Hard removal requires explicit unlock in the same atomic command. Undo/Redo preserves exact source, inverse, lock and prior-selection checks. Consume owned source/receipt representations rather than duplicating full trees; admission counters remain cumulative and are never refilled on release. A confirmed commit survives later cleanup; reconcile unknown outcomes from History rather than replaying.
- Work editors retain raw intent; Rust resolves local endpoints, recurrence and reconciliation. Bind approval to the immutable source, revision and prospective hash. Display windows do not limit command scope, and occurrence ledgers are never editable presentation fields.
- Validation navigation uses native owner and field provenance through closed editor adapters. Preserve scenario, revision, library epoch, draft ownership and post-await focus checks; wait for initial reads without reopening a handled request on later background refreshes. Never infer an endpoint from a record-level finding or treat a resource reference as a substitute for its explicit owner path.
- Domain packs depend on the domain API and solver-neutral planning IR. They never construct backend objects or depend on Tauri, SQLite, credentials, network providers, OR-Tools, or Pumpkin.
- Solver adapters depend on planning IR and solver APIs, never official domain packs.
- Assumption diagnostics use an objective-free, exactly-one-worker profile with native presolve disabled. Validate the entire bounded signed core against submitted source literals before mapping; never publish a filtered subset or treat an empty core as foundational proof. The ordinary parameter digest alone does not record static presolve policy; preserve model and authenticated worker identity binding.
- An established native non-clock correctness alarm survives timing and cleanup failure. Preserve actual observation bounds; do not clamp/backdate timestamps or publish a duration sentinel. Closed alarm metadata reports identity and exact durable confirmation, never verification authority. A clock marker carries no measured timings and cannot itself establish an alarm.
- Compact native observations are descriptive, never acceptance or alarm authority. Preserve them through custody and retiming; retain runtime only from an admitted completed terminal with its applied-parameter digest. Store checks exact input bindings without solver dependencies; Core checks the recorded routed fingerprint at every typed replay exposure. A valid public checksum or historical worker-artifact digest does not authenticate an imported claim. Within an admitted exact source scope, metadata failure never outranks a known alarm; an incorrect source revision remains a request refusal.
- Recipient reports project the complete immutable, freshly verified source, not visible pages or public acceptance flags. Keep privacy switches explicit and recorded A/B membership distinct from native original/manual custody. Native preview identity binds window, source, head and library; native publication consumes exact captured bytes under the revision lease with explicit stale consent and no clobber. Unknown outcomes are never replayed.
- Report custody charges retained source capacities and bounded late growth, not just Arc pointers. Install reservations before exposing operation IDs; retain active counts/bytes through owned work and application-response handoff. Observer release does not cancel work or release ready human review. Queued failures preserve known source alarms; confirmed publication survives cleanup. Transport handoff is not delivery or a promise to purge Tauri queues.
- HTML, PDF and CSV/calendar review share one native count/byte budget. Bind capture kind and exact format before admitting retained source alarms, including when the custody clock fails. Keep terminal/checkpoint sources explicit; never fabricate a terminal manifest for a checkpoint. CSV/calendar samples are bounded inert excerpts, not complete-file review, and per-person identities come from the accepted source rather than the visible page or mutable head.
- Explicit-file recipient reports never open a library or infer its live head. Retain complete recorded comparisons without promoting historical custody or substituting current facts for unavailable original facts. Finite CLI HTML requires a named destination and one budget spanning source reads, preparation and publication; interruptible pre-capture waits must not become timeouts around owned capture/publication settlement or reject a confirmed commit.
- AI may read bounded context and propose typed application commands. It cannot bypass validation, mutate persistence directly, access arbitrary files, execute shell/code, or act as solver/verifier authority.
- Every accepted candidate is projected and independently verified against the original domain scenario, with authoritative score recomputation.
- Hidden global mutable state is forbidden.
- `unsafe` is forbidden in normal crates. A narrow exception requires an isolated safety module, documented invariants, dedicated tests, and maintainer review.

## Data and protocol rules

- Mutations are typed, validated, transactional, revision-checked, durable, and reversible when semantics permit.
- Solves operate on immutable scenario revisions. Stale results remain explicit and cannot be silently applied.
- Stable typed IDs identify entities; never use display names, collection positions, or database row IDs as identity.
- Counterfactual assignment identities come from the saved native result, not frontend ID codecs. A Workforce replacement must select its target and exclude its incumbent on the same shift while preserving original rules, objectives and revision; forcing the target alone is not a replacement under AtLeast coverage. Retained alternatives are accepted result records, never automatic scenario mutations.
- Public scenario, bundle, database, command, event, and worker-protocol formats are versioned. Never reuse a field or tag with different meaning.
- Unknown newer versions fail safely. Preserve unknown extension data where the format contract requires forward compatibility.
- Canonical serialization and hashing use stable ordering and checked arithmetic.
- Generic JSON domain views preserve safe signed numbers, including negative timezone offsets, through the setup encoder. Keep exact-value strings explicit and retain wide-integer losslessness; do not apply the typed score/evidence serializer's all-signed-integer quoting policy to domain view trees.
- Objective selection is explicit recorded context, not inferred from effort mode, policy labels, score levels, or generic metadata. Historical absence remains distinct from current Authored semantics.
- Alternatives share one immutable native capture, parent duration/start and sequential admission. Worker fair shares do not reset the parent budget or fallback reserve. Reattachment uses exact native group ownership; a durable request anchor or an evicted/restarted summary never authorizes replay.
- Suppress Alternatives only after complete, cumulatively charged native non-rank metrics and per-person configured workload/preference outcomes match exactly. Preserve signed and fractional facts, distinct baseline additions/removals and exact Soft-lock outcomes. Unavailable equivalence leaves accepted results distinct; suppression never deletes History or establishes cross-profile utility.
- Score comparison requires fresh admission of both original sources and pack-established original cost, population and layout meaning; each side validates its own saved policy digest. Different revisions or lowered policies need not change utility, while matching unknown metadata never establishes it. Preserve semantic changes, protection state, side labels and descriptive run bindings when score comparison is unavailable.
- Archival result flags are descriptive, never native acceptance. Preserve original payload/identities and SQL-absent versus JSON-null fields; copy and deletion follow only the live wrapper roots. Require the host archival capability while keeping content/assets checks recursive. Missing native-only sources refuse selected result export, not unrelated edits or results-excluded backups; retain known exact sources and keep native identity occupancy separate from persisted supplemental ownership.
- Imported files, bundles, worker frames, provider responses, and URLs are untrusted, bounded inputs. Parse and validate before one atomic commit.
- Generated TypeScript guards are not native admission checks. Bound untrusted native request data before additional typed allocation or queued retention, then preserve authoritative application/domain validation.
- OR-Tools communicates through the versioned worker protocol and remains isolated from the desktop process. Worker failure must not corrupt scenario state.

## Security and privacy

- Secrets never enter Vue/JavaScript state, logs, SQLite, exports, diagnostics, Nix derivations, repository files, or normal IPC payloads.
- Credentials are entered through a Rust/native-owned secure surface and stored only in the operating-system credential store; Vue receives opaque references and status.
- Native-selected CSV paths and raw snapshots stay in Rust. Vue receives opaque source IDs, bounded review DTOs, and explicitly selected inert samples, not filesystem authority or a second CSV parser.
- CSV apply rebuilds through the retained preview family, not a caller-selected decoder. Assignment imports reserve canonical framing even when malformed or skipped, hash all original bytes, and review one complete unverified plan. Keep all assignment exclusions under their own report bound; do not inherit People's smaller rejection-report cap or infer identity from names or eligibility.
- Eligibility CSV edits sparse typed person/type pairs, not computed feasibility or a whole matrix. Preserve original raw Person fields and retained membership spelling/order; group one ordinary update per changed person. Native command root IDs remain canonical, while raw reference guards accept native-equivalent UUID spellings. Bound each alias map before typed decoding and retain the complete eligibility exclusion report independently of People's smaller cap.
- Pasted eligibility matrices permit only explicitly entered, bounded transient Vue text, never clipboard access or native-file snapshots. Reject whole overflow before Vue retention and recheck borrowed native text before typed allocation; do not use `maxlength` or accept a truncated prefix. Rust validates the complete tab-separated rectangle and counts every cell, including blanks/skips; blank preserves membership, aliases are role-specific, and native cell coordinates retain their distinct record/column bases. Retire source/review custody before recapture, preserve row-major additions and complete exclusion reports, and keep unknown outcomes replay-free.
- Shift CSV targets stored manual/detached instances only. Native Add collision checks include document/occurrence-ledger and both stored wrapper extension identity layers in preview and rebuild; a supplied unknown ID can only be added unchanged. Resolve offset-free local intent under captured scenario time policy, preserve unmapped endpoints/raw fields, and compare inner qualification lists as typed sets without discarding authored coverage rows. Missing row projections never imply an identity is available for Add.
- Availability CSV preserves the four native kinds and complete Instant/Weekly variants. Approved leave blocks independently; requested leave is not approval and only contributes through explicit preferences. Preserve incoming active/inactive requested-leave references, validate raw structured cells before semantic comparison, keep role-specific aliases and exact nested-leaf spelling, and retain native endpoint facts even for equivalent edits. Weekly order/multiplicity and source/note whitespace are authored; never infer rollover, merge window variants, expand recurrence during import or invent rules/preferences.
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

Native command cutovers also update the authored `apps/desktop/src-tauri/permissions/foundation-status.toml` allowlist and pass `foundation_permission_matches_the_command_catalog`. Capability reporting lists implementations; it does not replace Tauri permission admission.

Commit `Cargo.lock`, `pnpm-lock.yaml`, `flake.lock`, exact CI action SHAs, worker source/hash metadata, and required generated protocol/DTO/schema artifacts. Build output, local caches, test reports, local databases, and secrets remain ignored.

Dependency updates must keep exact pins aligned across target-specific manifests and generator-tool version checks. Regenerate notices, the license inventory, and the SBOM through `just licenses sbom`, and run `just generate` when an updated formatter changes generated output. Preserve existing license and advisory policy; a dependency bump does not approve new exceptions.

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

## Agent workflow and Ponytail

Use the repository-local [Ponytail skill](.agents/skills/ponytail/SKILL.md) for coding work, with Full as the default for each task. The orchestrator may promote a task to Ultra with a stated reason when unclear assumptions, unnecessary complexity, or consequential decisions warrant challenging the approach. Pass the mode explicitly in the handoff; promotion ends with that task. Explicit user choices, including Lite or Off, win. This task-local policy supersedes the vendored skills' session-persistent mode wording. Ponytail modes do not change model reasoning effort.

Route technical specifications to `planning` (`gpt-6-astra`, high reasoning), implementation to `implementation`, correctness/security/regression review to `correctness_review`, and simplification review to `complexity_review`. Implementation and routine correctness/complexity review must explicitly use `gpt-6.1-sol` with `high` reasoning. Use `gpt-6-astra` with `high` only for planning or genuinely difficult decisions; do not reuse an Astra planning worker for routine implementation or packaging. Use these roles when delegation is appropriate; simple tasks need not spawn all four. Profiles live in [`.codex/agents/`](.codex/agents/). Include scope, project instructions, user constraints, task mode, evidence, file ownership, and dependencies in every handoff.

Named TOML profiles remain available for local clients that support them. Their presence does not register hosted roles. Native hosted selection of these profiles is unverified. When custom role selection is unavailable, read the profile and applicable skills directly and pass their instructions explicitly. Set explicit model and reasoning overrides for every invocation: planning uses `gpt-6-astra` / `high`, and implementation and routine reviews use `gpt-6.1-sol` / `high`. Use `fork_turns="none"` when the host requires it for overrides, supplying the necessary context. Do not substitute unknown inherited settings for these explicit choices. Report explicit handoffs separately from native profile discovery and do not claim installation proves activation. Model-setting admission and task completion are separate evidence; neither independently attests the backend model. Do not describe requested settings as verified runtime identity.

At most two subagents may be active for this project in addition to the lead. This is orchestration policy; hosted enforcement has not been established. Check existing workers and do not create another worker while at that cap. Checkpoint and stop excess workers safely, preserving edits and test evidence. A profile edit or requested handoff does not change an already-running agent's model: verify an explicit new invocation or stop the worker before claiming compliance. Routine lead work uses `gpt-6.1-sol` / `high` when selected by the host; if a lead model change takes effect only next turn, checkpoint and end the current turn before resuming.

Keep static configuration checks, optional nested CLI diagnostics, actual hosted delegation smoke and device/runtime checks separate. `just codex-static-check` verifies repository inputs only. Do not require nested `codex app-server` startup for hosted readiness. Managed read-only `CODEX_HOME` or project-trust failures in a nested CLI do not establish hosted delegation failure; retain those diagnostic errors when that diagnostic is requested. Do not claim checks not run or weaken validators to obtain success.

Preserve all project guidance and approval boundaries. Never merge to main, and obtain parent/user approval before a push or PR, deployment, or publication of saved-environment settings. Do not change dependencies, architecture, security, credentials, network or billing without explicit approval. Do not enable hooks, execute untrusted hooks, grant hook trust, write persistent mode flags, modify protected home configuration, relocate or replace `CODEX_HOME`, or inject `BASH_ENV`. Preserve existing pinned Nix/Docker startup. Repository configuration changes do not update saved-environment settings. Read the permitted files directly if discovery is unavailable and report the limitation. Source provenance, license inputs, and host limitations are recorded in [`.agents/README.md`](.agents/README.md); see [hosted/local verification and publication guidance](docs/contributors/codex-cloud.md).
