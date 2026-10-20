import { onScopeDispose, shallowReactive, watch } from "vue";
import {
  newUuidV7,
  CsvImportFlow,
  SetupOperationScope,
  type ActorRef,
  type OperationProgressV1,
  type CsvApplyV1,
  type CsvDetectionV1,
  type CsvReadFormat,
  type CsvReportV1,
  type CsvSampleV1,
  type CsvSourceOpenedV1,
  type AssignmentCsvDetectionV1,
  type AssignmentCsvMapping,
  type AssignmentCsvPreviewV1,
  type AssignmentCsvReadFormat,
  type AssignmentCsvTarget,
  type EligibilityCsvMapping,
  type EligibilityCsvPreviewV1,
  type EligibilityMatrixCellAddress,
  type EligibilityMatrixDetection,
  type EligibilityMatrixMapping,
  type EligibilityMatrixPreviewV1,
  type ShiftCsvMapping,
  type ShiftCsvPreviewV1,
  type ShiftCsvRowDecision,
  type AvailabilityCsvMapping,
  type AvailabilityCsvPreviewV1,
  type AvailabilityCsvRowDecision,
  type PeopleCsvDecision,
  type PeopleCsvDetectionV1,
  type CsvDialect,
  type PeopleCsvMapping,
  type PeopleCsvPreviewV1,
  type SetupOperation,
} from "./api/generated";
import type { WorkforcePerson } from "./api/generated-domain-pack-contracts";
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

/** Source/format/scope/request correlation is established before publishing a sample. */
export type CsvRecordSampleState =
  | { readonly status: "idle" }
  | { readonly status: "loading"; readonly record: number }
  | { readonly status: "error"; readonly record: number; readonly message: string }
  | { readonly status: "ready"; readonly sample: CsvSampleV1 };
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

export function usePeopleCsvImport(home: ProjectHomeController, project: () => ProjectSummary) {
  const importer = useCsvImport(home, project, {
    family: "people",
    detection: (value: CsvDetectionV1): PeopleCsvDetectionV1 => {
      if (value.family !== "people") throw new Error(messages.csvImport.proposalMismatch);
      return value.data;
    },
    preview: (
      owner: CsvImportFlow,
      scope: SetupOperationScope,
      sourceId: string,
      input: { mapping: PeopleCsvMapping; decisions: readonly PeopleCsvDecision[] },
      report: (event: OperationProgressV1) => void,
    ) => owner.previewPeople(scope, sourceId, input, report),
    proposed: proposedPeople,
  });
  return {
    ...importer,
    inspect: (dialect: CsvDialect, record: number) =>
      importer.inspect({ family: "people", dialect }, record),
    preview: (mapping: PeopleCsvMapping, decisions: readonly PeopleCsvDecision[]) =>
      importer.preview({ mapping, decisions }),
  };
}

export function useAssignmentCsvImport(home: ProjectHomeController, project: () => ProjectSummary) {
  const importer = useCsvImport(home, project, {
    family: "assignments",
    detection: (value: CsvDetectionV1): AssignmentCsvDetectionV1 => {
      if (value.family !== "assignments") throw new Error(messages.csvImport.proposalMismatch);
      return value.data;
    },
    preview: (
      owner: CsvImportFlow,
      scope: SetupOperationScope,
      sourceId: string,
      input: {
        mapping: AssignmentCsvMapping;
        target: AssignmentCsvTarget;
        skippedRecords: readonly number[];
        allowEmpty: boolean;
      },
      report: (event: OperationProgressV1) => void,
    ) => owner.previewAssignments(scope, sourceId, input, report),
  });
  return {
    ...importer,
    inspect: (format: AssignmentCsvReadFormat, record: number) =>
      importer.inspect({ family: "assignments", format }, record),
    preview: (
      mapping: AssignmentCsvMapping,
      target: AssignmentCsvTarget,
      skippedRecords: readonly number[],
      allowEmpty: boolean,
    ) => importer.preview({ mapping, target, skippedRecords, allowEmpty }),
  };
}

export function useEligibilityCsvImport(
  home: ProjectHomeController,
  project: () => ProjectSummary,
) {
  const importer = useCsvImport(home, project, {
    family: "eligibility",
    detection: (value: CsvDetectionV1): PeopleCsvDetectionV1 => {
      if (value.family !== "eligibility") throw new Error(messages.csvImport.proposalMismatch);
      return value.data;
    },
    preview: (
      owner: CsvImportFlow,
      scope: SetupOperationScope,
      sourceId: string,
      input: { mapping: EligibilityCsvMapping; skippedRecords: readonly number[] },
      report: (event: OperationProgressV1) => void,
    ) => owner.previewEligibility(scope, sourceId, input, report),
  });
  return {
    ...importer,
    inspect: (dialect: CsvDialect, record: number) =>
      importer.inspect({ family: "eligibility", dialect }, record),
    preview: (mapping: EligibilityCsvMapping, skippedRecords: readonly number[]) =>
      importer.preview({ mapping, skippedRecords }),
  };
}

export function useEligibilityMatrixImport(
  home: ProjectHomeController,
  project: () => ProjectSummary,
) {
  const importer = useCsvImport(home, project, {
    family: "eligibilityMatrix",
    sourceLabels: messages.eligibilityMatrix,
    detection: (value: CsvDetectionV1): EligibilityMatrixDetection => {
      if (value.family !== "eligibilityMatrix")
        throw new Error(messages.csvImport.proposalMismatch);
      return value.data;
    },
    preview: (
      owner: CsvImportFlow,
      scope: SetupOperationScope,
      sourceId: string,
      input: {
        mapping: EligibilityMatrixMapping;
        skippedCells: readonly EligibilityMatrixCellAddress[];
      },
      report: (event: OperationProgressV1) => void,
    ) => owner.previewEligibilityMatrix(scope, sourceId, input, report),
  });
  return {
    ...importer,
    paste: (text: string) =>
      importer.choose((owner, scope, report) => owner.openEligibilityMatrix(scope, text, report)),
    inspect: (record: number) => importer.inspect({ family: "eligibilityMatrix" }, record),
    preview: (
      mapping: EligibilityMatrixMapping,
      skippedCells: readonly EligibilityMatrixCellAddress[],
    ) => importer.preview({ mapping, skippedCells }),
  };
}

export function useShiftCsvImport(home: ProjectHomeController, project: () => ProjectSummary) {
  const importer = useCsvImport(home, project, {
    family: "shifts",
    detection: (value: CsvDetectionV1): PeopleCsvDetectionV1 => {
      if (value.family !== "shifts") throw new Error(messages.csvImport.proposalMismatch);
      return value.data;
    },
    preview: (
      owner: CsvImportFlow,
      scope: SetupOperationScope,
      sourceId: string,
      input: { mapping: ShiftCsvMapping; decisions: readonly ShiftCsvRowDecision[] },
      report: (event: OperationProgressV1) => void,
    ) => owner.previewShifts(scope, sourceId, input, report),
  });
  return {
    ...importer,
    inspect: (dialect: CsvDialect, record: number) =>
      importer.inspect({ family: "shifts", dialect }, record),
    preview: (mapping: ShiftCsvMapping, decisions: readonly ShiftCsvRowDecision[]) =>
      importer.preview({ mapping, decisions }),
  };
}

export function useAvailabilityCsvImport(
  home: ProjectHomeController,
  project: () => ProjectSummary,
) {
  const importer = useCsvImport(home, project, {
    family: "availability",
    detection: (value: CsvDetectionV1): PeopleCsvDetectionV1 => {
      if (value.family !== "availability") throw new Error(messages.csvImport.proposalMismatch);
      return value.data;
    },
    preview: (
      owner: CsvImportFlow,
      scope: SetupOperationScope,
      sourceId: string,
      input: { mapping: AvailabilityCsvMapping; decisions: readonly AvailabilityCsvRowDecision[] },
      report: (event: OperationProgressV1) => void,
    ) => owner.previewAvailability(scope, sourceId, input, report),
  });
  return {
    ...importer,
    inspect: (dialect: CsvDialect, record: number) =>
      importer.inspect({ family: "availability", dialect }, record),
    preview: (mapping: AvailabilityCsvMapping, decisions: readonly AvailabilityCsvRowDecision[]) =>
      importer.preview({ mapping, decisions }),
  };
}

/** One native source/report owner; every family retains the same immutable review lifecycle. */
function useCsvImport<
  Detection,
  Preview extends
    | PeopleCsvPreviewV1
    | AssignmentCsvPreviewV1
    | EligibilityCsvPreviewV1
    | EligibilityMatrixPreviewV1
    | ShiftCsvPreviewV1
    | AvailabilityCsvPreviewV1,
  Input,
>(
  home: ProjectHomeController,
  project: () => ProjectSummary,
  family: {
    family: CsvDetectionV1["family"];
    sourceLabels?: { opening: string; opened: string; detecting: string; detected: string };
    detection: (value: CsvDetectionV1) => Detection;
    preview: (
      owner: CsvImportFlow,
      scope: SetupOperationScope,
      sourceId: string,
      input: Input,
      report: (event: OperationProgressV1) => void,
    ) => SetupOperation<Preview>;
    proposed?: (value: Preview) => readonly CsvProposedPerson[];
  },
) {
  const state = shallowReactive<{
    source: CsvSourceOpenedV1 | null;
    sourceState: "ready" | "consumed" | "uncertain";
    detection: Detection | null;
    preview: Preview | null;
    proposed: readonly CsvProposedPerson[];
    approval: CsvApproval | null;
    reportId: string | null;
    report: CsvReportV1 | null;
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
  let flow: CsvImportFlow | null = null;
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

  function detachSource(): CsvImportFlow | null {
    const previous = flow;
    flow = null;
    invalidate();
    invalidateSample();
    state.source = null;
    state.detection = null;
    state.reportId = null;
    state.report = null;
    state.stale = false;
    return previous;
  }

  async function discardSource(): Promise<boolean> {
    if (!available() || outcomeUnknown()) return false;
    const previous = detachSource();
    state.pending = true;
    try {
      return previous === null || (await home.retireReviewOwner(previous));
    } finally {
      state.pending = false;
    }
  }

  async function choose(
    open: (
      owner: CsvImportFlow,
      scope: SetupOperationScope,
      report: (event: OperationProgressV1) => void,
    ) => SetupOperation<CsvSourceOpenedV1> = (owner, scope, report) => owner.open(scope, report),
  ): Promise<boolean> {
    if (!available() || outcomeUnknown() || home.state.reviewCleanupError !== null) return false;
    const scenarioId = project().scenarioId;
    const previous = detachSource();
    state.pending = true;
    const retirement = previous === null ? true : home.retireReviewOwner(previous);
    let operation: SetupOperation<CsvSourceOpenedV1> | null = null;
    const owned: { scope: SetupOperationScope | null } = { scope: null };
    let cancelled = false;
    const wasCancelled = (): boolean => cancelled;
    try {
      const response = await home.runOperation({
        action: `${family.family}-csv-open`,
        label: family.sourceLabels?.opening ?? messages.csvImport.selecting,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
          const cancellation = operation?.cancel();
          owned.scope?.dispose();
          await cancellation;
        },
        execute: async (report) => {
          if (!(await retirement)) throw new Error(messages.csvImport.sourceResetFailed);
          if (!alive || wasCancelled() || scenarioId !== project().scenarioId)
            throw cancelledRead();
          const next = new CsvImportFlow(scenarioId, family.family);
          home.registerReviewOwner(next);
          flow = next;
          owned.scope = new SetupOperationScope(scenarioId, project().revision);
          operation = open(next, owned.scope, report);
          return operation.result;
        },
        success: () =>
          alive && !wasCancelled() && scenarioId === project().scenarioId
            ? (family.sourceLabels?.opened ?? messages.csvImport.selected)
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
      await retirement;
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
    let operation: SetupOperation<CsvDetectionV1> | null = null;
    state.pending = true;
    state.error = null;
    state.failure = null;
    let cancelled = false;
    const wasCancelled = (): boolean => cancelled;
    try {
      const response = await home.runOperation({
        action: `${family.family}-csv-detect`,
        label: family.sourceLabels?.detecting ?? messages.csvImport.detecting,
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
          alive && owner === flow && !wasCancelled()
            ? (family.sourceLabels?.detected ?? messages.csvImport.detected)
            : "",
      });
      // Detection is source-only; it does not claim a current stored revision.
      if (!alive || owner !== flow || wasCancelled()) return false;
      state.detection = family.detection(response.result);
      return true;
    } catch (error) {
      fail(error);
      return false;
    } finally {
      scope.dispose();
      state.pending = false;
    }
  }

  async function inspect(readFormat: CsvReadFormat, record: number): Promise<void> {
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
      const response = await owner.sample(scope, source.sourceId, { readFormat, record }).result;
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

  async function preview(rawInput: Input): Promise<boolean> {
    const owner = flow;
    const source = state.source;
    if (!available() || !sourceReady() || owner === null || source === null) return false;
    invalidate();
    const selected = generation;
    const captured = context();
    const input = structuredClone(rawInput);
    const scope = new SetupOperationScope(captured.scenarioId, captured.revision);
    previewScope = scope;
    state.pending = true;
    let operation: SetupOperation<Preview> | null = null;
    let cancelled = false;
    const wasCancelled = (): boolean => cancelled;
    try {
      const response = await home.runOperation({
        action: `${family.family}-csv-preview`,
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
          operation = family.preview(owner, scope, source.sourceId, input, report);
          const result = await operation.result;
          if (selected === generation && current(captured) && !wasCancelled()) {
            state.reportId = result.result.previewId;
            // Correlation failures cannot create an approval or a success announcement.
            state.proposed = family.proposed?.(result.result) ?? [];
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

  async function apply(truncateRedo = false): Promise<CsvApplyV1 | null> {
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
    let operation: SetupOperation<CsvApplyV1> | null = null;
    try {
      const response = await home.runOperation({
        action: `${family.family}-csv-apply`,
        label: messages.csvImport.applying,
        mutation: {
          scenarioId: approved.scenarioId,
          expectedRevision: approved.revision,
          commandId: approved.commandId,
          receipt: (value: CsvApplyV1) => value.outcome,
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
        action: `${family.family}-csv-report`,
        label: messages.csvImport.readingReport,
        refreshLibrary: false,
        execute: () => owner.report(id),
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
        action: `${family.family}-csv-report-save`,
        label: messages.csvImport.savingReport,
        refreshLibrary: false,
        cancel: async () => {
          await operation?.cancel();
        },
        execute: (report: (event: OperationProgressV1) => void) => {
          const saving = owner.saveReport(scope, id, report);
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
      const previous = detachSource();
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
    discardSource,
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
