<script setup lang="ts">
import { computed, nextTick, onScopeDispose, reactive, ref, shallowRef, watch } from "vue";
import { RouterLink } from "vue-router";
import { getScenarioEntity, getScenarioView, SetupOperationScope } from "../api/generated";
import type {
  WorkforceAssignmentPair,
  WorkforceDateRange,
  WorkforceExternalStartingPlan,
  WorkforceSetupTimedShiftContinuation,
  WorkforceSetupWorkPage,
  WorkforceSetupWorkShift,
} from "../api/generated-domain-pack-contracts";
import {
  rebaseEntityDraft,
  resolveEntityDraftField,
  sameField,
  type EntityRebase,
} from "../entity-draft";
import { safeMessage, type ProjectHomeController, type ProjectSummary } from "../project-home";
import { useSetupCommandReview } from "../setup-command-review";
import { messages } from "../messages";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";

type Plan = WorkforceExternalStartingPlan;
type Source = { scenarioId: string; revision: number; libraryEpoch: number; entityId: string };
const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  dates: WorkforceDateRange;
  active: boolean;
}>();
const emit = defineEmits<{ source: [source: Source | null]; dirty: [dirty: boolean] }>();
const owner = props.home;
const review = useSetupCommandReview(owner, () => props.project);
const selected = shallowRef<readonly string[]>([]);
const base = shallowRef<Plan | null>(null);
const draft = shallowRef<Plan | null>(null);
const source = shallowRef<Source | null>(null);
const rebase = shallowRef<EntityRebase<Plan> | null>(null);
const reading = ref(false);
const error = ref<string | null>(null);
const receipt = shallowRef<{ scenarioId: string; commandId: string; revision: number } | null>(
  null,
);
const person = shallowRef<readonly string[]>([]);
const selectedShift = shallowRef<WorkforceSetupWorkShift | null>(null);
const pairPage = ref(0);
const pageSize = 20;
const dates = reactive({ ...props.dates });
const work = shallowRef<WorkforceSetupWorkPage | null>(null);
const workLoading = ref(false);
const workError = ref<string | null>(null);
const workCursors = shallowRef<readonly (WorkforceSetupTimedShiftContinuation | null)[]>([null]);
const workPage = ref(0);
const labels = shallowRef<ReadonlyMap<string, string>>(new Map());
const labelError = ref<string | null>(null);
const heading = ref<HTMLElement>();
const reviewHeading = ref<HTMLElement>();
const conflictHeading = ref<HTMLElement>();
let generation = 0;
let workGeneration = 0;
let labelGeneration = 0;
let alive = true;
const scopes = new Set<SetupOperationScope>();
let labelScope: SetupOperationScope | null = null;
let workScope: SetupOperationScope | null = null;
const dirty = computed(
  () =>
    !sameField(base.value, draft.value) || person.value.length > 0 || selectedShift.value !== null,
);
const unresolved = computed(() => owner.state.mutation?.outcome === "outcomeUnknown");
const busy = computed(
  () =>
    reading.value || review.state.pending || owner.state.busyAction !== null || unresolved.value,
);
const matches = (value: Source) =>
  alive &&
  props.home === owner &&
  props.project.scenarioId === value.scenarioId &&
  props.project.revision === value.revision &&
  owner.state.libraryEpoch === value.libraryEpoch;
const stale = computed(() => source.value !== null && !matches(source.value));
const proposed = computed(() => {
  const value = review.state.review?.proposed;
  return value?.kind === "externalStartingPlan" ? value : null;
});
const reviewPairs = computed(() => {
  if (!review.state.review) return null;
  const before = base.value?.assignments ?? [];
  const after = proposed.value?.assignments ?? [];
  const key = (pair: WorkforceAssignmentPair) => `${pair.personId}:${pair.shiftId}`;
  const beforeKeys = new Set(before.map(key)),
    afterKeys = new Set(after.map(key));
  const rows: (WorkforceAssignmentPair & { change: "Add" | "Remove" })[] = [];
  for (const pair of before)
    if (!afterKeys.has(key(pair))) rows.push({ ...pair, change: "Remove" });
  for (const pair of after) if (!beforeKeys.has(key(pair))) rows.push({ ...pair, change: "Add" });
  return rows;
});
const pairRows = computed(() => reviewPairs.value ?? draft.value?.assignments ?? []);
const pairs = computed(() =>
  pairRows.value.slice(pairPage.value * pageSize, (pairPage.value + 1) * pageSize),
);
const warnings = computed(() =>
  review.state.review
    ? [...review.state.review.warnings.changes, ...review.state.review.warnings.proposed]
    : [],
);
const warningPage = ref(0);
const canEdit = computed(
  () =>
    props.active &&
    !busy.value &&
    !stale.value &&
    rebase.value === null &&
    review.state.review === null &&
    draft.value !== null,
);

function disposeReads(): void {
  generation += 1;
  workGeneration += 1;
  labelGeneration += 1;
  for (const scope of scopes) scope.dispose();
  scopes.clear();
  labelScope = null;
  workScope = null;
  reading.value = false;
  workLoading.value = false;
}
function capture(entityId: string): Source {
  return {
    scenarioId: props.project.scenarioId,
    revision: props.project.revision,
    libraryEpoch: owner.state.libraryEpoch,
    entityId,
  };
}
function edit(value: Plan): void {
  if (!canEdit.value) return;
  review.invalidate();
  draft.value = value;
  error.value = null;
}
function changeName(event: Event): void {
  if (draft.value && event.target instanceof HTMLInputElement)
    edit({ ...draft.value, name: event.target.value });
}
function addPair(): void {
  const record = draft.value,
    personId = person.value[0],
    shift = selectedShift.value;
  if (!record || !personId || !shift || !canEdit.value) return;
  if (
    record.assignments.some((pair) => pair.personId === personId && pair.shiftId === shift.shiftId)
  ) {
    error.value = "This person–shift pair is already in the starting plan.";
    return;
  }
  edit({ ...record, assignments: [...record.assignments, { personId, shiftId: shift.shiftId }] });
  person.value = [];
  selectedShift.value = null;
}
function removePair(pair: WorkforceAssignmentPair): void {
  if (!draft.value) return;
  edit({
    ...draft.value,
    assignments: draft.value.assignments.filter(
      (item) => item.personId !== pair.personId || item.shiftId !== pair.shiftId,
    ),
  });
  pairPage.value = Math.min(
    pairPage.value,
    Math.max(0, Math.ceil(draft.value.assignments.length / pageSize) - 1),
  );
}
function discard(): void {
  if (review.state.applying || unresolved.value) return;
  review.invalidate();
  draft.value = base.value;
  rebase.value = null;
  person.value = [];
  selectedShift.value = null;
  error.value = null;
  pairPage.value = 0;
}
async function readRecord(merge = false): Promise<boolean> {
  const entityId = selected.value[0];
  if (!entityId || !props.active || props.home !== owner || busy.value || (!merge && dirty.value))
    return false;
  const context = capture(entityId),
    token = ++generation;
  const scope = new SetupOperationScope(context.scenarioId, context.revision);
  scopes.add(scope);
  reading.value = true;
  error.value = null;
  review.invalidate();
  try {
    const response = await getScenarioEntity(scope, { kind: "externalStartingPlan", entityId })
      .result;
    if (token !== generation || !matches(context) || selected.value[0] !== entityId) return false;
    const value = response.result.view.data.result.data;
    if (
      response.currentRevision !== context.revision ||
      response.result.scenarioId !== context.scenarioId ||
      response.result.revision !== context.revision ||
      value.kind !== "externalStartingPlan" ||
      value.id !== entityId
    )
      throw new Error("The imported starting-plan source changed.");
    if (merge && base.value && draft.value) {
      const merged = rebaseEntityDraft(base.value, draft.value, value, rebase.value);
      rebase.value = merged.conflicts.length ? merged : null;
      draft.value = merged.value;
    } else {
      draft.value = value;
    }
    base.value = value;
    source.value = context;
    pairPage.value = 0;
    return true;
  } catch (failure) {
    if (token === generation && matches(context)) error.value = safeMessage(failure);
    return false;
  } finally {
    scopes.delete(scope);
    scope.dispose();
    if (token === generation) reading.value = false;
  }
}
function resolve(field: keyof Plan, choice: "current" | "draft"): void {
  if (!rebase.value || busy.value || stale.value) return;
  const resolved = resolveEntityDraftField(rebase.value, field, choice);
  draft.value = resolved.value;
  rebase.value = resolved.conflicts.length ? resolved : null;
  heading.value?.focus();
}
async function preview(remove = false): Promise<boolean> {
  const value = draft.value;
  if (
    !value ||
    !canEdit.value ||
    person.value.length ||
    selectedShift.value ||
    (remove && dirty.value)
  )
    return false;
  warningPage.value = 0;
  pairPage.value = 0;
  return review.preview(
    {
      type: "applyDomainCommand",
      payload: remove
        ? { commandType: "official.workforce.remove_entity", payload: { entityId: value.id } }
        : { commandType: "official.workforce.update_entity", payload: { entity: value } },
    },
    remove ? null : { id: value.id, kind: "externalStartingPlan" },
  );
}
async function apply(truncateRedo = false): Promise<boolean> {
  const approved = review.state.review;
  if (!approved) return false;
  const result = await review.apply(truncateRedo);
  if (!result) return false;
  receipt.value = {
    scenarioId: approved.snapshot.scenarioId,
    commandId: approved.snapshot.commandId,
    revision: result.newRevision,
  };
  // A durable receipt wins even when the revision/library refresh invalidated its review.
  if (alive && props.home === owner && props.project.scenarioId === approved.snapshot.scenarioId) {
    base.value = draft.value = null;
    source.value = null;
    rebase.value = null;
    person.value = [];
    selectedShift.value = null;
    review.invalidate();
    if (approved.proposed === null) selected.value = [];
    else await readRecord();
  }
  return true;
}
async function focusAfter(
  action: Promise<boolean>,
  target: "editor" | "review" | "conflict",
): Promise<void> {
  const origin = document.activeElement,
    scenarioId = props.project.scenarioId;
  const accepted = await action;
  await nextTick();
  if (!alive || props.home !== owner || props.project.scenarioId !== scenarioId) return;
  if (document.activeElement === origin || document.activeElement === document.body)
    (accepted && target === "review"
      ? reviewHeading.value
      : accepted && target === "conflict"
        ? (conflictHeading.value ?? heading.value)
        : heading.value
    )?.focus();
}
function shiftLabel(shift: WorkforceSetupWorkShift): string {
  return `${shift.assignmentTypeName} · ${shift.interval.startsAt.local} – ${shift.interval.endsAt.local} · ${shift.locationName ?? "No location"} · ${shift.interval.startsAt.instant}`;
}
async function loadWork(index = 0): Promise<void> {
  if (!canEdit.value || workLoading.value) return;
  const context = capture(selected.value[0] ?? ""),
    token = ++workGeneration;
  workScope?.dispose();
  const scope = new SetupOperationScope(context.scenarioId, context.revision);
  scopes.add(scope);
  workScope = scope;
  workLoading.value = true;
  workError.value = null;
  try {
    const response = await getScenarioView(scope, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId: "official.workforce.setup.work_window",
        parameters: { dates: { ...dates }, limit: 50 },
        continuation: workCursors.value[index] ?? null,
      },
    }).result;
    if (token !== workGeneration || !matches(context)) return;
    if (
      response.currentRevision !== context.revision ||
      response.result.scenarioId !== context.scenarioId ||
      response.result.revision !== context.revision
    )
      throw new Error("The native work revision changed.");
    work.value = response.result.view.data.result.data;
    workPage.value = index;
    if (index === 0) workCursors.value = [null];
    if (work.value.continuation)
      workCursors.value = [...workCursors.value.slice(0, index + 1), work.value.continuation];
  } catch (failure) {
    if (token === workGeneration && matches(context)) workError.value = safeMessage(failure);
  } finally {
    scopes.delete(scope);
    scope.dispose();
    if (workScope === scope) workScope = null;
    if (token === workGeneration) workLoading.value = false;
  }
}
async function loadLabels(): Promise<void> {
  const context = source.value,
    rows = pairs.value,
    token = ++labelGeneration;
  labelScope?.dispose();
  labelScope = null;
  labels.value = new Map();
  labelError.value = null;
  if (!context || !props.active || !matches(context) || !rows.length) return;
  const scope = new SetupOperationScope(context.scenarioId, context.revision);
  scopes.add(scope);
  labelScope = scope;
  const found = new Map<string, string>();
  try {
    // Only the visible twenty pairs need labels; commands retain the complete pair set.
    for (const pair of rows) {
      if (token !== labelGeneration || !matches(context)) return;
      if (!found.has(`person:${pair.personId}`)) {
        const response = await getScenarioEntity(scope, { kind: "person", entityId: pair.personId })
          .result;
        const person = response.result.view.data.result.data;
        if (
          response.currentRevision !== context.revision ||
          response.result.scenarioId !== context.scenarioId ||
          response.result.revision !== context.revision ||
          person.kind !== "person" ||
          person.id !== pair.personId
        )
          throw new Error("Person label source changed.");
        found.set(`person:${pair.personId}`, person.name);
      }
      if (token !== labelGeneration || !matches(context)) return;
      if (!found.has(`shift:${pair.shiftId}`)) {
        const response = await getScenarioView(scope, {
          source: { kind: "stored" },
          query: {
            schemaVersion: 1,
            viewId: "official.workforce.setup.work_detail",
            parameters: { shiftId: pair.shiftId },
          },
        }).result;
        const shift = response.result.view.data.result.data.shift;
        if (
          response.currentRevision !== context.revision ||
          response.result.scenarioId !== context.scenarioId ||
          response.result.revision !== context.revision ||
          shift.shiftId !== pair.shiftId
        )
          throw new Error("Work label source changed.");
        found.set(`shift:${pair.shiftId}`, shiftLabel(shift));
      }
    }
    if (token === labelGeneration && matches(context)) labels.value = found;
  } catch (failure) {
    if (token === labelGeneration && matches(context)) labelError.value = safeMessage(failure);
  } finally {
    scopes.delete(scope);
    scope.dispose();
    if (labelScope === scope) labelScope = null;
  }
}
watch(
  [dirty, () => review.state.review],
  () => {
    emit("dirty", dirty.value || review.state.review !== null);
  },
  { immediate: true, flush: "sync" },
);
watch(
  [
    source,
    selected,
    dirty,
    reading,
    error,
    rebase,
    () => review.state.review,
    () => props.home,
    () => props.active,
    () => props.project.revision,
    () => owner.state.libraryEpoch,
  ],
  () => {
    const value = source.value;
    emit(
      "source",
      props.active &&
        value &&
        value.entityId === selected.value[0] &&
        matches(value) &&
        !dirty.value &&
        !reading.value &&
        !error.value &&
        !rebase.value &&
        !review.state.review
        ? value
        : null,
    );
  },
  { immediate: true, flush: "sync" },
);
watch(
  selected,
  () => {
    discard();
    base.value = draft.value = null;
    source.value = null;
    void readRecord();
  },
  { flush: "sync" },
);
watch(
  [
    () => props.home,
    () => props.project.scenarioId,
    () => props.project.revision,
    () => owner.state.libraryEpoch,
    () => props.active,
  ],
  () => {
    disposeReads();
    review.invalidate();
    work.value = null;
    workCursors.value = [null];
    labels.value = new Map();
    if (source.value && source.value.scenarioId !== props.project.scenarioId) {
      base.value = draft.value = null;
      source.value = null;
      selected.value = [];
      rebase.value = null;
      person.value = [];
      selectedShift.value = null;
    }
    if (props.active && !dirty.value && !review.state.applying) void readRecord();
  },
  { flush: "sync" },
);
watch([() => dates.startDate, () => dates.endDateExclusive], () => {
  workGeneration += 1;
  workScope?.dispose();
  workScope = null;
  work.value = null;
  workCursors.value = [null];
  workPage.value = 0;
  workLoading.value = false;
});
watch([pairs, source, () => props.active], () => void loadLabels(), { flush: "post" });
watch(
  () => owner.state.busyAction,
  (value) => {
    if (
      value === null &&
      props.active &&
      !dirty.value &&
      !review.state.applying &&
      (!base.value || stale.value)
    )
      void readRecord();
  },
);
onScopeDispose(() => {
  alive = false;
  disposeReads();
  emit("source", null);
});
</script>

<template>
  <section
    v-show="active"
    class="state-panel field-stack grid-cols-1"
    aria-labelledby="external-plan-heading"
    data-external-plan-editor
  >
    <h3 id="external-plan-heading" ref="heading" tabindex="-1">
      Imported starting plan — unverified
    </h3>
    <p>
      These pairs are editable intent, not Required locks or an accepted result. Repair and
      Alternatives use the complete saved plan; every candidate still requires native independent
      verification.
    </p>
    <RouterLink
      :to="{ name: 'project-assignments-import', params: { scenarioId: project.scenarioId } }"
    >
      Import assignments from CSV
    </RouterLink>
    <WorkforceEntityPicker
      kind="externalStartingPlan"
      label="Imported starting plan"
      :multiple="false"
      :model-value="selected"
      :project="project"
      :library-epoch="home.state.libraryEpoch"
      :disabled="busy || dirty"
      @update:model-value="selected = $event"
    />
    <p v-if="selected.length === 0">Choose an imported starting plan to inspect or edit it.</p>
    <p v-if="reading" role="status">Reading the exact current starting plan…</p>
    <p v-if="receipt?.scenarioId === project.scenarioId" role="status">
      Committed command <code>{{ receipt.commandId }}</code
      >; revision {{ receipt.revision }}.
    </p>
    <p v-if="error || review.state.error" role="alert">{{ error ?? review.state.error }}</p>
    <p v-if="unresolved" role="alert">
      The write outcome is unknown. Reconcile it in History; do not replay the command.
    </p>
    <RouterLink :to="{ name: 'project-history', params: { scenarioId: project.scenarioId } }"
      >Open History — undo, redo or reconcile the exact command</RouterLink
    >
    <button
      v-if="selected.length && !draft"
      type="button"
      :disabled="busy"
      @click="focusAfter(readRecord(), 'editor')"
    >
      Retry starting-plan read
    </button>
    <template v-if="draft && !review.state.review">
      <p v-if="stale" role="status">
        The revision or library changed. Raw edits are retained; rebase before review.
      </p>
      <button
        v-if="stale"
        type="button"
        :disabled="busy"
        @click="focusAfter(readRecord(true), 'conflict')"
      >
        Read current record and rebase edits
      </button>
      <section v-if="rebase" aria-labelledby="external-plan-conflicts">
        <h4 id="external-plan-conflicts" ref="conflictHeading" tabindex="-1">
          Resolve changed fields
        </h4>
        <div v-for="field in rebase.conflicts" :key="field">
          <p>{{ field === "assignments" ? "Complete assignment set" : field }}</p>
          <details>
            <summary>Current and draft values</summary>
            <pre class="whitespace-pre-wrap break-all">
Current: {{ JSON.stringify(rebase.current[field], null, 2) }}
Draft: {{ JSON.stringify(rebase.value[field], null, 2) }}</pre>
          </details>
          <button type="button" :disabled="busy || stale" @click="resolve(field, 'current')">
            Use current {{ field }}
          </button>
          <button type="button" :disabled="busy || stale" @click="resolve(field, 'draft')">
            Keep draft {{ field }}
          </button>
        </div>
      </section>
      <label
        >Starting-plan name
        <input :value="draft.name" :disabled="!canEdit" maxlength="1024" @input="changeName"
      /></label>
      <details>
        <summary>Descriptive original import fingerprints</summary>
        <p>
          Source bytes: <code>{{ draft.originalImport.sourceBlake3 }}</code>
        </p>
        <p>
          Original imported pairs: <code>{{ draft.originalImport.pairContentHash }}</code>
        </p>
        <p>
          These fingerprints do not authenticate acceptance and do not change when current pairs are
          edited.
        </p>
      </details>
      <p>
        {{ draft.assignments.length }} total pairs. Paging and the work display window never limit
        the saved command.
      </p>
      <p v-if="labelError" role="status">
        Labels unavailable: {{ labelError }}. Exact identities remain visible.
      </p>
      <p v-if="draft.assignments.length === 0">
        Explicit empty starting plan — not a missing source.
      </p>
      <ul>
        <li v-for="pair in pairs" :key="`${pair.personId}:${pair.shiftId}`">
          <p>
            {{ labels.get(`person:${pair.personId}`) ?? "Person label unavailable" }}
            <code>{{ pair.personId }}</code>
          </p>
          <p>
            {{ labels.get(`shift:${pair.shiftId}`) ?? "Work label unavailable" }}
            <code>{{ pair.shiftId }}</code>
          </p>
          <button
            type="button"
            :disabled="!canEdit"
            :aria-label="`Remove ${labels.get(`person:${pair.personId}`) ?? `person ${pair.personId}`} from ${labels.get(`shift:${pair.shiftId}`) ?? `work ${pair.shiftId}`}`"
            @click="removePair(pair)"
          >
            Remove pair
          </button>
        </li>
      </ul>
      <nav aria-label="Starting-plan pair pages">
        <button type="button" :disabled="pairPage === 0" @click="pairPage -= 1">
          Previous pairs
        </button>
        <button
          type="button"
          :disabled="(pairPage + 1) * pageSize >= draft.assignments.length"
          @click="pairPage += 1"
        >
          Next pairs
        </button>
      </nav>
      <fieldset :disabled="!canEdit" class="field-stack">
        <legend>Add a person–shift pair</legend>
        <WorkforceEntityPicker
          kind="person"
          label="Person for starting plan"
          :multiple="false"
          :model-value="person"
          :project="project"
          :library-epoch="home.state.libraryEpoch"
          :disabled="!canEdit"
          @update:model-value="
            person = $event;
            review.invalidate();
          "
        />
        <p v-if="selectedShift">
          Selected work: {{ shiftLabel(selectedShift) }} <code>{{ selectedShift.shiftId }}</code>
        </p>
        <label>Work starts on <input v-model="dates.startDate" type="date" /></label>
        <label>Work ends before <input v-model="dates.endDateExclusive" type="date" /></label>
        <button type="button" :disabled="workLoading" @click="loadWork()">
          Read native work, including recurring shifts
        </button>
        <p v-if="workLoading" role="status">Reading native work…</p>
        <p v-if="workError" role="alert">{{ workError }}</p>
        <template v-if="work">
          <p v-if="work.items.length === 0">No work in this display window.</p>
          <ul>
            <li v-for="shift in work.items" :key="shift.shiftId">
              <button
                type="button"
                @click="
                  selectedShift = shift;
                  review.invalidate();
                "
              >
                Choose {{ shiftLabel(shift) }}
              </button>
            </li>
          </ul>
          <button
            type="button"
            :disabled="workLoading || workPage === 0"
            @click="loadWork(workPage - 1)"
          >
            Previous work
          </button>
          <button
            type="button"
            :disabled="workLoading || work.continuation === null"
            @click="loadWork(workPage + 1)"
          >
            Next work
          </button>
        </template>
        <button
          type="button"
          :disabled="person.length !== 1 || selectedShift === null"
          @click="addPair"
        >
          Add pair to draft
        </button>
        <button
          type="button"
          @click="
            person = [];
            selectedShift = null;
          "
        >
          Clear pending pair
        </button>
      </fieldset>
      <button
        type="button"
        :disabled="!canEdit || person.length > 0 || selectedShift !== null"
        @click="focusAfter(preview(), 'review')"
      >
        Review complete starting-plan update
      </button>
      <button
        type="button"
        :disabled="!canEdit || dirty"
        @click="focusAfter(preview(true), 'review')"
      >
        Review starting-plan removal
      </button>
      <button
        type="button"
        :disabled="review.state.applying || unresolved"
        @click="
          discard();
          heading?.focus();
        "
      >
        Discard local edits
      </button>
    </template>
    <section v-if="review.state.review" aria-labelledby="external-plan-review">
      <h4 id="external-plan-review" ref="reviewHeading" tabindex="-1">
        Review native starting-plan changes
      </h4>
      <p>
        Scenario revision {{ review.state.review.snapshot.revision }}; one atomic, undoable command.
        No Required locks or accepted ancestry are added.
      </p>
      <p v-if="proposed">
        Save “{{ proposed.name }}”: {{ base?.assignments.length ?? 0 }} saved pairs →
        {{ proposed.assignments.length }} proposed pairs.
      </p>
      <p v-else>
        Remove the complete starting plan “{{ base?.name }}”. Historical run snapshots remain
        unchanged.
      </p>
      <p v-if="reviewPairs?.length === 0">Pair membership is unchanged.</p>
      <ul>
        <li v-for="pair in pairs" :key="`${pair.personId}:${pair.shiftId}`">
          <strong>{{ "change" in pair ? pair.change : "" }}</strong
          >:
          {{ labels.get(`person:${pair.personId}`) ?? "Person label unavailable" }}
          <code>{{ pair.personId }}</code> —
          {{ labels.get(`shift:${pair.shiftId}`) ?? "Work label unavailable" }}
          <code>{{ pair.shiftId }}</code>
        </li>
      </ul>
      <p v-if="labelError" role="status">
        Labels unavailable: {{ labelError }}. Exact identities remain visible.
      </p>
      <nav aria-label="Reviewed starting-plan pair changes">
        <button type="button" :disabled="pairPage === 0" @click="pairPage -= 1">
          Previous pair changes
        </button>
        <button
          type="button"
          :disabled="(pairPage + 1) * pageSize >= pairRows.length"
          @click="pairPage += 1"
        >
          Next pair changes
        </button>
      </nav>
      <details>
        <summary>Exact native change paths</summary>
        <ul>
          <li v-for="item in review.state.review.changes.items" :key="item.ordinal">
            {{ messages.people.changeKinds[item.change.kind] }}: <code>{{ item.change.path }}</code>
          </li>
        </ul>
        <p>{{ review.state.review.changes.totalItems }} native changes.</p>
        <button
          type="button"
          :disabled="busy || review.state.redoRequired"
          @click="focusAfter(review.page(true), 'review')"
        >
          First changes
        </button>
        <button
          type="button"
          :disabled="
            busy || review.state.redoRequired || review.state.review.changes.continuation === null
          "
          @click="focusAfter(review.page(), 'review')"
        >
          Next changes
        </button>
      </details>
      <section v-if="warnings.length" aria-label="Starting-plan review warnings">
        <ul>
          <li
            v-for="(warning, index) in warnings.slice(warningPage * 20, (warningPage + 1) * 20)"
            :key="index"
          >
            {{ warning.code }}: {{ warning.message }}
          </li>
        </ul>
        <button type="button" :disabled="warningPage === 0" @click="warningPage -= 1">
          Previous warnings
        </button>
        <button
          type="button"
          :disabled="(warningPage + 1) * 20 >= warnings.length"
          @click="warningPage += 1"
        >
          Next warnings
        </button>
      </section>
      <p v-if="review.state.redoRequired" role="alert">{{ messages.people.redoWarning }}</p>
      <button
        type="button"
        :disabled="busy || stale"
        @click="focusAfter(apply(review.state.redoRequired), 'editor')"
      >
        {{
          review.state.redoRequired
            ? messages.people.confirmRedo
            : "Apply reviewed starting-plan changes"
        }}
      </button>
      <button
        type="button"
        :disabled="busy"
        @click="
          review.invalidate();
          pairPage = 0;
          heading?.focus();
        "
      >
        Back to editing without applying
      </button>
    </section>
    <RouteLeaveGuard
      :home="home"
      :dirty="dirty"
      :pending="review.state.pending || home.state.busyAction !== null"
      :discard="discard"
    />
  </section>
</template>
