# Phase 07 — Workforce Solving, Results, Repair, and Export

## Outcome

Deliver the complete workforce-scheduling MVP vertical slice: every official required rule and preference compiles, solves through the default OR-Tools path, is independently verified and explained, appears in accessible result and repair workflows, and round-trips through the supported imports and exports. Every published or exported solution is an accepted, revision-matched, independently verified solution.

This phase completes the behavior begun in Phases 05–06. It does not weaken the separation between domain meaning, planning IR, backend translation, and independent verification.

## Source coverage

This phase is the implementation source of truth for blueprint Sections 17–18; workforce solve/result portions of Section 22; Phases 7 and the applicable Phase 5–6 prerequisites; Sections 31.2–31.4, 32, and 33.1–33.4; CLI Appendix C; Tauri API Appendix D; backlog items `WF-004` through `WF-008`, `UI-005`, `UI-006`, `EXPLAIN-001`, `REPAIR-001`, and `EXPORT-001` from Appendix I; the workforce delivery slice in [Performance and Solver UX Targets](performance-and-solver-ux-targets.md); and the generic/workforce report slice in [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md).

Project-wide contracts and sequencing are in [README.md](README.md). Version and evidence gates are in [assumptions.md](assumptions.md). This phase depends on [Phase 02](02-domain-pack-and-planning-ir-contracts.md), [Phase 03](03-ortools-worker-vertical-slice.md), [Phase 04](04-independent-verifier-and-explanations.md), [Phase 05](05-workforce-core-vertical-slice.md), and [Phase 06](06-desktop-design-system-and-workforce-setup.md). Backend diversification follows in [Phase 08](08-pumpkin-backend-and-router.md); seating reuses these result/repair contracts in [Phase 09](09-seating-domain-and-venue-experience.md); AI may invoke only the same typed commands in [Phase 10](10-ai-assistant-mvp.md).

## Dependencies and entry conditions

- Workforce entities, time-zone/DST semantics, shift generation, stable IDs, migrations, eligibility, availability, coverage, no-overlap, and minimum-rest foundations exist from Phase 05.
- Revisioned command batches, undo/redo, scenario persistence, planning IR, provenance, score vectors, canonical model hashes, solution projection, and verifier interfaces are stable enough to extend without a second convention.
- The OR-Tools worker protocol, process supervision, cancellation, resource limits, status normalization, and candidate projection work end to end.
- Phase 04 rejects deliberately invalid candidates and persists deterministic verification/explanation evidence.
- The Phase 06 desktop shell, rule builder, validation UX, import preview, generated Rust-to-TypeScript API boundary, keyboard/focus infrastructure, and semantic design tokens exist.
- The working CLI executable name `optimizer` remains an unresolved example. `.eutheto` is the proposed portable extension pending the Phase-11 identity ADR; implementation and public documentation use the final decision consistently. The project/package prefix is `eutheto`.

## Decisions and invariants

1. **Required means acceptance-critical.** A required rule violation always rejects a candidate. A preference is a bounded objective contribution and can never silently change required feasibility.
2. **Verification is independent.** Backend values are projected into a normalized domain solution, structurally validated, evaluated against every original domain required rule, and rescored independently. Backend status and compiler penalty variables are not authoritative.
3. **Verification failure is quarantined.** Store the raw result only in bounded, redacted diagnostics; fail the run; show an internal-correctness error; disable select, publish, repair-from, and assignment/result/solution-derived export actions for the candidate. Project/scenario backup bundles and diagnostic/infeasibility reports remain available when their own source data is valid and must not imply candidate acceptance.
4. **Revision identity is explicit.** Jobs, events, solutions, verification reports, explanations, exports, and counterfactuals carry scenario revision and relevant model/solution hashes. An older-revision result remains inspectable but is visibly stale and cannot masquerade as current.
5. **Time is unambiguous.** Rest, overlap, and actual-hours rules operate on elapsed instants unless a rule explicitly names scheduled/local hours. Recurring generation applies the scenario’s reviewed DST policy and reports wall-clock versus elapsed differences.
6. **Fairness is configured, not asserted.** Show the chosen population, workload bucket, target/share computation, actual distribution, deviations, and spread. Never present one opaque “fairness percentage.”
7. **Repair semantics are explicit.** Hard locks cannot change; soft-lock changes incur a high stability penalty; unlocked assignments can change. Search hints never replace locks or stability objectives.
8. **No exact-schedule assertions by default.** Equivalent valid schedules are expected. Tests assert status, verification, score invariants, metrics, and stable explanation keys unless deterministic settings intentionally select one arrangement.
9. **Solution-derived exports use accepted solutions only.** Assignment/result/solution-derived export preview identifies scenario/solution revision, verification checksum, included data, and stale status. A stale but previously accepted result may be exported only after an explicit warning and accurate labeling; an unverified or rejected candidate never may drive those exports. Project/scenario backup bundles and diagnostic/infeasibility reports have separate input-validity gates and require no accepted solution.
10. **Human language is primary.** Use `Required`, `Preference`, `Optimize`, and `Repair plan`; backend jargon is confined to advanced diagnostics.
11. **One bounded responsive operation.** Validation, compilation, backend work, fallback, projection, verification, and result preparation consume one end-to-end budget. An accepted incumbent is available immediately; proof search or optional explanation/AI work cannot withhold it.
12. **Share data is purpose-built and immutable.** Standalone reports and PDF use a versioned privacy-filtered Share Result Model derived from one accepted result and exact share options, never the full Scenario Model. The same preview payload drives every renderer.

## Complete workforce scope

### Domain data completed or consumed

`WorkforceScenario` contains planning horizon, calendars, people, qualifications, locations, assignment types, shift templates and instances, coverage requirements, availability, required rules, preferences, optional base schedule, assignment locks, and the workforce score policy.

The solve/result implementation must preserve these facts:

- People use stable IDs, never names as identity keys; include display name, active range, qualifications/eligibilities, eligible assignment types, optional home location, optional contract/workload target, tags/teams, and display-only color/avatar metadata.
- Qualifications are dated capabilities, including effective and expiration dates.
- Assignment types define semantic category, time behavior, default duration, eligible qualifications, location behavior, and configured workload-bucket counting.
- Shift instances retain template ID when generated, assignment type, location, zoned start/end, required/preferred/maximum counts, required qualification sets, and tags.
- Detached one-off instances survive regeneration; regeneration shows a diff and never overwrites manual edits without confirmation.
- Availability supports unavailable intervals, weekly recurrence, available-only windows, approved leave, effective ranges, assignment-type restrictions, location restrictions, source, and note; normalized rejection provenance remains inspectable.

### Exhaustive required-rule catalog

Each rule below requires a stable schema/ID, migration handling, command DTO, validation, sentence-like editor, planning-IR compilation and backend tests, independent evaluator, provenance/explanation, CLI/document example, boundary and infeasible fixtures, import/export preservation, model-size review, and typed AI exposure only when appropriate.

1. **Eligibility:** assign only when every required qualification and assignment-type eligibility condition holds, including effective/expiration dates.
2. **Availability:** reject a shift intersecting an unavailable period or outside an available-only window, including recurring, approved-leave, type, location, and effective-range filters.
3. **Coverage:** meet exact, minimum, and bounded maximum staffing for every shift and every required qualification slot.
4. **No overlapping assignments:** one person cannot hold overlapping shifts unless explicit configuration declares the two categories compatible and defines counted hours.
5. **Minimum rest:** require elapsed duration between a matching source assignment’s end and target assignment’s start; scope may filter source/target assignment types, people, teams, weekdays, or locations. The reference fixture enforces 10 hours after overnight call before clinic.
6. **Maximum hours in a fixed period:** cap configured counted hours per day, week, pay period, or custom calendar interval.
7. **Maximum hours in a rolling window:** cap counted hours in every rolling duration, such as 80 hours in any 14-day window. Generate candidate windows only where a shift start/end changes the sum, and enforce model-size limits.
8. **Consecutive assignments/days:** cap consecutive worked days, nights, or matching assignment types.
9. **Maximum assignment count:** cap matching assignment counts in a configured period.
10. **Required skill mix:** require at least the configured number of assigned people satisfying each qualification expression.
11. **Fixed assignment and hard lock:** force the named assignment and preserve hard-locked base work during repair.
12. **Mutual assignment restriction:** keep configured people apart on the same shift or on overlapping matching shifts where this is a legitimate operational rule.
13. **Transition/travel time:** require configured setup/travel duration between assignments at distinct locations using the MVP location-pair duration matrix; no map service is needed.

Empty or default scopes must be safe. An empty scope cannot be saved accidentally; an intentional inactive state is explicit and never compiles as a surprising global rule.

### Exhaustive preference catalog

Every preference has a priority and bounded weight. The visible labels `Low`, `Normal`, `High`, and `Very high` map through the persisted score policy; numeric backend coefficients are not the user contract.

1. Prefer or avoid a person-specific date/time.
2. Prefer or avoid an assignment type.
3. Prefer a location.
4. Honor requested time off when it is not Required.
5. Keep assignment count near a target.
6. Balance a named workload bucket across a peer group.
7. Avoid consecutive nights or weekends.
8. Prefer or avoid assignments adjacent to existing work.
9. Keep a base schedule stable.
10. Keep configured people together or separate when operationally appropriate.
11. Prefer coverage above the required minimum.

### Fairness and score policy

The setup and result views allow the user to balance any explicit combination of:

- assignment count;
- counted hours;
- overnight call;
- weekends;
- holidays;
- undesirable shifts;
- deviation from each person’s target or FTE-weighted share.

Use lexicographic/priority score levels so required feasibility is outside the preference score, high-level stability can dominate ordinary preferences during repair, and all contributions are bounded. The recommended default fairness penalty is convex target deviation via absolute deviation or reviewed piecewise penalties. Unequal contracts use configurable workload weights; equal raw shares occur only when explicitly selected. Persist score-policy/profile IDs and complete objective configuration.

Named alternatives map to recorded objective profiles, not labels alone:

- `Balanced`;
- `Preferences first`/`Preference-focused`;
- `Minimal changes`;
- `Fairness first`.

Run profiles sequentially in the MVP, verify every candidate, compare authoritative score vectors and visual diffs, and suppress alternatives that differ only through arbitrary tie-breaking without meaningful metric change.

### Practitioner scheduling clarifications

**Status: requirements clarification approved; practitioner answers pending.** These distinctions supplement the baseline catalogs above; they are not implemented capabilities, new schema variants, approved policy defaults, or permission to move Phase 07 production behavior into Phase 05. Resolve them before claiming that the complete Workforce MVP supports the corresponding clinic-and-call workflow. An unanswered question never becomes an implicit default.

| Requirement to resolve | Existing boundary and required distinction | Decisions needed before implementation |
|---|---|---|
| Required workload and assignment totals | Maximum-hour/count rules and preferred targets do not express every minimum, exact, or ranged requirement. A quota derived incidentally from coverage and other totals is not a substitute for an independently authored requirement. | Determine which totals are Required versus preferred, their scope and period, permitted deviation, credited call/clinic/leave/administrative work, breaks, overtime, pay-period anchor, overnight attribution, and scheduled versus elapsed/DST accounting. Workload accounting does not add a payroll/vendor adapter. |
| Distinct weekends worked | Assignment/hour counts, distinct weekends touched, consecutive weekends, and completely free weekends are separate measures. The current workload measurement vocabulary and consecutive-weekend preference do not by themselves establish all these behaviors. | Define weekend boundaries and membership, what makes a weekend worked, count/participation targets or caps, eligible peer groups, equal versus contract-weighted shares, separate day/night/undesirable-shift burdens, and whether imbalance carries across rotations. |
| Work and time-off grouping | Adjacency to a base schedule is not general grouping of newly selected assignments. Avoiding consecutive nights does not specify desirable work blocks or uninterrupted time off. | Define worked/reporting days, isolated work, block boundaries and gap units, desired or maximum block lengths, whether short work interrupts time off, horizon-edge context, and priority relative to recovery and other preferences. |
| Optional split-clinic arrangements | Predefined shorter shifts are different from selecting a linked alternative between one full session and multiple partial sessions. Supporting the former does not establish automatic choice of the latter. | Determine whether splitting is required or optional, who may form a pair, ordering and handover, whether the optimizer selects the date, frequency limits per person/pair/rotation, and whether other sessions must remain whole. Confirm exclusive alternatives and continuous coverage without double-counting workload. |
| Combined practitioner acceptance case | Generic clinic fixtures and isolated rule tests do not prove the agreed combination of periods, coverage, restrictions, preferences, and repair works end to end. | Agree a synthetic scenario and expected semantic outcomes after the relevant answers are recorded. Include operator-visible totals and tradeoffs, infeasible and time-limited outcomes, boundary behavior, repair, and required existing export views. |

Collect clarification in this priority order:

1. **Validity:** consistent fictional roster references and eligibility; exact coverage/start/end times and holiday exceptions; contractual totals and credited work; pay-period and overnight/DST attribution; directional clinic/call and call/call recovery, continuous-duty limits, and consecutive-work restrictions; fixed assignments and independent quotas versus current practice.
2. **Tradeoffs:** weekend definition, distinct participation, fairness population and equal/weighted sharing; free/consecutive weekends and carryover; good/bad work-block examples; split-session alternatives; person-specific priorities and which compromises require explicit approval.
3. **Operation:** leave and vacancies, authorized relief/overtime and non-negotiable requirements; protected assignments and repair costs measured in shifts versus affected people; prior/next-rotation context; required schedule, personal, period-total, burden and exception views, using the existing MVP export scope.

Record each disposition here as a confirmed requirement with explicit semantics and acceptance criteria, or as explicitly not applicable with rationale. Maintainer approval is required for any scope change; silence or an unresolved answer cannot justify dropping a confirmed requirement or claiming complete workflow support. Answers are practitioner evidence, not legal/compliance certification or universal defaults. Until resolved, keep applicable implementation blocked while continuing independent, already-defined work.

**Ownership and cutover:** WF-004 owns confirmed Required semantics; WF-005 owns confirmed preference/fairness semantics; WF-006/WF-007 own compilation, independent evaluation, authoritative metrics and score; UI-006, REPAIR-001, EXPORT-001 and acceptance hardening expose and prove the resulting behavior. Reuse these work-package IDs and existing abstractions. Choose the smallest typed domain extension only after the relevant decisions; reconcile schemas, real migration impact, generated commands/editors, backend capabilities, compatibility and fixtures together under the existing rule checklist. Do not reinterpret an existing field/tag or invent a universal rule language.

**Privacy:** no supplied practitioner roster, real initials, organization/location, dated title, source schedule, identity mapping, or identifying notes enter repository fixtures, documentation, snapshots, or CI artifacts. Initials replacement alone is insufficient. Construct deliberately synthetic identities, dates, assignments and distinctive personal patterns while preserving the confirmed scheduling semantics; review the combination for recognizability before committing it. Store only the non-identifying decisions and synthetic evidence, not a reconstructable copy of the source example.

### Compilation and model construction

For each statically feasible person/shift pair, create `x[person, shift] ∈ {0,1}`. Do not allocate variables for impossible pairs after eligibility/static-availability preprocessing, but retain rejection provenance for explanations.

Core formulations include:

- coverage: `min ≤ Σ x[p,s] ≤ max`;
- person overlap/rest conflicts: `x[p,s1] + x[p,s2] ≤ 1`;
- counted-hour limits: `Σ duration_bucket[s] × x[p,s] ≤ limit`;
- skill mix sums over only people satisfying the qualification expression;
- preferences compile to bounded penalty terms and never alter required coverage.

Benchmark optional intervals/no-overlap against pairwise conflict cliques and cardinality constraints for fixed shifts. Apply symmetry breaking only where person distinctions are truly interchangeable; qualifications, preferences, teams, and identity-visible effects prevent arbitrary ordering.

### Validation

Fast validation after edits covers:

- unique IDs and names where required;
- valid time ranges;
- existing references;
- non-negative counts and hours;
- non-empty or explicitly inactive rule scopes;
- templates generating within the horizon;
- existing qualifications referenced by coverage.

Full pre-solve validation additionally covers:

- enough statically eligible candidates for every required shift;
- internally possible qualification coverage slots;
- individually valid hard locks;
- overlapping hard locks;
- obvious minimum-rest conflicts among hard locks;
- hours rules already violated by locks;
- non-empty target/fairness groups;
- rolling-window generation within configured model/resource limits.

Validation issues are grouped as `Must fix before optimizing`, `Likely problem`, `Review suggested`, and `Information`; each uses plain language, names affected entities, deep-links to the editor/field, offers only deterministic safe bulk fixes, and distinguishes malformed/incomplete data from proven optimization infeasibility.

## Solve jobs and result experience

### Optimize flow

Before `Optimize`, show scenario revision and validation status, Quick/Balanced/Deep mode, optional maximum time, repair/base-plan state, and backend only in advanced disclosure. Quick is short bounded search for a verified feasible plan; Balanced is the default budget/quality profile; Deep uses a longer search and may seek more alternatives but never promises optimality. The provisional Balanced hypothesis places approximately 2–3 seconds of CP-SAT time inside a 3–5 second end-to-end interactive budget; Phase 12 calibrates the shipped values from representative whole-pipeline evidence.

Detailed events remain available to bounded diagnostics, but the normal UI maps them to truthful coarse phases. Operations completing below approximately 300–500 ms do not flash progress. Longer operations show validation, candidate preparation, transportation when applicable, model build, optimization, verification, and result preparation only when that work is active. Percent is absent unless a real denominator exists. Raw callbacks are coalesced for rendering and assistive technology; cancellation and navigation remain available.

The application may report that a backend incumbent was found without calling it valid. `Found a valid plan` and `Verified schedule ready` require completed independent verification. A same-scenario edit either cancels the job or allows it to finish against its recorded revision and labels the result stale. Normalize final results as `Optimal`, `Feasible`, `Infeasible`, `Unbounded`, `NoSolutionWithinLimit`, `Cancelled`, `InvalidModel`, `BackendUnavailable`, or `BackendFailed`; map backend `Unknown` from verified-incumbent presence and termination cause, never `Feasible` to `Optimal`.

### Independent verification and scoring

Acceptance pipeline:

```text
backend values
→ normalized workforce solution projection
→ structural solution validation
→ independent evaluation of every required workforce rule
→ independent score recomputation
→ accepted solution
```

`VerificationReport` records acceptance, evaluated revision, every required `RuleEvaluation`, authoritative `ScoreVector`, warnings, typed metrics, and checksum. A rule evaluation names affected entities, relevant time facts, expected and observed conditions, and a localized stable message key. Shared time/interval/value-object utilities are allowed; re-querying the backend model, trusting backend status, or using compiler penalty variables as authoritative scores is prohibited. Differential mutation tests must prove compiler/verifier disagreement is detected.

### Result views

Implement virtualized views rather than a DOM cell per large horizon:

- schedule/calendar grid by person;
- schedule grid by shift and location;
- person detail timeline;
- uncovered and overcovered work;
- required-rule summary;
- preference-satisfaction summary;
- fairness distribution with targets, actual values, deviation, and spread;
- changes from base schedule;
- pack-owned Plan Health indicators for applicable rules—rest above the configured minimum, hours below a configured limit, transition/travel margin, and coverage margin—with typed units, affected assignments, and no universal health score;
- warnings and accurate solver/proof status;
- explanation side panel.

The result header states an accurate `Verified schedule ready` alternative, that all required rules passed, optimal/feasible/limit status, secondary time/backend details, preference/fairness metrics, base-plan changes, and warnings. Design normal, empty, loading, stale, active, cancelled, backend-unavailable, no-solution, older-revision, and internal-verification-failure states. No indefinite spinner lacks text, cancellation, or timeout behavior.

### Performance and quality evidence

For every benchmark run, persist bounded local metrics for validation, candidate generation/pruning, Planning-IR build, backend translation/startup/solve, first incumbent, first verified feasible, verification, result post-processing, primary UI render, and total time, plus model counts, cache/fallback state, termination/proof, backend objective/bound, and authoritative score vector. Names, notes, scenario payloads, arbitrary logs, and provider content never enter timing records.

The initial workforce objectives are <500 ms end to end for the approved small warm fixture, target <1 second warm and usually <3 seconds cold for the approved typical fixture, p95 <5 seconds over the normal corpus, and a bounded responsive experience for large/stress fixtures. They remain provisional until Phase 12 records the exact reference machine and distribution. Phase 07 supplies reproducible small/typical/stress manifests and packaged-desktop evidence; it never turns one fast backend microbenchmark into a product claim.

The result shell renders as soon as the first candidate completes projection and independent verification. Continued bounded improvement may replace the displayed best verified result through a versioned event; optional detailed explanations and AI paraphrase load independently and cannot delay the first usable result.

### Explanation catalog

Persist and render all seven explanation categories:

1. validation: incomplete or contradictory scenario before solve;
2. infeasibility: sufficient subset of Required rules that cannot coexist;
3. assignment: verified eligibility/availability facts and preference/score effects;
4. counterfactual: effects of temporarily forcing or forbidding an assignment;
5. solution difference: changed assignments and score/proof differences;
6. repair: why a prior assignment changed after new facts;
7. optimality/status: proof versus bounded incumbent.

An assignment inspector shows assigned person/work, eligibility, availability, rule checks, preferences helped/hurt, fairness/stability contribution, lock state, and actions `Why this?`, `Why not…?`, and `Try a change`.

Why-assigned statements are limited to verified facts: eligibility; unavailable/incompatible competing assignments; named preference satisfaction/violation; calculated fairness/stability deltas. Distinguish validating an exact move with all other assignments fixed from a global counterfactual that allows the remaining assignments to change. `Why not Alice instead?` records the selected question, clones the same revision, adds the corresponding temporary force/forbid condition, uses a bounded diagnostic budget, independently verifies any candidate, and reports verified feasibility, proven infeasibility, or unresolved search. A worse candidate found within the budget does not prove every alternative is worse. Never claim unique causality across equivalent optima or necessity without proving the appropriately scoped negation.

For infeasibility: confirm no verified candidate; read an available assumption core; map assumptions through provenance; bounded-shrink an oversized core by removing a rule/group and re-solving within a small diagnostic budget; retain removal only while infeasibility remains proven; stop on budget exhaustion; render pack-owned evidence and label it `sufficient conflict`, never `minimal` unless proved. Offer inspect, edit, temporary relaxation in a reversible diagnostic copy, or export diagnostic report. Never alter the original.

Comparison computes added/removed/changed assignments, required-rule status, score-vector deltas by level/category, fairness deltas, preference deltas, affected people/groups, lock preservation, and solver status/proof differences.

Compare aggregate scores only when objective definitions, units, population/scope, weights, scaling, and priority order are compatible. Otherwise use a supported common evaluation policy or explicitly comparable raw metrics and label unavailable comparisons; never present unlike scores as universal utility. Record the baseline, result, query scope, proof/termination state, and evidence bindings so later text or voice clients can explain the exact established claim. A valid evidence ID alone does not substantiate arbitrary prose.

Phase 07's Scenario Compare scope is intentionally bounded: compare the current accepted result with the immediately preceding accepted result for the same scenario. Show the complete semantic difference above, preserve both result/run/revision identities, and let the user reopen or restore the prior accepted plan through existing revisioned commands. Arbitrary revision selection, named milestones, branching, reconciliation, generalized criticality, and cross-pack health semantics remain post-MVP.

Persist compact assumption-to-rule mappings, objective contribution records, counterfactual summaries, backend bounds/status, verifier evaluations, and model/solution hashes. Full solver logs are opt-in and bounded. A later AI paraphrase never replaces the deterministic evidence view.

## Repair and manual edits

A base assignment is `Hard locked`, `Soft locked` (`try to keep`), or `Unlocked`. Compile a change indicator for every base assignment and minimize changes at a high objective level. A previous solution may be a search hint, but correctness comes from explicit locks and stability penalties.

Before committing an invalid drag/reassignment, explain the violated rule and offer cancel, edit a related rule, convert the request to a preference and re-optimize, replace a conflicting assignment, or create a diagnostic copy. A valid manual change is a revisioned, undoable command and may remain unlocked, soft locked, or hard locked. `Repair plan` becomes primary after edits and defaults to minimum change.

The repair diff highlights unchanged, moved/replaced, newly unfilled, and newly assigned work, plus the new rule or fact requiring each explainable change. Hard locks must survive; any impossible hard-lock set is rejected before solve with actionable evidence.

## Imports and exports

### Imports

Support people CSV, eligibility CSV or pasted matrix, shift-instance CSV, availability/time-off CSV, and existing-assignment CSV. Every importer follows choose file, safe format/encoding detection, column mapping/bundle summary, additions/updates/duplicates/rejected-row preview, stable-identity matching, proposed-change validation, one atomic undoable command batch, and a report with downloadable rejected rows. Never partially mutate or merge similar names silently.

### Exports and result sharing

Support all MVP outputs under distinct input-validity gates:

- editable scenario/full backup through the proposed `.eutheto` portable services from Phase 01, including complete workforce portable meaning;
- assignment CSV;
- people/shift summary CSV;
- iCalendar `.ics`, per person or combined;
- one self-contained interactive HTML result report;
- direct PDF from the same validated share payload/renderer;
- JSON through the CLI; and
- bounded, redacted diagnostic/infeasibility report.

Assignment CSV, result-derived summaries, ICS, HTML, PDF, and solution-bearing CLI JSON require an accepted independently verified result. A stale but previously accepted result may export only after explicit warning and immutable old-revision labeling. Editable scenario/full backup remains available before solve and after infeasibility when its portable source is valid; included results retain exact accepted/unaccepted status and cannot elevate a candidate. Diagnostic/infeasibility output uses its own bounded valid evidence gate and states revision, run/status, proof limits, and whether infeasibility was proved.

#### Share Result Model and renderer

`Share Result Builder` consumes the immutable accepted Result Model, scenario/result/revision/verification identities, chosen profile/options, workforce recipient views, and provider/redaction policy. It emits a strict versioned inert-data model with plan version, generation/application/solver/report versions, schedule/person/location/transportation sections as available, safe explanation/status/provenance, and explicit privacy flags. It defaults to excluding source calendar titles, full addresses, private notes, complete constraint graphs, rejected alternatives, raw objective/solver details, diagnostics, and all scenario fields not needed by the recipient.

The generic report/component boundary introduced here has no Tauri, SQLite, filesystem, credential, provider, solver, or mutable-scenario dependency. Desktop and standalone renderers reuse accessible result view models/components where sound; desktop-only actions are injected explicitly. The workforce report provides timeline, By Person, By Location/shift, coverage, preference/fairness, changes and transportation views only when their selected data exists. Every graphical summary has a list/table equivalent.

The HTML report is one file that opens from `file://` in supported modern desktop browsers, requires no Eutheto/server/account/browser storage, and works offline. It embeds project-owned code/styles/small approved assets plus safely encoded Share Result data and print CSS. Core meaning makes zero network request and has no CDN, remote font/script/icon, analytics/tracker, authentication, or map-tile dependency. Explicit external links require a visible action and are never necessary to understand the plan.

Presentation interactions may switch views, filter/highlight/search, select a day/person/location, expand details/evidence, adjust density, and print. They alter only local view state. The report cannot edit a scenario, apply a command, rerun the solver, imply filtering changes the plan, load scenario-authored code, or silently contact a service.

User-authored strings remain inert encoded data and render through text nodes. Validate decoded data; escape container terminators; avoid `innerHTML`, `eval`, dynamic functions, and scenario-to-JavaScript concatenation; use a restrictive self-contained-report CSP. An `About this plan` section records scenario/revision, result ID, friendly plan version, generation time, versions, verification/status basis, and applicable snapshot assumptions. Source edits never alter an existing report.

#### Privacy preview, print, and PDF

The intent-led UI separates **Share Result** from **Export editable scenario** and **Back up everything**. MVP has one strong participant-friendly default plus advanced allowed field/section switches; it reserves coordinator/full-analysis/custom profile IDs for later or separately justified delivery. The preview renders the exact Share Result payload and lists whether names/aliases, full addresses, source calendar titles, notes, constraint details, rejected alternatives, objective details, transportation, and external links are included. Potentially sensitive options default off.

Reviewed print styles remove controls without paper meaning, retain date/plan version/status, avoid clipped schedules, control page breaks/repeated context, support grayscale, and include concise provenance. Direct PDF uses the same validated Share Result Model and controlled local report/print renderer; it cannot have a broader privacy model or remote dependencies. Atomic temporary output and cleanup apply to HTML/PDF.

CSV and ICS schemas remain deterministic, stable-ID-aware, time-zone explicit, escaped correctly, and covered by semantic round-trip/consumer fixtures. XLSX, payroll/vendor adapters, live calendar synchronization, encrypted/signed bundles, automatic backups, hosted share links, and report annotations are post-MVP.

## Application and CLI contracts

### Tauri API

Use revisioned request/response envelopes. Mutations carry `scenario_id`, `expected_revision`, and `request_id`; responses carry request ID, current revision, warnings, and schema version. Errors are typed/actionable and never expose Rust backtraces.

Required solve endpoints: `solve_get_backend_options`, `solve_estimate_model`, `solve_start`, `solve_cancel`, `solve_get_job`, `solve_list_runs`, and `solve_get_diagnostics_summary`.

Required solution endpoints: `solution_list`, `solution_get_summary`, `solution_get_view`, `solution_select`, `solution_verify`, `solution_compare`, `solution_explain`, `solution_start_counterfactual`, `solution_cancel_counterfactual`, `solution_lock_assignment`, `solution_unlock_assignment`, `solution_create_repair_request`, `solution_export_preview`, `solution_export`, `solution_share_preview`, `solution_share_create`, and `solution_export_cancel`. Lock convenience endpoints issue normal scenario commands.

Events: `solve://progress`, `solve://completed`, `scenario://changed`, `scenario://validation-changed`, `counterfactual://progress`, `export://progress`, `export://completed`, and application notifications. Only typed frontend API modules invoke Tauri directly.

### CLI

The working command form is `optimizer` pending the CLI naming gate. It runs without desktop state, uses the same application/domain/backend/verifier services, supports human and JSON output, sends diagnostics separately, never prints secrets, accepts explicit paths, and preserves stable exit codes.

Relevant commands are `scenario validate`, `scenario apply`, `projects import/export`, `backup inspect/create/restore`, `solve`, `solutions list`, `solutions verify`, `solutions compare`, `solutions explain`, and `solutions export --format csv|ics|html|pdf|json`. They reuse the same portable/share services, accepted-result/privacy gates, and output staging as desktop. Solve options remain backend override, quick/balanced/deep mode, max time, threads, seed, first feasible, repair-from solution, output, bounded diagnostics directory, and human/JSONL/none progress.

Exit behavior: `0` produced the requested result; `3` validation failure; `4` proven infeasible; `5` no verified result in limits; `6` backend unavailable/incompatible/failed; `7` candidate verification/correctness alarm; `8` file/bundle/database/migration failure; `10` revision/state conflict; `130` user cancellation. Proven infeasibility is a completed domain outcome, not an internal crash.

The JSON envelope uses the `eutheto` API namespace, command, `ok`, normalized status, result, warnings, and optional diagnostic ID. The exact CLI schema version is pinned with compatibility fixtures.

## Ordered work packages

1. **WF-004 — Complete required rules:** implement hours, rolling windows, consecutive, maximum count, skill mix, locks, mutual restrictions, and travel transitions through schema-to-explanation slices.
2. **WF-005 — Complete preferences/fairness/stability:** bounded score contributions, FTE/target policy, named profiles, score serialization, metric views, and small-model differential checks.
3. **WF-006/WF-007 — Complete compiler and independent verifier:** preprocessing provenance, formulations, rejection paths, all rule evaluators, authoritative score, mutation tests, and verification quarantine.
4. **UI-005 — Solve orchestration:** revisioned job service, one parent budget, 300–500 ms progress threshold, truthful coarse phases, event/announcement coalescing, cancellation, first-verified-feasible delivery, stale-result handling, normalized status/error UI, phase/quality metrics, and advanced disclosure.
5. **UI-006 — Results:** virtualized grids/timelines, coverage, preference, fairness, base-diff, immediate accepted-result summary, independently loading detail/explanation, empty/error states, keyboard and screen-reader paths.
6. **EXPLAIN-001 — Explanation and diagnostics:** assignment evidence, bounded counterfactual jobs, infeasibility mapping/shrinking, deterministic comparisons, and compact persistence.
7. **REPAIR-001 — Locks and repair:** base-solution references, lock commands, high-level stability objective, manual-edit validation, repair diff, one-step undo.
8. **Alternatives:** sequential recorded objective profiles, verified metrics, semantic deduplication, comparison UI.
9. **EXPORT-001 — Portable/workforce output and shared report suite:** workforce current/historical portable round trips; separate editable-backup/result/diagnostic gates; accepted-only Share Result builder; privacy-exact preview; reusable offline-safe view components; one-file `file://` HTML; safe inert embedding and restrictive CSP; direct PDF/print; assignments/summary CSV; ICS; CLI JSON; atomic generation and deterministic/security/browser/accessibility fixtures.
10. **Acceptance hardening:** all workforce fixtures, warm/cold whole-pipeline benchmarks, target/regression evidence, large-grid profiling, task-based usability scripts, docs and generated contracts.

The [practitioner scheduling clarifications](#practitioner-scheduling-clarifications) are decision gates within these packages, not a new phase or permission to skip existing catalog entries. Resolve each affected gate before implementing or enabling its semantics; unrelated completed prerequisites remain usable.

## Tests and acceptance fixtures

Checked-in minimum corpus:

1. **Clinic only, small:** 8 people, 4 weeks, availability and fairness.
2. **Clinic plus overnight call:** 12 people and cross-category 10-hour rest.
3. **Rolling-hours stress:** overlapping candidate windows and hard locks.
4. **Qualification coverage:** one specialist required per session.
5. **Repair after call-out:** accepted/published base schedule plus one new unavailability.
6. **Provably infeasible:** insufficient eligible coverage with mapped sufficient conflict.
7. **DST transition:** overnight work across both spring and fall transitions.
8. **Large benchmark:** configurable 100+ people and thousands of candidate assignments.
9. **Synthetic practitioner workflow:** after clarification, combine the confirmed clinic/call, pay-period, weekend, grouping and split-session requirements from the [decision ledger](#practitioner-scheduling-clarifications). Use no real or merely relabeled source schedule.

For every fixture assert domain validation, expected normalized status, independent verification, required-rule outcomes, score invariants, stable explanation keys, import/export preservation, and revision/hash identity. Add valid, invalid, edge, empty-scope, period-boundary, lock-conflict, cancellation, stale-result, and resource-limit cases per rule. Differential exhaustive small models compare compiler results with the independent evaluator; intentional mutation proves detection.
Evidence fixtures distinguish fixed-other-assignment conflicts from globally feasible rescheduling, a short-budget unresolved run from proven infeasibility, and changed objective policies from comparable score deltas. Deterministic UI/CLI renders these distinctions without AI; conversational experiments and hypothetical entities remain in Phase-13 Branch K rather than expanding Phase 07's bounded comparison scope.
Performance assertions use the versioned benchmark manifests and record raw distributions/artifacts rather than one best run. Tests cover sub-threshold no-flicker behavior, longer truthful progress, one parent deadline across fallback/diagnostics, first-incumbent versus first-verified-feasible ordering, immediate accepted-result availability, optional-explanation delay/failure, cancellation latency, event/announcement caps, UI long tasks, and the calibrated small/typical/stress thresholds.
Export tests prove that unverified/rejected/no-solution runs cannot produce assignment/result/solution-derived outputs, while valid editable scenario/full backup and bounded diagnostic/infeasibility output remain available before acceptance. Workforce portable current/historical round trips preserve every rule/reference/time meaning; unknown semantics fail and declared nonsemantic extensions preserve.

The synthetic practitioner case jointly optimizes clinic and call even when views separate them. Independently check every confirmed Required total and restriction, report period/burden metrics and preference tradeoffs, and exercise fixed work plus a repair. Include boundary and infeasible cases sensitive to hour/count granularity, distinct-weekend versus shift counting, new-work versus base adjacency, and exclusive split-session coverage where those requirements are confirmed. Aggregate hours balance alone does not prove a feasible assignment; resource exhaustion is not infeasibility. Freeze explicit reviewed policies and semantic expectations, not an arbitrary exact roster. Missing practitioner answers cannot be replaced by invented passing expectations.

Share/report tests prove the preview payload exactly equals the rendered Share Result fields; sensitive/source-only fields default absent; malicious names/notes cannot inject markup/script/navigation; one HTML file opens from `file://` with no server/storage and zero required network requests; principal views/filter/search/expansion plus keyboard/focus/screen-reader operation work; graphical content has list/table parity; source edits cannot alter a generated report; print/PDF preserve readable grayscale/page context/provenance; generation cancellation/failure removes staging; supported-browser and large-report responsiveness gates pass.

Desktop acceptance includes keyboard-only setup/result/lock/repair/export; separate Share Result/Export editable scenario/Back up everything intents; exact privacy preview; HTML/PDF generation progress/cancel/success/failure; focus restoration; semantic grid headers; screen-reader announcements; no color-only state; cancellation, stale and internal-failure states; and a representative large grid/report that does not freeze the webview.

Required usability tasks: create a small schedule, import people, express one required rest rule and one preference, understand an infeasible conflict, lock and repair, inspect `Why this?` and `Why not?`, export editable data, and share a privacy-reviewed immutable result. Confusion between Required/Preference, editable scenario/report, backup/share, or solver jargon blocks exit.

## Risks and failure handling

- **Compiler/verifier disagreement:** quarantine and correctness alarm; separate evaluators, exhaustive differential/mutation tests.
- **Feasible mistaken for optimal:** normalized proof state and explicit time-limit language.
- **Large/nonminimal conflict:** bounded shrinking and `sufficient conflict` wording.
- **Rule/model explosion:** static pair filtering, boundary-based rolling windows, conflict cliques/global constraints, model estimate, resource limits, benchmarks.
- **DST error:** IANA time zone, instant arithmetic, explicit ambiguity/nonexistence policy, spring/fall fixtures.
- **Misleading fairness:** explicit target population and raw metrics; no universal fairness score.
- **Stale solve/export:** revisioned jobs and warnings; reject unverified assignment/result/solution-derived exports without blocking valid project/scenario backups or diagnostic/infeasibility reports.
- **Backend crash/unavailability:** preserve scenario, typed retry/recovery; a fallback is governed by Phase 08, not improvised here.
- **Webview overload:** purpose-built queries, virtualization, throttled events, no solver/webview-thread work.
- **Import corruption:** bounded parsers, preview, stable-ID disambiguation, atomic command batch.
- **Compliance assumptions:** verified means configured rules passed, not legal or professional compliance.

Pause and write an ADR if a rule cannot be independently verified, a backend needs workforce-specific knowledge, the UI needs database access, decomposition cannot prove independence, migration loses scenarios, or correctness depends on undocumented backend behavior.

## Exit gate

Phase 07 is complete only when:

- all thirteen Required rules and all eleven preference categories satisfy the rule checklist and domain-rule definition of done;
- every practitioner clarification above has an explicit reviewed disposition, every confirmed requirement satisfies the same rule-completion discipline, and the synthetic combined workflow has acceptance evidence; no unresolved decision is presented as supported behavior;
- every workforce fixture passes validation, expected status, independent verification, scoring, explanations, and accepted-solution export checks;
- repair preserves every hard lock and minimizes changes according to the recorded score policy;
- results accurately distinguish optimal, feasible, infeasible, cancelled, and time/resource-limited outcomes;
- assignment and counterfactual explanations return impossible, worse by explicit category, or undetermined—never invented causality;
- current-versus-immediately-previous Scenario Compare renders semantic assignment/rule/score/fairness/preference/lock/proof deltas, while pack-owned buffer/tightness indicators show typed rule margins without false precision or a universal health score;
- every assignment/result/solution-derived export comes from an accepted verified solution; the exact privacy preview drives one-file offline HTML and direct PDF; editable scenario/full backup and diagnostic/infeasibility outputs remain available under their own valid-input gates; workforce portable/share schemas and all supported import/export flows are versioned, atomic, safe, and reviewed;
- versioned small/typical/stress benchmarks record complete phase/model/quality evidence; calibrated normal-corpus targets pass on the reference machine; first verified results are not withheld by proof or optional explanation work; and large solve/progress/results do not freeze the webview;
- keyboard, screen-reader, focus, non-color, stale/error/offline-equivalent core states, revision conflict, undo/redo, and representative-user usability gates pass;
- CLI and desktop call the same services and the desktop never owns authoritative scenario state.

## Deferred and non-goals

- Pumpkin selection and multi-backend routing are Phase 08; OR-Tools remains the default here.
- XLSX, payroll/vendor adapters, live calendar synchronization, encrypted/signed bundles, automatic rotating backups, semantic merge, hosted sharing, and report annotations are post-MVP. One-file HTML and direct PDF are required MVP outputs.
- Concurrent portfolios, superficial alternative generation, map-service travel calculation, legal-compliance certification, arbitrary solver parameters, and full unbounded solver logs are excluded.
- AI is not required to create, solve, verify, explain deterministically, repair, import, or export a workforce project.

## Assumption and version gates

- Use the Phase 03 pinned OR-Tools/protobuf pair and record worker, adapter, model, seed, thread count, and options. Do not independently upgrade protocol dependencies here.
- Exact lockfile pins are established in Phase 00; frontend result work uses the verified 2026-08-29 stack recorded in [assumptions.md](assumptions.md), including Vue 3.5.42, Vue Router 5.3.0, Pinia 4.0.3, TanStack Table 9.2.4, and TanStack Virtual 3.13.36. Account for Table v9 API and Pinia 4 ESM/devtools requirements.
- The CLI name, application ID, and release/hosting decisions remain explicit gates. `.eutheto` is the proposed portable extension; Phase 11 must close its media type/file association through the identity ADR before public use.
- Before public release, practitioner review must confirm workforce defaults, fairness presets/weights, workload bucket semantics, and any medical-practice wording. Presets remain starting templates, not compliance claims.
