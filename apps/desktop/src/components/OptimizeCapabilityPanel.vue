<script setup lang="ts">
import { computed } from "vue";
import type {
  AppCapabilitiesDto,
  CatalogCommand,
  Revision,
  ScenarioSummaryV2,
} from "../api/generated";
import { messages } from "../messages";
import { isOptimizeValidationReady } from "../optimize-readiness";

const props = defineProps<{
  summary: Pick<ScenarioSummaryV2, "revision" | "fast" | "full">;
  currentRevision: Revision;
  capabilities: AppCapabilitiesDto | null;
  loading: boolean;
  error: string | null;
}>();
const emit = defineEmits<{ retry: [] }>();

type CapabilityStatus = "available" | "unavailable" | "unreported";

const validationReady = computed(() =>
  isOptimizeValidationReady(props.summary, props.currentRevision),
);
const handoffReady = computed(
  () =>
    validationReady.value && !props.loading && props.error === null && props.capabilities !== null,
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
const executionStatus = computed<"unavailable" | "reported" | "unreported">(() => {
  if (solveStartStatus.value === "unavailable") return "unavailable";
  if (solveStartStatus.value === "available" || solveCancelStatus.value === "available")
    return "reported";
  return "unreported";
});
function statusLabel(status: CapabilityStatus): string {
  return status === "available"
    ? messages.setup.optimize.available
    : status === "unavailable"
      ? messages.setup.optimize.unavailable
      : messages.setup.optimize.unreported;
}
</script>

<template>
  <section
    class="state-panel field-stack"
    aria-labelledby="setup-optimize"
    data-optimize-panel
    :data-optimize-ready="handoffReady ? 'true' : 'false'"
  >
    <h3 id="setup-optimize" tabindex="-1">{{ messages.setup.optimize.heading }}</h3>
    <p>{{ messages.setup.optimize.description }}</p>
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
    <template v-else-if="!validationReady">
      <p data-optimize-state="validation-not-ready">
        {{ messages.setup.optimize.validationRequired }}
      </p>
    </template>
    <template v-else>
      <p role="status" data-optimize-state="ready">{{ messages.setup.optimize.ready }}</p>
      <dl class="metadata-list">
        <div>
          <dt>{{ messages.setup.optimize.solveStart }}</dt>
          <dd>{{ statusLabel(solveStartStatus) }}</dd>
        </div>
        <div>
          <dt>{{ messages.setup.optimize.solveCancel }}</dt>
          <dd>{{ statusLabel(solveCancelStatus) }}</dd>
        </div>
      </dl>
      <p v-if="executionStatus === 'unavailable'">
        {{ messages.setup.optimize.executionUnavailable }}
      </p>
      <p v-else-if="executionStatus === 'reported'">
        {{ messages.setup.optimize.executionReported }}
      </p>
      <p v-else>{{ messages.setup.optimize.executionUnreported }}</p>
      <h4>{{ messages.setup.optimize.modesHeading }}</h4>
      <dl class="metadata-list">
        <div>
          <dt>{{ messages.setup.optimize.quick }}</dt>
          <dd>{{ messages.setup.optimize.quickDescription }}</dd>
        </div>
        <div>
          <dt>{{ messages.setup.optimize.balanced }}</dt>
          <dd>{{ messages.setup.optimize.balancedDescription }}</dd>
        </div>
        <div>
          <dt>{{ messages.setup.optimize.deep }}</dt>
          <dd>{{ messages.setup.optimize.deepDescription }}</dd>
        </div>
      </dl>
      <p>{{ messages.setup.optimize.modeBoundary }}</p>
    </template>
  </section>
</template>
