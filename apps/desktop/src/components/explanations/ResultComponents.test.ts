import { createSSRApp, h, type Component } from "vue";
import { renderToString } from "@vue/server-renderer";
import { describe, expect, it } from "vitest";

import type {
  AssignmentEvidenceV1,
  CounterfactualResult,
  ExplanationCertainty,
  ExplanationResultV2,
  ScoreContributionV1,
  ScoreVector,
  SolveStatus,
  VerificationWarning,
} from "../../api/generated";
import AssignmentInspector from "./AssignmentInspector.vue";
import ExplanationPanel from "./ExplanationPanel.vue";
import ScoreBreakdown from "./ScoreBreakdown.vue";
import SolutionStatus from "./SolutionStatus.vue";
import { renderEvidenceTemplate } from "./messages";

async function render(component: Component, props: Record<string, unknown>): Promise<string> {
  const context: { teleports?: Record<string, string> } = {};
  const app = createSSRApp({ render: () => h(component, props) });
  const html = await renderToString(app, context);
  return html + Object.values(context.teleports ?? {}).join("");
}

const emptyScore: ScoreVector = { feasibility: "0", levels: [] };
const noWarnings: readonly VerificationWarning[] = [];
const solutionWarning: VerificationWarning = {
  id: "warning.1",
  messageKey: "warning.rounding",
  affectedEntities: [{ kind: "person", id: "alice" }],
  facts: { count: { type: "integer", value: "9007199254740993" } },
};

function solutionProps(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    accepted: true,
    status: "optimal" satisfies SolveStatus,
    allRequiredRulesPassed: true,
    score: emptyScore,
    warnings: noWarnings,
    state: "ready",
    ...overrides,
  };
}

const contribution: ScoreContributionV1 = {
  evidenceId: "contribution.preference",
  levelId: "level.preferences",
  categoryId: "category.request",
  value: "-9007199254740993",
};

const assignmentEvidence: AssignmentEvidenceV1 = {
  assignment: {
    id: "assignment.alice-night",
    entity: { kind: "person", id: "alice" },
    value: { type: "integer", value: "9007199254740993" },
    evidence: ["assignment.fact"],
  },
  relatedRules: [
    {
      ruleId: "rule.rest",
      satisfied: true,
      affectedEntities: [{ kind: "person", id: "alice" }],
      messageKey: "rule.rest.passed",
      expected: {},
      observed: {},
      evidence: ["rule.fact"],
    },
  ],
  scoreContributions: [contribution],
  metrics: {
    "metric.fairness": {
      type: "ratio",
      value: { numerator: "1", denominator: "3" },
    },
  },
  lockState: { state: "locked", value: { type: "boolean", value: true } },
};

function assignmentProps(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    evidence: assignmentEvidence,
    entityLabel: "Alice Example",
    workLabel: "Night shift",
    eligibility: "Eligible for this shift",
    availability: "Available for the full interval",
    preferencesHelped: ["Requested nights"],
    preferencesHurt: ["Preferred a shorter shift"],
    fairnessContribution: "+2 toward balanced hours",
    stabilityContribution: "No change from the base plan",
    state: "ready",
    diagnosticState: "idle",
    ...overrides,
  };
}

function diagnosticResult(conclusion: CounterfactualResult["conclusion"]): CounterfactualResult {
  return { conclusion } as CounterfactualResult;
}

function explanationResult(certainty: ExplanationCertainty = "deterministic"): ExplanationResultV2 {
  return {
    schemaVersion: 2,
    evidence: {
      schemaVersion: 2,
      certainty,
      evidence: {
        kind: "assignment",
        assignment: assignmentEvidence,
      },
      checksum: "evidence-checksum",
    },
    rendered: {
      schemaVersion: 1,
      kind: "assignment",
      evidenceChecksum: "evidence-checksum",
      messages: [
        {
          messageKey: "assignment.eligible",
          parameters: {},
          entities: [{ kind: "person", id: "alice" }],
          rules: ["rule.rest"],
          assignments: ["assignment.alice-night"],
          evidence: ["fact.eligibility"],
        },
      ],
    },
    checksum: "result-checksum",
  };
}

describe("SolutionStatus", () => {
  it("does not mislabel rejected candidates as internal verification failures", async () => {
    const unaccepted = await render(
      SolutionStatus,
      solutionProps({ accepted: false, warnings: [solutionWarning], baseChangeCount: 2 }),
    );
    const failedRules = await render(
      SolutionStatus,
      solutionProps({
        accepted: true,
        allRequiredRulesPassed: false,
        warnings: [solutionWarning],
        baseChangeCount: 2,
      }),
    );

    for (const html of [unaccepted, failedRules]) {
      expect(html).toContain("No independently accepted result is available for selection.");
      expect(html.toLowerCase()).not.toContain("ready");
      expect(html).not.toContain("internal verification failure");
      expect(html).not.toContain("Required rules");
      expect(html).not.toContain("Verified score");
      expect(html).not.toContain("verification warning");
      expect(html).not.toContain("verified changes");
      expect(html).not.toContain("View proof");
    }
  });

  it.each([
    "empty",
    "loading",
    "stale",
    "cancelled",
    "inconclusive",
    "unavailable",
    "internalFailure",
  ])("prioritizes the %s UI state and hides candidate authority", async (state) => {
    const html = await render(
      SolutionStatus,
      solutionProps({
        state,
        status: "infeasible",
        accepted: false,
        warnings: [solutionWarning],
        backend: "solver-backend",
        elapsed: 1250,
        baseChangeCount: 2,
        warningTexts: { "warning.rounding": "Rounded {count} assignments." },
      }),
    );

    expect(html).not.toContain("All required rules passed");
    expect(html).not.toContain("Required rules did not pass");
    expect(html).not.toContain("Verified score");
    expect(html).not.toContain("verified changes");
    expect(html).not.toContain("verification warning");
    expect(html).not.toContain("Rounded");
    expect(html).not.toContain('href="#entity-person-alice"');
    expect(html).not.toContain("View proof");
    expect(html).toContain("Backend solver-backend");
    expect(html).toContain("Finished in 1,250 ms");
    expect(html).toContain("View run details");
    expect(html).not.toMatch(/(?:action|score|status|warning)\.[A-Za-z]/);
  });

  it("reserves internal verification failure copy for the quarantined UI state", async () => {
    const html = await render(
      SolutionStatus,
      solutionProps({
        accepted: false,
        state: "internalFailure",
        status: "backendFailed",
        warnings: [solutionWarning],
        backend: "solver-backend",
        elapsed: 1250,
        baseChangeCount: 2,
        warningTexts: { "warning.rounding": "Rounded {count} assignments." },
      }),
    );

    expect(html).toContain('role="alert"');
    expect(html).toContain("An internal verification failure quarantined this candidate.");
    expect(html).not.toContain("Backend failed");
    expect(html).not.toContain("All required rules passed");
    expect(html).not.toContain("Verified score");
    expect(html).not.toContain("verification warning");
    expect(html).not.toContain("verified changes");
    expect(html).not.toContain("View proof");
    expect(html).toContain("Backend solver-backend");
    expect(html).toContain("Finished in 1,250 ms");
    expect(html).toContain("View run details");
    expect(html.match(/type="button"/g)).toHaveLength(1);
  });
});

describe("ScoreBreakdown", () => {
  it("renders semantic objective order, direction, exact signed values, and selectable contributions", async () => {
    const score: ScoreVector = {
      feasibility: "0",
      levels: [
        {
          levelId: "level.preferences",
          value: "9007199254740993",
          direction: "maximize",
          categoryBreakdown: {
            "category.request": "-9007199254740993",
          },
        },
        {
          levelId: "level.fairness",
          value: "0",
          direction: "minimize",
          categoryBreakdown: {},
        },
      ],
    };
    const html = await render(ScoreBreakdown, {
      score,
      contributions: [contribution],
      state: "ready",
      levelLabels: {
        "level.preferences": "Preference satisfaction",
        "level.fairness": "Fairness spread",
      },
      categoryLabels: { "category.request": "Shift requests" },
    });

    expect(html).toContain("Required-rule feasibility");
    expect(html).toContain("Passed");
    expect(html).toContain("Objective level 1");
    expect(html).toContain("Objective level 2");
    expect(html.indexOf("Preference satisfaction")).toBeLessThan(html.indexOf("Fairness spread"));
    expect(html).toContain("Maximize — higher is better");
    expect(html).toContain("Minimize — lower is better");
    expect(html).toContain("+9,007,199,254,740,993");
    expect(html).toContain("−9,007,199,254,740,993");
    expect(html).toContain("Shift requests");
    expect(html).toContain('type="button"');
  });
  it("treats only zero feasibility violations as passing", async () => {
    const html = await render(ScoreBreakdown, {
      score: { feasibility: "3", levels: [] },
      contributions: [],
      state: "ready",
    });

    expect(html).toContain("Required rules did not pass");
    expect(html).toContain("Failed (+3)");
    expect(html).not.toContain("Required rules passed");
  });

  it.each([
    ["empty", "No verified preference or fairness scores are available."],
    ["loading", "Verified scores are being prepared."],
    ["stale", "These scores belong to an earlier scenario revision."],
    ["cancelled", "Score verification was cancelled."],
    ["inconclusive", "does not establish a verified score breakdown"],
    ["unavailable", "A verified score breakdown is unavailable."],
    ["internalFailure", "internal verification failure quarantined this candidate"],
  ])("renders the %s recovery state", async (state, expected) => {
    const html = await render(ScoreBreakdown, { score: null, contributions: [], state });
    expect(html).toContain(expected);
  });
});

describe("AssignmentInspector", () => {
  it("renders every assignment evidence field, lock state, and keyboard-reachable actions", async () => {
    const html = await render(AssignmentInspector, assignmentProps());
    for (const text of [
      "Alice Example",
      "Night shift",
      "Eligible for this shift",
      "Available for the full interval",
      "9,007,199,254,740,993",
      "rule.rest",
      "rule.rest.passed",
      "Requested nights",
      "Preferred a shorter shift",
      "+2 toward balanced hours",
      "No change from the base plan",
      "−9,007,199,254,740,993",
      "metric.fairness",
      "1 / 3",
      "Locked",
      "Why this?",
      "Why not…?",
      "Try a change",
      "A short diagnostic optimization may run.",
    ]) {
      expect(html).toContain(text);
    }
    expect(html.match(/type="button"/g)).toHaveLength(3);
  });

  it.each([
    ["idle", "A short diagnostic optimization may run."],
    ["ready", "No verified diagnostic result is available."],
    ["empty", "No Why not…? diagnostic has been started."],
    ["loading", "A short diagnostic optimization is running."],
    ["stale", "Start this diagnostic again from the current revision."],
    ["cancelled", "The diagnostic optimization was cancelled."],
    ["limit", "reached its limit without proof"],
    ["inconclusive", "No verified distinction was found within the diagnostic limit."],
    ["unavailable", "A diagnostic backend is unavailable."],
    ["internalFailure", "internal verification failed"],
  ])("renders the %s diagnostic state", async (diagnosticState, expected) => {
    const html = await render(AssignmentInspector, assignmentProps({ diagnosticState }));
    expect(html).toContain(expected);
    if (diagnosticState === "ready") {
      expect(html).not.toContain("independently verified evidence");
    }
    if (diagnosticState === "loading") {
      expect(html).toContain("Cancel diagnostic");
      expect(html).toContain('aria-live="polite"');
    }
  });

  it.each([
    [
      diagnosticResult({ type: "provenImpossible" }),
      "The alternative was proven impossible under the temporary condition.",
    ],
    [
      diagnosticResult({ type: "notDistinguishedWithinBudget" }),
      "The diagnostic budget did not distinguish the alternatives.",
    ],
    [
      diagnosticResult({
        type: "verifiedAlternative",
        ordering: "worse",
      } as CounterfactualResult["conclusion"]),
      "An independently verified alternative was found and is worse than the base result.",
    ],
  ])("renders the typed ready diagnostic conclusion", async (result, expected) => {
    const html = await render(
      AssignmentInspector,
      assignmentProps({ diagnosticState: "ready", diagnosticResult: result }),
    );
    expect(html).toContain(expected);
  });

  it("suppresses assignment actions for quarantined evidence", async () => {
    const html = await render(AssignmentInspector, assignmentProps({ state: "internalFailure" }));
    expect(html).toContain('role="alert"');
    expect(html).toContain("internal verification failure quarantined this candidate");
    expect(html).not.toContain("Why this?");
    expect(html).not.toContain("Why not…?");
    expect(html).not.toContain("Try a change");
  });
});

describe("renderEvidenceTemplate", () => {
  it("interpolates typed evidence parameters without losing Int64 precision", () => {
    const text = renderEvidenceTemplate(
      "{count} for {entity}: {ok}; {note}",
      {
        count: { type: "integer", value: "9007199254740993" },
        entity: { type: "entity", value: { kind: "person", id: "alice" } },
        ok: { type: "boolean", value: true },
        note: { type: "text", value: "verified" },
      },
      "en-US",
    );
    expect(text).toBe("9,007,199,254,740,993 for person alice: Yes; verified");
  });
});

describe("ExplanationPanel", () => {
  it("does not render dialog content when closed", async () => {
    const html = await render(ExplanationPanel, {
      open: false,
      result: explanationResult("deterministic"),
      state: "ready",
      messageTexts: { "assignment.eligible": "Hidden evidence" },
      scenarioId: "01900000-0000-7000-8000-000000000001",
      currentRevision: 3,
    });
    expect(html).not.toContain('role="dialog"');
    expect(html).not.toContain("Hidden evidence");
  });
});
