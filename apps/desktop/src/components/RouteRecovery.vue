<script setup lang="ts">
import { RouterLink } from "vue-router";
import { messages } from "../messages";
</script>

<template>
  <section class="state-panel page-stack" aria-labelledby="route-recovery-heading">
    <h1 id="route-recovery-heading" data-route-heading tabindex="-1">
      {{ messages.shell.notFoundTitle }}
    </h1>
    <p>{{ messages.shell.notFoundDescription }}</p>
    <div class="action-row">
      <RouterLink :to="{ name: 'projects' }">{{ messages.projects.library }}</RouterLink>
      <RouterLink :to="{ name: 'welcome' }">{{ messages.shell.start }}</RouterLink>
    </div>
  </section>
</template>
