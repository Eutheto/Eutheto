import { computed, nextTick, onScopeDispose, shallowReactive, watch } from "vue";
import {
  getScenarioEntity,
  getScenarioView,
  newUuidV7,
  SetupOperationScope,
  type ScenarioCommand,
} from "./api/generated";
import type {
  WorkforceAvailability,
  WorkforceSetupAvailabilityRecordsPage,
  WorkforceSetupEntityContinuation,
  WorkforceSetupFacts,
} from "./api/generated-domain-pack-contracts";
import {
  availabilityDraftValue,
  createAvailabilityDraft,
  type AvailabilityDraft,
  type AvailabilityResolvedWindow,
} from "./availability-draft";
import {
  rebaseEntityDraft,
  resolveEntityDraftField,
  sameField,
  type EntityField,
  type EntityRebase,
} from "./entity-draft";
import {
  isOperationCancelled,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "./project-home";
import { availabilityMessages as copy } from "./components/availability/messages";
import { useSetupCommandReview } from "./setup-command-review";

interface AvailabilityContext {
  readonly scenarioId: string;
  readonly revision: number;
  readonly epoch: number;
}
interface AvailabilityTimeContext {
  readonly timeZone: string;
  readonly gapPolicy: WorkforceSetupFacts["settings"]["gapPolicy"];
  readonly overlapPolicy: WorkforceSetupFacts["settings"]["overlapPolicy"];
}
interface AvailabilityEditor {
  readonly context: AvailabilityContext;
  readonly time: AvailabilityTimeContext;
  readonly timeApproval: AvailabilityTimeContext | null;
  readonly id: string;
  readonly base: WorkforceAvailability | null;
  readonly raw: AvailabilityDraft;
  readonly dirty: boolean;
  readonly rebase: EntityRebase<WorkforceAvailability> | null;
  readonly rebaseContext: AvailabilityContext | null;
}
function timeContext(facts: WorkforceSetupFacts): AvailabilityTimeContext {
  const { timeZone, gapPolicy, overlapPolicy } = facts.settings;
  return { timeZone, gapPolicy, overlapPolicy };
}

/** Owns raw availability drafts; native queries and reviewed commands own domain meaning. */
export function useAvailabilitySetup(home: ProjectHomeController, project: () => ProjectSummary) {
  const state = shallowReactive<{
    personId: string | null;
    records: WorkforceSetupAvailabilityRecordsPage | null;
    recordsLoading: boolean;
    recordsError: string | null;
    facts: { readonly context: AvailabilityContext; readonly value: WorkforceSetupFacts } | null;
    factsLoading: boolean;
    factsError: string | null;
    editor: AvailabilityEditor | null;
    detailLoading: boolean;
    editorError: string | null;
    errors: Readonly<Record<string, string>>;
    draftGeneration: number;
    reviewAction: "save" | "remove" | null;
  }>({
    personId: null,
    records: null,
    recordsLoading: false,
    recordsError: null,
    facts: null,
    factsLoading: false,
    factsError: null,
    editor: null,
    detailLoading: false,
    editorError: null,
    errors: {},
    draftGeneration: 0,
    reviewAction: null,
  });
  const review = useSetupCommandReview(home, project);
  let alive = true;
  let recordsGeneration = 0;
  let detailGeneration = 0;
  let factsGeneration = 0;
  let recordsScope: SetupOperationScope | null = null;
  let detailScope: SetupOperationScope | null = null;
  let factsScope: SetupOperationScope | null = null;
  let endpointScope: SetupOperationScope | null = null;
  function context(): AvailabilityContext {
    return {
      scenarioId: project().scenarioId,
      revision: project().revision,
      epoch: home.state.libraryEpoch,
    };
  }
  function matches(value: AvailabilityContext): boolean {
    return alive && sameField(value, context());
  }
  const facts = computed(() =>
    state.facts !== null && matches(state.facts.context) ? state.facts.value : null,
  );
  const busy = computed(
    () => home.state.busyAction !== null || review.state.pending || state.detailLoading,
  );
  const unresolved = computed(() => home.state.mutation?.outcome === "outcomeUnknown");
  const dirty = computed(() => state.editor?.dirty === true || review.state.review !== null);
  const stale = computed(() => state.editor !== null && !matches(state.editor.context));
  const canReplace = computed(() => !busy.value && !dirty.value);
  const timeChanged = computed(
    () =>
      state.editor !== null &&
      facts.value !== null &&
      !sameField(state.editor.time, timeContext(facts.value)),
  );
  const needsTimeApproval = computed(() => {
    const editor = state.editor;
    return (
      editor !== null &&
      facts.value !== null &&
      timeChanged.value &&
      (editor.raw.windowKind === "weekly" || editor.raw.replaceInstant) &&
      !sameField(editor.timeApproval, timeContext(facts.value))
    );
  });
  function invalidateDraft(): void {
    state.draftGeneration += 1;
    endpointScope?.dispose();
    endpointScope = null;
    review.invalidate();
    state.reviewAction = null;
    state.editorError = null;
    state.errors = {};
  }
  function discard(): void {
    invalidateDraft();
    detailGeneration += 1;
    detailScope?.dispose();
    detailScope = null;
    state.detailLoading = false;
    state.editor = null;
  }
  async function loadFacts(): Promise<void> {
    const captured = context(),
      generation = ++factsGeneration;
    factsScope?.dispose();
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    factsScope = owned;
    state.factsLoading = true;
    state.factsError = null;
    try {
      const response = await getScenarioView(owned, {
        source: { kind: "stored" },
        query: { schemaVersion: 1, viewId: "official.workforce.setup.overview", parameters: {} },
      }).result;
      if (generation === factsGeneration && matches(captured))
        state.facts = { context: captured, value: response.result.view.data.result.data };
    } catch (failure) {
      if (generation === factsGeneration && matches(captured))
        state.factsError = safeMessage(failure);
    } finally {
      owned.dispose();
      if (factsScope === owned) factsScope = null;
      if (generation === factsGeneration) state.factsLoading = false;
    }
  }
  async function loadRecords(
    cursor: WorkforceSetupEntityContinuation | null = null,
  ): Promise<void> {
    const personId = state.personId;
    const captured = context(),
      generation = ++recordsGeneration;
    recordsScope?.dispose();
    recordsScope = null;
    state.records = null;
    state.recordsError = null;
    if (personId === null || !alive) {
      state.recordsLoading = false;
      return;
    }
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    recordsScope = owned;
    state.recordsLoading = true;
    try {
      const response = await getScenarioView(owned, {
        source: { kind: "stored" },
        query: {
          schemaVersion: 1,
          viewId: "official.workforce.setup.availability_records",
          parameters: { personId, limit: 50 },
          ...(cursor === null ? {} : { continuation: cursor }),
        },
      }).result;
      if (generation !== recordsGeneration || !matches(captured) || state.personId !== personId)
        return;
      const page = response.result.view.data.result.data;
      if (
        page.items.length > 50 ||
        new Set(page.items.map((item) => item.availabilityId)).size !== page.items.length
      )
        throw new Error(copy.recordsPageIdentityError);
      state.records = page;
    } catch (failure) {
      if (generation === recordsGeneration && matches(captured))
        state.recordsError = safeMessage(failure);
    } finally {
      owned.dispose();
      if (recordsScope === owned) recordsScope = null;
      if (generation === recordsGeneration) state.recordsLoading = false;
    }
  }
  async function selectPerson(personId: string | null): Promise<void> {
    if (!canReplace.value || personId === state.personId) return;
    discard();
    state.personId = personId;
    recordsGeneration += 1;
    recordsScope?.dispose();
    state.records = null;
    await nextTick();
    if (alive && state.personId === personId) await loadRecords();
  }
  function startAdd(dates?: {
    readonly startDate: string;
    readonly endDateExclusive: string;
  }): boolean {
    const currentFacts = facts.value;
    if (!canReplace.value || state.personId === null || currentFacts === null) return false;
    discard();
    state.editor = {
      context: context(),
      time: timeContext(currentFacts),
      timeApproval: null,
      id: newUuidV7(),
      base: null,
      dirty: true,
      rebase: null,
      rebaseContext: null,
      raw: {
        ...createAvailabilityDraft("new", state.personId),
        ...(dates ?? currentFacts.planningDates),
      },
    };
    return true;
  }
  async function readRecord(id: string): Promise<{
    readonly context: AvailabilityContext;
    readonly value: WorkforceAvailability;
  } | null> {
    const personId = state.personId;
    const captured = context(),
      generation = ++detailGeneration;
    detailScope?.dispose();
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    detailScope = owned;
    state.detailLoading = true;
    state.editorError = null;
    try {
      const response = await getScenarioEntity(owned, { kind: "availability", entityId: id })
        .result;
      if (generation !== detailGeneration || !matches(captured) || state.personId !== personId)
        return null;
      const value = response.result.view.data.result.data;
      if (value.kind !== "availability" || value.id !== id || value.personId !== personId)
        throw new Error(copy.recordIdentityMismatch);
      return { context: captured, value };
    } catch (failure) {
      if (generation === detailGeneration && matches(captured))
        state.editorError = safeMessage(failure);
      return null;
    } finally {
      owned.dispose();
      if (detailScope === owned) detailScope = null;
      if (generation === detailGeneration) state.detailLoading = false;
    }
  }
  async function open(id: string): Promise<boolean> {
    if (!canReplace.value || facts.value === null) return false;
    discard();
    const generation = state.draftGeneration;
    const record = await readRecord(id);
    const currentFacts = state.facts;
    if (
      record === null ||
      generation !== state.draftGeneration ||
      currentFacts === null ||
      !matches(currentFacts.context)
    )
      return false;
    state.editor = {
      context: record.context,
      time: timeContext(currentFacts.value),
      timeApproval: null,
      id,
      base: record.value,
      raw: createAvailabilityDraft(record.value, record.value.personId),
      dirty: false,
      rebase: null,
      rebaseContext: null,
    };
    return true;
  }
  function change(raw: AvailabilityDraft): void {
    const editor = state.editor;
    if (editor === null || busy.value || editor.rebase !== null) return;
    invalidateDraft();
    state.editor = { ...editor, raw, dirty: true };
  }
  function approveTime(enabled: boolean): void {
    if (state.editor === null || facts.value === null || busy.value) return;
    invalidateDraft();
    state.editor = { ...state.editor, timeApproval: enabled ? timeContext(facts.value) : null };
  }
  async function candidate(editor: AvailabilityEditor): Promise<WorkforceAvailability | null> {
    if (needsTimeApproval.value || facts.value === null) {
      state.editorError = copy.timeSettingsReviewRequired;
      return null;
    }
    const captured = context(),
      generation = state.draftGeneration;
    let resolved: AvailabilityResolvedWindow | null = null;
    const raw = editor.raw.localInterval.raw;
    if (
      editor.raw.windowKind === "instant" &&
      (editor.raw.replaceInstant || editor.raw.storedInstant === null) &&
      editor.raw.localInterval.status === "readyForNativeValidation" &&
      raw.kind === "localInterval"
    ) {
      const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
      endpointScope?.dispose();
      endpointScope = owned;
      const cancellation = { requested: false };
      try {
        const endpoints = [];
        for (const [label, local] of [
          ["start", raw.startsAt],
          ["end", raw.endsAt],
        ] as const) {
          let cancelCurrent: (() => Promise<unknown>) | null = null;
          const response = await home.runOperation({
            action: `availability-resolve-${label}`,
            label: copy.resolvingEndpoint(label),
            refreshLibrary: false,
            cancel: async () => {
              cancellation.requested = true;
              await cancelCurrent?.();
            },
            execute: (report) => {
              const operation = getScenarioView(
                owned,
                {
                  source: { kind: "stored" },
                  query: {
                    schemaVersion: 1,
                    viewId: "official.workforce.setup.local_time_resolution",
                    parameters: { local },
                  },
                },
                report,
              );
              cancelCurrent = () => operation.cancel();
              return operation.result;
            },
            success: () => "",
          });
          if (cancellation.requested || !matches(captured) || generation !== state.draftGeneration)
            return null;
          endpoints.push(response.result.view.data.result.data);
        }
        const [startsAt, endsAt] = endpoints;
        if (startsAt === undefined || endsAt === undefined) return null;
        resolved = { raw: { startsAt: raw.startsAt, endsAt: raw.endsAt }, startsAt, endsAt };
      } catch (failure) {
        if (
          matches(captured) &&
          generation === state.draftGeneration &&
          !isOperationCancelled(failure)
        )
          state.errors = { timeWindow: safeMessage(failure) };
        return null;
      } finally {
        owned.dispose();
        if (endpointScope === owned) endpointScope = null;
      }
    }
    if (!matches(captured) || generation !== state.draftGeneration) return null;
    const parsed = availabilityDraftValue(editor.id, editor.raw, resolved);
    state.errors = parsed.errors;
    return parsed.value;
  }
  async function preview(remove = false): Promise<boolean> {
    const editor = state.editor;
    if (
      editor === null ||
      busy.value ||
      stale.value ||
      unresolved.value ||
      editor.rebase !== null ||
      (remove && (editor.base === null || editor.dirty))
    )
      return false;
    const generation = state.draftGeneration;
    const value = remove ? null : await candidate(editor);
    if (
      (!remove && value === null) ||
      generation !== state.draftGeneration ||
      !matches(editor.context)
    )
      return false;
    const command: ScenarioCommand = {
      type: "applyDomainCommand",
      payload: remove
        ? { commandType: "official.workforce.remove_entity", payload: { entityId: editor.id } }
        : {
            commandType:
              editor.base === null
                ? "official.workforce.add_entity"
                : "official.workforce.update_entity",
            payload: { entity: value },
          },
    };
    const ready = await review.preview(
      command,
      remove ? null : { id: editor.id, kind: "availability" },
    );
    if (ready && generation === state.draftGeneration)
      state.reviewAction = remove ? "remove" : "save";
    return ready;
  }
  function acceptRebase(
    editor: AvailabilityEditor,
    merged: EntityRebase<WorkforceAvailability>,
    captured: AvailabilityContext,
  ): void {
    if (!matches(captured) || facts.value === null) return;
    invalidateDraft();
    state.editor =
      merged.conflicts.length > 0
        ? { ...editor, rebase: merged, rebaseContext: captured }
        : {
            ...editor,
            context: captured,
            time: timeContext(facts.value),
            timeApproval: null,
            base: merged.current,
            raw: createAvailabilityDraft(merged.value, merged.value.personId, {
              value: merged.local,
              raw: editor.raw,
            }),
            dirty: !sameField(merged.value, merged.current),
            rebase: null,
            rebaseContext: null,
          };
  }
  async function rebase(): Promise<boolean> {
    const editor = state.editor;
    if (
      editor === null ||
      busy.value ||
      unresolved.value ||
      facts.value === null ||
      needsTimeApproval.value
    )
      return false;
    const generation = state.draftGeneration;
    const local = await candidate(editor);
    if (local === null || generation !== state.draftGeneration) return false;
    const captured = context();
    if (editor.base === null) {
      invalidateDraft();
      state.editor = {
        ...editor,
        context: captured,
        time: timeContext(facts.value),
        timeApproval: null,
        rebase: null,
        rebaseContext: null,
      };
      return true;
    }
    const current = await readRecord(editor.id);
    if (current === null || generation !== state.draftGeneration || !matches(captured))
      return false;
    acceptRebase(
      editor,
      rebaseEntityDraft(editor.base, local, current.value, editor.rebase),
      current.context,
    );
    return true;
  }
  function chooseConflict(
    field: EntityField<WorkforceAvailability>,
    choice: "current" | "draft",
  ): void {
    const editor = state.editor;
    if (
      editor?.rebase === null ||
      editor?.rebase === undefined ||
      editor.rebaseContext === null ||
      !matches(editor.rebaseContext) ||
      busy.value
    )
      return;
    acceptRebase(
      editor,
      resolveEntityDraftField(editor.rebase, field, choice),
      editor.rebaseContext,
    );
  }
  async function save(truncateRedo = false): Promise<boolean> {
    const generation = state.draftGeneration,
      scenarioId = project().scenarioId;
    const receipt = await review.apply(truncateRedo);
    // The received commit receipt wins over the write's own revision notification.
    if (
      receipt === null ||
      !alive ||
      project().scenarioId !== scenarioId ||
      generation !== state.draftGeneration
    )
      return false;
    discard();
    const settled = context();
    await loadRecords();
    return matches(settled);
  }
  async function refresh(): Promise<void> {
    if (home.state.busyAction !== null) return;
    if (await home.refreshLibrary()) {
      await loadFacts();
      await loadRecords();
    }
  }
  watch(
    [() => project().scenarioId, () => project().revision, () => home.state.libraryEpoch],
    (current, previous) => {
      review.invalidate();
      state.reviewAction = null;
      recordsGeneration += 1;
      detailGeneration += 1;
      factsGeneration += 1;
      recordsScope?.dispose();
      detailScope?.dispose();
      factsScope?.dispose();
      endpointScope?.dispose();
      state.detailLoading = false;
      if (previous[0] !== undefined && previous[0] !== current[0]) {
        discard();
        state.personId = null;
      }
      if (project().domainPackId === "official.workforce") {
        void loadFacts();
        void loadRecords();
      }
    },
    { immediate: true, flush: "sync" },
  );
  onScopeDispose(() => {
    alive = false;
    recordsGeneration += 1;
    detailGeneration += 1;
    factsGeneration += 1;
    recordsScope?.dispose();
    detailScope?.dispose();
    factsScope?.dispose();
    endpointScope?.dispose();
  });
  return {
    state,
    review,
    facts,
    busy,
    unresolved,
    dirty,
    stale,
    canReplace,
    timeChanged,
    needsTimeApproval,
    selectPerson,
    startAdd,
    open,
    change,
    approveTime,
    preview,
    rebase,
    chooseConflict,
    save,
    discard,
    refresh,
    loadRecords,
  };
}
