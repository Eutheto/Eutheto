<script setup lang="ts">
import { computed, nextTick, reactive, ref, useId, watch } from "vue";
import type { DomainEntityRef } from "../api/generated";
import type { WorkforceWeekday } from "../api/generated-domain-pack-contracts";
import { messages } from "../messages";
import type { ProjectSummary } from "../project-home";
import type { SupportingRecordDraft } from "../supporting-record-fields";
import {
  workRecordValue,
  workRowKey,
  type WorkRecordDraft,
  type WorkShiftFieldsDraft,
  type WorkTextRowDraft,
  type WorkTransitionDraft,
} from "../work-record-draft";
import SupportingRecordFields from "./SupportingRecordFields.vue";
import WorkCalendarFields from "./WorkCalendarFields.vue";
import WorkCoverageFields from "./WorkCoverageFields.vue";
import DateTimeRangeField from "./planner/DateTimeRangeField.vue";
import DurationField from "./planner/DurationField.vue";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";
import type { TemporalDraft, TemporalFeedback } from "./planner/field-contracts";

const props = defineProps<{
  readonly modelValue: WorkRecordDraft;
  readonly project: ProjectSummary;
  readonly libraryEpoch: number;
  readonly draftReferences?: readonly DomainEntityRef[];
  readonly requestKey: string;
  readonly timeZone: string;
  readonly locale?: string;
  readonly disabled?: boolean;
  readonly readOnly?: boolean;
  readonly showErrors?: boolean;
  readonly errors?: Readonly<Record<string, string>>;
  readonly temporalFeedback?: TemporalFeedback;
}>();
const emit = defineEmits<{ "update:modelValue": [value: WorkRecordDraft] }>();
const copy = messages.work.fields;
const prefix = useId();
const host = ref<HTMLElement>();
const pages = reactive({ transitions: 0, tags: 0, excludedDates: 0 });
const locked = computed(() => props.disabled || props.readOnly);
const errors = computed(() => {
  const result: Record<string, string> = {};
  if (props.showErrors) {
    const parsed = workRecordValue("", props.modelValue, {
      baseline: null,
      startsAt: null,
      endsAt: null,
    });
    for (const [path, message] of Object.entries(parsed.errors)) {
      // Preparation belongs to the parent; this component has no native baseline or resolution.
      if (message !== copy.endpointPreparation) result[path] = message;
    }
  }
  return { ...result, ...props.errors };
});
const fieldContext = computed(() => ({
  disabled: props.disabled,
  readOnly: props.readOnly,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
const pickerContext = computed(() => ({
  ...fieldContext.value,
  project: props.project,
  libraryEpoch: props.libraryEpoch,
  ...(props.draftReferences === undefined ? {} : { draftReferences: props.draftReferences }),
}));
const temporalContext = computed(() => ({
  ...fieldContext.value,
  timeZone: props.timeZone,
  requestKey: props.requestKey,
}));
const transitions = computed(() => {
  const all = props.modelValue.kind === "location" ? props.modelValue.transitions : [];
  const offset = Math.min(pages.transitions, Math.max(0, Math.ceil(all.length / 50) - 1)) * 50;
  return { total: all.length, offset, rows: all.slice(offset, offset + 50) };
});
type TextCollection = "tags" | "excludedDates";
const textCollections = computed(() => {
  const draft = props.modelValue;
  if (draft.kind !== "shiftInstance" && draft.kind !== "shiftTemplate") return [];
  const collections: {
    key: TextCollection;
    label: string;
    add: string;
    remove: string;
    path: string;
    all: readonly WorkTextRowDraft[];
  }[] = [
    {
      key: "tags",
      label: copy.tags,
      add: copy.addTag,
      remove: copy.removeTag,
      path: "tags",
      all: draft.tags,
    },
  ];
  if (draft.kind === "shiftTemplate")
    collections.unshift({
      key: "excludedDates",
      label: copy.excludedDates,
      add: copy.addExcludedDate,
      remove: copy.removeExcludedDate,
      path: "recurrence.excludedDates",
      all: draft.recurrence.excludedDates,
    });
  return collections.map(({ all, ...collection }) => {
    const offset =
      Math.min(pages[collection.key], Math.max(0, Math.ceil(all.length / 50) - 1)) * 50;
    return { ...collection, offset, total: all.length, rows: all.slice(offset, offset + 50) };
  });
});
watch(
  [errors, () => props.modelValue],
  ([reported, draft], [before, beforeDraft]) => {
    // Reveal new reports before the parent's nextTick focus; never navigate for an edit.
    if (beforeDraft && draft !== beforeDraft) return;
    const first = { transitions: Infinity, tags: Infinity, excludedDates: Infinity };
    for (const [path, message] of Object.entries(reported)) {
      if (!message || before?.[path]) continue;
      const match = /^(transitions|tags|recurrence\.excludedDates)\.(\d+)(?:\.|$)/u.exec(path);
      if (!match) continue;
      const key =
        match[1] === "transitions" ? "transitions" : match[1] === "tags" ? "tags" : "excludedDates";
      first[key] = Math.min(first[key], Number(match[2]));
    }
    if (draft.kind === "location" && first.transitions < draft.transitions.length)
      pages.transitions = Math.floor(first.transitions / 50);
    if (draft.kind === "shiftTemplate" || draft.kind === "shiftInstance") {
      if (first.tags < draft.tags.length) pages.tags = Math.floor(first.tags / 50);
      if (
        draft.kind === "shiftTemplate" &&
        first.excludedDates < draft.recurrence.excludedDates.length
      )
        pages.excludedDates = Math.floor(first.excludedDates / 50);
    }
  },
  { immediate: true },
);
async function changePage(collection: "transitions" | TextCollection, page: number): Promise<void> {
  if (props.disabled) return;
  pages[collection] = page;
  await nextTick();
  const first =
    collection === "transitions"
      ? transitions.value.rows[0]
      : textCollections.value.find((item) => item.key === collection)?.rows[0];
  if (first) host.value?.querySelector<HTMLElement>(`[data-row-key="${first.key}"] input`)?.focus();
}
const feedback = computed<TemporalFeedback>(() => {
  const entries: TemporalFeedback["errors"][number][] =
    props.temporalFeedback?.requestKey === props.requestKey
      ? [...props.temporalFeedback.errors]
      : [];
  const paths: readonly [string, TemporalFeedback["errors"][number]["field"]][] =
    props.modelValue.kind === "shiftInstance"
      ? [
          ["startsAt", "start"],
          ["endsAt", "end"],
          ["interval", "range"],
        ]
      : [
          ["timing.startTime", "start"],
          ["timing.endTime", "end"],
          ["timing.endDayOffset", "endDayOffset"],
          ["timing", "range"],
        ];
  for (const [path, field] of paths) {
    const message = errors.value[path];
    if (message) entries.push({ field, message });
  }
  return { requestKey: props.requestKey, errors: entries };
});
type TemplateDraft = Extract<WorkRecordDraft, { kind: "shiftTemplate" }>;
function text(event: Event): string {
  return (event.target as HTMLInputElement | HTMLSelectElement).value;
}
function field(path: string) {
  return {
    id: `${prefix}-${path}`,
    name: path,
    disabled: props.disabled,
    readonly: props.readOnly,
    "aria-invalid": Boolean(errors.value[path]) || undefined,
    "aria-describedby": `${prefix}-${path}-error`,
  };
}
function setName(event: Event): void {
  if (!locked.value && props.modelValue.kind !== "shiftInstance")
    emit("update:modelValue", { ...props.modelValue, name: text(event) });
}
function setAssignment(value: SupportingRecordDraft): void {
  if (
    !locked.value &&
    props.modelValue.kind === "assignmentType" &&
    value.kind === "assignmentType"
  )
    emit("update:modelValue", value);
}
function setShift<K extends keyof WorkShiftFieldsDraft>(
  key: K,
  value: WorkShiftFieldsDraft[K],
): void {
  if (
    !locked.value &&
    (props.modelValue.kind === "shiftTemplate" || props.modelValue.kind === "shiftInstance")
  )
    emit("update:modelValue", { ...props.modelValue, [key]: value });
}
function setTemplate<K extends keyof Omit<TemplateDraft, "kind">>(
  key: K,
  value: TemplateDraft[K],
): void {
  if (!locked.value && props.modelValue.kind === "shiftTemplate")
    emit("update:modelValue", { ...props.modelValue, [key]: value });
}
function setRecurrence<K extends keyof TemplateDraft["recurrence"]>(
  key: K,
  value: TemplateDraft["recurrence"][K],
): void {
  if (props.modelValue.kind === "shiftTemplate")
    setTemplate("recurrence", { ...props.modelValue.recurrence, [key]: value });
}
function weekday(value: WorkforceWeekday, checked: boolean): void {
  if (props.modelValue.kind !== "shiftTemplate") return;
  const all = props.modelValue.recurrence.weekdays;
  setRecurrence(
    "weekdays",
    checked ? (all.includes(value) ? all : [...all, value]) : all.filter((day) => day !== value),
  );
}
function timingMode(event: Event): void {
  const value = text(event);
  if (value === "" || value === "localWindow" || value === "elapsedDuration")
    setTemplate("timingMode", value);
}
function attribution(event: Event): void {
  const value = text(event);
  if (value === "" || value === "startLocalDate" || value === "endLocalDate")
    setShift("reportingAttribution", value);
}
function measurement(event: Event): void {
  const value = text(event);
  if (
    !locked.value &&
    props.modelValue.kind === "workloadBucket" &&
    (value === "" ||
      value === "assignmentCount" ||
      value === "elapsedMinutes" ||
      value === "scheduledMinutes")
  )
    emit("update:modelValue", { ...props.modelValue, measurement: value });
}
function contribution(event: Event): void {
  const value = text(event);
  if (
    !locked.value &&
    props.modelValue.kind === "workloadBucket" &&
    (value === "" || value === "sum" || value === "union")
  )
    emit("update:modelValue", { ...props.modelValue, overlappingContribution: value });
}
function setInterval(interval: TemporalDraft): void {
  if (
    !locked.value &&
    props.modelValue.kind === "shiftInstance" &&
    interval.raw.kind === "localInterval"
  )
    emit("update:modelValue", { ...props.modelValue, interval });
}
function updateTransition<K extends keyof Omit<WorkTransitionDraft, "key">>(
  key: string,
  field: K,
  value: WorkTransitionDraft[K],
): void {
  if (!locked.value && props.modelValue.kind === "location")
    emit("update:modelValue", {
      ...props.modelValue,
      transitions: props.modelValue.transitions.map((row) =>
        row.key === key ? { ...row, [field]: value } : row,
      ),
    });
}
async function addTransition(): Promise<void> {
  if (locked.value || props.modelValue.kind !== "location") return;
  const key = workRowKey();
  const count = props.modelValue.transitions.length;
  emit("update:modelValue", {
    ...props.modelValue,
    transitions: [
      ...props.modelValue.transitions,
      { key, locationId: "", minutes: { raw: "", unit: "minutes", status: "empty" } },
    ],
  });
  pages.transitions = Math.floor(count / 50);
  await nextTick();
  host.value?.querySelector<HTMLElement>(`[data-row-key="${key}"] input`)?.focus();
}
async function removeTransition(key: string): Promise<void> {
  if (locked.value || props.modelValue.kind !== "location") return;
  const index = props.modelValue.transitions.findIndex((row) => row.key === key);
  emit("update:modelValue", {
    ...props.modelValue,
    transitions: props.modelValue.transitions.filter((row) => row.key !== key),
  });
  await nextTick();
  const nearby =
    transitions.value.rows[
      Math.max(0, Math.min(index - transitions.value.offset, transitions.value.rows.length - 1))
    ];
  if (nearby)
    host.value?.querySelector<HTMLElement>(`[data-row-key="${nearby.key}"] input`)?.focus();
  else host.value?.querySelector<HTMLElement>("[data-add-transition]")?.focus();
}
function replaceTextRows(collection: TextCollection, rows: readonly WorkTextRowDraft[]): void {
  if (collection === "tags") setShift("tags", rows);
  else setRecurrence("excludedDates", rows);
}
function updateTextRow(collection: TextCollection, key: string, value: string): void {
  const draft = props.modelValue;
  if (draft.kind !== "shiftTemplate" && draft.kind !== "shiftInstance") return;
  const all =
    collection === "tags"
      ? draft.tags
      : draft.kind === "shiftTemplate"
        ? draft.recurrence.excludedDates
        : [];
  replaceTextRows(
    collection,
    all.map((row) => (row.key === key ? { ...row, value } : row)),
  );
}
async function addTextRow(collection: TextCollection): Promise<void> {
  const draft = props.modelValue;
  if (locked.value || (draft.kind !== "shiftTemplate" && draft.kind !== "shiftInstance")) return;
  const all =
    collection === "tags"
      ? draft.tags
      : draft.kind === "shiftTemplate"
        ? draft.recurrence.excludedDates
        : [];
  const key = workRowKey();
  replaceTextRows(collection, [...all, { key, value: "" }]);
  pages[collection] = Math.floor(all.length / 50);
  await nextTick();
  host.value?.querySelector<HTMLElement>(`[data-row-key="${key}"] input`)?.focus();
}
async function removeTextRow(collection: TextCollection, key: string): Promise<void> {
  const draft = props.modelValue;
  if (locked.value || (draft.kind !== "shiftTemplate" && draft.kind !== "shiftInstance")) return;
  const all =
    collection === "tags"
      ? draft.tags
      : draft.kind === "shiftTemplate"
        ? draft.recurrence.excludedDates
        : [];
  const index = all.findIndex((row) => row.key === key);
  replaceTextRows(
    collection,
    all.filter((row) => row.key !== key),
  );
  await nextTick();
  const current = textCollections.value.find((item) => item.key === collection);
  const nearby =
    current?.rows[Math.max(0, Math.min(index - current.offset, current.rows.length - 1))];
  if (nearby)
    host.value?.querySelector<HTMLElement>(`[data-row-key="${nearby.key}"] input`)?.focus();
  else host.value?.querySelector<HTMLElement>(`[data-add-collection="${collection}"]`)?.focus();
}
</script>

<template>
  <div ref="host" class="field-stack">
    <SupportingRecordFields
      v-if="modelValue.kind === 'assignmentType'"
      v-bind="pickerContext"
      :model-value="modelValue"
      :show-errors="showErrors"
      :errors="errors"
      @update:model-value="setAssignment"
    />
    <template v-else>
      <p :id="`${prefix}-native-help`" class="field-help">{{ copy.nativeValidation }}</p>
      <div v-if="modelValue.kind !== 'shiftInstance'" class="field-stack">
        <label :for="`${prefix}-name`">{{ copy.name }}</label>
        <input
          v-bind="field('name')"
          type="text"
          required
          :value="modelValue.name"
          @input="setName"
        />
        <p :id="`${prefix}-name-error`" class="text-danger" role="status">{{ errors.name }}</p>
      </div>
      <fieldset v-if="modelValue.kind === 'location'" class="field-stack" :disabled="disabled">
        <legend>{{ copy.transitions }}</legend>
        <p class="text-danger" role="status">{{ errors.transitions }}</p>
        <fieldset
          v-for="(row, index) in transitions.rows"
          :key="row.key"
          :data-row-key="row.key"
          class="field-stack"
        >
          <legend>{{ copy.destination }} {{ transitions.offset + index + 1 }}</legend>
          <WorkforceEntityPicker
            v-bind="pickerContext"
            :id="`${prefix}-${row.key}-location`"
            kind="location"
            :label="copy.destination"
            :model-value="row.locationId === '' ? [] : [row.locationId]"
            :error="errors[`transitions.${transitions.offset + index}.locationId`]"
            @update:model-value="updateTransition(row.key, 'locationId', $event[0] ?? '')"
          />
          <DurationField
            v-bind="fieldContext"
            :id="`${prefix}-${row.key}-minutes`"
            :label="copy.transitionMinutes"
            :model-value="row.minutes"
            :minimum="0"
            :error="errors[`transitions.${transitions.offset + index}.minutes`]"
            @update:model-value="updateTransition(row.key, 'minutes', $event)"
          />
          <button
            type="button"
            class="button-secondary"
            :disabled="locked"
            @click="removeTransition(row.key)"
          >
            {{ copy.removeTransition }}
          </button>
        </fieldset>
        <nav
          :aria-label="`${copy.transitions}: ${copy.collectionPages}`"
          class="flex flex-wrap items-center gap-2"
        >
          <span role="status"
            >{{ copy.rows }} {{ transitions.total }} · {{ copy.page }}
            {{ transitions.offset / 50 + 1 }}</span
          >
          <button
            type="button"
            class="button-secondary"
            :disabled="disabled || transitions.offset === 0"
            @click="changePage('transitions', 0)"
          >
            {{ copy.first }}
          </button>
          <button
            type="button"
            class="button-secondary"
            :disabled="disabled || transitions.offset === 0"
            @click="changePage('transitions', transitions.offset / 50 - 1)"
          >
            {{ copy.previous }}
          </button>
          <button
            type="button"
            class="button-secondary"
            :disabled="disabled || transitions.offset + 50 >= transitions.total"
            @click="changePage('transitions', transitions.offset / 50 + 1)"
          >
            {{ copy.next }}
          </button>
        </nav>
        <button
          type="button"
          class="button-secondary"
          data-add-transition
          :disabled="locked"
          @click="addTransition"
        >
          {{ copy.addTransition }}
        </button>
      </fieldset>
      <template v-if="modelValue.kind === 'workloadBucket'">
        <div class="field-stack">
          <label :for="`${prefix}-measurement`">{{ copy.measurement }}</label>
          <select
            v-bind="field('measurement')"
            :disabled="locked"
            :value="modelValue.measurement"
            @change="measurement"
          >
            <option value="">{{ copy.selection }}</option>
            <option v-for="(label, value) in copy.measurements" :key="value" :value="value">
              {{ label }}
            </option>
          </select>
          <p :id="`${prefix}-measurement-error`" class="text-danger" role="status">
            {{ errors.measurement }}
          </p>
        </div>
        <div class="field-stack">
          <label :for="`${prefix}-overlappingContribution`">{{
            copy.overlappingContribution
          }}</label>
          <select
            v-bind="field('overlappingContribution')"
            :disabled="locked"
            :value="modelValue.overlappingContribution"
            @change="contribution"
          >
            <option value="">{{ copy.selection }}</option>
            <option v-for="(label, value) in copy.contributions" :key="value" :value="value">
              {{ label }}
            </option>
          </select>
          <p :id="`${prefix}-overlappingContribution-error`" class="text-danger" role="status">
            {{ errors.overlappingContribution }}
          </p>
        </div>
      </template>
      <WorkCalendarFields
        v-if="modelValue.kind === 'calendar'"
        v-bind="temporalContext"
        :model-value="modelValue.period"
        :errors="errors"
        @update:model-value="
          !locked &&
          modelValue.kind === 'calendar' &&
          emit('update:modelValue', { ...modelValue, period: $event })
        "
      />
      <template v-if="modelValue.kind === 'shiftTemplate' || modelValue.kind === 'shiftInstance'">
        <WorkforceEntityPicker
          v-bind="pickerContext"
          :id="`${prefix}-assignmentTypeId`"
          kind="assignmentType"
          :label="copy.assignmentTypeId"
          :model-value="modelValue.assignmentTypeId === '' ? [] : [modelValue.assignmentTypeId]"
          :error="errors.assignmentTypeId"
          @update:model-value="setShift('assignmentTypeId', $event[0] ?? '')"
        />
        <label :for="`${prefix}-location-enabled`">
          <input
            :id="`${prefix}-location-enabled`"
            type="checkbox"
            :disabled="locked"
            :checked="modelValue.locationEnabled"
            @change="setShift('locationEnabled', ($event.target as HTMLInputElement).checked)"
          />
          {{ copy.locationEnabled }}
        </label>
        <WorkforceEntityPicker
          v-if="modelValue.locationEnabled"
          v-bind="pickerContext"
          :id="`${prefix}-locationId`"
          kind="location"
          :label="copy.locationId"
          :model-value="modelValue.locationId === '' ? [] : [modelValue.locationId]"
          :error="errors.locationId"
          @update:model-value="setShift('locationId', $event[0] ?? '')"
        />
        <template v-if="modelValue.kind === 'shiftTemplate'">
          <fieldset class="field-stack" :disabled="disabled">
            <legend>{{ copy.effectiveRange }}</legend>
            <div class="field-stack">
              <label :for="`${prefix}-recurrence.effectiveRange.startDate`">{{
                copy.startDate
              }}</label>
              <input
                v-bind="field('recurrence.effectiveRange.startDate')"
                type="text"
                :spellcheck="false"
                :value="modelValue.recurrence.startDate"
                @input="setRecurrence('startDate', text($event))"
              />
              <p
                :id="`${prefix}-recurrence.effectiveRange.startDate-error`"
                class="text-danger"
                role="status"
              >
                {{ errors["recurrence.effectiveRange.startDate"] }}
              </p>
            </div>
            <div class="field-stack">
              <label :for="`${prefix}-recurrence.effectiveRange.endDateExclusive`">{{
                copy.endDateExclusive
              }}</label>
              <input
                v-bind="field('recurrence.effectiveRange.endDateExclusive')"
                type="text"
                :spellcheck="false"
                :value="modelValue.recurrence.endDateExclusive"
                @input="setRecurrence('endDateExclusive', text($event))"
              />
              <p
                :id="`${prefix}-recurrence.effectiveRange.endDateExclusive-error`"
                class="text-danger"
                role="status"
              >
                {{ errors["recurrence.effectiveRange.endDateExclusive"] }}
              </p>
            </div>
          </fieldset>
          <fieldset
            class="field-stack"
            :disabled="disabled"
            :aria-describedby="`${prefix}-weekdays-error`"
          >
            <legend>{{ copy.weekdays }}</legend>
            <label
              v-for="(label, value) in copy.weekdayNames"
              :key="value"
              :for="`${prefix}-${value}`"
            >
              <input
                :id="`${prefix}-${value}`"
                type="checkbox"
                :disabled="locked"
                :checked="modelValue.recurrence.weekdays.includes(value)"
                @change="weekday(value, ($event.target as HTMLInputElement).checked)"
              />
              {{ label }}
            </label>
            <p :id="`${prefix}-weekdays-error`" class="text-danger" role="status">
              {{ errors["recurrence.weekdays"] }}
            </p>
          </fieldset>
          <div class="field-stack">
            <label :for="`${prefix}-timing.kind`">{{ copy.timing }}</label>
            <select
              v-bind="field('timing.kind')"
              :disabled="locked"
              :value="modelValue.timingMode"
              @change="timingMode"
            >
              <option value="">{{ copy.selection }}</option>
              <option v-for="(label, value) in copy.timings" :key="value" :value="value">
                {{ label }}
              </option>
            </select>
            <p :id="`${prefix}-timing.kind-error`" class="text-danger" role="status">
              {{ errors["timing.kind"] }}
            </p>
          </div>
          <DateTimeRangeField
            v-if="modelValue.timingMode === 'localWindow'"
            v-bind="temporalContext"
            :id="`${prefix}-timing`"
            :label="copy.localWindow"
            :model-value="modelValue.localWindow"
            :feedback="feedback"
            @update:model-value="
              $event.raw.kind === 'localWindow' && setTemplate('localWindow', $event)
            "
          />
          <template v-if="modelValue.timingMode === 'elapsedDuration'">
            <div class="field-stack">
              <label :for="`${prefix}-timing.startTime`">{{ copy.elapsedStartTime }}</label>
              <input
                v-bind="field('timing.startTime')"
                type="text"
                :spellcheck="false"
                :value="modelValue.elapsedStartTime"
                @input="setTemplate('elapsedStartTime', text($event))"
              />
              <p :id="`${prefix}-timing.startTime-error`" class="text-danger" role="status">
                {{ errors["timing.startTime"] }}
              </p>
            </div>
            <DurationField
              v-bind="fieldContext"
              :id="`${prefix}-duration`"
              :label="copy.duration"
              :model-value="modelValue.duration"
              :minimum="1"
              :error="errors['timing.durationMinutes']"
              @update:model-value="setTemplate('duration', $event)"
            />
          </template>
        </template>
        <DateTimeRangeField
          v-if="modelValue.kind === 'shiftInstance'"
          v-bind="temporalContext"
          :id="`${prefix}-interval`"
          :label="copy.interval"
          :model-value="modelValue.interval"
          :feedback="feedback"
          @update:model-value="setInterval"
        />
        <div class="field-stack">
          <label :for="`${prefix}-reportingAttribution`">{{ copy.reportingAttribution }}</label>
          <select
            v-bind="field('reportingAttribution')"
            :disabled="locked"
            :value="modelValue.reportingAttribution"
            @change="attribution"
          >
            <option value="">{{ copy.selection }}</option>
            <option v-for="(label, value) in copy.attributions" :key="value" :value="value">
              {{ label }}
            </option>
          </select>
          <p :id="`${prefix}-reportingAttribution-error`" class="text-danger" role="status">
            {{ errors.reportingAttribution }}
          </p>
        </div>
        <fieldset
          v-for="collection in textCollections"
          :key="collection.key"
          class="field-stack"
          :disabled="disabled"
        >
          <legend>{{ collection.label }}</legend>
          <p class="text-danger" role="status">{{ errors[collection.path] }}</p>
          <div
            v-for="(row, index) in collection.rows"
            :key="row.key"
            :data-row-key="row.key"
            class="field-stack"
          >
            <label :for="`${prefix}-${collection.path}.${collection.offset + index}`"
              >{{ collection.label }} {{ collection.offset + index + 1 }}</label
            >
            <input
              v-bind="field(`${collection.path}.${collection.offset + index}`)"
              type="text"
              :value="row.value"
              @input="updateTextRow(collection.key, row.key, text($event))"
            />
            <p
              :id="`${prefix}-${collection.path}.${collection.offset + index}-error`"
              class="text-danger"
              role="status"
            >
              {{ errors[`${collection.path}.${collection.offset + index}`] }}
            </p>
            <button
              type="button"
              class="button-secondary"
              :disabled="locked"
              @click="removeTextRow(collection.key, row.key)"
            >
              {{ collection.remove }}
            </button>
          </div>
          <nav
            :aria-label="`${collection.label}: ${copy.collectionPages}`"
            class="flex flex-wrap items-center gap-2"
          >
            <span role="status"
              >{{ copy.rows }} {{ collection.total }} · {{ copy.page }}
              {{ collection.offset / 50 + 1 }}</span
            >
            <button
              type="button"
              class="button-secondary"
              :disabled="disabled || collection.offset === 0"
              @click="changePage(collection.key, 0)"
            >
              {{ copy.first }}
            </button>
            <button
              type="button"
              class="button-secondary"
              :disabled="disabled || collection.offset === 0"
              @click="changePage(collection.key, collection.offset / 50 - 1)"
            >
              {{ copy.previous }}
            </button>
            <button
              type="button"
              class="button-secondary"
              :disabled="disabled || collection.offset + 50 >= collection.total"
              @click="changePage(collection.key, collection.offset / 50 + 1)"
            >
              {{ copy.next }}
            </button>
          </nav>
          <button
            type="button"
            class="button-secondary"
            :data-add-collection="collection.key"
            :disabled="locked"
            @click="addTextRow(collection.key)"
          >
            {{ collection.add }}
          </button>
        </fieldset>
        <WorkCoverageFields
          v-bind="fieldContext"
          :project="project"
          :library-epoch="libraryEpoch"
          :model-value="modelValue.coverage"
          :errors="errors"
          @update:model-value="setShift('coverage', $event)"
        />
      </template>
    </template>
  </div>
</template>
