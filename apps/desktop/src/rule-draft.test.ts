import { describe, expect, it } from "vitest";
import { parseDurationDraft } from "./components/planner/duration-field";
import { rebaseEntityDraft, resolveEntityDraftField } from "./entity-draft";
import {
  createRuleDraft,
  parseMaximumAssignmentCountDraft,
  parseMaximumHoursDurationDraft,
  parseMaximumHoursMaximumMinutesDraft,
  parseRequiredSkillMixMinimumDraft,
  ruleDraftValue,
  ruleRebaseValue,
  type EditableRule,
} from "./rule-draft";
import { messages } from "./messages";

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
const weekends: EditableRule & { readonly kind: "maximumWorkedWeekends" } = {
  id: "01900000-0000-7000-8000-000000000053",
  kind: "maximumWorkedWeekends",
  active: true,
  strength: "required",
  scope: { people: { kind: "all" } },
  maximum: 2,
};
const quota: EditableRule & { readonly kind: "workloadQuota" } = {
  id: "01900000-0000-7000-8000-000000000054",
  kind: "workloadQuota",
  active: true,
  strength: "required",
  scope: { people: { kind: "all" } },
  calendarId: count.calendarId,
  membership: "reportingDate",
  measure: { kind: "assignmentCount" },
  minimum: 1,
  maximum: 2,
};
const consecutive: EditableRule & { readonly kind: "maximumConsecutive" } = {
  id: "01900000-0000-7000-8000-000000000052",
  kind: "maximumConsecutive",
  active: true,
  strength: "required",
  scope: { people: { kind: "all" } },
  maximum: 3,
  mode: { kind: "workedDays" },
};

const hours: EditableRule & { readonly kind: "maximumHours" } = {
  id: "01900000-0000-7000-8000-000000000049",
  kind: "maximumHours",
  active: true,
  strength: "required",
  scope: { people: { kind: "all" } },
  bucketId: "01900000-0000-7000-8000-000000000050",
  maximumMinutes: 480,
  window: {
    kind: "calendar",
    calendarId: "01900000-0000-7000-8000-000000000051",
    membership: "reportingDate",
  },
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
describe("Maximum worked weekends raw custody", () => {
  it("accepts zero, rejects missing input, and preserves invalid raw syntax", () => {
    const initial = createRuleDraft(weekends);
    expect(ruleDraftValue(weekends.id, weekends.kind, initial).value).toEqual(weekends);
    expect(
      ruleDraftValue(weekends.id, weekends.kind, {
        ...initial,
        maximum: parseMaximumAssignmentCountDraft(""),
      }),
    ).toMatchObject({
      value: null,
      errors: { maximum: messages.ruleSetup.maximumInvalid },
    });
    expect(
      ruleDraftValue(weekends.id, weekends.kind, {
        ...initial,
        maximum: parseMaximumAssignmentCountDraft("2e"),
      }),
    ).toMatchObject({
      value: null,
      errors: { maximum: messages.ruleSetup.maximumInvalid },
    });
    expect(
      ruleDraftValue(weekends.id, weekends.kind, {
        ...initial,
        maximum: parseMaximumAssignmentCountDraft("0"),
      }).value,
    ).toEqual({ ...weekends, maximum: 0 });
  });

  it("retains invalid weekend caps through concurrent updates and accepts a repaired zero", () => {
    const raw = {
      ...createRuleDraft(weekends),
      maximum: parseMaximumAssignmentCountDraft("2e"),
    };
    const local = ruleRebaseValue(weekends, raw);
    expect(local).not.toBeNull();
    if (local === null) throw new Error("Invalid weekend maximum must not block rebasing");
    const current = { ...weekends, active: false, maximum: 4 };
    const merged = rebaseEntityDraft(weekends, local, current);
    if (merged.value.kind !== "maximumWorkedWeekends") throw new Error("Rebased rule changed kind");
    expect(merged.value.maximum).toBe(4);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.active).toBe(false);
    expect(rebased.maximum).toEqual(raw.maximum);
    expect(ruleDraftValue(weekends.id, weekends.kind, rebased).value).toBeNull();
    expect(
      ruleDraftValue(weekends.id, weekends.kind, {
        ...rebased,
        maximum: parseMaximumAssignmentCountDraft("0"),
      }).value,
    ).toEqual({ ...current, maximum: 0 });
  });

  it("adopts a concurrent valid cap when the weekend draft is untouched", () => {
    const changed = { ...weekends, maximum: 4 };
    const rebased = createRuleDraft(changed, { value: weekends, raw: createRuleDraft(weekends) });
    expect(rebased.maximum).toEqual(parseMaximumAssignmentCountDraft("4"));
    expect(ruleDraftValue(weekends.id, weekends.kind, rebased).value).toEqual(changed);
  });
});
describe("Required workload quota editor", () => {
  it("submits exact count ranges, optional maximum, and named minute bucket without inferred credits", () => {
    const initial = createRuleDraft(quota);
    expect(ruleDraftValue(quota.id, quota.kind, initial).value).toEqual(quota);
    const exact = {
      ...initial,
      quotaMinimum: parseMaximumAssignmentCountDraft("2"),
      quotaMaximum: parseMaximumAssignmentCountDraft("2"),
    };
    expect(ruleDraftValue(quota.id, quota.kind, exact).value).toEqual({
      ...quota,
      minimum: 2,
      maximum: 2,
    });
    const unbounded = { ...initial, quotaMaximum: parseMaximumAssignmentCountDraft("") };
    expect(ruleDraftValue(quota.id, quota.kind, unbounded).value).toEqual({
      ...quota,
      maximum: undefined,
      minimum: 1,
    });
    expect(
      Object.hasOwn(ruleDraftValue(quota.id, quota.kind, unbounded).value ?? {}, "maximum"),
    ).toBe(false);
    const minute = {
      ...initial,
      quotaMeasure: "bucketMinutes" as const,
      quotaMembership: "intersection" as const,
      bucketId: hours.bucketId,
    };
    expect(ruleDraftValue(quota.id, quota.kind, minute).value).toEqual({
      ...quota,
      measure: { kind: "bucketMinutes", bucketId: hours.bucketId },
      membership: "intersection",
    });
  });

  it("rejects count intersections and inverted ranges; retains incomplete raw through a concurrent rebase", () => {
    const initial = createRuleDraft(quota);
    const invalid = {
      ...initial,
      quotaMembership: "intersection" as const,
      quotaMinimum: parseMaximumAssignmentCountDraft("3"),
      quotaMaximum: parseMaximumAssignmentCountDraft("2"),
    };
    expect(ruleDraftValue(quota.id, quota.kind, invalid).errors).toMatchObject({
      maximum: messages.ruleSetup.quotaRangeInvalid,
      membership: messages.ruleSetup.quotaCountMembershipInvalid,
    });
    const raw = {
      ...initial,
      quotaMinimum: parseMaximumAssignmentCountDraft("3e"),
      calendarId: "",
    };
    const local = ruleRebaseValue(quota, raw);
    if (local === null)
      throw new Error("Incomplete quota input must not block full-record rebasing");
    const current = { ...quota, active: false, maximum: 4 };
    const merged = rebaseEntityDraft(quota, local, current);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.active).toBe(false);
    expect(rebased.quotaMinimum).toEqual(raw.quotaMinimum);
    expect(rebased.calendarId).toBe("");
    expect(rebased.quotaMaximum).toEqual(parseMaximumAssignmentCountDraft("4"));
    expect(ruleDraftValue(quota.id, quota.kind, rebased).value).toBeNull();
  });
  it("adopts a concurrent maximum change instead of treating cleared optional text as invalid", () => {
    const raw = {
      ...createRuleDraft(quota),
      quotaMaximum: parseMaximumAssignmentCountDraft(""),
    };
    const changed = { ...quota, maximum: 4 };
    const rebased = createRuleDraft(changed, { value: quota, raw });
    expect(rebased.quotaMaximum).toEqual(parseMaximumAssignmentCountDraft("4"));
    expect(ruleDraftValue(quota.id, quota.kind, rebased).value).toEqual(changed);
  });
});

describe("Maximum consecutive raw mode and cap custody", () => {
  it("round-trips worked-day and assignment modes while keeping a blank new cap", () => {
    expect(createRuleDraft("new").maximum).toEqual({ raw: "", status: "empty" });
    expect(createRuleDraft(consecutive).mode).toEqual({ kind: "workedDays" });
    expect(
      ruleDraftValue(consecutive.id, consecutive.kind, createRuleDraft(consecutive)).value,
    ).toEqual(consecutive);
    const assignments = {
      ...createRuleDraft(consecutive),
      maximum: parseMaximumAssignmentCountDraft("0"),
      mode: {
        kind: "assignments" as const,
        breakMinutes: parseMaximumAssignmentCountDraft("000"),
      },
    };
    expect(ruleDraftValue(consecutive.id, consecutive.kind, assignments).value).toEqual({
      ...consecutive,
      maximum: 0,
      mode: { kind: "assignments", breakMinutes: 0 },
    });
  });
  it("keeps an inactive break draft while adopting a changed native mode", () => {
    const raw = {
      ...createRuleDraft(consecutive),
      mode: {
        kind: "workedDays" as const,
        breakMinutes: parseMaximumAssignmentCountDraft("1.5"),
      },
    };
    expect(createRuleDraft(consecutive, { value: consecutive, raw }).mode).toEqual(raw.mode);
    const local = ruleRebaseValue(consecutive, raw);
    if (local === null) throw new Error("Worked-days raw mode must rebase");
    const current = {
      ...consecutive,
      mode: { kind: "assignments" as const, breakMinutes: 15 },
    };
    const merged = rebaseEntityDraft(consecutive, local, current);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.mode).toEqual({
      kind: "assignments",
      breakMinutes: parseMaximumAssignmentCountDraft("15"),
    });
    expect(ruleDraftValue(consecutive.id, consecutive.kind, rebased).value).toEqual(current);
  });

  it("rejects negative, fractional and overflowing assignment breaks", () => {
    for (const rawBreak of ["-1", "1.5", "4294967296"]) {
      const result = ruleDraftValue(consecutive.id, consecutive.kind, {
        ...createRuleDraft(consecutive),
        mode: {
          kind: "assignments",
          breakMinutes: parseMaximumAssignmentCountDraft(rawBreak),
        },
      });
      expect(result.value).toBeNull();
      expect(result.errors["mode.breakMinutes"]).toBeDefined();
    }
    expect(
      ruleDraftValue(consecutive.id, consecutive.kind, {
        ...createRuleDraft(consecutive),
        mode: {
          kind: "assignments",
          breakMinutes: parseMaximumAssignmentCountDraft("4294967295"),
        },
      }).value,
    ).toEqual({
      ...consecutive,
      mode: { kind: "assignments", breakMinutes: 4_294_967_295 },
    });
  });

  it("preserves invalid cap and break input through a concurrent rebase", () => {
    const raw = {
      ...createRuleDraft(consecutive),
      maximum: parseMaximumAssignmentCountDraft("2e"),
      mode: {
        kind: "assignments" as const,
        breakMinutes: parseMaximumAssignmentCountDraft(""),
      },
    };
    const local = ruleRebaseValue(consecutive, raw);
    expect(local).not.toBeNull();
    if (local === null) throw new Error("Invalid consecutive fields must not block rebasing");
    const current = {
      ...consecutive,
      active: false,
      maximum: 4,
      mode: { kind: "assignments" as const, breakMinutes: 9 },
    };
    const merged = rebaseEntityDraft(consecutive, local, current);
    const restored = createRuleDraft(merged.value, { value: local, raw });
    expect(restored.active).toBe(false);
    expect(restored.maximum).toEqual(raw.maximum);
    expect(restored.mode).toEqual(raw.mode);
    const invalid = ruleDraftValue(consecutive.id, consecutive.kind, restored);
    expect(invalid.value).toBeNull();
    expect(invalid.errors.maximum).toBeDefined();
    expect(invalid.errors["mode.breakMinutes"]).toBeDefined();
  });

  it("resolves a changed mode as one whole field during rebase", () => {
    const base = {
      ...consecutive,
      mode: { kind: "assignments" as const, breakMinutes: 5 },
    };
    const local = ruleRebaseValue(base, {
      ...createRuleDraft(base),
      mode: { kind: "workedDays" as const },
    });
    if (local === null) throw new Error("Changed consecutive mode must rebase");
    const current = {
      ...base,
      mode: { kind: "assignments" as const, breakMinutes: 15 },
    };
    const merged = rebaseEntityDraft(base, local, current);
    expect(merged.conflicts).toContain("mode");
    const adopted = resolveEntityDraftField(merged, "mode", "current").value;
    const retained = resolveEntityDraftField(merged, "mode", "draft").value;
    if (
      adopted.kind !== "maximumConsecutive" ||
      retained.kind !== "maximumConsecutive" ||
      local.kind !== "maximumConsecutive"
    )
      throw new Error("Consecutive rebase changed rule kind");
    expect(adopted.mode).toEqual(current.mode);
    expect(retained.mode).toEqual(local.mode);
  });
});

describe("Maximum hours raw window and cap custody", () => {
  it("parses unsigned maximum minutes and positive rolling duration without rounding", () => {
    expect(parseMaximumHoursMaximumMinutesDraft("0004294967295")).toEqual({
      raw: "0004294967295",
      status: "valid",
      maximumMinutes: 4_294_967_295,
    });
    expect(parseMaximumHoursMaximumMinutesDraft("4294967296")).toMatchObject({
      status: "invalid",
      error: "range",
    });
    expect(parseMaximumHoursDurationDraft("0")).toMatchObject({
      status: "invalid",
      error: "range",
    });
    expect(parseMaximumHoursDurationDraft("1.5")).toMatchObject({
      status: "invalid",
      error: "syntax",
    });
    expect(parseMaximumHoursDurationDraft("20160")).toEqual({
      raw: "20160",
      status: "valid",
      durationMinutes: 20160,
    });
  });

  it("prepares both calendar and rolling windows while leaving membership semantics native", () => {
    expect(ruleDraftValue(hours.id, hours.kind, createRuleDraft(hours)).value).toEqual(hours);
    const rolling = {
      ...createRuleDraft(hours),
      bucketId: hours.bucketId,
      maximumMinutes: parseMaximumHoursMaximumMinutesDraft("0"),
      window: {
        kind: "rolling" as const,
        durationMinutes: parseMaximumHoursDurationDraft("20160"),
        membership: "startInstant" as const,
      },
    };
    expect(ruleDraftValue(hours.id, hours.kind, rolling).value).toEqual({
      ...hours,
      maximumMinutes: 0,
      window: { kind: "rolling", durationMinutes: 20160, membership: "startInstant" },
    });
  });

  it("keeps invalid cap and rolling duration raw through a concurrent rebase", () => {
    const raw = {
      ...createRuleDraft(hours),
      bucketId: "",
      maximumMinutes: parseMaximumHoursMaximumMinutesDraft("2e"),
      window: {
        kind: "rolling" as const,
        durationMinutes: parseMaximumHoursDurationDraft("0"),
        membership: "startInstant" as const,
      },
    };
    const local = ruleRebaseValue(hours, raw);
    expect(local).not.toBeNull();
    if (local === null) throw new Error("Invalid hours fields must not block rebasing");
    const current = { ...hours, active: false, maximumMinutes: 600 };
    const merged = rebaseEntityDraft(hours, local, current);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.active).toBe(false);
    expect(rebased.bucketId).toBe("");
    expect(rebased.maximumMinutes).toEqual(raw.maximumMinutes);
    expect(rebased.window).toEqual(raw.window);
    const invalid = ruleDraftValue(hours.id, hours.kind, rebased);
    expect(invalid.value).toBeNull();
    expect(invalid.errors.bucketId).toBeDefined();
    expect(invalid.errors.maximumMinutes).toBeDefined();
    expect(invalid.errors["window.durationMinutes"]).toBeDefined();
    expect(
      ruleDraftValue(hours.id, hours.kind, {
        ...rebased,
        bucketId: hours.bucketId,
        maximumMinutes: parseMaximumHoursMaximumMinutesDraft("0"),
        window: {
          kind: "rolling",
          durationMinutes: parseMaximumHoursDurationDraft("1"),
          membership: "startInstant",
        },
      }).value,
    ).toEqual({
      ...current,
      maximumMinutes: 0,
      window: { kind: "rolling", durationMinutes: 1, membership: "startInstant" },
    });
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

  it("keeps a local exclusion while rebasing an independent saved person change", () => {
    const raw = { ...createRuleDraft(fixed), assigned: false };
    const local = ruleRebaseValue(fixed, raw);
    if (local === null) throw new Error("Excluded pair must remain a complete rule");
    const current = { ...fixed, personId: "01900000-0000-7000-8000-000000000049" };
    const merged = rebaseEntityDraft(fixed, local, current);
    expect(merged.conflicts).toEqual([]);
    const restored = createRuleDraft(merged.value, { value: local, raw });
    const saved = ruleDraftValue(fixed.id, fixed.kind, restored).value;
    if (saved?.kind !== "fixedAssignment") throw new Error("Wrong saved rule");
    expect(saved.assigned).toBe(false);
    expect(saved.personId).toBe(current.personId);
    expect(saved.shiftId).toBe(fixed.shiftId);
  });

  it("adopts a native exclusion without replacing an independently edited target", () => {
    const raw = { ...createRuleDraft(fixed), shiftId: "01900000-0000-7000-8000-00000000004a" };
    const local = ruleRebaseValue(fixed, raw);
    if (local === null) throw new Error("Named target must rebase");
    const merged = rebaseEntityDraft(fixed, local, { ...fixed, assigned: false });
    expect(merged.conflicts).toEqual([]);
    const restored = createRuleDraft(merged.value, { value: local, raw });
    const saved = ruleDraftValue(fixed.id, fixed.kind, restored).value;
    if (saved?.kind !== "fixedAssignment") throw new Error("Wrong saved rule");
    expect(saved.assigned).toBe(false);
    expect(saved.shiftId).toBe(raw.shiftId);
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

const transitionTime: EditableRule & { readonly kind: "transitionTime" } = {
  id: "01900000-0000-7000-8000-00000000004c",
  kind: "transitionTime",
  active: true,
  strength: "required",
  scope: { people: { kind: "all" } },
  locationIds: ["01900000-0000-7000-8000-00000000004d"],
};

describe("Transition time draft custody", () => {
  it("adopts an independent native scope edit without dropping local locations", () => {
    const raw = {
      ...createRuleDraft(transitionTime),
      locationIds: [...transitionTime.locationIds, "01900000-0000-7000-8000-00000000004e"],
    };
    const local = ruleRebaseValue(transitionTime, raw);
    if (local === null) throw new Error("Edited location set must be rebasable");
    const current: EditableRule & { readonly kind: "transitionTime" } = {
      ...transitionTime,
      scope: { people: { kind: "all" }, categories: ["clinic"] },
    };
    const merged = rebaseEntityDraft(transitionTime, local, current);
    expect(merged.conflicts).toEqual([]);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(ruleDraftValue(transitionTime.id, transitionTime.kind, rebased).value).toEqual({
      ...current,
      locationIds: raw.locationIds,
    });
  });

  it("keeps conflicting location sets whole and preserves an active legacy singleton", () => {
    expect(
      ruleDraftValue(transitionTime.id, transitionTime.kind, createRuleDraft(transitionTime)).value,
    ).toEqual(transitionTime);
    const [firstLocation] = transitionTime.locationIds;
    if (firstLocation === undefined) throw new Error("Transition fixture needs one location");
    const authored = {
      ...transitionTime,
      locationIds: [firstLocation, "01900000-0000-7000-8000-00000000004e"],
    };
    const raw = {
      ...createRuleDraft(authored),
      locationIds: [firstLocation],
    };
    const local = ruleRebaseValue(authored, raw);
    if (local === null) throw new Error("Incomplete location set must be rebasable");
    const current = {
      ...authored,
      locationIds: ["01900000-0000-7000-8000-00000000004f", "01900000-0000-7000-8000-000000000050"],
    };
    const merged = rebaseEntityDraft(authored, local, current);
    expect(merged.conflicts).toContain("locationIds");
    if (merged.value.kind !== "transitionTime") throw new Error("Wrong rule kind");
    expect(merged.value.locationIds).toEqual(raw.locationIds);
    expect(
      ruleDraftValue(authored.id, authored.kind, {
        ...createRuleDraft(merged.value, { value: local, raw }),
        active: false,
        locationIds: [],
      }).value,
    ).toEqual({ ...merged.value, active: false, locationIds: [] });
  });
});

const timePreference: EditableRule & { readonly kind: "time" } = {
  id: "01900000-0000-7000-8000-000000000054",
  kind: "time",
  active: true,
  scope: { people: { kind: "all" } },
  priority: "high",
  weight: 12,
  direction: "avoid",
  timeWindow: {
    kind: "weekly",
    windows: [
      {
        weekdays: ["monday", "wednesday"],
        startTime: "09:00:00",
        endTime: "17:00:00",
        endDayOffset: 0,
      },
    ],
  },
};
const assignmentTargetPreference: EditableRule & { readonly kind: "assignmentTarget" } = {
  id: "01900000-0000-7000-8000-00000000005b",
  kind: "assignmentTarget",
  active: true,
  scope: { people: { kind: "all" } },
  priority: "high",
  weight: 17,
  calendarId: "01900000-0000-7000-8000-00000000005c",
  targetCount: 0,
};
const weekendTargetPreference: Extract<EditableRule, { readonly kind: "weekendTarget" }> = {
  id: "01900000-0000-7000-8000-00000000005d",
  kind: "weekendTarget",
  active: true,
  scope: { people: { kind: "all" } },
  priority: "high",
  weight: 17,
  targetCount: 0,
};
const workloadBalancePreference: EditableRule & { readonly kind: "workloadBalance" } = {
  id: "01900000-0000-7000-8000-00000000005e",
  kind: "workloadBalance",
  active: true,
  scope: { people: { kind: "all" } },
  priority: "veryHigh",
  weight: 23,
  workloadPolicyId: "01900000-0000-7000-8000-00000000005f",
};
const consecutiveWorkPreference: Extract<EditableRule, { readonly kind: "consecutiveWork" }> = {
  id: "01900000-0000-7000-8000-000000000060",
  kind: "consecutiveWork",
  active: true,
  scope: { people: { kind: "all" } },
  priority: "high",
  weight: 19,
  mode: { kind: "reportingDays" },
  assignmentTypeIds: ["01900000-0000-7000-8000-000000000061"],
  threshold: 1,
};
const consecutiveWorkWeekendsPreference: Extract<
  EditableRule,
  { readonly kind: "consecutiveWork" }
> = {
  id: "01900000-0000-7000-8000-000000000062",
  kind: "consecutiveWork",
  active: true,
  scope: { people: { kind: "all" } },
  priority: "normal",
  weight: 7,
  mode: {
    kind: "weekends",
    calendarId: "01900000-0000-7000-8000-000000000063",
    weekdays: ["saturday", "sunday"],
  },
  assignmentTypeIds: ["01900000-0000-7000-8000-000000000064"],
  threshold: 2,
};
const adjacencyPreference: Extract<EditableRule, { readonly kind: "adjacency" }> = {
  id: "01900000-0000-7000-8000-000000000065",
  kind: "adjacency",
  active: true,
  scope: { people: { kind: "all" } },
  priority: "normal",
  weight: 11,
  direction: "avoid",
  baseScheduleId: "01900000-0000-7000-8000-000000000066",
  maximumGapMinutes: 60,
  mode: "either",
};
const baseStabilityPreference: Extract<EditableRule, { readonly kind: "baseStability" }> = {
  id: "01900000-0000-7000-8000-000000000067",
  kind: "baseStability",
  active: true,
  scope: { people: { kind: "all" } },
  priority: "high",
  weight: 12,
  baseScheduleId: "01900000-0000-7000-8000-000000000066",
};
const togetherSeparatePreference: Extract<EditableRule, { readonly kind: "togetherSeparate" }> = {
  id: "01900000-0000-7000-8000-000000000068",
  kind: "togetherSeparate",
  active: true,
  scope: { people: { kind: "all" } },
  priority: "low",
  weight: 13,
  direction: "together",
  personIds: ["01900000-0000-7000-8000-000000000069", "01900000-0000-7000-8000-00000000006a"],
  mode: "overlappingShifts",
};
const preferredCoveragePreference: Extract<EditableRule, { readonly kind: "preferredCoverage" }> = {
  id: "01900000-0000-7000-8000-00000000006b",
  kind: "preferredCoverage",
  active: true,
  scope: { people: { kind: "all" } },
  priority: "veryHigh",
  weight: 14,
  coverageRequirementIds: ["01900000-0000-7000-8000-00000000006c"],
};

const preferenceFixtures: readonly EditableRule[] = [
  timePreference,
  {
    id: "01900000-0000-7000-8000-000000000055",
    kind: "assignmentType",
    active: true,
    scope: { people: { kind: "all" } },
    priority: "normal",
    weight: 4,
    direction: "prefer",
    assignmentTypeIds: ["01900000-0000-7000-8000-000000000056"],
  },
  {
    id: "01900000-0000-7000-8000-000000000057",
    kind: "location",
    active: true,
    scope: { people: { kind: "all" } },
    priority: "low",
    weight: 1,
    direction: "prefer",
    locationIds: ["01900000-0000-7000-8000-000000000058"],
  },
  {
    id: "01900000-0000-7000-8000-000000000059",
    kind: "requestedTimeOff",
    active: true,
    scope: { people: { kind: "all" } },
    priority: "veryHigh",
    weight: 99,
    availabilityIds: ["01900000-0000-7000-8000-00000000005a"],
  },
  assignmentTargetPreference,
  weekendTargetPreference,
  workloadBalancePreference,
  consecutiveWorkPreference,
  consecutiveWorkWeekendsPreference,
  adjacencyPreference,
  baseStabilityPreference,
  togetherSeparatePreference,
  preferredCoveragePreference,
];

describe("Preference draft custody", () => {
  it.each(preferenceFixtures)("round-trips the supported %s preference fields", (value) => {
    const draft = createRuleDraft(value);
    expect(ruleDraftValue(value.id, value.kind, draft).value).toEqual(value);
  });

  it("rejects zero preference weight on its own field", () => {
    const draft = createRuleDraft(timePreference);
    const result = ruleDraftValue(timePreference.id, timePreference.kind, {
      ...draft,
      weight: parseMaximumAssignmentCountDraft("0"),
    });
    expect(result.value).toBeNull();
    expect(result.errors).toEqual({
      weight: messages.ruleSetup.preferenceWeightInvalid,
    });
  });

  it("retains a saved typed preference while time input is incomplete", () => {
    const draft = createRuleDraft(timePreference);
    const raw: typeof draft = {
      ...draft,
      weight: parseMaximumAssignmentCountDraft(""),
      timeWindow: { kind: "instant", startsAt: "", endsAt: "" },
    };
    const local = ruleRebaseValue(timePreference, raw);
    expect(local).toEqual(timePreference);
    if (local === null) throw new Error("Incomplete time input must not block rebasing");
    const rebased = createRuleDraft(timePreference, { value: local, raw });
    expect(rebased.weight).toEqual(raw.weight);
    expect(rebased.timeWindow).toEqual(raw.timeWindow);
  });

  it("reports local preference weight and weekly-window errors", () => {
    const draft = createRuleDraft(timePreference);
    const result = ruleDraftValue(timePreference.id, timePreference.kind, {
      ...draft,
      weight: parseMaximumAssignmentCountDraft("0"),
      timeWindow: { kind: "weekly", windows: [] },
    });
    expect(result.value).toBeNull();
    expect(result.errors).toEqual({
      "timeWindow.windows": messages.ruleSetup.timeWindowRequired,
      weight: messages.ruleSetup.preferenceWeightInvalid,
    });
  });

  it("reports an independent weekly-window conflict and preserves invalid weight raw input", () => {
    const draft = createRuleDraft(timePreference);
    if (draft.timeWindow.kind !== "weekly" || timePreference.timeWindow.kind !== "weekly")
      throw new Error("Expected weekly time preference fixtures");
    const savedWindow = draft.timeWindow.windows[0];
    const currentWindow = timePreference.timeWindow.windows[0];
    if (savedWindow === undefined || currentWindow === undefined)
      throw new Error("Expected a weekly time preference window");
    const raw: typeof draft = {
      ...draft,
      weight: parseMaximumAssignmentCountDraft("0"),
      timeWindow: {
        ...draft.timeWindow,
        windows: [{ ...savedWindow, startTime: "08:00:00" }],
      },
    };
    const local = ruleRebaseValue(timePreference, raw);
    if (local === null) throw new Error("Invalid preference weight must not block rebasing");
    const current: typeof timePreference = {
      ...timePreference,
      timeWindow: {
        ...timePreference.timeWindow,
        windows: [{ ...currentWindow, startTime: "10:00:00" }],
      },
    };
    const merged = rebaseEntityDraft(timePreference, local, current);
    expect(merged.conflicts).toContain("timeWindow");
    if (local.kind !== "time" || merged.value.kind !== "time")
      throw new Error("Time preference rebase changed the record kind");
    expect(merged.value.timeWindow).toEqual(local.timeWindow);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.weight).toEqual(raw.weight);
    expect(rebased.timeWindow).toEqual(raw.timeWindow);
    expect(ruleDraftValue(timePreference.id, timePreference.kind, rebased).value).toBeNull();
    expect(ruleDraftValue(timePreference.id, timePreference.kind, rebased).errors).toEqual({
      weight: messages.ruleSetup.preferenceWeightInvalid,
    });
  });
  it("validates adjacency references, direction, mode, and bounded gap input", () => {
    const draft = createRuleDraft(adjacencyPreference);
    const result = ruleDraftValue(adjacencyPreference.id, adjacencyPreference.kind, {
      ...draft,
      baseScheduleId: "",
      direction: "later" as typeof draft.direction,
      mode: "invalid" as typeof draft.mode,
      maximumGapMinutes: parseMaximumAssignmentCountDraft("4294967296"),
    });
    expect(result.value).toBeNull();
    expect(result.errors).toEqual({
      baseScheduleId: messages.ruleSetup.baseScheduleRequired,
      direction: messages.ruleSetup.preferenceDirectionInvalid,
      mode: messages.ruleSetup.adjacencyModeInvalid,
      maximumGapMinutes: messages.ruleSetup.adjacencyGapInvalid,
    });
  });

  it("keeps incomplete adjacency input local while rebasing typed values", () => {
    const draft = createRuleDraft(adjacencyPreference);
    const raw = {
      ...draft,
      baseScheduleId: "",
      maximumGapMinutes: parseMaximumAssignmentCountDraft(""),
    };
    expect(ruleRebaseValue(adjacencyPreference, raw)).toEqual(adjacencyPreference);
    const local = ruleRebaseValue(adjacencyPreference, raw);
    if (local === null) throw new Error("Incomplete adjacency input must not block rebasing");
    const rebased = createRuleDraft(adjacencyPreference, { value: local, raw });
    expect(rebased.baseScheduleId).toBe("");
    expect(rebased.maximumGapMinutes).toEqual(raw.maximumGapMinutes);
    expect(
      ruleDraftValue(adjacencyPreference.id, adjacencyPreference.kind, rebased).value,
    ).toBeNull();
  });

  it("adopts a new native base-schedule field when a different preference kind replaces a stale draft", () => {
    const previous = preferenceFixtures.find((rule) => rule.kind === "assignmentType");
    if (previous?.kind !== "assignmentType") throw new Error("Missing assignment type fixture");
    const raw = { ...createRuleDraft(previous), baseScheduleId: "" };
    const updated = { ...baseStabilityPreference, id: previous.id };
    const rebased = createRuleDraft(updated, { value: previous, raw });
    expect(rebased.baseScheduleId).toBe(updated.baseScheduleId);
    expect(ruleDraftValue(updated.id, updated.kind, rebased).errors).toEqual({});
  });

  it("rejects duplicate together/separate people and invalid group controls", () => {
    const draft = createRuleDraft(togetherSeparatePreference);
    const firstPerson = draft.personIds[0];
    if (firstPerson === undefined) throw new Error("Expected a selected person");
    const result = ruleDraftValue(togetherSeparatePreference.id, togetherSeparatePreference.kind, {
      ...draft,
      direction: "near" as typeof draft.direction,
      mode: "invalid" as typeof draft.mode,
      personIds: [firstPerson, firstPerson],
    });
    expect(result.value).toBeNull();
    expect(result.errors).toEqual({
      direction: messages.ruleSetup.groupDirectionInvalid,
      mode: messages.ruleSetup.togetherSeparateModeInvalid,
      personIds: messages.ruleSetup.togetherSeparateDuplicatePeople,
    });
  });

  it("requires a base schedule for base-stability preferences", () => {
    const draft = createRuleDraft(baseStabilityPreference);
    const result = ruleDraftValue(baseStabilityPreference.id, baseStabilityPreference.kind, {
      ...draft,
      baseScheduleId: "",
    });
    expect(result.value).toBeNull();
    expect(result.errors).toEqual({
      baseScheduleId: messages.ruleSetup.baseScheduleRequired,
    });
  });
});
describe("Consecutive work preference draft custody", () => {
  it("round-trips reporting-day and weekend modes with explicit assignment types", () => {
    for (const value of [consecutiveWorkPreference, consecutiveWorkWeekendsPreference]) {
      const draft = createRuleDraft(value);
      expect(ruleDraftValue(value.id, value.kind, draft).value).toEqual(value);
    }
  });

  it("reports threshold and active required-field errors without changing raw intent", () => {
    const draft = createRuleDraft(consecutiveWorkPreference);
    const invalid = {
      ...draft,
      threshold: parseMaximumAssignmentCountDraft(""),
      assignmentTypeIds: [],
    };
    const result = ruleDraftValue(
      consecutiveWorkPreference.id,
      consecutiveWorkPreference.kind,
      invalid,
    );
    expect(result.value).toBeNull();
    expect(result.errors).toEqual({
      threshold: messages.ruleSetup.consecutiveWorkThresholdInvalid,
      assignmentTypeIds: messages.ruleSetup.consecutiveWorkAssignmentTypesRequired,
    });
    expect(invalid.threshold).toEqual(parseMaximumAssignmentCountDraft(""));
    expect(invalid.assignmentTypeIds).toEqual([]);
  });

  it("rebases a changed weekend mode as one whole nested field", () => {
    const draft = createRuleDraft(consecutiveWorkWeekendsPreference);
    if (typeof draft.mode !== "object" || draft.mode.kind !== "weekends")
      throw new Error("Expected weekend mode draft");
    const savedMode = consecutiveWorkWeekendsPreference.mode;
    if (savedMode.kind !== "weekends") throw new Error("Expected saved weekend mode");
    const raw = {
      ...draft,
      mode: {
        ...draft.mode,
        calendarId: "01900000-0000-7000-8000-000000000065",
        weekdays: ["saturday"] as const,
      },
    };
    const local = ruleRebaseValue(consecutiveWorkWeekendsPreference, raw);
    if (local === null || local.kind !== "consecutiveWork" || local.mode.kind !== "weekends")
      throw new Error("Valid weekend mode must remain rebaseable");
    expect(local.mode).toEqual(raw.mode);
    const current = {
      ...consecutiveWorkWeekendsPreference,
      mode: {
        kind: "weekends" as const,
        calendarId: savedMode.calendarId,
        weekdays: ["sunday"] as const,
      },
    };
    const merged = rebaseEntityDraft(consecutiveWorkWeekendsPreference, local, current);
    expect(merged.conflicts).toContain("mode");
    expect(merged.value.mode).toEqual(local.mode);
  });

  it("preserves inactive invalid threshold, assignment types and weekend raw fields", () => {
    const inactive = { ...consecutiveWorkWeekendsPreference, active: false };
    const draft = createRuleDraft(inactive);
    if (typeof draft.mode !== "object" || draft.mode.kind !== "weekends")
      throw new Error("Expected weekend mode draft");
    const raw = {
      ...draft,
      threshold: parseMaximumAssignmentCountDraft(""),
      assignmentTypeIds: [],
      mode: { ...draft.mode, calendarId: "", weekdays: [] },
    };
    const local = ruleRebaseValue(inactive, raw);
    if (local === null) throw new Error("Inactive invalid input must remain rebaseable");
    const rebased = createRuleDraft(local, { value: local, raw });
    expect(rebased.active).toBe(false);
    expect(rebased.threshold).toEqual(raw.threshold);
    expect(rebased.assignmentTypeIds).toEqual([]);
    expect(rebased.mode).toEqual(raw.mode);
    expect(ruleDraftValue(inactive.id, inactive.kind, rebased)).toEqual({
      value: null,
      errors: { threshold: messages.ruleSetup.consecutiveWorkThresholdInvalid },
    });
  });

  it("allows inactive empty required collections while retaining the inactive meaning", () => {
    const inactive = { ...consecutiveWorkPreference, active: false };
    const draft = createRuleDraft(inactive);
    const value = ruleDraftValue(inactive.id, inactive.kind, {
      ...draft,
      assignmentTypeIds: [],
      threshold: parseMaximumAssignmentCountDraft("0"),
    });
    expect(value.errors).toEqual({});
    expect(value.value).toMatchObject({
      active: false,
      assignmentTypeIds: [],
      threshold: 0,
    });
  });
});
describe("Workload balance preference draft custody", () => {
  it("rejects a policy reference outside the persisted singleton without mutating raw input", () => {
    const draft = createRuleDraft(workloadBalancePreference);
    const invalid = {
      ...draft,
      workloadPolicyId: "01900000-0000-7000-8000-000000000060",
    };
    const result = ruleDraftValue(
      workloadBalancePreference.id,
      workloadBalancePreference.kind,
      invalid,
      [workloadBalancePreference.workloadPolicyId],
    );
    expect(result.value).toBeNull();
    expect(result.errors).toEqual({
      workloadPolicyId: messages.ruleSetup.fields.workloadPolicyInvalid,
    });
    expect(draft.workloadPolicyId).toBe(workloadBalancePreference.workloadPolicyId);
    expect(invalid.workloadPolicyId).toBe("01900000-0000-7000-8000-000000000060");
  });

  it("retains inactive raw policy intent while rebasing incomplete input", () => {
    const inactive = { ...workloadBalancePreference, active: false };
    const raw = { ...createRuleDraft(inactive), workloadPolicyId: "" };
    const local = ruleRebaseValue(inactive, raw);
    expect(local).toEqual(inactive);
    if (local === null) throw new Error("Inactive incomplete policy input must remain rebaseable");
    const rebased = createRuleDraft(inactive, { value: local, raw });
    expect(rebased.active).toBe(false);
    expect(rebased.workloadPolicyId).toBe("");
    expect(
      ruleDraftValue(inactive.id, inactive.kind, rebased, [
        workloadBalancePreference.workloadPolicyId,
      ]),
    ).toEqual({
      value: null,
      errors: { workloadPolicyId: messages.ruleSetup.fields.workloadPolicyRequired },
    });
  });
});
describe("Assignment target preference draft custody", () => {
  it("accepts a zero target count and round-trips its calendar, priority and weight", () => {
    const draft = createRuleDraft(assignmentTargetPreference);
    expect(draft.targetCount).toEqual(parseMaximumAssignmentCountDraft("0"));
    expect(
      ruleDraftValue(assignmentTargetPreference.id, assignmentTargetPreference.kind, draft).value,
    ).toEqual(assignmentTargetPreference);
  });
  it("rebases a changed typed target count as an independent whole field", () => {
    const raw = {
      ...createRuleDraft(assignmentTargetPreference),
      targetCount: parseMaximumAssignmentCountDraft("3"),
    };
    const local = ruleRebaseValue(assignmentTargetPreference, raw);
    if (local === null) throw new Error("Valid target input must remain rebaseable");
    const current = { ...assignmentTargetPreference, targetCount: 4 };
    const merged = rebaseEntityDraft(assignmentTargetPreference, local, current);
    expect(merged.conflicts).toContain("targetCount");
    if (merged.value.kind !== "assignmentTarget")
      throw new Error("Assignment target rebase changed the record kind");
    expect(merged.value.targetCount).toBe(3);
  });

  it("keeps invalid target input while rebasing independent saved fields", () => {
    const draft = createRuleDraft(assignmentTargetPreference);
    const raw = {
      ...draft,
      targetCount: parseMaximumAssignmentCountDraft("2e"),
      calendarId: "",
      priority: "low" as const,
    };
    const local = ruleRebaseValue(assignmentTargetPreference, raw);
    if (local === null) throw new Error("Invalid target input must not block rebasing");
    const current = {
      ...assignmentTargetPreference,
      active: false,
      priority: "veryHigh" as const,
      weight: 22,
      calendarId: "01900000-0000-7000-8000-00000000005d",
      targetCount: 4,
    };
    const merged = rebaseEntityDraft(assignmentTargetPreference, local, current);
    if (merged.value.kind !== "assignmentTarget")
      throw new Error("Assignment target rebase changed the record kind");
    expect(merged.conflicts).toEqual(expect.arrayContaining(["calendarId", "priority"]));
    expect(merged.value.active).toBe(false);
    expect(merged.value.weight).toBe(22);
    expect(merged.value.priority).toBe("low");
    expect(merged.value.calendarId).toBe("");
    expect(merged.value.targetCount).toBe(4);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.targetCount).toEqual(raw.targetCount);
    expect(rebased.calendarId).toBe("");
    expect(
      ruleDraftValue(assignmentTargetPreference.id, assignmentTargetPreference.kind, rebased),
    ).toEqual({
      value: null,
      errors: {
        calendarId: messages.ruleSetup.calendarRequired,
        targetCount: messages.ruleSetup.targetCountInvalid,
      },
    });
  });

  it("retains inactive target fields as raw intent until they are repaired", () => {
    const inactive = { ...assignmentTargetPreference, active: false };
    const draft = createRuleDraft(inactive);
    const raw = { ...draft, targetCount: parseMaximumAssignmentCountDraft("") };
    const local = ruleRebaseValue(inactive, raw);
    expect(local).toEqual(inactive);
    if (local === null) throw new Error("Inactive incomplete target input must remain rebaseable");
    const rebased = createRuleDraft(inactive, { value: local, raw });
    expect(rebased.active).toBe(false);
    expect(rebased.targetCount).toEqual(raw.targetCount);
    expect(ruleDraftValue(inactive.id, inactive.kind, rebased).value).toBeNull();
  });
});
describe("Weekend target preference draft custody", () => {
  it("round-trips a zero total target", () => {
    const draft = createRuleDraft(weekendTargetPreference);
    expect(draft.targetCount).toEqual(parseMaximumAssignmentCountDraft("0"));
    expect(
      ruleDraftValue(weekendTargetPreference.id, weekendTargetPreference.kind, draft).value,
    ).toEqual(weekendTargetPreference);
  });

  it("keeps invalid target input while rebasing unrelated saved fields", () => {
    const draft = createRuleDraft(weekendTargetPreference);
    const raw = {
      ...draft,
      targetCount: parseMaximumAssignmentCountDraft("2e"),
      priority: "low" as const,
    };
    const local = ruleRebaseValue(weekendTargetPreference, raw);
    if (local === null) throw new Error("Invalid weekend target must not block rebasing");
    const current = {
      ...weekendTargetPreference,
      active: false,
      priority: "veryHigh" as const,
      weight: 22,
      targetCount: 4,
    };
    const merged = rebaseEntityDraft(weekendTargetPreference, local, current);
    if (merged.value.kind !== "weekendTarget")
      throw new Error("Weekend target rebase changed the record kind");
    expect(merged.conflicts).toContain("priority");
    expect(merged.value.active).toBe(false);
    expect(merged.value.weight).toBe(22);
    expect(merged.value.priority).toBe("low");
    expect(merged.value.targetCount).toBe(4);
    const rebased = createRuleDraft(merged.value, { value: local, raw });
    expect(rebased.targetCount).toEqual(raw.targetCount);
    expect(
      ruleDraftValue(weekendTargetPreference.id, weekendTargetPreference.kind, rebased),
    ).toEqual({
      value: null,
      errors: { targetCount: messages.ruleSetup.targetCountInvalid },
    });
  });

  it("recovers an inactive invalid target without losing inactive meaning", () => {
    const inactive = { ...weekendTargetPreference, active: false };
    const raw = {
      ...createRuleDraft(inactive),
      targetCount: parseMaximumAssignmentCountDraft(""),
    };
    const local = ruleRebaseValue(inactive, raw);
    expect(local).toEqual(inactive);
    if (local === null) throw new Error("Inactive invalid target must remain rebaseable");
    const rebased = createRuleDraft(inactive, { value: local, raw });
    expect(rebased.active).toBe(false);
    expect(rebased.targetCount).toEqual(raw.targetCount);
    expect(ruleDraftValue(inactive.id, inactive.kind, rebased).value).toBeNull();
    const repaired = {
      ...rebased,
      targetCount: parseMaximumAssignmentCountDraft("0"),
    };
    expect(ruleDraftValue(inactive.id, inactive.kind, repaired).value).toEqual({
      ...inactive,
      targetCount: 0,
    });
  });
});
