<script setup lang="ts">
import { useId } from "vue";

withDefaults(
  defineProps<{
    heading: string;
    description?: string;
    headingLevel?: 2 | 3 | 4;
  }>(),
  { headingLevel: 2, description: "" },
);
const headingId = useId();
</script>

<template>
  <section class="state-panel" :aria-labelledby="headingId">
    <component :is="`h${headingLevel.toString()}`" :id="headingId">{{ heading }}</component>
    <p v-if="description">{{ description }}</p>
    <div v-if="$slots.actions" class="action-row"><slot name="actions" /></div>
  </section>
</template>
