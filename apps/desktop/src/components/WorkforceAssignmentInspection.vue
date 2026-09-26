<script setup lang="ts">
import { nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import { getScenarioView, SetupOperationScope } from "../api/generated";
import type {
  WorkforceSetupAssignmentInspection,
  WorkforceSetupFacts,
  WorkforceSetupOrdinalContinuation,
  WorkforceSetupRejectionCause,
  WorkforceSetupTimedShiftContinuation,
  WorkforceSetupWorkPage,
  WorkforceSetupWorkShift,
} from "../api/generated-domain-pack-contracts";
import {
  isOperationCancelled,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";
import { formatNumber, messages } from "../messages";
import WorkShiftTable from "./WorkShiftTable.vue";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  personId: string | null;
  locale?: string;
}>();
const copy = messages.assignmentInspection;
const heading = ref<HTMLElement>();
const resultHeading = ref<HTMLElement>();
const facts = shallowRef<WorkforceSetupFacts | null>(null);
const dates = ref<{ startDate: string; endDateExclusive: string } | null>(null);
const work = shallowRef<WorkforceSetupWorkPage | null>(null);
const selected = shallowRef<WorkforceSetupWorkShift | null>(null);
const inspection = shallowRef<WorkforceSetupAssignmentInspection | null>(null);
const loading = ref(false);
const inspecting = ref(false);
const error = ref<string | null>(null);
const inspectionError = ref<string | null>(null);
const cancelRequested = ref(false);
const cancelled = ref(false);
let alive = true;
let workGeneration = 0;
let inspectionGeneration = 0;
let workScope: SetupOperationScope | null = null;
let inspectionScope: SetupOperationScope | null = null;
let cancelCurrent: (() => Promise<unknown>) | null = null;

interface InspectionContext {
  readonly scenarioId: string;
  readonly revision: number;
  readonly epoch: number;
}

function capture(): InspectionContext {
  return {
    scenarioId: props.project.scenarioId,
    revision: props.project.revision,
    epoch: props.home.state.libraryEpoch,
  };
}
function current(context: InspectionContext): boolean {
  return (
    alive &&
    context.scenarioId === props.project.scenarioId &&
    context.revision === props.project.revision &&
    context.epoch === props.home.state.libraryEpoch
  );
}
function clearInspection(): void {
  inspectionGeneration += 1;
  inspectionScope?.dispose();
  inspectionScope = null;
  cancelCurrent = null;
  inspection.value = null;
  inspectionError.value = null;
  inspecting.value = false;
  cancelled.value = false;
  cancelRequested.value = false;
}
function clearWork(): void {
  workGeneration += 1;
  workScope?.dispose();
  workScope = null;
  work.value = null;
  selected.value = null;
  loading.value = false;
  error.value = null;
  clearInspection();
}
async function loadAssignments(
  cursor: WorkforceSetupTimedShiftContinuation | null = null,
  focus = false,
): Promise<void> {
  if (!alive || props.personId === null || props.project.domainPackId !== "official.workforce")
    return;
  if (focus) heading.value?.focus();
  clearWork();
  const generation = workGeneration;
  const context = capture();
  const owned = new SetupOperationScope(context.scenarioId, context.revision);
  workScope = owned;
  loading.value = true;
  try {
    if (facts.value === null) {
      const response = await getScenarioView(owned, {
        source: { kind: "stored" },
        query: { schemaVersion: 1, viewId: "official.workforce.setup.overview", parameters: {} },
      }).result;
      if (generation !== workGeneration || !current(context)) return;
      facts.value = response.result.view.data.result.data;
      dates.value ??= { ...facts.value.initialWorkWindow };
    }
    if (dates.value === null) return;
    const response = await getScenarioView(owned, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId: "official.workforce.setup.work_window",
        parameters: { dates: { ...dates.value }, limit: 20 },
        ...(cursor === null ? {} : { continuation: cursor }),
      },
    }).result;
    if (generation !== workGeneration || !current(context)) return;
    const result = response.result.view.data.result.data;
    if (
      result.items.length > 20 ||
      new Set(result.items.map((row) => row.shiftId)).size !== result.items.length
    )
      throw new Error("The native assignment page did not match its identity or size bounds.");
    work.value = result;
  } catch (failure) {
    if (generation === workGeneration && current(context)) error.value = safeMessage(failure);
  } finally {
    owned.dispose();
    if (workScope === owned) workScope = null;
    if (generation === workGeneration) loading.value = false;
  }
}
async function inspect(cursor: WorkforceSetupOrdinalContinuation | null = null): Promise<void> {
  const shift = selected.value;
  const personId = props.personId;
  if (!alive || shift === null || personId === null) return;
  const origin = document.activeElement;
  // Reinspection removes its own button; move focus before that render.
  resultHeading.value?.focus();
  clearInspection();
  const generation = inspectionGeneration;
  const context = capture();
  const owned = new SetupOperationScope(context.scenarioId, context.revision);
  inspectionScope = owned;
  inspecting.value = true;
  try {
    // Paint the new selection before starting the heavier native pair analysis.
    await nextTick();
    if (generation !== inspectionGeneration || !current(context) || props.personId !== personId)
      return;
    if (document.activeElement === origin) resultHeading.value?.focus();
    const operation = getScenarioView(owned, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId: "official.workforce.setup.assignment_inspection",
        parameters: { personId, shiftId: shift.shiftId, limit: 20 },
        ...(cursor === null ? {} : { continuation: cursor }),
      },
    });
    cancelCurrent = () => operation.cancel();
    const response = await operation.result;
    if (generation !== inspectionGeneration || !current(context) || props.personId !== personId)
      return;
    const result = response.result.view.data.result.data;
    if (
      result.personId !== personId ||
      result.shiftId !== shift.shiftId ||
      result.rejections.items.length > 20
    )
      throw new Error(
        "The native inspection did not match the selected person, assignment or page bound.",
      );
    inspection.value = result;
  } catch (failure) {
    if (generation === inspectionGeneration && current(context)) {
      if (isOperationCancelled(failure)) cancelled.value = true;
      else inspectionError.value = safeMessage(failure);
    }
  } finally {
    owned.dispose();
    if (inspectionScope === owned) inspectionScope = null;
    if (generation === inspectionGeneration) {
      inspecting.value = false;
      cancelCurrent = null;
    }
  }
}
function selectShift(id: string): void {
  const shift = work.value?.items.find((row) => row.shiftId === id);
  if (shift === undefined) return;
  selected.value = shift;
  void inspect();
}
async function cancel(): Promise<void> {
  const operation = cancelCurrent;
  if (operation === null || cancelRequested.value) return;
  const generation = inspectionGeneration;
  resultHeading.value?.focus();
  cancelRequested.value = true;
  try {
    await operation();
    // Only operation.result decides whether cancellation actually took effect.
  } catch (failure) {
    if (generation === inspectionGeneration && inspecting.value) {
      cancelRequested.value = false;
      inspectionError.value = safeMessage(failure);
    }
  }
}
function bindingLabel(cause: WorkforceSetupRejectionCause): string {
  if (cause.kind === "outsideActiveRange") return copy.person;
  if (cause.kind === "approvedTimeOff") return copy.record;
  return copy.rule;
}
function details(cause: WorkforceSetupRejectionCause): readonly { label: string; value: string }[] {
  switch (cause.kind) {
    case "outsideActiveRange":
      return [
        { label: copy.allowedStart, value: cause.allowed.start },
        { label: copy.allowedEnd, value: cause.allowed.end },
        { label: copy.outsideStart, value: cause.outside.start },
        { label: copy.outsideEnd, value: cause.outside.end },
      ];
    case "assignmentTypeNotAllowed":
    case "qualificationExpression":
      return [{ label: copy.assignmentType, value: cause.assignmentTypeId }];
    case "unavailable":
    case "approvedTimeOff":
      return [
        { label: copy.record, value: cause.availabilityId },
        { label: copy.overlapStart, value: cause.overlap.start },
        { label: copy.overlapEnd, value: cause.overlap.end },
      ];
    case "outsideAvailableOnly":
      return [
        { label: copy.record, value: cause.availabilityId },
        { label: copy.uncoveredStart, value: cause.uncovered.start },
        { label: copy.uncoveredEnd, value: cause.uncovered.end },
      ];
  }
}
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
  ],
  () => {
    clearWork();
    facts.value = null;
    dates.value = null;
    void loadAssignments();
  },
  { immediate: true, flush: "sync" },
);
watch(
  () => props.personId,
  (personId) => {
    clearInspection();
    if (personId === null) clearWork();
    else if (work.value === null && !loading.value) void loadAssignments();
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  alive = false;
  clearWork();
});
defineExpose({ focus: () => heading.value?.focus() });
</script>

<template>
  <section class="state-panel field-stack" aria-labelledby="assignment-inspection-heading">
    <h3 id="assignment-inspection-heading" ref="heading" tabindex="-1">{{ copy.heading }}</h3>
    <p>{{ copy.description }}</p>
    <p v-if="personId === null">{{ copy.selectPerson }}</p>
    <template v-else>
      <p>
        {{ copy.person }}: <code class="break-all">{{ personId }}</code>
      </p>
      <form
        v-if="dates !== null && facts !== null"
        class="field-stack"
        @submit.prevent="loadAssignments(null, true)"
      >
        <fieldset class="field-stack">
          <legend>{{ copy.dates }} · {{ facts.settings.timeZone }}</legend>
          <label for="inspection-start-date">{{ copy.startDate }}</label>
          <input
            id="inspection-start-date"
            v-model="dates.startDate"
            type="date"
            required
            aria-describedby="inspection-date-help"
            @input="clearWork"
          />
          <label for="inspection-end-date">{{ copy.endDate }}</label>
          <input
            id="inspection-end-date"
            v-model="dates.endDateExclusive"
            type="date"
            required
            aria-describedby="inspection-date-help"
            @input="clearWork"
          />
          <p id="inspection-date-help" class="field-help">{{ copy.dateHelp }}</p>
          <button type="submit" :disabled="loading">{{ copy.load }}</button>
        </fieldset>
      </form>
      <p v-if="loading" role="status">{{ copy.loading }}</p>
      <p v-if="error" role="alert">{{ error }}</p>
      <button v-if="error" type="button" @click="loadAssignments(null, true)">
        {{ messages.projects.retry }}
      </button>
      <template v-if="work !== null && facts !== null">
        <p>{{ copy.count(formatNumber(work.totalItems, locale)) }}</p>
        <WorkShiftTable
          :rows="work.items"
          :time-zone="facts.settings.timeZone"
          v-bind="locale === undefined ? {} : { locale }"
          @inspect="selectShift"
        />
        <div class="action-row">
          <button type="button" @click="loadAssignments(null, true)">{{ copy.first }}</button>
          <button
            type="button"
            :disabled="work.continuation === null"
            @click="loadAssignments(work.continuation, true)"
          >
            {{ copy.next }}
          </button>
        </div>
      </template>
      <section
        v-if="selected !== null"
        class="field-stack"
        aria-labelledby="assignment-inspection-result"
      >
        <h4 id="assignment-inspection-result" ref="resultHeading" tabindex="-1">
          {{ copy.result }}
        </h4>
        <p>
          {{ selected.assignmentTypeName }} · {{ selected.interval.startsAt.local }} ·
          <code class="break-all">{{ selected.shiftId }}</code>
        </p>
        <p v-if="inspecting" role="status">
          {{ cancelRequested ? copy.cancelling : copy.inspecting }}
        </p>
        <p v-if="cancelled" role="status">{{ copy.cancelled }}</p>
        <p v-if="inspectionError" role="alert">{{ inspectionError }}</p>
        <button v-if="inspecting" type="button" :disabled="cancelRequested" @click="cancel">
          {{ copy.cancel }}
        </button>
        <button v-else type="button" @click="inspect()">{{ copy.inspectAgain }}</button>
        <template v-if="inspection !== null">
          <p v-if="cancelRequested" role="status">{{ copy.completedBeforeCancellation }}</p>
          <p role="status">
            {{ inspection.candidateInImplementedAssignmentGraph ? copy.passed : copy.blocked }}
          </p>
          <p>{{ copy.remaining(formatNumber(inspection.remainingRequiredRuleCount, locale)) }}</p>
          <p>{{ copy.rejectionCount(formatNumber(inspection.rejections.totalItems, locale)) }}</p>
          <ul class="field-stack">
            <li
              v-for="finding in inspection.rejections.items"
              :key="finding.ordinal"
              class="state-panel"
            >
              <p>{{ copy.causes[finding.cause.kind] }}</p>
              <dl class="metadata-list">
                <div>
                  <dt>{{ bindingLabel(finding.cause) }}</dt>
                  <dd>
                    <code class="break-all">{{ finding.bindingId }}</code>
                  </dd>
                </div>
                <div v-for="detail in details(finding.cause)" :key="detail.label">
                  <dt>{{ detail.label }}</dt>
                  <dd>
                    <code class="break-all">{{ detail.value }}</code>
                  </dd>
                </div>
              </dl>
            </li>
          </ul>
          <div class="action-row">
            <button type="button" @click="inspect()">{{ copy.firstFinding }}</button>
            <button
              type="button"
              :disabled="inspection.rejections.continuation === null"
              @click="inspect(inspection.rejections.continuation)"
            >
              {{ copy.nextFinding }}
            </button>
          </div>
        </template>
      </section>
    </template>
  </section>
</template>
