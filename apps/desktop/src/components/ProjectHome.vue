<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import type { AppliedMigrationDto, FixedExclusion } from "../api/generated";
import { formatDateTime, formatUnit, messages } from "../messages";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./ui/dialog";

import {
  defaultSupplementalCollisionChoices,
  recoverFocus,
  scenarioRevisionOutcome,
  supplementalIdentityKey,
  type CollisionChoice,
  type ProjectHomeController,
  type ProjectSummary,
  type SupplementalCollisionChoice,
} from "../project-home";

const props = defineProps<{ readonly home: ProjectHomeController }>();
const fixedExclusionLabels: Readonly<Record<FixedExclusion, string>> = {
  "local-undo-and-audit-history": "Local undo and audit history",
  "sqlite-and-database-internals": "SQLite and database internals",
  "credentials-tokens-and-keychain-references": "Credentials, tokens, and keychain references",
  "device-local-paths-and-window-state": "Device-local paths and window state",
  "logs-caches-and-temporary-data": "Logs, caches, and temporary data",
  "redistribution-prohibited-provider-data": "Redistribution-prohibited provider data",
  "executable-content": "Executable content",
};

function fixedExclusionLabel(exclusion: FixedExclusion): string {
  return fixedExclusionLabels[exclusion];
}

function appliedMigrationKey(migration: AppliedMigrationDto): string {
  return `${migration.registry}\u0000${migration.name}\u0000${migration.fromVersion.toString()}\u0000${migration.toVersion.toString()}\u0000${migration.versionSpace ?? ""}\u0000${migration.subject?.packId ?? ""}\u0000${migration.subject?.scenarioId ?? ""}\u0000${migration.subject?.revision.toString() ?? ""}`;
}

const state = props.home.state;

const createTitle = ref("");
const createDescription = ref("");
const timeZone = ref("UTC");
const locale = ref("en-US");
const units = ref<"metric" | "us-customary">("metric");
const firstDate = ref("");
const lastDate = ref("");
const gapPolicy = ref<"reject" | "moveForward" | "packDefined">("reject");
const overlapPolicy = ref<"earlier" | "later" | "reject">("earlier");
const duplicateTitle = ref("");
const deleteCandidate = ref<ProjectSummary | null>(null);
const confirmingDelete = ref(false);
const importIncludeResults = ref(true);
const importIncludeAssets = ref(true);
const importCollisions = ref<Record<string, CollisionChoice>>({});
const importSupplementalCollisions = ref<Record<string, SupplementalCollisionChoice>>({});
const backupTitle = ref("");
const restoreMode = ref<"add-backup" | "replace-library">("add-backup");
const restoreCollisions = ref<Record<string, CollisionChoice>>({});
const restoreSupplementalCollisions = ref<Record<string, SupplementalCollisionChoice>>({});
const confirmingRestore = ref(false);
const safetyBackupBypassPhrase = ref("");

const projectsHeading = ref<HTMLElement | null>(null);
const deleteCancel = ref<HTMLButtonElement | null>(null);
const restoreCancel = ref<HTMLButtonElement | null>(null);
const restoreReview = ref<HTMLButtonElement | null>(null);
const restoreChoose = ref<HTMLButtonElement | null>(null);
let deleteReturnFocus: HTMLElement | null = null;
let restoreReturnFocus: HTMLElement | null = null;
const deleteTrigger = ref<HTMLButtonElement | null>(null);

const activeProjects = computed(() => state.projects.filter((project) => !project.archived));
const archivedProjects = computed(() => state.projects.filter((project) => project.archived));
const selectedProject = computed(
  () => state.projects.find(({ scenarioId }) => scenarioId === state.selectedId) ?? null,
);
const restoreHasSettingChanges = computed(() => {
  const preview = state.restorePreview;
  return (
    preview !== null && (preview.settingsChanged.length > 0 || preview.settingsRemoved.length > 0)
  );
});
const restoreHasSupplementalCollisions = computed(
  () =>
    state.restoreMode !== "replace-library" &&
    (state.restorePreview?.supplementalCollisions.length ?? 0) > 0,
);
const restoreDescription = computed(() => {
  if (state.restoreMode !== "replace-library") return messages.restore.addDescription;
  if (
    state.restoreSafetyBackupFailure &&
    safetyBackupBypassPhrase.value.trim() === "REPLACE WITHOUT BACKUP"
  ) {
    return messages.restore.bypassDescription;
  }
  return messages.restore.replaceDescription;
});

async function submitCreate(): Promise<void> {
  const created = await props.home.createProject({
    title: createTitle.value.trim(),
    description: createDescription.value.trim(),
    domainPack: { id: "official.workforce", schemaVersion: 1 },
    settings: {
      timeZone: timeZone.value.trim(),
      locale: locale.value.trim(),
      units: units.value,
      firstDate: firstDate.value,
      lastDate: lastDate.value,
      gapPolicy: gapPolicy.value,
      overlapPolicy: overlapPolicy.value,
    },
  });
  if (created) {
    createTitle.value = "";
    createDescription.value = "";
    firstDate.value = "";
    lastDate.value = "";
  }
}

async function submitDuplicate(project: ProjectSummary): Promise<void> {
  const title = duplicateTitle.value.trim();
  if (!title) return;
  if (await props.home.duplicateProject(project, title)) duplicateTitle.value = "";
}

function openDeleteConfirmation(project: ProjectSummary): void {
  if (state.busyAction !== null) return;
  deleteReturnFocus = deleteTrigger.value ?? projectsHeading.value;
  deleteCandidate.value = project;
  confirmingDelete.value = true;
}

function cancelDelete(): void {
  if (state.busyAction !== null) return;
  confirmingDelete.value = false;
}

function focusDeleteCancel(event: Event): void {
  event.preventDefault();
  recoverFocus(deleteCancel.value);
}

async function restoreDeleteFocus(event: Event): Promise<void> {
  event.preventDefault();
  await nextTick();
  if (confirmingDelete.value) return;
  deleteCandidate.value = null;
  recoverFocus(deleteReturnFocus?.isConnected ? deleteReturnFocus : projectsHeading.value);
}

function preventBusyDismissal(event: Event): void {
  if (state.busyAction !== null) event.preventDefault();
}

async function confirmDelete(): Promise<void> {
  const project = deleteCandidate.value;
  if (!confirmingDelete.value || !project || state.busyAction !== null) return;
  const deleted = await props.home.deleteProject(project);
  if (deleted) {
    deleteReturnFocus = projectsHeading.value;
    confirmingDelete.value = false;
  }
}

async function submitImportPreview(): Promise<void> {
  if (
    !(await props.home.previewImport({
      includeResults: importIncludeResults.value,
      includeAssets: importIncludeAssets.value,
    }))
  )
    return;
  const collisions: Record<string, CollisionChoice> = {};
  for (const scenario of state.importPreview?.scenarios ?? []) {
    if (scenario.collides) collisions[scenario.scenarioId] = "create-copy";
  }
  importCollisions.value = collisions;
  importSupplementalCollisions.value = defaultSupplementalCollisionChoices(
    state.importPreview?.supplementalCollisions ?? [],
  );
}

async function submitRestorePreview(): Promise<void> {
  if (!(await props.home.previewRestore(restoreMode.value))) return;
  safetyBackupBypassPhrase.value = "";
  confirmingRestore.value = false;
  const collisions: Record<string, CollisionChoice> = {};
  if (restoreMode.value !== "replace-library") {
    for (const scenario of state.restorePreview?.scenarios ?? []) {
      if (scenario.collides) collisions[scenario.scenarioId] = "create-copy";
    }
  }
  restoreCollisions.value = collisions;
  restoreSupplementalCollisions.value =
    restoreMode.value === "replace-library"
      ? {}
      : defaultSupplementalCollisionChoices(state.restorePreview?.supplementalCollisions ?? []);
}

function openRestoreConfirmation(): void {
  if (state.busyAction !== null || !state.restorePreview) return;
  restoreReturnFocus = restoreReview.value;
  confirmingRestore.value = true;
}

function cancelRestore(): void {
  if (state.busyAction !== null) return;
  confirmingRestore.value = false;
}

function focusRestoreCancel(event: Event): void {
  event.preventDefault();
  recoverFocus(restoreCancel.value);
}

async function restoreRestoreFocus(event: Event): Promise<void> {
  event.preventDefault();
  await nextTick();
  recoverFocus(
    restoreReturnFocus?.isConnected
      ? restoreReturnFocus
      : (restoreChoose.value ?? projectsHeading.value),
  );
}

async function confirmRestore(): Promise<void> {
  if (!confirmingRestore.value || state.busyAction !== null) return;
  const restored = await props.home.applyRestore(
    restoreCollisions.value,
    restoreSupplementalCollisions.value,
    safetyBackupBypassPhrase.value.trim(),
  );
  if (restored) {
    restoreReturnFocus = projectsHeading.value;
    confirmingRestore.value = false;
  }
}
</script>

<template>
  <section
    class="project-home"
    aria-labelledby="projects-heading"
    :aria-busy="state.phase === 'loading'"
  >
    <div class="section-heading">
      <div>
        <p class="eyebrow">{{ messages.projects.library }}</p>
        <h2 id="projects-heading" ref="projectsHeading" tabindex="-1">
          {{ messages.projects.heading }}
        </h2>
      </div>
      <span v-if="state.phase === 'ready'" class="count-badge">
        {{ messages.projects.count(state.projects.length) }}
      </span>
    </div>

    <p
      :class="{ 'sr-only': state.phase !== 'error' }"
      role="status"
      aria-live="polite"
      aria-atomic="true"
    >
      {{ state.announcement }}
    </p>

    <div v-if="state.phase === 'loading'" class="state-panel" role="status" aria-live="polite">
      <span class="activity-mark" aria-hidden="true" />
      <div>
        <h3>{{ messages.projects.loading }}</h3>
        <p>{{ messages.projects.loadingDescription }}</p>
      </div>
    </div>

    <div v-else-if="state.phase === 'error'" class="state-panel state-panel--error" role="alert">
      <div>
        <h3>{{ messages.projects.loadFailed }}</h3>
        <p>{{ state.errorMessage }}</p>
      </div>
      <button type="button" @click="home.load">{{ messages.projects.retry }}</button>
    </div>

    <template v-else>
      <div v-if="state.errorMessage" class="inline-alert" role="alert">
        <strong>{{ messages.projects.requestFailed }}</strong>
        <span>{{ state.errorMessage }}</span>
      </div>

      <div class="home-layout">
        <div class="library-column">
          <div v-if="state.projects.length === 0" class="empty-state">
            <p class="empty-state__number" aria-hidden="true">01</p>
            <div>
              <h3>{{ messages.projects.emptyHeading }}</h3>
              <p>
                Create an <code>official.test</code> project below, or review an import artifact.
                Saved projects will reappear here when Eutheto restarts.
              </p>
            </div>
          </div>

          <template v-else>
            <section aria-labelledby="active-projects-heading">
              <h3 id="active-projects-heading" class="list-heading">
                {{ messages.projects.active }}
              </h3>
              <ul v-if="activeProjects.length" class="project-list">
                <li v-for="project in activeProjects" :key="project.scenarioId">
                  <button
                    type="button"
                    class="project-row"
                    :disabled="state.busyAction !== null"
                    :class="{ 'project-row--selected': project.scenarioId === state.selectedId }"
                    :aria-pressed="project.scenarioId === state.selectedId"
                    :aria-label="messages.projects.open(project.title)"
                    @click="home.selectProject(project.scenarioId)"
                  >
                    <span>
                      <strong>{{ project.title }}</strong>
                      <small>{{ project.domainPackId }} · revision {{ project.revision }}</small>
                    </span>
                    <span class="status-label">
                      <span aria-hidden="true">●</span>
                      <span>{{ messages.projects.active }}</span>
                    </span>
                  </button>
                </li>
              </ul>
              <p v-else class="quiet-state">
                {{ messages.projects.noActive }}
              </p>
            </section>

            <section v-if="archivedProjects.length" aria-labelledby="archived-projects-heading">
              <h3 id="archived-projects-heading" class="list-heading">
                {{ messages.projects.archived }}
              </h3>
              <ul class="project-list">
                <li v-for="project in archivedProjects" :key="project.scenarioId">
                  <button
                    type="button"
                    class="project-row"
                    :disabled="state.busyAction !== null"
                    :class="{ 'project-row--selected': project.scenarioId === state.selectedId }"
                    :aria-pressed="project.scenarioId === state.selectedId"
                    :aria-label="messages.projects.openArchived(project.title)"
                    @click="home.selectProject(project.scenarioId)"
                  >
                    <span>
                      <strong>{{ project.title }}</strong>
                      <small>{{ project.domainPackId }} · revision {{ project.revision }}</small>
                    </span>
                    <span class="status-label status-label--archived">
                      <span aria-hidden="true">◇</span> {{ messages.projects.archived }}
                    </span>
                  </button>
                </li>
              </ul>
            </section>
          </template>

          <article
            v-if="selectedProject"
            class="project-detail"
            aria-labelledby="selected-project-heading"
          >
            <div class="project-detail__heading">
              <div>
                <p class="eyebrow">{{ messages.projects.savedMetadata }}</p>
                <h3 id="selected-project-heading">{{ selectedProject.title }}</h3>
              </div>
              <span
                class="status-label"
                :class="{ 'status-label--archived': selectedProject.archived }"
              >
                <span aria-hidden="true">{{ selectedProject.archived ? "◇" : "●" }}</span>
                {{
                  selectedProject.archived ? messages.projects.archived : messages.projects.active
                }}
              </span>
            </div>
            <dl class="metadata-list">
              <div>
                <dt>{{ messages.projects.domainPack }}</dt>
                <dd>{{ selectedProject.domainPackId }}</dd>
              </div>
              <div>
                <dt>{{ messages.projects.revision }}</dt>
                <dd>{{ selectedProject.revision }}</dd>
              </div>
              <div>
                <dt>{{ messages.projects.lastSaved }}</dt>
                <dd>{{ formatDateTime(selectedProject.updatedAt) }}</dd>
              </div>
              <div>
                <dt>{{ messages.projects.id }}</dt>
                <dd class="identifier">{{ selectedProject.scenarioId }}</dd>
              </div>
            </dl>

            <div class="action-row">
              <button
                type="button"
                class="button-secondary"
                :disabled="state.busyAction !== null"
                @click="home.setArchived(selectedProject)"
              >
                {{
                  selectedProject.archived ? messages.projects.unarchive : messages.projects.archive
                }}
              </button>
              <button
                ref="deleteTrigger"
                type="button"
                class="button-danger button-quiet"
                :disabled="state.busyAction !== null"
                @click="openDeleteConfirmation(selectedProject)"
              >
                {{ messages.projects.delete }}
              </button>
            </div>

            <form class="compact-form" @submit.prevent="submitDuplicate(selectedProject)">
              <label :for="`duplicate-title-${selectedProject.scenarioId}`">
                {{ messages.projects.duplicateAs }}
              </label>
              <div class="inline-control">
                <input
                  :id="`duplicate-title-${selectedProject.scenarioId}`"
                  v-model="duplicateTitle"
                  required
                  autocomplete="off"
                  :placeholder="messages.projects.copyTitle"
                />
                <button
                  type="submit"
                  class="button-secondary"
                  :disabled="state.busyAction !== null"
                >
                  {{ messages.projects.duplicate }}
                </button>
              </div>
            </form>
          </article>
        </div>

        <aside class="create-panel" aria-labelledby="create-heading">
          <p class="eyebrow">{{ messages.projects.newProject }}</p>
          <h3 id="create-heading">Create Workforce project</h3>
          <p class="form-intro">
            {{ messages.projects.createIntroduction }}
          </p>
          <form class="stacked-form" @submit.prevent="submitCreate">
            <label for="create-title">{{ messages.projects.title }}</label>
            <input id="create-title" v-model="createTitle" required autocomplete="off" />

            <label for="create-description">{{ messages.projects.description }}</label>
            <textarea id="create-description" v-model="createDescription" rows="3" />

            <div class="field-grid">
              <div>
                <label for="create-time-zone">{{ messages.projects.timeZone }}</label>
                <input id="create-time-zone" v-model="timeZone" required autocomplete="off" />
              </div>
              <div>
                <label for="create-locale">{{ messages.projects.locale }}</label>
                <input id="create-locale" v-model="locale" required autocomplete="off" />
              </div>
            </div>

            <label for="create-units">{{ messages.projects.displayUnits }}</label>
            <select id="create-units" v-model="units">
              <option value="metric">{{ messages.projects.metric }}</option>
              <option value="us-customary">{{ messages.projects.usCustomary }}</option>
            </select>

            <label for="first-date">{{ messages.projects.planningStarts }}</label>
            <input
              id="first-date"
              v-model="firstDate"
              type="date"
              required
              autocomplete="off"
              aria-describedby="horizon-help"
            />
            <label for="last-date">{{ messages.projects.planningEnds }}</label>
            <input
              id="last-date"
              v-model="lastDate"
              type="date"
              :min="firstDate || undefined"
              required
              autocomplete="off"
              aria-describedby="horizon-help"
            />
            <p id="horizon-help" class="field-help">
              {{ messages.projects.planningDatesHelp }}
            </p>

            <div class="field-grid">
              <div>
                <label for="gap-policy">{{ messages.projects.missingClockTime }}</label>
                <select id="gap-policy" v-model="gapPolicy">
                  <option value="reject">{{ messages.projects.reject }}</option>
                  <option value="moveForward">{{ messages.projects.moveForward }}</option>
                  <option value="packDefined">{{ messages.projects.packPolicy }}</option>
                </select>
              </div>
              <div>
                <label for="overlap-policy">{{ messages.projects.repeatedClockTime }}</label>
                <select id="overlap-policy" v-model="overlapPolicy">
                  <option value="earlier">{{ messages.projects.earlier }}</option>
                  <option value="later">{{ messages.projects.later }}</option>
                  <option value="reject">{{ messages.projects.reject }}</option>
                </select>
              </div>
            </div>

            <button type="submit" :disabled="state.busyAction !== null">
              {{
                state.busyAction === "create"
                  ? messages.projects.creating
                  : messages.projects.create
              }}
            </button>
          </form>
        </aside>
      </div>

      <section class="portable-section" aria-labelledby="portable-heading">
        <div class="section-heading section-heading--compact">
          <div>
            <p class="eyebrow">Portable data</p>
            <h2 id="portable-heading">Import, backup, and restore</h2>
          </div>
        </div>
        <p class="portable-intro">
          Use system dialogs to choose <code>.eutheto</code> files and save backups. Cancelling a
          file chooser or Save dialog leaves the current preview and project library unchanged.
        </p>

        <div class="portable-grid">
          <details>
            <summary>Import projects</summary>
            <form class="stacked-form" @submit.prevent="submitImportPreview">
              <p id="import-file-help" class="field-help">
                Choose a scenario export to inspect its projects and collisions.
              </p>
              <fieldset>
                <legend>Bundled portable sections</legend>
                <label class="choice-row">
                  <input v-model="importIncludeResults" type="checkbox" />
                  <span>
                    <strong>Include retained results</strong>
                    <small>Preserves bundled accepted and current-revision results.</small>
                  </span>
                </label>
                <label class="choice-row">
                  <input v-model="importIncludeAssets" type="checkbox" />
                  <span>
                    <strong>Include referenced assets</strong>
                    <small>Preserves bundled files referenced by the imported project.</small>
                  </span>
                </label>
              </fieldset>
              <button
                type="submit"
                class="button-secondary"
                aria-describedby="import-file-help"
                :disabled="state.busyAction !== null"
              >
                {{
                  state.busyAction === "preview-import" ? "Choosing file…" : "Choose import file"
                }}
              </button>
            </form>
            <div
              v-if="state.importPreview"
              class="preview"
              aria-labelledby="import-preview-heading"
            >
              <h4 id="import-preview-heading">Import preview: {{ state.importPreview.title }}</h4>
              <p>{{ state.importPreview.scenarios.length }} project records found.</p>
              <dl class="preview-metadata">
                <div>
                  <dt>Source</dt>
                  <dd>
                    {{ state.importPreview.sourceApplication.name }}
                    {{ state.importPreview.sourceApplication.version }}
                  </dd>
                </div>
                <div>
                  <dt>Created</dt>
                  <dd>
                    <time :datetime="state.importPreview.createdAt">
                      {{ formatDateTime(state.importPreview.createdAt) }}
                    </time>
                  </dd>
                </div>
                <div>
                  <dt>Portable versions</dt>
                  <dd>
                    format {{ state.importPreview.sourceFormatVersion }}, schema
                    {{ state.importPreview.sourceSchemaVersion }}
                  </dd>
                </div>
                <div>
                  <dt>Bundle counts</dt>
                  <dd>
                    {{ state.importPreview.counts.scenarios }} projects,
                    {{ state.importPreview.counts.scenarioRevisions }} historical revisions,
                    {{ state.importPreview.counts.results }} results,
                    {{ state.importPreview.counts.sharedRecords }} shared records,
                    {{ state.importPreview.counts.preferences }} preferences,
                    {{ state.importPreview.counts.assets }} assets
                  </dd>
                </div>
                <div>
                  <dt>Included sections</dt>
                  <dd>{{ state.importPreview.includedSections.join(", ") }}</dd>
                </div>
                <div v-if="state.importPreview.excludedSections.length > 0">
                  <dt>Explicitly excluded sections</dt>
                  <dd>{{ state.importPreview.excludedSections.join(", ") }}</dd>
                </div>
                <div v-if="state.importPreview.sourceBackupSelection">
                  <dt>Source selection</dt>
                  <dd>
                    Results
                    {{
                      state.importPreview.sourceBackupSelection.includeResults
                        ? "included"
                        : "explicitly excluded"
                    }}; assets {{ state.importPreview.sourceBackupSelection.assetSelection }};
                    {{ state.importPreview.sourceBackupSelection.excludedAssetCount }} omitted;
                    scope {{ state.importPreview.sourceBackupSelection.scope }}.
                    <span v-if="state.importPreview.sourceBackupSelection.excludedAssetIds.length">
                      IDs:
                      {{ state.importPreview.sourceBackupSelection.excludedAssetIds.join(", ") }}
                    </span>
                  </dd>
                  <dd v-if="state.importPreview.sourceBackupSelection.fixedExclusions.length">
                    <p>Always excluded from this portable file:</p>
                    <ul aria-label="Fixed exclusions in import source">
                      <li
                        v-for="exclusion in state.importPreview.sourceBackupSelection
                          .fixedExclusions"
                        :key="exclusion"
                      >
                        {{ fixedExclusionLabel(exclusion) }}
                      </li>
                    </ul>
                  </dd>
                </div>
              </dl>
              <section aria-labelledby="import-capabilities-heading">
                <h5 id="import-capabilities-heading">Required capabilities</h5>
                <p v-if="state.importPreview.requiredCapabilities.length === 0">None.</p>
                <ul v-else>
                  <li
                    v-for="capability in state.importPreview.requiredCapabilities"
                    :key="`${capability.id}:${capability.version}`"
                  >
                    <code>{{ capability.id }}</code> version {{ capability.version }}
                  </li>
                </ul>
              </section>
              <section aria-labelledby="import-extensions-heading">
                <h5 id="import-extensions-heading">Preserved extensions</h5>
                <p v-if="state.importPreview.preservedExtensions.length === 0">None.</p>
                <ul v-else>
                  <li v-for="extension in state.importPreview.preservedExtensions" :key="extension">
                    <code>{{ extension }}</code>
                  </li>
                </ul>
              </section>
              <section aria-labelledby="import-migrations-heading">
                <h5 id="import-migrations-heading">Applied migrations</h5>
                <p v-if="state.importPreview.appliedMigrations.length === 0">None.</p>
                <ul v-else>
                  <li
                    v-for="migration in state.importPreview.appliedMigrations"
                    :key="appliedMigrationKey(migration)"
                  >
                    {{ migration.registry }} · {{ migration.name }} · {{ migration.fromVersion }} →
                    {{ migration.toVersion }}
                    <span v-if="migration.versionSpace">
                      · {{ migration.versionSpace }} version space
                    </span>
                    <span v-if="migration.subject">
                      · pack <code>{{ migration.subject.packId }}</code> · scenario
                      <code>{{ migration.subject.scenarioId }}</code> · revision
                      {{ migration.subject.revision }}
                    </span>
                  </li>
                </ul>
              </section>
              <section
                v-if="state.importWarnings.length > 0"
                aria-labelledby="import-warnings-heading"
              >
                <h5 id="import-warnings-heading">Preview warnings</h5>
                <ul>
                  <li v-for="warning in state.importWarnings" :key="warning.code">
                    <strong>{{ warning.code }}</strong> — {{ warning.message }}
                  </li>
                </ul>
              </section>
              <section
                v-if="state.importPreview.omittedAssets.length > 0"
                aria-labelledby="import-omitted-assets-heading"
              >
                <h5 id="import-omitted-assets-heading">Omitted asset placeholders</h5>
                <ul>
                  <li v-for="asset in state.importPreview.omittedAssets" :key="asset.assetId">
                    <strong>{{ asset.assetId }}</strong> — {{ asset.reason }};
                    {{ asset.originalMediaType }}, {{ formatUnit(asset.originalSize, "byte") }}
                  </li>
                </ul>
              </section>
              <ul class="preview-list">
                <li v-for="scenario in state.importPreview.scenarios" :key="scenario.scenarioId">
                  <span>
                    <strong>{{ scenario.title }}</strong>
                    <small>{{ scenario.scenarioId }}</small>
                    <small
                      v-if="
                        scenarioRevisionOutcome(scenario, importCollisions[scenario.scenarioId])
                          .revision !== null
                      "
                    >
                      Source revision {{ scenario.sourceRevision }} → selected outcome revision
                      {{
                        scenarioRevisionOutcome(scenario, importCollisions[scenario.scenarioId])
                          .revision
                      }}
                    </small>
                    <small v-else>No resulting project: Skip selected.</small>
                  </span>
                  <p
                    v-if="
                      scenarioRevisionOutcome(scenario, importCollisions[scenario.scenarioId])
                        .warning
                    "
                    class="warning"
                    role="alert"
                  >
                    <strong>Same-identity revision warning:</strong>
                    {{
                      scenarioRevisionOutcome(scenario, importCollisions[scenario.scenarioId])
                        .warning
                    }}
                  </p>
                  <template v-if="scenario.collides">
                    <label :for="`import-collision-${scenario.scenarioId}`">Collision action</label>
                    <select
                      :id="`import-collision-${scenario.scenarioId}`"
                      v-model="importCollisions[scenario.scenarioId]"
                    >
                      <option value="create-copy">Create a copy</option>
                      <option value="replace">Replace existing</option>
                      <option value="skip">Skip</option>
                    </select>
                  </template>
                  <span v-else class="status-label">
                    <span aria-hidden="true">✓</span> New project
                  </span>
                </li>
              </ul>
              <section
                v-if="state.importPreview.supplementalCollisions.length > 0"
                aria-labelledby="import-supplemental-heading"
              >
                <h5 id="import-supplemental-heading">Existing supplemental records</h5>
                <ul class="preview-list">
                  <li
                    v-for="identity in state.importPreview.supplementalCollisions"
                    :key="`${identity.section}:${identity.key}`"
                  >
                    <span>
                      <strong>{{ identity.key }}</strong>
                      <small>{{ identity.section }}</small>
                    </span>
                    <label :for="`import-supplemental-${identity.section}-${identity.key}`">
                      Supplemental collision action
                    </label>
                    <select
                      :id="`import-supplemental-${identity.section}-${identity.key}`"
                      v-model="importSupplementalCollisions[supplementalIdentityKey(identity)]"
                    >
                      <option value="skip">Skip</option>
                      <option value="replace">Replace existing</option>
                    </select>
                  </li>
                </ul>
              </section>
              <button
                type="button"
                :disabled="state.busyAction !== null"
                @click="home.applyImport(importCollisions, importSupplementalCollisions)"
              >
                Apply reviewed import
              </button>
            </div>
          </details>

          <details>
            <summary>Create backup</summary>
            <form class="stacked-form" @submit.prevent="home.previewBackup(backupTitle.trim())">
              <label for="backup-title">Backup title</label>
              <input id="backup-title" v-model="backupTitle" required autocomplete="off" />
              <button type="submit" class="button-secondary" :disabled="state.busyAction !== null">
                Preview backup
              </button>
            </form>
            <div
              v-if="state.backupPreview"
              class="preview"
              aria-labelledby="backup-preview-heading"
            >
              <h4 id="backup-preview-heading">Backup preview: {{ state.backupPreview.title }}</h4>
              <p>
                This backup will contain
                {{ formatUnit(state.backupPreview.byteLength, "byte") }}.
              </p>
              <p>
                Prepared library revision {{ state.backupPreview.libraryRevision }} · digest
                <code class="identifier">{{ state.backupPreview.digest }}</code>
              </p>
              <div v-if="state.backupPreview.backupSummary" class="preview">
                <p>
                  Results
                  {{ state.backupPreview.backupSummary.includeResults ? "included" : "excluded" }}.
                  Asset selection: {{ state.backupPreview.backupSummary.assetSelection }}.
                </p>
                <p>
                  {{ state.backupPreview.backupSummary.excludedAssetCount }} preserved omission
                  {{
                    state.backupPreview.backupSummary.excludedAssetCount === 1
                      ? "placeholder"
                      : "placeholders"
                  }}.
                  <span v-if="state.backupPreview.backupSummary.excludedAssetIds.length">
                    IDs: {{ state.backupPreview.backupSummary.excludedAssetIds.join(", ") }}.
                  </span>
                  <span v-if="state.backupPreview.backupSummary.exclusionScope">
                    Scope: {{ state.backupPreview.backupSummary.exclusionScope }}.
                  </span>
                </p>
                <p v-if="state.backupPreview.backupSummary.thresholdBytes !== null">
                  Threshold version {{ state.backupPreview.backupSummary.thresholdVersion }}:
                  {{ formatUnit(state.backupPreview.backupSummary.thresholdBytes, "byte") }}.
                </p>
                <section
                  v-if="state.backupPreview.backupSummary.fixedExclusions.length"
                  aria-labelledby="backup-fixed-exclusions-heading"
                >
                  <h5 id="backup-fixed-exclusions-heading">
                    Always excluded from this backup file
                  </h5>
                  <ul>
                    <li
                      v-for="exclusion in state.backupPreview.backupSummary.fixedExclusions"
                      :key="exclusion"
                    >
                      {{ fixedExclusionLabel(exclusion) }}
                    </li>
                  </ul>
                </section>
              </div>
              <form class="stacked-form" @submit.prevent="home.createBackup()">
                <p id="backup-save-help" class="field-help">
                  Choose where to save the reviewed backup.
                </p>
                <button
                  type="submit"
                  aria-describedby="backup-save-help"
                  :disabled="state.busyAction !== null"
                >
                  {{ state.busyAction === "create-backup" ? "Saving…" : "Save backup file" }}
                </button>
              </form>
            </div>
          </details>

          <details>
            <summary>Restore backup</summary>
            <form class="stacked-form" @submit.prevent="submitRestorePreview">
              <p id="restore-file-help" class="field-help">
                Choose a backup to inspect before applying either restore behavior.
              </p>
              <fieldset>
                <legend>Restore behavior</legend>
                <label class="choice-row">
                  <input v-model="restoreMode" type="radio" value="add-backup" />
                  <span>
                    <strong>Add to library</strong>
                    <small>Keep current projects and review collisions.</small>
                  </span>
                </label>
                <label class="choice-row">
                  <input v-model="restoreMode" type="radio" value="replace-library" />
                  <span>
                    <strong>Replace library</strong>
                    <small>Remove projects not present in this backup.</small>
                  </span>
                </label>
              </fieldset>
              <button
                ref="restoreChoose"
                type="submit"
                class="button-secondary"
                aria-describedby="restore-file-help"
                :disabled="state.busyAction !== null"
              >
                {{
                  state.busyAction === "preview-restore" ? "Choosing file…" : "Choose backup file"
                }}
              </button>
            </form>
            <div
              v-if="state.restorePreview"
              class="preview"
              aria-labelledby="restore-preview-heading"
            >
              <h4 id="restore-preview-heading">
                {{ state.restoreMode === "replace-library" ? "Replace" : "Add" }} preview:
                {{ state.restorePreview.title }}
              </h4>
              <p v-if="state.restoreMode === 'replace-library'" class="danger-note">
                <strong>Library replacement:</strong> projects absent from this backup will be
                removed after a safety backup.
              </p>
              <dl class="preview-metadata">
                <div>
                  <dt>Source</dt>
                  <dd>
                    {{ state.restorePreview.sourceApplication.name }}
                    {{ state.restorePreview.sourceApplication.version }}
                  </dd>
                </div>
                <div>
                  <dt>Created</dt>
                  <dd>
                    <time :datetime="state.restorePreview.createdAt">
                      {{ formatDateTime(state.restorePreview.createdAt) }}
                    </time>
                  </dd>
                </div>
                <div>
                  <dt>Portable versions</dt>
                  <dd>
                    format {{ state.restorePreview.sourceFormatVersion }}, schema
                    {{ state.restorePreview.sourceSchemaVersion }}
                  </dd>
                </div>
                <div>
                  <dt>Bundle counts</dt>
                  <dd>
                    {{ state.restorePreview.counts.scenarios }} projects,
                    {{ state.restorePreview.counts.scenarioRevisions }} historical revisions,
                    {{ state.restorePreview.counts.results }} results,
                    {{ state.restorePreview.counts.sharedRecords }} shared records,
                    {{ state.restorePreview.counts.preferences }} preferences,
                    {{ state.restorePreview.counts.assets }} assets
                  </dd>
                </div>
                <div>
                  <dt>Included sections</dt>
                  <dd>{{ state.restorePreview.includedSections.join(", ") }}</dd>
                </div>
                <div v-if="state.restorePreview.excludedSections.length > 0">
                  <dt>Excluded sections</dt>
                  <dd>{{ state.restorePreview.excludedSections.join(", ") }}</dd>
                </div>
                <div v-if="state.restorePreview.sourceBackupSelection">
                  <dt>Source selection</dt>
                  <dd>
                    Results
                    {{
                      state.restorePreview.sourceBackupSelection.includeResults
                        ? "included"
                        : "explicitly excluded"
                    }}; assets {{ state.restorePreview.sourceBackupSelection.assetSelection }};
                    {{ state.restorePreview.sourceBackupSelection.excludedAssetCount }} omitted;
                    scope {{ state.restorePreview.sourceBackupSelection.scope }}.
                    <span v-if="state.restorePreview.sourceBackupSelection.excludedAssetIds.length">
                      IDs:
                      {{ state.restorePreview.sourceBackupSelection.excludedAssetIds.join(", ") }}
                    </span>
                  </dd>
                  <dd v-if="state.restorePreview.sourceBackupSelection.fixedExclusions.length">
                    <p>Always excluded from this portable file:</p>
                    <ul aria-label="Fixed exclusions in restore source">
                      <li
                        v-for="exclusion in state.restorePreview.sourceBackupSelection
                          .fixedExclusions"
                        :key="exclusion"
                      >
                        {{ fixedExclusionLabel(exclusion) }}
                      </li>
                    </ul>
                  </dd>
                </div>
              </dl>
              <section aria-labelledby="restore-capabilities-heading">
                <h5 id="restore-capabilities-heading">Required capabilities</h5>
                <p v-if="state.restorePreview.requiredCapabilities.length === 0">None.</p>
                <ul v-else>
                  <li
                    v-for="capability in state.restorePreview.requiredCapabilities"
                    :key="`${capability.id}:${capability.version}`"
                  >
                    <code>{{ capability.id }}</code> version {{ capability.version }}
                  </li>
                </ul>
              </section>
              <section aria-labelledby="restore-extensions-heading">
                <h5 id="restore-extensions-heading">Preserved extensions</h5>
                <p v-if="state.restorePreview.preservedExtensions.length === 0">None.</p>
                <ul v-else>
                  <li
                    v-for="extension in state.restorePreview.preservedExtensions"
                    :key="extension"
                  >
                    <code>{{ extension }}</code>
                  </li>
                </ul>
              </section>
              <section aria-labelledby="restore-migrations-heading">
                <h5 id="restore-migrations-heading">Applied migrations</h5>
                <p v-if="state.restorePreview.appliedMigrations.length === 0">None.</p>
                <ul v-else>
                  <li
                    v-for="migration in state.restorePreview.appliedMigrations"
                    :key="appliedMigrationKey(migration)"
                  >
                    {{ migration.registry }} · {{ migration.name }} · {{ migration.fromVersion }} →
                    {{ migration.toVersion }}
                    <span v-if="migration.versionSpace">
                      · {{ migration.versionSpace }} version space
                    </span>
                    <span v-if="migration.subject">
                      · pack <code>{{ migration.subject.packId }}</code> · scenario
                      <code>{{ migration.subject.scenarioId }}</code> · revision
                      {{ migration.subject.revision }}
                    </span>
                  </li>
                </ul>
              </section>
              <section
                v-if="state.restoreWarnings.length > 0"
                aria-labelledby="restore-warnings-heading"
              >
                <h5 id="restore-warnings-heading">Preview warnings</h5>
                <ul>
                  <li v-for="warning in state.restoreWarnings" :key="warning.code">
                    <strong>{{ warning.code }}</strong> — {{ warning.message }}
                  </li>
                </ul>
              </section>
              <section
                v-if="state.restorePreview.omittedAssets.length > 0"
                aria-labelledby="restore-omitted-assets-heading"
              >
                <h5 id="restore-omitted-assets-heading">Omitted asset placeholders</h5>
                <ul>
                  <li v-for="asset in state.restorePreview.omittedAssets" :key="asset.assetId">
                    <strong>{{ asset.assetId }}</strong> — {{ asset.reason }};
                    {{ asset.originalMediaType }}, {{ formatUnit(asset.originalSize, "byte") }}
                  </li>
                </ul>
              </section>
              <section aria-labelledby="restore-settings-heading">
                <h5 id="restore-settings-heading">Application setting changes</h5>
                <p v-if="!restoreHasSettingChanges">No application settings will change.</p>
                <template v-else>
                  <div v-if="state.restorePreview.settingsChanged.length > 0">
                    <strong>Changed or added</strong>
                    <ul>
                      <li
                        v-for="key in state.restorePreview.settingsChanged"
                        :key="`changed:${key}`"
                      >
                        <code>{{ key }}</code>
                      </li>
                    </ul>
                  </div>
                  <div v-if="state.restorePreview.settingsRemoved.length > 0">
                    <strong>Removed</strong>
                    <ul>
                      <li
                        v-for="key in state.restorePreview.settingsRemoved"
                        :key="`removed:${key}`"
                      >
                        <code>{{ key }}</code>
                      </li>
                    </ul>
                  </div>
                </template>
              </section>
              <section
                v-if="state.restoreMode === 'replace-library'"
                aria-labelledby="replace-removals-heading"
              >
                <h5 id="replace-removals-heading">Current projects that will be removed</h5>
                <p v-if="state.restorePreview.removedScenarios.length === 0">
                  No current projects will be removed.
                </p>
                <ul v-else class="preview-list">
                  <li
                    v-for="scenario in state.restorePreview.removedScenarios"
                    :key="scenario.scenarioId"
                  >
                    <span>
                      <strong>{{ scenario.title }}</strong>
                      <small>
                        {{ scenario.scenarioId }} · revision {{ scenario.revision }} ·
                        {{ scenario.archived ? "archived" : "active" }}
                      </small>
                    </span>
                  </li>
                </ul>
                <h5>Current supplemental records that will be replaced or removed</h5>
                <p v-if="state.restorePreview.removedSupplemental.length === 0">
                  No current supplemental records will be removed.
                </p>
                <ul v-else class="preview-list">
                  <li
                    v-for="identity in state.restorePreview.removedSupplemental"
                    :key="`${identity.section}:${identity.key}`"
                  >
                    <span>
                      <strong>{{ identity.key }}</strong>
                      <small>{{ identity.section }}</small>
                    </span>
                  </li>
                </ul>
              </section>
              <ul class="preview-list">
                <li v-for="scenario in state.restorePreview.scenarios" :key="scenario.scenarioId">
                  <span>
                    <strong>{{ scenario.title }}</strong>
                    <small>{{ scenario.scenarioId }}</small>
                    <small
                      v-if="
                        scenarioRevisionOutcome(
                          scenario,
                          restoreCollisions[scenario.scenarioId],
                          state.restoreMode === 'replace-library',
                        ).revision !== null
                      "
                    >
                      Source revision {{ scenario.sourceRevision }} → selected outcome revision
                      {{
                        scenarioRevisionOutcome(
                          scenario,
                          restoreCollisions[scenario.scenarioId],
                          state.restoreMode === "replace-library",
                        ).revision
                      }}
                    </small>
                    <small v-else>No resulting project: Skip selected.</small>
                  </span>
                  <p
                    v-if="
                      scenarioRevisionOutcome(
                        scenario,
                        restoreCollisions[scenario.scenarioId],
                        state.restoreMode === 'replace-library',
                      ).warning
                    "
                    class="warning"
                    role="alert"
                  >
                    <strong>Same-identity revision warning:</strong>
                    {{
                      scenarioRevisionOutcome(
                        scenario,
                        restoreCollisions[scenario.scenarioId],
                        state.restoreMode === "replace-library",
                      ).warning
                    }}
                  </p>
                  <template v-if="scenario.collides && state.restoreMode !== 'replace-library'">
                    <label :for="`restore-collision-${scenario.scenarioId}`">
                      Collision action
                    </label>
                    <select
                      :id="`restore-collision-${scenario.scenarioId}`"
                      v-model="restoreCollisions[scenario.scenarioId]"
                    >
                      <option value="create-copy">Create a copy</option>
                      <option value="replace">Replace existing</option>
                      <option value="skip">Skip</option>
                    </select>
                  </template>
                  <span v-else-if="state.restoreMode === 'replace-library'" class="status-label">
                    Included by library replacement
                  </span>
                  <span v-else class="status-label">
                    <span aria-hidden="true">✓</span> New project
                  </span>
                </li>
              </ul>
              <section
                v-if="restoreHasSupplementalCollisions"
                aria-labelledby="restore-supplemental-heading"
              >
                <h5 id="restore-supplemental-heading">Supplemental collisions</h5>
                <ul class="preview-list">
                  <li
                    v-for="identity in state.restorePreview.supplementalCollisions"
                    :key="`${identity.section}:${identity.key}`"
                  >
                    <span>
                      <strong>{{ identity.key }}</strong>
                      <small>{{ identity.section }}</small>
                    </span>
                    <label :for="`restore-supplemental-${identity.section}-${identity.key}`">
                      Supplemental collision action
                    </label>
                    <select
                      :id="`restore-supplemental-${identity.section}-${identity.key}`"
                      v-model="restoreSupplementalCollisions[supplementalIdentityKey(identity)]"
                    >
                      <option value="skip">Skip</option>
                      <option value="replace">Replace existing</option>
                    </select>
                  </li>
                </ul>
              </section>
              <button
                ref="restoreReview"
                type="button"
                class="button-danger"
                :disabled="state.busyAction !== null"
                @click="openRestoreConfirmation"
              >
                {{ messages.restore.review }}
              </button>
            </div>
          </details>
        </div>
      </section>
    </template>
    <Dialog :open="confirmingDelete" @update:open="(open) => !open && cancelDelete()">
      <DialogContent
        class="project-confirmation"
        @open-auto-focus="focusDeleteCancel"
        @close-auto-focus="restoreDeleteFocus"
        @escape-key-down="preventBusyDismissal"
        @interact-outside.prevent
      >
        <DialogTitle>{{ messages.deletion.title(deleteCandidate?.title ?? "") }}</DialogTitle>
        <DialogDescription>{{ messages.deletion.description }}</DialogDescription>
        <p v-if="state.errorMessage" class="inline-alert" role="alert">
          <strong>{{ messages.projects.requestFailed }}</strong>
          {{ state.errorMessage }}
        </p>
        <p v-if="state.busyAction !== null" role="status">{{ messages.deletion.pending }}</p>
        <div class="action-row">
          <button
            ref="deleteCancel"
            type="button"
            class="button-secondary"
            :aria-disabled="state.busyAction !== null"
            @click="cancelDelete"
          >
            {{ messages.deletion.keep }}
          </button>
          <button
            type="button"
            class="button-danger"
            :disabled="!confirmingDelete || state.busyAction !== null"
            @click="confirmDelete"
          >
            {{ messages.deletion.confirm }}
          </button>
        </div>
      </DialogContent>
    </Dialog>

    <Dialog :open="confirmingRestore" @update:open="(open) => !open && cancelRestore()">
      <DialogContent
        class="project-confirmation"
        @open-auto-focus="focusRestoreCancel"
        @close-auto-focus="restoreRestoreFocus"
        @escape-key-down="preventBusyDismissal"
        @interact-outside.prevent
      >
        <DialogTitle>
          {{
            state.restoreMode === "replace-library"
              ? messages.restore.replaceTitle
              : messages.restore.addTitle
          }}
        </DialogTitle>
        <DialogDescription>{{ restoreDescription }}</DialogDescription>
        <p v-if="state.errorMessage" class="inline-alert" role="alert">
          <strong>{{ messages.projects.requestFailed }}</strong>
          {{ state.errorMessage }}
        </p>
        <p v-if="!state.restorePreview && state.busyAction === null" role="alert">
          {{ messages.restore.previewExpired }}
        </p>
        <div v-if="state.restoreSafetyBackupFailure" class="stacked-form">
          <p class="danger-note" role="alert">
            <strong>{{ messages.restore.safetyBackupFailed }}</strong>
            {{ state.restoreSafetyBackupFailure }}
          </p>
          <label for="safety-backup-bypass-phrase">{{ messages.restore.bypassLabel }}</label>
          <input
            id="safety-backup-bypass-phrase"
            v-model="safetyBackupBypassPhrase"
            autocomplete="off"
            :disabled="state.busyAction !== null"
            aria-describedby="safety-backup-bypass-help"
          />
          <p id="safety-backup-bypass-help" class="field-help">
            {{ messages.restore.bypassHelpBefore }} <code>REPLACE WITHOUT BACKUP</code>
            {{ messages.restore.bypassHelpAfter }}
          </p>
        </div>
        <p v-if="state.busyAction !== null" role="status">{{ messages.restore.pending }}</p>
        <div class="action-row">
          <button
            ref="restoreCancel"
            type="button"
            class="button-secondary"
            :aria-disabled="state.busyAction !== null"
            @click="cancelRestore"
          >
            {{ messages.restore.back }}
          </button>
          <button
            type="button"
            class="button-danger"
            :disabled="!confirmingRestore || state.busyAction !== null || !state.restorePreview"
            @click="confirmRestore"
          >
            {{ messages.restore.confirm }}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  </section>
</template>
