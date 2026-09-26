<script setup lang="ts">
import { computed, useId } from "vue";
import type {
  WorkforceSetupGenerationReview,
  WorkforceSetupGenerationRow,
} from "../api/generated-domain-pack-contracts";
import { formatNumber, messages } from "../messages";
import {
  formatWorkCoverage,
  formatWorkDuration,
  formatWorkOffset,
  formatWorkOrigin,
} from "../work-display";

const props = defineProps<{
  review: WorkforceSetupGenerationReview;
  timeZone: string;
  beforeTimeZone: string;
  locale?: string;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  inspect: [row: WorkforceSetupGenerationRow, side: "before" | "after"];
}>();
const copy = messages.work.generation;
const table = messages.work.table;
const headingId = useId();
const noChanges = computed(
  () =>
    !props.review.reconciliationRequired &&
    props.review.totalAdded === 0 &&
    props.review.totalChanged === 0 &&
    props.review.totalRemoved === 0,
);

function inspect(row: WorkforceSetupGenerationRow, side: "before" | "after"): void {
  if (!props.disabled && row[side] !== null) emit("inspect", row, side);
}
</script>

<template>
  <section :aria-labelledby="headingId" class="space-y-4">
    <h3 :id="headingId">{{ copy.heading }}</h3>
    <p>{{ copy.scope }}</p>
    <h4>{{ copy.totals }}</h4>
    <dl :aria-label="copy.totals" class="metadata-list">
      <div>
        <dt>{{ copy.added }}</dt>
        <dd>{{ formatNumber(review.totalAdded, locale) }}</dd>
      </div>
      <div>
        <dt>{{ copy.changed }}</dt>
        <dd>{{ formatNumber(review.totalChanged, locale) }}</dd>
      </div>
      <div>
        <dt>{{ copy.removed }}</dt>
        <dd>{{ formatNumber(review.totalRemoved, locale) }}</dd>
      </div>
    </dl>
    <p v-if="noChanges" class="quiet-state">{{ copy.noChanges }}</p>
    <p v-if="review.reconciliationRequired">{{ copy.reconciliationRequired }}</p>
    <p>{{ copy.matchingRows(formatNumber(review.page.totalItems, locale)) }}</p>
    <p v-if="review.page.items.length === 0" class="quiet-state">{{ copy.empty }}</p>
    <div
      v-else
      class="overflow-x-auto focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
      role="region"
      :aria-label="copy.heading"
      tabindex="0"
    >
      <table class="w-full min-w-3xl table-fixed border-collapse text-left text-sm">
        <caption class="p-2 text-left font-medium">
          {{
            copy.heading
          }}
        </caption>
        <thead>
          <tr>
            <th scope="col" class="w-1/5 p-2">{{ table.shift }}</th>
            <th scope="col" class="p-2">{{ copy.beforeInZone(beforeTimeZone) }}</th>
            <th scope="col" class="p-2">{{ copy.afterInZone(timeZone) }}</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="row in review.page.items"
            :key="row.shiftId"
            class="border-t border-line align-top"
          >
            <th scope="row" class="p-2 font-normal">
              <code class="break-all">{{ row.shiftId }}</code>
              <p class="font-medium">
                {{ row.change === null ? copy.unchanged : copy[row.change] }}
              </p>
            </th>
            <td class="p-2 break-words">
              <p v-if="row.before === null">{{ copy.missingBefore }}</p>
              <template v-else>
                <template v-if="row.before.kind === 'unresolved'">
                  <p class="font-medium">{{ copy.unresolved }}</p>
                  <p>{{ row.before.data.message }}</p>
                  <p>
                    <code>{{ row.before.data.issue }}</code>
                  </p>
                  <p v-if="row.before.data.fieldPath !== null">
                    {{ copy.field }} <code>{{ row.before.data.fieldPath }}</code>
                  </p>
                  <p>{{ table.origin }}: {{ formatWorkOrigin(row.before.data.origin) }}</p>
                </template>
                <dl v-else class="space-y-2">
                  <div>
                    <dt class="font-medium">{{ table.shift }}</dt>
                    <dd class="m-0">
                      <code>{{ row.before.data.shiftId }}</code>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.type }}</dt>
                    <dd class="m-0">
                      {{ row.before.data.assignmentTypeName }}
                      <code class="block">{{ row.before.data.assignmentTypeId }}</code>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.origin }}</dt>
                    <dd class="m-0">
                      {{ formatWorkOrigin(row.before.data.origin) }}
                      <p v-if="row.before.data.templateName !== null">
                        {{ table.templateValue(row.before.data.templateName) }}
                      </p>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.start }}</dt>
                    <dd class="m-0">
                      {{ table.localIntent }}
                      <code>{{ row.before.data.interval.startsAt.local }}</code>
                      <p>
                        {{ table.offset }}
                        {{ formatWorkOffset(row.before.data.interval.startsAt.offsetSeconds) }}
                      </p>
                      <p>
                        {{ table.instant }}
                        <code>{{ row.before.data.interval.startsAt.instant }}</code>
                      </p>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.end }}</dt>
                    <dd class="m-0">
                      {{ table.localIntent }}
                      <code>{{ row.before.data.interval.endsAt.local }}</code>
                      <p>
                        {{ table.offset }}
                        {{ formatWorkOffset(row.before.data.interval.endsAt.offsetSeconds) }}
                      </p>
                      <p>
                        {{ table.instant }}
                        <code>{{ row.before.data.interval.endsAt.instant }}</code>
                      </p>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.reportingDate }}</dt>
                    <dd class="m-0">{{ row.before.data.reportingDate }}</dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.location }}</dt>
                    <dd class="m-0">
                      <template v-if="row.before.data.locationId !== null">
                        <span v-if="row.before.data.locationName !== null">{{
                          row.before.data.locationName
                        }}</span>
                        <code class="block">{{ row.before.data.locationId }}</code>
                      </template>
                      <span v-else>{{ table.noLocation }}</span>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.coverage }}</dt>
                    <dd class="m-0">{{ formatWorkCoverage(row.before.data.coverage, locale) }}</dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.scheduled }}</dt>
                    <dd class="m-0 font-mono">
                      {{ formatWorkDuration(row.before.data.scheduled) }}
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.elapsed }}</dt>
                    <dd class="m-0 font-mono">{{ formatWorkDuration(row.before.data.elapsed) }}</dd>
                  </div>
                </dl>
                <button
                  type="button"
                  class="button-secondary mt-3"
                  :disabled="disabled"
                  :aria-label="`${copy.inspectBefore}: ${row.shiftId}`"
                  @click="inspect(row, 'before')"
                >
                  {{ copy.inspectBefore }}
                </button>
              </template>
            </td>
            <td class="p-2 break-words">
              <p v-if="row.after === null">{{ copy.missingAfter }}</p>
              <template v-else>
                <dl class="space-y-2">
                  <div>
                    <dt class="font-medium">{{ table.shift }}</dt>
                    <dd class="m-0">
                      <code>{{ row.after.shiftId }}</code>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.type }}</dt>
                    <dd class="m-0">
                      {{ row.after.assignmentTypeName }}
                      <code class="block">{{ row.after.assignmentTypeId }}</code>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.origin }}</dt>
                    <dd class="m-0">
                      {{ formatWorkOrigin(row.after.origin) }}
                      <p v-if="row.after.templateName !== null">
                        {{ table.templateValue(row.after.templateName) }}
                      </p>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.start }}</dt>
                    <dd class="m-0">
                      {{ table.localIntent }} <code>{{ row.after.interval.startsAt.local }}</code>
                      <p>
                        {{ table.offset }}
                        {{ formatWorkOffset(row.after.interval.startsAt.offsetSeconds) }}
                      </p>
                      <p>
                        {{ table.instant }} <code>{{ row.after.interval.startsAt.instant }}</code>
                      </p>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.end }}</dt>
                    <dd class="m-0">
                      {{ table.localIntent }} <code>{{ row.after.interval.endsAt.local }}</code>
                      <p>
                        {{ table.offset }}
                        {{ formatWorkOffset(row.after.interval.endsAt.offsetSeconds) }}
                      </p>
                      <p>
                        {{ table.instant }} <code>{{ row.after.interval.endsAt.instant }}</code>
                      </p>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.reportingDate }}</dt>
                    <dd class="m-0">{{ row.after.reportingDate }}</dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.location }}</dt>
                    <dd class="m-0">
                      <template v-if="row.after.locationId !== null">
                        <span v-if="row.after.locationName !== null">{{
                          row.after.locationName
                        }}</span>
                        <code class="block">{{ row.after.locationId }}</code>
                      </template>
                      <span v-else>{{ table.noLocation }}</span>
                    </dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.coverage }}</dt>
                    <dd class="m-0">{{ formatWorkCoverage(row.after.coverage, locale) }}</dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.scheduled }}</dt>
                    <dd class="m-0 font-mono">{{ formatWorkDuration(row.after.scheduled) }}</dd>
                  </div>
                  <div>
                    <dt class="font-medium">{{ table.elapsed }}</dt>
                    <dd class="m-0 font-mono">{{ formatWorkDuration(row.after.elapsed) }}</dd>
                  </div>
                </dl>
                <button
                  type="button"
                  class="button-secondary mt-3"
                  :disabled="disabled"
                  :aria-label="`${copy.inspectAfter}: ${row.shiftId}`"
                  @click="inspect(row, 'after')"
                >
                  {{ copy.inspectAfter }}
                </button>
              </template>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>
