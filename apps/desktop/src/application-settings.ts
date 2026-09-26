import { computed, shallowReactive, watch } from "vue";
import {
  getApplicationSettings,
  type ApplicationSettingKey,
  type ApplicationSettingEntryV1,
  type ApplicationSettingValues,
  type ApplicationSettingsSnapshotV1,
} from "./api/generated";
import { messages } from "./messages";
import { safeMessage, type ProjectHomeController } from "./project-home";

export interface ApplicationSettingsController {
  readonly state: {
    readonly phase: "loading" | "ready" | "error";
    readonly snapshot: ApplicationSettingsSnapshotV1 | null;
    readonly errorMessage: string | null;
  };
  readonly preferences: {
    readonly theme: "system" | "light" | "dark";
    readonly forceReducedMotion: boolean;
    readonly locale: string | undefined;
    readonly localeWarning: string | null;
    readonly units: "metric" | "us-customary";
  };
  load(): Promise<void>;
  acceptSnapshot(snapshot: ApplicationSettingsSnapshotV1): boolean;
  dispose(): void;
}

export function createApplicationSettingsController(
  home: ProjectHomeController,
): ApplicationSettingsController {
  const state = shallowReactive<{
    phase: "loading" | "ready" | "error";
    snapshot: ApplicationSettingsSnapshotV1 | null;
    errorMessage: string | null;
  }>({ phase: "loading", snapshot: null, errorMessage: null });
  let generation = 0;
  let disposed = false;
  const preferences = computed(() => {
    const saved = state.snapshot?.settings;
    const storedLocale = saved?.locale?.value;
    let locale = storedLocale;
    let localeWarning: string | null = null;
    if (storedLocale !== undefined) {
      try {
        if (
          Intl.NumberFormat.supportedLocalesOf([storedLocale]).length === 0 ||
          Intl.DateTimeFormat.supportedLocalesOf([storedLocale]).length === 0
        )
          throw new RangeError();
      } catch {
        locale = undefined;
        localeWarning = messages.settings.unsupportedLocale(storedLocale);
      }
    }
    return {
      theme: saved?.appearance?.value.theme ?? "system",
      forceReducedMotion: saved?.appearance?.value.reducedMotion === true,
      locale,
      localeWarning,
      units: saved?.units?.value ?? "metric",
    };
  });

  function acceptSnapshot(snapshot: ApplicationSettingsSnapshotV1): boolean {
    if (
      disposed ||
      (state.snapshot !== null && snapshot.libraryRevision < state.snapshot.libraryRevision)
    )
      return false;
    // An exact receipt also retires a read whose late failure would hide this success.
    generation += 1;
    state.snapshot = snapshot;
    state.phase = "ready";
    state.errorMessage = null;
    return true;
  }

  async function load(): Promise<void> {
    if (disposed) return;
    const current = ++generation;
    state.phase = "loading";
    state.errorMessage = null;
    try {
      const response = await getApplicationSettings();
      if (current !== generation) return;
      if (!acceptSnapshot(response.result)) state.phase = "ready";
    } catch (error) {
      if (current !== generation) return;
      state.phase = "error";
      state.errorMessage = safeMessage(error);
    }
  }

  const stop = watch(
    () => home.state.libraryEpoch,
    () => void load(),
  );
  return {
    state,
    get preferences() {
      return preferences.value;
    },
    load,
    acceptSnapshot,
    dispose() {
      disposed = true;
      generation += 1;
      stop();
    },
  };
}

// These drafts are created by the page, never retained by the root cache.
export interface SettingDraft<K extends ApplicationSettingKey> {
  readonly key: K;
  base: ApplicationSettingsSnapshotV1 | null;
  value: ApplicationSettingValues[K];
  stale: boolean;
}

function draftValue<K extends ApplicationSettingKey>(
  key: K,
  snapshot: ApplicationSettingsSnapshotV1 | null,
): ApplicationSettingValues[K] {
  const defaults: ApplicationSettingValues = { appearance: {}, locale: "", units: "metric" };
  const value = snapshot?.settings[key]?.value ?? defaults[key];
  return typeof value === "object" ? { ...value } : value;
}

function sameValue(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (typeof left !== "object" || left === null || typeof right !== "object" || right === null)
    return false;
  const a = left as ApplicationSettingValues["appearance"];
  const b = right as ApplicationSettingValues["appearance"];
  return a.theme === b.theme && a.reducedMotion === b.reducedMotion;
}

function sameEntry(
  left: ApplicationSettingEntryV1 | null,
  right: ApplicationSettingEntryV1 | null,
): boolean {
  return (
    left === right ||
    (left !== null &&
      right !== null &&
      left.updatedAt === right.updatedAt &&
      sameValue(left.value, right.value))
  );
}

export function createSettingDraft<K extends ApplicationSettingKey>(key: K): SettingDraft<K> {
  return { key, base: null, value: draftValue(key, null), stale: false };
}

export function isSettingDraftDirty<K extends ApplicationSettingKey>(
  draft: SettingDraft<K>,
): boolean {
  return !sameValue(draft.value, draftValue(draft.key, draft.base));
}

export function reloadSettingDraft<K extends ApplicationSettingKey>(
  draft: SettingDraft<K>,
  snapshot: ApplicationSettingsSnapshotV1,
): void {
  draft.base = snapshot;
  draft.value = draftValue(draft.key, snapshot);
  draft.stale = false;
}

export function observeSettingDraft<K extends ApplicationSettingKey>(
  draft: SettingDraft<K>,
  snapshot: ApplicationSettingsSnapshotV1,
): void {
  if (draft.base?.libraryRevision === snapshot.libraryRevision) return;
  if (draft.stale || isSettingDraftDirty(draft)) draft.stale = true;
  else reloadSettingDraft(draft, snapshot);
}

export function acceptSettingDraftCommit<K extends ApplicationSettingKey>(
  draft: SettingDraft<K>,
  writtenKey: ApplicationSettingKey,
  prior: ApplicationSettingsSnapshotV1,
  receipt: ApplicationSettingsSnapshotV1,
  accepted: boolean,
): void {
  if (!accepted) {
    draft.stale = true;
    return;
  }
  if (draft.key === writtenKey) {
    reloadSettingDraft(draft, receipt);
    return;
  }
  // The atomic receipt proves only this captured predecessor, not an intervening read.
  if (
    draft.base?.libraryRevision === prior.libraryRevision &&
    sameEntry(draft.base.settings[draft.key], prior.settings[draft.key]) &&
    sameEntry(prior.settings[draft.key], receipt.settings[draft.key])
  ) {
    draft.base = receipt;
    draft.stale = false;
  } else {
    draft.stale = true;
  }
}
