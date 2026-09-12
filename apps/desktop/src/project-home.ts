import { onScopeDispose, shallowReactive, watch } from "vue";
import { useQuery, useQueryCache } from "@pinia/colada";
import { useWorkspaceStore } from "./stores/workspace";
import { messages } from "./messages";

import {
  LibraryOperationScope,
  PortableReviewFlow,
  createProject,
  deleteProject,
  duplicateProject,
  listProjects,
  onAppNotification,
  onLibraryRefreshRequired,
  onScenarioChanged,
  onScenarioValidationChanged,
  setProjectArchived,
} from "./api/generated";
import type {
  ApiResponseDto,
  CollisionAction,
  CollisionPlan,
  DomainPackRef,
  ImportOptions,
  OperationPhaseV1,
  OperationProgressV1,
  Revision,
  PortableFilePreviewDto,
  PortablePreviewDto,
  PortableScenarioDto,
  ProjectListItemV1,
  ScenarioChangedEvent,
  SupplementalCollisionAction,
  SupplementalIdentity,
  CalendarSettingsV1,
  ScenarioValidationChangedEvent,
  ValidationIssue,
} from "./api/generated";

export type ProjectPhase = "loading" | "ready" | "error";
export type SupplementalCollisionChoice = SupplementalCollisionAction;
export type CollisionChoice = CollisionAction;
export type ProjectSummary = ProjectListItemV1;
export type PortablePreview = PortablePreviewDto;
export type BackupPreview = PortableFilePreviewDto;

export interface CreateProjectInput {
  readonly title: string;
  readonly description: string;
  readonly domainPack: DomainPackRef;
  readonly settings: CalendarSettingsV1;
}

export interface ProjectHomeApi {
  listProjects(scope: "all"): Promise<ApiResponseDto<readonly ProjectListItemV1[]>>;
  createProject(input: CreateProjectInput): Promise<ApiResponseDto<unknown>>;
  duplicateProject(input: {
    readonly sourceId: string;
    readonly expectedRevision: Revision;
    readonly title: string;
  }): Promise<ApiResponseDto<unknown>>;
  setProjectArchived(input: {
    readonly scenarioId: string;
    readonly expectedRevision: Revision;
    readonly archived: boolean;
  }): Promise<ApiResponseDto<unknown>>;
  deleteProject(scenarioId: string, expectedRevision: Revision): Promise<ApiResponseDto<unknown>>;
  readonly portable: Pick<
    PortableReviewFlow,
    | "previewImport"
    | "applyImport"
    | "previewBackup"
    | "createBackup"
    | "previewRestore"
    | "applyRestore"
    | "discardPreview"
    | "dispose"
  >;
  onAppNotification(listener: () => void): Promise<() => void>;
  onLibraryRefreshRequired(listener: () => void): Promise<() => void>;
  onScenarioChanged(listener: (event: ScenarioChangedEvent) => void): Promise<() => void>;
  onScenarioValidationChanged(
    listener: (event: ScenarioValidationChangedEvent) => void,
  ): Promise<() => void>;
}

function generatedProjectHomeApi(): ProjectHomeApi {
  return {
    listProjects,
    createProject,
    duplicateProject,
    setProjectArchived,
    deleteProject,
    portable: new PortableReviewFlow(),
    onAppNotification,
    onLibraryRefreshRequired,
    onScenarioChanged,
    onScenarioValidationChanged,
  };
}

export interface WorkspaceOperationState {
  readonly label: string;
  readonly cancel: (() => Promise<unknown>) | null;
  phase: OperationPhaseV1 | null;
  cancellationRequested: boolean;
  settled: boolean;
}

export interface WorkspaceOperation<T> {
  readonly action: string;
  readonly label: string;
  readonly execute: (report: (event: OperationProgressV1) => void) => Promise<ApiResponseDto<T>>;
  readonly success: (result: T) => string;
  readonly cancel?: () => Promise<unknown>;
  readonly refreshLibrary?: boolean;
}

type ReviewOwner = Pick<PortableReviewFlow, "dispose">;

export interface ProjectHomeState {
  phase: ProjectPhase;
  readonly projects: readonly ProjectSummary[];
  selectedId: string | null;
  busyAction: string | null;
  operation: WorkspaceOperationState | null;
  libraryEpoch: number;
  reviewCleanupError: string | null;
  retryingReviewCleanup: boolean;
  errorMessage: string | null;
  announcement: string;
  importPreview: PortablePreview | null;
  importWarnings: readonly ValidationIssue[];
  backupPreview: BackupPreview | null;
  restorePreview: PortablePreview | null;
  restoreWarnings: readonly ValidationIssue[];
  restoreSafetyBackupFailure: string | null;
  restoreMode: "add-backup" | "replace-library";
}

export interface ProjectHomeController {
  readonly state: ProjectHomeState;
  load(): Promise<void>;
  startEventListeners(): Promise<void>;
  dispose(): Promise<void>;
  runOperation<T>(operation: WorkspaceOperation<T>): Promise<ApiResponseDto<T>>;
  cancelOperation(): Promise<void>;
  registerReviewOwner(owner: ReviewOwner): void;
  retireReviewOwner(owner: ReviewOwner): Promise<boolean>;
  retryReviewCleanup(): Promise<void>;
  selectProject(scenarioId: string): void;
  createProject(input: CreateProjectInput): Promise<boolean>;
  duplicateProject(project: ProjectSummary, title: string): Promise<boolean>;
  setArchived(project: ProjectSummary): Promise<boolean>;
  deleteProject(project: ProjectSummary): Promise<boolean>;
  previewImport(options: Pick<ImportOptions, "includeResults" | "includeAssets">): Promise<boolean>;
  applyImport(
    collisions: Readonly<Record<string, CollisionChoice>>,
    supplemental: Readonly<Record<string, SupplementalCollisionChoice>>,
  ): Promise<boolean>;
  previewBackup(title: string): Promise<boolean>;
  createBackup(): Promise<boolean>;
  previewRestore(mode: Exclude<ImportOptions["restoreMode"], "import-scenario">): Promise<boolean>;
  applyRestore(
    collisions: Readonly<Record<string, CollisionChoice>>,
    supplemental: Readonly<Record<string, SupplementalCollisionChoice>>,
    safetyBackupBypassPhrase?: string,
  ): Promise<boolean>;
}

export interface FocusTarget {
  focus(): void;
}

export function recoverFocus(target: FocusTarget | null | undefined): void {
  target?.focus();
}

interface ApiFailure {
  readonly category?: unknown;
  readonly code?: unknown;
  readonly message?: unknown;
  readonly retryable?: unknown;
  readonly details?: unknown;
}

export function safeMessage(error: unknown): string {
  const failure = error as ApiFailure | null;
  if (
    typeof failure === "object" &&
    failure !== null &&
    typeof failure.category === "string" &&
    typeof failure.code === "string" &&
    typeof failure.message === "string"
  ) {
    return failure.message;
  }

  return "The local project library could not complete that request.";
}

export function isRevisionConflict(error: unknown): boolean {
  const failure = error as ApiFailure;
  return (
    failure.category === "conflict" ||
    (typeof failure.code === "string" && failure.code.includes("revision"))
  );
}

export function isOperationCancelled(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    (error as ApiFailure).code === "operation.cancelled"
  );
}

export function isRetainedRestoreFailure(error: unknown): boolean {
  if (typeof error !== "object" || error === null) return false;
  const failure = error as ApiFailure;
  if (
    failure.code !== "restore.safety_backup_failed" ||
    typeof failure.details !== "object" ||
    failure.details === null ||
    !("portablePreviewRetained" in failure.details)
  )
    return false;
  const retained = failure.details.portablePreviewRetained;
  return (
    typeof retained === "object" &&
    retained !== null &&
    "type" in retained &&
    retained.type === "boolean" &&
    "value" in retained &&
    retained.value === true
  );
}

export function supplementalIdentityKey(identity: SupplementalIdentity): string {
  return `${identity.section}\u0000${identity.key}`;
}

export function defaultSupplementalCollisionChoices(
  identities: readonly SupplementalIdentity[],
  action: SupplementalCollisionChoice = "skip",
): Record<string, SupplementalCollisionChoice> {
  const choices: Record<string, SupplementalCollisionChoice> = {};
  for (const identity of identities) choices[supplementalIdentityKey(identity)] = action;
  return choices;
}
export interface ScenarioRevisionOutcome {
  readonly revision: Revision | null;
  readonly warning: string | null;
}

export function scenarioRevisionOutcome(
  scenario: PortableScenarioDto,
  action: CollisionChoice | undefined,
  replaceLibrary = false,
): ScenarioRevisionOutcome {
  if (scenario.collides && !replaceLibrary && action === "skip") {
    return { revision: null, warning: null };
  }
  if (!scenario.collides || replaceLibrary || action === "replace") {
    return {
      revision: scenario.sameIdentityRevision,
      warning: scenario.sameIdentityRevisionWarning,
    };
  }
  return { revision: scenario.sourceRevision, warning: null };
}

function collisionPlan(
  preview: PortablePreviewDto,
  scenarios: Readonly<Record<string, CollisionChoice>>,
  supplemental: Readonly<Record<string, SupplementalCollisionChoice>>,
  replaceLibrary = false,
): CollisionPlan | null {
  if (replaceLibrary) return { scenarios: {}, supplementalChoices: [] };
  const scenarioChoices: Record<string, CollisionChoice> = {};
  for (const scenario of preview.scenarios) {
    if (!scenario.collides) continue;
    const action = scenarios[scenario.scenarioId];
    if (!action) return null;
    scenarioChoices[scenario.scenarioId] = action;
  }
  const supplementalChoices = preview.supplementalCollisions.map((identity) => {
    const action = supplemental[supplementalIdentityKey(identity)];
    return action ? { ...identity, action } : null;
  });
  if (supplementalChoices.some((choice) => choice === null)) return null;
  return {
    scenarios: scenarioChoices,
    supplementalChoices: supplementalChoices.filter(
      (choice): choice is NonNullable<typeof choice> => choice !== null,
    ),
  };
}

export function createProjectHomeController(
  api: ProjectHomeApi = generatedProjectHomeApi(),
): ProjectHomeController {
  const workspace = useWorkspaceStore();
  const queryCache = useQueryCache();
  const projectKey = ["projects", "all"] as const;
  const emptyProjects: readonly ProjectSummary[] = [];
  const projects = useQuery({
    key: projectKey,
    query: async () => (await api.listProjects("all")).result,
    enabled: false,
    refetchOnMount: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const state = shallowReactive<ProjectHomeState>({
    phase: "loading",
    get projects() {
      return projects.data.value ?? emptyProjects;
    },
    get selectedId() {
      return workspace.selectedProjectId;
    },
    set selectedId(value: string | null) {
      workspace.selectedProjectId = value;
    },
    busyAction: null,
    operation: null,
    libraryEpoch: 0,
    reviewCleanupError: null,
    retryingReviewCleanup: false,
    errorMessage: null,
    announcement: "",
    importPreview: null,
    importWarnings: [],
    backupPreview: null,
    restorePreview: null,
    restoreWarnings: [],
    restoreSafetyBackupFailure: null,
    restoreMode: "add-backup",
  });
  const eventUnlisteners: Array<() => void> = [];
  const reviewOwners = new Set<ReviewOwner>([api.portable]);
  const failedReviewCleanup = new Set<ReviewOwner>();
  let listenersStarted = false;
  let disposed = false;
  let disposal: Promise<void> | undefined;

  // Native awaits can outlive the controller; read the current lifetime each time.
  const isDisposed = (): boolean => disposed;

  // Colada only publishes the current fetch, even when native IPC ignores abort.
  // Observe that publication, never the settlement of an older refetch promise.
  watch(
    projects.state,
    (result) => {
      if (isDisposed()) return;
      if (result.status === "success") {
        if (!result.data.some(({ scenarioId }) => scenarioId === state.selectedId)) {
          state.selectedId = result.data[0]?.scenarioId ?? null;
        }
        state.phase = "ready";
        state.libraryEpoch += 1;
        state.errorMessage = null;
      } else if (result.status === "error") {
        state.phase = "error";
        state.errorMessage = `${safeMessage(result.error)} Try again to refresh the saved project library.`;
      }
    },
    { flush: "sync" },
  );

  async function reload(showLoading = true): Promise<boolean> {
    if (isDisposed()) return false;
    if (showLoading) state.announcement = "";
    if (showLoading) state.phase = "loading";
    state.errorMessage = null;
    const result = await projects.refetch();
    return !isDisposed() && result.status === "success";
  }

  function refreshFromEvent(event: ScenarioChangedEvent | ScenarioValidationChangedEvent): void {
    if (event.payload.context.scenarioId !== null) {
      void reload(false);
    }
  }

  function releaseListener(unlisten: () => void): void {
    try {
      unlisten();
    } catch {
      // One failed release must not strand the other acquired listeners.
    }
  }

  async function startEventListeners(): Promise<void> {
    if (isDisposed() || listenersStarted) return;
    listenersStarted = true;
    let registrationFailed = false;
    await Promise.all(
      [
        () => api.onScenarioChanged(refreshFromEvent),
        () => api.onScenarioValidationChanged(refreshFromEvent),
        () =>
          api.onAppNotification(() => {
            void reload(false);
          }),
        () =>
          api.onLibraryRefreshRequired(() => {
            void reload(false);
          }),
      ].map(async (register) => {
        try {
          const unlisten = await register();
          if (isDisposed() || registrationFailed) releaseListener(unlisten);
          else eventUnlisteners.push(unlisten);
        } catch (error) {
          registrationFailed = true;
          for (const unlisten of eventUnlisteners.splice(0)) releaseListener(unlisten);
          if (!isDisposed()) state.errorMessage = safeMessage(error);
        }
      }),
    );
    listenersStarted = eventUnlisteners.length > 0;
  }

  async function discardPreview(previewId: string): Promise<void> {
    try {
      await api.portable.discardPreview(previewId);
    } catch {
      // A consumed or evicted preview is already unavailable.
    }
  }

  function dispose(): Promise<void> {
    if (disposal) return disposal;
    disposed = true;
    disposal = Promise.all([...reviewOwners].map((owner) => owner.dispose())).then(() => undefined);
    // cancel() also detaches pending writes; scope untracking alone only aborts.
    const entry = queryCache.get(projectKey);
    if (entry) queryCache.cancel(entry);
    for (const unlisten of eventUnlisteners.splice(0)) releaseListener(unlisten);
    state.operation = null;
    state.importPreview = null;
    state.importWarnings = [];
    state.backupPreview = null;
    state.restorePreview = null;
    state.restoreWarnings = [];
    state.restoreSafetyBackupFailure = null;
    return disposal;
  }

  onScopeDispose(() => {
    void dispose().catch(() => {
      // An unmounted owner cannot display cleanup errors; native teardown also owns cleanup.
    });
  });

  function registerReviewOwner(owner: ReviewOwner): void {
    if (isDisposed()) throw new Error(messages.operations.closed);
    reviewOwners.add(owner);
  }

  async function retireReviewOwner(owner: ReviewOwner): Promise<boolean> {
    try {
      await owner.dispose();
      reviewOwners.delete(owner);
      failedReviewCleanup.delete(owner);
      if (!isDisposed() && failedReviewCleanup.size === 0) state.reviewCleanupError = null;
      return true;
    } catch {
      // Keep the concrete owner reachable across route unmount so cleanup can be retried.
      failedReviewCleanup.add(owner);
      if (!isDisposed()) state.reviewCleanupError = messages.operations.cleanupFailed;
      return false;
    }
  }

  async function retryReviewCleanup(): Promise<void> {
    if (isDisposed() || state.retryingReviewCleanup) return;
    state.retryingReviewCleanup = true;
    try {
      await Promise.all([...failedReviewCleanup].map(retireReviewOwner));
    } finally {
      if (!isDisposed()) state.retryingReviewCleanup = false;
    }
  }

  async function cancelOperation(): Promise<void> {
    const active = state.operation;
    if (!active?.cancel || active.settled || active.cancellationRequested) return;
    active.cancellationRequested = true;
    try {
      await active.cancel();
    } catch (error) {
      const current = state.operation;
      if (!isDisposed() && current === active && !current.settled) {
        current.cancellationRequested = false;
        state.errorMessage = safeMessage(error);
      }
    }
  }

  async function runOperation<T>(operation: WorkspaceOperation<T>): Promise<ApiResponseDto<T>> {
    if (isDisposed()) throw new Error(messages.operations.closed);
    if (state.busyAction) throw new Error(messages.operations.busy);
    const active = shallowReactive<WorkspaceOperationState>({
      label: operation.label,
      cancel: operation.cancel ?? null,
      phase: null,
      cancellationRequested: false,
      settled: false,
    });
    state.busyAction = operation.action;
    state.operation = active;
    state.errorMessage = null;
    try {
      const response = await operation.execute((event) => {
        if (!isDisposed() && state.operation === active && !active.settled)
          active.phase = event.phase;
      });
      active.settled = true;
      if (!isDisposed()) {
        state.announcement = operation.success(response.result);
        if (operation.refreshLibrary !== false) await reload(false);
      }
      return response;
    } catch (error) {
      active.settled = true;
      if (!isDisposed()) {
        if (isRevisionConflict(error)) {
          const reloaded = await reload(false);
          if (!isDisposed())
            state.announcement = reloaded
              ? messages.operations.conflictReloaded
              : messages.operations.conflictRefreshFailed;
        } else if (isOperationCancelled(error)) {
          state.announcement = messages.operations.cancelled;
        } else {
          state.errorMessage = safeMessage(error);
        }
      }
      throw error;
    } finally {
      if (!isDisposed() && state.operation === active) {
        state.operation = null;
        state.busyAction = null;
      }
    }
  }

  async function mutate(
    action: string,
    operation: () => Promise<ApiResponseDto<unknown>>,
    successAnnouncement: string,
  ): Promise<boolean> {
    if (isDisposed() || state.busyAction) return false;
    try {
      await runOperation({
        action,
        label: messages.operations.pending,
        execute: operation,
        success: () => successAnnouncement,
      });
      return true;
    } catch {
      return false;
    }
  }

  return {
    state,
    startEventListeners,
    dispose,
    runOperation,
    cancelOperation,
    registerReviewOwner,
    retireReviewOwner,
    retryReviewCleanup,
    load: async () => {
      await reload();
    },
    selectProject(scenarioId) {
      if (!isDisposed() && !state.busyAction) state.selectedId = scenarioId;
    },
    createProject(input) {
      return mutate("create", () => api.createProject(input), `Created ${input.title}.`);
    },
    duplicateProject(project, title) {
      return mutate(
        `duplicate:${project.scenarioId}`,
        () =>
          api.duplicateProject({
            sourceId: project.scenarioId,
            expectedRevision: project.revision,
            title,
          }),
        `Duplicated ${project.title} as ${title}.`,
      );
    },
    setArchived(project) {
      const archived = !project.archived;
      return mutate(
        `archive:${project.scenarioId}`,
        () =>
          api.setProjectArchived({
            scenarioId: project.scenarioId,
            expectedRevision: project.revision,
            archived,
          }),
        `${archived ? "Archived" : "Unarchived"} ${project.title}.`,
      );
    },
    deleteProject(project) {
      return mutate(
        `delete:${project.scenarioId}`,
        () => api.deleteProject(project.scenarioId, project.revision),
        `Deleted ${project.title}.`,
      );
    },
    async previewImport(selection) {
      if (isDisposed() || state.busyAction) return false;
      const previousError = state.errorMessage;
      state.busyAction = "preview-import";
      state.errorMessage = null;
      try {
        const previousPreview = state.importPreview;
        const response = await api.portable.previewImport(new LibraryOperationScope(null), {
          restoreMode: "import-scenario",
          includeResults: selection.includeResults,
          includeAssets: selection.includeAssets,
        }).result;
        if (isDisposed()) {
          await discardPreview(response.result.previewId);
          return false;
        }
        state.importPreview = response.result;
        state.importWarnings = response.warnings;
        if (previousPreview && previousPreview.previewId !== response.result.previewId) {
          await discardPreview(previousPreview.previewId);
        }
        if (isDisposed()) return false;
        state.announcement = "Import preview ready. Review every collision before applying it.";
        return true;
      } catch (error) {
        if (isDisposed()) return false;
        state.errorMessage = isOperationCancelled(error) ? previousError : safeMessage(error);
        return false;
      } finally {
        if (!isDisposed()) state.busyAction = null;
      }
    },
    applyImport(collisions, supplemental) {
      if (isDisposed() || state.busyAction) return Promise.resolve(false);
      const preview = state.importPreview;
      if (!preview) {
        return Promise.resolve(false);
      }
      const plan = collisionPlan(preview, collisions, supplemental);
      if (!plan) {
        state.errorMessage = "Choose an action for every collision shown in the preview.";
        return Promise.resolve(false);
      }
      return mutate(
        "apply-import",
        () =>
          api.portable.applyImport(new LibraryOperationScope(preview.libraryRevision), {
            previewId: preview.previewId,
            collisionPlan: plan,
          }).result,
        "Import applied.",
      ).then((applied) => {
        if (!isDisposed()) {
          state.importPreview = null;
          state.importWarnings = [];
        }
        return applied;
      });
    },
    async previewBackup(title) {
      if (isDisposed() || state.busyAction) return false;
      state.busyAction = "preview-backup";
      state.errorMessage = null;
      try {
        const previousPreview = state.backupPreview;
        const response = await api.portable.previewBackup(new LibraryOperationScope(null), title)
          .result;
        if (isDisposed()) {
          await discardPreview(response.result.previewId);
          return false;
        }
        state.backupPreview = response.result;
        if (previousPreview && previousPreview.previewId !== response.result.previewId) {
          await discardPreview(previousPreview.previewId);
        }
        if (isDisposed()) return false;
        state.announcement = "Backup preview ready.";
        return true;
      } catch (error) {
        if (isDisposed()) return false;
        state.errorMessage = safeMessage(error);
        return false;
      } finally {
        if (!isDisposed()) state.busyAction = null;
      }
    },
    async createBackup() {
      if (isDisposed() || state.busyAction) return false;
      const preview = state.backupPreview;
      if (!preview) return false;
      const previousError = state.errorMessage;
      state.busyAction = "create-backup";
      state.errorMessage = null;
      try {
        const response = await api.portable.createBackup(
          new LibraryOperationScope(preview.libraryRevision),
          preview.previewId,
        ).result;
        if (isDisposed()) return true;
        await reload(false);
        if (!isDisposed()) state.announcement = `Backup saved as ${response.result.artifactName}.`;
        return true;
      } catch (error) {
        if (isDisposed()) return false;
        if (isOperationCancelled(error)) {
          state.errorMessage = previousError;
        } else if (isRevisionConflict(error)) {
          const reloaded = await reload(false);
          if (!isDisposed())
            state.announcement = reloaded
              ? "The project changed in another window. The latest saved version has been reloaded."
              : "The project changed in another window. Refresh the library before trying the change again.";
        } else {
          state.errorMessage = safeMessage(error);
        }
        return false;
      } finally {
        if (!isDisposed()) {
          state.backupPreview = null;
          state.busyAction = null;
        }
      }
    },
    async previewRestore(mode) {
      if (isDisposed() || state.busyAction) return false;
      const previousError = state.errorMessage;
      state.busyAction = "preview-restore";
      state.errorMessage = null;
      try {
        const previousPreview = state.restorePreview;
        const response = await api.portable.previewRestore(
          new LibraryOperationScope(null),
          {
            restoreMode: mode,
            includeResults: true,
            includeAssets: true,
          },
          "userSelected",
        ).result;
        if (isDisposed()) {
          await discardPreview(response.result.previewId);
          return false;
        }
        state.restoreSafetyBackupFailure = null;
        state.restorePreview = response.result;
        state.restoreWarnings = response.warnings;
        if (previousPreview && previousPreview.previewId !== response.result.previewId) {
          await discardPreview(previousPreview.previewId);
        }
        if (isDisposed()) return false;
        state.restoreMode = mode;
        state.announcement = `${mode === "replace-library" ? "Replace" : "Add"} restore preview ready.`;
        return true;
      } catch (error) {
        if (isDisposed()) return false;
        state.errorMessage = isOperationCancelled(error) ? previousError : safeMessage(error);
        return false;
      } finally {
        if (!isDisposed()) state.busyAction = null;
      }
    },
    async applyRestore(collisions, supplemental, safetyBackupBypassPhrase = "") {
      if (isDisposed() || state.busyAction) return false;
      const preview = state.restorePreview;
      if (!preview) return false;
      const plan = collisionPlan(
        preview,
        collisions,
        supplemental,
        state.restoreMode === "replace-library",
      );
      if (!plan) {
        state.errorMessage = "Choose an action for every collision shown in the preview.";
        return false;
      }
      state.busyAction = "apply-restore";
      state.errorMessage = null;
      try {
        const response = await api.portable.applyRestore(
          new LibraryOperationScope(preview.libraryRevision),
          {
            previewId: preview.previewId,
            collisionPlan: plan,
            authorization: {
              destructiveActionConfirmed: state.restoreMode === "replace-library",
              safetyBackupBypassPhrase: safetyBackupBypassPhrase || null,
            },
          },
        ).result;
        if (isDisposed()) return true;
        state.restorePreview = null;
        state.restoreWarnings = [];
        state.restoreSafetyBackupFailure = null;
        await reload(false);
        if (!isDisposed()) {
          const outcome = response.result.safetyBackup;
          state.announcement =
            outcome.kind === "createdAndVerified"
              ? `Restore applied. Safety backup saved and verified as ${outcome.artifactName}.`
              : outcome.kind === "confirmedBypass"
                ? "Restore applied without a safety backup after explicit confirmation."
                : "Restore applied.";
        }
        return true;
      } catch (error) {
        if (isDisposed()) return false;
        if (isRetainedRestoreFailure(error)) {
          state.restoreSafetyBackupFailure = safeMessage(error);
          state.errorMessage = null;
          state.announcement =
            "The safety backup failed. Review the reason before choosing whether to continue.";
        } else {
          state.restorePreview = null;
          state.restoreWarnings = [];
          state.restoreSafetyBackupFailure = null;
          if (isRevisionConflict(error)) {
            const reloaded = await reload(false);
            if (!isDisposed())
              state.announcement = reloaded
                ? "The project changed in another window. The latest saved version has been reloaded."
                : "The project changed in another window. Refresh the library before trying the change again.";
          } else {
            state.errorMessage = safeMessage(error);
          }
        }
        return false;
      } finally {
        if (!isDisposed()) state.busyAction = null;
      }
    },
  };
}
