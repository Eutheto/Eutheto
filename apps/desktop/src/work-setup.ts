import { computed, onScopeDispose, shallowReactive, watch } from "vue";
import {
  getScenarioEntity,
  getScenarioView,
  searchScenarioEntities,
  SetupOperationScope,
  newUuidV7,
  type FieldErrorDto,
  type ScenarioCommand,
} from "./api/generated";
import {
  WORKFORCE_ADD_ENTITY_COMMAND_ID,
  WORKFORCE_UPDATE_ENTITY_COMMAND_ID,
  WORKFORCE_REMOVE_ENTITY_COMMAND_ID,
  WORKFORCE_DETACH_SHIFT_COMMAND_ID,
  WORKFORCE_REATTACH_SHIFT_COMMAND_ID,
  type WorkforceDateRange,
  type WorkforceSetupFacts,
  type WorkforceSetupEntityPage,
  type WorkforceSetupEntityContinuation,
  type WorkforceSetupWorkPage,
  type WorkforceSetupTimedShiftContinuation,
  type WorkforceSetupWorkShiftDetail,
  type WorkforceSetupGenerationRow,
  type WorkforceSetupGapPolicy,
  type WorkforceSetupOverlapPolicy,
  type WorkforceShiftInstance,
} from "./api/generated-domain-pack-contracts";
import { parseTemporalDraft } from "./components/planner/temporal-field";
import type { TemporalFeedback } from "./components/planner/field-contracts";
import {
  createWorkRecordDraft,
  isWorkRecord,
  workRecordValue,
  type WorkRecord,
  type WorkRecordDraft,
  type WorkEndpointResolution,
} from "./work-record-draft";
import {
  rebaseEntityDraft,
  resolveEntityDraftField,
  sameField,
  type EntityRebase,
  type EntityField,
} from "./entity-draft";
import {
  useWorkCommandReview,
  type WorkReviewSide,
  type WorkReviewTarget,
} from "./work-command-review";
import { createClinicWorkPreset, workPresetCommand } from "./work-presets";
import {
  isOperationCancelled,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "./project-home";
import { messages } from "./messages";

interface Context {
  readonly scenarioId: string;
  readonly revision: number;
  readonly epoch: number;
}
interface TimeContext {
  readonly timeZone: string;
  readonly gapPolicy: WorkforceSetupGapPolicy;
  readonly overlapPolicy: WorkforceSetupOverlapPolicy;
}
interface FactsCapture {
  readonly context: Context;
  readonly value: WorkforceSetupFacts;
}
interface RecordCapture {
  readonly context: Context;
  readonly value: WorkRecord;
}
type Endpoint = "startsAt" | "endsAt";
interface PreparedEndpoint extends WorkEndpointResolution {
  readonly context: Context;
  readonly time: TimeContext;
}
interface EndpointDecision {
  readonly choice: "current" | "reinterpret";
  readonly time: TimeContext;
}
export interface WorkSettingsDraft {
  readonly id: string;
  readonly kind: "settings";
  readonly timeZone: string;
  readonly startDate: string;
  readonly endDateExclusive: string;
  readonly gapPolicy: WorkforceSetupGapPolicy;
  readonly overlapPolicy: WorkforceSetupOverlapPolicy;
}
interface RecordEditor {
  readonly kind: "record";
  readonly context: Context;
  readonly time: TimeContext;
  readonly id: string;
  readonly base: WorkRecord | null;
  readonly raw: WorkRecordDraft;
  readonly editing: boolean;
  readonly dirty: boolean;
  readonly current: RecordCapture | null;
  readonly rebase: EntityRebase<WorkRecord> | null;
  readonly rebaseContext: Context | null;
  readonly prepared: Readonly<Record<Endpoint, PreparedEndpoint | null>>;
  readonly decisions: Readonly<Record<Endpoint, EndpointDecision | null>>;
}
interface SettingsEditor {
  readonly kind: "settings";
  readonly context: Context;
  readonly raw: WorkSettingsDraft;
  readonly base: WorkSettingsDraft;
  readonly dirty: boolean;
  readonly rebase: EntityRebase<WorkSettingsDraft> | null;
  readonly rebaseContext: Context | null;
}
interface PresetEntry {
  readonly id: string;
  readonly original: WorkRecord;
  readonly raw: WorkRecordDraft;
}
interface PresetEditor {
  readonly kind: "preset";
  readonly context: Context;
  readonly entries: readonly PresetEntry[];
  readonly selectedId: string;
  readonly dirty: boolean;
}
type Editor = RecordEditor | SettingsEditor | PresetEditor;
const endpoints: readonly Endpoint[] = ["startsAt", "endsAt"];

function timeContext(facts: WorkforceSetupFacts): TimeContext {
  return {
    timeZone: facts.settings.timeZone,
    gapPolicy: facts.settings.gapPolicy,
    overlapPolicy: facts.settings.overlapPolicy,
  };
}
function settingsDraft(context: Context, facts: WorkforceSetupFacts): WorkSettingsDraft {
  return {
    id: context.scenarioId,
    kind: "settings",
    timeZone: facts.settings.timeZone,
    startDate: facts.planningDates.startDate,
    endDateExclusive: facts.planningDates.endDateExclusive,
    gapPolicy: facts.settings.gapPolicy,
    overlapPolicy: facts.settings.overlapPolicy,
  };
}
function rawEndpoint(raw: WorkRecordDraft, side: Endpoint): string | null {
  return raw.kind === "shiftInstance" && raw.interval.raw.kind === "localInterval"
    ? raw.interval.raw[side]
    : null;
}
function replaceEndpoint(raw: WorkRecordDraft, side: Endpoint, local: string): WorkRecordDraft {
  if (raw.kind !== "shiftInstance" || raw.interval.raw.kind !== "localInterval") return raw;
  return { ...raw, interval: parseTemporalDraft({ ...raw.interval.raw, [side]: local }) };
}
function fieldErrors(failure: unknown): readonly FieldErrorDto[] {
  if (
    typeof failure !== "object" ||
    failure === null ||
    !("fieldErrors" in failure) ||
    !Array.isArray(failure.fieldErrors)
  )
    return [];
  return failure.fieldErrors.filter(
    (item: unknown): item is FieldErrorDto =>
      typeof item === "object" &&
      item !== null &&
      "field" in item &&
      typeof item.field === "string" &&
      "code" in item &&
      typeof item.code === "string" &&
      "message" in item &&
      typeof item.message === "string",
  );
}
function relativeErrors(failure: unknown, prefix: string): Readonly<Record<string, string>> {
  const result: Record<string, string> = {};
  for (const issue of fieldErrors(failure)) {
    if (issue.field.startsWith(`${prefix}/`))
      result[issue.field.slice(prefix.length + 1).replaceAll("/", ".")] = issue.message;
  }
  return result;
}

/** Route-local raw ownership. Native snapshots and the root operation owner remain authoritative. */
export function useWorkSetup(home: ProjectHomeController, project: () => ProjectSummary) {
  const review = useWorkCommandReview(home, project);
  const state = shallowReactive<{
    facts: FactsCapture | null;
    factsLoading: boolean;
    factsError: string | null;
    recordKind: WorkRecord["kind"];
    search: string;
    records: WorkforceSetupEntityPage | null;
    recordsLoading: boolean;
    recordsError: string | null;
    detailLoading: boolean;
    detailError: string | null;
    dates: WorkforceDateRange | null;
    work: WorkforceSetupWorkPage | null;
    workLoading: boolean;
    workError: string | null;
    workFailure: unknown;
    selectedWork: {
      readonly context: Context;
      readonly value: WorkforceSetupWorkShiftDetail;
    } | null;
    editor: Editor | null;
    editorPending: boolean;
    errors: Readonly<Record<string, string>>;
    temporalFeedback: TemporalFeedback | undefined;
    draftGeneration: number;
    editorError: string | null;
    reviewChangesOnly: boolean;
    reviewWithinWindow: boolean;
  }>({
    facts: null,
    factsLoading: false,
    factsError: null,
    recordKind: "shiftTemplate",
    search: "",
    records: null,
    recordsLoading: false,
    recordsError: null,
    detailLoading: false,
    detailError: null,
    dates: null,
    work: null,
    workLoading: false,
    workError: null,
    workFailure: null,
    selectedWork: null,
    editor: null,
    editorPending: false,
    errors: {},
    temporalFeedback: undefined,
    draftGeneration: 0,
    editorError: null,
    reviewChangesOnly: true,
    reviewWithinWindow: false,
  });
  let alive = true;
  let factsGeneration = 0,
    recordsGeneration = 0,
    detailGeneration = 0,
    workGeneration = 0;
  let factsScope: SetupOperationScope | null = null;
  let recordsScope: SetupOperationScope | null = null;
  let detailScope: SetupOperationScope | null = null;
  let workScope: SetupOperationScope | null = null;
  let endpointScope: SetupOperationScope | null = null;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  const context = (): Context => ({
    scenarioId: project().scenarioId,
    revision: project().revision,
    epoch: home.state.libraryEpoch,
  });
  const matches = (captured: Context): boolean => alive && sameField(captured, context());
  const facts = computed(() =>
    state.facts !== null && matches(state.facts.context) ? state.facts.value : null,
  );
  const busy = computed(
    () => home.state.busyAction !== null || state.editorPending || review.state.pending,
  );
  const dirty = computed(() => state.editor?.dirty === true);
  const stale = computed(() => state.editor !== null && !matches(state.editor.context));
  const requestKey = computed(
    () =>
      `${project().scenarioId}:${String(project().revision)}:${String(home.state.libraryEpoch)}:${String(state.draftGeneration)}`,
  );
  const activeRaw = computed(() => {
    const editor = state.editor;
    return editor?.kind === "record"
      ? editor.raw
      : editor?.kind === "preset"
        ? (editor.entries.find((entry) => entry.id === editor.selectedId)?.raw ?? null)
        : null;
  });
  const canReplace = computed(
    () =>
      !busy.value &&
      (state.editor === null || (state.editor.kind === "record" && !state.editor.editing)),
  );
  // Re-read route ownership after awaits; navigation can replace or discard a draft.
  function readRecordEditor(): RecordEditor | null {
    const editor = state.editor;
    return editor?.kind === "record" ? editor : null;
  }
  const currentRecord = computed(() => {
    const editor = state.editor;
    return editor?.kind === "record" && editor.current !== null && matches(editor.current.context)
      ? editor.current.value
      : null;
  });
  const conflictsCurrent = computed(() => {
    const editor = state.editor;
    return (
      editor !== null &&
      editor.kind !== "preset" &&
      editor.rebaseContext !== null &&
      matches(editor.rebaseContext)
    );
  });
  const diagnostics = computed(() => [
    ...fieldErrors(state.workFailure),
    ...fieldErrors(review.state.failure),
  ]);

  function diagnosticTarget(
    field: string,
  ):
    | { readonly kind: "settings" }
    | { readonly kind: "shiftTemplate" | "shiftInstance"; readonly id: string }
    | null {
    if (field === "/settings" || field.startsWith("/settings/")) return { kind: "settings" };
    const match = /^\/domain\/entities\/([^/]+)\/(timing|startsAt|endsAt)(?:\/|$)/u.exec(field);
    return match?.[1] === undefined
      ? null
      : { kind: match[2] === "timing" ? "shiftTemplate" : "shiftInstance", id: match[1] };
  }
  async function repairDiagnostic(issue: FieldErrorDto): Promise<boolean> {
    const target = diagnosticTarget(issue.field);
    if (!canReplace.value || target === null) return false;
    if (target.kind === "settings") {
      editSettings();
      state.errors = relativeErrors({ fieldErrors: [issue] }, "/settings");
      return state.editor?.kind === "settings";
    }
    if (!(await loadRecord(target.id, target.kind))) return false;
    editRecord();
    state.errors = relativeErrors({ fieldErrors: [issue] }, `/domain/entities/${target.id}`);
    return true;
  }

  function invalidateDraft(): void {
    state.draftGeneration += 1;
    endpointScope?.dispose();
    endpointScope = null;
    review.invalidate();
    state.errors = {};
    state.editorError = null;
    state.temporalFeedback = undefined;
  }
  function discard(): void {
    invalidateDraft();
    state.editor = null;
    state.detailError = null;
    detailGeneration += 1;
    detailScope?.dispose();
    detailScope = null;
    state.detailLoading = false;
  }
  async function loadFacts(): Promise<WorkforceSetupFacts | null> {
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
      if (generation !== factsGeneration || !matches(captured)) return null;
      const value = response.result.view.data.result.data;
      state.facts = { context: captured, value };
      state.dates ??= { ...value.initialWorkWindow };
      return value;
    } catch (failure) {
      if (generation === factsGeneration) state.factsError = safeMessage(failure);
      return null;
    } finally {
      owned.dispose();
      if (factsScope === owned) factsScope = null;
      if (generation === factsGeneration) state.factsLoading = false;
    }
  }
  async function loadRecords(
    cursor: WorkforceSetupEntityContinuation | null = null,
  ): Promise<void> {
    const captured = context(),
      generation = ++recordsGeneration;
    recordsScope?.dispose();
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    recordsScope = owned;
    const kind = state.recordKind,
      search = state.search;
    state.recordsLoading = true;
    state.recordsError = null;
    state.records = null;
    try {
      const response = await searchScenarioEntities(owned, { kind, search, limit: 50, cursor })
        .result;
      if (generation !== recordsGeneration || !matches(captured)) return;
      const page = response.result.view.data.result.data;
      if (page.items.length > 50 || page.items.some((item) => item.kind !== kind))
        throw new Error("The native work-record page did not match its requested kind or bound.");
      state.records = page;
    } catch (failure) {
      if (generation === recordsGeneration) state.recordsError = safeMessage(failure);
    } finally {
      owned.dispose();
      if (recordsScope === owned) recordsScope = null;
      if (generation === recordsGeneration) state.recordsLoading = false;
    }
  }
  function setRecordKind(kind: WorkRecord["kind"]): void {
    state.recordKind = kind;
    void loadRecords();
  }
  function setSearch(value: string): void {
    state.search = value;
    clearTimeout(searchTimer);
    recordsGeneration += 1;
    recordsScope?.dispose();
    state.records = null;
    searchTimer = setTimeout(() => {
      void loadRecords();
    }, 150);
  }
  function setDates(dates: WorkforceDateRange): void {
    state.dates = dates;
    workGeneration += 1;
    workScope?.dispose();
    state.work = null;
    state.workLoading = false;
    if (state.reviewWithinWindow) review.invalidate();
  }
  function setReviewFilter(changesOnly: boolean, withinWindow: boolean): void {
    state.reviewChangesOnly = changesOnly;
    state.reviewWithinWindow = withinWindow;
    review.invalidate();
  }
  async function loadWork(
    cursor: WorkforceSetupTimedShiftContinuation | null = null,
  ): Promise<void> {
    if (home.state.busyAction !== null || state.dates === null || facts.value === null) return;
    const captured = context(),
      generation = ++workGeneration,
      dates = { ...state.dates };
    workScope?.dispose();
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    workScope = owned;
    state.workLoading = true;
    state.workError = null;
    state.workFailure = null;
    state.work = null;
    let cancelled = false;
    const current = (): boolean => !cancelled && generation === workGeneration && matches(captured);
    let cancelCurrent: (() => Promise<unknown>) | null = null;
    try {
      const response = await home.runOperation({
        action: "work-window",
        label: messages.work.instances,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
          await cancelCurrent?.();
        },
        execute: (report) => {
          const operation = getScenarioView(
            owned,
            {
              source: { kind: "stored" },
              query: {
                schemaVersion: 1,
                viewId: "official.workforce.setup.work_window",
                parameters: { dates, limit: 50 },
                continuation: cursor,
              },
            },
            report,
          );
          cancelCurrent = () => operation.cancel();
          return operation.result;
        },
        success: () => "",
      });
      if (!current()) return;
      const page = response.result.view.data.result.data;
      if (page.items.length > 50)
        throw new Error("The native work page exceeded its requested bound.");
      state.work = page;
    } catch (failure) {
      if (generation === workGeneration) {
        state.workError = isOperationCancelled(failure) ? null : safeMessage(failure);
        state.workFailure = failure;
      }
    } finally {
      owned.dispose();
      if (workScope === owned) workScope = null;
      if (generation === workGeneration) state.workLoading = false;
    }
  }
  async function loadRecord(
    id: string,
    kind: WorkRecord["kind"],
    refresh = false,
  ): Promise<boolean> {
    const capturedFacts = facts.value;
    if (capturedFacts === null || (!refresh && !canReplace.value)) return false;
    if (!refresh) discard();
    const captured = context(),
      generation = ++detailGeneration;
    detailScope?.dispose();
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    detailScope = owned;
    state.detailLoading = true;
    state.detailError = null;
    try {
      const response = await getScenarioEntity(owned, { kind, entityId: id }).result;
      if (generation !== detailGeneration || !matches(captured)) return false;
      const value = response.result.view.data.result.data;
      if (value.id !== id || value.kind !== kind || !isWorkRecord(value))
        throw new Error("The native work record did not match the requested identity.");
      const existing = state.editor;
      if (refresh && existing?.kind === "record" && existing.id === id && existing.editing)
        state.editor = { ...existing, current: { context: captured, value } };
      else {
        state.editor = {
          kind: "record",
          context: captured,
          time: timeContext(capturedFacts),
          id,
          base: value,
          raw: createWorkRecordDraft(value),
          editing: false,
          dirty: false,
          current: { context: captured, value },
          rebase: null,
          rebaseContext: null,
          prepared: { startsAt: null, endsAt: null },
          decisions: { startsAt: null, endsAt: null },
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
  function createRecord(): void {
    if (!canReplace.value || facts.value === null) return;
    discard();
    state.editor = {
      kind: "record",
      context: context(),
      time: timeContext(facts.value),
      id: newUuidV7(),
      base: null,
      raw: createWorkRecordDraft(state.recordKind),
      editing: true,
      dirty: true,
      current: null,
      rebase: null,
      rebaseContext: null,
      prepared: { startsAt: null, endsAt: null },
      decisions: { startsAt: null, endsAt: null },
    };
  }
  function editRecord(): void {
    const editor = state.editor;
    if (busy.value || stale.value || editor?.kind !== "record") return;
    invalidateDraft();
    state.editor = { ...editor, editing: true };
  }
  function updateRaw(raw: WorkRecordDraft): void {
    const editor = state.editor;
    if (busy.value || editor === null) return;
    if (editor.kind === "record" && editor.editing && raw.kind === editor.raw.kind) {
      const prepared = { ...editor.prepared },
        decisions = { ...editor.decisions };
      for (const side of endpoints)
        if (rawEndpoint(raw, side) !== rawEndpoint(editor.raw, side)) {
          prepared[side] = null;
          decisions[side] = null;
        }
      invalidateDraft();
      state.editor = {
        ...editor,
        raw,
        dirty: true,
        rebase: null,
        rebaseContext: null,
        prepared,
        decisions,
      };
    } else if (editor.kind === "preset") {
      const selected = editor.entries.find((entry) => entry.id === editor.selectedId);
      if (selected?.raw.kind !== raw.kind) return;
      invalidateDraft();
      state.editor = {
        ...editor,
        dirty: true,
        entries: editor.entries.map((entry) =>
          entry.id === editor.selectedId ? { ...entry, raw } : entry,
        ),
      };
    }
  }
  function editSettings(): void {
    if (!canReplace.value || facts.value === null) return;
    discard();
    const captured = context(),
      raw = settingsDraft(captured, facts.value);
    state.editor = {
      kind: "settings",
      context: captured,
      base: raw,
      raw,
      dirty: false,
      rebase: null,
      rebaseContext: null,
    };
  }
  function updateSettings(raw: WorkSettingsDraft): void {
    const editor = state.editor;
    if (busy.value || editor?.kind !== "settings") return;
    invalidateDraft();
    state.editor = { ...editor, raw, dirty: true, rebase: null, rebaseContext: null };
  }
  function createPreset(): void {
    if (!canReplace.value || facts.value === null) return;
    const entries = createClinicWorkPreset(facts.value.planningDates).map((original) => ({
      id: original.id,
      original,
      raw: createWorkRecordDraft(original),
    }));
    const first = entries[0];
    if (first === undefined) throw new Error("The finite Work proposal has no records.");
    discard();
    state.editor = {
      kind: "preset",
      context: context(),
      entries,
      selectedId: first.id,
      dirty: true,
    };
  }
  function selectPreset(id: string): void {
    const editor = state.editor;
    if (!busy.value && editor?.kind === "preset" && editor.entries.some((entry) => entry.id === id))
      state.editor = { ...editor, selectedId: id };
  }
  function restorePresetReferences(): void {
    const editor = state.editor;
    if (busy.value || editor?.kind !== "preset") return;
    const entry = editor.entries.find((item) => item.id === editor.selectedId);
    if (entry === undefined) return;
    if (entry.raw.kind === "assignmentType" && entry.original.kind === "assignmentType") {
      const original = entry.original;
      updateRaw({
        ...entry.raw,
        workloadBucketIds: original.workloadBucketIds,
        locationMode: original.locationBehavior.kind,
        locationId:
          original.locationBehavior.kind === "fixed" ? original.locationBehavior.locationId : "",
      });
    } else if (entry.raw.kind === "shiftTemplate" && entry.original.kind === "shiftTemplate")
      updateRaw({
        ...entry.raw,
        assignmentTypeId: entry.original.assignmentTypeId,
        locationEnabled: entry.original.locationId !== undefined,
        locationId: entry.original.locationId ?? "",
      });
  }
  function changedEndpoint(editor: RecordEditor, side: Endpoint): boolean {
    const raw = rawEndpoint(editor.raw, side);
    return (
      raw !== null &&
      raw !== "" &&
      (editor.base?.kind !== "shiftInstance" || raw !== editor.base[side].local)
    );
  }
  function needsTimeChoice(side: Endpoint): boolean {
    const editor = state.editor,
      current = facts.value;
    if (
      editor?.kind !== "record" ||
      current === null ||
      !changedEndpoint(editor, side) ||
      sameField(editor.time, timeContext(current))
    )
      return false;
    const decision = editor.decisions[side];
    return (
      decision === null ||
      (decision.choice === "reinterpret" && !sameField(decision.time, timeContext(current)))
    );
  }
  function chooseEndpoint(side: Endpoint, choice: "current" | "reinterpret" | "clear"): void {
    const editor = state.editor,
      current = facts.value;
    if (
      busy.value ||
      editor?.kind !== "record" ||
      editor.raw.kind !== "shiftInstance" ||
      current === null
    )
      return;
    let raw = editor.raw;
    let prepared: PreparedEndpoint | null = null;
    const decision: EndpointDecision | null =
      choice === "clear" ? null : { choice, time: timeContext(current) };
    if (choice === "current") {
      const stored = currentRecord.value;
      if (stored?.kind !== "shiftInstance") return;
      raw = replaceEndpoint(raw, side, stored[side].local) as Extract<
        WorkRecordDraft,
        { kind: "shiftInstance" }
      >;
      prepared = {
        context: context(),
        time: timeContext(current),
        local: stored[side].local,
        resolved: stored[side],
      };
    } else if (choice === "clear")
      raw = replaceEndpoint(raw, side, "") as Extract<WorkRecordDraft, { kind: "shiftInstance" }>;
    invalidateDraft();
    let rebase = editor.rebase;
    if (choice === "current" && rebase !== null)
      rebase = resolveEntityDraftField(rebase, side, "current");
    state.editor = {
      ...editor,
      raw,
      dirty: true,
      rebase,
      prepared: { ...editor.prepared, [side]: prepared },
      decisions: { ...editor.decisions, [side]: decision },
    };
  }
  async function prepareEndpoint(side: Endpoint): Promise<boolean> {
    const editor = state.editor,
      currentFacts = facts.value;
    if (
      home.state.busyAction !== null ||
      editor?.kind !== "record" ||
      currentFacts === null ||
      needsTimeChoice(side) ||
      !changedEndpoint(editor, side)
    )
      return false;
    const local = rawEndpoint(editor.raw, side);
    if (local === null || local === "") return false;
    const captured = context(),
      draft = state.draftGeneration,
      time = timeContext(currentFacts);
    const existing = editor.prepared[side];
    if (
      existing !== null &&
      matches(existing.context) &&
      existing.local === local &&
      sameField(existing.time, time)
    )
      return true;
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    endpointScope?.dispose();
    endpointScope = owned;
    let cancelled = false;
    const current = (): boolean =>
      !cancelled && matches(captured) && draft === state.draftGeneration;
    let cancelCurrent: (() => Promise<unknown>) | null = null;
    try {
      const response = await home.runOperation({
        action: `work-prepare-${side}`,
        label: messages.work.preparingTime,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
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
      if (!current() || state.editor?.kind !== "record" || state.editor.id !== editor.id)
        return false;
      state.editor = {
        ...state.editor,
        prepared: {
          ...state.editor.prepared,
          [side]: {
            context: captured,
            time,
            local,
            resolved: response.result.view.data.result.data,
          },
        },
      };
      state.errors = {};
      state.temporalFeedback = undefined;
      return true;
    } catch (failure) {
      if (current()) {
        state.editorError = isOperationCancelled(failure) ? null : safeMessage(failure);
        const errors = fieldErrors(failure).filter(
          (item) => item.field === "/query/parameters/local",
        );
        state.errors = { [side]: errors[0]?.message ?? safeMessage(failure) };
        state.temporalFeedback = {
          requestKey: requestKey.value,
          errors: errors.map((item) => ({
            field: side === "startsAt" ? "start" : "end",
            message: item.message,
          })),
        };
      }
      return false;
    } finally {
      owned.dispose();
      if (endpointScope === owned) endpointScope = null;
    }
  }
  function candidate(editor: RecordEditor): WorkRecord | null {
    const preparation = (side: Endpoint): WorkEndpointResolution | null => {
      const value = editor.prepared[side];
      return value !== null && matches(value.context) ? value : null;
    };
    const parsed = workRecordValue(editor.id, editor.raw, {
      baseline: editor.base,
      startsAt: preparation("startsAt"),
      endsAt: preparation("endsAt"),
    });
    state.errors = parsed.errors;
    return parsed.value;
  }
  async function prepareChangedEndpoints(): Promise<boolean> {
    for (const side of endpoints) {
      const editor = state.editor;
      if (editor?.kind !== "record") return false;
      if (changedEndpoint(editor, side) && !(await prepareEndpoint(side))) return false;
    }
    return true;
  }
  async function previewSource(
    source:
      | { readonly kind: "stored" }
      | { readonly kind: "commandPreview"; readonly command: ScenarioCommand },
  ): Promise<boolean> {
    const captured = context(),
      draft = state.draftGeneration;
    const ready = await review.preview(source, {
      changesOnly: state.reviewChangesOnly,
      dates: state.reviewWithinWindow ? state.dates : null,
    });
    if (!ready && matches(captured) && draft === state.draftGeneration && state.editor !== null) {
      const editor = state.editor;
      if (editor.kind === "record")
        state.errors = relativeErrors(review.state.failure, `/domain/entities/${editor.id}`);
      if (editor.kind === "preset") {
        for (const entry of editor.entries) {
          const errors = relativeErrors(review.state.failure, `/domain/entities/${entry.id}`);
          if (Object.keys(errors).length !== 0) {
            state.editor = { ...editor, selectedId: entry.id };
            state.errors = errors;
            break;
          }
        }
      }
    }
    return ready;
  }
  async function previewEditor(): Promise<boolean> {
    if (busy.value || stale.value || facts.value === null || state.editor === null) return false;
    state.editorError = null;
    state.errors = {};
    const editor = state.editor;
    const captured = context(),
      draft = state.draftGeneration;
    if (editor.kind === "record") {
      if (!editor.editing || editor.rebase !== null) return false;
      state.editorPending = true;
      try {
        if (!(await prepareChangedEndpoints())) return false;
      } finally {
        state.editorPending = false;
      }
      const fresh = readRecordEditor();
      if (
        fresh === null ||
        fresh.id !== editor.id ||
        !matches(captured) ||
        draft !== state.draftGeneration
      )
        return false;
      const entity = candidate(fresh);
      if (entity === null) return false;
      return previewSource({
        kind: "commandPreview",
        command: {
          type: "applyDomainCommand",
          payload: {
            commandType:
              fresh.base === null
                ? WORKFORCE_ADD_ENTITY_COMMAND_ID
                : WORKFORCE_UPDATE_ENTITY_COMMAND_ID,
            payload: { entity },
          },
        },
      });
    }
    if (editor.kind === "preset") {
      const values: WorkRecord[] = [];
      for (const entry of editor.entries) {
        const parsed = workRecordValue(entry.id, entry.raw, {
          baseline: null,
          startsAt: null,
          endsAt: null,
        });
        if (parsed.value === null) {
          state.editor = { ...editor, selectedId: entry.id };
          state.errors = parsed.errors;
          return false;
        }
        values.push(parsed.value);
      }
      return previewSource({ kind: "commandPreview", command: workPresetCommand(values) });
    }
    if (editor.rebase !== null) return false;
    const native = facts.value;
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    endpointScope = owned;
    state.editorPending = true;
    let cancelled = false;
    let cancelCurrent: (() => Promise<unknown>) | null = null;
    try {
      const response = await home.runOperation({
        action: "work-prepare-settings",
        label: messages.work.reviewSettings,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
          await cancelCurrent?.();
        },
        execute: (report) => {
          const operation = getScenarioView(
            owned,
            {
              source: { kind: "stored" },
              query: {
                schemaVersion: 1,
                viewId: "official.workforce.setup.settings_preparation",
                parameters: {
                  timeZone: editor.raw.timeZone,
                  dates: {
                    startDate: editor.raw.startDate,
                    endDateExclusive: editor.raw.endDateExclusive,
                  },
                  gapPolicy: editor.raw.gapPolicy,
                  overlapPolicy: editor.raw.overlapPolicy,
                  locale: native.settings.locale,
                  units: native.settings.units,
                },
              },
            },
            report,
          );
          cancelCurrent = () => operation.cancel();
          return operation.result;
        },
        success: () => "",
      });
      const current = (): boolean =>
        !cancelled && draft === state.draftGeneration && matches(captured);
      if (!current()) return false;
      state.editorPending = false;
      return await previewSource({
        kind: "commandPreview",
        command: {
          type: "setScenarioSettings",
          payload: { settings: response.result.view.data.result.data, restoration: null },
        },
      });
    } catch (failure) {
      if (draft === state.draftGeneration && matches(captured)) {
        state.editorError = isOperationCancelled(failure) ? null : safeMessage(failure);
        state.errors = relativeErrors(failure, "/query/parameters");
      }
      return false;
    } finally {
      owned.dispose();
      if (endpointScope === owned) endpointScope = null;
      state.editorPending = false;
    }
  }
  async function previewDelete(): Promise<boolean> {
    const editor = state.editor;
    if (
      busy.value ||
      stale.value ||
      editor?.kind !== "record" ||
      editor.editing ||
      editor.base === null
    )
      return false;
    return previewSource({
      kind: "commandPreview",
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: WORKFORCE_REMOVE_ENTITY_COMMAND_ID,
          payload: { entityId: editor.id },
        },
      },
    });
  }
  function acceptRecordRebase(editor: RecordEditor, merged: EntityRebase<WorkRecord>): void {
    if (merged.conflicts.length !== 0) {
      state.editor = { ...editor, rebase: merged, rebaseContext: context() };
      return;
    }
    const native = facts.value;
    if (native === null) return;
    state.editor = {
      ...editor,
      context: context(),
      time: timeContext(native),
      base: merged.current,
      raw: createWorkRecordDraft(merged.value, { value: merged.local, raw: editor.raw }),
      current: { context: context(), value: merged.current },
      dirty: true,
      rebase: null,
      rebaseContext: null,
      decisions: { startsAt: null, endsAt: null },
    };
  }
  async function rebase(): Promise<void> {
    const targetFacts = facts.value;
    if (busy.value || !stale.value || state.editor === null || targetFacts === null) return;
    const initial = state.editor;
    const captured = context(),
      draft = state.draftGeneration;
    if (initial.kind === "settings") {
      const current = settingsDraft(captured, targetFacts);
      const merged = rebaseEntityDraft(initial.base, initial.raw, current, initial.rebase);
      invalidateDraft();
      state.editor =
        merged.conflicts.length === 0
          ? {
              ...initial,
              context: context(),
              base: current,
              raw: merged.value,
              rebase: null,
              rebaseContext: null,
              dirty: true,
            }
          : { ...initial, rebase: merged, rebaseContext: context() };
      return;
    }
    if (initial.kind === "preset") {
      invalidateDraft();
      state.editor = { ...initial, context: context() };
      return;
    }
    if (!initial.editing) return;
    if (
      initial.base !== null &&
      (currentRecord.value === null || !matches(initial.current?.context ?? initial.context))
    ) {
      if (!(await loadRecord(initial.id, initial.raw.kind, true))) return;
    }
    let editor = readRecordEditor();
    if (
      editor === null ||
      editor.id !== initial.id ||
      !matches(captured) ||
      draft !== state.draftGeneration
    )
      return;
    // A prior explicit Current endpoint follows the newly read native value, never old raw intent.
    const current = currentRecord.value;
    if (current?.kind === "shiftInstance") {
      for (const side of endpoints)
        if (editor.decisions[side]?.choice === "current") {
          editor = {
            ...editor,
            raw: replaceEndpoint(editor.raw, side, current[side].local),
            prepared: {
              ...editor.prepared,
              [side]: {
                context: context(),
                time: timeContext(targetFacts),
                local: current[side].local,
                resolved: current[side],
              },
            },
          };
        }
      state.editor = editor;
    }
    if (endpoints.some(needsTimeChoice)) {
      state.editorError = messages.work.timeContextChanged;
      return;
    }
    state.editorPending = true;
    try {
      if (!(await prepareChangedEndpoints())) return;
    } finally {
      state.editorPending = false;
    }
    editor = readRecordEditor();
    if (
      editor === null ||
      editor.id !== initial.id ||
      !matches(captured) ||
      draft !== state.draftGeneration
    )
      return;
    const local = candidate(editor);
    if (local === null) return;
    if (editor.base === null) {
      review.invalidate();
      state.editor = { ...editor, context: captured, time: timeContext(targetFacts) };
      return;
    }
    const saved = currentRecord.value;
    if (saved === null) return;
    let previous = editor.rebase;
    if (
      previous !== null &&
      local.kind === "shiftInstance" &&
      previous.value.kind === "shiftInstance" &&
      previous.current.kind === "shiftInstance" &&
      previous.local.kind === "shiftInstance"
    ) {
      let value = previous.value,
        original = previous.local;
      let conflicts = previous.conflicts;
      for (const side of endpoints) {
        if (editor.decisions[side]?.choice === "current") {
          value = { ...value, [side]: previous.current[side] };
          conflicts = conflicts.filter((field) => field !== side);
        } else if (changedEndpoint(editor, side)) {
          value = { ...value, [side]: local[side] };
          original = { ...original, [side]: local[side] };
        }
      }
      previous = { ...previous, value, local: original, conflicts };
    }
    review.invalidate();
    acceptRecordRebase(editor, rebaseEntityDraft(editor.base, local, saved, previous));
  }
  function chooseConflict(
    field: EntityField<WorkRecord> | EntityField<WorkSettingsDraft>,
    choice: "current" | "draft",
  ): void {
    const editor = state.editor;
    if (
      busy.value ||
      !conflictsCurrent.value ||
      editor === null ||
      editor.kind === "preset" ||
      editor.rebase === null ||
      facts.value === null
    )
      return;
    review.invalidate();
    if (editor.kind === "settings") {
      const merged = resolveEntityDraftField(
        editor.rebase,
        field as EntityField<WorkSettingsDraft>,
        choice,
      );
      state.editor =
        merged.conflicts.length === 0
          ? {
              ...editor,
              context: context(),
              base: merged.current,
              raw: merged.value,
              rebase: null,
              rebaseContext: null,
              dirty: true,
            }
          : { ...editor, rebase: merged };
    } else {
      let changed = editor;
      if (
        choice === "current" &&
        (field === "startsAt" || field === "endsAt") &&
        editor.rebase.current.kind === "shiftInstance"
      ) {
        const endpoint = editor.rebase.current[field];
        changed = {
          ...editor,
          raw: replaceEndpoint(editor.raw, field, endpoint.local),
          decisions: {
            ...editor.decisions,
            [field]: { choice: "current", time: timeContext(facts.value) },
          },
          prepared: {
            ...editor.prepared,
            [field]: {
              context: context(),
              time: timeContext(facts.value),
              local: endpoint.local,
              resolved: endpoint,
            },
          },
        };
      }
      acceptRecordRebase(
        changed,
        resolveEntityDraftField(editor.rebase, field as EntityField<WorkRecord>, choice),
      );
    }
  }
  function copyAsNew(): void {
    const editor = state.editor;
    if (busy.value || editor?.kind !== "record" || facts.value === null) return;
    invalidateDraft();
    state.editor = {
      ...editor,
      id: newUuidV7(),
      base: null,
      current: null,
      rebase: null,
      rebaseContext: null,
      context: context(),
      editing: true,
      dirty: true,
      prepared: { startsAt: null, endsAt: null },
      decisions: { startsAt: null, endsAt: null },
    };
  }
  async function inspectShift(shiftId: string): Promise<boolean> {
    if (busy.value || facts.value === null) return false;
    const captured = context(),
      generation = ++detailGeneration;
    detailScope?.dispose();
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    detailScope = owned;
    state.detailLoading = true;
    state.detailError = null;
    state.selectedWork = null;
    try {
      const response = await getScenarioView(owned, {
        source: { kind: "stored" },
        query: {
          schemaVersion: 1,
          viewId: "official.workforce.setup.work_detail",
          parameters: { shiftId },
        },
      }).result;
      if (generation !== detailGeneration || !matches(captured)) return false;
      const value = response.result.view.data.result.data;
      if (value.shift.shiftId !== shiftId)
        throw new Error("The native shift detail changed identity.");
      state.selectedWork = { context: captured, value };
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
  async function openShiftSource(): Promise<boolean> {
    const selected = state.selectedWork;
    if (selected === null || !matches(selected.context)) return false;
    return loadRecord(
      selected.value.source.record.id,
      selected.value.source.kind === "template" ? "shiftTemplate" : "shiftInstance",
    );
  }
  async function detach(): Promise<boolean> {
    const selected = state.selectedWork;
    if (!canReplace.value || selected === null || !matches(selected.context)) return false;
    const { shift, source } = selected.value;
    if (
      source.kind !== "template" ||
      shift.origin.kind !== "generated" ||
      shift.origin.templateId !== source.record.id
    )
      return false;
    const record = source.record;
    const instance: WorkforceShiftInstance = {
      kind: "shiftInstance",
      id: shift.shiftId,
      assignmentTypeId: record.assignmentTypeId,
      coverage: record.coverage,
      startsAt: shift.interval.startsAt,
      endsAt: shift.interval.endsAt,
      origin: {
        kind: "detached",
        templateId: record.id,
        occurrenceDate: shift.origin.occurrenceDate,
      },
      reportingAttribution: record.reportingAttribution,
      tags: record.tags,
      ...(record.locationId === undefined ? {} : { locationId: record.locationId }),
    };
    return previewSource({
      kind: "commandPreview",
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: WORKFORCE_DETACH_SHIFT_COMMAND_ID,
          payload: { templateId: record.id, instance },
        },
      },
    });
  }
  async function reattach(): Promise<boolean> {
    const editor = state.editor;
    if (
      busy.value ||
      stale.value ||
      editor?.kind !== "record" ||
      editor.editing ||
      editor.base?.kind !== "shiftInstance" ||
      editor.base.origin.kind !== "detached"
    )
      return false;
    const record = editor.base;
    const origin = record.origin;
    if (origin.kind !== "detached") return false;
    return previewSource({
      kind: "commandPreview",
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: WORKFORCE_REATTACH_SHIFT_COMMAND_ID,
          payload: {
            templateId: origin.templateId,
            occurrenceIdentities: {
              [record.id]: { id: record.id, localStartDate: origin.occurrenceDate },
            },
          },
        },
      },
    });
  }
  async function inspectGeneration(
    row: WorkforceSetupGenerationRow,
    side: WorkReviewSide,
  ): Promise<boolean> {
    const origin = side === "after" ? row.after?.origin : row.before?.data.origin;
    if (origin === undefined) return false;
    return review.inspect(
      {
        kind: "entity",
        id: origin.kind === "generated" ? origin.templateId : row.shiftId,
        entityKind: origin.kind === "generated" ? "shiftTemplate" : "shiftInstance",
      },
      side,
    );
  }
  async function inspectChange(ordinal: number, side: WorkReviewSide): Promise<boolean> {
    const row = review.state.review?.changes?.items.find((item) => item.ordinal === ordinal);
    if (row === undefined) return false;
    const path = row.change.path;
    let target: WorkReviewTarget;
    if (path === "/settings" || path.startsWith("/settings/")) target = { kind: "settings" };
    else {
      const match = /^\/domain\/entities\/([^/]+)(?:\/([^/]+))?(?:\/|$)/u.exec(path);
      if (match?.[1] === undefined) return false;
      const value = side === "before" ? row.change.before : row.change.after;
      let kind: unknown;
      if (match[2] === "occurrenceIdentities") kind = "shiftTemplate";
      else if (
        typeof value === "object" &&
        value !== null &&
        !Array.isArray(value) &&
        "kind" in value
      )
        kind = value.kind;
      else return false;
      if (
        kind !== "location" &&
        kind !== "workloadBucket" &&
        kind !== "calendar" &&
        kind !== "assignmentType" &&
        kind !== "shiftTemplate" &&
        kind !== "shiftInstance"
      )
        return false;
      target = { kind: "entity", id: match[1], entityKind: kind };
    }
    return review.inspect(target, side);
  }
  async function apply(truncateRedo = false): Promise<boolean> {
    const scenarioId = review.state.review?.snapshot.scenarioId;
    const receipt = await review.apply(truncateRedo);
    if (receipt === null) return false;
    if (!alive || project().scenarioId !== scenarioId) return true;
    discard();
    state.selectedWork = null;
    await refresh();
    return true;
  }
  async function refresh(): Promise<void> {
    if (!alive || project().domainPackId !== "official.workforce") return;
    const value = await loadFacts();
    if (value === null) return;
    const editor = state.editor;
    if (editor?.kind === "record" && !editor.editing)
      await loadRecord(editor.id, editor.raw.kind, true);
    await loadRecords();
    await loadWork();
  }
  watch(
    [() => project().scenarioId, () => project().revision, () => home.state.libraryEpoch],
    (next, previous) => {
      factsGeneration += 1;
      recordsGeneration += 1;
      detailGeneration += 1;
      workGeneration += 1;
      factsScope?.dispose();
      recordsScope?.dispose();
      detailScope?.dispose();
      workScope?.dispose();
      endpointScope?.dispose();
      state.facts = null;
      state.records = null;
      state.work = null;
      state.selectedWork = null;
      state.workError = null;
      state.workFailure = null;
      state.detailError = null;
      state.editorError = null;
      state.detailLoading = false;
      state.workLoading = false;
      state.draftGeneration += 1;
      state.temporalFeedback = undefined;
      state.errors = {};
      if (state.editor?.kind === "record")
        state.editor = {
          ...state.editor,
          current: null,
          prepared: { startsAt: null, endsAt: null },
        };
      if (next[0] !== previous[0]) {
        state.dates = null;
        discard();
      }
      void refresh();
    },
    { flush: "sync" },
  );
  onScopeDispose(() => {
    alive = false;
    clearTimeout(searchTimer);
    factsGeneration += 1;
    recordsGeneration += 1;
    detailGeneration += 1;
    workGeneration += 1;
    factsScope?.dispose();
    recordsScope?.dispose();
    detailScope?.dispose();
    workScope?.dispose();
    endpointScope?.dispose();
  });
  void refresh();
  return {
    state,
    review,
    facts,
    busy,
    dirty,
    stale,
    requestKey,
    activeRaw,
    canReplace,
    currentRecord,
    loadRecords,
    setRecordKind,
    setSearch,
    setDates,
    setReviewFilter,
    loadWork,
    loadRecord,
    createRecord,
    editRecord,
    updateRaw,
    editSettings,
    updateSettings,
    createPreset,
    selectPreset,
    restorePresetReferences,
    needsTimeChoice,
    chooseEndpoint,
    prepareEndpoint,
    previewEditor,
    previewDelete,
    rebase,
    chooseConflict,
    copyAsNew,
    inspectShift,
    openShiftSource,
    detach,
    reattach,
    inspectGeneration,
    inspectChange,
    apply,
    refresh,
    discard,
    diagnostics,
    diagnosticTarget,
    repairDiagnostic,
    conflictsCurrent,
    changedEndpoint,
    regenerate: () =>
      canReplace.value ? previewSource({ kind: "stored" }) : Promise.resolve(false),
  };
}
