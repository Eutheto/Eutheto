<script setup lang="ts">
import type { WorkforceSetupFacts } from "../api/generated-domain-pack-contracts";
import { messages } from "../messages";

defineProps<{ readonly facts: WorkforceSetupFacts }>();
const copy = messages.work;
</script>

<template>
  <dl class="field-stack">
    <div>
      <dt>{{ copy.fields.startDate }}</dt>
      <dd>{{ facts.planningDates.startDate }}</dd>
    </div>
    <div>
      <dt>{{ copy.fields.endDateExclusive }}</dt>
      <dd>{{ facts.planningDates.endDateExclusive }}</dd>
    </div>
    <div>
      <dt>{{ copy.timeZone }}</dt>
      <dd>{{ facts.settings.timeZone }}</dd>
    </div>
    <div>
      <dt>{{ copy.gapPolicy }}</dt>
      <dd>{{ copy.gapOptions[facts.settings.gapPolicy] }}</dd>
    </div>
    <div>
      <dt>{{ copy.overlapPolicy }}</dt>
      <dd>{{ copy.overlapOptions[facts.settings.overlapPolicy] }}</dd>
    </div>
    <div>
      <dt>{{ copy.horizonStart }}</dt>
      <dd>{{ facts.settings.horizon.start }}</dd>
    </div>
    <div>
      <dt>{{ copy.horizonEnd }}</dt>
      <dd>{{ facts.settings.horizon.end }}</dd>
    </div>
    <div>
      <dt>{{ copy.locale }}</dt>
      <dd>{{ facts.settings.locale }}</dd>
    </div>
    <div>
      <dt>{{ copy.units }}</dt>
      <dd>{{ facts.settings.units }}</dd>
    </div>
  </dl>
</template>
