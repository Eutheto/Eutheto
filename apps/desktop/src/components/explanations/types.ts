import type {
  DomainEntityRef,
  EvidenceMessageV1,
  InfeasibilityExplanationContextV1,
  RuleEvaluation,
  UuidV7,
} from "../../api/generated";

export type ExplanationUiState =
  | "ready"
  | "empty"
  | "loading"
  | "stale"
  | "cancelled"
  | "inconclusive"
  | "unavailable"
  | "internalFailure";

export type ConflictRuleAction = InfeasibilityExplanationContextV1 & {
  readonly scenarioId: UuidV7;
  readonly ruleId: UuidV7;
  readonly action: "inspect" | "edit";
};

export interface LabeledEntityRef {
  readonly entity: DomainEntityRef;
  readonly label: string;
}

export interface LabeledRuleEvaluation {
  readonly evaluation: RuleEvaluation;
  readonly label: string;
}

export interface RenderedEvidenceMessage {
  readonly evidence: EvidenceMessageV1;
  readonly text: string;
}
