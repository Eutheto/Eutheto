import { cleanup, render, screen } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { afterEach, expect, it, onTestFinished, vi } from "vitest";
import { defineComponent, h, nextTick, ref } from "vue";
import { createMemoryHistory, createRouter, RouterView } from "vue-router";
import * as api from "../api/generated";
import type { WorkforceSetupQueryResults } from "../api/generated-domain-pack-contracts";
import { createProjectHomeController } from "../project-home";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import EligibilitySetup from "./EligibilitySetup.vue";
import "../styles.css";

vi.mock("../api/generated", { spy: true });
const personId = "01900000-0000-7000-8000-000000000011";
const typeId = "01900000-0000-7000-8000-000000000012";
const dates = { startDate: "2030-01-01", endDateExclusive: "2030-01-08" };
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
    result: { kind: "workWindow", data: { items: [], totalItems: 0, continuation: null } },
  },
} satisfies Partial<WorkforceSetupQueryResults>;

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it.each(["retained", "moved during insertion", "scrolled away before insertion"])(
  "keeps inspector focus ownership when its delayed matrix returns: %s",
  async (ownership) => {
    const currentProject = ref({ ...project, domainPackId: "official.workforce" });
    const native = fakeApi([currentProject.value]);
    const matrix = Promise.withResolvers<undefined>();
    let delayMatrix = false;
    let matrixPending = false;
    function receipt<Id extends keyof typeof views>(viewId: Id) {
      return response({
        schemaVersion: 2 as const,
        scenarioId: project.scenarioId,
        revision: currentProject.value.revision,
        view: { viewId, data: views[viewId] },
      });
    }
    vi.mocked(api.searchScenarioEntities).mockImplementation(() =>
      portableOperation(receipt("eutheto.setup.entity_page")),
    );
    vi.mocked(api.getScenarioView).mockImplementation((_scope, input) => {
      switch (input.query.viewId) {
        case "official.workforce.setup.people_page":
          return portableOperation(receipt("official.workforce.setup.people_page"));
        case "official.workforce.setup.overview":
          return portableOperation(receipt("official.workforce.setup.overview"));
        case "official.workforce.setup.work_window":
          return portableOperation(receipt("official.workforce.setup.work_window"));
        case "official.workforce.setup.eligibility_matrix": {
          const result = receipt("official.workforce.setup.eligibility_matrix");
          if (delayMatrix) {
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
        return () => h(EligibilitySetup, { home, project: currentProject.value });
      },
    });
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: "/project/:scenarioId/eligibility", component: Page },
        {
          path: "/project/:scenarioId/import",
          name: "project-eligibility-import",
          component: Page,
        },
        {
          path: "/project/:scenarioId/matrix",
          name: "project-eligibility-matrix-import",
          component: Page,
        },
        { path: "/project/:scenarioId/history", name: "project-history", component: Page },
      ],
    });
    await router.push(`/project/${project.scenarioId}/eligibility`);
    render(RouterView, { global: { plugins: [createPinia(), PiniaColada, router] } });
    await screen.findByRole("checkbox", { name: "Person — Type" });
    await expect.poll(() => document.querySelector("#inspection-start-date")).not.toBeNull();
    delayMatrix = true;
    currentProject.value = { ...currentProject.value, revision: project.revision + 1 };
    await expect.poll(() => matrixPending).toBe(true);
    await expect.poll(() => document.querySelector("#inspection-start-date")).not.toBeNull();
    const input = document.querySelector<HTMLInputElement>("#inspection-start-date");
    if (!input) throw new Error("Missing inspector date");
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
        if (!document.querySelector("[data-eligibility-matrix]")) return;
        observer.disconnect();
        replacement?.focus();
      });
      observer.observe(document.body, { childList: true, subtree: true });
      onTestFinished(() => {
        observer.disconnect();
        replacement?.remove();
      });
    }
    matrix.resolve(undefined);
    await screen.findByRole("checkbox", { name: "Person — Type" });
    await nextTick();
    const focused = replacement ?? input;
    expect(document.activeElement).toBe(focused);
    if (ownership === "scrolled away before insertion") {
      expect(scrollY).toBe(0);
      expect(focused.getBoundingClientRect().top).toBeGreaterThan(innerHeight);
    } else {
      expect(focused.getBoundingClientRect().top).toBeLessThan(innerHeight);
      expect(focused.getBoundingClientRect().bottom).toBeGreaterThan(0);
    }
  },
);
