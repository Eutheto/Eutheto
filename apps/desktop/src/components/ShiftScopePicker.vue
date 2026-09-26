<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, useId, watch } from "vue";
import { getScenarioView, SetupOperationScope } from "../api/generated";
import type {
  WorkforceDateRange,
  WorkforceSetupWorkPage,
  WorkforceSetupWorkShift,
} from "../api/generated-domain-pack-contracts";
import { safeMessage, type ProjectSummary } from "../project-home";
import { formatNumber, messages } from "../messages";

const props = defineProps<{
  readonly id: string;
  readonly selectedIds: readonly string[];
  readonly project: ProjectSummary;
  readonly libraryEpoch: number;
  readonly dates: WorkforceDateRange;
  readonly error?: string | undefined;
  readonly disabled?: boolean;
  readonly readOnly?: boolean;
  readonly locale?: string;
}>();
const emit = defineEmits<{ "update:modelValue": [value: readonly string[]] }>();
const copy = messages.work.fields;
const prefix = useId();
const host = ref<HTMLElement>();
const page = shallowRef<WorkforceSetupWorkPage | null>(null);
const loading = ref(false);
const loadError = ref<string | null>(null);
const selectedPage = ref(0);
const selectedOffset = computed(
  () =>
    Math.min(selectedPage.value, Math.max(0, Math.ceil(props.selectedIds.length / 50) - 1)) * 50,
);
const selectedIdsPage = computed(() =>
  props.selectedIds.slice(selectedOffset.value, selectedOffset.value + 50),
);
const selected = shallowRef<
  ReadonlyMap<
    string,
    { readonly shift: WorkforceSetupWorkShift | null; readonly error: string | null }
  >
>(new Map());
const selectedLoading = ref(false);
let windowScope: SetupOperationScope | null = null;
let selectedScope: SetupOperationScope | null = null;
let windowGeneration = 0;
let selectedGeneration = 0;
let alive = true;
const context = () => ({
  id: props.project.scenarioId,
  revision: props.project.revision,
  epoch: props.libraryEpoch,
});
function matches(captured: ReturnType<typeof context>): boolean {
  return (
    alive &&
    captured.id === props.project.scenarioId &&
    captured.revision === props.project.revision &&
    captured.epoch === props.libraryEpoch
  );
}
async function loadWindow(next = false): Promise<void> {
  const continuation = next ? page.value?.continuation : null;
  if (next && (continuation === null || continuation === undefined)) return;
  const captured = context();
  const dates = { ...props.dates };
  const generation = ++windowGeneration;
  windowScope?.dispose();
  const owned = new SetupOperationScope(captured.id, captured.revision);
  windowScope = owned;
  const current = () =>
    generation === windowGeneration &&
    matches(captured) &&
    props.dates.startDate === dates.startDate &&
    props.dates.endDateExclusive === dates.endDateExclusive;
  loading.value = true;
  loadError.value = null;
  page.value = null;
  try {
    const response = await getScenarioView(owned, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId: "official.workforce.setup.work_window",
        parameters: { dates, limit: 50 },
        continuation: continuation ?? null,
      },
    }).result;
    if (current()) page.value = response.result.view.data.result.data;
  } catch (failure) {
    if (current()) loadError.value = safeMessage(failure);
  } finally {
    owned.dispose();
    if (windowScope === owned) windowScope = null;
    if (current()) loading.value = false;
  }
}
async function loadSelected(): Promise<void> {
  const captured = context();
  const selection = props.selectedIds;
  const offset = selectedOffset.value;
  const ids = selection.slice(offset, offset + 50);
  const generation = ++selectedGeneration;
  selectedScope?.dispose();
  const owned = new SetupOperationScope(captured.id, captured.revision);
  selectedScope = owned;
  const current = () =>
    generation === selectedGeneration &&
    matches(captured) &&
    props.selectedIds === selection &&
    selectedOffset.value === offset;
  const rows = new Map<
    string,
    { readonly shift: WorkforceSetupWorkShift | null; readonly error: string | null }
  >();
  for (const id of ids) {
    const known =
      page.value?.items.find((shift) => shift.shiftId === id) ?? selected.value.get(id)?.shift;
    if (known !== null && known !== undefined) rows.set(id, { shift: known, error: null });
  }
  selected.value = rows;
  selectedLoading.value = true;
  const missing = ids.filter((id) => !rows.has(id));
  let next = 0;
  async function worker(): Promise<void> {
    while (current()) {
      const id = missing[next++];
      if (id === undefined) return;
      try {
        const response = await getScenarioView(owned, {
          source: { kind: "stored" },
          query: {
            schemaVersion: 1,
            viewId: "official.workforce.setup.work_detail",
            parameters: { shiftId: id },
          },
        }).result;
        if (!current()) return;
        const shift = response.result.view.data.result.data.shift;
        if (shift.shiftId !== id) throw new Error(copy.selectedIdentityMismatch);
        rows.set(id, { shift, error: null });
      } catch (failure) {
        if (!current()) return;
        rows.set(id, { shift: null, error: safeMessage(failure) });
      }
      if (current()) selected.value = new Map(rows);
    }
  }
  try {
    await Promise.all([worker(), worker()]);
  } finally {
    owned.dispose();
    if (selectedScope === owned) selectedScope = null;
    if (current()) selectedLoading.value = false;
  }
}
function toggle(id: string, checked: boolean): void {
  if (props.disabled || props.readOnly) return;
  if (checked && !page.value?.items.some((shift) => shift.shiftId === id)) return;
  if (props.selectedIds.includes(id) === checked) return;
  emit(
    "update:modelValue",
    checked
      ? [...props.selectedIds, id]
      : props.selectedIds.filter((selectedId) => selectedId !== id),
  );
}
function label(shift: WorkforceSetupWorkShift): string {
  return `${shift.templateName ?? shift.assignmentTypeName} · ${shift.interval.startsAt.local}–${shift.interval.endsAt.local}`;
}
async function focusField(path: readonly string[], isCurrent: () => boolean): Promise<boolean> {
  const captured = context();
  const selection = props.selectedIds;
  if (!isCurrent()) return false;
  let id = props.id;
  if (path.length === 1 && /^\d+$/.test(path[0] ?? "")) {
    const index = Number(path[0]);
    if (!Number.isSafeInteger(index) || selection[index] === undefined) return false;
    selectedPage.value = Math.floor(index / 50);
    id = `${prefix}-selected-${selection[index]}`;
  } else if (path.length !== 0) return false;
  await nextTick();
  if (!isCurrent() || !matches(captured) || props.selectedIds !== selection) return false;
  const element = document.getElementById(id);
  if (!(element instanceof HTMLElement) || !host.value?.contains(element)) return false;
  element.focus();
  return document.activeElement === element;
}
defineExpose({ focusField });
watch(
  [() => props.project.scenarioId, () => props.project.revision, () => props.libraryEpoch],
  () => {
    selected.value = new Map();
    page.value = null;
  },
  { flush: "sync" },
);
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.libraryEpoch,
    () => props.dates.startDate,
    () => props.dates.endDateExclusive,
  ],
  () => {
    void loadWindow();
  },
  { immediate: true, flush: "sync" },
);
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.libraryEpoch,
    () => props.selectedIds,
    selectedOffset,
  ],
  () => {
    void loadSelected();
  },
  { immediate: true, flush: "sync" },
);
onScopeDispose(() => {
  alive = false;
  windowGeneration += 1;
  selectedGeneration += 1;
  windowScope?.dispose();
  selectedScope?.dispose();
});
</script>

<template>
  <fieldset
    :id="id"
    ref="host"
    tabindex="-1"
    class="field-stack"
    :aria-describedby="`${prefix}-error`"
  >
    <legend>{{ copy.selectedShiftIds }}</legend>
    <p :id="`${prefix}-error`" class="text-danger" role="status">{{ error }}</p>
    <p>{{ copy.shiftWindowHelp(dates.startDate, dates.endDateExclusive) }}</p>
    <p v-if="loading" role="status">{{ copy.loading }}</p>
    <p v-if="loadError" role="alert">{{ loadError }}</p>
    <p v-if="page?.items.length === 0">{{ copy.noShiftsAvailable }}</p>
    <ul v-if="page" class="field-stack">
      <li v-for="shift in page.items" :key="shift.shiftId">
        <label :for="`${prefix}-choice-${shift.shiftId}`"
          ><input
            :id="`${prefix}-choice-${shift.shiftId}`"
            type="checkbox"
            :disabled="disabled || readOnly"
            :checked="selectedIds.includes(shift.shiftId)"
            @change="toggle(shift.shiftId, ($event.target as HTMLInputElement).checked)"
          />
          {{ label(shift) }}</label
        ><code class="break-all">{{ shift.shiftId }}</code>
      </li>
    </ul>
    <nav class="action-row" :aria-label="copy.shiftChoices">
      <button type="button" :disabled="disabled || loading" @click="loadWindow()">
        {{ copy.first }}</button
      ><button
        type="button"
        :disabled="disabled || loading || page?.continuation == null"
        @click="loadWindow(true)"
      >
        {{ copy.next }}
      </button>
    </nav>
    <h4>{{ copy.selectedShiftCount(formatNumber(selectedIds.length, locale)) }}</h4>
    <p v-if="selectedLoading" role="status">{{ copy.loadingSelectedShifts }}</p>
    <ul class="field-stack">
      <li
        v-for="shiftId in selectedIdsPage"
        :id="`${prefix}-selected-${shiftId}`"
        :key="shiftId"
        tabindex="-1"
      >
        <span v-if="selected.get(shiftId)?.shift">{{ label(selected.get(shiftId)!.shift!) }}</span>
        <code class="break-all">{{ shiftId }}</code>
        <p v-if="selected.get(shiftId)?.error" role="status">{{ selected.get(shiftId)?.error }}</p>
        <button
          v-if="!readOnly"
          type="button"
          :disabled="disabled"
          :aria-label="copy.removeSelectedShiftById(shiftId)"
          @click="toggle(shiftId, false)"
        >
          {{ copy.removeSelectedShift }}
        </button>
      </li>
    </ul>
    <nav class="action-row" :aria-label="copy.selectedShiftIds">
      <button
        type="button"
        :disabled="disabled || selectedOffset === 0"
        @click="selectedPage = selectedOffset / 50 - 1"
      >
        {{ copy.previous }}</button
      ><button
        type="button"
        :disabled="disabled || selectedOffset + 50 >= selectedIds.length"
        @click="selectedPage = selectedOffset / 50 + 1"
      >
        {{ copy.next }}</button
      ><button type="button" :disabled="disabled || selectedLoading" @click="loadSelected">
        {{ messages.work.refresh }}
      </button>
    </nav>
  </fieldset>
</template>
