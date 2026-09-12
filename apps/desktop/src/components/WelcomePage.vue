<script setup lang="ts">
import { computed, onScopeDispose, reactive, shallowRef, watch } from "vue";
import { RouterLink, useRouter } from "vue-router";
import type { CalendarSettingsV1 } from "../api/generated";
import type { ProjectHomeController } from "../project-home";
import { messages } from "../messages";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";

const props = withDefaults(
  defineProps<{
    home: ProjectHomeController;
    startCreate?: boolean;
    locale?: string | undefined;
    units?: "metric" | "us-customary";
  }>(),
  { startCreate: false, locale: undefined, units: "metric" },
);
const router = useRouter();
const runtime = new Intl.DateTimeFormat().resolvedOptions();
const draft = reactive<
  { title: string; description: string } & {
    -readonly [K in keyof CalendarSettingsV1]: CalendarSettingsV1[K];
  }
>({
  title: "",
  description: "",
  timeZone: runtime.timeZone,
  locale: props.locale ?? runtime.locale,
  units: props.units,
  firstDate: "",
  lastDate: "",
  gapPolicy: "reject",
  overlapPolicy: "earlier",
});
const baseline = shallowRef({ ...draft });
const keys = Object.keys(draft) as (keyof typeof draft)[];
const dirty = computed(() => keys.some((key) => draft[key] !== baseline.value[key]));
const pending = computed(() => props.home.state.busyAction === "create");
let current = true;
onScopeDispose(() => {
  current = false;
});
watch([() => props.locale, () => props.units], () => {
  if (!dirty.value) {
    draft.locale = props.locale ?? runtime.locale;
    draft.units = props.units;
    baseline.value = { ...draft };
  }
});
function discard(): void {
  Object.assign(draft, baseline.value);
}
async function create(): Promise<void> {
  const created = await props.home.createProject({
    title: draft.title.trim(),
    description: draft.description.trim(),
    domainPack: { id: "official.workforce", schemaVersion: 1 },
    settings: {
      timeZone: draft.timeZone.trim(),
      locale: draft.locale.trim(),
      units: draft.units,
      firstDate: draft.firstDate,
      lastDate: draft.lastDate,
      gapPolicy: draft.gapPolicy,
      overlapPolicy: draft.overlapPolicy,
    },
  });
  if (created !== null && current) {
    baseline.value = { ...draft };
    await router.push({ name: "project-setup", params: { scenarioId: created.scenarioId } });
  }
}
</script>

<template>
  <section v-if="!startCreate" class="page-stack" aria-labelledby="welcome-heading">
    <header class="page-heading">
      <p class="eyebrow">{{ messages.welcome.eyebrow }}</p>
      <h1 id="welcome-heading" data-route-heading tabindex="-1">{{ messages.welcome.heading }}</h1>
      <p>{{ messages.welcome.description }}</p>
    </header>
    <div class="intent-grid">
      <RouterLink :to="{ name: 'project-create' }" class="intent-card">
        <h2>{{ messages.welcome.workSchedule }}</h2>
        <p>{{ messages.welcome.workDescription }}</p>
        <span>{{ messages.welcome.startWork }}</span>
      </RouterLink>
      <article class="intent-card unavailable-card" aria-labelledby="seating-heading">
        <h2 id="seating-heading">{{ messages.welcome.eventSeating }}</h2>
        <p>{{ messages.welcome.seatingDescription }}</p>
        <span class="status-label">{{ messages.welcome.unavailable }}</span>
      </article>
      <RouterLink :to="{ name: 'project-import' }" class="intent-card">
        <h2>{{ messages.welcome.openExisting }}</h2>
        <p>{{ messages.welcome.existingDescription }}</p>
        <span>{{ messages.welcome.reviewFile }}</span>
      </RouterLink>
    </div>
    <div class="state-panel">
      <p>{{ messages.welcome.localOnly }}</p>
      <RouterLink :to="{ name: 'projects' }">{{ messages.projects.library }}</RouterLink>
      <p v-if="home.state.phase === 'ready'">
        {{ messages.projects.count(home.state.projects.length, locale) }}
      </p>
    </div>
  </section>
  <section v-else class="page-stack narrow-page" aria-labelledby="create-heading">
    <header class="page-heading">
      <p class="eyebrow">{{ messages.welcome.workSchedule }}</p>
      <h1 id="create-heading" data-route-heading tabindex="-1">
        {{ messages.welcome.createHeading }}
      </h1>
      <p>{{ messages.projects.createIntroduction }}</p>
    </header>
    <form class="stacked-form state-panel" @submit.prevent="create">
      <fieldset :disabled="home.state.busyAction !== null" class="form-fields">
        <legend>{{ messages.welcome.projectAndCalendar }}</legend>
        <label for="create-title">{{ messages.projects.title }}</label>
        <input id="create-title" v-model="draft.title" required autocomplete="off" />
        <label for="create-description">{{ messages.projects.description }}</label>
        <textarea id="create-description" v-model="draft.description" rows="3" />
        <label for="create-time-zone">{{ messages.projects.timeZone }}</label>
        <input
          id="create-time-zone"
          v-model="draft.timeZone"
          required
          autocomplete="off"
          spellcheck="false"
          aria-describedby="time-zone-help"
        />
        <p id="time-zone-help" class="field-help">{{ messages.welcome.timeZoneHelp }}</p>
        <div class="form-columns">
          <div class="field-stack">
            <label for="first-date">{{ messages.projects.planningStarts }}</label>
            <input
              id="first-date"
              v-model="draft.firstDate"
              type="date"
              required
              aria-describedby="planning-dates-help"
            />
          </div>
          <div class="field-stack">
            <label for="last-date">{{ messages.projects.planningEnds }}</label>
            <input
              id="last-date"
              v-model="draft.lastDate"
              type="date"
              required
              aria-describedby="planning-dates-help"
            />
          </div>
        </div>
        <p id="planning-dates-help" class="field-help">{{ messages.projects.planningDatesHelp }}</p>
        <details>
          <summary>{{ messages.welcome.calendarOptions }}</summary>
          <div class="form-fields">
            <label for="create-locale">{{ messages.projects.locale }}</label>
            <input
              id="create-locale"
              v-model="draft.locale"
              required
              autocomplete="off"
              spellcheck="false"
            />
            <label for="create-units">{{ messages.projects.displayUnits }}</label>
            <select id="create-units" v-model="draft.units">
              <option value="metric">{{ messages.projects.metric }}</option>
              <option value="us-customary">{{ messages.projects.usCustomary }}</option>
            </select>
            <label for="create-gap">{{ messages.projects.missingClockTime }}</label>
            <select id="create-gap" v-model="draft.gapPolicy">
              <option value="reject">{{ messages.projects.reject }}</option>
              <option value="moveForward">{{ messages.projects.moveForward }}</option>
              <option value="packDefined">{{ messages.projects.packPolicy }}</option>
            </select>
            <label for="create-overlap">{{ messages.projects.repeatedClockTime }}</label>
            <select id="create-overlap" v-model="draft.overlapPolicy">
              <option value="earlier">{{ messages.projects.earlier }}</option>
              <option value="later">{{ messages.projects.later }}</option>
              <option value="reject">{{ messages.projects.reject }}</option>
            </select>
          </div>
        </details>
        <p class="field-help">{{ messages.welcome.nativeValidation }}</p>
        <div class="dialog-actions">
          <RouterLink :to="{ name: 'welcome' }" class="button-link button-secondary">
            {{ messages.welcome.back }}
          </RouterLink>
          <button type="submit">
            {{ pending ? messages.projects.creating : messages.projects.create }}
          </button>
        </div>
      </fieldset>
    </form>
    <RouteLeaveGuard :home="home" :dirty="dirty" :pending="pending" :discard="discard" />
  </section>
</template>
