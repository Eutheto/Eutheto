import { cleanup, render, screen, within } from "@testing-library/vue";
import { afterEach, describe, expect, it } from "vitest";
import { userEvent } from "vitest/browser";
import type {
  AcceptedResultRefV1,
  AssignmentProtectionComparisonV1,
  AssignmentValue,
  ComparisonBindingV1,
  DomainAssignment,
  MetricComparisonV1,
  RuleComparisonV1,
  RunComparisonSideV2,
  RunComparisonV2,
  ScoreLevelComparisonV1,
  SolutionComparisonV2,
} from "../api/generated";
import AcceptedComparison from "./AcceptedComparison.vue";
import "../styles.css";

const scenarioId = "01900000-0000-7000-8000-000000000001";
const baseSolutionId = "01900000-0000-7000-8000-000000000002";
const candidateSolutionId = "01900000-0000-7000-8000-000000000003";
const baseRunId = "01900000-0000-7000-8000-000000000004";
const candidateRunId = "01900000-0000-7000-8000-000000000005";
const revision = 7;

function acceptedResult(solutionId: string, suffix: string): AcceptedResultRefV1 {
  return { solutionId, resultChecksum: `${suffix}-result-checksum` };
}

function binding(side: "base" | "candidate"): ComparisonBindingV1 {
  return {
    packId: "official.workforce",
    scenarioId,
    scenarioRevision: side === "base" ? 5 : 6,
    documentHash: `${side}-document-hash`,
    projectionVersion: 3,
    verificationScopeChecksum: `${side}-scope-checksum`,
    acceptedResult:
      side === "base"
        ? acceptedResult(baseSolutionId, "base")
        : acceptedResult(candidateSolutionId, "candidate"),
  };
}

function assignment(
  id: string,
  value: AssignmentValue = { type: "boolean", value: true },
): DomainAssignment {
  return {
    id,
    entity: { kind: "person", id: `person-${id}` },
    value,
    evidence: [`evidence-${id}`],
  };
}

function runSide(side: "base" | "candidate"): RunComparisonSideV2 {
  return {
    runId: side === "base" ? baseRunId : candidateRunId,
    runManifestChecksum: `${side}-manifest-checksum`,
    outcome: {
      type: "accepted",
      status: side === "base" ? "feasible" : "optimal",
      solutionId: side === "base" ? baseSolutionId : candidateSolutionId,
      acceptedResultChecksum: `${side}-result-checksum`,
      verificationChecksum: `${side}-verification-checksum`,
    },
  };
}

const runs: RunComparisonV2 = { base: runSide("base"), candidate: runSide("candidate") };
const changedRule: RuleComparisonV1 = {
  ruleId: "01900000-0000-7000-8000-000000000006",
  before: {
    ruleId: "01900000-0000-7000-8000-000000000006",
    satisfied: true,
    affectedEntities: [{ kind: "person", id: "person-rule" }],
    messageKey: "required.rule",
    expected: { threshold: { type: "integer", value: "2" } },
    observed: { capacity: { type: "integer", value: "3" } },
    evidence: ["rule-before-evidence"],
  },
  after: {
    ruleId: "01900000-0000-7000-8000-000000000006",
    satisfied: true,
    affectedEntities: [{ kind: "person", id: "person-rule" }],
    messageKey: "required.rule",
    expected: { threshold: { type: "integer", value: "4" } },
    observed: { capacity: { type: "integer", value: "5" } },
    evidence: ["rule-after-evidence"],
  },
};
const changedMetric: MetricComparisonV1 = {
  metricId: "coverage",
  before: { type: "integer", value: "1" },
  after: { type: "integer", value: "2" },
};
const protections: readonly AssignmentProtectionComparisonV1[] = [
  {
    assignmentId: "hard-to-soft",
    before: { type: "hard" },
    after: { type: "soft", weight: 12 },
    beforeSelected: true,
    afterSelected: true,
    statePreserved: false,
    assignmentPreserved: true,
  },
  {
    assignmentId: "soft-weight-only",
    before: { type: "soft", weight: 2 },
    after: { type: "soft", weight: 9 },
    beforeSelected: true,
    afterSelected: true,
    statePreserved: false,
    assignmentPreserved: true,
  },
  {
    assignmentId: "selection-changed",
    before: { type: "unlocked" },
    after: { type: "hard" },
    beforeSelected: false,
    afterSelected: true,
    statePreserved: false,
    assignmentPreserved: false,
  },
  {
    assignmentId: "absent-to-unlocked",
    before: null,
    after: { type: "unlocked" },
    beforeSelected: false,
    afterSelected: false,
    statePreserved: false,
    assignmentPreserved: true,
  },
];

function comparison(overrides: Partial<SolutionComparisonV2> = {}): SolutionComparisonV2 {
  const location = { kind: "location", id: "location-1" };
  const team = { kind: "team", id: "team-1" };
  const person = { kind: "person", id: "person-1" };
  return {
    schemaVersion: 2,
    base: binding("base"),
    candidate: binding("candidate"),
    assignments: [
      { type: "added", after: assignment("assignment-added") },
      { type: "removed", before: assignment("assignment-removed", { type: "absent" }) },
      {
        type: "changed",
        before: assignment("assignment-changed", { type: "integer", value: "1" }),
        after: assignment("assignment-changed", { type: "integer", value: "2" }),
      },
    ],
    rules: [changedRule],
    score: { type: "unavailable", reason: "policyOrScopeMismatch" },
    metrics: [changedMetric],
    protections,
    runs,
    affectedEntities: [person, location, team],
    entityObservations: [
      { entity: person, beforeLabel: "Ada Former", afterLabel: "Ada Rivera" },
      { entity: location, beforeLabel: "Old location", afterLabel: null },
      { entity: team, beforeLabel: "Evening team", afterLabel: "Night team" },
    ],
    checksum: "comparison-checksum",
    ...overrides,
  };
}

function renderComparison(value: SolutionComparisonV2 = comparison()): void {
  render(AcceptedComparison, {
    props: {
      comparison: value,
      scenarioId,
      currentRevision: revision,
      baseLabel: "Previous result",
      candidateLabel: "Latest result",
    },
  });
}

afterEach(() => {
  cleanup();
});

describe("AcceptedComparison", () => {
  it("renders source labels, changed evidence, protection state and selection independently, and recorded runs without proof claims", async () => {
    renderComparison();

    expect(screen.getByRole("heading", { name: "Accepted result comparison" })).toBeVisible();
    const bindings = screen.getByText(/Previous result.*Latest result/);
    expect(bindings).toHaveTextContent(baseSolutionId);
    expect(bindings).toHaveTextContent(candidateSolutionId);
    expect(screen.getByText("Score comparison unavailable")).toBeVisible();
    expect(screen.getByText(/Before source label: Ada Former/)).toBeVisible();
    expect(screen.getByText(/After source label: Ada Rivera/)).toBeVisible();
    expect(screen.getByText("Before source label: Old location.")).toBeVisible();
    expect(screen.getByText("After source label: Not present.")).toBeVisible();
    expect(screen.getByText("Before source label: Evening team.")).toBeVisible();
    expect(screen.getByText("After source label: Night team.")).toBeVisible();
    expect(screen.getByText("coverage")).toBeVisible();
    await userEvent.click(screen.getByText("Inspect changed facts and evidence"));
    expect(screen.getByText("rule-before-evidence")).toBeVisible();
    const protectionRecord = (assignmentId: string) => {
      const article = screen.getByText(assignmentId).closest("article");
      if (article === null) throw new Error(`Missing protection record ${assignmentId}`);
      return within(article);
    };
    const hard = protectionRecord("hard-to-soft");
    expect(hard.getByText("Before protection: Hard.")).toBeVisible();
    expect(hard.getByText("After protection: Soft (weight 12).")).toBeVisible();
    expect(hard.getByText("Protection state preserved: No.")).toBeVisible();
    expect(hard.getByText("Selection preserved: Yes.")).toBeVisible();
    const soft = protectionRecord("soft-weight-only");
    expect(soft.getByText("Before protection: Soft (weight 2).")).toBeVisible();
    expect(soft.getByText("After protection: Soft (weight 9).")).toBeVisible();
    expect(soft.getByText("Before selected: Selected.")).toBeVisible();
    expect(soft.getByText("After selected: Selected.")).toBeVisible();
    expect(soft.getByText("Protection state preserved: No.")).toBeVisible();
    expect(soft.getByText("Selection preserved: Yes.")).toBeVisible();
    const changed = protectionRecord("selection-changed");
    expect(changed.getByText("Before selected: Not selected.")).toBeVisible();
    expect(changed.getByText("After selected: Selected.")).toBeVisible();
    expect(changed.getByText("Selection preserved: No.")).toBeVisible();
    const absent = protectionRecord("absent-to-unlocked");
    expect(absent.getByText("Before protection: Absent.")).toBeVisible();
    expect(absent.getByText("After protection: Unlocked.")).toBeVisible();
    expect(absent.getByText("Before selected: Not selected.")).toBeVisible();
    expect(absent.getByText("After selected: Not selected.")).toBeVisible();
    await userEvent.click(screen.getByText("Source identities and recorded run metadata"));
    expect(screen.getByText("base-document-hash")).toBeVisible();
    expect(screen.getByText(baseRunId)).toBeVisible();
    expect(screen.getByText("candidate-manifest-checksum")).toBeVisible();
    expect(screen.getByText("Recorded accepted status: optimal.")).toBeVisible();
    expect(screen.queryByText(/Backend proof/)).toBeNull();
    expect(screen.queryByText(/Candidate ordering/)).toBeNull();
    expect(screen.getByTestId("read-only-notice")).toBeVisible();
  });

  it("shows compatible score levels and pages all categories without dropping them", async () => {
    const categories = Array.from({ length: 21 }, (_, index) => ({
      categoryId: `category-${String(index)}`,
      before: String(index),
      after: String(index + 1),
      delta: "1",
    }));
    const level: ScoreLevelComparisonV1 = {
      levelId: "fairness",
      direction: "minimize",
      before: "4",
      after: "2",
      delta: "-2",
      categories,
    };
    renderComparison(
      comparison({
        score: {
          type: "comparable",
          baseScore: { feasibility: "1", levels: [] },
          candidateScore: { feasibility: "1", levels: [] },
          scoreLevels: [level],
          ordering: "better",
        },
      }),
    );

    expect(screen.getByText("better")).toBeVisible();
    const list = screen.getByRole("list", { name: "Score categories" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(20);
    expect(screen.queryByText("category-20")).toBeNull();
    const navigation = screen.getByRole("navigation", {
      name: "Score categories pages for fairness",
    });
    await userEvent.click(within(navigation).getByRole("button", { name: "Next" }));
    expect(screen.getByText("category-20")).toBeVisible();
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
  });

  it("shows the other unavailable reason and keeps empty semantic evidence explicit", () => {
    renderComparison(
      comparison({
        score: { type: "unavailable", reason: "scoreShapeMismatch" },
        assignments: [],
        rules: [],
        metrics: [],
        protections: [],
        affectedEntities: [],
        entityObservations: [],
      }),
    );
    expect(screen.getByText("The accepted results use different score shapes.")).toBeVisible();
    expect(
      screen.getByText("No semantic changes were found between these accepted results."),
    ).toBeVisible();
  });
});
