<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { eligibilityCellKey, type EligibilityReviewCell } from "../../eligibility-draft";
import { formatNumber, messages } from "../../messages";
const props = defineProps<{
  cells: readonly EligibilityReviewCell[];
  removable: boolean;
  locale?: string;
}>();
const emit = defineEmits<{ remove: [key: string] }>();
const page = ref(0),
  heading = ref<HTMLElement>();
const visible = computed(() => props.cells.slice(page.value * 50, (page.value + 1) * 50));
watch(
  () => props.cells,
  () => {
    page.value = Math.min(page.value, Math.max(0, Math.ceil(props.cells.length / 50) - 1));
  },
);
function remove(cell: EligibilityReviewCell): void {
  heading.value?.focus();
  emit("remove", eligibilityCellKey(cell.personId, cell.typeId));
}
</script>
<template>
  <section class="field-stack" :aria-label="messages.eligibilityUi.bulk.label">
    <h4 ref="heading" tabindex="-1">
      {{ messages.eligibilityUi.bulk.heading(formatNumber(props.cells.length, props.locale)) }}
    </h4>
    <div
      class="overflow-x-auto"
      tabindex="0"
      role="region"
      :aria-label="messages.eligibilityUi.bulk.region"
    >
      <table>
        <thead>
          <tr>
            <th scope="col">{{ messages.eligibilityUi.bulk.person }}</th>
            <th scope="col">{{ messages.eligibilityUi.bulk.assignmentType }}</th>
            <th scope="col">{{ messages.eligibilityUi.bulk.before }}</th>
            <th scope="col">{{ messages.eligibilityUi.bulk.desired }}</th>
            <th v-if="props.removable" scope="col">
              {{ messages.eligibilityUi.bulk.selection }}
            </th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="cell in visible" :key="eligibilityCellKey(cell.personId, cell.typeId)">
            <th scope="row">
              {{ cell.personName }}<br /><code>{{ cell.personId }}</code>
            </th>
            <td>
              {{ cell.typeName }}<br /><code>{{ cell.typeId }}</code>
            </td>
            <td>
              {{
                cell.before
                  ? messages.eligibilityUi.bulk.configured
                  : messages.eligibilityUi.bulk.notConfigured
              }}
            </td>
            <td>
              {{
                cell.after
                  ? messages.eligibilityUi.bulk.configured
                  : messages.eligibilityUi.bulk.notConfigured
              }}
            </td>
            <td v-if="props.removable">
              <button
                type="button"
                :aria-label="messages.eligibilityUi.bulk.remove(cell.personName, cell.typeName)"
                @click="remove(cell)"
              >
                {{ messages.eligibilityUi.bulk.removeButton }}
              </button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <div class="action-row">
      <button type="button" :disabled="page === 0" @click="page -= 1">
        {{ messages.eligibilityUi.bulk.previous }}
      </button>
      <button type="button" :disabled="(page + 1) * 50 >= props.cells.length" @click="page += 1">
        {{ messages.eligibilityUi.bulk.next }}
      </button>
    </div>
  </section>
</template>
