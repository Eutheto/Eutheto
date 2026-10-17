import { cleanup, render, screen, within } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { afterEach, beforeEach, expect, it, onTestFinished, vi } from "vitest";
import { userEvent } from "vitest/browser";
import * as generatedApi from "../api/generated";
import type {
  AvailabilityCsvMapping,
  AvailabilityCsvPreviewV1,
  AvailabilityCsvRowDecision,
} from "../api/generated";
import type { WorkforceAvailability } from "../api/generated-domain-pack-contracts";
import App from "../App.vue";
import AvailabilityCsvDecisions from "./planner/AvailabilityCsvDecisions.vue";
import { messages } from "../messages";
import { createAppRouter } from "../router";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import "../styles.css";

vi.mock("../api/generated", { spy: true });
const copy = messages.availabilityCsv;
const shared = messages.csvImport;
const sourceId = "01900000-0000-7000-8000-000000000081";
const previewId = "01900000-0000-7000-8000-000000000082";
const suppliedId = "01900000-0000-7000-8000-000000000083";
const storedId = "01900000-0000-7000-8000-000000000084";
const typeId = "019a0000-0000-7000-8000-000000000085";
const personId = "01900000-0000-7000-8000-000000000086";
const locationId = "01900000-0000-7000-8000-000000000087";
const source = { rawBytes: 900, logicalRecords: 3, blake3: "a".repeat(64) };
const resolvedStartsAt = {
  local: "2026-01-05T08:00:00",
  instant: "2026-01-05T13:00:00Z",
  offsetSeconds: -18000,
};
const resolvedEndsAt = {
  local: "2026-01-05T16:00:00",
  instant: "2026-01-05T21:00:00Z",
  offsetSeconds: -18000,
};
const stored: WorkforceAvailability = {
  kind: "availability",
  id: storedId,
  personId,
  availabilityKind: "requestedTimeOff",
  timeWindow: {
    kind: "instant",
    startsAt: "2026-01-05T08:00:00.000-05:00",
    endsAt: "2026-01-05T16:00:00.000-05:00",
  },
  effectiveRange: { startDate: "2026-01-01", endDateExclusive: "2026-02-01" },
  assignmentTypeIds: [typeId.toUpperCase()],
  locationIds: [locationId.toUpperCase()],
  source: "  preserved source  ",
  note: "<img src=x onerror=window.injected=true>",
};
const changed: WorkforceAvailability = { ...stored, note: "Reviewed note" };
const added: WorkforceAvailability = {
  ...stored,
  id: suppliedId,
  availabilityKind: "approvedTimeOff",
};

// Coherent read-only presentation receipts: two actual proposed mutations, not empty-batch authority mocks.
function nativeReview(
  mapping: AvailabilityCsvMapping,
  decisions: readonly AvailabilityCsvRowDecision[],
  revision: number,
): AvailabilityCsvPreviewV1 {
  const decision = decisions.find((item) => item.record === 2)?.decision;
  const supplied = mapping.identityColumn !== null;
  const accepted = supplied && decision?.kind === "add" && decision.availabilityId === suppliedId;
  const skipped = decision?.kind === "skip";
  const blocked = !accepted && !skipped;
  const excludedRows: AvailabilityCsvPreviewV1["preview"]["excludedRows"] = [
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
          identity: supplied
            ? { kind: "supplied", availabilityId: suppliedId }
            : { kind: "absent" },
          availabilityId: accepted ? suppliedId : null,
          before: null,
          after: accepted ? added : null,
          resolvedStartsAt: accepted ? resolvedStartsAt : null,
          resolvedEndsAt: accepted ? resolvedEndsAt : null,
          issue: blocked ? "unresolvedIdentity" : null,
        },
        supplied
          ? {
              record: 3,
              status: "updated",
              identity: { kind: "supplied", availabilityId: storedId },
              availabilityId: storedId,
              before: stored,
              after: changed,
              resolvedStartsAt,
              resolvedEndsAt,
              issue: null,
            }
          : {
              record: 3,
              status: "unresolved",
              identity: { kind: "absent" },
              availabilityId: null,
              before: null,
              after: null,
              resolvedStartsAt: null,
              resolvedEndsAt: null,
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
            format: "eutheto/workforce-availability-csv-review",
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
        family: "availability",
        data: {
          schemaVersion: 1,
          dialects: [
            { status: "candidate", dialect: "comma", source, consistentColumns: 12, samples: [] },
          ],
        },
      }),
    ),
  );
  const preview = vi
    .spyOn(generatedApi.CsvImportFlow.prototype, "previewAvailability")
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
  await router.push({
    name: "project-availability-import",
    params: { scenarioId: project.scenarioId },
  });
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
      "personId",
      "availabilityKind",
      "startsAt",
      "endsAt",
      "weeklyWindows",
      "effectiveStartDate",
      "effectiveEndDateExclusive",
      "assignmentTypeIds",
      "locationIds",
      "source",
      "note",
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
    { record: 2, decision: { kind: "add", availabilityId: suppliedId } },
  ]);
  const records = screen.getByRole("table", { name: copy.records });
  expect(within(records).getAllByRole("row")).toHaveLength(3);
  expect(records).toHaveTextContent("2026-01-05T08:00:00");
  expect(records).toHaveTextContent("2026-01-05T13:00:00Z");
  expect(records).toHaveTextContent("-18000 s");
  expect(records).toHaveTextContent("2026-01-05T08:00:00.000-05:00");
  expect(records).toHaveTextContent(typeId.toUpperCase());
  expect(records).toHaveTextContent("preserved source");
  expect(records).toHaveTextContent("requestedTimeOff");
  expect(records).toHaveTextContent("approvedTimeOff");
  expect(records).toHaveTextContent("Reviewed note");
  expect(records.querySelector("img")).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: copy.inspectRow("2") }));
  const selected = await screen.findByRole("region", { name: copy.selectedRecord });
  expect(within(selected).getByText('<img src=x onerror="window.injected=true">')).toBeVisible();
  expect(selected.querySelector("img")).toBeNull();
  expect(native.sample.mock.calls.at(-1)?.[2]).toEqual({
    readFormat: { family: "availability", dialect: "comma" },
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
              identity: { kind: "supplied", availabilityId: typeId },
              availabilityId: null,
              before: null,
              after: null,
              resolvedStartsAt: null,
              resolvedEndsAt: null,
              issue: "reservedIdentity",
            },
            {
              record: 3,
              status: "blocked",
              identity: { kind: "supplied", availabilityId: storedId },
              availabilityId: storedId,
              before: null,
              after: null,
              resolvedStartsAt: null,
              resolvedEndsAt: null,
              issue: "invalidAvailability",
            },
          ],
          excludedRows: [
            { record: 2, skipped: false, issue: "reservedIdentity" },
            { record: 3, skipped: false, issue: "invalidAvailability" },
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
    screen.getByRole("combobox", { name: `${copy.fields.note} · ${copy.blank}` }),
    "clear",
  );
  expect(screen.queryByRole("checkbox", { name: copy.approve })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  expect(
    native.preview.mock.calls.at(-1)?.[2].mapping.columns.find((item) => item.field === "note")
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
  expect(screen.getByRole("table", { name: copy.records })).toHaveTextContent(suppliedId);
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  const approval = await screen.findByRole("checkbox", { name: copy.approve });
  expect(approval).not.toBeChecked();
  expect(native.preview.mock.calls.at(-1)?.[2].decisions).toEqual([
    { record: 2, decision: { kind: "add", availabilityId: suppliedId } },
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
            rows: { family: "availability", excludedRows: [] },
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

it("keeps identical exact tokens independent across roles and invalidates consent without losing raw review", async () => {
  const native = configureNative();
  await openImport();
  await reviewWithSuppliedAdd();
  await userEvent.click(screen.getByRole("checkbox", { name: copy.approve }));
  for (const [kind, id] of [
    ["person", personId],
    ["assignmentType", typeId],
    ["location", locationId],
  ] as const) {
    await userEvent.selectOptions(screen.getByRole("combobox", { name: copy.endpoint }), kind);
    await userEvent.fill(screen.getByRole("textbox", { name: copy.token }), " exact token ");
    await userEvent.fill(screen.getByRole("textbox", { name: copy.endpointId }), id);
    await userEvent.click(screen.getByRole("button", { name: copy.addMapping }));
    await expect.element(screen.getByRole("textbox", { name: copy.token })).toHaveFocus();
  }
  expect(screen.queryByRole("checkbox", { name: copy.approve })).toBeNull();
  expect(screen.getByRole("table", { name: copy.records })).toHaveTextContent(storedId);
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  await screen.findByRole("checkbox", { name: copy.approve });
  expect(native.preview.mock.calls.at(-1)?.[2].mapping).toMatchObject({
    personMappings: { " exact token ": personId },
    assignmentTypeMappings: { " exact token ": typeId },
    locationMappings: { " exact token ": locationId },
  });
  await userEvent.fill(screen.getByRole("textbox", { name: copy.token }), " exact token ");
  await userEvent.fill(screen.getByRole("textbox", { name: copy.endpointId }), personId);
  await userEvent.click(screen.getByRole("button", { name: copy.addMapping }));
  expect(screen.getByText(copy.duplicateMapping)).toBeVisible();
  await userEvent.click(
    screen.getByRole("button", {
      name: copy.removeMappingLabel(copy.endpoints.person, JSON.stringify(" exact token ")),
    }),
  );
  expect(screen.queryByRole("checkbox", { name: copy.approve })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  await screen.findByRole("checkbox", { name: copy.approve });
  expect(native.preview.mock.calls.at(-1)?.[2].mapping).toMatchObject({
    personMappings: {},
    assignmentTypeMappings: { " exact token ": typeId },
    locationMappings: { " exact token ": locationId },
  });
  expect(native.apply).not.toHaveBeenCalled();
});

it("shows native equivalent endpoint facts and authored weekly order without implying missing projections mean absence", async () => {
  const native = configureNative();
  const weekly: WorkforceAvailability = {
    ...stored,
    availabilityKind: "availableOnly",
    timeWindow: {
      kind: "weekly",
      windows: [
        {
          weekdays: ["friday", "monday"],
          startTime: "22:00:00",
          endTime: "06:00:00",
          endDayOffset: 1,
        },
        { weekdays: ["tuesday"], startTime: "09:00:00", endTime: "12:00:00", endDayOffset: 0 },
      ],
    },
  };
  native.preview.mockImplementation((_scope, _sourceId, input) => {
    const receipt = nativeReview(input.mapping, input.decisions, project.revision);
    return portableOperation(
      response({
        ...receipt,
        preview: {
          ...receipt.preview,
          rows: [
            ...receipt.preview.rows.slice(0, 1),
            {
              record: 3,
              status: "unchanged",
              identity: { kind: "supplied", availabilityId: storedId },
              availabilityId: storedId,
              before: stored,
              after: null,
              resolvedStartsAt,
              resolvedEndsAt,
              issue: null,
            },
            {
              record: 4,
              status: "unchanged",
              identity: { kind: "supplied", availabilityId: locationId },
              availabilityId: locationId,
              before: { ...weekly, id: locationId },
              after: null,
              resolvedStartsAt: null,
              resolvedEndsAt: null,
              issue: null,
            },
          ],
        },
      }),
    );
  });
  await openImport();
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  const records = await screen.findByRole("table", { name: copy.records });
  const equivalent = records.querySelector('[data-availability-csv-record="3"]');
  const recurring = records.querySelector('[data-availability-csv-record="4"]');
  if (!(equivalent instanceof HTMLElement) || !(recurring instanceof HTMLElement))
    throw new Error("Missing native review records");
  expect(equivalent).toHaveTextContent("2026-01-05T08:00:00.000-05:00");
  expect(equivalent).toHaveTextContent(resolvedStartsAt.local);
  expect(equivalent).toHaveTextContent(resolvedStartsAt.instant);
  expect(equivalent).toHaveTextContent("-18000 s");
  expect(within(equivalent).getByText(copy.unchanged)).toBeVisible();
  expect(recurring).toHaveTextContent('"kind": "weekly"');
  expect(recurring.textContent.indexOf("friday")).toBeLessThan(
    recurring.textContent.indexOf("tuesday"),
  );
  expect(within(recurring).getByText(copy.noTemporalFacts)).toBeVisible();
  expect(screen.queryByRole("button", { name: copy.addRecord("3") })).toBeNull();
  expect(screen.queryByRole("button", { name: copy.addRecord("4") })).toBeNull();
  expect(native.apply).not.toHaveBeenCalled();
});

it("quarantines a lost apply receipt and offers History rather than reusing consent", async () => {
  const native = configureNative();
  native.apply.mockImplementation(() =>
    portableOperation(Promise.reject(new Error("Native receipt unavailable"))),
  );
  await openImport();
  await reviewWithSuppliedAdd();
  await userEvent.click(screen.getByRole("checkbox", { name: copy.approve }));
  await userEvent.click(screen.getByRole("button", { name: shared.apply }));
  await screen.findByText(shared.uncertain);
  expect(screen.getByRole("link", { name: copy.history })).toBeVisible();
  expect(screen.queryByRole("checkbox", { name: copy.approve })).toBeNull();
  expect(screen.queryByRole("button", { name: shared.preview })).toBeNull();
  expect(screen.queryByRole("button", { name: shared.apply })).toBeNull();
  expect(native.apply).toHaveBeenCalledOnce();
});

it("pages raw native record projections with keyboard focus recovery rather than dropping later authored content", async () => {
  const rows: AvailabilityCsvPreviewV1["preview"]["rows"] = Array.from(
    { length: 51 },
    (_, index) => {
      const record = index + 2;
      const id = `01900000-0000-7000-8000-${String(record).padStart(12, "0")}`;
      return {
        record,
        status: "unchanged",
        identity: { kind: "supplied", availabilityId: id },
        availabilityId: id,
        before: { ...stored, id, note: `Authored note ${String(record)}` },
        after: null,
        resolvedStartsAt: null,
        resolvedEndsAt: null,
        issue: null,
      };
    },
  );
  render(AvailabilityCsvDecisions, {
    props: {
      project: { ...project, domainPackId: "official.workforce" },
      libraryEpoch: 0,
      rows,
      modelValue: [],
      current: true,
      disabled: false,
    },
  });
  const records = screen.getByRole("table", { name: copy.records });
  expect(records).toHaveTextContent("Authored note 2");
  expect(records).not.toHaveTextContent("Authored note 52");
  const next = screen.getByRole("button", { name: copy.nextRecords });
  next.focus();
  await userEvent.keyboard("{Enter}");
  await expect.element(screen.getByRole("heading", { name: copy.decisions })).toHaveFocus();
  expect(records).toHaveTextContent("Authored note 52");
  expect(records).not.toHaveTextContent("Authored note 2");
  const previous = screen.getByRole("button", { name: copy.previousRecords });
  previous.focus();
  await userEvent.keyboard("{Enter}");
  await expect.element(screen.getByRole("heading", { name: copy.decisions })).toHaveFocus();
  expect(records).toHaveTextContent("Authored note 2");
});
