<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type { WorkforceAvailability } from "../api/generated-domain-pack-contracts";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import { useAvailabilitySetup } from "../availability-setup";
import { messages } from "../messages";
import AvailabilityFields from "./availability/AvailabilityFields.vue";
import AvailabilityCalendar from "./availability/AvailabilityCalendar.vue";
import { availabilityMessages as copy, AVAILABILITY_KIND_LABELS } from "./availability/messages";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";
import WorkforceAssignmentInspection from "./WorkforceAssignmentInspection.vue";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import { getScenarioEntity, SetupOperationScope } from "../api/generated";
import { useValidationRoute } from "../validation-route";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  locale?: string;
}>();
const vm = useAvailabilitySetup(props.home, () => props.project);
const {
  state,
  review,
  facts,
  busy,
  dirty,
  stale,
  canReplace,
  unresolved,
  timeChanged,
  needsTimeApproval,
} = vm;
const editorHeading = ref<HTMLElement>();
const recordFields = ref<InstanceType<typeof AvailabilityFields>>();
const recordsHeading = ref<HTMLElement>();
const reviewHeading = ref<HTMLElement>();
const conflictHeading = ref<HTMLElement>();
const errorHeading = ref<HTMLElement>();
const focusTargets = { editor: editorHeading, review: reviewHeading, records: recordsHeading };
const warningsPage = ref(0);
const warnings = computed(() => {
  const value = review.state.review?.warnings;
  return value === undefined ? [] : [...value.changes, ...value.proposed];
});
const proposed = computed(() => {
  const value = review.state.review?.proposed;
  return value?.kind === "availability" ? value : null;
});
const fieldLabels = copy.fieldLabels;
watch(
  () => review.state.review,
  () => {
    warningsPage.value = 0;
  },
);
async function focusAfter(
  result: boolean | Promise<boolean>,
  target: keyof typeof focusTargets,
): Promise<void> {
  const origin = document.activeElement;
  const accepted = await result;
  await nextTick();
  if (document.activeElement === origin || document.activeElement === document.body)
    (accepted ? focusTargets[target].value : errorHeading.value)?.focus();
}
async function rebase(): Promise<void> {
  const origin = document.activeElement;
  const accepted = await vm.rebase();
  await nextTick();
  if (document.activeElement === origin || document.activeElement === document.body)
    (accepted ? (conflictHeading.value ?? editorHeading.value) : errorHeading.value)?.focus();
}
function discard(): void {
  recordsHeading.value?.focus();
  vm.discard();
}
function page(next: boolean): void {
  recordsHeading.value?.focus();
  void vm.loadRecords(next ? (state.records?.continuation ?? null) : null);
}
function conflict(field: keyof WorkforceAvailability, choice: "current" | "draft"): void {
  editorHeading.value?.focus();
  vm.chooseConflict(field, choice);
}
const navigationError = useValidationRoute(
  props.home,
  () => props.project,
  () => facts.value !== null,
  async (target, isCurrent) => {
    const replaceable = () => isCurrent() && canReplace.value;
    if (target.collection !== "entities" || !replaceable()) return false;
    const before = state.draftGeneration;
    const owned = new SetupOperationScope(target.scenarioId, target.revision);
    try {
      const { result } = await getScenarioEntity(owned, {
        kind: "availability",
        entityId: target.id,
      }).result;
      if (!replaceable() || before !== state.draftGeneration) return false;
      const record = result.view.data.result.data;
      if (record.kind !== "availability" || record.id !== target.id) return false;
      const selecting = vm.selectPerson(record.personId);
      const selectedGeneration = state.draftGeneration;
      await selecting;
      if (
        !replaceable() ||
        selectedGeneration !== state.draftGeneration ||
        state.personId !== record.personId
      )
        return false;
      const opening = vm.open(target.id);
      const openingGeneration = state.draftGeneration;
      if (!(await opening) || !isCurrent() || openingGeneration !== state.draftGeneration)
        return false;
      const opened = state.editor;
      if (opened?.id !== target.id || opened.raw.personId !== record.personId) return false;
      const valid = () => isCurrent() && state.editor === opened && !dirty.value && !busy.value;
      await nextTick();
      if (!valid()) return false;
      if (target.fieldPath.length === 0) {
        editorHeading.value?.focus();
        return document.activeElement === editorHeading.value;
      }
      return recordFields.value?.focusField(target.fieldPath, valid) ?? false;
    } finally {
      owned.dispose();
    }
  },
);
</script>

<template>
  <section class="page-stack" aria-labelledby="availability-heading">
    <h2 id="availability-heading" data-route-heading tabindex="-1">{{ copy.heading }}</h2>
    <p v-if="navigationError" class="text-danger" role="alert">{{ navigationError }}</p>
    <p>{{ copy.description }}</p>
    <template v-if="project.domainPackId === 'official.workforce'">
      <button type="button" :disabled="busy" @click="vm.refresh">{{ copy.refresh }}</button>
      <p v-if="state.factsLoading" role="status">{{ copy.factsLoading }}</p>
      <p v-if="state.factsError" role="alert">{{ state.factsError }}</p>
      <WorkforceEntityPicker
        id="availability-person"
        kind="person"
        :label="copy.person"
        :model-value="state.personId === null ? [] : [state.personId]"
        :project="project"
        :library-epoch="home.state.libraryEpoch"
        :disabled="!canReplace"
        v-bind="locale === undefined ? {} : { locale }"
        @update:model-value="vm.selectPerson($event[0] ?? null)"
      />
      <p v-if="dirty" class="field-help">{{ copy.dirtyHelp }}</p>
      <template v-if="state.personId !== null">
        <section class="state-panel field-stack" aria-labelledby="availability-records-heading">
          <h3 id="availability-records-heading" ref="recordsHeading" tabindex="-1">
            {{ copy.recordsHeading }}
          </h3>
          <p>{{ copy.recordsDescription }}</p>
          <button
            type="button"
            :disabled="!canReplace || facts === null"
            @click="focusAfter(vm.startAdd(), 'editor')"
          >
            {{ copy.add }}
          </button>
          <p v-if="state.recordsLoading" role="status">{{ copy.recordsLoading }}</p>
          <p v-if="state.recordsError" role="alert">{{ state.recordsError }}</p>
          <template v-if="state.records !== null">
            <p>{{ copy.authoredRecords(state.records.totalItems, locale) }}</p>
            <p v-if="state.records.totalItems === 0">{{ copy.recordsEmpty }}</p>
            <ul class="field-stack">
              <li
                v-for="record in state.records.items"
                :key="record.availabilityId"
                class="state-panel field-stack"
              >
                <p>
                  {{
                    copy.recordSummary(
                      AVAILABILITY_KIND_LABELS[record.availabilityKind],
                      record.effectiveRange.startDate,
                      record.effectiveRange.endDateExclusive,
                      locale,
                    )
                  }}
                </p>
                <code class="break-all">{{ record.availabilityId }}</code>
                <p v-if="record.availabilityKind === 'requestedTimeOff'" class="field-help">
                  {{ copy.importedKindHelp }}
                </p>
                <button
                  type="button"
                  :disabled="!canReplace || facts === null"
                  :aria-label="copy.editAvailability(record.availabilityId)"
                  @click="focusAfter(vm.open(record.availabilityId), 'editor')"
                >
                  {{ copy.editRecord }}
                </button>
              </li>
            </ul>
            <nav class="action-row" :aria-label="copy.recordsPages">
              <button type="button" @click="page(false)">{{ messages.people.first }}</button>
              <button
                type="button"
                :disabled="state.records.continuation === null"
                @click="page(true)"
              >
                {{ messages.people.next }}
              </button>
            </nav>
          </template>
        </section>
        <p v-if="state.detailLoading" role="status">{{ copy.loadingDetail }}</p>
        <section
          v-if="state.editorError || review.state.error || Object.keys(state.errors).length"
          class="state-panel field-stack"
          aria-labelledby="availability-errors-heading"
        >
          <h3 id="availability-errors-heading" ref="errorHeading" tabindex="-1">
            {{ copy.errorHeading }}
          </h3>
          <p v-if="state.editorError" role="alert">{{ state.editorError }}</p>
          <p v-if="review.state.error" role="alert">{{ review.state.error }}</p>
          <ul>
            <li v-for="(error, field) in state.errors" :key="field">{{ error }}</li>
          </ul>
        </section>
        <section
          v-if="state.editor !== null"
          class="state-panel field-stack"
          aria-labelledby="availability-editor-heading"
        >
          <h3 id="availability-editor-heading" ref="editorHeading" tabindex="-1">
            {{ state.editor.base === null ? copy.editorNewHeading : copy.editorHeading }}
          </h3>
          <p>{{ copy.recordIdentity(state.editor.id) }}</p>
          <p v-if="stale" role="status">{{ copy.stale }}</p>
          <div v-if="timeChanged && facts !== null" class="field-stack">
            <p>
              {{
                copy.timeSettingsChanged(
                  state.editor.time.timeZone,
                  state.editor.time.gapPolicy,
                  state.editor.time.overlapPolicy,
                  facts.settings.timeZone,
                  facts.settings.gapPolicy,
                  facts.settings.overlapPolicy,
                )
              }}
            </p>
            <label class="checkbox-label"
              ><input
                type="checkbox"
                :checked="!needsTimeApproval"
                :disabled="busy"
                @change="vm.approveTime(($event.target as HTMLInputElement).checked)"
              />{{ copy.approveTime }}</label
            >
          </div>
          <p v-if="unresolved" role="alert">{{ copy.unresolvedWrite }}</p>
          <form class="field-stack" @submit.prevent="focusAfter(vm.preview(), 'review')">
            <AvailabilityFields
              ref="recordFields"
              :draft="state.editor.raw"
              :project="project"
              :library-epoch="home.state.libraryEpoch"
              :time-zone="state.editor.time.timeZone"
              :request-key="`availability-${String(state.draftGeneration)}`"
              :errors="state.errors"
              :disabled="busy || state.editor.rebase !== null"
              v-bind="locale === undefined ? {} : { locale }"
              @change="vm.change"
            />
            <div class="action-row">
              <button
                type="submit"
                :disabled="busy || stale || unresolved || state.editor.rebase !== null"
              >
                {{ copy.reviewChanges }}
              </button>
              <button
                v-if="stale"
                type="button"
                :disabled="busy || facts === null || needsTimeApproval || unresolved"
                @click="rebase"
              >
                {{ copy.reviewCurrentChanges }}
              </button>
              <button type="button" :disabled="busy" @click="discard">{{ copy.discard }}</button>
              <button
                v-if="state.editor.base !== null"
                type="button"
                :disabled="busy || stale || dirty || unresolved"
                @click="focusAfter(vm.preview(true), 'review')"
              >
                {{ copy.reviewRemoval }}
              </button>
            </div>
          </form>
          <section
            v-if="state.editor.rebase !== null"
            class="field-stack"
            aria-labelledby="availability-conflicts-heading"
          >
            <h4 id="availability-conflicts-heading" ref="conflictHeading" tabindex="-1">
              {{ copy.conflictsHeading }}
            </h4>
            <p>{{ copy.conflictsHelp }}</p>
            <ul class="field-stack">
              <li v-for="field in state.editor.rebase.conflicts" :key="field" class="field-stack">
                <h5>{{ fieldLabels[field] }}</h5>
                <p>{{ copy.currentSavedValue }}</p>
                <pre class="whitespace-pre-wrap break-all">{{
                  JSON.stringify(state.editor.rebase.current[field], null, 2)
                }}</pre>
                <p>{{ copy.draftValue }}</p>
                <pre class="whitespace-pre-wrap break-all">{{
                  JSON.stringify(state.editor.rebase.value[field], null, 2)
                }}</pre>
                <div class="action-row">
                  <button
                    type="button"
                    :disabled="
                      busy ||
                      state.editor.rebaseContext?.revision !== project.revision ||
                      state.editor.rebaseContext?.epoch !== home.state.libraryEpoch
                    "
                    @click="conflict(field, 'current')"
                  >
                    {{ copy.useSavedField(fieldLabels[field]) }}
                  </button>
                  <button
                    type="button"
                    :disabled="
                      busy ||
                      state.editor.rebaseContext?.revision !== project.revision ||
                      state.editor.rebaseContext?.epoch !== home.state.libraryEpoch
                    "
                    @click="conflict(field, 'draft')"
                  >
                    {{ copy.keepDraftField(fieldLabels[field]) }}
                  </button>
                </div>
              </li>
            </ul>
          </section>
        </section>
        <section
          v-if="review.state.review !== null"
          class="state-panel field-stack"
          aria-labelledby="availability-review-heading"
        >
          <h3 id="availability-review-heading" ref="reviewHeading" tabindex="-1">
            {{ state.reviewAction === "remove" ? copy.reviewRemoval : copy.previewHeading }}
          </h3>
          <p>{{ copy.previewDescription }}</p>
          <p>{{ copy.capturedRevision(review.state.review.snapshot.revision, locale) }}</p>
          <p v-if="state.reviewAction === 'remove'">
            {{ copy.removeConfirm(state.editor?.id ?? "") }}
          </p>
          <template v-if="proposed !== null">
            <dl class="metadata-list">
              <div>
                <dt>{{ copy.proposedKind }}</dt>
                <dd>{{ AVAILABILITY_KIND_LABELS[proposed.availabilityKind] }}</dd>
              </div>
              <div>
                <dt>{{ copy.proposedPerson }}</dt>
                <dd>
                  <code>{{ proposed.personId }}</code>
                </dd>
              </div>
              <div>
                <dt>{{ copy.proposedEffectiveDates }}</dt>
                <dd>
                  {{
                    copy.effectiveDateRange(
                      proposed.effectiveRange.startDate,
                      proposed.effectiveRange.endDateExclusive,
                      locale,
                    )
                  }}
                </dd>
              </div>
              <div>
                <dt>{{ copy.proposedAssignmentTypes }}</dt>
                <dd>{{ copy.assignmentTypes(proposed.assignmentTypeIds) }}</dd>
              </div>
              <div>
                <dt>{{ copy.proposedLocations }}</dt>
                <dd>{{ copy.locations(proposed.locationIds) }}</dd>
              </div>
              <div>
                <dt>{{ copy.proposedSource }}</dt>
                <dd class="whitespace-pre-wrap break-words">{{ proposed.source }}</dd>
              </div>
              <div>
                <dt>{{ copy.proposedNote }}</dt>
                <dd class="whitespace-pre-wrap break-words">{{ proposed.note }}</dd>
              </div>
            </dl>
            <details>
              <summary>{{ copy.proposedTimeWindow }}</summary>
              <pre class="whitespace-pre-wrap break-all">{{
                JSON.stringify(proposed.timeWindow, null, 2)
              }}</pre>
            </details>
            <p v-if="proposed.availabilityKind === 'requestedTimeOff'">
              {{ copy.importedKindHelp }}
            </p>
          </template>
          <ul class="field-stack">
            <li v-for="item in review.state.review.changes.items" :key="item.ordinal">
              {{
                copy.reviewChange(messages.people.changeKinds[item.change.kind], item.change.path)
              }}
            </li>
          </ul>
          <p>{{ copy.nativeChanges(review.state.review.changes.totalItems, locale) }}</p>
          <nav class="action-row" :aria-label="copy.reviewChangePages">
            <button
              type="button"
              :disabled="busy || review.state.redoRequired"
              @click="focusAfter(review.page(true), 'review')"
            >
              {{ messages.people.first }}
            </button>
            <button
              type="button"
              :disabled="
                busy ||
                review.state.redoRequired ||
                review.state.review.changes.continuation === null
              "
              @click="focusAfter(review.page(), 'review')"
            >
              {{ messages.people.next }}
            </button>
          </nav>
          <section v-if="warnings.length" :aria-label="copy.reviewWarnings">
            <p>{{ copy.nativeWarnings(warnings.length, locale) }}</p>
            <ul>
              <li
                v-for="(warning, index) in warnings.slice(
                  warningsPage * 20,
                  (warningsPage + 1) * 20,
                )"
                :key="index"
              >
                {{ warning.code }}: {{ warning.message }}
              </li>
            </ul>
            <div class="action-row">
              <button type="button" :disabled="warningsPage === 0" @click="warningsPage -= 1">
                {{ copy.previousWarnings }}
              </button>
              <button
                type="button"
                :disabled="(warningsPage + 1) * 20 >= warnings.length"
                @click="warningsPage += 1"
              >
                {{ copy.nextWarnings }}
              </button>
            </div>
          </section>
          <div v-if="review.state.redoRequired" role="alert">
            <p>{{ messages.people.redoWarning }}</p>
            <button
              type="button"
              :disabled="busy || unresolved"
              @click="focusAfter(vm.save(true), 'records')"
            >
              {{ messages.people.confirmRedo }}
            </button>
          </div>
          <button
            v-else
            type="button"
            :disabled="busy || unresolved"
            @click="focusAfter(vm.save(), 'records')"
          >
            {{ copy.apply }}
          </button>
          <RouterLink :to="{ name: 'project-history', params: { scenarioId: project.scenarioId } }">
            {{ copy.projectHistory }}
          </RouterLink>
        </section>
        <AvailabilityCalendar
          v-if="facts !== null"
          :home="home"
          :project="project"
          :person-id="state.personId"
          :initial-work-window="facts.initialWorkWindow"
          :time-zone="facts.settings.timeZone"
          :disabled="!canReplace"
          v-bind="locale === undefined ? {} : { locale }"
          @edit="focusAfter(vm.open($event), 'editor')"
          @add="focusAfter(vm.startAdd($event), 'editor')"
        />
        <WorkforceAssignmentInspection
          :home="home"
          :project="project"
          :person-id="state.personId"
          v-bind="locale === undefined ? {} : { locale }"
        />
      </template>
      <p v-else>{{ copy.noSelection }}</p>
    </template>
    <p v-else>{{ copy.unsupportedProject }}</p>
    <RouteLeaveGuard
      :home="home"
      :dirty="dirty"
      :pending="review.state.pending || home.state.busyAction !== null"
      :discard="vm.discard"
    />
  </section>
</template>
