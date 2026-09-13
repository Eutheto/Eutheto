export interface EntityRecord {
  readonly id: string;
  readonly kind: string;
}

/** Include variant-specific fields when a controller edits a discriminated union. */
export type EntityField<T extends EntityRecord> = T extends EntityRecord ? keyof T : never;

/** Arrays and nested objects are whole fields, not an implicit element merge. */
export interface EntityRebase<T extends EntityRecord> {
  /** Original typed meaning of the raw buffer, retained through partial choices. */
  readonly local: T;
  readonly current: T;
  readonly value: T;
  readonly conflicts: readonly EntityField<T>[];
}

export function sameField(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (typeof left !== "object" || left === null || typeof right !== "object" || right === null)
    return false;
  if (Array.isArray(left))
    return (
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((value: unknown, index: number) => sameField(value, right[index]))
    );
  if (Array.isArray(right)) return false;
  const a = left as Readonly<Record<string, unknown>>;
  const b = right as Readonly<Record<string, unknown>>;
  const keys = Object.keys(a);
  return (
    keys.length === Object.keys(b).length &&
    keys.every((key) => Object.hasOwn(b, key) && sameField(a[key], b[key]))
  );
}

function copyField<T extends EntityRecord>(target: T, source: T, field: EntityField<T>): void {
  const key = field as keyof T;
  if (Object.hasOwn(source, key)) target[key] = source[key];
  else Reflect.deleteProperty(target, key);
}

/** Called only for a typed candidate after preserving/repairing any invalid raw input. */
export function rebaseEntityDraft<T extends EntityRecord>(
  base: T,
  local: T,
  current: T,
  previous: EntityRebase<T> | null = null,
): EntityRebase<T> {
  if (
    base.id !== local.id ||
    base.id !== current.id ||
    base.kind !== local.kind ||
    base.kind !== current.kind ||
    (previous !== null && (previous.current.id !== base.id || previous.current.kind !== base.kind))
  )
    throw new Error("A draft cannot be rebased onto a different record identity.");
  const baseline = previous?.current ?? base;
  const candidate = previous?.value ?? local;
  const value = { ...current };
  const conflicts: EntityField<T>[] = [];
  const fields = Object.keys({ ...baseline, ...candidate, ...current }) as EntityField<T>[];
  for (const field of fields) {
    const key = field as keyof T;
    if (field === "id" || field === "kind" || sameField(candidate[key], baseline[key])) continue;
    if (
      !sameField(current[key], candidate[key]) &&
      (!sameField(current[key], baseline[key]) || previous?.conflicts.includes(field) === true)
    )
      conflicts.push(field);
    copyField(value, candidate, field);
  }
  return { local: previous?.local ?? local, current, value, conflicts };
}

export function resolveEntityDraftField<T extends EntityRecord>(
  rebase: EntityRebase<T>,
  field: EntityField<T>,
  choice: "current" | "draft",
): EntityRebase<T> {
  if (!rebase.conflicts.includes(field)) return rebase;
  const value = { ...rebase.value };
  if (choice === "current") copyField(value, rebase.current, field);
  return {
    local: rebase.local,
    current: rebase.current,
    value,
    conflicts: rebase.conflicts.filter((item) => item !== field),
  };
}
