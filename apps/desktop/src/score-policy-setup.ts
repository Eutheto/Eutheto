import { computed, onScopeDispose, shallowReactive, watch } from "vue";
import {
  getScenarioEntity,
  newUuidV7,
  searchScenarioEntities,
  SetupOperationScope,
  type ScenarioCommand,
} from "./api/generated";
import {
  WORKFORCE_ADD_ENTITY_COMMAND_ID,
  WORKFORCE_UPDATE_ENTITY_COMMAND_ID,
  type WorkforceSetupEntityContinuation,
  type WorkforceSetupEntityPage,
} from "./api/generated-domain-pack-contracts";
import {
  rebaseEntityDraft,
  resolveEntityDraftField,
  type EntityField,
  type EntityRebase,
} from "./entity-draft";
import { messages } from "./messages";
import { safeMessage, type ProjectHomeController, type ProjectSummary } from "./project-home";
import { useSetupCommandReview } from "./setup-command-review";
import {
  createScorePolicyDraft,
  scorePolicyDraftValue,
  type ScorePolicyDraft,
  type ScorePolicyDraftResult,
  type ScorePolicyValue,
} from "./score-policy-draft";

interface Context {
  readonly scenarioId: string;
  readonly revision: number;
  readonly epoch: number;
}
interface Editor {
  readonly context: Context;
  readonly id: string;
  readonly creating: boolean;
  readonly base: ScorePolicyValue;
  readonly raw: ScorePolicyDraft;
  readonly editing: boolean;
  readonly dirty: boolean;
  readonly current: ScorePolicyValue | null;
  readonly rebase: EntityRebase<ScorePolicyValue> | null;
  readonly rebaseContext: Context | null;
}

function sameContext(left: Context, right: Context): boolean {
  return (
    left.scenarioId === right.scenarioId &&
    left.revision === right.revision &&
    left.epoch === right.epoch
  );
}
function fieldErrors(failure: unknown): Readonly<Record<string, string>> {
  if (typeof failure !== "object" || failure === null || !("fieldErrors" in failure)) return {};
  const entries = failure.fieldErrors;
  if (!Array.isArray(entries)) return {};
  const result: Record<string, string> = {};
  for (const entry of entries) {
    if (typeof entry !== "object" || entry === null) continue;
    if (!("field" in entry) || !("message" in entry)) continue;
    const field = entry.field;
    const message = entry.message;
    if (typeof field === "string" && typeof message === "string") result[field] = message;
  }
  return result;
}

/** Owns one singleton ScorePolicy draft while native reads, preview and apply stay authoritative. */
export function useScorePolicySetup(home: ProjectHomeController, project: () => ProjectSummary) {
  const review = useSetupCommandReview(home, project);
  const state = shallowReactive<{
    page: WorkforceSetupEntityPage | null;
    pageLoading: boolean;
    pageError: string | null;
    detailLoading: boolean;
    detailError: string | null;
    editor: Editor | null;
    errors: Readonly<Record<string, string>>;
  }>({
    page: null,
    pageLoading: false,
    pageError: null,
    detailLoading: false,
    detailError: null,
    editor: null,
    errors: {},
  });
  let alive = true;
  let pageGeneration = 0;
  let detailGeneration = 0;
  let draftGeneration = 0;
  let pageScope: SetupOperationScope | null = null;
  let detailScope: SetupOperationScope | null = null;

  const context = (): Context => ({
    scenarioId: project().scenarioId,
    revision: project().revision,
    epoch: home.state.libraryEpoch,
  });
  const matches = (captured: Context): boolean => alive && sameContext(captured, context());
  const busy = computed(
    () => home.state.busyAction !== null || state.detailLoading || review.state.pending,
  );
  const canReplace = computed(
    () => !busy.value && (state.editor === null || !state.editor.editing),
  );
  const dirty = computed(() => state.editor?.dirty === true);
  const stale = computed(() => {
    const editor = state.editor;
    return (
      editor !== null &&
      (editor.creating
        ? !matches(editor.context) || state.page?.items.length !== 0
        : editor.current === null || !matches(editor.context))
    );
  });
  const conflictsCurrent = computed(() => {
    const editor = state.editor;
    return (
      editor !== null &&
      editor.rebase !== null &&
      editor.rebaseContext !== null &&
      matches(editor.rebaseContext) &&
      !state.detailLoading
    );
  });

  function invalidateDraft(): void {
    draftGeneration += 1;
    review.invalidate();
    state.errors = {};
  }
  function discard(): void {
    invalidateDraft();
    detailGeneration += 1;
    detailScope?.dispose();
    detailScope = null;
    state.detailLoading = false;
    state.detailError = null;
    state.editor = null;
  }

  async function loadPage(
    cursor: WorkforceSetupEntityContinuation | null = null,
  ): Promise<WorkforceSetupEntityPage | null> {
    const captured = context();
    const generation = ++pageGeneration;
    pageScope?.dispose();
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    pageScope = owned;
    state.pageLoading = true;
    state.pageError = null;
    state.page = null;
    try {
      const response = await searchScenarioEntities(owned, {
        kind: "scorePolicy",
        search: "",
        limit: 50,
        cursor,
      }).result;
      if (generation !== pageGeneration || !matches(captured)) return null;
      const page = response.result.view.data.result.data;
      if (
        page.items.length > 50 ||
        page.totalItems > 1 ||
        page.items.some((item) => item.kind !== "scorePolicy")
      )
        throw new Error(messages.scorePolicy.singletonMismatch);
      state.page = page;
      return page;
    } catch (failure) {
      if (generation === pageGeneration && alive) state.pageError = safeMessage(failure);
      return null;
    } finally {
      owned.dispose();
      if (pageScope === owned) pageScope = null;
      if (generation === pageGeneration) state.pageLoading = false;
    }
  }

  async function loadDetail(id: string, refresh = false): Promise<boolean> {
    if (!alive || (!refresh && !canReplace.value)) return false;
    const captured = context();
    const generation = ++detailGeneration;
    detailScope?.dispose();
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    detailScope = owned;
    state.detailLoading = true;
    state.detailError = null;
    if (!refresh) {
      invalidateDraft();
      state.editor = null;
    }
    try {
      const response = await getScenarioEntity(owned, { kind: "scorePolicy", entityId: id }).result;
      if (generation !== detailGeneration || !matches(captured)) return false;
      const value = response.result.view.data.result.data;
      if (value.id !== id || value.kind !== "scorePolicy")
        throw new Error(messages.scorePolicy.identityMismatch);
      const currentValue: ScorePolicyValue = value;
      const existing = state.editor;
      if (refresh && existing !== null && existing.id === id && existing.editing) {
        state.editor = { ...existing, current: currentValue };
      } else {
        state.editor = {
          context: captured,
          id,
          base: currentValue,
          creating: false,
          raw: createScorePolicyDraft(currentValue),
          editing: false,
          dirty: false,
          current: currentValue,
          rebase: null,
          rebaseContext: null,
        };
      }
      return true;
    } catch (failure) {
      if (generation === detailGeneration) state.detailError = safeMessage(failure);
      return false;
    } finally {
      owned.dispose();
      if (detailScope === owned) detailScope = null;
      if (generation === detailGeneration) state.detailLoading = false;
    }
  }

  function create(): void {
    if (!canReplace.value || state.page?.items.length !== 0) return;
    const id = newUuidV7();
    const base: ScorePolicyValue = {
      kind: "scorePolicy",
      id,
      profileKey: "",
      levels: [{ levelKey: "preferences", label: "Preferences" }],
      priorityMapping: (["low", "normal", "high", "veryHigh"] as const).map((priority) => ({
        priority,
        levelKey: "preferences",
        scale: 1,
      })),
      workloadPolicies: {},
      tieBreak: "stableAssignmentRank",
    };
    invalidateDraft();
    state.editor = {
      context: context(),
      id,
      base,
      raw: createScorePolicyDraft(base),
      creating: true,
      editing: true,
      dirty: true,
      current: null,
      rebase: null,
      rebaseContext: null,
    };
  }
  async function openPolicy(id: string): Promise<boolean> {
    return loadDetail(id);
  }
  function edit(): void {
    const editor = state.editor;
    if (!canReplace.value || editor === null || stale.value) return;
    invalidateDraft();
    state.editor = { ...editor, editing: true };
  }
  function updateRaw(raw: ScorePolicyDraft): void {
    const editor = state.editor;
    if (editor === null || !editor.editing || busy.value || raw.id !== editor.id) return;
    invalidateDraft();
    state.editor = {
      ...editor,
      raw,
      dirty: true,
      rebase: null,
      rebaseContext: null,
    };
  }
  function candidate(editor: Editor): ScorePolicyDraftResult {
    const result = scorePolicyDraftValue(editor.id, editor.raw);
    state.errors = result.errors;
    return result;
  }
  async function previewEditor(): Promise<boolean> {
    const editor = state.editor;
    if (editor === null || !editor.editing || stale.value || editor.rebase !== null || busy.value)
      return false;
    const converted = candidate(editor);
    if (converted.value === null) return false;
    const captured = context();
    const draft = draftGeneration;
    const command: ScenarioCommand = {
      type: "applyDomainCommand",
      payload: {
        commandType: editor.creating
          ? WORKFORCE_ADD_ENTITY_COMMAND_ID
          : WORKFORCE_UPDATE_ENTITY_COMMAND_ID,
        payload: { entity: converted.value },
      },
    };
    const ready = await review.preview(command, { id: editor.id, kind: "scorePolicy" });
    if (!ready && draft === draftGeneration && matches(captured))
      state.errors = { ...state.errors, ...fieldErrors(review.state.failure) };
    return ready;
  }

  function acceptRebase(editor: Editor, merged: EntityRebase<ScorePolicyValue>): void {
    if (merged.conflicts.length !== 0) {
      state.editor = {
        ...editor,
        current: merged.current,
        rebase: merged,
        rebaseContext: context(),
      };
      return;
    }
    state.editor = {
      ...editor,
      context: context(),
      base: merged.current,
      raw: createScorePolicyDraft(merged.value, { value: merged.local, raw: editor.raw }),
      current: merged.current,
      dirty: true,
      rebase: null,
      rebaseContext: null,
    };
  }
  async function rebase(): Promise<void> {
    const initial = state.editor;
    if (initial === null || !initial.editing || !stale.value || busy.value) return;
    if (initial.creating) return;
    const converted = candidate(initial);
    if (converted.value === null) return;
    const captured = context();
    if (
      initial.current === null ||
      initial.rebaseContext === null ||
      !sameContext(initial.rebaseContext, captured)
    ) {
      if (!(await loadDetail(initial.id, true))) return;
    }
    const refreshed = state.editor;
    if (
      refreshed === null ||
      refreshed.id !== initial.id ||
      refreshed.current === null ||
      !matches(captured)
    )
      return;
    invalidateDraft();
    acceptRebase(
      { ...initial, current: refreshed.current, context: initial.context },
      rebaseEntityDraft(initial.base, converted.value, refreshed.current, initial.rebase),
    );
  }
  function chooseConflict(field: EntityField<ScorePolicyValue>, choice: "current" | "draft"): void {
    const editor = state.editor;
    if (editor === null || editor.rebase === null || !conflictsCurrent.value || busy.value) return;
    const merged = resolveEntityDraftField(editor.rebase, field, choice);
    invalidateDraft();
    acceptRebase(editor, merged);
  }
  async function apply(truncateRedo = false): Promise<boolean> {
    const scenarioId = review.state.review?.snapshot.scenarioId;
    const result = await review.apply(truncateRedo);
    if (result === null) return false;
    if (!alive || project().scenarioId !== scenarioId) return true;
    discard();
    await loadPage();
    return true;
  }
  async function refresh(): Promise<void> {
    if (!alive || project().domainPackId !== "official.workforce") return;
    const existing = state.editor;
    const page = await loadPage();
    if (page === null) return;
    if (existing?.creating) {
      const current = state.editor;
      if (current?.id !== existing.id || !current.creating) return;
      if (page.items.length === 0) state.editor = { ...current, context: context() };
      return;
    }
    if (state.editor !== existing) return;
    const id = existing?.id;
    if (id !== undefined) await loadDetail(id, existing?.editing === true);
  }
  watch(
    [() => project().scenarioId, () => project().revision, () => home.state.libraryEpoch],
    (next, previous) => {
      pageGeneration += 1;
      detailGeneration += 1;
      pageScope?.dispose();
      detailScope?.dispose();
      pageScope = null;
      detailScope = null;
      state.page = null;
      state.pageError = null;
      state.detailError = null;
      state.detailLoading = false;
      if (next[0] !== previous[0]) discard();
      else if (state.editor !== null) state.editor = { ...state.editor, current: null };
      void refresh();
    },
    { immediate: true, flush: "sync" },
  );
  onScopeDispose(() => {
    alive = false;
    pageGeneration += 1;
    detailGeneration += 1;
    pageScope?.dispose();
    detailScope?.dispose();
  });
  return {
    state,
    review,
    busy,
    dirty,
    stale,
    canReplace,
    conflictsCurrent,
    loadPage,
    loadDetail,
    create,
    openPolicy,
    edit,
    updateRaw,
    previewEditor,
    rebase,
    chooseConflict,
    apply,
    refresh,
    discard,
  };
}
