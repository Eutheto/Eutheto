<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, shallowRef, watch } from "vue";
import { RouterLink, useRouter } from "vue-router";
import {
  adoptBaseSolution,
  getAppCapabilities,
  explainSolution,
  getScenarioSummary,
  getScenarioView,
  getSolutionView,
  listSolutions,
  newUuidV7,
  selectSolution,
  SetupOperationScope,
  type AcceptedResultRefV1,
  type AppCapabilitiesDto,
  type Revision,
  type ScenarioSummaryV2,
  type SetupOperation,
  type SolutionExplanationDtoV2,
  type ExplanationResultV2,
  type SolutionListDtoV1,
  type SolutionSummaryDtoV1,
  type SolutionComparisonV2,
  type UuidV7,
  type ManualEditSourceV1,
  type SolveIntent,
  type SolveBaselineSourceV1,
  type RunBaselineOriginal,
} from "../api/generated";
import type {
  WorkforceSetupEntityKind,
  WorkforceSetupFacts,
} from "../api/generated-domain-pack-contracts";
import {
  isOperationCancelled,
  isRedoBranchTruncation,
  isRevisionConflict,
  safeMessage,
  type ProjectHomeController,
  type ProjectSummary,
} from "../project-home";
import { formatDateTime, formatNumber, messages } from "../messages";
import AcceptedComparison from "./AcceptedComparison.vue";
import AcceptedSchedule from "./AcceptedSchedule.vue";
import OptimizeCapabilityPanel from "./OptimizeCapabilityPanel.vue";
import WorkforceRepairPanel from "./WorkforceRepairPanel.vue";
import RecordedBaselineDiff from "./RecordedBaselineDiff.vue";
import WorkforceExternalPlanPanel from "./WorkforceExternalPlanPanel.vue";
import WorkforceAlternativesPanel from "./WorkforceAlternativesPanel.vue";
import ExplanationPanel from "./explanations/ExplanationPanel.vue";
import {
  isWorkforceResultScheduleViewData,
  WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
} from "../api/generated-domain-pack-contracts";
import ValidationSummary from "./explanations/ValidationSummary.vue";
import type { ExplanationUiState, ConflictRuleAction } from "./explanations/types";
import { resolveValidationAddress, validationRoute } from "../validation-navigation";

const props = defineProps<{
  home: ProjectHomeController;
  project: ProjectSummary;
  locale?: string;
  libraryRevision?: Revision | null;
}>();
const router = useRouter();
const ruleNavigationPending = ref(false);
const ruleNavigationError = ref<string | null>(null);
let ruleNavigationScope: SetupOperationScope | null = null;

function clearRuleNavigation(): void {
  ruleNavigationScope?.dispose();
  ruleNavigationScope = null;
  ruleNavigationPending.value = false;
  ruleNavigationError.value = null;
}

async function openConflictRule(input: ConflictRuleAction): Promise<void> {
  const action = { ...input };
  const home = props.home;
  if (
    !current ||
    ruleNavigationPending.value ||
    props.project.archived ||
    props.home.state.busyAction !== null ||
    action.scenarioId !== props.project.scenarioId ||
    action.sourceRevision !== props.project.revision
  )
    return;
  const captured = generation;
  const libraryEpoch = home.state.libraryEpoch;
  const stillCurrent = () =>
    current &&
    generation === captured &&
    props.home === home &&
    home.state.libraryEpoch === libraryEpoch &&
    props.project.scenarioId === action.scenarioId &&
    props.project.revision === action.sourceRevision;
  const owned = new SetupOperationScope(action.scenarioId, action.sourceRevision);
  ruleNavigationScope = owned;
  ruleNavigationPending.value = true;
  ruleNavigationError.value = null;
  try {
    const { result } = await explainSolution(owned, {
      schemaVersion: 1,
      subject: {
        kind: "infeasibility",
        solveRunId: action.runId,
        runManifestChecksum: action.runManifestChecksum,
        conflictId: action.conflictId,
      },
    }).result;
    if (!stillCurrent()) return;
    const context = result.infeasibilityContext;
    const payload = result.explanation.evidence.evidence;
    if (
      result.currentRevision !== action.sourceRevision ||
      context?.evidenceChecksum !== action.evidenceChecksum ||
      payload.kind !== "infeasibility" ||
      payload.infeasibility.type !== "conflict" ||
      !payload.infeasibility.groups.some((group) => group.requiredRules.includes(action.ruleId))
    )
      throw new Error(messages.validationWorkspace.targetInvalid);
    const address = { collection: "rules" as const, id: action.ruleId, fieldPath: [] };
    const resolved = await resolveValidationAddress(owned, address);
    if (!stillCurrent()) return;
    if (resolved.route === null) throw new Error(messages.validationWorkspace.targetInvalid);
    await router.push(
      validationRoute(
        {
          ...address,
          scenarioId: action.scenarioId,
          revision: action.sourceRevision,
          libraryEpoch,
        },
        resolved.route,
      ),
    );
  } catch (failure) {
    if (stillCurrent()) ruleNavigationError.value = safeMessage(failure);
  } finally {
    owned.dispose();
    if (ruleNavigationScope === owned) {
      ruleNavigationScope = null;
      ruleNavigationPending.value = false;
    }
  }
}
const snapshot = shallowRef<{
  owner: ProjectHomeController;
  libraryEpoch: Revision;
  facts: WorkforceSetupFacts;
  summary: ScenarioSummaryV2;
  results: SolutionListDtoV1;
} | null>(null);
const snapshotMatchesProject = computed(() => {
  // Keep same-source receipts mounted during refresh; operations fence their own library epoch.
  const loaded = snapshot.value;
  return (
    loaded !== null &&
    loaded.owner === props.home &&
    loaded.summary.scenarioId === props.project.scenarioId &&
    loaded.summary.revision === props.project.revision
  );
});
const loading = ref(false);
const error = ref<string | null>(null);
const capabilities = shallowRef<AppCapabilitiesDto | null>(null);
const resultsRefreshing = ref(false);
const capabilitiesLoading = ref(false);
const capabilitiesError = ref<string | null>(null);
const inspectedSolutionId = ref<string | null>(null);
const chosenRepairCandidate = shallowRef<SolutionSummaryDtoV1 | null>(null);
const manualSource = shallowRef<ManualEditSourceV1 | null>(null);
const baselineMode = ref<"accepted" | "external">("accepted");
const externalDraftDirty = ref(false);
// Retain display defaults, not approval, so overview refreshes cannot destroy raw edits.
const externalInitialDates = shallowRef<WorkforceSetupFacts["initialWorkWindow"] | null>(null);
const externalSource = shallowRef<{
  scenarioId: string;
  revision: number;
  libraryEpoch: number;
  entityId: string;
} | null>(null);
const selectedBaseline = computed<SolveBaselineSourceV1 | null>(() => {
  if (baselineMode.value === "accepted") {
    const source = manualSource.value;
    return source?.scenarioId === props.project.scenarioId &&
      source.revision === props.project.revision
      ? { kind: "accepted" }
      : null;
  }
  const source = externalSource.value;
  return source?.scenarioId === props.project.scenarioId &&
    source.revision === props.project.revision &&
    source.libraryEpoch === props.home.state.libraryEpoch &&
    !externalDraftDirty.value
    ? { kind: "external", entityId: source.entityId }
    : null;
});
type NextSolveIntent = SolveIntent | "alternatives";
const explicitSolveIntent = ref<NextSolveIntent | null>(null);
const solveIntent = computed({
  get: (): NextSolveIntent =>
    explicitSolveIntent.value ?? (manualSource.value?.currentManualId ? "repair" : "optimize"),
  set: (intent: NextSolveIntent) => {
    explicitSolveIntent.value = intent;
  },
});
const sourceAdmissionFailure = ref<string | null>(null);
const manualCommitReceipt = shallowRef<{
  commandId: string;
  revision: number;
  alarm: string | null;
} | null>(null);
function receiveManualSource(source: ManualEditSourceV1 | null): void {
  manualSource.value = source;
  if (source !== null) {
    sourceAdmissionFailure.value = null;
  }
}
function receiveManualCommit(receipt: {
  commandId: string;
  revision: number;
  alarm: string | null;
}): void {
  manualCommitReceipt.value = receipt;
  sourceAdmissionFailure.value = receipt.alarm;
  solveIntent.value = "repair";
}
const selectingSolutionId = ref<string | null>(null);
const selectionError = ref<string | null>(null);
const selectionAnnouncement = ref<string | null>(null);
type ComparisonPair = {
  scenarioId: UuidV7;
  visibleRevision: Revision;
  libraryEpoch: Revision;
  owner: ProjectHomeController;
  base: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision" | "runId">;
  candidate: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision" | "runId">;
  recorded?: boolean;
  alternatives?: boolean;
  baseLabel?: string;
  candidateLabel?: string;
};
const recordedRepairPair = shallowRef<ComparisonPair | null>(null);
const requestedRepairPair = shallowRef<ComparisonPair | null>(null);
const requestedAlternativesPair = shallowRef<ComparisonPair | null>(null);
function alternativesContextCurrent(
  input: Pick<ComparisonPair, "owner" | "libraryEpoch" | "scenarioId" | "visibleRevision">,
): boolean {
  return (
    current &&
    props.home === input.owner &&
    props.home.state.libraryEpoch === input.libraryEpoch &&
    props.project.scenarioId === input.scenarioId &&
    props.project.revision === input.visibleRevision
  );
}
function clearAlternativesPair(): void {
  if (comparison.value?.pair.alternatives || comparisonPairInFlight?.alternatives)
    clearComparison();
  requestedAlternativesPair.value = null;
}
function compareAlternatives(input: ComparisonPair, trigger: HTMLButtonElement): void {
  if (
    !alternativesContextCurrent(input) ||
    sourceAdmissionFailure.value !== null ||
    comparing.value ||
    resultsRefreshing.value ||
    selectingSolutionId.value !== null
  )
    return;
  clearComparison();
  requestedAlternativesPair.value = { ...input, alternatives: true };
  comparisonTrigger.value = trigger;
  void compareLatest(requestedAlternativesPair.value);
}
function viewAlternative(
  input: Pick<ComparisonPair, "owner" | "libraryEpoch" | "scenarioId" | "visibleRevision"> & {
    result: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision" | "runId">;
  },
  trigger: HTMLButtonElement,
): void {
  if (!alternativesContextCurrent(input) || sourceAdmissionFailure.value !== null) return;
  void selectAcceptedResult(input.result, trigger, true);
}
function receiveRecordedBaseline(
  input: { candidate: SolutionSummaryDtoV1; original: RunBaselineOriginal } | null,
): void {
  const previous = recordedRepairPair.value;
  if (
    input !== null &&
    previous !== null &&
    previous.visibleRevision === props.project.revision &&
    previous.libraryEpoch === props.home.state.libraryEpoch &&
    previous.owner === props.home &&
    sameAcceptedResult(previous.candidate, input.candidate) &&
    sameAcceptedRef(previous.base.result, input.original.acceptedResult) &&
    previous.base.scenarioRevision === input.original.sourceRevision &&
    previous.base.runId === input.original.sourceRunId
  )
    return;
  clearRepairFacts();
  recordedRepairPair.value =
    input === null || input.original.sourceScenarioId !== props.project.scenarioId
      ? null
      : {
          scenarioId: props.project.scenarioId,
          visibleRevision: props.project.revision,
          libraryEpoch: props.home.state.libraryEpoch,
          owner: props.home,
          recorded: true,
          candidate: input.candidate,
          base: {
            result: input.original.acceptedResult,
            scenarioRevision: input.original.sourceRevision,
            runId: input.original.sourceRunId,
          },
        };
}
function stillShowingRepairPair(pair: ComparisonPair): boolean {
  if (pair.alternatives) {
    const shown = requestedAlternativesPair.value;
    return (
      alternativesContextCurrent(pair) &&
      shown !== null &&
      sameAcceptedResult(shown.base, pair.base) &&
      sameAcceptedResult(shown.candidate, pair.candidate)
    );
  }
  if (!pair.recorded) return stillShowingPair(pair);
  const shown = recordedRepairPair.value;
  return (
    current &&
    props.home === pair.owner &&
    props.home.state.libraryEpoch === pair.libraryEpoch &&
    props.project.scenarioId === pair.scenarioId &&
    props.project.revision === pair.visibleRevision &&
    shown !== null &&
    sameAcceptedResult(shown.base, pair.base) &&
    sameAcceptedResult(shown.candidate, pair.candidate) &&
    chosenRepairCandidate.value !== null &&
    sameAcceptedResult(chosenRepairCandidate.value, pair.candidate)
  );
}
const comparison = shallowRef<{ pair: ComparisonPair; value: SolutionComparisonV2 } | null>(null);
const comparing = ref(false);
const comparisonCancellationRequested = ref(false);
const comparisonAnnouncement = ref<string | null>(null);
const comparisonError = ref<string | null>(null);
const comparisonTrigger = ref<HTMLButtonElement | null>(null);
const comparisonCancelTrigger = ref<HTMLButtonElement | null>(null);
let comparisonScope: SetupOperationScope | null = null;
let comparisonOperation: SetupOperation<SolutionExplanationDtoV2> | null = null;
let comparisonPairInFlight: ComparisonPair | null = null;
type RepairFacts = {
  pair: ComparisonPair;
  result: ExplanationResultV2;
  sourceRevisions: readonly Revision[];
};
const repairFacts = shallowRef<RepairFacts | null>(null);
const repairFactsOpen = ref(false);
const repairFactsState = ref<ExplanationUiState>("empty");
const repairFactsStatus = ref<string | null>(null);
const repairFactsCancellationRequested = ref(false);
const repairFactsTrigger = ref<HTMLButtonElement | null>(null);
let repairFactsEpoch = 0;
let repairFactsScope: SetupOperationScope | null = null;
const repairFactMessageTexts: Readonly<Record<string, string>> = {};
let repairFactsOperation: SetupOperation<SolutionExplanationDtoV2> | null = null;
const recordedRepairFactsTrigger = ref<HTMLButtonElement | null>(null);
type BaseReview = {
  scenarioId: string;
  revision: Revision;
  libraryEpoch: Revision;
  result: SolutionSummaryDtoV1;
};
const baseReview = shallowRef<BaseReview | null>(null);
const basePending = ref(false);
const baseRedoRequired = ref(false);
const baseError = ref<string | null>(null);
function reviewCurrent(review: BaseReview): boolean {
  return (
    current &&
    props.project.scenarioId === review.scenarioId &&
    props.project.revision === review.revision &&
    props.home.state.libraryEpoch === review.libraryEpoch &&
    snapshot.value?.results.solutions.some((item) => sameAcceptedResult(item, review.result)) ===
      true
  );
}
function clearBaseReview(): void {
  baseReview.value = null;
  baseRedoRequired.value = false;
  baseError.value = null;
}
let comparisonEpoch = 0;
const latestPair = computed(() => {
  const [candidate, base] = snapshot.value?.results.solutions ?? [];
  return candidate && base
    ? {
        scenarioId: props.project.scenarioId,
        visibleRevision: props.project.revision,
        libraryEpoch: props.home.state.libraryEpoch,
        owner: props.home,
        base,
        candidate,
      }
    : null;
});
const latestFactsEligibility = ref<"checking" | "ordinary" | "recorded" | "unavailable">(
  "unavailable",
);
let latestFactsEpoch = 0;
async function readLatestFactsEligibility(): Promise<void> {
  const pair = latestPair.value,
    token = ++latestFactsEpoch;
  latestFactsEligibility.value = pair === null ? "unavailable" : "checking";
  if (pair === null) return;
  try {
    const { result } = await getSolutionView({
      scenarioId: pair.scenarioId,
      solutionId: pair.candidate.result.solutionId,
      viewId: WORKFORCE_RESULT_SCHEDULE_VIEW_ID,
      parameters: { kind: "repairDiff", limit: 1 },
    });
    if (token !== latestFactsEpoch || !stillShowingPair(pair)) return;
    if (
      result.scenarioId !== pair.scenarioId ||
      result.scenarioRevision !== pair.candidate.scenarioRevision ||
      !sameAcceptedRef(result.result, pair.candidate.result) ||
      !isWorkforceResultScheduleViewData(result.view.data) ||
      result.view.data.result.kind !== "repairDiff"
    ) {
      latestFactsEligibility.value = "unavailable";
      return;
    }
    latestFactsEligibility.value =
      result.view.data.result.availability === "absent" && result.view.data.result.baseline === null
        ? "ordinary"
        : "recorded";
  } catch {
    if (token === latestFactsEpoch && stillShowingPair(pair))
      latestFactsEligibility.value = "unavailable";
  }
}
function sameAcceptedResult(
  a: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision" | "runId">,
  b: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision" | "runId">,
): boolean {
  return (
    a.result.solutionId === b.result.solutionId &&
    a.result.resultChecksum === b.result.resultChecksum &&
    a.runId === b.runId &&
    a.scenarioRevision === b.scenarioRevision
  );
}
function sameAcceptedRef(a: AcceptedResultRefV1, b: AcceptedResultRefV1): boolean {
  return a.solutionId === b.solutionId && a.resultChecksum === b.resultChecksum;
}
function stillShowingPair(pair: ComparisonPair): boolean {
  const shown = latestPair.value;
  return (
    current &&
    props.home === pair.owner &&
    props.home.state.libraryEpoch === pair.libraryEpoch &&
    props.project.scenarioId === pair.scenarioId &&
    props.project.revision === pair.visibleRevision &&
    shown !== null &&
    shown.scenarioId === pair.scenarioId &&
    shown.visibleRevision === pair.visibleRevision &&
    shown.libraryEpoch === pair.libraryEpoch &&
    sameAcceptedResult(shown.base, pair.base) &&
    sameAcceptedResult(shown.candidate, pair.candidate)
  );
}
function clearComparison(): void {
  const restoreFocus =
    current &&
    typeof document !== "undefined" &&
    comparisonCancelTrigger.value !== null &&
    document.activeElement === comparisonCancelTrigger.value;
  comparisonEpoch += 1;
  comparisonScope?.dispose();
  comparisonScope = null;
  comparisonOperation = null;
  comparisonPairInFlight = null;
  comparing.value = false;
  comparisonCancellationRequested.value = false;
  comparisonAnnouncement.value = null;
  comparison.value = null;
  comparisonError.value = null;
  if (restoreFocus)
    void nextTick(() => {
      if (current && comparisonTrigger.value?.isConnected) comparisonTrigger.value.focus();
    });
}
function clearRepairFacts(): void {
  repairFactsEpoch += 1;
  repairFactsScope?.dispose();
  repairFactsScope = null;
  repairFactsOperation = null;
  repairFacts.value = null;
  repairFactsOpen.value = false;
  repairFactsState.value = "empty";
  repairFactsStatus.value = null;
  repairFactsCancellationRequested.value = false;
}
let generation = 0;
let resultSelectionEpoch = 0;
let resultsRefreshEpoch = 0;
let current = true;
let scope: SetupOperationScope | null = null;
const supported = computed(() => props.project.domainPackId === "official.workforce");
const groups: readonly { id: "people" | "work"; kinds: readonly WorkforceSetupEntityKind[] }[] = [
  { id: "people", kinds: ["person", "qualification", "team", "availability"] },
  {
    id: "work",
    kinds: [
      "location",
      "workloadBucket",
      "calendar",
      "assignmentType",
      "shiftTemplate",
      "shiftInstance",
      "coverageRequirement",
      "baseSchedule",
      "scorePolicy",
    ],
  },
];

function invalidateSnapshot(): void {
  generation += 1;
  scope?.dispose();
  scope = null;
  clearAlternativesPair();
  clearComparison();
  clearRepairFacts();
  snapshot.value = null;
  inspectedSolutionId.value = null;
  loading.value = false;
  selectionError.value = null;
  selectionAnnouncement.value = null;
  selectingSolutionId.value = null;
  error.value = messages.setup.stale;
  clearRuleNavigation();
}
const horizonFormatter = computed(() => {
  if (snapshot.value === null) return null;
  try {
    return new Intl.DateTimeFormat(props.locale, {
      dateStyle: "medium",
      timeStyle: "short",
      timeZone: snapshot.value.facts.settings.timeZone,
    });
  } catch {
    return null;
  }
});
function horizon(value: string): string {
  const instant = new Date(value);
  return horizonFormatter.value !== null && !Number.isNaN(instant.getTime())
    ? horizonFormatter.value.format(instant)
    : value;
}
async function readCapabilities(captured: number): Promise<void> {
  capabilitiesLoading.value = true;
  capabilities.value = null;
  capabilitiesError.value = null;
  try {
    const response = await getAppCapabilities();
    if (captured !== generation || !current) return;
    capabilities.value = response.result;
  } catch (failure) {
    if (captured === generation && current) capabilitiesError.value = safeMessage(failure);
  } finally {
    if (captured === generation && current) capabilitiesLoading.value = false;
  }
}
async function load(): Promise<void> {
  const captured = ++generation;
  const home = props.home;
  const libraryEpoch = home.state.libraryEpoch;
  clearAlternativesPair();
  const keepSnapshot = snapshotMatchesProject.value;
  clearBaseReview();
  clearComparison();
  clearRepairFacts();
  clearRuleNavigation();
  resultsRefreshEpoch += 1;
  resultsRefreshing.value = false;
  selectingSolutionId.value = null;
  selectionError.value = null;
  const selectionAtStart = resultSelectionEpoch;
  const resultsAtStart = resultsRefreshEpoch;
  selectionAnnouncement.value = null;
  scope?.dispose();
  scope = null;
  if (!keepSnapshot) {
    snapshot.value = null;
    inspectedSolutionId.value = null;
  }
  error.value = null;
  capabilities.value = null;
  capabilitiesError.value = null;
  capabilitiesLoading.value = false;
  if (!supported.value || !current) {
    loading.value = false;
    return;
  }
  loading.value = true;
  void readCapabilities(captured);
  const { scenarioId, revision } = props.project;
  let owned: SetupOperationScope | null = null;
  try {
    owned = new SetupOperationScope(scenarioId, revision);
    scope = owned;
    const [summary, overview, results] = await Promise.all([
      getScenarioSummary(owned).result,
      getScenarioView(owned, {
        source: { kind: "stored" },
        query: { schemaVersion: 1, viewId: "official.workforce.setup.overview", parameters: {} },
      }).result,
      listSolutions(scenarioId),
    ]);
    if (captured !== generation || props.home !== home || home.state.libraryEpoch !== libraryEpoch)
      return;
    if (
      summary.result.scenarioId !== scenarioId ||
      overview.result.scenarioId !== scenarioId ||
      results.result.scenarioId !== scenarioId ||
      summary.result.revision !== revision ||
      overview.result.revision !== revision ||
      results.result.currentRevision !== revision ||
      props.project.scenarioId !== scenarioId ||
      props.project.revision !== revision
    ) {
      invalidateSnapshot();
      return;
    }
    const currentResults = snapshot.value?.results;
    snapshot.value = {
      owner: home,
      libraryEpoch,
      facts: overview.result.view.data.result.data,
      summary: summary.result,
      results:
        currentResults !== undefined &&
        (selectionAtStart !== resultSelectionEpoch || resultsAtStart !== resultsRefreshEpoch)
          ? currentResults
          : results.result,
    };
    externalInitialDates.value ??= snapshot.value.facts.initialWorkWindow;
  } catch (failure) {
    if (captured === generation) {
      if (isRevisionConflict(failure)) {
        invalidateSnapshot();
      } else {
        error.value = safeMessage(failure);
      }
    }
  } finally {
    owned?.dispose();
    if (captured === generation) {
      scope = null;
      loading.value = false;
    }
  }
}
watch(
  [
    () => props.home,
    () => props.project.scenarioId,
    () => props.project.revision,
    () => props.home.state.libraryEpoch,
  ],
  () => void load(),
  { immediate: true, flush: "sync" },
);
watch(latestPair, () => void readLatestFactsEligibility(), { immediate: true });
watch(
  [() => props.home, () => props.project.scenarioId, () => props.home.state.libraryEpoch],
  () => {
    chosenRepairCandidate.value = null;
    recordedRepairPair.value = null;
    manualSource.value = null;
    externalSource.value = null;
    sourceAdmissionFailure.value = null;
    manualCommitReceipt.value = null;
    explicitSolveIntent.value = null;
  },
  { flush: "sync" },
);
watch(
  [() => props.home, () => props.project.scenarioId],
  () => {
    baselineMode.value = "accepted";
    externalDraftDirty.value = false;
    externalSource.value = null;
    externalInitialDates.value = null;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  current = false;
  latestFactsEpoch += 1;
  clearComparison();
  clearRepairFacts();
  clearRuleNavigation();
  generation += 1;
  scope?.dispose();
});
async function refresh(): Promise<void> {
  const epoch = props.home.state.libraryEpoch;
  if ((await props.home.refreshLibrary()) && current && props.home.state.libraryEpoch === epoch)
    await load();
}
async function retryCapabilities(): Promise<void> {
  if (!current || !supported.value || snapshot.value === null || capabilitiesLoading.value) return;
  await readCapabilities(generation);
}
async function refreshResults(): Promise<void> {
  const captured = generation;
  clearComparison();
  clearRepairFacts();
  clearBaseReview();
  const refreshAtStart = ++resultsRefreshEpoch;
  const selectionAtStart = resultSelectionEpoch;
  resultsRefreshing.value = true;
  const { scenarioId, revision } = props.project;
  try {
    const response = await listSolutions(scenarioId);
    if (
      !current ||
      captured !== generation ||
      refreshAtStart !== resultsRefreshEpoch ||
      selectionAtStart !== resultSelectionEpoch ||
      props.project.scenarioId !== scenarioId ||
      props.project.revision !== revision
    )
      return;
    if (response.result.scenarioId !== scenarioId || response.result.currentRevision !== revision) {
      invalidateSnapshot();
      return;
    }
    selectionError.value = null;
    if (snapshot.value !== null) snapshot.value = { ...snapshot.value, results: response.result };
  } catch (failure) {
    if (
      current &&
      captured === generation &&
      refreshAtStart === resultsRefreshEpoch &&
      selectionAtStart === resultSelectionEpoch
    ) {
      if (isRevisionConflict(failure)) invalidateSnapshot();
      else if (selectionError.value !== null) selectionError.value = safeMessage(failure);
      else error.value = safeMessage(failure);
    }
  } finally {
    if (refreshAtStart === resultsRefreshEpoch) resultsRefreshing.value = false;
  }
}
async function compareLatest(requested: ComparisonPair | null = latestPair.value): Promise<void> {
  const pair = requested;
  if (
    pair === null ||
    !stillShowingRepairPair(pair) ||
    comparing.value ||
    resultsRefreshing.value ||
    selectingSolutionId.value
  )
    return;
  const captured = ++comparisonEpoch;
  const atGeneration = generation;
  comparison.value = null;
  comparisonError.value = null;
  comparisonAnnouncement.value = null;
  comparing.value = true;
  comparisonPairInFlight = pair;

  let owned: SetupOperationScope | null = null;
  let request: SetupOperation<SolutionExplanationDtoV2> | null = null;
  try {
    owned = new SetupOperationScope(pair.scenarioId, pair.candidate.scenarioRevision);
    comparisonScope = owned;
    request = explainSolution(owned, {
      schemaVersion: 1,
      subject: {
        kind: "solutionDifference",
        left: pair.base.result,
        right: pair.candidate.result,
      },
    });
    comparisonOperation = request;
    const active = () =>
      captured === comparisonEpoch &&
      atGeneration === generation &&
      comparisonScope === owned &&
      comparisonOperation === request &&
      owned?.context.expectedRevision === pair.candidate.scenarioRevision &&
      request?.isCurrent() === true &&
      stillShowingRepairPair(pair);

    const response = await request.result;
    if (!active()) return;
    const result = response.result;
    const evidence = result.explanation.evidence;
    const payload = evidence.evidence;
    const value = payload.kind === "solutionDifference" ? payload.comparison : null;
    if (
      value === null ||
      response.currentRevision !== pair.visibleRevision ||
      result.scenarioId !== pair.scenarioId ||
      result.currentRevision !== pair.visibleRevision ||
      result.scenarioRevisions.length !== 2 ||
      result.scenarioRevisions[0] !== pair.base.scenarioRevision ||
      result.scenarioRevisions[1] !== pair.candidate.scenarioRevision ||
      value.base.scenarioId !== pair.scenarioId ||
      value.base.scenarioRevision !== pair.base.scenarioRevision ||
      !sameAcceptedRef(value.base.acceptedResult, pair.base.result) ||
      value.candidate.scenarioId !== pair.scenarioId ||
      value.candidate.scenarioRevision !== pair.candidate.scenarioRevision ||
      !sameAcceptedRef(value.candidate.acceptedResult, pair.candidate.result) ||
      value.runs.base.runId !== pair.base.runId ||
      value.runs.candidate.runId !== pair.candidate.runId
    ) {
      comparisonError.value = messages.setup.comparisonChanged;
      return;
    }
    comparison.value = { pair, value };
    if (comparisonCancellationRequested.value)
      comparisonAnnouncement.value =
        "The comparison completed before cancellation took effect; no scenario changes were made.";
  } catch (failure) {
    if (
      captured === comparisonEpoch &&
      atGeneration === generation &&
      stillShowingRepairPair(pair)
    ) {
      if (isOperationCancelled(failure)) {
        comparisonAnnouncement.value = messages.setup.comparisonCancelled;
      } else {
        comparisonError.value = isRevisionConflict(failure)
          ? messages.setup.stale
          : safeMessage(failure);
      }
    }
  } finally {
    const restoreFocus =
      captured === comparisonEpoch &&
      current &&
      typeof document !== "undefined" &&
      comparisonCancelTrigger.value !== null &&
      document.activeElement === comparisonCancelTrigger.value;
    if (request !== null) {
      void request.release().catch(() => undefined);
      if (comparisonOperation === request) comparisonOperation = null;
    }
    owned?.dispose();
    if (comparisonScope === owned) comparisonScope = null;
    if (captured === comparisonEpoch) {
      comparisonPairInFlight = null;
      comparing.value = false;
      comparisonCancellationRequested.value = false;
      await nextTick();
      if (restoreFocus && stillShowingRepairPair(pair) && comparisonTrigger.value?.isConnected)
        comparisonTrigger.value.focus();
    }
  }
}

async function cancelLatestComparison(): Promise<void> {
  const request = comparisonOperation;
  if (request === null || comparisonCancellationRequested.value) return;
  comparisonCancellationRequested.value = true;
  comparisonAnnouncement.value = messages.setup.comparisonCancelling;
  try {
    await request.cancel();
  } catch (failure) {
    if (comparisonOperation === request && request.isCurrent()) {
      comparisonCancellationRequested.value = false;
      comparisonAnnouncement.value = null;
      comparisonError.value = safeMessage(failure);
    }
  }
}
async function explainLatestRepairFacts(
  requested: ComparisonPair | null = latestPair.value,
): Promise<void> {
  const pair = requested;
  if (
    pair === null ||
    (!pair.recorded && latestFactsEligibility.value !== "ordinary") ||
    sourceAdmissionFailure.value !== null ||
    repairFactsState.value === "loading" ||
    resultsRefreshing.value ||
    selectingSolutionId.value !== null
  )
    return;
  clearRepairFacts();
  const captured = repairFactsEpoch;
  requestedRepairPair.value = pair;
  const atGeneration = generation;
  repairFactsOpen.value = true;
  repairFactsState.value = "loading";
  let owned: SetupOperationScope | null = null;
  let request: SetupOperation<SolutionExplanationDtoV2> | null = null;
  try {
    owned = new SetupOperationScope(pair.scenarioId, pair.candidate.scenarioRevision);
    repairFactsScope = owned;
    request = explainSolution(owned, {
      schemaVersion: 1,
      subject: {
        kind: "repair",
        current: pair.candidate.result,
        base: pair.base.result,
      },
    });
    repairFactsOperation = request;
    const active = () =>
      captured === repairFactsEpoch &&
      atGeneration === generation &&
      props.home === pair.owner &&
      pair.owner.state.libraryEpoch === pair.libraryEpoch &&
      repairFactsScope === owned &&
      repairFactsOperation === request &&
      owned?.context.expectedRevision === pair.candidate.scenarioRevision &&
      request?.isCurrent() === true &&
      stillShowingRepairPair(pair);

    const response = await request.result;
    if (!active()) return;
    if (repairFactsCancellationRequested.value) {
      repairFactsState.value = "cancelled";
      repairFactsStatus.value =
        "Cancellation was requested before this response was published; no facts are shown.";
      return;
    }
    const result = response.result;
    const evidence = result.explanation.evidence;
    const payload = evidence.evidence;
    const value = payload.kind === "repair" ? payload.repair.comparison : null;
    if (
      value === null ||
      payload.kind !== "repair" ||
      result.explanation.rendered.schemaVersion !== 1 ||
      result.explanation.rendered.kind !== "repair" ||
      result.explanation.rendered.evidenceChecksum !== evidence.checksum ||
      response.currentRevision !== pair.visibleRevision ||
      result.scenarioId !== pair.scenarioId ||
      result.currentRevision !== pair.visibleRevision ||
      result.scenarioRevisions.length !== 2 ||
      result.scenarioRevisions[0] !== pair.candidate.scenarioRevision ||
      result.scenarioRevisions[1] !== pair.base.scenarioRevision ||
      value.base.scenarioId !== pair.scenarioId ||
      value.base.scenarioRevision !== pair.base.scenarioRevision ||
      !sameAcceptedRef(value.base.acceptedResult, pair.base.result) ||
      value.candidate.scenarioId !== pair.scenarioId ||
      value.candidate.scenarioRevision !== pair.candidate.scenarioRevision ||
      !sameAcceptedRef(value.candidate.acceptedResult, pair.candidate.result) ||
      value.runs.base.runId !== pair.base.runId ||
      value.runs.candidate.runId !== pair.candidate.runId ||
      value.runs.base.outcome.type !== "accepted" ||
      value.runs.base.outcome.solutionId !== pair.base.result.solutionId ||
      value.runs.base.outcome.acceptedResultChecksum !== pair.base.result.resultChecksum ||
      value.runs.candidate.outcome.type !== "accepted" ||
      value.runs.candidate.outcome.solutionId !== pair.candidate.result.solutionId ||
      value.runs.candidate.outcome.acceptedResultChecksum !== pair.candidate.result.resultChecksum
    ) {
      repairFactsState.value = "stale";
      repairFactsStatus.value = messages.setup.comparisonChanged;
      return;
    }
    repairFacts.value = {
      pair,
      result: result.explanation,
      sourceRevisions: result.scenarioRevisions,
    };
    repairFactsState.value = "ready";
  } catch (failure) {
    if (
      captured === repairFactsEpoch &&
      atGeneration === generation &&
      stillShowingRepairPair(pair)
    ) {
      if (isOperationCancelled(failure)) {
        repairFactsState.value = "cancelled";
        repairFactsStatus.value = null;
      } else if (isRevisionConflict(failure)) {
        repairFactsState.value = "stale";
        repairFactsStatus.value = messages.setup.stale;
      } else {
        repairFactsState.value = "unavailable";
        repairFactsStatus.value = safeMessage(failure);
      }
    }
  } finally {
    if (
      captured === repairFactsEpoch &&
      atGeneration === generation &&
      repairFactsState.value === "loading" &&
      stillShowingRepairPair(pair)
    ) {
      repairFactsState.value = "stale";
      repairFactsStatus.value = messages.setup.comparisonChanged;
    }
    if (request !== null) {
      void request.release().catch(() => undefined);
      if (repairFactsOperation === request) repairFactsOperation = null;
    }
    owned?.dispose();
    if (repairFactsScope === owned) repairFactsScope = null;
  }
}

async function cancelLatestRepairFacts(): Promise<void> {
  const request = repairFactsOperation;
  if (request === null || repairFactsCancellationRequested.value) return;
  repairFactsCancellationRequested.value = true;
  repairFactsStatus.value = null;
  try {
    await request.cancel();
  } catch (failure) {
    if (repairFactsOperation === request && request.isCurrent()) {
      repairFactsCancellationRequested.value = false;
      repairFactsStatus.value = safeMessage(failure);
    }
  }
}

function setRepairFactsOpen(open: boolean): void {
  const pair = requestedRepairPair.value;
  const captured = repairFactsEpoch;
  repairFactsOpen.value = open;
  if (open) return;
  if (repairFactsState.value === "loading") void cancelLatestRepairFacts();
  void nextTick(() => {
    if (
      captured === repairFactsEpoch &&
      pair !== null &&
      stillShowingRepairPair(pair) &&
      (pair.recorded ? recordedRepairFactsTrigger.value : repairFactsTrigger.value)?.isConnected
    )
      (pair.recorded ? recordedRepairFactsTrigger.value : repairFactsTrigger.value)?.focus();
  });
}

function prepareBaseReview(result: SolutionSummaryDtoV1): void {
  if (
    !current ||
    basePending.value ||
    props.home.state.busyAction !== null ||
    props.home.state.mutation?.outcome === "outcomeUnknown" ||
    snapshot.value?.results.solutions.some((item) => sameAcceptedResult(item, result)) !== true
  )
    return;
  clearBaseReview();
  baseReview.value = {
    scenarioId: props.project.scenarioId,
    revision: props.project.revision,
    libraryEpoch: props.home.state.libraryEpoch,
    result,
  };
}
async function adoptReviewedBase(truncateRedo: boolean): Promise<void> {
  const review = baseReview.value;
  if (
    review === null ||
    !reviewCurrent(review) ||
    basePending.value ||
    props.home.state.busyAction !== null ||
    props.home.state.mutation?.outcome === "outcomeUnknown" ||
    truncateRedo !== baseRedoRequired.value
  )
    return;
  basePending.value = true;
  baseError.value = null;
  const commandId = newUuidV7();
  try {
    await props.home.runOperation({
      action: "adopt-accepted-base",
      label: messages.setup.baseCapture.saving,
      mutation: {
        scenarioId: review.scenarioId,
        expectedRevision: review.revision,
        commandId,
        receipt: (result) => ({
          kind: result.newRevision === review.revision ? "noChanges" : "applied",
          revision: result.newRevision,
        }),
      },
      execute: () =>
        adoptBaseSolution({
          commandId,
          scenarioId: review.scenarioId,
          expectedRevision: review.revision,
          result: review.result.result,
          actor: { actorId: null, displayName: messages.people.localActor },
          truncateRedo,
        }),
      success: () => messages.setup.baseCapture.saved,
    });
    if (current) clearBaseReview();
  } catch (failure) {
    if (!current) return;
    baseError.value = isRevisionConflict(failure) ? messages.setup.stale : safeMessage(failure);
    if (reviewCurrent(review) && isRedoBranchTruncation(failure)) {
      baseRedoRequired.value = true;
    } else {
      baseReview.value = null;
    }
  } finally {
    if (current) basePending.value = false;
  }
}

async function selectAcceptedResult(
  result: Pick<SolutionSummaryDtoV1, "result" | "scenarioRevision" | "runId">,
  trigger: HTMLButtonElement,
  retainedAlternative = false,
): Promise<void> {
  if (
    !current ||
    snapshot.value === null ||
    selectingSolutionId.value !== null ||
    selectionError.value !== null
  )
    return;
  const captured = generation;
  const { scenarioId, revision } = props.project;
  const home = props.home;
  const libraryEpoch = home.state.libraryEpoch;
  const { solutionId, resultChecksum } = result.result;
  const stillCurrent = () =>
    current &&
    captured === generation &&
    props.home === home &&
    home.state.libraryEpoch === libraryEpoch &&
    props.project.scenarioId === scenarioId &&
    props.project.revision === revision;
  const inspectButton = trigger.parentElement?.querySelector("[data-result-inspect]");
  const restoreFocus = document.activeElement === trigger;
  // A rejected or interrupted selection may have committed; only a fresh native list clears it.
  resultSelectionEpoch += 1;
  selectionAnnouncement.value = null;
  selectingSolutionId.value = solutionId;
  try {
    const selected = (
      await selectSolution({
        scenarioId,
        expectedRevision: revision,
        solutionId,
      })
    ).result;
    if (!stillCurrent()) return;
    if (
      selected.scenarioId !== scenarioId ||
      selected.currentRevision !== revision ||
      selected.scenarioRevision !== result.scenarioRevision ||
      selected.runId !== result.runId ||
      selected.result.solutionId !== solutionId ||
      selected.result.resultChecksum !== resultChecksum ||
      !selected.selected ||
      (!retainedAlternative &&
        snapshot.value.results.solutions.find((item) => item.result.solutionId === solutionId)
          ?.result.resultChecksum !== resultChecksum)
    ) {
      selectionError.value = messages.setup.selectionChanged;
      return;
    }
    const refreshInFlight = resultsRefreshing.value;
    resultSelectionEpoch += 1;
    const loaded = snapshot.value;
    snapshot.value = {
      ...loaded,
      results: {
        ...loaded.results,
        solutions: [
          ...(loaded.results.solutions.some((item) => item.result.solutionId === solutionId)
            ? []
            : [selected]),
          ...loaded.results.solutions.map((item) =>
            item.result.solutionId === solutionId ? selected : { ...item, selected: false },
          ),
        ],
      },
    };
    selectionAnnouncement.value = messages.setup.selectionSucceeded(solutionId);
    if (retainedAlternative) {
      inspectedSolutionId.value = solutionId;
      chosenRepairCandidate.value = selected;
    }
    await nextTick();
    if (
      stillCurrent() &&
      restoreFocus &&
      document.activeElement === document.body &&
      inspectButton instanceof HTMLButtonElement &&
      inspectButton.isConnected
    )
      inspectButton.focus();
    if (refreshInFlight && stillCurrent()) void refreshResults();
  } catch (failure) {
    if (stillCurrent())
      selectionError.value = isRevisionConflict(failure)
        ? messages.setup.stale
        : safeMessage(failure);
  } finally {
    if (stillCurrent()) selectingSolutionId.value = null;
  }
}
</script>

<template>
  <section class="page-stack" aria-labelledby="setup-heading">
    <h2 id="setup-heading" data-route-heading tabindex="-1">
      {{ supported ? messages.setup.heading : messages.setup.unsupportedTitle }}
    </h2>
    <div v-if="!supported" class="state-panel">
      <p>{{ messages.setup.unsupported }}</p>
    </div>
    <template v-else>
      <p>{{ messages.setup.description }}</p>
      <div
        v-if="loading && !snapshotMatchesProject"
        class="state-panel"
        role="status"
        aria-busy="true"
      >
        {{ messages.setup.loading }}
      </div>
      <div v-else-if="error && !snapshotMatchesProject" class="state-panel" role="alert">
        <p>{{ error }}</p>
        <button type="button" :disabled="home.state.busyAction !== null" @click="refresh">
          {{ messages.setup.refresh }}
        </button>
      </div>
      <template v-else-if="snapshot && snapshotMatchesProject">
        <div v-if="loading" class="state-panel" role="status" aria-busy="true">
          {{ messages.setup.loading }}
        </div>
        <div v-if="error" class="state-panel" role="alert">
          <p>{{ error }}</p>
          <button type="button" :disabled="home.state.busyAction !== null" @click="refresh">
            {{ messages.setup.refresh }}
          </button>
        </div>
        <h3 id="setup-guide">{{ messages.setup.guide }}</h3>
        <ol class="setup-steps" aria-labelledby="setup-guide">
          <li class="state-panel setup-step" aria-labelledby="setup-people">
            <h4 id="setup-people" tabindex="-1">1. {{ messages.setup.people }}</h4>
            <p>{{ messages.setup.peopleStep }}</p>
            <RouterLink
              :to="{ name: 'project-people', params: { scenarioId: project.scenarioId } }"
            >
              {{ messages.people.heading }}
            </RouterLink>
            <div class="action-row">
              <RouterLink
                :to="{ name: 'project-eligibility', params: { scenarioId: project.scenarioId } }"
              >
                {{ messages.setup.eligibility }}
              </RouterLink>
              <RouterLink
                :to="{ name: 'project-availability', params: { scenarioId: project.scenarioId } }"
              >
                {{ messages.setup.availability }}
              </RouterLink>
            </div>
          </li>
          <li class="state-panel setup-step" aria-labelledby="setup-work">
            <h4 id="setup-work" tabindex="-1">2. {{ messages.setup.work }}</h4>
            <p>{{ messages.setup.workStep }}</p>
            <RouterLink :to="{ name: 'project-work', params: { scenarioId: project.scenarioId } }">
              {{ messages.work.heading }}
            </RouterLink>
            <div class="action-row">
              <RouterLink
                :to="{ name: 'project-score-policy', params: { scenarioId: project.scenarioId } }"
              >
                {{ messages.setup.entityKinds.scorePolicy }}
              </RouterLink>
            </div>
          </li>
          <li class="state-panel setup-step" aria-labelledby="setup-rules">
            <h4 id="setup-rules" tabindex="-1">3. {{ messages.setup.rules }}</h4>
            <p>{{ messages.setup.rulesStep }}</p>
            <RouterLink :to="{ name: 'project-rules', params: { scenarioId: project.scenarioId } }">
              {{ messages.setup.rules }}
            </RouterLink>
          </li>
          <li class="state-panel setup-step" aria-labelledby="setup-validation">
            <h4 id="setup-validation" tabindex="-1">4. {{ messages.setup.validation }}</h4>
            <p>{{ messages.setup.validationStep }}</p>
            <p>
              {{ messages.setup.full }}:
              {{ messages.setup.validationStates[snapshot.summary.full.state] }}
            </p>
            <p v-if="snapshot.summary.full.state !== 'notRun' && snapshot.summary.full.stale">
              {{ messages.setup.staleValidation }}
            </p>
            <p v-if="snapshot.summary.fast.counts.errors" role="alert">
              {{ messages.setup.fast }}:
              {{ formatNumber(snapshot.summary.fast.counts.errors, locale) }}
              {{ messages.setup.errors }}
            </p>
            <p
              v-if="
                snapshot.summary.full.state === 'completed' && snapshot.summary.full.counts.errors
              "
              role="alert"
            >
              {{ messages.setup.full }}:
              {{ formatNumber(snapshot.summary.full.counts.errors, locale) }}
              {{ messages.setup.errors }}
            </p>
            <RouterLink
              :to="{ name: 'project-validation', params: { scenarioId: project.scenarioId } }"
            >
              {{ messages.validationWorkspace.heading }}
            </RouterLink>
          </li>
        </ol>
        <section class="state-panel setup-step" aria-labelledby="setup-calendar">
          <h3 id="setup-calendar" tabindex="-1">{{ messages.setup.calendarSummary }}</h3>
          <p>{{ snapshot.facts.settings.timeZone }}</p>
          <p>
            {{ horizon(snapshot.facts.settings.horizon.start) }} –
            {{ horizon(snapshot.facts.settings.horizon.end) }}
          </p>
          <p v-if="horizonFormatter === null" role="status">{{ messages.setup.zoneUnavailable }}</p>
          <RouterLink :to="{ name: 'project-work', params: { scenarioId: project.scenarioId } }">
            {{ messages.setup.calendar }}
          </RouterLink>
        </section>
        <details id="setup-calendar-details" class="state-panel setup-more">
          <summary>{{ messages.setup.calendarDetails }}</summary>
          <dl class="metadata-list">
            <div>
              <dt>{{ messages.projects.timeZone }}</dt>
              <dd>{{ snapshot.facts.settings.timeZone }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.horizonStart }}</dt>
              <dd>{{ horizon(snapshot.facts.settings.horizon.start) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.horizonEnd }}</dt>
              <dd>{{ horizon(snapshot.facts.settings.horizon.end) }}</dd>
            </div>
            <div>
              <dt>{{ messages.projects.locale }}</dt>
              <dd>{{ snapshot.facts.settings.locale }}</dd>
            </div>
            <div>
              <dt>{{ messages.projects.displayUnits }}</dt>
              <dd>
                {{
                  snapshot.facts.settings.units === "metric"
                    ? messages.projects.metric
                    : messages.projects.usCustomary
                }}
              </dd>
            </div>
            <div>
              <dt>{{ messages.projects.missingClockTime }}</dt>
              <dd>
                {{
                  snapshot.facts.settings.gapPolicy === "reject"
                    ? messages.projects.reject
                    : snapshot.facts.settings.gapPolicy === "moveForward"
                      ? messages.projects.moveForward
                      : messages.projects.packPolicy
                }}
              </dd>
            </div>
            <div>
              <dt>{{ messages.projects.repeatedClockTime }}</dt>
              <dd>
                {{
                  snapshot.facts.settings.overlapPolicy === "reject"
                    ? messages.projects.reject
                    : snapshot.facts.settings.overlapPolicy === "earlier"
                      ? messages.projects.earlier
                      : messages.projects.later
                }}
              </dd>
            </div>
            <template v-for="group in groups" :key="group.id">
              <div
                v-for="count in snapshot.facts.entities.filter((item) =>
                  group.kinds.includes(item.kind),
                )"
                :key="count.kind"
              >
                <dt>{{ messages.setup.entityKinds[count.kind] }}</dt>
                <dd>{{ formatNumber(count.count, locale) }}</dd>
              </div>
            </template>
            <div>
              <dt>{{ messages.setup.required }}</dt>
              <dd>{{ formatNumber(snapshot.facts.requiredRules, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.activeRequired }}</dt>
              <dd>{{ formatNumber(snapshot.facts.activeRequiredRules, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.preferences }}</dt>
              <dd>{{ formatNumber(snapshot.facts.preferences, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.activePreferences }}</dt>
              <dd>{{ formatNumber(snapshot.facts.activePreferences, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.locks }}</dt>
              <dd>{{ formatNumber(snapshot.facts.lockedAssignments, locale) }}</dd>
            </div>
            <div>
              <dt>{{ messages.setup.memberships }}</dt>
              <dd>{{ formatNumber(snapshot.facts.configuredTypeMemberships, locale) }}</dd>
            </div>
          </dl>
          <p>{{ messages.setup.countsBoundary }}</p>
        </details>
        <details class="state-panel setup-more">
          <summary>{{ messages.setup.validationDetails }}</summary>
          <ValidationSummary
            :findings="snapshot.summary.fast"
            state="ready"
            interaction="static"
            presentation="embedded"
            :heading="messages.setup.fast"
            :heading-level="4"
            :locale="locale"
          />
          <h4>{{ messages.setup.full }}</h4>
          <p>{{ messages.setup.validationStates[snapshot.summary.full.state] }}</p>
          <template v-if="snapshot.summary.full.state !== 'notRun'">
            <p>
              {{
                messages.setup.validationRevision(
                  formatNumber(snapshot.summary.full.inputRevision, locale),
                )
              }}
            </p>
            <p v-if="snapshot.summary.full.stale">{{ messages.setup.staleValidation }}</p>
            <code v-if="snapshot.summary.full.state === 'failed'">{{
              snapshot.summary.full.code
            }}</code>
            <dl v-if="snapshot.summary.full.state === 'completed'" class="metadata-list">
              <div>
                <dt>{{ messages.setup.errors }}</dt>
                <dd>{{ formatNumber(snapshot.summary.full.counts.errors, locale) }}</dd>
              </div>
              <div>
                <dt>{{ messages.setup.warnings }}</dt>
                <dd>{{ formatNumber(snapshot.summary.full.counts.warnings, locale) }}</dd>
              </div>
              <div>
                <dt>{{ messages.setup.information }}</dt>
                <dd>{{ formatNumber(snapshot.summary.full.counts.information, locale) }}</dd>
              </div>
            </dl>
          </template>
        </details>
        <p v-if="manualCommitReceipt" role="status">
          Committed manual command <code>{{ manualCommitReceipt.commandId }}</code
          >; new revision {{ manualCommitReceipt.revision }}.
          {{ manualCommitReceipt.alarm ? messages.repair.alarm : messages.repair.committed }}
        </p>
      </template>
      <template v-if="externalInitialDates">
        <label>
          Starting source for Repair and Alternatives
          <select
            v-model="baselineMode"
            :disabled="
              externalDraftDirty ||
              home.state.busyAction !== null ||
              home.state.mutation?.outcome === 'outcomeUnknown'
            "
          >
            <option value="accepted">Accepted result and native manual edits</option>
            <option value="external">Imported starting plan — unverified</option>
          </select>
        </label>
        <WorkforceRepairPanel
          v-if="snapshot && snapshotMatchesProject"
          v-show="baselineMode === 'accepted'"
          :home="home"
          :project="project"
          :dates="snapshot.facts.initialWorkWindow"
          :candidate="chosenRepairCandidate"
          v-bind="locale === undefined ? {} : { locale }"
          @source="receiveManualSource"
          @committed="receiveManualCommit"
          @authority-failure="sourceAdmissionFailure = $event"
          @reoptimize="solveIntent = 'optimize'"
        />
        <WorkforceExternalPlanPanel
          :home="home"
          :project="project"
          :dates="externalInitialDates"
          :active="baselineMode === 'external'"
          @source="externalSource = $event"
          @dirty="externalDraftDirty = $event"
        />
      </template>
      <template v-if="snapshot && snapshotMatchesProject">
        <RecordedBaselineDiff
          :home="home"
          :project="project"
          :candidate="chosenRepairCandidate"
          @recorded-baseline="receiveRecordedBaseline"
        />
        <label
          >Next solve intent
          <select
            v-model="solveIntent"
            :disabled="
              home.state.busyAction !== null || home.state.mutation?.outcome === 'outcomeUnknown'
            "
          >
            <option value="repair" :disabled="selectedBaseline === null">
              Repair plan — preserve the selected saved starting plan
            </option>
            <option value="optimize">Ordinary Optimize — authored objective</option>
            <option value="alternatives">Alternatives — explore four named tradeoffs</option>
          </select>
        </label>
        <OptimizeCapabilityPanel
          v-if="solveIntent !== 'alternatives'"
          :scenario-id="project.scenarioId"
          :summary="snapshot.summary"
          :current-revision="project.revision"
          :home="home"
          :intent="solveIntent"
          :baseline-source="solveIntent === 'optimize' ? { kind: 'accepted' } : selectedBaseline"
          :authored-optimize-unavailable-reason="
            manualSource?.authoredOptimizeUnavailableReason ?? null
          "
          :authority-unavailable-reason="
            sourceAdmissionFailure ??
            (solveIntent === 'repair' && selectedBaseline === null
              ? 'Select and read a current starting plan; apply or discard pending external edits before Repair.'
              : null)
          "
          :rule-navigation-available="!project.archived && home.state.phase === 'ready'"
          :capabilities="capabilities"
          :loading="capabilitiesLoading"
          :error="capabilitiesError"
          @retry="retryCapabilities"
          @terminal="refreshResults"
          @rule-action="openConflictRule"
        />
        <WorkforceAlternativesPanel
          v-else
          :home="home"
          :project="project"
          :summary="snapshot.summary"
          :capabilities="capabilities"
          :loading="capabilitiesLoading || snapshot.libraryEpoch !== home.state.libraryEpoch"
          :error="capabilitiesError"
          :baseline-available="selectedBaseline !== null"
          :baseline-source="selectedBaseline"
          :authority-unavailable-reason="sourceAdmissionFailure"
          :choice-unavailable="
            snapshot.libraryEpoch !== home.state.libraryEpoch ||
            comparing ||
            resultsRefreshing ||
            selectingSolutionId !== null ||
            selectionError !== null
          "
          @retry="retryCapabilities"
          @terminal="refreshResults"
          @pair-changed="clearAlternativesPair"
          @compare="compareAlternatives"
          @view="viewAlternative"
        />
        <p v-if="ruleNavigationPending" role="status">Checking the native rule source…</p>
        <p v-if="ruleNavigationError" role="alert">{{ ruleNavigationError }}</p>
        <p v-if="resultsRefreshing" role="status" aria-live="polite">
          {{ messages.setup.optimize.terminalRefresh }}
        </p>
        <p v-if="snapshot.results.solutions.length">
          {{
            messages.setup.existingResults(formatNumber(snapshot.results.solutions.length, locale))
          }}
        </p>
        <p v-if="snapshot.results.solutions.some((result) => result.stale)" role="status">
          {{ messages.setup.staleResult }}
        </p>
        <details
          id="setup-results-details"
          class="state-panel setup-more"
          :open="snapshot.results.solutions.length > 0"
        >
          <summary>{{ messages.setup.resultsDetails }}</summary>
          <section aria-labelledby="setup-results">
            <h3 id="setup-results" tabindex="-1">{{ messages.setup.results }}</h3>
            <p>{{ messages.setup.acceptedBoundary }}</p>
            <p>{{ messages.setup.selectionHelp }}</p>
            <p v-if="selectionAnnouncement" role="status" aria-live="polite">
              {{ selectionAnnouncement }}
            </p>
            <p v-if="selectingSolutionId !== null" role="status">
              {{ messages.setup.selectingResult }}
            </p>
            <div v-if="selectionError" role="alert">
              <p>{{ selectionError }} {{ messages.setup.selectionRecovery }}</p>
              <button type="button" :disabled="resultsRefreshing" @click="refreshResults">
                {{ messages.setup.refreshResults }}
              </button>
              <p v-if="resultsRefreshing" role="status">{{ messages.setup.loading }}</p>
            </div>
            <p v-if="snapshot.results.solutions.length === 0">{{ messages.setup.noAccepted }}</p>
            <button type="button" :disabled="resultsRefreshing" @click="refreshResults">
              {{ messages.setup.comparisonRefresh }}
            </button>
            <section aria-label="Compare saved results">
              <h4>{{ messages.setup.comparisonHeading }}</h4>
              <p>{{ messages.setup.comparisonHelp }}</p>
              <p>Saved-change facts are read-only; this action does not apply a result.</p>
              <template v-if="latestPair">
                <button
                  ref="comparisonTrigger"
                  type="button"
                  :disabled="resultsRefreshing || selectingSolutionId !== null"
                  :aria-disabled="comparing"
                  @click="compareLatest(latestPair)"
                >
                  {{ messages.setup.compareResults }}
                </button>
                <button
                  ref="repairFactsTrigger"
                  type="button"
                  :disabled="
                    resultsRefreshing ||
                    selectingSolutionId !== null ||
                    repairFactsState === 'loading' ||
                    latestFactsEligibility !== 'ordinary' ||
                    sourceAdmissionFailure !== null
                  "
                  @click="explainLatestRepairFacts(latestPair)"
                >
                  Explain saved-change facts
                </button>
              </template>
              <p v-else role="status">{{ messages.setup.comparisonNoPair }}</p>
              <p v-if="latestPair">
                The actions above compare the latest two accepted results; they do not choose the
                recorded manual baseline.
              </p>
              <p v-if="latestFactsEligibility === 'checking'" role="status">
                Checking native recorded-baseline eligibility for the latest-two fact action…
              </p>
              <p v-else-if="latestFactsEligibility === 'recorded'" role="status">
                The latest candidate has a recorded repair baseline. Inspect that chosen result and
                use its recorded facts; the latest-two comparison above remains separate.
              </p>
              <p v-else-if="latestFactsEligibility === 'unavailable' && latestPair" role="status">
                Native baseline eligibility is unavailable. Refresh saved results before requesting
                latest-two facts.
              </p>
              <button
                v-if="recordedRepairPair"
                ref="recordedRepairFactsTrigger"
                type="button"
                :disabled="repairFactsState === 'loading' || sourceAdmissionFailure !== null"
                @click="explainLatestRepairFacts(recordedRepairPair)"
              >
                Explain recorded repair facts for chosen result
              </button>
              <button
                v-if="recordedRepairPair"
                type="button"
                :disabled="comparing || sourceAdmissionFailure !== null"
                @click="compareLatest(recordedRepairPair)"
              >
                Compare original A with chosen C
              </button>
              <ExplanationPanel
                v-if="requestedRepairPair"
                :open="repairFactsOpen"
                :result="
                  repairFacts !== null && stillShowingRepairPair(repairFacts.pair)
                    ? repairFacts.result
                    : null
                "
                :state="repairFactsState"
                :message-texts="repairFactMessageTexts"
                :scenario-id="requestedRepairPair.scenarioId"
                :current-revision="requestedRepairPair.visibleRevision"
                :source-revisions="
                  repairFacts !== null && stillShowingRepairPair(repairFacts.pair)
                    ? repairFacts.sourceRevisions
                    : undefined
                "
                :status-message="repairFactsStatus"
                :cancel-requested="repairFactsCancellationRequested"
                :show-edit-related="false"
                :restore-focus="false"
                :locale="locale"
                @update:open="setRepairFactsOpen"
                @retry="explainLatestRepairFacts(requestedRepairPair)"
                @cancel="cancelLatestRepairFacts"
              />
              <p v-if="comparing" role="status" aria-live="polite">
                {{ messages.setup.comparisonLoading }}
              </p>
              <button
                v-if="comparing"
                ref="comparisonCancelTrigger"
                type="button"
                :aria-disabled="comparisonCancellationRequested"
                @click="cancelLatestComparison"
              >
                {{ messages.setup.comparisonCancel }}
              </button>
              <p v-if="comparisonAnnouncement" role="status" aria-live="polite" aria-atomic="true">
                {{ comparisonAnnouncement }}
              </p>
              <p v-if="comparisonError" role="alert">{{ comparisonError }}</p>
              <AcceptedComparison
                v-if="comparison && stillShowingRepairPair(comparison.pair)"
                :comparison="comparison.value"
                :scenario-id="comparison.pair.scenarioId"
                :current-revision="comparison.pair.visibleRevision"
                :base-label="
                  comparison.pair.alternatives
                    ? (comparison.pair.baseLabel ?? 'First accepted alternative')
                    : comparison.pair.recorded
                      ? 'Recorded original A'
                      : 'Previous saved result'
                "
                :candidate-label="
                  comparison.pair.alternatives
                    ? (comparison.pair.candidateLabel ?? 'Second accepted alternative')
                    : comparison.pair.recorded
                      ? 'Chosen accepted C'
                      : 'Latest saved result'
                "
              />
            </section>
            <ul v-if="snapshot.results.solutions.length > 0" class="result-summary-list">
              <li v-for="result in snapshot.results.solutions" :key="result.result.solutionId">
                <strong>{{ messages.setup.accepted }}</strong>
                <p>
                  {{ result.stale ? messages.setup.staleResult : messages.setup.current }} ·
                  {{ result.selected ? messages.setup.selected : messages.setup.unselected }}
                </p>
                <p>
                  {{ messages.setup.resultRevision(formatNumber(result.scenarioRevision, locale)) }}
                </p>
                <dl class="metadata-list">
                  <div>
                    <dt>{{ messages.setup.status }}</dt>
                    <dd>{{ messages.setup.optimize.statuses[result.status] }}</dd>
                  </div>
                  <div>
                    <dt>{{ messages.setup.finished }}</dt>
                    <dd>{{ formatDateTime(result.finishedAt, locale) }}</dd>
                  </div>
                  <div>
                    <dt>{{ messages.setup.resultId }}</dt>
                    <dd>
                      <code>{{ result.result.solutionId }}</code>
                    </dd>
                  </div>
                </dl>
                <button
                  v-if="!result.selected"
                  type="button"
                  :aria-label="messages.setup.selectResultForViewing(result.result.solutionId)"
                  :disabled="
                    selectingSolutionId !== null ||
                    selectionError !== null ||
                    sourceAdmissionFailure !== null
                  "
                  @click="selectAcceptedResult(result, $event.currentTarget as HTMLButtonElement)"
                >
                  {{ messages.setup.selectForViewing }}
                </button>
                <button
                  v-if="capabilities?.availableCommands.includes('solution_adopt_base')"
                  type="button"
                  :aria-expanded="baseReview?.result.result.solutionId === result.result.solutionId"
                  :aria-label="messages.setup.baseCapture.reviewResult(result.result.solutionId)"
                  :disabled="
                    basePending || home.state.busyAction !== null || sourceAdmissionFailure !== null
                  "
                  @click="prepareBaseReview(result)"
                >
                  {{ messages.setup.baseCapture.review }}
                </button>
                <section
                  v-if="
                    baseReview?.result.result.solutionId === result.result.solutionId &&
                    reviewCurrent(baseReview)
                  "
                  class="state-panel"
                  :aria-label="messages.setup.baseCapture.heading"
                >
                  <h4>{{ messages.setup.baseCapture.heading }}</h4>
                  <p>{{ messages.setup.baseCapture.description }}</p>
                  <p>
                    {{ messages.setup.baseCapture.source }}:
                    <code>{{ baseReview.result.result.solutionId }}</code>
                    ({{
                      messages.setup.resultRevision(
                        formatNumber(baseReview.result.scenarioRevision, locale),
                      )
                    }})
                  </p>
                  <p>
                    {{
                      messages.setup.baseCapture.revision(formatNumber(baseReview.revision, locale))
                    }}
                  </p>
                  <p v-if="baseReview.result.stale" role="status">
                    {{ messages.setup.baseCapture.olderSource }}
                  </p>
                  <p v-if="basePending" role="status" aria-live="polite">
                    {{ messages.setup.baseCapture.saving }}
                  </p>
                  <p v-if="baseRedoRequired" role="alert">
                    {{ messages.setup.baseCapture.redoWarning }}
                  </p>
                  <div class="action-row">
                    <button
                      type="button"
                      :aria-disabled="basePending"
                      :disabled="home.state.busyAction !== null && !basePending"
                      @click="adoptReviewedBase(baseRedoRequired)"
                    >
                      {{
                        baseRedoRequired
                          ? messages.setup.baseCapture.confirmRedo
                          : messages.setup.baseCapture.confirm
                      }}
                    </button>
                    <button type="button" :disabled="basePending" @click="clearBaseReview">
                      {{ messages.setup.baseCapture.cancel }}
                    </button>
                  </div>
                </section>
                <button
                  type="button"
                  data-result-inspect
                  :aria-expanded="inspectedSolutionId === result.result.solutionId"
                  @click="
                    inspectedSolutionId =
                      inspectedSolutionId === result.result.solutionId
                        ? null
                        : result.result.solutionId;
                    chosenRepairCandidate = inspectedSolutionId === null ? null : result;
                  "
                >
                  {{
                    inspectedSolutionId === result.result.solutionId
                      ? messages.acceptedSchedule.close
                      : messages.acceptedSchedule.open
                  }}
                </button>
                <AcceptedSchedule
                  v-if="
                    inspectedSolutionId === result.result.solutionId &&
                    sourceAdmissionFailure === null
                  "
                  :scenario-id="project.scenarioId"
                  :summary="result"
                  :current-revision="project.revision"
                  :home="home"
                  :rule-navigation-available="!project.archived && home.state.phase === 'ready'"
                  @rule-action="openConflictRule"
                />
              </li>
            </ul>
            <p v-if="baseError && !baseReview" role="alert">{{ baseError }}</p>
          </section>
        </details>
      </template>
    </template>
  </section>
</template>
