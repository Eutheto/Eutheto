<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import {
  getScenarioEntity,
  searchScenarioEntities,
  newUuidV7,
  SetupOperationScope,
  type ScenarioCommand,
} from "../api/generated";
import {
  WORKFORCE_ADD_ENTITY_COMMAND_ID,
  WORKFORCE_UPDATE_ENTITY_COMMAND_ID,
  WORKFORCE_REMOVE_ENTITY_COMMAND_ID,
  type WorkforceSetupEntityContinuation,
  type WorkforceSetupEntityPage,
} from "../api/generated-domain-pack-contracts";
import { type ProjectHomeController, type ProjectSummary, safeMessage } from "../project-home";
import {
  createPeopleRecordDraft,
  peopleRecordValue,
  isPeopleRecord,
  type PeopleRecord,
  type PeopleRecordDraft,
} from "../people-record-draft";
import {
  rebaseEntityDraft,
  resolveEntityDraftField,
  type EntityRebase,
  type EntityField,
} from "../entity-draft";
import { useSetupCommandReview } from "../setup-command-review";
import { messages, formatNumber } from "../messages";
import PeopleRecordFields from "./PeopleRecordFields.vue";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import PeopleBulkEditor from "./PeopleBulkEditor.vue";
import type { PeopleBulkSelection } from "../people-bulk-draft";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  locale?: string;
}>();
const copy = messages.people;
const kinds = ["person", "qualification", "team", "assignmentType"] as const;
const kind = ref<PeopleRecord["kind"]>("person");
const search = ref("");
const page = shallowRef<WorkforceSetupEntityPage | null>(null);
const pageLoading = ref(false);
const selectedIds = shallowRef<readonly string[]>([]);
const bulkSelection = shallowRef<PeopleBulkSelection | null>(null);
const bulkEditor = ref<InstanceType<typeof PeopleBulkEditor>>();
const pageError = ref<string | null>(null);
const detailLoading = ref(false);
const detailError = ref<string | null>(null);
const form = ref<HTMLFormElement>();
const reviewHeading = ref<HTMLElement>();
const editorHeading = ref<HTMLElement>();
const conflictHeading = ref<HTMLElement>();
const currentHeading = ref<HTMLElement>();
const listHeading = ref<HTMLElement>();
const fieldErrors = shallowRef<Readonly<Record<string, string>>>({});
const showErrors = ref(false);
const warningsPage = ref(0);
const showCurrent = ref(false);
const currentRaw = shallowRef<PeopleRecordDraft | null>(null);
const proposedRaw = shallowRef<PeopleRecordDraft | null>(null);
const review = useSetupCommandReview(props.home, () => props.project);
const reviewWarnings = computed(() => {
  const warnings = review.state.review?.warnings;
  return warnings === undefined ? [] : [...warnings.changes, ...warnings.proposed];
});
const fieldContext = computed(() => ({
  project: props.project,
  libraryEpoch: props.home.state.libraryEpoch,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
interface Editor {
  readonly scenarioId: string;
  readonly id: string;
  readonly base: PeopleRecord | null;
  readonly revision: number;
  readonly epoch: number;
  readonly raw: PeopleRecordDraft;
  readonly editing: boolean;
  readonly dirty: boolean;
  readonly current: PeopleRecord | null;
  readonly currentRevision: number | null;
  readonly currentEpoch: number | null;
  readonly rebase:
    (EntityRebase<PeopleRecord> & { readonly revision: number; readonly epoch: number }) | null;
}
const editor = shallowRef<Editor | null>(null);
const busy = computed(() => props.home.state.busyAction !== null);
const dirty = computed(() => editor.value?.dirty === true || bulkSelection.value !== null);
const stale = computed(
  () =>
    editor.value !== null &&
    (editor.value.revision !== props.project.revision ||
      editor.value.epoch !== props.home.state.libraryEpoch),
);
const rebaseCurrent = computed(() => {
  const value = editor.value;
  return (
    value?.rebase != null &&
    value.rebase.revision === props.project.revision &&
    value.rebase.epoch === props.home.state.libraryEpoch &&
    value.currentRevision === props.project.revision &&
    value.currentEpoch === props.home.state.libraryEpoch &&
    !detailLoading.value
  );
});
let alive = true;
let listGeneration = 0;
let detailGeneration = 0;
let draftGeneration = 0;
let listScope: SetupOperationScope | null = null;
let detailScope: SetupOperationScope | null = null;
let searchTimer: ReturnType<typeof setTimeout> | undefined;

function invalidateDraft(): void {
  draftGeneration += 1;
  review.invalidate();
  showErrors.value = false;
  fieldErrors.value = {};
}
function discard(): void {
  invalidateDraft();
  bulkEditor.value?.discard();
  bulkSelection.value = null;
  detailGeneration += 1;
  detailScope?.dispose();
  detailScope = null;
  detailLoading.value = false;
  detailError.value = null;
  editor.value = null;
  showCurrent.value = false;
  currentRaw.value = null;
}
async function close(): Promise<void> {
  if (review.state.pending) return;
  discard();
  await nextTick();
  listHeading.value?.focus();
}
function contextMatches(scenarioId: string, revision: number, epoch: number): boolean {
  return (
    alive &&
    props.project.scenarioId === scenarioId &&
    props.project.revision === revision &&
    props.home.state.libraryEpoch === epoch
  );
}
async function loadPage(cursor: WorkforceSetupEntityContinuation | null = null): Promise<void> {
  const captured = ++listGeneration;
  selectedIds.value = [];
  listScope?.dispose();
  listScope = null;
  page.value = null;
  pageError.value = null;
  if (!alive || props.project.domainPackId !== "official.workforce") return;
  pageLoading.value = true;
  const { scenarioId, revision } = props.project;
  const epoch = props.home.state.libraryEpoch;
  const entityKind = kind.value;
  const query = search.value;
  const owned = new SetupOperationScope(scenarioId, revision);
  listScope = owned;
  try {
    const response = await searchScenarioEntities(owned, {
      kind: entityKind,
      search: query,
      limit: 50,
      cursor,
    }).result;
    if (captured !== listGeneration || !contextMatches(scenarioId, revision, epoch)) return;
    const result = response.result.view.data.result.data;
    if (result.items.length > 50 || result.items.some((item) => item.kind !== entityKind))
      throw new Error("The native entity page did not match the requested kind or bound.");
    page.value = result;
  } catch (failure) {
    if (captured === listGeneration) pageError.value = safeMessage(failure);
  } finally {
    owned.dispose();
    if (listScope === owned) listScope = null;
    if (captured === listGeneration) pageLoading.value = false;
  }
}
async function loadDetail(
  id: string,
  entityKind: PeopleRecord["kind"],
  refresh = false,
): Promise<void> {
  if (!alive || bulkSelection.value !== null || (!refresh && editor.value?.editing)) return;
  const captured = ++detailGeneration;
  detailScope?.dispose();
  const { scenarioId, revision } = props.project;
  const epoch = props.home.state.libraryEpoch;
  const owned = new SetupOperationScope(scenarioId, revision);
  detailScope = owned;
  detailLoading.value = true;
  detailError.value = null;
  if (!refresh) {
    invalidateDraft();
    editor.value = null;
  }
  try {
    const response = await getScenarioEntity(owned, { kind: entityKind, entityId: id }).result;
    if (captured !== detailGeneration || !contextMatches(scenarioId, revision, epoch)) return;
    const entity = response.result.view.data.result.data;
    if (entity.id !== id || entity.kind !== entityKind || !isPeopleRecord(entity))
      throw new Error("The native detail did not match the selected record identity.");
    const existing = editor.value;
    if (refresh && existing?.id === id && existing.editing) {
      editor.value = {
        ...existing,
        current: entity,
        currentRevision: revision,
        currentEpoch: epoch,
      };
    } else {
      editor.value = {
        scenarioId,
        id,
        base: entity,
        revision,
        epoch,
        raw: createPeopleRecordDraft(entity),
        editing: false,
        dirty: false,
        current: entity,
        currentRevision: revision,
        currentEpoch: epoch,
        rebase: null,
      };
    }
    if (!refresh) {
      await nextTick();
      editorHeading.value?.focus();
    }
  } catch (failure) {
    if (captured === detailGeneration) detailError.value = safeMessage(failure);
  } finally {
    owned.dispose();
    if (detailScope === owned) detailScope = null;
    if (captured === detailGeneration) detailLoading.value = false;
  }
}
async function create(): Promise<void> {
  if (busy.value || editor.value?.editing || bulkSelection.value !== null) return;
  discard();
  editor.value = {
    scenarioId: props.project.scenarioId,
    id: newUuidV7(),
    base: null,
    revision: props.project.revision,
    epoch: props.home.state.libraryEpoch,
    raw: createPeopleRecordDraft(kind.value),
    editing: true,
    dirty: true,
    current: null,
    currentRevision: null,
    currentEpoch: null,
    rebase: null,
  };
  await nextTick();
  editorHeading.value?.focus();
}
async function edit(): Promise<void> {
  if (busy.value || stale.value || detailLoading.value || editor.value === null) return;
  invalidateDraft();
  editor.value = { ...editor.value, editing: true };
  await nextTick();
  form.value?.querySelector<HTMLInputElement>('input[name="name"]')?.focus();
}
function update(raw: PeopleRecordDraft): void {
  const value = editor.value;
  if (value === null || !value.editing || busy.value || raw.kind !== value.raw.kind) return;
  invalidateDraft();
  editor.value = { ...value, raw, dirty: true, rebase: null };
}
function candidate(): PeopleRecord | null {
  const value = editor.value;
  if (value === null) return null;
  showErrors.value = true;
  const converted = peopleRecordValue(value.id, value.raw);
  fieldErrors.value = converted.errors;
  if (!form.value?.reportValidity()) return null;
  return converted.value;
}
async function preview(): Promise<void> {
  const value = editor.value;
  if (value === null || !value.editing || stale.value || value.rebase !== null) return;
  const entity = candidate();
  if (entity === null) return;
  const command: ScenarioCommand = {
    type: "applyDomainCommand",
    payload: {
      commandType:
        value.base === null ? WORKFORCE_ADD_ENTITY_COMMAND_ID : WORKFORCE_UPDATE_ENTITY_COMMAND_ID,
      payload: { entity },
    },
  };
  if (await review.preview(command, { id: entity.id, kind: entity.kind })) {
    await nextTick();
    reviewHeading.value?.focus();
  }
}
async function previewDelete(): Promise<void> {
  const value = editor.value;
  if (value?.base == null || value.editing || stale.value || detailLoading.value) return;
  const command: ScenarioCommand = {
    type: "applyDomainCommand",
    payload: { commandType: WORKFORCE_REMOVE_ENTITY_COMMAND_ID, payload: { entityId: value.id } },
  };
  if (await review.preview(command, null)) {
    await nextTick();
    reviewHeading.value?.focus();
  }
}
function acceptRebase(value: Editor, merged: EntityRebase<PeopleRecord>): void {
  if (merged.conflicts.length !== 0) {
    editor.value = {
      ...value,
      rebase: { ...merged, revision: props.project.revision, epoch: props.home.state.libraryEpoch },
    };
    return;
  }
  editor.value = {
    ...value,
    base: merged.current,
    revision: props.project.revision,
    epoch: props.home.state.libraryEpoch,
    raw: createPeopleRecordDraft(merged.value, { value: merged.local, raw: value.raw }),
    current: merged.current,
    currentRevision: props.project.revision,
    currentEpoch: props.home.state.libraryEpoch,
    rebase: null,
    dirty: true,
  };
  showCurrent.value = false;
  currentRaw.value = null;
}
async function focusRebaseOutcome(): Promise<void> {
  await nextTick();
  if (editor.value?.rebase) conflictHeading.value?.focus();
  else form.value?.querySelector<HTMLButtonElement>('button[type="submit"]')?.focus();
}
async function rebase(): Promise<void> {
  const value = editor.value;
  if (value === null || !value.editing || !stale.value || busy.value) return;
  const local = candidate();
  if (local === null) return;
  if (value.base === null) {
    invalidateDraft();
    editor.value = {
      ...value,
      revision: props.project.revision,
      epoch: props.home.state.libraryEpoch,
    };
    await focusRebaseOutcome();
    return;
  }
  if (
    value.current === null ||
    value.currentRevision !== props.project.revision ||
    value.currentEpoch !== props.home.state.libraryEpoch ||
    detailLoading.value
  )
    return;
  invalidateDraft();
  acceptRebase(value, rebaseEntityDraft(value.base, local, value.current, value.rebase));
  await focusRebaseOutcome();
}
async function chooseField(
  field: EntityField<PeopleRecord>,
  choice: "current" | "draft",
): Promise<void> {
  const value = editor.value;
  if (value?.rebase == null || busy.value || !rebaseCurrent.value) return;
  acceptRebase(value, resolveEntityDraftField(value.rebase, field, choice));
  await focusRebaseOutcome();
}
async function inspectCurrent(): Promise<void> {
  const current = editor.value?.current;
  if (current) {
    currentRaw.value = createPeopleRecordDraft(current);
    showCurrent.value = true;
    await nextTick();
    currentHeading.value?.focus();
  }
}
async function copyAsNew(): Promise<void> {
  const value = editor.value;
  if (value === null || busy.value) return;
  invalidateDraft();
  detailGeneration += 1;
  detailScope?.dispose();
  detailScope = null;
  detailLoading.value = false;
  editor.value = {
    ...value,
    id: newUuidV7(),
    base: null,
    revision: props.project.revision,
    epoch: props.home.state.libraryEpoch,
    editing: true,
    dirty: true,
    current: null,
    currentRevision: null,
    currentEpoch: null,
    rebase: null,
  };
  detailError.value = null;
  await nextTick();
  form.value?.querySelector<HTMLInputElement>('input[name="name"]')?.focus();
}
function fieldSummary(record: PeopleRecord, field: EntityField<PeopleRecord>): string {
  const value: unknown = Reflect.get(record, field);
  if (value === undefined) return copy.absent;
  if (Array.isArray(value)) return `${formatNumber(value.length, props.locale)} ${copy.items}`;
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean" || value === null)
    return String(value);
  return copy.structuredField;
}
async function save(truncateRedo = false): Promise<void> {
  const value = editor.value;
  const captured = draftGeneration;
  const scenarioId = props.project.scenarioId;
  const focused = document.activeElement;
  const result = await review.apply(truncateRedo);
  if (
    result !== null &&
    alive &&
    props.project.scenarioId === scenarioId &&
    captured === draftGeneration &&
    editor.value?.id === value?.id
  ) {
    discard();
    await loadPage();
    await nextTick(() => {
      if (
        alive &&
        props.project.scenarioId === scenarioId &&
        editor.value === null &&
        (document.activeElement === focused || document.activeElement === document.body)
      ) {
        listHeading.value?.focus();
      }
    });
  }
}
async function refresh(): Promise<void> {
  const epoch = props.home.state.libraryEpoch;
  if ((await props.home.refreshLibrary()) && alive && epoch === props.home.state.libraryEpoch) {
    await loadPage();
    const value = editor.value;
    if (value?.base) await loadDetail(value.id, value.raw.kind, true);
  }
}
watch(
  () => review.state.review?.proposed,
  (record) => {
    proposedRaw.value = record && isPeopleRecord(record) ? createPeopleRecordDraft(record) : null;
  },
);
watch(
  () => review.state.review?.snapshot.commandId,
  () => {
    warningsPage.value = 0;
  },
);
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
  ],
  () => {
    const focused = document.activeElement;
    const restoreFocus = reviewHeading.value?.closest("section")?.contains(focused) === true;
    clearTimeout(searchTimer);
    void loadPage();
    detailGeneration += 1;
    detailScope?.dispose();
    detailScope = null;
    detailLoading.value = false;
    showCurrent.value = false;
    currentRaw.value = null;
    if (bulkSelection.value !== null && bulkSelection.value.scenarioId !== props.project.scenarioId)
      discard();
    const value = editor.value;
    if (value !== null) {
      if (value.scenarioId !== props.project.scenarioId) {
        discard();
        return;
      }
      editor.value = {
        ...value,
        current: null,
        currentRevision: null,
        currentEpoch: null,
      };
      if (value.base !== null) void loadDetail(value.id, value.raw.kind, true);
      if (restoreFocus) {
        void nextTick(() => {
          if (
            alive &&
            editor.value?.id === value.id &&
            stale.value &&
            (document.activeElement === focused || document.activeElement === document.body)
          ) {
            editorHeading.value?.focus();
          }
        });
      }
    }
  },
  { immediate: true, flush: "sync" },
);
watch(
  search,
  () => {
    clearTimeout(searchTimer);
    listGeneration += 1;
    listScope?.dispose();
    listScope = null;
    page.value = null;
    pageLoading.value = true;
    searchTimer = setTimeout(() => void loadPage(), 150);
  },
  { flush: "sync" },
);
async function changePage(cursor: WorkforceSetupEntityContinuation | null = null): Promise<void> {
  await loadPage(cursor);
  selectedIds.value = [];
  await nextTick();
  listHeading.value?.focus();
}
function chooseKind(event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  if (editor.value?.editing || busy.value || bulkSelection.value !== null) return;
  if (
    value === "person" ||
    value === "qualification" ||
    value === "team" ||
    value === "assignmentType"
  ) {
    discard();
    kind.value = value;
    search.value = "";
    clearTimeout(searchTimer);
    void loadPage();
  }
}
function selectPerson(id: string, checked: boolean): void {
  if (
    kind.value !== "person" ||
    bulkSelection.value !== null ||
    busy.value ||
    editor.value?.editing ||
    page.value?.items.some((item) => item.entityId === id) !== true
  )
    return;
  if (checked && !selectedIds.value.includes(id) && selectedIds.value.length < 50)
    selectedIds.value = [...selectedIds.value, id];
  else if (!checked) selectedIds.value = selectedIds.value.filter((value) => value !== id);
}
function openBulk(): void {
  if (
    kind.value !== "person" ||
    selectedIds.value.length === 0 ||
    busy.value ||
    editor.value?.editing ||
    bulkSelection.value !== null ||
    selectedIds.value.some((id) => page.value?.items.some((item) => item.entityId === id) !== true)
  )
    return;
  discard();
  bulkSelection.value = {
    scenarioId: props.project.scenarioId,
    revision: props.project.revision,
    libraryEpoch: props.home.state.libraryEpoch,
    ids: [...selectedIds.value],
  };
}
async function closeBulk(): Promise<void> {
  discard();
  selectedIds.value = [];
  await nextTick();
  listHeading.value?.focus();
}
onScopeDispose(() => {
  alive = false;
  clearTimeout(searchTimer);
  listGeneration += 1;
  detailGeneration += 1;
  listScope?.dispose();
  detailScope?.dispose();
});
</script>

<template>
  <section class="page-stack" aria-labelledby="people-heading">
    <h2 id="people-heading" data-route-heading tabindex="-1">{{ copy.heading }}</h2>
    <p>{{ copy.description }}</p>
    <p v-if="project.domainPackId !== 'official.workforce'">{{ messages.setup.unsupported }}</p>
    <template v-else>
      <RouterLink
        :to="{ name: 'project-people-import', params: { scenarioId: project.scenarioId } }"
      >
        {{ copy.importCsv }}
      </RouterLink>
      <section class="state-panel field-stack" aria-labelledby="people-list-heading">
        <h3 id="people-list-heading" ref="listHeading" tabindex="-1">{{ copy.records }}</h3>
        <label for="people-kind">{{ copy.recordKind }}</label>
        <select
          id="people-kind"
          :value="kind"
          :disabled="editor?.editing || busy || bulkSelection !== null"
          @change="chooseKind"
        >
          <option v-for="item in kinds" :key="item" :value="item">{{ copy.kinds[item] }}</option>
        </select>
        <label for="people-search">{{ copy.search }}</label>
        <input
          id="people-search"
          v-model="search"
          type="search"
          :disabled="bulkSelection !== null"
          :aria-describedby="'people-search-help'"
        />
        <p id="people-search-help" class="field-help">{{ copy.searchHelp }}</p>
        <div class="action-row">
          <button
            type="button"
            :disabled="busy || editor?.editing || bulkSelection !== null"
            @click="create"
          >
            {{ copy.create }}
          </button>
          <button type="button" :disabled="busy" @click="refresh">
            {{ messages.setup.refresh }}
          </button>
        </div>
        <p v-if="pageLoading" role="status">{{ copy.loading }}</p>
        <p v-else-if="pageError" role="alert">{{ pageError }}</p>
        <template v-else-if="page">
          <p v-if="page.items.length === 0">{{ copy.empty }}</p>
          <ul v-else class="field-stack">
            <li v-for="item in page.items" :key="item.entityId">
              <label v-if="kind === 'person'" class="break-all">
                <input
                  type="checkbox"
                  :checked="selectedIds.includes(item.entityId)"
                  :disabled="busy || editor?.editing || bulkSelection !== null"
                  @change="selectPerson(item.entityId, ($event.target as HTMLInputElement).checked)"
                />
                {{ messages.bulkPeople.select }} · {{ item.name ?? copy.unnamed }} ·
                {{ item.entityId }}
              </label>
              <button
                type="button"
                class="break-all"
                :disabled="busy || editor?.editing || bulkSelection !== null"
                @click="loadDetail(item.entityId, kind)"
              >
                {{ item.name ?? copy.unnamed }} · {{ item.entityId }}
              </button>
            </li>
          </ul>
          <p>{{ copy.matching }} {{ formatNumber(page.totalItems, locale) }}</p>
          <div v-if="kind === 'person'" class="action-row">
            <p>{{ messages.bulkPeople.selected }} {{ formatNumber(selectedIds.length, locale) }}</p>
            <button
              type="button"
              :disabled="
                selectedIds.length === 0 || busy || editor?.editing || bulkSelection !== null
              "
              @click="openBulk"
            >
              {{ messages.bulkPeople.open }}
            </button>
          </div>
          <nav class="action-row" :aria-label="copy.pages">
            <button
              type="button"
              :disabled="pageLoading || bulkSelection !== null"
              @click="changePage()"
            >
              {{ copy.first }}
            </button>
            <button
              type="button"
              :disabled="page.continuation === null || pageLoading || bulkSelection !== null"
              @click="changePage(page.continuation)"
            >
              {{ copy.next }}
            </button>
          </nav>
        </template>
      </section>
      <PeopleBulkEditor
        v-if="bulkSelection"
        ref="bulkEditor"
        :home="home"
        :project="project"
        :selection="bulkSelection"
        v-bind="locale === undefined ? {} : { locale }"
        @close="closeBulk"
      />
      <p v-if="detailLoading" role="status">{{ copy.loadingDetail }}</p>
      <div v-if="detailError" class="state-panel" role="alert">
        <p>{{ detailError }}</p>
        <p>{{ copy.detailUnavailable }}</p>
        <button
          v-if="editor"
          type="button"
          :disabled="busy"
          @click="loadDetail(editor.id, editor.raw.kind, true)"
        >
          {{ copy.retryDetail }}
        </button>
      </div>
      <section
        v-if="editor"
        class="state-panel field-stack"
        aria-labelledby="people-editor-heading"
      >
        <h3 id="people-editor-heading" ref="editorHeading" tabindex="-1">
          {{ copy.kinds[editor.raw.kind] }}: {{ editor.base?.name ?? copy.newRecord }}
        </h3>
        <p class="break-all">{{ copy.identity }} {{ editor.id }}</p>
        <p>{{ copy.baseline }} {{ formatNumber(editor.revision, locale) }}</p>
        <p v-if="editor.dirty" role="status">{{ copy.unsaved }}</p>
        <div v-if="stale" class="state-panel" role="status">
          <p>{{ copy.stale }}</p>
          <button
            v-if="editor.editing"
            type="button"
            :disabled="busy || detailLoading"
            @click="rebase"
          >
            {{ copy.reviewCurrent }}
          </button>
        </div>
        <form ref="form" class="field-stack" @submit.prevent="preview">
          <PeopleRecordFields
            v-bind="fieldContext"
            :model-value="editor.raw"
            :read-only="!editor.editing"
            :disabled="busy"
            :show-errors="showErrors"
            :errors="fieldErrors"
            @update:model-value="update"
          />
          <div class="action-row">
            <button
              v-if="!editor.editing"
              type="button"
              :disabled="busy || stale || detailLoading"
              @click="edit"
            >
              {{ copy.edit }}
            </button>
            <button v-else type="submit" :disabled="busy || stale || editor.rebase !== null">
              {{ copy.preview }}
            </button>
            <button
              v-if="!editor.editing"
              type="button"
              :disabled="busy || stale || detailLoading"
              @click="previewDelete"
            >
              {{ copy.reviewDelete }}
            </button>
            <button type="button" :disabled="review.state.pending" @click="close">
              {{ editor.editing ? copy.discard : copy.close }}
            </button>
            <button v-if="detailError" type="button" :disabled="busy" @click="copyAsNew">
              {{ copy.copyAsNew }}
            </button>
          </div>
        </form>
        <section v-if="editor.rebase" class="field-stack" :aria-label="copy.conflicts">
          <h4 ref="conflictHeading" tabindex="-1">{{ copy.conflicts }}</h4>
          <p>{{ copy.conflictHelp }}</p>
          <ul class="field-stack">
            <li v-for="field in editor.rebase.conflicts" :key="field">
              <h5>{{ copy.fields[field] }}</h5>
              <p>{{ copy.currentValue }} {{ fieldSummary(editor.rebase.current, field) }}</p>
              <p>{{ copy.draftValue }} {{ fieldSummary(editor.rebase.value, field) }}</p>
              <div class="action-row">
                <button
                  type="button"
                  :disabled="busy || !rebaseCurrent"
                  @click="chooseField(field, 'current')"
                >
                  {{ copy.useCurrent }}
                </button>
                <button
                  type="button"
                  :disabled="busy || !rebaseCurrent"
                  @click="chooseField(field, 'draft')"
                >
                  {{ copy.keepDraft }}
                </button>
              </div>
            </li>
          </ul>
          <button type="button" @click="inspectCurrent">{{ copy.inspectCurrent }}</button>
        </section>
        <section
          v-if="showCurrent && currentRaw"
          class="field-stack"
          :aria-label="copy.currentRecord"
        >
          <h4 ref="currentHeading" tabindex="-1">{{ copy.currentRecord }}</h4>
          <PeopleRecordFields v-bind="fieldContext" :model-value="currentRaw" read-only />
        </section>
      </section>
      <div v-if="review.state.error" class="state-panel" role="alert">{{ review.state.error }}</div>
      <section
        v-if="review.state.review"
        class="state-panel field-stack"
        aria-labelledby="people-review-heading"
      >
        <h3 id="people-review-heading" ref="reviewHeading" tabindex="-1">{{ copy.reviewTitle }}</h3>
        <p>{{ copy.previewReady }}</p>
        <p>{{ copy.validationScope }}</p>
        <p>
          {{ copy.reviewRevision }}
          {{ formatNumber(review.state.review.snapshot.revision, locale) }}
        </p>
        <p class="break-all">
          {{ copy.commandIdentity }} {{ review.state.review.snapshot.commandId }}
        </p>
        <p>{{ copy.localActorHelp }}</p>
        <ul class="field-stack">
          <li v-for="item in review.state.review.changes.items" :key="item.ordinal">
            {{ copy.changeKinds[item.change.kind] }} · <code>{{ item.change.path }}</code>
          </li>
        </ul>
        <p>
          {{ copy.changeCount }} {{ formatNumber(review.state.review.changes.totalItems, locale) }}
        </p>
        <nav class="action-row" :aria-label="copy.changePages">
          <button
            type="button"
            :disabled="busy || review.state.redoRequired"
            @click="review.page(true)"
          >
            {{ copy.first }}
          </button>
          <button
            type="button"
            :disabled="
              busy || review.state.redoRequired || review.state.review.changes.continuation === null
            "
            @click="review.page()"
          >
            {{ copy.next }}
          </button>
        </nav>
        <section v-if="proposedRaw" class="field-stack" :aria-label="copy.proposedRecord">
          <h4>{{ copy.proposedRecord }}</h4>
          <PeopleRecordFields v-bind="fieldContext" :model-value="proposedRaw" read-only />
        </section>
        <p v-else>{{ copy.deleteProposal }}</p>
        <section v-if="reviewWarnings.length" :aria-label="copy.warnings">
          <h4>{{ copy.warnings }}</h4>
          <ul>
            <li
              v-for="(warning, index) in reviewWarnings.slice(
                warningsPage * 50,
                (warningsPage + 1) * 50,
              )"
              :key="index"
            >
              {{ warning.message }}
            </li>
          </ul>
          <button type="button" :disabled="warningsPage === 0" @click="warningsPage -= 1">
            {{ copy.previous }}
          </button>
          <button
            type="button"
            :disabled="(warningsPage + 1) * 50 >= reviewWarnings.length"
            @click="warningsPage += 1"
          >
            {{ copy.next }}
          </button>
        </section>
        <div v-if="review.state.redoRequired" role="alert">
          <p>{{ copy.redoWarning }}</p>
          <button type="button" :disabled="busy" @click="save(true)">{{ copy.confirmRedo }}</button>
        </div>
        <button
          v-else
          type="button"
          :disabled="busy || home.state.mutation?.outcome === 'outcomeUnknown'"
          @click="save()"
        >
          {{ proposedRaw ? copy.save : copy.confirmDelete }}
        </button>
      </section>
    </template>
    <RouteLeaveGuard
      :home="home"
      :dirty="dirty"
      :pending="review.state.pending || bulkEditor?.pending === true"
      :discard="discard"
    />
  </section>
</template>
