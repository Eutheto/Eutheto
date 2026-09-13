import type { PeopleCsvDecision, PeopleCsvSampleV1 } from "../../api/generated";
import { plannerMessage } from "./messages";

export interface CsvDecisionDraft {
  readonly record: number;
  readonly decision:
    | { readonly kind: "add"; readonly personId: string }
    | { readonly kind: "update"; readonly personId: string | null }
    | { readonly kind: "skip" };
}

/** The parent correlates source, dialect, scope and request before publishing a sample. */
export type CsvRecordSampleState =
  | { readonly status: "idle" }
  | { readonly status: "loading"; readonly record: number }
  | { readonly status: "error"; readonly record: number; readonly message: string }
  | { readonly status: "ready"; readonly sample: PeopleCsvSampleV1 };

function representableRecord(record: number): boolean {
  return Number.isSafeInteger(record) && record >= 1 && record <= 10_001;
}

/** Admit only an explicit whole logical-record number; never normalize unfinished input. */
export function csvDecisionRecord(raw: string): number | null {
  if (!/^[0-9]+$/.test(raw)) return null;
  const record = Number(raw);
  return representableRecord(record) ? record : null;
}

/** Structural conversion only. Native preview remains the identity and policy authority. */
export function csvDecisionValues(drafts: readonly CsvDecisionDraft[]): {
  decisions: readonly PeopleCsvDecision[] | null;
  errors: readonly { record: number; message: string }[];
} {
  const errors: { record: number; message: string }[] = [];
  const decisions: PeopleCsvDecision[] = [];
  const records = new Set<number>();
  for (const [index, draft] of drafts.entries()) {
    const { record, decision } = draft;
    if (!representableRecord(record))
      errors.push({ record, message: plannerMessage("csvDecision.recordError") });
    if (records.has(record))
      errors.push({ record, message: plannerMessage("csvDecision.duplicateRecord") });
    records.add(record);
    if (index === 10_000)
      errors.push({ record, message: plannerMessage("csvDecision.limit") });
    if (decision.kind === "skip") {
      decisions.push({ record, decision: { kind: "skip" } });
    } else if (decision.personId === null || decision.personId.trim() === "") {
      errors.push({
        record,
        message: plannerMessage(
          decision.kind === "update" ? "csvDecision.updateRequired" : "csvDecision.addRequired",
        ),
      });
    } else {
      decisions.push({ record, decision: { kind: decision.kind, personId: decision.personId } });
    }
  }
  return { decisions: errors.length === 0 ? decisions : null, errors };
}
