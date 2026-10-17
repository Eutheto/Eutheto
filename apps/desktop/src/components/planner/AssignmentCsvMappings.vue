<script setup lang="ts">
import { computed, onScopeDispose, ref, shallowRef, watch } from "vue";
import { getScenarioView, SetupOperationScope } from "../../api/generated";
import type {
  WorkforceSetupTimedShiftContinuation,
  WorkforceSetupWorkPage,
} from "../../api/generated-domain-pack-contracts";
import { safeMessage, type ProjectSummary } from "../../project-home";
import WorkforceEntityPicker from "./WorkforceEntityPicker.vue";

const props = defineProps<{
  project: ProjectSummary;
  libraryEpoch: number;
  personMappings: Readonly<Record<string, string>>;
  shiftMappings: Readonly<Record<string, string>>;
  disabled: boolean;
}>();
const emit = defineEmits<{
  "update:personMappings": [value: Readonly<Record<string, string>>];
  "update:shiftMappings": [value: Readonly<Record<string, string>>];
}>();
const endpoint = ref<"person" | "shift">("person");
const token = ref("");
const endpointId = ref("");
const error = ref("");
const page = ref(0);
const startDate = ref("");
const endDate = ref("");
const work = shallowRef<WorkforceSetupWorkPage | null>(null);
const cursors = shallowRef<readonly (WorkforceSetupTimedShiftContinuation | null)[]>([null]);
const workPage = ref(0);
const loading = ref(false);
const workError = ref("");
let scope: SetupOperationScope | null = null;
let generation = 0;
const mappings = computed(() => [
  ...Object.entries(props.personMappings).map(([value, id]) => ({
    endpoint: "person" as const,
    token: value,
    id,
  })),
  ...Object.entries(props.shiftMappings).map(([value, id]) => ({
    endpoint: "shift" as const,
    token: value,
    id,
  })),
]);
function publish(kind: "person" | "shift", values: Readonly<Record<string, string>>): void {
  if (kind === "person") emit("update:personMappings", values);
  else emit("update:shiftMappings", values);
}
function add(): void {
  if (props.disabled) return;
  const values = endpoint.value === "person" ? props.personMappings : props.shiftMappings;
  if (
    !token.value ||
    new TextEncoder().encode(token.value).length > 256 ||
    /\p{Cc}/u.test(token.value)
  ) {
    error.value =
      "Enter an exact, nonempty token of at most 256 UTF-8 bytes without control characters.";
    return;
  }
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(endpointId.value)) {
    error.value = "Choose a native endpoint or enter its exact canonical identity.";
    return;
  }
  if (Object.hasOwn(values, token.value) || Object.keys(values).length >= 10_000) {
    error.value =
      "Remove the existing mapping for this token first. Each endpoint permits at most 10,000 mappings.";
    return;
  }
  publish(endpoint.value, { ...values, [token.value]: endpointId.value });
  token.value = "";
  endpointId.value = "";
  error.value = "";
}
function remove(kind: "person" | "shift", value: string): void {
  if (props.disabled) return;
  const values = { ...(kind === "person" ? props.personMappings : props.shiftMappings) };
  Reflect.deleteProperty(values, value);
  publish(kind, values);
  page.value = Math.min(page.value, Math.max(0, Math.ceil((mappings.value.length - 1) / 20) - 1));
}
function clearWork(): void {
  generation += 1;
  scope?.dispose();
  scope = null;
  work.value = null;
  loading.value = false;
  cursors.value = [null];
  workPage.value = 0;
  workError.value = "";
}
async function readWork(index = 0): Promise<void> {
  if (props.disabled || loading.value || !startDate.value || !endDate.value) return;
  const captured = ++generation;
  const scenarioId = props.project.scenarioId,
    revision = props.project.revision,
    epoch = props.libraryEpoch;
  const owned = new SetupOperationScope(scenarioId, revision);
  scope = owned;
  loading.value = true;
  workError.value = "";
  try {
    const response = await getScenarioView(owned, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId: "official.workforce.setup.work_window",
        parameters: {
          dates: { startDate: startDate.value, endDateExclusive: endDate.value },
          limit: 50,
        },
        continuation: cursors.value[index] ?? null,
      },
    }).result;
    if (
      captured !== generation ||
      props.project.scenarioId !== scenarioId ||
      props.project.revision !== revision ||
      props.libraryEpoch !== epoch
    )
      return;
    if (
      response.currentRevision !== revision ||
      response.result.scenarioId !== scenarioId ||
      response.result.revision !== revision
    )
      throw new Error("The native work source changed. Read it again.");
    work.value = response.result.view.data.result.data;
    workPage.value = index;
    if (index === 0) cursors.value = [null];
    if (work.value.continuation)
      cursors.value = [...cursors.value.slice(0, index + 1), work.value.continuation];
  } catch (failure) {
    if (captured === generation) workError.value = safeMessage(failure);
  } finally {
    owned.dispose();
    if (scope === owned) scope = null;
    if (captured === generation) loading.value = false;
  }
}
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.libraryEpoch,
    () => props.disabled,
    startDate,
    endDate,
  ],
  clearWork,
  { flush: "sync" },
);
watch(endpoint, () => {
  endpointId.value = "";
  error.value = "";
});
onScopeDispose(clearWork);
</script>

<template>
  <section class="field-stack" aria-labelledby="assignment-token-mappings">
    <h4 id="assignment-token-mappings">Exact token mappings</h4>
    <p>
      Canonical endpoint IDs need no mapping. Other tokens must match exactly, including spaces and
      case. A person token and a shift token have separate meanings. Names are never matched
      automatically, and a mapping cannot shadow an endpoint ID.
    </p>
    <fieldset :disabled="disabled" class="field-stack">
      <legend>Add one explicit endpoint mapping</legend>
      <label
        >Endpoint
        <select v-model="endpoint">
          <option value="person">Person</option>
          <option value="shift">Shift</option>
        </select></label
      >
      <label>Exact CSV token <input v-model="token" maxlength="256" autocomplete="off" /></label>
      <WorkforceEntityPicker
        v-if="endpoint === 'person'"
        kind="person"
        label="Person for token"
        :project="project"
        :library-epoch="libraryEpoch"
        :model-value="endpointId ? [endpointId] : []"
        :disabled="disabled"
        @update:model-value="endpointId = $event[0] ?? ''"
      />
      <template v-else>
        <WorkforceEntityPicker
          kind="shiftInstance"
          label="Saved shift for token"
          :project="project"
          :library-epoch="libraryEpoch"
          :model-value="endpointId ? [endpointId] : []"
          :disabled="disabled"
          @update:model-value="endpointId = $event[0] ?? ''"
        />
        <details>
          <summary>Choose a recurring or timed shift from native work</summary>
          <p>
            This display window does not limit the import. If work cannot currently resolve, an
            exact retained shift identity can still be reviewed below.
          </p>
          <label>Work starts on <input v-model="startDate" type="date" /></label>
          <label>Work ends before <input v-model="endDate" type="date" /></label>
          <button type="button" :disabled="loading || !startDate || !endDate" @click="readWork()">
            Read native work
          </button>
          <p v-if="loading" role="status">Reading native work…</p>
          <p v-if="workError" role="alert">{{ workError }}</p>
          <template v-if="work">
            <p v-if="work.items.length === 0" role="status">No work in this display window.</p>
            <ul>
              <li v-for="shift in work.items" :key="shift.shiftId">
                <button type="button" @click="endpointId = shift.shiftId">
                  Choose {{ shift.assignmentTypeName }} · {{ shift.interval.startsAt.local }} –
                  {{ shift.interval.endsAt.local }} · {{ shift.locationName ?? "No location" }} ·
                  {{ shift.shiftId }}
                </button>
              </li>
            </ul>
            <button
              type="button"
              :disabled="loading || workPage === 0"
              @click="readWork(workPage - 1)"
            >
              Previous work
            </button>
            <button
              type="button"
              :disabled="loading || work.continuation === null"
              @click="readWork(workPage + 1)"
            >
              Next work
            </button>
          </template>
        </details>
      </template>
      <label
        >Exact endpoint identity <input v-model="endpointId" maxlength="36" autocomplete="off"
      /></label>
      <p>
        Rust checks that this identity belongs to the selected endpoint kind. Inactive, dormant and
        out-of-horizon work are not silently dropped.
      </p>
      <button type="button" @click="add">Add token mapping</button>
      <p v-if="error" role="alert">{{ error }}</p>
    </fieldset>
    <p v-if="mappings.length === 0">No token mappings. Only exact endpoint IDs will resolve.</p>
    <ul>
      <li
        v-for="item in mappings.slice(page * 20, (page + 1) * 20)"
        :key="`${item.endpoint}:${item.token}`"
      >
        {{ item.endpoint }} <code class="break-all">{{ JSON.stringify(item.token) }}</code> →
        <code class="break-all">{{ item.id }}</code>
        <button
          type="button"
          :disabled="disabled"
          :aria-label="`Remove ${item.endpoint} mapping for ${JSON.stringify(item.token)}`"
          @click="remove(item.endpoint, item.token)"
        >
          Remove mapping
        </button>
      </li>
    </ul>
    <nav v-if="mappings.length > 20" aria-label="Token mapping pages">
      <button type="button" :disabled="page === 0" @click="page -= 1">Previous mappings</button
      ><button type="button" :disabled="(page + 1) * 20 >= mappings.length" @click="page += 1">
        Next mappings
      </button>
    </nav>
  </section>
</template>
