<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, useId, watch } from "vue";
import { getScenarioEntity, newUuidV7, SetupOperationScope } from "../api/generated";
import type {
  WorkforceCalendar,
  WorkforceWorkloadBucket,
} from "../api/generated-domain-pack-contracts";
import {
  grantDraft,
  personFieldErrors,
  type GrantDraft,
  type PersonFieldErrors,
  type PersonFieldsDraft,
} from "../person-fields";
import { safeMessage, type ProjectSummary } from "../project-home";
import { messages } from "../messages";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";
import { SELECTION_PAGE_SIZE } from "./planner/entity-picker";

const props = defineProps<{
  readonly modelValue: PersonFieldsDraft;
  readonly project: ProjectSummary;
  readonly libraryEpoch: number;
  readonly locale?: string;
  readonly disabled?: boolean;
  readonly readOnly?: boolean;
  readonly showErrors?: boolean;
  readonly errors?: PersonFieldErrors;
}>();
const emit = defineEmits<{ "update:modelValue": [value: PersonFieldsDraft] }>();
const copy = messages.personFields;
const prefix = useId();
const grantPage = ref(0);
const tagPage = ref(0);
const targetInput = ref<HTMLInputElement>();
const bucket = shallowRef<WorkforceWorkloadBucket | null>(null);
const calendar = shallowRef<WorkforceCalendar | null>(null);
const bucketError = ref<string | null>(null);
let bucketScope: SetupOperationScope | null = null;
let bucketGeneration = 0;
let current = true;
const locked = computed(() => props.disabled || props.readOnly);
const errors = computed(() => ({
  ...(props.showErrors ? personFieldErrors(props.modelValue) : {}),
  ...props.errors,
}));
const pickerContext = computed(() => ({
  project: props.project,
  libraryEpoch: props.libraryEpoch,
  disabled: props.disabled,
  readOnly: props.readOnly,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
const grants = computed(() =>
  props.modelValue.qualificationGrants.slice(
    grantPage.value * SELECTION_PAGE_SIZE,
    (grantPage.value + 1) * SELECTION_PAGE_SIZE,
  ),
);
const tags = computed(() =>
  props.modelValue.tags.slice(
    tagPage.value * SELECTION_PAGE_SIZE,
    (tagPage.value + 1) * SELECTION_PAGE_SIZE,
  ),
);
const unit = computed(() => {
  switch (bucket.value?.measurement) {
    case "assignmentCount":
      return copy.assignments;
    case "elapsedMinutes":
      return copy.elapsedMinutes;
    case "scheduledMinutes":
      return copy.scheduledMinutes;
    default:
      return copy.unknownUnit;
  }
});
const calendarDescription = computed(() => {
  const period = calendar.value?.period;
  if (!period) return copy.calendarPending;
  switch (period.kind) {
    case "day":
      return copy.dayPeriod(period.startTime);
    case "week":
      return copy.weekPeriod(period.anchorDate, period.startTime);
    case "payPeriod":
      return copy.payPeriod(period.lengthDays, period.anchorDate, props.locale);
    case "custom":
      return copy.customPeriod(period.intervals.length, props.locale);
  }
});
function set<K extends keyof PersonFieldsDraft>(field: K, value: PersonFieldsDraft[K]): void {
  if (!locked.value) emit("update:modelValue", { ...props.modelValue, [field]: value });
}
function text(event: Event): string {
  return (event.target as HTMLInputElement).value;
}
function checked(event: Event): boolean {
  return (event.target as HTMLInputElement).checked;
}
function membership(event: Event): void {
  const value = text(event);
  if (value === "reportingDate" || value === "startInstant" || value === "intersection")
    set("targetMembership", value);
}
function changeGrant(
  key: string,
  field: "qualificationId" | "effectiveFrom" | "expiresAt",
  value: string,
): void {
  set(
    "qualificationGrants",
    props.modelValue.qualificationGrants.map((grant) =>
      grant.key === key ? { ...grant, [field]: value } : grant,
    ),
  );
}
async function focus(id: string): Promise<void> {
  await nextTick();
  if (current) document.getElementById(id)?.focus();
}
async function addGrant(): Promise<void> {
  if (locked.value || props.modelValue.qualificationGrants.length >= 10_000) return;
  const row = grantDraft();
  grantPage.value = Math.floor(props.modelValue.qualificationGrants.length / SELECTION_PAGE_SIZE);
  set("qualificationGrants", [...props.modelValue.qualificationGrants, row]);
  await focus(`${prefix}-grant-${row.key}`);
}
async function removeGrant(row: GrantDraft): Promise<void> {
  set(
    "qualificationGrants",
    props.modelValue.qualificationGrants.filter((item) => item.key !== row.key),
  );
  await focus(`${prefix}-grants-heading`);
}
async function addTag(): Promise<void> {
  if (locked.value || props.modelValue.tags.length >= 10_000) return;
  const row = { key: newUuidV7(), text: "" };
  tagPage.value = Math.floor(props.modelValue.tags.length / SELECTION_PAGE_SIZE);
  set("tags", [...props.modelValue.tags, row]);
  await focus(`${prefix}-tag-${row.key}`);
}
async function removeTag(key: string): Promise<void> {
  set(
    "tags",
    props.modelValue.tags.filter((item) => item.key !== key),
  );
  await focus(`${prefix}-tags-heading`);
}
async function changePage(kind: "grants" | "tags", step: number): Promise<void> {
  const page = kind === "grants" ? grantPage : tagPage;
  page.value += step;
  await focus(`${prefix}-${kind}-heading`);
}
watch(
  () => props.modelValue.qualificationGrants.length,
  (length) => {
    grantPage.value = Math.min(
      grantPage.value,
      Math.max(0, Math.ceil(length / SELECTION_PAGE_SIZE) - 1),
    );
  },
);
watch(
  () => props.modelValue.tags.length,
  (length) => {
    tagPage.value = Math.min(
      tagPage.value,
      Math.max(0, Math.ceil(length / SELECTION_PAGE_SIZE) - 1),
    );
  },
);
async function readBucket(): Promise<void> {
  const generation = ++bucketGeneration;
  bucketScope?.dispose();
  bucketScope = null;
  bucket.value = null;
  calendar.value = null;
  bucketError.value = null;
  if (!props.modelValue.targetEnabled) return;
  const id = props.modelValue.targetBucketId;
  const calendarId = props.modelValue.targetCalendarId;
  if (!id && !calendarId) return;
  let owned: SetupOperationScope | null = null;
  try {
    owned = new SetupOperationScope(props.project.scenarioId, props.project.revision);
    bucketScope = owned;
    const [bucketResponse, calendarResponse] = await Promise.all([
      id ? getScenarioEntity(owned, { kind: "workloadBucket", entityId: id }).result : null,
      calendarId
        ? getScenarioEntity(owned, { kind: "calendar", entityId: calendarId }).result
        : null,
    ]);
    if (!current || generation !== bucketGeneration) return;
    const data = bucketResponse?.result.view.data.result.data;
    const calendarData = calendarResponse?.result.view.data.result.data;
    if (data && (data.kind !== "workloadBucket" || data.id !== id))
      throw new Error("The native workload bucket did not match the requested identity.");
    if (calendarData && (calendarData.kind !== "calendar" || calendarData.id !== calendarId))
      throw new Error("The native calendar did not match the requested identity.");
    bucket.value = data ?? null;
    calendar.value = calendarData ?? null;
  } catch (error) {
    if (current && generation === bucketGeneration) bucketError.value = safeMessage(error);
  } finally {
    owned?.dispose();
    if (generation === bucketGeneration) bucketScope = null;
  }
}
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.libraryEpoch,
    () => props.modelValue.targetEnabled,
    () => props.modelValue.targetBucketId,
    () => props.modelValue.targetCalendarId,
  ],
  () => {
    void readBucket();
  },
  { immediate: true, flush: "sync" },
);
watch(
  [targetInput, bucket, calendar, () => props.modelValue.targetEnabled],
  () => {
    targetInput.value?.setCustomValidity(
      props.modelValue.targetEnabled && (!bucket.value || !calendar.value)
        ? copy.metadataRequired
        : "",
    );
  },
  { flush: "post" },
);
onScopeDispose(() => {
  current = false;
  bucketGeneration++;
  bucketScope?.dispose();
});
</script>

<template>
  <div class="field-stack">
    <fieldset :disabled="disabled" class="field-stack">
      <legend>{{ copy.activeDates }}</legend>
      <label
        ><input
          type="checkbox"
          :checked="modelValue.activeDatesEnabled"
          :disabled="locked"
          @change="set('activeDatesEnabled', checked($event))"
        />
        {{ copy.limitActiveDates }}</label
      >
      <p class="field-help">
        {{ copy.dateHelp }}
      </p>
      <div v-if="modelValue.activeDatesEnabled" class="form-columns">
        <div
          v-for="field in [
            { key: 'startDate', label: copy.startDate },
            { key: 'endDateExclusive', label: copy.endDate },
          ] as const"
          :key="field.key"
          class="field-stack"
        >
          <label :for="`${prefix}-${field.key}`">{{ field.label }}</label>
          <input
            :id="`${prefix}-${field.key}`"
            type="text"
            placeholder="YYYY-MM-DD"
            maxlength="64"
            required
            :readonly="readOnly"
            :value="modelValue[field.key]"
            @input="set(field.key, text($event))"
          />
        </div>
      </div>
    </fieldset>

    <section class="field-stack" :aria-labelledby="`${prefix}-grants-heading`">
      <h3 :id="`${prefix}-grants-heading`" tabindex="-1">{{ copy.grants }}</h3>
      <p class="field-help">
        {{ copy.grantHelp }}
      </p>
      <p v-if="errors.qualificationGrants" class="text-danger" role="alert">
        {{ errors.qualificationGrants }}
      </p>
      <fieldset
        v-for="(row, index) in grants"
        :key="row.key"
        :disabled="disabled"
        class="field-stack"
      >
        <legend>{{ copy.grantNumber(grantPage * SELECTION_PAGE_SIZE + index + 1, locale) }}</legend>
        <WorkforceEntityPicker
          v-bind="pickerContext"
          :id="`${prefix}-grant-${row.key}`"
          kind="qualification"
          :label="copy.qualification"
          required
          :model-value="row.qualificationId ? [row.qualificationId] : []"
          @update:model-value="changeGrant(row.key, 'qualificationId', $event[0] ?? '')"
        />
        <div class="form-columns">
          <div
            v-for="field in [
              { key: 'effectiveFrom', label: copy.effectiveFrom },
              { key: 'expiresAt', label: copy.expiresAt },
            ] as const"
            :key="field.key"
            class="field-stack"
          >
            <label :for="`${prefix}-${row.key}-${field.key}`">{{ field.label }}</label>
            <input
              :id="`${prefix}-${row.key}-${field.key}`"
              type="text"
              maxlength="64"
              :readonly="readOnly"
              :value="row[field.key]"
              @input="changeGrant(row.key, field.key, text($event))"
            />
          </div>
        </div>
        <button
          type="button"
          class="button-secondary"
          :disabled="locked"
          :aria-label="copy.removeGrantNumber(grantPage * SELECTION_PAGE_SIZE + index + 1, locale)"
          @click="removeGrant(row)"
        >
          {{ copy.removeGrant }}
        </button>
      </fieldset>
      <p role="status">
        {{
          copy.grantPage(
            modelValue.qualificationGrants.length,
            grantPage + 1,
            Math.max(1, Math.ceil(modelValue.qualificationGrants.length / SELECTION_PAGE_SIZE)),
            locale,
          )
        }}
      </p>
      <div class="action-row">
        <button
          type="button"
          class="button-secondary"
          :disabled="grantPage === 0"
          @click="changePage('grants', -1)"
        >
          {{ copy.previousGrants }}
        </button>
        <button
          type="button"
          class="button-secondary"
          :disabled="(grantPage + 1) * SELECTION_PAGE_SIZE >= modelValue.qualificationGrants.length"
          @click="changePage('grants', 1)"
        >
          {{ copy.nextGrants }}
        </button>
        <button
          type="button"
          class="button-secondary"
          :disabled="locked || modelValue.qualificationGrants.length >= 10_000"
          @click="addGrant"
        >
          {{ copy.addGrant }}
        </button>
      </div>
    </section>

    <WorkforceEntityPicker
      v-bind="pickerContext"
      kind="assignmentType"
      :label="copy.eligibleTypes"
      multiple
      :model-value="modelValue.eligibleAssignmentTypeIds"
      :error="errors.eligibleAssignmentTypeIds"
      @update:model-value="set('eligibleAssignmentTypeIds', $event)"
    />
    <WorkforceEntityPicker
      v-bind="pickerContext"
      kind="team"
      :label="copy.teams"
      multiple
      :model-value="modelValue.teamIds"
      :error="errors.teamIds"
      @update:model-value="set('teamIds', $event)"
    />
    <WorkforceEntityPicker
      v-bind="pickerContext"
      kind="location"
      :label="copy.homeLocation"
      :model-value="modelValue.homeLocationId ? [modelValue.homeLocationId] : []"
      :error="errors.homeLocationId"
      @update:model-value="set('homeLocationId', $event[0] ?? '')"
    />

    <fieldset :disabled="disabled" class="field-stack">
      <legend>{{ copy.weight }}</legend>
      <p :id="`${prefix}-weight-help`" class="field-help">
        {{ copy.weightHelp }}
      </p>
      <div class="form-columns">
        <div
          v-for="field in [
            { key: 'weightNumerator', label: copy.numerator },
            { key: 'weightDenominator', label: copy.denominator },
          ] as const"
          :key="field.key"
          class="field-stack"
        >
          <label :for="`${prefix}-${field.key}`">{{ field.label }}</label>
          <input
            :id="`${prefix}-${field.key}`"
            type="text"
            inputmode="numeric"
            maxlength="32"
            required
            :readonly="readOnly"
            :value="modelValue[field.key]"
            :aria-invalid="Boolean(errors[field.key]) || undefined"
            :aria-describedby="`${prefix}-weight-help ${prefix}-${field.key}-error`"
            @input="set(field.key, text($event))"
          />
          <p :id="`${prefix}-${field.key}-error`" class="text-danger" role="status">
            {{ errors[field.key] }}
          </p>
        </div>
      </div>
    </fieldset>

    <fieldset :disabled="disabled" class="field-stack">
      <legend>{{ copy.target }}</legend>
      <label
        ><input
          type="checkbox"
          :checked="modelValue.targetEnabled"
          :disabled="locked"
          @change="set('targetEnabled', checked($event))"
        />
        {{ copy.enableTarget }}</label
      >
      <p class="field-help">
        {{ copy.targetHelp }}
      </p>
      <template v-if="modelValue.targetEnabled">
        <WorkforceEntityPicker
          v-bind="pickerContext"
          kind="workloadBucket"
          :label="copy.bucket"
          required
          :model-value="modelValue.targetBucketId ? [modelValue.targetBucketId] : []"
          @update:model-value="set('targetBucketId', $event[0] ?? '')"
        />
        <WorkforceEntityPicker
          v-bind="pickerContext"
          kind="calendar"
          :label="copy.calendar"
          required
          :model-value="modelValue.targetCalendarId ? [modelValue.targetCalendarId] : []"
          @update:model-value="set('targetCalendarId', $event[0] ?? '')"
        />
        <label :for="`${prefix}-membership`">{{ copy.membership }}</label>
        <select
          :id="`${prefix}-membership`"
          :value="modelValue.targetMembership"
          :disabled="locked"
          @change="membership"
        >
          <option value="reportingDate">{{ copy.reportingDate }}</option>
          <option value="startInstant">{{ copy.startInstant }}</option>
          <option value="intersection">{{ copy.intersection }}</option>
        </select>
        <label :for="`${prefix}-target`">{{ copy.targetUnit(unit) }}</label>
        <input
          :id="`${prefix}-target`"
          ref="targetInput"
          type="text"
          inputmode="numeric"
          maxlength="32"
          required
          :readonly="readOnly"
          :value="modelValue.target"
          :aria-invalid="Boolean(errors.target) || undefined"
          :aria-describedby="`${prefix}-target-error ${prefix}-bucket-status`"
          @input="set('target', text($event))"
        />
        <p :id="`${prefix}-target-error`" class="text-danger" role="status">{{ errors.target }}</p>
        <p :id="`${prefix}-bucket-status`" role="status">
          {{
            bucketError ??
            (bucket ? copy.unitStatus(unit, bucket.overlappingContribution) : copy.bucketPending)
          }}
        </p>
        <p class="field-help">{{ calendar?.name }} {{ calendarDescription }}</p>
        <button
          v-if="bucketError"
          type="button"
          class="button-secondary"
          :disabled="disabled"
          @click="readBucket"
        >
          {{ copy.retryMetadata }}
        </button>
      </template>
    </fieldset>

    <section class="field-stack" :aria-labelledby="`${prefix}-tags-heading`">
      <h3 :id="`${prefix}-tags-heading`" tabindex="-1">{{ copy.tags }}</h3>
      <p v-if="errors.tags" class="text-danger" role="alert">{{ errors.tags }}</p>
      <div v-for="(row, index) in tags" :key="row.key" class="field-stack">
        <label :for="`${prefix}-tag-${row.key}`">{{
          copy.tagNumber(tagPage * SELECTION_PAGE_SIZE + index + 1, locale)
        }}</label>
        <input
          :id="`${prefix}-tag-${row.key}`"
          type="text"
          maxlength="256"
          required
          :disabled="disabled"
          :readonly="readOnly"
          :value="row.text"
          @input="
            set(
              'tags',
              modelValue.tags.map((tag) =>
                tag.key === row.key ? { ...tag, text: text($event) } : tag,
              ),
            )
          "
        />
        <button
          type="button"
          class="button-secondary"
          :disabled="locked"
          :aria-label="copy.removeTagNumber(tagPage * SELECTION_PAGE_SIZE + index + 1, locale)"
          @click="removeTag(row.key)"
        >
          {{ copy.removeTag }}
        </button>
      </div>
      <p role="status">
        {{
          copy.tagPage(
            modelValue.tags.length,
            tagPage + 1,
            Math.max(1, Math.ceil(modelValue.tags.length / SELECTION_PAGE_SIZE)),
            locale,
          )
        }}
      </p>
      <div class="action-row">
        <button
          type="button"
          class="button-secondary"
          :disabled="tagPage === 0"
          @click="changePage('tags', -1)"
        >
          {{ copy.previousTags }}
        </button>
        <button
          type="button"
          class="button-secondary"
          :disabled="(tagPage + 1) * SELECTION_PAGE_SIZE >= modelValue.tags.length"
          @click="changePage('tags', 1)"
        >
          {{ copy.nextTags }}
        </button>
        <button
          type="button"
          class="button-secondary"
          :disabled="locked || modelValue.tags.length >= 10_000"
          @click="addTag"
        >
          {{ copy.addTag }}
        </button>
      </div>
    </section>

    <fieldset :disabled="disabled" class="field-stack">
      <legend>{{ copy.display }}</legend>
      <label
        ><input
          type="checkbox"
          :checked="modelValue.displayEnabled"
          :disabled="locked"
          @change="set('displayEnabled', checked($event))"
        />
        {{ copy.enableDisplay }}</label
      >
      <div v-if="modelValue.displayEnabled" class="form-columns">
        <div class="field-stack">
          <label :for="`${prefix}-color`">{{ copy.color }}</label>
          <input
            :id="`${prefix}-color`"
            type="text"
            maxlength="7"
            :readonly="readOnly"
            :value="modelValue.color"
            @input="set('color', text($event))"
          />
        </div>
        <div class="field-stack">
          <label :for="`${prefix}-initials`">{{ copy.initials }}</label>
          <input
            :id="`${prefix}-initials`"
            type="text"
            maxlength="16"
            :readonly="readOnly"
            :value="modelValue.avatarInitials"
            @input="set('avatarInitials', text($event))"
          />
        </div>
      </div>
    </fieldset>
  </div>
</template>
