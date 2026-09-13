import { newUuidV7, type PeopleCsvNewPersonDefaults } from "./api/generated";
import type {
  WorkforceQualificationGrant,
  WorkforceWindowMembership,
} from "./api/generated-domain-pack-contracts";
import { messages } from "./messages";

/** Temporary row keys are UI identity; the native grant remains the complete temporal tuple. */
export interface GrantDraft {
  readonly key: string;
  readonly qualificationId: string;
  readonly effectiveFrom: string;
  readonly expiresAt: string;
}
export interface TagDraft {
  readonly key: string;
  readonly text: string;
}
export interface PersonFieldsDraft {
  readonly activeDatesEnabled: boolean;
  readonly startDate: string;
  readonly endDateExclusive: string;
  readonly qualificationGrants: readonly GrantDraft[];
  readonly eligibleAssignmentTypeIds: readonly string[];
  readonly teamIds: readonly string[];
  readonly homeLocationId: string;
  readonly weightNumerator: string;
  readonly weightDenominator: string;
  readonly targetEnabled: boolean;
  readonly targetBucketId: string;
  readonly targetCalendarId: string;
  readonly targetMembership: WorkforceWindowMembership;
  readonly target: string;
  readonly tags: readonly TagDraft[];
  readonly displayEnabled: boolean;
  readonly color: string;
  readonly avatarInitials: string;
}
export type PersonFieldErrors = Partial<Record<keyof PersonFieldsDraft, string>>;

export function grantDraft(value?: WorkforceQualificationGrant): GrantDraft {
  return {
    key: newUuidV7(),
    qualificationId: value?.qualificationId ?? "",
    effectiveFrom: value?.effectiveFrom ?? "",
    expiresAt: value?.expiresAt ?? "",
  };
}
export function createPersonFieldsDraft(value?: PeopleCsvNewPersonDefaults): PersonFieldsDraft {
  const dates = value?.activeRange.kind === "dateRange" ? value.activeRange : null;
  return {
    activeDatesEnabled: dates !== null,
    startDate: dates?.startDate ?? "",
    endDateExclusive: dates?.endDateExclusive ?? "",
    qualificationGrants: value?.qualificationGrants.map(grantDraft) ?? [],
    eligibleAssignmentTypeIds: value?.eligibleAssignmentTypeIds ?? [],
    teamIds: value?.teamIds ?? [],
    homeLocationId: value?.homeLocationId ?? "",
    weightNumerator: String(value?.workloadWeight.numerator ?? 1),
    weightDenominator: String(value?.workloadWeight.denominator ?? 1),
    targetEnabled: value?.workloadTarget !== undefined,
    targetBucketId: value?.workloadTarget?.bucketId ?? "",
    targetCalendarId: value?.workloadTarget?.calendarId ?? "",
    targetMembership: value?.workloadTarget?.membership ?? "reportingDate",
    target: String(value?.workloadTarget?.target ?? 0),
    tags: value?.tags.map((text) => ({ key: newUuidV7(), text })) ?? [],
    displayEnabled: value?.display !== undefined,
    color: value?.display?.color ?? "",
    avatarInitials: value?.display?.avatarInitials ?? "",
  };
}

function parseNumbers(draft: PersonFieldsDraft): {
  readonly numerator: number;
  readonly denominator: number;
  readonly target: number;
  readonly errors: PersonFieldErrors;
} {
  const errors: PersonFieldErrors = {};
  function integer(
    field: "weightNumerator" | "weightDenominator" | "target",
    min: number,
    max: number,
  ): number {
    const raw = draft[field];
    const number = /^[0-9]+$/u.test(raw) ? Number(raw) : Number.NaN;
    if (!Number.isSafeInteger(number) || number < min || number > max)
      errors[field] = messages.personFields.integer(min, max);
    return number;
  }
  const numerator = integer("weightNumerator", 1, 1_000_000);
  const denominator = integer("weightDenominator", 1, 1_000_000);
  const target = draft.targetEnabled ? integer("target", 0, 4_294_967_295) : 0;
  return { numerator, denominator, target, errors };
}

/** Input feedback must not rebuild potentially 10,000-row native collections on each keystroke. */
export function personFieldErrors(draft: PersonFieldsDraft): PersonFieldErrors {
  return parseNumbers(draft).errors;
}

/** Converts representable integers only. Rust still validates every proposed domain value. */
export function personFieldsValue(draft: PersonFieldsDraft): {
  readonly value: PeopleCsvNewPersonDefaults | null;
  readonly errors: PersonFieldErrors;
} {
  const { numerator, denominator, target, errors } = parseNumbers(draft);
  if (Object.keys(errors).length !== 0) return { value: null, errors };
  return {
    errors,
    value: {
      activeRange: draft.activeDatesEnabled
        ? {
            kind: "dateRange",
            startDate: draft.startDate,
            endDateExclusive: draft.endDateExclusive,
          }
        : { kind: "always" },
      qualificationGrants: draft.qualificationGrants.map((grant) => ({
        qualificationId: grant.qualificationId,
        ...(grant.effectiveFrom === "" ? {} : { effectiveFrom: grant.effectiveFrom }),
        ...(grant.expiresAt === "" ? {} : { expiresAt: grant.expiresAt }),
      })),
      eligibleAssignmentTypeIds: draft.eligibleAssignmentTypeIds,
      teamIds: draft.teamIds,
      ...(draft.homeLocationId === "" ? {} : { homeLocationId: draft.homeLocationId }),
      workloadWeight: { numerator, denominator },
      ...(draft.targetEnabled
        ? {
            workloadTarget: {
              bucketId: draft.targetBucketId,
              calendarId: draft.targetCalendarId,
              membership: draft.targetMembership,
              target,
            },
          }
        : {}),
      tags: draft.tags.map((tag) => tag.text),
      ...(draft.displayEnabled
        ? {
            display: {
              ...(draft.color === "" ? {} : { color: draft.color }),
              ...(draft.avatarInitials === "" ? {} : { avatarInitials: draft.avatarInitials }),
            },
          }
        : {}),
    },
  };
}
