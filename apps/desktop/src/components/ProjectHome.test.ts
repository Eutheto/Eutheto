import { createSSRApp, effectScope } from "vue";
import { createPinia, disposePinia } from "pinia";
import { PiniaColada } from "@pinia/colada";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import type { ApiResponseDto, ProjectMetadataDto, ScenarioChangedEvent } from "../api/generated";
import {
  createProjectHomeController,
  type CreateProjectInput,
  type ProjectHomeApi,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";
import { fakeApi, project, response } from "../testing/project-home";

afterEach(() => vi.unstubAllGlobals());

function createHome(api: ProjectHomeApi): ProjectHomeController {
  const app = createSSRApp({ render: () => null });
  const pinia = createPinia();
  app.use(pinia);
  app.use(PiniaColada);
  const scope = effectScope();
  const home = app.runWithContext(() => scope.run(() => createProjectHomeController(api)));
  if (!home) throw new Error("Expected an active controller scope");
  onTestFinished(async () => {
    await home.dispose();
    scope.stop();
    disposePinia(pinia);
  });
  return home;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

const creation: CreateProjectInput = {
  title: "Clinic roster",
  description: "Autumn plan",
  domainPack: { id: "official.workforce", schemaVersion: 1 },
  settings: {
    timeZone: "UTC",
    locale: "en-US",
    units: "metric",
    firstDate: "2026-09-01",
    lastDate: "2026-09-30",
    gapPolicy: "reject",
    overlapPolicy: "earlier",
  },
};

const changedEvent: ScenarioChangedEvent = {
  type: "scenarioChanged",
  payload: {
    context: {
      eventVersion: 1,
      timestamp: "2026-08-29T12:00:00Z",
      requestId: "01900000-0000-7000-8000-000000000099",
      scenarioId: project.scenarioId,
      revision: project.revision,
      solveRunId: null,
    },
    changeSet: { changes: [] },
  },
};

describe("root project controller", () => {
  it.each(["success", "error"] as const)(
    "ignores an older refresh's late %s after a newer saved list",
    async (settlement) => {
      const api = fakeApi([project]);
      const home = createHome(api);
      await home.load();
      const older = deferred<ApiResponseDto<readonly ProjectSummary[]>>();
      api.listProjects.mockReturnValueOnce(older.promise);
      const first = home.load();
      const latest = { ...project, revision: 4 };
      const selected = { ...project, scenarioId: "01900000-0000-7000-8000-000000000002" };
      api.listProjects.mockResolvedValueOnce(response([latest, selected]));
      await home.load();
      home.selectProject(selected.scenarioId);
      if (settlement === "success") older.resolve(response([]));
      else older.reject({ category: "storage", code: "read.failed", message: "Old failure" });
      await first;
      expect(home.state.projects).toEqual([latest, selected]);
      expect(home.state.selectedId).toBe(selected.scenarioId);
      expect(home.state.phase).toBe("ready");
      expect(home.state.errorMessage).toBeNull();
    },
  );

  it("does not let an older successful read erase a newer refresh error", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    home.selectProject(project.scenarioId);
    const older = deferred<ApiResponseDto<readonly ProjectSummary[]>>();
    api.listProjects.mockReturnValueOnce(older.promise);
    const first = home.load();
    api.listProjects.mockRejectedValueOnce({
      category: "storage",
      code: "read.failed",
      message: "Current failure",
    });
    await home.load();
    older.resolve(response([]));
    await first;
    expect(home.state.projects).toEqual([project]);
    expect(home.state.selectedId).toBe(project.scenarioId);
    expect(home.state.phase).toBe("error");
    expect(home.state.errorMessage).toContain("Current failure");
  });

  it("never substitutes the first project when the selected identity disappears", async () => {
    const other = { ...project, scenarioId: "01900000-0000-7000-8000-000000000002" };
    const api = fakeApi([project, other]);
    const home = createHome(api);
    await home.load();
    expect(home.state.selectedId).toBeNull();
    home.selectProject(other.scenarioId);
    api.listProjects.mockResolvedValueOnce(response([project]));
    await home.refreshLibrary();
    expect(home.state.projects).toEqual([project]);
    expect(home.state.selectedId).toBeNull();
  });

  it("preserves a committed receipt and outcome when its authoritative refresh fails", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    api.listProjects.mockRejectedValueOnce({
      category: "storage",
      code: "read.failed",
      message: "Library unavailable",
    });
    const receipt = response({ artifactName: "committed-safety.eutheto" });
    const result = await home.runOperation({
      action: "restore",
      label: "Restore",
      execute: () => Promise.resolve(receipt),
      success: ({ artifactName }) => `Committed with ${artifactName}`,
    });
    expect(result).toBe(receipt);
    expect(home.state.phase).toBe("error");
    expect(home.state.errorMessage).toContain("Library unavailable");
    expect(home.state.announcement).toContain(receipt.result.artifactName);
    const committedAnnouncement = home.state.announcement;
    await home.refreshLibrary();
    expect(home.state.phase).toBe("ready");
    expect(home.state.announcement).toBe(committedAnnouncement);
  });

  it("keeps mutation exclusion through cancellation acknowledgement and actual settlement", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    const terminal = deferred<ApiResponseDto<unknown>>();
    const cancellation = deferred<undefined>();
    const running = home.runOperation({
      action: "restore",
      label: "Restore",
      execute: () => terminal.promise,
      success: () => "Committed restore",
      cancel: () => cancellation.promise,
    });
    const cancelling = home.cancelOperation();
    cancellation.resolve(undefined);
    await cancelling;
    expect(home.state.operation?.cancellationRequested).toBe(true);
    expect(home.state.operation?.settled).toBe(false);
    expect(await home.deleteProject(project)).toBe(false);
    expect(await home.openProject(project.scenarioId)).toBeNull();
    expect(api.deleteProject).not.toHaveBeenCalled();
    expect(api.openProject).not.toHaveBeenCalled();
    const overlap = vi.fn(() => Promise.resolve(response({})));
    await expect(
      home.runOperation({
        action: "overlap",
        label: "Overlap",
        execute: overlap,
        success: () => "Wrong outcome",
      }),
    ).rejects.toBeInstanceOf(Error);
    expect(overlap).not.toHaveBeenCalled();
    terminal.resolve(response({}));
    await running;
    expect(home.state.announcement).toBe("Committed restore");
    expect(await home.deleteProject(project)).toBe(true);
  });

  it("keeps cancellation acknowledgement errors from rewriting a settled operation", async () => {
    const home = createHome(fakeApi([project]));
    const terminal = deferred<ApiResponseDto<unknown>>();
    const cancellation = deferred<unknown>();
    const running = home.runOperation({
      action: "save",
      label: "Save",
      execute: () => terminal.promise,
      success: () => "Committed save",
      cancel: () => cancellation.promise,
      refreshLibrary: false,
    });
    const cancelling = home.cancelOperation();
    terminal.resolve(response({}));
    await running;
    cancellation.reject({
      category: "protocol",
      code: "cancel.failed",
      message: "Late cancellation failure",
    });
    await cancelling;
    expect(home.state.announcement).toBe("Committed save");
    expect(home.state.errorMessage).toBeNull();
    expect(home.state.operation).toBeNull();
  });

  it("releases partial and late listener acquisitions even when another release throws", async () => {
    const api = fakeApi();
    const late = deferred<() => void>();
    const failed = deferred<() => void>();
    const releaseChanged = vi.fn(() => {
      throw new Error("Release failed");
    });
    const releaseValidation = vi.fn();
    const releaseLate = vi.fn();
    api.onScenarioChanged.mockResolvedValueOnce(releaseChanged);
    api.onScenarioValidationChanged.mockResolvedValueOnce(releaseValidation);
    api.onAppNotification.mockReturnValueOnce(failed.promise);
    api.onLibraryRefreshRequired.mockReturnValueOnce(late.promise);
    const home = createHome(api);
    const started = home.startEventListeners();
    await Promise.resolve();
    failed.reject(new Error("Registration failed"));
    await Promise.resolve();
    late.resolve(releaseLate);
    await started;
    expect(releaseChanged).toHaveBeenCalledOnce();
    expect(releaseValidation).toHaveBeenCalledOnce();
    expect(releaseLate).toHaveBeenCalledOnce();
    await home.dispose();
    expect(releaseChanged).toHaveBeenCalledOnce();
    expect(releaseValidation).toHaveBeenCalledOnce();
    expect(releaseLate).toHaveBeenCalledOnce();
  });

  it("ignores disposed reads and events, releases a late listener and awaits owned cleanup", async () => {
    const api = fakeApi([project]);
    const read = deferred<ApiResponseDto<readonly ProjectSummary[]>>();
    const registration = deferred<() => void>();
    const cleanup = deferred<undefined>();
    const released = vi.fn();
    let notify: (() => void) | undefined;
    api.onAppNotification.mockImplementation((listener) => {
      notify = listener;
      return registration.promise;
    });
    const home = createHome(api);
    await home.load();
    home.selectProject(project.scenarioId);
    const started = home.startEventListeners();
    api.listProjects.mockReturnValueOnce(read.promise);
    const loading = home.load();
    home.registerReviewOwner({ dispose: () => cleanup.promise });
    const disposal = home.dispose();
    expect(home.dispose()).toBe(disposal);
    let settled = false;
    void disposal.then(() => {
      settled = true;
    });
    const previous = {
      phase: home.state.phase,
      error: home.state.errorMessage,
      selection: home.state.selectedId,
      announcement: home.state.announcement,
    };
    registration.resolve(released);
    await started;
    notify?.();
    read.resolve(response([]));
    await loading;
    expect(settled).toBe(false);
    cleanup.resolve(undefined);
    await disposal;
    expect(home.state.projects).toEqual([project]);
    expect({
      phase: home.state.phase,
      error: home.state.errorMessage,
      selection: home.state.selectedId,
      announcement: home.state.announcement,
    }).toEqual(previous);
    expect(released).toHaveBeenCalledOnce();
    expect(api.listProjects).toHaveBeenCalledTimes(2);
  });

  it("returns late committed work without republishing into a disposed root", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    const terminal = deferred<ApiResponseDto<{ artifactName: string }>>();
    home.registerReviewOwner({ dispose: () => terminal.promise.then(() => undefined) });
    const running = home.runOperation({
      action: "restore",
      label: "Restore",
      execute: () => terminal.promise,
      success: ({ artifactName }) => `Committed ${artifactName}`,
    });
    const disposal = home.dispose();
    const announcement = home.state.announcement;
    let settled = false;
    void disposal.then(() => {
      settled = true;
    });
    await Promise.resolve();
    expect(settled).toBe(false);
    const committed = response({ artifactName: "late-safety.eutheto" });
    terminal.resolve(committed);
    expect(await running).toBe(committed);
    await disposal;
    expect(home.state.announcement).toBe(announcement);
    expect(home.state.projects).toEqual([project]);
    expect(api.listProjects).toHaveBeenCalledOnce();
  });

  it("retains failed review cleanup and retries failed owners without disposing active reviews", async () => {
    const home = createHome(fakeApi());
    const failedOwner = {
      dispose: vi
        .fn()
        .mockRejectedValueOnce(new Error("Cleanup failed"))
        .mockResolvedValue(undefined),
    };
    const activeOwner = { dispose: vi.fn().mockResolvedValue(undefined) };
    home.registerReviewOwner(failedOwner);
    home.registerReviewOwner(activeOwner);
    expect(await home.retireReviewOwner(failedOwner)).toBe(false);
    expect(home.state.reviewCleanupError).not.toBeNull();
    const retry = deferred<undefined>();
    failedOwner.dispose.mockReturnValueOnce(retry.promise);
    const retrying = home.retryReviewCleanup();
    expect(home.state.retryingReviewCleanup).toBe(true);
    expect(home.state.reviewCleanupError).not.toBeNull();
    expect(activeOwner.dispose).not.toHaveBeenCalled();
    retry.resolve(undefined);
    await retrying;
    expect(home.state.reviewCleanupError).toBeNull();
    expect(home.state.retryingReviewCleanup).toBe(false);
    await home.dispose();
    expect(activeOwner.dispose).toHaveBeenCalledOnce();
    expect(failedOwner.dispose).toHaveBeenCalledTimes(2);
  });

  it("exposes structured safe errors but not runtime details", async () => {
    const api = fakeApi();
    const home = createHome(api);
    api.listProjects.mockRejectedValueOnce({
      category: "storage",
      code: "local_library.unavailable",
      message: "Library unavailable",
    });
    await home.load();
    expect(home.state.phase).toBe("error");
    expect(home.state.errorMessage).toContain("Library unavailable");
    const runtimeMessage = "Cannot read properties of undefined (reading 'invoke')";
    api.listProjects.mockRejectedValueOnce(new Error(runtimeMessage));
    await home.load();
    expect(home.state.errorMessage).not.toBeNull();
    expect(home.state.errorMessage).not.toContain(runtimeMessage);
  });

  it("preserves even a null native rejection without replacing it with a classifier TypeError", async () => {
    const home = createHome(fakeApi());
    const terminal = deferred<ApiResponseDto<unknown>>();
    const result = home.runOperation({
      action: "malformed-error",
      label: "Request",
      execute: () => terminal.promise,
      success: () => "Must not commit",
      refreshLibrary: false,
    });
    terminal.reject(null);
    await expect(result).rejects.toBeNull();
    expect(home.state.errorMessage).not.toBeNull();
    expect(home.state.busyAction).toBeNull();
  });

  it("does not optimistically create or select a project and returns the actual committed metadata", async () => {
    const api = fakeApi();
    const terminal = deferred<ApiResponseDto<ProjectMetadataDto>>();
    api.createProject.mockReturnValueOnce(terminal.promise);
    const home = createHome(api);
    await home.load();
    const creating = home.createProject(creation);
    expect(home.state.projects).toEqual([]);
    expect(home.state.selectedId).toBeNull();
    const metadata: ProjectMetadataDto = {
      scenarioId: project.scenarioId,
      title: "Native title",
      description: "Autumn plan",
      domainPack: creation.domainPack,
      revision: 0,
      createdAt: project.updatedAt,
      updatedAt: project.updatedAt,
      archivedAt: null,
    };
    const authoritative = { ...project, title: "Newer authoritative title", revision: 1 };
    api.listProjects.mockResolvedValueOnce(response([authoritative]));
    terminal.resolve(response(metadata, [], 0));
    expect(await creating).toBe(metadata);
    expect(home.state.projects).toEqual([authoritative]);
    expect(home.state.selectedId).toBeNull();
  });

  it("returns a committed open receipt even if its subsequent list refresh fails", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    const opened = { ...project, lastOpenedAt: "2026-08-29T15:30:00Z" };
    api.openProject.mockResolvedValueOnce(response(opened));
    api.listProjects.mockRejectedValueOnce({
      category: "storage",
      code: "refresh.failed",
      message: "List refresh failed",
    });
    expect(await home.openProject(project.scenarioId)).toBe(opened);
    expect(home.state.phase).toBe("error");
    expect(home.state.errorMessage).toContain("List refresh failed");
    expect(home.state.selectedId).toBeNull();
  });

  it("publishes archive, duplicate and deletion state only from the authoritative refresh", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    const terminal = deferred<ApiResponseDto<unknown>>();
    api.setProjectArchived.mockReturnValueOnce(terminal.promise);
    const archiving = home.setArchived(project);
    expect(home.state.projects[0]?.archived).toBe(false);
    const archived = { ...project, archived: true, revision: 4 };
    api.listProjects.mockResolvedValueOnce(response([archived]));
    terminal.resolve(response({}));
    expect(await archiving).toBe(true);
    expect(home.state.projects).toEqual([archived]);
    const unarchived = { ...archived, archived: false, revision: 5 };
    api.listProjects.mockResolvedValueOnce(response([unarchived]));
    expect(await home.setArchived(archived)).toBe(true);
    expect(home.state.projects).toEqual([unarchived]);
    const copy = {
      ...project,
      scenarioId: "01900000-0000-7000-8000-000000000002",
      title: "Native copy title",
    };
    api.listProjects.mockResolvedValueOnce(response([unarchived, copy]));
    expect(await home.duplicateProject(unarchived, "Requested title")).toBe(true);
    expect(home.state.projects).toEqual([unarchived, copy]);
    home.selectProject(copy.scenarioId);
    api.listProjects.mockResolvedValueOnce(response([unarchived]));
    expect(await home.deleteProject(copy)).toBe(true);
    expect(home.state.projects).toEqual([unarchived]);
    expect(home.state.selectedId).toBeNull();
  });

  it("refreshes rejected revision conflicts without silently retrying against the new revision", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    api.duplicateProject.mockRejectedValueOnce({
      category: "conflict",
      code: "project.revision_conflict",
      message: "Revision conflict",
    });
    const authoritative = { ...project, title: "Authoritative title", revision: 4 };
    api.listProjects.mockResolvedValueOnce(response([authoritative]));
    expect(await home.duplicateProject(project, "Copy")).toBe(false);
    expect(home.state.projects).toEqual([authoritative]);
    expect(api.duplicateProject).toHaveBeenCalledOnce();
  });

  it("refreshes all native event sources without erasing the committed outcome and releases listeners", async () => {
    const api = fakeApi([project]);
    let changed: ((event: ScenarioChangedEvent) => void) | undefined;
    let notification: (() => void) | undefined;
    let refreshRequired: (() => void) | undefined;
    let validation: Parameters<ProjectHomeApi["onScenarioValidationChanged"]>[0] | undefined;
    const releases = [vi.fn(), vi.fn(), vi.fn(), vi.fn()] as const;
    api.onScenarioChanged.mockImplementation((listener) => {
      changed = listener;
      return Promise.resolve(releases[0]);
    });
    api.onScenarioValidationChanged.mockImplementation((listener) => {
      validation = listener;
      return Promise.resolve(releases[1]);
    });
    api.onAppNotification.mockImplementation((listener) => {
      notification = listener;
      return Promise.resolve(releases[2]);
    });
    api.onLibraryRefreshRequired.mockImplementation((listener) => {
      refreshRequired = listener;
      return Promise.resolve(releases[3]);
    });
    const home = createHome(api);
    await home.load();
    await home.startEventListeners();
    await home.startEventListeners();
    await home.runOperation({
      action: "commit",
      label: "Commit",
      execute: () => Promise.resolve(response({ artifactName: "saved.eutheto" })),
      success: ({ artifactName }) => artifactName,
      refreshLibrary: false,
    });
    for (const fire of [
      () => changed?.(changedEvent),
      () => notification?.(),
      () => refreshRequired?.(),
      () =>
        validation?.({
          type: "scenarioValidationChanged",
          payload: {
            context: changedEvent.payload.context,
            validationDelta: { added: [], resolved: [] },
          },
        }),
    ]) {
      const next = { ...project, revision: (home.state.projects[0]?.revision ?? 3) + 1 };
      api.listProjects.mockResolvedValueOnce(response([next]));
      fire();
      await vi.waitFor(() => {
        expect(home.state.projects).toEqual([next]);
      });
      expect(home.state.announcement).toBe("saved.eutheto");
    }
    await home.dispose();
    for (const release of releases) expect(release).toHaveBeenCalledOnce();
  });
});
