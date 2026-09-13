<script setup lang="ts">
import { computed, nextTick, ref, useAttrs, useId, watch, type StyleValue } from "vue";
import type { DomainEntityRef } from "../../api/generated";
import type {
  EntityMultiPickerEvents,
  EntityMultiPickerProps,
  EntityPickerPage,
} from "./field-contracts";
import EntityPicker from "./EntityPicker.vue";
import { ENTITY_PAGE_LIMIT, SELECTION_PAGE_SIZE, entityKey, uniqueEntities } from "./entity-picker";
import { plannerMessage } from "./messages";

defineOptions({ inheritAttrs: false });
const props = defineProps<EntityMultiPickerProps>();
const emit = defineEmits<EntityMultiPickerEvents>();
const attrs = useAttrs();
const generatedId = useId();
const fieldId = computed(() => props.id ?? `entities-${generatedId}`);
const host = ref<HTMLElement>();
const selectedList = ref<HTMLElement>();
const selectionPage = ref(0);
const locked = computed(() => props.disabled || props.readOnly);
const selection = computed(() => uniqueEntities(props.modelValue));
const selectedKeys = computed(() => new Set(selection.value.map(entityKey)));
const selectedMetadata = computed(
  () => new Map(props.selectedOptions.map((option) => [entityKey(option.entity), option])),
);
const pageCount = computed(() =>
  Math.max(1, Math.ceil(selection.value.length / SELECTION_PAGE_SIZE)),
);
watch(
  pageCount,
  (count) => {
    selectionPage.value = Math.min(selectionPage.value, count - 1);
  },
  { flush: "sync" },
);
const start = computed(() => selectionPage.value * SELECTION_PAGE_SIZE);
const visibleSelection = computed(() =>
  selection.value.slice(start.value, start.value + SELECTION_PAGE_SIZE),
);
const searchPage = computed<EntityPickerPage>(() => {
  // Check the original page first: filtering must not conceal a producer limit failure.
  if (props.page.status !== "ready" || props.page.options.length > ENTITY_PAGE_LIMIT)
    return props.page;
  return {
    ...props.page,
    options: props.page.options.filter(({ entity }) => !selectedKeys.value.has(entityKey(entity))),
  };
});
const inputAttrs = computed(() =>
  Object.fromEntries(
    Object.entries(attrs).filter(([key]) => !["class", "style", "aria-describedby"].includes(key)),
  ),
);
const describedBy = computed(() =>
  [
    attrs["aria-describedby"],
    props.description && `${fieldId.value}-description`,
    props.error && `${fieldId.value}-error`,
    `${fieldId.value}-selection-summary`,
  ]
    .filter(Boolean)
    .join(" "),
);

function add(entity: DomainEntityRef | null) {
  if (locked.value || !entity || selectedKeys.value.has(entityKey(entity))) return;
  emit("update:modelValue", [...selection.value, { ...entity }]);
}
async function remove(entity: DomainEntityRef, index: number) {
  if (locked.value) return;
  const key = entityKey(entity);
  const removedPosition = start.value + index;
  emit(
    "update:modelValue",
    selection.value.filter((item) => entityKey(item) !== key),
  );
  await nextTick();
  const buttons = selectedList.value?.querySelectorAll<HTMLButtonElement>("button");
  const nextIndex = Math.min(removedPosition, selection.value.length - 1) - start.value;
  const nextButton = buttons?.[nextIndex];
  if (nextButton) nextButton.focus();
  else host.value?.querySelector<HTMLInputElement>('input[role="combobox"]')?.focus();
}
async function changeSelectionPage(page: number) {
  if (props.disabled) return;
  selectionPage.value = Math.max(0, Math.min(page, pageCount.value - 1));
  await nextTick();
  const firstRow = selectedList.value?.querySelector<HTMLElement>("li");
  (firstRow?.querySelector<HTMLButtonElement>("button") ?? firstRow)?.focus();
}
</script>

<template>
  <fieldset
    ref="host"
    class="min-w-0"
    :class="attrs.class"
    :style="attrs.style as StyleValue"
    :disabled="disabled"
    :aria-invalid="!!error || undefined"
    :aria-describedby="describedBy"
  >
    <legend>{{ label }}</legend>
    <p v-if="description" :id="`${fieldId}-description`" class="field-help">{{ description }}</p>
    <EntityPicker
      v-bind="inputAttrs"
      :id="fieldId"
      :label="label"
      :model-value="null"
      :selected-option="null"
      :request-key="requestKey"
      :query="query"
      :page="searchPage"
      :required="required && selection.length === 0"
      :aria-required="required || undefined"
      :disabled="disabled"
      :read-only="readOnly"
      :locale="locale"
      :aria-describedby="describedBy"
      :aria-invalid="!!error || undefined"
      @update:model-value="add"
      @search="emit('search', $event)"
      @next-page="emit('nextPage')"
      @retry="emit('retry')"
    />
    <p :id="`${fieldId}-selection-summary`" class="field-help" role="status">
      {{
        selection.length === 0
          ? plannerMessage("entity.noneSelected")
          : plannerMessage(
              "entity.selectionPage",
              {
                start: start + 1,
                end: Math.min(start + SELECTION_PAGE_SIZE, selection.length),
                total: selection.length,
              },
              locale,
            )
      }}
    </p>
    <ul
      v-if="visibleSelection.length"
      :id="`${fieldId}-selected-items`"
      ref="selectedList"
      class="grid list-none gap-2 p-0"
    >
      <li
        v-for="(entity, index) in visibleSelection"
        :key="entityKey(entity)"
        tabindex="-1"
        class="grid gap-2 rounded-sm border border-line p-3"
      >
        <span v-if="selectedMetadata.get(entityKey(entity))?.label" class="wrap-anywhere">{{
          selectedMetadata.get(entityKey(entity))?.label
        }}</span>
        <span class="monospace text-sm text-muted">{{
          plannerMessage("entity.identity", { ...entity })
        }}</span>
        <button
          v-if="!locked"
          type="button"
          class="button-secondary"
          @click="remove(entity, index)"
        >
          {{
            plannerMessage(
              selectedMetadata.get(entityKey(entity))?.label
                ? "entity.remove"
                : "entity.removeUnnamed",
              {
                label: selectedMetadata.get(entityKey(entity))?.label ?? "",
                kind: entity.kind,
                id: entity.id,
              },
            )
          }}
        </button>
      </li>
    </ul>
    <div v-if="pageCount > 1" class="action-row">
      <button
        type="button"
        class="button-secondary"
        :aria-controls="`${fieldId}-selected-items`"
        :disabled="disabled || selectionPage === 0"
        @click="changeSelectionPage(selectionPage - 1)"
      >
        {{ plannerMessage("entity.previousSelected") }}
      </button>
      <button
        type="button"
        class="button-secondary"
        :aria-controls="`${fieldId}-selected-items`"
        :disabled="disabled || selectionPage === pageCount - 1"
        @click="changeSelectionPage(selectionPage + 1)"
      >
        {{ plannerMessage("entity.nextSelected") }}
      </button>
    </div>
    <p v-if="error" :id="`${fieldId}-error`" class="field-help text-danger" role="alert">
      {{ error }}
    </p>
  </fieldset>
</template>
