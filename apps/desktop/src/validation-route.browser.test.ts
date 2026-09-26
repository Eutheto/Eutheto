import { PiniaColada } from "@pinia/colada";
import { cleanup, render, screen } from "@testing-library/vue";
import { flushPromises } from "@vue/test-utils";
import { userEvent } from "vitest/browser";
import { createPinia } from "pinia";
import { afterEach, expect, it, onTestFinished } from "vitest";
import { defineComponent, h, nextTick, ref } from "vue";
import { createMemoryHistory, createRouter } from "vue-router";
import { createProjectHomeController } from "./project-home";
import { fakeApi, project } from "./testing/project-home";
import { useValidationRoute } from "./validation-route";

afterEach(cleanup);

it("waits for initial reads, then leaves subsequent editing focus alone across readiness changes", async () => {
  const ready = ref(false);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/work", component: { render: () => null } }],
  });
  onTestFinished(() => {
    router.options.history.destroy();
  });
  await router.push({
    path: "/work",
    query: {
      findingPath: `/domain/entities/${project.scenarioId}/coverage/count`,
      findingRevision: String(project.revision),
      findingEpoch: "0",
    },
  });
  render(
    defineComponent({
      setup() {
        const home = createProjectHomeController(fakeApi());
        const error = useValidationRoute(
          home,
          () => project,
          () => ready.value,
          async (_target, current) => {
            await nextTick();
            if (!current()) return false;
            const field = document.getElementById("coverage-count");
            field?.focus();
            return field !== null && document.activeElement === field;
          },
        );
        return () =>
          h("main", [
            h("input", { id: "coverage-count", "aria-label": "Coverage count" }),
            h("input", { "aria-label": "Uncommitted note" }),
            error.value === null ? null : h("p", { role: "alert" }, error.value),
          ]);
      },
    }),
    { global: { plugins: [createPinia(), PiniaColada, router] } },
  );
  const count = screen.getByRole("textbox", { name: "Coverage count" });
  const note = screen.getByRole("textbox", { name: "Uncommitted note" });
  await userEvent.fill(note, "preserve this draft");
  await nextTick();
  expect(document.activeElement).toBe(note);
  expect(screen.queryByRole("alert")).toBeNull();
  ready.value = true;
  await expect.element(count).toHaveFocus();
  await userEvent.click(note);
  ready.value = false;
  await nextTick();
  ready.value = true;
  await flushPromises();
  expect(document.activeElement).toBe(note);
  await expect.element(note).toHaveValue("preserve this draft");
  expect(screen.queryByRole("alert")).toBeNull();
});
