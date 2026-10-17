<script setup lang="ts">
import { computed, nextTick, onScopeDispose, reactive, ref, shallowRef, watch } from "vue";
import {
  SetupOperationScope,
  type RecipientCreatedDtoV1,
  type RecipientPreviewDtoV1,
  type Revision,
  type SetupOperation,
  type SolutionSummaryDtoV1,
  type UuidV7,
} from "../api/generated";
import type { RecipientReportOptionsV1 } from "../api/generated-recipient-report";
import { messages } from "../messages";
import {
  isOperationCancelled,
  safeMessage,
  type ProjectHomeController,
  type WorkspaceOperationState,
} from "../project-home";
import { mountRecipientReport } from "../recipient-report";
import "../recipient-report.css";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./ui/dialog";

const props = defineProps<{
  home: ProjectHomeController;
  scenarioId: UuidV7;
  summary: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision"> &
    Partial<Pick<SolutionSummaryDtoV1, "verificationChecksum" | "status">>;
  currentRevision: Revision;
}>();
const copy = messages.recipientShare;
const options = reactive({
  includePersonNames: false,
  includeAuthoredLabels: false,
  includeStaffing: false,
  includeChanges: false,
  includeFairnessActuals: false,
});
const optionKeys = Object.keys(copy.options) as (keyof RecipientReportOptionsV1)[];
const open = ref(false);
const pending = ref<"preview" | "create" | null>(null);
const activeOperation = shallowRef<WorkspaceOperationState | null>(null);
const cleaning = ref(false);
const review = shallowRef<RecipientPreviewDtoV1 | null>(null);
const capturedEpoch = ref<number | null>(null);
const allowStale = ref(false);
const notice = ref<string>(copy.initial);
const error = ref<string | null>(null);
const cleanupError = ref<string | null>(null);
const trigger = ref<HTMLButtonElement | null>(null);
const heading = ref<HTMLElement | null>(null);
const reportRoot = ref<HTMLElement | null>(null);
const unavailable = computed(() => props.home.state.phase !== "ready");
const busy = computed(
  () => pending.value !== null || cleaning.value || props.home.state.busyAction !== null,
);
const staleSource = computed(() => {
  const source = review.value?.report.source;
  return source !== undefined && source.scenarioRevision !== source.observedCurrentRevision;
});
let alive = true;
let generation = 0;
let disposeReport: (() => void) | null = null;
let expiryTimer: ReturnType<typeof setTimeout> | null = null;
let expiresAt = 0;
let cleanup: Promise<boolean> = Promise.resolve(true);
let focusCleanup: Promise<boolean> = cleanup;
let focusGeneration = 0;

type Owner = {
  readonly home: ProjectHomeController;
  readonly scope: SetupOperationScope;
  preview: SetupOperation<RecipientPreviewDtoV1> | null;
  create: SetupOperation<RecipientCreatedDtoV1> | null;
  previewId: UuidV7 | null;
  previewSettled: Promise<void>;
  dispose(): Promise<void>;
};
let owner: Owner | null = null;
let failedOwner: Owner | null = null;

function newOwner(home: ProjectHomeController, scenarioId: UuidV7, head: Revision): Owner {
  let retiring: Promise<void> | null = null;
  const owned: Owner = {
    home,
    scope: new SetupOperationScope(scenarioId, head),
    preview: null,
    create: null,
    previewId: null,
    previewSettled: Promise.resolve(),
    dispose() {
      // Disposal detaches observation, not cancellation. The exact owner survives root cleanup retries.
      owned.scope.dispose();
      retiring ??= (async () => {
        const operationId = await owned.preview?.operationId.catch(() => null);
        if (owned.previewId !== null) {
          await owned.scope.releaseRecipientReport({ kind: "preview", previewId: owned.previewId });
        } else if (operationId) {
          await owned.scope.releaseRecipientReport({ kind: "operation", operationId });
        }
        // A closing acknowledgement is not settlement. A late response adopts its immutable ID here.
        await owned.previewSettled;
        await owned.create?.result.catch(() => undefined);
        if (owned.previewId !== null) {
          await owned.scope.releaseRecipientReport({ kind: "preview", previewId: owned.previewId });
        } else if (operationId) {
          await owned.scope.releaseRecipientReport({ kind: "operation", operationId });
        }
      })().finally(() => {
        retiring = null;
      });
      return retiring;
    },
  };
  home.registerReviewOwner(owned);
  return owned;
}

function clearReport(): void {
  if (expiryTimer !== null) clearTimeout(expiryTimer);
  expiryTimer = null;
  disposeReport?.();
  disposeReport = null;
  reportRoot.value?.replaceChildren();
  review.value = null;
  capturedEpoch.value = null;
  allowStale.value = false;
}

function retire(): Promise<boolean> {
  const previous = owner ?? failedOwner;
  owner = null;
  failedOwner = null;
  clearReport();
  if (!previous) return cleanup;
  cleaning.value = true;
  const retirement = cleanup
    .then(async () => {
      const released = await previous.home.retireReviewOwner(previous);
      if (!released) failedOwner = previous;
      if (alive) cleanupError.value = released ? null : messages.operations.cleanupFailed;
      return released;
    })
    .finally(() => {
      if (alive && cleanup === retirement) cleaning.value = false;
    });
  cleanup = retirement;
  return cleanup;
}

async function retryCleanup(): Promise<void> {
  if (cleaning.value) return;
  const previous = failedOwner;
  if (!previous) {
    await props.home.retryReviewCleanup();
    return;
  }
  cleaning.value = true;
  try {
    const released = await previous.home.retireReviewOwner(previous);
    if (released && failedOwner === previous) {
      failedOwner = null;
      cleanup = Promise.resolve(true);
      cleanupError.value = null;
    }
  } finally {
    if (alive) cleaning.value = false;
  }
}

function invalidate(message: string): Promise<boolean> {
  generation += 1;
  notice.value = message;
  error.value = null;
  return retire();
}

function canPrepare(): boolean {
  return alive && open.value && !unavailable.value && !props.home.state.reviewCleanupError;
}

async function prepare(): Promise<void> {
  if (!canPrepare() || busy.value) return;
  const token = ++generation;
  error.value = null;
  notice.value = copy.preparing;
  const cleaned = await retire();
  if (!cleaned || !canPrepare() || token !== generation) return;
  const home = props.home;
  const scenarioId = props.scenarioId;
  const solutionId = props.summary.result.solutionId;
  const checksum = props.summary.result.resultChecksum;
  const sourceRevision = props.summary.scenarioRevision;
  const verificationChecksum = props.summary.verificationChecksum;
  const head = props.currentRevision;
  const epoch = home.state.libraryEpoch;
  const disclosure = { ...options };
  const current = () =>
    alive &&
    token === generation &&
    open.value &&
    props.home === home &&
    home.state.phase === "ready" &&
    home.state.libraryEpoch === epoch &&
    props.scenarioId === scenarioId &&
    props.currentRevision === head &&
    props.summary.result.solutionId === solutionId &&
    props.summary.result.resultChecksum === checksum &&
    props.summary.scenarioRevision === sourceRevision &&
    props.summary.verificationChecksum === verificationChecksum;
  let owned: Owner | null = null;
  pending.value = "preview";
  const started = performance.now();
  try {
    owned = newOwner(home, scenarioId, head);
    owner = owned;
    const captured = owned;
    const response = await home.runOperation({
      action: "recipient-share-preview",
      label: copy.preparing,
      refreshLibrary: false,
      cancel: async () => {
        await captured.preview?.cancel();
      },
      execute: async (report) => {
        activeOperation.value = home.state.operation;
        const operation = captured.scope.previewRecipientReport(solutionId, disclosure, report);
        captured.preview = operation;
        captured.previewSettled = operation.result.then(
          ({ result }) => {
            captured.previewId = result.previewId;
          },
          () => undefined,
        );
        return operation.result;
      },
      success: () => copy.ready,
    });
    if (!current() || owner !== owned || !owned.preview?.isCurrent()) {
      await home.retireReviewOwner(owned);
      return;
    }
    const preview = response.result;
    const source = preview.report.source;
    if (
      source.scenarioId !== scenarioId ||
      source.solutionId !== solutionId ||
      source.acceptedResultChecksum !== checksum ||
      source.scenarioRevision !== sourceRevision ||
      source.observedCurrentRevision !== head ||
      (verificationChecksum !== undefined &&
        source.verificationReportChecksum !== verificationChecksum) ||
      optionKeys.some((key) => preview.report.privacy[key] !== disclosure[key])
    ) {
      throw new Error(copy.mismatch);
    }
    // A conservative UI deadline never extends the native ten-minute approval lifetime.
    expiresAt = started + preview.expiresAfterMilliseconds;
    if (performance.now() >= expiresAt) {
      await invalidate(copy.expired);
      return;
    }
    review.value = preview;
    capturedEpoch.value = epoch;
    await nextTick();
    if (!current() || owner !== owned || review.value !== preview || !owned.preview.isCurrent())
      return;
    const root = reportRoot.value;
    if (!root) throw new Error(copy.renderFailed);
    try {
      disposeReport = mountRecipientReport(root, preview.report);
    } catch {
      throw new Error(copy.renderFailed);
    }
    notice.value =
      preview.report.content.schedule.assignments.length === 0 ? copy.empty : copy.ready;
    expiryTimer = setTimeout(
      () => {
        if (current() && owner === captured) void invalidate(copy.expired);
      },
      Math.max(0, expiresAt - performance.now()),
    );
    // Ready review custody is separate from routine operation-observer release.
    void owned.preview.release().catch(() => undefined);
  } catch (failure) {
    if (current()) {
      error.value =
        failure instanceof Error &&
        (failure.message === copy.mismatch || failure.message === copy.renderFailed)
          ? failure.message
          : isOperationCancelled(failure)
            ? null
            : safeMessage(failure);
      notice.value = isOperationCancelled(failure) ? copy.cancelled : copy.initial;
      await retire();
    } else if (owned) {
      await home.retireReviewOwner(owned);
    }
  } finally {
    if (alive) pending.value = null;
  }
}

async function save(): Promise<void> {
  const preview = review.value;
  const owned = owner;
  if (
    !preview ||
    !owned ||
    busy.value ||
    unavailable.value ||
    (staleSource.value && !allowStale.value)
  )
    return;
  if (performance.now() >= expiresAt) {
    await invalidate(copy.expired);
    return;
  }
  const token = generation;
  const epoch = capturedEpoch.value;
  const staleConsent = allowStale.value;
  const current = () =>
    alive &&
    token === generation &&
    open.value &&
    props.home === owned.home &&
    owned.home.state.phase === "ready" &&
    owned.home.state.libraryEpoch === epoch &&
    props.scenarioId === preview.report.source.scenarioId &&
    props.currentRevision === preview.report.source.observedCurrentRevision &&
    props.summary.result.solutionId === preview.report.source.solutionId &&
    props.summary.result.resultChecksum === preview.report.source.acceptedResultChecksum &&
    props.summary.scenarioRevision === preview.report.source.scenarioRevision &&
    (props.summary.verificationChecksum === undefined ||
      props.summary.verificationChecksum === preview.report.source.verificationReportChecksum);
  if (!current()) {
    await invalidate(copy.stale);
    return;
  }
  pending.value = "create";
  notice.value = copy.saving;
  error.value = null;
  // One-shot approval is retired before dispatch, including cancellation, failed and unknown outcomes.
  clearReport();
  try {
    const response = await owned.home.runOperation({
      action: "recipient-share-create",
      label: copy.saving,
      refreshLibrary: false,
      cancel: async () => {
        await owned.create?.cancel();
      },
      execute: async (report) => {
        activeOperation.value = owned.home.state.operation;
        owned.create = owned.scope.createRecipientReport(preview.previewId, staleConsent, report);
        return owned.create.result;
      },
      success: (result) => copy.saved(result.filename),
    });
    const publication = response.result.publication;
    if (
      publication.solutionId !== preview.report.source.solutionId ||
      publication.sourceRevision !== preview.report.source.scenarioRevision ||
      publication.currentRevision !== preview.report.source.observedCurrentRevision ||
      publication.modelSha256 !== preview.modelSha256 ||
      publication.htmlSha256 !== preview.htmlSha256
    ) {
      if (current()) notice.value = copy.unknown;
    } else if (current() && owned.create?.isCurrent()) {
      notice.value = copy.saved(response.result.filename);
    }
  } catch (failure) {
    if (current()) {
      if (isOperationCancelled(failure)) notice.value = copy.cancelled;
      else if (
        typeof failure === "object" &&
        failure !== null &&
        "code" in failure &&
        failure.code === "recipient.preview_not_active"
      ) {
        notice.value = copy.expired;
      } else if (
        typeof failure === "object" &&
        failure !== null &&
        "code" in failure &&
        [
          "recipient.preview_binding_mismatch",
          "recipient.extension_invalid",
          "recipient.selection_invalid",
        ].includes(String(failure.code))
      ) {
        notice.value = copy.failed;
        error.value = safeMessage(failure);
      } else {
        notice.value = copy.unknown;
        error.value = safeMessage(failure);
      }
    }
  } finally {
    if (owner === owned) await retire();
    else await owned.home.retireReviewOwner(owned);
    if (alive) pending.value = null;
  }
}

function show(): void {
  if (busy.value || unavailable.value) return;
  focusGeneration = generation;
  open.value = true;
}
function close(): void {
  focusCleanup = invalidate(copy.initial);
  focusGeneration = generation;
  open.value = false;
}
async function focusDialog(event: Event): Promise<void> {
  event.preventDefault();
  const token = generation;
  await nextTick();
  if (alive && open.value && token === generation && heading.value?.isConnected)
    heading.value.focus();
}
async function restoreFocus(event: Event): Promise<void> {
  event.preventDefault();
  const token = focusGeneration;
  const home = props.home;
  const button = trigger.value;
  await focusCleanup;
  await nextTick();
  if (
    alive &&
    !open.value &&
    token === generation &&
    props.home === home &&
    button === trigger.value &&
    button?.isConnected
  )
    button.focus();
}
watch(
  () => [
    props.home,
    props.scenarioId,
    props.summary.result.solutionId,
    props.summary.result.resultChecksum,
    props.summary.scenarioRevision,
    props.summary.verificationChecksum,
    props.currentRevision,
    props.home.state.libraryEpoch,
    props.home.state.phase,
    props.home.state.changeSignal,
  ],
  () => {
    void invalidate(copy.stale);
  },
  { flush: "sync" },
);
watch(
  options,
  () => {
    void invalidate(copy.optionsChanged);
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  alive = false;
  generation += 1;
  void retire();
});
</script>

<template>
  <button
    ref="trigger"
    type="button"
    class="button-secondary"
    :disabled="busy || unavailable"
    @click="show"
  >
    {{ copy.trigger }}
  </button>
  <Dialog :open="open" @update:open="(value) => !value && close()">
    <DialogContent
      class="recipient-share-dialog max-w-7xl"
      @open-auto-focus="focusDialog"
      @close-auto-focus="restoreFocus"
      @interact-outside.prevent
    >
      <DialogTitle
        ><span ref="heading" tabindex="-1">{{ copy.title }}</span></DialogTitle
      >
      <DialogDescription>{{ copy.description }}</DialogDescription>
      <p>{{ copy.format }}</p>
      <p>{{ copy.offline }}</p>
      <p>{{ copy.privacy }}</p>
      <p>{{ copy.optional }}</p>
      <fieldset :disabled="busy">
        <legend>{{ copy.disclosures }}</legend>
        <label v-for="key in optionKeys" :key="key" class="choice-row">
          <input v-model="options[key]" type="checkbox" />
          <span>{{ copy.options[key] }}</span>
        </label>
      </fieldset>
      <p v-if="unavailable" role="status">{{ copy.unavailable }}</p>
      <p role="status" :aria-busy="pending !== null || cleaning">
        {{ cleaning ? copy.cleaning : notice }}
      </p>
      <p v-if="error" class="inline-alert" role="alert">{{ error }}</p>
      <div v-if="cleanupError || home.state.reviewCleanupError" class="inline-alert" role="alert">
        <p>{{ cleanupError || home.state.reviewCleanupError }}</p>
        <button
          type="button"
          :disabled="cleaning || home.state.retryingReviewCleanup"
          @click="retryCleanup"
        >
          {{ messages.operations.retryCleanup }}
        </button>
      </div>
      <section v-if="review" :aria-label="copy.source">
        <h3>{{ copy.source }}</h3>
        <dl class="recipient-share-source">
          <dt>{{ copy.solution }}</dt>
          <dd>{{ review.report.source.solutionId }}</dd>
          <dt>{{ copy.checksum }}</dt>
          <dd>{{ review.report.source.acceptedResultChecksum }}</dd>
          <dt>{{ copy.verificationChecksum }}</dt>
          <dd>{{ review.report.source.verificationReportChecksum }}</dd>
          <dt>{{ copy.sourceRevision }}</dt>
          <dd>{{ review.report.source.scenarioRevision }}</dd>
          <dt>{{ copy.head }}</dt>
          <dd>{{ review.report.source.observedCurrentRevision }}</dd>
          <dt>{{ copy.libraryRevision }}</dt>
          <dd>{{ review.libraryRevision }}</dd>
          <dt>{{ copy.libraryEpoch }}</dt>
          <dd>{{ capturedEpoch }}</dd>
        </dl>
      </section>
      <div v-if="review" ref="reportRoot" />
      <template v-if="review && staleSource">
        <p>{{ copy.staleSource }}</p>
        <label class="choice-row">
          <input v-model="allowStale" type="checkbox" :disabled="busy" />
          <span>{{ copy.consent }}</span>
        </label>
      </template>
      <div class="action-row">
        <button type="button" class="button-secondary" @click="close">{{ copy.close }}</button>
        <button
          v-if="
            pending &&
            home.state.operation === activeOperation &&
            home.state.operation?.cancel &&
            !home.state.operation.settled
          "
          type="button"
          class="button-secondary"
          :disabled="home.state.operation.cancellationRequested"
          @click="home.cancelOperation()"
        >
          {{ messages.operations.cancel }}
        </button>
        <button
          v-if="!review"
          type="button"
          :disabled="busy || unavailable || !!home.state.reviewCleanupError"
          @click="prepare"
        >
          {{ copy.preview }}
        </button>
        <button
          v-else
          type="button"
          :disabled="busy || unavailable || (staleSource && !allowStale)"
          @click="save"
        >
          {{ copy.save }}
        </button>
      </div>
    </DialogContent>
  </Dialog>
</template>

<style scoped>
.recipient-share-source {
  overflow-wrap: anywhere;
}
.recipient-share-source dt {
  font-weight: 700;
}
.recipient-share-source dd {
  margin: 0 0 0.5rem;
}
</style>
