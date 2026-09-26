<script setup lang="ts">
import { computed, nextTick, ref, useId } from "vue";
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
const host = ref<HTMLElement>();
const personFields = ref<InstanceType<typeof PersonFields>>();
const supportingFields = ref<InstanceType<typeof SupportingRecordFields>>();
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
async function focusField(path: readonly string[], isCurrent: () => boolean): Promise<boolean> {
  if (!isCurrent()) return false;
  if (props.modelValue.kind !== "person")
    return supportingFields.value?.focusField(path, isCurrent) ?? false;
  if (path.length === 1 && (path[0] === "name" || path[0] === "externalId")) {
    const raw = props.modelValue;
    const suffix = path[0] === "name" ? "name" : "external";
    await nextTick();
    if (!isCurrent() || props.modelValue !== raw) return false;
    const element = document.getElementById(`${prefix}-${suffix}`);
    if (element === null || !host.value?.contains(element)) return false;
    const detail = element.closest("details");
    if (detail && host.value.contains(detail)) detail.open = true;
    element.focus();
    return document.activeElement === element;
  }
  return personFields.value?.focusField(path, isCurrent) ?? false;
}
defineExpose({ focusField });
</script>

<template>
  <div v-if="modelValue.kind === 'person'" ref="host" class="field-stack">
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
    <PersonFields
      ref="personFields"
      v-bind="context"
      :model-value="modelValue.fields"
      @update:model-value="update({ ...modelValue, fields: $event })"
    />
    <details class="setup-more">
      <summary>
        {{
          modelValue.externalIdEnabled
            ? messages.people.externalIdentitySet
            : messages.people.externalIdentity
        }}
      </summary>
      <label>
        <input
          type="checkbox"
          :checked="modelValue.externalIdEnabled"
          :disabled="disabled || readOnly"
          @change="
            update({
              ...modelValue,
              externalIdEnabled: ($event.target as HTMLInputElement).checked,
            })
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
    </details>
  </div>
  <SupportingRecordFields
    v-else
    ref="supportingFields"
    v-bind="context"
    :model-value="modelValue"
    @update:model-value="update"
  />
</template>
