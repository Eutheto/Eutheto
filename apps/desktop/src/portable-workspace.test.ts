import { createSSRApp, effectScope } from "vue";
import { createPinia, disposePinia } from "pinia";
import { PiniaColada } from "@pinia/colada";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import {
  PortableReviewFlow,
  type ApiResponseDto,
  type PortableArtifactDto,
  type PortableAppliedDto,
  type PortableFilePreviewDto,
  type PortablePreviewDto,
} from "./api/generated";
import {
  createProjectHomeController,
  scenarioRevisionOutcome,
  supplementalIdentityKey,
} from "./project-home";
import {
  createPortableWorkspaceController,
  reviewedCollisionPlan,
  type PortableContext,
} from "./portable-workspace";
import {
  fakeApi,
  portableApplied,
  portableOperation,
  portablePreview,
  project,
  response,
} from "./testing/project-home";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function fixture(initial: PortableContext) {
  const api = fakeApi([project]);
  const app = createSSRApp({ render: () => null });
  const pinia = createPinia();
  app.use(pinia);
  app.use(PiniaColada);
  const scope = effectScope();
  const home = app.runWithContext(() => scope.run(() => createProjectHomeController(api)));
  if (!home) throw new Error("Controller scope is required");
  const flow = new PortableReviewFlow();
  vi.spyOn(flow, "dispose").mockResolvedValue();
  let context = initial;
  const events = { exported: vi.fn(), exportCancelled: vi.fn(), exportFailed: vi.fn() };
  const workspace = createPortableWorkspaceController(
    home,
    () => context,
    events,
    () => flow,
  );
  onTestFinished(async () => {
    workspace.leave();
    await home.dispose();
    scope.stop();
    disposePinia(pinia);
  });
  return {
    api,
    home,
    workspace,
    flow,
    events,
    setContext: (next: PortableContext) => {
      context = next;
    },
  };
}

const exportPreview: PortableFilePreviewDto = {
  schemaVersion: 1,
  title: project.title,
  previewId: "01900000-0000-7000-8000-000000000070",
  byteLength: 4096,
  digest: "a".repeat(64),
  currentRevision: project.revision,
  libraryRevision: 7,
  backupSummary: null,
};

describe("portable review boundaries", () => {
  it("requires an explicit choice for both collision kinds and omits unrelated draft keys", () => {
    const preview = {
      ...portablePreview("scenario-export"),
      supplementalCollisions: [{ section: "assets" as const, key: "retained-image" }],
    };
    const scenario = preview.scenarios[0];
    const supplemental = preview.supplementalCollisions[0];
    if (!supplemental) throw new Error("Collision fixtures are required");
    expect(reviewedCollisionPlan(preview, {}, {})).toBeNull();
    expect(reviewedCollisionPlan(preview, { [scenario.scenarioId]: "create-copy" }, {})).toBeNull();
    expect(
      reviewedCollisionPlan(
        preview,
        { [scenario.scenarioId]: "replace", unrelated: "replace" },
        {
          [supplementalIdentityKey(supplemental)]: "skip",
          unrelated: "replace",
        },
      ),
    ).toEqual({
      scenarios: { [scenario.scenarioId]: "replace" },
      supplementalChoices: [{ ...supplemental, action: "skip" }],
    });
  });

  it.each(["success", "failure"] as const)(
    "does not publish a departed preview's late %s into a reused route lifetime",
    async (outcome) => {
      const { workspace, flow } = fixture({ mode: "import", libraryRevision: 1 });
      const terminal = deferred<ApiResponseDto<PortablePreviewDto>>();
      vi.spyOn(flow, "previewImport").mockReturnValue(portableOperation(terminal.promise));
      const pending = workspace.previewImport(true, true);
      workspace.leave();
      workspace.enter();
      if (outcome === "success")
        terminal.resolve(response(portablePreview("scenario-export"), [], 1));
      else
        terminal.reject({ category: "storage", code: "preview.failed", message: "Late failure" });
      await pending;
      expect(workspace.state.review).toBeNull();
      expect(workspace.state.notice).toBeNull();
      expect(workspace.state.error).toBeNull();
    },
  );

  it("keeps cancellation acknowledgement nonterminal and reports late committed export only at the root", async () => {
    const { home, workspace, flow, events } = fixture({
      mode: "export",
      project,
      libraryRevision: 7,
    });
    vi.spyOn(flow, "previewExport").mockReturnValue(
      portableOperation(response(exportPreview, [], project.revision)),
    );
    await workspace.previewExport();
    const terminal = deferred<ApiResponseDto<PortableArtifactDto>>();
    const native = portableOperation(terminal.promise);
    vi.spyOn(flow, "createExport").mockReturnValue(native);
    const saving = workspace.save();
    await home.cancelOperation();
    expect(workspace.state.pending).toBe(true);
    expect(home.state.operation?.settled).toBe(false);
    expect(events.exportCancelled).not.toHaveBeenCalled();
    workspace.leave();
    terminal.resolve(
      response(
        {
          schemaVersion: 1,
          artifactName: "committed.eutheto",
          currentRevision: project.revision,
          libraryRevision: 7,
        },
        [],
        project.revision,
      ),
    );
    await saving;
    expect(events.exported).not.toHaveBeenCalled();
    expect(events.exportCancelled).not.toHaveBeenCalled();
    expect(events.exportFailed).not.toHaveBeenCalled();
    expect(home.state.announcement).toContain("committed.eutheto");
    expect(home.state.busyAction).toBeNull();
    expect(workspace.state.review).toBeNull();
  });

  it("never rebinds a captured export to newer project or library props", async () => {
    const { workspace, flow, setContext } = fixture({
      mode: "export",
      project,
      libraryRevision: 7,
    });
    const terminal = deferred<ApiResponseDto<PortableFilePreviewDto>>();
    vi.spyOn(flow, "previewExport").mockReturnValue(portableOperation(terminal.promise));
    const create = vi.spyOn(flow, "createExport");
    const pending = workspace.previewExport();
    setContext({ mode: "export", project: { ...project, revision: 4 }, libraryRevision: 8 });
    workspace.refreshBinding();
    terminal.resolve(response(exportPreview, [], project.revision));
    await pending;
    expect(workspace.state.stale).toBe(true);
    expect(workspace.state.review).toMatchObject({
      kind: "export",
      scenarioRevision: 3,
      preview: { libraryRevision: 7 },
    });
    expect(await workspace.save()).toBe(false);
    expect(create).not.toHaveBeenCalled();
  });

  it("does not mistake publication epochs or a pre-receipt self-change for a stale review", async () => {
    const { home, workspace, flow, setContext } = fixture({
      mode: "backup-restore",
      libraryRevision: 1,
    });
    vi.spyOn(flow, "previewRestore").mockReturnValue(
      portableOperation(response(portablePreview("full-backup"), [], 1)),
    );
    await workspace.previewRestore("replace-library", "safetyBackups");
    home.state.libraryEpoch += 1;
    workspace.refreshBinding();
    expect(workspace.state.stale).toBe(false);
    const terminal = deferred<ApiResponseDto<PortableAppliedDto>>();
    vi.spyOn(flow, "applyRestore").mockReturnValue(portableOperation(terminal.promise));
    const applying = workspace.apply({}, {}, "replace");
    setContext({ mode: "backup-restore", libraryRevision: 2 });
    workspace.refreshBinding();
    expect(workspace.state.stale).toBe(false);
    terminal.resolve(
      portableApplied({ kind: "createdAndVerified", artifactName: "actual-safety.eutheto" }),
    );
    expect(await applying).toBe(true);
    expect(workspace.state.review).toBeNull();
    expect(workspace.state.notice).toContain("actual-safety.eutheto");
  });

  it("allows safety bypass only after exact native retained handback for the same review", async () => {
    const { workspace, flow } = fixture({ mode: "backup-restore", libraryRevision: 1 });
    vi.spyOn(flow, "previewRestore").mockReturnValue(
      portableOperation(response(portablePreview("full-backup"), [], 1)),
    );
    await workspace.previewRestore("replace-library", "userSelected");
    const terminal = deferred<ApiResponseDto<PortableAppliedDto>>();
    const nativeApply = vi
      .spyOn(flow, "applyRestore")
      .mockReturnValueOnce(portableOperation(terminal.promise))
      .mockImplementationOnce(() =>
        portableOperation(portableApplied({ kind: "confirmedBypass" })),
      );
    expect(await workspace.apply({}, {}, "bypass", "REPLACE WITHOUT BACKUP")).toBe(false);
    expect(nativeApply).not.toHaveBeenCalled();
    const applying = workspace.apply({}, {}, "replace");
    terminal.reject({
      category: "storage",
      code: "restore.safety_backup_failed",
      message: "Safety device is full.",
      details: { portablePreviewRetained: { type: "boolean", value: true } },
    });
    expect(await applying).toBe(false);
    expect(workspace.state.safetyFailure).toBe("Safety device is full.");
    expect(workspace.state.review?.kind).toBe("restore");
    expect(await workspace.apply({}, {}, "replace")).toBe(false);
    expect(await workspace.apply({}, {}, "bypass", "replace without backup")).toBe(false);
    expect(nativeApply).toHaveBeenCalledTimes(1);
    expect(await workspace.apply({}, {}, "bypass", "REPLACE WITHOUT BACKUP")).toBe(true);
    expect(workspace.state.review).toBeNull();
  });

  it.each([
    undefined,
    { portablePreviewRetained: { type: "boolean", value: false } },
    { portablePreviewRetained: { type: "boolean", value: "true" } },
  ])(
    "does not guess safety-failure retention from missing or inexact handback: %j",
    async (details) => {
      const { workspace, flow } = fixture({ mode: "backup-restore", libraryRevision: 1 });
      vi.spyOn(flow, "previewRestore").mockReturnValue(
        portableOperation(response(portablePreview("full-backup"), [], 1)),
      );
      await workspace.previewRestore("replace-library", "userSelected");
      const terminal = deferred<ApiResponseDto<PortableAppliedDto>>();
      vi.spyOn(flow, "applyRestore").mockReturnValue(portableOperation(terminal.promise));
      const applying = workspace.apply({}, {}, "replace");
      terminal.reject({
        category: "storage",
        code: "restore.safety_backup_failed",
        message: "No retained custody.",
        details,
      });
      await applying;
      expect(workspace.state.review).toBeNull();
      expect(workspace.state.safetyFailure).toBeNull();
      expect(await workspace.apply({}, {}, "bypass", "REPLACE WITHOUT BACKUP")).toBe(false);
    },
  );

  it("preserves copy, skip and tombstoned same-identity revision outcomes", () => {
    const scenario = portablePreview("scenario-export").scenarios[0];
    expect(scenarioRevisionOutcome(scenario, "create-copy")).toEqual({
      revision: 2,
      warning: null,
    });
    expect(scenarioRevisionOutcome(scenario, "replace")).toEqual({
      revision: 6,
      warning: scenario.sameIdentityRevisionWarning,
    });
    expect(scenarioRevisionOutcome(scenario, "skip")).toEqual({ revision: null, warning: null });
    expect(scenarioRevisionOutcome({ ...scenario, collides: false }, undefined)).toEqual({
      revision: 6,
      warning: scenario.sameIdentityRevisionWarning,
    });
  });

  it("refuses overlapping mutations without losing the existing portable review", async () => {
    const { api, home, workspace, flow } = fixture({ mode: "import", libraryRevision: 1 });
    vi.spyOn(flow, "previewImport").mockReturnValue(
      portableOperation(response(portablePreview("scenario-export"), [], 1)),
    );
    const apply = vi.spyOn(flow, "applyImport");
    await home.load();
    await workspace.previewImport(true, true);
    const review = workspace.state.review;
    const terminal = deferred<ApiResponseDto<unknown>>();
    api.duplicateProject.mockReturnValueOnce(terminal.promise);
    const duplicating = home.duplicateProject(project, "Copy");
    expect(await workspace.apply({ [project.scenarioId]: "create-copy" }, {}, "reviewed")).toBe(
      false,
    );
    expect(await workspace.inspect()).toBe(false);
    expect(workspace.state.review).toBe(review);
    expect(apply).not.toHaveBeenCalled();
    terminal.resolve(response({}));
    await duplicating;
    expect(workspace.state.review).toBe(review);
  });

  it("consumes a committed import once even when the subsequent library refresh fails", async () => {
    const { api, home, workspace, flow } = fixture({ mode: "import", libraryRevision: 1 });
    vi.spyOn(flow, "previewImport").mockReturnValue(
      portableOperation(response(portablePreview("scenario-export"), [], 1)),
    );
    const apply = vi
      .spyOn(flow, "applyImport")
      .mockReturnValue(portableOperation(portableApplied()));
    await home.load();
    await workspace.previewImport(true, true);
    api.listProjects.mockRejectedValueOnce({
      category: "storage",
      code: "read.failed",
      message: "Refresh unavailable",
    });
    expect(await workspace.apply({ [project.scenarioId]: "create-copy" }, {}, "reviewed")).toBe(
      true,
    );
    expect(workspace.state.review).toBeNull();
    expect(home.state.phase).toBe("error");
    expect(home.state.errorMessage).toContain("Refresh unavailable");
    const committedNotice = workspace.state.notice;
    const committedAnnouncement = home.state.announcement;
    expect(await workspace.apply({ [project.scenarioId]: "create-copy" }, {}, "reviewed")).toBe(
      false,
    );
    await home.refreshLibrary();
    expect(apply).toHaveBeenCalledOnce();
    expect(workspace.state.notice).toBe(committedNotice);
    expect(home.state.announcement).toBe(committedAnnouncement);
  });

  it.each(["cancelled", "failed"] as const)(
    "reports current export %s without deleting a project or reusing consumed custody",
    async (outcome) => {
      const { api, home, workspace, flow, events } = fixture({
        mode: "export",
        project,
        libraryRevision: 7,
      });
      await home.load();
      vi.spyOn(flow, "previewExport").mockReturnValue(
        portableOperation(response(exportPreview, [], project.revision)),
      );
      await workspace.previewExport();
      const terminal = deferred<ApiResponseDto<PortableArtifactDto>>();
      const save = vi
        .spyOn(flow, "createExport")
        .mockReturnValue(portableOperation(terminal.promise));
      const saving = workspace.save();
      terminal.reject(
        outcome === "cancelled"
          ? { category: "protocol", code: "operation.cancelled", message: "Save chooser cancelled" }
          : { category: "storage", code: "publish.failed", message: "Publication not confirmed" },
      );
      expect(await saving).toBe(false);
      expect(events.exportCancelled).toHaveBeenCalledTimes(outcome === "cancelled" ? 1 : 0);
      expect(events.exportFailed).toHaveBeenCalledTimes(outcome === "failed" ? 1 : 0);
      expect(events.exported).not.toHaveBeenCalled();
      expect(workspace.state.review).toBeNull();
      expect(home.state.projects).toEqual([project]);
      expect(api.deleteProject).not.toHaveBeenCalled();
      expect(await workspace.save()).toBe(false);
      expect(save).toHaveBeenCalledOnce();
    },
  );

  it("rejects a stale import without retrying the consumed review against refreshed revisions", async () => {
    const { api, home, workspace, flow } = fixture({ mode: "import", libraryRevision: 1 });
    await home.load();
    vi.spyOn(flow, "previewImport").mockReturnValue(
      portableOperation(response(portablePreview("scenario-export"), [], 1)),
    );
    await workspace.previewImport(true, true);
    const terminal = deferred<ApiResponseDto<PortableAppliedDto>>();
    const apply = vi
      .spyOn(flow, "applyImport")
      .mockReturnValue(portableOperation(terminal.promise));
    const applying = workspace.apply({ [project.scenarioId]: "create-copy" }, {}, "reviewed");
    const changed = { ...project, revision: 4 };
    api.listProjects.mockResolvedValueOnce(response([changed]));
    terminal.reject({
      category: "conflict",
      code: "library.revision_conflict",
      message: "Library revision changed",
    });
    expect(await applying).toBe(false);
    expect(home.state.projects).toEqual([changed]);
    expect(workspace.state.review).toBeNull();
    expect(await workspace.apply({ [project.scenarioId]: "create-copy" }, {}, "reviewed")).toBe(
      false,
    );
    expect(apply).toHaveBeenCalledOnce();
  });
});
