import { flushPromises } from "@vue/test-utils";
import { compileScript, parse } from "@vue/compiler-sfc";
import { compile, createRenderer, h, nextTick, shallowReactive, ssrContextKey } from "vue";
import { beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import * as api from "../api/generated";
import type {
  ApiResponseDto,
  CounterfactualJobListItemV1,
  CounterfactualJobRecordV1,
  CounterfactualProgressEvent,
  RunInput,
  RunManifest,
  SolutionGetCounterfactualDtoV1,
} from "../api/generated";
import {
  WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
  type WorkforceResultScheduleAssignmentRow,
} from "../api/generated-domain-pack-contracts";
import AcceptedCounterfactual from "./AcceptedCounterfactual.vue";
import source from "./AcceptedCounterfactual.vue?raw";

vi.mock("../api/generated", { spy: true });

// The Node suite imports SSR setup; compile the same template for the public client renderer.
const { descriptor } = parse(source);
if (descriptor.template === null) throw new Error("Expected the counterfactual template");
const bindings = compileScript(descriptor, { id: "counterfactual-selection-test" }).bindings;
if (bindings === undefined) throw new Error("Expected the counterfactual setup bindings");
const component = Object.assign({}, AcceptedCounterfactual, {
  render: compile(descriptor.template.content, {
    prefixIdentifiers: true,
    bindingMetadata: bindings,
  }),
});

const scenarioId = "01900000-0000-7000-8000-000000000001";
const solutionId = "01900000-0000-7000-8000-000000000002";
const jobA = "01900000-0000-7000-8000-000000000003";
const jobB = "01900000-0000-7000-8000-000000000004";
const revision = 7;
const timestamp = "2026-10-23T12:00:00Z";
const base = { solutionId, resultChecksum: "base-result-checksum" };
const input: RunInput = {
  schemaVersion: 1,
  runId: "01900000-0000-7000-8000-000000000005",
  requestId: "01900000-0000-7000-8000-000000000006",
  requestHash: "request-hash",
  scenarioId,
  scenarioRevision: revision,
  snapshotId: "01900000-0000-7000-8000-000000000007",
  snapshotDocumentHash: "snapshot-hash",
  snapshotCreatedAt: timestamp,
  packId: "official.workforce",
  packSchemaVersion: 1,
  planningIrSchemaVersion: 2,
  compilerVersion: "test",
  applicationVersion: "test",
  backendId: "synthetic.test",
  backendVersion: "test",
  adapterVersion: "test",
  workerVersion: "test",
  solverVersion: "test",
  protocolMajor: 1,
  protocolMinor: 0,
  modelHash: "base-model-hash",
  objectivePolicyHash: "policy-hash",
  scenarioTimezone: "UTC",
  temporaryConditionHash: null,
  checksum: "base-input-checksum",
  solveOptions: {
    backend: { kind: "specific", backendId: "synthetic.test" },
    mode: "balanced",
    timeLimitMilliseconds: 30_000,
    memoryLimitBytes: null,
    workerThreads: { kind: "exact", count: 1 },
    randomSeed: "7",
    solutionLimit: null,
    stopAfterFirstFeasible: false,
    collectIntermediateSolutions: false,
    explanationMode: "standard",
    preserveExisting: "none",
    reproducibility: "deterministic",
    resourceLimits: { maxEntities: 100, maxVariables: 100, maxConstraints: 100, maxRules: 100 },
  },
};
function manifest(run: RunInput): RunManifest {
  return {
    schemaVersion: 1,
    runId: run.runId,
    runInputChecksum: run.checksum,
    outcome: { type: "noResult", status: "noSolutionWithinLimit" },
    startedAt: timestamp,
    finishedAt: timestamp,
    elapsedMilliseconds: 0,
    firstIncumbentMilliseconds: null,
    firstVerifiedFeasibleMilliseconds: null,
    phaseTimings: {
      compileMilliseconds: null,
      backendMilliseconds: null,
      projectionMilliseconds: null,
      structuralValidationMilliseconds: null,
      scoreRecomputationMilliseconds: null,
      requiredRuleVerificationMilliseconds: null,
      evidencePersistenceMilliseconds: null,
      optionalExplanationMilliseconds: null,
    },
    verificationWarnings: [],
    checksum: `${run.checksum}-manifest`,
  };
}
function record(
  jobId: string,
  state: CounterfactualJobRecordV1["state"],
): CounterfactualJobRecordV1 {
  const condition = {
    schemaVersion: 1,
    condition: {
      type: "forbidAssignmentValue" as const,
      assignmentId: `assignment-${jobId}`,
      value: { type: "boolean" as const, value: true },
    },
    checksum: `${jobId}-condition`,
  };
  const request: CounterfactualJobRecordV1["request"] = {
    schemaVersion: 1,
    jobId,
    requestId: jobId,
    semantics: {
      schemaVersion: 1,
      scenarioId,
      scenarioRevision: revision,
      snapshotId: input.snapshotId,
      snapshotDocumentHash: input.snapshotDocumentHash,
      base,
      baseRunId: input.runId,
      baseRunInputChecksum: input.checksum,
      baseModelHash: input.modelHash,
      objectivePolicyHash: input.objectivePolicyHash,
      conditionChecksum: condition.checksum,
      totalBudgetMilliseconds: 30_000,
    },
    condition,
    requestHash: `${jobId}-request`,
    createdAt: timestamp,
  };
  const derived: RunInput = {
    ...input,
    runId: jobId,
    requestId: jobId,
    requestHash: request.requestHash,
    modelHash: `${jobId}-model`,
    temporaryConditionHash: condition.checksum,
    checksum: `${jobId}-input`,
  };
  return {
    schemaVersion: 1,
    request,
    state,
    startedAt: timestamp,
    finishedAt: state === "running" ? null : timestamp,
    cancelRequestId: null,
    cancelRequestedAt: null,
    error: null,
    result:
      state !== "completed"
        ? null
        : {
            schemaVersion: 1,
            request,
            baseRunInput: input,
            baseRunManifest: {
              ...manifest(input),
              outcome: {
                type: "accepted",
                status: "feasible",
                solutionId,
                acceptedResultChecksum: base.resultChecksum,
                verificationChecksum: "base-verification-checksum",
              },
            },
            compilation: {
              schemaVersion: 1,
              baseModelHash: input.modelHash,
              conditionChecksum: condition.checksum,
              derivedModelHash: derived.modelHash,
              objectivePolicyHash: input.objectivePolicyHash,
              checksum: `${jobId}-compilation`,
            },
            runInput: derived,
            runManifest: manifest(derived),
            conclusion: { type: "notDistinguishedWithinBudget" },
            checksum: `${jobId}-result`,
          },
  };
}
const runningA = record(jobA, "running");
const completedB = record(jobB, "completed");
function response<T>(result: T): ApiResponseDto<T> {
  return { schemaVersion: 1, requestId: jobA, currentRevision: revision, warnings: [], result };
}
function jobResponse(
  job: CounterfactualJobRecordV1,
): ApiResponseDto<SolutionGetCounterfactualDtoV1> {
  return response({ schemaVersion: 1, scenarioId, currentRevision: revision, job });
}
function listItem(job: CounterfactualJobRecordV1): CounterfactualJobListItemV1 {
  return {
    ...job.request.semantics,
    ...job.request,
    baseSolutionId: solutionId,
    state: job.state,
    finishedAt: job.finishedAt,
    error: job.error,
  };
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

// A minimal Vue host exercises the authored component and its rendered handlers in Node.
// DOM layout, native transport and accessibility behavior remain browser/native checks.
interface HostNode {
  tag: string;
  text: string;
  parent: HostNode | null;
  children: HostNode[];
  props: Record<string, unknown>;
  options: never[];
  addEventListener: () => void;
}
function node(tag: string, text = ""): HostNode {
  return { tag, text, parent: null, children: [], props: {}, options: [], addEventListener() {} };
}
function insert(child: HostNode, parent: HostNode, anchor: HostNode | null = null): void {
  if (child.parent) child.parent.children.splice(child.parent.children.indexOf(child), 1);
  const index = anchor === null ? -1 : parent.children.indexOf(anchor);
  parent.children.splice(index < 0 ? parent.children.length : index, 0, child);
  child.parent = parent;
}
const renderer = createRenderer<HostNode, HostNode>({
  createElement: node,
  createText: (text) => node("#text", text),
  createComment: () => node("#comment"),
  setText: (target, text) => (target.text = text),
  setElementText(target, text) {
    target.children = [];
    target.text = text;
  },
  patchProp: (target, key, _previous, value: unknown) => (target.props[key] = value),
  parentNode: (target) => target.parent,
  nextSibling(target) {
    const siblings = target.parent?.children ?? [];
    return siblings[siblings.indexOf(target) + 1] ?? null;
  },
  insert,
  remove(target) {
    target.parent?.children.splice(target.parent.children.indexOf(target), 1);
    target.parent = null;
  },
  insertStaticContent(content, parent, anchor) {
    const target = node("#static", content);
    insert(target, parent, anchor);
    return [target, target];
  },
});
function text(target: HostNode): string {
  return target.text + target.children.map(text).join("");
}
function find(target: HostNode, predicate: (item: HostNode) => boolean): HostNode | undefined {
  return predicate(target)
    ? target
    : target.children.map((child) => find(child, predicate)).find((item) => item !== undefined);
}
function mount() {
  const assignment: WorkforceResultScheduleAssignmentRow = {
    assignmentId: `assignment-${jobA}`,
    assignmentTypeId: jobA,
    assignmentTypeName: "Day shift",
    endsAt: { instant: timestamp, local: "2026-10-23T12:00:00", offsetSeconds: 0 },
    startsAt: { instant: timestamp, local: "2026-10-23T12:00:00", offsetSeconds: 0 },
    locationId: null,
    locationName: null,
    personId: jobA,
    personName: "Ada",
    shiftId: jobB,
  };
  const props = shallowReactive({
    scenarioId,
    summary: { result: base, scenarioRevision: revision },
    currentRevision: revision,
    assignment,
  });
  const root = node("root");
  const app = renderer.createApp({ render: () => h(component, props) });
  app.provide(ssrContextKey, {});
  app.mount(root);
  let mounted = true;
  const unmount = () => {
    if (mounted) app.unmount();
    mounted = false;
  };
  onTestFinished(unmount);
  return {
    props,
    root,
    unmount,
    excludeIncumbent() {
      const select = find(root, (item) => item.props.name === "counterfactual-question");
      const update = select?.props["onUpdate:modelValue"];
      if (typeof update !== "function") throw new Error("Missing question model handler");
      update("exclude");
    },
    click(label: string) {
      const button = find(root, (item) => item.tag === "button" && text(item).includes(label));
      expect(button, `Rendered button: ${label}`).toBeDefined();
      expect(button?.props.disabled).toBeFalsy();
      const handler = button?.props.onClick;
      if (typeof handler !== "function") throw new Error(`Missing click handler: ${label}`);
      handler();
    },
  };
}

let listener: (event: CounterfactualProgressEvent) => void;
const stop = vi.fn();
function progress(jobId: string, eventRevision = revision): void {
  listener({
    type: "counterfactualProgress",
    payload: {
      context: {
        eventVersion: 1,
        timestamp,
        requestId: jobId,
        scenarioId,
        revision: eventRevision,
        solveRunId: jobId,
      },
      jobId,
      phase: "completed",
    },
  });
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.onCounterfactualProgress).mockImplementation((callback) => {
    listener = callback;
    return Promise.resolve(stop);
  });
  vi.mocked(api.getSolutionView).mockResolvedValue(
    response({
      schemaVersion: 1,
      scenarioId,
      currentRevision: revision,
      scenarioRevision: revision,
      result: base,
      view: {
        viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
        data: {
          schemaVersion: 1,
          timeZone: "UTC",
          result: { kind: "people", items: [], continuation: null },
        },
      },
    }),
  );
  vi.mocked(api.listCounterfactuals).mockResolvedValue(
    response({
      schemaVersion: 1,
      scenarioId,
      currentRevision: revision,
      page: { jobs: [listItem(runningA), listItem(completedB)], nextCursor: null },
    }),
  );
  vi.mocked(api.getCounterfactual).mockImplementation(({ jobId }) =>
    Promise.resolve(jobResponse(jobId === jobA ? runningA : completedB)),
  );
});

describe("AcceptedCounterfactual selection", () => {
  it("keeps a saved B selection when previously displayed A emits progress during B's read", async () => {
    const view = mount();
    await flushPromises();
    view.click(`assignment-${jobA}`);
    await flushPromises();
    expect(text(view.root)).toContain(`Recorded question: Forbid assignment assignment-${jobA}`);
    const pending = deferred<ApiResponseDto<SolutionGetCounterfactualDtoV1>>();
    vi.mocked(api.getCounterfactual).mockReturnValueOnce(pending.promise);
    view.click(`assignment-${jobB}`);
    progress(jobA);
    await flushPromises();
    pending.resolve(jobResponse(completedB));
    await flushPromises();
    expect(text(view.root)).toContain(`Recorded question: Forbid assignment assignment-${jobB}`);
    expect(text(view.root)).toContain("No conclusive answer within the budget");
    expect(api.getCounterfactual).toHaveBeenCalledTimes(2);
  });

  it("refreshes selected B while its first read is pending and rejects the older running response", async () => {
    const view = mount();
    await flushPromises();
    const older = deferred<ApiResponseDto<SolutionGetCounterfactualDtoV1>>();
    const newer = deferred<ApiResponseDto<SolutionGetCounterfactualDtoV1>>();
    vi.mocked(api.getCounterfactual)
      .mockReturnValueOnce(older.promise)
      .mockReturnValueOnce(newer.promise);
    view.click(`assignment-${jobB}`);
    progress(jobB, revision + 1);
    expect(api.getCounterfactual).toHaveBeenCalledTimes(1);
    progress(jobB);
    expect(api.getCounterfactual).toHaveBeenCalledTimes(2);
    newer.resolve(jobResponse(completedB));
    await flushPromises();
    older.resolve(jobResponse(record(jobB, "running")));
    await flushPromises();
    expect(text(view.root)).toContain("completed · requested");
    expect(text(view.root)).toContain("No conclusive answer within the budget");
    vi.mocked(api.getCounterfactual).mockResolvedValueOnce(jobResponse(record(jobB, "running")));
    progress(jobB);
    await flushPromises();
    expect(text(view.root)).toContain("completed · requested");
  });

  it("clears actionable A after B fails and explicitly refreshes the intended B selection", async () => {
    const view = mount();
    await flushPromises();
    view.click(`assignment-${jobA}`);
    await flushPromises();
    expect(text(view.root)).toContain("Cancel question");
    vi.mocked(api.getCounterfactual).mockRejectedValueOnce({
      category: "storage",
      code: "read.failed",
      message: "Saved B read failed",
    });
    view.click(`assignment-${jobB}`);
    await flushPromises();
    expect(text(view.root)).toContain("Saved B read failed");
    expect(text(view.root)).not.toContain("Recorded question:");
    expect(text(view.root)).not.toContain("Cancel question");
    view.click("Refresh saved questions");
    await flushPromises();
    expect(api.getCounterfactual).toHaveBeenLastCalledWith({ scenarioId, jobId: jobB });
    expect(text(view.root)).toContain(`Recorded question: Forbid assignment assignment-${jobB}`);
  });

  it("observes the newly started question before its saved-list refresh settles", async () => {
    const view = mount();
    await flushPromises();
    view.excludeIncumbent();
    await nextTick();
    vi.mocked(api.getSolutionView).mockResolvedValueOnce(
      response({
        schemaVersion: 1,
        scenarioId,
        currentRevision: revision,
        scenarioRevision: revision,
        result: base,
        view: {
          viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
          data: {
            schemaVersion: 1,
            timeZone: "UTC",
            result: {
              kind: "decision",
              personId: jobA,
              shiftId: jobB,
              selected: true,
              assignmentId: `assignment-${jobA}`,
            },
          },
        },
      }),
    );
    vi.mocked(api.startCounterfactual).mockResolvedValueOnce(
      response({ schemaVersion: 1, requestId: jobA, currentRevision: revision, job: runningA }),
    );
    const pendingList = deferred<Awaited<ReturnType<typeof api.listCounterfactuals>>>();
    vi.mocked(api.listCounterfactuals).mockReturnValueOnce(pendingList.promise);
    view.click("Check excluded person");
    await flushPromises();
    expect(text(view.root)).toContain(`Recorded question: Forbid assignment assignment-${jobA}`);
    vi.mocked(api.getCounterfactual).mockResolvedValueOnce(jobResponse(record(jobA, "completed")));
    progress(jobA);
    await flushPromises();
    expect(api.getCounterfactual).toHaveBeenCalledOnce();
    expect(text(view.root)).toContain("completed · requested");
    pendingList.resolve(
      response({
        schemaVersion: 1,
        scenarioId,
        currentRevision: revision,
        page: { jobs: [listItem(record(jobA, "completed"))], nextCursor: null },
      }),
    );
    await flushPromises();
    expect(api.getCounterfactual).toHaveBeenLastCalledWith({ scenarioId, jobId: jobA });
    expect(text(view.root)).toContain("completed · requested");
  });

  it("still rejects a saved record with a different accepted-result checksum", async () => {
    const view = mount();
    await flushPromises();
    vi.mocked(api.getCounterfactual).mockResolvedValueOnce(
      jobResponse({
        ...completedB,
        request: {
          ...completedB.request,
          semantics: {
            ...completedB.request.semantics,
            base: { ...base, resultChecksum: "different-source" },
          },
        },
      }),
    );
    view.click(`assignment-${jobB}`);
    await flushPromises();
    expect(text(view.root)).toContain(
      "This saved diagnostic does not belong to the inspected result",
    );
    expect(text(view.root)).not.toContain("Recorded question:");
  });

  it.each(["reset", "unmount"] as const)(
    "ignores late reads and events after %s",
    async (action) => {
      const view = mount();
      await flushPromises();
      const pending = deferred<ApiResponseDto<SolutionGetCounterfactualDtoV1>>();
      vi.mocked(api.getCounterfactual).mockReturnValueOnce(pending.promise);
      view.click(`assignment-${jobB}`);
      if (action === "reset") {
        view.props.currentRevision += 1;
        view.props.summary = {
          result: { ...base, resultChecksum: "new-source-checksum" },
          scenarioRevision: revision + 1,
        };
        await nextTick();
      } else view.unmount();
      const listsBeforeLateEvent = vi.mocked(api.listCounterfactuals).mock.calls.length;
      pending.resolve(jobResponse(completedB));
      progress(jobB);
      await flushPromises();
      expect(text(view.root)).not.toContain("Recorded question:");
      expect(api.getCounterfactual).toHaveBeenCalledTimes(1);
      expect(api.listCounterfactuals).toHaveBeenCalledTimes(listsBeforeLateEvent);
      if (action === "unmount") expect(stop).toHaveBeenCalledOnce();
    },
  );

  it("disposes a listener whose registration settles after unmount", async () => {
    const registration = deferred<() => void>();
    vi.mocked(api.onCounterfactualProgress).mockReturnValueOnce(registration.promise);
    const view = mount();
    view.unmount();
    registration.resolve(stop);
    await flushPromises();
    expect(stop).toHaveBeenCalledOnce();
    expect(api.getCounterfactual).not.toHaveBeenCalled();
  });
});
