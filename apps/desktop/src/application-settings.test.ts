import { nextTick, reactive } from "vue";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import * as api from "./api/generated";
import type { ApplicationSettingsSnapshotV1 } from "./api/generated";
import {
  acceptSettingDraftCommit,
  createApplicationSettingsController,
  createSettingDraft,
  isSettingDraftDirty,
  observeSettingDraft,
  reloadSettingDraft,
} from "./application-settings";
import type { ProjectHomeController } from "./project-home";
import { response } from "./testing/project-home";

const initial: ApplicationSettingsSnapshotV1 = {
  schemaVersion: 1,
  libraryRevision: 7,
  settings: {
    appearance: { value: { theme: "light" }, updatedAt: "2026-09-01T12:00:00Z" },
    locale: { value: "en-US", updatedAt: "2026-09-01T12:00:00Z" },
    units: null,
  },
};
const committed: ApplicationSettingsSnapshotV1 = {
  ...initial,
  libraryRevision: 8,
  settings: {
    ...initial.settings,
    appearance: { value: { theme: "dark" }, updatedAt: "2026-09-02T12:00:00Z" },
  },
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function createCache() {
  // Only the epoch belongs to this controller's home dependency; native reads remain mocked at API.
  const home = { state: reactive({ libraryEpoch: 0 }) } as ProjectHomeController;
  const cache = createApplicationSettingsController(home);
  onTestFinished(() => {
    cache.dispose();
  });
  return { home, cache };
}

afterEach(() => vi.restoreAllMocks());

describe("monotonic settings presentation", () => {
  it("does not let an old read failure hide a newer exact commit", async () => {
    const read = deferred<api.ApiResponseDto<ApplicationSettingsSnapshotV1>>();
    vi.spyOn(api, "getApplicationSettings").mockReturnValue(read.promise);
    const { cache } = createCache();
    cache.acceptSnapshot(initial);
    const pending = cache.load();
    cache.acceptSnapshot(committed);
    read.reject(new Error("late read failure"));
    await pending;
    expect(cache.state.phase).toBe("ready");
    expect(cache.state.errorMessage).toBeNull();
    expect(cache.preferences.theme).toBe("dark");
    expect(cache.state.snapshot?.libraryRevision).toBe(8);
  });

  it("hands off unaffected dirty drafts when its own notification arrives before its receipt", async () => {
    vi.spyOn(api, "getApplicationSettings").mockResolvedValue(response(committed));
    const { home, cache } = createCache();
    cache.acceptSnapshot(initial);
    const appearance = createSettingDraft("appearance");
    const locale = createSettingDraft("locale");
    reloadSettingDraft(appearance, initial);
    reloadSettingDraft(locale, initial);
    appearance.value = { ...appearance.value, theme: "dark" };
    locale.value = "fr-CA";
    // While Save is pending the page defers draft observation, but root reads continue.
    home.state.libraryEpoch += 1;
    await nextTick();
    await nextTick();
    expect(cache.preferences.theme).toBe("dark");
    const accepted = cache.acceptSnapshot(committed);
    acceptSettingDraftCommit(appearance, "appearance", initial, committed, accepted);
    acceptSettingDraftCommit(locale, "appearance", initial, committed, accepted);
    const observed = cache.state.snapshot;
    if (observed === null) throw new Error("Expected the accepted settings snapshot");
    observeSettingDraft(locale, observed);
    expect(isSettingDraftDirty(appearance)).toBe(false);
    expect(locale.value).toBe("fr-CA");
    expect(isSettingDraftDirty(locale)).toBe(true);
    expect(locale.base?.libraryRevision).toBe(8);
    expect(locale.stale).toBe(false);
  });

  it("preserves draft text and bases when R+2 is observed before an R+1 write receipt", () => {
    const { cache } = createCache();
    cache.acceptSnapshot(initial);
    const appearance = createSettingDraft("appearance");
    const locale = createSettingDraft("locale");
    reloadSettingDraft(appearance, initial);
    reloadSettingDraft(locale, initial);
    appearance.value = { ...appearance.value, theme: "dark" };
    locale.value = "fr-CA";
    const newer: ApplicationSettingsSnapshotV1 = {
      ...committed,
      libraryRevision: 9,
      settings: {
        ...committed.settings,
        appearance: { value: { theme: "system" }, updatedAt: "2026-09-03T12:00:00Z" },
      },
    };
    cache.acceptSnapshot(newer);
    const accepted = cache.acceptSnapshot(committed);
    acceptSettingDraftCommit(appearance, "appearance", initial, committed, accepted);
    acceptSettingDraftCommit(locale, "appearance", initial, committed, accepted);
    observeSettingDraft(appearance, newer);
    observeSettingDraft(locale, newer);
    expect(cache.preferences.theme).toBe("system");
    expect(cache.state.snapshot?.libraryRevision).toBe(9);
    expect(appearance.value.theme).toBe("dark");
    expect(locale.value).toBe("fr-CA");
    expect(appearance.base?.libraryRevision).toBe(7);
    expect(locale.base?.libraryRevision).toBe(7);
    expect(appearance.stale).toBe(true);
    expect(locale.stale).toBe(true);
  });

  it("requires prior-entry evidence including timestamp before handing off another draft", () => {
    const locale = createSettingDraft("locale");
    reloadSettingDraft(locale, initial);
    locale.value = "fr-CA";
    const differentPredecessor: ApplicationSettingsSnapshotV1 = {
      ...initial,
      settings: {
        ...initial.settings,
        locale: { value: "en-US", updatedAt: "2026-09-01T13:00:00Z" },
      },
    };
    acceptSettingDraftCommit(
      locale,
      "appearance",
      differentPredecessor,
      {
        ...committed,
        settings: { ...committed.settings, locale: differentPredecessor.settings.locale },
      },
      true,
    );
    expect(locale.base).toBe(initial);
    expect(locale.value).toBe("fr-CA");
    expect(locale.stale).toBe(true);
  });

  it("never rebases a dirty draft from an external or import refresh, even if the value matches", () => {
    const locale = createSettingDraft("locale");
    reloadSettingDraft(locale, initial);
    locale.value = "fr-CA";
    const imported: ApplicationSettingsSnapshotV1 = {
      ...committed,
      settings: {
        ...committed.settings,
        locale: { value: "fr-CA", updatedAt: "2026-09-02T12:00:00Z" },
      },
    };
    observeSettingDraft(locale, imported);
    expect(locale.base).toBe(initial);
    expect(locale.value).toBe("fr-CA");
    expect(locale.stale).toBe(true);
    reloadSettingDraft(locale, imported);
    expect(locale.stale).toBe(false);
    expect(isSettingDraftDirty(locale)).toBe(false);
  });

  it("retains last known settings on refresh failure and ignores late reads after disposal", async () => {
    const read = deferred<api.ApiResponseDto<ApplicationSettingsSnapshotV1>>();
    vi.spyOn(api, "getApplicationSettings")
      .mockRejectedValueOnce(new Error("unavailable"))
      .mockReturnValueOnce(read.promise);
    const { cache } = createCache();
    cache.acceptSnapshot(initial);
    await cache.load();
    expect(cache.state.phase).toBe("error");
    expect(cache.preferences.theme).toBe("light");
    const pending = cache.load();
    cache.dispose();
    read.resolve(response(committed));
    await pending;
    expect(cache.state.snapshot?.libraryRevision).toBe(7);
  });
});
