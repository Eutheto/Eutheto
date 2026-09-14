<script setup lang="ts">
import type { WorkforceSetupResolvedInterval } from "../api/generated-domain-pack-contracts";
import { messages } from "../messages";
import { formatWorkOffset } from "../work-display";

defineProps<{
  readonly interval: WorkforceSetupResolvedInterval;
  readonly timeZone: string;
}>();
const sides = ["startsAt", "endsAt"] as const;
const copy = messages.work;
</script>

<template>
  <div class="field-stack">
    <p>{{ copy.timeZone }}: {{ timeZone }}</p>
    <dl
      v-for="side in sides"
      :key="side"
      class="field-stack"
      :aria-label="side === 'startsAt' ? copy.table.start : copy.table.end"
    >
      <div>
        <dt>
          {{ side === "startsAt" ? copy.table.start : copy.table.end }} ·
          {{ copy.table.localIntent }}
        </dt>
        <dd>
          <code>{{ interval[side].local }}</code>
        </dd>
      </div>
      <div>
        <dt>{{ copy.table.offset }}</dt>
        <dd>{{ formatWorkOffset(interval[side].offsetSeconds) }}</dd>
      </div>
      <div>
        <dt>{{ copy.table.instant }}</dt>
        <dd>
          <code>{{ interval[side].instant }}</code>
        </dd>
      </div>
    </dl>
  </div>
</template>
