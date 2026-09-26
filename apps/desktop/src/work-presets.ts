import { newUuidV7, type ScenarioCommand } from "./api/generated";
import {
  WORKFORCE_ADD_ENTITY_COMMAND_ID,
  WORKFORCE_UPDATE_ENTITY_COMMAND_ID,
  type WorkforceDateRange,
} from "./api/generated-domain-pack-contracts";
import type { WorkRecord } from "./work-record-draft";
import { messages } from "./messages";

/** Editable synthetic proposals, not clinical recommendations or saved scenario authority. */
export function createClinicWorkPreset(dates: WorkforceDateRange): readonly WorkRecord[] {
  const bucketId = newUuidV7();
  const locationId = newUuidV7();
  const clinicTypeId = newUuidV7();
  const onCallTypeId = newUuidV7();
  const copy = messages.work.presets;
  return [
    {
      kind: "workloadBucket",
      id: bucketId,
      name: copy.bucket,
      measurement: "elapsedMinutes",
      overlappingContribution: "sum",
    },
    { kind: "location", id: locationId, name: copy.location, transitions: [] },
    {
      kind: "assignmentType",
      id: clinicTypeId,
      name: copy.clinicType,
      category: "clinic",
      defaultDurationMinutes: 240,
      qualifications: { kind: "unconstrained" },
      locationBehavior: { kind: "fixed", locationId },
      timeBehavior: "localWallClock",
      workloadBucketIds: [bucketId],
    },
    {
      kind: "assignmentType",
      id: onCallTypeId,
      name: copy.onCallType,
      category: "on-call",
      defaultDurationMinutes: 720,
      qualifications: { kind: "unconstrained" },
      locationBehavior: { kind: "none" },
      timeBehavior: "elapsed",
      workloadBucketIds: [bucketId],
    },
    {
      kind: "shiftTemplate",
      id: newUuidV7(),
      name: copy.clinicTemplate,
      assignmentTypeId: clinicTypeId,
      locationId,
      occurrenceIdentities: {},
      recurrence: {
        effectiveRange: { ...dates },
        weekdays: ["monday", "tuesday", "wednesday", "thursday", "friday"],
        excludedDates: [],
      },
      timing: { kind: "localWindow", startTime: "08:00:00", endTime: "12:00:00", endDayOffset: 0 },
      coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
      reportingAttribution: "startLocalDate",
      tags: [],
    },
    {
      kind: "shiftTemplate",
      id: newUuidV7(),
      name: copy.onCallTemplate,
      assignmentTypeId: onCallTypeId,
      occurrenceIdentities: {},
      recurrence: {
        effectiveRange: { ...dates },
        weekdays: ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"],
        excludedDates: [],
      },
      timing: { kind: "elapsedDuration", startTime: "20:00:00", durationMinutes: 720 },
      coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
      reportingAttribution: "startLocalDate",
      tags: [],
    },
  ];
}

/** Retain constructor order; defer transitions until every proposed location exists. */
export function workPresetCommand(records: readonly WorkRecord[]): ScenarioCommand {
  const commands: ScenarioCommand[] = records.map((record) => ({
    type: "applyDomainCommand",
    payload: {
      commandType: WORKFORCE_ADD_ENTITY_COMMAND_ID,
      payload: {
        entity:
          record.kind === "location" && record.transitions.length !== 0
            ? { ...record, transitions: [] }
            : record,
      },
    },
  }));
  for (const record of records) {
    if (record.kind === "location" && record.transitions.length !== 0)
      commands.push({
        type: "applyDomainCommand",
        payload: { commandType: WORKFORCE_UPDATE_ENTITY_COMMAND_ID, payload: { entity: record } },
      });
  }
  return { type: "applyBatch", payload: { label: messages.work.presets.label, commands } };
}
