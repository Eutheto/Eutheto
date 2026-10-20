<script setup lang="ts">
import { computed, onScopeDispose, ref, watch } from "vue";
import {
  verifySolution,
  type Revision,
  type SolutionSummaryDtoV1,
  type UuidV7,
  type VerificationReport,
} from "../api/generated";
import { messages } from "../messages";
import { safeMessage } from "../project-home";

const props = defineProps<{
  scenarioId: UuidV7;
  summary: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision"> &
    Partial<Pick<SolutionSummaryDtoV1, "verificationChecksum" | "status">>;
  currentRevision: Revision;
}>();
const copy = messages.acceptedSchedule;
const report = ref<VerificationReport | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
let generation = 0;
let alive = true;
const preferenceLevels = computed(
  () =>
    report.value?.score.levels.filter((level) =>
      level.levelId.startsWith("official.workforce.objective.preference."),
    ) ?? [],
);
const prioritiesWithoutPenalty = computed(
  () => preferenceLevels.value.filter((level) => level.value === "0").length,
);

async function load(): Promise<void> {
  const token = ++generation;
  const scenarioId = props.scenarioId;
  const solutionId = props.summary.result.solutionId;
  const resultChecksum = props.summary.result.resultChecksum;
  const verificationChecksum = props.summary.verificationChecksum;
  const sourceRevision = props.summary.scenarioRevision;
  const currentRevision = props.currentRevision;
  const stillCurrent = () =>
    alive &&
    token === generation &&
    props.scenarioId === scenarioId &&
    props.currentRevision === currentRevision &&
    props.summary.scenarioRevision === sourceRevision &&
    props.summary.result.solutionId === solutionId &&
    props.summary.result.resultChecksum === resultChecksum &&
    props.summary.verificationChecksum === verificationChecksum;
  report.value = null;
  error.value = null;
  loading.value = true;
  try {
    const { result } = await verifySolution(scenarioId, solutionId);
    if (!stillCurrent()) return;
    if (
      result.scenarioId !== scenarioId ||
      result.currentRevision !== currentRevision ||
      result.scenarioRevision !== sourceRevision ||
      result.result.solutionId !== solutionId ||
      result.result.resultChecksum !== resultChecksum ||
      result.verification.scenarioId !== scenarioId ||
      result.verification.evaluatedRevision !== sourceRevision ||
      (verificationChecksum !== undefined &&
        result.verification.checksum !== verificationChecksum) ||
      !result.verification.accepted ||
      result.verification.requiredRuleResults.some((rule) => !rule.satisfied)
    ) {
      error.value = copy.sourceChanged;
      return;
    }
    report.value = result.verification;
  } catch (failure) {
    if (stillCurrent()) error.value = safeMessage(failure);
  } finally {
    if (stillCurrent()) loading.value = false;
  }
}

watch(
  [
    () => props.scenarioId,
    () => props.summary.result.solutionId,
    () => props.summary.result.resultChecksum,
    () => props.summary.scenarioRevision,
    () => props.summary.verificationChecksum,
    () => props.currentRevision,
  ],
  () => void load(),
  { immediate: true },
);
onScopeDispose(() => {
  alive = false;
  generation += 1;
});

const categoryLabels: Readonly<Record<string, string>> = {
  "official.workforce.score.preference.time": "Preferred times",
  "official.workforce.score.preference.assignment_type": "Preferred work types",
  "official.workforce.score.preference.location": "Preferred locations",
  "official.workforce.score.preference.requested_time_off": "Requested time off",
  "official.workforce.score.preference.assignment_target": "Workload targets",
  "official.workforce.score.preference.weekend_target": "Weekend targets",
  "official.workforce.score.preference.workload_balance": "Workload balance",
  "official.workforce.score.preference.consecutive_work": "Consecutive work",
  "official.workforce.score.preference.adjacency": "Adjacent work",
  "official.workforce.score.preference.base_stability": "Changes from the base plan",
  "official.workforce.score.preference.together_separate": "Working together or apart",
  "official.workforce.score.preference.preferred_coverage": "Preferred coverage",
};
</script>

<template>
  <section class="field-stack" :aria-label="copy.verification.heading">
    <h5>{{ copy.verification.heading }}</h5>
    <p v-if="summary.status">
      {{ copy.verification.recordedStatus }}: {{ messages.setup.optimize.statuses[summary.status] }}
    </p>
    <p v-if="loading" role="status">{{ copy.verification.loading }}</p>
    <div v-if="error" role="alert">
      <p>{{ copy.verification.error }}: {{ error }}</p>
      <button type="button" @click="load">{{ copy.verification.retry }}</button>
    </div>
    <template v-if="report">
      <p role="status">
        {{
          summary.scenarioRevision === currentRevision
            ? copy.verification.current
            : copy.verification.older(String(summary.scenarioRevision))
        }}
      </p>
      <p>{{ copy.verification.required(report.requiredRuleResults.length) }}</p>
      <section :aria-label="copy.verification.preferences">
        <h6>{{ copy.verification.preferences }}</h6>
        <p v-if="preferenceLevels.length">
          {{ copy.verification.satisfaction(prioritiesWithoutPenalty, preferenceLevels.length) }}
        </p>
        <p>{{ copy.verification.preferenceHelp }}</p>
        <ol v-if="preferenceLevels.length">
          <li v-for="(level, index) in preferenceLevels" :key="level.levelId">
            <span>{{ copy.verification.priority(index + 1) }}: {{ level.value }}</span>
            <ul v-if="Object.keys(level.categoryBreakdown).length">
              <li v-for="(value, category) in level.categoryBreakdown" :key="category">
                {{ categoryLabels[category] ?? category }}: {{ value }}
              </li>
            </ul>
          </li>
        </ol>
        <p v-else>{{ copy.verification.noPreferences }}</p>
      </section>
      <section v-if="report.warnings.length" :aria-label="copy.verification.warnings">
        <h6>{{ copy.verification.warnings }}</h6>
        <ul>
          <li v-for="warning in report.warnings" :key="warning.id">{{ warning.messageKey }}</li>
        </ul>
      </section>
    </template>
  </section>
</template>
