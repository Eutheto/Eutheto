<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import { useEligibilitySetup } from "../eligibility-setup";
import { formatNumber, messages } from "../messages";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import WorkforceAssignmentInspection from "./WorkforceAssignmentInspection.vue";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";
import EligibilityMatrix from "./eligibility/EligibilityMatrix.vue";
import EligibilityBulkReview from "./eligibility/EligibilityBulkReview.vue";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  locale?: string;
}>();
const vm = useEligibilitySetup(props.home, () => props.project);
const { state, review, busy, dirty, stale, unresolved, pendingCells } = vm;
const matrixHeading = ref<HTMLElement>(),
  reviewHeading = ref<HTMLElement>(),
  errorHeading = ref<HTMLElement>();
const inspector = ref<InstanceType<typeof WorkforceAssignmentInspection>>();
const warningPage = ref(0);
const warnings = computed(() => review.state.review?.warnings.changes ?? []);
watch(
  () => review.state.review,
  () => {
    warningPage.value = 0;
  },
);
function select(personId: string, typeId: string): void {
  state.selectedPersonId = personId;
  state.selectedTypeId = typeId;
}
async function inspect(personId: string): Promise<void> {
  state.selectedPersonId = personId;
  await nextTick();
  inspector.value?.focus();
}
async function load(axis: "people" | "types" | null = null, search = false): Promise<void> {
  matrixHeading.value?.focus();
  await vm.loadWindow(axis, search);
}
async function discard(): Promise<void> {
  matrixHeading.value?.focus();
  vm.discard();
  await vm.loadWindow();
}
async function focusAfter(action: Promise<boolean>, target: "review" | "matrix"): Promise<void> {
  const origin = document.activeElement;
  const accepted = await action;
  await nextTick();
  if (document.activeElement === origin || document.activeElement === document.body)
    (accepted
      ? target === "review"
        ? reviewHeading.value
        : matrixHeading.value
      : (errorHeading.value ?? matrixHeading.value)
    )?.focus();
}
</script>

<template>
  <section class="page-stack" aria-labelledby="eligibility-setup-heading">
    <h2 id="eligibility-setup-heading" ref="matrixHeading" data-route-heading tabindex="-1">
      {{ messages.eligibilityUi.heading }}
    </h2>
    <template v-if="project.domainPackId === 'official.workforce'">
      <p>{{ messages.eligibilityUi.description }}</p>
      <form class="state-panel field-stack" @submit.prevent="load(null, true)">
        <label for="eligibility-people-search">{{
          messages.eligibilityUi.filters.peopleSearch
        }}</label
        ><input
          id="eligibility-people-search"
          v-model="state.peopleSearch"
          type="search"
          :disabled="dirty || busy || unresolved"
        />
        <WorkforceEntityPicker
          :label="messages.eligibilityUi.filters.qualificationGrant"
          :description="messages.eligibilityUi.filters.qualificationGrantDescription"
          kind="qualification"
          :project="project"
          :library-epoch="home.state.libraryEpoch"
          :model-value="state.qualificationId === null ? [] : [state.qualificationId]"
          :disabled="dirty || busy || unresolved"
          @update:model-value="state.qualificationId = $event[0] ?? null"
        />
        <label for="eligibility-type-search">{{ messages.eligibilityUi.filters.typeSearch }}</label
        ><input
          id="eligibility-type-search"
          v-model="state.typeSearch"
          type="search"
          :disabled="dirty || busy || unresolved"
        />
        <button type="submit" :disabled="dirty || busy || unresolved">
          {{ messages.eligibilityUi.filters.submit }}
        </button>
        <p v-if="dirty">{{ messages.eligibilityUi.filters.dirty }}</p>
      </form>
      <p v-if="state.loading" role="status">{{ messages.eligibilityUi.loading }}</p>
      <section v-if="state.error || review.state.error" class="state-panel" role="alert">
        <h3 ref="errorHeading" tabindex="-1">{{ messages.eligibilityUi.errorHeading }}</h3>
        <p>{{ state.error ?? review.state.error }}</p>
      </section>
      <div v-if="unresolved" class="state-panel" role="alert">
        <p>{{ messages.eligibilityUi.unresolved }}</p>
        <button type="button" :disabled="busy" @click="vm.refresh">
          {{ messages.eligibilityUi.refresh }}
        </button>
      </div>
      <p v-if="stale" role="status">{{ messages.eligibilityUi.stale }}</p>
      <template v-if="state.window !== null">
        <p>
          {{
            messages.eligibilityUi.capturedRevision(
              formatNumber(state.window.context.revision, locale),
            )
          }}
        </p>
        <EligibilityMatrix
          :window="state.window"
          :edits="state.edits"
          :disabled="busy || stale || unresolved"
          :selected-person-id="state.selectedPersonId"
          :selected-type-id="state.selectedTypeId"
          v-bind="locale === undefined ? {} : { locale }"
          @select="select"
          @inspect="inspect"
          @cell="(person, type, allowed) => vm.setMembership([person], [type], allowed)"
          @row="
            (person, allowed) =>
              vm.setMembership(
                [person],
                state.window?.matrix.assignmentTypeIds ?? [],
                allowed,
                true,
              )
          "
          @column="
            (type, allowed) =>
              vm.setMembership(state.window?.matrix.personIds ?? [], [type], allowed, true)
          "
        />
        <nav class="action-row" :aria-label="messages.eligibilityUi.axisPages">
          <button type="button" :disabled="dirty || busy || unresolved" @click="load()">
            {{ messages.eligibilityUi.firstAxisPages }}
          </button>
          <button
            type="button"
            :disabled="
              dirty || busy || stale || unresolved || state.window.people.continuation === null
            "
            @click="load('people')"
          >
            {{ messages.eligibilityUi.nextPeoplePage }}
          </button>
          <button
            type="button"
            :disabled="
              dirty || busy || stale || unresolved || state.window.types.continuation === null
            "
            @click="load('types')"
          >
            {{ messages.eligibilityUi.nextTypePage }}
          </button>
        </nav>
      </template>
      <button
        v-else-if="!state.loading"
        type="button"
        :disabled="busy || dirty || unresolved"
        @click="load()"
      >
        {{ messages.eligibilityUi.reload }}
      </button>
      <section
        v-if="dirty"
        class="state-panel field-stack"
        :aria-label="messages.eligibilityUi.pending.label"
      >
        <p>{{ messages.eligibilityUi.pending.help }}</p>
        <p>
          {{
            messages.eligibilityUi.pending.baseline(
              formatNumber(
                state.approval?.context.revision ??
                  state.window?.context.revision ??
                  project.revision,
                locale,
              ),
            )
          }}
        </p>
        <EligibilityBulkReview
          :cells="state.approval?.cells ?? pendingCells"
          :removable="!busy && !unresolved"
          v-bind="locale === undefined ? {} : { locale }"
          @remove="vm.removeChoice"
        />
        <div class="action-row">
          <button
            type="button"
            :disabled="busy || unresolved"
            @click="focusAfter(vm.preview(), 'review')"
          >
            {{
              stale
                ? messages.eligibilityUi.pending.reviewCurrent
                : messages.eligibilityUi.pending.review
            }}</button
          ><button type="button" :disabled="busy || unresolved" @click="discard">
            {{ messages.eligibilityUi.pending.discard }}
          </button>
        </div>
      </section>
      <section
        v-if="review.state.review !== null && state.approval !== null"
        class="state-panel field-stack"
        aria-labelledby="eligibility-review-heading"
      >
        <h3 id="eligibility-review-heading" ref="reviewHeading" tabindex="-1">
          {{ messages.eligibilityUi.review.heading }}
        </h3>
        <p>
          {{
            messages.eligibilityUi.review.summary(
              formatNumber(review.state.review.snapshot.revision, locale),
              formatNumber(review.state.review.changes.totalItems, locale),
            )
          }}
        </p>
        <ul>
          <li v-for="item in review.state.review.changes.items" :key="item.ordinal">
            {{
              messages.eligibilityUi.change(
                messages.eligibilityUi.changeKinds[item.change.kind],
                item.change.path,
              )
            }}
          </li>
        </ul>
        <nav class="action-row" :aria-label="messages.eligibilityUi.review.changePages">
          <button
            type="button"
            :disabled="busy || review.state.redoRequired"
            @click="focusAfter(review.page(true), 'review')"
          >
            {{ messages.eligibilityUi.review.firstPage }}</button
          ><button
            type="button"
            :disabled="
              busy || review.state.redoRequired || review.state.review.changes.continuation === null
            "
            @click="focusAfter(review.page(), 'review')"
          >
            {{ messages.eligibilityUi.review.nextPage }}
          </button>
        </nav>
        <section v-if="warnings.length" :aria-label="messages.eligibilityUi.review.warnings">
          <p>
            {{ messages.eligibilityUi.review.warningCount(formatNumber(warnings.length, locale)) }}
          </p>
          <ul>
            <li
              v-for="(warning, index) in warnings.slice(warningPage * 20, (warningPage + 1) * 20)"
              :key="index"
            >
              {{ warning.code }}: {{ warning.message }}
            </li>
          </ul>
          <div class="action-row">
            <button type="button" :disabled="warningPage === 0" @click="warningPage -= 1">
              {{ messages.eligibilityUi.review.previousWarnings }}</button
            ><button
              type="button"
              :disabled="(warningPage + 1) * 20 >= warnings.length"
              @click="warningPage += 1"
            >
              {{ messages.eligibilityUi.review.nextWarnings }}
            </button>
          </div>
        </section>
        <div v-if="review.state.redoRequired" role="alert">
          <p>{{ messages.eligibilityUi.review.redoWarning }}</p>
          <button
            type="button"
            :disabled="busy || unresolved"
            @click="focusAfter(vm.save(true), 'matrix')"
          >
            {{ messages.eligibilityUi.review.confirmRedo }}
          </button>
        </div>
        <button
          v-else
          type="button"
          :disabled="busy || unresolved"
          @click="focusAfter(vm.save(), 'matrix')"
        >
          {{ messages.eligibilityUi.review.apply }}
        </button>
        <RouterLink :to="{ name: 'project-history', params: { scenarioId: project.scenarioId } }">{{
          messages.eligibilityUi.review.history
        }}</RouterLink>
      </section>
      <WorkforceAssignmentInspection
        v-if="state.selectedPersonId !== null"
        ref="inspector"
        :home="home"
        :project="project"
        :person-id="state.selectedPersonId"
        v-bind="locale === undefined ? {} : { locale }"
      />
    </template>
    <p v-else>{{ messages.eligibilityUi.unsupported }}</p>
    <RouteLeaveGuard :home="home" :dirty="dirty" :pending="busy" :discard="vm.discard" />
  </section>
</template>
