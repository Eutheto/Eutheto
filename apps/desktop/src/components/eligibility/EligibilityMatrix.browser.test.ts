import { cleanup, render, screen } from "@testing-library/vue";
import axe from "axe-core";
import { afterEach, describe, expect, it } from "vitest";
import { userEvent } from "vitest/browser";
import { defineComponent, h } from "vue";
import type { EligibilityWindow } from "../../eligibility-setup";
import EligibilityMatrix from "./EligibilityMatrix.vue";
import "../../styles.css";

// Renderer-only fixture: native persistence and membership semantics are exercised separately.
const people = Array.from({ length: 100 }, (_, index) => ({
  personId: `01900000-0000-7000-8000-${String(index + 1).padStart(12, "0")}`,
  name: `Person ${String(index + 1).padStart(3, "0")}`,
}));
const types = Array.from({ length: 64 }, (_, index) => ({
  entityId: `01900000-0000-7000-8001-${String(index + 1).padStart(12, "0")}`,
  kind: "assignmentType" as const,
  name: `Type ${String(index + 1).padStart(2, "0")}`,
}));
const window: EligibilityWindow = {
  context: { scenarioId: "01900000-0000-7000-8000-000000001000", revision: 1, epoch: 0 },
  filters: { peopleSearch: "", typeSearch: "", qualificationId: null },
  people: { items: people, totalItems: 100, continuation: null },
  types: { items: types, totalItems: 64, continuation: null },
  matrix: {
    personIds: people.map((person) => person.personId),
    assignmentTypeIds: types.map((type) => type.entityId),
    configuredMemberships: people.map(() => types.map(() => true)),
  },
};
function setup(disabled = false) {
  return render(
    defineComponent({
      setup: () => () =>
        h("main", [
          h(EligibilityMatrix, {
            window,
            edits: new Map(),
            disabled,
            selectedPersonId: null,
            selectedTypeId: null,
          }),
          h("button", "After matrix"),
        ]),
    }),
  );
}
afterEach(cleanup);

describe("two-axis membership keyboard access", () => {
  it("retains a focused cell offscreen and reaches the far corner without tab trapping", async () => {
    const rendered = setup();
    const first = await screen.findByRole("checkbox", { name: "Person 001 — Type 01" });
    expect((await axe.run(rendered.container)).violations).toEqual([]);
    first.focus();
    const scroll = rendered.container.querySelector<HTMLElement>("[data-matrix-scroll]");
    if (scroll === null) throw new Error("Missing matrix scroll surface");
    scroll.scrollTo({ top: 5_000, left: 8_000 });
    await expect.poll(() => scroll.scrollTop).toBeGreaterThan(4_000);
    await expect.element(first).toHaveFocus();
    await userEvent.keyboard("{Control>}{End}{/Control}");
    const last = await screen.findByRole("checkbox", { name: "Person 100 — Type 64" });
    await expect.element(last).toHaveFocus();
    await expect.element(last).toBeVisible();
    // A full 6,400-input render would violate the bounded renderer contract.
    expect(screen.getAllByRole("checkbox").length).toBeLessThan(300);
    await userEvent.tab();
    await expect.element(screen.getByRole("button", { name: "After matrix" })).toHaveFocus();
    screen.getByRole("button", { name: "Use equivalent paged table" }).focus();
    await userEvent.keyboard("{Enter}");
    await expect
      .element(await screen.findByRole("checkbox", { name: "Person 100 — Type 64" }))
      .toHaveFocus();
    await userEvent.keyboard("{Control>}{Home}{/Control}");
    await expect
      .element(await screen.findByRole("checkbox", { name: "Person 001 — Type 01" }))
      .toHaveFocus();
  });

  it("keeps stale memberships readable and focusable but prevents keyboard changes", async () => {
    setup(true);
    const first = await screen.findByRole("checkbox", { name: "Person 001 — Type 01" });
    first.focus();
    await userEvent.keyboard(" ");
    await expect.element(first).toHaveFocus();
    await expect.element(first).toBeChecked();
    await userEvent.keyboard("{ArrowRight}");
    await expect
      .element(screen.getByRole("checkbox", { name: "Person 001 — Type 02" }))
      .toHaveFocus();
  });
});
