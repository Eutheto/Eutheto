<script setup lang="ts">
import { computed, ref, shallowRef, useId, watch } from "vue";
import type { DurationDraft, DurationFieldEvents, DurationFieldProps } from "./field-contracts";
import { formatDurationQuantity, parseDurationDraft } from "./duration-field";
import { plannerMessage } from "./messages";

const props = defineProps<DurationFieldProps>();
const emit = defineEmits<DurationFieldEvents>();
const generatedId = useId();
const fieldId = computed(() => props.id ?? generatedId);
const draft = shallowRef(props.modelValue);
const unitBlocked = ref(false);
watch(
  () => props.modelValue,
  (value) => {
    draft.value = value;
  },
  { flush: "sync" },
);
const localError = computed(() => {
  if (draft.value.status !== "invalid") return "";
  return plannerMessage(
    `duration.${draft.value.error}`,
    {
      minimum: props.minimum,
      maximum: 4294967295,
    },
    props.locale,
  );
});
const describedBy = computed(() =>
  [
    props.description ? `${fieldId.value}-description` : "",
    `${fieldId.value}-grammar`,
    draft.value.status === "valid" ? `${fieldId.value}-quantity` : "",
    localError.value ? `${fieldId.value}-local-error` : "",
    props.error ? `${fieldId.value}-error` : "",
    unitBlocked.value ? `${fieldId.value}-unit-help` : "",
  ]
    .filter(Boolean)
    .join(" "),
);

function publish(value: DurationDraft) {
  draft.value = value;
  unitBlocked.value = false;
  emit("update:modelValue", value);
}
function editRaw(event: Event) {
  if (props.disabled || props.readOnly) return;
  publish(
    parseDurationDraft((event.target as HTMLInputElement).value, draft.value.unit, props.minimum),
  );
}
function changeUnit(event: Event) {
  const control = event.target as HTMLSelectElement;
  if (props.disabled || props.readOnly) {
    control.value = draft.value.unit;
    return;
  }
  const unit = control.value;
  if (unit !== "minutes" && unit !== "hours") return;
  if (unit === draft.value.unit) return;
  const raw =
    draft.value.status === "valid"
      ? formatDurationQuantity(draft.value.minutes, unit)
      : draft.value.raw;
  if (raw === null) {
    control.value = draft.value.unit;
    unitBlocked.value = true;
    return;
  }
  publish(parseDurationDraft(raw, unit, props.minimum));
}
</script>

<template>
  <div class="field-stack">
    <div class="form-columns">
      <div class="field-stack">
        <label :for="fieldId">{{ label }}</label>
        <input
          :id="fieldId"
          type="text"
          inputmode="decimal"
          autocomplete="off"
          :spellcheck="false"
          :value="draft.raw"
          :required="required"
          :disabled="disabled"
          :readonly="readOnly"
          :aria-invalid="Boolean(error || localError) || undefined"
          :aria-describedby="describedBy"
          @input="editRaw"
        />
      </div>
      <div class="field-stack">
        <label :for="`${fieldId}-unit`">{{ plannerMessage("duration.unit") }}</label>
        <select
          :id="`${fieldId}-unit`"
          :value="draft.unit"
          :disabled="disabled || readOnly"
          :aria-describedby="describedBy"
          @change="changeUnit"
        >
          <option value="minutes">{{ plannerMessage("duration.minutes") }}</option>
          <option value="hours">{{ plannerMessage("duration.hours") }}</option>
        </select>
      </div>
    </div>
    <p v-if="description" :id="`${fieldId}-description`" class="field-help">{{ description }}</p>
    <p :id="`${fieldId}-grammar`" class="field-help">{{ plannerMessage("duration.grammar") }}</p>
    <p v-if="draft.status === 'valid'" :id="`${fieldId}-quantity`" class="field-help">
      {{ plannerMessage("duration.quantity", { minutes: draft.minutes }, locale) }}
    </p>
    <p v-if="localError" :id="`${fieldId}-local-error`" class="text-sm text-danger" role="status">
      {{ localError }}
    </p>
    <p v-if="error" :id="`${fieldId}-error`" class="text-sm text-danger" role="alert">
      {{ error }}
    </p>
    <p v-if="unitBlocked" :id="`${fieldId}-unit-help`" class="field-help" role="status">
      {{ plannerMessage("duration.exactUnit") }}
    </p>
  </div>
</template>
