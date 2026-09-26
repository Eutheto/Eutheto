<script setup lang="ts">
import { computed, nextTick, ref, shallowRef, watch } from "vue";
import type { FieldErrorDto, PeopleCsvColumn, PeopleCsvDialect } from "../api/generated";
import { usePeopleCsvImport } from "../people-csv-import";
import {
  createPersonFieldsDraft,
  personFieldsValue,
  type PersonFieldsDraft,
  type PersonFieldErrors,
} from "../person-fields";
import { createPeopleRecordDraft } from "../people-record-draft";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import { messages, formatNumber } from "../messages";
import PersonFields from "./PersonFields.vue";
import PeopleRecordFields from "./PeopleRecordFields.vue";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import ImportMappingTable from "./planner/ImportMappingTable.vue";
import PeopleCsvDecisions from "./planner/PeopleCsvDecisions.vue";
import PeopleCsvReferences from "./planner/PeopleCsvReferences.vue";
import ValidationSummary from "./explanations/ValidationSummary.vue";
import { mappingRows, validMappingWidth } from "./planner/import-mapping";
import { csvDecisionValues, type CsvDecisionDraft } from "./planner/people-csv-decisions";
import { csvReferenceValues, type CsvReferenceDraft } from "./planner/people-csv-references";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  locale?: string;
}>();
const copy = messages.csvImport;
const importer = usePeopleCsvImport(props.home, () => props.project);
const { state } = importer;
const dialect = ref<PeopleCsvDialect | "">("");
const header = ref<"" | "yes" | "no">("");
const width = ref("");
const columns = shallowRef<readonly PeopleCsvColumn[]>([]);
const defaults = shallowRef(createPersonFieldsDraft());
const references = shallowRef<readonly CsvReferenceDraft[]>([]);
const decisions = shallowRef<readonly CsvDecisionDraft[]>([]);
const interpretation = ref(0);
const announcement = ref("");
const inputError = ref("");
const defaultErrors = shallowRef<PersonFieldErrors>({});
const referenceErrors = shallowRef<Readonly<Record<string, string>>>({});
const decisionErrors = shallowRef<readonly { record: number; message: string }[]>([]);
const form = ref<HTMLFormElement>();
const decisionFields = ref<InstanceType<typeof PeopleCsvDecisions>>();
const heading = ref<HTMLElement>();
const reviewHeading = ref<HTMLElement>();
const errorHeading = ref<HTMLElement>();
const reportHeading = ref<HTMLElement>();
const proposalPage = ref(0);
const selectedPerson = ref("");
const validationPage = ref(0);
const reportPage = ref(0);
const busy = computed(() => state.pending || props.home.state.busyAction !== null);
const editable = computed(() => state.source !== null && state.sourceState === "ready");
const fieldContext = computed(() => ({
  project: props.project,
  libraryEpoch: props.home.state.libraryEpoch,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
const candidate = computed(() =>
  state.detection?.dialects.find(
    (item) => item.status === "candidate" && item.dialect === dialect.value,
  ),
);
const expectedColumns = computed(() => (/^[0-9]+$/.test(width.value) ? Number(width.value) : 0));
const nativeErrors = computed(() => {
  const error = state.failure;
  if (
    typeof error !== "object" ||
    error === null ||
    !("fieldErrors" in error) ||
    !Array.isArray(error.fieldErrors)
  )
    return [];
  return error.fieldErrors.filter(
    (field: unknown): field is FieldErrorDto =>
      typeof field === "object" &&
      field !== null &&
      "field" in field &&
      typeof field.field === "string" &&
      "code" in field &&
      typeof field.code === "string" &&
      "message" in field &&
      typeof field.message === "string",
  );
});
const rowErrors = computed(() => [
  ...decisionErrors.value,
  ...nativeErrors.value.flatMap((error) => {
    const match = /^\/peopleCsv\/records\/([1-9][0-9]{0,4})(?:\/|$)/.exec(error.field);
    return match ? [{ record: Number(match[1]), message: error.message }] : [];
  }),
]);
const nativeInvalidMapping = computed(() =>
  nativeErrors.value.some((error) => error.field === "/peopleCsv/mapping"),
);
const proposals = computed(() =>
  state.proposed.slice(proposalPage.value * 50, (proposalPage.value + 1) * 50),
);
const proposedRaw = computed(() => {
  const person = state.proposed.find((item) => item.person.id === selectedPerson.value)?.person;
  return person === undefined ? null : createPeopleRecordDraft(person);
});
const validationCounts = computed(() => {
  const counts = { errors: 0, warnings: 0, information: 0 };
  for (const issue of state.preview?.preview.validationIssues ?? []) {
    if (issue.severity === "error") counts.errors += 1;
    else if (issue.severity === "warning") counts.warnings += 1;
    else counts.information += 1;
  }
  return counts;
});
const findings = computed(() => {
  const all = state.preview?.preview.validationIssues ?? [];
  const issues = all.slice(validationPage.value * 50, (validationPage.value + 1) * 50);
  return { counts: validationCounts.value, issues, omitted: all.length - issues.length };
});
const rejected = computed(
  () => state.report?.rejectedRows.slice(reportPage.value * 50, (reportPage.value + 1) * 50) ?? [],
);

function clearFeedback(): void {
  inputError.value = "";
  defaultErrors.value = {};
  referenceErrors.value = {};
  decisionErrors.value = [];
}
function invalidateInterpretation(): void {
  importer.invalidate();
  importer.invalidateSample();
  decisions.value = [];
  interpretation.value += 1;
  clearFeedback();
  announcement.value = copy.interpretationChanged;
}
function changeDialect(event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  const selected = state.detection?.dialects.find(
    (item) => item.status === "candidate" && item.dialect === value,
  );
  if (value !== "" && selected === undefined) return;
  dialect.value = selected?.dialect ?? "";
  invalidateInterpretation();
}
function changeHeader(event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  if (value !== "" && value !== "yes" && value !== "no") return;
  header.value = value;
  invalidateInterpretation();
}
function changeWidth(event: Event): void {
  width.value = (event.target as HTMLInputElement).value;
  invalidateInterpretation();
}
function changeColumns(value: readonly PeopleCsvColumn[]): void {
  columns.value = value;
  invalidateInterpretation();
}
function changeDefaults(value: PersonFieldsDraft): void {
  defaults.value = value;
  invalidateInterpretation();
}
function changeReferences(value: readonly CsvReferenceDraft[]): void {
  references.value = value;
  invalidateInterpretation();
}
function changeDecisions(value: readonly CsvDecisionDraft[]): void {
  decisions.value = value;
  importer.invalidate();
  clearFeedback();
  announcement.value = copy.decisionChanged;
}
async function choose(): Promise<void> {
  if (busy.value || props.home.state.reviewCleanupError !== null) return;
  dialect.value = "";
  header.value = "";
  width.value = "";
  columns.value = [];
  defaults.value = createPersonFieldsDraft();
  references.value = [];
  invalidateInterpretation();
  announcement.value = "";
  if (await importer.choose()) await importer.detect();
  await nextTick();
  (state.error ? errorHeading.value : heading.value)?.focus();
}
async function inspect(record: number): Promise<void> {
  if (dialect.value !== "") await importer.inspect(dialect.value, record);
}
function revealInvalidField(event: Event): void {
  const target = event.target;
  if (!(target instanceof HTMLElement) || !form.value?.contains(target)) return;
  const detail = target.closest("details");
  if (detail && form.value.contains(detail)) detail.open = true;
}
async function preview(): Promise<void> {
  if (busy.value || !editable.value) return;
  clearFeedback();
  const convertedDefaults = personFieldsValue(defaults.value);
  const convertedReferences = csvReferenceValues(references.value);
  const convertedDecisions = csvDecisionValues(decisions.value);
  defaultErrors.value = convertedDefaults.errors;
  referenceErrors.value = convertedReferences.errors;
  decisionErrors.value = convertedDecisions.errors;
  const mappingInvalid =
    !validMappingWidth(expectedColumns.value) ||
    columns.value.length === 0 ||
    mappingRows(columns.value, expectedColumns.value).some((row) => row.errors.length !== 0);
  const submittedForm = form.value;
  if (!submittedForm?.reportValidity()) return;
  if (
    dialect.value === "" ||
    header.value === "" ||
    mappingInvalid ||
    convertedDefaults.value === null ||
    convertedReferences.value === null ||
    convertedDecisions.decisions === null
  ) {
    inputError.value = copy.invalidDraft;
    await nextTick();
    if (submittedForm.isConnected) {
      const invalid = submittedForm.querySelector<HTMLElement>('[aria-invalid="true"]');
      const detail = invalid?.closest("details");
      if (detail && submittedForm.contains(detail)) detail.open = true;
      (invalid ?? errorHeading.value)?.focus();
    }
    return;
  }
  const accepted = await importer.preview(
    {
      dialect: dialect.value,
      hasHeader: header.value === "yes",
      expectedColumns: expectedColumns.value,
      columns: columns.value,
      newPersonDefaults: convertedDefaults.value,
      referenceMappings: convertedReferences.value,
    },
    convertedDecisions.decisions,
  );
  if (accepted) announcement.value = "";
  await nextTick();
  if (accepted) reviewHeading.value?.focus();
  else if (rowErrors.value[0] && decisionFields.value)
    await decisionFields.value.focusRecord(rowErrors.value[0].record);
  else if (nativeInvalidMapping.value) document.getElementById("csv-mapping")?.focus();
  else errorHeading.value?.focus();
}
async function apply(truncateRedo = false): Promise<void> {
  const result = await importer.apply(truncateRedo);
  await nextTick();
  (result === null ? (errorHeading.value ?? reviewHeading.value) : reportHeading.value)?.focus();
}
async function readReport(): Promise<void> {
  if (await importer.readReport()) {
    await nextTick();
    reportHeading.value?.focus();
  }
}
function discard(): void {
  importer.invalidate();
  importer.invalidateSample();
}
watch(
  () => state.preview,
  () => {
    proposalPage.value = 0;
    selectedPerson.value = "";
    validationPage.value = 0;
  },
);
watch(
  () => state.report,
  () => {
    reportPage.value = 0;
  },
);
watch(
  () => state.stale,
  (stale) => {
    const active = document.activeElement;
    if (!stale || !(active instanceof HTMLElement) || active.closest("[data-csv-review]") === null)
      return;
    void nextTick().then(() => {
      if (
        state.stale &&
        (document.activeElement === active || document.activeElement === document.body)
      )
        heading.value?.focus();
    });
  },
  { flush: "sync" },
);
</script>

<template>
  <section class="page-stack" aria-labelledby="csv-import-heading">
    <h2 id="csv-import-heading" ref="heading" data-route-heading tabindex="-1">
      {{ copy.heading }}
    </h2>
    <p>{{ copy.description }}</p>
    <RouterLink :to="{ name: 'project-people', params: { scenarioId: project.scenarioId } }">{{
      copy.back
    }}</RouterLink>
    <p v-if="project.domainPackId !== 'official.workforce'">{{ messages.setup.unsupported }}</p>
    <template v-else>
      <p>{{ copy.encoding }}</p>
      <p>{{ copy.limits }}</p>
      <p v-if="state.source">{{ copy.replaceHelp }}</p>
      <button
        type="button"
        :disabled="busy || home.state.reviewCleanupError !== null"
        @click="choose"
      >
        {{ state.source ? copy.replace : copy.choose }}
      </button>
      <div v-if="inputError || state.error" class="state-panel" role="alert">
        <h3 ref="errorHeading" tabindex="-1">{{ copy.error }}</h3>
        <p>{{ inputError || state.error }}</p>
        <ul>
          <li v-for="(error, index) in nativeErrors.slice(0, 50)" :key="index">
            {{ error.message }} <code>{{ error.field }}</code>
          </li>
        </ul>
      </div>
      <p role="status">{{ state.stale ? copy.stale : announcement }}</p>
      <section v-if="state.source" class="field-stack" :aria-label="copy.source">
        <h3>{{ copy.source }}</h3>
        <p class="break-all">
          {{ copy.sourceId }} <code>{{ state.source.sourceId }}</code>
        </p>
        <p>{{ copy.bytes }} {{ formatNumber(state.source.byteCount, locale) }}</p>
        <p v-if="state.sourceState === 'consumed'" role="status">{{ copy.consumed }}</p>
        <p v-if="state.sourceState === 'uncertain'" role="alert">{{ copy.uncertain }}</p>
        <template v-if="editable">
          <button type="button" :disabled="busy" @click="importer.detect()">
            {{ copy.detect }}
          </button>
          <ul v-if="state.detection">
            <template v-for="item in state.detection.dialects" :key="item.dialect">
              <li v-if="item.status === 'rejected'">
                {{ copy.rejectedDialect }} {{ copy.dialects[item.dialect] }} —
                <code>{{ item.error.code }}</code
                ><span v-if="item.error.record">
                  · {{ copy.record }} {{ formatNumber(item.error.record, locale) }}</span
                >
              </li>
            </template>
          </ul>
          <form
            v-if="state.detection"
            ref="form"
            class="field-stack"
            @invalid.capture="revealInvalidField"
            @submit.prevent="preview"
          >
            <fieldset :disabled="busy" class="field-stack">
              <legend>{{ copy.mapping }}</legend>
              <label
                >{{ copy.dialect
                }}<select id="csv-dialect" :value="dialect" required @change="changeDialect">
                  <option value="">{{ copy.chooseDialect }}</option>
                  <template v-for="item in state.detection.dialects" :key="item.dialect"
                    ><option v-if="item.status === 'candidate'" :value="item.dialect">
                      {{ copy.dialects[item.dialect] }}
                    </option></template
                  >
                </select></label
              >
              <label
                >{{ copy.header
                }}<select id="csv-header" :value="header" required @change="changeHeader">
                  <option value="">{{ copy.chooseHeader }}</option>
                  <option value="yes">{{ copy.yes }}</option>
                  <option value="no">{{ copy.no }}</option>
                </select></label
              >
              <label
                >{{ copy.width
                }}<input
                  id="csv-width"
                  :value="width"
                  required
                  inputmode="numeric"
                  pattern="[0-9]+"
                  maxlength="2"
                  :aria-invalid="
                    inputError !== '' && !validMappingWidth(expectedColumns) ? true : undefined
                  "
                  aria-describedby="csv-width-help"
                  @input="changeWidth"
              /></label>
              <p id="csv-width-help">{{ copy.widthHelp }}</p>
              <template v-if="candidate?.status === 'candidate'"
                ><p v-if="candidate.consistentColumns !== null">
                  {{ copy.observedWidth }} {{ formatNumber(candidate.consistentColumns, locale) }}
                </p>
                <p v-else>{{ copy.inconsistentWidth }}</p></template
              >
              <ImportMappingTable
                id="csv-mapping"
                :model-value="columns"
                :expected-columns="expectedColumns"
                :label="copy.mapping"
                :disabled="busy"
                :native-invalid-mapping="nativeInvalidMapping"
                :header-cells="
                  header === 'yes' && candidate?.status === 'candidate'
                    ? (candidate.samples[0]?.cells ?? [])
                    : []
                "
                :selected-samples="candidate?.status === 'candidate' ? candidate.samples : []"
                @update:model-value="changeColumns"
              />
              <PeopleCsvReferences
                v-bind="fieldContext"
                :model-value="references"
                :disabled="busy"
                :errors="referenceErrors"
                @update:model-value="changeReferences"
              />
              <h3>{{ copy.defaults }}</h3>
              <p>{{ copy.defaultsHelp }}</p>
              <PersonFields
                v-bind="fieldContext"
                :model-value="defaults"
                :disabled="busy"
                :errors="defaultErrors"
                @update:model-value="changeDefaults"
              />
            </fieldset>
            <PeopleCsvDecisions
              :key="interpretation"
              ref="decisionFields"
              v-bind="fieldContext"
              :model-value="decisions"
              :rows="state.preview?.preview.rows ?? []"
              :sample="state.sample"
              :disabled="busy || dialect === ''"
              :errors="rowErrors"
              @update:model-value="changeDecisions"
              @inspect="inspect"
            />
            <p v-if="state.reportId">{{ copy.replaceReport }}</p>
            <button type="submit" :disabled="busy || dialect === '' || header === ''">
              {{ copy.preview }}
            </button>
          </form>
        </template>
      </section>
      <section
        v-if="state.preview"
        data-csv-review
        class="state-panel field-stack"
        aria-labelledby="csv-review-heading"
      >
        <h3 id="csv-review-heading" ref="reviewHeading" tabindex="-1">{{ copy.review }}</h3>
        <p>{{ copy.dispositions[state.preview.preview.disposition] }}</p>
        <p>{{ copy.previewHelp }}</p>
        <p>
          {{ messages.people.reviewRevision }} {{ formatNumber(state.preview.revision, locale) }}
        </p>
        <p v-if="state.approval" class="break-all">
          {{ messages.people.commandIdentity }} <code>{{ state.approval.commandId }}</code>
        </p>
        <p>{{ messages.people.localActorHelp }}</p>
        <section v-if="state.proposed.length" class="field-stack" :aria-label="copy.proposed">
          <label
            >{{ copy.proposed
            }}<select v-model="selectedPerson">
              <option value="">{{ copy.chooseProposed }}</option>
              <option v-for="item in proposals" :key="item.person.id" :value="item.person.id">
                {{ formatNumber(item.record, locale) }} · {{ item.person.name }} ·
                {{ item.person.id }}
              </option>
            </select></label
          >
          <div class="action-row">
            <button
              type="button"
              :disabled="proposalPage === 0"
              @click="
                proposalPage -= 1;
                selectedPerson = '';
              "
            >
              {{ messages.people.previous }}</button
            ><button
              type="button"
              :disabled="(proposalPage + 1) * 50 >= state.proposed.length"
              @click="
                proposalPage += 1;
                selectedPerson = '';
              "
            >
              {{ messages.people.next }}
            </button>
          </div>
          <PeopleRecordFields
            v-if="proposedRaw"
            v-bind="fieldContext"
            :model-value="proposedRaw"
            read-only
          />
        </section>
        <ValidationSummary
          :findings="findings"
          state="ready"
          interaction="static"
          :heading="copy.validation"
          :heading-level="4"
          :locale="locale"
        />
        <div v-if="state.preview.preview.validationIssues.length > 50" class="action-row">
          <button type="button" :disabled="validationPage === 0" @click="validationPage -= 1">
            {{ messages.people.previous }}</button
          ><button
            type="button"
            :disabled="(validationPage + 1) * 50 >= state.preview.preview.validationIssues.length"
            @click="validationPage += 1"
          >
            {{ messages.people.next }}
          </button>
        </div>
        <template v-if="state.approval">
          <p v-if="state.redoRequired" role="alert">{{ copy.redo }}</p>
          <button
            type="button"
            :disabled="busy || home.state.mutation?.outcome === 'outcomeUnknown'"
            @click="apply(state.redoRequired)"
          >
            {{ state.redoRequired ? copy.confirmRedo : copy.apply }}
          </button>
        </template>
      </section>
      <section
        v-if="state.reportId"
        class="state-panel field-stack"
        aria-labelledby="csv-report-heading"
      >
        <h3 id="csv-report-heading" ref="reportHeading" tabindex="-1">{{ copy.report }}</h3>
        <p>{{ copy.reportHelp }}</p>
        <div class="action-row">
          <button type="button" :disabled="busy" @click="readReport">{{ copy.readReport }}</button
          ><button type="button" :disabled="busy" @click="importer.saveReport()">
            {{ copy.saveReport }}
          </button>
        </div>
        <template v-if="state.report"
          ><p>
            {{ state.report.consumed ? copy.reportConsumed : copy.reportUnconsumed }}
          </p>
          <p v-if="state.report.rejectedRows.length === 0">{{ copy.reportEmpty }}</p>
          <table v-else>
            <caption>
              {{
                copy.report
              }}
            </caption>
            <thead>
              <tr>
                <th scope="col">{{ copy.record }}</th>
                <th scope="col">{{ copy.rejection }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in rejected" :key="row.record">
                <td>{{ formatNumber(row.record, locale) }}</td>
                <td>
                  <code>{{ row.code }}</code>
                </td>
              </tr>
            </tbody>
          </table>
          <div v-if="state.report.rejectedRows.length > 50" class="action-row">
            <button type="button" :disabled="reportPage === 0" @click="reportPage -= 1">
              {{ messages.people.previous }}</button
            ><button
              type="button"
              :disabled="(reportPage + 1) * 50 >= state.report.rejectedRows.length"
              @click="reportPage += 1"
            >
              {{ messages.people.next }}
            </button>
          </div>
        </template>
      </section>
    </template>
    <RouteLeaveGuard
      :home="home"
      :dirty="state.source !== null"
      :pending="state.pending"
      :discard="discard"
    />
  </section>
</template>
