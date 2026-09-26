import { createSSRApp, h, type Component } from "vue";
import { renderToString } from "@vue/server-renderer";
import { describe, expect, it } from "vitest";

import type {
  ApiErrorDto,
  DomainAssignment,
  FastFindingsV1,
  InfeasibilityEvidenceV1,
  SolutionComparisonV1,
  ValidationIssue,
} from "../../api/generated";
import ChangeSetPreview from "./ChangeSetPreview.vue";
import ConflictCard from "./ConflictCard.vue";
import ErrorRecoveryPanel from "./ErrorRecoveryPanel.vue";
import ValidationSummary from "./ValidationSummary.vue";

async function render(component: Component, props: Record<string, unknown>): Promise<string> {
  return renderToString(
    createSSRApp({
      render: () => h(component, props),
    }),
  );
}

const requiredRuleId = "01900000-0000-7000-8000-000000000010";
const personId = "01900000-0000-7000-8000-000000000020";

const conflict: InfeasibilityEvidenceV1 = {
  type: "conflict",
  groups: [{ groupId: "coverage.minimum", requiredRules: [requiredRuleId] }],
  minimality: "sufficient",
  shrink: {
    initialGroupCount: 3,
    remainingGroupCount: 1,
    attemptedTrials: 2,
    maxTrials: 2,
    stopReason: "trialLimit",
  },
};

const unavailableConflict: InfeasibilityEvidenceV1 = {
  type: "unavailable",
  reason: "invalidAssumptionCore",
};

const validationIssues: readonly ValidationIssue[] = [
  {
    code: "coverage.required",
    severity: "error",
    message: "At least one qualified person is required.",
    fieldPath: "rules.coverage",
    resource: { type: "rule", id: requiredRuleId },
  },
  {
    code: "availability.review",
    severity: "warning",
    message: "Review the availability window.",
    fieldPath: null,
    resource: null,
  },
];

const validationFindings: FastFindingsV1 = {
  counts: { errors: 1, warnings: 1, information: 0 },
  issues: validationIssues,
  omitted: 0,
};

const beforeAssignment: DomainAssignment = {
  id: "assignment.coverage",
  entity: { kind: "person", id: personId },
  value: { type: "integer", value: "1" },
  evidence: [],
};
const afterAssignment: DomainAssignment = {
  ...beforeAssignment,
  value: { type: "integer", value: "2" },
};
const addedAssignment: DomainAssignment = {
  id: "assignment.added",
  entity: { kind: "person", id: "01900000-0000-7000-8000-000000000021" },
  value: { type: "boolean", value: true },
  evidence: [],
};
const removedAssignment: DomainAssignment = {
  id: "assignment.removed",
  entity: { kind: "person", id: "01900000-0000-7000-8000-000000000022" },
  value: { type: "absent" },
  evidence: [],
};

const comparison: SolutionComparisonV1 = {
  schemaVersion: 1,
  base: {
    packId: "official.test",
    scenarioId: "01900000-0000-7000-8000-000000000001",
    scenarioRevision: 4,
    documentHash: "base-document",
    projectionVersion: 1,
    verificationScopeChecksum: "base-scope",
    acceptedResult: {
      solutionId: "01900000-0000-7000-8000-000000000002",
      resultChecksum: "base-result",
    },
  },
  candidate: {
    packId: "official.test",
    scenarioId: "01900000-0000-7000-8000-000000000001",
    scenarioRevision: 5,
    documentHash: "candidate-document",
    projectionVersion: 1,
    verificationScopeChecksum: "candidate-scope",
    acceptedResult: {
      solutionId: "01900000-0000-7000-8000-000000000003",
      resultChecksum: "candidate-result",
    },
  },
  baseScore: { feasibility: "0", levels: [] },
  candidateScore: { feasibility: "0", levels: [] },
  assignments: [
    { type: "added", after: addedAssignment },
    { type: "removed", before: removedAssignment },
    { type: "changed", before: beforeAssignment, after: afterAssignment },
  ],
  rules: [],
  scoreLevels: [],
  metrics: [],
  locks: [],
  runs: null,
  affectedEntities: [beforeAssignment.entity],
  ordering: "better",
  checksum: "comparison",
};

const retryableError: ApiErrorDto = {
  code: "explanation.backend_unavailable",
  message: "The explanation backend did not respond.",
  category: "solver",
  retryable: true,
  fieldErrors: [],
  details: null,
  diagnosticId: "01900000-0000-7000-8000-000000000099",
};

const terminalError: ApiErrorDto = {
  code: "validation.rule_invalid",
  message: "A required rule is invalid.",
  category: "validation",
  retryable: false,
  fieldErrors: [
    {
      field: "rules.coverage",
      code: "coverage.invalid",
      message: "Coverage must be greater than zero.",
    },
  ],
  details: null,
  diagnosticId: null,
};

describe("evidence explanation components", () => {
  it("renders the selected finding's authoritative details", async () => {
    const html = await render(ValidationSummary, {
      findings: validationFindings,
      state: "ready",
      interaction: "selectable",
      selectedIssue: validationIssues[0],
    });

    expect(html).toContain(validationIssues[0]?.message);
    expect(html).toContain(requiredRuleId);
    expect(html.match(/aria-current="true"/g)).toHaveLength(1);
  });

  it("keeps omitted fast findings explicit without exposing static findings as actions", async () => {
    const html = await render(ValidationSummary, {
      findings: {
        ...validationFindings,
        counts: { errors: 3, warnings: 1, information: 0 },
        omitted: 2,
      },
      state: "ready",
      interaction: "static",
      presentation: "embedded",
      headingLevel: 4,
    });
    const announcement = html.match(/<p[^>]*role="status"[^>]*>(.*?)<\/p>/s)?.[1];
    expect(announcement?.match(/\d+/g)).toEqual(["4", "2", "2"]);
    expect(html).toContain("<h4");
    expect(html).not.toContain("<h2");
    expect(html).not.toContain("<button");
    expect(html).toContain(requiredRuleId);
  });

  it("preserves the conflict heading and never upgrades sufficient evidence to minimal", async () => {
    const html = await render(ConflictCard, {
      evidence: conflict,
      state: "ready",
      ruleLabels: { [requiredRuleId]: "Minimum qualified coverage" },
    });

    expect(html).toContain("These required rules cannot all be satisfied together");
    expect(html).toContain("Sufficient conflict");
    expect(html).not.toContain("Proven minimal conflict");
    expect(html).toContain("other conflicts may also exist");
    expect(html).toContain("1 of 3 groups remain");
    expect(html).toContain("stopped at the trial limit after 2 trials");
    expect(html).toContain("Minimum qualified coverage");
    expect(html).toContain("Relax in a copy");
    expect(html).toContain("Paraphrase");
    expect(html).toContain("Export deterministic diagnostic");
  });

  it("labels minimality only when the evidence proves it", async () => {
    const html = await render(ConflictCard, {
      evidence: {
        ...conflict,
        minimality: "provenMinimal",
        shrink: {
          initialGroupCount: 1,
          remainingGroupCount: 1,
          attemptedTrials: 1,
          maxTrials: 1,
          stopReason: "completed",
        },
      } satisfies InfeasibilityEvidenceV1,
      state: "ready",
      ruleLabels: {},
    });

    expect(html).toContain("Proven minimal conflict");
    expect(html).toContain("Every remaining group was proven necessary under this grouping.");
    expect(html).not.toContain("Sufficient conflict");
  });

  it("explains unavailable conflict evidence without guessing rule provenance", async () => {
    const html = await render(ConflictCard, {
      evidence: unavailableConflict,
      state: "ready",
      ruleLabels: {},
    });

    expect(html).toContain("Conflict evidence unavailable");
    expect(html).not.toContain("These required rules cannot all be satisfied together");
    expect(html).toContain("The returned conflict evidence was invalid and was not shown.");
    expect(html).toContain("No required rules were guessed or inferred.");
    expect(html).not.toContain("Relax in a copy");
  });

  it("renders accessible Added, Removed, and Changed categories with an exact count", async () => {
    const html = await render(ChangeSetPreview, {
      comparison,
      state: "ready",
      entityLabels: { [personId]: "Ada Rivera" },
    });

    expect(html).toContain('aria-live="polite"');
    expect(html).toContain(
      "3 verified changes: 1 added, 1 removed, 1 changed assignments; 0 rule, 0 score, 0 metric, 0 lock, and 0 run or proof changes.",
    );
    expect(html).toContain("Added (1)");
    expect(html).toContain("Removed (1)");
    expect(html).toContain("Changed (1)");
    expect(html).toContain("Before: 1");
    expect(html).toContain("After: 2");
    expect(html).toContain("Affected references");
    expect(html).toContain("Ada Rivera");
  });

  it("renders non-assignment deltas instead of treating the comparison as empty", async () => {
    const nonAssignmentComparison: SolutionComparisonV1 = {
      ...comparison,
      assignments: [],
      rules: [
        {
          ruleId: requiredRuleId,
          before: {
            ruleId: requiredRuleId,
            satisfied: true,
            affectedEntities: [beforeAssignment.entity],
            messageKey: "coverage.before",
            expected: {},
            observed: {},
            evidence: [],
          },
          after: {
            ruleId: requiredRuleId,
            satisfied: false,
            affectedEntities: [beforeAssignment.entity],
            messageKey: "coverage.after",
            expected: {},
            observed: {},
            evidence: [],
          },
        },
      ],
      scoreLevels: [
        {
          levelId: "fairness",
          direction: "minimize",
          before: "9223372036854775807",
          after: "9223372036854775806",
          delta: "-1",
          categories: [
            {
              categoryId: "weekends",
              before: "1000",
              after: "999",
              delta: "-1",
            },
          ],
        },
      ],
      metrics: [
        {
          metricId: "coverage-ratio",
          before: { type: "ratio", value: { numerator: "3", denominator: "4" } },
          after: { type: "integer", value: "1" },
        },
      ],
      locks: [
        {
          assignmentId: beforeAssignment.id,
          before: { state: "unlocked" },
          after: { state: "locked", value: { type: "integer", value: "2" } },
          preserved: false,
        },
      ],
      runs: {
        base: {
          runId: "01900000-0000-7000-8000-000000000030",
          runManifestChecksum: "base-run",
          outcome: {
            type: "accepted",
            status: "feasible",
            solutionId: "01900000-0000-7000-8000-000000000002",
            acceptedResultChecksum: "base-result",
            verificationChecksum: "base-verification",
          },
          certainty: "independentlyVerified",
        },
        candidate: {
          runId: "01900000-0000-7000-8000-000000000031",
          runManifestChecksum: "candidate-run",
          outcome: {
            type: "accepted",
            status: "optimal",
            solutionId: "01900000-0000-7000-8000-000000000003",
            acceptedResultChecksum: "candidate-result",
            verificationChecksum: "candidate-verification",
          },
          certainty: "backendProof",
        },
      },
    };

    const html = await render(ChangeSetPreview, {
      comparison: nonAssignmentComparison,
      state: "ready",
      locale: "en-US",
      entityLabels: { [personId]: "Ada Rivera" },
      ruleLabels: { [requiredRuleId]: "Minimum qualified coverage" },
    });

    expect(html).not.toContain("There are no verified changes to show.");
    expect(html).toContain(
      "5 verified changes: 0 added, 0 removed, 0 changed assignments; 1 rule, 1 score, 1 metric, 1 lock, and 1 run or proof changes.",
    );
    expect(html).toContain("Required rule changes");
    expect(html).toContain("Before: Satisfied. After: Not satisfied.");
    expect(html).toContain("Score changes");
    expect(html).toContain("9,223,372,036,854,775,807");
    expect(html).toContain("Metric changes");
    expect(html).toContain("3 / 4");
    expect(html).toContain("Lock changes");
    expect(html).toContain("Run and proof changes");
    expect(html).toContain("Before: Unlocked. After: Locked to 2.");
    expect(html).toContain("The lock was not preserved.");
    expect(html).toContain("Accepted feasible result");
    expect(html).toContain("Independently verified");
    expect(html).toContain("Accepted optimal result");
    expect(html).toContain("Backend proof");
    expect(html).toContain('href="#entity-person-');
    expect(html).toContain(`href=\"#rule-${requiredRuleId}\"`);
  });

  it("formats integer and interval assignment values from Int64 strings without number coercion", async () => {
    const huge = "9223372036854775807";
    const assignmentComparison: SolutionComparisonV1 = {
      ...comparison,
      assignments: [
        {
          type: "changed",
          before: { ...beforeAssignment, value: { type: "integer", value: huge } },
          after: {
            ...afterAssignment,
            value: {
              type: "interval",
              value: { start: huge, end: "-9223372036854775808", duration: "3600" },
            },
          },
        },
      ],
    };

    const html = await render(ChangeSetPreview, {
      comparison: assignmentComparison,
      state: "ready",
      locale: "en-US",
    });

    expect(html).toContain("Before: 9,223,372,036,854,775,807");
    expect(html).toContain(
      "After: Interval 9,223,372,036,854,775,807–-9,223,372,036,854,775,808 (duration 3,600)",
    );
  });

  it("quarantines internal-failure data and suppresses ready selection actions", async () => {
    const changeHtml = await render(ChangeSetPreview, {
      comparison,
      state: "internalFailure",
    });
    const validationHtml = await render(ValidationSummary, {
      findings: validationFindings,
      state: "internalFailure",
      interaction: "selectable",
    });

    expect(changeHtml).toContain("An internal verification failure quarantined this candidate.");
    expect(changeHtml).not.toContain("3 verified changes");
    expect(changeHtml).not.toContain("Select added change");
    expect(validationHtml).not.toContain(requiredRuleId);
    expect(validationHtml).not.toContain("<button");
  });

  it("gates retry and diagnostic export actions from structured error authority", async () => {
    const recoverableHtml = await render(ErrorRecoveryPanel, {
      error: retryableError,
      state: "unavailable",
      actions: ["retry", "exportDiagnostic", "dismiss"],
    });
    expect(recoverableHtml).toContain('role="alert"');
    expect(recoverableHtml).toContain("This explanation is unavailable.");
    expect(recoverableHtml).toContain("The explanation backend did not respond.");
    expect(recoverableHtml).toContain("Try again");
    expect(recoverableHtml).toContain("Export diagnostic");
    expect(recoverableHtml).toContain("Dismiss");
    expect(recoverableHtml).toContain(retryableError.diagnosticId);

    expect(recoverableHtml.match(/role="alert"/g)).toHaveLength(1);
    const alertStart = recoverableHtml.indexOf('role="alert"');
    const alertEnd = recoverableHtml.indexOf("</div>", alertStart);
    const detailStart = recoverableHtml.indexOf("The explanation backend did not respond.");
    expect(alertStart).toBeGreaterThan(-1);
    expect(alertEnd).toBeGreaterThan(alertStart);
    expect(detailStart).toBeGreaterThan(alertEnd);
    const terminalHtml = await render(ErrorRecoveryPanel, {
      error: terminalError,
      state: "ready",
      actions: ["retry", "exportDiagnostic", "dismiss"],
    });
    expect(terminalHtml).toContain("A required rule is invalid.");
    expect(terminalHtml).toContain("Fields needing attention");
    expect(terminalHtml).toContain("Coverage must be greater than zero.");
    expect(terminalHtml).not.toContain("Try again");
    expect(terminalHtml).not.toContain("Export diagnostic");
    expect(terminalHtml).toContain("Dismiss");
  });

  it("keeps diagnostic identity visible without offering unavailable recovery actions", async () => {
    const html = await render(ErrorRecoveryPanel, {
      error: retryableError,
      state: "ready",
      actions: [],
    });
    expect(html).toContain(retryableError.message);
    expect(html).toContain(retryableError.diagnosticId);
    expect(html).not.toContain("<button");
  });
});
