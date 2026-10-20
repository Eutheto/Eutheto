// SPDX-License-Identifier: Apache-2.0
import type {
  RecipientAssignmentV1,
  RecipientEndpointV1,
  RecipientIntervalV1,
  RecipientReportV1,
  RecipientShiftV1,
} from "./api/generated-recipient-report";
import {
  MAX_RECIPIENT_PRESENTATION_COST,
  MAX_RECIPIENT_REPORT_BYTES,
  MAX_RECIPIENT_REPORT_ROWS,
  type RecipientLabelV1,
} from "./api/generated-recipient-report";

/** Admission precedes indexes, expanded strings and DOM construction, including native preview. */
function admitPresentation(report: RecipientReportV1): void {
  const canonical = JSON.stringify(report);
  if (canonical.length > MAX_RECIPIENT_REPORT_BYTES) throw new Error("Recipient byte limit.");
  const encoder = new TextEncoder();
  const bytes = encoder.encode(canonical).byteLength;
  if (bytes > MAX_RECIPIENT_REPORT_BYTES) throw new Error("Recipient byte limit.");
  const schedule = report.content.schedule;
  const privacy = report.privacy;
  if (
    (schedule.staffing !== null) !== privacy.includeStaffing ||
    (schedule.changes !== null) !== privacy.includeChanges ||
    (schedule.fairnessActuals !== null) !== privacy.includeFairnessActuals
  )
    throw new Error("Recipient disclosure differs from content.");
  let rows = 0;
  let maximumText = Math.max(
    64,
    encoder.encode(report.planVersion).byteLength,
    encoder.encode(report.timeZone).byteLength,
    encoder.encode(report.generatedAt).byteLength,
  );
  function charge(count: number): void {
    rows += count;
    if (rows > MAX_RECIPIENT_REPORT_ROWS) throw new Error("Recipient row limit.");
  }
  function label(value: RecipientLabelV1, allowed: boolean, referenced: boolean): void {
    if (!allowed && value.kind === "authored") throw new Error("Unapproved recipient label.");
    if (referenced) maximumText = Math.max(maximumText, encoder.encode(value.text).byteLength);
  }
  for (const values of [
    schedule.people,
    schedule.assignmentTypes,
    schedule.locations,
    schedule.shifts,
    schedule.assignments,
    schedule.staffing ?? [],
    schedule.changes ?? [],
    schedule.fairnessActuals ?? [],
  ])
    charge(values.length);
  for (const comparison of schedule.changes ?? []) charge(comparison.rows.length);
  for (const group of schedule.fairnessActuals ?? []) {
    charge(group.people.length + group.qualifyingShifts.length + group.periods.length);
    for (const period of group.periods) charge(period.actuals.length);
  }
  label(report.title, privacy.includeAuthoredLabels, false);
  for (const person of schedule.people) label(person.label, privacy.includePersonNames, true);
  for (const entity of schedule.assignmentTypes)
    label(entity.label, privacy.includeAuthoredLabels, true);
  for (const entity of schedule.locations) label(entity.label, privacy.includeAuthoredLabels, true);
  // Valid native temporal strings fit the 64-byte floor. Modified public data need not.
  for (const shift of schedule.shifts) {
    for (const text of [
      shift.interval.startsAt.actual,
      shift.interval.startsAt.resolved.local,
      shift.interval.endsAt.actual,
      shift.interval.endsAt.resolved.local,
      shift.reportingDate,
    ])
      maximumText = Math.max(maximumText, encoder.encode(text).byteLength);
  }
  for (const comparison of schedule.changes ?? []) {
    for (const row of comparison.rows) {
      if (row.previous?.availability !== "available") continue;
      const previous = row.previous.facts;
      label(previous.personLabel, privacy.includePersonNames, false);
      label(previous.assignmentType.label, privacy.includeAuthoredLabels, false);
      if (previous.location) label(previous.location.label, privacy.includeAuthoredLabels, false);
    }
  }
  // Same conservative census as native encoding: all views and print replacement,
  // not a promise about browser heap bytes. Reject whole reports; never truncate.
  const cost = 65_536 + 4 * bytes + (rows + 1) * (4096 + 16 * maximumText);
  if (cost > MAX_RECIPIENT_PRESENTATION_COST) throw new Error("Recipient presentation limit.");
}

/** Presentation only. Native capture owns disclosure, temporal meaning and acceptance. */
export function mountRecipientReport(
  root: HTMLElement,
  report: RecipientReportV1,
  printAction?: () => void,
): () => void {
  admitPresentation(report);
  const document = root.ownerDocument;
  const window = document.defaultView;
  const schedule = report.content.schedule;
  const people = new Map(schedule.people.map((person) => [person.personId, person]));
  const types = new Map(schedule.assignmentTypes.map((type) => [type.entityId, type]));
  const locations = new Map(schedule.locations.map((location) => [location.entityId, location]));
  const shifts = new Map(schedule.shifts.map((shift) => [shift.shiftId, shift]));
  const assignmentsByShift = new Map<string, RecipientAssignmentV1[]>();
  const assignmentsByPerson = new Map<string, RecipientAssignmentV1[]>();
  if (
    people.size !== schedule.people.length ||
    types.size !== schedule.assignmentTypes.length ||
    locations.size !== schedule.locations.length ||
    shifts.size !== schedule.shifts.length
  ) {
    throw new Error("Duplicate recipient identity.");
  }
  const usedTypes = new Set<string>();
  const usedLocations = new Set<string>();
  for (const shift of schedule.shifts) {
    required(types, shift.assignmentTypeId);
    usedTypes.add(shift.assignmentTypeId);
    if (shift.locationId !== null) {
      required(locations, shift.locationId);
      usedLocations.add(shift.locationId);
    }
  }
  if (usedTypes.size !== types.size || usedLocations.size !== locations.size)
    throw new Error("Unreferenced recipient work label.");
  const assignmentIds = new Set<string>();
  const currentPairs = new Set<string>();
  for (const assignment of schedule.assignments) {
    required(shifts, assignment.shiftId);
    required(people, assignment.personId);
    const pair = `${assignment.personId}/${assignment.shiftId}`;
    if (
      assignment.assignmentId.length === 0 ||
      assignmentIds.has(assignment.assignmentId) ||
      currentPairs.has(pair)
    )
      throw new Error("Duplicate or empty recipient assignment.");
    assignmentIds.add(assignment.assignmentId);
    currentPairs.add(pair);
    const rows = assignmentsByShift.get(assignment.shiftId) ?? [];
    rows.push(assignment);
    assignmentsByShift.set(assignment.shiftId, rows);
  }
  // Build each person's rows in the native work chronology, not assignment-ID order.
  for (const shift of schedule.shifts) {
    for (const assignment of assignmentsByShift.get(shift.shiftId) ?? []) {
      const personal = assignmentsByPerson.get(assignment.personId) ?? [];
      personal.push(assignment);
      assignmentsByPerson.set(assignment.personId, personal);
    }
  }
  if (schedule.staffing !== null) {
    const seen = new Set<string>();
    for (const row of schedule.staffing) {
      required(shifts, row.shiftId);
      if (
        seen.has(row.shiftId) ||
        row.assignedCount !== (assignmentsByShift.get(row.shiftId)?.length ?? 0)
      )
        throw new Error("Inconsistent recipient staffing.");
      seen.add(row.shiftId);
    }
    if (seen.size !== shifts.size) throw new Error("Incomplete recipient staffing.");
  }
  const maximumInteger = (1n << 127n) - 1n;
  for (const group of schedule.fairnessActuals ?? []) {
    if (
      (group.measure === "assignmentCount" && group.overlap === "union") ||
      (group.measure === "scheduledMinutes" &&
        (group.overlap === "union" || group.membership === "intersection"))
    )
      throw new Error("Unsupported recipient measurement.");
    const peers = new Set(group.people);
    const qualifying = new Set(group.qualifyingShifts);
    if (peers.size !== group.people.length || qualifying.size !== group.qualifyingShifts.length)
      throw new Error("Duplicate recipient measurement reference.");
    for (const id of peers) required(people, id);
    for (const id of qualifying) required(shifts, id);
    for (const period of group.periods) {
      if (
        period.actuals.length !== group.people.length ||
        period.actuals.some((actual, index) => actual.personId !== group.people[index])
      )
        throw new Error("Incomplete recipient measurement population.");
      for (const { actual } of period.actuals) {
        if (
          !/^(0|-?[1-9][0-9]{0,38})$/.test(actual.numerator) ||
          !/^[1-9][0-9]{0,38}$/.test(actual.denominator)
        )
          throw new Error("Invalid recipient exact value.");
        const numerator = BigInt(actual.numerator);
        const denominator = BigInt(actual.denominator);
        if (
          numerator < -maximumInteger - 1n ||
          numerator > maximumInteger ||
          denominator > maximumInteger ||
          (group.measure !== "scheduledMinutes" && numerator < 0n) ||
          (group.measure === "assignmentCount" && numerator % denominator !== 0n)
        )
          throw new Error("Recipient actual contradicts its measurement.");
      }
    }
  }
  const bases = new Set<string>();
  for (const comparison of schedule.changes ?? []) {
    if (
      bases.has(comparison.basis) ||
      (comparison.basis === "recordedOriginal") !== (comparison.original !== null)
    )
      throw new Error("Inconsistent recipient comparison source.");
    bases.add(comparison.basis);
    const pairs = new Set<string>();
    let currentCount = 0;
    for (const row of comparison.rows) {
      const pair = `${row.personId}/${row.shiftId}`;
      const hasCurrent = row.membership !== "removed";
      if (
        pairs.has(pair) ||
        currentPairs.has(pair) !== hasCurrent ||
        (row.previous !== null) === (row.membership === "added")
      )
        throw new Error("Inconsistent recipient comparison membership.");
      pairs.add(pair);
      if (hasCurrent) currentCount += 1;
    }
    if (currentCount !== currentPairs.size) throw new Error("Incomplete recipient comparison.");
  }
  if (
    (bases.has("recordedManual") && !bases.has("recordedOriginal")) ||
    (bases.has("externalStartingPlan") && bases.size !== 1)
  )
    throw new Error("Incompatible recipient comparison sources.");
  const state = { view: "timeline", day: "", person: "", location: "", search: "" };
  let printing = false;
  let optionalSectionsRendered = false;

  function node<K extends keyof HTMLElementTagNameMap>(
    tag: K,
    text?: string,
    className?: string,
  ): HTMLElementTagNameMap[K] {
    const element = document.createElement(tag);
    if (text !== undefined) element.textContent = text;
    if (className) element.className = className;
    return element;
  }
  function required<T>(index: ReadonlyMap<string, T>, id: string): T {
    const value = index.get(id);
    if (value === undefined) throw new Error("Invalid recipient reference.");
    return value;
  }
  function locationName(id: string | null): string {
    return id === null ? "No location" : required(locations, id).label.text;
  }
  function personName(id: string): string {
    return required(people, id).label.text;
  }
  function workName(shift: RecipientShiftV1): string {
    return required(types, shift.assignmentTypeId).label.text;
  }
  function offset(seconds: number): string {
    const absolute = Math.abs(seconds);
    const hours = String(Math.floor(absolute / 3600)).padStart(2, "0");
    const minutes = String(Math.floor((absolute % 3600) / 60)).padStart(2, "0");
    const remainder = absolute % 60;
    return `UTC${seconds < 0 ? "−" : "+"}${hours}:${minutes}${remainder ? `:${String(remainder).padStart(2, "0")}` : ""}`;
  }
  function endpoint(value: RecipientEndpointV1): string {
    const actual = `${value.actual.replace("T", " ")} (${offset(value.resolved.offsetSeconds)})`;
    return value.actual === value.resolved.local
      ? actual
      : `${actual}; adjusted from requested ${value.resolved.local.replace("T", " ")}`;
  }
  function interval(value: RecipientIntervalV1): string {
    return `${endpoint(value.startsAt)} → ${endpoint(value.endsAt)}`;
  }
  function table(caption: string, headings: string[], rows: Iterable<string[]>): HTMLTableElement {
    const table = node("table");
    table.append(node("caption", caption));
    const head = node("thead");
    const header = node("tr");
    for (const heading of headings) {
      const cell = node("th", heading);
      cell.scope = "col";
      header.append(cell);
    }
    head.append(header);
    const body = node("tbody");
    for (const values of rows) {
      const row = node("tr");
      for (const value of values) {
        const cell = node("td");
        const text = node("bdi", value);
        text.dir = "auto";
        cell.append(text);
        row.append(cell);
      }
      body.append(row);
    }
    table.append(head, body);
    return table;
  }
  function section(title: string): HTMLElement {
    const section = node("section", undefined, "recipient-section");
    section.append(node("h2", title));
    return section;
  }

  root.classList.add("recipient-report");
  const heading = node("header", undefined, "recipient-heading");
  heading.append(
    node("p", "Eutheto · Shared plan", "recipient-eyebrow"),
    node("h1", report.title.text),
  );
  heading.append(node("p", `${report.planVersion} · ${report.timeZone} · Verified feasible`));
  if (
    report.source.observedCurrentRevision !== null &&
    report.source.observedCurrentRevision !== report.source.scenarioRevision
  ) {
    heading.append(
      node(
        "p",
        `Older accepted plan: solved revision ${String(report.source.scenarioRevision)}; current revision was ${String(report.source.observedCurrentRevision)} when captured. This immutable report does not follow later edits.`,
        "recipient-warning",
      ),
    );
  }
  heading.append(
    node(
      "p",
      "Feasibility was independently verified for the captured source. This is not a claim of optimality, fairness or coverage satisfaction.",
    ),
  );

  const controls = node("form", undefined, "recipient-controls");
  controls.addEventListener("submit", (event) => {
    event.preventDefault();
  });
  function select(
    labelText: string,
    options: readonly (readonly [string, string])[],
    change: (value: string) => void,
  ): void {
    const label = node("label", labelText);
    const select = node("select");
    for (const [value, text] of options) {
      const option = node("option", text);
      option.value = value;
      select.append(option);
    }
    select.addEventListener("change", () => {
      change(select.value);
      render();
    });
    label.append(select);
    controls.append(label);
  }
  select(
    "Schedule view",
    [
      ["timeline", "Timeline"],
      ["person", "By Person"],
      ["location", "By Location / shift"],
    ],
    (value) => {
      state.view = value;
    },
  );
  select(
    "Actual start day",
    [
      ["", "All start days"],
      ...Array.from(
        new Set(schedule.shifts.map((shift) => shift.interval.startsAt.actual.split("T")[0] ?? "")),
        (date) => [date, date] as const,
      ),
    ],
    (value) => {
      state.day = value;
    },
  );
  select(
    "Person",
    [
      ["", "All people"],
      ...schedule.people.map((person) => [person.personId, person.label.text] as const),
    ],
    (value) => {
      state.person = value;
    },
  );
  select(
    "Location",
    [
      ["", "All locations"],
      ["none", "No location"],
      ...schedule.locations.map((location) => [location.entityId, location.label.text] as const),
    ],
    (value) => {
      state.location = value;
    },
  );
  const searchLabel = node("label", "Search schedule");
  const search = node("input");
  search.type = "search";
  search.autocomplete = "off";
  search.addEventListener("input", () => {
    state.search = search.value.toLocaleLowerCase();
    render();
  });
  searchLabel.append(search);
  controls.append(searchLabel);
  if (printAction) {
    const print = node("button", "Print complete plan");
    print.type = "button";
    print.addEventListener("click", printAction);
    controls.append(print);
  }
  controls.append(
    node(
      "p",
      "Filters affect only the schedule display, not the shared plan. Optional sections stay complete. Print includes the complete approved plan, ignoring these filters.",
      "recipient-filter-note",
    ),
  );
  const summary = node("p", undefined, "recipient-scope");
  summary.setAttribute("role", "status");
  summary.setAttribute("aria-live", "polite");
  const body = node("div", undefined, "recipient-content");
  const scheduleBody = node("div");
  body.append(scheduleBody);

  const privacy = section("Privacy and disclosure");
  privacy.append(
    node(
      "p",
      "Stable identifiers and work patterns remain linkable. Generated aliases are pseudonyms, not anonymity.",
    ),
  );
  privacy.append(
    table(
      "Included and excluded fields",
      ["Field", "Disclosure"],
      [
        [
          "Person names",
          report.privacy.includePersonNames ? "Included" : "Excluded; generated aliases",
        ],
        [
          "Authored plan, work and location labels",
          report.privacy.includeAuthoredLabels
            ? "Included; labels may themselves contain private information"
            : "Excluded; generated labels",
        ],
        [
          "Staffing counts",
          report.privacy.includeStaffing
            ? "Included; actuals only, not a coverage assessment"
            : "Excluded",
        ],
        ["Historical changes", report.privacy.includeChanges ? "Included" : "Excluded"],
        [
          "Configured workload actuals",
          report.privacy.includeFairnessActuals
            ? "Included; full measurement populations and qualifying work, not an equity assessment"
            : "Excluded",
        ],
        ["Full addresses, source calendar titles and private notes", "Excluded"],
        ["Constraint details, rejected alternatives and diagnostics", "Excluded"],
        ["Objective details and preference assessments", "Excluded"],
        ["Transportation", "Not available in this report"],
        ["External links", "Excluded"],
      ],
    ),
  );
  const about = section("About this plan");
  about.append(
    table(
      "Immutable source and presentation",
      ["Field", "Value"],
      [
        ["Plan version", report.planVersion],
        ["Scenario", report.source.scenarioId],
        ["Solved revision", String(report.source.scenarioRevision)],
        ["Result", report.source.solutionId],
        [
          "Current revision at capture",
          report.source.observedCurrentRevision === null
            ? "Not established for supplied file"
            : String(report.source.observedCurrentRevision),
        ],
        ["Source", report.source.origin],
        ["Scenario horizon", `${report.horizon.start} → ${report.horizon.end}`],
        ["Time zone", report.timeZone],
        ["Source locale", report.locale],
        ["Generated", report.generatedAt],
        ["Application", report.applicationVersion],
        ["Report schema", String(report.schemaVersion)],
        ["Result schema", String(report.source.resultSchemaVersion)],
        ["Renderer", String(report.renderer.version)],
        [
          "Recorded solver",
          `${report.source.solver.backendId} · ${report.source.solver.recordedVersion ?? "version unavailable"}`,
        ],
        ["Accepted-result checksum", report.source.acceptedResultChecksum],
        ["Verification checksum", report.source.verificationReportChecksum],
        ["Source-document hash", report.source.sourceDocumentHash],
        ["Renderer JavaScript SHA-256", report.renderer.javascriptSha256],
        ["Renderer CSS SHA-256", report.renderer.cssSha256],
      ],
    ),
  );
  about.append(
    node(
      "p",
      "Public hashes and recorded solver metadata are descriptive, not independent authentication of an imported report. Intervals are half-open: their end instant is excluded. Actual civil times and offsets are captured natively; reporting dates may use a different authored attribution.",
    ),
  );
  const printContext = node(
    "p",
    `${report.planVersion} · ${report.timeZone} · Verified feasible · Generated ${report.generatedAt}`,
    "recipient-print-context",
  );
  const printLayout = node("div", undefined, "recipient-print-layout");
  const printContent = node("div", undefined, "recipient-print-content");
  printContent.append(heading, controls, summary, body, privacy, about);
  printLayout.append(printContext, printContent);
  root.replaceChildren(printLayout);

  function visible(shift: RecipientShiftV1): boolean {
    if (printing) return true;
    if (state.day && !shift.interval.startsAt.actual.startsWith(`${state.day}T`)) return false;
    if (state.location && (shift.locationId ?? "none") !== state.location) return false;
    const assignments = assignmentsByShift.get(shift.shiftId) ?? [];
    if (state.person && !assignments.some((assignment) => assignment.personId === state.person))
      return false;
    return (
      !state.search ||
      [
        workName(shift),
        locationName(shift.locationId),
        ...assignments.map((assignment) => personName(assignment.personId)),
      ].some((text) => text.toLocaleLowerCase().includes(state.search))
    );
  }
  function render(): void {
    const work = schedule.shifts.filter(visible);
    const visibleIds = new Set(work.map((shift) => shift.shiftId));
    summary.textContent = printing
      ? "Complete approved plan"
      : `${String(work.length)} of ${String(schedule.shifts.length)} included work instances shown. Start-day filtering retains each instance's full interval, including overnight tails.`;
    const main = section(
      printing || state.view === "timeline"
        ? "Timeline"
        : state.view === "person"
          ? "By Person"
          : "By Location / shift",
    );
    if (!work.length)
      main.append(
        node(
          "p",
          schedule.shifts.length
            ? "No work matches these schedule filters."
            : "This accepted plan has no included work instances.",
        ),
      );
    if (!printing && state.view === "person") {
      for (const person of schedule.people) {
        if (state.person && state.person !== person.personId) continue;
        const assignments = (assignmentsByPerson.get(person.personId) ?? []).filter((assignment) =>
          visibleIds.has(assignment.shiftId),
        );
        if (!assignments.length && (state.search || state.day || state.location)) continue;
        const personWork = assignments.map((assignment) => required(shifts, assignment.shiftId));
        main.append(
          table(
            person.label.text,
            ["Work", "Actual interval", "Location", "Reporting date"],
            personWork.map((shift) => [
              workName(shift),
              interval(shift.interval),
              locationName(shift.locationId),
              shift.reportingDate,
            ]),
          ),
        );
        if (!assignments.length) main.append(node("p", "No assignments in this display scope."));
      }
    } else if (!printing && state.view === "location") {
      const byLocation = new Map<string | null, RecipientShiftV1[]>();
      for (const shift of work) {
        const located = byLocation.get(shift.locationId) ?? [];
        located.push(shift);
        byLocation.set(shift.locationId, located);
      }
      for (const location of [null, ...schedule.locations]) {
        const located = byLocation.get(location?.entityId ?? null) ?? [];
        if (located.length)
          main.append(
            table(
              location?.label.text ?? "No location",
              ["Work", "Actual interval", "Location", "Assigned people", "Reporting date"],
              located.map((shift) => [
                workName(shift),
                interval(shift.interval),
                locationName(shift.locationId),
                (assignmentsByShift.get(shift.shiftId) ?? [])
                  .map((assignment) => personName(assignment.personId))
                  .join("; ") || "No assignments",
                shift.reportingDate,
              ]),
            ),
          );
      }
    } else {
      const days = new Map<string, RecipientShiftV1[]>();
      for (const shift of work) {
        const day = shift.interval.startsAt.actual.split("T")[0] ?? "";
        const daily = days.get(day) ?? [];
        daily.push(shift);
        days.set(day, daily);
      }
      const timeline = node("ol", undefined, "recipient-timeline");
      for (const [day, daily] of days) {
        const item = node("li");
        item.append(
          table(
            `${day} · ${report.planVersion} · Verified feasible`,
            ["Work", "Actual interval", "Location", "Assigned people", "Reporting date"],
            daily.map((shift) => [
              workName(shift),
              interval(shift.interval),
              locationName(shift.locationId),
              (assignmentsByShift.get(shift.shiftId) ?? [])
                .map((assignment) => personName(assignment.personId))
                .join("; ") || "No assignments",
              shift.reportingDate,
            ]),
          ),
        );
        timeline.append(item);
      }
      main.append(timeline);
    }
    scheduleBody.replaceChildren(main);
    if (optionalSectionsRendered) return;
    const sections: HTMLElement[] = [];
    if (schedule.staffing !== null) {
      const staffing = section("Staffing actuals");
      staffing.append(
        node(
          "p",
          "Assigned headcounts only. No required thresholds, qualification conditions or coverage satisfaction are disclosed.",
        ),
      );
      staffing.append(
        table(
          "Complete included work census",
          ["Work", "Actual interval", "Assigned people"],
          schedule.staffing.map((row) => {
            const shift = required(shifts, row.shiftId);
            return [workName(shift), interval(shift.interval), String(row.assignedCount)];
          }),
        ),
      );
      sections.push(staffing);
    }
    if (schedule.fairnessActuals !== null) {
      const fairness = section("Configured workload actuals");
      fairness.append(
        node(
          "p",
          "Actual quantities are not an equity assessment. Targets, deviations, weights and preference penalties are excluded. Every measurement keeps its full population, including zeros.",
        ),
      );
      schedule.fairnessActuals.forEach((group, index) => {
        const measure = {
          assignmentCount: "Assignments",
          elapsedMinutes: "Elapsed minutes",
          scheduledMinutes: "Scheduled minutes",
        }[group.measure];
        const membership = {
          reportingDate: "Reporting date",
          startInstant: "Start instant",
          intersection: "Interval intersection",
        }[group.membership];
        fairness.append(node("h3", `Measurement ${String(index + 1)}: ${measure}`));
        fairness.append(
          node(
            "p",
            `${membership}; ${group.overlap === "sum" ? "overlapping contributions counted separately" : "elapsed union of overlapping contributions"}. Population: ${group.people.map(personName).join("; ") || "Empty"}.`,
          ),
        );
        fairness.append(
          table(
            "Qualifying work",
            ["Work", "Actual interval", "Location", "Reporting date"],
            group.qualifyingShifts.map((id) => {
              const shift = required(shifts, id);
              return [
                workName(shift),
                interval(shift.interval),
                locationName(shift.locationId),
                shift.reportingDate,
              ];
            }),
          ),
        );
        if (!group.periods.length)
          fairness.append(node("p", "No measurement periods in this captured context."));
        for (const period of group.periods) {
          fairness.append(
            table(
              interval(period.interval),
              ["Person", measure],
              period.actuals.map((actual) => [
                personName(actual.personId),
                actual.actual.denominator === "1"
                  ? actual.actual.numerator
                  : `${actual.actual.numerator}/${actual.actual.denominator}`,
              ]),
            ),
          );
        }
      });
      if (!schedule.fairnessActuals.length)
        fairness.append(node("p", "No active configured measurements."));
      sections.push(fairness);
    }
    if (schedule.changes !== null) {
      const changes = section("Recorded changes");
      changes.append(
        node(
          "p",
          "Pair membership only: retaining a person/work pair does not establish unchanged times, labels or policy. Recorded manual and external starting plans are not authenticated accepted ancestors.",
        ),
      );
      for (const comparison of schedule.changes) {
        const title = {
          recordedOriginal: "Recorded original → current plan",
          recordedManual: "Recorded manual selection → current plan",
          externalStartingPlan: "External starting plan → current plan",
        }[comparison.basis];
        changes.append(node("h3", title));
        if (comparison.original)
          changes.append(
            node(
              "p",
              `Original ${comparison.original.solutionId}; scenario ${comparison.original.scenarioId}, revision ${String(comparison.original.scenarioRevision)}; ${comparison.original.admission === "freshlyReadmittedNative" ? "freshly re-admitted natively at capture" : "native acceptance not established"}; checksum ${comparison.original.acceptedResultChecksum}.`,
            ),
          );
        changes.append(
          table(
            title,
            ["Membership", "Person identity", "Work identity", "Previous facts", "Current facts"],
            comparison.rows.map((row) => {
              const previous =
                row.previous?.availability === "available"
                  ? `${row.previous.facts.personLabel.text}; ${row.previous.facts.assignmentType.label.text}; ${row.previous.facts.location?.label.text ?? "No location"}; ${interval(row.previous.facts.interval)}; reporting date ${row.previous.facts.reportingDate}`
                  : row.previous
                    ? "Historical display facts unavailable"
                    : "Not in starting pairs";
              let current = "Not in current pairs";
              if (row.membership !== "removed") {
                const shift = required(shifts, row.shiftId);
                current = `${personName(row.personId)}; ${workName(shift)}; ${locationName(shift.locationId)}; ${interval(shift.interval)}; reporting date ${shift.reportingDate}`;
              }
              return [row.membership, row.personId, row.shiftId, previous, current];
            }),
          ),
        );
      }
      if (!schedule.changes.length)
        changes.append(node("p", "No recorded starting-plan comparison in this source."));
      sections.push(changes);
    }
    body.append(...sections);
    optionalSectionsRendered = true;
  }
  const beforePrint = (): void => {
    printing = true;
    render();
  };
  const afterPrint = (): void => {
    printing = false;
    render();
  };
  render();
  window?.addEventListener("beforeprint", beforePrint);
  window?.addEventListener("afterprint", afterPrint);
  root.dataset.recipientReady = "true";
  return () => {
    window?.removeEventListener("beforeprint", beforePrint);
    window?.removeEventListener("afterprint", afterPrint);
    delete root.dataset.recipientReady;
    root.replaceChildren();
    root.classList.remove("recipient-report");
  };
}
