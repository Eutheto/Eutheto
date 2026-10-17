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
import peopleCsvNative from "./fixtures/people-csv-native.json";
import assignmentCsvNative from "./fixtures/assignment-csv-native.json";
import eligibilityCsvNative from "./fixtures/eligibility-csv-native.json";
import shiftCsvNative from "./fixtures/shift-csv-native.json";
import availabilityCsvNative from "./fixtures/availability-csv-native.json";
import eligibilityMatrixNative from "./fixtures/eligibility-matrix-native.json";

const scenarioId = "01900000-0000-7000-8000-000000000001";
const operationId = "01900000-0000-7000-8000-000000000002";
const solveRunId = "01900000-0000-7000-8000-000000000003";
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
function solveJob(
  requestId: string,
  status: generated.SolveJobStatusV1 = "running",
): generated.SolveJobDtoV2 {
  return {
    schemaVersion: 2,
    requestId,
    runId: solveRunId,
    scenarioId,
    scenarioRevision: 3,
    status,
    startedAt: "2026-09-10T12:00:00Z",
    finishedAt: status === "running" ? null : "2026-09-10T12:00:03Z",
    solutionId: status === "optimal" || status === "feasible" ? operationId : null,
    verificationChecksum: status === "optimal" || status === "feasible" ? "a".repeat(64) : null,
    checkpoint: null,
    cancelOperationId: null,
    stale: false,
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
function currentStatusFixture(
  certainty: generated.ExplanationCertainty = "independentlyVerified",
): generated.SolutionExplanationDtoV2 {
  const result = structuredClone(optimalityStatus) as unknown as generated.SolutionExplanationDtoV2;
  Reflect.set(result, "schemaVersion", 2);
  Reflect.set(result.explanation, "schemaVersion", 2);
  Reflect.set(result.explanation.evidence, "schemaVersion", 2);
  const payload = result.explanation.evidence.evidence;
  if (payload.kind !== "optimalityStatus") throw new Error("Fixture is not an optimality status");
  Reflect.set(payload.status, "schemaVersion", 2);
  Reflect.set(result.explanation.evidence, "certainty", certainty);
  return result;
}

function solutionDifferenceFixture(): generated.SolutionExplanationDtoV2 {
  const base = {
    solutionId: operationId,
    resultChecksum: "a".repeat(64),
  };
  const candidate = {
    solutionId: "01900000-0000-7000-8000-000000000004",
    resultChecksum: "b".repeat(64),
  };
  const comparison: generated.SolutionComparisonV2 = {
    schemaVersion: 2,
    base: {
      packId: "official.test",
      scenarioId,
      scenarioRevision: 1,
      documentHash: "c".repeat(64),
      projectionVersion: 1,
      verificationScopeChecksum: "d".repeat(64),
      acceptedResult: base,
    },
    candidate: {
      packId: "official.test",
      scenarioId,
      scenarioRevision: 2,
      documentHash: "e".repeat(64),
      projectionVersion: 1,
      verificationScopeChecksum: "f".repeat(64),
      acceptedResult: candidate,
    },
    assignments: [],
    rules: [],
    score: { type: "unavailable", reason: "policyOrScopeMismatch" },
    metrics: [],
    protections: [],
    runs: {
      base: {
        runId: "01900000-0000-7000-8000-000000000005",
        runManifestChecksum: "1".repeat(64),
        outcome: {
          type: "accepted",
          status: "optimal",
          solutionId: base.solutionId,
          acceptedResultChecksum: base.resultChecksum,
          verificationChecksum: "2".repeat(64),
        },
      },
      candidate: {
        runId: "01900000-0000-7000-8000-000000000006",
        runManifestChecksum: "3".repeat(64),
        outcome: {
          type: "accepted",
          status: "optimal",
          solutionId: candidate.solutionId,
          acceptedResultChecksum: candidate.resultChecksum,
          verificationChecksum: "4".repeat(64),
        },
      },
    },
    affectedEntities: [],
    entityObservations: [],
    checksum: "5".repeat(64),
  };
  return {
    schemaVersion: 2,
    scenarioId,
    currentRevision: 3,
    scenarioRevisions: [1, 2],
    explanation: {
      schemaVersion: 2,
      evidence: {
        schemaVersion: 2,
        certainty: "deterministic",
        evidence: { kind: "solutionDifference", comparison },
        checksum: "6".repeat(64),
      },
      rendered: {
        schemaVersion: 1,
        kind: "solutionDifference",
        evidenceChecksum: "6".repeat(64),
        messages: [],
      },
      checksum: "7".repeat(64),
    },
  };
}

function repairFixture(): generated.SolutionExplanationDtoV2 {
  const result = solutionDifferenceFixture();
  const payload = result.explanation.evidence.evidence;
  if (payload.kind !== "solutionDifference") throw new Error("Fixture is not a Difference");
  Reflect.set(result, "scenarioRevisions", [2, 1]);
  Reflect.set(result.explanation.evidence, "evidence", {
    kind: "repair",
    repair: { comparison: payload.comparison, causality: "notEstablished" },
  });
  Reflect.set(result.explanation.rendered, "kind", "repair");
  return result;
}

describe("strict desktop response boundary", () => {
  const custodyChecksum = "b".repeat(64);
  const retainedConflictId = `infeasibility.${custodyChecksum}`;
  it.each([
    {
      name: "accepts a retained conflict",
      payloadKind: "conflict",
      contextPatch: {},
      requestConflictId: retainedConflictId,
      expected: "admit",
    },
    {
      name: "accepts retained unavailable evidence",
      payloadKind: "unavailable",
      contextPatch: {},
      requestConflictId: retainedConflictId,
      expected: "admit",
    },
    {
      name: "accepts unavailable evidence without a requested conflict or context",
      payloadKind: "unavailable",
      contextPresent: false,
      contextPatch: {},
      requestConflictId: null,
      expected: "admit",
    },
    {
      name: "rejects a requested conflict without unavailable evidence context",
      payloadKind: "unavailable",
      contextPresent: false,
      contextPatch: {},
      requestConflictId: retainedConflictId,
      expected: "reject",
    },
    {
      name: "rejects another run",
      payloadKind: "conflict",
      contextPatch: { runId: operationId },
      requestConflictId: retainedConflictId,
      expected: "reject",
    },
    {
      name: "rejects another manifest",
      payloadKind: "conflict",
      contextPatch: { runManifestChecksum: "c".repeat(64) },
      requestConflictId: retainedConflictId,
      expected: "reject",
    },
    {
      name: "rejects another custody checksum",
      payloadKind: "conflict",
      contextPatch: { evidenceChecksum: "c".repeat(64) },
      requestConflictId: retainedConflictId,
      expected: "reject",
    },
    {
      name: "rejects another conflict identity",
      payloadKind: "conflict",
      contextPatch: { conflictId: "infeasibility.other" },
      requestConflictId: retainedConflictId,
      expected: "reject",
    },
    {
      name: "rejects a request for another conflict",
      payloadKind: "conflict",
      contextPatch: {},
      requestConflictId: "infeasibility.other",
      expected: "reject",
    },
    {
      name: "rejects another unavailable custody checksum",
      payloadKind: "unavailable",
      contextPatch: { evidenceChecksum: "c".repeat(64) },
      requestConflictId: retainedConflictId,
      expected: "reject",
    },
    {
      name: "rejects a request mismatch for unavailable evidence",
      payloadKind: "unavailable",
      contextPatch: {},
      requestConflictId: "infeasibility.other",
      expected: "reject",
    },
  ])(
    "$name while keeping custody and general evidence checksums distinct",
    async ({ payloadKind, contextPresent = true, contextPatch, requestConflictId, expected }) => {
      const native = nativeOperations();
      const scope = new generated.SetupOperationScope(scenarioId, 3);
      const operation = generated.explainSolution(scope, {
        schemaVersion: 1,
        subject: {
          kind: "infeasibility",
          solveRunId,
          runManifestChecksum: "a".repeat(64),
          conflictId: requestConflictId,
        },
      });
      native.prepared();
      const input = await native.claimed.promise;
      const retainedInfeasibility =
        payloadKind === "conflict"
          ? {
              type: "conflict",
              groups: [{ groupId: "coverage.required", requiredRules: [operationId] }],
              minimality: "sufficient",
              shrink: {
                initialGroupCount: 1,
                remainingGroupCount: 1,
                attemptedTrials: 0,
                maxTrials: 0,
                stopReason: "notAttempted",
              },
            }
          : { type: "unavailable", reason: "diagnosticFailed" };
      const result = {
        schemaVersion: 2,
        scenarioId,
        currentRevision: 3,
        scenarioRevisions: [3],
        ...(contextPresent
          ? {
              infeasibilityContext: {
                runId: solveRunId,
                sourceRevision: 3,
                runManifestChecksum: "a".repeat(64),
                evidenceChecksum: custodyChecksum,
                conflictId: retainedConflictId,
                ...contextPatch,
              },
            }
          : {}),
        explanation: {
          schemaVersion: 2,
          checksum: "d".repeat(64),
          evidence: {
            schemaVersion: 2,
            certainty: payloadKind === "conflict" ? "sufficientConflict" : "unavailable",
            checksum: "e".repeat(64),
            evidence: {
              kind: "infeasibility",
              infeasibility: retainedInfeasibility,
            },
          },
          rendered: {
            schemaVersion: 1,
            kind: "infeasibility",
            evidenceChecksum: "e".repeat(64),
            messages: [],
          },
        },
      };
      const reply = response(input, result, 3);
      if (expected === "admit") {
        const admitted = operation.result;
        native.terminal.resolve(reply);
        const value = await admitted;
        if (contextPresent) {
          expect(value.result.infeasibilityContext).toMatchObject({
            runId: solveRunId,
            sourceRevision: 3,
            runManifestChecksum: "a".repeat(64),
            evidenceChecksum: custodyChecksum,
            conflictId: requestConflictId,
          });
          expect(value.result.infeasibilityContext?.evidenceChecksum).not.toBe(
            value.result.explanation.evidence.checksum,
          );
        } else {
          expect(value.result.infeasibilityContext).toBeUndefined();
        }
        const evidence = value.result.explanation.evidence.evidence;
        if (evidence.kind !== "infeasibility") {
          throw new Error("Fixture is not infeasibility evidence");
        }
        if (payloadKind === "unavailable") {
          expect(evidence.infeasibility).toEqual({
            type: "unavailable",
            reason: "diagnosticFailed",
          });
        } else {
          expect(evidence.infeasibility).toMatchObject({ type: "conflict" });
        }
      } else {
        const rejected = expect(operation.result).rejects.toMatchObject({ category: "protocol" });
        native.terminal.resolve(reply);
        await rejected;
      }
      await operation.release();
      scope.dispose();
    },
  );

  it("does not rebind explanation admission when the caller edits during preparation", async () => {
    const native = nativeOperations();
    const result = currentStatusFixture();
    const payload = result.explanation.evidence.evidence;
    if (payload.kind !== "optimalityStatus") throw new Error("Fixture is not an optimality status");
    const status = payload.status;
    const revision = status.runInput.scenarioRevision;
    const scope = new generated.SetupOperationScope(result.scenarioId, revision);
    const request: generated.ExplanationRequestV1 = {
      schemaVersion: 1,
      subject: {
        kind: "infeasibility",
        solveRunId,
        runManifestChecksum: "a".repeat(64),
        conflictId: null,
      },
    };
    const operation = generated.explainSolution(scope, request);
    Reflect.set(request, "subject", {
      kind: "optimalityStatus",
      solveRunId: status.runInput.runId,
      runManifestChecksum: status.runManifest.checksum,
      result: status.result,
    });
    native.prepared();
    const input = await native.claimed.promise;
    const rejected = expect(operation.result).rejects.toMatchObject({ category: "protocol" });
    native.terminal.resolve(response(input, result, result.currentRevision));
    await rejected;
    await operation.release();
    scope.dispose();
  });

  it("rejects an oversized explanation before running caller serialization", () => {
    const scope = new generated.SetupOperationScope(scenarioId, 3);
    const request: generated.ExplanationRequestV1 = {
      schemaVersion: 1,
      subject: {
        kind: "assignment",
        result: { solutionId: solveRunId, resultChecksum: "a".repeat(64) },
        assignmentId: "x".repeat(64 * 1024),
      },
    };
    Object.defineProperty(request, "toJSON", {
      value: () => {
        throw new Error("Caller serialization ran before admission");
      },
    });
    expect(() => generated.explainSolution(scope, request)).toThrow(RangeError);
    scope.dispose();
  });
  it("rejects current statusV2 that claims backend proof from a recorded optimal outcome", async () => {
    const native = nativeOperations();
    const result = currentStatusFixture("backendProof");
    const payload = result.explanation.evidence.evidence;
    if (payload.kind !== "optimalityStatus") throw new Error("Fixture is not an optimality status");
    const status = payload.status;
    const scope = new generated.SetupOperationScope(
      result.scenarioId,
      status.runInput.scenarioRevision,
    );
    const request: generated.ExplanationRequestV1 = {
      schemaVersion: 1,
      subject: {
        kind: "optimalityStatus",
        solveRunId: status.runInput.runId,
        runManifestChecksum: status.runManifest.checksum,
        result: status.result,
      },
    };
    const operation = generated.explainSolution(scope, request);
    native.prepared();
    const input = await native.claimed.promise;
    const rejected = expect(operation.result).rejects.toMatchObject({ category: "protocol" });
    native.terminal.resolve(response(input, result, result.currentRevision));
    await rejected;
    await operation.release();
    scope.dispose();
  });

  it("admits current Difference evidence only at its candidate anchor with exact ordered source refs", async () => {
    const native = nativeOperations();
    const result = solutionDifferenceFixture();
    const payload = result.explanation.evidence.evidence;
    if (payload.kind !== "solutionDifference") throw new Error("Fixture is not a Difference");
    const comparison = payload.comparison;
    const request: generated.ExplanationRequestV1 = {
      schemaVersion: 1,
      subject: {
        kind: "solutionDifference",
        left: comparison.base.acceptedResult,
        right: comparison.candidate.acceptedResult,
      },
    };
    const scope = new generated.SetupOperationScope(scenarioId, 2);
    const operation = generated.explainSolution(scope, request);
    native.prepared();
    const input = await native.claimed.promise;
    expect(input.request).toMatchObject({ sourceRevision: 2, scenarioId });
    native.terminal.resolve(response(input, result, 3));
    await expect(operation.result).resolves.toMatchObject({
      result: { currentRevision: 3, scenarioRevisions: [1, 2] },
    });
    await operation.release();
    scope.dispose();
  });

  it.each([
    "base accepted result",
    "candidate accepted result",
    "base run outcome",
    "candidate run outcome",
    "source revision ordering",
  ])("rejects Difference evidence with mismatched %s", async (mismatch) => {
    const native = nativeOperations();
    const result = solutionDifferenceFixture();
    const payload = result.explanation.evidence.evidence;
    if (payload.kind !== "solutionDifference") throw new Error("Fixture is not a Difference");
    const comparison = payload.comparison;
    switch (mismatch) {
      case "base accepted result":
        Reflect.set(comparison.base.acceptedResult, "resultChecksum", "8".repeat(64));
        break;
      case "candidate accepted result":
        Reflect.set(
          comparison.candidate.acceptedResult,
          "solutionId",
          "01900000-0000-7000-8000-000000000007",
        );
        break;
      case "base run outcome":
        if (comparison.runs.base.outcome.type !== "accepted")
          throw new Error("Fixture run is not accepted");
        Reflect.set(comparison.runs.base.outcome, "acceptedResultChecksum", "9".repeat(64));
        break;
      case "candidate run outcome":
        if (comparison.runs.candidate.outcome.type !== "accepted")
          throw new Error("Fixture run is not accepted");
        Reflect.set(comparison.runs.candidate.outcome, "solutionId", operationId);
        break;
      case "source revision ordering":
        Reflect.set(result, "scenarioRevisions", [2, 1]);
        break;
    }
    const request: generated.ExplanationRequestV1 = {
      schemaVersion: 1,
      subject: {
        kind: "solutionDifference",
        left: { solutionId: operationId, resultChecksum: "a".repeat(64) },
        right: {
          solutionId: "01900000-0000-7000-8000-000000000004",
          resultChecksum: "b".repeat(64),
        },
      },
    };
    const scope = new generated.SetupOperationScope(scenarioId, 2);
    const operation = generated.explainSolution(scope, request);
    native.prepared();
    const input = await native.claimed.promise;
    const rejected = expect(operation.result).rejects.toMatchObject({ category: "protocol" });
    native.terminal.resolve(response(input, result, 3));
    await rejected;
    await operation.release();
    scope.dispose();
  });

  it("admits Repair evidence with the current candidate anchored before its saved base", async () => {
    const native = nativeOperations();
    const result = repairFixture();
    const payload = result.explanation.evidence.evidence;
    if (payload.kind !== "repair") throw new Error("Fixture is not Repair evidence");
    const comparison = payload.repair.comparison;
    const request: generated.ExplanationRequestV1 = {
      schemaVersion: 1,
      subject: {
        kind: "repair",
        current: comparison.candidate.acceptedResult,
        base: comparison.base.acceptedResult,
      },
    };
    const scope = new generated.SetupOperationScope(scenarioId, 2);
    const operation = generated.explainSolution(scope, request);
    native.prepared();
    const input = await native.claimed.promise;
    expect(input.request).toMatchObject({ sourceRevision: 2, scenarioId });
    native.terminal.resolve(response(input, result, 3));
    await expect(operation.result).resolves.toMatchObject({
      result: {
        currentRevision: 3,
        scenarioRevisions: [2, 1],
        explanation: { evidence: { evidence: { repair: { causality: "notEstablished" } } } },
      },
    });
    await operation.release();
    scope.dispose();
  });

  it.each(["current result", "saved repair base", "source revision ordering"])(
    "rejects Repair evidence with mismatched %s",
    async (mismatch) => {
      const native = nativeOperations();
      const result = repairFixture();
      const payload = result.explanation.evidence.evidence;
      if (payload.kind !== "repair") throw new Error("Fixture is not Repair evidence");
      const comparison = payload.repair.comparison;
      switch (mismatch) {
        case "current result":
          Reflect.set(comparison.candidate.acceptedResult, "resultChecksum", "8".repeat(64));
          break;
        case "saved repair base":
          Reflect.set(
            comparison.base.acceptedResult,
            "solutionId",
            "01900000-0000-7000-8000-000000000007",
          );
          break;
        case "source revision ordering":
          Reflect.set(result, "scenarioRevisions", [1, 2]);
          break;
      }
      const request: generated.ExplanationRequestV1 = {
        schemaVersion: 1,
        subject: {
          kind: "repair",
          current: {
            solutionId: "01900000-0000-7000-8000-000000000004",
            resultChecksum: "b".repeat(64),
          },
          base: { solutionId: operationId, resultChecksum: "a".repeat(64) },
        },
      };
      const scope = new generated.SetupOperationScope(scenarioId, 2);
      const operation = generated.explainSolution(scope, request);
      native.prepared();
      const input = await native.claimed.promise;
      const rejected = expect(operation.result).rejects.toMatchObject({ category: "protocol" });
      native.terminal.resolve(response(input, result, 3));
      await rejected;
      await operation.release();
      scope.dispose();
    },
  );

  it.each([
    null,
    { operationId: solveRunId },
    { runId: operationId },
    { evidenceChecksum: "c".repeat(64) },
    { path: "/private/diagnostic.json" },
  ])("admits only the exact path-free historical diagnostic export receipt %#", async (patch) => {
    const native = nativeOperations();
    const operation = generated.exportDiagnostic(new generated.SetupOperationScope(scenarioId, 2), {
      runId: solveRunId,
      runManifestChecksum: "a".repeat(64),
      evidenceChecksum: "b".repeat(64),
    });
    native.prepared();
    const input = await native.claimed.promise;
    const receipt = {
      schemaVersion: 1,
      operationId,
      requestId: input.request.requestId,
      scenarioId,
      sourceRevision: 2,
      observedCurrentRevision: 3,
      runId: solveRunId,
      runManifestChecksum: "a".repeat(64),
      evidenceChecksum: "b".repeat(64),
      ...patch,
    };
    const terminal =
      patch === null
        ? expect(operation.result).resolves.toMatchObject({ result: receipt, currentRevision: 3 })
        : expect(operation.result).rejects.toMatchObject(invalidResponse);
    native.terminal.resolve(response(input, receipt, 3));
    await terminal;
  });

  it.each([
    ["historical absent context", true],
    ["authored context one", true],
    ["in-snapshot original context one", true],
    ["in-snapshot manual context one", true],
    ["captured original context two", true],
    ["opaque capture leaves pack admission native", true],
    ["capture at byte ceiling", true],
    ["UTF-8 capture at byte ceiling", true],
    ["capture above byte ceiling", false],
    ["escaped capture above byte ceiling", false],
    ["complete explanation above unchanged ceiling", false],
    ["context one with capture", false],
    ["context two without capture", false],
    ["context two without baseline", false],
    ["captured manual selection", false],
    ["null capture", false],
    ["non-JSON capture", false],
    ["invalid Unicode capture", false],
    ["unknown context version", false],
    ["unknown binding field", false],
  ] as const)("guards versioned repair carriers: %s", async (boundary, admit) => {
    const native = nativeOperations();
    const result = currentStatusFixture();
    const payload = result.explanation.evidence.evidence;
    if (payload.kind !== "optimalityStatus") throw new Error("Fixture is not an optimality status");
    const status = payload.status;
    if (status.result === null) throw new Error("Fixture has no accepted result");
    const baseline: generated.RunBaselineBinding = {
      original: {
        entity: { kind: "baseSchedule", id: operationId },
        pairContentHash: "a".repeat(64),
        acceptedResult: status.result,
        sourceScenarioId: result.scenarioId,
        sourceRevision: status.runInput.scenarioRevision,
        sourceDocumentHash: status.runInput.snapshotDocumentHash,
        sourceRunId: status.runInput.runId,
        sourceRunInputChecksum: status.runInput.checksum,
        sourceRunManifestChecksum: status.runManifest.checksum,
      },
      selection: { kind: "originalAccepted" },
      selectedPairContentHash: "a".repeat(64),
      capturedOriginal: {
        kind: "baseSchedule",
        id: operationId,
        sourceSolutionId: status.result.solutionId,
        sourceRevision: status.runInput.scenarioRevision,
        assignments: [{ personId: scenarioId, shiftId: solveRunId }],
      },
    };
    const context: generated.RunObjectiveContext = {
      schemaVersion: 2,
      packId: status.runInput.packId,
      profileId: "official.workforce.objective.minimal-changes",
      semanticVersion: 1,
      operation: "repair",
      baseline,
    };
    Reflect.set(status.runInput, "schemaVersion", 2);
    Reflect.set(status.runInput, "objectiveContext", context);
    switch (boundary) {
      case "historical absent context":
        Reflect.set(status.runInput, "schemaVersion", 1);
        Reflect.deleteProperty(status.runInput, "objectiveContext");
        break;
      case "authored context one":
        Reflect.set(context, "schemaVersion", 1);
        Reflect.set(context, "profileId", "eutheto.objective.authored");
        Reflect.set(context, "operation", "optimize");
        Reflect.deleteProperty(context, "baseline");
        break;
      case "in-snapshot original context one":
      case "in-snapshot manual context one":
        Reflect.set(context, "schemaVersion", 1);
        Reflect.deleteProperty(baseline, "capturedOriginal");
        break;
      case "opaque capture leaves pack admission native":
        // Structural IPC admission is not validation of the pack's closed BaseSchedule record.
        Reflect.set(baseline, "capturedOriginal", {});
        break;
      case "capture at byte ceiling":
      case "complete explanation above unchanged ceiling":
        Reflect.set(
          baseline,
          "capturedOriginal",
          "x".repeat(generated.MAX_CAPTURED_BASELINE_BYTES - 2),
        );
        if (boundary === "complete explanation above unchanged ceiling") {
          const message: generated.EvidenceMessageV1 = {
            messageKey: "test.render",
            parameters: { text: { type: "text", value: "x".repeat(800 * 1024) } },
            entities: [],
            rules: [],
            assignments: [],
            evidence: [],
          };
          Reflect.set(result.explanation.rendered, "messages", [message, message]);
        }
        break;
      case "UTF-8 capture at byte ceiling":
        Reflect.set(
          baseline,
          "capturedOriginal",
          "\u{10400}".repeat((generated.MAX_CAPTURED_BASELINE_BYTES - 4) / 4) + "xx",
        );
        break;
      case "capture above byte ceiling":
        Reflect.set(
          baseline,
          "capturedOriginal",
          "x".repeat(generated.MAX_CAPTURED_BASELINE_BYTES - 1),
        );
        break;
      case "escaped capture above byte ceiling":
        Reflect.set(
          baseline,
          "capturedOriginal",
          '"'.repeat(generated.MAX_CAPTURED_BASELINE_BYTES / 2),
        );
        break;
      case "context one with capture":
        Reflect.set(context, "schemaVersion", 1);
        break;
      case "context two without capture":
        Reflect.deleteProperty(baseline, "capturedOriginal");
        break;
      case "context two without baseline":
        Reflect.deleteProperty(context, "baseline");
        break;
      case "null capture":
        Reflect.set(baseline, "capturedOriginal", null);
        break;
      case "non-JSON capture":
        Reflect.set(baseline, "capturedOriginal", { value: Infinity });
        break;
      case "invalid Unicode capture":
        Reflect.set(baseline, "capturedOriginal", "\ud800");
        break;
      case "unknown context version":
        Reflect.set(context, "schemaVersion", 3);
        break;
      case "unknown binding field":
        Reflect.set(baseline, "unrecognized", true);
        break;
      case "captured original context two":
      case "captured manual selection":
        break;
    }
    if (boundary === "in-snapshot manual context one" || boundary === "captured manual selection") {
      Reflect.set(baseline, "selection", {
        kind: "manualSelection",
        entity: { kind: "manualSelection", id: solveRunId },
        recordChecksum: "b".repeat(64),
        admissionSourceScenarioId: result.scenarioId,
        admissionSourceRevision: status.runInput.scenarioRevision,
        admissionSourceDocumentHash: status.runInput.snapshotDocumentHash,
      });
    }
    const scope = new generated.SetupOperationScope(
      result.scenarioId,
      status.runInput.scenarioRevision,
    );
    const operation = generated.explainSolution(scope, {
      schemaVersion: 1,
      subject: {
        kind: "optimalityStatus",
        solveRunId: status.runInput.runId,
        runManifestChecksum: status.runManifest.checksum,
        result: status.result,
      },
    });
    native.prepared();
    const input = await native.claimed.promise;
    const terminal = admit
      ? expect(operation.result).resolves.toMatchObject({ result })
      : expect(operation.result).rejects.toMatchObject(invalidResponse);
    native.terminal.resolve(response(input, result, result.currentRevision));
    await terminal;
    await operation.release();
    scope.dispose();
  });

  it("preserves safe millisecond durations beyond u32 and rejects unrepresentable worker counts", async () => {
    const result = currentStatusFixture();
    const payload = result.explanation.evidence.evidence;
    if (payload.kind !== "optimalityStatus") throw new Error("Fixture is not an optimality status");
    const status = payload.status;
    const sourceRevision = status.runInput.scenarioRevision;
    // The retained source is historical; the response also reports a newer current revision.
    Reflect.set(result, "scenarioRevisions", [sourceRevision]);
    Reflect.set(result, "currentRevision", sourceRevision + 1);
    Reflect.set(status.runInput.solveOptions, "timeLimitMilliseconds", Number.MAX_SAFE_INTEGER);
    Reflect.set(status.runManifest, "elapsedMilliseconds", Number.MAX_SAFE_INTEGER);
    Reflect.set(status.runManifest.phaseTimings, "backendMilliseconds", Number.MAX_SAFE_INTEGER);
    Reflect.set(status.runInput.solveOptions.workerThreads, "count", 65_535);
    const request: generated.ExplanationRequestV1 = {
      schemaVersion: 1,
      subject: {
        kind: "optimalityStatus",
        solveRunId: status.runInput.runId,
        runManifestChecksum: status.runManifest.checksum,
        result: status.result,
      },
    };
    tauri.invoke.mockImplementation((command: string, input: Invocation) => {
      if (command === "operation_prepare")
        return Promise.resolve(response(input, { schemaVersion: 1, operationId }));
      if (command === "operation_release")
        return Promise.resolve(response(input, { schemaVersion: 1, acknowledgement: "released" }));
      if (command === "operation_cancel")
        return Promise.resolve(
          response(input, {
            schemaVersion: 1,
            acknowledgement: "cancellationRequested",
          }),
        );
      return Promise.resolve(response(input, result, result.currentRevision));
    });
    const scope = new generated.SetupOperationScope(
      result.scenarioId,
      status.runInput.scenarioRevision,
    );
    const accepted = await generated.explainSolution(scope, request).result;
    expect(accepted.result.explanation.evidence.evidence).toMatchObject({
      status: { runManifest: { elapsedMilliseconds: Number.MAX_SAFE_INTEGER } },
    });
    Reflect.set(status.runInput.solveOptions.workerThreads, "count", 65_536);
    await expect(generated.explainSolution(scope, request).result).rejects.toMatchObject(
      invalidResponse,
    );
    Reflect.set(status.runInput.solveOptions.workerThreads, "count", 1);
    Reflect.set(status.runManifest, "elapsedMilliseconds", Number.MAX_SAFE_INTEGER + 1);
    await expect(generated.explainSolution(scope, request).result).rejects.toMatchObject(
      invalidResponse,
    );
  });

  it("admits a clock-marked native alarm but refuses fabricated timing or another outcome", async () => {
    const result = currentStatusFixture("deterministic");
    const payload = result.explanation.evidence.evidence;
    if (payload.kind !== "optimalityStatus") throw new Error("Fixture is not an optimality status");
    const status = payload.status;
    const manifest = status.runManifest;
    const alarm = { type: "verificationAlarm", diagnosticCode: "verification.projection_failed" };
    Reflect.set(status, "result", null);
    Reflect.set(manifest, "outcome", alarm);
    Reflect.set(manifest, "clockFailure", { kind: "unreliableObservation" });
    Reflect.set(manifest, "elapsedMilliseconds", null);
    Reflect.set(manifest, "firstIncumbentMilliseconds", null);
    Reflect.set(manifest, "firstVerifiedFeasibleMilliseconds", null);
    for (const key in manifest.phaseTimings) Reflect.set(manifest.phaseTimings, key, null);
    const request: generated.ExplanationRequestV1 = {
      schemaVersion: 1,
      subject: {
        kind: "optimalityStatus",
        solveRunId: status.runInput.runId,
        runManifestChecksum: manifest.checksum,
        result: null,
      },
    };
    tauri.invoke.mockImplementation((command: string, input: Invocation) => {
      if (command === "operation_prepare")
        return Promise.resolve(response(input, { schemaVersion: 1, operationId }));
      if (command === "operation_release")
        return Promise.resolve(response(input, { schemaVersion: 1, acknowledgement: "released" }));
      return Promise.resolve(response(input, result, result.currentRevision));
    });
    const scope = new generated.SetupOperationScope(
      result.scenarioId,
      status.runInput.scenarioRevision,
    );
    await expect(generated.explainSolution(scope, request).result).resolves.toMatchObject({
      result: {
        explanation: {
          evidence: {
            evidence: {
              status: {
                runManifest: { outcome: alarm, clockFailure: { kind: "unreliableObservation" } },
              },
            },
          },
        },
      },
    });
    for (const marker of [
      { kind: "unknown" },
      { kind: "unreliableObservation", measuredMilliseconds: 0 },
    ]) {
      Reflect.set(manifest, "clockFailure", marker);
      await expect(generated.explainSolution(scope, request).result).rejects.toMatchObject(
        invalidResponse,
      );
    }
    Reflect.set(manifest, "clockFailure", { kind: "unreliableObservation" });
    Reflect.set(manifest, "elapsedMilliseconds", 0);
    await expect(generated.explainSolution(scope, request).result).rejects.toMatchObject(
      invalidResponse,
    );
    Reflect.set(manifest, "elapsedMilliseconds", null);
    Reflect.set(manifest.phaseTimings, "backendMilliseconds", 0);
    await expect(generated.explainSolution(scope, request).result).rejects.toMatchObject(
      invalidResponse,
    );
    Reflect.set(manifest.phaseTimings, "backendMilliseconds", null);
    Reflect.set(manifest, "outcome", { type: "noResult", status: "infeasible" });
    await expect(generated.explainSolution(scope, request).result).rejects.toMatchObject(
      invalidResponse,
    );
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
      schemaVersion: 1,
      scenarioId,
      title: "Clinic",
      domainPackId: "official.workforce",
      revision: 3,
      updatedAt: "2026-09-10T12:00:00.123456789Z",
      archived: false,
      lastOpenedAt: null,
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
      (input: Invocation) => response(input, [{ ...project, schemaVersion: 2 }]),
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
  it("admits a native shift-calendar margin quoted from signed i64 without admitting malformed margins", async () => {
    const shift = {
      assignmentTypeId: operationId,
      assignmentTypeName: "Clinic",
      endsAt: { instant: "2030-01-16T17:00:00Z", local: "2030-01-16T17:00:00", offsetSeconds: 0 },
      locationId: null,
      locationName: null,
      margin: "-1",
      maximum: 1,
      minimum: 1,
      preferred: null,
      required: 1,
      selectedCount: 0,
      shiftId: solveRunId,
      startsAt: { instant: "2030-01-16T08:00:00Z", local: "2030-01-16T08:00:00", offsetSeconds: 0 },
    };
    const view = {
      schemaVersion: 1,
      scenarioId,
      currentRevision: 3,
      scenarioRevision: 3,
      result: { solutionId: operationId, resultChecksum: "a".repeat(64) },
      view: {
        viewId: "official.workforce.result.schedule",
        data: {
          schemaVersion: 1,
          timeZone: "UTC",
          result: {
            kind: "calendarShifts",
            planningDates: { startDate: "2030-01-01", endDateExclusive: "2030-02-01" },
            window: { startDate: "2030-01-15", endDateExclusive: "2030-01-22" },
            totalItems: 1,
            continuation: null,
            items: [{ date: "2030-01-16", selectedPersonIds: [], shift }],
          },
        },
      },
    };
    const input = {
      scenarioId,
      solutionId: operationId,
      viewId: view.view.viewId,
      parameters: { kind: "calendarShifts" as const, startDate: "2030-01-15" },
    };
    tauri.invoke.mockImplementation((_command: string, call: Invocation) =>
      Promise.resolve(response(call, view, view.currentRevision)),
    );
    await expect(generated.getSolutionView(input)).resolves.toMatchObject({
      result: { view: { data: { result: { items: [{ shift: { margin: "-1" } }] } } } },
    });
    Reflect.set(shift, "margin", -1);
    await expect(generated.getSolutionView(input)).resolves.toMatchObject({
      result: { view: { data: { result: { items: [{ shift: { margin: -1 } }] } } } },
    });
    for (const margin of ["-0", "4294967296", "not a number"]) {
      shift.margin = margin;
      await expect(generated.getSolutionView(input)).rejects.toMatchObject(invalidResponse);
    }
  });
  it("uses the V3 current comparison query with V2 semantic and score-unavailable evidence", async () => {
    const baseId = "01900000-0000-7000-8000-000000000010";
    const candidateId = "01900000-0000-7000-8000-000000000011";
    const digest = "a".repeat(64);
    const base = {
      solutionId: baseId,
      resultChecksum: "b".repeat(64),
    };
    const candidate = {
      solutionId: candidateId,
      resultChecksum: "c".repeat(64),
    };
    const comparison: generated.SolutionComparisonV2 = {
      schemaVersion: 2,
      base: {
        packId: "official.workforce",
        scenarioId,
        scenarioRevision: 1,
        documentHash: digest,
        projectionVersion: 1,
        verificationScopeChecksum: "d".repeat(64),
        acceptedResult: base,
      },
      candidate: {
        packId: "official.workforce",
        scenarioId,
        scenarioRevision: 2,
        documentHash: digest,
        projectionVersion: 1,
        verificationScopeChecksum: "e".repeat(64),
        acceptedResult: candidate,
      },
      assignments: [],
      rules: [],
      score: { type: "unavailable", reason: "policyOrScopeMismatch" },
      metrics: [],
      protections: [],
      runs: {
        base: {
          runId: solveRunId,
          runManifestChecksum: "f".repeat(64),
          outcome: {
            type: "accepted",
            status: "optimal",
            solutionId: base.solutionId,
            acceptedResultChecksum: base.resultChecksum,
            verificationChecksum: "1".repeat(64),
          },
        },
        candidate: {
          runId: operationId,
          runManifestChecksum: "2".repeat(64),
          outcome: {
            type: "accepted",
            status: "feasible",
            solutionId: candidate.solutionId,
            acceptedResultChecksum: candidate.resultChecksum,
            verificationChecksum: "3".repeat(64),
          },
        },
      },
      affectedEntities: [],
      entityObservations: [],
      checksum: "4".repeat(64),
    };
    const result: generated.SolutionComparisonDtoV3 = {
      schemaVersion: 3,
      scenarioId,
      currentRevision: 3,
      comparison,
    };
    const input = { scenarioId, baseSolutionId: baseId, candidateSolutionId: candidateId };
    tauri.invoke.mockImplementation((command: string, call: Invocation) => {
      expect(command).toBe("solution_compare");
      expect(call.request.schemaVersion).toBe(3);
      return Promise.resolve(response(call, result, 3));
    });
    await expect(generated.compareSolutions(input)).resolves.toMatchObject({
      result: {
        schemaVersion: 3,
        comparison: {
          schemaVersion: 2,
          score: { type: "unavailable", reason: "policyOrScopeMismatch" },
          runs: {
            base: { runId: solveRunId, outcome: { type: "accepted", status: "optimal" } },
            candidate: { runId: operationId, outcome: { type: "accepted", status: "feasible" } },
          },
        },
      },
    });
    expect(result.comparison.runs.base).not.toHaveProperty("certainty");
    expect(result.comparison.runs.candidate).not.toHaveProperty("certainty");
    expect((tauri.invoke.mock.calls[0]?.[1] as Invocation).request.schemaVersion).toBe(3);

    for (const mutation of [
      "unavailable score reason",
      "candidate run binding",
      "source revision",
    ]) {
      const malformed = structuredClone(result);
      switch (mutation) {
        case "unavailable score reason":
          if (malformed.comparison.score.type !== "unavailable")
            throw new Error("Fixture score should be unavailable");
          Reflect.set(malformed.comparison.score, "reason", "unknown");
          break;
        case "candidate run binding":
          if (malformed.comparison.runs.candidate.outcome.type !== "accepted")
            throw new Error("Fixture run should be accepted");
          Reflect.set(
            malformed.comparison.runs.candidate.outcome,
            "acceptedResultChecksum",
            "9".repeat(64),
          );
          break;
        case "source revision":
          Reflect.set(malformed.comparison.candidate, "scenarioRevision", 4);
          break;
      }
      tauri.invoke.mockImplementationOnce((_command: string, call: Invocation) =>
        Promise.resolve(response(call, malformed, 3)),
      );
      await expect(generated.compareSolutions(input)).rejects.toMatchObject(invalidResponse);
    }
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
      {
        ...page.result.data,
        items: [{ ...item, entityId: "0194AA00-0000-7000-8000-000000000201" }],
      },
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

  it("rejects oversized setup search before native preparation or draft snapshot", () => {
    const scope = new generated.SetupOperationScope(scenarioId, 3);
    expect(() =>
      generated.searchScenarioEntities(scope, {
        kind: "person",
        search: "x".repeat(2 * 64 * 1024),
      }),
    ).toThrow(RangeError);
    expect(tauri.invoke).not.toHaveBeenCalled();
    scope.dispose();
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

describe("native solve API lifecycle", () => {
  it("invokes bounded backend, durable job and run-list requests and rejects mismatched ownership", async () => {
    tauri.invoke.mockImplementation((command: string, input: Invocation) => {
      switch (command) {
        case "solve_get_backend_options":
          return Promise.resolve(
            response(
              input,
              {
                schemaVersion: 2,
                scenarioId,
                revision: 3,
                options: [
                  {
                    backendId: "solver.fixture",
                    backendVersion: "1.0.0",
                    adapterVersion: "1.0.0",
                    displayName: "Fixture solver",
                  },
                ],
              },
              3,
            ),
          );
        case "solve_get_job":
          return Promise.resolve(response(input, solveJob(operationId), 3));
        case "solve_list_runs":
          return Promise.resolve(
            response(
              input,
              {
                schemaVersion: 2,
                scenarioId,
                currentRevision: 3,
                runs: [solveJob(operationId)],
              },
              3,
            ),
          );
        default:
          throw new Error(`Unexpected native command: ${command}`);
      }
    });
    const backends = await generated.getSolveBackendOptions(scenarioId, 3);
    expect(backends.result.options[0]?.backendId).toBe("solver.fixture");
    const backendInput = tauri.invoke.mock.calls[0]?.[1] as Invocation;
    expect(backendInput.request).toMatchObject({
      schemaVersion: 2,
      scenarioId,
      expectedRevision: 3,
    });
    const job = await generated.getSolveJob(scenarioId, solveRunId);
    expect(job.result.runId).toBe(solveRunId);
    const runs = await generated.listSolveRuns(scenarioId, null, 1);
    expect(runs.result.runs).toHaveLength(1);
    expect(tauri.invoke.mock.calls[2]?.[1]).toMatchObject({
      request: { schemaVersion: 2, scenarioId, revision: null, limit: 1 },
    });
    await expect(generated.getSolveJob(scenarioId, operationId)).rejects.toMatchObject(
      invalidResponse,
    );
    expect(() => generated.listSolveRuns(scenarioId, null, 33)).toThrow(RangeError);
  });

  it("admits only run-owned, bounded diagnostic codes and terminal manifest evidence", async () => {
    const safe = {
      schemaVersion: 2,
      job: solveJob(operationId, "verificationAlarm"),
      elapsedMilliseconds: 2_500,
      firstIncumbentMilliseconds: 100,
      firstVerifiedFeasibleMilliseconds: null,
      phaseTimings: {
        compileMilliseconds: 20,
        backendMilliseconds: 2_000,
        projectionMilliseconds: null,
        structuralValidationMilliseconds: null,
        scoreRecomputationMilliseconds: null,
        requiredRuleVerificationMilliseconds: null,
        evidencePersistenceMilliseconds: 5,
        optionalExplanationMilliseconds: null,
      },
      diagnosticCode: "verify.correctness-failed",
      manifestChecksum: "b".repeat(64),
    } satisfies generated.SolveDiagnosticsSummaryDtoV2;
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, safe, 3)),
    );
    const retained = await generated.getSolveDiagnosticsSummary(scenarioId, solveRunId);
    expect(retained.result.firstIncumbentMilliseconds).toBe(100);
    expect(retained.result.firstVerifiedFeasibleMilliseconds).toBeNull();
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(
        response(input, { ...safe, diagnosticCode: "private roster note, not a code" }, 3),
      ),
    );
    await expect(
      generated.getSolveDiagnosticsSummary(scenarioId, solveRunId),
    ).rejects.toMatchObject(invalidResponse);
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(
        response(input, { ...safe, job: { ...safe.job, scenarioId: operationId } }, 3),
      ),
    );
    await expect(
      generated.getSolveDiagnosticsSummary(scenarioId, solveRunId),
    ).rejects.toMatchObject(invalidResponse);
  });

  it("uses the explicit solve cancellation command and release does not invoke generic cancellation", async () => {
    const terminal = deferred<unknown>();
    const started = deferred<Invocation>();
    const controls: string[] = [];
    tauri.invoke.mockImplementation((command: string, input: Invocation) => {
      switch (command) {
        case "operation_prepare":
          return Promise.resolve(response(input, { schemaVersion: 1, operationId }));
        case "solve_start":
          started.resolve(input);
          return terminal.promise;
        case "solve_cancel":
          controls.push(command);
          return Promise.resolve({
            ...response(input, {
              schemaVersion: 2,
              cancelRequestId: input.request.cancelRequestId,
              acknowledgement: "cancellationRequested",
            }),
            requestId: input.request.cancelRequestId,
          });
        case "operation_release":
          controls.push(command);
          return Promise.resolve(
            response(input, { schemaVersion: 1, acknowledgement: "released" }),
          );
        case "operation_cancel":
          controls.push(command);
          return Promise.reject(new Error("generic cancellation must not be used for solves"));
        default:
          throw new Error(`Unexpected native command: ${command}`);
      }
    });
    const scope = new generated.SetupOperationScope(scenarioId, 3);
    const operation = generated.startSolve(scope, "deep");
    expect("cancel" in operation).toBe(false);
    const startInput = await started.promise;
    await expect(
      generated.cancelSolve({ operationId, scenarioId, expectedRevision: 3 }),
    ).resolves.toMatchObject({
      result: { acknowledgement: "cancellationRequested" },
    });
    terminal.resolve(
      response(
        startInput,
        {
          schemaVersion: 2,
          currentRevision: 3,
          reused: false,
          job: solveJob(startInput.request.requestId, "cancelled"),
        },
        3,
      ),
    );
    await operation.result;
    await operation.release();
    expect(controls).toEqual(["solve_cancel", "operation_release"]);
  });
});

function csvInput(): {
  mapping: generated.PeopleCsvMapping;
  decisions: readonly generated.PeopleCsvDecision[];
} {
  const review = peopleCsvNative.preview.preview.review;
  return {
    mapping: {
      dialect: "comma",
      hasHeader: true,
      expectedColumns: 2,
      columns: [
        { index: 0, field: "externalId", blank: "preserve" },
        { index: 1, field: "name", blank: "preserve" },
      ],
      newPersonDefaults: {
        activeRange: { kind: "always" },
        qualificationGrants: [],
        eligibleAssignmentTypeIds: [],
        workloadWeight: { numerator: 1, denominator: 1 },
        tags: [],
        teamIds: [],
      },
      referenceMappings: {},
    },
    decisions: review.decisions.map(({ record, decision }) => ({
      record,
      decision: { kind: "add", personId: decision.personId },
    })),
  };
}

function csvApplyInput() {
  return {
    sourceId: peopleCsvNative.sourceOpened.sourceId,
    previewId: peopleCsvNative.preview.previewId,
    approvedDigest: peopleCsvNative.preview.preview.approvalDigest,
    commandId: peopleCsvNative.applied.outcome.commandId,
    actor: { actorId: null, displayName: "CSV native fixture" },
    truncateRedo: false,
  };
}

/** Captured native receipts cross the real generated guards and the pinned SDK Channel. */
function nativeCsv(
  override?: (command: string, input: Invocation) => Promise<unknown> | undefined,
) {
  let nextOperation = 0;
  const lifecycle: { command: string; input: Invocation }[] = [];
  tauri.invoke.mockImplementation((command: string, input: Invocation) => {
    if (command === "csv_source_close" || command === "csv_preview_discard")
      lifecycle.push({ command, input });
    const overridden = override?.(command, input);
    if (overridden !== undefined) return overridden;
    switch (command) {
      case "operation_prepare":
        return Promise.resolve(
          response(input, {
            schemaVersion: 1,
            operationId: `01900000-0000-7000-8000-${String(++nextOperation).padStart(12, "0")}`,
          }),
        );
      case "operation_release":
        return Promise.resolve(response(input, { schemaVersion: 1, acknowledgement: "released" }));
      case "operation_cancel":
        return Promise.resolve(
          response(input, { schemaVersion: 1, acknowledgement: "cancellationRequested" }),
        );
      case "csv_source_close":
      case "csv_preview_discard":
        return Promise.resolve(response(input, { schemaVersion: 1 }));
      case "csv_source_open":
        return Promise.resolve(response(input, peopleCsvNative.sourceOpened));
      case "csv_detect":
        return Promise.resolve(response(input, peopleCsvNative.detection));
      case "people_csv_preview":
        return Promise.resolve(
          response(input, peopleCsvNative.preview, peopleCsvNative.preview.revision),
        );
      case "csv_apply":
        return Promise.resolve(
          response(input, peopleCsvNative.applied, peopleCsvNative.applied.outcome.revision),
        );
      case "csv_report":
        return Promise.resolve(response(input, peopleCsvNative.rejectedRows));
      case "csv_report_save":
        return Promise.resolve(response(input, peopleCsvNative.reportSaved));
      default:
        throw new Error(`Unexpected native CSV command: ${command}`);
    }
  });
  return { lifecycle };
}

function csvContext() {
  const scenario = peopleCsvNative.preview.scenarioId;
  return {
    flow: new generated.CsvImportFlow(scenario, "people"),
    scope: new generated.SetupOperationScope(scenario, peopleCsvNative.preview.revision),
  };
}

function eligibilityInput(): Pick<generated.EligibilityCsvReview, "mapping" | "skippedRecords"> {
  const review = eligibilityCsvNative.preview.result.preview.review;
  return {
    mapping: { ...review.mapping, dialect: "comma" },
    skippedRecords: review.skippedRecords,
  };
}

function nativeEligibilityCsv(
  override?: (command: string, input: Invocation) => Promise<unknown> | undefined,
) {
  return nativeCsv((command, input) => {
    const overridden = override?.(command, input);
    if (overridden !== undefined) return overridden;
    const receipt =
      command === "csv_detect"
        ? eligibilityCsvNative.detection
        : command === "csv_record_sample"
          ? eligibilityCsvNative.sample
          : command === "eligibility_csv_preview"
            ? eligibilityCsvNative.preview
            : command === "csv_apply"
              ? eligibilityCsvNative.applied
              : command === "csv_report"
                ? eligibilityCsvNative.report
                : null;
    return receipt === null
      ? undefined
      : Promise.resolve(response(input, receipt.result, receipt.currentRevision));
  });
}

function shiftInput(): Pick<generated.ShiftCsvReview, "mapping" | "decisions"> {
  const review = shiftCsvNative.preview.result.preview.review as generated.ShiftCsvReview;
  return { mapping: review.mapping, decisions: review.decisions };
}

function nativeShiftCsv(
  override?: (command: string, input: Invocation) => Promise<unknown> | undefined,
) {
  return nativeCsv((command, input) => {
    const overridden = override?.(command, input);
    if (overridden !== undefined) return overridden;
    const receipt =
      command === "csv_detect"
        ? shiftCsvNative.detection
        : command === "csv_record_sample"
          ? shiftCsvNative.sample
          : command === "shift_csv_preview"
            ? shiftCsvNative.preview
            : command === "csv_apply"
              ? shiftCsvNative.applied
              : command === "csv_report"
                ? shiftCsvNative.report
                : null;
    return receipt === null
      ? undefined
      : Promise.resolve(response(input, receipt.result, receipt.currentRevision));
  });
}

function availabilityInput(
  review = availabilityCsvNative.preview.result.preview.review,
): Pick<generated.AvailabilityCsvReview, "mapping" | "decisions"> {
  const typed = review as generated.AvailabilityCsvReview;
  return { mapping: typed.mapping, decisions: typed.decisions };
}

function nativeAvailabilityCsv(
  override?: (command: string, input: Invocation) => Promise<unknown> | undefined,
) {
  return nativeCsv((command, input) => {
    const overridden = override?.(command, input);
    if (overridden !== undefined) return overridden;
    const receipt =
      command === "csv_detect"
        ? availabilityCsvNative.detection
        : command === "csv_record_sample"
          ? availabilityCsvNative.sample
          : command === "availability_csv_preview"
            ? availabilityCsvNative.preview
            : command === "csv_apply"
              ? availabilityCsvNative.apply
              : command === "csv_report"
                ? availabilityCsvNative.report
                : null;
    return receipt === null
      ? undefined
      : Promise.resolve(response(input, receipt.result, receipt.currentRevision));
  });
}

function matrixInput(
  review = eligibilityMatrixNative.preview.result.preview.review,
): Pick<generated.EligibilityMatrixReview, "mapping" | "skippedCells"> {
  return { mapping: review.mapping, skippedCells: review.skippedCells };
}

function nativeEligibilityMatrix(
  override?: (command: string, input: Invocation) => Promise<unknown> | undefined,
  noChanges = false,
) {
  return nativeCsv((command, input) => {
    const overridden = override?.(command, input);
    if (overridden !== undefined) return overridden;
    const receipt =
      command === "eligibility_matrix_source_open"
        ? noChanges
          ? eligibilityMatrixNative.noChangesSourceOpened
          : eligibilityMatrixNative.sourceOpened
        : command === "csv_detect"
          ? noChanges
            ? eligibilityMatrixNative.noChangesDetection
            : eligibilityMatrixNative.detection
          : command === "csv_record_sample"
            ? input.request.record === 5
              ? eligibilityMatrixNative.inertSample
              : eligibilityMatrixNative.sample
            : command === "eligibility_matrix_preview"
              ? noChanges
                ? eligibilityMatrixNative.noChangesPreview
                : eligibilityMatrixNative.preview
              : command === "csv_apply"
                ? noChanges
                  ? eligibilityMatrixNative.noChangesApply
                  : eligibilityMatrixNative.apply
                : command === "csv_report"
                  ? eligibilityMatrixNative.report
                  : null;
    return receipt === null
      ? undefined
      : Promise.resolve(response(input, receipt.result, receipt.currentRevision));
  });
}

describe("native pasted eligibility matrix boundary", () => {
  it("admits native cells, independent aliases and grouped raw-preserving proposals through apply and report", async () => {
    const captured = eligibilityMatrixNative.preview.result;
    const native = nativeEligibilityMatrix((command, input) =>
      command === "eligibility_matrix_preview" &&
      (input.request.skippedCells as readonly unknown[]).length === 0
        ? Promise.resolve(
            response(input, eligibilityMatrixNative.unresolvedPreview.result, captured.revision),
          )
        : undefined,
    );
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibilityMatrix");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    const opened = (
      await flow.openEligibilityMatrix(scope, eligibilityMatrixNative.submittedText).result
    ).result;
    expect(opened.byteCount).toBe(
      new TextEncoder().encode(eligibilityMatrixNative.submittedText).length,
    );
    const detection = (await flow.detect(scope, opened.sourceId).result).result;
    if (detection.family !== "eligibilityMatrix") throw new Error("Expected matrix census");
    expect(detection.data).toMatchObject({
      columns: 4,
      cells: 12,
      source: { rawBytes: opened.byteCount, logicalRecords: 5 },
    });
    const selected = { readFormat: { family: "eligibilityMatrix" }, record: 4 } as const;
    const sample = (await flow.sample(scope, opened.sourceId, selected).result).result;
    expect(sample.readFormat).toEqual({ family: "eligibilityMatrix" });
    expect(sample.cells?.map(({ text }) => text)).toEqual([
      "018f7b40-a000-7000-8000-00000000000d",
      "false",
      "false",
      "TRUE",
    ]);
    const inert = (
      await flow.sample(scope, opened.sourceId, {
        readFormat: { family: "eligibilityMatrix" },
        record: 5,
      }).result
    ).result;
    expect(inert.cells?.[0]).toEqual({ text: "<img src=x>", truncated: false });
    const unresolved = (
      await flow.previewEligibilityMatrix(scope, opened.sourceId, {
        mapping: { personMappings: {}, assignmentTypeMappings: {} },
        skippedCells: [],
      }).result
    ).result;
    expect(unresolved.preview).toMatchObject({
      disposition: "blocked",
      batch: null,
      review: null,
      approvalDigest: null,
    });
    expect(unresolved.preview.rows[1]).toMatchObject({
      cell: { record: 2, column: 2 },
      status: "blocked",
      issue: "unresolvedPerson",
    });
    await flow.discardPreview(unresolved.previewId);
    const reviewed = (
      await flow.previewEligibilityMatrix(scope, opened.sourceId, matrixInput()).result
    ).result;
    expect(
      reviewed.preview.rows
        .slice(0, 3)
        .map(({ cell, before, after, status }) => [cell, before, after, status]),
    ).toEqual([
      [{ record: 2, column: 1 }, false, true, "changed"],
      [{ record: 2, column: 2 }, true, true, "unchanged"],
      [{ record: 2, column: 3 }, false, true, "changed"],
    ]);
    expect(reviewed.preview.rows[0]).toMatchObject({
      personId: "018f7b40-a000-7000-8000-000000000001",
      assignmentTypeId: "018f7b40-a000-7000-8000-000000000015",
    });
    expect(reviewed.preview.batch?.commands.map(({ payload }) => payload.entity)).toMatchObject([
      {
        id: "018f7b40-a000-7000-8000-000000000001",
        eligibleAssignmentTypeIds: [
          "018F7B40-A000-7000-8000-000000000004",
          "018f7b40-a000-7000-8000-000000000015",
          "018f7b40-a000-7000-8000-000000000017",
        ],
        homeLocationId: "018f7b40a00070008000000000000005",
        tags: ["night", "reviewed"],
        workloadWeight: { numerator: 3, denominator: 2 },
      },
      { id: "018f7b40-a000-7000-8000-00000000000d", eligibleAssignmentTypeIds: [] },
    ]);
    if (reviewed.preview.approvalDigest === null) throw new Error("Missing matrix approval");
    const applied = (
      await flow.apply(scope, {
        sourceId: opened.sourceId,
        previewId: reviewed.previewId,
        approvedDigest: reviewed.preview.approvalDigest,
        commandId: eligibilityMatrixNative.apply.result.outcome.commandId,
        actor: { actorId: null, displayName: "Matrix boundary fixture" },
        truncateRedo: false,
      }).result
    ).result;
    expect(applied.outcome).toMatchObject({ kind: "applied", revision: 3 });
    scope.dispose();
    const report = (await flow.report(reviewed.previewId)).result;
    expect(report.consumed).toBe(true);
    expect(report.rows).toEqual({
      family: "eligibilityMatrix",
      excludedCells: [
        { cell: { record: 3, column: 1 }, skipped: true, issue: "duplicatePair" },
        { cell: { record: 3, column: 2 }, skipped: true, issue: "duplicatePair" },
        { cell: { record: 3, column: 3 }, skipped: true, issue: "duplicatePair" },
        { cell: { record: 4, column: 3 }, skipped: true, issue: "invalidMembership" },
        { cell: { record: 5, column: 1 }, skipped: true, issue: "unresolvedPerson" },
        { cell: { record: 5, column: 2 }, skipped: true, issue: "unresolvedPerson" },
        { cell: { record: 5, column: 3 }, skipped: true, issue: "unresolvedPerson" },
      ],
    });
    await flow.dispose();
    expect(native.lifecycle.filter(({ command }) => command === "csv_source_close")).toEqual([]);
    expect(callbacks.size).toBe(0);
  });

  it("consumes a same-text native no-op after apply/Undo/Redo without advancing revision", async () => {
    const native = nativeEligibilityMatrix(undefined, true);
    const captured = eligibilityMatrixNative.noChangesPreview.result;
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibilityMatrix");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    expect(eligibilityMatrixNative.noChangesText).toBe(eligibilityMatrixNative.submittedText);
    const opened = (
      await flow.openEligibilityMatrix(scope, eligibilityMatrixNative.noChangesText).result
    ).result;
    const detection = (await flow.detect(scope, opened.sourceId).result).result;
    if (detection.family !== "eligibilityMatrix") throw new Error("Expected matrix census");
    expect(detection.data.cells).toBe(12);
    const reviewed = (
      await flow.previewEligibilityMatrix(
        scope,
        opened.sourceId,
        matrixInput(captured.preview.review),
      ).result
    ).result;
    expect(reviewed.preview).toMatchObject({ disposition: "noChanges", batch: null, cells: 12 });
    expect(
      reviewed.preview.rows
        .filter(({ status }) => status !== "skipped")
        .map(({ before, after }) => [before, after]),
    ).toEqual([
      [true, true],
      [true, true],
      [true, true],
      [false, false],
      [false, false],
    ]);
    if (reviewed.preview.approvalDigest === null) throw new Error("Missing no-op approval");
    const applied = (
      await flow.apply(scope, {
        sourceId: opened.sourceId,
        previewId: reviewed.previewId,
        approvedDigest: reviewed.preview.approvalDigest,
        commandId: generated.newUuidV7(),
        actor: { actorId: null, displayName: "Matrix no-op fixture" },
        truncateRedo: false,
      }).result
    ).result;
    expect(applied.outcome).toEqual({ kind: "noChanges", revision: 5 });
    expect(applied.report.consumed).toBe(true);
    scope.dispose();
    await flow.dispose();
    expect(native.lifecycle.filter(({ command }) => command === "csv_source_close")).toEqual([]);
  });

  it("rejects over-byte and ill-formed pasted intent and invalid cell decisions before native acquisition", async () => {
    const captured = eligibilityMatrixNative.preview.result;
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibilityMatrix");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    expect(() => flow.open(scope)).toThrow(RangeError);
    for (const text of [
      "a".repeat(generated.MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES + 1),
      "😀".repeat(generated.MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES / 4) + "a",
      "\ud800",
    ]) {
      expect(() => flow.openEligibilityMatrix(scope, text)).toThrow(RangeError);
    }
    for (const skippedCells of [
      [{ record: 1, column: 1 }],
      [{ record: 2, column: 0 }],
      [{ record: 2, column: 64 }],
      [
        { record: 3, column: 1 },
        { record: 2, column: 1 },
      ],
      [
        { record: 2, column: 1 },
        { record: 2, column: 1 },
      ],
    ]) {
      expect(() =>
        flow.previewEligibilityMatrix(scope, captured.sourceId, {
          ...matrixInput(),
          skippedCells,
        }),
      ).toThrow(RangeError);
    }
    expect(tauri.invoke).not.toHaveBeenCalled();
    scope.dispose();
    await flow.dispose();
  });

  it("refuses a native matrix detection with a false rectangle, sample width or source bound", async () => {
    const captured = eligibilityMatrixNative.preview.result;
    const original = eligibilityMatrixNative.detection.result;
    let detection: unknown = original;
    nativeEligibilityMatrix((command, input) =>
      command === "csv_detect" ? Promise.resolve(response(input, detection)) : undefined,
    );
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibilityMatrix");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    for (const data of [
      { ...original.data, cells: 11 },
      { ...original.data, samples: original.data.samples.slice(1) },
      {
        ...original.data,
        samples: original.data.samples.map((sample) => ({
          ...sample,
          cells: sample.cells.slice(1),
        })),
      },
      {
        ...original.data,
        source: {
          ...original.data.source,
          rawBytes: generated.MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES + 1,
        },
      },
    ]) {
      detection = { ...original, data };
      await expect(flow.detect(scope, captured.sourceId).result).rejects.toMatchObject(
        invalidResponse,
      );
    }
    scope.dispose();
    await flow.dispose();
  });

  it("correlates no-dialect native samples by family, source and logical record", async () => {
    const captured = eligibilityMatrixNative.preview.result;
    const original = eligibilityMatrixNative.sample.result;
    let sample: unknown = original;
    nativeEligibilityMatrix((command, input) =>
      command === "csv_record_sample" ? Promise.resolve(response(input, sample)) : undefined,
    );
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibilityMatrix");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    const selected = { readFormat: { family: "eligibilityMatrix" }, record: 4 } as const;
    expect(
      (await flow.sample(scope, captured.sourceId, selected).result).result.cells?.[3]?.text,
    ).toBe("TRUE");
    for (const malformed of [
      { ...original, readFormat: { family: "eligibility", dialect: "tab" } },
      { ...original, readFormat: { family: "eligibilityMatrix", dialect: "tab" } },
      { ...original, sourceId: generated.newUuidV7() },
      { ...original, record: 5 },
    ]) {
      sample = malformed;
      await expect(flow.sample(scope, captured.sourceId, selected).result).rejects.toMatchObject(
        invalidResponse,
      );
    }
    scope.dispose();
    await flow.dispose();
  });

  it("refuses tampered matrix cell census, coordinates and source/review bindings instead of admitting approval", async () => {
    const captured = eligibilityMatrixNative.preview.result;
    const original = captured.preview;
    let preview: unknown = captured;
    nativeEligibilityMatrix((command, input) =>
      command === "eligibility_matrix_preview"
        ? Promise.resolve(response(input, preview, captured.revision))
        : undefined,
    );
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibilityMatrix");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    for (const malformed of [
      { ...captured, sourceId: generated.newUuidV7() },
      { ...captured, scenarioId: generated.newUuidV7() },
      { ...captured, preview: { ...original, rows: original.rows.slice(1) } },
      {
        ...captured,
        preview: {
          ...original,
          rows: original.rows.map((row, index) =>
            index === 0 ? { ...row, cell: { record: 1, column: 1 } } : row,
          ),
        },
      },
      {
        ...captured,
        preview: {
          ...original,
          rows: original.rows.map((row, index) =>
            index === 0 ? { ...row, cell: { record: 2, column: 0 } } : row,
          ),
        },
      },
      {
        ...captured,
        preview: {
          ...original,
          excludedCells: [
            ...original.excludedCells,
            { cell: { record: 6, column: 1 }, skipped: true, issue: null },
          ],
        },
      },
      ...[
        { ...original.review, parserVersion: "unknown-parser" },
        { ...original.review, revision: captured.revision + 1 },
        { ...original.review, cells: 11 },
        { ...original.review, source: { ...original.review.source, rawBytes: 220 } },
        { ...original.review, source: { ...original.review.source, blake3: "0".repeat(64) } },
        { ...original.review, skippedCells: [{ record: 5, column: 4 }] },
      ].map((review) => ({ ...captured, preview: { ...original, review } })),
    ]) {
      preview = malformed;
      await expect(
        flow.previewEligibilityMatrix(scope, captured.sourceId, matrixInput()).result,
      ).rejects.toMatchObject(invalidResponse);
    }
    scope.dispose();
    await flow.dispose();
  });

  it("refuses wrong-family or unsorted matrix reports and unconsumed apply receipts", async () => {
    const captured = eligibilityMatrixNative.preview.result;
    const original = eligibilityMatrixNative.report.result;
    let report: unknown = original;
    let applied: unknown = eligibilityMatrixNative.apply.result;
    nativeEligibilityMatrix((command, input) =>
      command === "csv_report"
        ? Promise.resolve(response(input, report))
        : command === "csv_apply"
          ? Promise.resolve(response(input, applied, 3))
          : undefined,
    );
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibilityMatrix");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    for (const rows of [
      { family: "eligibility", excludedRows: [] },
      { ...original.rows, excludedCells: [...original.rows.excludedCells].reverse() },
    ]) {
      report = { ...original, rows };
      applied = { ...eligibilityMatrixNative.apply.result, report };
      await expect(flow.report(captured.previewId)).rejects.toMatchObject(invalidResponse);
      await expect(
        flow.apply(scope, {
          sourceId: captured.sourceId,
          previewId: captured.previewId,
          approvedDigest: captured.preview.approvalDigest,
          commandId: eligibilityMatrixNative.apply.result.outcome.commandId,
          actor: { actorId: null, displayName: "Matrix boundary fixture" },
          truncateRedo: false,
        }).result,
      ).rejects.toMatchObject(invalidResponse);
    }
    applied = { ...eligibilityMatrixNative.apply.result, report: { ...original, consumed: false } };
    await expect(
      flow.apply(scope, {
        sourceId: captured.sourceId,
        previewId: captured.previewId,
        approvedDigest: captured.preview.approvalDigest,
        commandId: eligibilityMatrixNative.apply.result.outcome.commandId,
        actor: { actorId: null, displayName: "Matrix boundary fixture" },
        truncateRedo: false,
      }).result,
    ).rejects.toMatchObject(invalidResponse);
    scope.dispose();
    await flow.dispose();
  });

  it("retires late pasted-source custody by creator when the flow is disposed before capture settles", async () => {
    const captured = eligibilityMatrixNative.preview.result;
    const entered = deferred<Invocation>();
    const terminal = deferred<unknown>();
    const firstCleanup = deferred<Invocation>();
    const native = nativeEligibilityMatrix((command, input) => {
      if (command === "eligibility_matrix_source_open") {
        entered.resolve(input);
        return terminal.promise;
      }
      if (command === "csv_source_close") firstCleanup.resolve(input);
      return undefined;
    });
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibilityMatrix");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    const opened = flow.openEligibilityMatrix(scope, eligibilityMatrixNative.submittedText);
    const invocation = await entered.promise;
    const disposal = flow.dispose();
    const cleanup = await firstCleanup.promise;
    const target = {
      kind: "creator",
      operationId: await opened.operationId,
      requestId: opened.requestId,
    };
    expect(cleanup.request.target).toEqual(target);
    expect(opened.isCurrent()).toBe(false);
    const beforeSettlement = native.lifecycle.length;
    terminal.resolve(response(invocation, eligibilityMatrixNative.sourceOpened.result));
    await opened.result;
    await disposal;
    expect(native.lifecycle.slice(beforeSettlement)).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          command: "csv_source_close",
          input: expect.objectContaining({ request: expect.objectContaining({ target }) }),
        }),
      ]),
    );
    expect(callbacks.size).toBe(0);
    scope.dispose();
  });
});

describe("native CSV receipt and custody boundary", () => {
  it("admits real availability receipts with raw references, weekly windows and signed endpoint facts", async () => {
    nativeAvailabilityCsv();
    const captured = availabilityCsvNative.preview.result;
    const flow = new generated.CsvImportFlow(captured.scenarioId, "availability");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    expect((await flow.detect(scope, captured.sourceId).result).result.family).toBe("availability");
    const sample = await flow.sample(scope, captured.sourceId, {
      readFormat: { family: "availability", dialect: "tab" },
      record: 2,
    }).result;
    expect(sample.result.cells?.[3]?.text).toBe("2026-09-01T09:00:00.123");
    const reviewed = (
      await flow.previewAvailability(scope, captured.sourceId, availabilityInput()).result
    ).result;
    expect(reviewed.preview.rows.map(({ status }) => status)).toEqual([
      "updated",
      "added",
      "unchanged",
    ]);
    const updated = reviewed.preview.rows[0];
    expect(updated?.after?.personId).toBe("018F7B40-A000-7000-8000-000000000001");
    expect(updated?.after?.assignmentTypeIds).toEqual(["018F7B40-A000-7000-8000-000000000004"]);
    expect(updated?.after?.locationIds).toEqual(["018f7b40a00070008000000000000005"]);
    expect(updated?.after?.timeWindow).toEqual({
      kind: "instant",
      startsAt: "2026-09-01T09:00:00.123-04:00",
      endsAt: "2026-09-01T10:00:00.456-04:00",
    });
    expect(updated?.resolvedStartsAt).toEqual({
      instant: "2026-09-01T13:00:00.123Z",
      local: "2026-09-01T09:00:00.123",
      offsetSeconds: -14400,
    });
    expect(reviewed.preview.rows[1]?.after).toMatchObject({
      availabilityKind: "requestedTimeOff",
      source: " weekly source ",
      note: " weekly note ",
      timeWindow: { kind: "weekly" },
    });
    expect(reviewed.preview.rows[2]?.resolvedEndsAt?.offsetSeconds).toBe(-14400);
    const applied = (
      await flow.apply(scope, {
        sourceId: captured.sourceId,
        previewId: captured.previewId,
        approvedDigest: captured.preview.approvalDigest,
        commandId: availabilityCsvNative.apply.result.outcome.commandId,
        actor: { actorId: null, displayName: "Availability boundary fixture" },
        truncateRedo: false,
      }).result
    ).result;
    expect(applied.outcome).toMatchObject({ kind: "applied", revision: 2 });
    expect((await flow.report(captured.previewId)).result.rows).toEqual({
      family: "availability",
      excludedRows: [],
    });
    scope.dispose();
    await flow.dispose();
  });

  it("retains native equivalent-time facts while consuming a no-change approval at the same revision", async () => {
    nativeAvailabilityCsv((command, input) => {
      const receipt =
        command === "availability_csv_preview"
          ? availabilityCsvNative.noChangesPreview
          : command === "csv_apply"
            ? availabilityCsvNative.noChangesApply
            : null;
      return receipt === null
        ? undefined
        : Promise.resolve(response(input, receipt.result, receipt.currentRevision));
    });
    const captured = availabilityCsvNative.noChangesPreview.result;
    const flow = new generated.CsvImportFlow(captured.scenarioId, "availability");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    const reviewed = (
      await flow.previewAvailability(
        scope,
        captured.sourceId,
        availabilityInput(captured.preview.review),
      ).result
    ).result;
    expect(reviewed.preview.disposition).toBe("noChanges");
    expect(reviewed.preview.rows[0]?.resolvedStartsAt).toEqual({
      instant: "2026-09-01T13:00:00.123Z",
      local: "2026-09-01T09:00:00.123",
      offsetSeconds: -14400,
    });
    const applied = (
      await flow.apply(scope, {
        sourceId: captured.sourceId,
        previewId: captured.previewId,
        approvedDigest: captured.preview.approvalDigest,
        commandId: generated.newUuidV7(),
        actor: { actorId: null, displayName: "Availability no-change fixture" },
        truncateRedo: false,
      }).result
    ).result;
    expect(applied.outcome).toEqual({ kind: "noChanges", revision: 2 });
    expect(applied.report.consumed).toBe(true);
    scope.dispose();
    await flow.dispose();
  });

  it("refuses malformed availability endpoint facts instead of creating approval", async () => {
    const captured = availabilityCsvNative.preview.result;
    const first = captured.preview.rows[0];
    if (!first?.resolvedStartsAt) throw new Error("Expected native endpoint facts");
    const malformed = {
      ...captured,
      preview: {
        ...captured.preview,
        rows: [
          {
            ...first,
            resolvedStartsAt: { ...first.resolvedStartsAt, offsetSeconds: "-14400" },
          },
          ...captured.preview.rows.slice(1),
        ],
      },
    };
    nativeAvailabilityCsv((command, input) =>
      command === "availability_csv_preview"
        ? Promise.resolve(response(input, malformed, captured.revision))
        : undefined,
    );
    const flow = new generated.CsvImportFlow(captured.scenarioId, "availability");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    await expect(
      flow.previewAvailability(scope, captured.sourceId, availabilityInput()).result,
    ).rejects.toMatchObject(invalidResponse);
    scope.dispose();
    await flow.dispose();
  });

  it("accepts captured picker, detection, preview, apply and retained report receipts across revision scopes", async () => {
    const native = nativeCsv();
    const { flow, scope } = csvContext();
    const opened = await flow.open(scope).result;
    const detected = await flow.detect(scope, opened.result.sourceId).result;
    expect(detected.result).toEqual(peopleCsvNative.detection);
    expect(opened.currentRevision).toBeNull();
    expect(detected.currentRevision).toBeNull();
    const preview = await flow.previewPeople(scope, opened.result.sourceId, csvInput()).result;
    expect(preview.result.preview.rows).toEqual(peopleCsvNative.preview.preview.rows);
    const applied = await flow.apply(scope, csvApplyInput()).result;
    expect(applied.result.outcome).toEqual(peopleCsvNative.applied.outcome);
    scope.dispose();
    const current = new generated.SetupOperationScope(peopleCsvNative.preview.scenarioId, 7);
    const retained = await flow.report(preview.result.previewId);
    const saved = await flow.saveReport(current, preview.result.previewId).result;
    expect(retained.result.rows).toEqual({
      family: "people",
      rejectedRows: [{ record: 3, code: "missingName" }],
    });
    expect(retained.currentRevision).toBeNull();
    expect(saved.result.previewId).toBe(preview.result.previewId);
    expect(saved.currentRevision).toBeNull();
    expect(native.lifecycle).toEqual([]);
    await flow.dispose();
    expect(callbacks.size).toBe(0);
  });

  it("admits real shift receipts with negative offsets and preserves raw references and local intent", async () => {
    nativeShiftCsv();
    const captured = shiftCsvNative.preview.result;
    const flow = new generated.CsvImportFlow(captured.scenarioId, "shifts");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    expect((await flow.detect(scope, captured.sourceId).result).result.family).toBe("shifts");
    const sample = await flow.sample(scope, captured.sourceId, {
      readFormat: { family: "shifts", dialect: "tab" },
      record: 2,
    }).result;
    expect(sample.result.cells?.[3]?.text).toBe("2026-09-01T09:00:00");
    const reviewed = (await flow.previewShifts(scope, captured.sourceId, shiftInput()).result)
      .result;
    expect(reviewed.preview.rows.map(({ status }) => status)).toEqual(["updated", "added"]);
    const updated = reviewed.preview.rows[0];
    expect(updated?.before?.tags).toEqual(["authored", "order"]);
    expect(updated?.after?.tags).toEqual(["changed"]);
    expect(updated?.after?.locationId).toBe("018f7b40a00070008000000000000005");
    expect(updated?.after?.startsAt).toEqual({
      local: "2026-09-01T09:00:00.000",
      instant: "2026-09-01T09:00:00-04:00",
      offsetSeconds: -14400,
    });
    const added = reviewed.preview.rows[1];
    expect(added?.identity).toEqual({ kind: "supplied", shiftId: added?.shiftId });
    expect(added?.after?.id).toBe("018f7b40-a000-7000-8000-000000000009");
    expect(added?.after?.startsAt.offsetSeconds).toBe(-14400);
    const applied = (
      await flow.apply(scope, {
        sourceId: captured.sourceId,
        previewId: captured.previewId,
        approvedDigest: captured.preview.approvalDigest,
        commandId: shiftCsvNative.applied.result.outcome.commandId,
        actor: { actorId: null, displayName: "Shift boundary fixture" },
        truncateRedo: false,
      }).result
    ).result;
    expect(applied.outcome).toMatchObject({ kind: "applied", revision: 2 });
    expect((await flow.report(captured.previewId)).result.rows).toEqual({
      family: "shifts",
      excludedRows: [],
    });
    scope.dispose();
    await flow.dispose();
  });

  it("refuses a shift preview with a nonnumeric resolved offset rather than creating approval", async () => {
    const captured = shiftCsvNative.preview.result;
    const first = captured.preview.rows[0];
    if (!first?.after) throw new Error("Expected native shift projection");
    const malformed = {
      ...captured,
      preview: {
        ...captured.preview,
        rows: [
          {
            ...first,
            after: {
              ...first.after,
              startsAt: { ...first.after.startsAt, offsetSeconds: "-14400" },
            },
          },
          ...captured.preview.rows.slice(1),
        ],
      },
    };
    nativeShiftCsv((command, input) =>
      command === "shift_csv_preview"
        ? Promise.resolve(response(input, malformed, captured.revision))
        : undefined,
    );
    const flow = new generated.CsvImportFlow(captured.scenarioId, "shifts");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    await expect(
      flow.previewShifts(scope, captured.sourceId, shiftInput()).result,
    ).rejects.toMatchObject(invalidResponse);
    scope.dispose();
    await flow.dispose();
  });

  it("admits native sparse eligibility review and grouped update receipts without changing raw Person meaning", async () => {
    nativeEligibilityCsv();
    const captured = eligibilityCsvNative.preview.result;
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibility");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    expect((await flow.detect(scope, captured.sourceId).result).result.family).toBe("eligibility");
    const sample = await flow.sample(scope, captured.sourceId, {
      readFormat: { family: "eligibility", dialect: "comma" },
      record: 2,
    }).result;
    expect(sample.result.cells?.map((cell) => cell.text)).toEqual(
      eligibilityCsvNative.sample.result.cells.map((cell) => cell.text),
    );
    const reviewed = (
      await flow.previewEligibility(scope, captured.sourceId, eligibilityInput()).result
    ).result;
    expect(reviewed.preview.rows.map(({ before, after }) => [before, after])).toEqual([
      [false, true],
      [false, true],
      [null, true],
    ]);
    expect(reviewed.preview.batch?.commands).toEqual(captured.preview.batch.commands);
    expect(reviewed.preview.batch?.commands[0]?.payload.entity.homeLocationId).toBe(
      "018f7b40a00070008000000000000005",
    );
    const applied = (
      await flow.apply(scope, {
        sourceId: captured.sourceId,
        previewId: captured.previewId,
        approvedDigest: captured.preview.approvalDigest,
        commandId: eligibilityCsvNative.applied.result.outcome.commandId,
        actor: { actorId: null, displayName: "Eligibility boundary fixture" },
        truncateRedo: false,
      }).result
    ).result;
    expect(applied.outcome).toEqual(eligibilityCsvNative.applied.result.outcome);
    expect((await flow.report(captured.previewId)).result.rows).toEqual({
      family: "eligibility",
      excludedRows: [{ record: 4, skipped: true, issue: "unresolvedPerson" }],
    });
    scope.dispose();
    await flow.dispose();
  });

  it("keeps canonical root and source identities separate from preserved raw references", async () => {
    const captured = eligibilityCsvNative.preview.result;
    const preview = structuredClone(captured);
    const entity = preview.preview.batch.commands[0]?.payload.entity;
    if (!entity) throw new Error("Expected native Person proposal");
    nativeEligibilityCsv((command, input) =>
      command === "eligibility_csv_preview"
        ? Promise.resolve(response(input, preview, captured.revision))
        : undefined,
    );
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibility");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    const canonical = entity.id;
    entity.id = canonical.toUpperCase();
    await expect(
      flow.previewEligibility(scope, captured.sourceId, eligibilityInput()).result,
    ).rejects.toMatchObject(invalidResponse);
    entity.id = canonical;
    preview.sourceId = preview.sourceId.toUpperCase();
    await expect(
      flow.previewEligibility(scope, captured.sourceId, eligibilityInput()).result,
    ).rejects.toMatchObject(invalidResponse);
    scope.dispose();
    await flow.dispose();
  });

  it("accepts equivalent mapping UUID spellings but rejects either alias map above its item ceiling", async () => {
    nativeEligibilityCsv();
    const captured = eligibilityCsvNative.preview.result;
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibility");
    const scope = new generated.SetupOperationScope(captured.scenarioId, captured.revision);
    const person = captured.preview.rows[0]?.personId;
    const assignmentType = captured.preview.rows[0]?.assignmentTypeId;
    if (!person || !assignmentType) throw new Error("Expected resolved native fixture endpoints");
    for (const spell of [
      (value: string) => value.toUpperCase(),
      (value: string) => value.replaceAll("-", ""),
      (value: string) => `{${value}}`,
      (value: string) => `urn:uuid:${value}`,
    ]) {
      const input = eligibilityInput();
      await flow.previewEligibility(scope, captured.sourceId, {
        ...input,
        mapping: {
          ...input.mapping,
          personMappings: { Staff: spell(person) },
          assignmentTypeMappings: { Duty: spell(assignmentType) },
        },
      }).result;
      await flow.discardPreview(captured.previewId);
    }
    for (const field of ["personMappings", "assignmentTypeMappings"] as const) {
      const input = eligibilityInput();
      const mapping = {
        ...input.mapping,
        [field]: Object.fromEntries(
          Array.from({ length: 10_001 }, (_, index) => [String(index), person]),
        ),
      };
      expect(new TextEncoder().encode(JSON.stringify(mapping)).length).toBeLessThan(1_048_576);
      expect(() =>
        flow.previewEligibility(scope, captured.sourceId, { ...input, mapping }),
      ).toThrow(RangeError);
    }
    scope.dispose();
    await flow.dispose();
  });

  it("keeps full eligibility exclusions separate from People limits and refuses wrong family or excess rows", async () => {
    const captured = eligibilityCsvNative.preview.result;
    const excludedRows = Array.from({ length: 10_000 }, (_, index) => ({
      record: index + 2,
      skipped: true,
      issue: "unresolvedPerson",
    }));
    let report: unknown = {
      ...eligibilityCsvNative.report.result,
      rows: { family: "eligibility", excludedRows },
    };
    nativeEligibilityCsv((command, input) =>
      command === "csv_report" ? Promise.resolve(response(input, report)) : undefined,
    );
    const flow = new generated.CsvImportFlow(captured.scenarioId, "eligibility");
    const rows = (await flow.report(captured.previewId)).result.rows;
    if (rows.family !== "eligibility") throw new Error("Expected eligibility exclusions");
    expect(rows.excludedRows.at(-1)).toEqual({
      record: 10_001,
      skipped: true,
      issue: "unresolvedPerson",
    });
    excludedRows.push({ record: 2, skipped: true, issue: "unresolvedPerson" });
    await expect(flow.report(captured.previewId)).rejects.toMatchObject(invalidResponse);
    report = {
      ...eligibilityCsvNative.report.result,
      rows: { family: "people", rejectedRows: [] },
    };
    await expect(flow.report(captured.previewId)).rejects.toMatchObject(invalidResponse);
    await flow.dispose();
  });

  it("rejects malformed native versions, source bounds and response bindings", async () => {
    const { flow, scope } = csvContext();
    let opened: unknown = { ...peopleCsvNative.sourceOpened, schemaVersion: 2 };
    let preview: unknown = peopleCsvNative.preview;
    nativeCsv((command, input) => {
      if (command === "csv_source_open") return Promise.resolve(response(input, opened));
      if (command === "people_csv_preview") return Promise.resolve(response(input, preview, 0));
      return undefined;
    });
    await expect(flow.open(scope).result).rejects.toMatchObject(invalidResponse);
    opened = { ...peopleCsvNative.sourceOpened, byteCount: 16 * 1024 * 1024 + 1 };
    await expect(flow.open(scope).result).rejects.toMatchObject(invalidResponse);
    preview = { ...peopleCsvNative.preview, sourceId: scenarioId };
    await expect(
      flow.previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, csvInput()).result,
    ).rejects.toMatchObject(invalidResponse);
    preview = {
      ...peopleCsvNative.preview,
      preview: {
        ...peopleCsvNative.preview.preview,
        review: { ...peopleCsvNative.preview.preview.review, revision: 1 },
      },
    };
    await expect(
      flow.previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, csvInput()).result,
    ).rejects.toMatchObject(invalidResponse);
    await flow.dispose();
  });

  it("refuses another family's detection, retained report and apply receipt", async () => {
    const { flow, scope } = csvContext();
    nativeCsv((command, input) => {
      if (command === "csv_detect")
        return Promise.resolve(
          response(input, {
            family: "assignments",
            data: {
              format: "mapped",
              ...peopleCsvNative.detection.data,
            },
          }),
        );
      const report = {
        ...peopleCsvNative.rejectedRows,
        rows: { family: "assignments", excludedRows: [] },
      };
      if (command === "csv_report") return Promise.resolve(response(input, report));
      if (command === "csv_apply")
        return Promise.resolve(
          response(
            input,
            {
              ...peopleCsvNative.applied,
              report: { ...peopleCsvNative.applied.report, rows: report.rows },
            },
            peopleCsvNative.applied.outcome.revision,
          ),
        );
      return undefined;
    });
    await expect(
      flow.detect(scope, peopleCsvNative.sourceOpened.sourceId).result,
    ).rejects.toMatchObject(invalidResponse);
    await expect(flow.report(peopleCsvNative.preview.previewId)).rejects.toMatchObject(
      invalidResponse,
    );
    await expect(flow.apply(scope, csvApplyInput()).result).rejects.toMatchObject(invalidResponse);
    await flow.dispose();
  });

  it("admits the complete assignment report through read and apply, but refuses an extra row", async () => {
    const excludedRows = Array.from({ length: 10_000 }, (_, index) => ({
      record: index + 3,
      skipped: true,
      issue: null,
    }));
    const report = {
      ...assignmentCsvNative.report,
      rows: { family: "assignments", excludedRows },
    };
    nativeCsv((command, input) => {
      if (command === "csv_report") return Promise.resolve(response(input, report));
      if (command === "csv_apply")
        return Promise.resolve(
          response(
            input,
            {
              ...assignmentCsvNative.applied,
              report,
            },
            assignmentCsvNative.applied.outcome.revision,
          ),
        );
      return undefined;
    });
    const flow = new generated.CsvImportFlow(assignmentCsvNative.preview.scenarioId, "assignments");
    const scope = new generated.SetupOperationScope(
      assignmentCsvNative.preview.scenarioId,
      assignmentCsvNative.preview.revision,
    );
    const read = (await flow.report(assignmentCsvNative.preview.previewId)).result.rows;
    if (read.family !== "assignments") throw new Error("Expected assignment exclusions");
    expect(read.excludedRows).toHaveLength(10_000);
    expect(read.excludedRows.at(-1)?.record).toBe(10_002);
    const applied = (
      await flow.apply(scope, {
        sourceId: assignmentCsvNative.sourceOpened.sourceId,
        previewId: assignmentCsvNative.preview.previewId,
        approvedDigest: assignmentCsvNative.preview.preview.approvalDigest,
        commandId: assignmentCsvNative.applied.outcome.commandId,
        actor: { actorId: null, displayName: "Assignment boundary fixture" },
        truncateRedo: false,
      }).result
    ).result.report.rows;
    if (applied.family !== "assignments") throw new Error("Expected assignment receipt exclusions");
    expect(applied.excludedRows).toHaveLength(10_000);
    expect(applied.excludedRows.at(-1)?.record).toBe(10_002);
    excludedRows.push({ record: 3, skipped: true, issue: null });
    await expect(flow.report(assignmentCsvNative.preview.previewId)).rejects.toMatchObject(
      invalidResponse,
    );
    scope.dispose();
    await flow.dispose();
  });

  it("accepts the first over-limit detection record but keeps samples within UTF-8 byte bounds", async () => {
    const { flow, scope } = csvContext();
    let detection: unknown = {
      ...peopleCsvNative.detection,
      data: {
        ...peopleCsvNative.detection.data,
        dialects: [
          {
            status: "rejected",
            dialect: "comma",
            error: { code: "logicalRecordLimit", record: 10_002 },
          },
          ...peopleCsvNative.detection.data.dialects.filter((item) => item.dialect !== "comma"),
        ],
      },
    };
    nativeCsv((command, input) =>
      command === "csv_detect" ? Promise.resolve(response(input, detection)) : undefined,
    );
    const accepted = await flow.detect(scope, peopleCsvNative.sourceOpened.sourceId).result;
    if (accepted.result.family !== "people") throw new Error("Expected people detection");
    expect(accepted.result.data.dialects.find((item) => item.dialect === "comma")).toEqual({
      status: "rejected",
      dialect: "comma",
      error: { code: "logicalRecordLimit", record: 10_002 },
    });
    detection = {
      ...peopleCsvNative.detection,
      data: {
        ...peopleCsvNative.detection.data,
        dialects: peopleCsvNative.detection.data.dialects.map((candidate) =>
          candidate.dialect === "comma"
            ? {
                ...candidate,
                samples: [
                  { record: 1, cells: [{ text: "\u{10400}".repeat(17), truncated: false }] },
                ],
              }
            : candidate,
        ),
      },
    };
    await expect(
      flow.detect(scope, peopleCsvNative.sourceOpened.sourceId).result,
    ).rejects.toMatchObject(invalidResponse);
    await flow.dispose();
  });

  it("inspects complete later identities and rejects mismatched or over-limit samples", async () => {
    const { flow, scope } = csvContext();
    const sourceId = peopleCsvNative.sourceOpened.sourceId;
    const selected = { readFormat: { family: "people", dialect: "comma" }, record: 3 } as const;
    const valid = {
      schemaVersion: 1,
      sourceId,
      ...selected,
      cells: [{ text: "é".repeat(128), truncated: false }],
    };
    let sample: unknown = valid;
    nativeCsv((command, input) =>
      command === "csv_record_sample" ? Promise.resolve(response(input, sample)) : undefined,
    );
    const inspected = await flow.sample(scope, sourceId, selected).result;
    expect(inspected.result.cells?.[0]).toEqual(valid.cells[0]);
    for (const malformed of [
      { ...valid, sourceId: scenarioId },
      { ...valid, readFormat: { family: "people", dialect: "tab" } },
      { ...valid, readFormat: { family: "assignments", format: { format: "canonicalV1" } } },
      { ...valid, record: 4 },
      { ...valid, cells: [{ text: `${"é".repeat(128)}x`, truncated: true }] },
      { ...valid, cells: Array.from({ length: 65 }, () => ({ text: "", truncated: false })) },
    ]) {
      sample = malformed;
      await expect(flow.sample(scope, sourceId, selected).result).rejects.toMatchObject(
        invalidResponse,
      );
    }
    sample = { ...valid, cells: null };
    expect((await flow.sample(scope, sourceId, selected).result).result.cells).toBeNull();
    expect(() => flow.sample(scope, sourceId, { ...selected, record: 10_002 })).toThrow(RangeError);
    await flow.dispose();
  });

  it("rejects blocked previews carrying commands and apply reports that contradict committed outcomes", async () => {
    const { flow, scope } = csvContext();
    let report: unknown = { ...peopleCsvNative.applied.report, consumed: false };
    nativeCsv((command, input) => {
      if (command === "people_csv_preview")
        return Promise.resolve(
          response(
            input,
            {
              ...peopleCsvNative.preview,
              preview: {
                ...peopleCsvNative.preview.preview,
                disposition: "blocked",
                review: null,
                approvalDigest: null,
              },
            },
            0,
          ),
        );
      if (command === "csv_apply")
        return Promise.resolve(
          response(
            input,
            {
              ...peopleCsvNative.applied,
              report,
            },
            1,
          ),
        );
      return undefined;
    });
    await expect(
      flow.previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, csvInput()).result,
    ).rejects.toMatchObject(invalidResponse);
    await expect(flow.apply(scope, csvApplyInput()).result).rejects.toMatchObject(invalidResponse);
    report = { ...peopleCsvNative.applied.report, disposition: "noChanges" };
    await expect(flow.apply(scope, csvApplyInput()).result).rejects.toMatchObject(invalidResponse);
    await flow.dispose();
  });

  it("bounds CSV policy without imposing complete-person semantics on unused defaults", async () => {
    const { flow, scope } = csvContext();
    const input = csvInput();
    const defaults = input.mapping.newPersonDefaults;
    const rejectPreview = vi.fn<() => Promise<never>>().mockRejectedValue({
      ...nativeError,
      code: "people_csv.source_unavailable",
    });
    nativeCsv((command) => (command === "people_csv_preview" ? rejectPreview() : undefined));
    const previewDefaults = (newPersonDefaults: generated.PeopleCsvNewPersonDefaults) =>
      flow.previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, {
        ...input,
        mapping: { ...input.mapping, newPersonDefaults },
      });
    // These are valid policy DTO values, although a newly created person could reject them.
    await expect(
      previewDefaults({
        ...defaults,
        tags: [""],
        workloadWeight: { numerator: 0, denominator: 0 },
        display: { color: "red", avatarInitials: "" },
      }).result,
    ).rejects.toMatchObject({ code: "people_csv.source_unavailable" });
    await expect(
      previewDefaults({
        ...defaults,
        tags: ["\u{10400}".repeat(64)],
        display: { avatarInitials: "\u{10400}".repeat(4) },
        activeRange: {
          kind: "dateRange",
          startDate: "+002024-02-29",
          endDateExclusive: "+002024-03-01",
        },
        qualificationGrants: [
          { qualificationId: scenarioId, effectiveFrom: "-000004-02-29T12:00:00+00:30" },
        ],
      }).result,
    ).rejects.toMatchObject({ code: "people_csv.source_unavailable" });
    expect(() => previewDefaults({ ...defaults, tags: ["\u{10400}".repeat(65)] })).toThrow(
      RangeError,
    );
    expect(() =>
      previewDefaults({
        ...defaults,
        display: { avatarInitials: "\u{10400}".repeat(5) },
      }),
    ).toThrow(RangeError);
    const column: generated.PeopleCsvColumn = { index: 0, field: "name", blank: "preserve" };
    expect(() =>
      flow.previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, {
        ...input,
        mapping: {
          ...input.mapping,
          expectedColumns: 64,
          columns: Array.from({ length: 11 }, (_, index) => ({ ...column, index })),
        },
      }),
    ).toThrow(RangeError);
    expect(() =>
      flow.previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, {
        ...input,
        mapping: {
          ...input.mapping,
          referenceMappings: Object.fromEntries(
            Array.from({ length: 10_001 }, (_, index) => [`ref${index.toString()}`, scenarioId]),
          ),
        },
      }),
    ).toThrow(RangeError);
    await flow.dispose();
  });

  it("checks typed CSV default dates and timestamps in both requests and reviews", async () => {
    const { flow, scope } = csvContext();
    const input = csvInput();
    const invalidRange = {
      kind: "dateRange",
      startDate: "2030-02-29",
      endDateExclusive: "2030-03-01",
    } as const;
    for (const date of ["02026-01-01", "-0001-01-01", "-000000-01-01", invalidRange.startDate]) {
      expect(() =>
        flow
          .previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, {
            ...input,
            mapping: {
              ...input.mapping,
              newPersonDefaults: {
                ...input.mapping.newPersonDefaults,
                activeRange: { ...invalidRange, startDate: date },
              },
            },
          })
          .result.catch(() => undefined),
      ).toThrow(RangeError);
      expect(() =>
        flow
          .previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, {
            ...input,
            mapping: {
              ...input.mapping,
              newPersonDefaults: {
                ...input.mapping.newPersonDefaults,
                qualificationGrants: [
                  { qualificationId: scenarioId, effectiveFrom: `${date}T12:00:00+00:30` },
                ],
              },
            },
          })
          .result.catch(() => undefined),
      ).toThrow(RangeError);
    }
    const preview = structuredClone(peopleCsvNative.preview);
    const mapping = {
      ...preview.preview.review.mapping,
      newPersonDefaults: {
        ...preview.preview.review.mapping.newPersonDefaults,
        activeRange: invalidRange,
      },
    };
    nativeCsv((command, invocation) =>
      command === "people_csv_preview"
        ? Promise.resolve(
            response(
              invocation,
              {
                ...preview,
                preview: { ...preview.preview, review: { ...preview.preview.review, mapping } },
              },
              0,
            ),
          )
        : undefined,
    );
    await expect(
      flow.previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, input).result,
    ).rejects.toMatchObject(invalidResponse);
    await flow.dispose();
  });

  it("bounds CSV timestamp defaults by the native instant range after offset conversion", async () => {
    const { flow, scope } = csvContext();
    const input = csvInput();
    const rejectPreview = vi.fn<() => Promise<never>>().mockRejectedValue({
      ...nativeError,
      code: "people_csv.source_unavailable",
    });
    nativeCsv((command) => (command === "people_csv_preview" ? rejectPreview() : undefined));
    const previewTimestamp = (effectiveFrom: string) =>
      flow.previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, {
        ...input,
        mapping: {
          ...input.mapping,
          newPersonDefaults: {
            ...input.mapping.newPersonDefaults,
            qualificationGrants: [{ qualificationId: scenarioId, effectiveFrom }],
          },
        },
      });
    for (const timestamp of [
      "-009999-01-01T02:00:59-23:59",
      "9999-12-31T21:59:00.999999999+23:59",
    ]) {
      await expect(previewTimestamp(timestamp).result).rejects.toMatchObject({
        code: "people_csv.source_unavailable",
      });
    }
    for (const timestamp of ["-009999-01-02T02:00:00+00:01", "9999-12-30T21:00:00-01:01"]) {
      expect(() => previewTimestamp(timestamp).result.catch(() => undefined)).toThrow(RangeError);
    }
    await flow.dispose();
  });

  it("keeps the selected source authoritative when a structurally typed preview input has an extra sourceId", async () => {
    const { flow, scope } = csvContext();
    const rejectSource = vi.fn<() => Promise<never>>().mockRejectedValue({
      ...nativeError,
      code: "people_csv.source_unavailable",
    });
    nativeCsv((command, input) => {
      if (command !== "people_csv_preview") return undefined;
      if (input.request.sourceId !== peopleCsvNative.sourceOpened.sourceId) return rejectSource();
      return Promise.resolve(response(input, peopleCsvNative.preview, 0));
    });
    const draft = { ...csvInput(), sourceId: scenarioId };
    const accepted = await flow.previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, draft)
      .result;
    expect(accepted.result.sourceId).toBe(peopleCsvNative.sourceOpened.sourceId);
    expect(accepted.result.previewId).toBe(peopleCsvNative.preview.previewId);
    await flow.dispose();
  });

  it("waits for late acquisition and repeats creator cleanup even when release acknowledgement is invalid", async () => {
    const { flow, scope } = csvContext();
    const entered = deferred<Invocation>();
    const terminal = deferred<unknown>();
    const firstCleanup = deferred<Invocation>();
    const native = nativeCsv((command, input) => {
      if (command === "csv_source_open") {
        entered.resolve(input);
        return terminal.promise;
      }
      if (command === "operation_release")
        return Promise.resolve(response(input, { schemaVersion: 2, acknowledgement: "released" }));
      if (command === "csv_source_close") firstCleanup.resolve(input);
      return undefined;
    });
    const opened = flow.open(scope);
    const invocation = await entered.promise;
    expect(callbacks.size).toBe(1);
    let disposed = false;
    const disposal = flow.dispose().then(() => {
      disposed = true;
    });
    const cleanup = await firstCleanup.promise;
    const target = {
      kind: "creator",
      operationId: await opened.operationId,
      requestId: opened.requestId,
    };
    expect(cleanup.request.target).toEqual(target);
    expect(cleanup.request.requestId).not.toBe(opened.requestId);
    expect(disposed).toBe(false);
    expect(callbacks.size).toBe(0);
    const cleanupBeforeSettlement = native.lifecycle.length;
    terminal.resolve(response(invocation, peopleCsvNative.sourceOpened));
    expect((await opened.result).result.sourceId).toBe(peopleCsvNative.sourceOpened.sourceId);
    await disposal;
    expect(native.lifecycle.slice(cleanupBeforeSettlement)).toContainEqual({
      command: "csv_source_close",
      input: expect.objectContaining({ request: expect.objectContaining({ target }) }),
    });
    expect(() => flow.open(scope)).toThrow();
  });

  it("discards a preview that arrives after explicit release without closing its selected source", async () => {
    const { flow, scope } = csvContext();
    const entered = deferred<Invocation>();
    const terminal = deferred<unknown>();
    const native = nativeCsv((command, input) => {
      if (command === "people_csv_preview") {
        entered.resolve(input);
        return terminal.promise;
      }
      return undefined;
    });
    const source = await flow.open(scope).result;
    const preview = flow.previewPeople(scope, source.result.sourceId, csvInput());
    const invocation = await entered.promise;
    await preview.release();
    const target = {
      kind: "creator",
      operationId: await preview.operationId,
      requestId: preview.requestId,
    };
    const cleanupBeforeSettlement = native.lifecycle.length;
    terminal.resolve(response(invocation, peopleCsvNative.preview, 0));
    expect((await preview.result).result.previewId).toBe(peopleCsvNative.preview.previewId);
    expect(preview.isCurrent()).toBe(false);
    expect(native.lifecycle.slice(cleanupBeforeSettlement)).toContainEqual({
      command: "csv_preview_discard",
      input: expect.objectContaining({ request: expect.objectContaining({ target }) }),
    });
    expect(native.lifecycle.some(({ command }) => command === "csv_source_close")).toBe(false);
    expect((await flow.detect(scope, source.result.sourceId).result).result).toEqual(
      peopleCsvNative.detection,
    );
    await flow.dispose();
  });

  it.each(["source", "preview"] as const)(
    "cleans up a lost %s result by its creator without replacing the delivery error",
    async (kind) => {
      const { flow, scope } = csvContext();
      const entered = deferred<Invocation>();
      const terminal = deferred<unknown>();
      const workCommand = kind === "source" ? "csv_source_open" : "people_csv_preview";
      const cleanupCommand = kind === "source" ? "csv_source_close" : "csv_preview_discard";
      const native = nativeCsv((command, input) => {
        if (command === workCommand) {
          entered.resolve(input);
          return terminal.promise;
        }
        return undefined;
      });
      const operation =
        kind === "source"
          ? flow.open(scope)
          : flow.previewPeople(scope, peopleCsvNative.sourceOpened.sourceId, csvInput());
      await entered.promise;
      const failed = expect(operation.result).rejects.toMatchObject(invalidResponse);
      terminal.reject("native acquisition completed but its IPC response was lost");
      await failed;
      expect(native.lifecycle).toEqual([
        {
          command: cleanupCommand,
          input: expect.objectContaining({
            request: expect.objectContaining({
              target: {
                kind: "creator",
                operationId: await operation.operationId,
                requestId: operation.requestId,
              },
            }),
          }),
        },
      ]);
      expect(callbacks.size).toBe(0);
      await flow.dispose();
    },
  );

  it("preserves selected sources and retained previews when a revision scope is replaced", async () => {
    const native = nativeCsv();
    const { flow, scope } = csvContext();
    const source = await flow.open(scope).result;
    const preview = await flow.previewPeople(scope, source.result.sourceId, csvInput()).result;
    scope.dispose();
    const replacement = new generated.SetupOperationScope(peopleCsvNative.preview.scenarioId, 1);
    const detected = await flow.detect(replacement, source.result.sourceId).result;
    const report = await flow.report(preview.result.previewId);
    expect(detected.result).toEqual(peopleCsvNative.detection);
    expect(report.result.previewId).toBe(preview.result.previewId);
    expect(native.lifecycle).toEqual([]);
    await flow.closeSource(source.result.sourceId);
    expect((await flow.report(preview.result.previewId)).result.rows).toEqual(
      peopleCsvNative.rejectedRows.rows,
    );
    await flow.dispose();
  });

  it("does not rebind an apply receipt to later draft edits or let optional report-save failure erase the commit", async () => {
    const { flow, scope } = csvContext();
    const entered = deferred<Invocation>();
    const terminal = deferred<unknown>();
    const rejectSave = vi.fn<() => Promise<never>>().mockRejectedValue({
      ...nativeError,
      code: "people_csv.destination_exists",
    });
    nativeCsv((command, input) => {
      if (command === "csv_apply") {
        entered.resolve(input);
        return terminal.promise;
      }
      if (command === "csv_report_save") return rejectSave();
      return undefined;
    });
    const draft = csvApplyInput();
    const apply = flow.apply(scope, draft);
    draft.sourceId = scenarioId;
    draft.previewId = operationId;
    draft.commandId = scenarioId;
    const invocation = await entered.promise;
    terminal.resolve(response(invocation, peopleCsvNative.applied, 1));
    const committed = await apply.result;
    expect(committed.result.outcome).toEqual(peopleCsvNative.applied.outcome);
    expect(committed.result.report.consumed).toBe(true);
    const current = new generated.SetupOperationScope(peopleCsvNative.preview.scenarioId, 1);
    await expect(
      flow.saveReport(current, committed.result.report.previewId).result,
    ).rejects.toMatchObject({ code: "people_csv.destination_exists" });
    expect((await flow.report(committed.result.report.previewId)).result.rows).toEqual(
      peopleCsvNative.rejectedRows.rows,
    );
    expect(await apply.result).toBe(committed);
    await flow.dispose();
  });
});

function settingsPreview(): generated.SettingsImportPreviewV1 {
  return {
    kind: "preview",
    schemaVersion: 1,
    previewId: scenarioId,
    sourceSha256: "a".repeat(64),
    approvalSha256: "b".repeat(64),
    libraryRevision: 7,
    changes: [
      {
        key: "appearance",
        before: null,
        after: {
          value: { theme: "dark", reducedMotion: true },
          updatedAt: "2026-09-12T12:00:00.123456789Z",
        },
      },
    ],
  };
}

function nativeSettings() {
  const entered = deferred<Invocation>();
  const terminal = deferred<unknown>();
  const discarded = deferred<Invocation>();
  const discards: Invocation[] = [];
  tauri.invoke.mockImplementation((command: string, input: Invocation) => {
    if (command === "operation_prepare")
      return Promise.resolve(response(input, { schemaVersion: 1, operationId }));
    if (command === "operation_cancel" || command === "operation_release")
      return Promise.resolve(
        response(input, {
          schemaVersion: 1,
          acknowledgement: command === "operation_cancel" ? "cancellationRequested" : "released",
        }),
      );
    if (command === "settings_import_nonsecret" && input.request.action === "discard") {
      discards.push(input);
      discarded.resolve(input);
      return Promise.resolve(response(input, { kind: "discarded", schemaVersion: 1 }));
    }
    entered.resolve(input);
    return terminal.promise;
  });
  return { entered, terminal, discarded, discards };
}

describe("settings library operations and native review custody", () => {
  it("rejects a later library snapshot masquerading as the exact settings commit", async () => {
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(
        response(
          input,
          {
            schemaVersion: 1,
            libraryRevision: 9,
            settings: {
              appearance: null,
              locale: { value: "en-GB", updatedAt: "2026-09-01T00:00:00Z" },
              units: null,
            },
            changed: true,
          },
          9,
        ),
      ),
    );
    await expect(generated.updateSetting("locale", "en-GB", 7)).rejects.toMatchObject(
      invalidResponse,
    );
  });

  it("captures apply approval and owns strict action, identity and library revision before preparation", async () => {
    const native = nativeOperations();
    const scope = new generated.LibraryOperationScope(7);
    const flow = new generated.SettingsImportFlow();
    const draft = { previewId: scenarioId, approvalSha256: "b".repeat(64) };
    Object.assign(draft, {
      action: "discard",
      schemaVersion: 99,
      requestId: scenarioId,
      operationId: scenarioId,
      scenarioId,
      expectedRevision: 900,
      expectedLibraryRevision: 900,
    });
    const apply = flow.apply(scope, draft);
    draft.previewId = operationId;
    draft.approvalSha256 = "c".repeat(64);
    native.prepared();
    const invocation = await native.claimed.promise;
    expect(invocation.request).toEqual({
      action: "apply",
      schemaVersion: 1,
      requestId: apply.requestId,
      operationId,
      previewId: scenarioId,
      approvalSha256: "b".repeat(64),
      expectedLibraryRevision: 7,
    });
    await apply.cancel();
    await apply.release();
    native.terminal.resolve(
      response(
        invocation,
        {
          kind: "applied",
          schemaVersion: 1,
          previewId: scenarioId,
          libraryRevision: 8,
          changed: true,
        },
        8,
      ),
    );
    expect((await apply.result).result).toMatchObject({ changed: true, libraryRevision: 8 });
    expect(apply.isCurrent()).toBe(false);
    expect(callbacks.size).toBe(0);
    await flow.dispose();
  });

  it("correlates full library progress identity and projects no scenario or expected revision into export", async () => {
    const native = nativeOperations();
    const scope = new generated.LibraryOperationScope(null);
    const sequences: number[] = [];
    const exported = generated.exportNonsecretSettings(scope, (event) =>
      sequences.push(event.sequence),
    );
    native.prepared();
    const invocation = await native.claimed.promise;
    expect(invocation.request).toEqual({
      schemaVersion: 1,
      requestId: exported.requestId,
      operationId,
    });
    const callback = invocation.onProgress && callbacks.get(invocation.onProgress.id);
    if (!callback) throw new Error("No SDK callback");
    const progress: generated.OperationProgressV1 = {
      eventVersion: 1,
      timestamp: "2026-09-12T12:00:00Z",
      operationId,
      requestId: exported.requestId,
      windowLabel: "main",
      context: scope.context,
      sequence: 1,
      phase: "publishingFile",
    };
    const messages = [
      { ...progress, context: { kind: "scenario", scenarioId, expectedRevision: null } },
      { ...progress, context: { kind: "library", expectedLibraryRevision: 7 } },
      { ...progress, context: { ...scope.context, scenarioId } },
      { ...progress, windowLabel: "other" },
      { ...progress, requestId: scenarioId },
      { ...progress, eventVersion: 2 },
      progress,
      { ...progress, sequence: 1 },
      { ...progress, sequence: 2 },
    ];
    messages.forEach((message, index) => {
      callback({ message, index });
    });
    expect(sequences).toEqual([1, 2]);
    native.terminal.resolve(
      response(invocation, { schemaVersion: 1, libraryRevision: 7, writtenBytes: 512 }, 7),
    );
    expect((await exported.result).result.libraryRevision).toBe(7);
    expect(callbacks.size).toBe(0);
    expect(() =>
      generated.exportNonsecretSettings(new generated.LibraryOperationScope(7)),
    ).toThrow();
    expect(() =>
      new generated.SettingsImportFlow().apply(scope, {
        previewId: scenarioId,
        approvalSha256: "a".repeat(64),
      }),
    ).toThrow();
  });

  it.each(["release", "scope", "flow"] as const)(
    "cleans creator custody on %s abandonment and again after late native settlement",
    async (abandonment) => {
      const native = nativeSettings();
      const scope = new generated.LibraryOperationScope(null);
      const flow = new generated.SettingsImportFlow();
      const preview = flow.preview(scope);
      const invocation = await native.entered.promise;
      expect(invocation.request).toEqual({
        action: "preview",
        schemaVersion: 1,
        requestId: preview.requestId,
        operationId,
      });
      let disposal: Promise<unknown> | undefined;
      if (abandonment === "release") disposal = preview.release();
      else if (abandonment === "scope") scope.dispose();
      else disposal = flow.dispose();
      const cleanup = await native.discarded.promise;
      const target = { kind: "creator", operationId, requestId: preview.requestId };
      expect(cleanup.request.target).toEqual(target);
      expect(cleanup.onProgress).toBeUndefined();
      expect(callbacks.size).toBe(0);
      const beforeSettlement = native.discards.length;
      native.terminal.resolve(response(invocation, settingsPreview(), 7));
      expect((await preview.result).result.previewId).toBe(scenarioId);
      await disposal;
      expect(
        native.discards.slice(beforeSettlement).map((input) => input.request.target),
      ).toContainEqual(target);
      expect(preview.isCurrent()).toBe(false);
      await flow.dispose();
    },
  );

  it("keeps a ready review discardable after revision scope replacement, without a progress channel", async () => {
    const native = nativeSettings();
    const scope = new generated.LibraryOperationScope(null);
    const flow = new generated.SettingsImportFlow();
    const preview = flow.preview(scope);
    const invocation = await native.entered.promise;
    native.terminal.resolve(response(invocation, settingsPreview(), 7));
    const ready = await preview.result;
    scope.dispose();
    expect(native.discards).toEqual([]);
    await flow.discardPreview(ready.result.previewId);
    expect(native.discards.map((input) => input.request.target)).toEqual([
      { kind: "preview", previewId: scenarioId },
    ]);
    await flow.dispose();
  });

  it("cleans an undelivered preview by creator without replacing its delivery error or replaying acquisition", async () => {
    const native = nativeSettings();
    const flow = new generated.SettingsImportFlow();
    const preview = flow.preview(new generated.LibraryOperationScope(null));
    await native.entered.promise;
    const failed = expect(preview.result).rejects.toMatchObject(invalidResponse);
    native.terminal.reject("result delivery failed");
    await failed;
    expect(native.discards.map((input) => input.request.target)).toEqual([
      {
        kind: "creator",
        operationId,
        requestId: preview.requestId,
      },
    ]);
    expect(
      tauri.invoke.mock.calls.filter(
        ([command, input]) =>
          command === "settings_import_nonsecret" &&
          (input as Invocation).request.action === "preview",
      ),
    ).toHaveLength(1);
    await flow.dispose();
  });

  it("still discards before and after settlement when release acknowledgement is malformed", async () => {
    const native = nativeSettings();
    const original = tauri.invoke.getMockImplementation();
    tauri.invoke.mockImplementation((command: string, input: Invocation) =>
      command === "operation_release"
        ? Promise.resolve({ invalid: true })
        : original?.(command, input),
    );
    const flow = new generated.SettingsImportFlow();
    const preview = flow.preview(new generated.LibraryOperationScope(null));
    const invocation = await native.entered.promise;
    const disposal = flow.dispose();
    await native.discarded.promise;
    const beforeSettlement = native.discards.length;
    native.terminal.resolve(response(invocation, settingsPreview(), 7));
    await preview.result;
    await disposal;
    expect(
      native.discards.slice(beforeSettlement).map((input) => input.request.target),
    ).toContainEqual({
      kind: "creator",
      operationId,
      requestId: preview.requestId,
    });
  });

  it.each([
    { kind: "applied", schemaVersion: 1, previewId: scenarioId, libraryRevision: 7, changed: true },
    {
      kind: "applied",
      schemaVersion: 1,
      previewId: scenarioId,
      libraryRevision: 8,
      changed: false,
    },
    {
      kind: "applied",
      schemaVersion: 1,
      previewId: operationId,
      libraryRevision: 8,
      changed: true,
    },
    {
      kind: "applied",
      schemaVersion: 1,
      previewId: scenarioId,
      libraryRevision: 8,
      changed: true,
      changes: [],
    },
    { kind: "discarded", schemaVersion: 1 },
  ])("rejects malformed apply receipt %# without replaying the mutation", async (result) => {
    const native = nativeOperations();
    const flow = new generated.SettingsImportFlow();
    const apply = flow.apply(new generated.LibraryOperationScope(7), {
      previewId: scenarioId,
      approvalSha256: "b".repeat(64),
    });
    native.prepared();
    const invocation = await native.claimed.promise;
    const failed = expect(apply.result).rejects.toMatchObject(invalidResponse);
    native.terminal.resolve(response(invocation, result, 8));
    await failed;
    expect(
      tauri.invoke.mock.calls.filter(([command]) => command === "settings_import_nonsecret"),
    ).toHaveLength(1);
    expect(callbacks.size).toBe(0);
    await flow.dispose();
  });

  it.each([
    { schemaVersion: 2 },
    {
      changes: [
        {
          key: "secret",
          before: null,
          after: { value: "hidden", updatedAt: "2026-09-12T12:00:00Z" },
        },
      ],
    },
    {
      changes: [
        {
          key: "appearance",
          before: null,
          after: { value: { theme: "blue" }, updatedAt: "2026-09-12T12:00:00Z" },
        },
      ],
    },
    {
      changes: [
        {
          key: "appearance",
          before: null,
          after: { value: { reducedMotion: null }, updatedAt: "2026-09-12T12:00:00Z" },
        },
      ],
    },
    {
      changes: [
        {
          key: "locale",
          before: null,
          after: { value: "en--US", updatedAt: "2026-09-12T12:00:00Z" },
        },
      ],
    },
    {
      changes: [
        {
          key: "units",
          before: null,
          after: { value: "imperial", updatedAt: "2026-09-12T12:00:00Z" },
        },
      ],
    },
    {
      changes: [
        { key: "locale", before: null, after: { value: "en", updatedAt: "2026-02-30T00:00:00Z" } },
      ],
    },
    {
      changes: [
        {
          key: "locale",
          before: null,
          after: { value: "en", updatedAt: "2026-09-12T12:00:00Z", path: "/private" },
        },
      ],
    },
    { changes: [{ key: "locale", before: null, after: null }] },
    {
      changes: [
        {
          key: "locale",
          before: { value: "en", updatedAt: "2026-09-12T12:00:00Z" },
          after: { value: "en", updatedAt: "2026-09-12T12:00:00Z" },
        },
      ],
    },
    {
      changes: [
        {
          key: "appearance",
          before: {
            value: { theme: "dark", reducedMotion: true },
            updatedAt: "2026-09-12T12:00:00Z",
          },
          after: {
            value: { reducedMotion: true, theme: "dark" },
            updatedAt: "2026-09-12T12:00:00Z",
          },
        },
      ],
    },
    { changes: [...settingsPreview().changes, ...settingsPreview().changes] },
    {
      changes: [
        {
          key: "units",
          before: null,
          after: { value: "metric", updatedAt: "2026-09-12T12:00:00Z" },
        },
        ...settingsPreview().changes,
      ],
    },
    { sourceSha256: "A".repeat(64) },
    { path: "/private/settings.json" },
    { libraryRevision: 8 },
  ])("rejects unsupported settings review data %# and discards its creator", async (patch) => {
    const native = nativeSettings();
    const flow = new generated.SettingsImportFlow();
    const preview = flow.preview(new generated.LibraryOperationScope(null));
    const invocation = await native.entered.promise;
    const failed = expect(preview.result).rejects.toMatchObject(invalidResponse);
    native.terminal.resolve(response(invocation, { ...settingsPreview(), ...patch }, 7));
    await failed;
    expect(native.discards.map((input) => input.request.target)).toContainEqual({
      kind: "creator",
      operationId,
      requestId: preview.requestId,
    });
    await flow.dispose();
  });

  it("accepts all three ordered changes, complete entries and locally valid unportable before-state", async () => {
    const entry = { value: "com1", updatedAt: "2026-09-12T12:00:00.123456789Z" };
    const result: generated.SettingsImportPreviewV1 = {
      ...settingsPreview(),
      changes: [
        { key: "appearance", before: { value: {}, updatedAt: entry.updatedAt }, after: null },
        {
          key: "locale",
          before: entry,
          after: { ...entry, value: "en-US", updatedAt: "2026-09-12T12:00:01Z" },
        },
        {
          key: "units",
          before: { value: "us-customary", updatedAt: entry.updatedAt },
          after: { value: "us-customary", updatedAt: "2026-09-12T12:00:01Z" },
        },
      ],
    };
    const native = nativeSettings();
    const flow = new generated.SettingsImportFlow();
    const preview = flow.preview(new generated.LibraryOperationScope(null));
    const invocation = await native.entered.promise;
    native.terminal.resolve(response(invocation, result, 7));
    expect((await preview.result).result.changes).toEqual(result.changes);
    await flow.dispose();
  });

  it("accepts an identical no-op at the reviewed revision without requiring a revision increment", async () => {
    const native = nativeOperations();
    const flow = new generated.SettingsImportFlow();
    const apply = flow.apply(new generated.LibraryOperationScope(7), {
      previewId: scenarioId,
      approvalSha256: "b".repeat(64),
    });
    native.prepared();
    const invocation = await native.claimed.promise;
    native.terminal.resolve(
      response(
        invocation,
        {
          kind: "applied",
          schemaVersion: 1,
          previewId: scenarioId,
          libraryRevision: 7,
          changed: false,
        },
        7,
      ),
    );
    expect((await apply.result).result).toMatchObject({ changed: false, libraryRevision: 7 });
    await flow.dispose();
  });

  it.each([
    { schemaVersion: 1, libraryRevision: 8, writtenBytes: 512 },
    { schemaVersion: 1, libraryRevision: 7, writtenBytes: 65_537 },
    { schemaVersion: 1, libraryRevision: 7, writtenBytes: 0 },
    { schemaVersion: 1, libraryRevision: 7, writtenBytes: 512, destination: "/private" },
  ])("rejects malformed export receipt %# without republishing", async (result) => {
    const native = nativeOperations();
    const exported = generated.exportNonsecretSettings(new generated.LibraryOperationScope(null));
    native.prepared();
    const invocation = await native.claimed.promise;
    const failed = expect(exported.result).rejects.toMatchObject(invalidResponse);
    native.terminal.resolve(response(invocation, result, 7));
    await failed;
    expect(
      tauri.invoke.mock.calls.filter(([command]) => command === "settings_export_nonsecret"),
    ).toHaveLength(1);
    expect(callbacks.size).toBe(0);
  });
});

function licenseInventory(): generated.LicenseInventoryV2 {
  return {
    scope: "lockedWorkspace",
    schemaVersion: 2,
    generatedBy: "cargo xtask licenses generate",
    authoritativeInputs: ["Cargo.lock", "pnpm-lock.yaml", "xtask/supply-chain-inputs.json"],
    packages: [
      {
        ecosystem: "cargo",
        name: "example",
        version: "1.0.0",
        kind: "dependency",
        licenseConcluded: "NOASSERTION",
        source: "registry+https://example.invalid/index",
      },
    ],
  };
}

describe("bounded offline inventory and redacted path reads", () => {
  it("preserves unknown license conclusions, required sources and omitted checksums", async () => {
    const inventory = licenseInventory();
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, inventory)),
    );
    expect((await generated.getLicenseInventory()).result).toEqual(inventory);
    const item = inventory.packages[0];
    if (!item) throw new Error("Missing package");
    const checksummed = {
      ...inventory,
      packages: [
        { ...item, checksum: { algorithm: "SHA256", value: "a".repeat(64) } },
        { ...item, checksum: { algorithm: "SHA512", value: "A".repeat(128) } },
      ],
    };
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, checksummed)),
    );
    expect((await generated.getLicenseInventory()).result.packages).toEqual(checksummed.packages);
  });

  it.each([
    { source: null },
    { checksum: null },
    { checksum: { algorithm: "SHA256", value: "a".repeat(128) } },
    { checksum: { algorithm: "SHA512", value: "z".repeat(128) } },
    { checksum: { algorithm: "SHA1", value: "a".repeat(40) } },
    { checksum: { algorithm: "SHA256", value: "a".repeat(64), approved: true } },
    { name: "é".repeat(512) + "a" },
    { licenseConcluded: "x".repeat(4097) },
    { ecosystem: "pip" },
    { kind: "installed" },
    { source: "" },
  ])("rejects unsupported or over-limit package metadata %#", async (patch) => {
    const inventory = licenseInventory();
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(
        response(input, {
          ...inventory,
          packages: [{ ...inventory.packages[0], ...patch }],
        }),
      ),
    );
    await expect(generated.getLicenseInventory()).rejects.toMatchObject(invalidResponse);
  });

  it("enforces inventory count, compact bytes, exact metadata and unrevisioned envelopes", async () => {
    const inventory = licenseInventory();
    const item = inventory.packages[0];
    if (!item) throw new Error("Missing package");
    for (const result of [
      { ...inventory, schemaVersion: 3 },
      { ...inventory, scope: "installedArtifact" },
      { ...inventory, authoritativeInputs: ["Cargo.lock"] },
      { ...inventory, packages: Array.from({ length: 4097 }, () => item) },
      {
        ...inventory,
        packages: Array.from({ length: 600 }, () => ({
          ...item,
          licenseConcluded: "x".repeat(4096),
        })),
      },
    ]) {
      tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
        Promise.resolve(response(input, result)),
      );
      await expect(generated.getLicenseInventory()).rejects.toMatchObject(invalidResponse);
    }
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, inventory, 7)),
    );
    await expect(generated.getLicenseInventory()).rejects.toMatchObject(invalidResponse);
    const paths = { appDataConfigured: true, cacheConfigured: true, backupConfigured: false };
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, paths)),
    );
    expect((await generated.getAppPathsSummary()).result).toEqual(paths);
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(
        response(input, {
          ...paths,
          appDataPath: "/private",
        }),
      ),
    );
    await expect(generated.getAppPathsSummary()).rejects.toMatchObject(invalidResponse);
  });
});

const portableOptions: generated.ImportOptions = {
  restoreMode: "replace-library",
  includeResults: true,
  includeAssets: false,
};
const portableCollisionPlan: generated.CollisionPlan = { scenarios: {}, supplementalChoices: [] };
const portableAuthorization: generated.RestoreAuthorizationDto = {
  destructiveActionConfirmed: true,
  safetyBackupBypassPhrase: null,
};
function portablePreview(previewId: string): generated.PortablePreviewDto {
  return {
    schemaVersion: 1,
    previewId,
    libraryRevision: 7,
    bundleId: scenarioId,
    bundleKind: "full-backup",
    title: "Review",
    createdAt: "2026-09-12T12:00:00Z",
    sourceApplication: { name: "Eutheto", version: "0.1.0" },
    sourceFormatVersion: 1,
    sourceSchemaVersion: 1,
    counts: {
      scenarios: 0,
      scenarioRevisions: 0,
      results: 0,
      sharedRecords: 0,
      preferences: 0,
      assets: 0,
    },
    requiredCapabilities: [],
    preservedExtensions: [],
    includedSections: [],
    excludedSections: [],
    sourceBackupSelection: null,
    omittedAssets: [],
    scenarios: [],
    supplementalCollisions: [],
    removedScenarios: [],
    removedSupplemental: [],
    settingsChanged: [],
    settingsRemoved: [],
    appliedMigrations: [],
  };
}

/** Deterministic transport fault fixtures; native custody itself is covered by Rust regressions. */
function portableTransport(
  override?: (command: string, input: Invocation) => Promise<unknown> | undefined,
) {
  const discards: Invocation[] = [];
  let nextOperation = 0;
  tauri.invoke.mockImplementation((command: string, input: Invocation) => {
    if (command === "project_operation_cancel") discards.push(input);
    const overridden = override?.(command, input);
    if (overridden !== undefined) return overridden;
    if (command === "operation_prepare")
      return Promise.resolve(
        response(input, {
          schemaVersion: 1,
          operationId: `01900000-0000-7000-8000-${String(++nextOperation).padStart(12, "0")}`,
        }),
      );
    if (command === "operation_release" || command === "operation_cancel")
      return Promise.resolve(
        response(input, {
          schemaVersion: 1,
          acknowledgement: command === "operation_release" ? "released" : "cancellationRequested",
        }),
      );
    if (command === "project_operation_cancel")
      return Promise.resolve(response(input, { schemaVersion: 1 }));
    if (command === "project_import_preview" || command === "project_restore_preview")
      return Promise.resolve(response(input, portablePreview(input.request.requestId), 7));
    if (command === "project_export_preview" || command === "project_backup_preview") {
      const exporting = command === "project_export_preview";
      return Promise.resolve(
        response(
          input,
          {
            schemaVersion: 1,
            previewId: input.request.requestId,
            title: "Output",
            byteLength: 512,
            digest: "a".repeat(64),
            currentRevision: exporting ? 3 : null,
            libraryRevision: 7,
            backupSummary: exporting
              ? null
              : {
                  includeResults: true,
                  assetSelection: "all",
                  excludedAssetCount: 0,
                  excludedAssetIds: [],
                  exclusionScope: null,
                  thresholdVersion: null,
                  thresholdBytes: null,
                  fixedExclusions: [],
                },
          },
          exporting ? 3 : 7,
        ),
      );
    }
    if (command === "project_unopened_bundle_inspect")
      return Promise.resolve(
        response(input, {
          schemaVersion: 1,
          previewId: input.request.requestId,
          metadata: {
            fileSha256: "a".repeat(64),
            format: "eutheto",
            formatVersion: 2,
            portableSchemaVersion: 2,
            bundleKind: null,
            title: null,
            requiredCapabilities: [],
            scenarios: [],
          },
        }),
      );
    if (command === "project_import_apply" || command === "project_restore_apply")
      return Promise.resolve(
        response(
          input,
          {
            schemaVersion: 1,
            libraryRevision: 8,
            scenarioIds: [],
            safetyBackup:
              command === "project_import_apply"
                ? { kind: "notRequired" }
                : { kind: "createdAndVerified", artifactName: "actual-safety.eutheto" },
          },
          8,
        ),
      );
    if (
      command === "project_export_create" ||
      command === "project_backup_create" ||
      command === "project_unopened_bundle_reexport"
    ) {
      const exporting = command === "project_export_create";
      const unopened = command === "project_unopened_bundle_reexport";
      return Promise.resolve(
        response(
          input,
          {
            schemaVersion: 1,
            artifactName: "selected.eutheto",
            currentRevision: exporting ? 3 : null,
            libraryRevision: unopened ? null : 7,
          },
          exporting ? 3 : unopened ? null : 7,
        ),
      );
    }
    throw new Error(`Unexpected portable command: ${command}`);
  });
  return { discards };
}

describe("portable review ownership", () => {
  it("cleans malformed and lost creator replies even when release acknowledgement fails", async () => {
    const transport = portableTransport((command, input) => {
      // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors -- Native IPC rejects closed error DTOs, not Error instances.
      if (command === "operation_release") return Promise.reject(nativeError);
      if (command === "project_restore_preview")
        return Promise.resolve(
          response(input, { ...portablePreview(input.request.requestId), schemaVersion: 2 }, 7),
        );
      if (command === "project_import_preview") return Promise.reject(new Error("lost reply"));
      return undefined;
    });
    const flow = new generated.PortableReviewFlow();
    const scope = new generated.LibraryOperationScope(null);
    const malformed = flow.previewRestore(scope, portableOptions, "safetyBackups");
    await expect(malformed.result).rejects.toMatchObject(invalidResponse);
    const lost = flow.previewImport(scope, portableOptions);
    await expect(lost.result).rejects.toMatchObject(invalidResponse);
    expect(transport.discards.map((entry) => entry.request.target)).toEqual([
      { kind: "creator", operationId: await malformed.operationId, requestId: malformed.requestId },
      { kind: "creator", operationId: await lost.operationId, requestId: lost.requestId },
    ]);
    expect(callbacks.size).toBe(0);
    await flow.dispose();
  });

  it("waits for a late creator and repeats cleanup after disposal acknowledgement", async () => {
    const entered = deferred<Invocation>();
    const terminal = deferred<unknown>();
    const discarded = deferred<undefined>();
    const transport = portableTransport((command, input) => {
      if (command === "project_unopened_bundle_inspect") {
        entered.resolve(input);
        return terminal.promise;
      }
      if (command === "project_operation_cancel") discarded.resolve(undefined);
      return undefined;
    });
    const flow = new generated.PortableReviewFlow();
    const operation = flow.inspectUnopenedBundle(new generated.LibraryOperationScope(null));
    const invocation = await entered.promise;
    let disposed = false;
    const disposal = flow.dispose().then(() => {
      disposed = true;
    });
    await discarded.promise;
    expect(disposed).toBe(false);
    const earlyDiscards = transport.discards.length;
    terminal.resolve(
      response(invocation, {
        schemaVersion: 1,
        previewId: scenarioId,
        metadata: {
          fileSha256: "a".repeat(64),
          format: "eutheto",
          formatVersion: 2,
          portableSchemaVersion: 2,
          bundleKind: null,
          title: null,
          requiredCapabilities: [],
          scenarios: [],
        },
      }),
    );
    await operation.result;
    await disposal;
    expect(transport.discards.length).toBeGreaterThan(earlyDiscards);
    expect(transport.discards.at(-1)?.request.target).toEqual({
      kind: "creator",
      operationId: await operation.operationId,
      requestId: operation.requestId,
    });
    expect(operation.isCurrent()).toBe(false);
    expect(callbacks.size).toBe(0);
  });

  it("reserves separate three-entry groups and refuses cross-kind or cross-revision consumption", async () => {
    portableTransport();
    const flow = new generated.PortableReviewFlow();
    const library = new generated.LibraryOperationScope(null);
    const scenario = new generated.SetupOperationScope(scenarioId, 3);
    const imported = (await flow.previewImport(library, portableOptions).result).result;
    await flow.previewRestore(library, portableOptions, "userSelected").result;
    const unopened = (await flow.inspectUnopenedBundle(library).result).result;
    const exported = (await flow.previewExport(scenario).result).result;
    const backup = (await flow.previewBackup(library, "Backup").result).result;
    await flow.previewExport(scenario).result;
    expect(() => flow.previewImport(library, portableOptions)).toThrow(RangeError);
    expect(() => flow.previewBackup(library, "Fourth")).toThrow(RangeError);
    expect(() =>
      flow.applyImport(new generated.LibraryOperationScope(8), {
        previewId: imported.previewId,
        collisionPlan: portableCollisionPlan,
      }),
    ).toThrow(RangeError);
    expect(() =>
      flow.applyRestore(new generated.LibraryOperationScope(7), {
        previewId: imported.previewId,
        collisionPlan: portableCollisionPlan,
        authorization: portableAuthorization,
      }),
    ).toThrow(RangeError);
    expect(() =>
      flow.createExport(new generated.SetupOperationScope(operationId, 3), {
        previewId: exported.previewId,
        expectedLibraryRevision: 7,
      }),
    ).toThrow(RangeError);
    expect(() =>
      flow.createExport(scenario, {
        previewId: exported.previewId,
        expectedLibraryRevision: 8,
      }),
    ).toThrow(RangeError);
    expect(() =>
      new generated.PortableReviewFlow().createBackup(
        new generated.LibraryOperationScope(7),
        backup.previewId,
      ),
    ).toThrow(RangeError);
    await flow.reexportUnopenedBundle(library, unopened.previewId).result;
    await flow.inspectUnopenedBundle(library).result;
    await flow.createBackup(new generated.LibraryOperationScope(7), backup.previewId).result;
    await flow.previewBackup(library, "Freed prepared slot").result;
    await flow.dispose();
  });

  it.each([
    { detail: { type: "boolean", value: true }, restore: true, retained: true },
    { detail: { type: "boolean", value: false }, restore: true, retained: false },
    { detail: { type: "text", value: "true" }, restore: true, retained: false },
    { detail: undefined, restore: true, retained: false },
    { detail: { type: "boolean", value: true }, restore: false, retained: false },
  ])(
    "retains only an owned restore with actual Boolean evidence: %j",
    async ({ detail, restore, retained }) => {
      let attempts = 0;
      portableTransport((command) => {
        if (
          (command === "project_restore_apply" || command === "project_import_apply") &&
          ++attempts === 1
        )
          // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors -- Exercise the native error DTO and its typed retention evidence.
          return Promise.reject({
            ...nativeError,
            code: "restore.safety_backup_failed",
            retryable: true,
            details: detail === undefined ? null : { portablePreviewRetained: detail },
          });
        return undefined;
      });
      const flow = new generated.PortableReviewFlow();
      const library = new generated.LibraryOperationScope(null);
      const preview = (
        await (
          restore
            ? flow.previewRestore(library, portableOptions, "userSelected")
            : flow.previewImport(library, portableOptions)
        ).result
      ).result;
      const scope = new generated.LibraryOperationScope(7);
      const apply = () =>
        restore
          ? flow.applyRestore(scope, {
              previewId: preview.previewId,
              collisionPlan: portableCollisionPlan,
              authorization: portableAuthorization,
            })
          : flow.applyImport(scope, {
              previewId: preview.previewId,
              collisionPlan: portableCollisionPlan,
            });
      await expect(apply().result).rejects.toMatchObject({ code: "restore.safety_backup_failed" });
      if (retained) {
        expect((await apply().result).result.safetyBackup).toEqual({
          kind: "createdAndVerified",
          artifactName: "actual-safety.eutheto",
        });
      } else {
        expect(apply).toThrow(RangeError);
      }
      await flow.dispose();
    },
  );

  it("preserves an owned committed restore after late cancellation and disposal", async () => {
    const entered = deferred<Invocation>();
    const terminal = deferred<unknown>();
    portableTransport((command, input) => {
      if (command === "project_restore_apply") {
        entered.resolve(input);
        return terminal.promise;
      }
      return undefined;
    });
    const flow = new generated.PortableReviewFlow();
    const preview = (
      await flow.previewRestore(
        new generated.LibraryOperationScope(null),
        portableOptions,
        "userSelected",
      ).result
    ).result;
    const apply = flow.applyRestore(new generated.LibraryOperationScope(7), {
      previewId: preview.previewId,
      collisionPlan: portableCollisionPlan,
      authorization: portableAuthorization,
    });
    const invocation = await entered.promise;
    await apply.cancel();
    const disposal = flow.dispose();
    terminal.resolve(
      response(
        invocation,
        {
          schemaVersion: 1,
          libraryRevision: 8,
          scenarioIds: [],
          safetyBackup: {
            kind: "createdAndVerified",
            artifactName: "verified-before-cancel.eutheto",
          },
        },
        8,
      ),
    );
    expect((await apply.result).result.safetyBackup).toEqual({
      kind: "createdAndVerified",
      artifactName: "verified-before-cancel.eutheto",
    });
    await disposal;
    expect(apply.isCurrent()).toBe(false);
  });

  it("rejects revision-bearing discovery contexts and malformed output bindings before exposing a review", async () => {
    const transport = portableTransport((command, input) => {
      if (command === "project_export_preview")
        return Promise.resolve(
          response(
            input,
            {
              schemaVersion: 1,
              previewId: scenarioId,
              title: "Wrong revision",
              byteLength: 512,
              digest: "a".repeat(64),
              backupSummary: null,
              currentRevision: 4,
              libraryRevision: 7,
            },
            4,
          ),
        );
      return undefined;
    });
    const flow = new generated.PortableReviewFlow();
    const revisioned = new generated.LibraryOperationScope(7);
    expect(() => flow.previewImport(revisioned, portableOptions)).toThrow(RangeError);
    expect(() => flow.previewBackup(revisioned, "Invalid context")).toThrow(RangeError);
    expect(() => flow.previewRestore(revisioned, portableOptions, "safetyBackups")).toThrow(
      RangeError,
    );
    expect(() => flow.inspectUnopenedBundle(revisioned)).toThrow(RangeError);
    expect(() => flow.previewExport(new generated.SetupOperationScope(scenarioId, null))).toThrow(
      RangeError,
    );
    expect(tauri.invoke).not.toHaveBeenCalled();
    await expect(
      flow.previewExport(new generated.SetupOperationScope(scenarioId, 3)).result,
    ).rejects.toMatchObject(invalidResponse);
    expect(transport.discards).toHaveLength(1);
    await flow.dispose();
  });

  it("enforces the portable compact result and final request caps and frees failed review custody", async () => {
    const chunk = "x".repeat(1_048_576);
    let oversized = true;
    const transport = portableTransport((command, input) => {
      if (command === "project_import_preview" && oversized)
        return Promise.resolve(
          response(
            input,
            {
              ...portablePreview(input.request.requestId),
              settingsChanged: Array<string>(65).fill(chunk),
            },
            7,
          ),
        );
      return undefined;
    });
    const flow = new generated.PortableReviewFlow();
    const scope = new generated.LibraryOperationScope(null);
    await expect(flow.previewImport(scope, portableOptions).result).rejects.toMatchObject(
      invalidResponse,
    );
    oversized = false;
    const preview = (await flow.previewImport(scope, portableOptions).result).result;
    const choices = Array.from({ length: 63 }, () => ({
      section: "assets" as const,
      key: chunk,
      action: "skip" as const,
    }));
    const last = { section: "assets" as const, key: "", action: "skip" as const };
    choices.push(last);
    const collisionPlan = { scenarios: {}, supplementalChoices: choices };
    // Payload alone fits; native controls push the complete compact request one byte over.
    const framed = {
      schemaVersion: 1,
      requestId: scenarioId,
      operationId,
      expectedLibraryRevision: 7,
      previewId: preview.previewId,
      collisionPlan,
    };
    last.key = "x".repeat(64 * 1_048_576 - JSON.stringify(framed).length + 1);
    const apply = flow.applyImport(new generated.LibraryOperationScope(7), {
      previewId: preview.previewId,
      collisionPlan,
    });
    await expect(apply.result).rejects.toBeInstanceOf(RangeError);
    await flow.dispose();
    expect(
      transport.discards.some((entry) => {
        const target = entry.request.target;
        return (
          typeof target === "object" &&
          target !== null &&
          Reflect.get(target, "requestId") === preview.previewId
        );
      }),
    ).toBe(true);
    expect(tauri.invoke.mock.calls.map(([command]) => command)).not.toContain(
      "project_import_apply",
    );
  }, 30_000);

  it("captures approvals before preparation and preserves a native no-change receipt", async () => {
    const prepared = deferred<unknown>();
    const preparing = deferred<Invocation>();
    const entered = deferred<Invocation>();
    let holdPreparation = false;
    portableTransport((command, input) => {
      if (command === "operation_prepare" && holdPreparation) {
        preparing.resolve(input);
        return prepared.promise;
      }
      if (command === "project_import_apply") {
        entered.resolve(input);
        return Promise.resolve(
          response(
            input,
            {
              schemaVersion: 1,
              libraryRevision: 7,
              scenarioIds: [],
              safetyBackup: { kind: "notRequired" },
            },
            7,
          ),
        );
      }
      return undefined;
    });
    const flow = new generated.PortableReviewFlow();
    const preview = (
      await flow.previewImport(new generated.LibraryOperationScope(null), portableOptions).result
    ).result;
    holdPreparation = true;
    const plan = {
      scenarios: { [scenarioId]: "skip" as generated.CollisionAction },
      supplementalChoices: [],
    };
    const draft = { previewId: preview.previewId, collisionPlan: plan };
    Object.assign(draft, { schemaVersion: 99, expectedLibraryRevision: 900, scenarioId });
    const apply = flow.applyImport(new generated.LibraryOperationScope(7), draft);
    draft.previewId = operationId;
    plan.scenarios[scenarioId] = "replace";
    prepared.resolve(response(await preparing.promise, { schemaVersion: 1, operationId }));
    const invocation = await entered.promise;
    expect(invocation.request).toEqual({
      schemaVersion: 1,
      requestId: apply.requestId,
      operationId,
      expectedLibraryRevision: 7,
      previewId: preview.previewId,
      collisionPlan: {
        scenarios: { [scenarioId]: "skip" },
        supplementalChoices: [],
      },
    });
    expect((await apply.result).result.libraryRevision).toBe(7);
    await flow.dispose();
  });

  it("rejects an oversized encoded envelope without loosening compact portable metadata limits", async () => {
    const chunk = "x".repeat(1_048_576);
    const transport = portableTransport((command, input) => {
      if (command === "project_import_preview")
        return Promise.resolve({
          ...response(input, portablePreview(input.request.requestId), 7),
          warnings: Array.from({ length: 129 }, () => ({
            code: "portable.warning",
            severity: "warning",
            message: chunk,
            fieldPath: null,
            resource: null,
          })),
        });
      return undefined;
    });
    const flow = new generated.PortableReviewFlow();
    await expect(
      flow.previewImport(new generated.LibraryOperationScope(null), portableOptions).result,
    ).rejects.toMatchObject(invalidResponse);
    expect(transport.discards).toHaveLength(1);
    await flow.dispose();
  }, 30_000);

  it("keeps creator identities available to retry a failed final disposal cleanup", async () => {
    let cleanupUnavailable = true;
    const transport = portableTransport((command) =>
      command === "project_operation_cancel" && cleanupUnavailable
        ? // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors -- Native cleanup failure uses the same closed error DTO.
          Promise.reject(nativeError)
        : undefined,
    );
    const flow = new generated.PortableReviewFlow();
    const creator = flow.previewImport(new generated.LibraryOperationScope(null), portableOptions);
    await creator.result;
    await expect(flow.dispose()).rejects.toEqual(nativeError);
    cleanupUnavailable = false;
    await flow.dispose();
    expect(transport.discards.at(-1)?.request.target).toEqual({
      kind: "creator",
      operationId: await creator.operationId,
      requestId: creator.requestId,
    });
    expect(() =>
      flow.previewImport(new generated.LibraryOperationScope(null), portableOptions),
    ).toThrow();
  });
});

describe("bounded revision-bound history metadata", () => {
  const request: generated.HistoryPageRequestV1 = {
    schemaVersion: 1,
    scenarioId,
    expectedRevision: 3,
    limit: 1,
    continuation: null,
  };
  const entry: generated.HistoryEntrySummaryDtoV1 = {
    id: operationId,
    revisionBefore: 0,
    revisionAfter: 1,
    source: "desktop",
    summary: "é".repeat(2048),
    createdAt: "2026-09-01T00:00:00Z",
    historySequence: 1,
    branchGeneration: 0,
    applied: true,
  };
  const page: generated.HistoryPageDtoV1 = {
    schemaVersion: 1,
    scenarioId,
    revision: 3,
    entries: [entry],
    continuation: null,
    undoAvailable: true,
    redoAvailable: false,
  };

  it("preserves a maximum UTF-8 summary but rejects payload leakage and oversized metadata", async () => {
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, page, 3)),
    );
    const accepted = await generated.getScenarioHistoryPage(request);
    expect(accepted.result.entries[0]?.summary).toBe(entry.summary);
    for (const invalid of [
      { ...page, schemaVersion: 2 },
      { ...page, entries: [{ ...entry, summary: "é".repeat(2049) }] },
      { ...page, entries: [{ ...entry, command: { privatePayload: true } }] },
      { ...page, entries: [entry, { ...entry, id: scenarioId }] },
    ]) {
      tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
        Promise.resolve(response(input, invalid, 3)),
      );
      await expect(generated.getScenarioHistoryPage(request)).rejects.toMatchObject(
        invalidResponse,
      );
    }
  });

  it("rejects pagination outside its captured request and response context", async () => {
    expect(() => generated.getScenarioHistoryPage({ ...request, limit: 101 })).toThrow(RangeError);
    expect(() =>
      generated.getScenarioHistoryPage({
        ...request,
        continuation: { scenarioId, revision: 2, beforeSequence: 1 },
      }),
    ).toThrow(RangeError);
    expect(tauri.invoke).not.toHaveBeenCalled();
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(response(input, { ...page, revision: 2 }, 2)),
    );
    await expect(generated.getScenarioHistoryPage(request)).rejects.toMatchObject(invalidResponse);
    tauri.invoke.mockImplementation((_command: string, input: Invocation) =>
      Promise.resolve(
        response(
          input,
          { ...page, continuation: { scenarioId: operationId, revision: 3, beforeSequence: 1 } },
          3,
        ),
      ),
    );
    await expect(generated.getScenarioHistoryPage(request)).rejects.toMatchObject(invalidResponse);
  });
});
