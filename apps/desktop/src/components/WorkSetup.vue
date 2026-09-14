<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type { FieldErrorDto } from "../api/generated";
import type {
  WorkforceSetupEntityContinuation,
  WorkforceSetupTimedShiftContinuation,
  WorkforceSetupGenerationRow,
} from "../api/generated-domain-pack-contracts";
import { formatNumber, messages } from "../messages";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import { useWorkSetup, type WorkSettingsDraft } from "../work-setup";
import {
  createWorkRecordDraft,
  isWorkRecord,
  type WorkRecord,
  type WorkRecordDraft,
} from "../work-record-draft";
import type { WorkReviewSide } from "../work-command-review";
import type { EntityField, EntityRecord } from "../entity-draft";
import { formatWorkOffset, formatWorkOrigin } from "../work-display";
import WorkRecordFields from "./WorkRecordFields.vue";
import WorkShiftTable from "./WorkShiftTable.vue";
import WorkGenerationTable from "./WorkGenerationTable.vue";
import WorkSettingsFacts from "./WorkSettingsFacts.vue";
import WorkResolvedEndpoints from "./WorkResolvedEndpoints.vue";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";

const props = defineProps<{
  readonly home: ProjectHomeController;
  readonly project: ProjectSummary;
  readonly locale?: string;
}>();
const vm = useWorkSetup(props.home, () => props.project);
const {
  state,
  review,
  facts,
  busy,
  dirty,
  stale,
  requestKey,
  activeRaw,
  canReplace,
  currentRecord,
  conflictsCurrent,
  diagnostics,
} = vm;
const copy = messages.work;
const recordsCopy = messages.people;
const editorHeading = ref<HTMLElement>();
const listHeading = ref<HTMLElement>();
const workHeading = ref<HTMLElement>();
const reviewHeading = ref<HTMLElement>();
const shiftInspectionHeading = ref<HTMLElement>();
const reviewInspectionHeading = ref<HTMLElement>();
const currentHeading = ref<HTMLElement>();
const editorHost = ref<HTMLElement>();
const form = ref<HTMLFormElement>();
const showErrors = ref(false);
const showCurrent = ref(false);
const warningsPage = ref(0);
const diagnosticsPage = ref(0);
const endpoints = ["startsAt", "endsAt"] as const;
const displayContext = computed(() => (props.locale === undefined ? {} : { locale: props.locale }));
const fieldContext = computed(() => ({
  project: props.project,
  libraryEpoch: props.home.state.libraryEpoch,
  requestKey: requestKey.value,
  ...displayContext.value,
}));
const editableFieldContext = computed(() => ({
  ...fieldContext.value,
  draftReferences:
    state.editor?.kind === "preset"
      ? state.editor.entries.map(({ original }) => ({ kind: original.kind, id: original.id }))
      : [],
  ...(state.temporalFeedback === undefined ? {} : { temporalFeedback: state.temporalFeedback }),
}));
const currentRaw = computed(() =>
  currentRecord.value === null ? null : createWorkRecordDraft(currentRecord.value),
);
const inspectedRecord = computed(() => {
  const inspected = review.state.review?.inspection;
  if (
    inspected?.kind !== "entity" ||
    !isWorkRecord(inspected.entity) ||
    facts.value === null ||
    review.state.review === null
  )
    return null;
  return {
    raw: createWorkRecordDraft(inspected.entity),
    id: inspected.entity.id,
    origin: inspected.entity.kind === "shiftInstance" ? inspected.entity.origin : null,
    instance: inspected.entity.kind === "shiftInstance" ? inspected.entity : null,
    side: inspected.side,
    timeZone:
      inspected.side === "before"
        ? facts.value.settings.timeZone
        : review.state.review.sourceFacts.settings.timeZone,
  };
});
const warnings = computed(() => {
  const value = review.state.review?.warnings;
  return value === undefined
    ? []
    : [...value.changes, ...value.generation, ...value.sourceFacts, ...value.inspection];
});
const fieldLabels: Readonly<Record<string, string>> = {
  ...recordsCopy.fields,
  assignmentTypeId: copy.fields.assignmentTypeId,
  locationId: copy.fields.locationId,
  transitions: copy.fields.transitions,
  measurement: copy.fields.measurement,
  overlappingContribution: copy.fields.overlappingContribution,
  period: copy.fields.calendarPeriod,
  recurrence: copy.fields.effectiveRange,
  timing: copy.fields.timing,
  reportingAttribution: copy.fields.reportingAttribution,
  coverage: copy.fields.coverage,
  startsAt: copy.table.start,
  endsAt: copy.table.end,
  origin: copy.table.origin,
  timeZone: copy.timeZone,
  gapPolicy: copy.gapPolicy,
  overlapPolicy: copy.overlapPolicy,
  startDate: copy.fields.startDate,
  endDateExclusive: copy.fields.endDateExclusive,
};
function text(event: Event): string {
  return (event.target as HTMLInputElement | HTMLSelectElement).value;
}
function checked(event: Event): boolean {
  return (event.target as HTMLInputElement).checked;
}
function settingsError(
  field: "timeZone" | "startDate" | "endDateExclusive" | "gapPolicy" | "overlapPolicy",
): string | undefined {
  return (
    state.errors[field] ??
    (field === "startDate"
      ? (state.errors["dates.startDate"] ??
        state.errors["horizon.start"] ??
        state.errors.dates ??
        state.errors.horizon)
      : field === "endDateExclusive"
        ? (state.errors["dates.endDateExclusive"] ??
          state.errors["horizon.end"] ??
          state.errors.dates ??
          state.errors.horizon)
        : undefined)
  );
}
function setSetting(field: "timeZone" | "startDate" | "endDateExclusive", event: Event): void {
  if (state.editor?.kind === "settings")
    vm.updateSettings({ ...state.editor.raw, [field]: text(event) });
}
function setGap(event: Event): void {
  const value = text(event);
  if (
    state.editor?.kind === "settings" &&
    (value === "reject" || value === "moveForward" || value === "packDefined")
  )
    vm.updateSettings({ ...state.editor.raw, gapPolicy: value });
}
function setOverlap(event: Event): void {
  const value = text(event);
  if (
    state.editor?.kind === "settings" &&
    (value === "reject" || value === "earlier" || value === "later")
  )
    vm.updateSettings({ ...state.editor.raw, overlapPolicy: value });
}
function setKind(event: Event): void {
  const value = text(event);
  if (
    value === "location" ||
    value === "workloadBucket" ||
    value === "assignmentType" ||
    value === "calendar" ||
    value === "shiftTemplate" ||
    value === "shiftInstance"
  )
    vm.setRecordKind(value);
}
function setDate(field: "startDate" | "endDateExclusive", event: Event): void {
  if (state.dates !== null) vm.setDates({ ...state.dates, [field]: text(event) });
}
function update(raw: WorkRecordDraft): void {
  showErrors.value = false;
  vm.updateRaw(raw);
}
async function focusEditor(): Promise<void> {
  await nextTick();
  editorHeading.value?.focus();
}
async function openRecord(id: string, kind: WorkRecord["kind"]): Promise<void> {
  showErrors.value = false;
  showCurrent.value = false;
  if (await vm.loadRecord(id, kind)) await focusEditor();
}
async function createRecord(): Promise<void> {
  vm.createRecord();
  showErrors.value = false;
  showCurrent.value = false;
  await focusEditor();
}
async function editSettings(): Promise<void> {
  vm.editSettings();
  showErrors.value = false;
  showCurrent.value = false;
  await focusEditor();
}
async function createPreset(): Promise<void> {
  vm.createPreset();
  showErrors.value = false;
  showCurrent.value = false;
  await focusEditor();
}
async function closeEditor(): Promise<void> {
  vm.discard();
  showCurrent.value = false;
  await nextTick();
  listHeading.value?.focus();
}
async function focusError(): Promise<void> {
  await nextTick();
  const invalid = editorHost.value?.querySelector<HTMLElement>('[aria-invalid="true"]');
  if (invalid !== undefined && invalid !== null) invalid.focus();
  else editorHeading.value?.focus();
}
async function preview(): Promise<void> {
  showErrors.value = true;
  if (form.value?.reportValidity() === false) return;
  if (await vm.previewEditor()) {
    warningsPage.value = 0;
    await nextTick();
    reviewHeading.value?.focus();
  } else await focusError();
}
async function reviewAction(action: () => Promise<boolean>): Promise<void> {
  if (await action()) {
    warningsPage.value = 0;
    await nextTick();
    reviewHeading.value?.focus();
  }
}
async function pageRecords(cursor: WorkforceSetupEntityContinuation | null = null): Promise<void> {
  await vm.loadRecords(cursor);
  await nextTick();
  listHeading.value?.focus();
}
async function pageWork(cursor: WorkforceSetupTimedShiftContinuation | null = null): Promise<void> {
  await vm.loadWork(cursor);
  await nextTick();
  workHeading.value?.focus();
}
async function resetWindow(): Promise<void> {
  if (facts.value !== null) {
    vm.setDates({ ...facts.value.initialWorkWindow });
    await pageWork();
  }
}
async function inspectShift(id: string): Promise<void> {
  if (await vm.inspectShift(id)) {
    await nextTick();
    shiftInspectionHeading.value?.focus();
  }
}
async function openShiftSource(): Promise<void> {
  if (await vm.openShiftSource()) await focusEditor();
}
async function inspectGeneration(
  row: WorkforceSetupGenerationRow,
  side: WorkReviewSide,
): Promise<void> {
  if (await vm.inspectGeneration(row, side)) {
    await nextTick();
    reviewInspectionHeading.value?.focus();
  }
}
async function inspectChange(ordinal: number, side: WorkReviewSide): Promise<void> {
  if (await vm.inspectChange(ordinal, side)) {
    await nextTick();
    reviewInspectionHeading.value?.focus();
  }
}
async function inspectCurrent(): Promise<void> {
  showCurrent.value = true;
  await nextTick();
  currentHeading.value?.focus();
}
async function rebase(): Promise<void> {
  await vm.rebase();
  showErrors.value = Object.keys(state.errors).length !== 0;
  await focusEditor();
}
async function repair(issue: FieldErrorDto): Promise<void> {
  if (await vm.repairDiagnostic(issue)) {
    showErrors.value = true;
    await focusError();
  }
}
async function save(truncateRedo = false): Promise<void> {
  if (await vm.apply(truncateRedo)) {
    showCurrent.value = false;
    await nextTick();
    workHeading.value?.focus();
  }
}
function fieldSummary(value: EntityRecord, field: string): string {
  const item = (value as unknown as Readonly<Record<string, unknown>>)[field];
  if (item === null || item === undefined) return recordsCopy.absent;
  if (Array.isArray(item)) return `${formatNumber(item.length, props.locale)} ${recordsCopy.items}`;
  if (typeof item === "string") return item;
  if (typeof item === "number") return formatNumber(item, props.locale);
  if (typeof item === "boolean") return String(item);
  return recordsCopy.structuredField;
}
function chooseConflict(
  field: EntityField<WorkRecord> | EntityField<WorkSettingsDraft>,
  choice: "current" | "draft",
): void {
  vm.chooseConflict(field, choice);
}
watch(
  stale,
  (value) => {
    const focused = document.activeElement;
    if (value && focused instanceof HTMLElement && editorHost.value?.contains(focused))
      void nextTick(() => {
        if (document.activeElement === focused || document.activeElement === document.body)
          editorHeading.value?.focus();
      });
  },
  { flush: "sync" },
);
watch(diagnostics, () => {
  diagnosticsPage.value = 0;
});
</script>

<template>
  <section class="page-stack" aria-labelledby="work-heading">
    <h2 id="work-heading">{{ copy.heading }}</h2>
    <p v-if="project.domainPackId !== 'official.workforce'">{{ messages.setup.unsupported }}</p>
    <template v-else>
      <p>{{ copy.description }}</p>
      <div class="action-row">
        <button type="button" :disabled="busy || state.factsLoading" @click="vm.refresh">
          {{ copy.refresh }}
        </button>
        <button type="button" :disabled="!canReplace || !facts" @click="editSettings">
          {{ copy.editSettings }}
        </button>
        <button type="button" :disabled="!canReplace || !facts" @click="createPreset">
          {{ copy.presets.create }}
        </button>
        <button
          type="button"
          :disabled="!canReplace || !facts"
          @click="reviewAction(vm.regenerate)"
        >
          {{ copy.regenerate }}
        </button>
      </div>
      <p v-if="state.factsLoading" role="status">{{ recordsCopy.loading }}</p>
      <p v-if="state.factsError" class="state-panel" role="alert">{{ state.factsError }}</p>
      <details v-if="facts" class="state-panel">
        <summary>{{ copy.currentSettings }}</summary>
        <p>{{ copy.settingsHelp }}</p>
        <WorkSettingsFacts :facts="facts" />
      </details>

      <section class="state-panel field-stack" aria-labelledby="work-records-heading">
        <h3 id="work-records-heading" ref="listHeading" tabindex="-1">{{ copy.records }}</h3>
        <p id="work-record-search-help">{{ copy.searchHelp }}</p>
        <label for="work-record-kind">{{ copy.recordKind }}</label>
        <select id="work-record-kind" :value="state.recordKind" :disabled="busy" @change="setKind">
          <option v-for="(label, kind) in copy.kinds" :key="kind" :value="kind">{{ label }}</option>
        </select>
        <label for="work-record-search">{{ copy.search }}</label>
        <input
          id="work-record-search"
          type="search"
          :value="state.search"
          maxlength="256"
          :disabled="busy"
          aria-describedby="work-record-search-help"
          @input="vm.setSearch(text($event))"
        />
        <button type="button" :disabled="!canReplace || !facts" @click="createRecord">
          {{ recordsCopy.create }}: {{ copy.kinds[state.recordKind] }}
        </button>
        <p v-if="state.recordsLoading" role="status">{{ recordsCopy.loading }}</p>
        <p v-if="state.recordsError" role="alert">{{ state.recordsError }}</p>
        <template v-if="state.records">
          <p>{{ recordsCopy.matching }} {{ formatNumber(state.records.totalItems, locale) }}</p>
          <p v-if="state.records.items.length === 0">{{ recordsCopy.empty }}</p>
          <ul class="field-stack">
            <li v-for="item in state.records.items" :key="item.entityId">
              <button
                type="button"
                :disabled="!canReplace || state.detailLoading"
                @click="openRecord(item.entityId, state.recordKind)"
              >
                {{ item.name ?? item.entityId }}
              </button>
              <code class="break-all">{{ item.entityId }}</code>
            </li>
          </ul>
          <nav class="action-row" :aria-label="recordsCopy.pages">
            <button type="button" :disabled="busy || state.recordsLoading" @click="pageRecords()">
              {{ recordsCopy.first }}
            </button>
            <button
              type="button"
              :disabled="busy || state.recordsLoading || state.records.continuation === null"
              @click="pageRecords(state.records.continuation)"
            >
              {{ recordsCopy.next }}
            </button>
          </nav>
        </template>
      </section>

      <section class="state-panel field-stack" aria-labelledby="work-window-heading">
        <h3 id="work-window-heading" ref="workHeading" tabindex="-1">{{ copy.instances }}</h3>
        <p id="work-window-help">{{ copy.windowHelp }}</p>
        <form v-if="state.dates" class="field-stack" @submit.prevent="pageWork()">
          <label for="work-window-start">{{ copy.fields.startDate }}</label>
          <input
            id="work-window-start"
            type="date"
            required
            :value="state.dates.startDate"
            :disabled="busy"
            aria-describedby="work-window-help"
            @input="setDate('startDate', $event)"
          />
          <label for="work-window-end">{{ copy.fields.endDateExclusive }}</label>
          <input
            id="work-window-end"
            type="date"
            required
            :value="state.dates.endDateExclusive"
            :disabled="busy"
            aria-describedby="work-window-help"
            @input="setDate('endDateExclusive', $event)"
          />
          <div class="action-row">
            <button type="submit" :disabled="busy || !facts">{{ copy.loadWindow }}</button>
            <button type="button" :disabled="busy || !facts" @click="resetWindow">
              {{ copy.resetWindow }}
            </button>
          </div>
        </form>
        <p v-if="state.workLoading" role="status">{{ recordsCopy.loading }}</p>
        <p v-if="state.workError" role="alert">{{ state.workError }}</p>
        <template v-if="state.work && facts">
          <WorkShiftTable
            v-bind="displayContext"
            :rows="state.work.items"
            :time-zone="facts.settings.timeZone"
            :disabled="busy || state.detailLoading"
            @inspect="inspectShift"
          />
          <p>{{ recordsCopy.matching }} {{ formatNumber(state.work.totalItems, locale) }}</p>
          <nav class="action-row" :aria-label="copy.shiftPages">
            <button type="button" :disabled="busy" @click="pageWork()">
              {{ recordsCopy.first }}
            </button>
            <button
              type="button"
              :disabled="busy || state.work.continuation === null"
              @click="pageWork(state.work.continuation)"
            >
              {{ recordsCopy.next }}
            </button>
          </nav>
        </template>
      </section>
      <p v-if="state.detailLoading" role="status">{{ recordsCopy.loadingDetail }}</p>
      <div v-if="state.detailError" class="state-panel" role="alert">
        <p>{{ recordsCopy.detailUnavailable }}</p>
        <p>{{ state.detailError }}</p>
        <button
          v-if="state.editor?.kind === 'record'"
          type="button"
          :disabled="busy || state.detailLoading"
          @click="vm.loadRecord(state.editor.id, state.editor.raw.kind, true)"
        >
          {{ recordsCopy.retryDetail }}
        </button>
      </div>
      <section
        v-if="state.selectedWork && facts"
        class="state-panel field-stack"
        :aria-label="copy.inspection"
      >
        <h3 ref="shiftInspectionHeading" tabindex="-1">{{ copy.inspection }}</h3>
        <WorkShiftTable
          v-bind="displayContext"
          :rows="[state.selectedWork.value.shift]"
          :time-zone="facts.settings.timeZone"
          disabled
        />
        <button
          type="button"
          :disabled="!canReplace || state.detailLoading"
          @click="openShiftSource"
        >
          {{ copy.openSource }}
        </button>
        <template v-if="state.selectedWork.value.shift.origin.kind === 'generated'">
          <p>{{ copy.detachHelp }}</p>
          <button type="button" :disabled="!canReplace" @click="reviewAction(vm.detach)">
            {{ copy.detach }}
          </button>
        </template>
      </section>

      <section
        v-if="state.editor"
        ref="editorHost"
        class="state-panel field-stack"
        aria-labelledby="work-editor-heading"
      >
        <h3 id="work-editor-heading" ref="editorHeading" tabindex="-1">
          {{
            state.editor.kind === "settings"
              ? copy.settings
              : state.editor.kind === "preset"
                ? copy.presets.heading
                : copy.kinds[state.editor.raw.kind]
          }}
        </h3>
        <p v-if="state.editor.kind === 'record'" class="break-all">
          {{ recordsCopy.identity }} {{ state.editor.id }}
        </p>
        <p
          v-if="state.editor.kind === 'record' && state.editor.base?.kind === 'shiftInstance'"
          class="break-all"
        >
          {{ copy.table.origin }}: {{ formatWorkOrigin(state.editor.base.origin) }}
        </p>
        <p>{{ recordsCopy.baseline }} {{ formatNumber(state.editor.context.revision, locale) }}</p>
        <p v-if="dirty" role="status">{{ recordsCopy.unsaved }}</p>
        <div v-if="stale" role="status" class="state-panel">
          <p>{{ recordsCopy.stale }}</p>
          <button type="button" :disabled="busy || state.detailLoading || !facts" @click="rebase">
            {{ recordsCopy.reviewCurrent }}
          </button>
        </div>
        <template v-if="state.editor.kind === 'preset'">
          <p>{{ copy.presets.description }}</p>
          <p>{{ copy.presets.references }}</p>
          <label for="work-preset-record">{{ copy.presets.record }}</label>
          <select
            id="work-preset-record"
            :value="state.editor.selectedId"
            :disabled="busy"
            @change="vm.selectPreset(text($event))"
          >
            <option v-for="entry in state.editor.entries" :key="entry.id" :value="entry.id">
              {{ "name" in entry.original ? entry.original.name : copy.kinds[entry.original.kind] }}
              · {{ entry.id }}
            </option>
          </select>
          <button type="button" :disabled="busy" @click="vm.restorePresetReferences">
            {{ copy.presets.restoreReferences }}
          </button>
        </template>
        <form ref="form" class="field-stack" @submit.prevent="preview">
          <fieldset v-if="state.editor.kind === 'settings'" class="field-stack" :disabled="busy">
            <legend>{{ copy.settings }}</legend>
            <p>{{ copy.settingsHelp }}</p>
            <label for="work-settings-zone">{{ copy.timeZone }}</label>
            <input
              id="work-settings-zone"
              :value="state.editor.raw.timeZone"
              required
              maxlength="255"
              spellcheck="false"
              autocomplete="off"
              :aria-invalid="Boolean(settingsError('timeZone')) || undefined"
              aria-describedby="work-settings-zone-error"
              @input="setSetting('timeZone', $event)"
            />
            <p id="work-settings-zone-error" class="field-help">{{ settingsError("timeZone") }}</p>
            <label for="work-settings-start">{{ copy.fields.startDate }}</label>
            <input
              id="work-settings-start"
              type="text"
              maxlength="64"
              spellcheck="false"
              required
              :value="state.editor.raw.startDate"
              :aria-invalid="Boolean(settingsError('startDate')) || undefined"
              aria-describedby="work-settings-start-error"
              @input="setSetting('startDate', $event)"
            />
            <p id="work-settings-start-error" class="field-help">
              {{ settingsError("startDate") }}
            </p>
            <label for="work-settings-end">{{ copy.fields.endDateExclusive }}</label>
            <input
              id="work-settings-end"
              type="text"
              maxlength="64"
              spellcheck="false"
              required
              :value="state.editor.raw.endDateExclusive"
              :aria-invalid="Boolean(settingsError('endDateExclusive')) || undefined"
              aria-describedby="work-settings-end-error"
              @input="setSetting('endDateExclusive', $event)"
            />
            <p id="work-settings-end-error" class="field-help">
              {{ settingsError("endDateExclusive") }}
            </p>
            <label for="work-settings-gap">{{ copy.gapPolicy }}</label>
            <select
              id="work-settings-gap"
              :value="state.editor.raw.gapPolicy"
              :aria-invalid="Boolean(settingsError('gapPolicy')) || undefined"
              aria-describedby="work-settings-gap-error"
              @change="setGap"
            >
              <option v-for="(label, value) in copy.gapOptions" :key="value" :value="value">
                {{ label }}
              </option>
            </select>
            <p id="work-settings-gap-error" class="field-help">{{ settingsError("gapPolicy") }}</p>
            <label for="work-settings-overlap">{{ copy.overlapPolicy }}</label>
            <select
              id="work-settings-overlap"
              :value="state.editor.raw.overlapPolicy"
              :aria-invalid="Boolean(settingsError('overlapPolicy')) || undefined"
              aria-describedby="work-settings-overlap-error"
              @change="setOverlap"
            >
              <option v-for="(label, value) in copy.overlapOptions" :key="value" :value="value">
                {{ label }}
              </option>
            </select>
            <p id="work-settings-overlap-error" class="field-help">
              {{ settingsError("overlapPolicy") }}
            </p>
          </fieldset>
          <WorkRecordFields
            v-else-if="activeRaw && facts"
            v-bind="editableFieldContext"
            :key="state.editor.kind === 'record' ? state.editor.id : state.editor.selectedId"
            :model-value="activeRaw"
            :time-zone="
              state.editor.kind === 'record' ? state.editor.time.timeZone : facts.settings.timeZone
            "
            :disabled="busy"
            :read-only="state.editor.kind === 'record' && !state.editor.editing"
            :show-errors="showErrors"
            :errors="state.errors"
            @update:model-value="update"
          />
          <template
            v-if="
              state.editor.kind === 'record' && state.editor.raw.kind === 'shiftInstance' && facts
            "
          >
            <div class="state-panel">
              <p>
                {{ copy.previousTimeContext }}: {{ state.editor.time.timeZone }} ·
                {{ copy.gapOptions[state.editor.time.gapPolicy] }} ·
                {{ copy.overlapOptions[state.editor.time.overlapPolicy] }}
              </p>
              <p>
                {{ copy.currentTimeContext }}: {{ facts.settings.timeZone }} ·
                {{ copy.gapOptions[facts.settings.gapPolicy] }} ·
                {{ copy.overlapOptions[facts.settings.overlapPolicy] }}
              </p>
            </div>
            <section
              v-for="side in endpoints"
              :key="side"
              class="field-stack"
              :aria-label="side === 'startsAt' ? copy.table.start : copy.table.end"
            >
              <h4>{{ side === "startsAt" ? copy.table.start : copy.table.end }}</h4>
              <div v-if="state.editor.base?.kind === 'shiftInstance'">
                <p>{{ copy.previousTimeContext }}: {{ state.editor.time.timeZone }}</p>
                <p>
                  {{ copy.table.localIntent }} <code>{{ state.editor.base[side].local }}</code>
                </p>
                <p>
                  {{ copy.table.offset }}
                  {{ formatWorkOffset(state.editor.base[side].offsetSeconds) }}
                </p>
                <p>
                  {{ copy.table.instant }} <code>{{ state.editor.base[side].instant }}</code>
                </p>
              </div>
              <div v-if="vm.needsTimeChoice(side)" role="status" class="state-panel">
                <p>{{ copy.timeContextChanged }}</p>
                <div class="action-row">
                  <button
                    type="button"
                    :disabled="busy"
                    @click="vm.chooseEndpoint(side, 'reinterpret')"
                  >
                    {{ copy.reinterpretEndpoint }}
                  </button>
                  <button
                    v-if="currentRecord?.kind === 'shiftInstance'"
                    type="button"
                    :disabled="busy"
                    @click="vm.chooseEndpoint(side, 'current')"
                  >
                    {{ copy.useCurrentEndpoint }}
                  </button>
                  <button
                    v-else-if="state.editor.base === null"
                    type="button"
                    :disabled="busy"
                    @click="vm.chooseEndpoint(side, 'clear')"
                  >
                    {{ copy.clearEndpoint }}
                  </button>
                </div>
              </div>
              <button
                v-if="state.editor.editing && vm.changedEndpoint(state.editor, side)"
                type="button"
                :disabled="busy || vm.needsTimeChoice(side)"
                @click="vm.prepareEndpoint(side)"
              >
                {{ side === "startsAt" ? copy.prepareStart : copy.prepareEnd }}
              </button>
              <div v-if="state.editor.prepared[side]">
                <h5>
                  {{ copy.preparedEndpoint }} · {{ state.editor.prepared[side].time.timeZone }}
                </h5>
                <p>
                  {{ copy.table.localIntent }}
                  <code>{{ state.editor.prepared[side].resolved.local }}</code>
                </p>
                <p>
                  {{ copy.table.offset }}
                  {{ formatWorkOffset(state.editor.prepared[side].resolved.offsetSeconds) }}
                </p>
                <p>
                  {{ copy.table.instant }}
                  <code>{{ state.editor.prepared[side].resolved.instant }}</code>
                </p>
              </div>
            </section>
          </template>
          <p v-if="state.editorError" role="alert">{{ state.editorError }}</p>
          <div class="action-row">
            <template v-if="state.editor.kind === 'record' && !state.editor.editing">
              <button
                type="button"
                :disabled="busy || stale || state.detailLoading"
                @click="vm.editRecord"
              >
                {{ recordsCopy.edit }}
              </button>
              <button
                type="button"
                :disabled="busy || stale || state.detailLoading"
                @click="reviewAction(vm.previewDelete)"
              >
                {{ recordsCopy.reviewDelete }}
              </button>
            </template>
            <button
              v-else
              type="submit"
              :disabled="
                busy ||
                stale ||
                !facts ||
                (state.editor.kind !== 'preset' && state.editor.rebase !== null)
              "
            >
              {{ state.editor.kind === "settings" ? copy.reviewSettings : recordsCopy.preview }}
            </button>
            <button type="button" :disabled="busy" @click="closeEditor">
              {{
                state.editor.kind === "record" && !state.editor.editing
                  ? recordsCopy.close
                  : recordsCopy.discard
              }}
            </button>
            <button
              v-if="state.detailError && state.editor.kind === 'record'"
              type="button"
              :disabled="busy || !facts"
              @click="vm.copyAsNew"
            >
              {{ recordsCopy.copyAsNew }}
            </button>
          </div>
          <template
            v-if="
              state.editor.kind === 'record' &&
              !state.editor.editing &&
              state.editor.base?.kind === 'shiftInstance' &&
              state.editor.base.origin.kind === 'detached'
            "
          >
            <p>{{ copy.reattachHelp }}</p>
            <button type="button" :disabled="busy || stale" @click="reviewAction(vm.reattach)">
              {{ copy.reattach }}
            </button>
          </template>
        </form>
        <section
          v-if="state.editor.kind !== 'preset' && state.editor.rebase"
          class="field-stack"
          :aria-label="recordsCopy.conflicts"
        >
          <h4>{{ recordsCopy.conflicts }}</h4>
          <p>{{ recordsCopy.conflictHelp }}</p>
          <ul class="field-stack">
            <li v-for="field in state.editor.rebase.conflicts" :key="field">
              <h5>{{ fieldLabels[field] ?? field }}</h5>
              <p>
                {{ recordsCopy.currentValue }}
                {{ fieldSummary(state.editor.rebase.current, field) }}
              </p>
              <p>
                {{ recordsCopy.draftValue }} {{ fieldSummary(state.editor.rebase.value, field) }}
              </p>
              <div class="action-row">
                <button
                  type="button"
                  :disabled="busy || !conflictsCurrent"
                  @click="chooseConflict(field, 'current')"
                >
                  {{ recordsCopy.useCurrent }}
                </button>
                <button
                  type="button"
                  :disabled="busy || !conflictsCurrent"
                  @click="chooseConflict(field, 'draft')"
                >
                  {{ recordsCopy.keepDraft }}
                </button>
              </div>
            </li>
          </ul>
          <button v-if="currentRecord" type="button" @click="inspectCurrent">
            {{ recordsCopy.inspectCurrent }}
          </button>
          <WorkSettingsFacts v-if="state.editor.kind === 'settings' && facts" :facts="facts" />
        </section>
        <section
          v-if="showCurrent && currentRaw && facts"
          class="field-stack"
          :aria-label="recordsCopy.currentRecord"
        >
          <h4 ref="currentHeading" tabindex="-1">{{ recordsCopy.currentRecord }}</h4>
          <p v-if="currentRecord?.kind === 'shiftInstance'" class="break-all">
            {{ copy.table.origin }}: {{ formatWorkOrigin(currentRecord.origin) }}
          </p>
          <WorkResolvedEndpoints
            v-if="currentRecord?.kind === 'shiftInstance'"
            :interval="currentRecord"
            :time-zone="facts.settings.timeZone"
          />
          <WorkRecordFields
            v-bind="fieldContext"
            :model-value="currentRaw"
            :time-zone="facts.settings.timeZone"
            read-only
          />
        </section>
      </section>

      <section
        v-if="diagnostics.length"
        class="state-panel field-stack"
        :aria-label="copy.diagnostics"
      >
        <h3>{{ copy.diagnostics }}</h3>
        <ul class="field-stack">
          <li
            v-for="(issue, index) in diagnostics.slice(
              diagnosticsPage * 50,
              (diagnosticsPage + 1) * 50,
            )"
            :key="index"
          >
            <p>{{ issue.message }}</p>
            <code>{{ issue.field }}</code>
            <button
              v-if="vm.diagnosticTarget(issue.field)"
              type="button"
              :disabled="!canReplace || !facts"
              @click="repair(issue)"
            >
              {{ copy.repairField }}
            </button>
          </li>
        </ul>
        <div class="action-row">
          <button type="button" :disabled="diagnosticsPage === 0" @click="diagnosticsPage -= 1">
            {{ recordsCopy.previous }}</button
          ><button
            type="button"
            :disabled="(diagnosticsPage + 1) * 50 >= diagnostics.length"
            @click="diagnosticsPage += 1"
          >
            {{ recordsCopy.next }}
          </button>
        </div>
      </section>
      <fieldset class="state-panel field-stack" :disabled="busy">
        <legend>{{ copy.reviewTitle }}</legend>
        <label
          ><input
            type="checkbox"
            :checked="state.reviewChangesOnly"
            @change="vm.setReviewFilter(checked($event), state.reviewWithinWindow)"
          />
          {{ copy.changesOnly }}</label
        >
        <label
          ><input
            type="checkbox"
            :checked="state.reviewWithinWindow"
            @change="vm.setReviewFilter(state.reviewChangesOnly, checked($event))"
          />
          {{ copy.reviewWithinWindow }}</label
        >
        <p>{{ copy.generation.scope }}</p>
      </fieldset>
      <p v-if="review.state.error" class="state-panel" role="alert">{{ review.state.error }}</p>
      <section
        v-if="review.state.review && facts"
        class="state-panel field-stack"
        aria-labelledby="work-review-heading"
      >
        <h3 id="work-review-heading" ref="reviewHeading" tabindex="-1">{{ copy.reviewTitle }}</h3>
        <p>{{ copy.previewReady }}</p>
        <p>{{ copy.validationScope }}</p>
        <p>
          {{ recordsCopy.reviewRevision }}
          {{ formatNumber(review.state.review.snapshot.revision, locale) }}
        </p>
        <p class="break-all">
          {{ recordsCopy.commandIdentity }} {{ review.state.review.snapshot.commandId }}
        </p>
        <p>{{ recordsCopy.localActorHelp }}</p>
        <details>
          <summary>{{ copy.currentSettings }}</summary>
          <WorkSettingsFacts :facts="facts" />
        </details>
        <details>
          <summary>{{ copy.proposedSettings }}</summary>
          <WorkSettingsFacts :facts="review.state.review.sourceFacts" />
        </details>
        <section
          v-if="review.state.review.changes"
          class="field-stack"
          :aria-label="recordsCopy.changePages"
        >
          <h4>{{ recordsCopy.changePages }}</h4>
          <p class="field-help">{{ copy.commandInspectionHelp }}</p>
          <ul class="field-stack">
            <li v-for="item in review.state.review.changes.items" :key="item.ordinal">
              <p>
                {{ recordsCopy.changeKinds[item.change.kind] }} ·
                <code>{{ item.change.path }}</code>
              </p>
              <div class="action-row">
                <button
                  v-if="item.change.before !== null && state.editor?.kind !== 'preset'"
                  type="button"
                  :disabled="busy || review.state.redoRequired"
                  @click="inspectChange(item.ordinal, 'before')"
                >
                  {{ copy.inspectStoredSource }}
                </button>
                <button
                  v-if="item.change.after !== null"
                  type="button"
                  :disabled="busy || review.state.redoRequired"
                  @click="inspectChange(item.ordinal, 'after')"
                >
                  {{ copy.inspectProposedSource }}
                </button>
              </div>
            </li>
          </ul>
          <p>
            {{ recordsCopy.changeCount }}
            {{ formatNumber(review.state.review.changes.totalItems, locale) }}
          </p>
          <nav class="action-row" :aria-label="recordsCopy.changePages">
            <button
              type="button"
              :disabled="busy || review.state.redoRequired"
              @click="review.pageChanges(true)"
            >
              {{ recordsCopy.first }}
            </button>
            <button
              type="button"
              :disabled="
                busy ||
                review.state.redoRequired ||
                review.state.review.changes.continuation === null
              "
              @click="review.pageChanges()"
            >
              {{ recordsCopy.next }}
            </button>
          </nav>
        </section>
        <WorkGenerationTable
          v-bind="displayContext"
          :review="review.state.review.generation"
          :time-zone="review.state.review.sourceFacts.settings.timeZone"
          :before-time-zone="facts.settings.timeZone"
          :disabled="busy || review.state.redoRequired"
          @inspect="inspectGeneration"
        />
        <nav class="action-row" :aria-label="copy.generationPages">
          <button
            type="button"
            :disabled="busy || review.state.redoRequired"
            @click="review.pageGeneration(true)"
          >
            {{ recordsCopy.first }}
          </button>
          <button
            type="button"
            :disabled="
              busy ||
              review.state.redoRequired ||
              review.state.review.generation.page.continuation === null
            "
            @click="review.pageGeneration()"
          >
            {{ recordsCopy.next }}
          </button>
        </nav>
        <section
          v-if="review.state.review.inspection"
          class="field-stack"
          :aria-label="copy.inspection"
        >
          <h4 ref="reviewInspectionHeading" tabindex="-1">
            {{ copy.inspection }} · {{ copy.generation[review.state.review.inspection.side] }}
          </h4>
          <WorkSettingsFacts
            v-if="review.state.review.inspection.kind === 'settings'"
            :facts="review.state.review.inspection.facts"
          />
          <template v-else-if="inspectedRecord">
            <p class="break-all">{{ recordsCopy.identity }} {{ inspectedRecord.id }}</p>
            <p v-if="inspectedRecord.origin" class="break-all">
              {{ copy.table.origin }}: {{ formatWorkOrigin(inspectedRecord.origin) }}
            </p>
            <WorkResolvedEndpoints
              v-if="inspectedRecord.instance"
              :interval="inspectedRecord.instance"
              :time-zone="inspectedRecord.timeZone"
            />
            <WorkRecordFields
              v-bind="fieldContext"
              :model-value="inspectedRecord.raw"
              :time-zone="inspectedRecord.timeZone"
              read-only
            />
          </template>
        </section>
        <section v-if="warnings.length" :aria-label="recordsCopy.warnings">
          <h4>{{ recordsCopy.warnings }}</h4>
          <ul>
            <li
              v-for="(warning, index) in warnings.slice(warningsPage * 50, (warningsPage + 1) * 50)"
              :key="index"
            >
              {{ warning.message }}
            </li>
          </ul>
          <div class="action-row">
            <button type="button" :disabled="warningsPage === 0" @click="warningsPage -= 1">
              {{ recordsCopy.previous }}</button
            ><button
              type="button"
              :disabled="(warningsPage + 1) * 50 >= warnings.length"
              @click="warningsPage += 1"
            >
              {{ recordsCopy.next }}
            </button>
          </div>
        </section>
        <p
          v-if="
            review.state.review.snapshot.source.kind === 'stored' &&
            !review.state.review.generation.reconciliationRequired
          "
        >
          {{ copy.noStoredChanges }}
        </p>
        <div v-else-if="review.state.redoRequired" role="alert">
          <p>{{ recordsCopy.redoWarning }}</p>
          <button type="button" :disabled="busy" @click="save(true)">{{ copy.confirmRedo }}</button>
        </div>
        <button
          v-else
          type="button"
          :disabled="busy || home.state.mutation?.outcome === 'outcomeUnknown'"
          @click="save()"
        >
          {{ copy.save }}
        </button>
      </section>
    </template>
    <RouteLeaveGuard
      :home="home"
      :dirty="dirty"
      :pending="busy || state.workLoading"
      :discard="vm.discard"
    />
  </section>
</template>
