# Phase 13 — Post-MVP Roadmap

## Outcome

Extend the released local-first `eutheto` platform without weakening public-MVP contracts. The original immediate release delivers the complete official school-timetabling pack, deeper explainability, stronger verified portfolio routing and alternative generation, and a separate Nuxt documentation/community website. Independently, early post-MVP Branch K delivers bounded conversational experiments and staged voice capabilities from completed Phase 12, without waiting for school, transportation, or the original immediate-wave exit. The approved Branch-H direction likewise defines an independently gated complete H1 Workforce collaboration pilot after Phase12 and its H0/service/operating gates, with N1 separately proving managed operation of the same open capabilities. Dependency-gated planner evolution deepens repair, semantic comparison, plan health, trade-off exploration, history, resilience analysis, and person-aware fairness before any generalized platform claim. Later independent branches may add sandboxed domain packs, optional solver adapters, specialized algorithms, new targets, richer imports, expanded collaboration/server deployments, enterprise integrations, additional official domains, advanced AI integrations, optional telemetry, and additional hosted-service profiles. Every branch remains independently gated, capability-scoped, license-reviewed, resource-bounded, migration-safe, accessible, and optional relative to the open-source core and desktop application; independent eligibility does not promise simultaneous implementation or silently change priorities.

The public-MVP baseline is defined in [Phase 11](11-public-mvp-packaging-and-documentation.md) and approved in [Phase 12](12-stabilization-and-public-release-gate.md). Version and decision evidence is in [the assumptions ledger](assumptions.md).

## Source coverage

This phase fully maps blueprint Section 20; immediate/later scope in Sections 6.4–6.5; Section 12.2's post-MVP pack model; Section 14.11; Sections 16.3–16.6; Sections 24.9 and 24.12; future targets and distribution options in Section 27; every branch in Section 30; Appendix C.8; applicable validation/definition-of-done/stop-condition gates; and the explicitly deferred opportunities in [Performance and Solver UX Targets](performance-and-solver-ux-targets.md) and [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md). It also incorporates the accepted, scope-disciplined recommendations from **Eutheto — Feature Expansion and Product Evolution Roadmap**, dated 2026-08-29, source SHA-256 `b55dc5a3e423a955a7dabb0a62cebd283be54598d5fc0afc6f6bf90cd37bb7cb`.

Branch K incorporates selected recommendations from **Eutheto: AI, Voice, and External Assistant Implementation Plan**, dated 2026-09-04, as proposal input rather than a replacement architecture or approval of its suggested ADRs. Existing ADRs and Phase-10 policy control conflicts; external integration references below are investigation starting points, not verified support or entitlement promises.

## Dependencies

- The Phase-12 public MVP provides the authoritative headless Rust core, versioned internal/portable/result/share/Planning-IR contracts, reversible commands, transactional persistence/migrations, independent verifier, workforce and seating packs, bundled OR-Tools worker, experimental Pumpkin gate, desktop, CLI, editable scenario/full backup/add-or-replace restore, immutable offline HTML/PDF result sharing, security/privacy controls, and release pipeline.
- Changes remain backward-compatible within semantic-version policy or intentionally trigger a versioned migration and compatibility notice.
- Every new pack, rule, backend, service, provider capability, target, and serialized field completes the same definitions of done as public-MVP features.
- Desktop and CLI remain first-class without accounts, servers, hosted services, telemetry, proprietary solvers, third-party packs, or AI.

## Decisions and invariants

- Project name is `eutheto`; working CLI name `optimizer` remains unresolved until explicitly finalized.
- School timetabling is compiled and explicitly registered for its first release; a plug-in marketplace/native ABI is not a prerequisite.
- Backend candidates never become authoritative without independent domain verification and authoritative score comparison.
- A pack that compiles rules also supplies independent verification semantics. Backends stay domain-agnostic; packs do not depend on solvers; verifiers do not trust backend status.
- No untrusted native dynamic libraries load in-process. Third-party packs use a sandboxed component model with no ambient authority.
- Process boundaries do not waive licensing review. Blocked/proprietary components remain outside official bundles absent an intentional reviewed policy change.
- Portfolio execution shares one global budget, verifies each incumbent, compares authoritative score vectors, cancels safely, preserves provenance, and respects battery/resource constraints.
- Telemetry, collaboration, servers, remote solves, AI gateways, and hosted services are optional. Opening, validating, solving locally, verifying, and exporting never require them.
- Legal/regulatory presets remain non-authoritative starting templates unless separately maintained and jurisdiction-reviewed.
- Post-MVP optimization may reuse only immutable, hash-addressed validated artifacts and accepted-result hints. Cache, precompute, warm start, portfolio, or adaptive policy never changes semantics, verification, revision identity, one-operation resource accounting, or truthful status/provenance.
- Portable/report enhancements remain optional layers over current canonical schemas and purpose-built Share Result data. Encryption, signatures, automation, merge and hosting cannot weaken local unencrypted recovery, exact preview, atomicity, privacy, export/exit, or offline access.
- Planner-evolution features remain pack-evidenced before they become generic. A universal health score, fairness policy, repair cost, resilience probability, shared-person model, or cross-pack objective hierarchy is forbidden until several materially different domain packs prove common semantics.
- Explanations, comparisons, health indicators, repairs, stress tests, and AI paraphrases are derived from structured pack/verifier evidence. They distinguish fact from interpretation, sufficient from minimal conflict, correlation from causality, deterministic perturbation from probability, and configured policy from legal or safety authority.

### At/after-1.0 compatibility contract

Every branch delivered at or after 1.0 must:

- publish and honor a documented support window for scenario and portable bundle formats;
- keep released migrations immutable, export current canonical formats, preserve declared nonsemantic extensions, and reject unknown required semantics safely;
- preserve the public CLI and pack API according to semantic versioning;
- announce deprecations and retain deprecated public behavior for the documented deprecation period before removal;
- document worker-protocol compatibility ranges for each release or bundle workers that match the release's protocol exactly.

## Delivery branches and ordering

### Original immediate post-MVP wave

1. Complete official school-timetabling pack.
2. Deeper explainability and explanation-quality evaluation.
3. Stronger local portfolio routing and explicit alternative/diversity generation.
4. Nuxt documentation/community site separate from the desktop Vue/Vite SPA.

### Independent early post-MVP — Branch K

Enter from completed Phase 12, alongside rather than after the original immediate wave. Bounded conversational experiments and dictation have independent deliveries; realtime voice follows its own media/security gate and does not require experiments to ship. Workforce and seating provide the initial cross-domain evidence. Neither school/transport completion, generalized Scenario Compare, richer branching/merge, nor Branches H/I/N is a prerequisite for these local desktop deliveries. The restricted local MCP server is a separate optional increment; conditional runtime integration and organization administration do not gate planning or voice.

### Independent post-MVP collaboration — Branch H

The [Collaborative Planning and Hosted Service specification](collaborative-planning-and-hosted-service.md) was approved as roadmap direction on 2026-09-07. H0 maps reusable contracts and validates a pilot charter without production expansion. H1 enters from completed Phase12 plus its own service/authority/operating gates and delivers one complete Workforce organizational cycle, including publication and a subsequent change/repair—not an intake-only prototype. N1 separately proves managed operation of the same open capabilities. Neither H1 nor the original immediate wave, Branch K or Phase14 requires completion of the other. Allocation and priority remain explicit; independent entry does not promise simultaneous implementation.

### Later post-MVP

- generalized pack-evidenced Plan Health, arbitrary semantic Scenario Compare, named timeline milestones, and refresh-and-compare;
- ranked reviewed repairs, curated trade-off presets, deterministic stress tests, multi-period fairness, and later scenario branching;
- an experimental sandboxed Pack SDK after multiple first-party packs validate the internal contract, followed only later by a Pack Composer or community registry;
- advanced result capsules and explicit capsule-to-scenario workflows under the portable-data privacy and provenance gates;
- native specialized algorithms;
- optional HiGHS, SCIP, MiniZinc, commercial-solver, and routing adapters;
- expanded collaboration policies/deployments and enterprise integrations beyond the bounded H1 pilot, under their applicable gates;
- the proposed [Transportation domain-pack plan](14-transportation-domain-pack.md), independently entered from completed Phase 12;
- additional official domain packs;
- richer imports and structured import assistance;
- advanced AI/import integrations beyond the independently gated early Branch-K deliveries;
- encrypted/automatic backup, signed bundles, selected multi-scenario portability, richer result-report profiles, annotations/branding, and optional hosted sharing;
- optional anonymous diagnostics;
- Linux arm64, Windows arm64, and additional package/repository targets;
- additional optional hosted-service profiles beyond N1, never a prerequisite for the local product.

## Branch A — Complete official school-timetabling pack

### Domain contract

```rust
pub struct SchoolScenario {
    pub academic_calendar: AcademicCalendar,
    pub cycle: ScheduleCycle,
    pub periods: Vec<Period>,
    pub teachers: Vec<Teacher>,
    pub students_or_cohorts: Vec<Cohort>,
    pub rooms: Vec<Room>,
    pub courses: Vec<Course>,
    pub sections: Vec<CourseSection>,
    pub meeting_patterns: Vec<MeetingPattern>,
    pub requirements: Vec<TimetableRule>,
    pub preferences: Vec<TimetablePreference>,
    pub locks: Vec<MeetingLock>,
}
```

Every field is versioned, validated, migratable, import/export round-trippable, and typed. Prefer cohort/conflict-group modeling unless an accepted use case requires individual student data.

### Supported calendars and patterns

The first release supports every-class/every-day period schedules, college-like/block patterns with longer classes on selected days, rotating A/B or custom cycle days, and one weekly all-class/special-activity day.

A cycle is an ordered list such as `Mon, Tue, Wed, Thu, Fri` or `A1, B1, A2, B2, Flex`. The academic calendar maps dates to cycle days with holidays/exceptions. Solve over the representative cycle, then deterministically project to dates with tested time-zone/DST, exception, and export behavior.

Meeting patterns are data rather than UI branches and include:

- daily one-period meeting;
- Monday/Wednesday/Friday;
- Tuesday/Thursday double period;
- four meetings plus one lab block;
- one long weekly seminar;
- custom allowed pattern set.

A section references teacher options, cohorts, required meetings, allowed patterns, room requirements, duration, capacity, and linked lecture/lab components. Candidate generation rejects impossible teacher/cohort/room/time/capacity/equipment/pattern combinations before Planning-IR variable creation.

### Required-rule catalog

Each rule has unambiguous semantics, validation, compiler mapping, independent verifier, explanation, import/export, migration, boundary/infeasible fixtures, and model-size evidence:

1. teacher meetings do not overlap;
2. room meetings do not overlap;
3. student/cohort meetings do not overlap;
4. room capacity covers enrollment;
5. room equipment/type satisfies the section;
6. each section receives a valid meeting pattern;
7. all required meeting occurrences are placed;
8. teacher availability;
9. room availability;
10. cohort availability;
11. linked lecture/lab ordering or separation;
12. required spacing across cycle days;
13. maximum consecutive teaching periods;
14. lunch/flex protections;
15. fixed meetings and hard locks;
16. shared-resource capacity.

### Preference catalog

Each preference has explicit scope, metric, direction, weight/preset, score contribution, explanation, and deterministic ties:

1. teacher preferred periods/days;
2. avoid first/last periods;
3. spread meetings across the cycle;
4. keep a section in one room;
5. minimize teacher/learner room changes;
6. prefer room proximity;
7. avoid long gaps in teacher/student schedules;
8. balance difficult courses across days;
9. preserve an existing timetable;
10. align shared planning periods;
11. prefer specific pattern families.

Users distinguish requirements from preferences and inspect raw distributions/targets rather than an unexplained quality score.

### Modeling

Benchmark two formulations per section/domain shape:

1. **Pattern choice:** pre-enumerate valid meeting-pattern/room combinations and select exactly one.
2. **Occurrence:** create assignment variables for each required occurrence, candidate period, and candidate room with linking constraints.

Pattern choice fits finite meaningful patterns; occurrence variables fit custom calendars. Planning IR supports both. Precompute teacher/cohort/room conflict graphs and never create impossible candidates. Formulation selection is deterministic, benchmark-backed, and visible in model summaries. Preserve entity provenance for verification/explanations.

### Setup and results UX

1. Define cycle days and periods from presets.
2. Add/import teachers, rooms, cohorts, courses, and sections.
3. Map enrollment/conflict groups.
4. Define reusable meeting patterns.
5. Set teacher/room availability.
6. Add required rules and preferences.
7. Validate data and inspect the conflict graph.
8. Optimize.
9. Review by section, teacher, room, cohort, and cycle day.
10. Lock edits and repair.
11. Project the cycle to the academic calendar;
12. export editable portable data or full backup; and
13. share a privacy-reviewed accepted timetable through accessible one-file HTML/PDF, CSV, or calendar output.

Use purpose-built pattern editors and timetable grids, not a giant generic rule form. Imports provide templates, bounded streaming parsing, field mapping, preview, rejected-row reports, and transactional apply. Results provide status/score/evidence, filters, explanations, lock/repair, alternatives, calendar projection, and accessible portable/Share Result/print/CSV/calendar exports. The school pack supplies current/historical Portable Scenario and Share Result conversion/migration fixtures with stable IDs and unknown-semantic rejection. All paths support keyboard, screen reader, zoom/high-DPI, non-color cues, exact share privacy preview, offline one-file HTML/PDF, and measured virtualization.

### Acceptance scenarios

Mandatory fixtures:

- traditional five-day high-school schedule;
- A/B block schedule;
- college-style M/W/F and T/Th patterns;
- lab plus lecture coupling;
- shared teachers across departments;
- room equipment constraints;
- infeasible cohort conflicts;
- repair after teacher unavailability;
- large generated benchmark.

Each records hash/seed, schema version, size metrics, expected status when known, required-rule/score invariants, permitted backends, time budget, and baselines. Add exhaustive small truth/differential/mutation tests, property/metamorphic tests, every rule's valid/invalid/edge/empty fixtures, compiler goldens, migrations, import/export round trips, keyboard/accessibility, packaged E2E, explanation wording, performance, and usability review with school practitioners.

### School exit gate

Every entity, required rule, preference, pattern, formulation, view, import, projection, lock/repair, explanation, and export above passes its definition of done; compiler/verifier agree against exhaustive truth sets; all nine acceptance scenarios pass; practitioners review defaults and semantics; large models meet budgets; and committed fixtures contain no identifying real-world data.

## Branch B — Solver portfolio and alternatives

Deliver CP-SAT parameter-profile portfolios, concurrent OR-Tools/Pumpkin for exactly compatible subsets, verified-incumbent sharing where safe, local resource-aware scheduling, benchmark-trained deterministic routing, explicit diversity objectives, and evidence-backed acceleration:

- allocate one total wall-clock/CPU/thread/memory/log budget across children;
- project, score, and independently verify each candidate;
- compare authoritative lexicographic score vectors, never backend objective strings;
- preserve backend/version/profile/seed/budget/cache/hint provenance;
- cancel losers safely and account for cooperative cancellation delay;
- avoid multiple heavy solvers by default on battery-constrained systems;
- let **Optimize further** start a new explicit user operation after the first accepted result rather than silently extending the prior budget;
- prefetch likely calendar-derived travel pairs and cache recurring transit alternatives only through explicit provider/freshness/licensing policy; background work commits immutable snapshots and never hides network inside solve;
- model transportation uncertainty as explicit versioned ranges/policies with domain/verifier semantics and honest confidence wording, not as an unrecorded routing heuristic;
- reuse a prior accepted result only as a revision-checked search hint after a domain semantic-diff policy proves which assignments/locks remain eligible;
- cache or precompute only immutable validated compilation/geometry/provider-snapshot artifacts keyed by every semantic input/version/policy hash, with measured invalidation and cold-path parity;
- comparison/“what changed?” solves may reuse prior model state only when a semantic diff proves unaffected structure and cold rebuild equivalence;
- scenario-specific learned heuristics may influence deterministic recorded search ordering only; they never replace constraints, scoring or verification and must pass reproducibility/privacy/regression gates;
- introduce incremental solving or domain-aware dominance pruning only after differential/verifier evidence proves semantic equivalence and benchmarks prove material benefit;
- allow adaptive budget/profile recommendations only when visible, deterministic from recorded inputs, bounded by the user’s selected operation, and never auto-spending a larger budget;
- require alternatives to declare diversity metrics and remain required-rule feasible;
- expose a small pack-defined Tradeoff Explorer preset set only after semantic Scenario Compare is stable; labels such as `Recommended`, `Lowest cost`, `Least disruption`, or `Most flexible` map to versioned objective configurations, not hidden narrative or an unbounded weight-slider surface;
- keep opaque remote AI outside backend selection and resource policy; and
- treat cross-solver decomposition of one connected component as separate research requiring proof of independence and merge equivalence.

## Branch C — Deeper explainability

Deliver more efficient bounded minimal-conflict shrinking; multiple independent conflict sets; causal repair explanations; visual rule-dependency graphs with non-visual equivalents; sensitivity analysis; preference shadow-price-like summaries only where mathematically meaningful; richer counterfactual batches; and explanation-quality evaluation with domain users. Preserve sufficient-versus-minimal, causal-versus-correlated, and configured-rule-versus-legal-certification distinctions. Add cancellation/resource budgets, provenance, replay, and certainty-wording tests.

## Cross-cutting planner-evolution sequence

These capabilities deepen the existing solve → understand → repair → reflow → compare → share loop. They are dependency-ordered evidence tracks, not one release bundle and not permission to retrofit speculative generic abstractions into the MVP.

```text
structured pack/verifier evidence
  → Why this plan? → reviewed repair alternatives → semantic comparison
accepted result + explicit change costs
  → Lock/Prefer/Move/Reflow → plan stability → timeline and branching
pack-owned health metrics + immutable run inputs
  → deterministic stress tests → resilience objectives → trade-off exploration
run manifest + frozen provider snapshots
  → reproducibility → refresh-and-compare → privacy-reviewed result capsules
```

### Early post-MVP

1. **Richer Constraint Clinic behavior:** extend the existing infeasibility workflow with several independently verified, explicitly reviewed repair alternatives. Rank by pack-defined named disruption/cost categories; say `minimum` only when the search proves it; never silently weaken a requirement or propose relaxing safety, accessibility, legal, or otherwise protected rules as an ordinary compromise.
2. **Generalized Plan Health:** let each pack expose typed, dimensioned buffer, tightness, dependency, and criticality evidence with source/freshness/uncertainty metadata and accessible list/table equivalents. Generalize only semantics demonstrated by multiple packs; do not collapse incomparable facts into one universal health score or imply independent slack where constraints are coupled.
3. **Full Scenario Compare and timeline:** compare any two retained revisions/runs using input changes, locks/preferences, assignments, independently verified score/category deltas, affected entities, proof status, and evidence-backed reasons. Add named milestones, restore points, retention controls, and `Duplicate from here`; retain separate identities for scenario revision, input snapshot, solver run, and accepted result.
4. **Curated Tradeoff Explorer:** begin with a small pack-defined set such as `Lowest cost`, `Least disruption`, `Most flexible`, or `Fairness first`. Each label maps to recorded objective configuration; every alternative is independently verified and rendered through semantic comparison. Do not present incomparable metrics as one magic score.
5. **Refresh and compare:** calendar, routing, price, or other provider updates create a reviewed immutable input snapshot and a new run; they never overwrite the accepted result's basis. The user can review source/freshness changes, refresh and compare, or retain the prior snapshot.
6. **Person-aware fairness expansion:** expose named burden definitions, populations, targets, distributions, deviations, and pack-owned balancing presets. Accessibility and safety remain requirements, not fairness trade-offs; equal treatment is not asserted to be equitable.

Branch-K isolated experiments reuse the existing revision/run/result identities, verified metric/evidence contracts, and bounded comparison views they actually need. They do not wait for arbitrary-pair Scenario Compare, the full timeline, curated trade-off UI, or the complete planner-evolution sequence above; neither do they claim to deliver those broader capabilities.

### Advanced post-MVP

1. **Deterministic stress tests first:** packs define bounded, understandable perturbations and recovery criteria. Each test records its base revision, immutable inputs, perturbation, budget, candidate, independent verification, and repair cost; supports progress/cancellation; and never emits a success probability without a reviewed empirical distribution.
2. **Resilience objectives and interactive trade-offs:** only after deterministic tests, performance evidence, and domain semantics mature may a pack trade a recorded amount of nominal score for a recorded resilience measure or expose interactive movement among competing objectives.
3. **Multi-period fairness:** burden history and rotation require explicit retention, correction/deletion, time-window, baseline, weighting, and privacy semantics. Historical disadvantage is not inferred from opaque scores.
4. **Scenario branching and reconciliation:** richer generic branching retains provenance and immutable base identities; guided reconciliation previews pack-defined semantic conflicts and applies revision-checked commands. Do not expose source-control terminology by default. This advanced scope is distinct from Branch-K isolated snapshots and reviewed promotion, which require no general branching or merge engine.
5. **Advanced conversational repair:** extend Branch-K reviewed conversations to richer repair alternatives only as those deterministic capabilities pass their own gates. Basic typed conversational edits remain Phase 10, and early bounded experiments remain Branch K; neither is deferred to this advanced track. The user reviews hard-versus-soft meaning and affected scope before apply; solving and independent verification remain deterministic authorities.
6. **Advanced result capsules:** precomputed alternatives, annotations, recipient-specific disclosure, or capsule-to-scenario conversion follow Branch O and the cross-cutting portable-data specification. A capsule never gains ambient network, filesystem, credential, or mutable-scenario authority.

### Platform research gates

- Publish an experimental Pack SDK only after at least three materially different completed first-party packs demonstrate the internal contract and its migration, verification, explanation, presentation, resource, and security boundaries. Compatibility remains explicitly experimental until its conformance corpus survives independent third-party use.
- A visual Pack Composer may follow the SDK only if it emits the same reviewed declarative representation and conformance evidence; it must not create a second pack format or permit arbitrary native/web code. A community catalog additionally requires signatures, capability disclosure, compatibility, recovery, license, moderation, and sandbox evidence while preserving sideloading.
- Cross-pack evolution proceeds only through explicit user-approved data handoff, then independently useful conflict detection, and only later research into coordinated solving. Stable typed identities and namespaces do not create a universal shared-person/resource model. Coordinated solving requires versioned shared semantics, priority governance, provenance, independent verification, budget accounting, privacy boundaries, and semantic merge evidence.

### Planner-evolution acceptance

- Health, comparison, repair, fairness, resilience, and trade-off claims round-trip as typed evidence and regenerate deterministic non-AI views.
- Counterfactual and causal wording is backed by the exact recorded diagnostic operation; otherwise the UI reports verified differences without invented causality.
- Every nested solve shares a visible parent operation budget, remains cancellable, preserves an already accepted result, and records partial/inconclusive outcomes honestly.
- Representative packs prove accessible keyboard/screen-reader views, deterministic fixtures, migration/portable behavior, privacy previews, resource bounds, and cold-path equivalence where reuse is introduced.
- User research validates product language and decision usefulness; it cannot waive verifier, privacy, security, compatibility, or evidence gates.


## Branch D — Native specialized algorithms

Add project-owned Rust or permissively licensed crates only when the router proves exact structure:

- bipartite matching;
- minimum-cost assignment;
- connected components;
- shortest path;
- minimum-cost flow;
- graph-coloring heuristics used only to seed a full solver;
- geometry preprocessing and spatial indexing.

Results still flow through normalized projection, authoritative scoring, and independent verification. Require capability proof, property/differential tests, cancellation/resource behavior, benchmark benefit, license review, and an ADR where routing changes materially.

## Branch E — Additional solver adapters

### HiGHS

Reverify current HiGHS (discovery value 1.15.1) at implementation. Use for LP/MIP/QP subsets. Prefer an out-of-process worker unless a stable Rust binding, cancellation, isolation, packaging, and licensing case is proven. Inspect HiPO/dependency licenses.

### SCIP

Reverify current SCIP (discovery value 10.0.3) and use only where benchmarks show benefit. Versions before 8.0.3 are ineligible under their former ZIB Academic License. Inspect SoPlex/GCG/IPOPT and all assembled components.

### MiniZinc

Use for research, comparison, and prototyping. Scenario/Planning IR remain authoritative. An adapter may export compatible subsets to MiniZinc/FlatZinc and invoke installed or separately reviewed permissive solvers. MiniZinc 2.10.0 is MPL-2.0, but bundled solvers/components have distinct licenses; review each exact component.

### Proprietary solvers

Gurobi, CPLEX, Xpress, and peers are user-provided only: never ship binaries/license material; discover only after explicit action; display provider requirements; isolate optional adapters; never require them to open, verify, migrate, or export.

### Routing worker

A vehicle/service-routing worker is permitted only behind domain-neutral capabilities, verifier, license, process, packaging, and benchmark gates.

Every backend needs exact version/hash/license manifest, capability matrix, translation tests for every claimed IR node, status/cancellation/timeout normalization, malformed-output/crash isolation, benchmark corpus, independent verification, target packaging/update strategy, accurate stability label, and an ADR establishing material benefit.

## Branch F — Sandboxed domain-pack SDK and catalog

Official school remains compiled. Third-party packs use a versioned WASM component ABI with a narrow typed host API, deterministic memory/fuel/time/output limits, signed manifests, and no ambient filesystem/network/process/credential access. Native `.dll`, `.dylib`, or `.so` loading is forbidden.

Manifest fields:

- pack ID/version/license;
- required host API version;
- requested capabilities;
- memory/fuel limits;
- signed package digest;
- UI contribution type;
- migration functions.

SDK supplies schema, command, validation, compile, verify, explanation/view, import/export, migration host interfaces; constrained UI extension slots; local pack manager; compatibility/downgrade/recovery; signatures; optional Nuxt catalog; and a conformance kit. A compiler-capable pack must supply verification semantics. Official review complements, never replaces, sandboxing.

## Branch G — Nuxt documentation/community website

Use a separate Nuxt site for landing pages, searchable docs, domain/solver capability catalogs, rule reference, tutorials/examples, contributor docs, downloadable releases/checksums/signatures/SBOM/provenance, optional pack catalog, and benchmark dashboards. Keep desktop Vue/Vite SPA separate. Site remains accessible and privacy-minimizing; no account/telemetry is required to read docs or download releases.

## Branch H — Collaboration and service mode

The provisional `optimizer serve` uses a separately designed authenticated API, never exposed Tauri commands. Initial local mode binds loopback, uses explicit tokens, and warns that internet exposure requires a hardened profile.

The [detailed Branch-H contract](collaborative-planning-and-hosted-service.md#delivery-and-ownership) owns H0 reuse/pilot validation and H1's complete collect/reconcile/verified-solve/approval/publication/change/repair cycle. It reuses ordinary typed commands, scenario revisions, immutable runs/results and Share Result projections. No universal shared-person store, parallel constraint authority or generic workflow engine is a foundation requirement. Requested treatment and accepting authority remain distinct from effective Required/Preference meaning.

Before any production participant data, satisfy the applicable concurrency/revision, audit, authentication/session/invitation lifecycle, subject/field authorization, isolation, quota/cancellation, credential/egress, TLS/proxy, abuse, retention/deletion, backup/restore, update/recovery and incident/support gates. Start with one documented self-hostable reference deployment and minimal workflow roles; managed operation adds N1 evidence rather than exclusive software. H2 expands only from demonstrated need. Scheduled jobs, broad webhooks/import pipelines, additional permissions and organization administration are not all mandatory H1 features.

This branch does not gate Branch-K desktop experiments, voice, a restricted local MCP server, or a locally embedded supported runtime. A remote/managed deployment must satisfy the applicable service identity, authorization, concurrency, quota, audit and operations controls; it cannot expose Tauri IPC. Those controls do not require an unrelated service to implement H1 campaigns or its complete product workflow.

## Branch I — Enterprise integrations

Optional directory/HRIS import, calendar sync, school information systems, event/guest platforms, signed policy packs, user-provided solver discovery, server SSO, and audit/report exports. Keep vendor fields out of core models. Each adapter needs typed boundary, disclosed data scope, credentials, rate/retry/cancellation, conformance fixtures, version mapping, preview/transactional apply, audit/redaction, license review, offline/failure behavior, and removal/migration path.

The existing Phase10 local privacy/provider boundaries remain binding; Branch K carries them into its own extensions. Central organization provider policy, SSO and advanced administration enter only with applicable H/I deployment gates. H1 does not wait for Branch-K features, and organization administration is not a dependency of desktop planning, voice or local MCP.

Branch I owns additional enterprise adapters, not H1's minimum subject-scoped permissions. The [pilot decision gate](collaborative-planning-and-hosted-service.md#pilot-validation-and-decision-gates) selects only necessary integrations; SSO, delegated administration and vendor breadth are evidence-led additions, with open software availability and distinct operating responsibilities.

## Branch J — Additional official packs

Household transportation is planned in detail as [Phase 14 — Transportation Domain Pack](14-transportation-domain-pack.md). Phase 14 is a sibling post-MVP branch entered directly from completed Phase 12; completing this Phase-13 umbrella is not its entry gate. Its calendar, routing, and optional transit inputs require provider-neutral application/infrastructure adapters to fetch, validate, normalize, and persist bounded immutable local snapshots before network-free pack compilation and verification. Provider selection, authentication/scopes, licensing, caching, privacy, persistence/export, and offline/manual behavior remain explicit Phase-14 gates; no provider is selected or shipped here.

Other candidates:

- volunteer scheduling;
- conference/session scheduling;
- sports tournament scheduling;
- exam timetabling;
- room/resource booking;
- maintenance scheduling;
- manufacturing/job-shop planning;
- task/team assignment;
- vehicle/service routing;
- housing/roommate assignment;
- election-worker scheduling with jurisdictional caution.

Select other packs on reusable primitives, domain maintainers, and clear UX—not modelability alone. Define privacy, rules/preferences, imports/exports, IR needs, verifier, explanations, accessibility, benchmark corpus, practitioner review, migration, ownership, and disclaimers before commitment.

## Branch K — Early conversational experiments, voice, and optional integrations

### Entry, authority, and local privacy

Completed Phase 12 is the entry gate. [Phase 10](10-ai-assistant-mvp.md) remains the complete text-AI MVP after deterministic workforce and seating, including profiles and opt-in local conversation retention; this branch extends rather than postpones it. Planning experiments and voice have separate early exits, independent of the school/portfolio/explainability/site wave and Phase 14. Use the existing command journal/undo, solver jobs, typed catalogs, revision/result identities, and evidence services as single authorities; do not introduce a universal agent framework, duplicate approval store, or generic branch/merge platform.

- Every persistent scenario mutation is typed, validated, previewed, explicitly applied through trusted Eutheto GUI/keyboard controls, revision-checked, atomic, and undoable. AI never accepts its own proposal, generates backend code, invents domain facts, bypasses independent verification, reads arbitrary files, executes shell, or receives credentials.
- Preserve Phase-10 separate explicit solve-start confirmation. A conversational request or open voice session is not compute permission. Any future session-budget compute grant requires an explicit reviewed native permission design and any necessary future ADR **before enablement**, with selected scenario/snapshot, allowed operations, total budget, expiry, revocation, and visible remaining allowance; it cannot authorize writes, protected-rule relaxation, provider changes, or export/import/backup/privacy actions. No such exception is approved here.
- Native code owns credential entry/custody, authenticated provider networking, authorization, and transport. Secrets, including ephemeral provider/session credentials, never enter Vue or ordinary IPC. OS-store failure disables the affected integration with an actionable error; frontend credentials or session-only fallback require a future explicit ADR, not an implementation shortcut.
- Carry local privacy controls into every increment: AI-disabled makes no AI network requests; local-only restricts Eutheto traffic to approved local routes without claiming an endpoint cannot forward upstream; external routes require disclosed destination and permitted data categories with no silent fallback. Apply minimization to typed input, transcript, tool context, and output; treat imported notes and transcripts as untrusted data. Users inspect/delete opt-in local conversations and transcripts with bounded retention; raw audio is not retained in logs, databases, backups, or diagnostics and transient media buffers are cleared on teardown.
- AI gains no portability, publication, privacy-selection, or import/restore/backup authority. Any later structured import assistance uses existing bounded parsers and deterministic preview/transactional apply; no assistant receives raw bundle or arbitrary-file access. Organization-wide administration is later H/I scope, not a prerequisite for these local controls.

Retain later domain-guided conversation templates, improved local-model capability profiles, reusable typed rule macros, and deterministic redaction/pseudonymization as bounded extensions of the existing profiles and catalogs. Macros expand to the same reviewed typed commands, not a second rule language; provider OAuth remains conditional on official suitable third-party flows. None of these later improvements delays the early exits below.

### K1 — Bounded conversational planning experiments

1. Create a visibly isolated experiment from an immutable selected baseline snapshot plus typed changes; record scenario/base revision, input snapshot, proposed assumptions, and hypothetical entities. Resolve missing eligibility, availability, capacity, dates, and hard-versus-soft meaning through the pack catalog, never model invention. Label hypothetical entities and results throughout, including retained views, and never silently change the working scenario or accepted result.
2. Submit only validated, explicitly confirmed requests to existing asynchronous solver jobs. Record the exact baseline and candidate input/revision, run and accepted-result identities, pack/compiler/verifier versions, objective configuration, metric definition/version/unit/direction/population/time window, budgets, assumptions, and proof/verification status. A completed job is not necessarily feasible, accepted, or optimal; a short-budget unknown outcome is not infeasibility.
3. Compare independently verified outcomes under named, compatible metrics. When objectives or weights differ, evaluate both results under a common recorded comparison policy where meaningful; otherwise report raw comparable metrics and explicit non-comparability, never compare unlike aggregate scores. “Improve fairness without increasing overtime” requires an explicit pack-supported overtime bound derived from the identified baseline, not a large penalty weight. Reject unsupported bound semantics rather than approximating a requirement.
4. Provide baseline-versus-experiment differences, assignments/disruption, required-rule checks, metric deltas, assumptions, and evidence-backed status through deterministic accessible views. Distinguish direct fixed-assignment diagnostics from global rescheduling; only the exact recorded diagnostic justifies causal claims. The experiment remains useful without AI paraphrase.
5. Support **Retain experiment**, **Discard**, **Keep as scenario**, and **Apply reviewed changes**. Retention/expiry and crash recovery keep temporary artifacts visibly separate, with predictable deletion and no implicit ordinary export inclusion. Promotion uses existing copy/command operations, fresh revision validation and approval; stale promotion produces a new review, never hidden rebase or partial application. General three-way merge, arbitrary branching, and richer timeline reconciliation are not prerequisites.

### K2 — Dictation, then independently gated realtime voice

Dictation ships first: explicit microphone activation → permitted transcription → editable transcript → ordinary text copilot. Sending the corrected transcript is explicit; partial utterances never apply writes or start jobs. A cascaded local/approved STT → text → TTS mode may follow but is labeled accurately, not called native realtime/full-duplex voice.

Before realtime delivery, prove capture, playback, microphone permission denial/revocation, device changes, echo/interruption handling, and the selected native-owned transport in the actual packaged Tauri application on each advertised OS. Measure under representative concurrent workforce/seating solver load, including audio responsiveness, resource pressure, and teardown; standalone browser success or an STT demo is insufficient. Proceed from safe capture/playback/shutdown to the provider route, read-only tools, then typed proposals with existing review controls. Unsupported platform/provider/model/transport combinations stay disabled with an explanation.

- Push-to-talk is the default. Show capture/playback state, provider/destination, scenario, current proposal/experiment, active jobs, and an always-accessible local **Stop everything in this session** control. Stop acknowledges locally and stops capture/playback/transports while requesting job cancellation even without network or speech recognition. All controls have keyboard/screen-reader equivalents.
- Distinguish **Stop playback**, **Mute capture**, **End voice session**, **Cancel analysis**, and **Cancel experiment**. Mute is not disconnect; end stops media/tracks/transports and clears buffers but does not silently cancel separately visible solver jobs. The combined Stop makes its broader cancellation scope explicit. Cancellation is not rollback of committed writes; show the committed change and normal undo.
- Opening/importing/restoring a scenario, application restart, retained conversation replay, or reconnection never activates capture, resurrects approvals, or automatically starts jobs. Require fresh explicit activation after teardown or loss of microphone permission.
- Interruptions stop audible output and reconcile what was actually heard. Corrections supersede draft interpretations; partial/duplicate/reordered/reconnected tool events cannot execute partial arguments or replay side effects. Bind events to session/turn/proposal/revision/job identities; late results remain attached to the original experiment and cannot overwrite or be narrated as the current answer.
- GUI/keyboard approval tied to the current authoritative diff remains mandatory for persistent writes, and solve start retains its separate confirmation. Background speech or an ambient “yes” is never authorization; speaker recognition and host/model confirmations confer no authority. Spoken authorization is not part of this delivery.
- Audio policy is separate from text policy: no external data permits only fully local selected components or disables the mode; filtered external text may use local transcription plus approved text/TTS filtering but forbids direct external microphone streaming; external audio requires explicit destination/data-sharing consent and current provider-handling review. Transcript redaction cannot undo raw-audio disclosure.
- Numerical/status narration uses deterministic text rendered from independently verified result/evidence cards, with visible evidence links, units, limitations and baseline identity. Free-form dialogue cannot invent progress, metrics, feasibility, optimality, or causality. Pending, infeasible, unknown, cancelled, and superseded outcomes remain distinct.

### K3 — One parent budget before optional reasoning delegation

Use the existing asynchronous solver-job lifecycle first, so conversation remains responsive without a second reasoning agent. All experiment/reasoning children share one visible parent operation budget for time, solver CPU/threads/memory, request/token/output limits and known or estimated provider usage; missing cost data is unknown, not free. Bound queues/concurrency, default to one heavy foreground solve, and preserve Phase-10 native confirmations. Starting another turn or child does not reset limits or silently extend the operation.

Cancellation propagates to queued/running work, requests cooperative solver cancellation with bounded fallback, preserves verified incumbents and truthful termination reasons, and rejects stale completions. An optional single-level reasoning delegation is enabled only after measured quality or responsiveness benefit over this baseline: bounded task count/deadline, same approved profile/data/tool scope, explicit evidence references, no recursion, no permission/provider escalation, and no independent budget. Delegated findings are untrusted data, not instructions or proof. Numeric performance/concurrency targets remain provisional until measured on declared workloads/platforms; no delegation framework is a first-release dependency.

### K4 — Restricted local MCP server, not inference or an MCP client

Build an optional Eutheto **MCP server** over the existing typed application services: bounded scenario/catalog/result/evidence reads and proposal/confirmed-job requests, with caller-specific exposure. An OpenAI-compatible inference adapter is not this server, and this milestone does not add a general MCP client or arbitrary third-party tools.

Start with explicitly launched authenticated local stdio or loopback transport and restricted scenario/resource/operation scope. Local process identity/permissions or per-session credentials must be verified; loopback alone is not authorization and network transport needs origin/DNS-rebinding protections. Bound request sizes, rates, time, output and parent compute budgets; preserve proposal/revision approval, idempotency/replay protection, cancellation, disconnect/revocation, redacted activity records, and independent result verification. Host tool annotations, screenshots, and host confirmation prompts never replace Eutheto authorization or its exact-diff approval. Inference credentials are not service-access tokens; never pass through tokens for another audience.

Disclose that an external host can forward returned data even if MCP is local. Verify actual host transport, tools, authentication and voice support before advertising it. A cloud host cannot reach a local desktop by implication: remote access requires a separately reviewed, explicitly enabled/revocable reachability mechanism; no silent tunnel, inbound listener, scenario upload, or hosted proxy. Only remote/managed deployments depend on applicable H service-security/operations, I organization-integration, and N hosted-service gates. Host-specific UI/distribution follow the proven local tool boundary and never gate the desktop product.

### K5 — Conditional supported embedded-runtime investigation

Investigate Codex App Server or another officially supported runtime as an optional local-capable provider-runtime route, not inherently an Eutheto-hosted service and not generic API access. No supported provider/model/entitlement is asserted here. This investigation never blocks the first experiment, voice, or local MCP release.

Before enabling an adapter, require official evidence that suitable third-party embedding, authentication, and the intended scheduling/tool use are permitted; distinguish identity, account/workspace entitlement, inference capability, and quota/billing. No extraction, impersonation, consumer-session reuse, token repurposing, or unofficial OAuth. Use OS credential-store custody through the native boundary, with explicit signout/revocation, cancellation, quota exhaustion and unavailable-store behavior; authentication success alone does not enable inference.

Prove confinement with inherited native shell, filesystem, network, tools/MCP servers, configuration, and plug-in capabilities disabled or isolated by an enforceable boundary. Only the explicitly approved native-brokered inference route and Eutheto typed capabilities may remain; prompts or runtime “approval” modes are not isolation. Inspect effective configuration and negative tests for hidden defaults, startup hooks, and ambient credentials. If that boundary cannot be enforced, keep the adapter disabled. Realtime/voice entitlement, media route, and cost are independently reviewed, never inferred from a text subscription. Exact runtime/dependency licenses, redistribution/bundling terms, notices/SBOM, update/compatibility, signed packaging, supported targets, resource limits and removal must pass before distribution; applicable H/I/N gates attach only to remote/managed deployments.

### K6 — Acceptance and independent staged exits

Use synthetic versioned workforce **and seating** scenarios now; extend evaluation to school only after Branch A's pack gates and transportation only after Phase 14's gates. Later domain coverage is not an early release prerequisite, and illustrative nurse/student/travel dialogue does not promise unsupported pack semantics.

- **Workforce:** clarify a hypothetical part-time worker's qualifications, dates and availability; refine weekdays to selected days without altering the real roster; compare verified coverage and declared fairness metrics. For “improve weekend fairness without increasing overtime,” enforce the exact baseline overtime bound, show raw metrics under a common policy when objectives differ, and never imply hiring guarantees.
- **Seating:** propose a hypothetical guest or party with resolved identity, relationship and accessibility assumptions supported by the catalog; revise its size or seat request without changing the accepted plan. Preserve capacity, accessibility, minimum-distance rules and locks; compare verified preference/assignment disruption metrics. Unsupported requests require clarification/refusal rather than invented seat, relationship, or safety semantics.
- **Both domains:** retain/discard/copy/promote experiments; reject stale approvals after another edit; distinguish proven infeasibility from deliberately time-limited unknown outcomes; compare changed objectives without misleading aggregate scores. Prove prompt injection, duplicate/reordered calls, cancellation and superseded completions cannot mutate the baseline or fabricate results; ordinary UI/CLI flows remain usable with AI disabled.
- **Voice in both domains:** correct an utterance before approval, inject background “yes,” interrupt numerical narration, switch/deny/revoke devices, disconnect the network during Stop, cancel under solver load, and restart/import/restore without capture or replay. Enforce filtered-text-only versus external-audio policy; retain no raw audio and expose no credentials, including ephemeral credentials, to Vue/ordinary IPC.
- Measure intent/entity resolution, clarification and hard/soft correctness, unsupported-request handling, evidence/numerical accuracy, correction effort, cancellation/stale-event handling, latency and resource use with recorded pack/model/prompt/adapter versions. Forbidden writes and credential exposure must be absent in the maintained adversarial corpus; this is a release gate, not proof of universal security. Use domain/verifier truth and human review, not a model judge alone. Live-provider evaluations are opt-in, synthetic, budgeted and separate from deterministic suites.

| Increment | Independent exit |
|---|---|
| K1 early planning | Isolated typed experiments, exact identities/common evaluation/explicit bounds, one-parent jobs and cancellation, reviewed retention/promotion, and workforce/seating evidence pass; no school, transport, generic merge, or voice prerequisite. |
| K2 early dictation | Editable transcript, explicit activation/send, local privacy, credential boundary, accessible controls and verified platform media teardown pass; no K1 or realtime prerequisite. |
| K2 realtime | Dictation baseline plus packaged media/transport evidence under solver load, interruption/corrections/stale-event handling, offline Stop, audio policy and deterministic narration pass. K1 is required only when advertising voice-driven experiments; K3 delegation is optional. |
| K3 delegation | Existing async-job baseline is measured first; bounded single-level delegation demonstrates benefit and all scope/budget/cancellation/authorization invariants. |
| K4 local MCP | Tested host/transport, authenticated scoped tools, Eutheto approvals, replay/resource/disclosure/revocation gates pass without H/I/N services. Remote/managed variants exit separately under applicable service gates. |
| K5 runtime | Official third-party terms/entitlement, secure custody, effective tool confinement, cancellation/signout/quota, independent voice entitlement where claimed, and exact-artifact license/package evidence pass; otherwise disabled. |

Each shipped increment also meets the common Phase-12-derived acceptance and branch exit requirements below. None authorizes a policy exception or makes another optional increment mandatory.

### Investigation references, not support promises

Recheck current official [Tauri capabilities](https://v2.tauri.app/security/capabilities/), [realtime audio](https://developers.openai.com/api/docs/guides/realtime), [WebRTC guidance](https://developers.openai.com/api/docs/guides/realtime-webrtc), [Codex App Server](https://developers.openai.com/codex/app-server), [Codex authentication](https://developers.openai.com/codex/auth), [MCP specification](https://modelcontextprotocol.io/specification), and [native OAuth guidance](https://www.rfc-editor.org/rfc/rfc8252.html) when investigating a concrete adapter. Client-side ephemeral-credential examples do not override Eutheto's native-only secret boundary. Record dated official provider terms, entitlement and exact-artifact evidence separately before enablement; mutable examples, model names, prices or successful login are not release authorization.

## Branch L — Optional anonymous diagnostics

If added after the no-telemetry MVP: explicit opt-in; publish schema/collection code; collect no scenario content or person/guest/student names; permit inspection/deletion; keep full functionality when disabled. Before release define purpose, event/field catalog, pre-send preview, retention, endpoint ownership, transport, sampling, deletion, versioning, privacy obligations, and re-identification assessment.

## Branch M — Future targets/distribution

Potential additions after reliable CI/end-user testing:

- Linux arm64 desktop/CLI/worker;
- Windows arm64 when Tauri, WebView2, OR-Tools, signing, runners, and demand justify it;
- rpm or adjusted AppImage/deb mix;
- universal macOS bundles;
- signed apt/rpm repositories;
- additional updater/catalog hosting.

Each completes the same one-install, worker, license/SBOM/provenance, signing, updater, migration, offline, accessibility, E2E, and clean-machine gates as Phase 11/12. Development-shell availability is not release support.

## Branch N — Optional managed services

N1 is the [managed collaboration operating milestone](collaborative-planning-and-hosted-service.md#delivery-and-ownership) over H1's documented self-hostable open capabilities. It requires evidenced isolation, supported workloads/admission, credential custody, monitored health, retention/deletion, controlled updates/recovery, restore-tested backups, incident/support responsibility and practical export/exit before production use. Baseline production safety is already required for H1; it is not postponed until managed scale. Official software functionality is not sold as proprietary feature unlocks, and technical resource limits remain legitimate.

Other update/catalog hosting, remote solves or service profiles remain independently optional with their applicable governance, security/privacy, authorization, compatibility, recovery, abuse and disclosure gates. Managed AI, SMS, hybrid workers and unrestricted APIs are demand-led investigations, not initial N1 offerings. No deployment promises unlimited resources, automatic local-endpoint reachability or institutional suitability. Desktop and local Branch-K capabilities remain independent.

Branch-K local MCP and local-capable embedded runtimes do not inherently require this branch. Managed gateways, hosted runtimes, or remote assistant access inherit these gates only when that deployment is explicitly selected; they never become prerequisites for early desktop experiments or voice.

## Branch O — Portable data, backup, and sharing enhancements

Enter each enhancement independently; none is required to keep opening, exporting, restoring, printing, or reading public-MVP data locally.

- **Selected multi-scenario portability:** export/import an explicit reviewed set, preserving stable identity and collision policy without exposing the full library.
- **Encrypted portable bundles:** use a reviewed versioned authenticated-encryption envelope, explicit key/passphrase recovery and wrong-key/corruption behavior, memory/work-factor limits, no plaintext temp leakage, and interoperability fixtures. Encryption never replaces manifest/schema validation after authenticated decryption.
- **Automatic rotating backups:** opt-in schedules, bounded retention, storage/permission/full-disk states, visible health/last-success, no overlap with migration/restore, atomic snapshots, restore drills, and user-controlled destinations. Failure is surfaced; it never silently deletes the last known-good backup.
- **Authenticity and integrity:** optional signatures identify exactly what is signed, signer trust/revocation/expiry and unsigned behavior; they never imply domain correctness, legal compliance, or result optimality.
- **Semantic merge:** only after pack-defined field/entity conflict semantics, three-way ancestry, preview, validation, atomic apply/undo, migration ordering, and exhaustive collision/reference tests. Until then users choose Create copy, Replace, or Skip.
- **Richer offline reports:** additional coordinator/custom profiles, annotations, branding, richer diagrams/maps and organization privacy policies only through versioned Share Result fields, exact preview, inert rendering, accessible equivalents, licensing/provenance and zero-required-network behavior.
- **Hosted sharing:** optional publish/revoke/expire/access-control/audit/download/export/delete flows over the same immutable Share Result artifact. Define authentication, authorization, tenancy, encryption, abuse, retention, region, incident response and billing/availability boundaries; hosted access can fail without affecting the local file.
- **Organization migration and deployment recovery:** use distinct declared profiles and destination identity/authority mapping, not ordinary scenario archives as a universal organization backup. Apply the [privacy/retention/recovery contract](collaborative-planning-and-hosted-service.md#privacy-retention-and-recovery): no imported privilege/session/invitation authority, safe restored-service quarantine, revocation/deletion reconciliation and no automatic job or notification replay.

Every enhancement has version/migration/downgrade behavior, bounded resource and cancellation policy, threat/privacy/licensing review, desktop/CLI parity where applicable, permanent compatibility/security/recovery fixtures, docs, and an exit path back to the local open format. Provider or third-party content remains excluded unless its redistribution terms and user choice permit inclusion.

## Ordered work packages

### Immediate wave

1. School schema/calendar/cycle/pattern/section contracts and migrations.
2. All 16 school required rules and 11 preferences: validation/compiler/verifier/explanations.
3. Conflict graphs, candidate filtering, formulation benchmarks, routing.
4. School setup/import/pattern editors/views/locks/repair/projection/export/accessibility.
5. Exhaustive school QA, all nine scenarios, performance, practitioner/usability review.
6. Explainability improvements.
7. Portfolio/resource/diversity improvements.
8. Separate Nuxt site.

### Independent early Branch-K work packages

From completed Phase 12, run the K1 isolated-experiment/verified-comparison delivery and K2 dictation/media-feasibility delivery independently of items 1–8. Realtime follows K2's own gate; the existing one-parent asynchronous-job controls apply before optional K3 reasoning delegation. K4 local MCP and K5 conditional runtime investigation are separately gated optional increments, not dependencies of those early deliveries. Use K6's staged exits rather than waiting for the original immediate-wave exit.

### Independent Branch-H and N1 work packages

Use H0/H1/N1 in the [detailed delivery contract](collaborative-planning-and-hosted-service.md#delivery-and-ownership). Reconcile existing authorities and the pilot charter, then complete service identity/subject access, typed intake/reconciliation, reused verified planning, atomic approval/publication/notification handoff, one change/repair/republication, and the full export/restore/accessibility/operating proof. These milestones are not dependencies of items1–8 or Branch K. The original service/hosted backlog coverage in items12/18 below is preserved by these explicit milestones plus later extensions.

### Later independent waves

9. Proven-structure algorithms.
10. Individually gated solver adapters.
11. WASM SDK, manager/catalog, conformance kit.
12. Authenticated service/collaboration design and delivery through H0/H1, followed by separately justified H2 breadth.
13. Enterprise adapters.
14. Other selected official domains; transportation is ordered separately in its sibling [Phase-14 plan](14-transportation-domain-pack.md).
15. Advanced AI/import integrations and organization administration beyond the early Branch-K increments, under their applicable deterministic/security/service gates.
16. Optional diagnostics.
17. Additional targets/distribution.
18. N1 managed collaboration operation and independently gated additional hosted-service profiles.
19. Individually gated portable-data, automatic/encrypted-backup, authenticity, richer-report, and hosted-sharing enhancements.

## Tests and acceptance

Every branch inherits Phase-12 definitions of done and adds backward compatibility/permanent migrations; typed unknown-newer rejection; independent verifier coverage; cancellation/crash/malformed/resource/failure tests; exact-artifact license/SBOM/notices/provenance; property/differential/metamorphic/fuzz/benchmark/E2E/accessibility/usability evidence; accurate capability/stability/privacy labels; docs/ADRs/ownership/recovery; exact portable/share preview and offline/local behavior when optional components fail.

Branch-K acceptance is staged by K6: workforce/seating planning and voice evidence precede only the increments that use them; school/transport evaluations follow their completed pack gates. Verify packaged native media and OS-store behavior on every advertised voice/runtime target, and external-host/runtime conformance for every advertised combination. No provider/model/voice support or numerical performance target is promoted from an investigation example without measured evidence.

## Risks and failure handling

| Failure | Required response |
|---|---|
| School semantics ambiguous | Obtain practitioner examples, specify semantics, add verifier fixtures before implementation. |
| School model explodes | Pre-filter, inspect formulation/model summaries, enforce budgets; do not freeze UI. |
| Unnecessary individual data | Prefer cohorts, minimize fields, document purpose/retention/export. |
| Portfolio overspends or compares unlike scores | Enforce one budget and authoritative vectors; disable unsafe combinations. |
| Backend cannot be verified/isolated | Stop branch and write ADR; do not ship/route. |
| License uncertain | Do not bundle; inspect exact components. |
| Native plug-in requested | Use constrained WASM or compiled official code; reject in-process untrusted loading. |
| Sandbox needs ambient authority | Redesign a narrow mediated capability with disclosure/tests; absent by default. |
| Service reuses Tauri IPC | Reject; design separate authenticated/authorized/audited API. |
| Hosted feature becomes local dependency | Restore local/offline parity or do not ship. |
| Cache/warm start changes meaning or hides budget | Hash every semantic input, retain cold-path parity, verify every candidate, account one explicit operation budget, and require measured equivalence/regression evidence. |
| Automated/encrypted/hosted portability traps or loses data | Preserve local current-format export, authenticated/versioned envelopes, visible backup health, restore drills, exact preview, revocation/deletion/export and documented exit. |
| AI/import needs arbitrary file/shell | Redesign typed scoped tools. |
| Experiment changes baseline or compares unlike objectives | Keep immutable isolated inputs, enforce reviewed promotion, record exact result/metric identities, and use common evaluation or explicit non-comparability. |
| Voice needs frontend secrets, ignores Stop, or leaks prohibited audio | Keep the mode disabled; prove native custody/transport, offline teardown, and distinct audio policy without waiving ADRs. |
| Runtime login succeeds but terms, entitlement, confinement, or packaging remain unproven | Keep the optional adapter disabled; do not reuse tokens, inherit ambient tools, or infer voice access. |
| Local MCP assumes cloud reachability or trusts host approval | Preserve scoped Eutheto authorization; require explicit separately reviewed remote deployment and applicable H/I/N gates. |
| Telemetry contains identifiers | Block collection/release and redesign. |
| Target lacks clean-machine evidence | Do not advertise it. |
| Correctness depends on undocumented behavior | Obtain official conformance evidence or remove capability. |

## Exit gates

The **original immediate-wave milestone** exits only when the complete school contract and nine scenarios pass; school Portable Scenario/Share Result migrations and offline reports meet Phase-12 contracts; explanation/portfolio features preserve verification, certainty, one-budget/resource and cache/hint provenance invariants; diversity is explicit and accurately scored; the Nuxt site is separate and accessible; formats migrate safely; and exact artifacts pass Phase-12 gates. These commitments are unchanged, but their completion is not an entry or exit prerequisite for Branch K.

**Branch K exits incrementally under K6**, beginning independently from completed Phase 12. Planning, dictation/realtime, optional delegation, local MCP, and conditional runtime support each meet their own listed gates and the common branch contract; no combined “all Phase 13 complete” gate holds early planning or voice behind school, transportation, or services. The original immediate wave likewise does not require Branch K.

**H1 and N1 have their own complete exits** in the [collaboration specification](collaborative-planning-and-hosted-service.md#delivery-and-ownership). They preserve all PRODUCT-01–15 and ACCEPT-01–22 dispositions and applicable additional adversarial cases; inherited phase ownership never counts as implementation or passing evidence. H1 requires the entire pilot lifecycle and minimum production operations. N1 adds the managed operator's release evidence. They do not widen the public MVP or waive the required Phase05 manual-testing pause.

Any independently delivered branch or increment exits only when its contract, tests, security/privacy/license review, migration/compatibility, accessibility/usability, packaging/operations, ownership/docs, and rollback/removal path are complete. One branch never makes another an implicit commitment, and a conditional integration cannot be marked supported until its evidence gates pass.

## Deferred and non-goals

- Marketplace, server, telemetry, hosted service, or proprietary solver is not prerequisite for school.
- MiniZinc text never becomes scenario/Planning-IR authority.
- Graph heuristics never become unverified final solutions.
- Cross-solver decomposition is not mislabeled portfolio execution.
- Third-party packs never use untrusted native in-process ABI.
- Server mode never directly exposes Tauri IPC.
- Vendor fields never pollute core domain contracts.
- Hosted/paid services never remove required local open-source functionality.
- Open local/self-hosted official capabilities have no commercial feature/population caps, but retain documented technical safety/resource bounds. Mode-specific infrastructure and support claims require an explicit deployment/capability matrix; source availability is not an uptime guarantee.
- Branch-K isolated experiments do not require or deliver universal branching/merge, a general agent framework, or coordinated cross-pack solving.
- Voice, MCP, and embedded runtimes never grant ambient approval, frontend credentials, export/privacy/import/backup authority, or an automatic session-budget compute permission; any policy exception requires a future explicit ADR before enablement.

## Assumption and version gates

- Direct evidence in [the assumptions ledger](assumptions.md) controls conflicts.
- School defaults, pattern/fairness weights, calendar/DST/projection semantics, sensitive/accessibility metadata, and legal wording require practitioner/usability review.
- OR-Tools 9.15 retains platform, flags, source/proto hashes, linkage, SBOM/license, benchmark, callback, and assumption-core gates.
- Pumpkin 0.5.0 remains gated on supported primitives, dedicated-thread ownership, cooperative cancellation/time limits, verifier, and benchmarks.
- HiGHS 1.15.1, SCIP 10.0.3, and MiniZinc 2.10.0 are discovery values, not automatic pins; reverify and inspect exact licenses at branch start.
- WASM ABI/signatures/resources/UI/catalog/migrations require an ADR.
- Provider APIs/local endpoints are mutable; reverify official contracts, OAuth, streaming, schemas, and keyring behavior per release.
- Branch-K provider/runtime terms, suitable third-party authorization, entitlement, effective tool confinement, media/platform capability, credential-store behavior, and exact-package licenses remain investigation gates. Numeric limits and latency targets are provisional until measured; text access never proves realtime/voice entitlement.
- Final CLI name must precede stable `optimizer serve` docs. Auth/token/binding/TLS/audit/concurrency remain service gates.
- Telemetry needs separately approved schema, consent/preview/deletion/retention and privacy review.
- Linux arm64, Windows arm64, rpm/repositories, universal macOS, and new endpoints require the full target gate.
