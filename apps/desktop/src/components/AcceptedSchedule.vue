<script setup lang="ts">
import { nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import {
  getSolutionView,
  getAppCapabilities,
  type Revision,
  type SolutionSummaryDtoV1,
  type UuidV7,
} from "../api/generated";
import {
  isWorkforceResultScheduleViewData,
  WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
  type WorkforceResultScheduleAssignmentRow,
  type WorkforceResultScheduleContinuation,
  type WorkforceResultHealthRow,
  type WorkforceResultScheduleData,
  type WorkforceResultScheduleShiftRow,
} from "../api/generated-domain-pack-contracts";
import { messages } from "../messages";
import AcceptedCalendar from "./AcceptedCalendar.vue";
import AcceptedAssignmentDetails from "./AcceptedAssignmentDetails.vue";
import AcceptedResultSummary from "./AcceptedResultSummary.vue";
import RecipientShare from "./RecipientShare.vue";
import ScheduleExport from "./ScheduleExport.vue";
import { safeMessage, type ProjectHomeController } from "../project-home";
import type { ConflictRuleAction } from "./explanations/types";

const props = defineProps<{
  scenarioId: UuidV7;
  summary: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision"> &
    Partial<Pick<SolutionSummaryDtoV1, "verificationChecksum" | "status">>;
  currentRevision: Revision;
  home?: ProjectHomeController | undefined;
  ruleNavigationAvailable?: boolean;
}>();
const emit = defineEmits<{ ruleAction: [action: ConflictRuleAction] }>();
type SchedulePage = Exclude<
  WorkforceResultScheduleData,
  { readonly kind: "decision" | "repairDiff" }
>;
type Kind = SchedulePage["kind"];
const kind = ref<Kind>("people");
const personId = ref<UuidV7 | null>(null);
const shiftId = ref<UuidV7 | null>(null);
const locationId = ref<UuidV7 | null>(null);
const calendarStartDate = ref<string | null>(null);
const localDate = ref<string | null>(null);
const page = shallowRef<SchedulePage | null>(null);
const sourceTimeZone = ref<string | null>(null);
const filterLabel = ref<string | null>(null);
const locationLabel = ref<string | null>(null);
const cursors = shallowRef<readonly (WorkforceResultScheduleContinuation | null)[]>([null]);
const pageNumber = ref(0);
const requestedPageNumber = ref(0);
const loading = ref(false);
const error = ref<string | null>(null);
const heading = ref<HTMLElement | null>(null);
const inspected = shallowRef<WorkforceResultScheduleAssignmentRow | null>(null);
const inspector = ref<{ focusHeading(): void } | null>(null);
let inspectorTrigger: HTMLButtonElement | null = null;
let generation = 0;
let alive = true;
const copy = messages.acceptedSchedule;
const sharingAvailable = ref(false);
const exportingAvailable = ref(false);
const scheduleExport = ref<{
  openForPerson(personId: UuidV7, label: string, trigger: HTMLButtonElement | null): void;
} | null>(null);
watch(
  () => props.home,
  async (home) => {
    sharingAvailable.value = false;
    exportingAvailable.value = false;
    if (!home) return;
    try {
      const { result } = await getAppCapabilities();
      if (alive && props.home === home) {
        sharingAvailable.value = (
          ["solution_share_preview", "solution_share_create", "solution_share_release"] as const
        ).every((command) => result.availableCommands.includes(command));
        exportingAvailable.value = (
          [
            "solution_export_preview",
            "solution_export",
            "solution_export_cancel",
            "solution_export_release",
          ] as const
        ).every((command) => result.availableCommands.includes(command));
      }
    } catch {
      // No native capability evidence means no sharing or export authority.
    }
  },
  { immediate: true },
);

function localTime(value: { readonly local: string; readonly offsetSeconds: number }): string {
  const seconds = Math.abs(value.offsetSeconds);
  const hours = String(Math.floor(seconds / 3600)).padStart(2, "0");
  const minutes = String(Math.floor((seconds % 3600) / 60)).padStart(2, "0");
  const remainder = seconds % 60;
  const offset = `${value.offsetSeconds < 0 ? "−" : "+"}${hours}:${minutes}${remainder ? `:${String(remainder).padStart(2, "0")}` : ""}`;
  return `${value.local} UTC${offset}`;
}
function exactValue(value: { readonly numerator: string; readonly denominator: string }): string {
  return value.denominator === "1" ? value.numerator : `${value.numerator}/${value.denominator}`;
}

function coverageDescription(shift: WorkforceResultScheduleShiftRow): string {
  if (shift.selectedCount < shift.minimum)
    return copy.coverage.belowMinimum(shift.minimum - shift.selectedCount);
  if (shift.maximum !== null && shift.selectedCount > shift.maximum)
    return copy.coverage.aboveMaximum(shift.selectedCount - shift.maximum);
  if (shift.preferred !== null && shift.selectedCount < shift.preferred)
    return copy.coverage.belowPreferred(shift.preferred - shift.selectedCount);
  if (shift.preferred !== null && shift.selectedCount > shift.preferred)
    return copy.coverage.abovePreferred(shift.selectedCount - shift.preferred);
  return copy.coverage.within;
}
function healthSource(health: WorkforceResultHealthRow): string {
  if (health.sourceRuleId !== null) return `${copy.health.rule} ${health.sourceRuleId}`;
  if (health.coverageOwnerId !== null && health.coverageOwnerKind !== null)
    return `${copy.health.ownerLabels[health.coverageOwnerKind]} ${health.coverageOwnerId}`;
  return copy.notAvailable;
}

async function load(index: number): Promise<void> {
  requestedPageNumber.value = index;
  const token = ++generation;
  const scenarioId = props.scenarioId;
  const solutionId = props.summary.result.solutionId;
  const checksum = props.summary.result.resultChecksum;
  const sourceRevision = props.summary.scenarioRevision;
  const revision = props.currentRevision;
  const requestedKind = kind.value;
  const continuation = cursors.value[index] ?? null;
  const requestedStart = calendarStartDate.value;
  const requestedLocalDate = localDate.value;
  loading.value = true;
  error.value = null;
  try {
    const { result } = await getSolutionView({
      scenarioId,
      solutionId,
      viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
      parameters: {
        kind: requestedKind,
        limit: 50,
        ...(personId.value === null ? {} : { personId: personId.value }),
        ...(shiftId.value === null ? {} : { shiftId: shiftId.value }),
        ...(locationId.value === null ? {} : { locationId: locationId.value }),
        ...(requestedStart === null ||
        (requestedKind !== "calendarPeople" && requestedKind !== "calendarShifts")
          ? {}
          : { startDate: requestedStart }),
        ...(requestedKind === "assignments" && requestedLocalDate !== null
          ? { localDate: requestedLocalDate }
          : {}),
      },
      continuation,
    });
    if (
      !alive ||
      token !== generation ||
      props.scenarioId !== scenarioId ||
      props.summary.scenarioRevision !== sourceRevision ||
      props.currentRevision !== revision ||
      props.summary.result.solutionId !== solutionId ||
      props.summary.result.resultChecksum !== checksum
    )
      return;
    if (
      result.scenarioId !== scenarioId ||
      result.currentRevision !== revision ||
      result.scenarioRevision !== sourceRevision ||
      result.result.solutionId !== solutionId ||
      result.result.resultChecksum !== checksum ||
      result.view.viewId !== WORKFORCE_RESULT_SCHEDULE_VIEW_ID ||
      !isWorkforceResultScheduleViewData(result.view.data) ||
      result.view.data.result.kind !== requestedKind ||
      typeof result.view.data.timeZone !== "string" ||
      result.view.data.timeZone.length === 0
    ) {
      error.value = copy.sourceChanged;
      return;
    }
    const returned = result.view.data.result;
    if (
      (returned.kind === "calendarPeople" || returned.kind === "calendarShifts") &&
      requestedStart !== null &&
      returned.window.startDate !== requestedStart
    ) {
      error.value = copy.sourceChanged;
      return;
    }
    page.value = returned;
    sourceTimeZone.value = result.view.data.timeZone;
    pageNumber.value = index;
    const next = page.value.continuation;
    cursors.value =
      next === null
        ? cursors.value.slice(0, index + 1)
        : [...cursors.value.slice(0, index + 1), next];
  } catch (failure) {
    if (alive && token === generation) error.value = safeMessage(failure);
  } finally {
    if (alive && token === generation) loading.value = false;
  }
}

function reset(): void {
  inspected.value = null;
  inspectorTrigger = null;
  generation += 1;
  page.value = null;
  sourceTimeZone.value = null;
  cursors.value = [null];
  requestedPageNumber.value = 0;
  pageNumber.value = 0;
  void load(0);
}

function showAssignments(filter: {
  personId?: UuidV7;
  shiftId?: UuidV7;
  localDate?: string;
  label?: string;
}): void {
  kind.value =
    filter.localDate === undefined &&
    filter.shiftId === undefined &&
    (filter.personId !== undefined || kind.value === "timeline")
      ? "timeline"
      : "assignments";
  personId.value = filter.personId ?? null;
  shiftId.value = filter.shiftId ?? null;
  localDate.value = filter.localDate ?? null;
  filterLabel.value = filter.label ?? null;
  locationId.value = null;
  locationLabel.value = null;
  reset();
  const token = generation;
  void nextTick(() => {
    if (alive && token === generation) heading.value?.focus();
  });
}
function showLocation(id: UuidV7, label: string): void {
  kind.value = kind.value === "calendarShifts" ? "calendarShifts" : "shifts";
  locationId.value = id;
  locationLabel.value = label;
  personId.value = null;
  shiftId.value = null;
  filterLabel.value = null;
  reset();
  const token = generation;
  void nextTick(() => {
    if (alive && token === generation) heading.value?.focus();
  });
}

function navigateDates(days: -7 | 7): void {
  const shown = page.value;
  if (shown?.kind !== "calendarPeople" && shown?.kind !== "calendarShifts") return;
  const start = new Date(`${shown.window.startDate}T00:00:00Z`);
  if (!Number.isFinite(start.getTime())) return;
  start.setUTCDate(start.getUTCDate() + days);
  const next = start.toISOString().slice(0, 10);
  if (next < shown.planningDates.startDate || next >= shown.planningDates.endDateExclusive) return;
  heading.value?.focus();
  calendarStartDate.value = next;
  reset();
}

function inspectAssignment(
  assignment: WorkforceResultScheduleAssignmentRow,
  event: MouseEvent,
): void {
  inspected.value = assignment;
  inspectorTrigger = event.currentTarget instanceof HTMLButtonElement ? event.currentTarget : null;
  const token = generation;
  const assignmentId = assignment.assignmentId;
  void nextTick(() => {
    if (alive && token === generation && inspected.value?.assignmentId === assignmentId)
      inspector.value?.focusHeading();
  });
}
function closeInspection(): void {
  const returnTo = inspectorTrigger?.isConnected ? inspectorTrigger : heading.value;
  inspected.value = null;
  inspectorTrigger = null;
  const token = generation;
  void nextTick(() => {
    if (alive && token === generation) (returnTo?.isConnected ? returnTo : heading.value)?.focus();
  });
}

watch(
  [
    () => props.scenarioId,
    () => props.summary.result.solutionId,
    () => props.summary.result.resultChecksum,
    () => props.summary.scenarioRevision,
    () => props.currentRevision,
  ],
  () => {
    calendarStartDate.value = null;
    localDate.value = null;
    reset();
  },
  { immediate: true },
);
onScopeDispose(() => {
  alive = false;
  generation += 1;
});
</script>

<template>
  <section class="field-stack" :aria-label="copy.heading" data-accepted-schedule>
    <h4 ref="heading" tabindex="-1">{{ copy.heading }}</h4>
    <p v-if="summary.scenarioRevision !== currentRevision" role="status">{{ copy.stale }}</p>
    <AcceptedResultSummary
      :scenario-id="scenarioId"
      :summary="summary"
      :current-revision="currentRevision"
    />
    <div v-if="home" class="action-row">
      <RecipientShare
        v-if="sharingAvailable"
        :home="home"
        :scenario-id="scenarioId"
        :summary="summary"
        :current-revision="currentRevision"
      />
      <ScheduleExport
        v-if="exportingAvailable"
        ref="scheduleExport"
        :home="home"
        :scenario-id="scenarioId"
        :summary="summary"
        :current-revision="currentRevision"
      />
    </div>
    <p v-if="sourceTimeZone">{{ copy.timeZone }}: {{ sourceTimeZone }}</p>
    <fieldset class="result-view-choices">
      <legend>{{ copy.view }}</legend>
      <label
        v-for="option in [
          'people',
          'shifts',
          'timeline',
          'calendarPeople',
          'calendarShifts',
          'assignments',
          'fairness',
          'baseDiff',
          'health',
        ] as const"
        :key="option"
      >
        <input
          v-model="kind"
          type="radio"
          :name="`result-view-${summary.result.solutionId}`"
          :value="option"
          @change="
            personId = null;
            shiftId = null;
            filterLabel = null;
            localDate = null;
            calendarStartDate = null;
            locationId = null;
            locationLabel = null;
            reset();
          "
        />
        {{ copy.kinds[option] }}
      </label>
    </fieldset>
    <button
      v-if="
        (kind === 'assignments' || kind === 'timeline') && (personId !== null || shiftId !== null)
      "
      type="button"
      @click="showAssignments({})"
    >
      {{ copy.clearFilter }}
    </button>
    <p v-if="(kind === 'assignments' || kind === 'timeline') && personId !== null">
      {{ copy.personFilter }}: {{ filterLabel ?? personId }} ({{ personId }})
    </p>
    <p v-if="kind === 'assignments' && shiftId !== null">
      {{ copy.shiftFilter }}: {{ filterLabel ?? shiftId }} ({{ shiftId }})
    </p>
    <p v-if="kind === 'assignments' && localDate !== null">
      {{ copy.localDateFilter }}: {{ localDate }}
    </p>
    <p v-if="(kind === 'shifts' || kind === 'calendarShifts') && locationId !== null">
      {{ copy.locationFilter }}: {{ locationLabel ?? locationId }} ({{ locationId }})
      <button
        type="button"
        @click="
          locationId = null;
          locationLabel = null;
          reset();
        "
      >
        {{ copy.clearLocation }}
      </button>
    </p>
    <p v-if="loading" role="status" aria-busy="true">{{ copy.loading }}</p>
    <div v-if="error" role="alert">
      <p>{{ error }}</p>
      <button type="button" @click="load(requestedPageNumber)">{{ copy.retry }}</button>
    </div>
    <template v-if="page">
      <p v-if="page.kind === 'baseDiff'">{{ copy.baseDiff.description }}</p>
      <p v-if="page.kind === 'baseDiff' && page.baseSchedule === null" role="status">
        {{ copy.baseDiff.noBase }}
      </p>
      <p v-if="page.kind === 'baseDiff' && page.baseSchedule !== null">
        {{ copy.baseDiff.source }}: <code>{{ page.baseSchedule.sourceSolutionId }}</code> ·
        {{ copy.baseDiff.sourceRevision }} {{ page.baseSchedule.sourceRevision }}
      </p>
      <p role="status">{{ copy.count(page.totalItems, pageNumber + 1) }}</p>
      <p v-if="page.kind === 'health'">{{ copy.health.description }}</p>
      <p
        v-if="
          page.totalItems === 0 &&
          page.kind !== 'calendarPeople' &&
          page.kind !== 'calendarShifts' &&
          (page.kind !== 'baseDiff' || page.baseSchedule !== null)
        "
      >
        {{ copy.empty[page.kind] }}
      </p>
      <AcceptedCalendar
        v-if="(page.kind === 'calendarPeople' || page.kind === 'calendarShifts') && sourceTimeZone"
        :page="page"
        :time-zone="sourceTimeZone"
        :loading="loading"
        :local-time="localTime"
        :coverage-description="coverageDescription"
        @day="(id, name, date) => showAssignments({ personId: id, label: name, localDate: date })"
        @shift="(id, label) => showAssignments({ shiftId: id, label })"
        @location="showLocation"
        @navigate="navigateDates"
      />
      <div v-else-if="page.totalItems > 0" class="overflow-x-auto" :aria-busy="loading">
        <table v-if="page.kind === 'people'" class="data-table">
          <caption>
            {{
              copy.kinds.people
            }}
          </caption>
          <thead>
            <tr>
              <th scope="col">{{ copy.person }}</th>
              <th scope="col">{{ copy.selected }}</th>
              <th scope="col">{{ copy.actions }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="person in page.items" :key="person.personId">
              <th scope="row">{{ person.name }}</th>
              <td>{{ person.selectedCount }}</td>
              <td>
                <button
                  type="button"
                  @click="showAssignments({ personId: person.personId, label: person.name })"
                >
                  {{ copy.showPerson(person.name) }}
                </button>
                <button
                  v-if="home && exportingAvailable"
                  type="button"
                  :disabled="
                    loading || home.state.phase !== 'ready' || home.state.busyAction !== null
                  "
                  @click="
                    scheduleExport?.openForPerson(
                      person.personId,
                      person.name,
                      $event.currentTarget as HTMLButtonElement,
                    )
                  "
                >
                  {{ messages.scheduleExport.personAction(person.name) }}
                </button>
              </td>
            </tr>
          </tbody>
        </table>
        <table v-else-if="page.kind === 'shifts'" class="data-table">
          <caption>
            {{
              copy.kinds.shifts
            }}
          </caption>
          <thead>
            <tr>
              <th scope="col">{{ copy.shift }}</th>
              <th scope="col">{{ copy.location }}</th>
              <th scope="col">{{ copy.start }}</th>
              <th scope="col">{{ copy.end }}</th>
              <th scope="col">{{ copy.minimum }}</th>
              <th scope="col">{{ copy.coverage.preferred }}</th>
              <th scope="col">{{ copy.coverage.maximum }}</th>
              <th scope="col">{{ copy.selected }}</th>
              <th scope="col">{{ copy.margin }}</th>
              <th scope="col">{{ copy.coverage.standing }}</th>
              <th scope="col">{{ copy.actions }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="shift in page.items" :key="shift.shiftId">
              <th scope="row">{{ shift.assignmentTypeName ?? shift.assignmentTypeId }}</th>
              <td>
                {{ shift.locationName ?? copy.noLocation }}
                <button
                  v-if="shift.locationId !== null"
                  type="button"
                  @click="showLocation(shift.locationId, shift.locationName ?? shift.locationId)"
                >
                  {{ copy.showLocation(shift.locationName ?? shift.locationId) }}
                </button>
              </td>
              <td>{{ localTime(shift.startsAt) }}</td>
              <td>{{ localTime(shift.endsAt) }}</td>
              <td>{{ shift.minimum }}</td>
              <td>{{ shift.preferred ?? copy.notAvailable }}</td>
              <td>{{ shift.maximum ?? copy.notAvailable }}</td>
              <td>{{ shift.selectedCount }}</td>
              <td>{{ shift.margin ?? copy.notAvailable }}</td>
              <td>{{ coverageDescription(shift) }}</td>
              <td>
                <button
                  type="button"
                  :aria-label="
                    copy.showShift(
                      `${shift.assignmentTypeName ?? shift.assignmentTypeId} · ${localTime(shift.startsAt)} · ${shift.locationName ?? copy.noLocation} · ${shift.shiftId}`,
                    )
                  "
                  @click="
                    showAssignments({
                      shiftId: shift.shiftId,
                      label: `${shift.assignmentTypeName ?? shift.assignmentTypeId} · ${localTime(shift.startsAt)} · ${shift.locationName ?? copy.noLocation}`,
                    })
                  "
                >
                  {{ copy.showShift(shift.assignmentTypeName ?? shift.assignmentTypeId) }}
                </button>
              </td>
            </tr>
          </tbody>
        </table>
        <table
          v-else-if="page.kind === 'assignments' || page.kind === 'timeline'"
          class="data-table"
        >
          <caption>
            {{
              copy.kinds[page.kind]
            }}
          </caption>
          <thead>
            <tr>
              <th scope="col">{{ copy.person }}</th>
              <th scope="col">{{ copy.shift }}</th>
              <th scope="col">{{ copy.location }}</th>
              <th scope="col">{{ copy.start }}</th>
              <th scope="col">{{ copy.end }}</th>
              <th scope="col">{{ copy.inspection.column }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="assignment in page.items" :key="assignment.assignmentId">
              <th scope="row">{{ assignment.personName }}</th>
              <td>{{ assignment.assignmentTypeName ?? assignment.assignmentTypeId }}</td>
              <td>{{ assignment.locationName ?? copy.noLocation }}</td>
              <td>{{ localTime(assignment.startsAt) }}</td>
              <td>{{ localTime(assignment.endsAt) }}</td>
              <td>
                <button
                  type="button"
                  :aria-label="
                    copy.inspection.inspect(
                      assignment.personName,
                      assignment.assignmentTypeName ?? assignment.assignmentTypeId,
                      localTime(assignment.startsAt),
                    )
                  "
                  @click="inspectAssignment(assignment, $event)"
                >
                  {{ copy.inspection.open }}
                </button>
              </td>
            </tr>
          </tbody>
        </table>
        <table v-else-if="page.kind === 'fairness'" class="data-table">
          <caption>
            {{
              copy.kinds.fairness
            }}
          </caption>
          <thead>
            <tr>
              <th scope="col">{{ copy.fairness.policy }}</th>
              <th scope="col">{{ copy.fairness.period }}</th>
              <th scope="col">{{ copy.person }}</th>
              <th scope="col">{{ copy.fairness.measure }}</th>
              <th scope="col">{{ copy.fairness.target }}</th>
              <th scope="col">{{ copy.fairness.actual }}</th>
              <th scope="col">{{ copy.fairness.deviation }}</th>
              <th scope="col">{{ copy.fairness.groupActual }}</th>
              <th scope="col">{{ copy.fairness.groupDeviation }}</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="fairness in page.items"
              :key="`${fairness.preferenceId}-${fairness.policyId}-${fairness.periodStartsAt.instant}-${fairness.personId}`"
            >
              <th scope="row">
                {{ fairness.bucketName }} · {{ fairness.targetMode }} ·
                {{ fairness.policyId }}
              </th>
              <td>
                {{ localTime(fairness.periodStartsAt) }} –
                {{ localTime(fairness.periodEndsAt) }}
              </td>
              <td>{{ fairness.personName }} ({{ fairness.personId }})</td>
              <td>{{ fairness.measure }} ({{ fairness.units }})</td>
              <td>{{ exactValue(fairness.target) }}</td>
              <td>{{ exactValue(fairness.actual) }}</td>
              <td>{{ exactValue(fairness.deviation) }}</td>
              <td>
                {{ exactValue(fairness.groupActualMinimum) }} –
                {{ exactValue(fairness.groupActualMaximum) }}; {{ copy.fairness.spread }}:
                {{ exactValue(fairness.groupSpread) }}
                <span class="sr-only">{{ copy.fairness.spreadDetails }}</span>
              </td>
              <td>
                {{ exactValue(fairness.groupDeviationMinimum) }} –
                {{ exactValue(fairness.groupDeviationMaximum) }}; {{ copy.fairness.spread }}:
                {{ exactValue(fairness.groupDeviationSpread) }}
                <span class="sr-only">{{ copy.fairness.spreadDetails }}</span>
              </td>
            </tr>
          </tbody>
        </table>
        <table v-else-if="page.kind === 'baseDiff'" class="data-table">
          <caption>
            {{
              copy.kinds.baseDiff
            }}
          </caption>
          <thead>
            <tr>
              <th scope="col">{{ copy.baseDiff.change }}</th>
              <th scope="col">{{ copy.person }}</th>
              <th scope="col">{{ copy.shift }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="item in page.items" :key="`${item.personId}-${item.shiftId}-${item.change}`">
              <th scope="row">{{ copy.baseDiff[item.change] }}</th>
              <td>{{ item.personName }} ({{ item.personId }})</td>
              <td>{{ item.shiftLabel }} ({{ item.shiftId }})</td>
            </tr>
          </tbody>
        </table>
        <table v-else-if="page.kind === 'health'" class="data-table health-table">
          <caption>
            {{
              copy.kinds.health
            }}
          </caption>
          <thead>
            <tr>
              <th scope="col">{{ copy.health.check }}</th>
              <th scope="col">{{ copy.health.status }}</th>
              <th scope="col">{{ copy.health.margin }}</th>
              <th scope="col">{{ copy.health.source }}</th>
              <th scope="col">{{ copy.health.showEvidence }}</th>
              <th scope="col">{{ copy.health.affected }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(health, index) in page.items" :key="index">
              <th scope="row">
                {{ copy.health.checkLabels[health.check] }}
                <span v-if="health.minimumRank !== null">
                  · {{ copy.health.rank(health.minimumRank) }}
                </span>
              </th>
              <td>{{ copy.health.statusLabels[health.status] }}</td>
              <td>
                {{ health.margin === null ? copy.notAvailable : exactValue(health.margin) }}
                <span v-if="health.margin !== null">{{
                  copy.health.unitsLabels[health.units]
                }}</span>
              </td>
              <td>{{ healthSource(health) }}</td>
              <td>
                <details>
                  <summary>{{ copy.health.showEvidence }}</summary>
                  <dl>
                    <div>
                      <dt>{{ copy.health.direction }}</dt>
                      <dd>{{ copy.health.directionLabels[health.direction] }}</dd>
                    </div>
                    <div>
                      <dt>{{ copy.health.threshold }}</dt>
                      <dd>
                        {{
                          health.threshold === null
                            ? copy.notAvailable
                            : exactValue(health.threshold)
                        }}
                      </dd>
                    </div>
                    <div>
                      <dt>{{ copy.health.observed }}</dt>
                      <dd>
                        {{
                          health.observed === null ? copy.notAvailable : exactValue(health.observed)
                        }}
                      </dd>
                    </div>
                    <div>
                      <dt>{{ copy.health.units }}</dt>
                      <dd>{{ copy.health.unitsLabels[health.units] }}</dd>
                    </div>
                  </dl>
                  <p>{{ copy.health.context }}</p>
                  <p v-if="health.personId !== null">
                    {{ copy.health.person }}: {{ health.personId }}
                  </p>
                  <p v-if="health.shiftId !== null">
                    {{ copy.health.shift }}: {{ health.shiftId }}
                  </p>
                  <p v-if="health.otherShiftId !== null">
                    {{ copy.health.otherShift }}: {{ health.otherShiftId }}
                  </p>
                  <p v-if="health.windowStartsAt !== null && health.windowEndsAt !== null">
                    {{ copy.health.interval }}: {{ health.windowStartsAt }} –
                    {{ health.windowEndsAt }}
                  </p>
                  <p
                    v-if="
                      health.personId === null &&
                      health.shiftId === null &&
                      health.otherShiftId === null &&
                      health.windowStartsAt === null
                    "
                  >
                    {{ copy.notAvailable }}
                  </p>
                </details>
              </td>
              <td>
                <details v-if="health.affectedAssignmentIds.length">
                  <summary>
                    {{
                      copy.health.assignmentIds(
                        health.affectedAssignmentIds.length,
                        health.affectedAssignmentCount,
                      )
                    }}
                  </summary>
                  <ul>
                    <li v-for="assignmentId in health.affectedAssignmentIds" :key="assignmentId">
                      {{ assignmentId }}
                    </li>
                  </ul>
                </details>
                <span v-else>
                  {{ copy.health.assignmentIds(0, health.affectedAssignmentCount) }}
                </span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="action-row">
        <button type="button" :disabled="loading || pageNumber === 0" @click="load(pageNumber - 1)">
          {{ copy.previous }}
        </button>
        <button
          type="button"
          :disabled="loading || page.continuation === null"
          @click="load(pageNumber + 1)"
        >
          {{ copy.next }}
        </button>
      </div>
      <AcceptedAssignmentDetails
        v-if="inspected"
        ref="inspector"
        :scenario-id="scenarioId"
        :summary="summary"
        :current-revision="currentRevision"
        :assignment="inspected"
        :start-label="localTime(inspected.startsAt)"
        :end-label="localTime(inspected.endsAt)"
        :home="home"
        :rule-navigation-available="ruleNavigationAvailable"
        @rule-action="emit('ruleAction', $event)"
        @close="closeInspection"
      />
    </template>
  </section>
</template>
