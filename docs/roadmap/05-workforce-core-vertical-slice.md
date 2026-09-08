# Phase 05 — Workforce core vertical slice

## Outcome

Deliver the first useful official domain pack entirely through the headless `eutheto` core and working CLI: create a workforce scenario; import people from CSV through safe detection, mapping, preview, validation, atomic apply, undo, and rejected-row reporting; generate timezone-correct shifts; validate people/work/eligibility/availability; compile eligibility/availability/coverage/no-overlap/minimum-rest semantics to planning IR; solve through the Phase 03 OR-Tools worker; and implement the first real Phase 04 projection, independent verifier, authoritative score, and typed evidence integration before exporting an accepted result. The clinic-plus-overnight-call scenario, including cross-category elapsed rest and DST behavior, is the executable proof.

The phase proves that official domain logic is not coupled to Vue/Tauri. [Phase 06](06-desktop-design-system-and-workforce-setup.md) adds setup and people-import UI over the completed Phase 05 services and commands. [Phase 07](07-workforce-solving-results-repair-and-export.md) completes the remaining MVP rule/preference/fairness/repair/result/export catalog and the remaining workforce import formats without changing the entity, import-transaction, and rule-completion disciplines established here.

## Source coverage

This phase incorporates blueprint Section 18 in full; the workforce document contract in Appendix F; Phase 5; workforce CLI behavior from Appendix C; workforce backlog items from Appendix I; planning/provenance/verification contracts from Sections 13 and 17; testing from Section 26; rule completion and definitions of done from Sections 31.3 and 33.2; domain/version gates from Appendix K; the workforce compiler/fixture responsibilities in [Performance and Solver UX Targets](performance-and-solver-ux-targets.md); and workforce portable/share-model groundwork from [Portable Data, Backup, and Result Sharing](portable-data-backup-and-sharing.md). Every MVP workforce rule, preference, validation, import/export, result view, and fixture is cataloged below even when implementation is dependency-gated to Phase 07.

## Dependencies

- Phase 01: scenario envelope/migrations, stable typed IDs/time/errors/revisions, commands/batches/undo, persistence/bundles, application services, and CLI shell.
- Phase 02: official pack registry, schema/command/view/validation interfaces, planning IR, projections, score/provenance/canonical hashing, and capability summary.
- [Phase 03](03-ortools-worker-vertical-slice.md): supported CP-SAT Boolean/integer/cardinality/linear translation, deterministic test profile, process budgets, candidate values, statuses, and assumption evidence.
- [Phase 04](04-independent-verifier-and-explanations.md): the completed pack-neutral projection/acceptance, verifier/report/score/evidence contracts, quarantine boundary, provenance mapping, and conformance harness. Phase 05 supplies the workforce implementations.
- `jiff` or the approved time layer must provide explicit IANA timezone and ambiguity handling. CSV import/export uses streaming parsing and bounded reports. Domain crates cannot depend on Tauri, SQLite implementation details, or solver implementations.
- The approved portable-v2 prerequisite separates pack-owned wire payloads from the frozen internal snapshot before WF-001 implementation. See the [compatibility contract](../architecture/compatibility-policy.md#portable-v2-prerequisite-compatibility). PR #21 integrated as `9b0b55ecb622a75b90e2bd97f30c0a223e6c3eb6` after all required PR and merge-group gates passed; this closes the portability prerequisite, not Workforce behavior.

## Domain purpose and boundary

The workforce pack assigns eligible people to time-bound work while satisfying coverage, qualification, rest, workload, fairness, and preference rules. Its reference setting is a medical practice with clinic sessions, daytime/overnight/weekend call, specialties, locations, and supervisory roles. The model must remain reusable for other shift-based work, but neither code nor presets claim certified legal/regulatory compliance. Presets disclose assumptions, jurisdiction/date, review status, and user responsibility.

## Entity and document contract

### Aggregate

```rust
pub struct WorkforceScenario {
    pub horizon: PlanningHorizon,
    pub calendars: Vec<WorkCalendar>,
    pub people: Vec<Person>,
    pub qualifications: Vec<Qualification>,
    pub locations: Vec<Location>,
    pub assignment_types: Vec<AssignmentType>,
    pub shift_templates: Vec<ShiftTemplate>,
    pub shift_instances: Vec<ShiftInstance>,
    pub coverage_requirements: Vec<CoverageRequirement>,
    pub availability: Vec<AvailabilityRule>,
    pub rules: Vec<WorkforceRule>,
    pub preferences: Vec<WorkforcePreference>,
    pub base_schedule: Option<BaseSchedule>,
    pub locks: Vec<AssignmentLock>,
    pub score_policy: WorkforceScorePolicy,
}
```

All IDs are typed stable identities; names are display fields, never identity keys. External JSON is camelCase and versioned. Unknown-newer schema versions fail safely. The authoritative pack document is stored once and migrated through pure sequential functions; generated instances/indexes are deterministic derived state unless explicitly detached.

Host-owned generic entity storage and generic entity commands use `EntityId`; `PersonId` identifies people, not qualifications, locations, or other entity kinds. Pack-owned typed identities cross the storage boundary through their existing UUID values. This distinction does not change the version-1 scenario envelope, UUID JSON strings, domain map layout, or persisted command representation.

### Planning horizon and calendars

`PlanningHorizon` defines inclusive local **shift-start dates** plus an explicit primary IANA timezone and DST policy from the scenario envelope. An overnight shift starting on the last included date remains in scope even when it ends on the following date; retain its complete interval for availability, overlap, rest, and elapsed-hour evaluation. `WorkCalendar` defines named day/week/pay/custom periods and reporting boundaries used by fixed-window rules and workload summaries. Calendar definitions must be deterministic and explicit; no host locale, timezone, wall clock, or “current date” is consulted during compilation.

### Person

Required semantics:

- stable internal ID and display name;
- active date/range;
- qualification and eligibility references;
- eligible assignment types;
- optional home location;
- optional contract/workload target or weight;
- tags and teams used by rule scopes;
- optional color/avatar display metadata that never changes solver identity.

Imports require a stable external ID or an explicit matching/review decision. Similar names are never silently merged.

### Qualification

A named capability such as `physician`, `pediatrics`, `overnight-call`, `supervisor`, or `lab-certified`. A person's qualification may be effective-dated and expiring. It must remain valid throughout the complete half-open shift interval, including an overnight extension beyond the final horizon start date. A qualification expiring during an assignment makes that person ineligible; validity ending exactly at the shift end is sufficient.

### Location

A stable named workplace referenced by shifts, people, availability, and future transition/travel rules. Optional home location and a configured location-pair duration matrix support later transition semantics; Phase 05 preserves the data shape but does not require a map service.

### Assignment type

Examples are morning clinic, afternoon clinic, full-day clinic, overnight call, weekend call, backup call, and administrative block. An assignment type defines semantic category, time behavior, default duration, eligible qualifications, location behavior, and the named workload buckets to which it contributes.

### Shift template and shift instance

A template generates recurring concrete instances over the horizon. The instance is the actual assignment/coverage unit:

```rust
pub struct ShiftInstance {
    pub id: ShiftId,
    pub template_id: Option<ShiftTemplateId>,
    pub assignment_type_id: AssignmentTypeId,
    pub location_id: Option<LocationId>,
    pub starts_at: ZonedDateTime,
    pub ends_at: ZonedDateTime,
    pub required_count: u16,
    pub preferred_count: Option<u16>,
    pub max_count: Option<u16>,
    pub required_qualification_sets: Vec<QualificationRequirement>,
    pub tags: BTreeSet<String>,
}
```

A generated instance may detach from its template for a one-off edit. Regeneration presents a deterministic diff and never overwrites a detached/manual change without confirmation. Template generation is stable for the same horizon/timezone/DST policy and never creates instances whose local start date is outside the inclusive horizon.

### Coverage requirement

Coverage can be embedded in generated instances and/or represented as stable requirements. It supports exact count, minimum count, bounded maximum, and one or more required qualification expressions/slots. Each qualification slot is an independent minimum among assigned people: a dual-qualified person may satisfy both a supervisor requirement and a specialist requirement, while counting only once toward total headcount. Slots do not imply distinct staffed positions.

### Availability rule

Availability expresses unavailable intervals, recurring weekly unavailability, available-only windows, approved time off, effective ranges, assignment-type restrictions, location restrictions, source, and note. Normalize rules to deterministic interval predicates over generated shifts while retaining rejection provenance so a user can inspect exactly which rule blocks a person/shift pair.

### Base schedule, locks, and score policy

The aggregate reserves a base accepted solution and assignment locks from the start so migrations do not retrofit identity later:

- **Hard locked:** assignment cannot change.
- **Soft locked:** a change incurs a high stability penalty.
- **Unlocked:** may change freely.

`WorkforceScorePolicy` records the objective profile, ordered score levels, priority mapping, workload/fairness configuration, and deterministic tie-break policy. Phase 05 serializes and validates these shapes; full repair/fairness behavior is Phase 07.

## Time, overnight, and DST invariants

- A scenario has one primary IANA timezone. Every shift stores an unambiguous instant range and intended local representation.
- Recurrence generation detects nonexistent local times and ambiguous local times. It applies the scenario's explicit DST policy or returns a review-required validation issue; it never guesses silently.
- Overlap, minimum rest, and actual-hour accounting use elapsed instants unless an individual rule explicitly declares scheduled/local hours.
- When wall-clock duration differs from elapsed duration, validation/views/export evidence retain both.
- An overnight call beginning Monday 20:00 and ending Tuesday 06:00 may belong to a configurable reporting day, but overlap and rest always use the entire instant interval.
- Horizon bounds, inclusive end date, reporting-day attribution, effective qualifications, recurring availability, and fixed/rolling windows are tested at boundaries.

## MVP rule and preference catalog ledger

### Phase 05 required vertical-slice rules

1. **Eligibility:** assign only when all required qualification expressions and assignment-type eligibility conditions hold. Preprocess impossible pairs and preserve rejection provenance.
2. **Availability:** reject a shift intersecting unavailable time or outside an available-only window, including effective date/type/location restrictions.
3. **Coverage:** meet each shift/qualification slot's exact or minimum count and bounded maximum.
4. **No overlapping assignments:** a person cannot hold overlapping shifts unless explicit domain configuration declares the two categories compatible and defines workload-hour counting.
5. **Minimum rest:** require elapsed duration between the end of a matching source assignment and start of a matching target assignment. Scope filters support assignment types, people, teams, weekdays, and locations. Reference rule: everyone eligible for overnight call and clinic needs at least 10 hours after overnight call before clinic; strength `Required`.

These rules are complete only under the rule discipline below. They include compiler, independent verifier, provenance/explanation, commands/document/CLI examples, and edge/infeasible fixtures—not merely a CP-SAT expression.

### Remaining required MVP rules, completed in Phase 07

The schema/catalog/commands must reserve stable semantics now; these are not silently omitted:

6. **Maximum hours in a fixed period:** counted hours per day, week, pay period, or custom calendar interval.
7. **Maximum hours in a rolling window:** counted hours in every rolling duration, for example 80 hours in any 14 days; generate candidate windows only where a relevant shift start/end changes membership, not every minute.
8. **Consecutive assignments/days:** maximum consecutive worked days, nights, or matching assignment types.
9. **Maximum assignment count:** limit assignment count by type over a configured period.
10. **Required skill mix:** at least the configured count satisfies each qualification expression.
11. **Fixed assignment/hard lock:** require a person/shift pair or preserve it during repair.
12. **Mutual assignment restriction:** prevent configured people together on the same shift or overlapping matching shifts when it is a legitimate operational rule.
13. **Transition/travel time:** configured setup/travel duration between assignments at distinct locations using a location-pair matrix; no map-service dependency.

### Complete MVP preference catalog, completed in Phase 07 unless needed by a Phase 05 fixture

- prefer or avoid a specific date/time;
- prefer or avoid an assignment type;
- prefer a location;
- honor requested time off when not required;
- keep assignment count near a target;
- balance a named workload bucket across a peer group;
- avoid consecutive nights/weekends;
- prefer or avoid assignments adjacent to existing work;
- keep a base schedule stable;
- keep people together or separate when operationally appropriate;
- prefer coverage above the minimum.

Every preference has explicit priority and bounded weight. Human labels `Low`, `Normal`, `High`, and `Very high` map through the recorded score policy; a preference is never a secretly relaxable required rule.

### Fairness and repair semantics carried forward

Fairness asks what to balance: assignment count, counted hours, overnight call, weekends, holidays, undesirable shifts, or deviation from target/FTE-weighted share. The recommended default is convex deviation from a target share using absolute or piecewise penalties. Report actual values/spread, not only a percentage. Unequal contracts use configurable workload weights; a half-time person is not forced toward a full-time raw count unless equal shares are explicitly selected.

Repair compiles a change indicator per base assignment and minimizes change at a high lexicographic level. Result evidence identifies unchanged, moved/replaced, newly unfilled, and newly assigned work and only claims a new rule/fact made a change necessary when deterministic/counterfactual evidence supports it. Backend hints may guide search but never substitute for explicit locks/stability penalties.

## Phase 05 compilation

### Pure pipeline

1. Parse/migrate the envelope and workforce document.
2. Normalize defaults, calendars, assignment types, templates, and generated instances.
3. Run structural then semantic validation.
4. Build stable indexes for people, qualifications, locations, types, shifts, coverage, availability, and rules.
5. Precompute timezone facts, static eligibility/availability, conflict/rest graphs, and model-size summary.
6. Create one Boolean variable for each feasible person/shift pair.
7. Compile required coverage, no-overlap, and cross-category rest with provenance.
8. Add any bounded fixture preference/objective terms and solution projections.
9. Validate planning IR, capability requirements, components, and canonical hash.

Compilation is pure for scenario revision plus explicit options/clock context.

### CP-SAT formulation

For every statically feasible pair:

```text
x[person, shift] ∈ {0,1}
```

Do not create variables for ineligible/unavailable pairs, but retain a `Person × Shift` rejection fact and mapped availability/qualification provenance for explanations.

```text
coverage:
  min ≤ Σ x[p,s] ≤ max

person overlap:
  x[p,s1] + x[p,s2] ≤ 1

rest conflict:
  x[p,s1] + x[p,s2] ≤ 1

hours (Phase 07 rule, reserved formulation):
  Σ duration_bucket[s] * x[p,s] ≤ limit
```

Coverage qualification sums include only people satisfying the expression. Required coverage is never weakened by a preference. For fixed shifts, compare pairwise conflict cliques/cardinality with optional interval/no-overlap formulations and choose based on benchmark/model size while preserving semantics. Apply symmetry breaking only when meaningful person distinctions are preserved; qualifications/preferences must not be erased by arbitrary person order.

### Projection

Project selected variables into normalized assignments keyed by stable `PersonId` and `ShiftId`, with scenario revision, source solution/run IDs, and score/explanation inputs. Reject missing required projection values, unknown IDs, duplicate assignments, assignment to a noncandidate pair, and malformed backend values before domain verification.

This is the workforce implementation of the Phase 04 pack projection contract, not a Phase 04 deliverable.

## Independent workforce verifier

The verifier works from normalized workforce scenario plus projected assignments, not planning constraints or CP-SAT status. Phase 05 evaluates:

- every assignment references an active person and concrete shift;
- qualification and assignment-type eligibility at the shift time;
- every applicable availability predicate;
- exact/min/max coverage and each required qualification slot;
- pairwise overlap under explicit category-compatibility semantics;
- elapsed minimum rest for every scoped ordered assignment pair, including overnight and DST boundaries;
- lock/base/score shapes needed by the initial fixture even when full repair is deferred;
- preference contributions actually used by checked-in fixtures;
- deterministic metrics, rule evaluations, stable explanation keys, and accepted feasibility `0`.

A backend candidate failing any item is quarantined under Phase 04. Compiler/verifier differential enumeration must cover small people/shift matrices and mutate both sides to prove disagreement detection.

Phase 05 owns the complete workforce adapter to the Phase 04 contracts: workforce projection, original-semantics rule verification, verifier-owned score recomputation, workforce evidence/message rendering, and acceptance/quarantine integration. No workforce compiler record or backend objective is reused as authoritative verification evidence.

## Validation

### Fast validation after edits

- IDs are unique; names are unique where the specific entity requires it;
- start/end and horizon ranges are valid;
- every reference resolves;
- counts, hours, durations, priorities, and weights satisfy non-negative/bounded semantics;
- rule scopes are non-empty unless explicitly saved inactive;
- templates generate within the horizon;
- every qualification referenced by coverage exists;
- local recurrence times resolve under the explicit DST policy.

### Full pre-solve validation

- every required shift has enough statically eligible candidates;
- qualification coverage slots are internally possible;
- hard locks are individually eligible/available;
- overlapping hard locks are detected;
- obvious minimum-rest conflicts among hard locks are detected;
- maximum-hour limits are not already violated by hard locks when that rule is enabled;
- target/fairness groups are non-empty;
- rolling-window generation remains within configured model limits.

Validation provides early actionable errors but does not replace solving. It distinguishes field/data errors, obvious contradictions, model-size/resource warnings, and genuine solver-proven infeasibility.

## People CSV import backend

Phase 05 completes the people CSV importer required by the Phase 06 setup flow; it is not deferred to Phase 07. The headless application/core service contract is:

```text
detect_people_csv(input, limits) -> PeopleCsvDetection
preview_people_csv(scenario_revision, input_digest, mapping, identity_decisions) -> PeopleImportPreview
apply_people_csv_import(scenario_id, expected_revision, preview_id, request_id) -> CommandBatchResult
people_csv_rejected_rows(preview_id) -> BoundedRejectedRowReport
```

The desktop adapter passes an explicitly granted file to the same bounded byte-stream service that a headless caller can use; Vue never performs the authoritative parse. Detection recognizes CSV structure and encoding without executing cell content, rejects unsupported/binary input, and enforces configured byte, row, column, cell, and report limits. Parsing is streaming and cancellable.

`PeopleImportPreview` is bound to the scenario ID/revision, input digest, mapping, and explicit identity decisions. It reports mapped columns, additions, updates, exact-ID duplicates, unresolved identity matches, validation issues, and rejected rows with stable row numbers and safe reason codes. A stable external ID is matched automatically only on exact equality; a missing/changed ID or similar name requires an explicit add/update/skip decision. Preview runs the workforce person/schema/reference validation against the proposed post-import state and blocks apply on unresolved or fatal issues.

Apply rechecks the preview binding and `expected_revision`, converts only the previewed accepted changes into one Phase 01 scenario command batch, and commits that batch atomically. Rejected rows never mutate the scenario; a failed or cancelled apply commits nothing. The result retains the bounded rejected-row report and returns the command-batch/history ID, so `scenario_undo` reverses the entire import in one step and `scenario_redo` reapplies that same canonical batch. A stale revision, changed file digest, mapping change, or changed identity decision requires a new preview.

## CLI vertical slice

The CLI is first-class, runs with no desktop process/database dependency when explicit paths are provided, and uses the same pack/compiler/backend/verifier/application services. The working binary name is `optimizer` until the naming gate is resolved.

Core workflow:

```text
optimizer projects create --pack official.workforce --title <title>
optimizer scenario apply <input> --commands <json> --output <path>
optimizer scenario validate <input>
optimizer solve <input> --mode quick|balanced|deep --max-time <duration> --output <solution-path>
optimizer solutions verify <scenario> <solution>
optimizer solutions explain <scenario> <solution> [request options]
optimizer solutions export <scenario> <solution> --format csv|ics|json
```

`solve` loads/migrates, validates, compiles/routes, solves, projects, independently verifies, writes a normalized solution only when accepted, then emits exact status/proof/verified-score summary. Diagnostics go to stderr; requested data goes to stdout/files; secrets never print. `--format json` emits one versioned envelope such as `apiVersion: "eutheto/cli-result/v1"`; progress is off unless `--progress jsonl`, then it goes to stderr or explicit destination.

Relevant stable exit codes are `0` success, `2` usage, `3` validation failure, `4` proven infeasible, `5` no verified solution within limits, `6` backend unavailable/incompatible/failed, `7` independent-verification alarm, `8` file/bundle/database/migration error, `10` revision/state conflict, and `130` cancellation. Infeasibility is a successful execution path with code `4`, not an internal exception.

The Phase 05 export proof includes normalized solution JSON and assignments CSV round-trip plus workforce Portable Scenario/Share Result conversion fixtures. The full MVP ledger is editable scenario/full backup through the proposed `.eutheto` portable contract, assignments CSV, people/shift summary CSV, per-person/combined iCalendar, one-file privacy-filtered offline HTML, direct PDF, and CLI JSON; Phase 07 completes the unimplemented formats and recipient UX.

## Illustrative canonical workforce fixture

The generated JSON Schema and migration fixtures are authoritative. This checked-in readable fixture preserves the complete Appendix F example while using the final project namespace:

```json
{
  "format": "eutheto/scenario",
  "formatVersion": 1,
  "scenarioId": "scenario-clinic-september",
  "domainPack": {
    "id": "official.workforce",
    "schemaVersion": 1
  },
  "metadata": {
    "title": "September clinic and overnight call",
    "description": "Example medical-practice schedule",
    "createdAt": "2026-08-28T23:00:00Z",
    "updatedAt": "2026-08-28T23:00:00Z"
  },
  "settings": {
    "timeZone": "America/Chicago",
    "locale": "en-US",
    "units": "us-customary",
    "dstPolicy": "require-review-on-ambiguity"
  },
  "domain": {
    "horizon": {
      "localStartDate": "2026-09-01",
      "localEndDateInclusive": "2026-09-30"
    },
    "qualifications": [
      { "id": "q-physician", "name": "Physician" },
      { "id": "q-call", "name": "Overnight call eligible" }
    ],
    "locations": [
      { "id": "loc-main", "name": "Main clinic" }
    ],
    "assignmentTypes": [
      {
        "id": "type-clinic-am",
        "name": "Morning clinic",
        "category": "clinic",
        "countsToward": ["all-hours", "clinic-hours"]
      },
      {
        "id": "type-call-overnight",
        "name": "Overnight call",
        "category": "call",
        "countsToward": ["all-hours", "call-hours", "overnight-count"]
      }
    ],
    "people": [
      {
        "id": "person-smith",
        "name": "Dr. Smith",
        "qualifications": ["q-physician", "q-call"],
        "eligibleAssignmentTypes": ["type-clinic-am", "type-call-overnight"],
        "workloadWeight": 1.0
      },
      {
        "id": "person-jones",
        "name": "Dr. Jones",
        "qualifications": ["q-physician", "q-call"],
        "eligibleAssignmentTypes": ["type-clinic-am", "type-call-overnight"],
        "workloadWeight": 1.0
      },
      {
        "id": "person-patel",
        "name": "Dr. Patel",
        "qualifications": ["q-physician", "q-call"],
        "eligibleAssignmentTypes": ["type-clinic-am", "type-call-overnight"],
        "workloadWeight": 0.8
      }
    ],
    "shiftTemplates": [
      {
        "id": "template-weekday-clinic",
        "name": "Weekday morning clinic",
        "assignmentTypeId": "type-clinic-am",
        "locationId": "loc-main",
        "recurrence": {
          "weekdays": ["monday", "tuesday", "wednesday", "thursday", "friday"]
        },
        "localStartTime": "08:00",
        "localEndTime": "12:00",
        "coverage": {
          "minimum": 2,
          "maximum": 2,
          "qualificationSets": [
            { "minimum": 2, "allOf": ["q-physician"] }
          ]
        }
      },
      {
        "id": "template-nightly-call",
        "name": "Nightly overnight call",
        "assignmentTypeId": "type-call-overnight",
        "recurrence": {
          "weekdays": [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday"
          ]
        },
        "localStartTime": "20:00",
        "localEndTimeNextDay": "06:00",
        "coverage": {
          "minimum": 1,
          "maximum": 1,
          "qualificationSets": [
            { "minimum": 1, "allOf": ["q-call"] }
          ]
        }
      }
    ],
    "availability": [
      {
        "id": "availability-smith-thursday",
        "personId": "person-smith",
        "kind": "unavailable",
        "range": {
          "localStart": "2026-09-10T00:00:00",
          "localEnd": "2026-09-11T00:00:00"
        },
        "note": "Requested day off"
      }
    ],
    "rules": [
      {
        "id": "rule-rest-call-to-clinic",
        "type": "minimum-rest",
        "strength": "required",
        "scope": { "people": "eligible-for-both" },
        "afterAssignmentTypes": ["type-call-overnight"],
        "beforeAssignmentTypes": ["type-clinic-am"],
        "minimumMinutes": 600
      },
      {
        "id": "rule-max-hours-rolling",
        "type": "maximum-hours",
        "strength": "required",
        "bucket": "all-hours",
        "window": { "kind": "rolling", "minutes": 20160 },
        "maximumMinutes": 4800
      },
      {
        "id": "rule-max-consecutive-nights",
        "type": "maximum-consecutive-assignments",
        "strength": "required",
        "assignmentTypes": ["type-call-overnight"],
        "maximum": 2
      }
    ],
    "preferences": [
      {
        "id": "pref-jones-no-friday",
        "type": "avoid-time",
        "strength": "preference",
        "priority": "normal",
        "personIds": ["person-jones"],
        "weekdays": ["friday"]
      },
      {
        "id": "pref-balance-call",
        "type": "balance-workload",
        "strength": "preference",
        "priority": "high",
        "bucket": "overnight-count",
        "peerGroup": { "qualificationAllOf": ["q-call"] },
        "target": "weighted-equal-share"
      }
    ],
    "baseSchedule": null,
    "locks": [],
    "scorePolicy": {
      "profile": "balanced",
      "levels": [
        "stability",
        "high-preferences",
        "fairness",
        "normal-preferences",
        "deterministic-tie-break"
      ]
    }
  },
  "extensions": {}
}
```

Expected behavior is deterministic generation of September instances in `America/Chicago`; static rejection of ineligible pairs; exactly two clinicians per clinic and one per overnight call; no call→clinic pair with less than 600 elapsed minutes; relevant rolling 14-day windows; no more than two consecutive overnight calls; Jones Friday preference and weighted call balance after required rules; stable person/shift projection; and independent verification of all enabled rules. Phase 05 must implement the slice rules and parse/preserve the later rules/preferences; Phase 07 makes every expected behavior above executable before the workforce MVP gate.

## Acceptance fixture corpus

Commit versioned fixtures with fixed clock, explicit timezone/locale, stable IDs, budgets/seeds, reviewed expected semantics, model-size envelope, warm/cold preconditions, and required timing/quality fields:

1. **Clinic only, small:** 8 people, 4 weeks, availability and fairness.
2. **Clinic + overnight call:** 12 people with cross-category 10-hour elapsed rest.
3. **Rolling-hours stress:** overlapping rule windows and hard locks.
4. **Qualification coverage:** one specialist required per session.
5. **Repair after call-out:** accepted, selected local baseline schedule plus one new unavailability.
6. **Provably infeasible:** insufficient eligible coverage with mapped sufficient conflict evidence.
7. **DST transition:** overnight shifts across spring-forward and fall-back transitions.
8. **Large benchmark:** configurable 100+ people and thousands of candidate assignments.

The small, clinic+overnight, and large/stress fixtures form the workforce instances of benchmark Packs A–D. Record candidate pairs before and after each deterministic rejection class, Planning-IR size, compile/backend/verify/total spans, first incumbent, first verified feasible, termination/proof, and authoritative score. Phase 05 establishes deterministic manifests and compiler/CLI evidence; Phase 07 measures the complete desktop result path; Phase 12 calibrates release thresholds.

The executable-fixture portion of the Phase 05 gate requires complete compile/solve/verify assertions for clinic+overnight, infeasible coverage, and DST plus the initial clinic slice. The other documents and stable expectations are committed now and graduate as their rule implementations land in Phase 07. For every executable fixture assert domain validation, expected status, independent verification, score invariants, stable explanation keys, and applicable import/export round trip. Do not assert one exact schedule when equivalent solutions exist unless the deterministic test profile intentionally makes that contract.

## Import completion and downstream contracts

Phase 05 completes people CSV end to end under the backend contract above. The complete MVP import ledger remains people CSV, eligibility CSV or pasted matrix, shift-instance CSV, availability/time-off CSV, and existing-assignment CSV. Phase 07 completes only the four remaining formats; each must reuse the Phase 05 detection, explicit identity, preview, validation, atomic-batch, undo, and rejected-row disciplines rather than replace them.

The complete result-view ledger for Phase 07/desktop is person schedule grid, shift/location grid, person timeline, uncovered/overcovered work, required-rule summary, preference summary, fairness distribution, base-schedule changes, warnings/status, and explanation panel. Large grids are virtualized rather than creating a DOM cell for a year-long matrix.

The complete export ledger is editable scenario/full backup through the proposed `.eutheto` portable contract, assignments CSV, people/shift summary CSV, per-person/combined ICS, one-file privacy-filtered standalone HTML, direct PDF, and CLI JSON. Phase 05 supplies workforce current portable conversion fixtures, every genuine supported historical conversion, and accepted Result→Share Result payload groundwork; the initial-schema history policy below applies. Phase 07 completes recipient rendering/privacy UX. XLSX, payroll/vendor adapters, live calendar synchronization, encryption, signatures, and hosted sharing are post-MVP.

The Phase-10 assistant consumes the same generated command/catalog and bounded query/evidence contracts as deterministic clients. For each Phase-05-complete rule, record stable entity/rule IDs, parameter units, scope and empty-scope behavior, Required/Preference support, timezone/effective-range semantics, localized command-derived summaries, and explicit implemented/unsupported capability status. These are domain contracts, not a second AI schema registry. Later-rule metadata must not imply that a reserved rule already compiles or verifies.

Preserve examples that expose ambiguous names, overnight/DST interpretation, and the difference between an invalid document, valid but contradictory requirements, and an unresolved search. Phase 10 adds conversational interpretation and proposal tests; Phase 13 adds experiments and voice. Neither requires AI providers, profiles, microphones, agents, or assistant production code in Phase 05.

## Ordered work packages

1. **WF-001 — schema/entities/migrations and people CSV import:** version the aggregate and every entity/value object, internal and portable JSON Schema, stable typed IDs and capability metadata, current portable conversions and every genuine supported historical migration, commands, semantic round-trip fixtures, and future-rule/declared-nonsemantic-extension preservation. First complete the unregistered module's bounded CSV detection, explicit mapping/identity decisions, validated preview, command-batch construction, and rejected-row reports. Application-level atomic batch apply and one-step undo/redo remain required and close after WF-007 under the staged integration policy below.
2. **WF-002 — timezone/shift generation:** calendars, horizon bounds, recurring templates, detached-instance diff semantics, overnight/reporting day, ambiguous/nonexistent local time policy, and deterministic generation.
3. **WF-003 — eligibility, availability, coverage, and no-overlap:** build the static candidate graph from qualification/assignment eligibility, effective ranges, and availability predicates; retain rejection provenance and the model-size estimate; complete coverage/qualification slots and no-overlap/compatibility through the full rule-completion artifacts.
4. **WF-004 slice — minimum rest:** complete scoped cross-category elapsed minimum rest, including overnight and DST boundaries, through compiler, verifier, provenance/explanation, command/document/CLI, and edge/infeasible fixtures. Maximum-hours and consecutive-assignment rules remain parse-and-preserve catalog entries here and are completed in Phase 07.
5. **WF-006 slice — compiler/workforce projection:** variables only for feasible pairs, exact initial constraints, bounded objective used by fixtures, capability/provenance maps, canonical ordering/hash, and the workforce implementation of the Phase 04 normalized-solution projection contract.
6. **WF-007 slice — workforce verifier/score/evidence:** independent structural/eligibility/availability/coverage/overlap/rest evaluation, verifier-owned score/evidence/checksum, stable workforce message keys, and acceptance/quarantine integration against the completed Phase 04 pack contract.
7. **WF-008 — CLI and fixtures:** create/apply/validate/solve/verify/explain/export flow, stable envelopes/exit codes, Appendix F fixture, benchmark manifests for small/typical/stress workloads, required timing/quality evidence, acceptance corpus, and UI-independent execution.
8. **Phase 07 handoff:** preserve and test schema/catalog entries for hours, rolling windows, consecutive/count/skill/lock/mutual/travel, every preference, fairness, repair, the four remaining import formats, results, and remaining exports.

**Maintainer-approved staged integration (2026-09-04):** WF-001's schema, pure commands, and CSV review may land before the complete pack exists; that is module completion, not application-end-to-end completion. Implement WF-002 through WF-007 in the listed order, then register the complete production pack and close WF-001's application CSV apply, rejection-report, atomicity, and restart-safe one-step undo/redo acceptance before WF-008's CLI closure. Do not add placeholder pack methods, bypass normal application dispatch, or invent a partial-pack lifecycle to claim earlier availability.

**Maintainer-approved initial-schema history (2026-09-04):** Workforce starts at internal schema v1 and portable schema v1 with no supported historical Workforce versions. Require current round trips and explicit rejection of unsupported older/newer versions now. Historical Workforce migrations and sequential migration fixtures become mandatory at the first genuine schema change; never invent a v0 or an artificial immediate v2 to manufacture history. Existing real global portable-format migration fixtures and compatibility gates remain required.

### WF-002 temporal module contract

The `eutheto_workforce::temporal` APIs (`resolve_shifts`, `preview_generation`, `resolve_calendar`, and `reporting_window`) return bounded operation-local Rust values. They do not register the pack, persist edits or approvals, establish eligibility/feasibility, or act as a verifier. The application owns explicit registration of the completed pack, normal command routing and durable review/apply.

- Generated recurrence uses inclusive scenario-zone local start dates, intersected with half-open effective dates, weekdays and exclusions. A final-date overnight shift keeps its entire end. After DST resolution the actual local start date must still belong to that date range, including repeated-midnight and skipped-date transitions.
- Manual and detached shifts instead use explicit start-instant membership in the half-open host horizon, retain their complete intervals and fold choices, and are never regenerated from templates. A detached origin suppresses its original template/date even when the edited one-off is outside the horizon.
- Local windows retain both intended wall endpoints; elapsed-duration shifts add elapsed time to the resolved start and derive the actual local end. Scheduled and elapsed durations remain exact signed durations, without minute rounding or substitution. Gap/fold rejection and pack-owned resolution remain explicit errors.
- Prospective ledger/detached owners take precedence. A matching before/prospective template/date with different IDs is an identity-transition conflict, including dormant and detached owners. Missing prospective definitions reuse matching prior IDs only without a different prospective owner; newly derived IDs are checked against the complete before/prospective owned-UUID union and other candidates. Dormant definitions are not discarded.
- The frozen `occurrence-id-v1` input is ASCII `eutheto/workforce/occurrence-id/v1`, one NUL byte, the template UUID's 16 network-order bytes, one date-text length byte, and canonical ASCII date text. Derivation date text uses four absolute-year digits (`YYYY-MM-DD` or `-YYYY-MM-DD`), independently of JSON date encoding. Output UUID bytes 0–5 retain the template prefix; bytes 6–15 take BLAKE3 digest bytes 0–9, with version 7 and RFC variant bits imposed. Golden vectors freeze the resulting 74 digest bits.
- A successful preview carries the exact prospective document hash and either no reconciliation or one compact `add_occurrence_identities` command. The host must check that hash against its target before applying the batch; the preview is not an authorization token. Diffs retain unresolved known prior occurrences without inventing instants, compare semantic metadata without treating set order or locale as shift changes, and expose added/changed/removed identities.
- Atomic add/remove occurrence commands retain one result and one ordinary inverse per command. Detach/reattach preserve exact raw ledger keys, child UUID spellings and stored-instance JSON. Dormant ownership transfers are intentional: detaching can activate an edited one-off, while reattaching can leave it inactive; generated descriptors require reviewing that consequence.
- Calendar queries retain whole intersecting windows. Regular reporting periods own civil dates from their anchor independently of clock boundaries. Custom periods own touched local dates, excluding a midnight end date; multiple reporting-date owners fail. Configured custom windows must remain ordered and nonoverlapping after resolution; gap movement is not repaired by sorting.
- Bounds remain 4,096 definitions per template, 32,768 document occurrences, 10,000 compound template targets, 42,768 resolved shifts, 32,768 calendar windows, and 85,536 shift/command changes. The command change accumulator also enforces a 64 MiB serialized array bound. Cancellation and checked arithmetic apply throughout operation-local loops.

WF-002 verification uses the Workforce crate's calendar, generation, occurrence-command and existing conformance regressions. A temporary real API smoke program additionally exercised 1,001 templates through one reconciliation command, strict resolution and exact inverse restoration. These are module-level checks, not application persistence, packaged desktop, solver or complete Phase-05 acceptance evidence.

### WF-003 assignment-rule module contract

The `eutheto_workforce::assignment_rules` contribution APIs (`analyze_assignments`, `compile_assignment_rules`, and `evaluate_assignment_rules`) provide bounded graph/readiness data, solver-neutral mathematics and independent original-domain rule summaries. These individual contributions do not construct a complete pack, project/accept a result, score, persist or route Workforce application commands. The completed `WorkforcePack` and application layer supply those separate responsibilities; every rule-completion gate below remains required.

- Person activity covers the whole shift unconditionally. Active scoped Eligibility enforces assignment-type membership and whole-shift qualification expressions; adjoining grants of one qualification may union, but gaps and alternating partial qualifications do not satisfy that qualification.
- Ordinary Unavailable/AvailableOnly records require a matching active Availability rule. Approved PTO remains an unconditional fact subject to its record restrictions; requested PTO is not hard. Instant and weekly windows **clip at effective boundaries**. AvailableOnly records are conjunctive, their own windows union, and an empty effective intersection is vacuous. Whole overnight tails, explicit DST policies and skipped-date lookback remain required.
- Coverage owners remain separate conjunctive requirements; total headcount counts a person once, while qualification minima count independently. Equivalent canonical minima deduplicate only within one owner; preferred counts are inert for hard mathematics. NoOverlap uses half-open instant intersection and rule-local symmetric category compatibility. Compatibility does not select workload-hour accounting: bucket `sum`/`union` semantics remain separate.
- The required-obligation universe is every active authored rule, every person activity fact, every approved-PTO fact and every Hard lock. Contributions return sorted disjoint handled/remaining partitions. Unsupported active families and Hard locks remain explicit; hard-lock readiness is not their enforcement. WF-004 adds minimum-rest contributions under the same boundary.
- The evaluator first rejects duplicate, missing/wrong-kind person and unresolved-shift selections. Identified but inadmissible selections remain in its original-domain population. Each handled binding yields one bounded summary with exact predicate/violation counts and one canonical first-failing witness at most.
- Bind source bytes using the existing `serde_json` document/BLAKE3 recipe. Derived planning identities use ASCII `eutheto/workforce/planning-id/v1`, NUL, fixed kind, NUL and canonical typed-key JSON, with full lowercase BLAKE3 hex. Stable pair associations and source-owned reachable provenance must survive canonical reordering; do not parse hashed planning IDs or infer pairs by vector position.
- Fixed limits are 1,000,000 inspected pairs, 20,000,000 work steps, 32,768 expanded availability intervals and 100,000 selected pairs. Caller IR limits and cumulative structural output limits apply; evaluator retention is capped at 100,000 records, 1,000,000 references/items and 16 MiB. Optional cancellation uses the real caller token; `None` remains finite-work bounded. Static limits are not elapsed deadlines, temporal review is not infeasibility, and errors never return truncated contributions.

See the [implemented contribution semantics](../../domains/workforce/core/README.md#assignment-rule-contributions). Core's legal cross-layer harness compares exhaustive tiny selections and compiler mutations, freezes mathematical identities, and runs real-worker feasible/infeasible cases through `just workforce-rule-worker-test` with required `EUTHETO_TEST_ORTOOLS_ARTIFACT`. These prove the contribution only, not registered-pack or complete Phase05 acceptance.

### WF-004 minimum-rest module contract

MinimumRest uses the existing stable rule schema, typed commands, editor/proposal metadata and portable-v1 shape without a version change. Common scope applies to both assignments; directional after/before scopes apply to source/target respectively. The same person must have two distinct shifts with source start at or before target start. Every such pair requires exact signed elapsed target-start minus source-end to meet the stored minute threshold. Equality passes, one nanosecond short fails, and overlap violates even zero-minute rest. Equal starts preserve both applicable role combinations regardless of UUID order. NoOverlap compatibility and intervening work never waive rest.

Compilation adds only violating `AtMostOne` pairs, with directional source/target identities and typed provenance; unary pruning remains unchanged. Independent evaluation consumes original selections, including inadmissible identified assignments, and counts all scoped pairs without scanning a quadratic safe suffix. Its canonical first witness retains source/target entities, required minutes and exact signed seconds/subsecond nanoseconds. Original resolved Hard locks receive scoped rest readiness findings, not enforcement or solver authority.

The [contribution documentation](../../domains/workforce/core/README.md#assignment-rule-contributions) gives a 600-minute call-to-clinic example and noncompliance disclaimer. Core's harness covers exhaustive boundary/direction selections, omitted/weak-threshold/reversed/nonadjacent/equal-start compiler defects, frozen directional identities, command inverses, portable semantics and real-worker feasible/infeasible rest models. Compiler/evaluator finite work, retention, cancellation and unchanged schema gates remain required. Complete CLI examples and product timing/quality benchmarks close under WF-008 after WF-007 registration; neither this module nor these tests close Phase05.

### WF-006 compiler and projection module contract

`compile_workforce` builds the complete supported five-family model, requiring an authored score policy and rejecting active later rules, preferences, Hard locks and Soft locks. Stable rank covers all source people and resolved in-horizon shifts before pruning (`p*S+s+1` in typed-ID order), not just feasible pairs; equal sums may tie. No-waiver NoOverlap uses maximal interval cliques, while category-waived rules retain pairwise conflicts. The [complete compiler contract](../../domains/workforce/core/README.md#complete-compiler-and-candidate-projection) fixes objective/category IDs, metadata keys, source-bound rejection diagnostics, hash behavior and cumulative operation budgets.

`project_workforce_candidate` reuses generic projection and decodes every result as a canonical typed person/shift pair, retaining required false decisions and rejecting malformed/missing/unknown values. It does not confer original-scenario correctness or acceptance. Real-worker rank/infeasibility and structural-verification checks run with the existing `just workforce-rule-worker-test` command. WF-007 supplies independent score/evidence and acceptance, and the application explicitly registers the complete pack. WF-008 complete CLI/fixtures/product benchmarks and every Phase05 exit gate remain separate requirements. The dense 100-person × 28-coincident-shift probe reaches the current work limit; no large-workload acceptance is claimed.

### WF-008 shared headless-service contract

`eutheto-core::HeadlessService` provides database-independent creation, checked portable decode/encode, typed exact-revision command application, full validation, immutable solve preparation/execution, fresh result verification and assignment explanation. It reuses the registered packs, planning IR, router and independent acceptance bridge; it is not a second optimizer or a new Tauri command. Full-readiness contradictions remain findings and do not prevent a supported model from reaching the backend for an actual infeasibility proof.

`SolveOperation` begins before input capture and retains one parent deadline through decoding, validation, compilation, dispatch, independent review and result preparation. The finite acceptance reserve is `min(1000ms, max(1ms, total/4))`; dispatch accounting subtracts it from the same observation used for the backend limit. Parent-observed spans retain full precision until final millisecond conversion, reject backwards/unrepresentable timing, and omit milestones or phases that did not occur.

Ordinary file and stored runs resolve Auto once, bind the selected runtime and effective Specific options into immutable `RunInputV1`, and never fall back across backends within that run. Generic router fallback remains a separately tested capability; [Phase 08](08-pumpkin-backend-and-router.md#deliberate-fallback-policy) owns the durable multi-attempt gate. New ordinary runs hash canonical objective-only policy with the `eutheto/planning-objective-policy/v1\0` domain; retained historical objective hashes remain opaque bindings.

`EuthetoApp::solve_stored` resolves the caller-stable request before reading moving-current state, allocating identities, resetting time or routing. Matching retries return retained running/terminal authority without redispatch; changed semantics conflict. Only the fresh owner finalizes. Known precommit rejection enters terminal cleanup; database/actor uncertainty remains explicit for recovery. Accepted results stay attached to their solved revision and are never automatically applied.

Authoritative run input, terminal manifest and retained result identity are read-only at the service boundary. Nonaccepted candidates cannot escape through execution reports. File output uses bounded prepared no-clobber publication under the original operation control. Database acceptance is the primary commit; secondary publication uses the reloaded stored portable result and preserves its identity on output failure.

Fresh verification recomputes original-domain scope, score and the full verification report. Imported backend proof, timings and arbitrary evidence-map text are not fresh authority; assignment explanations consume independently reproduced facts.

The focused `headless_scenario` and `headless_stored` core tests cover standalone closure, typed stale edits, readiness contradictions and durable retry/failure behavior. `headless_worker` explicitly requires `EUTHETO_TEST_ORTOOLS_ARTIFACT` to name a canonical approved installed artifact and runs with `cargo test --locked -p eutheto-core --test headless_worker -- --ignored --test-threads=1`; there is no helper fallback. These services do not complete WF-008's CLI parser, installed-worker host adapter, CSV codecs, required corpus or product benchmarks. The Phase05 exit gates and manual-testing pause before Phase06 remain open until their own evidence is complete.

## Rule-completion discipline

Every official rule or preference—including each initial Phase 05 rule—requires all of:

1. stable rule ID/schema and migration impact;
2. typed command DTO and command validation;
3. plain-language editor contract;
4. fast and full domain validation;
5. planning-IR compilation;
6. backend capability and translation tests;
7. independent verifier evaluation;
8. provenance and deterministic explanation;
9. generated AI-proposal metadata where appropriate, with strict schemas and invalid-input rejection; production assistant exposure remains Phase 10;
10. CLI/document-format example;
11. valid, invalid, boundary, empty-scope, infeasible, DST/time-boundary, and relevant import fixtures;
12. user semantics, examples, limitations, and noncompliance disclaimer where relevant;
13. benchmark/model-size review.

Additionally, Required/Preference meaning must be unambiguous; empty/default scopes safe; compiler and verifier agree under exhaustive small enumeration; import/export preserves the rule; result/explanation output is human-readable; and model impact is measured. A solver formulation alone is not a completed rule.

## Tests and acceptance

### Schema, command, migration, import, and time tests

- JSON Schema accepts all valid entity/rule shapes and rejects missing, unknown-newer, invalid reference, negative/bounds, and unsafe empty-scope cases;
- command/inverse restores canonical hash; batches are atomic/undoable; fixtures for genuine supported historical versions upgrade sequentially and round-trip without losing stable IDs/later catalog entries, subject to the initial-schema history policy above;
- workforce current portable export/import preserves every enabled Required rule, Preference, stable reference, time-zone/DST meaning and accepted-result identity; every supported historical migration is deterministic; unknown semantic rule/capability blocks while declared nonsemantic extensions preserve;
- initial workforce Share Result contribution rejects unaccepted/mismatched revisions and exposes only explicit recipient view fields, privacy flags, status/provenance and stable identities—not the full Scenario Model.
- people CSV tests cover supported encoding/header detection, explicit column mapping, exact external-ID updates, unresolved/similar-name review, additions/duplicates/rejections, proposed-state validation, bounded reports, cancellation, stale revision/input/mapping rejection, atomic failure, and one-step apply/undo/redo;
- template regeneration/detach diffs never overwrite manual changes silently;
- timezone tests cover normal days, nonexistent spring time, both ambiguous fall instants, overnight boundaries, reporting-day policy, effective qualification/availability edges, inclusive horizon end, wall/elapsed duration differences;
- property tests cover instant conversion, overlap symmetry, rest boundary equality, deterministic reorder/serialization, and no generated instance with a local start date outside the horizon; an overnight shift starting on the inclusive final date retains its entire next-day interval.

### Compiler/verifier tests

- exhaustive small eligibility/availability/coverage/no-overlap/rest matrices compare planning feasibility with independent rule evaluation;
- exact/min/max and qualification coverage; zero/one/multiple candidates; duplicate/overlapping requirements; explicit compatible overlap categories;
- rest exactly below/equal/above 600 minutes, both assignment orderings, scoped person/team/day/location/type filters, overnight and DST elapsed time;
- static variable pruning never loses rejection explanations;
- unsupported later rule fails compatibility rather than being ignored;
- deliberate compiler and verifier mutations are detected;
- feasible candidate verifies, deliberately invalid candidate quarantines, backend score cannot replace authoritative score.
- benchmark fixtures prove deterministic candidate pruning and model-size bounds, record the stable phase/quality metrics, and never equate raw first incumbent with first independently verified feasible.

### CLI and fixture acceptance

- clinic+overnight enforces cross-category 10-hour rest;
- insufficient coverage is proven infeasible and assumption/provenance evidence maps to workforce rule/entity IDs, or explicitly reports unavailable core without false certainty;
- DST fixture generates/validates correct instants and elapsed rest for spring/fall transitions;
- CLI creates/applies/validates/solves/verifies and exports an accepted solution without desktop/Tauri;
- JSON output stays on stdout, progress/diagnostics stay separate, stable exit codes distinguish validation/infeasible/limit/backend/verification/file/conflict/cancel states;
- no rejected or unverified solution file is written as accepted output;
- assignments JSON/CSV round-trip preserves stable person/shift identity.

### Phase exit gate

Phase 05 exits only when workforce schema/migrations/entities are versioned; initial five required-rule semantics are complete under the discipline; the workforce projection/verifier/score/evidence adapter passes the Phase 04 pack-integration contract; the clinic+overnight, infeasible coverage, and DST fixtures pass validation/compile/solve/project/independent-verify/explanation checks; the people CSV service proves bounded detect/map/preview/validate, explicit identity review, atomic apply, one-step undo/redo, and rejected-row retrieval without Vue/Tauri; CLI export is verified; and no UI dependency is needed. Unsupported remainder rules and the four remaining import formats must be explicitly reported, never ignored or partially enforced.

## Risks and failure handling

| Risk or failure | Required behavior |
|---|---|
| DST local time is ambiguous/nonexistent | Apply explicit scenario policy or require review; never infer silently. |
| Person name collision/import similarity | Match stable external ID or require explicit user mapping; names never become identity. |
| Static preprocessing removes a pair | Retain exact qualification/availability rejection provenance. |
| Obvious coverage impossible | Return actionable full-validation issue before solve; solving remains the authority for nonobvious infeasibility. |
| Compiler/verifier disagree | Quarantine candidate, fail run as correctness alarm, retain bounded diagnostics. |
| Later rule appears in Phase 05 document | Preserve it but report backend/phase unsupported before solve; never drop or treat it satisfied. |
| Rolling-window generation explodes | Enforce model limits and generate only membership-changing boundary windows when implemented. |
| Fairness harms unequal-contract staff | Use explicit workload weights/targets; never default to equal raw counts without user choice. |
| Hard lock is invalid/overlapping/rest-conflicting | Full validation identifies exact locks and rule evidence before solving. |
| Multiple valid schedules | Assert semantic invariants/score, not an arbitrary exact assignment. |
| Domain preset is mistaken for legal compliance | Show non-authoritative disclaimer and user responsibility; no certified-compliance claim. |

## Deferred and non-goals

- Phase 07 completes fixed/rolling hours, consecutive/count/skill-mix/locks/mutual/travel, the full preference/fairness/stability policy, repair, results, the four remaining import formats, and remaining exports.
- Desktop setup, people-import UI, and the rule builder are Phase 06; they consume the Phase 05 headless import backend. Desktop solving/results is Phase 07.
- Map-service travel integration, XLSX/payroll/vendor adapters, live calendar sync, encrypted/signed bundles, automatic backups, hosted sharing, and legal-compliance certification are not MVP requirements. Standalone HTML and direct PDF are Phase-07 MVP outputs under the shared privacy-filtered report contract.
- No universal rule-expression DSL, arbitrary custom code, dynamic native pack, or Tauri dependency in workforce core.
- Do not optimize/model unsupported rule types approximately; capability rejection is required.

## Assumption and version gates

Evidence date: **2026-08-29**.

- Review workforce defaults with real scheduling practitioners. Validate DST, rolling-hours, rest, reporting-day, qualification-slot, and repair semantics using domain examples.
- Maintainer semantic decisions (2026-09-04): qualifications must remain valid for the entire shift; qualification slots allow shared contribution with distinct total headcount; horizon membership uses the local shift-start date and retains complete overnight intervals. These decisions do not substitute for the broader practitioner/defaults and fairness review gates.
- Exact public fairness presets and weights require usability/practitioner evidence. Until then fixtures use explicit recorded policies rather than claiming universal fairness.
- The [Phase 07 practitioner scheduling clarifications](07-workforce-solving-results-repair-and-export.md#practitioner-scheduling-clarifications) track unresolved Required workload/count totals, distinct-weekend participation, new-work/time-off grouping, optional linked split sessions and synthetic combined acceptance evidence. Phase 05 hands these decisions forward without creating new schemas, implementing later rules, weakening unsupported-rule rejection, or claiming full practitioner-workflow support. Preserve all existing Phase 05 fixtures and exit gates; Phase 07 owns confirmed extensions and their complete compiler/verifier/UI evidence. Supplied identifying schedules never become repository data through initials-only replacement.
- OR-Tools **9.15** is used only after Phase 03 platform/build/benchmark/license/protobuf/assumption-core gates. Workforce infeasibility wording respects the known assumption-core issue gate.
- Rust stays **1.97.1** until a fixed stable newer than 1.98.0 resolves the known P-critical compiler issue.
- The final project name is `eutheto`; external format namespace is normalized accordingly. The working CLI name `optimizer` remains unresolved. `.eutheto` is the proposed bundle extension and must remain labelled pending until the Phase-11 identity ADR closes its media type/file-association commitment.
