import { computed, onScopeDispose, shallowReactive, watch } from "vue";
import {
  getScenarioEntity,
  getScenarioView,
  searchScenarioEntities,
  SetupOperationScope,
} from "./api/generated";
import type {
  WorkforcePerson,
  WorkforceSetupEligibilityMatrix,
  WorkforceSetupEntityPage,
  WorkforceSetupPersonScopePage,
} from "./api/generated-domain-pack-contracts";
import {
  ELIGIBILITY_PERSON_BYTES,
  eligibilityBatchCommand,
  eligibilityCellKey,
  type EligibilityEdits,
  type EligibilityReviewCell,
} from "./eligibility-draft";
import { sameField } from "./entity-draft";
import { messages } from "./messages";
import {
  isOperationCancelled,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "./project-home";
import { useSetupCommandReview } from "./setup-command-review";

export interface EligibilityContext {
  readonly scenarioId: string;
  readonly revision: number;
  readonly epoch: number;
}
interface EligibilityFilters {
  readonly peopleSearch: string;
  readonly typeSearch: string;
  readonly qualificationId: string | null;
}
export interface EligibilityWindow {
  readonly context: EligibilityContext;
  readonly filters: EligibilityFilters;
  readonly people: WorkforceSetupPersonScopePage;
  readonly types: WorkforceSetupEntityPage;
  readonly matrix: WorkforceSetupEligibilityMatrix;
}
function checkMatrix(
  matrix: WorkforceSetupEligibilityMatrix,
  people: readonly string[],
  types: readonly string[],
): void {
  if (
    !sameField(matrix.personIds, people) ||
    !sameField(matrix.assignmentTypeIds, types) ||
    matrix.configuredMemberships.length !== people.length ||
    matrix.configuredMemberships.some((row) => row.length !== types.length)
  ) {
    throw new Error(messages.eligibilityUi.operation.errors.matrixAxes);
  }
}

export function useEligibilitySetup(home: ProjectHomeController, project: () => ProjectSummary) {
  const state = shallowReactive<{
    peopleSearch: string;
    typeSearch: string;
    qualificationId: string | null;
    window: EligibilityWindow | null;
    loading: boolean;
    preparing: boolean;
    error: string | null;
    edits: EligibilityEdits;
    selectedPersonId: string | null;
    selectedTypeId: string | null;
    approval: {
      readonly context: EligibilityContext;
      readonly cells: readonly EligibilityReviewCell[];
      readonly personBytes: number;
    } | null;
  }>({
    peopleSearch: "",
    typeSearch: "",
    qualificationId: null,
    window: null,
    loading: false,
    preparing: false,
    error: null,
    edits: new Map(),
    selectedPersonId: null,
    selectedTypeId: null,
    approval: null,
  });
  const review = useSetupCommandReview(home, project);
  let alive = true,
    readGeneration = 0,
    prepareGeneration = 0,
    draftGeneration = 0;
  let readScope: SetupOperationScope | null = null,
    prepareScope: SetupOperationScope | null = null;
  let appliedFilters: EligibilityFilters = {
    peopleSearch: "",
    typeSearch: "",
    qualificationId: null,
  };
  function context(): EligibilityContext {
    return {
      scenarioId: project().scenarioId,
      revision: project().revision,
      epoch: home.state.libraryEpoch,
    };
  }
  function matches(value: EligibilityContext): boolean {
    return alive && sameField(value, context());
  }
  const dirty = computed(() => state.edits.size > 0);
  const busy = computed(
    () =>
      state.loading || state.preparing || review.state.pending || home.state.busyAction !== null,
  );
  const unresolved = computed(() => home.state.mutation?.outcome === "outcomeUnknown");
  const stale = computed(() => state.window !== null && !matches(state.window.context));
  const pendingCells = computed<readonly EligibilityReviewCell[]>(() => {
    const people = new Map(
      state.window?.people.items.map((person) => [person.personId, person.name]),
    );
    const types = new Map(
      state.window?.types.items.map((type) => [type.entityId, type.name ?? type.entityId]),
    );
    return [...state.edits.values()].map((edit) => ({
      ...edit,
      personName: people.get(edit.personId) ?? edit.personId,
      typeName: types.get(edit.typeId) ?? edit.typeId,
    }));
  });
  function invalidateApproval(): void {
    prepareGeneration += 1;
    prepareScope?.dispose();
    prepareScope = null;
    state.approval = null;
    review.invalidate();
  }
  function discard(): void {
    draftGeneration += 1;
    invalidateApproval();
    state.edits = new Map();
    state.error = null;
  }
  async function loadWindow(axis: "people" | "types" | null = null, search = false): Promise<void> {
    if (!alive || dirty.value || busy.value || unresolved.value) return;
    const previous = state.window;
    if (axis !== null && (previous === null || !matches(previous.context))) return;
    if (search)
      appliedFilters = {
        peopleSearch: state.peopleSearch,
        typeSearch: state.typeSearch,
        qualificationId: state.qualificationId,
      };
    const filters = appliedFilters,
      captured = context(),
      generation = ++readGeneration;
    readScope?.dispose();
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    readScope = owned;
    state.window = null;
    state.loading = true;
    state.error = null;
    try {
      const people =
        axis === "types" && previous !== null
          ? previous.people
          : (
              await getScenarioView(owned, {
                source: { kind: "stored" },
                query: {
                  schemaVersion: 1,
                  viewId: "official.workforce.setup.people_page",
                  parameters: {
                    search: filters.peopleSearch,
                    limit: 128,
                    ...(filters.qualificationId === null
                      ? {}
                      : { qualificationId: filters.qualificationId }),
                  },
                  continuation: axis === "people" ? (previous?.people.continuation ?? null) : null,
                },
              }).result
            ).result.view.data.result.data;
      if (generation !== readGeneration || !matches(captured)) return;
      const types =
        axis === "people" && previous !== null
          ? previous.types
          : (
              await searchScenarioEntities(owned, {
                kind: "assignmentType",
                search: filters.typeSearch,
                limit: 64,
                cursor: axis === "types" ? (previous?.types.continuation ?? null) : null,
              }).result
            ).result.view.data.result.data;
      if (generation !== readGeneration || !matches(captured)) return;
      const personIds = people.items.map((person) => person.personId),
        assignmentTypeIds = types.items.map((type) => type.entityId);
      if (
        personIds.length > 128 ||
        assignmentTypeIds.length > 64 ||
        new Set(personIds).size !== personIds.length ||
        new Set(assignmentTypeIds).size !== assignmentTypeIds.length ||
        types.items.some((type) => type.kind !== "assignmentType")
      )
        throw new Error(messages.eligibilityUi.operation.errors.axisBounds);
      // The native matrix query also owns empty-axis semantics.
      const matrix = (
        await getScenarioView(owned, {
          source: { kind: "stored" },
          query: {
            schemaVersion: 1,
            viewId: "official.workforce.setup.eligibility_matrix",
            parameters: { personIds, assignmentTypeIds },
          },
        }).result
      ).result.view.data.result.data;
      checkMatrix(matrix, personIds, assignmentTypeIds);
      if (generation === readGeneration && matches(captured))
        state.window = { context: captured, filters, people, types, matrix };
    } catch (failure) {
      if (generation === readGeneration && matches(captured)) state.error = safeMessage(failure);
    } finally {
      owned.dispose();
      if (readScope === owned) readScope = null;
      if (generation === readGeneration) state.loading = false;
    }
  }
  function setMembership(
    personIds: readonly string[],
    typeIds: readonly string[],
    after: boolean,
    retainNoop = false,
  ): void {
    const window = state.window;
    if (window === null || stale.value || busy.value || unresolved.value) return;
    const people = new Map(window.matrix.personIds.map((id, index) => [id, index]));
    const types = new Map(window.matrix.assignmentTypeIds.map((id, index) => [id, index]));
    const next = new Map(state.edits);
    for (const personId of personIds)
      for (const typeId of typeIds) {
        const pi = people.get(personId),
          ti = types.get(typeId);
        if (pi === undefined || ti === undefined) return;
        const before = window.matrix.configuredMemberships[pi]?.[ti];
        if (before === undefined) return;
        const key = eligibilityCellKey(personId, typeId);
        if (before === after && !retainNoop) next.delete(key);
        else next.set(key, { personId, typeId, before, after });
      }
    draftGeneration += 1;
    invalidateApproval();
    state.edits = next;
  }
  function removeChoice(key: string): void {
    if (busy.value || unresolved.value) return;
    const next = new Map(state.edits);
    next.delete(key);
    draftGeneration += 1;
    invalidateApproval();
    state.edits = next;
  }
  async function preview(): Promise<boolean> {
    if (!dirty.value || busy.value || unresolved.value || state.window === null) return false;
    invalidateApproval();
    const captured = context(),
      generation = prepareGeneration,
      edits = state.edits;
    const personIds = [...new Set([...edits.values()].map((edit) => edit.personId))];
    const assignmentTypeIds = [...new Set([...edits.values()].map((edit) => edit.typeId))];
    const refreshTypes = !matches(state.window.context);
    const typeNames = new Map(
      refreshTypes
        ? []
        : state.window.types.items.map((type) => [type.entityId, type.name ?? type.entityId]),
    );
    const owned = new SetupOperationScope(captured.scenarioId, captured.revision);
    prepareScope = owned;
    state.preparing = true;
    state.error = null;
    let cancelled = false,
      cancelCurrent: (() => Promise<unknown>) | null = null;
    const current = (): boolean =>
      !cancelled && generation === prepareGeneration && matches(captured);
    const people = new Map<string, WorkforcePerson>();
    try {
      const prepared = await home.runOperation({
        action: "eligibility-review-capture",
        label: messages.eligibilityUi.operation.preparing,
        refreshLibrary: false,
        cancel: async () => {
          cancelled = true;
          await cancelCurrent?.();
        },
        success: () => (current() ? messages.eligibilityUi.operation.captured : ""),
        execute: async (report) => {
          const operation = getScenarioView(
            owned,
            {
              source: { kind: "stored" },
              query: {
                schemaVersion: 1,
                viewId: "official.workforce.setup.eligibility_matrix",
                parameters: { personIds, assignmentTypeIds },
              },
            },
            report,
          );
          cancelCurrent = () => operation.cancel();
          const response = await operation.result;
          const matrix = response.result.view.data.result.data;
          checkMatrix(matrix, personIds, assignmentTypeIds);
          let personBytes = 0;
          for (const personId of personIds) {
            if (!current())
              throw Object.assign(new Error(messages.eligibilityUi.operation.errors.cancelled), {
                code: "operation.cancelled",
                category: "protocol",
              });
            const detail = getScenarioEntity(owned, { kind: "person", entityId: personId }, report);
            cancelCurrent = () => detail.cancel();
            const person = (await detail.result).result.view.data.result.data;
            if (person.kind !== "person" || person.id !== personId)
              throw new Error(messages.eligibilityUi.operation.errors.personMismatch);
            // Charge exact compact UTF-8 bytes before retaining the next record.
            personBytes += new TextEncoder().encode(JSON.stringify(person)).byteLength;
            if (personBytes > ELIGIBILITY_PERSON_BYTES)
              throw Object.assign(new Error(messages.eligibilityUi.operation.errors.peopleLimit), {
                category: "validation",
                code: "desktop.eligibility.captureLimit",
              });
            people.set(personId, person);
          }
          if (refreshTypes) {
            for (const typeId of assignmentTypeIds) {
              if (!current())
                throw Object.assign(new Error(messages.eligibilityUi.operation.errors.cancelled), {
                  code: "operation.cancelled",
                  category: "protocol",
                });
              const detail = getScenarioEntity(
                owned,
                { kind: "assignmentType", entityId: typeId },
                report,
              );
              cancelCurrent = () => detail.cancel();
              const assignmentType = (await detail.result).result.view.data.result.data;
              if (assignmentType.kind !== "assignmentType" || assignmentType.id !== typeId)
                throw new Error(messages.eligibilityUi.operation.errors.typeMismatch);
              typeNames.set(typeId, assignmentType.name);
            }
          }
          if (!current())
            throw Object.assign(new Error(messages.eligibilityUi.operation.errors.cancelled), {
              code: "operation.cancelled",
              category: "protocol",
            });
          const pi = new Map(personIds.map((id, index) => [id, index])),
            ti = new Map(assignmentTypeIds.map((id, index) => [id, index]));
          const cells: EligibilityReviewCell[] = [...edits.values()].map((edit) => {
            const row = pi.get(edit.personId),
              column = ti.get(edit.typeId);
            const before =
              row === undefined || column === undefined
                ? undefined
                : matrix.configuredMemberships[row]?.[column];
            const person = people.get(edit.personId);
            if (before === undefined || person === undefined)
              throw new Error(messages.eligibilityUi.operation.errors.membershipUnavailable);
            return {
              ...edit,
              before,
              personName: person.name,
              typeName: typeNames.get(edit.typeId) ?? edit.typeId,
            };
          });
          return {
            ...response,
            result: { command: eligibilityBatchCommand(edits, people), cells, personBytes },
          };
        },
      });
      people.clear();
      if (!current()) return false;
      const ready = await review.preview(prepared.result.command, null);
      if (ready && current())
        state.approval = {
          context: captured,
          cells: prepared.result.cells,
          personBytes: prepared.result.personBytes,
        };
      return ready && current();
    } catch (failure) {
      if (current() && !isOperationCancelled(failure)) state.error = safeMessage(failure);
      return false;
    } finally {
      people.clear();
      owned.dispose();
      if (prepareScope === owned) prepareScope = null;
      state.preparing = false;
    }
  }
  async function save(truncateRedo = false): Promise<boolean> {
    if (state.approval === null || !matches(state.approval.context) || unresolved.value)
      return false;
    const generation = draftGeneration,
      scenarioId = project().scenarioId;
    const receipt = await review.apply(truncateRedo);
    if (
      receipt === null ||
      !alive ||
      project().scenarioId !== scenarioId ||
      generation !== draftGeneration
    )
      return false;
    discard();
    const settled = context();
    await loadWindow();
    return matches(settled);
  }
  async function refresh(): Promise<void> {
    if (busy.value) return;
    if ((await home.refreshLibrary()) && !dirty.value) await loadWindow();
  }
  watch(
    [() => project().scenarioId, () => project().revision, () => home.state.libraryEpoch],
    (current, previous) => {
      invalidateApproval();
      readGeneration += 1;
      readScope?.dispose();
      readScope = null;
      state.loading = false;
      if (previous[0] !== undefined && previous[0] !== current[0]) {
        discard();
        state.window = null;
        state.selectedPersonId = null;
      }
      // Preserve desired values across drift. A new native review is an explicit action.
      if (
        !dirty.value &&
        project().domainPackId === "official.workforce" &&
        home.state.busyAction === null
      )
        void loadWindow();
    },
    { immediate: true, flush: "sync" },
  );
  onScopeDispose(() => {
    alive = false;
    readGeneration += 1;
    invalidateApproval();
    readScope?.dispose();
  });
  return {
    state,
    review,
    dirty,
    busy,
    unresolved,
    stale,
    pendingCells,
    loadWindow,
    setMembership,
    removeChoice,
    preview,
    save,
    discard,
    refresh,
  };
}
