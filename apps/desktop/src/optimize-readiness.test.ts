import { describe, expect, it } from "vitest";
import type { ScenarioSummaryV2 } from "./api/generated";
import { isOptimizeValidationReady } from "./optimize-readiness";

const operationId = "01900000-0000-7000-8000-000000000099";

type ReadinessSummary = Pick<ScenarioSummaryV2, "revision" | "fast" | "full">;

function summary(
  overrides: {
    readonly revision?: number;
    readonly inputRevision?: number;
    readonly stale?: boolean;
    readonly fullErrors?: number;
    readonly fastErrors?: number;
  } = {},
): ReadinessSummary {
  const revision = overrides.revision ?? 7;
  return {
    revision,
    fast: {
      counts: { errors: overrides.fastErrors ?? 0, warnings: 0, information: 0 },
      issues: [],
      omitted: 0,
    },
    full: {
      state: "completed",
      operationId,
      inputRevision: overrides.inputRevision ?? revision,
      stale: overrides.stale ?? false,
      counts: { errors: overrides.fullErrors ?? 0, warnings: 0, information: 0 },
    },
  };
}

describe("Optimize validation readiness", () => {
  it("requires a completed, current, nonstale full run with no fast or full errors", () => {
    expect(isOptimizeValidationReady(summary(), 7)).toBe(true);
    for (const invalid of [
      summary({ stale: true }),
      summary({ inputRevision: 6 }),
      summary({ revision: 6 }),
      summary({ fullErrors: 1 }),
      summary({ fastErrors: 1 }),
    ]) {
      expect(isOptimizeValidationReady(invalid, 7)).toBe(false);
    }
  });

  it("does not offer a handoff for non-completed full validation states", () => {
    const states: ScenarioSummaryV2["full"][] = [
      { state: "notRun" },
      { state: "running", operationId, inputRevision: 7, stale: false },
      { state: "failed", operationId, inputRevision: 7, stale: false, code: "invalid" },
      { state: "cancelled", operationId, inputRevision: 7, stale: false },
    ];
    for (const full of states) {
      expect(isOptimizeValidationReady({ ...summary(), full }, 7)).toBe(false);
    }
  });
});
