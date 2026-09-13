<script setup lang="ts">
import { computed, nextTick, onMounted, onScopeDispose, ref, shallowRef, watch } from "vue";
import { getScenarioEntity, SetupOperationScope } from "../api/generated";
import type { WorkforcePerson } from "../api/generated-domain-pack-contracts";
import { resolveEntityDraftField, type EntityField, type EntityRebase } from "../entity-draft";
import {
  peopleBulkCandidates,
  peopleBulkCommand,
  rebasePeopleBulkPerson,
  type PeopleBulkAction,
  type PeopleBulkSelection,
} from "../people-bulk-draft";
import { createPeopleRecordDraft } from "../people-record-draft";
import {
  isOperationCancelled,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";
import { useSetupCommandReview } from "../setup-command-review";
import { formatNumber, messages } from "../messages";
import PeopleRecordFields from "./PeopleRecordFields.vue";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  selection: PeopleBulkSelection;
  locale?: string;
}>();
const emit = defineEmits<{ close: [] }>();
const copy = messages.bulkPeople;
const common = messages.people;
const heading = ref<HTMLElement>();
const errorHeading = ref<HTMLElement>();
const conflictHeading = ref<HTMLElement>();
const reviewHeading = ref<HTMLElement>();
const form = ref<HTMLFormElement>();
const records = shallowRef<readonly WorkforcePerson[] | null>(null);
const baseline = shallowRef(props.selection);
const resolved = shallowRef<readonly WorkforcePerson[] | null>(null);
const rebase = shallowRef<{
  context: PeopleBulkSelection;
  people: readonly EntityRebase<WorkforcePerson>[];
} | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const note = ref("");
const inspectedId = ref(props.selection.ids[0] ?? "");
const inspectMode = ref<"saved" | "current" | "draft">("saved");
const proposedId = ref("");
const warningsPage = ref(0);
interface RawAction {
  readonly kind: "" | PeopleBulkAction["kind"];
  readonly activeDatesEnabled: boolean;
  readonly startDate: string;
  readonly endDateExclusive: string;
  readonly teamId: string;
  readonly confirmDeletion: boolean;
}
const raw = shallowRef<RawAction>({
  kind: "",
  activeDatesEnabled: false,
  startDate: "",
  endDateExclusive: "",
  teamId: "",
  confirmDeletion: false,
});
const review = useSetupCommandReview(props.home, () => props.project);
const busy = computed(() => props.home.state.busyAction !== null);
const pending = computed(() => loading.value || review.state.pending);
const stale = computed(() => !sameContext(baseline.value));
const locked = computed(() => busy.value || pending.value || rebase.value !== null);
const context = computed(() => ({
  project: props.project,
  libraryEpoch: props.home.state.libraryEpoch,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
const action = computed<PeopleBulkAction | null>(() => {
  const value = raw.value;
  if (
    value.kind === "" ||
    ((value.kind === "addTeam" || value.kind === "removeTeam") && value.teamId === "")
  )
    return null;
  if (value.kind === "activeRange")
    return {
      kind: value.kind,
      range: value.activeDatesEnabled
        ? {
            kind: "dateRange",
            startDate: value.startDate,
            endDateExclusive: value.endDateExclusive,
          }
        : { kind: "always" },
    };
  if (value.kind === "delete") return { kind: "delete" };
  return { kind: value.kind, teamId: value.teamId };
});
const candidates = computed(
  () =>
    resolved.value ??
    (records.value !== null && action.value !== null
      ? peopleBulkCandidates(records.value, action.value)
      : records.value),
);
const inspected = computed(() => {
  const id = inspectedId.value;
  const entity =
    inspectMode.value === "current"
      ? rebase.value?.people.find((person) => person.current.id === id)?.current
      : inspectMode.value === "draft"
        ? (rebase.value?.people.map((person) => person.value) ?? candidates.value)?.find(
            (person) => person.id === id,
          )
        : records.value?.find((person) => person.id === id);
  return entity === undefined ? null : createPeopleRecordDraft(entity);
});
const proposed = computed(() => {
  if (review.state.pending) return null;
  const entity = review.state.review?.proposed;
  return entity?.kind === "person" ? createPeopleRecordDraft(entity) : null;
});
const warnings = computed(() => {
  const value = review.state.review?.warnings;
  return value === undefined ? [] : [...value.changes, ...value.proposed];
});
const conflictsRemain = computed(
  () => rebase.value?.people.some((person) => person.conflicts.length !== 0) === true,
);
const rebaseCurrent = computed(() => rebase.value !== null && sameContext(rebase.value.context));
let alive = true;
let generation = 0;
let scope: SetupOperationScope | null = null;

function sameContext(value: PeopleBulkSelection): boolean {
  return (
    alive &&
    value.scenarioId === props.project.scenarioId &&
    value.revision === props.project.revision &&
    value.libraryEpoch === props.home.state.libraryEpoch
  );
}
function cancelRead(): void {
  generation += 1;
  scope?.dispose();
  scope = null;
  loading.value = false;
}
function discard(): void {
  cancelRead();
  review.invalidate();
}
function patch(change: Partial<RawAction>): void {
  raw.value = { ...raw.value, ...change };
  resolved.value = null;
  review.invalidate();
  note.value = "";
}
function changeAction(event: Event): void {
  const kind = (event.target as HTMLSelectElement).value;
  if (
    kind === "" ||
    kind === "activeRange" ||
    kind === "addTeam" ||
    kind === "removeTeam" ||
    kind === "delete"
  )
    patch({ kind, confirmDeletion: false });
}
async function capture(context: PeopleBulkSelection, merge: boolean): Promise<void> {
  if (pending.value || busy.value || !sameContext(context)) return;
  const before = records.value;
  const local = candidates.value;
  const partial = rebase.value;
  const selectedAction = action.value;
  const captured = ++generation;
  const owned = new SetupOperationScope(context.scenarioId, context.revision);
  scope = owned;
  loading.value = true;
  error.value = null;
  note.value = "";
  review.invalidate();
  try {
    if (
      context.ids.length === 0 ||
      context.ids.length > 50 ||
      new Set(context.ids).size !== context.ids.length
    )
      throw new Error("A bulk action requires 1–50 distinct selected people.");
    const current: WorkforcePerson[] = [];
    // Publish only a complete, same-revision selection. A failed read never means deletion.
    for (const id of context.ids) {
      if (captured !== generation || !sameContext(context)) return;
      const response = await getScenarioEntity(owned, { kind: "person", entityId: id }).result;
      if (captured !== generation || !sameContext(context)) return;
      const entity = response.result.view.data.result.data;
      if (entity.kind !== "person" || entity.id !== id)
        throw new Error("The native person did not match the selected identity.");
      current.push(entity);
    }
    if (
      merge &&
      before !== null &&
      local !== null &&
      selectedAction !== null &&
      selectedAction.kind !== "delete"
    ) {
      rebase.value = {
        context,
        people: current.map((person, index) => {
          const prior = partial?.people[index];
          const base = prior?.current ?? before[index];
          const draft = prior?.value ?? local[index];
          if (base === undefined || draft === undefined)
            throw new Error("The captured selection changed during rebase.");
          return rebasePeopleBulkPerson(base, draft, person, selectedAction, prior?.conflicts);
        }),
      };
      inspectMode.value = "current";
      await nextTick();
      conflictHeading.value?.focus();
    } else {
      records.value = current;
      baseline.value = context;
      resolved.value = null;
      raw.value = { ...raw.value, confirmDeletion: false };
      if (merge) note.value = copy.rebaseReady;
    }
  } catch (failure) {
    if (captured === generation && alive)
      error.value = isOperationCancelled(failure) ? null : safeMessage(failure);
  } finally {
    owned.dispose();
    if (scope === owned) scope = null;
    if (captured === generation) loading.value = false;
  }
}
async function readCurrent(): Promise<void> {
  await capture(
    {
      scenarioId: props.project.scenarioId,
      revision: props.project.revision,
      libraryEpoch: props.home.state.libraryEpoch,
      ids: props.selection.ids,
    },
    true,
  );
}
async function choose(
  id: string,
  field: EntityField<WorkforcePerson>,
  choice: "current" | "draft",
): Promise<void> {
  const value = rebase.value;
  if (value === null || !sameContext(value.context) || busy.value) return;
  const focused = document.activeElement;
  const updated = {
    ...value,
    people: value.people.map((person) =>
      person.current.id === id ? resolveEntityDraftField(person, field, choice) : person,
    ),
  };
  rebase.value = updated;
  await nextTick();
  if (
    rebase.value === updated &&
    (document.activeElement === focused || document.activeElement === document.body)
  )
    conflictHeading.value
      ?.closest("section")
      ?.querySelector<HTMLButtonElement>("button:not(:disabled)")
      ?.focus();
}
async function acceptRebase(): Promise<void> {
  const value = rebase.value;
  if (value === null || conflictsRemain.value || !sameContext(value.context) || busy.value) return;
  records.value = value.people.map((person) => person.current);
  resolved.value = value.people.map((person) => person.value);
  baseline.value = value.context;
  rebase.value = null;
  inspectMode.value = "draft";
  note.value = copy.rebaseReady;
  await nextTick();
  form.value?.querySelector<HTMLButtonElement>('button[type="submit"]')?.focus();
}
async function preview(): Promise<void> {
  const value = action.value;
  const people = candidates.value;
  if (
    value === null ||
    people === null ||
    stale.value ||
    locked.value ||
    !form.value?.reportValidity() ||
    (value.kind === "delete" && !raw.value.confirmDeletion)
  )
    return;
  const first = people[0];
  const target =
    value.kind === "delete" || first === undefined
      ? null
      : { id: first.id, kind: "person" as const };
  if (await review.preview(peopleBulkCommand(people, value, copy.label), target)) {
    proposedId.value = target?.id ?? "";
    warningsPage.value = 0;
    await nextTick();
    reviewHeading.value?.focus();
  } else {
    await nextTick();
    errorHeading.value?.focus();
  }
}
async function inspectProposal(event: Event): Promise<void> {
  const id = (event.target as HTMLSelectElement).value;
  if (!props.selection.ids.includes(id)) return;
  if (await review.inspect({ id, kind: "person" })) proposedId.value = id;
}
async function save(truncateRedo = false): Promise<void> {
  const scenarioId = props.selection.scenarioId;
  const result = await review.apply(truncateRedo);
  if (result !== null && alive && props.project.scenarioId === scenarioId) emit("close");
}
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
  ],
  () => {
    const focused = document.activeElement;
    const restoreFocus =
      reviewHeading.value?.closest("section")?.contains(focused) === true ||
      conflictHeading.value?.closest("section")?.contains(focused) === true;
    cancelRead();
    note.value = common.stale;
    raw.value = { ...raw.value, confirmDeletion: false };
    if (restoreFocus)
      void nextTick(() => {
        if (
          alive &&
          (document.activeElement === focused || document.activeElement === document.body)
        )
          heading.value?.focus();
      });
  },
  { flush: "sync" },
);
onMounted(() => {
  heading.value?.focus();
  void capture(props.selection, false);
});
onScopeDispose(() => {
  alive = false;
  discard();
});
defineExpose({ pending, discard });
</script>

<template>
  <section class="state-panel field-stack" aria-labelledby="people-bulk-heading">
    <h3 id="people-bulk-heading" ref="heading" tabindex="-1">{{ copy.title }}</h3>
    <p>{{ copy.help }}</p>
    <p>
      {{ copy.selected }} {{ formatNumber(selection.ids.length, locale) }} · {{ common.baseline }}
      {{ formatNumber(baseline.revision, locale) }}
    </p>
    <p v-if="note" role="status">{{ note }}</p>
    <div v-if="loading" role="status">
      <p>{{ copy.loading }}</p>
      <button
        type="button"
        @click="
          cancelRead();
          note = copy.cancelled;
        "
      >
        {{ copy.cancelRead }}
      </button>
    </div>
    <div v-if="error || review.state.error" class="state-panel" role="alert">
      <h4 ref="errorHeading" tabindex="-1">{{ copy.errorTitle }}</h4>
      <p>{{ error ?? review.state.error }}</p>
      <p v-if="error">{{ copy.unavailable }}</p>
    </div>
    <button
      v-if="stale || records === null || error"
      type="button"
      :disabled="pending || busy"
      @click="readCurrent"
    >
      {{ copy.readCurrent }}
    </button>
    <template v-if="records">
      <label for="bulk-person-inspection">{{ copy.savedPerson }}</label>
      <select id="bulk-person-inspection" v-model="inspectedId">
        <option v-for="person in records" :key="person.id" :value="person.id">
          {{ person.name }} · {{ person.id }}
        </option>
      </select>
      <label for="bulk-person-version">{{ copy.inspectMode }}</label>
      <select id="bulk-person-version" v-model="inspectMode">
        <option value="saved">{{ copy.saved }}</option>
        <option v-if="rebase" value="current">{{ copy.current }}</option>
        <option value="draft">{{ copy.draft }}</option>
      </select>
      <PeopleRecordFields v-if="inspected" v-bind="context" :model-value="inspected" read-only />
      <form ref="form" class="field-stack" @submit.prevent="preview">
        <fieldset :disabled="locked" class="field-stack">
          <label for="bulk-action">{{ copy.action }}</label>
          <select id="bulk-action" :value="raw.kind" required @change="changeAction">
            <option value="">{{ copy.chooseAction }}</option>
            <option value="activeRange">{{ copy.activeRange }}</option>
            <option value="addTeam">{{ copy.addTeam }}</option>
            <option value="removeTeam">{{ copy.removeTeam }}</option>
            <option value="delete">{{ copy.delete }}</option>
          </select>
          <template v-if="raw.kind === 'activeRange'">
            <label
              ><input
                type="checkbox"
                :checked="raw.activeDatesEnabled"
                @change="patch({ activeDatesEnabled: ($event.target as HTMLInputElement).checked })"
              />
              {{ messages.personFields.limitActiveDates }}</label
            >
            <p>{{ messages.personFields.dateHelp }}</p>
            <template v-if="raw.activeDatesEnabled">
              <label for="bulk-start-date">{{ messages.personFields.startDate }}</label>
              <input
                id="bulk-start-date"
                type="text"
                placeholder="YYYY-MM-DD"
                maxlength="64"
                required
                :value="raw.startDate"
                @input="patch({ startDate: ($event.target as HTMLInputElement).value })"
              />
              <label for="bulk-end-date">{{ messages.personFields.endDate }}</label>
              <input
                id="bulk-end-date"
                type="text"
                placeholder="YYYY-MM-DD"
                maxlength="64"
                required
                :value="raw.endDateExclusive"
                @input="patch({ endDateExclusive: ($event.target as HTMLInputElement).value })"
              />
            </template>
          </template>
          <WorkforceEntityPicker
            v-if="raw.kind === 'addTeam' || raw.kind === 'removeTeam'"
            v-bind="context"
            id="bulk-team"
            kind="team"
            :label="copy.team"
            :disabled="locked"
            required
            :model-value="raw.teamId ? [raw.teamId] : []"
            @update:model-value="patch({ teamId: $event[0] ?? '' })"
          />
          <template v-if="raw.kind === 'delete'">
            <p>{{ copy.deleteWarning }}</p>
            <label
              ><input
                type="checkbox"
                required
                :checked="raw.confirmDeletion"
                @change="patch({ confirmDeletion: ($event.target as HTMLInputElement).checked })"
              />
              {{ copy.confirmDeletion }}</label
            >
          </template>
          <button
            type="submit"
            :disabled="
              stale || action === null || home.state.mutation?.outcome === 'outcomeUnknown'
            "
          >
            {{ copy.preview }}
          </button>
        </fieldset>
      </form>
    </template>
    <section v-if="rebase" class="state-panel field-stack" :aria-label="common.conflicts">
      <h4 ref="conflictHeading" tabindex="-1">{{ common.conflicts }}</h4>
      <p>{{ copy.conflictHelp }}</p>
      <p>{{ common.baseline }} {{ formatNumber(rebase.context.revision, locale) }}</p>
      <template v-for="person in rebase.people" :key="person.current.id">
        <div v-if="person.conflicts.length" class="field-stack">
          <p class="break-all">{{ person.current.name }} · {{ person.current.id }}</p>
          <fieldset
            v-for="field in person.conflicts"
            :key="field"
            :disabled="busy || !rebaseCurrent"
            class="action-row"
          >
            <legend>
              {{ common.fields[field] }} · {{ person.current.name }} · {{ person.current.id }}
            </legend>
            <button type="button" @click="choose(person.current.id, field, 'current')">
              {{ common.useCurrent }}
            </button>
            <button
              type="button"
              :disabled="busy"
              @click="choose(person.current.id, field, 'draft')"
            >
              {{ common.keepDraft }}
            </button>
          </fieldset>
        </div>
      </template>
      <button
        type="button"
        :disabled="conflictsRemain || busy || !rebaseCurrent"
        @click="acceptRebase"
      >
        {{ copy.acceptRebase }}
      </button>
    </section>
    <section
      v-if="review.state.review"
      class="state-panel field-stack"
      aria-labelledby="bulk-review-heading"
    >
      <h4 id="bulk-review-heading" ref="reviewHeading" tabindex="-1">{{ common.reviewTitle }}</h4>
      <p>{{ common.validationScope }}</p>
      <p>
        {{ common.reviewRevision }}
        {{ formatNumber(review.state.review.snapshot.revision, locale) }}
      </p>
      <p class="break-all">
        {{ common.commandIdentity }} {{ review.state.review.snapshot.commandId }}
      </p>
      <p>{{ common.localActorHelp }}</p>
      <ul>
        <li v-for="item in review.state.review.changes.items" :key="item.ordinal">
          {{ common.changeKinds[item.change.kind] }} · <code>{{ item.change.path }}</code>
        </li>
      </ul>
      <p>
        {{ common.changeCount }} {{ formatNumber(review.state.review.changes.totalItems, locale) }}
      </p>
      <nav class="action-row" :aria-label="common.changePages">
        <button
          type="button"
          :disabled="busy || review.state.redoRequired"
          @click="review.page(true)"
        >
          {{ common.first }}
        </button>
        <button
          type="button"
          :disabled="
            busy || review.state.redoRequired || review.state.review.changes.continuation === null
          "
          @click="review.page()"
        >
          {{ common.next }}
        </button>
      </nav>
      <template v-if="raw.kind !== 'delete'">
        <label for="bulk-proposed-person">{{ copy.proposedPerson }}</label>
        <select
          id="bulk-proposed-person"
          :value="proposedId"
          :disabled="busy || review.state.redoRequired"
          @change="inspectProposal"
        >
          <option v-for="person in records" :key="person.id" :value="person.id">
            {{ person.name }} · {{ person.id }}
          </option>
        </select>
        <section v-if="proposed" :aria-label="common.proposedRecord">
          <h5>{{ common.proposedRecord }}</h5>
          <PeopleRecordFields v-bind="context" :model-value="proposed" read-only />
        </section>
      </template>
      <p v-else>{{ copy.deleteWarning }}</p>
      <section v-if="warnings.length" :aria-label="common.warnings">
        <h5>{{ common.warnings }}</h5>
        <ul>
          <li
            v-for="(warning, index) in warnings.slice(warningsPage * 50, (warningsPage + 1) * 50)"
            :key="index"
          >
            {{ warning.message }}
          </li>
        </ul>
        <button type="button" :disabled="warningsPage === 0" @click="warningsPage -= 1">
          {{ common.previous }}
        </button>
        <button
          type="button"
          :disabled="(warningsPage + 1) * 50 >= warnings.length"
          @click="warningsPage += 1"
        >
          {{ common.next }}
        </button>
      </section>
      <div v-if="review.state.redoRequired" role="alert">
        <p>{{ common.redoWarning }}</p>
        <button type="button" :disabled="busy" @click="save(true)">{{ common.confirmRedo }}</button>
      </div>
      <button
        v-else
        type="button"
        :disabled="busy || home.state.mutation?.outcome === 'outcomeUnknown'"
        @click="save()"
      >
        {{ copy.save }}
      </button>
    </section>
    <button type="button" :disabled="pending" @click="emit('close')">{{ copy.discard }}</button>
  </section>
</template>
