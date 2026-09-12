<script setup lang="ts">
import { computed } from "vue";
import type { AppliedMigrationDto, ValidationIssue } from "../api/generated";
import { formatDateTime, formatNumber, formatUnit, messages } from "../messages";
import type { PortableReview } from "../portable-workspace";

const props = defineProps<{
  readonly review: PortableReview;
  readonly warnings: readonly ValidationIssue[];
  readonly locale?: string | undefined;
}>();
const preview = computed(() =>
  props.review.kind === "import" || props.review.kind === "restore" ? props.review.preview : null,
);
const file = computed(() =>
  props.review.kind === "export" || props.review.kind === "backup" ? props.review.preview : null,
);
const unopened = computed(() =>
  props.review.kind === "unopened" ? props.review.preview.metadata : null,
);
const selection = computed(
  () => preview.value?.sourceBackupSelection ?? file.value?.backupSummary ?? null,
);
const selectionScope = computed(
  () =>
    preview.value?.sourceBackupSelection?.scope ??
    file.value?.backupSummary?.exclusionScope ??
    null,
);
function migrationKey(migration: AppliedMigrationDto): string {
  return `${migration.registry}\u0000${migration.name}\u0000${migration.fromVersion.toString()}\u0000${migration.toVersion.toString()}\u0000${migration.versionSpace ?? ""}\u0000${migration.subject?.packId ?? ""}\u0000${migration.subject?.scenarioId ?? ""}\u0000${(migration.subject?.revision ?? "").toString()}`;
}
</script>

<template>
  <div class="portable-evidence">
    <template v-if="preview">
      <h3>{{ preview.title }}</h3>
      <dl class="preview-metadata">
        <div>
          <dt>{{ messages.portable.source }}</dt>
          <dd>{{ preview.sourceApplication.name }} {{ preview.sourceApplication.version }}</dd>
        </div>
        <div>
          <dt>{{ messages.portable.created }}</dt>
          <dd>
            <time :datetime="preview.createdAt">{{
              formatDateTime(preview.createdAt, locale)
            }}</time>
          </dd>
        </div>
        <div>
          <dt>{{ messages.portable.bundleIdentity }}</dt>
          <dd>
            <code>{{ preview.bundleId }}</code> · {{ preview.bundleKind }}
          </dd>
        </div>
        <div>
          <dt>{{ messages.portable.versions }}</dt>
          <dd>
            {{
              messages.portable.versionPair(
                formatNumber(preview.sourceFormatVersion, locale),
                formatNumber(preview.sourceSchemaVersion, locale),
              )
            }}
          </dd>
        </div>
        <div>
          <dt>{{ messages.portable.libraryRevision }}</dt>
          <dd>{{ formatNumber(preview.libraryRevision, locale) }}</dd>
        </div>
        <div>
          <dt>{{ messages.portable.counts }}</dt>
          <dd>
            <ul>
              <li>
                {{ messages.portable.projectCount(formatNumber(preview.counts.scenarios, locale)) }}
              </li>
              <li>
                {{
                  messages.portable.revisionCount(
                    formatNumber(preview.counts.scenarioRevisions, locale),
                  )
                }}
              </li>
              <li>
                {{ messages.portable.resultCount(formatNumber(preview.counts.results, locale)) }}
              </li>
              <li>
                {{
                  messages.portable.sharedCount(formatNumber(preview.counts.sharedRecords, locale))
                }}
              </li>
              <li>
                {{
                  messages.portable.preferenceCount(
                    formatNumber(preview.counts.preferences, locale),
                  )
                }}
              </li>
              <li>
                {{ messages.portable.assetCount(formatNumber(preview.counts.assets, locale)) }}
              </li>
            </ul>
          </dd>
        </div>
        <div>
          <dt>{{ messages.portable.includedSections }}</dt>
          <dd>{{ preview.includedSections.join(", ") || messages.portable.none }}</dd>
        </div>
        <div>
          <dt>{{ messages.portable.excludedSections }}</dt>
          <dd>{{ preview.excludedSections.join(", ") || messages.portable.none }}</dd>
        </div>
      </dl>
      <section>
        <h4>{{ messages.portable.capabilities }}</h4>
        <p v-if="preview.requiredCapabilities.length === 0">{{ messages.portable.none }}</p>
        <ul v-else>
          <li
            v-for="capability in preview.requiredCapabilities"
            :key="`${capability.id}:${capability.version}`"
          >
            <code>{{ capability.id }}</code> ·
            {{ messages.portable.version(formatNumber(capability.version, locale)) }}
          </li>
        </ul>
      </section>
      <section>
        <h4>{{ messages.portable.extensions }}</h4>
        <p v-if="preview.preservedExtensions.length === 0">{{ messages.portable.none }}</p>
        <ul v-else>
          <li v-for="extension in preview.preservedExtensions" :key="extension">
            <code>{{ extension }}</code>
          </li>
        </ul>
      </section>
      <section>
        <h4>{{ messages.portable.migrations }}</h4>
        <p v-if="preview.appliedMigrations.length === 0">{{ messages.portable.none }}</p>
        <ul v-else>
          <li v-for="migration in preview.appliedMigrations" :key="migrationKey(migration)">
            {{ migration.registry }} · {{ migration.name }} ·
            {{ formatNumber(migration.fromVersion, locale) }} →
            {{ formatNumber(migration.toVersion, locale) }}
            <span v-if="migration.versionSpace">
              · {{ messages.portable.versionSpace(migration.versionSpace) }}</span
            >
            <span v-if="migration.subject">
              · {{ messages.portable.pack }} <code>{{ migration.subject.packId }}</code> ·
              {{ messages.portable.scenario }} <code>{{ migration.subject.scenarioId }}</code> ·
              {{
                messages.portable.revision(formatNumber(migration.subject.revision, locale))
              }}</span
            >
          </li>
        </ul>
      </section>
      <section v-if="preview.omittedAssets.length">
        <h4>{{ messages.portable.omissions }}</h4>
        <p>{{ messages.portable.reconnection }}</p>
        <ul>
          <li v-for="asset in preview.omittedAssets" :key="asset.assetId">
            <strong>{{ asset.assetId }}</strong> — {{ asset.reason }};
            {{ asset.originalMediaType }}, {{ formatUnit(asset.originalSize, "byte", locale) }}
            <small
              >{{ asset.format }} ·
              {{ messages.portable.version(formatNumber(asset.version, locale)) }}</small
            >
            <code class="identifier">{{ asset.contentSha256 }}</code>
          </li>
        </ul>
      </section>
      <section>
        <h4>{{ messages.portable.settingChanges }}</h4>
        <p v-if="!preview.settingsChanged.length && !preview.settingsRemoved.length">
          {{ messages.portable.noSettingChanges }}
        </p>
        <template v-if="preview.settingsChanged.length">
          <h5>{{ messages.portable.changed }}</h5>
          <ul>
            <li v-for="key in preview.settingsChanged" :key="key">
              <code>{{ key }}</code>
            </li>
          </ul>
        </template>
        <template v-if="preview.settingsRemoved.length">
          <h5>{{ messages.portable.removed }}</h5>
          <ul>
            <li v-for="key in preview.settingsRemoved" :key="key">
              <code>{{ key }}</code>
            </li>
          </ul>
        </template>
      </section>
      <section v-if="review.kind === 'restore' && review.mode === 'replace-library'">
        <h4>{{ messages.portable.removedProjects }}</h4>
        <p v-if="!preview.removedScenarios.length">{{ messages.portable.noRemovedProjects }}</p>
        <ul v-else class="preview-list">
          <li v-for="scenario in preview.removedScenarios" :key="scenario.scenarioId">
            <span
              ><strong>{{ scenario.title }}</strong
              ><small
                >{{ scenario.scenarioId }} ·
                {{ messages.portable.revision(formatNumber(scenario.revision, locale)) }} ·
                {{
                  scenario.archived ? messages.portable.archived : messages.portable.active
                }}</small
              ></span
            >
          </li>
        </ul>
        <h4>{{ messages.portable.removedSupplemental }}</h4>
        <p v-if="!preview.removedSupplemental.length">
          {{ messages.portable.noRemovedSupplemental }}
        </p>
        <ul v-else class="preview-list">
          <li
            v-for="identity in preview.removedSupplemental"
            :key="`${identity.section}:${identity.key}`"
          >
            <strong>{{ identity.key }}</strong
            ><small>{{ identity.section }}</small>
          </li>
        </ul>
      </section>
    </template>

    <template v-if="file">
      <h3>{{ file.title }}</h3>
      <dl class="preview-metadata">
        <div>
          <dt>{{ messages.portable.fileSize }}</dt>
          <dd>{{ formatUnit(file.byteLength, "byte", locale) }}</dd>
        </div>
        <div>
          <dt>{{ messages.portable.libraryRevision }}</dt>
          <dd>{{ formatNumber(file.libraryRevision, locale) }}</dd>
        </div>
        <div v-if="file.currentRevision !== null">
          <dt>{{ messages.portable.scenarioRevision }}</dt>
          <dd>{{ formatNumber(file.currentRevision, locale) }}</dd>
        </div>
        <div v-if="review.kind === 'export'">
          <dt>{{ messages.portable.scenario }}</dt>
          <dd>
            <code>{{ review.scenarioId }}</code>
          </dd>
        </div>
        <div>
          <dt>{{ messages.portable.digest }}</dt>
          <dd>
            <code class="identifier">{{ file.digest }}</code>
          </dd>
        </div>
      </dl>
      <p v-if="review.kind === 'export'">{{ messages.portable.exportEvidenceBoundary }}</p>
    </template>

    <section v-if="selection">
      <h4>{{ messages.portable.sourceSelection }}</h4>
      <dl class="preview-metadata">
        <div>
          <dt>{{ messages.portable.results }}</dt>
          <dd>
            {{ selection.includeResults ? messages.portable.included : messages.portable.excluded }}
          </dd>
        </div>
        <div>
          <dt>{{ messages.portable.assetSelection }}</dt>
          <dd>{{ selection.assetSelection }}</dd>
        </div>
        <div>
          <dt>{{ messages.portable.omittedCount }}</dt>
          <dd>{{ formatNumber(selection.excludedAssetCount, locale) }}</dd>
        </div>
        <div v-if="selection.excludedAssetIds.length">
          <dt>{{ messages.portable.omittedIds }}</dt>
          <dd>{{ selection.excludedAssetIds.join(", ") }}</dd>
        </div>
        <div v-if="selectionScope">
          <dt>{{ messages.portable.scope }}</dt>
          <dd>{{ selectionScope }}</dd>
        </div>
        <div v-if="selection.thresholdBytes !== null">
          <dt>{{ messages.portable.threshold }}</dt>
          <dd>
            {{ formatUnit(selection.thresholdBytes, "byte", locale) }} ·
            {{
              selection.thresholdVersion === null
                ? messages.portable.unknown
                : messages.portable.version(formatNumber(selection.thresholdVersion, locale))
            }}
          </dd>
        </div>
      </dl>
      <h5>{{ messages.portable.fixedExclusions }}</h5>
      <ul>
        <li v-for="exclusion in selection.fixedExclusions" :key="exclusion">
          {{ messages.portable.fixedExclusionLabels[exclusion] }}
        </li>
      </ul>
    </section>

    <template v-if="unopened">
      <h3>{{ unopened.title ?? messages.portable.unknownTitle }}</h3>
      <p>{{ messages.portable.unopenedBoundary }}</p>
      <dl class="preview-metadata">
        <div>
          <dt>{{ messages.portable.fileSha256 }}</dt>
          <dd>
            <code class="identifier">{{ unopened.fileSha256 }}</code>
          </dd>
        </div>
        <div>
          <dt>{{ messages.portable.format }}</dt>
          <dd>{{ unopened.format }}</dd>
        </div>
        <div>
          <dt>{{ messages.portable.versions }}</dt>
          <dd>
            {{
              messages.portable.versionPair(
                formatNumber(unopened.formatVersion, locale),
                formatNumber(unopened.portableSchemaVersion, locale),
              )
            }}
          </dd>
        </div>
        <div>
          <dt>{{ messages.portable.bundleKind }}</dt>
          <dd>{{ unopened.bundleKind ?? messages.portable.unknown }}</dd>
        </div>
      </dl>
      <h4>{{ messages.portable.capabilities }}</h4>
      <p v-if="!unopened.requiredCapabilities.length">{{ messages.portable.none }}</p>
      <ul v-else>
        <li
          v-for="capability in unopened.requiredCapabilities"
          :key="`${capability.id}:${capability.version}`"
        >
          <code>{{ capability.id }}</code> ·
          {{ messages.portable.version(formatNumber(capability.version, locale)) }}
        </li>
      </ul>
      <h4>{{ messages.portable.bundleScenarios }}</h4>
      <ul class="preview-list">
        <li v-for="scenario in unopened.scenarios" :key="scenario.path">
          <dl class="preview-metadata">
            <div>
              <dt>{{ messages.portable.archiveEntry }}</dt>
              <dd>
                <code>{{ scenario.path }}</code>
              </dd>
            </div>
            <div>
              <dt>{{ messages.portable.scenario }}</dt>
              <dd>{{ scenario.scenarioId ?? messages.portable.unknown }}</dd>
            </div>
            <div>
              <dt>{{ messages.portable.pack }}</dt>
              <dd>{{ scenario.packId ?? messages.portable.unknown }}</dd>
            </div>
            <div>
              <dt>{{ messages.portable.internalPackVersion }}</dt>
              <dd>
                {{
                  scenario.internalPackSchemaVersion === null
                    ? messages.portable.unknown
                    : formatNumber(scenario.internalPackSchemaVersion, locale)
                }}
              </dd>
            </div>
            <div>
              <dt>{{ messages.portable.portablePackVersion }}</dt>
              <dd>
                {{
                  scenario.portablePackSchemaVersion === null
                    ? messages.portable.unknown
                    : formatNumber(scenario.portablePackSchemaVersion, locale)
                }}
              </dd>
            </div>
          </dl>
        </li>
      </ul>
    </template>

    <section v-if="warnings.length">
      <h4>{{ messages.portable.warnings }}</h4>
      <ul>
        <li v-for="(warning, index) in warnings" :key="`${warning.code}:${index}`">
          <strong>{{ warning.code }}</strong> — {{ warning.message }}
        </li>
      </ul>
    </section>
  </div>
</template>
