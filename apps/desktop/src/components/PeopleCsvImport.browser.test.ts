import { cleanup, render, screen, within } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { userEvent } from "vitest/browser";
import * as generatedApi from "../api/generated";
import type {
  PeopleCsvMapping,
  PeopleCsvPreviewV1,
  PeopleCsvRejectedRowsV1,
} from "../api/generated";
import App from "../App.vue";
import { messages } from "../messages";
import { createAppRouter } from "../router";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import { plannerMessage } from "./planner/messages";
import "../styles.css";

vi.mock("../api/generated", { spy: true });

const workforceProject = { ...project, domainPackId: "official.workforce" };
const sourceId = "01900000-0000-7000-8000-000000000071";
const previewId = "01900000-0000-7000-8000-000000000072";
const source = { rawBytes: 200_000, logicalRecords: 10_001, blake3: "a".repeat(64) };
const mapping: PeopleCsvMapping = {
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
    teamIds: [],
    tags: [],
    workloadWeight: { numerator: 1, denominator: 1 },
  },
  referenceMappings: {},
};

// Native response fixtures exercise renderer transitions, not CSV parsing, matching or admission.
function review(
  rows: PeopleCsvPreviewV1["preview"]["rows"],
  batch: PeopleCsvPreviewV1["preview"]["batch"],
  revision = project.revision,
): PeopleCsvPreviewV1 {
  const blocked = batch === null;
  return {
    schemaVersion: 1,
    sourceId,
    previewId,
    scenarioId: project.scenarioId,
    revision,
    preview: {
      schemaVersion: 1,
      source,
      columns: mapping.columns,
      disposition: blocked ? "blocked" : "reviewable",
      rows,
      rejectedRows: rows.flatMap((row) =>
        row.rejection === null ? [] : [{ record: row.record, code: row.rejection }],
      ),
      validationIssues: [],
      batch,
      approvalDigest: blocked ? null : "b".repeat(64),
      review: blocked
        ? null
        : {
            schemaVersion: 1,
            format: "eutheto/workforce-people-csv-review",
            scenarioId: project.scenarioId,
            revision,
            scenarioBlake3: "c".repeat(64),
            source,
            parserVersion: "csv-core-0.1.13",
            limitsVersion: 1,
            mapping,
            decisions: [],
            changesBlake3: "d".repeat(64),
            rejectedBlake3: "e".repeat(64),
            validationBlake3: "f".repeat(64),
          },
    },
  };
}

function changedReview(ids: readonly string[], revision = project.revision, add = false) {
  return review(
    [
      ...ids.map((personId, index): PeopleCsvPreviewV1["preview"]["rows"][number] => ({
        record: index + 2,
        status: index === 0 || add ? "added" : "updated",
        personId,
        rejection: null,
      })),
      ...(add
        ? [
            {
              record: 3,
              status: "rejected" as const,
              personId: null,
              rejection: "missingName" as const,
            },
          ]
        : []),
    ],
    {
      schemaVersion: 1,
      packId: "official.workforce",
      scenarioSchemaVersion: 1,
      label: null,
      commands: ids.map((id, index) => ({
        commandType:
          index === 0 || add ? "official.workforce.add_entity" : "official.workforce.update_entity",
        payload: {
          entity: {
            ...mapping.newPersonDefaults,
            kind: "person",
            id,
            name: `Person ${String(index + 1)}`,
          },
        },
      })),
    },
    revision,
  );
}

function configureNative() {
  const library = [{ ...workforceProject }];
  const api = fakeApi(library);
  vi.mocked(generatedApi.listProjects).mockImplementation(() => api.listProjects("all"));
  vi.mocked(generatedApi.openProject).mockImplementation(api.openProject);
  vi.mocked(generatedApi.onAppNotification).mockResolvedValue(() => {});
  vi.mocked(generatedApi.onLibraryRefreshRequired).mockResolvedValue(() => {});
  vi.mocked(generatedApi.onScenarioChanged).mockImplementation(api.onScenarioChanged);
  vi.mocked(generatedApi.onScenarioValidationChanged).mockImplementation(
    api.onScenarioValidationChanged,
  );
  vi.mocked(generatedApi.getApplicationSettings).mockResolvedValue(
    response(
      {
        schemaVersion: 1,
        libraryRevision: 1,
        settings: { appearance: null, locale: null, units: null },
      },
      [],
      1,
    ),
  );
  vi.spyOn(generatedApi.PeopleCsvFlow.prototype, "dispose").mockResolvedValue(undefined);
  vi.spyOn(generatedApi.PeopleCsvFlow.prototype, "discardPreview").mockResolvedValue(undefined);
  vi.spyOn(generatedApi.PeopleCsvFlow.prototype, "open").mockImplementation(() =>
    portableOperation(
      response({
        schemaVersion: 1,
        sourceId,
        byteCount: source.rawBytes,
      }),
    ),
  );
  vi.spyOn(generatedApi.PeopleCsvFlow.prototype, "detect").mockImplementation(() =>
    portableOperation(
      response({
        schemaVersion: 1,
        dialects: [
          { status: "candidate", dialect: "comma", source, consistentColumns: 2, samples: [] },
        ],
      }),
    ),
  );
  const preview = vi.spyOn(generatedApi.PeopleCsvFlow.prototype, "preview");
  const apply = vi.spyOn(generatedApi.PeopleCsvFlow.prototype, "apply");
  return { library, preview, apply };
}

async function openImport() {
  const router = createAppRouter();
  onTestFinished(() => {
    router.options.history.destroy();
  });
  await router.push({ name: "project-people-import", params: { scenarioId: project.scenarioId } });
  render(App, { global: { plugins: [createPinia(), PiniaColada, router] } });
  await router.isReady();
  await userEvent.click(await screen.findByRole("button", { name: messages.csvImport.choose }));
  await userEvent.selectOptions(
    await screen.findByRole("combobox", { name: messages.csvImport.dialect }),
    "comma",
  );
  await userEvent.selectOptions(
    screen.getByRole("combobox", { name: messages.csvImport.header }),
    "yes",
  );
  await userEvent.fill(screen.getByRole("textbox", { name: messages.csvImport.width }), "2");
  for (const [column, field] of [
    [1, "externalId"],
    [2, "name"],
  ] as const) {
    await userEvent.selectOptions(
      screen.getByRole("combobox", {
        name: `${plannerMessage("mapping.column", { column })} ${plannerMessage("mapping.field")}`,
      }),
      field,
    );
  }
}

function decisionRegion() {
  return screen.getByRole("region", { name: plannerMessage("csvDecision.title") });
}
function recordName(record: number) {
  return plannerMessage("csvDecision.logicalRecord", { record });
}
function mappingControl(position: number, index: number, control: "field" | "blank") {
  return screen.getByRole("combobox", {
    name: `${plannerMessage("mapping.row", { position, column: index + 1 })} ${plannerMessage(`mapping.${control}`)}`,
  });
}
async function addRecord(record: number) {
  await userEvent.fill(
    screen.getByRole("textbox", { name: plannerMessage("csvDecision.recordInput") }),
    String(record),
  );
  const manual = screen.getByRole("region", { name: plannerMessage("csvDecision.recordInput") });
  await userEvent.click(
    within(manual).getByRole("button", { name: plannerMessage("csvDecision.add") }),
  );
  const article = within(decisionRegion()).getByRole("article", { name: recordName(record) });
  const id = article.textContent.match(
    /[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}/u,
  )?.[0];
  if (!id) throw new Error("The explicit Add decision did not expose its identity");
  return id;
}
async function previewImport() {
  await userEvent.click(screen.getByRole("button", { name: messages.csvImport.preview }));
  await expect
    .element(await screen.findByRole("heading", { name: messages.csvImport.review }))
    .toHaveFocus();
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.resetAllMocks();
  vi.unstubAllGlobals();
});

describe("Routed CSV review transitions", () => {
  it("retains Add identity across revision-only review, but clears approval and identities on interpretation edits", async () => {
    const native = configureNative();
    await openImport();
    const originalId = await addRecord(2);
    native.preview.mockImplementation(() =>
      portableOperation(response(changedReview([originalId], 3, true))),
    );
    await previewImport();
    await expect
      .element(screen.getByRole("button", { name: messages.csvImport.apply }))
      .toBeEnabled();

    native.library[0] = { ...workforceProject, revision: 4 };
    const notify = vi.mocked(generatedApi.onScenarioChanged).mock.calls[0]?.[0];
    if (!notify) throw new Error("The root did not register native scenario notifications");
    notify({
      type: "scenarioChanged",
      payload: {
        context: {
          eventVersion: 1,
          timestamp: "2026-09-01T12:00:00Z",
          requestId: "01900000-0000-7000-8000-000000000099",
          scenarioId: project.scenarioId,
          revision: 4,
          solveRunId: null,
        },
        changeSet: { changes: [] },
      },
    });
    await expect
      .poll(() => screen.queryByRole("button", { name: messages.csvImport.apply }))
      .toBeNull();
    await expect.element(decisionRegion()).toHaveTextContent(originalId);
    native.preview.mockImplementation(() =>
      portableOperation(response(changedReview([originalId], 4, true))),
    );
    await previewImport();
    await expect.element(decisionRegion()).toHaveTextContent(originalId);
    await expect
      .element(screen.getByRole("button", { name: messages.csvImport.apply }))
      .toBeEnabled();

    const report: PeopleCsvRejectedRowsV1 = {
      schemaVersion: 1,
      previewId,
      disposition: "reviewable",
      consumed: false,
      rejectedRows: [{ record: 3, code: "missingName" }],
    };
    vi.spyOn(generatedApi.PeopleCsvFlow.prototype, "rejectedRows").mockResolvedValue(
      response(report),
    );
    await userEvent.click(screen.getByText(/Workload and targets/u));
    const edits = [
      () =>
        userEvent.selectOptions(
          screen.getByRole("combobox", { name: messages.csvImport.header }),
          "no",
        ),
      () => userEvent.fill(screen.getByRole("textbox", { name: "Top number" }), "2"),
      () => userEvent.selectOptions(mappingControl(1, 0, "blank"), "clear"),
    ];
    let previousId = originalId;
    for (const edit of edits) {
      await edit();
      expect(screen.queryByRole("button", { name: messages.csvImport.apply })).toBeNull();
      expect(within(decisionRegion()).queryAllByRole("article")).toEqual([]);
      expect(decisionRegion().textContent).not.toContain(previousId);
      if (previousId === originalId) {
        // The last native report remains usable even though its approval was invalidated.
        await userEvent.click(screen.getByRole("button", { name: messages.csvImport.readReport }));
        await expect
          .element(screen.getByRole("heading", { name: messages.csvImport.report }))
          .toHaveFocus();
        expect(
          within(screen.getByRole("table", { name: messages.csvImport.report })).getByRole("cell", {
            name: "3",
          }),
        ).toBeVisible();
      }
      const nextId = await addRecord(2);
      expect(nextId).not.toBe(previousId);
      native.preview.mockImplementation(() =>
        portableOperation(response(changedReview([nextId], 4, true))),
      );
      await previewImport();
      previousId = nextId;
    }
    expect(native.apply).not.toHaveBeenCalled();
  });

  it("repairs a native 1001-mutation refusal with explicit Skip, preserving drafts and dispatching one atomic apply", async () => {
    const native = configureNative();
    native.preview.mockImplementationOnce(() =>
      portableOperation(
        Promise.reject(
          Object.assign(new Error("The proposed import exceeds the atomic changed-person limit"), {
            category: "validation",
            code: "people_csv.mutationLimit",
            fieldErrors: [],
          }),
        ),
      ),
    );
    await openImport();
    await userEvent.click(screen.getByText(/Workload and targets/u));
    await userEvent.fill(screen.getByRole("textbox", { name: "Top number" }), "0002");
    const retainedId = await addRecord(2);
    await userEvent.click(screen.getByRole("button", { name: messages.csvImport.preview }));
    await expect
      .element(await screen.findByRole("heading", { name: messages.csvImport.error }))
      .toHaveFocus();
    expect(screen.queryByRole("button", { name: messages.csvImport.apply })).toBeNull();
    await expect.element(decisionRegion()).toHaveTextContent(retainedId);
    await userEvent.fill(
      screen.getByRole("textbox", { name: plannerMessage("csvDecision.recordInput") }),
      "1002",
    );
    await userEvent.click(
      within(
        screen.getByRole("region", { name: plannerMessage("csvDecision.recordInput") }),
      ).getByRole("button", { name: plannerMessage("csvDecision.skip") }),
    );
    await expect
      .element(within(decisionRegion()).getByRole("article", { name: recordName(1002) }))
      .toHaveFocus();
    await expect
      .element(
        within(within(decisionRegion()).getByRole("article", { name: recordName(1002) })).getByRole(
          "button",
          {
            name: plannerMessage("csvDecision.skip"),
          },
        ),
      )
      .toHaveAttribute("aria-pressed", "true");
    await expect.element(screen.getByRole("textbox", { name: "Top number" })).toHaveValue("0002");
    await expect.element(mappingControl(1, 0, "field")).toHaveValue("externalId");
    await expect.element(mappingControl(2, 1, "field")).toHaveValue("name");
    await expect.element(decisionRegion()).toHaveTextContent(retainedId);

    const ids = Array.from({ length: 1000 }, (_, index) =>
      index === 0
        ? retainedId
        : `01900000-0000-7000-8000-${String(index + 1000).padStart(12, "0")}`,
    );
    const repaired = changedReview(ids);
    native.preview.mockImplementationOnce(() =>
      portableOperation(
        response({
          ...repaired,
          preview: {
            ...repaired.preview,
            rows: [
              ...repaired.preview.rows,
              { record: 1002, status: "skipped", personId: null, rejection: null },
            ],
          },
        }),
      ),
    );
    const receipt =
      Promise.withResolvers<generatedApi.ApiResponseDto<generatedApi.PeopleCsvApplyV1>>();
    native.apply.mockImplementation(() => portableOperation(receipt.promise));
    await previewImport();
    expect(native.apply).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: messages.csvImport.apply }));
    await expect.poll(() => native.apply.mock.calls.length).toBe(1);
    const applied = native.apply.mock.calls[0]?.[1];
    if (!applied) throw new Error("No atomic CSV apply was dispatched");
    native.library[0] = { ...workforceProject, revision: 4 };
    receipt.resolve(
      response({
        schemaVersion: 1,
        sourceId,
        scenarioId: project.scenarioId,
        outcome: { kind: "applied", commandId: applied.commandId, revision: 4 },
        report: {
          schemaVersion: 1,
          previewId,
          disposition: "reviewable",
          consumed: true,
          rejectedRows: [],
        },
      }),
    );
    await expect
      .element(screen.getByRole("heading", { name: messages.csvImport.report }))
      .toHaveFocus();
    expect(screen.queryByRole("button", { name: messages.csvImport.apply })).toBeNull();
    expect(native.apply).toHaveBeenCalledTimes(1);
    expect(native.preview).toHaveBeenCalledTimes(2);
  });

  it("bounds a 10000-row native review and reaches distant records through keyboard Go and paging", async () => {
    const native = configureNative();
    native.preview.mockImplementation(() =>
      portableOperation(
        response(
          review(
            Array.from({ length: 10_000 }, (_, index) => ({
              record: index + 2,
              status: "unresolved",
              personId: null,
              rejection: null,
            })),
            null,
          ),
        ),
      ),
    );
    await openImport();
    await previewImport();
    const rows = screen.getByRole("region", { name: plannerMessage("csvDecision.reviewTitle") });
    expect(within(rows).getAllByRole("article")).toHaveLength(50);
    expect(screen.getAllByRole("article")).toHaveLength(50);
    const record = screen.getByRole("textbox", { name: plannerMessage("csvDecision.recordInput") });
    await userEvent.fill(record, "9999");
    await userEvent.keyboard("{Enter}");
    await expect
      .element(within(rows).getByRole("article", { name: recordName(9999) }))
      .toHaveFocus();
    expect(within(rows).queryByRole("article", { name: recordName(2) })).toBeNull();
    expect(screen.getAllByRole("article")).toHaveLength(50);
    const previous = within(rows).getByRole("button", {
      name: plannerMessage("csvDecision.previous"),
    });
    previous.focus();
    await userEvent.keyboard("{Enter}");
    await expect
      .element(
        within(rows).getByRole("heading", { name: plannerMessage("csvDecision.reviewTitle") }),
      )
      .toHaveFocus();
    expect(within(rows).queryByRole("article", { name: recordName(9999) })).toBeNull();
    expect(within(rows).getByRole("article", { name: recordName(9902) })).toBeVisible();
    expect(screen.getAllByRole("article")).toHaveLength(50);
    await userEvent.fill(record, "10001");
    await userEvent.tab();
    await userEvent.tab();
    await userEvent.tab();
    await userEvent.tab();
    await userEvent.tab();
    await expect
      .element(screen.getByRole("button", { name: plannerMessage("csvDecision.go") }))
      .toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await expect
      .element(within(rows).getByRole("article", { name: recordName(10001) }))
      .toHaveFocus();
    expect(screen.getAllByRole("article")).toHaveLength(50);
    expect(native.apply).not.toHaveBeenCalled();
  });
});
