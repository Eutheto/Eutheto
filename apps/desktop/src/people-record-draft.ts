import type { WorkforceEntity, WorkforcePerson } from "./api/generated-domain-pack-contracts";
import {
  createPersonFieldsDraft,
  personFieldsValue,
  type PersonFieldsDraft,
} from "./person-fields";
import {
  createSupportingRecordDraft,
  supportingRecordValue,
  type SupportingRecord,
  type SupportingRecordDraft,
} from "./supporting-record-fields";
import { sameField } from "./entity-draft";

export type PeopleRecord = WorkforcePerson | SupportingRecord;
export type PeopleRecordDraft =
  | SupportingRecordDraft
  | {
      readonly kind: "person";
      readonly name: string;
      readonly externalIdEnabled: boolean;
      readonly externalId: string;
      readonly fields: PersonFieldsDraft;
    };
export function isPeopleRecord(entity: WorkforceEntity): entity is PeopleRecord {
  return (
    entity.kind === "person" ||
    entity.kind === "qualification" ||
    entity.kind === "team" ||
    entity.kind === "assignmentType"
  );
}
export function createPeopleRecordDraft(
  record: PeopleRecord | PeopleRecord["kind"],
  retained?: { readonly value: PeopleRecord; readonly raw: PeopleRecordDraft },
): PeopleRecordDraft {
  if (record === "person" || (typeof record !== "string" && record.kind === "person")) {
    const person = record === "person" ? null : record;
    const previous =
      retained?.value.kind === "person" && retained.raw.kind === "person"
        ? { value: retained.value, raw: retained.raw }
        : undefined;
    const external =
      previous && sameField(person?.externalId, previous.value.externalId)
        ? previous.raw
        : undefined;
    return {
      kind: "person",
      name: person?.name ?? "",
      externalIdEnabled: external?.externalIdEnabled ?? person?.externalId !== undefined,
      externalId: external?.externalId ?? person?.externalId ?? "",
      fields: createPersonFieldsDraft(
        person ?? undefined,
        previous && {
          value: previous.value,
          raw: previous.raw.fields,
        },
      ),
    };
  }
  return createSupportingRecordDraft(
    record,
    retained && retained.value.kind !== "person" && retained.raw.kind !== "person"
      ? { value: retained.value, raw: retained.raw }
      : undefined,
  );
}
export function peopleRecordValue(
  id: string,
  draft: PeopleRecordDraft,
): {
  readonly value: PeopleRecord | null;
  readonly errors: Readonly<Record<string, string>>;
} {
  if (draft.kind !== "person") return supportingRecordValue(id, draft);
  const fields = personFieldsValue(draft.fields);
  if (fields.value === null) return { value: null, errors: fields.errors };
  return {
    errors: fields.errors,
    value: {
      ...fields.value,
      id,
      kind: "person",
      name: draft.name,
      ...(draft.externalIdEnabled ? { externalId: draft.externalId } : {}),
    },
  };
}
