<script setup lang="ts">
import { computed, useId } from "vue";
import type {
  WorkforcePreferencePriority,
  WorkforceSetupRuleClass,
} from "../../api/generated-domain-pack-contracts";
import type { RuleStrengthControlEvents, RuleStrengthControlProps } from "./field-contracts";
import { plannerMessage } from "./messages";

const props = defineProps<RuleStrengthControlProps>();
const emit = defineEmits<RuleStrengthControlEvents>();
const generatedId = useId();
const fieldId = computed(() => props.id ?? generatedId);
const classes: readonly WorkforceSetupRuleClass[] = ["required", "preference"];
const priorities: readonly WorkforcePreferencePriority[] = ["low", "normal", "high", "veryHigh"];
const describedBy = computed(
  () =>
    [
      props.description ? `${fieldId.value}-description` : "",
      !props.classEditable ? `${fieldId.value}-locked` : "",
      props.error ? `${fieldId.value}-error` : "",
    ]
      .filter(Boolean)
      .join(" ") || undefined,
);
function implemented(ruleClass: WorkforceSetupRuleClass): boolean {
  const entry = ruleClass === "required" ? props.requiredKind : props.preferenceKind;
  return entry?.support === "implemented";
}
function classDisabled(ruleClass: WorkforceSetupRuleClass): boolean {
  return props.disabled || props.readOnly || !props.classEditable || !implemented(ruleClass);
}
function classDescription(ruleClass: WorkforceSetupRuleClass): string {
  return [
    describedBy.value,
    `${fieldId.value}-${ruleClass}-help`,
    !implemented(ruleClass) ? `${fieldId.value}-${ruleClass}-unavailable` : "",
  ]
    .filter(Boolean)
    .join(" ");
}
function changeClass(value: WorkforceSetupRuleClass) {
  if (classDisabled(value) || value === props.modelValue) return;
  emit("update:modelValue", value);
}
function changePriority(event: Event) {
  if (
    props.disabled ||
    props.readOnly ||
    props.modelValue !== "preference" ||
    !implemented("preference")
  )
    return;
  const value = (event.target as HTMLSelectElement).value;
  const priority = priorities.find((item) => item === value);
  if (priority !== undefined && priority !== props.priority) emit("update:priority", priority);
}
</script>

<template>
  <fieldset :id="fieldId" class="form-fields" :disabled="disabled" :aria-describedby="describedBy">
    <legend>{{ label }}</legend>
    <p v-if="description" :id="`${fieldId}-description`" class="field-help">{{ description }}</p>
    <div class="form-columns">
      <div v-for="ruleClass in classes" :key="ruleClass" class="field-stack">
        <label :for="`${fieldId}-${ruleClass}`" class="flex min-h-11 items-center gap-2">
          <input
            :id="`${fieldId}-${ruleClass}`"
            type="radio"
            :name="`${fieldId}-class`"
            :value="ruleClass"
            :checked="modelValue === ruleClass"
            :required="required"
            :disabled="classDisabled(ruleClass)"
            :aria-describedby="classDescription(ruleClass)"
            :aria-invalid="Boolean(error) || undefined"
            @change="changeClass(ruleClass)"
          />
          {{ plannerMessage(`strength.${ruleClass}`) }}
        </label>
        <p :id="`${fieldId}-${ruleClass}-help`" class="field-help">
          {{
            plannerMessage(
              ruleClass === "required" ? "strength.requiredHelp" : "strength.preferenceHelp",
            )
          }}
        </p>
        <p
          v-if="!implemented(ruleClass)"
          :id="`${fieldId}-${ruleClass}-unavailable`"
          class="field-help"
        >
          {{ plannerMessage("strength.unavailable") }}
        </p>
      </div>
    </div>
    <p v-if="!classEditable" :id="`${fieldId}-locked`" class="field-help">
      {{ plannerMessage("strength.classLocked") }}
    </p>
    <div v-if="modelValue === 'preference'" class="field-stack">
      <label :for="`${fieldId}-priority`">{{ plannerMessage("strength.priority") }}</label>
      <select
        :id="`${fieldId}-priority`"
        :value="priority ?? ''"
        :required="required"
        :disabled="disabled || readOnly || !implemented('preference')"
        :aria-describedby="classDescription('preference')"
        :aria-invalid="Boolean(error) || undefined"
        @change="changePriority"
      >
        <option value="" disabled>{{ plannerMessage("strength.choosePriority") }}</option>
        <option v-for="value in priorities" :key="value" :value="value">
          {{ plannerMessage(`strength.${value}`) }}
        </option>
      </select>
    </div>
    <p v-if="error" :id="`${fieldId}-error`" class="text-sm text-danger" role="alert">
      {{ error }}
    </p>
  </fieldset>
</template>
