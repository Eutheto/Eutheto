import { describe, expect, it } from "vitest";
import { csvReferenceValues, type CsvReferenceDraft } from "./people-csv-references";

const first = "01900000-0000-7000-8000-000000000001";
const second = "01900000-0000-7000-8000-000000000002";
function row(key: string, token: string, entityId = first): CsvReferenceDraft {
  return { key, token, kind: "team", entityId };
}

describe("explicit CSV reference mapping", () => {
  it("rejects duplicate exact tokens instead of silently choosing the last identity", () => {
    const duplicate = csvReferenceValues([row("a", "ward"), row("b", "ward", second)]);
    expect(duplicate.value).toBeNull();
    expect(Object.keys(duplicate.errors)).toEqual(["a", "b"]);
    expect(csvReferenceValues([row("a", "ward"), row("b", "Ward", second)]).value).toEqual({
      ward: first,
      Ward: second,
    });
  });

  it("preserves special keys and whitespace as inert JSON tokens, not prototype assignments", () => {
    const value = csvReferenceValues([row("a", "__proto__"), row("b", " ward ", second)]).value;
    expect(value).not.toBeNull();
    expect(JSON.parse(JSON.stringify(value))).toEqual(
      JSON.parse(`{"__proto__":"${first}"," ward ":"${second}"}`),
    );
    expect(csvReferenceValues([row("a", "ward", "")]).value).toBeNull();
  });

  it("enforces the native UTF-8 token boundary without truncating or normalizing input", () => {
    const exact = `${"漢".repeat(21)}a`;
    expect(csvReferenceValues([row("a", exact)]).value).toEqual({ [exact]: first });
    expect(csvReferenceValues([row("a", `${exact}a`)]).value).toBeNull();
    expect(csvReferenceValues([row("a", "ward\n")]).value).toBeNull();
  });
});
