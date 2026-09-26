import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  WorkforceCalendar,
  WorkforceCoverageRequirement,
  WorkforceLocation,
  WorkforceQualificationMinimum,
  WorkforceShiftTemplate,
} from "./api/generated-domain-pack-contracts";
import { parseDurationDraft } from "./components/planner/duration-field";
import { parseTemporalDraft } from "./components/planner/temporal-field";
import { rebaseEntityDraft } from "./entity-draft";
import { createWorkRecordDraft, workRecordValue } from "./work-record-draft";

beforeEach(() => vi.stubGlobal("window", { crypto: globalThis.crypto }));
afterEach(() => vi.unstubAllGlobals());
const id = "01900000-0000-7000-8000-000000000001";
const firstId = "01900000-0000-7000-8000-000000000002";
const secondId = "01900000-0000-7000-8000-000000000003";
const thirdId = "01900000-0000-7000-8000-000000000004";

function template(): WorkforceShiftTemplate {
  return {
    id,
    kind: "shiftTemplate",
    name: "Shift",
    assignmentTypeId: firstId,
    coverage: { kind: "exact", count: 3, qualificationMinimums: [] },
    recurrence: {
      effectiveRange: { startDate: "2026-09-01", endDateExclusive: "2026-10-01" },
      weekdays: ["monday"],
      excludedDates: [],
    },
    timing: { kind: "localWindow", startTime: "09:00", endTime: "17:00", endDayOffset: 0 },
    reportingAttribution: "startLocalDate",
    tags: [],
    occurrenceIdentities: {},
  };
}

describe("rebased work collection raw buffers", () => {
  it("follows transition location identities after insertion and reorder, but adopts changed minutes", () => {
    const base: WorkforceLocation = {
      id,
      kind: "location",
      name: "Location",
      transitions: [
        { locationId: firstId, minutes: 90 },
        { locationId: secondId, minutes: 120 },
      ],
    };
    const initial = createWorkRecordDraft(base);
    if (initial.kind !== "location") throw new Error("Expected location fields");
    const raw = {
      ...initial,
      transitions: [
        { key: "first", locationId: firstId, minutes: parseDurationDraft("1.5", "hours", 0) },
        { key: "second", locationId: secondId, minutes: parseDurationDraft("02", "hours", 0) },
      ],
    };
    const current: WorkforceLocation = {
      ...base,
      transitions: [
        { locationId: thirdId, minutes: 90 },
        { locationId: secondId, minutes: 180 },
        { locationId: firstId, minutes: 90 },
      ],
    };
    const merged = rebaseEntityDraft(base, base, current);
    const restored = createWorkRecordDraft(merged.value, { value: merged.local, raw });
    if (restored.kind !== "location") throw new Error("Expected location fields");
    expect(restored.transitions[0]).toMatchObject({
      locationId: thirdId,
      minutes: { raw: "90", unit: "minutes", minutes: 90 },
    });
    expect(restored.transitions[0]?.key).not.toBe("first");
    expect(restored.transitions[0]?.key).not.toBe("second");
    expect(restored.transitions[1]).toMatchObject({
      key: "second",
      locationId: secondId,
      minutes: { raw: "180", unit: "minutes", minutes: 180 },
    });
    expect(restored.transitions[2]).toEqual(raw.transitions[0]);
  });

  it("matches tags and excluded dates by value, consuming duplicate rows once in prior order", () => {
    const base: WorkforceShiftTemplate = {
      ...template(),
      tags: ["duplicate", "other", "duplicate"],
      recurrence: {
        ...template().recurrence,
        excludedDates: ["2026-09-07", "2026-09-14", "2026-09-07"],
      },
    };
    const initial = createWorkRecordDraft(base);
    if (initial.kind !== "shiftTemplate") throw new Error("Expected template fields");
    const raw = {
      ...initial,
      tags: [
        { key: "tag-first", value: "duplicate" },
        { key: "tag-other", value: "other" },
        { key: "tag-second", value: "duplicate" },
      ],
      recurrence: {
        ...initial.recurrence,
        excludedDates: [
          { key: "date-first", value: "2026-09-07" },
          { key: "date-other", value: "2026-09-14" },
          { key: "date-second", value: "2026-09-07" },
        ],
      },
    };
    const current: WorkforceShiftTemplate = {
      ...base,
      tags: ["new", "duplicate", "duplicate", "duplicate", "other"],
      recurrence: {
        ...base.recurrence,
        excludedDates: ["2026-09-21", "2026-09-07", "2026-09-07", "2026-09-07", "2026-09-14"],
      },
    };
    const restored = createWorkRecordDraft(current, { value: base, raw });
    if (restored.kind !== "shiftTemplate") throw new Error("Expected template fields");
    for (const [rows, previous, values] of [
      [restored.tags, raw.tags, current.tags],
      [
        restored.recurrence.excludedDates,
        raw.recurrence.excludedDates,
        current.recurrence.excludedDates,
      ],
    ] as const) {
      expect(rows.map((row) => row.value)).toEqual(values);
      expect(rows[1]).toEqual(previous[0]);
      expect(rows[2]).toEqual(previous[2]);
      expect(rows[4]).toEqual(previous[1]);
      expect(new Set(rows.map((row) => row.key)).size).toBe(5);
      expect(previous.map((row) => row.key)).not.toContain(rows[0]?.key);
      expect(previous.map((row) => row.key)).not.toContain(rows[3]?.key);
    }
  });

  it("retains custom interval buffers only for complete semantic matches, including duplicate and inactive buffers", () => {
    const first = { startsAt: "2026-09-07T09:00", endsAt: "2026-09-07T17:00" };
    const other = { startsAt: "2026-09-14T09:00", endsAt: "2026-09-14T17:00" };
    const base: WorkforceCalendar = {
      id,
      kind: "calendar",
      name: "Calendar",
      period: { kind: "custom", intervals: [first, other, first] },
    };
    const initial = createWorkRecordDraft(base);
    if (initial.kind !== "calendar") throw new Error("Expected calendar fields");
    const raw = {
      ...initial,
      period: {
        ...initial.period,
        custom: [
          {
            key: "first",
            interval: parseTemporalDraft({
              kind: "localInterval",
              startsAt: "unfinished first",
              endsAt: "",
            }),
          },
          {
            key: "other",
            interval: parseTemporalDraft({
              kind: "localInterval",
              startsAt: "unfinished other",
              endsAt: "",
            }),
          },
          {
            key: "second",
            interval: parseTemporalDraft({
              kind: "localInterval",
              startsAt: "unfinished second",
              endsAt: "",
            }),
          },
        ],
      },
    };
    const changed = { ...other, endsAt: "2026-09-14T18:00" };
    const restored = createWorkRecordDraft(
      {
        ...base,
        period: { kind: "custom", intervals: [changed, first, first, first, other] },
      },
      { value: base, raw },
    );
    if (restored.kind !== "calendar") throw new Error("Expected calendar fields");
    const rows = restored.period.custom;
    expect(rows[0]?.interval.raw).toEqual({ kind: "localInterval", ...changed });
    expect(rows[1]).toEqual(raw.period.custom[0]);
    expect(rows[2]).toEqual(raw.period.custom[2]);
    expect(rows[3]?.interval.raw).toEqual({ kind: "localInterval", ...first });
    expect(rows[4]).toEqual(raw.period.custom[1]);
    expect(new Set(rows.map((row) => row.key)).size).toBe(5);
    expect(raw.period.custom.map((row) => row.key)).not.toContain(rows[0]?.key);
    expect(raw.period.custom.map((row) => row.key)).not.toContain(rows[3]?.key);

    const unchanged = createWorkRecordDraft({ ...base, name: "Renamed" }, { value: base, raw });
    const inactive = createWorkRecordDraft(
      { ...base, period: { kind: "day", startTime: "08:00" } },
      { value: base, raw },
    );
    if (unchanged.kind !== "calendar" || inactive.kind !== "calendar")
      throw new Error("Expected calendar fields");
    expect(unchanged.period.custom).toEqual(raw.period.custom);
    expect(inactive.period.custom).toEqual(raw.period.custom);
    expect(inactive.period.day.startTime).toBe("08:00");
  });

  it("matches whole qualification minima with ordered duplicate consumption, never stale partial fields", () => {
    const first: WorkforceQualificationMinimum = {
      minimum: 1,
      qualifications: { allQualificationIds: [firstId], anyQualificationIds: [] },
    };
    const other: WorkforceQualificationMinimum = {
      minimum: 2,
      qualifications: { allQualificationIds: [], anyQualificationIds: [secondId] },
    };
    const base: WorkforceShiftTemplate = {
      ...template(),
      coverage: { kind: "exact", count: 3, qualificationMinimums: [first, other, first] },
    };
    const initial = createWorkRecordDraft(base);
    if (initial.kind !== "shiftTemplate") throw new Error("Expected template fields");
    const raw = {
      ...initial,
      coverage: {
        ...initial.coverage,
        qualificationMinimums: initial.coverage.qualificationMinimums.map((row, index) => ({
          ...row,
          key: index === 0 ? "first" : index === 1 ? "other" : "second",
          minimum: index === 0 ? "01" : index === 1 ? "02" : "001",
        })),
      },
    };
    const changedMinimum = { ...other, minimum: 3 };
    const changedQualifications = { ...other, qualifications: first.qualifications };
    const restored = createWorkRecordDraft(
      {
        ...base,
        coverage: {
          ...base.coverage,
          qualificationMinimums: [
            changedMinimum,
            first,
            first,
            first,
            other,
            changedQualifications,
          ],
        },
      },
      { value: base, raw },
    );
    if (restored.kind !== "shiftTemplate") throw new Error("Expected template fields");
    const rows = restored.coverage.qualificationMinimums;
    expect(rows[0]).toMatchObject({
      minimum: "3",
      allQualificationIds: [],
      anyQualificationIds: [secondId],
    });
    expect(rows[1]).toEqual(raw.coverage.qualificationMinimums[0]);
    expect(rows[2]).toEqual(raw.coverage.qualificationMinimums[2]);
    expect(rows[3]).toMatchObject({
      minimum: "1",
      allQualificationIds: [firstId],
      anyQualificationIds: [],
    });
    expect(rows[4]).toEqual(raw.coverage.qualificationMinimums[1]);
    expect(rows[5]).toMatchObject({
      minimum: "2",
      allQualificationIds: [firstId],
      anyQualificationIds: [],
    });
    expect(new Set(rows.map((row) => row.key)).size).toBe(6);
    const oldKeys = raw.coverage.qualificationMinimums.map((row) => row.key);
    expect(oldKeys).not.toContain(rows[0]?.key);
    expect(oldKeys).not.toContain(rows[3]?.key);
    expect(oldKeys).not.toContain(rows[5]?.key);
  });
});

describe("coverage requirement scope and rebase", () => {
  const base: WorkforceCoverageRequirement = {
    id,
    kind: "coverageRequirement",
    active: false,
    scope: { kind: "filter", assignmentTypeIds: [] },
    coverage: { kind: "atLeast", minimum: 3, qualificationMinimums: [] },
  };
  const preparation = { baseline: base, startsAt: null, endsAt: null };

  it("preserves an explicit empty filter without introducing an absent filter", () => {
    const draft = createWorkRecordDraft(base);
    const result = workRecordValue(id, draft, preparation);
    expect(result.value).toEqual(base);
    expect(result.errors).toEqual({});
  });

  it("rejects a partly authored enabled date range instead of broadening scope", () => {
    const draft = createWorkRecordDraft(base);
    if (draft.kind !== "coverageRequirement") throw new Error("Expected coverage requirement");
    const result = workRecordValue(
      id,
      {
        ...draft,
        scope: {
          ...draft.scope,
          filterDateRangeEnabled: true,
          filterStartDateRangeStart: "2030-01-01",
          filterStartDateRangeEndExclusive: "",
        },
      },
      preparation,
    );
    expect(result.value).toBeNull();
    expect(result.errors["scope.startDateRange.endDateExclusive"]).toBeDefined();
  });

  it("adopts changed coverage and scope while preserving inactive raw input", () => {
    const draft = createWorkRecordDraft(base);
    if (draft.kind !== "coverageRequirement") throw new Error("Expected coverage requirement");
    const raw = { ...draft, coverage: { ...draft.coverage, minimum: "003", count: "unfinished" } };
    const changed: WorkforceCoverageRequirement = {
      ...base,
      scope: { kind: "filter", assignmentTypeIds: [firstId], locationIds: [secondId] },
      coverage: { kind: "atLeast", minimum: 5, qualificationMinimums: [] },
    };
    const merged = rebaseEntityDraft(base, base, changed);
    const restored = createWorkRecordDraft(merged.value, { value: merged.local, raw });
    if (restored.kind !== "coverageRequirement") throw new Error("Expected coverage requirement");
    expect(restored.coverage.minimum).toBe("5");
    expect(restored.coverage.count).toBe("unfinished");
    expect(workRecordValue(id, restored, preparation).value).toEqual(changed);
  });

  it("does not reactivate inactive restrictions when adopting a different scope variant", () => {
    const original: WorkforceCoverageRequirement = { ...base, scope: { kind: "all" } };
    const draft = createWorkRecordDraft(original);
    if (draft.kind !== "coverageRequirement") throw new Error("Expected coverage requirement");
    const raw = {
      ...draft,
      scope: {
        ...draft.scope,
        filterAssignmentTypesEnabled: true,
        filterAssignmentTypeIds: [firstId],
        filterLocationsEnabled: true,
        filterLocationIds: [secondId],
        filterDateRangeEnabled: true,
        filterStartDateRangeStart: "2030-01-01",
        filterStartDateRangeEndExclusive: "2030-02-01",
      },
    };
    const current: WorkforceCoverageRequirement = { ...original, scope: { kind: "filter" } };
    const merged = rebaseEntityDraft(original, original, current);
    const restored = createWorkRecordDraft(merged.value, { value: merged.local, raw });
    if (restored.kind !== "coverageRequirement") throw new Error("Expected coverage requirement");
    expect(workRecordValue(id, restored, preparation).value).toEqual(current);
    expect(restored.scope.filterAssignmentTypeIds).toEqual([firstId]);
    expect(restored.scope.filterLocationIds).toEqual([secondId]);
    expect(restored.scope.filterStartDateRangeStart).toBe("2030-01-01");
  });

  it("retains inactive filter intent when a whole-field rebase adopts all shifts", () => {
    const original: WorkforceCoverageRequirement = {
      ...base,
      scope: {
        kind: "filter",
        assignmentTypeIds: [firstId],
        startDateRange: { startDate: "2030-01-01", endDateExclusive: "2030-02-01" },
      },
    };
    const raw = createWorkRecordDraft(original);
    if (raw.kind !== "coverageRequirement") throw new Error("Expected coverage requirement");
    const current: WorkforceCoverageRequirement = { ...original, scope: { kind: "all" } };
    const merged = rebaseEntityDraft(original, original, current);
    const restored = createWorkRecordDraft(merged.value, { value: merged.local, raw });
    if (restored.kind !== "coverageRequirement") throw new Error("Expected coverage requirement");
    expect(restored.scope.filterAssignmentTypeIds).toEqual([firstId]);
    expect(restored.scope.filterStartDateRangeStart).toBe("2030-01-01");
    expect(workRecordValue(id, restored, preparation).value).toEqual(current);
  });
});
