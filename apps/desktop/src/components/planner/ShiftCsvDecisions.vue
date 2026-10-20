<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { newUuidV7, type ShiftCsvPreview, type ShiftCsvRowDecision } from "../../api/generated";
import type { ProjectSummary } from "../../project-home";
import { messages } from "../../messages";
import { formatWorkOffset } from "../../work-display";
import WorkforceEntityPicker from "./WorkforceEntityPicker.vue";

const props = defineProps<{
  project: ProjectSummary;
  libraryEpoch: number;
  rows: ShiftCsvPreview["rows"];
  modelValue: readonly ShiftCsvRowDecision[];
  current: boolean;
  disabled: boolean;
}>();
const emit = defineEmits<{
  "update:modelValue": [value: readonly ShiftCsvRowDecision[]];
  inspect: [record: number];
}>();
const copy = messages.shiftCsv;
const page = ref(0);
const heading = ref<HTMLElement>();
const announcement = ref("");
const decisions = computed(
  () => new Map(props.modelValue.map((item) => [item.record, item.decision])),
);
const sides = ["before", "after"] as const;
const times = ["startsAt", "endsAt"] as const;
function publish(record: number, decision: ShiftCsvRowDecision["decision"]): void {
  if (props.disabled) return;
  emit(
    "update:modelValue",
    [...props.modelValue.filter((item) => item.record !== record), { record, decision }].sort(
      (a, b) => a.record - b.record,
    ),
  );
  announcement.value = `${copy.decision}: ${String(record)} · ${decision.kind}`;
}
function canAdd(row: ShiftCsvPreview["rows"][number]): boolean {
  return (
    row.identity.kind === "absent" ||
    (row.identity.kind === "supplied" && row.status === "unresolved")
  );
}
function add(row: ShiftCsvPreview["rows"][number]): void {
  if (props.disabled || !canAdd(row)) return;
  const previous = decisions.value.get(row.record);
  const shiftId =
    row.identity.kind === "supplied"
      ? row.identity.shiftId
      : previous?.kind === "add"
        ? previous.shiftId
        : newUuidV7();
  publish(row.record, { kind: "add", shiftId });
}
function update(row: ShiftCsvPreview["rows"][number], ids: readonly string[]): void {
  if (props.disabled || row.identity.kind !== "absent") return;
  if (ids[0]) publish(row.record, { kind: "update", shiftId: ids[0] });
  else if (decisions.value.get(row.record)?.kind === "update") reset(row.record);
}
function selection(record: number): readonly string[] {
  const decision = decisions.value.get(record);
  return decision?.kind === "update" ? [decision.shiftId] : [];
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
  document.getElementById(`shift-csv-update-target-${String(record)}`)?.focus();
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
  <section class="field-stack" aria-labelledby="shift-csv-records-heading">
    <h3 id="shift-csv-records-heading" ref="heading" tabindex="-1">{{ copy.decisions }}</h3>
    <p>{{ current ? copy.currentFindings : copy.previousFindings }} {{ copy.duplicatePolicy }}</p>
    <p>{{ copy.identityHelp }}</p>
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
            <th scope="col">{{ copy.decision }}</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="row in rows.slice(page * 50, (page + 1) * 50)"
            :key="row.record"
            :data-shift-csv-record="row.record"
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
                row.identity.shiftId
              }}</code>
            </td>
            <td>
              <code class="break-all">{{ row.shiftId ?? copy.unresolved }}</code>
            </td>
            <td v-for="side in sides" :key="side" :data-shift-csv-side="side">
              <dl v-if="row[side]" class="field-stack">
                <div>
                  <dt>{{ copy.target }}</dt>
                  <dd>
                    <code class="break-all">{{ row[side].id }}</code>
                  </dd>
                </div>
                <div>
                  <dt>{{ copy.fields.assignmentTypeId }}</dt>
                  <dd>
                    <code class="break-all">{{ row[side].assignmentTypeId }}</code>
                  </dd>
                </div>
                <div>
                  <dt>{{ copy.fields.locationId }}</dt>
                  <dd>
                    <code class="break-all">{{ row[side].locationId ?? copy.none }}</code>
                  </dd>
                </div>
                <div v-for="time in times" :key="time" :data-shift-csv-time="time">
                  <dt>{{ copy.fields[time] }} · {{ copy.local }}</dt>
                  <dd>
                    <code>{{ row[side][time].local }}</code>
                  </dd>
                  <dt>{{ copy.instant }}</dt>
                  <dd>
                    <code>{{ row[side][time].instant }}</code>
                  </dd>
                  <dt>{{ copy.offset }}</dt>
                  <dd>
                    {{ formatWorkOffset(row[side][time].offsetSeconds) }} ({{
                      row[side][time].offsetSeconds
                    }}
                    s)
                  </dd>
                </div>
                <div>
                  <dt>{{ copy.fields.coverage }}</dt>
                  <dd>
                    <pre class="whitespace-pre-wrap break-all">{{
                      JSON.stringify(row[side].coverage, null, 2)
                    }}</pre>
                  </dd>
                </div>
                <div>
                  <dt>{{ copy.fields.tags }}</dt>
                  <dd>
                    <pre class="whitespace-pre-wrap break-all">{{
                      JSON.stringify(row[side].tags, null, 2)
                    }}</pre>
                  </dd>
                </div>
                <div>
                  <dt>{{ copy.fields.reportingAttribution }}</dt>
                  <dd>
                    <pre class="whitespace-pre-wrap break-all">{{
                      JSON.stringify(row[side].reportingAttribution, null, 2)
                    }}</pre>
                  </dd>
                </div>
                <div>
                  <dt>{{ copy.origin }}</dt>
                  <dd>
                    <pre class="whitespace-pre-wrap break-all">{{
                      JSON.stringify(row[side].origin, null, 2)
                    }}</pre>
                  </dd>
                </div>
              </dl>
              <p v-else>
                {{
                  side === "after" && row.status === "unchanged" ? copy.unchanged : copy.noRecord
                }}
              </p>
            </td>
            <td>
              <fieldset :disabled="disabled" class="field-stack">
                <legend>{{ copy.decision }} · {{ row.record }}</legend>
                <p v-if="decisions.has(row.record)">
                  <code class="break-all">{{ JSON.stringify(decisions.get(row.record)) }}</code>
                </p>
                <button
                  v-if="canAdd(row)"
                  :id="`shift-csv-add-${row.record}`"
                  type="button"
                  @click="add(row)"
                >
                  {{ copy.addRecord(String(row.record)) }}
                </button>
                <template v-if="row.identity.kind === 'absent'">
                  <button
                    :id="`shift-csv-update-${row.record}`"
                    type="button"
                    @click="focusPicker(row.record)"
                  >
                    {{ copy.updateRecord(String(row.record)) }}
                  </button>
                  <WorkforceEntityPicker
                    :id="`shift-csv-update-target-${row.record}`"
                    kind="shiftInstance"
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
                  :id="`shift-csv-skip-${row.record}`"
                  type="button"
                  @click="publish(row.record, { kind: 'skip' })"
                >
                  {{ copy.skipRecord(String(row.record)) }}
                </button>
                <button
                  :id="`shift-csv-reset-${row.record}`"
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
