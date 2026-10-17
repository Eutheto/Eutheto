<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, unref, watch } from "vue";
import { RouterLink } from "vue-router";
import {
  cancelSolve,
  getAlternativesGroup,
  getSolutionSummary,
  getSolveBackendOptions,
  getSolveJob,
  SetupOperationScope,
  startAlternatives,
  type AlternativesGroupDtoV1,
  type AlternativesGroupStatusV1,
  type AlternativesStartOperation,
  type AppCapabilitiesDto,
  type Revision,
  type ScenarioSummaryV2,
  type SolutionSummaryDtoV1,
  type SolveMode,
  type SolveBaselineSourceV1,
  type UuidV7,
  type WorkforceAlternativeMemberV1,
  type WorkforceAlternativeProfileV1,
} from "../api/generated";
import {
  isRevisionConflict,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";
import { isOptimizeValidationReady } from "../optimize-readiness";
import { messages } from "../messages";

type Binding = Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision" | "runId">;
type Context = {
  owner: ProjectHomeController;
  libraryEpoch: Revision;
  scenarioId: UuidV7;
  visibleRevision: Revision;
};
const props = withDefaults(
  defineProps<{
    home: ProjectHomeController;
    project: ProjectSummary;
    summary: ScenarioSummaryV2;
    capabilities: AppCapabilitiesDto | null;
    loading: boolean;
    error: string | null;
    baselineAvailable: boolean;
    baselineSource?: SolveBaselineSourceV1 | null;
    authorityUnavailableReason: string | null;
    choiceUnavailable: boolean;
  }>(),
  { baselineSource: () => ({ kind: "accepted" }) },
);
const emit = defineEmits<{
  retry: [];
  terminal: [];
  pairChanged: [];
  compare: [
    pair: Context & {
      base: Binding;
      candidate: Binding;
      baseLabel: string;
      candidateLabel: string;
    },
    trigger: HTMLButtonElement,
  ];
  view: [context: Context & { result: Binding }, trigger: HTMLButtonElement];
}>();
const profiles: readonly WorkforceAlternativeProfileV1[] = [
  "balanced",
  "preferenceFocused",
  "minimalChanges",
  "fairnessFirst",
];
const profileLabels: Record<WorkforceAlternativeProfileV1, string> = {
  balanced: "Balanced",
  preferenceFocused: "Preference-focused",
  minimalChanges: "Minimal-changes",
  fairnessFirst: "Fairness-first",
};
const group = shallowRef<AlternativesGroupDtoV1 | null>(null);
const discovering = ref(false);
const discovered = ref(false);
const queryError = ref<string | null>(null);
const actionError = ref<string | null>(null);
const notice = ref<string | null>(null);
const starting = ref(false);
const cancelling = ref(false);
const backendLoading = ref(false);
const backendAvailable = ref(false);
const backendError = ref<string | null>(null);
const mode = ref<Exclude<SolveMode, "custom">>("balanced");
const maximumSeconds = ref<string | number>("");
const maximumMs = computed(() => {
  const raw = String(maximumSeconds.value).trim();
  return raw === "" ? null : /^(?:[1-9]|[12][0-9]|30)$/.test(raw) ? Number(raw) * 1000 : undefined;
});
const left = ref<WorkforceAlternativeProfileV1 | "">("");
const right = ref<WorkforceAlternativeProfileV1 | "">("");
const readingChoice = ref(false);
const startTrigger = ref<HTMLButtonElement | null>(null);
const cancelTrigger = ref<HTMLButtonElement | null>(null);
let alive = true;
let generation = 0;
let queryEpoch = 0;
let choiceEpoch = 0;
let backendEpoch = 0;
let timer: ReturnType<typeof setTimeout> | null = null;
let scope: SetupOperationScope | null = null;
let operation: AlternativesStartOperation | null = null;
let notifiedRequest: UuidV7 | null = null;
let pendingLookupExpiresAt = 0;
const validationReady = computed(() =>
  isOptimizeValidationReady(props.summary, props.project.revision),
);
const heading = ref<HTMLHeadingElement | null>(null);
const readAvailable = computed(
  () => props.capabilities?.availableCommands.includes("alternatives_get_group") === true,
);
const executionAvailable = computed(() =>
  ["alternatives_start", "solve_cancel", "solve_get_backend_options"].every(
    (command) =>
      props.capabilities?.availableCommands.some((available) => available === command) === true,
  ),
);
const active = computed(() => starting.value || (group.value !== null && !group.value.settled));
const canStart = computed(
  () =>
    discovered.value &&
    !discovering.value &&
    queryError.value === null &&
    !active.value &&
    !props.loading &&
    props.error === null &&
    executionAvailable.value &&
    validationReady.value &&
    props.baselineAvailable &&
    props.baselineSource !== null &&
    props.authorityUnavailableReason === null &&
    !props.project.archived &&
    props.home.state.phase === "ready" &&
    props.home.state.busyAction === null &&
    props.home.state.mutation?.outcome !== "outcomeUnknown" &&
    backendAvailable.value &&
    !backendLoading.value &&
    backendError.value === null &&
    maximumMs.value !== undefined,
);
const choices = computed(
  () =>
    group.value?.snapshot?.members.filter(
      (member) => member.outcome?.type === "accepted" && member.equivalence?.kind !== "equivalent",
    ) ?? [],
);
const canChoose = computed(
  () =>
    !props.choiceUnavailable && props.authorityUnavailableReason === null && !readingChoice.value,
);
const stopLabels = {
  cancelled: "Cancellation stopped the remaining profiles.",
  timeLimit: "The shared time limit stopped the remaining profiles.",
  sourceChanged: "The source changed. Remaining profiles were not rebased or restarted.",
  failed: "A native failure stopped the remaining profiles.",
  verificationAlarm:
    "A verification alarm stopped the remaining profiles. Do not use an unaccepted result.",
  finalizationUnknown:
    "Finalization is unconfirmed. Reconcile the exact run in History; do not replay it.",
  summaryUnavailable:
    "The native group summary is unavailable. Use History; this request is not permission to replay runs.",
  interrupted: "The group was interrupted. Use History to reconcile recorded runs.",
};
function context(): Context {
  return {
    owner: props.home,
    libraryEpoch: props.home.state.libraryEpoch,
    scenarioId: props.project.scenarioId,
    visibleRevision: props.project.revision,
  };
}
function owns(captured: Context, token: number): boolean {
  return (
    alive &&
    token === generation &&
    props.home === captured.owner &&
    props.home.state.libraryEpoch === captured.libraryEpoch &&
    props.project.scenarioId === captured.scenarioId &&
    props.project.revision === captured.visibleRevision
  );
}
function stopTimer(): void {
  if (timer !== null) clearTimeout(timer);
  timer = null;
}
function clearChoices(): void {
  choiceEpoch += 1;
  left.value = "";
  right.value = "";
  readingChoice.value = false;
  emit("pairChanged");
}
function publish(value: AlternativesGroupStatusV1, captured: Context): void {
  if (
    value.currentRevision !== captured.visibleRevision ||
    value.scenarioId !== captured.scenarioId ||
    value.scenarioRevision !== captured.visibleRevision
  ) {
    queryError.value = messages.setup.stale;
    stopTimer();
    return;
  }
  const next = value.group;
  // A slower active-state read must not replace an already observed native settlement.
  if (group.value?.settled && next?.requestId === group.value.requestId && !next.settled) return;
  if (next?.requestId !== group.value?.requestId) clearChoices();
  group.value = next;
  discovered.value = true;
  queryError.value = null;
  if (next?.settled) {
    stopTimer();
    if (notifiedRequest !== next.requestId) {
      notifiedRequest = next.requestId;
      emit("terminal");
    }
  }
}
function scheduleQuery(): void {
  if (
    !alive ||
    timer !== null ||
    !readAvailable.value ||
    group.value?.settled ||
    queryError.value === messages.setup.stale ||
    (!starting.value && group.value === null)
  )
    return;
  if (group.value === null && performance.now() >= pendingLookupExpiresAt) {
    notice.value =
      "Still awaiting the start response; no active native group was confirmed. Refresh status or check History without replaying the request.";
    return;
  }
  const captured = context(),
    token = generation;
  timer = setTimeout(() => {
    timer = null;
    if (owns(captured, token)) void discover().finally(scheduleQuery);
  }, 1000);
}
async function discover(): Promise<void> {
  if (!readAvailable.value) return;
  const captured = context(),
    token = generation,
    query = ++queryEpoch;
  const requestId = operation?.requestId ?? group.value?.requestId ?? null;
  discovering.value = true;
  try {
    const response = await getAlternativesGroup(
      captured.scenarioId,
      captured.visibleRevision,
      requestId,
    );
    if (!owns(captured, token) || query !== queryEpoch) return;
    publish(response.result, captured);
  } catch (failure) {
    if (owns(captured, token) && query === queryEpoch)
      queryError.value = isRevisionConflict(failure) ? messages.setup.stale : safeMessage(failure);
  } finally {
    if (owns(captured, token) && query === queryEpoch) discovering.value = false;
    if (owns(captured, token)) scheduleQuery();
  }
}
async function loadBackends(): Promise<void> {
  const captured = context(),
    token = generation,
    query = ++backendEpoch;
  backendAvailable.value = false;
  backendError.value = null;
  if (!executionAvailable.value || !validationReady.value) {
    backendLoading.value = false;
    return;
  }
  backendLoading.value = true;
  try {
    const response = await getSolveBackendOptions(captured.scenarioId, captured.visibleRevision);
    if (!owns(captured, token) || query !== backendEpoch) return;
    if (
      response.result.scenarioId !== captured.scenarioId ||
      response.result.revision !== captured.visibleRevision
    )
      throw new Error(messages.setup.stale);
    backendAvailable.value = response.result.options.length > 0;
  } catch (failure) {
    if (owns(captured, token) && query === backendEpoch) backendError.value = safeMessage(failure);
  } finally {
    if (owns(captured, token) && query === backendEpoch) backendLoading.value = false;
  }
}
async function start(): Promise<void> {
  const maximum = maximumMs.value;
  const baselineSource = props.baselineSource;
  if (!canStart.value || maximum === undefined || operation !== null || baselineSource === null)
    return;
  const captured = context(),
    token = generation;
  const owned = new SetupOperationScope(captured.scenarioId, captured.visibleRevision);
  let request: AlternativesStartOperation;
  try {
    request = startAlternatives(owned, mode.value, maximum, baselineSource);
  } catch (failure) {
    owned.dispose();
    actionError.value = safeMessage(failure);
    return;
  }
  stopTimer();
  queryEpoch += 1;
  scope = owned;
  operation = request;
  group.value = null;
  clearChoices();
  starting.value = true;
  pendingLookupExpiresAt = performance.now() + 30_000;
  cancelling.value = false;
  notice.value = "Starting one native sequential Alternatives group…";
  actionError.value = null;
  scheduleQuery();
  try {
    const response = await request.result;
    if (!owns(captured, token) || operation !== request || !request.isCurrent()) return;
    // A pre-admission discovery response must not erase this settled native result.
    queryEpoch += 1;
    discovering.value = false;
    publish(response.result, captured);
    notice.value = null;
  } catch (failure) {
    if (!owns(captured, token) || operation !== request) return;
    actionError.value = safeMessage(failure);
    notice.value =
      "The start response was not confirmed. Looking up the exact native group; no solve is replayed.";
    await discover();
  } finally {
    void request.release().catch(() => undefined);
    owned.dispose();
    if (owns(captured, token) && operation === request) {
      operation = null;
      scope = null;
      starting.value = false;
      if (unref(group) === null)
        notice.value =
          "No retained native group summary is available. Check History for recorded runs; no automatic replay or continuation occurs.";
      scheduleQuery();
    }
  }
}
async function cancel(): Promise<void> {
  const shown = group.value;
  if (shown === null || shown.settled || shown.cancelOperationId === null || cancelling.value)
    return;
  const captured = context(),
    token = generation;
  cancelling.value = true;
  actionError.value = null;
  notice.value =
    "Cancellation requested; waiting for native settlement. Accepted earlier results remain saved.";
  try {
    const response = await cancelSolve({
      operationId: shown.cancelOperationId,
      scenarioId: captured.scenarioId,
      expectedRevision: shown.scenarioRevision,
    });
    if (!owns(captured, token) || group.value?.requestId !== shown.requestId) return;
    if (response.result.acknowledgement !== "cancellationRequested") cancelling.value = false;
    await discover();
  } catch (failure) {
    if (owns(captured, token) && group.value?.requestId === shown.requestId) {
      cancelling.value = false;
      actionError.value = safeMessage(failure);
    }
  }
}
async function readMember(
  member: WorkforceAlternativeMemberV1,
  captured: Context,
): Promise<Binding> {
  const outcome = member.outcome;
  if (
    outcome?.type !== "accepted" ||
    member.runId === null ||
    member.equivalence?.kind === "equivalent"
  )
    throw new Error("Choose an unsuppressed accepted native result.");
  const [detail, run] = await Promise.all([
    getSolutionSummary(captured.scenarioId, outcome.solutionId),
    getSolveJob(captured.scenarioId, member.runId),
  ]);
  const value = detail.result,
    job = run.result;
  if (
    value.scenarioId !== captured.scenarioId ||
    value.currentRevision !== captured.visibleRevision ||
    detail.currentRevision !== captured.visibleRevision ||
    value.scenarioRevision !== captured.visibleRevision ||
    value.result.solution.scenarioId !== captured.scenarioId ||
    value.result.solution.scenarioRevision !== value.scenarioRevision ||
    value.result.solution.solutionId !== outcome.solutionId ||
    value.result.checksum !== outcome.acceptedResultChecksum ||
    value.result.verification.checksum !== outcome.verificationChecksum ||
    !value.result.verification.accepted ||
    job.scenarioId !== captured.scenarioId ||
    job.runId !== member.runId ||
    job.requestId !== member.requestId ||
    job.scenarioRevision !== value.scenarioRevision ||
    job.solutionId !== outcome.solutionId ||
    job.verificationChecksum !== outcome.verificationChecksum ||
    (job.status !== "optimal" && job.status !== "feasible")
  )
    throw new Error(messages.setup.selectionChanged);
  return {
    result: { solutionId: outcome.solutionId, resultChecksum: value.result.checksum },
    scenarioRevision: value.scenarioRevision,
    runId: job.runId,
  };
}
async function choose(
  kind: "view" | "compare",
  trigger: HTMLButtonElement,
  member?: WorkforceAlternativeMemberV1,
): Promise<void> {
  if (!canChoose.value) return;
  const base = kind === "view" ? member : choices.value.find((item) => item.profile === left.value);
  const candidate = choices.value.find((item) => item.profile === right.value);
  if (!base || (kind === "compare" && (!candidate || candidate.profile === base.profile))) return;
  const captured = context(),
    token = generation,
    selection = ++choiceEpoch;
  readingChoice.value = true;
  actionError.value = null;
  try {
    const bindings = await Promise.all([
      readMember(base, captured),
      ...(kind === "compare" && candidate ? [readMember(candidate, captured)] : []),
    ]);
    if (
      !owns(captured, token) ||
      selection !== choiceEpoch ||
      !canChooseAfterRead() ||
      !stillSuggested(base) ||
      (kind === "compare" && (!candidate || !stillSuggested(candidate)))
    )
      return;
    const a = bindings[0];
    if (kind === "view") emit("view", { ...captured, result: a }, trigger);
    else {
      const b = bindings[1];
      if (b && candidate)
        emit(
          "compare",
          {
            ...captured,
            base: a,
            candidate: b,
            baseLabel: profileLabels[base.profile],
            candidateLabel: profileLabels[candidate.profile],
          },
          trigger,
        );
    }
  } catch (failure) {
    if (owns(captured, token) && selection === choiceEpoch)
      actionError.value = safeMessage(failure);
  } finally {
    if (owns(captured, token) && selection === choiceEpoch) readingChoice.value = false;
  }
}
function stillSuggested(member: WorkforceAlternativeMemberV1): boolean {
  const outcome = member.outcome;
  return (
    outcome?.type === "accepted" &&
    choices.value.some(
      (shown) =>
        shown.profile === member.profile &&
        shown.runId === member.runId &&
        shown.requestId === member.requestId &&
        shown.outcome?.type === "accepted" &&
        shown.outcome.solutionId === outcome.solutionId &&
        shown.outcome.acceptedResultChecksum === outcome.acceptedResultChecksum,
    )
  );
}
function canChooseAfterRead(): boolean {
  return !props.choiceUnavailable && props.authorityUnavailableReason === null;
}
function memberLabel(member: WorkforceAlternativeMemberV1): string {
  const labels = {
    queued: "Queued",
    preparing: "Preparing",
    running: "Running",
    finished: "Finished",
    failed: "Failed",
    skipped: "Skipped",
    outcomeUnknown: "Outcome unconfirmed",
  };
  return labels[member.status];
}
function reset(): void {
  generation += 1;
  queryEpoch += 1;
  backendEpoch += 1;
  stopTimer();
  scope?.dispose();
  scope = null;
  operation = null;
  group.value = null;
  discovered.value = false;
  discovering.value = false;
  starting.value = false;
  pendingLookupExpiresAt = 0;
  cancelling.value = false;
  queryError.value = null;
  actionError.value = null;
  notice.value = null;
  notifiedRequest = null;
  clearChoices();
}
watch(
  [
    () => props.home,
    () => props.home.state.libraryEpoch,
    () => props.project.scenarioId,
    () => props.project.revision,
  ],
  reset,
  { flush: "sync" },
);
watch(
  [
    () => props.home,
    () => props.home.state.libraryEpoch,
    () => props.project.scenarioId,
    () => props.project.revision,
    readAvailable,
  ],
  () => {
    if (!discovered.value) void discover();
  },
  { immediate: true },
);
watch(
  [
    () => props.home,
    () => props.home.state.libraryEpoch,
    () => props.project.scenarioId,
    () => props.project.revision,
    executionAvailable,
    validationReady,
  ],
  () => void loadBackends(),
  { immediate: true },
);
watch([left, right], () => {
  choiceEpoch += 1;
  readingChoice.value = false;
  emit("pairChanged");
});
watch(
  choices,
  (available) => {
    if (left.value && !available.some((member) => member.profile === left.value)) left.value = "";
    if (right.value && !available.some((member) => member.profile === right.value))
      right.value = "";
  },
  { flush: "sync" },
);
watch(
  () => group.value?.settled,
  async (settled) => {
    if (!settled) return;
    const captured = context(),
      token = generation;
    const restore =
      typeof document !== "undefined" && document.activeElement === cancelTrigger.value;
    cancelling.value = false;
    notice.value = null;
    await nextTick();
    if (owns(captured, token) && restore) {
      const target = canStart.value ? startTrigger.value : heading.value;
      if (target?.isConnected) target.focus();
    }
  },
);
onScopeDispose(() => {
  alive = false;
  reset();
});
</script>

<template>
  <section
    class="state-panel field-stack"
    aria-labelledby="setup-alternatives"
    data-alternatives-panel
    :data-alternatives-ready="canStart ? 'true' : 'false'"
  >
    <h3 id="setup-alternatives" ref="heading" tabindex="-1">Alternatives</h3>
    <p>
      Run Balanced → Preference-focused → Minimal-changes → Fairness-first sequentially from one
      explicitly selected starting plan and immutable source, within one total time limit.
    </p>
    <p>
      Profiles express different tradeoffs, not a shared ranking. Selecting a saved plan only views
      it; applying or adopting a base remains a separate reviewed action.
    </p>
    <p v-if="loading" role="status" aria-busy="true">
      {{ messages.setup.optimize.capabilityLoading }}
    </p>
    <div v-if="error" role="alert">
      <p>{{ error }}</p>
      <button type="button" @click="emit('retry')">Retry capability check</button>
    </div>
    <p v-if="!loading && !error && !executionAvailable" role="status">
      Native Alternatives execution is unavailable or unreported. This local workflow requires the
      bundled worker; it does not use a network service.
    </p>
    <p v-if="!readAvailable && !loading" role="status">
      Native group discovery is unavailable. Saved accepted results remain in History.
    </p>
    <p v-if="!validationReady">{{ messages.setup.optimize.validationRequired }}</p>
    <p v-if="!baselineAvailable" role="status">
      Select a current starting plan before Alternatives. An accepted base requires explicit
      reviewed adoption; an imported starting plan remains unverified.
    </p>
    <p v-if="authorityUnavailableReason" role="alert">{{ authorityUnavailableReason }}</p>
    <p v-if="backendLoading" role="status">Checking native backend availability…</p>
    <div v-if="backendError" role="alert">
      <p>{{ backendError }}</p>
      <button type="button" @click="loadBackends">Retry Alternatives backend check</button>
    </div>
    <p
      v-else-if="executionAvailable && validationReady && !backendLoading && !backendAvailable"
      role="status"
    >
      No native backend is available for this source. Saved results and History remain readable.
    </p>
    <fieldset :disabled="active">
      <legend>Alternatives effort and total time</legend>
      <label
        >Effort
        <select v-model="mode">
          <option value="quick">Quick</option>
          <option value="balanced">Balanced</option>
          <option value="deep">Deep</option>
        </select></label
      >
      <label for="alternatives-maximum-time">Total Alternatives time limit (seconds)</label>
      <input
        id="alternatives-maximum-time"
        v-model="maximumSeconds"
        type="number"
        min="1"
        max="30"
        step="1"
        inputmode="numeric"
        :aria-invalid="maximumMs === undefined"
        aria-describedby="alternatives-time-help"
      />
      <p id="alternatives-time-help">
        Leave blank for the native effort default, or enter 1–30 seconds shared by all four
        profiles. Effort does not change the named objective profiles.
      </p>
      <p v-if="maximumMs === undefined" role="alert">Enter a whole number from 1 to 30 seconds.</p>
    </fieldset>
    <div class="action-row">
      <button ref="startTrigger" type="button" :aria-disabled="!canStart" @click="start">
        Run Alternatives
      </button>
      <button
        v-if="group && !group.settled && group.cancelOperationId"
        ref="cancelTrigger"
        type="button"
        :aria-disabled="cancelling"
        @click="cancel"
      >
        Cancel Alternatives
      </button>
      <button type="button" :disabled="discovering || !readAvailable" @click="discover">
        Refresh Alternatives status
      </button>
      <RouterLink :to="{ name: 'project-history', params: { scenarioId: project.scenarioId } }"
        >Open Alternatives History</RouterLink
      >
    </div>
    <p v-if="discovering && !group" role="status" aria-busy="true">
      Looking up the native Alternatives group…
    </p>
    <p v-if="discovered && !group && !starting" role="status">
      No native group summary is retained for this context. After restart or summary eviction, use
      History; groups are never reconstructed or automatically continued.
    </p>
    <div v-if="queryError" role="alert">
      <p>{{ queryError }}</p>
      <p>Status could not be confirmed. No group is replayed. Refresh status or check History.</p>
    </div>
    <p v-if="actionError" role="alert">{{ actionError }}</p>
    <p v-if="notice" role="status" aria-live="polite">{{ notice }}</p>
    <section v-if="group" aria-label="Native Alternatives group status">
      <p role="status" aria-live="polite" aria-atomic="true">
        {{ group.settled ? "Settled" : "Active" }} ·
        {{
          group.snapshot?.status ??
          (group.settled ? "Profile summary unavailable" : "Awaiting native admission")
        }}
        · Source revision {{ group.scenarioRevision }}
      </p>
      <p>
        Group request <code>{{ group.requestId }}</code>
      </p>
      <p v-if="group.baselineSource.kind === 'external'">
        Group source selection — unverified starting plan:
        <code>{{ group.baselineSource.entityId }}</code
        >.
      </p>
      <p v-else>Group source selection — accepted/manual starting source.</p>
      <p v-if="group.snapshot?.stopReason" role="status">
        {{ stopLabels[group.snapshot.stopReason] }}
      </p>
      <p v-if="group.snapshot?.diagnosticCode">
        Native diagnostic: <code>{{ group.snapshot.diagnosticCode }}</code>
      </p>
      <p v-if="group.snapshot?.knownAlarm" role="alert">
        Known verification alarm: <code>{{ group.snapshot.knownAlarm.diagnosticCode }}</code
        >; terminalization {{ group.snapshot.knownAlarm.terminalization }}. Secondary failures do
        not clear this alarm.
      </p>
      <ol v-if="group.snapshot" aria-label="Alternatives profile outcomes">
        <li
          v-for="member in group.snapshot.members"
          :key="member.profile"
          :data-alternative-profile="member.profile"
        >
          <h4>{{ profileLabels[member.profile] }}</h4>
          <p role="status" aria-live="polite" aria-atomic="true">
            {{ profileLabels[member.profile] }}: {{ memberLabel(member)
            }}<template v-if="member.phase">
              · {{ messages.setup.optimize.phases[member.phase] }}</template
            >
          </p>
          <p v-if="member.outcome?.type === 'accepted'">
            Accepted · {{ member.outcome.status }} · <code>{{ member.outcome.solutionId }}</code>
          </p>
          <p v-else-if="member.outcome?.type === 'noResult'">
            No accepted result · {{ messages.setup.optimize.statuses[member.outcome.status] }}
          </p>
          <p v-else-if="member.outcome?.type === 'verificationAlarm'" role="alert">
            Verification alarm: <code>{{ member.outcome.diagnosticCode }}</code>
          </p>
          <p v-else-if="member.outcome?.type === 'interrupted'">
            Interrupted; no accepted result is established by this outcome.
          </p>
          <p v-if="member.finalizationUnknown || member.status === 'outcomeUnknown'" role="alert">
            Finalization unconfirmed. Reconcile this exact run in History; do not replay
            it.<template v-if="member.unconfirmedResultId">
              Unconfirmed result <code>{{ member.unconfirmedResultId }}</code> is not an accepted
              suggestion.</template
            >
          </p>
          <p v-if="member.knownAlarm" role="alert">
            Known verification alarm: <code>{{ member.knownAlarm.diagnosticCode }}</code
            >; terminalization {{ member.knownAlarm.terminalization
            }}<template v-if="member.knownAlarm.secondaryCauseCode"
              >; secondary cause <code>{{ member.knownAlarm.secondaryCauseCode }}</code></template
            >.
          </p>
          <p v-if="member.equivalence?.kind === 'equivalent'">
            Equivalent to {{ profileLabels[member.equivalence.profile] }} on exact native non-rank
            metrics and every configured per-person workload/preference outcome. Recorded in
            History, but not a distinct suggestion.
          </p>
          <p
            v-else-if="
              member.outcome?.type === 'accepted' && member.equivalence?.kind === 'unavailable'
            "
          >
            Equivalence metrics unavailable. This accepted result remains a distinct suggestion; no
            equality is inferred.
          </p>
          <p v-if="member.warningCode">
            Native warning: <code>{{ member.warningCode }}</code>
          </p>
          <p v-if="member.runId">
            Recorded run <code>{{ member.runId }}</code>
          </p>
          <button
            v-if="member.outcome?.type === 'accepted' && member.equivalence?.kind !== 'equivalent'"
            type="button"
            :aria-disabled="!canChoose"
            :aria-label="`View ${profileLabels[member.profile]} accepted alternative`"
            @click="choose('view', $event.currentTarget as HTMLButtonElement, member)"
          >
            View {{ profileLabels[member.profile] }} accepted alternative
          </button>
        </li>
      </ol>
      <p v-else role="status">
        {{
          group.settled
            ? "The native owner settled without a retained profile summary. Check History; no result or replay authority is inferred."
            : "Profile states are not yet reported by native admission."
        }}
        Expected order: {{ profiles.map((profile) => profileLabels[profile]).join(" → ") }}.
      </p>
      <p v-if="group.settled && choices.length === 0" role="status">
        No distinct accepted suggestions are available. Review all recorded outcomes in History.
      </p>
      <fieldset v-if="choices.length >= 2">
        <legend>Compare two accepted alternatives</legend>
        <label
          >First accepted alternative
          <select v-model="left" :disabled="!canChoose">
            <option value="">Choose a profile</option>
            <option v-for="member in choices" :key="member.profile" :value="member.profile">
              {{ profileLabels[member.profile] }}
            </option>
          </select></label
        >
        <label
          >Second accepted alternative
          <select v-model="right" :disabled="!canChoose">
            <option value="">Choose a profile</option>
            <option v-for="member in choices" :key="member.profile" :value="member.profile">
              {{ profileLabels[member.profile] }}
            </option>
          </select></label
        >
        <button
          type="button"
          :aria-disabled="!canChoose || !left || !right || left === right"
          @click="choose('compare', $event.currentTarget as HTMLButtonElement)"
        >
          Compare selected Alternatives
        </button>
      </fieldset>
      <p v-if="readingChoice" role="status" aria-busy="true">
        Reading the exact accepted native result and recorded run…
      </p>
      <p>
        Cross-profile utility is not inferred. When native score comparison is unavailable, accepted
        assignment and rule differences remain visible.
      </p>
    </section>
  </section>
</template>
