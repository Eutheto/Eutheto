import { onScopeDispose, shallowReactive, watch } from "vue";
import { useQuery, useQueryCache } from "@pinia/colada";
import { useWorkspaceStore } from "./stores/workspace";
import { messages } from "./messages";

import {
  createProject,
  deleteProject,
  duplicateProject,
  listProjects,
  openProject,
  onAppNotification,
  onLibraryRefreshRequired,
  onScenarioChanged,
  onScenarioValidationChanged,
  setProjectArchived,
} from "./api/generated";
import type {
  ApiResponseDto,
  PortableReviewFlow,
  CollisionAction,
  DomainPackRef,
  OperationPhaseV1,
  OperationProgressV1,
  Revision,
  PortableScenarioDto,
  ProjectListItemV1,
  ProjectMetadataDto,
  ScenarioChangedEvent,
  SupplementalIdentity,
  CalendarSettingsV1,
  ScenarioValidationChangedEvent,
} from "./api/generated";

export type ProjectPhase = "loading" | "ready" | "error";
export type ProjectSummary = ProjectListItemV1;

export interface CreateProjectInput {
  readonly title: string;
  readonly description: string;
  readonly domainPack: DomainPackRef;
  readonly settings: CalendarSettingsV1;
}

export interface ProjectHomeApi {
  listProjects(scope: "all"): Promise<ApiResponseDto<readonly ProjectListItemV1[]>>;
  openProject(scenarioId: string): Promise<ApiResponseDto<ProjectListItemV1>>;
  createProject(input: CreateProjectInput): Promise<ApiResponseDto<ProjectMetadataDto>>;
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
    openProject,
    createProject,
    duplicateProject,
    setProjectArchived,
    deleteProject,
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
}

export interface ProjectHomeController {
  readonly state: ProjectHomeState;
  load(): Promise<void>;
  refreshLibrary(): Promise<boolean>;
  startEventListeners(): Promise<void>;
  dispose(): Promise<void>;
  runOperation<T>(operation: WorkspaceOperation<T>): Promise<ApiResponseDto<T>>;
  cancelOperation(): Promise<void>;
  registerReviewOwner(owner: ReviewOwner): void;
  retireReviewOwner(owner: ReviewOwner): Promise<boolean>;
  retryReviewCleanup(): Promise<void>;
  selectProject(scenarioId: string | null): void;
  openProject(scenarioId: string): Promise<ProjectSummary | null>;
  createProject(input: CreateProjectInput): Promise<ProjectMetadataDto | null>;
  duplicateProject(project: ProjectSummary, title: string): Promise<boolean>;
  setArchived(project: ProjectSummary): Promise<boolean>;
  deleteProject(project: ProjectSummary): Promise<boolean>;
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
  if (typeof error !== "object" || error === null) return false;
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

export interface ScenarioRevisionOutcome {
  readonly revision: Revision | null;
  readonly warning: string | null;
}

export function scenarioRevisionOutcome(
  scenario: PortableScenarioDto,
  action: CollisionAction | undefined,
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
  });
  const eventUnlisteners: Array<() => void> = [];
  const reviewOwners = new Set<ReviewOwner>();
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
          state.selectedId = null;
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

  function dispose(): Promise<void> {
    if (disposal) return disposal;
    disposed = true;
    disposal = Promise.all([...reviewOwners].map((owner) => owner.dispose())).then(() => undefined);
    // cancel() also detaches pending writes; scope untracking alone only aborts.
    const entry = queryCache.get(projectKey);
    if (entry) queryCache.cancel(entry);
    for (const unlisten of eventUnlisteners.splice(0)) releaseListener(unlisten);
    state.operation = null;
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
    refreshLibrary: () => reload(false),
    selectProject(scenarioId) {
      if (!isDisposed() && !state.busyAction) state.selectedId = scenarioId;
    },
    async openProject(scenarioId) {
      try {
        const response = await runOperation({
          action: `open:${scenarioId}`,
          label: messages.projects.opening,
          execute: () => api.openProject(scenarioId),
          success: (project) => messages.projects.opened(project.title),
        });
        return response.result;
      } catch {
        return null;
      }
    },
    async createProject(input) {
      try {
        const response = await runOperation({
          action: "create",
          label: messages.projects.creating,
          execute: () => api.createProject(input),
          success: (project) => messages.projects.created(project.title),
        });
        return response.result;
      } catch {
        return null;
      }
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
  };
}
