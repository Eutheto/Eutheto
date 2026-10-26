import { cleanup, render, screen, within } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { afterEach, beforeEach, expect, it, onTestFinished, vi } from "vitest";
import { userEvent } from "vitest/browser";
import * as generatedApi from "../api/generated";
import type { ShiftCsvMapping, ShiftCsvPreviewV1, ShiftCsvRowDecision } from "../api/generated";
import type { WorkforceShiftInstance } from "../api/generated-domain-pack-contracts";
import App from "../App.vue";
import { messages } from "../messages";
import { createAppRouter } from "../router";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import "../styles.css";

vi.mock("../api/generated", { spy: true });
const copy = messages.shiftCsv;
const shared = messages.csvImport;
const sourceId = "01900000-0000-7000-8000-000000000081";
const previewId = "01900000-0000-7000-8000-000000000082";
const suppliedId = "01900000-0000-7000-8000-000000000083";
const storedId = "01900000-0000-7000-8000-000000000084";
const typeId = "01900000-0000-7000-8000-000000000085";
const templateId = "01900000-0000-7000-8000-000000000086";
const source = { rawBytes: 900, logicalRecords: 3, blake3: "a".repeat(64) };
const stored: WorkforceShiftInstance = {
  kind: "shiftInstance",
  id: storedId,
  assignmentTypeId: typeId,
  startsAt: {
    local: "2026-01-05T08:00:00",
    instant: "2026-01-05T13:00:00Z",
    offsetSeconds: -18000,
  },
  endsAt: { local: "2026-01-05T16:00:00", instant: "2026-01-05T21:00:00Z", offsetSeconds: -18000 },
  coverage: {
    kind: "atLeast",
    minimum: 1,
    preferredCount: 2,
    maximumCount: 3,
    qualificationMinimums: [],
  },
  tags: ["Retained tag"],
  reportingAttribution: "endLocalDate",
  origin: { kind: "detached", templateId, occurrenceDate: "2026-01-05" },
};
const changed: WorkforceShiftInstance = { ...stored, tags: ["Reviewed tag"] };
const added: WorkforceShiftInstance = {
  ...stored,
  id: suppliedId,
  origin: { kind: "manual" },
  tags: [],
};

// Coherent read-only presentation receipts: two actual proposed mutations, not empty-batch authority mocks.
function nativeReview(
  mapping: ShiftCsvMapping,
  decisions: readonly ShiftCsvRowDecision[],
  revision: number,
): ShiftCsvPreviewV1 {
  const decision = decisions.find((item) => item.record === 2)?.decision;
  const supplied = mapping.identityColumn !== null;
  const accepted = supplied && decision?.kind === "add" && decision.shiftId === suppliedId;
  const skipped = decision?.kind === "skip";
  const blocked = !accepted && !skipped;
  const excludedRows: ShiftCsvPreviewV1["preview"]["excludedRows"] = [
    ...(blocked || skipped
      ? [{ record: 2, skipped, issue: blocked ? ("unresolvedIdentity" as const) : null }]
      : []),
    ...(!supplied ? [{ record: 3, skipped: false, issue: "unresolvedIdentity" as const }] : []),
  ];
  return {
    schemaVersion: 1,
    sourceId,
    previewId,
    scenarioId: project.scenarioId,
    revision,
    preview: {
      schemaVersion: 1,
      source,
      disposition: blocked ? "blocked" : "reviewable",
      rows: [
        {
          record: 2,
          status: blocked ? "unresolved" : skipped ? "skipped" : "added",
          identity: supplied ? { kind: "supplied", shiftId: suppliedId } : { kind: "absent" },
          shiftId: accepted ? suppliedId : null,
          before: null,
          after: accepted ? added : null,
          issue: blocked ? "unresolvedIdentity" : null,
        },
        supplied
          ? {
              record: 3,
              status: "updated",
              identity: { kind: "supplied", shiftId: storedId },
              shiftId: storedId,
              before: stored,
              after: changed,
              issue: null,
            }
          : {
              record: 3,
              status: "unresolved",
              identity: { kind: "absent" },
              shiftId: null,
              before: null,
              after: null,
              issue: "unresolvedIdentity",
            },
      ],
      excludedRows,
      validationIssues: [],
      batch: blocked
        ? null
        : {
            schemaVersion: 1,
            packId: "official.workforce",
            scenarioSchemaVersion: 1,
            label: null,
            commands: [
              ...(accepted
                ? [
                    {
                      commandType: "official.workforce.add_entity" as const,
                      payload: { entity: added },
                    },
                  ]
                : []),
              {
                commandType: "official.workforce.update_entity" as const,
                payload: { entity: changed },
              },
            ],
          },
      approvalDigest: blocked ? null : "b".repeat(64),
      review: blocked
        ? null
        : {
            schemaVersion: 1,
            format: "eutheto/workforce-shift-csv-review",
            scenarioId: project.scenarioId,
            revision,
            scenarioBlake3: "c".repeat(64),
            source,
            parserVersion: "csv-core-0.1.13",
            limitsVersion: 1,
            mapping,
            decisions,
            rowsBlake3: "d".repeat(64),
            changesBlake3: "e".repeat(64),
            excludedBlake3: "f".repeat(64),
            validationBlake3: "0".repeat(64),
          },
    },
  };
}

function configureNative() {
  const library: [typeof project] = [{ ...project, domainPackId: "official.workforce" }];
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
  vi.spyOn(generatedApi.CsvImportFlow.prototype, "dispose").mockResolvedValue(undefined);
  vi.spyOn(generatedApi.CsvImportFlow.prototype, "discardPreview").mockResolvedValue(undefined);
  vi.spyOn(generatedApi.CsvImportFlow.prototype, "open").mockImplementation(() =>
    portableOperation(response({ schemaVersion: 1, sourceId, byteCount: source.rawBytes })),
  );
  vi.spyOn(generatedApi.CsvImportFlow.prototype, "detect").mockImplementation(() =>
    portableOperation(
      response({
        family: "shifts",
        data: {
          schemaVersion: 1,
          dialects: [
            { status: "candidate", dialect: "comma", source, consistentColumns: 8, samples: [] },
          ],
        },
      }),
    ),
  );
  const preview = vi
    .spyOn(generatedApi.CsvImportFlow.prototype, "previewShifts")
    .mockImplementation((_scope, _sourceId, input) =>
      portableOperation(
        response(nativeReview(input.mapping, input.decisions, library[0].revision)),
      ),
    );
  const sample = vi
    .spyOn(generatedApi.CsvImportFlow.prototype, "sample")
    .mockImplementation((_scope, _sourceId, input) =>
      portableOperation(
        response({
          schemaVersion: 1,
          sourceId,
          readFormat: input.readFormat,
          record: input.record,
          cells: [{ text: '<img src=x onerror="window.injected=true">', truncated: false }],
        }),
      ),
    );
  const apply = vi.spyOn(generatedApi.CsvImportFlow.prototype, "apply");
  return { library, api, preview, sample, apply };
}
async function openImport(): Promise<void> {
  const router = createAppRouter();
  onTestFinished(() => {
    router.options.history.destroy();
  });
  await router.push({ name: "project-work-import", params: { scenarioId: project.scenarioId } });
  render(App, { global: { plugins: [createPinia(), PiniaColada, router] } });
  await router.isReady();
  await userEvent.click(await screen.findByRole("button", { name: shared.choose }));
  await userEvent.selectOptions(
    await screen.findByRole("combobox", { name: shared.dialect }),
    "comma",
  );
  await userEvent.selectOptions(screen.getByRole("combobox", { name: shared.header }), "yes");
  await userEvent.selectOptions(screen.getByRole("combobox", { name: copy.identityColumn }), "0");
  for (const [index, field] of (
    [
      "assignmentTypeId",
      "locationId",
      "startsAt",
      "endsAt",
      "coverage",
      "tags",
      "reportingAttribution",
    ] as const
  ).entries()) {
    await userEvent.selectOptions(
      screen.getByRole("combobox", { name: copy.fields[field] }),
      String(index + 1),
    );
  }
}
async function reviewWithSuppliedAdd(): Promise<void> {
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  await userEvent.click(await screen.findByRole("button", { name: copy.addRecord("2") }));
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  await screen.findByRole("checkbox", { name: copy.approve });
}
beforeEach(() => {
  vi.clearAllMocks();
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("uses native Supplied facts for same-ID Add and shows complete inert before/after local-time meaning", async () => {
  const native = configureNative();
  await openImport();
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  const addButton = await screen.findByRole("button", { name: copy.addRecord("2") });
  await userEvent.click(addButton);
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  await screen.findByRole("checkbox", { name: copy.approve });
  expect(native.preview.mock.calls.at(-1)?.[2].decisions).toEqual([
    { record: 2, decision: { kind: "add", shiftId: suppliedId } },
  ]);
  const records = screen.getByRole("table", { name: copy.records });
  expect(within(records).getAllByRole("row")).toHaveLength(3);
  expect(records).toMatchTextContent("2026-01-05T08:00:00");
  expect(records).toMatchTextContent("2026-01-05T13:00:00Z");
  expect(records).toMatchTextContent("-18000 s");
  expect(records).toMatchTextContent('"maximumCount": 3');
  expect(records).toMatchTextContent('"kind": "detached"');
  expect(records).toMatchTextContent("Retained tag");
  expect(records).toMatchTextContent("Reviewed tag");
  await userEvent.click(screen.getByRole("button", { name: copy.inspectRow("2") }));
  const selected = await screen.findByRole("region", { name: copy.selectedRecord });
  expect(within(selected).getByText('<img src=x onerror="window.injected=true">')).toBeVisible();
  expect(selected.querySelector("img")).toBeNull();
  expect(native.sample.mock.calls.at(-1)?.[2]).toEqual({
    readFormat: { family: "shifts", dialect: "comma" },
    record: 2,
  });
  expect(native.apply).not.toHaveBeenCalled();
});

it("does not offer Add when a supplied identity is reserved or an existing projection is unavailable", async () => {
  const native = configureNative();
  native.preview.mockImplementation((_scope, _sourceId, input) => {
    const result = nativeReview(input.mapping, input.decisions, project.revision);
    return portableOperation(
      response({
        ...result,
        preview: {
          ...result.preview,
          rows: [
            {
              record: 2,
              status: "blocked",
              identity: { kind: "supplied", shiftId: typeId },
              shiftId: null,
              before: null,
              after: null,
              issue: "reservedIdentity",
            },
            {
              record: 3,
              status: "blocked",
              identity: { kind: "supplied", shiftId: storedId },
              shiftId: storedId,
              before: null,
              after: null,
              issue: "invalidShift",
            },
          ],
          excludedRows: [
            { record: 2, skipped: false, issue: "reservedIdentity" },
            { record: 3, skipped: false, issue: "invalidShift" },
          ],
        },
      }),
    );
  });
  await openImport();
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  await screen.findByText(/reservedIdentity/u);
  expect(screen.queryByRole("button", { name: copy.addRecord("2") })).toBeNull();
  expect(screen.queryByRole("button", { name: copy.addRecord("3") })).toBeNull();
  expect(screen.getByRole("button", { name: copy.skipRecord("2") })).toBeEnabled();
  expect(native.apply).not.toHaveBeenCalled();
});

it("invalidates mapping and stale approval while retaining exact decisions across revision-only refresh", async () => {
  const native = configureNative();
  await openImport();
  await reviewWithSuppliedAdd();
  await userEvent.click(screen.getByRole("checkbox", { name: copy.approve }));
  expect(screen.getByRole("button", { name: shared.apply })).toBeEnabled();
  await userEvent.selectOptions(
    screen.getByRole("combobox", { name: `${copy.fields.tags} · ${copy.blank}` }),
    "clear",
  );
  expect(screen.queryByRole("checkbox", { name: copy.approve })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  expect(
    native.preview.mock.calls.at(-1)?.[2].mapping.columns.find((item) => item.field === "tags")
      ?.blank,
  ).toBe("clear");
  await userEvent.click(await screen.findByRole("checkbox", { name: copy.approve }));
  native.library[0] = { ...native.library[0], revision: project.revision + 1 };
  const notify = vi.mocked(generatedApi.onScenarioChanged).mock.calls[0]?.[0];
  if (!notify) throw new Error("Missing root scenario notification registration");
  notify({
    type: "scenarioChanged",
    payload: {
      context: {
        eventVersion: 1,
        timestamp: "2026-10-05T12:00:00Z",
        requestId: "01900000-0000-7000-8000-000000000099",
        scenarioId: project.scenarioId,
        revision: project.revision + 1,
        solveRunId: null,
      },
      changeSet: { changes: [] },
    },
  });
  await expect.poll(() => screen.queryByRole("checkbox", { name: copy.approve })).toBeNull();
  expect(screen.getByRole("table", { name: copy.records })).toMatchTextContent(suppliedId);
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  const approval = await screen.findByRole("checkbox", { name: copy.approve });
  expect(approval).not.toBeChecked();
  expect(native.preview.mock.calls.at(-1)?.[2].decisions).toEqual([
    { record: 2, decision: { kind: "add", shiftId: suppliedId } },
  ]);
  await userEvent.selectOptions(screen.getByRole("combobox", { name: copy.identityColumn }), "");
  expect(screen.queryByRole("checkbox", { name: copy.approve })).toBeNull();
  expect(screen.queryByRole("table", { name: copy.records })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  expect(native.preview.mock.calls.at(-1)?.[2].decisions).toEqual([]);
  expect(native.apply).not.toHaveBeenCalled();
});

it("requires explicit approval and focuses retained report after a correlated successful apply refresh", async () => {
  const native = configureNative();
  await openImport();
  await reviewWithSuppliedAdd();
  const applyButton = screen.getByRole("button", { name: shared.apply });
  expect(applyButton).toBeDisabled();
  native.apply.mockImplementation((_scope, input) => {
    const revision = project.revision + 1;
    native.library[0] = { ...native.library[0], revision };
    return portableOperation(
      response(
        {
          schemaVersion: 1,
          scenarioId: project.scenarioId,
          sourceId,
          outcome: { kind: "applied", revision, commandId: input.commandId },
          report: {
            schemaVersion: 1,
            previewId,
            disposition: "reviewable",
            consumed: true,
            rows: { family: "shifts", excludedRows: [] },
          },
        },
        [],
        revision,
      ),
    );
  });
  await userEvent.click(screen.getByRole("checkbox", { name: copy.approve }));
  await userEvent.click(applyButton);
  await expect.element(screen.getByRole("heading", { name: copy.report })).toHaveFocus();
  expect(native.apply).toHaveBeenCalledOnce();
  expect(screen.getByText(shared.consumed)).toBeVisible();
  expect(screen.queryByRole("button", { name: shared.apply })).toBeNull();
});
