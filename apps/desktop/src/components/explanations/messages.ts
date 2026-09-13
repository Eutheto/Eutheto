import type { VerificationValue } from "../../api/generated";

export const explanationMessages = {
  "action.cancel": "Cancel",
  "action.close": "Close",
  "action.dismiss": "Dismiss",
  "action.edit": "Edit related item",
  "action.exportDiagnostic": "Export diagnostic",
  "action.inspect": "Inspect",
  "action.paraphrase": "Paraphrase",
  "action.relaxCopy": "Relax in a copy",
  "action.retry": "Try again",
  "action.tryChange": "Try a change",
  "action.viewProof": "View proof",
  "action.viewRun": "View run details",
  "action.whyNot": "Why not…?",
  "action.whyThis": "Why this?",
  "assignment.availability": "Availability",
  "assignment.eligibility": "Eligibility",
  "assignment.empty": "No assignment is selected.",
  "assignment.fairness": "Fairness and stability",
  "assignment.heading": "Assignment details",
  "assignment.locked": "Locked",
  "assignment.preferences": "Preferences helped and hurt",
  "assignment.rules": "Required rule checks",
  "assignment.unlocked": "Not locked",
  "change.added": "Added",
  "change.changed": "Changed",
  "change.empty": "There are no verified changes to show.",
  "change.heading": "Verified changes",
  "change.removed": "Removed",
  "conflict.diagnosticExport": "Export deterministic diagnostic",
  "conflict.empty": "No mapped conflict evidence is available.",
  "conflict.heading": "These required rules cannot all be satisfied together",
  "conflict.minimal": "Proven minimal conflict",
  "conflict.paraphrase": "Paraphrase does not change the deterministic evidence.",
  "conflict.sufficient": "Sufficient conflict",
  "counterfactual.cancelled": "The diagnostic optimization was cancelled.",
  "counterfactual.inconclusive": "No verified distinction was found within the diagnostic limit.",
  "counterfactual.limit": "The diagnostic optimization reached its limit without proof.",
  "counterfactual.progress": "A short diagnostic optimization is running.",
  "counterfactual.stale":
    "The scenario changed. Start this diagnostic again from the current revision.",
  "counterfactual.unavailable": "A diagnostic backend is unavailable.",
  "error.cancelled": "The operation was cancelled.",
  "error.heading": "This explanation could not be completed",
  "error.inconclusive": "The available evidence is inconclusive.",
  "error.internal": "An internal verification failure quarantined this candidate.",
  "error.loading": "Loading explanation evidence…",
  "error.stale": "The scenario changed. Refresh before trying again.",
  "error.unavailable": "This explanation is unavailable.",
  "explanation.empty": "No explanation evidence is available for this subject.",
  "explanation.heading": "Explanation",
  "explanation.messages": "Evidence",
  "explanation.proof": "Proof and certainty",
  "score.empty": "No verified preference or fairness scores are available.",
  "score.feasibility": "Required-rule feasibility",
  "score.heading": "Verified score breakdown",
  "score.level": "Objective level {level}",
  "score.requiredPassed": "All required rules passed",
  "score.requiredFailed": "Required rules did not pass",
  "status.backend": "Backend {backend}",
  "status.backendFailed": "Backend failed",
  "status.cancelled": "Solve cancelled",
  "status.feasible": "Verified feasible result ready",
  "status.infeasible": "Infeasibility proven",
  "status.invalidModel": "Invalid model",
  "status.limit": "No verified result within the limit",
  "status.optimal": "Verified optimal result ready",
  "status.ready": "Verified result ready",
  "status.time": "Finished in {milliseconds} ms",
  "status.unavailable": "Backend unavailable",
  "status.unbounded": "Unbounded model proven",
  "validation.empty": "No validation issues need attention.",
  "validation.heading": "Validation summary",
  "validation.findingsCount": "{total} validation issues: {displayed} shown, {omitted} omitted.",
  "validation.errors": "Errors",
  "validation.warnings": "Warnings",
  "validation.information": "Information",
  "validation.loading": "Checking the scenario…",
} as const;

export type ExplanationMessageKey = keyof typeof explanationMessages;
export type ExplanationMessageParameters = Readonly<Record<string, string | number>>;

export function explanationMessage(
  key: ExplanationMessageKey,
  parameters: ExplanationMessageParameters = {},
  locale?: string,
): string {
  return explanationMessages[key].replace(/\{([A-Za-z][A-Za-z0-9]*)\}/g, (_, name: string) => {
    const value = parameters[name];
    if (value === undefined) return `{${name}}`;
    return typeof value === "number" ? new Intl.NumberFormat(locale).format(value) : value;
  });
}

function verificationValueText(value: VerificationValue, locale?: string): string {
  switch (value.type) {
    case "boolean":
      return value.value ? "Yes" : "No";
    case "integer":
      try {
        return new Intl.NumberFormat(locale).format(BigInt(value.value));
      } catch {
        return value.value;
      }
    case "text":
      return value.value;
    case "entity":
      return `${value.value.kind} ${value.value.id}`;
  }
}

export function renderEvidenceTemplate(
  template: string,
  parameters: Readonly<Record<string, VerificationValue>>,
  locale?: string,
): string {
  return template.replace(/\{([A-Za-z][A-Za-z0-9]*)\}/g, (_, name: string) => {
    const value = parameters[name];
    return value === undefined ? `{${name}}` : verificationValueText(value, locale);
  });
}
