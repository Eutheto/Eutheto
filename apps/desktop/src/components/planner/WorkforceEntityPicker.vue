<script setup lang="ts">
import { computed, onScopeDispose, ref, shallowRef, useId, watch } from "vue";
import {
  getScenarioEntity,
  searchScenarioEntities,
  SetupOperationScope,
  type DomainEntityRef,
} from "../../api/generated";
import type {
  WorkforceSetupEntityContinuation,
  WorkforceSetupEntityKind,
} from "../../api/generated-domain-pack-contracts";
import { safeMessage, type ProjectSummary } from "../../project-home";
import type { LabeledEntityRef } from "../explanations/types";
import type { EntityPickerPage, PlannerFieldProps } from "./field-contracts";
import { entityLabel } from "./entity-picker";
import EntityPicker from "./EntityPicker.vue";
import EntityMultiPicker from "./EntityMultiPicker.vue";

const props = defineProps<
  PlannerFieldProps & {
    readonly kind: WorkforceSetupEntityKind;
    readonly modelValue: readonly string[];
    readonly multiple?: boolean;
    readonly project: ProjectSummary;
    readonly libraryEpoch: number;
    readonly draftReferences?: readonly DomainEntityRef[];
  }
>();
const emit = defineEmits<{ "update:modelValue": [value: readonly string[]] }>();
// Reference labels come only from native projections; unknown names retain the full identity.
const host = ref<HTMLElement>();
const identity = useId();
const fieldProps = computed(() => ({
  id: props.id ?? `${identity}-input`,
  label: props.label,
  description: props.description ?? "",
  error: props.error,
  required: props.required,
  disabled: props.disabled,
  readOnly: props.readOnly,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
const query = ref("");
const requestKey = ref("");
const page = shallowRef<EntityPickerPage>({ status: "idle" });
const labels = shallowRef<ReadonlyMap<string, LabeledEntityRef>>(new Map());
const labelError = ref<string | null>(null);
const selection = computed(() => props.modelValue.map((id) => ({ kind: props.kind, id })));
const selectedLabels = computed(() => [...labels.value.values()]);
let cursor: WorkforceSetupEntityContinuation | null = null;
let scope: SetupOperationScope | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
let generation = 0;

function retainLabels(ids: readonly string[], additions: readonly LabeledEntityRef[] = []): void {
  const available = new Map(labels.value);
  for (const option of additions)
    if (option.entity.kind === props.kind) available.set(option.entity.id, option);
  labels.value = new Map(
    ids.flatMap((id) => {
      const option = available.get(id);
      return option ? [[id, option] as const] : [];
    }),
  );
}
function invalidate(): number {
  if (timer !== null) clearTimeout(timer);
  timer = null;
  scope?.dispose();
  scope = null;
  requestKey.value = `${identity}-${String(++generation)}`;
  return generation;
}
async function fetchPage(
  captured: number,
  continuation: WorkforceSetupEntityContinuation | null,
): Promise<void> {
  if (captured !== generation) return;
  const { scenarioId, revision } = props.project;
  const kind = props.kind;
  const search = query.value;
  const key = requestKey.value;
  let owned: SetupOperationScope | null = null;
  try {
    owned = new SetupOperationScope(scenarioId, revision);
    scope = owned;
    const response = await searchScenarioEntities(owned, {
      kind,
      search,
      limit: 50,
      cursor: continuation,
    }).result;
    if (captured !== generation) return;
    const data = response.result.view.data.result.data;
    if (data.items.some((item) => item.kind !== kind))
      throw new Error("The native query returned a different record kind.");
    const options = data.items.map((item): LabeledEntityRef => ({
      entity: { kind: item.kind, id: item.entityId },
      label: item.name ?? entityLabel({ kind: item.kind, id: item.entityId }, null),
    }));
    retainLabels(props.modelValue, options);
    cursor = data.continuation;
    page.value = {
      status: "ready",
      requestKey: key,
      query: search,
      options,
      hasMore: cursor !== null,
    };
    const selected =
      !props.multiple && props.modelValue.length === 1 ? props.modelValue[0] : undefined;
    const selectedIsDraft = props.draftReferences?.some(
      (reference) => reference.kind === kind && reference.id === selected,
    );
    if (selected && !selectedIsDraft && !labels.value.has(selected)) {
      try {
        const detail = (await getScenarioEntity(owned, { kind, entityId: selected }).result).result
          .view.data.result.data;
        if (captured !== generation) return;
        if (detail.id !== selected || detail.kind !== kind)
          throw new Error("The native detail did not match the selected record.");
        if ("name" in detail && typeof detail.name === "string")
          retainLabels(props.modelValue, [{ entity: { kind, id: selected }, label: detail.name }]);
      } catch (error) {
        // Missing label metadata must not disable the valid page needed to select a replacement.
        if (captured === generation) labelError.value = safeMessage(error);
      }
    }
  } catch (error) {
    if (captured === generation)
      page.value = { status: "error", requestKey: key, query: search, message: safeMessage(error) };
  } finally {
    owned?.dispose();
    if (captured === generation) scope = null;
  }
}
function load(continuation: WorkforceSetupEntityContinuation | null, delayed = false): void {
  labelError.value = null;
  const captured = invalidate();
  page.value = { status: "loading", requestKey: requestKey.value, query: query.value };
  if (delayed)
    timer = setTimeout(() => {
      timer = null;
      void fetchPage(captured, continuation);
    }, 150);
  else void fetchPage(captured, continuation);
}
function search(value: string): void {
  query.value = value;
  cursor = null;
  load(null, true);
}
function select(values: readonly DomainEntityRef[]): void {
  const ids = values.filter((value) => value.kind === props.kind).map((value) => value.id);
  retainLabels(ids, page.value.status === "ready" ? page.value.options : []);
  emit("update:modelValue", ids);
}
function activate(): void {
  if (page.value.status === "idle" && !props.disabled && !props.readOnly) load(null);
}
watch(
  () => props.modelValue,
  (value) => {
    retainLabels(value);
  },
);
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.libraryEpoch,
    () => props.kind,
  ],
  () => {
    invalidate();
    labels.value = new Map();
    labelError.value = null;
    cursor = null;
    page.value = { status: "idle" };
    if (host.value?.contains(document.activeElement)) activate();
  },
  { immediate: true, flush: "sync" },
);
onScopeDispose(() => {
  invalidate();
});
</script>

<template>
  <div ref="host" class="min-w-0" @focusin="activate">
    <EntityMultiPicker
      v-if="multiple"
      v-bind="fieldProps"
      :maxlength="256"
      :model-value="selection"
      :selected-options="selectedLabels"
      :request-key="requestKey"
      :query="query"
      :page="page"
      @update:model-value="select"
      @search="search"
      @next-page="load(cursor)"
      @retry="load(null)"
    />
    <EntityPicker
      v-else
      v-bind="fieldProps"
      :maxlength="256"
      :model-value="selection[0] ?? null"
      :selected-option="selectedLabels[0] ?? null"
      :request-key="requestKey"
      :query="query"
      :page="page"
      @update:model-value="select($event ? [$event] : [])"
      @search="search"
      @next-page="load(cursor)"
      @retry="load(null)"
    />
    <p v-if="labelError" class="field-help" role="status">{{ labelError }}</p>
  </div>
</template>
