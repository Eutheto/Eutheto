import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPersonFieldsDraft, personFieldsValue } from "./person-fields";
import type { PeopleCsvNewPersonDefaults } from "./api/generated";

beforeEach(() => vi.stubGlobal("window", { crypto: globalThis.crypto }));
afterEach(() => vi.unstubAllGlobals());

const defaults: PeopleCsvNewPersonDefaults = {
  activeRange: { kind: "dateRange", startDate: "2026-09-01", endDateExclusive: "2026-10-01" },
  qualificationGrants: [
    {
      qualificationId: "01900000-0000-7000-8000-000000000001",
      effectiveFrom: "2026-09-01T08:00:00+02:00",
    },
    {
      qualificationId: "01900000-0000-7000-8000-000000000001",
      effectiveFrom: "2026-10-01T08:00:00+02:00",
    },
  ],
  eligibleAssignmentTypeIds: [],
  teamIds: [],
  tags: [" ward "],
  workloadWeight: { numerator: 2, denominator: 3 },
  workloadTarget: {
    bucketId: "01900000-0000-7000-8000-000000000002",
    calendarId: "01900000-0000-7000-8000-000000000003",
    membership: "intersection",
    target: 480,
  },
  display: { color: "#123456", avatarInitials: "AB" },
};

describe("person field drafts", () => {
  it("preserves distinct temporal grants, explicit offsets, whitespace and every native field", () => {
    const draft = createPersonFieldsDraft(defaults);
    expect(draft.qualificationGrants[0]?.key).not.toBe(draft.qualificationGrants[1]?.key);
    expect(personFieldsValue(draft).value).toEqual(defaults);
  });

  it("keeps an incomplete numeric edit instead of coercing it to a valid proposal", () => {
    const draft = {
      ...createPersonFieldsDraft(defaults),
      weightNumerator: "2e",
      target: "4294967296",
    };
    const result = personFieldsValue(draft);
    expect(result.value).toBeNull();
    expect(Object.keys(result.errors)).toEqual(["weightNumerator", "target"]);
    expect(draft.weightNumerator).toBe("2e");
    expect(
      personFieldsValue({ ...draft, weightNumerator: "02", target: "0" }).value?.workloadTarget
        ?.target,
    ).toBe(0);
  });

  it("omits disabled optional fields without discarding their raw input", () => {
    const draft = {
      ...createPersonFieldsDraft(defaults),
      target: "unfinished",
      targetEnabled: false,
      displayEnabled: false,
      activeDatesEnabled: false,
    };
    const value = personFieldsValue(draft).value;
    expect(value).not.toHaveProperty("workloadTarget");
    expect(value).not.toHaveProperty("display");
    expect(value?.activeRange).toEqual({ kind: "always" });
    expect(personFieldsValue({ ...draft, targetEnabled: true }).value).toBeNull();
    expect(draft.startDate).toBe("2026-09-01");
    expect(draft.color).toBe("#123456");
  });
});
