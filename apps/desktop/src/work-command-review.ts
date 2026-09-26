import { onScopeDispose, shallowReactive, watch } from "vue";
import {
  applyReviewedGeneration,
  getScenarioView,
  newUuidV7,
  SetupOperationScope,
  type ActorRef,
  type CommandResultDto,
  type SetupSourceV2,
  type ValidationIssue,
} from "./api/generated";
import type {
  WorkforceDateRange,
  WorkforceEntity,
  WorkforceSetupCommandChangePage,
  WorkforceSetupEntityKind,
  WorkforceSetupFacts,
  WorkforceSetupGenerationReview,
  WorkforceSetupOrdinalContinuation,
  WorkforceSetupShiftContinuation,
} from "./api/generated-domain-pack-contracts";
import { sameField } from "./entity-draft";
import {
  isOperationCancelled,
  isRedoBranchTruncation,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "./project-home";
import { messages } from "./messages";

export interface WorkGenerationFilter {
  readonly changesOnly: boolean;
  readonly dates: WorkforceDateRange | null;
}
export interface WorkReviewSnapshot {
  readonly scenarioId: string;
  readonly revision: number;
  readonly libraryEpoch: number;
  readonly commandId: string;
  readonly actor: ActorRef;
  readonly source: SetupSourceV2;
  readonly filter: WorkGenerationFilter;
}
export type WorkReviewSide = "before" | "after";
export type WorkReviewTarget =
  | { readonly kind: "settings" }
  | { readonly kind: "entity"; readonly id: string; readonly entityKind: WorkforceSetupEntityKind };
export type WorkReviewInspection =
  | {
      readonly kind: "settings";
      readonly side: WorkReviewSide;
      readonly facts: WorkforceSetupFacts;
    }
  | { readonly kind: "entity"; readonly side: WorkReviewSide; readonly entity: WorkforceEntity };
export interface WorkCommandReview {
  readonly snapshot: WorkReviewSnapshot;
  readonly changes: WorkforceSetupCommandChangePage | null;
  readonly generation: WorkforceSetupGenerationReview;
  readonly sourceFacts: WorkforceSetupFacts;
  readonly inspection: WorkReviewInspection | null;
  readonly warnings: {
    readonly changes: readonly ValidationIssue[];
    readonly generation: readonly ValidationIssue[];
    readonly inspection: readonly ValidationIssue[];
    readonly sourceFacts: readonly ValidationIssue[];
  };
}
type ReadAction =
  | { readonly kind: "initial" }
  | { readonly kind: "generation"; readonly cursor: WorkforceSetupShiftContinuation | null }
  | { readonly kind: "changes"; readonly cursor: WorkforceSetupOrdinalContinuation | null }
  | { readonly kind: "inspect"; readonly target: WorkReviewTarget; readonly side: WorkReviewSide };

/** One frozen stored-or-command source and native generation hash, not a scenario store. */
export function useWorkCommandReview(home: ProjectHomeController, project: () => ProjectSummary) {
  const state = shallowReactive<{
    review: WorkCommandReview | null;
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
  function sameContext(snapshot: WorkReviewSnapshot): boolean {
    const current = project();
    return (
      alive &&
      current.scenarioId === snapshot.scenarioId &&
      current.revision === snapshot.revision &&
      home.state.libraryEpoch === snapshot.libraryEpoch
    );
  }

  async function readReview(
    snapshot: WorkReviewSnapshot,
    previous: WorkCommandReview | null,
    action: ReadAction,
  ): Promise<boolean> {
    if (!sameContext(snapshot) || state.pending || home.state.busyAction !== null) return false;
    const captured = ++generation;
    const owned = new SetupOperationScope(snapshot.scenarioId, snapshot.revision);
    scope?.dispose();
    scope = owned;
    state.pending = true;
    state.error = null;
    state.failure = null;
    let cancelled = false;
    let cancelCurrent: (() => Promise<unknown>) | null = null;
    const current = (): boolean => !cancelled && captured === generation && sameContext(snapshot);
    const checkBetweenReads = (): void => {
      if (!current())
        throw Object.assign(new Error(messages.operations.cancelled), {
          category: "protocol",
          code: "operation.cancelled",
          message: messages.operations.cancelled,
        });
    };
    try {
      const response = await home.runOperation({
        action: "work-review",
        label: messages.work.previewing,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
          await cancelCurrent?.();
        },
        execute: async (report) => {
          if (action.kind === "inspect") {
            if (previous === null)
              throw new Error("An inspection requires an existing Work review.");
            const source: SetupSourceV2 =
              action.side === "before" ? { kind: "stored" } : snapshot.source;
            if (action.target.kind === "settings") {
              const operation = getScenarioView(
                owned,
                {
                  source,
                  query: {
                    schemaVersion: 1,
                    viewId: "official.workforce.setup.overview",
                    parameters: {},
                  },
                },
                report,
              );
              cancelCurrent = () => operation.cancel();
              const detail = await operation.result;
              const inspection: WorkReviewInspection = {
                kind: "settings",
                side: action.side,
                facts: detail.result.view.data.result.data,
              };
              return {
                ...detail,
                result: {
                  ...previous,
                  inspection,
                  warnings: { ...previous.warnings, inspection: detail.warnings },
                },
              };
            }
            const { id, entityKind } = action.target;
            const operation = getScenarioView(
              owned,
              {
                source,
                query: {
                  schemaVersion: 1,
                  viewId: "eutheto.setup.entity_detail",
                  parameters: { entityId: id, entityKind },
                },
              },
              report,
            );
            cancelCurrent = () => operation.cancel();
            const detail = await operation.result;
            const entity = detail.result.view.data.result.data;
            if (entity.id !== id || entity.kind !== entityKind)
              throw new Error("The native reviewed record did not match the requested identity.");
            const inspection: WorkReviewInspection = { kind: "entity", side: action.side, entity };
            return {
              ...detail,
              result: {
                ...previous,
                inspection,
                warnings: { ...previous.warnings, inspection: detail.warnings },
              },
            };
          }
          if (action.kind === "changes") {
            if (previous === null || snapshot.source.kind !== "commandPreview")
              throw new Error("Command-change paging requires a command-source Work review.");
            const operation = getScenarioView(
              owned,
              {
                source: snapshot.source,
                query: {
                  schemaVersion: 1,
                  viewId: "eutheto.setup.command_changes",
                  parameters: { limit: 50 },
                  continuation: action.cursor,
                },
              },
              report,
            );
            cancelCurrent = () => operation.cancel();
            const changes = await operation.result;
            const page = changes.result.view.data.result.data;
            if (page.items.length > 50)
              throw new Error("The native command page exceeded its requested bound.");
            return {
              ...changes,
              result: {
                ...previous,
                changes: page,
                warnings: { ...previous.warnings, changes: changes.warnings },
              },
            };
          }
          const operation = getScenarioView(
            owned,
            {
              source: snapshot.source,
              query: {
                schemaVersion: 1,
                viewId: "official.workforce.setup.generation_review",
                parameters: {
                  limit: 50,
                  changesOnly: snapshot.filter.changesOnly,
                  dates: snapshot.filter.dates,
                },
                continuation: action.kind === "generation" ? action.cursor : null,
              },
            },
            report,
          );
          cancelCurrent = () => operation.cancel();
          const generated = await operation.result;
          const generatedPage = generated.result.view.data.result.data;
          if (generatedPage.page.items.length > 50)
            throw new Error("The native generation page exceeded its requested bound.");
          if (
            previous !== null &&
            !sameField(previous.generation.prospectiveHash, generatedPage.prospectiveHash)
          )
            throw new Error("The native generation hash changed within a frozen Work review.");
          let changes = previous?.changes ?? null;
          let changeWarnings = previous?.warnings.changes ?? [];
          if (action.kind === "initial" && snapshot.source.kind === "commandPreview") {
            checkBetweenReads();
            const changeOperation = getScenarioView(
              owned,
              {
                source: snapshot.source,
                query: {
                  schemaVersion: 1,
                  viewId: "eutheto.setup.command_changes",
                  parameters: { limit: 50 },
                },
              },
              report,
            );
            cancelCurrent = () => changeOperation.cancel();
            const changed = await changeOperation.result;
            changes = changed.result.view.data.result.data;
            if (changes.items.length > 50)
              throw new Error("The native command page exceeded its requested bound.");
            changeWarnings = changed.warnings;
          }
          let sourceFacts = previous?.sourceFacts ?? null;
          let sourceWarnings = previous?.warnings.sourceFacts ?? [];
          if (action.kind === "initial") {
            checkBetweenReads();
            const factsOperation = getScenarioView(
              owned,
              {
                source: snapshot.source,
                query: {
                  schemaVersion: 1,
                  viewId: "official.workforce.setup.overview",
                  parameters: {},
                },
              },
              report,
            );
            cancelCurrent = () => factsOperation.cancel();
            const facts = await factsOperation.result;
            sourceFacts = facts.result.view.data.result.data;
            sourceWarnings = facts.warnings;
          }
          if (sourceFacts === null)
            throw new Error("A Work review requires native source settings.");
          const review: WorkCommandReview = {
            snapshot,
            changes,
            generation: generatedPage,
            sourceFacts,
            inspection: previous?.inspection ?? null,
            warnings: {
              changes: changeWarnings,
              generation: generated.warnings,
              inspection: previous?.warnings.inspection ?? [],
              sourceFacts: sourceWarnings,
            },
          };
          return { ...generated, result: review };
        },
        success: () => (current() ? messages.work.previewReady : ""),
      });
      if (!current()) return false;
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
      state.pending = false;
    }
  }

  async function preview(
    source: SetupSourceV2,
    filter: WorkGenerationFilter = { changesOnly: true, dates: null },
  ): Promise<boolean> {
    if (!alive || state.pending || home.state.busyAction !== null) return false;
    invalidate();
    const current = project();
    const snapshot: WorkReviewSnapshot = {
      scenarioId: current.scenarioId,
      revision: current.revision,
      libraryEpoch: home.state.libraryEpoch,
      commandId: newUuidV7(),
      actor: { actorId: null, displayName: messages.people.localActor },
      source: structuredClone(source),
      filter: structuredClone(filter),
    };
    return readReview(snapshot, null, { kind: "initial" });
  }
  async function pageGeneration(first = false): Promise<boolean> {
    const review = state.review;
    if (
      review === null ||
      state.redoRequired ||
      (!first && review.generation.page.continuation === null)
    )
      return false;
    return readReview(review.snapshot, review, {
      kind: "generation",
      cursor: first ? null : review.generation.page.continuation,
    });
  }
  async function pageChanges(first = false): Promise<boolean> {
    const review = state.review;
    if (
      review === null ||
      state.redoRequired ||
      review.changes === null ||
      (!first && review.changes.continuation === null)
    )
      return false;
    return readReview(review.snapshot, review, {
      kind: "changes",
      cursor: first ? null : review.changes.continuation,
    });
  }
  async function inspect(target: WorkReviewTarget, side: WorkReviewSide): Promise<boolean> {
    const review = state.review;
    if (review === null || state.redoRequired) return false;
    return readReview(review.snapshot, review, { kind: "inspect", target: { ...target }, side });
  }
  async function apply(truncateRedo = false): Promise<CommandResultDto | null> {
    const review = state.review;
    if (
      review === null ||
      state.pending ||
      home.state.busyAction !== null ||
      home.state.mutation?.outcome === "outcomeUnknown" ||
      !sameContext(review.snapshot) ||
      truncateRedo !== state.redoRequired ||
      (review.snapshot.source.kind === "stored" && !review.generation.reconciliationRequired)
    )
      return null;
    const captured = generation;
    const { snapshot } = review;
    const owned = new SetupOperationScope(snapshot.scenarioId, snapshot.revision);
    scope?.dispose();
    scope = owned;
    state.pending = true;
    state.applying = true;
    state.review = null;
    state.redoRequired = false;
    state.error = null;
    state.failure = null;
    let cancelCurrent: (() => Promise<unknown>) | null = null;
    try {
      const response = await home.runOperation({
        action: "work-apply",
        label: messages.work.saving,
        mutation: {
          scenarioId: snapshot.scenarioId,
          expectedRevision: snapshot.revision,
          commandId: snapshot.commandId,
          receipt: (result: CommandResultDto) => ({
            kind: result.newRevision === snapshot.revision ? "noChanges" : "applied",
            revision: result.newRevision,
          }),
        },
        cancel: async () => {
          await cancelCurrent?.();
        },
        execute: (report) => {
          const operation = applyReviewedGeneration(
            owned,
            {
              commandId: snapshot.commandId,
              actor: snapshot.actor,
              source: snapshot.source,
              prospectiveHash: review.generation.prospectiveHash,
              truncateRedo,
            },
            report,
          );
          cancelCurrent = () => operation.cancel();
          return operation.result;
        },
        success: () => messages.work.saved,
      });
      // A durable receipt survives its own revision event, disposal and library refresh.
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
      owned.dispose();
      if (scope === owned) scope = null;
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
  return { state, invalidate, preview, pageGeneration, pageChanges, inspect, apply };
}
