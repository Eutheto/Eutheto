<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type { ProjectSummary } from "../../project-home";
import { messages } from "../../messages";
import WorkforceEntityPicker from "./WorkforceEntityPicker.vue";

const props = defineProps<{
  project: ProjectSummary;
  libraryEpoch: number;
  modelValue: Readonly<Record<string, string>>;
  disabled: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: Readonly<Record<string, string>>] }>();
const copy = messages.shiftCsv;
const endpoint = ref<"assignmentType" | "location" | "qualification">("assignmentType");
const token = ref("");
const endpointId = ref("");
const error = ref("");
const page = ref(0);
const tokenInput = ref<HTMLInputElement>();
const mappings = computed(() => Object.entries(props.modelValue));
function add(): void {
  if (props.disabled) return;
  if (
    !token.value ||
    new TextEncoder().encode(token.value).length > 256 ||
    /\p{Cc}/u.test(token.value)
  ) {
    error.value = copy.invalidToken;
    return;
  }
  if (!endpointId.value || endpointId.value.length > 256 || /\p{Cc}/u.test(endpointId.value)) {
    error.value = copy.invalidEndpoint;
    return;
  }
  if (Object.hasOwn(props.modelValue, token.value) || mappings.value.length >= 10_000) {
    error.value = copy.duplicateMapping;
    return;
  }
  // Kind and typed-reference meaning are validated by native review, never inferred from labels.
  emit("update:modelValue", { ...props.modelValue, [token.value]: endpointId.value });
  token.value = "";
  endpointId.value = "";
  error.value = "";
  tokenInput.value?.focus();
}
async function remove(token: string): Promise<void> {
  if (props.disabled) return;
  const origin = document.activeElement;
  const values = { ...props.modelValue };
  Reflect.deleteProperty(values, token);
  emit("update:modelValue", values);
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
  <section class="field-stack" aria-labelledby="shift-csv-token-mappings">
    <h4 id="shift-csv-token-mappings">{{ copy.mappings }}</h4>
    <p>{{ copy.mappingsHelp }}</p>
    <fieldset :disabled="disabled" class="field-stack">
      <legend>{{ copy.addMappingLegend }}</legend>
      <label
        >{{ copy.endpoint }}
        <select id="shift-csv-mapping-endpoint" v-model="endpoint">
          <option value="assignmentType">{{ copy.endpoints.assignmentType }}</option>
          <option value="location">{{ copy.endpoints.location }}</option>
          <option value="qualification">{{ copy.endpoints.qualification }}</option>
        </select>
      </label>
      <label
        >{{ copy.token
        }}<input
          id="shift-csv-token"
          ref="tokenInput"
          v-model="token"
          maxlength="256"
          autocomplete="off"
      /></label>
      <WorkforceEntityPicker
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
        >{{ copy.endpointId
        }}<input id="shift-csv-endpoint-id" v-model="endpointId" maxlength="256" autocomplete="off"
      /></label>
      <button id="shift-csv-add-alias" type="button" @click="add">{{ copy.addMapping }}</button>
      <p v-if="error" role="alert">{{ error }}</p>
    </fieldset>
    <p v-if="mappings.length === 0">{{ copy.noMappings }}</p>
    <ul>
      <li v-for="[alias, id] in mappings.slice(page * 20, (page + 1) * 20)" :key="alias">
        <code class="break-all">{{ JSON.stringify(alias) }}</code> →
        <code class="break-all">{{ id }}</code>
        <button
          type="button"
          :disabled="disabled"
          :aria-label="copy.removeMappingLabel(JSON.stringify(alias))"
          @click="remove(alias)"
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
