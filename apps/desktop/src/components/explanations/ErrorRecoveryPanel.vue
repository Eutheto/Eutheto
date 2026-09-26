<script setup lang="ts">
import { computed, useId } from "vue";

import type { ApiErrorDto } from "../../api/generated";
import { Badge, Card } from "../ui";
import { explanationMessage } from "./messages";
import type { ExplanationUiState } from "./types";

const props = defineProps<{
  error: ApiErrorDto | null;
  state: ExplanationUiState;
  actions: readonly ("retry" | "exportDiagnostic" | "dismiss")[];
}>();

const emit = defineEmits<{
  retry: [];
  exportDiagnostic: [];
  dismiss: [];
}>();

const headingId = useId();
const canAct = computed(() => props.state !== "loading" && props.state !== "empty");
const canRetry = computed(
  () => canAct.value && props.actions.includes("retry") && props.error?.retryable === true,
);
const canExport = computed(
  () =>
    canAct.value &&
    props.actions.includes("exportDiagnostic") &&
    props.error?.diagnosticId !== null &&
    props.error?.diagnosticId !== undefined,
);
const canDismiss = computed(() => canAct.value && props.actions.includes("dismiss"));
const announcementRole = computed(() =>
  props.error || props.state === "stale" || props.state === "internalFailure" ? "alert" : "status",
);
const summary = computed(() => {
  if (props.state === "loading") return explanationMessage("error.loading");
  if (props.state === "stale") return explanationMessage("error.stale");
  if (props.state === "cancelled") return explanationMessage("error.cancelled");
  if (props.state === "inconclusive") return explanationMessage("error.inconclusive");
  if (props.state === "unavailable") return explanationMessage("error.unavailable");
  if (props.state === "internalFailure") return explanationMessage("error.internal");
  if (props.state === "empty" || !props.error) {
    return explanationMessage("explanation.empty");
  }
  return props.error.message;
});
</script>

<template>
  <Card
    as="section"
    :variant="state === 'internalFailure' ? 'danger' : 'surface'"
    :aria-labelledby="headingId"
  >
    <h2 :id="headingId" class="font-display text-lg font-bold text-ink">
      {{ explanationMessage("error.heading") }}
    </h2>
    <div
      :role="announcementRole"
      :aria-live="announcementRole === 'alert' ? 'assertive' : 'polite'"
      aria-atomic="true"
    >
      <p class="mt-2 text-sm font-semibold text-ink">{{ summary }}</p>
    </div>

    <template v-if="error">
      <p v-if="summary !== error.message" class="mt-2 text-sm text-muted">{{ error.message }}</p>
      <p class="mt-2 text-xs text-muted">
        Error code: <code class="font-mono text-ink">{{ error.code }}</code>
      </p>

      <section v-if="error.fieldErrors.length" class="mt-4" aria-label="Fields needing attention">
        <h3 class="font-semibold text-ink">Fields needing attention</h3>
        <ul class="mt-2 space-y-2">
          <li
            v-for="fieldError in error.fieldErrors"
            :key="`${fieldError.field}:${fieldError.code}`"
            class="rounded-md border border-line bg-raised p-3 text-sm"
          >
            <p class="font-semibold text-ink">{{ fieldError.field }}</p>
            <p class="text-muted">{{ fieldError.message }}</p>
            <p class="mt-1 font-mono text-xs text-muted">{{ fieldError.code }}</p>
          </li>
        </ul>
      </section>

      <p v-if="error.diagnosticId" class="mt-4 text-xs text-muted">
        <Badge variant="outline">Diagnostic ID</Badge>
        <code class="ml-2 font-mono text-ink">{{ error.diagnosticId }}</code>
      </p>
    </template>

    <div v-if="canRetry || canExport || canDismiss" class="mt-4 flex flex-wrap gap-2">
      <button
        v-if="canRetry"
        type="button"
        class="rounded-md bg-accent px-3 py-2 text-sm font-bold text-raised focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
        @click="emit('retry')"
      >
        {{ explanationMessage("action.retry") }}
      </button>
      <button
        v-if="canExport"
        type="button"
        class="rounded-md border border-line-strong px-3 py-2 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
        @click="emit('exportDiagnostic')"
      >
        {{ explanationMessage("action.exportDiagnostic") }}
      </button>
      <button
        v-if="canDismiss"
        type="button"
        class="rounded-md border border-line-strong px-3 py-2 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
        @click="emit('dismiss')"
      >
        {{ explanationMessage("action.dismiss") }}
      </button>
    </div>
  </Card>
</template>
