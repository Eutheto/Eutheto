import { newUuidV7 } from "./api/generated";
import type {
  WorkforceDeviationPenalty,
  WorkforceEntity,
  WorkforcePeerGroup,
  WorkforcePersonSelection,
  WorkforcePersonTarget,
  WorkforcePriorityMapping,
  WorkforceScoreLevel,
  WorkforceScorePolicy,
  WorkforceWindowMembership,
  WorkforceWorkloadPolicy,
  WorkforceWorkloadTargetMode,
} from "./api/generated-domain-pack-contracts";
import { sameField, type EntityRecord } from "./entity-draft";

export type ScorePolicyTargetModeValue = WorkforceWorkloadTargetMode;
export type ScorePolicyWorkloadPolicyValue = Omit<WorkforceWorkloadPolicy, "targetMode"> & {
  readonly targetMode: ScorePolicyTargetModeValue;
};
export type ScorePolicyValue = Omit<WorkforceScorePolicy, "workloadPolicies"> & {
  readonly workloadPolicies: Readonly<Record<string, ScorePolicyWorkloadPolicyValue>>;
};

export interface ScorePolicyLevelDraft {
  readonly key: string;
  readonly levelKey: string;
  readonly label: string;
}
export interface ScorePolicyPriorityMappingDraft {
  readonly key: string;
  readonly levelKey: string;
  readonly priority: "low" | "normal" | "high" | "veryHigh";
  readonly scale: string;
}
export type ScorePolicyPeopleDraft =
  | { readonly kind: "all" }
  | { readonly kind: "selected"; readonly personIds: readonly string[] }
  | {
      readonly kind: "filter";
      readonly allTags: readonly string[];
      readonly anyTags: readonly string[];
    };
export interface ScorePolicyPeerGroupDraft {
  readonly people: ScorePolicyPeopleDraft;
  readonly teamIds: readonly string[];
}
export interface ScorePolicyExplicitTargetDraft {
  readonly key: string;
  readonly personId: string;
  readonly target: string;
}
export type ScorePolicyTargetModeDraft =
  | { readonly kind: "explicit"; readonly targets: readonly ScorePolicyExplicitTargetDraft[] }
  | { readonly kind: "personTargets" }
  | { readonly kind: "weightedEqualShare" }
  | { readonly kind: "equalShare" };
export interface ScorePolicyBreakpointDraft {
  readonly key: string;
  readonly fromDeviation: string;
  readonly slope: string;
}
export type ScorePolicyPenaltyDraft =
  | { readonly kind: "absolute" }
  | { readonly kind: "piecewise"; readonly breakpoints: readonly ScorePolicyBreakpointDraft[] };
export interface ScorePolicyWorkloadPolicyDraft {
  readonly key: string;
  readonly id: string;
  readonly bucketId: string;
  readonly calendarId: string;
  readonly membership: WorkforceWindowMembership | "";
  readonly peerGroup: ScorePolicyPeerGroupDraft;
  readonly targetMode: ScorePolicyTargetModeDraft | null;
  readonly penalty: ScorePolicyPenaltyDraft | null;
}

/** Raw editor state retains incomplete numbers, empty references, and row identity. */
export interface ScorePolicyDraft extends EntityRecord {
  readonly kind: "scorePolicy";
  readonly profileKey: string;
  readonly tieBreak: "stableAssignmentRank";
  readonly levels: readonly ScorePolicyLevelDraft[];
  readonly priorityMapping: readonly ScorePolicyPriorityMappingDraft[];
  readonly workloadPolicies: readonly ScorePolicyWorkloadPolicyDraft[];
}

export interface ScorePolicyDraftResult {
  readonly value: ScorePolicyValue | null;
  readonly errors: Readonly<Record<string, string>>;
}

const MAX_UINT32 = 4_294_967_295;
const MAX_WEIGHT = 1_000_000;
function isMembership(value: string): value is WorkforceWindowMembership {
  return value === "reportingDate" || value === "startInstant" || value === "intersection";
}
function rowKey(prefix: string): string {
  return `${prefix}-${newUuidV7()}`;
}
function rawNumber(value: number): string {
  return String(value);
}
function parseWhole(
  value: string,
  path: string,
  errors: Record<string, string>,
  maximum: number,
  minimum = 0,
): number | null {
  if (!/^\d+$/.test(value)) {
    errors[path] = "Enter a whole number.";
    return null;
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < minimum || parsed > maximum) {
    errors[path] = `Enter a whole number from ${String(minimum)} to ${String(maximum)}.`;
    return null;
  }
  return parsed;
}
function splitTags(value: readonly string[]): readonly string[] {
  return value
    .flatMap((item) => item.split(/[\n,]/u))
    .map((item) => item.trim())
    .filter((item) => item.length !== 0);
}
function restore<T>(value: T, before: T | undefined, raw: T | undefined): T {
  return raw !== undefined && before !== undefined && sameField(value, before) ? raw : value;
}
function peopleDraft(value: WorkforcePersonSelection): ScorePolicyPeopleDraft {
  if (value.kind === "all") return { kind: "all" };
  if (value.kind === "selected") return { kind: "selected", personIds: [...value.personIds] };
  return { kind: "filter", allTags: [...value.allTags], anyTags: [...value.anyTags] };
}
function peerDraft(value: WorkforcePeerGroup): ScorePolicyPeerGroupDraft {
  return { people: peopleDraft(value.people), teamIds: [...(value.teamIds ?? [])] };
}
function targetDraft(value: ScorePolicyTargetModeValue): ScorePolicyTargetModeDraft {
  if (value.kind === "explicit")
    return {
      kind: "explicit",
      targets: value.targets.map((target) => ({
        key: rowKey("target"),
        personId: target.personId,
        target: rawNumber(target.target),
      })),
    };
  return { kind: value.kind };
}
function penaltyDraft(value: WorkforceDeviationPenalty): ScorePolicyPenaltyDraft {
  if (value.kind === "absolute") return { kind: "absolute" };
  return {
    kind: "piecewise",
    breakpoints: value.breakpoints.map((breakpoint) => ({
      key: rowKey("breakpoint"),
      fromDeviation: rawNumber(breakpoint.fromDeviation),
      slope: rawNumber(breakpoint.slope),
    })),
  };
}
function workloadDraft(value: ScorePolicyWorkloadPolicyValue): ScorePolicyWorkloadPolicyDraft {
  return {
    key: rowKey("policy"),
    id: value.id,
    bucketId: value.bucketId,
    calendarId: value.calendarId,
    membership: value.membership,
    peerGroup: peerDraft(value.peerGroup),
    targetMode: targetDraft(value.targetMode),
    penalty: penaltyDraft(value.penalty),
  };
}

function retainField<Value, Raw>(
  value: Value,
  before: Value | undefined,
  raw: Raw | undefined,
  create: () => Raw,
): Raw {
  return raw !== undefined && before !== undefined && sameField(value, before) ? raw : create();
}

export function isScorePolicyValue(
  value: WorkforceEntity | ScorePolicyValue,
): value is ScorePolicyValue {
  return value.kind === "scorePolicy";
}

export function createScorePolicyDraft(
  value: ScorePolicyValue,
  retained?: { readonly value: ScorePolicyValue; readonly raw: ScorePolicyDraft },
): ScorePolicyDraft {
  const previous = retained?.value;
  const old = retained?.raw;
  return {
    id: value.id,
    kind: "scorePolicy",
    profileKey: restore(value.profileKey, previous?.profileKey, old?.profileKey ?? undefined),
    tieBreak: "stableAssignmentRank",
    levels: retainField(value.levels, previous?.levels, old?.levels, () =>
      value.levels.map((level) => ({
        key: rowKey("level"),
        levelKey: level.levelKey,
        label: level.label,
      })),
    ),
    priorityMapping: retainField(
      value.priorityMapping,
      previous?.priorityMapping,
      old?.priorityMapping,
      () =>
        value.priorityMapping.map((mapping) => ({
          key: rowKey("mapping"),
          levelKey: mapping.levelKey,
          priority: mapping.priority,
          scale: rawNumber(mapping.scale),
        })),
    ),
    workloadPolicies: retainField(
      value.workloadPolicies,
      previous?.workloadPolicies,
      old?.workloadPolicies,
      () => Object.values(value.workloadPolicies).map(workloadDraft),
    ),
  };
}

export function emptyWorkloadPolicyDraft(): ScorePolicyWorkloadPolicyDraft {
  return {
    key: rowKey("policy"),
    id: newUuidV7(),
    bucketId: "",
    calendarId: "",
    membership: "",
    peerGroup: { people: { kind: "selected", personIds: [] }, teamIds: [] },
    targetMode: null,
    penalty: null,
  };
}
export function emptyExplicitTargetDraft(): ScorePolicyExplicitTargetDraft {
  return { key: rowKey("target"), personId: "", target: "" };
}
export function emptyBreakpointDraft(): ScorePolicyBreakpointDraft {
  return { key: rowKey("breakpoint"), fromDeviation: "", slope: "" };
}
export function createInitialLevelDraft(): ScorePolicyLevelDraft {
  return { key: rowKey("level"), levelKey: "", label: "" };
}

function peopleValue(value: ScorePolicyPeopleDraft): WorkforcePersonSelection {
  if (value.kind === "all") return { kind: "all" };
  if (value.kind === "selected") return { kind: "selected", personIds: value.personIds };
  return { kind: "filter", allTags: splitTags(value.allTags), anyTags: splitTags(value.anyTags) };
}
function peerValue(value: ScorePolicyPeerGroupDraft): WorkforcePeerGroup {
  const people = peopleValue(value.people);
  return { people, ...(value.teamIds.length === 0 ? {} : { teamIds: value.teamIds }) };
}
function targetValue(
  value: ScorePolicyTargetModeDraft | null,
  path: string,
  errors: Record<string, string>,
): ScorePolicyTargetModeValue | null {
  if (value === null) {
    errors[path] = "Choose an explicit target mode.";
    return null;
  }
  if (value.kind !== "explicit") return { kind: value.kind };
  const targets: WorkforcePersonTarget[] = [];
  value.targets.forEach((target, index) => {
    const targetValue = parseWhole(
      target.target,
      `${path}.targets.${String(index)}.target`,
      errors,
      MAX_UINT32,
    );
    if (target.personId === "")
      errors[`${path}.targets.${String(index)}.personId`] = "Choose a person.";
    if (targetValue !== null && target.personId !== "")
      targets.push({ personId: target.personId, target: targetValue });
  });
  return { kind: "explicit", targets };
}
function penaltyValue(
  value: ScorePolicyPenaltyDraft | null,
  path: string,
  errors: Record<string, string>,
): WorkforceDeviationPenalty | null {
  if (value === null) {
    errors[path] = "Choose a penalty mode.";
    return null;
  }
  if (value.kind === "absolute") return { kind: "absolute" };
  const breakpoints = value.breakpoints.flatMap((breakpoint, index) => {
    const fromDeviation = parseWhole(
      breakpoint.fromDeviation,
      `${path}.breakpoints.${String(index)}.fromDeviation`,
      errors,
      MAX_UINT32,
    );
    const slope = parseWhole(
      breakpoint.slope,
      `${path}.breakpoints.${String(index)}.slope`,
      errors,
      MAX_WEIGHT,
    );
    return fromDeviation === null || slope === null ? [] : [{ fromDeviation, slope }];
  });
  return { kind: "piecewise", breakpoints };
}

export function scorePolicyDraftValue(id: string, draft: ScorePolicyDraft): ScorePolicyDraftResult {
  const errors: Record<string, string> = {};
  if (draft.id !== id) errors.id = "The policy identity changed while editing.";
  if (draft.profileKey.length === 0) errors.profileKey = "Enter a profile key.";
  const levels: WorkforceScoreLevel[] = draft.levels.map((level, index) => {
    if (level.levelKey.length === 0)
      errors[`levels.${String(index)}.levelKey`] = "Enter a level key.";
    if (level.label.length === 0) errors[`levels.${String(index)}.label`] = "Enter a level label.";
    return { levelKey: level.levelKey, label: level.label };
  });
  if (levels.length < 1 || levels.length > 15)
    errors.levels = "Use between 1 and 15 ordered levels.";
  const priorityMapping: WorkforcePriorityMapping[] = [];
  draft.priorityMapping.forEach((mapping, index) => {
    if (mapping.levelKey.length === 0)
      errors[`priorityMapping.${String(index)}.levelKey`] = "Choose a level.";
    const scale = parseWhole(
      mapping.scale,
      `priorityMapping.${String(index)}.scale`,
      errors,
      MAX_WEIGHT,
      1,
    );
    if (scale !== null)
      priorityMapping.push({ levelKey: mapping.levelKey, priority: mapping.priority, scale });
  });
  if (priorityMapping.length !== 4)
    errors.priorityMapping = "Provide exactly four priority mappings.";
  const workloadPolicies: Record<string, ScorePolicyWorkloadPolicyValue> = {};
  const ids = new Set<string>();
  draft.workloadPolicies.forEach((policy, index) => {
    const path = `workloadPolicies.${String(index)}`;
    if (policy.id.length === 0)
      errors[`${path}.id`] = "The workload policy needs a stable identity.";
    if (ids.has(policy.id)) errors[`${path}.id`] = "Workload policy identities must be unique.";
    ids.add(policy.id);
    if (policy.bucketId.length === 0) errors[`${path}.bucketId`] = "Choose a workload bucket.";
    if (policy.calendarId.length === 0) errors[`${path}.calendarId`] = "Choose a calendar.";
    if (!isMembership(policy.membership))
      errors[`${path}.membership`] = "Choose a window membership.";
    const peerGroup = peerValue(policy.peerGroup);
    const targetMode = targetValue(policy.targetMode, `${path}.targetMode`, errors);
    const penalty = penaltyValue(policy.penalty, `${path}.penalty`, errors);
    if (
      policy.id.length !== 0 &&
      policy.bucketId.length !== 0 &&
      policy.calendarId.length !== 0 &&
      isMembership(policy.membership) &&
      targetMode !== null &&
      penalty !== null
    ) {
      workloadPolicies[policy.id] = {
        id: policy.id,
        bucketId: policy.bucketId,
        calendarId: policy.calendarId,
        membership: policy.membership,
        peerGroup,
        targetMode,
        penalty,
      };
    }
  });
  if (Object.keys(errors).length !== 0) return { value: null, errors };
  return {
    value: {
      id,
      kind: "scorePolicy",
      profileKey: draft.profileKey,
      tieBreak: "stableAssignmentRank",
      levels,
      priorityMapping,
      workloadPolicies,
    },
    errors,
  };
}
