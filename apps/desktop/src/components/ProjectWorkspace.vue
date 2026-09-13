<script setup lang="ts">
import { computed, onScopeDispose, ref, watch } from "vue";
import { RouterLink, RouterView } from "vue-router";
import type { Revision } from "../api/generated";
import type { ProjectHomeController } from "../project-home";
import { formatDateTime, formatNumber, messages } from "../messages";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import { plannerMessage } from "./planner/messages";

const props = defineProps<{
  home: ProjectHomeController;
  scenarioId: string;
  locale?: string;
  libraryRevision?: Revision | null;
}>();
const emit = defineEmits<{
  exported: [outcome: { artifactName: string }];
  exportCancelled: [];
  exportFailed: [];
  undo: [];
  redo: [];
}>();
const project = computed(() =>
  props.home.state.projects.find((item) => item.scenarioId === props.scenarioId),
);
const opened = ref(false);
const opening = ref(false);
const failed = ref(false);
let attempted = false;
let current = true;
let generation = 0;
onScopeDispose(() => {
  current = false;
  generation += 1;
});
watch(
  () => props.scenarioId,
  () => {
    generation += 1;
    attempted = false;
    opened.value = false;
    failed.value = false;
  },
);
async function enter(): Promise<void> {
  if (
    !current ||
    attempted ||
    props.home.state.phase !== "ready" ||
    props.home.state.busyAction !== null ||
    project.value === undefined
  )
    return;
  attempted = true;
  opening.value = true;
  const captured = generation;
  const id = props.scenarioId;
  const result = await props.home.openProject(id);
  if (captured !== generation || id !== props.scenarioId) return;
  opening.value = false;
  failed.value = result === null;
  opened.value = result !== null;
  if (result !== null) props.home.selectProject(result.scenarioId);
}
watch(
  [
    () => props.home.state.phase,
    () => props.home.state.busyAction,
    () => project.value?.scenarioId,
  ],
  () => void enter(),
  { immediate: true },
);
async function retry(): Promise<void> {
  if (props.home.state.busyAction !== null) return;
  failed.value = false;
  if (!opened.value) attempted = false;
  await props.home.refreshLibrary();
  await enter();
}
function discardFeedback(): void {
  failed.value = false;
}
</script>

<template>
  <section class="page-stack" aria-labelledby="project-heading">
    <header class="page-heading">
      <p class="eyebrow">
        {{
          project?.domainPackId === "official.workforce"
            ? messages.welcome.workSchedule
            : messages.projects.savedMetadata
        }}
      </p>
      <h1 id="project-heading" data-route-heading tabindex="-1">
        {{
          project?.title ??
          (home.state.phase === "loading"
            ? messages.setup.opening
            : messages.setup.unavailableTitle)
        }}
      </h1>
      <p v-if="opened && project">{{ messages.library.local }}</p>
      <div class="action-row">
        <RouterLink :to="{ name: 'projects' }">{{ messages.setup.manage }}</RouterLink>
        <template v-if="opened && project">
          <RouterLink :to="{ name: 'project-setup', params: { scenarioId } }">
            {{ messages.setup.heading }}
          </RouterLink>
          <RouterLink :to="{ name: 'project-history', params: { scenarioId } }">
            {{ plannerMessage("history.title") }}
          </RouterLink>
          <RouterLink :to="{ name: 'project-export', params: { scenarioId } }">
            {{ messages.shell.export }}
          </RouterLink>
        </template>
      </div>
    </header>
    <div v-if="home.state.phase === 'error' || failed" class="state-panel" role="alert">
      <p>{{ home.state.errorMessage ?? messages.projects.requestFailed }}</p>
      <button type="button" :disabled="home.state.busyAction !== null" @click="retry">
        {{ messages.projects.retry }}
      </button>
    </div>
    <div
      v-else-if="home.state.phase === 'loading' || (project && !opened)"
      class="state-panel"
      role="status"
      aria-busy="true"
    >
      <p>{{ messages.setup.opening }}</p>
    </div>
    <div v-else-if="!project" class="state-panel" role="status">
      <p>{{ messages.setup.unavailable }}</p>
      <button type="button" :disabled="home.state.busyAction !== null" @click="retry">
        {{ messages.projects.retry }}
      </button>
    </div>
    <template v-else-if="opened">
      <details class="state-panel">
        <summary>{{ messages.projects.savedMetadata }}</summary>
        <dl class="metadata-list">
          <div>
            <dt>{{ messages.projects.domainPack }}</dt>
            <dd>{{ project.domainPackId }}</dd>
          </div>
          <div>
            <dt>{{ messages.projects.revision }}</dt>
            <dd>{{ formatNumber(project.revision, locale) }}</dd>
          </div>
          <div>
            <dt>{{ messages.projects.lastSaved }}</dt>
            <dd>{{ formatDateTime(project.updatedAt, locale) }}</dd>
          </div>
          <div>
            <dt>{{ messages.library.lastOpened }}</dt>
            <dd>
              {{
                project.lastOpenedAt === null
                  ? messages.library.neverOpened
                  : formatDateTime(project.lastOpenedAt, locale)
              }}
            </dd>
          </div>
          <div>
            <dt>{{ messages.projects.id }}</dt>
            <dd class="monospace">{{ project.scenarioId }}</dd>
          </div>
        </dl>
        <p v-if="project.archived">{{ messages.library.archivedHelp }}</p>
      </details>
      <RouterView v-slot="{ Component }">
        <component
          :is="Component"
          :home="home"
          :project="project"
          :locale="locale"
          :library-revision="libraryRevision"
          @exported="emit('exported', $event)"
          @export-cancelled="emit('exportCancelled')"
          @export-failed="emit('exportFailed')"
          @undo="emit('undo')"
          @redo="emit('redo')"
        />
      </RouterView>
    </template>
    <RouteLeaveGuard :home="home" :dirty="false" :pending="opening" :discard="discardFeedback" />
  </section>
</template>
