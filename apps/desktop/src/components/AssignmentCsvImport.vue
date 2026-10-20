<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import { RouterLink } from "vue-router";
import {
  newUuidV7,
  type AssignmentCsvMapping,
  type AssignmentCsvPreview,
  type AssignmentCsvReadFormat,
  type CsvDialect,
} from "../api/generated";
import { useAssignmentCsvImport } from "../csv-import";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import { messages, formatNumber } from "../messages";
import AssignmentCsvMappings from "./planner/AssignmentCsvMappings.vue";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";
import ValidationSummary from "./explanations/ValidationSummary.vue";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  locale?: string;
}>();
const importer = useAssignmentCsvImport(props.home, () => props.project);
const { state } = importer;
const copy = messages.csvImport;
const dialect = ref<CsvDialect | "">("");
const header = ref<"" | "yes" | "no">("");
const width = ref("");
const personColumn = ref("");
const shiftColumn = ref("");
const personMappings = shallowRef<Readonly<Record<string, string>>>({});
const shiftMappings = shallowRef<Readonly<Record<string, string>>>({});
const targetKind = ref<"" | "create" | "replace">("");
const createdId = ref(newUuidV7());
const selectedPlan = shallowRef<readonly string[]>([]);
const name = ref("");
const allowEmpty = ref(false);
const skippedRecords = shallowRef<readonly number[]>([]);
const lastRows = shallowRef<AssignmentCsvPreview["rows"]>([]);
const rowPage = ref(0),
  changePage = ref(0),
  reportPage = ref(0),
  validationPage = ref(0);
const inspectRecord = ref("1");
const inputError = ref("");
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
const canonical = computed(() => state.detection?.format === "canonicalV1");
const candidate = computed(() =>
  state.detection?.format === "mapped"
    ? state.detection.dialects.find(
        (item) => item.status === "candidate" && item.dialect === dialect.value,
      )
    : undefined,
);
const columnCount = computed(() =>
  /^[0-9]+$/.test(width.value) && Number(width.value) >= 2 && Number(width.value) <= 64
    ? Number(width.value)
    : 0,
);
const readFormat = computed<AssignmentCsvReadFormat | null>(() =>
  canonical.value
    ? { format: "canonicalV1" }
    : dialect.value
      ? { format: "mapped", dialect: dialect.value }
      : null,
);
const excluded = computed(() =>
  state.report?.rows.family === "assignments" ? state.report.rows.excludedRows : [],
);
const changes = computed(() => {
  const value = state.preview?.preview.changes;
  return value
    ? [
        ...value.addedPairs.map((pair) => ({ ...pair, change: "Add" })),
        ...value.removedPairs.map((pair) => ({ ...pair, change: "Remove" })),
      ]
    : [];
});
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
  importer.invalidate();
  inputError.value = "";
  changePage.value = 0;
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
  shiftColumn.value = "";
  changeInterpretation();
}
async function focusAfter(
  action: Promise<unknown>,
  target: () => HTMLElement | undefined,
): Promise<void> {
  const origin = document.activeElement,
    scenarioId = props.project.scenarioId;
  await action;
  await nextTick();
  if (
    alive &&
    props.project.scenarioId === scenarioId &&
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
  shiftColumn.value = "";
  personMappings.value = {};
  shiftMappings.value = {};
  targetKind.value = "";
  selectedPlan.value = [];
  createdId.value = newUuidV7();
  name.value = "";
  allowEmpty.value = false;
  if (await importer.choose()) await importer.detect();
}
async function preview(): Promise<void> {
  if (busy.value || !editable.value) return;
  inputError.value = "";
  let mapping: AssignmentCsvMapping;
  if (canonical.value) mapping = { format: "canonicalV1" };
  else {
    if (
      !dialect.value ||
      !header.value ||
      !columnCount.value ||
      !/^[0-9]+$/.test(personColumn.value) ||
      !/^[0-9]+$/.test(shiftColumn.value) ||
      personColumn.value === shiftColumn.value ||
      Number(personColumn.value) >= columnCount.value ||
      Number(shiftColumn.value) >= columnCount.value
    ) {
      inputError.value =
        "Choose a delimiter, header policy, 2–64 columns and two distinct endpoint columns.";
      return;
    }
    mapping = {
      format: "mapped",
      dialect: dialect.value,
      hasHeader: header.value === "yes",
      expectedColumns: columnCount.value,
      personColumn: Number(personColumn.value),
      shiftColumn: Number(shiftColumn.value),
      personMappings: personMappings.value,
      shiftMappings: shiftMappings.value,
    };
  }
  const id = targetKind.value === "create" ? createdId.value : selectedPlan.value[0];
  if (!targetKind.value || !id || !name.value) {
    inputError.value =
      "Explicitly choose a new named plan or an existing plan to replace, and enter its proposed name.";
    return;
  }
  if (
    await importer.preview(
      mapping,
      { kind: targetKind.value, id, name: name.value },
      skippedRecords.value,
      allowEmpty.value,
    )
  ) {
    lastRows.value = state.preview?.preview.rows ?? [];
    rowPage.value = 0;
    reportPage.value = 0;
  }
}
function skip(record: number, selected: boolean): void {
  if (busy.value || !editable.value) return;
  skippedRecords.value = selected
    ? [...skippedRecords.value.filter((value) => value !== record), record].sort((a, b) => a - b)
    : skippedRecords.value.filter((value) => value !== record);
  invalidate();
}
function skipAll(): void {
  if (busy.value || !editable.value) return;
  skippedRecords.value = lastRows.value.map((row) => row.record);
  invalidate();
}
function inspect(record: number): void {
  if (readFormat.value && Number.isInteger(record)) void importer.inspect(readFormat.value, record);
}
watch(
  [() => props.project.scenarioId],
  () => {
    lastRows.value = [];
    skippedRecords.value = [];
  },
  { flush: "sync" },
);
</script>

<template>
  <section class="page-stack" aria-labelledby="assignment-csv-heading" data-assignment-csv-import>
    <h2 id="assignment-csv-heading" ref="heading" data-route-heading tabindex="-1">
      Import assignments from CSV
    </h2>
    <p>
      Import a complete, editable starting plan — explicitly unverified. This never creates an
      accepted result, selects a Repair source, changes accepted ancestry, or adds Required locks.
      Review every addition and removal before one atomic save.
    </p>
    <RouterLink :to="{ name: 'project-setup', params: { scenarioId: project.scenarioId } }"
      >Back to planning and imported starting plans</RouterLink
    >
    <p v-if="project.domainPackId !== 'official.workforce'">{{ messages.setup.unsupported }}</p>
    <template v-else>
      <p>
        Use an Eutheto assignments v1 export or explicitly map comma-, semicolon- or tab-separated
        UTF-8. At most 16 MiB and 10,000 data records, including skipped records. Reserved Eutheto
        files must pass their strict format; malformed exports cannot fall back to mapped CSV.
      </p>
      <p v-if="state.source">
        Choosing another source discards its mapping, decisions and retained report. Save the report
        first if needed.
      </p>
      <button
        type="button"
        :disabled="busy || home.state.reviewCleanupError !== null"
        @click="focusAfter(choose(), () => errorHeading ?? heading)"
      >
        {{ state.source ? copy.replace : copy.choose }}
      </button>
      <section v-if="inputError || state.error" class="state-panel" role="alert">
        <h3 ref="errorHeading" tabindex="-1">{{ copy.error }}</h3>
        <p>{{ inputError || state.error }}</p>
      </section>
      <p v-if="state.stale" role="status">{{ copy.stale }}</p>
      <section v-if="state.source" class="field-stack" aria-label="Selected assignment source">
        <p>
          {{ copy.sourceId }} <code class="break-all">{{ state.source.sourceId }}</code> ·
          {{ formatNumber(state.source.byteCount, locale) }} bytes
        </p>
        <p v-if="state.sourceState === 'consumed'" role="status">{{ copy.consumed }}</p>
        <p v-if="state.sourceState === 'uncertain'" role="alert">{{ copy.uncertain }}</p>
        <template v-if="editable">
          <button type="button" :disabled="busy" @click="importer.detect()">
            {{ copy.detect }}
          </button>
          <p v-if="canonical">
            Strict Eutheto assignments v1. Endpoint columns and framing are fixed; logical data
            records begin at record 3.
          </p>
          <ul v-if="state.detection?.format === 'mapped'">
            <template v-for="item in state.detection.dialects" :key="item.dialect"
              ><li v-if="item.status === 'rejected'">
                {{ copy.dialects[item.dialect] }}: <code>{{ item.error.code }}</code
                ><span v-if="item.error.record"> · record {{ item.error.record }}</span>
              </li></template
            >
          </ul>
          <form
            v-if="state.detection"
            class="field-stack"
            @submit.prevent="focusAfter(preview(), () => errorHeading ?? reviewHeading)"
          >
            <fieldset v-if="!canonical" :disabled="busy" class="field-stack">
              <legend>Map the two assignment endpoints</legend>
              <label
                >{{ copy.dialect }}
                <select
                  id="assignment-csv-dialect"
                  :value="dialect"
                  required
                  @change="chooseDialect"
                >
                  <option value="">{{ copy.chooseDialect }}</option>
                  <template v-if="state.detection.format === 'mapped'"
                    ><template v-for="item in state.detection.dialects" :key="item.dialect"
                      ><option v-if="item.status === 'candidate'" :value="item.dialect">
                        {{ copy.dialects[item.dialect] }}
                      </option></template
                    ></template
                  >
                </select></label
              >
              <label
                >{{ copy.header }}
                <select
                  id="assignment-csv-header"
                  v-model="header"
                  required
                  @change="changeInterpretation"
                >
                  <option value="">{{ copy.chooseHeader }}</option>
                  <option value="yes">{{ copy.yes }}</option>
                  <option value="no">{{ copy.no }}</option>
                </select></label
              >
              <label
                >{{ copy.width }}
                <input
                  id="assignment-csv-width"
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
                    ? copy.inconsistentWidth
                    : `${copy.observedWidth} ${candidate.consistentColumns}`
                }}
              </p>
              <label
                >Person column
                <select
                  id="assignment-csv-person-column"
                  v-model="personColumn"
                  required
                  @change="changeInterpretation"
                >
                  <option value="">Choose a column</option>
                  <option v-for="column in columnCount" :key="column" :value="String(column - 1)">
                    Column {{ column
                    }}<template v-if="header === 'yes' && candidate?.status === 'candidate'">
                      · {{ candidate.samples[0]?.cells[column - 1]?.text }}</template
                    >
                  </option>
                </select></label
              >
              <label
                >Shift column
                <select
                  id="assignment-csv-shift-column"
                  v-model="shiftColumn"
                  required
                  @change="changeInterpretation"
                >
                  <option value="">Choose a column</option>
                  <option v-for="column in columnCount" :key="column" :value="String(column - 1)">
                    Column {{ column
                    }}<template v-if="header === 'yes' && candidate?.status === 'candidate'">
                      · {{ candidate.samples[0]?.cells[column - 1]?.text }}</template
                    >
                  </option>
                </select></label
              >
              <p>
                Other columns are ignored. The browser does not parse the source or infer identities
                from names.
              </p>
              <AssignmentCsvMappings
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                :person-mappings="personMappings"
                :shift-mappings="shiftMappings"
                :disabled="busy"
                @update:person-mappings="
                  personMappings = $event;
                  invalidate();
                "
                @update:shift-mappings="
                  shiftMappings = $event;
                  invalidate();
                "
              />
            </fieldset>
            <fieldset :disabled="busy" class="field-stack">
              <legend>Complete starting-plan destination</legend>
              <label
                >Import action
                <select
                  id="assignment-csv-target-kind"
                  v-model="targetKind"
                  required
                  @change="invalidate"
                >
                  <option value="">Choose explicitly</option>
                  <option value="create">Create a new named starting plan</option>
                  <option value="replace">Replace one existing starting plan completely</option>
                </select></label
              >
              <WorkforceEntityPicker
                v-if="targetKind === 'replace'"
                kind="externalStartingPlan"
                label="Starting plan to replace"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                :model-value="selectedPlan"
                :disabled="busy"
                @update:model-value="
                  selectedPlan = $event;
                  invalidate();
                "
              />
              <label
                >Proposed starting-plan name
                <input
                  id="assignment-csv-name"
                  v-model="name"
                  required
                  maxlength="256"
                  @input="invalidate"
              /></label>
              <p v-if="targetKind === 'replace'">
                Replacement removes every existing pair absent from this complete import. It also
                replaces the original-import fingerprints; pair-only edits later preserve those
                fingerprints.
              </p>
              <label
                ><input
                  id="assignment-csv-allow-empty"
                  v-model="allowEmpty"
                  type="checkbox"
                  @change="invalidate"
                />
                I explicitly allow an empty starting plan, including after all records are
                skipped.</label
              >
            </fieldset>
            <p v-if="state.reportId">
              Reviewing again replaces the previous row report. Save it first if needed.
            </p>
            <button type="submit" :disabled="busy">{{ copy.preview }}</button>
          </form>
          <section v-if="readFormat" class="field-stack" aria-label="Inspect one native CSV record">
            <label
              >Logical record to inspect
              <input v-model="inspectRecord" inputmode="numeric" pattern="[0-9]+" maxlength="5"
            /></label>
            <button
              type="button"
              :disabled="busy || !/^[0-9]+$/.test(inspectRecord)"
              @click="inspect(Number(inspectRecord))"
            >
              Inspect selected record
            </button>
            <p v-if="state.sample.status === 'loading'" role="status">Reading selected record…</p>
            <p v-if="state.sample.status === 'error'" role="alert">{{ state.sample.message }}</p>
            <p
              v-if="state.sample.status === 'ready' && state.sample.sample.cells === null"
              role="status"
            >
              No such logical record in this source.
            </p>
            <div
              v-if="state.sample.status === 'ready' && state.sample.sample.cells !== null"
              class="overflow-x-auto focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
              role="region"
              aria-label="Selected native CSV record"
              tabindex="0"
            >
              <table class="w-full text-left">
                <caption>
                  Selected native record
                  {{
                    state.sample.sample.record
                  }}
                </caption>
                <thead>
                  <tr>
                    <th scope="col">Column</th>
                    <th scope="col">Inert source text</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="(cell, index) in state.sample.sample.cells" :key="index">
                    <td>{{ index + 1 }}</td>
                    <td>
                      <code class="break-all">{{ cell.text }}</code
                      ><span v-if="cell.truncated"> (truncated sample)</span>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </section>
        </template>
      </section>
      <section
        v-if="lastRows.length && editable"
        class="field-stack"
        aria-label="Explicit record decisions"
      >
        <h3>Record decisions</h3>
        <p>
          {{
            state.preview
              ? "Native findings from this review."
              : "Previous findings only. Review again after changing any decision."
          }}
          Every unskipped invalid or duplicate pair blocks the whole import; no winner is chosen
          automatically.
        </p>
        <p>
          {{ skippedRecords.length }} records explicitly skipped. Skips still count toward source
          limits.
        </p>
        <button type="button" :disabled="busy" @click="skipAll">Skip all data records</button>
        <button
          type="button"
          :disabled="busy || skippedRecords.length === 0"
          @click="
            skippedRecords = [];
            invalidate();
          "
        >
          Clear all skip decisions
        </button>
        <div
          class="overflow-x-auto focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
          role="region"
          aria-label="Assignment data records"
          tabindex="0"
        >
          <table class="w-full text-left">
            <caption>
              Assignment data records
            </caption>
            <thead>
              <tr>
                <th scope="col">Record</th>
                <th scope="col">Native status</th>
                <th scope="col">Person identity</th>
                <th scope="col">Shift identity</th>
                <th scope="col">Decision</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in lastRows.slice(rowPage * 50, (rowPage + 1) * 50)" :key="row.record">
                <td>{{ row.record }}</td>
                <td>
                  {{ row.status }}<code v-if="row.issue"> · {{ row.issue }}</code>
                </td>
                <td>
                  <code class="break-all">{{ row.personId ?? "Unresolved" }}</code>
                </td>
                <td>
                  <code class="break-all">{{ row.shiftId ?? "Unresolved" }}</code>
                </td>
                <td>
                  <label
                    ><input
                      type="checkbox"
                      :checked="skippedRecords.includes(row.record)"
                      :disabled="busy"
                      @change="skip(row.record, ($event.target as HTMLInputElement).checked)"
                    />
                    Skip record {{ row.record }}</label
                  ><button type="button" :disabled="busy" @click="inspect(row.record)">
                    Inspect record {{ row.record }}
                  </button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <nav v-if="lastRows.length > 50" aria-label="Assignment record pages">
          <button type="button" :disabled="rowPage === 0" @click="rowPage -= 1">
            Previous records</button
          ><button
            type="button"
            :disabled="(rowPage + 1) * 50 >= lastRows.length"
            @click="rowPage += 1"
          >
            Next records
          </button>
        </nav>
      </section>
      <section
        v-if="state.preview"
        class="state-panel field-stack"
        aria-labelledby="assignment-csv-review"
        data-assignment-csv-review
      >
        <h3 id="assignment-csv-review" ref="reviewHeading" tabindex="-1">
          Review complete unverified starting plan
        </h3>
        <p>
          {{
            state.preview.preview.disposition === "blocked"
              ? "Blocked — resolve or explicitly skip the affected records."
              : copy.dispositions[state.preview.preview.disposition]
          }}
        </p>
        <p>
          Revision {{ state.preview.revision }}. This review checks import integrity, not schedule
          feasibility or acceptance.
        </p>
        <template v-if="state.preview.preview.review"
          ><p>
            {{ state.preview.preview.review.target.kind }} ·
            {{ state.preview.preview.review.target.name }} ·
            <code class="break-all">{{ state.preview.preview.review.target.id }}</code>
          </p>
          <p class="break-all">
            Original source fingerprint: <code>{{ state.preview.preview.source.blake3 }}</code
            >. Descriptive only; never acceptance authority.
          </p></template
        >
        <template v-if="state.preview.preview.changes"
          ><p>
            {{ state.preview.preview.changes.beforePairCount }} existing pairs →
            {{ state.preview.preview.changes.afterPairCount }} complete imported pairs.
          </p>
          <p v-if="state.preview.preview.changes.previousName !== null">
            Previous name: {{ state.preview.preview.changes.previousName }}
          </p>
          <p v-if="state.preview.preview.changes.provenanceChanged">
            Original-import provenance will be replaced.
          </p>
          <p v-if="state.preview.preview.changes.afterPairCount === 0">
            Explicit empty starting plan — not a missing source.
          </p></template
        >
        <div
          v-if="changes.length"
          class="overflow-x-auto focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
          role="region"
          aria-label="Complete assignment pair changes"
          tabindex="0"
        >
          <table class="w-full text-left">
            <caption>
              Complete additions and removals, paged without filtering command scope
            </caption>
            <thead>
              <tr>
                <th scope="col">Change</th>
                <th scope="col">Person identity</th>
                <th scope="col">Shift identity</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="pair in changes.slice(changePage * 50, (changePage + 1) * 50)"
                :key="`${pair.change}:${pair.personId}:${pair.shiftId}`"
              >
                <td>{{ pair.change }}</td>
                <td>
                  <code class="break-all">{{ pair.personId }}</code>
                </td>
                <td>
                  <code class="break-all">{{ pair.shiftId }}</code>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <nav v-if="changes.length > 50" aria-label="Pair change pages">
          <button type="button" :disabled="changePage === 0" @click="changePage -= 1">
            Previous pair changes</button
          ><button
            type="button"
            :disabled="(changePage + 1) * 50 >= changes.length"
            @click="changePage += 1"
          >
            Next pair changes
          </button>
        </nav>
        <ValidationSummary
          :findings="findings"
          state="ready"
          interaction="static"
          :heading="copy.validation"
          :heading-level="4"
          :locale="locale"
        />
        <nav
          v-if="state.preview.preview.validationIssues.length > 50"
          aria-label="Validation pages"
        >
          <button type="button" :disabled="validationPage === 0" @click="validationPage -= 1">
            Previous findings</button
          ><button
            type="button"
            :disabled="(validationPage + 1) * 50 >= state.preview.preview.validationIssues.length"
            @click="validationPage += 1"
          >
            Next findings
          </button>
        </nav>
        <template v-if="state.approval"
          ><p class="break-all">
            Command identity: <code>{{ state.approval.commandId }}</code>
          </p>
          <p v-if="state.redoRequired" role="alert">{{ copy.redo }}</p>
          <button
            type="button"
            :disabled="busy || home.state.mutation?.outcome === 'outcomeUnknown'"
            @click="
              focusAfter(
                importer.apply(state.redoRequired),
                () => errorHeading ?? reportHeading ?? reviewHeading,
              )
            "
          >
            {{ state.redoRequired ? copy.confirmRedo : copy.apply }}
          </button></template
        >
      </section>
      <section
        v-if="state.reportId"
        class="state-panel field-stack"
        aria-labelledby="assignment-csv-report"
      >
        <h3 id="assignment-csv-report" ref="reportHeading" tabindex="-1">
          Complete native exclusion report
        </h3>
        <p>
          Every omitted data record is retained, including explicit skips: up to 10,000 rows and 1
          MiB. No raw source cells are included. This report is ephemeral; save it before replacing
          the source or leaving.
        </p>
        <button
          type="button"
          :disabled="busy"
          @click="focusAfter(importer.readReport(), () => errorHeading ?? reportHeading)"
        >
          Read exclusion report</button
        ><button type="button" :disabled="busy" @click="importer.saveReport()">
          Save exclusion report
        </button>
        <template v-if="state.report"
          ><p>{{ state.report.consumed ? copy.reportConsumed : copy.reportUnconsumed }}</p>
          <p v-if="excluded.length === 0">No excluded records.</p>
          <div
            v-else
            class="overflow-x-auto focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
            role="region"
            aria-label="Excluded assignment records"
            tabindex="0"
          >
            <table class="w-full text-left">
              <caption>
                All excluded assignment records
              </caption>
              <thead>
                <tr>
                  <th scope="col">Record</th>
                  <th scope="col">Explicitly skipped</th>
                  <th scope="col">Native issue</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="row in excluded.slice(reportPage * 50, (reportPage + 1) * 50)"
                  :key="row.record"
                >
                  <td>{{ row.record }}</td>
                  <td>{{ row.skipped ? "Yes" : "No" }}</td>
                  <td>
                    <code>{{ row.issue ?? "None" }}</code>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <nav v-if="excluded.length > 50" aria-label="Exclusion report pages">
            <button type="button" :disabled="reportPage === 0" @click="reportPage -= 1">
              Previous exclusions</button
            ><button
              type="button"
              :disabled="(reportPage + 1) * 50 >= excluded.length"
              @click="reportPage += 1"
            >
              Next exclusions
            </button>
          </nav></template
        >
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
