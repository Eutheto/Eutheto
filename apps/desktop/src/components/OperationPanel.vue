<script setup lang="ts">
import { computed, onScopeDispose, ref, shallowRef, watch } from "vue";
import { messages } from "../messages";
import type { WorkspaceOperationState } from "../project-home";
import {
  OperationPresentationScheduler,
  operationPresentationAnnouncementIsUrgent,
  operationPresentationText,
} from "../operation-presentation";

const props = defineProps<{
  readonly operation: WorkspaceOperationState | null;
}>();
const emit = defineEmits<{
  cancel: [];
}>();

const visibleOperation = shallowRef<WorkspaceOperationState | null>(null);
const liveAnnouncement = ref("");
const trackedOperation = shallowRef<WorkspaceOperationState | null>(null);
const labels = messages.operations;
const currentText = computed(() => {
  const operation = visibleOperation.value;
  return operation ? operationPresentationText(operation, labels) : "";
});

function announceCurrent(operation: WorkspaceOperationState): void {
  scheduler.announce(
    operation,
    operationPresentationText(operation, labels),
    operationPresentationAnnouncementIsUrgent(operation),
  );
}

const scheduler = new OperationPresentationScheduler({
  onDisplay: (operation) => {
    visibleOperation.value = operation;
    announceCurrent(operation);
  },
  onHide: () => {
    visibleOperation.value = null;
    liveAnnouncement.value = "";
  },
  onAnnouncement: (text) => {
    liveAnnouncement.value = text;
  },
});

watch(
  () =>
    [
      props.operation,
      props.operation?.phase,
      props.operation?.cancellationRequested,
      props.operation?.settled,
      props.operation?.refreshingLibrary,
    ] as const,
  ([operation]) => {
    if (operation !== trackedOperation.value) {
      trackedOperation.value = operation ?? null;
      scheduler.replace(operation ?? null);
    }
    if (operation) announceCurrent(operation);
  },
  { immediate: true, flush: "sync" },
);

onScopeDispose(() => {
  scheduler.dispose();
});
</script>

<template>
  <p class="sr-only" role="status" aria-live="polite" aria-atomic="true">
    {{ liveAnnouncement }}
  </p>
  <section
    v-if="visibleOperation"
    class="state-panel operation-panel"
    aria-labelledby="operation-label"
  >
    <div>
      <h2 id="operation-label">{{ visibleOperation.label }}</h2>
      <p class="operation-phase" role="status" aria-live="off" aria-atomic="true">
        {{ currentText }}
      </p>
    </div>
    <button
      v-if="visibleOperation.cancel && !visibleOperation.settled"
      type="button"
      class="button-secondary"
      :disabled="visibleOperation.cancellationRequested"
      @click="emit('cancel')"
    >
      {{ messages.operations.cancel }}
    </button>
  </section>
</template>
