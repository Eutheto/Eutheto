<script setup lang="ts">
import { computed, useId } from "vue";
import type { DomainEntityRef } from "../api/generated";
import { messages } from "../messages";
import type { ProjectSummary } from "../project-home";
import { supportingRecordValue, type SupportingRecordDraft } from "../supporting-record-fields";
import DurationField from "./planner/DurationField.vue";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";

const props = defineProps<{
  readonly modelValue: SupportingRecordDraft;
  readonly project: ProjectSummary;
  readonly libraryEpoch: number;
  readonly draftReferences?: readonly DomainEntityRef[];
  readonly locale?: string;
  readonly disabled?: boolean;
  readonly readOnly?: boolean;
  readonly showErrors?: boolean;
  readonly errors?: Readonly<Record<string, string>>;
}>();
const emit = defineEmits<{ "update:modelValue": [value: SupportingRecordDraft] }>();
const copy = messages.supportingFields;
const prefix = useId();
const locked = computed(() => props.disabled || props.readOnly);
const errors = computed(() => ({
  ...(props.showErrors ? supportingRecordValue("", props.modelValue).errors : {}),
  ...props.errors,
}));
const fieldContext = computed(() => ({
  disabled: props.disabled,
  readOnly: props.readOnly,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
const pickerContext = computed(() => ({
  ...fieldContext.value,
  project: props.project,
  libraryEpoch: props.libraryEpoch,
  ...(props.draftReferences === undefined ? {} : { draftReferences: props.draftReferences }),
}));
type AssignmentDraft = Extract<SupportingRecordDraft, { kind: "assignmentType" }>;

function text(event: Event): string {
  return (event.target as HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement).value;
}
function setName(event: Event): void {
  if (!locked.value) emit("update:modelValue", { ...props.modelValue, name: text(event) });
}
function setDescription(event: Event): void {
  if (!locked.value && props.modelValue.kind === "qualification")
    emit("update:modelValue", { ...props.modelValue, description: text(event) });
}
function setAssignment<K extends keyof Omit<AssignmentDraft, "kind">>(
  field: K,
  value: AssignmentDraft[K],
): void {
  if (!locked.value && props.modelValue.kind === "assignmentType")
    emit("update:modelValue", { ...props.modelValue, [field]: value });
}
function qualificationMode(event: Event): void {
  const value = text(event);
  if (value === "" || value === "unconstrained" || value === "matches")
    setAssignment("qualificationMode", value);
}
function locationMode(event: Event): void {
  const value = text(event);
  if (
    value === "" ||
    value === "none" ||
    value === "optional" ||
    value === "required" ||
    value === "fixed"
  )
    setAssignment("locationMode", value);
}
function timeBehavior(event: Event): void {
  const value = text(event);
  if (value === "" || value === "localWallClock" || value === "elapsed")
    setAssignment("timeBehavior", value);
}
</script>

<template>
  <div class="field-stack">
    <p :id="`${prefix}-native-help`" class="field-help">{{ copy.nativeValidation }}</p>
    <div class="field-stack">
      <label :for="`${prefix}-name`">{{ copy.name }}</label>
      <input
        :id="`${prefix}-name`"
        name="name"
        type="text"
        required
        :disabled="disabled"
        :readonly="readOnly"
        :value="modelValue.name"
        :aria-invalid="Boolean(errors.name) || undefined"
        :aria-describedby="`${prefix}-native-help ${prefix}-name-error`"
        @input="setName"
      />
      <p :id="`${prefix}-name-error`" class="text-danger" role="status">{{ errors.name }}</p>
    </div>

    <div v-if="modelValue.kind === 'qualification'" class="field-stack">
      <label :for="`${prefix}-description`">{{ copy.description }}</label>
      <textarea
        :id="`${prefix}-description`"
        name="description"
        rows="4"
        :disabled="disabled"
        :readonly="readOnly"
        :value="modelValue.description"
        :aria-invalid="Boolean(errors.description) || undefined"
        :aria-describedby="`${prefix}-description-help ${prefix}-description-error`"
        @input="setDescription"
      />
      <p :id="`${prefix}-description-help`" class="field-help">{{ copy.descriptionHelp }}</p>
      <p :id="`${prefix}-description-error`" class="text-danger" role="status">
        {{ errors.description }}
      </p>
    </div>

    <template v-if="modelValue.kind === 'assignmentType'">
      <div class="field-stack">
        <label :for="`${prefix}-category`">{{ copy.category }}</label>
        <input
          :id="`${prefix}-category`"
          name="category"
          type="text"
          required
          :disabled="disabled"
          :readonly="readOnly"
          :value="modelValue.category"
          :aria-invalid="Boolean(errors.category) || undefined"
          :aria-describedby="`${prefix}-native-help ${prefix}-category-error`"
          @input="setAssignment('category', text($event))"
        />
        <p :id="`${prefix}-category-error`" class="text-danger" role="status">
          {{ errors.category }}
        </p>
      </div>
      <DurationField
        v-bind="fieldContext"
        :id="`${prefix}-duration`"
        :label="copy.duration"
        :description="copy.durationHelp"
        :model-value="modelValue.duration"
        :minimum="1"
        required
        :error="errors.duration"
        @update:model-value="setAssignment('duration', $event)"
      />

      <div class="field-stack">
        <label :for="`${prefix}-qualification-mode`">{{ copy.qualificationMode }}</label>
        <select
          :id="`${prefix}-qualification-mode`"
          name="qualificationMode"
          required
          :disabled="locked"
          :value="modelValue.qualificationMode"
          :aria-invalid="Boolean(errors.qualificationMode) || undefined"
          :aria-describedby="`${prefix}-qualification-help ${prefix}-qualification-mode-error`"
          @change="qualificationMode"
        >
          <option value="">{{ copy.chooseQualificationMode }}</option>
          <option value="unconstrained">{{ copy.unconstrained }}</option>
          <option value="matches">{{ copy.matches }}</option>
        </select>
        <p :id="`${prefix}-qualification-help`" class="field-help">{{ copy.qualificationHelp }}</p>
        <p :id="`${prefix}-qualification-mode-error`" class="text-danger" role="status">
          {{ errors.qualificationMode }}
        </p>
      </div>
      <!-- Keep inactive branch controls mounted: selections and focus order survive mode changes. -->
      <WorkforceEntityPicker
        v-bind="pickerContext"
        :id="`${prefix}-all-qualifications`"
        kind="qualification"
        :label="copy.allQualifications"
        multiple
        :disabled="disabled || modelValue.qualificationMode !== 'matches'"
        :model-value="modelValue.allQualificationIds"
        :error="errors.allQualificationIds"
        @update:model-value="setAssignment('allQualificationIds', $event)"
      />
      <WorkforceEntityPicker
        v-bind="pickerContext"
        :id="`${prefix}-any-qualifications`"
        kind="qualification"
        :label="copy.anyQualifications"
        multiple
        :disabled="disabled || modelValue.qualificationMode !== 'matches'"
        :model-value="modelValue.anyQualificationIds"
        :error="errors.anyQualificationIds"
        @update:model-value="setAssignment('anyQualificationIds', $event)"
      />

      <div class="field-stack">
        <label :for="`${prefix}-location-mode`">{{ copy.locationMode }}</label>
        <select
          :id="`${prefix}-location-mode`"
          name="locationMode"
          required
          :disabled="locked"
          :value="modelValue.locationMode"
          :aria-invalid="Boolean(errors.locationMode) || undefined"
          :aria-describedby="`${prefix}-location-help ${prefix}-location-mode-error`"
          @change="locationMode"
        >
          <option value="">{{ copy.chooseLocationMode }}</option>
          <option value="none">{{ copy.noLocation }}</option>
          <option value="optional">{{ copy.optionalLocation }}</option>
          <option value="required">{{ copy.requiredLocation }}</option>
          <option value="fixed">{{ copy.fixedLocation }}</option>
        </select>
        <p :id="`${prefix}-location-help`" class="field-help">{{ copy.locationHelp }}</p>
        <p :id="`${prefix}-location-mode-error`" class="text-danger" role="status">
          {{ errors.locationMode }}
        </p>
      </div>
      <WorkforceEntityPicker
        v-bind="pickerContext"
        :id="`${prefix}-fixed-location`"
        kind="location"
        :label="copy.fixedLocation"
        :disabled="disabled || modelValue.locationMode !== 'fixed'"
        :required="modelValue.locationMode === 'fixed'"
        :model-value="modelValue.locationId ? [modelValue.locationId] : []"
        :error="errors.locationId"
        @update:model-value="setAssignment('locationId', $event[0] ?? '')"
      />

      <div class="field-stack">
        <label :for="`${prefix}-time-behavior`">{{ copy.timeBehavior }}</label>
        <select
          :id="`${prefix}-time-behavior`"
          name="timeBehavior"
          required
          :disabled="locked"
          :value="modelValue.timeBehavior"
          :aria-invalid="Boolean(errors.timeBehavior) || undefined"
          :aria-describedby="`${prefix}-time-behavior-help ${prefix}-time-behavior-error`"
          @change="timeBehavior"
        >
          <option value="">{{ copy.chooseTimeBehavior }}</option>
          <option value="localWallClock">{{ copy.localWallClock }}</option>
          <option value="elapsed">{{ copy.elapsed }}</option>
        </select>
        <p :id="`${prefix}-time-behavior-help`" class="field-help">{{ copy.timeBehaviorHelp }}</p>
        <p :id="`${prefix}-time-behavior-error`" class="text-danger" role="status">
          {{ errors.timeBehavior }}
        </p>
      </div>
      <WorkforceEntityPicker
        v-bind="pickerContext"
        :id="`${prefix}-workload-buckets`"
        kind="workloadBucket"
        :label="copy.workloadBuckets"
        multiple
        :model-value="modelValue.workloadBucketIds"
        :error="errors.workloadBucketIds"
        @update:model-value="setAssignment('workloadBucketIds', $event)"
      />
    </template>
  </div>
</template>
