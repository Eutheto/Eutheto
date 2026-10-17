<script setup lang="ts">
import { computed, nextTick, ref, useId, watch, type ComponentPublicInstance } from "vue";
import { defaultRangeExtractor, useVirtualizer } from "@tanstack/vue-virtual";
import type {
  WorkforceResultScheduleData,
  WorkforceResultScheduleShiftRow,
} from "../api/generated-domain-pack-contracts";
import { messages } from "../messages";

type CalendarPage = Extract<
  WorkforceResultScheduleData,
  { kind: "calendarPeople" | "calendarShifts" }
>;
const props = defineProps<{
  page: CalendarPage;
  timeZone: string;
  locale?: string;
  loading: boolean;
  localTime: (value: { readonly local: string; readonly offsetSeconds: number }) => string;
  coverageDescription: (shift: WorkforceResultScheduleShiftRow) => string;
}>();
const emit = defineEmits<{
  day: [personId: string, name: string, date: string];
  shift: [shiftId: string, label: string];
  location: [locationId: string, name: string];
  navigate: [days: -7 | 7];
}>();
const copy = messages.acceptedSchedule;
const id = useId();
const scrollElement = ref<HTMLElement>();
const mode = ref<"virtual" | "paged">("virtual");
const rowPage = ref(0);
const active = ref({ row: 0, column: 0 });
const headerHeight = 56;
const rowsPerPage = 8;
const dates = computed(() => {
  const { startDate, endDateExclusive } = props.page.window;
  if (!/^\d{4}-\d{2}-\d{2}$/.test(startDate) || !/^\d{4}-\d{2}-\d{2}$/.test(endDateExclusive))
    return [];
  const start = new Date(`${startDate}T00:00:00Z`);
  const end = new Date(`${endDateExclusive}T00:00:00Z`);
  if (
    !Number.isFinite(start.getTime()) ||
    !Number.isFinite(end.getTime()) ||
    start.toISOString().slice(0, 10) !== startDate ||
    end.toISOString().slice(0, 10) !== endDateExclusive ||
    end.getTime() <= start.getTime() ||
    end.getTime() - start.getTime() > 7 * 86_400_000 ||
    startDate < props.page.planningDates.startDate ||
    endDateExclusive > props.page.planningDates.endDateExclusive
  )
    return [];
  const result: string[] = [];
  for (let cursor = new Date(start); cursor < end; cursor.setUTCDate(cursor.getUTCDate() + 1))
    result.push(cursor.toISOString().slice(0, 10));
  return result;
});
function dateLabel(date: string): string {
  try {
    return new Intl.DateTimeFormat(props.locale, {
      calendar: "iso8601",
      timeZone: "UTC",
      weekday: "short",
      month: "short",
      day: "numeric",
    }).format(new Date(`${date}T00:00:00Z`));
  } catch {
    return date;
  }
}
function rowId(index: number): string {
  const row = props.page.items[index];
  if (!row) return String(index);
  return "person" in row ? row.person.personId : row.shift.shiftId;
}
function rowName(index: number): string {
  const row = props.page.items[index];
  if (!row) return copy.notAvailable;
  if ("person" in row) return row.person.name;
  const shift = row.shift;
  return `${shift.assignmentTypeName ?? shift.assignmentTypeId} · ${shift.locationName ?? copy.noLocation} · ${props.localTime(shift.startsAt)} (${shift.shiftId})`;
}
function cellCount(row: number, column: number): number {
  if (props.page.kind === "calendarPeople")
    return props.page.items[row]?.days[column]?.selectedCount ?? 0;
  const shift = props.page.items[row];
  return shift !== undefined && shift.date === dates.value[column] ? shift.shift.selectedCount : 0;
}
function shiftDescription(index: number): string {
  if (props.page.kind !== "calendarShifts") return "";
  const shift = props.page.items[index]?.shift;
  if (!shift) return "";
  return `${props.localTime(shift.startsAt)} – ${props.localTime(shift.endsAt)}. ${copy.calendar.staffing}: ${props.coverageDescription(shift)}`;
}
function cellId(row: number, column: number): string {
  return `${id}-cell-${rowId(row)}-${dates.value[column] ?? ""}`;
}
const rowVirtualizer = useVirtualizer(
  computed(() => ({
    count: props.page.items.length,
    getScrollElement: () => scrollElement.value ?? null,
    estimateSize: () => 176,
    getItemKey: rowId,
    overscan: 2,
    scrollMargin: headerHeight,
    scrollPaddingStart: headerHeight,
    rangeExtractor: (range: Parameters<typeof defaultRangeExtractor>[0]) => {
      const pinned = active.value.row;
      return [...new Set([...defaultRangeExtractor(range), ...(pinned < 0 ? [] : [pinned])])].sort(
        (a, b) => a - b,
      );
    },
  })),
);
const visibleRows = computed(() =>
  mode.value === "virtual"
    ? rowVirtualizer.value.getVirtualItems()
    : props.page.items
        .slice(rowPage.value * rowsPerPage, (rowPage.value + 1) * rowsPerPage)
        .map((_, offset) => ({ index: rowPage.value * rowsPerPage + offset, start: 0, size: 0 })),
);
function measureRow(element: Element | ComponentPublicInstance | null): void {
  if (element instanceof HTMLElement) rowVirtualizer.value.measureElement(element);
}
async function focusCell(row: number, column: number): Promise<void> {
  if (row < 0 || row >= props.page.items.length || column < 0 || column >= dates.value.length)
    return;
  const captured = props.page;
  active.value = { row, column };
  if (mode.value === "virtual") rowVirtualizer.value.scrollToIndex(row, { align: "auto" });
  else rowPage.value = Math.floor(row / rowsPerPage);
  await nextTick();
  if (props.page !== captured) return;
  const cell = document.getElementById(cellId(row, column));
  cell?.focus({ preventScroll: true });
  const scroller = scrollElement.value;
  if (!cell || !scroller) return;
  const bounds = cell.getBoundingClientRect();
  const viewport = scroller.getBoundingClientRect();
  const ownerWidth = scroller.querySelector("thead th")?.getBoundingClientRect().width ?? 0;
  if (bounds.right > viewport.right) scroller.scrollLeft += bounds.right - viewport.right;
  else if (bounds.left < viewport.left + ownerWidth)
    scroller.scrollLeft -= viewport.left + ownerWidth - bounds.left;
}
function keydown(event: KeyboardEvent, row: number, column: number): void {
  let nextRow = row;
  let nextColumn = column;
  switch (event.key) {
    case "ArrowUp":
      nextRow -= 1;
      break;
    case "ArrowDown":
      nextRow += 1;
      break;
    case "ArrowLeft":
      nextColumn -= 1;
      break;
    case "ArrowRight":
      nextColumn += 1;
      break;
    case "Home":
      nextColumn = 0;
      if (event.ctrlKey) nextRow = 0;
      break;
    case "End":
      nextColumn = dates.value.length - 1;
      if (event.ctrlKey) nextRow = props.page.items.length - 1;
      break;
    default:
      return;
  }
  event.preventDefault();
  void focusCell(
    Math.max(0, Math.min(props.page.items.length - 1, nextRow)),
    Math.max(0, Math.min(dates.value.length - 1, nextColumn)),
  );
}
function navigateRows(step: -1 | 1): void {
  rowPage.value += step;
  active.value = { row: rowPage.value * rowsPerPage, column: 0 };
}
async function switchMode(): Promise<void> {
  const focused = document.activeElement?.id === cellId(active.value.row, active.value.column);
  mode.value = mode.value === "virtual" ? "paged" : "virtual";
  if (mode.value === "paged") rowPage.value = Math.floor(active.value.row / rowsPerPage);
  if (focused) await focusCell(active.value.row, active.value.column);
}
function viewDay(index: number, date: string): void {
  if (props.page.kind !== "calendarPeople") return;
  const person = props.page.items[index]?.person;
  if (person) emit("day", person.personId, person.name, date);
}
function viewShift(index: number): void {
  if (props.page.kind !== "calendarShifts") return;
  const shift = props.page.items[index]?.shift;
  if (shift) emit("shift", shift.shiftId, rowName(index));
}
function viewLocation(index: number): void {
  if (props.page.kind !== "calendarShifts") return;
  const shift = props.page.items[index]?.shift;
  if (shift?.locationId) emit("location", shift.locationId, shift.locationName ?? shift.locationId);
}
watch(
  () => props.page,
  () => {
    rowPage.value = 0;
    active.value = { row: 0, column: 0 };
    scrollElement.value?.scrollTo({ top: 0, left: 0 });
  },
);
</script>

<template>
  <section :aria-labelledby="`${id}-heading`" data-accepted-calendar>
    <h5 :id="`${id}-heading`">{{ copy.kinds[page.kind] }}</h5>
    <p>{{ copy.calendar.description }}</p>
    <p>{{ copy.timeZone }}: {{ timeZone }}</p>
    <p>
      {{
        copy.calendar.originalDates(
          page.planningDates.startDate,
          page.planningDates.endDateExclusive,
        )
      }}
    </p>
    <p>{{ copy.calendar.shownDates(page.window.startDate, page.window.endDateExclusive) }}</p>
    <div class="action-row">
      <button
        type="button"
        :disabled="loading || page.window.startDate <= page.planningDates.startDate"
        @click="emit('navigate', -7)"
      >
        {{ copy.calendar.previousWeek }}
      </button>
      <button
        type="button"
        :disabled="loading || page.window.endDateExclusive >= page.planningDates.endDateExclusive"
        @click="emit('navigate', 7)"
      >
        {{ copy.calendar.nextWeek }}
      </button>
      <button v-if="page.items.length" type="button" @click="switchMode">
        {{ mode === "virtual" ? copy.calendar.paged : copy.calendar.virtual }}
      </button>
    </div>
    <p v-if="dates.length === 0" role="alert">{{ copy.calendar.sourceError }}</p>
    <p v-else-if="page.items.length === 0" role="status">{{ copy.empty[page.kind] }}</p>
    <template v-else>
      <p :id="`${id}-help`">{{ copy.calendar.help }}</p>
      <p role="status" aria-live="polite">
        {{
          copy.calendar.dayLabel(
            rowName(active.row),
            dates[active.column] ?? "",
            cellCount(active.row, active.column),
          )
        }}
      </p>
      <div v-if="mode === 'paged'" class="action-row">
        <button type="button" :disabled="rowPage === 0" @click="navigateRows(-1)">
          {{ copy.calendar.previousRows }}
        </button>
        <button
          type="button"
          :disabled="(rowPage + 1) * rowsPerPage >= page.items.length"
          @click="navigateRows(1)"
        >
          {{ copy.calendar.nextRows }}
        </button>
      </div>
      <div
        ref="scrollElement"
        :class="mode === 'virtual' ? 'calendar-scroll' : 'calendar-page-scroll'"
        role="region"
        :aria-label="copy.calendar.region"
        tabindex="0"
      >
        <table
          class="calendar-grid"
          :class="{ virtual: mode === 'virtual' }"
          role="grid"
          :aria-rowcount="page.items.length + 1"
          :aria-colcount="dates.length + 1"
          :aria-describedby="`${id}-help`"
          :style="{ '--calendar-days': dates.length }"
        >
          <caption class="sr-only">
            {{
              copy.kinds[page.kind]
            }}
          </caption>
          <thead>
            <tr aria-rowindex="1">
              <th scope="col" class="calendar-owner">
                {{
                  page.kind === "calendarPeople"
                    ? copy.calendar.personCorner
                    : copy.calendar.shiftCorner
                }}
              </th>
              <th
                v-for="(date, index) in dates"
                :id="`${id}-date-${index}`"
                :key="date"
                scope="col"
                :aria-colindex="index + 2"
              >
                {{ dateLabel(date) }} <span class="sr-only">{{ date }}</span>
              </th>
            </tr>
          </thead>
          <tbody
            :style="
              mode === 'virtual' ? { height: `${rowVirtualizer.getTotalSize()}px` } : undefined
            "
          >
            <tr
              v-for="entry in visibleRows"
              :key="rowId(entry.index)"
              :ref="mode === 'virtual' ? measureRow : undefined"
              :data-index="entry.index"
              :aria-rowindex="entry.index + 2"
              :style="
                mode === 'virtual'
                  ? { transform: `translateY(${entry.start - headerHeight}px)` }
                  : undefined
              "
            >
              <th :id="`${id}-owner-${rowId(entry.index)}`" scope="row" class="calendar-owner">
                {{ rowName(entry.index) }}
                <span v-if="page.kind === 'calendarPeople'" class="quiet-state">
                  {{
                    copy.calendar.totalSelected(page.items[entry.index]?.person.selectedCount ?? 0)
                  }}
                </span>
                <button
                  v-if="page.kind === 'calendarShifts' && page.items[entry.index]?.shift.locationId"
                  type="button"
                  @click="viewLocation(entry.index)"
                >
                  {{
                    copy.showLocation(
                      page.items[entry.index]?.shift.locationName ??
                        page.items[entry.index]?.shift.locationId ??
                        "",
                    )
                  }}
                </button>
              </th>
              <td
                v-for="(date, column) in dates"
                :id="cellId(entry.index, column)"
                :key="date"
                role="gridcell"
                :aria-colindex="column + 2"
                :headers="`${id}-owner-${rowId(entry.index)} ${id}-date-${column}`"
                :aria-label="
                  copy.calendar.dayLabel(rowName(entry.index), date, cellCount(entry.index, column))
                "
                :tabindex="active.row === entry.index && active.column === column ? 0 : -1"
                @focus="active = { row: entry.index, column }"
                @keydown="keydown($event, entry.index, column)"
              >
                <template v-if="page.kind === 'calendarPeople'">
                  <strong>{{
                    copy.calendar.count(page.items[entry.index]?.days[column]?.selectedCount ?? 0)
                  }}</strong>
                  <ul v-if="page.items[entry.index]?.days[column]?.selectedShifts.length">
                    <li
                      v-for="sample in page.items[entry.index]?.days[column]?.selectedShifts"
                      :key="sample.assignmentId"
                    >
                      {{ localTime(sample.startsAt) }} – {{ localTime(sample.endsAt) }}
                    </li>
                  </ul>
                  <p v-if="(page.items[entry.index]?.days[column]?.selectedCount ?? 0) > 3">
                    {{
                      copy.calendar.sample(
                        3,
                        page.items[entry.index]?.days[column]?.selectedCount ?? 0,
                      )
                    }}
                  </p>
                  <button
                    type="button"
                    :aria-label="copy.calendar.viewDay(rowName(entry.index), date)"
                    @click="viewDay(entry.index, date)"
                  >
                    {{ copy.calendar.openAssignments }}
                  </button>
                </template>
                <template v-else-if="page.items[entry.index]?.date === date">
                  <strong>{{
                    copy.calendar.count(page.items[entry.index]?.shift.selectedCount ?? 0)
                  }}</strong>
                  <p>{{ shiftDescription(entry.index) }}</p>
                  <ul v-if="page.items[entry.index]?.selectedPersonIds.length">
                    <li v-for="person in page.items[entry.index]?.selectedPersonIds" :key="person">
                      {{ person }}
                    </li>
                  </ul>
                  <p v-if="(page.items[entry.index]?.shift.selectedCount ?? 0) > 3">
                    {{ copy.calendar.sample(3, page.items[entry.index]?.shift.selectedCount ?? 0) }}
                  </p>
                  <button
                    type="button"
                    :aria-label="copy.calendar.viewShift(rowName(entry.index), date)"
                    @click="viewShift(entry.index)"
                  >
                    {{ copy.calendar.openShift }}
                  </button>
                </template>
                <span v-else>{{ copy.calendar.noShift }}</span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>
  </section>
</template>

<style scoped>
section[data-accepted-calendar] {
  min-width: 0;
}
.calendar-scroll,
.calendar-page-scroll {
  overflow: auto;
  width: 100%;
  min-width: 0;
  box-sizing: border-box;
  max-width: 100%;
  border: 1px solid var(--line);
}
.calendar-scroll {
  height: 34rem;
}
.calendar-grid {
  width: max(100%, calc(13rem + var(--calendar-days) * 13rem));
  border-collapse: separate;
  border-spacing: 0;
  text-align: left;
}
.calendar-grid tr {
  display: grid;
  grid-template-columns: 13rem repeat(var(--calendar-days), 13rem);
}
.calendar-grid th,
.calendar-grid td {
  min-width: 0;
  padding: 0.5rem;
  border-right: 1px solid var(--line);
  border-bottom: 1px solid var(--line);
  overflow-wrap: anywhere;
}
.calendar-grid thead {
  position: sticky;
  top: 0;
  z-index: 2;
  background: var(--raised);
}
.calendar-grid .calendar-owner {
  position: sticky;
  left: 0;
  z-index: 1;
  background: var(--raised);
}
.calendar-grid .calendar-owner span {
  display: block;
}
.calendar-grid td:focus-visible {
  outline: 3px solid currentColor;
  outline-offset: -3px;
}
.calendar-grid.virtual tbody {
  display: block;
  position: relative;
}
.calendar-grid.virtual tbody tr {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
}
</style>
