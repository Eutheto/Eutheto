<script setup lang="ts">
import { computed, reactive, useId, watch } from "vue";
import type {
  AssignmentComparisonV1,
  AssignmentProtectionStateV1,
  AssignmentValue,
  ComparisonBindingV1,
  DomainAssignment,
  Int64,
  MetricValue,
  Revision,
  RuleEvaluation,
  VerificationValue,
  RunTerminalOutcomeV1,
  ScoreLevelComparisonV1,
  ScoreComparisonUnavailableReasonV1,
  SolutionComparisonV2,
  Uint64,
  UuidV7,
} from "../api/generated";
import { Badge, Card } from "./ui";

const PAGE_SIZE = 20;

type ScoreLevel = ScoreLevelComparisonV1;
type ScoreCategory = ScoreLevel["categories"][number];
type PageKey =
  | "added"
  | "removed"
  | "changed"
  | "affected"
  | "rules"
  | "metrics"
  | "scoreLevels"
  | "protections";
type AddedAssignment = Extract<AssignmentComparisonV1, { readonly type: "added" }>;
type RemovedAssignment = Extract<AssignmentComparisonV1, { readonly type: "removed" }>;
type ChangedAssignment = Extract<AssignmentComparisonV1, { readonly type: "changed" }>;
const ruleSides = ["before", "after"] as const;

const props = withDefaults(
  defineProps<{
    comparison: SolutionComparisonV2;
    scenarioId: UuidV7;
    currentRevision: Revision;
    baseLabel?: string;
    candidateLabel?: string;
  }>(),
  { baseLabel: "Base accepted result", candidateLabel: "Candidate accepted result" },
);

const headingId = useId();
const sourceHeadingId = useId();
const unavailableHeadingId = useId();
const assignmentHeadingId = useId();
const addedHeadingId = useId();
const removedHeadingId = useId();
const changedHeadingId = useId();
const rulesHeadingId = useId();
const metricsHeadingId = useId();
const affectedHeadingId = useId();
const runsHeadingId = useId();
const scoreHeadingId = useId();
const protectionsHeadingId = useId();

const pages = reactive<Record<PageKey, number>>({
  added: 0,
  removed: 0,
  changed: 0,
  affected: 0,
  rules: 0,
  metrics: 0,
  scoreLevels: 0,
  protections: 0,
});
const categoryPages = reactive<Record<string, number>>({});
const numberFormatter = new Intl.NumberFormat();

const comparableResult = computed(() =>
  props.comparison.score.type === "comparable" ? props.comparison.score : null,
);
const unavailableReason = computed(() => {
  if (props.comparison.score.type !== "unavailable") return null;
  const reason: Record<ScoreComparisonUnavailableReasonV1, string> = {
    policyOrScopeMismatch:
      "Shared scoring meaning was not established for these source objective policies and cost scopes.",
    scoreShapeMismatch: "The accepted results use different score shapes.",
  };
  return reason[props.comparison.score.reason];
});
const details = computed(() => props.comparison);
const baseBinding = computed<ComparisonBindingV1>(() => details.value.base);
const candidateBinding = computed<ComparisonBindingV1>(() => details.value.candidate);
const runs = computed(() => details.value.runs);
const comparisonChecksum = computed(() => details.value.checksum);
const entityObservations = computed(
  () =>
    new Map(
      details.value.entityObservations.map((observation) => [
        `${observation.entity.kind}:${observation.entity.id}`,
        observation,
      ]),
    ),
);
const contextNotice = computed(() => {
  const bindings = [baseBinding.value, candidateBinding.value];
  if (bindings.some((binding) => binding.scenarioId !== props.scenarioId)) {
    return "A compared result belongs to a different scenario identity; this is historical evidence, not current scenario state.";
  }
  const revisions = bindings
    .map((binding) => binding.scenarioRevision)
    .filter((revision) => revision !== props.currentRevision);
  if (revisions.length) {
    return `The compared source revision${revisions.length === 1 ? "" : "s"} (${revisions.join(", ")}) differs from current scenario revision ${String(props.currentRevision)}. Historical source results are not a fresh check of today's scenario.`;
  }
  return null;
});

const added = computed<readonly AddedAssignment[]>(() =>
  details.value.assignments.filter((change): change is AddedAssignment => change.type === "added"),
);
const removed = computed<readonly RemovedAssignment[]>(() =>
  details.value.assignments.filter(
    (change): change is RemovedAssignment => change.type === "removed",
  ),
);
const changed = computed<readonly ChangedAssignment[]>(() =>
  details.value.assignments.filter(
    (change): change is ChangedAssignment => change.type === "changed",
  ),
);
const visibleAdded = computed(() => pageSlice(added.value, pages.added));
const visibleRemoved = computed(() => pageSlice(removed.value, pages.removed));
const visibleChanged = computed(() => pageSlice(changed.value, pages.changed));
const visibleAffected = computed(() => pageSlice(details.value.affectedEntities, pages.affected));
const visibleRules = computed(() => pageSlice(details.value.rules, pages.rules));
const visibleMetrics = computed(() => pageSlice(details.value.metrics, pages.metrics));
const visibleScoreLevels = computed(() =>
  pageSlice(comparableResult.value?.scoreLevels ?? [], pages.scoreLevels),
);
const visibleProtections = computed(() => pageSlice(details.value.protections, pages.protections));

const hasChanges = computed(() => {
  const value = details.value;
  return Boolean(
    value.assignments.length ||
    value.rules.length ||
    value.metrics.length ||
    value.affectedEntities.length ||
    value.protections.length ||
    (comparableResult.value && comparableResult.value.scoreLevels.length),
  );
});
const countAnnouncement = computed(() => {
  const scoreCount = comparableResult.value?.scoreLevels.length ?? 0;
  const semanticCount =
    details.value.assignments.length +
    details.value.rules.length +
    details.value.metrics.length +
    scoreCount +
    details.value.protections.length;
  return `${formatCount(semanticCount)} comparison entries: ${formatCount(added.value.length)} added, ${formatCount(removed.value.length)} removed, ${formatCount(changed.value.length)} changed assignments; ${formatCount(details.value.rules.length)} required rules, ${formatCount(details.value.metrics.length)} metrics, and ${formatCount(details.value.protections.length)} protection records. Run metadata is recorded for both sides; it does not authenticate historical solver proof.`;
});

function sourceLabel(entity: { kind: string; id: string }, side: "before" | "after"): string {
  const observation = entityObservations.value.get(`${entity.kind}:${entity.id}`);
  const label = side === "before" ? observation?.beforeLabel : observation?.afterLabel;
  return label === undefined ? "No source label recorded" : (label ?? "Not present");
}

function formatCount(value: number): string {
  return numberFormatter.format(value);
}

function formatInt64(value: Int64): string {
  try {
    return numberFormatter.format(BigInt(value));
  } catch {
    return value;
  }
}

function formatUint64(value: Uint64): string {
  return typeof value === "number" ? numberFormatter.format(value) : formatInt64(value);
}

function valueText(value: AssignmentValue): string {
  if (value.type === "boolean") return value.value ? "Yes" : "No";
  if (value.type === "integer") return formatInt64(value.value);
  if (value.type === "absent") return "Not assigned";
  return `Interval ${formatInt64(value.value.start)}–${formatInt64(value.value.end)} (duration ${formatInt64(value.value.duration)})`;
}

function assignmentIdentity(assignment: DomainAssignment): string {
  return `${assignment.entity.kind} ${assignment.entity.id}`;
}

function ruleStatus(evaluation: RuleEvaluation | null): string {
  if (evaluation === null) return "Not present";
  return evaluation.satisfied ? "Satisfied" : "Not satisfied";
}

function factValue(value: VerificationValue): string {
  if (value.type === "boolean") return value.value ? "Yes" : "No";
  if (value.type === "integer") return formatInt64(value.value);
  if (value.type === "entity") return `${value.value.kind} ${value.value.id}`;
  return value.value;
}

function factsText(facts: RuleEvaluation["expected"]): string {
  const entries = Object.entries(facts);
  return entries.length
    ? entries.map(([key, value]) => `${key.replaceAll("_", " ")}: ${factValue(value)}`).join("; ")
    : "None recorded";
}

function metricText(value: MetricValue | null): string {
  if (value === null) return "Not present";
  if (value.type === "integer") return formatInt64(value.value);
  return `${formatInt64(value.value.numerator)} / ${formatUint64(value.value.denominator)}`;
}

function protectionText(protection: AssignmentProtectionStateV1 | null): string {
  if (protection === null) return "Absent";
  if (protection.type === "unlocked") return "Unlocked";
  if (protection.type === "hard") return "Hard";
  return `Soft (weight ${formatCount(protection.weight)})`;
}

function outcomeText(outcome: RunTerminalOutcomeV1): string {
  if (outcome.type === "accepted") {
    return `Recorded accepted status: ${outcome.status}`;
  }
  if (outcome.type === "verificationAlarm")
    return `Recorded verification alarm (${outcome.diagnosticCode})`;
  if (outcome.type === "interrupted") return "Recorded interrupted outcome";
  const labels: Record<(typeof outcome)["status"], string> = {
    infeasible: "Recorded backend infeasible status",
    unbounded: "Recorded backend unbounded status",
    noSolutionWithinLimit: "Recorded no verified result within the limit",
    cancelled: "Recorded cancelled status",
    invalidModel: "Recorded invalid-model status",
    backendUnavailable: "Recorded backend-unavailable status",
    backendFailed: "Recorded backend-failed status",
  };
  return labels[outcome.status];
}

function pageCount(length: number): number {
  return Math.max(1, Math.ceil(length / PAGE_SIZE));
}

function pageSlice<T>(items: readonly T[], page: number): readonly T[] {
  return items.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);
}

function pageStatus(length: number, page: number): string {
  if (length === 0) return "No entries.";
  const first = page * PAGE_SIZE + 1;
  const last = Math.min((page + 1) * PAGE_SIZE, length);
  return `Showing ${formatCount(first)}–${formatCount(last)} of ${formatCount(length)}.`;
}
function categoryPage(levelId: string): number {
  return categoryPages[levelId] ?? 0;
}

function visibleCategories(level: ScoreLevel): readonly ScoreCategory[] {
  return pageSlice(level.categories, categoryPage(level.levelId));
}

function categoryPageCount(level: ScoreLevel): number {
  return pageCount(level.categories.length);
}

function setCategoryPage(levelId: string, direction: number, length: number): void {
  const current = categoryPage(levelId);
  categoryPages[levelId] = Math.min(pageCount(length) - 1, Math.max(0, current + direction));
}

function setPage(key: PageKey, direction: number, length: number): void {
  const lastPage = pageCount(length) - 1;
  pages[key] = Math.min(lastPage, Math.max(0, pages[key] + direction));
}

function resetPages(): void {
  for (const key of Object.keys(pages) as PageKey[]) pages[key] = 0;
  for (const key of Object.keys(categoryPages)) categoryPages[key] = 0;
}

watch(() => props.comparison, resetPages);
</script>

<template>
  <Card as="section" variant="surface" :aria-labelledby="headingId">
    <h2 :id="headingId" class="font-display text-lg font-bold text-ink">
      Accepted result comparison
    </h2>
    <p class="mt-2 text-sm text-muted" data-read-only-notice data-testid="read-only-notice">
      Viewing these accepted results is read-only; it does not apply either result to the scenario.
    </p>
    <p class="mt-2 break-words text-sm text-muted">
      {{ baseLabel }} <code>{{ baseBinding.acceptedResult.solutionId }}</code> (source revision
      {{ baseBinding.scenarioRevision }}) versus {{ candidateLabel }}
      <code>{{ candidateBinding.acceptedResult.solutionId }}</code> (source revision
      {{ candidateBinding.scenarioRevision }}).
    </p>
    <p v-if="contextNotice" class="mt-2 text-sm text-muted" role="status">
      {{ contextNotice }}
    </p>

    <details class="mt-5">
      <summary class="cursor-pointer font-semibold text-ink">
        Source identities and recorded run metadata
      </summary>
      <section class="mt-3" :aria-labelledby="sourceHeadingId">
        <h3 :id="sourceHeadingId" class="font-semibold text-ink">Source identities</h3>
        <div class="mt-3 grid gap-4 lg:grid-cols-2">
          <article class="rounded-md border border-line bg-raised p-3 text-sm">
            <h4 class="font-semibold text-ink">{{ baseLabel }}</h4>
            <dl class="mt-2 space-y-2 text-muted">
              <div>
                <dt class="font-semibold text-ink">Pack</dt>
                <dd>
                  <code class="break-all">{{ baseBinding.packId }}</code>
                </dd>
              </div>
              <div>
                <dt class="font-semibold text-ink">Scenario and source revision</dt>
                <dd>
                  <code class="break-all">{{ baseBinding.scenarioId }}</code> ·
                  {{ baseBinding.scenarioRevision }}
                </dd>
              </div>
              <div>
                <dt class="font-semibold text-ink">Source document hash</dt>
                <dd>
                  <code class="break-all">{{ baseBinding.documentHash }}</code>
                </dd>
              </div>
              <div>
                <dt class="font-semibold text-ink">Projection version</dt>
                <dd>{{ baseBinding.projectionVersion }}</dd>
              </div>
              <div>
                <dt class="font-semibold text-ink">Verification scope checksum</dt>
                <dd>
                  <code class="break-all">{{ baseBinding.verificationScopeChecksum }}</code>
                </dd>
              </div>
              <div>
                <dt class="font-semibold text-ink">Accepted result identity</dt>
                <dd>
                  <code class="break-all">{{ baseBinding.acceptedResult.solutionId }}</code> ·
                  <code class="break-all">{{ baseBinding.acceptedResult.resultChecksum }}</code>
                </dd>
              </div>
            </dl>
          </article>
          <article class="rounded-md border border-line bg-raised p-3 text-sm">
            <h4 class="font-semibold text-ink">{{ candidateLabel }}</h4>
            <dl class="mt-2 space-y-2 text-muted">
              <div>
                <dt class="font-semibold text-ink">Pack</dt>
                <dd>
                  <code class="break-all">{{ candidateBinding.packId }}</code>
                </dd>
              </div>
              <div>
                <dt class="font-semibold text-ink">Scenario and source revision</dt>
                <dd>
                  <code class="break-all">{{ candidateBinding.scenarioId }}</code> ·
                  {{ candidateBinding.scenarioRevision }}
                </dd>
              </div>
              <div>
                <dt class="font-semibold text-ink">Source document hash</dt>
                <dd>
                  <code class="break-all">{{ candidateBinding.documentHash }}</code>
                </dd>
              </div>
              <div>
                <dt class="font-semibold text-ink">Projection version</dt>
                <dd>{{ candidateBinding.projectionVersion }}</dd>
              </div>
              <div>
                <dt class="font-semibold text-ink">Verification scope checksum</dt>
                <dd>
                  <code class="break-all">{{ candidateBinding.verificationScopeChecksum }}</code>
                </dd>
              </div>
              <div>
                <dt class="font-semibold text-ink">Accepted result identity</dt>
                <dd>
                  <code class="break-all">{{ candidateBinding.acceptedResult.solutionId }}</code> ·
                  <code class="break-all">{{
                    candidateBinding.acceptedResult.resultChecksum
                  }}</code>
                </dd>
              </div>
            </dl>
          </article>
        </div>
        <p class="mt-3 text-xs text-muted">
          Comparison checksum: <code class="break-all">{{ comparisonChecksum }}</code>
        </p>
      </section>
    </details>

    <section
      v-if="unavailableReason"
      class="mt-5 rounded-md border border-danger bg-danger-soft p-4"
      :aria-labelledby="unavailableHeadingId"
      role="alert"
    >
      <h3 :id="unavailableHeadingId" class="font-semibold text-danger-strong">
        Score comparison unavailable
      </h3>
      <p class="mt-2 text-sm text-danger-strong">{{ unavailableReason }}</p>
      <p class="mt-2 text-sm text-danger-strong">
        The semantic changes below are shown without score values or result ordering.
      </p>
    </section>

    <p class="mt-4 text-sm text-muted" role="status" aria-live="polite" aria-atomic="true">
      {{ countAnnouncement }}
    </p>
    <p v-if="!hasChanges" class="mt-2 text-sm text-muted" role="status">
      No semantic changes were found between these accepted results.
    </p>

    <section class="mt-5" :aria-labelledby="assignmentHeadingId">
      <h3 :id="assignmentHeadingId" class="font-semibold text-ink">Assignment changes</h3>
      <div class="mt-3 grid gap-4 lg:grid-cols-3">
        <section :aria-labelledby="addedHeadingId">
          <h4 :id="addedHeadingId" class="font-semibold text-ink">
            Added assignments ({{ added.length }})
          </h4>
          <ul v-if="visibleAdded.length" class="mt-2 space-y-2">
            <li v-for="change in visibleAdded" :key="`added:${change.after.id}`">
              <article class="rounded-md border border-line bg-raised p-3 text-sm">
                <Badge variant="neutral">Added</Badge>
                <p class="mt-2 font-semibold text-ink">{{ assignmentIdentity(change.after) }}</p>
                <p class="break-all text-xs text-muted">{{ change.after.id }}</p>
                <p class="mt-1">Value: {{ valueText(change.after.value) }}</p>
              </article>
            </li>
          </ul>
          <p v-else class="mt-2 text-sm text-muted">No added assignments.</p>
          <nav
            v-if="pageCount(added.length) > 1"
            class="mt-3 flex items-center gap-2"
            aria-label="Added assignments pages"
          >
            <button
              type="button"
              :disabled="pages.added === 0"
              @click="setPage('added', -1, added.length)"
            >
              Previous
            </button>
            <span role="status">{{ pageStatus(added.length, pages.added) }}</span>
            <button
              type="button"
              :disabled="pages.added + 1 >= pageCount(added.length)"
              @click="setPage('added', 1, added.length)"
            >
              Next
            </button>
          </nav>
        </section>

        <section :aria-labelledby="removedHeadingId">
          <h4 :id="removedHeadingId" class="font-semibold text-ink">
            Removed assignments ({{ removed.length }})
          </h4>
          <ul v-if="visibleRemoved.length" class="mt-2 space-y-2">
            <li v-for="change in visibleRemoved" :key="`removed:${change.before.id}`">
              <article class="rounded-md border border-line bg-raised p-3 text-sm">
                <Badge variant="outline">Removed</Badge>
                <p class="mt-2 font-semibold text-ink">{{ assignmentIdentity(change.before) }}</p>
                <p class="break-all text-xs text-muted">{{ change.before.id }}</p>
                <p class="mt-1">Previous value: {{ valueText(change.before.value) }}</p>
              </article>
            </li>
          </ul>
          <p v-else class="mt-2 text-sm text-muted">No removed assignments.</p>
          <nav
            v-if="pageCount(removed.length) > 1"
            class="mt-3 flex items-center gap-2"
            aria-label="Removed assignments pages"
          >
            <button
              type="button"
              :disabled="pages.removed === 0"
              @click="setPage('removed', -1, removed.length)"
            >
              Previous
            </button>
            <span role="status">{{ pageStatus(removed.length, pages.removed) }}</span>
            <button
              type="button"
              :disabled="pages.removed + 1 >= pageCount(removed.length)"
              @click="setPage('removed', 1, removed.length)"
            >
              Next
            </button>
          </nav>
        </section>

        <section :aria-labelledby="changedHeadingId">
          <h4 :id="changedHeadingId" class="font-semibold text-ink">
            Changed assignments ({{ changed.length }})
          </h4>
          <ul v-if="visibleChanged.length" class="mt-2 space-y-2">
            <li v-for="change in visibleChanged" :key="`changed:${change.after.id}`">
              <article class="rounded-md border border-line bg-raised p-3 text-sm">
                <Badge variant="accent">Changed</Badge>
                <p class="mt-2 font-semibold text-ink">{{ assignmentIdentity(change.after) }}</p>
                <p class="break-all text-xs text-muted">{{ change.after.id }}</p>
                <p class="mt-1">Before: {{ valueText(change.before.value) }}</p>
                <p>After: {{ valueText(change.after.value) }}</p>
              </article>
            </li>
          </ul>
          <p v-else class="mt-2 text-sm text-muted">No changed assignments.</p>
          <nav
            v-if="pageCount(changed.length) > 1"
            class="mt-3 flex items-center gap-2"
            aria-label="Changed assignments pages"
          >
            <button
              type="button"
              :disabled="pages.changed === 0"
              @click="setPage('changed', -1, changed.length)"
            >
              Previous
            </button>
            <span role="status">{{ pageStatus(changed.length, pages.changed) }}</span>
            <button
              type="button"
              :disabled="pages.changed + 1 >= pageCount(changed.length)"
              @click="setPage('changed', 1, changed.length)"
            >
              Next
            </button>
          </nav>
        </section>
      </div>
    </section>

    <section
      v-if="details.affectedEntities.length"
      class="mt-5"
      :aria-labelledby="affectedHeadingId"
    >
      <h3 :id="affectedHeadingId" class="font-semibold text-ink">
        Affected entities ({{ details.affectedEntities.length }})
      </h3>
      <ul class="mt-2 flex flex-wrap gap-2">
        <li v-for="entity in visibleAffected" :key="`${entity.kind}:${entity.id}`">
          <article class="rounded-md border border-line bg-raised p-3 text-sm">
            <p class="font-semibold text-ink">{{ entity.kind }}:{{ entity.id }}</p>
            <p>Before source label: {{ sourceLabel(entity, "before") }}.</p>
            <p>After source label: {{ sourceLabel(entity, "after") }}.</p>
          </article>
        </li>
      </ul>
      <nav
        v-if="pageCount(details.affectedEntities.length) > 1"
        class="mt-3 flex items-center gap-2"
        aria-label="Affected entities pages"
      >
        <button
          type="button"
          :disabled="pages.affected === 0"
          @click="setPage('affected', -1, details.affectedEntities.length)"
        >
          Previous
        </button>
        <span role="status">{{ pageStatus(details.affectedEntities.length, pages.affected) }}</span>
        <button
          type="button"
          :disabled="pages.affected + 1 >= pageCount(details.affectedEntities.length)"
          @click="setPage('affected', 1, details.affectedEntities.length)"
        >
          Next
        </button>
      </nav>
    </section>

    <section class="mt-5" :aria-labelledby="rulesHeadingId">
      <h3 :id="rulesHeadingId" class="font-semibold text-ink">
        Changed required rules ({{ details.rules.length }})
      </h3>
      <ul v-if="visibleRules.length" class="mt-2 space-y-2">
        <li v-for="rule in visibleRules" :key="rule.ruleId">
          <article class="rounded-md border border-line bg-raised p-3 text-sm">
            <p class="font-semibold text-ink">
              Required rule <code>{{ rule.ruleId }}</code>
            </p>
            <p class="mt-1 text-muted">Before: {{ ruleStatus(rule.before) }}.</p>
            <p class="text-muted">After: {{ ruleStatus(rule.after) }}.</p>
            <details class="mt-2">
              <summary>Inspect changed facts and evidence</summary>
              <div v-for="side in ruleSides" :key="side" class="mt-2">
                <h4 class="font-medium">
                  {{ side === "before" ? "Before" : "After" }} rule evidence
                </h4>
                <template v-if="rule[side]">
                  <dl class="mt-1 grid gap-1 break-words">
                    <div>
                      <dt class="inline font-medium">Rule context:</dt>
                      <dd class="inline">
                        <code>{{ rule[side].messageKey }}</code>
                      </dd>
                    </div>
                    <div>
                      <dt class="inline font-medium">Expected:</dt>
                      <dd class="inline">{{ factsText(rule[side].expected) }}</dd>
                    </div>
                    <div>
                      <dt class="inline font-medium">Observed:</dt>
                      <dd class="inline">{{ factsText(rule[side].observed) }}</dd>
                    </div>
                    <div>
                      <dt class="inline font-medium">Evidence:</dt>
                      <dd class="inline">
                        {{
                          rule[side].evidence.length
                            ? rule[side].evidence.join("; ")
                            : "None recorded"
                        }}
                      </dd>
                    </div>
                    <div>
                      <dt class="inline font-medium">Affected entities:</dt>
                      <dd class="inline">
                        {{
                          rule[side].affectedEntities.length
                            ? rule[side].affectedEntities
                                .map((entity) => `${entity.kind} ${entity.id}`)
                                .join("; ")
                            : "None recorded"
                        }}
                      </dd>
                    </div>
                  </dl>
                </template>
                <p v-else>Not present.</p>
              </div>
            </details>
          </article>
        </li>
      </ul>
      <p v-else class="mt-2 text-sm text-muted">No changed required rules.</p>
      <nav
        v-if="pageCount(details.rules.length) > 1"
        class="mt-3 flex items-center gap-2"
        aria-label="Changed required rules pages"
      >
        <button
          type="button"
          :disabled="pages.rules === 0"
          @click="setPage('rules', -1, details.rules.length)"
        >
          Previous
        </button>
        <span role="status">{{ pageStatus(details.rules.length, pages.rules) }}</span>
        <button
          type="button"
          :disabled="pages.rules + 1 >= pageCount(details.rules.length)"
          @click="setPage('rules', 1, details.rules.length)"
        >
          Next
        </button>
      </nav>
    </section>

    <section class="mt-5" :aria-labelledby="metricsHeadingId">
      <h3 :id="metricsHeadingId" class="font-semibold text-ink">
        Changed metrics ({{ details.metrics.length }})
      </h3>
      <ul v-if="visibleMetrics.length" class="mt-2 space-y-2">
        <li v-for="metric in visibleMetrics" :key="metric.metricId">
          <article class="rounded-md border border-line bg-raised p-3 text-sm text-muted">
            <span class="font-semibold text-ink"
              ><code>{{ metric.metricId }}</code></span
            >
            — Before: {{ metricText(metric.before) }}. After: {{ metricText(metric.after) }}.
          </article>
        </li>
      </ul>
      <p v-else class="mt-2 text-sm text-muted">No changed metrics.</p>
      <nav
        v-if="pageCount(details.metrics.length) > 1"
        class="mt-3 flex items-center gap-2"
        aria-label="Changed metrics pages"
      >
        <button
          type="button"
          :disabled="pages.metrics === 0"
          @click="setPage('metrics', -1, details.metrics.length)"
        >
          Previous
        </button>
        <span role="status">{{ pageStatus(details.metrics.length, pages.metrics) }}</span>
        <button
          type="button"
          :disabled="pages.metrics + 1 >= pageCount(details.metrics.length)"
          @click="setPage('metrics', 1, details.metrics.length)"
        >
          Next
        </button>
      </nav>
    </section>

    <section class="mt-5" :aria-labelledby="runsHeadingId">
      <h3 :id="runsHeadingId" class="font-semibold text-ink">
        Recorded run metadata — not historical proof
      </h3>
      <dl class="mt-2 grid gap-3 text-sm text-muted lg:grid-cols-2">
        <div class="rounded-md border border-line bg-raised p-3">
          <dt class="font-semibold text-ink">{{ baseLabel }} run</dt>
          <dd>
            <p>
              <code class="break-all">{{ runs.base.runId }}</code>
            </p>
            <p>
              Run manifest checksum:
              <code class="break-all">{{ runs.base.runManifestChecksum }}</code>
            </p>
            <p>{{ outcomeText(runs.base.outcome) }}.</p>
          </dd>
        </div>
        <div class="rounded-md border border-line bg-raised p-3">
          <dt class="font-semibold text-ink">{{ candidateLabel }} run</dt>
          <dd>
            <p>
              <code class="break-all">{{ runs.candidate.runId }}</code>
            </p>
            <p>
              Run manifest checksum:
              <code class="break-all">{{ runs.candidate.runManifestChecksum }}</code>
            </p>
            <p>{{ outcomeText(runs.candidate.outcome) }}.</p>
          </dd>
        </div>
      </dl>
      <p class="mt-2 text-sm text-muted">
        These are recorded outcomes and checksums, not authenticated historical solver proof.
      </p>
    </section>

    <template v-if="comparableResult">
      <section class="mt-5" :aria-labelledby="scoreHeadingId">
        <h3 :id="scoreHeadingId" class="font-semibold text-ink">
          Compatible score comparison ({{ comparableResult.scoreLevels.length }} levels)
        </h3>
        <p class="mt-2 text-sm text-muted">
          Base feasibility: {{ formatInt64(comparableResult.baseScore.feasibility) }}. Candidate
          feasibility: {{ formatInt64(comparableResult.candidateScore.feasibility) }}. Candidate
          ordering: <strong>{{ comparableResult.ordering }}</strong
          >.
        </p>
        <ul v-if="visibleScoreLevels.length" class="mt-2 space-y-2">
          <li v-for="level in visibleScoreLevels" :key="level.levelId">
            <article class="rounded-md border border-line bg-raised p-3 text-sm text-muted">
              <p class="font-semibold text-ink">
                <code>{{ level.levelId }}</code> ({{
                  level.direction === "minimize" ? "Minimize" : "Maximize"
                }})
              </p>
              <p>
                Before: {{ formatInt64(level.before) }}. After: {{ formatInt64(level.after) }}.
                Change: {{ formatInt64(level.delta) }}.
              </p>
              <ul
                v-if="level.categories.length"
                class="mt-2 space-y-1"
                aria-label="Score categories"
              >
                <li v-for="category in visibleCategories(level)" :key="category.categoryId">
                  <code>{{ category.categoryId }}</code> — Before:
                  {{ category.before === null ? "Not present" : formatInt64(category.before) }}.
                  After:
                  {{ category.after === null ? "Not present" : formatInt64(category.after) }}.
                  Change:
                  {{ category.delta === null ? "Not available" : formatInt64(category.delta) }}.
                </li>
              </ul>
              <nav
                v-if="categoryPageCount(level) > 1"
                class="mt-2 flex items-center gap-2"
                :aria-label="`Score categories pages for ${level.levelId}`"
              >
                <button
                  type="button"
                  :disabled="categoryPage(level.levelId) === 0"
                  @click="setCategoryPage(level.levelId, -1, level.categories.length)"
                >
                  Previous
                </button>
                <span role="status">{{
                  pageStatus(level.categories.length, categoryPage(level.levelId))
                }}</span>
                <button
                  type="button"
                  :disabled="categoryPage(level.levelId) + 1 >= categoryPageCount(level)"
                  @click="setCategoryPage(level.levelId, 1, level.categories.length)"
                >
                  Next
                </button>
              </nav>
            </article>
          </li>
        </ul>
        <p v-else class="mt-2 text-sm text-muted">No score levels were reported.</p>
        <nav
          v-if="pageCount(comparableResult.scoreLevels.length) > 1"
          class="mt-3 flex items-center gap-2"
          aria-label="Score changes pages"
        >
          <button
            type="button"
            :disabled="pages.scoreLevels === 0"
            @click="setPage('scoreLevels', -1, comparableResult.scoreLevels.length)"
          >
            Previous
          </button>
          <span role="status">{{
            pageStatus(comparableResult.scoreLevels.length, pages.scoreLevels)
          }}</span>
          <button
            type="button"
            :disabled="pages.scoreLevels + 1 >= pageCount(comparableResult.scoreLevels.length)"
            @click="setPage('scoreLevels', 1, comparableResult.scoreLevels.length)"
          >
            Next
          </button>
        </nav>
      </section>
    </template>
    <section class="mt-5" :aria-labelledby="protectionsHeadingId">
      <h3 :id="protectionsHeadingId" class="font-semibold text-ink">
        Assignment protection and selection evidence ({{ details.protections.length }})
      </h3>
      <ul v-if="visibleProtections.length" class="mt-2 space-y-2">
        <li v-for="protection in visibleProtections" :key="protection.assignmentId">
          <article class="rounded-md border border-line bg-raised p-3 text-sm text-muted">
            <p class="font-semibold text-ink">
              Assignment <code>{{ protection.assignmentId }}</code>
            </p>
            <p>Before protection: {{ protectionText(protection.before) }}.</p>
            <p>After protection: {{ protectionText(protection.after) }}.</p>
            <p>Before selected: {{ protection.beforeSelected ? "Selected" : "Not selected" }}.</p>
            <p>After selected: {{ protection.afterSelected ? "Selected" : "Not selected" }}.</p>
            <p>Protection state preserved: {{ protection.statePreserved ? "Yes" : "No" }}.</p>
            <p>Selection preserved: {{ protection.assignmentPreserved ? "Yes" : "No" }}.</p>
          </article>
        </li>
      </ul>
      <p v-else class="mt-2 text-sm text-muted">No protection comparisons were reported.</p>
      <nav
        v-if="pageCount(details.protections.length) > 1"
        class="mt-3 flex items-center gap-2"
        aria-label="Assignment protection pages"
      >
        <button
          type="button"
          :disabled="pages.protections === 0"
          @click="setPage('protections', -1, details.protections.length)"
        >
          Previous
        </button>
        <span role="status">
          {{ pageStatus(details.protections.length, pages.protections) }}
        </span>
        <button
          type="button"
          :disabled="pages.protections + 1 >= pageCount(details.protections.length)"
          @click="setPage('protections', 1, details.protections.length)"
        >
          Next
        </button>
      </nav>
    </section>
  </Card>
</template>
