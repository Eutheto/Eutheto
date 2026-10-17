<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, watch } from "vue";
import { RouterLink } from "vue-router";
import type { ProjectHomeController, ProjectSummary } from "../project-home";
import type {
  WorkforceConsecutiveWorkMode,
  WorkforceDateRange,
  WorkforceSetupRuleCatalogEntry,
  WorkforceSetupRuleReference,
  WorkforceSetupScopePart,
  WorkforceTimeWindow,
  WorkforceWeekday,
  WorkforceWorkloadPolicy,
} from "../api/generated-domain-pack-contracts";
import { formatNumber, messages } from "../messages";
import {
  parseMaximumAssignmentCountDraft,
  parseMaximumHoursDurationDraft,
  parseMaximumHoursMaximumMinutesDraft,
  type QualificationMinimumDraft,
} from "../rule-draft";
import type { ScopeFieldName } from "./planner/scope-field";
import { useRuleSetup } from "../rule-setup";
import { useValidationRoute } from "../validation-route";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import RuleScopeBuilder from "./planner/RuleScopeBuilder.vue";
import DurationField from "./planner/DurationField.vue";
import ShiftScopePicker from "./ShiftScopePicker.vue";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";

const props = defineProps<{
  readonly home: ProjectHomeController;
  readonly project: ProjectSummary;
  readonly locale?: string;
}>();
const vm = useRuleSetup(props.home, () => props.project);
const { state, review, busy, dirty, stale, canReplace, canApply, conflictsCurrent } = vm;
const copy = messages.ruleSetup;
const recordsCopy = messages.people;
const editorHeading = ref<HTMLElement>();
const listHeading = ref<HTMLElement>();
const reviewHeading = ref<HTMLElement>();
const editorHost = ref<HTMLElement>();
const scopeFields = new Map<WorkforceSetupScopePart, InstanceType<typeof RuleScopeBuilder>>();
const pairKeys = ref<string[]>([]);
const pairPage = ref(0);
const skillMixPage = ref(0);
const warningPage = ref(0);
const fixedShiftDates = ref<WorkforceDateRange>({ startDate: "", endDateExclusive: "" });
let rowSequence = 0;
let focusGeneration = 0;
let alive = true;
const locked = computed(
  () => busy.value || state.editor?.editing !== true || state.editor.rebase !== null,
);
const parts = computed<readonly WorkforceSetupScopePart[]>(() =>
  state.editor?.kind === "minimumRest"
    ? ["main", "minimumRestBefore", "minimumRestAfter"]
    : ["main"],
);
const partLabels: Readonly<Record<WorkforceSetupScopePart, string>> = {
  main: copy.fields.scope,
  minimumRestBefore: copy.fields.beforeScope,
  minimumRestAfter: copy.fields.afterScope,
};
const partPaths = {
  main: "scope",
  minimumRestBefore: "beforeScope",
  minimumRestAfter: "afterScope",
} as const;
const pairOffset = computed(() => pairPage.value * 50);
const pairRows = computed(
  () =>
    state.editor?.raw.compatibleCategoryPairs.slice(pairOffset.value, pairOffset.value + 50) ?? [],
);
const skillMixOffset = computed(() => skillMixPage.value * 50);
const skillMixRows = computed(
  () =>
    state.editor?.raw.qualificationMinimums.slice(
      skillMixOffset.value,
      skillMixOffset.value + 50,
    ) ?? [],
);
const warnings = computed(() =>
  review.state.review === null
    ? []
    : [...review.state.review.warnings.changes, ...review.state.review.warnings.proposed],
);
const catalogEntries = computed(
  () => state.catalog?.[state.classFilter === "required" ? "required" : "preferences"] ?? [],
);
const calendarCount = computed(
  () => state.facts?.entities.find((item) => item.kind === "calendar")?.count ?? 0,
);
const workloadBucketCount = computed(
  () => state.facts?.entities.find((item) => item.kind === "workloadBucket")?.count ?? 0,
);
const baseScheduleCount = computed(
  () => state.facts?.entities.find((item) => item.kind === "baseSchedule")?.count ?? 0,
);
const coverageRequirementCount = computed(
  () => state.facts?.entities.find((item) => item.kind === "coverageRequirement")?.count ?? 0,
);
const workloadPolicyEntries = computed<readonly WorkforceWorkloadPolicy[]>(() =>
  Object.values(state.workloadPolicies.policies).sort((left, right) =>
    left.id.localeCompare(right.id),
  ),
);
function workloadPolicyTargetMode(policy: WorkforceWorkloadPolicy): string {
  switch (policy.targetMode.kind) {
    case "equalShare":
      return "EqualShare";
    case "weightedEqualShare":
      return "Weighted equal share";
    case "personTargets":
      return "Person targets";
    case "explicit":
      return "Explicit targets";
  }
}
function workloadPolicyPeerGroup(policy: WorkforceWorkloadPolicy): string {
  const people =
    policy.peerGroup.people.kind === "all"
      ? "all people"
      : policy.peerGroup.people.kind === "selected"
        ? `${String(policy.peerGroup.people.personIds.length)} selected people`
        : "tag-filtered people";
  const teams =
    policy.peerGroup.teamIds === undefined
      ? ""
      : ` and ${String(policy.peerGroup.teamIds.length)} teams`;
  return `${people}${teams}`;
}
function workloadPolicySentence(policyId: string): string {
  const policy = state.workloadPolicies.policies[policyId];
  return policy === undefined
    ? copy.fields.workloadPolicyInvalid
    : copy.workloadBalanceSentence(
        policy.id,
        workloadPolicyTargetMode(policy),
        workloadPolicyPeerGroup(policy),
      );
}
const weekdays: readonly WorkforceWeekday[] = [
  "monday",
  "tuesday",
  "wednesday",
  "thursday",
  "friday",
  "saturday",
  "sunday",
];
const consecutiveWorkWeekdayOptions: readonly WorkforceWeekday[] = ["saturday", "sunday"];
watch(
  () => [state.editor?.id, state.editor?.raw.compatibleCategoryPairs.length] as const,
  ([id, length], previous) => {
    if (id !== previous?.[0]) {
      pairKeys.value = [];
      pairPage.value = 0;
    }
    pairKeys.value.splice(length ?? 0);
    while (pairKeys.value.length < (length ?? 0))
      pairKeys.value.push(`pair-${String(++rowSequence)}`);
    pairPage.value = Math.min(pairPage.value, Math.max(0, Math.ceil((length ?? 0) / 50) - 1));
  },
  { immediate: true, flush: "sync" },
);
watch(
  () => [state.editor?.id, state.editor?.raw.qualificationMinimums.length] as const,
  ([id, length], previous) => {
    if (id !== previous?.[0]) skillMixPage.value = 0;
    skillMixPage.value = Math.min(
      skillMixPage.value,
      Math.max(0, Math.ceil((length ?? 0) / 50) - 1),
    );
  },
  { immediate: true, flush: "sync" },
);
watch(
  () => [props.project.scenarioId, state.facts?.initialWorkWindow] as const,
  ([, dates]) => {
    if (dates) fixedShiftDates.value = { ...dates };
  },
  { immediate: true },
);
watch(
  () => review.state.review?.snapshot,
  () => {
    warningPage.value = 0;
  },
);
watch(
  [
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
  ],
  () => {
    focusGeneration += 1;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  alive = false;
  focusGeneration += 1;
});

function ownership() {
  const generation = ++focusGeneration;
  const scenarioId = props.project.scenarioId;
  const revision = props.project.revision;
  const epoch = props.home.state.libraryEpoch;
  return () =>
    alive &&
    generation === focusGeneration &&
    props.project.scenarioId === scenarioId &&
    props.project.revision === revision &&
    props.home.state.libraryEpoch === epoch;
}
function kindLabel(kind: string): string {
  return Object.hasOwn(copy.kinds, kind) ? copy.kinds[kind as keyof typeof copy.kinds] : kind;
}
function descriptorLabel(id: string): string {
  return (
    [...(state.catalog?.required ?? []), ...(state.catalog?.preferences ?? [])].find(
      (entry) => entry.descriptor.id === id,
    )?.descriptor.title.defaultText ?? id
  );
}
function descriptorDescription(id: string): string {
  if (id === "official.workforce.rule.maximum-worked-weekends")
    return copy.fields.maximumWorkedWeekendsHelp;
  if (id === "official.workforce.preference.weekend-target")
    return copy.fields.weekendTargetCountHelp;
  return (
    [...(state.catalog?.required ?? []), ...(state.catalog?.preferences ?? [])].find(
      (entry) => entry.descriptor.id === id,
    )?.descriptor.description.defaultText ?? id
  );
}
function canCreateEntry(entry: WorkforceSetupRuleCatalogEntry): boolean {
  if (!canReplace.value || state.facts === null) return false;
  if (
    (entry.descriptor.id === "official.workforce.preference.adjacency" ||
      entry.descriptor.id === "official.workforce.preference.base-stability") &&
    baseScheduleCount.value === 0
  )
    return false;
  if (
    entry.descriptor.id === "official.workforce.preference.preferred-coverage" &&
    coverageRequirementCount.value === 0
  )
    return false;
  return (
    entry.descriptor.id !== "official.workforce.preference.workload-balance" ||
    state.workloadPolicies.status === "ready"
  );
}
function sourceUnavailableMessage(entry: WorkforceSetupRuleCatalogEntry): string | null {
  if (
    (entry.descriptor.id === "official.workforce.preference.adjacency" ||
      entry.descriptor.id === "official.workforce.preference.base-stability") &&
    baseScheduleCount.value === 0
  )
    return copy.fields.baseScheduleUnavailable;
  if (
    entry.descriptor.id === "official.workforce.preference.preferred-coverage" &&
    coverageRequirementCount.value === 0
  )
    return copy.fields.coverageRequirementsUnavailable;
  return null;
}
function nativeValue(value: unknown): string {
  return value === undefined ? recordsCopy.absent : JSON.stringify(value, null, 2);
}
function fieldValue(value: object, field: string): unknown {
  return Object.hasOwn(value, field) ? Reflect.get(value, field) : undefined;
}
function text(event: Event): string {
  return (event.target as HTMLInputElement | HTMLSelectElement).value;
}
function preferenceDirection(): "prefer" | "avoid" {
  return state.editor?.raw.direction === "avoid" ? "avoid" : "prefer";
}
function groupDirection(): "together" | "separate" {
  return state.editor?.raw.direction === "separate" ? "separate" : "together";
}
function updateGroupDirection(value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || state.editor?.kind !== "togetherSeparate") return;
  vm.updateRaw({ ...raw, direction: value === "separate" ? "separate" : "together" });
}
function adjacencyMode(): "before" | "after" | "either" {
  const mode = state.editor?.raw.mode;
  return mode === "before" || mode === "after" ? mode : "either";
}
function updateAdjacencyMode(value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || state.editor?.kind !== "adjacency") return;
  const mode = value === "before" || value === "after" || value === "either" ? value : "either";
  vm.updateRaw({ ...raw, mode });
}
function updatePreferencePriority(value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || state.editor?.class !== "preference") return;
  const priority = (["low", "normal", "high", "veryHigh"] as const).find(
    (candidate) => candidate === value,
  );
  if (priority !== undefined) vm.updateRaw({ ...raw, priority });
}
function updatePreferenceWeight(value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || state.editor?.class !== "preference") return;
  vm.updateRaw({ ...raw, weight: parseMaximumAssignmentCountDraft(value) });
}
function updatePreferenceDirection(value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || state.editor?.class !== "preference") return;
  vm.updateRaw({ ...raw, direction: value === "avoid" ? "avoid" : "prefer" });
}
function timeWindowKind(): "instant" | "weekly" {
  return state.editor?.raw.timeWindow.kind === "instant" ? "instant" : "weekly";
}
function updateTimeWindowKind(value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || state.editor?.kind !== "time") return;
  vm.updateRaw({
    ...raw,
    timeWindow:
      value === "instant"
        ? { kind: "instant", startsAt: "", endsAt: "" }
        : {
            kind: "weekly",
            windows: [
              {
                key: nextTimeWindowKey([]),
                weekdays: ["monday"],
                startTime: "09:00:00",
                endTime: "17:00:00",
                endDayOffset: "0",
              },
            ],
          },
  });
}
function nextTimeWindowKey(windows: readonly { readonly key: string }[]): string {
  const used = new Set(windows.map((window) => window.key));
  let key: string;
  do {
    key = `time-window-${String(++rowSequence)}`;
  } while (used.has(key));
  return key;
}
function updateTimeWindowField(field: "startsAt" | "endsAt", value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || raw.timeWindow.kind !== "instant") return;
  vm.updateRaw({ ...raw, timeWindow: { ...raw.timeWindow, [field]: value } });
}
function timeWindowRowId(key: string, field: string): string {
  return `rule-${state.editor?.id ?? "none"}-${key}-${field}`;
}
function timeWindowWeekdayId(key: string, weekday: WorkforceWeekday): string {
  return timeWindowRowId(key, `weekday-${weekday}`);
}
function updateWeeklyWindow(
  key: string,
  field: "startTime" | "endTime" | "endDayOffset",
  value: string,
): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || raw.timeWindow.kind !== "weekly") return;
  vm.updateRaw({
    ...raw,
    timeWindow: {
      ...raw.timeWindow,
      windows: raw.timeWindow.windows.map((window) =>
        window.key === key ? { ...window, [field]: value } : window,
      ),
    },
  });
}
function toggleWeeklyWeekday(key: string, weekday: WorkforceWeekday, checked: boolean): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || raw.timeWindow.kind !== "weekly") return;
  vm.updateRaw({
    ...raw,
    timeWindow: {
      ...raw.timeWindow,
      windows: raw.timeWindow.windows.map((window) => {
        if (window.key !== key) return window;
        const selected = new Set(window.weekdays);
        if (checked) selected.add(weekday);
        else selected.delete(weekday);
        return {
          ...window,
          weekdays: weekdays.filter((candidate) => selected.has(candidate)),
        };
      }),
    },
  });
}
async function addWeeklyWindow(): Promise<void> {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || raw.timeWindow.kind !== "weekly") return;
  const index = raw.timeWindow.windows.length;
  const key = nextTimeWindowKey(raw.timeWindow.windows);
  vm.updateRaw({
    ...raw,
    timeWindow: {
      ...raw.timeWindow,
      windows: [
        ...raw.timeWindow.windows,
        {
          key,
          weekdays: ["monday"],
          startTime: "09:00:00",
          endTime: "17:00:00",
          endDayOffset: "0",
        },
      ],
    },
  });
  await focusField(["timeWindow", "windows", String(index), "startTime"], ownership());
}
async function removeWeeklyWindow(index: number): Promise<void> {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || raw.timeWindow.kind !== "weekly") return;
  vm.updateRaw({
    ...raw,
    timeWindow: {
      ...raw.timeWindow,
      windows: raw.timeWindow.windows.filter((_, at) => at !== index),
    },
  });
  const next = Math.min(index, raw.timeWindow.windows.length - 2);
  await focusField(
    next < 0 ? ["timeWindow"] : ["timeWindow", "windows", String(next), "startTime"],
    ownership(),
  );
}
function updateMaximumHoursWindowKind(kind: string): void {
  const raw = state.editor?.raw;
  if (
    raw === undefined ||
    locked.value ||
    state.editor?.kind !== "maximumHours" ||
    (kind !== "calendar" && kind !== "rolling")
  )
    return;
  const membership = raw.window.membership;
  vm.updateRaw({
    ...raw,
    window:
      kind === "calendar"
        ? { kind: "calendar", calendarId: "", membership }
        : { kind: "rolling", durationMinutes: parseMaximumHoursDurationDraft(""), membership },
  });
}
function updateMaximumHoursMembership(value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || state.editor?.kind !== "maximumHours") return;
  const membership =
    value === "startInstant"
      ? "startInstant"
      : value === "intersection"
        ? "intersection"
        : "reportingDate";
  vm.updateRaw({ ...raw, window: { ...raw.window, membership } });
}
function maximumConsecutiveModeKind(): "workedDays" | "assignments" {
  const mode = state.editor?.raw.mode;
  return typeof mode === "object" && mode.kind === "assignments" ? "assignments" : "workedDays";
}
function maximumConsecutiveBreakMinutesRaw(): string {
  const mode = state.editor?.raw.mode;
  if (typeof mode !== "object" || mode.kind !== "assignments") return "";
  return mode.breakMinutes.raw;
}
function mutualModeKind(): "sameShift" | "overlappingShifts" {
  const mode = state.editor?.raw.mode;
  return mode === "overlappingShifts" ? mode : "sameShift";
}
function updateMaximumConsecutiveMode(value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || state.editor?.kind !== "maximumConsecutive") return;
  const mode = raw.mode;
  const breakMinutes =
    typeof mode === "object" && (mode.kind === "assignments" || mode.kind === "workedDays")
      ? (mode.breakMinutes ?? parseMaximumAssignmentCountDraft(""))
      : parseMaximumAssignmentCountDraft("");
  vm.updateRaw({
    ...raw,
    mode:
      value === "assignments"
        ? { kind: "assignments" as const, breakMinutes }
        : { kind: "workedDays" as const, breakMinutes },
  });
}
function updateMaximumConsecutiveBreakMinutes(value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || state.editor?.kind !== "maximumConsecutive") return;
  const mode = raw.mode;
  if (typeof mode !== "object" || mode.kind !== "assignments") return;
  vm.updateRaw({
    ...raw,
    mode: {
      ...mode,
      breakMinutes: parseMaximumAssignmentCountDraft(value),
    },
  });
}
function consecutiveWorkModeKind(): "reportingDays" | "weekends" {
  const mode = state.editor?.raw.mode;
  return typeof mode === "object" && mode.kind === "weekends" ? "weekends" : "reportingDays";
}
function consecutiveWorkCalendarId(): string {
  const mode = state.editor?.raw.mode;
  return typeof mode === "object" && mode.kind === "weekends" ? mode.calendarId : "";
}
function consecutiveWorkWeekdays(): readonly WorkforceWeekday[] {
  const mode = state.editor?.raw.mode;
  return typeof mode === "object" && mode.kind === "weekends" ? mode.weekdays : [];
}
function updateConsecutiveWorkMode(value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value || state.editor?.kind !== "consecutiveWork") return;
  const mode = raw.mode;
  vm.updateRaw({
    ...raw,
    mode:
      value === "weekends"
        ? typeof mode === "object" && mode.kind === "weekends"
          ? mode
          : { kind: "weekends" as const, calendarId: "", weekdays: [] }
        : { kind: "reportingDays" as const },
  });
}
function updateConsecutiveWorkCalendar(value: readonly string[]): void {
  const raw = state.editor?.raw;
  if (
    raw === undefined ||
    locked.value ||
    state.editor?.kind !== "consecutiveWork" ||
    typeof raw.mode !== "object" ||
    raw.mode.kind !== "weekends"
  )
    return;
  vm.updateRaw({ ...raw, mode: { ...raw.mode, calendarId: value[0] ?? "" } });
}
function toggleConsecutiveWorkWeekday(weekday: WorkforceWeekday, checked: boolean): void {
  const raw = state.editor?.raw;
  if (
    raw === undefined ||
    locked.value ||
    state.editor?.kind !== "consecutiveWork" ||
    typeof raw.mode !== "object" ||
    raw.mode.kind !== "weekends"
  )
    return;
  const selected = new Set(raw.mode.weekdays);
  if (checked) selected.add(weekday);
  else selected.delete(weekday);
  vm.updateRaw({
    ...raw,
    mode: {
      ...raw.mode,
      weekdays: consecutiveWorkWeekdayOptions.filter((candidate) => selected.has(candidate)),
    },
  });
}

function maximumHoursWindowLabel(kind: "calendar" | "rolling", membership: string): string {
  const window = kind === "calendar" ? copy.fields.calendarWindow : copy.fields.rollingWindow;
  const member =
    membership === "startInstant"
      ? copy.fields.startInstantMembership
      : membership === "intersection"
        ? copy.fields.intersectionMembership
        : copy.fields.reportingDateMembership;
  return `${window} (${member})`;
}
function preferenceTimeWindowLabel(window: WorkforceTimeWindow): string {
  return window.kind === "instant"
    ? `${window.startsAt} – ${window.endsAt}`
    : window.windows
        .map(
          (value) =>
            `${value.weekdays.join(", ")} ${value.startTime}–${value.endTime} (+${String(value.endDayOffset)}d)`,
        )
        .join("; ");
}
function consecutiveWorkWeekdayLabel(mode: WorkforceConsecutiveWorkMode): string {
  return mode.kind === "weekends"
    ? mode.weekdays.map((weekday) => copy.fields[weekday]).join(", ")
    : "";
}
function pairId(index: number, field: "firstCategory" | "secondCategory"): string | undefined {
  const id = state.editor?.id;
  const key = pairKeys.value[index];
  return id === undefined || key === undefined ? undefined : `rule-${id}-${key}-${field}`;
}
function qualificationMinimumId(key: string, field: "group" | "minimum" | "all" | "any"): string {
  if (state.editor === null) throw new Error("No rule selected.");
  return `rule-${state.editor.id}-qualification-minimum-${key}-${field}`;
}
function qualificationIds(ids: readonly string[]): string {
  return ids.join(", ");
}
function scopeErrors(
  part: WorkforceSetupScopePart,
): Readonly<Partial<Record<ScopeFieldName, string>>> {
  const prefix = `${partPaths[part]}.`;
  const errors: Partial<Record<ScopeFieldName, string>> = {};
  for (const [path, message] of Object.entries(state.errors)) {
    if (!path.startsWith(prefix)) continue;
    const segments = path.slice(prefix.length).split(".");
    const field =
      segments[0] === "people" && (segments[1] === "allTags" || segments[1] === "anyTags")
        ? segments[1]
        : segments[0];
    if (
      field === "people" ||
      field === "allTags" ||
      field === "anyTags" ||
      field === "teamIds" ||
      field === "assignmentTypeIds" ||
      field === "locationIds" ||
      field === "categories" ||
      field === "weekdays"
    )
      errors[field] = message;
  }
  return errors;
}
async function focusField(path: readonly string[], current: () => boolean): Promise<boolean> {
  const editor = state.editor;
  if (!current() || editor === null) return false;
  const owns = () => current() && state.editor === editor;
  const part =
    path[0] === "scope"
      ? "main"
      : path[0] === "beforeScope"
        ? "minimumRestBefore"
        : path[0] === "afterScope"
          ? "minimumRestAfter"
          : null;
  if (part !== null) {
    if (!parts.value.includes(part)) return false;
    await nextTick();
    if (!owns()) return false;
    return (await scopeFields.get(part)?.focusField(path.slice(1), owns)) ?? false;
  }
  let id: string | null = null;
  if (path.length === 0) {
    await nextTick();
    if (!owns()) return false;
    editorHeading.value?.focus();
    return document.activeElement === editorHeading.value;
  }
  if (path.length === 1) {
    const fields = {
      id: "rule-identity",
      kind: "rule-kind",
      strength: "rule-strength",
      active: "rule-active",
      targetCount:
        editor.kind === "weekendTarget"
          ? "rule-weekend-target-count"
          : "rule-assignment-target-count",
      assignmentTypeIds:
        editor.kind === "consecutiveWork"
          ? "rule-consecutive-work-assignment-types"
          : "rule-preference-assignment-types",
      availabilityIds: "rule-preference-availabilities",
      bucketId: "rule-workload-bucket",
      workloadPolicyId: "rule-workload-policy",
      calendarId:
        editor.kind === "assignmentTarget"
          ? "rule-assignment-target-calendar"
          : editor.kind === "consecutiveWork"
            ? "rule-consecutive-work-calendar"
            : "rule-calendar",
      measure: "rule-quota-measure",
      membership: "rule-quota-membership",
      minimum: "rule-quota-minimum",
      maximum: editor.kind === "workloadQuota" ? "rule-quota-maximum" : "rule-maximum",
      maximumMinutes: "rule-maximum-minutes",
      window: "rule-window",
      timeWindow: "rule-preference-time-window",
      minimumMinutes: "rule-minimum-rest",
      compatibleCategoryPairs: "rule-category-pairs",
      personId: "rule-fixed-person",
      shiftId: "rule-fixed-shift",
      assigned: "rule-fixed-assigned",
      personIds: "rule-mutual-people",
      locationIds:
        editor.class === "preference" ? "rule-preference-locations" : "rule-transition-locations",
      mode: "rule-mutual-mode",
      qualificationMinimums: "rule-qualification-minimums",
    } as const;
    const field = path[0];
    if (field !== undefined && Object.hasOwn(fields, field))
      id = fields[field as keyof typeof fields];
  }
  if (path.length === 1 && path[0] === "mode" && editor.kind === "maximumConsecutive")
    id = "rule-consecutive-mode";
  if (path.length === 1 && path[0] === "mode" && editor.kind === "consecutiveWork")
    id = "rule-consecutive-work-mode";
  if (path.length === 2 && path[0] === "mode" && path[1] === "breakMinutes")
    id = "rule-consecutive-break-minutes";
  if (path.length === 2 && path[0] === "mode" && path[1] === "calendarId")
    id = "rule-consecutive-work-calendar";
  if (path.length >= 2 && path[0] === "mode" && path[1] === "weekdays")
    id = "rule-consecutive-work-saturday";
  if (path.length === 2 && path[0] === "window") {
    const windowFields: Readonly<Record<string, string>> = {
      calendarId: "rule-calendar",
      durationMinutes: "rule-window-duration",
      membership: "rule-window-membership",
      kind: "rule-window-kind",
    };
    id = windowFields[path[1] ?? ""] ?? null;
  }
  if (path.length === 2 && path[0] === "measure" && path[1] === "bucketId")
    id = "rule-workload-bucket";
  if (path[0] === "timeWindow" && path[1] === "startsAt") id = "rule-preference-time-starts-at";
  if (path[0] === "timeWindow" && path[1] === "endsAt") id = "rule-preference-time-ends-at";
  if (
    path[0] === "timeWindow" &&
    path[1] === "windows" &&
    /^\d+$/.test(path[2] ?? "") &&
    (path[3] === "weekdays" ||
      path[3] === "startTime" ||
      path[3] === "endTime" ||
      path[3] === "endDayOffset")
  ) {
    const index = Number(path[2]);
    const windows = editor.raw.timeWindow.kind === "weekly" ? editor.raw.timeWindow.windows : [];
    const window = Number.isSafeInteger(index) && index >= 0 ? windows[index] : undefined;
    if (window === undefined) return false;
    if (path[3] === "weekdays") {
      let weekday = window.weekdays[0] ?? weekdays[0];
      if (path.length === 5) {
        const weekdayIndex = Number(path[4]);
        if (!/^\d+$/.test(path[4] ?? "") || !Number.isSafeInteger(weekdayIndex) || weekdayIndex < 0)
          return false;
        weekday = weekdays[weekdayIndex];
      } else if (path.length !== 4) return false;
      if (weekday === undefined) return false;
      id = timeWindowWeekdayId(window.key, weekday);
    } else if (path.length === 4 && path[3] === "startTime")
      id = timeWindowRowId(window.key, "start-time");
    else if (path.length === 4 && path[3] === "endTime")
      id = timeWindowRowId(window.key, "end-time");
    else if (path.length === 4 && path[3] === "endDayOffset")
      id = timeWindowRowId(window.key, "end-day-offset");
    else return false;
  }
  if (
    path[0] === "compatibleCategoryPairs" &&
    path.length === 3 &&
    /^\d+$/.test(path[1] ?? "") &&
    (path[2] === "firstCategory" || path[2] === "secondCategory")
  ) {
    const index = Number(path[1]);
    if (!Number.isSafeInteger(index) || index >= editor.raw.compatibleCategoryPairs.length)
      return false;
    pairPage.value = Math.floor(index / 50);
    id = pairId(index, path[2]) ?? null;
  }
  if (
    path[0] === "qualificationMinimums" &&
    /^\d+$/.test(path[1] ?? "") &&
    (path.length === 2 || path.length === 3 || path.length === 4)
  ) {
    const index = Number(path[1]);
    const row = Number.isSafeInteger(index) ? editor.raw.qualificationMinimums[index] : undefined;
    if (row === undefined) return false;
    const field = path.slice(2).join(".");
    const rowFields: Readonly<Record<string, string | undefined>> = {
      "": qualificationMinimumId(row.key, "group"),
      minimum: qualificationMinimumId(row.key, "minimum"),
      qualifications: qualificationMinimumId(row.key, "group"),
      "qualifications.allQualificationIds": qualificationMinimumId(row.key, "all"),
      "qualifications.anyQualificationIds": qualificationMinimumId(row.key, "any"),
    };
    id = rowFields[field] ?? null;
    skillMixPage.value = Math.floor(index / 50);
  }
  if (id === null) return false;
  await nextTick();
  if (!owns()) return false;
  const control = document.getElementById(id);
  if (!(control instanceof HTMLElement) || !editorHost.value?.contains(control)) return false;
  control.focus();
  return document.activeElement === control;
}
async function openRule(rule: WorkforceSetupRuleReference): Promise<void> {
  const current = ownership();
  if ((await vm.loadDetail(rule)) && current()) {
    await nextTick();
    if (current()) editorHeading.value?.focus();
  }
}
async function createRule(entry: WorkforceSetupRuleCatalogEntry): Promise<void> {
  const current = ownership();
  if (vm.createRule(entry)) await focusField([], current);
}
async function editRule(): Promise<void> {
  const current = ownership();
  if (vm.editRule()) await focusField([], current);
}
async function closeEditor(): Promise<void> {
  const current = ownership();
  vm.discard();
  await nextTick();
  if (current() && state.editor === null) listHeading.value?.focus();
}
async function preview(action: "create" | "update" | "activation" | "delete"): Promise<void> {
  const current = ownership();
  const ready = await vm.preview(action);
  if (!current()) return;
  await nextTick();
  if (!current()) return;
  if (ready) reviewHeading.value?.focus();
  else {
    const first = Object.keys(state.errors)[0];
    if (first === undefined || !(await focusField(first.split("."), current))) {
      if (current()) editorHeading.value?.focus();
    }
  }
}
async function save(truncateRedo = false): Promise<void> {
  const scenarioId = props.project.scenarioId;
  const focused = document.activeElement;
  if (
    !(await vm.save(truncateRedo)) ||
    !alive ||
    props.project.scenarioId !== scenarioId ||
    state.editor !== null
  )
    return;
  const current = ownership();
  const ownsList = () => current() && state.editor === null;
  await nextTick();
  if (
    ownsList() &&
    (document.activeElement === focused || document.activeElement === document.body)
  )
    listHeading.value?.focus();
}
async function rebase(): Promise<void> {
  const current = ownership();
  await vm.rebase();
  if (!current()) return;
  const first = Object.keys(state.errors)[0];
  await focusField(first === undefined ? [] : first.split("."), current);
}
function updatePair(index: number, field: "firstCategory" | "secondCategory", value: string): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value) return;
  vm.updateRaw({
    ...raw,
    compatibleCategoryPairs: raw.compatibleCategoryPairs.map((pair, at) =>
      at === index ? { ...pair, [field]: value } : pair,
    ),
  });
}
async function addPair(): Promise<void> {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value) return;
  const index = raw.compatibleCategoryPairs.length;
  vm.updateRaw({
    ...raw,
    compatibleCategoryPairs: [
      ...raw.compatibleCategoryPairs,
      { firstCategory: "", secondCategory: "" },
    ],
  });
  await focusField(["compatibleCategoryPairs", String(index), "firstCategory"], ownership());
}
async function removePair(index: number): Promise<void> {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value) return;
  pairKeys.value.splice(index, 1);
  vm.updateRaw({
    ...raw,
    compatibleCategoryPairs: raw.compatibleCategoryPairs.filter((_, at) => at !== index),
  });
  const next = Math.min(index, raw.compatibleCategoryPairs.length - 2);
  await focusField(
    next < 0
      ? ["compatibleCategoryPairs"]
      : ["compatibleCategoryPairs", String(next), "firstCategory"],
    ownership(),
  );
}
function updateQualificationMinimum<K extends keyof Omit<QualificationMinimumDraft, "key">>(
  key: string,
  field: K,
  value: QualificationMinimumDraft[K],
): void {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value) return;
  vm.updateRaw({
    ...raw,
    qualificationMinimums: raw.qualificationMinimums.map((row) =>
      row.key === key ? { ...row, [field]: value } : row,
    ),
  });
}
async function addQualificationMinimum(): Promise<void> {
  const raw = state.editor?.raw;
  if (raw === undefined || locked.value) return;
  const index = raw.qualificationMinimums.length;
  const key = `qualification-minimum-new-${String(++rowSequence)}`;
  vm.updateRaw({
    ...raw,
    qualificationMinimums: [
      ...raw.qualificationMinimums,
      { key, minimum: "", allQualificationIds: [], anyQualificationIds: [] },
    ],
  });
  skillMixPage.value = Math.floor(index / 50);
  await focusField(["qualificationMinimums", String(index), "minimum"], ownership());
}
async function removeQualificationMinimum(index: number): Promise<void> {
  const editor = state.editor;
  const raw = editor?.raw;
  if (
    raw === undefined ||
    locked.value ||
    (editor !== null && editor.base === null && raw.qualificationMinimums.length === 1)
  )
    return;
  vm.updateRaw({
    ...raw,
    qualificationMinimums: raw.qualificationMinimums.filter((_, at) => at !== index),
  });
  const next = Math.min(index, raw.qualificationMinimums.length - 2);
  await focusField(
    next < 0 ? ["qualificationMinimums"] : ["qualificationMinimums", String(next), "minimum"],
    ownership(),
  );
}
const navigationError = useValidationRoute(
  props.home,
  () => props.project,
  () => state.catalog !== null && state.facts !== null,
  async (target, current) => {
    if (
      (target.collection !== "rules" && target.collection !== "preferences") ||
      !current() ||
      !canReplace.value
    )
      return false;
    const loading = vm.loadDetail({
      class: target.collection === "preferences" ? "preference" : "required",
      ruleId: target.id,
    });
    const generation = state.draftGeneration;
    if (
      !(await loading) ||
      !current() ||
      state.draftGeneration !== generation ||
      state.editor?.id !== target.id ||
      !vm.editRule()
    )
      return false;
    const editor = state.editor;
    return await focusField(target.fieldPath, () => current() && state.editor === editor);
  },
);
</script>

<template>
  <section class="page-stack" aria-labelledby="rules-heading">
    <h2 id="rules-heading" data-route-heading tabindex="-1">{{ copy.heading }}</h2>
    <p v-if="project.domainPackId !== 'official.workforce'">{{ messages.setup.unsupported }}</p>
    <template v-else>
      <RouteLeaveGuard
        :home="home"
        :dirty="dirty || state.editor?.editing === true || review.state.review !== null"
        :pending="review.state.pending || home.state.busyAction === 'rule-scope-review'"
        :discard="vm.discard"
      />
      <p>{{ copy.strengthDescription }}</p>
      <div v-if="navigationError" class="state-panel" role="alert">{{ navigationError }}</div>
      <div v-if="state.catalogError" class="state-panel" role="alert">{{ state.catalogError }}</div>
      <button
        v-if="state.catalogError"
        type="button"
        :disabled="busy || state.catalogLoading || state.pageLoading"
        @click="vm.refresh"
      >
        {{ copy.refresh }}
      </button>
      <p v-if="state.catalogLoading" role="status">{{ recordsCopy.loading }}</p>
      <p v-if="state.facts">{{ messages.work.timeZone }}: {{ state.facts.settings.timeZone }}</p>
      <details v-if="state.facts" class="setup-more">
        <summary>{{ copy.moreRuleContext }}</summary>
        <p>
          {{
            copy.planningDates(
              state.facts.planningDates.startDate,
              state.facts.planningDates.endDateExclusive,
            )
          }}
        </p>
        <p>{{ recordsCopy.reviewRevision }} {{ formatNumber(project.revision, locale) }}</p>
      </details>
      <section
        v-if="state.catalog"
        v-show="state.editor === null && state.detail === null && review.state.review === null"
        class="field-stack"
        aria-labelledby="rule-catalog-heading"
      >
        <h3 id="rule-catalog-heading">{{ copy.availableKinds }}</h3>
        <ul class="field-stack">
          <li
            v-for="entry in state.catalog.required.filter((item) => vm.kindFor(item) !== null)"
            :key="entry.descriptor.id"
          >
            <button type="button" :disabled="!canCreateEntry(entry)" @click="createRule(entry)">
              {{ entry.descriptor.title.defaultText }}
            </button>
            <p>{{ descriptorDescription(entry.descriptor.id) }}</p>
          </li>
        </ul>
        <details>
          <summary>{{ copy.otherKinds }}</summary>
          <ul>
            <li
              v-for="entry in state.catalog.required.filter((item) => vm.kindFor(item) === null)"
              :key="entry.descriptor.id"
            >
              {{ entry.descriptor.title.defaultText }} — {{ copy.notImplemented }}
            </li>
          </ul>
        </details>
        <details>
          <summary>{{ copy.preferencesHeading }}</summary>
          <p>{{ copy.preferenceHint }}</p>
          <ul class="field-stack">
            <li
              v-for="entry in state.catalog.preferences.filter((item) => vm.kindFor(item) !== null)"
              :key="entry.descriptor.id"
            >
              <button type="button" :disabled="!canCreateEntry(entry)" @click="createRule(entry)">
                {{ entry.descriptor.title.defaultText }}
              </button>
              <p>{{ descriptorDescription(entry.descriptor.id) }}</p>
              <p v-if="sourceUnavailableMessage(entry)" role="status">
                {{ sourceUnavailableMessage(entry) }}
              </p>
              <p
                v-if="
                  entry.descriptor.id === 'official.workforce.preference.workload-balance' &&
                  state.workloadPolicies.status !== 'ready'
                "
                role="status"
              >
                {{
                  state.workloadPolicies.status === "empty"
                    ? copy.fields.workloadPoliciesEmpty
                    : state.workloadPolicies.status === "error"
                      ? copy.fields.workloadPoliciesError
                      : copy.fields.workloadPoliciesLoading
                }}
                <RouterLink
                  v-if="
                    state.workloadPolicies.status === 'empty' ||
                    state.workloadPolicies.status === 'error'
                  "
                  :to="{ name: 'project-score-policy', params: { scenarioId: project.scenarioId } }"
                >
                  {{ messages.scorePolicy.heading }}
                </RouterLink>
              </p>
            </li>
          </ul>
          <details>
            <summary>{{ copy.otherKinds }}</summary>
            <ul>
              <li
                v-for="entry in state.catalog.preferences.filter(
                  (item) => vm.kindFor(item) === null,
                )"
                :key="entry.descriptor.id"
              >
                {{ entry.descriptor.title.defaultText }} — {{ copy.notImplemented }}
              </li>
            </ul>
          </details>
        </details>
        <button
          type="button"
          :disabled="busy || state.catalogLoading || state.pageLoading"
          @click="vm.refresh"
        >
          {{ copy.refresh }}
        </button>
      </section>
      <section
        v-show="state.editor === null && state.detail === null && review.state.review === null"
        class="field-stack"
        aria-labelledby="rule-list-heading"
        :aria-busy="state.pageLoading"
      >
        <h3 id="rule-list-heading" ref="listHeading" tabindex="-1">{{ copy.list }}</h3>
        <div class="form-columns">
          <div class="field-stack">
            <label for="rule-class-filter">{{ copy.strength }}</label
            ><select
              id="rule-class-filter"
              :value="state.classFilter"
              @change="
                vm.setFilter(text($event) === 'preference' ? 'preference' : 'required', null)
              "
            >
              <option value="required">{{ copy.requiredHeading }}</option>
              <option value="preference">{{ copy.preferencesHeading }}</option>
            </select>
          </div>
          <div class="field-stack">
            <label for="rule-kind-filter">{{ copy.fields.kind }}</label
            ><select
              id="rule-kind-filter"
              :value="state.kindFilter ?? ''"
              @change="vm.setFilter(state.classFilter, text($event) || null)"
            >
              <option value="">{{ copy.allKinds }}</option>
              <option
                v-for="entry in catalogEntries"
                :key="entry.descriptor.id"
                :value="entry.descriptor.id"
              >
                {{ entry.descriptor.title.defaultText }}
              </option>
            </select>
          </div>
        </div>
        <p v-if="state.pageError" role="alert">{{ state.pageError }}</p>
        <p v-if="state.page?.items.length === 0">{{ copy.listEmpty }}</p>
        <ul v-if="state.page" class="field-stack">
          <li v-for="item in state.page.items" :key="`${item.rule.class}:${item.rule.ruleId}`">
            <button type="button" :disabled="!canReplace" @click="openRule(item.rule)">
              {{ descriptorLabel(item.kindId) }} · {{ item.active ? copy.active : copy.inactive }} ·
              {{ item.rule.ruleId }}
            </button>
          </li>
        </ul>
        <nav v-if="state.page" class="action-row" :aria-label="copy.list">
          <button type="button" :disabled="state.pageLoading" @click="vm.loadPage()">
            {{ recordsCopy.first }}</button
          ><button
            type="button"
            :disabled="state.pageLoading || state.page.continuation === null"
            @click="vm.loadPage(state.page.continuation)"
          >
            {{ recordsCopy.next }}
          </button>
        </nav>
      </section>
      <p v-if="state.detailLoading" role="status">{{ recordsCopy.loading }}</p>
      <p v-if="state.detailError" role="alert">{{ state.detailError }}</p>
      <section
        v-if="state.editor || state.detail"
        ref="editorHost"
        class="state-panel field-stack"
        aria-labelledby="rule-editor-heading"
      >
        <h3 id="rule-editor-heading" ref="editorHeading" tabindex="-1">
          {{ state.editor ? kindLabel(state.editor.kind) : copy.unsupportedClass }}
        </h3>
        <template v-if="state.editor">
          <p id="rule-identity" tabindex="-1" class="break-all">
            {{ copy.identity }}: {{ state.editor.id }}
          </p>
          <p id="rule-kind" tabindex="-1">
            {{ copy.fields.kind }}: {{ kindLabel(state.editor.kind) }}
          </p>
          <p id="rule-strength" tabindex="-1">
            {{ copy.strength }}:
            {{
              state.editor.class === "preference"
                ? copy.preferencesHeading
                : copy.strengthAlwaysRequired
            }}
          </p>
          <p>{{ copy.baseline }}: {{ formatNumber(state.editor.context.revision, locale) }}</p>
          <p v-if="dirty" role="status">{{ copy.unsaved }}</p>
          <p v-if="stale" role="status">{{ copy.stale }}</p>
          <p v-if="state.editorError" role="alert">{{ state.editorError }}</p>
          <div class="action-row">
            <button
              v-if="!state.editor.editing"
              type="button"
              :disabled="busy || stale"
              @click="editRule"
            >
              {{ copy.edit }}
            </button>
            <button
              v-if="!state.editor.editing"
              type="button"
              :disabled="busy || stale"
              @click="preview('activation')"
            >
              {{ copy.reviewToggle }}
            </button>
            <button
              v-if="!state.editor.editing"
              type="button"
              :disabled="busy || stale"
              @click="preview('delete')"
            >
              {{ copy.reviewDelete }}
            </button>
            <button v-if="stale" type="button" :disabled="busy" @click="rebase">
              {{ copy.reviewCurrent }}
            </button>
            <button type="button" :disabled="busy" @click="closeEditor">
              {{ dirty || state.editor.editing ? copy.discard : copy.close }}
            </button>
          </div>
          <section
            v-if="Object.keys(state.errors).length"
            role="alert"
            class="field-stack"
            :aria-label="copy.diagnostics"
          >
            <ul>
              <li v-for="(message, field) in state.errors" :key="field">
                <button type="button" @click="focusField(String(field).split('.'), ownership())">
                  {{ field }}: {{ message }}
                </button>
              </li>
            </ul>
          </section>
          <section v-if="state.editor.rebase" class="field-stack" :aria-label="copy.conflicts">
            <h4>{{ copy.conflicts }}</h4>
            <p>{{ copy.conflictHelp }}</p>
            <ul class="field-stack">
              <li v-for="field in state.editor.rebase.conflicts" :key="field">
                <strong>{{ field }}</strong>
                <details>
                  <summary>{{ copy.currentValue }}</summary>
                  <pre class="wrap-anywhere whitespace-pre-wrap">{{
                    nativeValue(fieldValue(state.editor.rebase.current, field))
                  }}</pre>
                </details>
                <details>
                  <summary>{{ copy.draftValue }}</summary>
                  <pre class="wrap-anywhere whitespace-pre-wrap">{{
                    nativeValue(fieldValue(state.editor.rebase.value, field))
                  }}</pre>
                </details>
                <div class="action-row">
                  <button
                    type="button"
                    :disabled="busy || !conflictsCurrent"
                    @click="vm.chooseConflict(field, 'current')"
                  >
                    {{ copy.useCurrent }}</button
                  ><button
                    type="button"
                    :disabled="busy || !conflictsCurrent"
                    @click="vm.chooseConflict(field, 'draft')"
                  >
                    {{ copy.keepDraft }}
                  </button>
                </div>
              </li>
            </ul>
          </section>
          <form
            class="field-stack"
            novalidate
            @submit.prevent="preview(state.editor.base === null ? 'create' : 'update')"
          >
            <label for="rule-active"
              ><input
                id="rule-active"
                type="checkbox"
                :checked="state.editor.raw.active"
                :disabled="locked"
                @change="
                  vm.updateRaw({
                    ...state.editor.raw,
                    active: ($event.target as HTMLInputElement).checked,
                  })
                "
              />
              {{ copy.active }}</label
            >
            <p v-if="!state.editor.raw.active">{{ copy.inactive }} — {{ copy.inactiveHelp }}</p>
            <RuleScopeBuilder
              v-for="part in parts"
              :id="`rule-${state.editor.id}-${part}`"
              :key="`${state.editor.id}:${part}`"
              :ref="
                (value) => {
                  if (value) scopeFields.set(part, value as InstanceType<typeof RuleScopeBuilder>);
                  else scopeFields.delete(part);
                }
              "
              :label="partLabels[part]"
              :model-value="vm.scopeValue(part)"
              :entity-options="vm.entityOptions(part)"
              :field-errors="scopeErrors(part)"
              :preview="state.scopes[part].preview"
              :current-request-key="state.scopes[part].requestKey"
              :preview-axis="state.scopes[part].axis"
              :preview-limit="50"
              :disabled="busy || state.editor.rebase !== null"
              :read-only="!state.editor.editing"
              v-bind="{
                ...(locale === undefined ? {} : { locale }),
                ...(state.errors[partPaths[part]] === undefined
                  ? {}
                  : { error: state.errors[partPaths[part]] }),
              }"
              @update:model-value="vm.updateScope(part, $event)"
              @entity-search="(field, query) => vm.searchEntity(part, field, query)"
              @entity-next-page="
                (field) => vm.searchEntity(part, field, vm.picker(part, field).query, true)
              "
              @entity-retry="
                (field) => vm.searchEntity(part, field, vm.picker(part, field).query, false, true)
              "
              @preview="vm.inspectScope(part, $event)"
            />
            <fieldset
              v-if="state.editor.class === 'preference'"
              id="rule-preference-settings"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ copy.preferencesHeading }}</legend>
              <p class="field-help">{{ copy.preferenceScopeHelp }}</p>
              <div class="form-grid">
                <div class="field-stack">
                  <label for="rule-preference-priority">{{ copy.fields.priority }}</label>
                  <select
                    id="rule-preference-priority"
                    :value="state.editor.raw.priority"
                    :disabled="
                      busy || stale || !state.editor.editing || state.editor.rebase !== null
                    "
                    :aria-invalid="Boolean(state.errors.priority) || undefined"
                    aria-describedby="rule-preference-priority-error"
                    @change="updatePreferencePriority(text($event))"
                  >
                    <option value="low">Low</option>
                    <option value="normal">Normal</option>
                    <option value="high">High</option>
                    <option value="veryHigh">Very high</option>
                  </select>
                  <p id="rule-preference-priority-error" class="text-danger" role="status">
                    {{ state.errors.priority }}
                  </p>
                </div>
                <div class="field-stack">
                  <label for="rule-preference-weight">{{ copy.fields.weight }}</label>
                  <p id="rule-preference-weight-help" class="field-help">
                    {{ copy.fields.weightHelp }}
                  </p>
                  <input
                    id="rule-preference-weight"
                    type="text"
                    inputmode="numeric"
                    required
                    :readonly="!state.editor.editing"
                    :disabled="busy || stale || state.editor.rebase !== null"
                    :value="state.editor.raw.weight.raw"
                    :aria-invalid="Boolean(state.errors.weight) || undefined"
                    aria-describedby="rule-preference-weight-help rule-preference-weight-error"
                    @input="updatePreferenceWeight(text($event))"
                  />
                  <p id="rule-preference-weight-error" class="text-danger" role="status">
                    {{ state.errors.weight }}
                  </p>
                </div>
              </div>
              <div v-if="state.editor.kind === 'workloadBalance'" class="field-stack">
                <label for="rule-workload-policy">{{ copy.fields.workloadPolicy }}</label>
                <p id="rule-workload-policy-help" class="field-help">
                  {{ copy.fields.workloadPolicyHelp }}
                </p>
                <select
                  id="rule-workload-policy"
                  :value="state.editor.raw.workloadPolicyId"
                  required
                  :disabled="
                    busy ||
                    stale ||
                    !state.editor.editing ||
                    state.editor.rebase !== null ||
                    state.workloadPolicies.status !== 'ready'
                  "
                  :aria-invalid="Boolean(state.errors.workloadPolicyId) || undefined"
                  aria-describedby="rule-workload-policy-help rule-workload-policy-error"
                  @change="
                    vm.updateRaw({
                      ...state.editor.raw,
                      workloadPolicyId: text($event),
                    })
                  "
                >
                  <option value="">{{ copy.fields.workloadPolicyChoose }}</option>
                  <option
                    v-for="policy in workloadPolicyEntries"
                    :key="policy.id"
                    :value="policy.id"
                  >
                    {{ policy.id }} · {{ workloadPolicyTargetMode(policy) }} ·
                    {{ workloadPolicyPeerGroup(policy) }}
                  </option>
                </select>
                <p id="rule-workload-policy-error" class="text-danger" role="status">
                  {{ state.errors.workloadPolicyId }}
                </p>
                <p v-if="state.workloadPolicies.status === 'loading'" role="status">
                  {{ copy.fields.workloadPoliciesLoading }}
                </p>
                <p v-else-if="state.workloadPolicies.status === 'empty'" role="status">
                  {{ copy.fields.workloadPoliciesEmpty }}
                  <RouterLink
                    :to="{
                      name: 'project-score-policy',
                      params: { scenarioId: project.scenarioId },
                    }"
                  >
                    {{ messages.scorePolicy.heading }}
                  </RouterLink>
                </p>
                <p v-else-if="state.workloadPolicies.status === 'error'" role="alert">
                  {{ copy.fields.workloadPoliciesError }}
                  <button type="button" :disabled="busy" @click="vm.refresh">
                    {{ copy.refresh }}
                  </button>
                </p>
                <p class="field-help">{{ copy.workloadBalanceScopeHelp }}</p>
              </div>
              <div
                v-if="
                  state.editor.kind === 'time' ||
                  state.editor.kind === 'assignmentType' ||
                  state.editor.kind === 'location' ||
                  state.editor.kind === 'adjacency'
                "
                class="field-stack"
              >
                <label for="rule-preference-direction">{{ copy.fields.direction }}</label>
                <select
                  id="rule-preference-direction"
                  :value="preferenceDirection()"
                  :disabled="busy || stale || !state.editor.editing || state.editor.rebase !== null"
                  :aria-invalid="Boolean(state.errors.direction) || undefined"
                  aria-describedby="rule-preference-direction-error"
                  @change="updatePreferenceDirection(text($event))"
                >
                  <option value="prefer">{{ copy.fields.prefer }}</option>
                  <option value="avoid">{{ copy.fields.avoid }}</option>
                </select>
                <p id="rule-preference-direction-error" class="text-danger" role="status">
                  {{ state.errors.direction }}
                </p>
              </div>
            </fieldset>
            <fieldset
              v-if="state.editor.kind === 'adjacency'"
              id="rule-adjacency"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ kindLabel(state.editor.kind) }}</legend>
              <WorkforceEntityPicker
                id="rule-adjacency-base-schedule"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="baseSchedule"
                :label="copy.fields.baseSchedule"
                :description="copy.fields.baseScheduleHelp"
                :model-value="
                  state.editor.raw.baseScheduleId === '' ? [] : [state.editor.raw.baseScheduleId]
                "
                required
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.baseScheduleId === undefined
                    ? {}
                    : { error: state.errors.baseScheduleId }),
                }"
                @update:model-value="
                  vm.updateRaw({ ...state.editor.raw, baseScheduleId: $event[0] ?? '' })
                "
              />
              <p v-if="baseScheduleCount === 0" role="status">
                {{ copy.fields.baseScheduleUnavailable }}
              </p>
              <div class="field-stack">
                <label for="rule-adjacency-gap">{{ copy.fields.maximumGapMinutes }}</label>
                <p id="rule-adjacency-gap-help" class="field-help">
                  {{ copy.fields.maximumGapMinutesHelp }}
                </p>
                <input
                  id="rule-adjacency-gap"
                  type="text"
                  inputmode="numeric"
                  required
                  :readonly="!state.editor.editing"
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :value="state.editor.raw.maximumGapMinutes.raw"
                  :aria-invalid="Boolean(state.errors.maximumGapMinutes) || undefined"
                  aria-describedby="rule-adjacency-gap-help rule-adjacency-gap-error"
                  @input="
                    vm.updateRaw({
                      ...state.editor.raw,
                      maximumGapMinutes: parseMaximumAssignmentCountDraft(text($event)),
                    })
                  "
                />
                <p id="rule-adjacency-gap-error" class="text-danger" role="status">
                  {{ state.errors.maximumGapMinutes }}
                </p>
              </div>
              <div class="field-stack">
                <label for="rule-adjacency-mode">{{ copy.fields.adjacencyMode }}</label>
                <select
                  id="rule-adjacency-mode"
                  :value="adjacencyMode()"
                  :disabled="busy || stale || !state.editor.editing || state.editor.rebase !== null"
                  :aria-invalid="Boolean(state.errors.mode) || undefined"
                  aria-describedby="rule-adjacency-mode-help rule-adjacency-mode-error"
                  @change="updateAdjacencyMode(text($event))"
                >
                  <option value="before">{{ copy.fields.adjacencyBefore }}</option>
                  <option value="after">{{ copy.fields.adjacencyAfter }}</option>
                  <option value="either">{{ copy.fields.adjacencyEither }}</option>
                </select>
                <p id="rule-adjacency-mode-help" class="field-help">
                  {{ copy.fields.adjacencyModeHelp }}
                </p>
                <p id="rule-adjacency-mode-error" class="text-danger" role="status">
                  {{ state.errors.mode }}
                </p>
              </div>
            </fieldset>
            <fieldset
              v-if="state.editor.kind === 'baseStability'"
              id="rule-base-stability"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ kindLabel(state.editor.kind) }}</legend>
              <WorkforceEntityPicker
                id="rule-base-stability-base-schedule"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="baseSchedule"
                :label="copy.fields.baseSchedule"
                :description="copy.fields.baseScheduleHelp"
                :model-value="
                  state.editor.raw.baseScheduleId === '' ? [] : [state.editor.raw.baseScheduleId]
                "
                required
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.baseScheduleId === undefined
                    ? {}
                    : { error: state.errors.baseScheduleId }),
                }"
                @update:model-value="
                  vm.updateRaw({ ...state.editor.raw, baseScheduleId: $event[0] ?? '' })
                "
              />
              <p v-if="baseScheduleCount === 0" role="status">
                {{ copy.fields.baseScheduleUnavailable }}
              </p>
            </fieldset>
            <fieldset
              v-if="state.editor.kind === 'togetherSeparate'"
              id="rule-together-separate"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ kindLabel(state.editor.kind) }}</legend>
              <WorkforceEntityPicker
                id="rule-together-separate-people"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="person"
                multiple
                :label="copy.fields.togetherSeparatePeople"
                :description="copy.fields.togetherSeparatePeopleHelp"
                :model-value="state.editor.raw.personIds"
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.personIds === undefined
                    ? {}
                    : { error: state.errors.personIds }),
                }"
                @update:model-value="vm.updateRaw({ ...state.editor.raw, personIds: $event })"
              />
              <div class="field-stack">
                <label for="rule-together-separate-direction">
                  {{ copy.fields.groupDirection }}
                </label>
                <select
                  id="rule-together-separate-direction"
                  :value="groupDirection()"
                  :disabled="busy || stale || !state.editor.editing || state.editor.rebase !== null"
                  :aria-invalid="Boolean(state.errors.direction) || undefined"
                  aria-describedby="rule-together-separate-direction-error"
                  @change="updateGroupDirection(text($event))"
                >
                  <option value="together">{{ copy.fields.together }}</option>
                  <option value="separate">{{ copy.fields.separate }}</option>
                </select>
                <p id="rule-together-separate-direction-error" class="text-danger" role="status">
                  {{ state.errors.direction }}
                </p>
              </div>
              <div class="field-stack">
                <label for="rule-together-separate-mode">{{ copy.fields.mutualMode }}</label>
                <select
                  id="rule-together-separate-mode"
                  :value="mutualModeKind()"
                  :disabled="busy || stale || !state.editor.editing || state.editor.rebase !== null"
                  :aria-invalid="Boolean(state.errors.mode) || undefined"
                  aria-describedby="rule-together-separate-mode-error"
                  @change="
                    vm.updateRaw({
                      ...state.editor.raw,
                      mode:
                        text($event) === 'overlappingShifts' ? 'overlappingShifts' : 'sameShift',
                    })
                  "
                >
                  <option value="sameShift">{{ copy.fields.sameShift }}</option>
                  <option value="overlappingShifts">{{ copy.fields.overlappingShifts }}</option>
                </select>
                <p id="rule-together-separate-mode-error" class="text-danger" role="status">
                  {{ state.errors.mode }}
                </p>
              </div>
            </fieldset>
            <fieldset
              v-if="state.editor.kind === 'preferredCoverage'"
              id="rule-preferred-coverage"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ kindLabel(state.editor.kind) }}</legend>
              <WorkforceEntityPicker
                id="rule-preferred-coverage-requirements"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="coverageRequirement"
                multiple
                :label="copy.fields.coverageRequirements"
                :description="copy.fields.coverageRequirementsHelp"
                :model-value="state.editor.raw.coverageRequirementIds"
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.coverageRequirementIds === undefined
                    ? {}
                    : { error: state.errors.coverageRequirementIds }),
                }"
                @update:model-value="
                  vm.updateRaw({ ...state.editor.raw, coverageRequirementIds: $event })
                "
              />
              <p v-if="coverageRequirementCount === 0" role="status">
                {{ copy.fields.coverageRequirementsUnavailable }}
              </p>
            </fieldset>
            <template v-if="state.editor.kind === 'consecutiveWork'">
              <fieldset
                id="rule-consecutive-work"
                tabindex="-1"
                class="field-stack"
                aria-describedby="rule-consecutive-work-help"
              >
                <legend>{{ kindLabel(state.editor.kind) }}</legend>
                <p id="rule-consecutive-work-help" class="field-help">
                  {{ copy.fields.maximumConsecutiveScopeHelp }}
                </p>
                <div class="field-stack">
                  <label for="rule-consecutive-work-threshold">
                    {{ copy.fields.consecutiveWorkThreshold }}
                  </label>
                  <p id="rule-consecutive-work-threshold-help" class="field-help">
                    {{ copy.fields.consecutiveWorkThresholdHelp }}
                  </p>
                  <input
                    id="rule-consecutive-work-threshold"
                    type="text"
                    inputmode="numeric"
                    required
                    :readonly="!state.editor.editing"
                    :disabled="busy || stale || state.editor.rebase !== null"
                    :value="state.editor.raw.threshold.raw"
                    :aria-invalid="Boolean(state.errors.threshold) || undefined"
                    aria-describedby="rule-consecutive-work-threshold-help rule-consecutive-work-threshold-error"
                    @input="
                      vm.updateRaw({
                        ...state.editor.raw,
                        threshold: parseMaximumAssignmentCountDraft(text($event)),
                      })
                    "
                  />
                  <p id="rule-consecutive-work-threshold-error" class="text-danger" role="status">
                    {{ state.errors.threshold }}
                  </p>
                </div>
                <div class="field-stack">
                  <label for="rule-consecutive-work-mode">{{
                    copy.fields.consecutiveWorkMode
                  }}</label>
                  <p id="rule-consecutive-work-mode-help" class="field-help">
                    {{ copy.fields.consecutiveWorkModeHelp }}
                  </p>
                  <select
                    id="rule-consecutive-work-mode"
                    :value="consecutiveWorkModeKind()"
                    :disabled="
                      busy || stale || !state.editor.editing || state.editor.rebase !== null
                    "
                    :aria-invalid="Boolean(state.errors.mode) || undefined"
                    aria-describedby="rule-consecutive-work-mode-help rule-consecutive-work-mode-error"
                    @change="updateConsecutiveWorkMode(text($event))"
                  >
                    <option value="reportingDays">{{ copy.fields.reportingDays }}</option>
                    <option value="weekends">{{ copy.fields.weekends }}</option>
                  </select>
                  <p id="rule-consecutive-work-mode-error" class="text-danger" role="status">
                    {{ state.errors.mode }}
                  </p>
                </div>
                <p v-if="consecutiveWorkModeKind() === 'reportingDays'" class="field-help">
                  {{ copy.fields.reportingDaysHelp }}
                </p>
                <template v-else>
                  <WorkforceEntityPicker
                    id="rule-consecutive-work-calendar"
                    :project="project"
                    :library-epoch="home.state.libraryEpoch"
                    kind="calendar"
                    :label="copy.fields.consecutiveWorkCalendar"
                    :description="copy.fields.consecutiveWorkCalendarHelp"
                    :model-value="
                      consecutiveWorkCalendarId() === '' ? [] : [consecutiveWorkCalendarId()]
                    "
                    :required="state.editor.raw.active"
                    :disabled="busy || stale || state.editor.rebase !== null"
                    :read-only="!state.editor.editing"
                    v-bind="{
                      ...(locale === undefined ? {} : { locale }),
                      ...(state.errors['mode.calendarId'] === undefined
                        ? {}
                        : { error: state.errors['mode.calendarId'] }),
                    }"
                    @update:model-value="updateConsecutiveWorkCalendar($event)"
                  />
                  <p v-if="calendarCount === 0" role="status">{{ copy.noCalendars }}</p>
                  <RouterLink
                    v-if="calendarCount === 0"
                    :to="{ name: 'project-work', params: { scenarioId: project.scenarioId } }"
                  >
                    {{ copy.manageCalendars }}
                  </RouterLink>
                  <fieldset
                    id="rule-consecutive-work-weekdays"
                    tabindex="-1"
                    class="field-stack"
                    :aria-invalid="Boolean(state.errors['mode.weekdays']) || undefined"
                    aria-describedby="rule-consecutive-work-weekdays-help rule-consecutive-work-weekdays-error"
                  >
                    <legend>{{ copy.fields.consecutiveWorkWeekdays }}</legend>
                    <p id="rule-consecutive-work-weekdays-help" class="field-help">
                      {{ copy.fields.consecutiveWorkWeekdaysHelp }}
                    </p>
                    <label
                      v-for="weekday in consecutiveWorkWeekdayOptions"
                      :key="weekday"
                      :for="`rule-consecutive-work-${weekday}`"
                    >
                      <input
                        :id="`rule-consecutive-work-${weekday}`"
                        type="checkbox"
                        :checked="consecutiveWorkWeekdays().includes(weekday)"
                        :disabled="
                          busy || stale || !state.editor.editing || state.editor.rebase !== null
                        "
                        @change="
                          toggleConsecutiveWorkWeekday(
                            weekday,
                            ($event.target as HTMLInputElement).checked,
                          )
                        "
                      />
                      {{ copy.fields[weekday] }}
                    </label>
                    <p id="rule-consecutive-work-weekdays-error" class="text-danger" role="status">
                      {{ state.errors["mode.weekdays"] }}
                    </p>
                  </fieldset>
                </template>
              </fieldset>
              <WorkforceEntityPicker
                id="rule-consecutive-work-assignment-types"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="assignmentType"
                multiple
                :label="copy.fields.consecutiveWorkAssignmentTypes"
                :description="copy.fields.consecutiveWorkAssignmentTypesHelp"
                :model-value="state.editor.raw.assignmentTypeIds"
                :required="state.editor.raw.active"
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.assignmentTypeIds === undefined
                    ? {}
                    : { error: state.errors.assignmentTypeIds }),
                }"
                @update:model-value="
                  vm.updateRaw({ ...state.editor.raw, assignmentTypeIds: $event })
                "
              />
            </template>
            <template v-if="state.editor.kind === 'assignmentTarget'">
              <WorkforceEntityPicker
                id="rule-assignment-target-calendar"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="calendar"
                :label="copy.fields.calendar"
                :description="copy.fields.calendarHelp"
                :model-value="
                  state.editor.raw.calendarId === '' ? [] : [state.editor.raw.calendarId]
                "
                required
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.calendarId === undefined
                    ? {}
                    : { error: state.errors.calendarId }),
                }"
                @update:model-value="
                  vm.updateRaw({ ...state.editor.raw, calendarId: $event[0] ?? '' })
                "
              />
              <p v-if="calendarCount === 0" role="status">{{ copy.noCalendars }}</p>
              <RouterLink
                v-if="calendarCount === 0"
                :to="{ name: 'project-work', params: { scenarioId: project.scenarioId } }"
              >
                {{ copy.manageCalendars }}
              </RouterLink>
              <div class="field-stack">
                <label for="rule-assignment-target-count">{{ copy.fields.targetCount }}</label>
                <p id="rule-assignment-target-count-help" class="field-help">
                  {{ copy.fields.targetCountHelp }}
                </p>
                <input
                  id="rule-assignment-target-count"
                  type="text"
                  inputmode="numeric"
                  required
                  :readonly="!state.editor.editing"
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :value="state.editor.raw.targetCount.raw"
                  :aria-invalid="Boolean(state.errors.targetCount) || undefined"
                  aria-describedby="rule-assignment-target-count-help rule-assignment-target-count-error"
                  @input="
                    vm.updateRaw({
                      ...state.editor.raw,
                      targetCount: parseMaximumAssignmentCountDraft(text($event)),
                    })
                  "
                />
                <p id="rule-assignment-target-count-error" class="text-danger" role="status">
                  {{ state.errors.targetCount }}
                </p>
              </div>
            </template>
            <template v-if="state.editor.kind === 'weekendTarget'">
              <fieldset id="rule-weekend-target" tabindex="-1" class="field-stack">
                <legend>{{ kindLabel(state.editor.kind) }}</legend>
                <p id="rule-weekend-target-help" class="field-help">
                  {{ copy.fields.weekendTargetCountHelp }}
                </p>
                <label for="rule-weekend-target-count">
                  {{ copy.fields.weekendTargetCount }}
                </label>
                <input
                  id="rule-weekend-target-count"
                  type="text"
                  inputmode="numeric"
                  required
                  :readonly="!state.editor.editing"
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :value="state.editor.raw.targetCount.raw"
                  :aria-invalid="Boolean(state.errors.targetCount) || undefined"
                  aria-describedby="rule-weekend-target-help rule-weekend-target-count-error"
                  @input="
                    vm.updateRaw({
                      ...state.editor.raw,
                      targetCount: parseMaximumAssignmentCountDraft(text($event)),
                    })
                  "
                />
                <p id="rule-weekend-target-count-error" class="text-danger" role="status">
                  {{ state.errors.targetCount }}
                </p>
              </fieldset>
            </template>
            <WorkforceEntityPicker
              v-if="state.editor.kind === 'assignmentType'"
              id="rule-preference-assignment-types"
              :project="project"
              :library-epoch="home.state.libraryEpoch"
              kind="assignmentType"
              multiple
              :label="copy.fields.assignmentTypes"
              :description="copy.fields.assignmentTypesHelp"
              :model-value="state.editor.raw.assignmentTypeIds"
              :disabled="busy || stale || state.editor.rebase !== null"
              :read-only="!state.editor.editing"
              v-bind="{
                ...(locale === undefined ? {} : { locale }),
                ...(state.errors.assignmentTypeIds === undefined
                  ? {}
                  : { error: state.errors.assignmentTypeIds }),
              }"
              @update:model-value="vm.updateRaw({ ...state.editor.raw, assignmentTypeIds: $event })"
            />
            <WorkforceEntityPicker
              v-if="state.editor.kind === 'location'"
              id="rule-preference-locations"
              :project="project"
              :library-epoch="home.state.libraryEpoch"
              kind="location"
              multiple
              :label="copy.fields.locations"
              :description="copy.fields.locationsHelp"
              :model-value="state.editor.raw.locationIds"
              :disabled="busy || stale || state.editor.rebase !== null"
              :read-only="!state.editor.editing"
              v-bind="{
                ...(locale === undefined ? {} : { locale }),
                ...(state.errors.locationIds === undefined
                  ? {}
                  : { error: state.errors.locationIds }),
              }"
              @update:model-value="vm.updateRaw({ ...state.editor.raw, locationIds: $event })"
            />
            <WorkforceEntityPicker
              v-if="state.editor.kind === 'requestedTimeOff'"
              id="rule-preference-availabilities"
              :project="project"
              :library-epoch="home.state.libraryEpoch"
              kind="availability"
              multiple
              :label="copy.fields.availabilities"
              :description="copy.fields.availabilitiesHelp"
              :model-value="state.editor.raw.availabilityIds"
              :disabled="busy || stale || state.editor.rebase !== null"
              :read-only="!state.editor.editing"
              v-bind="{
                ...(locale === undefined ? {} : { locale }),
                ...(state.errors.availabilityIds === undefined
                  ? {}
                  : { error: state.errors.availabilityIds }),
              }"
              @update:model-value="vm.updateRaw({ ...state.editor.raw, availabilityIds: $event })"
            />
            <fieldset
              v-if="state.editor.kind === 'time'"
              id="rule-preference-time-window"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ copy.fields.timeWindow }}</legend>
              <p id="rule-preference-time-window-help" class="field-help">
                {{ copy.fields.timeWindowHelp }}
              </p>
              <label for="rule-preference-time-window-kind">
                {{ copy.fields.timeWindowKind }}
              </label>
              <select
                id="rule-preference-time-window-kind"
                :value="timeWindowKind()"
                :disabled="busy || stale || !state.editor.editing || state.editor.rebase !== null"
                :aria-invalid="Boolean(state.errors['timeWindow.kind']) || undefined"
                aria-describedby="rule-preference-time-window-help rule-preference-time-window-kind-error"
                @change="updateTimeWindowKind(text($event))"
              >
                <option value="instant">{{ copy.fields.instantWindow }}</option>
                <option value="weekly">{{ copy.fields.weeklyWindow }}</option>
              </select>
              <p id="rule-preference-time-window-kind-error" class="text-danger" role="status">
                {{ state.errors["timeWindow.kind"] }}
              </p>
              <div v-if="state.editor.raw.timeWindow.kind === 'instant'" class="form-grid">
                <div class="field-stack">
                  <label for="rule-preference-time-starts-at">{{ copy.fields.startsAt }}</label>
                  <input
                    id="rule-preference-time-starts-at"
                    type="text"
                    autocomplete="off"
                    :readonly="!state.editor.editing"
                    :disabled="busy || stale || state.editor.rebase !== null"
                    :value="state.editor.raw.timeWindow.startsAt"
                    :aria-invalid="Boolean(state.errors['timeWindow.startsAt']) || undefined"
                    aria-describedby="rule-preference-time-starts-at-error"
                    @input="updateTimeWindowField('startsAt', text($event))"
                  />
                  <p id="rule-preference-time-starts-at-error" class="text-danger" role="status">
                    {{ state.errors["timeWindow.startsAt"] }}
                  </p>
                </div>
                <div class="field-stack">
                  <label for="rule-preference-time-ends-at">{{ copy.fields.endsAt }}</label>
                  <input
                    id="rule-preference-time-ends-at"
                    type="text"
                    autocomplete="off"
                    :readonly="!state.editor.editing"
                    :disabled="busy || stale || state.editor.rebase !== null"
                    :value="state.editor.raw.timeWindow.endsAt"
                    :aria-invalid="Boolean(state.errors['timeWindow.endsAt']) || undefined"
                    aria-describedby="rule-preference-time-ends-at-error"
                    @input="updateTimeWindowField('endsAt', text($event))"
                  />
                  <p id="rule-preference-time-ends-at-error" class="text-danger" role="status">
                    {{ state.errors["timeWindow.endsAt"] }}
                  </p>
                </div>
              </div>
              <template v-else>
                <p v-if="state.editor.raw.timeWindow.windows.length === 0" role="status">
                  {{ copy.timeWindowRequired }}
                </p>
                <fieldset
                  v-for="(window, index) in state.editor.raw.timeWindow.windows"
                  :id="timeWindowRowId(window.key, 'group')"
                  :key="window.key"
                  class="field-stack"
                >
                  <legend>{{ copy.fields.windowNumber(formatNumber(index + 1, locale)) }}</legend>
                  <div class="field-stack">
                    <span :id="timeWindowRowId(window.key, 'weekdays-help')" class="field-help">
                      {{ copy.fields.weekdays }}
                    </span>
                    <div
                      class="form-columns"
                      role="group"
                      :aria-labelledby="timeWindowRowId(window.key, 'weekdays-help')"
                    >
                      <label
                        v-for="weekday in weekdays"
                        :key="weekday"
                        :for="timeWindowWeekdayId(window.key, weekday)"
                      >
                        <input
                          :id="timeWindowWeekdayId(window.key, weekday)"
                          type="checkbox"
                          :checked="window.weekdays.includes(weekday)"
                          :disabled="
                            busy || stale || !state.editor.editing || state.editor.rebase !== null
                          "
                          @change="
                            toggleWeeklyWeekday(
                              window.key,
                              weekday,
                              ($event.target as HTMLInputElement).checked,
                            )
                          "
                        />
                        {{ copy.fields[weekday] }}
                      </label>
                    </div>
                    <p class="text-danger" role="status">
                      {{ state.errors[`timeWindow.windows.${index}.weekdays`] }}
                    </p>
                  </div>
                  <div class="form-grid">
                    <div class="field-stack">
                      <label :for="timeWindowRowId(window.key, 'start-time')">
                        {{ copy.fields.startTime }}
                      </label>
                      <input
                        :id="timeWindowRowId(window.key, 'start-time')"
                        type="text"
                        :readonly="!state.editor.editing"
                        :disabled="busy || stale || state.editor.rebase !== null"
                        :value="window.startTime"
                        :aria-invalid="
                          Boolean(state.errors[`timeWindow.windows.${index}.startTime`]) ||
                          undefined
                        "
                        :aria-describedby="timeWindowRowId(window.key, 'start-time-error')"
                        @input="updateWeeklyWindow(window.key, 'startTime', text($event))"
                      />
                      <p
                        :id="timeWindowRowId(window.key, 'start-time-error')"
                        class="text-danger"
                        role="status"
                      >
                        {{ state.errors[`timeWindow.windows.${index}.startTime`] }}
                      </p>
                    </div>
                    <div class="field-stack">
                      <label :for="timeWindowRowId(window.key, 'end-time')">
                        {{ copy.fields.endTime }}
                      </label>
                      <input
                        :id="timeWindowRowId(window.key, 'end-time')"
                        type="text"
                        :readonly="!state.editor.editing"
                        :disabled="busy || stale || state.editor.rebase !== null"
                        :value="window.endTime"
                        :aria-invalid="
                          Boolean(state.errors[`timeWindow.windows.${index}.endTime`]) || undefined
                        "
                        :aria-describedby="timeWindowRowId(window.key, 'end-time-error')"
                        @input="updateWeeklyWindow(window.key, 'endTime', text($event))"
                      />
                      <p
                        :id="timeWindowRowId(window.key, 'end-time-error')"
                        class="text-danger"
                        role="status"
                      >
                        {{ state.errors[`timeWindow.windows.${index}.endTime`] }}
                      </p>
                    </div>
                    <div class="field-stack">
                      <label :for="timeWindowRowId(window.key, 'end-day-offset')">
                        {{ copy.fields.endDayOffset }}
                      </label>
                      <input
                        :id="timeWindowRowId(window.key, 'end-day-offset')"
                        type="text"
                        inputmode="numeric"
                        :readonly="!state.editor.editing"
                        :disabled="busy || stale || state.editor.rebase !== null"
                        :value="window.endDayOffset"
                        :aria-invalid="
                          Boolean(state.errors[`timeWindow.windows.${index}.endDayOffset`]) ||
                          undefined
                        "
                        :aria-describedby="timeWindowRowId(window.key, 'end-day-offset-error')"
                        @input="updateWeeklyWindow(window.key, 'endDayOffset', text($event))"
                      />
                      <p
                        :id="timeWindowRowId(window.key, 'end-day-offset-error')"
                        class="text-danger"
                        role="status"
                      >
                        {{ state.errors[`timeWindow.windows.${index}.endDayOffset`] }}
                      </p>
                    </div>
                  </div>
                  <button
                    v-if="state.editor.editing"
                    type="button"
                    :disabled="locked"
                    @click="removeWeeklyWindow(index)"
                  >
                    {{ copy.fields.removeTimeWindow }}
                  </button>
                </fieldset>
                <button
                  v-if="state.editor.editing"
                  type="button"
                  :disabled="locked"
                  @click="addWeeklyWindow"
                >
                  {{ copy.fields.addTimeWindow }}
                </button>
              </template>
            </fieldset>
            <fieldset
              v-if="state.editor.kind === 'mutualAssignmentRestriction'"
              id="rule-mutual-assignment"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ kindLabel(state.editor.kind) }}</legend>
              <p class="field-help">{{ copy.mutualAssignmentRestrictionScopeHelp }}</p>
              <WorkforceEntityPicker
                id="rule-mutual-people"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="person"
                multiple
                :label="copy.fields.mutualPeople"
                :description="copy.fields.mutualPeopleHelp"
                :model-value="state.editor.raw.personIds"
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.personIds === undefined
                    ? {}
                    : { error: state.errors.personIds }),
                }"
                @update:model-value="vm.updateRaw({ ...state.editor.raw, personIds: $event })"
              />
              <div class="field-stack">
                <label for="rule-mutual-mode">{{ copy.fields.mutualMode }}</label>
                <p id="rule-mutual-mode-help" class="field-help">
                  {{ copy.fields.mutualModeHelp }}
                </p>
                <select
                  id="rule-mutual-mode"
                  :value="mutualModeKind()"
                  :disabled="busy || stale || !state.editor.editing || state.editor.rebase !== null"
                  aria-describedby="rule-mutual-mode-help rule-mutual-mode-error"
                  :aria-invalid="Boolean(state.errors.mode) || undefined"
                  @change="
                    vm.updateRaw({
                      ...state.editor.raw,
                      mode:
                        text($event) === 'overlappingShifts' ? 'overlappingShifts' : 'sameShift',
                    })
                  "
                >
                  <option value="sameShift">{{ copy.fields.sameShift }}</option>
                  <option value="overlappingShifts">{{ copy.fields.overlappingShifts }}</option>
                </select>
                <p id="rule-mutual-mode-error" class="text-danger" role="status">
                  {{ state.errors.mode }}
                </p>
              </div>
              <p>
                {{
                  copy.mutualAssignmentRestrictionSentence(
                    formatNumber(state.editor.raw.personIds.length, locale),
                    mutualModeKind(),
                  )
                }}
              </p>
            </fieldset>
            <fieldset
              v-if="state.editor.kind === 'transitionTime'"
              id="rule-transition-time"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ kindLabel(state.editor.kind) }}</legend>
              <p class="field-help">{{ copy.transitionTimeScopeHelp }}</p>
              <WorkforceEntityPicker
                id="rule-transition-locations"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="location"
                multiple
                :label="copy.fields.transitionLocations"
                :description="copy.fields.transitionLocationsHelp"
                :model-value="state.editor.raw.locationIds"
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.locationIds === undefined
                    ? {}
                    : { error: state.errors.locationIds }),
                }"
                @update:model-value="vm.updateRaw({ ...state.editor.raw, locationIds: $event })"
              />
              <p>
                {{
                  copy.transitionTimeSentence(
                    formatNumber(state.editor.raw.locationIds.length, locale),
                  )
                }}
              </p>
            </fieldset>
            <fieldset
              v-if="state.editor.kind === 'fixedAssignment'"
              id="rule-fixed-assignment-target"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ kindLabel(state.editor.kind) }}</legend>
              <p class="field-help">{{ copy.fixedAssignmentScopeHelp }}</p>
              <label for="rule-fixed-assigned">
                {{ copy.fields.fixedAssignmentMode }}
                <select
                  id="rule-fixed-assigned"
                  :value="state.editor.raw.assigned ? 'required' : 'excluded'"
                  :disabled="locked"
                  @change="
                    vm.updateRaw({ ...state.editor.raw, assigned: text($event) === 'required' })
                  "
                >
                  <option value="required">{{ copy.fields.fixedAssignmentRequired }}</option>
                  <option value="excluded">{{ copy.fields.fixedAssignmentExcluded }}</option>
                </select>
              </label>
              <WorkforceEntityPicker
                id="rule-fixed-person"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="person"
                :label="copy.fields.fixedAssignmentPerson"
                :model-value="state.editor.raw.personId === '' ? [] : [state.editor.raw.personId]"
                required
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.personId === undefined ? {} : { error: state.errors.personId }),
                }"
                @update:model-value="
                  vm.updateRaw({ ...state.editor.raw, personId: $event[0] ?? '' })
                "
              />
              <h4 id="rule-fixed-shift-label">{{ copy.fields.fixedAssignmentShift }}</h4>
              <div v-if="state.facts" class="form-grid">
                <label for="rule-fixed-shift-start">
                  {{ copy.fields.fixedAssignmentShiftStart }}
                  <input
                    id="rule-fixed-shift-start"
                    v-model="fixedShiftDates.startDate"
                    type="date"
                    :min="state.facts.planningDates.startDate"
                    :max="state.facts.planningDates.endDateExclusive"
                  />
                </label>
                <label for="rule-fixed-shift-end">
                  {{ copy.fields.fixedAssignmentShiftEnd }}
                  <input
                    id="rule-fixed-shift-end"
                    v-model="fixedShiftDates.endDateExclusive"
                    type="date"
                    :min="state.facts.planningDates.startDate"
                    :max="state.facts.planningDates.endDateExclusive"
                  />
                </label>
              </div>
              <ShiftScopePicker
                v-if="state.facts"
                id="rule-fixed-shift"
                aria-labelledby="rule-fixed-shift-label"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                :dates="fixedShiftDates"
                :selected-ids="state.editor.raw.shiftId === '' ? [] : [state.editor.raw.shiftId]"
                :error="state.errors.shiftId"
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                }"
                @update:model-value="
                  vm.updateRaw({ ...state.editor.raw, shiftId: $event[0] ?? '' })
                "
              />
              <p v-else role="status">{{ recordsCopy.loading }}</p>
              <p>
                {{
                  copy.fixedAssignmentSentence(
                    state.editor.raw.personId || copy.absent,
                    state.editor.raw.shiftId || copy.absent,
                    state.editor.raw.assigned,
                  )
                }}
              </p>
            </fieldset>
            <template
              v-if="
                state.editor.kind === 'maximumAssignmentCount' ||
                state.editor.kind === 'maximumWorkedWeekends'
              "
            >
              <template v-if="state.editor.kind === 'maximumAssignmentCount'">
                <WorkforceEntityPicker
                  id="rule-calendar"
                  :project="project"
                  :library-epoch="home.state.libraryEpoch"
                  kind="calendar"
                  :label="copy.fields.calendar"
                  :description="copy.fields.calendarHelp"
                  :model-value="
                    state.editor.raw.calendarId === '' ? [] : [state.editor.raw.calendarId]
                  "
                  required
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :read-only="!state.editor.editing"
                  v-bind="{
                    ...(locale === undefined ? {} : { locale }),
                    ...(state.errors.calendarId === undefined
                      ? {}
                      : { error: state.errors.calendarId }),
                  }"
                  @update:model-value="
                    vm.updateRaw({ ...state.editor.raw, calendarId: $event[0] ?? '' })
                  "
                />
                <p v-if="calendarCount === 0" role="status">{{ copy.noCalendars }}</p>
                <RouterLink
                  v-if="calendarCount === 0"
                  :to="{ name: 'project-work', params: { scenarioId: project.scenarioId } }"
                >
                  {{ copy.manageCalendars }}
                </RouterLink>
              </template>
              <div class="field-stack">
                <label for="rule-maximum">{{
                  state.editor.kind === "maximumWorkedWeekends"
                    ? copy.fields.maximumWorkedWeekends
                    : copy.fields.maximum
                }}</label>
                <p id="rule-maximum-help" class="field-help">
                  {{
                    state.editor.kind === "maximumWorkedWeekends"
                      ? copy.fields.maximumWorkedWeekendsHelp
                      : copy.fields.maximumHelp
                  }}
                </p>
                <input
                  id="rule-maximum"
                  type="text"
                  inputmode="numeric"
                  required
                  :readonly="!state.editor.editing"
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :value="state.editor.raw.maximum.raw"
                  :aria-invalid="Boolean(state.errors.maximum) || undefined"
                  aria-describedby="rule-maximum-help rule-maximum-error"
                  @input="
                    vm.updateRaw({
                      ...state.editor.raw,
                      maximum: parseMaximumAssignmentCountDraft(text($event)),
                    })
                  "
                />
                <p id="rule-maximum-error" class="text-danger" role="status">
                  {{ state.errors.maximum }}
                </p>
              </div>
            </template>
            <fieldset
              v-if="state.editor.kind === 'workloadQuota'"
              id="rule-quota"
              class="field-stack"
              aria-describedby="rule-quota-help"
            >
              <legend>{{ kindLabel(state.editor.kind) }}</legend>
              <p id="rule-quota-help" class="field-help">{{ copy.fields.quotaHelp }}</p>
              <WorkforceEntityPicker
                id="rule-calendar"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="calendar"
                :label="copy.fields.quotaCalendar"
                :description="copy.fields.quotaCalendarHelp"
                :model-value="
                  state.editor.raw.calendarId === '' ? [] : [state.editor.raw.calendarId]
                "
                required
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.calendarId === undefined
                    ? {}
                    : { error: state.errors.calendarId }),
                }"
                @update:model-value="
                  vm.updateRaw({ ...state.editor.raw, calendarId: $event[0] ?? '' })
                "
              />
              <p v-if="calendarCount === 0" role="status">{{ copy.noCalendars }}</p>
              <RouterLink
                v-if="calendarCount === 0"
                :to="{ name: 'project-work', params: { scenarioId: project.scenarioId } }"
              >
                {{ copy.manageCalendars }}
              </RouterLink>
              <div class="field-stack">
                <label for="rule-quota-measure">{{ copy.fields.quotaMeasure }}</label>
                <select
                  id="rule-quota-measure"
                  :value="state.editor.raw.quotaMeasure"
                  :disabled="locked"
                  @change="
                    vm.updateRaw({
                      ...state.editor.raw,
                      quotaMeasure:
                        text($event) === 'bucketMinutes' ? 'bucketMinutes' : 'assignmentCount',
                    })
                  "
                >
                  <option value="assignmentCount">{{ copy.fields.quotaAssignments }}</option>
                  <option value="bucketMinutes">{{ copy.fields.quotaMinutes }}</option>
                </select>
              </div>
              <WorkforceEntityPicker
                v-if="state.editor.raw.quotaMeasure === 'bucketMinutes'"
                id="rule-workload-bucket"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="workloadBucket"
                :label="copy.fields.workloadBucket"
                :description="copy.fields.workloadBucketHelp"
                :model-value="state.editor.raw.bucketId === '' ? [] : [state.editor.raw.bucketId]"
                required
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors['measure.bucketId'] === undefined
                    ? {}
                    : { error: state.errors['measure.bucketId'] }),
                }"
                @update:model-value="
                  vm.updateRaw({ ...state.editor.raw, bucketId: $event[0] ?? '' })
                "
              />
              <p
                v-if="
                  state.editor.raw.quotaMeasure === 'bucketMinutes' && workloadBucketCount === 0
                "
                role="status"
              >
                {{ copy.fields.noWorkloadBuckets }}
              </p>
              <div class="field-stack">
                <label for="rule-quota-membership">{{ copy.fields.membership }}</label>
                <select
                  id="rule-quota-membership"
                  :value="state.editor.raw.quotaMembership"
                  :disabled="locked"
                  :aria-invalid="Boolean(state.errors.membership) || undefined"
                  aria-describedby="rule-quota-help rule-quota-membership-error"
                  @change="
                    vm.updateRaw({
                      ...state.editor.raw,
                      quotaMembership:
                        text($event) === 'startInstant'
                          ? 'startInstant'
                          : text($event) === 'intersection'
                            ? 'intersection'
                            : 'reportingDate',
                    })
                  "
                >
                  <option value="reportingDate">{{ copy.fields.reportingDateMembership }}</option>
                  <option value="startInstant">{{ copy.fields.startInstantMembership }}</option>
                  <option
                    value="intersection"
                    :disabled="state.editor.raw.quotaMeasure === 'assignmentCount'"
                  >
                    {{ copy.fields.intersectionMembership }}
                  </option>
                </select>
                <p id="rule-quota-membership-error" class="text-danger" role="status">
                  {{ state.errors.membership }}
                </p>
              </div>
              <div class="field-stack">
                <label for="rule-quota-minimum">{{ copy.fields.quotaMinimum }}</label>
                <input
                  id="rule-quota-minimum"
                  type="text"
                  inputmode="numeric"
                  required
                  :readonly="!state.editor.editing"
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :value="state.editor.raw.quotaMinimum.raw"
                  :aria-invalid="Boolean(state.errors.minimum) || undefined"
                  aria-describedby="rule-quota-help rule-quota-minimum-error"
                  @input="
                    vm.updateRaw({
                      ...state.editor.raw,
                      quotaMinimum: parseMaximumAssignmentCountDraft(text($event)),
                    })
                  "
                />
                <p id="rule-quota-minimum-error" class="text-danger" role="status">
                  {{ state.errors.minimum }}
                </p>
              </div>
              <div class="field-stack">
                <label for="rule-quota-maximum">{{ copy.fields.quotaMaximum }}</label>
                <p id="rule-quota-maximum-help" class="field-help">
                  {{ copy.fields.quotaMaximumHelp }}
                </p>
                <input
                  id="rule-quota-maximum"
                  type="text"
                  inputmode="numeric"
                  :readonly="!state.editor.editing"
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :value="state.editor.raw.quotaMaximum.raw"
                  :aria-invalid="Boolean(state.errors.maximum) || undefined"
                  aria-describedby="rule-quota-maximum-help rule-quota-maximum-error"
                  @input="
                    vm.updateRaw({
                      ...state.editor.raw,
                      quotaMaximum: parseMaximumAssignmentCountDraft(text($event)),
                    })
                  "
                />
                <p id="rule-quota-maximum-error" class="text-danger" role="status">
                  {{ state.errors.maximum }}
                </p>
              </div>
            </fieldset>
            <template v-if="state.editor.kind === 'maximumConsecutive'">
              <fieldset
                id="rule-maximum-consecutive"
                tabindex="-1"
                class="field-stack"
                aria-describedby="rule-maximum-consecutive-help"
              >
                <legend>{{ kindLabel(state.editor.kind) }}</legend>
                <p id="rule-maximum-consecutive-help" class="field-help">
                  {{ copy.fields.maximumConsecutiveScopeHelp }}
                </p>
                <div class="field-stack">
                  <label for="rule-maximum">{{ copy.fields.maximumConsecutive }}</label>
                  <p id="rule-maximum-help" class="field-help">
                    {{ copy.fields.maximumConsecutiveHelp }}
                  </p>
                  <input
                    id="rule-maximum"
                    type="text"
                    inputmode="numeric"
                    required
                    :readonly="!state.editor.editing"
                    :disabled="busy || stale || state.editor.rebase !== null"
                    :value="state.editor.raw.maximum.raw"
                    :aria-invalid="Boolean(state.errors.maximum) || undefined"
                    aria-describedby="rule-maximum-help rule-maximum-error"
                    @input="
                      vm.updateRaw({
                        ...state.editor.raw,
                        maximum: parseMaximumAssignmentCountDraft(text($event)),
                      })
                    "
                  />
                  <p id="rule-maximum-error" class="text-danger" role="status">
                    {{ state.errors.maximum }}
                  </p>
                </div>
                <div class="field-stack">
                  <label for="rule-consecutive-mode">{{
                    copy.fields.maximumConsecutiveMode
                  }}</label>
                  <p id="rule-consecutive-mode-help" class="field-help">
                    {{ copy.fields.maximumConsecutiveModeHelp }}
                  </p>
                  <select
                    id="rule-consecutive-mode"
                    :value="maximumConsecutiveModeKind()"
                    :disabled="
                      busy || stale || !state.editor.editing || state.editor.rebase !== null
                    "
                    aria-describedby="rule-consecutive-mode-help rule-consecutive-mode-error"
                    :aria-invalid="Boolean(state.errors.mode) || undefined"
                    @change="updateMaximumConsecutiveMode(text($event))"
                  >
                    <option value="workedDays">{{ copy.fields.workedDays }}</option>
                    <option value="assignments">{{ copy.fields.assignments }}</option>
                  </select>
                  <p id="rule-consecutive-mode-error" class="text-danger" role="status">
                    {{ state.errors.mode }}
                  </p>
                </div>
                <p v-if="maximumConsecutiveModeKind() === 'workedDays'" class="field-help">
                  {{ copy.fields.workedDaysHelp }}
                </p>
                <div v-else class="field-stack">
                  <label for="rule-consecutive-break-minutes">
                    {{ copy.fields.breakMinutes }}
                  </label>
                  <p id="rule-consecutive-break-minutes-help" class="field-help">
                    {{ copy.fields.breakMinutesHelp }}
                  </p>
                  <input
                    id="rule-consecutive-break-minutes"
                    type="text"
                    inputmode="numeric"
                    required
                    :readonly="!state.editor.editing"
                    :disabled="busy || stale || state.editor.rebase !== null"
                    :value="maximumConsecutiveBreakMinutesRaw()"
                    :aria-invalid="Boolean(state.errors['mode.breakMinutes']) || undefined"
                    aria-describedby="rule-consecutive-break-minutes-help rule-consecutive-break-minutes-error"
                    @input="updateMaximumConsecutiveBreakMinutes(text($event))"
                  />
                  <p id="rule-consecutive-break-minutes-error" class="text-danger" role="status">
                    {{ state.errors["mode.breakMinutes"] }}
                  </p>
                  <p class="field-help">{{ copy.fields.assignmentsHelp }}</p>
                </div>
              </fieldset>
            </template>
            <template v-if="state.editor.kind === 'maximumHours'">
              <WorkforceEntityPicker
                id="rule-workload-bucket"
                :project="project"
                :library-epoch="home.state.libraryEpoch"
                kind="workloadBucket"
                :label="copy.fields.workloadBucket"
                :description="copy.fields.workloadBucketHelp"
                :model-value="state.editor.raw.bucketId === '' ? [] : [state.editor.raw.bucketId]"
                required
                :disabled="busy || stale || state.editor.rebase !== null"
                :read-only="!state.editor.editing"
                v-bind="{
                  ...(locale === undefined ? {} : { locale }),
                  ...(state.errors.bucketId === undefined ? {} : { error: state.errors.bucketId }),
                }"
                @update:model-value="
                  vm.updateRaw({ ...state.editor.raw, bucketId: $event[0] ?? '' })
                "
              />
              <p v-if="workloadBucketCount === 0" role="status">
                {{ copy.fields.noWorkloadBuckets }}
              </p>
              <fieldset id="rule-window" tabindex="-1" class="field-stack">
                <legend>{{ copy.fields.window }}</legend>
                <p id="rule-window-help" class="field-help">{{ copy.fields.windowHelp }}</p>
                <div class="field-stack">
                  <label for="rule-window-kind">{{ copy.fields.windowKind }}</label>
                  <select
                    id="rule-window-kind"
                    :value="state.editor.raw.window.kind"
                    :disabled="
                      busy || stale || !state.editor.editing || state.editor.rebase !== null
                    "
                    aria-describedby="rule-window-help rule-window-kind-error"
                    :aria-invalid="Boolean(state.errors['window.kind']) || undefined"
                    @change="updateMaximumHoursWindowKind(text($event))"
                  >
                    <option value="calendar">{{ copy.fields.calendarWindow }}</option>
                    <option value="rolling">{{ copy.fields.rollingWindow }}</option>
                  </select>
                  <p id="rule-window-kind-error" class="text-danger" role="status">
                    {{ state.errors["window.kind"] }}
                  </p>
                </div>
                <div class="field-stack">
                  <label for="rule-window-membership">{{ copy.fields.membership }}</label>
                  <select
                    id="rule-window-membership"
                    :value="state.editor.raw.window.membership"
                    :disabled="
                      busy || stale || !state.editor.editing || state.editor.rebase !== null
                    "
                    aria-describedby="rule-window-help rule-window-membership-error"
                    :aria-invalid="Boolean(state.errors['window.membership']) || undefined"
                    @change="updateMaximumHoursMembership(text($event))"
                  >
                    <option value="reportingDate">
                      {{ copy.fields.reportingDateMembership }}
                    </option>
                    <option value="startInstant">
                      {{ copy.fields.startInstantMembership }}
                    </option>
                    <option value="intersection">
                      {{ copy.fields.intersectionMembership }}
                    </option>
                  </select>
                  <p id="rule-window-membership-error" class="text-danger" role="status">
                    {{ state.errors["window.membership"] }}
                  </p>
                </div>
                <template v-if="state.editor.raw.window.kind === 'calendar'">
                  <WorkforceEntityPicker
                    id="rule-calendar"
                    :project="project"
                    :library-epoch="home.state.libraryEpoch"
                    kind="calendar"
                    :label="copy.fields.calendar"
                    :description="copy.fields.calendarHelp"
                    :model-value="
                      state.editor.raw.window.calendarId === ''
                        ? []
                        : [state.editor.raw.window.calendarId]
                    "
                    required
                    :disabled="busy || stale || state.editor.rebase !== null"
                    :read-only="!state.editor.editing"
                    v-bind="{
                      ...(locale === undefined ? {} : { locale }),
                      ...(state.errors['window.calendarId'] === undefined
                        ? {}
                        : { error: state.errors['window.calendarId'] }),
                    }"
                    @update:model-value="
                      vm.updateRaw({
                        ...state.editor.raw,
                        window: { ...state.editor.raw.window, calendarId: $event[0] ?? '' },
                      })
                    "
                  />
                  <p v-if="calendarCount === 0" role="status">{{ copy.noCalendars }}</p>
                  <RouterLink
                    v-if="calendarCount === 0"
                    :to="{ name: 'project-work', params: { scenarioId: project.scenarioId } }"
                  >
                    {{ copy.manageCalendars }}
                  </RouterLink>
                </template>
                <div v-else class="field-stack">
                  <label for="rule-window-duration">{{ copy.fields.rollingDuration }}</label>
                  <p id="rule-window-duration-help" class="field-help">
                    {{ copy.fields.rollingDurationHelp }}
                  </p>
                  <input
                    id="rule-window-duration"
                    type="text"
                    inputmode="numeric"
                    required
                    :readonly="!state.editor.editing"
                    :disabled="busy || stale || state.editor.rebase !== null"
                    :value="state.editor.raw.window.durationMinutes.raw"
                    :aria-invalid="Boolean(state.errors['window.durationMinutes']) || undefined"
                    aria-describedby="rule-window-duration-help rule-window-duration-error"
                    @input="
                      vm.updateRaw({
                        ...state.editor.raw,
                        window: {
                          ...state.editor.raw.window,
                          durationMinutes: parseMaximumHoursDurationDraft(text($event)),
                        },
                      })
                    "
                  />
                  <p id="rule-window-duration-error" class="text-danger" role="status">
                    {{ state.errors["window.durationMinutes"] }}
                  </p>
                </div>
              </fieldset>
              <div class="field-stack">
                <label for="rule-maximum-minutes">{{ copy.fields.maximumMinutes }}</label>
                <p id="rule-maximum-minutes-help" class="field-help">
                  {{ copy.fields.maximumMinutesHelp }}
                </p>
                <input
                  id="rule-maximum-minutes"
                  type="text"
                  inputmode="numeric"
                  required
                  :readonly="!state.editor.editing"
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :value="state.editor.raw.maximumMinutes.raw"
                  :aria-invalid="Boolean(state.errors.maximumMinutes) || undefined"
                  aria-describedby="rule-maximum-minutes-help rule-maximum-minutes-error"
                  @input="
                    vm.updateRaw({
                      ...state.editor.raw,
                      maximumMinutes: parseMaximumHoursMaximumMinutesDraft(text($event)),
                    })
                  "
                />
                <p id="rule-maximum-minutes-error" class="text-danger" role="status">
                  {{ state.errors.maximumMinutes }}
                </p>
              </div>
            </template>
            <fieldset
              v-if="state.editor.kind === 'requiredSkillMix'"
              id="rule-qualification-minimums"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ copy.fields.qualificationMinimums }}</legend>
              <p class="field-help">{{ copy.fields.qualificationMinimumHelp }}</p>
              <p v-if="state.editor.raw.qualificationMinimums.length === 0" role="status">
                {{
                  state.editor.raw.active
                    ? copy.fields.requiredSkillMixEmpty
                    : copy.fields.requiredSkillMixInactiveEmpty
                }}
              </p>
              <p class="text-danger" role="alert">
                {{ state.errors.qualificationMinimums }}
              </p>
              <fieldset
                v-for="(row, index) in skillMixRows"
                :id="qualificationMinimumId(row.key, 'group')"
                :key="row.key"
                :data-row-key="row.key"
                tabindex="-1"
                class="field-stack"
              >
                <legend>
                  {{
                    copy.fields.qualificationMinimumNumber(
                      formatNumber(skillMixOffset + index + 1, locale),
                    )
                  }}
                </legend>
                <label :for="qualificationMinimumId(row.key, 'minimum')">
                  {{ copy.fields.qualificationMinimum }}
                </label>
                <input
                  :id="qualificationMinimumId(row.key, 'minimum')"
                  type="text"
                  inputmode="numeric"
                  required
                  :readonly="!state.editor.editing"
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :value="row.minimum"
                  :aria-invalid="
                    Boolean(
                      state.errors[`qualificationMinimums.${skillMixOffset + index}.minimum`],
                    ) || undefined
                  "
                  :aria-describedby="`rule-qualification-minimum-${row.key}-minimum-error`"
                  @input="updateQualificationMinimum(row.key, 'minimum', text($event))"
                />
                <p
                  :id="`rule-qualification-minimum-${row.key}-minimum-error`"
                  class="text-danger"
                  role="status"
                >
                  {{ state.errors[`qualificationMinimums.${skillMixOffset + index}.minimum`] }}
                </p>
                <WorkforceEntityPicker
                  :id="qualificationMinimumId(row.key, 'all')"
                  :project="project"
                  :library-epoch="home.state.libraryEpoch"
                  kind="qualification"
                  multiple
                  :label="copy.fields.allQualifications"
                  :model-value="row.allQualificationIds"
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :read-only="!state.editor.editing"
                  :error="
                    state.errors[
                      `qualificationMinimums.${skillMixOffset + index}.qualifications.allQualificationIds`
                    ]
                  "
                  v-bind="{
                    ...(locale === undefined ? {} : { locale }),
                  }"
                  @update:model-value="
                    updateQualificationMinimum(row.key, 'allQualificationIds', $event)
                  "
                />
                <WorkforceEntityPicker
                  :id="qualificationMinimumId(row.key, 'any')"
                  :project="project"
                  :library-epoch="home.state.libraryEpoch"
                  kind="qualification"
                  multiple
                  :label="copy.fields.anyQualifications"
                  :model-value="row.anyQualificationIds"
                  :disabled="busy || stale || state.editor.rebase !== null"
                  :read-only="!state.editor.editing"
                  :error="
                    state.errors[
                      `qualificationMinimums.${skillMixOffset + index}.qualifications.anyQualificationIds`
                    ]
                  "
                  v-bind="{
                    ...(locale === undefined ? {} : { locale }),
                  }"
                  @update:model-value="
                    updateQualificationMinimum(row.key, 'anyQualificationIds', $event)
                  "
                />
                <button
                  v-if="state.editor.editing"
                  type="button"
                  class="button-secondary"
                  :disabled="
                    locked ||
                    (state.editor.base === null &&
                      state.editor.raw.qualificationMinimums.length === 1)
                  "
                  :aria-label="
                    copy.fields.removeQualificationMinimumNumber(
                      formatNumber(skillMixOffset + index + 1, locale),
                    )
                  "
                  @click="removeQualificationMinimum(skillMixOffset + index)"
                >
                  {{ copy.fields.removeQualificationMinimum }}
                  {{ formatNumber(skillMixOffset + index + 1, locale) }}
                </button>
              </fieldset>
              <nav
                v-if="state.editor.raw.qualificationMinimums.length > 0"
                class="action-row"
                :aria-label="copy.fields.qualificationMinimums"
              >
                <span role="status">{{
                  copy.fields.qualificationMinimumPageStatus(
                    formatNumber(state.editor.raw.qualificationMinimums.length, locale),
                    formatNumber(skillMixOffset / 50 + 1, locale),
                  )
                }}</span>
                <button
                  type="button"
                  class="button-secondary"
                  :disabled="skillMixPage === 0"
                  @click="skillMixPage = 0"
                >
                  {{ recordsCopy.first }}
                </button>
                <button
                  type="button"
                  class="button-secondary"
                  :disabled="skillMixPage === 0"
                  @click="skillMixPage -= 1"
                >
                  {{ recordsCopy.previous }}
                </button>
                <button
                  type="button"
                  class="button-secondary"
                  :disabled="
                    (skillMixPage + 1) * 50 >= state.editor.raw.qualificationMinimums.length
                  "
                  @click="skillMixPage += 1"
                >
                  {{ recordsCopy.next }}
                </button>
              </nav>
              <button
                v-if="state.editor.editing"
                type="button"
                class="button-secondary"
                data-add-qualification-minimum
                :disabled="locked"
                @click="addQualificationMinimum"
              >
                {{ copy.fields.addQualificationMinimum }}
              </button>
            </fieldset>
            <DurationField
              v-if="state.editor.kind === 'minimumRest'"
              id="rule-minimum-rest"
              :label="copy.fields.minimumMinutes"
              :description="copy.fields.minimumRestHelp"
              :model-value="state.editor.raw.minimumRest"
              :minimum="0"
              required
              :disabled="busy || state.editor.rebase !== null"
              :read-only="!state.editor.editing"
              v-bind="{
                ...(locale === undefined ? {} : { locale }),
                ...(state.errors.minimumMinutes === undefined
                  ? {}
                  : { error: state.errors.minimumMinutes }),
              }"
              @update:model-value="vm.updateRaw({ ...state.editor.raw, minimumRest: $event })"
            />
            <fieldset
              v-if="state.editor.kind === 'noOverlap'"
              id="rule-category-pairs"
              tabindex="-1"
              class="field-stack"
            >
              <legend>{{ copy.fields.compatibleCategoryPairs }}</legend>
              <p>{{ copy.fields.categoryPairHelp }}</p>
              <ul class="field-stack">
                <li
                  v-for="(pair, row) in pairRows"
                  :key="pairKeys[pairOffset + row]"
                  class="field-stack"
                >
                  <label :for="pairId(pairOffset + row, 'firstCategory')"
                    >{{ copy.fields.firstCategory }} {{ pairOffset + row + 1 }}</label
                  ><input
                    :id="pairId(pairOffset + row, 'firstCategory')"
                    :value="pair.firstCategory"
                    :readonly="!state.editor.editing"
                    :disabled="busy || state.editor.rebase !== null"
                    @input="updatePair(pairOffset + row, 'firstCategory', text($event))"
                  /><label :for="pairId(pairOffset + row, 'secondCategory')"
                    >{{ copy.fields.secondCategory }} {{ pairOffset + row + 1 }}</label
                  ><input
                    :id="pairId(pairOffset + row, 'secondCategory')"
                    :value="pair.secondCategory"
                    :readonly="!state.editor.editing"
                    :disabled="busy || state.editor.rebase !== null"
                    @input="updatePair(pairOffset + row, 'secondCategory', text($event))"
                  /><button
                    v-if="state.editor.editing"
                    type="button"
                    :disabled="locked"
                    @click="removePair(pairOffset + row)"
                  >
                    {{ copy.fields.removePair }} {{ pairOffset + row + 1 }}
                  </button>
                </li>
              </ul>
              <nav class="action-row" :aria-label="copy.fields.compatibleCategoryPairs">
                <button type="button" :disabled="pairPage === 0" @click="pairPage -= 1">
                  {{ recordsCopy.previous }}</button
                ><button
                  type="button"
                  :disabled="pairOffset + 50 >= state.editor.raw.compatibleCategoryPairs.length"
                  @click="pairPage += 1"
                >
                  {{ recordsCopy.next }}</button
                ><button
                  v-if="state.editor.editing"
                  type="button"
                  :disabled="locked"
                  @click="addPair"
                >
                  {{ copy.fields.addPair }}
                </button>
              </nav>
            </fieldset>
            <button
              v-if="state.editor.editing"
              type="submit"
              :disabled="busy || stale || state.editor.rebase !== null"
            >
              {{ copy.preview }}
            </button>
          </form>
        </template>
        <template v-else-if="state.detail"
          ><p>{{ copy.notImplementedHint }}</p>
          <pre class="wrap-anywhere whitespace-pre-wrap">{{ nativeValue(state.detail) }}</pre>
          <button type="button" @click="closeEditor">{{ copy.close }}</button></template
        >
      </section>
      <p v-if="review.state.error" role="alert">{{ review.state.error }}</p>
      <section
        v-if="review.state.review"
        class="state-panel field-stack"
        aria-labelledby="rule-review-heading"
      >
        <h3 id="rule-review-heading" ref="reviewHeading" tabindex="-1">
          {{ copy.previewHeading }}
        </h3>
        <p>
          {{ recordsCopy.reviewRevision }}
          {{ formatNumber(review.state.review.snapshot.revision, locale) }}
        </p>
        <p class="break-all">
          {{ recordsCopy.commandIdentity }} {{ review.state.review.snapshot.commandId }}
        </p>
        <p>{{ recordsCopy.localActorHelp }}</p>
        <p>
          {{ recordsCopy.changeCount }}
          {{ formatNumber(review.state.review.changes.totalItems, locale) }}
        </p>
        <ul class="field-stack">
          <li v-for="item in review.state.review.changes.items" :key="item.ordinal">
            <p>
              {{ recordsCopy.changeKinds[item.change.kind] }} · <code>{{ item.change.path }}</code>
            </p>
            <details>
              <summary>{{ copy.currentValue }}</summary>
              <pre class="wrap-anywhere whitespace-pre-wrap">{{
                nativeValue(item.change.before)
              }}</pre>
            </details>
            <details>
              <summary>{{ copy.proposedValue }}</summary>
              <pre class="wrap-anywhere whitespace-pre-wrap">{{
                nativeValue(item.change.after)
              }}</pre>
            </details>
          </li>
        </ul>
        <nav class="action-row" :aria-label="recordsCopy.changePages">
          <button
            type="button"
            :disabled="busy || review.state.redoRequired"
            @click="review.page(true)"
          >
            {{ recordsCopy.first }}</button
          ><button
            type="button"
            :disabled="
              busy || review.state.redoRequired || review.state.review.changes.continuation === null
            "
            @click="review.page()"
          >
            {{ recordsCopy.next }}
          </button>
        </nav>
        <template v-if="state.reviewed && state.reviewed.snapshot === review.state.review.snapshot">
          <p>
            {{ copy.reviewActions[state.reviewed.action] }} ·
            {{ state.reviewed.after?.id ?? state.reviewed.before?.id }}
          </p>
          <p>
            {{ copy.currentValue }}
            {{
              state.reviewed.before === null
                ? copy.absent
                : state.reviewed.before.active
                  ? copy.active
                  : copy.inactive
            }}
            → {{ copy.proposedValue }}
            {{
              state.reviewed.after === null
                ? copy.absent
                : state.reviewed.after.active
                  ? copy.active
                  : copy.inactive
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'minimumRest'">
            {{
              copy.minimumRestSentence(formatNumber(state.reviewed.after.minimumMinutes, locale))
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'maximumAssignmentCount'">
            {{
              copy.maximumAssignmentCountSentence(
                formatNumber(state.reviewed.after.maximum, locale),
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'maximumWorkedWeekends'">
            {{
              copy.maximumWorkedWeekendsSentence(formatNumber(state.reviewed.after.maximum, locale))
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'workloadQuota'">
            {{
              copy.workloadQuotaSentence(
                formatNumber(state.reviewed.after.minimum, locale),
                state.reviewed.after.maximum === undefined
                  ? null
                  : formatNumber(state.reviewed.after.maximum, locale),
                state.reviewed.after.measure.kind === "assignmentCount"
                  ? copy.fields.quotaAssignments
                  : copy.fields.quotaMinutes,
                state.reviewed.after.membership === "startInstant"
                  ? copy.fields.startInstantMembership
                  : state.reviewed.after.membership === "intersection"
                    ? copy.fields.intersectionMembership
                    : copy.fields.reportingDateMembership,
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'maximumConsecutive'">
            {{
              copy.maximumConsecutiveSentence(
                formatNumber(state.reviewed.after.maximum, locale),
                state.reviewed.after.mode.kind,
                state.reviewed.after.mode.kind === "assignments"
                  ? formatNumber(state.reviewed.after.mode.breakMinutes, locale)
                  : "0",
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'maximumHours'">
            {{
              copy.maximumHoursSentence(
                formatNumber(state.reviewed.after.maximumMinutes, locale),
                maximumHoursWindowLabel(
                  state.reviewed.after.window.kind,
                  state.reviewed.after.window.membership,
                ),
              )
            }}
          </p>
          <template v-if="state.reviewed.after?.kind === 'requiredSkillMix'">
            <p>
              {{
                copy.requiredSkillMixSentence(
                  formatNumber(state.reviewed.after.qualificationMinimums.length, locale),
                )
              }}
            </p>
            <ul class="field-stack">
              <li
                v-for="(minimum, index) in state.reviewed.after.qualificationMinimums"
                :key="index"
              >
                {{
                  copy.requiredSkillMixMinimumSentence(
                    formatNumber(minimum.minimum, locale),
                    qualificationIds(minimum.qualifications.allQualificationIds),
                    qualificationIds(minimum.qualifications.anyQualificationIds),
                  )
                }}
              </li>
            </ul>
          </template>
          <p v-if="state.reviewed.after?.kind === 'fixedAssignment'">
            {{
              copy.fixedAssignmentSentence(
                state.reviewed.after.personId || copy.absent,
                state.reviewed.after.shiftId || copy.absent,
                state.reviewed.after.assigned ?? true,
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'mutualAssignmentRestriction'">
            {{
              copy.mutualAssignmentRestrictionSentence(
                formatNumber(state.reviewed.after.personIds.length, locale),
                state.reviewed.after.mode,
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'transitionTime'">
            {{
              copy.transitionTimeSentence(
                formatNumber(state.reviewed.after.locationIds.length, locale),
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'adjacency'">
            {{
              copy.preferenceAdjacencySentence(
                state.reviewed.after.direction === "avoid" ? copy.fields.avoid : copy.fields.prefer,
                state.reviewed.after.baseScheduleId,
                formatNumber(state.reviewed.after.maximumGapMinutes, locale),
                state.reviewed.after.mode,
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'baseStability'">
            {{ copy.preferenceBaseStabilitySentence(state.reviewed.after.baseScheduleId) }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'togetherSeparate'">
            {{
              copy.preferenceTogetherSeparateSentence(
                state.reviewed.after.direction === "together"
                  ? copy.fields.together
                  : copy.fields.separate,
                state.reviewed.after.personIds.join(", "),
                state.reviewed.after.mode,
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'preferredCoverage'">
            {{
              copy.preferencePreferredCoverageSentence(
                state.reviewed.after.coverageRequirementIds.join(", "),
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'time'">
            {{
              copy.preferenceTimeSentence(
                state.reviewed.after.direction === "avoid" ? copy.fields.avoid : copy.fields.prefer,
                preferenceTimeWindowLabel(state.reviewed.after.timeWindow),
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'assignmentType'">
            {{
              copy.preferenceAssignmentTypeSentence(
                state.reviewed.after.direction === "avoid" ? copy.fields.avoid : copy.fields.prefer,
                formatNumber(state.reviewed.after.assignmentTypeIds.length, locale),
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'consecutiveWork'">
            {{
              copy.preferenceConsecutiveWorkSentence(
                formatNumber(state.reviewed.after.threshold, locale),
                state.reviewed.after.mode.kind,
                formatNumber(state.reviewed.after.assignmentTypeIds.length, locale),
                state.reviewed.after.mode.kind === "weekends"
                  ? state.reviewed.after.mode.calendarId
                  : "",
                consecutiveWorkWeekdayLabel(state.reviewed.after.mode),
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'location'">
            {{
              copy.preferenceLocationSentence(
                state.reviewed.after.direction === "avoid" ? copy.fields.avoid : copy.fields.prefer,
                formatNumber(state.reviewed.after.locationIds.length, locale),
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'requestedTimeOff'">
            {{
              copy.preferenceRequestedTimeOffSentence(
                formatNumber(state.reviewed.after.availabilityIds.length, locale),
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'assignmentTarget'">
            {{
              copy.preferenceAssignmentTargetSentence(
                formatNumber(state.reviewed.after.targetCount, locale),
                state.reviewed.after.calendarId,
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'weekendTarget'">
            {{
              copy.preferenceWeekendTargetSentence(
                formatNumber(state.reviewed.after.targetCount, locale),
              )
            }}
          </p>
          <p v-if="state.reviewed.after?.kind === 'workloadBalance'">
            {{ workloadPolicySentence(state.reviewed.after.workloadPolicyId) }}
          </p>
          <p
            v-if="
              state.reviewed.after?.kind === 'maximumHours' ||
              (state.reviewed.after?.kind === 'workloadQuota' &&
                state.reviewed.after.measure.kind === 'bucketMinutes')
            "
          >
            {{ copy.maximumHoursPreviewHelp }}
          </p>
          <p v-else>{{ copy.previewHelp }}</p>
          <dl v-if="state.reviewed.summary">
            <div>
              <dt>{{ copy.previewPeople }}</dt>
              <dd>{{ formatNumber(state.reviewed.summary.population.peopleCount, locale) }}</dd>
            </div>
            <template v-if="state.reviewed.summary.population.kind === 'minimumRest'"
              ><div>
                <dt>{{ copy.previewBeforeShifts }}</dt>
                <dd>
                  {{ formatNumber(state.reviewed.summary.population.beforeShiftCount, locale) }}
                </dd>
              </div>
              <div>
                <dt>{{ copy.previewAfterShifts }}</dt>
                <dd>
                  {{ formatNumber(state.reviewed.summary.population.afterShiftCount, locale) }}
                </dd>
              </div></template
            >
            <div v-else>
              <dt>{{ copy.previewShifts }}</dt>
              <dd>{{ formatNumber(state.reviewed.summary.population.shiftCount, locale) }}</dd>
            </div>
          </dl>
          <p v-if="state.reviewed.scopeError" role="alert">{{ state.reviewed.scopeError }}</p>
          <p v-if="!canApply && !busy && state.reviewed.after?.active" role="status">
            {{
              state.reviewed.after?.kind === "workloadQuota"
                ? copy.quotaScopeRequired
                : state.reviewed.after?.kind === "maximumWorkedWeekends"
                  ? copy.maximumWorkedWeekendsScopeRequired
                  : state.reviewed.after?.kind === "weekendTarget"
                    ? copy.weekendTargetScopeRequired
                    : state.reviewed.after?.kind === "fixedAssignment"
                      ? copy.fixedAssignmentScopeRequired
                      : state.reviewed.after?.kind === "mutualAssignmentRestriction" &&
                          state.reviewed.mutualScopeProof === null
                        ? copy.mutualAssignmentRestrictionScopeRequired
                        : state.reviewed.after?.kind === "mutualAssignmentRestriction" &&
                            state.reviewed.scopeError ===
                              copy.mutualAssignmentRestrictionPairRequired
                          ? copy.mutualAssignmentRestrictionPairRequired
                          : state.reviewed.after?.kind === "transitionTime" &&
                              state.reviewed.transitionScopeProof === null
                            ? copy.transitionTimeScopeRequired
                            : state.reviewed.after?.kind === "transitionTime" &&
                                state.reviewed.scopeError === copy.transitionTimePairRequired
                              ? copy.transitionTimePairRequired
                              : copy.activeScopeRequired
            }}
          </p>
        </template>
        <section v-if="warnings.length" :aria-label="recordsCopy.warnings">
          <h4>{{ recordsCopy.warnings }}</h4>
          <ul>
            <li
              v-for="(warning, index) in warnings.slice(warningPage * 50, (warningPage + 1) * 50)"
              :key="index"
            >
              {{ warning.message }}
            </li>
          </ul>
          <button type="button" :disabled="warningPage === 0" @click="warningPage -= 1">
            {{ recordsCopy.previous }}</button
          ><button
            type="button"
            :disabled="(warningPage + 1) * 50 >= warnings.length"
            @click="warningPage += 1"
          >
            {{ recordsCopy.next }}
          </button>
        </section>
        <p v-if="review.state.redoRequired" role="alert">{{ copy.redoWarning }}</p>
        <button
          v-if="review.state.redoRequired"
          type="button"
          :disabled="!canApply"
          @click="save(true)"
        >
          {{ copy.confirmRedo }}
        </button>
        <button v-else type="button" :disabled="!canApply" @click="save()">{{ copy.save }}</button>
      </section>
    </template>
  </section>
</template>
