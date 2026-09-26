import { describe, expect, it } from "vitest";
import { parseDurationDraft } from "./components/planner/duration-field";
import {
  createRuleDraft,
  parseMaximumAssignmentCountDraft,
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
