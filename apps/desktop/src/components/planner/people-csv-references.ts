import { plannerMessage } from "./messages";

export const csvReferenceKinds = [
  "qualification",
  "assignmentType",
  "team",
  "location",
  "workloadBucket",
  "calendar",
] as const;
export type CsvReferenceKind = (typeof csvReferenceKinds)[number];
export interface CsvReferenceDraft {
  readonly key: string;
  readonly token: string;
  readonly kind: CsvReferenceKind | "";
  readonly entityId: string;
}

/** Exact token mapping only. Rust validates UUIDs, referenced kinds and the final candidate. */
export function csvReferenceValues(rows: readonly CsvReferenceDraft[]): {
  readonly value: Readonly<Record<string, string>> | null;
  readonly errors: Readonly<Record<string, string>>;
} {
  const counts = new Map<string, number>();
  for (const row of rows) counts.set(row.token, (counts.get(row.token) ?? 0) + 1);
  const errors: [string, string][] = [];
  const encoder = new TextEncoder();
  for (const [index, row] of rows.entries()) {
    // MAX_TOKEN_BYTES in workforce validation/score.rs; do not trim or case-fold identity.
    if (
      row.token === "" ||
      row.token.length > 64 ||
      encoder.encode(row.token).length > 64 ||
      /\p{Cc}/u.test(row.token)
    )
      errors.push([row.key, plannerMessage("csvReference.tokenError")]);
    else if ((counts.get(row.token) ?? 0) > 1)
      errors.push([row.key, plannerMessage("csvReference.duplicate")]);
    else if (row.kind === "" || row.entityId === "")
      errors.push([row.key, plannerMessage("csvReference.targetRequired")]);
    else if (index >= 10_000) errors.push([row.key, plannerMessage("csvReference.limit")]);
  }
  return {
    // fromEntries preserves even special property names as inert own keys, without prototype setters.
    value:
      errors.length === 0 ? Object.fromEntries(rows.map((row) => [row.token, row.entityId])) : null,
    errors: Object.fromEntries(errors),
  };
}
