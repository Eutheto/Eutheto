import { cleanup, render, screen } from "@testing-library/vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { h, shallowRef } from "vue";
import { project } from "../../testing/project-home";
import WorkforceEntityPicker from "./WorkforceEntityPicker.vue";
import "../../styles.css";

const native = vi.hoisted(() => ({ search: vi.fn(), detail: vi.fn() }));
vi.mock("../../api/generated", async (original) => ({
  ...(await original<typeof import("../../api/generated")>()),
  SetupOperationScope: class {
    dispose(): void {}
  },
  searchScenarioEntities: native.search,
  getScenarioEntity: native.detail,
}));
function deferred() {
  let resolve!: (value: unknown) => void;
  const result = new Promise<unknown>((done) => {
    resolve = done;
  });
  return { result, resolve };
}
const oldId = "01900000-0000-7000-8000-000000000001";
const nextId = "01900000-0000-7000-8000-000000000002";
function page(id: string, name: string) {
  return {
    result: {
      view: {
        data: {
          result: {
            data: {
              items: [{ entityId: id, kind: "person", name }],
              continuation: null,
              totalItems: 1,
            },
          },
        },
      },
    },
  };
}
beforeEach(() => {
  native.search.mockReset();
  native.detail.mockReset();
});
afterEach(cleanup);

describe("native-backed reference picker", () => {
  it("does not publish a late page from the previous revision", async () => {
    const first = deferred();
    const second = deferred();
    native.search.mockReturnValueOnce(first).mockReturnValueOnce(second);
    const revision = shallowRef(project.revision);
    const selected = shallowRef<readonly string[]>([]);
    render({
      render: () =>
        h(WorkforceEntityPicker, {
          project: { ...project, revision: revision.value },
          libraryEpoch: 1,
          kind: "person",
          label: "Person",
          modelValue: selected.value,
          "onUpdate:modelValue": (ids: readonly string[]) => {
            selected.value = ids;
          },
        }),
    });
    await userEvent.click(screen.getByRole("combobox", { name: "Search Person" }));
    revision.value++;
    await expect.poll(() => native.search.mock.calls.length).toBe(2);
    first.resolve(page(oldId, "Old revision"));
    await expect.poll(() => screen.queryAllByRole("option")).toEqual([]);
    second.resolve(page(nextId, "Current revision"));
    const current = await screen.findByRole("option", { name: /Current revision/u });
    await userEvent.click(current);
    expect(selected.value).toEqual([nextId]);
    expect(screen.queryByText("Old revision")).toBeNull();
  });

  it("keeps valid replacement choices when the selected record label cannot be read", async () => {
    native.search.mockReturnValue({ result: Promise.resolve(page(nextId, "Replacement")) });
    native.detail.mockImplementation(() => ({
      result: Promise.reject(new Error("missing selected record")),
    }));
    const selected = shallowRef<readonly string[]>([oldId]);
    render({
      render: () =>
        h(WorkforceEntityPicker, {
          project,
          libraryEpoch: 1,
          kind: "person",
          label: "Person",
          modelValue: selected.value,
          "onUpdate:modelValue": (ids: readonly string[]) => {
            selected.value = ids;
          },
        }),
    });
    await userEvent.click(screen.getByRole("combobox", { name: "Search Person" }));
    await userEvent.click(await screen.findByRole("option", { name: /Replacement/u }));
    expect(selected.value).toEqual([nextId]);
  });
});
