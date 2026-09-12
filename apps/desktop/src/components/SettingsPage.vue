<script setup lang="ts">
import { computed, nextTick, onScopeDispose, reactive, ref, shallowRef, watch } from "vue";
import {
  LibraryOperationScope,
  SettingsImportFlow,
  exportNonsecretSettings,
  resetSettingsSection,
  updateSetting,
  type ApplicationSettingKey,
  type ApplicationSettingsSnapshotV1,
  type SettingsChangeV1,
  type SettingsImportPreviewV1,
  type SetupOperation,
  type ValidationIssue,
} from "../api/generated";
import {
  acceptSettingDraftCommit,
  createSettingDraft,
  isSettingDraftDirty,
  observeSettingDraft,
  reloadSettingDraft,
  type ApplicationSettingsController,
  type SettingDraft,
} from "../application-settings";
import { formatDateTime, formatNumber, formatUnit, messages } from "../messages";
import {
  isOperationCancelled,
  isRevisionConflict,
  safeMessage,
  type ProjectHomeController,
} from "../project-home";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import { Card } from "./ui/card";

const props = defineProps<{
  readonly home: ProjectHomeController;
  readonly settings: ApplicationSettingsController;
}>();
const appearance = reactive(createSettingDraft("appearance"));
const locale = reactive(createSettingDraft("locale"));
const units = reactive(createSettingDraft("units"));
const drafts = [appearance, locale, units];
const pending = ref(false);
const writing = ref(false);
const cleaning = ref(false);
const errorMessage = ref<string | null>(null);
const outcome = ref<string | null>(null);
const review = shallowRef<SettingsImportPreviewV1 | null>(null);
const warnings = shallowRef<readonly ValidationIssue[]>([]);
const reviewHeading = ref<HTMLElement | null>(null);
const importButton = ref<HTMLButtonElement | null>(null);
let flow: SettingsImportFlow | null = null;
let lifetime = 0;
let disposed = false;
const busy = computed(
  () => props.home.state.busyAction !== null || pending.value || cleaning.value,
);
const dirty = computed(() => drafts.some(isSettingDraftDirty) || review.value !== null);
const reviewStale = computed(
  () =>
    review.value !== null &&
    !pending.value &&
    (props.settings.state.snapshot === null ||
      review.value.libraryRevision !== props.settings.state.snapshot.libraryRevision),
);

function observeSnapshot(snapshot: ApplicationSettingsSnapshotV1 | null): void {
  if (snapshot === null || writing.value || disposed) return;
  for (const draft of drafts) observeSettingDraft(draft, snapshot);
}
watch(() => props.settings.state.snapshot, observeSnapshot, { immediate: true, flush: "sync" });

async function retireFlow(): Promise<void> {
  const owner = flow;
  flow = null;
  if (owner === null) return;
  cleaning.value = true;
  try {
    // Root retains failed cleanup, including undelivered creator results.
    await props.home.retireReviewOwner(owner);
  } finally {
    if (!disposed) cleaning.value = false;
  }
}

function discard(): void {
  lifetime += 1;
  pending.value = false;
  writing.value = false;
  review.value = null;
  warnings.value = [];
  errorMessage.value = null;
  outcome.value = null;
  const snapshot = props.settings.state.snapshot;
  if (snapshot !== null) for (const draft of drafts) reloadSettingDraft(draft, snapshot);
  void retireFlow();
}
onScopeDispose(() => {
  disposed = true;
  discard();
});

async function reloadSection(draft: SettingDraft<ApplicationSettingKey>): Promise<void> {
  const snapshot = props.settings.state.snapshot;
  if (busy.value || snapshot === null) return;
  const current = lifetime;
  reloadSettingDraft(draft, snapshot);
  errorMessage.value = null;
  outcome.value = messages.settings.draftReloaded;
  await nextTick();
  if (!disposed && current === lifetime)
    document.getElementById(`settings-${draft.key}-title`)?.focus();
}

async function saveSection(key: ApplicationSettingKey, reset = false): Promise<void> {
  const draft = drafts.find((candidate) => candidate.key === key);
  const prior = draft?.base;
  if (busy.value || !draft || !prior || draft.stale || props.settings.state.phase !== "ready")
    return;
  if (!reset && key === "locale" && locale.value.length === 0) return;
  const current = lifetime;
  pending.value = true;
  writing.value = true;
  errorMessage.value = null;
  outcome.value = null;
  warnings.value = [];
  try {
    await props.home.runOperation({
      action: `settings-${reset ? "reset" : "save"}-${key}`,
      label: reset ? messages.settings.resetting : messages.settings.saving,
      execute: async () => {
        const response = reset
          ? await resetSettingsSection(key, prior.libraryRevision)
          : key === "appearance"
            ? await updateSetting("appearance", { ...appearance.value }, prior.libraryRevision)
            : key === "locale"
              ? await updateSetting("locale", locale.value, prior.libraryRevision)
              : await updateSetting("units", units.value, prior.libraryRevision);
        // Root presentation must receive actual commits even if this page has left.
        const accepted = props.settings.acceptSnapshot(response.result);
        if (!accepted) void props.settings.load();
        if (!disposed && current === lifetime) {
          for (const section of drafts)
            acceptSettingDraftCommit(section, key, prior, response.result, accepted);
          warnings.value = response.warnings;
          outcome.value = response.result.changed
            ? messages.settings.saved(
                messages.settings.sections[key],
                formatNumber(response.result.libraryRevision, props.settings.preferences.locale),
              )
            : messages.settings.unchanged;
        }
        return response;
      },
      success: (result) =>
        result.changed
          ? messages.settings.saved(
              messages.settings.sections[key],
              formatNumber(result.libraryRevision, props.settings.preferences.locale),
            )
          : messages.settings.unchanged,
    });
  } catch (error) {
    // Lost responses are not evidence that a write did not commit.
    void props.settings.load();
    if (!disposed && current === lifetime) {
      draft.stale = true;
      errorMessage.value = isRevisionConflict(error)
        ? messages.settings.conflict
        : safeMessage(error);
    }
  } finally {
    if (!disposed && current === lifetime) {
      pending.value = false;
      writing.value = false;
      observeSnapshot(props.settings.state.snapshot);
      await nextTick();
      if (current === lifetime) document.getElementById(`settings-${key}-title`)?.focus();
    }
  }
}

async function previewImport(): Promise<void> {
  if (busy.value || review.value !== null) return;
  const current = lifetime;
  const owner = new SettingsImportFlow();
  flow = owner;
  props.home.registerReviewOwner(owner);
  let operation: SetupOperation<SettingsImportPreviewV1> | undefined;
  pending.value = true;
  errorMessage.value = null;
  outcome.value = null;
  warnings.value = [];
  try {
    await props.home.runOperation({
      action: "settings-import-preview",
      label: messages.settings.choosingImport,
      refreshLibrary: false,
      cancel: async () => {
        await operation?.cancel();
      },
      execute: async (report) => {
        operation = owner.preview(new LibraryOperationScope(null), report);
        const response = await operation.result;
        if (!disposed && current === lifetime) {
          review.value = response.result;
          warnings.value = response.warnings;
        }
        return response;
      },
      success: () => messages.settings.reviewReady,
    });
    if (!disposed && current === lifetime) {
      pending.value = false;
      void props.settings.load();
      await nextTick();
      if (current === lifetime) reviewHeading.value?.focus();
    }
  } catch (error) {
    if (!disposed && current === lifetime) {
      errorMessage.value = isOperationCancelled(error) ? null : safeMessage(error);
      outcome.value = isOperationCancelled(error) ? messages.operations.cancelled : null;
      await retireFlow();
    }
  } finally {
    if (!disposed && current === lifetime) pending.value = false;
  }
}

async function discardReview(): Promise<void> {
  if (busy.value) return;
  const current = lifetime;
  review.value = null;
  warnings.value = [];
  await retireFlow();
  await nextTick();
  if (!disposed && current === lifetime) importButton.value?.focus();
}

async function applyImport(): Promise<void> {
  const preview = review.value;
  const owner = flow;
  if (
    busy.value ||
    !preview ||
    !owner ||
    reviewStale.value ||
    props.settings.state.phase !== "ready"
  )
    return;
  const current = lifetime;
  let operation: SetupOperation<unknown> | undefined;
  pending.value = true;
  errorMessage.value = null;
  outcome.value = null;
  try {
    await props.home.runOperation({
      action: "settings-import-apply",
      label: messages.settings.applyingImport,
      cancel: async () => {
        await operation?.cancel();
      },
      execute: async (report) => {
        const applying = owner.apply(
          new LibraryOperationScope(preview.libraryRevision),
          {
            previewId: preview.previewId,
            approvalSha256: preview.approvalSha256,
          },
          report,
        );
        operation = applying;
        const response = await applying.result;
        if (!disposed && current === lifetime) {
          review.value = null;
          warnings.value = response.warnings;
          outcome.value = response.result.changed
            ? messages.settings.imported
            : messages.settings.unchanged;
        }
        // Import has no full exact snapshot; dirty drafts keep their original bases.
        void props.settings.load();
        return response;
      },
      success: (result) =>
        result.changed ? messages.settings.imported : messages.settings.unchanged,
    });
  } catch (error) {
    void props.settings.load();
    if (!disposed && current === lifetime) {
      errorMessage.value = isOperationCancelled(error)
        ? null
        : isRevisionConflict(error)
          ? messages.settings.importConflict
          : safeMessage(error);
      outcome.value = isOperationCancelled(error) ? messages.operations.cancelled : null;
      // Native consumes settings apply custody, including failed/cancelled apply.
      review.value = null;
    }
  } finally {
    if (!disposed && current === lifetime) {
      pending.value = false;
      await retireFlow();
      await nextTick();
      if (current === lifetime) importButton.value?.focus();
    }
  }
}

async function exportSettings(): Promise<void> {
  if (busy.value) return;
  const current = lifetime;
  let operation: SetupOperation<unknown> | undefined;
  pending.value = true;
  errorMessage.value = null;
  outcome.value = null;
  warnings.value = [];
  try {
    await props.home.runOperation({
      action: "settings-export",
      label: messages.settings.exporting,
      refreshLibrary: false,
      cancel: async () => {
        await operation?.cancel();
      },
      execute: async (report) => {
        const exporting = exportNonsecretSettings(new LibraryOperationScope(null), report);
        operation = exporting;
        const response = await exporting.result;
        if (!disposed && current === lifetime) {
          outcome.value = messages.settings.exported(
            formatNumber(response.result.writtenBytes, props.settings.preferences.locale),
          );
          warnings.value = response.warnings;
        }
        return response;
      },
      success: (result) =>
        messages.settings.exported(
          formatNumber(result.writtenBytes, props.settings.preferences.locale),
        ),
    });
  } catch (error) {
    if (!disposed && current === lifetime) {
      errorMessage.value = isOperationCancelled(error) ? null : safeMessage(error);
      outcome.value = isOperationCancelled(error) ? messages.operations.cancelled : null;
    }
  } finally {
    if (!disposed && current === lifetime) pending.value = false;
  }
}

function settingValue(change: SettingsChangeV1, side: "before" | "after"): string {
  const entry = change[side];
  if (entry === null) return messages.settings.absent;
  if (typeof entry.value === "string") return entry.value;
  return messages.settings.appearanceValue(
    entry.value.theme === undefined
      ? messages.settings.absent
      : messages.settings.themes[entry.value.theme],
    entry.value.reducedMotion === undefined
      ? messages.settings.absent
      : entry.value.reducedMotion
        ? messages.settings.on
        : messages.settings.off,
  );
}
</script>

<template>
  <RouteLeaveGuard :home="home" :dirty="dirty" :pending="pending" :discard="discard" />
  <section aria-labelledby="settings-title">
    <header class="section-heading">
      <div>
        <p class="eyebrow">{{ messages.settings.eyebrow }}</p>
        <h1 id="settings-title" data-route-heading tabindex="-1">{{ messages.settings.title }}</h1>
      </div>
      <button
        class="button-secondary"
        :disabled="busy || settings.state.phase === 'loading'"
        @click="settings.load()"
      >
        {{ messages.settings.refresh }}
      </button>
    </header>
    <p class="portable-intro">{{ messages.settings.description }}</p>
    <p v-if="settings.state.phase === 'loading'" role="status">{{ messages.settings.loading }}</p>
    <p v-if="settings.state.errorMessage" class="inline-alert" role="alert">
      {{ settings.state.errorMessage }}
    </p>
    <p v-if="settings.preferences.localeWarning" class="inline-alert" role="status">
      {{ settings.preferences.localeWarning }}
    </p>
    <p v-if="errorMessage" class="inline-alert" role="alert">{{ errorMessage }}</p>
    <p v-if="outcome" role="status">{{ outcome }}</p>
    <ul v-if="warnings.length" class="preview-list">
      <li v-for="(warning, index) in warnings" :key="index">{{ warning.message }}</li>
    </ul>
    <p v-if="settings.state.snapshot" class="quiet-state">
      {{
        messages.settings.revision(
          formatNumber(settings.state.snapshot.libraryRevision, settings.preferences.locale),
        )
      }}
    </p>

    <div v-if="settings.state.snapshot" class="portable-grid">
      <Card
        v-for="draft in drafts"
        :key="draft.key"
        as="section"
        :aria-labelledby="`settings-${draft.key}-title`"
      >
        <h2 :id="`settings-${draft.key}-title`" tabindex="-1">
          {{ messages.settings.sections[draft.key] }}
        </h2>
        <form class="stacked-form" @submit.prevent="saveSection(draft.key)">
          <fieldset :disabled="busy || settings.state.phase !== 'ready'">
            <legend class="sr-only">{{ messages.settings.sections[draft.key] }}</legend>
            <template v-if="draft.key === 'appearance'">
              <label for="settings-theme">{{ messages.settings.theme }}</label>
              <select
                id="settings-theme"
                v-model="appearance.value.theme"
                aria-describedby="settings-appearance-help"
              >
                <option :value="undefined">{{ messages.settings.themeDefault }}</option>
                <option value="system">{{ messages.settings.themes.system }}</option>
                <option value="light">{{ messages.settings.themes.light }}</option>
                <option value="dark">{{ messages.settings.themes.dark }}</option>
              </select>
              <label class="checkbox-row" for="settings-motion"
                ><input
                  id="settings-motion"
                  v-model="appearance.value.reducedMotion"
                  type="checkbox"
                  aria-describedby="settings-appearance-help"
                />{{ messages.settings.reducedMotion }}</label
              >
              <p id="settings-appearance-help" class="field-help">
                {{ messages.settings.appearanceHelp }}
              </p>
            </template>
            <template v-else-if="draft.key === 'locale'">
              <label for="settings-locale">{{ messages.settings.locale }}</label>
              <input
                id="settings-locale"
                v-model="locale.value"
                type="text"
                spellcheck="false"
                autocomplete="off"
                aria-describedby="settings-locale-help"
              />
              <p id="settings-locale-help" class="field-help">{{ messages.settings.localeHelp }}</p>
              <p class="field-help">
                {{
                  messages.settings.numberExample(
                    formatNumber(12345.67, settings.preferences.locale),
                  )
                }}
              </p>
            </template>
            <template v-else>
              <label for="settings-units">{{ messages.settings.units }}</label>
              <select
                id="settings-units"
                v-model="units.value"
                aria-describedby="settings-units-help"
              >
                <option value="metric">{{ messages.settings.metric }}</option>
                <option value="us-customary">{{ messages.settings.usCustomary }}</option>
              </select>
              <p id="settings-units-help" class="field-help">{{ messages.settings.unitsHelp }}</p>
              <p class="field-help">
                {{
                  formatUnit(
                    1,
                    settings.preferences.units === "metric" ? "kilometer" : "mile",
                    settings.preferences.locale,
                  )
                }}
              </p>
            </template>
            <p v-if="draft.base?.settings[draft.key] === null" class="quiet-state">
              {{ messages.settings.defaultInUse }}
            </p>
            <p v-else-if="draft.base" class="quiet-state">
              {{
                messages.settings.savedAt(
                  formatDateTime(
                    draft.base.settings[draft.key]!.updatedAt,
                    settings.preferences.locale,
                  ),
                )
              }}
            </p>
            <p v-if="draft.stale" role="status" class="inline-alert">
              {{ messages.settings.stale }}
            </p>
            <p v-else-if="isSettingDraftDirty(draft)" class="quiet-state">
              {{ messages.settings.unsaved }}
            </p>
            <div class="action-row">
              <button
                type="submit"
                :disabled="
                  draft.stale ||
                  !isSettingDraftDirty(draft) ||
                  (draft.key === 'locale' && locale.value.length === 0)
                "
              >
                {{ messages.settings.save }}
              </button>
              <button
                type="button"
                class="button-secondary"
                :disabled="draft.stale"
                @click="saveSection(draft.key, true)"
              >
                {{ messages.settings.reset }}
              </button>
              <button
                v-if="draft.stale || isSettingDraftDirty(draft)"
                type="button"
                class="button-quiet"
                @click="reloadSection(draft)"
              >
                {{ messages.settings.reloadDraft }}
              </button>
            </div>
          </fieldset>
        </form>
      </Card>
    </div>

    <Card as="section" class="portable-section" aria-labelledby="settings-portable-title">
      <h2 id="settings-portable-title">{{ messages.settings.portableTitle }}</h2>
      <p class="portable-intro">{{ messages.settings.portableDescription }}</p>
      <div class="action-row">
        <button ref="importButton" :disabled="busy || review !== null" @click="previewImport">
          {{ messages.settings.import }}
        </button>
        <button class="button-secondary" :disabled="busy" @click="exportSettings">
          {{ messages.settings.export }}
        </button>
      </div>
      <p v-if="pending" role="status">{{ messages.settings.pendingHelp }}</p>
      <button
        v-if="pending && home.state.operation?.cancel && !home.state.operation.settled"
        class="button-secondary"
        :disabled="home.state.operation.cancellationRequested"
        @click="home.cancelOperation()"
      >
        {{
          home.state.operation.cancellationRequested
            ? messages.operations.cancelling
            : messages.operations.cancel
        }}
      </button>
      <div v-if="review" class="preview">
        <h3 ref="reviewHeading" tabindex="-1">{{ messages.settings.reviewTitle }}</h3>
        <p>
          {{
            messages.settings.revision(
              formatNumber(review.libraryRevision, settings.preferences.locale),
            )
          }}
        </p>
        <p class="field-help">{{ messages.settings.reviewDescription }}</p>
        <p v-if="reviewStale" role="status" class="inline-alert">
          {{ messages.settings.importStale }}
        </p>
        <p v-if="review.changes.length === 0">{{ messages.settings.noChanges }}</p>
        <ul class="preview-list">
          <li v-for="change in review.changes" :key="change.key">
            <h4>
              {{ messages.settings.sections[change.key] }} —
              {{ change.after === null ? messages.settings.removal : messages.settings.change }}
            </h4>
            <dl class="metadata-list">
              <div>
                <dt>{{ messages.settings.before }}</dt>
                <dd>
                  {{ settingValue(change, "before")
                  }}<small v-if="change.before">
                    ·
                    {{
                      formatDateTime(change.before.updatedAt, settings.preferences.locale)
                    }}</small
                  >
                </dd>
              </div>
              <div>
                <dt>{{ messages.settings.after }}</dt>
                <dd>
                  {{ settingValue(change, "after")
                  }}<small v-if="change.after">
                    ·
                    {{ formatDateTime(change.after.updatedAt, settings.preferences.locale) }}</small
                  >
                </dd>
              </div>
            </dl>
          </li>
        </ul>
        <div class="action-row">
          <button
            :disabled="busy || reviewStale || settings.state.phase !== 'ready'"
            @click="applyImport"
          >
            {{ messages.settings.applyImport }}</button
          ><button class="button-secondary" :disabled="busy" @click="discardReview">
            {{ messages.settings.discardReview }}
          </button>
        </div>
      </div>
    </Card>
  </section>
</template>
