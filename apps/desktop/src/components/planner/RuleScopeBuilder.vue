<script setup lang="ts">
import { computed, nextTick, reactive, ref, useId, watch } from "vue";
import type { DomainEntityRef } from "../../api/generated";
import type {
  WorkforceScope,
  WorkforceSetupScopeAxis,
  WorkforceWeekday,
} from "../../api/generated-domain-pack-contracts";
import EntityMultiPicker from "./EntityMultiPicker.vue";
import ScopeTextInput from "./ScopeTextInput.vue";
import { plannerMessage } from "./messages";
import {
  scopeEntities,
  scopeEntityKinds,
  scopeWeekdays,
  toggleScopeFilter,
  type RuleScopeBuilderEvents,
  type RuleScopeBuilderProps,
  type ScopeEntityField,
  type ScopeFieldName,
  type ScopeOptionalField,
  type ScopePreviewIntent,
  type ScopeTextField,
} from "./scope-field";

const props = defineProps<RuleScopeBuilderProps>();
const emit = defineEmits<RuleScopeBuilderEvents>();
const generatedId = useId();
const fieldId = computed(() => props.id ?? `scope-${generatedId}`);
const locked = computed(() => props.disabled || props.readOnly);
const host = ref<HTMLElement>();
const previewStatus = ref<HTMLElement>();
const blockedKey = ref<string | null>(null);
const idFilters = ["teamIds", "assignmentTypeIds", "locationIds"] as const;
const textFields = ["allTags", "anyTags", "categories"] as const;
const rowKeys = reactive<Record<ScopeTextField, string[]>>({
  allTags: [],
  anyTags: [],
  categories: [],
});
let nextRowKey = 0;
const descriptions = computed(() =>
  [
    `${fieldId.value}-help`,
    props.description && `${fieldId.value}-description`,
    props.error && `${fieldId.value}-error`,
    props.readOnly && `${fieldId.value}-readonly`,
  ]
    .filter(Boolean)
    .join(" "),
);

function fieldLabel(field: ScopeFieldName) {
  return plannerMessage(`scope.${field}`);
}
function links(field: ScopeFieldName) {
  return [
    descriptions.value,
    `${fieldId.value}-${field}-help`,
    props.fieldErrors?.[field] && `${fieldId.value}-${field}-error`,
  ]
    .filter(Boolean)
    .join(" ");
}
function values(field: ScopeTextField): readonly string[] {
  if (field === "categories") return props.modelValue.categories ?? [];
  return props.modelValue.people.kind === "filter" ? props.modelValue.people[field] : [];
}
for (const field of textFields) {
  watch(
    () => values(field).length,
    (length) => {
      rowKeys[field].splice(length);
      while (rowKeys[field].length < length)
        rowKeys[field].push(`row-${(nextRowKey++).toString()}`);
    },
    { immediate: true, flush: "sync" },
  );
}

// Hide the captured page BEFORE emitting; a delayed parent echo cannot revive it.
function invalidatePreview() {
  blockedKey.value = props.currentRequestKey;
}
watch(
  () => props.currentRequestKey,
  (key) => {
    if (key !== blockedKey.value) blockedKey.value = null;
  },
  { flush: "sync" },
);
function update(scope: WorkforceScope) {
  if (locked.value) return;
  invalidatePreview();
  emit("update:modelValue", scope);
}
function changePeople(event: Event) {
  if (locked.value) return;
  const kind = (event.target as HTMLSelectElement).value;
  if (kind === props.modelValue.people.kind) return;
  const people =
    kind === "all"
      ? { kind: "all" as const }
      : kind === "selected"
        ? { kind: "selected" as const, personIds: [] }
        : { kind: "filter" as const, allTags: [], anyTags: [] };
  update({ ...props.modelValue, people });
}
function toggleFilter(field: ScopeOptionalField) {
  update(toggleScopeFilter(props.modelValue, field));
}
function changeEntities(field: ScopeEntityField, entities: readonly DomainEntityRef[]) {
  if (locked.value || entities.some((entity) => entity.kind !== scopeEntityKinds[field])) return;
  const ids = entities.map((entity) => entity.id);
  update(
    field === "people"
      ? { ...props.modelValue, people: { kind: "selected", personIds: ids } }
      : { ...props.modelValue, [field]: ids },
  );
}
function searchEntities(field: ScopeEntityField, query: string) {
  if (!locked.value) emit("entitySearch", field, query);
}
function nextEntities(field: ScopeEntityField) {
  if (!locked.value) emit("entityNextPage", field);
}
function retryEntities(field: ScopeEntityField) {
  if (!locked.value) emit("entityRetry", field);
}
function updateText(field: ScopeTextField, items: readonly string[]) {
  if (field === "categories") update({ ...props.modelValue, categories: items });
  else if (props.modelValue.people.kind === "filter") {
    update({ ...props.modelValue, people: { ...props.modelValue.people, [field]: items } });
  }
}
function editText(field: ScopeTextField, index: number, value: string) {
  const items = [...values(field)];
  items[index] = value;
  updateText(field, items);
}
async function addText(field: ScopeTextField) {
  if (locked.value) return;
  updateText(field, [...values(field), ""]);
  await nextTick();
  const rows = host.value?.querySelectorAll<HTMLTextAreaElement>(`[data-scope-text="${field}"]`);
  rows?.[rows.length - 1]?.focus();
}
async function removeText(field: ScopeTextField, index: number) {
  if (locked.value) return;
  rowKeys[field].splice(index, 1);
  updateText(
    field,
    values(field).filter((_, row) => row !== index),
  );
  await nextTick();
  const rows = host.value?.querySelectorAll<HTMLTextAreaElement>(`[data-scope-text="${field}"]`);
  const target = rows?.[Math.min(index, rows.length - 1)];
  if (target) target.focus();
  else host.value?.querySelector<HTMLButtonElement>(`[data-scope-add="${field}"]`)?.focus();
}
function changeWeekday(day: WorkforceWeekday, event: Event) {
  if (props.modelValue.weekdays === undefined) return;
  const checked = (event.target as HTMLInputElement).checked;
  update({
    ...props.modelValue,
    weekdays: checked
      ? [...props.modelValue.weekdays, day]
      : props.modelValue.weekdays.filter((value) => value !== day),
  });
}

const previewFresh = computed(
  () =>
    props.preview.status !== "idle" &&
    blockedKey.value === null &&
    props.preview.requestKey === props.currentRequestKey,
);
const ready = computed(() =>
  previewFresh.value && props.preview.status === "ready" ? props.preview.inspection : null,
);
const pageInvalid = computed(
  () =>
    !!ready.value &&
    (!Number.isInteger(props.previewLimit) ||
      props.previewLimit < 1 ||
      props.previewLimit > 200 ||
      ready.value.population.axis !== props.previewAxis ||
      ready.value.population.page.items.length > props.previewLimit),
);
const inspection = computed(() => (pageInvalid.value ? null : ready.value));
const previewMessage = computed(() => {
  if (blockedKey.value !== null || (props.preview.status !== "idle" && !previewFresh.value))
    return plannerMessage("scope.previewStale");
  if (pageInvalid.value) return plannerMessage("scope.previewInvalid");
  if (props.preview.status === "idle") return plannerMessage("scope.previewIdle");
  if (props.preview.status === "loading") return plannerMessage("scope.previewLoading");
  if (props.preview.status === "error") return props.preview.message;
  return inspection.value?.population.page.items.length === 0
    ? plannerMessage("scope.previewEmpty")
    : plannerMessage("scope.previewReady");
});
async function requestPreview(intent: ScopePreviewIntent, moveFocus = true) {
  if (props.disabled) return;
  invalidatePreview();
  emit("preview", intent);
  if (moveFocus) {
    await nextTick();
    previewStatus.value?.focus();
  }
}
function previewAction(kind: "preview" | "retry" | "firstPage") {
  void requestPreview({ kind, axis: props.previewAxis, requestKey: props.currentRequestKey });
}
function changeAxis(event: Event) {
  const axis = (event.target as HTMLSelectElement).value as WorkforceSetupScopeAxis;
  if (axis !== props.previewAxis)
    void requestPreview({ kind: "axis", axis, requestKey: props.currentRequestKey }, false);
}
function nextPreviewPage() {
  const population = inspection.value?.population;
  if (population?.page.continuation)
    void requestPreview({
      kind: "nextPage",
      axis: population.axis,
      continuation: population.page.continuation,
      requestKey: props.currentRequestKey,
    });
}
</script>

<template>
  <fieldset
    ref="host"
    class="grid min-w-0 gap-4"
    :disabled="disabled"
    :aria-describedby="descriptions"
    :aria-invalid="!!error || undefined"
  >
    <legend>{{ label }}</legend>
    <p v-if="description" :id="`${fieldId}-description`" class="field-help">{{ description }}</p>
    <p :id="`${fieldId}-help`" class="field-help">{{ plannerMessage("scope.help") }}</p>
    <p v-if="readOnly" :id="`${fieldId}-readonly`" class="field-help">
      {{ plannerMessage("scope.readOnly") }}
    </p>
    <p v-if="error" :id="`${fieldId}-error`" class="field-help text-danger" role="alert">
      {{ error }}
    </p>

    <div class="grid gap-2">
      <label :for="`${fieldId}-people-mode`">{{ plannerMessage("scope.people") }}</label>
      <select
        :id="`${fieldId}-people-mode`"
        :name="`${fieldId}-people-mode`"
        :value="modelValue.people.kind"
        :disabled="locked"
        :required="required"
        :aria-describedby="links('people')"
        :aria-invalid="!!fieldErrors?.people || undefined"
        @change="changePeople"
      >
        <option value="all">{{ plannerMessage("scope.all") }}</option>
        <option value="selected">{{ plannerMessage("scope.selected") }}</option>
        <option value="filter">{{ plannerMessage("scope.filter") }}</option>
      </select>
      <p :id="`${fieldId}-people-help`" class="field-help">
        {{ plannerMessage("scope.peopleHelp") }}
      </p>
      <p
        v-if="fieldErrors?.people"
        :id="`${fieldId}-people-error`"
        class="field-help text-danger"
        role="alert"
      >
        {{ fieldErrors.people }}
      </p>
      <EntityMultiPicker
        v-if="modelValue.people.kind === 'selected'"
        :id="`${fieldId}-people-picker`"
        :label="plannerMessage('scope.selected')"
        :model-value="scopeEntities(modelValue, 'people')"
        v-bind="entityOptions.people"
        :disabled="disabled"
        :read-only="readOnly"
        :locale="locale"
        :aria-describedby="links('people')"
        :error="fieldErrors?.people"
        @update:model-value="changeEntities('people', $event)"
        @search="searchEntities('people', $event)"
        @next-page="nextEntities('people')"
        @retry="retryEntities('people')"
      />
    </div>

    <fieldset
      v-for="field in idFilters"
      :key="field"
      class="grid min-w-0 gap-2"
      :aria-describedby="links(field)"
    >
      <legend>{{ fieldLabel(field) }}</legend>
      <p :id="`${fieldId}-${field}-help`" class="field-help">
        {{ plannerMessage(modelValue[field] === undefined ? "scope.absent" : "scope.enabled") }}
      </p>
      <button
        type="button"
        class="button-secondary"
        :disabled="locked"
        :aria-describedby="links(field)"
        @click="toggleFilter(field)"
      >
        {{
          plannerMessage(modelValue[field] === undefined ? "scope.enable" : "scope.removeFilter", {
            label: fieldLabel(field),
          })
        }}
      </button>
      <EntityMultiPicker
        v-if="modelValue[field] !== undefined"
        :id="`${fieldId}-${field}-picker`"
        :label="fieldLabel(field)"
        :model-value="scopeEntities(modelValue, field)"
        v-bind="entityOptions[field]"
        :disabled="disabled"
        :read-only="readOnly"
        :locale="locale"
        :aria-describedby="links(field)"
        :error="fieldErrors?.[field]"
        @update:model-value="changeEntities(field, $event)"
        @search="searchEntities(field, $event)"
        @next-page="nextEntities(field)"
        @retry="retryEntities(field)"
      />
      <p
        v-if="fieldErrors?.[field]"
        :id="`${fieldId}-${field}-error`"
        class="field-help text-danger"
        role="alert"
      >
        {{ fieldErrors[field] }}
      </p>
    </fieldset>

    <template v-for="field in textFields" :key="field">
      <fieldset
        v-if="field === 'categories' || modelValue.people.kind === 'filter'"
        class="grid min-w-0 gap-2"
        :aria-describedby="links(field)"
      >
        <legend>{{ fieldLabel(field) }}</legend>
        <p :id="`${fieldId}-${field}-help`" class="field-help">
          {{
            plannerMessage(
              field === "categories"
                ? "scope.categoriesHelp"
                : field === "allTags"
                  ? "scope.allTagsHelp"
                  : "scope.anyTagsHelp",
            )
          }}
        </p>
        <template v-if="field === 'categories'">
          <p class="field-help">
            {{
              plannerMessage(modelValue.categories === undefined ? "scope.absent" : "scope.enabled")
            }}
          </p>
          <button
            type="button"
            class="button-secondary"
            :disabled="locked"
            :aria-describedby="links(field)"
            @click="toggleFilter('categories')"
          >
            {{
              plannerMessage(
                modelValue.categories === undefined ? "scope.enable" : "scope.removeFilter",
                { label: fieldLabel(field) },
              )
            }}
          </button>
        </template>
        <template v-if="field !== 'categories' || modelValue.categories !== undefined">
          <p v-if="values(field).length === 0" class="field-help">
            {{ plannerMessage("scope.emptyList") }}
          </p>
          <div
            v-for="(value, index) in values(field)"
            :key="rowKeys[field][index]"
            class="grid gap-2 rounded-sm border border-line p-3"
          >
            <label :for="`${fieldId}-${rowKeys[field][index]}`">{{
              plannerMessage("scope.entry", { label: fieldLabel(field), number: index + 1 }, locale)
            }}</label>
            <ScopeTextInput
              :id="`${fieldId}-${rowKeys[field][index]}`"
              :name="`${fieldId}-${field}-${rowKeys[field][index]}`"
              :data-scope-text="field"
              :model-value="value"
              rows="2"
              :readonly="readOnly"
              :disabled="disabled"
              :aria-describedby="links(field)"
              :aria-invalid="!!fieldErrors?.[field] || undefined"
              @update:model-value="editText(field, index, $event)"
            />
            <button
              type="button"
              class="button-secondary"
              :disabled="locked"
              @click="removeText(field, index)"
            >
              {{
                plannerMessage(
                  "scope.removeEntry",
                  { label: fieldLabel(field), number: index + 1 },
                  locale,
                )
              }}
            </button>
          </div>
          <button
            type="button"
            class="button-secondary"
            :data-scope-add="field"
            :disabled="locked"
            :aria-describedby="links(field)"
            @click="addText(field)"
          >
            {{ plannerMessage("scope.addEntry", { label: fieldLabel(field) }) }}
          </button>
        </template>
        <p
          v-if="fieldErrors?.[field]"
          :id="`${fieldId}-${field}-error`"
          class="field-help text-danger"
          role="alert"
        >
          {{ fieldErrors[field] }}
        </p>
      </fieldset>
    </template>

    <fieldset class="grid gap-2" :aria-describedby="links('weekdays')">
      <legend>{{ plannerMessage("scope.weekdays") }}</legend>
      <p :id="`${fieldId}-weekdays-help`" class="field-help">
        {{ plannerMessage(modelValue.weekdays === undefined ? "scope.absent" : "scope.enabled") }}
      </p>
      <button
        type="button"
        class="button-secondary"
        :disabled="locked"
        :aria-describedby="links('weekdays')"
        @click="toggleFilter('weekdays')"
      >
        {{
          plannerMessage(
            modelValue.weekdays === undefined ? "scope.enable" : "scope.removeFilter",
            { label: plannerMessage("scope.weekdays") },
          )
        }}
      </button>
      <template v-if="modelValue.weekdays !== undefined">
        <p v-if="modelValue.weekdays.length === 0" class="field-help">
          {{ plannerMessage("scope.emptyList") }}
        </p>
        <label
          v-for="day in scopeWeekdays"
          :key="day"
          :for="`${fieldId}-${day}`"
          class="flex items-center gap-2"
        >
          <input
            :id="`${fieldId}-${day}`"
            :name="`${fieldId}-weekdays`"
            type="checkbox"
            :value="day"
            :checked="modelValue.weekdays.includes(day)"
            :disabled="locked"
            :aria-describedby="links('weekdays')"
            :aria-invalid="!!fieldErrors?.weekdays || undefined"
            @change="changeWeekday(day, $event)"
          />
          {{ plannerMessage(`scope.${day}`) }}
        </label>
      </template>
      <p
        v-if="fieldErrors?.weekdays"
        :id="`${fieldId}-weekdays-error`"
        class="field-help text-danger"
        role="alert"
      >
        {{ fieldErrors.weekdays }}
      </p>
    </fieldset>

    <fieldset
      class="grid min-w-0 gap-3 rounded-sm border border-line p-3"
      :aria-describedby="`${fieldId}-preview-help`"
    >
      <legend>{{ plannerMessage("scope.previewTitle") }}</legend>
      <p :id="`${fieldId}-preview-help`" class="field-help">
        {{ plannerMessage("scope.previewHelp") }}
      </p>
      <label :for="`${fieldId}-axis`">{{ plannerMessage("scope.axis") }}</label>
      <select
        :id="`${fieldId}-axis`"
        :name="`${fieldId}-axis`"
        :value="previewAxis"
        :disabled="disabled"
        :aria-describedby="`${fieldId}-preview-help ${fieldId}-preview-status`"
        @change="changeAxis"
      >
        <option value="people">{{ plannerMessage("scope.people") }}</option>
        <option value="shifts">{{ plannerMessage("scope.shifts") }}</option>
      </select>
      <div class="action-row">
        <button
          type="button"
          class="button-secondary"
          :disabled="disabled"
          :aria-controls="`${fieldId}-population`"
          @click="previewAction('preview')"
        >
          {{ plannerMessage("scope.previewAction") }}
        </button>
        <button
          v-if="previewFresh && preview.status === 'error'"
          type="button"
          class="button-secondary"
          :disabled="disabled"
          :aria-describedby="`${fieldId}-preview-status`"
          @click="previewAction('retry')"
        >
          {{ plannerMessage("action.retry") }}
        </button>
      </div>
      <p
        :id="`${fieldId}-preview-status`"
        ref="previewStatus"
        class="field-help"
        tabindex="-1"
        role="status"
        aria-live="polite"
      >
        {{ previewMessage }}
      </p>
      <div :id="`${fieldId}-population`" :aria-busy="previewFresh && preview.status === 'loading'">
        <template v-if="inspection">
          <dl class="grid gap-2">
            <div>
              <dt>{{ plannerMessage("scope.peopleCount") }}</dt>
              <dd>{{ inspection.peopleCount.toLocaleString(locale) }}</dd>
            </div>
            <div>
              <dt>{{ plannerMessage("scope.shiftCount") }}</dt>
              <dd>{{ inspection.shiftCount.toLocaleString(locale) }}</dd>
            </div>
            <div>
              <dt>{{ plannerMessage("scope.pairCount") }}</dt>
              <dd>{{ inspection.cartesianPairCount.toLocaleString(locale) }}</dd>
            </div>
          </dl>
          <p class="field-help">
            {{
              plannerMessage(
                "scope.pageSummary",
                {
                  shown: inspection.population.page.items.length,
                  total: inspection.population.page.totalItems,
                },
                locale,
              )
            }}
          </p>
          <ul v-if="inspection.population.axis === 'people'" class="grid list-none gap-2 p-0">
            <li
              v-for="person in inspection.population.page.items"
              :key="person.personId"
              class="grid gap-1 rounded-sm border border-line p-3"
            >
              <span class="wrap-anywhere whitespace-pre-wrap">{{ person.name }}</span>
              <span class="monospace wrap-anywhere text-sm text-muted">{{
                plannerMessage("scope.personIdentity", { id: person.personId })
              }}</span>
            </li>
          </ul>
          <ul v-else class="grid list-none gap-2 p-0">
            <li
              v-for="shift in inspection.population.page.items"
              :key="shift.shiftId"
              class="grid gap-1 rounded-sm border border-line p-3"
            >
              <span class="monospace wrap-anywhere">{{
                plannerMessage("scope.shiftIdentity", { id: shift.shiftId })
              }}</span>
              <span class="wrap-anywhere whitespace-pre-wrap">{{ shift.assignmentTypeName }}</span>
              <span class="monospace wrap-anywhere text-sm text-muted">{{
                plannerMessage("scope.assignmentIdentity", { id: shift.assignmentTypeId })
              }}</span>
              <template v-if="shift.locationId !== null">
                <span
                  v-if="shift.locationName !== null"
                  class="wrap-anywhere whitespace-pre-wrap"
                  >{{ shift.locationName }}</span
                >
                <span class="monospace wrap-anywhere text-sm text-muted">{{
                  plannerMessage("scope.locationIdentity", { id: shift.locationId })
                }}</span>
              </template>
              <span class="wrap-anywhere">{{
                plannerMessage("scope.startsAt", {
                  local: shift.interval.startsAt.local,
                  instant: shift.interval.startsAt.instant,
                })
              }}</span>
              <span class="wrap-anywhere">{{
                plannerMessage("scope.endsAt", {
                  local: shift.interval.endsAt.local,
                  instant: shift.interval.endsAt.instant,
                })
              }}</span>
            </li>
          </ul>
          <div class="action-row">
            <button
              type="button"
              class="button-secondary"
              :disabled="disabled"
              :aria-controls="`${fieldId}-population`"
              @click="previewAction('firstPage')"
            >
              {{ plannerMessage("scope.firstPage") }}
            </button>
            <button
              v-if="inspection.population.page.continuation !== null"
              type="button"
              class="button-secondary"
              :disabled="disabled"
              :aria-controls="`${fieldId}-population`"
              @click="nextPreviewPage"
            >
              {{ plannerMessage("scope.nextPage") }}
            </button>
          </div>
        </template>
      </div>
    </fieldset>
  </fieldset>
</template>
