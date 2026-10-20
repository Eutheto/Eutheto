//! Compact diagnostic proof custody. Structural validity is not local solver authority.

use crate::{
    AssumptionGroupId, ConflictGroupV1, ConflictUnavailableReason, CounterfactualConditionV1,
    DiagnosticAssumptionVariableId, DomainContractError, InfeasibilityEvidenceV1,
    RunClockFailureV1, RunInput, RunManifest, RunTerminalOutcomeV1, checksum_without_field,
    validate_hash, validate_revision,
};
use eutheto_types::{
    DurationMillis, PortableJsonLimits, Rfc3339Timestamp, RuleId, ScenarioId, SolveRunId,
    SolveStatus, validate_nonsecret_portable_json_bytes,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Complete mapped groups per diagnostic, not a truncation threshold.
pub const MAX_DIAGNOSTIC_GROUPS: usize = 512;
/// Encoded retained evidence ceiling, checked before checksum allocation.
pub const MAX_RUN_INFEASIBILITY_EVIDENCE_BYTES: usize = 1024 * 1024;
/// Shared optional diagnostic phase ceiling, in milliseconds.
pub const MAX_INFEASIBILITY_DIAGNOSTIC_MILLISECONDS: u64 = 10_000;
/// Each inherited native dispatch ceiling, in milliseconds.
pub const MAX_INFEASIBILITY_DISPATCH_MILLISECONDS: u64 = 2_000;
/// Deletion solves permitted after the initial proof.
pub const MAX_INFEASIBILITY_DELETION_TRIALS: u32 = 32;

/// Actual native execution profile, never inferred from a returned empty core.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticNativeProfileV1 {
    /// Objective-free, Exact(1), nonempty assumptions, native presolve disabled.
    NonemptyAssumptionSatisfaction,
    /// Objective-free, Exact(1), no assumptions, unchanged native presolve defaults.
    ZeroAssumptionSatisfaction,
}

/// One exact submitted signed literal and its complete source group.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticSubmittedAssumptionV1 {
    /// Exact Boolean planning identity.
    pub variable_id: DiagnosticAssumptionVariableId,
    /// Submitted polarity, not normalized from a backend response.
    pub value: bool,
    /// Source-authored group identity.
    pub group_id: AssumptionGroupId,
    /// Exactly one genuine Required rule.
    pub required_rule_id: RuleId,
}

/// Exact admitted native request identity retained instead of worker frames.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticSolveBindingV1 {
    /// Canonical diagnostic model hash, including this submitted assumption set.
    pub model_hash: String,
    /// Exact routed request fingerprint.
    pub routed_fingerprint: String,
    /// Canonical ascending Boolean identities; no duplicate groups or rules.
    pub submitted_assumptions: Vec<DiagnosticSubmittedAssumptionV1>,
    /// Native presolve policy actually used.
    pub native_profile: DiagnosticNativeProfileV1,
    /// Digest of the actual worker parameters.
    pub parameter_digest: String,
    /// Approved assembly manifest SHA256 observed at execution.
    pub worker_manifest_sha256: String,
    /// Actual inherited duration sent to this native request.
    pub time_limit_milliseconds: DurationMillis,
}

impl DiagnosticSolveBindingV1 {
    /// Checks bounded complete request/profile identity without granting execution authority.
    ///
    /// # Errors
    /// Rejects malformed hashes, duplicate identities, or a mismatched profile/budget.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        for hash in [
            &self.model_hash,
            &self.routed_fingerprint,
            &self.parameter_digest,
            &self.worker_manifest_sha256,
        ] {
            validate_hash(hash)?;
        }
        if self.time_limit_milliseconds.value() == 0
            || self.time_limit_milliseconds.value() > MAX_INFEASIBILITY_DISPATCH_MILLISECONDS
            || self.submitted_assumptions.len() > MAX_DIAGNOSTIC_GROUPS
            || self
                .submitted_assumptions
                .windows(2)
                .any(|pair| pair[0].variable_id >= pair[1].variable_id)
            || matches!(
                self.native_profile,
                DiagnosticNativeProfileV1::ZeroAssumptionSatisfaction
            ) != self.submitted_assumptions.is_empty()
        {
            return Err(invalid("diagnostic request profile"));
        }
        let mut groups = BTreeSet::new();
        let mut rules = BTreeSet::new();
        for assumption in &self.submitted_assumptions {
            if !groups.insert(&assumption.group_id) || !rules.insert(assumption.required_rule_id) {
                return Err(invalid("duplicate diagnostic source group"));
            }
        }
        Ok(())
    }
}

/// A locally admitted native infeasibility observation, measured on the original parent timeline.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticInfeasibilityObservationV1 {
    /// Exact validated request.
    pub binding: DiagnosticSolveBindingV1,
    /// Parent elapsed time at dispatch, without restarting its clock.
    pub dispatched_after_milliseconds: DurationMillis,
    /// Parent elapsed time after complete output/core admission.
    pub observed_after_milliseconds: DurationMillis,
}

impl DiagnosticInfeasibilityObservationV1 {
    fn validate(
        &self,
        diagnostic_start: DurationMillis,
        parent_limit: Option<DurationMillis>,
    ) -> Result<(), DomainContractError> {
        self.binding.validate()?;
        let Some(dispatch_elapsed) = self
            .observed_after_milliseconds
            .value()
            .checked_sub(self.dispatched_after_milliseconds.value())
        else {
            return Err(invalid("diagnostic observation order"));
        };
        let Some(phase_elapsed) = self
            .observed_after_milliseconds
            .value()
            .checked_sub(diagnostic_start.value())
        else {
            return Err(invalid("diagnostic phase observation"));
        };
        let dispatch_deadline = self
            .dispatched_after_milliseconds
            .value()
            .checked_add(self.binding.time_limit_milliseconds.value())
            .ok_or(invalid("diagnostic dispatch deadline overflow"))?;
        let phase_deadline = diagnostic_start
            .value()
            .checked_add(MAX_INFEASIBILITY_DIAGNOSTIC_MILLISECONDS)
            .ok_or(invalid("diagnostic phase deadline overflow"))?;
        if self.dispatched_after_milliseconds < diagnostic_start
            || dispatch_elapsed >= self.binding.time_limit_milliseconds.value()
            || phase_elapsed >= MAX_INFEASIBILITY_DIAGNOSTIC_MILLISECONDS
            || parent_limit.is_some_and(|limit| self.observed_after_milliseconds >= limit)
            || dispatch_deadline > phase_deadline
            || parent_limit.is_some_and(|limit| dispatch_deadline > limit.value())
        {
            return Err(invalid("late diagnostic proof"));
        }
        Ok(())
    }
}

/// Complete mapped or explicit zero-assumption proof; no per-trial witnesses or frames.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticInfeasibilityProofV1 {
    /// Parent elapsed time when the shared optional cap was first taken.
    pub diagnostic_started_after_milliseconds: DurationMillis,
    /// Initial admitted infeasible request.
    pub initial: DiagnosticInfeasibilityObservationV1,
    /// Complete initial source core, empty only for explicit zero-assumption infeasibility.
    pub initial_core: Vec<ConflictGroupV1>,
    /// Only the last successful deletion proof, absent if no deletion succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_proven_infeasible: Option<DiagnosticInfeasibilityObservationV1>,
    /// Actual deletion dispatches, including feasible/inconclusive trials.
    pub attempted_trials: u32,
    /// Configured deletion cap, at most the fixed native policy.
    pub max_trials: u32,
}

impl DiagnosticInfeasibilityProofV1 {
    #[allow(clippy::too_many_lines)]
    fn validate(
        &self,
        evidence: &InfeasibilityEvidenceV1,
        original_elapsed: DurationMillis,
        parent_limit: Option<DurationMillis>,
    ) -> Result<(), DomainContractError> {
        self.initial
            .validate(self.diagnostic_started_after_milliseconds, parent_limit)?;
        if self.diagnostic_started_after_milliseconds < original_elapsed
            || self.initial_core.len() > MAX_DIAGNOSTIC_GROUPS
            || self
                .initial_core
                .windows(2)
                .any(|pair| pair[0].group_id >= pair[1].group_id)
            || self.attempted_trials > self.max_trials
            || self.max_trials > MAX_INFEASIBILITY_DELETION_TRIALS
            || (self.initial_core.len() <= 16 && self.max_trials != 0)
        {
            return Err(invalid("diagnostic initial proof"));
        }
        let initial_by_group = self
            .initial
            .binding
            .submitted_assumptions
            .iter()
            .map(|assumption| (&assumption.group_id, assumption))
            .collect::<BTreeMap<_, _>>();
        for group in &self.initial_core {
            group.validate()?;
            if group.required_rules.len() != 1
                || initial_by_group
                    .get(&group.group_id)
                    .is_none_or(|assumption| assumption.required_rule_id != group.required_rules[0])
            {
                return Err(invalid("initial core source ownership"));
            }
        }
        if self.initial_core.is_empty() != self.initial.binding.submitted_assumptions.is_empty() {
            return Err(invalid("empty native core is not foundational proof"));
        }
        let final_binding = if let Some(last) = &self.last_proven_infeasible {
            last.validate(self.diagnostic_started_after_milliseconds, parent_limit)?;
            if self.attempted_trials == 0
                || last.dispatched_after_milliseconds < self.initial.observed_after_milliseconds
                || last.binding.worker_manifest_sha256
                    != self.initial.binding.worker_manifest_sha256
                || (last.binding.model_hash == self.initial.binding.model_hash
                    && last.binding.submitted_assumptions
                        != self.initial.binding.submitted_assumptions)
            {
                return Err(invalid("last diagnostic proof binding"));
            }
            for assumption in &last.binding.submitted_assumptions {
                if self
                    .initial
                    .binding
                    .submitted_assumptions
                    .binary_search(assumption)
                    .is_err()
                    || self
                        .initial_core
                        .binary_search_by(|group| group.group_id.cmp(&assumption.group_id))
                        .is_err()
                {
                    return Err(invalid(
                        "last diagnostic request not an initial-core subset",
                    ));
                }
            }
            let removed_groups = self
                .initial_core
                .len()
                .checked_sub(last.binding.submitted_assumptions.len())
                .ok_or(invalid("last diagnostic request group count"))?;
            if usize::try_from(self.attempted_trials)
                .ok()
                .is_none_or(|trials| removed_groups > trials)
            {
                return Err(invalid("successful deletions exceed dispatched trials"));
            }
            &last.binding
        } else {
            &self.initial.binding
        };
        match evidence {
            InfeasibilityEvidenceV1::Conflict { groups, shrink, .. } => {
                if self.initial_core.is_empty()
                    || usize::try_from(shrink.initial_group_count).ok()
                        != Some(self.initial_core.len())
                    || shrink.attempted_trials != self.attempted_trials
                    || shrink.max_trials != self.max_trials
                {
                    return Err(invalid("mapped diagnostic shrink binding"));
                }
                for group in groups {
                    let Ok(index) = self
                        .initial_core
                        .binary_search_by(|initial| initial.group_id.cmp(&group.group_id))
                    else {
                        return Err(invalid("final core not an initial-core subset"));
                    };
                    if &self.initial_core[index] != group
                        || initial_by_group
                            .get(&group.group_id)
                            .is_none_or(|assumption| {
                                final_binding
                                    .submitted_assumptions
                                    .binary_search(assumption)
                                    .is_err()
                            })
                    {
                        return Err(invalid("final core source ownership"));
                    }
                }
                if let Some(last) = &self.last_proven_infeasible {
                    if last.binding.submitted_assumptions.len() != groups.len() {
                        return Err(invalid(
                            "final core does not match last successful deletion",
                        ));
                    }
                } else if groups != &self.initial_core {
                    return Err(invalid("deleted groups lack a retained proof"));
                }
            }
            InfeasibilityEvidenceV1::Unavailable {
                reason: ConflictUnavailableReason::FoundationalInfeasibility,
            } => {
                if !final_binding.submitted_assumptions.is_empty() {
                    return Err(invalid(
                        "foundation requires explicit zero-assumption infeasibility",
                    ));
                }
                if self.initial_core.is_empty()
                    && (self.attempted_trials != 0 || self.max_trials != 0)
                {
                    return Err(invalid("initial foundation trial count"));
                }
            }
            InfeasibilityEvidenceV1::Unavailable { .. } => {
                return Err(invalid(
                    "unavailable evidence cannot carry diagnostic proof",
                ));
            }
        }
        Ok(())
    }
}

/// Timely original solve observation captured before any optional diagnostic work.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OriginalInfeasibilityObservationV1 {
    /// Exact immutable original input checksum.
    pub run_input_checksum: String,
    /// Original routed model, not the guarded diagnostic model.
    pub model_hash: String,
    /// Approved worker identity when the original route used an isolated worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_manifest_sha256: Option<String>,
    /// Real wall observation of admitted original infeasibility.
    pub observed_at: Rfc3339Timestamp,
    /// Parent-measured original observation time.
    pub elapsed_milliseconds: DurationMillis,
}

/// Terminal-only evidence bound to the final refreshed/retimed manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunInfeasibilityEvidenceV1 {
    /// Current contract version, exactly one.
    pub schema_version: u32,
    /// Owned terminal run.
    pub run_id: SolveRunId,
    /// Exact immutable source scenario.
    pub scenario_id: ScenarioId,
    /// Exact immutable source revision.
    pub scenario_revision: u64,
    /// Captured original input checksum.
    pub run_input_checksum: String,
    /// Final committed manifest checksum, never a preliminary finish.
    pub run_manifest_checksum: String,
    /// Captured source document hash.
    pub source_document_hash: String,
    /// Local timely original proof observation.
    pub original: OriginalInfeasibilityObservationV1,
    /// Exact immutable question, absent for ordinary solves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<CounterfactualConditionV1>,
    /// Complete mapped/foundational proof, absent for typed unavailable enrichment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<DiagnosticInfeasibilityProofV1>,
    /// Complete conflict or truthful unavailable classification.
    pub infeasibility: InfeasibilityEvidenceV1,
    /// Canonical BLAKE3 checksum excluding this field.
    pub checksum: String,
}

impl RunInfeasibilityEvidenceV1 {
    /// Binds inert native observations only after the exact final manifest is known.
    ///
    /// # Errors
    /// Rejects invalid timing, source, profile, condition or terminal bindings.
    pub fn new(
        input: &RunInput,
        manifest: &RunManifest,
        original: OriginalInfeasibilityObservationV1,
        condition: Option<CounterfactualConditionV1>,
        diagnostic: Option<DiagnosticInfeasibilityProofV1>,
        infeasibility: InfeasibilityEvidenceV1,
    ) -> Result<Self, DomainContractError> {
        let mut value = Self {
            schema_version: 1,
            run_id: input.run_id,
            scenario_id: input.scenario_id,
            scenario_revision: input.scenario_revision,
            run_input_checksum: input.checksum.clone(),
            run_manifest_checksum: manifest.checksum.clone(),
            source_document_hash: input.snapshot_document_hash.clone(),
            original,
            condition,
            diagnostic,
            infeasibility,
            checksum: String::new(),
        };
        value.validate_payload()?;
        value.validate_bound_payload(input, manifest)?;
        value.checksum = checksum_without_field(&value, "checksum")?;
        Ok(value)
    }

    /// Strictly parses bounded retained evidence without granting local execution authority.
    ///
    /// # Errors
    /// Rejects oversize/malformed JSON, unknown versions and all structural invariants.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        if bytes.len() > MAX_RUN_INFEASIBILITY_EVIDENCE_BYTES {
            return Err(DomainContractError::LimitExceeded(
                "run infeasibility evidence bytes",
            ));
        }
        validate_nonsecret_portable_json_bytes(
            bytes,
            &PortableJsonLimits {
                max_depth: 16,
                max_string_bytes: 160,
                max_collection_items: 20_000,
            },
        )
        .map_err(|error| DomainContractError::PortableJsonViolation(error.to_string()))?;
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|error| DomainContractError::MalformedJson(error.to_string()))?;
        value.validate()?;
        Ok(value)
    }

    /// Validates canonical checksum and bounded complete identity; not producer authority.
    ///
    /// # Errors
    /// Rejects malformed or inconsistent retained records.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    /// Cross-checks exact immutable input and final terminal manifest.
    ///
    /// # Errors
    /// Rejects source/run substitution, late proofs or condition/profile drift.
    pub fn validate_binding(
        &self,
        input: &RunInput,
        manifest: &RunManifest,
    ) -> Result<(), DomainContractError> {
        self.validate()?;
        self.validate_bound_payload(input, manifest)
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        if self.schema_version != 1 {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        validate_revision(self.scenario_revision)?;
        for hash in [
            &self.run_input_checksum,
            &self.run_manifest_checksum,
            &self.source_document_hash,
            &self.original.run_input_checksum,
            &self.original.model_hash,
        ] {
            validate_hash(hash)?;
        }
        if self.original.run_input_checksum != self.run_input_checksum {
            return Err(invalid("original infeasibility input binding"));
        }
        if let Some(hash) = &self.original.worker_manifest_sha256 {
            validate_hash(hash)?;
        }
        if let Some(condition) = &self.condition {
            condition.validate()?;
        }
        self.infeasibility.validate()?;
        match (&self.infeasibility, &self.diagnostic) {
            (InfeasibilityEvidenceV1::Conflict { groups, .. }, Some(_))
                if groups.len() <= MAX_DIAGNOSTIC_GROUPS => {}
            (
                InfeasibilityEvidenceV1::Unavailable {
                    reason: ConflictUnavailableReason::FoundationalInfeasibility,
                },
                Some(_),
            ) => {}
            (InfeasibilityEvidenceV1::Unavailable { reason }, None)
                if *reason != ConflictUnavailableReason::FoundationalInfeasibility => {}
            _ => return Err(invalid("complete diagnostic evidence/proof partition")),
        }
        if let Some(diagnostic) = &self.diagnostic {
            diagnostic.validate(
                &self.infeasibility,
                self.original.elapsed_milliseconds,
                None,
            )?;
        }
        let mut count = EncodedByteCount {
            bytes: if self.checksum.is_empty() { 64 } else { 0 },
        };
        serde_json::to_writer(&mut count, self)
            .map_err(|_| DomainContractError::LimitExceeded("run infeasibility evidence bytes"))?;
        Ok(())
    }

    fn validate_bound_payload(
        &self,
        input: &RunInput,
        manifest: &RunManifest,
    ) -> Result<(), DomainContractError> {
        input.validate()?;
        manifest.validate()?;
        if self.run_id != input.run_id
            || manifest.run_id != input.run_id
            || self.scenario_id != input.scenario_id
            || self.scenario_revision != input.scenario_revision
            || self.source_document_hash != input.snapshot_document_hash
            || self.run_input_checksum != input.checksum
            || manifest.run_input_checksum != input.checksum
            || self.run_manifest_checksum != manifest.checksum
            || self.original.model_hash != input.model_hash
            || manifest.outcome
                != (RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Infeasible,
                })
            || manifest.first_verified_feasible_milliseconds.is_some()
            || self.original.elapsed_milliseconds >= input.solve_options.time_limit_milliseconds
            || manifest
                .elapsed_milliseconds
                .is_none_or(|elapsed| self.original.elapsed_milliseconds > elapsed)
            || manifest.elapsed_milliseconds.is_some_and(|elapsed| {
                elapsed
                    .value()
                    .saturating_sub(input.solve_options.time_limit_milliseconds.value())
                    > MAX_INFEASIBILITY_DIAGNOSTIC_MILLISECONDS
            })
            || self.original.observed_at < manifest.started_at
            || self.original.observed_at > manifest.finished_at
            || self.condition.as_ref().map(|condition| &condition.checksum)
                != input.temporary_condition_hash.as_ref()
        {
            return Err(invalid("terminal infeasibility custody binding"));
        }
        if let Some(diagnostic) = &self.diagnostic {
            diagnostic.validate(
                &self.infeasibility,
                self.original.elapsed_milliseconds,
                Some(input.solve_options.time_limit_milliseconds),
            )?;
            if self.original.worker_manifest_sha256.as_ref()
                != Some(&diagnostic.initial.binding.worker_manifest_sha256)
                || manifest.elapsed_milliseconds.is_none_or(|elapsed| {
                    diagnostic.initial.observed_after_milliseconds > elapsed
                        || diagnostic
                            .last_proven_infeasible
                            .as_ref()
                            .is_some_and(|last| last.observed_after_milliseconds > elapsed)
                })
            {
                return Err(invalid("diagnostic original/runtime binding"));
            }
        }
        Ok(())
    }
}

fn invalid(invariant: &'static str) -> DomainContractError {
    DomainContractError::InvalidExplanationContract(invariant)
}

struct EncodedByteCount {
    bytes: usize,
}

impl std::io::Write for EncodedByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|count| *count <= MAX_RUN_INFEASIBILITY_EVIDENCE_BYTES)
            .ok_or_else(|| std::io::Error::other("run infeasibility evidence bytes"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Local terminal authority reference. Serialized shape alone cannot authorize a store transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    deny_unknown_fields
)]
pub enum CounterfactualTerminalAuthorityV1 {
    /// A timely original infeasible observation survived stopped optional enrichment.
    TimelyInfeasibility {
        /// Exact owned derived run.
        run_id: SolveRunId,
        /// Exact final refreshed manifest checksum.
        run_manifest_checksum: String,
        /// Atomic retained run evidence checksum.
        evidence_checksum: String,
    },
    /// A locally owned correctness alarm outranks optional cancellation or expiry.
    VerificationAlarm {
        /// Exact owned derived run.
        run_id: SolveRunId,
        /// Exact final alarm manifest checksum.
        run_manifest_checksum: String,
        /// Exact clock-failure marker copied from the admitted alarm manifest.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clock_failure: Option<RunClockFailureV1>,
    },
}

impl CounterfactualTerminalAuthorityV1 {
    /// Checks closed reference shape without granting producer authority.
    ///
    /// # Errors
    /// Rejects malformed retained hashes.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        match self {
            Self::TimelyInfeasibility {
                run_manifest_checksum,
                evidence_checksum,
                ..
            } => {
                validate_hash(run_manifest_checksum)?;
                validate_hash(evidence_checksum)
            }
            Self::VerificationAlarm {
                run_manifest_checksum,
                ..
            } => validate_hash(run_manifest_checksum),
        }
    }

    /// Cross-checks locally captured terminal data before one native job transaction.
    ///
    /// # Errors
    /// Rejects run, manifest, condition-proof, checksum or alarm substitution.
    pub fn validate_binding(
        &self,
        input: &RunInput,
        manifest: &RunManifest,
        evidence: Option<&RunInfeasibilityEvidenceV1>,
    ) -> Result<(), DomainContractError> {
        self.validate()?;
        input.validate()?;
        manifest.validate()?;
        let (run_id, checksum) = match self {
            Self::TimelyInfeasibility {
                run_id,
                run_manifest_checksum,
                evidence_checksum,
            } => {
                let evidence = evidence.ok_or(invalid("missing timely terminal evidence"))?;
                evidence.validate_binding(input, manifest)?;
                if &evidence.checksum != evidence_checksum || evidence.condition.is_none() {
                    return Err(invalid("counterfactual terminal evidence reference"));
                }
                (run_id, run_manifest_checksum)
            }
            Self::VerificationAlarm {
                run_id,
                run_manifest_checksum,
                clock_failure,
            } => {
                if !matches!(
                    &manifest.outcome,
                    RunTerminalOutcomeV1::VerificationAlarm { .. }
                ) || evidence.is_some()
                    || clock_failure != &manifest.clock_failure
                {
                    return Err(invalid("counterfactual alarm terminal reference"));
                }
                (run_id, run_manifest_checksum)
            }
        };
        if *run_id != input.run_id
            || manifest.run_id != input.run_id
            || checksum != &manifest.checksum
            || manifest.run_input_checksum != input.checksum
        {
            return Err(invalid("counterfactual terminal authority binding"));
        }
        Ok(())
    }
}
