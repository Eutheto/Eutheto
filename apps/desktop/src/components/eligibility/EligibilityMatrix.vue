<script setup lang="ts">
import { computed, nextTick, ref, useId, watch } from "vue";
import {
  coreFeatures,
  createCoreRowModel,
  tableFeatures,
  useTable,
  type ColumnDef,
  type Row,
} from "@tanstack/vue-table";
import { defaultRangeExtractor, useVirtualizer } from "@tanstack/vue-virtual";
import type { WorkforceSetupPersonSummary } from "../../api/generated-domain-pack-contracts";
import { eligibilityCellKey, type EligibilityEdits } from "../../eligibility-draft";
import type { EligibilityWindow } from "../../eligibility-setup";
import { formatNumber, messages } from "../../messages";

const props = defineProps<{
  window: EligibilityWindow;
  edits: EligibilityEdits;
  disabled: boolean;
  selectedPersonId: string | null;
  selectedTypeId: string | null;
  locale?: string;
}>();
const emit = defineEmits<{
  select: [personId: string, typeId: string];
  inspect: [personId: string];
  cell: [personId: string, typeId: string, allowed: boolean];
  row: [personId: string, allowed: boolean];
  column: [typeId: string, allowed: boolean];
}>();
const id = useId(),
  scrollElement = ref<HTMLElement>();
const rowHeight = 64,
  columnWidth = 176,
  rowHeaderWidth = 224,
  headerHeight = 80;
const mode = ref<"virtual" | "paged">("virtual");
const rowPage = ref(0),
  columnPage = ref(0);
const active = ref<{ personId: string; typeId: string } | null>(null);
interface EligibilityRow {
  readonly person: WorkforceSetupPersonSummary;
  readonly memberships: readonly boolean[];
}
const features = tableFeatures({ ...coreFeatures, coreRowModel: createCoreRowModel() });
type MatrixRow = Row<typeof features, EligibilityRow>;
const data = computed<EligibilityRow[]>(() =>
  props.window.people.items.map((person, index) => ({
    person,
    memberships: props.window.matrix.configuredMemberships[index] ?? [],
  })),
);
const columns = computed<ColumnDef<typeof features, EligibilityRow>[]>(() =>
  props.window.types.items.map((type, index) => ({
    id: type.entityId,
    header: type.name ?? type.entityId,
    accessorFn: (row) => row.memberships[index],
  })),
);
const table = useTable<typeof features, EligibilityRow>({
  features,
  data,
  columns,
  getRowId: (row) => row.person.personId,
});
const rows = computed(() => table.getRowModel().rows);
const leafColumns = computed(() => table.getAllLeafColumns());
const activeRow = computed(() => rows.value.findIndex((row) => row.id === active.value?.personId));
const activeColumn = computed(() =>
  leafColumns.value.findIndex((column) => column.id === active.value?.typeId),
);
const rowVirtualizer = useVirtualizer(
  computed(() => {
    const pinned = activeRow.value;
    return {
      count: rows.value.length,
      getScrollElement: () => scrollElement.value ?? null,
      estimateSize: () => rowHeight,
      getItemKey: (index: number) => rows.value[index]?.id ?? index,
      overscan: 3,
      scrollMargin: headerHeight,
      scrollPaddingStart: headerHeight,
      rangeExtractor: (range: Parameters<typeof defaultRangeExtractor>[0]) =>
        [...new Set([...defaultRangeExtractor(range), ...(pinned < 0 ? [] : [pinned])])].sort(
          (a, b) => a - b,
        ),
    };
  }),
);
const columnVirtualizer = useVirtualizer(
  computed(() => {
    const pinned = activeColumn.value;
    return {
      horizontal: true,
      count: leafColumns.value.length,
      getScrollElement: () => scrollElement.value ?? null,
      estimateSize: () => columnWidth,
      getItemKey: (index: number) => leafColumns.value[index]?.id ?? index,
      overscan: 2,
      paddingStart: rowHeaderWidth,
      scrollPaddingStart: rowHeaderWidth,
      rangeExtractor: (range: Parameters<typeof defaultRangeExtractor>[0]) =>
        [...new Set([...defaultRangeExtractor(range), ...(pinned < 0 ? [] : [pinned])])].sort(
          (a, b) => a - b,
        ),
    };
  }),
);
const virtualRows = computed(() => rowVirtualizer.value.getVirtualItems());
const virtualColumns = computed(() => columnVirtualizer.value.getVirtualItems());
const pagedRows = computed(() => rows.value.slice(rowPage.value * 16, (rowPage.value + 1) * 16));
const pagedColumns = computed(() =>
  leafColumns.value.slice(columnPage.value * 8, (columnPage.value + 1) * 8),
);
const selectedPerson = computed(() => rows.value[activeRow.value]?.original.person);
const selectedType = computed(() => props.window.types.items[activeColumn.value]);
function select(personId: string, typeId: string): void {
  active.value = { personId, typeId };
  emit("select", personId, typeId);
}
watch(
  [rows, leafColumns],
  () => {
    const personId = props.selectedPersonId,
      typeId = props.selectedTypeId;
    const row = rows.value.find((item) => item.id === personId) ?? rows.value[0];
    const column = leafColumns.value.find((item) => item.id === typeId) ?? leafColumns.value[0];
    if (row !== undefined && column !== undefined) select(row.id, column.id);
    else active.value = null;
  },
  { immediate: true },
);
function allowed(row: MatrixRow, typeId: string): boolean {
  return (
    props.edits.get(eligibilityCellKey(row.id, typeId))?.after ?? row.getValue<boolean>(typeId)
  );
}
async function focusCell(rowIndex: number, columnIndex: number): Promise<void> {
  const row = rows.value[rowIndex],
    column = leafColumns.value[columnIndex];
  if (row === undefined || column === undefined) return;
  select(row.id, column.id);
  if (mode.value === "virtual") {
    rowVirtualizer.value.scrollToIndex(rowIndex, { align: "auto" });
    columnVirtualizer.value.scrollToIndex(columnIndex, { align: "auto" });
  } else {
    rowPage.value = Math.floor(rowIndex / 16);
    columnPage.value = Math.floor(columnIndex / 8);
  }
  await nextTick();
  document.getElementById(`${id}-cell-${row.id}-${column.id}`)?.focus({ preventScroll: true });
}
function keydown(event: KeyboardEvent, rowIndex: number, columnIndex: number): void {
  let row = rowIndex,
    column = columnIndex;
  switch (event.key) {
    case "ArrowUp":
      row -= 1;
      break;
    case "ArrowDown":
      row += 1;
      break;
    case "ArrowLeft":
      column -= 1;
      break;
    case "ArrowRight":
      column += 1;
      break;
    case "Home":
      column = 0;
      if (event.ctrlKey) row = 0;
      break;
    case "End":
      column = leafColumns.value.length - 1;
      if (event.ctrlKey) row = rows.value.length - 1;
      break;
    case "Enter": {
      const person = rows.value[rowIndex];
      if (person !== undefined) {
        event.preventDefault();
        emit("inspect", person.id);
      }
      return;
    }
    default:
      return;
  }
  event.preventDefault();
  void focusCell(
    Math.max(0, Math.min(rows.value.length - 1, row)),
    Math.max(0, Math.min(leafColumns.value.length - 1, column)),
  );
}
async function switchMode(): Promise<void> {
  mode.value = mode.value === "virtual" ? "paged" : "virtual";
  await nextTick();
  if (activeRow.value >= 0 && activeColumn.value >= 0)
    await focusCell(activeRow.value, activeColumn.value);
}
function page(axis: "row" | "column", delta: number): void {
  if (axis === "row") rowPage.value += delta;
  else columnPage.value += delta;
  const row = rows.value[rowPage.value * 16],
    column = leafColumns.value[columnPage.value * 8];
  if (row !== undefined && column !== undefined) select(row.id, column.id);
}
</script>

<template>
  <section class="page-stack" :aria-labelledby="`${id}-heading`" data-eligibility-matrix>
    <h3 :id="`${id}-heading`">{{ messages.eligibilityUi.matrix.heading }}</h3>
    <p :id="`${id}-help`">{{ messages.eligibilityUi.matrix.help }}</p>
    <p>
      {{
        messages.eligibilityUi.matrix.loaded(
          formatNumber(rows.length, locale),
          formatNumber(window.people.totalItems, locale),
          formatNumber(leafColumns.length, locale),
          formatNumber(window.types.totalItems, locale),
        )
      }}
    </p>
    <div class="action-row">
      <button type="button" @click="switchMode">
        {{
          mode === "virtual"
            ? messages.eligibilityUi.matrix.equivalentPaged
            : messages.eligibilityUi.matrix.virtual
        }}
      </button>
      <button v-if="selectedPerson" type="button" @click="emit('inspect', selectedPerson.personId)">
        {{ messages.eligibilityUi.matrix.inspect(selectedPerson.name) }}
      </button>
    </div>
    <div v-if="selectedPerson && selectedType" class="state-panel">
      <p aria-live="polite">
        {{
          messages.eligibilityUi.matrix.selected(
            selectedPerson.name,
            selectedType.name ?? selectedType.entityId,
          )
        }}
      </p>
      <div class="action-row">
        <button
          type="button"
          :disabled="disabled"
          @click="emit('row', selectedPerson.personId, true)"
        >
          {{ messages.eligibilityUi.matrix.allowRow(selectedPerson.name) }}
        </button>
        <button
          type="button"
          :disabled="disabled"
          @click="emit('row', selectedPerson.personId, false)"
        >
          {{ messages.eligibilityUi.matrix.disallowRow(selectedPerson.name) }}
        </button>
        <button
          type="button"
          :disabled="disabled"
          @click="emit('column', selectedType.entityId, true)"
        >
          {{
            messages.eligibilityUi.matrix.allowColumn(selectedType.name ?? selectedType.entityId)
          }}
        </button>
        <button
          type="button"
          :disabled="disabled"
          @click="emit('column', selectedType.entityId, false)"
        >
          {{
            messages.eligibilityUi.matrix.disallowColumn(selectedType.name ?? selectedType.entityId)
          }}
        </button>
      </div>
    </div>
    <p v-if="rows.length === 0 || leafColumns.length === 0" role="status">
      {{ messages.eligibilityUi.matrix.empty }}
    </p>
    <div
      v-else-if="mode === 'virtual'"
      ref="scrollElement"
      class="matrix-scroll"
      :aria-labelledby="`${id}-heading`"
      data-matrix-scroll
    >
      <table
        role="grid"
        :aria-describedby="`${id}-help`"
        :aria-rowcount="rows.length + 1"
        :aria-colcount="leafColumns.length + 1"
        :style="{ width: `${columnVirtualizer.getTotalSize()}px` }"
        class="virtual-table"
      >
        <thead :style="{ height: `${headerHeight}px` }">
          <tr :style="{ height: `${headerHeight}px` }" aria-rowindex="1">
            <th class="corner" scope="col" :style="{ width: `${rowHeaderWidth}px` }">
              {{ messages.eligibilityUi.matrix.corner }}
            </th>
            <th
              v-for="column in virtualColumns"
              :id="`${id}-type-${leafColumns[column.index]?.id}`"
              :key="leafColumns[column.index]?.id ?? column.index"
              scope="col"
              :aria-colindex="column.index + 2"
              class="virtual-column"
              :style="{ left: `${column.start}px`, width: `${column.size}px` }"
            >
              {{ window.types.items[column.index]?.name ?? leafColumns[column.index]?.id }}
            </th>
          </tr>
        </thead>
        <tbody :style="{ height: `${rowVirtualizer.getTotalSize()}px` }">
          <tr
            v-for="row in virtualRows"
            :key="rows[row.index]?.id ?? row.index"
            :aria-rowindex="row.index + 2"
            class="virtual-row"
            :style="{
              height: `${row.size}px`,
              transform: `translateY(${row.start - headerHeight}px)`,
            }"
          >
            <th
              :id="`${id}-person-${rows[row.index]?.id}`"
              scope="row"
              class="person-heading"
              :style="{ width: `${rowHeaderWidth}px` }"
            >
              {{ rows[row.index]?.original.person.name }}
            </th>
            <td
              v-for="column in virtualColumns"
              :key="leafColumns[column.index]?.id ?? column.index"
              role="gridcell"
              :aria-colindex="column.index + 2"
              :headers="`${id}-person-${rows[row.index]?.id} ${id}-type-${leafColumns[column.index]?.id}`"
              class="virtual-column"
              :style="{ left: `${column.start}px`, width: `${column.size}px` }"
            >
              <input
                v-if="rows[row.index] && leafColumns[column.index]"
                :id="`${id}-cell-${rows[row.index]!.id}-${leafColumns[column.index]!.id}`"
                type="checkbox"
                :checked="allowed(rows[row.index]!, leafColumns[column.index]!.id)"
                :aria-label="
                  messages.eligibilityUi.matrix.cell(
                    rows[row.index]!.original.person.name,
                    window.types.items[column.index]?.name ?? leafColumns[column.index]!.id,
                  )
                "
                :aria-disabled="disabled"
                :tabindex="activeRow === row.index && activeColumn === column.index ? 0 : -1"
                @focus="select(rows[row.index]!.id, leafColumns[column.index]!.id)"
                @keydown="keydown($event, row.index, column.index)"
                @click="disabled && $event.preventDefault()"
                @change="
                  emit(
                    'cell',
                    rows[row.index]!.id,
                    leafColumns[column.index]!.id,
                    ($event.target as HTMLInputElement).checked,
                  )
                "
              />
              <span
                v-if="
                  edits.has(
                    eligibilityCellKey(
                      rows[row.index]?.id ?? '',
                      leafColumns[column.index]?.id ?? '',
                    ),
                  )
                "
                class="pending-mark"
              >
                {{ messages.eligibilityUi.matrix.pending }}
              </span>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <template v-else>
      <div class="action-row">
        <button type="button" :disabled="rowPage === 0" @click="page('row', -1)">
          {{ messages.eligibilityUi.matrix.previousRows }}
        </button>
        <button type="button" :disabled="(rowPage + 1) * 16 >= rows.length" @click="page('row', 1)">
          {{ messages.eligibilityUi.matrix.nextRows }}
        </button>
        <button type="button" :disabled="columnPage === 0" @click="page('column', -1)">
          {{ messages.eligibilityUi.matrix.previousColumns }}
        </button>
        <button
          type="button"
          :disabled="(columnPage + 1) * 8 >= leafColumns.length"
          @click="page('column', 1)"
        >
          {{ messages.eligibilityUi.matrix.nextColumns }}
        </button>
      </div>
      <div
        class="overflow-x-auto"
        tabindex="0"
        role="region"
        :aria-label="messages.eligibilityUi.matrix.pagedRegion"
      >
        <table class="w-full border-collapse text-left text-sm">
          <caption>
            {{
              messages.eligibilityUi.matrix.pagedCaption
            }}
          </caption>
          <thead>
            <tr>
              <th scope="col" class="p-2">{{ messages.eligibilityUi.matrix.person }}</th>
              <th v-for="column in pagedColumns" :key="column.id" scope="col" class="p-2">
                {{ window.types.items[leafColumns.indexOf(column)]?.name ?? column.id }}
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="row in pagedRows" :key="row.id" class="border-t border-line">
              <th scope="row" class="p-2">{{ row.original.person.name }}</th>
              <td v-for="column in pagedColumns" :key="column.id" class="p-2">
                <input
                  :id="`${id}-cell-${row.id}-${column.id}`"
                  type="checkbox"
                  :checked="allowed(row, column.id)"
                  :aria-label="
                    messages.eligibilityUi.matrix.cell(
                      row.original.person.name,
                      window.types.items[leafColumns.indexOf(column)]?.name ?? column.id,
                    )
                  "
                  :aria-disabled="disabled"
                  @focus="select(row.id, column.id)"
                  @keydown="keydown($event, row.index, leafColumns.indexOf(column))"
                  @click="disabled && $event.preventDefault()"
                  @change="
                    emit('cell', row.id, column.id, ($event.target as HTMLInputElement).checked)
                  "
                />
                <span v-if="edits.has(eligibilityCellKey(row.id, column.id))">{{
                  messages.eligibilityUi.matrix.pending
                }}</span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>
  </section>
</template>

<style scoped>
.matrix-scroll {
  overflow: auto;
  max-width: 100%;
  height: 32rem;
  position: relative;
  border: 1px solid var(--line);
}
.virtual-table {
  display: grid;
  table-layout: fixed;
  border-collapse: separate;
  border-spacing: 0;
}
.virtual-table thead {
  position: sticky;
  top: 0;
  z-index: 3;
  display: block;
  background: var(--raised);
}
.virtual-table tr {
  position: relative;
  width: 100%;
  display: flex;
}
.virtual-table tbody {
  display: block;
  position: relative;
}
.virtual-table .virtual-row {
  position: absolute;
  top: 0;
}
.virtual-table th,
.virtual-table td {
  box-sizing: border-box;
  padding: 0.5rem;
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  border-right: 1px solid var(--line);
  border-bottom: 1px solid var(--line);
}
.virtual-table .virtual-column {
  position: absolute;
  top: 0;
}
.virtual-table .corner,
.virtual-table .person-heading {
  position: sticky;
  left: 0;
  z-index: 2;
  flex-shrink: 0;
  background: var(--raised);
  justify-content: flex-start;
  overflow-wrap: anywhere;
  overflow: hidden;
}
.virtual-table th {
  overflow: hidden;
  overflow-wrap: anywhere;
}
.virtual-table input:focus-visible {
  outline: 3px solid currentColor;
  outline-offset: 4px;
}
.pending-mark {
  margin-inline-start: 0.5rem;
  font-size: 0.8rem;
}
.table-scroll {
  overflow: auto;
  max-width: 100%;
}
</style>
