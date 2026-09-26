import type { ScenarioCommand } from "./api/generated";
import {
  WORKFORCE_UPDATE_ENTITY_COMMAND_ID,
  type WorkforcePerson,
} from "./api/generated-domain-pack-contracts";
import { messages } from "./messages";

export const ELIGIBILITY_PERSON_BYTES = 8 * 1024 * 1024;
export const ELIGIBILITY_COMMAND_BYTES = 16 * 1024 * 1024;

/** An explicit desired membership, never a toggle against a later revision. */
export interface EligibilityCellEdit {
  readonly personId: string;
  readonly typeId: string;
  readonly before: boolean;
  readonly after: boolean;
}
export interface EligibilityReviewCell extends EligibilityCellEdit {
  readonly personName: string;
  readonly typeName: string;
}
export type EligibilityEdits = ReadonlyMap<string, EligibilityCellEdit>;

export function eligibilityCellKey(personId: string, typeId: string): string {
  return `${personId}:${typeId}`;
}

/** Preserve unselected memberships, their order, and every other person field. */
export function eligibilityBatchCommand(
  edits: EligibilityEdits,
  people: ReadonlyMap<string, WorkforcePerson>,
): ScenarioCommand {
  if (edits.size === 0) throw new Error(messages.eligibilityUi.draft.emptySelection);
  const byPerson = new Map<string, Map<string, boolean>>();
  for (const edit of edits.values()) {
    let choices = byPerson.get(edit.personId);
    if (choices === undefined) {
      choices = new Map();
      byPerson.set(edit.personId, choices);
    }
    choices.set(edit.typeId, edit.after);
  }
  const commands: ScenarioCommand[] = [];
  for (const [personId, choices] of byPerson) {
    const person = people.get(personId);
    if (person?.id !== personId) throw new Error(messages.eligibilityUi.draft.personUnavailable);
    const membership = person.eligibleAssignmentTypeIds.filter((id) => choices.get(id) !== false);
    const present = new Set(membership);
    for (const [typeId, allowed] of choices) {
      if (allowed && !present.has(typeId)) {
        membership.push(typeId);
        present.add(typeId);
      }
    }
    commands.push({
      type: "applyDomainCommand",
      payload: {
        commandType: WORKFORCE_UPDATE_ENTITY_COMMAND_ID,
        payload: { entity: { ...person, eligibleAssignmentTypeIds: membership } },
      },
    });
  }
  const command: ScenarioCommand = {
    type: "applyBatch",
    payload: { label: messages.eligibilityUi.draft.commandLabel, commands },
  };
  if (new TextEncoder().encode(JSON.stringify(command)).byteLength > ELIGIBILITY_COMMAND_BYTES) {
    throw Object.assign(new Error(messages.eligibilityUi.draft.commandLimit), {
      category: "validation",
      code: "desktop.eligibility.commandLimit",
    });
  }
  return command;
}
