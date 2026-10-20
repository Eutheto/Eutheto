<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import { RouterLink } from "vue-router";
import {
  MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES,
  type CsvApplyV1,
  type EligibilityMatrixCellAddress,
  type EligibilityMatrixPreview,
} from "../api/generated";
import { useEligibilityMatrixImport } from "../csv-import";
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
const importer = useEligibilityMatrixImport(props.home, () => props.project);
const { state } = importer;
const copy = messages.eligibilityMatrix;
const labels = messages.eligibilityCsv;
const shared = messages.csvImport;
const raw = ref("");
const textarea = ref<HTMLTextAreaElement>();
const personMappings = shallowRef<Readonly<Record<string, string>>>({});
const assignmentTypeMappings = shallowRef<Readonly<Record<string, string>>>({});
const skippedCells = shallowRef<readonly EligibilityMatrixCellAddress[]>([]);
const lastRows = shallowRef<EligibilityMatrixPreview["rows"]>([]);
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
const outcomeUnknown = computed(() => props.home.state.mutation?.outcome === "outcomeUnknown");
const busy = computed(
  () =>
    state.pending || state.applying || props.home.state.busyAction !== null || outcomeUnknown.value,
);
const editable = computed(
  () => state.source !== null && state.sourceState === "ready" && !outcomeUnknown.value,
);
const excluded = computed(() =>
  state.report?.rows.family === "eligibilityMatrix" ? state.report.rows.excludedCells : [],
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
function clearRaw(): void {
  raw.value = "";
  if (textarea.value) textarea.value.value = "";
}
function admit(value: string): boolean {
  // Check code units before allocating UTF-8; never retain a truncated prefix.
  if (
    value.length > MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES ||
    new TextEncoder().encode(value).length > MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES
  ) {
    clearRaw();
    inputError.value = copy.overflow;
    return false;
  }
  inputError.value = "";
  return true;
}
function input(event: Event): void {
  const element = event.target as HTMLTextAreaElement;
  if (busy.value || state.source !== null) {
    element.value = raw.value;
    return;
  }
  if (admit(element.value)) raw.value = element.value;
  else element.value = "";
}
function invalidate(): void {
  approved.value = false;
  importer.invalidate();
  validationPage.value = 0;
}
function resetDecisions(): void {
  personMappings.value = {};
  assignmentTypeMappings.value = {};
  skippedCells.value = [];
  lastRows.value = [];
  rowPage.value = 0;
  reportPage.value = 0;
  validationPage.value = 0;
  inspectRecord.value = "1";
  approved.value = false;
  inputError.value = "";
}
async function capture(): Promise<void> {
  if (busy.value || state.source !== null || props.home.state.reviewCleanupError !== null) return;
  // Recheck the whole DOM value, including changes not yet delivered as input events.
  const text = textarea.value?.value ?? raw.value;
  if (!admit(text)) return;
  raw.value = text;
  if (!text) {
    inputError.value = copy.empty;
    return;
  }
  resetDecisions();
  if (await importer.paste(text)) await importer.detect();
}
async function replace(): Promise<boolean> {
  if (busy.value) return false;
  const retirement = importer.discardSource();
  resetDecisions();
  return retirement;
}
async function preview(): Promise<void> {
  if (busy.value || !editable.value) return;
  approved.value = false;
  if (
    await importer.preview(
      {
        personMappings: personMappings.value,
        assignmentTypeMappings: assignmentTypeMappings.value,
      },
      skippedCells.value,
    )
  ) {
    lastRows.value = state.preview?.preview.rows ?? [];
    rowPage.value = 0;
    reportPage.value = 0;
    validationPage.value = 0;
  }
}
function sameCell(a: EligibilityMatrixCellAddress, b: EligibilityMatrixCellAddress): boolean {
  return a.record === b.record && a.column === b.column;
}
function skip(cell: EligibilityMatrixCellAddress, selected: boolean): void {
  if (busy.value || !editable.value || !lastRows.value.some((row) => sameCell(row.cell, cell)))
    return;
  const others = skippedCells.value.filter((value) => !sameCell(value, cell));
  skippedCells.value = selected
    ? [...others, cell].sort((a, b) => a.record - b.record || a.column - b.column)
    : others;
  invalidate();
}
function skipAll(): void {
  if (busy.value || !editable.value) return;
  // The generated native-row guard guarantees unique row-major coordinates.
  skippedCells.value = lastRows.value.map((row) => row.cell);
  invalidate();
}
function inspect(record: number): void {
  if (
    !busy.value &&
    editable.value &&
    Number.isInteger(record) &&
    record >= 1 &&
    record <= (state.detection?.source.logicalRecords ?? 10_001)
  )
    void importer.inspect(record);
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
watch(
  () => state.approval,
  () => {
    approved.value = false;
  },
  { flush: "sync" },
);
watch(
  () => state.sourceState,
  (value) => {
    if (value === "consumed") clearRaw();
  },
  { flush: "sync" },
);
watch(
  () => props.project.scenarioId,
  () => {
    clearRaw();
    resetDecisions();
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  alive = false;
  clearRaw();
});
function discard(): void {
  clearRaw();
  importer.invalidate();
  importer.invalidateSample();
}
</script>

<template>
  <section
    class="page-stack"
    aria-labelledby="eligibility-matrix-heading"
    data-eligibility-matrix-import
  >
    <h2 id="eligibility-matrix-heading" ref="heading" data-route-heading tabindex="-1">
      {{ copy.heading }}
    </h2>
    <p>{{ copy.description }}</p>
    <RouterLink :to="{ name: 'project-eligibility', params: { scenarioId: project.scenarioId } }">{{
      labels.back
    }}</RouterLink>
    <p v-if="project.domainPackId !== 'official.workforce'">{{ messages.setup.unsupported }}</p>
    <template v-else>
      <p>{{ copy.grammar }}</p>
      <p id="eligibility-matrix-input-help">{{ copy.limits }}</p>
      <p>{{ copy.fingerprintHelp }}</p>
      <form
        class="field-stack"
        @submit.prevent="
          focusAfter(capture(), () => (inputError || state.error ? errorHeading : heading))
        "
      >
        <label for="eligibility-matrix-text">{{ copy.text }}</label>
        <textarea
          id="eligibility-matrix-text"
          ref="textarea"
          :value="raw"
          :readonly="state.source !== null"
          :disabled="busy"
          :aria-invalid="inputError !== ''"
          aria-describedby="eligibility-matrix-input-help"
          rows="8"
          spellcheck="false"
          autocomplete="off"
          @input="input"
        />
        <button
          v-if="!state.source"
          id="eligibility-matrix-capture"
          type="submit"
          :disabled="busy || home.state.reviewCleanupError !== null"
        >
          {{ copy.capture }}
        </button>
        <button
          v-else
          id="eligibility-matrix-replace"
          type="button"
          :disabled="busy"
          @click="
            focusAfter(replace(), () => (home.state.reviewCleanupError ? errorHeading : textarea))
          "
        >
          {{ copy.replace }}
        </button>
        <p v-if="state.source">{{ copy.replaceHelp }}</p>
      </form>
      <section
        v-if="inputError || state.error || home.state.reviewCleanupError"
        class="state-panel"
        role="alert"
      >
        <h3 id="eligibility-matrix-error" ref="errorHeading" tabindex="-1">{{ shared.error }}</h3>
        <p>{{ inputError || state.error || shared.sourceResetFailed }}</p>
      </section>
      <p v-if="state.pending" role="status">{{ copy.pending }}</p>
      <p v-if="state.stale" role="status">{{ shared.stale }}</p>
      <RouterLink
        v-if="outcomeUnknown || state.sourceState === 'uncertain'"
        id="eligibility-matrix-history"
        :to="{ name: 'project-history', params: { scenarioId: project.scenarioId } }"
        >{{ labels.history }}</RouterLink
      >
      <section v-if="state.source" class="field-stack" :aria-label="copy.source">
        <p>
          {{ shared.sourceId }} <code class="break-all">{{ state.source.sourceId }}</code> ·
          {{ shared.bytes }} {{ formatNumber(state.source.byteCount, locale) }}
        </p>
        <p v-if="state.sourceState === 'consumed'" role="status">{{ copy.consumed }}</p>
        <p v-if="state.sourceState === 'uncertain'" role="alert">{{ shared.uncertain }}</p>
        <template v-if="editable">
          <button
            id="eligibility-matrix-detect"
            type="button"
            :disabled="busy"
            @click="importer.detect()"
          >
            {{ copy.detect }}
          </button>
          <template v-if="state.detection">
            <p id="eligibility-matrix-census">
              {{
                copy.census(
                  formatNumber(state.detection.columns, locale),
                  formatNumber(state.detection.source.logicalRecords, locale),
                  formatNumber(state.detection.cells, locale),
                )
              }}
            </p>
            <section :aria-label="copy.samples">
              <h3>{{ copy.samples }}</h3>
              <div v-for="sample in state.detection.samples" :key="sample.record">
                <p>{{ shared.record }} {{ sample.record }}</p>
                <ul>
                  <li v-for="(cell, index) in sample.cells" :key="index">
                    {{ labels.column(String(index + 1)) }}:
                    <code class="break-all">{{ cell.text }}</code
                    ><span v-if="cell.truncated"> {{ labels.truncated }}</span>
                  </li>
                </ul>
              </div>
            </section>
            <form
              class="field-stack"
              @submit.prevent="
                focusAfter(preview(), () => (state.error ? errorHeading : reviewHeading))
              "
            >
              <EligibilityCsvMappings
                :key="state.source.sourceId"
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
              <p v-if="state.reportId">{{ labels.replaceReport }}</p>
              <button id="eligibility-matrix-preview" type="submit" :disabled="busy">
                {{ shared.preview }}
              </button>
            </form>
          </template>
          <section class="field-stack" :aria-label="copy.inspect">
            <label
              >{{ labels.inspectRecord
              }}<input
                id="eligibility-matrix-inspect-record"
                v-model="inspectRecord"
                inputmode="numeric"
                pattern="[0-9]+"
                maxlength="5"
                :disabled="busy"
            /></label>
            <button
              id="eligibility-matrix-inspect"
              type="button"
              :disabled="
                busy ||
                !/^[0-9]+$/.test(inspectRecord) ||
                Number(inspectRecord) < 1 ||
                Number(inspectRecord) > (state.detection?.source.logicalRecords ?? 10_001)
              "
              @click="inspect(Number(inspectRecord))"
            >
              {{ labels.inspectSelected }}
            </button>
            <p v-if="state.sample.status === 'loading'" role="status">{{ labels.inspecting }}</p>
            <p v-if="state.sample.status === 'error'" role="alert">{{ state.sample.message }}</p>
            <p
              v-if="state.sample.status === 'ready' && state.sample.sample.cells === null"
              role="status"
            >
              {{ labels.missingRecord }}
            </p>
            <div
              v-if="state.sample.status === 'ready' && state.sample.sample.cells !== null"
              class="overflow-x-auto"
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
                    <th scope="col">{{ copy.column }}</th>
                    <th scope="col">{{ labels.inertText }}</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="(cell, index) in state.sample.sample.cells" :key="index">
                    <td>{{ index + 1 }}</td>
                    <td>
                      <code class="break-all">{{ cell.text }}</code
                      ><span v-if="cell.truncated"> {{ labels.truncated }}</span>
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
          {{ state.preview ? labels.currentFindings : labels.previousFindings }}
          {{ copy.duplicatePolicy }}
        </p>
        <p>{{ copy.skipCount(formatNumber(skippedCells.length, locale)) }}</p>
        <button id="eligibility-matrix-skip-all" type="button" :disabled="busy" @click="skipAll">
          {{ copy.skipAll }}
        </button>
        <button
          id="eligibility-matrix-clear-skips"
          type="button"
          :disabled="busy || skippedCells.length === 0"
          @click="
            skippedCells = [];
            invalidate();
          "
        >
          {{ labels.clearSkips }}
        </button>
        <div class="overflow-x-auto" role="region" :aria-label="copy.cells" tabindex="0">
          <table id="eligibility-matrix-cells" class="w-full text-left">
            <caption>
              {{
                copy.cells
              }}
            </caption>
            <thead>
              <tr>
                <th scope="col">{{ shared.record }}</th>
                <th scope="col">{{ copy.column }}</th>
                <th scope="col">{{ labels.status }}</th>
                <th scope="col">{{ labels.personIdentity }}</th>
                <th scope="col">{{ labels.typeIdentity }}</th>
                <th scope="col">{{ labels.before }}</th>
                <th scope="col">{{ labels.after }}</th>
                <th scope="col">{{ labels.decision }}</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="row in lastRows.slice(rowPage * 50, (rowPage + 1) * 50)"
                :key="`${row.cell.record}:${row.cell.column}`"
                :data-matrix-record="row.cell.record"
                :data-matrix-column="row.cell.column"
              >
                <td>{{ row.cell.record }}</td>
                <td>{{ row.cell.column + 1 }}</td>
                <td>
                  {{ row.status }}<code v-if="row.issue"> · {{ row.issue }}</code>
                </td>
                <td>
                  <code class="break-all">{{ row.personId ?? labels.unresolved }}</code>
                </td>
                <td>
                  <code class="break-all">{{ row.assignmentTypeId ?? labels.unresolved }}</code>
                </td>
                <td>{{ row.before === null ? labels.unknown : String(row.before) }}</td>
                <td>{{ row.after === null ? labels.unknown : String(row.after) }}</td>
                <td>
                  <label
                    ><input
                      :id="`eligibility-matrix-skip-${row.cell.record}-${row.cell.column}`"
                      type="checkbox"
                      :checked="skippedCells.some((cell) => sameCell(cell, row.cell))"
                      :disabled="busy"
                      @change="skip(row.cell, ($event.target as HTMLInputElement).checked)"
                    />{{
                      copy.skipCell(String(row.cell.record), String(row.cell.column + 1))
                    }}</label
                  ><button type="button" :disabled="busy" @click="inspect(row.cell.record)">
                    {{ labels.inspectRow(String(row.cell.record)) }}
                  </button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <nav v-if="lastRows.length > 50" :aria-label="copy.cellPages">
          <button
            id="eligibility-matrix-previous-cells"
            type="button"
            :disabled="rowPage === 0"
            @click="rowPage -= 1"
          >
            {{ copy.previousCells }}</button
          ><button
            id="eligibility-matrix-next-cells"
            type="button"
            :disabled="(rowPage + 1) * 50 >= lastRows.length"
            @click="rowPage += 1"
          >
            {{ copy.nextCells }}
          </button>
        </nav>
      </section>
      <section
        v-if="state.preview"
        class="state-panel field-stack"
        aria-labelledby="eligibility-matrix-review"
        data-eligibility-matrix-review
      >
        <h3 id="eligibility-matrix-review" ref="reviewHeading" tabindex="-1">
          {{ labels.review }}
        </h3>
        <p>
          {{
            state.preview.preview.disposition === "blocked"
              ? copy.blocked
              : shared.dispositions[state.preview.preview.disposition]
          }}
        </p>
        <p>{{ labels.revision(formatNumber(state.preview.revision, locale)) }}</p>
        <p v-if="state.preview.preview.batch">
          {{ labels.batch(formatNumber(state.preview.preview.batch.commands.length, locale)) }}
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
          :aria-label="labels.validationPages"
        >
          <button type="button" :disabled="validationPage === 0" @click="validationPage -= 1">
            {{ labels.previousFindingsPage }}</button
          ><button
            type="button"
            :disabled="(validationPage + 1) * 50 >= state.preview.preview.validationIssues.length"
            @click="validationPage += 1"
          >
            {{ labels.nextFindingsPage }}
          </button>
        </nav>
        <template v-if="state.approval">
          <p class="break-all">
            {{ labels.command }} <code>{{ state.approval.commandId }}</code>
          </p>
          <label
            ><input
              id="eligibility-matrix-approve"
              v-model="approved"
              type="checkbox"
              :disabled="busy || state.stale"
            />{{ labels.approve }}</label
          >
          <p v-if="state.redoRequired" role="alert">{{ shared.redo }}</p>
          <button
            id="eligibility-matrix-apply"
            type="button"
            :disabled="busy || state.stale || !approved"
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
        aria-labelledby="eligibility-matrix-report"
      >
        <h3 id="eligibility-matrix-report" ref="reportHeading" tabindex="-1">
          {{ labels.report }}
        </h3>
        <p>{{ copy.reportHelp }}</p>
        <button
          id="eligibility-matrix-read-report"
          type="button"
          :disabled="busy"
          @click="
            focusAfter(importer.readReport(), () => (state.error ? errorHeading : reportHeading))
          "
        >
          {{ labels.readReport }}</button
        ><button
          id="eligibility-matrix-save-report"
          type="button"
          :disabled="busy"
          @click="importer.saveReport()"
        >
          {{ labels.saveReport }}
        </button>
        <template v-if="state.report">
          <p>{{ state.report.consumed ? shared.reportConsumed : shared.reportUnconsumed }}</p>
          <p v-if="excluded.length === 0">{{ copy.noExclusions }}</p>
          <div
            v-else
            class="overflow-x-auto"
            role="region"
            :aria-label="copy.excluded"
            tabindex="0"
          >
            <table id="eligibility-matrix-excluded" class="w-full text-left">
              <caption>
                {{
                  copy.excluded
                }}
              </caption>
              <thead>
                <tr>
                  <th scope="col">{{ shared.record }}</th>
                  <th scope="col">{{ copy.column }}</th>
                  <th scope="col">{{ labels.skipped }}</th>
                  <th scope="col">{{ labels.issue }}</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="row in excluded.slice(reportPage * 50, (reportPage + 1) * 50)"
                  :key="`${row.cell.record}:${row.cell.column}`"
                >
                  <td>{{ row.cell.record }}</td>
                  <td>{{ row.cell.column + 1 }}</td>
                  <td>{{ row.skipped ? labels.yes : labels.no }}</td>
                  <td>
                    <code>{{ row.issue ?? labels.none }}</code>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <nav v-if="excluded.length > 50" :aria-label="labels.reportPages">
            <button
              id="eligibility-matrix-previous-exclusions"
              type="button"
              :disabled="reportPage === 0"
              @click="reportPage -= 1"
            >
              {{ labels.previousExclusions }}</button
            ><button
              id="eligibility-matrix-next-exclusions"
              type="button"
              :disabled="(reportPage + 1) * 50 >= excluded.length"
              @click="reportPage += 1"
            >
              {{ labels.nextExclusions }}
            </button>
          </nav>
        </template>
      </section>
    </template>
    <RouteLeaveGuard
      :home="home"
      :dirty="raw !== '' || state.source !== null"
      :pending="state.pending"
      :discard="discard"
    />
  </section>
</template>
