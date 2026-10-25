/* eslint-disable vue/one-component-per-file -- Browser-only routing fixtures need distinct components in one isolated harness. Production SFCs retain the rule. */
import { cleanup, render, screen, within } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import axe from "axe-core";
import { createPinia } from "pinia";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { defineComponent, h, nextTick, ref } from "vue";
import { createMemoryHistory, createRouter, RouterLink, RouterView, useRoute } from "vue-router";

import {
  createProjectHomeController,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";
import type {
  ApiResponseDto,
  EmptyDto,
  EvidenceMessageV1,
  PortableArtifactDto,
  PortableFilePreviewDto,
  SetupOperation,
  SolutionComparisonV2,
  SolutionExplanationDtoV2,
  SolutionListDtoV1,
  SolutionSummaryDtoV1,
  VerificationValue,
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
import { plannerMessage } from "./planner/messages";

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
  vi.mocked(generatedApi.manualEdit).mockImplementation(() =>
    portableOperation(
      Promise.reject(
        Object.assign(new Error("Adopt an original accepted result before manual editing."), {
          category: "validation",
          code: "manual.base_missing",
        }),
      ),
    ),
  );
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

function mockWorkforceSetupResults(
  results: readonly SolutionSummaryDtoV1[],
  listRevision = 3,
): void {
  vi.mocked(generatedApi.getSolutionView).mockImplementation((input) => {
    const summary = results.find((item) => item.result.solutionId === input.solutionId);
    if (!summary || input.parameters?.kind !== "repairDiff") {
      throw new Error("Unexpected result view in comparison fixture");
    }
    return Promise.resolve(
      response(
        {
          schemaVersion: 1,
          scenarioId: workforceProject.scenarioId,
          currentRevision: listRevision,
          scenarioRevision: summary.scenarioRevision,
          result: summary.result,
          view: {
            viewId: "official.workforce.result.schedule",
            data: {
              schemaVersion: 1,
              timeZone: "UTC",
              result: {
                kind: "repairDiff",
                availability: "absent",
                source: {
                  scenarioId: workforceProject.scenarioId,
                  solutionId: input.solutionId,
                  sourceRevision: summary.scenarioRevision,
                  sourceDocumentHash: "a".repeat(64),
                  normalizedSolutionHash: "b".repeat(64),
                },
                baseline: null,
                counts: null,
                totalItems: null,
                items: [],
                continuation: null,
              },
            },
          },
        },
        [],
        listRevision,
      ),
    );
  });
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
                planningDates: { startDate: "2030-01-01", endDateExclusive: "2030-02-01" },
                initialWorkWindow: { startDate: "2030-01-01", endDateExclusive: "2030-01-08" },
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
  vi.mocked(generatedApi.listSolutions).mockImplementation(() =>
    Promise.resolve(
      response({
        schemaVersion: 1,
        scenarioId: workforceProject.scenarioId,
        currentRevision: listRevision,
        freshlyVerified: false,
        solutions: results,
      }),
    ),
  );
}
function acceptedSummary(
  solutionId: string,
  resultChecksum: string,
  runId: string,
  scenarioRevision: number,
  selected = false,
): SolutionSummaryDtoV1 {
  return {
    schemaVersion: 1,
    result: { solutionId, resultChecksum },
    runId,
    scenarioId: workforceProject.scenarioId,
    scenarioRevision,
    currentRevision: workforceProject.revision,
    stale: scenarioRevision !== workforceProject.revision,
    selected,
    status: "optimal",
    score: { feasibility: "0", levels: [] },
    verificationChecksum: "v".repeat(64),
    finishedAt: "2030-01-01T12:00:00Z",
  };
}

function comparisonFor(
  base: SolutionSummaryDtoV1,
  candidate: SolutionSummaryDtoV1,
): SolutionComparisonV2 {
  return {
    schemaVersion: 2,
    base: {
      packId: "official.workforce",
      scenarioId: base.scenarioId,
      scenarioRevision: base.scenarioRevision,
      documentHash: "a".repeat(64),
      projectionVersion: 1,
      verificationScopeChecksum: "b".repeat(64),
      acceptedResult: base.result,
    },
    candidate: {
      packId: "official.workforce",
      scenarioId: candidate.scenarioId,
      scenarioRevision: candidate.scenarioRevision,
      documentHash: "a".repeat(64),
      projectionVersion: 1,
      verificationScopeChecksum: "b".repeat(64),
      acceptedResult: candidate.result,
    },
    assignments: [],
    rules: [],
    score: { type: "unavailable", reason: "policyOrScopeMismatch" },
    metrics: [],
    protections: [],
    runs: {
      base: {
        runId: base.runId,
        runManifestChecksum: "d".repeat(64),
        outcome: {
          type: "accepted",
          status: "optimal",
          solutionId: base.result.solutionId,
          acceptedResultChecksum: base.result.resultChecksum,
          verificationChecksum: base.verificationChecksum,
        },
      },
      candidate: {
        runId: candidate.runId,
        runManifestChecksum: "e".repeat(64),
        outcome: {
          type: "accepted",
          status: "optimal",
          solutionId: candidate.result.solutionId,
          acceptedResultChecksum: candidate.result.resultChecksum,
          verificationChecksum: candidate.verificationChecksum,
        },
      },
    },
    affectedEntities: [],
    entityObservations: [],
    checksum: "c".repeat(64),
  };
}

function differenceResponse(
  base: SolutionSummaryDtoV1,
  candidate: SolutionSummaryDtoV1,
  comparison = comparisonFor(base, candidate),
  scenarioRevisions = [base.scenarioRevision, candidate.scenarioRevision],
): ApiResponseDto<SolutionExplanationDtoV2> {
  const evidenceChecksum = "f".repeat(64);
  const currentRevision = workforceProject.revision;
  return response(
    {
      schemaVersion: 2,
      scenarioId: workforceProject.scenarioId,
      currentRevision,
      scenarioRevisions,
      explanation: {
        schemaVersion: 2,
        evidence: {
          schemaVersion: 2,
          certainty: "deterministic",
          evidence: { kind: "solutionDifference", comparison },
          checksum: evidenceChecksum,
        },
        rendered: {
          schemaVersion: 1,
          kind: "solutionDifference",
          evidenceChecksum,
          messages: [],
        },
        checksum: "g".repeat(64),
      },
    },
    [],
    currentRevision,
  );
}
function repairResponse(
  base: SolutionSummaryDtoV1,
  candidate: SolutionSummaryDtoV1,
  scenarioRevisions = [candidate.scenarioRevision, base.scenarioRevision],
  comparison = comparisonFor(base, candidate),
  messages: readonly EvidenceMessageV1[] = [],
  currentRevision = workforceProject.revision,
): ApiResponseDto<SolutionExplanationDtoV2> {
  const evidenceChecksum = "h".repeat(64);
  return response(
    {
      schemaVersion: 2,
      scenarioId: workforceProject.scenarioId,
      currentRevision,
      scenarioRevisions,
      explanation: {
        schemaVersion: 2,
        evidence: {
          schemaVersion: 2,
          certainty: "deterministic",
          evidence: { kind: "repair", repair: { comparison, causality: "notEstablished" } },
          checksum: evidenceChecksum,
        },
        rendered: {
          schemaVersion: 1,
          kind: "repair",
          evidenceChecksum,
          messages,
        },
        checksum: "i".repeat(64),
      },
    },
    [],
    currentRevision,
  );
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
      .toMatchTextContent("reviewed-roster.eutheto");
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
  it.each(["retained", "pointer intervention", "stale focus"])(
    "reveals Tab keyup without native host focusin or focus-visible: %s",
    async (ownership) => {
      configureNative();
      await renderWorkspace();
      await screen.findByRole("heading", { name: messages.projects.heading });
      const main = screen.getByRole("main");
      const origin = document.createElement("button");
      origin.textContent = "Date focus origin";
      const input = document.createElement("input");
      input.type = "date";
      input.style.display = "block";
      input.style.marginTop = "200vh";
      main.prepend(origin, input);
      const matches = input.matches.bind(input);
      vi.spyOn(input, "matches").mockImplementation((selector) =>
        selector === ":focus-visible" ? false : matches(selector),
      );
      // Emulate the observed WebKit host: focus changes without bubbling focusin or UA reveal.
      input.addEventListener("focusin", (event) => {
        event.stopPropagation();
      });
      input.addEventListener("keyup", (event) => {
        if (event.code !== "Tab") return;
        window.scrollTo(0, 0);
        if (ownership === "pointer intervention")
          input.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
        if (ownership === "stale focus") origin.focus({ preventScroll: true });
      });
      origin.focus();
      await userEvent.keyboard("{Tab}");
      expect(input.matches(":focus-visible")).toBe(false);
      if (ownership === "retained") {
        await expect.poll(() => input.getBoundingClientRect().bottom <= innerHeight).toBe(true);
        expect(document.activeElement).toBe(input);
      } else {
        await new Promise<void>((resolve) =>
          requestAnimationFrame(() =>
            requestAnimationFrame(() => {
              resolve();
            }),
          ),
        );
        expect(scrollY).toBe(0);
        expect(input.getBoundingClientRect().top).toBeGreaterThan(innerHeight);
        expect(document.activeElement).toBe(ownership === "stale focus" ? origin : input);
      }
    },
  );

  it("reveals offscreen keyboard focus without scrolling for stale or mouse focus", async () => {
    configureNative();
    await renderWorkspace();
    await screen.findByRole("heading", { name: messages.projects.heading });
    const main = screen.getByRole("main");
    const origin = document.createElement("button");
    origin.textContent = "Focus origin";
    main.prepend(origin);
    const details = document.createElement("details");
    details.className = "state-panel setup-more";
    details.style.marginTop = "200vh";
    const summary = document.createElement("summary");
    summary.textContent = "Saved metadata focus regression";
    details.append(summary);
    main.append(details);

    origin.focus();
    await userEvent.keyboard("{Tab}");
    window.scrollTo(0, 0);
    // Reproduce native WebKit's observed focus transition without its expected reveal.
    summary.focus({ preventScroll: true });
    expect(summary.matches(":focus-visible")).toBe(true);
    expect(summary.getBoundingClientRect().top).toBeGreaterThan(innerHeight);
    await expect.poll(() => summary.getBoundingClientRect().bottom <= innerHeight).toBe(true);

    origin.focus();
    window.scrollTo(0, 0);
    const before = scrollY;
    summary.focus({ preventScroll: true });
    origin.focus({ preventScroll: true });
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          resolve();
        }),
      ),
    );
    expect(document.activeElement).toBe(origin);
    expect(scrollY).toBe(before);

    const mouseOrigin = document.createElement("button");
    mouseOrigin.textContent = "Mouse focus origin";
    main.prepend(mouseOrigin);
    await userEvent.click(mouseOrigin);
    expect(mouseOrigin.matches(":focus-visible")).toBe(false);
    summary.focus({ preventScroll: true });
    expect(summary.matches(":focus-visible")).toBe(false);
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          resolve();
        }),
      ),
    );
    expect(scrollY).toBe(before);
    expect(summary.getBoundingClientRect().top).toBeGreaterThan(innerHeight);
  });

  it("shows a first-launch library failure and lets the user retry it", async () => {
    configureNative();
    vi.mocked(generatedApi.listProjects).mockRejectedValueOnce(
      new Error("Library temporarily unavailable"),
    );
    await renderWorkspace("/");
    await expect
      .element(await screen.findByRole("alert"))
      .toMatchTextContent(messages.projects.loadFailed);
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
    mockWorkforceSetupResults([], workforceProject.revision + 1);
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    await expect.element(await screen.findByRole("alert")).toMatchTextContent(messages.setup.stale);
    expect(screen.queryByRole("heading", { name: messages.setup.calendarSummary })).toBeNull();
  });

  it.each([
    ["mismatch", false],
    ["conflict", false],
    ["mismatch", true],
    ["conflict", true],
  ] as const)(
    "revokes result consumers after native %s (selection recovery=%s)",
    async (failure, recoveringSelection) => {
      configureNative();
      const accepted: SolutionSummaryDtoV1 = {
        schemaVersion: 1,
        result: {
          solutionId: "01900000-0000-7000-8000-000000000020",
          resultChecksum: "a".repeat(64),
        },
        scenarioId: workforceProject.scenarioId,
        scenarioRevision: workforceProject.revision,
        currentRevision: workforceProject.revision,
        runId: "01900000-0000-7000-8000-000000000022",
        stale: false,
        selected: false,
        status: "feasible",
        score: { feasibility: "0", levels: [] },
        verificationChecksum: "b".repeat(64),
        finishedAt: "2030-01-01T12:00:00Z",
      };
      mockWorkforceSetupResults([accepted], workforceProject.revision);
      await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
      const select = await screen.findByRole("button", {
        name: messages.setup.selectResultForViewing(accepted.result.solutionId),
      });
      if (recoveringSelection) {
        vi.mocked(generatedApi.selectSolution).mockRejectedValueOnce(
          Object.assign(new Error("Selection acknowledgement was interrupted."), {
            category: "cancelled",
            code: "operation.cancelled",
          }),
        );
        await userEvent.click(select);
        await screen.findByRole("button", { name: messages.setup.refreshResults });
      }
      if (failure === "mismatch") {
        vi.mocked(generatedApi.listSolutions).mockResolvedValueOnce(
          response({
            schemaVersion: 1,
            scenarioId: workforceProject.scenarioId,
            currentRevision: workforceProject.revision + 1,
            freshlyVerified: false,
            solutions: [accepted],
          }),
        );
      } else {
        vi.mocked(generatedApi.listSolutions).mockRejectedValueOnce(
          Object.assign(new Error("The native revision changed."), {
            category: "conflict",
            code: "revision.conflict",
          }),
        );
      }
      await userEvent.click(
        screen.getByRole("button", {
          name: recoveringSelection
            ? messages.setup.refreshResults
            : messages.setup.comparisonRefresh,
        }),
      );
      await expect
        .element(await screen.findByRole("alert"))
        .toMatchTextContent(messages.setup.stale);
      expect(screen.queryByRole("heading", { name: messages.setup.calendarSummary })).toBeNull();
      expect(
        screen.queryByRole("button", {
          name: messages.setup.selectResultForViewing(accepted.result.solutionId),
        }),
      ).toBeNull();
    },
  );

  it("refreshes an uncertain result selection before retrying and restores focus after native selection", async () => {
    configureNative();
    let notifyLibrary: (() => void) | undefined;
    vi.mocked(generatedApi.onAppNotification).mockImplementation((listener) => {
      notifyLibrary = () => {
        listener({
          type: "appNotification",
          payload: {
            context: {
              eventVersion: 1,
              timestamp: "2030-01-01T12:00:01Z",
              requestId: null,
              scenarioId: workforceProject.scenarioId,
              revision: workforceProject.revision,
              solveRunId: null,
            },
            code: "fixture.refresh",
            message: "Project data refreshed.",
          },
        });
      };
      return Promise.resolve(() => {});
    });
    vi.mocked(generatedApi.getAppCapabilities).mockResolvedValue(
      response({ availableCommands: ["solution_adopt_base"], unavailableCommands: [] }),
    );
    const firstId = "01900000-0000-7000-8000-000000000020";
    const secondId = "01900000-0000-7000-8000-000000000021";
    const result = (solutionId: string, selected: boolean): SolutionSummaryDtoV1 => ({
      schemaVersion: 1,
      result: { solutionId, resultChecksum: "a".repeat(64) },
      scenarioId: workforceProject.scenarioId,
      scenarioRevision: workforceProject.revision,
      currentRevision: workforceProject.revision,
      runId: "01900000-0000-7000-8000-000000000022",
      stale: false,
      selected,
      status: "feasible",
      score: { feasibility: "0", levels: [] },
      verificationChecksum: "b".repeat(64),
      finishedAt: "2030-01-01T12:00:00Z",
    });
    let selectedId = firstId;
    mockWorkforceSetupResults([result(firstId, true), result(secondId, false)]);
    vi.mocked(generatedApi.listSolutions).mockImplementation(() =>
      Promise.resolve(
        response({
          schemaVersion: 1,
          scenarioId: workforceProject.scenarioId,
          currentRevision: workforceProject.revision,
          freshlyVerified: false,
          solutions: [
            result(firstId, selectedId === firstId),
            result(secondId, selectedId === secondId),
          ],
        }),
      ),
    );
    vi.mocked(generatedApi.selectSolution)
      .mockRejectedValueOnce(new Error("Response interrupted after commit"))
      .mockImplementationOnce(() => {
        selectedId = secondId;
        return Promise.resolve(response(result(secondId, true), [], workforceProject.revision));
      });
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    const results = (await screen.findByText(messages.setup.selectionHelp)).closest("details");
    if (results === null) throw new Error("Accepted-result details are missing.");
    const select = await within(results).findByRole("button", {
      name: messages.setup.selectResultForViewing(secondId),
    });
    await userEvent.click(select);
    await expect
      .element(await within(results).findByRole("alert"))
      .toMatchTextContent(messages.setup.selectionRecovery);
    await expect.element(select).toBeDisabled();
    expect(generatedApi.selectSolution).toHaveBeenCalledTimes(1);
    const pendingRefresh = Promise.withResolvers<ApiResponseDto<SolutionListDtoV1>>();
    vi.mocked(generatedApi.listSolutions).mockReturnValueOnce(pendingRefresh.promise);
    const refreshButton = within(results).getByRole("button", {
      name: messages.setup.refreshResults,
    });
    await userEvent.click(refreshButton);
    await expect.element(refreshButton).toBeDisabled();
    await expect.element(select).toBeDisabled();
    expect(generatedApi.selectSolution).toHaveBeenCalledTimes(1);
    pendingRefresh.resolve(
      response({
        schemaVersion: 1,
        scenarioId: workforceProject.scenarioId,
        currentRevision: workforceProject.revision,
        freshlyVerified: false,
        solutions: [result(firstId, true), result(secondId, false)],
      }),
    );
    await expect.poll(() => within(results).queryByRole("alert")).toBeNull();
    await expect.element(select).toBeEnabled();
    const row = select.closest("li");
    if (!row) throw new Error("The accepted-result row is missing.");
    const inspect = within(row).getByRole("button", { name: messages.acceptedSchedule.open });
    await expect
      .element(
        within(row).getByRole("button", {
          name: messages.setup.baseCapture.reviewResult(secondId),
        }),
      )
      .toBeVisible();
    const loadedSummary = await vi.mocked(generatedApi.getScenarioSummary).mock.results[0]?.value
      .result;
    if (loadedSummary === undefined) throw new Error("The initial native summary is missing.");
    const delayedSummary = Promise.withResolvers<typeof loadedSummary>();
    const summaryCalls = vi.mocked(generatedApi.getScenarioSummary).mock.calls.length;
    vi.mocked(generatedApi.getScenarioSummary).mockReturnValueOnce(
      portableOperation(delayedSummary.promise),
    );
    notifyLibrary?.();
    await expect
      .poll(() => vi.mocked(generatedApi.getScenarioSummary).mock.calls.length)
      .toBe(summaryCalls + 1);
    await userEvent.click(select);
    await expect
      .element(await within(results).findByText(messages.setup.selectionSucceeded(secondId)))
      .toBeVisible();
    expect(select.isConnected).toBe(false);
    await expect.element(inspect).toHaveFocus();
    expect(generatedApi.selectSolution).toHaveBeenNthCalledWith(2, {
      scenarioId: workforceProject.scenarioId,
      expectedRevision: workforceProject.revision,
      solutionId: secondId,
    });
    expect(generatedApi.applyScenarioCommand).not.toHaveBeenCalled();
    delayedSummary.resolve(loadedSummary);
    await expect.poll(() => screen.queryByText(messages.setup.loading)).toBeNull();
    expect(
      within(results).queryByRole("button", {
        name: messages.setup.selectResultForViewing(secondId),
      }),
    ).toBeNull();
    await expect
      .element(
        within(results).getByRole("button", {
          name: messages.setup.selectResultForViewing(firstId),
        }),
      )
      .toBeEnabled();
  });

  it("preserves explicit Optimize intent when an initial manual-source read finishes later", async () => {
    configureNative();
    mockWorkforceSetupResults([]);
    const pending = Promise.withResolvers<ApiResponseDto<generatedApi.ManualEditResultV1>>();
    vi.mocked(generatedApi.manualEdit).mockReturnValue(portableOperation(pending.promise));
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    const intent = await screen.findByRole("combobox", { name: "Next solve intent" });
    await userEvent.selectOptions(intent, "optimize");
    const sourceId = "01900000-0000-7000-8000-000000000063";
    pending.resolve(
      response(
        {
          kind: "read",
          read: {
            schemaVersion: 1,
            source: {
              scenarioId: workforceProject.scenarioId,
              revision: workforceProject.revision,
              documentHash: "a".repeat(64),
              original: {
                entity: { kind: "baseSchedule", id: sourceId },
                pairContentHash: "b".repeat(64),
                acceptedResult: { solutionId: sourceId, resultChecksum: "c".repeat(64) },
                sourceScenarioId: workforceProject.scenarioId,
                sourceRevision: workforceProject.revision,
                sourceDocumentHash: "d".repeat(64),
                sourceRunId: sourceId,
                sourceRunInputChecksum: "e".repeat(64),
                sourceRunManifestChecksum: "f".repeat(64),
              },
              currentManualId: sourceId,
              currentManualChecksum: "1".repeat(64),
              selectedCount: 0,
              protectionCount: 0,
              authoredOptimizeUnavailableReason: null,
            },
            offset: 0,
            totalRows: 0,
            nextOffset: null,
            rows: [],
          },
        },
        [],
        workforceProject.revision,
      ),
    );
    await screen.findByRole("heading", { name: "Current admitted manual B" });
    await expect.element(intent).toHaveValue("optimize");
  });

  it("requires a result-specific review and explicit redo consent before saving a repair base", async () => {
    configureNative();
    const solutionId = "01900000-0000-7000-8000-000000000063";
    const accepted: SolutionSummaryDtoV1 = {
      schemaVersion: 1,
      result: { solutionId, resultChecksum: "b".repeat(64) },
      scenarioId: workforceProject.scenarioId,
      scenarioRevision: workforceProject.revision - 1,
      currentRevision: workforceProject.revision,
      runId: "01900000-0000-7000-8000-000000000064",
      stale: true,
      selected: false,
      status: "feasible",
      score: { feasibility: "0", levels: [] },
      verificationChecksum: "c".repeat(64),
      finishedAt: "2030-01-01T12:00:00Z",
    };
    mockWorkforceSetupResults([accepted]);
    vi.mocked(generatedApi.getAppCapabilities).mockResolvedValue(
      response({ availableCommands: ["solution_adopt_base"], unavailableCommands: [] }),
    );
    const redoRejection = Object.assign(new Error("Redo branch exists"), {
      category: "validation",
      code: "history.redo_branch_requires_truncation",
      fieldErrors: [
        {
          field: "/truncateRedo",
          code: "history.redo_branch_requires_truncation",
          message: "Redo branch exists",
        },
      ],
    });
    const pending =
      Promise.withResolvers<Awaited<ReturnType<typeof generatedApi.adoptBaseSolution>>>();
    vi.mocked(generatedApi.adoptBaseSolution)
      .mockRejectedValueOnce(redoRejection)
      .mockReturnValueOnce(pending.promise);
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    const results = (await screen.findByText(messages.setup.selectionHelp)).closest("details");
    if (results === null) throw new Error("Accepted-result details are missing.");
    const review = await within(results).findByRole("button", {
      name: messages.setup.baseCapture.reviewResult(solutionId),
    });
    expect(generatedApi.adoptBaseSolution).not.toHaveBeenCalled();
    await userEvent.click(review);
    const confirm = within(results).getByRole("button", {
      name: messages.setup.baseCapture.confirm,
    });
    await expect
      .element(within(results).getByText(messages.setup.baseCapture.olderSource))
      .toBeVisible();
    expect(generatedApi.adoptBaseSolution).not.toHaveBeenCalled();
    await userEvent.click(confirm);
    await expect
      .element(await within(results).findByText(messages.setup.baseCapture.redoWarning))
      .toBeVisible();
    expect(generatedApi.adoptBaseSolution).toHaveBeenCalledWith({
      commandId: expect.any(String),
      scenarioId: workforceProject.scenarioId,
      expectedRevision: workforceProject.revision,
      result: accepted.result,
      actor: { actorId: null, displayName: messages.people.localActor },
      truncateRedo: false,
    });
    await userEvent.click(
      within(results).getByRole("button", { name: messages.setup.baseCapture.confirmRedo }),
    );
    await expect
      .element(within(results).getByText(messages.setup.baseCapture.saving))
      .toBeVisible();
    await expect
      .element(
        within(results).getByRole("button", { name: messages.setup.baseCapture.confirmRedo }),
      )
      .toHaveAttribute("aria-disabled", "true");
    expect(generatedApi.adoptBaseSolution).toHaveBeenCalledTimes(2);
    expect(vi.mocked(generatedApi.adoptBaseSolution).mock.calls[1]?.[0]).toMatchObject({
      scenarioId: workforceProject.scenarioId,
      expectedRevision: workforceProject.revision,
      result: accepted.result,
      truncateRedo: true,
    });
    expect(vi.mocked(generatedApi.adoptBaseSolution).mock.calls[1]?.[0].commandId).not.toBe(
      vi.mocked(generatedApi.adoptBaseSolution).mock.calls[0]?.[0].commandId,
    );
  });
  it("compares the exact latest pair at the candidate revision and discards a replaced pending pair", async () => {
    configureNative();
    const newest = acceptedSummary(
      "01900000-0000-7000-8000-000000000030",
      "1".repeat(64),
      "01900000-0000-7000-8000-000000000040",
      2,
    );
    const previous = acceptedSummary(
      "01900000-0000-7000-8000-000000000031",
      "2".repeat(64),
      "01900000-0000-7000-8000-000000000041",
      1,
    );
    const older = acceptedSummary(
      "01900000-0000-7000-8000-000000000032",
      "3".repeat(64),
      "01900000-0000-7000-8000-000000000042",
      1,
      true,
    );
    const replacement = acceptedSummary(
      "01900000-0000-7000-8000-000000000033",
      "4".repeat(64),
      "01900000-0000-7000-8000-000000000043",
      3,
    );
    mockWorkforceSetupResults([newest, previous, older]);
    const pending = Promise.withResolvers<ApiResponseDto<SolutionExplanationDtoV2>>();
    vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(portableOperation(pending.promise));
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);

    const compare = await screen.findByRole("button", { name: messages.setup.compareResults });
    compare.focus();
    await userEvent.keyboard("{Enter}");
    await expect.element(compare).toHaveFocus();
    const firstCall = vi.mocked(generatedApi.explainSolution).mock.calls[0];
    expect(firstCall?.[0].context).toMatchObject({
      kind: "scenario",
      scenarioId: workforceProject.scenarioId,
      expectedRevision: newest.scenarioRevision,
    });
    expect(firstCall?.[1]).toEqual({
      schemaVersion: 1,
      subject: { kind: "solutionDifference", left: previous.result, right: newest.result },
    });

    vi.mocked(generatedApi.listSolutions).mockResolvedValueOnce(
      response({
        schemaVersion: 1,
        scenarioId: workforceProject.scenarioId,
        currentRevision: workforceProject.revision,
        freshlyVerified: false,
        solutions: [replacement, newest, previous, older],
      }),
    );
    await userEvent.click(screen.getByRole("button", { name: messages.setup.comparisonRefresh }));
    pending.resolve(differenceResponse(previous, newest));
    await expect
      .poll(() => screen.queryByRole("heading", { name: "Accepted result comparison" }))
      .toBeNull();

    vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(
      portableOperation(differenceResponse(newest, replacement)),
    );
    await userEvent.click(
      await screen.findByRole("button", { name: messages.setup.compareResults }),
    );
    await expect
      .element(await screen.findByRole("heading", { name: "Accepted result comparison" }))
      .toBeVisible();
    await expect
      .element(
        screen.getByText(/Historical source results are not a fresh check of today's scenario/u),
      )
      .toBeVisible();
    const secondCall = vi.mocked(generatedApi.explainSolution).mock.calls[1];
    expect(secondCall?.[0].context.expectedRevision).toBe(replacement.scenarioRevision);
    expect(secondCall?.[1].subject).toEqual({
      kind: "solutionDifference",
      left: newest.result,
      right: replacement.result,
    });
    expect(generatedApi.selectSolution).not.toHaveBeenCalled();
    expect(generatedApi.applyScenarioCommand).not.toHaveBeenCalled();
    expect(generatedApi.adoptBaseSolution).not.toHaveBeenCalled();
  });

  it("keeps comparison unavailable below two results and retries a failed native explanation", async () => {
    configureNative();
    mockWorkforceSetupResults([]);
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    await userEvent.click(await screen.findByText(messages.setup.resultsDetails));
    const comparisonRegion = screen.getByRole("region", { name: "Compare saved results" });
    expect(
      within(comparisonRegion).queryByRole("button", { name: messages.setup.compareResults }),
    ).toBeNull();
    expect(within(comparisonRegion).getByText(messages.setup.comparisonNoPair)).toBeVisible();

    const previous = acceptedSummary(
      "01900000-0000-7000-8000-000000000050",
      "5".repeat(64),
      "01900000-0000-7000-8000-000000000051",
      1,
    );
    const newest = acceptedSummary(
      "01900000-0000-7000-8000-000000000052",
      "6".repeat(64),
      "01900000-0000-7000-8000-000000000053",
      2,
    );
    vi.mocked(generatedApi.listSolutions).mockResolvedValueOnce(
      response({
        schemaVersion: 1 as const,
        scenarioId: workforceProject.scenarioId,
        currentRevision: workforceProject.revision,
        freshlyVerified: false as const,
        solutions: [previous],
      }),
    );
    await userEvent.click(screen.getByRole("button", { name: messages.setup.comparisonRefresh }));
    expect(
      within(comparisonRegion).queryByRole("button", { name: messages.setup.compareResults }),
    ).toBeNull();
    expect(within(comparisonRegion).getByText(messages.setup.comparisonNoPair)).toBeVisible();

    vi.mocked(generatedApi.listSolutions).mockResolvedValueOnce(
      response({
        schemaVersion: 1 as const,
        scenarioId: workforceProject.scenarioId,
        currentRevision: workforceProject.revision,
        freshlyVerified: false as const,
        solutions: [newest, previous],
      }),
    );
    await userEvent.click(screen.getByRole("button", { name: messages.setup.comparisonRefresh }));
    const compare = await screen.findByRole("button", { name: messages.setup.compareResults });
    const failed = Promise.withResolvers<ApiResponseDto<SolutionExplanationDtoV2>>();
    vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(portableOperation(failed.promise));
    await userEvent.click(compare);
    failed.reject(new Error("Comparison read failed"));
    await expect
      .element(await within(comparisonRegion).findByRole("alert"))
      .toHaveTextContent("The local project library could not complete that request.");
    expect(within(comparisonRegion).queryByText("Comparison read failed")).toBeNull();
    await expect.element(compare).toBeEnabled();

    vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(
      portableOperation(differenceResponse(previous, newest)),
    );
    await userEvent.click(compare);
    await expect
      .element(
        await within(comparisonRegion).findByRole("heading", {
          name: "Accepted result comparison",
        }),
      )
      .toBeVisible();
    expect(generatedApi.explainSolution).toHaveBeenCalledTimes(2);
    expect(generatedApi.selectSolution).not.toHaveBeenCalled();
    expect(generatedApi.applyScenarioCommand).not.toHaveBeenCalled();
    expect(generatedApi.adoptBaseSolution).not.toHaveBeenCalled();
  });

  it("rejects comparison evidence for another accepted result, run, or source ordering", async () => {
    configureNative();
    const previous = acceptedSummary(
      "01900000-0000-7000-8000-000000000060",
      "6".repeat(64),
      "01900000-0000-7000-8000-000000000061",
      1,
    );
    const newest = acceptedSummary(
      "01900000-0000-7000-8000-000000000062",
      "7".repeat(64),
      "01900000-0000-7000-8000-000000000063",
      2,
    );
    mockWorkforceSetupResults([newest, previous]);
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    const compare = await screen.findByRole("button", { name: messages.setup.compareResults });
    const comparisonRegion = screen.getByRole("region", { name: "Compare saved results" });
    const correct = comparisonFor(previous, newest);
    const wrongBase = {
      ...correct,
      base: {
        ...correct.base,
        acceptedResult: { ...correct.base.acceptedResult, resultChecksum: "8".repeat(64) },
      },
    };
    const wrongCandidate = {
      ...correct,
      candidate: {
        ...correct.candidate,
        acceptedResult: {
          ...correct.candidate.acceptedResult,
          solutionId: previous.result.solutionId,
        },
      },
    };
    const wrongRun = {
      ...correct,
      runs: {
        ...correct.runs,
        candidate: { ...correct.runs.candidate, runId: previous.runId },
      },
    };
    const malformed = [
      differenceResponse(previous, newest, wrongBase),
      differenceResponse(previous, newest, wrongCandidate),
      differenceResponse(previous, newest, wrongRun),
      differenceResponse(previous, newest, correct, [
        newest.scenarioRevision,
        previous.scenarioRevision,
      ]),
    ];
    for (const result of malformed) {
      vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(portableOperation(result));
      await userEvent.click(compare);
      await expect
        .element(await within(comparisonRegion).findByRole("alert"))
        .toHaveTextContent(messages.setup.comparisonChanged);
      expect(
        within(comparisonRegion).queryByRole("heading", { name: "Accepted result comparison" }),
      ).toBeNull();
    }
    expect(generatedApi.explainSolution).toHaveBeenCalledTimes(malformed.length);
  });

  it("cancels an in-flight comparison through its native operation and restores keyboard focus", async () => {
    configureNative();
    const previous = acceptedSummary(
      "01900000-0000-7000-8000-000000000070",
      "7".repeat(64),
      "01900000-0000-7000-8000-000000000071",
      1,
    );
    const newest = acceptedSummary(
      "01900000-0000-7000-8000-000000000072",
      "8".repeat(64),
      "01900000-0000-7000-8000-000000000073",
      2,
    );
    mockWorkforceSetupResults([newest, previous]);
    const pending = Promise.withResolvers<ApiResponseDto<SolutionExplanationDtoV2>>();
    const cancellationAck =
      Promise.withResolvers<ApiResponseDto<generatedApi.OperationCancelledV1>>();
    let operationCurrent = true;
    const cancelOperation = vi.fn(() => cancellationAck.promise);
    const operation: SetupOperation<SolutionExplanationDtoV2> = {
      requestId: "01900000-0000-7000-8000-000000000074",
      operationId: Promise.resolve("01900000-0000-7000-8000-000000000075"),
      result: pending.promise,
      cancel: cancelOperation,
      release: vi.fn(() => {
        operationCurrent = false;
        return Promise.resolve(
          response({ schemaVersion: 1 as const, acknowledgement: "released" as const }),
        );
      }),
      isCurrent: () => operationCurrent,
    };
    vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(operation);
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);

    const compare = await screen.findByRole("button", { name: messages.setup.compareResults });
    compare.focus();
    await userEvent.keyboard("{Enter}");
    const cancel = await screen.findByRole("button", { name: messages.setup.comparisonCancel });
    await userEvent.click(cancel);
    await expect
      .element(await screen.findByText(messages.setup.comparisonCancelling))
      .toBeVisible();
    expect(cancelOperation).toHaveBeenCalledOnce();
    pending.reject(
      Object.assign(new Error("Cancelled."), { code: "operation.cancelled", category: "protocol" }),
    );
    cancellationAck.resolve(
      response({ schemaVersion: 1 as const, acknowledgement: "cancellationRequested" as const }),
    );
    await expect.element(await screen.findByText(messages.setup.comparisonCancelled)).toBeVisible();
    await expect.element(compare).toHaveFocus();
    expect(cancelOperation).toHaveBeenCalledOnce();
    expect(generatedApi.selectSolution).not.toHaveBeenCalled();
    expect(generatedApi.applyScenarioCommand).not.toHaveBeenCalled();
    expect(generatedApi.adoptBaseSolution).not.toHaveBeenCalled();
  });
  it("reads Repair facts for the exact accepted pair and shows native parameters and bindings", async () => {
    configureNative();
    const candidate = acceptedSummary(
      "01900000-0000-7000-8000-000000000080",
      "8".repeat(64),
      "01900000-0000-7000-8000-000000000081",
      2,
    );
    const base = acceptedSummary(
      "01900000-0000-7000-8000-000000000082",
      "9".repeat(64),
      "01900000-0000-7000-8000-000000000083",
      1,
    );
    const parameterEntries: [string, VerificationValue][] = [];
    for (const [prefix, start] of [
      ["expected", 100],
      ["observed", 200],
    ] as const) {
      for (let index = 0; index < 10; index += 1) {
        parameterEntries.push([
          `${prefix}.fact-${String(index).padStart(2, "0")}`,
          { type: "integer", value: String(start + index) },
        ]);
      }
    }
    parameterEntries.push(
      ["official.workforce.fact.baseline_admission", { type: "text", value: "notEstablished" }],
      [
        "official.workforce.fact.repair_frame",
        { type: "text", value: "recordedBaselineToCandidate" },
      ],
      [
        "official.workforce.fact.summary",
        { type: "text", value: "Native saved-change fact summary." },
      ],
    );
    parameterEntries.push(["untrusted", { type: "text", value: "<img src=x onerror=alert(1)>" }]);
    const fact: EvidenceMessageV1 = {
      messageKey: "unknown.native.fact",
      parameters: Object.fromEntries(parameterEntries),
      entities: [],
      rules: [],
      assignments: Array.from(
        { length: 22 },
        (_, index) => `official.workforce/assignment/shift-${String(index + 1).padStart(3, "0")}`,
      ),
      evidence: [],
    };
    mockWorkforceSetupResults([candidate, base]);
    vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(
      portableOperation(repairResponse(base, candidate, undefined, undefined, [fact])),
    );
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);

    const trigger = await screen.findByRole("button", { name: "Explain saved-change facts" });
    expect(screen.getByRole("button", { name: messages.setup.compareResults })).toBeVisible();
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    const dialog = await screen.findByRole("dialog", { name: "Explanation" });
    await within(dialog).findByText("Native saved-change fact summary.");
    expect(within(dialog).getByText("expected.fact-00")).toBeVisible();
    expect(within(dialog).getByText("100")).toBeVisible();
    expect(within(dialog).getByText("observed.fact-00")).toBeVisible();
    expect(within(dialog).getByText("200")).toBeVisible();
    await userEvent.click(
      within(
        within(dialog).getByRole("navigation", {
          name: "Evidence parameter pages for unknown.native.fact",
        }),
      ).getByRole("button", { name: "Next" }),
    );
    expect(within(dialog).getByText("official.workforce.fact.repair_frame")).toBeVisible();
    expect(within(dialog).getByText("recordedBaselineToCandidate")).toBeVisible();
    expect(within(dialog).getByText("official.workforce.fact.baseline_admission")).toBeVisible();
    expect(within(dialog).getByText("notEstablished")).toBeVisible();
    expect(within(dialog).getByText("official.workforce.fact.summary")).toBeVisible();
    expect(within(dialog).getByText("official.workforce/assignment/shift-001")).toBeVisible();
    expect(within(dialog).getByText("<img src=x onerror=alert(1)>")).toBeVisible();
    expect(dialog.querySelector("img")).toBeNull();
    expect(within(dialog).queryByText("official.workforce/assignment/shift-022")).toBeNull();
    await userEvent.click(
      within(
        within(dialog).getByRole("navigation", {
          name: "Assignment reference pages for unknown.native.fact",
        }),
      ).getByRole("button", { name: "Next" }),
    );
    expect(within(dialog).getByText("official.workforce/assignment/shift-022")).toBeVisible();
    expect(within(dialog).getByText("Repair source bindings")).toBeVisible();
    expect(within(dialog).getByText("[2, 1]")).toBeVisible();
    expect(dialog).toMatchTextContent("Accepted base A");
    expect(dialog).toMatchTextContent(base.result.solutionId);
    expect(dialog).toMatchTextContent(base.runId);
    expect(dialog).toMatchTextContent(candidate.result.solutionId);
    expect(dialog).toMatchTextContent(candidate.runId);
    expect(dialog).toMatchTextContent("A → C");
    expect(dialog).toMatchTextContent("B → C");
    expect(dialog).toMatchTextContent("notEstablished");
    expect(dialog).toMatchTextContent("no unique cause");
    expect(dialog).not.toMatchTextContent("fairness");

    const call = vi.mocked(generatedApi.explainSolution).mock.calls[0];
    expect(call?.[0].context.expectedRevision).toBe(candidate.scenarioRevision);
    expect(call?.[1].subject).toEqual({
      kind: "repair",
      current: candidate.result,
      base: base.result,
    });
    await userEvent.keyboard("{Escape}");
    await expect.poll(() => screen.queryByRole("dialog")).toBeNull();
    await expect.element(trigger).toHaveFocus();
    expect(generatedApi.selectSolution).not.toHaveBeenCalled();
    expect(generatedApi.adoptBaseSolution).not.toHaveBeenCalled();
    expect(generatedApi.applyScenarioCommand).not.toHaveBeenCalled();
  });

  it("rejects reversed or wrong Repair source vectors and accepted run identities", async () => {
    configureNative();
    const candidate = acceptedSummary(
      "01900000-0000-7000-8000-000000000084",
      "a".repeat(64),
      "01900000-0000-7000-8000-000000000085",
      2,
    );
    const base = acceptedSummary(
      "01900000-0000-7000-8000-000000000086",
      "b".repeat(64),
      "01900000-0000-7000-8000-000000000087",
      1,
    );
    const wrongRun = comparisonFor(base, candidate);
    const wrongCandidateRun: SolutionComparisonV2 = {
      ...wrongRun,
      runs: {
        ...wrongRun.runs,
        candidate: { ...wrongRun.runs.candidate, runId: base.runId },
      },
    };
    const wrongAcceptedBase = {
      ...base,
      result: { ...base.result, resultChecksum: "c".repeat(64) },
    };
    const wrongBase = comparisonFor(wrongAcceptedBase, candidate);
    mockWorkforceSetupResults([candidate, base]);
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    const trigger = await screen.findByRole("button", { name: "Explain saved-change facts" });
    const malformed = [
      repairResponse(base, candidate, [base.scenarioRevision, candidate.scenarioRevision]),
      repairResponse(base, candidate, [candidate.scenarioRevision + 1, base.scenarioRevision]),
      repairResponse(
        base,
        candidate,
        [candidate.scenarioRevision, base.scenarioRevision],
        wrongBase,
      ),
      repairResponse(base, candidate, undefined, undefined, [], workforceProject.revision + 1),
      repairResponse(
        base,
        candidate,
        [candidate.scenarioRevision, base.scenarioRevision],
        wrongCandidateRun,
      ),
    ];
    for (const result of malformed) {
      vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(portableOperation(result));
      await userEvent.click(trigger);
      const dialog = await screen.findByRole("dialog", { name: "Explanation" });
      await expect
        .element(await within(dialog).findByRole("alert"))
        .toMatchTextContent(messages.setup.comparisonChanged);
      expect(dialog.contains(document.activeElement)).toBe(true);
      expect(
        within(dialog).queryByRole("heading", { name: "Accepted result comparison" }),
      ).toBeNull();
      await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    }
    expect(generatedApi.explainSolution).toHaveBeenCalledTimes(malformed.length);
  });

  it("does not publish a late Repair response after the accepted pair is refreshed", async () => {
    configureNative();
    const candidate = acceptedSummary(
      "01900000-0000-7000-8000-000000000088",
      "c".repeat(64),
      "01900000-0000-7000-8000-000000000089",
      2,
    );
    const base = acceptedSummary(
      "01900000-0000-7000-8000-000000000090",
      "d".repeat(64),
      "01900000-0000-7000-8000-000000000091",
      1,
    );
    const replacement = acceptedSummary(
      "01900000-0000-7000-8000-000000000092",
      "e".repeat(64),
      "01900000-0000-7000-8000-000000000093",
      3,
    );
    const pending = Promise.withResolvers<ApiResponseDto<SolutionExplanationDtoV2>>();
    mockWorkforceSetupResults([candidate, base]);
    vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(portableOperation(pending.promise));
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    const refreshSavedResults = await screen.findByRole("button", {
      name: messages.setup.comparisonRefresh,
    });
    const trigger = await screen.findByRole("button", { name: "Explain saved-change facts" });
    await userEvent.click(trigger);
    await screen.findByRole("dialog", { name: "Explanation" });
    vi.mocked(generatedApi.listSolutions).mockResolvedValueOnce(
      response({
        schemaVersion: 1,
        scenarioId: workforceProject.scenarioId,
        currentRevision: workforceProject.revision,
        freshlyVerified: false,
        solutions: [replacement, candidate, base],
      }),
    );
    refreshSavedResults.click();
    await expect
      .poll(() => document.body.textContent.includes(replacement.result.solutionId))
      .toBe(true);
    expect(trigger).not.toHaveFocus();
    pending.resolve(
      repairResponse(base, candidate, undefined, undefined, [
        {
          messageKey: "late.fact",
          parameters: {
            "official.workforce.fact.summary": { type: "text", value: "Late pair fact." },
          },
          entities: [],
          rules: [],
          assignments: [],
          evidence: [],
        },
      ]),
    );
    await expect.poll(() => screen.queryByText("Late pair fact.")).toBeNull();
    await expect.poll(() => screen.queryByRole("dialog", { name: "Explanation" })).toBeNull();
  });

  it("does not publish a pending Repair result after project-library ownership changes", async () => {
    const library = configureNative();
    const candidate = acceptedSummary(
      "01900000-0000-7000-8000-000000000106",
      "4".repeat(64),
      "01900000-0000-7000-8000-000000000107",
      2,
    );
    const base = acceptedSummary(
      "01900000-0000-7000-8000-000000000108",
      "5".repeat(64),
      "01900000-0000-7000-8000-000000000109",
      1,
    );
    const pending = Promise.withResolvers<ApiResponseDto<SolutionExplanationDtoV2>>();
    mockWorkforceSetupResults([candidate, base]);
    vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(portableOperation(pending.promise));
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    const trigger = await screen.findByRole("button", { name: "Explain saved-change facts" });
    await userEvent.click(trigger);
    await screen.findByRole("dialog", { name: "Explanation" });
    library[0] = { ...workforceProject, title: "Replaced roster" };
    const notify = vi.mocked(generatedApi.onLibraryRefreshRequired).mock.calls[0]?.[0];
    if (!notify) throw new Error("The application did not register native library notifications.");
    notify({ reason: "event-subscription-lagged" });
    await expect
      .poll(() => vi.mocked(generatedApi.listSolutions).mock.calls.length)
      .toBeGreaterThan(1);
    expect(trigger).not.toHaveFocus();
    pending.resolve(
      repairResponse(base, candidate, undefined, undefined, [
        {
          messageKey: "late.library.fact",
          parameters: {
            "official.workforce.fact.summary": { type: "text", value: "Late library fact." },
          },
          entities: [],
          rules: [],
          assignments: [],
          evidence: [],
        },
      ]),
    );
    await expect.poll(() => screen.queryByText("Late library fact.")).toBeNull();
    await expect.poll(() => screen.queryByRole("dialog", { name: "Explanation" })).toBeNull();
  });
  it("does not publish Repair facts after native window ownership is lost", async () => {
    configureNative();
    const candidate = acceptedSummary(
      "01900000-0000-7000-8000-000000000094",
      "f".repeat(64),
      "01900000-0000-7000-8000-000000000095",
      2,
    );
    const base = acceptedSummary(
      "01900000-0000-7000-8000-000000000096",
      "1".repeat(64),
      "01900000-0000-7000-8000-000000000097",
      1,
    );
    const pending = Promise.withResolvers<ApiResponseDto<SolutionExplanationDtoV2>>();
    let operationCurrent = true;
    const operation: SetupOperation<SolutionExplanationDtoV2> = {
      requestId: "01900000-0000-7000-8000-000000000098",
      operationId: Promise.resolve("01900000-0000-7000-8000-000000000099"),
      result: pending.promise,
      cancel: vi.fn(() =>
        Promise.resolve(
          response({
            schemaVersion: 1 as const,
            acknowledgement: "cancellationRequested" as const,
          }),
        ),
      ),
      release: vi.fn(() => {
        operationCurrent = false;
        return Promise.resolve(
          response({ schemaVersion: 1 as const, acknowledgement: "released" as const }),
        );
      }),
      isCurrent: () => operationCurrent,
    };
    const releaseOperation = vi.spyOn(operation, "release");
    mockWorkforceSetupResults([candidate, base]);
    vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(operation);
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    await userEvent.click(
      await screen.findByRole("button", { name: "Explain saved-change facts" }),
    );
    const dialog = await screen.findByRole("dialog", { name: "Explanation" });
    operationCurrent = false;
    pending.resolve(
      repairResponse(base, candidate, undefined, undefined, [
        {
          messageKey: "late.window.fact",
          parameters: {
            "official.workforce.fact.summary": { type: "text", value: "Late window fact." },
          },
          entities: [],
          rules: [],
          assignments: [],
          evidence: [],
        },
      ]),
    );
    await expect
      .element(await within(dialog).findByRole("alert"))
      .toMatchTextContent(messages.setup.comparisonChanged);
    expect(within(dialog).queryByText("Late window fact.")).toBeNull();
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(releaseOperation).toHaveBeenCalled();
  });

  it("cancels Repair through the owned operation and discards a success that races cancellation", async () => {
    configureNative();
    const candidate = acceptedSummary(
      "01900000-0000-7000-8000-000000000100",
      "2".repeat(64),
      "01900000-0000-7000-8000-000000000101",
      2,
    );
    const base = acceptedSummary(
      "01900000-0000-7000-8000-000000000102",
      "3".repeat(64),
      "01900000-0000-7000-8000-000000000103",
      1,
    );
    const pending = Promise.withResolvers<ApiResponseDto<SolutionExplanationDtoV2>>();
    const cancellationAck =
      Promise.withResolvers<ApiResponseDto<generatedApi.OperationCancelledV1>>();
    let operationCurrent = true;
    const cancelOperation = vi.fn(() => cancellationAck.promise);
    const operation: SetupOperation<SolutionExplanationDtoV2> = {
      requestId: "01900000-0000-7000-8000-000000000104",
      operationId: Promise.resolve("01900000-0000-7000-8000-000000000105"),
      result: pending.promise,
      cancel: cancelOperation,
      release: vi.fn(() => {
        operationCurrent = false;
        return Promise.resolve(
          response({ schemaVersion: 1 as const, acknowledgement: "released" as const }),
        );
      }),
      isCurrent: () => operationCurrent,
    };
    const releaseOperation = vi.spyOn(operation, "release");
    mockWorkforceSetupResults([candidate, base]);
    vi.mocked(generatedApi.explainSolution).mockReturnValueOnce(operation);
    await renderWorkspace(`/project/${workforceProject.scenarioId}/setup`);
    const trigger = await screen.findByRole("button", { name: "Explain saved-change facts" });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    const dialog = await screen.findByRole("dialog", { name: "Explanation" });
    const cancel = await within(dialog).findByRole("button", { name: "Cancel" });
    cancel.focus();
    await userEvent.keyboard("{Enter}");
    expect(cancelOperation).toHaveBeenCalledOnce();
    cancellationAck.resolve(
      response({ schemaVersion: 1 as const, acknowledgement: "cancellationRequested" as const }),
    );
    pending.resolve(
      repairResponse(base, candidate, undefined, undefined, [
        {
          messageKey: "late.cancelled.fact",
          parameters: {
            "official.workforce.fact.summary": { type: "text", value: "Cancelled race fact." },
          },
          entities: [],
          rules: [],
          assignments: [],
          evidence: [],
        },
      ]),
    );
    await expect
      .element(await within(dialog).findByRole("heading", { name: "Explanation cancelled" }))
      .toBeVisible();
    expect(within(dialog).queryByText("Cancelled race fact.")).toBeNull();
    await expect.element(within(dialog).getByRole("button", { name: "Close" })).toHaveFocus();
    expect(releaseOperation).toHaveBeenCalled();
    expect(generatedApi.selectSolution).not.toHaveBeenCalled();
    expect(generatedApi.adoptBaseSolution).not.toHaveBeenCalled();
    await userEvent.keyboard("{Escape}");
    await expect.poll(() => screen.queryByRole("dialog", { name: "Explanation" })).toBeNull();
    await expect.element(trigger).toHaveFocus();
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
    expect(screen.queryByRole("link", { name: messages.shell.export })).toBeNull();
    await userEvent.click(screen.getByText(messages.setup.moreActions));
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

function historyMetadata(
  revision: number,
  summary: string,
  applied = true,
): generatedApi.HistoryPageDtoV1 {
  return {
    schemaVersion: 1,
    scenarioId: workforceProject.scenarioId,
    revision,
    entries: [
      {
        id: "01900000-0000-7000-8000-000000000081",
        revisionBefore: 2,
        revisionAfter: 3,
        source: "desktop",
        summary,
        createdAt: "2026-09-12T12:00:00Z",
        historySequence: 1,
        branchGeneration: 0,
        applied,
      },
    ],
    continuation: null,
    undoAvailable: applied,
    redoAvailable: !applied,
  };
}
function historyMutation(revision: number): ApiResponseDto<generatedApi.CommandResultDto> {
  return response(
    {
      newRevision: revision,
      changeSet: { changes: [] },
      validationDelta: { added: [], resolved: [] },
      inverse: null,
    },
    [],
    revision,
  );
}

describe("Routed history publication fences", () => {
  it("rejects a pending older page during root undo, then exposes only the authoritative undo/redo publication", async () => {
    const library = configureNative();
    const stale = Promise.withResolvers<ApiResponseDto<generatedApi.HistoryPageDtoV1>>();
    const undo = Promise.withResolvers<ApiResponseDto<generatedApi.CommandResultDto>>();
    const first = {
      ...historyMetadata(3, "Current journal"),
      continuation: {
        scenarioId: workforceProject.scenarioId,
        revision: 3,
        beforeSequence: 2,
      },
    };
    vi.mocked(generatedApi.getScenarioHistoryPage)
      .mockResolvedValueOnce(response(first))
      .mockReturnValueOnce(stale.promise)
      .mockResolvedValueOnce(response(historyMetadata(4, "After undo", false)))
      .mockResolvedValueOnce(response(historyMetadata(5, "After redo")));
    vi.mocked(generatedApi.undoScenario).mockReturnValueOnce(undo.promise);
    vi.mocked(generatedApi.redoScenario).mockImplementationOnce(() => {
      library[0] = { ...workforceProject, revision: 5 };
      return Promise.resolve(historyMutation(5));
    });
    await renderWorkspace(`/project/${workforceProject.scenarioId}/history`);
    const history = await screen.findByRole("region", { name: plannerMessage("history.title") });
    await within(history).findByText("Current journal");
    await userEvent.click(
      within(history).getByRole("button", { name: plannerMessage("history.older") }),
    );
    await userEvent.keyboard("{Control>}z{/Control}");
    await expect
      .element(within(history).getByRole("status"))
      .toHaveTextContent(plannerMessage("history.busy"));
    stale.resolve(response(historyMetadata(3, "Stale older journal")));
    await stale.promise;
    await nextTick();
    expect(within(history).queryByText("Stale older journal")).toBeNull();
    expect(within(history).queryByRole("button", { name: messages.shell.redo })).toBeNull();
    library[0] = { ...workforceProject, revision: 4 };
    undo.resolve(historyMutation(4));
    await within(history).findByText("After undo");
    await expect
      .element(within(history).getByRole("status"))
      .toHaveTextContent(plannerMessage("history.revision", { revision: 4 }));
    await expect
      .element(within(history).getByRole("button", { name: messages.shell.undo }))
      .toBeDisabled();
    await userEvent.click(within(history).getByRole("button", { name: messages.shell.redo }));
    await within(history).findByText("After redo");
    await expect
      .element(within(history).getByRole("status"))
      .toHaveTextContent(plannerMessage("history.revision", { revision: 5 }));
    await expect.element(within(history).getByRole("status")).toHaveFocus();
    await expect
      .element(within(history).getByRole("button", { name: messages.shell.redo }))
      .toBeDisabled();
  });

  it("rejects an old journal page after library replacement even when scenario ID and revision are unchanged", async () => {
    const library = configureNative();
    const stale = Promise.withResolvers<ApiResponseDto<generatedApi.HistoryPageDtoV1>>();
    const first = {
      ...historyMetadata(3, "Original journal"),
      continuation: {
        scenarioId: workforceProject.scenarioId,
        revision: 3,
        beforeSequence: 2,
      },
    };
    vi.mocked(generatedApi.getScenarioHistoryPage)
      .mockResolvedValueOnce(response(first))
      .mockReturnValueOnce(stale.promise)
      .mockResolvedValueOnce(response(historyMetadata(3, "Restored journal")));
    await renderWorkspace(`/project/${workforceProject.scenarioId}/history`);
    const history = await screen.findByRole("region", { name: plannerMessage("history.title") });
    await within(history).findByText("Original journal");
    await userEvent.click(
      within(history).getByRole("button", { name: plannerMessage("history.older") }),
    );
    library[0] = { ...workforceProject, title: "Restored roster" };
    const notify = vi.mocked(generatedApi.onLibraryRefreshRequired).mock.calls[0]?.[0];
    if (!notify) throw new Error("The application did not register native library notifications");
    notify({ reason: "event-subscription-lagged" });
    await within(history).findByText("Restored journal");
    stale.resolve(response(historyMetadata(3, "Replaced journal")));
    await stale.promise;
    await nextTick();
    expect(within(history).queryByText("Replaced journal")).toBeNull();
    await expect.element(within(history).getByText("Restored journal")).toBeVisible();
  });

  it("stops repeated conflict refreshes and permits an explicit retry", async () => {
    configureNative();
    const conflict = Object.assign(new Error("Stale history revision"), {
      category: "conflict",
      code: "revision.conflict",
    });
    vi.mocked(generatedApi.getScenarioHistoryPage)
      .mockRejectedValueOnce(conflict)
      .mockRejectedValueOnce(conflict)
      .mockResolvedValueOnce(response(historyMetadata(3, "Fresh journal")));
    await renderWorkspace(`/project/${workforceProject.scenarioId}/history`);
    const history = await screen.findByRole("region", { name: plannerMessage("history.title") });
    const refresh = within(history).getByRole("button", {
      name: plannerMessage("history.refresh"),
    });
    await expect.element(refresh).toBeEnabled();
    await expect
      .element(within(history).getByRole("status"))
      .toHaveTextContent(plannerMessage("history.stale"));
    expect(vi.mocked(generatedApi.getScenarioHistoryPage).mock.calls).toHaveLength(2);
    await userEvent.click(refresh);
    await within(history).findByText("Fresh journal");
  });
});
