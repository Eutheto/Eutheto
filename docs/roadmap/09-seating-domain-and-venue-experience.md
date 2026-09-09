# Phase 09 — Seating Domain and Venue Experience

## Outcome

Deliver the official event-seating domain as a complete, independently verified desktop and CLI vertical slice. Eutheto assigns each included guest to an **actual seat** with stable identity, canonical venue geometry, table membership, world position, facing direction, adjacency, proximity, accessibility, and relationship semantics. The product must never collapse this domain to guest-to-table assignment.

The phase includes schemas/migrations, deterministic integer geometry, table/seat generation and custom placement, every MVP required rule and preference, CP-SAT compilation, symmetry control, independent verification, import, accessible venue/list editors, results, locks/repair/alternatives, current portable conversion, and deterministic CSV/SVG/one-file HTML/PDF/JSON output.

## Source coverage

This phase is the implementation source of truth for blueprint Section 19, relevant canvas/API/accessibility material in Sections 21–22, Phase 9, rule/desktop/backend Definitions of Done in Sections 31–33, CLI Appendix C, Tauri API Appendix D, complete Appendix G geometry/rule material, backlog `SEAT-001` through `SEAT-005` plus relevant `QA-001` work from Appendix I, the seating benchmark/rendering slice in [Performance and Solver UX Targets](performance-and-solver-ux-targets.md), and seating portable/share specialization in [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md).

Project-wide contracts are in [README.md](README.md), evidence/version gates in [assumptions.md](assumptions.md), domain/IR contracts in [Phase 02](02-domain-pack-and-planning-ir-contracts.md), solving in [Phase 03](03-ortools-worker-vertical-slice.md), independent verification in [Phase 04](04-independent-verifier-and-explanations.md), result/repair patterns in [Phase 07](07-workforce-solving-results-repair-and-export.md), and router policy in [Phase 08](08-pumpkin-backend-and-router.md). The optional assistant in [Phase 10](10-ai-assistant-mvp.md) may propose only typed seating commands and cannot replace deterministic setup.

## Dependencies and entry conditions

- Stable IDs, integer unit value objects, revisioned commands/batches, migrations, bundle import/export, history, undo/redo, and generated TypeScript contracts exist.
- Domain-pack command/catalog/UI-manifest and planning-IR/provenance/projection contracts exist.
- OR-Tools CP-SAT, independent verifier, score vectors, explanation evidence, counterfactuals, solve jobs, accepted-solution gate, repair, and alternatives work end to end.
- The desktop design system, typed API layer, workspace routing, import mapping, validation/error states, keyboard/focus infrastructure, and purpose-built query/view pattern exist.
- Canonical Rust code remains authoritative for scenario, geometry snapshot, solutions, verification, and persisted settings. Vue owns only route/selection/pan/zoom/temporary drag/form state.

## Decisions and invariants

1. **Actual-seat chain:** `Guest → Seat → Table → Venue position and orientation`. Every assignment and explanation names a stable seat.
2. **Canonical units:** persist and compute geometry in integer millimeters regardless of display units. Table rotation and seat facing use integer millidegrees.
3. **Deterministic preprocessing:** solver inputs use precomputed integer pair/relationship sets, never floating-point canvas geometry. Geometry has a canonical hash.
4. **Independent verification:** the verifier recomputes domain relationships from canonical seat geometry; it does not trust compiler pair lists or backend feasibility.
5. **Canvas is a view/editor, not authority:** every committed action is a revisioned Rust command. The synchronized non-canvas table/list editor exposes all essential operations.
6. **Accessible by design:** drag has keyboard actions; selection/status is not color-only; no critical fact exists only on hover or pixels.
7. **Identity survives layout edits:** generated seat numbering is canonical and stable under unchanged generation inputs. Changes requiring seat recreation show a diff and explicit lock/assignment impact.
8. **Manual edits are reversible and verified:** guest movement, table transforms, seat edits, adjacency overrides, and locks are command-history operations.
9. **Accepted outputs only:** arrangement exports derive from an accepted independently verified solution and canonical geometry.
10. **Privacy minimization:** store only guest/category/relationship data genuinely required; relationship labels are neutral and user-defined where possible.
11. **Seating share data is minimized and immutable.** Reports derive a versioned seating Share Result Model from an accepted arrangement and exact privacy options. It includes only selected guest aliases/names, table/seat assignments, deterministic plan geometry, accessible list semantics, safe provenance, and optional transportation summaries—not the editable relationship graph or arbitrary scenario data.

## Complete domain model

### `SeatingScenario`

```rust
pub struct SeatingScenario {
    pub venue: Venue,
    pub tables: Vec<Table>,
    pub seats: Vec<Seat>,
    pub guests: Vec<Guest>,
    pub groups: Vec<GuestGroup>,
    pub relationships: Vec<GuestRelationship>,
    pub placement_rules: Vec<SeatingRule>,
    pub preferences: Vec<SeatingPreference>,
    pub locks: Vec<SeatLock>,
    pub geometry_policy: GeometryPolicy,
    pub score_policy: SeatingScorePolicy,
}
```

All entities use stable IDs and pack-versioned schemas with migrations and unknown-data preservation.

### Venue

The venue contains stable identity/name, coordinate system, integer width/height, optional background/floor-plan image reference, zones, obstacles, entrances, stages/focal points, exits, and preferred display units. Canonical dimensions and all derived positions remain millimeters. Imported images are bounded by encoded size, decoded dimensions/memory, and safe local asset handling; pixels never define solver geometry implicitly.

### Table

```rust
pub struct Table {
    pub id: TableId,
    pub label: String,
    pub shape: TableShape,
    pub center_mm: PointMm,
    pub rotation_millidegrees: i32,
    pub zone_id: Option<ZoneId>,
    pub seat_generation: SeatGeneration,
    pub tags: BTreeSet<String>,
}
```

MVP shapes are circular, rectangular, banquet/long rectangular, head table, and custom polygon outline for display with manually placed seats. Table activation/capacity semantics are explicit. Generation definitions capture count, edge/order policy, and offsets sufficient to recreate stable local seats deterministically.

### Seat

A seat stores stable ID and label, table reference, local position, local facing angle, accessibility attributes/tags, activation/exclusion state, and optional fixed world position for custom seating. The compiled snapshot derives world position and facing from the table transform unless the custom fixed-world mode explicitly applies. A seat is not inferred from table capacity at solve time.

### Guest, groups, and relationships

A guest stores stable ID, display name, party/group references, accessibility needs, necessary tags, age/category metadata only when explicitly needed, and optional fixed/excluded zones. A `GuestGroup` gives stable group/party scope and required/preferred cohesion semantics. A `GuestRelationship` has stable ID, endpoints/group scope, neutral user-facing label, and one or more typed Required/Preference rules. Endpoints must exist and be distinct where the rule requires it.

Excluded guests and optional unassigned placeholder seats are represented explicitly, never by missing records. Imports never identify guests solely through similar names.

### Locks and policies

`SeatLock` records guest, seat, strength (hard/soft as supported by repair), source, and revision/base-arrangement relationship. `GeometryPolicy` stores deterministic distance/orientation thresholds, inclusive boundary rules, rounding policy, override policy, and model-size limits. `SeatingScorePolicy` records bounded priorities/weights and named alternative profiles.

## Deterministic geometry and relationships

### Preprocessing pipeline

1. Resolve every active seat’s world position in integer millimeters.
2. Resolve world facing direction using fixed-point vectors or one documented, consistently rounded trigonometric implementation.
3. Compute seat-to-seat squared distances in a sufficiently wide integer type with checked overflow.
4. Classify same-table, adjacent, opposite, nearby, back-to-back, line-of-interest, and zone relationships.
5. Build indexed normalized undirected seat-pair sets for each configured distance/orientation threshold.
6. Hash canonical venue/tables/seats/policy inputs and persist the geometry snapshot under that layout hash.

Distance comparisons use squared integer distances, avoiding square roots and floating instability. Angular thresholds are inclusive/exclusive exactly as documented and use deterministic rounding. Transform-invariance tests cover translation and full rotation where semantic zone/focal relationships permit.

### Adjacency

Generated circular tables derive adjacency from cyclic seat order. Rectangular/banquet/head tables derive it from canonical order along each edge with reviewed corner behavior. Custom layouts use explicit editable undirected adjacency edges. Normalize pair IDs so orientation is not accidentally duplicated or omitted. Required adjacency compilation consumes this canonical set.

### Proximity, opposite, orientation, and back-to-back

- Nearby/minimum-distance sets are computed from squared world distance across all tables, not only within a table.
- Opposite relationships derive from generated shape/order or explicit custom relation where geometry alone is ambiguous.
- Stage/focal-view preference compares world-facing direction with the vector toward the configured focal point using documented angular tolerance.
- Back-to-back classification requires distance below the configured threshold plus facing vectors generally away from each other or backs facing the same boundary; optional table-edge geometry may confirm close parallel placement.
- Because back-to-back is venue-specific, users can inspect the generated overlay and add/remove explicit reviewed overrides before solve. Overrides are scenario commands and enter the geometry hash/provenance.

### Appendix G conformance fixture

Preserve a checked-in complete equivalent of the illustrative two-table case:

- venue `venue-main`, `30000 × 18000` mm, display units feet;
- round `table-a` centered `(9000, 9000)`, diameter `1800` mm, rotation `0`, eight evenly generated seats at `500` mm offset;
- round `table-b` centered `(11500, 9000)`, diameter `1800` mm, rotation `180000`, same generation;
- guests Alice, Bob, and Carol, with Carol requiring `wheelchair-accessible` seat tags;
- Alice/Bob Required minimum physical distance `1600` mm;
- Alice/Bob Required back-to-back prohibition with maximum distance `2200` mm and angular tolerance `30000` millidegrees;
- Alice/Carol normal-priority same-table preference.

The compiled fixture asserts complete deterministic `seatWorldGeometry`, normalized adjacency pairs, back-to-back pairs, and within-1600-mm pairs under a BLAKE3 layout hash. The verifier independently recomputes classifications from canonical geometry.

## Exhaustive MVP rule and preference catalog

Every rule/preference receives schema/migration, namespaced command, validation, plain-language editor, IR compilation, backend translation/capability tests, independent evaluator, provenance/explanation, CLI/document example, edge/infeasible fixtures, import/export round trip, and model-size review.

### Required rules

1. Every included guest receives exactly one allowed seat.
2. No seat holds more than one guest.
3. Fixed guest-to-seat assignment.
4. Guest restricted to or excluded from a table, zone, or seat tag.
5. Guests/groups must be at the same table.
6. Guests/groups must be at different tables.
7. Guests must be adjacent.
8. Guests must not be adjacent.
9. Guests must be at least a configured physical distance apart.
10. Guests must not occupy a precomputed back-to-back or nearby pair.
11. Guest accessibility requirements must match seat accessibility attributes.
12. A group marked Required must fit at one compatible table.
13. Seat/table capacity and activation rules.
14. Explicit behavior for allowed unassigned placeholders and excluded guests; missing assignments never silently stand in for policy.

### Preferences

1. Prefer same table.
2. Prefer adjacent or nearby seats.
3. Prefer distance from another guest or group.
4. Prefer table, zone, or seat attributes.
5. Prefer view/orientation toward a stage or focal point.
6. Prefer balanced table occupancy.
7. Prefer party cohesion.
8. Prefer preserving manually placed guests/base arrangement.
9. Prefer avoiding empty singleton gaps around a table.

Preferences use bounded weights and recorded priority levels. Required feasibility is never traded for a preference.

## Compilation and symmetry

Create `y[guest, seat] ∈ {0,1}` only for statically allowed pairs after accessibility, zone, fixed-placement, required-tag, activation, and exclusion filtering; retain rejection provenance.

Core constraints:

```text
each included guest: Σ y[g,s] = 1
each active seat:    Σ y[g,s] ≤ 1
forbidden guest pair on forbidden seat pair:
  y[g1,s1] + y[g2,s2] ≤ 1
```

Generate both guest/seat orientations or canonicalize carefully so no forbidden orientation is missed. Same-table group rules may use per-guest/per-group table-selection variables instead of quadratic seat-pair constraints. Required adjacency can use allowed tuple constraints. Relevant distance/orientation sets are precomputed; do not generate all pair rules when thresholds/scopes eliminate most pairs.

Add only semantics-preserving symmetry breakers:

- order otherwise indistinguishable empty tables by first assigned stable guest ID;
- anchor one member of a fully interchangeable group to the lowest equivalent table/seat orbit;
- canonicalize generated seat numbering;
- form identical-table equivalence classes only when zone, geometry relationships, labels, activation/tags, and preferences do not distinguish them;
- never change user-visible table/seat identity or lock meaning.

Keep symmetry strategies behind compiler options for diagnostic isolation and benchmark them on generated event fixtures. A disabled strategy must preserve identical accepted semantics.

## Validation

### Fast validation

- unique guest, group, relationship, table, and seat IDs;
- every seat references an existing table;
- table transforms and canonical geometry are valid and within reviewed venue policy/bounds;
- no duplicate fixed guest-to-seat assignments;
- all relationship endpoints exist and differ where required;
- all thresholds are non-negative and arithmetic-safe;
- every custom adjacency edge references two valid distinct seats;
- shape/generation inputs, zones, focal points, attributes, locks, and overrides reference valid entities.

### Full pre-solve validation

- included guest count does not exceed allowed active seats unless explicit unassigned behavior is enabled;
- every included guest has at least one statically allowed seat;
- every Required same-table group fits at one compatible active table;
- fixed placements satisfy accessibility, tags, zones, activation, and relationship rules;
- obvious same/different, adjacency/non-adjacency, fixed-seat, and minimum-distance contradictions are actionable;
- geometry classifications match the current layout hash;
- hard locks are mutually valid;
- estimated pairwise constraints/variables remain within model limits, with warning before model generation.

Validation accelerates feedback but never replaces solve-based infeasibility detection.

## Venue setup and editing experience

Guided flow:

1. choose event preset and display units;
2. create venue dimensions or import a safely bounded floor-plan image;
3. place tables with drag, rotate, duplicate, align, shape, and seat count controls;
4. review adjacency, proximity, orientation, and back-to-back overlays and overrides;
5. add guests manually, by paste, or CSV;
6. create groups and neutral typed relationships in a plain-language editor;
7. add every Required rule and Preference with guest/group/table/zone/seat pickers;
8. validate capacity, geometry, accessibility, and relationship issues;
9. optimize one or more verified arrangements;
10. review visually and highlight allowed, prohibited, and related seats for a selected guest;
11. adjust and lock through reversible commands;
12. repair after guest/table/layout changes while minimizing movement;
13. export guest/table lists, deterministic high-resolution plan, privacy-reviewed immutable HTML/PDF result, or editable proposed `.eutheto` scenario.

The seating workspace maps setup routes to `venue`, `guests`, `relationships`, and `arrangement`. The domain UI manifest lists setup steps, entities, rule kinds, result views, importers, and exporters; purpose-built Vue components implement rich canvas/relationship interaction rather than forcing a schema-generated canvas.

## Canvas, design, performance, and accessibility

Use **Konva 10.3.2** with **vue-konva 3.4.0**, pinned in the Phase 00 lockfile. The scene has separate background, table, seat, relationship/geometry overlay, and selection layers. Cache derived viewport data by layout hash; query `SeatingViewport { bounds, zoom_bucket }` instead of returning the full document for large layouts. Rust performs parsing, transforms, and geometry classification. Profile representative 500-guest layouts, avoid rebuilding the scene on selection, and update selection immediately before fetching detail. Measure pan/zoom/select/input/render responsiveness separately from compile/backend/verification time so canvas work cannot hide solve regressions or vice versa.

Vue may hold viewport, zoom, selection, and temporary drag state. A drag preview is never authoritative; commit one transform command with `expected_revision`, then reconcile from returned invalidations. Canvas and list editors observe the same Rust view and commands.

Use semantic tokens for background, surface, text, focus, required, preference, success, warning, error, selection, plus canvas styles. Meet WCAG AA for text/controls, support light/dark and reduced motion, and never encode relationship/conflict/status solely by hue. Selected guest views distinguish assigned seat, Required companions, preferred companions, Required separation guests, and relationship-prohibited seat pairs using patterns, icons, outlines, and textual legends.

All operations are keyboard reachable with visible focus and predictable restoration. Provide table/list alternatives for venue objects, seats, coordinates, rotations, adjacency edges, guests, relationships, placement, and locks. Drag/rotate/resize/reassign has keyboard/button/command-palette equivalents with meaningful increments and direct numeric editing. Canvas objects expose semantic labels/descriptions through a synchronized DOM structure; announce validation and solve completion; no critical detail is hover-only. Manual keyboard and screen-reader scripts supplement automated checks.

Application-owned components include `VenueCanvas`, `TableEditor`, `SeatInspector`, `GuestRelationshipEditor`, and `GeometryOverlayLegend`, reusing `EntityPicker`, `RuleCard`, `ValidationSummary`, `ConflictCard`, `SolveProgress`, `SolutionStatus`, `ScoreBreakdown`, `ExplanationPanel`, `LockControl`, `ChangeSetPreview`, `ImportMappingTable`, empty states, and error recovery.

## Results, explanations, repair, and alternatives

MVP result surfaces:

- interactive pan/zoom venue canvas;
- table-centric guest list;
- guest search and location highlight;
- conflict and preference overlays;
- relationship inspector;
- unassigned/invalid list, empty for an accepted Required-complete arrangement;
- verified score and tradeoffs;
- ghost-overlay comparison between arrangements;
- print preview.

Selecting a guest identifies assigned seat, Required/preferred companions, Required separation guests, and relationship-prohibited seat pairs. Explain same/different/adjacent/distance/back-to-back/accessibility/orientation facts with exact canonical geometry and stable rule keys. Across-table proximity and back-to-back evidence must be visible, textual, and independently verified.

Manual guest placement is a command and can become soft/hard lock. Repair after guest, table, seat, accessibility, or relationship change records a base arrangement and minimizes changed placements at a high score level while preserving hard locks. Highlight preserved, moved, newly placed, and unassigned guests and the new fact/rule causing explainable changes. A search hint is not a lock.

Offer only meaningful recorded alternatives such as Balanced, relationships/preference focused, minimal changes, or occupancy focused where the scenario uses those objectives. Verify each sequential result and compare authoritative score/geometry metrics; do not manufacture arbitrary symmetric variants.

Seating reuses Phase 07's one-parent-budget, 300–500 ms progress threshold, truthful coarse phases, cancellation, status/proof language, and first-verified-feasible delivery. A raw backend arrangement never appears as a valid floor plan. The accepted arrangement and accessible list render before optional detailed explanation or AI paraphrase; a 500-guest result must not create one unbounded DOM/canvas update.

## Imports and exports

### Imports

- guest CSV;
- guest-relationship CSV;
- safely bounded venue background image;
- venue layout JSON from the project format.

Every import uses safe detection, explicit mapping/summary, stable-ID matching, additions/updates/duplicates/rejections preview, validation, one atomic undoable batch, and a rejected-row report. Never execute image metadata, macros, templates, paths, or embedded instructions.

### Exports and result sharing

- editable proposed `.eutheto` scenario/full backup through the shared Phase-01 portable services and seating pack conversion/migrations;
- assignment CSV by guest;
- assignment CSV by table;
- deterministic SVG venue plan;
- one self-contained interactive HTML result report;
- direct PDF from the same validated Share Result Model and reviewed local renderer; and
- JSON through the CLI.

Arrangement CSV/SVG/HTML/PDF/solution JSON require one accepted independently verified result and immutable canonical geometry. Editable scenario/full backup uses its separate valid portable-input gate and is available without a result. Preview shows scenario/result/revision/verification identities, stale status, exact selected report sections and privacy fields, editable/backup inclusions, and provider/reconnection exclusions.

The seating pack contributes versioned current Portable Scenario and Share Result schemas plus sequential historical migrations. Stable guest/table/seat IDs, canonical millimeter/millidegree geometry, relationships/rules, source references, and extension data round-trip semantically; unknown semantics fail safely. Provider-restricted or cached content is represented only by allowed references/metadata and never copied against policy.

SVG is generated from canonical Rust geometry or a reviewed shared deterministic geometry module, never from a rasterized canvas screenshot. It carries stable geometry, labels, legends/status/provenance, correct transforms/orientation, accessible text/contrast, and no executable or remote content.

HTML/PDF reuse Phase 07’s generic privacy-filtered Share Result builder, safe inert-data embedding, restrictive self-contained-report CSP, offline `file://` shell, print styles, and controlled local PDF path. The seating report embeds the deterministic schematic and accessible guest/table/seat lists; supports local view switching, search/filter/highlight, selection, detail expansion and print; and has no edit/solve/provider/server/storage/network authority. No essential meaning depends on remote fonts, icons, map tiles, services, canvas pixels, or color. Potentially sensitive guest names, addresses, source titles, notes, relationships/constraints, rejected alternatives, objective details, and external links default absent and are itemized by the exact preview payload.

PNG remains post-MVP. Hosted reports, annotations, signatures, encrypted bundles, automatic backups and rich map/routing presentation remain post-MVP.

## Commands, API, events, and CLI

Every seating command has a stable `official.seating.*` type ID, strict JSON Schema, result/change schema, localized title/description, human summary, permission/risk and reversibility metadata, AI-proposal eligibility, and examples. The catalog must cover venue settings/background reference; add/edit/remove/transform/duplicate table; generation/custom seat editing; adjacency/geometry override; add/edit/exclude guest; groups/relationships; every Required rule/preference; lock/unlock; and import application.

Generated TypeScript declarations include seating DTOs. Mutations carry scenario ID, expected revision, and request ID. Responses carry request ID, current revision, warnings, schema version, changed IDs, and invalidated view keys. Purpose-built views include overview, paged entities/rules, `SeatingViewport { bounds, zoom_bucket }`, solution summary, table list, and guest/relationship inspectors.

This catalog is a first-class Phase-10 assistant input, not a workforce-shaped API adapted by analogy. Bounded identity resolution must disambiguate duplicate guest/table labels using stable IDs; command-derived previews distinguish same-table, actual-seat adjacency, physical distance across tables, Required/Preference strength, accessibility requirements, and lock consequences with canonical units. Phase 09 owns these deterministic meanings and examples; Phase 10 owns conversational proposal execution and evaluation.

Use general scenario endpoints (`scenario_get_view`, entity/search/catalog queries, `scenario_apply_command`, batch, validation, undo/redo/history), solve endpoints, and complete solution compare/explain/counterfactual/lock/repair/export endpoints from Appendix D. Events carry version/timestamp/request/job/scenario/revision fields. Only frontend API modules call Tauri; Vue components call typed services/composables.

The working CLI name `optimizer` remains an unresolved naming gate. The CLI can validate, apply typed commands, solve, verify, compare, explain, and export seating scenarios without the desktop. `projects import/export` uses the portable services; `solutions export --format csv|svg|html|pdf|json` uses the accepted-result/privacy gates and report builder. Human/JSON results use the `eutheto` schema namespace and stable exit codes; every saved solution is independently accepted.

## Ordered work packages

1. **SEAT-001 — Schema/entities/migrations/portability:** all venue/table/seat/guest/group/relationship/rule/preference/lock/policy shapes, commands, document examples, import identities, current Portable Scenario/Share Result schemas, sequential migrations, capability declarations, and semantic round-trip fixtures.
2. **SEAT-002 — Integer geometry engine:** transforms, generation, stable seat IDs, distance/orientation/adjacency/back-to-back sets, overrides, layout hash/cache, Appendix G fixture, independent geometry evaluator.
3. **SEAT-003a — Rule slices:** each Required and Preference through validation, IR, OR-Tools translation, verifier, provenance/explanation, fixture, round trip, and size review.
4. **SEAT-003b — Formulation and symmetry:** allowed-pair filtering, table/group helpers, tuple/forbidden-pair generation, safe symmetry options, benchmark/model estimation.
5. **SEAT-004 — Venue experience:** routes, viewport query, layered Konva canvas, table/seat/overlay editors, synchronized accessible list/numeric controls, keyboard/focus and error states.
6. **Guest setup/import:** guest/group/relationship editors, CSV mappings, atomic import reports, privacy minimization.
7. **SEAT-005 — Solve/results/repair:** result canvas/lists/search/overlays/inspectors, locks, repair diffs, meaningful alternatives, counterfactual and status evidence.
8. **SEAT-005 — Export/share:** distinct editable/result gates; exact privacy preview; guest/table CSV; deterministic safe SVG; accessible offline one-file HTML; direct PDF/print; CLI JSON; source-edit immutability; visual/canonical/privacy/security/browser snapshots.
9. **Acceptance and usability:** versioned small/typical/symmetry/stress benchmark manifests; complete phase/model/quality metrics; 500-guest solve/result/canvas profiling; transform properties; canvas/list synchronization; keyboard/screen-reader and proximity-rule task scripts.

## Tests and acceptance fixtures

Minimum checked-in fixtures:

1. **Small round-table event:** 40 guests, 5 tables, group preferences.
2. **Required separation:** different tables but physically close across the table boundary.
3. **Back-to-back restriction:** exact two-adjacent-table case, including Appendix G thresholds/overlays.
4. **Accessibility:** fixed accessible seats and zone restrictions.
5. **Head table:** Required placements and orientation preferences.
6. **Repair:** remove a table after accepting an arrangement.
7. **Infeasible relationships:** Required same-table plus minimum-distance rules that cannot coexist.
8. **Symmetry benchmark:** many identical tables/interchangeable seats.

Each fixture includes visual geometry snapshot, complete deterministic pair-set/hash assertions, domain validation, expected status/metrics, every Required evaluator, score invariants, stable explanations, accepted-solution gating, current/historical Portable Scenario and Share Result preservation, unknown-semantic rejection, and import/export preservation. Do not assert one arrangement when equivalent solutions exist unless deterministic settings intentionally do so.

Benchmark evidence records geometry/candidate preprocessing, Planning-IR/model size, backend startup/solve, first incumbent, first verified feasible, verification, result preparation, initial canvas and accessible-list render, total time, termination/proof, and authoritative score. Assertions cover sub-threshold no-flicker behavior, truthful longer progress, cancellation latency, event/announcement bounds, optional-explanation delay/failure, and UI responsiveness under the 500-guest fixture. Phase 12 calibrates release thresholds against the cross-domain reference machine and corpus.

Property/differential tests cover integer transform composition; translation/rotation invariance; canonical seat numbering; pair normalization; distance/angular inclusive boundaries; checked overflow; generated/custom adjacency; back-to-back overrides; stale layout hash rejection; compiler/verifier mutation; symmetry on/off semantic equivalence; and SVG/canonical geometry agreement.

Share/report tests prove exact preview-to-payload equality; default omission of sensitive/source-only fields; inert rendering of malicious guest/relationship/label text; one-file `file://` operation with zero required network requests; HTML/SVG/list semantic agreement; keyboard/focus/screen-reader and graphical/list parity; print/PDF page context and grayscale readability; immutable generated output after source edits; atomic cancellation/failure cleanup; supported-browser compatibility; and responsive 500-guest output.

Desktop acceptance proves canvas/list synchronization through revisioned commands, keyboard parity for every drag operation, focus restoration, screen-reader labels/announcements, non-color overlays, stale/error/empty/active states, undo/redo, and responsive 500-guest pan/zoom/select. Required usability task: create an across-table physical-separation rule, inspect its overlay/evidence, optimize, manually move/lock, repair, export editable data, and share a privacy-reviewed immutable arrangement.

Retain synthetic duplicate-name, across-table-distance, accessibility, seat-lock, and infeasible-relationship examples as the seating side of Phase-10 cross-domain acceptance. Later experiment/voice clients reuse the same catalog, verified metrics, geometry and accessible list representation; their implementation is not a Phase-09 exit requirement.

## Risks and failure handling

- **Pairwise explosion:** pre-filter pairs, table/group variables, tuple constraints, threshold indexes, model estimates/limits, benchmarks.
- **Floating/rounding drift:** canonical integer millimeters/millidegrees, squared distance, one rounding policy, layout hash, property tests.
- **Canvas excludes users:** synchronized list/numeric editor, keyboard actions, semantic DOM, non-color legends, manual QA.
- **Canvas becomes authority:** temporary frontend state only; every committed edit uses expected-revision Rust commands.
- **Symmetry changes identity:** equivalence requires no distinguishing geometry/labels/preferences; never change user-visible identity.
- **Stale geometry:** hash every relevant input and reject/recompute mismatched classification snapshots before solving.
- **Sensitive guest data:** collect only needed fields; neutral relationships; local-first behavior and redacted diagnostics.
- **Malicious image/JSON:** dimension/allocation/nesting/path limits, strict parser, transactional import.
- **Invalid backend candidate:** same independent verification/quarantine as workforce.
- **Misleading “verified”:** means configured rules passed, not event safety, accessibility law, or legal compliance.

Pause for an ADR if a geometry/rule cannot be independently recomputed, domain semantics require backend knowledge, canvas needs direct persistence access, pair decomposition cannot prove independence, migration loses seat identities, or correctness relies on undocumented canvas/backend behavior.

## Exit gate

Phase 09 is complete only when:

- the user has tested the complete Seating workflow, including canvas/list parity, feedback is resolved, and explicit checkpoint acceptance and authorization to enter Phase 10 are recorded under the [manual checkpoint policy](README.md#user-manual-checkpoint-gates);
- every entity, Required rule, Preference, geometry relationship, and command above satisfies the complete rule/domain Definition of Done;
- all eight fixtures pass validation, solve status, independent verification, scoring, explanation, geometry snapshot, and expected metrics;
- the across-table and back-to-back cases are modeled from actual seat geometry and visibly/textually explained;
- canvas and list editors remain synchronized solely through revisioned commands, with undoable guest/table/seat/lock edits;
- venue transform properties, compiler/verifier differential tests, and safe symmetry equivalence pass;
- hard locks survive repair and every accepted manual/repaired result verifies;
- deterministic SVG exactly matches canonical geometry; CSV/SVG/HTML/PDF/JSON arrangement outputs enforce accepted-result provenance and exact privacy selection; editable proposed `.eutheto` output round-trips complete seating meaning without requiring a solution;
- all major states, keyboard routes, focus, labels, announcements, reduced motion, non-color cues, and screen-reader scripts pass;
- versioned small/typical/stress performance evidence records solver and render paths separately, first verified arrangements are not withheld by proof or optional explanations, and representative 500-guest profiling meets the responsiveness gate without moving geometry/solving onto the webview thread;
- desktop and CLI provide equivalent deterministic non-AI creation, validation, solve, explanation, repair, and export behavior.

## Deferred and non-goals

- PNG and richer raster/vector conversion beyond the required deterministic SVG and direct PDF path.
- CAD/BIM, map/routing services, arbitrary floor-plan interpretation, automatic obstacle safety certification, and legal venue compliance.
- Guest-to-table-only modeling, screenshot-based authoritative export, arbitrary visual-only relationships, or canvas-only functionality.
- School timetabling, plugin marketplace, dynamic native packs, and multi-backend portfolio behavior.
- AI is optional and cannot be required for any seating workflow.

## Assumption and version gates

- Pin **Konva 10.3.2** and **vue-konva 3.4.0** exactly in Phase 00’s pnpm lock. Reconfirm vue-konva peer compatibility with Vue 3.5.42 and Konva 10.3.2 in the install/build smoke gate; do not silently downgrade to the blueprint’s older unstated line.
- The wider verified frontend inventory as of 2026-08-29 is Vue 3.5.42, Vue Router 5.3.0, Pinia 4.0.3, Vite 8.2.2, Tailwind 4.3.3, shadcn-vue 2.8.2, Reka 2.10.4, TanStack Table 9.2.4, TanStack Virtual 3.13.36, ECharts 6.1.0, and vue-echarts 8.1.0. Exact locks remain Phase 00 actions.
- Public acceptance requires profiling the chosen Konva layer/cache/hit-detection strategy on 500-guest layouts and manual screen-reader/keyboard tests; library choice alone does not satisfy accessibility/performance.
- Geometry distance/orientation thresholds, back-to-back defaults, display-unit rounding, event presets, and user-facing score weights require usability/domain review and explicit persisted defaults.
- CLI name, application ID, signing, and hosting remain unresolved project gates. `.eutheto` is the proposed portable extension; Phase 11 closes the extension/media type/file association before public use.
