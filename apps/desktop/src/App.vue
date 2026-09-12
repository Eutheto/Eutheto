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
  void home.dispose().catch(() => {
    // Native window teardown remains responsible for resources after the root has gone.
  });
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

    <section v-if="home.state.operation" class="state-panel" aria-labelledby="operation-label">
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

    <RouterView v-slot="{ Component }">
      <component :is="Component" :home="home" />
    </RouterView>
  </main>
</template>
