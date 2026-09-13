import { cleanup, render, screen, within } from "@testing-library/vue";
import { afterEach, describe, expect, it } from "vitest";
import { userEvent } from "vitest/browser";
import { h, nextTick, shallowRef } from "vue";
import type { PeopleCsvColumn } from "../../api/generated";
import type {
  WorkforceScope,
  WorkforceSetupScopeInspection,
} from "../../api/generated-domain-pack-contracts";
import ImportMappingTable from "./ImportMappingTable.vue";
import type { ImportMappingSample } from "./import-mapping";
import { plannerMessage } from "./messages";
import RuleScopeBuilder from "./RuleScopeBuilder.vue";
import type { RuleScopeBuilderProps, ScopePreviewState } from "./scope-field";
import "../../styles.css";

const emptyEntities = {
  requestKey: "entities",
  query: "",
  page: { status: "idle" as const },
  selectedOptions: [],
};
const entityOptions: RuleScopeBuilderProps["entityOptions"] = {
  people: emptyEntities,
  teamIds: emptyEntities,
  assignmentTypeIds: emptyEntities,
  locationIds: emptyEntities,
};
const inspection: WorkforceSetupScopeInspection = {
  part: "main",
  rule: { class: "required", ruleId: "01900000-0000-7000-8000-000000000001" },
  peopleCount: 7,
  shiftCount: 3,
  cartesianPairCount: 21,
  population: {
    axis: "people",
    page: {
      continuation: null,
      totalItems: 7,
      items: [{ personId: "01900000-0000-7000-8000-000000000002", name: "Native person" }],
    },
  },
};

function mappingControl(position: number, index: number, control: "source" | "field" | "blank") {
  return screen.getByRole("combobox", {
    name: `${plannerMessage("mapping.row", { position, column: index + 1 })} ${plannerMessage(`mapping.${control}`)}`,
  });
}

afterEach(cleanup);

describe("Scope draft identity and native preview freshness", () => {
  it("hides old counts before a delayed draft echo and keeps exact tag text", async () => {
    const scope = shallowRef<WorkforceScope>({
      people: { kind: "filter", allTags: ["old"], anyTags: ["Keep CASE"] },
    });
    let pending: WorkforceScope | undefined;
    const requestKey = shallowRef("initial");
    const preview = shallowRef<ScopePreviewState>({
      status: "ready",
      requestKey: "initial",
      inspection,
    });
    render({
      render: () =>
        h(RuleScopeBuilder, {
          label: "Rule scope",
          modelValue: scope.value,
          entityOptions,
          preview: preview.value,
          currentRequestKey: requestKey.value,
          previewAxis: "people",
          previewLimit: 1,
          "onUpdate:modelValue": (value: WorkforceScope) => {
            pending = value;
          },
        }),
    });
    await expect.element(screen.getByText("21", { selector: "dd" })).toBeVisible();
    const exactTag = "  ICU, Night;Case\nβ  ";
    await userEvent.fill(
      screen.getByRole("textbox", {
        name: plannerMessage("scope.entry", { label: plannerMessage("scope.allTags"), number: 1 }),
      }),
      exactTag,
    );
    expect(pending).toEqual({
      people: { kind: "filter", allTags: [exactTag], anyTags: ["Keep CASE"] },
    });
    // The parent deliberately has not echoed either the draft or a new request key.
    await expect
      .element(screen.getByRole("status"))
      .toHaveTextContent(plannerMessage("scope.previewStale"));
    expect(screen.queryAllByRole("definition")).toEqual([]);
    expect(screen.queryByText("Native person")).toBeNull();
    if (pending === undefined) throw new Error("The edited scope was not emitted");
    scope.value = pending;
    await nextTick();
    expect(screen.queryAllByRole("definition")).toEqual([]);
    requestKey.value = "edited";
    await nextTick();
    // Rotating the request alone must not admit the old response.
    expect(screen.queryAllByRole("definition")).toEqual([]);
    preview.value = { status: "ready", requestKey: "edited", inspection };
    await nextTick();
    await expect.element(screen.getByText("21", { selector: "dd" })).toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: plannerMessage("scope.previewAction") }),
    );
    await expect.element(screen.getByRole("status")).toHaveFocus();
    expect(screen.queryAllByRole("definition")).toEqual([]);
  });

  it("preserves explicit empty filters and moves removal focus to the next entry or add button", async () => {
    const scope = shallowRef<WorkforceScope>({ people: { kind: "all" } });
    render({
      render: () =>
        h(RuleScopeBuilder, {
          label: "Rule scope",
          modelValue: scope.value,
          entityOptions,
          preview: { status: "idle" },
          currentRequestKey: "idle",
          previewAxis: "people",
          previewLimit: 1,
          "onUpdate:modelValue": (value: WorkforceScope) => {
            scope.value = value;
          },
        }),
    });
    const label = plannerMessage("scope.categories");
    const add = () =>
      screen.getByRole("button", { name: plannerMessage("scope.addEntry", { label }) });
    const entry = (number: number) =>
      screen.getByRole("textbox", {
        name: plannerMessage("scope.entry", { label, number }),
      });
    const remove = (number: number) =>
      screen.getByRole("button", {
        name: plannerMessage("scope.removeEntry", { label, number }),
      });
    await userEvent.click(
      screen.getByRole("button", { name: plannerMessage("scope.enable", { label }) }),
    );
    expect(scope.value.categories).toEqual([]);
    await userEvent.click(add());
    await userEvent.fill(entry(1), "first");
    await userEvent.click(add());
    await userEvent.fill(entry(2), "second");
    await userEvent.click(remove(1));
    expect(scope.value.categories).toEqual(["second"]);
    await expect.element(entry(1)).toHaveValue("second");
    await expect.element(entry(1)).toHaveFocus();
    await userEvent.click(remove(1));
    expect(scope.value.categories).toEqual([]);
    await expect.element(add()).toHaveFocus();
    await userEvent.click(
      screen.getByRole("button", { name: plannerMessage("scope.removeFilter", { label }) }),
    );
    expect(Object.hasOwn(scope.value, "categories")).toBe(false);
    expect(
      screen.queryByRole("textbox", { name: plannerMessage("scope.entry", { label, number: 1 }) }),
    ).toBeNull();

    const weekdays = plannerMessage("scope.weekdays");
    await userEvent.click(
      screen.getByRole("button", { name: plannerMessage("scope.enable", { label: weekdays }) }),
    );
    await userEvent.click(screen.getByRole("checkbox", { name: plannerMessage("scope.monday") }));
    expect(scope.value.weekdays).toEqual(["monday"]);
    await userEvent.click(screen.getByRole("checkbox", { name: plannerMessage("scope.monday") }));
    expect(scope.value.weekdays).toEqual([]);
    await userEvent.click(
      screen.getByRole("button", {
        name: plannerMessage("scope.removeFilter", { label: weekdays }),
      }),
    );
    expect(Object.hasOwn(scope.value, "weekdays")).toBe(false);
  });
});

describe("Physical CSV mapping repair and inert native metadata", () => {
  it("distinguishes duplicate headers by zero-based source index and repairs a duplicate mapping without dropping either row", async () => {
    const columns = shallowRef<readonly PeopleCsvColumn[]>([]);
    render({
      render: () =>
        h(ImportMappingTable, {
          label: "People mapping",
          modelValue: columns.value,
          expectedColumns: 2,
          headerCells: [
            { text: "Name", truncated: false },
            { text: "Name", truncated: false },
          ],
          "onUpdate:modelValue": (value: readonly PeopleCsvColumn[]) => {
            columns.value = value;
          },
        }),
    });
    await userEvent.selectOptions(
      screen.getByRole("combobox", {
        name: `${plannerMessage("mapping.column", { column: 2 })} ${plannerMessage("mapping.field")}`,
      }),
      "externalId",
    );
    await userEvent.selectOptions(
      screen.getByRole("combobox", {
        name: `${plannerMessage("mapping.column", { column: 1 })} ${plannerMessage("mapping.field")}`,
      }),
      "name",
    );
    expect(columns.value.map(({ index, field }) => ({ index, field }))).toEqual([
      { index: 1, field: "externalId" },
      { index: 0, field: "name" },
    ]);
    await userEvent.selectOptions(mappingControl(1, 1, "source"), "0");
    expect(columns.value.map(({ index }) => index)).toEqual([0, 0]);
    await expect.element(mappingControl(1, 0, "source")).toHaveAttribute("aria-invalid", "true");
    await expect.element(mappingControl(2, 0, "source")).toHaveAttribute("aria-invalid", "true");
    await userEvent.selectOptions(mappingControl(1, 0, "source"), "1");
    expect(columns.value.map(({ index, field }) => ({ index, field }))).toEqual([
      { index: 1, field: "externalId" },
      { index: 0, field: "name" },
    ]);
    await expect.element(mappingControl(1, 1, "source")).not.toHaveAttribute("aria-invalid");
    expect(screen.queryByText(plannerMessage("mapping.duplicateIndex"))).toBeNull();
  });

  it("keeps an invalid source and Clear policy until explicitly repaired while rendering bounded samples as text", async () => {
    const columns = shallowRef<readonly PeopleCsvColumn[]>([
      { index: 9, field: "name", blank: "clear" },
    ]);
    const markup = '<img src="x" onerror="alert(1)">';
    const samples: readonly ImportMappingSample[] = [
      {
        record: 2,
        cells: [
          { text: markup, truncated: false },
          { text: "é".repeat(40), truncated: false },
        ],
      },
      {
        record: 3,
        cells: [
          { text: "=SUM(A1:A2)", truncated: false },
          { text: "native prefix", truncated: true },
        ],
      },
      { record: 4, cells: [{ text: "omitted third record", truncated: false }] },
    ];
    const { container } = render({
      render: () =>
        h(ImportMappingTable, {
          label: "People mapping",
          modelValue: columns.value,
          expectedColumns: 2,
          selectedSamples: samples,
          "onUpdate:modelValue": (value: readonly PeopleCsvColumn[]) => {
            columns.value = value;
          },
        }),
    });
    await expect.element(mappingControl(1, 9, "source")).toHaveValue("9");
    await expect.element(mappingControl(1, 9, "blank")).toHaveValue("clear");
    await expect.element(screen.getByText(plannerMessage("mapping.indexRange"))).toBeVisible();
    await userEvent.selectOptions(mappingControl(1, 9, "field"), "activeRange");
    expect(columns.value).toEqual([{ index: 9, field: "activeRange", blank: "clear" }]);
    await expect.element(mappingControl(1, 9, "blank")).toHaveValue("clear");
    await userEvent.selectOptions(mappingControl(1, 9, "source"), "1");
    expect(columns.value).toEqual([{ index: 1, field: "activeRange", blank: "clear" }]);
    expect(screen.queryByText(plannerMessage("mapping.indexRange"))).toBeNull();
    await expect.element(mappingControl(1, 1, "blank")).toHaveAttribute("aria-invalid", "true");
    await userEvent.selectOptions(mappingControl(1, 1, "blank"), "preserve");
    expect(columns.value).toEqual([{ index: 1, field: "activeRange", blank: "preserve" }]);
    await expect.element(mappingControl(1, 1, "blank")).not.toHaveAttribute("aria-invalid");

    const table = screen.getByRole("table", { name: "People mapping" });
    await expect.element(within(table).getByText(markup, { exact: false })).toBeVisible();
    await expect.element(within(table).getByText("=SUM(A1:A2)", { exact: false })).toBeVisible();
    expect(container.querySelector("img, script, iframe")).toBeNull();
    const bounded = within(table).getByText("é".repeat(32), { exact: false });
    await expect.element(bounded).toHaveTextContent(plannerMessage("mapping.truncated"));
    expect(bounded.textContent).not.toContain("é".repeat(33));
    await expect
      .element(within(table).getByText("native prefix", { exact: false }))
      .toHaveTextContent(plannerMessage("mapping.truncated"));
    expect(screen.queryByText("omitted third record", { exact: false })).toBeNull();
  });
});
