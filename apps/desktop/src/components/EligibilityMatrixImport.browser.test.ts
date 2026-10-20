/* eslint-disable vue/one-component-per-file -- Browser routing fixtures mount the real component and route guards through a separate harness. */
import { cleanup, render, screen, within } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { defineComponent, h, nextTick, onScopeDispose, reactive, shallowRef } from "vue";
import { createMemoryHistory, createRouter, RouterView } from "vue-router";
import { afterEach, expect, it, onTestFinished, vi } from "vitest";
import { userEvent } from "vitest/browser";
import * as generated from "../api/generated";
import type { ProjectHomeController } from "../project-home";
import { createProjectHomeController } from "../project-home";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import native from "../api/fixtures/eligibility-matrix-native.json";
import { messages } from "../messages";
import EligibilityMatrixImport from "./EligibilityMatrixImport.vue";
import "../styles.css";

vi.mock("../api/generated", { spy: true });
const copy = messages.eligibilityMatrix;
const labels = messages.eligibilityCsv;
const shared = messages.csvImport;
// Captured native receipts are presentation inputs, not a replacement parser or mutation engine.
const captured = native as unknown as {
  submittedText: string;
  sourceOpened: generated.ApiResponseDto<generated.CsvSourceOpenedV1>;
  noChangesText: string;
  noChangesSourceOpened: generated.ApiResponseDto<generated.CsvSourceOpenedV1>;
  noChangesDetection: generated.ApiResponseDto<generated.CsvDetectionV1>;
  inertSample: generated.ApiResponseDto<generated.CsvSampleV1>;
  detection: generated.ApiResponseDto<generated.CsvDetectionV1>;
  sample: generated.ApiResponseDto<generated.CsvSampleV1>;
  unresolvedPreview: generated.ApiResponseDto<generated.EligibilityMatrixPreviewV1>;
  preview: generated.ApiResponseDto<generated.EligibilityMatrixPreviewV1>;
  apply: generated.ApiResponseDto<generated.CsvApplyV1>;
  report: generated.ApiResponseDto<generated.CsvReportV1>;
  noChangesPreview: generated.ApiResponseDto<generated.EligibilityMatrixPreviewV1>;
  noChangesApply: generated.ApiResponseDto<generated.CsvApplyV1>;
};
const retained = captured.preview.result;
const sourceId = retained.sourceId;
const nativeReview = retained.preview.review;
if (!nativeReview) throw new Error("Expected retained native matrix review");
const mapping = nativeReview.mapping;
const nativeSkips = nativeReview.skippedCells;
const firstSkip = nativeSkips[0];
if (!firstSkip) throw new Error("Expected an explicitly skipped native cell");
const firstPersonMapping = Object.entries(mapping.personMappings)[0];
if (!firstPersonMapping) throw new Error("Expected an explicit native person alias");
const [personToken, personId] = firstPersonMapping;
const matrixText = captured.submittedText;

function element(id: string): HTMLElement {
  const value = document.getElementById(id);
  if (!value) throw new Error(`Missing element ${id}`);
  return value;
}
function textarea(): HTMLTextAreaElement {
  const value = element("eligibility-matrix-text");
  if (!(value instanceof HTMLTextAreaElement)) throw new Error("Expected matrix textarea");
  return value;
}
async function text(
  value: string,
  inputType = "insertFromPaste",
  isComposing = false,
): Promise<void> {
  const field = textarea();
  field.value = value;
  field.dispatchEvent(new InputEvent("input", { bubbles: true, inputType, isComposing }));
  await nextTick();
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

async function setup() {
  const visible = reactive({
    ...project,
    domainPackId: "official.workforce",
    scenarioId: retained.scenarioId,
    revision: retained.revision,
  });
  const api = fakeApi([visible]);
  const dispose = vi
    .spyOn(generated.CsvImportFlow.prototype, "dispose")
    .mockResolvedValue(undefined);
  vi.spyOn(generated.CsvImportFlow.prototype, "discardPreview").mockResolvedValue(undefined);
  const paste = vi
    .spyOn(generated.CsvImportFlow.prototype, "openEligibilityMatrix")
    .mockImplementation(() => portableOperation(captured.sourceOpened));
  const detect = vi
    .spyOn(generated.CsvImportFlow.prototype, "detect")
    .mockImplementation(() => portableOperation(captured.detection));
  const preview = vi
    .spyOn(generated.CsvImportFlow.prototype, "previewEligibilityMatrix")
    .mockImplementation(() => portableOperation(captured.preview));
  const sample = vi
    .spyOn(generated.CsvImportFlow.prototype, "sample")
    .mockImplementation(() => portableOperation(captured.inertSample));
  const apply = vi.spyOn(generated.CsvImportFlow.prototype, "apply");
  const report = vi
    .spyOn(generated.CsvImportFlow.prototype, "report")
    .mockResolvedValue(captured.report);
  const save = vi
    .spyOn(generated.CsvImportFlow.prototype, "saveReport")
    .mockImplementation((_scope, previewId) =>
      portableOperation(response({ schemaVersion: 1, previewId })),
    );
  vi.mocked(generated.searchScenarioEntities).mockImplementation(() =>
    portableOperation(
      response({
        schemaVersion: 2,
        scenarioId: visible.scenarioId,
        revision: visible.revision,
        view: {
          viewId: "eutheto.setup.entity_page",
          data: {
            schemaVersion: 1,
            result: { kind: "entityPage", data: { items: [], continuation: null, totalItems: 0 } },
          },
        },
      }),
    ),
  );
  vi.mocked(generated.getScenarioEntity).mockImplementation(() =>
    portableOperation(Promise.reject(new Error("No label fixture"))),
  );
  const showing = shallowRef(true);
  let home!: ProjectHomeController;
  const Harness = defineComponent({
    setup() {
      home = createProjectHomeController(api);
      onScopeDispose(() => {
        void home.dispose();
      });
      return () =>
        showing.value
          ? h("div", [
              h(EligibilityMatrixImport, { home, project: visible }),
              h("button", { id: "outside-focus" }, "Other action"),
            ])
          : null;
    },
  });
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/", name: "project-eligibility-matrix-import", component: Harness },
      {
        path: "/eligibility/:scenarioId",
        name: "project-eligibility",
        component: { template: "<div />" },
      },
      { path: "/history/:scenarioId", name: "project-history", component: { template: "<div />" } },
    ],
  });
  onTestFinished(() => {
    router.options.history.destroy();
  });
  await router.push("/");
  render(defineComponent({ setup: () => () => h(RouterView) }), {
    global: { plugins: [createPinia(), PiniaColada, router] },
  });
  await router.isReady();
  return {
    visible,
    api,
    showing,
    home,
    paste,
    detect,
    preview,
    sample,
    apply,
    report,
    save,
    dispose,
  };
}
async function capture(value = matrixText) {
  await text(value);
  await userEvent.click(element("eligibility-matrix-capture"));
  await screen.findByRole("button", { name: shared.preview });
}
async function review() {
  await userEvent.click(element("eligibility-matrix-preview"));
  await screen.findByRole("heading", { name: labels.review });
}
async function alias(kind: "person" | "assignmentType", token: string, id: string) {
  await userEvent.selectOptions(element("eligibility-csv-mapping-endpoint"), kind);
  await userEvent.fill(element("eligibility-csv-token"), token);
  await userEvent.fill(element("eligibility-csv-endpoint-id"), id);
  await userEvent.click(screen.getByRole("button", { name: labels.addMapping }));
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

it("rejects whole overflowing DOM input without a prefix, including non-BMP, UTF-8, selection replacement and composition, and rechecks on submit", async () => {
  const view = await setup();
  const cap = generated.MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES;
  const field = textarea();
  expect(field.hasAttribute("maxlength")).toBe(false);
  for (const [value, composing] of [
    ["a".repeat(cap + 1), false],
    ["a".repeat(cap - 1) + "😀" + "SENTINEL", false],
    ["é".repeat(cap / 2 + 1), false],
    ["😀".repeat(cap / 4 + 1), true],
  ] as const) {
    await text(value, composing ? "insertCompositionText" : "insertFromPaste", composing);
    expect(field.value).toBe("");
    expect(screen.getByRole("alert")).toBeVisible();
    await userEvent.click(element("eligibility-matrix-capture"));
    expect(view.paste).not.toHaveBeenCalled();
  }
  field.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  await text("é".repeat(cap / 2 + 1), "insertCompositionText", true);
  field.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
  expect(field.value).toBe("");
  await text("a".repeat(cap));
  field.setSelectionRange(cap - 1, cap);
  field.setRangeText("😀SENTINEL", cap - 1, cap, "end");
  field.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertFromPaste" }));
  await nextTick();
  expect(field.value).toBe("");
  // A missed input event must not bypass submission's whole-value admission.
  await text("safe");
  field.value = "é".repeat(cap / 2 + 1);
  await userEvent.click(element("eligibility-matrix-capture"));
  expect(field.value).toBe("");
  expect(view.paste).not.toHaveBeenCalled();
  // After rejection a complete fresh value, rather than the old prefix, is captured.
  await text("personId\tshared\nshared\t");
  await userEvent.click(element("eligibility-matrix-capture"));
  await screen.findByRole("button", { name: shared.preview });
  expect(view.paste.mock.calls[0]?.[1]).toBe("personId\tshared\nshared\t");
});

it("uses native census, statuses, before/after facts, role aliases and exact sorted unique native cell skips without identity inference", async () => {
  const view = await setup();
  view.preview.mockImplementationOnce(() => portableOperation(captured.unresolvedPreview));
  await capture();
  expect(view.paste.mock.calls[0]?.[1]).toBe(matrixText);
  const field = textarea();
  expect(field.readOnly).toBe(true);
  const detected = captured.detection.result;
  if (detected.family !== "eligibilityMatrix") throw new Error("Expected native matrix detection");
  expect(element("eligibility-matrix-census").textContent).toContain(String(detected.data.cells));
  await review();
  const table = element("eligibility-matrix-cells");
  for (const row of captured.unresolvedPreview.result.preview.rows) {
    const rendered = table.querySelector(
      `[data-matrix-record="${String(row.cell.record)}"][data-matrix-column="${String(row.cell.column)}"]`,
    );
    if (!(rendered instanceof HTMLElement)) throw new Error("Missing native review cell");
    const cells = within(rendered).getAllByRole("cell");
    expect(cells[0]?.textContent).toBe(String(row.cell.record));
    expect(cells[1]?.textContent).toBe(String(row.cell.column + 1));
    expect(cells[2]?.textContent).toContain(row.status);
    expect(cells[3]?.textContent).toBe(row.personId ?? labels.unresolved);
    expect(cells[4]?.textContent).toBe(row.assignmentTypeId ?? labels.unresolved);
    expect(cells[5]?.textContent).toBe(row.before === null ? labels.unknown : String(row.before));
    expect(cells[6]?.textContent).toBe(row.after === null ? labels.unknown : String(row.after));
  }
  expect(screen.queryByRole("button", { name: shared.apply })).toBeNull();
  for (const [token, id] of Object.entries(mapping.personMappings))
    await alias("person", token, id);
  for (const [token, id] of Object.entries(mapping.assignmentTypeMappings))
    await alias("assignmentType", token, id);
  await review();
  for (const cell of [...nativeSkips].reverse())
    await userEvent.click(
      element(`eligibility-matrix-skip-${String(cell.record)}-${String(cell.column)}`),
    );
  // Toggle one native coordinate twice: it must never be duplicated or display-offset.
  await userEvent.click(
    element(`eligibility-matrix-skip-${String(firstSkip.record)}-${String(firstSkip.column)}`),
  );
  await userEvent.click(
    element(`eligibility-matrix-skip-${String(firstSkip.record)}-${String(firstSkip.column)}`),
  );
  expect(screen.queryByRole("heading", { name: labels.review })).toBeNull();
  await review();
  expect(view.preview.mock.calls.at(-1)?.[2]).toEqual({ mapping, skippedCells: nativeSkips });
  for (const row of retained.preview.rows) {
    const rendered = element("eligibility-matrix-cells").querySelector(
      `[data-matrix-record="${String(row.cell.record)}"][data-matrix-column="${String(row.cell.column)}"]`,
    );
    if (!(rendered instanceof HTMLElement)) throw new Error("Missing native review cell");
    const cells = within(rendered).getAllByRole("cell");
    expect(cells[2]?.textContent).toContain(row.status);
    expect(cells[5]?.textContent).toBe(row.before === null ? labels.unknown : String(row.before));
    expect(cells[6]?.textContent).toBe(row.after === null ? labels.unknown : String(row.after));
  }
  await userEvent.click(element("eligibility-matrix-read-report"));
  await screen.findByRole("table", { name: copy.excluded });
  const reportRows = captured.report.result.rows;
  if (reportRows.family !== "eligibilityMatrix") throw new Error("Expected native matrix report");
  const displayed = within(element("eligibility-matrix-excluded")).getAllByRole("row").slice(1);
  expect(displayed).toHaveLength(reportRows.excludedCells.length);
  for (const [index, exclusion] of reportRows.excludedCells.entries()) {
    const displayedRow = displayed[index];
    if (!displayedRow) throw new Error("Missing native excluded cell");
    const cells = within(displayedRow).getAllByRole("cell");
    expect(cells[0]?.textContent).toBe(String(exclusion.cell.record));
    expect(cells[1]?.textContent).toBe(String(exclusion.cell.column + 1));
    expect(cells[2]?.textContent).toBe(exclusion.skipped ? labels.yes : labels.no);
    expect(cells[3]?.textContent).toBe(exclusion.issue ?? labels.none);
  }
  expect(element("eligibility-matrix-apply")).toBeDisabled();
  await userEvent.click(element("eligibility-matrix-approve"));
  expect(element("eligibility-matrix-apply")).toBeEnabled();
  await alias("person", "another exact token", personId);
  expect(screen.queryByRole("checkbox", { name: labels.approve })).toBeNull();
  expect(view.apply).not.toHaveBeenCalled();
  await userEvent.fill(
    element("eligibility-matrix-inspect-record"),
    String(captured.inertSample.result.record),
  );
  await userEvent.click(element("eligibility-matrix-inspect"));
  const selected = await screen.findByRole("region", { name: copy.selectedRecord });
  expect(view.sample.mock.calls.at(-1)?.[2]).toEqual({
    readFormat: { family: "eligibilityMatrix" },
    record: captured.inertSample.result.record,
  });
  for (const cell of captured.inertSample.result.cells ?? [])
    if (cell.text)
      expect(within(selected).getAllByText(cell.text, { exact: true })[0]).toBeVisible();
  expect(selected.querySelector("img,script")).toBeNull();
});

it("admits the exact whole UTF-8 boundary, freezes pending capture and does not steal post-await focus", async () => {
  const view = await setup();
  const opened = deferred<generated.ApiResponseDto<generated.CsvSourceOpenedV1>>();
  view.paste.mockImplementationOnce(() => portableOperation(opened.promise));
  const complete = "é".repeat(generated.MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES / 2);
  await text(complete);
  await userEvent.click(element("eligibility-matrix-capture"));
  const field = textarea();
  expect(field).toBeDisabled();
  expect(element("eligibility-matrix-capture")).toBeDisabled();
  expect(view.paste.mock.calls[0]?.[1]).toBe(complete);
  await text("changed while capture is pending");
  expect(field.value).toBe(complete);
  element("outside-focus").focus();
  opened.resolve(
    response({
      schemaVersion: 1,
      sourceId,
      byteCount: generated.MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES,
    }),
  );
  await screen.findByRole("button", { name: shared.preview });
  expect(field.readOnly).toBe(true);
  expect(element("outside-focus")).toHaveFocus();
});

it("releases an approved old source before editing, resets mappings/skips/review, and cannot reuse old approval", async () => {
  const view = await setup();
  await capture();
  await alias("person", "local", personId);
  await review();
  await userEvent.click(element("eligibility-matrix-skip-all"));
  await review();
  await userEvent.click(element("eligibility-matrix-approve"));
  const retirement = deferred<undefined>();
  view.dispose.mockImplementationOnce(() => retirement.promise);
  await userEvent.click(element("eligibility-matrix-replace"));
  const field = textarea();
  expect(field).toBeDisabled();
  expect(field.value).toBe(matrixText);
  expect(screen.queryByRole("checkbox", { name: labels.approve })).toBeNull();
  expect(screen.queryByRole("table", { name: copy.cells })).toBeNull();
  await text("changed while cleanup is pending");
  expect(field.value).toBe(matrixText);
  retirement.resolve(undefined);
  await expect.element(field).toBeEnabled();
  expect(field.readOnly).toBe(false);
  await text("personId\tshared\nnew\tfalse");
  await userEvent.click(element("eligibility-matrix-capture"));
  await screen.findByRole("button", { name: shared.preview });
  await review();
  expect(view.preview.mock.calls.at(-1)?.[2]).toEqual({
    mapping: { personMappings: {}, assignmentTypeMappings: {} },
    skippedCells: [],
  });
  expect(view.paste.mock.calls.at(-1)?.[1]).toBe("personId\tshared\nnew\tfalse");
  expect(element("eligibility-matrix-apply")).toBeDisabled();
  expect(view.apply).not.toHaveBeenCalled();
});

it("blocks recapture after root cleanup failure until the retained root owner is successfully retired", async () => {
  const view = await setup();
  await capture();
  await review();
  view.dispose.mockRejectedValueOnce(new Error("native cleanup failed"));
  await userEvent.click(element("eligibility-matrix-replace"));
  await screen.findByRole("alert");
  expect(view.home.state.reviewCleanupError).not.toBeNull();
  expect(element("eligibility-matrix-capture")).toBeDisabled();
  await text("replacement intent");
  expect(view.paste).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("checkbox", { name: labels.approve })).toBeNull();
  await view.home.retryReviewCleanup();
  await expect.element(element("eligibility-matrix-capture")).toBeEnabled();
});

it("keeps retained native source for stale re-review, rejects late preview focus, and clears raw on scenario change and disposal", async () => {
  const view = await setup();
  await capture();
  const token = personToken;
  const person = personId;
  await alias("person", token, person);
  await review();
  const skipped = firstSkip;
  await userEvent.click(
    element(`eligibility-matrix-skip-${String(skipped.record)}-${String(skipped.column)}`),
  );
  await review();
  await userEvent.click(element("eligibility-matrix-approve"));
  view.visible.revision += 1;
  await nextTick();
  expect(screen.queryByRole("checkbox", { name: labels.approve })).toBeNull();
  expect(textarea().value).toBe(matrixText);
  expect(textarea().readOnly).toBe(true);
  const renewed = {
    ...structuredClone(captured.preview),
    result: { ...retained, revision: view.visible.revision },
  };
  view.preview.mockImplementationOnce(() => portableOperation(renewed));
  await review();
  expect(view.preview.mock.calls.at(-1)?.[1]).toBe(sourceId);
  expect(view.preview.mock.calls.at(-1)?.[2]).toEqual({
    mapping: { personMappings: { [token]: person }, assignmentTypeMappings: {} },
    skippedCells: [skipped],
  });
  expect(view.paste).toHaveBeenCalledTimes(1);
  const pending = deferred<generated.ApiResponseDto<generated.EligibilityMatrixPreviewV1>>();
  view.preview.mockImplementationOnce(() => portableOperation(pending.promise));
  await userEvent.click(element("eligibility-matrix-preview"));
  expect(element("eligibility-matrix-text")).toBeDisabled();
  expect(element("eligibility-matrix-replace")).toBeDisabled();
  element("outside-focus").focus();
  view.visible.revision += 1;
  pending.resolve(renewed);
  await expect.element(element("eligibility-matrix-preview")).toBeEnabled();
  expect(element("outside-focus")).toHaveFocus();
  expect(screen.queryByRole("checkbox", { name: labels.approve })).toBeNull();
  view.visible.scenarioId = "01900000-0000-7000-8000-000000000099";
  await nextTick();
  const field = textarea();
  expect(field.value).toBe("");
  expect(field.readOnly).toBe(false);
  await text("unsubmitted raw intent");
  view.showing.value = false;
  await nextTick();
  expect(field.value).toBe("");
});

it("quarantines a lost apply outcome in History with no replay, replacement or recapture", async () => {
  const view = await setup();
  await capture();
  await review();
  await userEvent.click(element("eligibility-matrix-approve"));
  view.apply.mockImplementation(() => portableOperation(Promise.reject(new Error("receipt lost"))));
  await userEvent.click(element("eligibility-matrix-apply"));
  await screen.findByRole("link", { name: labels.history });
  expect(view.home.state.mutation?.outcome).toBe("outcomeUnknown");
  expect(element("eligibility-matrix-text")).toBeDisabled();
  expect(element("eligibility-matrix-replace")).toBeDisabled();
  expect(screen.queryByRole("button", { name: shared.apply })).toBeNull();
  expect(view.apply).toHaveBeenCalledTimes(1);
  expect(view.paste).toHaveBeenCalledTimes(1);
  expect(element("eligibility-matrix-history").getAttribute("href")).toContain("/history/");
});

it("consumes a native no-op without another revision, clears raw and keeps the complete native exclusion report and save action", async () => {
  const view = await setup();
  const noChange = captured.noChangesPreview.result;
  view.visible.scenarioId = noChange.scenarioId;
  view.visible.revision = noChange.revision;
  view.paste.mockImplementation(() => portableOperation(captured.noChangesSourceOpened));
  view.detect.mockImplementation(() => portableOperation(captured.noChangesDetection));
  view.preview.mockImplementation(() => portableOperation(captured.noChangesPreview));
  view.apply.mockImplementation(() => portableOperation(captured.noChangesApply));
  const revision = view.visible.revision;
  await capture(captured.noChangesText);
  await review();
  await userEvent.click(element("eligibility-matrix-approve"));
  await userEvent.click(element("eligibility-matrix-apply"));
  await expect.element(element("eligibility-matrix-report")).toHaveFocus();
  expect(view.visible.revision).toBe(revision);
  expect(view.home.state.mutation?.outcome).toBe("noChanges");
  expect(textarea().value).toBe("");
  expect(textarea().readOnly).toBe(true);
  expect(screen.queryByRole("button", { name: shared.preview })).toBeNull();
  expect(screen.queryByRole("button", { name: shared.apply })).toBeNull();
  await userEvent.click(element("eligibility-matrix-save-report"));
  await expect.poll(() => view.save.mock.calls.length).toBe(1);
  expect(view.save.mock.calls[0]?.[1]).toBe(captured.noChangesApply.result.report.previewId);
});

it("pages every native review cell and complete exclusion, saves the whole report, and focuses only an exact confirmed apply receipt", async () => {
  const view = await setup();
  // Synthetic volume at the presentation seam only; cell facts themselves come from native review.
  const template = retained.preview.rows[0];
  if (!template) throw new Error("Expected a native review cell for presentation pagination");
  const rows = Array.from({ length: 103 }, (_, index) => ({
    ...template,
    cell: { record: index + 2, column: 1 },
  }));
  const paged = {
    ...structuredClone(captured.preview),
    result: { ...retained, preview: { ...retained.preview, rows } },
  };
  view.preview.mockImplementation(() => portableOperation(paged));
  await capture();
  await review();
  expect(within(element("eligibility-matrix-cells")).getAllByRole("row")).toHaveLength(51);
  await userEvent.click(element("eligibility-matrix-next-cells"));
  expect(within(element("eligibility-matrix-cells")).getAllByRole("row")).toHaveLength(51);
  await userEvent.click(element("eligibility-matrix-next-cells"));
  expect(within(element("eligibility-matrix-cells")).getAllByRole("row")).toHaveLength(4);
  const exclusions = rows.map((row) => ({ cell: row.cell, skipped: true, issue: row.issue }));
  const report = {
    ...captured.report.result,
    previewId: paged.result.previewId,
    rows: { family: "eligibilityMatrix" as const, excludedCells: exclusions },
  };
  view.report.mockResolvedValue(response(report));
  await userEvent.click(element("eligibility-matrix-read-report"));
  await screen.findByRole("table", { name: copy.excluded });
  expect(within(element("eligibility-matrix-excluded")).getAllByRole("row")).toHaveLength(51);
  await userEvent.click(element("eligibility-matrix-next-exclusions"));
  await userEvent.click(element("eligibility-matrix-next-exclusions"));
  expect(within(element("eligibility-matrix-excluded")).getAllByRole("row")).toHaveLength(4);
  await userEvent.click(element("eligibility-matrix-save-report"));
  await expect.poll(() => view.save.mock.calls.length).toBe(1);
  expect(view.save.mock.calls[0]?.[1]).toBe(paged.result.previewId);
  view.apply.mockImplementation((_scope, input) => {
    const receipt = {
      ...structuredClone(captured.apply),
      result: {
        ...captured.apply.result,
        sourceId,
        scenarioId: view.visible.scenarioId,
        outcome: {
          kind: "applied" as const,
          revision: view.visible.revision + 1,
          commandId: input.commandId,
        },
        report: { ...report, consumed: true },
      },
    };
    view.visible.revision = receipt.result.outcome.revision;
    view.api.listProjects.mockResolvedValue(response([{ ...view.visible }]));
    return portableOperation(receipt);
  });
  await userEvent.click(element("eligibility-matrix-approve"));
  await userEvent.click(element("eligibility-matrix-apply"));
  await expect.element(element("eligibility-matrix-report")).toHaveFocus();
  expect(textarea().value).toBe("");
  expect(view.apply).toHaveBeenCalledTimes(1);
});
