import { formatNumber } from "../../messages";
import type {
  WorkforceSetupAvailabilityKind,
  WorkforceWeekday,
} from "../../api/generated-domain-pack-contracts";

function formatDate(value: string, locale?: string): string {
  if (locale === undefined || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return value;
  const instant = new Date(`${value}T00:00:00Z`);
  if (Number.isNaN(instant.getTime())) return value;
  try {
    return new Intl.DateTimeFormat(locale, {
      calendar: "iso8601",
      dateStyle: "medium",
      timeZone: "UTC",
    }).format(instant);
  } catch {
    return value;
  }
}

export const availabilityMessages = {
  heading: "Availability",
  description:
    "Record when someone cannot work, when they are available only at certain times, or when they have approved time off. Unavailable and available-only windows affect shifts only if a matching Required rule is active; approved time off blocks overlapping shifts on its own. The calendar shows saved intervals, not whether a whole schedule is possible.",
  person: "Person",
  personPrompt: "Select a person to manage availability records.",
  refresh: "Refresh",
  factsLoading: "Loading date and time settings…",
  dirtyHelp: "Save or explicitly discard this draft before selecting another person or record.",
  unsupportedProject: "Availability setup requires a Workforce project.",
  add: "Add availability record",
  recordsHeading: "Saved availability records",
  recordsDescription:
    "This list also includes past and future records that are not visible in the selected calendar dates.",
  authoredRecords: (count: number, locale?: string) =>
    `${formatNumber(count, locale)} saved records.`,
  recordSummary: (kind: string, start: string, end: string, locale?: string) =>
    `${kind} · ${formatDate(start, locale)} – ${formatDate(end, locale)} (end date not included)`,
  effectiveDateRange: (start: string, end: string, locale?: string) =>
    `${formatDate(start, locale)} – ${formatDate(end, locale)} (end date not included)`,
  recordsEmpty: "No availability records for this person.",
  recordsLoading: "Loading records…",
  recordsPages: "Availability record pages",
  loadingDetail: "Loading the selected availability record…",
  editRecord: "Edit record",
  editAvailability: (id: string) => `Edit availability ${id}`,
  calendarHeading: "Calendar view",
  calendarDescription: (timeZone: string) =>
    `Saved intervals in ${timeZone}. Unsaved edits are not shown, and this view does not check whether rules allow a particular shift.`,
  calendarEmpty: "No occurrences in the selected date range.",
  calendarLoading: "Loading calendar occurrences…",
  calendarRangeStart: "First date",
  calendarRangeEnd: "Stop before this date",
  calendarRangeHelp:
    "Choose up to 366 dates in the project's time zone. The calendar shows at most 31 dates at a time.",
  calendarApplyRange: "Show range",
  calendarSummary: (count: number, start: string, end: string, locale?: string) =>
    `${formatNumber(count, locale)} intervals between ${formatDate(start, locale)} and ${formatDate(end, locale)} (end date not included).`,
  calendarEvidence:
    "Calendar marks show only this page of intervals. A blank date does not prove the person is available. Use the full list below to see other pages.",
  calendarDaysLabel: "Calendar dates and configured intervals on this page",
  calendarOccurrence: (kind: string, ordinal: number, locale?: string) =>
    `${kind} · ${formatNumber(ordinal, locale)}`,
  calendarMoreOccurrences: (count: number, locale?: string) =>
    `${formatNumber(count, locale)} more on this page; see the interval list.`,
  calendarAddAvailabilityForDate: (date: string) => `Add availability for ${date}`,
  calendarAddForDate: "Add for this date",
  calendarDatePages: "Visual calendar date pages",
  calendarPreviousDates: "Previous dates",
  calendarNextDates: "Next dates",
  calendarIntervalList: "Availability intervals on this page",
  calendarIntervalCaption: "All intervals on this page",
  calendarOccurrenceColumn: "Interval",
  calendarKindColumn: "Type",
  calendarStartColumn: "Starts",
  calendarEndColumn: "Ends before",
  calendarRecordColumn: "Saved record",
  calendarOccurrencePages: "Availability interval pages",
  calendarFirstOccurrencePage: "First occurrence page",
  calendarNextOccurrencePage: "Next occurrence page",
  calendarPageIdentityError: "The native occurrence page exceeded its identity or size bounds.",
  calendarRangeError: "This date range cannot be represented by the visual calendar.",
  calendarDateLabelsError: "Calendar date labels are unavailable.",
  calendarDisplayError:
    "The browser cannot display calendar labels for these native dates or timezone. The exact native intervals remain available in the list below.",
  kindLabel: "Type of availability",
  kindHelp:
    "Unavailable and available-only windows apply through active Required availability rules. Approved time off blocks overlapping shifts on its own.",
  importedKindHelp:
    "Requested time off is a Preference. Unlike approved time off, it does not block a shift on its own.",
  startDate: "Starts on",
  startDateHelp: "The first date when this record applies.",
  endDateExclusive: "Stops before this date",
  endDateHelp: "This date is not included in the record.",
  effectiveDates: "Effective dates",
  windowType: "When does this apply?",
  windowTypeInstant: "One time window with start and end dates",
  windowTypeWeekly: "Weekly recurring schedule",
  instantStartsAt: "Starts at",
  instantStartsAtHelp: "Local text timestamp for the start of this availability.",
  instantEndsAt: "Ends at",
  instantEndsAtHelp: "Local text timestamp for the end of this availability.",
  storedStart: "Stored start:",
  storedEnd: "Stored end, exclusive:",
  replaceStoredInterval: "Replace this stored interval using scenario-local endpoints",
  preserveStoredInterval:
    "Leaving replacement unchecked preserves both stored instants exactly, including their precision and offsets.",
  localInterval: "Local interval to resolve",
  recurringLocalInterval: "Recurring local interval",
  weeklyHeading: (number: number, locale?: string) =>
    `Weekly schedule ${formatNumber(number, locale)}`,
  weeklyAddEntry: "Add weekly entry",
  weeklyRemoveEntry: "Remove entry",
  weeklyStartTime: "Start time",
  weeklyEndTime: "End time",
  weeklyEndDayOffset: "Days after the start day",
  weeklyEndDayOffsetHelp: "Enter 0 for the same day or 1 for the following day (up to 255 days).",
  weeklyWeekdays: "Weekdays",
  weeklyWeekdaysHelp: "Select the weekdays on which this time window applies.",
  weekdayNames: {
    monday: "Monday",
    tuesday: "Tuesday",
    wednesday: "Wednesday",
    thursday: "Thursday",
    friday: "Friday",
    saturday: "Saturday",
    sunday: "Sunday",
  } satisfies Readonly<Record<WorkforceWeekday, string>>,
  optionalAssignmentRestrictions: "Optional work type and location limits",
  assignmentTypeRestriction: "Limit to certain work types",
  assignmentTypeRestrictionHelp:
    "Leave empty to cover all work types. Select specific types if this record only applies to those shifts.",
  selectedAssignmentTypes: "Selected work types",
  locationRestriction: "Restrict to locations",
  locationRestrictionHelp:
    "Leave empty to apply to all locations. Select specific locations to limit this availability record.",
  selectedLocations: "Selected locations",
  restrictionHelp:
    "Unchecked restrictions apply to all types or locations. A checked restriction needs an explicit selection.",
  source: "Source",
  sourceHelp: "For your reference only. This text is not run or treated as a link.",
  note: "Note",
  noteHelp: "For your reference only. This text is not formatted or treated as a link.",
  editorHeading: "Edit availability record",
  editorNewHeading: "New availability record",
  recordIdentity: (id: string) => `Record identity: ${id}`,
  previewHeading: "Review availability changes",
  previewDescription:
    "Review the changes before applying. Applied commands are recorded as a single undo step in project history.",
  capturedRevision: (revision: number, locale?: string) =>
    `Captured revision: ${formatNumber(revision, locale)}`,
  apply: "Apply changes",
  discard: "Discard",
  cancel: "Cancel",
  remove: "Remove",
  removeConfirm: (id: string) =>
    `This will remove the availability record from the scenario. The change is reviewed and applied as a single undo step. Record: ${id}`,
  pending: "Applying changes…",
  stale:
    "The saved revision changed while editing. Review the current state before reapplying. Your raw draft has not been replaced.",
  reviewError: "Could not review availability changes",
  noSelection: "Select a person from the list above to manage their availability.",
  draft: {
    personRequired: "Select a person.",
    startDateRequired: "Enter the first date when this record applies.",
    endDateRequired: "Enter the date when this record stops; that date is not included.",
    assignmentTypesRequired: "Select at least one work type, or remove the limit.",
    locationsRequired: "Select at least one location, or remove the limit.",
    localEndpointsRequired: "Enter both start and end times so the app can check them.",
    localEndpointsResolutionRequired:
      "Check the start and end times against the project's time settings.",
    weeklyIntervalRequired: "Add at least one weekly time window.",
    weeklyIntervalInvalid:
      "Enter a start and end time, a whole number of days after the start (0–255), and at least one weekday.",
  },
  errorHeading: "Check this availability draft",
  timeSettingsChanged: (
    previousZone: string,
    previousGapPolicy: string,
    previousOverlapPolicy: string,
    currentZone: string,
    currentGapPolicy: string,
    currentOverlapPolicy: string,
  ) =>
    `The native time settings changed from ${previousZone} / ${previousGapPolicy} / ${previousOverlapPolicy} to ${currentZone} / ${currentGapPolicy} / ${currentOverlapPolicy}.`,
  approveTime: "Use the current native time settings when reviewing local or weekly intervals",
  unresolvedWrite:
    "The previous write's outcome is unknown. Resolve it through the operation status before approving another write.",
  reviewChanges: "Review changes",
  reviewCurrentChanges: "Review current changes",
  reviewRemoval: "Review removal",
  conflictsHeading: "Choose each conflicting field",
  conflictsHelp:
    "Arrays and nested windows are whole fields. No automatic element merge is applied.",
  currentSavedValue: "Current saved value:",
  draftValue: "Draft value:",
  useSavedField: (field: string) => `Use saved ${field}`,
  keepDraftField: (field: string) => `Keep draft ${field}`,
  fieldLabels: {
    id: "Record identity",
    kind: "Record kind",
    personId: "Person identity",
    availabilityKind: "Availability kind",
    effectiveRange: "Effective dates",
    timeWindow: "Time windows",
    assignmentTypeIds: "Assignment type restriction",
    locationIds: "Location restriction",
    source: "Source",
    note: "Note",
  },
  proposedKind: "Kind",
  proposedPerson: "Person",
  proposedEffectiveDates: "Effective dates",
  proposedAssignmentTypes: "Assignment types",
  proposedLocations: "Locations",
  proposedSource: "Source",
  proposedNote: "Note",
  assignmentTypes: (ids: readonly string[] | undefined) =>
    ids === undefined ? "All assignment types" : ids.join(", "),
  locations: (ids: readonly string[] | undefined) =>
    ids === undefined ? "All locations" : ids.join(", "),
  proposedTimeWindow: "Native proposed time window",
  reviewChange: (kind: string, path: string) => `${kind} · ${path}`,
  nativeChanges: (count: number, locale?: string) =>
    `${formatNumber(count, locale)} native changes.`,
  reviewChangePages: "Availability command change pages",
  reviewWarnings: "Native review warnings",
  nativeWarnings: (count: number, locale?: string) =>
    `${formatNumber(count, locale)} native warnings.`,
  previousWarnings: "Previous warnings",
  nextWarnings: "Next warnings",
  projectHistory: "Open project history for undo and redo",
  recordsPageIdentityError:
    "The native availability page did not match its identity or size bounds.",
  recordIdentityMismatch:
    "The availability record no longer matches the selected person and record identity.",
  timeSettingsReviewRequired:
    "Review the current native time settings before preparing this interval.",
  resolvingEndpoint: (endpoint: "start" | "end") =>
    `Resolving availability ${endpoint} in the scenario timezone…`,
} as const;

export const AVAILABILITY_KIND_LABELS: Readonly<Record<WorkforceSetupAvailabilityKind, string>> = {
  unavailable: "Unavailable",
  availableOnly: "Available only",
  approvedTimeOff: "Approved time off",
  requestedTimeOff: "Requested time off",
};
