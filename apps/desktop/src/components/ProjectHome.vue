<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, shallowRef, watch } from "vue";
import { RouterLink, useRoute, useRouter } from "vue-router";
import { formatDateTime, formatNumber, messages } from "../messages";
import { type ProjectHomeController, type ProjectSummary } from "../project-home";
import { useWorkspaceStore } from "../stores/workspace";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./ui/dialog";
import EmptyState from "./planner/EmptyState.vue";

const props = defineProps<{ home: ProjectHomeController; locale?: string }>();
const router = useRouter();
const route = useRoute();
const workspace = useWorkspaceStore();
const search = ref("");
const duplicateTitle = ref("");
const duplicateBase = shallowRef<ProjectSummary | null>(null);
const checking = ref(false);
const checked = ref(false);
const deleting = ref(false);
const heading = ref<HTMLElement | null>(null);
const keepButton = ref<HTMLButtonElement | null>(null);
let returnFocus: HTMLElement | null = null;
let current = true;
let reviewGeneration = 0;
onBeforeUnmount(() => {
  current = false;
  reviewGeneration += 1;
});
const selected = computed(
  () =>
    props.home.state.projects.find((item) => item.scenarioId === props.home.state.selectedId) ??
    null,
);
const dirty = computed(() => duplicateTitle.value !== "");
const source = computed(() =>
  props.home.state.projects.find((item) => item.scenarioId === duplicateBase.value?.scenarioId),
);
const duplicateStale = computed(
  () =>
    duplicateBase.value !== null &&
    (props.home.state.phase !== "ready" || source.value?.revision !== duplicateBase.value.revision),
);
const filtered = computed(() => {
  const query = search.value.trim().toLocaleLowerCase(props.locale);
  return props.home.state.projects.filter((project) =>
    `${project.title}\n${project.domainPackId}\n${project.scenarioId}`
      .toLocaleLowerCase(props.locale)
      .includes(query),
  );
});
const groups = computed(() => [
  {
    key: "active",
    label: messages.projects.active,
    projects: filtered.value.filter((project) => !project.archived),
  },
  {
    key: "archived",
    label: messages.projects.archived,
    projects: filtered.value.filter((project) => project.archived),
  },
]);
const candidate = computed(() => workspace.deletionReview);
const currentCandidate = computed(() =>
  props.home.state.projects.find((item) => item.scenarioId === candidate.value?.project.scenarioId),
);
const candidateChanged = computed(
  () => currentCandidate.value?.revision !== candidate.value?.project.revision,
);
const exportNotice = computed(() => {
  const outcome = candidate.value?.exportOutcome;
  if (!outcome || outcome.kind === "none") return null;
  if (outcome.kind === "saved") return messages.library.exportSaved(outcome.artifactName);
  return outcome.kind === "cancelled"
    ? messages.library.exportCancelled
    : messages.library.exportReturned;
});
watch(
  selected,
  (project) => {
    if (!dirty.value) duplicateBase.value = project ? { ...project } : null;
  },
  { immediate: true },
);
watch(
  [() => route.query.project, () => props.home.state.phase, () => props.home.state.busyAction],
  () => {
    const id = route.query.project;
    if (typeof id === "string" && props.home.state.phase === "ready") {
      props.home.selectProject(
        props.home.state.projects.some((project) => project.scenarioId === id) ? id : null,
      );
    }
  },
  { immediate: true },
);
function discardDraft(): void {
  duplicateTitle.value = "";
  duplicateBase.value = selected.value ? { ...selected.value } : null;
}
async function duplicate(): Promise<void> {
  const base = duplicateBase.value;
  if (base === null || duplicateStale.value || !duplicateTitle.value.trim()) return;
  if ((await props.home.duplicateProject(base, duplicateTitle.value.trim())) && current)
    discardDraft();
}
async function reviewCopySource(): Promise<void> {
  if ((await props.home.refreshLibrary()) && current && source.value)
    duplicateBase.value = { ...source.value };
}
function startDelete(project: ProjectSummary, event: Event): void {
  if (props.home.state.busyAction !== null) return;
  returnFocus = event.currentTarget instanceof HTMLElement ? event.currentTarget : null;
  workspace.deletionReview = { project: { ...project }, exportOutcome: { kind: "none" } };
}
async function refreshDeletion(): Promise<boolean> {
  const review = candidate.value;
  if (review === null) return false;
  const captured = ++reviewGeneration;
  checking.value = true;
  checked.value = false;
  const refreshed = await props.home.refreshLibrary();
  if (!current || captured !== reviewGeneration || workspace.deletionReview !== review)
    return false;
  checking.value = false;
  checked.value = refreshed;
  return refreshed && !candidateChanged.value && currentCandidate.value !== undefined;
}
watch(
  candidate,
  (review) => {
    checked.value = false;
    if (review !== null) void refreshDeletion();
  },
  { immediate: true },
);
async function rereviewDeletion(): Promise<void> {
  await refreshDeletion();
  const review = candidate.value;
  if (current && checked.value && review !== null && currentCandidate.value) {
    workspace.deletionReview = {
      project: { ...currentCandidate.value },
      exportOutcome: review.exportOutcome,
    };
  }
}
async function confirmDelete(): Promise<void> {
  if (deleting.value || props.home.state.busyAction !== null || !(await refreshDeletion())) return;
  const review = candidate.value;
  if (review === null) return;
  deleting.value = true;
  const deleted = await props.home.deleteProject(review.project);
  if (current) {
    deleting.value = false;
    if (deleted && workspace.deletionReview === review) workspace.deletionReview = null;
  }
}
async function archiveInstead(): Promise<void> {
  if (props.home.state.busyAction !== null || !(await refreshDeletion())) return;
  const review = candidate.value;
  if (review === null || review.project.archived) return;
  deleting.value = true;
  const archived = await props.home.setArchived(review.project);
  if (current) {
    deleting.value = false;
    if (archived && workspace.deletionReview === review) workspace.deletionReview = null;
  }
}
async function exportFirst(): Promise<void> {
  const review = candidate.value;
  if (review === null || props.home.state.busyAction !== null) return;
  workspace.deletionReview = { ...review, exportOutcome: { kind: "unconfirmed" } };
  await router.push({ name: "project-export", params: { scenarioId: review.project.scenarioId } });
}
function closeDeletion(open: boolean): void {
  if (!open && !deleting.value) workspace.deletionReview = null;
}
function keep(): void {
  closeDeletion(false);
}
function preventDuringDelete(event: Event): void {
  if (deleting.value) event.preventDefault();
}
async function focusKeep(event: Event): Promise<void> {
  event.preventDefault();
  await nextTick();
  if (current) keepButton.value?.focus();
}
async function restoreFocus(event: Event): Promise<void> {
  event.preventDefault();
  await nextTick();
  if (current) (returnFocus?.isConnected ? returnFocus : heading.value)?.focus();
}
</script>

<template>
  <section class="page-stack" aria-labelledby="projects-heading">
    <header class="page-heading">
      <p class="eyebrow">{{ messages.projects.library }}</p>
      <h1 id="projects-heading" ref="heading" data-route-heading tabindex="-1">
        {{ messages.projects.heading }}
      </h1>
      <div class="action-row">
        <RouterLink :to="{ name: 'project-create' }" class="button-link">
          {{ messages.projects.newProject }}
        </RouterLink>
        <RouterLink :to="{ name: 'project-import' }">
          {{ messages.welcome.openExisting }}
        </RouterLink>
        <RouterLink :to="{ name: 'backup-restore' }">{{ messages.shell.backupRestore }}</RouterLink>
      </div>
    </header>
    <div v-if="home.state.phase === 'loading'" class="state-panel" role="status" aria-busy="true">
      <p>{{ messages.projects.loadingDescription }}</p>
    </div>
    <div v-else-if="home.state.phase === 'error'" class="state-panel" role="alert">
      <h2>{{ messages.projects.loadFailed }}</h2>
      <p>{{ home.state.errorMessage }}</p>
      <button type="button" @click="home.load">{{ messages.projects.retry }}</button>
    </div>
    <template v-else>
      <p class="status-label">{{ messages.library.local }}</p>
      <div class="field-stack">
        <label for="project-search">{{ messages.library.search }}</label>
        <input
          id="project-search"
          v-model="search"
          data-view-search
          type="search"
          aria-describedby="project-search-help"
        />
        <p id="project-search-help" class="field-help">{{ messages.library.searchHelp }}</p>
      </div>
      <p role="status">{{ messages.projects.count(filtered.length, locale) }}</p>
      <EmptyState
        v-if="home.state.projects.length === 0"
        :heading="messages.projects.emptyHeading"
        :description="messages.welcome.localOnly"
      >
        <template #actions>
          <RouterLink :to="{ name: 'project-create' }">{{ messages.welcome.startWork }}</RouterLink>
        </template>
      </EmptyState>
      <p v-else-if="filtered.length === 0">{{ messages.library.noMatches }}</p>
      <div class="library-layout">
        <div class="page-stack">
          <section
            v-for="group in groups"
            :key="group.key"
            class="state-panel"
            :aria-labelledby="`projects-${group.key}`"
          >
            <h2 :id="`projects-${group.key}`">{{ group.label }}</h2>
            <p v-if="group.projects.length === 0">
              {{
                group.key === "active" ? messages.projects.noActive : messages.library.noArchived
              }}
            </p>
            <ul v-else class="project-list">
              <li v-for="project in group.projects" :key="project.scenarioId" class="project-row">
                <RouterLink
                  :to="{ name: 'project-setup', params: { scenarioId: project.scenarioId } }"
                  :aria-label="
                    project.archived
                      ? messages.projects.openArchived(project.title)
                      : messages.projects.open(project.title)
                  "
                >
                  {{ project.title }}
                </RouterLink>
                <p>
                  {{
                    project.domainPackId === "official.workforce"
                      ? messages.welcome.workSchedule
                      : messages.library.unavailablePack(project.domainPackId)
                  }}
                </p>
                <p>
                  {{ messages.projects.lastSaved }}: {{ formatDateTime(project.updatedAt, locale) }}
                </p>
                <p>
                  {{ messages.library.lastOpened }}:
                  {{
                    project.lastOpenedAt === null
                      ? messages.library.neverOpened
                      : formatDateTime(project.lastOpenedAt, locale)
                  }}
                </p>
                <RouterLink :to="{ name: 'projects', query: { project: project.scenarioId } }">
                  {{ messages.library.details(project.title) }}
                </RouterLink>
              </li>
            </ul>
          </section>
        </div>
        <aside class="state-panel" aria-labelledby="project-details-heading">
          <h2 id="project-details-heading">{{ messages.library.detailsHeading }}</h2>
          <template v-if="selected">
            <h3>{{ selected.title }}</h3>
            <dl class="metadata-list">
              <div>
                <dt>{{ messages.projects.domainPack }}</dt>
                <dd>{{ selected.domainPackId }}</dd>
              </div>
              <div>
                <dt>{{ messages.projects.revision }}</dt>
                <dd>{{ formatNumber(selected.revision, locale) }}</dd>
              </div>
              <div>
                <dt>{{ messages.projects.lastSaved }}</dt>
                <dd>{{ formatDateTime(selected.updatedAt, locale) }}</dd>
              </div>
              <div>
                <dt>{{ messages.library.lastOpened }}</dt>
                <dd>
                  {{
                    selected.lastOpenedAt === null
                      ? messages.library.neverOpened
                      : formatDateTime(selected.lastOpenedAt, locale)
                  }}
                </dd>
              </div>
              <div>
                <dt>{{ messages.projects.id }}</dt>
                <dd class="monospace">{{ selected.scenarioId }}</dd>
              </div>
            </dl>
            <p v-if="selected.archived">{{ messages.library.archivedHelp }}</p>
            <div class="action-row">
              <RouterLink
                :to="{ name: 'project-setup', params: { scenarioId: selected.scenarioId } }"
              >
                {{ messages.projects.open(selected.title) }}
              </RouterLink>
              <RouterLink
                :to="{ name: 'project-export', params: { scenarioId: selected.scenarioId } }"
              >
                {{ messages.shell.export }}
              </RouterLink>
              <button
                type="button"
                class="button-secondary"
                :disabled="home.state.busyAction !== null"
                @click="home.setArchived(selected)"
              >
                {{ selected.archived ? messages.projects.unarchive : messages.projects.archive }}
              </button>
              <button
                type="button"
                class="button-danger-secondary"
                :disabled="home.state.busyAction !== null"
                @click="startDelete(selected, $event)"
              >
                {{ messages.projects.delete }}
              </button>
            </div>
          </template>
          <p v-else>{{ messages.library.noSelection }}</p>
          <form v-if="duplicateBase" class="stacked-form" @submit.prevent="duplicate">
            <h3>{{ messages.projects.duplicateAs }}</h3>
            <p>{{ messages.library.editCopy }}</p>
            <label for="duplicate-title">{{ messages.library.duplicateTitle }}</label>
            <input
              id="duplicate-title"
              v-model="duplicateTitle"
              required
              :disabled="home.state.busyAction !== null"
              autocomplete="off"
            />
            <p v-if="duplicateStale" role="status">{{ messages.library.copySourceChanged }}</p>
            <div class="action-row">
              <button
                v-if="duplicateStale"
                type="button"
                class="button-secondary"
                :disabled="home.state.busyAction !== null || !source"
                @click="reviewCopySource"
              >
                {{ messages.library.reviewCopySource }}
              </button>
              <button
                type="submit"
                :disabled="
                  home.state.busyAction !== null || duplicateStale || !duplicateTitle.trim()
                "
              >
                {{ messages.projects.duplicate }}
              </button>
              <button
                v-if="dirty"
                type="button"
                class="button-secondary"
                :disabled="home.state.busyAction !== null"
                @click="discardDraft"
              >
                {{ messages.library.discardDraft }}
              </button>
            </div>
          </form>
        </aside>
      </div>
    </template>
    <Dialog :open="candidate !== null" @update:open="closeDeletion">
      <DialogContent
        @open-auto-focus="focusKeep"
        @close-auto-focus="restoreFocus"
        @escape-key-down="preventDuringDelete"
        @pointer-down-outside="preventDuringDelete"
      >
        <template v-if="candidate">
          <DialogTitle>{{ messages.deletion.title(candidate.project.title) }}</DialogTitle>
          <DialogDescription>{{ messages.deletion.description }}</DialogDescription>
          <p>
            {{ messages.library.reviewRevision(formatNumber(candidate.project.revision, locale)) }}
          </p>
          <p>{{ messages.library.exportBeforeDelete }}</p>
          <p v-if="exportNotice" role="status">{{ exportNotice }}</p>
          <p v-if="checking" role="status">{{ messages.library.refreshBeforeDelete }}</p>
          <p v-else-if="!checked && !deleting" role="alert">
            {{ home.state.errorMessage ?? messages.projects.requestFailed }}
          </p>
          <p v-else-if="!currentCandidate && !deleting" role="status">
            {{ messages.library.missingBeforeDelete }}
          </p>
          <template v-else-if="candidateChanged && !deleting">
            <p role="status">{{ messages.library.changedBeforeDelete }}</p>
            <p v-if="currentCandidate">
              {{ currentCandidate.title }} ·
              {{ messages.library.reviewRevision(formatNumber(currentCandidate.revision, locale)) }}
            </p>
            <button
              type="button"
              :disabled="home.state.busyAction !== null"
              @click="rereviewDeletion"
            >
              {{ messages.library.rereview }}
            </button>
          </template>
          <p v-if="deleting" role="status">{{ messages.deletion.pending }}</p>
          <div class="dialog-actions">
            <button
              ref="keepButton"
              type="button"
              class="button-secondary"
              :disabled="deleting"
              @click="keep"
            >
              {{ messages.deletion.keep }}
            </button>
            <button
              v-if="!candidate.project.archived"
              type="button"
              class="button-secondary"
              :disabled="
                deleting ||
                checking ||
                !checked ||
                candidateChanged ||
                home.state.busyAction !== null
              "
              @click="archiveInstead"
            >
              {{ messages.library.archiveInstead }}
            </button>
            <button
              type="button"
              class="button-secondary"
              :disabled="deleting || home.state.busyAction !== null || !currentCandidate"
              @click="exportFirst"
            >
              {{ messages.library.exportFirst }}
            </button>
            <button
              type="button"
              class="button-danger"
              :disabled="
                deleting ||
                checking ||
                !checked ||
                candidateChanged ||
                home.state.busyAction !== null
              "
              @click="confirmDelete"
            >
              {{ messages.deletion.confirm }}
            </button>
          </div>
        </template>
      </DialogContent>
    </Dialog>
    <RouteLeaveGuard
      :home="home"
      :dirty="dirty"
      :pending="
        deleting ||
        home.state.busyAction?.startsWith('duplicate:') === true ||
        home.state.busyAction?.startsWith('archive:') === true
      "
      :discard="discardDraft"
    />
  </section>
</template>
