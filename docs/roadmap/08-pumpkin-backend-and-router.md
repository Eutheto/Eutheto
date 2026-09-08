# Phase 08 — Pumpkin Experimental Backend and Deterministic Router

## Outcome

Add Pumpkin as an explicitly experimental, opt-in backend without changing reliable default behavior. Implement one deterministic, explainable backend router; generated capability/support reporting; pre-solve incompatibility blocking; deliberate fallback; backend comparison diagnostics; and a compatible benchmark subset. After any proven exact-native specialized match, OR-Tools CP-SAT remains the stable general default for all supported nontrivial official-domain models.

Pumpkin 0.5.0 may be integrated only after actual pinned-API compatibility, dedicated-thread ownership, cooperative cancellation, verification, packaging, and benchmark gates pass. Its presence, absence, failure, or incompatibility cannot make the public MVP’s default path less reliable.

## Source coverage

This phase covers blueprint Section 14 in full, Section 16 in full, Phase 8, Sections 31.3–31.4, 32, and backend Definition of Done 33.3, CLI Appendix C solver surfaces, Tauri API Appendix D solve surfaces, backlog `SOLVER-001`/`PUMPKIN-001` from Appendix I, and the router/fallback evidence required by [Performance and Solver UX Targets](performance-and-solver-ux-targets.md).

Shared contracts are in [README.md](README.md), version evidence and unresolved gates in [assumptions.md](assumptions.md), the planning IR in [Phase 02](02-domain-pack-and-planning-ir-contracts.md), stable OR-Tools implementation in [Phase 03](03-ortools-worker-vertical-slice.md), verification in [Phase 04](04-independent-verifier-and-explanations.md), and the first complete official-domain corpus in [Phase 07](07-workforce-solving-results-repair-and-export.md). Seating in [Phase 09](09-seating-domain-and-venue-experience.md) consumes the same router without backend-specific domain code.

## Dependencies and entry conditions

- Planning IR primitives, summaries, objective levels, provenance, hashes, resource limits, and projection contracts are stable.
- `SolverBackend`, descriptor, compatibility, options, statuses, progress, errors, and cancellation contracts exist and are exercised by OR-Tools.
- OR-Tools CP-SAT is packaged and remains the default stable backend.
- The independent verifier rejects malformed or semantically invalid candidates irrespective of backend.
- Official workforce fixtures and benchmarks provide a representative compatibility and performance corpus.
- The typed experimental feature registry and advanced settings UI exist; experimental enablement is not an arbitrary local-storage flag.

## Decisions and invariants

1. **OR-Tools is the default.** With no explicit override, supported nontrivial workforce and seating problems route to OR-Tools CP-SAT unless an exact native specialized structure is proven.
2. **Pumpkin is experimental.** It requires desktop opt-in, is clearly labeled in every picker/result, and is exposed by the working CLI only with an experimental warning. It is never silently auto-selected in public-MVP defaults.
3. **Compatibility precedes execution.** Unsupported IR nodes, objective semantics, assumption/explanation modes, or options yield an explainable compatibility report and disabled picker choice. Never start and then fail for a known gap.
4. **Shared IR is backend-neutral.** Do not contort the IR or official domain semantics around Pumpkin’s current capabilities. Backend adapters translate only supported nodes.
5. **Every candidate uses the same trust pipeline.** Project, structurally validate, independently evaluate all domain rules, and recompute score. Backend claims are never authoritative.
6. **Routing is deterministic code, never AI.** Persist chosen backend and ordered reason codes with the solve run.
7. **Fallback is policy, not retry folklore.** It follows the failure-class rules below and one total resource budget.
8. **Decomposition requires proof.** Fairness/global objectives commonly reconnect apparent partitions. No heuristic visual/domain split is allowed.
9. **Version and stability are visible.** Descriptor, run metadata, diagnostics, support matrix, and exported evidence carry backend and adapter versions.
10. **Cancellation must be real enough for desktop safety.** Pumpkin’s cooperative polling is acceptable only after latency is measured on supported primitives and nominal limits are not misleading.

## Backend contract and normalized model

All backends implement the common asynchronous contract:

```rust
#[async_trait]
pub trait SolverBackend: Send + Sync {
    fn descriptor(&self) -> &SolverDescriptor;
    fn compatibility(
        &self,
        problem: &PlanningProblemSummary,
        options: &SolveOptions,
    ) -> CompatibilityReport;
    async fn solve(
        &self,
        problem: Arc<PlanningProblem>,
        options: SolveOptions,
        progress: ProgressSink,
        cancellation: CancellationToken,
    ) -> Result<BackendSolveResult, BackendError>;
}
```

The Pumpkin adapter may satisfy this outer `Send + Sync` service contract by owning the actual `pumpkin_solver::Solver` on one dedicated blocking thread; Pumpkin 0.5.0’s solver is `!Send + !Sync` and must never cross that thread boundary. Requests/results cross through owned, bounded messages. Recoverable adapter errors become typed `BackendError`; no unwind crosses the application boundary.

`SolverDescriptor` includes stable solver ID, display name, backend version, adapter version, distribution (`BuiltIn`, `BundledWorker`, or `UserProvided`), license metadata, stability (`Stable`, `Beta`, or `Experimental`), and capabilities. Pumpkin is `BuiltIn` only if linked into that build, but remains `Experimental`.

`CompatibilityReport` includes `compatible`, exhaustive unsupported features, warnings, and optional estimated translation/model cost. The UI and CLI display exact unsupported node/option/objective/explanation reasons.

`SolveOptions` carries mode, maximum time, optional worker-thread count, random seed, optional backend override, stop-after-first-feasible, intermediate-solution collection, explanation mode, and resource limits. An adapter rejects unsupported options explicitly rather than ignoring them.

Normalize statuses to:

- `Optimal`;
- `Feasible`;
- `Infeasible`;
- `Unbounded`;
- `NoSolutionWithinLimit`;
- `Cancelled`;
- `InvalidModel`;
- `BackendUnavailable`;
- `BackendFailed`.

A backend’s unknown result maps from verified-incumbent presence and actual termination cause. Pumpkin timeout/cooperative stop without a candidate maps to `NoSolutionWithinLimit`; explicit user stop maps to `Cancelled`; a verified candidate may yield `Feasible` unless optimality is proved.

Progress is normalized as queued, compiling, backend started, presolve summary, incumbent found, bound improved, redacted safe log line, verifying, explaining, and completed. Throttle before the webview. Backend-specific events never leak into application/domain APIs.

## Deterministic router policy

Apply these ordered rules exactly:

1. If the user supplied a backend override, run its compatibility report. Honor it only if compatible and enabled under stability policy; otherwise return typed incompatibility before solve.
2. Without an override, first evaluate whether the compiler emitted a recognized exact native specialized structure. Select its native algorithm only when the algorithm’s semantics cover the complete connected component and objective and an independent verifier exists.
3. If no exact-native rule matched, use OR-Tools CP-SAT for every supported nontrivial official-domain problem. This general default must not preempt rule 2.
4. Never auto-select Pumpkin in the public MVP. A future opt-in experimental-routing setting may consider it only when experimental backends are enabled and its generated support matrix has no gap for the exact model/options.
5. Split connected components only after the proof below succeeds.
6. Persist the decision, candidates considered, compatibility reports, enabled-feature state, exact rule/reason IDs, and selected backend descriptor.

Router tests use fixed inputs and assert the selected backend and stable reason sequence. They include an overlap case where a model is both generally OR-Tools-compatible and a recognized exact-native structure, proving the exact-native rule is evaluated first and selected; a near-match proves the OR-Tools default follows when exact recognition fails. Routing cannot depend on wall-clock order, model text, provider output, or unrecorded environment state.

### Connected-component proof

Build a hypergraph whose nodes are planning variables. Every constraint and multi-variable objective expression connects all variables it references. Components are independent only when all are true:

- no constraint spans components;
- no objective term spans components;
- no global score normalization spans components;
- no solution projection or original domain rule requires a cross-component relationship;
- merging values from independently solved components cannot violate a domain invariant.

A proof artifact records component hashes and each passed criterion. Fairness, shared coverage, normalization, target-share, stability, and solution-level constraints frequently connect otherwise separate dates, locations, or tables. Never split because ranges merely look separate. If proof is incomplete, solve as one component.

### Deliberate fallback policy

**Durable multi-attempt gate:** Phase05 ordinary file/stored runs bind one selected backend/runtime before immutable run creation; their Auto request is recorded as effective Specific options and cannot silently become another backend's execution. Before enabling the fallback policy below for ordinary persisted runs, this phase must define and verify the durable attempt representation, exact per-attempt runtime/options/proof binding, compatibility/migration behavior and one-parent-budget accounting. Never archive backend B under backend A's immutable input or reinterpret retained historical hashes. Existing generic router fallback tests do not, by themselves, close this application/persistence gate.

- **Unavailable or crash before any candidate:** a configured compatible fallback may run.
- **Invalid model:** never silently fall back; surface a compiler/adapter defect and preserve bounded diagnostics.
- **Proven infeasible:** do not treat this as a performance failure. Another backend is available only as an explicit advanced diagnostic cross-check.
- **Time limit with no candidate:** a configured compatible fallback may run only within the remaining total budget.
- **Candidate fails independent verification:** quarantine it and raise a critical correctness defect. A different backend may run only after recording the failure; never erase the alarm.

Fallback records original/fallback descriptors, compatibility, failure category, consumed and remaining total budget, and final verified outcome. It never restarts a hidden unbounded retry loop.

### Determinism and reproducibility

Tests use a fixed seed and deterministic/single-worker settings where each backend permits. Production may use parallel backend search and can return a different equivalent result. Every run records random seed, thread count, backend version, adapter version, model hash, complete options, router decision, support-matrix version, and component proof so reproducibility is represented accurately.

## Pumpkin 0.5.0 adapter

### Integration boundary

- Place the adapter in a separate Rust crate behind the sparse `backend-pumpkin` Cargo feature.
- Pin exactly `pumpkin-solver`/its matched workspace crates at 0.5.0 and preserve lockfile/source evidence.
- Translate only IR nodes proven by executable conformance tests against the pinned API.
- Execute the non-`Send`/non-`Sync` solver on a dedicated blocking thread.
- Use the pinned `TerminationCondition::should_stop()` mechanism with atomic cancellation and time-budget composition; cancellation is cooperative, not preemptive.
- Bound request/result channels, memory/model size, progress emission, diagnostics, and thread teardown.
- Catch panics at the adapter/thread boundary where safe, convert them to typed backend failure, and preserve application integrity.
- Project and verify candidates identically to OR-Tools.
- Benchmark the compatible subset against OR-Tools on every official corpus update.

### Generated support matrix

Generate runtime and documentation views from backend capability descriptors; CI detects drift. Do not hand-claim support. Initial rows and required evidence are:

| Planning feature | OR-Tools CP-SAT default | Pumpkin 0.5.0 experimental | Native specialized |
|---|---|---|---|
| Boolean clauses | Yes, adapter-tested | Enable only after pinned contract tests | Some exact structures |
| Integer linear constraints | Yes, adapter-tested | Enable only for tested coefficient/domain/overflow bounds | Limited |
| Optional intervals | Yes | Adapter-dependent; otherwise incompatible | No |
| No-overlap | Yes | Enable only after pinned semantics tests | Interval-specific |
| Cumulative | Yes | Enable only after pinned semantics tests | No |
| Table constraints | Yes | Enable only after pinned semantics tests | No |
| Assumption core | Yes, with Phase 03 caveats | Capability-dependent; incompatible when requested and absent | No |
| Multi-level objective | Adapter lowering | Enable only after lexicographic/overflow/status equivalence tests | Algorithm-dependent |

Each enabled cell links to supported IR node/schema version, semantic restrictions, translation test IDs, cancellation behavior, status/proof behavior, and benchmark coverage. A partial primitive implementation remains `No` or `Restricted`, with machine-readable reasons.

### Mandatory activation gates

Pumpkin 0.5.0 cannot be offered for solving until all of these pass:

1. exact crate versions/source/checksums and Apache-2.0-compatible license inventory reviewed;
2. every claimed primitive and objective behavior translated and contract-tested, including boundary/overflow cases;
3. status, proof, timeout, no-candidate, and invalid-model normalization verified;
4. dedicated-thread ownership and no cross-thread solver access demonstrated;
5. cooperative cancellation measured across every supported expensive primitive, including long propagations; UI wording reflects observed latency;
6. panic, allocation/resource-limit, malformed-input, and teardown failure isolation tested;
7. candidates pass independent domain verification and deliberate invalid candidate tests are quarantined;
8. capability matrix generated from the exact build and exposed at runtime;
9. compatible official fixture subset and benchmark comparison completed;
10. explicit desktop opt-in, CLI warning, stability label, and unavailable-build behavior verified.

Passing these gates makes Pumpkin available experimentally; it does not make it default or stable. Auto-routing remains disabled unless a later explicit decision and benchmark/reliability evidence change the policy.

## Native and future backend boundaries

Native specialized algorithms are allowed for provably exact structures such as bipartite matching, minimum-cost assignment, connected components, shortest path, min-cost flow, and geometry preprocessing/spatial indexing. Graph-coloring heuristics may seed a full solver but cannot independently establish a trusted solution unless the exact algorithm/verifier contract does. Every specialized result returns through normalized projection and independent verification.

HiGHS is a post-MVP candidate for LP/MIP/QP-shaped IR; SCIP is a post-MVP candidate for mixed-integer/constraint-integer programming. Prefer out-of-process adapters unless a stable maintainable Rust binding and cancellation story is demonstrated. SCIP versions before 8.0.3 are not under the current Apache-2.0 licensing line; exact packaged dependencies must be reviewed.

Every future backend requires reviewed license/distribution manifest, descriptor and capabilities, translation tests per claimed IR node, cancellation/failure isolation, benchmarks, independent-verifier coverage, packaging/update strategy, and an ADR proving material platform benefit.

MiniZinc remains for research, benchmark comparison, and model prototyping. It is not scenario or planning IR. A post-MVP adapter may export compatible subsets to MiniZinc/FlatZinc and invoke installed or permissively bundled solvers after exact contents are reviewed: MiniZinc is MPL-2.0, while bundled solvers have separate licenses.

Gurobi, CPLEX, Xpress, and similar proprietary engines are user-provided integrations only. Eutheto does not ship binaries/license material, discovers installations only after explicit action, displays licensing requirements, isolates support in optional adapters/features, and never requires them to open or verify a scenario.

A post-MVP portfolio runner is distinct from decomposition. It may run compatible backends or parameter profiles concurrently only with one total resource budget, independent verification of every candidate, comparison of authoritative score vectors, safe cancellation, producer identity per alternative, and battery-aware defaults. Cross-solver decomposition of one connected component is separate research.

## Desktop and CLI contracts

The advanced backend picker calls `solve_get_backend_options`, showing descriptor, stability, distribution/license, version, capability summary, warnings, and exact incompatibility reasons. Experimental enablement uses the typed feature registry. Normal flows keep backend selection collapsed while the automatic router evaluates exact-native recognition first and otherwise defaults to OR-Tools.

Use existing solve endpoints: `solve_get_backend_options`, `solve_estimate_model`, `solve_start`, `solve_cancel`, `solve_get_job`, `solve_list_runs`, and `solve_get_diagnostics_summary`; existing namespaced solve events retain backend ID and revision. Backend comparison diagnostics are sanitized and bounded.

The working CLI name `optimizer` remains an unresolved naming gate. `solvers list`, `solvers describe <solver-id>`, and `solvers check <solver-id>` expose descriptor, build availability, support/capability details, stability, and health. `solve --backend <id>` is an explicit override. Selecting Pumpkin requires a visible experimental warning; incompatible use exits through the documented backend incompatible/unavailable/failure code rather than beginning a doomed solve. Human and JSON output preserve stable reason codes.

## Ordered work packages

1. **SOLVER-001 hardening:** finalize descriptor/capability, compatibility, status, progress, route-decision, component-proof, and fallback data contracts.
2. **Generated capability matrix:** derive machine/runtime/docs representations from descriptors and add drift/conformance checks.
3. **PUMPKIN-001 adapter spike behind gate:** pin 0.5.0, isolate crate/feature/thread, enumerate actual API support, and keep unavailable until mandatory gates pass.
4. **Primitive slices:** for each enabled IR primitive, add translation, boundaries, status, cancellation, independent projection/verification, matrix cell, and benchmark fixture together.
5. **Deterministic router:** implement ordered selection, explicit override, native exact-structure path, OR-Tools default, experimental exclusion, persisted reasons.
6. **Component proof and fallback:** implement hypergraph/objective/domain-invariant checks, total budget accounting, failure categories, diagnostics, and tests.
7. **Advanced UX/CLI:** picker, reasons, stability warnings, `solvers` inspection/check surfaces, unavailable-build state, run metadata.
8. **Benchmark and release evidence:** run compatible official corpus comparisons, cancellation latency scenarios, failure isolation, and accurate documentation.

## Tests and acceptance

- Table-driven router tests cover compatible/incompatible explicit override; exact-native precedence over the general OR-Tools default for a recognized structure; OR-Tools selection for an exact-native near-match and an ordinary model; disabled/enabled Pumpkin; objective/fairness preventing split; proven component split; and stable persisted reason codes and order.
- Compatibility tests exercise every IR primitive, option, explanation mode, objective level, coefficient/domain boundary, unsupported combination, and resource-limit constraint.
- Generated support matrix must equal runtime descriptors and link each positive claim to contract tests.
- Pumpkin candidate results pass the same projection, verifier, score, and quarantine behavior as OR-Tools.
- Cancellation tests cover before start, during translation, during search, long propagation, after incumbent, and completion race; observed cooperative latency is bounded/documented.
- Status tests cover optimal, feasible, infeasible, unknown with/without incumbent, timeout, cancellation, invalid model, unavailable, panic/failure, and verification failure.
- Fallback tests cover all five policy classes, total-budget exhaustion, and proof that infeasibility/invalid-model do not silently retry.
- Component property tests prove merged values and scores equal unsplit semantics; adversarial fairness/global-objective cases refuse decomposition.
- Benchmarks use the compatible subset of all official workforce fixtures and later seating fixtures, recording candidate/model size, translation/startup, first incumbent, first independently verified feasible, final solve/verify/total time, solve status/termination/proof, authoritative score, bound, memory, cancellation latency, fallback consumed/remaining budget, backend/adapter versions, and environment. Performance alone never overrides correctness, and backend time alone never substantiates an end-to-end product claim.
- Desktop/CLI tests verify disabled incompatible choices, experimental labels, reasons, no reliability regression when Pumpkin is not compiled/available, and sanitization of diagnostics.

## Risks and failure handling

- **Pre-1.0 API churn:** exact 0.5.0 pin, narrow adapter, no leaked types, conformance matrix.
- **`!Send + !Sync` misuse:** dedicated owner thread and owned-message boundary.
- **Slow cooperative cancellation:** measured polling behavior; reject primitives whose cancellation latency is unacceptable; never claim a hard deadline not enforced.
- **Semantic translation gap:** incompatibility before solve; never approximate a Required rule or score level.
- **Backend candidate invalid:** quarantine, critical alarm, optional recorded alternate run only under fallback policy.
- **Experimental path degrades defaults:** Pumpkin remains excluded from automatic selection; absent exact-native recognition, OR-Tools remains selected; feature absence is normal; exhaustive router regression tests.
- **False decomposition:** require full hypergraph/objective/projection/domain proof or do not split.
- **Fallback masks defects:** invalid-model and verification failure remain visible and recorded.
- **License/distribution drift:** exact dependency manifest and review for every bundled adapter.
- **Benchmark gaming:** complete compatible corpus, authoritative verified score, resource metadata, no single headline metric.

Pause and write an ADR if a backend needs domain-specific knowledge, a claimed feature cannot be independently verified, decomposition cannot prove independence, cancellation/failure isolation is inadequate, packaging requires disabling security controls, a dependency license is outside policy, or correctness relies on undocumented backend behavior.

## Exit gate

Phase 08 is complete only when:

- unsupported models/options are blocked before execution with exact reasons;
- deterministic selection, connected-component proof, and fallback rules are tested and persisted;
- OR-Tools remains the default for all supported nontrivial official-domain problems that do not match a proven exact-native specialized structure;
- every Pumpkin result traverses identical projection and independent verification;
- Pumpkin 0.5.0 passes every mandatory compatibility/thread/cancellation/failure/benchmark gate before it is exposed, remains explicitly experimental and opt-in, and is never required;
- generated support matrix and runtime descriptor cannot drift silently;
- backend version, adapter version, seed, threads, options, model hash, selection/reasons, and component proof are recorded;
- status/cancellation/timeouts, crash/malformed-result handling, packaging availability, benchmark comparison, and user-facing stability labels satisfy backend Definition of Done;
- every fallback and backend comparison consumes the original remaining parent budget and preserves first-backend failure/proof evidence rather than restarting time or hiding defects;
- disabling or omitting Pumpkin leaves default UX, CLI, solving, verification, and exports fully functional and no less reliable.

## Deferred and non-goals

- Automatic Pumpkin routing in the public MVP.
- Concurrent multi-backend portfolios or cross-solver decomposition.
- HiGHS, SCIP, MiniZinc/FlatZinc execution, proprietary adapters, and a general solver plugin marketplace.
- Changing planning IR or domain semantics to fit one backend.
- Treating a heuristic, backend status, or benchmark win as independent verification.

## Assumption and version gates

- Current verified Pumpkin line is **0.5.0** as of 2026-08-29. It is pre-1.0; lock exact crates and sources. Its cooperative `TerminationCondition` and `!Send + !Sync` solver are design constraints, not assumptions to hide.
- OR-Tools **9.15** remains the recommended stable backend after Phase 03 platform/build/protobuf/assumption-core gates. The known OR-Tools 9.14/9.15 assumption-core presolve issue must remain visible in diagnostic capability and Phase 03 gating.
- Verify Pumpkin’s actual Boolean, integer-linear, optional-interval, no-overlap, cumulative, table, assumption-core, and multi-level-objective behavior against the pinned API; no documentation-only capability claim is sufficient.
- Exact lockfile pins remain Phase 00 actions. The `backend-pumpkin` feature must be included in an explicit tested build configuration rather than proliferating untested combinations.
- The working CLI name remains unresolved. Any future backend distribution, signing, application-ID, or hosting implications require the corresponding project-wide decision gate.
