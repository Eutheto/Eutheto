//! Strict explanation and counterfactual wire contracts.
//!
//! This module contains inert, solver-neutral data only. Constructors derive every certainty,
//! request hash, and checksum; validation rejects noncanonical collections and inconsistent nested
//! records so deserialized clients cannot upgrade a claim.

use crate::VerificationFactId;

use super::CounterfactualTerminalAuthorityV1;
use super::{
    AcceptedResult, AssignmentValue, AssumptionGroupId, BTreeMap, BTreeSet, CounterfactualJobId,
    Deserialize, DomainAssignment, DomainAssignmentId, DomainContractError, DomainEntityRef,
    DomainEvidenceId, DurationMillis, MAX_SCORE_CATEGORIES_PER_LEVEL, MAX_SCORE_LEVELS,
    MAX_VERIFICATION_ENTITIES, MAX_VERIFICATION_EVIDENCE, MAX_VERIFICATION_METRICS,
    MAX_VERIFICATION_RULES, MetricId, MetricValue, NormalizedSolution, OptimizationDirection,
    Ordering, PackId, RequestId, Rfc3339Timestamp, RuleEvaluation, RuleId, RunInput, RunManifest,
    RunTerminalOutcomeV1, ScenarioId, ScenarioSnapshotId, ScoreCategoryId, ScoreLevelId,
    ScoreVector, Serialize, SolutionId, SolveRunId, SolveStatus, VerificationIssueId,
    VerificationValue, canonical_hash, checksum_without_field, ensure_domain_contract_size,
    parse_portable_domain_json, strictly_sorted_unique, strictly_sorted_unique_by,
    validate_fact_map, validate_hash, validate_message_key, validate_metrics, validate_revision,
};
use eutheto_types::ValidationIssue;

/// Current explanation request schema.
pub const EXPLANATION_REQUEST_SCHEMA_VERSION: u32 = 1;
/// Current V2 explanation-evidence schema.
pub const EXPLANATION_EVIDENCE_V2_SCHEMA_VERSION: u32 = 2;
/// Current V2 evidence-render request schema.
pub const EVIDENCE_RENDER_REQUEST_V2_SCHEMA_VERSION: u32 = 2;
/// Current unchanged V1 evidence-render result schema.
pub const EVIDENCE_RENDER_RESULT_SCHEMA_VERSION: u32 = 1;
/// Current full explanation-result schema.
pub const EXPLANATION_RESULT_V2_SCHEMA_VERSION: u32 = 2;
/// Current comparison schema.
pub const SOLUTION_COMPARISON_SCHEMA_VERSION: u32 = 1;
/// Current semantic-only accepted-result comparison schema.
pub const SEMANTIC_DIFFERENCE_SCHEMA_VERSION: u32 = 1;
/// Current counterfactual condition schema.
pub const COUNTERFACTUAL_CONDITION_SCHEMA_VERSION: u32 = 1;
/// Current counterfactual compilation-binding schema.
pub const COUNTERFACTUAL_COMPILATION_BINDING_SCHEMA_VERSION: u32 = 1;
/// Current counterfactual request-semantics schema.
pub const COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION: u32 = 1;
/// Current counterfactual job-request schema.
pub const COUNTERFACTUAL_JOB_REQUEST_SCHEMA_VERSION: u32 = 1;
/// Current counterfactual job-record schema.
pub const COUNTERFACTUAL_JOB_RECORD_SCHEMA_VERSION: u32 = 1;
/// Current counterfactual result schema.
pub const COUNTERFACTUAL_RESULT_SCHEMA_VERSION: u32 = 2;
/// Largest total budget accepted for one interactive counterfactual diagnostic.
///
/// Thirty seconds keeps the diagnostic bounded to the short, user-initiated workflow while still
/// covering the existing five- and thirty-second UX choices.
pub const COUNTERFACTUAL_TOTAL_BUDGET_MAX_MILLISECONDS_V1: u64 = 30_000;

/// Maximum records in one explanation-owned canonical vector.
pub const MAX_EXPLANATION_RECORDS: usize = 100_000;
/// Maximum references of one kind in an evidence message.
pub const MAX_EVIDENCE_MESSAGE_REFERENCES: usize = 1_024;
/// Maximum messages returned by one rendering operation.
pub const MAX_EVIDENCE_MESSAGES: usize = 10_000;
/// Maximum components in a typed validation field path.
pub const MAX_VALIDATION_FIELD_PATH: usize = 64;
/// Derives a stable identity for one complete native validation issue.
///
/// The complete canonical JSON representation is hashed, so findings that share a code and
/// field path remain distinct when their message, severity, or resource differs. Serialization is
/// bounded by [`MAX_DOMAIN_CONTRACT_JSON_BYTES`].
///
/// # Errors
/// Returns [`DomainContractError`] when canonical serialization exceeds its bound or fails.
pub fn validation_issue_identity(
    issue: &ValidationIssue,
) -> Result<VerificationIssueId, DomainContractError> {
    let digest = canonical_hash(issue)?;
    VerificationIssueId::new(format!("validation.issue.{digest}"))
        .map_err(|_| DomainContractError::InvalidExplanationContract("validation issue identity"))
}

fn explanation_error(invariant: &'static str) -> DomainContractError {
    DomainContractError::InvalidExplanationContract(invariant)
}

fn validate_version(actual: u32, expected: u32) -> Result<(), DomainContractError> {
    if actual == expected {
        Ok(())
    } else {
        Err(DomainContractError::UnsupportedVersion(actual))
    }
}

fn validate_bounded_strict<T: Ord>(
    values: &[T],
    maximum: usize,
    label: &'static str,
) -> Result<(), DomainContractError> {
    if values.len() > maximum {
        return Err(DomainContractError::LimitExceeded(label));
    }
    if !strictly_sorted_unique(values) {
        return Err(DomainContractError::NonCanonicalExplanationCollection(
            label,
        ));
    }
    Ok(())
}

fn validate_assignment(assignment: &DomainAssignment) -> Result<(), DomainContractError> {
    if let AssignmentValue::Interval(interval) = assignment.value {
        interval.validate()?;
    }
    validate_bounded_strict(
        &assignment.evidence,
        MAX_VERIFICATION_EVIDENCE,
        "assignment evidence",
    )
}

/// The exact seven explanation kinds exposed to clients.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExplanationKind {
    /// Scenario validation evidence.
    Validation,
    /// Infeasibility conflict evidence.
    Infeasibility,
    /// Evidence for one projected assignment.
    Assignment,
    /// Evidence from a temporary diagnostic solve.
    Counterfactual,
    /// Deterministic accepted-result comparison.
    SolutionDifference,
    /// Repair comparison evidence.
    Repair,
    /// Terminal solve status and proof evidence.
    OptimalityStatus,
}

/// Pack support for the exact seven explanation kinds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExplanationCapability {
    /// Scenario validation rendering.
    Validation,
    /// Infeasibility rendering.
    Infeasibility,
    /// Assignment rendering.
    Assignment,
    /// Counterfactual rendering and compilation.
    Counterfactual,
    /// Solution-difference rendering.
    SolutionDifference,
    /// Repair rendering.
    Repair,
    /// Optimality/status rendering.
    OptimalityStatus,
}

/// Evidence strength. Values are derived from typed evidence and cannot be upgraded by callers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExplanationCertainty {
    /// Deterministic data transformation, not a solver proof.
    Deterministic,
    /// Independently verified candidate evidence.
    IndependentlyVerified,
    /// A backend terminal proof bound to its exact run input.
    BackendProof,
    /// A sufficient, not proven-minimal, conflict.
    SufficientConflict,
    /// A conflict proven minimal by complete deletion testing.
    ProvenMinimalConflict,
    /// The bounded operation did not distinguish the requested case.
    Inconclusive,
    /// The requested evidence was unavailable.
    Unavailable,
}

/// A compact reference to one independently accepted result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptedResultRefV1 {
    /// Accepted normalized solution identity.
    pub solution_id: SolutionId,
    /// Checksum of the complete accepted-result contract.
    pub result_checksum: String,
}

impl AcceptedResultRefV1 {
    /// Creates a validated reference from an independently accepted result.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_result(result: &AcceptedResult) -> Result<Self, DomainContractError> {
        result.validate()?;
        Ok(Self {
            solution_id: result.solution.solution_id,
            result_checksum: result.checksum.clone(),
        })
    }

    /// Validates the checksum shape.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_hash(&self.result_checksum)
    }
}

/// Exact subject vocabulary for an explanation request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    deny_unknown_fields
)]
pub enum ExplanationRequestSubjectV1 {
    /// Explain one issue or the validation summary.
    Validation {
        /// Optional stable issue identity.
        issue_id: Option<VerificationIssueId>,
    },
    /// Explain an infeasible run or one retained conflict.
    Infeasibility {
        /// Terminal solve run.
        solve_run_id: SolveRunId,
        /// Checksum of its validated run manifest.
        run_manifest_checksum: String,
        /// Optional retained conflict evidence identity.
        conflict_id: Option<DomainEvidenceId>,
    },
    /// Explain one assignment in an accepted result.
    Assignment {
        /// Accepted result containing the assignment.
        result: AcceptedResultRefV1,
        /// Assignment to explain.
        assignment_id: DomainAssignmentId,
    },
    /// Explain a completed counterfactual job.
    Counterfactual {
        /// Counterfactual job identity.
        job_id: CounterfactualJobId,
        /// Base accepted result.
        base: AcceptedResultRefV1,
    },
    /// Compare two accepted results.
    SolutionDifference {
        /// Left/base result.
        left: AcceptedResultRefV1,
        /// Right/candidate result.
        right: AcceptedResultRefV1,
    },
    /// Explain a repaired result against its base.
    Repair {
        /// Current repaired result.
        current: AcceptedResultRefV1,
        /// Prior base result.
        base: AcceptedResultRefV1,
    },
    /// Explain terminal status and any proof claim.
    OptimalityStatus {
        /// Terminal solve run.
        solve_run_id: SolveRunId,
        /// Checksum of its validated manifest.
        run_manifest_checksum: String,
        /// Optional independently accepted result.
        result: Option<AcceptedResultRefV1>,
    },
}

impl ExplanationRequestSubjectV1 {
    /// Returns the subject kind.
    #[must_use]
    pub const fn kind(&self) -> ExplanationKind {
        match self {
            Self::Validation { .. } => ExplanationKind::Validation,
            Self::Infeasibility { .. } => ExplanationKind::Infeasibility,
            Self::Assignment { .. } => ExplanationKind::Assignment,
            Self::Counterfactual { .. } => ExplanationKind::Counterfactual,
            Self::SolutionDifference { .. } => ExplanationKind::SolutionDifference,
            Self::Repair { .. } => ExplanationKind::Repair,
            Self::OptimalityStatus { .. } => ExplanationKind::OptimalityStatus,
        }
    }

    fn validate(&self) -> Result<(), DomainContractError> {
        match self {
            Self::Validation { .. } => Ok(()),
            Self::Infeasibility {
                run_manifest_checksum,
                ..
            }
            | Self::OptimalityStatus {
                run_manifest_checksum,
                ..
            } => validate_hash(run_manifest_checksum),
            Self::Assignment { result, .. } => result.validate(),
            Self::Counterfactual { base, .. } => base.validate(),
            Self::SolutionDifference { left, right } => {
                left.validate()?;
                right.validate()
            }
            Self::Repair { current, base } => {
                current.validate()?;
                base.validate()
            }
        }?;
        if let Self::OptimalityStatus {
            result: Some(result),
            ..
        } = self
        {
            result.validate()?;
        }
        Ok(())
    }
}

/// Strict versioned explanation request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExplanationRequestV1 {
    /// Must equal [`EXPLANATION_REQUEST_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Exact typed subject.
    pub subject: ExplanationRequestSubjectV1,
}

impl ExplanationRequestV1 {
    /// Creates a validated request.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(subject: ExplanationRequestSubjectV1) -> Result<Self, DomainContractError> {
        let value = Self {
            schema_version: EXPLANATION_REQUEST_SCHEMA_VERSION,
            subject,
        };
        value.validate()?;
        ensure_domain_contract_size(&value)?;
        Ok(value)
    }

    /// Parses with shared portable-JSON ingress limits and validates every invariant.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates the schema and nested reference checksums.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_version(self.schema_version, EXPLANATION_REQUEST_SCHEMA_VERSION)?;
        self.subject.validate()
    }
}

/// Exact four-level validation meaning used by explanations.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExplanationValidationSeverity {
    /// Must be corrected before solving.
    MustFix,
    /// Strong evidence of a problem that may still be allowed by policy.
    LikelyProblem,
    /// A deterministic review prompt without a correctness claim.
    ReviewSuggested,
    /// Informational context only.
    Information,
}

/// Typed validation issue evidence; no free-form path or rendered text is retained.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationIssueEvidenceV1 {
    /// Stable issue identity.
    pub issue_id: VerificationIssueId,
    /// Exact explanation severity.
    pub severity: ExplanationValidationSeverity,
    /// Stable localization key.
    pub message_key: String,
    /// Bounded typed message parameters.
    pub parameters: BTreeMap<VerificationFactId, VerificationValue>,
    /// Optional non-empty typed field path in semantic order.
    pub field_path: Option<Vec<VerificationFactId>>,
    /// Optional affected entity.
    pub entity: Option<DomainEntityRef>,
    /// Optional affected rule.
    pub rule_id: Option<RuleId>,
}

impl ValidationIssueEvidenceV1 {
    /// Validates the message, parameters, and bounded typed field path.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_message_key(&self.message_key)?;
        validate_fact_map(&self.parameters)?;
        if let Some(path) = &self.field_path
            && (path.is_empty() || path.len() > MAX_VALIDATION_FIELD_PATH)
        {
            return Err(explanation_error("validation field path"));
        }
        Ok(())
    }
}

/// Conflict minimality established by bounded deletion trials.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictMinimality {
    /// The groups are known sufficient for infeasibility.
    Sufficient,
    /// Every remaining group was proven necessary.
    ProvenMinimal,
}

/// Why bounded conflict shrinking stopped.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictShrinkStopReason {
    /// Every remaining group was proven necessary.
    Completed,
    /// No deletion trials were configured.
    NotAttempted,
    /// The configured trial count was exhausted.
    TrialLimit,
    /// The shared wall-clock budget expired.
    BudgetExpired,
    /// Explicit cancellation was observed.
    Cancelled,
    /// A trial did not prove either feasibility or infeasibility.
    Inconclusive,
}

/// Auditable summary of bounded conflict shrinking.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictShrinkSummaryV1 {
    /// Number of groups before deletion trials.
    pub initial_group_count: u32,
    /// Number of groups retained in the resulting conflict.
    pub remaining_group_count: u32,
    /// Number of solver trials actually attempted.
    pub attempted_trials: u32,
    /// Maximum permitted trials.
    pub max_trials: u32,
    /// Deterministic stop classification.
    pub stop_reason: ConflictShrinkStopReason,
}

impl ConflictShrinkSummaryV1 {
    /// Validates trial counts and stop semantics.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        let counts_valid = self.initial_group_count > 0
            && self.remaining_group_count > 0
            && self.remaining_group_count <= self.initial_group_count;
        let stop_valid = match self.stop_reason {
            ConflictShrinkStopReason::Completed => {
                self.attempted_trials == self.initial_group_count
                    && self.attempted_trials <= self.max_trials
            }
            ConflictShrinkStopReason::NotAttempted => {
                self.max_trials == 0
                    && self.attempted_trials == 0
                    && self.remaining_group_count == self.initial_group_count
            }
            ConflictShrinkStopReason::TrialLimit => {
                self.max_trials > 0
                    && self.attempted_trials == self.max_trials
                    && self.attempted_trials < self.initial_group_count
            }
            ConflictShrinkStopReason::BudgetExpired
            | ConflictShrinkStopReason::Cancelled
            | ConflictShrinkStopReason::Inconclusive => self.attempted_trials <= self.max_trials,
        };
        if !counts_valid || !stop_valid {
            return Err(explanation_error("conflict shrink summary"));
        }
        Ok(())
    }
}

/// One canonical explainable assumption group in a conflict.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictGroupV1 {
    /// Typed assumption-group identity.
    pub group_id: AssumptionGroupId,
    /// Non-empty required rules in ascending identity order.
    pub required_rules: Vec<RuleId>,
}

impl ConflictGroupV1 {
    /// Validates a non-empty canonical required-rule set.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.required_rules.is_empty() {
            return Err(explanation_error("empty conflict group"));
        }
        validate_bounded_strict(
            &self.required_rules,
            MAX_VERIFICATION_RULES,
            "conflict required rules",
        )
    }
}

/// Typed reason that assumption-based conflict evidence is unavailable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictUnavailableReason {
    /// The model has no explainable assumptions.
    AssumptionsUnavailable,
    /// Infeasibility is caused by foundational constraints outside assumptions.
    FoundationalInfeasibility,
    /// The backend did not return a usable conflict.
    ConflictNotReturned,
    /// Returned assumption evidence was invalid, duplicated, out of set, or wrong-polarity.
    InvalidAssumptionCore,
    /// The captured run intentionally requested no optional diagnostics.
    NotRequested,
    /// A legacy run has no retained diagnostic evidence.
    NotRecorded,
    /// The installed pack does not advertise this diagnostic capability.
    UnsupportedDomainPack,
    /// Optional enrichment exhausted its inherited deadline.
    BudgetExpired,
    /// Explicit cancellation stopped optional enrichment.
    Cancelled,
    /// Complete source groups or producer storage exceeded the charged resource ceiling.
    ResourceLimitExceeded,
    /// Diagnostic compilation or dispatch failed without replacing original solve knowledge.
    DiagnosticFailed,
}

/// Complete infeasibility evidence or an explicit typed unavailable reason.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type", deny_unknown_fields)]
pub enum InfeasibilityEvidenceV1 {
    /// Non-empty canonical conflict groups.
    Conflict {
        /// Groups sorted by typed group identity.
        groups: Vec<ConflictGroupV1>,
        /// Established minimality.
        minimality: ConflictMinimality,
        /// Bounded deletion-trial summary.
        shrink: ConflictShrinkSummaryV1,
    },
    /// Conflict evidence could not safely be produced.
    Unavailable {
        /// Exact unavailable classification.
        reason: ConflictUnavailableReason,
    },
}

impl InfeasibilityEvidenceV1 {
    /// Validates complete conflict shape and shrink state, without granting local proof authority.
    ///
    /// # Errors
    /// Rejects empty, noncanonical or inconsistent evidence.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        match self {
            Self::Conflict {
                groups,
                minimality,
                shrink,
            } => {
                if groups.is_empty() {
                    return Err(explanation_error("empty infeasibility conflict"));
                }
                validate_bounded_strict(groups, MAX_EXPLANATION_RECORDS, "conflict groups")?;
                for group in groups {
                    group.validate()?;
                }
                shrink.validate()?;
                if (*minimality == ConflictMinimality::ProvenMinimal)
                    != (shrink.stop_reason == ConflictShrinkStopReason::Completed)
                {
                    return Err(explanation_error("conflict minimality"));
                }
                let remaining_group_count = usize::try_from(shrink.remaining_group_count)
                    .map_err(|_| explanation_error("conflict remaining group count"))?;
                if remaining_group_count != groups.len() {
                    return Err(explanation_error("conflict remaining group count"));
                }
                Ok(())
            }
            Self::Unavailable { .. } => Ok(()),
        }
    }

    const fn certainty(&self) -> ExplanationCertainty {
        match self {
            Self::Conflict {
                minimality: ConflictMinimality::Sufficient,
                ..
            } => ExplanationCertainty::SufficientConflict,
            Self::Conflict {
                minimality: ConflictMinimality::ProvenMinimal,
                ..
            } => ExplanationCertainty::ProvenMinimalConflict,
            Self::Unavailable { .. } => ExplanationCertainty::Unavailable,
        }
    }
}

/// One explicit contribution associated with an assignment explanation.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreContributionV1 {
    /// Stable identity of this independently attributable contribution.
    pub evidence_id: DomainEvidenceId,
    /// Objective level receiving the contribution.
    pub level_id: ScoreLevelId,
    /// Optional explanatory category within that level.
    pub category_id: Option<ScoreCategoryId>,
    /// Exact signed contribution.
    pub value: i64,
}

/// Optional observed lock state. Absence means lock information was not supplied.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state", deny_unknown_fields)]
pub enum AssignmentLockStateV1 {
    /// No lock was present.
    Unlocked,
    /// A lock required this exact typed value.
    Locked {
        /// Locked value.
        value: AssignmentValue,
    },
}

impl AssignmentLockStateV1 {
    fn validate(&self) -> Result<(), DomainContractError> {
        if let Self::Locked {
            value: AssignmentValue::Interval(interval),
        } = self
        {
            interval.validate()?;
        }
        Ok(())
    }
}

/// Independently verified evidence for one exact assignment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentEvidenceV1 {
    /// Exact assignment.
    pub assignment: DomainAssignment,
    /// Related independently evaluated rules in ascending rule-ID order.
    pub related_rules: Vec<RuleEvaluation>,
    /// Explicit score contributions in ascending stable evidence-ID order.
    pub score_contributions: Vec<ScoreContributionV1>,
    /// Exact independently computed metrics.
    pub metrics: BTreeMap<MetricId, MetricValue>,
    /// Lock state only when a caller supplied it.
    pub lock_state: Option<AssignmentLockStateV1>,
}

impl AssignmentEvidenceV1 {
    /// Validates the exact assignment and every canonical nested collection.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_assignment(&self.assignment)?;
        if self.related_rules.len() > MAX_VERIFICATION_RULES {
            return Err(DomainContractError::LimitExceeded(
                "assignment related rules",
            ));
        }
        if !strictly_sorted_unique_by(&self.related_rules, |value| &value.rule_id) {
            return Err(DomainContractError::NonCanonicalExplanationCollection(
                "assignment related rules",
            ));
        }
        for evaluation in &self.related_rules {
            evaluation.validate()?;
        }
        if self.score_contributions.len() > MAX_EXPLANATION_RECORDS {
            return Err(DomainContractError::LimitExceeded("score contributions"));
        }
        if !strictly_sorted_unique_by(&self.score_contributions, |value| &value.evidence_id) {
            return Err(DomainContractError::NonCanonicalExplanationCollection(
                "score contributions",
            ));
        }
        validate_metrics(&self.metrics)?;
        if let Some(lock_state) = &self.lock_state {
            lock_state.validate()?;
        }
        Ok(())
    }
}

/// Candidate ordering relative to the base accepted result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ComparisonOrdering {
    /// Candidate is lexicographically better.
    Better,
    /// Scores are equivalent under the compared objective shape.
    Equivalent,
    /// Candidate is lexicographically worse.
    Worse,
}

/// Immutable identity retained for one side of a comparison.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComparisonBindingV1 {
    /// Domain pack identity.
    pub pack_id: PackId,
    /// Scenario identity.
    pub scenario_id: ScenarioId,
    /// Side-specific immutable scenario revision.
    pub scenario_revision: u64,
    /// Side-specific canonical scenario document hash.
    pub document_hash: String,
    /// Compatible projection contract version.
    pub projection_version: u32,
    /// Side-specific verification-scope checksum.
    pub verification_scope_checksum: String,
    /// Side-specific accepted result.
    pub accepted_result: AcceptedResultRefV1,
}

impl ComparisonBindingV1 {
    /// Validates revision and hash/reference bindings retained for a comparison side.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_revision(self.scenario_revision)?;
        if self.projection_version == 0 {
            return Err(DomainContractError::ZeroVersion("projection version"));
        }
        validate_hash(&self.document_hash)?;
        validate_hash(&self.verification_scope_checksum)?;
        self.accepted_result.validate()
    }
}

/// Canonical assignment delta.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type", deny_unknown_fields)]
pub enum AssignmentComparisonV1 {
    /// Assignment exists only in the candidate.
    Added {
        /// Candidate assignment.
        after: DomainAssignment,
    },
    /// Assignment exists only in the base.
    Removed {
        /// Base assignment.
        before: DomainAssignment,
    },
    /// Assignment value/entity/evidence changed.
    Changed {
        /// Base assignment.
        before: DomainAssignment,
        /// Candidate assignment with the same identity.
        after: DomainAssignment,
    },
}

impl AssignmentComparisonV1 {
    /// Returns the assignment identity used for canonical ordering.
    #[must_use]
    pub fn assignment_id(&self) -> &DomainAssignmentId {
        match self {
            Self::Added { after } => &after.id,
            Self::Removed { before } | Self::Changed { before, .. } => &before.id,
        }
    }

    fn validate(&self) -> Result<(), DomainContractError> {
        match self {
            Self::Added { after } => validate_assignment(after),
            Self::Removed { before } => validate_assignment(before),
            Self::Changed { before, after } => {
                validate_assignment(before)?;
                validate_assignment(after)?;
                if before.id != after.id || before == after {
                    return Err(explanation_error("assignment comparison"));
                }
                Ok(())
            }
        }
    }
}

/// Required-rule status delta. `None` denotes an added or removed rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleComparisonV1 {
    /// Stable required-rule identity.
    pub rule_id: RuleId,
    /// Base evaluation, absent for an added rule.
    pub before: Option<RuleEvaluation>,
    /// Candidate evaluation, absent for a removed rule.
    pub after: Option<RuleEvaluation>,
}

impl RuleComparisonV1 {
    /// Validates optional-side identity and requires a real add/remove/change.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.before.is_none() && self.after.is_none() {
            return Err(explanation_error("empty rule comparison"));
        }
        for evaluation in [self.before.as_ref(), self.after.as_ref()]
            .into_iter()
            .flatten()
        {
            evaluation.validate()?;
            if evaluation.rule_id != self.rule_id {
                return Err(explanation_error("rule comparison identity"));
            }
        }
        if self.before == self.after {
            return Err(explanation_error("unchanged rule comparison"));
        }
        Ok(())
    }
}

/// Exact category values and checked arithmetic delta.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreCategoryComparisonV1 {
    /// Category identity.
    pub category_id: ScoreCategoryId,
    /// Base total when present.
    pub before: Option<i64>,
    /// Candidate total when present.
    pub after: Option<i64>,
    /// `after - before` exactly when both sides are present.
    pub delta: Option<i64>,
}

impl ScoreCategoryComparisonV1 {
    /// Validates presence and checked delta.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.before.is_none() && self.after.is_none() {
            return Err(explanation_error("empty score category comparison"));
        }
        let expected = if let (Some(before), Some(after)) = (self.before, self.after) {
            after
                .checked_sub(before)
                .ok_or_else(|| explanation_error("score category comparison arithmetic overflow"))?
        } else {
            if self.delta.is_some() {
                return Err(explanation_error("score category comparison delta"));
            }
            return Ok(());
        };
        if self.delta != Some(expected) {
            return Err(explanation_error("score category comparison delta"));
        }
        Ok(())
    }
}

/// Exact score-level values, direction, checked delta, and optional category detail.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreLevelComparisonV1 {
    /// Level identity.
    pub level_id: ScoreLevelId,
    /// Shared comparison direction.
    pub direction: OptimizationDirection,
    /// Base value.
    pub before: i64,
    /// Candidate value.
    pub after: i64,
    /// Checked arithmetic `after - before`.
    pub delta: i64,
    /// Canonical union of present category totals.
    pub categories: Vec<ScoreCategoryComparisonV1>,
}

impl ScoreLevelComparisonV1 {
    /// Validates checked arithmetic and canonical category records.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.after.checked_sub(self.before) != Some(self.delta) {
            return Err(explanation_error("score level comparison delta"));
        }
        validate_bounded_strict(
            &self.categories,
            MAX_SCORE_CATEGORIES_PER_LEVEL,
            "score category comparisons",
        )?;
        for category in &self.categories {
            category.validate()?;
        }
        Ok(())
    }
}

fn validate_score_categories(
    comparisons: &[ScoreCategoryComparisonV1],
    before: &BTreeMap<ScoreCategoryId, i64>,
    after: &BTreeMap<ScoreCategoryId, i64>,
) -> Result<(), DomainContractError> {
    let mut before = before.iter().peekable();
    let mut after = after.iter().peekable();
    let mut comparison_index = 0;
    loop {
        let (category_id, before_value, after_value) = match (before.peek(), after.peek()) {
            (Some((before_id, before_value)), Some((after_id, after_value))) => {
                match before_id.cmp(after_id) {
                    Ordering::Less => {
                        let result = ((**before_id).clone(), Some(**before_value), None);
                        before.next();
                        result
                    }
                    Ordering::Equal => {
                        let result = (
                            (**before_id).clone(),
                            Some(**before_value),
                            Some(**after_value),
                        );
                        before.next();
                        after.next();
                        result
                    }
                    Ordering::Greater => {
                        let result = ((**after_id).clone(), None, Some(**after_value));
                        after.next();
                        result
                    }
                }
            }
            (Some((before_id, before_value)), None) => {
                let result = ((**before_id).clone(), Some(**before_value), None);
                before.next();
                result
            }
            (None, Some((after_id, after_value))) => {
                let result = ((**after_id).clone(), None, Some(**after_value));
                after.next();
                result
            }
            (None, None) => break,
        };
        let comparison = comparisons
            .get(comparison_index)
            .ok_or_else(|| explanation_error("score category comparison binding"))?;
        if comparison.category_id != category_id
            || comparison.before != before_value
            || comparison.after != after_value
        {
            return Err(explanation_error("score category comparison binding"));
        }
        comparison_index += 1;
    }
    if comparison_index != comparisons.len() {
        return Err(explanation_error("score category comparison binding"));
    }
    Ok(())
}

/// Exact metric values. No numeric delta is fabricated for absent or non-integer values.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetricComparisonV1 {
    /// Metric identity.
    pub metric_id: MetricId,
    /// Base metric when present.
    pub before: Option<MetricValue>,
    /// Candidate metric when present.
    pub after: Option<MetricValue>,
}

impl MetricComparisonV1 {
    /// Validates both typed values and requires a real add/remove/change.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.before == self.after {
            return Err(explanation_error("unchanged metric comparison"));
        }
        for metric in [self.before.as_ref(), self.after.as_ref()]
            .into_iter()
            .flatten()
        {
            metric.validate()?;
        }
        Ok(())
    }
}

/// Lock preservation record emitted only from explicitly supplied lock context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockComparisonV1 {
    /// Assignment whose lock context was supplied.
    pub assignment_id: DomainAssignmentId,
    /// Base lock state.
    pub before: AssignmentLockStateV1,
    /// Candidate lock state.
    pub after: AssignmentLockStateV1,
    /// True exactly when both states are equal.
    pub preserved: bool,
}

impl LockComparisonV1 {
    /// Validates typed values and the derived preservation flag.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.before.validate()?;
        self.after.validate()?;
        if self.preserved != (self.before == self.after) {
            return Err(explanation_error("lock preservation"));
        }
        Ok(())
    }
}

/// Side-specific terminal status/proof comparison emitted only from supplied manifests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunComparisonSideV1 {
    /// Run identity.
    pub run_id: SolveRunId,
    /// Checksum of the validated terminal run manifest.
    pub run_manifest_checksum: String,
    /// Validated terminal outcome.
    pub outcome: RunTerminalOutcomeV1,
    /// Certainty derived from the terminal outcome.
    pub certainty: ExplanationCertainty,
}

impl RunComparisonSideV1 {
    fn expected_certainty(&self) -> ExplanationCertainty {
        certainty_for_terminal_outcome(&self.outcome)
    }

    /// Validates the outcome and derived certainty.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_hash(&self.run_manifest_checksum)?;
        self.outcome.validate()?;
        if self.certainty != self.expected_certainty() {
            return Err(explanation_error("run comparison certainty"));
        }
        Ok(())
    }
}

/// Optional terminal run comparison based on explicitly supplied manifests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunComparisonV1 {
    /// Base run.
    pub base: RunComparisonSideV1,
    /// Candidate run.
    pub candidate: RunComparisonSideV1,
}

impl RunComparisonV1 {
    /// Validates both terminal sides.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.base.validate()?;
        self.candidate.validate()
    }
}

/// Exact recorded terminal metadata, not authenticated historical backend proof.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunComparisonSideV2 {
    /// Recorded run identity.
    pub run_id: SolveRunId,
    /// Checksum binding the recorded terminal manifest.
    pub run_manifest_checksum: String,
    /// Recorded closed outcome; current domain verification does not reprove optimality.
    pub outcome: RunTerminalOutcomeV1,
}

impl RunComparisonSideV2 {
    /// Validates descriptive status and checksum without deriving backend authority.
    ///
    /// # Errors
    /// Rejects malformed terminal metadata or checksum.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_hash(&self.run_manifest_checksum)?;
        self.outcome.validate()
    }
}

/// Both recorded terminal sides of a current comparison.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunComparisonV2 {
    /// Base recorded run.
    pub base: RunComparisonSideV2,
    /// Candidate recorded run.
    pub candidate: RunComparisonSideV2,
}

impl RunComparisonV2 {
    /// Validates both descriptive terminal sides.
    ///
    /// # Errors
    /// Rejects malformed terminal metadata or checksum.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.base.validate()?;
        self.candidate.validate()
    }
}

fn validate_shared_comparison_bindings(
    base: &ComparisonBindingV1,
    candidate: &ComparisonBindingV1,
) -> Result<(), DomainContractError> {
    if base.pack_id != candidate.pack_id
        || base.scenario_id != candidate.scenario_id
        || base.projection_version != candidate.projection_version
    {
        return Err(explanation_error("comparison compatibility"));
    }
    if base.scenario_revision == candidate.scenario_revision
        && base.verification_scope_checksum != candidate.verification_scope_checksum
    {
        return Err(explanation_error("same-revision verification scope"));
    }
    Ok(())
}

fn validate_assignment_comparisons(
    assignments: &[AssignmentComparisonV1],
) -> Result<(), DomainContractError> {
    if assignments.len() > MAX_EXPLANATION_RECORDS
        || !assignments
            .windows(2)
            .all(|pair| pair[0].assignment_id() < pair[1].assignment_id())
    {
        return Err(DomainContractError::NonCanonicalExplanationCollection(
            "assignment comparisons",
        ));
    }
    for value in assignments {
        value.validate()?;
    }
    Ok(())
}

fn validate_rule_comparisons(rules: &[RuleComparisonV1]) -> Result<(), DomainContractError> {
    if rules.len() > MAX_VERIFICATION_RULES
        || !strictly_sorted_unique_by(rules, |value| &value.rule_id)
    {
        return Err(DomainContractError::NonCanonicalExplanationCollection(
            "rule comparisons",
        ));
    }
    for value in rules {
        value.validate()?;
    }
    Ok(())
}

fn validate_metric_comparisons(metrics: &[MetricComparisonV1]) -> Result<(), DomainContractError> {
    if metrics.len() > MAX_VERIFICATION_METRICS
        || !strictly_sorted_unique_by(metrics, |value| &value.metric_id)
    {
        return Err(DomainContractError::NonCanonicalExplanationCollection(
            "metric comparisons",
        ));
    }
    for value in metrics {
        value.validate()?;
    }
    Ok(())
}

fn validate_affected_entities(
    affected_entities: &[DomainEntityRef],
) -> Result<(), DomainContractError> {
    validate_bounded_strict(
        affected_entities,
        MAX_VERIFICATION_ENTITIES,
        "comparison affected entities",
    )
}

/// Canonical accepted-result differences that deliberately omit objective scores and lock state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticDifferenceV1 {
    /// Must equal [`SEMANTIC_DIFFERENCE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Base result binding.
    pub base: ComparisonBindingV1,
    /// Candidate result binding. Its revision/scope may differ for repair.
    pub candidate: ComparisonBindingV1,
    /// Canonical added/removed/changed assignments.
    pub assignments: Vec<AssignmentComparisonV1>,
    /// Canonical added/removed/changed required rules.
    pub rules: Vec<RuleComparisonV1>,
    /// Canonical changed metrics.
    pub metrics: Vec<MetricComparisonV1>,
    /// Terminal comparison only when validated manifests were supplied.
    pub runs: Option<RunComparisonV1>,
    /// Canonical union of entities affected by assignment/rule changes.
    pub affected_entities: Vec<DomainEntityRef>,
    /// Lowercase BLAKE3 checksum excluding this field.
    pub checksum: String,
}

impl SemanticDifferenceV1 {
    /// Computes a checksum after validating all semantic comparison invariants.
    #[allow(clippy::too_many_arguments)]
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(
        base: ComparisonBindingV1,
        candidate: ComparisonBindingV1,
        assignments: Vec<AssignmentComparisonV1>,
        rules: Vec<RuleComparisonV1>,
        metrics: Vec<MetricComparisonV1>,
        runs: Option<RunComparisonV1>,
        affected_entities: Vec<DomainEntityRef>,
    ) -> Result<Self, DomainContractError> {
        let mut value = Self {
            schema_version: SEMANTIC_DIFFERENCE_SCHEMA_VERSION,
            base,
            candidate,
            assignments,
            rules,
            metrics,
            runs,
            affected_entities,
            checksum: String::new(),
        };
        value.validate_payload()?;
        value.checksum = checksum_without_field(&value, "checksum")?;
        ensure_domain_contract_size(&value)?;
        Ok(value)
    }

    /// Parses under strict portable JSON limits and validates the checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when the JSON or semantic difference is invalid.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates all nested records, compatibility, canonical order, bounds, and checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        ensure_domain_contract_size(self)?;
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        validate_version(self.schema_version, SEMANTIC_DIFFERENCE_SCHEMA_VERSION)?;
        self.base.validate()?;
        self.candidate.validate()?;
        validate_shared_comparison_bindings(&self.base, &self.candidate)?;
        validate_assignment_comparisons(&self.assignments)?;
        validate_rule_comparisons(&self.rules)?;
        validate_metric_comparisons(&self.metrics)?;
        if let Some(runs) = &self.runs {
            runs.validate()?;
        }
        validate_affected_entities(&self.affected_entities)
    }
}

/// Canonical cross-revision-capable accepted-result comparison.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionComparisonV1 {
    /// Must equal [`SOLUTION_COMPARISON_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Base result binding.
    pub base: ComparisonBindingV1,
    /// Candidate result binding. Its revision/scope may differ for repair.
    pub candidate: ComparisonBindingV1,
    /// Exact independently recomputed base score.
    pub base_score: ScoreVector,
    /// Exact independently recomputed candidate score.
    pub candidate_score: ScoreVector,
    /// Canonical added/removed/changed assignments.
    pub assignments: Vec<AssignmentComparisonV1>,
    /// Canonical added/removed/changed required rules.
    pub rules: Vec<RuleComparisonV1>,
    /// Objective levels in semantic precedence order.
    pub score_levels: Vec<ScoreLevelComparisonV1>,
    /// Canonical changed metrics.
    pub metrics: Vec<MetricComparisonV1>,
    /// Canonical lock comparisons, empty when no lock context was supplied.
    pub locks: Vec<LockComparisonV1>,
    /// Terminal comparison only when validated manifests were supplied.
    pub runs: Option<RunComparisonV1>,
    /// Canonical union of entities affected by assignment/rule changes.
    pub affected_entities: Vec<DomainEntityRef>,
    /// Candidate ordering relative to base.
    pub ordering: ComparisonOrdering,
    /// Lowercase BLAKE3 checksum excluding this field.
    pub checksum: String,
}

impl SolutionComparisonV1 {
    /// Computes a checksum after validating all cross-revision comparison invariants.
    #[allow(clippy::too_many_arguments)]
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(
        base: ComparisonBindingV1,
        candidate: ComparisonBindingV1,
        base_score: ScoreVector,
        candidate_score: ScoreVector,
        assignments: Vec<AssignmentComparisonV1>,
        rules: Vec<RuleComparisonV1>,
        score_levels: Vec<ScoreLevelComparisonV1>,
        metrics: Vec<MetricComparisonV1>,
        locks: Vec<LockComparisonV1>,
        runs: Option<RunComparisonV1>,
        affected_entities: Vec<DomainEntityRef>,
        ordering: ComparisonOrdering,
    ) -> Result<Self, DomainContractError> {
        let mut value = Self {
            schema_version: SOLUTION_COMPARISON_SCHEMA_VERSION,
            base,
            base_score,
            candidate_score,
            candidate,
            assignments,
            rules,
            score_levels,
            metrics,
            locks,
            runs,
            affected_entities,
            ordering,
            checksum: String::new(),
        };
        value.validate_payload()?;
        value.checksum = checksum_without_field(&value, "checksum")?;
        ensure_domain_contract_size(&value)?;
        Ok(value)
    }

    /// Parses under strict portable JSON limits and validates the checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates all nested records, compatibility, canonical order, and checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        validate_version(self.schema_version, SOLUTION_COMPARISON_SCHEMA_VERSION)?;
        self.base.validate()?;
        self.candidate.validate()?;
        validate_shared_comparison_bindings(&self.base, &self.candidate)?;
        validate_assignment_comparisons(&self.assignments)?;
        validate_rule_comparisons(&self.rules)?;
        validate_score_comparisons(
            &self.base_score,
            &self.candidate_score,
            &self.score_levels,
            self.ordering,
        )?;
        self.validate_optional_context()
    }

    fn validate_optional_context(&self) -> Result<(), DomainContractError> {
        validate_metric_comparisons(&self.metrics)?;
        if self.locks.len() > MAX_EXPLANATION_RECORDS
            || !strictly_sorted_unique_by(&self.locks, |value| &value.assignment_id)
        {
            return Err(DomainContractError::NonCanonicalExplanationCollection(
                "lock comparisons",
            ));
        }
        for value in &self.locks {
            value.validate()?;
        }
        if let Some(runs) = &self.runs {
            runs.validate()?;
        }
        validate_affected_entities(&self.affected_entities)
    }
}

fn validate_score_comparisons(
    base_score: &ScoreVector,
    candidate_score: &ScoreVector,
    score_levels: &[ScoreLevelComparisonV1],
    ordering: ComparisonOrdering,
) -> Result<(), DomainContractError> {
    base_score.validate_current_shape()?;
    candidate_score.validate_current_shape()?;
    let expected_ordering = match base_score.compare(candidate_score)? {
        Ordering::Less => ComparisonOrdering::Worse,
        Ordering::Equal => ComparisonOrdering::Equivalent,
        Ordering::Greater => ComparisonOrdering::Better,
    };
    if ordering != expected_ordering {
        return Err(explanation_error("comparison ordering"));
    }
    if score_levels.len() != base_score.levels.len() {
        return Err(explanation_error("score comparison levels"));
    }
    if score_levels.len() > MAX_SCORE_LEVELS {
        return Err(DomainContractError::LimitExceeded(
            "score level comparisons",
        ));
    }
    let mut level_ids = BTreeSet::new();
    for ((comparison, base), candidate) in score_levels
        .iter()
        .zip(&base_score.levels)
        .zip(&candidate_score.levels)
    {
        if !level_ids.insert(&comparison.level_id) {
            return Err(DomainContractError::NonCanonicalExplanationCollection(
                "score level comparisons",
            ));
        }
        comparison.validate()?;
        if comparison.level_id != base.level_id
            || comparison.level_id != candidate.level_id
            || comparison.direction != base.direction
            || comparison.direction != candidate.direction
            || comparison.before != base.value
            || comparison.after != candidate.value
        {
            return Err(explanation_error("score comparison binding"));
        }
        validate_score_categories(
            &comparison.categories,
            &base.category_breakdown,
            &candidate.category_breakdown,
        )?;
    }
    Ok(())
}

/// Current complete accepted-result comparison schema; V1 remains a stored counterfactual format.
pub const SOLUTION_COMPARISON_V2_SCHEMA_VERSION: u32 = 2;

/// Protection of one Boolean assignment selection, not a generic locked value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type", deny_unknown_fields)]
pub enum AssignmentProtectionStateV1 {
    /// An explicit unlocked record, distinct from an absent record.
    Unlocked,
    /// The assignment must be selected.
    Hard,
    /// Deselection incurs this positive stability weight.
    Soft {
        /// Positive source-authored weight.
        weight: u32,
    },
}

/// Both source observations and independently derived preservation facts for a protected pair.
#[allow(clippy::struct_excessive_bools)] // Separate source observations and checked derived facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentProtectionComparisonV1 {
    /// Pack-owned canonical assignment identity.
    pub assignment_id: DomainAssignmentId,
    /// Base source record, absent when no record existed.
    pub before: Option<AssignmentProtectionStateV1>,
    /// Candidate source record, absent when no record existed.
    pub after: Option<AssignmentProtectionStateV1>,
    /// Selection in the complete independently verified base result.
    pub before_selected: bool,
    /// Selection in the complete independently verified candidate result.
    pub after_selected: bool,
    /// True exactly when optional protection states are equal.
    pub state_preserved: bool,
    /// True exactly when both selection observations are equal.
    pub assignment_preserved: bool,
}

impl AssignmentProtectionComparisonV1 {
    /// Derives preservation from explicit source and selection observations.
    ///
    /// # Errors
    /// Rejects absent protection on both sides, zero Soft weight or an unselected Hard pair.
    pub fn new(
        assignment_id: DomainAssignmentId,
        before: Option<AssignmentProtectionStateV1>,
        after: Option<AssignmentProtectionStateV1>,
        before_selected: bool,
        after_selected: bool,
    ) -> Result<Self, DomainContractError> {
        let value = Self {
            assignment_id,
            before,
            after,
            before_selected,
            after_selected,
            state_preserved: before == after,
            assignment_preserved: before_selected == after_selected,
        };
        value.validate()?;
        Ok(value)
    }

    /// Validates Boolean selection meaning and both derived preservation flags.
    ///
    /// # Errors
    /// Rejects inconsistent protection or selection evidence.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.before.is_none() && self.after.is_none() {
            return Err(explanation_error("empty assignment protection"));
        }
        for (state, selected) in [
            (self.before, self.before_selected),
            (self.after, self.after_selected),
        ] {
            if matches!(state, Some(AssignmentProtectionStateV1::Soft { weight: 0 }))
                || matches!(state, Some(AssignmentProtectionStateV1::Hard)) && !selected
            {
                return Err(explanation_error("assignment protection observation"));
            }
        }
        if self.state_preserved != (self.before == self.after)
            || self.assignment_preserved != (self.before_selected == self.after_selected)
        {
            return Err(explanation_error("assignment protection preservation"));
        }
        Ok(())
    }

    pub(super) fn validate_selection_values(
        &self,
        before: Option<&AssignmentValue>,
        after: Option<&AssignmentValue>,
    ) -> Result<(), DomainContractError> {
        for (value, observed) in [(before, self.before_selected), (after, self.after_selected)] {
            match value {
                Some(AssignmentValue::Boolean(selected)) if *selected == observed => {}
                None => {}
                Some(_) => {
                    return Err(explanation_error("assignment protection selection binding"));
                }
            }
        }
        Ok(())
    }
}

/// Inert labels retained from each entity's own immutable source, never from the current head.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComparisonEntityObservationV1 {
    /// Canonical affected entity.
    pub entity: DomainEntityRef,
    /// Label in the base source, absent if unavailable on that side.
    pub before_label: Option<String>,
    /// Label in the candidate source, absent if unavailable on that side.
    pub after_label: Option<String>,
}

impl ComparisonEntityObservationV1 {
    fn validate(&self) -> Result<(), DomainContractError> {
        if self.before_label.is_none() && self.after_label.is_none() {
            return Err(explanation_error("empty comparison entity observation"));
        }
        for label in [self.before_label.as_ref(), self.after_label.as_ref()]
            .into_iter()
            .flatten()
        {
            if label.is_empty() || label.len() > super::MAX_VERIFICATION_TEXT_BYTES {
                return Err(explanation_error("comparison entity label"));
            }
        }
        Ok(())
    }
}

/// Why aggregate score comparison was not established; semantic evidence remains available.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScoreComparisonUnavailableReasonV1 {
    /// Recorded policy or pack-established scoring population/semantics differ or are unknown.
    PolicyOrScopeMismatch,
    /// Ordered objective levels, directions or score-vector shape differ.
    ScoreShapeMismatch,
}

/// Exact score comparison only when source-owned scoring compatibility was established.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    deny_unknown_fields
)]
pub enum ScoreComparisonV2 {
    /// Full independently recomputed vectors, categories and candidate ordering.
    Comparable {
        /// Exact base score.
        base_score: ScoreVector,
        /// Exact candidate score.
        candidate_score: ScoreVector,
        /// Objective levels in recorded precedence order.
        score_levels: Vec<ScoreLevelComparisonV1>,
        /// Candidate ordering relative to the base.
        ordering: ComparisonOrdering,
    },
    /// Aggregate scores and ordering are deliberately absent.
    Unavailable {
        /// Closed reason, not an inferred utility comparison.
        reason: ScoreComparisonUnavailableReasonV1,
    },
}

/// Complete semantic/protection comparison with descriptive runs and an independently gated score.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionComparisonV2 {
    /// Must equal [`SOLUTION_COMPARISON_V2_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Exact base acceptance and immutable source.
    pub base: ComparisonBindingV1,
    /// Exact candidate acceptance and immutable source.
    pub candidate: ComparisonBindingV1,
    /// Canonical assignment changes.
    pub assignments: Vec<AssignmentComparisonV1>,
    /// Canonical Required-rule changes.
    pub rules: Vec<RuleComparisonV1>,
    /// Explicitly comparable or unavailable aggregate scores.
    pub score: ScoreComparisonV2,
    /// Canonical metric changes.
    pub metrics: Vec<MetricComparisonV1>,
    /// Complete canonical union of supplied source protections, including unchanged records.
    pub protections: Vec<AssignmentProtectionComparisonV1>,
    /// Both recorded terminal outcomes, not authenticated historical backend proof.
    pub runs: RunComparisonV2,
    /// Canonical union of changed generic entities and pack-projected affected parties.
    pub affected_entities: Vec<DomainEntityRef>,
    /// Canonical source-side labels for affected parties when available.
    pub entity_observations: Vec<ComparisonEntityObservationV1>,
    /// Lowercase BLAKE3 checksum excluding this field.
    pub checksum: String,
}

impl SolutionComparisonV2 {
    pub(super) fn seal(mut self) -> Result<Self, DomainContractError> {
        self.validate_payload()?;
        self.checksum = checksum_without_field(&self, "checksum")?;
        crate::ensure_portable_domain_contract_size(&self)?;
        Ok(self)
    }

    /// Strictly parses the bounded current comparison format.
    ///
    /// # Errors
    /// Rejects malformed JSON, unsupported versions or invalid complete comparison evidence.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates complete nested evidence and integrity, not local acceptance authority.
    ///
    /// # Errors
    /// Rejects noncanonical, contradictory, oversized or incorrectly bound evidence.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        crate::ensure_portable_domain_contract_size(self)
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        validate_version(self.schema_version, SOLUTION_COMPARISON_V2_SCHEMA_VERSION)?;
        self.base.validate()?;
        self.candidate.validate()?;
        validate_shared_comparison_bindings(&self.base, &self.candidate)?;
        validate_assignment_comparisons(&self.assignments)?;
        validate_rule_comparisons(&self.rules)?;
        validate_metric_comparisons(&self.metrics)?;
        if let ScoreComparisonV2::Comparable {
            base_score,
            candidate_score,
            score_levels,
            ordering,
        } = &self.score
        {
            validate_score_comparisons(base_score, candidate_score, score_levels, *ordering)?;
        }
        self.validate_protections_and_entities()?;
        self.runs.validate()?;
        for (run, binding) in [
            (&self.runs.base, &self.base),
            (&self.runs.candidate, &self.candidate),
        ] {
            if !matches!(&run.outcome, RunTerminalOutcomeV1::Accepted {
                solution_id, accepted_result_checksum, ..
            } if *solution_id == binding.accepted_result.solution_id
                && *accepted_result_checksum == binding.accepted_result.result_checksum)
            {
                return Err(explanation_error("comparison accepted run binding"));
            }
        }
        Ok(())
    }

    fn validate_protections_and_entities(&self) -> Result<(), DomainContractError> {
        if self.protections.len() > MAX_EXPLANATION_RECORDS
            || !strictly_sorted_unique_by(&self.protections, |value| &value.assignment_id)
        {
            return Err(DomainContractError::NonCanonicalExplanationCollection(
                "assignment protection comparisons",
            ));
        }
        for value in &self.protections {
            value.validate()?;
            if let Ok(index) = self
                .assignments
                .binary_search_by(|assignment| assignment.assignment_id().cmp(&value.assignment_id))
            {
                let (before, after) = match &self.assignments[index] {
                    AssignmentComparisonV1::Added { after } => (None, Some(&after.value)),
                    AssignmentComparisonV1::Removed { before } => (Some(&before.value), None),
                    AssignmentComparisonV1::Changed { before, after } => {
                        (Some(&before.value), Some(&after.value))
                    }
                };
                value.validate_selection_values(before, after)?;
            }
        }
        validate_bounded_strict(
            &self.affected_entities,
            MAX_EXPLANATION_RECORDS,
            "comparison affected entities",
        )?;
        let require_affected = |entity: &DomainEntityRef| {
            if self.affected_entities.binary_search(entity).is_err() {
                Err(explanation_error("missing comparison affected entity"))
            } else {
                Ok(())
            }
        };
        for assignment in &self.assignments {
            match assignment {
                AssignmentComparisonV1::Added { after } => require_affected(&after.entity)?,
                AssignmentComparisonV1::Removed { before } => require_affected(&before.entity)?,
                AssignmentComparisonV1::Changed { before, after } => {
                    require_affected(&before.entity)?;
                    if before.entity != after.entity {
                        require_affected(&after.entity)?;
                    }
                }
            }
        }
        for rule in &self.rules {
            for entity in rule
                .before
                .iter()
                .chain(&rule.after)
                .flat_map(|evaluation| &evaluation.affected_entities)
            {
                require_affected(entity)?;
            }
        }
        if self.entity_observations.len() > MAX_EXPLANATION_RECORDS
            || !strictly_sorted_unique_by(&self.entity_observations, |value| &value.entity)
        {
            return Err(DomainContractError::NonCanonicalExplanationCollection(
                "comparison entity observations",
            ));
        }
        for observation in &self.entity_observations {
            observation.validate()?;
            require_affected(&observation.entity)?;
        }
        Ok(())
    }
}

/// V1 non-causality marker retained by the V2 repair contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RepairCausalityV1 {
    /// Causality was not established by the retained deterministic comparison.
    NotEstablished,
}

/// Deterministic repair evidence without a causal claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepairEvidenceV2 {
    /// Complete comparison of the independently accepted base and repaired result.
    pub comparison: SolutionComparisonV2,
    /// Explicit non-causality claim.
    pub causality: RepairCausalityV1,
}

impl RepairEvidenceV2 {
    /// Validates the complete comparison.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.comparison.validate()
    }
}

fn validate_run_pair(
    run_input: &RunInput,
    run_manifest: &RunManifest,
    invariant: &'static str,
) -> Result<(), DomainContractError> {
    run_input.validate()?;
    run_manifest.validate()?;
    if run_input.run_id != run_manifest.run_id
        || run_input.checksum != run_manifest.run_input_checksum
    {
        return Err(explanation_error(invariant));
    }
    if let Some(observations) = &run_manifest.native_observations {
        observations.validate_input_bindings(run_input)?;
    }
    Ok(())
}

/// Terminal optimality/status evidence bound to exact run authority records.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptimalityStatusEvidence {
    /// Current writers record 2; absence is the exact historical unversioned status payload.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::deserialize_present"
    )]
    pub schema_version: Option<u32>,
    /// Exact immutable run input.
    pub run_input: RunInput,
    /// Exact terminal manifest bound to the run input.
    pub run_manifest: RunManifest,
    /// Accepted result only for an accepted outcome.
    pub result: Option<AcceptedResultRefV1>,
}

impl OptimalityStatusEvidence {
    /// Validates exact run binding and accepted-result partitioning.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        match self.schema_version {
            None if self.run_input.schema_version == 1 && self.run_manifest.schema_version == 1 => {
            }
            Some(2) => {}
            None => return Err(explanation_error("legacy optimality nested versions")),
            Some(version) => return Err(DomainContractError::UnsupportedVersion(version)),
        }
        validate_run_pair(
            &self.run_input,
            &self.run_manifest,
            "optimality run binding",
        )?;
        if let Some(result) = &self.result {
            result.validate()?;
        }
        match (&self.run_manifest.outcome, &self.result) {
            (
                RunTerminalOutcomeV1::Accepted {
                    solution_id,
                    accepted_result_checksum,
                    ..
                },
                Some(result),
            ) if solution_id == &result.solution_id
                && accepted_result_checksum == &result.result_checksum =>
            {
                Ok(())
            }
            (RunTerminalOutcomeV1::Accepted { .. }, _) | (_, Some(_)) => {
                Err(explanation_error("optimality accepted result"))
            }
            _ => Ok(()),
        }
    }
}

fn certainty_for_terminal_outcome(outcome: &RunTerminalOutcomeV1) -> ExplanationCertainty {
    match outcome {
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Optimal,
            ..
        } => ExplanationCertainty::BackendProof,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            ..
        } => ExplanationCertainty::IndependentlyVerified,
        _ => ExplanationCertainty::Deterministic,
    }
}

/// One safe localization message and canonical typed references.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceMessageV1 {
    /// Stable localization message key.
    pub message_key: String,
    /// Typed bounded parameters.
    pub parameters: BTreeMap<VerificationFactId, VerificationValue>,
    /// Canonical affected entity references.
    pub entities: Vec<DomainEntityRef>,
    /// Canonical required-rule references.
    pub rules: Vec<RuleId>,
    /// Canonical assignment references.
    pub assignments: Vec<DomainAssignmentId>,
    /// Canonical opaque evidence references; these never establish proof by themselves.
    pub evidence: Vec<DomainEvidenceId>,
}

impl EvidenceMessageV1 {
    /// Validates the safe message key, typed values, and every canonical bounded reference set.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_message_key(&self.message_key)?;
        validate_fact_map(&self.parameters)?;
        validate_bounded_strict(
            &self.entities,
            MAX_EVIDENCE_MESSAGE_REFERENCES,
            "message entities",
        )?;
        validate_bounded_strict(
            &self.rules,
            MAX_EVIDENCE_MESSAGE_REFERENCES,
            "message rules",
        )?;
        validate_bounded_strict(
            &self.assignments,
            MAX_EVIDENCE_MESSAGE_REFERENCES,
            "message assignments",
        )?;
        validate_bounded_strict(
            &self.evidence,
            MAX_EVIDENCE_MESSAGE_REFERENCES,
            "message evidence",
        )
    }
}

/// Exact seven-variant typed evidence payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum ExplanationEvidencePayloadV2 {
    /// Validation issue evidence.
    Validation {
        /// Exact issue.
        issue: ValidationIssueEvidenceV1,
    },
    /// Conflict or typed unavailable evidence.
    Infeasibility {
        /// Exact infeasibility evidence.
        infeasibility: InfeasibilityEvidenceV1,
        /// Exact immutable question, omitted for old unconditional payloads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        condition: Option<CounterfactualConditionV1>,
    },
    /// Independently verified assignment evidence.
    Assignment {
        /// Exact assignment evidence.
        assignment: AssignmentEvidenceV1,
    },
    /// Exact validated counterfactual result.
    Counterfactual {
        /// Counterfactual job result.
        result: Box<CounterfactualResult>,
    },
    /// Deterministic accepted-result comparison.
    SolutionDifference {
        /// Exact comparison.
        comparison: Box<SolutionComparisonV2>,
    },
    /// Deterministic repair comparison without a causal claim.
    Repair {
        /// Repair evidence.
        repair: Box<RepairEvidenceV2>,
    },
    /// Terminal run status/proof evidence.
    OptimalityStatus {
        /// Exact terminal evidence.
        status: Box<OptimalityStatusEvidence>,
    },
}

impl ExplanationEvidencePayloadV2 {
    /// Returns the exact evidence discriminant.
    #[must_use]
    pub const fn kind(&self) -> ExplanationKind {
        match self {
            Self::Validation { .. } => ExplanationKind::Validation,
            Self::Infeasibility { .. } => ExplanationKind::Infeasibility,
            Self::Assignment { .. } => ExplanationKind::Assignment,
            Self::Counterfactual { .. } => ExplanationKind::Counterfactual,
            Self::SolutionDifference { .. } => ExplanationKind::SolutionDifference,
            Self::Repair { .. } => ExplanationKind::Repair,
            Self::OptimalityStatus { .. } => ExplanationKind::OptimalityStatus,
        }
    }

    fn certainty(&self) -> ExplanationCertainty {
        match self {
            Self::Validation { .. } | Self::SolutionDifference { .. } | Self::Repair { .. } => {
                ExplanationCertainty::Deterministic
            }
            Self::Infeasibility { infeasibility, .. } => infeasibility.certainty(),
            Self::Assignment { .. } => ExplanationCertainty::IndependentlyVerified,
            Self::Counterfactual { result } => result.certainty(),
            Self::OptimalityStatus { status } if status.schema_version == Some(2) => {
                match status.run_manifest.outcome {
                    RunTerminalOutcomeV1::Accepted { .. } => {
                        ExplanationCertainty::IndependentlyVerified
                    }
                    _ => ExplanationCertainty::Deterministic,
                }
            }
            Self::OptimalityStatus { status } => {
                certainty_for_terminal_outcome(&status.run_manifest.outcome)
            }
        }
    }

    fn validate(&self) -> Result<(), DomainContractError> {
        match self {
            Self::Validation { issue } => issue.validate(),
            Self::Infeasibility {
                infeasibility,
                condition,
            } => {
                infeasibility.validate()?;
                if let Some(condition) = condition {
                    condition.validate()?;
                }
                Ok(())
            }
            Self::Assignment { assignment } => assignment.validate(),
            Self::Counterfactual { result } => result.validate(),
            Self::SolutionDifference { comparison } => comparison.validate(),
            Self::Repair { repair } => repair.validate(),
            Self::OptimalityStatus { status } => status.validate(),
        }
    }
}

/// Checksummed evidence whose certainty is exactly derived from its typed payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExplanationEvidenceV2 {
    /// Must equal [`EXPLANATION_EVIDENCE_V2_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Derived evidence strength.
    pub certainty: ExplanationCertainty,
    /// Exact typed payload.
    pub evidence: ExplanationEvidencePayloadV2,
    /// Lowercase BLAKE3 checksum excluding this field.
    pub checksum: String,
}

impl ExplanationEvidenceV2 {
    /// Validates a payload, derives certainty, and computes its checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(evidence: ExplanationEvidencePayloadV2) -> Result<Self, DomainContractError> {
        evidence.validate()?;
        let certainty = evidence.certainty();
        let mut value = Self {
            schema_version: EXPLANATION_EVIDENCE_V2_SCHEMA_VERSION,
            certainty,
            evidence,
            checksum: String::new(),
        };
        value.checksum = checksum_without_field(&value, "checksum")?;
        ensure_domain_contract_size(&value)?;
        Ok(value)
    }

    /// Parses under strict portable JSON limits and validates certainty and checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates schema, nested payload, derived certainty, and checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_version(self.schema_version, EXPLANATION_EVIDENCE_V2_SCHEMA_VERSION)?;
        self.evidence.validate()?;
        if self.certainty != self.evidence.certainty() {
            return Err(explanation_error("evidence certainty"));
        }
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    /// Returns the exact evidence kind.
    #[must_use]
    pub const fn kind(&self) -> ExplanationKind {
        self.evidence.kind()
    }
}

/// Pack rendering request. The redundant kind is validated against evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceRenderRequestV2 {
    /// Must equal [`EVIDENCE_RENDER_REQUEST_V2_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Expected evidence kind.
    pub kind: ExplanationKind,
    /// Exact checksummed evidence.
    pub evidence: ExplanationEvidenceV2,
}

impl EvidenceRenderRequestV2 {
    /// Creates a kind-bound rendering request.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(evidence: ExplanationEvidenceV2) -> Result<Self, DomainContractError> {
        evidence.validate()?;
        Ok(Self {
            schema_version: EVIDENCE_RENDER_REQUEST_V2_SCHEMA_VERSION,
            kind: evidence.kind(),
            evidence,
        })
    }

    /// Parses and validates the rendering request.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates schema, evidence, and kind agreement.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_version(
            self.schema_version,
            EVIDENCE_RENDER_REQUEST_V2_SCHEMA_VERSION,
        )?;
        self.evidence.validate()?;
        if self.kind != self.evidence.kind() {
            return Err(explanation_error("render request kind"));
        }
        Ok(())
    }
}

/// Data-only rendering result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceRenderResultV1 {
    /// Must equal [`EVIDENCE_RENDER_RESULT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Rendered evidence kind.
    pub kind: ExplanationKind,
    /// Checksum of the exact evidence rendered.
    pub evidence_checksum: String,
    /// Bounded localization messages.
    pub messages: Vec<EvidenceMessageV1>,
}

impl EvidenceRenderResultV1 {
    /// Creates a result bound to the validated request kind.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(
        request: &EvidenceRenderRequestV2,
        messages: Vec<EvidenceMessageV1>,
    ) -> Result<Self, DomainContractError> {
        request.validate()?;
        let value = Self {
            schema_version: EVIDENCE_RENDER_RESULT_SCHEMA_VERSION,
            kind: request.kind,
            evidence_checksum: request.evidence.checksum.clone(),
            messages,
        };
        value.validate()?;
        ensure_domain_contract_size(&value)?;
        Ok(value)
    }

    /// Parses and validates data-only messages.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates schema, bounds, and every safe message.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_version(self.schema_version, EVIDENCE_RENDER_RESULT_SCHEMA_VERSION)?;
        validate_hash(&self.evidence_checksum)?;
        if self.messages.len() > MAX_EVIDENCE_MESSAGES {
            return Err(DomainContractError::LimitExceeded("evidence messages"));
        }
        for message in &self.messages {
            message.validate()?;
        }
        Ok(())
    }
}

/// Checksummed pairing of exact evidence with its matching data-only rendering.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExplanationResultV2 {
    /// Must equal [`EXPLANATION_RESULT_V2_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Exact checksummed evidence.
    pub evidence: ExplanationEvidenceV2,
    /// Matching rendered messages.
    pub rendered: EvidenceRenderResultV1,
    /// Lowercase BLAKE3 checksum excluding this field.
    pub checksum: String,
}

impl ExplanationResultV2 {
    /// Creates and checksums a matching evidence/rendering pair.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(
        evidence: ExplanationEvidenceV2,
        rendered: EvidenceRenderResultV1,
    ) -> Result<Self, DomainContractError> {
        let mut value = Self {
            schema_version: EXPLANATION_RESULT_V2_SCHEMA_VERSION,
            evidence,
            rendered,
            checksum: String::new(),
        };
        value.validate_payload()?;
        value.checksum = checksum_without_field(&value, "checksum")?;
        ensure_domain_contract_size(&value)?;
        Ok(value)
    }

    /// Parses and validates a complete explanation result.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates nested contracts, kind agreement, and checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        validate_version(self.schema_version, EXPLANATION_RESULT_V2_SCHEMA_VERSION)?;
        self.evidence.validate()?;
        self.rendered.validate()?;
        if self.evidence.kind() != self.rendered.kind {
            return Err(explanation_error("explanation result kind"));
        }
        if self.evidence.checksum != self.rendered.evidence_checksum {
            return Err(explanation_error("explanation result evidence binding"));
        }
        Ok(())
    }
}

/// Exact three-condition counterfactual vocabulary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    deny_unknown_fields
)]
pub enum CounterfactualConditionPayloadV1 {
    /// Require an assignment to equal a value.
    ForceAssignmentValue {
        /// Assignment identity.
        assignment_id: DomainAssignmentId,
        /// Required value.
        value: AssignmentValue,
    },
    /// Require an assignment not to equal a value.
    ForbidAssignmentValue {
        /// Assignment identity.
        assignment_id: DomainAssignmentId,
        /// Forbidden value.
        value: AssignmentValue,
    },
    /// Require the target assignment to be true while the replaced assignment is false.
    ReplaceAssignment {
        /// Assignment identity selected by the counterfactual.
        target_assignment_id: DomainAssignmentId,
        /// Existing assignment identity that must no longer be selected.
        replaced_assignment_id: DomainAssignmentId,
    },
}

impl CounterfactualConditionPayloadV1 {
    fn validate(&self) -> Result<(), DomainContractError> {
        match self {
            Self::ForceAssignmentValue { value, .. }
            | Self::ForbidAssignmentValue { value, .. } => {
                if let AssignmentValue::Interval(interval) = value {
                    interval.validate()?;
                }
            }
            Self::ReplaceAssignment {
                target_assignment_id,
                replaced_assignment_id,
            } => {
                // DomainAssignmentId construction/deserialization already enforces the canonical
                // namespace grammar; retain an explicit non-empty check at this V1 boundary.
                if target_assignment_id.as_str().is_empty()
                    || replaced_assignment_id.as_str().is_empty()
                {
                    return Err(explanation_error(
                        "counterfactual replacement assignment ids",
                    ));
                }
                if target_assignment_id == replaced_assignment_id {
                    return Err(explanation_error(
                        "counterfactual replacement assignments must be distinct",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Returns whether a normalized solution satisfies the exact counterfactual condition.
///
/// A condition never matches when its target assignment is absent. Force requires equality;
/// forbid requires inequality. Replace requires both exact assignments, with the target set to
/// `true` and the replaced assignment set to `false`.
#[must_use]
pub fn counterfactual_condition_satisfied(
    condition: &CounterfactualConditionV1,
    solution: &NormalizedSolution,
) -> bool {
    let assignment = solution
        .assignments
        .iter()
        .find(|assignment| &assignment.id == condition_assignment_id(&condition.condition));
    let Some(assignment) = assignment else {
        return false;
    };
    match &condition.condition {
        CounterfactualConditionPayloadV1::ForceAssignmentValue { value, .. } => {
            &assignment.value == value
        }
        CounterfactualConditionPayloadV1::ForbidAssignmentValue { value, .. } => {
            &assignment.value != value
        }
        CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: _,
            replaced_assignment_id,
        } => {
            matches!(&assignment.value, AssignmentValue::Boolean(true))
                && solution
                    .assignments
                    .iter()
                    .find(|assignment| &assignment.id == replaced_assignment_id)
                    .is_some_and(|assignment| {
                        matches!(&assignment.value, AssignmentValue::Boolean(false))
                    })
        }
    }
}

fn condition_assignment_id(condition: &CounterfactualConditionPayloadV1) -> &DomainAssignmentId {
    match condition {
        CounterfactualConditionPayloadV1::ForceAssignmentValue { assignment_id, .. }
        | CounterfactualConditionPayloadV1::ForbidAssignmentValue { assignment_id, .. } => {
            assignment_id
        }
        CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id,
            ..
        } => target_assignment_id,
    }
}

/// Immutable checksummed temporary diagnostic condition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CounterfactualConditionV1 {
    /// Must equal [`COUNTERFACTUAL_CONDITION_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Exact force/forbid/replace condition.
    pub condition: CounterfactualConditionPayloadV1,
    /// Lowercase BLAKE3 checksum excluding this field.
    pub checksum: String,
}

impl CounterfactualConditionV1 {
    /// Validates and checksums a condition.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(condition: CounterfactualConditionPayloadV1) -> Result<Self, DomainContractError> {
        condition.validate()?;
        let mut value = Self {
            schema_version: COUNTERFACTUAL_CONDITION_SCHEMA_VERSION,
            condition,
            checksum: String::new(),
        };
        value.checksum = checksum_without_field(&value, "checksum")?;
        ensure_domain_contract_size(&value)?;
        Ok(value)
    }

    /// Parses and validates a strict condition.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates schema, typed value, and checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_version(self.schema_version, COUNTERFACTUAL_CONDITION_SCHEMA_VERSION)?;
        self.condition.validate()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }
}

/// Checksummed proof that a derived model is the validated temporary-condition compilation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CounterfactualCompilationBindingV1 {
    /// Must equal [`COUNTERFACTUAL_COMPILATION_BINDING_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Canonical base planning-model hash.
    pub base_model_hash: String,
    /// Exact condition checksum recorded by compilation metadata.
    pub condition_checksum: String,
    /// Canonical derived planning-model hash.
    pub derived_model_hash: String,
    /// Unchanged objective-policy hash.
    pub objective_policy_hash: String,
    /// Lowercase BLAKE3 checksum excluding this field.
    pub checksum: String,
}

impl CounterfactualCompilationBindingV1 {
    /// Creates a checksummed binding from independently validated model hashes.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(
        base_model_hash: String,
        condition_checksum: String,
        derived_model_hash: String,
        objective_policy_hash: String,
    ) -> Result<Self, DomainContractError> {
        let mut value = Self {
            schema_version: COUNTERFACTUAL_COMPILATION_BINDING_SCHEMA_VERSION,
            base_model_hash,
            condition_checksum,
            derived_model_hash,
            objective_policy_hash,
            checksum: String::new(),
        };
        value.validate_payload()?;
        value.checksum = checksum_without_field(&value, "checksum")?;
        Ok(value)
    }

    /// Validates schema, every hash, and checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        validate_version(
            self.schema_version,
            COUNTERFACTUAL_COMPILATION_BINDING_SCHEMA_VERSION,
        )?;
        for hash in [
            &self.base_model_hash,
            &self.condition_checksum,
            &self.derived_model_hash,
            &self.objective_policy_hash,
        ] {
            validate_hash(hash)?;
        }
        if self.base_model_hash == self.derived_model_hash {
            return Err(explanation_error("unchanged counterfactual model"));
        }
        Ok(())
    }
}

/// Canonical idempotency preimage for a counterfactual request.
///
/// It deliberately excludes job/request IDs and every timestamp.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CounterfactualRequestSemanticsV1 {
    /// Must equal [`COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Immutable scenario identity.
    pub scenario_id: ScenarioId,
    /// Exact scenario revision.
    pub scenario_revision: u64,
    /// Immutable snapshot identity.
    pub snapshot_id: ScenarioSnapshotId,
    /// Canonical snapshot document hash.
    pub snapshot_document_hash: String,
    /// Base accepted result.
    pub base: AcceptedResultRefV1,
    /// Base solve run identity.
    pub base_run_id: SolveRunId,
    /// Base run-input checksum.
    pub base_run_input_checksum: String,
    /// Base canonical planning-model hash.
    pub base_model_hash: String,
    /// Base objective-policy hash.
    pub objective_policy_hash: String,
    /// Exact counterfactual condition checksum.
    pub condition_checksum: String,
    /// Positive total budget shared by compilation and solve work, capped at
    /// [`COUNTERFACTUAL_TOTAL_BUDGET_MAX_MILLISECONDS_V1`].
    pub total_budget_milliseconds: DurationMillis,
}

impl CounterfactualRequestSemanticsV1 {
    /// Validates all immutable bindings and the positive total budget.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_version(
            self.schema_version,
            COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION,
        )?;
        validate_revision(self.scenario_revision)?;
        self.base.validate()?;
        for hash in [
            &self.snapshot_document_hash,
            &self.base_run_input_checksum,
            &self.base_model_hash,
            &self.objective_policy_hash,
            &self.condition_checksum,
        ] {
            validate_hash(hash)?;
        }
        if self.total_budget_milliseconds == DurationMillis::ZERO {
            return Err(explanation_error("zero counterfactual budget"));
        }
        if self.total_budget_milliseconds.value() > COUNTERFACTUAL_TOTAL_BUDGET_MAX_MILLISECONDS_V1
        {
            return Err(explanation_error("counterfactual budget exceeds maximum"));
        }
        Ok(())
    }

    /// Computes the deterministic request hash, excluding IDs and timestamps by construction.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn canonical_hash(&self) -> Result<String, DomainContractError> {
        self.validate()?;
        canonical_hash(self)
    }
}

/// Counterfactual job creation request with correlation identities outside the semantic hash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CounterfactualJobRequestV1 {
    /// Must equal [`COUNTERFACTUAL_JOB_REQUEST_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Execution identity, excluded from `request_hash`.
    pub job_id: CounterfactualJobId,
    /// Correlation identity, excluded from `request_hash`.
    pub request_id: RequestId,
    /// Canonical semantic preimage.
    pub semantics: CounterfactualRequestSemanticsV1,
    /// Exact typed condition persisted for compilation.
    pub condition: CounterfactualConditionV1,
    /// Hash of `semantics` only.
    pub request_hash: String,
    /// Creation timestamp, excluded from `request_hash`.
    pub created_at: Rfc3339Timestamp,
}

impl CounterfactualJobRequestV1 {
    /// Derives a request hash from immutable semantics only.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(
        job_id: CounterfactualJobId,
        request_id: RequestId,
        semantics: CounterfactualRequestSemanticsV1,
        condition: CounterfactualConditionV1,
        created_at: Rfc3339Timestamp,
    ) -> Result<Self, DomainContractError> {
        let request_hash = semantics.canonical_hash()?;
        let value = Self {
            schema_version: COUNTERFACTUAL_JOB_REQUEST_SCHEMA_VERSION,
            job_id,
            request_id,
            semantics,
            condition,
            request_hash,
            created_at,
        };
        value.validate()?;
        ensure_domain_contract_size(&value)?;
        Ok(value)
    }

    /// Parses and validates a strict job request.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates schema, typed condition/semantics binding, and the derived request hash.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_version(
            self.schema_version,
            COUNTERFACTUAL_JOB_REQUEST_SCHEMA_VERSION,
        )?;
        self.semantics.validate()?;
        self.condition.validate()?;
        if self.condition.checksum != self.semantics.condition_checksum {
            return Err(explanation_error(
                "counterfactual request condition binding",
            ));
        }
        validate_hash(&self.request_hash)?;
        if self.semantics.canonical_hash()? != self.request_hash {
            return Err(DomainContractError::RequestHashMismatch);
        }
        Ok(())
    }
}

/// Exact counterfactual job lifecycle states.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CounterfactualJobState {
    /// Waiting for bounded execution capacity.
    Queued,
    /// Compilation or solving has started.
    Running,
    /// A valid counterfactual result was recorded.
    Completed,
    /// A typed safe failure was recorded.
    Failed,
    /// Cancellation finished without a proof result.
    Cancelled,
    /// Started work was interrupted without a proof result.
    Interrupted,
}

/// Typed counterfactual failure classification. No backend text or arbitrary payload is retained.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CounterfactualFailureKind {
    /// The scenario revision became stale before completion.
    StaleRevision,
    /// The diagnostic budget expired before a valid derived run could be created.
    ///
    /// This is an inconclusive "not distinguished within the budget" outcome,
    /// not a compiler defect, impossibility proof, or proof of equality.
    BudgetExhausted,
    /// The selected backend was unavailable.
    BackendUnavailable,
    /// The backend failed without a trusted result.
    BackendFailed,
    /// Compilation or backend validation rejected the model.
    InvalidModel,
    /// A candidate failed projection or independent verification.
    InvalidCandidate,
    /// The pack could not compile the requested typed condition.
    CompilationFailed,
    /// A cross-contract binding or checksum was invalid.
    InvalidBinding,
}

/// Safe typed job error retained only for a failed job.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CounterfactualJobErrorV1 {
    /// Exact failure category.
    pub kind: CounterfactualFailureKind,
}

/// A successful counterfactual conclusion.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type", deny_unknown_fields)]
pub enum CounterfactualConclusionV1 {
    /// The exact condition-bound derived model was proven infeasible.
    ProvenImpossible,
    /// An independently accepted alternative was verified and compared exactly.
    VerifiedAlternative {
        /// Compact reference to the accepted alternative.
        alternative: AcceptedResultRefV1,
        /// Deterministic same-base counterfactual comparison.
        comparison: Box<SolutionComparisonV1>,
        /// Exact alternative ordering; strict improvement is represented as `better`.
        ordering: ComparisonOrdering,
    },
    /// No verified candidate or independently validated impossibility proof was available.
    ///
    /// This includes an untrusted backend infeasibility status as well as a limit terminal outcome.
    NotDistinguishedWithinBudget,
}

/// Checksummed result bound to the exact request, base authority, derived run, and compilation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CounterfactualResult {
    /// Must equal [`COUNTERFACTUAL_RESULT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Exact typed job request and condition.
    pub request: CounterfactualJobRequestV1,
    /// Exact immutable input for the accepted base run.
    pub base_run_input: RunInput,
    /// Exact accepted manifest for the base run.
    pub base_run_manifest: RunManifest,
    /// Validated derived-model compilation binding.
    pub compilation: CounterfactualCompilationBindingV1,
    /// Exact condition-bound derived run input.
    pub run_input: RunInput,
    /// Exact terminal manifest bound to the derived run input.
    pub run_manifest: RunManifest,
    /// One truthful successful conclusion.
    pub conclusion: CounterfactualConclusionV1,
    /// Lowercase BLAKE3 checksum excluding this field.
    pub checksum: String,
}

impl CounterfactualResult {
    /// Creates and checksums a result from exact request, base, and derived-run authority.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(
        request: CounterfactualJobRequestV1,
        base_run_input: RunInput,
        base_run_manifest: RunManifest,
        compilation: CounterfactualCompilationBindingV1,
        run_input: RunInput,
        run_manifest: RunManifest,
        conclusion: CounterfactualConclusionV1,
    ) -> Result<Self, DomainContractError> {
        let mut value = Self {
            schema_version: COUNTERFACTUAL_RESULT_SCHEMA_VERSION,
            request,
            base_run_input,
            base_run_manifest,
            compilation,
            run_input,
            run_manifest,
            conclusion,
            checksum: String::new(),
        };
        value.validate_payload()?;
        value.checksum = checksum_without_field(&value, "checksum")?;
        ensure_domain_contract_size(&value)?;
        Ok(value)
    }

    /// Parses and validates a complete counterfactual result.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates every nested authority contract, cross-binding, conclusion, and checksum.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        if !matches!(
            self.schema_version,
            1 | COUNTERFACTUAL_RESULT_SCHEMA_VERSION
        ) {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        if self.schema_version == 1
            && (self.base_run_input.schema_version != 1
                || self.run_input.schema_version != 1
                || self.base_run_manifest.schema_version != 1
                || self.run_manifest.schema_version != 1)
        {
            return Err(explanation_error("legacy counterfactual nested versions"));
        }
        self.request.validate()?;
        self.compilation.validate()?;
        self.validate_base_run()?;
        self.validate_derived_run()?;
        self.validate_conclusion()
    }

    fn validate_base_run(&self) -> Result<(), DomainContractError> {
        validate_run_pair(
            &self.base_run_input,
            &self.base_run_manifest,
            "counterfactual base run binding",
        )?;
        let semantics = &self.request.semantics;
        if self.base_run_input.run_id != semantics.base_run_id
            || self.base_run_input.checksum != semantics.base_run_input_checksum
            || self.base_run_input.scenario_id != semantics.scenario_id
            || self.base_run_input.scenario_revision != semantics.scenario_revision
            || self.base_run_input.snapshot_id != semantics.snapshot_id
            || self.base_run_input.snapshot_document_hash != semantics.snapshot_document_hash
            || self.base_run_input.model_hash != semantics.base_model_hash
            || self.base_run_input.objective_policy_hash != semantics.objective_policy_hash
            || self.base_run_input.temporary_condition_hash.is_some()
        {
            return Err(explanation_error("counterfactual base run binding"));
        }
        match &self.base_run_manifest.outcome {
            RunTerminalOutcomeV1::Accepted {
                solution_id,
                accepted_result_checksum,
                ..
            } if solution_id == &semantics.base.solution_id
                && accepted_result_checksum == &semantics.base.result_checksum =>
            {
                Ok(())
            }
            _ => Err(explanation_error("counterfactual base accepted result")),
        }
    }

    fn validate_derived_run(&self) -> Result<(), DomainContractError> {
        validate_run_pair(
            &self.run_input,
            &self.run_manifest,
            "counterfactual derived run binding",
        )?;
        let semantics = &self.request.semantics;
        if self.run_input.scenario_id != semantics.scenario_id
            || self.run_input.scenario_revision != semantics.scenario_revision
            || self.run_input.snapshot_id != semantics.snapshot_id
            || self.run_input.snapshot_document_hash != semantics.snapshot_document_hash
            || self.run_input.run_id == self.base_run_input.run_id
            || self.run_input.pack_id != self.base_run_input.pack_id
            || self.run_input.pack_schema_version != self.base_run_input.pack_schema_version
            || self.run_input.objective_context != self.base_run_input.objective_context
            || self.run_input.planning_ir_schema_version
                != self.base_run_input.planning_ir_schema_version
            || self.run_input.compiler_version != self.base_run_input.compiler_version
            || self.run_input.model_hash != self.compilation.derived_model_hash
            || self.run_input.objective_policy_hash != self.compilation.objective_policy_hash
            || self.run_input.temporary_condition_hash.as_ref()
                != Some(&self.compilation.condition_checksum)
            || self.compilation.base_model_hash != semantics.base_model_hash
            || self.compilation.base_model_hash != self.base_run_input.model_hash
            || self.compilation.condition_checksum != semantics.condition_checksum
            || self.compilation.condition_checksum != self.request.condition.checksum
            || self.compilation.objective_policy_hash != semantics.objective_policy_hash
            || self.run_input.solve_options.time_limit_milliseconds
                > semantics.total_budget_milliseconds
        {
            return Err(explanation_error("counterfactual derived run binding"));
        }
        Ok(())
    }
    fn validate_conclusion(&self) -> Result<(), DomainContractError> {
        match (&self.conclusion, &self.run_manifest.outcome) {
            (
                CounterfactualConclusionV1::ProvenImpossible,
                RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Infeasible,
                },
            )
            | (
                CounterfactualConclusionV1::NotDistinguishedWithinBudget,
                RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Infeasible | SolveStatus::NoSolutionWithinLimit,
                },
            ) => Ok(()),
            (
                CounterfactualConclusionV1::VerifiedAlternative {
                    alternative,
                    comparison,
                    ordering,
                },
                RunTerminalOutcomeV1::Accepted {
                    solution_id,
                    accepted_result_checksum,
                    ..
                },
            ) => {
                alternative.validate()?;
                comparison.validate()?;
                let semantics = &self.request.semantics;
                if alternative.solution_id != *solution_id
                    || alternative.result_checksum != *accepted_result_checksum
                    || comparison.base.accepted_result != semantics.base
                    || comparison.candidate.accepted_result != *alternative
                    || comparison.base.pack_id != self.base_run_input.pack_id
                    || comparison.candidate.pack_id != self.run_input.pack_id
                    || comparison.base.scenario_id != semantics.scenario_id
                    || comparison.candidate.scenario_id != semantics.scenario_id
                    || comparison.base.scenario_revision != semantics.scenario_revision
                    || comparison.candidate.scenario_revision != semantics.scenario_revision
                    || comparison.base.document_hash != semantics.snapshot_document_hash
                    || comparison.candidate.document_hash != semantics.snapshot_document_hash
                    || comparison.ordering != *ordering
                {
                    return Err(explanation_error("counterfactual alternative binding"));
                }
                Ok(())
            }
            _ => Err(explanation_error("counterfactual conclusion outcome")),
        }
    }

    /// Returns the exact derived certainty for explanation envelopes.
    #[must_use]
    pub const fn certainty(&self) -> ExplanationCertainty {
        match &self.conclusion {
            CounterfactualConclusionV1::ProvenImpossible => ExplanationCertainty::BackendProof,
            CounterfactualConclusionV1::VerifiedAlternative { .. } => {
                ExplanationCertainty::IndependentlyVerified
            }
            CounterfactualConclusionV1::NotDistinguishedWithinBudget => {
                ExplanationCertainty::Inconclusive
            }
        }
    }
}

/// Persistable lifecycle record with an exact state/timestamp/result/error matrix.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CounterfactualJobRecordV1 {
    /// Must equal [`COUNTERFACTUAL_JOB_RECORD_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Immutable creation request.
    pub request: CounterfactualJobRequestV1,
    /// Current exact lifecycle state.
    pub state: CounterfactualJobState,
    /// Start timestamp when work began.
    pub started_at: Option<Rfc3339Timestamp>,
    /// Terminal timestamp.
    pub finished_at: Option<Rfc3339Timestamp>,
    /// Cancellation correlation identity, paired with `cancel_requested_at`.
    pub cancel_request_id: Option<RequestId>,
    /// Cancellation request timestamp, paired with `cancel_request_id`.
    pub cancel_requested_at: Option<Rfc3339Timestamp>,
    /// Present exactly for `completed`.
    pub result: Option<CounterfactualResult>,
    /// Present exactly for `failed`.
    pub error: Option<CounterfactualJobErrorV1>,
    /// Optional local terminal custody reference, omitted by old lifecycle records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_authority: Option<CounterfactualTerminalAuthorityV1>,
}

impl CounterfactualJobRecordV1 {
    /// Creates a lifecycle record and validates the complete matrix.
    #[allow(clippy::too_many_arguments)]
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn new(
        request: CounterfactualJobRequestV1,
        state: CounterfactualJobState,
        started_at: Option<Rfc3339Timestamp>,
        finished_at: Option<Rfc3339Timestamp>,
        cancel_request_id: Option<RequestId>,
        cancel_requested_at: Option<Rfc3339Timestamp>,
        result: Option<CounterfactualResult>,
        error: Option<CounterfactualJobErrorV1>,
        terminal_authority: Option<CounterfactualTerminalAuthorityV1>,
    ) -> Result<Self, DomainContractError> {
        let value = Self {
            schema_version: COUNTERFACTUAL_JOB_RECORD_SCHEMA_VERSION,
            request,
            state,
            started_at,
            finished_at,
            cancel_request_id,
            cancel_requested_at,
            result,
            error,
            terminal_authority,
        };
        value.validate()?;
        ensure_domain_contract_size(&value)?;
        Ok(value)
    }

    /// Parses and validates a strict lifecycle record.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates request binding, paired cancellation data, monotone timestamps, and state matrix.
    ///
    /// # Errors
    /// Returns [`DomainContractError`] when a documented invariant is violated.
    #[allow(clippy::too_many_lines)]
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_version(
            self.schema_version,
            COUNTERFACTUAL_JOB_RECORD_SCHEMA_VERSION,
        )?;
        self.request.validate()?;
        if self.cancel_request_id.is_some() != self.cancel_requested_at.is_some() {
            return Err(explanation_error("counterfactual cancellation pair"));
        }
        let marked_alarm_authority = matches!(
            &self.terminal_authority,
            Some(CounterfactualTerminalAuthorityV1::VerificationAlarm {
                clock_failure: Some(_),
                ..
            })
        );
        if self
            .started_at
            .is_some_and(|started| started < self.request.created_at)
            || self.finished_at.is_some_and(|finished| {
                !marked_alarm_authority
                    && (finished < self.request.created_at
                        || self.started_at.is_some_and(|started| finished < started))
            })
            || self.cancel_requested_at.is_some_and(|cancelled| {
                cancelled < self.request.created_at
                    || (self
                        .finished_at
                        .is_some_and(|finished| cancelled > finished)
                        && !marked_alarm_authority)
            })
        {
            return Err(explanation_error("counterfactual timestamps"));
        }
        let has_cancel_request = self.cancel_request_id.is_some();
        let timely_authority = matches!(
            self.terminal_authority,
            Some(CounterfactualTerminalAuthorityV1::TimelyInfeasibility { .. })
        );
        let alarm_authority = matches!(
            self.terminal_authority,
            Some(CounterfactualTerminalAuthorityV1::VerificationAlarm { .. })
        );
        let valid_shape = match self.state {
            CounterfactualJobState::Queued => {
                self.started_at.is_none()
                    && self.finished_at.is_none()
                    && self.result.is_none()
                    && self.error.is_none()
                    && !has_cancel_request
            }
            CounterfactualJobState::Running => {
                self.started_at.is_some()
                    && self.finished_at.is_none()
                    && self.result.is_none()
                    && self.error.is_none()
            }
            CounterfactualJobState::Completed => {
                self.started_at.is_some()
                    && self.finished_at.is_some()
                    && self.result.is_some()
                    && self.error.is_none()
                    && (!has_cancel_request || timely_authority)
            }
            CounterfactualJobState::Failed => {
                self.finished_at.is_some()
                    && self.result.is_none()
                    && self.error.is_some()
                    && (!has_cancel_request || alarm_authority)
            }
            CounterfactualJobState::Cancelled => {
                self.finished_at.is_some()
                    && self.result.is_none()
                    && self.error.is_none()
                    && has_cancel_request
            }
            CounterfactualJobState::Interrupted => {
                self.started_at.is_some()
                    && self.finished_at.is_some()
                    && self.result.is_none()
                    && self.error.is_none()
                    && !has_cancel_request
            }
        };
        if !valid_shape {
            return Err(explanation_error("counterfactual job state matrix"));
        }
        if let Some(result) = &self.result {
            result.validate()?;
            if result.request != self.request {
                return Err(explanation_error("counterfactual job result binding"));
            }
        }
        if let Some(authority) = &self.terminal_authority {
            authority.validate()?;
            match (authority, self.state) {
                (
                    CounterfactualTerminalAuthorityV1::TimelyInfeasibility {
                        run_id,
                        run_manifest_checksum,
                        ..
                    },
                    CounterfactualJobState::Completed,
                ) => {
                    let result = self
                        .result
                        .as_ref()
                        .ok_or(explanation_error("missing terminal result authority"))?;
                    if *run_id != result.run_input.run_id
                        || run_manifest_checksum != &result.run_manifest.checksum
                        || result.run_manifest.outcome
                            != (RunTerminalOutcomeV1::NoResult {
                                status: SolveStatus::Infeasible,
                            })
                    {
                        return Err(explanation_error(
                            "counterfactual timely authority reference",
                        ));
                    }
                }
                (
                    CounterfactualTerminalAuthorityV1::VerificationAlarm { .. },
                    CounterfactualJobState::Failed,
                ) if self.error.is_some_and(|error| {
                    error.kind == CounterfactualFailureKind::InvalidCandidate
                }) => {}
                _ => return Err(explanation_error("counterfactual terminal authority state")),
            }
        }
        Ok(())
    }
}
