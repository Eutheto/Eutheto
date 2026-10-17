<script setup lang="ts">
import { computed, ref, useId, watch } from "vue";

import type {
  AssignmentValue,
  CounterfactualConditionV1,
  EvidenceMessageV1,
  InfeasibilityEvidenceV1,
  InfeasibilityExplanationContextV1,
  Revision,
  SolveRunId,
  UuidV7,
} from "../../api/generated";
import { Badge, Card } from "../ui";
import { explanationMessage } from "./messages";
import type { ExplanationUiState } from "./types";

const props = defineProps<{
  scenarioId: UuidV7;
  sourceRevision: Revision;
  headingLevel: 6 | 7;
  currentRevision: Revision;
  evidence: InfeasibilityEvidenceV1 | null;
  messages: readonly EvidenceMessageV1[];
  condition: CounterfactualConditionV1 | null;
  context: InfeasibilityExplanationContextV1 | null;
  ruleNavigationAvailable: boolean;
  diagnosticActionsAvailable: boolean;
  copyableGroupIds: readonly string[];
  actionButtonsDisabled: boolean;
  state: ExplanationUiState;
  errorMessage: string | null;
  cancelRequested: boolean;
}>();

const emit = defineEmits<{
  ruleAction: [
    action: {
      readonly action: "inspect" | "edit";
      readonly scenarioId: UuidV7;
      readonly sourceRevision: Revision;
      readonly runId: SolveRunId;
      readonly runManifestChecksum: string;
      readonly evidenceChecksum: string;
      readonly conflictId: string;
      readonly ruleId: UuidV7;
    },
  ];
  cancel: [];
  reviewCopy: [groupId: string];
  exportDiagnostic: [];
}>();

const headingId = useId();
const exportDisclosureId = useId();
const page = ref(0);
const PAGE_SIZE = 20;
const conflict = computed(() =>
  props.state === "ready" && props.evidence?.type === "conflict" ? props.evidence : null,
);
const unavailableEvidence = computed(() =>
  props.state === "ready" && props.evidence?.type === "unavailable" ? props.evidence : null,
);
const headingText = computed(() => {
  if (conflict.value) return explanationMessage("conflict.heading");
  if (unavailableEvidence.value) return "Conflict evidence unavailable";
  return "Conflict evidence";
});
const statusMessage = computed(() =>
  props.messages.find(
    (message) => message.messageKey === "official.workforce.infeasibility.status",
  ),
);
const fixedMessage = computed(() =>
  props.messages.find(
    (message) => message.messageKey === "official.workforce.infeasibility.fixed_scope",
  ),
);
const statusSummary = computed(() =>
  statusMessage.value === undefined ? null : textFact(statusMessage.value, "summary"),
);
const fixedScopeSummary = computed(() =>
  fixedMessage.value === undefined ? null : textFact(fixedMessage.value, "summary"),
);
const fixedFactCount = computed(() =>
  fixedMessage.value === undefined ? null : integerFact(fixedMessage.value, "fixed_fact_count"),
);
const hardLockCount = computed(() =>
  fixedMessage.value === undefined ? null : integerFact(fixedMessage.value, "hard_lock_count"),
);
const visibleGroups = computed(
  () => conflict.value?.groups.slice(page.value * PAGE_SIZE, (page.value + 1) * PAGE_SIZE) ?? [],
);
const pageCount = computed(() =>
  conflict.value === null ? 0 : Math.ceil(conflict.value.groups.length / PAGE_SIZE),
);
const canNavigate = computed(
  () =>
    props.ruleNavigationAvailable &&
    props.context !== null &&
    props.context.sourceRevision === props.currentRevision &&
    props.sourceRevision === props.currentRevision,
);
const stateText = computed(() => {
  if (props.state === "loading") return "Loading immutable conflict evidence…";
  if (props.state === "stale") return explanationMessage("error.stale");
  if (props.state === "cancelled") return "Conflict inspection was cancelled.";
  if (props.state === "inconclusive") return explanationMessage("error.inconclusive");
  if (props.state === "unavailable") return explanationMessage("error.unavailable");
  if (props.state === "internalFailure") return explanationMessage("error.internal");
  return explanationMessage("conflict.empty");
});
const shrinkText = computed(() => {
  const shrink = conflict.value?.shrink;
  if (!shrink) return "";
  const retained = `${formatCount(shrink.remainingGroupCount)} of ${formatCount(shrink.initialGroupCount)} groups remain.`;
  const trials = formatCount(shrink.attemptedTrials);
  if (shrink.stopReason === "completed")
    return `${retained} Shrinking completed after ${trials} of ${formatCount(shrink.maxTrials)} permitted trials.`;
  if (shrink.stopReason === "notAttempted")
    return `${retained} Conflict shrinking was not attempted.`;
  if (shrink.stopReason === "trialLimit")
    return `${retained} Conflict shrinking stopped at the trial limit after ${trials} trials.`;
  if (shrink.stopReason === "budgetExpired")
    return `${retained} Conflict shrinking stopped because its diagnostic time budget expired after ${trials} trials.`;
  if (shrink.stopReason === "cancelled")
    return `${retained} Conflict shrinking was cancelled after ${trials} trials.`;
  return `${retained} Conflict shrinking stopped because a trial was inconclusive after ${trials} trials.`;
});
const conditionSummary = computed(() =>
  props.condition === null ? null : conditionText(props.condition),
);

watch(
  () => props.evidence,
  () => (page.value = 0),
);

function textFact(message: EvidenceMessageV1, name: string): string | null {
  const value = message.parameters[`official.workforce.fact.${name}`];
  return value?.type === "text" && value.value.trim().length > 0 ? value.value : null;
}

function integerFact(message: EvidenceMessageV1, name: string): number | null {
  const value = message.parameters[`official.workforce.fact.${name}`];
  if (value?.type !== "integer" || !/^(0|[1-9][0-9]*)$/.test(value.value)) return null;
  const parsed = Number(value.value);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

function captionFor(groupId: string, ruleIds: readonly UuidV7[]): string | null {
  const expectedRules = new Set(ruleIds);
  const message = props.messages.find(
    (item) =>
      item.messageKey === "official.workforce.infeasibility.conflict_group" &&
      textFact(item, "group_id") === groupId &&
      item.rules.length === expectedRules.size &&
      item.rules.every((ruleId) => expectedRules.has(ruleId)),
  );
  return message === undefined ? null : textFact(message, "source_caption");
}

function assignmentValueText(value: AssignmentValue): string {
  switch (value.type) {
    case "boolean":
      return `Boolean ${String(value.value)}`;
    case "integer":
      return `Integer ${value.value}`;
    case "interval":
      return `Interval ${value.value.start}–${value.value.end} (duration ${value.value.duration})`;
    case "absent":
      return "Absent";
  }
}

function conditionText(condition: CounterfactualConditionV1): string {
  const value = condition.condition;
  if (value.type === "replaceAssignment")
    return `Set target assignment ${value.targetAssignmentId} to Boolean true and replaced assignment ${value.replacedAssignmentId} to Boolean false.`;
  return `${value.type === "forceAssignmentValue" ? "Force" : "Forbid"} assignment ${value.assignmentId} = ${assignmentValueText(value.value)}.`;
}

function emitRuleAction(action: "inspect" | "edit", ruleId: UuidV7): void {
  const source = props.context;
  if (!canNavigate.value || source === null) return;
  emit("ruleAction", {
    action,
    scenarioId: props.scenarioId,
    sourceRevision: source.sourceRevision,
    runId: source.runId,
    runManifestChecksum: source.runManifestChecksum,
    evidenceChecksum: source.evidenceChecksum,
    conflictId: source.conflictId,
    ruleId,
  });
}

function formatCount(value: number): string {
  return new Intl.NumberFormat().format(value);
}
</script>

<template>
  <Card
    as="article"
    :variant="state === 'internalFailure' ? 'danger' : 'surface'"
    :aria-labelledby="headingId"
  >
    <h6 v-if="headingLevel === 6" :id="headingId" class="font-display text-lg font-bold text-ink">
      {{ headingText }}
    </h6>
    <div
      v-else
      :id="headingId"
      role="heading"
      :aria-level="headingLevel"
      class="font-display text-lg font-bold text-ink"
    >
      {{ headingText }}
    </div>

    <template v-if="state === 'ready' && evidence !== null">
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <Badge
          v-if="evidence.type === 'conflict'"
          :variant="evidence.minimality === 'provenMinimal' ? 'neutral' : 'accent'"
        >
          {{
            evidence.minimality === "provenMinimal"
              ? "Proven minimal conflict"
              : "Sufficient conflict"
          }}
        </Badge>
        <Badge v-else variant="outline">
          {{
            evidence.reason === "foundationalInfeasibility"
              ? "Foundational infeasibility"
              : "Unavailable"
          }}
        </Badge>
        <code v-if="evidence.type === 'unavailable'">{{ evidence.reason }}</code>
      </div>

      <p v-if="statusSummary" class="mt-3 text-sm text-muted">{{ statusSummary }}</p>
      <p v-if="fixedScopeSummary" class="mt-3 text-sm text-muted">{{ fixedScopeSummary }}</p>
      <dl v-if="fixedFactCount !== null && hardLockCount !== null" class="metadata-list">
        <div>
          <dt>Fixed activity and approved-leave facts</dt>
          <dd>{{ formatCount(fixedFactCount) }}</dd>
        </div>
        <div>
          <dt>Hard locks</dt>
          <dd>{{ formatCount(hardLockCount) }}</dd>
        </div>
      </dl>
      <p v-if="conditionSummary" class="mt-3 break-all" data-infeasibility-condition>
        Recorded temporary condition: {{ conditionSummary }}
      </p>
      <p
        v-if="diagnosticActionsAvailable"
        :id="exportDisclosureId"
        role="note"
        class="mt-3 text-sm text-muted"
      >
        {{ explanationMessage("conflict.exportDisclosure") }}
      </p>
      <button
        v-if="diagnosticActionsAvailable"
        type="button"
        class="mt-3 rounded-md border border-line-strong px-3 py-2 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
        :aria-describedby="exportDisclosureId"
        :disabled="actionButtonsDisabled"
        @click="emit('exportDiagnostic')"
      >
        {{ explanationMessage("conflict.diagnosticExport") }}
      </button>

      <template v-if="conflict">
        <p class="mt-3 text-sm text-muted">{{ shrinkText }}</p>
        <ol class="mt-4 space-y-4" aria-label="Mapped Required-rule conflict groups">
          <li
            v-for="group in visibleGroups"
            :key="group.groupId"
            class="rounded-md border border-line bg-raised p-4"
          >
            <p role="heading" :aria-level="headingLevel + 1" class="font-semibold text-ink">
              Conflict group {{ group.groupId }}
            </p>
            <p class="mt-2 text-sm text-ink">
              {{ captionFor(group.groupId, group.requiredRules) }}
            </p>
            <button
              v-if="copyableGroupIds.includes(group.groupId)"
              type="button"
              class="mt-3 rounded-md border border-line-strong px-3 py-2 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
              :disabled="actionButtonsDisabled"
              @click="emit('reviewCopy', group.groupId)"
            >
              {{ explanationMessage("conflict.copyReviewAction") }}
            </button>
            <ul :aria-label="`Required source rules in ${group.groupId}`">
              <li
                v-for="ruleId in group.requiredRules"
                :key="ruleId"
                class="border-t border-line pt-2 first:border-t-0 first:pt-0"
              >
                <p class="text-xs text-muted">
                  Source Required rule <code>{{ ruleId }}</code>
                </p>
                <span v-if="canNavigate" class="mt-2 flex flex-wrap gap-2">
                  <button
                    type="button"
                    class="rounded-md border border-line-strong px-3 py-1 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
                    @click="emitRuleAction('inspect', ruleId)"
                  >
                    Inspect <span class="sr-only">source Required rule {{ ruleId }}</span>
                  </button>
                  <button
                    type="button"
                    class="rounded-md border border-line-strong px-3 py-1 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
                    @click="emitRuleAction('edit', ruleId)"
                  >
                    Edit <span class="sr-only">source Required rule {{ ruleId }}</span>
                  </button>
                </span>
              </li>
            </ul>
          </li>
        </ol>
        <nav
          v-if="pageCount > 1"
          class="mt-3 flex items-center gap-3"
          aria-label="Conflict group pages"
        >
          <button type="button" :disabled="page === 0" @click="page -= 1">Previous groups</button>
          <span>Page {{ page + 1 }} of {{ pageCount }}</span>
          <button
            type="button"
            :disabled="(page + 1) * PAGE_SIZE >= conflict.groups.length"
            @click="page += 1"
          >
            Next groups
          </button>
        </nav>
        <p v-if="!canNavigate" class="mt-3 text-sm text-muted" role="status">
          Rule navigation is unavailable here. The recorded evidence remains inspectable.
        </p>
      </template>
    </template>

    <div v-else class="mt-3" :role="state === 'internalFailure' ? 'alert' : 'status'">
      <p class="text-sm text-muted">{{ stateText }}</p>
      <p v-if="errorMessage" class="mt-1 break-all text-sm text-danger">{{ errorMessage }}</p>
      <button
        v-if="state === 'loading'"
        type="button"
        class="mt-3 rounded-md border border-line-strong px-3 py-2 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
        :disabled="cancelRequested"
        @click="emit('cancel')"
      >
        {{ cancelRequested ? "Cancellation requested…" : explanationMessage("action.cancel") }}
      </button>
    </div>
  </Card>
</template>
