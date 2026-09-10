import { webcrypto } from "node:crypto";
import type { Channel } from "@tauri-apps/api/core";
import type * as TauriCore from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
const tauriEvents = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof TauriCore>()),
  invoke: tauri.invoke,
}));
vi.mock("@tauri-apps/api/event", () => tauriEvents);
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));

import * as generated from "./generated";
import { isWorkforceSetupQueryResult } from "./generated-domain-pack-contracts";
import acceptedDetail from "./fixtures/accepted-detail-native.json";
import optimalityStatus from "./fixtures/optimality-status-native.json";

const scenarioId = "01900000-0000-7000-8000-000000000001";
const operationId = "01900000-0000-7000-8000-000000000002";
interface Invocation {
  readonly request: { readonly requestId: string; readonly [key: string]: unknown };
  readonly onProgress?: Channel;
}
function response(input: Invocation, result: unknown, currentRevision: number | null = null) {
  return {
    schemaVersion: generated.API_SCHEMA_VERSION,
    requestId: input.request.requestId,
    currentRevision,
    warnings: [],
    result,
  };
}
function summary(): generated.ScenarioSummaryV2 {
  return {
    schemaVersion: 2,
    scenarioId,
    revision: 3,
    title: "Clinic",
    structure: { entities: 1, rules: 0, preferences: 0, lockedAssignments: 0 },
    fast: { counts: { errors: 0, warnings: 0, information: 0 }, issues: [], omitted: 0 },
    full: { state: "notRun" },
  };
}
const nativeError: generated.ApiErrorDto = {
  code: "operation.cancelled",
  category: "protocol",
  message: "Operation cancelled.",
  retryable: false,
  fieldErrors: [],
  details: null,
  diagnosticId: null,
};
const invalidResponse = { code: "protocol.invalid_response", retryable: false, details: null };
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function collectKeys(value: unknown): readonly string[] {
  if (Array.isArray(value)) return value.flatMap(collectKeys);
  if (typeof value !== "object" || value === null) return [];
  return Object.entries(value).flatMap(([key, child]) => [key, ...collectKeys(child)]);
}

let callbacks: Map<number, (message: unknown) => void>;
beforeEach(() => {
  tauri.invoke.mockReset();
  tauriEvents.listen.mockReset();
  callbacks = new Map();
  let nextCallback = 0;
  vi.stubGlobal("window", {
    crypto: webcrypto,
    __TAURI_INTERNALS__: {
      transformCallback(callback: (message: unknown) => void) {
        const id = ++nextCallback;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback(id: number) {
        callbacks.delete(id);
      },
    },
  });
});
afterEach(() => {
  vi.unstubAllGlobals();
});

/** Only native transport is substituted; Channel ordering and client validation are real. */
function nativeOperations() {
  const prepare = deferred<unknown>();
  const terminal = deferred<unknown>();
  const claimed = deferred<Invocation>();
  let preparation: Invocation | undefined;
  const controls: string[] = [];
  tauri.invoke.mockImplementation((command: string, input: Invocation) => {
    if (command === "operation_prepare") {
      preparation = input;
      return prepare.promise;
    }
    if (command === "operation_cancel" || command === "operation_release") {
      controls.push(command);
      return Promise.resolve(
        response(input, {
          schemaVersion: 1,
          acknowledgement: command === "operation_cancel" ? "cancellationRequested" : "released",
        }),
      );
    }
    claimed.resolve(input);
    return terminal.promise;
  });
  return {
    terminal,
    claimed,
    controls,
    prepared() {
      if (!preparation) throw new Error("No preparation request");
      prepare.resolve(response(preparation, { schemaVersion: 1, operationId }));
    },
  };
}

describe("strict desktop response boundary", () => {
  it("preserves safe millisecond durations beyond u32 and rejects unrepresentable worker counts", async () => {
    const result = structuredClone(optimalityStatus);
    const status = result.explanation.evidence.evidence.status;
    status.runInput.solveOptions.timeLimitMilliseconds = Number.MAX_SAFE_INTEGER;
    status.runManifest.elapsedMilliseconds = Number.MAX_SAFE_INTEGER;
    Reflect.set(status.runManifest.phaseTimings, "backendMilliseconds", Number.MAX_SAFE_INTEGER);
    status.runInput.solveOptions.workerThreads.count = 65_535;
    const request: generated.ExplanationRequestV1 = {
      schemaVersion: 1,
      subject: {
        kind: "optimalityStatus",
        solveRunId: status.runInput.runId,
        runManifestChecksum: status.runManifest.checksum,
        result: status.result,
      },
    };
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, result, result.currentRevision)),
    );
    const accepted = await generated.explainSolution({ scenarioId: result.scenarioId, request });
    expect(accepted.result.explanation.evidence.evidence).toMatchObject({
      status: { runManifest: { elapsedMilliseconds: Number.MAX_SAFE_INTEGER } },
    });
    status.runInput.solveOptions.workerThreads.count = 65_536;
    await expect(
      generated.explainSolution({ scenarioId: result.scenarioId, request }),
    ).rejects.toMatchObject(invalidResponse);
    status.runInput.solveOptions.workerThreads.count = 1;
    status.runManifest.elapsedMilliseconds = Number.MAX_SAFE_INTEGER + 1;
    await expect(
      generated.explainSolution({ scenarioId: result.scenarioId, request }),
    ).rejects.toMatchObject(invalidResponse);
  });

  it("rejects conflicting envelope revisions and scenario echoes without retrying the read", async () => {
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, acceptedDetail, acceptedDetail.currentRevision + 1)),
    );
    await expect(
      generated.getSolutionSummary(
        acceptedDetail.scenarioId,
        acceptedDetail.result.solution.solutionId,
      ),
    ).rejects.toMatchObject(invalidResponse);
    expect(tauri.invoke).toHaveBeenCalledTimes(1);
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, acceptedDetail, acceptedDetail.currentRevision)),
    );
    await expect(
      generated.getSolutionSummary(scenarioId, acceptedDetail.result.solution.solutionId),
    ).rejects.toMatchObject(invalidResponse);
    expect(tauri.invoke).toHaveBeenCalledTimes(2);
  });

  it("parses committed domain-command inverses instead of treating command IDs as pack IDs", async () => {
    const receipt = {
      newRevision: 1,
      changeSet: { changes: [] },
      validationDelta: { added: [], resolved: [] },
      inverse: {
        type: "applyDomainCommand",
        payload: {
          commandType: "official.workforce.remove_entity",
          payload: { entityId: operationId },
        },
      },
    };
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, receipt, 1)),
    );
    const result = await generated.applyScenarioCommand({
      commandId: operationId,
      scenarioId,
      expectedRevision: 0,
      actor: { actorId: null, displayName: "Native fixture" },
      truncateRedo: false,
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: "official.workforce.add_entity",
          payload: {
            entity: { id: operationId, kind: "qualification", name: "CPR", description: "" },
          },
        },
      },
    });
    expect(result.result.inverse).toEqual(receipt.inverse);
    expect(result.result.newRevision).toBe(1);
    expect(tauri.invoke).toHaveBeenCalledTimes(1);
  });

  it("bounds verifier evidence and rejects zero metric denominators", async () => {
    let detail = structuredClone(acceptedDetail);
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, detail, detail.currentRevision)),
    );
    const level = detail.result.verification.score.levels[0];
    if (!level) throw new Error("Captured score level missing");
    detail.result.verification.score.levels = Array.from({ length: 17 }, () => level);
    await expect(
      generated.getSolutionSummary(detail.scenarioId, detail.result.solution.solutionId),
    ).rejects.toMatchObject(invalidResponse);
    detail = structuredClone(acceptedDetail);
    Reflect.set(detail.result.verification, "metrics", {
      ratio: { type: "ratio", value: { numerator: "1", denominator: 0 } },
    });
    await expect(
      generated.getSolutionSummary(detail.scenarioId, detail.result.solution.solutionId),
    ).rejects.toMatchObject(invalidResponse);
  });

  it("keeps the V1 solution API distinct from nested accepted and verification V2 formats", async () => {
    // Captured through real WebKit/native IPC from the core's independently verified fixture.
    let detail = structuredClone(acceptedDetail);
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, detail, detail.currentRevision)),
    );
    const result = await generated.getSolutionSummary(
      detail.scenarioId,
      detail.result.solution.solutionId,
    );
    expect(result.result.result.schemaVersion).toBe(2);
    expect(result.result.result.verification.schemaVersion).toBe(2);
    expect(result.result.result.verification.score.feasibility).toBe("0");
    detail.result.schemaVersion = 3;
    await expect(
      generated.getSolutionSummary(detail.scenarioId, detail.result.solution.solutionId),
    ).rejects.toMatchObject(invalidResponse);
    detail = structuredClone(acceptedDetail);
    detail.result.verification.schemaVersion = 3;
    await expect(
      generated.getSolutionSummary(detail.scenarioId, detail.result.solution.solutionId),
    ).rejects.toMatchObject(invalidResponse);
  });

  it("accepts Rust portable version descriptors and semantic capability objects", async () => {
    // Captured from real native pack_list, not constructed from the TypeScript DTO.
    const descriptor = {
      id: "official.workforce",
      packVersion: "0.1.0",
      scenarioVersions: { latest: 1, migratableFrom: [] },
      portableVersions: { latest: 1, migratableFrom: [] },
      portableCapabilities: [{ id: "official.workforce.portable", version: 1 }],
      shareResultSchemaVersion: 1,
      syntheticTestOnly: false,
      displayName: { defaultText: "Workforce", key: "official.workforce.name" },
      description: {
        defaultText: "Workforce assignment planning and independent verification.",
        key: "official.workforce.description",
      },
      capabilities: [
        "commands",
        "compilation",
        "projection",
        "verification",
        "scoring",
        "portableData",
        "shareResult",
        "aiTools",
      ],
      explanationCapabilities: ["validation", "assignment"],
      documentationUrl: null,
      iconId: "official.workforce.icon",
      license: { spdxExpression: "Apache-2.0", attribution: "Eutheto contributors" },
    };
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, [descriptor])),
    );
    const packs = (await generated.listDomainPacks()).result;
    expect(packs[0]?.portableVersions).toEqual({ latest: 1, migratableFrom: [] });
    expect(packs[0]?.portableCapabilities).toEqual([
      { id: "official.workforce.portable", version: 1 },
    ]);
  });

  it("accepts namespaced pack identities and rejects malformed envelope or project data without replay", async () => {
    const project = {
      scenarioId,
      title: "Clinic",
      domainPackId: "official.workforce",
      revision: 3,
      updatedAt: "2026-09-10T12:00:00.123456789Z",
      archived: false,
    };
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, [project])),
    );
    expect((await generated.listProjects()).result[0]?.domainPackId).toBe("official.workforce");
    const corruptions = [
      (input: Invocation) => ({ ...response(input, [project]), requestId: operationId }),
      (input: Invocation) => ({ ...response(input, [project]), schemaVersion: 99 }),
      (input: Invocation) => ({
        ...response(input, [project]),
        currentRevision: Number.MAX_SAFE_INTEGER + 1,
      }),
      (input: Invocation) => response(input, [{ ...project, scenarioId: "not-an-id" }]),
      (input: Invocation) => response(input, [{ ...project, revision: 1.5 }]),
      (input: Invocation) => response(input, [{ ...project, updatedAt: "2026-02-30T12:00:00Z" }]),
      (input: Invocation) => response(input, [{ ...project, unexpected: true }]),
      (input: Invocation) => response(input, [{ ...project, title: "\ud800" }]),
    ];
    for (const corrupt of corruptions) {
      tauri.invoke
        .mockReset()
        .mockImplementation((_command: string, input: Invocation) =>
          Promise.resolve(corrupt(input)),
        );
      await expect(generated.listProjects()).rejects.toMatchObject(invalidResponse);
      expect(tauri.invoke).toHaveBeenCalledTimes(1);
    }
  });

  it("preserves structured safe errors but never exposes unstructured native failure text", async () => {
    tauri.invoke.mockRejectedValue(nativeError);
    await expect(generated.listProjects()).rejects.toEqual(nativeError);
    tauri.invoke.mockRejectedValue("/private/scenario.sqlite: secret captured data");
    await expect(generated.listProjects()).rejects.toMatchObject(invalidResponse);
    try {
      await generated.listProjects();
    } catch (error: unknown) {
      expect(JSON.stringify(error)).not.toMatch(/private|secret|captured/);
    }
  });

  it.each([Number.MAX_SAFE_INTEGER + 1, 1.5, -1])(
    "rejects unsafe revision %s before native entry",
    async (revision) => {
      await expect(generated.redoScenario(scenarioId, revision)).rejects.toBeInstanceOf(RangeError);
      expect(tauri.invoke).not.toHaveBeenCalled();
    },
  );

  it("preserves the safe revision ceiling and wide signed values without numeric coercion", async () => {
    tauri.invoke.mockRejectedValue(nativeError);
    await expect(generated.undoScenario(scenarioId, Number.MAX_SAFE_INTEGER)).rejects.toEqual(
      nativeError,
    );
    await expect(
      generated.startCounterfactual({
        scenarioId,
        expectedRevision: 0,
        baseSolutionId: operationId,
        condition: {
          type: "forceAssignmentValue",
          assignmentId: "shift.primary",
          value: { type: "integer", value: "9007199254740993" },
        },
        totalBudgetMilliseconds: 1_000,
      }),
    ).rejects.toEqual(nativeError);
    expect(tauri.invoke.mock.calls[0]?.[1]).toMatchObject({
      request: { expectedRevision: Number.MAX_SAFE_INTEGER },
    });
    expect(tauri.invoke.mock.calls[1]?.[1]).toMatchObject({
      request: { condition: { value: { value: "9007199254740993" } } },
    });
  });

  it.each(["9223372036854775808", "-9223372036854775809", "01", "-0"])(
    "rejects noncanonical or overflowing signed value %s",
    (value) => {
      expect(() =>
        generated.startCounterfactual({
          scenarioId,
          expectedRevision: 0,
          baseSolutionId: operationId,
          condition: {
            type: "forceAssignmentValue",
            assignmentId: "shift.primary",
            value: { type: "integer", value },
          },
          totalBudgetMilliseconds: 1_000,
        }),
      ).toThrow(RangeError);
      expect(tauri.invoke).not.toHaveBeenCalled();
    },
  );

  it("rejects excessive counterfactual budgets before native entry", async () => {
    await expect(
      generated.startCounterfactual({
        scenarioId,
        expectedRevision: 0,
        baseSolutionId: operationId,
        condition: {
          type: "forceAssignmentValue",
          assignmentId: "shift.primary",
          value: { type: "boolean", value: true },
        },
        totalBudgetMilliseconds: generated.COUNTERFACTUAL_TOTAL_BUDGET_MAX_MILLISECONDS_V1 + 1,
      }),
    ).rejects.toBeInstanceOf(RangeError);
    expect(tauri.invoke).not.toHaveBeenCalled();
  });

  it("keeps transfer paths and unopened bytes behind native custody", async () => {
    tauri.invoke.mockRejectedValue(nativeError);
    const options = {
      restoreMode: "import-scenario",
      includeResults: true,
      includeAssets: false,
    } as const;
    for (const operation of [
      () => generated.previewImport(options),
      () => generated.previewRestore(options),
      () => generated.createBackup("Before migration", operationId),
      () => generated.createExport(scenarioId, operationId),
      () => generated.inspectUnopenedBundle(),
      () => generated.reexportUnopenedBundle(operationId),
      () => generated.cancelPortablePreview(operationId),
    ])
      await expect(operation()).rejects.toEqual(nativeError);
    for (const [, payload] of tauri.invoke.mock.calls) {
      expect(
        collectKeys(payload).some((key) =>
          /^(?:bytes|sourceArtifact|fileName|path|url)$/i.test(key),
        ),
      ).toBe(false);
    }
  });
});

describe("setup operation ownership using the pinned SDK Channel", () => {
  it("validates nested setup pages and Unicode scalar limits from the authoritative schema", () => {
    const item = { entityId: scenarioId, kind: "person", name: "\u{10400}".repeat(256) };
    const continuation = {
      schemaVersion: 1,
      scenarioId,
      revision: 3,
      queryFingerprint: Array<number>(32).fill(0),
      position: { kind: "entity", entityId: scenarioId },
    };
    const page = {
      schemaVersion: 1,
      result: { kind: "entityPage", data: { items: [item], totalItems: 2, continuation } },
    };
    const accepts = (value: unknown) =>
      isWorkforceSetupQueryResult("eutheto.setup.entity_page", value);
    expect(accepts(page)).toBe(true);
    for (const data of [
      { ...page.result.data, items: [{ ...item, name: item.name + "a" }] },
      { ...page.result.data, items: [{ ...item, name: "\ud800" }] },
      { ...page.result.data, continuation: { ...continuation, schemaVersion: 2 } },
      { ...page.result.data, continuation: { ...continuation, queryFingerprint: [256] } },
      {
        ...page.result.data,
        continuation: { ...continuation, position: { kind: "unknown", entityId: scenarioId } },
      },
      { ...page.result.data, items: [{ ...item, rawRecord: {} }] },
    ])
      expect(accepts({ ...page, result: { ...page.result, data } })).toBe(false);
    expect(accepts({ ...page, result: { ...page.result, kind: "entityDetail" } })).toBe(false);
  });

  it("ignores malformed advisory events and isolates throwing listeners without losing unsubscription", async () => {
    const registered = deferred<(event: { readonly payload: unknown }) => void>();
    const unlisten = vi.fn();
    tauriEvents.listen.mockImplementation(
      (_topic: string, callback: (event: { readonly payload: unknown }) => void) => {
        registered.resolve(callback);
        return Promise.resolve(unlisten);
      },
    );
    const seen: string[] = [];
    const stop = await generated.onLibraryRefreshRequired((event) => {
      seen.push(event.reason);
      throw new Error("presentation failure");
    });
    const callback = await registered.promise;
    callback({ payload: { reason: "unknown" } });
    callback({ payload: { reason: "event-subscription-lagged", unexpected: true } });
    callback({ payload: { reason: "event-subscription-lagged" } });
    callback({ payload: { reason: "event-subscription-lagged" } });
    expect(seen).toEqual(["event-subscription-lagged", "event-subscription-lagged"]);
    stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("releases a late preparation after disposal without claiming or registering a channel", async () => {
    const native = nativeOperations();
    const scope = new generated.SetupOperationScope(scenarioId, 3);
    const operation = generated.getScenarioSummary(scope);
    scope.dispose();
    const failure = expect(operation.result).rejects.toMatchObject({
      code: "operation.context_disposed",
    });
    native.prepared();
    await failure;
    expect(native.controls).toEqual(["operation_release"]);
    expect(tauri.invoke.mock.calls.map(([command]) => command)).not.toContain(
      "scenario_get_summary",
    );
    expect(callbacks.size).toBe(0);
    expect(operation.isCurrent()).toBe(false);
  });

  it("waits for preclaim cancellation and rechecks disposal before native work", async () => {
    const native = nativeOperations();
    const cancel = deferred<unknown>();
    const cancelled = deferred<Invocation>();
    const original = tauri.invoke.getMockImplementation();
    tauri.invoke.mockImplementation((command: string, input: Invocation) => {
      if (command === "operation_cancel") {
        cancelled.resolve(input);
        return cancel.promise;
      }
      return original?.(command, input);
    });
    const scope = new generated.SetupOperationScope(scenarioId, 3);
    const operation = generated.getScenarioSummary(scope);
    const acknowledgement = operation.cancel();
    native.prepared();
    const cancelInput = await cancelled.promise;
    expect(callbacks.size).toBe(0);
    scope.dispose();
    const failure = expect(operation.result).rejects.toMatchObject({
      code: "operation.context_disposed",
    });
    cancel.resolve(
      response(cancelInput, { schemaVersion: 1, acknowledgement: "cancellationRequested" }),
    );
    await acknowledgement;
    await failure;
    expect(tauri.invoke.mock.calls.map(([command]) => command)).not.toContain(
      "scenario_get_summary",
    );
    expect(callbacks.size).toBe(0);
  });

  it("detaches the SDK callback after pre-entry rejection with no channel end, keeping the error presentable", async () => {
    const native = nativeOperations();
    const scope = new generated.SetupOperationScope(scenarioId, 3);
    const operation = generated.getScenarioSummary(scope);
    native.prepared();
    await native.claimed.promise;
    expect(callbacks.size).toBe(1);
    const failure = expect(operation.result).rejects.toMatchObject(invalidResponse);
    native.terminal.reject("ACL rejected the invoke before Rust entry");
    await failure;
    expect(callbacks.size).toBe(0);
    expect(operation.isCurrent()).toBe(true);
    await operation.release();
    expect(native.controls).toEqual(["operation_release"]);
    expect(operation.isCurrent()).toBe(false);
  });

  it("uses sequence rather than wallclock and survives invalid frames and throwing progress consumers", async () => {
    const native = nativeOperations();
    const scope = new generated.SetupOperationScope(scenarioId, 3);
    const sequences: number[] = [];
    const operation = generated.getScenarioSummary(scope, (event) => {
      sequences.push(event.sequence);
      if (event.sequence === 1) throw new Error("consumer failed");
    });
    native.prepared();
    const input = await native.claimed.promise;
    const callback = input.onProgress && callbacks.get(input.onProgress.id);
    if (!callback) throw new Error("No SDK callback");
    const progress: generated.OperationProgressV1 = {
      eventVersion: 1,
      timestamp: "2026-09-10T12:00:00Z",
      operationId,
      requestId: operation.requestId,
      windowLabel: "main",
      context: scope.context,
      sequence: 1,
      phase: "capturingSnapshot",
    };
    const messages = [
      progress,
      { ...progress, sequence: 2, timestamp: "invalid" },
      { ...progress, sequence: 2, windowLabel: "other" },
      { ...progress, sequence: 2, requestId: scenarioId },
      { ...progress, sequence: 2, context: { ...scope.context, expectedRevision: 4 } },
      { ...progress, sequence: 2, timestamp: "2026-09-10T11:59:59Z" },
      { ...progress, sequence: 1 },
    ];
    messages.forEach((message, index) => {
      callback({ message, index });
    });
    expect(sequences).toEqual([1, 2]);
    native.terminal.resolve(response(input, summary(), 3));
    expect((await operation.result).result.full).toEqual({ state: "notRun" });
    expect(callbacks.size).toBe(0);
  });

  it("preserves committed success after cancellation and explicit presentation release", async () => {
    const native = nativeOperations();
    const scope = new generated.SetupOperationScope(scenarioId, 3);
    const operation = generated.getScenarioSummary(scope);
    native.prepared();
    const input = await native.claimed.promise;
    await operation.cancel();
    expect(operation.isCurrent()).toBe(true);
    await operation.release();
    expect(callbacks.size).toBe(0);
    native.terminal.resolve(response(input, summary(), 3));
    expect((await operation.result).result.revision).toBe(3);
    expect(operation.isCurrent()).toBe(false);
  });

  it("captures draft input before preparation instead of sending subsequent presentation edits", async () => {
    const native = nativeOperations();
    const scope = new generated.SetupOperationScope(scenarioId, 3);
    const input = { kind: "person", search: "Alice", limit: 10 } as const;
    const draft = { ...input };
    const operation = generated.searchScenarioEntities(scope, draft);
    Reflect.set(draft, "search", "Bob");
    native.prepared();
    const invocation = await native.claimed.promise;
    expect(invocation.request.search).toBe("Alice");
    const failure = expect(operation.result).rejects.toEqual(nativeError);
    native.terminal.reject(nativeError);
    await failure;
  });

  it("rejects a valid summary from another revision rather than presenting stale readiness", async () => {
    const native = nativeOperations();
    const scope = new generated.SetupOperationScope(scenarioId, 3);
    const operation = generated.getScenarioSummary(scope);
    native.prepared();
    const input = await native.claimed.promise;
    const failure = expect(operation.result).rejects.toMatchObject(invalidResponse);
    native.terminal.resolve(response(input, { ...summary(), revision: 4 }, 4));
    await failure;
    expect(callbacks.size).toBe(0);
  });
});
