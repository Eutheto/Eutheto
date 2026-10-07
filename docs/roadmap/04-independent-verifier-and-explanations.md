# Phase 04 — Independent verifier and explanation foundation

## Outcome

Define and enforce the pack-neutral trust boundary that makes `eutheto` incapable of accepting a solver candidate on backend authority alone. This phase owns generic projection, verifier, score, evidence, quarantine, and explanation contracts plus their orchestration and conformance harness. Domain packs implement those contracts later from their own normalized semantics; Phase 04 does not implement an official domain projection, rule verifier, score, or evidence renderer.

Phase 04 is the trust boundary between [the OR-Tools worker](03-ortools-worker-vertical-slice.md) and every official domain/result flow. It exits with a pack-integration contract and domain-neutral test harness. [Phase 05](05-workforce-core-vertical-slice.md) then supplies the first real projection, verifier, score, and evidence renderer; [Phase 06](06-desktop-design-system-and-workforce-setup.md) consumes validation components; later result phases consume the full explanation surface.

## Source coverage

This phase incorporates blueprint Section 17; provenance, score, assumptions, projection, validation, hashing, and compilation requirements from Sections 13–15; Phase 4; explanation/result UX from Sections 21–22; related CLI and Tauri APIs from Appendices C and D; verification/explanation standards, tests, backlog, and definitions of done from Sections 26, 31, 33 and Appendices H–I; and the verified-result boundary in [Performance and Solver UX Targets](performance-and-solver-ux-targets.md).

## Dependencies

- Phase 01 supplies revisioned scenario state, solution/run persistence, stable DTO errors, job/cancellation infrastructure, command journaling, and quarantined diagnostic storage.
- Phase 02 supplies normalized domain models, `PlanningProblem`, solution projections, `ScoreVector`, provenance records, canonical hashes, capability metadata, and pack extension points.
- Phase 03 supplies backend variable assignments, raw/normalized status, bounds, assumption IDs, model fingerprint, applied parameters, and reproducibility metadata. It does not supply trusted feasibility or score.
- Phase 04 defines the pack contract requiring projection, independent verification of original normalized domain semantics, verifier-owned score recomputation, and typed evidence rendering. Official packs implement that contract in their own later phases. Backend crates never depend on domain packs; verifiers never use backend status as a rule result.

Phase 04 entry depends only on Phases 01–03. Its exit gate is satisfied by the generic contract and a synthetic conformance pack/harness; it never waits for Phase 05 workforce code.

## Decisions and global invariants

### Acceptance pipeline

The only candidate-acceptance path is:

```text
backend values
  → projection into normalized domain solution
  → structural solution validation
  → independent evaluation of every required domain rule
  → independent score recomputation
  → accepted solution
```

The verifier evaluates the exact scenario revision for which the model was compiled. A changed scenario makes the candidate stale; re-verification against the new revision is a separate operation, not an implicit update.

Record projection, structural validation, required-rule verification, score recomputation, evidence persistence, and first-verified-feasible timings separately. A backend incumbent timestamp is never substituted for `first_verified_feasible`. Once a candidate verifies, deterministic result availability does not wait for optional explanation enrichment, infeasibility shrinking, counterfactual work, or AI paraphrase.

If projection, structural validation, any required-rule evaluation, or score integrity fails:

- mark the solve run failed with a stable internal-correctness category;
- store raw backend/candidate material only in bounded quarantined diagnostics;
- do not place the candidate in normal solution lists;
- never enable select, export, publish, repair-base, or “ready” actions for it;
- expose a safe summary, redacted diagnostic ID, and recovery/report action rather than raw Rust/backend output;
- optionally invoke a different backend only after recording the critical failure under the Phase 03 fallback policy.

### Genuine independence

Acceptable shared foundations are stable IDs/value objects, timezone conversion, interval-overlap predicates, deterministic distance calculations, and other primitive facts whose semantics are independently tested.

Forbidden shortcuts are:

- ask CP-SAT whether its returned assignment satisfies the generated CP-SAT model;
- interpret backend `FEASIBLE`, `OPTIMAL`, or `INFEASIBLE` as a domain-rule evaluation;
- reuse compiler-generated penalty variables as authoritative preference/fairness score;
- iterate compiler constraint records as if they were the original rule meaning;
- share one implementation branch for compiler and verifier merely under different wrappers.

Compiler/verifier differential tests must deliberately mutate one side to demonstrate the suite detects disagreement.

### Verification report

```rust
pub struct VerificationReport {
    pub accepted: bool,
    pub evaluated_revision: u64,
    pub required_rule_results: Vec<RuleEvaluation>,
    pub score: ScoreVector,
    pub warnings: Vec<VerificationWarning>,
    pub metrics: BTreeMap<MetricId, MetricValue>,
    pub checksum: String,
}
```

Each `RuleEvaluation` identifies the domain rule and affected entities; records relevant time, seat, geometry, eligibility, or other domain facts; states expected and observed conditions; and carries a localizable message key with typed parameters. IDs and message keys—not rendered text—are persisted. Ordering/checksum generation is canonical and deterministic.

An accepted report always has `score.feasibility == 0`, all mandatory structural checks passed, and every required domain rule satisfied. Warnings cannot conceal a required-rule violation.

### Authoritative score contract

```rust
pub struct ScoreVector {
    pub feasibility: i64,
    pub levels: Vec<ScoreLevelValue>,
}

pub struct ScoreLevelValue {
    pub level_id: ScoreLevelId,
    pub value: i64,
    pub direction: OptimizationDirection,
    pub category_breakdown: BTreeMap<ScoreCategoryId, i64>,
}
```

Required rules are mandatory and are never secretly relaxed. Each preference has an explicit strength and compiles to bounded violation/satisfaction expressions, an objective level/category, and explanation provenance. The verifier recomputes these meanings from the domain solution.

Recommended workforce lexicographic order is: preserve hard locks/required rules; minimize change from a published/base schedule in repair mode; high-priority preferences; fairness/balance; ordinary preferences; deterministic tie-breakers. Recommended seating order is: required placement/separation; manual locks; requested groups together; undesirable proximity; table balance/ordinary preferences; deterministic tie-breakers.

Backend adapters may use safely bounded scalarization or multiple passes, but Phase 04 compares verified score vectors. Scalarization requires finite bound proof and rejection of overflow-prone formulations; exact multi-pass solving is preferred when weights cannot prove priority preservation. The UI displays verified category breakdowns, never an unexplained scalar backend objective.

For the current contract, a category breakdown is optional explanatory data: a pack may omit
categories whose totals it does not expose. Every category it does expose must belong to the
corresponding Planning IR objective level. Category totals are signed `i64` values and do not
implicitly sum to the level value; a pack that needs that relationship owns and verifies it from
domain meaning. Score vectors are bounded to 16 ordered levels and 1,024 exposed categories per
level. Accepted results persist only the verifier-recomputed vector. Raw backend objective and bound
evidence remains a non-gating reconciliation diagnostic and cannot establish acceptance,
optimality, or user-visible score authority.

### Provenance chain

Every user-relevant generated artifact follows this chain:

```text
Domain rule ID
  → normalized rule and affected entity IDs
  → planning variables/constraints/objective terms
  → backend IDs or proto indices
  → solver evidence
  → user-facing explanation
```

```rust
pub struct ProvenanceRecord {
    pub source_kind: ProvenanceSourceKind,
    pub source_id: String,
    pub entity_refs: Vec<DomainEntityRef>,
    pub message_key: String,
    pub parameters: serde_json::Value,
    pub parent: Option<ProvenanceRef>,
}
```

The provenance index must cover every user-relevant variable, constraint, objective contribution, assumption, and projection. Display strings are not identity. Message keys and typed parameters make evidence stable, reviewable, and localization-ready. Missing provenance is a compiler defect and prevents explanation-dependent output.

The schema-v2 planning provenance index is closed: every planning variable, constraint, objective
level and term, assumption, and projection references a declared record, and every declared record
must be reachable from one of those artifacts either directly or through its bounded parent chain.
An unused record is a compiler defect, not harmless metadata. Generic projection carries the
projection record's canonical ID into `DomainAssignment.evidence`; packs may append further
non-authoritative evidence without changing projected value identity. Backend adapters retain the
validated IR-to-native maps outside native payloads under the planning model hash and exact adapter
version. Native payloads contain neither planning IDs nor domain/display text.

Planning IR schema v2 completes the pre-public assumption contract. Each assumption ID identifies
one explainable group controlled by one Boolean literal and bound to a non-empty canonical set of
stable required `RuleId`s. A rule and controlling Boolean variable each belong to at most one group,
and every group directly references `RequiredRule` provenance. Foundational, fact, preference, and
derived provenance therefore cannot be exposed as user-relaxable assumptions. This required-field
change makes schema v1 incompatible; current writers emit only v2 and older input fails safely.

Missing source/IR/projection provenance is acceptance-critical because it invalidates the planning
problem before solving or projection. Missing optional rendering evidence discovered only after an
accepted result remains an evidence-availability defect: it disables the affected explanation but
does not revoke or delay the already verified result.

Canonical evidence ties together scenario revision, scenario/document hash, planning model hash, solution hash, pack/compiler/backend/adapter/protocol versions, and verification checksum. Cached candidates are accepted only after current verification against the current scenario revision.

## Explanation taxonomy and certainty rules

The product exposes seven distinct explanation types. API/DTO discriminants, persistence records, UI labels, and tests must keep them separate.

### 1. Validation explanation

Explains why the scenario is incomplete, malformed, or obviously contradictory before solving. It points to exact fields/entities/rules and distinguishes deterministic data/model validation from optimization infeasibility. It may state “must fix before optimizing,” “likely problem,” “review suggested,” or “information” according to typed severity. It must not imply a solver proof.

### 2. Infeasibility explanation

Explains a sufficient subset of required rules that cannot all hold. Its certainty is limited by evidence:

- use “sufficient conflict” unless minimality has actually been proven;
- say other conflicts may also exist;
- never map unknown assumption literals;
- do not claim an explanation when the backend merely ended without a candidate or at a limit;
- distinguish an unavailable/invalid assumption core from a valid proof of infeasibility;
- foundational model invariants are not presented as user-relaxable rules.

### 3. Assignment explanation

Reports verified facts associated with an assignment: eligibility, availability, named rule checks, preference satisfaction/violation, fairness/stability contribution, and lock state. It may report that competing assignments were unavailable or incompatible only when deterministic evidence proves that fact. It does not infer unique causality from correlation or backend search order.

### 4. Counterfactual explanation

Answers a bounded “Why not X?” or “What if forced/forbidden?” question by cloning the same scenario/model revision, adding a temporary condition, solving under a recorded diagnostic budget, and independently verifying any result. The only safe conclusions are:

- the alternative is proven impossible under the temporary condition;
- a verified alternative is worse by named score levels/categories/metrics;
- the diagnostic budget did not distinguish the alternatives.

Never claim impossibility from a timeout, claim a unique reason when equivalent optima exist, or present an unverified counterfactual candidate. The temporary condition never mutates the original scenario.

### 5. Solution-difference explanation

Computes added, removed, and changed assignments; required-rule status; verified score-vector deltas by level/category; fairness and preference deltas; affected people/groups/tables; lock preservation; and solver status/proof differences. Variant names such as `Balanced`, `Preference-focused`, and `Minimal changes` are labels for explicit recorded objective configuration, not narratives invented after solving.

### 6. Repair explanation

Explains why a base assignment remained, moved, was replaced, or disappeared after facts/rules changed. A claim that a new fact “made” a change necessary requires counterfactual or deterministic rule evidence. Otherwise report the verified differences and stability contribution without inventing causality.

### 7. Optimality/status explanation

States exactly whether optimality or infeasibility was proved, a verified feasible incumbent was found, a limit was reached, cancellation occurred, or no verified candidate exists. `FEASIBLE` never becomes “best” or “optimal”; Deep mode never implies a proof. Solver status and proof state remain secondary to independent acceptance.

A heuristic narrative is never labeled mathematical proof. AI may paraphrase deterministic evidence, but the typed deterministic explanation remains visible and authoritative.

## Infeasibility workflow

1. Confirm a backend infeasibility status and absence of any verified candidate; otherwise use the accurate limit/failure status.
2. Read the assumption set only if Phase 03 marked the capability valid for the exact backend build.
3. Validate literal membership/polarity and map assumptions to domain rules/groups through provenance.
4. If the set is large, run bounded deletion/re-solve shrinking: try removing one rule/group; re-solve with a small fixed diagnostic budget; keep it removed only when infeasibility remains proven; stop when budget is exhausted.
5. Label the result `minimal conflict` only when minimality under the stated grouping has actually been established; otherwise label it `sufficient conflict`.
6. Ask the pack renderer to produce typed domain evidence and safe message parameters.
7. Present conflict cards with inspect/edit actions and the option to test temporary relaxation in a reversible diagnostic copy.

A diagnostic copy may convert selected required rules to preferences through normal typed commands. Never alter the original scenario silently. OR-Tools 9.15 issue `google/or-tools#5141` can yield out-of-assumption-set literals under presolve; invalid evidence is rejected, not guessed or partially presented.

## Evidence model and persistence

Persist compact evidence required to regenerate deterministic explanations:

- assumption IDs/groups and mapped domain rules;
- objective contribution records and verified category totals;
- counterfactual request, temporary condition, budget, status, proof, score, and result hashes;
- backend status/bounds and normalized termination;
- verifier rule evaluations and domain metrics;
- scenario revision, model/solution hashes, verifier checksum, and component/backend provenance.

Each accepted or terminal solve owns a versioned **run manifest** distinct from the scenario revision, immutable normalized input/enrichment snapshot, and accepted Result Model. The immutable run input binds the request ID and canonical semantic request hash to the exact scenario revision, complete portable scenario snapshot, pack/schema/domain/Planning-IR/compiler/adapter/worker/solver/application versions, selected backend and options, model and objective-policy hashes, scenario time zone, and any temporary-condition hash. The request row and snapshot commit before backend launch. Reusing a request ID is valid only for the same canonical request; the backend always loads the persisted snapshot rather than current mutable state.

One compare-and-set transaction writes each terminal manifest exactly once. Accepted finalization writes the verifier-produced normalized solution, score, report, compact evidence, and terminal run together; any persistence failure rolls back the solution. A verification alarm writes only bounded redacted quarantine diagnostics. Every normal terminal outcome carries measured elapsed and phase timings. Crash recovery uses `Interrupted` with absent, rather than fabricated, elapsed data only after the immutable total parent deadline and a bounded terminal-persistence grace interval have expired; accepted-result persistence rejects the same cutoff, so another store instance cannot interrupt an accept-finalizable solve. A running input temporarily retains its complete portable scenario revision across edits without exporting that active-only history; accepted results keep and export the required revision, while resultless terminal runs release it.

Database schema upgrades are append-only and checksum-verified. Before upgrading an existing V1 database, the store holds the migration write lock while it creates or validates an owner-private, logically identical V1 backup and commits V2. V1 solutions become `legacy_unverified`; V1 running solves become `legacy_interrupted`; legacy payloads are preserved rather than rewritten into fabricated V2 manifests.

The fixed owner-private application database is the local authority store, not an import format. Foreign database files are never opened as application state. Portable accepted-result wrappers contain bounded checksums and exact cross-bindings for corruption detection, but those public hashes do not authenticate an issuer and never grant local acceptance authority. Every imported, restored, copied, or remapped result remains inert portable data until a fresh local independent verification against the exact local snapshot mints a new authoritative accepted result.

Do not persist full solver logs by default. Opt-in diagnostics are bounded, redacted, and separate from normal evidence. Native-worker evidence must not contain names, notes, credentials, AI content, or paths. Retention/export rules distinguish normal explanation evidence from quarantined correctness alarms.

## APIs and events

### Core/domain interfaces

The pack contract must support:

- projection from typed backend values to a normalized solution with structural error reporting;
- `verify(normalized_domain, solution, context) -> VerificationReport` independent of compiler records;
- authoritative `score(...) -> ScoreVector` or equivalent verifier-owned scoring stage;
- rendering typed validation/rule/score/assumption/comparison/counterfactual evidence into message keys and parameters;
- capability declarations for which explanation types a pack supports.

All potentially long counterfactual/shrinking operations accept cancellation and total budgets. Clock, random seed, backend launcher, and persistence are injected for deterministic tests.

### CLI surface

The working CLI name remains `optimizer` pending the explicit naming gate. Required commands use shared application services:

```text
optimizer solutions verify <scenario> <solution>
optimizer solutions compare <scenario> <solution-a> <solution-b>
optimizer solutions explain <scenario> <solution> [request options]
```

`--format json` emits one versioned result envelope on stdout; requested progress is JSONL on stderr or an explicit destination. Verification failure uses stable exit code `7`; proven infeasibility uses `4`; no verified result within limits uses `5`; backend failure uses `6`; cancellation uses `130`. Never print secrets or unbounded diagnostics.

### Tauri/application surface

Mutating requests include `scenario_id`, `expected_revision`, `request_id`; responses include request ID, current revision where applicable, warnings, and DTO schema version. Errors carry stable `code`, safe `message`, category, retryability, field errors, optional safe details, and optional diagnostic ID—never normal backtraces/Rust type names.

Required solution endpoints are:

```text
solution_list
solution_get_summary
solution_get_view
solution_select
solution_verify
solution_compare
solution_explain
solution_start_counterfactual
solution_cancel_counterfactual
solution_lock_assignment
solution_unlock_assignment
solution_create_repair_request
solution_export_preview
solution_export
solution_share_preview
solution_share_create
solution_export_cancel
```

The explanation foundation also consumes `scenario_validate`, `solve_get_job`, `solve_get_diagnostics_summary`, and stable events:

```text
solve://progress
solve://completed
scenario://validation-changed
counterfactual://progress
```

Every event carries `eventVersion`, timestamp, request/job/scenario IDs, and revision where applicable. Only `apps/desktop/src/api` invokes/listens to Tauri directly; components use generated typed composables/services.

## Explanation UX foundation

Phase 04 owns the minimum shared frontend foundation required by the eight explanation components below. Lock and configure Tailwind CSS and `@tailwindcss/vite` 4.3.3, reviewed application-owned shadcn-vue-derived component source, Reka UI 2.10.4, `@lucide/vue` 1.37.0, and only the directly used support packages: `class-variance-authority` 0.7.1, `clsx` 2.1.1, `tailwind-merge` 3.6.0, and `tw-animate-css` 1.4.0. Generated wrappers are reviewed, application-owned source rather than a second runtime design authority. The unused shadcn-vue generator is not an installed workspace dependency after the 2026-10-07 dependency review; reintroducing a generator requires a reviewed, locked graph that passes the existing audit policy. This does not replace or remove the application-owned wrappers.

The initial primitive set is Reka Dialog, Popover, Tabs, Tooltip, and Collapsible plus application-owned Badge and Card wrappers. Add another primitive only when one of the eight required components needs it. Map the existing semantic tokens into the Tailwind/shadcn variables; do not create a parallel token vocabulary or component convention. Tailwind preflight and global resets must preserve the existing base styles. Phase 06 consumes and extends this foundation instead of selecting or locking a second stack.

Build and integrate application-owned `ValidationSummary`, `ConflictCard`, `SolutionStatus`, `ScoreBreakdown`, `ExplanationPanel`, `AssignmentInspector`, `ChangeSetPreview`, and `ErrorRecoveryPanel` on shadcn-vue/Reka primitives and semantic tokens.

- An assignment inspector exposes assigned entity/work, eligibility/availability, rule checks, preferences helped/hurt, fairness/stability contribution, lock state, and `Why this?`, `Why not…?`, `Try a change` actions.
- `Why not…?` explicitly says a short diagnostic optimization may run; it has progress, cancellation, limit, stale-revision, unavailable-backend, and inconclusive states.
- Infeasibility uses the heading “These required rules cannot all be satisfied together,” renders each mapped fact/rule, says “sufficient conflict” unless minimality is proved, and offers inspect, edit, relax-in-copy, deterministic/AI paraphrase, and diagnostic-export actions.
- A result header says “Verified … ready” only after `accepted == true`; it shows all-required-rules status, accurate optimal/feasible/limit proof, verified preference/fairness summaries, base-plan changes, warnings, time/backend in secondary detail.
- Quarantined candidates show an internal verification failure state and never appear ready.
- Every state uses plain language and links to affected entities/rules. Important information is not color-only, hover-only, or canvas-only.
- Dynamic validation, solve completion, and counterfactual completion are announced to assistive technology; focus returns predictably after dialogs and cancellation.
- Explanation actions are keyboard reachable: focus/select assignment, `Enter` or explicit action opens explanation; popovers/dialogs have correct focus containment/restoration; reduced motion is respected.
- English MVP strings use message keys and typed parameters; dates, times, numbers, and units are locale-aware while scenario timezone remains distinct.

## Ordered work packages

1. **VERIFY-001 — normalized solution and verifier interface:** define projection/structural validation, report/evaluation/metric/checksum DTOs, revision/hash binding, phase timing and first-verified-feasible evidence, and accept/quarantine transaction boundary.
2. **Authoritative score engine:** implement lexicographic comparison, category contribution records, feasibility invariant, finite arithmetic/overflow checks, and backend-objective reconciliation diagnostics without trusting backend values.
3. **IR-002 completion — provenance index:** make source→IR→backend→evidence mappings canonical, complete, versioned, and localizable; fail on missing user-relevant provenance.
4. **Assumption grouping:** compile explainable required-rule groups, exclude foundational invariants, validate returned literal maps, and integrate the 9.15 issue gate.
5. **VERIFY-002 — evidence models:** define all seven typed explanation request/result kinds, certainty/proof labels, bounded shrinking, comparison, and counterfactual job records.
6. **Quarantine and persistence:** atomically separate accepted solutions from failed candidates; persist compact evidence and bounded opt-in diagnostics; enforce export/select restrictions.
7. **Application/CLI APIs:** implement verify/compare/explain/counterfactual endpoints, cancellation, revision handling, stable errors/exit codes, generated DTOs, and versioned events.
8. **Generic explanation UI:** implement accessible components and all normal, empty, loading, stale, cancelled, inconclusive, unavailable, and internal-failure states.
9. **Pack-integration conformance kit:** publish adapters, fixtures, and a synthetic test-pack harness that exercises compile→project→structural-check→verify→score→evidence→accept/quarantine without official domain logic. Phase 05 implements the first real workforce integration against this completed contract.

## Tests and acceptance

### Unit, property, and contract coverage

- accepted report iff projection/structure and every required-rule evaluation pass; accepted feasibility score is exactly zero;
- canonical ordering/checksum and stable message keys/typed parameters;
- lexicographic compare, direction handling, category totals, bounded arithmetic, scalarization reconciliation, and overflow rejection;
- revision/model/solution hash mismatch and stale-cache rejection;
- all seven explanation discriminants serialize with versioned stable DTOs;
- sufficient-versus-minimal labels, shrink budget exhaustion, cancellation, and invalid/out-of-set assumption literals;
- counterfactual outcomes: proven impossible, verified worse by categories, equivalent/undistinguished, timeout without proof, stale revision, backend failure, and invalid candidate;
- comparison covers assignment deltas, required-rule state, score/fairness/preference deltas, affected entities, locks, and proof status;
- compact evidence round-trips while full logs remain absent by default.
- run manifests reject revision/snapshot/model/result mismatches, round-trip every recorded version/option and distinguish same-model, equivalent-result, and exact-assignment reproduction claims;
- timing fixtures distinguish raw incumbent, projection, verification, first verified feasible, score/evidence persistence, and optional explanation work; the accepted result remains available when later explanation work is slow, cancelled, or unavailable.

### Independence and differential tests

- deliberately invalid candidates that satisfy backend shape but violate a domain rule are rejected and quarantined;
- intentionally mutate compiler semantics, verifier semantics, projection mapping, and score mapping one at a time and prove differential tests fail;
- enumerate all assignments for small models and compare compiler feasibility to independent verifier results;
- run generated small scenarios through compile/solve/project/verify and compare verified scores to direct domain enumeration;
- metamorphic cases include renaming IDs/display text, reordering input, adding irrelevant unavailable entities, and equivalent representations without changing semantic verification/score;
- every backend contract fixture is independently verified; an unsupported feature is rejected before solve.

### UI/API behavior

- generated DTO drift check covers reports, evidence, requests, errors, and events;
- components use typed API modules only and cannot invoke Tauri directly;
- unverified/quarantined candidates never show ready/select/export/publish actions;
- `FEASIBLE`, `OPTIMAL`, infeasible proof, timeout, cancellation, and no-candidate copy is exact;
- assignment, infeasibility, comparison, and counterfactual flows work by keyboard, restore focus, announce results, pass automated accessibility checks, and do not rely on hue/hover;
- stale/cancelled/unavailable/inconclusive/internal-failure states have text and recovery actions rather than indefinite spinners;
- CLI exit code `7` and JSON error envelope are stable for independent-verification alarms.

### Phase exit gate

Phase 04 exits only when the synthetic pack-integration harness rejects and quarantines a deliberately invalid candidate; compiler/verifier differential enumeration passes for reviewed synthetic small models and is proven mutation-sensitive; valid assumption evidence maps to synthetic domain rule IDs while invalid evidence is rejected; authoritative score/category breakdown is independent of backend objective variables; first-verified-feasible timing occurs only after the full acceptance pipeline; an accepted result is not withheld by optional explanation work; no UI/API calls an unverified candidate ready; and solver status/proof wording is exact. No workforce projection, verifier, score, evidence renderer, fixture, or other Phase 05 deliverable is required for this gate.

A core feature is done only with typed errors, cancellation/budgets where relevant, unit/property/integration coverage, migration/schema/protocol compatibility as applicable, documentation of invariants, accessibility/API impact review, and benchmark evidence when the path is performance-sensitive. An explanation capability is not done until all certainty labels, stale/error/cancel paths, deterministic evidence, and accessibility behavior pass.

## Risks and failure handling

| Risk or failure | Required behavior |
|---|---|
| Compiler and verifier share the same bug | Maintain semantic independence, mutation tests, differential enumeration, and pack-specific reviewed fixtures. |
| Backend candidate violates domain meaning | Quarantine, correctness alarm, no export/publish, safe diagnostic ID, bounded evidence retention. |
| Backend score differs from verifier score | Trust verifier, record reconciliation defect, and prevent misleading backend objective display. |
| Assumption core is non-minimal | Render “sufficient conflict”; bounded shrinking may improve it but never upgrades certainty without proof. |
| OR-Tools 9.15 returns unknown assumption literal | Reject the core and show evidence unavailable; never guess provenance. |
| Counterfactual reaches its limit | Report “not distinguished within the budget,” not impossible or equally optimal. |
| Multiple equivalent optima | Avoid unique-cause language; report verified facts and measured score effects. |
| Scenario changes during explanation | Bind output to recorded revision, cancel or label stale, and never merge evidence into the new revision. |
| Evidence payload grows without bound | Persist compact structured records; bound/redact optional logs and diagnostic artifacts. |
| Localized copy changes | Stable message keys/typed parameters preserve identity; rendered strings are not hashes/provenance. |
| AI paraphrase adds unsupported claims | Deterministic evidence remains visible/source of truth; AI output is labeled paraphrase and cannot change status. |

## Deferred and non-goals

- Workforce-specific rule evaluation belongs to Phase 05 and the remaining catalog to Phase 07; seating specifics belong to Phase 09.
- More efficient minimal-conflict shrinking, multiple independent conflict sets, richer causal graphs, and deeper interactive relaxation are post-MVP.
- Claiming globally unique causes, proof from heuristic narratives, or optimality from a feasible/limited run.
- Persisting arbitrary full solver logs by default.
- Letting AI generate, alter, or become the only view of explanation evidence.
- Making every explanation synchronous; bounded counterfactual and shrinking operations are cancellable jobs.

## Assumption and version gates

Evidence date: **2026-08-29**.

- Assumption-core support is conditional on the exact OR-Tools **9.15** build passing the Phase 03 callback/core gate and `google/or-tools#5141` handling. Core minimization diagnostics use the pinned API's required single-worker, non-optimization profile.
- No backend version can bypass independent verification. A backend without valid assumption evidence may still solve, but infeasibility explanation must honestly report unavailable evidence.
- Exact workforce fairness presets/weights and domain defaults remain practitioner/usability gates; Phase 04 provides generic bounded score semantics without freezing unsupported policy.
- Rust stays at **1.97.1** until a fixed stable newer than 1.98.0 resolves the known P-critical compiler issue.
- The project name is `eutheto`; the working CLI `optimizer` and portable file extension remain unresolved product gates. API/event/message identities are versioned independently of those labels.
