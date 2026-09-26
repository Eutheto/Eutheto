import { nextTick, onScopeDispose, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { messages } from "./messages";
import { safeMessage, type ProjectHomeController, type ProjectSummary } from "./project-home";
import { validationRouteTarget, type ValidationNavigationTarget } from "./validation-navigation";

/** Navigation is presentation-only and never authorizes replacing an existing dirty draft. */
export function useValidationRoute(
  home: ProjectHomeController,
  project: () => ProjectSummary,
  ready: () => boolean,
  open: (target: ValidationNavigationTarget, isCurrent: () => boolean) => Promise<boolean>,
) {
  const route = useRoute();
  const error = ref<string | null>(null);
  let alive = true;
  let generation = 0;
  let handled: string | null = null;
  watch(
    [
      () => route.fullPath,
      () => project().scenarioId,
      () => project().revision,
      () => home.state.libraryEpoch,
      ready,
    ],
    async () => {
      const captured = ++generation;
      error.value = null;
      const request = JSON.stringify([
        route.fullPath,
        project().scenarioId,
        project().revision,
        home.state.libraryEpoch,
      ]);
      if (handled !== request) handled = null;
      if (handled === request) return;
      if (route.query.findingPath === undefined || !ready()) return;
      const context = {
        scenarioId: project().scenarioId,
        revision: project().revision,
        libraryEpoch: home.state.libraryEpoch,
      };
      const path = route.fullPath;
      const target = validationRouteTarget(route.query, context);
      if (target === null) {
        error.value = messages.validationWorkspace.targetInvalid;
        return;
      }
      const current = () =>
        alive &&
        captured === generation &&
        route.fullPath === path &&
        project().scenarioId === context.scenarioId &&
        project().revision === context.revision &&
        home.state.libraryEpoch === context.libraryEpoch;
      await nextTick();
      if (!current()) return;
      try {
        const opened = await open(target, current);
        if (current()) {
          if (opened) handled = request;
          else error.value = messages.validationWorkspace.targetFailed;
        }
      } catch (failure) {
        if (current()) error.value = safeMessage(failure);
      }
    },
    { immediate: true, flush: "post" },
  );
  onScopeDispose(() => {
    alive = false;
    generation += 1;
  });
  return error;
}
