import { cleanup, render, screen } from "@testing-library/vue";
import { afterEach, describe, expect, it } from "vitest";
import { userEvent } from "vitest/browser";
import { h, shallowRef } from "vue";
import type { DomainEntityRef } from "../../api/generated";
import type { EntityPickerPage } from "./field-contracts";
import EntityMultiPicker from "./EntityMultiPicker.vue";
import EntityPicker from "./EntityPicker.vue";
import { plannerMessage } from "./messages";
import "../../styles.css";

function person(index: number): DomainEntityRef {
  return {
    kind: "person",
    id: `01900000-0000-7000-8000-${index.toString().padStart(12, "0")}`,
  };
}

afterEach(cleanup);

describe("Entity picker identity and focus", () => {
  it("keeps kind-qualified selection when a newer search has no current response", async () => {
    const selected = shallowRef<DomainEntityRef | null>(null);
    const team = { ...person(1), kind: "team" };
    const options = [person(1), team, person(2)].map((entity) => ({ entity, label: "Alex" }));
    const page: EntityPickerPage = {
      status: "ready",
      requestKey: "initial",
      query: "",
      options,
      hasMore: false,
    };
    render({
      render: () =>
        h(EntityPicker, {
          label: "Person",
          modelValue: selected.value,
          selectedOption:
            options.find(
              ({ entity }) =>
                entity.kind === selected.value?.kind && entity.id === selected.value.id,
            ) ?? null,
          requestKey: "initial",
          query: "",
          page,
          "onUpdate:modelValue": (value: DomainEntityRef | null) => {
            selected.value = value;
          },
        }),
    });
    const search = screen.getByRole("combobox", { name: "Search Person" });
    await userEvent.click(search);
    await expect
      .element(screen.getByRole("option", { name: new RegExp(`team.*${team.id}`, "u") }))
      .toBeVisible();
    await userEvent.click(
      screen.getByRole("option", { name: new RegExp(`team.*${team.id}`, "u") }),
    );
    expect(selected.value).toEqual(team);
    // Leave the parent page/key unchanged: local typing must reject that old page immediately.
    await userEvent.fill(search, "Alex");
    await expect.poll(() => screen.queryAllByRole("option")).toEqual([]);
    await userEvent.keyboard("{ArrowDown}{Enter}{Escape}");
    expect(selected.value).toEqual(team);
  });

  it("returns last-page removal focus to the adjacent remaining identity", async () => {
    const selected = shallowRef<readonly DomainEntityRef[]>(
      Array.from({ length: 51 }, (_, index) => person(index + 1)),
    );
    render({
      render: () =>
        h(EntityMultiPicker, {
          label: "People",
          modelValue: selected.value,
          selectedOptions: [],
          requestKey: "initial",
          query: "",
          page: { status: "idle" },
          "onUpdate:modelValue": (value: readonly DomainEntityRef[]) => {
            selected.value = value;
          },
        }),
    });
    await userEvent.click(
      screen.getByRole("button", { name: plannerMessage("entity.nextSelected") }),
    );
    await userEvent.click(
      screen.getByRole("button", {
        name: plannerMessage("entity.removeUnnamed", { ...person(51) }),
      }),
    );
    expect(selected.value).toEqual(Array.from({ length: 50 }, (_, index) => person(index + 1)));
    await expect
      .element(
        screen.getByRole("button", {
          name: plannerMessage("entity.removeUnnamed", { ...person(50) }),
        }),
      )
      .toHaveFocus();
  });

  it("keeps focus in search when requesting a page removes its paging button", async () => {
    const requestKey = shallowRef("initial");
    const page = shallowRef<EntityPickerPage>({
      status: "ready",
      requestKey: "initial",
      query: "",
      options: [],
      hasMore: true,
    });
    render({
      render: () =>
        h(EntityPicker, {
          label: "Person",
          modelValue: null,
          selectedOption: null,
          requestKey: requestKey.value,
          query: "",
          page: page.value,
          onNextPage: () => {
            requestKey.value = "next";
            page.value = { status: "loading", requestKey: "next", query: "" };
          },
        }),
    });
    await userEvent.click(
      screen.getByRole("button", { name: plannerMessage("entity.nextMatches") }),
    );
    await expect.element(screen.getByRole("combobox", { name: "Search Person" })).toHaveFocus();
  });
});
