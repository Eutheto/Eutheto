import { shallowReactive } from "vue";
import {
  LibraryOperationScope,
  PortableReviewFlow,
  SetupOperationScope,
  type ApiResponseDto,
  type CollisionAction,
  type CollisionPlan,
  type OperationProgressV1,
  type PortableAppliedDto,
  type PortableFilePreviewDto,
  type PortablePreviewDto,
  type Revision,
  type SetupOperation,
  type SupplementalCollisionAction,
  type UnopenedBundlePreviewDto,
  type ValidationIssue,
} from "./api/generated";
import { messages } from "./messages";
import {
  isOperationCancelled,
  isRetainedRestoreFailure,
  safeMessage,
  supplementalIdentityKey,
  type ProjectHomeController,
  type ProjectSummary,
} from "./project-home";

export type PortableMode = "import" | "export" | "backup-restore";
export interface PortableContext {
  readonly mode: PortableMode;
  readonly project?: ProjectSummary | undefined;
  readonly libraryRevision?: Revision | null | undefined;
}
export type PortableReview =
  | { readonly kind: "import"; readonly preview: PortablePreviewDto }
  | {
      readonly kind: "restore";
      readonly preview: PortablePreviewDto;
      readonly mode: "add-backup" | "replace-library";
      readonly origin: "userSelected" | "safetyBackups";
    }
  | {
      readonly kind: "export";
      readonly preview: PortableFilePreviewDto;
      readonly scenarioId: string;
      readonly scenarioRevision: Revision;
    }
  | { readonly kind: "backup"; readonly preview: PortableFilePreviewDto }
  | { readonly kind: "unopened"; readonly preview: UnopenedBundlePreviewDto };

export interface PortableWorkspaceState {
  review: PortableReview | null;
  warnings: readonly ValidationIssue[];
  pending: boolean;
  stale: boolean;
  error: string | null;
  notice: string | null;
  safetyFailure: string | null;
}
export interface PortableExportEvents {
  exported(outcome: { artifactName: string }): void;
  exportCancelled(): void;
  exportFailed(): void;
}

type PortableFlow = Pick<
  PortableReviewFlow,
  | "previewImport"
  | "applyImport"
  | "previewExport"
  | "createExport"
  | "previewBackup"
  | "createBackup"
  | "previewRestore"
  | "applyRestore"
  | "inspectUnopenedBundle"
  | "reexportUnopenedBundle"
  | "dispose"
>;

export interface PortableWorkspaceController {
  readonly state: PortableWorkspaceState;
  discardReview(): void;
  leave(): void;
  enter(): void;
  refreshBinding(): void;
  previewImport(includeResults: boolean, includeAssets: boolean): Promise<boolean>;
  previewExport(): Promise<boolean>;
  previewBackup(title: string): Promise<boolean>;
  previewRestore(
    mode: "add-backup" | "replace-library",
    origin: "userSelected" | "safetyBackups",
  ): Promise<boolean>;
  inspect(): Promise<boolean>;
  save(): Promise<boolean>;
  apply(
    scenarios: Readonly<Record<string, CollisionAction | "">>,
    supplemental: Readonly<Record<string, SupplementalCollisionAction | "">>,
    confirmation: "reviewed" | "replace" | "bypass",
    bypassPhrase?: string,
  ): Promise<boolean>;
}

/** Only explicitly reviewed choices become a native plan; unrelated draft keys confer no authority. */
export function reviewedCollisionPlan(
  preview: PortablePreviewDto,
  scenarios: Readonly<Record<string, CollisionAction | "">>,
  supplemental: Readonly<Record<string, SupplementalCollisionAction | "">>,
  replaceLibrary = false,
): CollisionPlan | null {
  if (replaceLibrary) return { scenarios: {}, supplementalChoices: [] };
  const choices: Record<string, CollisionAction> = {};
  for (const scenario of preview.scenarios) {
    if (!scenario.collides) continue;
    const action = scenarios[scenario.scenarioId];
    if (action !== "create-copy" && action !== "replace" && action !== "skip") return null;
    choices[scenario.scenarioId] = action;
  }
  const supplementalChoices: CollisionPlan["supplementalChoices"][number][] = [];
  for (const identity of preview.supplementalCollisions) {
    const action = supplemental[supplementalIdentityKey(identity)];
    if (action !== "replace" && action !== "skip") return null;
    supplementalChoices.push({ ...identity, action });
  }
  return { scenarios: choices, supplementalChoices };
}

export function portableAppliedMessage(result: PortableAppliedDto): string {
  const outcome = result.safetyBackup;
  return outcome.kind === "createdAndVerified"
    ? messages.portable.restoredWithBackup(outcome.artifactName)
    : outcome.kind === "confirmedBypass"
      ? messages.portable.restoredWithoutBackup
      : messages.portable.appliedWithoutSafetyRequirement;
}

export function createPortableWorkspaceController(
  home: ProjectHomeController,
  context: () => PortableContext,
  events: PortableExportEvents,
  createFlow: () => PortableFlow = () => new PortableReviewFlow(),
): PortableWorkspaceController {
  const state = shallowReactive<PortableWorkspaceState>({
    review: null,
    warnings: [],
    pending: false,
    stale: false,
    error: null,
    notice: null,
    safetyFailure: null,
  });
  let generation = 0;
  let active = true;
  let flow: PortableFlow | null = null;

  function current(captured: number): boolean {
    return active && generation === captured;
  }
  function clearReview(): void {
    state.review = null;
    state.warnings = [];
    state.safetyFailure = null;
    state.stale = false;
  }
  function retire(): void {
    const owner = flow;
    flow = null;
    if (owner) void home.retireReviewOwner(owner);
  }
  function discardReview(): void {
    if (state.pending) return;
    generation += 1;
    clearReview();
    state.error = null;
    state.notice = null;
    retire();
  }
  function leave(): void {
    active = false;
    generation += 1;
    clearReview();
    state.pending = false;
    state.error = null;
    state.notice = null;
    retire();
  }
  function enter(): void {
    leave();
    active = true;
  }
  function refreshBinding(): void {
    if (!active || state.pending || !state.review) return;
    const review = state.review;
    if (review.kind === "unopened") return;
    const binding = context();
    // An older settings read cannot invalidate a newer native preview. Epochs are not revisions.
    if (binding.libraryRevision != null && binding.libraryRevision > review.preview.libraryRevision)
      state.stale = true;
    if (
      review.kind === "export" &&
      (binding.project?.scenarioId !== review.scenarioId ||
        binding.project.revision !== review.scenarioRevision)
    )
      state.stale = true;
  }
  function available(): boolean {
    return active && !state.pending && !home.state.busyAction;
  }
  function owner(): PortableFlow {
    if (!flow) {
      flow = createFlow();
      home.registerReviewOwner(flow);
    }
    return flow;
  }

  async function run<T>(
    action: string,
    label: string,
    start: (owner: PortableFlow, report: (event: OperationProgressV1) => void) => SetupOperation<T>,
    success: (result: T) => string,
    publish: (response: ApiResponseDto<T>) => void,
    options: {
      mutation?: boolean;
      consume?: boolean;
      exportOutcome?: boolean;
      retainedRestore?: boolean;
    } = {},
  ): Promise<boolean> {
    if (!available()) return false;
    const captured = generation;
    const operationOwner = owner();
    let operation: SetupOperation<T> | undefined;
    state.pending = true;
    state.error = null;
    state.notice = null;
    try {
      await home.runOperation({
        action,
        label,
        refreshLibrary: options.mutation === true,
        cancel: async () => {
          await operation?.cancel();
        },
        success,
        execute: async (report) => {
          operation = start(operationOwner, report);
          const response = await operation.result;
          if (current(captured)) {
            if (options.consume) clearReview();
            state.notice = success(response.result);
            publish(response);
          }
          return response;
        },
      });
      return true;
    } catch (error) {
      if (!current(captured)) return false;
      if (
        options.retainedRestore &&
        state.review?.kind === "restore" &&
        state.review.mode === "replace-library" &&
        isRetainedRestoreFailure(error)
      ) {
        // Only the native Boolean handback retains this exact review for a stronger confirmation.
        state.safetyFailure ??= safeMessage(error);
      } else {
        if (options.consume) clearReview();
        state.error = isOperationCancelled(error) ? null : safeMessage(error);
        state.notice = isOperationCancelled(error) ? messages.portable.cancelled : null;
      }
      if (options.exportOutcome) {
        if (isOperationCancelled(error)) events.exportCancelled();
        else events.exportFailed();
      }
      return false;
    } finally {
      if (current(captured)) {
        state.pending = false;
        refreshBinding();
        // Failed/lost preview responses can still own native custody. Hand their cleanup to root.
        if (!state.review) retire();
      }
    }
  }

  function canPreview(): boolean {
    return available() && state.review === null;
  }
  function canConsume(): boolean {
    refreshBinding();
    return available() && state.review !== null && !state.stale;
  }
  function previewImport(includeResults: boolean, includeAssets: boolean): Promise<boolean> {
    if (!canPreview() || context().mode !== "import") return Promise.resolve(false);
    return run(
      "portable-import-preview",
      messages.portable.chooseImport,
      (f, report) =>
        f.previewImport(
          new LibraryOperationScope(null),
          {
            restoreMode: "import-scenario",
            includeResults,
            includeAssets,
          },
          report,
        ),
      () => messages.portable.reviewReady,
      ({ result, warnings }) => {
        state.review = { kind: "import", preview: result };
        state.warnings = warnings;
      },
    );
  }
  function previewExport(): Promise<boolean> {
    const project = context().project;
    if (!canPreview() || context().mode !== "export" || !project) return Promise.resolve(false);
    const { scenarioId, revision: scenarioRevision } = project;
    return run(
      "portable-export-preview",
      messages.portable.previewExport,
      (f, report) => f.previewExport(new SetupOperationScope(scenarioId, scenarioRevision), report),
      () => messages.portable.reviewReady,
      ({ result, warnings }) => {
        state.review = { kind: "export", preview: result, scenarioId, scenarioRevision };
        state.warnings = warnings;
      },
      { exportOutcome: true },
    );
  }
  function previewBackup(title: string): Promise<boolean> {
    if (!canPreview() || context().mode !== "backup-restore" || !title.trim())
      return Promise.resolve(false);
    return run(
      "portable-backup-preview",
      messages.portable.previewBackup,
      (f, report) => f.previewBackup(new LibraryOperationScope(null), title.trim(), report),
      () => messages.portable.reviewReady,
      ({ result, warnings }) => {
        state.review = { kind: "backup", preview: result };
        state.warnings = warnings;
      },
    );
  }
  function previewRestore(
    mode: "add-backup" | "replace-library",
    origin: "userSelected" | "safetyBackups",
  ): Promise<boolean> {
    if (!canPreview() || context().mode !== "backup-restore") return Promise.resolve(false);
    return run(
      "portable-restore-preview",
      messages.portable.chooseRestore,
      (f, report) =>
        f.previewRestore(
          new LibraryOperationScope(null),
          {
            restoreMode: mode,
            includeResults: true,
            includeAssets: true,
          },
          origin,
          report,
        ),
      () => messages.portable.reviewReady,
      ({ result, warnings }) => {
        state.review = { kind: "restore", preview: result, mode, origin };
        state.warnings = warnings;
      },
    );
  }
  function inspect(): Promise<boolean> {
    if (!canPreview() || context().mode !== "import") return Promise.resolve(false);
    return run(
      "portable-inspect",
      messages.portable.inspect,
      (f, report) => f.inspectUnopenedBundle(new LibraryOperationScope(null), report),
      () => messages.portable.inspectionReady,
      ({ result, warnings }) => {
        state.review = { kind: "unopened", preview: result };
        state.warnings = warnings;
      },
    );
  }
  function save(): Promise<boolean> {
    if (!canConsume()) return Promise.resolve(false);
    const review = state.review;
    if (!review || review.kind === "import" || review.kind === "restore")
      return Promise.resolve(false);
    return run(
      `portable-${review.kind}-save`,
      messages.portable.saveFile,
      (f, report) =>
        review.kind === "export"
          ? f.createExport(
              new SetupOperationScope(review.scenarioId, review.scenarioRevision),
              {
                previewId: review.preview.previewId,
                expectedLibraryRevision: review.preview.libraryRevision,
              },
              report,
            )
          : review.kind === "backup"
            ? f.createBackup(
                new LibraryOperationScope(review.preview.libraryRevision),
                review.preview.previewId,
                report,
              )
            : f.reexportUnopenedBundle(
                new LibraryOperationScope(null),
                review.preview.previewId,
                report,
              ),
      (result) => messages.portable.saved(result.artifactName),
      ({ result }) => {
        if (review.kind === "export") events.exported({ artifactName: result.artifactName });
      },
      { consume: true, exportOutcome: review.kind === "export" },
    );
  }
  function apply(
    scenarios: Readonly<Record<string, CollisionAction | "">>,
    supplemental: Readonly<Record<string, SupplementalCollisionAction | "">>,
    confirmation: "reviewed" | "replace" | "bypass",
    bypassPhrase = "",
  ): Promise<boolean> {
    if (!canConsume()) return Promise.resolve(false);
    const review = state.review;
    if (!review || (review.kind !== "import" && review.kind !== "restore"))
      return Promise.resolve(false);
    const replace = review.kind === "restore" && review.mode === "replace-library";
    const collisionPlan = reviewedCollisionPlan(review.preview, scenarios, supplemental, replace);
    if (!collisionPlan) {
      state.error = messages.portable.chooseEveryCollision;
      return Promise.resolve(false);
    }
    if (
      replace &&
      (state.safetyFailure
        ? confirmation !== "bypass" || bypassPhrase !== "REPLACE WITHOUT BACKUP"
        : confirmation !== "replace")
    )
      return Promise.resolve(false);
    if (!replace && confirmation !== "reviewed") return Promise.resolve(false);
    return run(
      `portable-${review.kind}-apply`,
      messages.portable.apply,
      (f, report) =>
        review.kind === "import"
          ? f.applyImport(
              new LibraryOperationScope(review.preview.libraryRevision),
              { previewId: review.preview.previewId, collisionPlan },
              report,
            )
          : f.applyRestore(
              new LibraryOperationScope(review.preview.libraryRevision),
              {
                previewId: review.preview.previewId,
                collisionPlan,
                authorization: {
                  destructiveActionConfirmed: replace,
                  safetyBackupBypassPhrase: replace && state.safetyFailure ? bypassPhrase : null,
                },
              },
              report,
            ),
      portableAppliedMessage,
      () => undefined,
      { mutation: true, consume: true, retainedRestore: replace },
    );
  }
  return {
    state,
    discardReview,
    leave,
    enter,
    refreshBinding,
    previewImport,
    previewExport,
    previewBackup,
    previewRestore,
    inspect,
    save,
    apply,
  };
}
