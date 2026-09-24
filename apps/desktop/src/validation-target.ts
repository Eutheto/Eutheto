import type { Revision } from "./api/generated";

/** Native diagnostic context; editors resolve only their own closed field-to-control mapping. */
export interface ValidationEditorTarget {
  readonly scenarioId: string;
  readonly revision: Revision;
  readonly libraryEpoch: number;
  readonly collection: "entities" | "rules" | "preferences";
  readonly id: string;
  /** Native field segments relative to the identified record, never a selector. */
  readonly fieldPath: readonly string[];
}
