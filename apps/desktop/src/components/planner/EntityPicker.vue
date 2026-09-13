<script setup lang="ts">
import { ComboboxContent, ComboboxInput, ComboboxItem, ComboboxRoot } from "reka-ui";
import { computed, ref, useAttrs, useId, watch, watchPostEffect, type StyleValue } from "vue";
import type { EntityPickerEvents, EntityPickerProps } from "./field-contracts";
import { ENTITY_PAGE_LIMIT, entityKey, entityLabel } from "./entity-picker";
import { plannerMessage } from "./messages";

defineOptions({ inheritAttrs: false });
const props = defineProps<EntityPickerProps>();
const emit = defineEmits<EntityPickerEvents>();
const attrs = useAttrs();
const generatedId = useId();
const fieldId = computed(() => props.id ?? `entity-${generatedId}`);
const host = ref<HTMLElement>();
const open = ref(false);
const localQuery = ref(props.query);
const pendingQuery = ref<string | null>(null);
const invalidatedRequestKey = ref<string | null>(null);
const locked = computed(() => props.disabled || props.readOnly);
const inputAttrs = computed(() =>
  Object.fromEntries(
    Object.entries(attrs).filter(([key]) => !["class", "style", "aria-describedby"].includes(key)),
  ),
);
const describedBy = computed(
  () =>
    [
      attrs["aria-describedby"],
      props.description && `${fieldId.value}-description`,
      props.error && `${fieldId.value}-error`,
      props.modelValue && `${fieldId.value}-selection`,
      props.readOnly && `${fieldId.value}-readonly`,
    ]
      .filter(Boolean)
      .join(" ") || undefined,
);

watch(
  () => props.query,
  (query) => {
    // Do not let a delayed parent echo replace a more recent local search.
    if (pendingQuery.value !== null && query !== pendingQuery.value) return;
    localQuery.value = query;
    pendingQuery.value = null;
  },
  { flush: "sync" },
);
watch(
  locked,
  (value) => {
    if (value) open.value = false;
  },
  { flush: "sync" },
);

const pageMatches = computed(
  () =>
    props.page.status !== "idle" &&
    props.requestKey !== invalidatedRequestKey.value &&
    props.page.requestKey === props.requestKey &&
    props.page.query === props.query &&
    props.page.query === localQuery.value,
);
const oversized = computed(
  () =>
    pageMatches.value &&
    props.page.status === "ready" &&
    props.page.options.length > ENTITY_PAGE_LIMIT,
);
const options = computed(() => {
  if (!pageMatches.value || oversized.value || props.page.status !== "ready") return [];
  const seen = new Set<string>();
  return props.page.options.filter(({ entity }) => {
    const key = entityKey(entity);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
});
const selectedLabel = computed(() =>
  props.modelValue ? entityLabel(props.modelValue, props.selectedOption) : "",
);
const stateMessage = computed(() => {
  if (oversized.value) return plannerMessage("entity.pageLimit");
  if (props.page.status === "idle" && localQuery.value === props.query)
    return plannerMessage("entity.idle");
  if (!pageMatches.value) return plannerMessage("entity.stale");
  if (props.page.status === "loading") return plannerMessage("entity.loading");
  if (props.page.status === "error") return props.page.message;
  if (options.value.length === 0) return plannerMessage("entity.empty");
  return "";
});

// Capture even intermediate IME text to invalidate old options immediately.
// Reka alone decides when composition has completed and emits a search value.
function captureInput(event: Event) {
  if (locked.value || !(event.target instanceof HTMLInputElement)) return;
  invalidatedRequestKey.value = props.requestKey;
  localQuery.value = event.target.value;
  pendingQuery.value = event.target.value;
}
function search(query: string) {
  if (locked.value) return;
  invalidatedRequestKey.value = props.requestKey;
  localQuery.value = query;
  pendingQuery.value = query === props.query ? null : query;
  emit("search", query);
}
function select(value: unknown) {
  if (locked.value || typeof value !== "string") return;
  const option = options.value.find(({ entity }) => entityKey(entity) === value);
  // Ignore Reka's toggle-to-undefined and never admit a stale highlighted item.
  if (option && (!props.modelValue || entityKey(props.modelValue) !== value))
    emit("update:modelValue", { ...option.entity });
}
function clear() {
  if (locked.value || !props.modelValue) return;
  emit("update:modelValue", null);
  focusSearch();
}
function focusSearch() {
  host.value?.querySelector<HTMLInputElement>('input[role="combobox"]')?.focus();
}
function nextPage() {
  if (
    locked.value ||
    !pageMatches.value ||
    oversized.value ||
    props.page.status !== "ready" ||
    !props.page.hasMore
  )
    return;
  emit("nextPage");
  focusSearch();
}
function retry() {
  if (locked.value || !pageMatches.value || props.page.status !== "error") return;
  emit("retry");
  focusSearch();
}
watchPostEffect(() => {
  const input = host.value?.querySelector<HTMLInputElement>('input[role="combobox"]');
  input?.setCustomValidity(
    props.required && !props.modelValue && !locked.value ? plannerMessage("entity.required") : "",
  );
});
</script>

<template>
  <div ref="host" class="field-stack" :class="attrs.class" :style="attrs.style as StyleValue">
    <label :id="`${fieldId}-label`" :for="fieldId">{{
      plannerMessage("entity.search", { label })
    }}</label>
    <p v-if="description" :id="`${fieldId}-description`" class="field-help">{{ description }}</p>
    <ComboboxRoot
      :model-value="modelValue ? entityKey(modelValue) : null"
      :open="open && !locked"
      :disabled="disabled"
      :ignore-filter="true"
      :reset-model-value-on-clear="false"
      :reset-search-term-on-blur="false"
      :reset-search-term-on-select="false"
      :open-on-focus="!locked"
      :open-on-click="!locked"
      @update:open="open = $event && !locked"
      @update:model-value="select"
    >
      <ComboboxInput
        v-bind="inputAttrs"
        :id="fieldId"
        :model-value="localQuery"
        :readonly="readOnly"
        :disabled="disabled"
        :aria-required="
          required ||
          attrs['aria-required'] === true ||
          attrs['aria-required'] === 'true' ||
          undefined
        "
        :aria-invalid="
          !!error || attrs['aria-invalid'] === true || attrs['aria-invalid'] === 'true' || undefined
        "
        :aria-describedby="describedBy"
        :aria-label="plannerMessage('entity.search', { label })"
        @input.capture="captureInput"
        @compositionend.capture="captureInput"
        @update:model-value="search"
      />
      <ComboboxContent
        :aria-labelledby="`${fieldId}-label`"
        class="max-h-64 overflow-y-auto rounded-sm border border-line bg-raised p-1"
      >
        <ComboboxItem
          v-for="option in options"
          :key="JSON.stringify([requestKey, option.entity.kind, option.entity.id])"
          :value="entityKey(option.entity)"
          :text-value="`${option.label} ${plannerMessage('entity.identity', { ...option.entity })}`"
          class="grid min-h-11 cursor-pointer gap-1 rounded-sm p-3 text-ink data-[highlighted]:bg-accent-soft data-[highlighted]:outline data-[highlighted]:outline-focus"
        >
          <span class="wrap-anywhere">{{ option.label }}</span>
          <span class="monospace text-sm text-muted">{{
            plannerMessage("entity.identity", { ...option.entity })
          }}</span>
        </ComboboxItem>
      </ComboboxContent>
    </ComboboxRoot>
    <p
      v-if="stateMessage"
      role="status"
      class="field-help"
      :class="{ 'text-danger': oversized || (pageMatches && page.status === 'error') }"
    >
      {{ stateMessage }}
    </p>
    <div v-if="pageMatches && !oversized && !locked" class="action-row">
      <template v-if="page.status === 'ready' && page.hasMore">
        <span class="field-help">{{ plannerMessage("entity.more") }}</span>
        <button type="button" class="button-secondary" @click="nextPage">
          {{ plannerMessage("entity.nextMatches") }}
        </button>
      </template>
      <button v-if="page.status === 'error'" type="button" class="button-secondary" @click="retry">
        {{ plannerMessage("action.retry") }}
      </button>
    </div>
    <div v-if="modelValue" :id="`${fieldId}-selection`" class="field-stack">
      <span class="wrap-anywhere">{{
        plannerMessage("entity.selected", { label: selectedLabel })
      }}</span>
      <span class="monospace text-sm text-muted">{{
        plannerMessage("entity.identity", { ...modelValue })
      }}</span>
      <button v-if="!locked" type="button" class="button-secondary" @click="clear">
        {{ plannerMessage("action.clear") }}
      </button>
    </div>
    <p v-if="readOnly" :id="`${fieldId}-readonly`" class="field-help">
      {{ plannerMessage("entity.readOnly") }}
    </p>
    <p v-if="error" :id="`${fieldId}-error`" class="field-help text-danger" role="alert">
      {{ error }}
    </p>
  </div>
</template>
