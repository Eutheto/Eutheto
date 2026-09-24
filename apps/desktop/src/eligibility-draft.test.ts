import { expect, it } from "vitest";
import type { WorkforcePerson } from "./api/generated-domain-pack-contracts";
import {
  eligibilityBatchCommand,
  eligibilityCellKey,
  type EligibilityCellEdit,
} from "./eligibility-draft";

it("preserves desired memberships after concurrent changes instead of replaying toggles", () => {
  const personId = "01900000-0000-7000-8000-000000000001";
  const unrelated = "01900000-0000-7000-8000-000000000002";
  const allow = "01900000-0000-7000-8000-000000000003";
  const disallow = "01900000-0000-7000-8000-000000000004";
  const keepAllowed = "01900000-0000-7000-8000-000000000005";
  const current: WorkforcePerson = {
    id: personId,
    kind: "person",
    name: "Current native person",
    activeRange: { kind: "always" },
    qualificationGrants: [],
    eligibleAssignmentTypeIds: [unrelated, allow],
    workloadWeight: { numerator: 1, denominator: 1 },
    tags: [],
    teamIds: [],
  };
  // Another writer already applied the first two choices and defeated the
  // explicit no-op choice. All three desired values must survive fresh review.
  const choices: EligibilityCellEdit[] = [
    { personId, typeId: allow, before: false, after: true },
    { personId, typeId: disallow, before: true, after: false },
    { personId, typeId: keepAllowed, before: true, after: true },
  ];
  const command = eligibilityBatchCommand(
    new Map(choices.map((choice) => [eligibilityCellKey(personId, choice.typeId), choice])),
    new Map([[personId, current]]),
  );
  expect(command).toMatchObject({
    type: "applyBatch",
    payload: {
      commands: [
        {
          payload: {
            payload: {
              entity: {
                eligibleAssignmentTypeIds: [unrelated, allow, keepAllowed],
              },
            },
          },
        },
      ],
    },
  });
});
