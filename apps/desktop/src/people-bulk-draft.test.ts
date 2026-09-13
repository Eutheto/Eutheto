import { describe, expect, it } from "vitest";
import type { WorkforcePerson } from "./api/generated-domain-pack-contracts";
import { resolveEntityDraftField } from "./entity-draft";
import { peopleBulkCandidates, rebasePeopleBulkPerson } from "./people-bulk-draft";

const team = "01900000-0000-7000-8000-000000000002";
const otherTeam = "01900000-0000-7000-8000-000000000003";
const person: WorkforcePerson = {
  id: "01900000-0000-7000-8000-000000000001",
  kind: "person",
  name: "Captured person",
  activeRange: { kind: "always" },
  qualificationGrants: [],
  eligibleAssignmentTypeIds: [],
  workloadWeight: { numerator: 1, denominator: 1 },
  tags: [],
  teamIds: [team],
};

const add = { kind: "addTeam" as const, teamId: team };

describe("People bulk rebase intent", () => {
  it("requires a choice when a concurrent writer defeats an initially no-op action", () => {
    const desired = peopleBulkCandidates([person], add)[0];
    if (desired === undefined) throw new Error("Missing selected person");
    const current = { ...person, teamIds: [], name: "Renamed remotely", tags: ["remote"] };
    const review = rebasePeopleBulkPerson(person, desired, current, add);
    expect(review.conflicts).toEqual(["teamIds"]);
    expect(resolveEntityDraftField(review, "teamIds", "draft").value).toMatchObject({
      name: "Renamed remotely",
      tags: ["remote"],
      teamIds: [team],
    });
    expect(resolveEntityDraftField(review, "teamIds", "current").value.teamIds).toEqual([]);
  });

  it("carries resolved values and unresolved conflicts separately through later revisions", () => {
    const base = { ...person, teamIds: [] };
    const current = { ...person, teamIds: [otherTeam] };
    const first = rebasePeopleBulkPerson(base, person, current, add);
    const later = { ...current, tags: ["later unrelated edit"] };
    const pending = rebasePeopleBulkPerson(first.current, first.value, later, add, first.conflicts);
    expect(pending.conflicts).toEqual(["teamIds"]);
    for (const choice of ["current", "draft"] as const) {
      const chosen = resolveEntityDraftField(first, "teamIds", choice);
      const continued = rebasePeopleBulkPerson(
        chosen.current,
        chosen.value,
        later,
        add,
        chosen.conflicts,
      );
      expect(continued.conflicts).toEqual([]);
      expect(continued.value.teamIds).toEqual(choice === "current" ? [otherTeam] : [team]);
      expect(continued.value.tags).toEqual(["later unrelated edit"]);
      const changedAgain = rebasePeopleBulkPerson(
        continued.current,
        continued.value,
        { ...later, teamIds: [] },
        add,
        continued.conflicts,
      );
      expect(changedAgain.conflicts).toEqual(["teamIds"]);
    }
  });
});
