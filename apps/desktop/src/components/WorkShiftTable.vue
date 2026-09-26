<script setup lang="ts">
import type { WorkforceSetupWorkShift } from "../api/generated-domain-pack-contracts";
import { messages } from "../messages";
import {
  formatWorkCoverage,
  formatWorkDuration,
  formatWorkOffset,
  formatWorkOrigin,
} from "../work-display";

const props = defineProps<{
  rows: readonly WorkforceSetupWorkShift[];
  timeZone: string;
  locale?: string;
  disabled?: boolean;
}>();

const emit = defineEmits<{ inspect: [shiftId: string] }>();
const copy = messages.work.table;

function inspect(shiftId: string): void {
  if (!props.disabled) emit("inspect", shiftId);
}
</script>

<template>
  <p v-if="rows.length === 0" class="quiet-state">{{ copy.empty }}</p>
  <div
    v-else
    class="overflow-x-auto focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
    role="region"
    :aria-label="copy.caption"
    tabindex="0"
  >
    <table class="w-full min-w-3xl border-collapse text-left text-sm">
      <caption class="p-2 text-left font-medium">
        {{
          copy.captionForZone(timeZone)
        }}
      </caption>
      <thead>
        <tr>
          <th scope="col" class="p-2">{{ copy.shift }}</th>
          <th scope="col" class="p-2">{{ copy.type }}</th>
          <th scope="col" class="p-2">{{ copy.origin }}</th>
          <th scope="col" class="p-2">{{ copy.start }}</th>
          <th scope="col" class="p-2">{{ copy.end }}</th>
          <th scope="col" class="p-2">{{ copy.location }}</th>
          <th scope="col" class="p-2">{{ copy.coverage }}</th>
          <th scope="col" class="p-2">{{ copy.scheduled }}</th>
          <th scope="col" class="p-2">{{ copy.elapsed }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="shift in rows" :key="shift.shiftId" class="border-t border-line align-top">
          <th scope="row" class="p-2 font-normal">
            <code class="break-all">{{ shift.shiftId }}</code>
            <p>{{ copy.reportingDateValue(shift.reportingDate) }}</p>
            <button
              type="button"
              class="button-secondary"
              :disabled="disabled"
              :aria-label="
                copy.inspectShift(
                  shift.assignmentTypeName,
                  shift.interval.startsAt.local,
                  shift.shiftId,
                )
              "
              @click="inspect(shift.shiftId)"
            >
              {{ copy.inspect }}
            </button>
          </th>
          <td class="p-2">
            <p>{{ shift.assignmentTypeName }}</p>
            <code class="break-all">{{ shift.assignmentTypeId }}</code>
          </td>
          <td class="p-2 break-words">
            <p>{{ formatWorkOrigin(shift.origin) }}</p>
            <p v-if="shift.templateName !== null">{{ copy.templateValue(shift.templateName) }}</p>
          </td>
          <td class="p-2">
            <p>
              {{ copy.localIntent }} <code>{{ shift.interval.startsAt.local }}</code>
            </p>
            <p>{{ copy.offset }} {{ formatWorkOffset(shift.interval.startsAt.offsetSeconds) }}</p>
            <p>
              {{ copy.instant }} <code>{{ shift.interval.startsAt.instant }}</code>
            </p>
          </td>
          <td class="p-2">
            <p>
              {{ copy.localIntent }} <code>{{ shift.interval.endsAt.local }}</code>
            </p>
            <p>{{ copy.offset }} {{ formatWorkOffset(shift.interval.endsAt.offsetSeconds) }}</p>
            <p>
              {{ copy.instant }} <code>{{ shift.interval.endsAt.instant }}</code>
            </p>
          </td>
          <td class="p-2">
            <template v-if="shift.locationId !== null">
              <p v-if="shift.locationName !== null">{{ shift.locationName }}</p>
              <code class="break-all">{{ shift.locationId }}</code>
            </template>
            <span v-else>{{ copy.noLocation }}</span>
          </td>
          <td class="p-2">{{ formatWorkCoverage(shift.coverage, locale) }}</td>
          <td class="p-2 font-mono">{{ formatWorkDuration(shift.scheduled) }}</td>
          <td class="p-2 font-mono">{{ formatWorkDuration(shift.elapsed) }}</td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
