import { cleanup, render, screen } from "@testing-library/vue";
import { afterEach, describe, expect, it } from "vitest";
import { userEvent } from "vitest/browser";
import { h, ref, shallowRef } from "vue";
import {
  createPersonFieldsDraft,
  personFieldsValue,
  type PersonFieldsDraft,
} from "../person-fields";
import { project } from "../testing/project-home";
import PersonFields from "./PersonFields.vue";
import "../styles.css";

afterEach(cleanup);
function mountFields(initial: PersonFieldsDraft) {
  const draft = shallowRef(initial);
  render({
    render: () =>
      h(PersonFields, {
        project,
        libraryEpoch: 1,
        modelValue: draft.value,
        showErrors: true,
        locale: "en-US",
        "onUpdate:modelValue": (value: PersonFieldsDraft) => {
          draft.value = value;
        },
      }),
  });
  return draft;
}

describe("person field editing", () => {
  it("retains incomplete raw numbers and date text across an optional section toggle", async () => {
    const draft = mountFields(createPersonFieldsDraft());
    await userEvent.click(screen.getByText(/Workload and targets/u));
    await userEvent.fill(screen.getByRole("textbox", { name: "Top number" }), "2e");
    expect(draft.value.weightNumerator).toBe("2e");
    expect(personFieldsValue(draft.value).value).toBeNull();
    await userEvent.click(screen.getByText(/Dates, teams and home location/u));
    await userEvent.click(screen.getByRole("checkbox", { name: "Only active during these dates" }));
    await userEvent.fill(screen.getByRole("textbox", { name: "Active from" }), "2026-0");
    await userEvent.click(screen.getByRole("checkbox", { name: "Only active during these dates" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "Only active during these dates" }));
    await expect
      .element(screen.getByRole("textbox", { name: "Active from" }))
      .toHaveValue("2026-0");
    expect(draft.value.weightNumerator).toBe("2e");
  });

  it("bounds maximum-size draft rows and restores focus after paging and append", async () => {
    const draft = mountFields({
      ...createPersonFieldsDraft(),
      qualificationGrants: Array.from({ length: 10_000 }, (_, index) => ({
        key: `grant-${String(index)}`,
        qualificationId: "",
        effectiveFrom: "2026-09-01T08:00:00+02:00",
        expiresAt: "",
      })),
      tags: Array.from({ length: 10_000 }, (_, index) => ({
        key: `tag-${String(index)}`,
        text: `Tag ${String(index)}`,
      })),
    });
    await userEvent.click(screen.getByText(/Qualifications and work types/u));
    await userEvent.click(screen.getByText(/Tags and appearance/u));
    expect(screen.getAllByRole("combobox", { name: "Search Qualification" })).toHaveLength(50);
    expect(screen.getAllByRole("textbox", { name: /^Tag [\d,]+$/u })).toHaveLength(50);
    await userEvent.click(screen.getByRole("button", { name: "Next qualifications" }));
    await expect
      .element(screen.getByRole("heading", { name: "Qualifications and valid times" }))
      .toHaveFocus();
    expect(screen.getAllByRole("textbox", { name: "Valid from (optional time)" })).toHaveLength(50);
    await userEvent.click(screen.getByRole("button", { name: "Remove tag 1" }));
    await userEvent.click(screen.getByRole("button", { name: "Add tag" }));
    await expect.element(screen.getByRole("textbox", { name: "Tag 10,000" })).toHaveFocus();
    expect(draft.value.tags).toHaveLength(10_000);
    expect(screen.getAllByRole("textbox", { name: /^Tag [\d,]+$/u })).toHaveLength(50);
  });

  it("opens a collapsed option when native validation routes to its exact field", async () => {
    const fields = ref<InstanceType<typeof PersonFields>>();
    render({
      render: () =>
        h(PersonFields, {
          ref: fields,
          project,
          libraryEpoch: 1,
          modelValue: createPersonFieldsDraft(),
        }),
    });
    expect(screen.queryByRole("textbox", { name: "Top number" })).toBeNull();
    expect(await fields.value?.focusField(["workloadWeight", "numerator"], () => true)).toBe(true);
    await expect.element(screen.getByRole("textbox", { name: "Top number" })).toHaveFocus();
  });
});
