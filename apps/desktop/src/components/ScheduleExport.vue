<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import {
  SetupOperationScope,
  type ScheduleExportCreatedDtoV1,
  type ScheduleExportFormatV1,
  type ScheduleExportPreviewDtoV1,
  type Revision,
  type SetupOperation,
  type SolutionSummaryDtoV1,
  type UuidV7,
} from "../api/generated";
import { messages } from "../messages";
import {
  isOperationCancelled,
  safeMessage,
  type ProjectHomeController,
  type WorkspaceOperationState,
} from "../project-home";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./ui/dialog";

const props = defineProps<{
  home: ProjectHomeController;
  scenarioId: UuidV7;
  summary: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision"> &
    Partial<Pick<SolutionSummaryDtoV1, "verificationChecksum" | "status">>;
  currentRevision: Revision;
}>();
const copy = messages.scheduleExport;
const sourceCopy = messages.recipientShare;
const formats: readonly ScheduleExportFormatV1[] = ["assignmentsCsv", "summaryCsv", "ics"];
const format = ref<ScheduleExportFormatV1>("assignmentsCsv");
const person = shallowRef<{ readonly id: UuidV7; readonly label: string } | null>(null);
const open = ref(false);
const pending = ref<"preview" | "create" | null>(null);
const activeOperation = shallowRef<WorkspaceOperationState | null>(null);
const cleaning = ref(false);
const review = shallowRef<ScheduleExportPreviewDtoV1 | null>(null);
const capturedEpoch = ref<number | null>(null);
const allowStale = ref(false);
const notice = ref<string>(copy.initial);
const error = ref<string | null>(null);
const cleanupError = ref<string | null>(null);
const toolbarTrigger = ref<HTMLButtonElement | null>(null);
const heading = ref<HTMLElement | null>(null);
const unavailable = computed(() => props.home.state.phase !== "ready");
const busy = computed(
  () => pending.value !== null || cleaning.value || props.home.state.busyAction !== null,
);
const staleSource = computed(() => {
  const source = review.value?.preview.source;
  return source !== undefined && source.scenarioRevision !== source.observedCurrentRevision;
});
let trigger: HTMLButtonElement | null = null;
let alive = true;
let generation = 0;
let expiryTimer: ReturnType<typeof setTimeout> | null = null;
let expiresAt = 0;
let cleanup: Promise<boolean> = Promise.resolve(true);
let focusCleanup: Promise<boolean> = cleanup;
let focusGeneration = 0;

// The parent mounts this surface only with native preview/create/cancel/release capability evidence.
type Owner = {
  readonly home: ProjectHomeController;
  readonly scope: SetupOperationScope;
  preview: SetupOperation<ScheduleExportPreviewDtoV1> | null;
  create: SetupOperation<ScheduleExportCreatedDtoV1> | null;
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
      // Detach observation without cancelling; keep this exact owner for cleanup retries.
      owned.scope.dispose();
      retiring ??= (async () => {
        const operationId = await owned.preview?.operationId.catch(() => null);
        const release = async () => {
          if (owned.previewId !== null) {
            await owned.scope.releaseScheduleExport({
              kind: "preview",
              previewId: owned.previewId,
            });
          } else if (operationId) {
            await owned.scope.releaseScheduleExport({ kind: "operation", operationId });
          }
        };
        await release();
        // Closing is not settlement. Adopt a late native preview ID before final retirement.
        await owned.previewSettled;
        await owned.create?.result.catch(() => undefined);
        await release();
      })().finally(() => {
        retiring = null;
      });
      return retiring;
    },
  };
  home.registerReviewOwner(owned);
  return owned;
}

function clearReview(): void {
  if (expiryTimer !== null) clearTimeout(expiryTimer);
  expiryTimer = null;
  review.value = null;
  capturedEpoch.value = null;
  allowStale.value = false;
}

function retire(): Promise<boolean> {
  const previous = owner ?? failedOwner;
  owner = null;
  failedOwner = null;
  clearReview();
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
  const requestedFormat = format.value;
  const personId = person.value?.id ?? null;
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
    props.summary.verificationChecksum === verificationChecksum &&
    format.value === requestedFormat &&
    (person.value?.id ?? null) === personId;
  let owned: Owner | null = null;
  pending.value = "preview";
  const started = performance.now();
  try {
    owned = newOwner(home, scenarioId, head);
    owner = owned;
    const captured = owned;
    const response = await home.runOperation({
      action: "schedule-export-preview",
      label: copy.preparing,
      refreshLibrary: false,
      cancel: async () => {
        await captured.preview?.cancel();
      },
      execute: async (report) => {
        activeOperation.value = home.state.operation;
        const operation = captured.scope.previewScheduleExport(
          solutionId,
          requestedFormat,
          personId,
          report,
        );
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
    const source = preview.preview.source;
    if (
      source.scenarioId !== scenarioId ||
      source.solutionId !== solutionId ||
      source.acceptedResultChecksum !== checksum ||
      source.scenarioRevision !== sourceRevision ||
      source.observedCurrentRevision !== head ||
      (verificationChecksum !== undefined &&
        source.verificationReportChecksum !== verificationChecksum) ||
      preview.preview.format !== requestedFormat ||
      preview.preview.personId !== personId
    ) {
      throw new Error(copy.mismatch);
    }
    // Starting before native preparation conservatively shortens, never extends, review lifetime.
    expiresAt = started + preview.expiresAfterMilliseconds;
    if (performance.now() >= expiresAt) {
      await invalidate(copy.expired);
      return;
    }
    review.value = preview;
    capturedEpoch.value = epoch;
    notice.value = copy.ready;
    expiryTimer = setTimeout(
      () => {
        if (current() && owner === captured) void invalidate(copy.expired);
      },
      Math.max(0, expiresAt - performance.now()),
    );
    // Ready artifact custody is distinct from operation-observer release.
    void owned.preview.release().catch(() => undefined);
  } catch (failure) {
    if (current()) {
      error.value =
        failure instanceof Error && failure.message === copy.mismatch
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
    props.home.state.reviewCleanupError ||
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
  const artifact = preview.preview;
  const source = artifact.source;
  const current = () =>
    alive &&
    token === generation &&
    open.value &&
    props.home === owned.home &&
    owned.home.state.phase === "ready" &&
    owned.home.state.libraryEpoch === epoch &&
    props.scenarioId === source.scenarioId &&
    props.currentRevision === source.observedCurrentRevision &&
    props.summary.result.solutionId === source.solutionId &&
    props.summary.result.resultChecksum === source.acceptedResultChecksum &&
    props.summary.scenarioRevision === source.scenarioRevision &&
    (props.summary.verificationChecksum === undefined ||
      props.summary.verificationChecksum === source.verificationReportChecksum) &&
    format.value === artifact.format &&
    (person.value?.id ?? null) === artifact.personId;
  if (!current()) {
    await invalidate(copy.stale);
    return;
  }
  pending.value = "create";
  notice.value = copy.saving;
  error.value = null;
  // Retire one-shot approval before dispatch: failed, cancelled and unknown outcomes cannot replay.
  clearReview();
  try {
    const response = await owned.home.runOperation({
      action: "schedule-export-create",
      label: copy.saving,
      refreshLibrary: false,
      cancel: async () => {
        await owned.create?.cancel();
      },
      execute: async (report) => {
        activeOperation.value = owned.home.state.operation;
        owned.create = owned.scope.createScheduleExport(preview, staleConsent, report);
        return owned.create.result;
      },
      success: (result) => copy.saved(result.filename),
    });
    const publication = response.result.publication;
    const confirmed =
      response.result.previewId === preview.previewId &&
      publication.solutionId === source.solutionId &&
      publication.sourceRevision === source.scenarioRevision &&
      publication.currentRevision === source.observedCurrentRevision &&
      publication.format === artifact.format &&
      publication.personId === artifact.personId &&
      publication.sha256 === artifact.sha256 &&
      publication.byteCount === artifact.byteCount;
    // A confirmed publication stays successful even if head/epoch/cancellation changed meanwhile.
    if (alive && open.value && props.home === owned.home) {
      notice.value = confirmed ? copy.saved(response.result.filename) : copy.unknown;
      error.value = null;
    }
  } catch (failure) {
    if (current()) {
      if (isOperationCancelled(failure)) notice.value = copy.cancelled;
      else {
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

function show(
  selected: { readonly id: UuidV7; readonly label: string } | null,
  button: HTMLButtonElement | null,
): void {
  if (!alive || open.value || busy.value || unavailable.value) return;
  person.value = selected;
  if (selected !== null) format.value = "ics";
  trigger = button;
  focusGeneration = generation;
  open.value = true;
}
function openForPerson(personId: UuidV7, label: string, button: HTMLButtonElement | null): void {
  show({ id: personId, label }, button);
}
defineExpose({ openForPerson });

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
  const button = trigger;
  await focusCleanup;
  await nextTick();
  if (
    alive &&
    !open.value &&
    token === generation &&
    props.home === home &&
    button === trigger &&
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
  [format, person],
  () => {
    void invalidate(copy.formatChanged);
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
    ref="toolbarTrigger"
    type="button"
    class="button-secondary"
    :disabled="busy || unavailable"
    @click="show(null, toolbarTrigger)"
  >
    {{ copy.trigger }}
  </button>
  <Dialog :open="open" @update:open="(value) => !value && close()">
    <DialogContent
      class="schedule-export-dialog max-w-4xl"
      @open-auto-focus="focusDialog"
      @close-auto-focus="restoreFocus"
      @interact-outside.prevent
    >
      <DialogTitle
        ><span ref="heading" tabindex="-1">{{ copy.title }}</span></DialogTitle
      >
      <DialogDescription>{{ copy.description }}</DialogDescription>
      <p>{{ person ? copy.personScope(person.label) : copy.wholeSource }}</p>
      <p v-if="person">{{ copy.personId }}: {{ person.id }}</p>
      <p v-if="person">{{ copy.emptyPerson }}</p>
      <fieldset :disabled="busy || person !== null">
        <legend>{{ copy.format }}</legend>
        <label v-for="choice in formats" :key="choice" class="choice-row">
          <input
            v-model="format"
            type="radio"
            :name="`schedule-export-format-${summary.result.solutionId}`"
            :value="choice"
          />
          <span>{{ copy.formats[choice] }}</span>
        </label>
      </fieldset>
      <p>{{ copy.disclosures[format] }}</p>
      <p>{{ copy.privacy }}</p>
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
      <section v-if="review" :aria-label="sourceCopy.source">
        <h3>{{ sourceCopy.source }}</h3>
        <dl class="schedule-export-source">
          <dt>{{ sourceCopy.solution }}</dt>
          <dd>{{ review.preview.source.solutionId }}</dd>
          <dt>{{ sourceCopy.checksum }}</dt>
          <dd>{{ review.preview.source.acceptedResultChecksum }}</dd>
          <dt>{{ sourceCopy.verificationChecksum }}</dt>
          <dd>{{ review.preview.source.verificationReportChecksum }}</dd>
          <dt>{{ sourceCopy.sourceRevision }}</dt>
          <dd>{{ review.preview.source.scenarioRevision }}</dd>
          <dt>{{ sourceCopy.head }}</dt>
          <dd>{{ review.preview.source.observedCurrentRevision }}</dd>
          <dt>{{ sourceCopy.libraryRevision }}</dt>
          <dd>{{ review.preview.libraryRevision }}</dd>
          <dt>{{ sourceCopy.libraryEpoch }}</dt>
          <dd>{{ capturedEpoch }}</dd>
          <dt>{{ copy.byteCount }}</dt>
          <dd>{{ review.preview.byteCount }}</dd>
          <dt>{{ copy.sha256 }}</dt>
          <dd>{{ review.preview.sha256 }}</dd>
        </dl>
        <h3>{{ copy.sample }}</h3>
        <p>{{ copy.sampleDescription }}</p>
        <p>{{ review.preview.sampleTruncated ? copy.truncated : copy.sampleComplete }}</p>
        <pre
          v-if="review.preview.sample"
          tabindex="0"
          :aria-label="copy.sample"
          class="schedule-export-sample"
          >{{ review.preview.sample }}</pre>
        <p v-else>{{ copy.emptySample }}</p>
      </section>
      <template v-if="review && staleSource">
        <p role="status">{{ copy.staleSource }}</p>
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
          :disabled="
            busy || unavailable || !!home.state.reviewCleanupError || (staleSource && !allowStale)
          "
          @click="save"
        >
          {{ copy.save }}
        </button>
      </div>
    </DialogContent>
  </Dialog>
</template>

<style scoped>
.schedule-export-source {
  overflow-wrap: anywhere;
}
.schedule-export-source dt {
  font-weight: 700;
}
.schedule-export-source dd {
  margin: 0 0 0.5rem;
}
.schedule-export-sample {
  max-height: 18rem;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
</style>
