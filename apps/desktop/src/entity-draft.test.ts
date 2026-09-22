import { describe, expect, it } from "vitest";
import { rebaseEntityDraft, resolveEntityDraftField } from "./entity-draft";

interface RecordDraft {
  readonly id: string;
  readonly kind: string;
  readonly name: string;
  readonly tags: readonly string[];
  readonly display?: { readonly color?: string; readonly avatarInitials?: string };
}
const base: RecordDraft = { id: "person-a", kind: "person", name: "Original", tags: ["ward"] };

describe("explicit entity draft rebase", () => {
  it("preserves an independent stored change instead of submitting the old complete record", () => {
    const result = rebaseEntityDraft(
      base,
      { ...base, name: "My name" },
      { ...base, tags: ["night"] },
    );
    expect(result.value).toEqual({ ...base, name: "My name", tags: ["night"] });
    expect(result.conflicts).toEqual([]);
    expect(base.tags).toEqual(["ward"]);
  });

  it("requires a whole-field choice for concurrent collection changes", () => {
    const local = { ...base, tags: ["ward", "draft"] };
    const current = { ...base, tags: ["ward", "stored"] };
    const result = rebaseEntityDraft(base, local, current);
    expect(result.conflicts).toEqual(["tags"]);
    expect(resolveEntityDraftField(result, "tags", "draft")).toMatchObject({
      value: local,
      conflicts: [],
    });
    expect(resolveEntityDraftField(result, "tags", "current")).toMatchObject({
      value: current,
      conflicts: [],
    });
    expect(result.conflicts).toEqual(["tags"]);
  });

  it("handles optional removal and ignores object property insertion order", () => {
    const initial: RecordDraft = { ...base, display: { color: "#112233", avatarInitials: "AB" } };
    const reordered = { ...base, display: { avatarInitials: "AB", color: "#112233" } };
    const changed = { ...base, display: { color: "#445566" } };
    expect(rebaseEntityDraft(initial, reordered, changed).value).toEqual(changed);
    const removed = rebaseEntityDraft(initial, base, changed);
    expect(removed.conflicts).toEqual(["display"]);
    expect(resolveEntityDraftField(removed, "display", "draft").value).not.toHaveProperty(
      "display",
    );
    const storedRemoval = rebaseEntityDraft(initial, changed, base);
    expect(resolveEntityDraftField(storedRemoval, "display", "current").value).not.toHaveProperty(
      "display",
    );
  });

  it("retains partial choices and unresolved fields across another revision", () => {
    const local = { ...base, name: "Local", tags: ["local-tag"] };
    const current = { ...base, name: "Current", tags: ["current-tag"] };
    const initial = rebaseEntityDraft(base, local, current);
    const partial = resolveEntityDraftField(initial, "tags", "current");
    const next = { ...current, display: { color: "#445566" } };
    const continued = rebaseEntityDraft(base, local, next, partial);
    expect(continued.conflicts).toEqual(["name"]);
    expect(resolveEntityDraftField(continued, "name", "draft").value).toEqual({
      ...next,
      name: "Local",
    });
    const keptDraft = resolveEntityDraftField(initial, "tags", "draft");
    const continuedDraft = rebaseEntityDraft(base, local, next, keptDraft);
    expect(continuedDraft.conflicts).toEqual(["name"]);
    expect(resolveEntityDraftField(continuedDraft, "name", "current").value).toEqual({
      ...next,
      tags: ["local-tag"],
    });
    expect(
      rebaseEntityDraft(base, local, { ...next, tags: ["new-current"] }, keptDraft).conflicts,
    ).toEqual(["name", "tags"]);
  });

  it("does not report an agreed change as a conflict and refuses identity substitution", () => {
    const same = { ...base, name: "Agreed" };
    expect(rebaseEntityDraft(base, same, same).conflicts).toEqual([]);
    expect(() => rebaseEntityDraft(base, same, { ...base, id: "another" })).toThrow();
    expect(() => rebaseEntityDraft(base, { ...same, kind: "team" }, base)).toThrow();
  });
});
