<script setup lang="ts">
import { computed, useId } from "vue";

import type { FastFindingsV1, ValidationIssue } from "../../api/generated";
import { formatNumber } from "../../messages";
import { Badge, Card } from "../ui";
import { explanationMessage } from "./messages";
import type { ExplanationUiState } from "./types";

const props = withDefaults(
  defineProps<{
    findings: FastFindingsV1;
    state: ExplanationUiState;
    interaction: "static" | "selectable";
    presentation?: "panel" | "embedded";
    heading?: string;
    headingLevel?: 2 | 3 | 4;
    locale?: string | undefined;
    selectedIssue?: ValidationIssue | null;
  }>(),
  {
    presentation: "panel",
    heading: explanationMessage("validation.heading"),
    headingLevel: 2,
    locale: undefined,
    selectedIssue: null,
  },
);

const emit = defineEmits<{
  selectIssue: [issue: ValidationIssue];
}>();

const headingId = useId();
const total = computed(
  () =>
    props.findings.counts.errors +
    props.findings.counts.warnings +
    props.findings.counts.information,
);
const countText = computed(() =>
  explanationMessage(
    "validation.findingsCount",
    {
      total: total.value,
      displayed: props.findings.issues.length,
      omitted: props.findings.omitted,
    },
    props.locale,
  ),
);
const showsIssues = computed(() => props.state === "ready" && props.findings.issues.length > 0);
const stateText = computed(() => {
  if (props.state === "ready" || props.state === "empty") {
    return explanationMessage("validation.empty");
  }
  if (props.state === "loading") return explanationMessage("validation.loading");
  if (props.state === "stale") return explanationMessage("error.stale");
  if (props.state === "cancelled") return explanationMessage("error.cancelled");
  if (props.state === "inconclusive") return explanationMessage("error.inconclusive");
  if (props.state === "unavailable") return explanationMessage("error.unavailable");
  return explanationMessage("error.internal");
});

function severityVariant(issue: ValidationIssue): "danger" | "accent" | "neutral" {
  if (issue.severity === "error") return "danger";
  if (issue.severity === "warning") return "accent";
  return "neutral";
}
</script>

<template>
  <component
    :is="presentation === 'panel' ? Card : 'section'"
    :as="presentation === 'panel' ? 'section' : undefined"
    :variant="presentation === 'panel' ? 'surface' : undefined"
    :aria-labelledby="headingId"
  >
    <component
      :is="`h${headingLevel.toString()}`"
      :id="headingId"
      class="font-display text-lg font-bold text-ink"
    >
      {{ heading }}
    </component>

    <p
      v-if="state === 'ready' && total > 0"
      class="mt-2 text-sm text-muted"
      role="status"
      aria-live="polite"
      aria-atomic="true"
    >
      {{ countText }}
    </p>
    <p v-else class="mt-2 text-sm text-muted" role="status" aria-live="polite">
      {{ stateText }}
    </p>
    <dl v-if="state === 'ready'" class="metadata-list">
      <div>
        <dt>{{ explanationMessage("validation.errors") }}</dt>
        <dd>{{ formatNumber(findings.counts.errors, locale) }}</dd>
      </div>
      <div>
        <dt>{{ explanationMessage("validation.warnings") }}</dt>
        <dd>{{ formatNumber(findings.counts.warnings, locale) }}</dd>
      </div>
      <div>
        <dt>{{ explanationMessage("validation.information") }}</dt>
        <dd>{{ formatNumber(findings.counts.information, locale) }}</dd>
      </div>
    </dl>

    <ul v-if="showsIssues" class="mt-4 space-y-3" aria-label="Validation issues">
      <li v-for="(issue, index) in findings.issues" :key="`${issue.code}:${index}`">
        <component
          :is="interaction === 'selectable' ? 'button' : 'div'"
          :type="interaction === 'selectable' ? 'button' : undefined"
          class="w-full rounded-md border border-line bg-raised p-3 text-left text-ink"
          :aria-current="
            interaction === 'selectable' && selectedIssue === issue ? 'true' : undefined
          "
          :aria-label="
            interaction === 'selectable'
              ? `Select ${issue.severity} validation issue: ${issue.message}`
              : undefined
          "
          @click="interaction === 'selectable' && emit('selectIssue', issue)"
        >
          <span class="flex flex-wrap items-center gap-2">
            <Badge :variant="severityVariant(issue)">{{ issue.severity }}</Badge>
            <span class="font-mono text-xs text-muted">{{ issue.code }}</span>
            <Badge v-if="interaction === 'selectable' && selectedIssue === issue" variant="outline"
              >Selected</Badge
            >
          </span>
          <span class="mt-2 block text-sm font-semibold">{{ issue.message }}</span>
          <span v-if="issue.fieldPath" class="mt-1 block text-xs text-muted">
            Field: {{ issue.fieldPath }}
          </span>
          <span v-if="issue.resource" class="mt-1 block text-xs text-muted">
            Affected {{ issue.resource.type }}: {{ issue.resource.id }}
          </span>
        </component>
      </li>
    </ul>
  </component>
</template>
