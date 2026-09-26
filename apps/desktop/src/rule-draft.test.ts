import { describe, expect, it } from "vitest";
import { parseDurationDraft } from "./components/planner/duration-field";
import {
  createRuleDraft,
  parseMaximumAssignmentCountDraft,
  parseRequiredSkillMixMinimumDraft,
  ruleDraftValue,
  ruleRebaseValue,
  type EditableRule,
} from "./rule-draft";
import { rebaseEntityDraft } from "./entity-draft";

const rest: EditableRule & { readonly kind: "minimumRest" } = {
  id: "01900000-0000-7000-8000-000000000040",
  kind: "minimumRest",
  active: true,
  strength: "required",
  scope: { people: { kind: "all" } },
  beforeScope: { people: { kind: "all" }, categories: ["overnight"] },
  afterScope: { people: { kind: "all" }, categories: ["clinic"] },
  minimumMinutes: 600,
};
const count: EditableRule & { readonly kind: "maximumAssignmentCount" } = {
  id: "01900000-0000-7000-8000-000000000041",
  kind: "maximumAssignmentCount",
  active: true,
  strength: "required",
  scope: { people: { kind: "all" } },
  calendarId: "01900000-0000-7000-8000-000000000042",
  maximum: 2,
};
const skillMix: EditableRule & { readonly kind: "requiredSkillMix" } = {
  id: "01900000-0000-7000-8000-000000000043",
  kind: "requiredSkillMix",
  active: true,
  strength: "required",
  scope: { people: { kind: "all" } },
  qualificationMinimums: [
    {
      qualifications: {
        allQualificationIds: ["01900000-0000-7000-8000-000000000044"],
        anyQualificationIds: [],
      },
      minimum: 1,
    },
  ],
};
const fixed: EditableRule & { readonly kind: "fixedAssignment" } = {
  id: "01900000-0000-7000-8000-000000000046",
  kind: "fixedAssignment",
  active: true,
  strength: "required",
  scope: { people: { kind: "all" } },
  personId: "01900000-0000-7000-8000-000000000047",
  shiftId: "01900000-0000-7000-8000-000000000048",
};

describe("Required rule raw duration custody", () => {
  it("keeps invalid raw duration after adopting unrelated native changes and refuses submission", () => {
    const raw = {
      ...createRuleDraft(rest),
      minimumRest: parseDurationDraft("10.001", "hours", 0),
    };
    const current = { ...rest, active: false };
    const local = ruleRebaseValue(rest, raw);
    expect(local).not.toBeNull();
    if (local === null) throw new Error("Incomplete duration must not block rebasing");
    const merged = rebaseEntityDraft(rest, local, current);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.active).toBe(false);
    expect(rebased.minimumRest).toEqual(raw.minimumRest);
    expect(ruleDraftValue(rest.id, rest.kind, rebased).value).toBeNull();
    const repaired = { ...rebased, minimumRest: parseDurationDraft("10.5", "hours", 0) };
    expect(ruleDraftValue(rest.id, rest.kind, repaired).value).toEqual({
      ...current,
      minimumMinutes: 630,
    });
  });

  it("keeps incomplete duration input when the saved duration changes concurrently", () => {
    const raw = { ...createRuleDraft(rest), minimumRest: parseDurationDraft("", "hours", 0) };
    const current = { ...rest, minimumMinutes: 720 };
    const local = ruleRebaseValue(rest, raw);
    if (local === null) throw new Error("Incomplete duration must not block rebasing");
    const merged = rebaseEntityDraft(rest, local, current);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.minimumRest).toEqual(raw.minimumRest);
    expect(ruleDraftValue(rest.id, rest.kind, rebased).value).toBeNull();
    expect(
      ruleDraftValue(rest.id, rest.kind, {
        ...rebased,
        minimumRest: parseDurationDraft("11", "hours", 0),
      }).value,
    ).toEqual({ ...current, minimumMinutes: 660 });
  });

  it("adopts changed native duration rather than retaining a conflicting display value", () => {
    const raw = {
      ...createRuleDraft(rest),
      minimumRest: parseDurationDraft("10.00", "hours", 0),
    };
    const current = { ...rest, minimumMinutes: 601 };
    const rebased = createRuleDraft(current, { value: rest, raw });
    expect(rebased.minimumRest).toEqual(parseDurationDraft("601", "minutes", 0));
    expect(ruleDraftValue(rest.id, rest.kind, rebased).value).toEqual(current);
  });
});
describe("Maximum assignment count raw custody", () => {
  it("parses exact u32 boundaries and rejects syntax or overflow", () => {
    expect(parseMaximumAssignmentCountDraft("0")).toEqual({
      raw: "0",
      status: "valid",
      maximum: 0,
    });
    expect(parseMaximumAssignmentCountDraft("4294967295")).toEqual({
      raw: "4294967295",
      status: "valid",
      maximum: 4_294_967_295,
    });
    expect(parseMaximumAssignmentCountDraft("4294967296")).toEqual({
      raw: "4294967296",
      status: "invalid",
      error: "range",
    });
    expect(parseMaximumAssignmentCountDraft("2e")).toEqual({
      raw: "2e",
      status: "invalid",
      error: "syntax",
    });
    expect(parseMaximumAssignmentCountDraft(`${"0".repeat(32)}1`)).toEqual({
      raw: `${"0".repeat(32)}1`,
      status: "invalid",
      error: "range",
    });
  });
  it("requires a selected calendar instead of accepting a blank reference", () => {
    const draft = { ...createRuleDraft(count), calendarId: "" };
    const result = ruleDraftValue(count.id, count.kind, draft);
    expect(result.value).toBeNull();
    expect(result.errors.calendarId).toBeDefined();
  });
  it("retains a valid cap and flags a concurrent cap change while its calendar is incomplete", () => {
    const raw = {
      ...createRuleDraft(count),
      calendarId: "",
      maximum: parseMaximumAssignmentCountDraft("3"),
    };
    const local = ruleRebaseValue(count, raw);
    if (local === null) throw new Error("Incomplete calendar must not block rebasing");
    const merged = rebaseEntityDraft(count, local, { ...count, maximum: 4 });
    expect(merged.conflicts).toContain("maximum");
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.calendarId).toBe("");
    expect(rebased.maximum).toEqual(raw.maximum);
    expect(ruleDraftValue(count.id, count.kind, rebased).value).toBeNull();
  });

  it("keeps invalid raw maximum through a rebase and preserves its native error path", () => {
    const raw = {
      ...createRuleDraft(count),
      maximum: parseMaximumAssignmentCountDraft("2e"),
    };
    const current = { ...count, active: false, maximum: 3 };
    const local = ruleRebaseValue(count, raw);
    expect(local).not.toBeNull();
    if (local === null) throw new Error("Invalid maximum must not block rebasing");
    const merged = rebaseEntityDraft(count, local, current);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.active).toBe(false);
    expect(rebased.maximum).toEqual(raw.maximum);
    expect(ruleDraftValue(count.id, count.kind, rebased).value).toBeNull();
    expect(ruleDraftValue(count.id, count.kind, rebased).errors.maximum).toBeDefined();
    expect(
      ruleDraftValue(count.id, count.kind, {
        ...rebased,
        maximum: parseMaximumAssignmentCountDraft("0"),
      }).value,
    ).toEqual({ ...current, maximum: 0 });
  });
});

describe("Required skill mix raw qualification minima", () => {
  it("enforces exact unsigned 16-bit bounds without rounding syntax", () => {
    expect(parseRequiredSkillMixMinimumDraft("00065535")).toEqual({
      raw: "00065535",
      status: "valid",
      minimum: 65535,
    });
    expect(parseRequiredSkillMixMinimumDraft("0")).toMatchObject({
      status: "valid",
      minimum: 0,
    });
    expect(parseRequiredSkillMixMinimumDraft("65536")).toMatchObject({
      status: "invalid",
      error: "range",
    });
    expect(parseRequiredSkillMixMinimumDraft("1e2")).toMatchObject({
      status: "invalid",
      error: "syntax",
    });
    expect(parseRequiredSkillMixMinimumDraft("")).toMatchObject({ status: "empty" });
  });

  it("retains invalid raw intent while adopting a concurrent qualification change", () => {
    const raw = createRuleDraft(skillMix);
    const first = raw.qualificationMinimums[0];
    if (first === undefined) throw new Error("Missing saved skill minimum");
    const incomplete = {
      ...raw,
      qualificationMinimums: [{ ...first, minimum: "1e2" }],
    };
    const current: typeof skillMix = {
      ...skillMix,
      active: false,
      qualificationMinimums: [
        {
          qualifications: {
            allQualificationIds: [],
            anyQualificationIds: ["01900000-0000-7000-8000-000000000045"],
          },
          minimum: 2,
        },
      ],
    };
    const local = ruleRebaseValue(skillMix, incomplete);
    if (local === null) throw new Error("Incomplete minimum must not block rebase");
    const merged = rebaseEntityDraft(skillMix, local, current);
    const restored = createRuleDraft(merged.value, { value: local, raw: incomplete });
    expect(restored.active).toBe(false);
    expect(restored.qualificationMinimums[0]).toMatchObject({
      minimum: "1e2",
      allQualificationIds: [],
      anyQualificationIds: ["01900000-0000-7000-8000-000000000045"],
    });
    const restoredRow = restored.qualificationMinimums[0];
    if (restoredRow === undefined) throw new Error("Rebase lost skill minimum");
    expect(ruleDraftValue(skillMix.id, skillMix.kind, restored).value).toBeNull();
    expect(
      ruleDraftValue(skillMix.id, skillMix.kind, {
        ...restored,
        qualificationMinimums: [{ ...restoredRow, minimum: "3" }],
      }).value,
    ).toEqual({
      ...current,
      qualificationMinimums: [{ ...current.qualificationMinimums[0], minimum: 3 }],
    });
  });

  it("keeps a whole-array conflict when both writers change qualification minima", () => {
    const raw = createRuleDraft(skillMix);
    const first = raw.qualificationMinimums[0];
    if (first === undefined) throw new Error("Missing saved skill minimum");
    const local = ruleRebaseValue(skillMix, {
      ...raw,
      qualificationMinimums: [{ ...first, minimum: "3" }],
    });
    if (local === null || local.kind !== "requiredSkillMix")
      throw new Error("Valid minimum must rebase");
    const saved = skillMix.qualificationMinimums[0];
    if (saved === undefined) throw new Error("Missing authored skill minimum");
    const current: typeof skillMix = {
      ...skillMix,
      qualificationMinimums: [{ ...saved, minimum: 2 }],
    };
    const merged = rebaseEntityDraft(skillMix, local, current);
    expect(merged.conflicts).toContain("qualificationMinimums");
    expect(merged.value.qualificationMinimums).toEqual(local.qualificationMinimums);
  });
  it("matches reordered duplicate qualification expressions in authored order during incomplete rebase", () => {
    const first = skillMix.qualificationMinimums[0];
    if (first === undefined) throw new Error("Missing qualification minimum");
    const second = {
      qualifications: {
        allQualificationIds: [],
        anyQualificationIds: ["01900000-0000-7000-8000-000000000045"],
      },
      minimum: 3,
    };
    const base: typeof skillMix = {
      ...skillMix,
      qualificationMinimums: [first, { ...first, minimum: 2 }, second],
    };
    const rows = createRuleDraft(base).qualificationMinimums;
    const [rowA, rowB, rowC] = rows;
    if (!rowA || !rowB || !rowC) throw new Error("Missing authored minimum rows");
    const local = ruleRebaseValue(base, {
      ...createRuleDraft(base),
      qualificationMinimums: [
        { ...rowC, minimum: "" },
        { ...rowA, minimum: "invalid" },
        { ...rowB, minimum: "" },
      ],
    });
    if (local?.kind !== "requiredSkillMix") throw new Error("Incomplete rows must rebase");
    expect(local.qualificationMinimums).toEqual([second, first, { ...first, minimum: 2 }]);
  });
});
describe("Fixed assignment target custody", () => {
  it("uses the full scope by default and produces the exact named pair", () => {
    expect(createRuleDraft("new").scope).toEqual({ people: { kind: "all" } });
    expect(ruleDraftValue(fixed.id, fixed.kind, createRuleDraft(fixed)).value).toEqual(fixed);
  });

  it("lets native admission decide whether an inactive target may be absent", () => {
    const result = ruleDraftValue(fixed.id, fixed.kind, {
      ...createRuleDraft(fixed),
      active: false,
      personId: "",
      shiftId: "",
    });
    expect(result.errors).toEqual({});
    expect(result.value).toEqual({ ...fixed, active: false, personId: "", shiftId: "" });
  });

  it("retains unselected target custody while adopting unrelated saved target changes", () => {
    const raw = { ...createRuleDraft(fixed), personId: "", shiftId: "" };
    const local = ruleRebaseValue(fixed, raw);
    if (local === null) throw new Error("Unselected targets must not block rebasing");
    const current = {
      ...fixed,
      personId: "01900000-0000-7000-8000-000000000049",
      shiftId: "01900000-0000-7000-8000-00000000004a",
    };
    const merged = rebaseEntityDraft(fixed, local, current);
    const restored = createRuleDraft(merged.value, { value: local, raw });
    expect(restored.personId).toBe("");
    expect(restored.shiftId).toBe("");
    expect(ruleDraftValue(fixed.id, fixed.kind, restored).errors).toEqual({
      personId: expect.any(String),
      shiftId: expect.any(String),
    });
  });

  it("treats each named target as a whole-field rebase conflict", () => {
    const local = ruleRebaseValue(fixed, {
      ...createRuleDraft(fixed),
      personId: "01900000-0000-7000-8000-000000000049",
    });
    if (local === null) throw new Error("Valid fixed target must rebase");
    const merged = rebaseEntityDraft(fixed, local, {
      ...fixed,
      personId: "01900000-0000-7000-8000-00000000004a",
    });
    expect(merged.conflicts).toContain("personId");
    if (merged.value.kind !== "fixedAssignment") throw new Error("Wrong rule kind");
    expect(merged.value.personId).toBe("01900000-0000-7000-8000-000000000049");
  });
});

const mutual: EditableRule & { readonly kind: "mutualAssignmentRestriction" } = {
  id: "01900000-0000-7000-8000-00000000004b",
  kind: "mutualAssignmentRestriction",
  active: true,
  strength: "required",
  scope: { people: { kind: "all" } },
  personIds: ["01900000-0000-7000-8000-000000000047", "01900000-0000-7000-8000-000000000049"],
  mode: "sameShift",
};

describe("Mutual assignment restriction draft custody", () => {
  it("preserves both native modes and the whole selected-person set", () => {
    const saved = ruleDraftValue(mutual.id, mutual.kind, createRuleDraft(mutual));
    expect(saved.value).toEqual(mutual);
    expect(
      ruleDraftValue(mutual.id, mutual.kind, {
        ...createRuleDraft(mutual),
        mode: "overlappingShifts",
      }).value,
    ).toEqual({ ...mutual, mode: "overlappingShifts" });
  });

  it("adopts an independent native mode edit without dropping local selected people", () => {
    const raw = {
      ...createRuleDraft(mutual),
      personIds: [...mutual.personIds, "01900000-0000-7000-8000-00000000004a"],
    };
    const local = ruleRebaseValue(mutual, raw);
    if (local === null) throw new Error("Edited person set must be rebasable");
    const current = { ...mutual, mode: "overlappingShifts" as const };
    const merged = rebaseEntityDraft(mutual, local, current);
    expect(merged.conflicts).toEqual([]);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(ruleDraftValue(mutual.id, mutual.kind, rebased).value).toEqual({
      ...current,
      personIds: raw.personIds,
    });
  });

  it("keeps conflicting selected sets whole and preserves legacy singleton input", () => {
    const [firstPerson, secondPerson] = mutual.personIds;
    if (firstPerson === undefined || secondPerson === undefined)
      throw new Error("Mutual draft fixture needs two people");
    const raw = {
      ...createRuleDraft(mutual),
      personIds: [firstPerson],
    };
    const local = ruleRebaseValue(mutual, raw);
    if (local === null) throw new Error("Existing singleton rule remains readable");
    const current = {
      ...mutual,
      personIds: [secondPerson, "01900000-0000-7000-8000-00000000004a"],
    };
    const merged = rebaseEntityDraft(mutual, local, current);
    expect(merged.conflicts).toContain("personIds");
    if (merged.value.kind !== "mutualAssignmentRestriction")
      throw new Error("Rebase must retain mutual rule kind");
    expect(merged.value.personIds).toEqual(raw.personIds);
    expect(
      ruleDraftValue(mutual.id, mutual.kind, {
        ...createRuleDraft(merged.value, { value: local, raw }),
        active: false,
      }).value,
    ).toEqual({ ...merged.value, active: false });
  });
});
