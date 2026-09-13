import type { DomainEntityRef } from "../../api/generated";
import type {
  WorkforceScope,
  WorkforceSetupScopeAxis,
  WorkforceSetupScopeInspection,
  WorkforceSetupScopePopulation,
  WorkforceWeekday,
} from "../../api/generated-domain-pack-contracts";
import type { LabeledEntityRef } from "../explanations/types";
import type { EntityPickerPage, PlannerFieldProps } from "./field-contracts";

export type ScopeEntityField = "people" | "teamIds" | "assignmentTypeIds" | "locationIds";
export type ScopeOptionalField = Exclude<keyof WorkforceScope, "people">;
export type ScopeTextField = "allTags" | "anyTags" | "categories";
export type ScopeFieldName = ScopeEntityField | ScopeTextField | "weekdays";

/** Every entity field owns a separate query page and retained selection metadata. */
export interface ScopeEntityOptions {
  readonly requestKey: string;
  readonly query: string;
  readonly page: EntityPickerPage;
  readonly selectedOptions: readonly LabeledEntityRef[];
}

export type ScopePreviewState =
  | { readonly status: "idle" }
  | { readonly status: "loading"; readonly requestKey: string }
  | { readonly status: "error"; readonly requestKey: string; readonly message: string }
  | {
      readonly status: "ready";
      readonly requestKey: string;
      readonly inspection: WorkforceSetupScopeInspection;
    };

type ScopeContinuation = NonNullable<WorkforceSetupScopePopulation["page"]["continuation"]>;
export type ScopePreviewIntent = {
  /** The key being invalidated, not the key for the next invocation. */
  readonly requestKey: string;
} & (
  | { readonly kind: "preview" | "retry" | "firstPage"; readonly axis: WorkforceSetupScopeAxis }
  | { readonly kind: "axis"; readonly axis: WorkforceSetupScopeAxis }
  | {
      readonly kind: "nextPage";
      readonly axis: WorkforceSetupScopeAxis;
      readonly continuation: ScopeContinuation;
    }
);

export interface RuleScopeBuilderProps extends PlannerFieldProps {
  readonly modelValue: WorkforceScope;
  readonly entityOptions: Readonly<Record<ScopeEntityField, ScopeEntityOptions>>;
  readonly fieldErrors?: Readonly<Partial<Record<ScopeFieldName, string>>>;
  readonly preview: ScopePreviewState;
  /**
   * Caller rotates for EVERY invocation: scenario/revision, complete command source/draft,
   * rule, part, axis, limit and continuation. This is not a hash or native admission check.
   */
  readonly currentRequestKey: string;
  readonly previewAxis: WorkforceSetupScopeAxis;
  /** The native requested limit, from 1 through 200. Only one current page is retained. */
  readonly previewLimit: number;
}

export interface RuleScopeBuilderEvents {
  "update:modelValue": [scope: WorkforceScope];
  entitySearch: [field: ScopeEntityField, query: string];
  entityNextPage: [field: ScopeEntityField];
  entityRetry: [field: ScopeEntityField];
  preview: [intent: ScopePreviewIntent];
}

export const scopeEntityKinds = {
  people: "person",
  teamIds: "team",
  assignmentTypeIds: "assignmentType",
  locationIds: "location",
} as const;

export const scopeWeekdays: readonly WorkforceWeekday[] = [
  "monday",
  "tuesday",
  "wednesday",
  "thursday",
  "friday",
  "saturday",
  "sunday",
];

export function scopeEntities(
  scope: WorkforceScope,
  field: ScopeEntityField,
): readonly DomainEntityRef[] {
  const ids =
    field === "people"
      ? scope.people.kind === "selected"
        ? scope.people.personIds
        : []
      : (scope[field] ?? []);
  return ids.map((id) => ({ kind: scopeEntityKinds[field], id }));
}

/** Removing an optional filter omits the property; enabling it is explicitly empty. */
export function toggleScopeFilter(
  scope: WorkforceScope,
  field: ScopeOptionalField,
): WorkforceScope {
  if (scope[field] === undefined) return { ...scope, [field]: [] };
  const { [field]: removed, ...next } = scope;
  void removed;
  return next;
}
