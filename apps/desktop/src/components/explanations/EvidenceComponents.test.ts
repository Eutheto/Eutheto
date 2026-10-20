import { createSSRApp, h, type Component } from "vue";
import { renderToString } from "@vue/server-renderer";
import { describe, expect, it } from "vitest";

import type { ApiErrorDto, FastFindingsV1, ValidationIssue } from "../../api/generated";
import ErrorRecoveryPanel from "./ErrorRecoveryPanel.vue";
import ValidationSummary from "./ValidationSummary.vue";

async function render(component: Component, props: Record<string, unknown>): Promise<string> {
  const context: { teleports?: Record<string, string> } = {};
  const html = await renderToString(
    createSSRApp({
      render: () => h(component, props),
    }),
    context,
  );
  return html + Object.values(context.teleports ?? {}).join("");
}

const requiredRuleId = "01900000-0000-7000-8000-000000000010";

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

  it("quarantines validation findings on internal failure", async () => {
    const validationHtml = await render(ValidationSummary, {
      findings: validationFindings,
      state: "internalFailure",
      interaction: "selectable",
    });

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
