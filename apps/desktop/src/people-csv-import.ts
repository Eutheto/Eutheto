import { onScopeDispose, shallowReactive, watch } from "vue";
import {
  newUuidV7,
  PeopleCsvFlow,
  SetupOperationScope,
  type ActorRef,
  type OperationProgressV1,
  type PeopleCsvApplyV1,
  type PeopleCsvDecision,
  type PeopleCsvDetectionV1,
  type PeopleCsvDialect,
  type PeopleCsvMapping,
  type PeopleCsvPreviewV1,
  type PeopleCsvRejectedRowsV1,
  type PeopleCsvSourceOpenedV1,
  type SetupOperation,
} from "./api/generated";
import type { WorkforcePerson } from "./api/generated-domain-pack-contracts";
import type { CsvRecordSampleState } from "./components/planner/people-csv-decisions";
import { messages } from "./messages";
import {
  isRedoBranchTruncation,
  isOperationCancelled,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "./project-home";

interface CsvContext {
  readonly scenarioId: string;
  readonly revision: number;
  readonly epoch: number;
}

function cancelledRead(): Error {
  return Object.assign(new Error(messages.operations.cancelled), {
    category: "protocol",
    code: "operation.cancelled",
  });
}
interface CsvApproval extends CsvContext {
  readonly sourceId: string;
  readonly previewId: string;
  readonly approvedDigest: string;
  readonly commandId: string;
  readonly actor: ActorRef;
}
export interface CsvProposedPerson {
  readonly record: number;
  readonly status: "added" | "updated";
  readonly person: WorkforcePerson;
}

function proposedPeople(value: PeopleCsvPreviewV1): readonly CsvProposedPerson[] {
  const { rows, batch } = value.preview;
  // Blocked and no-change reviews may intentionally have no proposed batch.
  if (batch === null) return [];
  const commands = new Map<string, (typeof batch.commands)[number]>();
  for (const command of batch.commands) {
    const id = command.payload.entity.id;
    if (commands.has(id)) throw new Error(messages.csvImport.proposalMismatch);
    commands.set(id, command);
  }
  const result: CsvProposedPerson[] = [];
  const records = new Set<number>();
  for (const row of rows) {
    if (row.status !== "added" && row.status !== "updated") continue;
    const command = row.personId === null ? undefined : commands.get(row.personId);
    const expected =
      row.status === "added" ? "official.workforce.add_entity" : "official.workforce.update_entity";
    if (command === undefined || command.commandType !== expected || records.has(row.record))
      throw new Error(messages.csvImport.proposalMismatch);
    records.add(row.record);
    commands.delete(command.payload.entity.id);
    result.push({ record: row.record, status: row.status, person: command.payload.entity });
  }
  if (commands.size !== 0) throw new Error(messages.csvImport.proposalMismatch);
  return result;
}

/** One native source/report owner. Mapping and identity decisions remain explicit parent drafts. */
export function usePeopleCsvImport(home: ProjectHomeController, project: () => ProjectSummary) {
  const state = shallowReactive<{
    source: PeopleCsvSourceOpenedV1 | null;
    sourceState: "ready" | "consumed" | "uncertain";
    detection: PeopleCsvDetectionV1 | null;
    preview: PeopleCsvPreviewV1 | null;
    proposed: readonly CsvProposedPerson[];
    approval: CsvApproval | null;
    reportId: string | null;
    report: PeopleCsvRejectedRowsV1 | null;
    sample: CsvRecordSampleState;
    pending: boolean;
    applying: boolean;
    redoRequired: boolean;
    stale: boolean;
    error: string | null;
    failure: unknown;
  }>({
    source: null,
    sourceState: "ready",
    detection: null,
    preview: null,
    proposed: [],
    approval: null,
    reportId: null,
    report: null,
    sample: { status: "idle" },
    pending: false,
    applying: false,
    redoRequired: false,
    stale: false,
    error: null,
    failure: null,
  });
  let alive = true;
  let flow: PeopleCsvFlow | null = null;
  let generation = 0;
  let sampleGeneration = 0;
  let previewScope: SetupOperationScope | null = null;
  let sampleScope: SetupOperationScope | null = null;

  const available = (): boolean => alive && !state.pending && home.state.busyAction === null;
  const outcomeUnknown = (): boolean => home.state.mutation?.outcome === "outcomeUnknown";
  const sourceReady = (): boolean => state.source !== null && state.sourceState === "ready";
  function context(): CsvContext {
    return {
      scenarioId: project().scenarioId,
      revision: project().revision,
      epoch: home.state.libraryEpoch,
    };
  }
  function current(captured: CsvContext): boolean {
    return (
      alive &&
      captured.scenarioId === project().scenarioId &&
      captured.revision === project().revision &&
      captured.epoch === home.state.libraryEpoch
    );
  }
  function fail(error: unknown): void {
    if (!alive) return;
    state.error = isOperationCancelled(error) ? null : safeMessage(error);
    state.failure = error;
  }
  function invalidate(): void {
    generation += 1;
    previewScope?.dispose();
    previewScope = null;
    state.preview = null;
    state.proposed = [];
    state.approval = null;
    state.redoRequired = false;
    state.error = null;
    state.failure = null;
    // The last report remains independently usable until explicit replacement/disposal.
  }
  function invalidateSample(): void {
    sampleGeneration += 1;
    sampleScope?.dispose();
    sampleScope = null;
    state.sample = { status: "idle" };
  }

  async function choose(): Promise<boolean> {
    if (!available() || home.state.reviewCleanupError !== null) return false;
    const scenarioId = project().scenarioId;
    invalidate();
    invalidateSample();
    state.pending = true;
    state.source = null;
    state.detection = null;
    state.reportId = null;
    state.report = null;
    state.stale = false;
    let operation: SetupOperation<PeopleCsvSourceOpenedV1> | null = null;
    const owned: { scope: SetupOperationScope | null } = { scope: null };
    let cancelled = false;
    const wasCancelled = (): boolean => cancelled;
    try {
      const response = await home.runOperation({
        action: "people-csv-open",
        label: messages.csvImport.selecting,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
          const cancellation = operation?.cancel();
          owned.scope?.dispose();
          await cancellation;
        },
        execute: async (report) => {
          if (flow !== null && !(await home.retireReviewOwner(flow)))
            throw new Error(messages.csvImport.sourceResetFailed);
          if (!alive || wasCancelled() || scenarioId !== project().scenarioId)
            throw cancelledRead();
          const next = new PeopleCsvFlow(scenarioId);
          home.registerReviewOwner(next);
          flow = next;
          owned.scope = new SetupOperationScope(scenarioId, project().revision);
          operation = next.open(owned.scope, report);
          return operation.result;
        },
        success: () =>
          alive && !wasCancelled() && scenarioId === project().scenarioId
            ? messages.csvImport.selected
            : "",
      });
      if (!alive || wasCancelled() || scenarioId !== project().scenarioId) return false;
      state.source = response.result;
      state.sourceState = "ready";
      return true;
    } catch (error) {
      fail(error);
      return false;
    } finally {
      owned.scope?.dispose();
      state.pending = false;
    }
  }

  async function detect(): Promise<boolean> {
    const owner = flow;
    const source = state.source;
    if (!available() || !sourceReady() || owner === null || source === null) return false;
    const captured = context();
    const scope = new SetupOperationScope(captured.scenarioId, captured.revision);
    let operation: SetupOperation<PeopleCsvDetectionV1> | null = null;
    state.pending = true;
    state.error = null;
    state.failure = null;
    let cancelled = false;
    const wasCancelled = (): boolean => cancelled;
    try {
      const response = await home.runOperation({
        action: "people-csv-detect",
        label: messages.csvImport.detecting,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
          const cancellation = operation?.cancel();
          scope.dispose();
          await cancellation;
        },
        execute: (report) => {
          operation = owner.detect(scope, source.sourceId, report);
          return operation.result;
        },
        success: () =>
          alive && owner === flow && !wasCancelled() ? messages.csvImport.detected : "",
      });
      // Detection is source-only; it does not claim a current stored revision.
      if (!alive || owner !== flow || wasCancelled()) return false;
      state.detection = response.result;
      return true;
    } catch (error) {
      fail(error);
      return false;
    } finally {
      scope.dispose();
      state.pending = false;
    }
  }

  async function inspect(dialect: PeopleCsvDialect, record: number): Promise<void> {
    const owner = flow;
    const source = state.source;
    if (!available() || !sourceReady() || owner === null || source === null) return;
    invalidateSample();
    const selected = sampleGeneration;
    const captured = context();
    const scope = new SetupOperationScope(captured.scenarioId, captured.revision);
    sampleScope = scope;
    state.sample = { status: "loading", record };
    try {
      const response = await owner.sample(scope, source.sourceId, { dialect, record }).result;
      if (selected !== sampleGeneration || owner !== flow || !current(captured)) return;
      state.sample = { status: "ready", sample: response.result };
    } catch (error) {
      if (selected === sampleGeneration && owner === flow && current(captured))
        state.sample = { status: "error", record, message: safeMessage(error) };
    } finally {
      scope.dispose();
      if (sampleScope === scope) sampleScope = null;
    }
  }

  async function preview(
    mapping: PeopleCsvMapping,
    decisions: readonly PeopleCsvDecision[],
  ): Promise<boolean> {
    const owner = flow;
    const source = state.source;
    if (!available() || !sourceReady() || owner === null || source === null) return false;
    invalidate();
    const selected = generation;
    const captured = context();
    const input = structuredClone({ mapping, decisions });
    const scope = new SetupOperationScope(captured.scenarioId, captured.revision);
    previewScope = scope;
    state.pending = true;
    let operation: SetupOperation<PeopleCsvPreviewV1> | null = null;
    let cancelled = false;
    const wasCancelled = (): boolean => cancelled;
    try {
      const response = await home.runOperation({
        action: "people-csv-preview",
        label: messages.csvImport.previewing,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
          const cancellation = operation?.cancel();
          scope.dispose();
          await cancellation;
        },
        execute: async (report) => {
          const previous = state.reportId;
          if (previous !== null) {
            await owner.discardPreview(previous);
            if (state.reportId === previous) {
              state.reportId = null;
              state.report = null;
            }
          }
          if (selected !== generation || !current(captured) || wasCancelled())
            throw cancelledRead();
          operation = owner.preview(scope, source.sourceId, input, report);
          const result = await operation.result;
          if (selected === generation && current(captured) && !wasCancelled()) {
            state.reportId = result.result.previewId;
            // Correlation failures cannot create an approval or a success announcement.
            state.proposed = proposedPeople(result.result);
          }
          return result;
        },
        success: () =>
          selected === generation && current(captured) && !wasCancelled()
            ? messages.csvImport.previewReady
            : "",
      });
      if (selected !== generation || !current(captured) || wasCancelled()) return false;
      const value = response.result;
      state.preview = value;
      state.stale = false;
      if (value.preview.disposition !== "blocked" && value.preview.approvalDigest !== null) {
        state.approval = {
          ...captured,
          sourceId: source.sourceId,
          previewId: value.previewId,
          approvedDigest: value.preview.approvalDigest,
          commandId: newUuidV7(),
          actor: { actorId: null, displayName: messages.people.localActor },
        };
      }
      return true;
    } catch (error) {
      if (selected === generation) fail(error);
      return false;
    } finally {
      scope.dispose();
      if (previewScope === scope) previewScope = null;
      state.pending = false;
    }
  }

  async function apply(truncateRedo = false): Promise<PeopleCsvApplyV1 | null> {
    const owner = flow;
    const approved = state.approval;
    if (
      !available() ||
      !sourceReady() ||
      owner === null ||
      approved === null ||
      !current(approved) ||
      truncateRedo !== state.redoRequired ||
      outcomeUnknown()
    )
      return null;
    const selected = generation;
    const scope = new SetupOperationScope(approved.scenarioId, approved.revision);
    state.approval = null;
    state.redoRequired = false;
    state.pending = true;
    state.applying = true;
    state.error = null;
    state.failure = null;
    let operation: SetupOperation<PeopleCsvApplyV1> | null = null;
    try {
      const response = await home.runOperation({
        action: "people-csv-apply",
        label: messages.csvImport.applying,
        mutation: {
          scenarioId: approved.scenarioId,
          expectedRevision: approved.revision,
          commandId: approved.commandId,
          receipt: (value: PeopleCsvApplyV1) => value.outcome,
        },
        cancel: async () => {
          await operation?.cancel();
        },
        execute: async (report) => {
          operation = owner.apply(
            scope,
            {
              sourceId: approved.sourceId,
              previewId: approved.previewId,
              approvedDigest: approved.approvedDigest,
              commandId: approved.commandId,
              actor: approved.actor,
              truncateRedo,
            },
            report,
          );
          const result = await operation.result;
          // No report follow-up I/O here. Publish the received native receipt before root refresh.
          if (alive && owner === flow) {
            state.reportId = result.result.report.previewId;
            state.report = result.result.report;
            state.sourceState = "consumed";
            state.stale = false;
            state.preview = null;
            state.proposed = [];
          }
          return result;
        },
        success: (value) =>
          value.outcome.kind === "applied"
            ? messages.csvImport.applied
            : messages.csvImport.noChanges,
      });
      return response.result;
    } catch (error) {
      fail(error);
      if (alive && owner === flow) {
        if (outcomeUnknown()) {
          state.sourceState = "uncertain";
          state.stale = false;
        } else if (selected === generation && current(approved) && isRedoBranchTruncation(error)) {
          state.approval = approved;
          state.redoRequired = true;
        }
      }
      return null;
    } finally {
      scope.dispose();
      state.pending = false;
      state.applying = false;
    }
  }

  async function readReport(): Promise<boolean> {
    const owner = flow;
    const id = state.reportId;
    if (!available() || owner === null || id === null) return false;
    state.pending = true;
    state.error = null;
    state.failure = null;
    try {
      const result = await home.runOperation({
        action: "people-csv-report",
        label: messages.csvImport.readingReport,
        refreshLibrary: false,
        execute: () => owner.rejectedRows(id),
        success: () => messages.csvImport.reportReady,
      });
      if (!alive || owner !== flow || id !== state.reportId) return false;
      state.report = result.result;
      return true;
    } catch (error) {
      fail(error);
      return false;
    } finally {
      state.pending = false;
    }
  }

  async function saveReport(): Promise<boolean> {
    const owner = flow;
    const id = state.reportId;
    if (!available() || owner === null || id === null) return false;
    const captured = context();
    const scope = new SetupOperationScope(captured.scenarioId, captured.revision);
    let operation: SetupOperation<unknown> | null = null;
    state.pending = true;
    state.error = null;
    state.failure = null;
    try {
      await home.runOperation({
        action: "people-csv-report-save",
        label: messages.csvImport.savingReport,
        refreshLibrary: false,
        cancel: async () => {
          await operation?.cancel();
        },
        execute: (report: (event: OperationProgressV1) => void) => {
          const saving = owner.saveRejectedRows(scope, id, report);
          operation = saving;
          return saving.result;
        },
        // A received file-publication receipt is not invalidated by an unrelated revision event.
        success: () => messages.csvImport.reportSaved,
      });
      return true;
    } catch (error) {
      fail(error);
      return false;
    } finally {
      scope.dispose();
      state.pending = false;
    }
  }

  watch(
    () => project().scenarioId,
    () => {
      const previous = flow;
      flow = null;
      invalidate();
      invalidateSample();
      state.source = null;
      state.detection = null;
      state.reportId = null;
      state.report = null;
      state.stale = false;
      if (previous !== null) void home.retireReviewOwner(previous);
    },
    { flush: "sync" },
  );
  watch(
    [() => project().revision, () => home.state.libraryEpoch],
    () => {
      invalidate();
      invalidateSample();
      if (sourceReady()) state.stale = true;
    },
    { flush: "sync" },
  );
  onScopeDispose(() => {
    alive = false;
    invalidate();
    invalidateSample();
    // Root keeps failed cleanup reachable, including receipts settling after this route exits.
    if (flow !== null) void home.retireReviewOwner(flow);
  });
  return {
    state,
    choose,
    detect,
    inspect,
    preview,
    apply,
    readReport,
    saveReport,
    invalidate,
    invalidateSample,
  };
}
