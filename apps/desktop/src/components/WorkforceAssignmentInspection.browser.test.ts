import { cleanup, render, screen } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { afterEach, describe, expect, it, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { defineComponent, h, nextTick } from "vue";
import * as api from "../api/generated";
import type {
  WorkforceSetupQueryResults,
  WorkforceSetupWorkShift,
} from "../api/generated-domain-pack-contracts";
import { createProjectHomeController } from "../project-home";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import { messages } from "../messages";
import WorkforceAssignmentInspection from "./WorkforceAssignmentInspection.vue";
import "../styles.css";

vi.mock("../api/generated", { spy: true });
const workforceProject = { ...project, domainPackId: "official.workforce" };
const personId = "01900000-0000-7000-8000-000000000011";
const secondPersonId = "01900000-0000-7000-8000-000000000012";
const bindingId = "01900000-0000-7000-8000-000000000020";
const shift: WorkforceSetupWorkShift = {
  shiftId: "01900000-0000-7000-8000-000000000030",
  assignmentTypeId: "01900000-0000-7000-8000-000000000004",
  assignmentTypeName: "Clinic",
  coverage: { minimum: 1, maximum: 1, preferred: null, qualificationMinimumCount: 0 },
  elapsed: { seconds: "3600", nanoseconds: 0 },
  scheduled: { seconds: "3600", nanoseconds: 0 },
  interval: {
    startsAt: { instant: "2030-01-01T09:00:00Z", local: "2030-01-01T09:00:00", offsetSeconds: 0 },
    endsAt: { instant: "2030-01-01T10:00:00Z", local: "2030-01-01T10:00:00", offsetSeconds: 0 },
  },
  locationId: null,
  locationName: null,
  templateName: null,
  origin: { kind: "manual" },
  reportingDate: "2030-01-01",
};
// Controlled receipts prove renderer ordering/cancellation, not native domain semantics.
function receipt<Id extends keyof WorkforceSetupQueryResults>(
  viewId: Id,
  data: WorkforceSetupQueryResults[Id],
) {
  return response({
    schemaVersion: 2 as const,
    scenarioId: project.scenarioId,
    revision: project.revision,
    view: { viewId, data },
  });
}
type InspectionReceipt = api.ApiResponseDto<
  api.ScenarioSetupViewResultV2<"official.workforce.setup.assignment_inspection">
>;
function blockedReceipt(): InspectionReceipt {
  return receipt("official.workforce.setup.assignment_inspection", {
    schemaVersion: 1,
    result: {
      kind: "assignmentInspection",
      data: {
        personId,
        shiftId: shift.shiftId,
        candidateInImplementedAssignmentGraph: false,
        remainingRequiredRuleCount: 2,
        rejections: {
          totalItems: 1,
          continuation: null,
          items: [
            {
              ordinal: 0,
              bindingId,
              cause: { kind: "assignmentTypeNotAllowed", assignmentTypeId: shift.assignmentTypeId },
            },
          ],
        },
      },
    },
  });
}
function setup() {
  vi.mocked(api.getScenarioView)
    .mockReturnValueOnce(
      portableOperation(
        receipt("official.workforce.setup.overview", {
          schemaVersion: 1,
          result: {
            kind: "overview",
            data: {
              activePreferences: 0,
              activeRequiredRules: 3,
              configuredTypeMemberships: 1,
              entities: [],
              lockedAssignments: 0,
              preferences: 0,
              requiredRules: 3,
              planningDates: { startDate: "2030-01-01", endDateExclusive: "2030-02-01" },
              initialWorkWindow: { startDate: "2030-01-01", endDateExclusive: "2030-01-08" },
              settings: {
                timeZone: "UTC",
                locale: "en-US",
                units: "metric",
                horizon: { start: "2030-01-01T00:00:00Z", end: "2030-02-01T00:00:00Z" },
                gapPolicy: "reject",
                overlapPolicy: "earlier",
              },
            },
          },
        }),
      ),
    )
    .mockReturnValueOnce(
      portableOperation(
        receipt("official.workforce.setup.work_window", {
          schemaVersion: 1,
          result: {
            kind: "workWindow",
            data: { totalItems: 1, items: [shift], continuation: null },
          },
        }),
      ),
    );
  const Harness = defineComponent({
    props: { personId: { type: String, required: true } },
    setup(props) {
      const home = createProjectHomeController(fakeApi([workforceProject]));
      return () =>
        h(WorkforceAssignmentInspection, {
          home,
          project: workforceProject,
          personId: props.personId,
        });
    },
  });
  return render(Harness, {
    props: { personId },
    global: { plugins: [createPinia(), PiniaColada] },
  });
}
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("applied assignment inspection ownership", () => {
  it("discards a late person's findings without stealing newer input focus", async () => {
    const rendered = setup();
    let complete: ((value: InspectionReceipt) => void) | undefined;
    const delayed = new Promise<InspectionReceipt>((resolve) => {
      complete = resolve;
    });
    vi.mocked(api.getScenarioView).mockReturnValueOnce(portableOperation(delayed));
    await userEvent.click(await screen.findByRole("button", { name: /^Inspect.*Clinic/ }));
    await expect
      .element(screen.getByRole("heading", { name: messages.assignmentInspection.result }))
      .toHaveFocus();
    await rendered.rerender({ personId: secondPersonId });
    const date = screen.getByLabelText(messages.assignmentInspection.startDate);
    await userEvent.click(date);
    complete?.(blockedReceipt());
    await delayed;
    await nextTick();
    await expect.element(date).toHaveFocus();
    expect(screen.queryByText(bindingId)).toBeNull();
    expect(screen.queryByText(messages.assignmentInspection.blocked)).toBeNull();
    await expect.element(screen.getByText(secondPersonId)).toBeVisible();
  });

  it("keeps focus on the result when reinspection removes its trigger", async () => {
    setup();
    vi.mocked(api.getScenarioView).mockReturnValueOnce(portableOperation(blockedReceipt()));
    await userEvent.click(await screen.findByRole("button", { name: /^Inspect.*Clinic/ }));
    await screen.findByText(messages.assignmentInspection.blocked);
    let complete: ((value: InspectionReceipt) => void) | undefined;
    const delayed = new Promise<InspectionReceipt>((resolve) => {
      complete = resolve;
    });
    vi.mocked(api.getScenarioView).mockReturnValueOnce(portableOperation(delayed));
    await userEvent.click(
      screen.getByRole("button", { name: messages.assignmentInspection.inspectAgain }),
    );
    await expect
      .element(screen.getByRole("heading", { name: messages.assignmentInspection.result }))
      .toHaveFocus();
    complete?.(blockedReceipt());
    await delayed;
  });

  it("keeps cancellation pending until the native terminal outcome and preserves focus", async () => {
    setup();
    let fail: ((failure: unknown) => void) | undefined;
    const delayed = new Promise<InspectionReceipt>((_resolve, reject) => {
      fail = reject;
    });
    vi.mocked(api.getScenarioView).mockReturnValueOnce(portableOperation(delayed));
    await userEvent.click(await screen.findByRole("button", { name: /^Inspect.*Clinic/ }));
    await userEvent.click(
      await screen.findByRole("button", { name: messages.assignmentInspection.cancel }),
    );
    await expect.element(screen.getByText(messages.assignmentInspection.cancelling)).toBeVisible();
    expect(screen.queryByText(messages.assignmentInspection.cancelled)).toBeNull();
    fail?.({ category: "cancelled", code: "operation.cancelled", message: "Cancelled" });
    await expect
      .element(await screen.findByText(messages.assignmentInspection.cancelled))
      .toBeVisible();
    await expect
      .element(screen.getByRole("heading", { name: messages.assignmentInspection.result }))
      .toHaveFocus();
    expect(screen.queryByText(messages.assignmentInspection.passed)).toBeNull();
  });
});
