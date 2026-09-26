import type { DomainEntityRef } from "../../api/generated";
import type { LabeledEntityRef } from "../explanations/types";
import { plannerMessage } from "./messages";

export const ENTITY_PAGE_LIMIT = 200;
export const SELECTION_PAGE_SIZE = 50;

/** Tuple encoding is unambiguous even when a kind or ID contains punctuation. */
export function entityKey(entity: DomainEntityRef): string {
  return JSON.stringify([entity.kind, entity.id]);
}

export function entityLabel(
  entity: DomainEntityRef,
  metadata: LabeledEntityRef | null | undefined,
): string {
  return metadata && metadata.entity.kind === entity.kind && metadata.entity.id === entity.id
    ? metadata.label
    : plannerMessage("entity.unnamed", { ...entity });
}

export function uniqueEntities(entities: readonly DomainEntityRef[]): DomainEntityRef[] {
  const seen = new Set<string>();
  return entities.filter((entity) => {
    const key = entityKey(entity);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
