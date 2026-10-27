import { cleanup, render, screen, within } from "@testing-library/vue";
import { flushPromises } from "@vue/test-utils";
import { userEvent } from "vitest/browser";
import { afterEach, expect, it, onTestFinished, vi } from "vitest";
import { computed, shallowReactive } from "vue";
import { RouterView, createMemoryHistory, createRouter } from "vue-router";
import {
  explainSolution,
  type ApiResponseDto,
  type SolutionExplanationDtoV2,
  type ValidationIssue,
} from "../api/generated";
import { useFullValidation } from "../full-validation";
import { messages } from "../messages";
import type { ProjectHomeController } from "../project-home";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import ValidationSetup from "./ValidationSetup.vue";
import "../styles.css";

vi.mock("../api/generated", { spy: true });
vi.mock("../full-validation", { spy: true });
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

const first: ValidationIssue = {
  code: "official.workforce.validation.same_code",
  severity: "error",
  message: "First source conflict.",
  fieldPath: null,
  resource: null,
};
const second: ValidationIssue = { ...first, message: "Second source conflict." };
const scenario = { ...project, domainPackId: "official.workforce" };
const digest = "a".repeat(64);
const firstId = `validation.issue.${"b".repeat(64)}`;
const secondId = `validation.issue.${"c".repeat(64)}`;
const coverageRuleId = "01900000-0000-7000-8000-000000000094";
const shortage: ValidationIssue = {
  code: "official.workforce.candidate_shortage",
  severity: "error",
  message: "Required coverage has an obvious contradiction.",
  fieldPath: "domain.entities.01900000-0000-7000-8000-000000000008.coverage.count",
  resource: { type: "rule", id: coverageRuleId },
};
const shortageId = `validation.issue.${"d".repeat(64)}`;

function sourceConflict(possible: string): SolutionExplanationDtoV2 {
  const original = nativeExplanation(shortage, shortageId);
  return {
    ...original,
    explanation: {
      ...original.explanation,
      rendered: {
        ...original.explanation.rendered,
        messages: [
          ...original.explanation.rendered.messages,
          {
            messageKey: "official.workforce.validation.coverage_shortage_proof",
            parameters: {
              "official.workforce.fact.summary": {
                type: "text",
                value: "The original person checks bound staffing below required coverage.",
              },
              "official.workforce.fact.required_count": { type: "integer", value: "2" },
              "official.workforce.fact.possible_people": { type: "integer", value: possible },
            },
            entities: [],
            rules: [coverageRuleId],
            assignments: [],
            evidence: [],
          },
        ],
      },
    },
  };
}

function nativeExplanation(issue: ValidationIssue, issueId: string): SolutionExplanationDtoV2 {
  return {
    schemaVersion: 2,
    scenarioId: scenario.scenarioId,
    currentRevision: scenario.revision,
    scenarioRevisions: [scenario.revision],
    explanation: {
      schemaVersion: 2,
      checksum: digest,
      evidence: {
        schemaVersion: 2,
        certainty: "deterministic",
        checksum: digest,
        evidence: {
          kind: "validation",
          issue: {
            issueId,
            severity: "mustFix",
            messageKey: issue.code,
            parameters: {},
            fieldPath: null,
            entity: null,
            ruleId: null,
          },
        },
      },
      rendered: {
        schemaVersion: 1,
        kind: "validation",
        evidenceChecksum: digest,
        messages: [
          {
            messageKey: issue.code,
            parameters: {
              "official.workforce.fact.summary": { type: "text", value: issue.message },
            },
            entities: [],
            rules: [],
            assignments: [],
            evidence: [],
          },
        ],
      },
    },
  };
}

async function mount() {
  fakeApi([scenario]);
  const home = {
    state: shallowReactive({ libraryEpoch: 4, busyAction: null, operation: null }),
  } as unknown as ProjectHomeController;
  const validation = shallowReactive({
    status: {
      schemaVersion: 2 as const,
      scenarioId: scenario.scenarioId,
      revision: scenario.revision,
      structure: { entities: 0, rules: 0, preferences: 0, lockedAssignments: 0 },
      fast: {
        counts: { errors: 1, warnings: 0, information: 0 },
        issues: [first],
        omitted: 0,
      },
      full: { state: "completed" as const, inputRevision: scenario.revision, stale: false },
    },
    report: {
      schemaVersion: 3 as const,
      scenarioId: scenario.scenarioId,
      revision: scenario.revision,
      report: { issues: [first, second, shortage] },
      issueIds: [firstId, secondId, shortageId],
    },
    loading: false,
    error: null,
    run: "completed" as const,
    failure: null,
  });
  vi.mocked(useFullValidation).mockReturnValue({
    state: validation,
    supported: computed(() => true),
    busy: computed(() => false),
    run: vi.fn(),
    refreshStatus: vi.fn(),
  } as unknown as ReturnType<typeof useFullValidation>);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/", component: ValidationSetup, props: { home, project: scenario } }],
  });
  onTestFinished(() => {
    router.options.history.destroy();
  });
  await router.push("/");
  render(RouterView, { global: { plugins: [router] } });
  await router.isReady();
  return { home, validation };
}

it("checks only the selected full-report occurrence despite another finding with the same code", async () => {
  await mount();
  const fast = within(screen.getByRole("region", { name: messages.setup.fast }));
  await userEvent.click(fast.getByRole("button", { name: /First source conflict/ }));
  expect(screen.queryByRole("button", { name: messages.validationWorkspace.explain })).toBeNull();

  const full = within(screen.getByRole("region", { name: messages.validationWorkspace.report }));
  await userEvent.click(full.getByRole("button", { name: /Second source conflict/ }));
  vi.mocked(explainSolution).mockReturnValue(
    portableOperation(response(nativeExplanation(second, secondId))),
  );
  await userEvent.click(screen.getByRole("button", { name: messages.validationWorkspace.explain }));
  expect(
    await screen.findByRole("region", { name: messages.validationWorkspace.explanationHeading }),
  ).toMatchTextContent(second.message);

  await userEvent.click(full.getByRole("button", { name: /First source conflict/ }));
  vi.mocked(explainSolution).mockReturnValue(
    portableOperation(response(nativeExplanation(second, firstId))),
  );
  await userEvent.click(screen.getByRole("button", { name: messages.validationWorkspace.explain }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    messages.validationWorkspace.explanationChanged,
  );
  expect(
    screen.queryByRole("region", { name: messages.validationWorkspace.explanationHeading }),
  ).toBeNull();
});

it("shows only source-bound native count contradictions and refuses impossible proof counts", async () => {
  await mount();
  const full = within(screen.getByRole("region", { name: messages.validationWorkspace.report }));
  await userEvent.click(full.getByRole("button", { name: new RegExp(shortage.message) }));
  vi.mocked(explainSolution).mockReturnValue(portableOperation(response(sourceConflict("1"))));
  await userEvent.click(screen.getByRole("button", { name: messages.validationWorkspace.explain }));
  const proof = await screen.findByRole("region", {
    name: messages.validationWorkspace.explanationProofHeading,
  });
  expect(proof).toMatchTextContent(messages.validationWorkspace.explanationProofCount("1", "2"));
  expect(proof).toMatchTextContent(coverageRuleId);
  expect(screen.getByText(messages.validationWorkspace.explanationProofBoundary)).toBeVisible();

  vi.mocked(explainSolution).mockReturnValue(portableOperation(response(sourceConflict("2"))));
  await userEvent.click(screen.getByRole("button", { name: messages.validationWorkspace.explain }));
  expect(await screen.findByText(messages.validationWorkspace.explanationChanged)).toHaveAttribute(
    "role",
    "alert",
  );
  expect(
    screen.queryByRole("region", { name: messages.validationWorkspace.explanationProofHeading }),
  ).toBeNull();
});

it("discards evidence returned after the saved project's library epoch changes", async () => {
  const { home } = await mount();
  const full = within(screen.getByRole("region", { name: messages.validationWorkspace.report }));
  await userEvent.click(full.getByRole("button", { name: /Second source conflict/ }));
  const { promise, resolve } = Promise.withResolvers<ApiResponseDto<SolutionExplanationDtoV2>>();
  vi.mocked(explainSolution).mockReturnValue(portableOperation(promise));
  await userEvent.click(screen.getByRole("button", { name: messages.validationWorkspace.explain }));
  home.state.libraryEpoch += 1;
  resolve(response(nativeExplanation(second, secondId)));
  await flushPromises();
  expect(
    screen.queryByRole("region", { name: messages.validationWorkspace.explanationHeading }),
  ).toBeNull();
  expect(screen.queryByRole("button", { name: messages.validationWorkspace.explain })).toBeNull();
});
