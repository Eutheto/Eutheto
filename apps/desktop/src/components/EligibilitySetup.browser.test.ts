import { cleanup, render, screen } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { afterEach, expect, it, onTestFinished, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { defineComponent, h, nextTick, ref, watch } from "vue";
import { createMemoryHistory, createRouter, RouterView } from "vue-router";
import * as api from "../api/generated";
import type {
  WorkforceSetupQueryResults,
  WorkforceSetupWorkShift,
} from "../api/generated-domain-pack-contracts";
import { createProjectHomeController } from "../project-home";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import EligibilitySetup from "./EligibilitySetup.vue";
import ProjectWorkspace from "./ProjectWorkspace.vue";
import WorkSetup from "./WorkSetup.vue";
import { messages } from "../messages";
import "../styles.css";

vi.mock("../api/generated", { spy: true });
const personId = "01900000-0000-7000-8000-000000000011";
const typeId = "01900000-0000-7000-8000-000000000012";
const dates = { startDate: "2030-01-01", endDateExclusive: "2030-01-08" };
const shift: WorkforceSetupWorkShift = {
  shiftId: "01900000-0000-7000-8000-000000000030",
  assignmentTypeId: typeId,
  assignmentTypeName: "Type",
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
// Native-shaped presentation receipts control read ordering, not validation or mutation authority.
const views = {
  "eutheto.setup.entity_page": {
    schemaVersion: 1,
    result: {
      kind: "entityPage",
      data: {
        items: [{ entityId: typeId, kind: "assignmentType", name: "Type" }],
        totalItems: 1,
        continuation: null,
      },
    },
  },
  "official.workforce.setup.people_page": {
    schemaVersion: 1,
    result: {
      kind: "peoplePage",
      data: { items: [{ personId, name: "Person" }], totalItems: 1, continuation: null },
    },
  },
  "official.workforce.setup.eligibility_matrix": {
    schemaVersion: 1,
    result: {
      kind: "eligibilityMatrix",
      data: { personIds: [personId], assignmentTypeIds: [typeId], configuredMemberships: [[true]] },
    },
  },
  "official.workforce.setup.overview": {
    schemaVersion: 1,
    result: {
      kind: "overview",
      data: {
        activePreferences: 0,
        activeRequiredRules: 0,
        configuredTypeMemberships: 1,
        entities: [],
        lockedAssignments: 0,
        preferences: 0,
        requiredRules: 0,
        planningDates: dates,
        initialWorkWindow: dates,
        settings: {
          timeZone: "UTC",
          locale: "en-US",
          units: "metric",
          horizon: { start: "2030-01-01T00:00:00Z", end: "2030-01-08T00:00:00Z" },
          gapPolicy: "reject",
          overlapPolicy: "earlier",
        },
      },
    },
  },
  "official.workforce.setup.work_window": {
    schemaVersion: 1,
    result: { kind: "workWindow", data: { items: [shift], totalItems: 1, continuation: null } },
  },
} satisfies Partial<WorkforceSetupQueryResults>;

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it.each(
  [
    { target: "inspector", ownership: "retained" },
    { target: "inspector", ownership: "moved during insertion" },
    { target: "inspector", ownership: "scrolled away before insertion" },
    { target: "metadata", ownership: "retained" },
    { target: "metadata", ownership: "moved during insertion" },
    { target: "metadata", ownership: "scrolled away before insertion" },
    { target: "outside workspace", ownership: "retained" },
  ]
    .map((value) => ({ ...value, phase: "matrix" }))
    .concat([
      { target: "metadata", ownership: "retained", phase: "work window" },
      { target: "metadata", ownership: "moved during insertion", phase: "work window" },
      { target: "metadata", ownership: "scrolled away before insertion", phase: "work window" },
      { target: "outside workspace", ownership: "retained", phase: "work window" },
      { target: "metadata", ownership: "retained", phase: "overview" },
      { target: "metadata", ownership: "moved during insertion", phase: "overview" },
      { target: "metadata", ownership: "scrolled away before insertion", phase: "overview" },
      { target: "outside workspace", ownership: "retained", phase: "overview" },
      ...["work setup page", "work setup view"].flatMap((phase) => [
        { target: "metadata", ownership: "retained", phase },
        { target: "metadata", ownership: "moved during insertion", phase },
        { target: "metadata", ownership: "scrolled away before insertion", phase },
        { target: "outside workspace", ownership: "retained", phase },
      ]),
    ]),
)(
  "keeps $target focus ownership when its delayed $phase returns: $ownership",
  async ({ target, ownership, phase }) => {
    const workSetup = phase.startsWith("work setup");
    const currentProject = ref({ ...project, domainPackId: "official.workforce" });
    const native = fakeApi([currentProject.value]);
    native.listProjects.mockImplementation(() => Promise.resolve(response([currentProject.value])));
    const matrix = Promise.withResolvers<undefined>();
    const laterWork = Promise.withResolvers<undefined>();
    let delayMatrix = false;
    let matrixPending = false;
    let workPending = false;
    let overviewPending = false;
    const insertionSelector =
      phase === "matrix"
        ? "[data-eligibility-matrix]"
        : phase === "overview"
          ? "#inspection-start-date"
          : workSetup
            ? '[aria-labelledby="work-window-heading"] table'
            : '[aria-labelledby="assignment-inspection-heading"] .action-row';
    function receipt<Id extends keyof typeof views>(viewId: Id) {
      return response({
        schemaVersion: 2 as const,
        scenarioId: project.scenarioId,
        revision: currentProject.value.revision,
        view: { viewId, data: views[viewId] },
      });
    }
    vi.mocked(api.searchScenarioEntities).mockImplementation(() => {
      const result = structuredClone(receipt("eutheto.setup.entity_page"));
      if (workSetup) result.result.view.data.result.data.items = [];
      return portableOperation(result);
    });
    vi.mocked(api.getScenarioView).mockImplementation((_scope, input) => {
      switch (input.query.viewId) {
        case "official.workforce.setup.people_page":
          return portableOperation(receipt("official.workforce.setup.people_page"));
        case "official.workforce.setup.overview": {
          const result = receipt("official.workforce.setup.overview");
          if (delayMatrix && phase === "overview") {
            overviewPending = true;
            return portableOperation(matrix.promise.then(() => result));
          }
          return portableOperation(result);
        }
        case "official.workforce.setup.work_window": {
          const result = receipt("official.workforce.setup.work_window");
          if (
            delayMatrix &&
            (phase === "work window" || phase === "overview" || phase === "work setup page")
          ) {
            workPending = true;
            const pending = phase === "overview" ? laterWork : matrix;
            return portableOperation(pending.promise.then(() => result));
          }
          return portableOperation(result);
        }
        case "official.workforce.setup.eligibility_matrix": {
          const result = receipt("official.workforce.setup.eligibility_matrix");
          if (delayMatrix && phase === "matrix") {
            matrixPending = true;
            return portableOperation(matrix.promise.then(() => result));
          }
          return portableOperation(result);
        }
        default:
          throw new Error(`Unexpected view ${input.query.viewId}`);
      }
    });
    const Page = defineComponent({
      setup: () => {
        const home = createProjectHomeController(native);
        void home.load();
        watch(currentProject, () => {
          void home.refreshLibrary();
        });
        return () =>
          // Do not let Chromium scroll anchoring hide displacement observed in native WebKit.
          h("div", { style: { overflowAnchor: "none" } }, [
            h(ProjectWorkspace, { home, scenarioId: currentProject.value.scenarioId }),
            h("button", { "data-unrelated-focus": "" }, "Unrelated action"),
          ]);
      },
    });
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        {
          path: "/project/:scenarioId",
          component: Page,
          children: [
            { path: "eligibility", name: "project-eligibility", component: EligibilitySetup },
            { path: "work", name: "project-work", component: WorkSetup },
            ...[
              "setup",
              "people",
              "rules",
              "validation",
              "availability",
              "score-policy",
              "history",
              "export",
              "work-import",
              "eligibility-import",
              "eligibility-matrix-import",
            ].map((name) => ({
              path: name,
              name: `project-${name}`,
              component: { render: () => null },
            })),
          ],
        },
      ],
    });
    await router.push(`/project/${project.scenarioId}/${workSetup ? "work" : "eligibility"}`);
    render(RouterView, { global: { plugins: [createPinia(), PiniaColada, router] } });
    if (workSetup) {
      await expect
        .poll(() => document.querySelector('[aria-labelledby="work-window-heading"] table'))
        .not.toBeNull();
      if (phase === "work setup page")
        await userEvent.click(screen.getByRole("button", { name: messages.work.instances }));
    } else {
      await screen.findByRole("checkbox", { name: "Person — Type" });
      await expect.poll(() => document.querySelector("#inspection-start-date")).not.toBeNull();
    }
    if (phase !== "work setup view") {
      delayMatrix = true;
      currentProject.value = { ...currentProject.value, revision: project.revision + 1 };
      await expect
        .poll(() =>
          phase === "matrix" ? matrixPending : phase === "overview" ? overviewPending : workPending,
        )
        .toBe(true);
      if (!workSetup) {
        if (phase !== "matrix") await screen.findByRole("checkbox", { name: "Person — Type" });
        if (phase !== "overview")
          await expect.poll(() => document.querySelector("#inspection-start-date")).not.toBeNull();
      }
    }
    const input = document.querySelector<HTMLElement>(
      target === "inspector"
        ? "#inspection-start-date"
        : target === "metadata"
          ? "[data-project-workspace] > details.setup-more > summary"
          : "[data-unrelated-focus]",
    );
    if (!input) throw new Error(`Missing ${target} focus target`);
    input.focus();
    await nextTick();
    expect(input.getBoundingClientRect().bottom).toBeLessThanOrEqual(innerHeight);
    const matches = input.matches.bind(input);
    vi.spyOn(input, "matches").mockImplementation((selector) =>
      selector === ":focus-visible" ? false : matches(selector),
    );
    if (ownership === "scrolled away before insertion") {
      window.scrollTo(0, 0);
      expect(input.getBoundingClientRect().top).toBeGreaterThan(innerHeight);
    }
    let replacement: HTMLButtonElement | undefined;
    if (ownership === "moved during insertion") {
      replacement = document.createElement("button");
      replacement.textContent = "New focus owner";
      document.body.prepend(replacement);
      const observer = new MutationObserver(() => {
        if (!document.querySelector(insertionSelector)) return;
        if (
          phase === "work setup view" &&
          document.querySelector<HTMLElement>('[aria-labelledby="work-window-heading"]')?.style
            .display === "none"
        )
          return;
        observer.disconnect();
        replacement?.focus();
      });
      observer.observe(document.body, { childList: true, subtree: true, attributes: true });
      onTestFinished(() => {
        observer.disconnect();
        replacement?.remove();
      });
    }
    if (phase === "work setup view") {
      // Save selects this same view after native reads, without moving a user's later focus.
      screen.getByRole("button", { name: messages.work.instances }).click();
    } else matrix.resolve(undefined);
    await expect.poll(() => document.querySelector(insertionSelector)).not.toBeNull();
    await nextTick();
    if (phase === "overview") {
      await expect.poll(() => workPending).toBe(true);
      if (target === "metadata" && ownership === "retained") {
        expect(input.getBoundingClientRect().top).toBeLessThan(innerHeight);
        expect(input.getBoundingClientRect().bottom).toBeGreaterThan(0);
      }
      laterWork.resolve(undefined);
      await expect
        .poll(() =>
          document.querySelector('[aria-labelledby="assignment-inspection-heading"] .action-row'),
        )
        .not.toBeNull();
      await nextTick();
    }
    const focused = replacement ?? input;
    expect(document.activeElement).toBe(focused);
    if (ownership === "scrolled away before insertion") {
      expect(scrollY).toBe(0);
      expect(focused.getBoundingClientRect().top).toBeGreaterThan(innerHeight);
    } else if (target === "outside workspace") {
      expect(focused.getBoundingClientRect().top).toBeGreaterThan(innerHeight);
    } else {
      expect(focused.getBoundingClientRect().top).toBeLessThan(innerHeight);
      expect(focused.getBoundingClientRect().bottom).toBeGreaterThan(0);
    }
  },
);
