<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watchEffect } from "vue";
import {
  RouterLink,
  RouterView,
  useRoute,
  useRouter,
  type RouteLocationNormalizedLoaded,
  type RouteLocationRaw,
} from "vue-router";
import { redoScenario, undoScenario } from "./api/generated";
import { createApplicationSettingsController } from "./application-settings";
import { messages } from "./messages";
import { createProjectHomeController, safeMessage } from "./project-home";
import { useWorkspaceStore, type ProjectDeletionReview } from "./stores/workspace";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./components/ui/dialog";

const home = createProjectHomeController();
const settings = createApplicationSettingsController(home);
const workspace = useWorkspaceStore();
const route = useRoute();
const router = useRouter();
const paletteOpen = ref(false);
const shortcutsOpen = ref(false);
const commandSearch = ref("");
const paletteSearch = ref<HTMLInputElement | null>(null);
const commandsButton = ref<HTMLButtonElement | null>(null);
const helpClose = ref<HTMLButtonElement | null>(null);
const main = ref<HTMLElement | null>(null);
const palette = ref<HTMLElement | null>(null);
const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");
const systemDark = ref(darkQuery.matches);
const modifier = /Macintosh|Mac OS X/u.test(navigator.userAgent) ? "Command" : "Ctrl";
let current = true;
let modalReturnFocus: HTMLElement | null = null;
let paletteNavigation = false;
function updateSystemTheme(event: MediaQueryListEvent): void {
  systemDark.value = event.matches;
}
darkQuery.addEventListener("change", updateSystemTheme);
watchEffect(() => {
  const preference = settings.preferences.theme;
  document.documentElement.dataset.theme =
    preference === "system" ? (systemDark.value ? "dark" : "light") : preference;
  document.documentElement.dataset.reducedMotion =
    settings.preferences.forceReducedMotion.toString();
});
const contextProject = computed(() => {
  if (home.state.phase !== "ready") return undefined;
  const id =
    typeof route.params.scenarioId === "string"
      ? route.params.scenarioId
      : route.name === "projects"
        ? workspace.selectedProjectId
        : null;
  return home.state.projects.find((project) => project.scenarioId === id);
});
const canChangeScenario = computed(
  () =>
    contextProject.value?.domainPackId === "official.workforce" && !contextProject.value.archived,
);
const returningToDelete = computed(
  () =>
    route.name === "project-export" &&
    workspace.deletionReview !== null &&
    route.params.scenarioId === workspace.deletionReview.project.scenarioId,
);
type WorkspaceCommand = { readonly id: string; readonly label: string } & (
  { readonly to: RouteLocationRaw } | { readonly change: "undo" | "redo" }
);
const commands = computed<readonly WorkspaceCommand[]>(() => {
  const available: WorkspaceCommand[] = [
    { id: "start", label: messages.shell.start, to: { name: "welcome" } },
    { id: "projects", label: messages.projects.library, to: { name: "projects" } },
    { id: "create", label: messages.welcome.startWork, to: { name: "project-create" } },
    { id: "import", label: messages.welcome.openExisting, to: { name: "project-import" } },
    { id: "settings", label: messages.shell.settings, to: { name: "settings" } },
    { id: "backup", label: messages.shell.backupRestore, to: { name: "backup-restore" } },
    { id: "about", label: messages.shell.about, to: { name: "about" } },
  ];
  if (contextProject.value)
    available.push({
      id: "export",
      label: messages.shell.export,
      to: { name: "project-export", params: { scenarioId: contextProject.value.scenarioId } },
    });
  if (canChangeScenario.value)
    available.push(
      { id: "undo", label: messages.shell.undo, change: "undo" },
      { id: "redo", label: messages.shell.redo, change: "redo" },
    );
  for (const project of home.state.projects)
    available.push({
      id: `project:${project.scenarioId}`,
      label: messages.projects.open(project.title),
      to: { name: "project-setup", params: { scenarioId: project.scenarioId } },
    });
  const query = commandSearch.value.trim().toLocaleLowerCase(settings.preferences.locale);
  return available.filter((command) =>
    `${command.label}\n${command.id}`
      .toLocaleLowerCase(settings.preferences.locale)
      .includes(query),
  );
});
function pageProps(location: RouteLocationNormalizedLoaded) {
  const common = { home, locale: settings.preferences.locale };
  if (location.name === "settings") return { home, settings };
  if (location.name === "welcome" || location.name === "project-create")
    return { ...common, units: settings.preferences.units };
  if (
    location.name === "project-import" ||
    location.name === "backup-restore" ||
    location.params.scenarioId !== undefined
  )
    return {
      ...common,
      libraryRevision: settings.state.snapshot?.libraryRevision ?? null,
      ...(typeof location.params.scenarioId === "string"
        ? {
            onUndo: () => changeScenario("undo"),
            onRedo: () => changeScenario("redo"),
          }
        : {}),
      ...(location.name === "project-export"
        ? {
            onExported,
            onExportCancelled: () => {
              recordExport({ kind: "cancelled" });
            },
            onExportFailed: () => {
              recordExport({ kind: "unconfirmed" });
            },
          }
        : {}),
    };
  if (location.name === "not-found") return {};
  return common;
}
function pageKey(location: RouteLocationNormalizedLoaded): string | symbol | undefined {
  return typeof location.params.scenarioId === "string"
    ? `project:${location.params.scenarioId}`
    : location.name;
}
async function focusRoute(location: RouteLocationNormalizedLoaded): Promise<void> {
  await nextTick();
  if (!current || route.fullPath !== location.fullPath) return;
  const fragment = location.hash ? document.getElementById(location.hash.slice(1)) : null;
  const headings = main.value?.querySelectorAll<HTMLElement>("[data-route-heading]");
  const target =
    fragment && main.value?.contains(fragment)
      ? fragment
      : (headings?.item(headings.length - 1) ?? main.value?.querySelector<HTMLElement>("h1"));
  if (target) {
    target.tabIndex = -1;
    target.focus();
    target.scrollIntoView({ block: "start", behavior: "auto" });
  }
}
const stopRouteFocus = router.afterEach((to, _from, failure) => {
  if (!failure) void focusRoute(to);
});
function rememberFocus(): void {
  modalReturnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
}
async function openPalette(): Promise<void> {
  if (!paletteOpen.value) {
    rememberFocus();
    commandSearch.value = "";
    paletteNavigation = false;
  }
  paletteOpen.value = true;
  await nextTick();
  if (current) paletteSearch.value?.focus();
}
async function openShortcuts(): Promise<void> {
  rememberFocus();
  paletteNavigation = false;
  shortcutsOpen.value = true;
  await nextTick();
  if (current) helpClose.value?.focus();
}
function restoreModalFocus(event: Event): void {
  event.preventDefault();
  if (current && !paletteNavigation)
    (modalReturnFocus?.isConnected ? modalReturnFocus : commandsButton.value)?.focus();
}
function moveCommand(event: KeyboardEvent, direction: number): void {
  const items = [
    ...(palette.value?.querySelectorAll<HTMLButtonElement>("[data-command-item]:not(:disabled)") ??
      []),
  ];
  if (!items.length) return;
  event.preventDefault();
  const active = items.findIndex((item) => item === document.activeElement);
  const next =
    active < 0
      ? direction > 0
        ? 0
        : items.length - 1
      : (active + direction + items.length) % items.length;
  items[next]?.focus();
}
async function changeScenario(change: "undo" | "redo"): Promise<void> {
  const project = contextProject.value;
  if (!project || !canChangeScenario.value || home.state.busyAction !== null) return;
  try {
    await home.runOperation({
      action: `${change}:${project.scenarioId}`,
      label: change === "undo" ? messages.shell.undoing : messages.shell.redoing,
      execute: () =>
        change === "undo"
          ? undoScenario(project.scenarioId, project.revision)
          : redoScenario(project.scenarioId, project.revision),
      success: () => (change === "undo" ? messages.shell.undone : messages.shell.redone),
    });
  } catch {
    /* The root operation owner already presents the authoritative failure. */
  }
}
async function chooseCommand(command: WorkspaceCommand): Promise<void> {
  const fromPalette = paletteOpen.value;
  paletteNavigation = "to" in command;
  paletteOpen.value = false;
  try {
    if ("to" in command) {
      const failure = await router.push(command.to);
      if (failure && current && fromPalette) {
        paletteNavigation = false;
        (modalReturnFocus?.isConnected ? modalReturnFocus : commandsButton.value)?.focus();
      }
    } else await changeScenario(command.change);
  } catch (error) {
    home.state.errorMessage = safeMessage(error);
  }
}
function recordExport(outcome: ProjectDeletionReview["exportOutcome"]): void {
  const review = workspace.deletionReview;
  if (returningToDelete.value && review !== null)
    workspace.deletionReview = { ...review, exportOutcome: outcome };
}
function onExported(outcome: { artifactName: string }): void {
  recordExport({ kind: "saved", artifactName: outcome.artifactName });
}
function onKeydown(event: KeyboardEvent): void {
  if (
    event.defaultPrevented ||
    event.isComposing ||
    event.altKey ||
    !(event.ctrlKey || event.metaKey)
  )
    return;
  const key = event.key.toLowerCase();
  const dialog = document.querySelector('[role="dialog"][data-state="open"]');
  if (key === "k" && (!dialog || paletteOpen.value)) {
    event.preventDefault();
    void openPalette();
    return;
  }
  if (dialog) return;
  if (key === "f") {
    const input = main.value?.querySelector<HTMLInputElement>("[data-view-search]");
    if (input) {
      event.preventDefault();
      input.focus();
      input.select();
    }
    return;
  }
  if (key === "s" && contextProject.value) {
    event.preventDefault();
    void chooseCommand({
      id: "export",
      label: messages.shell.export,
      to: { name: "project-export", params: { scenarioId: contextProject.value.scenarioId } },
    });
    return;
  }
  const target = event.target instanceof HTMLElement ? event.target : null;
  if (
    key === "z" &&
    !target?.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"])') &&
    canChangeScenario.value
  ) {
    event.preventDefault();
    void changeScenario(event.shiftKey ? "redo" : "undo");
  }
}
onMounted(async () => {
  window.addEventListener("keydown", onKeydown);
  await home.startEventListeners();
  await Promise.all([home.load(), settings.load()]);
});
onUnmounted(() => {
  current = false;
  stopRouteFocus();
  window.removeEventListener("keydown", onKeydown);
  darkQuery.removeEventListener("change", updateSystemTheme);
  settings.dispose();
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.reducedMotion;
  void home.dispose().catch(() => {
    /* Native window teardown owns resources after the root has gone. */
  });
});
</script>

<template>
  <div class="desktop-shell">
    <a href="#workspace-main" class="skip-link" @click.prevent="main?.focus()">{{
      messages.shell.skip
    }}</a>
    <header class="shell-header">
      <RouterLink :to="{ name: 'welcome' }" class="app-brand">{{ messages.app.title }}</RouterLink>
      <p>{{ messages.app.eyebrow }}</p>
      <div class="action-row">
        <button ref="commandsButton" type="button" class="button-secondary" @click="openPalette">
          {{ messages.shell.commands }} <kbd>{{ modifier }}+K</kbd>
        </button>
        <button type="button" class="button-secondary" @click="openShortcuts">
          {{ messages.shell.shortcuts }}
        </button>
      </div>
    </header>
    <nav class="shell-navigation" :aria-label="messages.shell.navigation">
      <RouterLink :to="{ name: 'welcome' }">{{ messages.shell.start }}</RouterLink>
      <RouterLink :to="{ name: 'projects' }">{{ messages.shell.projects }}</RouterLink>
      <RouterLink :to="{ name: 'settings' }">{{ messages.shell.settings }}</RouterLink>
      <RouterLink :to="{ name: 'backup-restore' }">{{ messages.shell.backupRestore }}</RouterLink>
      <RouterLink :to="{ name: 'about' }">{{ messages.shell.about }}</RouterLink>
    </nav>
    <main id="workspace-main" ref="main" tabindex="-1">
      <p v-if="contextProject" class="workspace-context">
        {{ messages.app.selectedProject(contextProject.title) }}
      </p>
      <p class="workspace-outcome" role="status" aria-live="polite" aria-atomic="true">
        {{ home.state.announcement }}
      </p>
      <p
        v-if="settings.preferences.localeWarning && route.name !== 'settings'"
        class="inline-alert"
        role="status"
      >
        {{ settings.preferences.localeWarning }}
        <RouterLink :to="{ name: 'settings' }">{{ messages.shell.settings }}</RouterLink>
      </p>
      <p
        v-if="settings.state.phase === 'error' && route.name !== 'settings'"
        class="inline-alert"
        role="alert"
      >
        {{ messages.settings.unavailablePreferences }}
        <RouterLink :to="{ name: 'settings' }">{{ messages.shell.settings }}</RouterLink>
      </p>
      <template v-if="route.name !== 'projects' && typeof route.params.scenarioId !== 'string'">
        <p v-if="home.state.phase === 'loading'" role="status">{{ messages.projects.loading }}</p>
        <section v-else-if="home.state.phase === 'error'" class="inline-alert" role="alert">
          <strong>{{ messages.projects.loadFailed }}</strong>
          <p>{{ home.state.errorMessage }}</p>
          <button type="button" @click="home.load">{{ messages.projects.retry }}</button>
        </section>
      </template>
      <p
        v-if="home.state.errorMessage && home.state.phase === 'ready'"
        class="inline-alert"
        role="alert"
      >
        {{ home.state.errorMessage }}
      </p>
      <section
        v-if="home.state.operation"
        class="state-panel operation-panel"
        aria-labelledby="operation-label"
      >
        <div>
          <h2 id="operation-label">{{ home.state.operation.label }}</h2>
          <p role="status" aria-live="polite" aria-atomic="true">
            {{
              home.state.operation.settled
                ? messages.operations.refreshing
                : home.state.operation.cancellationRequested
                  ? messages.operations.cancelling
                  : home.state.operation.phase
                    ? messages.operations.phases[home.state.operation.phase]
                    : messages.operations.pending
            }}
          </p>
        </div>
        <button
          v-if="home.state.operation.cancel && !home.state.operation.settled"
          type="button"
          class="button-secondary"
          :disabled="home.state.operation.cancellationRequested"
          @click="home.cancelOperation"
        >
          {{ messages.operations.cancel }}
        </button>
      </section>
      <section v-if="home.state.reviewCleanupError" class="inline-alert" role="alert">
        <p>{{ home.state.reviewCleanupError }}</p>
        <button
          type="button"
          class="button-secondary"
          :disabled="home.state.retryingReviewCleanup"
          @click="home.retryReviewCleanup"
        >
          {{
            home.state.retryingReviewCleanup
              ? messages.operations.retryingCleanup
              : messages.operations.retryCleanup
          }}
        </button>
      </section>
      <p v-if="returningToDelete && workspace.deletionReview" class="state-panel">
        <RouterLink
          :to="{
            name: 'projects',
            query: { project: workspace.deletionReview.project.scenarioId },
          }"
        >
          {{ messages.library.returnToDelete }}
        </RouterLink>
      </p>
      <RouterView v-slot="{ Component, route: location }">
        <component :is="Component" :key="pageKey(location)" v-bind="pageProps(location)" />
      </RouterView>
      <footer class="boundary-note">{{ messages.app.boundary }}</footer>
    </main>
    <Dialog v-model:open="paletteOpen">
      <DialogContent
        @close-auto-focus="restoreModalFocus"
        @keydown.down="moveCommand($event, 1)"
        @keydown.up="moveCommand($event, -1)"
      >
        <DialogTitle>{{ messages.shell.commandTitle }}</DialogTitle
        ><DialogDescription>{{ messages.shell.commandDescription }}</DialogDescription>
        <div ref="palette" class="page-stack">
          <label for="command-search">{{ messages.shell.commandSearch }}</label
          ><input
            id="command-search"
            ref="paletteSearch"
            v-model="commandSearch"
            type="search"
            autocomplete="off"
          />
          <p v-if="commands.length === 0" role="status">{{ messages.shell.noCommands }}</p>
          <ul v-else class="command-list">
            <li v-for="command in commands" :key="command.id">
              <button
                type="button"
                data-command-item
                :disabled="'change' in command && home.state.busyAction !== null"
                @click="chooseCommand(command)"
              >
                {{ command.label }}
              </button>
            </li>
          </ul>
        </div>
        <button type="button" class="button-secondary" @click="paletteOpen = false">
          {{ messages.shell.close }}
        </button>
      </DialogContent>
    </Dialog>
    <Dialog v-model:open="shortcutsOpen">
      <DialogContent @close-auto-focus="restoreModalFocus">
        <DialogTitle>{{ messages.shell.shortcuts }}</DialogTitle
        ><DialogDescription>{{ messages.shell.shortcutDescription }}</DialogDescription>
        <dl class="metadata-list">
          <div>
            <dt>
              <kbd>{{ modifier }}+K</kbd>
            </dt>
            <dd>{{ messages.shell.shortcutActions.palette }}</dd>
          </div>
          <div>
            <dt>
              <kbd>{{ modifier }}+F</kbd>
            </dt>
            <dd>{{ messages.shell.shortcutActions.search }}</dd>
          </div>
          <div>
            <dt>
              <kbd>{{ modifier }}+S</kbd>
            </dt>
            <dd>{{ messages.shell.shortcutActions.export }}</dd>
          </div>
          <div>
            <dt>
              <kbd>{{ modifier }}+Z</kbd>
            </dt>
            <dd>{{ messages.shell.shortcutActions.undo }}</dd>
          </div>
          <div>
            <dt>
              <kbd>{{ modifier }}+Shift+Z</kbd>
            </dt>
            <dd>{{ messages.shell.shortcutActions.redo }}</dd>
          </div>
        </dl>
        <p>{{ messages.shell.unavailableActions }}</p>
        <button
          ref="helpClose"
          type="button"
          class="button-secondary"
          @click="shortcutsOpen = false"
        >
          {{ messages.shell.close }}
        </button>
      </DialogContent>
    </Dialog>
  </div>
</template>
