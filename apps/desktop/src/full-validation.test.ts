import { effectScope, shallowReactive } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  getScenarioSetupStatus,
  validateScenario,
  type ApiResponseDto,
  type FullValidationResultV2,
} from "./api/generated";
import { useFullValidation } from "./full-validation";
import type { ProjectHomeController, ProjectSummary } from "./project-home";
import { fakeApi, portableOperation, project, response } from "./testing/project-home";

vi.mock("./api/generated", { spy: true });
afterEach(() => {
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

const workforceProject: ProjectSummary = { ...project, domainPackId: "official.workforce" };
const otherId = "01900000-0000-7000-8000-000000000002";

async function startValidation() {
  fakeApi([workforceProject]);
  const state = shallowReactive({
    libraryEpoch: 1,
    changeSignal: null as { scenarioId: string | null } | null,
    busyAction: null,
  });
  const home = {
    state,
    runOperation: ({ execute }: { execute: (report: () => void) => Promise<unknown> }) =>
      execute(() => {}),
  } as unknown as ProjectHomeController;
  const status = {
    schemaVersion: 2 as const,
    scenarioId: workforceProject.scenarioId,
    revision: workforceProject.revision,
    structure: { entities: 0, rules: 0, preferences: 0, lockedAssignments: 0 },
    fast: { counts: { errors: 0, warnings: 0, information: 0 }, issues: [], omitted: 0 },
    full: { state: "notRun" as const },
  };
  vi.mocked(getScenarioSetupStatus).mockImplementation(() => portableOperation(response(status)));
  const terminal = deferred<ApiResponseDto<FullValidationResultV2>>();
  vi.mocked(validateScenario).mockImplementation(() => portableOperation(terminal.promise));
  const scope = effectScope();
  const validation = scope.run(() => useFullValidation(home, () => workforceProject));
  if (!validation) throw new Error("Validation scope did not start");
  const running = validation.run();
  await vi.waitFor(() => {
    expect(validateScenario).toHaveBeenCalledOnce();
  });
  const operationScope = vi.mocked(validateScenario).mock.calls[0]?.[0];
  if (!operationScope) throw new Error("Native validation scope was not created");
  const disposal = vi.spyOn(operationScope, "dispose");
  disposal.mockClear();
  return { state, validation, running, terminal, disposal, scope };
}

describe("full validation context", () => {
  it("does not cancel validation of A when unrelated project B refreshes the library", async () => {
    const { state, validation, running, terminal, disposal, scope } = await startValidation();
    expect(disposal).not.toHaveBeenCalled();
    try {
      state.changeSignal = { scenarioId: otherId };
      state.libraryEpoch += 1;
      expect(disposal).not.toHaveBeenCalled();
      expect(validation.state.run).toBe("running");
      terminal.resolve(
        response({
          schemaVersion: 2,
          scenarioId: workforceProject.scenarioId,
          revision: workforceProject.revision,
          report: { issues: [] },
        }),
      );
      await running;
      expect(validation.state.run).toBe("completed");
    } finally {
      scope.stop();
    }
  });

  it.each([
    { reason: "its own project changes", changedId: workforceProject.scenarioId },
    { reason: "native event loss makes the library identity uncertain", changedId: null },
  ])("cancels an active validation when $reason", async ({ changedId }) => {
    const { state, validation, running, terminal, disposal, scope } = await startValidation();
    expect(disposal).not.toHaveBeenCalled();
    try {
      state.changeSignal = { scenarioId: changedId };
      expect(disposal).toHaveBeenCalledOnce();
      expect(validation.state.run).toBe("idle");
      terminal.resolve(
        response({
          schemaVersion: 2,
          scenarioId: workforceProject.scenarioId,
          revision: workforceProject.revision,
          report: { issues: [] },
        }),
      );
      await running;
      expect(validation.state.report).toBeNull();
    } finally {
      scope.stop();
    }
  });
});
