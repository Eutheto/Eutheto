import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  WorkforceAssignmentType,
  WorkforcePerson,
} from "./api/generated-domain-pack-contracts";
import { rebaseEntityDraft, resolveEntityDraftField } from "./entity-draft";
import { createPeopleRecordDraft, peopleRecordValue } from "./people-record-draft";

beforeEach(() => vi.stubGlobal("window", { crypto: globalThis.crypto }));
afterEach(() => vi.unstubAllGlobals());
const id = "01900000-0000-7000-8000-000000000001";
const qualificationId = "01900000-0000-7000-8000-000000000002";
const otherQualificationId = "01900000-0000-7000-8000-000000000003";

describe("rebased people raw buffers", () => {
  it("retains disabled invalid input and raw number spelling while adopting independent current fields", () => {
    const base: WorkforcePerson = {
      id,
      kind: "person",
      name: "Person",
      activeRange: { kind: "always" },
      qualificationGrants: [],
      eligibleAssignmentTypeIds: [],
      teamIds: [],
      tags: [],
      workloadWeight: { numerator: 2, denominator: 3 },
    };
    const initial = createPeopleRecordDraft(base);
    if (initial.kind !== "person") throw new Error("Expected person fields");
    const raw = {
      ...initial,
      externalId: "hidden external identity",
      fields: {
        ...initial.fields,
        startDate: "unfinished date",
        endDateExclusive: "unfinished end",
        weightNumerator: "02",
        target: "unfinished target",
        targetBucketId: qualificationId,
        targetCalendarId: otherQualificationId,
        color: "unfinished color",
        avatarInitials: "AB",
      },
    };
    const local = peopleRecordValue(id, raw).value;
    if (local?.kind !== "person") throw new Error("Expected representable inactive fields");
    const current = { ...base, tags: ["current-tag"] };
    const merged = rebaseEntityDraft(base, local, current);
    const restored = createPeopleRecordDraft(merged.value, { value: merged.local, raw });
    if (restored.kind !== "person") throw new Error("Expected rebased person fields");
    expect(restored.externalIdEnabled).toBe(false);
    expect(restored.externalId).toBe("hidden external identity");
    expect(restored.fields).toMatchObject({
      activeDatesEnabled: false,
      startDate: "unfinished date",
      endDateExclusive: "unfinished end",
      weightNumerator: "02",
      targetEnabled: false,
      target: "unfinished target",
      targetBucketId: qualificationId,
      targetCalendarId: otherQualificationId,
      displayEnabled: false,
      color: "unfinished color",
      avatarInitials: "AB",
    });
    expect(peopleRecordValue(id, restored).value).toEqual(current);
    expect(
      peopleRecordValue(id, {
        ...restored,
        fields: { ...restored.fields, targetEnabled: true },
      }).value,
    ).toBeNull();

    const withExternal = rebaseEntityDraft(base, local, {
      ...current,
      externalId: "current identity",
    });
    expect(
      createPeopleRecordDraft(withExternal.value, { value: withExternal.local, raw }),
    ).toMatchObject({ externalIdEnabled: true, externalId: "current identity" });
  });

  it("keeps inactive assignment policy selections unless an explicit conflict choice replaces that field", () => {
    const base: WorkforceAssignmentType = {
      id,
      kind: "assignmentType",
      name: "Shift",
      category: "care",
      defaultDurationMinutes: 480,
      qualifications: {
        kind: "matches",
        allQualificationIds: [qualificationId],
        anyQualificationIds: [],
      },
      locationBehavior: { kind: "fixed", locationId: otherQualificationId },
      timeBehavior: "elapsed",
      workloadBucketIds: [],
    };
    const initial = createPeopleRecordDraft(base);
    if (initial.kind !== "assignmentType") throw new Error("Expected assignment fields");
    const raw = {
      ...initial,
      qualificationMode: "unconstrained" as const,
      locationMode: "none" as const,
      duration: { raw: "8", unit: "hours" as const, status: "valid" as const, minutes: 480 },
    };
    const local = peopleRecordValue(id, raw).value;
    if (local?.kind !== "assignmentType")
      throw new Error("Expected representable assignment fields");
    const merged = rebaseEntityDraft(base, local, { ...base, name: "Current shift" });
    const restored = createPeopleRecordDraft(merged.value, { value: merged.local, raw });
    expect(restored).toMatchObject({
      name: "Current shift",
      qualificationMode: "unconstrained",
      allQualificationIds: [qualificationId],
      locationMode: "none",
      locationId: otherQualificationId,
      duration: { raw: "8", unit: "hours" },
    });
    expect(peopleRecordValue(id, restored).value).toEqual(merged.value);

    const conflict = rebaseEntityDraft(base, local, {
      ...base,
      qualifications: {
        kind: "matches",
        allQualificationIds: [otherQualificationId],
        anyQualificationIds: [],
      },
    });
    const selected = resolveEntityDraftField(conflict, "qualifications", "current");
    expect(createPeopleRecordDraft(selected.value, { value: selected.local, raw })).toMatchObject({
      qualificationMode: "matches",
      allQualificationIds: [otherQualificationId],
      locationMode: "none",
      locationId: otherQualificationId,
    });
  });
});
