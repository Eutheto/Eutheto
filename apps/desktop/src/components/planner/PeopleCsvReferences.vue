<script setup lang="ts">
import { computed, nextTick, ref, useId, watch } from "vue";
import { newUuidV7 } from "../../api/generated";
import type { ProjectSummary } from "../../project-home";
import { plannerMessage } from "./messages";
import WorkforceEntityPicker from "./WorkforceEntityPicker.vue";
import {
  csvReferenceKinds,
  type CsvReferenceDraft,
  type CsvReferenceKind,
} from "./people-csv-references";

const props = defineProps<{
  modelValue: readonly CsvReferenceDraft[];
  project: ProjectSummary;
  libraryEpoch: number;
  locale?: string;
  disabled?: boolean;
  errors?: Readonly<Record<string, string>>;
}>();
const emit = defineEmits<{ "update:modelValue": [value: readonly CsvReferenceDraft[]] }>();
const prefix = useId();
const page = ref(0);
const pages = computed(() => Math.max(1, Math.ceil(props.modelValue.length / 50)));
watch(pages, (value) => {
  page.value = Math.min(page.value, value - 1);
});
const rows = computed(() => props.modelValue.slice(page.value * 50, (page.value + 1) * 50));
const pickerContext = computed(() => ({
  project: props.project,
  libraryEpoch: props.libraryEpoch,
  disabled: props.disabled,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
async function focus(id: string): Promise<void> {
  await nextTick();
  document.getElementById(id)?.focus();
}
function update(
  key: string,
  patch: Partial<Pick<CsvReferenceDraft, "token" | "kind" | "entityId">>,
): void {
  if (props.disabled) return;
  emit(
    "update:modelValue",
    props.modelValue.map((row) => (row.key === key ? { ...row, ...patch } : row)),
  );
}
function kind(key: string, event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  const selected = csvReferenceKinds.find((item) => item === value);
  if (value === "" || selected !== undefined) update(key, { kind: selected ?? "", entityId: "" });
}
async function add(): Promise<void> {
  if (props.disabled || props.modelValue.length >= 10_000) return;
  const key = newUuidV7();
  page.value = Math.floor(props.modelValue.length / 50);
  emit("update:modelValue", [...props.modelValue, { key, token: "", kind: "", entityId: "" }]);
  await focus(`${prefix}-${key}-token`);
}
async function remove(key: string): Promise<void> {
  if (props.disabled) return;
  const next = props.modelValue.filter((row) => row.key !== key);
  page.value = Math.min(page.value, Math.max(0, Math.ceil(next.length / 50) - 1));
  emit("update:modelValue", next);
  await focus(`${prefix}-heading`);
}
async function changePage(change: number): Promise<void> {
  page.value = Math.max(0, Math.min(pages.value - 1, page.value + change));
  await focus(`${prefix}-heading`);
}
async function focusRow(key: string): Promise<void> {
  const index = props.modelValue.findIndex((row) => row.key === key);
  if (index < 0) return;
  page.value = Math.floor(index / 50);
  await focus(`${prefix}-${key}-token`);
}
defineExpose({ focusRow });
function label(kind: CsvReferenceKind): string {
  return plannerMessage(`csvReference.${kind}`);
}
</script>

<template>
  <fieldset class="field-stack" :disabled="disabled">
    <legend :id="`${prefix}-heading`" tabindex="-1">
      {{ plannerMessage("csvReference.title") }}
    </legend>
    <p class="field-help">{{ plannerMessage("csvReference.help") }}</p>
    <p v-if="modelValue.length === 0">{{ plannerMessage("csvReference.empty") }}</p>
    <article v-for="row in rows" :key="row.key" class="field-stack">
      <label :for="`${prefix}-${row.key}-token`">{{ plannerMessage("csvReference.token") }}</label>
      <input
        :id="`${prefix}-${row.key}-token`"
        type="text"
        :value="row.token"
        maxlength="64"
        required
        :aria-invalid="Boolean(errors?.[row.key]) || undefined"
        :aria-describedby="`${prefix}-${row.key}-error`"
        @input="update(row.key, { token: ($event.target as HTMLInputElement).value })"
      />
      <label :for="`${prefix}-${row.key}-kind`">{{ plannerMessage("csvReference.kind") }}</label>
      <select
        :id="`${prefix}-${row.key}-kind`"
        :value="row.kind"
        required
        @change="kind(row.key, $event)"
      >
        <option value="">{{ plannerMessage("csvReference.chooseKind") }}</option>
        <option v-for="item in csvReferenceKinds" :key="item" :value="item">
          {{ label(item) }}
        </option>
      </select>
      <WorkforceEntityPicker
        v-if="row.kind !== ''"
        v-bind="pickerContext"
        :kind="row.kind"
        :label="plannerMessage('csvReference.target')"
        :model-value="row.entityId ? [row.entityId] : []"
        required
        @update:model-value="update(row.key, { entityId: $event[0] ?? '' })"
      />
      <p :id="`${prefix}-${row.key}-error`" class="field-error">{{ errors?.[row.key] }}</p>
      <button type="button" @click="remove(row.key)">
        {{ plannerMessage("csvReference.remove") }}
      </button>
    </article>
    <p role="status">
      {{
        plannerMessage(
          "csvReference.page",
          { page: page + 1, pages, total: modelValue.length },
          locale,
        )
      }}
    </p>
    <div class="action-row">
      <button type="button" :disabled="page === 0" @click="changePage(-1)">
        {{ plannerMessage("csvDecision.previous") }}
      </button>
      <button type="button" :disabled="page + 1 >= pages" @click="changePage(1)">
        {{ plannerMessage("csvDecision.next") }}
      </button>
      <button type="button" :disabled="modelValue.length >= 10_000" @click="add">
        {{ plannerMessage("csvReference.add") }}
      </button>
    </div>
  </fieldset>
</template>
