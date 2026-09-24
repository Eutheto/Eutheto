import type { WorkforceRule, WorkforceScope } from "./api/generated-domain-pack-contracts";
import type { DurationDraft } from "./components/planner/field-contracts";
import { parseDurationDraft } from "./components/planner/duration-field";
import { sameField } from "./entity-draft";

export type EditableRule = Extract<
  WorkforceRule,
  { readonly kind: "eligibility" | "availability" | "coverage" | "noOverlap" | "minimumRest" }
>;
export type EditableRuleKind = EditableRule["kind"];

export interface RuleDraft {
  readonly active: boolean;
  readonly scope: WorkforceScope;
  readonly beforeScope: WorkforceScope;
  readonly afterScope: WorkforceScope;
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
    rule.kind === "minimumRest"
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
  return {
    active: record?.active ?? true,
    scope: record?.scope ?? { people: { kind: "all" } },
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
  if (base.kind !== "minimumRest") return null;
  return {
    ...base,
    active: draft.active,
    scope: draft.scope,
    beforeScope: draft.beforeScope,
    afterScope: draft.afterScope,
  };
}
