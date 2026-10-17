import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { cleanup, render, screen } from "@testing-library/vue";
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { defineComponent, h, onScopeDispose, reactive } from "vue";
import * as api from "../api/generated";
import type {
  AcceptedResultRefV1,
  ApiResponseDto,
  AppCapabilitiesDto,
  ApplicationSettingsSnapshotV1,
  DiagnosticCopyResultV1,
  DiagnosticExportMetadataV1,
  ProjectListItemV1,
  ScenarioSummaryV2,
  SetupOperation,
  SolutionExplanationDtoV2,
  SolutionListDtoV1,
  SolutionSummaryDtoV1,
  SolveCompletedEvent,
  SolveEstimateModelResultV1,
  SolveJobDtoV2,
  SolveDiagnosticsSummaryDtoV2,
  SolveProgressEvent,
  SolveVerifiedEvent,
  SolveStartDtoV2,
} from "../api/generated";
import optimalityStatus from "../api/fixtures/optimality-status-native.json";
import {
  createProjectHomeController,
  type ProjectHomeController,
  type ProjectHomeState,
  type ProjectSummary,
  type WorkspaceOperation,
} from "../project-home";
import { createAppRouter } from "../router";
import { fakeApi, portableOperation, project as projectFixture } from "../testing/project-home";
import {
  WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
  type WorkforceSetupFacts,
} from "../api/generated-domain-pack-contracts";
import { explanationMessage } from "./explanations/messages";
import { messages } from "../messages";
import OptimizeCapabilityPanel from "./OptimizeCapabilityPanel.vue";
import WorkforceSetupOverview from "./WorkforceSetupOverview.vue";
import "../styles.css";

vi.mock("../api/generated", { spy: true });

const scenarioId = "01900000-0000-7000-8000-000000000001";
const operationId = "01900000-0000-7000-8000-000000000002";
const runId = "01900000-0000-7000-8000-000000000003";
const projectRevision = 3;

function response<T>(
  result: T,
  currentRevision: number | null = projectRevision,
): ApiResponseDto<T> {
  return {
    schemaVersion: 1,
    requestId: "01900000-0000-7000-8000-000000000004",
    currentRevision,
    warnings: [],
    result,
  };
}
function summary(
  revision = projectRevision,
): Pick<ScenarioSummaryV2, "revision" | "fast" | "full"> {
  return {
    revision,
    fast: { counts: { errors: 0, warnings: 0, information: 0 }, issues: [], omitted: 0 },
    full: {
      state: "completed",
      operationId,
      inputRevision: revision,
      stale: false,
      counts: { errors: 0, warnings: 0, information: 0 },
    },
  };
}
function job(status: SolveJobDtoV2["status"]): SolveJobDtoV2 {
  return {
    schemaVersion: 2,
    requestId: "01900000-0000-7000-8000-000000000005",
    runId,
    scenarioId,
    scenarioRevision: projectRevision,
    status,
    startedAt: "2026-09-10T12:00:00Z",
    finishedAt: status === "running" ? null : "2026-09-10T12:00:03Z",
    solutionId: status === "feasible" || status === "optimal" ? operationId : null,
    verificationChecksum: status === "feasible" || status === "optimal" ? "a".repeat(64) : null,
    checkpoint: null,
    cancelOperationId: null,
    stale: false,
  };
}
function mockCheckpointSchedule(result: { solutionId: string; resultChecksum: string }): void {
  vi.mocked(api.getSolutionView).mockResolvedValue(
    response({
      schemaVersion: 1,
      scenarioId,
      currentRevision: projectRevision,
      scenarioRevision: projectRevision,
      result,
      view: {
        viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
        data: {
          schemaVersion: 1,
          timeZone: "America/New_York",
          result: {
            kind: "people",
            totalItems: 1,
            items: [{ personId: operationId, name: "River", selectedCount: 1 }],
            continuation: null,
          },
        },
      },
    }),
  );
}
function terminalExplanationResponse(
  terminalStatus: SolveJobDtoV2["status"],
  result: AcceptedResultRefV1 | null,
  sourceRevision = projectRevision,
  currentRevision = projectRevision,
  manifestChecksum = "m".repeat(64),
): ApiResponseDto<SolutionExplanationDtoV2> {
  const explanation = structuredClone(optimalityStatus) as unknown as SolutionExplanationDtoV2;
  const payload = explanation.explanation.evidence.evidence;
  if (payload.kind !== "optimalityStatus") throw new Error("Fixture is not an optimality status");
  Object.assign(explanation, {
    schemaVersion: 2,
    scenarioId,
    currentRevision,
    scenarioRevisions: [sourceRevision],
  });
  Object.assign(explanation.explanation, { schemaVersion: 2 });
  Object.assign(explanation.explanation.evidence, { schemaVersion: 2 });
  Object.assign(payload.status.runInput, {
    runId,
    scenarioId,
    scenarioRevision: sourceRevision,
  });
  Object.assign(payload.status, { schemaVersion: 2 });
  const outcome =
    result !== null
      ? {
          type: "accepted" as const,
          status: terminalStatus as "optimal" | "feasible",
          solutionId: result.solutionId,
          acceptedResultChecksum: result.resultChecksum,
          verificationChecksum: "v".repeat(64),
        }
      : terminalStatus === "verificationAlarm"
        ? { type: "verificationAlarm" as const, diagnosticCode: "verify.failed" }
        : terminalStatus === "interrupted"
          ? { type: "interrupted" as const }
          : {
              type: "noResult" as const,
              status: terminalStatus as
                | "infeasible"
                | "unbounded"
                | "noSolutionWithinLimit"
                | "cancelled"
                | "invalidModel"
                | "backendUnavailable"
                | "backendFailed",
            };
  Object.assign(payload.status.runManifest, {
    runId,
    checksum: manifestChecksum,
    outcome,
  });
  Object.assign(payload.status, { result });
  Object.assign(explanation.explanation.evidence, {
    certainty: result === null ? "deterministic" : "independentlyVerified",
  });
  Object.assign(explanation.explanation.rendered, {
    messages: [
      {
        messageKey: "official.workforce.optimality_status.backend_recorded_status",
        parameters: {
          "official.workforce.fact.summary": {
            type: "text",
            value:
              result === null
                ? "Recorded backend status: no accepted result."
                : `Recorded terminal status: ${terminalStatus}; an accepted result is independently verified separately.`,
          },
        },
        entities: [],
        rules: [],
        assignments: [],
        evidence: [],
      },
    ],
  });
  return response(explanation, currentRevision);
}

function mappedInfeasibilityResponse(
  manifestChecksum = "m".repeat(64),
  contextManifestChecksum = manifestChecksum,
  sourceRevision = projectRevision,
  currentRevision = projectRevision,
): ApiResponseDto<SolutionExplanationDtoV2> {
  const evidenceChecksum = "e".repeat(64);
  const explanationEvidenceChecksum = "f".repeat(64);
  const groupId = "coverage.group.1";
  const requiredRuleIds = [operationId];
  const message = (
    messageKey: string,
    parameters: Record<string, { type: "text" | "integer"; value: string }>,
    rules: string[] = [],
  ) => ({ messageKey, parameters, entities: [], rules, assignments: [], evidence: [] });
  return response(
    {
      schemaVersion: 2,
      scenarioId,
      currentRevision,
      scenarioRevisions: [sourceRevision],
      explanation: {
        schemaVersion: 2,
        evidence: {
          schemaVersion: 2,
          certainty: "sufficientConflict",
          evidence: {
            kind: "infeasibility",
            infeasibility: {
              type: "conflict",
              groups: [{ groupId, requiredRules: requiredRuleIds }],
              minimality: "sufficient",
              shrink: {
                initialGroupCount: 1,
                remainingGroupCount: 1,
                attemptedTrials: 0,
                maxTrials: 0,
                stopReason: "notAttempted",
              },
            },
          },
          checksum: explanationEvidenceChecksum,
        },
        rendered: {
          schemaVersion: 1,
          kind: "infeasibility",
          evidenceChecksum: explanationEvidenceChecksum,
          messages: [
            message("official.workforce.infeasibility.status", {
              "official.workforce.fact.summary": {
                type: "text",
                value:
                  "The recorded mapped conflict is sufficient for this historical infeasibility result.",
              },
              "official.workforce.fact.status": { type: "text", value: "sufficient" },
            }),
            message("official.workforce.infeasibility.fixed_scope", {
              "official.workforce.fact.summary": {
                type: "text",
                value:
                  "Fixed activity and approved-leave facts, together with hard locks, remain enforced.",
              },
              "official.workforce.fact.fixed_fact_count": { type: "integer", value: "2" },
              "official.workforce.fact.hard_lock_count": { type: "integer", value: "1" },
            }),
            message(
              "official.workforce.infeasibility.conflict_group",
              {
                "official.workforce.fact.group_id": { type: "text", value: groupId },
                "official.workforce.fact.source_caption": {
                  type: "text",
                  value: "Coverage: at least one qualified person must be assigned.",
                },
                "official.workforce.fact.source_kind": {
                  type: "text",
                  value: "official.workforce.rule.coverage",
                },
                "official.workforce.fact.source_rule_count": {
                  type: "integer",
                  value: String(requiredRuleIds.length),
                },
              },
              requiredRuleIds,
            ),
          ],
        },
        checksum: "g".repeat(64),
      },
      infeasibilityContext: {
        runId,
        sourceRevision,
        runManifestChecksum: contextManifestChecksum,
        evidenceChecksum,
        conflictId: `infeasibility.${evidenceChecksum}`,
      },
    } as unknown as SolutionExplanationDtoV2,
    currentRevision,
  );
}

function acceptedSolution(
  result: AcceptedResultRefV1,
  sourceRevision = projectRevision,
  currentRevision = projectRevision,
  status: "optimal" | "feasible" = "feasible",
): SolutionSummaryDtoV1 {
  return {
    schemaVersion: 1,
    result,
    runId,
    scenarioId,
    scenarioRevision: sourceRevision,
    currentRevision,
    stale: sourceRevision !== currentRevision,
    status,
    selected: false,
    score: { feasibility: "0", levels: [] },
    verificationChecksum: "v".repeat(64),
    finishedAt: "2026-09-10T12:00:03Z",
  };
}

const capabilities: AppCapabilitiesDto = {
  availableCommands: [
    "solve_get_backend_options",
    "solve_estimate_model",
    "solve_start",
    "solve_cancel",
    "solve_get_job",
    "solve_get_diagnostics_summary",
    "solve_list_runs",
  ],
  unavailableCommands: [],
};
const noWorkerCapabilities: AppCapabilitiesDto = {
  availableCommands: ["solve_estimate_model"],
  unavailableCommands: [
    "solve_get_backend_options",
    "solve_start",
    "solve_cancel",
    "solve_get_job",
    "solve_list_runs",
  ],
};

interface SetupOptions {
  readonly capabilities?: AppCapabilitiesDto;
  readonly revision?: number;
  readonly home?: ProjectHomeController;
}

function diagnosticHome() {
  const state = reactive({
    busyAction: null as string | null,
    operation: null as ProjectHomeState["operation"],
    libraryEpoch: 0,
  });
  const operations: Array<Pick<WorkspaceOperation<unknown>, "action" | "refreshLibrary">> = [];
  const runOperation = vi.fn(
    async <T>(operation: WorkspaceOperation<T>): Promise<ApiResponseDto<T>> => {
      operations.push({
        action: operation.action,
        ...(operation.refreshLibrary === undefined
          ? {}
          : { refreshLibrary: operation.refreshLibrary }),
      });
      state.busyAction = operation.action;
      state.operation = {
        label: operation.label,
        cancel: operation.cancel ?? null,
        phase: null,
        cancellationRequested: false,
        settled: false,
        refreshingLibrary: false,
      };
      try {
        const result = await operation.execute(() => undefined);
        state.operation.settled = true;
        operation.success(result.result);
        if (operation.refreshLibrary !== false) state.libraryEpoch += 1;
        return result;
      } finally {
        state.operation = null;
        state.busyAction = null;
      }
    },
  );
  const home = {
    state,
    runOperation,
    cancelOperation: vi.fn(() => Promise.resolve()),
  } as unknown as ProjectHomeController;
  return { home, state, operations, runOperation };
}

function setup(retained: SolveJobDtoV2[] = [], options: SetupOptions = {}) {
  const revision = options.revision ?? projectRevision;
  let progressListener: ((event: SolveProgressEvent) => void) | undefined;
  let verifiedListener: ((event: SolveVerifiedEvent) => void) | undefined;
  let completedListener: ((event: SolveCompletedEvent) => void) | undefined;
  const release = vi.fn(() =>
    Promise.resolve(response({ schemaVersion: 1 as const, acknowledgement: "released" as const })),
  );
  const estimateRelease = vi.fn(() =>
    Promise.resolve(response({ schemaVersion: 1 as const, acknowledgement: "released" as const })),
  );
  const estimateCancel = vi.fn(() =>
    Promise.resolve(
      response({ schemaVersion: 1 as const, acknowledgement: "cancellationRequested" as const }),
    ),
  );
  const dispose = vi.fn();
  const scopes: Array<readonly [string, number | null]> = [];
  const startResult = Promise.withResolvers<ApiResponseDto<SolveStartDtoV2>>();
  const estimateResult = Promise.withResolvers<ApiResponseDto<SolveEstimateModelResultV1>>();
  const operation: api.SolveStartOperation = {
    requestId: "01900000-0000-7000-8000-000000000006",
    operationId: Promise.resolve(operationId),
    result: startResult.promise,
    release,
    isCurrent: () => true,
  };
  const estimateOperation: SetupOperation<SolveEstimateModelResultV1> = {
    requestId: "01900000-0000-7000-8000-000000000008",
    operationId: Promise.resolve(operationId),
    result: estimateResult.promise,
    cancel: estimateCancel,
    release: estimateRelease,
    isCurrent: () => true,
  };
  vi.mocked(api.SetupOperationScope).mockImplementation(
    class {
      constructor(scenario: string, revision: number | null) {
        scopes.push([scenario, revision]);
      }
      dispose = dispose;
    } as unknown as typeof api.SetupOperationScope,
  );
  vi.mocked(api.getSolveBackendOptions).mockResolvedValue(
    response({
      schemaVersion: 2,
      scenarioId,
      revision,
      options: [
        {
          backendId: "solver.fixture",
          backendVersion: "1.0.0",
          adapterVersion: "1.0.0",
          displayName: "Fixture solver",
        },
      ],
    }),
  );
  vi.mocked(api.listSolveRuns).mockResolvedValue(
    response({ schemaVersion: 2, scenarioId, currentRevision: revision, runs: retained }),
  );
  vi.mocked(api.onSolveProgress).mockImplementation((listener) => {
    progressListener = listener;
    return Promise.resolve(vi.fn());
  });
  vi.mocked(api.onSolveVerified).mockImplementation((listener) => {
    verifiedListener = listener;
    return Promise.resolve(vi.fn());
  });
  vi.mocked(api.onSolveCompleted).mockImplementation((listener) => {
    completedListener = listener;
    return Promise.resolve(vi.fn());
  });
  vi.mocked(api.startSolve).mockReturnValue(operation);
  vi.mocked(api.estimateSolveModel).mockReturnValue(estimateOperation);
  vi.mocked(api.cancelSolve).mockResolvedValue(
    response({
      schemaVersion: 2,
      cancelRequestId: "01900000-0000-7000-8000-000000000007",
      acknowledgement: "cancellationRequested",
    }),
  );
  const terminal = vi.fn();
  const rendered = render(OptimizeCapabilityPanel, {
    props: {
      scenarioId,
      summary: summary(revision),
      currentRevision: revision,
      capabilities: options.capabilities ?? capabilities,
      loading: false,
      error: null,
      onTerminal: terminal,
      home: options.home,
    },
  });
  return {
    rendered,
    scopes,
    dispose,
    release,
    startResult,
    operation,
    estimateResult,
    estimateOperation,
    estimateCancel,
    estimateRelease,
    terminal,
    get progressListener() {
      return progressListener;
    },
    get verifiedListener() {
      return verifiedListener;
    },
    get completedListener() {
      return completedListener;
    },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Optimize capability lifecycle", () => {
  it("keeps model estimation available when solve execution is unavailable", async () => {
    const state = setup([], { capabilities: noWorkerCapabilities });
    const estimate = await screen.findByRole("button", {
      name: messages.setup.optimize.estimateModel,
    });
    await expect.element(estimate).toBeEnabled();
    expect(screen.queryByRole("button", { name: messages.setup.optimize.solveStart })).toBeNull();
    await expect
      .element(await screen.findByText(messages.setup.optimize.executionUnavailable))
      .toBeVisible();
    const technical = document.querySelector<HTMLDetailsElement>("[data-optimize-capabilities]");
    expect(technical?.open).toBe(false);
    await userEvent.click(screen.getByText(messages.setup.optimize.technicalCapabilities));
    expect(technical?.open).toBe(true);
    const startCapability = technical?.querySelector("dd");
    if (!startCapability) throw new Error("No reported start capability");
    await expect.element(startCapability).toBeVisible();
    expect(startCapability.textContent).toBe(messages.setup.optimize.unavailable);

    await userEvent.click(estimate);
    expect(state.scopes).toContainEqual([scenarioId, projectRevision]);
    state.estimateResult.resolve(
      response({
        schemaVersion: 1,
        scenarioId,
        scenarioRevision: projectRevision,
        currentRevision: projectRevision,
        stale: false,
        compiled: true,
        solved: false,
        candidateCount: 101,
        variableCount: 202,
        constraintCount: 303,
        projectionCount: 404,
        objectiveTermCount: 505,
      }),
    );
    await state.estimateOperation.result;
    await expect
      .element(await screen.findByText(messages.setup.optimize.estimateCompiled))
      .toBeVisible();
    for (const count of ["101", "202", "303", "404", "505"])
      await expect.element(screen.getByText(count)).toBeVisible();
  });

  it("gates estimation on its own catalog capability", async () => {
    setup([], {
      capabilities: {
        availableCommands: [],
        unavailableCommands: [
          "solve_get_backend_options",
          "solve_estimate_model",
          "solve_start",
          "solve_cancel",
          "solve_get_job",
          "solve_list_runs",
        ],
      },
    });
    await expect
      .element(await screen.findByRole("button", { name: messages.setup.optimize.estimateModel }))
      .toBeDisabled();
    await expect
      .element(await screen.findByText(messages.setup.optimize.estimateUnavailable))
      .toBeVisible();
  });

  it("labels native response revisions without presenting stale counts as current", async () => {
    const state = setup();
    await userEvent.click(
      await screen.findByRole("button", { name: messages.setup.optimize.estimateModel }),
    );
    state.estimateResult.resolve(
      response(
        {
          schemaVersion: 1,
          scenarioId,
          scenarioRevision: projectRevision,
          currentRevision: projectRevision + 1,
          stale: true,
          compiled: true,
          solved: false,
          candidateCount: 111,
          variableCount: 222,
          constraintCount: 333,
          projectionCount: 444,
          objectiveTermCount: 555,
        },
        projectRevision + 1,
      ),
    );
    await state.estimateOperation.result;
    await expect
      .element(await screen.findByText(messages.setup.optimize.estimateStale))
      .toBeVisible();
    await expect
      .element(
        await screen.findByText(
          messages.setup.optimize.estimateResponseRevision(String(projectRevision + 1)),
        ),
      )
      .toBeVisible();
    await expect.element(screen.getByText("111")).toBeVisible();
  });

  it("cancels and ignores a late estimate response after the revision changes", async () => {
    const state = setup();
    await userEvent.click(
      await screen.findByRole("button", { name: messages.setup.optimize.estimateModel }),
    );
    await state.rendered.rerender({ currentRevision: projectRevision + 1, summary: summary(4) });
    await vi.waitFor(() => {
      expect(state.estimateCancel).toHaveBeenCalled();
    });
    state.estimateResult.resolve(
      response({
        schemaVersion: 1,
        scenarioId,
        scenarioRevision: projectRevision,
        currentRevision: projectRevision,
        stale: false,
        compiled: true,
        solved: false,
        candidateCount: 999,
        variableCount: 999,
        constraintCount: 999,
        projectionCount: 999,
        objectiveTermCount: 999,
      }),
    );
    await state.estimateOperation.result;
    expect(screen.queryByText("999")).toBeNull();
    expect(state.estimateRelease).toHaveBeenCalled();
  });

  it("offers modes, reports native phases, and cancels through solve_cancel", async () => {
    const state = setup();
    const start = await screen.findByRole("button", { name: messages.setup.optimize.solveStart });
    await userEvent.click(screen.getByRole("radio", { name: messages.setup.optimize.deep }));
    await userEvent.click(start);
    const progress = state.progressListener;
    if (!progress) throw new Error("Progress listener was not registered");
    progress({
      type: "solveProgress",
      payload: {
        context: {
          eventVersion: 1,
          timestamp: "2026-09-10T12:00:01Z",
          requestId: state.operation.requestId,
          scenarioId,
          revision: projectRevision,
          solveRunId: runId,
        },
        phase: "solving",
        percent: null,
      },
    });
    await expect
      .element(
        await screen.findByText(
          `${messages.setup.optimize.phase}: ${messages.setup.optimize.phases.solving}`,
        ),
      )
      .toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: messages.setup.optimize.solveCancel }),
    );
    await expect
      .element(screen.getByRole("button", { name: messages.setup.optimize.solveCancelling }))
      .toBeDisabled();
    state.startResult.resolve(
      response({
        schemaVersion: 2,
        currentRevision: projectRevision,
        reused: false,
        job: job("feasible"),
      }),
    );
    await state.operation.result;
    const completed = state.completedListener;
    completed?.({
      type: "solveCompleted",
      payload: {
        context: {
          eventVersion: 1,
          timestamp: "2026-09-10T12:00:03Z",
          requestId: state.operation.requestId,
          scenarioId,
          revision: projectRevision,
          solveRunId: runId,
        },
        status: "feasible",
        solutionId: operationId,
      },
    });
    await expect
      .element(await screen.findByText(messages.setup.optimize.acceptedNotice))
      .toBeVisible();
    expect(state.terminal).toHaveBeenCalledWith(job("feasible"));
  });

  it("does not flash a live progress announcement when a verified solve finishes quickly", async () => {
    const state = setup();
    const start = await screen.findByRole("button", { name: messages.setup.optimize.solveStart });
    const announcements: string[] = [];
    const observer = new MutationObserver(() => {
      if (screen.queryByText(messages.setup.optimize.running))
        announcements.push(messages.setup.optimize.running);
    });
    observer.observe(document.body, { childList: true, subtree: true, characterData: true });
    try {
      await userEvent.click(start);
      expect(screen.queryByText(messages.setup.optimize.running)).toBeNull();
      state.startResult.resolve(
        response({
          schemaVersion: 2,
          currentRevision: projectRevision,
          reused: false,
          job: job("feasible"),
        }),
      );
      await expect
        .element(await screen.findByText(messages.setup.optimize.acceptedNotice))
        .toBeVisible();
      await Promise.resolve();
      expect(announcements).toEqual([]);
      expect(state.terminal).toHaveBeenCalledWith(job("feasible"));
    } finally {
      observer.disconnect();
    }
  });

  it("shows an independently checked schedule before proof and retains it after cancellation", async () => {
    const state = setup();
    const checkpoint = {
      result: { solutionId: operationId, resultChecksum: "b".repeat(64) },
      verifiedAfterMilliseconds: 430,
      acceptedAt: "2026-09-10T12:00:01Z",
      checkpointChecksum: "c".repeat(64),
    };
    const running = {
      ...job("running"),
      requestId: state.operation.requestId,
      checkpoint,
    };
    vi.mocked(api.getSolveJob).mockResolvedValue(response(running));
    mockCheckpointSchedule(checkpoint.result);
    await userEvent.click(
      await screen.findByRole("button", { name: messages.setup.optimize.solveStart }),
    );
    await vi.waitFor(() => {
      expect(state.verifiedListener).toBeTypeOf("function");
    });
    state.verifiedListener?.({
      type: "solveVerified",
      payload: {
        context: {
          eventVersion: 1,
          timestamp: "2026-09-10T12:00:01.500Z",
          requestId: state.operation.requestId,
          scenarioId,
          revision: projectRevision,
          solveRunId: runId,
        },
        solutionId: checkpoint.result.solutionId,
        resultChecksum: checkpoint.result.resultChecksum,
        checkpointChecksum: checkpoint.checkpointChecksum,
      },
    });
    await expect
      .element(await screen.findByText(messages.setup.optimize.firstVerified))
      .toBeVisible();
    await expect.element(await screen.findByText("River")).toBeVisible();
    await expect
      .element(screen.getByRole("button", { name: messages.setup.optimize.solveCancel }))
      .toBeEnabled();
    expect(state.terminal).not.toHaveBeenCalled();
    await vi.waitFor(
      () => {
        expect(api.getSolveJob).toHaveBeenCalledTimes(2);
      },
      { timeout: 3_500 },
    );

    state.completedListener?.({
      type: "solveCompleted",
      payload: {
        context: {
          eventVersion: 1,
          timestamp: "2026-09-10T12:00:02Z",
          requestId: state.operation.requestId,
          scenarioId,
          revision: projectRevision,
          solveRunId: runId,
        },
        status: "cancelled",
        solutionId: null,
      },
    });
    await vi.waitFor(() => {
      expect(api.getSolveJob).toHaveBeenCalledTimes(3);
    });
    expect(state.terminal).not.toHaveBeenCalled();
    await expect.element(screen.getByText(messages.setup.optimize.firstVerified)).toBeVisible();
    vi.mocked(api.getSolveJob).mockResolvedValue(
      response({ ...running, status: "cancelled", finishedAt: "2026-09-10T12:00:02Z" }),
    );
    await vi.waitFor(
      () => {
        expect(screen.getByText(messages.setup.optimize.retainedVerified)).toBeTruthy();
      },
      { timeout: 3_500 },
    );
    await expect.element(screen.getByText(messages.setup.optimize.retainedVerified)).toBeVisible();
    await expect.element(screen.getByText("River")).toBeVisible();
  });

  it("recovers an inspectable checkpoint from the durable run when all native events are lost", async () => {
    const state = setup();
    const start = await screen.findByRole("button", { name: messages.setup.optimize.solveStart });
    await expect.element(start).toBeEnabled();
    const checkpoint = {
      result: { solutionId: operationId, resultChecksum: "b".repeat(64) },
      verifiedAfterMilliseconds: 430,
      acceptedAt: "2026-09-10T12:00:01Z",
      checkpointChecksum: "c".repeat(64),
    };
    vi.mocked(api.listSolveRuns).mockResolvedValue(
      response({
        schemaVersion: 2,
        scenarioId,
        currentRevision: projectRevision,
        runs: [{ ...job("running"), requestId: state.operation.requestId, checkpoint }],
      }),
    );
    mockCheckpointSchedule(checkpoint.result);
    await userEvent.click(start);
    await expect
      .element(await screen.findByText(messages.setup.optimize.firstVerified))
      .toBeVisible();
    await expect.element(await screen.findByText("River")).toBeVisible();
    expect(state.terminal).not.toHaveBeenCalled();
  });

  it("recovers the durable terminal for a rejected solve start without selecting another run", async () => {
    const state = setup();
    const start = await screen.findByRole("button", { name: messages.setup.optimize.solveStart });
    await expect.element(start).toBeEnabled();
    await userEvent.click(start);
    const cancelled = { ...job("cancelled"), requestId: state.operation.requestId };
    const otherRun = { ...job("feasible"), runId: operationId, requestId: operationId };
    vi.mocked(api.listSolveRuns).mockResolvedValue(
      response({
        schemaVersion: 2,
        scenarioId,
        currentRevision: projectRevision,
        runs: [otherRun, cancelled],
      }),
    );
    vi.mocked(api.getSolveJob).mockResolvedValue(response(cancelled));
    const rejected = state.operation.result.catch(() => undefined);
    state.startResult.reject(new Error("The solve was cancelled after starting."));
    await rejected;
    await expect
      .element(await screen.findByText(messages.setup.optimize.statuses.cancelled))
      .toBeVisible();
    expect(api.getSolveJob).toHaveBeenCalledWith(scenarioId, runId);
    expect(screen.queryByText(messages.setup.optimize.acceptedNotice)).toBeNull();
  });

  it("retries the exact rejected-start request after its first durable read fails", async () => {
    const state = setup();
    await userEvent.click(
      await screen.findByRole("button", { name: messages.setup.optimize.solveStart }),
    );
    const cancelled = { ...job("cancelled"), requestId: state.operation.requestId };
    vi.mocked(api.listSolveRuns)
      .mockRejectedValueOnce(new Error("Temporary native read failure"))
      .mockResolvedValue(
        response({
          schemaVersion: 2,
          scenarioId,
          currentRevision: projectRevision,
          runs: [cancelled],
        }),
      );
    vi.mocked(api.getSolveJob).mockResolvedValue(response(cancelled));
    const rejected = state.operation.result.catch(() => undefined);
    state.startResult.reject(new Error("The solve was cancelled after starting."));
    await rejected;
    await vi.waitFor(
      () => {
        expect(state.terminal).toHaveBeenCalledWith(cancelled);
      },
      { timeout: 3_000 },
    );
    expect(screen.queryByText(messages.setup.optimize.acceptedNotice)).toBeNull();
  });

  it("keeps an independently accepted older result distinct from a run without a result", async () => {
    const older = { ...job("feasible"), stale: true };
    vi.mocked(api.getSolveJob).mockResolvedValue(response(older, projectRevision + 1));
    setup([older], { revision: projectRevision + 1 });
    await expect
      .element(await screen.findByText(messages.setup.optimize.acceptedStale))
      .toBeVisible();
    expect(screen.queryByText(messages.setup.optimize.noResult)).toBeNull();
    await expect.element(screen.getByText(messages.setup.optimize.stale)).toBeVisible();
  });

  it("reports a recovered running run as active rather than unavailable", async () => {
    vi.mocked(api.getSolveJob).mockResolvedValue(response(job("running")));
    setup([job("running")]);
    await expect
      .element(await screen.findByText(messages.setup.optimize.detachedRunning))
      .toBeVisible();
    expect(screen.queryByText(messages.setup.optimize.durableUnavailable)).toBeNull();
  });

  it("keeps backend details collapsed and reveals only run-owned durable diagnostics on request", async () => {
    vi.mocked(api.getSolveJob).mockResolvedValue(response(job("verificationAlarm")));
    vi.mocked(api.getSolveDiagnosticsSummary).mockResolvedValue(
      response({
        schemaVersion: 2,
        job: job("verificationAlarm"),
        elapsedMilliseconds: 2_500,
        firstIncumbentMilliseconds: 100,
        firstVerifiedFeasibleMilliseconds: null,
        phaseTimings: {
          compileMilliseconds: 10,
          backendMilliseconds: 2_000,
          projectionMilliseconds: null,
          structuralValidationMilliseconds: null,
          scoreRecomputationMilliseconds: null,
          requiredRuleVerificationMilliseconds: null,
          evidencePersistenceMilliseconds: 5,
          optionalExplanationMilliseconds: null,
        },
        diagnosticCode: "verify.correctness-failed",
        manifestChecksum: "a".repeat(64),
      }),
    );
    const state = setup();
    const backendDisclosure = await screen.findByText(messages.setup.optimize.backendHeading, {
      selector: "summary",
    });
    await expect.element(screen.getByText("Fixture solver")).not.toBeVisible();
    await userEvent.click(backendDisclosure);
    await expect.element(screen.getByText("Fixture solver")).toBeVisible();
    await userEvent.click(
      await screen.findByRole("button", { name: messages.setup.optimize.solveStart }),
    );
    state.startResult.resolve(
      response({
        schemaVersion: 2,
        currentRevision: projectRevision,
        reused: false,
        job: job("verificationAlarm"),
      }),
    );
    await expect
      .element(await screen.findByText(messages.setup.optimize.statuses.verificationAlarm))
      .toBeVisible();
    expect(api.getSolveDiagnosticsSummary).not.toHaveBeenCalled();
    await userEvent.click(screen.getByText(messages.setup.optimize.diagnosticsHeading));
    await expect.element(screen.getByText("verify.correctness-failed")).toBeVisible();
    await expect.element(screen.getByText("2500")).toBeVisible();
    await expect.element(screen.getByText("100")).toBeVisible();
    await expect
      .element(screen.getByText(messages.setup.optimize.diagnosticsNotRecorded))
      .toBeVisible();
    expect(api.getSolveDiagnosticsSummary).toHaveBeenCalledWith(scenarioId, runId);
  });
  it("explains an accepted optimal terminal run as recorded status plus independently verified result", async () => {
    const result = { solutionId: operationId, resultChecksum: "r".repeat(64) };
    const terminalJob = { ...job("optimal"), solutionId: result.solutionId };
    const manifestChecksum = "m".repeat(64);
    vi.mocked(api.getSolveJob).mockResolvedValue(response(terminalJob));
    vi.mocked(api.getSolveDiagnosticsSummary).mockResolvedValue(
      response({
        schemaVersion: 2,
        job: terminalJob,
        elapsedMilliseconds: 3_000,
        firstIncumbentMilliseconds: 200,
        firstVerifiedFeasibleMilliseconds: 1_000,
        phaseTimings: null,
        diagnosticCode: null,
        manifestChecksum,
      }),
    );
    const accepted = acceptedSolution(result, projectRevision, projectRevision, "optimal");
    vi.mocked(api.listSolutions).mockResolvedValue(
      response<SolutionListDtoV1>({
        schemaVersion: 1,
        scenarioId,
        currentRevision: projectRevision,
        freshlyVerified: false,
        solutions: [accepted],
      }),
    );
    vi.mocked(api.explainSolution).mockReturnValue(
      portableOperation(
        terminalExplanationResponse(
          "optimal",
          result,
          projectRevision,
          projectRevision,
          manifestChecksum,
        ),
      ),
    );
    const state = setup([terminalJob]);
    await expect
      .element(await screen.findByText(messages.setup.optimize.acceptedNotice))
      .toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: messages.setup.optimize.terminalExplanationOpen }),
    );
    await expect
      .element(await screen.findByText(messages.setup.optimize.terminalExplanationAccepted))
      .toBeVisible();
    await expect
      .element(screen.getByText(messages.setup.optimize.terminalExplanationSummary))
      .toBeVisible();
    await expect
      .element(
        screen.getByText(
          messages.setup.optimize.terminalExplanationCertainty.independentlyVerified,
        ),
      )
      .toBeVisible();
    await expect
      .element(
        screen.getByText(
          "Recorded terminal status: optimal; an accepted result is independently verified separately.",
        ),
      )
      .toBeVisible();
    expect(api.getSolveDiagnosticsSummary).toHaveBeenCalledWith(scenarioId, runId);
    expect(api.listSolutions).toHaveBeenCalledWith(scenarioId);
    expect(state.scopes).toContainEqual([scenarioId, projectRevision]);
  });

  it("explains a terminal status without an accepted result and preserves the no-result boundary", async () => {
    const terminalJob = job("infeasible");
    const manifestChecksum = "n".repeat(64);
    vi.mocked(api.getSolveJob).mockResolvedValue(response(terminalJob));
    vi.mocked(api.getSolveDiagnosticsSummary).mockResolvedValue(
      response({
        schemaVersion: 2,
        job: terminalJob,
        elapsedMilliseconds: 2_000,
        firstIncumbentMilliseconds: null,
        firstVerifiedFeasibleMilliseconds: null,
        phaseTimings: null,
        diagnosticCode: null,
        manifestChecksum,
      }),
    );
    vi.mocked(api.explainSolution).mockReturnValue(
      portableOperation(
        terminalExplanationResponse(
          "infeasible",
          null,
          projectRevision,
          projectRevision,
          manifestChecksum,
        ),
      ),
    );
    const state = setup([terminalJob]);
    await expect.element(await screen.findByText(messages.setup.optimize.noResult)).toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: messages.setup.optimize.terminalExplanationOpen }),
    );
    await expect
      .element(await screen.findByText(messages.setup.optimize.terminalExplanationNoResult))
      .toBeVisible();
    await expect
      .element(screen.getByText(messages.setup.optimize.terminalExplanationSummary))
      .toBeVisible();
    await expect
      .element(screen.getByText(messages.setup.optimize.terminalExplanationCertainty.deterministic))
      .toBeVisible();
    expect(api.listSolutions).not.toHaveBeenCalled();
    expect(state.scopes).toContainEqual([scenarioId, projectRevision]);
  });

  it("discards terminal explanation evidence when the source revision changes while diagnostics are pending", async () => {
    const terminalJob = { ...job("feasible"), solutionId: operationId };
    const pendingDiagnostics =
      Promise.withResolvers<ApiResponseDto<SolveDiagnosticsSummaryDtoV2>>();
    vi.mocked(api.getSolveJob).mockResolvedValue(response(terminalJob));
    vi.mocked(api.getSolveDiagnosticsSummary).mockReturnValue(pendingDiagnostics.promise);
    const state = setup([terminalJob]);
    await expect
      .element(await screen.findByText(messages.setup.optimize.acceptedNotice))
      .toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: messages.setup.optimize.terminalExplanationOpen }),
    );
    await expect
      .element(
        await screen.findByRole("button", {
          name: messages.setup.optimize.terminalExplanationLoading,
        }),
      )
      .toBeDisabled();
    await state.rendered.rerender({
      currentRevision: projectRevision + 1,
      summary: summary(projectRevision + 1),
    });
    pendingDiagnostics.resolve(
      response(
        {
          schemaVersion: 2,
          job: terminalJob,
          elapsedMilliseconds: 3_000,
          firstIncumbentMilliseconds: 200,
          firstVerifiedFeasibleMilliseconds: 1_000,
          phaseTimings: null,
          diagnosticCode: null,
          manifestChecksum: "m".repeat(64),
        },
        projectRevision,
      ),
    );
    await pendingDiagnostics.promise;
    await Promise.resolve();
    expect(api.listSolutions).not.toHaveBeenCalled();
    expect(api.explainSolution).not.toHaveBeenCalled();
    expect(document.querySelector("[data-optimize-terminal-explanation]")).toBeNull();
  });

  it("loads current no-result conflicts from native custody and rejects foreign context", async () => {
    const terminalJob = job("infeasible");
    const manifestChecksum = "m".repeat(64);
    const caption = "Coverage: at least one qualified person must be assigned.";
    vi.mocked(api.getSolveJob).mockResolvedValue(response(terminalJob));
    vi.mocked(api.getSolveDiagnosticsSummary).mockResolvedValue(
      response({
        schemaVersion: 2,
        job: terminalJob,
        elapsedMilliseconds: 2_000,
        firstIncumbentMilliseconds: null,
        firstVerifiedFeasibleMilliseconds: null,
        phaseTimings: null,
        diagnosticCode: null,
        manifestChecksum,
      }),
    );
    vi.mocked(api.explainSolution).mockReturnValueOnce(
      portableOperation(mappedInfeasibilityResponse(manifestChecksum, "x".repeat(64))),
    );
    const owner = diagnosticHome();
    const state = setup([terminalJob], { home: owner.home });
    await expect.element(await screen.findByText(messages.setup.optimize.noResult)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Inspect mapped conflict" }));
    await expect
      .element(await screen.findByRole("article", { name: "Conflict evidence" }))
      .toBeVisible();
    expect(screen.queryByText(caption)).toBeNull();
    expect(screen.queryByRole("button", { name: "Export deterministic diagnostic" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Review a new diagnostic copy" })).toBeNull();
    expect(state.scopes).toContainEqual([scenarioId, projectRevision]);
    expect(api.explainSolution).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({
        subject: {
          kind: "infeasibility",
          solveRunId: runId,
          runManifestChecksum: manifestChecksum,
          conflictId: null,
        },
      }),
    );

    vi.mocked(api.explainSolution).mockReturnValueOnce(
      portableOperation(mappedInfeasibilityResponse(manifestChecksum)),
    );
    await userEvent.click(screen.getByRole("button", { name: "Close conflict inspection" }));
    const trigger = screen.getByRole("button", { name: "Inspect mapped conflict" });
    expect(document.activeElement).toBe(trigger);
    await userEvent.click(trigger);
    await expect.element(await screen.findByText(caption)).toBeVisible();
    expect(screen.getAllByText(caption)).toHaveLength(1);
    await expect
      .element(await screen.findByRole("button", { name: "Export deterministic diagnostic" }))
      .toBeVisible();
    await expect
      .element(screen.getByRole("button", { name: "Review a new diagnostic copy" }))
      .toBeVisible();
    expect(screen.queryByRole("button", { name: /^Inspect source Required rule/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Edit source Required rule/ })).toBeNull();
  });

  it("publishes a reviewed mapped group copy and native metadata-only export", async () => {
    const terminalJob = job("infeasible");
    const manifestChecksum = "m".repeat(64);
    const evidenceChecksum = "e".repeat(64);
    const copiedScenarioId = "01900000-0000-7000-8000-000000000024";
    const libraryRevision = 41;
    const copiedProject: ProjectListItemV1 = {
      schemaVersion: 1,
      scenarioId: copiedScenarioId,
      title: "Clinic roster diagnostic copy",
      domainPackId: "official.test",
      revision: 1,
      updatedAt: "2026-09-10T12:00:00Z",
      archived: false,
      lastOpenedAt: null,
    };
    const copyResult: DiagnosticCopyResultV1 = {
      schemaVersion: 1,
      project: copiedProject,
      commandId: "01900000-0000-7000-8000-000000000025",
      libraryRevision: libraryRevision + 1,
    };
    const exportResult: DiagnosticExportMetadataV1 = {
      schemaVersion: 1,
      operationId: "01900000-0000-7000-8000-000000000026",
      requestId: "01900000-0000-7000-8000-000000000027",
      scenarioId,
      sourceRevision: projectRevision,
      observedCurrentRevision: projectRevision + 1,
      runId,
      runManifestChecksum: manifestChecksum,
      evidenceChecksum,
    };
    vi.mocked(api.getSolveJob).mockResolvedValue(response(terminalJob));
    vi.mocked(api.getSolveDiagnosticsSummary).mockResolvedValue(
      response({
        schemaVersion: 2,
        job: terminalJob,
        elapsedMilliseconds: 2_000,
        firstIncumbentMilliseconds: null,
        firstVerifiedFeasibleMilliseconds: null,
        phaseTimings: null,
        diagnosticCode: null,
        manifestChecksum,
      }),
    );
    vi.mocked(api.explainSolution).mockReturnValue(
      portableOperation(mappedInfeasibilityResponse(manifestChecksum)),
    );
    vi.mocked(api.getApplicationSettings).mockResolvedValue(
      response(
        {
          schemaVersion: 1,
          libraryRevision,
          settings: { appearance: null, locale: null, units: null },
        } satisfies ApplicationSettingsSnapshotV1,
        libraryRevision,
      ),
    );
    vi.mocked(api.createDiagnosticCopy).mockReturnValue(
      portableOperation(response(copyResult, null)),
    );
    vi.mocked(api.exportDiagnostic).mockReturnValue(
      portableOperation(response(exportResult, exportResult.observedCurrentRevision)),
    );
    const owner = diagnosticHome();
    const state = setup([terminalJob], { home: owner.home });
    await userEvent.click(await screen.findByRole("button", { name: "Inspect mapped conflict" }));
    await expect
      .element(await screen.findByText("Coverage: at least one qualified person must be assigned."))
      .toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Review a new diagnostic copy" }));
    const copyReview = screen.getByRole("region", { name: "Review diagnostic copy" });
    expect(copyReview).toHaveTextContent(String(libraryRevision));
    expect(copyReview).toHaveTextContent(operationId);
    expect(copyReview).toHaveTextContent("coverage.group.1");
    expect(api.getApplicationSettings).toHaveBeenCalledOnce();
    await userEvent.click(
      screen.getByRole("button", { name: "Create a new copy with this group relaxed" }),
    );
    const copyReceipt = await screen.findByRole("region", {
      name: "Created diagnostic copy receipt",
    });
    expect(copyReceipt).toHaveTextContent("Clinic roster diagnostic copy");
    expect(copyReceipt).toHaveTextContent(copiedScenarioId);
    expect(copyReceipt).toHaveTextContent(String(libraryRevision + 1));
    expect(api.createDiagnosticCopy).toHaveBeenCalledWith(
      expect.anything(),
      {
        runId,
        runManifestChecksum: manifestChecksum,
        evidenceChecksum,
        groupId: "coverage.group.1",
        expectedLibraryRevision: libraryRevision,
      },
      expect.any(Function),
    );
    expect(state.scopes).toContainEqual([scenarioId, projectRevision]);
    expect(owner.operations[0]?.refreshLibrary).toBeUndefined();
    const exportButton = screen.getByRole("button", {
      name: "Export deterministic diagnostic",
    });
    const exportDisclosure = screen.getByRole("note");
    expect(exportDisclosure).toBeVisible();
    expect(exportButton).toHaveAttribute("aria-describedby", exportDisclosure.id);
    await userEvent.click(exportButton);
    const exportReceipt = await screen.findByRole("region", {
      name: "Native diagnostic export receipt",
    });
    expect(exportReceipt).toHaveTextContent(String(exportResult.observedCurrentRevision));
    expect(exportReceipt).toHaveTextContent(exportResult.runManifestChecksum);
    expect(exportReceipt).toHaveTextContent(exportResult.evidenceChecksum);
    expect(exportReceipt.textContent).not.toMatch(/destinationPath|reportBytes/i);
    expect(api.exportDiagnostic).toHaveBeenCalledWith(
      expect.anything(),
      {
        runId,
        runManifestChecksum: manifestChecksum,
        evidenceChecksum,
      },
      expect.any(Function),
    );
    expect(owner.operations[1]?.refreshLibrary).toBe(false);
    expect(owner.state.libraryEpoch).toBe(1);
  });

  it("invalidates a diagnostic-copy review when the project library epoch changes", async () => {
    const terminalJob = job("infeasible");
    const manifestChecksum = "m".repeat(64);
    vi.mocked(api.getSolveJob).mockResolvedValue(response(terminalJob));
    vi.mocked(api.getSolveDiagnosticsSummary).mockResolvedValue(
      response({
        schemaVersion: 2,
        job: terminalJob,
        elapsedMilliseconds: 2_000,
        firstIncumbentMilliseconds: null,
        firstVerifiedFeasibleMilliseconds: null,
        phaseTimings: null,
        diagnosticCode: null,
        manifestChecksum,
      }),
    );
    vi.mocked(api.explainSolution).mockReturnValue(
      portableOperation(mappedInfeasibilityResponse(manifestChecksum)),
    );
    vi.mocked(api.getApplicationSettings).mockResolvedValue(
      response(
        {
          schemaVersion: 1,
          libraryRevision: 41,
          settings: { appearance: null, locale: null, units: null },
        } satisfies ApplicationSettingsSnapshotV1,
        41,
      ),
    );
    const owner = diagnosticHome();
    setup([terminalJob], { home: owner.home });
    await userEvent.click(await screen.findByRole("button", { name: "Inspect mapped conflict" }));
    await screen.findByText("Coverage: at least one qualified person must be assigned.");
    await userEvent.click(screen.getByRole("button", { name: "Review a new diagnostic copy" }));
    const copyReview = await screen.findByRole("region", { name: "Review diagnostic copy" });
    expect(copyReview).toHaveTextContent("41");
    owner.state.libraryEpoch += 1;
    await vi.waitFor(() => {
      expect(document.querySelector("[data-diagnostic-copy-review]")).toBeNull();
    });
    expect(screen.getByRole("button", { name: "Review a new diagnostic copy" })).toBeEnabled();
    expect(api.createDiagnosticCopy).not.toHaveBeenCalled();
  });

  it("discards a late manifest read when the current revision changes", async () => {
    const terminalJob = job("infeasible");
    const pending = Promise.withResolvers<ApiResponseDto<SolveDiagnosticsSummaryDtoV2>>();
    vi.mocked(api.getSolveJob).mockResolvedValue(response(terminalJob));
    vi.mocked(api.getSolveDiagnosticsSummary).mockReturnValue(pending.promise);
    const state = setup([terminalJob]);
    await userEvent.click(await screen.findByRole("button", { name: "Inspect mapped conflict" }));
    await state.rendered.rerender({
      currentRevision: projectRevision + 1,
      summary: summary(projectRevision + 1),
    });
    pending.resolve(
      response(
        {
          schemaVersion: 2,
          job: terminalJob,
          elapsedMilliseconds: 2_000,
          firstIncumbentMilliseconds: null,
          firstVerifiedFeasibleMilliseconds: null,
          phaseTimings: null,
          diagnosticCode: null,
          manifestChecksum: "m".repeat(64),
        },
        projectRevision,
      ),
    );
    await expect
      .element(await screen.findByRole("article", { name: "Conflict evidence" }))
      .toBeVisible();
    expect(api.explainSolution).not.toHaveBeenCalled();
  });

  it("keeps optimization unavailable for an invalid optional time but restores the mode default when cleared", async () => {
    setup();
    const start = await screen.findByRole("button", { name: messages.setup.optimize.solveStart });
    const maximum = screen.getByRole("spinbutton", {
      name: messages.setup.optimize.maximumTimeLabel,
    });
    await userEvent.fill(maximum, "31");
    await expect.element(start).toBeDisabled();
    await expect
      .element(screen.getByText(messages.setup.optimize.maximumTimeInvalid))
      .toBeVisible();
    await userEvent.fill(maximum, "30");
    await expect.element(start).toBeEnabled();
    await userEvent.clear(maximum);
    await expect.element(start).toBeEnabled();
  });

  it("does not replace an observed terminal job with a delayed running start response", async () => {
    vi.mocked(api.getSolveJob).mockResolvedValue(response(job("feasible")));
    const state = setup();
    await userEvent.click(
      await screen.findByRole("button", { name: messages.setup.optimize.solveStart }),
    );
    const completed = state.completedListener;
    if (!completed) throw new Error("Completion listener was not registered");
    completed({
      type: "solveCompleted",
      payload: {
        context: {
          eventVersion: 1,
          timestamp: "2026-09-10T12:00:03Z",
          requestId: state.operation.requestId,
          scenarioId,
          revision: projectRevision,
          solveRunId: runId,
        },
        status: "feasible",
        solutionId: operationId,
      },
    });
    await expect
      .element(await screen.findByText(messages.setup.optimize.acceptedNotice))
      .toBeVisible();
    state.startResult.resolve(
      response({
        schemaVersion: 2,
        currentRevision: projectRevision,
        reused: false,
        job: job("running"),
      }),
    );
    await state.operation.result;
    await expect
      .element(await screen.findByText(messages.setup.optimize.acceptedNotice))
      .toBeVisible();
    expect(state.terminal).toHaveBeenCalledTimes(1);
  });
  it("ignores a late completion lookup from run A after run B becomes the active job", async () => {
    const delayedA = Promise.withResolvers<ApiResponseDto<SolveJobDtoV2>>();
    vi.mocked(api.getSolveJob).mockReturnValue(delayedA.promise);
    const state = setup();
    const secondResult = Promise.withResolvers<ApiResponseDto<SolveStartDtoV2>>();
    const secondRunId = "01900000-0000-7000-8000-000000000009";
    const secondJob = { ...job("infeasible"), runId: secondRunId };
    const secondOperation: api.SolveStartOperation = {
      ...state.operation,
      requestId: "01900000-0000-7000-8000-000000000010",
      result: secondResult.promise,
    };
    vi.mocked(api.startSolve)
      .mockReturnValueOnce(state.operation)
      .mockReturnValueOnce(secondOperation);
    const start = await screen.findByRole("button", { name: messages.setup.optimize.solveStart });
    await userEvent.click(start);
    const completed = state.completedListener;
    if (!completed) throw new Error("Completion listener was not registered");
    completed({
      type: "solveCompleted",
      payload: {
        context: {
          eventVersion: 1,
          timestamp: "2026-09-10T12:00:03Z",
          requestId: state.operation.requestId,
          scenarioId,
          revision: projectRevision,
          solveRunId: runId,
        },
        status: "feasible",
        solutionId: operationId,
      },
    });
    await vi.waitFor(() => {
      expect(api.getSolveJob).toHaveBeenCalledWith(scenarioId, runId);
    });
    state.startResult.resolve(
      response({
        schemaVersion: 2,
        currentRevision: projectRevision,
        reused: false,
        job: job("feasible"),
      }),
    );
    await expect.element(start).toBeEnabled();
    await userEvent.click(start);
    secondResult.resolve(
      response({
        schemaVersion: 2,
        currentRevision: projectRevision,
        reused: false,
        job: secondJob,
      }),
    );
    await expect.element(await screen.findByText(messages.setup.optimize.noResult)).toBeVisible();
    delayedA.resolve(response(job("feasible")));
    await delayedA.promise;
    await Promise.resolve();
    await expect.element(screen.getByText(messages.setup.optimize.noResult)).toBeVisible();
    expect(state.terminal).toHaveBeenCalledTimes(2);
    expect(state.terminal).toHaveBeenLastCalledWith(secondJob);
  });

  it("releases the presentation observer on unmount without requesting cancellation", async () => {
    const state = setup();
    await userEvent.click(
      await screen.findByRole("button", { name: messages.setup.optimize.solveStart }),
    );
    state.rendered.unmount();
    expect(state.dispose).toHaveBeenCalled();
    expect(api.cancelSolve).not.toHaveBeenCalled();
    expect(state.release).not.toHaveBeenCalled();
  });
  it("continues durable polling for a live solve after its scenario revision changes", async () => {
    const state = setup();
    await userEvent.click(
      await screen.findByRole("button", { name: messages.setup.optimize.solveStart }),
    );
    const progress = state.progressListener;
    if (!progress) throw new Error("Progress listener was not registered");
    vi.mocked(api.getSolveJob).mockResolvedValue(response(job("running")));
    progress({
      type: "solveProgress",
      payload: {
        context: {
          eventVersion: 1,
          timestamp: "2026-09-10T12:00:01Z",
          requestId: state.operation.requestId,
          scenarioId,
          revision: projectRevision,
          solveRunId: runId,
        },
        phase: "solving",
        percent: null,
      },
    });
    await expect.element(await screen.findByText(messages.setup.optimize.running)).toBeVisible();
    vi.mocked(api.listSolveRuns).mockResolvedValue(
      response({
        schemaVersion: 2,
        scenarioId,
        currentRevision: projectRevision + 1,
        runs: [{ ...job("running"), stale: true }],
      }),
    );
    await state.rendered.rerender({
      currentRevision: projectRevision + 1,
      summary: summary(projectRevision + 1),
    });
    await vi.waitFor(() => {
      expect(api.listSolveRuns).toHaveBeenCalledTimes(2);
    });
    const terminal = { ...job("cancelled"), stale: true };
    vi.mocked(api.getSolveJob).mockResolvedValue(response(terminal, projectRevision + 1));
    await vi.waitFor(
      () => {
        expect(state.terminal).toHaveBeenCalledWith(terminal);
      },
      { timeout: 3_000 },
    );
  });

  it("reattaches and cancels an older-revision run after validation becomes stale", async () => {
    const retained = { ...job("running"), cancelOperationId: operationId };
    vi.mocked(api.getSolveJob).mockResolvedValue(response(retained));
    const state = setup([retained]);
    await expect
      .element(await screen.findByRole("button", { name: messages.setup.optimize.solveCancel }))
      .toBeEnabled();
    const stale = { ...retained, stale: true };
    vi.mocked(api.listSolveRuns).mockResolvedValue(
      response(
        { schemaVersion: 2, scenarioId, currentRevision: projectRevision + 1, runs: [stale] },
        projectRevision + 1,
      ),
    );
    vi.mocked(api.getSolveJob).mockResolvedValue(response(stale, projectRevision + 1));
    await state.rendered.rerender({
      currentRevision: projectRevision + 1,
      summary: {
        ...summary(projectRevision + 1),
        full: {
          state: "completed",
          operationId,
          inputRevision: projectRevision,
          stale: true,
          counts: { errors: 0, warnings: 0, information: 0 },
        },
      },
    });
    await expect
      .element(
        screen.getByText(messages.setup.optimize.validationRequired, {
          selector: '[data-optimize-state="validation-not-ready"]',
        }),
      )
      .toBeVisible();
    await expect
      .element(screen.getByRole("button", { name: messages.setup.optimize.solveCancel }))
      .toBeEnabled();
    expect(api.startSolve).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getByRole("button", { name: messages.setup.optimize.solveCancel }),
    );
    expect(api.cancelSolve).toHaveBeenCalledWith({
      operationId,
      scenarioId,
      expectedRevision: projectRevision,
    });
    await expect
      .element(screen.getByRole("button", { name: messages.setup.optimize.solveCancelling }))
      .toBeDisabled();
    const terminal = { ...job("cancelled"), stale: true };
    vi.mocked(api.getSolveJob).mockResolvedValue(response(terminal, projectRevision + 1));
    await vi.waitFor(
      () => {
        expect(state.terminal).toHaveBeenCalledWith(terminal);
      },
      { timeout: 3_000 },
    );
  });
  it("allows retrying the original solve cancellation when its revision changes during a failed request", async () => {
    const retained = { ...job("running"), cancelOperationId: operationId };
    vi.mocked(api.getSolveJob).mockResolvedValue(response(retained));
    const state = setup([retained]);
    const cancelled = Promise.withResolvers<Awaited<ReturnType<typeof api.cancelSolve>>>();
    vi.mocked(api.cancelSolve).mockImplementationOnce(() => cancelled.promise);
    await userEvent.click(
      await screen.findByRole("button", { name: messages.setup.optimize.solveCancel }),
    );
    const stale = { ...retained, stale: true };
    vi.mocked(api.listSolveRuns).mockResolvedValue(
      response({
        schemaVersion: 2,
        scenarioId,
        currentRevision: projectRevision + 1,
        runs: [stale],
      }),
    );
    vi.mocked(api.getSolveJob).mockResolvedValue(response(stale, projectRevision + 1));
    await state.rendered.rerender({
      currentRevision: projectRevision + 1,
      summary: summary(projectRevision + 1),
    });
    cancelled.reject(new Error("Temporary cancellation transport failure"));
    await expect
      .element(await screen.findByRole("button", { name: messages.setup.optimize.solveCancel }))
      .toBeEnabled();
    await userEvent.click(
      screen.getByRole("button", { name: messages.setup.optimize.solveCancel }),
    );
    expect(api.cancelSolve).toHaveBeenCalledTimes(2);
    expect(api.cancelSolve).toHaveBeenLastCalledWith({
      operationId,
      scenarioId,
      expectedRevision: projectRevision,
    });
  });

  it("does not expose cancellation for a retained run without live window ownership", async () => {
    const retained = job("running");
    vi.mocked(api.getSolveJob).mockResolvedValue(response(retained));
    setup([retained]);
    await expect.element(await screen.findByText(messages.setup.optimize.running)).toBeVisible();
    expect(screen.queryByRole("button", { name: messages.setup.optimize.solveCancel })).toBeNull();
  });
  it("recovers a retained running job after a missed terminal event", async () => {
    vi.mocked(api.getSolveJob)
      .mockResolvedValueOnce(response(job("running")))
      .mockResolvedValue(response(job("feasible")));
    const state = setup([job("running")]);
    await expect.element(await screen.findByText(messages.setup.optimize.running)).toBeVisible();
    await vi.waitFor(
      () => {
        expect(state.terminal).toHaveBeenCalledWith(job("feasible"));
      },
      { timeout: 3_000 },
    );
    await expect.element(screen.getByText(messages.setup.optimize.acceptedNotice)).toBeVisible();
  });
});

describe("diagnostic receipts during home refresh", () => {
  it("keeps copy and export outcomes owned while home refreshes reload the overview", async () => {
    const workforceProject: ProjectSummary = {
      ...projectFixture,
      domainPackId: "official.workforce",
    };
    const projects = [workforceProject];
    const homeApi = fakeApi(projects);
    const appNotification: { listener: (() => void) | null } = { listener: null };
    homeApi.onAppNotification.mockImplementation((listener) => {
      appNotification.listener = listener;
      return Promise.resolve(vi.fn());
    });
    const owner: { home: ProjectHomeController | null } = { home: null };
    const router = createAppRouter();
    onTestFinished(async () => {
      await owner.home?.dispose();
      router.options.history.destroy();
    });

    const currentSummary = (): ScenarioSummaryV2 => ({
      schemaVersion: 2,
      scenarioId,
      revision: projectRevision,
      title: workforceProject.title,
      structure: { entities: 0, rules: 1, preferences: 0, lockedAssignments: 1 },
      fast: { counts: { errors: 0, warnings: 0, information: 0 }, issues: [], omitted: 0 },
      full: { state: "notRun" },
    });
    const overviewFacts: WorkforceSetupFacts = {
      activePreferences: 0,
      activeRequiredRules: 1,
      configuredTypeMemberships: 0,
      entities: [],
      initialWorkWindow: { startDate: "2030-01-01", endDateExclusive: "2030-01-08" },
      lockedAssignments: 1,
      planningDates: { startDate: "2030-01-01", endDateExclusive: "2030-02-01" },
      preferences: 0,
      requiredRules: 1,
      settings: {
        timeZone: "UTC",
        locale: "en-US",
        units: "metric",
        horizon: { start: "2030-01-01T00:00:00Z", end: "2030-02-01T00:00:00Z" },
        gapPolicy: "reject",
        overlapPolicy: "earlier",
      },
    };
    const overviewResponse = response({
      schemaVersion: 2,
      scenarioId,
      revision: projectRevision,
      view: {
        viewId: "official.workforce.setup.overview",
        data: {
          schemaVersion: 1,
          result: { kind: "overview", data: overviewFacts },
        },
      },
    } as const);
    vi.mocked(api.manualEdit).mockImplementation(() =>
      portableOperation(
        Promise.reject(
          Object.assign(new Error("Adopt an original accepted result before manual editing."), {
            category: "validation",
            code: "manual.base_missing",
          }),
        ),
      ),
    );
    const terminalJob = job("infeasible");
    const manifestChecksum = "m".repeat(64);
    const libraryRevision = 41;
    const copiedProject: ProjectSummary = {
      schemaVersion: 1,
      scenarioId: "01900000-0000-7000-8000-000000000024",
      title: "Clinic roster diagnostic copy",
      domainPackId: "official.workforce",
      revision: 1,
      updatedAt: "2026-09-10T12:00:00Z",
      archived: false,
      lastOpenedAt: null,
    };
    const copyResult: DiagnosticCopyResultV1 = {
      schemaVersion: 1,
      project: copiedProject,
      commandId: "01900000-0000-7000-8000-000000000025",
      libraryRevision: libraryRevision + 1,
    };
    const exportMetadata: DiagnosticExportMetadataV1 = {
      schemaVersion: 1,
      operationId: "01900000-0000-7000-8000-000000000026",
      requestId: "01900000-0000-7000-8000-000000000027",
      scenarioId,
      sourceRevision: projectRevision,
      observedCurrentRevision: projectRevision,
      runId,
      runManifestChecksum: manifestChecksum,
      evidenceChecksum: "e".repeat(64),
    };
    const exportOperation = (
      result: Promise<ApiResponseDto<DiagnosticExportMetadataV1>>,
    ): SetupOperation<DiagnosticExportMetadataV1> => ({
      requestId: exportMetadata.requestId,
      operationId: Promise.resolve(exportMetadata.operationId),
      result,
      cancel: () =>
        Promise.resolve(
          response({
            schemaVersion: 1,
            acknowledgement: "cancellationRequested",
          }),
        ),
      release: () => Promise.resolve(response({ schemaVersion: 1, acknowledgement: "released" })),
      isCurrent: () => true,
    });

    vi.mocked(api.SetupOperationScope).mockImplementation(
      class {
        dispose = vi.fn();
      } as unknown as typeof api.SetupOperationScope,
    );
    vi.mocked(api.getScenarioSummary).mockImplementation(() =>
      portableOperation(response(currentSummary())),
    );
    vi.mocked(api.getScenarioView).mockReturnValue(portableOperation(overviewResponse));
    vi.mocked(api.listSolutions).mockResolvedValue(
      response({
        schemaVersion: 1,
        scenarioId,
        currentRevision: projectRevision,
        freshlyVerified: false,
        solutions: [],
      }),
    );
    vi.mocked(api.getAppCapabilities).mockResolvedValue(response(capabilities));
    vi.mocked(api.getSolveBackendOptions).mockResolvedValue(
      response({
        schemaVersion: 2,
        scenarioId,
        revision: projectRevision,
        options: [
          {
            backendId: "solver.fixture",
            backendVersion: "1.0.0",
            adapterVersion: "1.0.0",
            displayName: "Fixture solver",
          },
        ],
      }),
    );
    vi.mocked(api.listSolveRuns).mockResolvedValue(
      response({
        schemaVersion: 2,
        scenarioId,
        currentRevision: projectRevision,
        runs: [terminalJob],
      }),
    );
    vi.mocked(api.getSolveJob).mockResolvedValue(response(terminalJob));
    vi.mocked(api.getSolveDiagnosticsSummary).mockResolvedValue(
      response({
        schemaVersion: 2,
        job: terminalJob,
        elapsedMilliseconds: 2_000,
        firstIncumbentMilliseconds: null,
        firstVerifiedFeasibleMilliseconds: null,
        phaseTimings: null,
        diagnosticCode: null,
        manifestChecksum,
      }),
    );
    vi.mocked(api.onSolveProgress).mockResolvedValue(vi.fn());
    vi.mocked(api.onSolveVerified).mockResolvedValue(vi.fn());
    vi.mocked(api.onSolveCompleted).mockResolvedValue(vi.fn());
    vi.mocked(api.explainSolution).mockReturnValue(
      portableOperation(mappedInfeasibilityResponse(manifestChecksum)),
    );
    vi.mocked(api.getApplicationSettings).mockResolvedValue(
      response(
        {
          schemaVersion: 1,
          libraryRevision,
          settings: { appearance: null, locale: null, units: null },
        },
        libraryRevision,
      ),
    );
    vi.mocked(api.createDiagnosticCopy).mockImplementation(() => {
      projects.push(copiedProject);
      return portableOperation(response(copyResult, null));
    });

    const Harness = defineComponent({
      setup() {
        const home = createProjectHomeController(homeApi);
        owner.home = home;
        onScopeDispose(() => void home.dispose());
        void home.load();
        void home.startEventListeners();
        return () => h(WorkforceSetupOverview, { home, project: workforceProject });
      },
    });
    render(Harness, { global: { plugins: [createPinia(), PiniaColada, router] } });

    await userEvent.click(await screen.findByRole("button", { name: "Inspect mapped conflict" }));
    await screen.findByText("Coverage: at least one qualified person must be assigned.");
    await userEvent.click(screen.getByRole("button", { name: "Review a new diagnostic copy" }));
    await screen.findByRole("region", { name: "Review diagnostic copy" });

    const home = owner.home;
    if (home === null) throw new Error("The real project-home controller was not created");
    const refreshFromNotification = async () => {
      const epochBefore = home.state.libraryEpoch;
      const callsBefore = vi.mocked(api.getScenarioSummary).mock.calls.length;
      const summary = Promise.withResolvers<ApiResponseDto<ScenarioSummaryV2>>();
      vi.mocked(api.getScenarioSummary).mockReturnValueOnce(portableOperation(summary.promise));
      const notify = appNotification.listener;
      if (notify === null) throw new Error("The controller did not register app notifications");
      notify();
      await vi.waitFor(() => {
        expect(home.state.libraryEpoch).toBe(epochBefore + 1);
        expect(api.getScenarioSummary).toHaveBeenCalledTimes(callsBefore + 1);
      });
      expect(
        screen.getByRole("region", { name: "Recorded infeasibility inspection" }),
      ).toBeVisible();
      return summary;
    };
    const epoch = home.state.libraryEpoch;
    const summaryDuringRefresh = Promise.withResolvers<ApiResponseDto<ScenarioSummaryV2>>();
    const summaryCalls = vi.mocked(api.getScenarioSummary).mock.calls.length;
    vi.mocked(api.getScenarioSummary).mockReturnValueOnce(
      portableOperation(summaryDuringRefresh.promise),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Create a new copy with this group relaxed" }),
    );

    await vi.waitFor(() => {
      expect(home.state.libraryEpoch).toBe(epoch + 1);
      expect(api.getScenarioSummary).toHaveBeenCalledTimes(summaryCalls + 1);
    });
    expect(homeApi.listProjects).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("region", { name: "Recorded infeasibility inspection" })).toBeVisible();

    const receipt = await screen.findByRole("region", { name: "Created diagnostic copy receipt" });
    expect(receipt).toHaveTextContent(copiedProject.title);
    expect(receipt).toHaveTextContent(copiedProject.scenarioId);
    expect(receipt).toHaveTextContent(String(libraryRevision + 1));
    expect(document.activeElement).toBe(receipt.querySelector("[role='heading']"));
    expect(screen.getByText(messages.setup.loading)).toBeVisible();

    summaryDuringRefresh.resolve(response(currentSummary()));
    await vi.waitFor(() => {
      expect(screen.queryByText(messages.setup.loading)).toBeNull();
    });
    expect(
      screen.getByRole("region", { name: "Created diagnostic copy receipt" }),
    ).toHaveTextContent(copiedProject.scenarioId);
    expect(home.state.projects).toContainEqual(copiedProject);

    const cancelledExport = Promise.withResolvers<ApiResponseDto<DiagnosticExportMetadataV1>>();
    vi.mocked(api.exportDiagnostic).mockReturnValueOnce(exportOperation(cancelledExport.promise));
    await userEvent.click(screen.getByRole("button", { name: "Export deterministic diagnostic" }));
    await screen.findByText(explanationMessage("conflict.exporting"));
    const cancellationRefresh = await refreshFromNotification();
    cancelledExport.reject(
      Object.assign(new Error("Native picker closed"), {
        code: "operation.cancelled",
      }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      explanationMessage("conflict.exportCancelled"),
    );
    expect(document.querySelector("[data-diagnostic-export-receipt]")).toBeNull();
    cancellationRefresh.resolve(response(currentSummary()));
    await vi.waitFor(() => {
      expect(screen.queryByText(messages.setup.loading)).toBeNull();
    });
    expect(screen.getByRole("alert")).toHaveTextContent(
      explanationMessage("conflict.exportCancelled"),
    );

    const successfulExport = Promise.withResolvers<ApiResponseDto<DiagnosticExportMetadataV1>>();
    vi.mocked(api.exportDiagnostic).mockReturnValueOnce(exportOperation(successfulExport.promise));
    await userEvent.click(screen.getByRole("button", { name: "Export deterministic diagnostic" }));
    await screen.findByText(explanationMessage("conflict.exporting"));
    const exportRefresh = await refreshFromNotification();
    successfulExport.resolve(response(exportMetadata, exportMetadata.observedCurrentRevision));
    const exportReceipt = await screen.findByRole("region", {
      name: "Native diagnostic export receipt",
    });
    expect(exportReceipt).toHaveTextContent(scenarioId);
    expect(exportReceipt).toHaveTextContent(runId);
    expect(exportReceipt).toHaveTextContent(manifestChecksum);
    expect(exportReceipt).toHaveTextContent(exportMetadata.evidenceChecksum);
    expect(document.activeElement).toBe(exportReceipt.querySelector("[role='heading']"));
    exportRefresh.resolve(response(currentSummary()));
    await vi.waitFor(() => {
      expect(screen.queryByText(messages.setup.loading)).toBeNull();
    });
    expect(
      screen.getByRole("region", { name: "Native diagnostic export receipt" }),
    ).toHaveTextContent(exportMetadata.evidenceChecksum);

    const repeatedExport = Promise.withResolvers<ApiResponseDto<DiagnosticExportMetadataV1>>();
    vi.mocked(api.exportDiagnostic).mockReturnValueOnce(exportOperation(repeatedExport.promise));
    await userEvent.click(screen.getByRole("button", { name: "Export deterministic diagnostic" }));
    expect(document.querySelector("[data-diagnostic-export-receipt]")).toBeNull();
    const repeatedRefresh = await refreshFromNotification();
    repeatedExport.reject(
      Object.assign(new Error("Native picker closed"), { code: "operation.cancelled" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      explanationMessage("conflict.exportCancelled"),
    );
    repeatedRefresh.resolve(response(currentSummary()));
    await vi.waitFor(() => {
      expect(screen.queryByText(messages.setup.loading)).toBeNull();
    });
    expect(document.querySelector("[data-diagnostic-export-receipt]")).toBeNull();
  });
});
