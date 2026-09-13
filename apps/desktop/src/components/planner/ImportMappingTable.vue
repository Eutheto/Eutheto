<script setup lang="ts">
import { computed, nextTick, ref, useId } from "vue";
import type { PeopleCsvColumn } from "../../api/generated";
import {
  allowsClear,
  boundedMappingMetadata,
  mappingRows,
  MAX_MAPPING_FIELDS,
  peopleCsvFields,
  validMappingWidth,
  validSourceIndex,
} from "./import-mapping";
import type {
  ImportMappingRow,
  ImportMappingTableEvents,
  ImportMappingTableProps,
} from "./import-mapping";
import { plannerMessage } from "./messages";

const props = defineProps<ImportMappingTableProps>();
const emit = defineEmits<ImportMappingTableEvents>();
const generatedId = useId();
const fieldId = computed(() => props.id ?? generatedId);
const root = ref<HTMLFieldSetElement | null>(null);
const locked = computed(() => props.disabled || props.readOnly);
const rows = computed(() => mappingRows(props.modelValue, props.expectedColumns));
const widthValid = computed(() => validMappingWidth(props.expectedColumns));
const sourceIndices = computed(() =>
  widthValid.value ? Array.from({ length: props.expectedColumns }, (_, index) => index) : [],
);
const metadata = computed(() => boundedMappingMetadata(props.headerCells, props.selectedSamples));
const describedBy = computed(() =>
  [
    `${fieldId.value}-help`,
    props.description ? `${fieldId.value}-description` : "",
    props.readOnly ? `${fieldId.value}-readonly` : "",
    !widthValid.value ? `${fieldId.value}-width` : "",
    props.modelValue.length > MAX_MAPPING_FIELDS ? `${fieldId.value}-limit` : "",
    props.nativeInvalidMapping ? `${fieldId.value}-native` : "",
    props.error ? `${fieldId.value}-error` : "",
  ]
    .filter(Boolean)
    .join(" "),
);

function rowId(row: ImportMappingRow): string {
  return `${fieldId.value}-${row.position === null ? `source-${row.index.toString()}` : `mapping-${row.position.toString()}`}`;
}
function rowDescription(row: ImportMappingRow): string {
  return [
    `${fieldId.value}-help`,
    `${rowId(row)}-metadata`,
    row.errors.length ? `${rowId(row)}-errors` : "",
  ]
    .filter(Boolean)
    .join(" ");
}
function rowLabel(row: ImportMappingRow): string {
  return plannerMessage(
    row.position === null ? "mapping.column" : "mapping.row",
    {
      column: row.index + 1,
      position: (row.position ?? 0) + 1,
    },
    props.locale,
  );
}
function replaceRow(row: ImportMappingRow, column: PeopleCsvColumn) {
  if (locked.value) return;
  if (row.position === null) {
    if (props.modelValue.length >= MAX_MAPPING_FIELDS || !widthValid.value) return;
    emit("update:modelValue", [...props.modelValue, column]);
  } else {
    emit(
      "update:modelValue",
      props.modelValue.map((current, position) => (position === row.position ? column : current)),
    );
  }
}
async function changeField(row: ImportMappingRow, event: Event) {
  if (locked.value) return;
  const value = (event.target as HTMLSelectElement).value;
  if (value === "") {
    if (row.position === null) return;
    emit(
      "update:modelValue",
      props.modelValue.filter((_, position) => position !== row.position),
    );
    await nextTick();
    // Unmapping can remove this row (including a duplicate or invalid source).
    const sameColumn = root.value?.querySelector<HTMLSelectElement>(
      `select[data-source-index="${row.index.toString()}"]:not(:disabled)`,
    );
    (
      sameColumn ??
      root.value?.querySelector<HTMLSelectElement>("select:not(:disabled)") ??
      root.value
    )?.focus();
    return;
  }
  const field = peopleCsvFields.find((candidate) => candidate === value);
  if (field === undefined || field === row.column?.field) return;
  // A field edit never silently repairs an existing disallowed blank policy.
  replaceRow(row, { index: row.index, field, blank: row.column?.blank ?? "preserve" });
  if (row.position === null) {
    await nextTick();
    root.value
      ?.querySelector<HTMLSelectElement>(
        `select[data-source-index="${row.index.toString()}"]:not(:disabled)`,
      )
      ?.focus();
  }
}
function changeSource(row: ImportMappingRow, event: Event) {
  if (!row.column || locked.value) return;
  const index = Number((event.target as HTMLSelectElement).value);
  if (!widthValid.value || !validSourceIndex(index, props.expectedColumns) || index === row.index)
    return;
  replaceRow(row, { ...row.column, index });
}
function changeBlank(row: ImportMappingRow, event: Event) {
  if (!row.column || locked.value) return;
  const blank = (event.target as HTMLSelectElement).value;
  if ((blank !== "preserve" && blank !== "clear") || blank === row.column.blank) return;
  if (blank === "clear" && !allowsClear(row.column.field)) return;
  replaceRow(row, { ...row.column, blank });
}
</script>

<template>
  <fieldset
    :id="fieldId"
    ref="root"
    class="form-fields min-w-0"
    tabindex="-1"
    :disabled="disabled"
    :aria-describedby="describedBy"
  >
    <legend>{{ label }}</legend>
    <p v-if="description" :id="`${fieldId}-description`" class="field-help">{{ description }}</p>
    <p :id="`${fieldId}-help`" class="field-help">{{ plannerMessage("mapping.help") }}</p>
    <p v-if="readOnly" :id="`${fieldId}-readonly`" class="field-help">
      {{ plannerMessage("mapping.readOnly") }}
    </p>
    <p v-if="!widthValid" :id="`${fieldId}-width`" class="text-sm text-danger" role="alert">
      {{ plannerMessage("mapping.width", { width: expectedColumns }, locale) }}
    </p>
    <p
      v-if="modelValue.length > MAX_MAPPING_FIELDS"
      :id="`${fieldId}-limit`"
      class="text-sm text-danger"
      role="alert"
    >
      {{ plannerMessage("mapping.mappingLimit") }}
    </p>
    <p
      v-if="nativeInvalidMapping"
      :id="`${fieldId}-native`"
      class="text-sm text-danger"
      role="alert"
    >
      {{ plannerMessage("mapping.nativeInvalidMapping") }}
    </p>
    <p v-if="error" :id="`${fieldId}-error`" class="text-sm text-danger" role="alert">
      {{ error }}
    </p>
    <p v-if="modelValue.length === 0" class="field-help">{{ plannerMessage("mapping.empty") }}</p>
    <p v-if="metadata.omitted" class="field-help">{{ plannerMessage("mapping.metadataLimit") }}</p>
    <p v-if="modelValue.length >= MAX_MAPPING_FIELDS" class="field-help">
      {{ plannerMessage("mapping.addLimit") }}
    </p>
    <div v-if="rows.length" class="overflow-x-auto">
      <table class="w-full min-w-3xl table-fixed border-collapse text-left text-sm">
        <caption class="sr-only">
          {{
            label
          }}
        </caption>
        <thead>
          <tr>
            <th scope="col" class="p-2">{{ plannerMessage("mapping.source") }}</th>
            <th scope="col" class="p-2">{{ plannerMessage("mapping.field") }}</th>
            <th scope="col" class="p-2">{{ plannerMessage("mapping.blank") }}</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="row in rows" :key="rowId(row)" class="align-top">
            <th scope="row" class="p-2 font-normal">
              <p :id="`${rowId(row)}-label`" class="font-medium">{{ rowLabel(row) }}</p>
              <template v-if="row.column">
                <label
                  :id="`${rowId(row)}-source-label`"
                  :for="`${rowId(row)}-source`"
                  class="sr-only"
                >
                  {{ plannerMessage("mapping.source") }}
                </label>
                <select
                  :id="`${rowId(row)}-source`"
                  :value="String(row.index)"
                  :disabled="locked || !widthValid"
                  :aria-labelledby="`${rowId(row)}-label ${rowId(row)}-source-label`"
                  :aria-describedby="rowDescription(row)"
                  :aria-invalid="row.errors.length > 0 || undefined"
                  @change="changeSource(row, $event)"
                >
                  <option
                    v-if="!widthValid || !validSourceIndex(row.index, expectedColumns)"
                    :value="String(row.index)"
                    disabled
                  >
                    {{
                      plannerMessage(
                        "mapping.invalidSource",
                        { index: row.index, column: row.index + 1 },
                        locale,
                      )
                    }}
                  </option>
                  <option v-for="index in sourceIndices" :key="index" :value="String(index)">
                    {{ plannerMessage("mapping.column", { column: index + 1 }, locale) }}
                  </option>
                </select>
              </template>
              <div
                :id="`${rowId(row)}-metadata`"
                class="field-help max-w-sm whitespace-pre-wrap break-words"
              >
                <template v-if="validSourceIndex(row.index, expectedColumns)">
                  <p v-if="metadata.headers[row.index]">
                    {{ plannerMessage("mapping.header") }}
                    {{ metadata.headers[row.index]?.text || plannerMessage("mapping.emptyText") }}
                    <span v-if="metadata.headers[row.index]?.truncated">
                      {{ plannerMessage("mapping.truncated") }}</span
                    >
                  </p>
                  <p v-else>{{ plannerMessage("mapping.noHeader") }}</p>
                  <p v-for="(sample, sampleIndex) in metadata.samples" :key="sampleIndex">
                    {{ plannerMessage("mapping.sample", { record: sample.record }, locale) }}
                    {{ sample.cells[row.index]?.text ?? plannerMessage("mapping.missingCell") }}
                    <span v-if="sample.cells[row.index]?.text === ''">{{
                      plannerMessage("mapping.emptyText")
                    }}</span>
                    <span v-if="sample.cells[row.index]?.truncated">
                      {{ plannerMessage("mapping.truncated") }}</span
                    >
                  </p>
                  <p v-if="metadata.samples.length === 0">
                    {{ plannerMessage("mapping.noSamples") }}
                  </p>
                </template>
                <p v-else>{{ plannerMessage("mapping.noMetadata") }}</p>
              </div>
            </th>
            <td class="p-2">
              <label :id="`${rowId(row)}-field-label`" :for="`${rowId(row)}-field`" class="sr-only">
                {{ plannerMessage("mapping.field") }}
              </label>
              <select
                :id="`${rowId(row)}-field`"
                :data-source-index="row.index"
                :value="row.column?.field ?? ''"
                :disabled="
                  locked || (row.position === null && modelValue.length >= MAX_MAPPING_FIELDS)
                "
                :aria-labelledby="`${rowId(row)}-label ${rowId(row)}-field-label`"
                :aria-describedby="rowDescription(row)"
                :aria-invalid="row.errors.length > 0 || undefined"
                @change="changeField(row, $event)"
              >
                <option value="">{{ plannerMessage("mapping.unmapped") }}</option>
                <option v-for="field in peopleCsvFields" :key="field" :value="field">
                  {{ plannerMessage(`mapping.field.${field}`) }}
                </option>
              </select>
              <ul
                v-if="row.errors.length"
                :id="`${rowId(row)}-errors`"
                class="mt-2 text-sm text-danger"
              >
                <li v-for="issue in row.errors" :key="issue">
                  {{ plannerMessage(`mapping.${issue}`) }}
                </li>
              </ul>
            </td>
            <td class="p-2">
              <label :id="`${rowId(row)}-blank-label`" :for="`${rowId(row)}-blank`" class="sr-only">
                {{ plannerMessage("mapping.blank") }}
              </label>
              <select
                :id="`${rowId(row)}-blank`"
                :value="row.column?.blank ?? 'preserve'"
                :disabled="locked || !row.column"
                :aria-labelledby="`${rowId(row)}-label ${rowId(row)}-blank-label`"
                :aria-describedby="rowDescription(row)"
                :aria-invalid="row.errors.length > 0 || undefined"
                @change="changeBlank(row, $event)"
              >
                <option value="preserve">{{ plannerMessage("mapping.preserve") }}</option>
                <option value="clear" :disabled="!row.column || !allowsClear(row.column.field)">
                  {{ plannerMessage("mapping.clear") }}
                </option>
              </select>
              <p v-if="row.column && !allowsClear(row.column.field)" class="field-help">
                {{ plannerMessage("mapping.clearUnavailable") }}
              </p>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </fieldset>
</template>
