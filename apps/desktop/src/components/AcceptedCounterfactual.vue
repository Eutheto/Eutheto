<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import {
  cancelCounterfactual,
  getCounterfactual,
  getSolutionView,
  getScenarioView,
  listCounterfactuals,
  onCounterfactualProgress,
  SetupOperationScope,
  startCounterfactual,
  type CounterfactualConditionV1,
  type CounterfactualConditionPayloadV1,
  type CounterfactualJobListCursorV1,
  type CounterfactualJobListItemV1,
  type CounterfactualJobRecordV1,
  type AssignmentComparisonV1,
  type ScoreCategoryComparisonV1,
  type DomainAssignment,
  type Revision,
  type SolutionSummaryDtoV1,
  type SolveRunId,
  type UuidV7,
} from "../api/generated";
import {
  isWorkforceResultScheduleViewData,
  WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
  type WorkforceResultScheduleAssignmentRow,
  type WorkforceResultScheduleContinuation,
  type WorkforceResultSchedulePersonRow,
  type WorkforceSetupAssignmentInspection,
} from "../api/generated-domain-pack-contracts";
import { safeMessage, type ProjectHomeController } from "../project-home";
import { messages } from "../messages";
import InfeasibilityInspection from "./InfeasibilityInspection.vue";

interface InspectionSource {
  readonly scenarioId: UuidV7;
  readonly currentRevision: Revision;
  readonly sourceRevision: Revision;
  readonly runId: SolveRunId;
  readonly runManifestChecksum: string;
  readonly condition: CounterfactualConditionV1;
}
type ConflictRuleAction = {
  readonly action: "inspect" | "edit";
  readonly scenarioId: UuidV7;
  readonly sourceRevision: Revision;
  readonly runId: SolveRunId;
  readonly runManifestChecksum: string;
  readonly evidenceChecksum: string;
  readonly conflictId: string;
  readonly ruleId: UuidV7;
};

const props = withDefaults(
  defineProps<{
    scenarioId: UuidV7;
    summary: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision">;
    currentRevision: Revision;
    assignment: WorkforceResultScheduleAssignmentRow;
    home?: ProjectHomeController | undefined;
    ruleNavigationAvailable?: boolean;
  }>(),
  { ruleNavigationAvailable: false, home: undefined },
);
const emit = defineEmits<{ ruleAction: [action: ConflictRuleAction] }>();
const people = shallowRef<readonly WorkforceResultSchedulePersonRow[]>([]);
const nextPeople = shallowRef<WorkforceResultScheduleContinuation | null>(null);
const personId = ref<UuidV7 | null>(null);
const questionMode = ref<"replace" | "require" | "exclude">("replace");
const jobs = shallowRef<readonly CounterfactualJobListItemV1[]>([]);
const nextJobs = shallowRef<CounterfactualJobListCursorV1 | null>(null);
const job = shallowRef<CounterfactualJobRecordV1 | null>(null);
const excludedPair = shallowRef<WorkforceSetupAssignmentInspection | null>(null);
const conflictInspectionOpen = ref(false);
const conflictInspectionTrigger = ref<HTMLButtonElement | null>(null);
const assignmentPage = ref(0);
const categoryPage = ref(0);
const busy = ref(false);
const loadingPeople = ref(false);
const loadingJobs = ref(false);
const error = ref<string | null>(null);
let generation = 0;
let alive = true;
let unlisten: (() => void) | null = null;
let selectedJobId: UuidV7 | null = null;
let jobRead = 0;
let listRead = 0;
let peopleRead = 0;
const matches = (item: CounterfactualJobListItemV1): boolean =>
  item.baseSolutionId === props.summary.result.solutionId;
const isCurrent = (token: number): boolean => alive && token === generation;
const terminal = (state: CounterfactualJobRecordV1["state"]): boolean =>
  state !== "queued" && state !== "running";
const assignmentChanges = computed(() =>
  job.value?.result?.conclusion.type === "verifiedAlternative"
    ? job.value.result.conclusion.comparison.assignments
    : [],
);
const visibleAssignmentChanges = computed(() =>
  assignmentChanges.value.slice(assignmentPage.value * 20, (assignmentPage.value + 1) * 20),
);
const scoreLevels = computed(() =>
  job.value?.result?.conclusion.type === "verifiedAlternative"
    ? job.value.result.conclusion.comparison.scoreLevels
    : [],
);
const categoryCount = computed(() =>
  scoreLevels.value.reduce((total, level) => total + level.categories.length, 0),
);
const visibleCategories = computed(() => {
  const rows: { levelId: string; category: ScoreCategoryComparisonV1 }[] = [];
  const start = categoryPage.value * 20;
  let offset = 0;
  for (const level of scoreLevels.value) {
    for (
      let index = Math.max(0, start - offset);
      index < level.categories.length && rows.length < 20;
      index += 1
    ) {
      const category = level.categories[index];
      if (category !== undefined) rows.push({ levelId: level.levelId, category });
    }
    offset += level.categories.length;
    if (rows.length === 20) break;
  }
  return rows;
});
function conditionText(condition: CounterfactualConditionPayloadV1): string {
  if (condition.type === "replaceAssignment")
    return `Set target assignment ${condition.targetAssignmentId} to Boolean true and replaced assignment ${condition.replacedAssignmentId} to Boolean false`;
  if (condition.value.type === "boolean")
    return `${condition.type === "forceAssignmentValue" ? "Force" : "Forbid"} assignment ${condition.assignmentId} = Boolean ${String(condition.value.value)}`;
  if (condition.value.type === "integer")
    return `${condition.type === "forceAssignmentValue" ? "Force" : "Forbid"} assignment ${condition.assignmentId} = Integer ${condition.value.value}`;
  if (condition.value.type === "absent")
    return `${condition.type === "forceAssignmentValue" ? "Force" : "Forbid"} assignment ${condition.assignmentId} = Absent`;
  return `${condition.type === "forceAssignmentValue" ? "Force" : "Forbid"} assignment ${condition.assignmentId} = Interval ${condition.value.value.start}–${condition.value.value.end} (duration ${condition.value.value.duration})`;
}
function sameCondition(left: CounterfactualConditionV1, right: CounterfactualConditionV1): boolean {
  if (left.schemaVersion !== right.schemaVersion || left.checksum !== right.checksum) return false;
  if (left.condition.type !== right.condition.type) return false;
  if (left.condition.type === "replaceAssignment" && right.condition.type === "replaceAssignment")
    return (
      left.condition.targetAssignmentId === right.condition.targetAssignmentId &&
      left.condition.replacedAssignmentId === right.condition.replacedAssignmentId
    );
  if (
    (left.condition.type === "forceAssignmentValue" ||
      left.condition.type === "forbidAssignmentValue") &&
    left.condition.type === right.condition.type
  ) {
    const a = left.condition.value;
    const b = right.condition.value;
    if (a.type !== b.type || left.condition.assignmentId !== right.condition.assignmentId)
      return false;
    if (a.type === "boolean" && b.type === "boolean") return a.value === b.value;
    if (a.type === "integer" && b.type === "integer") return a.value === b.value;
    if (a.type === "absent" && b.type === "absent") return true;
    return (
      a.type === "interval" &&
      b.type === "interval" &&
      a.value.start === b.value.start &&
      a.value.duration === b.value.duration &&
      a.value.end === b.value.end
    );
  }
  return false;
}

function inspectionSource(record: CounterfactualJobRecordV1 | null): InspectionSource | null {
  const result = record?.result;
  if (record?.state !== "completed" || result === null || result === undefined) return null;
  const semantics = record.request.semantics;
  const input = result.runInput;
  const manifest = result.runManifest;
  if (
    result.request.jobId !== record.request.jobId ||
    result.request.requestHash !== record.request.requestHash ||
    !sameCondition(result.request.condition, record.request.condition) ||
    result.request.semantics.schemaVersion !== semantics.schemaVersion ||
    result.request.semantics.scenarioId !== semantics.scenarioId ||
    result.request.semantics.scenarioRevision !== semantics.scenarioRevision ||
    result.request.semantics.snapshotId !== semantics.snapshotId ||
    result.request.semantics.snapshotDocumentHash !== semantics.snapshotDocumentHash ||
    result.request.semantics.base.solutionId !== semantics.base.solutionId ||
    result.request.semantics.base.resultChecksum !== semantics.base.resultChecksum ||
    result.request.semantics.baseRunId !== semantics.baseRunId ||
    result.request.semantics.baseRunInputChecksum !== semantics.baseRunInputChecksum ||
    result.request.semantics.baseModelHash !== semantics.baseModelHash ||
    result.request.semantics.objectivePolicyHash !== semantics.objectivePolicyHash ||
    result.request.semantics.conditionChecksum !== semantics.conditionChecksum ||
    result.request.semantics.totalBudgetMilliseconds !== semantics.totalBudgetMilliseconds ||
    semantics.scenarioId !== props.scenarioId ||
    semantics.scenarioRevision !== props.summary.scenarioRevision ||
    semantics.base.solutionId !== props.summary.result.solutionId ||
    semantics.base.resultChecksum !== props.summary.result.resultChecksum ||
    semantics.conditionChecksum !== record.request.condition.checksum ||
    result.baseRunInput.snapshotId !== semantics.snapshotId ||
    result.baseRunInput.snapshotDocumentHash !== semantics.snapshotDocumentHash ||
    result.baseRunInput.objectivePolicyHash !== semantics.objectivePolicyHash ||
    result.baseRunManifest.runId !== semantics.baseRunId ||
    result.baseRunManifest.runInputChecksum !== result.baseRunInput.checksum ||
    result.compilation.baseModelHash !== semantics.baseModelHash ||
    result.compilation.conditionChecksum !== record.request.condition.checksum ||
    result.compilation.objectivePolicyHash !== semantics.objectivePolicyHash ||
    result.compilation.derivedModelHash !== input.modelHash ||
    input.snapshotId !== semantics.snapshotId ||
    input.snapshotDocumentHash !== semantics.snapshotDocumentHash ||
    input.objectivePolicyHash !== semantics.objectivePolicyHash ||
    result.baseRunInput.runId !== semantics.baseRunId ||
    result.baseRunInput.checksum !== semantics.baseRunInputChecksum ||
    result.baseRunInput.scenarioId !== props.scenarioId ||
    result.baseRunInput.scenarioRevision !== semantics.scenarioRevision ||
    input.scenarioId !== props.scenarioId ||
    input.scenarioRevision !== semantics.scenarioRevision ||
    input.temporaryConditionHash !== record.request.condition.checksum ||
    manifest.runId !== input.runId ||
    manifest.runInputChecksum !== input.checksum ||
    manifest.outcome.type !== "noResult" ||
    manifest.outcome.status !== "infeasible"
  )
    return null;
  return {
    scenarioId: props.scenarioId,
    currentRevision: props.currentRevision,
    sourceRevision: input.scenarioRevision,
    runId: input.runId,
    runManifestChecksum: manifest.checksum,
    condition: record.request.condition,
  };
}

const diagnosticSource = computed(() => inspectionSource(job.value));
watch(
  () => JSON.stringify(diagnosticSource.value),
  (source, previous) => {
    if (source !== previous) conflictInspectionOpen.value = false;
  },
);

function assignmentText(assignment: DomainAssignment): string {
  const value = assignment.value;
  if (value.type === "boolean")
    return `${assignment.entity.id}: ${value.value ? "assigned" : "not assigned"}`;
  if (value.type === "integer") return `${assignment.entity.id}: ${value.value}`;
  if (value.type === "absent") return `${assignment.entity.id}: not assigned`;
  return `${assignment.entity.id}: ${value.value.start}–${value.value.end}`;
}
function changeText(change: AssignmentComparisonV1): string {
  if (change.type === "added") return `Added ${assignmentText(change.after)}`;
  if (change.type === "removed") return `Removed ${assignmentText(change.before)}`;
  return `Changed ${assignmentText(change.before)} → ${assignmentText(change.after)}`;
}
function rememberJob(record: CounterfactualJobRecordV1): void {
  if (record.request.jobId !== selectedJobId) return;
  if (
    job.value?.request.jobId === record.request.jobId &&
    terminal(job.value.state) &&
    !terminal(record.state)
  )
    return;
  if (job.value?.request.jobId !== record.request.jobId) {
    assignmentPage.value = 0;
    categoryPage.value = 0;
    conflictInspectionOpen.value = false;
  }
  job.value = record;
}

async function loadPeople(continuation: WorkforceResultScheduleContinuation | null): Promise<void> {
  const read = ++peopleRead;
  const token = generation;
  loadingPeople.value = true;
  error.value = null;
  try {
    const { result } = await getSolutionView({
      scenarioId: props.scenarioId,
      solutionId: props.summary.result.solutionId,
      viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
      parameters: { kind: "people", limit: 50 },
      continuation,
    });
    if (!isCurrent(token) || read !== peopleRead) return;
    if (
      result.scenarioId !== props.scenarioId ||
      result.scenarioRevision !== props.summary.scenarioRevision ||
      result.result.resultChecksum !== props.summary.result.resultChecksum ||
      result.view.viewId !== WORKFORCE_RESULT_SCHEDULE_VIEW_ID ||
      !isWorkforceResultScheduleViewData(result.view.data) ||
      result.view.data.result.kind !== "people"
    ) {
      error.value =
        "The saved schedule changed. Reopen this result before asking another question.";
      return;
    }
    const page = result.view.data.result;
    people.value = page.items;
    personId.value = null;
    nextPeople.value = page.continuation;
  } catch (failure) {
    if (isCurrent(token) && read === peopleRead) error.value = safeMessage(failure);
  } finally {
    if (isCurrent(token) && read === peopleRead) loadingPeople.value = false;
  }
}

async function loadJobs(before: CounterfactualJobListCursorV1 | null = null): Promise<void> {
  const token = generation;
  const read = ++listRead;
  loadingJobs.value = true;
  error.value = null;
  try {
    const { result } = await listCounterfactuals({
      scenarioId: props.scenarioId,
      before,
      limit: 8,
    });
    if (!isCurrent(token) || read !== listRead) return;
    if (result.scenarioId !== props.scenarioId) {
      error.value = "The diagnostic history belongs to a different scenario.";
      return;
    }
    const relevant = result.page.jobs.filter(matches);
    jobs.value = relevant;
    nextJobs.value = result.page.nextCursor;
  } catch (failure) {
    if (isCurrent(token) && read === listRead) error.value = safeMessage(failure);
  } finally {
    if (isCurrent(token) && read === listRead) loadingJobs.value = false;
  }
}

async function showJob(jobId: UuidV7): Promise<void> {
  const token = generation;
  const read = ++jobRead;
  if (selectedJobId !== jobId) job.value = null;
  selectedJobId = jobId;
  busy.value = true;
  error.value = null;
  try {
    const response = await getCounterfactual({ scenarioId: props.scenarioId, jobId });
    if (!isCurrent(token) || read !== jobRead) return;
    const { result } = response;
    if (
      response.currentRevision !== props.currentRevision ||
      result.scenarioId !== props.scenarioId ||
      result.job.request.jobId !== jobId ||
      result.job.request.semantics.scenarioId !== props.scenarioId ||
      result.job.request.semantics.base.solutionId !== props.summary.result.solutionId ||
      result.job.request.semantics.base.resultChecksum !== props.summary.result.resultChecksum ||
      result.job.request.semantics.scenarioRevision !== props.summary.scenarioRevision ||
      result.job.request.semantics.conditionChecksum !== result.job.request.condition.checksum
    ) {
      error.value = "This saved diagnostic does not belong to the inspected result.";
      return;
    }
    rememberJob(result.job);
  } catch (failure) {
    if (isCurrent(token) && read === jobRead) error.value = safeMessage(failure);
  } finally {
    if (isCurrent(token) && read === jobRead) busy.value = false;
  }
}

async function start(): Promise<void> {
  const mode = questionMode.value;
  const targetPersonId = mode === "exclude" ? props.assignment.personId : personId.value;
  if (
    targetPersonId === null ||
    (mode !== "exclude" &&
      (targetPersonId === props.assignment.personId ||
        !people.value.some((person) => person.personId === targetPersonId)))
  )
    return;
  const token = generation;
  busy.value = true;
  jobRead += 1;
  selectedJobId = null;
  job.value = null;
  error.value = null;
  excludedPair.value = null;
  try {
    const source = (
      await getSolutionView({
        scenarioId: props.scenarioId,
        solutionId: props.summary.result.solutionId,
        viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
        parameters: {
          kind: "decision",
          personId: targetPersonId,
          shiftId: props.assignment.shiftId,
        },
      })
    ).result;
    if (!isCurrent(token)) return;
    if (
      source.scenarioId !== props.scenarioId ||
      source.scenarioRevision !== props.summary.scenarioRevision ||
      source.result.solutionId !== props.summary.result.solutionId ||
      source.result.resultChecksum !== props.summary.result.resultChecksum ||
      source.view.viewId !== WORKFORCE_RESULT_SCHEDULE_VIEW_ID ||
      !isWorkforceResultScheduleViewData(source.view.data) ||
      source.view.data.result.kind !== "decision" ||
      source.view.data.result.personId !== targetPersonId ||
      source.view.data.result.shiftId !== props.assignment.shiftId
    )
      throw new Error("The saved decision does not match this person, shift, or accepted source.");
    const decision = source.view.data.result;
    if (
      mode === "exclude" &&
      (decision.selected !== true || decision.assignmentId !== props.assignment.assignmentId)
    ) {
      error.value = "The saved incumbent decision does not match this assignment.";
      return;
    }
    if (mode !== "exclude" && decision.selected === true) {
      error.value = "This person is already assigned to this shift in the saved result.";
      return;
    }
    if (decision.assignmentId === null) {
      const scope = new SetupOperationScope(props.scenarioId, props.summary.scenarioRevision);
      let inspection: WorkforceSetupAssignmentInspection;
      try {
        const response = await getScenarioView(scope, {
          source: { kind: "stored" },
          query: {
            schemaVersion: 1,
            viewId: "official.workforce.setup.assignment_inspection",
            parameters: {
              personId: targetPersonId,
              shiftId: props.assignment.shiftId,
              limit: 20,
            },
          },
        }).result;
        inspection = response.result.view.data.result.data;
      } finally {
        scope.dispose();
      }
      if (!isCurrent(token) || personId.value !== targetPersonId) return;
      if (inspection.personId !== targetPersonId || inspection.shiftId !== props.assignment.shiftId)
        throw new Error("The native check did not match this person and shift.");
      if (!inspection.candidateInImplementedAssignmentGraph) {
        excludedPair.value = inspection;
        return;
      }
      throw new Error("The saved result has no supported decision for this person and shift.");
    }
    const condition: CounterfactualConditionPayloadV1 =
      mode === "replace"
        ? {
            type: "replaceAssignment",
            targetAssignmentId: decision.assignmentId,
            replacedAssignmentId: props.assignment.assignmentId,
          }
        : {
            type: mode === "require" ? "forceAssignmentValue" : "forbidAssignmentValue",
            assignmentId: decision.assignmentId,
            value: { type: "boolean", value: true },
          };
    const { result } = await startCounterfactual({
      scenarioId: props.scenarioId,
      expectedRevision: props.summary.scenarioRevision,
      baseSolutionId: props.summary.result.solutionId,
      condition,
      totalBudgetMilliseconds: 30_000,
    });
    if (!isCurrent(token)) return;
    selectedJobId = result.job.request.jobId;
    rememberJob(result.job);
    await loadJobs();
    if (isCurrent(token) && selectedJobId === result.job.request.jobId)
      await showJob(result.job.request.jobId);
  } catch (failure) {
    if (isCurrent(token)) error.value = safeMessage(failure);
  } finally {
    if (isCurrent(token)) busy.value = false;
  }
}

async function cancel(): Promise<void> {
  const current = job.value;
  if (current === null) return;
  const token = generation;
  busy.value = true;
  error.value = null;
  try {
    const { result } = await cancelCounterfactual({
      scenarioId: props.scenarioId,
      expectedRevision: props.summary.scenarioRevision,
      jobId: current.request.jobId,
    });
    if (isCurrent(token)) {
      rememberJob(result.job);
      await loadJobs();
    }
  } catch (failure) {
    if (isCurrent(token)) error.value = safeMessage(failure);
  } finally {
    if (isCurrent(token)) busy.value = false;
  }
}

async function closeConflictInspection(): Promise<void> {
  const jobId = job.value?.request.jobId ?? null;
  conflictInspectionOpen.value = false;
  await nextTick();
  const trigger = conflictInspectionTrigger.value;
  if (
    alive &&
    jobId !== null &&
    job.value?.request.jobId === jobId &&
    document.activeElement === document.body &&
    trigger?.isConnected
  )
    trigger.focus();
}

watch(
  [
    () => props.scenarioId,
    () => props.summary.result.solutionId,
    () => props.summary.result.resultChecksum,
    () => props.summary.scenarioRevision,
    () => props.assignment.assignmentId,
    () => props.currentRevision,
  ],
  () => {
    generation += 1;
    jobRead += 1;
    listRead += 1;
    peopleRead += 1;
    people.value = [];
    nextPeople.value = null;
    personId.value = null;
    questionMode.value = "replace";
    jobs.value = [];
    nextJobs.value = null;
    selectedJobId = null;
    job.value = null;
    conflictInspectionOpen.value = false;
    excludedPair.value = null;
    assignmentPage.value = 0;
    busy.value = false;
    loadingPeople.value = false;
    loadingJobs.value = false;
    error.value = null;
    void loadPeople(null);
    void loadJobs();
  },
  { immediate: true },
);
watch([personId, questionMode], () => {
  error.value = null;
  excludedPair.value = null;
});
async function refreshQuestions(): Promise<void> {
  const token = generation;
  const current = selectedJobId;
  await loadJobs();
  if (isCurrent(token) && current !== null && selectedJobId === current) await showJob(current);
}
void onCounterfactualProgress((event) => {
  const context = event.payload.context;
  if (
    !alive ||
    context.scenarioId !== props.scenarioId ||
    context.revision !== props.summary.scenarioRevision
  )
    return;
  if (selectedJobId === event.payload.jobId) void showJob(event.payload.jobId);
  void loadJobs();
})
  .then((stop) => {
    if (alive) unlisten = stop;
    else stop();
  })
  .catch((failure: unknown) => {
    if (alive) error.value = safeMessage(failure);
  });
onScopeDispose(() => {
  alive = false;
  selectedJobId = null;
  generation += 1;
  jobRead += 1;
  listRead += 1;
  peopleRead += 1;
  unlisten?.();
});
</script>

<template>
  <section
    class="field-stack min-w-0"
    aria-label="Check an assignment change"
    data-accepted-counterfactual
  >
    <h6>Check an assignment change</h6>
    <p>
      Temporarily require or forbid an assignment in this saved result. The recorded original rules
      and score policy remain in force; other assignments may change. This question never edits the
      schedule.
    </p>
    <label>
      Temporary change
      <select
        v-model="questionMode"
        name="counterfactual-question"
        :disabled="busy || summary.scenarioRevision !== currentRevision"
      >
        <option value="replace">Replace the assigned person</option>
        <option value="require">Require another person without excluding the incumbent</option>
        <option value="exclude">Exclude the currently assigned person</option>
      </select>
    </label>
    <p v-if="questionMode === 'replace'">
      Choose another person for this shift and exclude {{ assignment.personName }} from that shift.
    </p>
    <p v-else-if="questionMode === 'require'">
      Require another person for this shift. {{ assignment.personName }} remains free to work this
      shift if the original Required rules permit both people.
    </p>
    <p v-else>
      Forbid {{ assignment.personName }} from working this shift. Required coverage still applies;
      other people may replace this person.
    </p>
    <p v-if="summary.scenarioRevision !== currentRevision" role="status">
      This saved schedule is older than the current scenario. Its diagnostic history is readable,
      but start a new solve to ask another question.
    </p>
    <label v-if="questionMode !== 'exclude'">
      Another person
      <select
        v-model="personId"
        name="counterfactual-person"
        :disabled="busy || loadingPeople || summary.scenarioRevision !== currentRevision"
      >
        <option :value="null">Choose a person</option>
        <option
          v-for="person in people.filter((item) => item.personId !== assignment.personId)"
          :key="person.personId"
          :value="person.personId"
        >
          {{ person.name }}
        </option>
      </select>
    </label>
    <button
      v-if="questionMode !== 'exclude'"
      type="button"
      :disabled="busy || loadingPeople || nextPeople === null"
      @click="loadPeople(nextPeople)"
    >
      Load next 50 people
    </button>
    <button
      type="button"
      :disabled="
        busy ||
        (questionMode !== 'exclude' && personId === null) ||
        summary.scenarioRevision !== currentRevision
      "
      @click="start"
    >
      {{
        questionMode === "replace"
          ? "Check replacement (up to 30 seconds)"
          : questionMode === "require"
            ? "Check required person (up to 30 seconds)"
            : "Check excluded person (up to 30 seconds)"
      }}
    </button>
    <p v-if="loadingPeople || loadingJobs || busy" role="status">
      Loading saved diagnostic information…
    </p>
    <div v-if="excludedPair" role="status">
      <p>
        {{ messages.assignmentInspection.blocked }} This is a check of the recorded original
        scenario, not a diagnostic solve or proof about another possible schedule. No question was
        saved.
      </p>
      <p>
        {{
          messages.assignmentInspection.rejectionCount(String(excludedPair.rejections.totalItems))
        }}
      </p>
      <ul>
        <li v-for="finding in excludedPair.rejections.items" :key="finding.ordinal">
          {{ messages.assignmentInspection.causes[finding.cause.kind] }}
          <code>{{ finding.bindingId }}</code>
        </li>
      </ul>
      <p v-if="excludedPair.rejections.continuation !== null">
        More reasons are available in the saved setup's person-and-shift inspector.
      </p>
    </div>
    <div v-if="error" role="alert">
      <p>{{ error }}</p>
      <button
        type="button"
        @click="
          loadJobs();
          loadPeople(null);
        "
      >
        Retry
      </button>
    </div>
    <h6>Saved questions about this result</h6>
    <p v-if="!loadingJobs && jobs.length === 0">No saved questions found in the loaded history.</p>
    <ul v-if="jobs.length">
      <li v-for="item in jobs" :key="item.jobId">
        <button type="button" class="break-all" :disabled="busy" @click="showJob(item.jobId)">
          {{ item.createdAt }} · {{ item.state }} ·
          {{ conditionText(item.condition.condition) }}
        </button>
      </li>
    </ul>
    <button type="button" :disabled="loadingJobs || nextJobs === null" @click="loadJobs(nextJobs)">
      Older saved questions
    </button>
    <button type="button" :disabled="busy || loadingJobs" @click="refreshQuestions()">
      Refresh saved questions
    </button>
    <template v-if="job">
      <h6>Recorded result</h6>
      <p role="status">
        {{ job.state }} · requested {{ job.request.createdAt }} · source revision
        {{ job.request.semantics.scenarioRevision }}
      </p>
      <p class="break-all">
        Recorded question: {{ conditionText(job.request.condition.condition) }}
      </p>
      <button
        v-if="diagnosticSource !== null"
        ref="conflictInspectionTrigger"
        type="button"
        :disabled="conflictInspectionOpen"
        @click="conflictInspectionOpen = true"
      >
        Inspect saved conflict evidence
      </button>
      <InfeasibilityInspection
        v-if="conflictInspectionOpen && diagnosticSource !== null"
        :source="diagnosticSource"
        :home="home"
        :heading-level="6"
        :rule-navigation-available="ruleNavigationAvailable"
        @close="closeConflictInspection"
        @rule-action="emit('ruleAction', $event)"
      />
      <button
        v-if="job.state === 'queued' || job.state === 'running'"
        type="button"
        :disabled="busy || summary.scenarioRevision !== currentRevision"
        @click="cancel"
      >
        Cancel question
      </button>
      <p v-if="job.state === 'failed'">
        The diagnostic failed ({{ job.error?.kind ?? "unknown" }}); it does not establish whether a
        temporary condition is possible.
      </p>
      <p v-else-if="job.state === 'cancelled' || job.state === 'interrupted'">
        The diagnostic was {{ job.state }}; no answer was established.
      </p>
      <template v-if="job.state === 'completed' && job.result">
        <p v-if="job.result.conclusion.type === 'provenImpossible'">
          The saved diagnostic proved the requested temporary condition impossible under the
          recorded original rules. The current scenario may have changed.
        </p>
        <p v-else-if="job.result.conclusion.type === 'notDistinguishedWithinBudget'">
          No conclusive answer within the budget. This does not prove the temporary condition
          impossible.
        </p>
        <template v-else-if="job.result.conclusion.type === 'verifiedAlternative'">
          <p>
            An independently verified alternative was found. Other assignments may change; it has
            not been applied to this schedule.
          </p>
          <p>
            Recorded score order relative to the saved base: {{ job.result.conclusion.ordering }}.
          </p>
          <p>
            This observed score difference does not, by itself, prove that every possible
            alternative satisfying this temporary condition is worse.
          </p>
          <p>
            Alternative result: <code>{{ job.result.conclusion.alternative.solutionId }}</code>
          </p>
          <p>
            Changed assignment records: {{ job.result.conclusion.comparison.assignments.length }};
            changed rules: {{ job.result.conclusion.comparison.rules.length }}.
          </p>
          <p v-if="job.result.runManifest.outcome.type === 'accepted'">
            Derived run status: {{ job.result.runManifest.outcome.status }}. Feasible means an
            alternative was independently accepted, not that this condition's best score was proved.
          </p>
          <section aria-label="Changed assignments">
            <h6>Changed assignments ({{ assignmentChanges.length }})</h6>
            <ol v-if="visibleAssignmentChanges.length" class="break-all">
              <li
                v-for="(change, index) in visibleAssignmentChanges"
                :key="assignmentPage * 20 + index"
              >
                {{ changeText(change) }}
              </li>
            </ol>
            <p v-else>No assignment records changed.</p>
            <div v-if="assignmentChanges.length > 20" class="action-row">
              <button type="button" :disabled="assignmentPage === 0" @click="assignmentPage -= 1">
                Previous changes
              </button>
              <span
                >Page {{ assignmentPage + 1 }} of
                {{ Math.ceil(assignmentChanges.length / 20) }}</span
              >
              <button
                type="button"
                :disabled="(assignmentPage + 1) * 20 >= assignmentChanges.length"
                @click="assignmentPage += 1"
              >
                Next changes
              </button>
            </div>
          </section>
          <table
            v-if="job.result.conclusion.comparison.scoreLevels.length"
            class="data-table w-full table-fixed"
          >
            <caption>
              Recorded objective levels (exact score units)
            </caption>
            <thead>
              <tr>
                <th scope="col">Level</th>
                <th scope="col">Direction</th>
                <th scope="col">Base</th>
                <th scope="col">Alternative</th>
                <th scope="col">Difference</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="level in job.result.conclusion.comparison.scoreLevels"
                :key="level.levelId"
              >
                <th scope="row" class="break-all">{{ level.levelId }}</th>
                <td>{{ level.direction }}</td>
                <td>{{ level.before }}</td>
                <td>{{ level.after }}</td>
                <td>{{ level.delta }}</td>
              </tr>
            </tbody>
          </table>
          <section v-if="categoryCount" aria-label="Preference category differences">
            <table class="data-table w-full table-fixed">
              <caption>
                Recorded penalty differences by objective level and category (exact score units)
              </caption>
              <thead>
                <tr>
                  <th scope="col">Level</th>
                  <th scope="col">Category</th>
                  <th scope="col">Base</th>
                  <th scope="col">Alternative</th>
                  <th scope="col">Difference</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="row in visibleCategories"
                  :key="`${row.levelId}:${row.category.categoryId}`"
                >
                  <td class="break-all">{{ row.levelId }}</td>
                  <th scope="row" class="break-all">{{ row.category.categoryId }}</th>
                  <td>{{ row.category.before ?? "Not present" }}</td>
                  <td>{{ row.category.after ?? "Not present" }}</td>
                  <td>{{ row.category.delta ?? "Not available" }}</td>
                </tr>
              </tbody>
            </table>
            <nav v-if="categoryCount > 20" aria-label="Preference category pages">
              <button type="button" :disabled="categoryPage === 0" @click="categoryPage -= 1">
                Previous categories
              </button>
              <span>Page {{ categoryPage + 1 }} of {{ Math.ceil(categoryCount / 20) }}</span>
              <button
                type="button"
                :disabled="(categoryPage + 1) * 20 >= categoryCount"
                @click="categoryPage += 1"
              >
                Next categories
              </button>
            </nav>
          </section>
        </template>
      </template>
    </template>
  </section>
</template>
