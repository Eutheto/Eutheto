<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import {
  getScenarioHistoryPage,
  type HistoryPageContinuationV1,
  type HistoryPageDtoV1,
} from "../api/generated";
import {
  isRevisionConflict,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";
import UndoHistory from "./planner/UndoHistory.vue";
import { plannerMessage } from "./planner/messages";

const props = withDefaults(
  defineProps<{
    home: ProjectHomeController;
    project: ProjectSummary;
    locale?: string | undefined;
  }>(),
  { locale: undefined },
);
const emit = defineEmits<{ undo: []; redo: [] }>();
const page = shallowRef<HistoryPageDtoV1 | null>(null);
const loading = ref(false);
const refreshing = ref(false);
const olderPage = ref(false);
const error = ref<string | null>(null);
const status = ref<HTMLElement>();
let current = true;
let generation = 0;
// One automatic library refresh per consecutive conflict run; no retry loop.
let mayRefreshConflict = true;
const busy = computed(() => props.home.state.busyAction !== null);
const mutationReason = computed(() =>
  busy.value
    ? plannerMessage("history.busy")
    : props.project.archived
      ? plannerMessage("history.archived")
      : props.project.domainPackId !== "official.workforce"
        ? plannerMessage("history.unsupported")
        : null,
);
const announcement = computed(() =>
  busy.value
    ? plannerMessage("history.busy")
    : loading.value || refreshing.value
      ? plannerMessage("history.loading")
      : (error.value ??
        (page.value
          ? plannerMessage("history.revision", { revision: page.value.revision }, props.locale)
          : "")),
);

async function load(continuation: HistoryPageContinuationV1 | null = null): Promise<void> {
  const captured = ++generation;
  const active = () =>
    current && captured === generation && !busy.value && props.home.state.phase === "ready";
  page.value = null;
  error.value = null;
  loading.value = false;
  if (!active()) return;
  loading.value = true;
  // Invalidate synchronously, but coalesce epoch/prop publication before starting native work.
  await nextTick();
  if (!active()) return;
  const { scenarioId, revision } = props.project;
  const epoch = props.home.state.libraryEpoch;
  const matches = () =>
    active() &&
    props.project.scenarioId === scenarioId &&
    props.project.revision === revision &&
    props.home.state.libraryEpoch === epoch;
  try {
    const result = await getScenarioHistoryPage({
      schemaVersion: 1,
      scenarioId,
      expectedRevision: revision,
      limit: 50,
      continuation,
    });
    if (!matches()) return;
    if (result.result.scenarioId !== scenarioId || result.result.revision !== revision) {
      error.value = plannerMessage("history.stale");
      return;
    }
    page.value = result.result;
    olderPage.value = continuation !== null;
    mayRefreshConflict = true;
  } catch (failure) {
    if (!matches()) return;
    error.value = isRevisionConflict(failure)
      ? plannerMessage("history.stale")
      : safeMessage(failure);
    if (isRevisionConflict(failure) && mayRefreshConflict) {
      mayRefreshConflict = false;
      await props.home.refreshLibrary();
    }
  } finally {
    if (captured === generation) loading.value = false;
  }
}
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
    () => props.home.state.busyAction,
    () => props.home.state.phase,
  ],
  () => void load(),
  { immediate: true, flush: "sync" },
);
onScopeDispose(() => {
  current = false;
  generation += 1;
});
async function focusStatus(): Promise<void> {
  await nextTick();
  if (current) status.value?.focus();
}
async function refresh(): Promise<void> {
  if (busy.value || refreshing.value || loading.value) return;
  mayRefreshConflict = true;
  refreshing.value = true;
  const epoch = props.home.state.libraryEpoch;
  try {
    if ((await props.home.refreshLibrary()) && current && props.home.state.libraryEpoch === epoch)
      void load();
  } finally {
    if (current) refreshing.value = false;
    await focusStatus();
  }
}
function change(kind: "undo" | "redo"): void {
  const snapshot = page.value;
  if (
    snapshot === null ||
    mutationReason.value !== null ||
    snapshot.scenarioId !== props.project.scenarioId ||
    snapshot.revision !== props.project.revision ||
    !(kind === "undo" ? snapshot.undoAvailable : snapshot.redoAvailable)
  )
    return;
  generation += 1;
  page.value = null;
  if (kind === "undo") emit("undo");
  else emit("redo");
  void focusStatus();
}
function navigate(older: boolean): void {
  if (busy.value || loading.value || page.value === null) return;
  const continuation = older ? page.value.continuation : null;
  if (older && continuation === null) return;
  void load(continuation);
  void focusStatus();
}
</script>

<template>
  <section class="page-stack" aria-labelledby="history-heading">
    <h2 id="history-heading" data-route-heading tabindex="-1">
      {{ plannerMessage("history.title") }}
    </h2>
    <p>{{ plannerMessage("history.help") }}</p>
    <button
      type="button"
      class="button-secondary"
      :disabled="busy || loading || refreshing"
      @click="refresh"
    >
      {{ plannerMessage("history.refresh") }}
    </button>
    <p
      ref="status"
      role="status"
      tabindex="-1"
      :aria-busy="loading || refreshing"
      :class="{ 'text-danger': error !== null }"
    >
      {{ announcement }}
    </p>
    <UndoHistory
      v-if="page"
      :page="page"
      :older-page="olderPage"
      :disabled-reason="mutationReason"
      :locale="locale"
      @undo="change('undo')"
      @redo="change('redo')"
      @older="navigate(true)"
      @newest="navigate(false)"
    />
  </section>
</template>
