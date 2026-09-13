import type { ScenarioCommand } from "./api/generated";
import {
  WORKFORCE_REMOVE_ENTITY_COMMAND_ID,
  WORKFORCE_UPDATE_ENTITY_COMMAND_ID,
  type WorkforceActiveRange,
  type WorkforcePerson,
} from "./api/generated-domain-pack-contracts";
import { sameField, type EntityField, type EntityRebase } from "./entity-draft";

export interface PeopleBulkSelection {
  readonly scenarioId: string;
  readonly revision: number;
  readonly libraryEpoch: number;
  readonly ids: readonly string[];
}

export type PeopleBulkAction =
  | { readonly kind: "activeRange"; readonly range: WorkforceActiveRange }
  | { readonly kind: "addTeam" | "removeTeam"; readonly teamId: string }
  | { readonly kind: "delete" };

/** These are proposals over captured native records, never an authoritative scenario cache. */
export function peopleBulkCandidates(
  people: readonly WorkforcePerson[],
  action: PeopleBulkAction,
): readonly WorkforcePerson[] {
  if (action.kind === "delete") return people;
  return people.map((person) => {
    switch (action.kind) {
      case "activeRange":
        return sameField(person.activeRange, action.range)
          ? person
          : { ...person, activeRange: action.range };
      case "addTeam":
        return person.teamIds.includes(action.teamId)
          ? person
          : { ...person, teamIds: [...person.teamIds, action.teamId] };
      case "removeTeam":
        return person.teamIds.includes(action.teamId)
          ? { ...person, teamIds: person.teamIds.filter((id) => id !== action.teamId) }
          : person;
    }
  });
}

/** The selected action owns its whole field even when it initially changes nothing. */
export function rebasePeopleBulkPerson(
  base: WorkforcePerson,
  local: WorkforcePerson,
  current: WorkforcePerson,
  action: Exclude<PeopleBulkAction, { readonly kind: "delete" }>,
  unresolved: readonly EntityField<WorkforcePerson>[] = [],
): EntityRebase<WorkforcePerson> {
  if (base.id !== local.id || base.id !== current.id)
    throw new Error("A bulk draft cannot be rebased onto a different person identity.");
  const field = action.kind === "activeRange" ? "activeRange" : "teamIds";
  const conflicts: EntityField<WorkforcePerson>[] = [];
  if (
    !sameField(current[field], local[field]) &&
    (unresolved.includes(field) || !sameField(current[field], base[field]))
  )
    conflicts.push(field);
  return {
    local,
    current,
    value:
      action.kind === "activeRange"
        ? { ...current, activeRange: local.activeRange }
        : { ...current, teamIds: local.teamIds },
    conflicts,
  };
}

export function peopleBulkCommand(
  people: readonly WorkforcePerson[],
  action: PeopleBulkAction,
  label: string,
): ScenarioCommand {
  return {
    type: "applyBatch",
    payload: {
      label,
      commands: people.map((person) => ({
        type: "applyDomainCommand",
        payload:
          action.kind === "delete"
            ? { commandType: WORKFORCE_REMOVE_ENTITY_COMMAND_ID, payload: { entityId: person.id } }
            : { commandType: WORKFORCE_UPDATE_ENTITY_COMMAND_ID, payload: { entity: person } },
      })),
    },
  };
}
