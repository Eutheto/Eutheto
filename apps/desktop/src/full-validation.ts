import { computed, onScopeDispose, shallowReactive, watch } from "vue";
import {
  getScenarioSetupStatus,
  SetupOperationScope,
  validateScenario,
  type FullValidationResultV2,
  type ScenarioSetupStatusV2,
  type SetupOperation,
} from "./api/generated";
import { messages } from "./messages";
import {
  isOperationCancelled,
  isRevisionConflict,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "./project-home";

export function useFullValidation(home: ProjectHomeController, project: () => ProjectSummary) {
  const state = shallowReactive<{
    status: ScenarioSetupStatusV2 | null;
    report: FullValidationResultV2 | null;
    loading: boolean;
    error: string | null;
    run: "idle" | "running" | "completed" | "cancelled" | "failed";
    failure: string | null;
  }>({ status: null, report: null, loading: false, error: null, run: "idle", failure: null });
  let current = true;
  let generation = 0;
  let readGeneration = 0;
  let readScope: SetupOperationScope | null = null;
  let runScope: SetupOperationScope | null = null;
  const supported = computed(() => project().domainPackId === "official.workforce");
  const busy = computed(() => home.state.busyAction !== null);

  function capture() {
    const { scenarioId, revision } = project();
    const captured = generation;
    return {
      scenarioId,
      revision,
      isCurrent: () =>
        current &&
        captured === generation &&
        project().scenarioId === scenarioId &&
        project().revision === revision,
    };
  }

  async function refreshStatus(): Promise<void> {
    const captured = capture();
    const read = ++readGeneration;
    readScope?.dispose();
    readScope = null;
    if (!captured.isCurrent() || !supported.value) return;
    state.loading = true;
    state.error = null;
    let owned: SetupOperationScope | null = null;
    try {
      owned = new SetupOperationScope(captured.scenarioId, captured.revision);
      readScope = owned;
      const { result } = await getScenarioSetupStatus(owned).result;
      if (!captured.isCurrent() || read !== readGeneration) return;
      state.status = result;
    } catch (failure) {
      if (captured.isCurrent() && read === readGeneration)
        state.error = isRevisionConflict(failure) ? messages.setup.stale : safeMessage(failure);
    } finally {
      owned?.dispose();
      if (captured.isCurrent() && read === readGeneration) {
        readScope = null;
        state.loading = false;
      }
    }
  }

  async function run(): Promise<void> {
    if (!current || !supported.value || busy.value || state.run === "running") return;
    const captured = capture();
    state.run = "running";
    state.failure = null;
    let owned: SetupOperationScope | null = null;
    let operation: SetupOperation<FullValidationResultV2> | null = null;
    try {
      owned = new SetupOperationScope(captured.scenarioId, captured.revision);
      runScope = owned;
      const scope = owned;
      const { result } = await home.runOperation({
        action: "validation.full",
        label: messages.validationWorkspace.run,
        refreshLibrary: false,
        execute: (report) => {
          operation = validateScenario(scope, report);
          return operation.result;
        },
        cancel: () => operation?.cancel() ?? Promise.resolve(),
        success: () => messages.validationWorkspace.completed(String(captured.revision)),
      });
      if (!captured.isCurrent()) return;
      state.report = result;
      state.run = "completed";
    } catch (failure) {
      if (!captured.isCurrent()) return;
      state.run = isOperationCancelled(failure) ? "cancelled" : "failed";
      state.failure = isRevisionConflict(failure)
        ? messages.setup.stale
        : isOperationCancelled(failure)
          ? null
          : safeMessage(failure);
    } finally {
      owned?.dispose();
      if (runScope === owned) runScope = null;
      if (captured.isCurrent()) await refreshStatus();
    }
  }

  watch(
    [() => project().scenarioId, () => project().revision, () => home.state.changeSignal],
    ([scenarioId, revision, changeSignal], previous) => {
      if (
        scenarioId === previous[0] &&
        revision === previous[1] &&
        changeSignal !== null &&
        changeSignal.scenarioId !== null &&
        changeSignal.scenarioId !== scenarioId
      )
        return;
      generation += 1;
      readGeneration += 1;
      readScope?.dispose();
      runScope?.dispose();
      readScope = null;
      runScope = null;
      state.status = null;
      state.report = null;
      state.run = "idle";
      state.failure = null;
      state.error = null;
      state.loading = false;
      void refreshStatus();
    },
    { immediate: true, flush: "sync" },
  );
  onScopeDispose(() => {
    current = false;
    generation += 1;
    readGeneration += 1;
    readScope?.dispose();
    runScope?.dispose();
  });
  return { state, supported, busy, run, refreshStatus };
}
