import { describe, expect, it } from "vitest";
import { rebaseEntityDraft, resolveEntityDraftField } from "./entity-draft";
import {
  createScorePolicyDraft,
  scorePolicyDraftValue,
  type ScorePolicyValue,
} from "./score-policy-draft";

const base: ScorePolicyValue = {
  id: "018f0000-0000-7000-8000-000000000001",
  kind: "scorePolicy",
  profileKey: "default",
  tieBreak: "stableAssignmentRank",
  levels: [
    { levelKey: "primary", label: "Primary" },
    { levelKey: "secondary", label: "Secondary" },
  ],
  priorityMapping: [
    { levelKey: "primary", priority: "low", scale: 1 },
    { levelKey: "primary", priority: "normal", scale: 2 },
    { levelKey: "secondary", priority: "high", scale: 3 },
    { levelKey: "secondary", priority: "veryHigh", scale: 4 },
  ],
  workloadPolicies: {
    "018f0000-0000-7000-8000-000000000002": {
      id: "018f0000-0000-7000-8000-000000000002",
      bucketId: "018f0000-0000-7000-8000-000000000003",
      calendarId: "018f0000-0000-7000-8000-000000000004",
      membership: "reportingDate",
      peerGroup: { people: { kind: "all" } },
      targetMode: { kind: "equalShare" },
      penalty: { kind: "piecewise", breakpoints: [{ fromDeviation: 0, slope: 2 }] },
    },
  },
};
const baseWorkloadPolicy = base.workloadPolicies["018f0000-0000-7000-8000-000000000002"];
if (baseWorkloadPolicy === undefined) throw new Error("Fixture workload policy is missing.");

describe("score policy draft", () => {
  it("rebases independent top-level fields without losing local intent", () => {
    const local: ScorePolicyValue = { ...base, profileKey: "local-profile" };
    const current: ScorePolicyValue = {
      ...base,
      levels: [{ levelKey: "primary", label: "Primary" }],
    };
    const merged = rebaseEntityDraft(base, local, current);

    expect(merged.conflicts).toEqual([]);
    expect(merged.value.profileKey).toBe("local-profile");
    expect(merged.value.levels).toEqual(current.levels);
  });

  it("treats nested workload policy changes as one conflict", () => {
    const local: ScorePolicyValue = {
      ...base,
      workloadPolicies: {
        ...base.workloadPolicies,
        "018f0000-0000-7000-8000-000000000002": {
          ...baseWorkloadPolicy,
          membership: "intersection",
        },
      },
    };
    const current: ScorePolicyValue = {
      ...base,
      workloadPolicies: {
        ...base.workloadPolicies,
        "018f0000-0000-7000-8000-000000000002": {
          ...baseWorkloadPolicy,
          penalty: { kind: "absolute" },
        },
      },
    };
    const merged = rebaseEntityDraft(base, local, current);

    expect(merged.conflicts).toEqual(["workloadPolicies"]);
    expect(
      resolveEntityDraftField(merged, "workloadPolicies", "current").value.workloadPolicies,
    ).toEqual(current.workloadPolicies);
    expect(
      resolveEntityDraftField(merged, "workloadPolicies", "draft").value.workloadPolicies,
    ).toEqual(local.workloadPolicies);
  });

  it("keeps raw numeric input until the candidate is requested", () => {
    const draft = createScorePolicyDraft(base);
    const edited = {
      ...draft,
      priorityMapping: draft.priorityMapping.map((mapping, index) =>
        index === 0 ? { ...mapping, scale: "0007" } : mapping,
      ),
    };
    const result = scorePolicyDraftValue(base.id, edited);

    expect(result.errors).toEqual({});
    expect(result.value?.priorityMapping[0]?.scale).toBe(7);
    expect(edited.priorityMapping[0]?.scale).toBe("0007");
  });

  it("rejects invalid raw numeric input without mutating the candidate", () => {
    const draft = createScorePolicyDraft(base);
    const edited = {
      ...draft,
      priorityMapping: draft.priorityMapping.map((mapping, index) =>
        index === 0 ? { ...mapping, scale: "not-a-number" } : mapping,
      ),
    };
    const result = scorePolicyDraftValue(base.id, edited);

    expect(result.value).toBeNull();
    expect(result.errors["priorityMapping.0.scale"]).toBe("Enter a whole number.");
    expect(edited.priorityMapping[0]?.scale).toBe("not-a-number");
  });

  it("retains valid raw text when an unchanged authoritative read refreshes the draft", () => {
    const draft = createScorePolicyDraft(base);
    const edited = {
      ...draft,
      priorityMapping: draft.priorityMapping.map((mapping, index) =>
        index === 0 ? { ...mapping, scale: "0007" } : mapping,
      ),
    };
    const refreshed = createScorePolicyDraft(base, { value: base, raw: edited });

    expect(refreshed.priorityMapping[0]?.scale).toBe("0007");
  });
});
