<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import { RouterLink } from "vue-router";
import type {
  CsvApplyV1,
  CsvDialect,
  ShiftCsvMapping,
  ShiftCsvPreview,
  ShiftCsvField,
  ShiftCsvRowDecision,
} from "../api/generated";
import { useShiftCsvImport } from "../csv-import";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import { messages, formatNumber } from "../messages";
import ShiftCsvMappings from "./planner/ShiftCsvMappings.vue";
import ShiftCsvDecisions from "./planner/ShiftCsvDecisions.vue";
import ValidationSummary from "./explanations/ValidationSummary.vue";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  locale?: string;
}>();
const importer = useShiftCsvImport(props.home, () => props.project);
const { state } = importer;
const copy = messages.shiftCsv;
const shared = messages.csvImport;
const dialect = ref<CsvDialect | "">("");
const header = ref<"" | "yes" | "no">("");
const width = ref("");
const fields: readonly ShiftCsvField[] = [
  "assignmentTypeId",
  "locationId",
  "startsAt",
  "endsAt",
  "coverage",
  "tags",
  "reportingAttribution",
];
const identityColumn = ref("");
const fieldColumns = ref<Record<ShiftCsvField, string>>({
  assignmentTypeId: "",
  locationId: "",
  startsAt: "",
  endsAt: "",
  coverage: "",
  tags: "",
  reportingAttribution: "",
});
const blanks = ref<Record<ShiftCsvField, "preserve" | "clear">>({
  assignmentTypeId: "preserve",
  locationId: "preserve",
  startsAt: "preserve",
  endsAt: "preserve",
  coverage: "preserve",
  tags: "preserve",
  reportingAttribution: "preserve",
});
const referenceMappings = shallowRef<Readonly<Record<string, string>>>({});
const decisions = shallowRef<readonly ShiftCsvRowDecision[]>([]);
const lastRows = shallowRef<ShiftCsvPreview["rows"]>([]);
const interpretationKey = ref(0);
const reportPage = ref(0),
  validationPage = ref(0);
const inspectRecord = ref("1");
const inputError = ref("");
const approved = ref(false);
const heading = ref<HTMLElement>(),
  reviewHeading = ref<HTMLElement>(),
  reportHeading = ref<HTMLElement>(),
  errorHeading = ref<HTMLElement>();
let alive = true;
onScopeDispose(() => {
  alive = false;
});
const busy = computed(() => state.pending || props.home.state.busyAction !== null);
const editable = computed(() => state.source !== null && state.sourceState === "ready");
const outcomeUnknown = computed(() => props.home.state.mutation?.outcome === "outcomeUnknown");
const candidate = computed(() =>
  state.detection?.dialects.find(
    (item) => item.status === "candidate" && item.dialect === dialect.value,
  ),
);
const columnCount = computed(() =>
  /^[0-9]+$/.test(width.value) && Number(width.value) >= 1 && Number(width.value) <= 64
    ? Number(width.value)
    : 0,
);
const excluded = computed(() =>
  state.report?.rows.family === "shifts" ? state.report.rows.excludedRows : [],
);
const findings = computed(() => {
  const all = state.preview?.preview.validationIssues ?? [];
  const counts = { errors: 0, warnings: 0, information: 0 };
  for (const issue of all) {
    if (issue.severity === "error") counts.errors += 1;
    else if (issue.severity === "warning") counts.warnings += 1;
    else counts.information += 1;
  }
  const issues = all.slice(validationPage.value * 50, (validationPage.value + 1) * 50);
  return { counts, issues, omitted: all.length - issues.length };
});
function invalidate(): void {
  approved.value = false;
  importer.invalidate();
  inputError.value = "";
  validationPage.value = 0;
}
function changeInterpretation(): void {
  invalidate();
  importer.invalidateSample();
  decisions.value = [];
  lastRows.value = [];
  interpretationKey.value += 1;
}
function chooseDialect(event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  if (value !== "" && value !== "comma" && value !== "semicolon" && value !== "tab") return;
  dialect.value = value;
  width.value =
    candidate.value?.status === "candidate" && candidate.value.consistentColumns !== null
      ? String(candidate.value.consistentColumns)
      : "";
  identityColumn.value = "";
  for (const field of fields) fieldColumns.value[field] = "";
  changeInterpretation();
}
async function focusAfter(
  action: Promise<boolean | CsvApplyV1 | null> | Promise<void>,
  target: () => HTMLElement | undefined,
): Promise<void> {
  const origin = document.activeElement;
  const scenarioId = props.project.scenarioId;
  const revision = props.project.revision;
  const epoch = props.home.state.libraryEpoch;
  const result = await action;
  const receipt = typeof result === "object" && result !== null ? result : null;
  // Apply owns a refresh: bind its displayed receipt and post-refresh context, not the old epoch.
  const expectedRevision = receipt?.outcome.revision ?? revision;
  const expectedEpoch = receipt === null ? epoch : props.home.state.libraryEpoch;
  await nextTick();
  if (
    alive &&
    props.project.scenarioId === scenarioId &&
    props.project.revision === expectedRevision &&
    props.home.state.libraryEpoch === expectedEpoch &&
    (receipt === null ||
      (receipt.scenarioId === scenarioId &&
        receipt.sourceId === state.source?.sourceId &&
        receipt.report.previewId === state.reportId)) &&
    (document.activeElement === origin || document.activeElement === document.body)
  )
    target()?.focus();
}
async function choose(): Promise<void> {
  if (busy.value || props.home.state.reviewCleanupError !== null) return;
  changeInterpretation();
  dialect.value = "";
  header.value = "";
  width.value = "";
  identityColumn.value = "";
  for (const field of fields) {
    fieldColumns.value[field] = "";
    blanks.value[field] = "preserve";
  }
  referenceMappings.value = {};
  if (await importer.choose()) await importer.detect();
}
async function preview(): Promise<void> {
  if (busy.value || !editable.value) return;
  inputError.value = "";
  const columns = [
    identityColumn.value,
    ...fields.map((field) => fieldColumns.value[field]),
  ].filter((column) => column !== "");
  if (
    !dialect.value ||
    !header.value ||
    !columnCount.value ||
    columns.some((column) => !/^[0-9]+$/.test(column) || Number(column) >= columnCount.value) ||
    new Set(columns.map(Number)).size !== columns.length
  ) {
    inputError.value = copy.invalidMapping;
    return;
  }
  const mapping: ShiftCsvMapping = {
    dialect: dialect.value,
    hasHeader: header.value === "yes",
    expectedColumns: columnCount.value,
    identityColumn: identityColumn.value === "" ? null : Number(identityColumn.value),
    columns: fields
      .filter((field) => fieldColumns.value[field] !== "")
      .map((field) => ({
        field,
        index: Number(fieldColumns.value[field]),
        blank: blanks.value[field],
      })),
    referenceMappings: referenceMappings.value,
  };
  approved.value = false;
  if (await importer.preview(mapping, decisions.value)) {
    lastRows.value = state.preview?.preview.rows ?? [];
    reportPage.value = 0;
    validationPage.value = 0;
  }
}
function inspect(record: number): void {
  if (dialect.value && Number.isInteger(record) && record > 0 && record <= 10_001)
    void importer.inspect(dialect.value, record);
}
watch(
  () => state.approval,
  () => {
    approved.value = false;
  },
  { flush: "sync" },
);
watch(
  () => props.project.scenarioId,
  () => {
    dialect.value = "";
    header.value = "";
    width.value = "";
    identityColumn.value = "";
    for (const field of fields) {
      fieldColumns.value[field] = "";
      blanks.value[field] = "preserve";
    }
    referenceMappings.value = {};
    lastRows.value = [];
    decisions.value = [];
    interpretationKey.value += 1;
    inputError.value = "";
  },
  { flush: "sync" },
);
</script>

<template>
  <section class="page-stack" aria-labelledby="shift-csv-heading" data-shift-csv-import>
    <h2 id="shift-csv-heading" ref="heading" data-route-heading tabindex="-1">
      {{ copy.heading }}
    </h2>
    <p>{{ copy.description }}</p>
    <RouterLink :to="{ name: 'project-work', params: { scenarioId: project.scenarioId } }">{{
      copy.back
    }}</RouterLink>
    <p v-if="project.domainPackId !== 'official.workforce'">{{ messages.setup.unsupported }}</p>
    <template v-else>
      <p>{{ copy.framing }}</p>
      <p v-if="state.source">{{ copy.replaceHelp }}</p>
      <button
        type="button"
        :disabled="busy || home.state.reviewCleanupError !== null"
        @click="focusAfter(choose(), () => (state.error ? errorHeading : heading))"
      >
        {{ state.source ? shared.replace : shared.choose }}
      </button>
      <section v-if="inputError || state.error" class="state-panel" role="alert">
        <h3 ref="errorHeading" tabindex="-1">{{ shared.error }}</h3>
        <p>{{ inputError || state.error }}</p>
      </section>
      <p v-if="state.pending" role="status">{{ copy.pending }}</p>
      <p v-if="state.stale" role="status">{{ shared.stale }}</p>
      <RouterLink
        v-if="outcomeUnknown || state.sourceState === 'uncertain'"
        :to="{ name: 'project-history', params: { scenarioId: project.scenarioId } }"
        >{{ copy.history }}</RouterLink
      >
      <section v-if="state.source" class="field-stack" :aria-label="copy.source">
        <p>
          {{ shared.sourceId }} <code class="break-all">{{ state.source.sourceId }}</code> ·
          {{ shared.bytes }} {{ formatNumber(state.source.byteCount, locale) }}
        </p>
        <p v-if="state.sourceState === 'consumed'" role="status">{{ shared.consumed }}</p>
        <p v-if="state.sourceState === 'uncertain'" role="alert">{{ shared.uncertain }}</p>
        <template v-if="editable">
          <button type="button" :disabled="busy" @click="importer.detect()">
            {{ shared.detect }}
          </button>
          <p>{{ shared.encoding }}</p>
          <ul v-if="state.detection">
            <template v-for="item in state.detection.dialects" :key="item.dialect"
              ><li v-if="item.status === 'rejected'">
                {{ shared.dialects[item.dialect] }}: <code>{{ item.error.code }}</code
                ><span v-if="item.error.record">
                  · {{ shared.record }} {{ item.error.record }}</span
                >
              </li></template
            >
          </ul>
          <form
            v-if="state.detection"
            class="field-stack"
            @submit.prevent="
              focusAfter(preview(), () =>
                inputError || state.error ? errorHeading : reviewHeading,
              )
            "
          >
            <fieldset :disabled="busy" class="field-stack">
              <legend>{{ copy.mapping }}</legend>
              <label
                >{{ shared.dialect }}
                <select id="shift-csv-dialect" :value="dialect" required @change="chooseDialect">
                  <option value="">{{ shared.chooseDialect }}</option>
                  <template v-for="item in state.detection.dialects" :key="item.dialect"
                    ><option v-if="item.status === 'candidate'" :value="item.dialect">
                      {{ shared.dialects[item.dialect] }}
                    </option></template
                  >
                </select>
              </label>
              <label
                >{{ shared.header }}
                <select
                  id="shift-csv-header"
                  v-model="header"
                  required
                  @change="changeInterpretation"
                >
                  <option value="">{{ shared.chooseHeader }}</option>
                  <option value="yes">{{ shared.yes }}</option>
                  <option value="no">{{ shared.no }}</option>
                </select>
              </label>
              <label
                >{{ shared.width }}
                <input
                  id="shift-csv-width"
                  v-model="width"
                  required
                  inputmode="numeric"
                  pattern="[0-9]+"
                  maxlength="2"
                  @input="changeInterpretation"
              /></label>
              <p v-if="candidate?.status === 'candidate'">
                {{
                  candidate.consistentColumns === null
                    ? shared.inconsistentWidth
                    : `${shared.observedWidth} ${candidate.consistentColumns}`
                }}
              </p>
              <label>
                {{ copy.identityColumn }}
                <select
                  id="shift-csv-identity-column"
                  v-model="identityColumn"
                  @change="changeInterpretation"
                >
                  <option value="">{{ copy.unmapped }}</option>
                  <option v-for="column in columnCount" :key="column" :value="String(column - 1)">
                    {{ copy.column(formatNumber(column, locale)) }}
                    <template v-if="header === 'yes' && candidate?.status === 'candidate'">
                      · {{ candidate.samples[0]?.cells[column - 1]?.text }}
                    </template>
                  </option>
                </select>
              </label>
              <div v-for="field in fields" :key="field" class="field-stack">
                <label>
                  {{ copy.fields[field] }}
                  <select
                    :id="`shift-csv-${field}-column`"
                    v-model="fieldColumns[field]"
                    @change="changeInterpretation"
                  >
                    <option value="">{{ copy.unmapped }}</option>
                    <option v-for="column in columnCount" :key="column" :value="String(column - 1)">
                      {{ copy.column(formatNumber(column, locale)) }}
                      <template v-if="header === 'yes' && candidate?.status === 'candidate'">
                        · {{ candidate.samples[0]?.cells[column - 1]?.text }}
                      </template>
                    </option>
                  </select>
                </label>
                <label>
                  {{ copy.fields[field] }} · {{ copy.blank }}
                  <select
                    :id="`shift-csv-${field}-blank`"
                    v-model="blanks[field]"
                    @change="invalidate"
                  >
                    <option value="preserve">{{ copy.preserve }}</option>
                    <option v-if="field === 'locationId' || field === 'tags'" value="clear">
                      {{ copy.clear }}
                    </option>
                  </select>
                </label>
              </div>
              <p>{{ copy.columnsHelp }}</p>
              <p>{{ copy.timeHelp }}</p>
              <p>{{ copy.structuredHelp }}</p>
              <ShiftCsvMappings
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                :model-value="referenceMappings"
                :disabled="busy"
                @update:model-value="
                  referenceMappings = $event;
                  invalidate();
                "
              />
            </fieldset>
            <p v-if="state.reportId">{{ copy.replaceReport }}</p>
            <button type="submit" :disabled="busy">{{ shared.preview }}</button>
          </form>
          <section v-if="dialect" class="field-stack" :aria-label="copy.inspect">
            <label
              >{{ copy.inspectRecord }}
              <input
                id="shift-csv-inspect-record"
                v-model="inspectRecord"
                inputmode="numeric"
                pattern="[0-9]+"
                maxlength="5"
                :disabled="busy"
            /></label>
            <button
              type="button"
              :disabled="
                busy ||
                !/^[0-9]+$/.test(inspectRecord) ||
                Number(inspectRecord) < 1 ||
                Number(inspectRecord) > 10_001
              "
              @click="inspect(Number(inspectRecord))"
            >
              {{ copy.inspectSelected }}
            </button>
            <p v-if="state.sample.status === 'loading'" role="status">{{ copy.inspecting }}</p>
            <p v-if="state.sample.status === 'error'" role="alert">{{ state.sample.message }}</p>
            <p
              v-if="state.sample.status === 'ready' && state.sample.sample.cells === null"
              role="status"
            >
              {{ copy.missingRecord }}
            </p>
            <div
              v-if="state.sample.status === 'ready' && state.sample.sample.cells !== null"
              class="overflow-x-auto focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
              role="region"
              :aria-label="copy.selectedRecord"
              tabindex="0"
            >
              <table class="w-full text-left">
                <caption>
                  {{
                    copy.selectedRecord
                  }}
                  {{
                    state.sample.sample.record
                  }}
                </caption>
                <thead>
                  <tr>
                    <th scope="col">{{ copy.column("") }}</th>
                    <th scope="col">{{ copy.inertText }}</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="(cell, index) in state.sample.sample.cells" :key="index">
                    <td>{{ index + 1 }}</td>
                    <td>
                      <code class="break-all">{{ cell.text }}</code
                      ><span v-if="cell.truncated"> {{ copy.truncated }}</span>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </section>
        </template>
      </section>
      <ShiftCsvDecisions
        v-if="lastRows.length && editable"
        :key="interpretationKey"
        :project="project"
        :library-epoch="home.state.libraryEpoch"
        :rows="lastRows"
        :current="state.preview !== null"
        :model-value="decisions"
        :disabled="busy"
        @update:model-value="
          decisions = $event;
          invalidate();
        "
        @inspect="inspect"
      />
      <section
        v-if="state.preview"
        class="state-panel field-stack"
        aria-labelledby="shift-csv-review"
        data-shift-csv-review
      >
        <h3 id="shift-csv-review" ref="reviewHeading" tabindex="-1">{{ copy.review }}</h3>
        <p>
          {{
            state.preview.preview.disposition === "blocked"
              ? copy.blocked
              : shared.dispositions[state.preview.preview.disposition]
          }}
        </p>
        <p>{{ copy.revision(formatNumber(state.preview.revision, locale)) }}</p>
        <p v-if="state.preview.preview.batch">
          {{ copy.batch(formatNumber(state.preview.preview.batch.commands.length, locale)) }}
        </p>
        <p class="break-all">
          {{ copy.fingerprint }} <code>{{ state.preview.preview.source.blake3 }}</code>
        </p>
        <ValidationSummary
          :findings="findings"
          state="ready"
          interaction="static"
          :heading="shared.validation"
          :heading-level="4"
          :locale="locale"
        />
        <nav
          v-if="state.preview.preview.validationIssues.length > 50"
          :aria-label="copy.validationPages"
        >
          <button type="button" :disabled="validationPage === 0" @click="validationPage -= 1">
            {{ copy.previousFindingsPage }}</button
          ><button
            type="button"
            :disabled="(validationPage + 1) * 50 >= state.preview.preview.validationIssues.length"
            @click="validationPage += 1"
          >
            {{ copy.nextFindingsPage }}
          </button>
        </nav>
        <template v-if="state.approval">
          <p class="break-all">
            {{ copy.command }} <code>{{ state.approval.commandId }}</code>
          </p>
          <label
            ><input
              id="shift-csv-approve"
              v-model="approved"
              type="checkbox"
              :disabled="busy || outcomeUnknown || state.stale"
            />{{ copy.approve }}</label
          >
          <p v-if="state.redoRequired" role="alert">{{ shared.redo }}</p>
          <button
            type="button"
            :disabled="busy || outcomeUnknown || state.stale || !approved"
            @click="
              focusAfter(importer.apply(state.redoRequired), () =>
                state.error ? errorHeading : (reportHeading ?? reviewHeading),
              )
            "
          >
            {{ state.redoRequired ? shared.confirmRedo : shared.apply }}
          </button>
        </template>
      </section>
      <section
        v-if="state.reportId"
        class="state-panel field-stack"
        aria-labelledby="shift-csv-report"
      >
        <h3 id="shift-csv-report" ref="reportHeading" tabindex="-1">{{ copy.report }}</h3>
        <p>{{ copy.reportHelp }}</p>
        <button
          type="button"
          :disabled="busy"
          @click="
            focusAfter(importer.readReport(), () => (state.error ? errorHeading : reportHeading))
          "
        >
          {{ copy.readReport }}
        </button>
        <button type="button" :disabled="busy" @click="importer.saveReport()">
          {{ copy.saveReport }}
        </button>
        <template v-if="state.report">
          <p>{{ state.report.consumed ? shared.reportConsumed : shared.reportUnconsumed }}</p>
          <p v-if="excluded.length === 0">{{ copy.noExclusions }}</p>
          <div
            v-else
            class="overflow-x-auto focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
            role="region"
            :aria-label="copy.excluded"
            tabindex="0"
          >
            <table class="w-full text-left">
              <caption>
                {{
                  copy.excluded
                }}
              </caption>
              <thead>
                <tr>
                  <th scope="col">{{ shared.record }}</th>
                  <th scope="col">{{ copy.skipped }}</th>
                  <th scope="col">{{ copy.issue }}</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="row in excluded.slice(reportPage * 50, (reportPage + 1) * 50)"
                  :key="row.record"
                >
                  <td>{{ row.record }}</td>
                  <td>{{ row.skipped ? copy.yes : copy.no }}</td>
                  <td>
                    <code>{{ row.issue ?? copy.none }}</code>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <nav v-if="excluded.length > 50" :aria-label="copy.reportPages">
            <button type="button" :disabled="reportPage === 0" @click="reportPage -= 1">
              {{ copy.previousExclusions }}</button
            ><button
              type="button"
              :disabled="(reportPage + 1) * 50 >= excluded.length"
              @click="reportPage += 1"
            >
              {{ copy.nextExclusions }}
            </button>
          </nav>
        </template>
      </section>
    </template>
    <RouteLeaveGuard
      :home="home"
      :dirty="state.source !== null"
      :pending="state.pending"
      :discard="
        () => {
          importer.invalidate();
          importer.invalidateSample();
        }
      "
    />
  </section>
</template>
