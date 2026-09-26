<script setup lang="ts">
import { computed, onScopeDispose, ref, shallowRef, watch } from "vue";
import { RouterLink } from "vue-router";
import {
  getAppCapabilities,
  getScenarioSummary,
  getScenarioView,
  listSolutions,
  SetupOperationScope,
  type AppCapabilitiesDto,
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
import OptimizeCapabilityPanel from "./OptimizeCapabilityPanel.vue";
import ValidationSummary from "./explanations/ValidationSummary.vue";

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
const capabilities = shallowRef<AppCapabilitiesDto | null>(null);
const capabilitiesLoading = ref(false);
const capabilitiesError = ref<string | null>(null);
let generation = 0;
let current = true;
let scope: SetupOperationScope | null = null;
const supported = computed(() => props.project.domainPackId === "official.workforce");
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
async function readCapabilities(captured: number): Promise<void> {
  capabilitiesLoading.value = true;
  capabilities.value = null;
  capabilitiesError.value = null;
  try {
    const response = await getAppCapabilities();
    if (captured !== generation || !current) return;
    capabilities.value = response.result;
  } catch (failure) {
    if (captured === generation && current) capabilitiesError.value = safeMessage(failure);
  } finally {
    if (captured === generation && current) capabilitiesLoading.value = false;
  }
}
async function load(): Promise<void> {
  const captured = ++generation;
  scope?.dispose();
  scope = null;
  snapshot.value = null;
  error.value = null;
  capabilities.value = null;
  capabilitiesError.value = null;
  capabilitiesLoading.value = false;
  if (!supported.value || !current) {
    loading.value = false;
    return;
  }
  loading.value = true;
  void readCapabilities(captured);
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
async function retryCapabilities(): Promise<void> {
  if (!current || !supported.value || snapshot.value === null || capabilitiesLoading.value) return;
  await readCapabilities(generation);
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
        <h3 id="setup-guide">{{ messages.setup.guide }}</h3>
        <ol class="setup-steps" aria-labelledby="setup-guide">
          <li class="state-panel setup-step" aria-labelledby="setup-people">
            <h4 id="setup-people" tabindex="-1">1. {{ messages.setup.people }}</h4>
            <p>{{ messages.setup.peopleStep }}</p>
            <RouterLink
              :to="{ name: 'project-people', params: { scenarioId: project.scenarioId } }"
            >
              {{ messages.people.heading }}
            </RouterLink>
            <div class="action-row">
              <RouterLink
                :to="{ name: 'project-eligibility', params: { scenarioId: project.scenarioId } }"
              >
                {{ messages.setup.eligibility }}
              </RouterLink>
              <RouterLink
                :to="{ name: 'project-availability', params: { scenarioId: project.scenarioId } }"
              >
                {{ messages.setup.availability }}
              </RouterLink>
            </div>
          </li>
          <li class="state-panel setup-step" aria-labelledby="setup-work">
            <h4 id="setup-work" tabindex="-1">2. {{ messages.setup.work }}</h4>
            <p>{{ messages.setup.workStep }}</p>
            <RouterLink :to="{ name: 'project-work', params: { scenarioId: project.scenarioId } }">
              {{ messages.work.heading }}
            </RouterLink>
          </li>
          <li class="state-panel setup-step" aria-labelledby="setup-rules">
            <h4 id="setup-rules" tabindex="-1">3. {{ messages.setup.rules }}</h4>
            <p>{{ messages.setup.rulesStep }}</p>
            <RouterLink :to="{ name: 'project-rules', params: { scenarioId: project.scenarioId } }">
              {{ messages.setup.rules }}
            </RouterLink>
          </li>
          <li class="state-panel setup-step" aria-labelledby="setup-validation">
            <h4 id="setup-validation" tabindex="-1">4. {{ messages.setup.validation }}</h4>
            <p>{{ messages.setup.validationStep }}</p>
            <p>
              {{ messages.setup.full }}:
              {{ messages.setup.validationStates[snapshot.summary.full.state] }}
            </p>
            <p v-if="snapshot.summary.full.state !== 'notRun' && snapshot.summary.full.stale">
              {{ messages.setup.staleValidation }}
            </p>
            <p v-if="snapshot.summary.fast.counts.errors" role="alert">
              {{ messages.setup.fast }}:
              {{ formatNumber(snapshot.summary.fast.counts.errors, locale) }}
              {{ messages.setup.errors }}
            </p>
            <p
              v-if="
                snapshot.summary.full.state === 'completed' && snapshot.summary.full.counts.errors
              "
              role="alert"
            >
              {{ messages.setup.full }}:
              {{ formatNumber(snapshot.summary.full.counts.errors, locale) }}
              {{ messages.setup.errors }}
            </p>
            <RouterLink
              :to="{ name: 'project-validation', params: { scenarioId: project.scenarioId } }"
            >
              {{ messages.validationWorkspace.heading }}
            </RouterLink>
          </li>
        </ol>
        <section class="state-panel setup-step" aria-labelledby="setup-calendar">
          <h3 id="setup-calendar" tabindex="-1">{{ messages.setup.calendarSummary }}</h3>
          <p>{{ snapshot.facts.settings.timeZone }}</p>
          <p>
            {{ horizon(snapshot.facts.settings.horizon.start) }} –
            {{ horizon(snapshot.facts.settings.horizon.end) }}
          </p>
          <p v-if="horizonFormatter === null" role="status">{{ messages.setup.zoneUnavailable }}</p>
          <RouterLink :to="{ name: 'project-work', params: { scenarioId: project.scenarioId } }">
            {{ messages.setup.calendar }}
          </RouterLink>
        </section>
        <details id="setup-calendar-details" class="state-panel setup-more">
          <summary>{{ messages.setup.calendarDetails }}</summary>
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
            <template v-for="group in groups" :key="group.id">
              <div
                v-for="count in snapshot.facts.entities.filter((item) =>
                  group.kinds.includes(item.kind),
                )"
                :key="count.kind"
              >
                <dt>{{ messages.setup.entityKinds[count.kind] }}</dt>
                <dd>{{ formatNumber(count.count, locale) }}</dd>
              </div>
            </template>
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
          <p>{{ messages.setup.countsBoundary }}</p>
        </details>
        <details class="state-panel setup-more">
          <summary>{{ messages.setup.validationDetails }}</summary>
          <ValidationSummary
            :findings="snapshot.summary.fast"
            state="ready"
            interaction="static"
            presentation="embedded"
            :heading="messages.setup.fast"
            :heading-level="4"
            :locale="locale"
          />
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
        </details>
        <p v-if="snapshot.results.solutions.length">
          {{
            messages.setup.existingResults(formatNumber(snapshot.results.solutions.length, locale))
          }}
        </p>
        <p v-if="snapshot.results.solutions.some((result) => result.stale)" role="status">
          {{ messages.setup.staleResult }}
        </p>
        <details id="setup-results-details" class="state-panel setup-more">
          <summary>{{ messages.setup.resultsDetails }}</summary>
          <OptimizeCapabilityPanel
            :summary="snapshot.summary"
            :current-revision="project.revision"
            :capabilities="capabilities"
            :loading="capabilitiesLoading"
            :error="capabilitiesError"
            @retry="retryCapabilities"
          />
          <section aria-labelledby="setup-results">
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
        </details>
      </template>
    </template>
  </section>
</template>
