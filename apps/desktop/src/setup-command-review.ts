import { onScopeDispose, shallowReactive, watch } from "vue";
import {
  applyScenarioCommand,
  getScenarioView,
  newUuidV7,
  SetupOperationScope,
  type ActorRef,
  type CommandResultDto,
  type ScenarioCommand,
  type ValidationIssue,
} from "./api/generated";
import type {
  WorkforceEntity,
  WorkforceSetupCommandChangePage,
  WorkforceSetupEntityKind,
  WorkforceSetupOrdinalContinuation,
} from "./api/generated-domain-pack-contracts";
import {
  isRedoBranchTruncation,
  isOperationCancelled,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "./project-home";
import { messages } from "./messages";

export interface SetupCommandSnapshot {
  readonly scenarioId: string;
  readonly revision: number;
  readonly libraryEpoch: number;
  readonly commandId: string;
  readonly actor: ActorRef;
  readonly command: ScenarioCommand;
  readonly target: { readonly id: string; readonly kind: WorkforceSetupEntityKind } | null;
}
export interface SetupCommandReview {
  readonly snapshot: SetupCommandSnapshot;
  readonly changes: WorkforceSetupCommandChangePage;
  readonly proposed: WorkforceEntity | null;
  readonly warnings: {
    readonly changes: readonly ValidationIssue[];
    readonly proposed: readonly ValidationIssue[];
  };
}

/** Owns one explicit command approval, not the editor's raw draft or authoritative scenario. */
export function useSetupCommandReview(home: ProjectHomeController, project: () => ProjectSummary) {
  const state = shallowReactive<{
    review: SetupCommandReview | null;
    pending: boolean;
    applying: boolean;
    redoRequired: boolean;
    error: string | null;
    failure: unknown;
  }>({
    review: null,
    pending: false,
    applying: false,
    redoRequired: false,
    error: null,
    failure: null,
  });
  let generation = 0;
  let alive = true;
  let scope: SetupOperationScope | null = null;

  function invalidate(): void {
    generation += 1;
    scope?.dispose();
    scope = null;
    state.review = null;
    state.redoRequired = false;
    state.error = null;
    state.failure = null;
  }
  function sameContext(snapshot: SetupCommandSnapshot): boolean {
    const current = project();
    return (
      alive &&
      current.scenarioId === snapshot.scenarioId &&
      current.revision === snapshot.revision &&
      home.state.libraryEpoch === snapshot.libraryEpoch
    );
  }

  async function readPreview(
    snapshot: SetupCommandSnapshot,
    previous: SetupCommandReview | null,
    continuation: WorkforceSetupOrdinalContinuation | null,
    inspectTarget: SetupCommandSnapshot["target"] = null,
  ): Promise<boolean> {
    if (!sameContext(snapshot) || home.state.busyAction !== null || state.pending) return false;
    const captured = ++generation;
    const owned = new SetupOperationScope(snapshot.scenarioId, snapshot.revision);
    scope?.dispose();
    scope = owned;
    state.pending = true;
    state.error = null;
    state.failure = null;
    let cancelled = false;
    const wasCancelled = (): boolean => cancelled;
    let cancelCurrent: (() => Promise<unknown>) | null = null;
    try {
      const response = await home.runOperation({
        action: "setup-command-preview",
        label: messages.people.previewing,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
          await cancelCurrent?.();
        },
        execute: async (report) => {
          if (inspectTarget !== null && previous !== null) {
            const operation = getScenarioView(
              owned,
              {
                source: { kind: "commandPreview", command: snapshot.command },
                query: {
                  schemaVersion: 1,
                  viewId: "eutheto.setup.entity_detail",
                  parameters: { entityId: inspectTarget.id, entityKind: inspectTarget.kind },
                },
              },
              report,
            );
            cancelCurrent = () => operation.cancel();
            const detail = await operation.result;
            const proposed = detail.result.view.data.result.data;
            if (proposed.id !== inspectTarget.id || proposed.kind !== inspectTarget.kind)
              throw new Error("The native proposed record did not match the requested identity.");
            return {
              ...detail,
              result: {
                ...previous,
                proposed,
                warnings: { changes: previous.warnings.changes, proposed: detail.warnings },
              },
            };
          }
          const operation = getScenarioView(
            owned,
            {
              source: { kind: "commandPreview", command: snapshot.command },
              query: {
                schemaVersion: 1,
                viewId: "eutheto.setup.command_changes",
                parameters: { limit: 50 },
                continuation,
              },
            },
            report,
          );
          cancelCurrent = () => operation.cancel();
          const changes = await operation.result;
          let proposed = previous?.proposed ?? null;
          let proposedWarnings = previous?.warnings.proposed ?? [];
          if (previous === null && snapshot.target !== null) {
            // Cancellation between the two reads must not start another native operation.
            if (cancelled || captured !== generation || !sameContext(snapshot)) {
              throw Object.assign(new Error(messages.operations.cancelled), {
                category: "protocol",
                code: "operation.cancelled",
                message: messages.operations.cancelled,
              });
            }
            const detailOperation = getScenarioView(
              owned,
              {
                source: { kind: "commandPreview", command: snapshot.command },
                query: {
                  schemaVersion: 1,
                  viewId: "eutheto.setup.entity_detail",
                  parameters: { entityId: snapshot.target.id, entityKind: snapshot.target.kind },
                },
              },
              report,
            );
            cancelCurrent = () => detailOperation.cancel();
            const detail = await detailOperation.result;
            proposed = detail.result.view.data.result.data;
            if (proposed.id !== snapshot.target.id || proposed.kind !== snapshot.target.kind)
              throw new Error("The native proposed record did not match the requested identity.");
            proposedWarnings = detail.warnings;
          }
          return {
            ...changes,
            result: {
              snapshot,
              changes: changes.result.view.data.result.data,
              proposed,
              warnings: { changes: changes.warnings, proposed: proposedWarnings },
            },
          };
        },
        success: () =>
          captured === generation && sameContext(snapshot) && !cancelled
            ? messages.people.previewReady
            : "",
      });
      if (captured !== generation || !sameContext(snapshot) || wasCancelled()) return false;
      state.review = response.result;
      return true;
    } catch (failure) {
      if (captured === generation && alive) {
        state.review = null;
        state.redoRequired = false;
        state.error = isOperationCancelled(failure) ? null : safeMessage(failure);
        state.failure = failure;
      }
      return false;
    } finally {
      owned.dispose();
      if (scope === owned) scope = null;
      // Only one root operation can run; invalidation must not strand its local pending state.
      state.pending = false;
    }
  }

  async function preview(
    command: ScenarioCommand,
    target: SetupCommandSnapshot["target"],
  ): Promise<boolean> {
    if (!alive || home.state.busyAction !== null || state.pending) return false;
    invalidate();
    const current = project();
    const snapshot: SetupCommandSnapshot = {
      scenarioId: current.scenarioId,
      revision: current.revision,
      libraryEpoch: home.state.libraryEpoch,
      commandId: newUuidV7(),
      actor: { actorId: null, displayName: messages.people.localActor },
      // Capture once. Subsequent editor changes cannot mutate the reviewed command or its pages.
      command: structuredClone(command),
      target: target === null ? null : { ...target },
    };
    return readPreview(snapshot, null, null);
  }

  async function page(first = false): Promise<boolean> {
    const review = state.review;
    if (review === null || state.redoRequired || (!first && review.changes.continuation === null))
      return false;
    return readPreview(review.snapshot, review, first ? null : review.changes.continuation);
  }

  async function inspect(target: NonNullable<SetupCommandSnapshot["target"]>): Promise<boolean> {
    const review = state.review;
    if (review === null || state.redoRequired) return false;
    return readPreview(review.snapshot, review, null, { ...target });
  }

  async function apply(truncateRedo = false): Promise<CommandResultDto | null> {
    const review = state.review;
    if (
      review === null ||
      state.pending ||
      home.state.busyAction !== null ||
      home.state.mutation?.outcome === "outcomeUnknown" ||
      !sameContext(review.snapshot) ||
      truncateRedo !== state.redoRequired
    )
      return null;
    const captured = generation;
    const { snapshot } = review;
    state.pending = true;
    state.applying = true;
    state.review = null;
    state.redoRequired = false;
    state.error = null;
    state.failure = null;
    try {
      const response = await home.runOperation({
        action: "setup-command-apply",
        label: messages.people.saving,
        mutation: {
          scenarioId: snapshot.scenarioId,
          expectedRevision: snapshot.revision,
          commandId: snapshot.commandId,
          receipt: (result: CommandResultDto) => ({
            kind: result.newRevision === snapshot.revision ? "noChanges" : "applied",
            revision: result.newRevision,
          }),
        },
        execute: () =>
          applyScenarioCommand({
            scenarioId: snapshot.scenarioId,
            expectedRevision: snapshot.revision,
            commandId: snapshot.commandId,
            actor: snapshot.actor,
            command: snapshot.command,
            truncateRedo,
          }),
        success: () => messages.people.saved,
      });
      // A received write receipt wins over its own revision event and subsequent library refresh.
      return response.result;
    } catch (failure) {
      if (alive) {
        state.error = safeMessage(failure);
        state.failure = failure;
        if (captured === generation && sameContext(snapshot) && isRedoBranchTruncation(failure)) {
          state.review = review;
          state.redoRequired = true;
        }
      }
      return null;
    } finally {
      state.pending = false;
      state.applying = false;
    }
  }

  watch(
    [() => project().scenarioId, () => project().revision, () => home.state.libraryEpoch],
    invalidate,
    { flush: "sync" },
  );
  onScopeDispose(() => {
    alive = false;
    invalidate();
  });
  return { state, invalidate, preview, page, inspect, apply };
}
