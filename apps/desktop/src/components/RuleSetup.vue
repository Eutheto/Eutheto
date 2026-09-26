<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, watch } from "vue";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import type {
  WorkforceSetupRuleCatalogEntry,
  WorkforceSetupRuleReference,
  WorkforceSetupScopePart,
} from "../api/generated-domain-pack-contracts";
import type { ScopeFieldName } from "./planner/scope-field";
import { formatNumber, messages } from "../messages";
import { useRuleSetup } from "../rule-setup";
import { useValidationRoute } from "../validation-route";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import RuleScopeBuilder from "./planner/RuleScopeBuilder.vue";
import DurationField from "./planner/DurationField.vue";

const props = defineProps<{
  readonly home: ProjectHomeController;
  readonly project: ProjectSummary;
  readonly locale?: string;
}>();
const vm = useRuleSetup(props.home, () => props.project);
const { state, review, busy, dirty, stale, canReplace, canApply, conflictsCurrent } = vm;
const copy = messages.ruleSetup;
const recordsCopy = messages.people;
const editorHeading = ref<HTMLElement>();
const listHeading = ref<HTMLElement>();
const reviewHeading = ref<HTMLElement>();
const editorHost = ref<HTMLElement>();
const scopeFields = new Map<WorkforceSetupScopePart, InstanceType<typeof RuleScopeBuilder>>();
const pairKeys = ref<string[]>([]);
const pairPage = ref(0);
const warningPage = ref(0);
let rowSequence = 0;
let focusGeneration = 0;
let alive = true;
const locked = computed(
  () => busy.value || state.editor?.editing !== true || state.editor.rebase !== null,
);
const parts = computed<readonly WorkforceSetupScopePart[]>(() =>
  state.editor?.kind === "minimumRest"
    ? ["main", "minimumRestBefore", "minimumRestAfter"]
    : ["main"],
);
const partLabels: Readonly<Record<WorkforceSetupScopePart, string>> = {
  main: copy.fields.scope,
  minimumRestBefore: copy.fields.beforeScope,
  minimumRestAfter: copy.fields.afterScope,
};
const partPaths = {
  main: "scope",
  minimumRestBefore: "beforeScope",
  minimumRestAfter: "afterScope",
} as const;
const pairOffset = computed(() => pairPage.value * 50);
const pairRows = computed(
  () =>
    state.editor?.raw.compatibleCategoryPairs.slice(pairOffset.value, pairOffset.value + 50) ?? [],
);
const warnings = computed(() =>
  review.state.review === null
    ? []
    : [...review.state.review.warnings.changes, ...review.state.review.warnings.proposed],
);
const catalogEntries = computed(
  () => state.catalog?.[state.classFilter === "required" ? "required" : "preferences"] ?? [],
);
watch(
  () => [state.editor?.id, state.editor?.raw.compatibleCategoryPairs.length] as const,
  ([id, length], previous) => {
    if (id !== previous?.[0]) {
      pairKeys.value = [];
      pairPage.value = 0;
    }
    pairKeys.value.splice(length ?? 0);
    while (pairKeys.value.length < (length ?? 0))
      pairKeys.value.push(`pair-${String(++rowSequence)}`);
    pairPage.value = Math.min(pairPage.value, Math.max(0, Math.ceil((length ?? 0) / 50) - 1));
  },
  { immediate: true, flush: "sync" },
);
watch(
  () => review.state.review?.snapshot,
  () => {
    warningPage.value = 0;
  },
);
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
  ],
  () => {
    focusGeneration += 1;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  alive = false;
  focusGeneration += 1;
});

function ownership() {
  const generation = ++focusGeneration;
  const scenarioId = props.project.scenarioId;
  const revision = props.project.revision;
  const epoch = props.home.state.libraryEpoch;
  return () =>
    alive &&
    generation === focusGeneration &&
    props.project.scenarioId === scenarioId &&
    props.project.revision === revision &&
    props.home.state.libraryEpoch === epoch;
}
function kindLabel(kind: string): string {
  return Object.hasOwn(copy.kinds, kind) ? copy.kinds[kind as keyof typeof copy.kinds] : kind;
}
function descriptorLabel(id: string): string {
  return (
    [...(state.catalog?.required ?? []), ...(state.catalog?.preferences ?? [])].find(
      (entry) => entry.descriptor.id === id,
    )?.descriptor.title.defaultText ?? id
  );
}
function nativeValue(value: unknown): string {
  return value === undefined ? recordsCopy.absent : JSON.stringify(value, null, 2);
}
function fieldValue(value: object, field: string): unknown {
  return Object.hasOwn(value, field) ? Reflect.get(value, field) : undefined;
}
function text(event: Event): string {
  return (event.target as HTMLInputElement | HTMLSelectElement).value;
}
function pairId(index: number, field: "firstCategory" | "secondCategory"): string | undefined {
  const id = state.editor?.id;
  const key = pairKeys.value[index];
  return id === undefined || key === undefined ? undefined : `rule-${id}-${key}-${field}`;
}
function scopeErrors(
  part: WorkforceSetupScopePart,
): Readonly<Partial<Record<ScopeFieldName, string>>> {
  const prefix = `${partPaths[part]}.`;
  const errors: Partial<Record<ScopeFieldName, string>> = {};
  for (const [path, message] of Object.entries(state.errors)) {
    if (!path.startsWith(prefix)) continue;
    const segments = path.slice(prefix.length).split(".");
    const field =
      segments[0] === "people" && (segments[1] === "allTags" || segments[1] === "anyTags")
        ? segments[1]
        : segments[0];
    if (
      field === "people" ||
      field === "allTags" ||
      field === "anyTags" ||
      field === "teamIds" ||
      field === "assignmentTypeIds" ||
      field === "locationIds" ||
      field === "categories" ||
      field === "weekdays"
    )
      errors[field] = message;
  }
  return errors;
}
async function focusField(path: readonly string[], current: () => boolean): Promise<boolean> {
  const editor = state.editor;
  if (!current() || editor === null) return false;
  const owns = () => current() && state.editor === editor;
  const part =
    path[0] === "scope"
      ? "main"
      : path[0] === "beforeScope"
        ? "minimumRestBefore"
        : path[0] === "afterScope"
          ? "minimumRestAfter"
          : null;
  if (part !== null) {
    if (!parts.value.includes(part)) return false;
    await nextTick();
    if (!owns()) return false;
    return (await scopeFields.get(part)?.focusField(path.slice(1), owns)) ?? false;
  }
  let id: string | null = null;
  if (path.length === 0) {
    await nextTick();
    if (!owns()) return false;
    editorHeading.value?.focus();
    return document.activeElement === editorHeading.value;
  }
  if (path.length === 1) {
    const fields = {
      id: "rule-identity",
      kind: "rule-kind",
      strength: "rule-strength",
      active: "rule-active",
      minimumMinutes: "rule-minimum-rest",
      compatibleCategoryPairs: "rule-category-pairs",
    } as const;
    const field = path[0];
    if (field !== undefined && Object.hasOwn(fields, field))
      id = fields[field as keyof typeof fields];
  }
  if (
    path[0] === "compatibleCategoryPairs" &&
    path.length === 3 &&
    /^\d+$/.test(path[1] ?? "") &&
    (path[2] === "firstCategory" || path[2] === "secondCategory")
  ) {
    const index = Number(path[1]);
    if (!Number.isSafeInteger(index) || index >= editor.raw.compatibleCategoryPairs.length)
      return false;
    pairPage.value = Math.floor(index / 50);
    id = pairId(index, path[2]) ?? null;
  }
  if (id === null) return false;
  await nextTick();
  if (!owns()) return false;
  const control = document.getElementById(id);
  if (!(control instanceof HTMLElement) || !editorHost.value?.contains(control)) return false;
  control.focus();
  return document.activeElement === control;
}
async function openRule(rule: WorkforceSetupRuleReference): Promise<void> {
  const current = ownership();
  if ((await vm.loadDetail(rule)) && current()) {
    await nextTick();
    if (current()) editorHeading.value?.focus();
  }
}
async function createRule(entry: WorkforceSetupRuleCatalogEntry): Promise<void> {
  const current = ownership();
  if (vm.createRule(entry)) await focusField([], current);
}
async function editRule(): Promise<void> {
  const current = ownership();
  if (vm.editRule()) await focusField([], current);
}
async function closeEditor(): Promise<void> {
  const current = ownership();
  vm.discard();
  await nextTick();
  if (current() && state.editor === null) listHeading.value?.focus();
}
async function preview(action: "create" | "update" | "activation" | "delete"): Promise<void> {
  const current = ownership();
  const ready = await vm.preview(action);
  if (!current()) return;
  await nextTick();
  if (!current()) return;
  if (ready) reviewHeading.value?.focus();
  else {
    const first = Object.keys(state.errors)[0];
    if (first === undefined || !(await focusField(first.split("."), current))) {
      if (current()) editorHeading.value?.focus();
    }
  }
}
async function save(truncateRedo = false): Promise<void> {
  const scenarioId = props.project.scenarioId;
  const focused = document.activeElement;
  if (
    !(await vm.save(truncateRedo)) ||
    !alive ||
    props.project.scenarioId !== scenarioId ||
    state.editor !== null
  )
    return;
  const current = ownership();
  const ownsList = () => current() && state.editor === null;
  await nextTick();
  if (
    ownsList() &&
    (document.activeElement === focused || document.activeElement === document.body)
  )
    listHeading.value?.focus();
}
async function rebase(): Promise<void> {
  const current = ownership();
  await vm.rebase();
  if (!current()) return;
  const first = Object.keys(state.errors)[0];
  await focusField(first === undefined ? [] : first.split("."), current);
}
function updatePair(index: number, field: "firstCategory" | "secondCategory", value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value) return;
  vm.updateRaw({
    ...raw,
    compatibleCategoryPairs: raw.compatibleCategoryPairs.map((pair, at) =>
      at === index ? { ...pair, [field]: value } : pair,
    ),
  });
}
async function addPair(): Promise<void> {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value) return;
  const index = raw.compatibleCategoryPairs.length;
  vm.updateRaw({
    ...raw,
    compatibleCategoryPairs: [
      ...raw.compatibleCategoryPairs,
      { firstCategory: "", secondCategory: "" },
    ],
  });
  await focusField(["compatibleCategoryPairs", String(index), "firstCategory"], ownership());
}
async function removePair(index: number): Promise<void> {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value) return;
  pairKeys.value.splice(index, 1);
  vm.updateRaw({
    ...raw,
    compatibleCategoryPairs: raw.compatibleCategoryPairs.filter((_, at) => at !== index),
  });
  const next = Math.min(index, raw.compatibleCategoryPairs.length - 2);
  await focusField(
    next < 0
      ? ["compatibleCategoryPairs"]
      : ["compatibleCategoryPairs", String(next), "firstCategory"],
    ownership(),
  );
}
const navigationError = useValidationRoute(
  props.home,
  () => props.project,
  () => state.catalog !== null && state.facts !== null,
  async (target, current) => {
    if (target.collection !== "rules" || !current() || !canReplace.value) return false;
    const loading = vm.loadDetail({ class: "required", ruleId: target.id });
    const generation = state.draftGeneration;
    if (
      !(await loading) ||
      !current() ||
      state.draftGeneration !== generation ||
      state.editor?.id !== target.id ||
      !vm.editRule()
    )
      return false;
    const editor = state.editor;
    return await focusField(target.fieldPath, () => current() && state.editor === editor);
  },
);
</script>

<template>
  <section class="page-stack" aria-labelledby="rules-heading">
    <h2 id="rules-heading" data-route-heading tabindex="-1">{{ copy.heading }}</h2>
    <p v-if="project.domainPackId !== 'official.workforce'">{{ messages.setup.unsupported }}</p>
    <template v-else>
      <RouteLeaveGuard
        :home="home"
        :dirty="dirty || state.editor?.editing === true || review.state.review !== null"
        :pending="review.state.pending || home.state.busyAction === 'rule-scope-review'"
        :discard="vm.discard"
      />
      <p>{{ copy.strengthDescription }}</p>
      <div v-if="navigationError" class="state-panel" role="alert">{{ navigationError }}</div>
      <div v-if="state.catalogError" class="state-panel" role="alert">{{ state.catalogError }}</div>
      <button
        v-if="state.catalogError"
        type="button"
        :disabled="busy || state.catalogLoading || state.pageLoading"
        @click="vm.refresh"
      >
        {{ copy.refresh }}
      </button>
      <p v-if="state.catalogLoading" role="status">{{ recordsCopy.loading }}</p>
      <p v-if="state.facts">{{ messages.work.timeZone }}: {{ state.facts.settings.timeZone }}</p>
      <details v-if="state.facts" class="setup-more">
        <summary>{{ copy.moreRuleContext }}</summary>
        <p>
          {{
            copy.planningDates(
              state.facts.planningDates.startDate,
              state.facts.planningDates.endDateExclusive,
            )
          }}
        </p>
        <p>{{ recordsCopy.reviewRevision }} {{ formatNumber(project.revision, locale) }}</p>
      </details>
      <section
        v-if="state.catalog"
        v-show="state.editor === null && state.detail === null && review.state.review === null"
        class="field-stack"
        aria-labelledby="rule-catalog-heading"
      >
        <h3 id="rule-catalog-heading">{{ copy.availableKinds }}</h3>
        <ul class="field-stack">
          <li
            v-for="entry in state.catalog.required.filter((item) => vm.kindFor(item) !== null)"
            :key="entry.descriptor.id"
          >
            <button
              type="button"
              :disabled="!canReplace || state.facts === null"
              @click="createRule(entry)"
            >
              {{ entry.descriptor.title.defaultText }}
            </button>
            <p>{{ entry.descriptor.description.defaultText }}</p>
          </li>
        </ul>
        <details>
          <summary>{{ copy.otherKinds }}</summary>
          <ul>
            <li
              v-for="entry in state.catalog.required.filter((item) => vm.kindFor(item) === null)"
              :key="entry.descriptor.id"
            >
              {{ entry.descriptor.title.defaultText }} — {{ copy.notImplemented }}
            </li>
          </ul>
        </details>
        <details>
          <summary>{{ copy.preferencesHeading }}</summary>
          <p>{{ copy.preferenceHint }}</p>
          <ul>
            <li v-for="entry in state.catalog.preferences" :key="entry.descriptor.id">
              {{ entry.descriptor.title.defaultText }} — {{ copy.notImplemented }}
            </li>
          </ul>
        </details>
        <button
          type="button"
          :disabled="busy || state.catalogLoading || state.pageLoading"
          @click="vm.refresh"
        >
          {{ copy.refresh }}
        </button>
      </section>
      <section
        v-show="state.editor === null && state.detail === null && review.state.review === null"
        class="field-stack"
        aria-labelledby="rule-list-heading"
        :aria-busy="state.pageLoading"
      >
        <h3 id="rule-list-heading" ref="listHeading" tabindex="-1">{{ copy.list }}</h3>
        <div class="form-columns">
          <div class="field-stack">
            <label for="rule-class-filter">{{ copy.strength }}</label
            ><select
              id="rule-class-filter"
              :value="state.classFilter"
              @change="
                vm.setFilter(text($event) === 'preference' ? 'preference' : 'required', null)
              "
            >
              <option value="required">{{ copy.requiredHeading }}</option>
              <option value="preference">{{ copy.preferencesHeading }}</option>
            </select>
          </div>
          <div class="field-stack">
            <label for="rule-kind-filter">{{ copy.fields.kind }}</label
            ><select
              id="rule-kind-filter"
              :value="state.kindFilter ?? ''"
              @change="vm.setFilter(state.classFilter, text($event) || null)"
            >
              <option value="">{{ copy.allKinds }}</option>
              <option
                v-for="entry in catalogEntries"
                :key="entry.descriptor.id"
                :value="entry.descriptor.id"
              >
                {{ entry.descriptor.title.defaultText }}
              </option>
            </select>
          </div>
        </div>
        <p v-if="state.pageError" role="alert">{{ state.pageError }}</p>
        <p v-if="state.page?.items.length === 0">{{ copy.listEmpty }}</p>
        <ul v-if="state.page" class="field-stack">
          <li v-for="item in state.page.items" :key="`${item.rule.class}:${item.rule.ruleId}`">
            <button type="button" :disabled="!canReplace" @click="openRule(item.rule)">
              {{ descriptorLabel(item.kindId) }} · {{ item.active ? copy.active : copy.inactive }} ·
              {{ item.rule.ruleId }}
            </button>
          </li>
        </ul>
        <nav v-if="state.page" class="action-row" :aria-label="copy.list">
          <button type="button" :disabled="state.pageLoading" @click="vm.loadPage()">
            {{ recordsCopy.first }}</button
          ><button
            type="button"
            :disabled="state.pageLoading || state.page.continuation === null"
            @click="vm.loadPage(state.page.continuation)"
          >
            {{ recordsCopy.next }}
          </button>
        </nav>
      </section>
      <p v-if="state.detailLoading" role="status">{{ recordsCopy.loading }}</p>
      <p v-if="state.detailError" role="alert">{{ state.detailError }}</p>
      <section
        v-if="state.editor || state.detail"
        ref="editorHost"
        class="state-panel field-stack"
        aria-labelledby="rule-editor-heading"
      >
        <h3 id="rule-editor-heading" ref="editorHeading" tabindex="-1">
          {{ state.editor ? kindLabel(state.editor.kind) : copy.unsupportedClass }}
        </h3>
        <template v-if="state.editor">
          <p id="rule-identity" tabindex="-1" class="break-all">
            {{ copy.identity }}: {{ state.editor.id }}
          </p>
          <p id="rule-kind" tabindex="-1">
            {{ copy.fields.kind }}: {{ kindLabel(state.editor.kind) }}
          </p>
          <p id="rule-strength" tabindex="-1">
            {{ copy.strength }}: {{ copy.strengthAlwaysRequired }}
          </p>
          <p>{{ copy.baseline }}: {{ formatNumber(state.editor.context.revision, locale) }}</p>
          <p v-if="dirty" role="status">{{ copy.unsaved }}</p>
          <p v-if="stale" role="status">{{ copy.stale }}</p>
          <p v-if="state.editorError" role="alert">{{ state.editorError }}</p>
          <div class="action-row">
            <button
              v-if="!state.editor.editing"
              type="button"
              :disabled="busy || stale"
              @click="editRule"
            >
              {{ copy.edit }}
            </button>
            <button
              v-if="!state.editor.editing"
              type="button"
              :disabled="busy || stale"
              @click="preview('activation')"
            >
              {{ copy.reviewToggle }}
            </button>
            <button
              v-if="!state.editor.editing"
              type="button"
              :disabled="busy || stale"
              @click="preview('delete')"
            >
              {{ copy.reviewDelete }}
            </button>
            <button v-if="stale" type="button" :disabled="busy" @click="rebase">
              {{ copy.reviewCurrent }}
            </button>
            <button type="button" :disabled="busy" @click="closeEditor">
              {{ dirty || state.editor.editing ? copy.discard : copy.close }}
            </button>
          </div>
          <section
            v-if="Object.keys(state.errors).length"
            role="alert"
            class="field-stack"
            :aria-label="copy.diagnostics"
          >
            <ul>
              <li v-for="(message, field) in state.errors" :key="field">
                <button type="button" @click="focusField(String(field).split('.'), ownership())">
                  {{ field }}: {{ message }}
                </button>
              </li>
            </ul>
          </section>
          <section v-if="state.editor.rebase" class="field-stack" :aria-label="copy.conflicts">
            <h4>{{ copy.conflicts }}</h4>
            <p>{{ copy.conflictHelp }}</p>
            <ul class="field-stack">
              <li v-for="field in state.editor.rebase.conflicts" :key="field">
                <strong>{{ field }}</strong>
                <details>
                  <summary>{{ copy.currentValue }}</summary>
                  <pre class="wrap-anywhere whitespace-pre-wrap">{{
                    nativeValue(fieldValue(state.editor.rebase.current, field))
                  }}</pre>
                </details>
                <details>
                  <summary>{{ copy.draftValue }}</summary>
                  <pre class="wrap-anywhere whitespace-pre-wrap">{{
                    nativeValue(fieldValue(state.editor.rebase.value, field))
                  }}</pre>
                </details>
                <div class="action-row">
                  <button
                    type="button"
                    :disabled="busy || !conflictsCurrent"
                    @click="vm.chooseConflict(field, 'current')"
                  >
                    {{ copy.useCurrent }}</button
                  ><button
                    type="button"
                    :disabled="busy || !conflictsCurrent"
                    @click="vm.chooseConflict(field, 'draft')"
                  >
                    {{ copy.keepDraft }}
                  </button>
                </div>
              </li>
            </ul>
          </section>
          <form
            class="field-stack"
            novalidate
            @submit.prevent="preview(state.editor.base === null ? 'create' : 'update')"
          >
            <label for="rule-active"
              ><input
                id="rule-active"
                type="checkbox"
                :checked="state.editor.raw.active"
                :disabled="locked"
                @change="
                  vm.updateRaw({
                    ...state.editor.raw,
                    active: ($event.target as HTMLInputElement).checked,
                  })
                "
              />
              {{ copy.active }}</label
            >
            <p v-if="!state.editor.raw.active">{{ copy.inactive }} — {{ copy.inactiveHelp }}</p>
            <RuleScopeBuilder
              v-for="part in parts"
              :id="`rule-${state.editor.id}-${part}`"
              :key="`${state.editor.id}:${part}`"
              :ref="
                (value) => {
                  if (value) scopeFields.set(part, value as InstanceType<typeof RuleScopeBuilder>);
                  else scopeFields.delete(part);
                }
              "
              :label="partLabels[part]"
              :model-value="vm.scopeValue(part)"
              :entity-options="vm.entityOptions(part)"
              :field-errors="scopeErrors(part)"
              :preview="state.scopes[part].preview"
              :current-request-key="state.scopes[part].requestKey"
              :preview-axis="state.scopes[part].axis"
              :preview-limit="50"
              :disabled="busy || state.editor.rebase !== null"
              :read-only="!state.editor.editing"
              v-bind="{
                ...(locale === undefined ? {} : { locale }),
                ...(state.errors[partPaths[part]] === undefined
                  ? {}
                  : { error: state.errors[partPaths[part]] }),
              }"
              @update:model-value="vm.updateScope(part, $event)"
              @entity-search="(field, query) => vm.searchEntity(part, field, query)"
              @entity-next-page="
                (field) => vm.searchEntity(part, field, vm.picker(part, field).query, true)
              "
              @entity-retry="
                (field) => vm.searchEntity(part, field, vm.picker(part, field).query, false, true)
              "
              @preview="vm.inspectScope(part, $event)"
            />
            <DurationField
              v-if="state.editor.kind === 'minimumRest'"
              id="rule-minimum-rest"
              :label="copy.fields.minimumMinutes"
              :description="copy.fields.minimumRestHelp"
              :model-value="state.editor.raw.minimumRest"
              :minimum="0"
              required
              :disabled="busy || state.editor.rebase !== null"
              :read-only="!state.editor.editing"
              v-bind="{
                ...(locale === undefined ? {} : { locale }),
                ...(state.errors.minimumMinutes === undefined
                  ? {}
                  : { error: state.errors.minimumMinutes }),
              }"
              @update:model-value="vm.updateRaw({ ...state.editor.raw, minimumRest: $event })"
            />
            <fieldset
              v-if="state.editor.kind === 'noOverlap'"
              id="rule-category-pairs"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ copy.fields.compatibleCategoryPairs }}</legend>
              <p>{{ copy.fields.categoryPairHelp }}</p>
              <ul class="field-stack">
                <li
                  v-for="(pair, row) in pairRows"
                  :key="pairKeys[pairOffset + row]"
                  class="field-stack"
                >
                  <label :for="pairId(pairOffset + row, 'firstCategory')"
                    >{{ copy.fields.firstCategory }} {{ pairOffset + row + 1 }}</label
                  ><input
                    :id="pairId(pairOffset + row, 'firstCategory')"
                    :value="pair.firstCategory"
                    :readonly="!state.editor.editing"
                    :disabled="busy || state.editor.rebase !== null"
                    @input="updatePair(pairOffset + row, 'firstCategory', text($event))"
                  /><label :for="pairId(pairOffset + row, 'secondCategory')"
                    >{{ copy.fields.secondCategory }} {{ pairOffset + row + 1 }}</label
                  ><input
                    :id="pairId(pairOffset + row, 'secondCategory')"
                    :value="pair.secondCategory"
                    :readonly="!state.editor.editing"
                    :disabled="busy || state.editor.rebase !== null"
                    @input="updatePair(pairOffset + row, 'secondCategory', text($event))"
                  /><button
                    v-if="state.editor.editing"
                    type="button"
                    :disabled="locked"
                    @click="removePair(pairOffset + row)"
                  >
                    {{ copy.fields.removePair }} {{ pairOffset + row + 1 }}
                  </button>
                </li>
              </ul>
              <nav class="action-row" :aria-label="copy.fields.compatibleCategoryPairs">
                <button type="button" :disabled="pairPage === 0" @click="pairPage -= 1">
                  {{ recordsCopy.previous }}</button
                ><button
                  type="button"
                  :disabled="pairOffset + 50 >= state.editor.raw.compatibleCategoryPairs.length"
                  @click="pairPage += 1"
                >
                  {{ recordsCopy.next }}</button
                ><button
                  v-if="state.editor.editing"
                  type="button"
                  :disabled="locked"
                  @click="addPair"
                >
                  {{ copy.fields.addPair }}
                </button>
              </nav>
            </fieldset>
            <button
              v-if="state.editor.editing"
              type="submit"
              :disabled="busy || stale || state.editor.rebase !== null"
            >
              {{ copy.preview }}
            </button>
          </form>
        </template>
        <template v-else-if="state.detail"
          ><p>{{ copy.notImplementedHint }}</p>
          <pre class="wrap-anywhere whitespace-pre-wrap">{{ nativeValue(state.detail) }}</pre>
          <button type="button" @click="closeEditor">{{ copy.close }}</button></template
        >
      </section>
      <p v-if="review.state.error" role="alert">{{ review.state.error }}</p>
      <section
        v-if="review.state.review"
        class="state-panel field-stack"
        aria-labelledby="rule-review-heading"
      >
        <h3 id="rule-review-heading" ref="reviewHeading" tabindex="-1">
          {{ copy.previewHeading }}
        </h3>
        <p>
          {{ recordsCopy.reviewRevision }}
          {{ formatNumber(review.state.review.snapshot.revision, locale) }}
        </p>
        <p class="break-all">
          {{ recordsCopy.commandIdentity }} {{ review.state.review.snapshot.commandId }}
        </p>
        <p>{{ recordsCopy.localActorHelp }}</p>
        <p>
          {{ recordsCopy.changeCount }}
          {{ formatNumber(review.state.review.changes.totalItems, locale) }}
        </p>
        <ul class="field-stack">
          <li v-for="item in review.state.review.changes.items" :key="item.ordinal">
            <p>
              {{ recordsCopy.changeKinds[item.change.kind] }} · <code>{{ item.change.path }}</code>
            </p>
            <details>
              <summary>{{ copy.currentValue }}</summary>
              <pre class="wrap-anywhere whitespace-pre-wrap">{{
                nativeValue(item.change.before)
              }}</pre>
            </details>
            <details>
              <summary>{{ copy.proposedValue }}</summary>
              <pre class="wrap-anywhere whitespace-pre-wrap">{{
                nativeValue(item.change.after)
              }}</pre>
            </details>
          </li>
        </ul>
        <nav class="action-row" :aria-label="recordsCopy.changePages">
          <button
            type="button"
            :disabled="busy || review.state.redoRequired"
            @click="review.page(true)"
          >
            {{ recordsCopy.first }}</button
          ><button
            type="button"
            :disabled="
              busy || review.state.redoRequired || review.state.review.changes.continuation === null
            "
            @click="review.page()"
          >
            {{ recordsCopy.next }}
          </button>
        </nav>
        <template v-if="state.reviewed && state.reviewed.snapshot === review.state.review.snapshot">
          <p>
            {{ copy.reviewActions[state.reviewed.action] }} ·
            {{ state.reviewed.after?.id ?? state.reviewed.before?.id }}
          </p>
          <p>
            {{ copy.currentValue }}
            {{
              state.reviewed.before === null
                ? copy.absent
                : state.reviewed.before.active
                  ? copy.active
                  : copy.inactive
            }}
            → {{ copy.proposedValue }}
            {{
              state.reviewed.after === null
                ? copy.absent
                : state.reviewed.after.active
                  ? copy.active
                  : copy.inactive
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'minimumRest'">
            {{
              copy.minimumRestSentence(formatNumber(state.reviewed.after.minimumMinutes, locale))
            }}
          </p>
          <p>{{ copy.previewHelp }}</p>
          <dl v-if="state.reviewed.summary">
            <div>
              <dt>{{ copy.previewPeople }}</dt>
              <dd>{{ formatNumber(state.reviewed.summary.population.peopleCount, locale) }}</dd>
            </div>
            <template v-if="state.reviewed.summary.population.kind === 'minimumRest'"
              ><div>
                <dt>{{ copy.previewBeforeShifts }}</dt>
                <dd>
                  {{ formatNumber(state.reviewed.summary.population.beforeShiftCount, locale) }}
                </dd>
              </div>
              <div>
                <dt>{{ copy.previewAfterShifts }}</dt>
                <dd>
                  {{ formatNumber(state.reviewed.summary.population.afterShiftCount, locale) }}
                </dd>
              </div></template
            >
            <div v-else>
              <dt>{{ copy.previewShifts }}</dt>
              <dd>{{ formatNumber(state.reviewed.summary.population.shiftCount, locale) }}</dd>
            </div>
          </dl>
          <p v-if="state.reviewed.scopeError" role="alert">{{ state.reviewed.scopeError }}</p>
          <p v-if="!canApply && !busy && state.reviewed.after?.active" role="status">
            {{ copy.activeScopeRequired }}
          </p>
        </template>
        <section v-if="warnings.length" :aria-label="recordsCopy.warnings">
          <h4>{{ recordsCopy.warnings }}</h4>
          <ul>
            <li
              v-for="(warning, index) in warnings.slice(warningPage * 50, (warningPage + 1) * 50)"
              :key="index"
            >
              {{ warning.message }}
            </li>
          </ul>
          <button type="button" :disabled="warningPage === 0" @click="warningPage -= 1">
            {{ recordsCopy.previous }}</button
          ><button
            type="button"
            :disabled="(warningPage + 1) * 50 >= warnings.length"
            @click="warningPage += 1"
          >
            {{ recordsCopy.next }}
          </button>
        </section>
        <p v-if="review.state.redoRequired" role="alert">{{ copy.redoWarning }}</p>
        <button
          v-if="review.state.redoRequired"
          type="button"
          :disabled="!canApply"
          @click="save(true)"
        >
          {{ copy.confirmRedo }}
        </button>
        <button v-else type="button" :disabled="!canApply" @click="save()">{{ copy.save }}</button>
      </section>
    </template>
  </section>
</template>
