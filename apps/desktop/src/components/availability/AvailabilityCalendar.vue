<script setup lang="ts">
import { computed, onScopeDispose, ref, shallowRef, watch } from "vue";
import { getScenarioView, SetupOperationScope } from "../../api/generated";
import type {
  WorkforceSetupAvailabilityOccurrence,
  WorkforceSetupAvailabilityPage,
  WorkforceSetupOrdinalContinuation,
} from "../../api/generated-domain-pack-contracts";
import { safeMessage, type ProjectHomeController, type ProjectSummary } from "../../project-home";
import { formatNumber } from "../../messages";
import { availabilityMessages as copy, AVAILABILITY_KIND_LABELS } from "./messages";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  personId: string;
  initialWorkWindow: { readonly startDate: string; readonly endDateExclusive: string };
  timeZone: string;
  disabled?: boolean;
  locale?: string;
}>();
const emit = defineEmits<{
  edit: [id: string];
  add: [dates: { readonly startDate: string; readonly endDateExclusive: string }];
}>();
const heading = ref<HTMLElement>();
const startDate = ref(props.initialWorkWindow.startDate);
const endDate = ref(props.initialWorkWindow.endDateExclusive);
const loading = ref(false);
const error = ref<string | null>(null);
const calendarOffset = ref(0);
const capturedPage = shallowRef<{
  readonly startDate: string;
  readonly endDateExclusive: string;
  readonly page: WorkforceSetupAvailabilityPage;
} | null>(null);
let generation = 0;
let alive = true;
let scope: SetupOperationScope | null = null;
function invalidate(): void {
  generation += 1;
  scope?.dispose();
  scope = null;
  capturedPage.value = null;
  loading.value = false;
  error.value = null;
}
async function load(
  cursor: WorkforceSetupOrdinalContinuation | null = null,
  moveFocus = false,
): Promise<void> {
  if (!alive) return;
  if (moveFocus) heading.value?.focus();
  invalidate();
  const token = generation;
  const { scenarioId, revision } = props.project;
  const epoch = props.home.state.libraryEpoch,
    personId = props.personId;
  const dates = { startDate: startDate.value, endDateExclusive: endDate.value };
  const owned = new SetupOperationScope(scenarioId, revision);
  scope = owned;
  loading.value = true;
  const current = (): boolean =>
    alive &&
    token === generation &&
    scenarioId === props.project.scenarioId &&
    revision === props.project.revision &&
    epoch === props.home.state.libraryEpoch &&
    personId === props.personId;
  try {
    const response = await getScenarioView(owned, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId: "official.workforce.setup.availability_window",
        parameters: { personId, dates, limit: 50 },
        ...(cursor === null ? {} : { continuation: cursor }),
      },
    }).result;
    if (!current()) return;
    const page = response.result.view.data.result.data;
    if (
      page.items.length > 50 ||
      new Set(page.items.map((item) => item.ordinal)).size !== page.items.length
    )
      throw new Error(copy.calendarPageIdentityError);
    capturedPage.value = { ...dates, page };
    if (cursor === null) calendarOffset.value = 0;
  } catch (failure) {
    if (current()) error.value = safeMessage(failure);
  } finally {
    owned.dispose();
    if (scope === owned) scope = null;
    if (token === generation) loading.value = false;
  }
}
interface CalendarDay {
  readonly date: string;
  readonly nextDate: string;
  readonly label: string;
  readonly occurrences: readonly WorkforceSetupAvailabilityOccurrence[];
}
const calendar = computed(() => {
  const capture = capturedPage.value;
  if (capture === null) return { days: [] as readonly CalendarDay[], more: false, error: null };
  try {
    if (
      !/^\d{4}-\d{2}-\d{2}$/.test(capture.startDate) ||
      !/^\d{4}-\d{2}-\d{2}$/.test(capture.endDateExclusive)
    )
      throw new Error(copy.calendarRangeError);
    // UTC below is Gregorian date arithmetic only. It never resolves a scenario-local instant.
    const cursor = new Date(`${capture.startDate}T00:00:00Z`);
    cursor.setUTCDate(cursor.getUTCDate() + calendarOffset.value);
    const labels = new Intl.DateTimeFormat(props.locale, {
      calendar: "iso8601",
      timeZone: "UTC",
      weekday: "long",
      year: "numeric",
      month: "short",
      day: "numeric",
    });
    const localDate = new Intl.DateTimeFormat("en-CA", {
      calendar: "iso8601",
      numberingSystem: "latn",
      timeZone: props.timeZone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    });
    const displayedDate = (instant: number): string => {
      const parts = localDate.formatToParts(instant);
      const year = parts.find((part) => part.type === "year")?.value;
      const month = parts.find((part) => part.type === "month")?.value;
      const day = parts.find((part) => part.type === "day")?.value;
      if (year === undefined || month === undefined || day === undefined)
        throw new Error(copy.calendarDateLabelsError);
      return `${year.padStart(4, "0")}-${month}-${day}`;
    };
    // Format native intervals for display, not availability evaluation. Preserve half-open
    // midnight boundaries even when the native endpoint has sub-millisecond precision.
    const spans = capture.page.items.map((occurrence) => {
      const fraction = /\.(\d+)(?:Z|[+-]\d{2}:\d{2})$/.exec(occurrence.interval.end)?.[1] ?? "";
      const endMilliseconds =
        Date.parse(occurrence.interval.end) - (/[1-9]/.test(fraction.slice(3)) ? 0 : 1);
      return {
        occurrence,
        first: displayedDate(Date.parse(occurrence.interval.start)),
        last: displayedDate(endMilliseconds),
      };
    });
    const days: CalendarDay[] = [];
    for (let count = 0; count < 31; count += 1) {
      const date = cursor.toISOString().slice(0, 10);
      if (date >= capture.endDateExclusive) break;
      const label = labels.format(cursor);
      cursor.setUTCDate(cursor.getUTCDate() + 1);
      const nextDate = cursor.toISOString().slice(0, 10);
      days.push({
        date,
        nextDate,
        label,
        occurrences: spans
          .filter((span) => span.first <= date && date <= span.last)
          .map((span) => span.occurrence),
      });
    }
    return {
      days,
      more: cursor.toISOString().slice(0, 10) < capture.endDateExclusive,
      error: null,
    };
  } catch {
    return {
      days: [] as readonly CalendarDay[],
      more: false,
      error: copy.calendarDisplayError,
    };
  }
});
function calendarPage(direction: -1 | 1): void {
  heading.value?.focus();
  calendarOffset.value = Math.max(0, calendarOffset.value + direction * 31);
}
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
    () => props.personId,
    () => props.timeZone,
  ],
  () => {
    void load();
  },
  { immediate: true, flush: "sync" },
);
onScopeDispose(() => {
  alive = false;
  invalidate();
});
</script>

<template>
  <section class="state-panel field-stack" aria-labelledby="availability-calendar-heading">
    <h3 id="availability-calendar-heading" ref="heading" tabindex="-1">
      {{ copy.calendarHeading }}
    </h3>
    <p>{{ copy.calendarDescription(timeZone) }}</p>
    <form class="field-stack" @submit.prevent="load(null, true)">
      <label for="availability-calendar-start">{{ copy.calendarRangeStart }}</label>
      <input
        id="availability-calendar-start"
        v-model="startDate"
        type="text"
        placeholder="YYYY-MM-DD"
        @input="invalidate"
      />
      <label for="availability-calendar-end">{{ copy.calendarRangeEnd }}</label>
      <input
        id="availability-calendar-end"
        v-model="endDate"
        type="text"
        placeholder="YYYY-MM-DD"
        @input="invalidate"
      />
      <p class="field-help">{{ copy.calendarRangeHelp }}</p>
      <button type="submit" :disabled="loading">{{ copy.calendarApplyRange }}</button>
    </form>
    <p v-if="loading" role="status">{{ copy.calendarLoading }}</p>
    <p v-if="error" role="alert">{{ error }}</p>
    <template v-if="capturedPage !== null">
      <p>
        {{
          copy.calendarSummary(
            capturedPage.page.totalItems,
            capturedPage.startDate,
            capturedPage.endDateExclusive,
            locale,
          )
        }}
      </p>
      <p>{{ copy.calendarEvidence }}</p>
      <p v-if="calendar.error" role="status">{{ calendar.error }}</p>
      <ol class="availability-calendar-days" :aria-label="copy.calendarDaysLabel">
        <li v-for="day in calendar.days" :key="day.date" class="state-panel field-stack">
          <h4>
            <time :datetime="day.date">{{ day.label }}</time>
          </h4>
          <code>{{ day.date }}</code>
          <ul>
            <li v-for="occurrence in day.occurrences.slice(0, 3)" :key="occurrence.ordinal">
              {{
                copy.calendarOccurrence(
                  AVAILABILITY_KIND_LABELS[occurrence.availabilityKind],
                  occurrence.ordinal + 1,
                  locale,
                )
              }}
            </li>
          </ul>
          <p v-if="day.occurrences.length > 3">
            {{ copy.calendarMoreOccurrences(day.occurrences.length - 3, locale) }}
          </p>
          <button
            type="button"
            :disabled="disabled"
            :aria-label="copy.calendarAddAvailabilityForDate(day.date)"
            @click="emit('add', { startDate: day.date, endDateExclusive: day.nextDate })"
          >
            {{ copy.calendarAddForDate }}
          </button>
        </li>
      </ol>
      <nav class="action-row" :aria-label="copy.calendarDatePages">
        <button type="button" :disabled="calendarOffset === 0" @click="calendarPage(-1)">
          {{ copy.calendarPreviousDates }}
        </button>
        <button type="button" :disabled="!calendar.more" @click="calendarPage(1)">
          {{ copy.calendarNextDates }}
        </button>
      </nav>
      <div
        class="overflow-x-auto"
        role="region"
        :aria-label="copy.calendarIntervalList"
        tabindex="0"
      >
        <table class="w-full border-collapse text-left text-sm">
          <caption>
            {{
              copy.calendarIntervalCaption
            }}
          </caption>
          <thead>
            <tr>
              <th scope="col">{{ copy.calendarOccurrenceColumn }}</th>
              <th scope="col">{{ copy.calendarKindColumn }}</th>
              <th scope="col">{{ copy.calendarStartColumn }}</th>
              <th scope="col">{{ copy.calendarEndColumn }}</th>
              <th scope="col">{{ copy.calendarRecordColumn }}</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="occurrence in capturedPage.page.items"
              :key="occurrence.ordinal"
              class="border-t border-line align-top"
            >
              <th scope="row" class="p-2">{{ formatNumber(occurrence.ordinal + 1, locale) }}</th>
              <td class="p-2">
                {{ AVAILABILITY_KIND_LABELS[occurrence.availabilityKind] }}
                <p v-if="occurrence.availabilityKind === 'requestedTimeOff'">
                  {{ copy.importedKindHelp }}
                </p>
              </td>
              <td class="p-2">
                <code>{{ occurrence.interval.start }}</code>
              </td>
              <td class="p-2">
                <code>{{ occurrence.interval.end }}</code>
              </td>
              <td class="p-2">
                <code class="break-all">{{ occurrence.availabilityId }}</code
                ><button
                  type="button"
                  :disabled="disabled"
                  :aria-label="copy.editAvailability(occurrence.availabilityId)"
                  @click="emit('edit', occurrence.availabilityId)"
                >
                  {{ copy.editRecord }}
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <p v-if="capturedPage.page.totalItems === 0">{{ copy.calendarEmpty }}</p>
      <nav class="action-row" :aria-label="copy.calendarOccurrencePages">
        <button type="button" @click="load(null, true)">
          {{ copy.calendarFirstOccurrencePage }}
        </button>
        <button
          type="button"
          :disabled="capturedPage.page.continuation === null"
          @click="load(capturedPage.page.continuation, true)"
        >
          {{ copy.calendarNextOccurrencePage }}
        </button>
      </nav>
    </template>
  </section>
</template>

<style scoped>
.availability-calendar-days {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 11rem), 1fr));
  gap: var(--s3);
  padding: 0;
  list-style: none;
}
</style>
