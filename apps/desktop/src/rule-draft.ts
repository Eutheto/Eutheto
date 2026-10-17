import type {
  WorkforceAdjacencyMode,
  WorkforceConsecutiveMode,
  WorkforceConsecutiveWorkMode,
  WorkforceGroupDirection,
  WorkforcePairingMode,
  WorkforcePreference,
  WorkforcePreferenceDirection,
  WorkforcePreferencePriority,
  WorkforceQualificationMinimum,
  WorkforceRequiredSkillMixRule,
  WorkforceRule,
  WorkforceScope,
  WorkforceTimeWindow,
  WorkforceWeekday,
  WorkforceWindowMembership,
  WorkforceWorkWindow,
} from "./api/generated-domain-pack-contracts";
import type { DurationDraft } from "./components/planner/field-contracts";
import { parseDurationDraft } from "./components/planner/duration-field";
import { sameField } from "./entity-draft";
import { messages } from "./messages";

export type QualificationMinimumDraft = {
  readonly key: string;
  readonly minimum: string;
  readonly allQualificationIds: readonly string[];
  readonly anyQualificationIds: readonly string[];
};

export type RequiredSkillMixMinimumDraft = {
  readonly raw: string;
} & (
  | { readonly status: "empty" }
  | { readonly status: "invalid"; readonly error: "syntax" | "range" }
  | { readonly status: "valid"; readonly minimum: number }
);

const requiredSkillMixMinimumLimit = 65_535n;
let qualificationMinimumSequence = 0;
function qualificationMinimumKey(index: number): string {
  return `qualification-minimum-${String(index)}`;
}
function qualificationMinimumNewKey(): string {
  qualificationMinimumSequence += 1;
  return `qualification-minimum-rebased-${String(qualificationMinimumSequence)}`;
}

/** Keep the editable text separate from its native u16 meaning. */
export function parseRequiredSkillMixMinimumDraft(raw: string): RequiredSkillMixMinimumDraft {
  if (raw === "") return { raw, status: "empty" };
  if (raw.length > 32) return { raw, status: "invalid", error: "range" };
  if (!/^[0-9]+$/u.test(raw)) return { raw, status: "invalid", error: "syntax" };
  const normalized = raw.replace(/^0+/u, "") || "0";
  if (normalized.length > 5) return { raw, status: "invalid", error: "range" };
  const minimum = BigInt(normalized);
  if (minimum > requiredSkillMixMinimumLimit) return { raw, status: "invalid", error: "range" };
  return { raw, status: "valid", minimum: Number(minimum) };
}

export type EditablePreference = Extract<
  WorkforcePreference,
  {
    readonly kind:
      | "time"
      | "assignmentType"
      | "location"
      | "requestedTimeOff"
      | "assignmentTarget"
      | "weekendTarget"
      | "workloadBalance"
      | "consecutiveWork"
      | "adjacency"
      | "baseStability"
      | "togetherSeparate"
      | "preferredCoverage";
  }
>;
export type EditableRule =
  | Extract<
      WorkforceRule,
      {
        readonly kind:
          | "eligibility"
          | "availability"
          | "coverage"
          | "noOverlap"
          | "minimumRest"
          | "maximumConsecutive"
          | "maximumHours"
          | "maximumWorkedWeekends"
          | "maximumAssignmentCount"
          | "workloadQuota"
          | "requiredSkillMix"
          | "fixedAssignment"
          | "mutualAssignmentRestriction"
          | "transitionTime";
      }
    >
  | EditablePreference;
export type EditableRuleKind = EditableRule["kind"];
function isPreferenceDirection(
  value: WorkforcePreferenceDirection | WorkforceGroupDirection,
): value is WorkforcePreferenceDirection {
  return value === "prefer" || value === "avoid";
}

export type MaximumAssignmentCountDraft = {
  readonly raw: string;
} & (
  | { readonly status: "empty" }
  | { readonly status: "invalid"; readonly error: "syntax" | "range" }
  | { readonly status: "valid"; readonly maximum: number }
);

const maximumAssignmentCountLimit = 4_294_967_295n;

/** Keep the editable text separate from its native u32 meaning. */
export function parseMaximumAssignmentCountDraft(raw: string): MaximumAssignmentCountDraft {
  if (raw === "") return { raw, status: "empty" };
  if (raw.length > 32) return { raw, status: "invalid", error: "range" };
  if (!/^[0-9]+$/u.test(raw)) return { raw, status: "invalid", error: "syntax" };
  const normalized = raw.replace(/^0+/u, "") || "0";
  if (normalized.length > 10) return { raw, status: "invalid", error: "range" };
  const maximum = BigInt(normalized);
  if (maximum > maximumAssignmentCountLimit) return { raw, status: "invalid", error: "range" };
  return { raw, status: "valid", maximum: Number(maximum) };
}
export type MaximumConsecutiveModeDraft =
  | { readonly kind: "workedDays"; readonly breakMinutes?: MaximumAssignmentCountDraft }
  | { readonly kind: "assignments"; readonly breakMinutes: MaximumAssignmentCountDraft };

function maximumConsecutiveModeDraft(value: WorkforceConsecutiveMode): MaximumConsecutiveModeDraft {
  return value.kind === "workedDays"
    ? { kind: "workedDays" }
    : {
        kind: "assignments",
        breakMinutes: parseMaximumAssignmentCountDraft(String(value.breakMinutes)),
      };
}
export type ConsecutiveWorkModeDraft =
  | { readonly kind: "reportingDays" }
  | {
      readonly kind: "weekends";
      readonly calendarId: string;
      readonly weekdays: readonly WorkforceWeekday[];
    };

function consecutiveWorkModeDraft(value: WorkforceConsecutiveWorkMode): ConsecutiveWorkModeDraft {
  return value.kind === "reportingDays"
    ? { kind: "reportingDays" }
    : {
        kind: "weekends",
        calendarId: value.calendarId,
        weekdays: value.weekdays,
      };
}

type UnsignedIntegerDraft = {
  readonly raw: string;
} & (
  | { readonly status: "empty" }
  | { readonly status: "invalid"; readonly error: "syntax" | "range" }
  | { readonly status: "valid"; readonly value: number }
);

const unsignedIntegerLimit = 4_294_967_295n;

function parseUnsignedIntegerDraft(raw: string, positive: boolean): UnsignedIntegerDraft {
  if (raw === "") return { raw, status: "empty" };
  if (raw.length > 32) return { raw, status: "invalid", error: "range" };
  if (!/^[0-9]+$/u.test(raw)) return { raw, status: "invalid", error: "syntax" };
  const normalized = raw.replace(/^0+/u, "") || "0";
  if (normalized.length > 10) return { raw, status: "invalid", error: "range" };
  const value = BigInt(normalized);
  if (value > unsignedIntegerLimit || (positive && value === 0n))
    return { raw, status: "invalid", error: "range" };
  return { raw, status: "valid", value: Number(value) };
}

export type MaximumHoursMaximumMinutesDraft = {
  readonly raw: string;
} & (
  | { readonly status: "empty" }
  | { readonly status: "invalid"; readonly error: "syntax" | "range" }
  | { readonly status: "valid"; readonly maximumMinutes: number }
);

/** Keep editable maximum minutes separate from its native u32 meaning. */
export function parseMaximumHoursMaximumMinutesDraft(raw: string): MaximumHoursMaximumMinutesDraft {
  const parsed = parseUnsignedIntegerDraft(raw, false);
  return parsed.status === "valid"
    ? { raw, status: "valid", maximumMinutes: parsed.value }
    : parsed;
}

export type MaximumHoursDurationDraft = {
  readonly raw: string;
} & (
  | { readonly status: "empty" }
  | { readonly status: "invalid"; readonly error: "syntax" | "range" }
  | { readonly status: "valid"; readonly durationMinutes: number }
);

/** Keep editable positive rolling duration separate from its native u32 meaning. */
export function parseMaximumHoursDurationDraft(raw: string): MaximumHoursDurationDraft {
  const parsed = parseUnsignedIntegerDraft(raw, true);
  return parsed.status === "valid"
    ? { raw, status: "valid", durationMinutes: parsed.value }
    : parsed;
}

export type MaximumHoursWindowDraft =
  | {
      readonly kind: "calendar";
      readonly calendarId: string;
      readonly membership: WorkforceWindowMembership;
    }
  | {
      readonly kind: "rolling";
      readonly durationMinutes: MaximumHoursDurationDraft;
      readonly membership: WorkforceWindowMembership;
    };

function maximumHoursWindowDraft(value: WorkforceWorkWindow): MaximumHoursWindowDraft {
  return value.kind === "calendar"
    ? { kind: value.kind, calendarId: value.calendarId, membership: value.membership }
    : {
        kind: value.kind,
        durationMinutes: parseMaximumHoursDurationDraft(String(value.durationMinutes)),
        membership: value.membership,
      };
}
export type WeeklyTimeWindowDraft = {
  readonly key: string;
  readonly weekdays: readonly WorkforceWeekday[];
  readonly startTime: string;
  readonly endTime: string;
  readonly endDayOffset: string;
};

export type TimeWindowDraft =
  | {
      readonly kind: "instant";
      readonly startsAt: string;
      readonly endsAt: string;
    }
  | {
      readonly kind: "weekly";
      readonly windows: readonly WeeklyTimeWindowDraft[];
    };

function timeWindowDraft(value: WorkforceTimeWindow): TimeWindowDraft {
  return value.kind === "instant"
    ? { kind: value.kind, startsAt: value.startsAt, endsAt: value.endsAt }
    : {
        kind: value.kind,
        windows: value.windows.map((window, index) => ({
          key: `time-window-${String(index)}`,
          weekdays: window.weekdays,
          startTime: window.startTime,
          endTime: window.endTime,
          endDayOffset: String(window.endDayOffset),
        })),
      };
}

const preferenceWeightLimit = 1_000_000n;
function parsePreferenceWeightDraft(raw: string): MaximumAssignmentCountDraft {
  const parsed = parseMaximumAssignmentCountDraft(raw);
  return parsed.status === "valid" &&
    (parsed.maximum === 0 || BigInt(parsed.maximum) > preferenceWeightLimit)
    ? { raw, status: "invalid", error: "range" }
    : parsed;
}

function isPreferenceKind(kind: EditableRuleKind): kind is EditablePreference["kind"] {
  return (
    kind === "time" ||
    kind === "assignmentType" ||
    kind === "location" ||
    kind === "requestedTimeOff" ||
    kind === "assignmentTarget" ||
    kind === "weekendTarget" ||
    kind === "workloadBalance" ||
    kind === "consecutiveWork" ||
    kind === "adjacency" ||
    kind === "baseStability" ||
    kind === "togetherSeparate" ||
    kind === "preferredCoverage"
  );
}

type DirectedEditablePreference = Extract<
  EditablePreference,
  { readonly direction: WorkforcePreferenceDirection }
>;
type GroupDirectedEditablePreference = Extract<
  EditablePreference,
  { readonly direction: WorkforceGroupDirection }
>;

function isEditablePreference(value: EditableRule): value is EditablePreference {
  return isPreferenceKind(value.kind);
}

function isDirectedEditablePreference(value: EditableRule): value is DirectedEditablePreference {
  return (
    value.kind === "time" ||
    value.kind === "assignmentType" ||
    value.kind === "location" ||
    value.kind === "adjacency"
  );
}

function isGroupDirectedEditablePreference(
  value: EditableRule,
): value is GroupDirectedEditablePreference {
  return value.kind === "togetherSeparate";
}

function timeWindowValue(window: TimeWindowDraft): WorkforceTimeWindow | null {
  if (window.kind === "instant")
    return window.startsAt === "" || window.endsAt === ""
      ? null
      : { kind: "instant", startsAt: window.startsAt, endsAt: window.endsAt };
  const windows = window.windows.flatMap((row) => {
    const offset = parseMaximumAssignmentCountDraft(row.endDayOffset);
    return row.weekdays.length > 0 &&
      row.startTime !== "" &&
      row.endTime !== "" &&
      offset.status === "valid" &&
      offset.maximum <= 255
      ? [
          {
            weekdays: row.weekdays,
            startTime: row.startTime,
            endTime: row.endTime,
            endDayOffset: offset.maximum,
          },
        ]
      : [];
  });
  return windows.length === 0 || windows.length !== window.windows.length
    ? null
    : { kind: "weekly", windows };
}

export interface RuleDraft {
  readonly active: boolean;
  readonly scope: WorkforceScope;
  readonly beforeScope: WorkforceScope;
  readonly afterScope: WorkforceScope;
  readonly calendarId: string;
  readonly targetCount: MaximumAssignmentCountDraft;
  readonly bucketId: string;
  readonly maximumMinutes: MaximumHoursMaximumMinutesDraft;
  readonly window: MaximumHoursWindowDraft;
  readonly maximum: MaximumAssignmentCountDraft;
  readonly maximumGapMinutes: MaximumAssignmentCountDraft;
  readonly threshold: MaximumAssignmentCountDraft;
  readonly quotaMeasure: "assignmentCount" | "bucketMinutes";
  readonly quotaMembership: WorkforceWindowMembership;
  readonly quotaMinimum: MaximumAssignmentCountDraft;
  readonly quotaMaximum: MaximumAssignmentCountDraft;
  readonly minimumRest: DurationDraft;
  readonly compatibleCategoryPairs: readonly {
    readonly firstCategory: string;
    readonly secondCategory: string;
  }[];
  readonly personId: string;
  readonly shiftId: string;
  readonly assigned: boolean;
  readonly personIds: readonly string[];
  readonly locationIds: readonly string[];
  readonly assignmentTypeIds: readonly string[];
  readonly availabilityIds: readonly string[];
  readonly coverageRequirementIds: readonly string[];
  readonly baseScheduleId: string;
  readonly mode:
    | WorkforceAdjacencyMode
    | WorkforcePairingMode
    | MaximumConsecutiveModeDraft
    | ConsecutiveWorkModeDraft;
  readonly qualificationMinimums: readonly QualificationMinimumDraft[];
  readonly workloadPolicyId: string;
  readonly priority: WorkforcePreferencePriority;
  readonly weight: MaximumAssignmentCountDraft;
  readonly direction: WorkforcePreferenceDirection | WorkforceGroupDirection;
  readonly timeWindow: TimeWindowDraft;
}

function qualificationIdentity(
  allQualificationIds: readonly string[],
  anyQualificationIds: readonly string[],
): string {
  return JSON.stringify([[...allQualificationIds].sort(), [...anyQualificationIds].sort()]);
}

function nativeQualificationIdentity(value: WorkforceQualificationMinimum): string {
  return qualificationIdentity(
    value.qualifications.allQualificationIds,
    value.qualifications.anyQualificationIds,
  );
}

function draftQualificationIdentity(value: QualificationMinimumDraft): string {
  return qualificationIdentity(value.allQualificationIds, value.anyQualificationIds);
}

function requiredSkillMixDraft(
  value: WorkforceRequiredSkillMixRule,
  before: WorkforceRequiredSkillMixRule | undefined,
  raw: readonly QualificationMinimumDraft[] | undefined,
): readonly QualificationMinimumDraft[] {
  const values = value.qualificationMinimums;
  if (raw !== undefined && sameField(values, before?.qualificationMinimums)) return raw;
  if (before === undefined || raw === undefined || before.qualificationMinimums.length === 0)
    return values.map((row, index) => ({
      key: qualificationMinimumKey(index),
      minimum: String(row.minimum),
      allQualificationIds: row.qualifications.allQualificationIds,
      anyQualificationIds: row.qualifications.anyQualificationIds,
    }));

  const matches = new Map<string, { indices: number[]; next: number }>();
  before.qualificationMinimums.forEach((row, index) => {
    const key = nativeQualificationIdentity(row);
    const match = matches.get(key);
    if (match) match.indices.push(index);
    else matches.set(key, { indices: [index], next: 0 });
  });
  const sameLength =
    values.length === before.qualificationMinimums.length && values.length === raw.length;
  return values.map((row, index) => {
    const match = matches.get(nativeQualificationIdentity(row));
    const beforeIndex = match?.indices[match.next++];
    const beforeRow =
      beforeIndex === undefined
        ? sameLength
          ? before.qualificationMinimums[index]
          : undefined
        : before.qualificationMinimums[beforeIndex];
    const retained =
      beforeIndex === undefined ? (sameLength ? raw[index] : undefined) : raw[beforeIndex];
    const parsed =
      retained === undefined ? undefined : parseRequiredSkillMixMinimumDraft(retained.minimum);
    const minimum =
      retained !== undefined && (parsed?.status !== "valid" || beforeRow?.minimum === row.minimum)
        ? retained.minimum
        : String(row.minimum);
    return {
      key: retained?.key ?? qualificationMinimumNewKey(),
      minimum,
      allQualificationIds: row.qualifications.allQualificationIds,
      anyQualificationIds: row.qualifications.anyQualificationIds,
    };
  });
}

function qualificationMinimumsForRebase(
  base: WorkforceRequiredSkillMixRule,
  raw: readonly QualificationMinimumDraft[],
): readonly WorkforceQualificationMinimum[] {
  const used = new Set<number>();
  const matches = new Map<string, { indices: number[]; next: number }>();
  base.qualificationMinimums.forEach((value, index) => {
    const key = nativeQualificationIdentity(value);
    const match = matches.get(key);
    if (match) match.indices.push(index);
    else matches.set(key, { indices: [index], next: 0 });
  });
  return raw.map((row, index) => {
    const match = matches.get(draftQualificationIdentity(row));
    let baseIndex = -1;
    if (match) {
      while (match.next < match.indices.length) {
        const candidate = match.indices[match.next];
        if (candidate === undefined) break;
        match.next += 1;
        if (!used.has(candidate)) {
          baseIndex = candidate;
          break;
        }
      }
    }
    if (baseIndex < 0 && index < base.qualificationMinimums.length && !used.has(index))
      baseIndex = index;
    if (baseIndex >= 0) used.add(baseIndex);
    const baseRow = baseIndex >= 0 ? base.qualificationMinimums[baseIndex] : undefined;
    const parsed = parseRequiredSkillMixMinimumDraft(row.minimum);
    return {
      minimum: parsed.status === "valid" ? parsed.minimum : (baseRow?.minimum ?? 0),
      qualifications: {
        allQualificationIds: row.allQualificationIds,
        anyQualificationIds: row.anyQualificationIds,
      },
    };
  });
}

/** Unsubmitted inactive fields remain raw intent, never extra fields in the native rule. */
export function createRuleDraft(
  value: EditableRule | "new",
  retained?: { readonly value: EditableRule; readonly raw: RuleDraft },
): RuleDraft {
  const record = value === "new" ? null : value;
  const previous =
    record !== null && retained?.value.id === record.id && retained.value.kind === record.kind
      ? retained
      : undefined;
  const preference = record !== null && isEditablePreference(record) ? record : undefined;
  let previousPreference: EditablePreference | undefined;
  if (
    preference !== undefined &&
    previous !== undefined &&
    isEditablePreference(previous.value) &&
    previous.value.kind === preference.kind
  )
    previousPreference = previous.value;
  const keepDuration =
    record?.kind === "minimumRest" &&
    previous?.value.kind === "minimumRest" &&
    (previous.raw.minimumRest.status !== "valid" ||
      sameField(record.minimumMinutes, previous.value.minimumMinutes));
  const keepMaximum =
    (record?.kind === "maximumAssignmentCount" &&
      previous?.value.kind === "maximumAssignmentCount" &&
      (previous.raw.maximum.status !== "valid" ||
        sameField(record.maximum, previous.value.maximum))) ||
    (record?.kind === "maximumWorkedWeekends" &&
      previous?.value.kind === "maximumWorkedWeekends" &&
      (previous.raw.maximum.status !== "valid" ||
        sameField(record.maximum, previous.value.maximum)));
  const keepAssignmentTarget =
    previous !== undefined &&
    record !== null &&
    ((record.kind === "assignmentTarget" && previous.value.kind === "assignmentTarget") ||
      (record.kind === "weekendTarget" && previous.value.kind === "weekendTarget")) &&
    (previous.raw.targetCount.status !== "valid" ||
      sameField(record.targetCount, previous.value.targetCount));
  const keepAdjacencyMaximumGap =
    record?.kind === "adjacency" &&
    previous?.value.kind === "adjacency" &&
    (previous.raw.maximumGapMinutes.status !== "valid" ||
      sameField(record.maximumGapMinutes, previous.value.maximumGapMinutes));
  const keepAdjacencyMode =
    record?.kind === "adjacency" &&
    previous?.value.kind === "adjacency" &&
    previous.raw.mode === record.mode;
  const previousConsecutiveMode =
    record?.kind === "maximumConsecutive" &&
    previous?.value.kind === "maximumConsecutive" &&
    typeof previous.raw.mode === "object"
      ? previous.raw.mode
      : undefined;
  const previousConsecutiveWorkMode =
    record?.kind === "consecutiveWork" &&
    previous?.value.kind === "consecutiveWork" &&
    typeof previous.raw.mode === "object" &&
    (previous.raw.mode.kind === "reportingDays" || previous.raw.mode.kind === "weekends")
      ? previous.raw.mode
      : undefined;
  const keepConsecutiveWork =
    record?.kind === "consecutiveWork" &&
    previous?.value.kind === "consecutiveWork" &&
    (previous.raw.threshold.status !== "valid" ||
      sameField(record.threshold, previous.value.threshold));
  const keepConsecutiveWorkMode =
    record?.kind === "consecutiveWork" &&
    previous?.value.kind === "consecutiveWork" &&
    previousConsecutiveWorkMode !== undefined &&
    (sameField(record.mode, previous.value.mode) ||
      (previousConsecutiveWorkMode.kind === "weekends" &&
        (previousConsecutiveWorkMode.calendarId === "" ||
          previousConsecutiveWorkMode.weekdays.length === 0)));
  const keepConsecutiveWorkAssignmentTypes =
    record?.kind === "consecutiveWork" &&
    previous?.value.kind === "consecutiveWork" &&
    (previous.raw.assignmentTypeIds.length === 0 ||
      sameField(record.assignmentTypeIds, previous.value.assignmentTypeIds));
  const keepMaximumConsecutive =
    record?.kind === "maximumConsecutive" &&
    previous?.value.kind === "maximumConsecutive" &&
    (previous.raw.maximum.status !== "valid" || sameField(record.maximum, previous.value.maximum));
  const keepMaximumConsecutiveMode =
    record?.kind === "maximumConsecutive" &&
    previous?.value.kind === "maximumConsecutive" &&
    ((previousConsecutiveMode?.kind === "assignments" &&
      previousConsecutiveMode.breakMinutes.status !== "valid") ||
      sameField(record.mode, previous.value.mode));
  const keepMaximumHours =
    record?.kind === "maximumHours" &&
    previous?.value.kind === "maximumHours" &&
    (previous.raw.maximumMinutes.status !== "valid" ||
      sameField(record.maximumMinutes, previous.value.maximumMinutes));
  const keepMaximumHoursWindow =
    record?.kind === "maximumHours" &&
    previous?.value.kind === "maximumHours" &&
    ((previous.raw.window.kind === "calendar" && previous.raw.window.calendarId === "") ||
      (previous.raw.window.kind === "rolling" &&
        previous.raw.window.durationMinutes.status !== "valid") ||
      sameField(record.window, previous.value.window));
  const priorQuota =
    record?.kind === "workloadQuota" &&
    previous !== undefined &&
    previous.value.kind === "workloadQuota"
      ? { value: previous.value, raw: previous.raw }
      : undefined;
  const previousSkillMix =
    record?.kind === "requiredSkillMix" && previous?.value.kind === "requiredSkillMix"
      ? previous.value
      : undefined;
  const keepMutualPeople =
    record?.kind === "mutualAssignmentRestriction" &&
    previous?.value.kind === "mutualAssignmentRestriction" &&
    sameField(record.personIds, previous.value.personIds);
  const keepTransitionLocations =
    record?.kind === "transitionTime" &&
    previous?.value.kind === "transitionTime" &&
    sameField(record.locationIds, previous.value.locationIds);
  const keepMutualMode =
    record?.kind === "mutualAssignmentRestriction" &&
    previous?.value.kind === "mutualAssignmentRestriction" &&
    sameField(record.mode, previous.value.mode);
  const keepTogetherSeparatePeople =
    record?.kind === "togetherSeparate" &&
    previous?.value.kind === "togetherSeparate" &&
    (previous.raw.personIds.length === 0 ||
      new Set(previous.raw.personIds).size !== previous.raw.personIds.length ||
      sameField(record.personIds, previous.value.personIds));
  const keepTogetherSeparateMode =
    record?.kind === "togetherSeparate" &&
    previous?.value.kind === "togetherSeparate" &&
    previous.raw.mode === record.mode;
  const keepBaseSchedule =
    (record?.kind === "adjacency" || record?.kind === "baseStability") &&
    (previousPreference?.kind === "adjacency" || previousPreference?.kind === "baseStability") &&
    (previous?.raw.baseScheduleId === "" ||
      sameField(record.baseScheduleId, previousPreference.baseScheduleId));
  const keepPreferredCoverage =
    record?.kind === "preferredCoverage" &&
    previousPreference?.kind === "preferredCoverage" &&
    (previous?.raw.coverageRequirementIds.length === 0 ||
      sameField(record.coverageRequirementIds, previousPreference.coverageRequirementIds));
  const keepPreferenceWeight =
    preference !== undefined &&
    previous !== undefined &&
    previousPreference !== undefined &&
    (previous.raw.weight.status !== "valid" ||
      sameField(preference.weight, previousPreference.weight));
  const keepTimeWindow =
    record?.kind === "time" &&
    previous?.value.kind === "time" &&
    (sameField(record.timeWindow, previous.value.timeWindow) ||
      (previous.raw.timeWindow.kind === "instant"
        ? previous.raw.timeWindow.startsAt === "" || previous.raw.timeWindow.endsAt === ""
        : previous.raw.timeWindow.windows.length === 0 ||
          previous.raw.timeWindow.windows.some(
            (window) =>
              window.weekdays.length === 0 ||
              window.startTime === "" ||
              window.endTime === "" ||
              window.endDayOffset === "",
          )));

  return {
    active: record?.active ?? true,
    scope: record?.scope ?? { people: { kind: "all" } },
    calendarId:
      record?.kind === "maximumAssignmentCount" || record?.kind === "assignmentTarget"
        ? record.calendarId
        : record?.kind === "workloadQuota"
          ? priorQuota !== undefined &&
            (priorQuota.raw.calendarId === "" ||
              sameField(record.calendarId, priorQuota.value.calendarId))
            ? priorQuota.raw.calendarId
            : record.calendarId
          : (previous?.raw.calendarId ?? ""),
    targetCount: keepAssignmentTarget
      ? previous.raw.targetCount
      : record !== null && (record.kind === "assignmentTarget" || record.kind === "weekendTarget")
        ? parseMaximumAssignmentCountDraft(String(record.targetCount))
        : (previous?.raw.targetCount ?? parseMaximumAssignmentCountDraft("")),
    bucketId:
      record?.kind === "maximumHours"
        ? record.bucketId
        : record?.kind === "workloadQuota"
          ? record.measure.kind === "bucketMinutes"
            ? priorQuota?.value.measure.kind === "bucketMinutes" &&
              (priorQuota.raw.bucketId === "" ||
                sameField(record.measure.bucketId, priorQuota.value.measure.bucketId))
              ? priorQuota.raw.bucketId
              : record.measure.bucketId
            : (priorQuota?.raw.bucketId ?? "")
          : (previous?.raw.bucketId ?? ""),
    maximumMinutes: keepMaximumHours
      ? previous.raw.maximumMinutes
      : record?.kind === "maximumHours"
        ? parseMaximumHoursMaximumMinutesDraft(String(record.maximumMinutes))
        : (previous?.raw.maximumMinutes ?? parseMaximumHoursMaximumMinutesDraft("")),
    window: keepMaximumHoursWindow
      ? previous.raw.window
      : record?.kind === "maximumHours"
        ? maximumHoursWindowDraft(record.window)
        : (previous?.raw.window ?? {
            kind: "calendar",
            calendarId: "",
            membership: "reportingDate",
          }),
    maximum: keepMaximumConsecutive
      ? previous.raw.maximum
      : record?.kind === "maximumConsecutive"
        ? parseMaximumAssignmentCountDraft(String(record.maximum))
        : keepMaximum
          ? previous.raw.maximum
          : record?.kind === "maximumAssignmentCount" || record?.kind === "maximumWorkedWeekends"
            ? parseMaximumAssignmentCountDraft(String(record.maximum))
            : (previous?.raw.maximum ?? parseMaximumAssignmentCountDraft("")),
    maximumGapMinutes: keepAdjacencyMaximumGap
      ? previous.raw.maximumGapMinutes
      : record?.kind === "adjacency"
        ? parseMaximumAssignmentCountDraft(String(record.maximumGapMinutes))
        : (previous?.raw.maximumGapMinutes ?? parseMaximumAssignmentCountDraft("")),
    threshold: keepConsecutiveWork
      ? previous.raw.threshold
      : record?.kind === "consecutiveWork"
        ? parseMaximumAssignmentCountDraft(String(record.threshold))
        : (previous?.raw.threshold ?? parseMaximumAssignmentCountDraft("")),
    quotaMeasure:
      record?.kind === "workloadQuota"
        ? priorQuota !== undefined && sameField(record.measure, priorQuota.value.measure)
          ? priorQuota.raw.quotaMeasure
          : record.measure.kind
        : (previous?.raw.quotaMeasure ?? "assignmentCount"),
    quotaMembership:
      record?.kind === "workloadQuota"
        ? priorQuota !== undefined && sameField(record.membership, priorQuota.value.membership)
          ? priorQuota.raw.quotaMembership
          : record.membership
        : (previous?.raw.quotaMembership ?? "reportingDate"),
    quotaMinimum:
      record?.kind === "workloadQuota"
        ? priorQuota !== undefined &&
          (priorQuota.raw.quotaMinimum.status !== "valid" ||
            sameField(record.minimum, priorQuota.value.minimum))
          ? priorQuota.raw.quotaMinimum
          : parseMaximumAssignmentCountDraft(String(record.minimum))
        : (previous?.raw.quotaMinimum ?? parseMaximumAssignmentCountDraft("")),
    quotaMaximum:
      record?.kind === "workloadQuota"
        ? priorQuota !== undefined &&
          (priorQuota.raw.quotaMaximum.status === "invalid" ||
            sameField(record.maximum, priorQuota.value.maximum))
          ? priorQuota.raw.quotaMaximum
          : parseMaximumAssignmentCountDraft(
              record.maximum === undefined ? "" : String(record.maximum),
            )
        : (previous?.raw.quotaMaximum ?? parseMaximumAssignmentCountDraft("")),
    beforeScope:
      record?.kind === "minimumRest"
        ? record.beforeScope
        : (previous?.raw.beforeScope ?? { people: { kind: "all" } }),
    afterScope:
      record?.kind === "minimumRest"
        ? record.afterScope
        : (previous?.raw.afterScope ?? { people: { kind: "all" } }),
    minimumRest: keepDuration
      ? previous.raw.minimumRest
      : record?.kind === "minimumRest"
        ? parseDurationDraft(String(record.minimumMinutes), "minutes", 0)
        : (previous?.raw.minimumRest ?? parseDurationDraft("", "hours", 0)),
    compatibleCategoryPairs:
      record?.kind === "noOverlap"
        ? record.compatibleCategoryPairs
        : (previous?.raw.compatibleCategoryPairs ?? []),
    personId:
      record?.kind === "fixedAssignment"
        ? previous?.value.kind === "fixedAssignment" &&
          (previous.raw.personId === "" || sameField(record.personId, previous.value.personId))
          ? previous.raw.personId
          : record.personId
        : (previous?.raw.personId ?? ""),
    shiftId:
      record?.kind === "fixedAssignment"
        ? previous?.value.kind === "fixedAssignment" &&
          (previous.raw.shiftId === "" || sameField(record.shiftId, previous.value.shiftId))
          ? previous.raw.shiftId
          : record.shiftId
        : (previous?.raw.shiftId ?? ""),
    assigned:
      record?.kind === "fixedAssignment"
        ? previous?.value.kind === "fixedAssignment" &&
          (record.assigned ?? true) === (previous.value.assigned ?? true)
          ? previous.raw.assigned
          : (record.assigned ?? true)
        : (previous?.raw.assigned ?? true),
    personIds:
      record?.kind === "mutualAssignmentRestriction"
        ? keepMutualPeople
          ? previous.raw.personIds
          : record.personIds
        : record?.kind === "togetherSeparate"
          ? keepTogetherSeparatePeople
            ? previous.raw.personIds
            : record.personIds
          : (previous?.raw.personIds ?? []),
    locationIds:
      record?.kind === "location"
        ? record.locationIds
        : record?.kind === "transitionTime"
          ? keepTransitionLocations
            ? previous.raw.locationIds
            : record.locationIds
          : (previous?.raw.locationIds ?? []),
    assignmentTypeIds:
      record?.kind === "assignmentType"
        ? record.assignmentTypeIds
        : record?.kind === "consecutiveWork"
          ? keepConsecutiveWorkAssignmentTypes
            ? previous.raw.assignmentTypeIds
            : record.assignmentTypeIds
          : (previous?.raw.assignmentTypeIds ?? []),
    availabilityIds:
      record?.kind === "requestedTimeOff"
        ? record.availabilityIds
        : (previous?.raw.availabilityIds ?? []),
    coverageRequirementIds:
      record?.kind === "preferredCoverage"
        ? keepPreferredCoverage
          ? (previous?.raw.coverageRequirementIds ?? record.coverageRequirementIds)
          : record.coverageRequirementIds
        : (previous?.raw.coverageRequirementIds ?? []),
    baseScheduleId:
      record?.kind === "adjacency" || record?.kind === "baseStability"
        ? keepBaseSchedule
          ? (previous?.raw.baseScheduleId ?? record.baseScheduleId)
          : record.baseScheduleId
        : (previous?.raw.baseScheduleId ?? ""),
    workloadPolicyId:
      record?.kind === "workloadBalance"
        ? previous?.value.kind === "workloadBalance" &&
          (previous.raw.workloadPolicyId === "" ||
            sameField(record.workloadPolicyId, previous.value.workloadPolicyId))
          ? previous.raw.workloadPolicyId
          : record.workloadPolicyId
        : (previous?.raw.workloadPolicyId ?? ""),
    priority: preference?.priority ?? previous?.raw.priority ?? "normal",
    weight: keepPreferenceWeight
      ? previous.raw.weight
      : preference !== undefined
        ? parsePreferenceWeightDraft(String(preference.weight))
        : (previous?.raw.weight ?? parsePreferenceWeightDraft("1")),
    direction:
      record !== null && isDirectedEditablePreference(record)
        ? record.direction
        : record !== null && isGroupDirectedEditablePreference(record)
          ? record.direction
          : (previous?.raw.direction ?? "prefer"),
    timeWindow: keepTimeWindow
      ? previous.raw.timeWindow
      : record?.kind === "time"
        ? timeWindowDraft(record.timeWindow)
        : (previous?.raw.timeWindow ?? { kind: "weekly", windows: [] }),
    mode:
      record?.kind === "adjacency"
        ? keepAdjacencyMode
          ? previous.raw.mode
          : record.mode
        : record?.kind === "togetherSeparate"
          ? keepTogetherSeparateMode
            ? previous.raw.mode
            : record.mode
          : record?.kind === "maximumConsecutive"
            ? keepMaximumConsecutiveMode && previousConsecutiveMode !== undefined
              ? previousConsecutiveMode
              : maximumConsecutiveModeDraft(record.mode)
            : record?.kind === "consecutiveWork"
              ? keepConsecutiveWorkMode
                ? previousConsecutiveWorkMode
                : consecutiveWorkModeDraft(record.mode)
              : record?.kind === "mutualAssignmentRestriction"
                ? keepMutualMode
                  ? previous.raw.mode
                  : record.mode
                : (previous?.raw.mode ?? "sameShift"),
    qualificationMinimums:
      record?.kind === "requiredSkillMix"
        ? requiredSkillMixDraft(record, previousSkillMix, previous?.raw.qualificationMinimums)
        : (previous?.raw.qualificationMinimums ?? []),
  };
}

/** Rust validates scope membership, references, category tokens and rule semantics. */
export function ruleDraftValue(
  id: string,
  kind: EditableRuleKind,
  draft: RuleDraft,
  workloadPolicyIds?: readonly string[],
): { readonly value: EditableRule | null; readonly errors: Readonly<Record<string, string>> } {
  const common = { id, active: draft.active, strength: "required" as const, scope: draft.scope };
  const preferenceWeight = parsePreferenceWeightDraft(draft.weight.raw);
  const preferenceHeader =
    preferenceWeight.status === "valid"
      ? {
          id,
          active: draft.active,
          scope: draft.scope,
          priority: draft.priority,
          weight: preferenceWeight.maximum,
        }
      : null;
  switch (kind) {
    case "adjacency": {
      const maximumGapMinutes = parseMaximumAssignmentCountDraft(draft.maximumGapMinutes.raw);
      const errors: Record<string, string> = {};
      if (draft.baseScheduleId === "")
        errors.baseScheduleId = messages.ruleSetup.baseScheduleRequired;
      if (maximumGapMinutes.status !== "valid")
        errors.maximumGapMinutes = messages.ruleSetup.adjacencyGapInvalid;
      const direction =
        draft.direction === "prefer" || draft.direction === "avoid" ? draft.direction : null;
      if (direction === null) errors.direction = messages.ruleSetup.preferenceDirectionInvalid;
      const mode =
        draft.mode === "before" || draft.mode === "after" || draft.mode === "either"
          ? draft.mode
          : null;
      if (mode === null) errors.mode = messages.ruleSetup.adjacencyModeInvalid;
      if (preferenceHeader === null) errors.weight = messages.ruleSetup.preferenceWeightInvalid;
      if (
        Object.keys(errors).length > 0 ||
        preferenceHeader === null ||
        maximumGapMinutes.status !== "valid" ||
        direction === null ||
        mode === null
      )
        return { value: null, errors };
      return {
        value: {
          ...preferenceHeader,
          kind,
          baseScheduleId: draft.baseScheduleId,
          direction,
          maximumGapMinutes: maximumGapMinutes.maximum,
          mode,
        },
        errors: {},
      };
    }
    case "baseStability": {
      const errors: Record<string, string> = {};
      if (draft.baseScheduleId === "")
        errors.baseScheduleId = messages.ruleSetup.baseScheduleRequired;
      if (preferenceHeader === null) errors.weight = messages.ruleSetup.preferenceWeightInvalid;
      if (Object.keys(errors).length > 0 || preferenceHeader === null)
        return { value: null, errors };
      return {
        value: { ...preferenceHeader, kind, baseScheduleId: draft.baseScheduleId },
        errors: {},
      };
    }
    case "togetherSeparate": {
      const errors: Record<string, string> = {};
      const direction =
        draft.direction === "together" || draft.direction === "separate" ? draft.direction : null;
      if (direction === null) errors.direction = messages.ruleSetup.groupDirectionInvalid;
      const mode =
        draft.mode === "sameShift" || draft.mode === "overlappingShifts" ? draft.mode : null;
      if (mode === null) errors.mode = messages.ruleSetup.togetherSeparateModeInvalid;
      if (new Set(draft.personIds).size !== draft.personIds.length)
        errors.personIds = messages.ruleSetup.togetherSeparateDuplicatePeople;
      if (preferenceHeader === null) errors.weight = messages.ruleSetup.preferenceWeightInvalid;
      if (
        Object.keys(errors).length > 0 ||
        preferenceHeader === null ||
        direction === null ||
        mode === null
      )
        return { value: null, errors };
      return {
        value: { ...preferenceHeader, kind, direction, mode, personIds: draft.personIds },
        errors: {},
      };
    }
    case "preferredCoverage": {
      if (preferenceHeader === null)
        return { value: null, errors: { weight: messages.ruleSetup.preferenceWeightInvalid } };
      return {
        value: { ...preferenceHeader, kind, coverageRequirementIds: draft.coverageRequirementIds },
        errors: {},
      };
    }
    case "time": {
      const errors: Record<string, string> = {};
      const window = draft.timeWindow;
      let value: WorkforceTimeWindow | null = null;
      if (window.kind === "instant") {
        if (window.startsAt === "")
          errors["timeWindow.startsAt"] = messages.ruleSetup.timeStartRequired;
        if (window.endsAt === "") errors["timeWindow.endsAt"] = messages.ruleSetup.timeEndRequired;
        if (Object.keys(errors).length === 0)
          value = { kind: "instant", startsAt: window.startsAt, endsAt: window.endsAt };
      } else {
        if (window.windows.length === 0)
          errors["timeWindow.windows"] = messages.ruleSetup.timeWindowRequired;
        const windows = window.windows.flatMap((row, index) => {
          const prefix = `timeWindow.windows.${String(index)}`;
          const offset = parseMaximumAssignmentCountDraft(row.endDayOffset);
          if (row.weekdays.length === 0)
            errors[`${prefix}.weekdays`] = messages.ruleSetup.timeWeekdaysRequired;
          if (row.startTime === "")
            errors[`${prefix}.startTime`] = messages.ruleSetup.timeStartRequired;
          if (row.endTime === "") errors[`${prefix}.endTime`] = messages.ruleSetup.timeEndRequired;
          if (offset.status !== "valid" || offset.maximum > 255)
            errors[`${prefix}.endDayOffset`] = messages.ruleSetup.timeDayOffsetInvalid;
          return row.weekdays.length > 0 &&
            row.startTime !== "" &&
            row.endTime !== "" &&
            offset.status === "valid" &&
            offset.maximum <= 255
            ? [
                {
                  weekdays: row.weekdays,
                  startTime: row.startTime,
                  endTime: row.endTime,
                  endDayOffset: offset.maximum,
                },
              ]
            : [];
        });
        if (Object.keys(errors).length === 0) value = { kind: "weekly", windows };
      }
      const direction = isPreferenceDirection(draft.direction) ? draft.direction : null;
      if (direction === null) errors.direction = messages.ruleSetup.preferenceDirectionInvalid;
      if (preferenceHeader === null) errors.weight = messages.ruleSetup.preferenceWeightInvalid;
      if (preferenceHeader === null || value === null || direction === null)
        return { value: null, errors };
      return {
        value: {
          ...preferenceHeader,
          kind,
          direction,
          timeWindow: value,
        },
        errors: {},
      };
    }
    case "assignmentType": {
      const errors: Record<string, string> = {};
      const direction = isPreferenceDirection(draft.direction) ? draft.direction : null;
      if (direction === null) errors.direction = messages.ruleSetup.preferenceDirectionInvalid;
      if (preferenceHeader === null) errors.weight = messages.ruleSetup.preferenceWeightInvalid;
      if (preferenceHeader === null || direction === null) return { value: null, errors };
      return {
        value: {
          ...preferenceHeader,
          kind,
          direction,
          assignmentTypeIds: draft.assignmentTypeIds,
        },
        errors: {},
      };
    }
    case "assignmentTarget": {
      const targetCount = parseMaximumAssignmentCountDraft(draft.targetCount.raw);
      const errors: Record<string, string> = {};
      if (draft.calendarId === "") errors.calendarId = messages.ruleSetup.calendarRequired;
      if (targetCount.status !== "valid")
        errors.targetCount = messages.ruleSetup.targetCountInvalid;
      if (preferenceHeader === null) errors.weight = messages.ruleSetup.preferenceWeightInvalid;
      if (
        preferenceHeader === null ||
        Object.keys(errors).length > 0 ||
        targetCount.status !== "valid"
      )
        return { value: null, errors };
      return {
        value: {
          ...preferenceHeader,
          kind,
          calendarId: draft.calendarId,
          targetCount: targetCount.maximum,
        },
        errors: {},
      };
    }
    case "weekendTarget": {
      const targetCount = parseMaximumAssignmentCountDraft(draft.targetCount.raw);
      const errors: Record<string, string> = {};
      if (targetCount.status !== "valid")
        errors.targetCount = messages.ruleSetup.targetCountInvalid;
      if (preferenceHeader === null) errors.weight = messages.ruleSetup.preferenceWeightInvalid;
      if (
        Object.keys(errors).length > 0 ||
        preferenceHeader === null ||
        targetCount.status !== "valid"
      )
        return { value: null, errors };
      return {
        value: {
          ...preferenceHeader,
          kind,
          targetCount: targetCount.maximum,
        },
        errors: {},
      };
    }
    case "location": {
      const errors: Record<string, string> = {};
      const direction = isPreferenceDirection(draft.direction) ? draft.direction : null;
      if (direction === null) errors.direction = messages.ruleSetup.preferenceDirectionInvalid;
      if (preferenceHeader === null) errors.weight = messages.ruleSetup.preferenceWeightInvalid;
      if (preferenceHeader === null || direction === null) return { value: null, errors };
      return {
        value: {
          ...preferenceHeader,
          kind,
          direction,
          locationIds: draft.locationIds,
        },
        errors: {},
      };
    }
    case "requestedTimeOff":
      if (preferenceHeader === null)
        return { value: null, errors: { weight: messages.ruleSetup.preferenceWeightInvalid } };
      return {
        value: { ...preferenceHeader, kind, availabilityIds: draft.availabilityIds },
        errors: {},
      };
    case "workloadBalance": {
      const errors: Record<string, string> = {};
      if (draft.workloadPolicyId === "")
        errors.workloadPolicyId = messages.ruleSetup.fields.workloadPolicyRequired;
      else if (
        workloadPolicyIds !== undefined &&
        !workloadPolicyIds.includes(draft.workloadPolicyId)
      )
        errors.workloadPolicyId = messages.ruleSetup.fields.workloadPolicyInvalid;
      if (preferenceHeader === null) errors.weight = messages.ruleSetup.preferenceWeightInvalid;
      if (Object.keys(errors).length > 0 || preferenceHeader === null)
        return { value: null, errors };
      return {
        value: { ...preferenceHeader, kind, workloadPolicyId: draft.workloadPolicyId },
        errors: {},
      };
    }
    case "consecutiveWork": {
      const threshold = parseMaximumAssignmentCountDraft(draft.threshold.raw);
      const errors: Record<string, string> = {};
      if (threshold.status !== "valid")
        errors.threshold = messages.ruleSetup.consecutiveWorkThresholdInvalid;
      if (draft.active && draft.assignmentTypeIds.length === 0)
        errors.assignmentTypeIds = messages.ruleSetup.consecutiveWorkAssignmentTypesRequired;
      let mode: WorkforceConsecutiveWorkMode | null = null;
      if (typeof draft.mode !== "object") {
        errors.mode = messages.ruleSetup.consecutiveWorkModeInvalid;
      } else if (draft.mode.kind === "reportingDays") {
        mode = { kind: "reportingDays" };
      } else if (draft.mode.kind === "weekends") {
        if (draft.active && draft.mode.calendarId === "")
          errors["mode.calendarId"] = messages.ruleSetup.consecutiveWorkCalendarRequired;
        if (draft.active && draft.mode.weekdays.length === 0)
          errors["mode.weekdays"] = messages.ruleSetup.consecutiveWorkWeekdaysRequired;
        mode = {
          kind: "weekends",
          calendarId: draft.mode.calendarId,
          weekdays: draft.mode.weekdays,
        };
      } else {
        errors.mode = messages.ruleSetup.consecutiveWorkModeInvalid;
      }
      if (preferenceHeader === null) errors.weight = messages.ruleSetup.preferenceWeightInvalid;
      if (
        Object.keys(errors).length > 0 ||
        preferenceHeader === null ||
        threshold.status !== "valid" ||
        mode === null
      )
        return { value: null, errors };
      return {
        value: {
          ...preferenceHeader,
          kind,
          mode,
          assignmentTypeIds: draft.assignmentTypeIds,
          threshold: threshold.maximum,
        },
        errors: {},
      };
    }
    case "fixedAssignment": {
      const errors: Record<string, string> = {};
      if (draft.active && draft.personId === "")
        errors.personId = messages.ruleSetup.fixedAssignmentPersonRequired;
      if (draft.active && draft.shiftId === "")
        errors.shiftId = messages.ruleSetup.fixedAssignmentShiftRequired;
      if (Object.keys(errors).length > 0) return { value: null, errors };
      return {
        value: {
          ...common,
          kind,
          personId: draft.personId,
          shiftId: draft.shiftId,
          ...(draft.assigned ? {} : { assigned: false }),
        },
        errors: {},
      };
    }
    case "maximumConsecutive": {
      const maximum = parseMaximumAssignmentCountDraft(draft.maximum.raw);
      const errors: Record<string, string> = {};
      if (maximum.status !== "valid") errors.maximum = messages.ruleSetup.maximumInvalid;
      let mode: WorkforceConsecutiveMode | null = null;
      if (typeof draft.mode !== "object") {
        errors.mode = messages.ruleSetup.maximumConsecutiveModeInvalid;
      } else if (draft.mode.kind === "workedDays") {
        mode = { kind: "workedDays" };
      } else if (draft.mode.kind === "assignments") {
        const breakMinutes = parseMaximumAssignmentCountDraft(draft.mode.breakMinutes.raw);
        if (breakMinutes.status !== "valid")
          errors["mode.breakMinutes"] = messages.ruleSetup.breakMinutesInvalid;
        else mode = { kind: "assignments", breakMinutes: breakMinutes.maximum };
      } else {
        errors.mode = messages.ruleSetup.maximumConsecutiveModeInvalid;
      }
      if (Object.keys(errors).length > 0 || maximum.status !== "valid" || mode === null)
        return { value: null, errors };
      return {
        value: { ...common, kind, maximum: maximum.maximum, mode },
        errors: {},
      };
    }
    case "maximumAssignmentCount": {
      const maximum = parseMaximumAssignmentCountDraft(draft.maximum.raw);
      const errors: Record<string, string> = {};
      if (draft.calendarId === "") errors.calendarId = messages.ruleSetup.calendarRequired;
      if (maximum.status !== "valid") errors.maximum = messages.ruleSetup.maximumInvalid;
      if (maximum.status !== "valid" || draft.calendarId === "") return { value: null, errors };
      return {
        value: {
          ...common,
          kind,
          calendarId: draft.calendarId,
          maximum: maximum.maximum,
        },
        errors: {},
      };
    }
    case "maximumWorkedWeekends": {
      const maximum = parseMaximumAssignmentCountDraft(draft.maximum.raw);
      if (maximum.status !== "valid")
        return { value: null, errors: { maximum: messages.ruleSetup.maximumInvalid } };
      return {
        value: { ...common, kind, maximum: maximum.maximum },
        errors: {},
      };
    }
    case "workloadQuota": {
      const minimum = parseMaximumAssignmentCountDraft(draft.quotaMinimum.raw);
      const maximum = parseMaximumAssignmentCountDraft(draft.quotaMaximum.raw);
      const errors: Record<string, string> = {};
      if (draft.calendarId === "") errors.calendarId = messages.ruleSetup.calendarRequired;
      if (minimum.status !== "valid") errors.minimum = messages.ruleSetup.quotaMinimumInvalid;
      if (maximum.status === "invalid") errors.maximum = messages.ruleSetup.quotaMaximumInvalid;
      if (
        maximum.status === "valid" &&
        minimum.status === "valid" &&
        maximum.maximum < minimum.maximum
      )
        errors.maximum = messages.ruleSetup.quotaRangeInvalid;
      if (draft.quotaMeasure === "bucketMinutes" && draft.bucketId === "")
        errors["measure.bucketId"] = messages.ruleSetup.workloadBucketRequired;
      if (draft.quotaMeasure === "assignmentCount" && draft.quotaMembership === "intersection")
        errors.membership = messages.ruleSetup.quotaCountMembershipInvalid;
      if (Object.keys(errors).length > 0 || minimum.status !== "valid")
        return { value: null, errors };
      const measure =
        draft.quotaMeasure === "bucketMinutes"
          ? { kind: "bucketMinutes" as const, bucketId: draft.bucketId }
          : { kind: "assignmentCount" as const };
      return {
        value: {
          ...common,
          kind,
          calendarId: draft.calendarId,
          membership: draft.quotaMembership,
          measure,
          minimum: minimum.maximum,
          ...(maximum.status === "valid" ? { maximum: maximum.maximum } : {}),
        },
        errors: {},
      };
    }
    case "maximumHours": {
      const maximumMinutes = parseMaximumHoursMaximumMinutesDraft(draft.maximumMinutes.raw);
      const window = draft.window;
      const errors: Record<string, string> = {};
      if (draft.bucketId === "") errors.bucketId = messages.ruleSetup.workloadBucketRequired;
      if (maximumMinutes.status !== "valid")
        errors.maximumMinutes = messages.ruleSetup.maximumHoursInvalid;
      if (window.kind === "calendar") {
        if (window.calendarId === "")
          errors["window.calendarId"] = messages.ruleSetup.calendarRequired;
      } else if (window.durationMinutes.status !== "valid") {
        errors["window.durationMinutes"] = messages.ruleSetup.rollingDurationInvalid;
      }
      const resolvedWindow =
        window.kind === "calendar"
          ? {
              kind: window.kind,
              calendarId: window.calendarId,
              membership: window.membership,
            }
          : window.durationMinutes.status === "valid"
            ? {
                kind: window.kind,
                durationMinutes: window.durationMinutes.durationMinutes,
                membership: window.membership,
              }
            : null;
      if (
        Object.keys(errors).length > 0 ||
        maximumMinutes.status !== "valid" ||
        resolvedWindow === null
      )
        return { value: null, errors };
      return {
        value: {
          ...common,
          kind,
          bucketId: draft.bucketId,
          maximumMinutes: maximumMinutes.maximumMinutes,
          window: resolvedWindow,
        },
        errors: {},
      };
    }
    case "requiredSkillMix": {
      const errors: Record<string, string> = {};
      const qualificationMinimums: WorkforceQualificationMinimum[] =
        draft.qualificationMinimums.map((row, index) => {
          const minimum = parseRequiredSkillMixMinimumDraft(row.minimum);
          if (minimum.status !== "valid")
            errors[`qualificationMinimums.${String(index)}.minimum`] =
              messages.ruleSetup.qualificationMinimumInvalid;
          return {
            minimum: minimum.status === "valid" ? minimum.minimum : 0,
            qualifications: {
              allQualificationIds: row.allQualificationIds,
              anyQualificationIds: row.anyQualificationIds,
            },
          };
        });
      if (Object.keys(errors).length > 0) return { value: null, errors };
      return {
        value: { ...common, kind, qualificationMinimums },
        errors: {},
      };
    }
    case "minimumRest": {
      if (draft.minimumRest.status !== "valid") {
        return { value: null, errors: { minimumMinutes: "Enter an exact whole-minute duration." } };
      }
      return {
        value: {
          ...common,
          kind,
          minimumMinutes: draft.minimumRest.minutes,
          beforeScope: draft.beforeScope,
          afterScope: draft.afterScope,
        },
        errors: {},
      };
    }
    case "mutualAssignmentRestriction": {
      if (draft.mode !== "sameShift" && draft.mode !== "overlappingShifts")
        return { value: null, errors: { mode: messages.ruleSetup.mutualModeInvalid } };
      return {
        value: { ...common, kind, personIds: draft.personIds, mode: draft.mode },
        errors: {},
      };
    }
    case "transitionTime":
      return {
        value: { ...common, kind, locationIds: draft.locationIds },
        errors: {},
      };
    case "noOverlap":
      return {
        value: { ...common, kind, compatibleCategoryPairs: draft.compatibleCategoryPairs },
        errors: {},
      };
    default:
      return { value: { ...common, kind }, errors: {} };
  }
}

/** Rebase incomplete input against its saved meaning; never use this to submit a command. */
export function ruleRebaseValue(base: EditableRule, draft: RuleDraft): EditableRule | null {
  const parsed = ruleDraftValue(base.id, base.kind, draft).value;
  if (parsed !== null) return parsed;
  const preference = isEditablePreference(base) ? base : undefined;
  if (preference !== undefined) {
    const weight = parsePreferenceWeightDraft(draft.weight.raw);
    const common = {
      ...preference,
      active: draft.active,
      scope: draft.scope,
      priority: draft.priority,
      weight: weight.status === "valid" ? weight.maximum : preference.weight,
    };
    if (preference.kind === "adjacency")
      return {
        ...common,
        kind: "adjacency",
        baseScheduleId: draft.baseScheduleId || preference.baseScheduleId,
        direction:
          draft.direction === "prefer" || draft.direction === "avoid"
            ? draft.direction
            : preference.direction,
        maximumGapMinutes:
          draft.maximumGapMinutes.status === "valid"
            ? draft.maximumGapMinutes.maximum
            : preference.maximumGapMinutes,
        mode:
          draft.mode === "before" || draft.mode === "after" || draft.mode === "either"
            ? draft.mode
            : preference.mode,
      };
    if (preference.kind === "baseStability")
      return {
        ...common,
        kind: "baseStability",
        baseScheduleId: draft.baseScheduleId || preference.baseScheduleId,
      };
    if (preference.kind === "togetherSeparate")
      return {
        ...common,
        kind: "togetherSeparate",
        direction:
          draft.direction === "together" || draft.direction === "separate"
            ? draft.direction
            : preference.direction,
        mode:
          draft.mode === "sameShift" || draft.mode === "overlappingShifts"
            ? draft.mode
            : preference.mode,
        personIds:
          new Set(draft.personIds).size === draft.personIds.length
            ? draft.personIds
            : preference.personIds,
      };
    if (preference.kind === "preferredCoverage")
      return {
        ...common,
        kind: "preferredCoverage",
        coverageRequirementIds: draft.coverageRequirementIds,
      };
    if (preference.kind === "workloadBalance")
      return {
        ...common,
        kind: "workloadBalance",
        workloadPolicyId: draft.workloadPolicyId || preference.workloadPolicyId,
      };
    if (preference.kind === "consecutiveWork") {
      let mode: WorkforceConsecutiveWorkMode = preference.mode;
      if (typeof draft.mode === "object") {
        if (draft.mode.kind === "reportingDays") {
          mode = { kind: "reportingDays" };
        } else if (
          draft.mode.kind === "weekends" &&
          draft.mode.calendarId !== "" &&
          draft.mode.weekdays.length > 0
        ) {
          mode = {
            kind: "weekends",
            calendarId: draft.mode.calendarId,
            weekdays: draft.mode.weekdays,
          };
        }
      }
      return {
        ...common,
        kind: "consecutiveWork",
        mode,
        assignmentTypeIds: draft.assignmentTypeIds,
        threshold:
          draft.threshold.status === "valid" ? draft.threshold.maximum : preference.threshold,
      };
    }
    if (preference.kind === "time")
      return {
        ...common,
        kind: "time",
        direction: isPreferenceDirection(draft.direction) ? draft.direction : preference.direction,
        timeWindow: timeWindowValue(draft.timeWindow) ?? preference.timeWindow,
      };
    if (preference.kind === "assignmentType")
      return {
        ...common,
        kind: "assignmentType",
        direction: isPreferenceDirection(draft.direction) ? draft.direction : preference.direction,
        assignmentTypeIds: draft.assignmentTypeIds,
      };
    if (preference.kind === "assignmentTarget")
      return {
        ...common,
        kind: "assignmentTarget",
        calendarId: draft.calendarId,
        targetCount:
          draft.targetCount.status === "valid" ? draft.targetCount.maximum : preference.targetCount,
      };
    if (preference.kind === "weekendTarget")
      return {
        ...common,
        kind: "weekendTarget",
        targetCount:
          draft.targetCount.status === "valid" ? draft.targetCount.maximum : preference.targetCount,
      };
    if (preference.kind === "location")
      return {
        ...common,
        kind: "location",
        direction: isPreferenceDirection(draft.direction) ? draft.direction : preference.direction,
        locationIds: draft.locationIds,
      };
    return { ...common, kind: "requestedTimeOff", availabilityIds: draft.availabilityIds };
  }
  if (base.kind === "fixedAssignment") {
    const value = {
      ...base,
      active: draft.active,
      scope: draft.scope,
      personId: draft.personId || base.personId,
      shiftId: draft.shiftId || base.shiftId,
    };
    if (draft.assigned) delete value.assigned;
    else value.assigned = false;
    return value;
  }
  if (base.kind === "maximumConsecutive") {
    let mode: WorkforceConsecutiveMode = base.mode;
    if (typeof draft.mode === "object") {
      if (draft.mode.kind === "workedDays") mode = { kind: "workedDays" };
      else if (draft.mode.kind === "assignments") {
        const breakMinutes = parseMaximumAssignmentCountDraft(draft.mode.breakMinutes.raw);
        if (breakMinutes.status === "valid")
          mode = { kind: "assignments", breakMinutes: breakMinutes.maximum };
      }
    }
    return {
      ...base,
      active: draft.active,
      scope: draft.scope,
      maximum: draft.maximum.status === "valid" ? draft.maximum.maximum : base.maximum,
      mode,
    };
  }
  if (base.kind === "maximumAssignmentCount")
    return {
      ...base,
      active: draft.active,
      scope: draft.scope,
      calendarId: draft.calendarId,
      maximum: draft.maximum.status === "valid" ? draft.maximum.maximum : base.maximum,
    };
  if (base.kind === "maximumWorkedWeekends")
    return {
      ...base,
      active: draft.active,
      scope: draft.scope,
      maximum: draft.maximum.status === "valid" ? draft.maximum.maximum : base.maximum,
    };
  if (base.kind === "workloadQuota") {
    const { maximum, ...withoutMaximum } = base;
    const measure =
      draft.quotaMeasure === "assignmentCount"
        ? { kind: "assignmentCount" as const }
        : draft.bucketId !== ""
          ? { kind: "bucketMinutes" as const, bucketId: draft.bucketId }
          : base.measure;
    const selectedMaximum =
      draft.quotaMaximum.status === "valid"
        ? draft.quotaMaximum.maximum
        : draft.quotaMaximum.status === "empty"
          ? undefined
          : maximum;
    return {
      ...withoutMaximum,
      active: draft.active,
      scope: draft.scope,
      calendarId: draft.calendarId || base.calendarId,
      membership: draft.quotaMembership,
      measure,
      minimum: draft.quotaMinimum.status === "valid" ? draft.quotaMinimum.maximum : base.minimum,
      ...(selectedMaximum === undefined ? {} : { maximum: selectedMaximum }),
    };
  }
  if (base.kind === "maximumHours") {
    const window =
      draft.window.kind === "calendar"
        ? {
            kind: draft.window.kind,
            calendarId: draft.window.calendarId,
            membership: draft.window.membership,
          }
        : {
            kind: draft.window.kind,
            durationMinutes:
              draft.window.durationMinutes.status === "valid"
                ? draft.window.durationMinutes.durationMinutes
                : base.window.kind === "rolling"
                  ? base.window.durationMinutes
                  : 1,
            membership: draft.window.membership,
          };
    return {
      ...base,
      active: draft.active,
      scope: draft.scope,
      bucketId: draft.bucketId,
      maximumMinutes:
        draft.maximumMinutes.status === "valid"
          ? draft.maximumMinutes.maximumMinutes
          : base.maximumMinutes,
      window,
    };
  }
  if (base.kind === "requiredSkillMix")
    return {
      ...base,
      active: draft.active,
      scope: draft.scope,
      qualificationMinimums: qualificationMinimumsForRebase(base, draft.qualificationMinimums),
    };
  if (base.kind === "transitionTime")
    return {
      ...base,
      active: draft.active,
      scope: draft.scope,
      locationIds: draft.locationIds,
    };
  if (base.kind !== "minimumRest") return null;
  return {
    ...base,
    active: draft.active,
    scope: draft.scope,
    beforeScope: draft.beforeScope,
    afterScope: draft.afterScope,
  };
}
