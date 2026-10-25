/* eslint-disable vue/one-component-per-file -- Browser routing fixtures mount the real component and route guards through a separate harness. */
import { cleanup, render, screen, within } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { defineComponent, h, nextTick, reactive, shallowRef } from "vue";
import { createMemoryHistory, createRouter, RouterView } from "vue-router";
import * as api from "../api/generated";
import { createProjectHomeController, type ProjectHomeController } from "../project-home";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import { messages } from "../messages";
import WorkforceRepairPanel from "./WorkforceRepairPanel.vue";
import RecordedBaselineDiff from "./RecordedBaselineDiff.vue";
import OptimizeCapabilityPanel from "./OptimizeCapabilityPanel.vue";
import "../styles.css";

vi.mock("../api/generated", { spy: true });
const personId = "01900000-0000-7000-8000-000000000011";
const targetId = "01900000-0000-7000-8000-000000000012";
const shiftId = "01900000-0000-7000-8000-000000000030";
const ruleId = "01900000-0000-7000-8000-000000000040";
const acceptedId = "01900000-0000-7000-8000-000000000050";
const original: api.RunBaselineOriginal = {
  entity: { kind: "baseSchedule", id: acceptedId },
  pairContentHash: "a".repeat(64),
  acceptedResult: { solutionId: acceptedId, resultChecksum: "b".repeat(64) },
  sourceScenarioId: project.scenarioId,
  sourceRevision: project.revision,
  sourceDocumentHash: "c".repeat(64),
  sourceRunId: acceptedId,
  sourceRunInputChecksum: "d".repeat(64),
  sourceRunManifestChecksum: "e".repeat(64),
};
const source: api.ManualEditSourceV1 = {
  scenarioId: project.scenarioId,
  revision: project.revision,
  documentHash: "f".repeat(64),
  original,
  currentManualId: null,
  currentManualChecksum: null,
  selectedCount: 2,
  protectionCount: 1,
  authoredOptimizeUnavailableReason: null,
};
const row: api.ManualEditRowV1 = {
  personId,
  shiftId,
  personLabel: "Alex",
  shiftLabel: "Recurring clinic",
  selected: true,
  lockId: ruleId,
  protection: { kind: "hard" },
  personAvailable: true,
  shiftAvailable: true,
};
const dates = { startDate: "2030-01-01", endDateExclusive: "2030-01-08" };
function receipt(result: api.ManualEditResultV1, revision = project.revision) {
  return response(result, [], revision);
}
function readResult(
  revision = project.revision,
  originalSourceRevision = project.revision,
): api.ManualEditResultV1 {
  return {
    kind: "read",
    read: {
      schemaVersion: 1,
      source: {
        ...source,
        revision,
        original: { ...original, sourceRevision: originalSourceRevision },
      },
      offset: 0,
      totalRows: 1,
      nextOffset: null,
      rows: [row],
    },
  };
}
function reviewResult(
  intent: api.ManualEditIntentV1,
  valid = true,
  originalSourceRevision = project.revision,
): api.ManualEditResultV1 {
  return {
    kind: "review",
    review: {
      schemaVersion: 1,
      source: { ...source, original: { ...original, sourceRevision: originalSourceRevision } },
      intent,
      changes: [{ before: row, after: { ...row, selected: intent.kind !== "remove" } }],
      disposition: valid ? "valid" : "invalid",
      approvalDigest: valid ? "1".repeat(64) : null,
      evaluations: valid
        ? []
        : [1, 2].map((minimum) => ({
            evaluation: {
              ruleId,
              satisfied: false,
              affectedEntities: [{ kind: "shiftInstance", id: shiftId }],
              messageKey: `Required coverage ${String(minimum)}`,
              expected: { minimum: { type: "integer", value: String(minimum) } },
              observed: { selected: { type: "integer", value: "0" } },
              evidence: [`Native failed obligation ${String(minimum)}`],
            },
            ownerPaths: [`/domain/rules/${ruleId}/coverage`],
          })),
    },
  };
}
function applied(failure: api.ApiErrorDto | null = null): api.ManualEditResultV1 {
  return {
    kind: "applied",
    applied: {
      schemaVersion: 1,
      sourceAdmissionFailure: failure,
      commandResult: {
        newRevision: project.revision + 1,
        changeSet: { changes: [] },
        validationDelta: { added: [], resolved: [] },
        inverse: null,
      },
    },
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}
async function setup(valid = true, originalSourceRevision = project.revision) {
  const native = fakeApi([{ ...project, domainPackId: "official.workforce" }]);
  const manual = vi.mocked(api.manualEdit).mockImplementation((scope, input) => {
    if (input.mode === "read")
      return portableOperation(
        receipt(
          readResult(scope.context.expectedRevision ?? project.revision, originalSourceRevision),
          scope.context.expectedRevision ?? project.revision,
        ),
      );
    if (input.mode === "review")
      return portableOperation(receipt(reviewResult(input.intent, valid, originalSourceRevision)));
    return portableOperation(receipt(applied(), project.revision + 1));
  });
  vi.mocked(api.searchScenarioEntities).mockReturnValue(
    portableOperation(
      response(
        {
          schemaVersion: 2,
          scenarioId: project.scenarioId,
          revision: project.revision,
          view: {
            viewId: "eutheto.setup.entity_page",
            data: {
              schemaVersion: 1,
              result: {
                kind: "entityPage",
                data: {
                  continuation: null,
                  totalItems: 2,
                  items: [
                    { kind: "person", entityId: personId, name: "Alex" },
                    { kind: "person", entityId: targetId, name: "Sam" },
                  ],
                },
              },
            },
          },
        },
        [],
        project.revision,
      ),
    ),
  );
  vi.mocked(api.getScenarioEntity).mockImplementation(() =>
    portableOperation(
      Promise.reject(
        Object.assign(new Error("No stored label"), {
          category: "notFound",
          code: "missing.label",
        }),
      ),
    ),
  );
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/history/:scenarioId", name: "project-history", component: { template: "<div />" } },
      { path: "/rules/:scenarioId", name: "project-rules", component: { template: "<div />" } },
    ],
  });
  const visible = reactive({ ...project, domainPackId: "official.workforce" });
  let home!: ProjectHomeController;
  const commits = vi.fn();
  const candidate = shallowRef<api.SolutionSummaryDtoV1 | null>(null);
  const baselines = vi.fn();
  const showing = shallowRef(true);
  const Harness = defineComponent({
    setup() {
      home = createProjectHomeController(native);
      return () =>
        showing.value
          ? h("div", [
              h(WorkforceRepairPanel, {
                home,
                project: visible,
                dates,
                candidate: candidate.value,
                onCommitted: commits,
              }),
              h(RecordedBaselineDiff, {
                home,
                project: visible,
                candidate: candidate.value,
                onRecordedBaseline: baselines,
              }),
            ])
          : null;
    },
  });
  router.addRoute({ path: "/", component: Harness });
  await router.push("/");
  const view = render(defineComponent({ setup: () => () => h(RouterView) }), {
    global: { plugins: [createPinia(), PiniaColada, router] },
  });
  await router.isReady();
  return {
    ...view,
    manual,
    router,
    native,
    visible,
    commits,
    candidate,
    baselines,
    showing,
    get home() {
      return home;
    },
  };
}
async function removeDraft() {
  await userEvent.click(await screen.findByRole("button", { name: "Remove Alex" }));
}
async function reviewDraft() {
  const button = await screen.findByRole("button", { name: messages.repair.review });
  button.focus();
  await userEvent.keyboard("{Enter}");
}
beforeEach(() => vi.clearAllMocks());
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

// These controllable native receipts test DOM ownership and intent only, not native custody or validity.
describe("guided repair native presentation", () => {
  it("leaves an untouched panel while its read-only selection refresh is pending", async () => {
    const state = await setup();
    await screen.findByRole("button", { name: "Remove Alex" });
    const terminal = deferred<api.ApiResponseDto<api.ManualEditResultV1>>();
    state.manual.mockReturnValueOnce(portableOperation(terminal.promise));
    await userEvent.click(screen.getByRole("button", { name: messages.repair.read }));
    const navigation = state.router.push(`/rules/${project.scenarioId}`);
    try {
      await expect.poll(() => state.router.currentRoute.value.name).toBe("project-rules");
      expect(screen.queryByRole("dialog")).toBeNull();
    } finally {
      terminal.resolve(receipt(readResult()));
    }
    await navigation;
  });
  it("treats a missing adopted base as empty, but retains genuine source failures", async () => {
    const state = await setup();
    await screen.findByRole("button", { name: "Remove Alex" });
    state.manual.mockImplementationOnce(() =>
      portableOperation(
        Promise.reject(
          Object.assign(new Error("No adopted original."), {
            category: "validation",
            code: "manual.base_missing",
          }),
        ),
      ),
    );
    await userEvent.click(screen.getByRole("button", { name: messages.repair.read }));
    await expect.element(await screen.findByText(messages.repair.noSource)).toBeVisible();
    expect(screen.queryByRole("alert")).toBeNull();
    await expect
      .element(screen.getByRole("button", { name: messages.repair.review }))
      .toBeDisabled();
    state.manual.mockImplementationOnce(() =>
      portableOperation(
        Promise.reject(
          Object.assign(new Error("The source could not be read."), {
            category: "storage",
            code: "storage.unavailable",
          }),
        ),
      ),
    );
    await userEvent.click(screen.getByRole("button", { name: messages.repair.read }));
    await expect
      .element(await screen.findByRole("alert"))
      .toHaveTextContent("The source could not be read.");
    await expect
      .element(screen.getByRole("button", { name: messages.repair.review }))
      .toBeDisabled();
  });
  it("settles a rejected review without retaining a busy announcement or losing the draft", async () => {
    const state = await setup();
    await removeDraft();
    state.manual.mockImplementationOnce(() =>
      portableOperation(
        Promise.reject(
          Object.assign(new Error("Native review resource limit"), {
            category: "validation",
            code: "command.resource_limit",
          }),
        ),
      ),
    );
    await reviewDraft();
    await expect.element(await screen.findByRole("alert")).toBeVisible();
    await expect
      .element(screen.getByRole("button", { name: messages.repair.review }))
      .toBeEnabled();
    expect(
      document.querySelector('[data-manual-repair-panel] > [role="status"][aria-live="polite"]'),
    ).toBeNull();
    expect(screen.getByRole<HTMLSelectElement>("combobox", { name: "Change" }).value).toBe(
      "remove",
    );
    expect(screen.queryByRole("button", { name: messages.repair.apply })).toBeNull();
    await reviewDraft();
    await expect
      .element(await screen.findByRole("button", { name: messages.repair.apply }))
      .toBeEnabled();
  });
  it("retains invalid intent, shows every native expected/observed finding, and never offers Apply", async () => {
    const state = await setup(false);
    await removeDraft();
    await reviewDraft();
    await expect
      .element(screen.getByRole("heading", { name: "Native manual review — invalid" }))
      .toHaveFocus();
    const review = screen.getByRole("region", {
      name: "Native manual review — invalid",
    });
    expect(review.getBoundingClientRect().right).toBeLessThanOrEqual(
      document.documentElement.clientWidth,
    );
    expect(screen.getAllByText(/^Required coverage/u)).toHaveLength(2);
    expect(screen.getAllByText("Expected")).toHaveLength(2);
    expect(screen.getAllByText("Observed")).toHaveLength(2);
    expect(
      screen.getAllByRole("button", { name: "Edit related native rule or owner" }),
    ).toHaveLength(2);
    expect(screen.queryByRole("button", { name: messages.repair.apply })).toBeNull();
    await userEvent.click(
      screen.getByRole("button", { name: "Check chosen current-result replacement diagnostic" }),
    );
    await expect.element(screen.getByText(messages.repair.diagnosticUnavailable)).toBeVisible();
    expect(vi.mocked(api.startCounterfactual)).not.toHaveBeenCalled();
    expect(screen.getByRole<HTMLSelectElement>("combobox", { name: "Change" }).value).toBe(
      "remove",
    );
    expect(state.manual.mock.calls.filter(([, input]) => input.mode === "apply")).toHaveLength(0);
    await userEvent.keyboard("{Escape}");
    await expect
      .element(screen.getByRole("heading", { name: messages.repair.heading }))
      .toHaveFocus();
  });
  it("retains incomplete raw protection intent across alternative-editor remount without restoring approval", async () => {
    const state = await setup();
    await userEvent.click(
      await screen.findByRole("button", { name: "Change protection for Alex" }),
    );
    await userEvent.selectOptions(
      screen.getByRole("combobox", { name: "Protection transition" }),
      "soft",
    );
    await userEvent.fill(screen.getByRole("textbox", { name: "Soft stability weight" }), "2e");
    state.showing.value = false;
    await nextTick();
    state.showing.value = true;
    await expect
      .element(await screen.findByRole("textbox", { name: "Soft stability weight" }))
      .toHaveValue("2e");
    expect(screen.queryByRole("button", { name: messages.repair.apply })).toBeNull();
    await reviewDraft();
    await expect.element(await screen.findByText(/Enter a Soft stability weight/u)).toBeVisible();
    expect(state.manual.mock.calls.filter(([, input]) => input.mode === "review")).toHaveLength(0);
  });

  it("does not publish a delayed approval after a source/library fence", async () => {
    const state = await setup();
    await removeDraft();
    const pending = deferred<api.ApiResponseDto<api.ManualEditResultV1>>();
    let captured!: api.ManualEditIntentV1;
    state.manual.mockImplementationOnce((_scope, input) => {
      if (input.mode !== "review") throw new Error("Expected review");
      captured = input.intent;
      return portableOperation(pending.promise);
    });
    await reviewDraft();
    state.home.state.libraryEpoch += 1;
    pending.resolve(receipt(reviewResult(captured)));
    await expect
      .poll(() => screen.queryByRole("button", { name: messages.repair.apply }))
      .toBeNull();
    await expect.element(screen.getByRole("combobox", { name: "Change" })).toHaveValue("remove");
  });

  it("cancels the same owned review, releases it separately, and retains the raw draft", async () => {
    const state = await setup();
    await removeDraft();
    const pending = deferred<api.ApiResponseDto<api.ManualEditResultV1>>();
    const operation = portableOperation(pending.promise),
      cancel = vi.spyOn(operation, "cancel"),
      release = vi.spyOn(operation, "release");
    let captured!: api.ManualEditIntentV1;
    state.manual.mockImplementationOnce((_scope, input) => {
      if (input.mode !== "review") throw new Error("Expected review");
      captured = input.intent;
      return operation;
    });
    await reviewDraft();
    await userEvent.click(screen.getByRole("button", { name: messages.repair.cancelOperation }));
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(release).not.toHaveBeenCalled();
    pending.resolve(receipt(reviewResult(captured)));
    await expect.poll(() => release.mock.calls.length).toBe(1);
    expect(screen.queryByRole("button", { name: messages.repair.apply })).toBeNull();
    await expect.element(screen.getByRole("combobox", { name: "Change" })).toHaveValue("remove");
    await expect
      .element(screen.getByRole("button", { name: messages.repair.review }))
      .toHaveFocus();
  });

  it("captures exact same-shift replacement, explicit incumbent unlock and target Soft weight", async () => {
    const state = await setup();
    await userEvent.click(
      await screen.findByRole("button", { name: "Replace Alex on this shift" }),
    );
    vi.mocked(api.getScenarioView).mockReturnValueOnce(
      portableOperation(
        response<api.ScenarioSetupViewResultV2<"official.workforce.setup.work_window">>(
          {
            schemaVersion: 2,
            scenarioId: project.scenarioId,
            revision: project.revision,
            view: {
              viewId: "official.workforce.setup.work_window",
              data: {
                schemaVersion: 1,
                result: {
                  kind: "workWindow",
                  data: {
                    continuation: null,
                    totalItems: 1,
                    items: [
                      {
                        shiftId,
                        assignmentTypeId: ruleId,
                        assignmentTypeName: "Clinic",
                        templateName: "Recurring clinic",
                        coverage: {
                          minimum: 1,
                          maximum: null,
                          preferred: null,
                          qualificationMinimumCount: 0,
                        },
                        elapsed: { seconds: "3600", nanoseconds: 0 },
                        scheduled: { seconds: "3600", nanoseconds: 0 },
                        interval: {
                          startsAt: {
                            instant: "2030-01-01T09:00:00Z",
                            local: "2030-01-01T09:00:00",
                            offsetSeconds: 0,
                          },
                          endsAt: {
                            instant: "2030-01-01T10:00:00Z",
                            local: "2030-01-01T10:00:00",
                            offsetSeconds: 0,
                          },
                        },
                        locationId: null,
                        locationName: null,
                        reportingDate: "2030-01-01",
                        origin: {
                          kind: "generated",
                          templateId: ruleId,
                          occurrenceDate: "2030-01-01",
                        },
                      },
                    ],
                  },
                },
              },
            },
          },
          [],
          project.revision,
        ),
      ),
    );
    await userEvent.click(screen.getByRole("button", { name: "Read native work window" }));
    await userEvent.click(
      await screen.findByRole("button", {
        name: "Choose Clinic · 2030-01-01T09:00:00 – 2030-01-01T10:00:00",
      }),
    );
    await userEvent.click(screen.getByRole("combobox", { name: "Search Replacement person" }));
    await userEvent.click(await screen.findByRole("option", { name: /Sam/u }));
    await userEvent.click(
      screen.getByRole("checkbox", {
        name: "Explicitly unlock the incumbent in this same command",
      }),
    );
    await userEvent.selectOptions(
      screen.getByRole("combobox", { name: "Protection transition" }),
      "soft",
    );
    await userEvent.fill(screen.getByRole("textbox", { name: "Soft stability weight" }), "7");
    await reviewDraft();
    const input = state.manual.mock.calls.find(([, input]) => input.mode === "review")?.[1];
    expect(input).toEqual({
      mode: "review",
      intent: {
        kind: "replace",
        incumbentPersonId: personId,
        targetPersonId: targetId,
        shiftId,
        protectionChanges: [
          { personId, shiftId, protection: { kind: "unlocked" } },
          { personId: targetId, shiftId, protection: { kind: "soft", stabilityWeight: 7 } },
        ],
      },
    });
    expect(
      vi
        .mocked(api.getScenarioEntity)
        .mock.calls.every(([, input]) => input.kind !== "shiftInstance"),
    ).toBe(true);
  });

  it("keeps a committed receipt plus categorized admission failure and never retries it as a failed mutation", async () => {
    const state = await setup();
    await removeDraft();
    await reviewDraft();
    const failure: api.ApiErrorDto = {
      category: "verification",
      code: "manual.source_unavailable",
      message: "Source admission refused",
      retryable: false,
      fieldErrors: [],
      details: null,
      diagnosticId: null,
    };
    state.manual.mockImplementationOnce(() =>
      portableOperation(receipt(applied(failure), project.revision + 1)),
    );
    await userEvent.click(await screen.findByRole("button", { name: messages.repair.apply }));
    await expect.poll(() => state.home.state.mutation?.outcome).toBe("applied");
    await expect.element(screen.getByText(/Committed command.*new revision 4/u)).toBeVisible();
    await expect
      .element(screen.getByText(/verification \/ manual.source_unavailable/u))
      .toBeVisible();
    expect(state.commits).toHaveBeenCalledWith(
      expect.objectContaining({ revision: 4, alarm: expect.stringContaining("verification") }),
    );
    expect(state.manual.mock.calls.filter(([, input]) => input.mode === "apply")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: messages.repair.apply })).toBeNull();
  });

  it("retains the settled mutation but does not publish its retired-library receipt or clear the draft", async () => {
    const state = await setup();
    await removeDraft();
    await reviewDraft();
    const pending = deferred<api.ApiResponseDto<api.ManualEditResultV1>>();
    state.manual.mockImplementationOnce(() => portableOperation(pending.promise));
    await userEvent.click(await screen.findByRole("button", { name: messages.repair.apply }));
    state.home.state.libraryEpoch += 1;
    await nextTick();
    pending.resolve(receipt(applied(), project.revision + 1));
    await expect.poll(() => state.home.state.mutation?.outcome).toBe("applied");
    expect(state.commits).not.toHaveBeenCalled();
    expect(screen.queryByText(/Committed command/u)).toBeNull();
    await expect.element(screen.getByRole("combobox", { name: "Change" })).toHaveValue("remove");
  });

  it("blocks fresh Apply after unknown transport outcome and asks for exact command History reconciliation", async () => {
    const state = await setup();
    await removeDraft();
    await reviewDraft();
    state.manual.mockImplementationOnce(() =>
      portableOperation(
        Promise.reject(
          Object.assign(new Error("Response lost"), {
            category: "protocol",
            code: "transport.lost",
          }),
        ),
      ),
    );
    await userEvent.click(await screen.findByRole("button", { name: messages.repair.apply }));
    await expect.poll(() => state.home.state.mutation?.outcome).toBe("outcomeUnknown");
    const command = state.manual.mock.calls.find(([, input]) => input.mode === "apply")?.[1];
    if (command?.mode !== "apply") throw new Error("No apply identity");
    expect(state.home.state.mutation?.commandId).toBe(command.commandId);
    expect(screen.queryByRole("button", { name: messages.repair.apply })).toBeNull();
    await expect
      .element(screen.getByRole("button", { name: "Reconcile exact command in History" }))
      .toBeVisible();
    await expect
      .element(screen.getByRole("button", { name: messages.repair.review }))
      .toBeDisabled();
    expect(state.manual.mock.calls.filter(([, input]) => input.mode === "apply")).toHaveLength(1);
    // Dismissing a root transport observer cannot restore manual Apply authority.
    state.home.acknowledgeMutation();
    await expect
      .element(screen.getByRole("button", { name: messages.repair.review }))
      .toBeDisabled();
    state.visible.revision = 4;
    vi.mocked(api.getScenarioHistoryPage).mockResolvedValue(
      response(
        {
          schemaVersion: 1,
          scenarioId: project.scenarioId,
          revision: 4,
          continuation: null,
          undoAvailable: true,
          redoAvailable: false,
          entries: [
            {
              id: command.commandId,
              revisionBefore: 3,
              revisionAfter: 4,
              source: "desktop",
              summary: "Manual change",
              createdAt: "2030-01-01T00:00:00Z",
              historySequence: 1,
              branchGeneration: 0,
              applied: true,
            },
          ],
        },
        [],
        4,
      ),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Reconcile exact command in History" }),
    );
    await expect
      .poll(() => screen.queryByRole("button", { name: "Reconcile exact command in History" }))
      .toBeNull();
    expect(vi.mocked(api.getScenarioHistoryPage)).toHaveBeenCalledWith(
      expect.objectContaining({ scenarioId: project.scenarioId, expectedRevision: 4, limit: 50 }),
    );
  });

  it("requires separate explicit redo consent with the same reviewed digest and intent", async () => {
    const state = await setup();
    await removeDraft();
    await reviewDraft();
    const redoError = {
      category: "validation",
      code: "history.redo_branch_requires_truncation",
      message: "Redo branch requires explicit consent",
      retryable: false,
      details: null,
      fieldErrors: [
        {
          field: "/truncateRedo",
          code: "history.redo_branch_requires_truncation",
          message: "Explicit consent required",
        },
      ],
    };
    state.manual.mockImplementationOnce(() =>
      portableOperation(Promise.reject(Object.assign(new Error(redoError.message), redoError))),
    );
    await userEvent.click(await screen.findByRole("button", { name: messages.repair.apply }));
    await userEvent.click(await screen.findByRole("button", { name: messages.repair.redo }));
    const writes = state.manual.mock.calls.flatMap(([, input]) =>
      input.mode === "apply" ? [input] : [],
    );
    expect(writes).toHaveLength(2);
    expect(writes[0]?.truncateRedo).toBe(false);
    expect(writes[1]?.truncateRedo).toBe(true);
    expect(writes[1]?.approvalDigest).toBe(writes[0]?.approvalDigest);
    expect(writes[1]?.intent).toEqual(writes[0]?.intent);
  });
});

it("opens a usable current chosen C diagnostic after original A becomes historical, but refuses a stale chosen C", async () => {
  const originalRevision = project.revision - 1;
  const state = await setup(false, originalRevision);
  const c: api.SolutionSummaryDtoV1 = {
    schemaVersion: 1,
    scenarioId: project.scenarioId,
    scenarioRevision: project.revision,
    currentRevision: project.revision,
    result: { solutionId: targetId, resultChecksum: "2".repeat(64) },
    runId: targetId,
    stale: false,
    selected: false,
    status: "feasible",
    score: { feasibility: "0", levels: [] },
    verificationChecksum: "3".repeat(64),
    finishedAt: "2030-01-01T10:00:00Z",
  };
  let assignmentPage = 0;
  let incumbentBeyondLimit = false;
  let holdPeople = true;
  const peopleReady = Promise.withResolvers<undefined>();
  vi.mocked(api.getSolutionView).mockImplementation(async (input) => {
    let result: api.JsonValue;
    switch (input.parameters?.kind) {
      case "decision":
        result = {
          kind: "decision",
          personId: input.parameters.personId ?? "",
          shiftId,
          selected: input.parameters.personId === personId,
          assignmentId:
            input.parameters.personId === personId
              ? "native-incumbent-decision"
              : "native-target-decision",
        };
        break;
      case "assignments": {
        if (input.parameters.personId !== undefined && input.parameters.shiftId !== undefined)
          throw new Error("Native assignment views forbid multiple source filters");
        assignmentPage += 1;
        const found = input.continuation != null && !incumbentBeyondLimit;
        const incumbentRow = {
          assignmentId: "native-incumbent-decision",
          personId,
          personName: "Alex",
          shiftId,
          assignmentTypeId: ruleId,
          assignmentTypeName: "Clinic",
          locationId: null,
          locationName: null,
          startsAt: {
            instant: "2030-01-01T09:00:00Z",
            local: "2030-01-01T09:00:00",
            offsetSeconds: 0,
          },
          endsAt: {
            instant: "2030-01-01T10:00:00Z",
            local: "2030-01-01T10:00:00",
            offsetSeconds: 0,
          },
        };
        const pageRows = found
          ? [incumbentRow]
          : Array.from({ length: 50 }, (_, index) => ({
              ...incumbentRow,
              assignmentId: `saved-native-assignment-${String(assignmentPage)}-${String(index)}`,
              personId: `01800000-0000-7000-8000-${String((assignmentPage - 1) * 50 + index + 1).padStart(12, "0")}`,
              personName: "Other accepted participant",
            }));
        const lastRow = pageRows.at(-1);
        if (!lastRow) throw new Error("Fixture assignment page must contain its native rows");
        result = {
          kind: "assignments",
          continuation: found
            ? null
            : {
                schemaVersion: 1,
                scenarioId: c.scenarioId,
                revision: c.scenarioRevision,
                queryFingerprint: Array(32).fill(1),
                position: { kind: "assignment", personId: lastRow.personId, shiftId },
              },
          totalItems: incumbentBeyondLimit ? 501 : 51,
          items: pageRows,
        };
        break;
      }
      case "people":
        if (holdPeople) await peopleReady.promise;
        result = {
          kind: "people",
          continuation: null,
          totalItems: 2,
          items: [
            { personId, name: "Alex", selectedCount: 1 },
            { personId: targetId, name: "Sam", selectedCount: 0 },
          ],
        };
        break;
      case "repairDiff":
        result = {
          kind: "repairDiff",
          availability: "absent",
          baseline: null,
          counts: null,
          continuation: null,
          totalItems: null,
          items: [],
          source: {
            scenarioId: c.scenarioId,
            solutionId: c.result.solutionId,
            sourceRevision: c.scenarioRevision,
            sourceDocumentHash: "4".repeat(64),
            normalizedSolutionHash: "5".repeat(64),
          },
        };
        break;
      default:
        throw new Error("Unexpected accepted view in diagnostic presentation test");
    }
    return Promise.resolve(
      response(
        {
          schemaVersion: 1,
          scenarioId: c.scenarioId,
          currentRevision: project.revision,
          scenarioRevision: c.scenarioRevision,
          result: c.result,
          view: {
            viewId: "official.workforce.result.schedule",
            data: { schemaVersion: 1, timeZone: "UTC", result },
          },
        },
        [],
        project.revision,
      ),
    );
  });
  vi.mocked(api.listCounterfactuals).mockResolvedValue(
    response(
      {
        schemaVersion: 1,
        scenarioId: project.scenarioId,
        currentRevision: project.revision,
        page: { jobs: [], nextCursor: null },
      },
      [],
      project.revision,
    ),
  );
  vi.mocked(api.onCounterfactualProgress).mockResolvedValue(vi.fn());
  state.candidate.value = c;
  await userEvent.click(await screen.findByRole("button", { name: "Replace Alex on this shift" }));
  await userEvent.click(screen.getByRole("combobox", { name: "Search Replacement person" }));
  await userEvent.click(await screen.findByRole("option", { name: /Sam/u }));
  await userEvent.selectOptions(
    screen.getByRole("combobox", { name: "Protection transition" }),
    "soft",
  );
  await userEvent.fill(screen.getByRole("textbox", { name: "Soft stability weight" }), "9");
  await reviewDraft();
  await userEvent.click(
    await screen.findByRole("button", {
      name: "Check chosen current-result replacement diagnostic",
    }),
  );
  await expect
    .element(
      await screen.findByRole("heading", {
        name: "Current accepted-source global diagnostic — other assignments free",
      }),
    )
    .toBeVisible();
  await expect.element(screen.getByRole("combobox", { name: "Another person" })).toBeDisabled();
  expect(screen.queryByRole("option", { name: "Sam" })).toBeNull();
  await expect
    .element(screen.getByRole("button", { name: "Check replacement (up to 30 seconds)" }))
    .toBeDisabled();
  holdPeople = false;
  peopleReady.resolve(undefined);
  await screen.findByRole("option", { name: "Sam" });
  await expect.element(screen.getByRole("combobox", { name: "Another person" })).toBeEnabled();
  await userEvent.selectOptions(screen.getByRole("combobox", { name: "Another person" }), targetId);
  await expect
    .element(screen.getByRole("button", { name: "Check replacement (up to 30 seconds)" }))
    .toBeEnabled();
  expect(screen.queryByText(/This saved schedule is older than the current scenario/u)).toBeNull();
  await expect
    .element(screen.getByRole("heading", { name: "Native manual review — invalid" }))
    .toBeVisible();
  expect(screen.queryByRole("button", { name: messages.repair.apply })).toBeNull();

  state.candidate.value = { ...c, scenarioRevision: originalRevision, stale: true };
  await expect
    .element(
      screen.getByRole("button", { name: "Check chosen current-result replacement diagnostic" }),
    )
    .toBeEnabled();
  await userEvent.click(
    screen.getByRole("button", { name: "Check chosen current-result replacement diagnostic" }),
  );
  await expect.element(screen.getByText(messages.repair.diagnosticUnavailable)).toBeVisible();
  expect(screen.queryByRole("heading", { name: "Check an assignment change" })).toBeNull();
  await expect.element(screen.getByRole("combobox", { name: "Change" })).toHaveValue("replace");
  await expect
    .element(screen.getByRole("textbox", { name: "Soft stability weight" }))
    .toHaveValue("9");
  await expect
    .element(screen.getByRole("heading", { name: "Native manual review — invalid" }))
    .toBeVisible();
  expect(screen.queryByRole("button", { name: messages.repair.apply })).toBeNull();

  incumbentBeyondLimit = true;
  assignmentPage = 0;
  state.candidate.value = c;
  await userEvent.click(
    await screen.findByRole("button", {
      name: "Check chosen current-result replacement diagnostic",
    }),
  );
  await expect.element(await screen.findByText(messages.repair.diagnosticPageLimit)).toBeVisible();
  expect(assignmentPage).toBe(10);
  expect(screen.queryByRole("heading", { name: "Check an assignment change" })).toBeNull();
  await expect
    .element(screen.getByRole("textbox", { name: "Soft stability weight" }))
    .toHaveValue("9");
  await expect
    .element(screen.getByRole("heading", { name: "Native manual review — invalid" }))
    .toBeVisible();
  expect(screen.queryByRole("button", { name: messages.repair.apply })).toBeNull();
});

it.each(["accepted", "external"] as const)(
  "renders paged %s baseline→C members without rebinding historical C to head or inventing accepted ancestry",
  async (sourceKind) => {
    const state = await setup();
    const c: api.SolutionSummaryDtoV1 = {
      schemaVersion: 1,
      scenarioId: project.scenarioId,
      scenarioRevision: 2,
      currentRevision: 3,
      result: { solutionId: targetId, resultChecksum: "2".repeat(64) },
      runId: targetId,
      stale: true,
      selected: false,
      status: "feasible",
      score: { feasibility: "0", levels: [] },
      verificationChecksum: "3".repeat(64),
      finishedAt: "2030-01-01T10:00:00Z",
    };
    const recordedOriginal: api.JsonValue = JSON.parse(
      JSON.stringify({ ...original, sourceRevision: 1 }),
    );
    const cursor = {
      schemaVersion: 1,
      scenarioId: project.scenarioId,
      revision: 2,
      queryFingerprint: Array(32).fill(1) as number[],
      position: { kind: "repairDiff", shiftId },
    };
    vi.mocked(api.getSolutionView).mockImplementation((input) =>
      Promise.resolve(
        response(
          {
            schemaVersion: 1,
            scenarioId: project.scenarioId,
            currentRevision: state.visible.revision,
            scenarioRevision: 2,
            result: c.result,
            view: {
              viewId: "official.workforce.result.schedule",
              data: {
                schemaVersion: 1,
                timeZone: "UTC",
                result: {
                  kind: "repairDiff",
                  availability: "available",
                  source: {
                    scenarioId: project.scenarioId,
                    solutionId: c.result.solutionId,
                    sourceRevision: 2,
                    sourceDocumentHash: "4".repeat(64),
                    normalizedSolutionHash: "5".repeat(64),
                  },
                  baseline:
                    sourceKind === "external"
                      ? {
                          kind: "external",
                          binding: {
                            entity: {
                              kind: "official.workforce.external-starting-plan",
                              id: `official.workforce.${ruleId}`,
                            },
                            selectedPairContentHash: "6".repeat(64),
                          },
                          verificationStatus: "notEstablished",
                        }
                      : {
                          kind: "accepted",
                          original: recordedOriginal,
                          selectedPairContentHash: "6".repeat(64),
                          verificationStatus: "notEstablished",
                          selection: {
                            kind: "manualSelection",
                            entity: { kind: "manualSelection", id: ruleId },
                            recordChecksum: "7".repeat(64),
                            admissionSourceScenarioId: project.scenarioId,
                            admissionSourceRevision: 1,
                            admissionSourceDocumentHash: "8".repeat(64),
                          },
                        },
                  counts: { unchangedPairs: 1, removedPairs: 1, addedPairs: 0 },
                  totalItems: 2,
                  continuation: input.continuation ? null : cursor,
                  items: [
                    {
                      shiftId: input.continuation ? ruleId : shiftId,
                      shiftLabel: input.continuation ? null : "Historical clinic",
                      missingShift: Boolean(input.continuation),
                      change: "newlyUnfilled",
                      beforeMemberCount: 2,
                      afterMemberCount: 1,
                      removedPairs: 1,
                      addedPairs: 0,
                      oldMembers: [
                        { personId, personName: null, missingPerson: true },
                        { personId: targetId, personName: "Sam", missingPerson: false },
                      ],
                      newMembers: [{ personId: targetId, personName: "Sam", missingPerson: false }],
                      unchangedParticipants: [
                        { personId: targetId, personName: "Sam", missingPerson: false },
                      ],
                    },
                  ],
                },
              },
            },
          },
          [],
          state.visible.revision,
        ),
      ),
    );
    state.candidate.value = c;
    await expect
      .element(await screen.findByText(/Some assignments removed; other selected members remain/u))
      .toBeVisible();
    const missingMember = within(screen.getByRole("region", { name: messages.repair.diffHeading }))
      .getAllByRole("listitem")
      .find((item) => item.querySelector("code")?.textContent === personId);
    if (!missingMember) throw new Error("Missing historical member was omitted");
    await expect.element(missingMember).toMatchTextContent(personId);
    await expect.element(missingMember).toMatchTextContent(/unavailable/iu);
    expect(
      within(screen.getByRole("region", { name: messages.repair.diffHeading })).getAllByText("Sam"),
    ).toHaveLength(2);
    if (sourceKind === "accepted") {
      expect(state.baselines).toHaveBeenCalledWith(
        expect.objectContaining({
          candidate: c,
          original: expect.objectContaining({
            acceptedResult: original.acceptedResult,
            sourceRevision: 1,
          }),
        }),
      );
    } else {
      await expect
        .element(screen.getByText(/Recorded unverified external starting plan:/u))
        .toBeVisible();
      expect(screen.queryByText(/Recorded original A checksum:/u)).toBeNull();
      expect(screen.queryByText(/Recorded manual B:/u)).toBeNull();
      expect(state.baselines.mock.calls.every(([source]) => source === null)).toBe(true);
    }
    await userEvent.click(screen.getByRole("button", { name: "Next recorded repair groups" }));
    await expect
      .poll(() =>
        vi
          .mocked(api.getSolutionView)
          .mock.calls.some(([input]) => input.continuation !== undefined),
      )
      .toBe(true);
    state.visible.revision = 4;
    await expect.poll(() => vi.mocked(api.getSolutionView).mock.calls.length).toBeGreaterThan(2);
    for (const [input] of vi.mocked(api.getSolutionView).mock.calls)
      expect(input.solutionId).toBe(c.result.solutionId);
    await expect.element(screen.getByText(/Chosen C:/u)).toMatchTextContent("immutable revision 2");
  },
);

it("sends explicit Repair intent, suppresses Authored-only estimate and leaves an active request immutable", async () => {
  fakeApi();
  const pending = deferred<api.ApiResponseDto<api.SolveStartDtoV2>>();
  const operation = portableOperation(pending.promise);
  vi.mocked(api.startSolve).mockReturnValue(operation);
  vi.mocked(api.getSolveBackendOptions).mockResolvedValue(
    response(
      {
        schemaVersion: 2,
        scenarioId: project.scenarioId,
        revision: project.revision,
        options: [
          {
            backendId: "ortools",
            backendVersion: "test",
            adapterVersion: "test",
            displayName: "Installed runtime",
          },
        ],
      },
      [],
      project.revision,
    ),
  );
  vi.mocked(api.listSolveRuns).mockResolvedValue(
    response(
      {
        schemaVersion: 2,
        scenarioId: project.scenarioId,
        currentRevision: project.revision,
        runs: [],
      },
      [],
      project.revision,
    ),
  );
  for (const subscribe of [api.onSolveProgress, api.onSolveVerified, api.onSolveCompleted])
    vi.mocked(subscribe).mockResolvedValue(vi.fn());
  const intent = shallowRef<api.SolveIntent>("repair");
  render(
    defineComponent({
      setup: () => () =>
        h(OptimizeCapabilityPanel, {
          scenarioId: project.scenarioId,
          currentRevision: project.revision,
          intent: intent.value,
          authoredOptimizeUnavailableReason: "Soft protection needs MinimalChanges",
          loading: false,
          error: null,
          summary: {
            revision: project.revision,
            fast: { counts: { errors: 0, warnings: 0, information: 0 }, issues: [], omitted: 0 },
            full: {
              state: "completed",
              operationId: ruleId,
              inputRevision: project.revision,
              stale: false,
              counts: { errors: 0, warnings: 0, information: 0 },
            },
          },
          capabilities: {
            availableCommands: [
              "solve_start",
              "solve_cancel",
              "solve_get_backend_options",
              "solve_list_runs",
              "solve_estimate_model",
            ],
            unavailableCommands: [],
          },
        }),
    }),
  );
  const start = await screen.findByRole("button", { name: messages.repair.repairPlan });
  await expect.element(start).toBeEnabled();
  await userEvent.click(start);
  expect(vi.mocked(api.estimateSolveModel)).not.toHaveBeenCalled();
  intent.value = "optimize";
  await expect.element(await screen.findByText(/Active request intent: repair/u)).toBeVisible();
  expect(vi.mocked(api.startSolve)).toHaveBeenCalledTimes(1);
});
