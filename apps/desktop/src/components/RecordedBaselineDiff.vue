<script setup lang="ts">
import { onScopeDispose, ref, shallowRef, watch } from "vue";
import {
  getSolutionView,
  type RunBaselineOriginal,
  type SolutionSummaryDtoV1,
} from "../api/generated";
import {
  WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
  isWorkforceResultScheduleViewData,
  type WorkforceResultScheduleData,
  type WorkforceResultScheduleContinuation,
} from "../api/generated-domain-pack-contracts";
import { sameField } from "../entity-draft";
import { messages } from "../messages";
import { safeMessage, type ProjectHomeController, type ProjectSummary } from "../project-home";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  candidate?: SolutionSummaryDtoV1 | null;
}>();
const emit = defineEmits<{
  recordedBaseline: [
    input: { candidate: SolutionSummaryDtoV1; original: RunBaselineOriginal } | null,
  ];
}>();
type RepairDiff = Extract<WorkforceResultScheduleData, { kind: "repairDiff" }>;
const diff = shallowRef<RepairDiff | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const cursors = shallowRef<readonly (WorkforceResultScheduleContinuation | null)[]>([null]);
const pageIndex = ref(0);
let generation = 0;
let alive = true;

async function load(index = 0): Promise<void> {
  const candidate = props.candidate;
  if (!candidate) return;
  const scenarioId = props.project.scenarioId;
  const token = ++generation;
  const current = () => alive && token === generation;
  loading.value = true;
  error.value = null;
  try {
    const response = await getSolutionView({
      scenarioId,
      solutionId: candidate.result.solutionId,
      viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
      parameters: { kind: "repairDiff", limit: 50 },
      ...(cursors.value[index] === null || cursors.value[index] === undefined
        ? {}
        : { continuation: cursors.value[index] }),
    });
    if (!current()) return;
    const result = response.result;
    if (
      result.scenarioId !== scenarioId ||
      result.scenarioRevision !== candidate.scenarioRevision ||
      result.result.solutionId !== candidate.result.solutionId ||
      result.result.resultChecksum !== candidate.result.resultChecksum ||
      result.view.viewId !== WORKFORCE_RESULT_SCHEDULE_VIEW_ID ||
      !isWorkforceResultScheduleViewData(result.view.data) ||
      result.view.data.result.kind !== "repairDiff"
    )
      throw new Error("Recorded repair result identity mismatch.");
    const value = result.view.data.result;
    if (
      value.source.scenarioId !== scenarioId ||
      value.source.solutionId !== candidate.result.solutionId ||
      value.source.sourceRevision !== candidate.scenarioRevision
    )
      throw new Error("Recorded repair source identity mismatch.");
    if (
      diff.value &&
      (!sameField(diff.value.source, value.source) ||
        !sameField(diff.value.baseline, value.baseline))
    )
      throw new Error("Recorded repair baseline changed between pages.");
    diff.value = value;
    pageIndex.value = index;
    if (index === 0) cursors.value = [null];
    if (value.continuation)
      cursors.value = [...cursors.value.slice(0, index + 1), value.continuation];
    emit(
      "recordedBaseline",
      value.baseline?.kind === "accepted" ? { candidate, original: value.baseline.original } : null,
    );
  } catch (failure) {
    if (current()) {
      diff.value = null;
      error.value = safeMessage(failure);
      emit("recordedBaseline", null);
    }
  } finally {
    if (current()) loading.value = false;
  }
}

watch(
  () => [
    props.home,
    props.project.scenarioId,
    props.project.revision,
    props.home.state.libraryEpoch,
    props.candidate?.scenarioId,
    props.candidate?.scenarioRevision,
    props.candidate?.result.solutionId,
    props.candidate?.result.resultChecksum,
  ],
  () => {
    generation += 1;
    diff.value = null;
    cursors.value = [null];
    pageIndex.value = 0;
    loading.value = false;
    error.value = null;
    emit("recordedBaseline", null);
    if (props.candidate) void load();
  },
  { immediate: true, flush: "sync" },
);
onScopeDispose(() => {
  alive = false;
  generation += 1;
});
</script>

<template>
  <section
    v-if="candidate"
    data-recorded-repair-diff
    class="state-panel field-stack grid-cols-1"
    aria-labelledby="recorded-repair-heading"
  >
    <h3 id="recorded-repair-heading">{{ messages.repair.diffHeading }}</h3>
    <p>{{ messages.repair.diffBoundary }}</p>
    <p>
      Chosen C: <code>{{ candidate.result.solutionId }}</code
      >, immutable revision {{ candidate.scenarioRevision }}.
    </p>
    <p v-if="loading" role="status">Reading recorded repair groups…</p>
    <p v-if="error" role="alert">{{ error }}</p>
    <button v-if="error" type="button" :disabled="loading" @click="load()">
      Retry recorded repair groups
    </button>
    <p v-if="diff?.baseline?.kind === 'accepted'">
      Recorded original A checksum:
      <code>{{ diff.baseline.original.acceptedResult.resultChecksum }}</code
      >; source revision {{ diff.baseline.original.sourceRevision }}; original document
      <code>{{ diff.baseline.original.sourceDocumentHash }}</code
      >; run <code>{{ diff.baseline.original.sourceRunId }}</code
      >.
    </p>
    <p
      v-if="
        diff?.baseline?.kind === 'accepted' && diff.baseline.selection.kind === 'manualSelection'
      "
      data-recorded-manual-baseline
    >
      Recorded manual B: <code>{{ diff.baseline.selection.entity.id }}</code
      >; record checksum <code>{{ diff.baseline.selection.recordChecksum }}</code
      >; admission source revision {{ diff.baseline.selection.admissionSourceRevision }}; admission
      document <code>{{ diff.baseline.selection.admissionSourceDocumentHash }}</code
      >.
    </p>
    <p v-if="diff?.baseline?.kind === 'external'" data-recorded-external-baseline>
      Recorded unverified external starting plan: <code>{{ diff.baseline.binding.entity.id }}</code
      >; exact current pair hash <code>{{ diff.baseline.binding.selectedPairContentHash }}</code
      >. This is descriptive starting-plan content, not accepted original A or a manual-admission
      receipt.
    </p>
    <template v-if="diff">
      <p v-if="diff.availability === 'absent'">{{ messages.repair.diffAbsent }}</p>
      <template v-else>
        <p>
          Recorded C source document: <code>{{ diff.source.sourceDocumentHash }}</code
          >; normalized solution: <code>{{ diff.source.normalizedSolutionHash }}</code
          >.
        </p>
        <p v-if="diff.baseline?.kind === 'accepted'">
          Original A: <code>{{ diff.baseline.original.acceptedResult.solutionId }}</code
          >; recorded baseline selection {{ diff.baseline.selection.kind }}; exact pair hash
          <code>{{ diff.baseline.selectedPairContentHash }}</code
          >; baseline verification {{ diff.baseline.verificationStatus }}.
        </p>
        <p v-if="diff.counts">
          Exact native pair counts: {{ diff.counts.unchangedPairs }} unchanged,
          {{ diff.counts.removedPairs }} removed, {{ diff.counts.addedPairs }} added.
        </p>
        <p v-if="diff.items.length === 0">No recorded shift groups in this page.</p>
        <article v-for="row in diff.items" :key="row.shiftId">
          <h4>{{ row.shiftLabel ?? row.shiftId }}</h4>
          <p v-if="row.missingShift">{{ messages.repair.missingEndpoint }}</p>
          <code>{{ row.shiftId }}</code>
          <p>
            {{
              row.change === "newlyUnfilled" && row.afterMemberCount > 0
                ? "Some assignments removed; other selected members remain — not a claim that the whole shift is unfilled"
                : row.change === "newlyUnfilled"
                  ? "All recorded selected members removed — Required coverage status is a separate native fact"
                  : row.change === "replacement"
                    ? "Same-shift member changes — no unique causal pairing is implied"
                    : row.change === "newlyAssigned"
                      ? "New assignments"
                      : "Unchanged work"
            }}. Before {{ row.beforeMemberCount }}, after {{ row.afterMemberCount }}; removed pairs
            {{ row.removedPairs }}, added pairs {{ row.addedPairs }}.
          </p>
          <h5>Before — exact selected members</h5>
          <ul>
            <li v-for="member in row.oldMembers" :key="member.personId">
              {{ member.personName ?? member.personId }} <code>{{ member.personId }}</code>
              <span v-if="member.missingPerson"> — {{ messages.repair.missingEndpoint }}</span>
            </li>
          </ul>
          <h5>After — exact selected members</h5>
          <ul>
            <li v-for="member in row.newMembers" :key="member.personId">
              {{ member.personName ?? member.personId }} <code>{{ member.personId }}</code>
              <span v-if="member.missingPerson"> — {{ messages.repair.missingEndpoint }}</span>
            </li>
          </ul>
        </article>
        <nav aria-label="Recorded repair group pages">
          <button type="button" :disabled="loading || pageIndex === 0" @click="load(pageIndex - 1)">
            Previous recorded repair groups
          </button>
          <button
            type="button"
            :disabled="loading || diff.continuation === null"
            @click="load(pageIndex + 1)"
          >
            Next recorded repair groups
          </button>
        </nav>
      </template>
    </template>
  </section>
</template>
