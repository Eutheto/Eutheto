<script setup lang="ts">
import { computed, useId } from "vue";
import type { HistoryPageDtoV1 } from "../../api/generated";
import { formatDateTime, formatNumber, messages } from "../../messages";
import EmptyState from "./EmptyState.vue";
import { plannerMessage } from "./messages";

const props = withDefaults(
  defineProps<{
    page: HistoryPageDtoV1;
    disabledReason: string | null;
    olderPage: boolean;
    locale?: string | undefined;
  }>(),
  { locale: undefined },
);
const emit = defineEmits<{ undo: []; redo: []; older: []; newest: [] }>();
const id = useId();
const undoReason = computed(
  () =>
    props.disabledReason ?? (!props.page.undoAvailable ? plannerMessage("history.noUndo") : null),
);
const redoReason = computed(
  () =>
    props.disabledReason ?? (!props.page.redoAvailable ? plannerMessage("history.noRedo") : null),
);
function change(kind: "undo" | "redo") {
  if (kind === "undo" ? undoReason.value !== null : redoReason.value !== null) return;
  if (kind === "undo") emit("undo");
  else emit("redo");
}
</script>

<template>
  <div class="page-stack">
    <div class="action-row">
      <button
        type="button"
        :disabled="undoReason !== null"
        :aria-describedby="undoReason ? `${id}-undo-reason` : undefined"
        @click="change('undo')"
      >
        {{ messages.shell.undo }}
      </button>
      <button
        type="button"
        class="button-secondary"
        :disabled="redoReason !== null"
        :aria-describedby="redoReason ? `${id}-redo-reason` : undefined"
        @click="change('redo')"
      >
        {{ messages.shell.redo }}
      </button>
    </div>
    <p v-if="undoReason" :id="`${id}-undo-reason`" class="field-help">{{ undoReason }}</p>
    <p v-if="redoReason" :id="`${id}-redo-reason`" class="field-help">{{ redoReason }}</p>
    <EmptyState
      v-if="page.entries.length === 0"
      :heading="plannerMessage('history.empty')"
      :description="plannerMessage('history.emptyHelp')"
      :heading-level="3"
    />
    <ol v-else class="grid list-none gap-3 p-0" :aria-label="plannerMessage('history.changes')">
      <li
        v-for="entry in page.entries"
        :key="entry.id"
        class="grid gap-3 rounded-sm border border-line p-4"
      >
        <p class="wrap-anywhere">
          {{
            entry.summary === null
              ? plannerMessage("history.summaryOmitted")
              : entry.summary === ""
                ? plannerMessage("history.summaryEmpty")
                : entry.summary
          }}
        </p>
        <p class="field-help">
          <time :datetime="entry.createdAt">{{ formatDateTime(entry.createdAt, locale) }}</time>
          · {{ plannerMessage(`history.source.${entry.source}`) }} ·
          {{ plannerMessage(entry.applied ? "history.applied" : "history.undone") }}
        </p>
        <details>
          <summary>{{ plannerMessage("history.details") }}</summary>
          <dl class="metadata-list">
            <div>
              <dt>{{ plannerMessage("history.commandId") }}</dt>
              <dd class="monospace wrap-anywhere">{{ entry.id }}</dd>
            </div>
            <div>
              <dt>{{ plannerMessage("history.revisions") }}</dt>
              <dd>
                {{ formatNumber(entry.revisionBefore, locale) }} →
                {{ formatNumber(entry.revisionAfter, locale) }}
              </dd>
            </div>
            <div>
              <dt>{{ plannerMessage("history.sequence") }}</dt>
              <dd>{{ formatNumber(entry.historySequence, locale) }}</dd>
            </div>
            <div>
              <dt>{{ plannerMessage("history.branch") }}</dt>
              <dd>{{ formatNumber(entry.branchGeneration, locale) }}</dd>
            </div>
          </dl>
        </details>
      </li>
    </ol>
    <div class="action-row">
      <button v-if="olderPage" type="button" class="button-secondary" @click="emit('newest')">
        {{ plannerMessage("history.newest") }}
      </button>
      <button
        v-if="page.continuation !== null"
        type="button"
        class="button-secondary"
        @click="emit('older')"
      >
        {{ plannerMessage("history.older") }}
      </button>
    </div>
  </div>
</template>
