import { describe, expect, it } from "vitest";
import { parseDurationDraft } from "./components/planner/duration-field";
import { createRuleDraft, ruleDraftValue, ruleRebaseValue, type EditableRule } from "./rule-draft";
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
