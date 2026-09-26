import type {
  WorkforceQualificationMinimum,
  WorkforceRequiredSkillMixRule,
  WorkforceRule,
  WorkforceScope,
} from "./api/generated-domain-pack-contracts";
import type { DurationDraft } from "./components/planner/field-contracts";
import { parseDurationDraft } from "./components/planner/duration-field";
import { sameField } from "./entity-draft";
import { messages } from "./messages";

export type QualificationMinimumDraft = {
  readonly key: string;
  readonly minimum: string;
  readonly allQualificationIds: readonly string[];
  readonly anyQualificationIds: readonly string[];
};

export type RequiredSkillMixMinimumDraft = {
  readonly raw: string;
} & (
  | { readonly status: "empty" }
  | { readonly status: "invalid"; readonly error: "syntax" | "range" }
  | { readonly status: "valid"; readonly minimum: number }
);

const requiredSkillMixMinimumLimit = 65_535n;
let qualificationMinimumSequence = 0;
function qualificationMinimumKey(index: number): string {
  return `qualification-minimum-${String(index)}`;
}
function qualificationMinimumNewKey(): string {
  qualificationMinimumSequence += 1;
  return `qualification-minimum-rebased-${String(qualificationMinimumSequence)}`;
}

/** Keep the editable text separate from its native u16 meaning. */
export function parseRequiredSkillMixMinimumDraft(raw: string): RequiredSkillMixMinimumDraft {
  if (raw === "") return { raw, status: "empty" };
  if (raw.length > 32) return { raw, status: "invalid", error: "range" };
  if (!/^[0-9]+$/u.test(raw)) return { raw, status: "invalid", error: "syntax" };
  const normalized = raw.replace(/^0+/u, "") || "0";
  if (normalized.length > 5) return { raw, status: "invalid", error: "range" };
  const minimum = BigInt(normalized);
  if (minimum > requiredSkillMixMinimumLimit) return { raw, status: "invalid", error: "range" };
  return { raw, status: "valid", minimum: Number(minimum) };
}

export type EditableRule = Extract<
  WorkforceRule,
  {
    readonly kind:
      | "eligibility"
      | "availability"
      | "coverage"
      | "noOverlap"
      | "minimumRest"
      | "maximumAssignmentCount"
      | "requiredSkillMix"
      | "fixedAssignment";
  }
>;
export type EditableRuleKind = EditableRule["kind"];

export type MaximumAssignmentCountDraft = {
  readonly raw: string;
} & (
  | { readonly status: "empty" }
  | { readonly status: "invalid"; readonly error: "syntax" | "range" }
  | { readonly status: "valid"; readonly maximum: number }
);

const maximumAssignmentCountLimit = 4_294_967_295n;

/** Keep the editable text separate from its native u32 meaning. */
export function parseMaximumAssignmentCountDraft(raw: string): MaximumAssignmentCountDraft {
  if (raw === "") return { raw, status: "empty" };
  if (raw.length > 32) return { raw, status: "invalid", error: "range" };
  if (!/^[0-9]+$/u.test(raw)) return { raw, status: "invalid", error: "syntax" };
  const normalized = raw.replace(/^0+/u, "") || "0";
  if (normalized.length > 10) return { raw, status: "invalid", error: "range" };
  const maximum = BigInt(normalized);
  if (maximum > maximumAssignmentCountLimit) return { raw, status: "invalid", error: "range" };
  return { raw, status: "valid", maximum: Number(maximum) };
}

export interface RuleDraft {
  readonly active: boolean;
  readonly scope: WorkforceScope;
  readonly beforeScope: WorkforceScope;
  readonly afterScope: WorkforceScope;
  readonly calendarId: string;
  readonly maximum: MaximumAssignmentCountDraft;
  readonly minimumRest: DurationDraft;
  readonly compatibleCategoryPairs: readonly {
    readonly firstCategory: string;
    readonly secondCategory: string;
  }[];
  readonly personId: string;
  readonly shiftId: string;
  readonly qualificationMinimums: readonly QualificationMinimumDraft[];
}

export function isEditableRule(rule: WorkforceRule): rule is EditableRule {
  return (
    rule.kind === "eligibility" ||
    rule.kind === "availability" ||
    rule.kind === "coverage" ||
    rule.kind === "noOverlap" ||
    rule.kind === "minimumRest" ||
    rule.kind === "maximumAssignmentCount" ||
    rule.kind === "requiredSkillMix" ||
    rule.kind === "fixedAssignment"
  );
}

function qualificationIdentity(
  allQualificationIds: readonly string[],
  anyQualificationIds: readonly string[],
): string {
  return JSON.stringify([[...allQualificationIds].sort(), [...anyQualificationIds].sort()]);
}

function nativeQualificationIdentity(value: WorkforceQualificationMinimum): string {
  return qualificationIdentity(
    value.qualifications.allQualificationIds,
    value.qualifications.anyQualificationIds,
  );
}

function draftQualificationIdentity(value: QualificationMinimumDraft): string {
  return qualificationIdentity(value.allQualificationIds, value.anyQualificationIds);
}

function requiredSkillMixDraft(
  value: WorkforceRequiredSkillMixRule,
  before: WorkforceRequiredSkillMixRule | undefined,
  raw: readonly QualificationMinimumDraft[] | undefined,
): readonly QualificationMinimumDraft[] {
  const values = value.qualificationMinimums;
  if (raw !== undefined && sameField(values, before?.qualificationMinimums)) return raw;
  if (before === undefined || raw === undefined || before.qualificationMinimums.length === 0)
    return values.map((row, index) => ({
      key: qualificationMinimumKey(index),
      minimum: String(row.minimum),
      allQualificationIds: row.qualifications.allQualificationIds,
      anyQualificationIds: row.qualifications.anyQualificationIds,
    }));

  const matches = new Map<string, { indices: number[]; next: number }>();
  before.qualificationMinimums.forEach((row, index) => {
    const key = nativeQualificationIdentity(row);
    const match = matches.get(key);
    if (match) match.indices.push(index);
    else matches.set(key, { indices: [index], next: 0 });
  });
  const sameLength =
    values.length === before.qualificationMinimums.length && values.length === raw.length;
  return values.map((row, index) => {
    const match = matches.get(nativeQualificationIdentity(row));
    const beforeIndex = match?.indices[match.next++];
    const beforeRow =
      beforeIndex === undefined
        ? sameLength
          ? before.qualificationMinimums[index]
          : undefined
        : before.qualificationMinimums[beforeIndex];
    const retained =
      beforeIndex === undefined ? (sameLength ? raw[index] : undefined) : raw[beforeIndex];
    const parsed =
      retained === undefined ? undefined : parseRequiredSkillMixMinimumDraft(retained.minimum);
    const minimum =
      retained !== undefined && (parsed?.status !== "valid" || beforeRow?.minimum === row.minimum)
        ? retained.minimum
        : String(row.minimum);
    return {
      key: retained?.key ?? qualificationMinimumNewKey(),
      minimum,
      allQualificationIds: row.qualifications.allQualificationIds,
      anyQualificationIds: row.qualifications.anyQualificationIds,
    };
  });
}

function qualificationMinimumsForRebase(
  base: WorkforceRequiredSkillMixRule,
  raw: readonly QualificationMinimumDraft[],
): readonly WorkforceQualificationMinimum[] {
  const used = new Set<number>();
  const matches = new Map<string, { indices: number[]; next: number }>();
  base.qualificationMinimums.forEach((value, index) => {
    const key = nativeQualificationIdentity(value);
    const match = matches.get(key);
    if (match) match.indices.push(index);
    else matches.set(key, { indices: [index], next: 0 });
  });
  return raw.map((row, index) => {
    const match = matches.get(draftQualificationIdentity(row));
    let baseIndex = -1;
    if (match) {
      while (match.next < match.indices.length) {
        const candidate = match.indices[match.next];
        if (candidate === undefined) break;
        match.next += 1;
        if (!used.has(candidate)) {
          baseIndex = candidate;
          break;
        }
      }
    }
    if (baseIndex < 0 && index < base.qualificationMinimums.length && !used.has(index))
      baseIndex = index;
    if (baseIndex >= 0) used.add(baseIndex);
    const baseRow = baseIndex >= 0 ? base.qualificationMinimums[baseIndex] : undefined;
    const parsed = parseRequiredSkillMixMinimumDraft(row.minimum);
    return {
      minimum: parsed.status === "valid" ? parsed.minimum : (baseRow?.minimum ?? 0),
      qualifications: {
        allQualificationIds: row.allQualificationIds,
        anyQualificationIds: row.anyQualificationIds,
      },
    };
  });
}

/** Unsubmitted inactive fields remain raw intent, never extra fields in the native rule. */
export function createRuleDraft(
  value: EditableRule | "new",
  retained?: { readonly value: EditableRule; readonly raw: RuleDraft },
): RuleDraft {
  const record = value === "new" ? null : value;
  const previous =
    record !== null && retained?.value.id === record.id && retained.value.kind === record.kind
      ? retained
      : undefined;
  const keepDuration =
    record?.kind === "minimumRest" &&
    previous?.value.kind === "minimumRest" &&
    (previous.raw.minimumRest.status !== "valid" ||
      sameField(record.minimumMinutes, previous.value.minimumMinutes));
  const keepMaximum =
    record?.kind === "maximumAssignmentCount" &&
    previous?.value.kind === "maximumAssignmentCount" &&
    (previous.raw.maximum.status !== "valid" || sameField(record.maximum, previous.value.maximum));
  const previousSkillMix =
    record?.kind === "requiredSkillMix" && previous?.value.kind === "requiredSkillMix"
      ? previous.value
      : undefined;
  return {
    active: record?.active ?? true,
    scope: record?.scope ?? { people: { kind: "all" } },
    calendarId:
      record?.kind === "maximumAssignmentCount"
        ? record.calendarId
        : (previous?.raw.calendarId ?? ""),
    maximum: keepMaximum
      ? previous.raw.maximum
      : record?.kind === "maximumAssignmentCount"
        ? parseMaximumAssignmentCountDraft(String(record.maximum))
        : (previous?.raw.maximum ?? parseMaximumAssignmentCountDraft("")),
    beforeScope:
      record?.kind === "minimumRest"
        ? record.beforeScope
        : (previous?.raw.beforeScope ?? { people: { kind: "all" } }),
    afterScope:
      record?.kind === "minimumRest"
        ? record.afterScope
        : (previous?.raw.afterScope ?? { people: { kind: "all" } }),
    minimumRest: keepDuration
      ? previous.raw.minimumRest
      : record?.kind === "minimumRest"
        ? parseDurationDraft(String(record.minimumMinutes), "minutes", 0)
        : (previous?.raw.minimumRest ?? parseDurationDraft("", "hours", 0)),
    compatibleCategoryPairs:
      record?.kind === "noOverlap"
        ? record.compatibleCategoryPairs
        : (previous?.raw.compatibleCategoryPairs ?? []),
    personId:
      record?.kind === "fixedAssignment"
        ? previous?.value.kind === "fixedAssignment" &&
          (previous.raw.personId === "" || sameField(record.personId, previous.value.personId))
          ? previous.raw.personId
          : record.personId
        : (previous?.raw.personId ?? ""),
    shiftId:
      record?.kind === "fixedAssignment"
        ? previous?.value.kind === "fixedAssignment" &&
          (previous.raw.shiftId === "" || sameField(record.shiftId, previous.value.shiftId))
          ? previous.raw.shiftId
          : record.shiftId
        : (previous?.raw.shiftId ?? ""),
    qualificationMinimums:
      record?.kind === "requiredSkillMix"
        ? requiredSkillMixDraft(record, previousSkillMix, previous?.raw.qualificationMinimums)
        : (previous?.raw.qualificationMinimums ?? []),
  };
}

/** Rust validates scope membership, references, category tokens and rule semantics. */
export function ruleDraftValue(
  id: string,
  kind: EditableRuleKind,
  draft: RuleDraft,
): { readonly value: EditableRule | null; readonly errors: Readonly<Record<string, string>> } {
  const common = { id, active: draft.active, strength: "required" as const, scope: draft.scope };
  switch (kind) {
    case "fixedAssignment": {
      const errors: Record<string, string> = {};
      if (draft.active && draft.personId === "")
        errors.personId = messages.ruleSetup.fixedAssignmentPersonRequired;
      if (draft.active && draft.shiftId === "")
        errors.shiftId = messages.ruleSetup.fixedAssignmentShiftRequired;
      if (Object.keys(errors).length > 0) return { value: null, errors };
      return {
        value: { ...common, kind, personId: draft.personId, shiftId: draft.shiftId },
        errors: {},
      };
    }
    case "maximumAssignmentCount": {
      const maximum = parseMaximumAssignmentCountDraft(draft.maximum.raw);
      const errors: Record<string, string> = {};
      if (draft.calendarId === "") errors.calendarId = messages.ruleSetup.calendarRequired;
      if (maximum.status !== "valid") errors.maximum = messages.ruleSetup.maximumInvalid;
      if (maximum.status !== "valid" || draft.calendarId === "") return { value: null, errors };
      return {
        value: {
          ...common,
          kind,
          calendarId: draft.calendarId,
          maximum: maximum.maximum,
        },
        errors: {},
      };
    }
    case "requiredSkillMix": {
      const errors: Record<string, string> = {};
      const qualificationMinimums: WorkforceQualificationMinimum[] =
        draft.qualificationMinimums.map((row, index) => {
          const minimum = parseRequiredSkillMixMinimumDraft(row.minimum);
          if (minimum.status !== "valid")
            errors[`qualificationMinimums.${String(index)}.minimum`] =
              messages.ruleSetup.qualificationMinimumInvalid;
          return {
            minimum: minimum.status === "valid" ? minimum.minimum : 0,
            qualifications: {
              allQualificationIds: row.allQualificationIds,
              anyQualificationIds: row.anyQualificationIds,
            },
          };
        });
      if (Object.keys(errors).length > 0) return { value: null, errors };
      return {
        value: { ...common, kind, qualificationMinimums },
        errors: {},
      };
    }
    case "minimumRest": {
      if (draft.minimumRest.status !== "valid") {
        return { value: null, errors: { minimumMinutes: "Enter an exact whole-minute duration." } };
      }
      return {
        value: {
          ...common,
          kind,
          minimumMinutes: draft.minimumRest.minutes,
          beforeScope: draft.beforeScope,
          afterScope: draft.afterScope,
        },
        errors: {},
      };
    }
    case "noOverlap":
      return {
        value: { ...common, kind, compatibleCategoryPairs: draft.compatibleCategoryPairs },
        errors: {},
      };
    default:
      return { value: { ...common, kind }, errors: {} };
  }
}

/** Rebase incomplete input against its saved meaning; never use this to submit a command. */
export function ruleRebaseValue(base: EditableRule, draft: RuleDraft): EditableRule | null {
  const parsed = ruleDraftValue(base.id, base.kind, draft).value;
  if (parsed !== null) return parsed;
  if (base.kind === "fixedAssignment")
    return {
      ...base,
      active: draft.active,
      scope: draft.scope,
      personId: draft.personId || base.personId,
      shiftId: draft.shiftId || base.shiftId,
    };
  if (base.kind === "maximumAssignmentCount")
    return {
      ...base,
      active: draft.active,
      scope: draft.scope,
      calendarId: draft.calendarId,
      maximum: draft.maximum.status === "valid" ? draft.maximum.maximum : base.maximum,
    };
  if (base.kind === "requiredSkillMix")
    return {
      ...base,
      active: draft.active,
      scope: draft.scope,
      qualificationMinimums: qualificationMinimumsForRebase(base, draft.qualificationMinimums),
    };
  if (base.kind !== "minimumRest") return null;
  return {
    ...base,
    active: draft.active,
    scope: draft.scope,
    beforeScope: draft.beforeScope,
    afterScope: draft.afterScope,
  };
}
