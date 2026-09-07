<!-- SPDX-License-Identifier: Apache-2.0 -->

# Phase 14 — Transportation Domain Pack

## Outcome

Deliver Transportation as a complete post-public-MVP vertical slice for planning the coordinated movement of people and household vehicles through time and place. The pack turns reviewed calendar commitments, resolved places, explicit traveler/vehicle policies, and immutable historical travel and scheduled-transit snapshots into bounded candidate journeys, solves the connected household model through the existing OR-Tools CP-SAT worker, independently verifies every accepted trajectory and authoritative score, and presents an understandable plan through the headless Rust core, application services, CLI, and accessible desktop.

Transportation is a **proposed post-MVP plan**, not current product behavior. In this phase, “Transportation MVP” means the first production-ready release of this pack; it does not mean Eutheto’s first public application release. The identifier `official.transportation` is proposed and reserved for this work, but remains unimplemented and unregistered until Phase 14 closes its registration, schema, migration, capability, compatibility, verification, packaging, and release gates.

## Source coverage

This phase incorporates the proposed **Eutheto Transportation Domain Pack — MVP and Post-MVP Product and Implementation Specification**, dated 2026-08-29, from `eutheto-transportation-domain-pack-mvp-post-mvp.md`, SHA-256 `9e269a173c217fcd6d08d4afaab9d82da3da43acda428b21c4c081cb353783e0`; the transportation-owned cache, critical-path, fallback, benchmark, and solver-UX requirements in [Performance and Solver UX Targets](performance-and-solver-ux-targets.md); and transportation portable/share specialization under [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md). It covers the blueprint’s product decisions; domain, calendar, Place Library, historical-travel, transit, candidate, optimization, validation, verification, explanation, desktop, output, privacy, CLI/application, integration, acceptance, delivery-wave, extension, risk, stop-condition, unresolved-decision, and source material.

Project-wide authority remains in [the roadmap index](README.md) and [the assumptions ledger](assumptions.md). This phase consumes the domain/Planning-IR contract from [Phase 02](02-domain-pack-and-planning-ir-contracts.md), the isolated OR-Tools worker from [Phase 03](03-ortools-worker-vertical-slice.md), the trust boundary from [Phase 04](04-independent-verifier-and-explanations.md), the command/persistence/desktop/result/repair patterns proven in Phases 01 and 05–09, the performance contract above, and the public-release baseline approved by [Phase 12](12-stabilization-and-public-release-gate.md).

Binding architecture decisions include [ADR-006](../adr/006-solver-neutral-planning-ir.md), [ADR-007](../adr/007-independent-solution-verification.md), [ADR-009](../adr/009-domain-pack-loading.md), [ADR-014](../adr/014-provider-authentication.md), [ADR-017](../adr/017-time-and-integer-units.md), and [ADR-018](../adr/018-public-scenario-representation.md).

## Roadmap position, dependencies, and entry conditions

Phase 14 is a sibling post-MVP branch entered directly from a completed Phase 12. It does **not** depend on completing the broad Phase 13 umbrella or any unrelated Phase-13 branch. If a Phase-13 capability is later consumed, that capability becomes an explicit, versioned Phase-14 dependency rather than an implicit prerequisite.

The early conversational experiment/voice deliveries in [Phase 13, Branch K](13-post-mvp-roadmap.md#branch-k--early-conversational-experiments-voice-and-optional-integrations) are likewise independent of Transportation. Phase-10 and initial Branch-K evaluations use workforce and seating; they must not require Transportation fixtures or production capabilities. Transportation remains usable without those assistant extensions.

The [Branch-H collaboration workflow](collaborative-planning-and-hosted-service.md#status-authority-and-scope) and its Workforce H1 pilot likewise do not gate Phase 14. Independent post-12 eligibility is not a promise of simultaneous staffing or priority. Collaboration links below govern only a future explicitly consumed integration; they do not change Phase 00–12 scope, the mandatory pause after Phase 05 before Phase 06, or this pack's T0–T7 and provider release gates.

Entry requires:

- the Phase-12 public release baseline, including versioned internal, Portable Scenario, immutable Result, privacy-filtered Share Result, bundle/report formats; stable IDs; permanent migrations; reversible revisioned commands; application services; local persistence; generated TypeScript contracts; desktop/CLI surfaces; one-file HTML/PDF report renderer; and packaged release evidence;
- compiled official-pack registration and compatibility checks, without a native plug-in ABI or ambient provider access;
- immutable solver-neutral Planning IR with deterministic ordering, checked integer arithmetic, provenance, lexicographic objectives, projection metadata, capability checks, and connected-component semantics;
- the isolated bundled OR-Tools CP-SAT worker and bounded protocol; Transportation adds no domain logic to that worker;
- projection, independent verification, authoritative scoring, quarantine, explanations, counterfactuals, accepted-solution persistence, locks, repair, and export gates proven by existing vertical slices;
- an approved design for any required shared path/state-continuity IR extension before Transportation relies on it;
- selected provider integrations, if any, passing the gates below. No calendar, geocoding, routing, traffic, directions, or transit provider is selected or shipped by this roadmap document.

## Non-goals for the first Transportation release

- Pulling Transportation into Phase 00, Phase 02’s current registry, the public MVP, or the Phase-12 release evidence.
- Creating placeholder domain directories, fake production adapters, registry entries, UI routes, provider catalogs, or claims of present support before the applicable wave exits.
- Live traffic monitoring, incident-driven replanning, real-time transit positions/disruptions, continuous push synchronization, or background operation while the desktop is closed.
- Calendar write-back, automatic calendar moves, automatic ticket/rideshare/parking/toll purchases, or any action in an external account without a structured preview and explicit approval.
- Turn-by-turn navigation, commercial fleet/delivery dispatch, arbitrary multi-household ride markets, unlimited-stop route synthesis, or splitting one connected household problem by day/person without proof of independence.
- Bike, scooter, rideshare, taxi, autonomous-vehicle, fuel, charging, parking, toll, monetary, or carbon optimization.
- Inferring driving eligibility, transit suitability, child supervision, disability/accessibility, school policy, legal compliance, or safety requirements from age, identity, calendar content, or other personal data.
- Guaranteeing provider accuracy, route availability, legal compliance, safety, accessibility-law compliance, transit operation, or on-time arrival.
- Requiring an Eutheto account, cloud service, AI, provider account, external solver installation, or network access after sufficient local snapshots/manual data exist.

## Binding decisions and invariants

1. **Joint trajectories, not car assignment.** Every person and vehicle has an explicit start state and a continuous time/place path. Driver, passenger, carpool, pickup, drop-off, stay, walk, transit, and bounded handoff choices synchronize those paths.
2. **Calendar-first, user-controlled meaning.** Calendar data proposes commitments. It does not silently make every event a mandatory physical trip. Users review classifications and local overrides survive synchronization.
3. **Places resolve before solve.** A mandatory physical commitment with an unresolved or invalid place blocks compilation with an actionable link.
4. **Historical planning remains distinct from operations.** Driving durations use typical/historical conditions for the planned weekday, local time bucket, and time zone. Current traffic at solve time never silently affects the plan. Live operational data is a later, separately labeled layer.
5. **Provider-neutral local snapshots.** Rust application/infrastructure adapters perform authorized network I/O and produce bounded, immutable, versioned provider-neutral snapshots. Provider-specific types, credentials, network clients, and cache policy do not enter the domain model, Planning IR, compiler, solver, verifier, or accepted plan.
6. **Network-free deterministic planning.** Once the scenario revision and required snapshots exist, domain validation, candidate generation where domain-owned, compilation, solving, projection, verification, scoring, explanation, repair, and export eligibility run without provider calls.
7. **Strict transit consent.** A person’s first-release transit policy is exactly `Never` or `Only if necessary`; it is never enabled implicitly. Walking, transfer, time, accessibility, and explicitly configured companion limits are hard constraints.
8. **Strict no-transit-first fallback.** The default orchestrator first solves and verifies the mandatory plan with public-transit candidates disabled. Any verified Stage-A solution is returned with zero transit. Stage B runs only under the diagnostic conditions defined below and enables transit only for opted-in people.
9. **Transit remains a mandatory-plan escape valve.** By default it is not used merely to preserve a low-priority optional commitment. In Stage B, authoritative transit use is minimized before ordinary convenience objectives.
10. **Bounded candidate generation.** The solver never invents arbitrary street routes, departure minutes, passenger groups, or stop sequences. Candidate counts, time buckets, group size, detours, waits, handoffs, stops, and per-transition alternatives have explicit budgets and diagnostics.
11. **Solver neutrality and isolation.** The pack compiles typed, immutable Planning IR and never constructs OR-Tools objects. The existing worker sees only backend mathematics and returns untrusted values/evidence.
12. **Independent acceptance.** Backend `FEASIBLE` or `OPTIMAL` is not acceptance. Projection, structural validation, independent evaluation of original Transportation semantics against the exact scenario/snapshot, and verifier-owned score recomputation must all pass.
13. **Explicit time semantics.** The scenario records an IANA time zone, horizon, local intent, resolved instants, and DST gap/overlap policy. Canonical solver durations are checked whole minutes; host time zone, locale, clock, and hash iteration order cannot alter compilation.
14. **Simple plans outrank superficially short plans.** Coordination burden, tight transfers, handoffs, pickups/drop-offs, waits, interruptions, and fragile detours are explicit score metrics, not narrative afterthoughts.
15. **Accepted outputs only.** Selected plans and solution-derived exports originate only from an independently verified solution for the exact scenario revision and snapshot. Stale solutions remain historical comparison inputs, not current accepted plans. These local states do not imply Branch-H organizational approval or publication.
16. **Honest claims.** “Verified” means internally consistent with configured facts and recorded snapshots. It is not a promise about real-world traffic, transit operation, accidents, weather, law, supervision, safety, or accessibility compliance.
17. **Registration is the last compatibility action.** `official.transportation` remains only proposed/reserved throughout T0–T6. T7 may explicitly register it only after pack descriptor, schemas/migrations, command catalog, UI manifest, Planning-IR compatibility, verifier, protocol, bundle, downgrade/recovery, packaging, and evidence gates pass.
18. **Provider work is outside the normal solve critical path.** Calendar sync, relevant-pair discovery, routing/transit refresh, geocoding, and selected-detail fetching are explicit cancellable jobs that atomically commit immutable facts before a solve captures its revision/snapshot. A warm solve never performs hidden per-candidate network calls.
19. **Both transit stages share one end-to-end budget.** Stage A, any Stage-B fallback, projection, verification, and result preparation consume remaining parent time. Starting Stage B never resets the full interactive limit, and a verified Stage-A result is not withheld for Stage B or optimality proof.
20. **Portable/share data obeys provider policy and user intent.** The pack supplies versioned current/historical Portable Scenario and Share Result conversion/migrations. Editable export, full backup, and immutable result sharing are separate actions; exact preview omits credentials/device state and any provider data whose terms disallow persistence or redistribution.

## Unresolved decisions and mandatory gates

No provider name, capability, account flow, default policy value, or redistribution right is closed by this plan. T0 records decisions; later waves may implement only approved selections.

| Gate | Decision and required evidence | Blocking outcome |
|---|---|---|
| `TRANS-GATE-001` provider selection | Capability matrix for each proposed calendar/geocoding/historical-routing/directions/transit adapter; maintained official API; region/platform coverage; bounded request/response behavior; error/reconnect behavior; conformance fixtures. | No adapter implementation or support claim. Manual/file paths continue. |
| `TRANS-GATE-002` authentication | ADR-014 compliance: optional/off by default; BYOK or explicit local endpoint unless an official suitable third-party OAuth flow is verified; least read-only scopes; safe redirects/revocation/refresh; OS credential-store custody; no token in Vue, SQLite, bundles, logs, or repository files. | No connected-provider path. Unofficial consumer sessions/OAuth are forbidden. |
| `TRANS-GATE-003` licensing and distribution | Exact API/data terms, attribution, caching, retention, route-geometry/directions storage, redistribution/export, user-key/project-key, and packaged-client permissions reviewed for every adapter and data class. | Do not cache, persist, bundle, export, or ship restricted data/adapter. |
| `TRANS-GATE-004` privacy and data minimization | Data inventory and retention; first-use destination disclosure; exclusion controls; disconnect/delete/detach behavior; normal-log and support-bundle redaction; no automatic AI transmission; precise-address/export warning; user deletion path. | No provider request or public release. |
| `TRANS-GATE-005` persistence/export | Decide which normalized profile fields, schedule facts, provenance, geometry, and instructions may persist; expiry/offline behavior; bundle inclusion/omission; redacted export; provider terms after cache expiry. | Snapshot cannot be durable or exported beyond permitted fields; fall back to manual refresh/entry. |
| `TRANS-GATE-006` temporal policy | Historical bucket size/interpolation/rounding, day classes, horizon, freshness, DST resolution, arrival/transfer/handoff buffers, and checked bounds validated with fixtures and usability review. | No canonical compilation or advertised default. |
| `TRANS-GATE-007` candidate/model policy | Maximum departure buckets, candidates per transition, group size, stops, detours, handoffs, waits, horizon, model estimates, warning/hard limits, and cancellation/resource budgets benchmarked. | Reject before unbounded generation/worker launch. |
| `TRANS-GATE-008` transit semantics | Stage-A proof/diagnostic rules, Stage-B activation, optional-commitment policy, authoritative lexicographic transit vector, “Try without transit” certainty wording, schedule/accessibility provenance, and no-transit mutation tests approved. | Transit remains unavailable. |
| `TRANS-GATE-009` pack/IR compatibility | Any path-continuity primitive is domain-neutral, typed, capability-checked, projected, independently verifiable, supported by the bundled worker, and covered by protocol/version tests; ADR if material. | Stop rather than put domain/backend knowledge across boundaries. |
| `TRANS-GATE-010` official registration | ADR-009 compiled-pack review, descriptor/schema/command/UI-manifest compatibility, migration/support window, bundle/CLI/desktop/package evidence, license/security review, and exact release manifest. | `official.transportation` stays proposed/reserved and absent from the registry. |

Defaults still requiring recorded decisions and representative-household usability evidence include arrival and transit-transfer buffers, walking limit, transfer limit, carpool detour threshold, handoff buffer, coordination weights, driver-fairness metric, planning horizon, time buckets, and supported household/model budgets.

## Provider-neutral snapshot and application-service boundary

The required flow is:

```text
read-only calendar/file/manual input
  → Rust integration service normalizes a bounded event batch
  → user review/classification and local commands
  → stable Place Library resolution
  → application service discovers relevant place pairs/time buckets
  → authorized Rust adapters produce immutable travel/transit snapshots
  → candidate generation and Planning-IR compilation without network access
  → existing isolated CP-SAT worker returns candidate values
  → Rust projects domain trajectories
  → independent Transportation verification and score recomputation
  → accepted local household plan
  → selected-route detail fetch only if requested, permitted, and separately recorded
```

Application/infrastructure responsibilities are account configuration, credential references, provider capability reporting, request limits/timeouts/cancellation, provider DTO normalization, transactional sync/import, provenance, cache/retention enforcement, snapshot refresh, relevant-pair discovery, and selected-route detail retrieval. Provider failure must not partially mutate authoritative scenario state.

The Transportation pack owns normalized facts, validation, deterministic candidate policy, Planning-IR compilation, solution projection, independent rule evaluation, authoritative score, explanations, views, imports/exports of pack-owned data, and migrations. The compiler and verifier consume the same immutable snapshot identity but independently interpret original domain facts. Vue owns only form, selection, route, map viewport, and temporary presentation state.

Every travel snapshot records schema/version, provider-neutral capability/basis, adapter/version provenance when applicable, generation time, scenario time zone, day class, bucket resolution, validity/freshness range, relevant-data hash, licensing/retention flags needed by application policy, and fallback/missing-data warnings. Detailed directions and route geometry are not required to compile; fetch them only for selected accepted journeys and only where terms permit.

## Domain model and trajectory semantics

The versioned Transportation scenario contains:

- planning horizon, IANA zone, DST policy, and Transportation settings;
- travelers with stable identity, explicit start state, end policy, driving permission, transit policy, accessibility needs, tags, and bounded external-source links;
- vehicles with stable identity, capacity including driver, start state, availability, authorized drivers, explicit accessibility attributes, and end policy;
- stable Places with display name, local address/coordinate when available, aliases, parent/child relationship, quality/resolution status, and separately stored provider references;
- Commitments with person, attendance (`Mandatory` or prioritized `Optional`), fixed or flexible timing, place, travel classification, arrival/departure policy, source link, local overrides, and review/status state;
- availability, driver permissions, explicit transportation/accessibility/companion rules, preferences, travel-data snapshot references, candidate policy, score policy, accepted base plan, and hard/soft locks.

A Commitment’s travel classification is physical presence, online/remote, no travel needed, ignored, or needs review. Fixed commitments carry zoned start/end values. Flexible commitments carry earliest start, latest end, checked whole-minute duration, and allowed granularity. Calendar timestamps alone do not decide attendance or travel semantics.

A Journey connects one origin and destination over a resolved interval and may contain a household-vehicle leg, scheduled-transit leg, configured walk leg, or a bounded combination. Household-vehicle legs bind one vehicle, exactly one authorized present driver, passengers, pickup/drop-off stops, travel-profile sample, buffer, and selected route reference. Transit alternatives bind one opted-in person to an immutable scheduled itinerary containing walking, boarding/riding, transfers, arrival, restrictions, and snapshot provenance.

Every solve starts from explicit person and vehicle places/times; Home is never inferred. Stay transitions preserve location. Selected person legs must form one non-overlapping continuous path through commitments and end policy. Selected vehicle legs must form one continuous non-overlapping path through availability and end policy. A handoff is valid only when the first authorized driver leaves the vehicle at the handoff place and a later authorized driver departs from that same place after the configured buffer.

Base-plan repair retains the prior accepted plan and identifies affected commitments, journeys, people, vehicles, snapshots, and locks. Hard locks remain constraints. Soft locks and unlocked base choices become named change metrics. Repair re-runs the no-transit stage first and independently verifies the repaired result.

## Calendar, Place Library, historical travel, and offline behavior

### Calendar behavior

- Connections are read-only for the first Transportation release. Connected sources, local calendar access, and account ecosystems remain provider-gated; this plan does not select any.
- ICS import, CSV commitment import, and manual commitments provide account-independent paths. Imports are bounded, mapped, previewed, validated, and applied as one undoable transaction with rejected-row reporting.
- A person may link multiple selected calendars. Sync records source identity, inclusion policy, stable occurrence identity, last success/error, and bounded cursor metadata outside domain meaning as appropriate.
- Normalization retains only fields needed for review: stable source occurrence, person, bounded title/description excerpt, start/end and source zone, recurrence/exception metadata, bounded location/conference facts, all-day/status/update facts. Raw provider payloads are not copied by default.
- Review states are ready physical, needs location, online, no travel needed, all-day review, ignored, and ambiguous. Deterministic suggestions remain reviewable.
- Local overrides may change classification, place, attendance, flexibility, buffer, ignore state, notes, or source attachment. They survive incremental sync. A provider change conflicting with an override creates review state rather than overwriting it.
- Recurrence expansion is horizon-bounded, occurrence-stable, exception/cancellation-aware, duplicate-free, and based on event/calendar zone. Changed/deleted source events mark accepted plans stale and retain them for explicit comparison/repair; no automatic first-release reoptimization.

### Place Library

Resolution tries an exact confirmed alias, then bounded structured address/coordinate data, then an explicitly invoked gated geocoder, then user confirmation/manual entry. Quality is visible: user-confirmed, exact alias, provider suggestion awaiting confirmation, approximate, unresolved, invalid/outside supported region, or duplicate candidate. Stable Eutheto place IDs remain separate from provider IDs. Users can add aliases, parent/child places, merge duplicates, enter an address/coordinate manually, and see which external destination receives a query.

### Historical travel and selected directions

Travel duration is selected by origin, destination, mode, planned day class, local departure/arrival bucket, and time zone. Historical/typical, manual, no-traffic fallback, predictive, and live bases are distinct; only an approved historical/typical or explicitly accepted visible fallback satisfies first-release planning. Arrival buffer is a separate configured duration, never presented as a probability guarantee. Arrive-by support uses a provider capability or a deterministic bounded search over departure buckets.

The application requests only plausible place-pair/time-bucket profiles, stores a bounded provider-neutral snapshot under adapter-enforced terms, compiles with durations rather than full instructions, and separately requests details only for selected accepted journeys. An adapter without genuine historical capability must report that fact and cannot masquerade as compliant.

### Scheduled transit and manual/offline behavior

Transit snapshots use service scheduled for the planned date/time, walking to/from stops, transfer durations and buffers, service calendars, and supplied route/accessibility attributes. Live positions and disruptions are excluded. Manual peak/off-peak travel matrices, manual places/commitments/vehicles, and file imports must support deterministic tests, private/rural locations, households without provider accounts, and offline solve/verify/export after permitted snapshots exist. Manual/no-traffic data is visibly labeled in validation, results, explanations, and exports.

## Required rules

Every rule below must have stable schema/migration, reversible command, fast/full validation, plain-language editor and CLI representation, deterministic candidate/IR semantics, bundled-worker translation/capability tests, independent evaluation, provenance/explanation, feasible/boundary/infeasible fixtures, import/export round trip, and model-size review.

1. Every mandatory commitment is attended for its complete required interval at its resolved place.
2. Every person follows a continuous time/place path; no teleportation, incompatible overlap, or simultaneous drive/ride/transit/attendance.
3. Every vehicle follows a continuous time/place path; no teleportation, simultaneous use, or movement outside availability.
4. A moving household vehicle has exactly one present authorized driver satisfying explicit attributes.
5. Passengers synchronize with vehicle/driver pickup and drop-off place/time.
6. Driver plus passengers never exceeds vehicle capacity.
7. Selected travel time, departure, arrival, waiting, arrival buffer, transfer buffer, and handoff buffer agree with the immutable snapshot and checked time semantics.
8. Person, driver, and vehicle availability is respected.
9. Transit is selectable only in Stage B and only for a person explicitly set to `Only if necessary`.
10. Transit/walking choices satisfy explicit maximum walking, transfer, time, accessibility, and companion restrictions.
11. Vehicle and person end policies are satisfied.
12. Hard-locked journeys, vehicles, drivers, pickups/drop-offs, transit choices, departure windows, and vehicle locations remain unchanged.
13. Explicit accessibility, supervision, companion, and vehicle attributes are respected without inference.
14. Flexible commitments use an allowed start and complete duration; optional commitment omission follows explicit priority semantics.
15. The exact scenario revision, snapshot hash/version, horizon, zone, and DST resolution used to compile match those used to project and verify.

## Preferences and authoritative score hierarchy

Named measurable preferences include preserving the accepted/base plan; retaining optional commitments by priority; minimizing public-transit users/journeys/minutes/transfers/walking when Stage B is active; minimizing vehicle handoffs, pickups/drop-offs, driver changes, multi-stop detours, tight transfers, excessive waits, unusually early departures, commitment interruptions, total driving, empty/deadhead travel, carpool detour, and person waiting; maximizing arrival buffers only to a useful cap; keeping usual drivers/vehicles; balancing driving and inconvenience; and honoring preferred vehicles and flexible timing.

Coordination burden counts at least each additional pickup, drop-off, handoff, driver change in a movement chain, transfer below a comfortable margin, wait above threshold, unusually early departure, multi-stop detour, and required interruption of another person’s availability/commitment window.

Authoritative lexicographic order is:

1. zero required-rule violations and all mandatory commitments;
2. hard locks and explicitly required accessibility/supervision conditions;
3. in Stage B only: fewest transit users, then journeys, transit minutes, transfers, walking beyond comfort, and less-preferred timing;
4. minimum change from an accepted/base plan;
5. preservation of higher-priority optional commitments;
6. minimum coordination burden and tight transfers;
7. minimum household driving, empty/deadhead travel, and bounded carpool detour;
8. minimum waiting and excessively early arrival;
9. maximum useful arrival buffers;
10. balanced driving/inconvenience among eligible people;
11. ordinary vehicle/timing preferences;
12. deterministic stable-ID tie-breakers.

Required behavior is never converted to a penalty. Score bounds and checked aggregation must prove that lower levels cannot dominate higher ones.

## Strict two-stage transit fallback

### Stage A — household transportation only

Disable every public-transit candidate. Solve mandatory commitments using household vehicles, direct driving/passenger legs, carpools, bounded pickups/drop-offs and handoffs, stays, and only explicitly configured allowed walking transitions. Project and independently verify the candidate. If a verified feasible plan exists, accept it with zero transit even when transit would reduce driving or improve lower preferences.

### Stage B — opted-in transit fallback

Run only when Stage A is proven infeasible, or when no verified solution is found under a separately configured and disclosed diagnostic policy. A timeout/limit without proof must be worded “no verified solution within the diagnostic limit,” never “infeasible.” Enable scheduled-transit candidates only for opted-in people, preserve all mandatory commitments, minimize the authoritative transit vector before ordinary preferences, project, and independently verify.

A plan using transit prominently identifies each transit journey, walking/transfers/buffers/restrictions, snapshot basis, and why it was necessary. “Try without transit” reuses or runs the bounded Stage-A diagnostic and reports proven infeasibility with a sufficient conflict, limit-based uncertainty, or verified smallest configured changes. It never calls a time-limited search failure proof.

Stage records consumed and remaining parent budget, candidate/model counts, snapshot/cache state, first incumbent, first independently verified feasible, proof/termination, authoritative score, and exact activation reason. Stage B cannot run after the parent budget is exhausted. Counterfactual `Try without transit` is a separate explicitly started bounded diagnostic, not hidden continuation of the original solve.

## Candidate generation, Planning IR, and CP-SAT

Candidate inputs are explicit start states, commitments, resolved places, driver permissions, capacities/availability, historical travel profiles, scheduled transit for opted-in people, buffers, locks/base plan, and bounded detour/pickup/handoff/wait policies.

First-release categories are direct person-as-driver; direct passenger with an existing driver; shared-origin carpool; one bounded pickup or drop-off detour; capacity-bounded multi-passenger household trip; vehicle handoff at a common place; opted-in scheduled transit itinerary; zero-distance stay; and explicitly configured bounded walk-only transition. Unlimited stop sequencing and open-ended route synthesis are excluded.

Static pruning rejects insufficient time, unauthorized driver, unavailable vehicle/person, capacity excess, incompatible start/end place, transit opt-out, excessive walking/transfers/duration, excess detour, missed buffer, overlap, accessibility mismatch, and horizon/zone mismatch. Rejection provenance supports explanations and counterfactuals.

Generation enforces departure-bucket, group-size, stop-count, detour, per-transition candidate, duplicate normalization, dominance, total variable/constraint, memory, and time budgets. The application exposes a deterministic model estimate and rejects an over-limit scenario before worker launch.

The connected household model is a time-expanded multi-resource path-selection problem. Compilation creates only statically plausible typed Boolean/integer/interval structures, uses stable order and checked whole-minute arithmetic, preserves entity/rule/snapshot/projection provenance, and capability-checks the bundled worker before launch. The existing worker receives only validated Planning IR/backend protocol data. It does not know calendars, addresses, household semantics, or provider identities. Decomposition is permitted only when person, vehicle, commitment, objective, lock, and end-state dependencies prove true independence and merge equivalence.

## Projection, independent verification, and explanations

The only acceptance path is:

```text
bounded worker values
  → project normalized people/vehicle journeys and commitments
  → structural solution validation
  → independently evaluate every original Transportation required rule
  → independently recompute the complete score vector and metrics
  → accept or quarantine
```

The verifier reads the exact normalized scenario revision, local snapshots, and projected solution. It does not trust backend status/objective, compiler constraint records, candidate rejection records, or penalty variables as rule/score evidence. It independently checks commitment attendance; person and vehicle continuity; driver presence/eligibility; passenger synchronization; capacity; non-overlap; snapshot-selected duration/departure and buffers; availability; transit stage/opt-in/restrictions; end policies; hard locks; base-plan changes; and all preference metrics.

Verification reports mandatory/optional commitments, transit users/journeys/minutes/transfers, driving/deadhead minutes, pickup/drop-off and handoff counts, buffers, waiting, workload distribution, base-plan changes, snapshot identity, and stale/fallback warnings. Mutation/differential fixtures must show that a compiler, projection, score, stage, or worker defect is rejected and quarantined without mutating the scenario.

Explanations name verified eligibility, availability, shared journey facts, historical time bucket/buffer, later vehicle consequence, and score tradeoffs. Bounded counterfactuals may force a driver/vehicle, prohibit transit, or test a small explicit scenario change against the same revision/snapshot; they report verified feasible, proven infeasible with sufficient/minimal qualification, worse by named authoritative metrics, or undetermined within limit. A failed day must provide actionable conflict evidence rather than only “No solution.”

## Application, CLI, desktop, accessibility, imports, and exports

### Commands and application services

Every Transportation mutation is a namespaced typed command with schema/version, expected revision, request identity, permission/risk metadata, reversibility, human summary, invalidated views, and generated TypeScript contract. Commands cover people, calendars/source links, commitments/overrides, places/aliases, vehicles/drivers, transit and advanced limits, travel assumptions, rules/preferences, locks/manual edits, imports, and repair. AI, if later enabled, may only propose these same reviewed commands.

When optional Transportation assistant exposure is introduced after the corresponding production pack/capability gates, extend the shared evaluation corpus rather than creating a transport-specific agent. Include ambiguous places/dates, stale snapshot evidence, hypothetical versus applied commitments, and attempted inference of travel times, driving eligibility, supervision, or transit consent. The assistant cannot invent historical traffic, refresh a provider implicitly, or enable transit for a non-opted-in person; all policy changes require the same typed preview and explicit approval. Text/voice privacy also covers precise addresses, calendar notes, and spoken personal information.

Future participant-submitted transit willingness is collaboration input, not an authoritative traveler policy or solver fact. Only after the relevant Transportation capability and [collaboration identity/input-policy gates](collaborative-planning-and-hosted-service.md#identity-input-authority-and-policy) pass may an authorized coordinator map an explicitly linked participant to a pack-owned traveler through a typed, revision-bound proposal, review its exact meaning, and apply it through the normal command path. Preserve the pack's explicit `Never` / `Only if necessary` semantics, strict no-transit-first fallback, and configured hard limits; inferred willingness or a submission alone cannot enable transit. This integration creates neither universal shared-person/resource identity nor cross-pack solver composition, and it is not part of H1's narrow Workforce pilot. Human collaboration approval cannot waive independent Transportation verification.

Shared application services own provider setup/secrets, transactional sync/import, snapshot refresh/status, model estimation, staged solve jobs, cancellation, projection/verification acceptance, explain/counterfactual, compare, lock/repair, and export orchestration. Components never call Tauri directly; typed frontend services/composables do. No provider secret storage becomes pack-specific.

### CLI

The final executable name follows the project-wide naming gate. The CLI must provide deterministic non-AI access to scenario creation/edit, source/calendar listing and explicit sync, commitment review, place/travel status and refresh, validation, model estimate, staged solve, verify, explain, compare, try-without-transit, repair, and export. Human and stable JSON output disclose revision, snapshot/provenance, stage, verification, proof/limit status, warnings, and exit code. File/manual workflows remain complete without a connected provider.

### Desktop

The guided flow is People and calendars → Review commitments → Places → Vehicles and drivers → Transportation options → Travel assumptions → Validate → Optimize → Review and repair. Users can revisit steps freely. Validation groups must-fix, travel-data, calendar-review, likely-conflict, suggested-review, and informational findings and links each to the exact entity or setting.

Before solve, show horizon/zone, source freshness/conflicts, historical/manual basis, snapshot freshness, transit-enabled people, base/repair state, model estimate, resource mode, and advanced backend detail. Progress reports only stages actually executed; it never claims Stage-A infeasibility, Stage-B use, optimality, or verification without corresponding evidence. Operations completing below approximately 300–500 ms avoid progress flicker; longer runs expose truthful coarse preparation/transport/model/optimize/verify/result phases with cancellation and bounded announcements.

Synchronized accepted-result views are household timeline, person itinerary, vehicle itinerary, coordination checklist, map/selected-route summaries, exceptions/warnings, authoritative score/evidence, explanation, and change comparison. Manual driver/passenger/vehicle/flexible-time/transit acceptability changes are previewed revisioned commands; invalid changes explain conflict, valid changes are undoable and require repair/verification before acceptance. The first verified plan renders without waiting for optimality proof, selected-route detail, optional explanation enrichment, or AI.

### Accessibility and performance

Every essential setup, validation, solve, timeline, itinerary, checklist, map fact, manual edit, lock, repair, explanation, comparison, and export is keyboard reachable with visible focus and predictable restoration. Maps/timelines have complete synchronized list/table alternatives and no critical fact exists only in color, geometry, hover, or animation. Transit/tight-buffer/stale/manual warnings use text and icons; status changes and solve completion are announced. Screen-reader scripts, reduced motion, zoom/high-DPI, responsive large-list virtualization, cancellation, and bounded payload/rendering tests are release evidence.

Record calendar/snapshot enrichment separately from validation, relevant-pair discovery, cache lookup, candidate generation/pruning, Planning-IR build, worker startup/backend solve, first incumbent, first verified feasible, verification, result preparation, initial UI render, and total. Also record pair/time-bucket cache hits/misses, candidate/model counts, Stage A/B/fallback, proof/termination, authoritative score, memory, and provider/model/freshness versions without scenario PII.

The initial hypotheses are <500 ms end to end for a small warm manual/snapshot fixture; target <1 second warm and usually <3 seconds cold for the normal household fixture; p95 <5 seconds for the approved normal corpus; and a bounded responsive experience, usually <5 seconds and expected <10 seconds, for moderate/large cases. T7 calibrates this pack's exact reference machine, cache states, corpus, percentiles, budgets, and regression tolerances before any claim. Cold network enrichment is separately disclosed and never disguised as solver time.

### Imports

Account-independent imports include ICS, CSV commitments, CSV places, manual commitments/places/time-dependent travel profiles, and manual vehicles. Connected calendar, geocoding, historical-profile, transit, and directions paths exist only for adapters that pass all gates. Every import performs safe type detection, byte/row/field/nesting limits, explicit mapping, additions/updates/duplicates/rejections preview, stable-ID/source matching, validation, one atomic undoable apply, and a rejected-row report. Imports never execute macros, templates, URLs, paths, or embedded instructions.

### Outputs, portability, and result sharing

Outputs are:

- editable proposed/final Eutheto portable scenario and full backup through the shared Phase-01 services;
- normalized Transportation solution JSON;
- household timeline, per-person and per-vehicle itineraries, pickup/drop-off/handoff checklist, and scheduled-transit itinerary;
- provider-permitted selected driving detail;
- CSV summaries;
- generated ICS transportation blocks without calendar write-back;
- one self-contained interactive HTML result report;
- direct PDF from the same validated Share Result Model; and
- bounded diagnostic report.

Only accepted, independently verified results may drive plan JSON/CSV/ICS/HTML/PDF. A stale accepted result may export only with explicit immutable old-revision/snapshot labeling. Editable scenario/full backup has its own valid portable-input gate and remains available without a result. Exact preview discloses revision/verification/snapshot assumptions, transit/fallback/manual basis, names or aliases, precise/generalized places, selected-route detail, source titles/notes, external links, and every provider-policy omission. Sensitive/source-only fields and precise addresses default absent; credentials, sync tokens, provider secrets, unrelated raw calendar data, Planning IR, worker frames, forbidden geometry/directions/cache data, AI conversations, and device state never enter portable/share output.

The Transportation pack converts stable people/vehicle/place/commitment/rule/policy IDs, canonical time/unit meaning, allowed normalized snapshots/references and accepted-result provenance to current Portable Scenario/Share Result schemas and migrates every supported historical version sequentially. Provider terms decide whether data is included, summarized, represented by a reconnectable reference, or omitted with a visible warning; unknown required semantics fail before mutation.

Privacy review also covers derived travel evidence, comparisons/counterfactuals, and small-group metrics that can expose private locations, routines, accessibility needs, or another participant's constraints without a raw address or note. Exact recipient disclosure uses the existing Share Result authority. Future collaboration retention distinguishes raw willingness submissions, minimal accepted decision lineage, publication snapshots/access metadata, and transport/provider-derived data under [privacy, retention, and recovery](collaborative-planning-and-hosted-service.md#privacy-retention-and-recovery); no blanket retention period, provider redistribution permission, or automatic export of participant intake is implied.

HTML/PDF reuse the Phase-07/09 generic accepted-result builder, exact privacy preview, safe inert-data embedding, restrictive standalone-report CSP, accessible components/list-table equivalents, offline `file://` shell, print styles, and controlled local PDF renderer. Transportation contributes immutable household/person/vehicle itinerary, coordination/checklist, transit, assumption/warning and optional provider-permitted route-detail payloads. The report supports local view switching, filtering/search/highlight/detail expansion and print, but no edit, solve, refresh, provider call, server/account/storage or required network request. Maps are optional schematics; complete place/leg meaning remains in accessible text/tables and never depends on remote tiles/fonts/icons.

## Dependency waves

The waves are dependency-driven, not calendar estimates. A later wave may begin only when every dependency named for it has exited with reviewed artifacts.

### T0 — Contracts, gates, and bounded snapshots

**Work:** `TRANS-001`, `TRANS-002`
**Depends on:** Phase 12 only.

Deliver the approved Transportation decision record set; provider/auth/license/privacy/persistence/export capability gates; provider-neutral calendar event, Place, historical travel, scheduled-transit, provenance, capability, credential-reference, and immutable snapshot contracts; relevant-pair/time-bucket discovery boundary; path-continuity Planning-IR gap analysis; exact worker/protocol compatibility plan; deterministic serialization/hash and resource limits.

**Exit:** no provider-specific type can enter the pack/IR; no secret can enter documents; adapters can be tested through conformance fixtures without claiming production support; snapshots are bounded, immutable, deterministic, versioned, hashed, and network-free consumers can compile/verify them; every provider remains unselected until its gate passes; any IR extension has an approved ADR and complete backend/verifier plan.

### T1 — Scenario, commands, migrations, and persistence

**Work:** `TRANS-003`
**Depends on:** T0.

Deliver the complete normalized schema for people, vehicles, places, commitments, availability, driver permissions, explicit accessibility/companion rules, transit policy, settings, score/candidate policies, allowed snapshot references, base plan, and locks; current Portable Scenario and Share Result schemas; sequential migrations/unknown-extension preservation; reversible commands/batches; validation; generated DTOs; CLI/application editing path; canonical internal/portable round trips.

**Exit:** create/edit/reopen/migrate/undo/redo plus current/historical Portable Scenario/Share Result round trips preserve canonical meaning; unknown required semantics reject and nonsemantic extensions preserve; command plus inverse restores state; DST/unit/reference/overflow validation is deterministic; no provider or solver is required to edit facts; credentials/backend/device/restricted-provider artifacts are absent; `official.transportation` remains unregistered.

### T2 — Calendar-first setup and Place Library

**Work:** `TRANS-004`
**Depends on:** T1 and applicable T0 gates.

Deliver manual, ICS, and bounded CSV ingestion; provider-neutral read-only connected-source support only for any adapter that has passed selection/auth/license/privacy gates; per-person source mapping; transactional initial/incremental sync; recurrence/exceptions/cancellations/time zones; event classification/review; durable local overrides and conflict handling; stale-plan impact; Place aliases, parent/child locations, duplicates, quality, manual resolution, and gated geocoding; guided desktop and CLI flows.

**Exit:** a representative household week imports without duplicates; online/ignored/no-travel events create no movement; unresolved mandatory physical places block solve actionably; overrides survive resync; changes/deletions mark accepted plans stale without automatic repair; provider failure is atomic; provider-free manual/file flow is complete.

### T3 — Historical and scheduled travel data

**Work:** `TRANS-005`
**Depends on:** T2 place/time semantics and applicable T0 gates.

Deliver relevant transition/time-bucket discovery; typical historical travel profiles; manual peak/off-peak fallback; scheduled-transit itinerary snapshots for opted-in people where a gated source exists; capability/freshness/provenance/status UI; deterministic bucket/interpolation and arrive-by behavior; bounded cache/retention enforcement; offline snapshot loading; selected-route detail fetching separated from compile data. Reusable historical matrix entries bind stable origin, destination, weekday/day class, departure-time bucket, mode, provider/model version, freshness/confidence, units, and provenance; deterministic selection rejects ambiguous or stale-incompatible entries.

**Exit:** rush-hour selection uses planned day/time rather than solve-time conditions; DST/day-class boundaries pass; unsupported capabilities cannot masquerade as historical traffic; compiler/verifier run offline from exact snapshots; provider terms govern persistence/export; detailed directions are fetched only for selected accepted journeys; manual-only planning works.

### T4 — Household-vehicle solver vertical slice

**Work:** `TRANS-006`, `TRANS-007`, `TRANS-008`
**Depends on:** T1–T3 and `TRANS-GATE-009`.

Deliver bounded candidate generation for stays, direct driver/passenger, carpool, pickup/drop-off, multi-passenger, walk, and handoff choices; pruning/model estimate; person/vehicle path compilation and score hierarchy; existing CP-SAT worker translation; normalized trajectory projection; independent Transportation verifier/scorer; explanations/counterfactual/conflict evidence; CLI solve/verify/JSON; base-plan locks and repair foundation.

**Exit:** ordinary weekday, rush-hour, carpool, pickup/drop-off, handoff, DST, manual-fallback, and representative infeasibility fixtures pass; invalid/mutated candidates are quarantined; score hierarchy is verifier-owned; one connected model reaches the existing worker without domain/provider knowledge; solve/verify requires no UI/network.

### T5 — Strict public-transit fallback

**Work:** `TRANS-009`
**Depends on:** T4 and approved `TRANS-GATE-008`; gated transit snapshots from T3 where used.

Deliver explicit per-person policy and restrictions; scheduled transit candidate integration; Stage-A/Stage-B orchestration; authoritative transit score vector; optional-commitment policy; accessibility/walking/transfer checks; prominent results; transit explanation and try-without-transit diagnostic; repair-stage parity.

**Exit:** a verified no-transit plan always has zero transit; opted-out people never receive transit; required fallback uses the minimum authoritative transit vector among verified candidates; diagnostic certainty is accurate; all transit restrictions are independently verified and disclosed.

### T6 — Complete application, desktop, repair, and export experience

**Work:** `TRANS-010`, `TRANS-011`, `TRANS-012`
**Depends on:** T4 and T5.

Deliver setup/status/application queries; complete CLI parity; household/person/vehicle/checklist/map/exception views; explanation/comparison; locks/manual edits; calendar/snapshot change impact and minimal-change repair; selected-route details; accepted-solution JSON/CSV/ICS plus one-file HTML/direct PDF through the shared Share Result renderer; editable portable scenario/full backup integration; exact privacy/provider-omission preview; diagnostics; provider disconnect/delete/detach; keyboard/screen-reader/list alternatives; large-view/report and cancellation behavior.

**Exit:** a first-time household can use a provider-free path to import/enter, review, resolve, configure, validate, solve, understand, lock, repair, compare, export editable data/back up, and share an immutable privacy-reviewed result offline; a gated connected path passes where claimed; every result export derives from accepted data and every portable/share field obeys terms; HTML opens from `file://` with zero required network and PDF/print matches the same payload; all essential flows have keyboard and non-map alternatives; large benchmark UI/report stays responsive.

### T7 — Stabilization, explicit registration, and release

**Work:** `TRANS-013`
**Depends on:** T0–T6 and all applicable gates.

Deliver complete acceptance corpus; Pack A–D warm/cold/manual/transit benchmark manifests and raw phase/model/cache/quality evidence; calibrated budgets/regression policy; compiler/verifier mutation and differential tests; provider failure/reconnect/conformance evidence for every claimed adapter; permanent database/Portable Scenario/Share Result migration and recovery/downgrade fixtures; malicious bundle/report security, exact privacy-preview, offline HTML/PDF browser/accessibility and no-forbidden-provider-data evidence; supported-platform packaged worker and calendar/manual-path smoke evidence; user/developer/provider/limitations documentation; SBOM/notices; release manifest; and explicit official-pack registration.

**Exit:** every Phase-14 gate below passes. Only then register and ship `official.transportation`; until that release commit, the identifier remains proposed/reserved.

## Ordered TRANS work packages

1. **TRANS-001 — Transportation decisions and compatibility:** bind trajectory, historical-versus-operational, transit-stage, time/unit, connected-model, IR capability, registration, and Definition-of-Done decisions; close required ADRs.
2. **TRANS-002 — Integration and snapshot boundary:** provider capability/auth/license/privacy gates; calendar/place/routing/transit normalized contracts; credential references; bounded immutable snapshots, provenance/hash/freshness/cache/export policy, manual fakes/conformance harness.
3. **TRANS-003 — Domain state, commands, and portability:** complete schema, migrations, validation, reversible revisioned commands, base plan/locks, persistence, current/historical Portable Scenario/Share Result conversions, unknown-semantic/extension behavior, generated DTOs, CLI/application editing.
4. **TRANS-004 — Calendar and Place Library:** manual/ICS/CSV paths, gated read-only adapter paths, normalization/review, recurrence/exceptions, sync/override/conflict/staleness, aliases/resolution/duplicates/privacy.
5. **TRANS-005 — Travel data:** historical planned-time profiles, scheduled transit snapshots, manual matrices, relevant-pair buckets, deterministic time selection, provider/model/freshness/confidence provenance, offline/cache policy, bounded enrichment metrics, selected-detail boundary.
6. **TRANS-006 — Candidate trajectories:** all bounded first-release journey categories, static pruning/rejection evidence, pre/post counts, model estimates and hard budgets.
7. **TRANS-007 — Planning IR and CP-SAT:** continuous person/vehicle paths, required rules, lexicographic score plan, locks/base plan, capability checking, existing worker translation/protocol evidence, one remaining parent budget.
8. **TRANS-008 — Projection, verification, and evidence:** normalized trajectory projection, independent required-rule evaluator/scorer, quarantine, first-verified-feasible timing, explanations, sufficient conflicts, counterfactuals, compare, repair foundation.
9. **TRANS-009 — Transit fallback:** per-person consent/restrictions, strict two-stage one-budget orchestration, authoritative transit vector, diagnostics, result disclosure, independent transit verification.
10. **TRANS-010 — Repair and explanations:** calendar/availability/snapshot impact, hard/soft locks, minimal-change repair, stage parity, deterministic change and infeasibility explanations.
11. **TRANS-011 — CLI/application/desktop/accessibility:** complete headless and desktop workflows, typed APIs/views/events, timelines/itineraries/checklist/map alternatives, 300–500 ms truthful progress, keyboard/screen-reader/performance evidence.
12. **TRANS-012 — Imports, exports, reports, and privacy:** transactional inputs; distinct editable/full-backup/result/diagnostic gates; accepted-only Share Result builder; exact privacy/provider-omission preview; portable JSON/CSV/ICS; one-file offline HTML and direct PDF/print; safe inert rendering; accessible non-map views; provider-permitted route details; redaction/disclosure/deletion/support-bundle behavior.
13. **TRANS-013 — Acceptance and release:** all fixtures/Pack A–D benchmarks, calibrated performance policy, conformance/security/license/accessibility/usability/migration/package/offline/docs evidence, support declarations, exact manifest, and final explicit registry addition.

## Required acceptance scenarios

All 15 scenarios are checked-in non-identifying fixtures. Each asserts validation, expected normalized status, Planning-IR/model limits, projection, independent verification, score and transit invariants, stable evidence keys, snapshot/schema versions, current/historical Portable Scenario/Share Result semantic preservation, unknown-semantic rejection, exact report privacy/provider omissions, and import/export preservation. Do not assert one exact plan where equivalent verified plans exist unless deterministic settings intentionally require it.

1. **Two cars, five people, ordinary weekday:** work, school, and social commitments; two vehicles; three authorized drivers; feasible without transit; assert zero transit.
2. **Rush-hour historical traffic:** same origin/destination at 04:00 and 07:30; the historical profile makes 07:30 materially longer; assert departure and feasibility use the planned bucket, not solve-time conditions.
3. **Required carpool:** separate vehicle use is impossible; one shared ride satisfies both commitments; assert synchronized carpool and explanation.
4. **Pickup and drop-off:** one driver makes a bounded detour for another person; assert capacity, person/vehicle continuity, pickup/drop-off synchronization, and detour limit.
5. **Vehicle handoff:** one driver leaves a vehicle at a shared place and another authorized driver uses it later; assert continuous vehicle path, authorization, and handoff buffer.
6. **Public transit fallback required:** no household-vehicle-only solution; one person opted in; transit makes all mandatory commitments feasible; assert exactly one opted-in transit journey and correct explanation.
7. **Public transit available but unnecessary:** transit would reduce driving but Stage A has a verified plan; assert zero transit under the strong last-resort policy.
8. **Transit opt-out:** a transit itinerary exists for an opted-out person; assert it is never selected by compiler, worker projection, repair, or verifier.
9. **Transit restrictions:** the fastest itinerary exceeds walking/transfers while a compliant slower one exists; assert the compliant itinerary or verified infeasibility, never the prohibited route.
10. **Calendar normalization:** physical address event, online meeting, all-day birthday, unresolved location, and recurring event with one exception; assert correct review states, stable occurrence identity, and no duplicate commitments.
11. **Calendar change and repair:** begin with an accepted plan, move one appointment, and assert stale detection, affected-journey analysis, no-transit-first minimal repair, preserved locks, and change explanation.
12. **Infeasible even with transit:** insufficient time or no available compliant route; assert no accepted solution, correct proof/limit wording, and a sufficient actionable conflict explanation.
13. **DST transition:** commitments/travel cross a daylight-saving ambiguity or gap; assert explicit IANA-zone/DST resolution and correct checked elapsed minutes.
14. **Manual travel fallback:** no provider account, distinct peak/off-peak manual values; assert visible manual-data warning, offline compile/solve/verify, and verified result.
15. **Large household benchmark:** configurable people, vehicles, commitments, places, and candidate journeys; assert bounded generation, resource/cancellation behavior, model-size diagnostics, and responsive list/map-alternative UI.

The ordinary weekday/manual fixture is Pack A; rush-hour and normal multi-person/carpool cases form Pack B; coordinated pickup/handoff/transit cases form Pack C; the configurable large household and adverse fallback/resource cases form Pack D. Each manifest declares manual/warm-snapshot/cold-enrichment state, relevant pair/time buckets, allowed network work, total and backend budgets, fixed seed/thread policy, expected status/invariants, and the complete timing/model/cache/quality fields above.

Mutation coverage removes or corrupts at least person continuity, vehicle continuity, driver presence, passenger synchronization, capacity, buffer, transit stage, opt-in, lock, end policy, snapshot bucket/hash, and score terms; every such candidate must fail independent acceptance.

## Post-first-release extensions

These are separate post-Phase-14 branches, not release prerequisites:

- operational live traffic that compares current conditions with the preserved historical basis and offers explicit adjusted departure/replanning;
- real-time transit positions, cancellations, disruptions, missed-connection risk, and alternate itineraries, visibly distinct from provider-published transit timetables and the user's selected accepted plan;
- previewed explicit calendar write-back and stable removal/update metadata;
- optional continuous sync/impact alerts/draft repair while preserving local desktop independence;
- expanded transit states (`Okay when helpful`, `Prefer when practical`) and mode/fare/reliability/stop preferences;
- walking, cycling, rideshare/taxi, micromobility, park-and-ride, mixed vehicle/transit, bike/transit, and known outside rides, all opt-in;
- percentile/reliability profiles, robust uncertainty planning, and honest confidence wording;
- dated weather snapshots and explicit weather policies that never silently alter a selected accepted plan;
- costs, tolls, parking, carbon, mileage balance, reimbursement, EV range/charging/fuel/maintenance;
- richer bounded multi-stop carpools, school/neighborhood groups, rotation, and a separately gated routing worker;
- explicit companion/escort/supervision/handoff-together rules that are never inferred;
- automatic impact detection, minimal-change draft repairs, notifications, and acceptance workflows;
- a mobile companion using the same typed application/domain API;
- multi-household/community coordination only after new identity, permission, privacy, collaboration, and abuse controls;
- additional/self-hosted providers, open map data, GTFS/GTFS-realtime, multimodal planners, CalDAV, and capability conformance matrices;
- optional AI classification/aliases/explanation/preference/repair assistance through reviewed typed commands only.

A future routing-specific worker remains provider-neutral, process-isolated, capability-scoped, license-reviewed, bounded, normalized through the standard solution contract, and subject to the same verifier and score hierarchy.

## Risks, mitigations, and stop conditions

| Risk | Required mitigation |
|---|---|
| Daily car assignment hides impossible movement | Continuous person/vehicle trajectories and handoff fixtures. |
| Current traffic contaminates future planning | Typed historical basis, planned-time bucket selection, immutable snapshot and UI disclosure. |
| Provider unavailable/unsupported | Capability checks, manual/file paths, local snapshots, actionable failure without partial mutation. |
| Calendar misclassification or sync overwrite | Review queue, deterministic suggestions only, durable local overrides, transactional sync/conflict state. |
| Ambiguous/sensitive places | Confirmed aliases, unresolved-place block, destination disclosure, redacted logs/exports. |
| Candidate/model explosion | Hard budgets, pruning/dominance/deduplication, pre-solve estimate, benchmark and cancellation evidence. |
| Transit chosen for convenience or without consent | Stage-A verified no-transit-first orchestration plus independent stage/opt-in verification. |
| Fragile transfers/coordination | Explicit buffers/restrictions and high-priority coordination-burden metrics. |
| Compiler/verifier share a defect | Original-domain evaluation, independently structured logic, mutation/differential/exhaustive small fixtures. |
| Provider data terms are violated | Adapter-enforced retention/cache/export flags, legal review, restricted-data omission. |
| Movement/calendar data leaks | Local-first storage, keyring, Rust network boundary, bounded payloads, CSP/capabilities, redaction/deletion. |
| Users read “verified” as legal/safety/reality certification | Persistent configured-model disclaimer and exact proof/fallback wording. |
| New domain destabilizes public MVP | Direct post-Phase-12 branch, clean registration gate, semantic compatibility/migration and release evidence. |

Pause and write or amend an ADR before proceeding if:

- historical planned-time travel cannot be represented independently of a provider;
- any provider requires network access from domain compilation, solving, projection, or verification;
- no-transit-first behavior cannot be proven by orchestration and authoritative score bounds;
- a route/journey/rule/score cannot be independently verified from original domain facts and local snapshots;
- calendar sync would silently overwrite local decisions or partially commit provider failure;
- credentials/raw tokens must enter Vue, SQLite, scenario/bundle/export, logs, or domain state;
- provider licensing/caching/retention/export terms cannot be established;
- a candidate set, payload, route stop sequence, or model becomes unbounded;
- the connected model must be split without proof of semantic independence and merge equivalence;
- a backend needs domain/provider knowledge or bypasses normalized projection, authoritative scoring, or verification;
- legal, safety, accessibility, supervision, driving, or transit suitability would be inferred rather than explicitly configured;
- a provider-free manual/file/offline path cannot remain functional;
- `official.transportation` would need registration before compatibility and release evidence is complete.

## Release exit gate

Phase 14 closes only when all of the following hold:

### Correctness and trust

- all 15 acceptance scenarios and required mutation/differential/property/boundary fixtures pass against canonical versions and hashes;
- every required rule, preference, score level, candidate category, repair/lock behavior, and two-stage transit invariant completes the full rule Definition of Done;
- every accepted plan is independently verified for the exact revision/snapshot, with zero required violations and verifier-owned score;
- person/vehicle continuity, time bucket/day class/DST, capacity/eligibility/buffers, transit consent/stage, and stale-data behavior have no known correctness defect;
- proof, infeasibility, time-limit, provider, manual-data, and real-world limitation wording matches evidence.

### Data integrity and compatibility

- calendar/file imports and sync are bounded and transactional; recurrence, exceptions, cancellations, conflicts, local overrides, stable IDs, detach/delete, and stale-plan handling pass;
- travel/transit snapshots are versioned, hashed, deterministic, provider-neutral, terms-compliant, and usable offline where retained;
- internal/database and Portable Scenario/Result/Share Result migrations, current-only export, unknown-semantic rejection/nonsemantic extension preservation, full-backup/add-or-replace restore/recovery, stable identity, provider reconnection/omission, and command history pass permanent fixtures;
- Planning IR, worker protocol, bundled OR-Tools capability, projection, CLI JSON, generated DTO, pack descriptor, portable bundle and standalone report compatibility are pinned and evidenced.

### Security, privacy, providers, and licensing

- every claimed adapter passes provider/auth/license/privacy/cache/persistence/export/conformance gates with current official evidence; unsupported candidates are not advertised or shipped;
- credentials reside only in the OS credential boundary; no precise/excluded scenario data or secrets leak into normal logs, support bundles, webview state, portable bundles, HTML/PDF/other exports;
- provider destinations and exact portable/share fields are previewed; disconnect/delete/detach, restricted-data omission and reconnectable references work;
- Tauri/application/report CSPs, inert rendering, URLs/redirects, archive/report payload/resource limits, output staging, SBOM/notices, packaged dependencies, and exact release licenses pass review.

### UX, accessibility, performance, and packaging

- a first-time household can complete provider-free manual/file setup, review, Place resolution, vehicle/transit policy, validation, Stage-A/Stage-B solve, explanation, lock, repair, compare, editable export/full backup, and privacy-reviewed immutable result sharing without AI/cloud/external solver;
- every claimed connected path passes the same flow and provider failure/reconnect/omission states on supported platforms;
- all essential actions work by keyboard and screen reader; map/timeline/report facts have list alternatives; status never relies on color; focus/announcements/reduced motion/zoom pass;
- Pack A–D manual/warm/cold/transit benchmarks meet their calibrated target and regression policy on the recorded reference machine; Stage A and B share one budget; calendar/provider work is absent from warm solve traces; first verified plans are not withheld by proof/detail/explanation; and bounded large-household generation, worker resource/cancellation, desktop/report responsiveness, and selected-detail fetching meet recorded baselines;
- clean supported-platform packages contain and run the exact tested worker; open/migrate/import/restore portable data; use manual travel; solve/verify; generate one-file `file://` HTML and direct PDF with exact privacy/no-network/inert-data evidence; and operate offline after permitted snapshots exist.

### Documentation and registration

- user documentation covers quick start, provider-free use, connected-source privacy/sync, Place resolution, historical-versus-live assumptions, transit fallback, repair, editable export/full backup/restore, Share Result privacy/offline HTML/PDF, limitations, recovery, and exact provider setup/support;
- developer documentation covers normalized contracts, snapshots/provenance/provider export policy, candidate/IR semantics, independent verification, score hierarchy, Portable Scenario/Share Result conversions/migrations, report trust boundary, conformance fixtures, and extension gates;
- the release manifest records exact source, pack/internal/portable/result/share/report/protocol/worker/dependency versions, hashes, licenses, provider capabilities/terms, platform/browser support, known limitations, and evidence;
- T7 performs one reviewed explicit registry addition and compatibility declaration. Before that change lands, `official.transportation` remains proposed/reserved and no current-behavior documentation may claim it is available.
