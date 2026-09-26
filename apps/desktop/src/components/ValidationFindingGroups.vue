<script setup lang="ts">
import { computed, onScopeDispose, ref, shallowRef, useId, watch } from "vue";
import { SetupOperationScope, type FastFindingsV1, type ValidationIssue } from "../api/generated";
import { formatNumber, messages } from "../messages";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import { issueAddress, resolveValidationAddress } from "../validation-navigation";
import ValidationSummary from "./explanations/ValidationSummary.vue";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  findings: FastFindingsV1;
  heading: string;
  locale?: string;
}>();
const emit = defineEmits<{ selectIssue: [issue: ValidationIssue] }>();
const copy = messages.validationWorkspace;
const prefix = useId();
const group = ref<"mustFix" | "likelyProblem" | "reviewSuggested" | "information">("mustFix");
const page = ref(0);
const labels = shallowRef<ReadonlyMap<ValidationIssue, string>>(new Map());
const namesUnavailable = ref(false);
let current = true;
let generation = 0;
let scope: SetupOperationScope | null = null;
const grouped = computed(() => {
  const result: Record<typeof group.value, ValidationIssue[]> = {
    mustFix: [],
    likelyProblem: [],
    reviewSuggested: [],
    information: [],
  };
  for (const issue of props.findings.issues)
    result[
      issue.severity === "error"
        ? "mustFix"
        : issue.severity === "warning"
          ? "reviewSuggested"
          : "information"
    ].push(issue);
  return result;
});
const counts = computed(() => ({
  mustFix: props.findings.counts.errors,
  likelyProblem: 0,
  reviewSuggested: props.findings.counts.warnings,
  information: props.findings.counts.information,
}));
const rows = computed(() => grouped.value[group.value]);
const offset = computed(
  () => Math.min(page.value, Math.max(0, Math.ceil(rows.value.length / 50) - 1)) * 50,
);
const visible = computed(() => rows.value.slice(offset.value, offset.value + 50));
const displayed = computed<FastFindingsV1>(() => ({
  counts: {
    errors: group.value === "mustFix" ? counts.value.mustFix : 0,
    warnings: group.value === "reviewSuggested" ? counts.value.reviewSuggested : 0,
    information: group.value === "information" ? counts.value.information : 0,
  },
  issues: visible.value,
  omitted: Math.max(0, counts.value[group.value] - visible.value.length),
}));
watch(
  [() => props.findings, group],
  () => {
    page.value = 0;
  },
  { flush: "sync" },
);
watch(
  [
    visible,
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
  ],
  async () => {
    const captured = ++generation;
    scope?.dispose();
    scope = null;
    labels.value = new Map();
    namesUnavailable.value = false;
    const issues = visible.value;
    if (issues.length === 0 || !current) return;
    const { scenarioId, revision } = props.project;
    const epoch = props.home.state.libraryEpoch;
    const valid = () =>
      current &&
      captured === generation &&
      props.project.scenarioId === scenarioId &&
      props.project.revision === revision &&
      props.home.state.libraryEpoch === epoch;
    let owned: SetupOperationScope | null = null;
    try {
      owned = new SetupOperationScope(scenarioId, revision);
      scope = owned;
      const queryScope = owned;
      const names = new Map<string, Promise<string>>();
      const resolved = new Map<ValidationIssue, string>();
      let next = 0;
      // At most two label reads per group; a rule label uses two native queries.
      const worker = async () => {
        while (valid() && next < issues.length) {
          const issue = issues[next++];
          if (issue === undefined) return;
          const address = issueAddress(issue);
          if (address === null) continue;
          const key =
            address.collection === "settings" ? "settings" : `${address.collection}:${address.id}`;
          let pending = names.get(key);
          if (pending === undefined) {
            pending = resolveValidationAddress(queryScope, address).then((value) => value.label);
            names.set(key, pending);
          }
          try {
            const label = await pending;
            if (!valid()) return;
            resolved.set(issue, label);
            labels.value = new Map(resolved);
          } catch {
            if (valid()) namesUnavailable.value = true;
          }
        }
      };
      await Promise.all([worker(), worker()]);
    } catch {
      if (valid()) namesUnavailable.value = true;
    } finally {
      owned?.dispose();
      if (valid()) scope = null;
    }
  },
  { immediate: true },
);
onScopeDispose(() => {
  current = false;
  generation += 1;
  scope?.dispose();
});
</script>

<template>
  <section class="state-panel field-stack" :aria-labelledby="`${prefix}-heading`">
    <h3 :id="`${prefix}-heading`">{{ heading }}</h3>
    <p>{{ copy.grouping }}</p>
    <label :for="`${prefix}-group`">{{ heading }}</label>
    <select :id="`${prefix}-group`" v-model="group">
      <option v-for="(label, key) in copy.groups" :key="key" :value="key">
        {{ label }} — {{ formatNumber(counts[key], locale) }}
      </option>
    </select>
    <p v-if="group === 'likelyProblem'">{{ copy.noLikelyProblem }}</p>
    <p v-else-if="rows.length === 0">
      {{
        counts[group] === 0
          ? copy.groupEmpty
          : messages.setup.omitted(formatNumber(counts[group], locale))
      }}
    </p>
    <template v-else>
      <ValidationSummary
        :findings="displayed"
        state="ready"
        interaction="selectable"
        presentation="embedded"
        :heading="copy.groups[group]"
        :heading-level="4"
        :locale="locale"
        :issue-labels="labels"
        @select-issue="emit('selectIssue', $event)"
      />
      <nav class="action-row" :aria-label="heading">
        <button type="button" :disabled="offset === 0" @click="page = Math.max(0, page - 1)">
          {{ copy.previousPage }}
        </button>
        <p role="status">
          {{
            copy.page(
              formatNumber(offset + 1, locale),
              formatNumber(offset + visible.length, locale),
              formatNumber(rows.length, locale),
            )
          }}
        </p>
        <button type="button" :disabled="offset + visible.length >= rows.length" @click="page += 1">
          {{ copy.nextPage }}
        </button>
      </nav>
    </template>
    <p v-if="namesUnavailable" role="status">{{ copy.namesUnavailable }}</p>
  </section>
</template>
