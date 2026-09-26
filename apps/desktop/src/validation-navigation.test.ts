import { describe, expect, it } from "vitest";
import type { ValidationIssue } from "./api/generated";
import { diagnosticAddress, issueAddress, validationRouteTarget } from "./validation-navigation";

const owner = "01900000-0000-7000-8000-000000000006";
const rule = "01900000-0000-7000-8000-000000000040";
const context = {
  scenarioId: "01900000-0000-7000-8000-000000000001",
  revision: 7,
  libraryEpoch: 3,
};

describe("native diagnostic navigation", () => {
  it("keeps the authored coverage owner and row instead of the triggering rule reference", () => {
    const issue: ValidationIssue = {
      code: "official.workforce.qualification_shortage",
      severity: "error",
      message: "Native coverage contradiction",
      fieldPath: `domain.entities.${owner}.coverage.qualificationMinimums.1.minimum`,
      resource: { type: "rule", id: rule },
    };
    expect(issueAddress(issue)).toEqual({
      collection: "entities",
      id: owner,
      fieldPath: ["coverage", "qualificationMinimums", "1", "minimum"],
    });
    expect(issueAddress({ ...issue, fieldPath: null })).toBeNull();
  });

  it("preserves the native temporal endpoint and refuses selector-shaped paths", () => {
    expect(diagnosticAddress(`/domain/entities/${owner}/timeWindow/windows/1/endTime`)).toEqual({
      collection: "entities",
      id: owner,
      fieldPath: ["timeWindow", "windows", "1", "endTime"],
    });
    expect(
      diagnosticAddress(`/domain/entities/${owner}/timeWindow/windows/1/startTime,input`),
    ).toBeNull();
    expect(diagnosticAddress(`/domain/entities/${owner}/timeWindow/windows/-1/endTime`)).toBeNull();
  });

  it("does not retarget a diagnostic after revision or library replacement", () => {
    const query = {
      findingPath: `/domain/entities/${owner}/name`,
      findingRevision: "7",
      findingEpoch: "3",
    };
    expect(validationRouteTarget(query, context)).toEqual({
      ...context,
      collection: "entities",
      id: owner,
      fieldPath: ["name"],
    });
    expect(validationRouteTarget(query, { ...context, revision: 8 })).toBeNull();
    expect(validationRouteTarget(query, { ...context, libraryEpoch: 4 })).toBeNull();
  });
});
