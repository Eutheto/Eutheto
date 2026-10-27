import { reactive } from "vue";
import { cleanup, render, screen, within } from "@testing-library/vue";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { userEvent } from "vitest/browser";
import * as api from "../api/generated";
import type {
  ApiResponseDto,
  SolutionSummaryDtoV1,
  SolutionVerificationDtoV1,
  SolutionExplanationDtoV2,
  SolutionViewDtoV1,
} from "../api/generated";
import { fakeApi, portableOperation } from "../testing/project-home";
import { WORKFORCE_RESULT_SCHEDULE_VIEW_ID } from "../api/generated-domain-pack-contracts";
import { messages } from "../messages";
import AcceptedSchedule from "./AcceptedSchedule.vue";
import "../styles.css";

vi.mock("../api/generated", { spy: true });

const scenarioId = "018f7b40-a000-7000-8000-000000000100";
const solutionId = "018f7b40-a000-7000-8000-000000000101";
const shiftId = "018f7b40-a000-7000-8000-000000000102";
const resultChecksum = "a".repeat(64);
const copy = messages.acceptedSchedule;
const personId = (index: number) =>
  `018f7b40-a000-7000-8000-${index.toString(16).padStart(12, "0")}`;
const assignmentId = `official.workforce.assignment.${personId(51)}.${shiftId}`;
const cursor = {
  schemaVersion: 1 as const,
  scenarioId,
  revision: 3,
  queryFingerprint: Array(32).fill(1) as number[],
  position: { kind: "person" as const, personId: personId(50) },
};
const summary: SolutionSummaryDtoV1 = {
  schemaVersion: 1,
  result: { solutionId, resultChecksum },
  scenarioId,
  scenarioRevision: 3,
  currentRevision: 3,
  runId: "018f7b40-a000-7000-8000-000000000103",
  stale: false,
  selected: true,
  status: "feasible",
  score: { feasibility: "0", levels: [] },
  verificationChecksum: "b".repeat(64),
  finishedAt: "2026-11-01T12:00:00Z",
};
function response(data: object): ApiResponseDto<SolutionViewDtoV1> {
  return {
    schemaVersion: 1,
    requestId: "018f7b40-a000-7000-8000-000000000104",
    currentRevision: 3,
    warnings: [],
    result: {
      schemaVersion: 1,
      scenarioId,
      currentRevision: 3,
      scenarioRevision: 3,
      result: summary.result,
      view: {
        viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
        data: { ...data, timeZone: "America/New_York" },
      },
    },
  };
}
function verifiedResponse(
  currentRevision = 3,
  checksum = summary.verificationChecksum,
  preferencePenalty = "5",
): ApiResponseDto<SolutionVerificationDtoV1> {
  return {
    schemaVersion: 1,
    requestId: personId(105),
    currentRevision,
    warnings: [],
    result: {
      schemaVersion: 1,
      scenarioId,
      currentRevision,
      scenarioRevision: summary.scenarioRevision,
      result: summary.result,
      verification: {
        schemaVersion: 2,
        accepted: true,
        scenarioId,
        evaluatedRevision: summary.scenarioRevision,
        documentHash: "c".repeat(64),
        planningModelHash: "d".repeat(64),
        normalizedSolutionHash: "e".repeat(64),
        verificationScopeChecksum: "f".repeat(64),
        requiredRuleResults: [
          {
            ruleId: personId(9),
            satisfied: true,
            affectedEntities: [],
            messageKey: "official.workforce.rule.no_overlap.passed",
            expected: {},
            observed: {},
            evidence: [],
          },
        ],
        score: {
          feasibility: "0",
          levels: [
            {
              levelId: "official.workforce.objective.preference.high",
              value: preferencePenalty,
              direction: "minimize",
              categoryBreakdown:
                preferencePenalty === "0"
                  ? {}
                  : { "official.workforce.score.preference.time": preferencePenalty },
            },
          ],
        },
        warnings: [],
        metrics: {},
        checksum,
      },
    },
  };
}
function explainedResponse(
  currentRevision = 3,
  explainedAssignment = assignmentId,
  messageCount = 1,
): ApiResponseDto<SolutionExplanationDtoV2> {
  return {
    schemaVersion: 1,
    requestId: personId(105),
    currentRevision,
    warnings: [],
    result: {
      schemaVersion: 2,
      scenarioId,
      currentRevision,
      scenarioRevisions: [summary.scenarioRevision],
      explanation: {
        schemaVersion: 2,
        evidence: {
          schemaVersion: 2,
          certainty: "deterministic",
          evidence: {
            kind: "assignment",
            assignment: {
              assignment: {
                id: explainedAssignment,
                entity: { kind: "official.workforce.assignment", id: explainedAssignment },
                value: { type: "boolean", value: true },
                evidence: [],
              },
              relatedRules: [],
              scoreContributions: [],
              metrics: {},
              lockState: null,
            },
          },
          checksum: "d".repeat(64),
        },
        rendered: {
          schemaVersion: 1,
          kind: "assignment",
          evidenceChecksum: "d".repeat(64),
          messages: Array.from({ length: messageCount }, (_, index) => ({
            messageKey:
              index === 0
                ? "official.workforce.assignment.recorded_decision"
                : "official.workforce.evaluation.summary",
            parameters:
              index === 0
                ? {
                    "official.workforce.fact.summary": {
                      type: "text",
                      value: "Recorded selection; only pair-local checks are recomputed.",
                    },
                    "official.workforce.fact.selected": { type: "boolean", value: true },
                    "official.workforce.fact.checked_predicates": { type: "integer", value: "2" },
                  }
                : {},
            entities: [],
            rules: [personId(index + 9)],
            assignments: [explainedAssignment],
            evidence: [],
          })),
        },
        checksum: "e".repeat(64),
      },
    },
  };
}

beforeEach(() => {
  fakeApi();
  vi.clearAllMocks();
  vi.mocked(api.explainSolution).mockReset();
  vi.mocked(api.explainSolution).mockReturnValue(portableOperation(explainedResponse()));
  vi.mocked(api.verifySolution).mockResolvedValue(verifiedResponse());
  vi.mocked(api.getSolutionView).mockImplementation(({ parameters, continuation }) => {
    const kind = parameters?.kind ?? "people";
    if (kind === "calendarPeople") {
      const start = parameters?.startDate ?? "2026-11-01";
      const firstDay = start === "2026-11-08" ? 8 : 1;
      const dates = Array.from(
        { length: 7 },
        (_, index) => `2026-11-${String(firstDay + index).padStart(2, "0")}`,
      );
      return Promise.resolve(
        response({
          schemaVersion: 1,
          result: {
            kind,
            planningDates: { startDate: "2026-11-01", endDateExclusive: "2026-11-16" },
            window: {
              startDate: start,
              endDateExclusive: `2026-11-${String(firstDay + 7).padStart(2, "0")}`,
            },
            totalItems: 32,
            continuation: null,
            items: Array.from({ length: 32 }, (_, index) => ({
              person: {
                personId: personId(index + 51),
                name: `Person ${String(index + 51)}`,
                selectedCount: index === 0 ? 1 : 0,
              },
              days: dates.map((date, day) => ({
                date,
                selectedCount: index === 0 && day === 0 && firstDay === 1 ? 1 : 0,
                selectedShifts:
                  index === 0 && day === 0 && firstDay === 1
                    ? [
                        {
                          assignmentId,
                          shiftId,
                          startsAt: {
                            instant: "2026-11-01T05:30:00Z",
                            local: "2026-11-01T01:30:00",
                            offsetSeconds: -14400,
                          },
                          endsAt: {
                            instant: "2026-11-01T07:30:00Z",
                            local: "2026-11-01T02:30:00",
                            offsetSeconds: -18000,
                          },
                        },
                      ]
                    : [],
              })),
            })),
          },
        }),
      );
    }
    if (kind === "calendarShifts") {
      const start = parameters?.startDate ?? "2026-11-01";
      const row = (id: string, locationId: string | null, locationName: string | null) => ({
        date: "2026-11-01",
        shift: {
          shiftId: id,
          assignmentTypeId: personId(200),
          assignmentTypeName: "Clinic",
          locationId,
          locationName,
          startsAt: {
            instant: "2026-11-01T05:30:00Z",
            local: "2026-11-01T01:30:00",
            offsetSeconds: -14400,
          },
          endsAt: {
            instant: "2026-11-01T07:30:00Z",
            local: "2026-11-01T02:30:00",
            offsetSeconds: -18000,
          },
          required: 1,
          minimum: 1,
          preferred: locationId === null ? null : 2,
          maximum: 2,
          selectedCount: locationId === null ? 0 : 1,
          margin: locationId === null ? -1 : 0,
        },
        selectedPersonIds: locationId === null ? [] : [personId(51)],
      });
      const duplicate = row(personId(204), personId(33), "North Clinic");
      duplicate.shift.startsAt = {
        instant: "2026-11-01T06:30:00Z",
        local: "2026-11-01T01:30:00",
        offsetSeconds: -18000,
      };
      duplicate.shift.selectedCount = 0;
      duplicate.shift.margin = -1;
      duplicate.selectedPersonIds = [];
      const items =
        start === "2026-11-01"
          ? parameters?.locationId === personId(33)
            ? [row(shiftId, personId(33), "North Clinic"), duplicate]
            : [
                row(shiftId, personId(33), "North Clinic"),
                row(personId(203), null, null),
                duplicate,
              ]
          : [];
      return Promise.resolve(
        response({
          schemaVersion: 1,
          result: {
            kind,
            planningDates: { startDate: "2026-11-01", endDateExclusive: "2026-11-16" },
            window: {
              startDate: start,
              endDateExclusive: start === "2026-11-08" ? "2026-11-15" : "2026-11-08",
            },
            totalItems: items.length,
            continuation: null,
            items,
          },
        }),
      );
    }
    if (kind === "fairness") {
      const row = (
        id: number,
        name: string,
        actual: string,
        target: string,
        deviation: string,
      ) => ({
        scorePolicyId: personId(9),
        policyId: personId(10),
        preferenceId: personId(11),
        bucketId: personId(3),
        bucketName: "Clinical minutes",
        calendarId: personId(2),
        membership: "reportingDate",
        targetMode: "weightedEqualShare",
        measure: "elapsedMinutes",
        units: "minutes",
        preferencePriority: "normal",
        preferenceWeight: 10,
        periodStartsAt: {
          instant: "2026-11-01T04:00:00Z",
          local: "2026-11-01T00:00:00",
          offsetSeconds: -14400,
        },
        periodEndsAt: {
          instant: "2026-11-02T05:00:00Z",
          local: "2026-11-02T00:00:00",
          offsetSeconds: -18000,
        },
        peerGroupSize: 2,
        personId: personId(id),
        personName: name,
        target: { numerator: target.split("/")[0], denominator: target.split("/")[1] ?? "1" },
        actual: { numerator: actual.split("/")[0], denominator: actual.split("/")[1] ?? "1" },
        deviation: {
          numerator: deviation.split("/")[0],
          denominator: deviation.split("/")[1] ?? "1",
        },
        groupActualMinimum: { numerator: "1", denominator: "1" },
        groupActualMaximum: { numerator: "5", denominator: "2" },
        groupSpread: { numerator: "3", denominator: "2" },
        groupDeviationMinimum: { numerator: "-1", denominator: "2" },
        groupDeviationMaximum: { numerator: "1", denominator: "1" },
        groupDeviationSpread: { numerator: "3", denominator: "2" },
      });
      return Promise.resolve(
        response({
          schemaVersion: 1,
          result: {
            kind,
            totalItems: 2,
            continuation: null,
            items: [row(1, "River", "1", "3/2", "-1/2"), row(20, "Oak", "5/2", "3/2", "1")],
          },
        }),
      );
    }
    if (kind === "health") {
      const lower = {
        check: "minimumRest",
        status: "satisfied",
        units: "minutes",
        direction: "minimum",
        sourceRuleId: personId(9),
        coverageOwnerId: null,
        coverageOwnerKind: null,
        personId: personId(51),
        shiftId,
        otherShiftId: personId(203),
        windowStartsAt: null,
        windowEndsAt: null,
        minimumRank: null,
        threshold: { numerator: "100", denominator: "1" },
        observed: { numerator: "201", denominator: "2" },
        margin: { numerator: "1", denominator: "2" },
        affectedAssignmentIds: [assignmentId],
        affectedAssignmentCount: 1,
        affectedAssignmentsTruncated: false,
      };
      return Promise.resolve(
        response({
          schemaVersion: 1,
          result: {
            kind,
            totalItems: 3,
            continuation: null,
            items: [
              lower,
              {
                ...lower,
                check: "coverageHeadcount",
                status: "satisfied",
                units: "people",
                direction: "maximum",
                sourceRuleId: null,
                personId: null,
                coverageOwnerId: shiftId,
                coverageOwnerKind: "shiftInstance",
                otherShiftId: null,
                windowStartsAt: "2026-11-01T13:00:00Z",
                windowEndsAt: "2026-11-01T14:00:00Z",
                threshold: { numerator: "2", denominator: "1" },
                observed: { numerator: "1", denominator: "1" },
                margin: { numerator: "1", denominator: "1" },
              },
              {
                ...lower,
                check: "coverageHeadcount",
                status: "noApplicable",
                units: "people",
                sourceRuleId: personId(11),
                personId: null,
                shiftId: null,
                otherShiftId: null,
                threshold: null,
                observed: null,
                margin: null,
                affectedAssignmentIds: [],
                affectedAssignmentCount: 0,
              },
            ],
          },
        }),
      );
    }
    if (kind === "shifts") {
      const atClinic = parameters?.locationId === personId(33);
      const row = (id: string, locationId: string | null, locationName: string | null) => ({
        shiftId: id,
        assignmentTypeId: personId(200),
        assignmentTypeName: "Clinic",
        locationId,
        locationName,
        startsAt: {
          instant: "2026-11-01T13:00:00Z",
          local: "2026-11-01T08:00:00",
          offsetSeconds: -18000,
        },
        endsAt: {
          instant: "2026-11-01T14:00:00Z",
          local: "2026-11-01T09:00:00",
          offsetSeconds: -18000,
        },
        required: null,
        minimum: 1,
        preferred: locationId === null ? null : 2,
        maximum: 2,
        selectedCount: locationId === null ? 0 : 1,
        margin: locationId === null ? -1 : 0,
      });
      return Promise.resolve(
        response({
          schemaVersion: 1,
          result: {
            kind,
            totalItems: atClinic ? 1 : 2,
            continuation: null,
            items: atClinic
              ? [row(shiftId, personId(33), "North Clinic")]
              : [row(shiftId, personId(33), "North Clinic"), row(personId(203), null, null)],
          },
        }),
      );
    }
    if (kind === "assignments" || kind === "timeline") {
      return Promise.resolve(
        response({
          schemaVersion: 1,
          result: {
            kind,
            totalItems: 1,
            continuation: null,
            items: [
              {
                assignmentId,
                personId: personId(51),
                personName: "Person 51",
                shiftId,
                assignmentTypeId: personId(200),
                assignmentTypeName: "Clinic",
                locationId: null,
                locationName: null,
                startsAt: {
                  instant: "2026-11-01T13:00:00Z",
                  local: "2026-11-01T08:00:00",
                  offsetSeconds: -18000,
                },
                endsAt: {
                  instant: "2026-11-01T14:00:00Z",
                  local: "2026-11-01T09:00:00",
                  offsetSeconds: -18000,
                },
              },
            ],
          },
        }),
      );
    }
    const items =
      continuation === null
        ? Array.from({ length: 50 }, (_, index) => ({
            personId: personId(index + 1),
            name: `Person ${String(index + 1)}`,
            selectedCount: 0,
          }))
        : [{ personId: personId(51), name: "Person 51", selectedCount: 1 }];
    return Promise.resolve(
      response({
        schemaVersion: 1,
        result: {
          kind: "people",
          totalItems: 51,
          items,
          continuation: continuation === null ? cursor : null,
        },
      }),
    );
  });
});
it("navigates original seven-day calendar windows and keyboard cells without rendering the whole native page", async () => {
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  await userEvent.click(screen.getByLabelText(copy.kinds.calendarPeople));
  const grid = await screen.findByRole("grid", { name: copy.kinds.calendarPeople });
  expect(screen.getByText(copy.calendar.originalDates("2026-11-01", "2026-11-16"))).toBeVisible();
  expect(screen.getByText(copy.calendar.shownDates("2026-11-01", "2026-11-08"))).toBeVisible();
  expect(within(grid).getAllByRole("row").length).toBeLessThan(33);
  const first = within(grid).getByRole("gridcell", {
    name: copy.calendar.dayLabel("Person 51", "2026-11-01", 1),
  });
  first.focus();
  await userEvent.keyboard("{ArrowRight}");
  expect(
    within(grid).getByRole("gridcell", {
      name: copy.calendar.dayLabel("Person 51", "2026-11-02", 0),
    }),
  ).toHaveFocus();
  await userEvent.keyboard("{End}");
  expect(
    within(grid).getByRole("gridcell", {
      name: copy.calendar.dayLabel("Person 51", "2026-11-07", 0),
    }),
  ).toHaveFocus();
  expect(screen.getByRole("region", { name: copy.calendar.region }).scrollLeft).toBeGreaterThan(0);
  await userEvent.click(screen.getByRole("button", { name: copy.calendar.paged }));
  expect(within(grid).getAllByRole("row")).toHaveLength(9);
  expect(
    within(grid).getByRole("gridcell", {
      name: copy.calendar.dayLabel("Person 51", "2026-11-01", 1),
    }),
  ).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: copy.calendar.nextRows }));
  expect(within(grid).getAllByRole("row")).toHaveLength(9);
  expect(
    within(grid).getByRole("gridcell", {
      name: copy.calendar.dayLabel("Person 59", "2026-11-01", 0),
    }),
  ).toHaveAttribute("tabindex", "0");
  await userEvent.click(screen.getByRole("button", { name: copy.calendar.previousRows }));
  await userEvent.click(screen.getByRole("button", { name: copy.calendar.nextWeek }));
  expect(
    await screen.findByText(copy.calendar.shownDates("2026-11-08", "2026-11-15")),
  ).toBeVisible();
  expect(api.getSolutionView).toHaveBeenLastCalledWith(
    expect.objectContaining({
      parameters: expect.objectContaining({ kind: "calendarPeople", startDate: "2026-11-08" }),
    }),
  );
  await userEvent.click(screen.getByRole("button", { name: copy.calendar.previousWeek }));
  expect(
    await screen.findByText(copy.calendar.shownDates("2026-11-01", "2026-11-08")),
  ).toBeVisible();
  await userEvent.click(
    within(screen.getByRole("grid", { name: copy.kinds.calendarPeople })).getByRole("button", {
      name: copy.calendar.viewDay("Person 51", "2026-11-01"),
    }),
  );
  expect(await screen.findByRole("table", { name: copy.kinds.assignments })).toBeVisible();
  expect(api.getSolutionView).toHaveBeenLastCalledWith(
    expect.objectContaining({
      parameters: expect.objectContaining({
        kind: "assignments",
        personId: personId(51),
        localDate: "2026-11-01",
      }),
    }),
  );
  expect(screen.getByRole("heading", { name: copy.heading })).toHaveFocus();
  expect(screen.getByText(`${copy.localDateFilter}: 2026-11-01`)).toBeVisible();
});

it("shows uncovered original shifts and keeps location and shift inspection on native calendar queries", async () => {
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  await userEvent.click(screen.getByLabelText(copy.kinds.calendarShifts));
  const grid = await screen.findByRole("grid", { name: copy.kinds.calendarShifts });
  expect(within(grid).getAllByRole("row")).toHaveLength(4);
  expect(within(grid).getAllByText(copy.coverage.belowMinimum(1), { exact: false })).toHaveLength(
    2,
  );
  const earlier = `Clinic · North Clinic · 2026-11-01T01:30:00 UTC−04:00 (${shiftId})`;
  const later = `Clinic · North Clinic · 2026-11-01T01:30:00 UTC−05:00 (${personId(204)})`;
  expect(
    within(grid).getByRole("button", { name: copy.calendar.viewShift(later, "2026-11-01") }),
  ).toBeVisible();
  const locations = within(grid).getAllByRole("button", {
    name: copy.showLocation("North Clinic"),
  });
  expect(locations).toHaveLength(2);
  const locationButton = locations[0];
  if (!locationButton) throw new Error("Both North Clinic shifts must have location navigation");
  await userEvent.click(locationButton);
  expect(
    await screen.findByText(`${copy.locationFilter}: North Clinic`, { exact: false }),
  ).toBeVisible();
  await expect
    .poll(
      () =>
        within(screen.getByRole("grid", { name: copy.kinds.calendarShifts })).getAllByRole("row")
          .length,
    )
    .toBe(3);
  expect(api.getSolutionView).toHaveBeenLastCalledWith(
    expect.objectContaining({
      parameters: expect.objectContaining({ kind: "calendarShifts", locationId: personId(33) }),
    }),
  );
  await userEvent.click(screen.getByRole("button", { name: copy.clearLocation }));
  await expect
    .poll(
      () =>
        within(screen.getByRole("grid", { name: copy.kinds.calendarShifts })).getAllByRole("row")
          .length,
    )
    .toBe(4);
  await userEvent.click(
    within(screen.getByRole("grid", { name: copy.kinds.calendarShifts })).getByRole("button", {
      name: copy.calendar.viewShift(earlier, "2026-11-01"),
    }),
  );
  expect(await screen.findByRole("table", { name: copy.kinds.assignments })).toBeVisible();
  expect(api.getSolutionView).toHaveBeenLastCalledWith(
    expect.objectContaining({
      parameters: expect.objectContaining({ kind: "assignments", shiftId }),
    }),
  );
});

it("distinguishes a missing original-source base from paged added and removed assignments", async () => {
  let hasBase = false;
  const original = vi.mocked(api.getSolutionView).getMockImplementation();
  if (original === undefined) throw new Error("Native result-view fixture is missing.");
  vi.mocked(api.getSolutionView).mockImplementation((input) => {
    if (input.parameters?.kind !== "baseDiff") return original(input);
    return Promise.resolve(
      response({
        schemaVersion: 1,
        result: {
          kind: "baseDiff",
          baseSchedule: hasBase
            ? { id: personId(70), sourceSolutionId: personId(71), sourceRevision: 2 }
            : null,
          totalItems: hasBase ? 2 : 0,
          items: hasBase
            ? [
                {
                  change: "added",
                  personId: personId(51),
                  shiftId,
                  personName: "Person 51",
                  shiftLabel: "North Clinic",
                },
                {
                  change: "removed",
                  personId: personId(52),
                  shiftId,
                  personName: "Person 52",
                  shiftLabel: "North Clinic",
                },
              ]
            : [],
          continuation: null,
        },
      }),
    );
  });
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  await userEvent.click(screen.getByLabelText(copy.kinds.baseDiff));
  await expect.element(await screen.findByText(copy.baseDiff.noBase)).toBeVisible();
  expect(screen.queryByText(copy.empty.baseDiff)).toBeNull();
  expect(screen.queryByRole("table", { name: copy.kinds.baseDiff })).toBeNull();
  hasBase = true;
  await userEvent.click(screen.getByLabelText(copy.kinds.fairness));
  await userEvent.click(screen.getByLabelText(copy.kinds.baseDiff));
  const table = await screen.findByRole("table", { name: copy.kinds.baseDiff });
  expect(screen.queryByText(copy.baseDiff.noBase)).toBeNull();
  expect(screen.getByText(personId(71), { exact: false })).toBeVisible();
  expect(
    within(table).getByRole("row", { name: /Added to result.*Person 51.*North Clinic/ }),
  ).toBeVisible();
  expect(
    within(table).getByRole("row", { name: /Removed from result.*Person 52.*North Clinic/ }),
  ).toBeVisible();
});
it("renders exact accepted fairness distributions with source policy and period", async () => {
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  await screen.findByRole("button", { name: copy.showPerson("Person 50") });
  await userEvent.click(screen.getByLabelText(copy.kinds.fairness));
  expect(await screen.findByRole("table", { name: copy.kinds.fairness })).toBeVisible();
  expect(screen.getByRole("columnheader", { name: copy.fairness.groupActual })).toBeVisible();
  expect(screen.getByRole("columnheader", { name: copy.fairness.groupDeviation })).toBeVisible();
  const river = within(screen.getByRole("row", { name: /River/ }));
  expect(
    river.getByRole("cell", { name: /1 – 5\/2; Spread: 3\/2 in authored bucket units/ }),
  ).toBeVisible();
  expect(
    river.getByRole("cell", { name: /-1\/2 – 1; Spread: 3\/2 in authored bucket units/ }),
  ).toBeVisible();
  expect(screen.getByRole("row", { name: /River/ })).toMatchTextContent(
    "Clinical minutes · weightedEqualShare · 018f7b40-a000-7000-8000-00000000000a",
  );
  expect(screen.getByRole("row", { name: /River/ })).toMatchTextContent("3/2");
  expect(screen.getByRole("row", { name: /River/ })).toMatchTextContent("-1/2");
  expect(screen.getByRole("row", { name: /River/ })).toMatchTextContent(
    "River (018f7b40-a000-7000-8000-000000000001)",
  );
  expect(screen.getByRole("row", { name: /River/ })).toMatchTextContent(
    "2026-11-01T00:00:00 UTC−04:00 – 2026-11-02T00:00:00 UTC−05:00",
  );
});
it("renders original-rule Health margins with exact fractions, authored upper limits and explicit no-applicable work", async () => {
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  await userEvent.click(screen.getByLabelText(copy.kinds.health));
  const table = await screen.findByRole("table", { name: copy.kinds.health });
  expect(screen.getByText(copy.health.description)).toBeVisible();
  const rest = within(table).getByRole("row", { name: /Minimum rest/ });
  expect(rest).toMatchTextContent(`1/2 ${copy.health.unitsLabels.minutes}`);
  expect(rest).toMatchTextContent(`${copy.health.rule} ${personId(9)}`);
  await userEvent.click(within(rest).getByText(copy.health.showEvidence));
  expect(within(rest).getByText("201/2")).toBeVisible();
  expect(within(rest).getByText(copy.health.directionLabels.minimum)).toBeVisible();
  const maximum = within(table).getByRole("row", { name: /Staffing headcount.*Met/ });
  expect(maximum).toMatchTextContent(`${copy.health.ownerLabels.shiftInstance} ${shiftId}`);
  expect(maximum).toMatchTextContent(`1 ${copy.health.unitsLabels.people}`);
  await userEvent.click(within(maximum).getByText(copy.health.showEvidence));
  expect(within(maximum).getByText(copy.health.directionLabels.maximum)).toBeVisible();
  expect(maximum).toMatchTextContent("2026-11-01T13:00:00Z");
  const noApplicable = within(table).getByRole("row", {
    name: /Staffing headcount.*No applicable work/,
  });
  expect(noApplicable).toMatchTextContent(`${copy.health.rule} ${personId(11)}`);
  expect(noApplicable).not.toMatchTextContent("0/1");
  expect(noApplicable).toMatchTextContent(copy.notAvailable);
  await userEvent.click(within(rest).getByText(copy.health.assignmentIds(1, 1)));
  expect(within(rest).getByText(assignmentId)).toBeVisible();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("keeps at most one accepted page in the DOM and navigates to its selected assignment", async () => {
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  await screen.findByRole("button", { name: copy.showPerson("Person 50") });
  expect(screen.getByText(`${copy.timeZone}: America/New_York`)).toBeVisible();
  expect(screen.getAllByRole("row")).toHaveLength(51);
  expect(screen.queryByText("Person 51")).toBeNull();

  await userEvent.click(screen.getByRole("button", { name: copy.next }));
  await screen.findByRole("button", { name: copy.showPerson("Person 51") });
  expect(screen.getAllByRole("row")).toHaveLength(2);
  expect(screen.queryByText("Person 1")).toBeNull();

  await userEvent.click(screen.getByRole("button", { name: copy.showPerson("Person 51") }));
  expect(await screen.findByRole("table", { name: copy.kinds.timeline })).toBeVisible();
  expect(screen.getByRole("heading", { name: copy.heading })).toHaveFocus();
  expect(screen.getByText("2026-11-01T08:00:00 UTC−05:00")).toBeVisible();
  expect(screen.getByRole("button", { name: copy.clearFilter })).toBeVisible();
});

it("loads terminal pair-local evidence on demand, labels its limits, and returns keyboard focus", async () => {
  const baseline = explainedResponse();
  const withPreference: ApiResponseDto<SolutionExplanationDtoV2> = {
    ...baseline,
    result: {
      ...baseline.result,
      explanation: {
        ...baseline.result.explanation,
        rendered: {
          ...baseline.result.explanation.rendered,
          messages: [
            ...baseline.result.explanation.rendered.messages,
            {
              messageKey: "official.workforce.assignment.preference.requested_time_off",
              parameters: {
                "official.workforce.fact.summary": {
                  type: "text",
                  value:
                    "This selected pair matches a source requested-time-off preference. Requested time off is not approved leave; at objective level official.workforce.objective.preference.preferences, this term has 100 penalty units when selected and 0 if unselected. This is not a feasible alternative, full-schedule score difference, or causal proof.",
                },
                "official.workforce.fact.kind": { type: "text", value: "Requested time off" },
                "official.workforce.fact.objective_level": {
                  type: "text",
                  value: "official.workforce.objective.preference.preferences",
                },
                "official.workforce.fact.selected_pair_penalty": {
                  type: "integer",
                  value: "100",
                },
                "official.workforce.fact.unselected_pair_penalty": {
                  type: "integer",
                  value: "0",
                },
              },
              entities: [],
              rules: [personId(42)],
              assignments: [assignmentId],
              evidence: [],
            },
            {
              messageKey: "official.workforce.assignment.preference.limit",
              parameters: {
                "official.workforce.fact.summary": {
                  type: "text",
                  value:
                    "Only the first 128 matching active source preference conditions are shown in rule order. Additional matches may exist; this is not a complete preference inventory.",
                },
              },
              entities: [],
              rules: [],
              assignments: [assignmentId],
              evidence: [],
            },
          ],
        },
      },
    },
  };
  vi.mocked(api.explainSolution).mockReturnValueOnce(portableOperation(withPreference));
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  await userEvent.click(screen.getByLabelText(copy.kinds.assignments));
  const table = await screen.findByRole("table", { name: copy.kinds.assignments });
  const inspect = within(table).getByRole("button", {
    name: copy.inspection.inspect("Person 51", "Clinic", "2026-11-01T08:00:00 UTC−05:00"),
  });
  expect(api.explainSolution).not.toHaveBeenCalled();
  await userEvent.click(inspect);
  const details = await screen.findByRole("complementary", { name: copy.inspection.heading });
  expect(await within(details).findByText(copy.inspection.limits)).toBeVisible();
  expect(within(details).getByText(`${copy.start}: 2026-11-01T08:00:00 UTC−05:00`)).toBeVisible();
  expect(within(details).getByText(copy.inspection.recordedDecision)).toBeVisible();
  expect(
    within(details).getByText(`${copy.inspection.sourceCheckIds}: ${personId(9)}`),
  ).toBeVisible();
  expect(within(details).getByText("checked predicates")).toBeVisible();
  expect(within(details).getByText("2")).toBeVisible();
  expect(
    within(details).getByText("Recorded selection; only pair-local checks are recomputed."),
  ).toBeVisible();
  expect(within(details).getByText(copy.inspection.preferenceFact)).toBeVisible();
  expect(
    within(details).getByText(`${copy.inspection.preferenceRuleIds}: ${personId(42)}`),
  ).toBeVisible();
  expect(within(details).getByText("Requested time off")).toBeVisible();
  expect(within(details).getByText(/Requested time off is not approved leave/u)).toBeVisible();
  expect(within(details).getByText("objective level")).toBeVisible();
  expect(
    within(details).getByText("official.workforce.objective.preference.preferences"),
  ).toBeVisible();
  expect(within(details).getByText("selected pair penalty")).toBeVisible();
  expect(within(details).getByText("100")).toBeVisible();
  expect(within(details).getByText("unselected pair penalty")).toBeVisible();
  expect(within(details).getByRole("heading", { name: copy.inspection.heading })).toHaveFocus();
  await userEvent.click(within(details).getByRole("button", { name: copy.inspection.close }));
  expect(inspect).toHaveFocus();
  expect(screen.queryByRole("complementary", { name: copy.inspection.heading })).toBeNull();
});

it("keeps the open assignment inspector through equivalent saved-result list refreshes", async () => {
  const rendered = render(AcceptedSchedule, {
    props: { scenarioId, summary, currentRevision: 3 },
  });
  await userEvent.click(screen.getByLabelText(copy.kinds.assignments));
  await screen.findByRole("table", { name: copy.kinds.assignments });
  await userEvent.click(
    screen.getByRole("button", {
      name: copy.inspection.inspect("Person 51", "Clinic", "2026-11-01T08:00:00 UTC−05:00"),
    }),
  );
  const details = await screen.findByRole("complementary", { name: copy.inspection.heading });
  await within(details).findByText(copy.inspection.limits);
  const viewReads = vi.mocked(api.getSolutionView).mock.calls.length;
  await rendered.rerender({
    scenarioId,
    summary: { ...summary, result: { ...summary.result } },
    currentRevision: 3,
  });
  expect(details).toBeVisible();
  expect(api.explainSolution).toHaveBeenCalledTimes(1);
  expect(api.getSolutionView).toHaveBeenCalledTimes(viewReads);
});

it("keeps checkpoint-only assignment inspection honest and rejects a mismatched terminal explanation", async () => {
  const checkpoint = reactive<{
    result: SolutionSummaryDtoV1["result"];
    scenarioRevision: number;
    status?: SolutionSummaryDtoV1["status"];
  }>({
    result: summary.result,
    scenarioRevision: summary.scenarioRevision,
  });
  render(AcceptedSchedule, {
    props: { scenarioId, summary: checkpoint, currentRevision: 3 },
  });
  await userEvent.click(screen.getByLabelText(copy.kinds.timeline));
  await screen.findByRole("table", { name: copy.kinds.timeline });
  await userEvent.click(
    screen.getByRole("button", {
      name: copy.inspection.inspect("Person 51", "Clinic", "2026-11-01T08:00:00 UTC−05:00"),
    }),
  );
  expect(screen.getByText(copy.inspection.checkpoint)).toBeVisible();
  expect(api.explainSolution).not.toHaveBeenCalled();

  const details = await screen.findByRole("complementary", { name: copy.inspection.heading });
  vi.mocked(api.explainSolution).mockReturnValueOnce(
    portableOperation(explainedResponse(3, personId(99))),
  );
  checkpoint.status = "feasible";
  expect(await within(details).findByRole("alert")).toMatchTextContent(copy.sourceChanged);
  expect(within(details).queryByText(copy.inspection.limits)).toBeNull();
  await userEvent.click(within(details).getByRole("button", { name: copy.inspection.retry }));
  expect(await within(details).findByText(copy.inspection.limits)).toBeVisible();
});

it("discards a late assignment explanation after the accepted view changes", async () => {
  const pending = Promise.withResolvers<ApiResponseDto<SolutionExplanationDtoV2>>();
  vi.mocked(api.explainSolution).mockReturnValueOnce(portableOperation(pending.promise));
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  await userEvent.click(screen.getByLabelText(copy.kinds.timeline));
  await screen.findByRole("table", { name: copy.kinds.timeline });
  await userEvent.click(
    screen.getByRole("button", {
      name: copy.inspection.inspect("Person 51", "Clinic", "2026-11-01T08:00:00 UTC−05:00"),
    }),
  );
  expect(screen.getByText(copy.inspection.loading)).toBeVisible();
  await userEvent.click(screen.getByLabelText(copy.kinds.fairness));
  expect(await screen.findByRole("table", { name: copy.kinds.fairness })).toBeVisible();
  pending.resolve(explainedResponse());
  await pending.promise;
  expect(screen.queryByRole("complementary", { name: copy.inspection.heading })).toBeNull();
});

it("keeps large native assignment evidence keyboard-pageable without rendering every message", async () => {
  vi.mocked(api.explainSolution).mockReturnValueOnce(
    portableOperation(explainedResponse(3, assignmentId, 121)),
  );
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  await userEvent.click(screen.getByLabelText(copy.kinds.assignments));
  await screen.findByRole("table", { name: copy.kinds.assignments });
  await userEvent.click(
    screen.getByRole("button", {
      name: copy.inspection.inspect("Person 51", "Clinic", "2026-11-01T08:00:00 UTC−05:00"),
    }),
  );
  const details = await screen.findByRole("complementary", { name: copy.inspection.heading });
  expect(await within(details).findByText(copy.inspection.messageCount(1, 50, 121))).toBeVisible();
  expect(within(details).getByRole("list").children).toHaveLength(50);
  await userEvent.click(within(details).getByRole("button", { name: copy.inspection.next }));
  expect(within(details).getByText(copy.inspection.messageCount(51, 100, 121))).toBeVisible();
  expect(within(details).getByRole("list").children).toHaveLength(50);
  await userEvent.click(within(details).getByRole("button", { name: copy.inspection.next }));
  expect(within(details).getByText(copy.inspection.messageCount(101, 121, 121))).toBeVisible();
  expect(within(details).getByRole("list").children).toHaveLength(21);
});

it("navigates native shift coverage by original location without losing keyboard focus", async () => {
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  await userEvent.click(screen.getByLabelText(copy.kinds.shifts));
  const all = await screen.findByRole("table", { name: copy.kinds.shifts });
  expect(within(all).getAllByRole("row")).toHaveLength(3);
  expect(within(all).getByRole("columnheader", { name: copy.minimum })).toBeVisible();
  expect(within(all).getByText(copy.coverage.belowPreferred(1))).toBeVisible();
  expect(within(all).getByText(copy.coverage.belowMinimum(1))).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: copy.showLocation("North Clinic") }));
  expect(screen.getByRole("heading", { name: copy.heading })).toHaveFocus();
  const filtered = await screen.findByRole("table", { name: copy.kinds.shifts });
  expect(within(filtered).getAllByRole("row")).toHaveLength(2);
  expect(within(filtered).getByText(copy.coverage.belowPreferred(1))).toBeVisible();
  expect(within(filtered).queryByText(copy.coverage.belowMinimum(1))).toBeNull();
  expect(screen.getByText(`${copy.locationFilter}: North Clinic`, { exact: false })).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: copy.clearLocation }));
  expect(
    within(await screen.findByRole("table", { name: copy.kinds.shifts })).getAllByRole("row"),
  ).toHaveLength(3);
});

it("does not promote retained status to verified facts when a fresh report disagrees, then recovers on retry", async () => {
  const pending = Promise.withResolvers<ApiResponseDto<SolutionVerificationDtoV1>>();
  vi.mocked(api.verifySolution).mockReturnValueOnce(pending.promise);
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  expect(screen.getByText(copy.verification.recordedStatus, { exact: false })).toBeVisible();
  expect(screen.queryByText(copy.verification.current)).toBeNull();
  pending.resolve(verifiedResponse(3, "0".repeat(64)));
  const error = await screen.findByRole("alert");
  expect(error).toMatchTextContent(copy.sourceChanged);
  expect(screen.queryByText(copy.verification.current)).toBeNull();

  await userEvent.click(screen.getByRole("button", { name: copy.verification.retry }));
  expect(await screen.findByText(copy.verification.current)).toBeVisible();
  expect(screen.getByText(copy.verification.required(1))).toBeVisible();
  expect(screen.getByText("Preferred times: 5")).toBeVisible();
  expect(screen.getByText(copy.verification.satisfaction(0, 1))).toBeVisible();
});

it("distinguishes a verified priority with no scored penalty from an unmet preference tradeoff", async () => {
  vi.mocked(api.verifySolution).mockResolvedValue(
    verifiedResponse(3, summary.verificationChecksum, "0"),
  );
  render(AcceptedSchedule, { props: { scenarioId, summary, currentRevision: 3 } });
  expect(await screen.findByText(copy.verification.satisfaction(1, 1))).toBeVisible();
  expect(screen.queryByText("Preferred times: 5")).toBeNull();
});

it("discards an old verification after a same-scenario edit and labels fresh old-result facts as older", async () => {
  const old = Promise.withResolvers<ApiResponseDto<SolutionVerificationDtoV1>>();
  const updated = Promise.withResolvers<ApiResponseDto<SolutionVerificationDtoV1>>();
  vi.mocked(api.verifySolution)
    .mockReturnValueOnce(old.promise)
    .mockReturnValueOnce(updated.promise);
  const rendered = render(AcceptedSchedule, {
    props: { scenarioId, summary, currentRevision: 3 },
  });
  await rendered.rerender({ scenarioId, summary, currentRevision: 4 });
  old.resolve(verifiedResponse());
  expect(screen.queryByText(copy.verification.current)).toBeNull();
  updated.resolve(verifiedResponse(4));
  expect(
    await screen.findByText(copy.verification.older(String(summary.scenarioRevision))),
  ).toBeVisible();
  expect(screen.queryByText(copy.verification.current)).toBeNull();
});

it("opens a freshly verified checkpoint schedule before any terminal run conclusion exists", async () => {
  render(AcceptedSchedule, {
    props: {
      scenarioId,
      summary: { result: summary.result, scenarioRevision: summary.scenarioRevision },
      currentRevision: 3,
    },
  });
  expect(await screen.findByText(copy.verification.current)).toBeVisible();
  expect(screen.queryByText(copy.verification.recordedStatus, { exact: false })).toBeNull();
  expect(await screen.findByRole("table", { name: copy.kinds.people })).toBeVisible();
});
