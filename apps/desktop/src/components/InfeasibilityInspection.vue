<script setup lang="ts">
import { computed, nextTick, onMounted, onScopeDispose, ref, shallowRef, watch } from "vue";

import {
  createDiagnosticCopy,
  exportDiagnostic,
  explainSolution,
  getApplicationSettings,
  getSolveDiagnosticsSummary,
  SetupOperationScope,
  type ApiResponseDto,
  type CounterfactualConditionV1,
  type DiagnosticCopyResultV1,
  type DiagnosticExportMetadataV1,
  type EvidenceMessageV1,
  type InfeasibilityEvidenceV1,
  type InfeasibilityExplanationContextV1,
  type OperationPhaseV1,
  type Revision,
  type SetupOperation,
  type SolutionExplanationDtoV2,
  type SolveRunId,
  type UuidV7,
} from "../api/generated";
import { isOperationCancelled, safeMessage, type ProjectHomeController } from "../project-home";
import { messages } from "../messages";
import { explanationMessage } from "./explanations/messages";
import ConflictCard from "./explanations/ConflictCard.vue";
import type { ExplanationUiState } from "./explanations/types";

interface InspectionSource {
  readonly scenarioId: UuidV7;
  readonly currentRevision: Revision;
  readonly sourceRevision: Revision;
  readonly runId: SolveRunId;
  readonly runManifestChecksum?: string;
  readonly condition?: CounterfactualConditionV1;
}
interface RuleAction {
  readonly action: "inspect" | "edit";
  readonly scenarioId: UuidV7;
  readonly sourceRevision: Revision;
  readonly runId: SolveRunId;
  readonly runManifestChecksum: string;
  readonly evidenceChecksum: string;
  readonly conflictId: string;
  readonly ruleId: UuidV7;
}
interface ActionIdentity {
  readonly scenarioId: UuidV7;
  readonly currentRevision: Revision;
  readonly sourceRevision: Revision;
  readonly runId: SolveRunId;
  readonly runManifestChecksum: string;
  readonly evidenceChecksum: string;
  readonly conflictId: string;
}
interface CopyGroupReview {
  readonly groupId: string;
  readonly sourceKind: string;
  readonly sourceCaption: string;
  readonly requiredRules: readonly UuidV7[];
}
interface CopyReview extends CopyGroupReview {
  readonly source: ActionIdentity;
  readonly owner: ProjectHomeController;
  readonly sourceKey: string;
  readonly libraryEpoch: number;
  readonly expectedLibraryRevision: Revision;
}
interface CopyReceipt extends CopyGroupReview {
  readonly result: DiagnosticCopyResultV1;
  readonly source: ActionIdentity;
}
interface ExportReceipt {
  readonly metadata: DiagnosticExportMetadataV1;
}

const props = withDefaults(
  defineProps<{
    source: InspectionSource;
    ruleNavigationAvailable?: boolean;
    headingLevel?: 5 | 6;
    home?: ProjectHomeController | undefined;
  }>(),
  { headingLevel: 5, ruleNavigationAvailable: false, home: undefined },
);
const emit = defineEmits<{
  close: [];
  ruleAction: [action: RuleAction];
  reviewCopy: [groupId: string];
  exportDiagnostic: [];
}>();

const heading = ref<HTMLElement | null>(null);
const cardHeadingLevel = computed<6 | 7>(() => (props.headingLevel === 5 ? 6 : 7));
const explanation = ref<SolutionExplanationDtoV2 | null>(null);
const state = ref<ExplanationUiState>("loading");
const error = ref<string | null>(null);
const cancelRequested = ref(false);
const infeasibility = computed<InfeasibilityEvidenceV1 | null>(() => {
  const payload = explanation.value?.explanation.evidence.evidence;
  return payload?.kind === "infeasibility" ? payload.infeasibility : null;
});
const renderedMessages = computed<readonly EvidenceMessageV1[]>(
  () => explanation.value?.explanation.rendered.messages ?? [],
);
const context = computed<InfeasibilityExplanationContextV1 | null>(
  () => explanation.value?.infeasibilityContext ?? null,
);
const condition = computed<CounterfactualConditionV1 | null>(() => {
  const payload = explanation.value?.explanation.evidence.evidence;
  return payload?.kind === "infeasibility" ? (payload.condition ?? null) : null;
});
// Exact native Workforce evidence source-kind keys; never infer relaxability from rule UUIDs.
const AUTHORED_REQUIRED_SOURCE_KINDS = new Set([
  "official.workforce.rule.eligibility",
  "official.workforce.rule.availability",
  "official.workforce.rule.coverage",
  "official.workforce.rule.no-overlap",
  "official.workforce.rule.minimum-rest",
  "official.workforce.rule.maximum-assignment-count",
  "official.workforce.rule.workload-quota",
  "official.workforce.rule.maximum-hours",
  "official.workforce.rule.maximum-consecutive",
  "official.workforce.rule.maximum-worked-weekends",
  "official.workforce.rule.required-skill-mix",
  "official.workforce.rule.fixed-assignment",
  "official.workforce.rule.mutual-assignment-restriction",
  "official.workforce.rule.transition-time",
]);
const copyReview = shallowRef<CopyReview | null>(null);
const copyReviewLoading = ref(false);
const copyReviewError = ref<string | null>(null);
const actionError = ref<string | null>(null);
const activeAction = ref<"copy" | "export" | null>(null);
const actionCancelAvailable = ref(false);
const actionSettling = ref(false);
const copyReceipt = shallowRef<CopyReceipt | null>(null);
const exportReceipt = shallowRef<ExportReceipt | null>(null);
const copyReviewHeading = ref<HTMLElement | null>(null);
const copyReceiptHeading = ref<HTMLElement | null>(null);
const exportReceiptHeading = ref<HTMLElement | null>(null);
const actionStatusHeading = ref<HTMLElement | null>(null);
const actionIdentity = computed<ActionIdentity | null>(() => {
  const sourceContext = context.value;
  if (state.value !== "ready" || props.home === undefined || sourceContext === null) return null;
  return {
    currentRevision: props.source.currentRevision,
    scenarioId: props.source.scenarioId,
    sourceRevision: sourceContext.sourceRevision,
    runId: sourceContext.runId,
    runManifestChecksum: sourceContext.runManifestChecksum,
    evidenceChecksum: sourceContext.evidenceChecksum,
    conflictId: sourceContext.conflictId,
  };
});
const sourceBindingKey = computed(() => {
  const identity = actionIdentity.value;
  const evidenceChecksum = explanation.value?.explanation.evidence.checksum;
  if (identity === null || evidenceChecksum === undefined) return null;
  return JSON.stringify({
    identity,
    currentRevision: props.source.currentRevision,
    condition: condition.value,
    explanationEvidenceChecksum: evidenceChecksum,
    conflictId: context.value?.conflictId,
  });
});
const diagnosticActionsAvailable = computed(() => actionIdentity.value !== null);
const copyableGroupIds = computed<readonly string[]>(() => {
  if (!diagnosticActionsAvailable.value || infeasibility.value?.type !== "conflict") return [];
  return infeasibility.value.groups
    .filter((group) => {
      const review = copyGroupReview(group.groupId);
      return review !== null && AUTHORED_REQUIRED_SOURCE_KINDS.has(review.sourceKind);
    })
    .map((group) => group.groupId);
});
const actionButtonsDisabled = computed(
  () =>
    activeAction.value !== null ||
    copyReviewLoading.value ||
    copyReview.value !== null ||
    (props.home?.state.busyAction !== null && props.home?.state.busyAction !== undefined),
);
const actionProgress = computed(() => {
  if (activeAction.value === null) return null;
  const currentOperation = props.home?.state.operation;
  if (actionSettling.value || currentOperation?.settled)
    return explanationMessage("conflict.actionFinalizing");
  if (currentOperation?.cancellationRequested)
    return explanationMessage("conflict.actionCancelRequested");
  const phase: OperationPhaseV1 | null = currentOperation?.phase ?? null;
  if (phase === null)
    return activeAction.value === "copy"
      ? explanationMessage("conflict.copyWorking")
      : explanationMessage("conflict.exporting");
  return messages.operations.phases[phase];
});
const canCancelAction = computed(() => {
  const currentOperation = props.home?.state.operation;
  return (
    actionCancelAvailable.value &&
    activeAction.value !== null &&
    currentOperation !== null &&
    currentOperation !== undefined &&
    !currentOperation.settled &&
    currentOperation.cancel !== null &&
    !currentOperation.cancellationRequested
  );
});
let copyReviewGeneration = 0;
let generation = 0;
let alive = true;
let scope: SetupOperationScope | null = null;
let operation: ReturnType<typeof explainSolution> | null = null;

function sameCondition(
  left: CounterfactualConditionV1 | undefined,
  right: CounterfactualConditionV1 | undefined,
): boolean {
  if (left === undefined || right === undefined) return left === right;
  if (left.schemaVersion !== right.schemaVersion || left.checksum !== right.checksum) return false;
  const a = left.condition;
  const b = right.condition;
  if (a.type !== b.type) return false;
  if (a.type === "replaceAssignment" && b.type === "replaceAssignment")
    return (
      a.targetAssignmentId === b.targetAssignmentId &&
      a.replacedAssignmentId === b.replacedAssignmentId
    );
  if (
    (a.type === "forceAssignmentValue" || a.type === "forbidAssignmentValue") &&
    a.type === b.type
  ) {
    const av = a.value;
    const bv = b.value;
    if (av.type !== bv.type || a.assignmentId !== b.assignmentId) return false;
    if (av.type === "boolean" && bv.type === "boolean") return av.value === bv.value;
    if (av.type === "integer" && bv.type === "integer") return av.value === bv.value;
    if (av.type === "absent" && bv.type === "absent") return true;
    return (
      av.type === "interval" &&
      bv.type === "interval" &&
      av.value.start === bv.value.start &&
      av.value.duration === bv.value.duration &&
      av.value.end === bv.value.end
    );
  }
  return false;
}

function textFact(message: EvidenceMessageV1, name: string): string | null {
  const value = message.parameters[`official.workforce.fact.${name}`];
  return value?.type === "text" && value.value.trim().length > 0 ? value.value : null;
}

function integerFact(message: EvidenceMessageV1, name: string): number | null {
  const value = message.parameters[`official.workforce.fact.${name}`];
  if (value?.type !== "integer" || !/^(0|[1-9][0-9]*)$/.test(value.value)) return null;
  const parsed = Number(value.value);
  return Number.isSafeInteger(parsed) ? parsed : null;
}
function copyGroupReview(groupId: string): CopyGroupReview | null {
  const evidence = infeasibility.value;
  if (evidence?.type !== "conflict") return null;
  const group = evidence.groups.find((item) => item.groupId === groupId);
  if (group === undefined || new Set(group.requiredRules).size !== group.requiredRules.length)
    return null;
  const expectedRules = new Set(group.requiredRules);
  const message = renderedMessages.value.find(
    (item) =>
      item.messageKey === "official.workforce.infeasibility.conflict_group" &&
      textFact(item, "group_id") === groupId,
  );
  const sourceKind = message === undefined ? null : textFact(message, "source_kind");
  const sourceCaption = message === undefined ? null : textFact(message, "source_caption");
  if (
    message === undefined ||
    message.rules.length !== expectedRules.size ||
    !message.rules.every((ruleId) => expectedRules.has(ruleId)) ||
    sourceKind === null ||
    sourceCaption === null
  )
    return null;
  return { groupId, sourceKind, sourceCaption, requiredRules: group.requiredRules };
}

function completeRendering(
  evidence: InfeasibilityEvidenceV1,
  messages: readonly EvidenceMessageV1[],
  expectedCondition: CounterfactualConditionV1 | undefined,
): boolean {
  if (messages.length > 514) return false;
  const statuses = messages.filter(
    (message) => message.messageKey === "official.workforce.infeasibility.status",
  );
  const fixed = messages.filter(
    (message) => message.messageKey === "official.workforce.infeasibility.fixed_scope",
  );
  const groups = messages.filter(
    (message) => message.messageKey === "official.workforce.infeasibility.conflict_group",
  );
  const status = statuses[0];
  const fixedScope = fixed[0];
  if (
    statuses.length !== 1 ||
    fixed.length !== 1 ||
    status === undefined ||
    fixedScope === undefined ||
    messages.length !== 2 + groups.length ||
    textFact(status, "summary") === null ||
    integerFact(fixedScope, "fixed_fact_count") === null ||
    integerFact(fixedScope, "hard_lock_count") === null ||
    textFact(fixedScope, "summary") === null
  )
    return false;

  const statusKind = textFact(status, "status");
  if (evidence.type === "conflict") {
    if (
      evidence.groups.length > 512 ||
      statusKind !== (evidence.minimality === "provenMinimal" ? "minimal" : "sufficient") ||
      groups.length !== evidence.groups.length
    )
      return false;
    const byGroup = new Map<string, EvidenceMessageV1[]>();
    for (const message of groups) {
      const groupId = textFact(message, "group_id");
      if (groupId === null) return false;
      const matches = byGroup.get(groupId);
      if (matches === undefined) byGroup.set(groupId, [message]);
      else matches.push(message);
    }
    const seen = new Set<string>();
    for (const group of evidence.groups) {
      if (
        group.requiredRules.length === 0 ||
        group.requiredRules.length > 1_024 ||
        seen.has(group.groupId)
      )
        return false;
      seen.add(group.groupId);
      const matches = byGroup.get(group.groupId);
      const message = matches?.[0];
      if (
        matches?.length !== 1 ||
        message === undefined ||
        new Set(group.requiredRules).size !== group.requiredRules.length
      )
        return false;
      const renderedRules = new Set(message.rules);
      if (
        renderedRules.size !== group.requiredRules.length ||
        group.requiredRules.some((ruleId) => !renderedRules.has(ruleId)) ||
        textFact(message, "source_caption") === null ||
        textFact(message, "source_kind") === null ||
        integerFact(message, "source_rule_count") !== group.requiredRules.length
      )
        return false;
    }
  } else if (
    groups.length !== 0 ||
    statusKind !==
      (evidence.reason === "foundationalInfeasibility" ? "foundational" : "unavailable")
  ) {
    return false;
  }

  const conditionFacts = Object.keys(status.parameters).filter((key) =>
    key.startsWith("official.workforce.fact.condition_"),
  );
  const conditionChecksum = status.parameters["official.workforce.fact.condition_checksum"];
  return expectedCondition === undefined
    ? conditionFacts.length === 0
    : conditionChecksum?.type === "text" && conditionChecksum.value === expectedCondition.checksum;
}

function contextMatches(result: SolutionExplanationDtoV2, source: InspectionSource): boolean {
  const payload = result.explanation.evidence.evidence;
  if (payload.kind !== "infeasibility" || !sameCondition(payload.condition, source.condition))
    return false;
  const returned = result.infeasibilityContext;
  if (returned === undefined)
    return (
      source.condition === undefined &&
      payload.infeasibility.type === "unavailable" &&
      payload.infeasibility.reason === "notRecorded"
    );
  return (
    returned.runId === source.runId &&
    returned.sourceRevision === source.sourceRevision &&
    returned.runManifestChecksum === source.runManifestChecksum &&
    /^[0-9a-f]{64}$/.test(returned.evidenceChecksum) &&
    returned.conflictId === `infeasibility.${returned.evidenceChecksum}`
  );
}

function validExplanation(
  response: ApiResponseDto<SolutionExplanationDtoV2>,
  source: InspectionSource,
): boolean {
  const result = response.result;
  const evidence = result.explanation.evidence;
  const payload = evidence.evidence;
  return (
    result.scenarioId === source.scenarioId &&
    result.currentRevision === source.currentRevision &&
    result.scenarioRevisions.length === 1 &&
    result.scenarioRevisions[0] === source.sourceRevision &&
    payload.kind === "infeasibility" &&
    contextMatches(result, source) &&
    result.explanation.rendered.kind === "infeasibility" &&
    result.explanation.rendered.evidenceChecksum === evidence.checksum &&
    completeRendering(payload.infeasibility, result.explanation.rendered.messages, source.condition)
  );
}

function stopOperation(): void {
  const active = operation;
  operation = null;
  if (active?.isCurrent()) void active.cancel().catch(() => undefined);
  scope?.dispose();
  scope = null;
}

async function load(): Promise<void> {
  const captured = ++generation;
  const { condition: requestedCondition, runManifestChecksum, ...sourceIdentity } = props.source;
  const source: InspectionSource = {
    ...sourceIdentity,
    ...(runManifestChecksum === undefined ? {} : { runManifestChecksum }),
    ...(requestedCondition === undefined ? {} : { condition: structuredClone(requestedCondition) }),
  };
  const sourceKey = JSON.stringify(source);
  const stillCurrent = (): boolean =>
    alive && captured === generation && JSON.stringify(props.source) === sourceKey;
  stopOperation();
  explanation.value = null;
  error.value = null;
  cancelRequested.value = false;
  state.value = "loading";
  let manifestChecksum = source.runManifestChecksum;
  try {
    if (manifestChecksum === undefined) {
      const diagnostics = await getSolveDiagnosticsSummary(source.scenarioId, source.runId);
      if (!stillCurrent()) return;
      const summary = diagnostics.result;
      if (
        diagnostics.currentRevision !== source.currentRevision ||
        summary.job.scenarioId !== source.scenarioId ||
        summary.job.runId !== source.runId ||
        summary.job.scenarioRevision !== source.sourceRevision ||
        summary.job.status !== "infeasible" ||
        summary.job.solutionId !== null
      ) {
        state.value = "stale";
        error.value = "The run manifest does not match this recorded infeasible source.";
        return;
      }
      if (summary.manifestChecksum === null) {
        state.value = "unavailable";
        error.value = "This run has no recorded native manifest identity for conflict inspection.";
        return;
      }
      manifestChecksum = summary.manifestChecksum;
    }
    if (!stillCurrent()) return;
    const requestScope = new SetupOperationScope(source.scenarioId, source.sourceRevision);
    scope = requestScope;
    operation = explainSolution(requestScope, {
      schemaVersion: 1,
      subject: {
        kind: "infeasibility",
        solveRunId: source.runId,
        runManifestChecksum: manifestChecksum,
        conflictId: null,
      },
    });
    const response = await operation.result;
    if (!stillCurrent()) return;
    if (!validExplanation(response, { ...source, runManifestChecksum: manifestChecksum })) {
      state.value = "stale";
      error.value =
        "The native response did not match this immutable run, source revision, or recorded condition.";
      return;
    }
    explanation.value = response.result;
    state.value = "ready";
  } catch (failure) {
    if (!stillCurrent()) return;
    if (isOperationCancelled(failure)) state.value = "cancelled";
    else {
      state.value = "internalFailure";
      error.value = safeMessage(failure);
    }
  } finally {
    if (captured === generation) {
      const completed = operation;
      operation = null;
      if (completed !== null) void completed.release().catch(() => undefined);
      scope?.dispose();
      scope = null;
    }
  }
}

async function cancel(): Promise<void> {
  const active = operation;
  if (cancelRequested.value) return;
  if (active === null) {
    generation += 1;
    state.value = "cancelled";
    error.value = null;
    return;
  }
  cancelRequested.value = true;
  try {
    await active.cancel();
  } catch (failure) {
    if (alive && operation === active) {
      cancelRequested.value = false;
      error.value = safeMessage(failure);
    }
  }
}

function invalidateCopyReview(): void {
  if (copyReview.value === null && !copyReviewLoading.value) return;
  copyReviewGeneration += 1;
  copyReview.value = null;
  copyReviewLoading.value = false;
  copyReviewError.value = explanationMessage("conflict.copyReviewInvalidated");
}

watch(
  [
    () => sourceBindingKey.value,
    () => props.home,
    () => props.home?.state.libraryEpoch,
    () => props.home?.state.busyAction,
  ],
  (current, previous) => {
    if (current.some((value, index) => value !== previous[index])) invalidateCopyReview();
  },
  { flush: "sync" },
);

async function reviewCopy(groupId: string): Promise<void> {
  const homeOwner = props.home;
  const identity = actionIdentity.value;
  const sourceKey = sourceBindingKey.value;
  const group = copyGroupReview(groupId);
  if (
    homeOwner === undefined ||
    sourceKey === null ||
    identity === null ||
    group === null ||
    !copyableGroupIds.value.includes(groupId) ||
    homeOwner.state.busyAction !== null ||
    actionButtonsDisabled.value
  )
    return;
  const token = ++copyReviewGeneration;
  const currentReview = () => alive && token === copyReviewGeneration;
  const libraryEpoch = homeOwner.state.libraryEpoch;
  copyReview.value = null;
  copyReviewError.value = null;
  actionError.value = null;
  copyReviewLoading.value = true;
  try {
    const { result } = await getApplicationSettings();
    if (!currentReview()) return;
    const currentOwnerState = homeOwner.state;
    if (
      props.home !== homeOwner ||
      sourceBindingKey.value !== sourceKey ||
      currentOwnerState.libraryEpoch !== libraryEpoch ||
      currentOwnerState.busyAction !== null
    ) {
      invalidateCopyReview();
      return;
    }
    const currentGroup = copyGroupReview(groupId);
    if (
      currentGroup === null ||
      currentGroup.sourceKind !== group.sourceKind ||
      currentGroup.sourceCaption !== group.sourceCaption ||
      currentGroup.requiredRules.length !== group.requiredRules.length ||
      currentGroup.requiredRules.some((ruleId, index) => ruleId !== group.requiredRules[index])
    ) {
      invalidateCopyReview();
      return;
    }
    copyReview.value = {
      ...group,
      source: identity,
      owner: homeOwner,
      sourceKey,
      libraryEpoch,
      expectedLibraryRevision: result.libraryRevision,
    };
    await nextTick();
    if (currentReview()) copyReviewHeading.value?.focus();
  } catch (failure) {
    if (alive && token === copyReviewGeneration) copyReviewError.value = safeMessage(failure);
  } finally {
    if (alive && token === copyReviewGeneration) copyReviewLoading.value = false;
  }
}

async function cancelCopyReview(): Promise<void> {
  copyReviewGeneration += 1;
  copyReview.value = null;
  copyReviewLoading.value = false;
  copyReviewError.value = null;
  await nextTick();
  if (alive) heading.value?.focus();
}

async function createCopy(): Promise<void> {
  const review = copyReview.value;
  const homeOwner = props.home;
  if (review === null || homeOwner === undefined || activeAction.value !== null) return;
  const identity = review.source;
  const currentGroup = copyGroupReview(review.groupId);
  if (
    review.owner !== homeOwner ||
    review.sourceKey !== sourceBindingKey.value ||
    review.libraryEpoch !== homeOwner.state.libraryEpoch ||
    homeOwner.state.busyAction !== null ||
    currentGroup === null ||
    currentGroup.sourceKind !== review.sourceKind ||
    !AUTHORED_REQUIRED_SOURCE_KINDS.has(currentGroup.sourceKind) ||
    currentGroup.sourceCaption !== review.sourceCaption ||
    currentGroup.requiredRules.length !== review.requiredRules.length ||
    currentGroup.requiredRules.some((ruleId, index) => ruleId !== review.requiredRules[index])
  ) {
    invalidateCopyReview();
    return;
  }

  copyReviewGeneration += 1;
  copyReview.value = null;
  copyReviewError.value = null;
  actionError.value = null;
  actionCancelAvailable.value = false;
  actionSettling.value = false;
  activeAction.value = "copy";
  await nextTick();
  actionStatusHeading.value?.focus();
  let nativeOperation: SetupOperation<DiagnosticCopyResultV1> | null = null;
  try {
    const response = await homeOwner.runOperation({
      action: "solution-diagnostic-copy",
      label: explanationMessage("conflict.copyReviewAction"),
      execute: async (report) => {
        const sourceScope = new SetupOperationScope(identity.scenarioId, identity.sourceRevision);
        try {
          const active = createDiagnosticCopy(
            sourceScope,
            {
              runId: identity.runId,
              runManifestChecksum: identity.runManifestChecksum,
              evidenceChecksum: identity.evidenceChecksum,
              groupId: review.groupId,
              expectedLibraryRevision: review.expectedLibraryRevision,
            },
            report,
          );
          nativeOperation = active;
          actionCancelAvailable.value = true;
          try {
            return await active.result;
          } finally {
            actionCancelAvailable.value = false;
            actionSettling.value = true;
          }
        } finally {
          const completed = nativeOperation;
          nativeOperation = null;
          if (completed !== null) await completed.release().catch(() => undefined);
          sourceScope.dispose();
        }
      },
      cancel: async () => {
        if (actionCancelAvailable.value) await nativeOperation?.cancel();
      },
      success: (result) =>
        explanationMessage("conflict.copyCreated", {
          title: result.project.title,
          scenarioId: result.project.scenarioId,
        }),
    });
    if (alive) {
      copyReceipt.value = {
        groupId: review.groupId,
        sourceKind: review.sourceKind,
        sourceCaption: review.sourceCaption,
        requiredRules: review.requiredRules,
        result: response.result,
        source: identity,
      };
      await nextTick();
      copyReceiptHeading.value?.focus();
    }
  } catch (failure) {
    if (alive)
      actionError.value = isOperationCancelled(failure)
        ? explanationMessage("conflict.copyCancelled")
        : safeMessage(failure);
  } finally {
    actionCancelAvailable.value = false;
    actionSettling.value = false;
    if (alive) activeAction.value = null;
  }
}

async function exportReport(): Promise<void> {
  const homeOwner = props.home;
  const identity = actionIdentity.value;
  if (
    homeOwner === undefined ||
    identity === null ||
    activeAction.value !== null ||
    homeOwner.state.busyAction !== null
  )
    return;
  exportReceipt.value = null;
  actionError.value = null;
  actionCancelAvailable.value = false;
  actionSettling.value = false;
  activeAction.value = "export";
  await nextTick();
  actionStatusHeading.value?.focus();
  let nativeOperation: SetupOperation<DiagnosticExportMetadataV1> | null = null;
  try {
    const response = await homeOwner.runOperation({
      action: "solution-diagnostic-export",
      label: explanationMessage("conflict.exporting"),
      refreshLibrary: false,
      execute: async (report) => {
        const sourceScope = new SetupOperationScope(identity.scenarioId, identity.sourceRevision);
        try {
          const active = exportDiagnostic(
            sourceScope,
            {
              runId: identity.runId,
              runManifestChecksum: identity.runManifestChecksum,
              evidenceChecksum: identity.evidenceChecksum,
            },
            report,
          );
          nativeOperation = active;
          actionCancelAvailable.value = true;
          try {
            return await active.result;
          } finally {
            actionCancelAvailable.value = false;
            actionSettling.value = true;
          }
        } finally {
          const completed = nativeOperation;
          nativeOperation = null;
          if (completed !== null) await completed.release().catch(() => undefined);
          sourceScope.dispose();
        }
      },
      cancel: async () => {
        if (actionCancelAvailable.value) await nativeOperation?.cancel();
      },
      success: (metadata) =>
        explanationMessage("conflict.exported", {
          sourceRevision: metadata.sourceRevision,
          observedCurrentRevision: metadata.observedCurrentRevision,
        }),
    });
    if (alive) {
      exportReceipt.value = { metadata: response.result };
      await nextTick();
      exportReceiptHeading.value?.focus();
    }
  } catch (failure) {
    if (alive)
      actionError.value = isOperationCancelled(failure)
        ? explanationMessage("conflict.exportCancelled")
        : safeMessage(failure);
  } finally {
    actionCancelAvailable.value = false;
    actionSettling.value = false;
    if (alive) activeAction.value = null;
  }
}

function cancelDiagnosticAction(): void {
  if (canCancelAction.value) void props.home?.cancelOperation();
}

watch(
  () => JSON.stringify(props.source),
  () => void load(),
  { immediate: true },
);
onMounted(() => void nextTick(() => heading.value?.focus()));
onScopeDispose(() => {
  alive = false;
  generation += 1;
  stopOperation();
});
</script>

<template>
  <section aria-label="Recorded infeasibility inspection" data-infeasibility-inspection>
    <h5 v-if="headingLevel === 5" ref="heading" tabindex="-1">Recorded conflict evidence</h5>
    <h6 v-else ref="heading" tabindex="-1">Recorded conflict evidence</h6>
    <p>
      Source revision {{ source.sourceRevision }} · current observed revision
      {{ source.currentRevision }}.
      <span v-if="source.sourceRevision !== source.currentRevision">
        This is historical evidence; it does not describe the current scenario.
      </span>
    </p>
    <button type="button" @click="emit('close')">Close conflict inspection</button>
    <ConflictCard
      :scenario-id="source.scenarioId"
      class="mt-3"
      :heading-level="cardHeadingLevel"
      :evidence="infeasibility"
      :messages="renderedMessages"
      :condition="condition"
      :context="context"
      :source-revision="source.sourceRevision"
      :current-revision="source.currentRevision"
      :rule-navigation-available="ruleNavigationAvailable"
      :diagnostic-actions-available="diagnosticActionsAvailable"
      :copyable-group-ids="copyableGroupIds"
      :action-buttons-disabled="actionButtonsDisabled"
      :state="state"
      :error-message="error"
      :cancel-requested="cancelRequested"
      @cancel="cancel"
      @rule-action="emit('ruleAction', $event)"
      @review-copy="reviewCopy"
      @export-diagnostic="exportReport"
    />
    <p v-if="copyReviewLoading" class="mt-3 text-sm text-muted" role="status">
      {{ explanationMessage("conflict.copyReviewLoading") }}
      <button type="button" class="ml-2 underline" @click="cancelCopyReview">
        {{ explanationMessage("conflict.copyReviewCancel") }}
      </button>
    </p>
    <p v-if="copyReviewError" class="mt-3 text-sm text-danger" role="alert">
      {{ copyReviewError }}
    </p>
    <section
      v-if="copyReview"
      class="mt-4 rounded-md border border-line bg-raised p-4"
      aria-label="Review diagnostic copy"
      data-diagnostic-copy-review
    >
      <p
        ref="copyReviewHeading"
        role="heading"
        :aria-level="cardHeadingLevel + 1"
        tabindex="-1"
        class="font-semibold text-ink"
      >
        {{ explanationMessage("conflict.copyReviewHeading") }}
      </p>
      <p class="mt-2 text-sm text-ink">
        {{ explanationMessage("conflict.copyReviewOriginal") }}
      </p>
      <dl class="mt-3 grid gap-2 text-sm">
        <div>
          <dt class="font-semibold">Original scenario ID</dt>
          <dd>
            <code class="break-all">{{ copyReview.source.scenarioId }}</code>
          </dd>
        </div>
        <div>
          <dt class="font-semibold">Recorded source revision</dt>
          <dd>{{ copyReview.source.sourceRevision }}</dd>
        </div>
        <div>
          <dt class="font-semibold">Observed current revision</dt>
          <dd>{{ copyReview.source.currentRevision }}</dd>
        </div>
        <div>
          <dt class="font-semibold">Recorded run ID</dt>
          <dd>
            <code class="break-all">{{ copyReview.source.runId }}</code>
          </dd>
        </div>
        <div>
          <dt class="font-semibold">Run manifest checksum</dt>
          <dd>
            <code class="break-all">{{ copyReview.source.runManifestChecksum }}</code>
          </dd>
        </div>
        <div>
          <dt class="font-semibold">Conflict evidence checksum</dt>
          <dd>
            <code class="break-all">{{ copyReview.source.evidenceChecksum }}</code>
          </dd>
        </div>
        <div>
          <dt class="font-semibold">Conflict ID</dt>
          <dd>
            <code class="break-all">{{ copyReview.source.conflictId }}</code>
          </dd>
        </div>
        <div>
          <dt class="font-semibold">Selected mapped source group</dt>
          <dd>{{ copyReview.groupId }} · {{ copyReview.sourceKind }}</dd>
        </div>
        <div>
          <dt class="font-semibold">Exact source caption</dt>
          <dd>{{ copyReview.sourceCaption }}</dd>
        </div>
        <div>
          <dt class="font-semibold">Required rule UUIDs relaxed only in the copy</dt>
          <dd>
            <ul>
              <li v-for="ruleId in copyReview.requiredRules" :key="ruleId">
                <code class="break-all">{{ ruleId }}</code>
              </li>
            </ul>
          </dd>
        </div>
        <div>
          <dt class="font-semibold">
            {{ explanationMessage("conflict.copyReviewLibraryRevision") }}
          </dt>
          <dd>{{ copyReview.expectedLibraryRevision }}</dd>
        </div>
      </dl>
      <div class="mt-4 flex flex-wrap gap-2">
        <button
          type="button"
          class="rounded-md border border-line-strong px-3 py-2 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
          :disabled="
            home === undefined ||
            sourceBindingKey !== copyReview.sourceKey ||
            home.state.libraryEpoch !== copyReview.libraryEpoch ||
            home.state.busyAction !== null
          "
          @click="createCopy"
        >
          {{ explanationMessage("conflict.copyReviewConfirm") }}
        </button>
        <button
          type="button"
          class="rounded-md border border-line-strong px-3 py-2 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
          @click="cancelCopyReview"
        >
          {{ explanationMessage("conflict.copyReviewCancel") }}
        </button>
      </div>
    </section>
    <div v-if="actionProgress" class="mt-4" role="status">
      <p ref="actionStatusHeading" tabindex="-1">{{ actionProgress }}</p>
      <button
        v-if="canCancelAction"
        type="button"
        class="mt-2 rounded-md border border-line-strong px-3 py-2 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
        @click="cancelDiagnosticAction"
      >
        {{
          home?.state.operation?.cancellationRequested
            ? explanationMessage("conflict.actionCancelRequested")
            : explanationMessage("action.cancel")
        }}
      </button>
    </div>
    <p v-if="actionError" class="mt-3 text-sm text-danger" role="alert">{{ actionError }}</p>
    <section
      v-if="copyReceipt"
      class="mt-4 rounded-md border border-line bg-raised p-4"
      aria-label="Created diagnostic copy receipt"
      data-diagnostic-copy-receipt
    >
      <p
        ref="copyReceiptHeading"
        role="heading"
        :aria-level="cardHeadingLevel + 1"
        tabindex="-1"
        class="font-semibold text-ink"
      >
        {{
          explanationMessage("conflict.copyCreated", {
            title: copyReceipt.result.project.title,
            scenarioId: copyReceipt.result.project.scenarioId,
          })
        }}
      </p>
      <p class="mt-2 text-sm text-ink">
        Original scenario <code>{{ copyReceipt.source.scenarioId }}</code
        >, source revision {{ copyReceipt.source.sourceRevision }}, observed current revision
        {{ copyReceipt.source.currentRevision }}; run <code>{{ copyReceipt.source.runId }}</code
        >.
      </p>
      <p class="mt-1 break-all text-sm text-muted">
        Manifest <code>{{ copyReceipt.source.runManifestChecksum }}</code
        >; evidence <code>{{ copyReceipt.source.evidenceChecksum }}</code
        >; conflict ID <code>{{ copyReceipt.source.conflictId }}</code
        >.
      </p>
      <p class="mt-1 text-sm text-ink">
        Copy selection: {{ copyReceipt.groupId }} · {{ copyReceipt.sourceKind }} ·
        {{ copyReceipt.sourceCaption }}
      </p>
      <ul class="mt-1 text-sm text-muted" aria-label="Required rules relaxed in this copy">
        <li v-for="ruleId in copyReceipt.requiredRules" :key="ruleId">
          <code class="break-all">{{ ruleId }}</code>
        </li>
      </ul>
      <p class="mt-1 text-sm text-ink">
        New scenario ID
        <code class="break-all">{{ copyReceipt.result.project.scenarioId }}</code
        >;
        {{
          explanationMessage("conflict.copyCurrentLibrary", {
            revision: copyReceipt.result.libraryRevision,
          })
        }}
      </p>
      <p class="mt-2 text-sm text-ink">
        {{ explanationMessage("conflict.copyUndoGuidance") }}
      </p>
    </section>
    <section
      v-if="exportReceipt"
      class="mt-4 rounded-md border border-line bg-raised p-4"
      aria-label="Native diagnostic export receipt"
      data-diagnostic-export-receipt
    >
      <p
        ref="exportReceiptHeading"
        role="heading"
        :aria-level="cardHeadingLevel + 1"
        tabindex="-1"
        class="font-semibold text-ink"
      >
        {{
          explanationMessage("conflict.exported", {
            sourceRevision: exportReceipt.metadata.sourceRevision,
            observedCurrentRevision: exportReceipt.metadata.observedCurrentRevision,
          })
        }}
      </p>
      <p class="mt-2 text-sm text-ink">
        Native source scenario <code>{{ exportReceipt.metadata.scenarioId }}</code
        >, run <code class="break-all">{{ exportReceipt.metadata.runId }}</code
        >, manifest <code class="break-all">{{ exportReceipt.metadata.runManifestChecksum }}</code
        >, conflict evidence
        <code class="break-all">{{ exportReceipt.metadata.evidenceChecksum }}</code
        >.
      </p>
      <p class="mt-1 text-sm text-muted">
        {{ explanationMessage("conflict.exportMetadata") }}
      </p>
    </section>
  </section>
</template>
