<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import { useRouter } from "vue-router";
import { SetupOperationScope, type FastFindingsV1, type ValidationIssue } from "../api/generated";
import { useFullValidation } from "../full-validation";
import { formatNumber, messages } from "../messages";
import { safeMessage, type ProjectHomeController, type ProjectSummary } from "../project-home";
import {
  issueAddress,
  resolveValidationAddress,
  validationRoute,
  type ResolvedValidationAddress,
  type ValidationContext,
  type ValidationNavigationTarget,
} from "../validation-navigation";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import ValidationFindingGroups from "./ValidationFindingGroups.vue";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  locale?: string;
}>();
const router = useRouter();
const vm = useFullValidation(props.home, () => props.project);
const { state, supported, busy } = vm;
const copy = messages.validationWorkspace;
const selectedHeading = ref<HTMLElement>();
const selected = shallowRef<{
  issue: ValidationIssue;
  context: ValidationContext;
  target: ValidationNavigationTarget | null;
  resolved: ResolvedValidationAddress | null;
  pending: boolean;
  error: string | null;
} | null>(null);
let current = true;
let generation = 0;
let selectionScope: SetupOperationScope | null = null;
const findings = computed<FastFindingsV1 | null>(() => {
  if (state.report === null) return null;
  const counts = { errors: 0, warnings: 0, information: 0 };
  for (const issue of state.report.report.issues)
    counts[
      issue.severity === "error"
        ? "errors"
        : issue.severity === "warning"
          ? "warnings"
          : "information"
    ] += 1;
  return { counts, issues: state.report.report.issues, omitted: 0 };
});
const full = computed(() => state.status?.full ?? null);
const canRun = computed(
  () => !busy.value && state.run !== "running" && full.value?.state !== "running",
);
const previousReport = computed(() => state.run !== "completed");

function matches(context: ValidationContext): boolean {
  return (
    current &&
    props.project.scenarioId === context.scenarioId &&
    props.project.revision === context.revision &&
    props.home.state.libraryEpoch === context.libraryEpoch
  );
}
function clearSelection(): void {
  generation += 1;
  selectionScope?.dispose();
  selectionScope = null;
  selected.value = null;
}
async function selectIssue(issue: ValidationIssue): Promise<void> {
  clearSelection();
  const captured = generation;
  const context = {
    scenarioId: props.project.scenarioId,
    revision: props.project.revision,
    libraryEpoch: props.home.state.libraryEpoch,
  };
  const address = issueAddress(issue);
  const target = address === null ? null : { ...context, ...address };
  const selection = {
    issue,
    context,
    target,
    resolved: null,
    pending: address !== null,
    error: null,
  };
  selected.value = selection;
  const valid = () => captured === generation && matches(context);
  await nextTick();
  if (!valid()) return;
  selectedHeading.value?.focus();
  if (address === null) return;
  let owned: SetupOperationScope | null = null;
  try {
    owned = new SetupOperationScope(context.scenarioId, context.revision);
    selectionScope = owned;
    const resolved = await resolveValidationAddress(owned, address);
    if (!valid()) return;
    selected.value = { ...selection, resolved, pending: false };
  } catch (failure) {
    if (valid()) selected.value = { ...selection, pending: false, error: safeMessage(failure) };
  } finally {
    owned?.dispose();
    if (valid()) selectionScope = null;
  }
}
async function openTarget(): Promise<void> {
  const selection = selected.value;
  if (selection?.target == null || selection.resolved?.route == null) return;
  if (!matches(selection.context)) {
    selected.value = { ...selection, error: copy.targetStale };
    return;
  }
  try {
    await router.push(validationRoute(selection.target, selection.resolved.route));
  } catch (failure) {
    if (selected.value === selection && matches(selection.context))
      selected.value = { ...selection, error: safeMessage(failure) };
  }
}
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
  ],
  clearSelection,
  { flush: "sync" },
);
onScopeDispose(() => {
  current = false;
  clearSelection();
});
</script>

<template>
  <section class="page-stack" aria-labelledby="validation-heading">
    <h2 id="validation-heading" data-route-heading tabindex="-1">{{ copy.heading }}</h2>
    <p v-if="!supported">{{ messages.setup.unsupported }}</p>
    <template v-else>
      <p>{{ copy.boundary }}</p>
      <div class="action-row">
        <button type="button" :disabled="!canRun" @click="vm.run">{{ copy.run }}</button>
        <button
          v-if="state.run === 'running'"
          type="button"
          :disabled="home.state.operation?.cancellationRequested"
          @click="home.cancelOperation"
        >
          {{ copy.cancel }}
        </button>
        <button type="button" :disabled="state.loading" @click="vm.refreshStatus">
          {{ copy.refresh }}
        </button>
      </div>
      <p v-if="state.loading" role="status">{{ messages.setup.loading }}</p>
      <p v-if="state.error" class="text-danger" role="alert">{{ state.error }}</p>
      <section class="state-panel field-stack" aria-labelledby="validation-state-heading">
        <h3 id="validation-state-heading">{{ messages.setup.full }}</h3>
        <p v-if="state.run === 'running'" role="status" data-full-validation-state="running">
          {{ home.state.operation?.cancellationRequested ? copy.cancelling : copy.running }}
        </p>
        <p
          v-else-if="state.run === 'cancelled'"
          role="status"
          data-full-validation-state="cancelled"
        >
          {{ copy.cancelled }}
        </p>
        <p v-else-if="state.run === 'failed'" role="alert" data-full-validation-state="failed">
          {{ copy.failed }}
        </p>
        <p v-else-if="full" role="status" :data-full-validation-state="full.state">
          {{ messages.setup.validationStates[full.state] }}
        </p>
        <p v-if="state.failure" class="text-danger">{{ state.failure }}</p>
        <template v-if="full && full.state !== 'notRun'">
          <p>{{ copy.reportedStatus }}: {{ messages.setup.validationStates[full.state] }}</p>
          <p>{{ messages.setup.validationRevision(formatNumber(full.inputRevision, locale)) }}</p>
          <p
            v-if="full.stale || full.inputRevision !== project.revision"
            class="text-danger"
            role="status"
            data-full-validation-stale
          >
            {{ messages.setup.staleValidation }}
          </p>
        </template>
        <p>{{ copy.completedBoundary }}</p>
      </section>
      <template v-if="state.status">
        <p>{{ copy.fastBoundary }}</p>
        <ValidationFindingGroups
          :home="home"
          :project="project"
          :findings="state.status.fast"
          :heading="messages.setup.fast"
          v-bind="locale === undefined ? {} : { locale }"
          @select-issue="selectIssue"
        />
        <p v-if="state.status.fast.omitted > 0">
          {{ messages.setup.omitted(formatNumber(state.status.fast.omitted, locale)) }}
        </p>
      </template>
      <template v-if="findings && state.report">
        <p v-if="previousReport">{{ copy.previousReport }}</p>
        <p v-else>{{ copy.completed(formatNumber(state.report.revision, locale)) }}</p>
        <ValidationFindingGroups
          :home="home"
          :project="project"
          :findings="findings"
          :heading="copy.report"
          v-bind="locale === undefined ? {} : { locale }"
          @select-issue="selectIssue"
        />
      </template>
      <p v-else-if="full?.state === 'completed'">{{ copy.reportUnavailable }}</p>
      <section
        v-if="selected"
        class="state-panel field-stack"
        aria-labelledby="validation-selected-heading"
      >
        <h3 id="validation-selected-heading" ref="selectedHeading" tabindex="-1">
          {{ copy.selected }}
        </h3>
        <p>{{ selected.issue.message }}</p>
        <p class="monospace">{{ selected.issue.code }}</p>
        <p v-if="selected.issue.fieldPath" class="monospace">{{ selected.issue.fieldPath }}</p>
        <p v-if="selected.pending" role="status">{{ copy.resolving }}</p>
        <p v-if="selected.resolved">{{ copy.affected }}: {{ selected.resolved.label }}</p>
        <p v-if="selected.error" class="text-danger" role="alert">{{ selected.error }}</p>
        <button v-if="selected.resolved?.route" type="button" @click="openTarget">
          {{ copy.open }}
        </button>
        <p v-else-if="!selected.pending && !selected.error">{{ copy.unsupportedTarget }}</p>
        <p>{{ copy.noAutomaticFix }}</p>
      </section>
    </template>
    <RouteLeaveGuard
      :home="home"
      :dirty="false"
      :pending="state.run === 'running'"
      :discard="clearSelection"
    />
  </section>
</template>
