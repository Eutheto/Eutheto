<script setup lang="ts">
import { computed, useId } from "vue";
import type { ProjectSummary } from "../project-home";
import type { PeopleRecordDraft } from "../people-record-draft";
import { messages } from "../messages";
import PersonFields from "./PersonFields.vue";
import SupportingRecordFields from "./SupportingRecordFields.vue";

const props = defineProps<{
  modelValue: PeopleRecordDraft;
  project: ProjectSummary;
  libraryEpoch: number;
  locale?: string;
  disabled?: boolean;
  readOnly?: boolean;
  showErrors?: boolean;
  errors?: Readonly<Record<string, string>>;
}>();
const emit = defineEmits<{ "update:modelValue": [value: PeopleRecordDraft] }>();
const prefix = useId();
const context = computed(() => ({
  project: props.project,
  libraryEpoch: props.libraryEpoch,
  disabled: props.disabled,
  readOnly: props.readOnly,
  showErrors: props.showErrors,
  errors: props.errors ?? {},
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
function update(value: PeopleRecordDraft): void {
  if (!props.disabled && !props.readOnly) emit("update:modelValue", value);
}
function text(event: Event): string {
  return (event.target as HTMLInputElement).value;
}
</script>

<template>
  <div v-if="modelValue.kind === 'person'" class="field-stack">
    <label :for="`${prefix}-name`">{{ messages.people.name }}</label>
    <input
      :id="`${prefix}-name`"
      name="name"
      type="text"
      required
      :value="modelValue.name"
      :disabled="disabled"
      :readonly="readOnly"
      @input="update({ ...modelValue, name: text($event) })"
    />
    <label>
      <input
        type="checkbox"
        :checked="modelValue.externalIdEnabled"
        :disabled="disabled || readOnly"
        @change="
          update({ ...modelValue, externalIdEnabled: ($event.target as HTMLInputElement).checked })
        "
      />
      {{ messages.people.externalIdEnabled }}
    </label>
    <template v-if="modelValue.externalIdEnabled">
      <label :for="`${prefix}-external`">{{ messages.people.externalId }}</label>
      <input
        :id="`${prefix}-external`"
        name="externalId"
        type="text"
        required
        :value="modelValue.externalId"
        :disabled="disabled"
        :readonly="readOnly"
        :aria-describedby="`${prefix}-external-help`"
        @input="update({ ...modelValue, externalId: text($event) })"
      />
    </template>
    <p :id="`${prefix}-external-help`" class="field-help">{{ messages.people.externalIdHelp }}</p>
    <PersonFields
      v-bind="context"
      :model-value="modelValue.fields"
      @update:model-value="update({ ...modelValue, fields: $event })"
    />
  </div>
  <SupportingRecordFields
    v-else
    v-bind="context"
    :model-value="modelValue"
    @update:model-value="update"
  />
</template>
