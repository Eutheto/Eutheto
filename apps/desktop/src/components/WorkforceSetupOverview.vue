<script setup lang="ts">
import { computed, onScopeDispose, ref, shallowRef, watch } from "vue";
import { RouterLink } from "vue-router";
import {
  getScenarioSummary,
  getScenarioView,
  listSolutions,
  SetupOperationScope,
  type Revision,
  type ScenarioSummaryV2,
  type SolutionListDtoV1,
} from "../api/generated";
import type {
  WorkforceSetupEntityKind,
  WorkforceSetupFacts,
} from "../api/generated-domain-pack-contracts";
import {
  isRevisionConflict,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";
import { formatDateTime, formatNumber, messages } from "../messages";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  locale?: string;
  libraryRevision?: Revision | null;
}>();
const snapshot = shallowRef<{
  facts: WorkforceSetupFacts;
  summary: ScenarioSummaryV2;
  results: SolutionListDtoV1;
} | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
let generation = 0;
let current = true;
let scope: SetupOperationScope | null = null;
const supported = computed(() => props.project.domainPackId === "official.workforce");
const sections = ["calendar", "people", "work", "rules", "validation", "results"] as const;
const groups: readonly { id: "people" | "work"; kinds: readonly WorkforceSetupEntityKind[] }[] = [
  { id: "people", kinds: ["person", "qualification", "team", "availability"] },
  {
    id: "work",
    kinds: [
      "location",
      "workloadBucket",
      "calendar",
      "assignmentType",
      "shiftTemplate",
      "shiftInstance",
      "coverageRequirement",
      "baseSchedule",
      "scorePolicy",
    ],
  },
];
const horizonFormatter = computed(() => {
  if (snapshot.value === null) return null;
  try {
    return new Intl.DateTimeFormat(props.locale, {
      dateStyle: "medium",
      timeStyle: "short",
      timeZone: snapshot.value.facts.settings.timeZone,
    });
  } catch {
    return null;
  }
});
function horizon(value: string): string {
  const instant = new Date(value);
  return horizonFormatter.value !== null && !Number.isNaN(instant.getTime())
    ? horizonFormatter.value.format(instant)
    : value;
}
async function load(): Promise<void> {
  const captured = ++generation;
  scope?.dispose();
  scope = null;
  snapshot.value = null;
  error.value = null;
  if (!supported.value || !current) {
    loading.value = false;
    return;
  }
  loading.value = true;
  const { scenarioId, revision } = props.project;
  let owned: SetupOperationScope | null = null;
  try {
    owned = new SetupOperationScope(scenarioId, revision);
    scope = owned;
    const [summary, overview, results] = await Promise.all([
      getScenarioSummary(owned).result,
      getScenarioView(owned, {
        source: { kind: "stored" },
        query: { schemaVersion: 1, viewId: "official.workforce.setup.overview", parameters: {} },
      }).result,
      listSolutions(scenarioId),
    ]);
    if (captured !== generation) return;
    if (
      summary.result.scenarioId !== scenarioId ||
      overview.result.scenarioId !== scenarioId ||
      results.result.scenarioId !== scenarioId ||
      summary.result.revision !== revision ||
      overview.result.revision !== revision ||
      results.result.currentRevision !== revision ||
      props.project.scenarioId !== scenarioId ||
      props.project.revision !== revision
    ) {
      error.value = messages.setup.stale;
      return;
    }
    snapshot.value = {
      facts: overview.result.view.data.result.data,
      summary: summary.result,
      results: results.result,
    };
  } catch (failure) {
    if (captured === generation)
      error.value = isRevisionConflict(failure) ? messages.setup.stale : safeMessage(failure);
  } finally {
    owned?.dispose();
    if (captured === generation) {
      scope = null;
      loading.value = false;
    }
  }
}
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
  ],
  () => void load(),
  { immediate: true },
);
onScopeDispose(() => {
  current = false;
  generation += 1;
  scope?.dispose();
});
async function refresh(): Promise<void> {
  const epoch = props.home.state.libraryEpoch;
  if ((await props.home.refreshLibrary()) && current && props.home.state.libraryEpoch === epoch)
    await load();
}
</script>

<template>
  <section class="page-stack" aria-labelledby="setup-heading">
    <h2 id="setup-heading" data-route-heading tabindex="-1">
      {{ supported ? messages.setup.heading : messages.setup.unsupportedTitle }}
    </h2>
    <div v-if="!supported" class="state-panel">
      <p>{{ messages.setup.unsupported }}</p>
    </div>
    <template v-else>
      <p>{{ messages.setup.description }}</p>
      <div v-if="loading" class="state-panel" role="status" aria-busy="true">
        {{ messages.setup.loading }}
      </div>
      <div v-else-if="error" class="state-panel" role="alert">
        <p>{{ error }}</p>
        <button type="button" :disabled="home.state.busyAction !== null" @click="refresh">
          {{ messages.setup.refresh }}
        </button>
      </div>
      <template v-else-if="snapshot">
        <nav class="setup-checklist" :aria-label="messages.setup.navigation">
          <RouterLink
            v-for="section in sections"
            :key="section"
            :to="{
              name: 'project-setup',
              params: { scenarioId: project.scenarioId },
              hash: `#setup-${section}`,
            }"
          >
            {{ messages.setup[section] }}
          </RouterLink>
        </nav>
        <section class="state-panel" aria-labelledby="setup-calendar">
          <h3 id="setup-calendar" tabindex="-1">{{ messages.setup.calendar }}</h3>
          <dl class="metadata-list">
            <div>
              <dt>{{ messages.projects.timeZone }}</dt>
              <dd>{{ snapshot.facts.settings.timeZone }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.horizonStart }}</dt>
              <dd>{{ horizon(snapshot.facts.settings.horizon.start) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.horizonEnd }}</dt>
              <dd>{{ horizon(snapshot.facts.settings.horizon.end) }}</dd>
            </div>
            <div>
              <dt>{{ messages.projects.locale }}</dt>
              <dd>{{ snapshot.facts.settings.locale }}</dd>
            </div>
            <div>
              <dt>{{ messages.projects.displayUnits }}</dt>
              <dd>
                {{
                  snapshot.facts.settings.units === "metric"
                    ? messages.projects.metric
                    : messages.projects.usCustomary
                }}
              </dd>
            </div>
            <div>
              <dt>{{ messages.projects.missingClockTime }}</dt>
              <dd>
                {{
                  snapshot.facts.settings.gapPolicy === "reject"
                    ? messages.projects.reject
                    : snapshot.facts.settings.gapPolicy === "moveForward"
                      ? messages.projects.moveForward
                      : messages.projects.packPolicy
                }}
              </dd>
            </div>
            <div>
              <dt>{{ messages.projects.repeatedClockTime }}</dt>
              <dd>
                {{
                  snapshot.facts.settings.overlapPolicy === "reject"
                    ? messages.projects.reject
                    : snapshot.facts.settings.overlapPolicy === "earlier"
                      ? messages.projects.earlier
                      : messages.projects.later
                }}
              </dd>
            </div>
          </dl>
          <p v-if="horizonFormatter === null" role="status">{{ messages.setup.zoneUnavailable }}</p>
        </section>
        <section
          v-for="group in groups"
          :key="group.id"
          class="state-panel"
          :aria-labelledby="`setup-${group.id}`"
        >
          <h3 :id="`setup-${group.id}`" tabindex="-1">{{ messages.setup[group.id] }}</h3>
          <p>{{ messages.setup.countsBoundary }}</p>
          <dl class="metadata-list">
            <div
              v-for="count in snapshot.facts.entities.filter((item) =>
                group.kinds.includes(item.kind),
              )"
              :key="count.kind"
            >
              <dt>{{ messages.setup.entityKinds[count.kind] }}</dt>
              <dd>{{ formatNumber(count.count, locale) }}</dd>
            </div>
          </dl>
        </section>
        <section class="state-panel" aria-labelledby="setup-rules">
          <h3 id="setup-rules" tabindex="-1">{{ messages.setup.rules }}</h3>
          <dl class="metadata-list">
            <div>
              <dt>{{ messages.setup.required }}</dt>
              <dd>{{ formatNumber(snapshot.facts.requiredRules, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.activeRequired }}</dt>
              <dd>{{ formatNumber(snapshot.facts.activeRequiredRules, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.preferences }}</dt>
              <dd>{{ formatNumber(snapshot.facts.preferences, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.activePreferences }}</dt>
              <dd>{{ formatNumber(snapshot.facts.activePreferences, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.locks }}</dt>
              <dd>{{ formatNumber(snapshot.facts.lockedAssignments, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.memberships }}</dt>
              <dd>{{ formatNumber(snapshot.facts.configuredTypeMemberships, locale) }}</dd>
            </div>
          </dl>
        </section>
        <section class="state-panel" aria-labelledby="setup-validation">
          <h3 id="setup-validation" tabindex="-1">{{ messages.setup.validation }}</h3>
          <h4>{{ messages.setup.fast }}</h4>
          <dl class="metadata-list">
            <div>
              <dt>{{ messages.setup.errors }}</dt>
              <dd>{{ formatNumber(snapshot.summary.fast.counts.errors, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.warnings }}</dt>
              <dd>{{ formatNumber(snapshot.summary.fast.counts.warnings, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.information }}</dt>
              <dd>{{ formatNumber(snapshot.summary.fast.counts.information, locale) }}</dd>
            </div>
          </dl>
          <ul v-if="snapshot.summary.fast.issues.length">
            <li v-for="(issue, index) in snapshot.summary.fast.issues" :key="index">
              <strong>{{ issue.severity }}:</strong> {{ issue.message }}
              <code v-if="issue.fieldPath">{{ issue.fieldPath }}</code>
            </li>
          </ul>
          <p v-if="snapshot.summary.fast.omitted">
            {{ messages.setup.omitted(formatNumber(snapshot.summary.fast.omitted, locale)) }}
          </p>
          <h4>{{ messages.setup.full }}</h4>
          <p>{{ messages.setup.validationStates[snapshot.summary.full.state] }}</p>
          <template v-if="snapshot.summary.full.state !== 'notRun'">
            <p>
              {{
                messages.setup.validationRevision(
                  formatNumber(snapshot.summary.full.inputRevision, locale),
                )
              }}
            </p>
            <p v-if="snapshot.summary.full.stale">{{ messages.setup.staleValidation }}</p>
            <code v-if="snapshot.summary.full.state === 'failed'">{{
              snapshot.summary.full.code
            }}</code>
            <dl v-if="snapshot.summary.full.state === 'completed'" class="metadata-list">
              <div>
                <dt>{{ messages.setup.errors }}</dt>
                <dd>{{ formatNumber(snapshot.summary.full.counts.errors, locale) }}</dd>
              </div>
              <div>
                <dt>{{ messages.setup.warnings }}</dt>
                <dd>{{ formatNumber(snapshot.summary.full.counts.warnings, locale) }}</dd>
              </div>
              <div>
                <dt>{{ messages.setup.information }}</dt>
                <dd>{{ formatNumber(snapshot.summary.full.counts.information, locale) }}</dd>
              </div>
            </dl>
          </template>
        </section>
        <section class="state-panel" aria-labelledby="setup-results">
          <h3 id="setup-results" tabindex="-1">{{ messages.setup.results }}</h3>
          <p>{{ messages.setup.acceptedBoundary }}</p>
          <p v-if="snapshot.results.solutions.length === 0">{{ messages.setup.noAccepted }}</p>
          <ul v-else class="result-summary-list">
            <li v-for="result in snapshot.results.solutions" :key="result.result.solutionId">
              <strong>{{ messages.setup.accepted }}</strong>
              <p>
                {{ result.stale ? messages.setup.staleResult : messages.setup.current }} ·
                {{ result.selected ? messages.setup.selected : messages.setup.unselected }}
              </p>
              <p>
                {{ messages.setup.resultRevision(formatNumber(result.scenarioRevision, locale)) }}
              </p>
              <dl class="metadata-list">
                <div>
                  <dt>{{ messages.setup.status }}</dt>
                  <dd>{{ result.status }}</dd>
                </div>
                <div>
                  <dt>{{ messages.setup.finished }}</dt>
                  <dd>{{ formatDateTime(result.finishedAt, locale) }}</dd>
                </div>
              </dl>
            </li>
          </ul>
        </section>
      </template>
    </template>
  </section>
</template>
