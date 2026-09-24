import { newUuidV7 } from "./api/generated";
import type {
  WorkforceAssignmentType,
  WorkforceCalendar,
  WorkforceCalendarPeriod,
  WorkforceCoverage,
  WorkforceCoverageRequirement,
  WorkforceEntity,
  WorkforceLocation,
  WorkforceQualificationMinimum,
  WorkforceResolvedLocalTime,
  WorkforceShiftFilter,
  WorkforceShiftInstance,
  WorkforceShiftScope,
  WorkforceShiftTemplate,
  WorkforceWeekday,
  WorkforceWorkloadBucket,
} from "./api/generated-domain-pack-contracts";
import { parseDurationDraft } from "./components/planner/duration-field";
import type { DurationDraft, TemporalDraft } from "./components/planner/field-contracts";
import { parseTemporalDraft } from "./components/planner/temporal-field";
import { sameField } from "./entity-draft";
import { messages } from "./messages";
import {
  createSupportingRecordDraft,
  supportingRecordValue,
  type SupportingRecordDraft,
} from "./supporting-record-fields";

export type WorkRecord =
  | WorkforceLocation
  | WorkforceWorkloadBucket
  | WorkforceAssignmentType
  | WorkforceCalendar
  | WorkforceShiftTemplate
  | WorkforceShiftInstance
  | WorkforceCoverageRequirement;

export interface WorkTextRowDraft {
  readonly key: string;
  readonly value: string;
}
export interface WorkTransitionDraft {
  readonly key: string;
  readonly locationId: string;
  readonly minutes: DurationDraft;
}
export interface WorkQualificationMinimumDraft {
  readonly key: string;
  readonly minimum: string;
  readonly allQualificationIds: readonly string[];
  readonly anyQualificationIds: readonly string[];
}
export interface WorkCoverageDraft {
  readonly kind: "exact" | "atLeast" | "";
  readonly count: string;
  readonly minimum: string;
  readonly preferredEnabled: boolean;
  readonly preferredCount: string;
  readonly maximumEnabled: boolean;
  readonly maximumCount: string;
  readonly qualificationMinimums: readonly WorkQualificationMinimumDraft[];
}
export interface WorkCalendarIntervalDraft {
  readonly key: string;
  readonly interval: TemporalDraft;
}
export interface WorkCalendarPeriodDraft {
  readonly kind: WorkforceCalendarPeriod["kind"] | "";
  readonly day: { readonly startTime: string };
  readonly week: { readonly anchorDate: string; readonly startTime: string };
  readonly payPeriod: {
    readonly anchorDate: string;
    readonly startTime: string;
    readonly lengthDays: string;
  };
  readonly custom: readonly WorkCalendarIntervalDraft[];
}
export interface WorkShiftScopeDraft {
  readonly scopeKind: "all" | "selected" | "filter";
  readonly selectedShiftIds: readonly string[];
  readonly filterAssignmentTypesEnabled: boolean;
  readonly filterAssignmentTypeIds: readonly string[];
  readonly filterLocationsEnabled: boolean;
  readonly filterLocationIds: readonly string[];
  readonly filterDateRangeEnabled: boolean;
  readonly filterStartDateRangeStart: string;
  readonly filterStartDateRangeEndExclusive: string;
}
export interface WorkShiftFieldsDraft {
  readonly assignmentTypeId: string;
  readonly locationEnabled: boolean;
  readonly locationId: string;
  readonly reportingAttribution: "startLocalDate" | "endLocalDate" | "";
  readonly tags: readonly WorkTextRowDraft[];
  readonly coverage: WorkCoverageDraft;
}
export type WorkRecordDraft =
  | Extract<SupportingRecordDraft, { kind: "assignmentType" }>
  | {
      readonly kind: "location";
      readonly name: string;
      readonly transitions: readonly WorkTransitionDraft[];
    }
  | {
      readonly kind: "workloadBucket";
      readonly name: string;
      readonly measurement: WorkforceWorkloadBucket["measurement"] | "";
      readonly overlappingContribution: WorkforceWorkloadBucket["overlappingContribution"] | "";
    }
  | {
      readonly kind: "calendar";
      readonly name: string;
      readonly period: WorkCalendarPeriodDraft;
    }
  | (WorkShiftFieldsDraft & {
      readonly kind: "shiftTemplate";
      readonly name: string;
      readonly recurrence: {
        readonly startDate: string;
        readonly endDateExclusive: string;
        readonly weekdays: readonly WorkforceWeekday[];
        readonly excludedDates: readonly WorkTextRowDraft[];
      };
      readonly timingMode: "localWindow" | "elapsedDuration" | "";
      readonly localWindow: TemporalDraft;
      readonly elapsedStartTime: string;
      readonly duration: DurationDraft;
    })
  | (WorkShiftFieldsDraft & {
      readonly kind: "shiftInstance";
      readonly interval: TemporalDraft;
    })
  | {
      readonly kind: "coverageRequirement";
      readonly active: boolean;
      readonly scope: WorkShiftScopeDraft;
      readonly coverage: WorkCoverageDraft;
    };

export interface WorkEndpointResolution {
  readonly local: string;
  readonly resolved: WorkforceResolvedLocalTime;
}
export interface WorkRecordPreparation {
  readonly baseline: WorkRecord | null;
  readonly startsAt: WorkEndpointResolution | null;
  readonly endsAt: WorkEndpointResolution | null;
}
export interface WorkRecordValueResult {
  readonly value: WorkRecord | null;
  readonly errors: Readonly<Record<string, string>>;
}

/** Presentation identity only; never serialized as a domain identity. */
export function workRowKey(): string {
  return `work-row-${newUuidV7()}`;
}
export function isWorkRecord(entity: WorkforceEntity): entity is WorkRecord {
  return (
    entity.kind === "location" ||
    entity.kind === "workloadBucket" ||
    entity.kind === "assignmentType" ||
    entity.kind === "calendar" ||
    entity.kind === "shiftTemplate" ||
    entity.kind === "shiftInstance" ||
    entity.kind === "coverageRequirement"
  );
}
function duration(minutes?: number): DurationDraft {
  return parseDurationDraft(minutes === undefined ? "" : String(minutes), "minutes", 0);
}
function interval(startsAt = "", endsAt = ""): TemporalDraft {
  return parseTemporalDraft({ kind: "localInterval", startsAt, endsAt });
}
/** Match prior meaning, not current position; duplicate anonymous values are consumed in order. */
function restoreRows<Value, Raw>(
  values: readonly Value[],
  before: readonly Value[] | undefined,
  raw: readonly Raw[] | undefined,
  identity: (value: Value) => string,
  create: (value: Value, before: Value | undefined, raw: Raw | undefined) => Raw,
): readonly Raw[] {
  if (raw && sameField(values, before)) return raw;
  if (!before || !raw || before.length === 0)
    return values.map((value) => create(value, undefined, undefined));
  const matches = new Map<string, { indices: number[]; next: number }>();
  before.forEach((value, index) => {
    const key = identity(value);
    const match = matches.get(key);
    if (match) match.indices.push(index);
    else matches.set(key, { indices: [index], next: 0 });
  });
  return values.map((value) => {
    const match = matches.get(identity(value));
    const index = match?.indices[match.next++];
    return create(
      value,
      index === undefined ? undefined : before[index],
      index === undefined ? undefined : raw[index],
    );
  });
}

function textRows(
  values: readonly string[],
  before?: readonly string[],
  raw?: readonly WorkTextRowDraft[],
): readonly WorkTextRowDraft[] {
  return restoreRows(
    values,
    before,
    raw,
    (value) => value,
    (value, _before, retained) => retained ?? { key: workRowKey(), value },
  );
}
function restore<T>(value: T, before: T | undefined, raw: T | undefined): T {
  return raw !== undefined && sameField(value, before) ? raw : value;
}
function restoreNumber(value: number, before: number | undefined, raw: string | undefined): string {
  return raw !== undefined && value === before ? raw : String(value);
}
function coverageDraft(
  value?: WorkforceCoverage,
  before?: WorkforceCoverage,
  raw?: WorkCoverageDraft,
): WorkCoverageDraft {
  const minima = value?.qualificationMinimums ?? [];
  return {
    kind: value ? restore(value.kind, before?.kind, raw?.kind) : "",
    count:
      value?.kind === "exact"
        ? restoreNumber(
            value.count,
            before?.kind === "exact" ? before.count : undefined,
            raw?.count,
          )
        : (raw?.count ?? ""),
    minimum:
      value?.kind === "atLeast"
        ? restoreNumber(
            value.minimum,
            before?.kind === "atLeast" ? before.minimum : undefined,
            raw?.minimum,
          )
        : (raw?.minimum ?? ""),
    preferredEnabled:
      value?.kind === "atLeast"
        ? raw && before?.kind === "atLeast" && value.preferredCount === before.preferredCount
          ? raw.preferredEnabled
          : value.preferredCount !== undefined
        : (raw?.preferredEnabled ?? false),
    preferredCount:
      value?.kind === "atLeast" && value.preferredCount !== undefined
        ? restoreNumber(
            value.preferredCount,
            before?.kind === "atLeast" ? before.preferredCount : undefined,
            raw?.preferredCount,
          )
        : (raw?.preferredCount ?? ""),
    maximumEnabled:
      value?.kind === "atLeast"
        ? raw && before?.kind === "atLeast" && value.maximumCount === before.maximumCount
          ? raw.maximumEnabled
          : value.maximumCount !== undefined
        : (raw?.maximumEnabled ?? false),
    maximumCount:
      value?.kind === "atLeast" && value.maximumCount !== undefined
        ? restoreNumber(
            value.maximumCount,
            before?.kind === "atLeast" ? before.maximumCount : undefined,
            raw?.maximumCount,
          )
        : (raw?.maximumCount ?? ""),
    qualificationMinimums: restoreRows(
      minima,
      before?.qualificationMinimums,
      raw?.qualificationMinimums,
      (row) =>
        JSON.stringify([
          row.minimum,
          row.qualifications.allQualificationIds,
          row.qualifications.anyQualificationIds,
        ]),
      (row, _before, retained) =>
        retained ?? {
          key: workRowKey(),
          minimum: String(row.minimum),
          allQualificationIds: row.qualifications.allQualificationIds,
          anyQualificationIds: row.qualifications.anyQualificationIds,
        },
    ),
  };
}
function calendarDraft(
  value?: WorkforceCalendarPeriod,
  before?: WorkforceCalendarPeriod,
  raw?: WorkCalendarPeriodDraft,
): WorkCalendarPeriodDraft {
  return {
    kind: value ? restore(value.kind, before?.kind, raw?.kind) : "",
    day:
      value?.kind === "day"
        ? {
            startTime: restore(
              value.startTime,
              before?.kind === "day" ? before.startTime : undefined,
              raw?.day.startTime,
            ),
          }
        : (raw?.day ?? { startTime: "" }),
    week:
      value?.kind === "week"
        ? {
            anchorDate: restore(
              value.anchorDate,
              before?.kind === "week" ? before.anchorDate : undefined,
              raw?.week.anchorDate,
            ),
            startTime: restore(
              value.startTime,
              before?.kind === "week" ? before.startTime : undefined,
              raw?.week.startTime,
            ),
          }
        : (raw?.week ?? { anchorDate: "", startTime: "" }),
    payPeriod:
      value?.kind === "payPeriod"
        ? {
            anchorDate: restore(
              value.anchorDate,
              before?.kind === "payPeriod" ? before.anchorDate : undefined,
              raw?.payPeriod.anchorDate,
            ),
            startTime: restore(
              value.startTime,
              before?.kind === "payPeriod" ? before.startTime : undefined,
              raw?.payPeriod.startTime,
            ),
            lengthDays: restoreNumber(
              value.lengthDays,
              before?.kind === "payPeriod" ? before.lengthDays : undefined,
              raw?.payPeriod.lengthDays,
            ),
          }
        : (raw?.payPeriod ?? { anchorDate: "", startTime: "", lengthDays: "" }),
    custom:
      value?.kind === "custom"
        ? restoreRows(
            value.intervals,
            before?.kind === "custom" ? before.intervals : undefined,
            raw?.custom,
            (row) => JSON.stringify([row.startsAt, row.endsAt]),
            (row, _before, retained) =>
              retained ?? {
                key: workRowKey(),
                interval: interval(row.startsAt, row.endsAt),
              },
          )
        : (raw?.custom ?? []),
  };
}
function shiftScopeDraft(
  value?: WorkforceShiftScope,
  before?: WorkforceShiftScope,
  raw?: WorkShiftScopeDraft,
): WorkShiftScopeDraft {
  const selected = value?.kind === "selected" ? value : undefined;
  const oldSelected = before?.kind === "selected" ? before : undefined;
  const filter = value?.kind === "filter" ? value : undefined;
  const oldFilter = before?.kind === "filter" ? before : undefined;
  const keepRange =
    raw !== undefined && sameField(filter?.startDateRange, oldFilter?.startDateRange);
  return {
    scopeKind: restore(value?.kind ?? "all", before?.kind, raw?.scopeKind),
    selectedShiftIds:
      selected === undefined
        ? (raw?.selectedShiftIds ?? [])
        : restore(selected.shiftIds, oldSelected?.shiftIds, raw?.selectedShiftIds),
    filterAssignmentTypesEnabled:
      filter === undefined
        ? (raw?.filterAssignmentTypesEnabled ?? false)
        : oldFilter === undefined
          ? filter.assignmentTypeIds !== undefined
          : restore(
              filter.assignmentTypeIds !== undefined,
              oldFilter.assignmentTypeIds !== undefined,
              raw?.filterAssignmentTypesEnabled,
            ),
    filterAssignmentTypeIds:
      filter?.assignmentTypeIds === undefined
        ? (raw?.filterAssignmentTypeIds ?? [])
        : restore(
            filter.assignmentTypeIds,
            oldFilter?.assignmentTypeIds,
            raw?.filterAssignmentTypeIds,
          ),
    filterLocationsEnabled:
      filter === undefined
        ? (raw?.filterLocationsEnabled ?? false)
        : oldFilter === undefined
          ? filter.locationIds !== undefined
          : restore(
              filter.locationIds !== undefined,
              oldFilter.locationIds !== undefined,
              raw?.filterLocationsEnabled,
            ),
    filterLocationIds:
      filter?.locationIds === undefined
        ? (raw?.filterLocationIds ?? [])
        : restore(filter.locationIds, oldFilter?.locationIds, raw?.filterLocationIds),
    filterDateRangeEnabled:
      filter === undefined
        ? (raw?.filterDateRangeEnabled ?? false)
        : oldFilter === undefined
          ? filter.startDateRange !== undefined
          : restore(
              filter.startDateRange !== undefined,
              oldFilter.startDateRange !== undefined,
              raw?.filterDateRangeEnabled,
            ),
    filterStartDateRangeStart:
      filter?.startDateRange === undefined || keepRange
        ? (raw?.filterStartDateRangeStart ?? "")
        : filter.startDateRange.startDate,
    filterStartDateRangeEndExclusive:
      filter?.startDateRange === undefined || keepRange
        ? (raw?.filterStartDateRangeEndExclusive ?? "")
        : filter.startDateRange.endDateExclusive,
  };
}
function shiftDraft(
  value?: WorkforceShiftInstance | WorkforceShiftTemplate,
  before?: WorkforceShiftInstance | WorkforceShiftTemplate,
  raw?: WorkShiftFieldsDraft,
): WorkShiftFieldsDraft {
  return {
    assignmentTypeId: value
      ? restore(value.assignmentTypeId, before?.assignmentTypeId, raw?.assignmentTypeId)
      : "",
    locationEnabled: value
      ? raw && value.locationId === before?.locationId
        ? raw.locationEnabled
        : value.locationId !== undefined
      : false,
    locationId:
      value?.locationId !== undefined
        ? restore(value.locationId, before?.locationId, raw?.locationId)
        : (raw?.locationId ?? ""),
    reportingAttribution: value
      ? restore(value.reportingAttribution, before?.reportingAttribution, raw?.reportingAttribution)
      : "",
    tags: textRows(value?.tags ?? [], before?.tags, raw?.tags),
    coverage: coverageDraft(value?.coverage, before?.coverage, raw?.coverage),
  };
}

export function createWorkRecordDraft(
  record: WorkRecord | WorkRecord["kind"],
  retained?: { readonly value: WorkRecord; readonly raw: WorkRecordDraft },
): WorkRecordDraft {
  if (typeof record === "string") {
    switch (record) {
      case "assignmentType": {
        const draft = createSupportingRecordDraft(record);
        if (draft.kind === "assignmentType") return draft;
        throw new Error("Assignment type draft expected");
      }
      case "location":
        return { kind: record, name: "", transitions: [] };
      case "workloadBucket":
        return { kind: record, name: "", measurement: "", overlappingContribution: "" };
      case "calendar":
        return { kind: record, name: "", period: calendarDraft() };
      case "shiftTemplate":
        return {
          ...shiftDraft(),
          kind: record,
          name: "",
          recurrence: { startDate: "", endDateExclusive: "", weekdays: [], excludedDates: [] },
          timingMode: "",
          localWindow: parseTemporalDraft({
            kind: "localWindow",
            startTime: "",
            endTime: "",
            endDayOffset: "",
          }),
          elapsedStartTime: "",
          duration: duration(),
        };
      case "shiftInstance":
        return { ...shiftDraft(), kind: record, interval: interval() };
      case "coverageRequirement":
        return {
          kind: record,
          active: false,
          scope: shiftScopeDraft(),
          coverage: coverageDraft(),
        };
    }
  }
  switch (record.kind) {
    case "assignmentType": {
      const previous =
        retained?.value.kind === record.kind && retained.raw.kind === record.kind
          ? { value: retained.value, raw: retained.raw }
          : undefined;
      const result = createSupportingRecordDraft(record, previous);
      if (result.kind !== "assignmentType") throw new Error("Assignment type draft expected");
      // Supporting fields supply the active meaning; retain buffers they no longer expose.
      return {
        ...result,
        allQualificationIds:
          record.qualifications.kind === "unconstrained"
            ? (previous?.raw.allQualificationIds ?? result.allQualificationIds)
            : result.allQualificationIds,
        anyQualificationIds:
          record.qualifications.kind === "unconstrained"
            ? (previous?.raw.anyQualificationIds ?? result.anyQualificationIds)
            : result.anyQualificationIds,
        locationId:
          record.locationBehavior.kind !== "fixed"
            ? (previous?.raw.locationId ?? result.locationId)
            : result.locationId,
      };
    }
    case "location": {
      const before = retained?.value.kind === record.kind ? retained.value : undefined;
      const raw = retained?.raw.kind === record.kind ? retained.raw : undefined;
      return {
        kind: record.kind,
        name: restore(record.name, before?.name, raw?.name),
        transitions: restoreRows(
          record.transitions,
          before?.transitions,
          raw?.transitions,
          (row) => row.locationId,
          (row, old, previous) => ({
            key: previous?.key ?? workRowKey(),
            locationId: restore(row.locationId, old?.locationId, previous?.locationId),
            minutes:
              previous && row.minutes === old?.minutes ? previous.minutes : duration(row.minutes),
          }),
        ),
      };
    }
    case "workloadBucket": {
      const before = retained?.value.kind === record.kind ? retained.value : undefined;
      const raw = retained?.raw.kind === record.kind ? retained.raw : undefined;
      return {
        kind: record.kind,
        name: restore(record.name, before?.name, raw?.name),
        measurement: restore(record.measurement, before?.measurement, raw?.measurement),
        overlappingContribution: restore(
          record.overlappingContribution,
          before?.overlappingContribution,
          raw?.overlappingContribution,
        ),
      };
    }
    case "calendar": {
      const before = retained?.value.kind === record.kind ? retained.value : undefined;
      const raw = retained?.raw.kind === record.kind ? retained.raw : undefined;
      return {
        kind: record.kind,
        name: restore(record.name, before?.name, raw?.name),
        period: calendarDraft(record.period, before?.period, raw?.period),
      };
    }
    case "shiftTemplate": {
      const before = retained?.value.kind === record.kind ? retained.value : undefined;
      const raw = retained?.raw.kind === record.kind ? retained.raw : undefined;
      const local = raw?.localWindow.raw.kind === "localWindow" ? raw.localWindow.raw : undefined;
      const oldLocal = before?.timing.kind === "localWindow" ? before.timing : undefined;
      return {
        ...shiftDraft(record, before, raw),
        kind: record.kind,
        name: restore(record.name, before?.name, raw?.name),
        recurrence: {
          startDate: restore(
            record.recurrence.effectiveRange.startDate,
            before?.recurrence.effectiveRange.startDate,
            raw?.recurrence.startDate,
          ),
          endDateExclusive: restore(
            record.recurrence.effectiveRange.endDateExclusive,
            before?.recurrence.effectiveRange.endDateExclusive,
            raw?.recurrence.endDateExclusive,
          ),
          weekdays: restore(
            record.recurrence.weekdays,
            before?.recurrence.weekdays,
            raw?.recurrence.weekdays,
          ),
          excludedDates: textRows(
            record.recurrence.excludedDates,
            before?.recurrence.excludedDates,
            raw?.recurrence.excludedDates,
          ),
        },
        timingMode: restore(record.timing.kind, before?.timing.kind, raw?.timingMode),
        localWindow:
          record.timing.kind === "localWindow"
            ? parseTemporalDraft({
                kind: "localWindow",
                startTime: restore(record.timing.startTime, oldLocal?.startTime, local?.startTime),
                endTime: restore(record.timing.endTime, oldLocal?.endTime, local?.endTime),
                endDayOffset: restoreNumber(
                  record.timing.endDayOffset,
                  oldLocal?.endDayOffset,
                  local?.endDayOffset,
                ),
              })
            : (raw?.localWindow ??
              parseTemporalDraft({
                kind: "localWindow",
                startTime: "",
                endTime: "",
                endDayOffset: "",
              })),
        elapsedStartTime:
          record.timing.kind === "elapsedDuration"
            ? restore(
                record.timing.startTime,
                before?.timing.kind === "elapsedDuration" ? before.timing.startTime : undefined,
                raw?.elapsedStartTime,
              )
            : (raw?.elapsedStartTime ?? ""),
        duration:
          record.timing.kind === "elapsedDuration"
            ? raw &&
              before?.timing.kind === "elapsedDuration" &&
              record.timing.durationMinutes === before.timing.durationMinutes
              ? raw.duration
              : duration(record.timing.durationMinutes)
            : (raw?.duration ?? duration()),
      };
    }
    case "shiftInstance": {
      const before = retained?.value.kind === record.kind ? retained.value : undefined;
      const raw = retained?.raw.kind === record.kind ? retained.raw : undefined;
      const local = raw?.interval.raw.kind === "localInterval" ? raw.interval.raw : undefined;
      return {
        ...shiftDraft(record, before, raw),
        kind: record.kind,
        interval: interval(
          sameField(record.startsAt, before?.startsAt) && local
            ? local.startsAt
            : record.startsAt.local,
          sameField(record.endsAt, before?.endsAt) && local ? local.endsAt : record.endsAt.local,
        ),
      };
    }
    case "coverageRequirement": {
      const before = retained?.value.kind === record.kind ? retained.value : undefined;
      const raw = retained?.raw.kind === record.kind ? retained.raw : undefined;
      return {
        kind: record.kind,
        active:
          raw && before !== undefined && record.active === before.active
            ? raw.active
            : record.active,
        scope: shiftScopeDraft(record.scope, before?.scope, raw?.scope),
        coverage: coverageDraft(record.coverage, before?.coverage, raw?.coverage),
      };
    }
  }
}

/** Renderer representation only. Rust approves references, civil time and domain semantics. */
export function workRecordValue(
  id: string,
  draft: WorkRecordDraft,
  preparation: WorkRecordPreparation,
): WorkRecordValueResult {
  if (draft.kind === "assignmentType") {
    const result = supportingRecordValue(id, draft);
    return {
      value: result.value?.kind === "assignmentType" ? result.value : null,
      errors: result.errors,
    };
  }
  const errors: Record<string, string> = {};
  const copy = messages.work.fields;
  function integer(raw: string, path: string, maximum: number, minimum = 0): number {
    const digits = raw.replace(/^0+/, "") || "0";
    if (
      raw === "" ||
      /[^0-9]/.test(raw) ||
      digits.length > 10 ||
      Number(digits) > maximum ||
      Number(digits) < minimum
    ) {
      errors[path] = copy.integer;
      return 0;
    }
    return Number(digits);
  }
  function required(raw: string, path: string): string {
    if (raw === "") errors[path] = copy.selection;
    return raw;
  }
  function minutes(raw: DurationDraft, path: string, minimum: 0 | 1): number {
    const parsed = parseDurationDraft(raw.raw, raw.unit, minimum);
    if (parsed.status !== "valid") {
      errors[path] =
        parsed.status === "empty"
          ? copy.incomplete
          : messages.supportingFields.durationErrors[parsed.error];
      return 0;
    }
    return parsed.minutes;
  }
  function coverage(raw: WorkCoverageDraft): WorkforceCoverage {
    const qualificationMinimums: WorkforceQualificationMinimum[] = raw.qualificationMinimums.map(
      (row, index) => ({
        minimum: integer(
          row.minimum,
          `coverage.qualificationMinimums.${String(index)}.minimum`,
          65535,
        ),
        qualifications: {
          allQualificationIds: row.allQualificationIds,
          anyQualificationIds: row.anyQualificationIds,
        },
      }),
    );
    if (raw.kind === "exact")
      return {
        kind: "exact",
        count: integer(raw.count, "coverage.count", 65535),
        qualificationMinimums,
      };
    if (raw.kind === "") errors["coverage.kind"] = copy.selection;
    return {
      kind: "atLeast",
      minimum: integer(raw.minimum, "coverage.minimum", 65535),
      qualificationMinimums,
      ...(raw.preferredEnabled
        ? { preferredCount: integer(raw.preferredCount, "coverage.preferredCount", 65535) }
        : {}),
      ...(raw.maximumEnabled
        ? { maximumCount: integer(raw.maximumCount, "coverage.maximumCount", 65535) }
        : {}),
    };
  }
  function result(value: WorkRecord): WorkRecordValueResult {
    return { value: Object.keys(errors).length ? null : value, errors };
  }
  switch (draft.kind) {
    case "location":
      return result({
        kind: draft.kind,
        id,
        name: draft.name,
        transitions: draft.transitions.map((row, index) => ({
          locationId: required(row.locationId, `transitions.${String(index)}.locationId`),
          minutes: minutes(row.minutes, `transitions.${String(index)}.minutes`, 0),
        })),
      });
    case "workloadBucket": {
      if (draft.measurement === "") errors.measurement = copy.selection;
      if (draft.overlappingContribution === "") errors.overlappingContribution = copy.selection;
      if (draft.measurement === "" || draft.overlappingContribution === "")
        return { value: null, errors };
      return result({
        kind: draft.kind,
        id,
        name: draft.name,
        measurement: draft.measurement,
        overlappingContribution: draft.overlappingContribution,
      });
    }
    case "calendar": {
      const raw = draft.period;
      let period: WorkforceCalendarPeriod;
      switch (raw.kind) {
        case "":
          return { value: null, errors: { "period.kind": copy.selection } };
        case "day":
          period = { kind: raw.kind, startTime: required(raw.day.startTime, "period.startTime") };
          break;
        case "week":
          period = {
            kind: raw.kind,
            anchorDate: required(raw.week.anchorDate, "period.anchorDate"),
            startTime: required(raw.week.startTime, "period.startTime"),
          };
          break;
        case "payPeriod":
          period = {
            kind: raw.kind,
            anchorDate: required(raw.payPeriod.anchorDate, "period.anchorDate"),
            startTime: required(raw.payPeriod.startTime, "period.startTime"),
            lengthDays: integer(raw.payPeriod.lengthDays, "period.lengthDays", 65535, 1),
          };
          break;
        case "custom":
          period = {
            kind: raw.kind,
            intervals: raw.custom.map((row, index) => {
              const path = `period.intervals.${String(index)}`;
              if (row.interval.raw.kind !== "localInterval") {
                errors[path] = copy.incomplete;
                return { startsAt: "", endsAt: "" };
              }
              return {
                startsAt: required(row.interval.raw.startsAt, `${path}.startsAt`),
                endsAt: required(row.interval.raw.endsAt, `${path}.endsAt`),
              };
            }),
          };
          break;
      }
      return result({ kind: draft.kind, id, name: draft.name, period });
    }
    case "shiftTemplate":
    case "shiftInstance": {
      const common = {
        id,
        assignmentTypeId: required(draft.assignmentTypeId, "assignmentTypeId"),
        coverage: coverage(draft.coverage),
        ...(draft.locationEnabled ? { locationId: required(draft.locationId, "locationId") } : {}),
        tags: draft.tags.map((row) => row.value),
      };
      if (draft.reportingAttribution === "") errors.reportingAttribution = copy.selection;
      if (draft.kind === "shiftTemplate") {
        const recurrence = {
          effectiveRange: {
            startDate: required(draft.recurrence.startDate, "recurrence.effectiveRange.startDate"),
            endDateExclusive: required(
              draft.recurrence.endDateExclusive,
              "recurrence.effectiveRange.endDateExclusive",
            ),
          },
          weekdays: draft.recurrence.weekdays,
          excludedDates: draft.recurrence.excludedDates.map((row, index) =>
            required(row.value, `recurrence.excludedDates.${String(index)}`),
          ),
        };
        let timing: WorkforceShiftTemplate["timing"] | null = null;
        if (draft.timingMode === "localWindow" && draft.localWindow.raw.kind === "localWindow") {
          const raw = draft.localWindow.raw;
          timing = {
            kind: "localWindow",
            startTime: required(raw.startTime, "timing.startTime"),
            endTime: required(raw.endTime, "timing.endTime"),
            endDayOffset: integer(raw.endDayOffset, "timing.endDayOffset", 255),
          };
        } else if (draft.timingMode === "elapsedDuration") {
          timing = {
            kind: "elapsedDuration",
            startTime: required(draft.elapsedStartTime, "timing.startTime"),
            durationMinutes: minutes(draft.duration, "timing.durationMinutes", 1),
          };
        } else errors["timing.kind"] = copy.selection;
        if (!timing || draft.reportingAttribution === "") return { value: null, errors };
        return result({
          ...common,
          kind: draft.kind,
          name: draft.name,
          reportingAttribution: draft.reportingAttribution,
          recurrence,
          timing,
          occurrenceIdentities:
            preparation.baseline?.kind === "shiftTemplate" && preparation.baseline.id === id
              ? preparation.baseline.occurrenceIdentities
              : {},
        });
      }
      const baseline =
        preparation.baseline?.kind === "shiftInstance" && preparation.baseline.id === id
          ? preparation.baseline
          : null;
      const raw = draft.interval.raw;
      function endpoint(field: "startsAt" | "endsAt"): WorkforceResolvedLocalTime | null {
        if (raw.kind !== "localInterval") {
          errors.interval = copy.incomplete;
          return null;
        }
        if (raw[field] === "") {
          errors[field] = copy.incomplete;
          return null;
        }
        if (baseline && raw[field] === baseline[field].local) return baseline[field];
        const prepared = preparation[field];
        if (prepared && prepared.local === raw[field]) return prepared.resolved;
        errors[field] = copy.endpointPreparation;
        return null;
      }
      const startsAt = endpoint("startsAt");
      const endsAt = endpoint("endsAt");
      if (!startsAt || !endsAt || draft.reportingAttribution === "") return { value: null, errors };
      return result({
        ...common,
        kind: draft.kind,
        reportingAttribution: draft.reportingAttribution,
        startsAt,
        endsAt,
        origin: baseline?.origin ?? { kind: "manual" },
      });
    }
    case "coverageRequirement": {
      let scope: WorkforceShiftScope;
      if (draft.scope.scopeKind === "all") {
        scope = { kind: "all" };
      } else if (draft.scope.scopeKind === "selected") {
        scope = { kind: "selected", shiftIds: draft.scope.selectedShiftIds };
      } else {
        const filter: WorkforceShiftFilter = {
          kind: "filter",
          ...(draft.scope.filterAssignmentTypesEnabled
            ? { assignmentTypeIds: draft.scope.filterAssignmentTypeIds }
            : {}),
          ...(draft.scope.filterLocationsEnabled
            ? { locationIds: draft.scope.filterLocationIds }
            : {}),
          ...(draft.scope.filterDateRangeEnabled
            ? {
                startDateRange: {
                  startDate: draft.scope.filterStartDateRangeStart,
                  endDateExclusive: draft.scope.filterStartDateRangeEndExclusive,
                },
              }
            : {}),
        };
        scope = filter;
        if (draft.scope.filterDateRangeEnabled) {
          if (draft.scope.filterStartDateRangeStart === "")
            errors["scope.startDateRange.startDate"] = copy.incomplete;
          if (draft.scope.filterStartDateRangeEndExclusive === "")
            errors["scope.startDateRange.endDateExclusive"] = copy.incomplete;
        }
      }
      return result({
        kind: draft.kind,
        id,
        active: draft.active,
        coverage: coverage(draft.coverage),
        scope,
      });
    }
  }
}
