import type {
  WorkforceSetupCoverageSummary,
  WorkforceSetupDisplayDuration,
  WorkforceSetupShiftOrigin,
} from "./api/generated-domain-pack-contracts";
import { formatNumber, messages } from "./messages";

const copy = messages.work.table;

export function formatWorkOffset(offsetSeconds: number): string {
  return `${offsetSeconds >= 0 ? "+" : ""}${String(offsetSeconds)}`;
}

export function formatWorkDuration(duration: WorkforceSetupDisplayDuration): string {
  // Keep native wide seconds as text; the separate remainder is exact too.
  return copy.durationValue(duration.seconds, String(duration.nanoseconds));
}

export function formatWorkOrigin(origin: WorkforceSetupShiftOrigin): string {
  if (origin.kind === "manual") return copy.manual;
  return copy.originDetails(copy[origin.kind], origin.templateId, origin.occurrenceDate);
}

export function formatWorkCoverage(
  coverage: WorkforceSetupCoverageSummary,
  locale?: string,
): string {
  const preferred =
    coverage.preferred === null ? copy.notSpecified : formatNumber(coverage.preferred, locale);
  const maximum =
    coverage.maximum === null ? copy.notSpecified : formatNumber(coverage.maximum, locale);
  return copy.coverageSummary(
    formatNumber(coverage.minimum, locale),
    preferred,
    maximum,
    formatNumber(coverage.qualificationMinimumCount, locale),
  );
}
