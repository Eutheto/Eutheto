import type {
  WorkforceAssignmentType,
  WorkforceQualification,
  WorkforceTeam,
} from "./api/generated-domain-pack-contracts";
import { parseDurationDraft } from "./components/planner/duration-field";
import type { DurationDraft } from "./components/planner/field-contracts";
import { messages } from "./messages";
import { sameField } from "./entity-draft";

export type SupportingRecord = WorkforceQualification | WorkforceTeam | WorkforceAssignmentType;

export type SupportingRecordDraft =
  | {
      readonly kind: "qualification";
      readonly name: string;
      readonly description: string;
    }
  | {
      readonly kind: "team";
      readonly name: string;
    }
  | {
      readonly kind: "assignmentType";
      readonly name: string;
      readonly category: string;
      readonly duration: DurationDraft;
      readonly qualificationMode: "unconstrained" | "matches" | "";
      readonly allQualificationIds: readonly string[];
      readonly anyQualificationIds: readonly string[];
      readonly locationMode: "none" | "optional" | "required" | "fixed" | "";
      readonly locationId: string;
      readonly timeBehavior: "localWallClock" | "elapsed" | "";
      readonly workloadBucketIds: readonly string[];
    };

export function createSupportingRecordDraft(
  value: SupportingRecord | SupportingRecord["kind"],
  retained?: { readonly value: SupportingRecord; readonly raw: SupportingRecordDraft },
): SupportingRecordDraft {
  if (typeof value === "string") {
    switch (value) {
      case "qualification":
        return { kind: value, name: "", description: "" };
      case "team":
        return { kind: value, name: "" };
      case "assignmentType":
        return {
          kind: value,
          name: "",
          category: "",
          duration: { raw: "", unit: "minutes", status: "empty" },
          qualificationMode: "",
          allQualificationIds: [],
          anyQualificationIds: [],
          locationMode: "",
          locationId: "",
          timeBehavior: "",
          workloadBucketIds: [],
        };
    }
  }
  switch (value.kind) {
    case "qualification":
      return { kind: value.kind, name: value.name, description: value.description };
    case "team":
      return { kind: value.kind, name: value.name };
    case "assignmentType": {
      const previous =
        retained?.value.kind === value.kind && retained.raw.kind === value.kind
          ? retained
          : undefined;
      const raw = previous?.raw.kind === "assignmentType" ? previous.raw : undefined;
      const native = previous?.value.kind === "assignmentType" ? previous.value : undefined;
      const qualifications =
        raw && sameField(value.qualifications, native?.qualifications) ? raw : undefined;
      const location =
        raw && sameField(value.locationBehavior, native?.locationBehavior) ? raw : undefined;
      return {
        kind: value.kind,
        name: value.name,
        category: value.category,
        duration:
          raw && value.defaultDurationMinutes === native?.defaultDurationMinutes
            ? raw.duration
            : parseDurationDraft(String(value.defaultDurationMinutes), "minutes", 1),
        qualificationMode: qualifications?.qualificationMode ?? value.qualifications.kind,
        allQualificationIds:
          qualifications?.allQualificationIds ??
          (value.qualifications.kind === "matches" ? value.qualifications.allQualificationIds : []),
        anyQualificationIds:
          qualifications?.anyQualificationIds ??
          (value.qualifications.kind === "matches" ? value.qualifications.anyQualificationIds : []),
        locationMode: location?.locationMode ?? value.locationBehavior.kind,
        locationId:
          location?.locationId ??
          (value.locationBehavior.kind === "fixed" ? value.locationBehavior.locationId : ""),
        timeBehavior: value.timeBehavior,
        workloadBucketIds: value.workloadBucketIds,
      };
    }
  }
}

/** Converts raw representation only; native preview owns domain and reference validation. */
export function supportingRecordValue(
  id: string,
  draft: SupportingRecordDraft,
): { value: SupportingRecord | null; errors: Readonly<Record<string, string>> } {
  const errors: Record<string, string> = {};
  switch (draft.kind) {
    case "qualification":
      return {
        value: { kind: draft.kind, id, name: draft.name, description: draft.description },
        errors,
      };
    case "team":
      return { value: { kind: draft.kind, id, name: draft.name }, errors };
    case "assignmentType": {
      // The native field is u32 and validation requires > 0: exact whole minutes, minimum 1.
      // Reparse the raw text rather than trusting a potentially stale cached quantity/status.
      const duration = parseDurationDraft(draft.duration.raw, draft.duration.unit, 1);
      const copy = messages.supportingFields;
      if (duration.status !== "valid") {
        errors.duration =
          duration.status === "empty" ? copy.durationRequired : copy.durationErrors[duration.error];
      }
      if (draft.qualificationMode === "") errors.qualificationMode = copy.qualificationRequired;
      if (draft.locationMode === "") errors.locationMode = copy.locationRequired;
      if (draft.locationMode === "fixed" && draft.locationId === "")
        errors.locationId = copy.fixedLocationRequired;
      if (draft.timeBehavior === "") errors.timeBehavior = copy.timeBehaviorRequired;
      if (
        duration.status !== "valid" ||
        draft.qualificationMode === "" ||
        draft.locationMode === "" ||
        (draft.locationMode === "fixed" && draft.locationId === "") ||
        draft.timeBehavior === ""
      ) {
        return { value: null, errors };
      }
      return {
        errors,
        value: {
          kind: draft.kind,
          id,
          name: draft.name,
          category: draft.category,
          defaultDurationMinutes: duration.minutes,
          qualifications:
            draft.qualificationMode === "matches"
              ? {
                  kind: "matches",
                  allQualificationIds: draft.allQualificationIds,
                  anyQualificationIds: draft.anyQualificationIds,
                }
              : { kind: "unconstrained" },
          locationBehavior:
            draft.locationMode === "fixed"
              ? { kind: "fixed", locationId: draft.locationId }
              : { kind: draft.locationMode },
          timeBehavior: draft.timeBehavior,
          workloadBucketIds: draft.workloadBucketIds,
        },
      };
    }
  }
}
