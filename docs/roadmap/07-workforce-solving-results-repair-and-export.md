# Phase 07 — Workforce Solving, Results, Repair, and Export

## Outcome

Deliver the complete workforce-scheduling MVP vertical slice: every official required rule and preference compiles, solves through the default OR-Tools path, is independently verified and explained, appears in accessible result and repair workflows, and round-trips through the supported imports and exports. Every selected or solution-derived exported result is an accepted, independently verified solution bound to its evaluated revision; stale exports retain the explicit warning and labeling gates below.

This phase completes the behavior begun in Phases 05–06. It does not weaken the separation between domain meaning, planning IR, backend translation, and independent verification.

`Verified` records independent acceptance against the configured rules; `selected` identifies the user's chosen accepted local result; `locally exported` describes a generated output under its privacy and provenance gates. None means organization-approved or published. Those later collaboration states belong to [verified approval and publication](collaborative-planning-and-hosted-service.md#verified-approval-and-publication): human approval references an independently verified result and cannot waive verification. Phase 07 adds no organization-approval or publication tables, workflows, or release scope.

## Source coverage

This phase is the implementation source of truth for blueprint Sections 17–18; workforce solve/result portions of Section 22; Phases 7 and the applicable Phase 5–6 prerequisites; Sections 31.2–31.4, 32, and 33.1–33.4; CLI Appendix C; Tauri API Appendix D; backlog items `WF-004` through `WF-008`, `UI-005`, `UI-006`, `EXPLAIN-001`, `REPAIR-001`, and `EXPORT-001` from Appendix I; the workforce delivery slice in [Performance and Solver UX Targets](performance-and-solver-ux-targets.md); and the generic/workforce report slice in [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md).

Project-wide contracts and sequencing are in [README.md](README.md). Version and evidence gates are in [assumptions.md](assumptions.md). This phase depends on [Phase 02](02-domain-pack-and-planning-ir-contracts.md), [Phase 03](03-ortools-worker-vertical-slice.md), [Phase 04](04-independent-verifier-and-explanations.md), [Phase 05](05-workforce-core-vertical-slice.md), and [Phase 06](06-desktop-design-system-and-workforce-setup.md). Backend diversification follows in [Phase 08](08-pumpkin-backend-and-router.md); seating reuses these result/repair contracts in [Phase 09](09-seating-domain-and-venue-experience.md); AI may invoke only the same typed commands in [Phase 10](10-ai-assistant-mvp.md).

## Dependencies and entry conditions

- Workforce entities, time-zone/DST semantics, shift generation, stable IDs, migrations, eligibility, availability, coverage, no-overlap, and minimum-rest foundations exist from Phase 05.
- Revisioned command batches, undo/redo, scenario persistence, planning IR, provenance, score vectors, canonical model hashes, solution projection, and verifier interfaces are stable enough to extend without a second convention.
- The OR-Tools worker protocol, process supervision, cancellation, resource limits, status normalization, and candidate projection work end to end.
- Phase 04 rejects deliberately invalid candidates and persists deterministic verification/explanation evidence.
- The Phase 06 desktop shell, rule builder, validation UX, import preview, generated Rust-to-TypeScript API boundary, keyboard/focus infrastructure, and semantic design tokens exist.
- The user has accepted the Phase-06 desktop setup checkpoint and explicitly authorized Phase-07 implementation under the [manual checkpoint policy](README.md#user-manual-checkpoint-gates).
- The working CLI executable name `optimizer` remains an unresolved example. `.eutheto` is the proposed portable extension pending the Phase-11 identity ADR; implementation and public documentation use the final decision consistently. The project/package prefix is `eutheto`.

## Decisions and invariants

1. **Required means acceptance-critical.** A required rule violation always rejects a candidate. A preference is a bounded objective contribution and can never silently change required feasibility.
2. **Verification is independent.** Backend values are projected into a normalized domain solution, structurally validated, evaluated against every original domain required rule, and rescored independently. Backend status and compiler penalty variables are not authoritative.
3. **Verification failure is quarantined.** Store the raw result only in bounded, redacted diagnostics; fail the run; show an internal-correctness error; disable select, repair-from, and assignment/result/solution-derived export actions for the candidate. The later collaboration layer must likewise reject it for organization approval or publication. Project/scenario backup bundles and diagnostic/infeasibility reports remain available when their own source data is valid and must not imply candidate acceptance.
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

For rule 13, `locationIds` names eligible endpoint locations, not directed
pairs. The native `Location.transitions` entry from the source to the target
supplies the only gap for two different matching shifts assigned to the same
person at two distinct named locations. Both assignments must match the rule
scope; an absent entry imposes no travel predicate, and no reverse entry is
inferred. Evaluate the source assignment's end to the target's start using
signed instants (including subsecond and daylight-saving boundaries), and
check both authored directions when starts tie. The independent No overlap
rule still governs overlaps. Existing active singleton selections remain
loadable no-ops; the desktop editor requires two effective named locations
for newly active selections, proved by the prospective native shift-scope
review. Inactive raw selections retain their authored intent.

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
12. Keep each selected person's distinct worked-weekend count near an explicitly authored target.

Together/separate compares pairs of **selected** person–shift assignments: together
penalizes selected pairs without the authored same-shift or positive-overlap
relation, and separate penalizes selected pairs with that relation. It does not
require either person to work. Base stability compares the in-horizon scoped
person–shift pairs against the named base schedule, charging for both removed
and added pairs; a base pair made infeasible by current Required rules still
contributes its removal penalty.

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

**Named workload-balance decisions approved 2026-09-28; implemented and
exercised through source scoring, exact Planning IR normalization, real-worker
solving, native score-policy save/reload, and accepted-result browser views.
Combined practitioner native acceptance is recorded below; manual acceptance
and final PDF/platform gates remain open.** A workload
policy's peer group is the only people population. The referencing preference's
remaining shift filters narrow workload-bucket contributions; score every owned
calendar period in the planning horizon, including periods with no matching
shifts or assignments. For weighted equal share, each peer's target is the
selected peer group's period total multiplied by that peer's positive rational
workload weight divided by the sum of peer weights; a zero total gives zero
targets. The distinct, explicitly selected, versioned equal-raw-share target
mode does not change personal FTE weights or assign a fairness default.

Preserve fractional elapsed/scheduled workload, signed scheduled credit, and
rational shares exactly through cumulative absolute or authored piecewise
penalties. Fractional values interpolate linearly within their authored segment;
piecewise slopes need not be convex. Normalize the **entire affected integer
priority level**, including unrelated preference categories, before solver
optimization and independently recomputed scoring. If checked value,
coefficient, retention or exact scalarization limits cannot express a valid
policy, return an explicit unsupported numeric/resource result before solving,
not a rounded score, a false infeasibility result, or a changed same-level
trade-off. Accepted results show the source-derived actual workload range in
bucket units and a separately labeled target-relative range of signed
actual-minus-target deviations, alongside each person's actual, target and
deviation. This does not resolve the separate practitioner questions about
weekends, required quotas, grouping or split-clinic arrangements below.

Named alternatives map to recorded objective profiles, not labels alone:

- `Balanced`;
- `Preferences first`/`Preference-focused`;
- `Minimal changes`;
- `Fairness first`.

Run profiles sequentially in the MVP, verify every candidate, compare authoritative score vectors and visual diffs, and suppress alternatives that differ only through arbitrary tie-breaking without meaningful metric change.

**Alternatives equivalence decision approved during Phase07 implementation.**
Preserve individual tradeoffs: suppress different rosters as tie-break-only
duplicates only when native profile-neutral, non-rank metrics and each person's
configured workload/preference outcomes match exactly. Aggregate equality alone
is insufficient; do not hide which people benefit or bear a different burden.
There is no tolerance and no inferred ordering between incompatible profile
scores.

**Local implementation evidence, 2026-10-04.** The actual unsigned packaged
Tauri/WebKit build with approved worker manifest
`c8a177ac1405cb0edd364ba213ecbe8ac12d6ad05d9f474609d350158a106f31`
passed the four-profile scenario: explicit base adoption, independent native
verification of each saved result, exact duplicate suppression, delayed real
discovery-response protection, settled remount without replay, selected-pair
comparison with unavailable cross-profile score utility, and read-only viewing.
Run, Compare, and View retained their exact keyboard focus targets. A second
native IPC group proved active release retains cancellation ownership, explicit
cancellation skips later profiles, and remount recovers that terminal group;
the earlier group's accepted results reverified unchanged and authored History
was preserved. This is local automated evidence, not hosted, release, screen-reader,
or maintainer manual-checkpoint acceptance.

### Time and assignment-type preference semantics

**Approved on 2026-09-28; implemented and exercised through source verification,
real-worker solving, and native editor save/reload.** A `Time` preference matches
a shift when the resolved shift interval has positive half-open instant overlap
with the selected instant or weekly civil-time window. Resolve weekly occurrences
using the scenario time zone and reviewed DST gap/fold policy; one pair matches
only once even if multiple windows overlap. Other rule-scope dimensions, including
reporting-date weekdays, retain their existing meanings.

For `Prefer` on Time or assignment type, charge one unmet preference contribution for **each unselected, statically feasible matching person–shift pair**. Static feasibility includes person activity and applicable eligibility, availability and approved time off; impossible pairs do not become missed opportunities. Dynamic overlap, coverage and rest choices do not erase an otherwise feasible opportunity. `Avoid` charges selected matching pairs. Contributions use the authored priority's persisted score-policy level and bounded scale/weight; deterministic assignment rank stays a lower-priority tie-break. These decisions do not establish a fairness default or resolve the separate practitioner questions below.

### Practitioner scheduling clarifications

**Status: practitioner dispositions approved; native synthetic acceptance recorded below, manual review pending.** These distinctions supplement the baseline catalogs above; they are not universal policy defaults or permission to move Phase 07 production behavior into Phase 05. The confirmed typed semantics are implemented; declined grouping and linked split-clinic alternatives remain unavailable. No unanswered question becomes an implicit default.

| Requirement to resolve                  | Existing boundary and required distinction                                                                                                                                                                                                                      | Decisions needed before implementation                                                                                                                                                                                                                                                                                     |
| --------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Required workload and assignment totals | Maximum-hour/count rules and preferred targets do not express every minimum, exact, or ranged requirement. A quota derived incidentally from coverage and other totals is not a substitute for an independently authored requirement.                           | Determine which totals are Required versus preferred, their scope and period, permitted deviation, credited call/clinic/leave/administrative work, breaks, overtime, pay-period anchor, overnight attribution, and scheduled versus elapsed/DST accounting. Workload accounting does not add a payroll/vendor adapter.     |
| Distinct weekends worked                | Assignment/hour counts, distinct weekends touched, consecutive weekends, and completely free weekends are separate measures. The current workload measurement vocabulary and consecutive-weekend preference do not by themselves establish all these behaviors. | Define weekend boundaries and membership, what makes a weekend worked, count/participation targets or caps, eligible peer groups, equal versus contract-weighted shares, separate day/night/undesirable-shift burdens, and whether imbalance carries across rotations.                                                     |
| Work and time-off grouping              | Adjacency to a base schedule is not general grouping of newly selected assignments. Avoiding consecutive nights does not specify desirable work blocks or uninterrupted time off.                                                                               | Define worked/reporting days, isolated work, block boundaries and gap units, desired or maximum block lengths, whether short work interrupts time off, horizon-edge context, and priority relative to recovery and other preferences.                                                                                      |
| Optional split-clinic arrangements      | Predefined shorter shifts are different from selecting a linked alternative between one full session and multiple partial sessions. Supporting the former does not establish automatic choice of the latter.                                                    | Determine whether splitting is required or optional, who may form a pair, ordering and handover, whether the optimizer selects the date, frequency limits per person/pair/rotation, and whether other sessions must remain whole. Confirm exclusive alternatives and continuous coverage without double-counting workload. |
| Combined practitioner acceptance case   | Generic clinic fixtures and isolated rule tests do not prove the agreed combination of periods, coverage, restrictions, preferences, and repair works end to end.                                                                                               | Agree a synthetic scenario and expected semantic outcomes after the relevant answers are recorded. Include operator-visible totals and tradeoffs, infeasible and time-limited outcomes, boundary behavior, repair, and required existing export views.                                                                     |

**Partial practitioner decisions approved 2026-09-28; implemented in Phase 07.**
WF-004 must offer independently authored, bounded Required per-person
minimum, exact and ranged quotas. Assignment-count quotas count all
scope-matching selected shifts without requiring a workload bucket; named
elapsed- or scheduled-minute quotas count only a referenced bucket's selected
work. Use existing day/week/anchored pay-period/custom calendars, including
owned empty periods. Count permits reporting-date or start-instant membership,
not intersection; minute membership follows existing bucket validation
(intersection and union require elapsed minutes). For each membership, enumerate
all periods intersecting the half-open planning horizon and only additional
periods reached by any matching resolved source shift's reporting-date
attribution or positive elapsed intersection beyond the horizon, whether or
not that shift is selected. A scoped person's Required minimum still applies
to a period with zero eligible or selected shifts.
Coverage and preferred targets do not replace independent quotas. Initial
totals credit only selected, explicitly scheduled shifts according to their
measure and calendar membership. Leave, administrative work and breaks have
no separate implicit credits or payroll meaning; they can contribute only
when modeled as actual scheduled work. No authorized relief or overtime
silently bypasses a Required quota or rest rule: operators must explicitly
review a typed, revision-checked, undoable rule edit before solving again.

The new Required quota rule and explicitly chosen equal-raw-share target use
additive tags in the existing versioned Workforce scenario and portable-v1
formats; neither reinterprets an older tag or claims a v2 migration. Previously
valid documents remain readable. An older client that does not know a new tag
must reject the document rather than silently omit its Required rule or score
policy; rollback to that client is not supported for documents using new tags.

A worked weekend is a distinct local Saturday 00:00 through Monday 00:00
interval with any positive elapsed overlap from a selected shift, including
an overnight tail; count one weekend per person however many shifts touch it.
Offer an independent Required per-person maximum and explicit soft
targets/fairness across selected peer groups, without turning a preference
into a hard cap. Only source-owned selected shifts resolved by the planning
horizon (including their overnight tails) participate in consecutive-weekend
and workload policies; visible edge evidence must say that adjacent rotation
context is unavailable, not infer prior/next work.
New-work and time-off grouping is **not applicable** to this MVP:
existing consecutive-work, base adjacency and explicit recovery rules do not
promise contiguous new-work blocks or uninterrupted leave. Linked full-versus-
partial clinic choices are likewise **not applicable**: operators may author
predefined shorter shifts, but the optimizer does not invent or select a split
alternative. These dispositions must not be presented as implemented new
quota/weekend behavior.

**Additional practitioner decisions approved 2026-09-28; isolated quota and
weekend semantics and combined native repair exercised below.**
Holiday exclusions come from explicitly authored dated shifts, template
excluded dates or detached occurrence edits; no inferred holiday law or new
holiday calendar.
Soft weekend preferences require an explicit per-person or peer-group target
count of distinct worked weekends, without a default penalty for working a
weekend. When a repair objective compares changes, charge each altered
person–shift assignment; reassigning one shift removes one old pair and adds
one new pair. Do not instead minimize distinct affected people before
assignment changes. Hard locks remain unchangeable.

The explicit soft `weekendTarget` preference is implemented for each person
in its selected scope across the whole planning horizon; each local weekend
with positive selected-shift overlap counts once, including overnight tails.
Its source score, bounded Planning IR, real OR-Tools solve, portable round-trip
and native reviewed save/restart have been exercised. It adds no default
weekend penalty, Required cap or inferred adjacent-rotation context. Combined
native acceptance does not establish practitioner usability or universal defaults.

**Final practitioner dispositions approved 2026-09-28; typed implementation
and native synthetic acceptance recorded below, manual review pending.** Recovery is only an explicitly
authored, direction-specific Required Minimum Rest rule (after/before scopes
select the relevant clinic/call transitions), together with independently
authored Maximum Hours and Maximum Consecutive rules where needed. The
reference fixture's ten-hour gap is a chosen fixture value, not a legal,
universal or inferred default. No new Required consecutive-weekend cap or
guaranteed free-weekend rule is authorized: use the independently authored
Required distinct-weekend total cap and explicit soft count targets only.
The synthetic combined workflow uses twelve fictional clinicians across four
weeks of clinic and overnight call, specialist coverage, explicit quotas/rest/
weekend targets, leave and call-out repair, and separate infeasible and
time-limited variants. Document reviewed semantic outcomes and operator-visible
totals without reproducing recognizable source names, dates, or assignment
patterns.

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

Local Linux evidence: the pinned native worker and Rust adapter admit nonempty
assumptions only in an objective-free, exactly-one-worker profile with presolve
disabled. Workforce lowering, bounded deletion shrinking, independently checked
original-domain witnesses and retained run/condition-bound mapped conflicts are
implemented and exercised through the approved worker. Malformed cores never
publish a partial mapping; zero-assumption foundational proof is distinguished
from an unavailable empty native core. Conflict inspection, reversible diagnostic
copies, private no-clobber JSON and owned V2 explanation callers are implemented.
The unsigned Linux package has exercised mapped-source editor navigation,
same-revision receipt retention and focus, a reviewed diagnostic copy, its
normal one-step History Undo, and actual native picker cancellation,
no-clobber rejection and private source-bound JSON publication. The original
scenario and history remain unchanged; the published file excludes paths,
assets, project descriptions and solver logs and has mode0600. A separate
approved-worker replacement exercised exact target-true/incumbent-false pins,
conditional mapped diagnostic copy, one journal command, UI Undo and Redo.
Current owned V2 explanations and latest-two comparison have packaged WebKit
evidence; the comparison's own-panel geometry regression also passed after
removing an incorrect inherited horizontal flex layout. These local Linux
checks do not close EXPLAIN-001, the full repair workflow or the Phase07
maintainer checkpoint.

Comparison computes added/removed/changed assignments, required-rule status, score-vector deltas by level/category, fairness deltas, preference deltas, affected people/groups, lock preservation, and solver status/proof differences.

Compare aggregate scores only when objective definitions, units, population/scope, weights, scaling, and priority order are compatible. Otherwise use a supported common evaluation policy or explicitly comparable raw metrics and label unavailable comparisons; never present unlike scores as universal utility. Record the baseline, result, query scope, proof/termination state, and evidence bindings so later text or voice clients can explain the exact established claim. A valid evidence ID alone does not substantiate arbitrary prose.

Phase 07's Scenario Compare scope is intentionally bounded: compare the current accepted result with the immediately preceding accepted result for the same scenario. Show the complete semantic difference above, preserve both result/run/revision identities, and let the user reopen or restore the prior accepted plan through existing revisioned commands. Arbitrary revision selection, named milestones, branching, reconciliation, generalized criticality, and cross-pack health semantics remain post-MVP.

Later collaboration reuses this existing compare evidence, revisioned repair path, and privacy-filtered Share Result behavior under [reuse of existing authorities](collaborative-planning-and-hosted-service.md#reuse-of-existing-authorities). Organization publication is a separate authorized immutable projection, not a relabeling of local selection or file sharing; its detailed contract does not expand MVP comparison, repair, or export requirements.

Persist compact assumption-to-rule mappings, objective contribution records, counterfactual summaries, backend bounds/status, verifier evaluations, and model/solution hashes. Full solver logs are opt-in and bounded. A later AI paraphrase never replaces the deterministic evidence view.

Historical run compatibility now has immutable bytes captured from the current
Rust writers before the planned objective-profile/bound cutover:
`crates/eutheto-domain-ir/tests/fixtures/request_semantics_v1.json`,
`portable_accepted_v2.json` and `verified_checkpoint_v1.json`. The existing
Domain IR contract suite reads those historical inputs, rechecks every nested
checksum/request semantic hash and enforces exact reserialization. Do not
regenerate these compatibility fixtures from future constructors. This does not
claim the proposed newer carriers or profiles have been implemented.

The four captured legacy Workforce cases in
`domains/workforce/core/tests/fixtures/pre_profile_semantics.json` preserve
rank-only no-preference/all-inactive behavior, active zero contribution, and
all fifteen authored levels plus rank. Their existing compiler/source-scorer
regression replays exact objective identities, model hashes, score bytes and
original-domain verification checksums; these are not new named profiles.

Internal terminal backend bounds are now independent of incumbent objectives.
Native admission preserves exact supported single-level values, rejects
malformed/range/directional contradictions, and retains a bound without
manufacturing an incumbent. Coalesced duplicate observations still advance
the measured time floor. CLI JSONL retains its previous incumbent/null-objective
wire shape; actual bounds remain separate events. Approved-worker primitive
and tiny independently verified Workforce samples exercise these producers.
Durable compact input-bound manifest retention now preserves the exact admitted
native attempt, independently optional objective/bound vectors and digest-backed
terminal runtime through ordinary/counterfactual finalization and quarantine.
Current/historical envelopes and generated guards are migrated; replay checks the
recorded routed fingerprint without recompiling its model. Private diagnostics
retain the manifest, while participant outputs exclude this metadata.
Named-profile evaluation and presentation were incomplete at this intermediate
stage; subsequent implementation and local evidence are recorded under
[Fairness and score policy](#fairness-and-score-policy).

**Local Repair source-fact evidence, 2026-10-03 (Phase07 working tree).**
The complete-source pack route captures and independently admits accepted A/C,
preserves A→C comparison, and rederives original-domain facts against recorded
B→C when present. Recorded B is explicitly `notEstablished`; this read neither
recaptures the visible head nor authenticates baseline custody. The Workforce
evaluator retains all failed predicates and exact native groups, checks the full
Required obligation census, and bounds preparation, observations and output with
one inherited budget. Expected thresholds and observed values remain distinct.
Missing historical endpoints are explicit; incomplete groups are not complete
aggregate proofs. Comparison bodies and schema/source bindings are rederived,
while permitted conservative score unavailability is retained.

The desktop's **Explain saved-change facts** dialog uses the immutable candidate
anchor and exact A/C identities, separate head/library/pair/window fences,
paginated inert parameters/references, owned cancellation/release and restored
focus. Facts identify group obligations, not uniquely necessary replacements.
The source-fact implementation and packaged A→C read are verified. The subsequent
packaged manual-selection flow also exercised recorded B→C facts, focus restoration
and actual History Undo/Redo, as recorded below. CLI repair verification is recorded
in the following subsection; the Phase07 maintainer checkpoint remains separate.

Fresh integrated verification:

- `cargo test -p eutheto-core -p eutheto-command -p eutheto-domain-api -p eutheto-explain -p eutheto-verify -p eutheto-cli -p eutheto-desktop --all-targets --locked`: Core244 passed/44 ignored, Command66, DomainAPI11, Explain14, Verify20, CLI41 and Desktop95 passed. Ignored worker cases are not covered by this command.
- `cargo test -p eutheto-workforce --lib assignment_rules --locked`:129 passed, including full-group/census, frame, exact source-body/schema, historical-endpoint and distinct expected/observed regressions.
- The approved artifact `/nix/store/gzxxab57rnfybaismn90x7a1b0mq59az-eutheto-ortools-worker-0.1.0`, supplied through `EUTHETO_TEST_ORTOOLS_ARTIFACT`, ran `cargo test -p eutheto-core --test headless_worker actual_worker_repair_facts_keep_recorded_manual_frame_after_history_undo --locked -- --ignored --exact --nocapture`:1 passed. Actual admitted A, manual B, changed source and accepted C retain frozen B-only facts after history Undo. `cargo test -p eutheto-core --test headless_worker later_native_alarm --locked -- --ignored --nocapture`:2 passed.
- `cargo clippy -p eutheto-core -p eutheto-command -p eutheto-domain-api -p eutheto-explain -p eutheto-verify -p eutheto-workforce -p eutheto-desktop --all-targets --locked -- -D warnings`:passed.
- `just frontend-typecheck frontend-lint frontend-test frontend-browser-test`:passed;354 unit tests across21 files and124 Chromium tests across16 files, including escaped native parameters, reference paging and delayed-response ownership.
- `just generate-check architecture-check`:50 generated files checked, all13 Workforce corpus variants verified, and workspace dependency boundaries passed.
- `just e2e true`:fresh unsigned Linux packages built and the extracted native desktop exited0 with its bundled approved OR-Tools worker. The real Repair dialog asserted A/C identities, native fact frame, `notEstablished` and candidate source hash; Close restored focus without changing the revision. `.cache/e2e/packaged-optimize-repair-source-facts.png` was visually inspected. The full run also exercised accepted views/comparison/base capture, authored rules/preferences, exact DST finding navigation and mapped infeasibility/diagnostic-copy/export custody. This is local Linux evidence, not cross-platform, screen-reader, maintainer-checkpoint or release acceptance.
- Independent DeepReview found no remaining defect in the integrated UI/Core/API/Workforce/synthetic source union. Independent security follow-up found no vulnerability in the five-path complete-source boundary; its leaf/UI-child/synthetic exclusions and earlier separate reviews remain distinct. Neither review executed runtime checks.

Completed counterfactual results remain bound to their retained derived run and request, not merely to a valid result checksum. Store migration V6 backfills those identities for V5 completed jobs from their checksummed result and rejects conflicting or unowned completions; ordinary job reads and evidence reads both verify the persisted run authority before publishing a result. Running jobs retain their separately recorded ownership.

## Repair and manual edits

A base assignment is `Hard locked`, `Soft locked` (`try to keep`), or `Unlocked`. Compile a change indicator for every base assignment and minimize changes at a high objective level. A previous solution may be a search hint, but correctness comes from explicit locks and stability penalties.

Before committing an invalid drag/reassignment, explain the violated rule and offer cancel, edit a related rule, convert the request to a preference and re-optimize, replace a conflicting assignment, or create a diagnostic copy. A valid manual change is a revisioned, undoable command and may remain unlocked, soft locked, or hard locked. `Repair plan` becomes primary after edits and defaults to minimum change.

The repair diff highlights unchanged, moved/replaced, newly unfilled, and newly assigned work, plus the new rule or fact requiring each explainable change. Hard locks must survive; any impossible hard-lock set is rejected before solve with actionable evidence.

**Maintainer decisions recorded during Phase07 contract review.** The native
guided and CLI implementations have local verification evidence below. After a
valid manual change, `Repair plan` preserves the latest
complete committed manual selection. Keep the original accepted plan immutable
as comparison/ancestry data; the editable selection does not itself become an
accepted solver result. Required hard locks remain outside the objective.
For minimum-change repair, first minimize changed person–shift pairs, then
Soft-lock penalties, then authored preferences and the deterministic rank.
Keeping a Soft lock does not justify more total pair changes in that profile.

Current guided implementation evidence:

- Native Read/Review/Apply authenticates immutable A and complete B, retains
  unavailable historical endpoints, resolves recurring work through the existing
  catalogue, and checks the complete Required census. Protection and replacement
  commit atomically; reopened Undo/Redo authenticates the original private
  receipt rather than treating public checksums as custody.
- The approved worker artifact
  `/nix/store/gzxxab57rnfybaismn90x7a1b0mq59az-eutheto-ortools-worker-0.1.0`
  ran `cargo test -p eutheto-core --test headless_worker manual --locked -- --ignored --nocapture`
  through `EUTHETO_TEST_ORTOOLS_ARTIFACT`: four passed. Cases cover recurring
  replacement/protection, tampered inverse refusal after reopen, recorded B
  facts after history Undo, and a 100-person/64-type source through successive
  reviews/applies and reopened Undo/Redo. The fixed 16 MiB admission budget is
  unchanged; owned receipt/source transfers remove duplicate trees rather than
  increasing limits. Original accepted bytes and streamed hash meaning remain
  checked.
- A separate exact approved-worker run of
  `actual_worker_manual_opaque_source_preserves_admitted_history` passed:
  600/800 KiB nonsemantic source fixtures preserve original accepted bytes and
  restore every admitted complete snapshot through reopened Undo/Redo. An Apply
  that reaches the resource limit must leave both document and revision unchanged.
  The 600 KiB case requires both protection changes to commit.
- Fresh Core/Store tests passed (Core244 tests plus3 doctests, Store124 tests;
  the ordinary command ignores worker cases). Strict workspace Clippy passed;
  the subsequent boundary regression passed its focused strict Clippy check.
  Generated drift checked50 files and all13 deterministic Workforce variants.
- Fresh frontend formatting/lint/type/unit checks passed (354 tests), alongside
  137 Chromium tests; the additional review-error/draft-retention case passed in
  the focused 14-test Repair browser run. These checks do not prove packaged
  native integration.
- Fresh independent correctness and security reviews of the final owned
  receipt/source/replay cutover found no remaining P1/P2 issue. These were static
  reviews, not execution or release acceptance.
- Rebuilt packaged execution now passes Hard Review/Apply and a protected
  minimum-change Repair. Manual review uses the existing lossless solution
  integer codec; all98 native desktop tests and strict desktop Clippy pass.
  Independent correctness and security reviews found no defect in that codec
  correction or its unchanged response limit and operation ownership.
- The protected replacement diagnostic reports native infeasibility but no
  returned assumption core on the approved worker. It correctly remains
  inconclusive, not a proof. A genuine-worker regression checks that the
  protected Repair cannot yield a Hard-violating alternative and still restores
  exact snapshots through reopened Undo/Redo. This follows the explanation
  catalog's explicit unresolved-search outcome; no empty core gains authority.
- Packaged execution has now exercised explicit unlock/Soft replacement,
  minimum-change Repair, exact A/B/C facts, focus restoration and actual History
  Undo/Redo. Evidence parameters remain paged; the driver reads both pages
  instead of assuming every scope fact is simultaneously rendered.
- A read-only selection refresh no longer traps navigation from an untouched
  manual panel. Its failing-before browser regression and all15 Repair browser
  checks pass, alongside frontend formatting/lint/type checks and354 unit tests.
  Visual inspection also exposed horizontal manual-panel layouts; the correction
  uses the existing stacked layout and a failing-before viewport regression.
- Rebuilt packaged execution also passed the layout containment check and complete
  repair subflow. The later ordinary Optimize handoff exposed a source-read race:
  arriving B metadata overwrote an explicit Optimize choice. Intent now distinguishes
  the source-derived default from explicit selection. A second regression corrects
  duplicate unchanged members in recorded before/after lists: native old/new arrays
  already contain the complete member census.
- Fresh focused browser checks pass53 tests across Repair and the real routed
  shell; frontend formatting, lint, type checks and354 unit tests pass. Rebuilt
  `just e2e true` passes the complete packaged suite (1407.64s under the2-CPU
  cap), including the repaired intent handoff and exact before/after member
  arrays, plus later rule, DST-navigation, diagnostic-copy and export checks.
  The remaining CLI acceptance is now recorded below. Local REPAIR-001 verification
  does not close the Phase07 maintainer checkpoint or hosted integration gates.

### CLI repair authority and current verification

`solve --repair-from` now selects an explicit accepted original by saved result ID
or JSON file. File workflows require its exact original snapshot in the current
input or inspected single-scenario bundle history. The scenario and original
result remain immutable. Native admitted manual B takes precedence only for the
same complete original A; exact retries preserve their recorded B rather than
recapturing later edits. A file operand cannot downgrade known native alarm/custody
checks, and genuinely imported originals do not gain native history authority.

Complete captured originals travel in objective-context2 under a512KiB canonical
ceiling; existing Authored/in-snapshot writers remain context1. Aggregate carrier
and IPC ceilings are unchanged. Capture admission, source verification and queued
verification-slot waits retain the original parent budget.

All five actual-worker Core repair integration cases passed, together with the
real quarantine-rollback ID/file refusal case. DomainIR compatibility/size checks,
the full Workforce package and strict affected-package Clippy passed; independent
DeepReview and security review have no unresolved findings. The covered-provenance
depth regression fails with the old expanded-limit admission and passes after
both counterfactual validation/hash paths use the existing effective ceilings.

A bounded representative file workflow used101 people,42 eligible for the
modeled work,28 consecutive days,1176 candidate pairs and560 selected/captured
pairs (56,720 canonical bytes). Actual CLI repair and independent reverification
passed; exact A/C/captured pair sets and input hashes were unchanged, no library
was created, and recomputed pair-change/Soft costs were zero. The repaired
terminal was **feasible**, not claimed optimal; this is not unpruned stress evidence.

The rebuilt packaged CLI corpus passes108 checks across all15 variants. The
maintainer chose to preserve Appendix-F's full-day absence and every Required
rule, correcting its expectation to infeasible as documented in the
[canonical fixture correction](05-workforce-core-vertical-slice.md#illustrative-canonical-workforce-fixture).
Local readiness is not solver feasibility, and no accepted roster, score or mapped
native conflict core is invented for that case.

For the synthetic practitioner family, the maintainer approved extending only
finite person activity to the exclusive end `2027-03-02`, so it contains the final
February 28 overnight ending March 1 at 06:00 local. The four-week horizon,
all 48 shifts, qualifications and Required rules remain unchanged. The specialist
variation retains its separate qualification shortage and remains infeasible in
the verified corpus.

The real worker then reported infeasible: the repeating weekly calendar owned a
fifth period reached by the final overnight, with six available hours against
twelve per-person four-hour minima. The maintainer approved replacing that
calendar with four explicit Monday-midnight periods: February 1–8, 8–15, 15–22
and February 22–March 1. Both practitioner variants retain elapsed-intersection
hour attribution, quota bounds, horizon and shifts. The final tail remains work
but lies outside these authored quota periods; production period ownership and
independent verification semantics are unchanged. The corrected baseline and
call-out repair are now exercised by the packaged corpus.

The corrected practitioner source now produces an independently verified
Feasible result through the packaged CLI. Its roster did not contain the old
recipe's assumed clinician-07/February-10-clinic pair. The maintainer chose to
bind the call-out to an actual movable incumbent of that accepted clinic instead
of constraining the original solve. The private recipe records the exact native
shift identity and unavailability window, not a placeholder person. Runtime
selection excludes only matching Hard-locked pairs; the single native mutation,
immutable original A and minimum-change verification remain required. Both saved-ID
and accepted-file repair paths pass, followed by fresh native verification against
the unchanged current source. Original bytes, exact captured ancestry/pairs,
no-library file behavior and missing-original no-clobber refusal remain checked.

The call-out repair is **Feasible**, with independently counted pair-change cost4
and Soft cost0. The original incumbent's only assignment in the February8–15
reporting week was the unavailable clinic. Its Required minimum-one quota forces
another weekly assignment, distinct from the replacement clinician on that clinic.
Exact unchanged coverage fixes68 selected pairs, so these two additions require
two removals: cost4 attains an independent lower bound. This proves minimum pair
cost for this fixture, not optimality of later preference/rank levels. Native fresh
verification checks the complete Minimal-changes score; its layout is not assumed
equal to Authored plus two levels.

The corpus separately preserves candidate-count and qualification-shortage
readiness findings; both infeasible variants still dispatch the actual worker.
Appendix-F and the specialist-shortage practitioner variant report exit4/Infeasible
without an accepted roster or invented conflict core. Strict affected Clippy,
50PlanningIR unit tests plus school fixtures,193Workforce unit tests, four benchmark
regressions, generated drift and independent correctness/security reviews passed.
The corpus evidence has SHA-256
`1ed35d3536cdea7125d1790f0bcad9232d766df1e41a24cbad1bfdf8e16cac62`,
binding CLI
`2017c68a94f71875965d3601ec8584c46b8784c32f484040fe47630078bf9ae8`
to approved worker manifest
`c8a177ac1405cb0edd364ba213ecbe8ac12d6ad05d9f474609d350158a106f31`.
No hosted, release, whole-pipeline performance or maintainer checkpoint acceptance
is inferred from this local corpus.

## Imports and exports

### Imports

Support people CSV, eligibility CSV or pasted matrix, shift-instance CSV, availability/time-off CSV, and existing-assignment CSV. Every importer follows choose file, safe format/encoding detection, column mapping/bundle summary, additions/updates/duplicates/rejected-row preview, stable-identity matching, proposed-change validation, one atomic undoable command batch, and a report with downloadable rejected rows. Never partially mutate or merge similar names silently.

**Existing-assignment import decision approved during EXPORT-001 planning.**
Imported person–shift pairs form an editable, explicitly unverified external
starting plan, not mandatory assignments and not an accepted solver result.
Do not invent accepted-result IDs or native receipt custody to fit an existing
accepted-source baseline. Give external baseline provenance its own explicit
contract; later review, repair and optimization still pass ordinary native
validation and independent candidate verification. Import itself neither
hard-locks every pair nor confers verified-result export authority.

**EXPORT-001 external-source prerequisite verified locally.** The native entity,
explicit V3 run binding, source-aware scoring and portable capability checks are
implemented. Packaged CLI smoke now exercises `solve --repair-external ENTITY_ID`
for JSON files and stored scenarios: an empty external plan still permits the
Required assignment, records its one added-pair cost without accepted ancestry,
passes fresh verification, and leaves the scenario revision unchanged. Existing
relocated CLI comparison/export and worker/manifest tamper checks also pass.
Packaged native acceptance now covers external source selection/editing, complete
off-page updates, stale rebase, review/apply and Undo/Redo, independently verified
Repair, immutable four-profile source capture, explicit empty source, lost-receipt
History reconciliation without replay, and reversible removal. The unsigned
DEB/RPM build passed; the corrected runner passed against that DEB in an isolated
offline native environment. Core stored tests cover copy remapping/provenance,
retry after edit/deletion and refusal of new group members after a revision change;
a real-worker regression rejects missing external sources even with an adopted
accepted base. All 46 domain-IR contract tests passed, including literal V1/V2
context/request-byte and hash compatibility. Scoped independent correctness and
security reviews completed; this is not hosted/platform/manual checkpoint
acceptance. The rest of EXPORT-001 remains open.

**Assignment CSV import verified locally.** Strict assignments v1 and explicit
comma/semicolon/tab mapping now share native CSV source custody with People import.
Reserved canonical framing cannot fall back to mapped input, including when rows
are skipped. Native review binds exact endpoint identities, the complete create or
replace target, empty-plan approval, skipped records and all original source bytes.
The source admits at most 16 MiB/10,000 data records before skips. Every unskipped
invalid or duplicate pair blocks the batch; known retained work identities are not
filtered by current eligibility or display windows. Assignment exclusion reports
retain all omitted records up to 10,000 rows/1 MiB, without changing People's
200-row/64 KiB report contract.

The unsigned Linux DEB/RPM build passed. Actual packaged acceptance exercised the
OS picker, reserved-format refusal, native endpoint selection, duplicate/skip
decisions, stale re-review, atomic complete-plan creation, saved report/cancellation,
canonical explicit-empty replacement, lost-receipt History reconciliation, and exact
Undo/Redo. Review/report screenshots were inspected. Domain assignment and strict
codec regressions, all 25 Core CSV tests, 104 native library tests, 378 frontend
tests, 141 existing browser tests, type/lint and generated drift passed; scoped
independent correctness/security reviews reported no actionable findings. Final
all-target Clippy for Workforce/Core/desktop/xtask, workspace Rust formatting, and
affected repair-view, stored-solve, assignment/Core/native CSV regressions passed
after cleanup. These are local checks, not hosted/platform/manual checkpoint
acceptance. The remaining import families are tracked below.

**Eligibility CSV import verified locally.** Explicit comma/semicolon/tab columns
resolve person/type identities and exact `true`/`false` membership. Missing pairs
remain unchanged; every unskipped invalid or duplicate typed pair blocks the
complete batch. Native review groups one ordinary update per changed person,
preserving unrelated raw fields, reference spelling and retained membership order.
The source ceiling is 16 MiB/10,000 data records before skips, with at most 1,000
changed people. Each endpoint alias map is capped at 10,000 entries before typed
decoding; complete exclusions retain up to 10,000 rows/1 MiB. No qualification,
rule, accepted-result or solver-feasibility authority is created.

The unsigned Linux DEB/RPM build passed, and actual acceptance against the DEB
exercised native file selection, exact endpoint mapping and selected-record
inspection, duplicate/invalid-boolean skips, stale re-review, sparse grouped apply
with independent Person fields preserved, saved report/cancellation, no-change
history, lost-receipt History reconciliation and exact Undo/Redo. Review/report
screenshots were inspected. The focused command is
`EUTHETO_E2E_ELIGIBILITY_CSV_ONLY=1 just e2e true`.

All 10 domain eligibility regressions, 29 Core CSV tests, all 107 native library
tests, 383 frontend tests and 142 browser tests passed, as did typecheck, lint, all-target
Clippy for Workforce/Core/desktop/xtask, and generated drift including the 15-variant
Workforce corpus. Independent correctness/security review is clear after two
reproduced fixes: native-valid non-hyphenated raw references now cross generated
guards without widening canonical root/setup/envelope identities, and successful
Apply restores focus using its correlated receipt and post-refresh context.
Native fixture-capture scaffolding was removed. This is local evidence, not
hosted/platform/manual checkpoint acceptance.

**Shift-instance CSV import verified locally.** Explicit columns cover assignment
type, location, local start/end intent, complete Coverage JSON, tags and reporting
attribution. Native identity facts distinguish absent, supplied and invalid IDs:
unknown supplied IDs require same-ID Add, while all document/ledger and stored
wrapper identity collisions are refused. Every unskipped duplicate target blocks,
including invalid and no-op rows. Partial updates preserve unrelated raw fields
and detached origin; semantically equivalent references, times and coverage retain
the original spelling. Local times use the captured timezone/gap/fold policy, reject
offset/zone-bearing input and require a positive interval without inferred rollover.
Imports remain one ordinary atomic batch with at most 1,000 actual changes,
cumulative pre-clone projection admission, and complete bounded exclusion metadata.

The unsigned Linux DEB/RPM build passed. Actual DEB acceptance exercised native
file selection and cancellation, exact reference mapping, same-ID Add, detached
partial updates, negative-offset/cross-midnight review, offset/gap/fold rejection,
explicit skips, stale re-review, atomic apply, complete saved report/cancellation,
semantic no-op without History, lost-receipt History reconciliation and exact
Undo/Redo. The final walkthrough waits for native Work reads and opens the History
disclosure by keyboard. Review/report screenshots were inspected; missing
projections are described as unavailable preview details, not proof of absent
instances. The focused build/run command is
`EUTHETO_E2E_SHIFT_CSV_ONLY=1 just e2e true`; the final walkthrough used
`EUTHETO_E2E_SHIFT_CSV_ONLY=1 just e2e true false` against the unchanged freshly
built application after script-only corrections.

All 18 shift-domain and four setup-time regressions, 37 Core CSV tests, 110 native
library tests, 385 frontend tests and 147 browser tests passed. The final copy
clarification also passed the four focused browser tests and packaged acceptance.
Typecheck, lint, all-target Clippy for types/Workforce/Core/desktop/xtask, and
generated drift (50 files and all 15 Workforce corpus variants) passed. Independent
correctness/security reviews, including the domain review-state refactor, are clear.
Genuine native fixtures cover preserved nested UUID/time spelling and signed offsets;
capture scaffolding and the temporary package-only runner were removed. This is
local evidence, not hosted/platform/manual checkpoint acceptance. Remaining
EXPORT-001 output work stays open.

**Availability/time-off CSV import verified locally.** Native review covers all four
availability kinds, instant and weekly forms, exact supplied-ID Add, explicit
identity-free selections, and the shared document/ledger/both-wrapper census.
Person/type/location aliases are independent; filter arrays reject typed duplicates.
Raw structured JSON is validated before equivalence. Partial nested-leaf updates
preserve original timestamp/reference spelling, independent dates, source/note
whitespace and authored weekly order/multiplicity. Variant changes are complete,
never hybrid; weekly recurrence is not expanded during import. Native local-time
resolution facts remain visible for equivalent edits.

Imports do not invent enforcement: approved leave blocks applicable overlap on
its own; unavailable/available-only records need matching Required rules; requested
leave only contributes through explicit preferences. Active and inactive incoming
requested-leave references prevent invalid kind conversion. Source, revision,
native identity census, mapping, decisions, projected rows, batch, validation and
complete exclusions bind approval/rebuild. Existing 10,000-row/1,000-change limits,
cumulative 16 MiB serialized projection admission and a separate 1 MiB complete
exclusion report remain enforced; these are not process-memory claims.

The unsigned Linux DEB/RPM build and actual DEB walkthrough passed. Native picker,
role mappings, all kinds/forms and transitions, partial raw updates, malformed
filters/windows, offset/gap/fold/nonpositive-time refusal, incoming preferences,
explicit skips, stale re-review, eight-change atomic apply, the complete 17-row
saved report and save cancellation were exercised. Semantic no-op preserves
revision/History; intercepted committed receipts reconcile through actual History,
and full-record Undo/Redo is exact. Review/report screenshots were inspected.
The walkthrough distinguishes the picker's idle status from pending native reads.
Run `EUTHETO_E2E_AVAILABILITY_CSV_ONLY=1 just e2e true`; script-only reruns used
`EUTHETO_E2E_AVAILABILITY_CSV_ONLY=1 just e2e true false` against that same package.

All 26 availability-domain, 45 Core CSV, 113 native-library, 388 frontend and 155
browser tests passed, along with typecheck, lint, focused all-target Clippy,
generated drift (50 files) and all 15 corpus variants. Genuine native guard fixtures
cover noncanonical nested references, fractional negative-offset raw instants,
signed resolution facts, weekly records and no-change receipts; capture scaffolding
is removed. Independent correctness/security reviews and the integration follow-up
are clear. This is local evidence, not hosted/platform/manual checkpoint acceptance.
Remaining EXPORT-001 output work stays open.

**Pasted eligibility matrix import verified locally (2026-10-05).** Explicit
transient text enters shared native source custody without clipboard access,
file-snapshot exposure or a frontend parser. Rust checks the complete fixed-tab
rectangle, mandatory header, exact `true`/`false`/blank-preserve semantics,
role-specific aliases and typed duplicate pairs. All cells count toward the
10,000-cell limit; the UTF-8 source cap is 1 MiB. Ragged sources and overflows are
refused, never truncated or rescued by skips. Native cell coordinates bind review,
approval and the complete exclusion report; additions retain row-major order.

The rebuilt unsigned Linux package exercised whole-input overflow, shape and
cell-limit refusal, native capture/census/selected samples, separate aliases,
explicit duplicate/invalid/unresolved skips, stale re-review, grouped updates,
report save/cancel, exact Undo/Redo, consumed no-op and actual committed
lost-receipt recovery through History without replay. Automation delivers a whole
paste through the real textarea input handler because WebDriver key input treats
tabs as focus traversal; it does not claim OS clipboard provenance. Native entity
detail reads are typed/canonical; raw spelling preservation is proved separately
by domain, Core and native snapshot regressions. Review/report screenshots were
inspected. Run `EUTHETO_E2E_ELIGIBILITY_MATRIX_ONLY=1 just e2e true`; script-only
reruns used `EUTHETO_E2E_ELIGIBILITY_MATRIX_ONLY=1 just e2e true false` against the
unchanged package.

All 10 matrix-domain, 10 existing eligibility CSV, 50 Core CSV, 118 native-library,
396 frontend and 164 browser tests passed, with typecheck, lint, focused all-target
Clippy, generated drift (50 files) and all 15 corpus variants. A genuine native
fixture covers source/census/sample/review/apply/report/no-op envelopes; its
temporary capture hook is removed. Independent review found and verified the
correction for known membership facts on unique valid skipped blanks; security
review and correctness follow-up are clear. This is local evidence, not hosted,
cross-platform or manual checkpoint acceptance. Remaining EXPORT-001 output work
stays open.

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

**Verified local Workforce portable evidence.** Fresh-store Core coverage restores
and reopens current Workforce data with an adopted accepted original, an external
unverified starting plan, the exact historical source, an accepted archival record
and a permitted attachment. Editable-scenario and full-backup re-export preserve
the semantic data and byte-exact attachment without creating native acceptance.
Workforce portable schema V1 and historical source revisions are separate
contracts; no historical domain migration is claimed.

A separate regression seeds the checked-in V1 database fixture and exercises its
actual migration to `legacy_unverified`/unaccepted. It failed before the fix because
results-inclusive backup omitted that row. The closed archival-result V1 host
format now preserves recorded status/flags and raw JSON, including SQL-absent
versus JSON-null fields, without inventing a historical Workforce result schema.
Known exact sources survive edits/synchronization/reopen; unavailable native-only
sources refuse selected result export rather than being guessed or omitted.

Local checks passed 49 Types, 26 Export, 54 Import, 119 Store, 10 Store settings,
66 Core application and 50 Core CSV tests. Independent correctness review found
a native collision-key mismatch; corrected `UUID.json` projection/occupancy and
authorization now pass real Core same-library Skip/Replace/CreateCopy coverage.
The correction was independently reviewed; security review found no remaining
scoped actionable findings.

A freshly built CLI backed up migrated synthetic V1 data, restored a fresh
profile, reopened and re-exported exact archival JSON. Native accepted export
refused both the original archive and a coherently resealed archive with forged
accepted/verified claims. Two persisted copies retained original payloads and
survived original-scenario deletion. Missing sources and prohibited native
payloads refused result-bearing publication without a destination, while
results-excluded backups and unrelated mutation/editable export succeeded.
Resealed missing-capability/newer-wrapper imports refused without mutation;
existing-destination publication preserved the original bytes. Earlier CLI
evidence separately established fresh file-source verification of the retained
accepted original and exact external-plan provenance preservation. This remains
local evidence, not platform/hosted/manual acceptance or HTML/PDF completion.

**Verified local desktop CSV/calendar evidence.** Inspect schedule now offers
complete assignment CSV, people/shift summary CSV and combined ICS, with per-person
ICS from native accepted-person rows, including zero-assignment people on later
pages. Terminal and before-proof checkpoint captures are explicit; checkpoints
never receive a fabricated terminal manifest. An 8 KiB inert sample is explicitly
partial. Native review retains the complete artifact, digest and source/head/library
bindings, sharing HTML's bounded custody and revision-leased no-clobber publication.
Assignment CSV V1 remains unchanged: revision labeling is in review/receipt and the
suggested filename, not new CSV columns.

Eight ICS consumer tests, three summary CSV consumer tests, real-worker Core
terminal/checkpoint/alarm and HTML regressions, seven native report/export tests,
19 operation lifecycle tests, ten custody tests and the strict IPC boundary matrix
passed. Focused all-target Core/CLI/Workforce/native Clippy, frontend lint/typecheck,
396 frontend tests, 55 generated artifacts and all 15 corpus variants passed.
Command-permission conformance passed after adding the missing export-release
allowlist entry. DeepReview found and corrected a shared custody-clock error that
could mask a retained source alarm; correctness and security reviewers rechecked
the final ordering and permission delta with no remaining findings.

An unsigned Linux DEB, SHA-256
`f1b37f130888a56fb1f87da72e5901b632639b5ff18423e5de5adc15a6020219`,
was built against the already approved manifest-bound worker and exercised in
Tauri/WebKit with network isolation. A genuine native solve over 56 people and one
shift produced all formats. The summary included all 56 people despite 50-row
paging; the last-page unassigned person produced an empty calendar. Reopened
files matched every preview byte count, SHA-256 and sample. Native picker
cancellation, refusal to overwrite an existing destination, keyboard activation,
focus restoration, inert hostile text, and subsequent default-private HTML sharing
passed. After a native edit advanced head 1→2, historical summary export required
consent and retained revision-1 names. Its SHA-256 was
`54b26aa5f121b86d0f38c49ac625e1f7c1b0f505b1dbb1680c37f3c577f9338b`.
Temporary smoke hooks were removed. This is Linux local evidence, not assistive
technology, other-platform, hosted or maintainer checkpoint acceptance. Direct PDF
and the remaining EXPORT-001 gates remain open.

#### Share Result Model and renderer

`Share Result Builder` consumes the immutable accepted Result Model, scenario/result/revision/verification identities, chosen profile/options, workforce recipient views, and provider/redaction policy. It emits a strict versioned inert-data model with plan version, generation/application/solver/report versions, schedule/person/location/transportation sections as available, safe explanation/status/provenance, and explicit privacy flags. It defaults to excluding source calendar titles, full addresses, private notes, complete constraint graphs, rejected alternatives, raw objective/solver details, diagnostics, and all scenario fields not needed by the recipient.

The current Workforce Share Result V2 pack hook is an identity-only, independently reverified input to that builder, not the participant report or its privacy preview. It includes accepted assignment identities and verification provenance, not the authoritative `ScoreVector`: priority levels and preference penalties can disclose private requested leave even without names. The V1 rank-only share shape is not reinterpreted as V2; a future recipient-facing objective view needs its own reviewed privacy projection before inclusion. Assignment/person identifiers remain pseudonymous, not anonymous.

The generic report/component boundary introduced here has no Tauri, SQLite, filesystem, credential, provider, solver, or mutable-scenario dependency. Desktop and standalone renderers reuse accessible result view models/components where sound; desktop-only actions are injected explicitly. The workforce report provides timeline, By Person, By Location/shift, coverage, preference/fairness, changes and transportation views only when their selected data exists. Every graphical summary has a list/table equivalent.

**Recipient privacy decision approved; native HTML sharing verified locally.** Participant
HTML/PDF may opt into configured fairness **actuals**: the exact measure, period,
population and actual distribution, with explicit privacy disclosure. Do not
include targets, deviations, objective weights or raw preference penalties, and
do not label actuals alone as an equity assessment. Recipient preference
assessments are explicitly excluded from the Phase07 participant report;
preference inspection remains available locally. All sensitive switches default
off. Preserve exact fractional values and the unchanged identity-only V2 hook.

The maintainer also selected **staffing actuals only** for the optional participant
coverage surface: assigned headcounts by shift, explicitly labeled as staffing
facts rather than a coverage assessment. Required thresholds, qualification
conditions, preferred targets and satisfaction assessments remain excluded.
This does not change required-coverage verification or the existing local
coverage/health views.

The HTML report is one file that opens from `file://` in supported modern desktop browsers, requires no Eutheto/server/account/browser storage, and works offline. It embeds project-owned code/styles/small approved assets plus safely encoded Share Result data and print CSS. Core meaning makes zero network request and has no CDN, remote font/script/icon, analytics/tracker, authentication, or map-tile dependency. Explicit external links require a visible action and are never necessary to understand the plan.

Presentation interactions may switch views, filter/highlight/search, select a day/person/location, expand details/evidence, adjust density, and print. They alter only local view state. The report cannot edit a scenario, apply a command, rerun the solver, imply filtering changes the plan, load scenario-authored code, or silently contact a service.

User-authored strings remain inert encoded data and render through text nodes. Validate decoded data; escape container terminators; avoid `innerHTML`, `eval`, dynamic functions, and scenario-to-JavaScript concatenation; use a restrictive self-contained-report CSP. An `About this plan` section records scenario/revision, result ID, friendly plan version, generation time, versions, verification/status basis, and applicable snapshot assumptions. Source edits never alter an existing report.

**In-progress model/renderer evidence, not export completion.** The separate
`eutheto/share-result` V1 wire, generated guards/artifacts and bounded native HTML
encoder now have local checks. The initial 29 export-crate tests, four recipient-type
tests, focused all-target Clippy, desktop typecheck/lint, and generated drift
(55 artifacts and all 15 Workforce corpus variants) passed. A synthetic native-encoded
`file://` report exercised inert hostile markup, Unicode, actual gap/overnight
times, accessible controls, person/location/search views and a 390-pixel viewport
without document overflow; observed requests were local document loads only.
Browser printing restored the filtered screen while including the complete plan.
The three-page output retained repeated plan/time-zone/status/generation context,
repeated table headers and grayscale layout; PDF inspection found no URL annotations.

Two reproduced security defects were corrected: contradictory privacy switches
now refuse display, and a conservative shared presentation-work ceiling rejects
reference-expanded text before native HTML emission or DOM construction.
Native and browser checks agree on a repeated-label 48/49-row boundary. Unknown
versions and contradictory/oversized presentations display an error without tables.
Scoped independent correctness/security reviews found no remaining issue in these
corrections. These synthetic presentation checks do not establish accepted-source
projection, native preview/save custody, CLI/desktop integration, packaged fonts,
controlled PDF-process isolation or four-platform runtime compliance; all remain
required before EXPORT-001 closes.

An expired native operation was reproduced being mislabeled as user cancellation.
Export serialization and shared staging/publication callbacks now retain the typed
interruption, with Core, CLI and native error mappings updated. All 31 export tests
pass, including expiry before encoding/commit and during staging with cleanup and
existing-destination preservation. A real public-API smoke executable observed
deadline refusal, staging removal, successful exact publication and no-clobber
refusal; the temporary executable was removed. Core/CLI/native-desktop compilation
and their library/binary Clippy checks pass. A scoped independent review found no
remaining defect; this does not establish a packaged native report flow.

A modified standalone report reproduced an internally contradictory staffing
count of 999 for one assignment. Bounded pre-DOM admission now checks complete
staffing and comparison censuses, assignment identity/pair uniqueness, work-label
references, ordered fairness populations and exact measurement values. Real
Chromium refused 27 structurally valid malformed variants with one alert, no
tables and no readiness marker. A freshly native-encoded synthetic report and
seven native-admitted edge variants rendered, including empty/zero populations,
signed fractions, the i128 minimum, unreduced integral counts, unavailable removed
history and separate original/manual comparisons. These checks establish internal
consistency, not authenticity or native acceptance. Frontend typecheck and focused
lint, eight recipient Rust tests, generation/drift and all 15 corpus variants pass;
independent correctness/security reviews found no remaining scoped defect.

Source-preparation prerequisites now retain the once-decoded typed Workforce input
through complete fresh verification and resolve recorded original/selected baseline
pairs together. Typed calendar scans use inherited interruption and prospective
retention checkpoints; public token APIs and temporal ownership semantics remain
unchanged. A review caught duplicate custom-scan charging in the hours compiler;
the correction passes a 100-interval regression with only 150 work steps remaining.
After correction, 404 affected Workforce tests, all-target Clippy and the real
corpus-check executable pass, including all 15 corpus variants. Independent
correctness/security reviews found no remaining prerequisite defect. These are not
recipient projection or native-custody completion. Report-specific original A
authority must additionally prove that its complete recorded pair census equals
the exact freshly admitted native A selection; matching identity tuples alone is
insufficient.

Actual-only fairness helpers now expose one operation-local checkpoint through
calendar generation, index retention, union admission, sorting and scanning.
Paged views retain their existing limits and real interruption control. A new
regression proves exact overlapping/adjacent fractional elapsed work, cumulative
admission across two computations before the second sort, and scan cancellation.
That regression, all 18 result-view tests, all-target Workforce Clippy and the
real 15-variant corpus check pass.
Historical manual B facts require its exact authenticated pre-edit journal source;
external B records no historical labels or times. Neither may borrow C's metadata
or treat A's source as B.

The Workforce recipient producer now freshly verifies selected and historical
sources, validates provenance on false assignments too, and derives complete
opted-in staffing, change and actual-only measurement censuses. One prospective
report budget covers selector scans, periods, contributions, union work, output
rows and escaped bytes. Security review found missing nested selector charges;
the correction refuses a freshly verified source whose six disclosed contexts
would inspect 12 million tag candidates. Actual optional groups also exercise
cumulative row/union refusal and inherited interruption.

Fresh packaged-CLI solves of `large-supported` and `practitioner-workflow` passed
producer → host-model validation → HTML encoding → canonical round-trip →
no-clobber publication. Their canonical models were 10,683 and 46,964 bytes;
the practitioner report retained all 68 assignments, 48 staffing rows and its
complete configured measurement group. Real Chromium exposed UUID-ordered days;
native exact-instant ordering and person-index consumption now preserve chronology,
including fractional instants. The regression failed before and passes after.
The actual rendered practitioner report has 28 ordered days and 12 person tables;
print preparation restores all 48 work rows despite a zero-result display filter,
then restores that filter. Both real-source reports rendered without alerts or
resource requests. The temporary smoke executable was removed.

Eleven producer tests, 18 paged-view tests, 21 Core authority tests, all-target
Workforce Clippy, desktop typecheck/focused lint, 55-artifact generation/drift and
the real 15-variant corpus check pass. Independent correctness/security reviews
found no remaining scoped defect. This evidence does not establish Core native
history promotion, retained approval/publication custody, desktop save integration
or the controlled PDF runtime.

Core now captures immutable terminal/checkpoint report bytes under one inherited
30-second preparation budget and owns queued Store/blocking work through settlement.
It retains exact ordinary/counterfactual source custody, promotes A only after the
native tuple and producer's complete pair comparison agree, and recovers B through
the existing authenticated private receipt. Missing/unapplied history leaves B facts
unavailable; an eligible non-private command ID is an integrity refusal, not absence.
HTML publication stages the captured bytes and uses the captured library/head lease,
explicit captured-stale consent and live source admission without post-commit revocation.

Real-worker regressions pass for terminal privacy/no-clobber/revision refusal,
original/manual comparisons before and after Undo, checkpoint access before proof
and after caller cancellation, and a captured report refusing a later native alarm
even when durable quarantine rolls back and create is cancelled. Both independent
reviews found deadline-stage/query-permit gaps; those are corrected and the security
recheck closed both findings. Core all-target Clippy passes.

A separate runnable Core smoke solved the practitioner source with the approved
real worker, persisted it in SQLite, captured all 12 people/48 shifts/68 assignments,
and published/reopened identical HTML under the native revision lease. Canonical
model: 47,484 bytes; HTML: 92,878 bytes; HTML SHA-256:
`349b47f24ebadc7e505de9a25a28945c0e3fd67de0758e436bdb14aabbfd2d7d`.
The temporary executable was removed. Source-state accounting now charges actual
retained capacities and bounded late growth, including checkpoints before native
observation installation; real before-proof regression and independent re-review
cover that lifecycle rather than treating Arcs as pointer-sized handles.

Native HTML preview/create/release now use window-owned, expiring, single-consumption
custody. Reservations exist before operation IDs are exposed; active work and
encoded replies remain charged through settlement and application-to-Tauri handoff.
Generic observer release does not cancel publication or retire ready human review.
Queued create failure re-admits retained sources before returning, while confirmed
publication is not revoked by later cleanup. The application-owned bound does not
claim to bound or purge Tauri's transport queues.

Local verification passed ten custody tests, four real-worker native tests,
strict command-boundary/refusal/reservation-rollback checks, bundled-feature
all-target Clippy, desktop typecheck/lint, 396 existing unit tests and generated
drift checks. Independent DeepReview and security review found no remaining
actionable static finding. A real packaged Tauri/WebKit smoke used the approved
worker and exercised default disclosure exclusion, inert hostile names after opt-in,
native picker cancellation, exact confirmed HTML publication, repeated cleanup and
focus restoration, old-revision consent, and existing-destination preservation.
The smoke used the already-staged hash-verified worker because the standard Nix
build omits the unstaged new source files; this is not a successful standard Nix
build, hosted/cross-platform evidence or maintainer checkpoint acceptance.

CLI recipient HTML now supports both stored IDs and explicit scenario/result files,
with five independent default-off disclosures and a required named destination.
File reports revalidate the complete source, preserve recorded A→C/B→C comparisons
without native historical promotion, and keep the live head unknown. One 30-second
budget covers source waits, preparation and publication; only pre-capture waits are
interruptible wrappers, never owned capture or publication settlement.

Local verification passes the bounded-wait unit test, ten CLI solution tests,
four approved-real-worker admission/Repair/alarm/publication tests, Core/CLI
all-target Clippy, and generated drift/corpus checks. Independent DeepReview and
security review report no actionable static findings. The actual assembled CLI
solved the practitioner fixture in both modes, then reopened exact-digest HTML
with all 12 people, 48 shifts and 68 assignments. It exercised private defaults,
opt-in inert names/labels, complete staffing and configured fairness actuals,
malformed-file refusal without database creation, and no-clobber in both modes.
A stored edit advanced revision 0→1: export refused without stale consent and,
with consent, retained revision-0 names while recording head 1. Default file HTML
SHA-256: `d60abbd5720a8f6bb3289c05c4328e8fe0afcc2a9e7ce4b30cc7bcf4fb4c9b93`;
historical stored HTML:
`a46cd89c93943a9ace29a8a69f7454e882fc1bf23a25f35fe21b5ed7d896a0a3`.
This CLI assembly likewise used the approved staged worker, not a successful
standard Nix source build or cross-platform/hosted acceptance.
Desktop CSV/ICS delivery is verified above; direct PDF and controlled PDF runtime
work remain open.

After pausing CEF, `just bench-workforce` and
`just workforce-corpus-smoke-cli` passed against approved worker manifest
`c8a177ac1405cb0edd364ba213ecbe8ac12d6ad05d9f474609d350158a106f31`.
Corpus v2 digest
`f3a3bb56aebec8cc0ef5331a6e75c0965e0d20323c171676b7c881b27efae6a8`
produced 60 headless samples across 15 cases and 108 packaged-CLI checks.
The packaged CLI digest was
`8d85d6a8be0f1f769e5fdf1969b7da1a170e1cd274413a6653f66f3b5fdffc91`.
Clinic, overnight, specialist and both DST cases passed fresh acceptance,
verification, explanation and JSON/CSV/portable checks; practitioner call-out
repair passed with stored and explicit-file originals. Four recipient encoder
tests and both approved-worker Core recipient/schedule capture tests also passed.
These are local synthetic observations, not a release baseline: each solve spawns
a worker, OS caches are unmanaged, and the CLI is the preserved assembled image.
The resource-limited case is not evidence of a solver time limit; native report
workflows, large result-view responsiveness and manual acceptance remain separate
obligations. `EUTHETO_E2E_CONFLICT_ONLY=1 just e2e true false` also passed the
packaged mapped-conflict, source navigation, reversible diagnostic-copy and native
JSON picker/publication workflow. The software-rendered Xvfb run had no working
accessibility bus; it is not native screen-reader evidence.

`EUTHETO_E2E_RECIPIENT_EXPORT_ONLY=1 just e2e true false` passed on the preserved
Linux DEB: real-worker acceptance, all three native CSV/calendar formats with
exact reviewed byte counts/digests, HTML picker cancellation with approval
retirement, and fresh-review HTML publication. The published private report
contained exactly the accepted assignment pairs and their participant identities,
excluded person names and optional staffing/change/fairness sections, and left the
scenario revision unchanged. The journey is retained in the native runner and
included in its full packaged path. No product behavior or CEF work changed.

The full `just e2e true false` packaged path also passed, including existing
solve/result/comparison/manual-edit/Repair, Required quota/weekend/DST,
mapped-conflict, Alternatives and external-plan workflows plus the new export
journey. Eleven Workforce recipient-projection tests passed. Focused runner
ESLint and Node syntax checks passed; the installed pinned formatter was used
directly because `pnpm exec` attempted an implicit modules-directory reinstall.

A newly solved practitioner file produced HTML digest
`c1ff20d8717d0ce278cf915c69cf9a520d98aaea99202e5ecb031988dbf68d6b`.
In real Chromium it rendered 12 participants, 48 shifts and 68 assignments,
reloaded with networking disabled, and exposed no external resource elements;
its CSP retained `connect-src 'none'` and hash-bound inline script/style.
By-person table counts matched every captured participant's assignment count
(68 total), all three schedule views rendered, and keyboard Tab moved from the
view selector to the labelled start-day selector. Axe 4.13.0 reported zero
violations or incomplete checks in both By Person and By Location/shift.
This is standalone HTML/browser evidence, not CEF, PDF or native assistive
technology acceptance.

All 37 ignored approved-worker Core acceptance regressions also passed with
`--test-threads=1`. A separate file-mode solve/export with an authored
`</script><img ... onerror=...>` title rendered that exact text in Chromium:
no image element, injected execution or subresource request occurred, and all
48 work instances remained visible. This exercised the native encoder and actual
standalone renderer, not only a fabricated browser model.

The post-pause integrated `just check` passed before the following timeout fix.
A real practitioner solve with Deep, three seconds, one thread and seed 1 exposed
`solution.authority_invalid`: the router's existing V1 time-limit observation had
no admitted worker outcome, but compact-observation validation required one.
The validator now admits only that exact dispatched, unresolved backend-limit
shape; other missing outcomes and mismatched present outcomes still fail.
No worker completion, runtime, result acceptance or timing is synthesized.
Existing valid serialized bytes are unchanged; older validators still reject
this formerly unusable V1 combination.

The domain regression failed before the fix and now passes with its negative
cases. All four compact-observation tests, ten stored-solve tests and 31 router
contract tests passed. The deterministic stored-run case verifies measured
backend/elapsed duration, durable terminal reload, no accepted result and retry
without another invocation. Independent correctness and security reviews found
no actionable defects in this bounded change.

The rebuilt approved-worker CLI
`81a6ce7df69098237613e9bd509ccdb2726dca6675f0f5f2c58a3b282ee12a47`
repeated the original three-second practitioner invocation: backend started,
`noSolutionWithinLimit`, exit 5, no accepted-result file, and a published diagnostic
with 2,254 ms observed elapsed and 1,463 ms backend time. This is unresolved,
not proved infeasible. Assembly used the approved worker root, not a successful
standard Nix source build.

The desktop was subsequently rebuilt with the approved staged worker through
`pnpm --filter @eutheto/desktop run tauri build --features bundled-ortools
--config src-tauri/tauri.sidecar.conf.json --no-sign --bundles deb,rpm`.
The Linux DEB SHA-256 is
`8e996ab4bb818c1be1f4cc5a06879c7398c3997679d17287e1fbd2b73fc7639f`;
the desktop executable SHA-256 is
`23f99ed0c43e17463d67a99288258ecad8ddef054b641e9bdf40760996113084`.
This proves local unsigned assembly, not standard Nix source closure, target-matrix
packaging or manual acceptance. A new large-result packaged journey exposed
a schedule-view alert after genuine accepted-result verification; large-webview
acceptance failed at that point and was subsequently fixed and rerun below.

The schedule-view alert was traced to `solution_get_view` quoting every signed
integer through the typed solution formatter: negative i32 timezone offsets in
the generic JSON view became strings and failed the generated Workforce guard.
The view now reuses the existing setup JSON encoder, preserving safe numeric
offsets and explicit exact-fact strings under the unchanged 512 KiB ceiling.
Typed solution/score/evidence integer encoding is unchanged. Cancellation remains
inside the same native operation and blocking response handoff. Both independent
correctness and security reviews found no actionable defect; the wide-integer
regression and all seven setup-boundary tests passed.

After a fresh unsigned rebuild, the focused packaged large-result journey passed.
DEB SHA-256:
`dfbc0f469b7d4b32738b4a0d266f605d7ecb73ccdad3080191562791d13f18f9`;
the extracted application measured by the runner has SHA-256
`a1c5c66631ce8a43ad738a92774e73a6ba0afd9710a0bc71a89c4288c324cdd5`.
It applies the versioned V1 `large-supported` payload through native commands,
checks semantic settings/counts/resolved shifts rather than byte-identical
scenario wrappers, and independently verifies the accepted source:
101 people, 12 shifts, 1,212 candidate pairs, 2,424 model variables and 12 selected
pairs. This is not a dense accepted-assignment workload.

On this Linux/Intel Core Ultra 7 268V/WebKit session, native first incumbent,
first verified feasible and elapsed timings were 379, 481 and 597 ms.
Eleven first-transition samples ranged 9–62 ms (median 49); eleven same-session
repeat samples ranged 8–58 ms (median 47). The boundary is in-page keyboard
keydown → exact settled native content → double animation frame, excluding
WebDriver round trips. It is not INP, paint, backend duration or heap measurement;
initial By Person rendering was unmeasured and caches were not flushed.
The observer reported six live-region mutations and zero supported long-task
entries with no sample/observer overflow; mutations are not spoken AT output.
Hardware acceleration and the accessibility bus were unavailable in this run.
Evidence SHA-256:
`8809a3f5b408ff37dd401c8af1a3ab41cee7c110a628049a770356c40d057941`.

**Non-PDF output hardening evidence (2026-10-06).** All 31 export-crate tests and
ten CLI solution tests passed, including atomic no-clobber publication,
cancellation/cleanup and malformed-input refusal. The approved-worker Core
portable-result regression passed with a well-formed, rechecksummed forged score
refused by HTML, JSON, assignments CSV, summary CSV and ICS exports. The native
recipient revision-lease regression and bundled-worker desktop schedule-publication
regression each passed; the latter exercises wrong owner/format/revision, one-use
review custody and exact CSV/calendar publication receipts.

A fresh actual CLI stored-source journey solved the manifest-bound V1
`clinic-tiny` fixture at revision 1, then changed all person names at revision 2.
Assignments CSV, summary CSV, ICS, JSON and HTML each refused publication without
explicit stale consent (exit 3, no destination), then succeeded with `--allow-stale`
(exit 0 and the old-source warning). The outputs retained revision-1 identities
and names rather than the changed head. Summary CSV records `1,2,old-revision`;
ICS records historical revision 1, revision-as-of 2 and stale provenance; JSON
retains scenario revision 1. The HTML captured source records revision 1 and
observed current revision 2 with `Plan from revision 1`. Its SHA-256 is
`a18c1bce436032630133b084f3ec57683c37783ba4f4607d04793e2f5791f0ee`.
Assignments CSV intentionally remains the identity-only round-trip format;
its CLI response/warning, not invented CSV columns, supplies stale context.
These checks do not close PDF publication, runtime cleanup or manual review.

The frozen practitioner source SHA-256
`9a43c8917164cc6033e155b74bbf14d0c6b284494637f5ab8edbe68e3264daa2`
contains 12 fictional people, 48 generated shifts, one approved-leave record,
one Hard pair, explicit weekly count/hour quotas, directional rest, maximum
hours/count/consecutive work and distinct-weekend limits. Its two preferences
record workload balance and weekend targets. The rebuilt CLI freshly verified
the complete saved baseline: all 26 Required-rule results passed, 1,494 predicates,
68 selected pairs, zero violations and the same verification checksum.
The versioned corpus above separately proves stored/file call-out repair and
specialist-shortage infeasibility; the three-second run proves unresolved timeout.
This closes native synthetic fixture semantics, not manual usability, universal
fairness/defaults, grouping, linked split sessions or final report gates.

The current rebuilt CLI also reran all 108 versioned corpus checks across
15 variants through `just workforce-corpus-smoke-cli`: baseline practitioner
solve/fresh verification/explanation/JSON/CSV/portable round-trip, both stored-ID
and explicit-file call-out repairs, missing-original refusal and separate
specialist-shortage Infeasible all passed. Evidence SHA-256:
`6d15d0a814c7faa9ba9100d39ec65fd59412ef6db612257884d52555d22e2977`.
This evidence binds the rebuilt CLI above rather than the earlier image.

Fresh rebuilt-CLI solves of V2 `dst-spring` and `dst-fall` were independently
verified Optimal; summary CSV and ICS publication passed. Spring preserves
authored `02:30–04:30` with MoveForward resolution `08:30–09:30Z` (one elapsed
hour); fall preserves Earlier-fold `01:30–02:30` with `06:30–08:30Z` (two elapsed
hours). Calendar event endpoints match the summary source instants exactly.
All eleven CSV/calendar consumer regressions passed, as did the independent
rest evaluator and compiler spring/fall tests: the apparent ten civil hours
are nine elapsed hours in spring and eleven in fall. This is deterministic
native time/export evidence, not external calendar-client or PDF acceptance.
Ten maximum-hours evaluator regressions also passed, including clipped
calendar intersection, scheduled-local versus elapsed measurement and
exact fractional boundaries.

Fresh `just frontend-browser-test` passed 164 tests in 21 Chromium component
files. `just generate-check architecture-check protocol-check migration-check
workforce-corpus-check` passed: 55 generated files, workspace dependency
boundaries, nine protocol fixture pairs, 76 fixture files in 14 roots, all 15
Workforce variants and four focused startup/newer-version/rollback/import
compatibility regressions. Browser semantics are not native spoken AT; these
local checks are not supported-target hosted checks or manual acceptance.

The latest full `just e2e true false` passed against that same rebuilt DEB in
308.61 seconds, including the complete UI-keyboard tutorial described below
and all 22 large-result transitions using actual Tab/Shift+Tab/arrow/Enter
events. The remaining native rule, DST, infeasible, Alternatives, external-base,
Undo/Redo and CSV/ICS/HTML publication journeys also passed. Native source and
revision checks distinguish read-only review from explicit Apply. Inspected
native screenshots show calendar controls, negative-offset assignment details,
textual valid/Hard review and privacy disclosures. Fixture seeding and other
non-tutorial journeys are not entirely keyboard-only; this is not spoken AT
or manual acceptance.

The latest integrated large-result artifact SHA-256 is
`4e30f026f9b6520982982ea853a36c9a4fe615d8fc96cf29ba945a27c86f0af4`.
Its eleven cold-view transitions measured 17–209 ms (median 66 ms), and eleven
warm transitions 15–240 ms (median 67 ms), under the same settled-content/double-RAF
boundary and fixture limitations described above. Navigation uses finite native
radio steps, bounded real Tab traversal and real-key re-entry after WebKit
blurs the temporarily disabled paging button; no DOM focus/click injection
activates these transitions. Sampling begins only on Enter or arrow activation,
not navigation Tab presses. Native first incumbent was 544 ms, first verified
feasible 678 ms and elapsed 805 ms. Long-task collection reported zero entries
with no sample/observer overflow; seven live-region mutations are announcement
proxies, not screen-reader speech.

The pinned `nightly-2026-08-28` bounded `just fuzz-check` also passed all nine
targets and 19 checked-in seeds, with seed zero, one worker, 30 requested seconds
per target, a five-second input timeout and a 4096 MiB RSS ceiling. Each target
reported 31 observed seconds and exit zero. This is local malformed-input smoke,
not exhaustive fuzzing, supported-platform or deferred PDF proof. Garbage-
collected pinned toolchain loader/linker dependencies were restored by their
exact existing Nix store identities; no toolchain or lockfile upgrade was made.

Fresh full `just check` passed in 1,021 seconds, covering the authoritative
generated/protocol/fixture/architecture/policy, format, lint, type, Rust
workspace/documentation and frontend checks. This predates the subsequent
keyboard-tutorial runner edits; their Node syntax, ESLint and full native
integration checks passed separately.

The current release benchmark reran all 15 V2 corpus variants with 60 raw
samples; evidence SHA-256:
`2b523ba66081131349f25c547e1ffa568555cf35f87fe73867b85f2ab1a9f8d2`.
Each variant has one first-operation sample, one discarded warmup and three
post-warmup samples under Deep/60 seconds, seed one, one worker and
stop-after-first-feasible. Workers spawn per solve; OS caches are unmanaged.
The practitioner first verified/total was 2,214/2,220 ms; post-warmup first
verified was 2,266–2,387 ms (median 2,282), total 2,274–2,396 ms (median 2,289).
Large-supported first verified/total was 470/472 ms; post-warmup first verified
was 467–472 ms (median 471), total 470–474 ms (median 473). All four large-pressure
samples refused preparation with `operation.resource_limit` and no model:
not backend infeasibility or solver-timeout proof. These are local synthetic
observations, not calibrated release-reference thresholds or physical cold-cache
measurements.

The subsequent focused
`EUTHETO_E2E_KEYBOARD_WORKFLOW_ONLY=1 just e2e true false` passed in 58.07 seconds
against the same unsigned DEB. Real WebDriver keyboard events create a fictional
person, work type, manual shift, explicit eligibility, active Required coverage
and a natively reviewed Authored score policy; validate and Optimize; inspect
assignment evidence; explicitly save the accepted repair base; review and Apply
Hard protection; validate and Repair; compare saved results; and review CSV and
HTML outputs bound to the repaired result's exact identity/checksum.
Explanation and recipient-review closure restore the exact trigger element.
The journey uses no DOM focus, click or value injection for those actions;
native API access disposes of the isolated fixture only after the proof.
The single-assignment example proves preservation of its protected pair, not a
better roster, cross-profile utility or unique causality. Destination-picker
keyboard operation, spoken native AT, human comprehension and the final PDF
candidate remain unverified.

#### Privacy preview, print, and PDF

The intent-led UI separates **Share Result** from **Export editable scenario** and **Back up everything**. MVP has one strong participant-friendly default plus advanced allowed field/section switches; it reserves coordinator/full-analysis/custom profile IDs for later or separately justified delivery. The preview renders the exact Share Result payload and lists whether names/aliases, full addresses, source calendar titles, notes, constraint details, rejected alternatives, objective details, transportation, and external links are included. Potentially sensitive options default off.

Reviewed print styles remove controls without paper meaning, retain date/plan version/status, avoid clipped schedules, control page breaks/repeated context, support grayscale, and include concise provenance. Direct PDF uses the same validated Share Result Model through a controlled local renderer; it cannot have a broader privacy model or remote dependencies. Atomic temporary output and cleanup apply to HTML/PDF.

**Typst direction approved 2026-10-17; implementation and acceptance pending.**
The maintainer approved replacing the browser-based PDF approach with Typst after
review of the official project's maintenance and adoption evidence. PDF will use
a fixed project-owned document layout over the exact captured, privacy-reviewed
Share Result Model. HTML keeps its existing renderer. Equivalent complete content,
provenance and disclosure are required; pixel-identical layouts are not.
Pin compatible official crates and reviewed fonts without upgrading unrelated
packages or the project toolchain. An initial representative-output and
cancellation gate precedes full desktop/CLI integration. A bounded dedicated
helper must admit only the fixed template, captured data and embedded fonts,
with no user templates, package downloads, system-font discovery or arbitrary
file access. Preserve existing source admission, operation budgets, stale
consent, review custody and atomic publication. Dependency/font notices, exact
artifact packaging, supported-platform evidence and manual acceptance remain
required. Approval of this direction is not evidence that PDF export works.

**Local Typst renderer proof (2026-10-17).** The initial helper passed seven
tests, including real rendering, retained-parent-pipe completion, active EOF and
cancellation termination, closed external resolution, and explicit refusal of
unsupported glyphs. Independent PDF text and bounding-box inspection checked a
20-page, 231,230-byte tagged A4 output from a deliberately synthetic 23,339-byte
model: all 24 assignment identities appeared once in the census, all 12 work
entries and selected sections remained present, signed rational and zero values
survived, Greek/Cyrillic/accented labels rendered, and hostile template/HTML text
remained literal. Inspection found and corrected long-label margin overflow and
missing repeated page context; the resulting pages passed bounds and repeated
context checks, with a representative page also visually inspected. Unsupported
CJK text fails explicitly instead of silently losing glyphs. This is a renderer
fixture, not the accepted 12-person/48-shift/68-assignment practitioner workflow.
At that initial renderer gate, packaged desktop/CLI integration, actual
practitioner output, supported-platform checks and the manual checkpoint
remained pending. The boundary is documented in
[recipient PDF rendering](../architecture/recipient-pdf.md).

**Packaged CLI/practitioner evidence (2026-10-17).** After the approved,
hash-checked seven-file development-runtime setup, all seven native recipient
custody tests passed with the actual approved OR-Tools worker. Canonical CLI
corpus smoke passed 110 checks across 15 cases, including two exact-byte PDF
publication checks. The actual synthetic practitioner source solved and was
independently verified; default and all-fields HTML/PDF exports preserved all
68 selected assignments and their person/work pairs, 48 work instances with
their intervals, and 12 people. Privacy switches, names/authored-label exclusion,
optional sections, receipt hashes, per-page context, word bounds, unchanged
source and no-clobber refusal passed. Independent review checked the full pair
census and installed runtime/artifact identities; representative rendered pages
were visually inspected. Default PDF: 25 pages, 307,424 bytes, SHA-256
`6f9d067b796cdbc62579ccb2ca93208e96c46182253d3edcd516943756679087`;
all-fields PDF: 32 pages, 378,753 bytes, SHA-256
`861cdd71f65a3bb0ec35c891040c02e9b0d5c4f552d4fca1855a40938dbf3fbc`.
This establishes CLI publication and native custody tests, not desktop picker,
keyboard/focus or cancellation integration. At this checkpoint, dependency
decisions, browser/desktop acceptance, assistive technology, supported-platform
evidence and manual acceptance remained open; see the
[validation handoff](../contributors/phase07-pdf-validation.md).

**Approved dependency remediation and revalidation (2026-10-18).** The exact
unchanged citationberg commit `06a591e2f237d25e1dfdedac3f3d1494c496c52d`
is vendored with preserved provenance, resolving quick-xml 0.41.0; ZIP 9.0.5
retains the existing feature selection in both workspace and fuzz locks.
The six exact maintenance dispositions expire after 2026-11-16 UTC, without
automatic renewal. The standard enforced security gate passes against a fresh
official advisory database with zero vulnerabilities; the five quick-xml/ZIP
vulnerability IDs are never ignored. All 21 upstream source files remain
unchanged, with a checked complete file census. The eight CSL test fixtures
retain separate CC-BY-SA-3.0 terms and attribution, distinct from the library's
MIT/Apache license and production renderer assets. Scoped license review makes
no general legal or release-clearance claim.

Upstream XML compatibility, archive/import/export and backup/restore regressions,
fuzz compilation, strict affected-crate lint, generated artifacts, native custody
and PDF helper regressions pass. The canonical CLI/PDF package rebuild and 110
CLI corpus checks across 15 cases pass. Complete practitioner default/all-fields
HTML/PDF acceptance passes again with 68 selected assignments, 48 work instances
and 12 people. Default PDF: 25 pages, 307,608 bytes, SHA-256
`af065ca6e437512091f260c26cfa20bf8e893bdb240afc04d97443eb877c50d8`;
all-fields PDF: 32 pages, 378,832 bytes, SHA-256
`e1badb1c2bf18268f9d8b6ec474727db8deab8b2eaa512cd4dd49f61172bb0a4`.
Browser/desktop acceptance, assistive technology, supported-platform evidence
and the maintainer checkpoint remain open.

**Stored-library CLI follow-up (2026-10-18).** A disposable `clinic-tiny`
project was populated and solved through public commands using the canonical
package and actual worker. Stored-UUID default/all-fields PDF exports preserved
all ten accepted assignments across five shifts and two assigned people,
matched receipt hashes, enforced no-clobber and left the scenario unchanged.
After a public edit advanced the head, export refused without explicit stale
consent; consented export retained the original accepted revision and complete
assignment census. This adds database-bound capture/publication evidence
without namespaces. Exact hashes and evidence scope are in the
[validation handoff](../contributors/phase07-pdf-validation.md).

The remaining gates are actual bundled desktop PDF picker/publication and
keyboard/focus/cancellation lifecycle; the pinned browser component case;
spoken assistive technology and representative-user usability; macOS/Windows
packaging and lifecycle; and maintainer checkpoint acceptance. Namespace
preflights remain `EPERM`. The separate fresh browser task is still blocked
by the proxy on the two approved download redirect hosts while environment
allowlist publication is pending. No environment or isolation bypass is part
of this work. Phase 07 is not complete and Phase 08 is not authorized.

The following CEF decisions and results are retained as **historical experimental
evidence**, superseded as the production PDF route. They do not establish Typst
acceptance, require completing CEF adoption for Typst, or approve the missing
cleanup/governance ADR proposals.

**PDF runtime decision approved during EXPORT-001 planning.** Ship a controlled
local browser renderer for direct PDF so it consumes the same approved offline
HTML and print styles rather than a separate typesetter layout. The maintainer
accepted the additional runtime packaging, license and security-update work.
This is not permission to use an arbitrary installed browser, download a runtime
on first use, weaken sandboxing, or permit network-dependent rendering. Exact
runtime/version/artifact custody, cancellation, offline behavior and supported
platform packaging remain implementation and verification obligations.

The maintainer subsequently selected pinned upstream **prebuilt** artifacts,
subject to exact-version/platform redistribution and component-license clearance,
rather than owning Chromium source builds. This is not approval of the currently
installed test browser. The Chrome for Testing repository's Apache license covers
its repository code, while an upstream maintainer
[identifies Chrome terms as the binary terms](https://github.com/GoogleChromeLabs/chrome-for-testing/issues/21#issuecomment-1594319082).
The installed headless shell also includes Chromium BSD and third-party notices;
the applicable complete-artifact obligations remain unresolved. Do not assume
blanket permission or blanket prohibition from either notice alone. Runtime
adoption and redistribution remain gated on that evidence.

The maintainer approved pursuing **CEF prebuilts with a project-owned native
PDF host and platform helpers**. Chromium itself remains prebuilt; the host,
macOS helper packaging and sandbox/process lifecycle are project responsibilities.
This authorizes that implementation direction, not any particular binary:
exact archive/source/component-notice clearance and all runtime security,
offline, cancellation and platform gates still precede adoption.
Ordinary pinned-dependency licensing/source evidence is the starting point, not a
blanket demand for all upstream build flags, logs or link graphs. Review included
notices and actual redistribution/source obligations first; seek upstream clarification
only for a specific unresolved obligation. The proposed public inquiry is deferred,
and no inquiry has been posted.

**Maintainer sequencing decision (2026-10-06): pause, then bounded resumption.**
The isolated `experiment/cef-154-native-validation` branch was preserved at
`ce31e949b0901c6d084408d17015845039f9c4aa` while independently executable
non-PDF work completed. The requested pre-manual CEF check-in and route research
then occurred; the maintainer authorized orchestrated implementation and testing
of the reported plan. Resume bounded platform causal controls and exact-artifact
licensing work, not runtime adoption. Mock-keychain diagnostics cannot establish
macOS acceptance. The maintainer subsequently approved an executable-bound
user-namespace grant for the root-owned probe on disposable Ubuntu CI only:
no global sysctl change, setuid helper, sandbox disablement or workstation change.
PDF integration remains gated on supported-target feasibility and redistribution
clearance. The Phase-07 manual walkthrough/review still follows the complete
PDF-enabled candidate; neither automation nor this resumption waives that gate.

**Remaining-work order: Mac/Windows testing last.** At the maintainer's request,
finish independently executable Linux verification, licensing/source work and
font preparation before resuming Mac/Windows CEF testing or platform-specific
troubleshooting. Check in with the maintainer when those deferred tests are
the final feasibility tasks. This changes sequencing, not supported targets,
credential permissions or acceptance requirements. Production PDF integration
still requires supported-target feasibility and redistribution clearance;
dependent packaged verification and the manual walkthrough/review follow.

**Maintainer-owned final native tests.** When the deferred macOS/Windows host
tests are the remaining feasibility tasks, commit and push all authorized
Phase-07 source, experimental tooling and documentation to
`feat/phase07-complete` before checking in. Supply the exact commit and
reproducible per-target commands; the maintainer will pull the branch and run
the host tests. Do not launch those deferred tests through SSH or infer their
results from a push. No SDK binaries, PDFs, raw logs, credentials or unrelated
local data belong in that publication. Retain required target coverage,
credential-safety, redistribution and later integration/manual gates.

**Independent preparation and Linux custody decision.** Static inspection
covered all 223 packaged PAKs per exact SDK, not only `resources.pak`.
The isolated experiment's licensing record retains counts, font identities
and embedding terms without asserting FFmpeg redistribution clearance.
Exact host-installed Liberation Sans candidates cover the fixture's glyphs;
they are not yet pinned/bundled report fonts or native embedding proof.
A non-CEF unprivileged namespace control exercised owner SIGKILL and an
escaped-session child with retained identities. This is kernel-primitive
evidence, not CEF or product acceptance. Independent plan/security reviews
require an architecture decision before a new native cleanup-custody boundary;
PID-namespace browser compatibility also requires a correct procfs view.
No namespace-custody implementation or new host security permission is implied.

The documentation-only `docs/phase07-cef-linux-custody` branch records proposed
ADR-020 and the initial-authority proposal ADR-021. Independent plan/security
review found ADR-020 ready for a governing decision, not accepted authority.
The maintainer confirmed the existing `@Eutheto/maintainers` code-owner principal
and explicitly supplied sole-maintainer consent for `@ZebulonRouseFrantzich`,
but deferred private reporting-route details. The governance closure criterion
therefore remains unmet: no appointment, formal ADR acceptance or namespace
implementation follows from that partial evidence. The authorized synthetic
public intake check is [issue #70](https://github.com/Eutheto/Eutheto/issues/70).
The user-confirmed account [acknowledged receipt](https://github.com/Eutheto/Eutheto/issues/70#issuecomment-6100920336)
at 2026-10-10 18:42:45 UTC. This completes the single intake/acknowledgement
check, not ongoing monitoring, continuity, private custody or governing approval.

**Bounded hosted control evidence (2026-10-07).** Experimental source
`4cb72a72e05b7ebb700040ae266bac98159a7f00`,
[run 37553821340](https://github.com/Eutheto/Eutheto/actions/runs/37553821340),
left every supported target red. Both Macs printed/closed but real-keychain
`CefShutdown` reached the teardown watchdog; the single-variable mock-keychain
control completed shutdown, pool drainage and framework unload, deliberately
without accepting the case or target. Pinned source traces eager process-global
OSCrypt initialization despite an OffTheRecord request context. No secure
in-scope bypass was found; native execution/keychain posture or an upstream
secure provider requires a separate decision. Windows passed the plain tool
but failed `owner-tests` before SDK admission; the closed evidence does not
identify the failed regression, so no wrapper/compiler correction is claimed.

Linux's protected-parent guard stopped before installer invocation. The hosted
image's world-writable `/opt` policy made that ancestor unsuitable; the narrow
correction moved only owned experiment tools/runtime under protected `/usr/lib`.
Source `f583649709fb936ddec0511c22f36b6a8ad22cf5`,
[Linux-only run 37555757214](https://github.com/Eutheto/Eutheto/actions/runs/37555757214),
passed all 15 experimental rows with AppArmor user namespaces enabled, matching
installed-copy identity, deliberate wrong-token refusal and confirmed cleanup.
Normal output retained 64 fixture rows in three pages with Unicode, inert-label
and page-context checks; faults/refusals removed staging without observed survivors.
The Unix owner row exercises handled SIGTERM, not abrupt owner death, and
printing-stage cancellation does not establish exact in-flight print cancellation.
Other targets, complete pipe/drop coverage, packaged fonts, exact FFmpeg
source/configuration and LGPL replacement clearance, product integration and
the manual checkpoint remain open. Independent correctness/security reviews
found no remaining experimental-source finding; no runtime adoption is implied.

**Native Mac prelaunch gate.** The maintainer offered an arm64 Mac and initially
approved creation of the missing Chromium Safe Storage item. Independent
correctness and security reviews of the pinned native source found a second
lookup that can update a concurrently created item; create-only approval does
not cover that shared-keychain race. CEF was not launched. The prepared GUI
service was confirmed absent and its unloaded descriptor removed. The
maintainer selected a fresh GUI test account instead; native testing awaits
that account's active GUI session and SSH access. The original account-scoped
grant does not authorize keychain operations in the new account. Isolation is
not an upstream fix or production credential-safety clearance. Windows access
is not yet available.

**Disposable Linux CEF feasibility is measured, not runtime adoption.** The pinned
154.0.34/Chromium154.0.8037.98 SDK and project-owned experimental host produced a
16-page tagged A4 PDF from the identical captured offline HTML without a display
or sandbox-disabling flags. Its timeline retained all 48 shifts and 68 assignment
mentions with matching per-person counts despite a screen filter; page context,
inert markup, Unicode arrows, embedded host-font subsets and grayscale boundary
pages were checked. Final PDF SHA-256:
`25cabd4116275e2f1d573b910bc477dce7ede946b84945eebf0cae6ceae6d673`.
Renderer processes showed additional user/PID/network namespaces and seccomp
filters beyond the outer supervisor baseline; this does not describe every CEF
process or establish post-browser-compromise filesystem containment. Exercised
cancellation, owner termination, host/renderer crashes, timeout, malformed input,
write failure and injected namespace refusal removed staging with no observed
descendants. A reachable private HTTP sentinel received zero report GETs, and
native navigation, popup and download refusals were observed. Independent
correctness/security reviews found no remaining blocker in this experimental
scope. Exact in-flight-print cancellation, complete pipe/drop coverage, packaged
fonts, production integration, supported-platform evidence and redistribution
clearance remain open. No product PDF command or runtime dependency was added.

CSV and ICS schemas remain deterministic, stable-ID-aware, time-zone explicit, escaped correctly, and covered by semantic round-trip/consumer fixtures. XLSX, payroll/vendor adapters, live calendar synchronization, encrypted/signed bundles, automatic backups, hosted share links, and report annotations are post-MVP.

**Verified local CSV/ICS consumer evidence.** Summary CSV is decoded with
`csv-core` and checked against its complete people/shift census, stable identities,
formula-safe decoded cells, coverage margins, and elapsed versus wall-clock DST
durations. Six calendar tests parse output with the independently implemented,
test-only `ical` 0.11.0 reader and check UTC/DST meaning, complete selected-pair
sets, per-person filtering, stable opaque event identities, stale provenance,
folded hostile Unicode text, and refusal boundaries. Parser acceptance alone is
not claimed as RFC conformance or native acceptance of synthetic codec fixtures.
The seven focused tests and Workforce test-target Clippy pass. The exact parser
dependency has an inspected Apache-2.0 license, no build/install script, no enabled
generator/vCard features, and no new transitive dependency. Full-shell
`just rust-advisories`, regenerated `just licenses`/`just sbom`, and
`just generate-check` pass; these checks do not establish desktop CSV/ICS delivery.

## Application and CLI contracts

### Tauri API

Use revisioned request/response envelopes. Mutations carry `scenario_id`, `expected_revision`, and `request_id`; responses carry request ID, current revision, warnings, and schema version. Errors are typed/actionable and never expose Rust backtraces.

Required solve endpoints: `solve_get_backend_options`, `solve_estimate_model`, `solve_start`, `solve_cancel`, `solve_get_job`, `solve_list_runs`, and `solve_get_diagnostics_summary`.

Required solution endpoints: `solution_list`, `solution_get_summary`, `solution_get_view`, `solution_select`, `solution_verify`, `solution_compare`, `solution_explain`, `solution_start_counterfactual`, `solution_cancel_counterfactual`, `solution_get_counterfactual`, `solution_list_counterfactuals`, `solution_lock_assignment`, `solution_unlock_assignment`, `solution_create_repair_request`, `solution_export_preview`, `solution_export`, `solution_share_preview`, `solution_share_create`, `solution_export_cancel`, and `solution_export_release`. Counterfactual get/list retain immutable request/result bindings after the originating view closes. Lock convenience endpoints issue normal scenario commands.

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
5. **Repair after call-out:** accepted, selected local base schedule plus one new unavailability.
6. **Provably infeasible:** insufficient eligible coverage with mapped sufficient conflict.
7. **DST transition:** overnight work across both spring and fall transitions.
8. **Large benchmark:** configurable 100+ people and thousands of candidate assignments.
9. **Synthetic practitioner workflow:** a deliberately fictional 12-clinician, four-week clinic/overnight-call schedule combines specialist coverage, explicitly authored quota/rest/maximum-hour and weekend targets, leave, call-out repair, and separate infeasible/time-limited outcomes. Do not derive names, dates or distinctive shift patterns from a supplied source; the declined grouping and linked split-clinic features are not fixture prerequisites.

For every fixture assert domain validation, expected normalized status, independent verification, required-rule outcomes, score invariants, stable explanation keys, import/export preservation, and revision/hash identity. Add valid, invalid, edge, empty-scope, period-boundary, lock-conflict, cancellation, stale-result, and resource-limit cases per rule. Differential exhaustive small models compare compiler results with the independent evaluator; intentional mutation proves detection.
Evidence fixtures distinguish fixed-other-assignment conflicts from globally feasible rescheduling, a short-budget unresolved run from proven infeasibility, and changed objective policies from comparable score deltas. Deterministic UI/CLI renders these distinctions without AI; conversational experiments and hypothetical entities remain in Phase-13 Branch K rather than expanding Phase 07's bounded comparison scope.
Performance assertions use the versioned benchmark manifests and record raw distributions/artifacts rather than one best run. Tests cover sub-threshold no-flicker behavior, longer truthful progress, one parent deadline across fallback/diagnostics, first-incumbent versus first-verified-feasible ordering, immediate accepted-result availability, optional-explanation delay/failure, cancellation latency, event/announcement caps, UI long tasks, and the calibrated small/typical/stress thresholds.
Export tests prove that unverified/rejected/no-solution runs cannot produce assignment/result/solution-derived outputs, while valid editable scenario/full backup and bounded diagnostic/infeasibility output remain available before acceptance. Workforce portable current/historical round trips preserve every rule/reference/time meaning; unknown semantics fail and declared nonsemantic extensions preserve.

The synthetic practitioner case jointly optimizes clinic and call even when views separate them. Independently check every confirmed Required total and restriction, report period/burden metrics and preference tradeoffs, and exercise fixed work plus a repair. Include boundary and infeasible cases sensitive to hour/count granularity and distinct-weekend versus shift counting; explicitly verify unsupported new-work grouping and linked split-session alternatives are not claimed. Aggregate hours balance alone does not prove a feasible assignment; resource exhaustion is not infeasibility. Freeze explicit reviewed policies and semantic expectations, not an arbitrary exact roster.

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

- the user has tested the complete Workforce workflow, feedback is resolved, and explicit checkpoint acceptance and authorization to enter Phase 08 are recorded under the [manual checkpoint policy](README.md#user-manual-checkpoint-gates);
- all thirteen baseline Required-rule categories, the confirmed practitioner additions (Required workload quotas and distinct worked-weekend limits), and all twelve preference categories satisfy the rule checklist and domain-rule definition of done;
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
