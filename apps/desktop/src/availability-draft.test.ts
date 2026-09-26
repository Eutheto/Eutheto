import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkforceAvailability } from "./api/generated-domain-pack-contracts";
import {
  availabilityDraftValue,
  availabilityWeeklyDraft,
  createAvailabilityDraft,
} from "./availability-draft";
import { parseTemporalDraft } from "./components/planner/temporal-field";

beforeEach(() => vi.stubGlobal("window", { crypto: globalThis.crypto }));
afterEach(() => vi.unstubAllGlobals());

const record: WorkforceAvailability = {
  id: "01900000-0000-7000-8000-000000000020",
  kind: "availability",
  personId: "01900000-0000-7000-8000-000000000011",
  availabilityKind: "approvedTimeOff",
  effectiveRange: { startDate: "2030-01-01", endDateExclusive: "2030-01-03" },
  timeWindow: {
    kind: "instant",
    startsAt: "2030-01-01T09:00:00.123456789+05:30",
    endsAt: "2030-01-01T10:00:00.000000007+05:30",
  },
  source: "Imported clinic record",
  note: "Original note",
};

describe("availability draft representation", () => {
  it("does not rewrite stored instant precision or offsets when an unrelated field changes", () => {
    const draft = { ...createAvailabilityDraft(record, record.personId), note: "Changed note" };
    const result = availabilityDraftValue(record.id, draft, null);
    expect(result.value).toEqual({ ...record, note: "Changed note" });
  });

  it("refuses the entire draft rather than dropping an incomplete weekly interval", () => {
    const draft = {
      ...createAvailabilityDraft(record, record.personId),
      windowKind: "weekly" as const,
      weekly: [
        availabilityWeeklyDraft({
          startTime: "09:00",
          endTime: "10:00",
          endDayOffset: 0,
          weekdays: ["monday"],
        }),
        availabilityWeeklyDraft(),
      ],
    };
    const result = availabilityDraftValue(record.id, draft, null);
    expect(result.value).toBeNull();
    expect(result.errors["timeWindow.windows.1"]).toBeDefined();
    expect(draft.weekly[1]?.interval.raw).toEqual({
      kind: "localWindow",
      startTime: "",
      endTime: "",
      endDayOffset: "0",
    });
  });

  it("does not wrap an out-of-range day offset into a different weekly interval", () => {
    const row = availabilityWeeklyDraft({
      startTime: "09:00",
      endTime: "10:00",
      endDayOffset: 0,
      weekdays: ["monday"],
    });
    const raw = {
      kind: "localWindow" as const,
      startTime: "09:00",
      endTime: "10:00",
      endDayOffset: "000256",
    };
    const draft = {
      ...createAvailabilityDraft(record, record.personId),
      windowKind: "weekly" as const,
      weekly: [{ ...row, interval: parseTemporalDraft(raw) }],
    };
    expect(availabilityDraftValue(record.id, draft, null).value).toBeNull();
    expect(draft.weekly[0]?.interval.raw).toEqual(raw);
  });

  it("requires native resolution for exactly the local endpoints being approved", () => {
    const raw = {
      kind: "localInterval" as const,
      startsAt: "2030-01-01T09:00",
      endsAt: "2030-01-01T10:00",
    };
    const draft = {
      ...createAvailabilityDraft(record, record.personId),
      replaceInstant: true,
      localInterval: parseTemporalDraft(raw),
    };
    expect(availabilityDraftValue(record.id, draft, null).value).toBeNull();
    const resolved = {
      raw: { startsAt: raw.startsAt, endsAt: raw.endsAt },
      startsAt: {
        local: "2030-01-01T09:00:00",
        instant: "2030-01-01T14:00:00Z",
        offsetSeconds: -18_000,
      },
      endsAt: {
        local: "2030-01-01T10:00:00",
        instant: "2030-01-01T15:00:00Z",
        offsetSeconds: -18_000,
      },
    };
    expect(availabilityDraftValue(record.id, draft, resolved).value?.timeWindow).toEqual({
      kind: "instant",
      startsAt: resolved.startsAt.instant,
      endsAt: resolved.endsAt.instant,
    });
    const edited = {
      ...draft,
      localInterval: parseTemporalDraft({ ...raw, endsAt: "2030-01-01T11:00" }),
    };
    expect(availabilityDraftValue(record.id, edited, resolved).value).toBeNull();
    expect(edited.localInterval.raw).toEqual({ ...raw, endsAt: "2030-01-01T11:00" });
  });
});
