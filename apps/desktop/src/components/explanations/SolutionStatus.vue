<script setup lang="ts">
import { computed } from "vue";
import type {
  DomainEntityRef,
  ScoreVector,
  SolveStatus,
  VerificationWarning,
} from "../../api/generated";
import { Badge, Card } from "../ui";
import { explanationMessage, renderEvidenceTemplate } from "./messages";
import type { ExplanationUiState } from "./types";

interface Props {
  readonly accepted: boolean;
  readonly status: SolveStatus | null;
  readonly allRequiredRulesPassed: boolean;
  readonly score: ScoreVector | null;
  readonly warnings: readonly VerificationWarning[];
  readonly state: ExplanationUiState;
  readonly backend?: string | null;
  readonly elapsed?: number | null;
  readonly baseChangeCount?: number | null;
  readonly warningTexts?: Readonly<Record<string, string>>;
}

const props = withDefaults(defineProps<Props>(), {
  backend: null,
  elapsed: null,
  baseChangeCount: null,
  warningTexts: () => ({}),
});
const emit = defineEmits<{
  viewProof: [];
  viewRun: [];
}>();

const acceptedReady = computed(
  () =>
    props.state === "ready" &&
    props.accepted &&
    props.allRequiredRulesPassed &&
    (props.status === "optimal" || props.status === "feasible"),
);

const statusCopy = computed(() => {
  if (props.state !== "ready") return stateCopy.value;

  switch (props.status) {
    case "optimal":
      return acceptedReady.value ? explanationMessage("status.optimal") : stateCopy.value;
    case "feasible":
      return acceptedReady.value ? explanationMessage("status.feasible") : stateCopy.value;
    case "infeasible":
      return explanationMessage("status.infeasible");
    case "unbounded":
      return explanationMessage("status.unbounded");
    case "noSolutionWithinLimit":
      return explanationMessage("status.limit");
    case "cancelled":
      return explanationMessage("status.cancelled");
    case "backendUnavailable":
      return explanationMessage("status.unavailable");
    case "invalidModel":
      return explanationMessage("status.invalidModel");
    case "backendFailed":
      return explanationMessage("status.backendFailed");
    case null:
      return stateCopy.value;
  }
});

const stateCopy = computed(() => {
  switch (props.state) {
    case "ready":
      if (acceptedReady.value) return "Independent verification accepted this result.";
      if (props.status === "infeasible" || props.status === "unbounded") {
        return "This backend model outcome has no independently verified candidate or mapped original-rule conflict.";
      }
      return "No independently accepted result is available for selection.";
    case "empty":
      return "No solve result is available.";
    case "loading":
      return "The solve is still being checked. You can cancel it or return later.";
    case "stale":
      return explanationMessage("error.stale");
    case "cancelled":
      return explanationMessage("error.cancelled");
    case "inconclusive":
      return explanationMessage("error.inconclusive");
    case "unavailable":
      return explanationMessage("error.unavailable");
    case "internalFailure":
      return explanationMessage("error.internal");
  }
});

const stateRole = computed(() =>
  props.state === "internalFailure" || props.state === "stale" ? "alert" : "status",
);

const warningSummary = computed(() => {
  const count = props.warnings.length;
  return `${new Intl.NumberFormat().format(count)} verification ${count === 1 ? "warning" : "warnings"}`;
});

function warningText(warning: VerificationWarning): string {
  const template = props.warningTexts[warning.messageKey] ?? "A verification warning needs review.";
  return renderEvidenceTemplate(template, warning.facts);
}

function entityHref(entity: DomainEntityRef): string {
  return `#entity-${encodeURIComponent(entity.kind)}-${encodeURIComponent(entity.id)}`;
}
</script>

<template>
  <Card
    as="section"
    :variant="state === 'internalFailure' ? 'danger' : 'raised'"
    aria-labelledby="solution-status-heading"
  >
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div>
        <p class="text-sm font-bold text-muted">Solve result</p>
        <h2 id="solution-status-heading" class="font-display text-2xl font-semibold text-ink">
          {{ statusCopy }}
        </h2>
      </div>
      <Badge
        :variant="acceptedReady ? 'accent' : state === 'internalFailure' ? 'danger' : 'outline'"
      >
        <span aria-hidden="true">{{ acceptedReady ? "✓" : "!" }}</span>
        {{ acceptedReady ? "Accepted and verified" : "Unavailable for use" }}
      </Badge>
    </div>

    <p
      class="mt-3 text-sm text-muted"
      :role="stateRole"
      :aria-live="stateRole === 'status' ? 'polite' : undefined"
      aria-atomic="true"
    >
      {{ stateCopy }}
    </p>

    <dl v-if="acceptedReady || backend || elapsed != null" class="mt-4 grid gap-3 sm:grid-cols-2">
      <div v-if="acceptedReady">
        <dt class="text-sm font-bold text-muted">Required rules</dt>
        <dd class="text-ink">
          <span aria-hidden="true">✓</span>
          {{ explanationMessage("score.requiredPassed") }}
        </dd>
      </div>
      <div v-if="acceptedReady">
        <dt class="text-sm font-bold text-muted">Verified score</dt>
        <dd class="text-ink">
          {{
            score
              ? `${new Intl.NumberFormat().format(score.levels.length)} objective ${score.levels.length === 1 ? "level" : "levels"}`
              : explanationMessage("score.empty")
          }}
        </dd>
      </div>
      <div v-if="acceptedReady && baseChangeCount != null">
        <dt class="text-sm font-bold text-muted">Base-plan changes</dt>
        <dd class="text-ink">
          {{ new Intl.NumberFormat().format(baseChangeCount) }}
          {{ baseChangeCount === 1 ? "verified change" : "verified changes" }}
        </dd>
      </div>
      <div v-if="backend || elapsed != null">
        <dt class="text-sm font-bold text-muted">Run details</dt>
        <dd class="text-ink">
          <span v-if="backend">{{ explanationMessage("status.backend", { backend }) }}</span>
          <span v-if="backend && elapsed != null" aria-hidden="true"> · </span>
          <span v-if="elapsed != null">
            {{ explanationMessage("status.time", { milliseconds: elapsed }) }}
          </span>
        </dd>
      </div>
    </dl>

    <section
      v-if="acceptedReady && warnings.length"
      class="mt-4"
      aria-labelledby="verification-warnings-heading"
    >
      <h3 id="verification-warnings-heading" class="font-bold text-ink">
        <span aria-hidden="true">!</span> {{ warningSummary }}
      </h3>
      <ul class="mt-2 list-disc space-y-1 pl-5 text-sm text-muted">
        <li v-for="warning in warnings" :key="warning.id">
          <p>{{ warningText(warning) }}</p>
          <ul v-if="warning.affectedEntities.length" class="mt-1 flex flex-wrap gap-2">
            <li v-for="entity in warning.affectedEntities" :key="`${entity.kind}:${entity.id}`">
              <a
                :href="entityHref(entity)"
                class="font-bold text-accent-strong underline underline-offset-2"
              >
                {{ entity.kind }} {{ entity.id }}
              </a>
            </li>
          </ul>
        </li>
      </ul>
    </section>

    <div v-if="acceptedReady || backend || elapsed != null" class="mt-5 flex flex-wrap gap-3">
      <button
        v-if="acceptedReady"
        type="button"
        class="rounded-md border border-line-strong bg-raised px-4 py-2 font-bold text-ink"
        @click="emit('viewProof')"
      >
        {{ explanationMessage("action.viewProof") }}
      </button>
      <button
        v-if="backend || elapsed != null"
        type="button"
        class="rounded-md border border-line bg-surface px-4 py-2 font-bold text-ink"
        @click="emit('viewRun')"
      >
        {{ explanationMessage("action.viewRun") }}
      </button>
    </div>
  </Card>
</template>
