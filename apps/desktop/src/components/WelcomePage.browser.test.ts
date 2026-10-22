import { cleanup, render, screen } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { defineComponent, h } from "vue";
import { createMemoryHistory, createRouter, RouterView } from "vue-router";
import { messages } from "../messages";
import { createProjectHomeController } from "../project-home";
import { fakeApi } from "../testing/project-home";
import WelcomePage from "./WelcomePage.vue";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

async function renderCreation() {
  const api = fakeApi();
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      {
        path: "/new",
        component: defineComponent({
          setup() {
            const home = createProjectHomeController(api);
            onTestFinished(() => home.dispose());
            return () => h(WelcomePage, { home, startCreate: true });
          },
        }),
      },
      {
        path: "/project/:scenarioId",
        name: "project-setup",
        component: { template: "<p>Created</p>" },
      },
      { path: "/", name: "welcome", component: { template: "<p>Welcome</p>" } },
    ],
  });
  await router.push("/new");
  render(RouterView, { global: { plugins: [createPinia(), PiniaColada, router] } });
  await router.isReady();
  return {
    api,
    first: screen.getByLabelText<HTMLInputElement>(messages.projects.planningStarts),
    last: screen.getByLabelText<HTMLInputElement>(messages.projects.planningEnds),
    title: screen.getByLabelText(messages.projects.title),
    submit: screen.getByRole("button", { name: messages.projects.create }),
  };
}

describe("project creation planning dates", () => {
  it("requires both dates explicitly and sends the exact entered inclusive dates", async () => {
    const { api, first, last, title, submit } = await renderCreation();
    expect(first.value).toBe("");
    expect(last.value).toBe("");
    expect(first.validity.valueMissing).toBe(true);
    expect(last.validity.valueMissing).toBe(true);
    await userEvent.fill(title, "Clinic roster");
    await userEvent.click(submit);
    await expect.element(first).toHaveFocus();
    expect(api.createProject).not.toHaveBeenCalled();
    await userEvent.fill(first, "2026-09-01");
    await userEvent.click(submit);
    await expect.element(last).toHaveFocus();
    expect(api.createProject).not.toHaveBeenCalled();
    await expect
      .element(screen.getByText(/Enter both dates explicitly; no dates are selected by default\./u))
      .toBeVisible();
    await userEvent.fill(last, "2026-09-01");
    await userEvent.click(submit);
    expect(api.createProject).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({
        settings: expect.objectContaining({ firstDate: "2026-09-01", lastDate: "2026-09-01" }),
      }),
    );
  });

  it.each(["first", "last"] as const)(
    "keeps an accessible range error until the %s date is corrected",
    async (corrected) => {
      const { api, first, last, title, submit } = await renderCreation();
      await userEvent.fill(title, "Clinic roster");
      await userEvent.fill(first, "2027-01-01");
      await userEvent.fill(last, "2026-12-31");
      await userEvent.click(submit);
      expect(api.createProject).not.toHaveBeenCalled();
      await expect.element(last).toHaveFocus();
      const error = screen.getByRole("alert");
      await expect
        .element(error)
        .toHaveTextContent("The last planning date must be on or after the first planning date.");
      await expect.element(last).toHaveAttribute("aria-invalid", "true");
      expect(last.getAttribute("aria-describedby")?.split(" ")).toContain(error.id);
      await userEvent.click(title);
      await expect.element(error).toBeVisible();
      await userEvent.keyboard("{Enter}");
      expect(api.createProject).not.toHaveBeenCalled();
      await userEvent.fill(
        corrected === "first" ? first : last,
        corrected === "first" ? "2026-12-31" : "2027-01-01",
      );
      await expect.element(last).not.toHaveAttribute("aria-invalid", "true");
      expect(screen.queryByRole("alert")).toBeNull();
      await userEvent.click(submit);
      expect(api.createProject).toHaveBeenCalledExactlyOnceWith(
        expect.objectContaining({
          title: "Clinic roster",
          settings: expect.objectContaining({
            firstDate: corrected === "first" ? "2026-12-31" : "2027-01-01",
            lastDate: corrected === "first" ? "2026-12-31" : "2027-01-01",
          }),
        }),
      );
    },
  );
});
