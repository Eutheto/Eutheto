<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { useRoute } from "vue-router";
import type { CollisionAction, Revision, SupplementalCollisionAction } from "../api/generated";
import { formatNumber, messages } from "../messages";
import {
  createPortableWorkspaceController,
  reviewedCollisionPlan,
  type PortableMode,
} from "../portable-workspace";
import {
  scenarioRevisionOutcome,
  supplementalIdentityKey,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";
import PortableEvidence from "./PortableEvidence.vue";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./ui/dialog";

const props = defineProps<{
  readonly home: ProjectHomeController;
  readonly mode: PortableMode;
  readonly project?: ProjectSummary;
  readonly locale?: string;
  readonly libraryRevision?: Revision | null;
}>();
const emit = defineEmits<{
  exported: [outcome: { artifactName: string }];
  exportCancelled: [];
  exportFailed: [];
}>();
const route = useRoute();
const workspace = createPortableWorkspaceController(props.home, () => props, {
  exported: (outcome) => {
    emit("exported", outcome);
  },
  exportCancelled: () => {
    emit("exportCancelled");
  },
  exportFailed: () => {
    emit("exportFailed");
  },
});
const state = workspace.state;
const includeResults = ref(true);
const includeAssets = ref(true);
const backupTitle = ref("");
const restoreMode = ref<"add-backup" | "replace-library">("add-backup");
const origin = ref<"userSelected" | "safetyBackups">("userSelected");
const scenarios = ref<Record<string, CollisionAction | "">>({});
const supplemental = ref<Record<string, SupplementalCollisionAction | "">>({});
const confirmation = ref<"reviewed" | "replace" | "bypass" | null>(null);
const replacementAcknowledged = ref(false);
const bypassPhrase = ref("");
const reviewButton = ref<HTMLButtonElement | null>(null);
const backButton = ref<HTMLButtonElement | null>(null);
const heading = ref<HTMLElement | null>(null);
const reviewHeading = ref<HTMLElement | null>(null);
let lifetime = 0;
let active = true;
const busy = computed(() => state.pending || props.home.state.busyAction !== null);
const mutation = computed(() =>
  state.review?.kind === "import" || state.review?.kind === "restore" ? state.review : null,
);
const replacing = computed(
  () => mutation.value?.kind === "restore" && mutation.value.mode === "replace-library",
);
const plan = computed(() =>
  mutation.value
    ? reviewedCollisionPlan(
        mutation.value.preview,
        scenarios.value,
        supplemental.value,
        replacing.value,
      )
    : null,
);
const scenarioRows = computed(
  () =>
    mutation.value?.preview.scenarios.map((scenario) => ({
      scenario,
      outcome:
        scenario.collides && !replacing.value && !scenarios.value[scenario.scenarioId]
          ? null
          : scenarioRevisionOutcome(
              scenario,
              scenarios.value[scenario.scenarioId] || undefined,
              replacing.value,
            ),
    })) ?? [],
);
const dirty = computed(
  () =>
    state.review !== null ||
    backupTitle.value !== "" ||
    !includeResults.value ||
    !includeAssets.value ||
    restoreMode.value !== "add-backup" ||
    origin.value !== "userSelected",
);
const title = computed(() =>
  props.mode === "import"
    ? messages.portable.importTitle
    : props.mode === "export"
      ? messages.portable.exportTitle
      : messages.portable.backupRestoreTitle,
);
const confirmationTitle = computed(() =>
  confirmation.value === "bypass"
    ? messages.portable.bypassTitle
    : confirmation.value === "replace"
      ? messages.portable.replaceTitle
      : messages.portable.applyTitle,
);
const confirmationDescription = computed(() =>
  confirmation.value === "bypass"
    ? messages.portable.bypassDescription
    : confirmation.value === "replace"
      ? messages.portable.replaceDescription
      : messages.portable.applyDescription,
);

function resetDrafts(): void {
  includeResults.value = true;
  includeAssets.value = true;
  backupTitle.value = "";
  restoreMode.value = "add-backup";
  origin.value = "userSelected";
  scenarios.value = {};
  supplemental.value = {};
  confirmation.value = null;
  replacementAcknowledged.value = false;
  bypassPhrase.value = "";
}
function leave(): void {
  active = false;
  lifetime += 1;
  resetDrafts();
  workspace.leave();
}
function discardReview(): void {
  if (busy.value) return;
  workspace.discardReview();
  resetDrafts();
  heading.value?.focus();
}
function openConfirmation(): void {
  if (busy.value || state.stale || !plan.value) return;
  replacementAcknowledged.value = false;
  bypassPhrase.value = "";
  confirmation.value = replacing.value ? (state.safetyFailure ? "bypass" : "replace") : "reviewed";
}
function back(): void {
  if (busy.value) return;
  confirmation.value = null;
  replacementAcknowledged.value = false;
  bypassPhrase.value = "";
}
async function confirm(): Promise<void> {
  const kind = confirmation.value;
  if (!kind || busy.value || !plan.value || state.stale) return;
  if ((kind === "replace" || kind === "bypass") && !replacementAcknowledged.value) return;
  if (kind === "bypass" && bypassPhrase.value !== "REPLACE WITHOUT BACKUP") return;
  const captured = lifetime;
  const applied = await workspace.apply(
    scenarios.value,
    supplemental.value,
    kind,
    bypassPhrase.value,
  );
  if (!active || lifetime !== captured) return;
  // A failed first safety attempt closes this confirmation. Bypass requires opening a new one.
  confirmation.value = null;
  replacementAcknowledged.value = false;
  bypassPhrase.value = "";
  if (applied) resetDrafts();
}
async function save(): Promise<void> {
  const captured = lifetime;
  const saved = await workspace.save();
  if (active && lifetime === captured && saved) resetDrafts();
}
async function restoreFocus(event: Event): Promise<void> {
  event.preventDefault();
  const captured = lifetime;
  await nextTick();
  if (active && lifetime === captured) (reviewButton.value ?? heading.value)?.focus();
}
function focusBack(event: Event): void {
  event.preventDefault();
  if (active) backButton.value?.focus();
}
watch(
  () => [props.libraryRevision, props.project?.scenarioId, props.project?.revision],
  () => {
    workspace.refreshBinding();
  },
  { flush: "sync" },
);
watch(
  () => state.review?.preview.previewId,
  async () => {
    scenarios.value = {};
    supplemental.value = {};
    confirmation.value = null;
    replacementAcknowledged.value = false;
    bypassPhrase.value = "";
    const captured = lifetime;
    await nextTick();
    if (active && lifetime === captured) (reviewHeading.value ?? heading.value)?.focus();
  },
);
watch(
  () => route.fullPath,
  () => {
    // Parameter/query navigation may reuse this component after the guard retired its old owner.
    lifetime += 1;
    active = true;
    resetDrafts();
    workspace.enter();
  },
  { flush: "sync" },
);
onBeforeUnmount(leave);
</script>

<template>
  <section
    class="portable-section"
    aria-labelledby="portable-workspace-heading"
    :aria-busy="state.pending"
  >
    <RouteLeaveGuard :home="home" :dirty="dirty" :pending="state.pending" :discard="leave" />
    <div class="section-heading">
      <div>
        <p class="eyebrow">{{ messages.portable.eyebrow }}</p>
        <component
          :is="mode === 'export' ? 'h2' : 'h1'"
          id="portable-workspace-heading"
          ref="heading"
          data-route-heading
          tabindex="-1"
        >
          {{ title }}
        </component>
      </div>
    </div>
    <p class="portable-intro">{{ messages.portable.nativeBoundary }}</p>
    <p v-if="mode === 'export'">{{ messages.portable.editableNotResult }}</p>
    <p v-if="mode === 'export'">{{ messages.portable.exportUnconfirmed }}</p>
    <p v-if="mode === 'backup-restore'">{{ messages.portable.recoveryAvailability }}</p>
    <p v-if="state.notice" role="status">{{ state.notice }}</p>
    <p v-if="state.error" class="inline-alert" role="alert">{{ state.error }}</p>
    <div v-if="state.pending" class="state-panel" role="status">
      <p>
        {{
          home.state.operation?.settled
            ? messages.operations.refreshing
            : home.state.operation?.phase
              ? messages.operations.phases[home.state.operation.phase]
              : messages.operations.pending
        }}
      </p>
      <p v-if="home.state.operation?.cancel && !home.state.operation.settled">
        {{ messages.portable.chooserCancellation }}
      </p>
      <button
        v-if="home.state.operation?.cancel && !home.state.operation.settled"
        type="button"
        class="button-secondary"
        :disabled="home.state.operation?.cancellationRequested"
        @click="home.cancelOperation()"
      >
        {{
          home.state.operation?.cancellationRequested
            ? messages.operations.cancelling
            : messages.operations.cancel
        }}
      </button>
    </div>

    <template v-if="!state.review">
      <template v-if="mode === 'import'">
        <form
          class="stacked-form"
          @submit.prevent="workspace.previewImport(includeResults, includeAssets)"
        >
          <h2>{{ messages.portable.importReviewTitle }}</h2>
          <p>{{ messages.portable.importDescription }}</p>
          <fieldset :disabled="busy">
            <legend>{{ messages.portable.portableSections }}</legend>
            <label class="choice-row"
              ><input v-model="includeResults" type="checkbox" /><span
                ><strong>{{ messages.portable.includeResults }}</strong
                ><small>{{ messages.portable.includeResultsHelp }}</small></span
              ></label
            >
            <label class="choice-row"
              ><input v-model="includeAssets" type="checkbox" /><span
                ><strong>{{ messages.portable.includeAssets }}</strong
                ><small>{{ messages.portable.includeAssetsHelp }}</small></span
              ></label
            >
          </fieldset>
          <button type="submit" :disabled="busy">{{ messages.portable.chooseImport }}</button>
        </form>
        <section class="preview">
          <h2>{{ messages.portable.inspectTitle }}</h2>
          <p>{{ messages.portable.inspectDescription }}</p>
          <button
            type="button"
            class="button-secondary"
            :disabled="busy"
            @click="workspace.inspect()"
          >
            {{ messages.portable.inspect }}
          </button>
        </section>
      </template>
      <template v-else-if="mode === 'export'">
        <p v-if="!project" class="inline-alert" role="alert">
          {{ messages.portable.projectUnavailable }}
        </p>
        <template v-else>
          <h2>{{ project.title }}</h2>
          <p>
            <code>{{ project.scenarioId }}</code> ·
            {{ messages.portable.revision(formatNumber(project.revision, locale)) }}
          </p>
          <button type="button" :disabled="busy" @click="workspace.previewExport()">
            {{ messages.portable.previewExport }}
          </button>
        </template>
      </template>
      <div v-else class="portable-grid">
        <form class="stacked-form" @submit.prevent="workspace.previewBackup(backupTitle)">
          <h2>{{ messages.portable.backupTitle }}</h2>
          <label for="portable-backup-title">{{ messages.portable.backupName }}</label>
          <input
            id="portable-backup-title"
            v-model="backupTitle"
            :disabled="busy"
            required
            autocomplete="off"
          />
          <button type="submit" :disabled="busy || !backupTitle.trim()">
            {{ messages.portable.previewBackup }}
          </button>
        </form>
        <form class="stacked-form" @submit.prevent="workspace.previewRestore(restoreMode, origin)">
          <h2>{{ messages.portable.restoreTitle }}</h2>
          <fieldset :disabled="busy">
            <legend>{{ messages.portable.restoreBehavior }}</legend>
            <label class="choice-row"
              ><input v-model="restoreMode" type="radio" value="add-backup" /><span
                ><strong>{{ messages.portable.add }}</strong
                ><small>{{ messages.portable.addHelp }}</small></span
              ></label
            >
            <label class="choice-row"
              ><input v-model="restoreMode" type="radio" value="replace-library" /><span
                ><strong>{{ messages.portable.replace }}</strong
                ><small>{{ messages.portable.replaceHelp }}</small></span
              ></label
            >
          </fieldset>
          <fieldset :disabled="busy">
            <legend>{{ messages.portable.restoreOrigin }}</legend>
            <label class="choice-row"
              ><input v-model="origin" type="radio" value="userSelected" /><span>{{
                messages.portable.userSelected
              }}</span></label
            >
            <label class="choice-row"
              ><input v-model="origin" type="radio" value="safetyBackups" /><span
                ><strong>{{ messages.portable.safetyBackups }}</strong
                ><small>{{ messages.portable.safetyBackupsHelp }}</small></span
              ></label
            >
          </fieldset>
          <button type="submit" :disabled="busy">{{ messages.portable.chooseRestore }}</button>
        </form>
      </div>
    </template>

    <section v-else class="preview" aria-labelledby="portable-review-heading">
      <h2 id="portable-review-heading" ref="reviewHeading" tabindex="-1">
        {{ messages.portable.reviewTitle }}
      </h2>
      <p v-if="state.stale" class="inline-alert" role="alert">{{ messages.portable.stale }}</p>
      <p v-if="replacing" class="danger-note">{{ messages.portable.replaceDescription }}</p>
      <p v-if="state.review.kind === 'restore' && state.review.origin === 'safetyBackups'">
        {{ messages.portable.recoveryReview }}
      </p>
      <PortableEvidence :review="state.review" :warnings="state.warnings" :locale="locale" />
      <template v-if="mutation">
        <h3>{{ messages.portable.projectsToImport }}</h3>
        <ul class="preview-list">
          <li v-for="{ scenario, outcome } in scenarioRows" :key="scenario.scenarioId">
            <span
              ><strong>{{ scenario.title }}</strong
              ><small>{{ scenario.scenarioId }}</small
              ><small>{{
                messages.portable.sourceRevision(formatNumber(scenario.sourceRevision, locale))
              }}</small></span
            >
            <template v-if="scenario.collides && !replacing">
              <label :for="`portable-collision-${scenario.scenarioId}`">{{
                messages.portable.collisionAction
              }}</label>
              <select
                :id="`portable-collision-${scenario.scenarioId}`"
                :value="scenarios[scenario.scenarioId] ?? ''"
                :disabled="busy || state.stale"
                @change="
                  scenarios[scenario.scenarioId] = ($event.target as HTMLSelectElement)
                    .value as CollisionAction
                "
              >
                <option value="" disabled>{{ messages.portable.chooseAction }}</option>
                <option value="create-copy">{{ messages.portable.createCopy }}</option>
                <option value="replace">{{ messages.portable.replaceExisting }}</option>
                <option value="skip">{{ messages.portable.skip }}</option>
              </select>
            </template>
            <p v-else class="status-label">
              {{
                replacing ? messages.portable.includedByReplacement : messages.portable.newProject
              }}
            </p>
            <template v-if="outcome">
              <p v-if="outcome.revision !== null">
                {{ messages.portable.outcomeRevision(formatNumber(outcome.revision, locale)) }}
              </p>
              <p v-else>{{ messages.portable.skippedProject }}</p>
              <p v-if="outcome.warning" class="warning">
                <strong>{{ messages.portable.revisionWarning }}</strong> {{ outcome.warning }}
              </p>
            </template>
          </li>
        </ul>
        <section v-if="!replacing && mutation.preview.supplementalCollisions.length">
          <h3>{{ messages.portable.supplementalCollisions }}</h3>
          <ul class="preview-list">
            <li
              v-for="(identity, index) in mutation.preview.supplementalCollisions"
              :key="supplementalIdentityKey(identity)"
            >
              <span
                ><strong>{{ identity.key }}</strong
                ><small>{{ identity.section }}</small></span
              >
              <label :for="`portable-supplemental-${index}`">{{
                messages.portable.supplementalAction
              }}</label>
              <select
                :id="`portable-supplemental-${index}`"
                :value="supplemental[supplementalIdentityKey(identity)] ?? ''"
                :disabled="busy || state.stale"
                @change="
                  supplemental[supplementalIdentityKey(identity)] = (
                    $event.target as HTMLSelectElement
                  ).value as SupplementalCollisionAction
                "
              >
                <option value="" disabled>{{ messages.portable.chooseAction }}</option>
                <option value="skip">{{ messages.portable.skip }}</option>
                <option value="replace">{{ messages.portable.replaceExisting }}</option>
              </select>
            </li>
          </ul>
        </section>
        <p v-if="!plan" class="field-help">{{ messages.portable.chooseEveryCollision }}</p>
        <div v-if="state.safetyFailure" class="inline-alert" role="alert">
          <strong>{{ messages.portable.safetyFailed }}</strong>
          <p>{{ state.safetyFailure }}</p>
          <p>{{ messages.portable.retainedFailure }}</p>
        </div>
      </template>
      <div class="action-row">
        <button type="button" class="button-secondary" :disabled="busy" @click="discardReview">
          {{ messages.portable.discardReview }}
        </button>
        <button
          v-if="mutation"
          ref="reviewButton"
          type="button"
          :class="{ 'button-danger': replacing }"
          :disabled="busy || state.stale || !plan"
          @click="openConfirmation"
        >
          {{ state.safetyFailure ? messages.portable.reviewBypass : messages.portable.reviewApply }}
        </button>
        <button v-else type="button" :disabled="busy || state.stale" @click="save">
          {{
            state.review.kind === "unopened"
              ? messages.portable.reexportExact
              : messages.portable.saveFile
          }}
        </button>
      </div>
    </section>

    <Dialog :open="confirmation !== null" @update:open="(open) => !open && back()">
      <DialogContent
        class="project-confirmation"
        @open-auto-focus="focusBack"
        @close-auto-focus="restoreFocus"
        @escape-key-down="(event) => busy && event.preventDefault()"
        @interact-outside.prevent
      >
        <DialogTitle>{{ confirmationTitle }}</DialogTitle>
        <DialogDescription>{{ confirmationDescription }}</DialogDescription>
        <p v-if="state.stale" class="inline-alert" role="alert">{{ messages.portable.stale }}</p>
        <p v-if="state.safetyFailure" class="danger-note">{{ state.safetyFailure }}</p>
        <label v-if="confirmation === 'replace' || confirmation === 'bypass'" class="choice-row"
          ><input v-model="replacementAcknowledged" type="checkbox" :disabled="busy" /><span>{{
            messages.portable.replacementAcknowledgement
          }}</span></label
        >
        <div v-if="confirmation === 'bypass'" class="stacked-form">
          <label for="portable-bypass-phrase">{{ messages.portable.bypassLabel }}</label>
          <input
            id="portable-bypass-phrase"
            v-model="bypassPhrase"
            autocomplete="off"
            :disabled="busy"
            aria-describedby="portable-bypass-help"
          />
          <p id="portable-bypass-help" class="field-help">{{ messages.portable.bypassHelp }}</p>
        </div>
        <p v-if="state.pending" role="status">{{ messages.portable.settlement }}</p>
        <div class="action-row">
          <button
            ref="backButton"
            type="button"
            class="button-secondary"
            :disabled="busy"
            @click="back"
          >
            {{ messages.portable.back }}
          </button>
          <button
            v-if="state.pending && home.state.operation?.cancel && !home.state.operation.settled"
            type="button"
            class="button-secondary"
            :disabled="home.state.operation?.cancellationRequested"
            @click="home.cancelOperation()"
          >
            {{ messages.operations.cancel }}
          </button>
          <button
            type="button"
            :class="{ 'button-danger': replacing }"
            :disabled="
              busy ||
              state.stale ||
              !plan ||
              ((confirmation === 'replace' || confirmation === 'bypass') &&
                !replacementAcknowledged) ||
              (confirmation === 'bypass' && bypassPhrase !== 'REPLACE WITHOUT BACKUP')
            "
            @click="confirm"
          >
            {{
              confirmation === "bypass"
                ? messages.portable.confirmBypass
                : messages.portable.confirmApply
            }}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  </section>
</template>
