<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import { RouterLink } from "vue-router";
import type {
  CsvApplyV1,
  CsvDialect,
  EligibilityCsvMapping,
  EligibilityCsvPreview,
} from "../api/generated";
import { useEligibilityCsvImport } from "../csv-import";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import { messages, formatNumber } from "../messages";
import EligibilityCsvMappings from "./planner/EligibilityCsvMappings.vue";
import ValidationSummary from "./explanations/ValidationSummary.vue";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  locale?: string;
}>();
const importer = useEligibilityCsvImport(props.home, () => props.project);
const { state } = importer;
const copy = messages.eligibilityCsv;
const shared = messages.csvImport;
const dialect = ref<CsvDialect | "">("");
const header = ref<"" | "yes" | "no">("");
const width = ref("");
const personColumn = ref("");
const assignmentTypeColumn = ref("");
const eligibleColumn = ref("");
const personMappings = shallowRef<Readonly<Record<string, string>>>({});
const assignmentTypeMappings = shallowRef<Readonly<Record<string, string>>>({});
const skippedRecords = shallowRef<readonly number[]>([]);
const lastRows = shallowRef<EligibilityCsvPreview["rows"]>([]);
const rowPage = ref(0),
  reportPage = ref(0),
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
  /^[0-9]+$/.test(width.value) && Number(width.value) >= 3 && Number(width.value) <= 64
    ? Number(width.value)
    : 0,
);
const excluded = computed(() =>
  state.report?.rows.family === "eligibility" ? state.report.rows.excludedRows : [],
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
  skippedRecords.value = [];
  lastRows.value = [];
  rowPage.value = 0;
}
function chooseDialect(event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  if (value !== "" && value !== "comma" && value !== "semicolon" && value !== "tab") return;
  dialect.value = value;
  width.value =
    candidate.value?.status === "candidate" && candidate.value.consistentColumns !== null
      ? String(candidate.value.consistentColumns)
      : "";
  personColumn.value = "";
  assignmentTypeColumn.value = "";
  eligibleColumn.value = "";
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
  personColumn.value = "";
  assignmentTypeColumn.value = "";
  eligibleColumn.value = "";
  personMappings.value = {};
  assignmentTypeMappings.value = {};
  if (await importer.choose()) await importer.detect();
}
async function preview(): Promise<void> {
  if (busy.value || !editable.value) return;
  inputError.value = "";
  const columns = [personColumn.value, assignmentTypeColumn.value, eligibleColumn.value];
  if (
    !dialect.value ||
    !header.value ||
    !columnCount.value ||
    columns.some((column) => !/^[0-9]+$/.test(column) || Number(column) >= columnCount.value) ||
    new Set(columns.map(Number)).size !== 3
  ) {
    inputError.value = copy.invalidMapping;
    return;
  }
  const mapping: EligibilityCsvMapping = {
    dialect: dialect.value,
    hasHeader: header.value === "yes",
    expectedColumns: columnCount.value,
    personColumn: Number(personColumn.value),
    assignmentTypeColumn: Number(assignmentTypeColumn.value),
    eligibleColumn: Number(eligibleColumn.value),
    personMappings: personMappings.value,
    assignmentTypeMappings: assignmentTypeMappings.value,
  };
  approved.value = false;
  if (await importer.preview(mapping, skippedRecords.value)) {
    lastRows.value = state.preview?.preview.rows ?? [];
    rowPage.value = 0;
    reportPage.value = 0;
    validationPage.value = 0;
  }
}
function skip(record: number, selected: boolean): void {
  if (busy.value || !editable.value) return;
  skippedRecords.value = selected
    ? [...skippedRecords.value.filter((value) => value !== record), record].sort((a, b) => a - b)
    : skippedRecords.value.filter((value) => value !== record);
  invalidate();
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
    personColumn.value = "";
    assignmentTypeColumn.value = "";
    eligibleColumn.value = "";
    personMappings.value = {};
    assignmentTypeMappings.value = {};
    lastRows.value = [];
    skippedRecords.value = [];
    inputError.value = "";
  },
  { flush: "sync" },
);
</script>

<template>
  <section class="page-stack" aria-labelledby="eligibility-csv-heading" data-eligibility-csv-import>
    <h2 id="eligibility-csv-heading" ref="heading" data-route-heading tabindex="-1">
      {{ copy.heading }}
    </h2>
    <p>{{ copy.description }}</p>
    <RouterLink :to="{ name: 'project-eligibility', params: { scenarioId: project.scenarioId } }">{{
      copy.back
    }}</RouterLink>
    <p v-if="project.domainPackId !== 'official.workforce'">{{ messages.setup.unsupported }}</p>
    <template v-else>
      <p>{{ copy.framing }}</p>
      <p>{{ shared.limits }}</p>
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
                <select
                  id="eligibility-csv-dialect"
                  :value="dialect"
                  required
                  @change="chooseDialect"
                >
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
                  id="eligibility-csv-header"
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
                  id="eligibility-csv-width"
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
              <label
                >{{ copy.personColumn }}
                <select
                  id="eligibility-csv-person-column"
                  v-model="personColumn"
                  required
                  @change="changeInterpretation"
                >
                  <option value="">{{ copy.chooseColumn }}</option>
                  <option v-for="column in columnCount" :key="column" :value="String(column - 1)">
                    {{ copy.column(formatNumber(column, locale))
                    }}<template v-if="header === 'yes' && candidate?.status === 'candidate'">
                      · {{ candidate.samples[0]?.cells[column - 1]?.text }}</template
                    >
                  </option>
                </select>
              </label>
              <label
                >{{ copy.typeColumn }}
                <select
                  id="eligibility-csv-assignment-type-column"
                  v-model="assignmentTypeColumn"
                  required
                  @change="changeInterpretation"
                >
                  <option value="">{{ copy.chooseColumn }}</option>
                  <option v-for="column in columnCount" :key="column" :value="String(column - 1)">
                    {{ copy.column(formatNumber(column, locale))
                    }}<template v-if="header === 'yes' && candidate?.status === 'candidate'">
                      · {{ candidate.samples[0]?.cells[column - 1]?.text }}</template
                    >
                  </option>
                </select>
              </label>
              <label
                >{{ copy.membershipColumn }}
                <select
                  id="eligibility-csv-eligible-column"
                  v-model="eligibleColumn"
                  required
                  @change="changeInterpretation"
                >
                  <option value="">{{ copy.chooseColumn }}</option>
                  <option v-for="column in columnCount" :key="column" :value="String(column - 1)">
                    {{ copy.column(formatNumber(column, locale))
                    }}<template v-if="header === 'yes' && candidate?.status === 'candidate'">
                      · {{ candidate.samples[0]?.cells[column - 1]?.text }}</template
                    >
                  </option>
                </select>
              </label>
              <p>{{ copy.columnsHelp }}</p>
              <EligibilityCsvMappings
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                :person-mappings="personMappings"
                :assignment-type-mappings="assignmentTypeMappings"
                :disabled="busy"
                @update:person-mappings="
                  personMappings = $event;
                  invalidate();
                "
                @update:assignment-type-mappings="
                  assignmentTypeMappings = $event;
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
                id="eligibility-csv-inspect-record"
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
      <section v-if="lastRows.length && editable" class="field-stack" :aria-label="copy.decisions">
        <h3>{{ copy.decisions }}</h3>
        <p>
          {{ state.preview ? copy.currentFindings : copy.previousFindings }}
          {{ copy.duplicatePolicy }}
        </p>
        <p>{{ copy.skipCount(formatNumber(skippedRecords.length, locale)) }}</p>
        <button
          type="button"
          :disabled="busy"
          @click="
            skippedRecords = lastRows.map((row) => row.record);
            invalidate();
          "
        >
          {{ copy.skipAll }}
        </button>
        <button
          type="button"
          :disabled="busy || skippedRecords.length === 0"
          @click="
            skippedRecords = [];
            invalidate();
          "
        >
          {{ copy.clearSkips }}
        </button>
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
                <th scope="col">{{ shared.record }}</th>
                <th scope="col">{{ copy.status }}</th>
                <th scope="col">{{ copy.personIdentity }}</th>
                <th scope="col">{{ copy.typeIdentity }}</th>
                <th scope="col">{{ copy.before }}</th>
                <th scope="col">{{ copy.after }}</th>
                <th scope="col">{{ copy.decision }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in lastRows.slice(rowPage * 50, (rowPage + 1) * 50)" :key="row.record">
                <td>{{ row.record }}</td>
                <td>
                  {{ row.status }}<code v-if="row.issue"> · {{ row.issue }}</code>
                </td>
                <td>
                  <code class="break-all">{{ row.personId ?? copy.unresolved }}</code>
                </td>
                <td>
                  <code class="break-all">{{ row.assignmentTypeId ?? copy.unresolved }}</code>
                </td>
                <td>{{ row.before === null ? copy.unknown : String(row.before) }}</td>
                <td>{{ row.after === null ? copy.unknown : String(row.after) }}</td>
                <td>
                  <label
                    ><input
                      type="checkbox"
                      :checked="skippedRecords.includes(row.record)"
                      :disabled="busy"
                      @change="skip(row.record, ($event.target as HTMLInputElement).checked)"
                    />{{ copy.skipRecord(String(row.record)) }}</label
                  ><button type="button" :disabled="busy" @click="inspect(row.record)">
                    {{ copy.inspectRow(String(row.record)) }}
                  </button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <nav v-if="lastRows.length > 50" :aria-label="copy.recordPages">
          <button type="button" :disabled="rowPage === 0" @click="rowPage -= 1">
            {{ copy.previousRecords }}</button
          ><button
            type="button"
            :disabled="(rowPage + 1) * 50 >= lastRows.length"
            @click="rowPage += 1"
          >
            {{ copy.nextRecords }}
          </button>
        </nav>
      </section>
      <section
        v-if="state.preview"
        class="state-panel field-stack"
        aria-labelledby="eligibility-csv-review"
        data-eligibility-csv-review
      >
        <h3 id="eligibility-csv-review" ref="reviewHeading" tabindex="-1">{{ copy.review }}</h3>
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
              id="eligibility-csv-approve"
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
        aria-labelledby="eligibility-csv-report"
      >
        <h3 id="eligibility-csv-report" ref="reportHeading" tabindex="-1">{{ copy.report }}</h3>
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
