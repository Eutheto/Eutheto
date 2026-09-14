<script setup lang="ts">
import { computed, nextTick, ref, useId, watch } from "vue";
import { messages } from "../messages";
import type { ProjectSummary } from "../project-home";
import {
  workRowKey,
  type WorkCoverageDraft,
  type WorkQualificationMinimumDraft,
} from "../work-record-draft";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";

const props = defineProps<{
  readonly modelValue: WorkCoverageDraft;
  readonly project: ProjectSummary;
  readonly libraryEpoch: number;
  readonly locale?: string;
  readonly disabled?: boolean;
  readonly readOnly?: boolean;
  readonly errors?: Readonly<Record<string, string>>;
}>();
const emit = defineEmits<{ "update:modelValue": [value: WorkCoverageDraft] }>();
const copy = messages.work.fields;
const prefix = useId();
const host = ref<HTMLElement>();
const page = ref(0);
const locked = computed(() => props.disabled || props.readOnly);
const offset = computed(
  () =>
    Math.min(
      page.value,
      Math.max(0, Math.ceil(props.modelValue.qualificationMinimums.length / 50) - 1),
    ) * 50,
);
const rows = computed(() =>
  props.modelValue.qualificationMinimums.slice(offset.value, offset.value + 50),
);
watch(
  [() => props.errors, () => props.modelValue],
  ([reported, draft], [before, beforeDraft]) => {
    if (beforeDraft && draft !== beforeDraft) return;
    let first = Infinity;
    for (const [path, message] of Object.entries(reported ?? {})) {
      if (!message || before?.[path]) continue;
      const match = /^coverage\.qualificationMinimums\.(\d+)(?:\.|$)/u.exec(path);
      if (match) first = Math.min(first, Number(match[1]));
    }
    if (first < draft.qualificationMinimums.length) page.value = Math.floor(first / 50);
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
const pickerContext = computed(() => ({
  project: props.project,
  libraryEpoch: props.libraryEpoch,
  disabled: props.disabled,
  readOnly: props.readOnly,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
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
function set<K extends keyof WorkCoverageDraft>(key: K, value: WorkCoverageDraft[K]): void {
  if (!locked.value) emit("update:modelValue", { ...props.modelValue, [key]: value });
}
function mode(event: Event): void {
  const value = text(event);
  if (value === "" || value === "exact" || value === "atLeast") set("kind", value);
}
function updateRow<K extends keyof Omit<WorkQualificationMinimumDraft, "key">>(
  key: string,
  field: K,
  value: WorkQualificationMinimumDraft[K],
): void {
  set(
    "qualificationMinimums",
    props.modelValue.qualificationMinimums.map((row) =>
      row.key === key ? { ...row, [field]: value } : row,
    ),
  );
}
async function add(): Promise<void> {
  if (locked.value) return;
  const key = workRowKey();
  const count = props.modelValue.qualificationMinimums.length;
  set("qualificationMinimums", [
    ...props.modelValue.qualificationMinimums,
    { key, minimum: "", allQualificationIds: [], anyQualificationIds: [] },
  ]);
  page.value = Math.floor(count / 50);
  await nextTick();
  host.value?.querySelector<HTMLElement>(`[data-row-key="${key}"] input`)?.focus();
}
async function remove(key: string): Promise<void> {
  if (locked.value) return;
  const index = props.modelValue.qualificationMinimums.findIndex((row) => row.key === key);
  set(
    "qualificationMinimums",
    props.modelValue.qualificationMinimums.filter((row) => row.key !== key),
  );
  await nextTick();
  const nearby = rows.value[Math.max(0, Math.min(index - offset.value, rows.value.length - 1))];
  if (nearby)
    host.value?.querySelector<HTMLElement>(`[data-row-key="${nearby.key}"] input`)?.focus();
  else host.value?.querySelector<HTMLElement>("[data-add-minimum]")?.focus();
}
</script>

<template>
  <fieldset ref="host" class="field-stack" :disabled="disabled">
    <legend>{{ copy.coverage }}</legend>
    <div class="field-stack">
      <label :for="`${prefix}-coverage.kind`">{{ copy.coverageMode }}</label>
      <select
        v-bind="field('coverage.kind')"
        :disabled="locked"
        :value="modelValue.kind"
        @change="mode"
      >
        <option value="">{{ copy.selection }}</option>
        <option v-for="(label, value) in copy.coverageModes" :key="value" :value="value">
          {{ label }}
        </option>
      </select>
      <p :id="`${prefix}-coverage.kind-error`" class="text-danger" role="status">
        {{ errors?.["coverage.kind"] }}
      </p>
    </div>
    <div v-if="modelValue.kind === 'exact'" class="field-stack">
      <label :for="`${prefix}-coverage.count`">{{ copy.exactCount }}</label>
      <input
        v-bind="field('coverage.count')"
        type="text"
        inputmode="numeric"
        :value="modelValue.count"
        @input="set('count', text($event))"
      />
      <p :id="`${prefix}-coverage.count-error`" class="text-danger" role="status">
        {{ errors?.["coverage.count"] }}
      </p>
    </div>
    <template v-if="modelValue.kind === 'atLeast'">
      <div class="field-stack">
        <label :for="`${prefix}-coverage.minimum`">{{ copy.minimum }}</label>
        <input
          v-bind="field('coverage.minimum')"
          type="text"
          inputmode="numeric"
          :value="modelValue.minimum"
          @input="set('minimum', text($event))"
        />
        <p :id="`${prefix}-coverage.minimum-error`" class="text-danger" role="status">
          {{ errors?.["coverage.minimum"] }}
        </p>
      </div>
      <label :for="`${prefix}-preferred-enabled`">
        <input
          :id="`${prefix}-preferred-enabled`"
          type="checkbox"
          :disabled="locked"
          :checked="modelValue.preferredEnabled"
          @change="set('preferredEnabled', ($event.target as HTMLInputElement).checked)"
        />
        {{ copy.preferredEnabled }}
      </label>
      <div v-if="modelValue.preferredEnabled" class="field-stack">
        <label :for="`${prefix}-coverage.preferredCount`">{{ copy.preferredCount }}</label>
        <input
          v-bind="field('coverage.preferredCount')"
          type="text"
          inputmode="numeric"
          :value="modelValue.preferredCount"
          @input="set('preferredCount', text($event))"
        />
        <p :id="`${prefix}-coverage.preferredCount-error`" class="text-danger" role="status">
          {{ errors?.["coverage.preferredCount"] }}
        </p>
      </div>
      <label :for="`${prefix}-maximum-enabled`">
        <input
          :id="`${prefix}-maximum-enabled`"
          type="checkbox"
          :disabled="locked"
          :checked="modelValue.maximumEnabled"
          @change="set('maximumEnabled', ($event.target as HTMLInputElement).checked)"
        />
        {{ copy.maximumEnabled }}
      </label>
      <div v-if="modelValue.maximumEnabled" class="field-stack">
        <label :for="`${prefix}-coverage.maximumCount`">{{ copy.maximumCount }}</label>
        <input
          v-bind="field('coverage.maximumCount')"
          type="text"
          inputmode="numeric"
          :value="modelValue.maximumCount"
          @input="set('maximumCount', text($event))"
        />
        <p :id="`${prefix}-coverage.maximumCount-error`" class="text-danger" role="status">
          {{ errors?.["coverage.maximumCount"] }}
        </p>
      </div>
    </template>
    <fieldset class="field-stack">
      <legend>{{ copy.qualificationMinimums }}</legend>
      <p class="text-danger" role="status">{{ errors?.["coverage.qualificationMinimums"] }}</p>
      <fieldset
        v-for="(row, index) in rows"
        :key="row.key"
        :data-row-key="row.key"
        class="field-stack"
      >
        <legend>{{ copy.qualificationMinimum }} {{ offset + index + 1 }}</legend>
        <label :for="`${prefix}-coverage.qualificationMinimums.${offset + index}.minimum`">{{
          copy.qualificationMinimum
        }}</label>
        <input
          v-bind="field(`coverage.qualificationMinimums.${offset + index}.minimum`)"
          type="text"
          inputmode="numeric"
          :value="row.minimum"
          @input="updateRow(row.key, 'minimum', text($event))"
        />
        <p
          :id="`${prefix}-coverage.qualificationMinimums.${offset + index}.minimum-error`"
          class="text-danger"
          role="status"
        >
          {{ errors?.[`coverage.qualificationMinimums.${offset + index}.minimum`] }}
        </p>
        <WorkforceEntityPicker
          v-bind="pickerContext"
          :id="`${prefix}-${row.key}-all`"
          kind="qualification"
          multiple
          :label="copy.allQualifications"
          :model-value="row.allQualificationIds"
          :error="
            errors?.[
              `coverage.qualificationMinimums.${offset + index}.qualifications.allQualificationIds`
            ]
          "
          @update:model-value="updateRow(row.key, 'allQualificationIds', $event)"
        />
        <WorkforceEntityPicker
          v-bind="pickerContext"
          :id="`${prefix}-${row.key}-any`"
          kind="qualification"
          multiple
          :label="copy.anyQualifications"
          :model-value="row.anyQualificationIds"
          :error="
            errors?.[
              `coverage.qualificationMinimums.${offset + index}.qualifications.anyQualificationIds`
            ]
          "
          @update:model-value="updateRow(row.key, 'anyQualificationIds', $event)"
        />
        <button type="button" class="button-secondary" :disabled="locked" @click="remove(row.key)">
          {{ copy.removeQualificationMinimum }}
        </button>
      </fieldset>
      <nav
        :aria-label="`${copy.qualificationMinimums}: ${copy.collectionPages}`"
        class="flex flex-wrap items-center gap-2"
      >
        <span role="status"
          >{{ copy.rows }} {{ modelValue.qualificationMinimums.length }} · {{ copy.page }}
          {{ offset / 50 + 1 }}</span
        >
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
          :disabled="disabled || offset + 50 >= modelValue.qualificationMinimums.length"
          @click="changePage(offset / 50 + 1)"
        >
          {{ copy.next }}
        </button>
      </nav>
      <button
        type="button"
        class="button-secondary"
        data-add-minimum
        :disabled="locked"
        @click="add"
      >
        {{ copy.addQualificationMinimum }}
      </button>
    </fieldset>
  </fieldset>
</template>
