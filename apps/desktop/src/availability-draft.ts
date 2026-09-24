import { newUuidV7 } from "./api/generated";
import type {
  WorkforceAvailability,
  WorkforceInstantWindow,
  WorkforceResolvedLocalTime,
  WorkforceWeeklyWindow,
  WorkforceWeekday,
} from "./api/generated-domain-pack-contracts";
import type { TemporalDraft } from "./components/planner/field-contracts";
import { parseTemporalDraft } from "./components/planner/temporal-field";
import { availabilityMessages } from "./components/availability/messages";
import { sameField } from "./entity-draft";

export interface AvailabilityWeeklyDraft {
  readonly key: string;
  readonly weekdays: readonly WorkforceWeekday[];
  readonly interval: TemporalDraft;
}
export interface AvailabilityDraft {
  readonly personId: string;
  readonly availabilityKind: WorkforceAvailability["availabilityKind"];
  readonly startDate: string;
  readonly endDateExclusive: string;
  readonly windowKind: "instant" | "weekly";
  /** Existing RFC3339 bytes stay intact unless replacement is explicitly requested. */
  readonly storedInstant: WorkforceInstantWindow | null;
  readonly replaceInstant: boolean;
  readonly localInterval: TemporalDraft;
  readonly weekly: readonly AvailabilityWeeklyDraft[];
  readonly restrictTypes: boolean;
  readonly assignmentTypeIds: readonly string[];
  readonly restrictLocations: boolean;
  readonly locationIds: readonly string[];
  readonly source: string;
  readonly note: string;
}
export interface AvailabilityResolvedWindow {
  readonly raw: { readonly startsAt: string; readonly endsAt: string };
  readonly startsAt: WorkforceResolvedLocalTime;
  readonly endsAt: WorkforceResolvedLocalTime;
}
export interface AvailabilityValueResult {
  readonly value: WorkforceAvailability | null;
  readonly errors: Readonly<Record<string, string>>;
}

export const availabilityWeekdays: readonly WorkforceWeekday[] = [
  "monday",
  "tuesday",
  "wednesday",
  "thursday",
  "friday",
  "saturday",
  "sunday",
];
export function availabilityWeeklyDraft(value?: WorkforceWeeklyWindow): AvailabilityWeeklyDraft {
  return {
    key: `availability-window-${newUuidV7()}`,
    weekdays: value?.weekdays ?? [],
    interval: parseTemporalDraft({
      kind: "localWindow",
      startTime: value?.startTime ?? "",
      endTime: value?.endTime ?? "",
      endDayOffset: String(value?.endDayOffset ?? 0),
    }),
  };
}
export function createAvailabilityDraft(
  value: WorkforceAvailability | "new",
  personId: string,
  retained?: { readonly value: WorkforceAvailability; readonly raw: AvailabilityDraft },
): AvailabilityDraft {
  const record = value === "new" ? null : value;
  const previous = record !== null && retained?.value.id === record.id ? retained : undefined;
  const sameWindow =
    previous !== undefined && sameField(record?.timeWindow, previous.value.timeWindow);
  const keepTypes =
    previous !== undefined &&
    sameField(record?.assignmentTypeIds, previous.value.assignmentTypeIds);
  const keepLocations =
    previous !== undefined && sameField(record?.locationIds, previous.value.locationIds);
  const window = record?.timeWindow;
  return {
    personId: record?.personId ?? personId,
    availabilityKind: record?.availabilityKind ?? "unavailable",
    startDate: record?.effectiveRange.startDate ?? "",
    endDateExclusive: record?.effectiveRange.endDateExclusive ?? "",
    windowKind: window?.kind ?? "weekly",
    storedInstant: sameWindow
      ? previous.raw.storedInstant
      : window?.kind === "instant"
        ? window
        : null,
    replaceInstant: sameWindow ? previous.raw.replaceInstant : window?.kind !== "instant",
    localInterval: sameWindow
      ? previous.raw.localInterval
      : parseTemporalDraft({ kind: "localInterval", startsAt: "", endsAt: "" }),
    weekly: sameWindow
      ? previous.raw.weekly
      : window?.kind === "weekly"
        ? window.windows.map(availabilityWeeklyDraft)
        : [availabilityWeeklyDraft()],
    restrictTypes: keepTypes ? previous.raw.restrictTypes : record?.assignmentTypeIds !== undefined,
    assignmentTypeIds: keepTypes
      ? previous.raw.assignmentTypeIds
      : (record?.assignmentTypeIds ?? []),
    restrictLocations: keepLocations
      ? previous.raw.restrictLocations
      : record?.locationIds !== undefined,
    locationIds: keepLocations ? previous.raw.locationIds : (record?.locationIds ?? []),
    source: record?.source ?? "",
    note: record?.note ?? "",
  };
}

/** Representation checks only. Rust owns civil-time, reference and interval validity. */
export function availabilityDraftValue(
  id: string,
  draft: AvailabilityDraft,
  resolved: AvailabilityResolvedWindow | null,
): AvailabilityValueResult {
  const errors: Record<string, string> = {};
  if (draft.personId === "") errors.personId = availabilityMessages.draft.personRequired;
  if (draft.startDate === "") errors.startDate = availabilityMessages.draft.startDateRequired;
  if (draft.endDateExclusive === "")
    errors.endDateExclusive = availabilityMessages.draft.endDateRequired;
  if (draft.restrictTypes && draft.assignmentTypeIds.length === 0)
    errors.assignmentTypeIds = availabilityMessages.draft.assignmentTypesRequired;
  if (draft.restrictLocations && draft.locationIds.length === 0)
    errors.locationIds = availabilityMessages.draft.locationsRequired;
  let timeWindow: WorkforceAvailability["timeWindow"] | null = null;
  if (draft.windowKind === "instant") {
    if (!draft.replaceInstant && draft.storedInstant !== null) timeWindow = draft.storedInstant;
    else {
      const raw = draft.localInterval.raw;
      if (draft.localInterval.status !== "readyForNativeValidation" || raw.kind !== "localInterval")
        errors.timeWindow = availabilityMessages.draft.localEndpointsRequired;
      else if (
        resolved === null ||
        !sameField(resolved.raw, { startsAt: raw.startsAt, endsAt: raw.endsAt })
      )
        errors.timeWindow = availabilityMessages.draft.localEndpointsResolutionRequired;
      else
        timeWindow = {
          kind: "instant",
          startsAt: resolved.startsAt.instant,
          endsAt: resolved.endsAt.instant,
        };
    }
  } else {
    const windows: WorkforceWeeklyWindow[] = [];
    if (draft.weekly.length === 0)
      errors.timeWindow = availabilityMessages.draft.weeklyIntervalRequired;
    for (const [index, row] of draft.weekly.entries()) {
      const parsed = parseTemporalDraft(row.interval.raw);
      if (
        parsed.status !== "readyForNativeValidation" ||
        !("endDayOffset" in parsed.candidate) ||
        row.weekdays.length === 0
      ) {
        errors[`timeWindow.windows.${String(index)}`] =
          availabilityMessages.draft.weeklyIntervalInvalid;
        continue;
      }
      windows.push({
        startTime: parsed.candidate.startTime,
        endTime: parsed.candidate.endTime,
        endDayOffset: parsed.candidate.endDayOffset,
        weekdays: row.weekdays,
      });
    }
    timeWindow = { kind: "weekly", windows };
  }
  if (timeWindow === null || Object.keys(errors).length !== 0) return { value: null, errors };
  return {
    value: {
      id,
      kind: "availability",
      personId: draft.personId,
      availabilityKind: draft.availabilityKind,
      effectiveRange: { startDate: draft.startDate, endDateExclusive: draft.endDateExclusive },
      timeWindow,
      ...(draft.restrictTypes ? { assignmentTypeIds: draft.assignmentTypeIds } : {}),
      ...(draft.restrictLocations ? { locationIds: draft.locationIds } : {}),
      source: draft.source,
      note: draft.note,
    },
    errors,
  };
}
