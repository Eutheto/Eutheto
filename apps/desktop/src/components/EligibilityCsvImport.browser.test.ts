import { cleanup, render, screen, within } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { afterEach, expect, it, onTestFinished, vi } from "vitest";
import { userEvent } from "vitest/browser";
import * as generatedApi from "../api/generated";
import type { EligibilityCsvMapping, EligibilityCsvPreviewV1 } from "../api/generated";
import App from "../App.vue";
import { messages } from "../messages";
import { createAppRouter } from "../router";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import "../styles.css";

vi.mock("../api/generated", { spy: true });
const copy = messages.eligibilityCsv;
const shared = messages.csvImport;
const sourceId = "01900000-0000-7000-8000-000000000071";
const previewId = "01900000-0000-7000-8000-000000000072";
const personId = "01900000-0000-7000-8000-000000000073";
const typeId = "01900000-0000-7000-8000-000000000074";
const source = { rawBytes: 900, logicalRecords: 61, blake3: "a".repeat(64) };

// Presentation fixtures only: native scanner, identities and mutation authority are not simulated.
function nativeReview(
  mapping: EligibilityCsvMapping,
  skippedRecords: readonly number[],
): EligibilityCsvPreviewV1 {
  const blocked = !skippedRecords.includes(2);
  const rows: EligibilityCsvPreviewV1["preview"]["rows"] = Array.from(
    { length: 60 },
    (_, index) => {
      const record = index + 2;
      const skipped = skippedRecords.includes(record);
      const duplicate = blocked && record <= 3;
      return {
        record,
        status: skipped ? "skipped" : duplicate ? "blocked" : index === 1 ? "changed" : "unchanged",
        personId:
          index < 2 ? personId : `01900000-0000-7000-8000-${String(index + 100).padStart(12, "0")}`,
        assignmentTypeId: typeId,
        before: true,
        after: index < 2 ? false : true,
        issue: duplicate ? "duplicatePair" : null,
      };
    },
  );
  return {
    schemaVersion: 1,
    sourceId,
    previewId,
    scenarioId: project.scenarioId,
    revision: project.revision,
    preview: {
      schemaVersion: 1,
      source,
      disposition: blocked ? "blocked" : "reviewable",
      rows,
      excludedRows: rows
        .filter((row) => row.status === "blocked" || row.status === "skipped")
        .map((row) => ({
          record: row.record,
          skipped: row.status === "skipped",
          issue: row.issue,
        })),
      validationIssues: [],
      batch: blocked
        ? null
        : {
            schemaVersion: 1,
            packId: "official.workforce",
            scenarioSchemaVersion: 1,
            label: null,
            commands: [
              {
                commandType: "official.workforce.update_entity",
                payload: {
                  entity: {
                    kind: "person",
                    id: personId,
                    name: "Reviewed person",
                    activeRange: { kind: "always" },
                    qualificationGrants: [],
                    eligibleAssignmentTypeIds: [],
                    teamIds: [],
                    tags: [],
                    workloadWeight: { numerator: 1, denominator: 1 },
                  },
                },
              },
            ],
          },
      approvalDigest: blocked ? null : "b".repeat(64),
      review: blocked
        ? null
        : {
            schemaVersion: 1,
            format: "eutheto/workforce-eligibility-csv-review",
            scenarioId: project.scenarioId,
            revision: project.revision,
            scenarioBlake3: "c".repeat(64),
            source,
            parserVersion: "csv-core-0.1.13",
            limitsVersion: 1,
            mapping,
            skippedRecords,
            rowsBlake3: "d".repeat(64),
            changesBlake3: "e".repeat(64),
            excludedBlake3: "f".repeat(64),
            validationBlake3: "0".repeat(64),
          },
    },
  };
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("requires explicit native review after skips, pages all rows, renders inert samples and clears approval on changed interpretation", async () => {
  const api = fakeApi([{ ...project, domainPackId: "official.workforce" }]);
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
        family: "eligibility",
        data: {
          schemaVersion: 1,
          dialects: [
            { status: "candidate", dialect: "comma", source, consistentColumns: 3, samples: [] },
          ],
        },
      }),
    ),
  );
  const preview = vi
    .spyOn(generatedApi.CsvImportFlow.prototype, "previewEligibility")
    .mockImplementation((_scope, _sourceId, input) =>
      portableOperation(response(nativeReview(input.mapping, input.skippedRecords))),
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
  const router = createAppRouter();
  onTestFinished(() => {
    router.options.history.destroy();
  });
  await router.push({
    name: "project-eligibility-import",
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
  for (const [name, value] of [
    [copy.personColumn, "0"],
    [copy.typeColumn, "1"],
    [copy.membershipColumn, "2"],
  ] as const) {
    await userEvent.selectOptions(screen.getByRole("combobox", { name }), value);
  }
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  const records = await screen.findByRole("table", { name: copy.records });
  expect(within(records).getAllByRole("row")).toHaveLength(51);
  expect(within(records).getAllByText("duplicatePair", { exact: false })).toHaveLength(2);
  expect(screen.queryByRole("button", { name: shared.apply })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: copy.nextRecords }));
  expect(within(records).getAllByRole("row")).toHaveLength(11);
  await userEvent.click(screen.getByRole("button", { name: copy.previousRecords }));
  await userEvent.click(screen.getByRole("checkbox", { name: copy.skipRecord("2") }));
  expect(screen.queryByRole("heading", { name: copy.review })).toBeNull();
  expect(screen.getByText(copy.previousFindings, { exact: false })).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  const applyButton = await screen.findByRole("button", { name: shared.apply });
  expect(applyButton).toBeDisabled();
  expect(preview.mock.calls.at(-1)?.[2].skippedRecords).toEqual([2]);
  expect(preview.mock.calls.at(-1)?.[2].mapping).toMatchObject({
    dialect: "comma",
    hasHeader: true,
    expectedColumns: 3,
    personColumn: 0,
    assignmentTypeColumn: 1,
    eligibleColumn: 2,
  });
  await userEvent.click(screen.getByRole("checkbox", { name: copy.approve }));
  expect(applyButton).toBeEnabled();
  await userEvent.click(screen.getByRole("button", { name: copy.inspectRow("3") }));
  const selected = await screen.findByRole("region", { name: copy.selectedRecord });
  expect(within(selected).getByText('<img src=x onerror="window.injected=true">')).toBeVisible();
  expect(selected.querySelector("img")).toBeNull();
  expect(sample.mock.calls.at(-1)?.[2]).toEqual({
    readFormat: { family: "eligibility", dialect: "comma" },
    record: 3,
  });
  await userEvent.selectOptions(screen.getByRole("combobox", { name: shared.header }), "no");
  expect(screen.queryByRole("checkbox", { name: copy.approve })).toBeNull();
  expect(screen.queryByRole("table", { name: copy.records })).toBeNull();
  expect(apply).not.toHaveBeenCalled();
  await userEvent.selectOptions(screen.getByRole("combobox", { name: shared.header }), "yes");
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  await userEvent.click(await screen.findByRole("checkbox", { name: copy.skipRecord("2") }));
  await userEvent.click(screen.getByRole("button", { name: shared.preview }));
  await userEvent.click(await screen.findByRole("checkbox", { name: copy.approve }));
  apply.mockImplementation((_scope, input) => {
    const revision = project.revision + 1;
    api.listProjects.mockResolvedValue(
      response([{ ...project, domainPackId: "official.workforce", revision }]),
    );
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
            rows: {
              family: "eligibility",
              excludedRows: [{ record: 2, skipped: true, issue: null }],
            },
          },
        },
        [],
        revision,
      ),
    );
  });
  await userEvent.click(screen.getByRole("button", { name: shared.apply }));
  await expect.element(screen.getByRole("heading", { name: copy.report })).toHaveFocus();
});
