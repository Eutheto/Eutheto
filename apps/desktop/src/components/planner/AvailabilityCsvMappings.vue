<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type { ProjectSummary } from "../../project-home";
import { messages } from "../../messages";
import WorkforceEntityPicker from "./WorkforceEntityPicker.vue";

const props = defineProps<{
  project: ProjectSummary;
  libraryEpoch: number;
  personMappings: Readonly<Record<string, string>>;
  assignmentTypeMappings: Readonly<Record<string, string>>;
  locationMappings: Readonly<Record<string, string>>;
  disabled: boolean;
}>();
const emit = defineEmits<{
  "update:personMappings": [value: Readonly<Record<string, string>>];
  "update:assignmentTypeMappings": [value: Readonly<Record<string, string>>];
  "update:locationMappings": [value: Readonly<Record<string, string>>];
}>();
const copy = messages.availabilityCsv;
const endpoint = ref<"person" | "assignmentType" | "location">("person");
const token = ref("");
const endpointId = ref("");
const error = ref("");
const page = ref(0);
const tokenInput = ref<HTMLInputElement>();
const mappings = computed(() => [
  ...Object.entries(props.personMappings).map(([token, id]) => ({
    endpoint: "person" as const,
    token,
    id,
  })),
  ...Object.entries(props.assignmentTypeMappings).map(([token, id]) => ({
    endpoint: "assignmentType" as const,
    token,
    id,
  })),
  ...Object.entries(props.locationMappings).map(([token, id]) => ({
    endpoint: "location" as const,
    token,
    id,
  })),
]);
function valuesFor(
  kind: "person" | "assignmentType" | "location",
): Readonly<Record<string, string>> {
  return kind === "person"
    ? props.personMappings
    : kind === "assignmentType"
      ? props.assignmentTypeMappings
      : props.locationMappings;
}
function publish(
  kind: "person" | "assignmentType" | "location",
  values: Readonly<Record<string, string>>,
): void {
  if (kind === "person") emit("update:personMappings", values);
  else if (kind === "assignmentType") emit("update:assignmentTypeMappings", values);
  else emit("update:locationMappings", values);
}
function add(): void {
  if (props.disabled) return;
  const values = valuesFor(endpoint.value);
  if (
    !token.value ||
    new TextEncoder().encode(token.value).length > 256 ||
    /\p{Cc}/u.test(token.value)
  ) {
    error.value = copy.invalidToken;
    return;
  }
  // Native authority resolves exact UUID spellings and checks the endpoint kind.
  if (!endpointId.value || endpointId.value.length > 256 || /\p{Cc}/u.test(endpointId.value)) {
    error.value = copy.invalidEndpoint;
    return;
  }
  if (Object.hasOwn(values, token.value) || Object.keys(values).length >= 10_000) {
    error.value = copy.duplicateMapping;
    return;
  }
  publish(endpoint.value, { ...values, [token.value]: endpointId.value });
  token.value = "";
  endpointId.value = "";
  error.value = "";
  tokenInput.value?.focus();
}
async function remove(
  kind: "person" | "assignmentType" | "location",
  token: string,
): Promise<void> {
  if (props.disabled) return;
  const origin = document.activeElement;
  const values = { ...valuesFor(kind) };
  Reflect.deleteProperty(values, token);
  publish(kind, values);
  await nextTick();
  page.value = Math.min(page.value, Math.max(0, Math.ceil(mappings.value.length / 20) - 1));
  if (document.activeElement === origin || document.activeElement === document.body)
    tokenInput.value?.focus();
}
watch(endpoint, () => {
  endpointId.value = "";
  error.value = "";
});
</script>

<template>
  <section class="field-stack" aria-labelledby="availability-csv-token-mappings">
    <h4 id="availability-csv-token-mappings">{{ copy.mappings }}</h4>
    <p>{{ copy.mappingsHelp }}</p>
    <fieldset :disabled="disabled" class="field-stack">
      <legend>{{ copy.addMappingLegend }}</legend>
      <label
        >{{ copy.endpoint }}
        <select id="availability-csv-mapping-endpoint" v-model="endpoint">
          <option v-for="(label, kind) in copy.endpoints" :key="kind" :value="kind">
            {{ label }}
          </option>
        </select>
      </label>
      <label
        >{{ copy.token }}
        <input
          id="availability-csv-token"
          ref="tokenInput"
          v-model="token"
          maxlength="256"
          autocomplete="off"
      /></label>
      <WorkforceEntityPicker
        id="availability-csv-reference-picker"
        :key="endpoint"
        :kind="endpoint"
        :label="copy.referenceForToken(copy.endpoints[endpoint])"
        :project="project"
        :library-epoch="libraryEpoch"
        :model-value="endpointId ? [endpointId] : []"
        :disabled="disabled"
        @update:model-value="endpointId = $event[0] ?? ''"
      />
      <label
        >{{ copy.endpointId }}
        <input
          id="availability-csv-endpoint-id"
          v-model="endpointId"
          maxlength="256"
          autocomplete="off"
      /></label>
      <p>{{ copy.endpointHelp }}</p>
      <button id="availability-csv-add-alias" type="button" @click="add">
        {{ copy.addMapping }}
      </button>
      <p v-if="error" role="alert">{{ error }}</p>
    </fieldset>
    <p v-if="mappings.length === 0">{{ copy.noMappings }}</p>
    <ul>
      <li
        v-for="item in mappings.slice(page * 20, (page + 1) * 20)"
        :key="`${item.endpoint}:${item.token}`"
      >
        {{ copy.endpoints[item.endpoint] }}
        <code class="break-all">{{ JSON.stringify(item.token) }}</code> →
        <code class="break-all">{{ item.id }}</code>
        <button
          type="button"
          :disabled="disabled"
          :aria-label="
            copy.removeMappingLabel(copy.endpoints[item.endpoint], JSON.stringify(item.token))
          "
          @click="remove(item.endpoint, item.token)"
        >
          {{ copy.removeMapping }}
        </button>
      </li>
    </ul>
    <nav v-if="mappings.length > 20" :aria-label="copy.mappingPages">
      <button type="button" :disabled="page === 0" @click="page -= 1">
        {{ copy.previousMappings }}
      </button>
      <button type="button" :disabled="(page + 1) * 20 >= mappings.length" @click="page += 1">
        {{ copy.nextMappings }}
      </button>
    </nav>
  </section>
</template>
