<script setup lang="ts">
import { computed, onMounted, onUnmounted } from "vue";
import { RouterLink, RouterView } from "vue-router";

import { messages } from "./messages";
import { createProjectHomeController } from "./project-home";
import { useWorkspaceStore } from "./stores/workspace";

const home = createProjectHomeController();
const workspace = useWorkspaceStore();
const selectedProjectTitle = computed(() =>
  home.state.phase === "ready"
    ? home.state.projects.find(({ scenarioId }) => scenarioId === workspace.selectedProjectId)
        ?.title
    : undefined,
);

onMounted(async () => {
  await home.startEventListeners();
  await home.load();
});
onUnmounted(() => {
  void home.dispose();
});
</script>

<template>
  <main>
    <header class="app-header">
      <div>
        <p class="eyebrow">{{ messages.app.eyebrow }}</p>
        <h1>
          <RouterLink :to="{ name: 'projects' }">{{ messages.app.title }}</RouterLink>
        </h1>
      </div>
      <div>
        <p class="lede">{{ messages.app.lede }}</p>
        <p class="boundary-note">{{ messages.app.boundary }}</p>
      </div>
    </header>
    <p class="workspace-context">
      {{
        selectedProjectTitle
          ? messages.app.selectedProject(selectedProjectTitle)
          : messages.app.noSelection
      }}
    </p>

    <RouterView v-slot="{ Component }">
      <component :is="Component" :home="home" />
    </RouterView>
  </main>
</template>
