<script setup lang="ts">
import { computed, ref, shallowRef, useId, watch } from "vue";
import type {
  DateTimeRangeFieldEvents,
  DateTimeRangeFieldProps,
  TemporalFeedback,
  TemporalRawDraft,
} from "./field-contracts";
import { plannerMessage } from "./messages";
import { parseTemporalDraft } from "./temporal-field";

const props = defineProps<DateTimeRangeFieldProps>();
const emit = defineEmits<DateTimeRangeFieldEvents>();
const generatedId = useId();
const fieldId = computed(() => props.id ?? generatedId);
const draft = shallowRef(props.modelValue);
const invalidatedRequestKey = ref<string | null>(null);
watch(
  () => props.modelValue,
  (value) => {
    draft.value = value;
  },
  { flush: "sync" },
);
const feedback = computed(() =>
  invalidatedRequestKey.value !== props.requestKey &&
  props.feedback?.requestKey === props.requestKey
    ? props.feedback.errors
    : [],
);
const start = computed(() =>
  draft.value.raw.kind === "localWindow" ? draft.value.raw.startTime : draft.value.raw.startsAt,
);
const end = computed(() =>
  draft.value.raw.kind === "localWindow" ? draft.value.raw.endTime : draft.value.raw.endsAt,
);
type FeedbackField = TemporalFeedback["errors"][number]["field"];
const errors = computed(() => {
  const result: Record<FeedbackField, string[]> = {
    start: [],
    end: [],
    endDayOffset: [],
    range: [],
  };
  for (const error of feedback.value) result[error.field].push(error.message);
  if (props.error) result.range.push(props.error);
  if (draft.value.status === "incomplete") {
    if (draft.value.error === "dayOffset") {
      result.endDayOffset.push(plannerMessage("temporal.offsetError"));
    } else {
      if (start.value === "") result.start.push(plannerMessage("temporal.endpoints"));
      if (end.value === "") result.end.push(plannerMessage("temporal.endpoints"));
    }
  }
  return result;
});
const commonDescription = computed(() =>
  [
    props.description ? `${fieldId.value}-description` : "",
    `${fieldId.value}-zone`,
    `${fieldId.value}-help`,
    `${fieldId.value}-pending`,
    errors.value.range.length ? `${fieldId.value}-range-error` : "",
  ]
    .filter(Boolean)
    .join(" "),
);
function describedBy(field: FeedbackField): string {
  return [
    commonDescription.value,
    field === "endDayOffset" ? `${fieldId.value}-offset-help` : "",
    errors.value[field].length ? `${fieldId.value}-${field}-error` : "",
  ]
    .filter(Boolean)
    .join(" ");
}
function edit(raw: TemporalRawDraft) {
  if (props.disabled || props.readOnly) return;
  // Hide old native feedback in this event, before a parent can rotate its key.
  invalidatedRequestKey.value = props.requestKey;
  const next = parseTemporalDraft(raw);
  draft.value = next;
  emit("update:modelValue", next);
}
function editEndpoint(field: "start" | "end", event: Event) {
  const value = (event.target as HTMLInputElement).value;
  const raw = draft.value.raw;
  if (raw.kind === "localWindow") {
    edit(field === "start" ? { ...raw, startTime: value } : { ...raw, endTime: value });
  } else {
    edit(field === "start" ? { ...raw, startsAt: value } : { ...raw, endsAt: value });
  }
}
function editOffset(value: string) {
  if (draft.value.raw.kind === "localWindow") edit({ ...draft.value.raw, endDayOffset: value });
}
function offsetInput(event: Event) {
  editOffset((event.target as HTMLInputElement).value);
}
</script>

<template>
  <fieldset
    :id="fieldId"
    class="form-fields"
    :disabled="disabled"
    :aria-describedby="commonDescription"
  >
    <legend>{{ label }}</legend>
    <p v-if="description" :id="`${fieldId}-description`" class="field-help">{{ description }}</p>
    <p :id="`${fieldId}-zone`" class="field-help">
      {{ plannerMessage("temporal.zone", { zone: timeZone }) }}
    </p>
    <div class="form-columns">
      <div class="field-stack">
        <label :for="`${fieldId}-start`">
          {{
            plannerMessage(
              draft.raw.kind === "localWindow" ? "temporal.startTime" : "temporal.startsAt",
            )
          }}
        </label>
        <input
          :id="`${fieldId}-start`"
          type="text"
          autocomplete="off"
          :spellcheck="false"
          :value="start"
          :required="required"
          :readonly="readOnly"
          :aria-invalid="Boolean(errors.start.length || errors.range.length) || undefined"
          :aria-describedby="describedBy('start')"
          @input="editEndpoint('start', $event)"
        />
        <ul
          v-if="errors.start.length"
          :id="`${fieldId}-start-error`"
          class="text-sm text-danger"
          role="status"
        >
          <li v-for="(message, index) in errors.start" :key="index">{{ message }}</li>
        </ul>
      </div>
      <div class="field-stack">
        <label :for="`${fieldId}-end`">
          {{
            plannerMessage(
              draft.raw.kind === "localWindow" ? "temporal.endTime" : "temporal.endsAt",
            )
          }}
        </label>
        <input
          :id="`${fieldId}-end`"
          type="text"
          autocomplete="off"
          :spellcheck="false"
          :value="end"
          :required="required"
          :readonly="readOnly"
          :aria-invalid="Boolean(errors.end.length || errors.range.length) || undefined"
          :aria-describedby="describedBy('end')"
          @input="editEndpoint('end', $event)"
        />
        <ul
          v-if="errors.end.length"
          :id="`${fieldId}-end-error`"
          class="text-sm text-danger"
          role="status"
        >
          <li v-for="(message, index) in errors.end" :key="index">{{ message }}</li>
        </ul>
      </div>
    </div>
    <div v-if="draft.raw.kind === 'localWindow'" class="field-stack">
      <label :for="`${fieldId}-endDayOffset`">{{ plannerMessage("temporal.endDayOffset") }}</label>
      <input
        :id="`${fieldId}-endDayOffset`"
        type="text"
        inputmode="numeric"
        autocomplete="off"
        :spellcheck="false"
        :value="draft.raw.endDayOffset"
        :required="required"
        :readonly="readOnly"
        :aria-invalid="Boolean(errors.endDayOffset.length || errors.range.length) || undefined"
        :aria-describedby="describedBy('endDayOffset')"
        @input="offsetInput"
      />
      <div class="flex flex-wrap gap-2">
        <button
          type="button"
          class="button-secondary"
          :disabled="readOnly"
          @click="editOffset('0')"
        >
          {{ plannerMessage("temporal.sameDay") }}
        </button>
        <button
          type="button"
          class="button-secondary"
          :disabled="readOnly"
          @click="editOffset('1')"
        >
          {{ plannerMessage("temporal.nextDay") }}
        </button>
      </div>
      <p :id="`${fieldId}-offset-help`" class="field-help">
        {{ plannerMessage("temporal.offsetHelp") }}
      </p>
      <ul
        v-if="errors.endDayOffset.length"
        :id="`${fieldId}-endDayOffset-error`"
        class="text-sm text-danger"
        role="status"
      >
        <li v-for="(message, index) in errors.endDayOffset" :key="index">{{ message }}</li>
      </ul>
    </div>
    <p :id="`${fieldId}-help`" class="field-help">
      {{
        plannerMessage(
          draft.raw.kind === "localWindow" ? "temporal.timeHelp" : "temporal.intervalHelp",
        )
      }}
    </p>
    <p :id="`${fieldId}-pending`" class="field-help">
      {{ plannerMessage("temporal.nativePending") }}
    </p>
    <ul
      v-if="errors.range.length"
      :id="`${fieldId}-range-error`"
      class="text-sm text-danger"
      role="alert"
    >
      <li v-for="(message, index) in errors.range" :key="index">{{ message }}</li>
    </ul>
  </fieldset>
</template>
