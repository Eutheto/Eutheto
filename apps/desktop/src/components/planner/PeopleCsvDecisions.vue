<script setup lang="ts">
import { computed, nextTick, ref, useId, watch } from "vue";
import { newUuidV7, type PeopleCsvPreview } from "../../api/generated";
import type { ProjectSummary } from "../../project-home";
import WorkforceEntityPicker from "./WorkforceEntityPicker.vue";
import { plannerMessage, type PlannerMessageKey } from "./messages";
import {
  csvDecisionRecord,
  type CsvDecisionDraft,
  type CsvRecordSampleState,
} from "./people-csv-decisions";

const props = defineProps<{
  readonly modelValue: readonly CsvDecisionDraft[];
  readonly rows: PeopleCsvPreview["rows"];
  readonly sample: CsvRecordSampleState;
  readonly project: ProjectSummary;
  readonly libraryEpoch: number;
  readonly locale?: string;
  readonly disabled?: boolean;
  readonly errors?: readonly { record: number; message: string }[];
}>();
const emit = defineEmits<{
  "update:modelValue": [value: readonly CsvDecisionDraft[]];
  inspect: [record: number];
}>();
// The parent resets the component key and decisions on source/interpretation changes.
// Samples must already be native-bounded and correlated to source, dialect, scope and request.
const prefix = useId();
const pageSize = 50;
const kinds = ["add", "update", "skip"] as const;
const recordInput = ref<HTMLInputElement>();
const rawRecord = ref("");
const recordError = ref("");
const announcement = ref("");
const decisionPage = ref(0);
const reviewPage = ref(0);
const errorIndex = ref(0);
const decisionPages = computed(() => Math.max(1, Math.ceil(props.modelValue.length / pageSize)));
const reviewPages = computed(() => Math.max(1, Math.ceil(props.rows.length / pageSize)));
const decisions = computed(() =>
  props.modelValue.slice(decisionPage.value * pageSize, (decisionPage.value + 1) * pageSize),
);
const reviewRows = computed(() =>
  props.rows.slice(reviewPage.value * pageSize, (reviewPage.value + 1) * pageSize),
);
const decisionsByRecord = computed(
  () => new Map(props.modelValue.map((draft) => [draft.record, draft])),
);
const errorsByRecord = computed(() => {
  const messages = new Map<number, string[]>();
  for (const error of props.errors ?? []) {
    const existing = messages.get(error.record);
    if (existing) existing.push(error.message);
    else messages.set(error.record, [error.message]);
  }
  return new Map([...messages].map(([record, values]) => [record, values.join(" ")]));
});
const selectedError = computed(() => props.errors?.[errorIndex.value]);
const sampleStatus = computed(() => {
  const sample = props.sample;
  switch (sample.status) {
    case "idle":
      return message("csvDecision.sampleIdle");
    case "loading":
      return message("csvDecision.sampleLoading", { record: sample.record });
    case "error":
      return message("csvDecision.sampleError", { record: sample.record });
    case "ready":
      return message(
        sample.sample.cells === null ? "csvDecision.sampleMissing" : "csvDecision.sampleReady",
        {
          record: sample.sample.record,
        },
      );
  }
});
const pickerContext = computed(() => ({
  project: props.project,
  libraryEpoch: props.libraryEpoch,
  disabled: props.disabled,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
watch([decisionPages, reviewPages, () => props.errors?.length ?? 0], () => {
  decisionPage.value = Math.min(decisionPage.value, decisionPages.value - 1);
  reviewPage.value = Math.min(reviewPage.value, reviewPages.value - 1);
  errorIndex.value = Math.min(errorIndex.value, Math.max(0, (props.errors?.length ?? 0) - 1));
});

function message(
  key: PlannerMessageKey,
  parameters: Readonly<Record<string, string | number>> = {},
): string {
  return plannerMessage(key, parameters, props.locale);
}
async function focus(id: string): Promise<void> {
  await nextTick();
  document.getElementById(id)?.focus();
}
async function focusRecord(record: number): Promise<void> {
  const decisionIndex = props.modelValue.findIndex((draft) => draft.record === record);
  const rowIndex = props.rows.findIndex((row) => row.record === record);
  if (decisionIndex !== -1) {
    decisionPage.value = Math.floor(decisionIndex / pageSize);
    await focus(`${prefix}-decision-${String(record)}`);
  } else if (rowIndex !== -1) {
    reviewPage.value = Math.floor(rowIndex / pageSize);
    await focus(`${prefix}-review-${String(record)}`);
  } else {
    rawRecord.value = String(record);
    recordError.value = "";
    await nextTick();
    if (props.disabled) await focus(`${prefix}-manual`);
    else recordInput.value?.focus();
  }
  announcement.value = message("csvDecision.focused", { record });
}
defineExpose({ focusRecord });

function admitRecord(): number | null {
  const record = csvDecisionRecord(rawRecord.value);
  recordError.value = record === null ? message("csvDecision.recordError") : "";
  if (record === null) {
    announcement.value = recordError.value;
    recordInput.value?.focus();
  }
  return record;
}
async function manual(
  action: CsvDecisionDraft["decision"]["kind"] | "inspect" | "go",
): Promise<void> {
  if (props.disabled) return;
  const record = admitRecord();
  if (record === null) return;
  if (action === "inspect") await inspect(record);
  else if (action === "go") await focusRecord(record);
  else await choose(record, action);
}
async function choose(record: number, kind: CsvDecisionDraft["decision"]["kind"]): Promise<void> {
  if (props.disabled) return;
  const previous = decisionsByRecord.value.get(record);
  if (!previous && props.modelValue.length >= 10_000) {
    recordError.value = message("csvDecision.limit");
    announcement.value = recordError.value;
    await focus(`${prefix}-record`);
    return;
  }
  // A repeated Add or Update retains its explicit identity; native row IDs are never fallbacks.
  if (previous?.decision.kind !== kind) {
    const decision: CsvDecisionDraft["decision"] =
      kind === "add"
        ? { kind, personId: newUuidV7() }
        : kind === "update"
          ? { kind, personId: null }
          : { kind };
    const draft = { record, decision };
    emit(
      "update:modelValue",
      previous
        ? props.modelValue.map((value) => (value.record === record ? draft : value))
        : [...props.modelValue, draft],
    );
  }
  await nextTick();
  await focusRecord(record);
  if (kind === "update") await focus(`${prefix}-person-${String(record)}`);
  announcement.value = message("csvDecision.changed", {
    record,
    choice: message(`csvDecision.${kind}`),
  });
}
function selectPerson(record: number, ids: readonly string[]): void {
  if (props.disabled || decisionsByRecord.value.get(record)?.decision.kind !== "update") return;
  emit(
    "update:modelValue",
    props.modelValue.map((draft) =>
      draft.record === record
        ? { record, decision: { kind: "update", personId: ids[0] ?? null } }
        : draft,
    ),
  );
  announcement.value = message("csvDecision.identityChanged", { record });
}
function rotate(record: number): void {
  if (props.disabled || decisionsByRecord.value.get(record)?.decision.kind !== "add") return;
  const personId = newUuidV7();
  emit(
    "update:modelValue",
    props.modelValue.map((draft) =>
      draft.record === record ? { record, decision: { kind: "add", personId } } : draft,
    ),
  );
  announcement.value = message("csvDecision.rotated", { record });
}
async function remove(record: number): Promise<void> {
  if (props.disabled) return;
  const index = props.modelValue.findIndex((draft) => draft.record === record);
  if (index === -1) return;
  const remaining = props.modelValue.filter((draft) => draft.record !== record);
  const next = remaining[Math.min(index, remaining.length - 1)];
  emit("update:modelValue", remaining);
  await nextTick();
  if (next) await focusRecord(next.record);
  else await focusRecord(record);
  announcement.value = message("csvDecision.removed", { record });
}
async function inspect(record: number): Promise<void> {
  if (props.disabled) return;
  emit("inspect", record);
  await focus(`${prefix}-sample`);
}
async function changePage(section: "decisions" | "review", direction: number): Promise<void> {
  if (props.disabled) return;
  const page = section === "decisions" ? decisionPage : reviewPage;
  const pages = section === "decisions" ? decisionPages.value : reviewPages.value;
  page.value = Math.max(0, Math.min(pages - 1, page.value + direction));
  await focus(`${prefix}-${section}-heading`);
  announcement.value = message(
    section === "decisions" ? "csvDecision.decisionsPage" : "csvDecision.reviewPage",
    {
      page: page.value + 1,
      pages,
    },
  );
}
async function changeError(direction: number): Promise<void> {
  if (props.disabled) return;
  errorIndex.value = Math.max(
    0,
    Math.min((props.errors?.length ?? 1) - 1, errorIndex.value + direction),
  );
  await focus(`${prefix}-error`);
}
</script>

<template>
  <fieldset :disabled="disabled" class="field-stack">
    <legend>{{ message("csvDecision.title") }}</legend>
    <p class="field-help">{{ message("csvDecision.help") }}</p>
    <p role="status" aria-live="polite" aria-atomic="true">{{ announcement }}</p>

    <section :aria-labelledby="`${prefix}-manual`" class="field-stack">
      <h3 :id="`${prefix}-manual`" tabindex="-1">{{ message("csvDecision.recordInput") }}</h3>
      <label :for="`${prefix}-record`">{{ message("csvDecision.recordInput") }}</label>
      <input
        :id="`${prefix}-record`"
        ref="recordInput"
        type="text"
        inputmode="numeric"
        :value="rawRecord"
        :aria-invalid="recordError ? true : undefined"
        :aria-describedby="`${prefix}-record-help ${prefix}-record-error`"
        @input="
          rawRecord = ($event.target as HTMLInputElement).value;
          recordError = '';
        "
        @keydown.enter.prevent="manual('go')"
      />
      <p :id="`${prefix}-record-help`" class="field-help">
        {{ message("csvDecision.recordHelp") }}
      </p>
      <p :id="`${prefix}-record-error`" class="field-error">{{ recordError }}</p>
      <div class="flex flex-wrap gap-2">
        <button v-for="kind in kinds" :key="kind" type="button" @click="manual(kind)">
          {{ message(`csvDecision.${kind}`) }}
        </button>
        <button type="button" @click="manual('inspect')">
          {{ message("csvDecision.inspect") }}
        </button>
        <button type="button" @click="manual('go')">{{ message("csvDecision.go") }}</button>
      </div>
    </section>

    <section v-if="selectedError" :aria-labelledby="`${prefix}-errors-heading`" class="field-stack">
      <h3 :id="`${prefix}-errors-heading`">{{ message("csvDecision.errors") }}</h3>
      <div :id="`${prefix}-error`" tabindex="-1">
        <p>
          {{
            message("csvDecision.errorCount", {
              position: errorIndex + 1,
              total: errors?.length ?? 0,
            })
          }}
        </p>
        <p>{{ message("csvDecision.logicalRecord", { record: selectedError.record }) }}</p>
        <p class="field-error">{{ selectedError.message }}</p>
      </div>
      <div class="flex flex-wrap gap-2">
        <button type="button" :disabled="errorIndex === 0" @click="changeError(-1)">
          {{ message("csvDecision.previousError") }}
        </button>
        <button
          type="button"
          :disabled="errorIndex + 1 >= (errors?.length ?? 0)"
          @click="changeError(1)"
        >
          {{ message("csvDecision.nextError") }}
        </button>
        <button type="button" @click="focusRecord(selectedError.record)">
          {{ message("csvDecision.go") }}
        </button>
      </div>
    </section>

    <section :aria-labelledby="`${prefix}-decisions-heading`" class="field-stack">
      <h3 :id="`${prefix}-decisions-heading`" tabindex="-1">{{ message("csvDecision.title") }}</h3>
      <p v-if="modelValue.length === 0">{{ message("csvDecision.empty") }}</p>
      <p v-else>
        {{
          message("csvDecision.page", {
            start: decisionPage * pageSize + 1,
            end: Math.min((decisionPage + 1) * pageSize, modelValue.length),
            total: modelValue.length,
          })
        }}
      </p>
      <article
        v-for="draft in decisions"
        :id="`${prefix}-decision-${draft.record}`"
        :key="draft.record"
        tabindex="-1"
        class="field-stack min-w-0 rounded border p-3"
        :aria-labelledby="`${prefix}-decision-title-${draft.record}`"
        :aria-describedby="
          errorsByRecord.has(draft.record) ? `${prefix}-decision-error-${draft.record}` : undefined
        "
      >
        <h4 :id="`${prefix}-decision-title-${draft.record}`">
          {{ message("csvDecision.logicalRecord", { record: draft.record }) }}
        </h4>
        <p>
          {{
            message("csvDecision.choice", { choice: message(`csvDecision.${draft.decision.kind}`) })
          }}
        </p>
        <div class="flex flex-wrap gap-2">
          <button
            v-for="kind in kinds"
            :key="kind"
            type="button"
            :aria-pressed="draft.decision.kind === kind"
            @click="choose(draft.record, kind)"
          >
            {{ message(`csvDecision.${kind}`) }}
          </button>
          <button type="button" @click="inspect(draft.record)">
            {{ message("csvDecision.inspect") }}
          </button>
          <button type="button" @click="remove(draft.record)">
            {{ message("csvDecision.remove") }}
          </button>
        </div>
        <template v-if="draft.decision.kind === 'add'">
          <p class="break-all">
            {{ message("csvDecision.personId", { id: draft.decision.personId }) }}
          </p>
          <p class="field-help">{{ message("csvDecision.rotateHelp") }}</p>
          <button type="button" @click="rotate(draft.record)">
            {{ message("csvDecision.rotate") }}
          </button>
        </template>
        <template v-else-if="draft.decision.kind === 'update'">
          <WorkforceEntityPicker
            v-bind="pickerContext"
            :id="`${prefix}-person-${draft.record}`"
            kind="person"
            :label="message('csvDecision.existingPerson', { record: draft.record })"
            :description="message('csvDecision.updateHelp')"
            :model-value="draft.decision.personId === null ? [] : [draft.decision.personId]"
            :error="
              draft.decision.personId === null || draft.decision.personId.trim() === ''
                ? message('csvDecision.updateRequired')
                : undefined
            "
            required
            @update:model-value="selectPerson(draft.record, $event)"
          />
          <p v-if="draft.decision.personId !== null" class="break-all">
            {{ message("csvDecision.personId", { id: draft.decision.personId }) }}
          </p>
        </template>
        <p
          v-if="errorsByRecord.has(draft.record)"
          :id="`${prefix}-decision-error-${draft.record}`"
          class="field-error"
        >
          {{ errorsByRecord.get(draft.record) }}
        </p>
      </article>
      <nav :aria-label="message('csvDecision.title')" class="flex flex-wrap gap-2">
        <button type="button" :disabled="decisionPage === 0" @click="changePage('decisions', -1)">
          {{ message("csvDecision.previous") }}
        </button>
        <button
          type="button"
          :disabled="decisionPage + 1 >= decisionPages"
          @click="changePage('decisions', 1)"
        >
          {{ message("csvDecision.next") }}
        </button>
      </nav>
    </section>

    <section :aria-labelledby="`${prefix}-review-heading`" class="field-stack">
      <h3 :id="`${prefix}-review-heading`" tabindex="-1">
        {{ message("csvDecision.reviewTitle") }}
      </h3>
      <p class="field-help">{{ message("csvDecision.reviewHelp") }}</p>
      <p v-if="rows.length === 0">{{ message("csvDecision.reviewEmpty") }}</p>
      <p v-else>
        {{
          message("csvDecision.page", {
            start: reviewPage * pageSize + 1,
            end: Math.min((reviewPage + 1) * pageSize, rows.length),
            total: rows.length,
          })
        }}
      </p>
      <article
        v-for="row in reviewRows"
        :id="`${prefix}-review-${row.record}`"
        :key="row.record"
        tabindex="-1"
        class="field-stack min-w-0 rounded border p-3"
        :aria-labelledby="`${prefix}-review-title-${row.record}`"
      >
        <h4 :id="`${prefix}-review-title-${row.record}`">
          {{ message("csvDecision.logicalRecord", { record: row.record }) }}
        </h4>
        <p>{{ message("csvDecision.nativeStatus", { status: row.status }) }}</p>
        <p v-if="row.personId !== null" class="break-all">
          {{ message("csvDecision.personId", { id: row.personId }) }}
        </p>
        <template v-if="row.rejection !== null">
          <p>{{ message("csvDecision.rejection", { code: row.rejection }) }}</p>
          <p>{{ message(`csvDecision.rejection.${row.rejection}`) }}</p>
        </template>
        <p v-if="!decisionsByRecord.has(row.record)" class="field-help">
          {{ message("csvDecision.noDecision") }}
        </p>
        <div class="flex flex-wrap gap-2">
          <button
            v-for="kind in kinds"
            :key="kind"
            type="button"
            :aria-pressed="decisionsByRecord.get(row.record)?.decision.kind === kind"
            @click="choose(row.record, kind)"
          >
            {{ message(`csvDecision.${kind}`) }}
          </button>
          <button type="button" @click="inspect(row.record)">
            {{ message("csvDecision.inspect") }}
          </button>
        </div>
        <p v-if="errorsByRecord.has(row.record)" class="field-error">
          {{ errorsByRecord.get(row.record) }}
        </p>
      </article>
      <nav :aria-label="message('csvDecision.reviewTitle')" class="flex flex-wrap gap-2">
        <button type="button" :disabled="reviewPage === 0" @click="changePage('review', -1)">
          {{ message("csvDecision.previous") }}
        </button>
        <button
          type="button"
          :disabled="reviewPage + 1 >= reviewPages"
          @click="changePage('review', 1)"
        >
          {{ message("csvDecision.next") }}
        </button>
      </nav>
    </section>

    <section
      :aria-labelledby="`${prefix}-sample`"
      class="field-stack"
      :aria-busy="sample.status === 'loading'"
    >
      <h3 :id="`${prefix}-sample`" tabindex="-1">{{ message("csvDecision.sampleTitle") }}</h3>
      <p class="field-help">{{ message("csvDecision.sampleHelp") }}</p>
      <p role="status" aria-live="polite" aria-atomic="true">{{ sampleStatus }}</p>
      <template v-if="sample.status === 'error'">
        <p class="field-error">{{ sample.message }}</p>
        <button type="button" @click="inspect(sample.record)">{{ message("action.retry") }}</button>
      </template>
      <dl v-else-if="sample.status === 'ready' && sample.sample.cells !== null" class="field-stack">
        <template v-for="(cell, index) in sample.sample.cells.slice(0, 64)" :key="index">
          <dt>{{ message("csvDecision.column", { column: index + 1 }) }}</dt>
          <dd class="min-w-0 whitespace-pre-wrap break-all">
            {{ cell.text === "" ? message("csvDecision.emptyCell") : cell.text
            }}<span v-if="cell.truncated"> {{ message("csvDecision.truncated") }}</span>
          </dd>
        </template>
      </dl>
    </section>
  </fieldset>
</template>
