import type { WorkforceRule, WorkforceScope } from "./api/generated-domain-pack-contracts";
import type { DurationDraft } from "./components/planner/field-contracts";
import { parseDurationDraft } from "./components/planner/duration-field";
import { sameField } from "./entity-draft";
import { messages } from "./messages";

export type EditableRule = Extract<
  WorkforceRule,
  {
    readonly kind:
      | "eligibility"
      | "availability"
      | "coverage"
      | "noOverlap"
      | "minimumRest"
      | "maximumAssignmentCount";
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
}

export function isEditableRule(rule: WorkforceRule): rule is EditableRule {
  return (
    rule.kind === "eligibility" ||
    rule.kind === "availability" ||
    rule.kind === "coverage" ||
    rule.kind === "noOverlap" ||
    rule.kind === "minimumRest" ||
    rule.kind === "maximumAssignmentCount"
  );
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
  if (base.kind === "maximumAssignmentCount")
    return {
      ...base,
      active: draft.active,
      scope: draft.scope,
      calendarId: draft.calendarId,
      maximum: draft.maximum.status === "valid" ? draft.maximum.maximum : base.maximum,
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
