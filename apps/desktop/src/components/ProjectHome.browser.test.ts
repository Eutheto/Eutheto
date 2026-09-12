/* eslint-disable vue/one-component-per-file -- Browser-only routing fixtures need distinct components in one isolated harness. Production SFCs retain the rule. */
import { cleanup, render, screen, within } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import axe from "axe-core";
import { createPinia } from "pinia";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { defineComponent, h, ref } from "vue";
import { createMemoryHistory, createRouter, RouterLink, RouterView, useRoute } from "vue-router";

import {
  createProjectHomeController,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";
import type {
  ApiResponseDto,
  EmptyDto,
  PortableArtifactDto,
  PortableFilePreviewDto,
} from "../api/generated";
import * as generatedApi from "../api/generated";
import App from "../App.vue";
import { createAppRouter } from "../router";
import {
  fakeApi,
  portableApplied,
  portableOperation,
  portablePreview,
  project,
  response,
} from "../testing/project-home";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import { messages } from "../messages";
import "../styles.css";

vi.mock("../api/generated", { spy: true });

const workforceProject: ProjectSummary = { ...project, domainPackId: "official.workforce" };
const otherProject: ProjectSummary = {
  ...workforceProject,
  scenarioId: "01900000-0000-7000-8000-000000000002",
  title: "Weekend roster",
};
const exportPreview: PortableFilePreviewDto = {
  schemaVersion: 1,
  title: workforceProject.title,
  previewId: "01900000-0000-7000-8000-000000000070",
  byteLength: 4096,
  digest: "a".repeat(64),
  currentRevision: workforceProject.revision,
  libraryRevision: 1,
  backupSummary: null,
};

function configureNative(library: ProjectSummary[] = [{ ...workforceProject }]): ProjectSummary[] {
  // Supply window identity for actual SDK scope constructors; mock only native API boundaries.
  const api = fakeApi(library);
  vi.mocked(generatedApi.listProjects).mockImplementation(() => api.listProjects("all"));
  vi.mocked(generatedApi.openProject).mockImplementation(api.openProject);
  vi.mocked(generatedApi.createProject).mockImplementation(api.createProject);
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
  vi.mocked(generatedApi.deleteProject).mockImplementation((id, expectedRevision) => {
    const index = library.findIndex(
      (item) => item.scenarioId === id && item.revision === expectedRevision,
    );
    if (index < 0)
      throw Object.assign(new Error("The deletion revision changed."), {
        category: "conflict",
        code: "revision.conflict",
      });
    library.splice(index, 1);
    return Promise.resolve(response({}));
  });
  vi.mocked(generatedApi.setProjectArchived).mockImplementation(
    ({ scenarioId, expectedRevision, archived }) => {
      const index = library.findIndex(
        (item) => item.scenarioId === scenarioId && item.revision === expectedRevision,
      );
      const saved = library[index];
      if (!saved)
        throw Object.assign(new Error("The archive revision changed."), {
          category: "conflict",
          code: "revision.conflict",
        });
      library[index] = { ...saved, archived, revision: saved.revision + 1 };
      return Promise.resolve(response({}));
    },
  );
  // Real flow classes remain the owners; boundary spies do not manufacture renderer custody.
  vi.spyOn(generatedApi.PortableReviewFlow.prototype, "dispose").mockResolvedValue(undefined);
  vi.spyOn(generatedApi.SettingsImportFlow.prototype, "dispose").mockResolvedValue(undefined);
  return library;
}

async function renderWorkspace(path = `/projects?project=${workforceProject.scenarioId}`) {
  const router = createAppRouter();
  onTestFinished(() => {
    router.options.history.destroy();
  });
  await router.push(path);
  render(App, { global: { plugins: [createPinia(), PiniaColada, router] } });
  await router.isReady();
  return router;
}

async function openDeletion() {
  const trigger = await screen.findByRole("button", { name: messages.projects.delete });
  await userEvent.click(trigger);
  const dialog = await screen.findByRole("dialog", {
    name: messages.deletion.title(workforceProject.title),
  });
  const confirm = within(dialog).getByRole("button", { name: messages.deletion.confirm });
  await expect.element(confirm).toBeEnabled();
  return { trigger, dialog, confirm };
}

async function openReplacement() {
  await userEvent.click(
    await screen.findByRole("radio", { name: new RegExp(messages.portable.replace, "u") }),
  );
  await userEvent.click(screen.getByRole("button", { name: messages.portable.chooseRestore }));
  const trigger = await screen.findByRole("button", { name: messages.portable.reviewApply });
  await expect.element(trigger).toBeEnabled();
  await userEvent.click(trigger);
  const dialog = await screen.findByRole("dialog", { name: messages.portable.replaceTitle });
  return { trigger, dialog };
}

function mockExportPreview(): void {
  vi.spyOn(generatedApi.PortableReviewFlow.prototype, "previewExport").mockReturnValue(
    portableOperation(response(exportPreview, [], workforceProject.revision)),
  );
}

async function reachExportReview() {
  const { dialog } = await openDeletion();
  await userEvent.click(within(dialog).getByRole("button", { name: messages.library.exportFirst }));
  await userEvent.click(
    await screen.findByRole("button", { name: messages.portable.previewExport }),
  );
  return screen.findByRole("button", { name: messages.portable.saveFile });
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.resetAllMocks();
  vi.unstubAllGlobals();
});

describe("Routed project deletion in the browser", () => {
  it("contains keyboard focus, restores it on Keep/Escape, and deletes only after explicit confirmation", async () => {
    const library = configureNative([{ ...workforceProject }, { ...otherProject }]);
    await renderWorkspace();
    const trigger = await screen.findByRole("button", { name: messages.projects.delete });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    const dialog = await screen.findByRole("dialog", {
      name: messages.deletion.title(workforceProject.title),
    });
    const keep = within(dialog).getByRole("button", { name: messages.deletion.keep });
    const confirm = within(dialog).getByRole("button", { name: messages.deletion.confirm });
    await expect.element(keep).toHaveFocus();
    await expect.element(confirm).toBeEnabled();
    await userEvent.tab({ shift: true });
    await expect.element(confirm).toHaveFocus();
    await userEvent.tab();
    await expect.element(keep).toHaveFocus();
    await Promise.all(document.getAnimations().map((animation) => animation.finished));
    expect((await axe.run(dialog)).violations).toEqual([]);

    await userEvent.keyboard("{Escape}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect.element(trigger).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    const reopened = await screen.findByRole("dialog");
    await expect
      .element(within(reopened).getByRole("button", { name: messages.deletion.keep }))
      .toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect.element(trigger).toHaveFocus();
    expect(generatedApi.deleteProject).not.toHaveBeenCalled();
    expect(library.map((item) => item.scenarioId)).toEqual([
      workforceProject.scenarioId,
      otherProject.scenarioId,
    ]);

    await userEvent.keyboard("{Enter}");
    const finalDialog = await screen.findByRole("dialog");
    const finalConfirm = within(finalDialog).getByRole("button", {
      name: messages.deletion.confirm,
    });
    await expect.element(finalConfirm).toBeEnabled();
    finalConfirm.focus();
    await userEvent.keyboard("{Enter}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect
      .poll(() =>
        screen.queryAllByRole("link", { name: messages.projects.open(workforceProject.title) }),
      )
      .toHaveLength(0);
    await expect
      .element(screen.getByRole("link", { name: messages.projects.open(otherProject.title) }))
      .toBeVisible();
    await expect
      .element(screen.getByRole("heading", { name: messages.projects.heading }))
      .toHaveFocus();
    expect(library.map((item) => item.scenarioId)).toEqual([otherProject.scenarioId]);
  });

  it("returns an empty library to the real creation route after deleting its final project", async () => {
    const library = configureNative();
    const router = await renderWorkspace();
    const { confirm } = await openDeletion();
    await userEvent.click(confirm);
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect
      .element(await screen.findByRole("heading", { name: messages.projects.emptyHeading }))
      .toBeVisible();
    await expect
      .element(screen.getByRole("heading", { name: messages.projects.heading }))
      .toHaveFocus();
    expect(library).toEqual([]);
    await userEvent.click(screen.getByRole("link", { name: messages.welcome.startWork }));
    await expect.poll(() => router.currentRoute.value.name).toBe("project-create");
    await expect
      .element(await screen.findByRole("heading", { name: messages.welcome.createHeading }))
      .toBeVisible();
  });

  it("does not dismiss an unsettled deletion and keeps its safe native failure reviewable", async () => {
    const library = configureNative();
    const terminal = Promise.withResolvers<ApiResponseDto<EmptyDto>>();
    vi.mocked(generatedApi.deleteProject).mockReturnValueOnce(terminal.promise);
    await renderWorkspace();
    const { trigger, dialog, confirm } = await openDeletion();
    await userEvent.click(confirm);
    await expect.element(confirm).toBeDisabled();
    await userEvent.keyboard("{Escape}");
    await expect.element(dialog).toBeVisible();
    await expect
      .element(within(dialog).getByRole("button", { name: messages.deletion.keep }))
      .toBeDisabled();
    terminal.reject(
      Object.assign(new Error("The project could not be removed from local storage."), {
        category: "storage",
        code: "local_library.unavailable",
      }),
    );
    await expect
      .element(await screen.findByRole("alert"))
      .toHaveTextContent("The project could not be removed from local storage.");
    await expect.element(confirm).toBeEnabled();
    await userEvent.keyboard("{Escape}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect.element(trigger).toHaveFocus();
    expect(library).toHaveLength(1);
  });

  it("reports committed deletion even when the subsequent publication refresh fails", async () => {
    const library = configureNative();
    await renderWorkspace();
    const { confirm } = await openDeletion();
    vi.mocked(generatedApi.deleteProject).mockImplementationOnce(() => {
      library.splice(0);
      vi.mocked(generatedApi.listProjects).mockRejectedValueOnce(
        new Error("Publication refresh unavailable"),
      );
      return Promise.resolve(response({}));
    });
    await userEvent.click(confirm);
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    const outcome = await screen.findByText(`Deleted ${workforceProject.title}.`);
    await expect.element(outcome).toBeVisible();
    expect(outcome.getBoundingClientRect().height).toBeGreaterThan(1);
    expect(screen.queryByText(messages.app.selectedProject(workforceProject.title))).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: messages.projects.retry }));
    await expect
      .element(await screen.findByRole("heading", { name: messages.projects.emptyHeading }))
      .toBeVisible();
    expect(generatedApi.deleteProject).toHaveBeenCalledTimes(1);
  });

  it("returns from a saved export to a fresh deletion review, never an automatic delete", async () => {
    const library = configureNative();
    mockExportPreview();
    vi.spyOn(generatedApi.PortableReviewFlow.prototype, "createExport").mockReturnValue(
      portableOperation(
        response({
          schemaVersion: 1,
          currentRevision: workforceProject.revision,
          libraryRevision: 1,
          artifactName: "reviewed-roster.eutheto",
        }),
      ),
    );
    const router = await renderWorkspace();
    await userEvent.click(await reachExportReview());
    await screen.findByRole("button", { name: messages.portable.previewExport });
    expect(generatedApi.deleteProject).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("link", { name: messages.library.returnToDelete }));
    await expect.poll(() => router.currentRoute.value.name).toBe("projects");
    const dialog = await screen.findByRole("dialog", {
      name: messages.deletion.title(workforceProject.title),
    });
    const confirm = within(dialog).getByRole("button", { name: messages.deletion.confirm });
    await expect.element(confirm).toBeEnabled();
    await expect
      .element(within(dialog).getByRole("status"))
      .toHaveTextContent("reviewed-roster.eutheto");
    expect(generatedApi.deleteProject).not.toHaveBeenCalled();
    await userEvent.click(confirm);
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    expect(library).toEqual([]);
  });

  it("keeps a project after export cancellation and requires re-review when its revision changed away", async () => {
    const library = configureNative();
    mockExportPreview();
    vi.spyOn(generatedApi.PortableReviewFlow.prototype, "createExport").mockImplementation(() =>
      portableOperation(
        Promise.reject(
          Object.assign(new Error("The export was cancelled."), {
            category: "protocol",
            code: "operation.cancelled",
          }),
        ),
      ),
    );
    await renderWorkspace();
    await userEvent.click(await reachExportReview());
    await screen.findByRole("button", { name: messages.portable.previewExport });
    const changed = {
      ...workforceProject,
      title: "Revised roster",
      revision: workforceProject.revision + 1,
    };
    library[0] = changed;
    await userEvent.click(screen.getByRole("link", { name: messages.library.returnToDelete }));
    const dialog = await screen.findByRole("dialog", {
      name: messages.deletion.title(workforceProject.title),
    });
    const confirm = within(dialog).getByRole("button", { name: messages.deletion.confirm });
    await expect.element(confirm).toBeDisabled();
    const rereview = await within(dialog).findByRole("button", { name: messages.library.rereview });
    expect(generatedApi.deleteProject).not.toHaveBeenCalled();
    expect(library[0]).toEqual(changed);
    await userEvent.click(rereview);
    await expect.element(dialog).toHaveAccessibleName(messages.deletion.title(changed.title));
    await expect.element(confirm).toBeEnabled();
    expect(generatedApi.deleteProject).not.toHaveBeenCalled();
    await userEvent.click(confirm);
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    expect(generatedApi.deleteProject).toHaveBeenCalledWith(changed.scenarioId, changed.revision);
    expect(library).toEqual([]);
  });
});

describe("Routed native portable settlement and restore review", () => {
  it("requires a separate stronger confirmation after a retained safety-backup failure", async () => {
    configureNative();
    vi.spyOn(generatedApi.PortableReviewFlow.prototype, "previewRestore").mockReturnValue(
      portableOperation(response(portablePreview("full-backup"))),
    );
    const apply = vi
      .spyOn(generatedApi.PortableReviewFlow.prototype, "applyRestore")
      .mockImplementationOnce(() =>
        portableOperation(
          Promise.reject(
            Object.assign(new Error("The private backup destination is unavailable."), {
              category: "protocol",
              code: "restore.safety_backup_failed",
              retryable: false,
              details: { portablePreviewRetained: { type: "boolean", value: true } },
            }),
          ),
        ),
      )
      .mockReturnValueOnce(portableOperation(portableApplied({ kind: "confirmedBypass" })));
    await renderWorkspace("/settings/backup-restore");
    const { trigger, dialog } = await openReplacement();
    await expect
      .element(within(dialog).getByRole("button", { name: messages.portable.back }))
      .toHaveFocus();
    await userEvent.keyboard("{Escape}");
    await expect.element(trigger).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    const first = await screen.findByRole("dialog", { name: messages.portable.replaceTitle });
    const firstConfirm = within(first).getByRole("button", {
      name: messages.portable.confirmApply,
    });
    await expect.element(firstConfirm).toBeDisabled();
    await userEvent.click(
      within(first).getByRole("checkbox", { name: messages.portable.replacementAcknowledgement }),
    );
    await userEvent.click(firstConfirm);
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    const bypassTrigger = await screen.findByRole("button", {
      name: messages.portable.reviewBypass,
    });
    await expect.element(bypassTrigger).toHaveFocus();
    expect(apply).toHaveBeenCalledTimes(1);
    await userEvent.click(bypassTrigger);
    const stronger = await screen.findByRole("dialog", { name: messages.portable.bypassTitle });
    await Promise.all(document.getAnimations().map((animation) => animation.finished));
    expect((await axe.run(stronger)).violations).toEqual([]);
    const bypassConfirm = within(stronger).getByRole("button", {
      name: messages.portable.confirmBypass,
    });
    await expect.element(bypassConfirm).toBeDisabled();
    await userEvent.click(
      within(stronger).getByRole("checkbox", {
        name: messages.portable.replacementAcknowledgement,
      }),
    );
    const phrase = within(stronger).getByRole("textbox", { name: messages.portable.bypassLabel });
    await userEvent.fill(phrase, "REPLACE");
    await expect.element(bypassConfirm).toBeDisabled();
    await userEvent.fill(phrase, "REPLACE WITHOUT BACKUP");
    await userEvent.click(bypassConfirm);
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect
      .element(await screen.findByRole("button", { name: messages.portable.chooseRestore }))
      .toBeEnabled();
    await expect
      .element(screen.getByRole("heading", { name: messages.portable.backupRestoreTitle }))
      .toHaveFocus();
    expect(apply).toHaveBeenCalledTimes(2);
    expect(apply.mock.calls[1]?.[1].authorization.safetyBackupBypassPhrase).toBe(
      "REPLACE WITHOUT BACKUP",
    );
  });

  it("presents the actual verified safety artifact from a successful replacement", async () => {
    const library = configureNative();
    vi.spyOn(generatedApi.PortableReviewFlow.prototype, "previewRestore").mockReturnValue(
      portableOperation(response(portablePreview("full-backup"))),
    );
    vi.spyOn(generatedApi.PortableReviewFlow.prototype, "applyRestore").mockReturnValue(
      portableOperation(
        portableApplied({
          kind: "createdAndVerified",
          artifactName: "actual-native-safety.eutheto",
        }),
      ),
    );
    await renderWorkspace("/settings/backup-restore");
    const { dialog } = await openReplacement();
    await userEvent.click(
      within(dialog).getByRole("checkbox", { name: messages.portable.replacementAcknowledgement }),
    );
    const refresh = Promise.withResolvers<ApiResponseDto<readonly ProjectSummary[]>>();
    vi.mocked(generatedApi.listProjects).mockReturnValueOnce(refresh.promise);
    await userEvent.click(
      within(dialog).getByRole("button", { name: messages.portable.confirmApply }),
    );
    await expect
      .poll(() =>
        screen
          .getAllByRole("status")
          .some((element) => element.textContent.includes(messages.operations.refreshing)),
      )
      .toBe(true);
    expect(screen.queryByRole("button", { name: messages.operations.cancel })).toBeNull();
    refresh.resolve(response(library));
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect
      .poll(() =>
        screen
          .getAllByRole("status")
          .some((element) => element.textContent.includes("actual-native-safety.eutheto")),
      )
      .toBe(true);
  });

  it("does not apply replacement when Enter follows Escape during the real modal exit", async () => {
    const library = configureNative();
    vi.spyOn(generatedApi.PortableReviewFlow.prototype, "previewRestore").mockReturnValue(
      portableOperation(response(portablePreview("full-backup"))),
    );
    const apply = vi.spyOn(generatedApi.PortableReviewFlow.prototype, "applyRestore");
    await renderWorkspace("/settings/backup-restore");
    const { dialog } = await openReplacement();
    await userEvent.click(
      within(dialog).getByRole("checkbox", { name: messages.portable.replacementAcknowledgement }),
    );
    within(dialog).getByRole("button", { name: messages.portable.confirmApply }).focus();
    dialog.style.animationDuration = "2s";
    await userEvent.keyboard("{Escape}");
    expect(dialog.isConnected).toBe(true);
    await userEvent.keyboard("{Enter}");
    expect(apply).not.toHaveBeenCalled();
    expect(library).toEqual([workforceProject]);
  });

  it("requires a new preview after native reports a nonretained safety failure", async () => {
    configureNative();
    vi.spyOn(generatedApi.PortableReviewFlow.prototype, "previewRestore").mockReturnValue(
      portableOperation(response(portablePreview("full-backup"))),
    );
    vi.spyOn(generatedApi.PortableReviewFlow.prototype, "applyRestore").mockImplementationOnce(() =>
      portableOperation(
        Promise.reject(
          Object.assign(new Error("The reviewed backup is no longer available."), {
            category: "storage",
            code: "restore.safety_backup_failed",
            details: { portablePreviewRetained: { type: "boolean", value: false } },
          }),
        ),
      ),
    );
    await renderWorkspace("/settings/backup-restore");
    const { dialog } = await openReplacement();
    await userEvent.click(
      within(dialog).getByRole("checkbox", { name: messages.portable.replacementAcknowledgement }),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: messages.portable.confirmApply }),
    );
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect
      .element(await screen.findByRole("button", { name: messages.portable.chooseRestore }))
      .toBeEnabled();
    expect(screen.queryByRole("button", { name: messages.portable.reviewBypass })).toBeNull();
    expect(screen.queryByRole("textbox", { name: messages.portable.bypassLabel })).toBeNull();
    await expect
      .element(screen.getByRole("heading", { name: messages.portable.backupRestoreTitle }))
      .toHaveFocus();
  });

  it("keeps cancellation nonterminal and announces a late committed export without reviving its departed route", async () => {
    configureNative();
    mockExportPreview();
    const terminal = Promise.withResolvers<ApiResponseDto<PortableArtifactDto>>();
    const native = portableOperation(terminal.promise);
    const cancel = vi.spyOn(native, "cancel");
    vi.spyOn(generatedApi.PortableReviewFlow.prototype, "createExport").mockReturnValue(native);
    const router = await renderWorkspace(`/project/${workforceProject.scenarioId}/export`);
    await userEvent.click(
      await screen.findByRole("button", { name: messages.portable.previewExport }),
    );
    await userEvent.click(await screen.findByRole("button", { name: messages.portable.saveFile }));
    await userEvent.click(
      within(screen.getByRole("navigation", { name: messages.shell.navigation })).getByRole(
        "link",
        { name: messages.shell.projects },
      ),
    );
    const guard = await screen.findByRole("dialog", { name: messages.navigation.leaveTitle });
    await userEvent.click(
      within(guard).getByRole("button", { name: messages.navigation.cancelAndLeave }),
    );
    await expect.poll(() => router.currentRoute.value.name).toBe("projects");
    expect(cancel).toHaveBeenCalledTimes(1);
    await expect
      .element(await screen.findByRole("button", { name: messages.operations.cancel }))
      .toBeDisabled();
    terminal.resolve(
      response({
        schemaVersion: 1,
        currentRevision: workforceProject.revision,
        libraryRevision: 1,
        artifactName: "late-committed-roster.eutheto",
      }),
    );
    await expect
      .element(await screen.findByText(messages.portable.saved("late-committed-roster.eutheto")))
      .toBeVisible();
    expect(router.currentRoute.value.name).toBe("projects");
    expect(screen.queryByRole("heading", { name: messages.portable.reviewTitle })).toBeNull();
    expect(screen.queryByRole("button", { name: messages.portable.saveFile })).toBeNull();
    expect(screen.queryByRole("button", { name: messages.operations.cancel })).toBeNull();
  });
});

describe("Real shell commands and route updates", () => {
  it("shows a first-launch library failure and lets the user retry it", async () => {
    configureNative();
    vi.mocked(generatedApi.listProjects).mockRejectedValueOnce(
      new Error("Library temporarily unavailable"),
    );
    await renderWorkspace("/");
    await expect
      .element(await screen.findByRole("alert"))
      .toHaveTextContent(messages.projects.loadFailed);
    await userEvent.click(screen.getByRole("button", { name: messages.projects.retry }));
    await expect.element(await screen.findByText(messages.projects.count(1))).toBeVisible();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("targets active search, traps palette focus, restores Escape focus, and routes its keyboard selection", async () => {
    configureNative();
    const router = await renderWorkspace();
    const search = await screen.findByRole("searchbox", { name: messages.library.search });
    await userEvent.fill(search, "roster");
    screen.getByRole("heading", { name: messages.projects.heading }).focus();
    await userEvent.keyboard("{Control>}f{/Control}");
    await expect.element(search).toHaveFocus();
    expect((search as HTMLInputElement).selectionStart).toBe(0);
    expect((search as HTMLInputElement).selectionEnd).toBe("roster".length);
    await userEvent.keyboard("{Control>}k{/Control}");
    const palette = await screen.findByRole("dialog", { name: messages.shell.commandTitle });
    const commandSearch = within(palette).getByRole("searchbox", {
      name: messages.shell.commandSearch,
    });
    await expect.element(commandSearch).toHaveFocus();
    await Promise.all(document.getAnimations().map((animation) => animation.finished));
    expect((await axe.run(palette)).violations).toEqual([]);
    await userEvent.tab({ shift: true });
    await expect
      .element(within(palette).getByRole("button", { name: messages.shell.close }))
      .toHaveFocus();
    await userEvent.keyboard("{Escape}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect.element(search).toHaveFocus();
    await userEvent.keyboard("{Control>}k{/Control}");
    const reopened = await screen.findByRole("dialog", { name: messages.shell.commandTitle });
    await userEvent.fill(
      within(reopened).getByRole("searchbox", { name: messages.shell.commandSearch }),
      "settings",
    );
    await userEvent.keyboard("{ArrowDown}");
    await expect
      .element(within(reopened).getByRole("button", { name: messages.shell.settings }))
      .toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await expect.poll(() => router.currentRoute.value.name).toBe("settings");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect
      .element(await screen.findByRole("heading", { name: messages.settings.title }))
      .toHaveFocus();
  });

  it("reports stale setup instead of combining facts from different revisions", async () => {
    configureNative();
    vi.mocked(generatedApi.getScenarioSummary).mockReturnValue(
      portableOperation(
        response({
          schemaVersion: 2,
          scenarioId: workforceProject.scenarioId,
          revision: workforceProject.revision,
          title: workforceProject.title,
          structure: { entities: 0, rules: 0, preferences: 0, lockedAssignments: 0 },
          fast: { counts: { errors: 0, warnings: 0, information: 0 }, issues: [], omitted: 0 },
          full: { state: "notRun" },
        }),
      ),
    );
    vi.mocked(generatedApi.getScenarioView).mockReturnValue(
      portableOperation(
        response({
          schemaVersion: 2,
          scenarioId: workforceProject.scenarioId,
          revision: workforceProject.revision,
          view: {
            viewId: "official.workforce.setup.overview",
            data: {
              schemaVersion: 1,
              result: {
                kind: "overview",
                data: {
                  activePreferences: 0,
                  activeRequiredRules: 0,
                  configuredTypeMemberships: 0,
                  entities: [],
                  lockedAssignments: 0,
                  preferences: 0,
                  requiredRules: 0,
                  settings: {
                    timeZone: "UTC",
                    locale: "en-US",
                    units: "metric",
                    horizon: { start: "2030-01-01T00:00:00Z", end: "2030-02-01T00:00:00Z" },
                    gapPolicy: "reject",
                    overlapPolicy: "earlier",
                  },
                },
              },
            },
          },
        }),
      ),
    );
    vi.mocked(generatedApi.listSolutions).mockResolvedValue(
      response({
        schemaVersion: 1,
        scenarioId: workforceProject.scenarioId,
        currentRevision: workforceProject.revision + 1,
        solutions: [],
      }),
    );
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    await expect.element(await screen.findByRole("alert")).toHaveTextContent(messages.setup.stale);
    expect(screen.queryByRole("heading", { name: messages.setup.calendar })).toBeNull();
  });

  it("never sends scenario undo from a text editor, but enables contextual undo/redo and export outside it", async () => {
    configureNative();
    vi.mocked(generatedApi.undoScenario).mockRejectedValue(
      Object.assign(new Error("No saved action to undo."), {
        category: "validation",
        code: "undo.empty",
      }),
    );
    vi.mocked(generatedApi.redoScenario).mockRejectedValue(
      Object.assign(new Error("No saved action to redo."), {
        category: "validation",
        code: "redo.empty",
      }),
    );
    const router = await renderWorkspace();
    const input = await screen.findByRole("textbox", { name: messages.library.duplicateTitle });
    await userEvent.click(input);
    await userEvent.keyboard("Draft title");
    await userEvent.keyboard("{Control>}z{/Control}");
    await userEvent.keyboard("{Control>}{Shift>}z{/Shift}{/Control}");
    expect(generatedApi.undoScenario).not.toHaveBeenCalled();
    expect(generatedApi.redoScenario).not.toHaveBeenCalled();
    screen.getByRole("heading", { name: messages.projects.heading }).focus();
    await userEvent.keyboard("{Control>}z{/Control}");
    await expect
      .element(await screen.findByRole("alert"))
      .toHaveTextContent("No saved action to undo.");
    await userEvent.keyboard("{Control>}{Shift>}z{/Shift}{/Control}");
    await expect
      .element(await screen.findByRole("alert"))
      .toHaveTextContent("No saved action to redo.");
    expect(generatedApi.undoScenario).toHaveBeenCalledWith(
      workforceProject.scenarioId,
      workforceProject.revision,
    );
    expect(generatedApi.redoScenario).toHaveBeenCalledWith(
      workforceProject.scenarioId,
      workforceProject.revision,
    );
    await userEvent.fill(input, "");
    screen.getByRole("heading", { name: messages.projects.heading }).focus();
    await userEvent.keyboard("{Control>}s{/Control}");
    await expect.poll(() => router.currentRoute.value.name).toBe("project-export");
    await expect
      .element(await screen.findByRole("button", { name: messages.portable.previewExport }))
      .toBeEnabled();
    await router.push({
      name: "project-setup",
      params: { scenarioId: workforceProject.scenarioId },
    });
    await userEvent.click(await screen.findByRole("link", { name: messages.shell.export }));
    await expect
      .element(await screen.findByRole("heading", { name: messages.portable.exportTitle }))
      .toHaveFocus();
  });

  it("preserves a duplicate draft through Stay on a real library query update", async () => {
    configureNative([{ ...workforceProject }, { ...otherProject }]);
    const router = await renderWorkspace();
    const input = await screen.findByRole("textbox", { name: messages.library.duplicateTitle });
    await userEvent.fill(input, "Unsaved duplicate title");
    await userEvent.keyboard("{Control>}s{/Control}");
    const shortcutGuard = await screen.findByRole("dialog", {
      name: messages.navigation.leaveTitle,
    });
    await userEvent.click(
      within(shortcutGuard).getByRole("button", { name: messages.navigation.stay }),
    );
    await expect.element(input).toHaveFocus();
    expect(router.currentRoute.value.name).toBe("projects");
    const other = screen.getByRole("link", { name: messages.library.details(otherProject.title) });
    await userEvent.click(other);
    const guard = await screen.findByRole("dialog", { name: messages.navigation.leaveTitle });
    await userEvent.click(within(guard).getByRole("button", { name: messages.navigation.stay }));
    await expect.element(other).toHaveFocus();
    await expect.element(input).toHaveValue("Unsaved duplicate title");
    expect(router.currentRoute.value.query.project).toBe(workforceProject.scenarioId);
    await userEvent.click(other);
    const leaving = await screen.findByRole("dialog", { name: messages.navigation.leaveTitle });
    await userEvent.click(
      within(leaving).getByRole("button", { name: messages.navigation.discardAndLeave }),
    );
    await expect.poll(() => router.currentRoute.value.query.project).toBe(otherProject.scenarioId);
    await expect.element(input).toHaveValue("");
    expect(generatedApi.duplicateProject).not.toHaveBeenCalled();
  });

  it("guards an unsettled noncancellable archive rather than allowing a route update to abandon it", async () => {
    const archived = { ...otherProject, archived: true };
    const library = configureNative([{ ...workforceProject }, archived]);
    const terminal = Promise.withResolvers<ApiResponseDto<EmptyDto>>();
    vi.mocked(generatedApi.setProjectArchived).mockReturnValueOnce(terminal.promise);
    const router = await renderWorkspace();
    await userEvent.click(await screen.findByRole("button", { name: messages.projects.archive }));
    const other = screen.getByRole("link", { name: messages.library.details(archived.title) });
    await userEvent.click(other);
    const guard = await screen.findByRole("dialog", { name: messages.navigation.leaveTitle });
    await expect
      .element(within(guard).getByRole("button", { name: messages.navigation.cancelAndLeave }))
      .toBeDisabled();
    await userEvent.keyboard("{Escape}");
    await expect.element(other).toHaveFocus();
    expect(router.currentRoute.value.query.project).toBe(workforceProject.scenarioId);
    library[0] = { ...workforceProject, archived: true, revision: workforceProject.revision + 1 };
    terminal.resolve(response({}));
    await expect
      .element(await screen.findByRole("button", { name: messages.projects.unarchive }))
      .toBeEnabled();
    await userEvent.click(other);
    await expect.poll(() => router.currentRoute.value.query.project).toBe(archived.scenarioId);
    await expect.element(screen.getByRole("heading", { name: archived.title })).toBeVisible();
  });
});

describe("Route draft guard in the browser", () => {
  it("preserves drafts and focus on Stay, and discards only after confirmed route update or leave", async () => {
    const draft = ref("");
    let home: ProjectHomeController | undefined;
    const editor = defineComponent({
      setup() {
        const route = useRoute();
        const controller = home;
        if (!controller) throw new Error("Expected the root controller before its route");
        return () =>
          h("section", [
            h("h2", `Editor ${String(route.params.id)}`),
            h("label", { for: "guard-draft" }, "Draft title"),
            h("input", {
              id: "guard-draft",
              value: draft.value,
              onInput(event: Event) {
                if (event.target instanceof HTMLInputElement) draft.value = event.target.value;
              },
            }),
            h(RouterLink, { to: "/edit/two" }, () => "Next editor"),
            h(RouterLink, { to: "/other" }, () => "Other view"),
            h(RouteLeaveGuard, {
              home: controller,
              dirty: draft.value !== "",
              pending: false,
              discard: () => {
                draft.value = "";
              },
            }),
          ]);
      },
    });
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: "/edit/:id", component: editor },
        { path: "/other", component: defineComponent({ render: () => h("h2", "Other view") }) },
      ],
    });
    onTestFinished(() => {
      router.options.history.destroy();
    });
    await router.push("/edit/one");
    render(
      defineComponent({
        setup() {
          home = createProjectHomeController(fakeApi());
          return () => h("main", [h("h1", "Workspace"), h(RouterView)]);
        },
      }),
      { global: { plugins: [createPinia(), PiniaColada, router] } },
    );
    const input = await screen.findByRole("textbox", { name: "Draft title" });
    await userEvent.fill(input, "Unsaved title");
    const next = screen.getByRole("link", { name: "Next editor" });
    await userEvent.click(next);
    const dialog = await screen.findByRole("dialog", { name: messages.navigation.leaveTitle });
    await expect
      .element(within(dialog).getByRole("button", { name: messages.navigation.stay }))
      .toHaveFocus();
    await Promise.all(document.getAnimations().map((animation) => animation.finished));
    expect((await axe.run(dialog)).violations).toEqual([]);
    await userEvent.keyboard("{Escape}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect.element(next).toHaveFocus();
    await expect.element(input).toHaveValue("Unsaved title");
    expect(router.currentRoute.value.params.id).toBe("one");
    await userEvent.keyboard("{Enter}");
    await screen.findByRole("dialog");
    await userEvent.tab();
    await userEvent.keyboard("{Enter}");
    await expect.element(await screen.findByRole("heading", { name: "Editor two" })).toBeVisible();
    await expect.element(input).toHaveValue("");
    await userEvent.fill(input, "Another draft");
    await userEvent.click(screen.getByRole("link", { name: "Other view" }));
    const leaving = await screen.findByRole("dialog", { name: messages.navigation.leaveTitle });
    await userEvent.click(
      within(leaving).getByRole("button", { name: messages.navigation.discardAndLeave }),
    );
    await expect.element(await screen.findByRole("heading", { name: "Other view" })).toBeVisible();
    expect(draft.value).toBe("");
  });
});
