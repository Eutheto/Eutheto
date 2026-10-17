<script lang="ts">
import type { ProjectHomeController } from "../project-home";
type RepairDraft = {
  kind: "add" | "remove" | "replace" | "protection";
  personId: string;
  targetPersonId: string;
  shiftId: string;
  protection: "preserve" | "unlocked" | "hard" | "soft";
  weight: string;
  unlockIncumbent: boolean;
};
// Volatile raw presentation intent only. Returning from an alternative editor never restores approval.
const retained = new WeakMap<ProjectHomeController, Map<string, RepairDraft>>();
const admissionFailures = new WeakMap<ProjectHomeController, Map<string, string>>();
const unresolvedCommands = new WeakMap<
  ProjectHomeController,
  Map<string, { commandId: string; revision: number }>
>();
function emptyDraft(): RepairDraft {
  return {
    kind: "add",
    personId: "",
    targetPersonId: "",
    shiftId: "",
    protection: "preserve",
    weight: "1",
    unlockIncumbent: false,
  };
}
</script>

<script setup lang="ts">
import { computed, nextTick, onScopeDispose, reactive, ref, shallowRef, watch } from "vue";
import { RouterLink, useRouter } from "vue-router";
import {
  getScenarioView,
  getSolutionView,
  manualEdit,
  newUuidV7,
  SetupOperationScope,
  type ManualEditIntentV1,
  type ManualEditReadV1,
  type ManualEditReviewV1,
  type ManualEditSourceV1,
  type ManualEditResultV1,
  type ManualProtectionV1,
  type ManualProtectionChangeV1,
  type SetupOperation,
  type SolutionSummaryDtoV1,
} from "../api/generated";
import {
  WORKFORCE_ADD_PREFERENCE_COMMAND_ID,
  WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
  isWorkforceResultScheduleViewData,
  type WorkforceDateRange,
  type WorkforceSetupWorkPage,
  type WorkforceSetupTimedShiftContinuation,
  type WorkforceResultScheduleContinuation,
  type WorkforceResultScheduleAssignmentRow,
  type WorkforceTimePreference,
  type WorkforcePreferencePriority,
} from "../api/generated-domain-pack-contracts";
import {
  isOperationCancelled,
  isRedoBranchTruncation,
  safeMessage,
  type ProjectSummary,
} from "../project-home";
import {
  diagnosticAddress,
  resolveValidationAddress,
  validationRoute,
} from "../validation-navigation";
import { useSetupCommandReview } from "../setup-command-review";
import { sameField } from "../entity-draft";
import { getScenarioHistoryPage, type HistoryPageContinuationV1 } from "../api/generated";
import { messages } from "../messages";
import WorkforceEntityPicker from "./planner/WorkforceEntityPicker.vue";
import AcceptedSchedule from "./AcceptedSchedule.vue";
import AcceptedCounterfactual from "./AcceptedCounterfactual.vue";
import RouteLeaveGuard from "./RouteLeaveGuard.vue";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  dates: WorkforceDateRange;
  candidate?: SolutionSummaryDtoV1 | null;
  locale?: string;
}>();
const emit = defineEmits<{
  source: [source: ManualEditSourceV1 | null];
  committed: [input: { commandId: string; revision: number; alarm: string | null }];
  authorityFailure: [reason: string | null];
  reoptimize: [];
}>();
const router = useRouter();
const draft = reactive<RepairDraft>({
  ...(retained.get(props.home)?.get(props.project.scenarioId) ?? emptyDraft()),
});
let draftHome = props.home;
let draftScenarioId = props.project.scenarioId;
const alternativeNavigation = ref(false);
const page = shallowRef<ManualEditReadV1 | null>(null);
const reviewed = shallowRef<ManualEditReviewV1 | null>(null);
const reviewEpoch = ref<number | null>(null);
const offsets = ref([0]);
const pageIndex = ref(0);
const pending = ref(false);
const applying = ref(false);
const cancelRequested = ref(false);
const stale = ref(false);
const error = ref<string | null>(null);
const announcement = ref<string | null>(null);
const alarm = ref<string | null>(
  admissionFailures.get(props.home)?.get(props.project.scenarioId) ?? null,
);
const committedReceipt = shallowRef<{ commandId: string; revision: number } | null>(null);
const redo = ref(false);
const reviewHeading = ref<HTMLElement | null>(null);
const cancelTrigger = ref<HTMLButtonElement | null>(null);
const preferenceHeading = ref<HTMLElement | null>(null);
const formHeading = ref<HTMLElement | null>(null);
const readTrigger = ref<HTMLButtonElement | null>(null);
const reviewTrigger = ref<HTMLButtonElement | null>(null);
const work = shallowRef<WorkforceSetupWorkPage | null>(null);
const workDates = reactive({ ...props.dates });
const workLoading = ref(false);
const workError = ref<string | null>(null);
const workCursors = shallowRef<readonly (WorkforceSetupTimedShiftContinuation | null)[]>([null]);
const workIndex = ref(0);
const originalOpen = ref(false);
const diagnosticOpen = ref(false);
const diagnosticAssignment = shallowRef<WorkforceResultScheduleAssignmentRow | null>(null);
const diagnosticSummary = shallowRef<SolutionSummaryDtoV1 | null>(null);
const diagnosticStatus = ref<string | null>(null);
const diagnosticLoading = ref(false);
const preferenceOpen = ref(false);
const preferenceDirection = ref<"prefer" | "avoid">("prefer");
const preferencePriority = ref<WorkforcePreferencePriority>("normal");
const preferenceWeight = ref("1");
const preferenceInterval = shallowRef<{ startsAt: string; endsAt: string } | null>(null);
const preference = useSetupCommandReview(props.home, () => props.project);
let alive = true;
let epoch = 0;
let draftEpoch = 0;
let workEpoch = 0;
let alternativeEpoch = 0;
let operation: SetupOperation<ManualEditResultV1> | null = null;
let cancellationFocusOwner: SetupOperation<ManualEditResultV1> | null = null;
let owned: SetupOperationScope | null = null;
const alternativeScopes = new Set<SetupOperationScope>();
const unresolvedCommand = shallowRef(
  unresolvedCommands.get(props.home)?.get(props.project.scenarioId) ?? null,
);
const historyContinuation = shallowRef<HistoryPageContinuationV1 | null>(null);
const reconciling = ref(false);
const dirty = computed(() => JSON.stringify(draft) !== JSON.stringify(emptyDraft()));
const unknown = computed(
  () => unresolvedCommand.value !== null || props.home.state.mutation?.outcome === "outcomeUnknown",
);
const locked = computed(
  () =>
    pending.value ||
    props.project.archived ||
    props.home.state.busyAction !== null ||
    unknown.value,
);
const sourceReady = computed(
  () =>
    page.value !== null &&
    !stale.value &&
    alarm.value === null &&
    page.value.source.revision === props.project.revision,
);
const canApply = computed(
  () =>
    sourceReady.value &&
    !locked.value &&
    reviewed.value?.disposition === "valid" &&
    reviewed.value.approvalDigest !== null &&
    reviewEpoch.value === draftEpoch,
);
const originalSummary = computed(() =>
  page.value === null
    ? null
    : {
        result: page.value.source.original.acceptedResult,
        scenarioRevision: page.value.source.original.sourceRevision,
      },
);
const pickerContext = computed(() => ({
  project: props.project,
  libraryEpoch: props.home.state.libraryEpoch,
  disabled: locked.value,
  ...(props.locale === undefined ? {} : { locale: props.locale }),
}));
function saveDraft(): void {
  let drafts = retained.get(draftHome);
  if (!drafts) {
    drafts = new Map();
    retained.set(draftHome, drafts);
  }
  drafts.set(draftScenarioId, { ...draft });
}
function invalidateApproval(): void {
  draftEpoch += 1;
  reviewed.value = null;
  reviewEpoch.value = null;
  redo.value = false;
  alternativeEpoch += 1;
  diagnosticOpen.value = false;
  diagnosticAssignment.value = null;
  diagnosticSummary.value = null;
  diagnosticLoading.value = false;
  diagnosticStatus.value = null;
  preferenceInterval.value = null;
  preference.invalidate();
}
watch(
  draft,
  () => {
    saveDraft();
    invalidateApproval();
  },
  { deep: true, flush: "sync" },
);
watch(
  [preferenceDirection, preferencePriority, preferenceWeight],
  () => {
    preference.invalidate();
  },
  {
    flush: "sync",
  },
);
function capture() {
  const home = props.home,
    scenarioId = props.project.scenarioId,
    revision = props.project.revision,
    libraryEpoch = home.state.libraryEpoch,
    token = epoch;
  return {
    home,
    scenarioId,
    revision,
    libraryEpoch,
    current: () =>
      alive &&
      props.home === home &&
      props.project.scenarioId === scenarioId &&
      props.project.revision === revision &&
      home.state.libraryEpoch === libraryEpoch &&
      epoch === token,
  };
}
async function release(
  request: SetupOperation<ManualEditResultV1> | null,
  scope: SetupOperationScope | null,
): Promise<void> {
  if (request) await request.release().catch(() => undefined);
  scope?.dispose();
  if (operation === request) operation = null;
  if (cancellationFocusOwner === request) cancellationFocusOwner = null;
  if (owned === scope) owned = null;
}
function disposeRead(): void {
  const request = operation,
    scope = owned;
  operation = null;
  owned = null;
  void release(request, scope);
}
function isCancellationRequested(): boolean {
  return cancelRequested.value;
}
async function read(index = 0): Promise<void> {
  if (pending.value || props.home.state.busyAction !== null || unknown.value) return;
  const context = capture();
  const offset = index === 0 ? 0 : offsets.value[index];
  if (offset === undefined) return;
  pending.value = true;
  error.value = null;
  cancelRequested.value = false;
  let request: SetupOperation<ManualEditResultV1> | null = null;
  let scope: SetupOperationScope | null = null;
  try {
    scope = new SetupOperationScope(context.scenarioId, context.revision);
    owned = scope;
    request = manualEdit(scope, { mode: "read", offset, limit: 50 });
    operation = request;
    const response = await request.result;
    if (!context.current() || !request.isCurrent() || operation !== request) return;
    if (isCancellationRequested()) {
      announcement.value = messages.repair.cancelled;
      return;
    }
    const result = response.result;
    if (
      result.kind !== "read" ||
      response.currentRevision !== context.revision ||
      result.read.source.scenarioId !== context.scenarioId ||
      result.read.source.revision !== context.revision ||
      result.read.offset !== offset
    )
      throw new Error("Native selection source changed.");
    if (page.value && !sameField(page.value.source, result.read.source)) invalidateApproval();
    page.value = result.read;
    stale.value = false;
    alarm.value = null;
    admissionFailures.get(context.home)?.delete(context.scenarioId);
    pageIndex.value = index;
    emit("authorityFailure", null);
    if (index === 0) offsets.value = [0];
    if (result.read.nextOffset !== null) offsets.value[index + 1] = result.read.nextOffset;
    emit("source", result.read.source);
    announcement.value = `Current selection read: ${String(result.read.source.selectedCount)} selected pairs; ${String(result.read.totalRows)} native rows. ${result.read.source.currentManualId ? "Current admitted manual B." : "Original accepted A; no manual B yet."}`;
  } catch (failure) {
    if (context.current()) {
      const missingBase =
        typeof failure === "object" &&
        failure !== null &&
        "category" in failure &&
        failure.category === "validation" &&
        "code" in failure &&
        failure.code === "manual.base_missing";
      error.value = isOperationCancelled(failure) || missingBase ? null : safeMessage(failure);
      announcement.value = isOperationCancelled(failure)
        ? messages.repair.cancelled
        : missingBase
          ? messages.repair.noSource
          : null;
      page.value = null;
      emit("source", null);
    }
  } finally {
    const restoreFocus =
      document.activeElement === cancelTrigger.value ||
      (request !== null && cancellationFocusOwner === request);
    await release(request, scope);
    if (context.current()) {
      pending.value = false;
      cancelRequested.value = false;
    }
    await nextTick();
    if (
      context.current() &&
      restoreFocus &&
      (document.activeElement === document.body || document.activeElement === cancelTrigger.value)
    )
      readTrigger.value?.focus();
  }
}
function protection(): ManualProtectionV1 | null {
  if (draft.protection === "preserve") return null;
  if (draft.protection === "soft") {
    if (!/^[1-9][0-9]*$/.test(draft.weight) || Number(draft.weight) > 1_000_000)
      throw new Error("Enter a Soft stability weight from 1 to 1,000,000.");
    return { kind: "soft", stabilityWeight: Number(draft.weight) };
  }
  return { kind: draft.protection };
}
function intent(): ManualEditIntentV1 {
  if (!draft.personId || !draft.shiftId || (draft.kind === "replace" && !draft.targetPersonId))
    throw new Error("Choose the people and exact native work shift for this change.");
  const selectedProtection = draft.kind === "remove" ? null : protection();
  if (draft.kind === "protection") {
    if (selectedProtection === null) throw new Error("Choose an explicit protection transition.");
    return {
      kind: "protection",
      personId: draft.personId,
      shiftId: draft.shiftId,
      protection: selectedProtection,
    };
  }
  const changes: ManualProtectionChangeV1[] = [];
  if (draft.unlockIncumbent && (draft.kind === "replace" || draft.kind === "remove"))
    changes.push({
      personId: draft.personId,
      shiftId: draft.shiftId,
      protection: { kind: "unlocked" },
    });
  if (selectedProtection)
    changes.push({
      personId: draft.kind === "replace" ? draft.targetPersonId : draft.personId,
      shiftId: draft.shiftId,
      protection: selectedProtection,
    });
  if (draft.kind === "replace")
    return {
      kind: "replace",
      incumbentPersonId: draft.personId,
      targetPersonId: draft.targetPersonId,
      shiftId: draft.shiftId,
      protectionChanges: changes,
    };
  return {
    kind: draft.kind,
    personId: draft.personId,
    shiftId: draft.shiftId,
    protectionChanges: changes,
  };
}
async function review(): Promise<void> {
  if (locked.value || !sourceReady.value) return;
  let proposal: ManualEditIntentV1;
  try {
    proposal = intent();
  } catch (failure) {
    error.value = failure instanceof Error ? failure.message : safeMessage(failure);
    return;
  }
  invalidateApproval();
  const atDraft = draftEpoch,
    context = capture();
  pending.value = true;
  error.value = null;
  announcement.value = "Reviewing the complete native selection…";
  cancelRequested.value = false;
  let request: SetupOperation<ManualEditResultV1> | null = null;
  let scope: SetupOperationScope | null = null;
  try {
    scope = new SetupOperationScope(context.scenarioId, context.revision);
    owned = scope;
    request = manualEdit(scope, { mode: "review", intent: proposal });
    operation = request;
    const response = await request.result;
    if (
      !context.current() ||
      draftEpoch !== atDraft ||
      operation !== request ||
      !request.isCurrent()
    )
      return;
    if (isCancellationRequested()) {
      announcement.value = messages.repair.cancelled;
      return;
    }
    const result = response.result;
    if (
      result.kind !== "review" ||
      response.currentRevision !== context.revision ||
      !sameField(result.review.source, page.value?.source) ||
      !sameField(result.review.intent, proposal)
    )
      throw new Error("Native review no longer matches the displayed source and intent.");
    reviewed.value = result.review;
    reviewEpoch.value = atDraft;
    announcement.value =
      result.review.disposition === "valid"
        ? "Native review is valid. Nothing has been applied."
        : "Native review is invalid. Nothing has been applied; your draft is retained.";
    await nextTick();
    if (context.current() && draftEpoch === atDraft) reviewHeading.value?.focus();
  } catch (failure) {
    if (context.current() && draftEpoch === atDraft) {
      error.value = isOperationCancelled(failure) ? null : safeMessage(failure);
      announcement.value = isOperationCancelled(failure) ? messages.repair.cancelled : "";
    }
  } finally {
    const restoreFocus =
      document.activeElement === cancelTrigger.value ||
      (request !== null && cancellationFocusOwner === request);
    await release(request, scope);
    if (context.current()) {
      pending.value = false;
      cancelRequested.value = false;
    }
    await nextTick();
    if (
      context.current() &&
      restoreFocus &&
      (document.activeElement === document.body || document.activeElement === cancelTrigger.value)
    )
      reviewTrigger.value?.focus();
  }
}
async function apply(): Promise<void> {
  const review = reviewed.value;
  if (!canApply.value || !review?.approvalDigest) return;
  const approvalDigest = review.approvalDigest;
  const context = capture(),
    commandId = newUuidV7(),
    truncateRedo = redo.value;
  pending.value = true;
  applying.value = true;
  error.value = null;
  cancelRequested.value = false;
  let request: SetupOperation<ManualEditResultV1> | null = null;
  const scope = new SetupOperationScope(context.scenarioId, context.revision);
  owned = scope;
  try {
    await context.home.runOperation({
      action: "manual-repair-apply",
      label: "Applying reviewed manual change…",
      mutation: {
        scenarioId: context.scenarioId,
        expectedRevision: context.revision,
        commandId,
        receipt: (result) => {
          if (result.kind !== "applied")
            throw new Error("Native manual apply did not return a committed receipt.");
          return {
            kind:
              result.applied.commandResult.newRevision === context.revision
                ? "noChanges"
                : "applied",
            revision: result.applied.commandResult.newRevision,
          };
        },
      },
      cancel: async () => {
        cancelRequested.value = true;
        await request?.cancel();
      },
      execute: async () => {
        request = manualEdit(scope, {
          mode: "apply",
          intent: review.intent,
          approvalDigest,
          commandId,
          actor: { actorId: null, displayName: messages.people.localActor },
          truncateRedo,
        });
        operation = request;
        const response = await request.result;
        if (response.result.kind === "applied") {
          // Committed receipt wins over our own revision event and cancellation.
          const committed = response.result.applied;
          const failure = committed.sourceAdmissionFailure;
          const admissionAlarm =
            failure === null ? null : `${failure.category} / ${failure.code}: ${failure.message}`;
          let failures = admissionFailures.get(context.home);
          if (!failures) {
            failures = new Map();
            admissionFailures.set(context.home, failures);
          }
          if (admissionAlarm) failures.set(context.scenarioId, admissionAlarm);
          else failures.delete(context.scenarioId);
          if (
            alive &&
            props.home === context.home &&
            props.project.scenarioId === context.scenarioId &&
            context.home.state.libraryEpoch === context.libraryEpoch
          ) {
            emit("committed", {
              commandId,
              revision: committed.commandResult.newRevision,
              alarm: admissionAlarm,
            });
            alarm.value = admissionAlarm;
            committedReceipt.value = { commandId, revision: committed.commandResult.newRevision };
            announcement.value = admissionAlarm ? messages.repair.alarm : messages.repair.committed;
            reviewed.value = null;
            reviewEpoch.value = null;
            Object.assign(draft, emptyDraft());
            retained.get(context.home)?.delete(context.scenarioId);
          }
        }
        return response;
      },
      success: (result) =>
        result.kind === "applied" && result.applied.sourceAdmissionFailure !== null
          ? messages.repair.alarm
          : messages.repair.committed,
    });
  } catch (failure) {
    if (
      context.home.state.mutation?.commandId === commandId &&
      context.home.state.mutation.outcome === "outcomeUnknown"
    ) {
      let commands = unresolvedCommands.get(context.home);
      if (!commands) {
        commands = new Map();
        unresolvedCommands.set(context.home, commands);
      }
      const identity = { commandId, revision: context.revision };
      commands.set(context.scenarioId, identity);
      if (alive && props.home === context.home && props.project.scenarioId === context.scenarioId)
        unresolvedCommand.value = identity;
    }
    if (context.current()) {
      error.value = safeMessage(failure);
      if (isRedoBranchTruncation(failure)) {
        reviewed.value = review;
        reviewEpoch.value = draftEpoch;
        redo.value = true;
      } else {
        reviewed.value = null;
        reviewEpoch.value = null;
      }
    }
  } finally {
    const restoreFocus =
      document.activeElement === cancelTrigger.value ||
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- The execute callback assigns request while runOperation is awaited.
      (request !== null && cancellationFocusOwner === request);
    await release(request, scope);
    if (alive) {
      pending.value = false;
      applying.value = false;
      cancelRequested.value = false;
    }
    await nextTick();
    if (
      alive &&
      props.home === context.home &&
      props.project.scenarioId === context.scenarioId &&
      context.home.state.libraryEpoch === context.libraryEpoch &&
      (document.activeElement === document.body ||
        document.activeElement === cancelTrigger.value) &&
      restoreFocus
    )
      formHeading.value?.focus();
  }
}
async function reconcileExactCommand(): Promise<void> {
  if (reconciling.value || props.home.state.busyAction !== null) return;
  const context = capture(),
    identity = unresolvedCommand.value;
  if (identity === null) {
    await context.home.reconcileMutation();
    return;
  }
  reconciling.value = true;
  try {
    if (context.home.state.mutation?.commandId === identity.commandId)
      await context.home.reconcileMutation();
    if (!context.current()) return;
    const mutation = context.home.state.mutation;
    if (mutation?.commandId === identity.commandId && mutation.history.kind === "found") {
      unresolvedCommands.get(context.home)?.delete(context.scenarioId);
      unresolvedCommand.value = null;
      announcement.value = `Exact command found in native History; recorded revision ${String(mutation.history.revision)}. Refresh the source and review again before another Apply.`;
      invalidateApproval();
      return;
    }
    // History remains the only reconciliation authority, even if a root observer was dismissed.
    const { result } = await getScenarioHistoryPage({
      schemaVersion: 1,
      scenarioId: context.scenarioId,
      expectedRevision: context.revision,
      limit: 50,
      continuation:
        historyContinuation.value?.revision === context.revision ? historyContinuation.value : null,
    });
    if (
      !context.current() ||
      result.scenarioId !== context.scenarioId ||
      result.revision !== context.revision
    )
      return;
    const entry = result.entries.find(
      (entry) => entry.id === identity.commandId && entry.revisionBefore === identity.revision,
    );
    if (entry) {
      unresolvedCommands.get(context.home)?.delete(context.scenarioId);
      unresolvedCommand.value = null;
      invalidateApproval();
      announcement.value = `Exact command found in native History, revision ${String(entry.revisionAfter)}; ${entry.applied ? "still applied" : "undone"}. Refresh and review again.`;
    } else {
      historyContinuation.value = result.continuation;
      announcement.value = result.continuation
        ? "Exact command not found in this native History page; check the next page. Apply remains blocked."
        : "Exact command was not found. Its outcome is still unresolved; Apply remains blocked. No mutation will be retried.";
    }
  } catch (failure) {
    if (context.current()) error.value = safeMessage(failure);
  } finally {
    if (alive) reconciling.value = false;
  }
}
async function cancelOperation(): Promise<void> {
  const request = operation;
  if (!request || cancelRequested.value) return;
  if (document.activeElement === cancelTrigger.value) cancellationFocusOwner = request;
  cancelRequested.value = true;
  announcement.value =
    "Cancellation requested; awaiting the same operation's actual outcome. A late cancellation cannot undo a commit.";
  try {
    await request.cancel();
  } catch (failure) {
    if (operation === request && request.isCurrent()) {
      cancelRequested.value = false;
      error.value = safeMessage(failure);
    }
  }
}
function cancelDraft(): void {
  if (pending.value) {
    void cancelOperation();
    return;
  }
  Object.assign(draft, emptyDraft());
  invalidateApproval();
  preferenceOpen.value = false;
  error.value = null;
  announcement.value = "Manual draft cancelled; no new command was sent.";
  retained.get(props.home)?.delete(props.project.scenarioId);
  void nextTick(() => formHeading.value?.focus());
}
function handleEscape(): void {
  if (pending.value) {
    void cancelOperation();
    return;
  }
  if (preference.state.applying) {
    announcement.value =
      "The preference command is already dispatched. Await its actual receipt; closing a draft cannot undo a commit.";
    return;
  }
  if (preferenceOpen.value) {
    if (preference.state.pending) void props.home.cancelOperation();
    preferenceOpen.value = false;
    preference.invalidate();
    void nextTick(() => reviewHeading.value?.focus());
    return;
  }
  if (diagnosticOpen.value) {
    diagnosticOpen.value = false;
    announcement.value =
      "Current accepted-source diagnostic observer closed. This does not cancel a saved diagnostic job or apply any manual change.";
    void nextTick(() => reviewHeading.value?.focus());
    return;
  }
  cancelDraft();
}
function chooseRow(personId: string, shiftId: string, kind: RepairDraft["kind"]): void {
  if (locked.value) return;
  Object.assign(draft, { personId, shiftId, kind, protection: "preserve", unlockIncumbent: false });
  void nextTick(() => formHeading.value?.focus());
}
async function loadWork(index = 0): Promise<void> {
  const context = capture(),
    token = ++workEpoch;
  workLoading.value = true;
  workError.value = null;
  const scope = new SetupOperationScope(context.scenarioId, context.revision);
  alternativeScopes.add(scope);
  try {
    const response = await getScenarioView(scope, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId: "official.workforce.setup.work_window",
        parameters: { dates: { ...workDates }, limit: 50 },
        continuation: workCursors.value[index] ?? null,
      },
    }).result;
    if (!context.current() || token !== workEpoch) return;
    if (
      response.currentRevision !== context.revision ||
      response.result.revision !== context.revision
    )
      throw new Error("Work source changed.");
    work.value = response.result.view.data.result.data;
    workIndex.value = index;
    if (index === 0) workCursors.value = [null];
    if (work.value.continuation)
      workCursors.value = [...workCursors.value.slice(0, index + 1), work.value.continuation];
  } catch (failure) {
    if (context.current() && token === workEpoch) workError.value = safeMessage(failure);
  } finally {
    alternativeScopes.delete(scope);
    scope.dispose();
    if (context.current() && token === workEpoch) workLoading.value = false;
  }
}
watch(
  workDates,
  () => {
    workEpoch += 1;
    work.value = null;
    workCursors.value = [null];
    workIndex.value = 0;
    workLoading.value = false;
  },
  { flush: "sync" },
);
async function editOwner(path: string): Promise<void> {
  if (
    locked.value ||
    stale.value ||
    !reviewed.value?.evaluations.some((item) => item.ownerPaths.includes(path))
  )
    return;
  const address = diagnosticAddress(path);
  if (!address) {
    error.value = "This native owner path has no supported editor.";
    return;
  }
  const context = capture(),
    atDraft = draftEpoch;
  const scope = new SetupOperationScope(context.scenarioId, context.revision);
  alternativeScopes.add(scope);
  try {
    const resolved = await resolveValidationAddress(scope, address);
    if (
      !context.current() ||
      atDraft !== draftEpoch ||
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- Reactive review ownership can be cleared during the native await.
      !reviewed.value?.evaluations.some((item) => item.ownerPaths.includes(path))
    )
      return;
    if (!resolved.route) {
      error.value = "This native owner has no supported editor.";
      return;
    }
    saveDraft();
    invalidateApproval();
    alternativeNavigation.value = true;
    await router.push(
      validationRoute(
        {
          ...address,
          scenarioId: context.scenarioId,
          revision: context.revision,
          libraryEpoch: context.libraryEpoch,
        },
        resolved.route,
      ),
    );
  } catch (failure) {
    if (context.current()) error.value = safeMessage(failure);
  } finally {
    alternativeNavigation.value = false;
    alternativeScopes.delete(scope);
    scope.dispose();
  }
}
async function openPreference(): Promise<void> {
  if (
    locked.value ||
    !sourceReady.value ||
    !reviewed.value ||
    reviewed.value.disposition !== "invalid"
  )
    return;
  const context = capture(),
    token = ++alternativeEpoch;
  const personId = draft.kind === "replace" ? draft.targetPersonId : draft.personId,
    shiftId = draft.shiftId;
  if (!personId || !shiftId) return;
  const scope = new SetupOperationScope(context.scenarioId, context.revision);
  alternativeScopes.add(scope);
  try {
    const response = await getScenarioView(scope, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId: "official.workforce.setup.work_detail",
        parameters: { shiftId },
      },
    }).result;
    if (!context.current() || token !== alternativeEpoch) return;
    const shift = response.result.view.data.result.data.shift;
    if (shift.shiftId !== shiftId || response.currentRevision !== context.revision)
      throw new Error("The native preference interval changed.");
    preferenceInterval.value = {
      startsAt: shift.interval.startsAt.instant,
      endsAt: shift.interval.endsAt.instant,
    };
    preferenceOpen.value = true;
    await nextTick();
    if (context.current() && token === alternativeEpoch) preferenceHeading.value?.focus();
  } catch (failure) {
    if (context.current() && token === alternativeEpoch) error.value = safeMessage(failure);
  } finally {
    alternativeScopes.delete(scope);
    scope.dispose();
  }
}
async function previewPreference(): Promise<void> {
  if (!preferenceInterval.value || locked.value) return;
  if (!/^[1-9][0-9]*$/.test(preferenceWeight.value) || Number(preferenceWeight.value) > 1_000_000) {
    error.value = "Enter a preference weight from 1 to 1,000,000.";
    return;
  }
  const value: WorkforceTimePreference = {
    id: newUuidV7(),
    kind: "time",
    active: true,
    direction: preferenceDirection.value,
    priority: preferencePriority.value,
    weight: Number(preferenceWeight.value),
    scope: {
      people: {
        kind: "selected",
        personIds: [draft.kind === "replace" ? draft.targetPersonId : draft.personId],
      },
    },
    timeWindow: { kind: "instant", ...preferenceInterval.value },
  };
  await preference.preview(
    {
      type: "applyDomainCommand",
      payload: { commandType: WORKFORCE_ADD_PREFERENCE_COMMAND_ID, payload: { preference: value } },
    },
    null,
  );
}
async function applyPreference(): Promise<void> {
  const result = await preference.apply(preference.state.redoRequired);
  if (result !== null && alive) {
    preferenceOpen.value = false;
    stale.value = true;
    reviewed.value = null;
    emit("reoptimize");
    announcement.value =
      "Time preference committed. The rejected manual draft remains retained; refresh and review it again if needed. Re-optimize uses the new authored preference.";
  }
}
async function openDiagnostic(): Promise<void> {
  diagnosticStatus.value = messages.repair.diagnosticUnavailable;
  const candidate = props.candidate;
  if (
    !sourceReady.value ||
    locked.value ||
    draft.kind !== "replace" ||
    !page.value ||
    !candidate ||
    candidate.scenarioId !== props.project.scenarioId ||
    candidate.scenarioRevision !== props.project.revision
  )
    return;
  const context = capture(),
    token = ++alternativeEpoch;
  const incumbent = draft.personId,
    target = draft.targetPersonId,
    shiftId = draft.shiftId;
  diagnosticLoading.value = true;
  try {
    const [before, after] = await Promise.all([
      getSolutionView({
        scenarioId: context.scenarioId,
        solutionId: candidate.result.solutionId,
        viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
        parameters: { kind: "decision", personId: incumbent, shiftId },
      }),
      getSolutionView({
        scenarioId: context.scenarioId,
        solutionId: candidate.result.solutionId,
        viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
        parameters: { kind: "decision", personId: target, shiftId },
      }),
    ]);
    if (!context.current() || token !== alternativeEpoch) return;
    for (const response of [before, after])
      if (
        response.result.scenarioId !== context.scenarioId ||
        response.result.currentRevision !== context.revision ||
        response.result.scenarioRevision !== context.revision ||
        response.result.result.solutionId !== candidate.result.solutionId ||
        response.result.result.resultChecksum !== candidate.result.resultChecksum ||
        response.result.view.viewId !== WORKFORCE_RESULT_SCHEDULE_VIEW_ID
      )
        throw new Error("Chosen current accepted diagnostic source mismatch.");
    const beforeData = before.result.view.data,
      afterData = after.result.view.data;
    if (
      !isWorkforceResultScheduleViewData(beforeData) ||
      !isWorkforceResultScheduleViewData(afterData)
    )
      throw new Error("Chosen accepted diagnostic view data is unavailable.");
    const a = beforeData.result,
      b = afterData.result;
    if (
      a.kind !== "decision" ||
      b.kind !== "decision" ||
      a.personId !== incumbent ||
      b.personId !== target ||
      a.shiftId !== shiftId ||
      b.shiftId !== shiftId ||
      a.selected !== true ||
      b.selected !== false ||
      !a.assignmentId ||
      !b.assignmentId
    )
      return;
    let continuation: WorkforceResultScheduleContinuation | null = null;
    // ponytail: cap automatic lookup at 500 rows; explicit schedule paging handles larger shifts.
    for (let pageIndex = 0; pageIndex < 10; pageIndex += 1) {
      const response = await getSolutionView({
        scenarioId: context.scenarioId,
        solutionId: candidate.result.solutionId,
        viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
        parameters: { kind: "assignments", shiftId, limit: 50 },
        continuation,
      });
      if (!context.current() || token !== alternativeEpoch) return;
      const result = response.result;
      if (
        result.scenarioId !== context.scenarioId ||
        result.currentRevision !== context.revision ||
        result.scenarioRevision !== context.revision ||
        result.result.solutionId !== candidate.result.solutionId ||
        result.result.resultChecksum !== candidate.result.resultChecksum ||
        result.view.viewId !== WORKFORCE_RESULT_SCHEDULE_VIEW_ID ||
        !isWorkforceResultScheduleViewData(result.view.data) ||
        result.view.data.result.kind !== "assignments"
      )
        throw new Error("Chosen accepted diagnostic assignment page mismatch.");
      const rows = result.view.data.result;
      const assignment = rows.items.find(
        (row) =>
          row.personId === incumbent &&
          row.shiftId === shiftId &&
          row.assignmentId === a.assignmentId,
      );
      if (assignment) {
        diagnosticSummary.value = candidate;
        diagnosticAssignment.value = assignment;
        diagnosticOpen.value = true;
        diagnosticStatus.value = null;
        return;
      }
      if (rows.continuation === null) return;
      continuation = rows.continuation;
    }
    diagnosticStatus.value = messages.repair.diagnosticPageLimit;
  } catch (failure) {
    if (context.current() && token === alternativeEpoch)
      diagnosticStatus.value = safeMessage(failure);
  } finally {
    if (context.current() && token === alternativeEpoch) diagnosticLoading.value = false;
  }
}
watch(
  () => [
    props.candidate?.scenarioId,
    props.candidate?.scenarioRevision,
    props.candidate?.result.solutionId,
    props.candidate?.result.resultChecksum,
    props.project.revision,
    props.home.state.libraryEpoch,
  ],
  () => {
    alternativeEpoch += 1;
    diagnosticOpen.value = false;
    diagnosticAssignment.value = null;
    diagnosticSummary.value = null;
    diagnosticStatus.value = null;
    diagnosticLoading.value = false;
  },
  { immediate: true, flush: "sync" },
);
watch(
  () => [
    props.home,
    props.project.scenarioId,
    props.project.revision,
    props.home.state.libraryEpoch,
  ],
  (_, previous) => {
    epoch += 1;
    invalidateApproval();
    disposeRead();
    if (draftHome !== props.home || draftScenarioId !== props.project.scenarioId) {
      saveDraft();
      draftHome = props.home;
      draftScenarioId = props.project.scenarioId;
      Object.assign(draft, retained.get(draftHome)?.get(draftScenarioId) ?? emptyDraft());
      alarm.value = admissionFailures.get(draftHome)?.get(draftScenarioId) ?? null;
      committedReceipt.value = null;
      unresolvedCommand.value = unresolvedCommands.get(draftHome)?.get(draftScenarioId) ?? null;
      historyContinuation.value = null;
    }
    for (const scope of alternativeScopes) scope.dispose();
    alternativeScopes.clear();
    page.value = null;
    pending.value = applying.value;
    stale.value = previous !== undefined;
    workEpoch += 1;
    work.value = null;
    workLoading.value = false;
    workCursors.value = [null];
    emit("source", null);
    if (alarm.value !== null) emit("authorityFailure", alarm.value);
    if (!applying.value) void read();
  },
  { immediate: true, flush: "sync" },
);
watch(
  () => props.home.state.busyAction,
  (busy) => {
    if (busy === null && !page.value && !pending.value && !unknown.value) void read();
  },
);
onScopeDispose(() => {
  saveDraft();
  alive = false;
  epoch += 1;
  workEpoch += 1;
  alternativeEpoch += 1;
  disposeRead();
  for (const scope of alternativeScopes) scope.dispose();
  alternativeScopes.clear();
});
</script>

<template>
  <section
    data-manual-repair-panel
    class="state-panel field-stack grid-cols-1"
    aria-labelledby="manual-repair-heading"
    @keydown.esc.stop.prevent="handleEscape"
  >
    <h3 id="manual-repair-heading" ref="formHeading" tabindex="-1">
      {{ messages.repair.heading }}
    </h3>
    <p>{{ messages.repair.description }}</p>
    <p v-if="announcement" role="status" aria-live="polite">{{ announcement }}</p>
    <p v-if="stale" role="status">{{ messages.repair.stale }}</p>
    <p v-if="committedReceipt" role="status">
      Committed command <code>{{ committedReceipt.commandId }}</code
      >; new revision {{ committedReceipt.revision }}. This receipt is not a failed mutation.
    </p>
    <p v-if="error" role="alert">{{ error }}</p>
    <p v-if="alarm" role="alert">{{ messages.repair.alarm }} {{ alarm }}</p>
    <div v-if="unknown" role="alert">
      <p>
        {{ messages.repair.unknown }} Exact command:
        <code>{{ unresolvedCommand?.commandId ?? home.state.mutation?.commandId }}</code>
      </p>
      <button
        type="button"
        :disabled="reconciling || home.state.busyAction !== null"
        @click="reconcileExactCommand"
      >
        Reconcile exact command in History
      </button>
    </div>
    <RouterLink :to="{ name: 'project-history', params: { scenarioId: project.scenarioId } }"
      >Open History — undo the single manual-change command</RouterLink
    >
    <button
      ref="readTrigger"
      type="button"
      :disabled="pending || home.state.busyAction !== null || unknown"
      @click="read()"
    >
      {{ messages.repair.read }}
    </button>
    <p v-if="pending" role="status" aria-busy="true">
      {{ applying ? "Waiting for the manual command outcome…" : messages.repair.loading }}
    </p>
    <button
      v-if="pending"
      ref="cancelTrigger"
      type="button"
      :disabled="cancelRequested"
      @click="cancelOperation"
    >
      {{
        cancelRequested
          ? "Cancellation requested — awaiting outcome"
          : messages.repair.cancelOperation
      }}
    </button>
    <template v-if="page">
      <h4>
        {{
          page.source.currentManualId
            ? "Current admitted manual B"
            : "Original accepted A — current selection"
        }}
      </h4>
      <p>
        Revision {{ page.source.revision }}; complete selected pairs
        {{ page.source.selectedCount }}; protection records {{ page.source.protectionCount }}.
      </p>
      <p>
        Original A: <code>{{ page.source.original.acceptedResult.solutionId }}</code> · source
        revision {{ page.source.original.sourceRevision }} ·
        <code>{{ page.source.original.acceptedResult.resultChecksum }}</code>
      </p>
      <p v-if="page.source.currentManualId">
        Manual B: <code>{{ page.source.currentManualId }}</code> ·
        <code>{{ page.source.currentManualChecksum }}</code
        >. Manual admission is not accepted-result verification.
      </p>
      <table class="data-table">
        <caption>
          Native current-selection and protection rows — page
          {{
            pageIndex + 1
          }}
          of
          {{
            page.totalRows
          }}
          rows
        </caption>
        <thead>
          <tr>
            <th scope="col">Person</th>
            <th scope="col">Work shift</th>
            <th scope="col">Selected</th>
            <th scope="col">Protection</th>
            <th scope="col">Edit</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="row in page.rows"
            :key="`${row.personId}:${row.shiftId}`"
            data-native-pair
            :data-person-id="row.personId"
            :data-shift-id="row.shiftId"
          >
            <td>
              {{ row.personLabel }} <code>{{ row.personId }}</code>
              <p v-if="!row.personAvailable">{{ messages.repair.missingEndpoint }}</p>
            </td>
            <td>
              {{ row.shiftLabel }} <code>{{ row.shiftId }}</code>
              <p v-if="!row.shiftAvailable">{{ messages.repair.missingEndpoint }}</p>
            </td>
            <td>{{ row.selected ? "Yes" : "No" }}</td>
            <td>
              {{ row.protection.kind
              }}<template v-if="row.protection.kind === 'soft'"
                >; weight {{ row.protection.stabilityWeight }}</template
              ><code v-if="row.lockId"> {{ row.lockId }}</code>
            </td>
            <td>
              <button
                v-if="row.selected"
                type="button"
                :disabled="locked"
                @click="chooseRow(row.personId, row.shiftId, 'remove')"
              >
                Remove {{ row.personLabel }}</button
              ><button
                v-if="row.selected"
                type="button"
                :disabled="locked || !row.shiftAvailable"
                @click="chooseRow(row.personId, row.shiftId, 'replace')"
              >
                Replace {{ row.personLabel }} on this shift</button
              ><button
                type="button"
                :disabled="locked"
                @click="chooseRow(row.personId, row.shiftId, 'protection')"
              >
                Change protection for {{ row.personLabel }}
              </button>
            </td>
          </tr>
        </tbody>
      </table>
      <p v-if="page.rows.length === 0">
        No current native rows in this page. Add a person to a native work shift below.
      </p>
      <nav aria-label="Current selection pages">
        <button type="button" :disabled="pending || pageIndex === 0" @click="read(pageIndex - 1)">
          Previous selection page</button
        ><button
          type="button"
          :disabled="pending || page.nextOffset === null"
          @click="read(pageIndex + 1)"
        >
          Next selection page
        </button>
      </nav>
      <button type="button" :aria-expanded="originalOpen" @click="originalOpen = !originalOpen">
        {{ originalOpen ? "Hide immutable original A" : "Inspect immutable original A" }}
      </button>
      <AcceptedSchedule
        v-if="originalOpen && originalSummary"
        :scenario-id="project.scenarioId"
        :summary="originalSummary"
        :current-revision="project.revision"
        :home="home"
      />
    </template>
    <form class="field-stack" @submit.prevent="review">
      <fieldset :disabled="locked">
        <legend>Manual intent — not yet applied</legend>
        <label
          >Change
          <select v-model="draft.kind">
            <option value="add">Add person to shift</option>
            <option value="remove">Remove person from shift</option>
            <option value="replace">Replace person on the same shift</option>
            <option value="protection">Change protection only</option>
          </select></label
        >
        <WorkforceEntityPicker
          v-bind="pickerContext"
          kind="person"
          :label="draft.kind === 'replace' ? 'Incumbent person' : 'Person'"
          :model-value="draft.personId ? [draft.personId] : []"
          @update:model-value="draft.personId = $event[0] ?? ''"
        />
        <WorkforceEntityPicker
          v-if="draft.kind === 'replace'"
          v-bind="pickerContext"
          kind="person"
          label="Replacement person"
          :model-value="draft.targetPersonId ? [draft.targetPersonId] : []"
          @update:model-value="draft.targetPersonId = $event[0] ?? ''"
        />
        <p>
          Exact native work shift:
          <code>{{ draft.shiftId || "Choose below, or from a current row" }}</code
          >. Replacement removes the incumbent and adds the target on this same shift, including
          AtLeast coverage; it is not an additional assignment.
        </p>
        <label v-if="draft.kind !== 'remove'"
          >Protection transition
          <select v-model="draft.protection">
            <option value="preserve">Preserve existing protection</option>
            <option value="unlocked">Unlocked</option>
            <option value="hard">Hard — must remain selected</option>
            <option value="soft">Soft — weighted stability preference</option>
          </select></label
        >
        <label v-if="draft.kind !== 'remove' && draft.protection === 'soft'"
          >Soft stability weight <input v-model="draft.weight" type="text" inputmode="numeric"
        /></label>
        <label v-if="draft.kind === 'remove' || draft.kind === 'replace'"
          ><input v-model="draft.unlockIncumbent" type="checkbox" /> Explicitly unlock the incumbent
          in this same command</label
        >
        <p>
          Unspecified protection is preserved. Removing or replacing a Hard-protected incumbent
          requires explicit unlock; native review never silently relaxes it. Replacement protection
          above applies to the target.
        </p>
      </fieldset>
      <button ref="reviewTrigger" type="submit" :disabled="locked || !sourceReady">
        {{ messages.repair.review }}
      </button>
      <button type="button" :disabled="applying" @click="cancelDraft">
        {{ messages.repair.cancel }}
      </button>
    </form>
    <section aria-label="Choose native work shift" class="field-stack">
      <h4>Choose work, including recurring shifts</h4>
      <label
        >Work starts on
        <input v-model="workDates.startDate" type="date" :disabled="locked" /></label
      ><label
        >Work ends before
        <input v-model="workDates.endDateExclusive" type="date" :disabled="locked"
      /></label>
      <button type="button" :disabled="locked || workLoading" @click="loadWork()">
        Read native work window
      </button>
      <p v-if="workLoading" role="status">Loading native work shifts…</p>
      <p v-if="workError" role="alert">{{ workError }}</p>
      <template v-if="work"
        ><p v-if="work.items.length === 0">No work shifts in this native window.</p>
        <ul>
          <li v-for="shift in work.items" :key="shift.shiftId">
            <button type="button" :disabled="locked" @click="draft.shiftId = shift.shiftId">
              Choose {{ shift.assignmentTypeName }} · {{ shift.interval.startsAt.local }} –
              {{ shift.interval.endsAt.local }}
            </button>
            <code>{{ shift.shiftId }}</code> · {{ shift.templateName ?? "One-off work" }}
          </li>
        </ul>
        <nav aria-label="Native work pages">
          <button
            type="button"
            :disabled="workLoading || workIndex === 0"
            @click="loadWork(workIndex - 1)"
          >
            Previous work page</button
          ><button
            type="button"
            :disabled="workLoading || work.continuation === null"
            @click="loadWork(workIndex + 1)"
          >
            Next work page
          </button>
        </nav></template
      >
    </section>
    <section
      v-if="reviewed"
      data-manual-review
      :data-disposition="reviewed.disposition"
      aria-labelledby="manual-review-heading"
      class="state-panel field-stack grid-cols-1"
    >
      <h4 id="manual-review-heading" ref="reviewHeading" tabindex="-1">
        Native manual review — {{ reviewed.disposition }}
      </h4>
      <p>
        Complete source document: <code>{{ reviewed.source.documentHash }}</code
        >. Nothing changes until explicit Apply.
      </p>
      <table class="data-table">
        <caption>
          Native exact changed pairs and protection
        </caption>
        <thead>
          <tr>
            <th scope="col">Pair</th>
            <th scope="col">Before</th>
            <th scope="col">After</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="change in reviewed.changes"
            :key="`${change.before.personId}:${change.before.shiftId}`"
          >
            <td>
              {{ change.before.personLabel }} · {{ change.before.shiftLabel }}
              <code>{{ change.before.personId }} / {{ change.before.shiftId }}</code>
            </td>
            <td>
              {{ change.before.selected ? "Selected" : "Not selected" }} ·
              {{ change.before.protection }}
            </td>
            <td>
              {{ change.after.selected ? "Selected" : "Not selected" }} ·
              {{ change.after.protection }}
            </td>
          </tr>
        </tbody>
      </table>
      <section v-for="(finding, index) in reviewed.evaluations" :key="index">
        <h5>
          Required-rule evaluation — {{ finding.evaluation.satisfied ? "Satisfied" : "Failed" }}
        </h5>
        <p>
          Native reason: <code>{{ finding.evaluation.messageKey }}</code
          >; rule <code>{{ finding.evaluation.ruleId }}</code
          >.
        </p>
        <dl>
          <dt>Expected</dt>
          <dd>
            <pre class="whitespace-pre-wrap">{{
              JSON.stringify(finding.evaluation.expected, null, 2)
            }}</pre>
          </dd>
          <dt>Observed</dt>
          <dd>
            <pre class="whitespace-pre-wrap">{{
              JSON.stringify(finding.evaluation.observed, null, 2)
            }}</pre>
          </dd>
        </dl>
        <p v-for="evidence in finding.evaluation.evidence" :key="evidence">{{ evidence }}</p>
        <ul>
          <li v-for="path in finding.ownerPaths" :key="path">
            <code>{{ path }}</code>
            <button type="button" :disabled="locked || stale" @click="editOwner(path)">
              Edit related native rule or owner
            </button>
          </li>
        </ul>
        <p v-if="finding.ownerPaths.length === 0">
          No authored editor owner is available for this native finding.
        </p>
      </section>
      <template v-if="reviewed.disposition === 'valid'"
        ><p v-if="redo" role="alert">
          A redo branch exists. Applying will discard that branch only with your explicit consent.
        </p>
        <button type="button" :disabled="!canApply" @click="apply">
          {{ redo ? messages.repair.redo : messages.repair.apply }}</button
        ><button
          type="button"
          :disabled="pending"
          @click="
            invalidateApproval();
            reviewTrigger?.focus();
          "
        >
          Edit manual intent instead
        </button></template
      >
      <template v-else
        ><p role="alert">
          This whole-selection change is invalid. No approval or command was created. Required rules
          remain enforced.
        </p>
        <button
          type="button"
          :disabled="locked"
          @click="
            draft.kind = 'replace';
            formHeading?.focus();
          "
        >
          Choose a same-shift replacement</button
        ><button type="button" :disabled="locked" @click="openPreference">
          {{ messages.repair.preference }}</button
        ><button type="button" :disabled="locked || diagnosticLoading" @click="openDiagnostic">
          Check chosen current-result replacement diagnostic
        </button>
        <p>{{ messages.repair.diagnosticScope }}</p>
        <p v-if="diagnosticStatus" role="status">{{ diagnosticStatus }}</p>
        <p v-if="diagnosticLoading" role="status">
          Checking exact chosen current-result decisions…
        </p></template
      >
    </section>
    <section
      v-if="preferenceOpen"
      class="state-panel field-stack grid-cols-1"
      aria-label="Time preference alternative"
    >
      <h4 ref="preferenceHeading" tabindex="-1">Time preference and re-optimize</h4>
      <p>{{ messages.repair.preferenceScope }}</p>
      <p v-if="preferenceInterval">
        Exact interval: {{ preferenceInterval.startsAt }} – {{ preferenceInterval.endsAt }}
      </p>
      <form @submit.prevent="previewPreference">
        <fieldset :disabled="locked || preference.state.pending">
          <legend>Preference intent</legend>
          <label
            >Direction
            <select v-model="preferenceDirection">
              <option value="prefer">Prefer</option>
              <option value="avoid">Avoid</option>
            </select></label
          ><label
            >Priority
            <select v-model="preferencePriority">
              <option value="low">Low</option>
              <option value="normal">Normal</option>
              <option value="high">High</option>
              <option value="veryHigh">Very high</option>
            </select></label
          ><label
            >Preference weight <input v-model="preferenceWeight" type="text" inputmode="numeric"
          /></label>
        </fieldset>
        <button type="submit" :disabled="locked || preference.state.pending || !preferenceInterval">
          Review normal Time preference command</button
        ><button
          type="button"
          :disabled="preference.state.applying"
          @click="
            preferenceOpen = false;
            preference.invalidate();
            reviewHeading?.focus();
          "
        >
          Cancel preference alternative; keep manual draft
        </button>
      </form>
      <p v-if="preference.state.pending" role="status">
        Waiting for native preference command review…
      </p>
      <p v-if="preference.state.error" role="alert">{{ preference.state.error }}</p>
      <template v-if="preference.state.review"
        ><h5>Native preference command preview</h5>
        <pre class="whitespace-pre-wrap">{{
          JSON.stringify(preference.state.review.proposed, null, 2)
        }}</pre>
        <pre class="whitespace-pre-wrap">{{
          JSON.stringify(preference.state.review.changes.items, null, 2)
        }}</pre>
        <p
          v-for="(warning, index) in [
            ...preference.state.review.warnings.changes,
            ...preference.state.review.warnings.proposed,
          ]"
          :key="index"
        >
          {{ warning.message }}
        </p>
        <button
          v-if="preference.state.review.changes.continuation"
          type="button"
          :disabled="preference.state.pending"
          @click="preference.page()"
        >
          Next preference command changes
        </button>
        <p v-if="preference.state.redoRequired" role="alert">
          This preference command needs explicit redo-branch truncation consent.
        </p>
        <button
          type="button"
          :disabled="locked || preference.state.pending"
          @click="applyPreference"
        >
          {{
            preference.state.redoRequired
              ? "Apply Time preference and explicitly discard redo branch"
              : "Apply reviewed Time preference, then re-optimize"
          }}
        </button></template
      >
    </section>
    <section
      v-if="diagnosticOpen && diagnosticAssignment && diagnosticSummary"
      class="state-panel field-stack grid-cols-1"
      aria-label="Current accepted-source global diagnostic"
    >
      <h4>Current accepted-source global diagnostic — other assignments free</h4>
      <p>{{ messages.repair.diagnosticScope }}</p>
      <p>
        Chosen accepted source <code>{{ diagnosticSummary.result.solutionId }}</code
        >, checksum <code>{{ diagnosticSummary.result.resultChecksum }}</code
        >, immutable revision {{ diagnosticSummary.scenarioRevision }}.
      </p>
      <p>
        Choose the intended replacement person <code>{{ draft.targetPersonId }}</code> in the native
        diagnostic below. Only its actual captured evidence can enable a diagnostic copy.
      </p>
      <AcceptedCounterfactual
        :scenario-id="project.scenarioId"
        :summary="diagnosticSummary"
        :current-revision="project.revision"
        :assignment="diagnosticAssignment"
        :home="home"
      />
      <button
        type="button"
        @click="
          diagnosticOpen = false;
          reviewHeading?.focus();
        "
      >
        Close current accepted-source diagnostic; keep manual draft
      </button>
    </section>
    <RouteLeaveGuard
      :home="home"
      :dirty="dirty && !alternativeNavigation"
      :pending="(pending && (applying || dirty)) || preference.state.pending"
      :discard="
        () => {
          if (alternativeNavigation) {
            saveDraft();
            invalidateApproval();
          } else cancelDraft();
        }
      "
    />
  </section>
</template>
