<script setup lang="ts">
import { computed, onScopeDispose, ref, shallowRef, watch } from "vue";
import {
  getAppPathsSummary,
  getLicenseInventory,
  type AppPathsSummaryDto,
  type LicenseInventoryV2,
} from "../api/generated";
import { formatNumber, messages } from "../messages";
import { safeMessage, type ProjectHomeController } from "../project-home";
import { Card } from "./ui/card";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "./ui/collapsible";

const props = defineProps<{ readonly home: ProjectHomeController; readonly locale?: string }>();
const inventory = shallowRef<LicenseInventoryV2 | null>(null);
const paths = shallowRef<AppPathsSummaryDto | null>(null);
const inventoryError = ref<string | null>(null);
const pathsError = ref<string | null>(null);
const loading = ref(false);
const search = ref("");
const page = ref(0);
const pageSize = 50;
let generation = 0;
let disposed = false;
const filtered = computed(() => {
  const query = search.value.trim().toLocaleLowerCase(props.locale);
  return (inventory.value?.packages ?? []).filter(
    (entry) =>
      !query ||
      [
        entry.name,
        entry.version,
        entry.ecosystem,
        entry.kind,
        entry.licenseConcluded,
        entry.source,
      ].some((value) => value.toLocaleLowerCase(props.locale).includes(query)),
  );
});
const pageCount = computed(() => Math.max(1, Math.ceil(filtered.value.length / pageSize)));
const visiblePackages = computed(() =>
  filtered.value.slice(page.value * pageSize, (page.value + 1) * pageSize),
);
watch([search, inventory], () => {
  page.value = 0;
});

async function load(): Promise<void> {
  const current = ++generation;
  loading.value = true;
  inventoryError.value = null;
  pathsError.value = null;
  // Independent offline queries preserve either successful category if the other fails.
  await Promise.all([
    getLicenseInventory()
      .then((response) => {
        if (!disposed && current === generation) inventory.value = response.result;
      })
      .catch((error: unknown) => {
        if (!disposed && current === generation) inventoryError.value = safeMessage(error);
      }),
    getAppPathsSummary()
      .then((response) => {
        if (!disposed && current === generation) paths.value = response.result;
      })
      .catch((error: unknown) => {
        if (!disposed && current === generation) pathsError.value = safeMessage(error);
      }),
  ]);
  if (!disposed && current === generation) loading.value = false;
}
void load();
onScopeDispose(() => {
  disposed = true;
  generation += 1;
});
</script>

<template>
  <section aria-labelledby="about-title">
    <header class="section-heading">
      <div>
        <p class="eyebrow">{{ messages.about.eyebrow }}</p>
        <h1 id="about-title" data-route-heading tabindex="-1">{{ messages.about.title }}</h1>
      </div>
      <button class="button-secondary" :disabled="loading" @click="load">
        {{ messages.about.refresh }}
      </button>
    </header>
    <p class="portable-intro">{{ messages.about.description }}</p>
    <p v-if="loading" role="status">{{ messages.about.loading }}</p>

    <Card as="section" aria-labelledby="about-paths-title">
      <h2 id="about-paths-title">{{ messages.about.configurationTitle }}</h2>
      <p class="boundary-note">{{ messages.about.configurationBoundary }}</p>
      <p v-if="pathsError" class="inline-alert" role="alert">
        {{ pathsError }} {{ paths ? messages.about.previousValues : messages.about.unavailable }}
      </p>
      <dl v-if="paths" class="metadata-list">
        <div>
          <dt>{{ messages.about.appData }}</dt>
          <dd>
            {{ paths.appDataConfigured ? messages.about.configured : messages.about.unconfigured }}
          </dd>
        </div>
        <div>
          <dt>{{ messages.about.cache }}</dt>
          <dd>
            {{ paths.cacheConfigured ? messages.about.configured : messages.about.unconfigured }}
          </dd>
        </div>
        <div>
          <dt>{{ messages.about.backup }}</dt>
          <dd>
            {{ paths.backupConfigured ? messages.about.configured : messages.about.unconfigured }}
          </dd>
        </div>
      </dl>
    </Card>

    <Card as="section" class="portable-section" aria-labelledby="about-inventory-title">
      <h2 id="about-inventory-title">{{ messages.about.inventoryTitle }}</h2>
      <p class="portable-intro">{{ messages.about.inventoryBoundary }}</p>
      <p v-if="inventoryError" class="inline-alert" role="alert">
        {{ inventoryError }}
        {{ inventory ? messages.about.previousValues : messages.about.unavailable }}
      </p>
      <template v-if="inventory">
        <dl class="metadata-list">
          <div>
            <dt>{{ messages.about.generatedBy }}</dt>
            <dd>
              <code>{{ inventory.generatedBy }}</code>
            </dd>
          </div>
          <div>
            <dt>{{ messages.about.inputs }}</dt>
            <dd>
              <ul>
                <li v-for="input in inventory.authoritativeInputs" :key="input">
                  <code>{{ input }}</code>
                </li>
              </ul>
            </dd>
          </div>
        </dl>
        <div class="stacked-form">
          <label for="about-search">{{ messages.about.search }}</label>
          <input
            id="about-search"
            v-model="search"
            data-view-search
            type="search"
            autocomplete="off"
            aria-describedby="about-search-help"
          />
          <p id="about-search-help" class="field-help">{{ messages.about.searchHelp }}</p>
        </div>
        <p role="status">
          {{
            messages.about.matchCount(
              formatNumber(filtered.length, locale),
              formatNumber(inventory.packages.length, locale),
            )
          }}
        </p>
        <p v-if="filtered.length === 0" class="empty-state">{{ messages.about.noMatches }}</p>
        <ul class="preview-list">
          <li
            v-for="(entry, index) in visiblePackages"
            :key="`${page}-${index}-${entry.ecosystem}-${entry.name}-${entry.version}-${entry.source}`"
          >
            <Collapsible>
              <CollapsibleTrigger class="button-secondary">
                {{ messages.about.packageLabel(entry.name, entry.version, entry.ecosystem) }} —
                {{ entry.licenseConcluded }}
              </CollapsibleTrigger>
              <CollapsibleContent>
                <dl class="metadata-list">
                  <div>
                    <dt>{{ messages.about.kind }}</dt>
                    <dd>
                      {{
                        entry.kind === "workspace"
                          ? messages.about.workspace
                          : messages.about.dependency
                      }}
                    </dd>
                  </div>
                  <div>
                    <dt>{{ messages.about.license }}</dt>
                    <dd class="identifier">{{ entry.licenseConcluded }}</dd>
                  </div>
                  <div>
                    <dt>{{ messages.about.source }}</dt>
                    <dd class="identifier">{{ entry.source }}</dd>
                  </div>
                  <div>
                    <dt>{{ messages.about.checksum }}</dt>
                    <dd class="identifier">
                      {{
                        entry.checksum
                          ? `${entry.checksum.algorithm}: ${entry.checksum.value}`
                          : messages.about.noChecksum
                      }}
                    </dd>
                  </div>
                </dl>
              </CollapsibleContent>
            </Collapsible>
          </li>
        </ul>
        <nav v-if="pageCount > 1" :aria-label="messages.about.pagination" class="action-row">
          <button class="button-secondary" :disabled="page === 0" @click="page -= 1">
            {{ messages.about.previous }}
          </button>
          <span role="status">{{
            messages.about.page(formatNumber(page + 1, locale), formatNumber(pageCount, locale))
          }}</span>
          <button class="button-secondary" :disabled="page + 1 >= pageCount" @click="page += 1">
            {{ messages.about.next }}
          </button>
        </nav>
      </template>
    </Card>
  </section>
</template>
