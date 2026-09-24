import type { Revision, ScenarioSummaryV2 } from "./api/generated";

/**
 * Readiness is presentation-only: native validation and the saved project revision remain
 * authoritative. A completed full run is useful here only when both validation layers report no
 * errors for the same current revision.
 */
export function isOptimizeValidationReady(
  summary: Pick<ScenarioSummaryV2, "revision" | "fast" | "full">,
  currentRevision: Revision,
): boolean {
  const full = summary.full;
  return (
    summary.revision === currentRevision &&
    full.state === "completed" &&
    full.inputRevision === currentRevision &&
    !full.stale &&
    full.counts.errors === 0 &&
    summary.fast.counts.errors === 0
  );
}
