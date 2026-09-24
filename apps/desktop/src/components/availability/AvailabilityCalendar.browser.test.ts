import { cleanup, render, screen, within } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import axe from "axe-core";
import { createPinia } from "pinia";
import { afterEach, describe, expect, it, vi } from "vitest";
import { defineComponent, h, nextTick, type PropType } from "vue";
import * as api from "../../api/generated";
import type {
  WorkforceSetupAvailabilityKind,
  WorkforceSetupAvailabilityOccurrence,
  WorkforceSetupAvailabilityPage,
  WorkforceSetupAvailabilityWindowResult,
} from "../../api/generated-domain-pack-contracts";
import { createProjectHomeController, type ProjectSummary } from "../../project-home";
import { fakeApi, portableOperation, project, response } from "../../testing/project-home";
import AvailabilityCalendar from "./AvailabilityCalendar.vue";
import "../../styles.css";

vi.mock("../../api/generated", { spy: true });

const workforceProject = { ...project, domainPackId: "official.workforce" };
const personId = "01900000-0000-7000-8000-000000000011";
const otherPersonId = "01900000-0000-7000-8000-000000000012";

type AvailabilityReceipt = api.ApiResponseDto<
  api.ScenarioSetupViewResultV2<"official.workforce.setup.availability_window">
>;

function availabilityReceipt(
  availPage: WorkforceSetupAvailabilityPage,
  revision = workforceProject.revision,
): AvailabilityReceipt {
  const viewResult: WorkforceSetupAvailabilityWindowResult = {
    schemaVersion: 1,
    result: { kind: "availabilityWindow", data: availPage },
  };
  return response(
    {
      schemaVersion: 2 as const,
      scenarioId: workforceProject.scenarioId,
      revision,
      view: {
        viewId: "official.workforce.setup.availability_window" as const,
        data: viewResult,
      },
    },
    [],
    revision,
  );
}

function occurrence(
  ordinal: number,
  availabilityId: string,
  availabilityKind: WorkforceSetupAvailabilityKind,
  start: string,
  end: string,
): WorkforceSetupAvailabilityOccurrence {
  return { availabilityId, availabilityKind, interval: { start, end }, ordinal };
}

function page(
  items: readonly WorkforceSetupAvailabilityOccurrence[],
): WorkforceSetupAvailabilityPage {
  return { items, totalItems: items.length, continuation: null };
}

function mountCalendar(timeZone = "UTC") {
  const Harness = defineComponent({
    props: {
      project: { type: Object as PropType<ProjectSummary>, required: true },
      personId: { type: String, required: true },
    },
    setup(props) {
      const home = createProjectHomeController(fakeApi([workforceProject]));
      return () =>
        h(AvailabilityCalendar, {
          home,
          project: props.project,
          personId: props.personId,
          initialWorkWindow: { startDate: "2026-01-01", endDateExclusive: "2026-01-05" },
          timeZone,
          locale: "en-US",
        });
    },
  });
  return render(Harness, {
    props: { project: workforceProject, personId },
    global: { plugins: [createPinia(), PiniaColada] },
  });
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("half-open midnight boundary in a non-UTC scenario timezone", () => {
  it("excludes an exact-midnight end and includes a nanosecond-after-midnight end on the next calendar day", async () => {
    const midnightEnd = occurrence(
      0,
      "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
      "unavailable",
      "2026-01-01T09:00:00-05:00",
      "2026-01-02T00:00:00-05:00",
    );
    const nanoAfterMidnightEnd = occurrence(
      1,
      "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
      "approvedTimeOff",
      "2026-01-01T09:00:00-05:00",
      "2026-01-02T00:00:00.000000001-05:00",
    );
    vi.mocked(api.getScenarioView).mockReturnValueOnce(
      portableOperation(availabilityReceipt(page([midnightEnd, nanoAfterMidnightEnd]))),
    );

    const rendered = mountCalendar("America/New_York");

    // Wait for async load to populate the calendar.
    await screen.findByRole("button", { name: "Add availability for 2026-01-01" });
    expect((await axe.run(rendered.container)).violations).toEqual([]);

    // Both raw interval strings appear in the native list.
    const list = screen.getByRole("region", { name: /availability intervals on this page/i });
    expect(within(list).getByText("2026-01-02T00:00:00-05:00")).toBeTruthy();
    expect(within(list).getByText("2026-01-02T00:00:00.000000001-05:00")).toBeTruthy();

    // Scope each day's occurrences via its Add button.
    const jan1Row = screen
      .getByRole("button", { name: "Add availability for 2026-01-01" })
      .closest("li");
    const jan2Row = screen
      .getByRole("button", { name: "Add availability for 2026-01-02" })
      .closest("li");
    if (jan1Row === null || jan2Row === null) throw new Error("Missing calendar date rows");
    const jan1 = within(jan1Row),
      jan2 = within(jan2Row);

    // Jan 1: both occurrences visible.
    expect(jan1.getByText(/^Unavailable\b/)).toBeTruthy();
    expect(jan1.getByText(/^Approved time off\b/)).toBeTruthy();

    // Jan 2: only the nanosecond-after-midnight occurrence.
    expect(jan2.getByText(/^Approved time off\b/)).toBeTruthy();
    expect(jan2.queryByText(/^Unavailable\b/)).toBeNull();
  });
});

describe("stale revision does not overwrite current results", () => {
  it("discards an old person's delayed response when props change to a newer person", async () => {
    const oldPage = page([
      occurrence(
        0,
        "cccccccc-cccc-cccc-cccc-cccccccccccc",
        "unavailable",
        "2026-01-01T00:00:00Z",
        "2026-01-02T00:00:00Z",
      ),
    ]);
    const newPage = page([
      occurrence(
        0,
        "dddddddd-dddd-dddd-dddd-dddddddddddd",
        "approvedTimeOff",
        "2026-01-02T00:00:00Z",
        "2026-01-03T00:00:00Z",
      ),
    ]);

    // Delayed mock for the initial render — still pending when rerender fires.
    const { promise: delayedOld, resolve: resolveOld } =
      Promise.withResolvers<AvailabilityReceipt>();
    vi.mocked(api.getScenarioView).mockReturnValueOnce(portableOperation(delayedOld));

    const rendered = mountCalendar();

    // Immediate mock for the new person's load — installed BEFORE rerender.
    vi.mocked(api.getScenarioView).mockReturnValueOnce(
      portableOperation(availabilityReceipt(newPage)),
    );
    await rendered.rerender({ personId: otherPersonId });

    // Wait for new person's data to render.
    await screen.findByText("dddddddd-dddd-dddd-dddd-dddddddddddd");

    // Resolve the stale old-person response after new data is already current.
    resolveOld(availabilityReceipt(oldPage));
    await delayedOld;
    await nextTick();

    // Old person's ID must not appear.
    expect(screen.queryByText("cccccccc-cccc-cccc-cccc-cccccccccccc")).toBeNull();
    // New person's ID must remain.
    const list = screen.getByRole("region", { name: /availability intervals on this page/i });
    expect(within(list).getByText("dddddddd-dddd-dddd-dddd-dddddddddddd")).toBeTruthy();
  });

  it("discards an old revision's delayed response when project revision bumps", async () => {
    const oldPage = page([
      occurrence(
        0,
        "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee",
        "availableOnly",
        "2026-01-01T00:00:00Z",
        "2026-01-02T00:00:00Z",
      ),
    ]);
    const bumpedRevision = workforceProject.revision + 1;
    const newPage = page([
      occurrence(
        0,
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "requestedTimeOff",
        "2026-01-03T00:00:00Z",
        "2026-01-04T00:00:00Z",
      ),
    ]);

    const { promise: delayedOld, resolve: resolveOld } =
      Promise.withResolvers<AvailabilityReceipt>();
    vi.mocked(api.getScenarioView).mockReturnValueOnce(portableOperation(delayedOld));

    const rendered = mountCalendar();

    // Mock carries bumped revision for the new load.
    vi.mocked(api.getScenarioView).mockReturnValueOnce(
      portableOperation(availabilityReceipt(newPage, bumpedRevision)),
    );
    const bumpedProject = { ...workforceProject, revision: bumpedRevision };
    await rendered.rerender({ project: bumpedProject });

    await screen.findByText("ffffffff-ffff-ffff-ffff-ffffffffffff");

    resolveOld(availabilityReceipt(oldPage));
    await delayedOld;
    await nextTick();

    expect(screen.queryByText("eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee")).toBeNull();
    const list = screen.getByRole("region", { name: /availability intervals on this page/i });
    expect(within(list).getByText("ffffffff-ffff-ffff-ffff-ffffffffffff")).toBeTruthy();
  });
});
