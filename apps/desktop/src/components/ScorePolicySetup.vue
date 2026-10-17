<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { formatNumber, messages } from "../messages";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import { useScorePolicySetup } from "../score-policy-setup";
import {
  createInitialLevelDraft,
  emptyBreakpointDraft,
  emptyExplicitTargetDraft,
  emptyWorkloadPolicyDraft,
  type ScorePolicyDraft,
  type ScorePolicyPenaltyDraft,
  type ScorePolicyPeopleDraft,
  type ScorePolicyTargetModeDraft,
  type ScorePolicyValue,
} from "../score-policy-draft";
import type { EntityField } from "../entity-draft";
import { useValidationRoute } from "../validation-route";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";

const props = defineProps<{
  readonly home: ProjectHomeController;
  readonly project: ProjectSummary;
  readonly locale?: string;
}>();
const copy = messages.scorePolicy;
const vm = useScorePolicySetup(props.home, () => props.project);
const { state, review, busy, dirty, stale, conflictsCurrent } = vm;
const editorHeading = ref<HTMLElement>();
const listHeading = ref<HTMLElement>();
const reviewHeading = ref<HTMLElement>();
const conflictHeading = ref<HTMLElement>();
const currentHeading = ref<HTMLElement>();
const editorHost = ref<HTMLElement>();
const form = ref<HTMLFormElement>();
const showErrors = ref(false);
const showCurrent = ref(false);
const supported = computed(() => props.project.domainPackId === "official.workforce");
const editorRaw = computed(() => state.editor?.raw ?? null);
const currentPolicy = computed(() => state.editor?.current ?? null);
const reviewChanges = computed(() => review.state.review?.changes.items ?? []);
const reviewWarnings = computed(() => {
  const value = review.state.review?.warnings;
  return value === undefined ? [] : [...value.changes, ...value.proposed];
});
const nativeErrors = computed(() => Object.entries(state.errors));

function text(event: Event): string {
  return (event.target as HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement).value;
}
function json(value: unknown): string {
  return JSON.stringify(value, null, 2);
}
function tagText(values: readonly string[]): string {
  return values.join(", ");
}
function fieldValue(value: ScorePolicyValue, field: EntityField<ScorePolicyValue>): unknown {
  return Reflect.get(value, field);
}
function errorFor(path: string): string | undefined {
  return state.errors[path] ?? state.errors[`/${path.replaceAll(".", "/")}`];
}
function update(raw: ScorePolicyDraft): void {
  showErrors.value = false;
  vm.updateRaw(raw);
}
function withRaw(change: (raw: ScorePolicyDraft) => ScorePolicyDraft): void {
  const raw = editorRaw.value;
  if (raw !== null) update(change(raw));
}
function updateLevel(index: number, field: "levelKey" | "label", value: string): void {
  withRaw((raw) => ({
    ...raw,
    levels: raw.levels.map((level, item) =>
      item !== index
        ? level
        : field === "levelKey"
          ? { ...level, levelKey: value }
          : { ...level, label: value },
    ),
  }));
}
function addLevel(): void {
  withRaw((raw) =>
    raw.levels.length >= 15 ? raw : { ...raw, levels: [...raw.levels, createInitialLevelDraft()] },
  );
}
function removeLevel(index: number): void {
  withRaw((raw) =>
    raw.levels.length <= 1
      ? raw
      : { ...raw, levels: raw.levels.filter((_, item) => item !== index) },
  );
}
function moveLevel(index: number, delta: -1 | 1): void {
  withRaw((raw) => {
    const target = index + delta;
    if (target < 0 || target >= raw.levels.length) return raw;
    const levels = [...raw.levels];
    const current = levels[index];
    const other = levels[target];
    if (current === undefined || other === undefined) return raw;
    levels[index] = other;
    levels[target] = current;
    return { ...raw, levels };
  });
}
function updateMapping(
  index: number,
  field: "levelKey" | "priority" | "scale",
  value: string,
): void {
  if (field === "levelKey") {
    withRaw((raw) => ({
      ...raw,
      priorityMapping: raw.priorityMapping.map((mapping, item) =>
        item === index ? { ...mapping, levelKey: value } : mapping,
      ),
    }));
    return;
  }
  if (field === "scale") {
    withRaw((raw) => ({
      ...raw,
      priorityMapping: raw.priorityMapping.map((mapping, item) =>
        item === index ? { ...mapping, scale: value } : mapping,
      ),
    }));
    return;
  }
  const priority =
    value === "low"
      ? "low"
      : value === "normal"
        ? "normal"
        : value === "high"
          ? "high"
          : value === "veryHigh"
            ? "veryHigh"
            : null;
  if (priority === null) return;
  withRaw((raw) => ({
    ...raw,
    priorityMapping: raw.priorityMapping.map((mapping, item) =>
      item === index ? { ...mapping, priority } : mapping,
    ),
  }));
}
function updatePolicy(
  index: number,
  change: (
    policy: ScorePolicyDraft["workloadPolicies"][number],
  ) => ScorePolicyDraft["workloadPolicies"][number],
): void {
  withRaw((raw) => ({
    ...raw,
    workloadPolicies: raw.workloadPolicies.map((policy, item) =>
      item === index ? change(policy) : policy,
    ),
  }));
}
function addPolicy(): void {
  withRaw((raw) => ({
    ...raw,
    workloadPolicies: [...raw.workloadPolicies, emptyWorkloadPolicyDraft()],
  }));
}
function removePolicy(index: number): void {
  withRaw((raw) => ({
    ...raw,
    workloadPolicies: raw.workloadPolicies.filter((_, item) => item !== index),
  }));
}
function setPolicyText(
  index: number,
  field: "bucketId" | "calendarId" | "membership",
  value: string,
): void {
  if (field === "bucketId") {
    updatePolicy(index, (policy) => ({ ...policy, bucketId: value }));
    return;
  }
  if (field === "calendarId") {
    updatePolicy(index, (policy) => ({ ...policy, calendarId: value }));
    return;
  }
  const membership =
    value === ""
      ? ""
      : value === "reportingDate" || value === "startInstant" || value === "intersection"
        ? value
        : null;
  if (membership === null) return;
  updatePolicy(index, (policy) => ({ ...policy, membership }));
}
function setPeopleKind(index: number, kind: string): void {
  if (kind !== "all" && kind !== "selected" && kind !== "filter") return;
  const people: ScorePolicyPeopleDraft =
    kind === "all"
      ? { kind: "all" }
      : kind === "selected"
        ? { kind: "selected", personIds: [] }
        : { kind: "filter", allTags: [], anyTags: [] };
  updatePolicy(index, (policy) => ({ ...policy, peerGroup: { ...policy.peerGroup, people } }));
}
function setPeopleIds(index: number, ids: readonly string[]): void {
  updatePolicy(index, (policy) => ({
    ...policy,
    peerGroup: {
      ...policy.peerGroup,
      people: { kind: "selected", personIds: ids },
    },
  }));
}
function setTags(index: number, field: "allTags" | "anyTags", value: string): void {
  updatePolicy(index, (policy) => {
    const people = policy.peerGroup.people;
    if (people.kind !== "filter") return policy;
    const nextPeople =
      field === "allTags" ? { ...people, allTags: [value] } : { ...people, anyTags: [value] };
    return { ...policy, peerGroup: { ...policy.peerGroup, people: nextPeople } };
  });
}
function setTeamIds(index: number, ids: readonly string[]): void {
  updatePolicy(index, (policy) => ({
    ...policy,
    peerGroup: { ...policy.peerGroup, teamIds: ids },
  }));
}
function setTargetMode(index: number, kind: string): void {
  let targetMode: ScorePolicyTargetModeDraft | null = null;
  if (kind === "explicit") targetMode = { kind: "explicit", targets: [] };
  else if (kind === "personTargets") targetMode = { kind: "personTargets" };
  else if (kind === "weightedEqualShare") targetMode = { kind: "weightedEqualShare" };
  else if (kind === "equalShare") targetMode = { kind: "equalShare" };
  updatePolicy(index, (policy) => ({ ...policy, targetMode }));
}
function addTarget(index: number): void {
  updatePolicy(index, (policy) => {
    if (policy.targetMode?.kind !== "explicit") return policy;
    return {
      ...policy,
      targetMode: {
        ...policy.targetMode,
        targets: [...policy.targetMode.targets, emptyExplicitTargetDraft()],
      },
    };
  });
}
function removeTarget(index: number, targetIndex: number): void {
  updatePolicy(index, (policy) => {
    if (policy.targetMode?.kind !== "explicit") return policy;
    return {
      ...policy,
      targetMode: {
        ...policy.targetMode,
        targets: policy.targetMode.targets.filter((_, item) => item !== targetIndex),
      },
    };
  });
}
function setTarget(
  index: number,
  targetIndex: number,
  field: "personId" | "target",
  value: string,
): void {
  updatePolicy(index, (policy) => {
    if (policy.targetMode?.kind !== "explicit") return policy;
    const targets = policy.targetMode.targets.map((target, item) => {
      if (item !== targetIndex) return target;
      return field === "personId" ? { ...target, personId: value } : { ...target, target: value };
    });
    return { ...policy, targetMode: { ...policy.targetMode, targets } };
  });
}
function setPenaltyMode(index: number, kind: string): void {
  const penalty: ScorePolicyPenaltyDraft | null =
    kind === "absolute"
      ? { kind: "absolute" }
      : kind === "piecewise"
        ? { kind: "piecewise", breakpoints: [] }
        : null;
  updatePolicy(index, (policy) => ({ ...policy, penalty }));
}
function addBreakpoint(index: number): void {
  updatePolicy(index, (policy) => {
    if (policy.penalty?.kind !== "piecewise") return policy;
    return {
      ...policy,
      penalty: {
        ...policy.penalty,
        breakpoints: [...policy.penalty.breakpoints, emptyBreakpointDraft()],
      },
    };
  });
}
function removeBreakpoint(index: number, breakpointIndex: number): void {
  updatePolicy(index, (policy) => {
    if (policy.penalty?.kind !== "piecewise") return policy;
    return {
      ...policy,
      penalty: {
        ...policy.penalty,
        breakpoints: policy.penalty.breakpoints.filter((_, item) => item !== breakpointIndex),
      },
    };
  });
}
function setBreakpoint(
  index: number,
  breakpointIndex: number,
  field: "fromDeviation" | "slope",
  value: string,
): void {
  updatePolicy(index, (policy) => {
    if (policy.penalty?.kind !== "piecewise") return policy;
    const breakpoints = policy.penalty.breakpoints.map((breakpoint, item) => {
      if (item !== breakpointIndex) return breakpoint;
      return field === "fromDeviation"
        ? { ...breakpoint, fromDeviation: value }
        : { ...breakpoint, slope: value };
    });
    return { ...policy, penalty: { ...policy.penalty, breakpoints } };
  });
}
async function focusEditor(): Promise<void> {
  await nextTick();
  editorHeading.value?.focus();
}
async function openPolicy(id: string): Promise<void> {
  if (await vm.openPolicy(id)) await focusEditor();
}
async function createPolicy(): Promise<void> {
  vm.create();
  if (state.editor?.creating) await focusEditor();
}
async function edit(): Promise<void> {
  vm.edit();
  await focusEditor();
}
async function close(): Promise<void> {
  vm.discard();
  showCurrent.value = false;
  await nextTick();
  listHeading.value?.focus();
}
async function preview(): Promise<void> {
  showErrors.value = true;
  if (form.value?.reportValidity() === false) return;
  if (await vm.previewEditor()) {
    await nextTick();
    reviewHeading.value?.focus();
  }
}
async function returnToDraft(): Promise<void> {
  review.invalidate();
  await focusEditor();
}
async function rebase(): Promise<void> {
  showErrors.value = true;
  await vm.rebase();
  await nextTick();
  if (state.editor?.rebase) conflictHeading.value?.focus();
  else editorHeading.value?.focus();
}
function chooseConflict(field: EntityField<ScorePolicyValue>, choice: "current" | "draft"): void {
  vm.chooseConflict(field, choice);
}
async function apply(truncateRedo = false): Promise<void> {
  const focused = document.activeElement;
  if (await vm.apply(truncateRedo)) {
    await nextTick(() => {
      if (focused === document.activeElement || document.activeElement === document.body)
        listHeading.value?.focus();
    });
  }
}
async function inspectCurrent(): Promise<void> {
  if (currentPolicy.value === null) return;
  showCurrent.value = true;
  await nextTick();
  currentHeading.value?.focus();
}
function fieldLabel(field: string): string {
  if (field === "profileKey") return copy.profileKey;
  if (field === "levels") return copy.levels;
  if (field === "priorityMapping") return copy.priorityMapping;
  if (field === "workloadPolicies") return copy.workloadPolicies;
  return field;
}
function reviewPage(first = false): void {
  void review.page(first);
}
function focusValidationField(path: readonly string[], isCurrent: () => boolean): boolean {
  if (!isCurrent()) return false;
  const first = path[0] ?? "";
  const ids: Readonly<Record<string, string>> = {
    profileKey: "score-policy-profile-key",
    tieBreak: "score-policy-tie-break",
    levels: "score-policy-levels",
    priorityMapping: "score-policy-priority-mapping",
    workloadPolicies: "score-policy-workload-policies",
  };
  const element = document.getElementById(ids[first] ?? "score-policy-editor-heading");
  if (!(element instanceof HTMLElement) || !editorHost.value?.contains(element)) return false;
  element.focus();
  return document.activeElement === element;
}
function editorIsEditing(): boolean {
  const editor = state.editor;
  return editor !== null && editor.editing;
}

const navigationError = useValidationRoute(
  props.home,
  () => props.project,
  () => supported.value && !state.pageLoading,
  async (target, isCurrent) => {
    if (
      target.collection !== "entities" ||
      target.id === "" ||
      dirty.value ||
      busy.value ||
      review.state.review !== null
    )
      return false;
    if (state.editor?.id !== target.id) {
      if (!(await vm.loadDetail(target.id))) return false;
    }
    if (!isCurrent() || state.editor?.id !== target.id || state.editor.editing) return false;
    await edit();
    if (!isCurrent() || !editorIsEditing()) return false;
    await nextTick();
    return focusValidationField(target.fieldPath, isCurrent);
  },
);
watch(
  () => state.editor?.rebase?.conflicts.length,
  (value) => {
    if (value !== undefined && value > 0) void nextTick(() => conflictHeading.value?.focus());
  },
);
</script>

<template>
  <section ref="editorHost" class="page-stack" aria-labelledby="score-policy-heading">
    <h2 id="score-policy-heading" data-route-heading tabindex="-1">{{ copy.heading }}</h2>
    <p v-if="navigationError" class="text-danger" role="alert">{{ navigationError }}</p>
    <p v-if="!supported" role="alert">{{ copy.unsupported }}</p>
    <template v-else>
      <RouteLeaveGuard
        :home="home"
        :dirty="dirty || review.state.review !== null"
        :pending="busy"
        :discard="vm.discard"
      />
      <p>{{ copy.description }}</p>
      <div v-if="review.state.review !== null" class="state-panel field-stack" aria-live="polite">
        <h3 id="score-policy-review-heading" ref="reviewHeading" tabindex="-1">
          {{ copy.review }}
        </h3>
        <p>{{ copy.reviewHelp }}</p>
        <p>
          {{ copy.revision }}:
          {{ formatNumber(review.state.review.snapshot.revision, locale) }}
        </p>
        <p class="break-all">{{ copy.identity }}: {{ review.state.review.snapshot.target?.id }}</p>
        <div class="action-row">
          <button type="button" :disabled="busy || review.state.redoRequired" @click="apply(false)">
            {{ copy.apply }}
          </button>
          <button type="button" :disabled="busy" @click="returnToDraft">
            {{ copy.backToDraft }}
          </button>
          <button
            v-if="review.state.redoRequired"
            type="button"
            :disabled="busy"
            @click="apply(true)"
          >
            {{ copy.redo }}
          </button>
        </div>
        <p v-if="review.state.error" role="alert">{{ review.state.error }}</p>
        <p v-if="review.state.review.changes.items.length === 0">{{ copy.noChanges }}</p>
        <table v-else>
          <caption>
            {{
              copy.changes
            }}
          </caption>
          <thead>
            <tr>
              <th scope="col">#</th>
              <th scope="col">Path</th>
              <th scope="col">Before</th>
              <th scope="col">After</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="item in reviewChanges" :key="item.ordinal">
              <td>{{ item.ordinal }}</td>
              <td>
                <code>{{ item.change.path }}</code>
              </td>
              <td>
                <pre>{{ json(item.change.before) }}</pre>
              </td>
              <td>
                <pre>{{ json(item.change.after) }}</pre>
              </td>
            </tr>
          </tbody>
        </table>
        <nav class="action-row" :aria-label="copy.changePage">
          <button type="button" :disabled="busy" @click="reviewPage(true)">{{ copy.first }}</button>
          <button
            type="button"
            :disabled="busy || review.state.review.changes.continuation === null"
            @click="reviewPage()"
          >
            {{ copy.next }}
          </button>
        </nav>
        <details v-if="reviewWarnings.length" open>
          <summary>{{ copy.warnings }}</summary>
          <ul>
            <li v-for="warning in reviewWarnings" :key="`${warning.code}-${warning.message}`">
              {{ warning.message }}
            </li>
          </ul>
        </details>
        <details v-if="review.state.review.proposed !== null" class="setup-more">
          <summary>{{ copy.proposed }}</summary>
          <pre class="break-all">{{ json(review.state.review.proposed) }}</pre>
        </details>
      </div>
      <section
        v-else-if="state.editor === null"
        class="state-panel field-stack"
        aria-labelledby="score-policy-list-heading"
      >
        <h3 id="score-policy-list-heading" ref="listHeading" tabindex="-1">{{ copy.current }}</h3>
        <p v-if="state.pageLoading" role="status" aria-busy="true">{{ copy.loading }}</p>
        <div v-else-if="state.pageError" role="alert">
          <p>{{ state.pageError }}</p>
          <button type="button" :disabled="busy" @click="vm.refresh">{{ copy.retry }}</button>
        </div>
        <template v-else-if="state.page !== null && state.page.items.length === 0">
          <p>{{ copy.empty }}</p>
          <button type="button" :disabled="busy" @click="createPolicy">{{ copy.create }}</button>
          <button type="button" :disabled="busy" @click="vm.refresh">{{ copy.refresh }}</button>
        </template>
        <template v-else-if="state.page !== null">
          <p v-if="state.page.items.length > 1" role="alert">{{ copy.singletonMismatch }}</p>
          <ul>
            <li v-for="item in state.page.items" :key="item.entityId">
              <button
                type="button"
                :disabled="busy || state.page.items.length > 1"
                @click="openPolicy(item.entityId)"
              >
                {{ item.name ?? item.entityId }}
              </button>
              <code class="break-all">{{ item.entityId }}</code>
            </li>
          </ul>
          <p v-if="state.detailLoading" role="status">{{ copy.loadingDetail }}</p>
          <p v-if="state.detailError" role="alert">{{ state.detailError }}</p>
          <button type="button" :disabled="busy" @click="vm.refresh">{{ copy.refresh }}</button>
        </template>
      </section>
      <section
        v-else-if="state.editor !== null"
        class="state-panel field-stack"
        aria-labelledby="score-policy-editor-heading"
      >
        <h3 id="score-policy-editor-heading" ref="editorHeading" tabindex="-1">
          {{ state.editor.creating ? copy.create : copy.edit }}
        </h3>
        <p class="break-all">{{ copy.identity }}: {{ state.editor.id }}</p>
        <p v-if="review.state.error" role="alert">{{ review.state.error }}</p>
        <p v-if="state.detailError" role="alert">{{ state.detailError }}</p>
        <p v-if="state.pageError" role="alert">{{ state.pageError }}</p>
        <button
          v-if="state.pageError || state.detailError"
          type="button"
          :disabled="busy"
          @click="vm.refresh"
        >
          {{ copy.retry }}
        </button>
        <div v-if="!state.editor.editing" class="action-row">
          <button type="button" :disabled="busy || stale" @click="edit">{{ copy.edit }}</button>
          <button type="button" :disabled="busy" @click="vm.refresh">{{ copy.refresh }}</button>
          <button type="button" :disabled="busy" @click="close">{{ copy.close }}</button>
        </div>
        <p v-if="stale" role="alert">{{ copy.stale }}</p>
        <p v-if="stale">{{ state.editor.creating ? copy.createStale : copy.rebaseHelp }}</p>
        <div v-if="stale" class="action-row">
          <button
            v-if="state.editor.editing && !state.editor.creating"
            type="button"
            :disabled="busy"
            @click="rebase"
          >
            {{ copy.rebase }}
          </button>
          <button type="button" :disabled="busy" @click="vm.refresh">{{ copy.refresh }}</button>
          <button
            v-if="!state.editor.creating"
            type="button"
            :disabled="busy"
            @click="inspectCurrent"
          >
            {{ copy.current }}
          </button>
        </div>
        <section
          v-if="state.editor.rebase !== null && conflictsCurrent"
          class="state-panel"
          aria-labelledby="score-policy-conflicts-heading"
        >
          <h4 id="score-policy-conflicts-heading" ref="conflictHeading" tabindex="-1">
            {{ copy.conflicts }}
          </h4>
          <p>{{ copy.conflictHelp }}</p>
          <fieldset v-for="field in state.editor.rebase.conflicts" :key="String(field)">
            <legend>{{ fieldLabel(String(field)) }}</legend>
            <p>{{ copy.current }}:</p>
            <pre class="break-all">{{ json(fieldValue(state.editor.rebase.current, field)) }}</pre>
            <p>{{ copy.keepDraft }}:</p>
            <pre class="break-all">{{ json(fieldValue(state.editor.rebase.value, field)) }}</pre>
            <div class="action-row">
              <button type="button" :disabled="busy" @click="chooseConflict(field, 'current')">
                {{ copy.useCurrent }}
              </button>
              <button type="button" :disabled="busy" @click="chooseConflict(field, 'draft')">
                {{ copy.keepDraft }}
              </button>
            </div>
          </fieldset>
        </section>
        <details v-if="showCurrent && currentPolicy !== null" open class="state-panel">
          <summary ref="currentHeading" tabindex="-1">{{ copy.current }}</summary>
          <pre class="break-all">{{ json(currentPolicy) }}</pre>
        </details>
        <form
          v-if="state.editor.editing"
          ref="form"
          class="field-stack"
          novalidate
          @submit.prevent="preview"
        >
          <fieldset :disabled="busy" class="form-fields field-stack">
            <label for="score-policy-profile-key">{{ copy.profileKey }}</label>
            <p id="score-policy-profile-key-help" class="field-help">{{ copy.profileKeyHelp }}</p>
            <input
              id="score-policy-profile-key"
              name="profileKey"
              required
              maxlength="64"
              :value="state.editor.raw.profileKey"
              :aria-invalid="showErrors && Boolean(errorFor('profileKey'))"
              aria-describedby="score-policy-profile-key-help"
              @input="withRaw((raw) => ({ ...raw, profileKey: text($event) }))"
            />
            <p v-if="showErrors && errorFor('profileKey')" class="text-danger" role="alert">
              {{ errorFor("profileKey") }}
            </p>
            <label for="score-policy-tie-break">{{ copy.tieBreak }}</label>
            <select
              id="score-policy-tie-break"
              disabled
              :value="state.editor.raw.tieBreak"
              aria-describedby="score-policy-tie-break-help"
            >
              <option value="stableAssignmentRank">{{ copy.stableAssignmentRank }}</option>
            </select>
            <p id="score-policy-tie-break-help" class="field-help">{{ copy.tieBreakHelp }}</p>

            <fieldset id="score-policy-levels" class="field-stack" tabindex="-1">
              <legend>{{ copy.levels }}</legend>
              <p>{{ copy.levelsHelp }}</p>
              <div
                v-for="(level, index) in state.editor.raw.levels"
                :key="level.key"
                class="state-panel field-stack"
              >
                <h4>{{ copy.levelNumber(String(index + 1)) }}</h4>
                <label :for="`score-level-${level.key}-key`">{{ copy.levelKey }}</label>
                <input
                  :id="`score-level-${level.key}-key`"
                  required
                  maxlength="64"
                  :value="level.levelKey"
                  @input="updateLevel(index, 'levelKey', text($event))"
                />
                <label :for="`score-level-${level.key}-label`">{{ copy.levelLabel }}</label>
                <input
                  :id="`score-level-${level.key}-label`"
                  required
                  maxlength="256"
                  :value="level.label"
                  @input="updateLevel(index, 'label', text($event))"
                />
                <div class="action-row">
                  <button
                    type="button"
                    :disabled="busy || index === 0"
                    @click="moveLevel(index, -1)"
                  >
                    {{ copy.moveLevelUp }}
                  </button>
                  <button
                    type="button"
                    :disabled="busy || index === state.editor.raw.levels.length - 1"
                    @click="moveLevel(index, 1)"
                  >
                    {{ copy.moveLevelDown }}
                  </button>
                  <button
                    type="button"
                    :disabled="busy || state.editor.raw.levels.length <= 1"
                    @click="removeLevel(index)"
                  >
                    {{ copy.removeLevel }}
                  </button>
                </div>
              </div>
              <button
                type="button"
                :disabled="busy || state.editor.raw.levels.length >= 15"
                @click="addLevel"
              >
                {{ copy.addLevel }}
              </button>
            </fieldset>

            <fieldset id="score-policy-priority-mapping" class="field-stack" tabindex="-1">
              <legend>{{ copy.priorityMapping }}</legend>
              <p>{{ copy.priorityMappingHelp }}</p>
              <div
                v-for="(mapping, index) in state.editor.raw.priorityMapping"
                :key="mapping.key"
                class="state-panel field-stack"
              >
                <h4>{{ copy.mappingNumber(String(index + 1)) }}</h4>
                <label :for="`score-mapping-${mapping.key}-level`">{{ copy.mappingLevel }}</label>
                <select
                  :id="`score-mapping-${mapping.key}-level`"
                  required
                  :value="mapping.levelKey"
                  @change="updateMapping(index, 'levelKey', text($event))"
                >
                  <option value="">{{ copy.mappingLevel }}</option>
                  <option
                    v-for="level in state.editor.raw.levels"
                    :key="level.key"
                    :value="level.levelKey"
                  >
                    {{ level.label }} ({{ level.levelKey }})
                  </option>
                </select>
                <label :for="`score-mapping-${mapping.key}-priority`">{{
                  copy.mappingPriority
                }}</label>
                <select
                  :id="`score-mapping-${mapping.key}-priority`"
                  :value="mapping.priority"
                  @change="updateMapping(index, 'priority', text($event))"
                >
                  <option value="low">{{ copy.priority.low }}</option>
                  <option value="normal">{{ copy.priority.normal }}</option>
                  <option value="high">{{ copy.priority.high }}</option>
                  <option value="veryHigh">{{ copy.priority.veryHigh }}</option>
                </select>
                <label :for="`score-mapping-${mapping.key}-scale`">{{ copy.mappingScale }}</label>
                <input
                  :id="`score-mapping-${mapping.key}-scale`"
                  inputmode="numeric"
                  required
                  :value="mapping.scale"
                  @input="updateMapping(index, 'scale', text($event))"
                />
              </div>
            </fieldset>

            <fieldset id="score-policy-workload-policies" class="field-stack" tabindex="-1">
              <legend>{{ copy.workloadPolicies }}</legend>
              <p>{{ copy.workloadPoliciesHelp }}</p>
              <p v-if="state.editor.raw.workloadPolicies.length === 0">
                {{ copy.noWorkloadPolicies }}
              </p>
              <article
                v-for="(policy, index) in state.editor.raw.workloadPolicies"
                :key="policy.key"
                class="state-panel field-stack"
              >
                <h4>{{ copy.workloadPolicyNumber(String(index + 1)) }}</h4>
                <p class="break-all">{{ copy.workloadIdentity }}: {{ policy.id }}</p>
                <p class="field-help">{{ copy.workloadIdentityHelp }}</p>
                <WorkforceEntityPicker
                  :id="`score-policy-${policy.key}-bucket`"
                  :label="copy.bucket"
                  :description="copy.bucketHelp"
                  kind="workloadBucket"
                  :project="project"
                  :library-epoch="home.state.libraryEpoch"
                  :model-value="policy.bucketId === '' ? [] : [policy.bucketId]"
                  @update:model-value="setPolicyText(index, 'bucketId', $event[0] ?? '')"
                />
                <WorkforceEntityPicker
                  :id="`score-policy-${policy.key}-calendar`"
                  :label="copy.calendar"
                  :description="copy.calendarHelp"
                  kind="calendar"
                  :project="project"
                  :library-epoch="home.state.libraryEpoch"
                  :model-value="policy.calendarId === '' ? [] : [policy.calendarId]"
                  @update:model-value="setPolicyText(index, 'calendarId', $event[0] ?? '')"
                />
                <label :for="`score-policy-${policy.key}-membership`">{{ copy.membership }}</label>
                <select
                  :id="`score-policy-${policy.key}-membership`"
                  required
                  :value="policy.membership"
                  @change="setPolicyText(index, 'membership', text($event))"
                >
                  <option value="">{{ copy.membership }}</option>
                  <option value="reportingDate">{{ copy.membershipOptions.reportingDate }}</option>
                  <option value="startInstant">{{ copy.membershipOptions.startInstant }}</option>
                  <option value="intersection">{{ copy.membershipOptions.intersection }}</option>
                </select>
                <fieldset class="field-stack">
                  <legend>{{ copy.peerGroup }}</legend>
                  <p>{{ copy.peerGroupHelp }}</p>
                  <label :for="`score-policy-${policy.key}-people-mode`">{{
                    copy.peopleMode
                  }}</label>
                  <select
                    :id="`score-policy-${policy.key}-people-mode`"
                    :value="policy.peerGroup.people.kind"
                    @change="setPeopleKind(index, text($event))"
                  >
                    <option value="all">{{ copy.peopleModes.all }}</option>
                    <option value="selected">{{ copy.peopleModes.selected }}</option>
                    <option value="filter">{{ copy.peopleModes.filter }}</option>
                  </select>
                  <WorkforceEntityPicker
                    v-if="policy.peerGroup.people.kind === 'selected'"
                    :id="`score-policy-${policy.key}-people`"
                    :label="copy.people"
                    :description="copy.peopleHelp"
                    kind="person"
                    multiple
                    :project="project"
                    :library-epoch="home.state.libraryEpoch"
                    :model-value="policy.peerGroup.people.personIds"
                    @update:model-value="setPeopleIds(index, $event)"
                  />
                  <template v-if="policy.peerGroup.people.kind === 'filter'">
                    <label :for="`score-policy-${policy.key}-all-tags`">{{ copy.allTags }}</label>
                    <input
                      :id="`score-policy-${policy.key}-all-tags`"
                      :value="tagText(policy.peerGroup.people.allTags)"
                      :aria-describedby="`score-policy-${policy.key}-tags-help`"
                      @input="setTags(index, 'allTags', text($event))"
                    />
                    <label :for="`score-policy-${policy.key}-any-tags`">{{ copy.anyTags }}</label>
                    <input
                      :id="`score-policy-${policy.key}-any-tags`"
                      :value="tagText(policy.peerGroup.people.anyTags)"
                      :aria-describedby="`score-policy-${policy.key}-tags-help`"
                      @input="setTags(index, 'anyTags', text($event))"
                    />
                    <p :id="`score-policy-${policy.key}-tags-help`" class="field-help">
                      {{ copy.tagsHelp }}
                    </p>
                  </template>
                  <WorkforceEntityPicker
                    :id="`score-policy-${policy.key}-teams`"
                    :label="copy.teams"
                    :description="copy.teamsHelp"
                    kind="team"
                    multiple
                    :project="project"
                    :library-epoch="home.state.libraryEpoch"
                    :model-value="policy.peerGroup.teamIds"
                    @update:model-value="setTeamIds(index, $event)"
                  />
                </fieldset>
                <fieldset class="field-stack">
                  <legend>{{ copy.targetMode }}</legend>
                  <p>{{ copy.targetModeHelp }}</p>
                  <label :for="`score-policy-${policy.key}-target-mode`">{{
                    copy.targetMode
                  }}</label>
                  <select
                    :id="`score-policy-${policy.key}-target-mode`"
                    required
                    :value="policy.targetMode?.kind ?? ''"
                    @change="setTargetMode(index, text($event))"
                  >
                    <option value="">{{ copy.targetModes.choose }}</option>
                    <option value="explicit">{{ copy.targetModes.explicit }}</option>
                    <option value="personTargets">{{ copy.targetModes.personTargets }}</option>
                    <option value="weightedEqualShare">
                      {{ copy.targetModes.weightedEqualShare }}
                    </option>
                    <option value="equalShare">{{ copy.targetModes.equalShare }}</option>
                  </select>
                  <template v-if="policy.targetMode?.kind === 'explicit'">
                    <h5>{{ copy.explicitTargets }}</h5>
                    <p>{{ copy.explicitTargetsHelp }}</p>
                    <div
                      v-for="(target, targetIndex) in policy.targetMode.targets"
                      :key="target.key"
                      class="state-panel field-stack"
                    >
                      <h6>{{ copy.targetNumber(String(targetIndex + 1)) }}</h6>
                      <WorkforceEntityPicker
                        :id="`score-policy-${policy.key}-${target.key}-person`"
                        :label="copy.targetPerson"
                        kind="person"
                        :project="project"
                        :library-epoch="home.state.libraryEpoch"
                        :model-value="target.personId === '' ? [] : [target.personId]"
                        @update:model-value="
                          setTarget(index, targetIndex, 'personId', $event[0] ?? '')
                        "
                      />
                      <label :for="`score-policy-${policy.key}-${target.key}-value`">{{
                        copy.targetValue
                      }}</label>
                      <input
                        :id="`score-policy-${policy.key}-${target.key}-value`"
                        inputmode="numeric"
                        required
                        :value="target.target"
                        @input="setTarget(index, targetIndex, 'target', text($event))"
                      />
                      <button
                        type="button"
                        :disabled="busy"
                        @click="removeTarget(index, targetIndex)"
                      >
                        {{ copy.removeTarget }}
                      </button>
                    </div>
                    <button type="button" :disabled="busy" @click="addTarget(index)">
                      {{ copy.addTarget }}
                    </button>
                  </template>
                </fieldset>
                <fieldset class="field-stack">
                  <legend>{{ copy.penalty }}</legend>
                  <p>{{ copy.penaltyHelp }}</p>
                  <label :for="`score-policy-${policy.key}-penalty`">{{ copy.penalty }}</label>
                  <select
                    :id="`score-policy-${policy.key}-penalty`"
                    required
                    :value="policy.penalty?.kind ?? ''"
                    @change="setPenaltyMode(index, text($event))"
                  >
                    <option value="">{{ copy.penaltyModes.choose }}</option>
                    <option value="absolute">{{ copy.penaltyModes.absolute }}</option>
                    <option value="piecewise">{{ copy.penaltyModes.piecewise }}</option>
                  </select>
                  <template v-if="policy.penalty?.kind === 'piecewise'">
                    <h5>{{ copy.breakpoints }}</h5>
                    <p>{{ copy.breakpointHelp }}</p>
                    <div
                      v-for="(breakpoint, breakpointIndex) in policy.penalty.breakpoints"
                      :key="breakpoint.key"
                      class="state-panel field-stack"
                    >
                      <h6>{{ copy.breakpointNumber(String(breakpointIndex + 1)) }}</h6>
                      <label :for="`score-policy-${policy.key}-${breakpoint.key}-from`">{{
                        copy.fromDeviation
                      }}</label>
                      <input
                        :id="`score-policy-${policy.key}-${breakpoint.key}-from`"
                        inputmode="numeric"
                        required
                        :value="breakpoint.fromDeviation"
                        @input="
                          setBreakpoint(index, breakpointIndex, 'fromDeviation', text($event))
                        "
                      />
                      <label :for="`score-policy-${policy.key}-${breakpoint.key}-slope`">{{
                        copy.slope
                      }}</label>
                      <input
                        :id="`score-policy-${policy.key}-${breakpoint.key}-slope`"
                        inputmode="numeric"
                        required
                        :value="breakpoint.slope"
                        @input="setBreakpoint(index, breakpointIndex, 'slope', text($event))"
                      />
                      <button
                        type="button"
                        :disabled="busy"
                        @click="removeBreakpoint(index, breakpointIndex)"
                      >
                        {{ copy.removeBreakpoint }}
                      </button>
                    </div>
                    <button
                      type="button"
                      :disabled="busy || policy.penalty.breakpoints.length >= 32"
                      @click="addBreakpoint(index)"
                    >
                      {{ copy.addBreakpoint }}
                    </button>
                  </template>
                </fieldset>
                <button type="button" :disabled="busy" @click="removePolicy(index)">
                  {{ copy.removeWorkloadPolicy }}
                </button>
              </article>
              <button type="button" :disabled="busy" @click="addPolicy">
                {{ copy.addWorkloadPolicy }}
              </button>
            </fieldset>
            <p v-if="showErrors && nativeErrors.length" role="alert">{{ copy.invalid }}</p>
            <ul v-if="showErrors && nativeErrors.length" class="text-danger">
              <li v-for="[path, message] in nativeErrors" :key="path">
                <code>{{ path }}</code
                >: {{ message }}
              </li>
            </ul>
            <div class="action-row">
              <button type="submit" :disabled="busy || stale || state.editor.rebase !== null">
                {{ copy.review }}
              </button>
              <button type="button" :disabled="busy" @click="close">{{ copy.close }}</button>
            </div>
          </fieldset>
        </form>
      </section>
    </template>
  </section>
</template>
