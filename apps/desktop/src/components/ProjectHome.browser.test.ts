import { cleanup, render, screen, within } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import axe from "axe-core";
import { createPinia } from "pinia";
import { afterEach, describe, expect, it, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { defineComponent, h, onMounted, onUnmounted } from "vue";

import { createProjectHomeController, type ProjectHomeApi } from "../project-home";
import type { ApiResponseDto } from "../api/generated";
import * as generatedApi from "../api/generated";
import App from "../App.vue";
import { createAppRouter } from "../router";
import { fakeApi, project, response } from "../testing/project-home";
import ProjectHome from "./ProjectHome.vue";
import "../styles.css";

vi.mock("../api/generated", { spy: true });

async function renderHome(api: ProjectHomeApi): Promise<void> {
  render(
    defineComponent({
      setup() {
        const home = createProjectHomeController(api);
        onMounted(() => {
          void home.load();
        });
        onUnmounted(() => {
          void home.dispose();
        });
        return () => h("main", [h("h1", "Eutheto"), h(ProjectHome, { home })]);
      },
    }),
    {
      global: { plugins: [createPinia(), PiniaColada] },
    },
  );
  await screen.findByRole("button", { name: `Open project ${project.title}` });
}

afterEach(() => {
  cleanup();
});

describe("ProjectHome confirmations in the browser", () => {
  it("opens by keyboard, contains focus, restores it on Escape/Keep, and deletes only after confirmation", async () => {
    const other = {
      ...project,
      scenarioId: "01900000-0000-7000-8000-000000000002",
      title: "Weekend roster",
    };
    const library = [project, other];
    const api = fakeApi(library);
    api.deleteProject.mockImplementationOnce(() => {
      library.splice(0, 1);
      return Promise.resolve(response({}));
    });
    await renderHome(api);
    const trigger = screen.getByRole("button", { name: "Delete project" });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    const dialog = await screen.findByRole("dialog", {
      name: `Permanently delete ${project.title}?`,
    });
    const keep = within(dialog).getByRole("button", { name: "Keep project" });
    const confirm = within(dialog).getByRole("button", { name: "Delete permanently" });
    await expect.element(keep).toHaveFocus();
    await expect
      .element(dialog)
      .toHaveAccessibleDescription(
        "This removes the saved project and its history. This action cannot be undone.",
      );
    await userEvent.tab({ shift: true });
    await expect.element(confirm).toHaveFocus();
    await userEvent.tab();
    await expect.element(keep).toHaveFocus();
    await userEvent.tab();
    await expect.element(confirm).toHaveFocus();
    await userEvent.tab();
    await expect.element(keep).toHaveFocus();
    // Automated DOM checks complement, but do not replace, native/screen-reader checks.
    const accessibility = await axe.run(dialog);
    expect(accessibility.violations).toEqual([]);

    await userEvent.keyboard("{Escape}");
    await expect.element(trigger).toHaveFocus();
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect
      .element(screen.getByRole("button", { name: `Open project ${project.title}` }))
      .toBeVisible();
    await userEvent.keyboard("{Enter}");
    await expect.element(await screen.findByRole("button", { name: "Keep project" })).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await expect.element(trigger).toHaveFocus();
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();

    await userEvent.keyboard("{Enter}");
    await screen.findByRole("dialog");
    await userEvent.tab();
    await userEvent.keyboard("{Enter}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect
      .poll(() => screen.queryByRole("button", { name: `Open project ${project.title}` }))
      .toBeNull();
    await expect
      .element(screen.getByRole("button", { name: "Open project Weekend roster" }))
      .toBeVisible();
    await expect.element(screen.getByRole("heading", { name: "Projects" })).toHaveFocus();
  });

  it("returns to the usable empty library after deleting its final project", async () => {
    const library = [project];
    const api = fakeApi(library);
    api.deleteProject.mockImplementationOnce(() => {
      library.splice(0);
      return Promise.resolve(response({}));
    });
    await renderHome(api);
    await userEvent.click(screen.getByRole("button", { name: "Delete project" }));
    await screen.findByRole("dialog");
    await userEvent.tab();
    await userEvent.keyboard("{Enter}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect
      .poll(() => screen.queryByRole("button", { name: `Open project ${project.title}` }))
      .toBeNull();
    await expect.element(screen.getByRole("heading", { name: "Projects" })).toHaveFocus();
    await expect.element(screen.getByRole("button", { name: "Create project" })).toBeEnabled();
  });

  it("does not dismiss an in-flight delete and keeps a safe API failure reviewable", async () => {
    const api = fakeApi([project]);
    const deletion = Promise.withResolvers<ApiResponseDto<unknown>>();
    api.deleteProject.mockReturnValueOnce(deletion.promise);
    await renderHome(api);
    const trigger = screen.getByRole("button", { name: "Delete project" });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    const dialog = await screen.findByRole("dialog");
    const keep = within(dialog).getByRole("button", { name: "Keep project" });
    const confirm = within(dialog).getByRole("button", { name: "Delete permanently" });
    await userEvent.tab();
    await userEvent.keyboard("{Enter}");
    await expect.element(confirm).toBeDisabled();
    await userEvent.keyboard("{Escape}");
    await expect.element(dialog).toBeVisible();
    keep.focus();
    await userEvent.keyboard("{Enter}");
    await expect.element(dialog).toBeVisible();
    await expect.element(within(dialog).getByRole("status")).toHaveTextContent("Deleting project");

    deletion.reject({
      category: "storage",
      code: "local_library.unavailable",
      message: "The project could not be removed from local storage.",
    });
    const alert = await within(dialog).findByRole("alert");
    await expect
      .element(alert)
      .toHaveTextContent("The project could not be removed from local storage.");
    await expect.element(confirm).toBeEnabled();
    await expect.element(keep).not.toHaveAttribute("aria-disabled", "true");
    await userEvent.keyboard("{Escape}");
    await expect.element(trigger).toHaveFocus();
    await expect
      .element(screen.getByRole("button", { name: `Open project ${project.title}` }))
      .toBeVisible();
  });

  it("keeps a failed safety backup and explicit bypass reachable in the restore modal", async () => {
    const api = fakeApi([project]);
    api.applyRestore.mockRejectedValueOnce({
      category: "protocol",
      code: "restore.safety_backup_failed",
      message: "The private backup destination is unavailable.",
      retryable: false,
    });
    await renderHome(api);
    await userEvent.click(screen.getByText("Restore backup", { selector: "summary" }));
    await userEvent.click(screen.getByRole("radio", { name: /Replace library/ }));
    await userEvent.click(screen.getByRole("button", { name: "Choose backup file" }));
    const trigger = await screen.findByRole("button", { name: "Review and confirm restore" });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    let dialog = await screen.findByRole("dialog", { name: "Confirm library replacement" });
    await expect.element(within(dialog).getByRole("button", { name: "Go back" })).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    await expect.element(trigger).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    dialog = await screen.findByRole("dialog", { name: "Confirm library replacement" });
    await userEvent.tab();
    await userEvent.keyboard("{Enter}");
    await expect
      .element(await within(dialog).findByRole("alert"))
      .toHaveTextContent("The private backup destination is unavailable.");
    const bypass = within(dialog).getByRole("textbox", {
      name: "Continue without a safety backup",
    });
    await userEvent.fill(bypass, "REPLACE WITHOUT BACKUP");
    await expect
      .element(dialog)
      .toHaveAccessibleDescription(
        "You are requesting replacement of the current library without a safety backup after reviewing the failure.",
      );
    await userEvent.tab();
    await expect.element(within(dialog).getByRole("button", { name: "Go back" })).toHaveFocus();
    await userEvent.tab();
    await userEvent.keyboard("{Enter}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect.element(screen.getByRole("heading", { name: "Projects" })).toHaveFocus();
    await expect
      .poll(() => screen.queryByRole("button", { name: "Review and confirm restore" }))
      .toBeNull();
    await expect.element(screen.getByRole("button", { name: "Choose backup file" })).toBeEnabled();
  });

  it("does not replace the library when Enter follows Escape during restore dismissal", async () => {
    const library = [project];
    const api = fakeApi(library);
    api.applyRestore.mockImplementationOnce(() => {
      library.splice(0);
      return Promise.resolve(response({}));
    });
    await renderHome(api);
    await userEvent.click(screen.getByText("Restore backup", { selector: "summary" }));
    await userEvent.click(screen.getByRole("radio", { name: /Replace library/ }));
    await userEvent.click(screen.getByRole("button", { name: "Choose backup file" }));
    const trigger = await screen.findByRole("button", { name: "Review and confirm restore" });
    await userEvent.click(trigger);
    const dialog = await screen.findByRole("dialog", { name: "Confirm library replacement" });
    // Widen the real exit animation so the cancelled-but-mounted interval is deterministic.
    dialog.style.animationDuration = "2s";
    await userEvent.tab();
    await expect
      .element(within(dialog).getByRole("button", { name: "Confirm restore" }))
      .toHaveFocus();
    await userEvent.keyboard("{Escape}");
    expect(dialog.isConnected).toBe(true);
    await userEvent.keyboard("{Enter}");
    await expect.element(trigger).toHaveFocus();
    await expect
      .element(screen.getByRole("button", { name: `Open project ${project.title}` }))
      .toBeVisible();
    expect(library).toEqual([project]);
  });

  it("keeps an invalidated restore failure visible and returns to choosing a fresh backup", async () => {
    const api = fakeApi([project]);
    api.applyRestore.mockRejectedValueOnce({
      category: "storage",
      code: "restore.failed",
      message: "The reviewed backup is no longer available.",
    });
    await renderHome(api);
    await userEvent.click(screen.getByText("Restore backup", { selector: "summary" }));
    const choose = screen.getByRole("button", { name: "Choose backup file" });
    await userEvent.click(choose);
    await userEvent.click(
      await screen.findByRole("button", { name: "Review and confirm restore" }),
    );
    const dialog = await screen.findByRole("dialog", { name: "Confirm backup restore" });
    await userEvent.tab();
    await userEvent.keyboard("{Enter}");
    await expect
      .element(
        await within(dialog).findByText("The reviewed backup is no longer available.", {
          exact: false,
        }),
      )
      .toBeVisible();
    await expect
      .element(within(dialog).getByRole("button", { name: "Confirm restore" }))
      .toBeDisabled();
    await userEvent.keyboard("{Escape}");
    await expect.element(choose).toHaveFocus();
  });
  it("reports committed deletion visibly without stale root selection when refresh fails", async () => {
    vi.mocked(generatedApi.listProjects)
      .mockResolvedValueOnce(response([project]))
      .mockRejectedValueOnce(new Error("Publication refresh unavailable"))
      .mockRejectedValueOnce(new Error("Independent refresh unavailable"))
      .mockResolvedValue(response([]));
    vi.mocked(generatedApi.deleteProject).mockResolvedValue(response({}));
    vi.mocked(generatedApi.onAppNotification).mockResolvedValue(() => {});
    vi.mocked(generatedApi.onLibraryRefreshRequired).mockResolvedValue(() => {});
    vi.mocked(generatedApi.onScenarioChanged).mockResolvedValue(() => {});
    vi.mocked(generatedApi.onScenarioValidationChanged).mockResolvedValue(() => {});
    render(App, {
      global: { plugins: [createPinia(), PiniaColada, createAppRouter()] },
    });
    await screen.findByRole("button", { name: `Open project ${project.title}` });
    await userEvent.click(screen.getByRole("button", { name: "Delete project" }));
    await screen.findByRole("dialog");
    await userEvent.tab();
    await userEvent.keyboard("{Enter}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    expect.soft(screen.queryByText(`Selected project: ${project.title}`)).toBeNull();
    const status = screen.getByRole("status");
    await expect.element(status).toHaveTextContent(`Deleted ${project.title}.`);
    // A screen-reader-only 1px box is not visible committed-success feedback.
    expect.soft(status.getBoundingClientRect().height).toBeGreaterThan(1);
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await expect.poll(() => screen.queryByText(`Deleted ${project.title}.`)).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await expect.element(screen.getByRole("button", { name: "Create project" })).toBeEnabled();
    expect(generatedApi.deleteProject).toHaveBeenCalledTimes(1);
  });

  it("disables active and archived project selection until a pending native action settles", async () => {
    const archived = {
      ...project,
      scenarioId: "01900000-0000-7000-8000-000000000002",
      title: "Archived roster",
      archived: true,
    };
    const api = fakeApi([project, archived]);
    const pending = Promise.withResolvers<ApiResponseDto<unknown>>();
    api.setProjectArchived.mockReturnValueOnce(pending.promise);
    await renderHome(api);
    const activeRow = screen.getByRole("button", { name: `Open project ${project.title}` });
    const archivedRow = screen.getByRole("button", {
      name: "Open archived project Archived roster",
    });
    await userEvent.click(screen.getByRole("button", { name: "Archive project" }));
    await expect.element(activeRow).toBeDisabled();
    await expect.element(archivedRow).toBeDisabled();
    pending.resolve(response({}));
    await expect.element(archivedRow).toBeEnabled();
    await userEvent.click(archivedRow);
    await expect.element(screen.getByRole("heading", { name: "Archived roster" })).toBeVisible();
  });
});
