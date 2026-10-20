<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type {
  AssignmentEvidenceV1,
  AssignmentValue,
  CounterfactualResult,
  MetricValue,
} from "../../api/generated";
import { Badge, Card } from "../ui";
import { explanationMessage } from "./messages";
import type { ExplanationUiState } from "./types";

type DiagnosticState = ExplanationUiState | "idle" | "limit";

interface Props {
  readonly evidence: AssignmentEvidenceV1 | null;
  readonly entityLabel: string;
  readonly workLabel: string;
  readonly eligibility: string;
  readonly availability: string;
  readonly preferencesHelped: readonly string[];
  readonly preferencesHurt: readonly string[];
  readonly fairnessContribution: string;
  readonly stabilityContribution: string;
  readonly state: ExplanationUiState;
  readonly diagnosticState: DiagnosticState;
  readonly diagnosticResult?: CounterfactualResult | null;
}

const props = withDefaults(defineProps<Props>(), {
  diagnosticResult: null,
});
const emit = defineEmits<{
  whyThis: [];
  whyNot: [];
  tryChange: [];
  cancelDiagnostic: [];
}>();
const whyNotButton = ref<HTMLButtonElement | null>(null);

watch(
  () => props.diagnosticState,
  async (state, previous) => {
    if (previous === "loading" && state !== "loading") {
      await nextTick();
      whyNotButton.value?.focus({ preventScroll: true });
    }
  },
);

function formatInteger(value: string | number): string {
  try {
    return new Intl.NumberFormat().format(typeof value === "string" ? BigInt(value) : value);
  } catch {
    return String(value);
  }
}

function assignmentValue(value: AssignmentValue): string {
  switch (value.type) {
    case "boolean":
      return value.value ? "Yes" : "No";
    case "integer":
      return formatInteger(value.value);
    case "interval":
      return `${formatInteger(value.value.start)}–${formatInteger(value.value.end)} (duration ${formatInteger(value.value.duration)})`;
    case "absent":
      return "Absent";
  }
}

function metricValue(value: MetricValue): string {
  if (value.type === "integer") return formatInteger(value.value);
  return `${formatInteger(value.value.numerator)} / ${formatInteger(value.value.denominator)}`;
}

function signed(value: string | number): string {
  const text = String(value);
  if (text.startsWith("-")) return `−${formatInteger(text.slice(1))}`;
  if (text === "0") return "0";
  return `+${formatInteger(value)}`;
}

const stateCopy = computed(() => {
  switch (props.state) {
    case "ready":
      return props.evidence ? null : explanationMessage("assignment.empty");
    case "empty":
      return explanationMessage("assignment.empty");
    case "loading":
      return "Assignment evidence is being loaded. Return when verification finishes.";
    case "stale":
      return explanationMessage("error.stale");
    case "cancelled":
      return "Assignment inspection was cancelled. Select the assignment again to retry.";
    case "inconclusive":
      return explanationMessage("error.inconclusive");
    case "unavailable":
      return explanationMessage("error.unavailable");
    case "internalFailure":
      return explanationMessage("error.internal");
  }
});

const diagnosticCopy = computed(() => {
  switch (props.diagnosticState) {
    case "idle":
      return "A short diagnostic optimization may run. It uses a temporary condition and does not change the scenario.";
    case "ready":
      switch (props.diagnosticResult?.conclusion.type) {
        case "provenImpossible":
          return "The alternative was proven impossible under the temporary condition.";
        case "verifiedAlternative":
          return `An independently verified alternative was found and is ${props.diagnosticResult.conclusion.ordering} than the base result.`;
        case "notDistinguishedWithinBudget":
          return "The diagnostic budget did not distinguish the alternatives.";
        case undefined:
          return "No verified diagnostic result is available.";
      }
    case "empty":
      return "No Why not…? diagnostic has been started.";
    case "loading":
      return explanationMessage("counterfactual.progress");
    case "stale":
      return explanationMessage("counterfactual.stale");
    case "cancelled":
      return explanationMessage("counterfactual.cancelled");
    case "limit":
      return explanationMessage("counterfactual.limit");
    case "inconclusive":
      return explanationMessage("counterfactual.inconclusive");
    case "unavailable":
      return explanationMessage("counterfactual.unavailable");
    case "internalFailure":
      return "The diagnostic stopped because internal verification failed. No candidate is available for use.";
  }
});

const diagnosticRole = computed(() =>
  props.diagnosticState === "internalFailure" ||
  props.diagnosticState === "stale" ||
  props.diagnosticState === "unavailable"
    ? "alert"
    : "status",
);
</script>

<template>
  <aside aria-labelledby="assignment-inspector-heading">
    <Card>
      <h2 id="assignment-inspector-heading" class="font-display text-xl font-semibold text-ink">
        {{ explanationMessage("assignment.heading") }}
      </h2>

      <div
        v-if="stateCopy"
        class="mt-4 rounded-md border border-line bg-surface p-4 text-sm text-muted"
        :role="state === 'stale' || state === 'internalFailure' ? 'alert' : 'status'"
        :aria-live="state === 'loading' ? 'polite' : undefined"
      >
        {{ stateCopy }}
      </div>

      <template v-else-if="evidence">
        <dl class="mt-4 grid gap-4 sm:grid-cols-2">
          <div>
            <dt class="text-sm font-bold text-muted">Assigned entity</dt>
            <dd class="text-ink">{{ entityLabel }}</dd>
          </div>
          <div>
            <dt class="text-sm font-bold text-muted">Assigned work</dt>
            <dd class="text-ink">{{ workLabel }}</dd>
          </div>
          <div>
            <dt class="text-sm font-bold text-muted">
              {{ explanationMessage("assignment.eligibility") }}
            </dt>
            <dd class="text-ink">{{ eligibility }}</dd>
          </div>
          <div>
            <dt class="text-sm font-bold text-muted">
              {{ explanationMessage("assignment.availability") }}
            </dt>
            <dd class="text-ink">{{ availability }}</dd>
          </div>
          <div>
            <dt class="text-sm font-bold text-muted">Assignment value</dt>
            <dd class="text-ink">{{ assignmentValue(evidence.assignment.value) }}</dd>
          </div>
          <div>
            <dt class="text-sm font-bold text-muted">Lock state</dt>
            <dd class="text-ink">
              <Badge :variant="evidence.lockState?.state === 'locked' ? 'accent' : 'outline'">
                <span aria-hidden="true">
                  {{
                    evidence.lockState === null
                      ? "?"
                      : evidence.lockState.state === "locked"
                        ? "■"
                        : "○"
                  }}
                </span>
                {{
                  evidence.lockState === null
                    ? "Lock information unavailable"
                    : evidence.lockState.state === "locked"
                      ? explanationMessage("assignment.locked")
                      : explanationMessage("assignment.unlocked")
                }}
              </Badge>
              <span v-if="evidence.lockState?.state === 'locked'" class="ml-2">
                {{ assignmentValue(evidence.lockState.value) }}
              </span>
            </dd>
          </div>
        </dl>

        <section class="mt-5" aria-labelledby="assignment-rules-heading">
          <h3 id="assignment-rules-heading" class="font-bold text-ink">
            {{ explanationMessage("assignment.rules") }}
          </h3>
          <ul v-if="evidence.relatedRules.length" class="mt-2 space-y-2">
            <li
              v-for="rule in evidence.relatedRules"
              :key="rule.ruleId"
              class="rounded-md border border-line bg-surface p-3"
            >
              <p class="font-bold text-ink">
                <span aria-hidden="true">{{ rule.satisfied ? "✓" : "✕" }}</span>
                {{ rule.satisfied ? "Passed" : "Failed" }} · {{ rule.ruleId }}
              </p>
              <p class="text-sm text-muted">{{ rule.messageKey }}</p>
            </li>
          </ul>
          <p v-else class="mt-2 text-sm text-muted">
            No related required rule checks were recorded.
          </p>
        </section>

        <section class="mt-5" aria-labelledby="assignment-preferences-heading">
          <h3 id="assignment-preferences-heading" class="font-bold text-ink">
            {{ explanationMessage("assignment.preferences") }}
          </h3>
          <div class="mt-2 grid gap-4 sm:grid-cols-2">
            <div>
              <h4 class="text-sm font-bold text-ink"><span aria-hidden="true">+</span> Helped</h4>
              <ul v-if="preferencesHelped.length" class="mt-1 list-disc pl-5 text-sm text-muted">
                <li v-for="item in preferencesHelped" :key="item">{{ item }}</li>
              </ul>
              <p v-else class="mt-1 text-sm text-muted">No preferences helped.</p>
            </div>
            <div>
              <h4 class="text-sm font-bold text-ink"><span aria-hidden="true">−</span> Hurt</h4>
              <ul v-if="preferencesHurt.length" class="mt-1 list-disc pl-5 text-sm text-muted">
                <li v-for="item in preferencesHurt" :key="item">{{ item }}</li>
              </ul>
              <p v-else class="mt-1 text-sm text-muted">No preferences hurt.</p>
            </div>
          </div>
        </section>

        <section class="mt-5" aria-labelledby="assignment-fairness-heading">
          <h3 id="assignment-fairness-heading" class="font-bold text-ink">
            {{ explanationMessage("assignment.fairness") }}
          </h3>
          <dl class="mt-2 grid gap-3 sm:grid-cols-2">
            <div>
              <dt class="text-sm font-bold text-muted">Fairness contribution</dt>
              <dd class="text-ink">{{ fairnessContribution }}</dd>
            </div>
            <div>
              <dt class="text-sm font-bold text-muted">Stability contribution</dt>
              <dd class="text-ink">{{ stabilityContribution }}</dd>
            </div>
          </dl>
          <ul v-if="evidence.scoreContributions.length" class="mt-3 space-y-1 text-sm text-muted">
            <li v-for="contribution in evidence.scoreContributions" :key="contribution.evidenceId">
              <span>{{ contribution.levelId }}</span>
              <span v-if="contribution.categoryId"> / {{ contribution.categoryId }}</span>
              <span>:</span>
              <span class="font-mono text-ink">{{ signed(contribution.value) }}</span>
            </li>
          </ul>
          <dl v-if="Object.keys(evidence.metrics).length" class="mt-3 space-y-1 text-sm">
            <div
              v-for="(value, metricId) in evidence.metrics"
              :key="metricId"
              class="flex justify-between gap-4"
            >
              <dt class="text-muted">{{ metricId }}</dt>
              <dd class="font-mono text-ink">{{ metricValue(value) }}</dd>
            </div>
          </dl>
        </section>

        <div class="mt-5 flex flex-wrap gap-3">
          <button
            type="button"
            class="rounded-md border border-line-strong bg-raised px-4 py-2 font-bold text-ink"
            @click="emit('whyThis')"
          >
            {{ explanationMessage("action.whyThis") }}
          </button>
          <button
            ref="whyNotButton"
            type="button"
            class="rounded-md border border-line-strong bg-raised px-4 py-2 font-bold text-ink"
            @click="emit('whyNot')"
          >
            {{ explanationMessage("action.whyNot") }}
          </button>
          <button
            type="button"
            class="rounded-md border border-line bg-surface px-4 py-2 font-bold text-ink"
            @click="emit('tryChange')"
          >
            {{ explanationMessage("action.tryChange") }}
          </button>
        </div>

        <details class="mt-4 rounded-md border border-line bg-surface p-3">
          <summary class="cursor-pointer font-bold text-ink">
            About {{ explanationMessage("action.whyNot") }}
          </summary>
          <p class="mt-2 text-sm text-muted">
            A short diagnostic optimization may run. It uses a temporary condition and does not
            change the scenario.
          </p>
        </details>

        <div
          class="mt-3 rounded-md border border-line bg-surface p-3 text-sm text-muted"
          :role="diagnosticRole"
          :aria-live="diagnosticRole === 'status' ? 'polite' : undefined"
          aria-atomic="true"
        >
          <p>{{ diagnosticCopy }}</p>
          <button
            v-if="diagnosticState === 'loading'"
            type="button"
            class="mt-3 rounded-md border border-line-strong bg-raised px-3 py-2 font-bold text-ink"
            @click="emit('cancelDiagnostic')"
          >
            {{ explanationMessage("action.cancel") }} diagnostic
          </button>
        </div>
      </template>
    </Card>
  </aside>
</template>
