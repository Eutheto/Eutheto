<script setup lang="ts">
import { computed, onScopeDispose, ref, watch } from "vue";
import {
  explainSolution,
  SetupOperationScope,
  type Revision,
  type SolutionExplanationDtoV2,
  type SolutionSummaryDtoV1,
  type VerificationValue,
  type UuidV7,
} from "../api/generated";
import type { WorkforceResultScheduleAssignmentRow } from "../api/generated-domain-pack-contracts";
import { messages } from "../messages";
import AcceptedCounterfactual from "./AcceptedCounterfactual.vue";
import { safeMessage, type ProjectHomeController } from "../project-home";
import type { ConflictRuleAction } from "./explanations/types";

const props = defineProps<{
  scenarioId: UuidV7;
  summary: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision"> &
    Partial<Pick<SolutionSummaryDtoV1, "status">>;
  currentRevision: Revision;
  assignment: WorkforceResultScheduleAssignmentRow;
  startLabel: string;
  endLabel: string;
  home?: ProjectHomeController | undefined;
  ruleNavigationAvailable?: boolean;
}>();
const emit = defineEmits<{ close: []; ruleAction: [action: ConflictRuleAction] }>();
const copy = messages.acceptedSchedule.inspection;
const heading = ref<HTMLElement | null>(null);
const explanation = ref<SolutionExplanationDtoV2 | null>(null);
const error = ref<string | null>(null);
const loading = ref(false);
const evidencePage = ref(0);
const evidenceMessages = computed(() => explanation.value?.explanation.rendered.messages ?? []);
const visibleEvidence = computed(() =>
  evidenceMessages.value.slice(evidencePage.value * 50, (evidencePage.value + 1) * 50),
);
let generation = 0;
let alive = true;
let explanationScope: SetupOperationScope | null = null;

function messageLabel(key: string): string {
  if (key === "official.workforce.assignment.recorded_decision") return copy.recordedDecision;
  if (key === "official.workforce.evaluation.summary") return copy.requiredCheck;
  if (key === "official.workforce.assignment.preference.limit") return copy.preferenceLimit;
  if (key.startsWith("official.workforce.assignment.preference.")) return copy.preferenceFact;
  return copy.otherEvidence;
}
function factLabel(key: string): string {
  const name = key.startsWith("official.workforce.fact.")
    ? key.slice("official.workforce.fact.".length)
    : key;
  return name.replaceAll("_", " ");
}
function factValue(value: VerificationValue): string {
  switch (value.type) {
    case "entity":
      return `${value.value.kind} ${value.value.id}`;
    case "boolean":
      return value.value ? copy.yes : copy.no;
    case "integer":
    case "text":
      return value.value;
  }
}

function focusHeading(): void {
  heading.value?.focus();
}
defineExpose({ focusHeading });

async function load(): Promise<void> {
  explanationScope?.dispose();
  explanationScope = null;
  const token = ++generation;
  const scenarioId = props.scenarioId;
  const solutionId = props.summary.result.solutionId;
  const resultChecksum = props.summary.result.resultChecksum;
  const sourceRevision = props.summary.scenarioRevision;
  const currentRevision = props.currentRevision;
  const assignmentId = props.assignment.assignmentId;
  const stillCurrent = () =>
    alive &&
    token === generation &&
    props.scenarioId === scenarioId &&
    props.summary.result.solutionId === solutionId &&
    props.summary.result.resultChecksum === resultChecksum &&
    props.summary.scenarioRevision === sourceRevision &&
    props.currentRevision === currentRevision &&
    props.assignment.assignmentId === assignmentId;
  explanation.value = null;
  error.value = null;
  evidencePage.value = 0;
  loading.value = false;
  if (props.summary.status !== "optimal" && props.summary.status !== "feasible") return;
  loading.value = true;
  let scope: SetupOperationScope | null = null;
  try {
    scope = new SetupOperationScope(scenarioId, sourceRevision);
    explanationScope = scope;
    const { result } = await explainSolution(scope, {
      schemaVersion: 1,
      subject: {
        kind: "assignment",
        result: { solutionId, resultChecksum },
        assignmentId,
      },
    }).result;
    if (!stillCurrent()) return;
    const evidence = result.explanation.evidence;
    if (
      result.scenarioId !== scenarioId ||
      result.currentRevision !== currentRevision ||
      result.scenarioRevisions.length !== 1 ||
      result.scenarioRevisions[0] !== sourceRevision ||
      evidence.evidence.kind !== "assignment" ||
      evidence.evidence.assignment.assignment.id !== assignmentId ||
      evidence.evidence.assignment.assignment.value.type !== "boolean" ||
      !evidence.evidence.assignment.assignment.value.value ||
      result.explanation.rendered.kind !== "assignment" ||
      result.explanation.rendered.evidenceChecksum !== evidence.checksum
    ) {
      error.value = messages.acceptedSchedule.sourceChanged;
      return;
    }
    explanation.value = result;
  } catch (failure) {
    if (stillCurrent()) error.value = safeMessage(failure);
  } finally {
    scope?.dispose();
    if (explanationScope === scope) explanationScope = null;
    if (stillCurrent()) loading.value = false;
  }
}

watch(
  [
    () => props.scenarioId,
    () => props.summary.result.solutionId,
    () => props.summary.result.resultChecksum,
    () => props.summary.scenarioRevision,
    () => props.summary.status,
    () => props.currentRevision,
    () => props.assignment.assignmentId,
  ],
  () => void load(),
  { immediate: true },
);
onScopeDispose(() => {
  alive = false;
  generation += 1;
  explanationScope?.dispose();
  explanationScope = null;
});
</script>

<template>
  <aside :aria-label="copy.heading" class="field-stack" data-assignment-details>
    <h5 ref="heading" tabindex="-1">{{ copy.heading }}</h5>
    <p>
      {{ assignment.personName }} ·
      {{ assignment.assignmentTypeName ?? assignment.assignmentTypeId }} ·
      {{ assignment.locationName ?? messages.acceptedSchedule.noLocation }}
    </p>
    <p>{{ messages.acceptedSchedule.start }}: {{ startLabel }}</p>
    <p>{{ messages.acceptedSchedule.end }}: {{ endLabel }}</p>
    <button type="button" @click="emit('close')">{{ copy.close }}</button>
    <p v-if="summary.status !== 'optimal' && summary.status !== 'feasible'" role="status">
      {{ copy.checkpoint }}
    </p>
    <p v-if="loading" role="status">{{ copy.loading }}</p>
    <div v-if="error" role="alert">
      <p>{{ error }}</p>
      <button type="button" @click="load">{{ copy.retry }}</button>
    </div>
    <template v-if="explanation">
      <p>{{ copy.limits }}</p>
      <p v-if="evidenceMessages.length" role="status">
        {{
          copy.messageCount(
            evidencePage * 50 + 1,
            Math.min((evidencePage + 1) * 50, evidenceMessages.length),
            evidenceMessages.length,
          )
        }}
      </p>
      <ol v-if="evidenceMessages.length">
        <li
          v-for="(message, index) in visibleEvidence"
          :key="`${message.messageKey}-${evidencePage * 50 + index}`"
        >
          <p>{{ messageLabel(message.messageKey) }}</p>
          <p v-if="message.rules.length">
            {{
              message.messageKey.startsWith("official.workforce.assignment.preference.")
                ? copy.preferenceRuleIds
                : copy.sourceCheckIds
            }}: {{ message.rules.join(", ") }}
          </p>
          <dl v-if="Object.keys(message.parameters).length">
            <div v-for="(value, key) in message.parameters" :key="key">
              <dt>{{ factLabel(key) }}</dt>
              <dd>{{ factValue(value) }}</dd>
            </div>
          </dl>
          <p v-if="message.assignments.length">
            {{ copy.assignments }}: {{ message.assignments.join(", ") }}
          </p>
        </li>
      </ol>
      <p v-else role="status">{{ copy.empty }}</p>
      <div v-if="evidenceMessages.length" class="action-row">
        <button type="button" :disabled="evidencePage === 0" @click="evidencePage -= 1">
          {{ copy.previous }}
        </button>
        <button
          type="button"
          :disabled="(evidencePage + 1) * 50 >= evidenceMessages.length"
          @click="evidencePage += 1"
        >
          {{ copy.next }}
        </button>
      </div>
    </template>
    <AcceptedCounterfactual
      v-if="summary.status === 'optimal' || summary.status === 'feasible'"
      :scenario-id="scenarioId"
      :summary="summary"
      :current-revision="currentRevision"
      :assignment="assignment"
      :home="home"
      :rule-navigation-available="ruleNavigationAvailable"
      @rule-action="emit('ruleAction', $event)"
    />
  </aside>
</template>
