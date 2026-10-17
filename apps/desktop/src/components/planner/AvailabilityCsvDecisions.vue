<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import {
  newUuidV7,
  type AvailabilityCsvPreview,
  type AvailabilityCsvRowDecision,
} from "../../api/generated";
import type { ProjectSummary } from "../../project-home";
import { messages } from "../../messages";
import { formatWorkOffset } from "../../work-display";
import WorkforceEntityPicker from "./WorkforceEntityPicker.vue";

const props = defineProps<{
  project: ProjectSummary;
  libraryEpoch: number;
  rows: AvailabilityCsvPreview["rows"];
  modelValue: readonly AvailabilityCsvRowDecision[];
  current: boolean;
  disabled: boolean;
}>();
const emit = defineEmits<{
  "update:modelValue": [value: readonly AvailabilityCsvRowDecision[]];
  inspect: [record: number];
}>();
const copy = messages.availabilityCsv;
const page = ref(0);
const heading = ref<HTMLElement>();
const announcement = ref("");
const decisions = computed(
  () => new Map(props.modelValue.map((item) => [item.record, item.decision])),
);
const sides = ["before", "after"] as const;
function publish(record: number, decision: AvailabilityCsvRowDecision["decision"]): void {
  if (props.disabled) return;
  emit(
    "update:modelValue",
    [...props.modelValue.filter((item) => item.record !== record), { record, decision }].sort(
      (a, b) => a.record - b.record,
    ),
  );
  announcement.value = `${copy.decision}: ${String(record)} · ${decision.kind}`;
}
function canAdd(row: AvailabilityCsvPreview["rows"][number]): boolean {
  return (
    row.identity.kind === "absent" ||
    (row.identity.kind === "supplied" && row.status === "unresolved")
  );
}
function add(row: AvailabilityCsvPreview["rows"][number]): void {
  if (props.disabled || !canAdd(row)) return;
  const previous = decisions.value.get(row.record);
  const availabilityId =
    row.identity.kind === "supplied"
      ? row.identity.availabilityId
      : previous?.kind === "add"
        ? previous.availabilityId
        : newUuidV7();
  publish(row.record, { kind: "add", availabilityId });
}
function update(row: AvailabilityCsvPreview["rows"][number], ids: readonly string[]): void {
  if (props.disabled || row.identity.kind !== "absent") return;
  if (ids[0]) publish(row.record, { kind: "update", availabilityId: ids[0] });
  else if (decisions.value.get(row.record)?.kind === "update") reset(row.record);
}
function selection(record: number): readonly string[] {
  const decision = decisions.value.get(record);
  return decision?.kind === "update" ? [decision.availabilityId] : [];
}
function reset(record: number): void {
  if (props.disabled) return;
  emit(
    "update:modelValue",
    props.modelValue.filter((item) => item.record !== record),
  );
  announcement.value = copy.resetRecord(String(record));
}
async function focusPicker(record: number): Promise<void> {
  await nextTick();
  document.getElementById(`availability-csv-update-target-${String(record)}`)?.focus();
}
async function move(direction: number): Promise<void> {
  page.value += direction;
  await nextTick();
  heading.value?.focus();
}
watch(
  () => props.rows.length,
  () => {
    page.value = Math.min(page.value, Math.max(0, Math.ceil(props.rows.length / 50) - 1));
  },
);
</script>

<template>
  <section class="field-stack" aria-labelledby="availability-csv-records-heading">
    <h3 id="availability-csv-records-heading" ref="heading" tabindex="-1">{{ copy.decisions }}</h3>
    <p>{{ current ? copy.currentFindings : copy.previousFindings }} {{ copy.duplicatePolicy }}</p>
    <p>{{ copy.identityHelp }}</p>
    <p>{{ copy.projectionHelp }}</p>
    <p>{{ copy.weeklyHelp }}</p>
    <p role="status" aria-live="polite" aria-atomic="true">{{ announcement }}</p>
    <div
      class="overflow-x-auto focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
      role="region"
      :aria-label="copy.records"
      tabindex="0"
    >
      <table class="w-full text-left">
        <caption>
          {{
            copy.records
          }}
        </caption>
        <thead>
          <tr>
            <th scope="col">{{ messages.csvImport.record }}</th>
            <th scope="col">{{ copy.status }}</th>
            <th scope="col">{{ copy.identity }}</th>
            <th scope="col">{{ copy.target }}</th>
            <th scope="col">{{ copy.before }}</th>
            <th scope="col">{{ copy.after }}</th>
            <th scope="col">{{ copy.temporalFacts }}</th>
            <th scope="col">{{ copy.decision }}</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="row in rows.slice(page * 50, (page + 1) * 50)"
            :key="row.record"
            :data-availability-csv-record="row.record"
          >
            <th scope="row">{{ row.record }}</th>
            <td>
              {{ row.status }}<code v-if="row.issue"> · {{ row.issue }}</code>
            </td>
            <td>
              {{
                row.identity.kind === "absent"
                  ? copy.absent
                  : row.identity.kind === "invalid"
                    ? copy.invalid
                    : copy.supplied
              }}
              <code v-if="row.identity.kind === 'supplied'" class="break-all">{{
                row.identity.availabilityId
              }}</code>
            </td>
            <td>
              <code class="break-all">{{ row.availabilityId ?? copy.unresolved }}</code>
            </td>
            <td v-for="side in sides" :key="side" :data-availability-csv-side="side">
              <pre v-if="row[side]" class="whitespace-pre-wrap break-all">{{
                JSON.stringify(row[side], null, 2)
              }}</pre>
              <p v-else>
                {{
                  side === "after" && row.status === "unchanged" ? copy.unchanged : copy.noRecord
                }}
              </p>
            </td>
            <td>
              <dl class="field-stack">
                <template
                  v-for="(resolved, time) in {
                    startsAt: row.resolvedStartsAt,
                    endsAt: row.resolvedEndsAt,
                  }"
                  :key="time"
                >
                  <div v-if="resolved" :data-availability-csv-time="time">
                    <dt>{{ copy.fields[time] }} · {{ copy.local }}</dt>
                    <dd>
                      <code>{{ resolved.local }}</code>
                    </dd>
                    <dt>{{ copy.instant }}</dt>
                    <dd>
                      <code>{{ resolved.instant }}</code>
                    </dd>
                    <dt>{{ copy.offset }}</dt>
                    <dd>
                      {{ formatWorkOffset(resolved.offsetSeconds) }} ({{ resolved.offsetSeconds }}
                      s)
                    </dd>
                  </div>
                </template>
              </dl>
              <p v-if="!row.resolvedStartsAt && !row.resolvedEndsAt">{{ copy.noTemporalFacts }}</p>
            </td>
            <td>
              <fieldset :disabled="disabled" class="field-stack">
                <legend>{{ copy.decision }} · {{ row.record }}</legend>
                <p v-if="decisions.has(row.record)">
                  <code class="break-all">{{ JSON.stringify(decisions.get(row.record)) }}</code>
                </p>
                <button
                  v-if="canAdd(row)"
                  :id="`availability-csv-add-${row.record}`"
                  type="button"
                  @click="add(row)"
                >
                  {{ copy.addRecord(String(row.record)) }}
                </button>
                <template v-if="row.identity.kind === 'absent'">
                  <button
                    :id="`availability-csv-update-${row.record}`"
                    type="button"
                    @click="focusPicker(row.record)"
                  >
                    {{ copy.updateRecord(String(row.record)) }}
                  </button>
                  <WorkforceEntityPicker
                    :id="`availability-csv-update-target-${row.record}`"
                    kind="availability"
                    :label="copy.updateTarget(String(row.record))"
                    :search-disabled="true"
                    :project="project"
                    :library-epoch="libraryEpoch"
                    :model-value="selection(row.record)"
                    :disabled="disabled"
                    @update:model-value="update(row, $event)"
                  />
                </template>
                <button
                  :id="`availability-csv-skip-${row.record}`"
                  type="button"
                  @click="publish(row.record, { kind: 'skip' })"
                >
                  {{ copy.skipRecord(String(row.record)) }}
                </button>
                <button
                  :id="`availability-csv-reset-${row.record}`"
                  type="button"
                  :disabled="!decisions.has(row.record)"
                  @click="reset(row.record)"
                >
                  {{ copy.resetRecord(String(row.record)) }}
                </button>
                <button type="button" @click="emit('inspect', row.record)">
                  {{ copy.inspectRow(String(row.record)) }}
                </button>
              </fieldset>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <nav v-if="rows.length > 50" :aria-label="copy.recordPages">
      <button type="button" :disabled="page === 0" @click="move(-1)">
        {{ copy.previousRecords }}
      </button>
      <button type="button" :disabled="(page + 1) * 50 >= rows.length" @click="move(1)">
        {{ copy.nextRecords }}
      </button>
    </nav>
  </section>
</template>
