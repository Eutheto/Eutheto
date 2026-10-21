<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import {
  cancelSolve,
  estimateSolveModel,
  explainSolution,
  getSolveBackendOptions,
  getSolveJob,
  getSolveDiagnosticsSummary,
  listSolutions,
  listSolveRuns,
  onSolveCompleted,
  onSolveVerified,
  onSolveProgress,
  SetupOperationScope,
  startSolve,
  type AcceptedResultRefV1,
  type AppCapabilitiesDto,
  type CatalogCommand,
  type ExplanationCertainty,
  type OptimalityStatusEvidence,
  type Revision,
  type SetupOperation,
  type SolutionExplanationDtoV2,
  type SolveDiagnosticsSummaryDtoV2,
  type SolveCompletedEvent,
  type SolveEstimateModelResultV1,
  type SolveVerifiedEvent,
  type SolveJobDtoV2,
  type SolveMode,
  type SolveIntent,
  type SolveBaselineSourceV1,
  type SolvePhase,
  type SolveProgressEvent,
  type SolveStartOperation,
  type RunTerminalOutcomeV1,
  type UuidV7,
  type VerificationValue,
  type ScenarioSummaryV2,
} from "../api/generated";
import {
  isOperationCancelled,
  isRevisionConflict,
  safeMessage,
  type ProjectHomeController,
} from "../project-home";
import { messages } from "../messages";
import { isOptimizeValidationReady } from "../optimize-readiness";

import AcceptedSchedule from "./AcceptedSchedule.vue";
import InfeasibilityInspection from "./InfeasibilityInspection.vue";

const PROGRESS_DELAY_MILLISECONDS = 350;
const REJECTED_START_RECOVERY_MILLISECONDS = 30_000;
const phaseTimingFields = [
  "compileMilliseconds",
  "backendMilliseconds",
  "projectionMilliseconds",
  "structuralValidationMilliseconds",
  "scoreRecomputationMilliseconds",
  "requiredRuleVerificationMilliseconds",
  "evidencePersistenceMilliseconds",
  "optionalExplanationMilliseconds",
] as const;
type ConflictRuleAction = {
  readonly action: "inspect" | "edit";
  readonly scenarioId: UuidV7;
  readonly sourceRevision: Revision;
  readonly runId: SolveJobDtoV2["runId"];
  readonly runManifestChecksum: string;
  readonly evidenceChecksum: string;
  readonly conflictId: string;
  readonly ruleId: UuidV7;
};

const props = withDefaults(
  defineProps<{
    scenarioId: UuidV7;
    summary: Pick<ScenarioSummaryV2, "revision" | "fast" | "full">;
    currentRevision: Revision;
    capabilities: AppCapabilitiesDto | null;
    loading: boolean;
    error: string | null;
    home?: ProjectHomeController | undefined;
    ruleNavigationAvailable?: boolean;
    intent?: SolveIntent;
    baselineSource?: SolveBaselineSourceV1 | null;
    authoredOptimizeUnavailableReason?: string | null;
    authorityUnavailableReason?: string | null;
  }>(),
  {
    ruleNavigationAvailable: false,
    home: undefined,
    intent: "optimize",
    baselineSource: () => ({ kind: "accepted" }),
    authoredOptimizeUnavailableReason: null,
    authorityUnavailableReason: null,
  },
);
const emit = defineEmits<{
  retry: [];
  terminal: [job: SolveJobDtoV2];
  ruleAction: [action: ConflictRuleAction];
}>();

type CapabilityStatus = "available" | "unavailable" | "unreported";
const validationReady = computed(() =>
  isOptimizeValidationReady(props.summary, props.currentRevision),
);
function commandStatus(command: CatalogCommand): CapabilityStatus {
  const capabilities = props.capabilities;
  if (capabilities === null) return "unreported";
  if (capabilities.availableCommands.includes(command)) return "available";
  if (capabilities.unavailableCommands.includes(command)) return "unavailable";
  return "unreported";
}
const solveStartStatus = computed(() => commandStatus("solve_start"));
const solveCancelStatus = computed(() => commandStatus("solve_cancel"));
const estimateCommandStatus = computed(() => commandStatus("solve_estimate_model"));
const backendCommandStatus = computed(() => commandStatus("solve_get_backend_options"));
const solveListRunsStatus = computed(() => commandStatus("solve_list_runs"));
const executionStatus = computed<"unavailable" | "reported" | "unreported">(() => {
  const statuses = [solveStartStatus.value, solveCancelStatus.value, backendCommandStatus.value];
  if (statuses.includes("unavailable")) return "unavailable";
  if (statuses.every((status) => status === "available")) return "reported";
  return "unreported";
});
function statusLabel(status: CapabilityStatus): string {
  return status === "available"
    ? messages.setup.optimize.available
    : status === "unavailable"
      ? messages.setup.optimize.unavailable
      : messages.setup.optimize.unreported;
}

const selectedMode = ref<SolveMode>("balanced");
const panel = ref<HTMLElement>();
// The intent is captured by startSolve; changing the next request never reinterprets an active job.
const activeIntent = ref<SolveIntent | null>(null);
const maximumTimeSeconds = ref<string | number>("");
const maximumTimeMilliseconds = computed<number | null | undefined>(() => {
  // Vue converts number-input v-model values to numbers, but leaves blank as text.
  const raw = String(maximumTimeSeconds.value).trim();
  if (raw === "") return null;
  if (!/^(?:[1-9]|[12][0-9]|30)$/.test(raw)) return undefined;
  return Number(raw) * 1_000;
});
const backendOptions = shallowRef<
  readonly {
    readonly backendId: string;
    readonly backendVersion: string;
    readonly adapterVersion: string;
    readonly displayName: string;
  }[]
>([]);
const backendLoading = ref(false);
const backendError = ref<string | null>(null);
const durableLoading = ref(false);
const durableLoaded = ref(false);
const activeJob = shallowRef<SolveJobDtoV2 | null>(null);
const activeRequestId = ref<UuidV7 | null>(null);
const infeasibilityInspectionOpen = ref(false);
const infeasibilityInspectionTrigger = ref<HTMLButtonElement | null>(null);
const infeasibilityInspectionSource = computed(() => {
  const job = activeJob.value;
  if (
    !infeasibilityInspectionOpen.value ||
    job === null ||
    job.scenarioId !== props.scenarioId ||
    job.status !== "infeasible" ||
    job.solutionId !== null
  )
    return null;
  return {
    scenarioId: props.scenarioId,
    currentRevision: props.currentRevision,
    sourceRevision: job.scenarioRevision,
    runId: job.runId,
  };
});
const activeRevision = ref<Revision | null>(null);
const phase = ref<SolvePhase | null>(null);
const solveError = ref<string | null>(null);
const solveStarting = ref(false);
const cancellationRequested = ref(false);
const reused = ref(false);
const estimateResult = shallowRef<SolveEstimateModelResultV1 | null>(null);
const estimateLoading = ref(false);
const estimateError = ref<string | null>(null);
const estimateCancellationRequested = ref(false);
const estimateCancelled = ref(false);
const diagnostics = shallowRef<SolveDiagnosticsSummaryDtoV2 | null>(null);
const diagnosticsLoading = ref(false);
const diagnosticsError = ref<string | null>(null);
const terminalExplanation = shallowRef<SolutionExplanationDtoV2 | null>(null);
const terminalExplanationLoading = ref(false);
const terminalExplanationError = ref<string | null>(null);
const terminalExplanationMessages = computed(
  () => terminalExplanation.value?.explanation.rendered.messages ?? [],
);
const terminalExplanationStatus = computed<OptimalityStatusEvidence | null>(() => {
  const evidence = terminalExplanation.value?.explanation.evidence.evidence;
  return evidence?.kind === "optimalityStatus" ? evidence.status : null;
});
const terminalExplanationResult = computed(() => terminalExplanationStatus.value?.result ?? null);
let terminalExplanationGeneration = 0;
let terminalExplanationScope: SetupOperationScope | null = null;
const progressVisible = ref(false);
let alive = true;
let generation = 0;
let estimateGeneration = 0;
let solveScope: SetupOperationScope | null = null;
let solveOperation: SolveStartOperation | null = null;
let solveScenarioId: UuidV7 | null = null;
let requestRecoveryExpiresAt = 0;
let estimateScope: SetupOperationScope | null = null;
let estimateOperation: SetupOperation<SolveEstimateModelResultV1> | null = null;
let stopProgress: (() => void) | null = null;
let stopVerified: (() => void) | null = null;
let stopCompleted: (() => void) | null = null;
let subscriptionGeneration = 0;
let pendingRunRead: UuidV7 | null = null;
let notifiedTerminalRun: UuidV7 | null = null;
let pendingCompletion: SolveCompletedEvent["payload"] | null = null;
let durableRefresh: ReturnType<typeof setTimeout> | null = null;
let progressDelay: ReturnType<typeof setTimeout> | null = null;
let useVerifiedFocus: {
  readonly job: SolveJobDtoV2;
  readonly origin: Element | null;
  readonly owner: HTMLElement | undefined;
  readonly home: ProjectHomeController | undefined;
  readonly libraryEpoch: number | undefined;
} | null = null;

function stopProgressDelay(): void {
  if (progressDelay !== null) clearTimeout(progressDelay);
  progressDelay = null;
}

function stopDurableRefresh(): void {
  if (durableRefresh !== null) clearTimeout(durableRefresh);
  durableRefresh = null;
}
function hasRecoverableRequest(): boolean {
  return solveScenarioId === props.scenarioId && performance.now() < requestRecoveryExpiresAt;
}

async function discoverActiveRun(captured: number, requestId: UuidV7): Promise<void> {
  try {
    const { result } = await listSolveRuns(props.scenarioId, null);
    if (
      !alive ||
      captured !== generation ||
      activeRequestId.value !== requestId ||
      result.scenarioId !== props.scenarioId
    )
      return;
    const job = result.runs.find((run) => run.requestId === requestId);
    if (
      job === undefined ||
      job.scenarioId !== props.scenarioId ||
      job.scenarioRevision !== activeRevision.value ||
      activeJob.value !== null
    )
      return;
    updateRunIdentity(job);
    requestRecoveryExpiresAt = 0;
    solveScenarioId = null;
    notifyTerminal(job);
  } catch {
    /* The next bounded poll or start response can still recover this exact request. */
  }
}

function scheduleDurableRefresh(): void {
  if (durableRefresh !== null || !alive) return;
  const runId = activeJob.value?.runId ?? null;
  const requestId = activeRequestId.value;
  if (
    runId === null &&
    (requestId === null || (solveOperation === null && !hasRecoverableRequest()))
  )
    return;
  if (runId !== null && activeJob.value?.status !== "running") return;
  const captured = generation;
  durableRefresh = setTimeout(() => {
    durableRefresh = null;
    if (!alive || captured !== generation) return;
    if (runId === null) {
      if (activeJob.value !== null) {
        scheduleDurableRefresh();
        return;
      }
      if (
        requestId === null ||
        activeRequestId.value !== requestId ||
        (solveOperation === null && !hasRecoverableRequest())
      )
        return;
      void discoverActiveRun(captured, requestId).finally(scheduleDurableRefresh);
      return;
    }
    if (activeJob.value?.runId !== runId || activeJob.value.status !== "running") return;
    void getSolveJob(props.scenarioId, runId)
      .then(
        ({ result }) => {
          if (
            alive &&
            captured === generation &&
            result.scenarioId === props.scenarioId &&
            result.runId === runId &&
            activeJob.value?.runId === runId &&
            activeJob.value.status === "running"
          ) {
            solveError.value = null;
            updateRunIdentity(result);
            notifyTerminal(result);
          }
        },
        () => {
          if (alive && captured === generation)
            solveError.value = messages.setup.optimize.durableUnavailable;
        },
      )
      .finally(scheduleDurableRefresh);
  }, 1_000);
}

const isRunning = computed(
  () => activeJob.value?.status === "running" || (activeJob.value === null && solveStarting.value),
);
const showRunningStatus = computed(
  () => isRunning.value && (!solveStarting.value || progressVisible.value),
);
const showRunStatus = computed(
  () =>
    showRunningStatus.value || (activeJob.value !== null && activeJob.value.status !== "running"),
);
const displayedStatus = computed(
  () => activeJob.value?.status ?? (isRunning.value ? "running" : null),
);
const isStale = computed(
  () =>
    activeJob.value !== null &&
    (activeJob.value.stale || activeJob.value.scenarioRevision !== props.currentRevision),
);
const acceptedJob = computed(
  () =>
    activeJob.value !== null &&
    (activeJob.value.status === "optimal" || activeJob.value.status === "feasible") &&
    activeJob.value.solutionId !== null,
);
const acceptedTerminal = computed(
  () => props.authorityUnavailableReason === null && acceptedJob.value && !isStale.value,
);
const terminalExplanationEligible = computed(
  () => activeJob.value !== null && activeJob.value.status !== "running",
);
const terminalExplanationAccepted = computed(
  () => props.authorityUnavailableReason === null && terminalExplanationResult.value !== null,
);
const firstVerifiedSchedule = computed(() => {
  const job = activeJob.value;
  if (job?.checkpoint === null || job === null || acceptedJob.value) return null;
  return { result: job.checkpoint.result, scenarioRevision: job.scenarioRevision };
});
const canUseVerifiedResult = computed(
  () =>
    props.authorityUnavailableReason === null &&
    activeJob.value?.scenarioId === props.scenarioId &&
    activeJob.value.status === "running" &&
    firstVerifiedSchedule.value !== null &&
    !isStale.value &&
    (activeJob.value.cancelOperationId !== null ||
      (solveStarting.value && solveOperation !== null)),
);
const canStart = computed(
  () =>
    props.authorityUnavailableReason === null &&
    props.baselineSource !== null &&
    props.home?.state.busyAction == null &&
    props.home?.state.mutation?.outcome !== "outcomeUnknown" &&
    (props.intent === "repair" || props.authoredOptimizeUnavailableReason === null) &&
    validationReady.value &&
    executionStatus.value === "reported" &&
    !backendLoading.value &&
    backendError.value === null &&
    backendOptions.value.length > 0 &&
    maximumTimeMilliseconds.value !== undefined &&
    !isRunning.value &&
    !durableLoading.value &&
    !solveStarting.value &&
    props.summary.revision === props.currentRevision,
);
const estimateIsStale = computed(() => {
  const result = estimateResult.value;
  return (
    result !== null &&
    (result.stale ||
      result.scenarioId !== props.scenarioId ||
      result.scenarioRevision !== props.currentRevision ||
      result.currentRevision !== props.currentRevision)
  );
});
const canEstimate = computed(
  () =>
    props.intent === "optimize" &&
    validationReady.value &&
    estimateCommandStatus.value === "available" &&
    !estimateLoading.value &&
    props.summary.revision === props.currentRevision,
);

function currentRunMatches(context: {
  readonly requestId: UuidV7 | null;
  readonly scenarioId: UuidV7 | null;
  readonly solveRunId: UuidV7 | null;
}): boolean {
  if (context.scenarioId !== props.scenarioId) return false;
  return (
    (activeRequestId.value !== null && context.requestId === activeRequestId.value) ||
    (activeJob.value !== null && context.solveRunId === activeJob.value.runId)
  );
}

function updateRunIdentity(job: SolveJobDtoV2, requestId = job.requestId): void {
  const previous = activeJob.value;
  if (previous?.runId === job.runId && previous.status !== "running" && job.status === "running")
    return;
  activeJob.value =
    previous?.runId === job.runId && previous.checkpoint !== null && job.checkpoint === null
      ? { ...job, checkpoint: previous.checkpoint }
      : job;
  activeRequestId.value = requestId;
  activeRevision.value = job.scenarioRevision;
}

function notifyTerminal(job: SolveJobDtoV2): void {
  if (job.status === "running" || notifiedTerminalRun === job.runId) return;
  notifiedTerminalRun = job.runId;
  emit("terminal", job);
}

function completedRunId(): UuidV7 | null {
  return pendingCompletion?.context.solveRunId ?? null;
}

function observedJob(): SolveJobDtoV2 | null {
  return activeJob.value;
}

function stillCurrent(captured: number): boolean {
  return alive && captured === generation;
}

function hasActiveSolve(): boolean {
  return solveOperation !== null;
}

async function handleCompletion(event: SolveCompletedEvent["payload"]): Promise<void> {
  if (!currentRunMatches(event.context)) return;
  const captured = generation;
  pendingCompletion = event;
  phase.value = null;
  if (event.context.solveRunId === null) return;
  try {
    const response = await getSolveJob(props.scenarioId, event.context.solveRunId);
    if (
      !stillCurrent(captured) ||
      !currentRunMatches(event.context) ||
      response.result.scenarioId !== props.scenarioId ||
      response.result.runId !== event.context.solveRunId
    )
      return;
    updateRunIdentity(response.result);
    if (response.result.status === "running") scheduleDurableRefresh();
    else notifyTerminal(response.result);
  } catch {
    /* A failed advisory read cannot fabricate terminal state; retry the durable owner. */
    scheduleDurableRefresh();
  }
}

async function observeRunningRun(context: SolveProgressEvent["payload"]["context"]): Promise<void> {
  const runId = context.solveRunId;
  if (runId === null || pendingRunRead === runId || activeJob.value?.runId === runId) return;
  pendingRunRead = runId;
  const captured = generation;
  try {
    const { result } = await getSolveJob(props.scenarioId, runId);
    if (
      !stillCurrent(captured) ||
      !currentRunMatches(context) ||
      result.scenarioId !== props.scenarioId ||
      result.runId !== runId
    )
      return;
    updateRunIdentity(result);
    if (result.status === "running") scheduleDurableRefresh();
    else notifyTerminal(result);
  } catch {
    /* A lost event or transient read is recovered by the next progress/completion read. */
  } finally {
    if (pendingRunRead === runId) pendingRunRead = null;
  }
}

async function handleVerified(event: SolveVerifiedEvent["payload"]): Promise<void> {
  const runId = event.context.solveRunId;
  if (runId === null || !currentRunMatches(event.context)) return;
  const captured = generation;
  try {
    const { result } = await getSolveJob(props.scenarioId, runId);
    if (
      !stillCurrent(captured) ||
      !currentRunMatches(event.context) ||
      result.scenarioId !== props.scenarioId ||
      result.runId !== runId ||
      result.checkpoint?.result.solutionId !== event.solutionId ||
      result.checkpoint.result.resultChecksum !== event.resultChecksum ||
      result.checkpoint.checkpointChecksum !== event.checkpointChecksum ||
      (activeJob.value?.runId === runId &&
        activeJob.value.status !== "running" &&
        result.status === "running")
    )
      return;
    updateRunIdentity(result);
    if (result.status === "running") scheduleDurableRefresh();
    else notifyTerminal(result);
  } catch {
    /* The event is advisory; durable get/list reads remain authoritative. */
  }
}

async function subscribe(): Promise<void> {
  const captured = ++subscriptionGeneration;
  try {
    const [progress, verified, completed] = await Promise.all([
      onSolveProgress((event: SolveProgressEvent) => {
        if (!currentRunMatches(event.payload.context)) return;
        phase.value = event.payload.phase;
        if (event.payload.context.solveRunId !== null) {
          activeRequestId.value ??= event.payload.context.requestId;
          void observeRunningRun(event.payload.context);
        }
      }),
      onSolveVerified((event: SolveVerifiedEvent) => {
        void handleVerified(event.payload);
      }),
      onSolveCompleted((event: SolveCompletedEvent) => {
        void handleCompletion(event.payload);
      }),
    ]);
    if (!alive || captured !== subscriptionGeneration) {
      progress();
      verified();
      completed();
      return;
    }
    stopProgress = progress;
    stopVerified = verified;
    stopCompleted = completed;
  } catch {
    /* Native event delivery is advisory; durable get/list reads remain authoritative. */
  }
}

function stopSubscriptions(): void {
  subscriptionGeneration += 1;
  const progress = stopProgress;
  const completed = stopCompleted;
  const verified = stopVerified;
  stopProgress = null;
  stopCompleted = null;
  stopVerified = null;
  try {
    progress?.();
    completed?.();
    verified?.();
  } catch {
    /* Cleanup is best effort and never changes the durable solve outcome. */
  }
}

async function loadBackends(captured: number): Promise<void> {
  backendError.value = null;
  backendOptions.value = [];
  if (!validationReady.value || backendCommandStatus.value !== "available") {
    backendLoading.value = false;
    return;
  }
  backendLoading.value = true;
  try {
    const response = await getSolveBackendOptions(props.scenarioId, props.currentRevision);
    if (!alive || captured !== generation) return;
    if (
      response.result.scenarioId !== props.scenarioId ||
      response.result.revision !== props.currentRevision
    ) {
      backendError.value = messages.setup.stale;
      return;
    }
    backendOptions.value = response.result.options;
  } catch (failure) {
    if (alive && captured === generation)
      backendError.value = isRevisionConflict(failure)
        ? messages.setup.stale
        : safeMessage(failure);
  } finally {
    if (alive && captured === generation) backendLoading.value = false;
  }
}

async function loadDurable(captured: number, requestId?: UuidV7): Promise<void> {
  durableLoading.value = true;
  durableLoaded.value = false;
  if (props.capabilities === null || solveListRunsStatus.value !== "available") {
    durableLoading.value = false;
    return;
  }
  try {
    const response = await listSolveRuns(props.scenarioId, null);
    if (!alive || captured !== generation) return;
    if (
      response.result.scenarioId !== props.scenarioId ||
      response.result.currentRevision !== props.currentRevision
    ) {
      solveError.value = messages.setup.stale;
      return;
    }
    durableLoaded.value = true;
    const sorted = [...response.result.runs].sort((left, right) =>
      right.startedAt.localeCompare(left.startedAt),
    );
    const latest =
      requestId === undefined ? sorted[0] : sorted.find((run) => run.requestId === requestId);
    if (latest === undefined) {
      if (requestId === undefined && solveOperation === null) activeJob.value = null;
      return;
    }
    let durable = latest;
    try {
      const detail = await getSolveJob(props.scenarioId, latest.runId);
      if (stillCurrent(captured)) durable = detail.result;
    } catch (failure) {
      if (stillCurrent(captured) && !hasActiveSolve())
        solveError.value = isRevisionConflict(failure)
          ? messages.setup.stale
          : messages.setup.optimize.durableUnavailable;
    }
    if (!stillCurrent(captured) || hasActiveSolve()) return;
    if (
      activeJob.value?.runId === durable.runId &&
      activeJob.value.status !== "running" &&
      durable.status === "running"
    )
      return;
    updateRunIdentity(durable);
    phase.value = null;
    if (durable.status === "running") scheduleDurableRefresh();
    else notifyTerminal(durable);
  } catch (failure) {
    if (alive && captured === generation)
      solveError.value = isRevisionConflict(failure) ? messages.setup.stale : safeMessage(failure);
  } finally {
    if (alive && captured === generation) durableLoading.value = false;
  }
}
async function loadRunDiagnostics(): Promise<void> {
  const job = activeJob.value;
  if (
    job === null ||
    job.status === "running" ||
    diagnosticsLoading.value ||
    diagnostics.value?.job.runId === job.runId
  )
    return;
  const captured = generation;
  diagnosticsLoading.value = true;
  diagnosticsError.value = null;
  try {
    const response = await getSolveDiagnosticsSummary(props.scenarioId, job.runId);
    if (
      alive &&
      captured === generation &&
      activeJob.value?.runId === job.runId &&
      response.result.job.scenarioRevision === job.scenarioRevision
    )
      diagnostics.value = response.result;
  } catch (failure) {
    if (alive && captured === generation && activeJob.value?.runId === job.runId)
      diagnosticsError.value = safeMessage(failure);
  } finally {
    if (alive && captured === generation) diagnosticsLoading.value = false;
  }
}

function onDiagnosticsToggle(event: Event): void {
  if ((event.currentTarget as HTMLDetailsElement).open) void loadRunDiagnostics();
}
type TerminalExplanationContext = {
  readonly scenarioId: UuidV7;
  readonly currentRevision: Revision;
  readonly runId: UuidV7;
  readonly sourceRevision: Revision;
  readonly status: SolveJobDtoV2["status"];
  readonly solutionId: UuidV7 | null;
  readonly stale: boolean;
  readonly accepted: boolean;
};

function sameAcceptedResult(
  left: AcceptedResultRefV1 | null,
  right: AcceptedResultRefV1 | null,
): boolean {
  return left?.solutionId === right?.solutionId && left?.resultChecksum === right?.resultChecksum;
}

function terminalOutcomeMatches(
  outcome: RunTerminalOutcomeV1,
  context: TerminalExplanationContext,
  result: AcceptedResultRefV1 | null,
): boolean {
  if (result !== null) {
    return (
      outcome.type === "accepted" &&
      outcome.status === context.status &&
      outcome.solutionId === result.solutionId &&
      outcome.acceptedResultChecksum === result.resultChecksum
    );
  }
  if (context.status === "verificationAlarm") return outcome.type === "verificationAlarm";
  if (context.status === "interrupted") return outcome.type === "interrupted";
  return outcome.type === "noResult" && outcome.status === context.status;
}

function terminalCertaintyLabel(certainty: ExplanationCertainty): string {
  const labels: Record<ExplanationCertainty, string> = {
    deterministic: messages.setup.optimize.terminalExplanationCertainty.deterministic,
    independentlyVerified:
      messages.setup.optimize.terminalExplanationCertainty.independentlyVerified,
    backendProof: messages.setup.optimize.terminalExplanationCertainty.backendProof,
    sufficientConflict: messages.setup.optimize.terminalExplanationCertainty.sufficientConflict,
    provenMinimalConflict:
      messages.setup.optimize.terminalExplanationCertainty.provenMinimalConflict,
    inconclusive: messages.setup.optimize.terminalExplanationCertainty.inconclusive,
    unavailable: messages.setup.optimize.terminalExplanationCertainty.unavailable,
  };
  return labels[certainty];
}

function terminalMessageLabel(key: string): string {
  return key.startsWith("official.workforce.optimality_status.")
    ? messages.setup.optimize.terminalExplanationSummary
    : messages.setup.optimize.terminalExplanationOtherEvidence;
}

function terminalFactLabel(key: string): string {
  const name = key.startsWith("official.workforce.fact.")
    ? key.slice("official.workforce.fact.".length)
    : key;
  return name.replaceAll("_", " ");
}

function terminalFactValue(value: VerificationValue): string {
  switch (value.type) {
    case "entity":
      return `${value.value.kind} ${value.value.id}`;
    case "boolean":
      return value.value
        ? messages.setup.optimize.terminalExplanationYes
        : messages.setup.optimize.terminalExplanationNo;
    case "integer":
    case "text":
      return value.value;
  }
}

function invalidateTerminalExplanation(): void {
  terminalExplanationScope?.dispose();
  terminalExplanationScope = null;
  terminalExplanationGeneration += 1;
  terminalExplanation.value = null;
  terminalExplanationLoading.value = false;
  terminalExplanationError.value = null;
}

async function closeInfeasibilityInspection(): Promise<void> {
  const runId = infeasibilityInspectionSource.value?.runId ?? null;
  infeasibilityInspectionOpen.value = false;
  await nextTick();
  const trigger = infeasibilityInspectionTrigger.value;
  if (
    alive &&
    runId !== null &&
    activeJob.value?.runId === runId &&
    document.activeElement === document.body &&
    trigger?.isConnected
  )
    trigger.focus();
}

async function explainTerminalStatus(): Promise<void> {
  const job = activeJob.value;
  if (job === null || job.status === "running" || terminalExplanationLoading.value) return;
  const context: TerminalExplanationContext = {
    scenarioId: props.scenarioId,
    currentRevision: props.currentRevision,
    runId: job.runId,
    sourceRevision: job.scenarioRevision,
    status: job.status,
    solutionId: job.solutionId,
    stale: job.stale,
    accepted: (job.status === "optimal" || job.status === "feasible") && job.solutionId !== null,
  };
  const captured = ++terminalExplanationGeneration;
  const stillCurrent = (): boolean => {
    const current = activeJob.value;
    return (
      alive &&
      captured === terminalExplanationGeneration &&
      props.scenarioId === context.scenarioId &&
      props.currentRevision === context.currentRevision &&
      current !== null &&
      current.scenarioId === context.scenarioId &&
      current.runId === context.runId &&
      current.scenarioRevision === context.sourceRevision &&
      current.status === context.status &&
      current.solutionId === context.solutionId &&
      current.stale === context.stale
    );
  };
  terminalExplanation.value = null;
  terminalExplanationError.value = null;
  terminalExplanationLoading.value = true;
  let scope: SetupOperationScope | null = null;
  try {
    const diagnosticsResponse = await getSolveDiagnosticsSummary(context.scenarioId, context.runId);
    if (!stillCurrent()) return;
    const diagnosticsResult = diagnosticsResponse.result;
    if (
      diagnosticsResponse.currentRevision !== context.currentRevision ||
      diagnosticsResult.job.scenarioId !== context.scenarioId ||
      diagnosticsResult.job.runId !== context.runId ||
      diagnosticsResult.job.scenarioRevision !== context.sourceRevision ||
      diagnosticsResult.job.status !== context.status ||
      diagnosticsResult.manifestChecksum === null
    ) {
      terminalExplanationError.value = messages.setup.optimize.terminalExplanationSourceChanged;
      return;
    }
    const manifestChecksum = diagnosticsResult.manifestChecksum;
    let resultRef: AcceptedResultRefV1 | null = null;
    if (context.accepted) {
      const solutionsResponse = await listSolutions(context.scenarioId);
      if (!stillCurrent()) return;
      const solutions = solutionsResponse.result;
      const matchingResult = solutions.solutions.find(
        (solution) =>
          solution.scenarioId === context.scenarioId &&
          solution.runId === context.runId &&
          solution.scenarioRevision === context.sourceRevision &&
          solution.currentRevision === context.currentRevision &&
          solution.status === context.status &&
          solution.result.solutionId === context.solutionId,
      );
      if (
        solutionsResponse.currentRevision !== context.currentRevision ||
        solutions.scenarioId !== context.scenarioId ||
        solutions.currentRevision !== context.currentRevision ||
        matchingResult === undefined
      ) {
        terminalExplanationError.value = messages.setup.optimize.terminalExplanationSourceChanged;
        return;
      }
      resultRef = matchingResult.result;
    }
    scope = new SetupOperationScope(context.scenarioId, context.sourceRevision);
    terminalExplanationScope = scope;
    const explanationResponse = await explainSolution(scope, {
      schemaVersion: 1,
      subject: {
        kind: "optimalityStatus",
        solveRunId: context.runId,
        runManifestChecksum: manifestChecksum,
        result: resultRef,
      },
    }).result;
    if (!stillCurrent()) return;
    const explanationResult = explanationResponse.result;
    const evidence = explanationResult.explanation.evidence;
    const rendered = explanationResult.explanation.rendered;
    const status = evidence.evidence.kind === "optimalityStatus" ? evidence.evidence.status : null;
    if (
      explanationResult.scenarioId !== context.scenarioId ||
      explanationResult.currentRevision !== context.currentRevision ||
      explanationResult.scenarioRevisions.length !== 1 ||
      explanationResult.scenarioRevisions[0] !== context.sourceRevision ||
      status === null ||
      status.runInput.scenarioId !== context.scenarioId ||
      status.runInput.scenarioRevision !== context.sourceRevision ||
      status.runInput.runId !== context.runId ||
      status.runManifest.runId !== context.runId ||
      status.runManifest.checksum !== manifestChecksum ||
      !sameAcceptedResult(status.result, resultRef) ||
      !terminalOutcomeMatches(status.runManifest.outcome, context, resultRef) ||
      rendered.kind !== "optimalityStatus" ||
      rendered.evidenceChecksum !== evidence.checksum
    ) {
      terminalExplanationError.value = messages.setup.optimize.terminalExplanationSourceChanged;
      return;
    }
    terminalExplanation.value = explanationResult;
  } catch (failure) {
    if (stillCurrent()) terminalExplanationError.value = safeMessage(failure);
  } finally {
    if (stillCurrent()) terminalExplanationLoading.value = false;
    scope?.dispose();
    if (terminalExplanationScope === scope) terminalExplanationScope = null;
  }
}

function requestEstimateCleanup(
  operation: SetupOperation<SolveEstimateModelResultV1> | null,
  scope: SetupOperationScope | null,
): void {
  if (operation !== null) {
    void Promise.resolve()
      .then(() => operation.cancel())
      .catch(() => undefined);
    void Promise.resolve()
      .then(() => operation.release())
      .catch(() => undefined);
  }
  if (scope !== null) {
    try {
      scope.dispose();
    } catch {
      /* Native scope cleanup is best effort during route disposal. */
    }
  }
}

function invalidateEstimate(): void {
  estimateGeneration += 1;
  const operation = estimateOperation;
  const scope = estimateScope;
  estimateOperation = null;
  estimateScope = null;
  estimateLoading.value = false;
  estimateCancellationRequested.value = false;
  estimateCancelled.value = false;
  estimateError.value = null;
  requestEstimateCleanup(operation, scope);
}

async function estimateModel(): Promise<void> {
  if (!canEstimate.value || estimateOperation !== null) return;
  const capturedGeneration = ++estimateGeneration;
  const capturedScenarioId = props.scenarioId;
  const capturedRevision = props.currentRevision;
  estimateResult.value = null;
  estimateError.value = null;
  estimateCancelled.value = false;
  estimateCancellationRequested.value = false;

  let scope: SetupOperationScope | null = null;
  let operation: SetupOperation<SolveEstimateModelResultV1> | null = null;
  try {
    scope = new SetupOperationScope(capturedScenarioId, capturedRevision);
    estimateScope = scope;
    operation = estimateSolveModel(scope);
    estimateOperation = operation;
    estimateLoading.value = true;
    const response = await operation.result;
    if (
      !alive ||
      capturedGeneration !== estimateGeneration ||
      estimateOperation !== operation ||
      props.scenarioId !== capturedScenarioId ||
      props.currentRevision !== capturedRevision
    )
      return;
    const result = response.result;
    if (
      result.scenarioId !== capturedScenarioId ||
      result.scenarioRevision !== capturedRevision ||
      result.currentRevision < capturedRevision
    ) {
      estimateError.value = messages.setup.optimize.estimateStale;
      return;
    }
    estimateResult.value = result;
  } catch (failure) {
    if (
      alive &&
      capturedGeneration === estimateGeneration &&
      estimateOperation === operation &&
      props.scenarioId === capturedScenarioId &&
      props.currentRevision === capturedRevision
    ) {
      estimateResult.value = null;
      if (isOperationCancelled(failure)) estimateCancelled.value = true;
      else
        estimateError.value = isRevisionConflict(failure)
          ? messages.setup.optimize.estimateStale
          : safeMessage(failure);
    }
  } finally {
    const current =
      alive &&
      capturedGeneration === estimateGeneration &&
      props.scenarioId === capturedScenarioId &&
      props.currentRevision === capturedRevision;
    if (operation !== null && estimateOperation === operation) {
      estimateOperation = null;
      estimateScope = null;
      estimateLoading.value = false;
      estimateCancellationRequested.value = false;
      await operation.release().catch(() => undefined);
    } else if (current) {
      estimateScope = null;
      estimateLoading.value = false;
      estimateCancellationRequested.value = false;
    }
    if (scope !== null) {
      try {
        scope.dispose();
      } catch {
        /* Native scope cleanup is best effort after a settled estimate. */
      }
    }
  }
}

async function cancelEstimate(): Promise<void> {
  const operation = estimateOperation;
  if (operation === null || !estimateLoading.value || estimateCancellationRequested.value) return;
  estimateCancellationRequested.value = true;
  try {
    await operation.cancel();
  } catch (failure) {
    if (alive && estimateOperation === operation) {
      estimateCancellationRequested.value = false;
      estimateError.value = isOperationCancelled(failure)
        ? null
        : isRevisionConflict(failure)
          ? messages.setup.optimize.estimateStale
          : safeMessage(failure);
    }
  }
}

function reload(): void {
  invalidateEstimate();
  invalidateTerminalExplanation();
  const captured = ++generation;
  diagnostics.value = null;
  diagnosticsLoading.value = false;
  diagnosticsError.value = null;
  stopDurableRefresh();
  void loadBackends(captured);
  void loadDurable(captured);
  if (
    activeJob.value?.scenarioId === props.scenarioId ||
    (activeJob.value === null &&
      solveScenarioId === props.scenarioId &&
      (solveOperation !== null || hasRecoverableRequest()))
  )
    scheduleDurableRefresh();
}

async function startOptimization(): Promise<void> {
  const maximumTime = maximumTimeMilliseconds.value;
  const baselineSource = props.baselineSource;
  if (
    !canStart.value ||
    solveOperation !== null ||
    maximumTime === undefined ||
    baselineSource === null
  )
    return;
  const revision = props.currentRevision;
  const scenarioId = props.scenarioId;
  const scope = new SetupOperationScope(props.scenarioId, revision);
  let operation: SolveStartOperation;
  try {
    operation = startSolve(scope, selectedMode.value, maximumTime, props.intent, baselineSource);
    activeIntent.value = props.intent;
  } catch (failure) {
    scope.dispose();
    solveError.value = safeMessage(failure);
    return;
  }
  solveScope = scope;
  solveOperation = operation;
  solveScenarioId = scenarioId;
  requestRecoveryExpiresAt = 0;
  solveStarting.value = true;
  cancellationRequested.value = false;
  solveError.value = null;
  phase.value = null;
  activeJob.value = null;
  activeRequestId.value = operation.requestId;
  activeRevision.value = revision;
  pendingCompletion = null;
  notifiedTerminalRun = null;
  reused.value = false;
  scheduleDurableRefresh();
  stopProgressDelay();
  progressVisible.value = false;
  progressDelay = setTimeout(() => {
    progressDelay = null;
    if (alive && solveOperation === operation && isRunning.value) progressVisible.value = true;
  }, PROGRESS_DELAY_MILLISECONDS);
  let rejected = false;
  try {
    const response = await operation.result;
    if (!alive || solveOperation !== operation) return;
    reused.value = response.result.reused;
    const job = response.result.job;
    const observed = observedJob();
    if (observed?.runId !== job.runId || observed.status === "running" || job.status !== "running")
      updateRunIdentity(job, operation.requestId);
    if (completedRunId() === job.runId) phase.value = null;
    if (job.status !== "running") notifyTerminal(job);
  } catch (failure) {
    if (alive && solveOperation === operation) {
      solveError.value = isRevisionConflict(failure)
        ? messages.setup.optimize.conflict
        : safeMessage(failure);
      rejected = true;
    }
  } finally {
    stopProgressDelay();
    if (solveOperation === operation) {
      const recovering =
        rejected && alive && props.scenarioId === scenarioId && observedJob() === null;
      if (recovering)
        requestRecoveryExpiresAt = performance.now() + REJECTED_START_RECOVERY_MILLISECONDS;
      solveOperation = null;
      solveScenarioId = recovering ? scenarioId : null;
      solveStarting.value = false;
      scheduleDurableRefresh();
      await operation.release().catch(() => undefined);
      solveScope = null;
      if (rejected && alive && props.scenarioId === scenarioId)
        await loadDurable(generation, operation.requestId);
    }
  }
}

async function cancelOptimization(): Promise<boolean> {
  const operation = solveOperation;
  const restoredOperationId = activeJob.value?.cancelOperationId ?? null;
  const revision = activeRevision.value;
  if (
    (operation === null && restoredOperationId === null) ||
    cancellationRequested.value ||
    !isRunning.value ||
    revision === null
  )
    return false;
  const scenarioId = props.scenarioId;
  const runId = activeJob.value?.runId;
  const requestId = activeRequestId.value;
  const sameRun = () =>
    alive &&
    props.scenarioId === scenarioId &&
    activeRequestId.value === requestId &&
    activeRevision.value === revision &&
    (runId === undefined || activeJob.value?.runId === runId);
  cancellationRequested.value = true;
  solveError.value = null;
  try {
    const operationId = operation === null ? restoredOperationId : await operation.operationId;
    if (operationId === null) {
      if (sameRun()) cancellationRequested.value = false;
      return false;
    }
    const response = await cancelSolve({ operationId, scenarioId, expectedRevision: revision });
    if (!sameRun()) return false;
    if (response.result.acknowledgement !== "cancellationRequested") {
      cancellationRequested.value = false;
      const job = observedJob();
      if (job?.status === "running" || (job === null && solveStarting.value)) {
        solveError.value = messages.setup.optimize.cancelNotActive;
        scheduleDurableRefresh();
      }
      return false;
    }
    return true;
  } catch (failure) {
    if (!sameRun()) return false;
    cancellationRequested.value = false;
    solveError.value = isRevisionConflict(failure)
      ? messages.setup.optimize.conflict
      : safeMessage(failure);
    return false;
  }
}

async function useVerifiedResult(): Promise<void> {
  const job = activeJob.value;
  if (!canUseVerifiedResult.value || job === null || cancellationRequested.value) return;
  const focus = {
    job,
    origin: document.activeElement,
    owner: panel.value,
    home: props.home,
    libraryEpoch: props.home?.state.libraryEpoch,
  };
  useVerifiedFocus = focus;
  const requested = await cancelOptimization();
  if (useVerifiedFocus === focus && !requested) useVerifiedFocus = null;
}

watch(
  [
    () => activeJob.value?.runId,
    () => activeJob.value?.status,
    () => activeJob.value?.cancelOperationId,
    () => props.authorityUnavailableReason,
    () => props.home,
    () => props.home?.state.libraryEpoch,
  ],
  async () => {
    const focus = useVerifiedFocus;
    const job = activeJob.value;
    if (focus === null) return;
    const sameContext = () =>
      alive &&
      panel.value === focus.owner &&
      props.authorityUnavailableReason === null &&
      props.home === focus.home &&
      props.home?.state.libraryEpoch === focus.libraryEpoch &&
      props.scenarioId === focus.job.scenarioId &&
      props.currentRevision === focus.job.scenarioRevision;
    if (
      !sameContext() ||
      job?.runId !== focus.job.runId ||
      (job.status === "running" && job.cancelOperationId !== focus.job.cancelOperationId)
    ) {
      useVerifiedFocus = null;
      return;
    }
    if (job.status === "running") return;
    useVerifiedFocus = null;
    await nextTick();
    const owner = panel.value;
    if (
      !sameContext() ||
      !owner?.isConnected ||
      owner.ownerDocument !== document ||
      activeJob.value?.runId !== job.runId ||
      (document.activeElement !== focus.origin && document.activeElement !== document.body)
    )
      return;
    const target =
      owner.querySelector<HTMLElement>("[data-accepted-schedule] > h4") ??
      owner.querySelector<HTMLElement>("#setup-optimize");
    target?.focus();
  },
);

watch(
  [
    () => props.scenarioId,
    () => props.currentRevision,
    () => props.summary.revision,
    () => props.summary.full,
    () => props.summary.fast,
    () => props.capabilities,
  ],
  reload,
  { immediate: true },
);
watch([() => activeJob.value?.runId, () => activeJob.value?.status], () => {
  diagnostics.value = null;
  diagnosticsLoading.value = false;
  diagnosticsError.value = null;
  infeasibilityInspectionOpen.value = false;
});
watch(
  [
    () => props.scenarioId,
    () => props.currentRevision,
    () => activeJob.value?.runId,
    () => activeJob.value?.status,
    () => activeJob.value?.scenarioRevision,
    () => activeJob.value?.solutionId,
    () => activeJob.value?.stale,
  ],
  invalidateTerminalExplanation,
);
void subscribe();
onScopeDispose(() => {
  alive = false;
  useVerifiedFocus = null;
  stopProgressDelay();
  stopDurableRefresh();
  generation += 1;
  invalidateEstimate();
  invalidateTerminalExplanation();
  stopSubscriptions();
  solveScope?.dispose();
  solveScope = null;
});
</script>

<template>
  <section
    ref="panel"
    class="state-panel field-stack"
    aria-labelledby="setup-optimize"
    data-optimize-panel
    :data-optimize-ready="canStart ? 'true' : 'false'"
  >
    <h3 id="setup-optimize" tabindex="-1">
      {{ intent === "repair" ? messages.repair.repairPlan : messages.setup.optimize.heading }}
    </h3>
    <p>
      {{
        intent === "repair"
          ? messages.repair.repairDescription
          : messages.repair.optimizeDescription
      }}
    </p>
    <p v-if="authorityUnavailableReason" role="alert">{{ authorityUnavailableReason }}</p>
    <p v-if="intent === 'optimize' && authoredOptimizeUnavailableReason" role="status">
      Ordinary Optimize is unavailable: {{ authoredOptimizeUnavailableReason }}. Choose Repair;
      protection will not be stripped.
    </p>
    <p v-if="activeIntent && showRunStatus" role="status">
      Active request intent: {{ activeIntent }}. Changing the next intent does not change this job.
    </p>
    <p>{{ messages.setup.optimize.currentRevision(String(currentRevision)) }}</p>
    <div v-if="loading" role="status" aria-busy="true">
      {{ messages.setup.optimize.capabilityLoading }}
    </div>
    <div v-else-if="error" role="alert" data-optimize-capability-error>
      <p>{{ messages.setup.optimize.capabilityError }}</p>
      <p class="text-danger">{{ error }}</p>
      <button type="button" @click="emit('retry')">
        {{ messages.setup.optimize.capabilityRetry }}
      </button>
    </div>
    <template v-else-if="capabilities === null">
      <p data-optimize-state="capability-unavailable">
        {{ messages.setup.optimize.capabilityError }}
      </p>
      <button type="button" @click="emit('retry')">
        {{ messages.setup.optimize.capabilityRetry }}
      </button>
    </template>
    <template v-else>
      <details data-optimize-capabilities>
        <summary>{{ messages.setup.optimize.technicalCapabilities }}</summary>
        <dl class="metadata-list">
          <div>
            <dt>{{ messages.setup.optimize.solveStart }}</dt>
            <dd>{{ statusLabel(solveStartStatus) }}</dd>
          </div>
          <div>
            <dt>{{ messages.setup.optimize.solveCancel }}</dt>
            <dd>{{ statusLabel(solveCancelStatus) }}</dd>
          </div>
          <div>
            <dt>{{ messages.setup.optimize.estimateHeading }}</dt>
            <dd>{{ statusLabel(estimateCommandStatus) }}</dd>
          </div>
          <div>
            <dt>{{ messages.setup.optimize.backendHeading }}</dt>
            <dd>{{ statusLabel(backendCommandStatus) }}</dd>
          </div>
        </dl>
      </details>
      <p v-if="!validationReady" data-optimize-state="validation-not-ready">
        {{ messages.setup.optimize.validationRequired }}
      </p>
      <p
        v-else-if="executionStatus !== 'reported'"
        role="status"
        data-optimize-state="validation-ready"
      >
        {{ messages.setup.optimize.validationPassed }}
      </p>
      <p v-if="executionStatus === 'unavailable'" data-optimize-state="unsupported">
        {{ messages.setup.optimize.executionUnavailable }}
      </p>
      <p v-else-if="executionStatus === 'unreported'" data-optimize-state="unreported">
        {{ messages.setup.optimize.executionUnreported }}
      </p>
      <p v-if="intent === 'repair'" role="status">{{ messages.repair.repairEstimate }}</p>
      <section v-else aria-labelledby="setup-optimize-estimate" data-optimize-estimate>
        <h4 id="setup-optimize-estimate">{{ messages.setup.optimize.estimateHeading }}</h4>
        <p>{{ messages.setup.optimize.estimateDescription }}</p>
        <p v-if="!validationReady" data-optimize-state="estimate-validation-not-ready">
          {{ messages.setup.optimize.validationRequired }}
        </p>
        <p
          v-else-if="estimateCommandStatus === 'unavailable'"
          data-optimize-state="estimate-unavailable"
        >
          {{ messages.setup.optimize.estimateUnavailable }}
        </p>
        <p
          v-else-if="estimateCommandStatus === 'unreported'"
          data-optimize-state="estimate-unreported"
        >
          {{ messages.setup.optimize.estimateUnreported }}
        </p>
        <div class="action-row">
          <button
            type="button"
            :disabled="!canEstimate"
            :aria-busy="estimateLoading ? 'true' : 'false'"
            @click="estimateModel"
          >
            {{
              estimateLoading
                ? messages.setup.optimize.estimateLoading
                : messages.setup.optimize.estimateModel
            }}
          </button>
          <button
            v-if="estimateLoading"
            type="button"
            :disabled="estimateCancellationRequested"
            @click="cancelEstimate"
          >
            {{
              estimateCancellationRequested
                ? messages.setup.optimize.estimateCancelling
                : messages.setup.optimize.estimateCancel
            }}
          </button>
        </div>
        <p v-if="estimateError" role="alert" class="text-danger">{{ estimateError }}</p>
        <p v-else-if="estimateCancelled" role="status">
          {{ messages.setup.optimize.estimateCancelled }}
        </p>
        <section v-if="estimateResult !== null" aria-live="polite" data-optimize-estimate-result>
          <p data-optimize-state="estimate-compiled">
            {{ messages.setup.optimize.estimateCompiled }}
          </p>
          <p v-if="estimateIsStale" role="alert" data-optimize-state="estimate-stale">
            {{ messages.setup.optimize.estimateStale }}
          </p>
          <p>
            {{ messages.setup.optimize.estimateRevision(String(estimateResult.scenarioRevision)) }}
          </p>
          <p
            v-if="estimateResult.currentRevision !== estimateResult.scenarioRevision"
            data-optimize-state="estimate-response-revision"
          >
            {{
              messages.setup.optimize.estimateResponseRevision(
                String(estimateResult.currentRevision),
              )
            }}
          </p>
          <dl class="metadata-list">
            <div>
              <dt>{{ messages.setup.optimize.candidateCount }}</dt>
              <dd v-if="estimateResult.candidateCount !== null">
                {{ estimateResult.candidateCount }}
              </dd>
              <dd v-else>{{ messages.setup.optimize.countUnknown }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.optimize.variableCount }}</dt>
              <dd>{{ estimateResult.variableCount }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.optimize.constraintCount }}</dt>
              <dd>{{ estimateResult.constraintCount }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.optimize.projectionCount }}</dt>
              <dd>{{ estimateResult.projectionCount }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.optimize.objectiveTermCount }}</dt>
              <dd>{{ estimateResult.objectiveTermCount }}</dd>
            </div>
          </dl>
        </section>
      </section>
      <template v-if="executionStatus === 'reported'">
        <p data-optimize-state="reported">
          {{ messages.setup.optimize.executionReported }}
        </p>
        <template v-if="validationReady">
          <p role="status" data-optimize-state="ready">{{ messages.setup.optimize.ready }}</p>
          <p v-if="backendLoading" role="status" aria-busy="true">
            {{ messages.setup.optimize.backendLoading }}
          </p>
          <div v-else-if="backendError" role="alert">
            <p>{{ messages.setup.optimize.backendError }}</p>
            <p class="text-danger">{{ backendError }}</p>
            <button type="button" @click="reload">
              {{ messages.setup.optimize.backendRetry }}
            </button>
          </div>
          <p v-else-if="backendOptions.length === 0" data-optimize-state="backend-unavailable">
            {{ messages.setup.optimize.backendUnavailable }}
          </p>
          <details v-else>
            <summary>{{ messages.setup.optimize.backendHeading }}</summary>
            <ul class="result-summary-list">
              <li v-for="backend in backendOptions" :key="backend.backendId">
                <strong>{{ backend.displayName }}</strong>
                <code>{{ backend.backendId }}</code>
                <p>
                  {{
                    messages.setup.optimize.backendVersion(
                      backend.backendVersion,
                      backend.adapterVersion,
                    )
                  }}
                </p>
              </li>
            </ul>
          </details>
          <fieldset
            :disabled="isRunning || durableLoading || solveStarting || backendOptions.length === 0"
          >
            <legend>{{ messages.setup.optimize.modesHeading }}</legend>
            <label>
              <input
                v-model="selectedMode"
                type="radio"
                value="quick"
                name="optimize-mode"
                :aria-label="messages.setup.optimize.quick"
                aria-describedby="optimize-quick-description"
              />
              <strong>{{ messages.setup.optimize.quick }}</strong>
              <span id="optimize-quick-description">{{
                messages.setup.optimize.quickDescription
              }}</span>
            </label>
            <label>
              <input
                v-model="selectedMode"
                type="radio"
                value="balanced"
                name="optimize-mode"
                :aria-label="messages.setup.optimize.balanced"
                aria-describedby="optimize-balanced-description"
              />
              <strong>{{ messages.setup.optimize.balanced }}</strong>
              <span id="optimize-balanced-description">{{
                messages.setup.optimize.balancedDescription
              }}</span>
            </label>
            <label>
              <input
                v-model="selectedMode"
                type="radio"
                value="deep"
                name="optimize-mode"
                :aria-label="messages.setup.optimize.deep"
                aria-describedby="optimize-deep-description"
              />
              <strong>{{ messages.setup.optimize.deep }}</strong>
              <span id="optimize-deep-description">{{
                messages.setup.optimize.deepDescription
              }}</span>
            </label>
            <label for="optimize-maximum-time">{{
              messages.setup.optimize.maximumTimeLabel
            }}</label>
            <input
              id="optimize-maximum-time"
              v-model="maximumTimeSeconds"
              type="number"
              inputmode="numeric"
              min="1"
              max="30"
              step="1"
              :aria-invalid="maximumTimeMilliseconds === undefined"
              :aria-describedby="
                maximumTimeMilliseconds === undefined
                  ? 'optimize-maximum-time-help optimize-maximum-time-error'
                  : 'optimize-maximum-time-help'
              "
            />
            <p id="optimize-maximum-time-help">{{ messages.setup.optimize.maximumTimeHelp }}</p>
            <p
              v-if="maximumTimeMilliseconds === undefined"
              id="optimize-maximum-time-error"
              role="alert"
            >
              {{ messages.setup.optimize.maximumTimeInvalid }}
            </p>
          </fieldset>
          <p>{{ messages.setup.optimize.modeBoundary }}</p>
          <div class="action-row">
            <button
              type="button"
              :disabled="!canStart"
              :aria-busy="solveStarting ? 'true' : 'false'"
              @click="startOptimization"
            >
              {{
                solveStarting
                  ? messages.setup.optimize.solveStarting
                  : intent === "repair"
                    ? messages.repair.repairPlan
                    : messages.setup.optimize.solveStart
              }}
            </button>
          </div>
        </template>
      </template>
    </template>
    <p
      v-if="durableLoaded && activeJob === null && !isRunning && solveError === null"
      data-optimize-state="no-run"
    >
      {{ messages.setup.optimize.noRun }}
    </p>
    <section v-if="showRunStatus || solveError" aria-live="polite">
      <h4>{{ messages.setup.optimize.heading }}</h4>
      <p v-if="showRunningStatus" role="status">{{ messages.setup.optimize.running }}</p>
      <dl v-if="displayedStatus !== null" class="metadata-list">
        <div>
          <dt>{{ messages.setup.optimize.status }}</dt>
          <dd>{{ messages.setup.optimize.statuses[displayedStatus] }}</dd>
        </div>
        <div v-if="activeRevision !== null">
          <dt>{{ messages.setup.optimize.inputRevision }}</dt>
          <dd>{{ messages.setup.optimize.runRevision(String(activeRevision)) }}</dd>
        </div>
      </dl>
      <p v-if="phase && showRunningStatus" role="status">
        {{ messages.setup.optimize.phase }}:
        {{ messages.setup.optimize.phases[phase] }}
      </p>
      <p v-if="isStale" role="alert">{{ messages.setup.optimize.stale }}</p>
      <p v-if="solveError" role="alert" class="text-danger">{{ solveError }}</p>
      <p v-if="acceptedTerminal" role="status">{{ messages.setup.optimize.acceptedNotice }}</p>
      <p v-else-if="acceptedJob" role="status">
        {{ messages.setup.optimize.acceptedStale }}
      </p>
      <p
        v-else-if="firstVerifiedSchedule !== null"
        role="status"
        data-optimize-state="first-verified"
      >
        {{
          isRunning
            ? messages.setup.optimize.firstVerified
            : messages.setup.optimize.retainedVerified
        }}
      </p>
      <p
        v-else-if="activeJob !== null && activeJob.status !== 'running'"
        data-optimize-state="no-result"
      >
        {{ messages.setup.optimize.noResult }}
      </p>
      <div v-if="canUseVerifiedResult" class="field-stack">
        <button
          type="button"
          :disabled="cancellationRequested"
          aria-describedby="optimize-use-verified-help"
          @click="useVerifiedResult"
        >
          {{
            cancellationRequested
              ? messages.setup.optimize.useVerifiedStopping
              : messages.setup.optimize.useVerifiedResult
          }}
        </button>
        <p id="optimize-use-verified-help">{{ messages.setup.optimize.useVerifiedHelp }}</p>
      </div>
      <button
        v-if="terminalExplanationEligible"
        type="button"
        data-optimize-terminal-explain
        :disabled="terminalExplanationLoading"
        :aria-busy="terminalExplanationLoading ? 'true' : 'false'"
        @click="explainTerminalStatus"
      >
        {{
          terminalExplanationLoading
            ? messages.setup.optimize.terminalExplanationLoading
            : messages.setup.optimize.terminalExplanationOpen
        }}
      </button>
      <button
        v-if="activeJob?.status === 'infeasible' && activeJob.solutionId === null"
        ref="infeasibilityInspectionTrigger"
        type="button"
        data-optimize-infeasibility-inspect
        :disabled="infeasibilityInspectionOpen"
        @click="infeasibilityInspectionOpen = true"
      >
        Inspect mapped conflict
      </button>
      <InfeasibilityInspection
        v-if="infeasibilityInspectionSource !== null"
        :source="infeasibilityInspectionSource"
        :home="home"
        :rule-navigation-available="ruleNavigationAvailable"
        @close="closeInfeasibilityInspection"
        @rule-action="emit('ruleAction', $event)"
      />
      <p v-if="isRunning && solveOperation === null" data-optimize-state="durable-running">
        {{ messages.setup.optimize.detachedRunning }}
      </p>
      <button
        v-if="
          isRunning && (solveOperation !== null || (activeJob?.cancelOperationId ?? null) !== null)
        "
        type="button"
        :disabled="cancellationRequested"
        @click="cancelOptimization"
      >
        {{
          cancellationRequested
            ? messages.setup.optimize.solveCancelling
            : messages.setup.optimize.solveCancel
        }}
      </button>
      <p v-if="reused">{{ messages.setup.optimize.reused }}</p>
      <details
        v-if="
          activeJob !== null &&
          activeJob.status !== 'running' &&
          commandStatus('solve_get_diagnostics_summary') === 'available'
        "
        @toggle="onDiagnosticsToggle"
      >
        <summary>{{ messages.setup.optimize.diagnosticsHeading }}</summary>
        <p v-if="diagnosticsLoading" role="status">
          {{ messages.setup.optimize.diagnosticsLoading }}
        </p>
        <p v-if="diagnosticsError" role="alert" class="text-danger">{{ diagnosticsError }}</p>
        <dl
          v-if="diagnostics !== null && diagnostics.job.runId === activeJob.runId"
          class="metadata-list"
        >
          <div>
            <dt>{{ messages.setup.optimize.diagnosticsElapsed }}</dt>
            <dd>
              {{
                diagnostics.elapsedMilliseconds === null
                  ? messages.setup.optimize.diagnosticsNotRecorded
                  : diagnostics.elapsedMilliseconds
              }}
            </dd>
          </div>
          <div>
            <dt>{{ messages.setup.optimize.diagnosticsIncumbent }}</dt>
            <dd>
              {{
                diagnostics.firstIncumbentMilliseconds === null
                  ? messages.setup.optimize.diagnosticsNotRecorded
                  : diagnostics.firstIncumbentMilliseconds
              }}
            </dd>
          </div>
          <div>
            <dt>{{ messages.setup.optimize.diagnosticsVerified }}</dt>
            <dd>
              {{
                diagnostics.firstVerifiedFeasibleMilliseconds === null
                  ? messages.setup.optimize.diagnosticsNotRecorded
                  : diagnostics.firstVerifiedFeasibleMilliseconds
              }}
            </dd>
          </div>
          <div v-if="diagnostics.diagnosticCode !== null">
            <dt>{{ messages.setup.optimize.diagnosticsCode }}</dt>
            <dd>
              <code>{{ diagnostics.diagnosticCode }}</code>
            </dd>
          </div>
        </dl>
        <section
          v-if="
            diagnostics !== null &&
            activeJob !== null &&
            diagnostics.job.runId === activeJob.runId &&
            diagnostics.phaseTimings !== null
          "
          aria-labelledby="optimize-phase-timings"
        >
          <h5 id="optimize-phase-timings">{{ messages.setup.optimize.diagnosticsPhases }}</h5>
          <dl class="metadata-list">
            <template v-for="field in phaseTimingFields" :key="field">
              <div v-if="diagnostics.phaseTimings[field] !== null">
                <dt>{{ messages.setup.optimize.diagnosticsPhaseLabels[field] }}</dt>
                <dd>{{ diagnostics.phaseTimings[field] }}</dd>
              </div>
            </template>
          </dl>
        </section>
      </details>
      <section
        v-if="
          terminalExplanationLoading ||
          terminalExplanationError !== null ||
          terminalExplanation !== null
        "
        aria-live="polite"
        data-optimize-terminal-explanation
      >
        <h5>{{ messages.setup.optimize.terminalExplanationHeading }}</h5>
        <p v-if="terminalExplanationLoading" role="status" aria-busy="true">
          {{ messages.setup.optimize.terminalExplanationLoading }}
        </p>
        <div v-if="terminalExplanationError !== null" role="alert">
          <p>{{ messages.setup.optimize.terminalExplanationError }}</p>
          <p class="text-danger">{{ terminalExplanationError }}</p>
          <button type="button" @click="explainTerminalStatus">
            {{ messages.setup.optimize.terminalExplanationRetry }}
          </button>
        </div>
        <template v-if="terminalExplanation !== null && terminalExplanationStatus !== null">
          <p>{{ messages.setup.optimize.terminalExplanationLimits }}</p>
          <dl class="metadata-list">
            <div v-if="displayedStatus !== null">
              <dt>{{ messages.setup.optimize.status }}</dt>
              <dd>{{ messages.setup.optimize.statuses[displayedStatus] }}</dd>
            </div>
            <div v-if="activeRevision !== null">
              <dt>{{ messages.setup.optimize.inputRevision }}</dt>
              <dd>{{ messages.setup.optimize.runRevision(String(activeRevision)) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.optimize.terminalExplanationRun }}</dt>
              <dd>
                <code>{{ terminalExplanationStatus.runInput.runId }}</code>
              </dd>
            </div>
            <div>
              <dt>{{ messages.setup.optimize.terminalExplanationDocument }}</dt>
              <dd>
                <code>{{ terminalExplanationStatus.runInput.snapshotDocumentHash }}</code>
              </dd>
            </div>
            <div>
              <dt>{{ messages.setup.optimize.terminalExplanationManifest }}</dt>
              <dd>
                <code>{{ terminalExplanationStatus.runManifest.checksum }}</code>
              </dd>
            </div>
            <div>
              <dt>{{ messages.setup.optimize.terminalExplanationSourceRevision }}</dt>
              <dd>
                {{
                  messages.setup.optimize.runRevision(
                    String(terminalExplanationStatus.runInput.scenarioRevision),
                  )
                }}
              </dd>
            </div>
            <div>
              <dt>{{ messages.setup.optimize.terminalExplanationCertaintyLabel }}</dt>
              <dd>
                {{ terminalCertaintyLabel(terminalExplanation.explanation.evidence.certainty) }}
              </dd>
            </div>
          </dl>
          <p
            v-if="terminalExplanationAccepted"
            role="status"
            data-optimize-state="terminal-explanation-accepted"
          >
            {{ messages.setup.optimize.terminalExplanationAccepted }}
          </p>
          <p v-else role="status" data-optimize-state="terminal-explanation-no-result">
            {{ messages.setup.optimize.terminalExplanationNoResult }}
          </p>
          <p v-if="isStale" role="alert" data-optimize-state="terminal-explanation-older">
            {{ messages.setup.optimize.terminalExplanationOlderSource }}
          </p>
          <p v-if="terminalExplanationMessages.length" role="status">
            {{ messages.setup.optimize.terminalExplanationEvidence }}
          </p>
          <ol v-if="terminalExplanationMessages.length">
            <li
              v-for="(message, index) in terminalExplanationMessages"
              :key="`${message.messageKey}-${index}`"
            >
              <p>{{ terminalMessageLabel(message.messageKey) }}</p>
              <dl v-if="Object.keys(message.parameters).length">
                <div v-for="(value, key) in message.parameters" :key="key">
                  <dt>{{ terminalFactLabel(key) }}</dt>
                  <dd>{{ terminalFactValue(value) }}</dd>
                </div>
              </dl>
              <p v-if="message.rules.length">
                {{ messages.setup.optimize.terminalExplanationRules }}:
                {{ message.rules.join(", ") }}
              </p>
              <p v-if="message.assignments.length">
                {{ messages.setup.optimize.terminalExplanationAssignments }}:
                {{ message.assignments.join(", ") }}
              </p>
              <p v-if="message.evidence.length">
                {{ messages.setup.optimize.terminalExplanationEvidenceIds }}:
                {{ message.evidence.join(", ") }}
              </p>
            </li>
          </ol>
          <p v-else role="status">
            {{ messages.setup.optimize.terminalExplanationEmpty }}
          </p>
        </template>
      </section>
    </section>
    <AcceptedSchedule
      v-if="firstVerifiedSchedule !== null && authorityUnavailableReason === null"
      :scenario-id="scenarioId"
      :summary="firstVerifiedSchedule"
      :current-revision="currentRevision"
      :home="home"
    />
  </section>
</template>
