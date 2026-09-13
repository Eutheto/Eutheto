import { webcrypto } from "node:crypto";
import { PiniaColada } from "@pinia/colada";
import { createPinia, disposePinia } from "pinia";
import { createSSRApp, effectScope, shallowRef } from "vue";
import { beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import type {
  ApiErrorDto,
  ApiResponseDto,
  HistoryEntrySummaryDtoV1,
  PeopleCsvDecision,
  PeopleCsvMapping,
} from "./api/generated";
import peopleCsvNative from "./api/fixtures/people-csv-native.json";
import { usePeopleCsvImport } from "./people-csv-import";
import { createProjectHomeController, type ProjectSummary } from "./project-home";
import { fakeApi, project, response } from "./testing/project-home";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  invoke: tauri.invoke,
}));

beforeEach(() => {
  tauri.invoke.mockReset();
  // Registered first, so native globals outlive the controller's onTestFinished cleanup.
  onTestFinished(() => {
    vi.unstubAllGlobals();
  });
});

interface Invocation {
  readonly request: { readonly requestId: string; readonly [key: string]: unknown };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function nativeResponse(input: Invocation, result: unknown, revision: number | null = null) {
  return { ...response(result, [], revision), requestId: input.request.requestId };
}

function applied(input: Invocation) {
  return {
    ...peopleCsvNative.applied,
    outcome: { ...peopleCsvNative.applied.outcome, commandId: input.request.commandId },
  };
}

const storageFailure: ApiErrorDto = {
  category: "storage",
  code: "people_csv.io_error",
  message: "CSV report storage is unavailable.",
  retryable: false,
  fieldErrors: [],
  details: null,
  diagnosticId: null,
};

// Captured native DTOs pass through the real generated guards, flow and SDK Channel.
function nativeCsv(
  override?: (command: string, input: Invocation) => Promise<unknown> | undefined,
) {
  let nextOperation = 0;
  tauri.invoke.mockImplementation((command: string, input: Invocation) => {
    const overridden = override?.(command, input);
    if (overridden !== undefined) return overridden;
    switch (command) {
      case "operation_prepare":
        return Promise.resolve(
          nativeResponse(input, {
            schemaVersion: 1,
            operationId: `01900000-0000-7000-8000-${String(++nextOperation).padStart(12, "0")}`,
          }),
        );
      case "operation_release":
        return Promise.resolve(
          nativeResponse(input, { schemaVersion: 1, acknowledgement: "released" }),
        );
      case "operation_cancel":
        return Promise.resolve(
          nativeResponse(input, { schemaVersion: 1, acknowledgement: "cancellationRequested" }),
        );
      case "people_csv_source_close":
      case "people_csv_preview_discard":
        return Promise.resolve(nativeResponse(input, { schemaVersion: 1 }));
      case "people_csv_source_open":
        return Promise.resolve(nativeResponse(input, peopleCsvNative.sourceOpened));
      case "people_csv_preview":
        return Promise.resolve(nativeResponse(input, peopleCsvNative.preview, 0));
      case "people_csv_apply":
        return Promise.resolve(nativeResponse(input, applied(input), 1));
      case "people_csv_rejected_rows":
        return Promise.resolve(
          nativeResponse(input, { ...peopleCsvNative.rejectedRows, consumed: false }),
        );
      case "people_csv_rejected_rows_save":
        return Promise.resolve(nativeResponse(input, peopleCsvNative.reportSaved));
      default:
        throw new Error(`Unexpected native CSV command: ${command}`);
    }
  });
}

function createConsumer() {
  const current = shallowRef<ProjectSummary>({
    ...project,
    scenarioId: peopleCsvNative.preview.scenarioId,
    revision: peopleCsvNative.preview.revision,
  });
  const api = fakeApi([current.value]);
  api.listProjects.mockImplementation(() => Promise.resolve(response([current.value])));
  let nextCallback = 0;
  vi.stubGlobal("window", {
    crypto: webcrypto,
    __TAURI_INTERNALS__: {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
      transformCallback: () => ++nextCallback,
      unregisterCallback: vi.fn(),
    },
  });
  const app = createSSRApp({ render: () => null });
  const pinia = createPinia();
  app.use(pinia);
  app.use(PiniaColada);
  const rootScope = effectScope();
  const home = app.runWithContext(() => rootScope.run(() => createProjectHomeController(api)));
  if (!home) throw new Error("Expected an active controller scope");
  const childScope = effectScope();
  const importer = childScope.run(() => usePeopleCsvImport(home, () => current.value));
  if (!importer) throw new Error("Expected an active import scope");
  onTestFinished(async () => {
    childScope.stop();
    await home.dispose();
    rootScope.stop();
    disposePinia(pinia);
  });
  return { home, importer, childScope, current, api };
}

function csvInput(): { mapping: PeopleCsvMapping; decisions: readonly PeopleCsvDecision[] } {
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

function nativeCalls(command: string) {
  return tauri.invoke.mock.calls.filter(([name]) => name === command);
}

describe("CSV consumer lifecycle with its root owner", () => {
  it("does not replace row B with late row A or restore a sample after interpretation invalidation", async () => {
    const rowAEntered = deferred<Invocation>();
    const rowA = deferred<unknown>();
    const invalidatedEntered = deferred<Invocation>();
    const invalidated = deferred<unknown>();
    let delayedReads = 0;
    const sample = (record: number, name: string) => ({
      schemaVersion: 1,
      sourceId: peopleCsvNative.sourceOpened.sourceId,
      dialect: "comma",
      record,
      cells: [{ text: name, truncated: false }],
    });
    nativeCsv((command, input) => {
      if (command !== "people_csv_record_sample") return undefined;
      if (input.request.record === 2) {
        if (++delayedReads === 1) {
          rowAEntered.resolve(input);
          return rowA.promise;
        }
        invalidatedEntered.resolve(input);
        return invalidated.promise;
      }
      return Promise.resolve(nativeResponse(input, sample(3, "Row B")));
    });
    const { importer } = createConsumer();
    expect(await importer.choose()).toBe(true);
    const { mapping, decisions } = csvInput();
    expect(await importer.preview(mapping, decisions)).toBe(true);
    const first = importer.inspect("comma", 2);
    const firstInput = await rowAEntered.promise;
    await importer.inspect("comma", 3);
    rowA.resolve(nativeResponse(firstInput, sample(2, "Row A")));
    await first;
    expect(importer.state.sample).toEqual({ status: "ready", sample: sample(3, "Row B") });
    expect(importer.state.approval).not.toBeNull();

    const obsolete = importer.inspect("comma", 2);
    const obsoleteInput = await invalidatedEntered.promise;
    // The route uses both public invalidators when its mapping interpretation changes.
    importer.invalidate();
    importer.invalidateSample();
    invalidated.resolve(nativeResponse(obsoleteInput, sample(2, "Old interpretation")));
    await obsolete;
    expect(importer.state.sample).toEqual({ status: "idle" });
    expect(await importer.apply()).toBeNull();
    expect(nativeCalls("people_csv_apply")).toHaveLength(0);
  });

  it("retains the old report and blocks replacement acquisition until failed cleanup is resolved", async () => {
    let failCleanup = true;
    nativeCsv((command) =>
      command === "people_csv_preview_discard" && failCleanup
        ? Promise.reject(Object.assign(new Error(storageFailure.message), storageFailure))
        : undefined,
    );
    const { importer, home, childScope } = createConsumer();
    expect(await importer.choose()).toBe(true);
    const { mapping, decisions } = csvInput();
    expect(await importer.preview(mapping, decisions)).toBe(true);
    expect(await importer.readReport()).toBe(true);
    const retained = importer.state.report;
    expect(await importer.preview(mapping, decisions)).toBe(false);
    expect(importer.state.report).toBe(retained);
    expect(importer.state.reportId).toBe(peopleCsvNative.preview.previewId);
    expect(importer.state.preview).toBeNull();
    expect(await importer.apply()).toBeNull();
    expect(nativeCalls("people_csv_preview")).toHaveLength(1);
    expect(nativeCalls("people_csv_apply")).toHaveLength(0);
    expect(await importer.readReport()).toBe(true);
    expect(importer.state.report?.rejectedRows).toEqual([{ record: 3, code: "missingName" }]);

    childScope.stop();
    await expect.poll(() => home.state.reviewCleanupError).not.toBeNull();
    failCleanup = false;
    await home.retryReviewCleanup();
    expect(home.state.reviewCleanupError).toBeNull();
    expect(nativeCalls("people_csv_preview")).toHaveLength(1);
  });

  it("keeps a lost committed apply uncertain without replay and reconciles its history after child disposal", async () => {
    const entered = deferred<Invocation>();
    const terminal = deferred<unknown>();
    const durableHistory: HistoryEntrySummaryDtoV1[] = [];
    nativeCsv((command, input) => {
      if (command !== "people_csv_apply") return undefined;
      const commandId = input.request.commandId;
      if (typeof commandId !== "string") throw new Error("Expected a command identity");
      // The native boundary committed once; only delivery of its result remains pending.
      durableHistory.push({
        id: commandId,
        revisionBefore: 0,
        revisionAfter: 1,
        source: "import",
        summary: "Import people",
        createdAt: project.updatedAt,
        historySequence: 1,
        branchGeneration: 0,
        applied: true,
      });
      entered.resolve(input);
      return terminal.promise;
    });
    const { importer, home, childScope, current, api } = createConsumer();
    expect(await importer.choose()).toBe(true);
    const { mapping, decisions } = csvInput();
    expect(await importer.preview(mapping, decisions)).toBe(true);
    const applying = importer.apply();
    const dispatch = await entered.promise;
    terminal.reject("Native commit completed but its IPC response was lost");
    expect(await applying).toBeNull();
    expect(importer.state.sourceState).toBe("uncertain");
    expect(importer.state.approval).toBeNull();
    expect(home.state.mutation).toMatchObject({
      outcome: "outcomeUnknown",
      commandId: dispatch.request.commandId,
      scenarioId: current.value.scenarioId,
      expectedRevision: 0,
    });
    expect(await importer.apply()).toBeNull();
    expect(await importer.preview(mapping, decisions)).toBe(false);
    expect(durableHistory).toHaveLength(1);

    childScope.stop();
    current.value = { ...current.value, revision: 1 };
    api.getScenarioHistoryPage.mockResolvedValue(
      response({
        schemaVersion: 1,
        scenarioId: current.value.scenarioId,
        revision: 1,
        entries: durableHistory,
        continuation: null,
        undoAvailable: true,
        redoAvailable: false,
      }),
    );
    await home.reconcileMutation();
    expect(home.state.mutation).toMatchObject({
      commandId: durableHistory[0]?.id,
      outcome: "applied",
      revision: 1,
      history: { kind: "found", applied: true },
    });
    expect(nativeCalls("people_csv_apply")).toHaveLength(1);
  });

  it("publishes a received receipt before refresh and keeps report I/O failures separate after late cancellation and revision change", async () => {
    const entered = deferred<Invocation>();
    const terminal = deferred<unknown>();
    const refreshing = deferred<undefined>();
    const refresh = deferred<ApiResponseDto<ProjectSummary[]>>();
    let failReport = true;
    nativeCsv((command, input) => {
      if (command === "people_csv_apply") {
        entered.resolve(input);
        return terminal.promise;
      }
      if (command === "people_csv_rejected_rows" || command === "people_csv_rejected_rows_save") {
        if (failReport)
          return Promise.reject(Object.assign(new Error(storageFailure.message), storageFailure));
        if (command === "people_csv_rejected_rows")
          return Promise.resolve(nativeResponse(input, peopleCsvNative.rejectedRows));
      }
      return undefined;
    });
    const { importer, home, current, api } = createConsumer();
    expect(await importer.choose()).toBe(true);
    const { mapping, decisions } = csvInput();
    expect(await importer.preview(mapping, decisions)).toBe(true);
    api.listProjects.mockImplementationOnce(() => {
      refreshing.resolve(undefined);
      return refresh.promise;
    });
    const applying = importer.apply();
    const dispatch = await entered.promise;
    await home.cancelOperation();
    current.value = { ...current.value, revision: 1 };
    terminal.resolve(nativeResponse(dispatch, applied(dispatch), 1));
    await refreshing.promise;
    expect(home.state.mutation?.outcome).toBe("applied");
    expect(importer.state.sourceState).toBe("consumed");
    expect(importer.state.report).toEqual(peopleCsvNative.applied.report);
    expect(nativeCalls("people_csv_rejected_rows")).toHaveLength(0);
    refresh.resolve(response([current.value]));
    expect(await applying).toEqual(applied(dispatch));

    expect(await importer.readReport()).toBe(false);
    expect(await importer.saveReport()).toBe(false);
    expect(importer.state.report).toEqual(peopleCsvNative.applied.report);
    expect(importer.state.reportId).toBe(peopleCsvNative.preview.previewId);
    expect(home.state.mutation?.outcome).toBe("applied");
    failReport = false;
    expect(await importer.readReport()).toBe(true);
    expect(await importer.saveReport()).toBe(true);
    expect(await importer.apply()).toBeNull();
    expect(nativeCalls("people_csv_apply")).toHaveLength(1);
  });

  it("settles an in-flight receipt at the root even after cancellation, scenario replacement and route disposal", async () => {
    const entered = deferred<Invocation>();
    const terminal = deferred<unknown>();
    nativeCsv((command, input) => {
      if (command !== "people_csv_apply") return undefined;
      entered.resolve(input);
      return terminal.promise;
    });
    const { importer, home, current, childScope } = createConsumer();
    expect(await importer.choose()).toBe(true);
    const { mapping, decisions } = csvInput();
    expect(await importer.preview(mapping, decisions)).toBe(true);
    const applying = importer.apply();
    const dispatch = await entered.promise;
    await home.cancelOperation();
    current.value = project;
    childScope.stop();
    terminal.resolve(nativeResponse(dispatch, applied(dispatch), 1));
    expect(await applying).toEqual(applied(dispatch));
    expect(home.state.mutation).toMatchObject({
      scenarioId: peopleCsvNative.preview.scenarioId,
      commandId: dispatch.request.commandId,
      outcome: "applied",
      revision: 1,
    });
    expect(home.state.errorMessage).toBeNull();
    expect(importer.state.report).toBeNull();
    expect(await importer.apply()).toBeNull();
    expect(nativeCalls("people_csv_apply")).toHaveLength(1);
  });

  it.each([false, true])(
    "restores exact redo authority only while its review remains current (invalidated: %s)",
    async (invalidate) => {
      const entered = deferred<Invocation>();
      const terminal = deferred<unknown>();
      let firstApply = true;
      nativeCsv((command, input) => {
        if (command !== "people_csv_apply" || !firstApply) return undefined;
        firstApply = false;
        entered.resolve(input);
        return terminal.promise;
      });
      const { importer, home, current } = createConsumer();
      expect(await importer.choose()).toBe(true);
      const { mapping, decisions } = csvInput();
      expect(await importer.preview(mapping, decisions)).toBe(true);
      const applying = importer.apply();
      const dispatch = await entered.promise;
      if (invalidate) current.value = { ...current.value, revision: 1 };
      terminal.reject({
        ...storageFailure,
        category: "validation",
        code: "history.redo_branch_requires_truncation",
        fieldErrors: [
          {
            field: "/truncateRedo",
            code: "history.redo_branch_requires_truncation",
            message: "Confirmation required",
          },
        ],
      });
      expect(await applying).toBeNull();
      expect(home.state.mutation).toBeNull();
      expect(importer.state.redoRequired).toBe(!invalidate);
      expect(await importer.apply()).toBeNull();
      if (invalidate) {
        expect(importer.state.approval).toBeNull();
        expect(await importer.apply(true)).toBeNull();
        expect(nativeCalls("people_csv_apply")).toHaveLength(1);
      } else {
        const receipt = await importer.apply(true);
        expect(receipt?.outcome).toEqual({
          kind: "applied",
          revision: 1,
          commandId: dispatch.request.commandId,
        });
        expect(home.state.mutation?.outcome).toBe("applied");
        expect(nativeCalls("people_csv_apply")).toHaveLength(2);
      }
    },
  );
});
