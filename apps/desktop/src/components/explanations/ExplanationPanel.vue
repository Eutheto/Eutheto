<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type {
  DomainEntityRef,
  EvidenceMessageV1,
  ExplanationCertainty,
  ExplanationResultV2,
  Revision,
  UuidV7,
} from "../../api/generated";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "../ui";
import AcceptedComparison from "../AcceptedComparison.vue";
import { explanationMessage, renderEvidenceTemplate, verificationValueText } from "./messages";
import type { ExplanationUiState } from "./types";

interface Props {
  readonly open: boolean;
  readonly result: ExplanationResultV2 | null;
  readonly state: ExplanationUiState;
  readonly messageTexts: Readonly<Record<string, string>>;
  readonly scenarioId: UuidV7;
  readonly currentRevision: Revision;
  readonly sourceRevisions?: readonly Revision[] | undefined;
  readonly statusMessage?: string | null;
  readonly cancelRequested?: boolean;
  readonly showEditRelated?: boolean;
  readonly locale?: string | undefined;
  readonly restoreFocus?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  sourceRevisions: undefined,
  locale: undefined,
  statusMessage: null,
  cancelRequested: false,
  showEditRelated: true,
  restoreFocus: true,
});
const emit = defineEmits<{
  "update:open": [open: boolean];
  retry: [];
  editRelated: [];
  cancel: [];
}>();
defineSlots<{
  trigger?(): unknown;
}>();

const PAGE_SIZE = 20;
const MESSAGE_PAGE_SIZE = 10;
const messagePage = ref(0);
const referencePages = ref<Record<string, number>>({});
const closeButton = ref<HTMLButtonElement>();
const cancelButton = ref<HTMLButtonElement>();

const comparisonEvidence = computed(() => {
  const evidence = props.result?.evidence.evidence;
  if (evidence?.kind === "solutionDifference")
    return { kind: "difference" as const, comparison: evidence.comparison };
  if (evidence?.kind === "repair")
    return { kind: "repair" as const, comparison: evidence.repair.comparison };
  return null;
});
const repairComparison = computed(() => {
  const evidence = props.result?.evidence.evidence;
  return evidence?.kind === "repair" ? evidence.repair.comparison : null;
});
const stateCopy = computed(() => {
  switch (props.state) {
    case "ready":
      return props.result ? null : explanationMessage("explanation.empty");
    case "empty":
      return explanationMessage("explanation.empty");
    case "loading":
      return explanationMessage("error.loading");
    case "stale":
      return explanationMessage("error.stale");
    case "cancelled":
      return explanationMessage("error.cancelled");
    case "inconclusive":
      return explanationMessage("error.inconclusive");
    case "unavailable":
      return explanationMessage("error.unavailable");
    case "internalFailure":
      return explanationMessage("error.internal");
  }
});

const stateHeading = computed(() => {
  switch (props.state) {
    case "ready":
    case "empty":
      return "No explanation available";
    case "loading":
      return "Preparing evidence";
    case "stale":
      return "Explanation is out of date";
    case "cancelled":
      return "Explanation cancelled";
    case "inconclusive":
      return "Explanation is inconclusive";
    case "unavailable":
      return "Explanation unavailable";
    case "internalFailure":
      return explanationMessage("error.heading");
  }
});

const stateRole = computed(() =>
  props.state === "stale" || props.state === "unavailable" || props.state === "internalFailure"
    ? "alert"
    : "status",
);
function handleCloseAutoFocus(event: Event): void {
  if (!props.restoreFocus) event.preventDefault();
}
const messages = computed(() => props.result?.rendered.messages ?? []);
const visibleMessages = computed(() =>
  messages.value.slice(
    messagePage.value * MESSAGE_PAGE_SIZE,
    (messagePage.value + 1) * MESSAGE_PAGE_SIZE,
  ),
);

function messageText(message: EvidenceMessageV1): string {
  const summary = message.parameters["official.workforce.fact.summary"];
  if (summary !== undefined) return verificationValueText(summary, props.locale);
  const template =
    props.messageTexts[message.messageKey] ??
    "Derived evidence fields from the selected sources are listed below.";
  return renderEvidenceTemplate(template, message.parameters, props.locale);
}

function visibleReferences<T>(
  values: readonly T[],
  index: number,
  collection: string,
): readonly T[] {
  const start = (referencePages.value[`${String(index)}:${collection}`] ?? 0) * PAGE_SIZE;
  return values.slice(start, start + PAGE_SIZE);
}

function referencePageCount(length: number): number {
  return Math.ceil(length / PAGE_SIZE);
}

function changeReferencePage(index: number, collection: string, direction: number, length: number) {
  const key = `${String(index)}:${collection}`;
  referencePages.value[key] = Math.min(
    Math.max(0, Math.ceil(length / PAGE_SIZE) - 1),
    Math.max(0, (referencePages.value[key] ?? 0) + direction),
  );
}

function pageStatus(length: number, page: number, size: number): string {
  const first = page * size + 1;
  const last = Math.min((page + 1) * size, length);
  return `Showing ${String(first)}–${String(last)} of ${String(length)}.`;
}

watch(
  () => props.result,
  () => {
    messagePage.value = 0;
    referencePages.value = {};
  },
);
watch(
  () => props.state,
  (state, previous) => {
    if (
      previous === "loading" &&
      state !== "loading" &&
      props.open &&
      typeof document !== "undefined" &&
      cancelButton.value !== undefined &&
      document.activeElement === cancelButton.value
    )
      void nextTick(() => closeButton.value?.focus());
  },
);

function certaintyLabel(certainty: ExplanationCertainty): string {
  switch (certainty) {
    case "deterministic":
      return "Deterministic evidence";
    case "independentlyVerified":
      return "Independently verified";
    case "backendProof":
      return "Backend proof";
    case "sufficientConflict":
      return "Sufficient conflict";
    case "provenMinimalConflict":
      return "Proven minimal conflict";
    case "inconclusive":
      return "Inconclusive";
    case "unavailable":
      return "Unavailable";
  }
}

function certaintyDescription(certainty: ExplanationCertainty): string {
  switch (certainty) {
    case "deterministic":
      return "These facts were derived deterministically. They are evidence, not a solver proof.";
    case "independentlyVerified":
      return "An independent verifier accepted the evidence shown here.";
    case "backendProof":
      return "The solver supplied proof evidence for this outcome; deterministic evidence remains listed separately.";
    case "sufficientConflict":
      return "This set is enough to prove a conflict. Other conflicts may also exist, and minimality was not proved.";
    case "provenMinimalConflict":
      return "Minimality was proved for the stated conflict grouping.";
    case "inconclusive":
      return "The available evidence does not establish a verified conclusion.";
    case "unavailable":
      return "No proof or verified certainty evidence is available.";
  }
}

function entityHref(entity: DomainEntityRef): string {
  return `#entity-${encodeURIComponent(entity.kind)}-${encodeURIComponent(entity.id)}`;
}

function ruleHref(ruleId: string): string {
  return `#rule-${encodeURIComponent(ruleId)}`;
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogTrigger v-if="$slots.trigger" as-child>
      <slot name="trigger" />
    </DialogTrigger>
    <DialogContent class="max-w-3xl" @close-auto-focus="handleCloseAutoFocus">
      <div class="flex items-start justify-between gap-4">
        <div>
          <DialogTitle>{{ explanationMessage("explanation.heading") }}</DialogTitle>
          <DialogDescription>
            Verified evidence and proof status are shown separately.
          </DialogDescription>
        </div>
        <DialogClose as-child>
          <button
            ref="closeButton"
            type="button"
            class="rounded-md border border-line bg-surface px-3 py-2 font-bold text-ink"
          >
            {{ explanationMessage("action.close") }}
          </button>
        </DialogClose>
      </div>
      <div
        v-if="stateCopy"
        class="rounded-md border border-line bg-surface p-4"
        :role="stateRole"
        :aria-live="stateRole === 'status' ? 'polite' : undefined"
        aria-atomic="true"
      >
        <h2 class="font-bold text-ink">{{ stateHeading }}</h2>
        <p class="mt-1 text-sm text-muted">{{ stateCopy }}</p>
        <p v-if="statusMessage" class="mt-1 text-sm text-muted">{{ statusMessage }}</p>
        <p v-if="state === 'loading' && cancelRequested" class="mt-1 text-sm text-muted">
          Cancellation was requested; waiting for the native operation to finish.
        </p>
        <p v-else-if="state === 'loading'" class="mt-1 text-sm text-muted">
          You can cancel this request instead of waiting.
        </p>
        <div class="mt-4 flex flex-wrap gap-3">
          <button
            v-if="state === 'loading'"
            ref="cancelButton"
            type="button"
            class="rounded-md border border-line-strong bg-raised px-3 py-2 font-bold text-ink"
            :aria-disabled="cancelRequested"
            @click="emit('cancel')"
          >
            {{ explanationMessage("action.cancel") }}
          </button>
          <button
            v-if="state !== 'loading' && state !== 'empty'"
            type="button"
            class="rounded-md border border-line-strong bg-raised px-3 py-2 font-bold text-ink"
            @click="emit('retry')"
          >
            {{ explanationMessage("action.retry") }}
          </button>
          <button
            v-if="showEditRelated && state !== 'loading'"
            type="button"
            class="rounded-md border border-line bg-surface px-3 py-2 font-bold text-ink"
            @click="emit('editRelated')"
          >
            {{ explanationMessage("action.edit") }}
          </button>
        </div>
      </div>

      <Tabs v-else-if="result" default-value="evidence">
        <TabsList aria-label="Explanation sections">
          <TabsTrigger value="evidence">
            {{ explanationMessage("explanation.messages") }}
          </TabsTrigger>
          <TabsTrigger value="proof">{{ explanationMessage("explanation.proof") }}</TabsTrigger>
        </TabsList>

        <TabsContent value="evidence" force-mount>
          <template v-if="comparisonEvidence">
            <p
              v-if="comparisonEvidence.kind === 'repair'"
              class="mt-3 rounded-md border border-line bg-surface p-3 text-sm text-muted"
              role="note"
            >
              The accepted comparison is A → C. Repair facts use recorded baseline B → C when B is
              present, otherwise A → C. B admission is not established, and no unique cause is
              claimed.
            </p>
            <AcceptedComparison
              :comparison="comparisonEvidence.comparison"
              :scenario-id="scenarioId"
              :current-revision="currentRevision"
              :base-label="
                comparisonEvidence.kind === 'repair'
                  ? 'Accepted comparison base A'
                  : 'Base accepted result'
              "
              :candidate-label="
                comparisonEvidence.kind === 'repair'
                  ? 'Current accepted result C'
                  : 'Candidate accepted result'
              "
            />
          </template>

          <section
            v-if="repairComparison"
            class="mt-4 rounded-md border border-line bg-surface p-4"
            aria-labelledby="repair-source-heading"
          >
            <h2 id="repair-source-heading" class="font-display text-lg font-semibold text-ink">
              Repair source bindings
            </h2>
            <p class="mt-1 text-sm text-muted">
              Recorded baseline admission: notEstablished. This read does not authenticate
              historical custody or execution of B.
            </p>
            <dl class="mt-3 grid gap-2 text-sm">
              <div>
                <dt class="font-bold text-muted">Visible scenario scope</dt>
                <dd class="break-all font-mono text-ink">
                  {{ scenarioId }} · visible revision {{ currentRevision }}
                </dd>
              </div>
              <div v-if="sourceRevisions">
                <dt class="font-bold text-muted">Repair response revisions [candidate, base]</dt>
                <dd class="font-mono text-ink">
                  [{{ sourceRevisions[0] }}, {{ sourceRevisions[1] }}]
                </dd>
              </div>
              <div>
                <dt class="font-bold text-muted">Accepted base A</dt>
                <dd class="break-all font-mono text-ink">
                  {{ repairComparison.base.acceptedResult.solutionId }} ·
                  {{ repairComparison.base.acceptedResult.resultChecksum }} · source revision
                  {{ repairComparison.base.scenarioRevision }} · run
                  {{ repairComparison.runs.base.runId }}
                </dd>
              </div>
              <div>
                <dt class="font-bold text-muted">Accepted candidate C</dt>
                <dd class="break-all font-mono text-ink">
                  {{ repairComparison.candidate.acceptedResult.solutionId }} ·
                  {{ repairComparison.candidate.acceptedResult.resultChecksum }} · source revision
                  {{ repairComparison.candidate.scenarioRevision }} · run
                  {{ repairComparison.runs.candidate.runId }}
                </dd>
              </div>
            </dl>
          </section>

          <section aria-labelledby="deterministic-evidence-heading">
            <h2
              id="deterministic-evidence-heading"
              class="font-display text-lg font-semibold text-ink"
            >
              Deterministic evidence
            </h2>
            <ol v-if="messages.length" class="mt-3 space-y-4">
              <li
                v-for="(message, index) in visibleMessages"
                :key="`${message.messageKey}-${messagePage * MESSAGE_PAGE_SIZE + index}`"
                class="rounded-md border border-line bg-surface p-4"
              >
                <p class="text-ink">{{ messageText(message) }}</p>

                <section
                  v-if="Object.keys(message.parameters).length"
                  class="mt-3"
                  aria-label="Evidence parameters"
                >
                  <h3 class="text-sm font-bold text-muted">Evidence parameters</h3>
                  <dl class="mt-1 grid gap-2 text-sm">
                    <div
                      v-for="[name, value] in visibleReferences(
                        Object.entries(message.parameters),
                        messagePage * MESSAGE_PAGE_SIZE + index,
                        'parameters',
                      )"
                      :key="name"
                    >
                      <dt class="break-all font-mono font-bold text-muted">{{ name }}</dt>
                      <dd class="break-words text-ink">
                        {{ verificationValueText(value, locale) }}
                      </dd>
                    </div>
                  </dl>
                  <nav
                    v-if="referencePageCount(Object.keys(message.parameters).length) > 1"
                    class="mt-2 flex items-center gap-2"
                    :aria-label="`Evidence parameter pages for ${message.messageKey}`"
                  >
                    <button
                      type="button"
                      :disabled="
                        (referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:parameters`] ??
                          0) === 0
                      "
                      @click="
                        changeReferencePage(
                          messagePage * MESSAGE_PAGE_SIZE + index,
                          'parameters',
                          -1,
                          Object.keys(message.parameters).length,
                        )
                      "
                    >
                      Previous
                    </button>
                    <span role="status">
                      {{
                        pageStatus(
                          Object.keys(message.parameters).length,
                          referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:parameters`] ??
                            0,
                          PAGE_SIZE,
                        )
                      }}
                    </span>
                    <button
                      type="button"
                      :disabled="
                        ((referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:parameters`] ??
                          0) +
                          1) *
                          PAGE_SIZE >=
                        Object.keys(message.parameters).length
                      "
                      @click="
                        changeReferencePage(
                          messagePage * MESSAGE_PAGE_SIZE + index,
                          'parameters',
                          1,
                          Object.keys(message.parameters).length,
                        )
                      "
                    >
                      Next
                    </button>
                  </nav>
                </section>

                <div v-if="message.entities.length || message.rules.length" class="mt-3">
                  <h3 class="text-sm font-bold text-muted">Related items</h3>
                  <ul class="mt-1 flex flex-wrap gap-2">
                    <li
                      v-for="entity in visibleReferences(
                        message.entities,
                        messagePage * MESSAGE_PAGE_SIZE + index,
                        'related',
                      )"
                      :key="`${entity.kind}:${entity.id}`"
                    >
                      <a
                        :href="entityHref(entity)"
                        class="font-bold text-accent-strong underline underline-offset-2"
                      >
                        {{ entity.kind }} {{ entity.id }}
                      </a>
                    </li>
                    <li
                      v-for="ruleId in visibleReferences(
                        message.rules,
                        messagePage * MESSAGE_PAGE_SIZE + index,
                        'related',
                      )"
                      :key="ruleId"
                    >
                      <a
                        :href="ruleHref(ruleId)"
                        class="font-bold text-accent-strong underline underline-offset-2"
                      >
                        Rule {{ ruleId }}
                      </a>
                    </li>
                  </ul>
                  <nav
                    v-if="referencePageCount(message.entities.length) > 1"
                    class="mt-2 flex items-center gap-2"
                    aria-label="Related entity pages"
                  >
                    <button
                      type="button"
                      :disabled="
                        (referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:related`] ??
                          0) === 0
                      "
                      @click="
                        changeReferencePage(
                          messagePage * MESSAGE_PAGE_SIZE + index,
                          'related',
                          -1,
                          message.entities.length,
                        )
                      "
                    >
                      Previous entities
                    </button>
                    <span role="status">
                      {{
                        pageStatus(
                          message.entities.length,
                          referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:related`] ?? 0,
                          PAGE_SIZE,
                        )
                      }}
                    </span>
                    <button
                      type="button"
                      :disabled="
                        ((referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:related`] ??
                          0) +
                          1) *
                          PAGE_SIZE >=
                        message.entities.length
                      "
                      @click="
                        changeReferencePage(
                          messagePage * MESSAGE_PAGE_SIZE + index,
                          'related',
                          1,
                          message.entities.length,
                        )
                      "
                    >
                      Next entities
                    </button>
                  </nav>
                  <nav
                    v-if="referencePageCount(message.rules.length) > 1"
                    class="mt-2 flex items-center gap-2"
                    aria-label="Related rule pages"
                  >
                    <button
                      type="button"
                      :disabled="
                        (referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:related`] ??
                          0) === 0
                      "
                      @click="
                        changeReferencePage(
                          messagePage * MESSAGE_PAGE_SIZE + index,
                          'related',
                          -1,
                          message.rules.length,
                        )
                      "
                    >
                      Previous rules
                    </button>
                    <span role="status">
                      {{
                        pageStatus(
                          message.rules.length,
                          referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:related`] ?? 0,
                          PAGE_SIZE,
                        )
                      }}
                    </span>
                    <button
                      type="button"
                      :disabled="
                        ((referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:related`] ??
                          0) +
                          1) *
                          PAGE_SIZE >=
                        message.rules.length
                      "
                      @click="
                        changeReferencePage(
                          messagePage * MESSAGE_PAGE_SIZE + index,
                          'related',
                          1,
                          message.rules.length,
                        )
                      "
                    >
                      Next rules
                    </button>
                  </nav>
                </div>

                <section v-if="message.assignments.length" class="mt-3">
                  <h3 class="text-sm font-bold text-muted">
                    Assignment references ({{ message.assignments.length }})
                  </h3>
                  <ul class="mt-1 list-disc break-all pl-5 font-mono text-sm text-ink">
                    <li
                      v-for="assignmentId in visibleReferences(
                        message.assignments,
                        messagePage * MESSAGE_PAGE_SIZE + index,
                        'assignments',
                      )"
                      :key="assignmentId"
                    >
                      {{ assignmentId }}
                    </li>
                  </ul>
                  <nav
                    v-if="referencePageCount(message.assignments.length) > 1"
                    class="mt-2 flex items-center gap-2"
                    :aria-label="`Assignment reference pages for ${message.messageKey}`"
                  >
                    <button
                      type="button"
                      :disabled="
                        (referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:assignments`] ??
                          0) === 0
                      "
                      @click="
                        changeReferencePage(
                          messagePage * MESSAGE_PAGE_SIZE + index,
                          'assignments',
                          -1,
                          message.assignments.length,
                        )
                      "
                    >
                      Previous
                    </button>
                    <span role="status">
                      {{
                        pageStatus(
                          message.assignments.length,
                          referencePages[
                            `${messagePage * MESSAGE_PAGE_SIZE + index}:assignments`
                          ] ?? 0,
                          PAGE_SIZE,
                        )
                      }}
                    </span>
                    <button
                      type="button"
                      :disabled="
                        ((referencePages[
                          `${messagePage * MESSAGE_PAGE_SIZE + index}:assignments`
                        ] ?? 0) +
                          1) *
                          PAGE_SIZE >=
                        message.assignments.length
                      "
                      @click="
                        changeReferencePage(
                          messagePage * MESSAGE_PAGE_SIZE + index,
                          'assignments',
                          1,
                          message.assignments.length,
                        )
                      "
                    >
                      Next
                    </button>
                  </nav>
                </section>

                <details v-if="message.evidence.length" class="mt-3">
                  <summary class="cursor-pointer text-sm font-bold text-ink">
                    Evidence references ({{ message.evidence.length }})
                  </summary>
                  <ul class="mt-1 list-disc pl-5 text-sm text-muted">
                    <li
                      v-for="evidenceId in visibleReferences(
                        message.evidence,
                        messagePage * MESSAGE_PAGE_SIZE + index,
                        'evidence',
                      )"
                      :key="evidenceId"
                    >
                      {{ evidenceId }}
                    </li>
                  </ul>
                  <nav
                    v-if="referencePageCount(message.evidence.length) > 1"
                    class="mt-2 flex items-center gap-2"
                    :aria-label="`Evidence reference pages for ${message.messageKey}`"
                  >
                    <button
                      type="button"
                      :disabled="
                        (referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:evidence`] ??
                          0) === 0
                      "
                      @click="
                        changeReferencePage(
                          messagePage * MESSAGE_PAGE_SIZE + index,
                          'evidence',
                          -1,
                          message.evidence.length,
                        )
                      "
                    >
                      Previous
                    </button>
                    <span role="status">
                      {{
                        pageStatus(
                          message.evidence.length,
                          referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:evidence`] ??
                            0,
                          PAGE_SIZE,
                        )
                      }}
                    </span>
                    <button
                      type="button"
                      :disabled="
                        ((referencePages[`${messagePage * MESSAGE_PAGE_SIZE + index}:evidence`] ??
                          0) +
                          1) *
                          PAGE_SIZE >=
                        message.evidence.length
                      "
                      @click="
                        changeReferencePage(
                          messagePage * MESSAGE_PAGE_SIZE + index,
                          'evidence',
                          1,
                          message.evidence.length,
                        )
                      "
                    >
                      Next
                    </button>
                  </nav>
                </details>
              </li>
            </ol>
            <nav
              v-if="messages.length > MESSAGE_PAGE_SIZE"
              class="mt-3 flex items-center gap-2"
              aria-label="Evidence message pages"
            >
              <button type="button" :disabled="messagePage === 0" @click="messagePage -= 1">
                Previous
              </button>
              <span role="status">
                {{ pageStatus(messages.length, messagePage, MESSAGE_PAGE_SIZE) }}
              </span>
              <button
                type="button"
                :disabled="(messagePage + 1) * MESSAGE_PAGE_SIZE >= messages.length"
                @click="messagePage += 1"
              >
                Next
              </button>
            </nav>
            <p v-if="messages.length === 0" class="mt-3 text-sm text-muted">
              {{ explanationMessage("explanation.empty") }}
            </p>
          </section>
        </TabsContent>

        <TabsContent value="proof" force-mount>
          <section aria-labelledby="proof-certainty-heading">
            <h2 id="proof-certainty-heading" class="font-display text-lg font-semibold text-ink">
              {{ explanationMessage("explanation.proof") }}
            </h2>
            <p class="mt-3 font-bold text-ink">
              Certainty: {{ certaintyLabel(result.evidence.certainty) }}
            </p>
            <p class="mt-1 text-sm text-muted">
              {{ certaintyDescription(result.evidence.certainty) }}
            </p>
            <dl class="mt-4 space-y-2 text-sm">
              <div>
                <dt class="font-bold text-muted">Evidence checksum</dt>
                <dd class="break-all font-mono text-ink">{{ result.evidence.checksum }}</dd>
              </div>
              <div>
                <dt class="font-bold text-muted">Rendered evidence checksum</dt>
                <dd class="break-all font-mono text-ink">{{ result.rendered.evidenceChecksum }}</dd>
              </div>
            </dl>
          </section>
        </TabsContent>
      </Tabs>

      <div v-if="!stateCopy && showEditRelated" class="flex justify-end">
        <button
          type="button"
          class="rounded-md border border-line bg-surface px-3 py-2 font-bold text-ink"
          @click="emit('editRelated')"
        >
          {{ explanationMessage("action.edit") }}
        </button>
      </div>
    </DialogContent>
  </Dialog>
</template>
