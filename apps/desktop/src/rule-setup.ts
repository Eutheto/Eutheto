import { computed, onScopeDispose, shallowReactive, watch } from "vue";
import {
  getScenarioView,
  newUuidV7,
  searchScenarioEntities,
  SetupOperationScope,
  type ScenarioCommand,
  type SetupSourceV2,
} from "./api/generated";
import {
  WORKFORCE_ADD_RULE_COMMAND_ID,
  WORKFORCE_REMOVE_RULE_COMMAND_ID,
  WORKFORCE_UPDATE_RULE_COMMAND_ID,
  WORKFORCE_SETUP_RULE_DETAIL_QUERY_ID,
  WORKFORCE_SETUP_RULE_PAGE_QUERY_ID,
  type WorkforceScope,
  type WorkforceSetupEntityContinuation,
  type WorkforceSetupFacts,
  type WorkforceSetupRuleCatalog,
  type WorkforceSetupRuleCatalogEntry,
  type WorkforceSetupRuleClass,
  type WorkforceSetupRulePage,
  type WorkforceSetupRuleRecord,
  type WorkforceSetupRuleReference,
  type WorkforceSetupRuleScopeSummary,
  type WorkforceSetupScopePart,
} from "./api/generated-domain-pack-contracts";
import {
  rebaseEntityDraft,
  resolveEntityDraftField,
  sameField,
  type EntityField,
  type EntityRebase,
} from "./entity-draft";
import {
  createRuleDraft,
  isEditableRule,
  ruleDraftValue,
  ruleRebaseValue,
  type EditableRule,
  type EditableRuleKind,
  type RuleDraft,
} from "./rule-draft";
import { useSetupCommandReview, type SetupCommandSnapshot } from "./setup-command-review";
import {
  isOperationCancelled,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "./project-home";
import { messages } from "./messages";
import {
  scopeEntities,
  scopeEntityKinds,
  type ScopeEntityField,
  type ScopeEntityOptions,
  type ScopePreviewIntent,
  type ScopePreviewState,
} from "./components/planner/scope-field";
import type { LabeledEntityRef } from "./components/explanations/types";

interface Context {
  readonly scenarioId: string;
  readonly revision: number;
  readonly libraryEpoch: number;
}
interface RuleEditor {
  readonly context: Context;
  readonly id: string;
  readonly kind: EditableRuleKind;
  readonly base: EditableRule | null;
  readonly raw: RuleDraft;
  readonly editing: boolean;
  readonly rebase: EntityRebase<EditableRule> | null;
  readonly rebaseContext: Context | null;
}
interface ScopeState {
  readonly requestKey: string;
  readonly axis: "people" | "shifts";
  readonly preview: ScopePreviewState;
}
interface PickerState extends ScopeEntityOptions {
  readonly continuation: WorkforceSetupEntityContinuation | null;
}
interface ReviewedRule {
  readonly snapshot: SetupCommandSnapshot;
  readonly draftGeneration: number;
  readonly action: "create" | "update" | "activation" | "delete";
  readonly before: EditableRule | null;
  readonly after: EditableRule | null;
  readonly summary: WorkforceSetupRuleScopeSummary | null;
  readonly scopeError: string | null;
}

const descriptorKinds: Readonly<Record<string, EditableRuleKind>> = {
  "official.workforce.rule.eligibility": "eligibility",
  "official.workforce.rule.availability": "availability",
  "official.workforce.rule.coverage": "coverage",
  "official.workforce.rule.no-overlap": "noOverlap",
  "official.workforce.rule.minimum-rest": "minimumRest",
};
const scopeParts = ["main", "minimumRestBefore", "minimumRestAfter"] as const;
const entityFields = ["people", "teamIds", "assignmentTypeIds", "locationIds"] as const;
type PickerKey = `${WorkforceSetupScopePart}:${ScopeEntityField}`;

/** One route-local owner for drafts, bounded reads and the shared command approval. */
export function useRuleSetup(home: ProjectHomeController, project: () => ProjectSummary) {
  let alive = true;
  let sequence = 0;
  const requestKey = () => `rule-read-${String(++sequence)}`;
  const emptyScopes = (): Record<WorkforceSetupScopePart, ScopeState> => ({
    main: { requestKey: requestKey(), axis: "people", preview: { status: "idle" } },
    minimumRestBefore: { requestKey: requestKey(), axis: "people", preview: { status: "idle" } },
    minimumRestAfter: { requestKey: requestKey(), axis: "people", preview: { status: "idle" } },
  });
  const state = shallowReactive<{
    catalog: WorkforceSetupRuleCatalog | null;
    facts: WorkforceSetupFacts | null;
    catalogLoading: boolean;
    catalogError: string | null;
    classFilter: WorkforceSetupRuleClass;
    kindFilter: string | null;
    page: WorkforceSetupRulePage | null;
    pageLoading: boolean;
    pageError: string | null;
    detail: WorkforceSetupRuleRecord | null;
    detailLoading: boolean;
    detailError: string | null;
    editor: RuleEditor | null;
    draftGeneration: number;
    errors: Readonly<Record<string, string>>;
    editorError: string | null;
    reviewed: ReviewedRule | null;
    scopes: Readonly<Record<WorkforceSetupScopePart, ScopeState>>;
    pickers: Readonly<Partial<Record<PickerKey, PickerState>>>;
  }>({
    catalog: null,
    facts: null,
    catalogLoading: false,
    catalogError: null,
    classFilter: "required",
    kindFilter: null,
    page: null,
    pageLoading: false,
    pageError: null,
    detail: null,
    detailLoading: false,
    detailError: null,
    editor: null,
    draftGeneration: 0,
    errors: {},
    editorError: null,
    reviewed: null,
    scopes: emptyScopes(),
    pickers: {},
  });
  const context = (): Context => ({
    scenarioId: project().scenarioId,
    revision: project().revision,
    libraryEpoch: home.state.libraryEpoch,
  });
  const matches = (captured: Context) =>
    alive &&
    captured.scenarioId === project().scenarioId &&
    captured.revision === project().revision &&
    captured.libraryEpoch === home.state.libraryEpoch;
  const reads = new Map<
    string,
    { readonly context: Context; readonly scope: SetupOperationScope }
  >();
  const timers = new Map<PickerKey, ReturnType<typeof setTimeout>>();
  function beginRead(name: string) {
    reads.get(name)?.scope.dispose();
    const captured = context();
    const read = {
      context: captured,
      scope: new SetupOperationScope(captured.scenarioId, captured.revision),
    };
    reads.set(name, read);
    return read;
  }
  function ownsRead(name: string, read: ReturnType<typeof beginRead>): boolean {
    return reads.get(name) === read && matches(read.context);
  }
  function finishRead(name: string, read: ReturnType<typeof beginRead>): void {
    read.scope.dispose();
    if (reads.get(name) === read) reads.delete(name);
  }
  function disposeReads(prefix: string): void {
    for (const [name, read] of reads) {
      if (!name.startsWith(prefix)) continue;
      read.scope.dispose();
      reads.delete(name);
    }
  }
  const review = useSetupCommandReview(home, project);
  const busy = computed(
    () => home.state.busyAction !== null || review.state.pending || state.detailLoading,
  );
  const dirty = computed(
    () =>
      state.editor !== null &&
      (state.editor.base === null ||
        !sameField(state.editor.raw, createRuleDraft(state.editor.base))),
  );
  const stale = computed(() => state.editor !== null && !matches(state.editor.context));
  const canReplace = computed(
    () =>
      !busy.value && !dirty.value && state.editor?.editing !== true && review.state.review === null,
  );
  const conflictsCurrent = computed(
    () =>
      state.editor?.rebaseContext !== null &&
      state.editor?.rebaseContext !== undefined &&
      matches(state.editor.rebaseContext),
  );
  const canApply = computed(() => {
    const reviewed = state.reviewed;
    if (
      busy.value ||
      stale.value ||
      reviewed === null ||
      reviewed.snapshot !== review.state.review?.snapshot ||
      reviewed.draftGeneration !== state.draftGeneration ||
      !matches(reviewed.snapshot) ||
      home.state.mutation?.outcome === "outcomeUnknown"
    )
      return false;
    if (reviewed.after === null || !reviewed.after.active) return true;
    const population = reviewed.summary?.population;
    if (population === undefined || population.peopleCount === 0) return false;
    return reviewed.after.kind === "minimumRest"
      ? population.kind === "minimumRest" &&
          population.beforeShiftCount > 0 &&
          population.afterShiftCount > 0
      : population.kind === "ordinary" && population.shiftCount > 0;
  });

  function invalidateEvidence(): void {
    review.invalidate();
    state.reviewed = null;
    state.scopes = emptyScopes();
    disposeReads("scope:");
    disposeReads("review-detail");
  }
  function invalidateDraft(): void {
    state.draftGeneration += 1;
    invalidateEvidence();
    state.errors = {};
    state.editorError = null;
  }
  function resetPickers(): void {
    for (const timer of timers.values()) clearTimeout(timer);
    timers.clear();
    disposeReads("picker:");
    state.pickers = {};
  }
  function discard(): void {
    invalidateDraft();
    disposeReads("detail");
    state.detailLoading = false;
    state.detailError = null;
    state.detail = null;
    state.editor = null;
    resetPickers();
  }
  function kindFor(entry: WorkforceSetupRuleCatalogEntry): EditableRuleKind | null {
    return entry.support === "implemented" ? (descriptorKinds[entry.descriptor.id] ?? null) : null;
  }
  function reference(editor: RuleEditor): WorkforceSetupRuleReference {
    return { class: "required", ruleId: editor.id };
  }
  function nativeErrors(failure: unknown, id: string): void {
    if (
      typeof failure !== "object" ||
      failure === null ||
      !("fieldErrors" in failure) ||
      !Array.isArray(failure.fieldErrors)
    )
      return;
    const errors = new Map<string, string>();
    for (const item of failure.fieldErrors as readonly unknown[]) {
      if (
        typeof item !== "object" ||
        item === null ||
        !("field" in item) ||
        typeof item.field !== "string" ||
        !("message" in item) ||
        typeof item.message !== "string"
      )
        continue;
      let field = item.field.replaceAll("/", ".").replace(/^\./, "");
      const prefix = `domain.rules.${id}.`;
      if (field.startsWith(prefix)) field = field.slice(prefix.length);
      errors.set(field, item.message);
    }
    state.errors = Object.fromEntries(errors);
  }

  async function loadCatalog(): Promise<void> {
    const read = beginRead("catalog");
    state.catalogLoading = true;
    state.catalogError = null;
    try {
      const [catalog, facts] = await Promise.all([
        getScenarioView(read.scope, {
          source: { kind: "stored" },
          query: { schemaVersion: 1, viewId: "eutheto.setup.rule_catalog", parameters: {} },
        }).result,
        getScenarioView(read.scope, {
          source: { kind: "stored" },
          query: { schemaVersion: 1, viewId: "official.workforce.setup.overview", parameters: {} },
        }).result,
      ]);
      if (!ownsRead("catalog", read)) return;
      state.catalog = catalog.result.view.data.result.data;
      state.facts = facts.result.view.data.result.data;
    } catch (failure) {
      if (ownsRead("catalog", read)) state.catalogError = safeMessage(failure);
    } finally {
      if (ownsRead("catalog", read)) state.catalogLoading = false;
      finishRead("catalog", read);
    }
  }
  async function loadPage(
    continuation: WorkforceSetupRulePage["continuation"] = null,
  ): Promise<void> {
    const read = beginRead("page");
    const classFilter = state.classFilter;
    const kindFilter = state.kindFilter;
    state.pageLoading = true;
    state.pageError = null;
    try {
      const response = await getScenarioView(read.scope, {
        source: { kind: "stored" },
        query: {
          schemaVersion: 1,
          viewId: WORKFORCE_SETUP_RULE_PAGE_QUERY_ID,
          parameters: { class: classFilter, kindId: kindFilter, limit: 50 },
          continuation,
        },
      }).result;
      if (
        ownsRead("page", read) &&
        classFilter === state.classFilter &&
        kindFilter === state.kindFilter
      )
        state.page = response.result.view.data.result.data;
    } catch (failure) {
      if (ownsRead("page", read)) state.pageError = safeMessage(failure);
    } finally {
      if (ownsRead("page", read)) state.pageLoading = false;
      finishRead("page", read);
    }
  }
  async function refresh(): Promise<void> {
    await Promise.all([loadCatalog(), loadPage()]);
  }
  function setFilter(ruleClass: WorkforceSetupRuleClass, kindId: string | null): void {
    state.classFilter = ruleClass;
    state.kindFilter = kindId;
    state.page = null;
    void loadPage();
  }
  async function loadDetail(rule: WorkforceSetupRuleReference): Promise<boolean> {
    if (!canReplace.value) return false;
    discard();
    const generation = state.draftGeneration;
    const read = beginRead("detail");
    state.detailLoading = true;
    try {
      const response = await getScenarioView(read.scope, {
        source: { kind: "stored" },
        query: {
          schemaVersion: 1,
          viewId: WORKFORCE_SETUP_RULE_DETAIL_QUERY_ID,
          parameters: { rule },
        },
      }).result;
      if (!ownsRead("detail", read) || generation !== state.draftGeneration) return false;
      const detail = response.result.view.data.result.data;
      if (detail.class !== rule.class || detail.record.id !== rule.ruleId)
        throw new Error("The native rule did not match the requested identity.");
      state.detail = detail;
      if (detail.class === "required" && isEditableRule(detail.record)) {
        state.editor = {
          context: read.context,
          id: detail.record.id,
          kind: detail.record.kind,
          base: detail.record,
          raw: createRuleDraft(detail.record),
          editing: false,
          rebase: null,
          rebaseContext: null,
        };
      }
      return true;
    } catch (failure) {
      if (ownsRead("detail", read)) state.detailError = safeMessage(failure);
      return false;
    } finally {
      if (ownsRead("detail", read)) state.detailLoading = false;
      finishRead("detail", read);
    }
  }
  function createRule(entry: WorkforceSetupRuleCatalogEntry): boolean {
    const kind = kindFor(entry);
    if (
      !canReplace.value ||
      kind === null ||
      state.facts === null ||
      !state.catalog?.required.includes(entry)
    )
      return false;
    discard();
    state.editor = {
      context: context(),
      id: newUuidV7(),
      kind,
      base: null,
      raw: createRuleDraft("new"),
      editing: true,
      rebase: null,
      rebaseContext: null,
    };
    return true;
  }
  function editRule(): boolean {
    const editor = state.editor;
    if (busy.value || stale.value || editor === null || editor.base === null) return false;
    invalidateDraft();
    state.editor = { ...editor, editing: true };
    return true;
  }
  function updateRaw(raw: RuleDraft): void {
    const editor = state.editor;
    if (busy.value || editor === null || !editor.editing || editor.rebase !== null) return;
    invalidateDraft();
    state.editor = { ...editor, raw };
  }
  function scopeValue(part: WorkforceSetupScopePart): WorkforceScope {
    const raw = state.editor?.raw;
    if (raw === undefined) return { people: { kind: "all" } };
    return part === "main"
      ? raw.scope
      : part === "minimumRestBefore"
        ? raw.beforeScope
        : raw.afterScope;
  }
  function updateScope(part: WorkforceSetupScopePart, value: WorkforceScope): void {
    const raw = state.editor?.raw;
    if (raw === undefined) return;
    updateRaw(
      part === "main"
        ? { ...raw, scope: value }
        : part === "minimumRestBefore"
          ? { ...raw, beforeScope: value }
          : { ...raw, afterScope: value },
    );
  }

  async function preview(action: ReviewedRule["action"]): Promise<boolean> {
    const editor = state.editor;
    if (busy.value || stale.value || editor === null || editor.rebase !== null) return false;
    if (action !== "create" && editor.base === null) return false;
    if ((action === "activation" || action === "delete") && editor.editing) return false;
    if ((action === "create" || action === "update") && !editor.editing) return false;
    invalidateEvidence();
    state.errors = {};
    state.editorError = null;
    const parsed = ruleDraftValue(editor.id, editor.kind, editor.raw);
    const after =
      action === "delete"
        ? null
        : action === "activation" && editor.base !== null
          ? { ...editor.base, active: !editor.base.active }
          : parsed.value;
    if (action !== "delete" && after === null) {
      state.errors = parsed.errors;
      return false;
    }
    const command: ScenarioCommand = {
      type: "applyDomainCommand",
      payload:
        action === "delete"
          ? { commandType: WORKFORCE_REMOVE_RULE_COMMAND_ID, payload: { ruleId: editor.id } }
          : {
              commandType:
                action === "create"
                  ? WORKFORCE_ADD_RULE_COMMAND_ID
                  : WORKFORCE_UPDATE_RULE_COMMAND_ID,
              payload: { rule: after },
            },
    };
    const generation = state.draftGeneration;
    const captured = context();
    const owns = () =>
      matches(captured) && state.editor === editor && generation === state.draftGeneration;
    if (!(await review.preview(command, null))) {
      if (owns()) nativeErrors(review.state.failure, editor.id);
      return false;
    }
    const snapshot = review.state.review?.snapshot;
    if (!owns() || snapshot === undefined) return false;
    const base = { snapshot, draftGeneration: generation, action, before: editor.base };
    if (after === null) {
      state.reviewed = { ...base, after: null, summary: null, scopeError: null };
      return true;
    }
    const read = beginRead("review-detail");
    let cancelled = false;
    const isCancelled = () => cancelled;
    let cancelCurrent: (() => Promise<unknown>) | null = null;
    const current = () =>
      owns() && ownsRead("review-detail", read) && review.state.review?.snapshot === snapshot;
    try {
      const response = await home.runOperation<ReviewedRule | null>({
        action: "rule-scope-review",
        label: messages.ruleSetup.previewLoading,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
          await cancelCurrent?.();
        },
        execute: async (report) => {
          const source = { kind: "commandPreview" as const, command: snapshot.command };
          const detailOperation = getScenarioView(
            read.scope,
            {
              source,
              query: {
                schemaVersion: 1,
                viewId: WORKFORCE_SETUP_RULE_DETAIL_QUERY_ID,
                parameters: { rule: reference(editor) },
              },
            },
            report,
          );
          cancelCurrent = () => detailOperation.cancel();
          const detailResponse = await detailOperation.result;
          if (isCancelled() || !current()) return { ...detailResponse, result: null };
          const detail = detailResponse.result.view.data.result.data;
          if (
            detail.class !== "required" ||
            detail.record.id !== editor.id ||
            detail.record.kind !== editor.kind ||
            !isEditableRule(detail.record)
          )
            throw new Error(
              "The native proposed rule did not match the reviewed identity and kind.",
            );
          const proposed = detail.record;
          const summaryOperation = getScenarioView(
            read.scope,
            {
              source,
              query: {
                schemaVersion: 1,
                viewId: "official.workforce.setup.rule_scope_summary",
                parameters: { rule: reference(editor) },
              },
            },
            report,
          );
          cancelCurrent = () => summaryOperation.cancel();
          try {
            const summaryResponse = await summaryOperation.result;
            const summary = summaryResponse.result.view.data.result.data;
            if (
              !sameField(summary.rule, reference(editor)) ||
              (proposed.kind === "minimumRest") !== (summary.population.kind === "minimumRest")
            )
              throw new Error("The native scope did not match the reviewed rule.");
            return {
              ...summaryResponse,
              result: { ...base, after: proposed, summary, scopeError: null },
            };
          } catch (failure) {
            if (isOperationCancelled(failure)) throw failure;
            return {
              ...detailResponse,
              result: { ...base, after: proposed, summary: null, scopeError: safeMessage(failure) },
            };
          }
        },
        success: () => "",
      });
      if (!current() || isCancelled() || response.result === null) return false;
      state.reviewed = response.result;
      return true;
    } catch (failure) {
      if (current())
        state.editorError = isOperationCancelled(failure) ? null : safeMessage(failure);
      return false;
    } finally {
      finishRead("review-detail", read);
    }
  }
  async function save(truncateRedo = false): Promise<boolean> {
    if (!canApply.value) return false;
    const editor = state.editor;
    const generation = state.draftGeneration;
    const scenarioId = project().scenarioId;
    const receipt = await review.apply(truncateRedo);
    if (receipt === null) return false;
    // A write receipt wins over its own revision event, not over another editor invocation.
    if (
      alive &&
      project().scenarioId === scenarioId &&
      state.editor === editor &&
      state.draftGeneration === generation
    ) {
      discard();
      const ownsView = () => alive && project().scenarioId === scenarioId && state.editor === null;
      await refresh();
      return ownsView();
    }
    return false;
  }

  function acceptRebase(
    editor: RuleEditor,
    merged: EntityRebase<EditableRule>,
    captured: Context,
    local: EditableRule,
  ): void {
    invalidateDraft();
    state.editor = {
      ...editor,
      raw: createRuleDraft(merged.value, { value: local, raw: editor.raw }),
      context: merged.conflicts.length === 0 ? captured : editor.context,
      base: merged.conflicts.length === 0 ? merged.current : editor.base,
      rebase: merged.conflicts.length === 0 ? null : merged,
      rebaseContext: merged.conflicts.length === 0 ? null : captured,
    };
  }
  async function rebase(): Promise<void> {
    const editor = state.editor;
    if (busy.value || editor === null) return;
    invalidateEvidence();
    state.editorError = null;
    if (editor.base === null) {
      invalidateDraft();
      state.editor = { ...editor, context: context() };
      return;
    }
    const local = ruleRebaseValue(editor.rebase?.value ?? editor.base, editor.raw);
    if (local === null) {
      state.errors = ruleDraftValue(editor.id, editor.kind, editor.raw).errors;
      return;
    }
    const generation = state.draftGeneration;
    const read = beginRead("detail");
    state.detailLoading = true;
    try {
      const response = await getScenarioView(read.scope, {
        source: { kind: "stored" },
        query: {
          schemaVersion: 1,
          viewId: WORKFORCE_SETUP_RULE_DETAIL_QUERY_ID,
          parameters: { rule: reference(editor) },
        },
      }).result;
      if (
        !ownsRead("detail", read) ||
        state.editor !== editor ||
        generation !== state.draftGeneration
      )
        return;
      const detail = response.result.view.data.result.data;
      if (
        detail.class !== "required" ||
        detail.record.id !== editor.id ||
        detail.record.kind !== editor.kind ||
        !isEditableRule(detail.record)
      )
        throw new Error("The current saved rule no longer has this identity and kind.");
      state.detail = detail;
      if (!editor.editing) {
        invalidateDraft();
        state.editor = {
          ...editor,
          context: read.context,
          base: detail.record,
          raw: createRuleDraft(detail.record),
          rebase: null,
          rebaseContext: null,
        };
      } else {
        acceptRebase(
          editor,
          rebaseEntityDraft(editor.base, local, detail.record, editor.rebase),
          read.context,
          local,
        );
      }
    } catch (failure) {
      if (ownsRead("detail", read)) state.editorError = safeMessage(failure);
    } finally {
      if (ownsRead("detail", read)) state.detailLoading = false;
      finishRead("detail", read);
    }
  }
  function chooseConflict(field: EntityField<EditableRule>, choice: "current" | "draft"): void {
    const editor = state.editor;
    if (
      busy.value ||
      editor?.rebase === null ||
      editor?.rebase === undefined ||
      editor.rebaseContext === null ||
      !matches(editor.rebaseContext)
    )
      return;
    const local = ruleRebaseValue(editor.rebase.value, editor.raw);
    if (local === null) return;
    const merged = resolveEntityDraftField(editor.rebase, field, choice);
    const retained =
      field === "minimumMinutes" && choice === "current"
        ? {
            ...editor,
            raw: { ...editor.raw, minimumRest: createRuleDraft(merged.value).minimumRest },
          }
        : editor;
    acceptRebase(retained, merged, editor.rebaseContext, local);
  }

  function scopeSource(): {
    readonly source: SetupSourceV2;
    readonly editor: RuleEditor;
    readonly snapshot: SetupCommandSnapshot | null;
  } | null {
    const editor = state.editor;
    if (editor === null || stale.value) return null;
    const reviewed = state.reviewed;
    if (
      reviewed !== null &&
      reviewed.snapshot === review.state.review?.snapshot &&
      reviewed.after !== null
    )
      return {
        source: { kind: "commandPreview", command: reviewed.snapshot.command },
        editor,
        snapshot: reviewed.snapshot,
      };
    if (editor.base !== null && !dirty.value && review.state.review === null)
      return { source: { kind: "stored" }, editor, snapshot: null };
    return null;
  }
  async function inspectScope(
    part: WorkforceSetupScopePart,
    intent: ScopePreviewIntent,
  ): Promise<void> {
    if (intent.requestKey !== state.scopes[part].requestKey) return;
    const source = scopeSource();
    const key = requestKey();
    const axis = intent.axis;
    state.scopes = {
      ...state.scopes,
      [part]: { requestKey: key, axis, preview: { status: "loading", requestKey: key } },
    };
    if (source === null) {
      state.scopes = {
        ...state.scopes,
        [part]: {
          requestKey: key,
          axis,
          preview: { status: "error", requestKey: key, message: messages.ruleSetup.noPreview },
        },
      };
      return;
    }
    const name = `scope:${part}`;
    const read = beginRead(name);
    const generation = state.draftGeneration;
    const current = () =>
      ownsRead(name, read) &&
      generation === state.draftGeneration &&
      state.scopes[part].requestKey === key &&
      state.editor === source.editor &&
      (source.snapshot === null
        ? scopeSource()?.snapshot === null
        : review.state.review?.snapshot === source.snapshot);
    try {
      const response = await getScenarioView(read.scope, {
        source: source.source,
        query: {
          schemaVersion: 1,
          viewId: "official.workforce.setup.rule_scope",
          parameters: { rule: reference(source.editor), part, axis, limit: 50 },
          continuation: intent.kind === "nextPage" ? intent.continuation : null,
        },
      }).result;
      if (!current()) return;
      const inspection = response.result.view.data.result.data;
      if (
        !sameField(inspection.rule, reference(source.editor)) ||
        inspection.part !== part ||
        inspection.population.axis !== axis
      )
        throw new Error(
          "The native scope page did not match the requested rule, part and population.",
        );
      state.scopes = {
        ...state.scopes,
        [part]: {
          requestKey: key,
          axis,
          preview: { status: "ready", requestKey: key, inspection },
        },
      };
    } catch (failure) {
      if (current())
        state.scopes = {
          ...state.scopes,
          [part]: {
            requestKey: key,
            axis,
            preview: { status: "error", requestKey: key, message: safeMessage(failure) },
          },
        };
    } finally {
      finishRead(name, read);
    }
  }

  function picker(part: WorkforceSetupScopePart, field: ScopeEntityField): PickerState {
    return (
      state.pickers[`${part}:${field}`] ?? {
        requestKey: "",
        query: "",
        page: { status: "idle" },
        selectedOptions: [],
        continuation: null,
      }
    );
  }
  function entityOptions(
    part: WorkforceSetupScopePart,
  ): Readonly<Record<ScopeEntityField, ScopeEntityOptions>> {
    return {
      people: picker(part, "people"),
      teamIds: picker(part, "teamIds"),
      assignmentTypeIds: picker(part, "assignmentTypeIds"),
      locationIds: picker(part, "locationIds"),
    };
  }
  function searchEntity(
    part: WorkforceSetupScopePart,
    field: ScopeEntityField,
    query: string,
    next = false,
    immediate = false,
  ): void {
    const editor = state.editor;
    if (editor === null) return;
    const key: PickerKey = `${part}:${field}`;
    clearTimeout(timers.get(key));
    timers.delete(key);
    const previous = picker(part, field);
    const continuation =
      next && previous.page.status === "ready" && previous.query === query
        ? previous.continuation
        : null;
    if (next && continuation === null) return;
    const token = requestKey();
    const name = `picker:${key}`;
    disposeReads(name);
    const retained = previous.selectedOptions;
    state.pickers = {
      ...state.pickers,
      [key]: {
        requestKey: token,
        query,
        continuation: null,
        selectedOptions: retained,
        page: { status: "loading", requestKey: token, query },
      },
    };
    const run = async () => {
      timers.delete(key);
      if (!alive || state.editor?.id !== editor.id || picker(part, field).requestKey !== token)
        return;
      const read = beginRead(name);
      const current = () =>
        ownsRead(name, read) &&
        state.editor?.id === editor.id &&
        picker(part, field).requestKey === token;
      try {
        const response = await searchScenarioEntities(read.scope, {
          kind: scopeEntityKinds[field],
          search: query,
          limit: 50,
          cursor: continuation,
        }).result;
        if (!current()) return;
        const page = response.result.view.data.result.data;
        if (page.items.some((item) => item.kind !== scopeEntityKinds[field]))
          throw new Error("The native picker returned a different entity kind.");
        const options: readonly LabeledEntityRef[] = page.items.map((item) => ({
          entity: { kind: item.kind, id: item.entityId },
          label: item.name ?? item.entityId,
        }));
        const selected = new Set(scopeEntities(scopeValue(part), field).map((entity) => entity.id));
        const labels = new Map(
          [...retained, ...options]
            .filter((item) => selected.has(item.entity.id))
            .map((item) => [item.entity.id, item]),
        );
        state.pickers = {
          ...state.pickers,
          [key]: {
            requestKey: token,
            query,
            continuation: page.continuation,
            selectedOptions: [...labels.values()],
            page: {
              status: "ready",
              requestKey: token,
              query,
              options,
              hasMore: page.continuation !== null,
            },
          },
        };
      } catch (failure) {
        if (current())
          state.pickers = {
            ...state.pickers,
            [key]: {
              requestKey: token,
              query,
              continuation: null,
              selectedOptions: retained,
              page: { status: "error", requestKey: token, query, message: safeMessage(failure) },
            },
          };
      } finally {
        finishRead(name, read);
      }
    };
    if (immediate || next) void run();
    else
      timers.set(
        key,
        setTimeout(() => void run(), 150),
      );
  }
  // Capture labels at selection time before a later query replaces its page.
  watch(
    () => state.editor?.raw,
    () => {
      let pickers = state.pickers;
      for (const part of scopeParts)
        for (const field of entityFields) {
          const key: PickerKey = `${part}:${field}`;
          const current = pickers[key];
          if (current === undefined) continue;
          const selected = new Set(
            scopeEntities(scopeValue(part), field).map((entity) => entity.id),
          );
          const options = current.page.status === "ready" ? current.page.options : [];
          const labels = new Map(
            [...current.selectedOptions, ...options]
              .filter((item) => selected.has(item.entity.id))
              .map((item) => [item.entity.id, item]),
          );
          pickers = { ...pickers, [key]: { ...current, selectedOptions: [...labels.values()] } };
        }
      state.pickers = pickers;
    },
    { flush: "sync" },
  );

  watch(
    [() => project().scenarioId, () => project().revision, () => home.state.libraryEpoch],
    (current, previous) => {
      disposeReads("");
      resetPickers();
      invalidateEvidence();
      state.catalog = null;
      state.facts = null;
      state.page = null;
      state.detail = null;
      state.catalogLoading = false;
      state.pageLoading = false;
      state.detailLoading = false;
      state.catalogError = null;
      state.pageError = null;
      state.detailError = null;
      if (current[0] !== previous[0]) discard();
      if (project().domainPackId === "official.workforce") void refresh();
    },
    { immediate: true, flush: "sync" },
  );
  onScopeDispose(() => {
    alive = false;
    disposeReads("");
    resetPickers();
    invalidateEvidence();
  });

  return {
    state,
    review,
    busy,
    dirty,
    stale,
    canReplace,
    canApply,
    conflictsCurrent,
    kindFor,
    refresh,
    loadPage,
    setFilter,
    loadDetail,
    createRule,
    editRule,
    discard,
    updateRaw,
    scopeValue,
    updateScope,
    preview,
    save,
    rebase,
    chooseConflict,
    inspectScope,
    picker,
    entityOptions,
    searchEntity,
  };
}
