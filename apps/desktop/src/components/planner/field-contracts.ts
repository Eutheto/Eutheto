import type { DomainEntityRef } from "../../api/generated";
import type {
  WorkforceLocalInterval,
  WorkforceLocalWindowTiming,
  WorkforcePreferencePriority,
  WorkforceSetupRuleCatalogEntry,
  WorkforceSetupRuleClass,
} from "../../api/generated-domain-pack-contracts";
import type { LabeledEntityRef } from "../explanations/types";

export interface PlannerFieldProps {
  readonly label: string;
  readonly id?: string;
  readonly description?: string;
  readonly error?: string | undefined;
  readonly required?: boolean;
  readonly disabled?: boolean;
  readonly readOnly?: boolean;
  readonly locale?: string | undefined;
}

/** A request key identifies one complete native query invocation, including its page. */
export type EntityPickerPage =
  | { readonly status: "idle" }
  | { readonly status: "loading"; readonly requestKey: string; readonly query: string }
  | {
      readonly status: "error";
      readonly requestKey: string;
      readonly query: string;
      readonly message: string;
    }
  | {
      readonly status: "ready";
      readonly requestKey: string;
      readonly query: string;
      readonly options: readonly LabeledEntityRef[];
      readonly hasMore: boolean;
    };

export interface EntityPickerQueryProps extends PlannerFieldProps {
  /** Rotate synchronously on source, context, query or page changes and on retry. */
  readonly requestKey: string;
  readonly query: string;
  readonly page: EntityPickerPage;
}

export interface EntityPickerProps extends EntityPickerQueryProps {
  readonly modelValue: DomainEntityRef | null;
  /** Retained label metadata is independent of the current search page. */
  readonly selectedOption: LabeledEntityRef | null;
}

export interface EntityMultiPickerProps extends EntityPickerQueryProps {
  readonly modelValue: readonly DomainEntityRef[];
  /** Missing labels render the actual kind and full ID, never a guessed name. */
  readonly selectedOptions: readonly LabeledEntityRef[];
}

export interface EntityPickerEvents {
  "update:modelValue": [value: DomainEntityRef | null];
  search: [query: string];
  nextPage: [];
  retry: [];
}

export interface EntityMultiPickerEvents {
  "update:modelValue": [value: readonly DomainEntityRef[]];
  search: [query: string];
  nextPage: [];
  retry: [];
}

export type DurationUnit = "minutes" | "hours";
export type DurationDraft = {
  readonly raw: string;
  readonly unit: DurationUnit;
} & (
  | { readonly status: "empty" }
  | { readonly status: "invalid"; readonly error: "syntax" | "wholeMinutes" | "range" }
  | { readonly status: "valid"; readonly minutes: number }
);

export interface DurationFieldProps extends PlannerFieldProps {
  readonly modelValue: DurationDraft;
  readonly minimum: 0 | 1;
}

export interface DurationFieldEvents {
  /** Invalid or empty drafts never carry a previous valid quantity. */
  "update:modelValue": [draft: DurationDraft];
}

export interface LocalWindowRawDraft {
  readonly kind: "localWindow";
  readonly startTime: string;
  readonly endTime: string;
  readonly endDayOffset: string;
}

export interface LocalIntervalRawDraft {
  readonly kind: "localInterval";
  readonly startsAt: string;
  readonly endsAt: string;
}

export type TemporalRawDraft = LocalWindowRawDraft | LocalIntervalRawDraft;

/** Readiness admits a native validation request, not a resolved or valid domain interval. */
export type TemporalDraft =
  | { readonly raw: TemporalRawDraft; readonly status: "empty" }
  | {
      readonly raw: TemporalRawDraft;
      readonly status: "incomplete";
      readonly error: "endpoints" | "dayOffset";
    }
  | {
      readonly raw: LocalWindowRawDraft;
      readonly status: "readyForNativeValidation";
      readonly candidate: WorkforceLocalWindowTiming;
    }
  | {
      readonly raw: LocalIntervalRawDraft;
      readonly status: "readyForNativeValidation";
      readonly candidate: WorkforceLocalInterval;
    };

export interface TemporalFeedback {
  readonly requestKey: string;
  readonly errors: readonly {
    readonly field: "start" | "end" | "endDayOffset" | "range";
    readonly message: string;
  }[];
}

export interface DateTimeRangeFieldProps extends PlannerFieldProps {
  readonly modelValue: TemporalDraft;
  readonly timeZone: string;
  /** Also changes when native resolution context or another relevant draft field changes. */
  readonly requestKey: string;
  readonly feedback?: TemporalFeedback;
}

export interface DateTimeRangeFieldEvents {
  "update:modelValue": [draft: TemporalDraft];
}

export interface RuleStrengthControlProps extends PlannerFieldProps {
  readonly modelValue: WorkforceSetupRuleClass;
  readonly priority: WorkforcePreferencePriority | null;
  /** Class selection is for a new intent; existing persisted rule classes remain locked. */
  readonly classEditable: boolean;
  readonly requiredKind: WorkforceSetupRuleCatalogEntry | null;
  readonly preferenceKind: WorkforceSetupRuleCatalogEntry | null;
}

export interface RuleStrengthControlEvents {
  /** Intent only: the consumer constructs the actual supported native rule kind. */
  "update:modelValue": [value: WorkforceSetupRuleClass];
  "update:priority": [value: WorkforcePreferencePriority];
}
