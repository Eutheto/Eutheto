<script setup lang="ts">
import { nextTick, ref } from "vue";
import {
  availabilityWeeklyDraft,
  availabilityWeekdays,
  type AvailabilityDraft,
  type AvailabilityWeeklyDraft,
} from "../../availability-draft";
import type { ProjectSummary } from "../../project-home";
import DateTimeRangeField from "../planner/DateTimeRangeField.vue";
import WorkforceEntityPicker from "../planner/WorkforceEntityPicker.vue";
import { AVAILABILITY_KIND_LABELS, availabilityMessages as copy } from "./messages";

const props = defineProps<{
  draft: AvailabilityDraft;
  project: ProjectSummary;
  libraryEpoch: number;
  timeZone: string;
  requestKey: string;
  errors: Readonly<Record<string, string>>;
  disabled?: boolean;
  readOnly?: boolean;
  locale?: string;
}>();
const emit = defineEmits<{ change: [draft: AvailabilityDraft] }>();
const host = ref<HTMLElement>();
function change(patch: Partial<AvailabilityDraft>): void {
  if (!props.disabled && !props.readOnly) emit("change", { ...props.draft, ...patch });
}
function text(event: Event): string {
  return (event.target as HTMLInputElement).value;
}
function checked(event: Event): boolean {
  return (event.target as HTMLInputElement).checked;
}
function kind(event: Event): void {
  const value = text(event);
  if (
    value === "unavailable" ||
    value === "availableOnly" ||
    value === "approvedTimeOff" ||
    value === "requestedTimeOff"
  )
    change({ availabilityKind: value });
}
function weekly(key: string, patch: Partial<AvailabilityWeeklyDraft>): void {
  change({
    weekly: props.draft.weekly.map((row) => (row.key === key ? { ...row, ...patch } : row)),
  });
}
function weekday(
  row: AvailabilityWeeklyDraft,
  day: AvailabilityWeeklyDraft["weekdays"][number],
  enabled: boolean,
): void {
  weekly(row.key, {
    weekdays: enabled ? [...row.weekdays, day] : row.weekdays.filter((value) => value !== day),
  });
}
const addWeeklyButton = ref<HTMLButtonElement>();
async function addWeekly(): Promise<void> {
  if (props.disabled || props.readOnly) return;
  const origin = document.activeElement;
  const row = availabilityWeeklyDraft();
  change({ weekly: [...props.draft.weekly, row] });
  await nextTick();
  if (
    document.activeElement === origin &&
    props.draft.weekly.some((entry) => entry.key === row.key)
  )
    document.getElementById(`${row.key}-heading`)?.focus();
}
function removeWeekly(key: string): void {
  if (props.disabled || props.readOnly) return;
  addWeeklyButton.value?.focus();
  change({ weekly: props.draft.weekly.filter((entry) => entry.key !== key) });
}
async function focusField(path: readonly string[], isCurrent: () => boolean): Promise<boolean> {
  if (!isCurrent()) return false;
  const raw = props.draft;
  const fields: Readonly<Record<string, string>> = {
    availabilityKind: "availability-kind",
    effectiveRange: "availability-effective-range",
    "effectiveRange.startDate": "availability-start-date",
    "effectiveRange.endDateExclusive": "availability-end-date",
    timeWindow: "availability-window",
    "timeWindow.kind": "availability-window-kind",
    "timeWindow.windows": "availability-window",
    assignmentTypeIds: "availability-types",
    locationIds: "availability-locations",
    source: "availability-source",
    note: "availability-note",
  };
  const key = path.join(".");
  let id = Object.hasOwn(fields, key) ? fields[key] : undefined;
  if (
    raw.windowKind === "instant" &&
    path.length === 2 &&
    path[0] === "timeWindow" &&
    (path[1] === "startsAt" || path[1] === "endsAt")
  ) {
    const endpoint = path[1] === "startsAt" ? "start" : "end";
    id =
      raw.storedInstant !== null && !raw.replaceInstant
        ? `availability-stored-${endpoint}`
        : `availability-local-interval-${endpoint}`;
  }
  if (
    raw.windowKind === "weekly" &&
    path[0] === "timeWindow" &&
    path[1] === "windows" &&
    (path.length === 3 || path.length === 4)
  ) {
    const index = Number(path[2]);
    const row = Number.isSafeInteger(index) && index >= 0 ? raw.weekly[index] : undefined;
    if (row === undefined) return false;
    if (path.length === 3) id = `${row.key}-heading`;
    else if (path[3] === "startTime") id = `${row.key}-start`;
    else if (path[3] === "endTime") id = `${row.key}-end`;
    else if (path[3] === "endDayOffset") id = `${row.key}-endDayOffset`;
    else if (path[3] === "weekdays") id = `${row.key}-weekdays`;
    else return false;
  }
  if (id === undefined) return false;
  await nextTick();
  if (!isCurrent() || props.draft !== raw) return false;
  const element = document.getElementById(id);
  if (element === null || !host.value?.contains(element)) return false;
  element.focus();
  return document.activeElement === element;
}
defineExpose({ focusField });
</script>

<template>
  <div ref="host" class="field-stack">
    <fieldset class="field-stack" :disabled="disabled || readOnly">
      <legend>{{ copy.kindLabel }}</legend>
      <label for="availability-kind">{{ copy.kindLabel }}</label>
      <select id="availability-kind" :value="draft.availabilityKind" @change="kind">
        <option value="unavailable">{{ AVAILABILITY_KIND_LABELS.unavailable }}</option>
        <option value="availableOnly">{{ AVAILABILITY_KIND_LABELS.availableOnly }}</option>
        <option value="approvedTimeOff">{{ AVAILABILITY_KIND_LABELS.approvedTimeOff }}</option>
        <option v-if="draft.availabilityKind === 'requestedTimeOff'" value="requestedTimeOff">
          {{ AVAILABILITY_KIND_LABELS.requestedTimeOff }}
        </option>
      </select>
      <p v-if="draft.availabilityKind === 'requestedTimeOff'" class="field-help" role="note">
        {{ copy.importedKindHelp }}
      </p>
      <p v-else class="field-help">{{ copy.kindHelp }}</p>
      <div
        id="availability-effective-range"
        class="form-columns"
        tabindex="-1"
        :aria-label="copy.effectiveDates"
      >
        <div class="field-stack">
          <label for="availability-start-date">{{ copy.startDate }}</label>
          <input
            id="availability-start-date"
            type="text"
            :value="draft.startDate"
            placeholder="YYYY-MM-DD"
            :aria-invalid="Boolean(errors.startDate)"
            aria-describedby="availability-start-error"
            @input="change({ startDate: text($event) })"
          />
          <p id="availability-start-error" class="field-help">{{ errors.startDate }}</p>
        </div>
        <div class="field-stack">
          <label for="availability-end-date">{{ copy.endDateExclusive }}</label>
          <input
            id="availability-end-date"
            type="text"
            :value="draft.endDateExclusive"
            placeholder="YYYY-MM-DD"
            :aria-invalid="Boolean(errors.endDateExclusive)"
            aria-describedby="availability-end-error"
            @input="change({ endDateExclusive: text($event) })"
          />
          <p id="availability-end-error" class="field-help">{{ errors.endDateExclusive }}</p>
        </div>
      </div>
    </fieldset>
    <fieldset
      id="availability-window"
      tabindex="-1"
      class="field-stack"
      :disabled="disabled || readOnly"
    >
      <legend>{{ copy.windowType }}</legend>
      <label for="availability-window-kind">{{ copy.windowType }}</label>
      <select
        id="availability-window-kind"
        :value="draft.windowKind"
        @change="change({ windowKind: text($event) === 'instant' ? 'instant' : 'weekly' })"
      >
        <option value="instant">{{ copy.windowTypeInstant }}</option>
        <option value="weekly">{{ copy.windowTypeWeekly }}</option>
      </select>
      <p v-if="errors.timeWindow" role="alert">{{ errors.timeWindow }}</p>
      <template v-if="draft.windowKind === 'instant'">
        <template v-if="draft.storedInstant !== null">
          <p id="availability-stored-start" tabindex="-1">
            {{ copy.storedStart }} <code>{{ draft.storedInstant.startsAt }}</code>
          </p>
          <p id="availability-stored-end" tabindex="-1">
            {{ copy.storedEnd }} <code>{{ draft.storedInstant.endsAt }}</code>
          </p>
          <label class="checkbox-label">
            <input
              id="availability-replace-instant"
              type="checkbox"
              :checked="draft.replaceInstant"
              @change="change({ replaceInstant: checked($event) })"
            />
            {{ copy.replaceStoredInterval }}
          </label>
          <p class="field-help">{{ copy.preserveStoredInterval }}</p>
        </template>
        <DateTimeRangeField
          v-if="draft.replaceInstant || draft.storedInstant === null"
          id="availability-local-interval"
          :label="copy.localInterval"
          :model-value="draft.localInterval"
          :time-zone="timeZone"
          :request-key="requestKey"
          :disabled="disabled"
          :read-only="readOnly"
          v-bind="locale === undefined ? {} : { locale }"
          @update:model-value="change({ localInterval: $event })"
        />
      </template>
      <template v-else>
        <fieldset
          v-for="(row, index) in draft.weekly"
          :key="row.key"
          class="state-panel field-stack"
        >
          <legend :id="`${row.key}-heading`" tabindex="-1">
            {{ copy.weeklyHeading(index + 1, locale) }}
          </legend>
          <DateTimeRangeField
            :id="row.key"
            :label="copy.recurringLocalInterval"
            :model-value="row.interval"
            :time-zone="timeZone"
            :request-key="requestKey"
            :disabled="disabled"
            :read-only="readOnly"
            v-bind="locale === undefined ? {} : { locale }"
            @update:model-value="weekly(row.key, { interval: $event })"
          />
          <fieldset :id="`${row.key}-weekdays`" tabindex="-1" class="field-stack">
            <legend>{{ copy.weeklyWeekdays }}</legend>
            <label v-for="day in availabilityWeekdays" :key="day" class="checkbox-label capitalize">
              <input
                type="checkbox"
                :checked="row.weekdays.includes(day)"
                @change="weekday(row, day, checked($event))"
              />{{ copy.weekdayNames[day] }}
            </label>
          </fieldset>
          <p v-if="errors[`timeWindow.windows.${String(index)}`]" role="alert">
            {{ errors[`timeWindow.windows.${String(index)}`] }}
          </p>
          <button type="button" @click="removeWeekly(row.key)">{{ copy.weeklyRemoveEntry }}</button>
        </fieldset>
        <button ref="addWeeklyButton" type="button" @click="addWeekly">
          {{ copy.weeklyAddEntry }}
        </button>
      </template>
    </fieldset>
    <fieldset class="field-stack" :disabled="disabled || readOnly">
      <legend>{{ copy.optionalAssignmentRestrictions }}</legend>
      <label class="checkbox-label"
        ><input
          type="checkbox"
          :checked="draft.restrictTypes"
          @change="change({ restrictTypes: checked($event) })"
        />{{ copy.assignmentTypeRestriction }}</label
      >
      <WorkforceEntityPicker
        v-if="draft.restrictTypes"
        id="availability-types"
        kind="assignmentType"
        :label="copy.selectedAssignmentTypes"
        :model-value="draft.assignmentTypeIds"
        multiple
        :project="project"
        :library-epoch="libraryEpoch"
        :disabled="disabled"
        :read-only="readOnly"
        v-bind="locale === undefined ? {} : { locale }"
        @update:model-value="change({ assignmentTypeIds: $event })"
      />
      <p v-if="errors.assignmentTypeIds" role="alert">{{ errors.assignmentTypeIds }}</p>
      <label class="checkbox-label"
        ><input
          type="checkbox"
          :checked="draft.restrictLocations"
          @change="change({ restrictLocations: checked($event) })"
        />{{ copy.locationRestriction }}</label
      >
      <WorkforceEntityPicker
        v-if="draft.restrictLocations"
        id="availability-locations"
        kind="location"
        :label="copy.selectedLocations"
        :model-value="draft.locationIds"
        multiple
        :project="project"
        :library-epoch="libraryEpoch"
        :disabled="disabled"
        :read-only="readOnly"
        v-bind="locale === undefined ? {} : { locale }"
        @update:model-value="change({ locationIds: $event })"
      />
      <p v-if="errors.locationIds" role="alert">{{ errors.locationIds }}</p>
      <p class="field-help">
        {{ copy.restrictionHelp }}
      </p>
    </fieldset>
    <div class="field-stack">
      <label for="availability-source">{{ copy.source }}</label>
      <input
        id="availability-source"
        type="text"
        :value="draft.source"
        :disabled="disabled"
        :readonly="readOnly"
        aria-describedby="availability-source-help"
        @input="change({ source: text($event) })"
      />
      <p id="availability-source-help" class="field-help">{{ copy.sourceHelp }}</p>
      <label for="availability-note">{{ copy.note }}</label>
      <textarea
        id="availability-note"
        :value="draft.note"
        :disabled="disabled"
        :readonly="readOnly"
        aria-describedby="availability-note-help"
        @input="change({ note: text($event) })"
      />
      <p id="availability-note-help" class="field-help">{{ copy.noteHelp }}</p>
    </div>
  </div>
</template>
