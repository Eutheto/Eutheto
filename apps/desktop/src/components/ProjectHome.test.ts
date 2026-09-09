import { createSSRApp, effectScope, h } from "vue";
import { createPinia, disposePinia } from "pinia";
import { PiniaColada } from "@pinia/colada";
import { renderToString } from "@vue/server-renderer";
import { describe, expect, it, onTestFinished, vi } from "vitest";

import type {
  AppliedMigrationDto,
  ApiResponseDto,
  PortableFilePreviewDto,
  ScenarioChangedEvent,
} from "../api/generated";
import ProjectHome from "./ProjectHome.vue";
import {
  defaultSupplementalCollisionChoices,
  createProjectHomeController,
  scenarioRevisionOutcome,
  type ProjectHomeApi,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";

import {
  fakeApi,
  portablePreview,
  previewWarning,
  project,
  response,
} from "../testing/project-home";

const fixedExclusionLabels = [
  "Local undo and audit history",
  "SQLite and database internals",
  "Credentials, tokens, and keychain references",
  "Device-local paths and window state",
  "Logs, caches, and temporary data",
  "Redistribution-prohibited provider data",
  "Executable content",
] as const;

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
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

async function render(home: ProjectHomeController): Promise<string> {
  return renderToString(
    createSSRApp({
      render: () => h(ProjectHome, { home }),
    }),
  );
}

function projectAt(home: ProjectHomeController, index: number): ProjectSummary {
  const item = home.state.projects[index];
  expect(item).toBeDefined();
  if (item === undefined) {
    throw new Error(`Expected project at index ${String(index)}`);
  }
  return item;
}

describe("ProjectHome", () => {
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

  it.each(["import", "restore", "backup"] as const)(
    "keeps a committed %s successful when the subsequent library refresh fails",
    async (operation) => {
      const api = fakeApi([project]);
      const home = createHome(api);
      await home.load();
      if (operation === "import") {
        await home.previewImport({ includeResults: true, includeAssets: true });
      } else if (operation === "restore") {
        await home.previewRestore("add-backup");
      } else {
        await home.previewBackup("Before changes");
      }
      api.listProjects.mockRejectedValueOnce({
        category: "storage",
        code: "read.failed",
        message: "Library unavailable",
      });
      const apply = () =>
        operation === "import"
          ? home.applyImport({ [project.scenarioId]: "create-copy" }, {})
          : operation === "restore"
            ? home.applyRestore({ [project.scenarioId]: "create-copy" }, {})
            : home.createBackup("Before changes");
      expect(await apply()).toBe(true);
      expect(home.state.phase).toBe("error");
      expect(home.state.errorMessage).toContain("Library unavailable");
      expect(home.state.errorMessage).toContain("Try again");
      expect(home.state.announcement).toMatch(/applied|saved/);
      expect(home.state.importPreview).toBeNull();
      expect(home.state.restorePreview).toBeNull();
      expect(home.state.backupPreview).toBeNull();
      expect(await apply()).toBe(false);
      await home.load();
      expect(home.state.phase).toBe("ready");
      const nativeApply =
        operation === "import"
          ? api.applyImport
          : operation === "restore"
            ? api.applyRestore
            : api.createBackup;
      expect(nativeApply).toHaveBeenCalledOnce();
    },
  );

  it("refuses overlapping user operations without losing an owned preview", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    await home.previewImport({ includeResults: true, includeAssets: true });
    const preview = home.state.importPreview;
    const pending = deferred<ApiResponseDto<unknown>>();
    api.duplicateProject.mockReturnValueOnce(pending.promise);
    const duplicate = home.duplicateProject(project, "Copy");
    expect(await home.applyImport({ [project.scenarioId]: "create-copy" }, {})).toBe(false);
    expect(await home.deleteProject(project)).toBe(false);
    expect(await home.previewRestore("replace-library")).toBe(false);
    expect(await home.previewBackup("Before changes")).toBe(false);
    expect(home.state.importPreview).toBe(preview);
    expect(home.state.busyAction).toBe(`duplicate:${project.scenarioId}`);
    expect(api.applyImport).not.toHaveBeenCalled();
    expect(api.deleteProject).not.toHaveBeenCalled();
    expect(api.previewRestore).not.toHaveBeenCalled();
    expect(api.previewBackup).not.toHaveBeenCalled();
    pending.resolve(response({}));
    expect(await duplicate).toBe(true);
    expect(home.state.busyAction).toBeNull();
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

  it("ignores disposed reads and events and releases a late listener and native preview", async () => {
    const api = fakeApi([project]);
    const read = deferred<ApiResponseDto<readonly ProjectSummary[]>>();
    const preview = deferred<ApiResponseDto<PortableFilePreviewDto>>();
    const registration = deferred<() => void>();
    const released = vi.fn();
    let notify: (() => void) | undefined;
    api.onAppNotification.mockImplementation((listener) => {
      notify = listener;
      return registration.promise;
    });
    const home = createHome(api);
    await home.load();
    const started = home.startEventListeners();
    api.listProjects.mockReturnValueOnce(read.promise);
    api.previewBackup.mockReturnValueOnce(preview.promise);
    const loading = home.load();
    const previewing = home.previewBackup("Before changes");
    await home.dispose();
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
    preview.resolve(
      response({
        title: "Before changes",
        byteLength: 1,
        previewId: "late-backup",
        digest: "b".repeat(64),
        currentRevision: null,
        libraryRevision: 1,
        backupSummary: null,
      }),
    );
    await loading;
    expect(await previewing).toBe(false);
    expect(home.state.projects).toEqual([project]);
    expect({
      phase: home.state.phase,
      error: home.state.errorMessage,
      selection: home.state.selectedId,
      announcement: home.state.announcement,
    }).toEqual(previous);
    expect(home.state.backupPreview).toBeNull();
    expect(released).toHaveBeenCalledOnce();
    expect(api.cancelPortablePreview).toHaveBeenCalledWith("late-backup");
    expect(api.listProjects).toHaveBeenCalledTimes(2);
  });

  it("discards superseded and disposed retained backup previews", async () => {
    const api = fakeApi();
    const home = createHome(api);
    await home.previewBackup("First");
    const first = home.state.backupPreview;
    if (!first) throw new Error("Expected a retained backup preview");
    api.previewBackup.mockResolvedValueOnce(response({ ...first, previewId: "next-backup" }));
    await home.previewBackup("Second");
    expect(api.cancelPortablePreview).toHaveBeenCalledWith(first.previewId);
    await home.dispose();
    expect(api.cancelPortablePreview).toHaveBeenCalledWith("next-backup");
    expect(home.state.backupPreview).toBeNull();
  });

  it("renders distinct loading and empty states", async () => {
    const loading = createHome(fakeApi());
    expect(await render(loading)).toContain("Loading saved projects");

    await loading.load();
    const emptyHtml = await render(loading);
    expect(emptyHtml).toContain("Begin with a local project");
    expect(emptyHtml).toContain("Create official.test project");
  });

  it("renders a user-safe message from a structured API error", async () => {
    const api = fakeApi();
    api.listProjects.mockRejectedValueOnce({
      category: "storage",
      code: "local_library.unavailable",
      message: "Library unavailable",
    });
    const home = createHome(api);

    await home.load();

    const html = await render(home);
    expect(html).toContain('role="alert"');
    expect(html).toContain("Library unavailable");
    expect(html).toContain("Try again");
  });

  it("does not render a raw runtime error message", async () => {
    const api = fakeApi();
    const runtimeMessage = "Cannot read properties of undefined (reading 'invoke')";
    api.listProjects.mockRejectedValueOnce(new Error(runtimeMessage));
    const home = createHome(api);

    await home.load();

    const html = await render(home);
    expect(html).toContain("The local project library could not complete that request.");
    expect(html).not.toContain(runtimeMessage);
  });

  it("creates and reloads authoritative state", async () => {
    const saved: ProjectSummary[] = [];
    const api = fakeApi(saved);
    api.listProjects.mockImplementation(() => Promise.resolve(response([...saved])));
    api.createProject.mockImplementation((input) => {
      saved.push({ ...project, title: input.title });
      return Promise.resolve(response({}));
    });
    const home = createHome(api);
    await home.load();
    await home.createProject({
      title: "Clinic roster",
      description: "Autumn plan",
      domainPack: { id: "official.test", schemaVersion: 1 },
      settings: {
        timeZone: "UTC",
        locale: "en-US",
        units: "metric",
        horizon: { start: "2026-09-01T00:00:00Z", end: "2026-10-01T00:00:00Z" },
        gapPolicy: "reject",
        overlapPolicy: "earlier",
      },
    });

    expect(home.state.projects).toEqual(saved);
    expect(await render(home)).toContain("Open project Clinic roster");
  });

  it("archives, unarchives, duplicates, and deletes only after reloading saved data", async () => {
    const saved = [{ ...project }];
    const api = fakeApi(saved);
    api.listProjects.mockImplementation(() =>
      Promise.resolve(response(saved.map((item) => ({ ...item })))),
    );
    api.setProjectArchived.mockImplementation(({ scenarioId, archived }) => {
      const item = saved.find((candidate) => candidate.scenarioId === scenarioId);
      if (item) item.archived = archived;
      return Promise.resolve(response({}));
    });
    api.duplicateProject.mockImplementation(({ title }) => {
      saved.push({ ...project, scenarioId: "01900000-0000-7000-8000-000000000002", title });
      return Promise.resolve(response({}));
    });
    api.deleteProject.mockImplementation((scenarioId) => {
      const index = saved.findIndex((candidate) => candidate.scenarioId === scenarioId);
      if (index >= 0) saved.splice(index, 1);
      return Promise.resolve(response({}));
    });
    const home = createHome(api);
    await home.load();

    await home.setArchived(projectAt(home, 0));
    expect(home.state.projects[0]?.archived).toBe(true);
    await home.setArchived(projectAt(home, 0));
    expect(home.state.projects[0]?.archived).toBe(false);
    await home.duplicateProject(projectAt(home, 0), "Clinic roster copy");
    expect(home.state.projects).toHaveLength(2);
    home.selectProject(projectAt(home, 1).scenarioId);
    await home.deleteProject(projectAt(home, 1));
    expect(home.state.projects).toHaveLength(1);
    expect(home.state.selectedId).toBe(project.scenarioId);
    expect(api.setProjectArchived).toHaveBeenCalledWith(
      expect.objectContaining({ expectedRevision: project.revision }),
    );
    expect(api.duplicateProject).toHaveBeenCalledWith(
      expect.objectContaining({ expectedRevision: project.revision }),
    );
    expect(api.deleteProject).toHaveBeenCalledWith(expect.any(String), project.revision);

    const html = await render(home);
    expect(html).toContain("Archive project");
    expect(html).toContain("Duplicate project as");
    expect(html).toContain("Delete project");
  });

  it("resolves reviewed scenario revisions from the selected collision action", () => {
    const scenario = portablePreview("scenario-export").scenarios[0];
    expect(scenarioRevisionOutcome(scenario, "create-copy")).toEqual({
      revision: 2,
      warning: null,
    });
    expect(scenarioRevisionOutcome(scenario, "replace")).toEqual({
      revision: 6,
      warning: scenario.sameIdentityRevisionWarning,
    });
    expect(scenarioRevisionOutcome(scenario, "skip")).toEqual({
      revision: null,
      warning: null,
    });
    expect(
      scenarioRevisionOutcome(
        { ...scenario, collides: false, title: "Tombstoned identity" },
        undefined,
      ),
    ).toEqual({
      revision: 6,
      warning: scenario.sameIdentityRevisionWarning,
    });
  });

  it.each(["import", "restore"] as const)(
    "distinguishes migration subjects and version spaces in the %s preview",
    async (mode) => {
      const api = fakeApi([project]);
      const otherScenarioId = "01900000-0000-7000-8000-000000000002";
      const subjects = [
        { scenarioId: project.scenarioId, revision: 2, versionSpace: "internal" },
        { scenarioId: project.scenarioId, revision: 3, versionSpace: "internal" },
        { scenarioId: otherScenarioId, revision: 2, versionSpace: "internal" },
        { scenarioId: project.scenarioId, revision: 2, versionSpace: "portable" },
      ] as const;
      const appliedMigrations: readonly AppliedMigrationDto[] = [
        ...portablePreview("scenario-export").appliedMigrations,
        ...subjects.map(({ scenarioId, revision, versionSpace }) => ({
          registry: "portable" as const,
          name: "pack-v1-to-v2",
          fromVersion: 1,
          toVersion: 2,
          versionSpace,
          subject: { packId: "official.test", scenarioId, revision },
        })),
      ];
      api.previewImport.mockResolvedValue(
        response({ ...portablePreview("scenario-export"), appliedMigrations }),
      );
      api.previewRestore.mockResolvedValue(
        response({ ...portablePreview("full-backup"), appliedMigrations }),
      );
      const home = createHome(api);
      await home.load();
      if (mode === "import") {
        await home.previewImport({ includeResults: true, includeAssets: true });
      } else {
        await home.previewRestore("add-backup");
      }
      const html = await render(home);
      const section =
        html.match(
          new RegExp(
            `<section aria-labelledby="${mode}-migrations-heading">([\\s\\S]*?)</section>`,
          ),
        )?.[1] ?? "";
      const rows = [...section.matchAll(/<li\b[^>]*>([\s\S]*?)<\/li>/g)].map((match) =>
        (match[1] ?? "")
          .replace(/<[^>]*>/g, "")
          .replace(/\s+/g, " ")
          .trim(),
      );
      for (const { scenarioId, revision, versionSpace } of subjects) {
        expect(rows).toContainEqual(
          expect.stringContaining(
            `${versionSpace} version space · pack official.test · scenario ${scenarioId} · revision ${revision.toString()}`,
          ),
        );
      }
    },
  );

  it("renders collision review for chooser-backed import and add/replace restore previews", async () => {
    const api = fakeApi([project]);
    const supplementalIdentity = { section: "preferences" as const, key: "view.json" };
    const sharedIdentity = { section: "shared-records" as const, key: "team.json" };
    const assetIdentity = { section: "assets" as const, key: "logo.png" };
    const supplementalIdentities = [supplementalIdentity, sharedIdentity, assetIdentity] as const;
    const supplementalDefaults = defaultSupplementalCollisionChoices(supplementalIdentities);
    expect(supplementalDefaults).toEqual({
      [`preferences\u0000view.json`]: "skip",
      [`shared-records\u0000team.json`]: "skip",
      [`assets\u0000logo.png`]: "skip",
    });
    api.previewImport.mockResolvedValue(
      response(
        {
          ...portablePreview("scenario-export"),
          supplementalCollisions: supplementalIdentities,
        },
        [previewWarning],
      ),
    );
    api.previewRestore.mockResolvedValue(
      response(
        {
          ...portablePreview("full-backup"),
          supplementalCollisions: supplementalIdentities,
          removedSupplemental: supplementalIdentities,
        },
        [previewWarning],
      ),
    );
    const home = createHome(api);
    await home.load();
    await home.previewImport({ includeResults: true, includeAssets: true });
    let html = await render(home);
    expect(api.previewImport).toHaveBeenCalledWith({
      restoreMode: "import-scenario",
      includeResults: true,
      includeAssets: true,
    });
    expect(html).toContain("Import preview: Imported roster");
    expect(html).toContain("Collision action");
    expect(html).toContain("Existing supplemental records");
    expect(html).toContain("Supplemental collision action");
    expect(html).toContain("team.json");
    expect(html).toContain("logo.png");
    expect(html).toContain("Apply reviewed import");
    expect(html).toContain("Include retained results");
    expect(html).toContain("Include referenced assets");
    expect(html).toContain("Create a copy");
    expect(html).toContain("eutheto-core");
    expect(html).toContain('datetime="2026-08-29T11:00:00Z"');
    expect(html).toContain("format 1, schema 1");
    expect(html).toContain("2 historical revisions");
    expect(html).toContain("0 results");
    expect(html).toContain("4 shared records");
    expect(html).toContain("5 preferences");
    expect(html).toContain("6 assets");
    expect(html).toContain("explicitly excluded");
    expect(html).toContain("v1-threshold");
    expect(html).toContain("large-video.mp4");
    expect(html).toContain("above-v1-threshold");
    expect(html).toContain("video/mp4");
    expect(html).toContain("portable.history");
    expect(html).toContain("example.extension");
    expect(html).toContain("portable-v0-to-v1");
    expect(html).toContain("Review migrated portable data.");
    expect(html).toContain("Source revision 2");
    expect(html).toContain("selected outcome revision");
    expect(html).toContain("2");
    expect(html).not.toContain('role="alert"');
    expect(html).not.toContain("Same-identity revision warning:");
    for (const label of fixedExclusionLabels) {
      expect(html).toContain(label);
      expect(html.indexOf(label)).toBeLessThan(html.indexOf("Apply reviewed import"));
    }
    expect(html).not.toContain("everything is included");
    await home.applyImport(
      {
        [project.scenarioId]: "create-copy",
        "01900000-0000-7000-8000-000000000099": "skip",
      },
      supplementalDefaults,
    );
    expect(api.applyImport).toHaveBeenCalledWith({
      previewId: "01900000-0000-7000-8000-000000000010",
      collisionPlan: {
        scenarios: { [project.scenarioId]: "create-copy" },
        supplementalChoices: [
          { ...supplementalIdentity, action: "skip" },
          { ...sharedIdentity, action: "skip" },
          { ...assetIdentity, action: "skip" },
        ],
      },
    });

    await home.previewRestore("add-backup");
    html = await render(home);
    expect(api.previewRestore).toHaveBeenLastCalledWith({
      restoreMode: "add-backup",
      includeResults: true,
      includeAssets: true,
    });
    expect(html).toContain("Add preview:");
    for (const label of fixedExclusionLabels) expect(html).toContain(label);
    expect(html).toContain("Review and confirm restore");
    expect(html).toContain("Source revision 2");
    expect(html).toContain("selected outcome revision");
    expect(html).not.toContain("Same-identity revision warning:");

    await home.previewRestore("replace-library");
    html = await render(home);
    expect(html).toContain("selected outcome revision");
    expect(html).toContain("6");
    expect(html).toContain('role="alert"');
    expect(html).toContain("Same-identity revision warning:");
    expect(html).toContain("A deleted project previously used this ID");
    expect(html).toContain("Library replacement:");
    expect(html).toContain("projects absent from this backup will be removed");
    expect(html).toContain("Current projects that will be removed");
    expect(html).toContain("revision 3");
    expect(html).toContain("active");
    expect(html).toContain("Current supplemental records that will be replaced or removed");
    expect(html).toContain("Application setting changes");
    expect(html).toContain("appearance");
    expect(html).toContain("units");
    expect(html).toContain("2 historical revisions");
    expect(html).toContain('datetime="2026-08-29T11:00:00Z"');
    expect(html).toContain("portable.history");
    expect(html).toContain("example.extension");
    expect(html).toContain("portable-v0-to-v1");
    expect(html).toContain("0 results");
    expect(html).toContain("explicitly excluded");
    expect(html).toContain("large-video.mp4");
    expect(html).toContain("above-v1-threshold");
    expect(html).toContain("video/mp4");
    expect(html).toContain("Review migrated portable data.");
    expect(html).toContain("Included by library replacement");
    expect(html).not.toContain("Supplemental collision action");
    expect(html).not.toContain(`restore-collision-${project.scenarioId}`);
    const replaceDefaults = defaultSupplementalCollisionChoices(supplementalIdentities, "replace");
    expect(Object.values(replaceDefaults)).toEqual(["replace", "replace", "replace"]);
    await home.applyRestore({ [project.scenarioId]: "create-copy" }, replaceDefaults);
    expect(api.applyRestore).toHaveBeenLastCalledWith({
      previewId: "01900000-0000-7000-8000-000000000010",
      collisionPlan: {
        scenarios: {},
        supplementalChoices: [],
      },
      authorization: {
        destructiveActionConfirmed: true,
        safetyBackupBypassPhrase: null,
      },
    });
  });

  it("previews and saves a backup through the native Save dialog", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    await home.previewBackup("Before changes");
    const html = await render(home);
    expect(html).toContain("Prepared library revision 1");
    expect(html).toContain("b".repeat(64));
    expect(html).toContain("Backup preview: Before changes");
    expect(html).toContain("Save backup file");
    expect(html).toContain("Results included");
    expect(html).toContain("Asset selection: all");
    expect(html).toContain("1 preserved omission placeholder");
    expect(html).toContain("inherited-placeholder.png");
    expect(html).toContain("inherited-placeholder");
    for (const label of fixedExclusionLabels) {
      expect(html).toContain(label);
      expect(html.indexOf(label)).toBeLessThan(html.indexOf("Save backup file"));
    }
    expect(html).not.toContain("everything is included");

    await home.createBackup("Before changes");
    expect(api.createBackup).toHaveBeenCalledWith(
      "Before changes",
      "01900000-0000-7000-8000-000000000070",
    );
    expect(home.state.announcement).toBe("Backup saved as before-changes.eutheto.");
    expect(home.state.announcement).not.toContain("/");
  });

  it("treats typed native-dialog cancellation as a non-mutating result", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    await home.previewImport({ includeResults: true, includeAssets: true });
    await home.previewRestore("add-backup");
    await home.previewBackup("Before changes");
    home.state.errorMessage = "Keep this notice";
    home.state.announcement = "Keep this announcement";
    const importPreview = home.state.importPreview;
    const restorePreview = home.state.restorePreview;
    const projects = [...home.state.projects];
    const cancelled = {
      category: "protocol",
      code: "operation.cancelled",
      message: "No file was selected.",
      retryable: true,
    };

    api.previewImport.mockRejectedValueOnce(cancelled);
    api.previewRestore.mockRejectedValueOnce(cancelled);
    api.createBackup.mockRejectedValueOnce(cancelled);

    expect(await home.previewImport({ includeResults: false, includeAssets: false })).toBe(false);
    expect(await home.previewRestore("replace-library")).toBe(false);
    expect(await home.createBackup("Before changes")).toBe(false);
    expect(home.state.busyAction).toBeNull();
    expect(home.state.importPreview).toBe(importPreview);
    expect(home.state.restorePreview).toBe(restorePreview);
    expect(home.state.backupPreview).toBeNull();
    expect(home.state.restoreMode).toBe("add-backup");
    expect(home.state.projects).toEqual(projects);

    expect(home.state.errorMessage).toBe("Keep this notice");
    expect(home.state.announcement).toBe("Keep this announcement");
    expect(api.listProjects).toHaveBeenCalledOnce();
  });
  it("retains a failed replace preview only for an informed second backup bypass", async () => {
    const api = fakeApi([project]);
    const home = createHome(api);
    await home.load();
    await home.previewRestore("replace-library");
    api.applyRestore
      .mockRejectedValueOnce({
        category: "protocol",
        code: "restore.safety_backup_failed",
        message: "The private backup destination is unavailable.",
        retryable: false,
      })
      .mockResolvedValueOnce(response({}));

    expect(await home.applyRestore({}, {})).toBe(false);
    expect(home.state.restorePreview).not.toBeNull();
    expect(home.state.restoreSafetyBackupFailure).toBe(
      "The private backup destination is unavailable.",
    );

    expect(await home.applyRestore({}, {}, "REPLACE WITHOUT BACKUP")).toBe(true);
    expect(home.state.restorePreview).toBeNull();
    expect(home.state.restoreSafetyBackupFailure).toBeNull();
    expect(api.applyRestore).toHaveBeenNthCalledWith(
      1,
      expect.objectContaining({
        authorization: expect.objectContaining({ safetyBackupBypassPhrase: null }),
      }),
    );
    expect(api.applyRestore).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({
        authorization: expect.objectContaining({
          safetyBackupBypassPhrase: "REPLACE WITHOUT BACKUP",
        }),
      }),
    );
  });

  it("announces revision conflicts and reloads the authoritative project list", async () => {
    const saved = [{ ...project }];
    const api = fakeApi(saved);
    api.listProjects.mockImplementation(() =>
      Promise.resolve(response(saved.map((item) => ({ ...item })))),
    );
    api.duplicateProject.mockRejectedValueOnce({
      category: "conflict",
      code: "project.revision_conflict",
      message: "Revision conflict",
    });
    const home = createHome(api);
    await home.load();
    const savedProject = saved[0];
    expect(savedProject).toBeDefined();
    if (savedProject === undefined) {
      throw new Error("Expected the saved project to exist");
    }
    saved[0] = { ...savedProject, title: "Authoritative title", revision: 4 };
    await home.duplicateProject(project, "Copy");

    expect(home.state.projects[0]?.title).toBe("Authoritative title");
    expect(home.state.announcement).toContain("changed in another window");
    expect(await render(home)).toContain('aria-live="polite"');
  });

  it("refreshes from native scenario events and disposes listeners and previews", async () => {
    const api = fakeApi([project]);
    let changed: ((event: ScenarioChangedEvent) => void) | undefined;
    const unlistenChanged = vi.fn();
    let revision = project.revision;
    api.listProjects.mockImplementation(() =>
      Promise.resolve(response([{ ...project, revision }])),
    );
    const unlistenValidation = vi.fn();
    let notification: (() => void) | undefined;
    const unlistenNotification = vi.fn();
    let refreshRequired: (() => void) | undefined;
    const unlistenRefreshRequired = vi.fn();
    api.onScenarioChanged.mockImplementation((listener) => {
      changed = listener;
      return Promise.resolve(unlistenChanged);
    });
    api.onAppNotification.mockImplementation((listener) => {
      notification = listener;
      return Promise.resolve(unlistenNotification);
    });
    api.onScenarioValidationChanged.mockResolvedValue(unlistenValidation);
    api.onLibraryRefreshRequired.mockImplementation((listener) => {
      refreshRequired = listener;
      return Promise.resolve(unlistenRefreshRequired);
    });
    const home = createHome(api);
    await home.startEventListeners();
    await home.load();
    await home.previewImport({ includeResults: true, includeAssets: true });
    revision += 1;

    changed?.({
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
    });
    await vi.waitFor(() => {
      expect(home.state.projects[0]?.revision).toBe(revision);
    });
    revision += 1;
    notification?.();
    await vi.waitFor(() => {
      expect(home.state.projects[0]?.revision).toBe(revision);
    });
    revision += 1;
    refreshRequired?.();
    await vi.waitFor(() => {
      expect(home.state.projects[0]?.revision).toBe(revision);
    });

    await home.dispose();
    expect(unlistenChanged).toHaveBeenCalledOnce();
    expect(unlistenValidation).toHaveBeenCalledOnce();
    expect(unlistenNotification).toHaveBeenCalledOnce();
    expect(unlistenRefreshRequired).toHaveBeenCalledOnce();
    expect(api.cancelPortablePreview).toHaveBeenCalledWith("01900000-0000-7000-8000-000000000010");
  });

  it("provides explicit accessible names", async () => {
    const home = createHome(fakeApi([project]));
    await home.load();
    const html = await render(home);
    expect(html).toContain('aria-label="Open project Clinic roster"');
    expect(html).toContain("Project title");
    expect(html).toContain("Choose import file");
    expect(html).toContain("Choose backup file");
    expect(html).toContain("Cancelling a file chooser or Save dialog");
    expect(html).toContain('tabindex="-1"');
  });
});
