<script setup lang="ts">
import { computed, nextTick, ref, useId, watch } from "vue";
import { formatNumber, messages } from "../messages";
import { workRowKey, type WorkCalendarPeriodDraft } from "../work-record-draft";
import DateTimeRangeField from "./planner/DateTimeRangeField.vue";
import type { TemporalDraft, TemporalFeedback } from "./planner/field-contracts";
import { parseTemporalDraft } from "./planner/temporal-field";

const props = defineProps<{
  readonly modelValue: WorkCalendarPeriodDraft;
  readonly requestKey: string;
  readonly timeZone: string;
  readonly locale?: string;
  readonly disabled?: boolean;
  readonly readOnly?: boolean;
  readonly errors?: Readonly<Record<string, string>>;
}>();
const emit = defineEmits<{ "update:modelValue": [value: WorkCalendarPeriodDraft] }>();
const copy = messages.work.fields;
const prefix = useId();
const host = ref<HTMLElement>();
const page = ref(0);
const locked = computed(() => props.disabled || props.readOnly);
const offset = computed(
  () => Math.min(page.value, Math.max(0, Math.ceil(props.modelValue.custom.length / 50) - 1)) * 50,
);
const rows = computed(() => props.modelValue.custom.slice(offset.value, offset.value + 50));
watch(
  [() => props.errors, () => props.modelValue],
  ([reported, draft], [before, beforeDraft]) => {
    if (draft.kind !== "custom" || (beforeDraft && draft !== beforeDraft)) return;
    let first = Infinity;
    for (const [path, message] of Object.entries(reported ?? {})) {
      if (!message || before?.[path]) continue;
      const match = /^period\.intervals\.(\d+)(?:\.|$)/u.exec(path);
      if (match) first = Math.min(first, Number(match[1]));
    }
    if (first < draft.custom.length) page.value = Math.floor(first / 50);
  },
  { immediate: true },
);
async function changePage(value: number): Promise<void> {
  if (props.disabled) return;
  page.value = value;
  await nextTick();
  const first = rows.value[0];
  if (first) host.value?.querySelector<HTMLElement>(`[data-row-key="${first.key}"] input`)?.focus();
}
const context = computed(() => ({
  disabled: props.disabled,
  readOnly: props.readOnly,
  requestKey: props.requestKey,
  timeZone: props.timeZone,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
const active = computed(() => {
  switch (props.modelValue.kind) {
    case "day":
      return props.modelValue.day;
    case "week":
      return props.modelValue.week;
    case "payPeriod":
      return props.modelValue.payPeriod;
    default:
      return null;
  }
});
function text(event: Event): string {
  return (event.target as HTMLInputElement | HTMLSelectElement).value;
}
function field(path: string) {
  return {
    id: `${prefix}-${path}`,
    name: path,
    disabled: props.disabled,
    readonly: props.readOnly,
    "aria-invalid": Boolean(props.errors?.[path]) || undefined,
    "aria-describedby": `${prefix}-${path}-error`,
  };
}
function mode(event: Event): void {
  const value = text(event);
  if (
    !locked.value &&
    (value === "" ||
      value === "day" ||
      value === "week" ||
      value === "payPeriod" ||
      value === "custom")
  )
    emit("update:modelValue", { ...props.modelValue, kind: value });
}
function set(field: "startTime" | "anchorDate" | "lengthDays", value: string): void {
  if (locked.value) return;
  const raw = props.modelValue;
  switch (raw.kind) {
    case "day":
      if (field === "startTime") emit("update:modelValue", { ...raw, day: { startTime: value } });
      break;
    case "week":
      if (field !== "lengthDays")
        emit("update:modelValue", { ...raw, week: { ...raw.week, [field]: value } });
      break;
    case "payPeriod":
      emit("update:modelValue", { ...raw, payPeriod: { ...raw.payPeriod, [field]: value } });
      break;
  }
}
function updateRow(key: string, interval: TemporalDraft): void {
  if (!locked.value && interval.raw.kind === "localInterval")
    emit("update:modelValue", {
      ...props.modelValue,
      custom: props.modelValue.custom.map((row) => (row.key === key ? { ...row, interval } : row)),
    });
}
function feedback(index: number): TemporalFeedback {
  const path = `period.intervals.${String(index)}`;
  const errors: TemporalFeedback["errors"][number][] = [];
  const start = props.errors?.[`${path}.startsAt`];
  const end = props.errors?.[`${path}.endsAt`];
  if (start) errors.push({ field: "start", message: start });
  if (end) errors.push({ field: "end", message: end });
  return { requestKey: props.requestKey, errors };
}
async function add(): Promise<void> {
  if (locked.value) return;
  const key = workRowKey();
  const count = props.modelValue.custom.length;
  emit("update:modelValue", {
    ...props.modelValue,
    custom: [
      ...props.modelValue.custom,
      { key, interval: parseTemporalDraft({ kind: "localInterval", startsAt: "", endsAt: "" }) },
    ],
  });
  page.value = Math.floor(count / 50);
  await nextTick();
  host.value?.querySelector<HTMLElement>(`[data-row-key="${key}"] input`)?.focus();
}
async function remove(key: string): Promise<void> {
  if (locked.value) return;
  const index = props.modelValue.custom.findIndex((row) => row.key === key);
  emit("update:modelValue", {
    ...props.modelValue,
    custom: props.modelValue.custom.filter((row) => row.key !== key),
  });
  await nextTick();
  const nearby = rows.value[Math.max(0, Math.min(index - offset.value, rows.value.length - 1))];
  if (nearby)
    host.value?.querySelector<HTMLElement>(`[data-row-key="${nearby.key}"] input`)?.focus();
  else host.value?.querySelector<HTMLElement>("[data-add-interval]")?.focus();
}
async function focusField(path: readonly string[], isCurrent: () => boolean): Promise<boolean> {
  if (!isCurrent()) return false;
  const raw = props.modelValue;
  let id: string | null = null;
  if (
    path.length === 1 &&
    (path[0] === "kind" ||
      path[0] === "anchorDate" ||
      path[0] === "startTime" ||
      path[0] === "lengthDays")
  )
    id = `${prefix}-period.${path[0]}`;
  else if (path.length === 1 && path[0] === "intervals") id = `${prefix}-intervals`;
  else if (
    raw.kind === "custom" &&
    path[0] === "intervals" &&
    (path.length === 2 || path.length === 3)
  ) {
    const index = Number(path[1]);
    const row = Number.isSafeInteger(index) && index >= 0 ? raw.custom[index] : undefined;
    if (row === undefined) return false;
    if (path.length === 2) id = `${prefix}-${row.key}-row`;
    else if (path[2] === "startsAt") id = `${prefix}-${row.key}-start`;
    else if (path[2] === "endsAt") id = `${prefix}-${row.key}-end`;
    else return false;
    page.value = Math.floor(index / 50);
  } else if (path.length !== 0) return false;
  await nextTick();
  if (!isCurrent() || props.modelValue !== raw) return false;
  const element = id === null ? host.value : document.getElementById(id);
  if (element == null || !host.value?.contains(element)) return false;
  element.focus();
  return document.activeElement === element;
}
defineExpose({ focusField });
</script>

<template>
  <fieldset ref="host" tabindex="-1" class="field-stack" :disabled="disabled">
    <legend>{{ copy.calendarPeriod }}</legend>
    <label :for="`${prefix}-period.kind`">{{ copy.calendarPeriod }}</label>
    <select
      v-bind="field('period.kind')"
      :disabled="locked"
      :value="modelValue.kind"
      @change="mode"
    >
      <option value="">{{ copy.selection }}</option>
      <option v-for="(label, value) in copy.periods" :key="value" :value="value">
        {{ label }}
      </option>
    </select>
    <p :id="`${prefix}-period.kind-error`" class="text-danger" role="status">
      {{ errors?.["period.kind"] }}
    </p>
    <template v-if="active">
      <div v-if="'anchorDate' in active" class="field-stack">
        <label :for="`${prefix}-period.anchorDate`">{{ copy.anchorDate }}</label>
        <input
          v-bind="field('period.anchorDate')"
          type="text"
          :spellcheck="false"
          :value="active.anchorDate"
          @input="set('anchorDate', text($event))"
        />
        <p :id="`${prefix}-period.anchorDate-error`" class="text-danger" role="status">
          {{ errors?.["period.anchorDate"] }}
        </p>
      </div>
      <div class="field-stack">
        <label :for="`${prefix}-period.startTime`">{{ copy.startTime }}</label>
        <input
          v-bind="field('period.startTime')"
          type="text"
          :spellcheck="false"
          :value="active.startTime"
          @input="set('startTime', text($event))"
        />
        <p :id="`${prefix}-period.startTime-error`" class="text-danger" role="status">
          {{ errors?.["period.startTime"] }}
        </p>
      </div>
      <div v-if="'lengthDays' in active" class="field-stack">
        <label :for="`${prefix}-period.lengthDays`">{{ copy.lengthDays }}</label>
        <input
          v-bind="field('period.lengthDays')"
          type="text"
          inputmode="numeric"
          :value="active.lengthDays"
          @input="set('lengthDays', text($event))"
        />
        <p :id="`${prefix}-period.lengthDays-error`" class="text-danger" role="status">
          {{ errors?.["period.lengthDays"] }}
        </p>
      </div>
    </template>
    <fieldset
      v-if="modelValue.kind === 'custom'"
      :id="`${prefix}-intervals`"
      tabindex="-1"
      class="field-stack"
    >
      <legend>{{ copy.customIntervals }}</legend>
      <p class="text-danger" role="status">{{ errors?.["period.intervals"] }}</p>
      <div
        v-for="(row, index) in rows"
        :id="`${prefix}-${row.key}-row`"
        :key="row.key"
        tabindex="-1"
        :data-row-key="row.key"
        class="field-stack"
      >
        <DateTimeRangeField
          v-bind="context"
          :id="`${prefix}-${row.key}`"
          :label="copy.customIntervalNumber(formatNumber(offset + index + 1, locale))"
          :model-value="row.interval"
          :error="errors?.[`period.intervals.${offset + index}`]"
          :feedback="feedback(offset + index)"
          @update:model-value="updateRow(row.key, $event)"
        />
        <button type="button" class="button-secondary" :disabled="locked" @click="remove(row.key)">
          {{ copy.removeInterval }}
        </button>
      </div>
      <nav
        :aria-label="copy.collectionPagesFor(copy.customIntervals)"
        class="flex flex-wrap items-center gap-2"
      >
        <span role="status">{{
          copy.collectionPageStatus(
            formatNumber(modelValue.custom.length, locale),
            formatNumber(offset / 50 + 1, locale),
          )
        }}</span>
        <button
          type="button"
          class="button-secondary"
          :disabled="disabled || offset === 0"
          @click="changePage(0)"
        >
          {{ copy.first }}
        </button>
        <button
          type="button"
          class="button-secondary"
          :disabled="disabled || offset === 0"
          @click="changePage(offset / 50 - 1)"
        >
          {{ copy.previous }}
        </button>
        <button
          type="button"
          class="button-secondary"
          :disabled="disabled || offset + 50 >= modelValue.custom.length"
          @click="changePage(offset / 50 + 1)"
        >
          {{ copy.next }}
        </button>
      </nav>
      <button
        type="button"
        class="button-secondary"
        data-add-interval
        :disabled="locked"
        @click="add"
      >
        {{ copy.addInterval }}
      </button>
    </fieldset>
  </fieldset>
</template>
