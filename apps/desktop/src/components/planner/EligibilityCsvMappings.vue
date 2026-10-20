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
  disabled: boolean;
}>();
const emit = defineEmits<{
  "update:personMappings": [value: Readonly<Record<string, string>>];
  "update:assignmentTypeMappings": [value: Readonly<Record<string, string>>];
}>();
const copy = messages.eligibilityCsv;
const endpoint = ref<"person" | "assignmentType">("person");
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
]);
function publish(
  kind: "person" | "assignmentType",
  values: Readonly<Record<string, string>>,
): void {
  if (kind === "person") emit("update:personMappings", values);
  else emit("update:assignmentTypeMappings", values);
}
function add(): void {
  if (props.disabled) return;
  const values = endpoint.value === "person" ? props.personMappings : props.assignmentTypeMappings;
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
async function remove(kind: "person" | "assignmentType", token: string): Promise<void> {
  if (props.disabled) return;
  const origin = document.activeElement;
  const values = { ...(kind === "person" ? props.personMappings : props.assignmentTypeMappings) };
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
  <section class="field-stack" aria-labelledby="eligibility-token-mappings">
    <h4 id="eligibility-token-mappings">{{ copy.mappings }}</h4>
    <p>{{ copy.mappingsHelp }}</p>
    <fieldset :disabled="disabled" class="field-stack">
      <legend>{{ copy.addMappingLegend }}</legend>
      <label
        >{{ copy.endpoint }}
        <select id="eligibility-csv-mapping-endpoint" v-model="endpoint">
          <option value="person">{{ copy.person }}</option>
          <option value="assignmentType">{{ copy.assignmentType }}</option>
        </select>
      </label>
      <label
        >{{ copy.token }}
        <input
          id="eligibility-csv-token"
          ref="tokenInput"
          v-model="token"
          maxlength="256"
          autocomplete="off"
      /></label>
      <WorkforceEntityPicker
        :key="endpoint"
        :kind="endpoint"
        :label="endpoint === 'person' ? copy.personForToken : copy.typeForToken"
        :project="project"
        :library-epoch="libraryEpoch"
        :model-value="endpointId ? [endpointId] : []"
        :disabled="disabled"
        @update:model-value="endpointId = $event[0] ?? ''"
      />
      <label
        >{{ copy.endpointId }}
        <input
          id="eligibility-csv-endpoint-id"
          v-model="endpointId"
          maxlength="256"
          autocomplete="off"
      /></label>
      <p>{{ copy.endpointHelp }}</p>
      <button type="button" @click="add">{{ copy.addMapping }}</button>
      <p v-if="error" role="alert">{{ error }}</p>
    </fieldset>
    <p v-if="mappings.length === 0">{{ copy.noMappings }}</p>
    <ul>
      <li
        v-for="item in mappings.slice(page * 20, (page + 1) * 20)"
        :key="`${item.endpoint}:${item.token}`"
      >
        {{ item.endpoint === "person" ? copy.person : copy.assignmentType }}
        <code class="break-all">{{ JSON.stringify(item.token) }}</code> →
        <code class="break-all">{{ item.id }}</code>
        <button
          type="button"
          :disabled="disabled"
          :aria-label="
            copy.removeMappingLabel(
              item.endpoint === 'person' ? copy.person : copy.assignmentType,
              JSON.stringify(item.token),
            )
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
