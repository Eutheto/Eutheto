use crate::{
    AcceptedResult, AcceptedResultRefV1, AssignmentComparisonV1, AssignmentLockStateV1,
    ComparisonBindingV1, ComparisonOrdering, DomainContractError, ExplanationCertainty,
    LockComparisonV1, MetricComparisonV1, RuleComparisonV1, RunComparisonSideV1, RunComparisonV1,
    RunManifest, RunTerminalOutcomeV1, ScoreCategoryComparisonV1, ScoreLevelComparisonV1,
    SemanticDifferenceV1, SolutionComparisonV1,
};
use eutheto_types::SolveStatus;
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

/// Identifies one side when reporting an invalid supplied contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonSide {
    /// The base accepted result or manifest.
    Base,
    /// The candidate accepted result or manifest.
    Candidate,
}

/// One caller-supplied lock-state pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComparisonLockPair {
    /// Assignment to which both observations apply.
    pub assignment_id: crate::DomainAssignmentId,
    /// Base lock state.
    pub before: AssignmentLockStateV1,
    /// Candidate lock state.
    pub after: AssignmentLockStateV1,
}

/// Both terminal manifests supplied as one inseparable pair.
#[derive(Clone, Copy, Debug)]
pub struct ComparisonRunManifests<'a> {
    /// Manifest associated with the base accepted result.
    pub base: &'a RunManifest,
    /// Manifest associated with the candidate accepted result.
    pub candidate: &'a RunManifest,
}

/// Optional, nonserializable comparison authority supplied by the caller.
#[derive(Clone, Copy, Debug, Default)]
pub struct ComparisonContext<'a> {
    /// Canonical assignment-ID-sorted lock pairs.
    pub locks: &'a [ComparisonLockPair],
    /// Either both validated manifests or neither.
    pub manifests: Option<ComparisonRunManifests<'a>>,
}

/// Failure to compare two independently accepted results.
#[derive(Debug)]
pub enum ComparisonError {
    /// An accepted result failed its own contract.
    InvalidAcceptedResult {
        /// Invalid side.
        side: ComparisonSide,
        /// Underlying domain-contract failure.
        source: DomainContractError,
    },
    /// The scenario, pack, projection, same-revision scope, or score shape is incompatible.
    Incompatible,
    /// Lock pairs are not strictly ordered by assignment ID.
    NonCanonicalLocks,
    /// A supplied manifest failed validation or did not bind its accepted result.
    InvalidManifest {
        /// Invalid side.
        side: ComparisonSide,
        /// Underlying domain-contract failure, when manifest validation itself failed.
        source: Option<DomainContractError>,
    },
    /// An exact score or category delta overflowed `i64`.
    ArithmeticOverflow,
    /// The resulting strict domain comparison contract was rejected.
    InvalidComparison(DomainContractError),
    /// The inherited operation was cancelled or reached its original deadline.
    Interrupted(eutheto_types::OperationInterruption),
}

impl fmt::Display for ComparisonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAcceptedResult { side, .. } => {
                write!(formatter, "invalid {side:?} accepted result")
            }
            Self::Incompatible => formatter.write_str("accepted results are incompatible"),
            Self::NonCanonicalLocks => {
                formatter.write_str("comparison lock pairs are not canonical")
            }
            Self::InvalidManifest { side, .. } => {
                write!(formatter, "invalid {side:?} run manifest")
            }
            Self::ArithmeticOverflow => formatter.write_str("score comparison arithmetic overflow"),
            Self::Interrupted(interruption) => {
                write!(
                    formatter,
                    "comparison operation interrupted: {interruption:?}"
                )
            }
            Self::InvalidComparison(error) => {
                write!(formatter, "invalid solution comparison: {error}")
            }
        }
    }
}

impl std::error::Error for ComparisonError {}

/// Compares two independently accepted results in linear time over canonical assignments/rules.
///
/// Cross-revision comparisons are supported. Both revisions, document hashes, verification scopes,
/// and accepted-result checksums remain bound into the result. Lock and terminal status evidence is
/// emitted only from explicit context; accepted results alone never fabricate either.
///
/// # Errors
/// Rejects invalid inputs, incompatible bindings or score shapes, noncanonical context, manifest
/// mismatches, and checked-arithmetic overflow.
pub fn compare_accepted_results(
    base: &AcceptedResult,
    candidate: &AcceptedResult,
    context: Option<&ComparisonContext<'_>>,
) -> Result<SolutionComparisonV1, ComparisonError> {
    validate_accepted_results(base, candidate)?;
    validate_compatibility(base, candidate)?;

    let (assignments, mut affected_entities) = assignment_deltas(base, candidate, None)?;
    let (rules, rule_entities) = rule_deltas(base, candidate, None)?;
    affected_entities.extend(rule_entities);
    let score_levels = score_deltas(base, candidate)?;
    let metrics = metric_deltas(base, candidate);
    let locks = lock_deltas(context)?;
    let runs = run_comparison(base, candidate, context)?;
    let ordering = ordering(base, candidate)?;

    SolutionComparisonV1::new(
        binding(base),
        binding(candidate),
        base.verification.score.clone(),
        candidate.verification.score.clone(),
        assignments,
        rules,
        score_levels,
        metrics,
        locks,
        runs,
        affected_entities.into_iter().collect(),
        ordering,
    )
    .map_err(ComparisonError::InvalidComparison)
}

/// Compares two independently accepted results without making objective-score or lock claims.
///
/// This path accepts different score shapes and never computes score deltas or ordering. It
/// retains only canonical assignment, required-rule, metric, and explicitly supplied terminal-run
/// differences.
///
/// # Errors
/// Rejects invalid inputs, incompatible bindings, invalid supplied manifests, and an invalid
/// resulting semantic-difference contract.
pub fn compare_semantic_accepted_results(
    base: &AcceptedResult,
    candidate: &AcceptedResult,
    context: Option<&ComparisonContext<'_>>,
) -> Result<SemanticDifferenceV1, ComparisonError> {
    validate_accepted_results(base, candidate)?;
    validate_semantic_compatibility(base, candidate)?;

    let (assignments, mut affected_entities) = assignment_deltas(base, candidate, None)?;
    let (rules, rule_entities) = rule_deltas(base, candidate, None)?;
    affected_entities.extend(rule_entities);
    let metrics = metric_deltas(base, candidate);
    let runs = run_comparison(base, candidate, context)?;

    SemanticDifferenceV1::new(
        binding(base),
        binding(candidate),
        assignments,
        rules,
        metrics,
        runs,
        affected_entities.into_iter().collect(),
    )
    .map_err(ComparisonError::InvalidComparison)
}

/// Pack-established score semantics for one exact result; this is not acceptance authority.
#[derive(Clone, Debug)]
pub struct ComparisonScoringContext<'a> {
    /// Exact immutable result/input/manifest source whose scoring meaning was evaluated.
    pub source: &'a crate::PortableAcceptedResult,
    /// Versioned pack-owned semantic projection producer, not a display label.
    pub producer_id: crate::DomainEvidenceId,
    /// BLAKE3 of scoring definitions, units, population, weights, scaling and order.
    pub semantic_hash: String,
}

/// Source-owned additions to generic deltas, obtained before the complete envelope is sealed.
#[derive(Clone, Debug, Default)]
pub struct ComparisonEnrichmentV2<'a> {
    /// Complete source protection union in canonical assignment order.
    pub protections: Vec<crate::AssignmentProtectionComparisonV1>,
    /// Canonical affected-party labels resolved separately from both immutable sources.
    pub affected_parties: Vec<crate::ComparisonEntityObservationV1>,
    /// Both exact source scoring projections, or no established compatible policy.
    pub scoring_contexts: Option<(ComparisonScoringContext<'a>, ComparisonScoringContext<'a>)>,
}

/// Computed generic deltas awaiting pack-owned source projection, with no wire checksum yet.
///
/// The two-stage boundary lets a pack map changed assignment identities without owning generic
/// delta computation or introducing a dependency from `DomainIR` back to a domain pack.
pub struct PreparedComparisonV2<'a> {
    base: &'a crate::PortableAcceptedResult,
    candidate: &'a crate::PortableAcceptedResult,
    control: Option<&'a eutheto_types::OperationControl>,
    assignments: Vec<AssignmentComparisonV1>,
    rules: Vec<RuleComparisonV1>,
    metrics: Vec<MetricComparisonV1>,
    runs: crate::RunComparisonV2,
    affected_entities: BTreeSet<crate::DomainEntityRef>,
}

/// Validates both immutable portable records and computes generic deltas exactly once.
///
/// Contract validation is not local acceptance. Native consumers must load canonical records and
/// freshly verify the two original sources before calling this helper.
///
/// # Errors
/// Rejects malformed/crossbound records, incompatible pack/projection identity, finite output
/// bounds, or inherited cancellation/deadline.
pub fn prepare_comparison_v2<'a>(
    base: &'a crate::PortableAcceptedResult,
    candidate: &'a crate::PortableAcceptedResult,
    control: Option<&'a eutheto_types::OperationControl>,
) -> Result<PreparedComparisonV2<'a>, ComparisonError> {
    for (source, side) in [
        (base, ComparisonSide::Base),
        (candidate, ComparisonSide::Candidate),
    ] {
        comparison_checkpoint(control, 0)?;
        crate::ensure_portable_domain_contract_size(source)
            .map_err(|source| ComparisonError::InvalidAcceptedResult { side, source })?;
        source
            .validate()
            .map_err(|source| ComparisonError::InvalidAcceptedResult { side, source })?;
    }
    let before = &base.accepted_result;
    let after = &candidate.accepted_result;
    validate_semantic_compatibility(before, after)?;
    let (assignments, mut affected_entities) = assignment_deltas(before, after, control)?;
    let (rules, rule_entities) = rule_deltas(before, after, control)?;
    for (index, entity) in rule_entities.into_iter().enumerate() {
        comparison_checkpoint(control, index)?;
        if affected_entities.len() >= crate::MAX_EXPLANATION_RECORDS
            && !affected_entities.contains(&entity)
        {
            return Err(ComparisonError::InvalidComparison(
                DomainContractError::LimitExceeded("comparison affected entities"),
            ));
        }
        affected_entities.insert(entity);
    }
    // Portable custody was validated above; recorded status cannot authenticate backend proof.
    let runs = crate::RunComparisonV2 {
        base: crate::RunComparisonSideV2 {
            run_id: base.run_manifest.run_id,
            run_manifest_checksum: base.run_manifest.checksum.clone(),
            outcome: base.run_manifest.outcome.clone(),
        },
        candidate: crate::RunComparisonSideV2 {
            run_id: candidate.run_manifest.run_id,
            run_manifest_checksum: candidate.run_manifest.checksum.clone(),
            outcome: candidate.run_manifest.outcome.clone(),
        },
    };
    comparison_checkpoint(control, 0)?;
    Ok(PreparedComparisonV2 {
        base,
        candidate,
        control,
        assignments,
        rules,
        metrics: metric_deltas(before, after),
        runs,
        affected_entities,
    })
}

impl PreparedComparisonV2<'_> {
    /// Borrowed canonical assignment deltas for pack-owned affected-party projection.
    #[must_use]
    pub fn assignments(&self) -> &[AssignmentComparisonV1] {
        &self.assignments
    }

    /// Borrowed Required-rule deltas; their scope bindings remain side-specific.
    #[must_use]
    pub fn rules(&self) -> &[RuleComparisonV1] {
        &self.rules
    }

    /// Borrowed changed metrics, without asserting that unlike policies share universal utility.
    #[must_use]
    pub fn metrics(&self) -> &[MetricComparisonV1] {
        &self.metrics
    }

    /// Canonical generic affected identities for source-side label resolution.
    #[must_use]
    pub fn affected_entities(&self) -> &BTreeSet<crate::DomainEntityRef> {
        &self.affected_entities
    }

    /// Validates source-owned additions, derives compatible scores and seals complete evidence.
    ///
    /// # Errors
    /// Rejects misbound scoring projections, contradictory additions, overflow, resource bounds
    /// or inherited interruption. Incompatible scores are explicitly unavailable, not an error.
    pub fn finish(
        mut self,
        enrichment: ComparisonEnrichmentV2<'_>,
    ) -> Result<crate::SolutionComparisonV2, ComparisonError> {
        comparison_checkpoint(self.control, 0)?;
        if enrichment.protections.len() > crate::MAX_EXPLANATION_RECORDS
            || enrichment.affected_parties.len() > crate::MAX_EXPLANATION_RECORDS
        {
            return Err(ComparisonError::InvalidComparison(
                DomainContractError::LimitExceeded("comparison source additions"),
            ));
        }
        for (index, protection) in enrichment.protections.iter().enumerate() {
            comparison_checkpoint(self.control, index)?;
            let [before, after] = [self.base, self.candidate].map(|source| {
                source
                    .accepted_result
                    .solution
                    .assignments
                    .binary_search_by(|assignment| assignment.id.cmp(&protection.assignment_id))
                    .ok()
                    .map(|index| &source.accepted_result.solution.assignments[index].value)
            });
            protection
                .validate_selection_values(before, after)
                .map_err(ComparisonError::InvalidComparison)?;
        }
        for (index, party) in enrichment.affected_parties.iter().enumerate() {
            comparison_checkpoint(self.control, index)?;
            insert_affected_entity(&mut self.affected_entities, &party.entity)?;
        }
        let score = comparison_score_v2(
            self.base,
            self.candidate,
            enrichment.scoring_contexts.as_ref(),
        )?;
        let comparison = crate::SolutionComparisonV2 {
            schema_version: crate::SOLUTION_COMPARISON_V2_SCHEMA_VERSION,
            base: binding(&self.base.accepted_result),
            candidate: binding(&self.candidate.accepted_result),
            assignments: self.assignments,
            rules: self.rules,
            score,
            metrics: self.metrics,
            protections: enrichment.protections,
            runs: self.runs,
            affected_entities: self.affected_entities.into_iter().collect(),
            entity_observations: enrichment.affected_parties,
            checksum: String::new(),
        }
        .seal()
        .map_err(ComparisonError::InvalidComparison)?;
        comparison_checkpoint(self.control, 0)?;
        Ok(comparison)
    }
}

fn comparison_score_v2(
    base: &crate::PortableAcceptedResult,
    candidate: &crate::PortableAcceptedResult,
    contexts: Option<&(ComparisonScoringContext<'_>, ComparisonScoringContext<'_>)>,
) -> Result<crate::ScoreComparisonV2, ComparisonError> {
    let unavailable = |reason| crate::ScoreComparisonV2::Unavailable { reason };
    let Some((before, after)) = contexts else {
        return Ok(unavailable(
            crate::ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch,
        ));
    };
    for (projection, source) in [(before, base), (after, candidate)] {
        if projection.source.result_id != source.result_id
            || projection.source.accepted_result.checksum != source.accepted_result.checksum
            || projection.source.run_input.checksum != source.run_input.checksum
            || projection.source.run_manifest.checksum != source.run_manifest.checksum
        {
            return Err(ComparisonError::InvalidComparison(
                DomainContractError::InvalidExplanationContract("comparison scoring source"),
            ));
        }
        crate::validate_hash(&projection.semantic_hash)
            .map_err(ComparisonError::InvalidComparison)?;
    }
    if before.producer_id != after.producer_id
        || before.semantic_hash != after.semantic_hash
        || base.run_input.compiler_version != candidate.run_input.compiler_version
        || objective_semantics(&base.run_input) != objective_semantics(&candidate.run_input)
    {
        return Ok(unavailable(
            crate::ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch,
        ));
    }
    let before = &base.accepted_result;
    let after = &candidate.accepted_result;
    if before
        .verification
        .score
        .compare(&after.verification.score)
        .is_err()
    {
        return Ok(unavailable(
            crate::ScoreComparisonUnavailableReasonV1::ScoreShapeMismatch,
        ));
    }
    Ok(crate::ScoreComparisonV2::Comparable {
        base_score: before.verification.score.clone(),
        candidate_score: after.verification.score.clone(),
        score_levels: score_deltas(before, after)?,
        ordering: ordering(before, after)?,
    })
}

fn objective_semantics(
    input: &crate::RunInput,
) -> Option<(
    &crate::ObjectiveProfileId,
    u32,
    crate::RunObjectiveOperation,
)> {
    input.objective_context.as_ref().map(|context| {
        (
            &context.profile_id,
            context.semantic_version,
            context.operation,
        )
    })
}

fn validate_accepted_results(
    base: &AcceptedResult,
    candidate: &AcceptedResult,
) -> Result<(), ComparisonError> {
    base.validate()
        .map_err(|source| ComparisonError::InvalidAcceptedResult {
            side: ComparisonSide::Base,
            source,
        })?;
    candidate
        .validate()
        .map_err(|source| ComparisonError::InvalidAcceptedResult {
            side: ComparisonSide::Candidate,
            source,
        })?;
    Ok(())
}

fn validate_semantic_compatibility(
    base: &AcceptedResult,
    candidate: &AcceptedResult,
) -> Result<(), ComparisonError> {
    if base.solution.pack_id != candidate.solution.pack_id
        || base.solution.scenario_id != candidate.solution.scenario_id
        || base.solution.projection_version != candidate.solution.projection_version
        || (base.solution.scenario_revision == candidate.solution.scenario_revision
            && base.verification.verification_scope_checksum
                != candidate.verification.verification_scope_checksum)
    {
        return Err(ComparisonError::Incompatible);
    }
    Ok(())
}

fn validate_compatibility(
    base: &AcceptedResult,
    candidate: &AcceptedResult,
) -> Result<(), ComparisonError> {
    validate_semantic_compatibility(base, candidate)?;
    if base
        .verification
        .score
        .compare(&candidate.verification.score)
        .is_err()
    {
        return Err(ComparisonError::Incompatible);
    }
    Ok(())
}

fn binding(result: &AcceptedResult) -> ComparisonBindingV1 {
    ComparisonBindingV1 {
        pack_id: result.solution.pack_id.clone(),
        scenario_id: result.solution.scenario_id,
        scenario_revision: result.solution.scenario_revision,
        document_hash: result.verification.document_hash.clone(),
        projection_version: result.solution.projection_version,
        verification_scope_checksum: result.verification.verification_scope_checksum.clone(),
        accepted_result: AcceptedResultRefV1 {
            solution_id: result.solution.solution_id,
            result_checksum: result.checksum.clone(),
        },
    }
}

fn assignment_deltas(
    base: &AcceptedResult,
    candidate: &AcceptedResult,
    control: Option<&eutheto_types::OperationControl>,
) -> Result<
    (
        Vec<AssignmentComparisonV1>,
        BTreeSet<crate::DomainEntityRef>,
    ),
    ComparisonError,
> {
    let before = &base.solution.assignments;
    let after = &candidate.solution.assignments;
    let mut left = 0;
    let mut right = 0;
    let mut deltas = Vec::new();
    let mut entities = BTreeSet::new();
    while left < before.len() || right < after.len() {
        comparison_checkpoint(control, left + right)?;
        let (old, new) = match (before.get(left), after.get(right)) {
            (Some(old), Some(new)) => match old.id.cmp(&new.id) {
                Ordering::Less => {
                    left += 1;
                    (Some(old), None)
                }
                Ordering::Greater => {
                    right += 1;
                    (None, Some(new))
                }
                Ordering::Equal => {
                    left += 1;
                    right += 1;
                    if old == new {
                        continue;
                    }
                    (Some(old), Some(new))
                }
            },
            (Some(old), None) => {
                left += 1;
                (Some(old), None)
            }
            (None, Some(new)) => {
                right += 1;
                (None, Some(new))
            }
            (None, None) => break,
        };
        comparison_record_capacity(deltas.len(), "assignment comparisons")?;
        for entity in old
            .into_iter()
            .map(|value| &value.entity)
            .chain(new.into_iter().map(|value| &value.entity))
        {
            insert_affected_entity(&mut entities, entity)?;
        }
        let delta = match (old, new) {
            (Some(before), Some(after)) => AssignmentComparisonV1::Changed {
                before: before.clone(),
                after: after.clone(),
            },
            (Some(before), None) => AssignmentComparisonV1::Removed {
                before: before.clone(),
            },
            (None, Some(after)) => AssignmentComparisonV1::Added {
                after: after.clone(),
            },
            (None, None) => unreachable!("a canonical merge always yields at least one side"),
        };
        deltas.push(delta);
    }
    Ok((deltas, entities))
}

fn rule_deltas(
    base: &AcceptedResult,
    candidate: &AcceptedResult,
    control: Option<&eutheto_types::OperationControl>,
) -> Result<(Vec<RuleComparisonV1>, BTreeSet<crate::DomainEntityRef>), ComparisonError> {
    let before = &base.verification.required_rule_results;
    let after = &candidate.verification.required_rule_results;
    let mut left = 0;
    let mut right = 0;
    let mut deltas = Vec::new();
    let mut entities = BTreeSet::new();
    while left < before.len() || right < after.len() {
        comparison_checkpoint(control, left + right)?;
        let (rule_id, old, new) = match (before.get(left), after.get(right)) {
            (Some(old), Some(new)) => match old.rule_id.cmp(&new.rule_id) {
                Ordering::Less => {
                    left += 1;
                    (old.rule_id, Some(old), None)
                }
                Ordering::Greater => {
                    right += 1;
                    (new.rule_id, None, Some(new))
                }
                Ordering::Equal => {
                    left += 1;
                    right += 1;
                    if old == new {
                        continue;
                    }
                    (old.rule_id, Some(old), Some(new))
                }
            },
            (Some(old), None) => {
                left += 1;
                (old.rule_id, Some(old), None)
            }
            (None, Some(new)) => {
                right += 1;
                (new.rule_id, None, Some(new))
            }
            (None, None) => break,
        };
        comparison_record_capacity(deltas.len(), "rule comparisons")?;
        for entity in old
            .into_iter()
            .flat_map(|value| &value.affected_entities)
            .chain(new.into_iter().flat_map(|value| &value.affected_entities))
        {
            insert_affected_entity(&mut entities, entity)?;
        }
        deltas.push(RuleComparisonV1 {
            rule_id,
            before: old.cloned(),
            after: new.cloned(),
        });
    }
    Ok((deltas, entities))
}

fn comparison_checkpoint(
    control: Option<&eutheto_types::OperationControl>,
    visited: usize,
) -> Result<(), ComparisonError> {
    // A merge advances one or two source rows; either parity must reach a checkpoint.
    if visited % 256 <= 1
        && let Some(control) = control
    {
        control.check().map_err(ComparisonError::Interrupted)?;
    }
    Ok(())
}

fn comparison_record_capacity(len: usize, label: &'static str) -> Result<(), ComparisonError> {
    if len >= crate::MAX_EXPLANATION_RECORDS {
        return Err(ComparisonError::InvalidComparison(
            DomainContractError::NonCanonicalExplanationCollection(label),
        ));
    }
    Ok(())
}

fn insert_affected_entity(
    entities: &mut BTreeSet<crate::DomainEntityRef>,
    entity: &crate::DomainEntityRef,
) -> Result<(), ComparisonError> {
    if !entities.contains(entity) {
        if entities.len() >= crate::MAX_EXPLANATION_RECORDS {
            return Err(ComparisonError::InvalidComparison(
                DomainContractError::LimitExceeded("comparison affected entities"),
            ));
        }
        entities.insert(entity.clone());
    }
    Ok(())
}

fn score_deltas(
    base: &AcceptedResult,
    candidate: &AcceptedResult,
) -> Result<Vec<ScoreLevelComparisonV1>, ComparisonError> {
    base.verification
        .score
        .levels
        .iter()
        .zip(&candidate.verification.score.levels)
        .map(|(before, after)| {
            let delta = after
                .value
                .checked_sub(before.value)
                .ok_or(ComparisonError::ArithmeticOverflow)?;
            let mut categories = Vec::with_capacity(
                before.category_breakdown.len() + after.category_breakdown.len(),
            );
            let mut left = before.category_breakdown.iter().peekable();
            let mut right = after.category_breakdown.iter().peekable();
            loop {
                let (category_id, old, new) = match (left.peek(), right.peek()) {
                    (Some((old_id, old)), Some((new_id, new))) => match old_id.cmp(new_id) {
                        Ordering::Less => {
                            let value = ((**old_id).clone(), Some(**old), None);
                            left.next();
                            value
                        }
                        Ordering::Equal => {
                            let value = ((**old_id).clone(), Some(**old), Some(**new));
                            left.next();
                            right.next();
                            value
                        }
                        Ordering::Greater => {
                            let value = ((**new_id).clone(), None, Some(**new));
                            right.next();
                            value
                        }
                    },
                    (Some((old_id, old)), None) => {
                        let value = ((**old_id).clone(), Some(**old), None);
                        left.next();
                        value
                    }
                    (None, Some((new_id, new))) => {
                        let value = ((**new_id).clone(), None, Some(**new));
                        right.next();
                        value
                    }
                    (None, None) => break,
                };
                let category_delta = match (old, new) {
                    (Some(old), Some(new)) => Some(
                        new.checked_sub(old)
                            .ok_or(ComparisonError::ArithmeticOverflow)?,
                    ),
                    _ => None,
                };
                categories.push(ScoreCategoryComparisonV1 {
                    category_id,
                    before: old,
                    after: new,
                    delta: category_delta,
                });
            }
            Ok(ScoreLevelComparisonV1 {
                level_id: before.level_id.clone(),
                direction: before.direction,
                before: before.value,
                after: after.value,
                delta,
                categories,
            })
        })
        .collect()
}

fn metric_deltas(base: &AcceptedResult, candidate: &AcceptedResult) -> Vec<MetricComparisonV1> {
    let mut before = base.verification.metrics.iter().peekable();
    let mut after = candidate.verification.metrics.iter().peekable();
    let mut deltas = Vec::new();
    loop {
        let (metric_id, old, new) = match (before.peek(), after.peek()) {
            (Some((old_id, old)), Some((new_id, new))) => match old_id.cmp(new_id) {
                Ordering::Less => {
                    let value = ((**old_id).clone(), Some((**old).clone()), None);
                    before.next();
                    value
                }
                Ordering::Equal => {
                    let value = (
                        (**old_id).clone(),
                        Some((**old).clone()),
                        Some((**new).clone()),
                    );
                    before.next();
                    after.next();
                    value
                }
                Ordering::Greater => {
                    let value = ((**new_id).clone(), None, Some((**new).clone()));
                    after.next();
                    value
                }
            },
            (Some((old_id, old)), None) => {
                let value = ((**old_id).clone(), Some((**old).clone()), None);
                before.next();
                value
            }
            (None, Some((new_id, new))) => {
                let value = ((**new_id).clone(), None, Some((**new).clone()));
                after.next();
                value
            }
            (None, None) => break,
        };
        if old != new {
            deltas.push(MetricComparisonV1 {
                metric_id,
                before: old,
                after: new,
            });
        }
    }
    deltas
}

fn lock_deltas(
    context: Option<&ComparisonContext<'_>>,
) -> Result<Vec<LockComparisonV1>, ComparisonError> {
    let Some(context) = context else {
        return Ok(Vec::new());
    };
    if !context
        .locks
        .windows(2)
        .all(|pair| pair[0].assignment_id < pair[1].assignment_id)
    {
        return Err(ComparisonError::NonCanonicalLocks);
    }
    Ok(context
        .locks
        .iter()
        .map(|pair| LockComparisonV1 {
            assignment_id: pair.assignment_id.clone(),
            before: pair.before.clone(),
            after: pair.after.clone(),
            preserved: pair.before == pair.after,
        })
        .collect())
}

fn run_comparison(
    base: &AcceptedResult,
    candidate: &AcceptedResult,
    context: Option<&ComparisonContext<'_>>,
) -> Result<Option<RunComparisonV1>, ComparisonError> {
    let Some(manifests) = context.and_then(|value| value.manifests) else {
        return Ok(None);
    };
    validate_manifest(manifests.base, base, ComparisonSide::Base)?;
    validate_manifest(manifests.candidate, candidate, ComparisonSide::Candidate)?;
    Ok(Some(RunComparisonV1 {
        base: run_side(manifests.base),
        candidate: run_side(manifests.candidate),
    }))
}

fn validate_manifest(
    manifest: &RunManifest,
    accepted: &AcceptedResult,
    side: ComparisonSide,
) -> Result<(), ComparisonError> {
    manifest
        .validate()
        .map_err(|source| ComparisonError::InvalidManifest {
            side,
            source: Some(source),
        })?;
    let expected = &accepted.solution;
    match &manifest.outcome {
        RunTerminalOutcomeV1::Accepted {
            solution_id,
            accepted_result_checksum,
            verification_checksum,
            ..
        } if solution_id == &expected.solution_id
            && accepted_result_checksum == &accepted.checksum
            && verification_checksum == &accepted.verification.checksum =>
        {
            Ok(())
        }
        _ => Err(ComparisonError::InvalidManifest { side, source: None }),
    }
}

fn run_side(manifest: &RunManifest) -> RunComparisonSideV1 {
    RunComparisonSideV1 {
        run_id: manifest.run_id,
        run_manifest_checksum: manifest.checksum.clone(),
        outcome: manifest.outcome.clone(),
        certainty: terminal_certainty(&manifest.outcome),
    }
}

fn terminal_certainty(outcome: &RunTerminalOutcomeV1) -> ExplanationCertainty {
    match outcome {
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Optimal,
            ..
        }
        | RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible | SolveStatus::Unbounded,
        } => ExplanationCertainty::BackendProof,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            ..
        } => ExplanationCertainty::IndependentlyVerified,
        _ => ExplanationCertainty::Deterministic,
    }
}

fn ordering(
    base: &AcceptedResult,
    candidate: &AcceptedResult,
) -> Result<ComparisonOrdering, ComparisonError> {
    match base
        .verification
        .score
        .compare(&candidate.verification.score)
        .map_err(|_| ComparisonError::Incompatible)?
    {
        Ordering::Less => Ok(ComparisonOrdering::Worse),
        Ordering::Equal => Ok(ComparisonOrdering::Equivalent),
        Ordering::Greater => Ok(ComparisonOrdering::Better),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DomainAssignment, DomainAssignmentId, DomainEntityId, DomainEntityKindId, DomainEntityRef,
        MetricId, MetricValue, NORMALIZED_SOLUTION_SCHEMA_VERSION, NormalizedSolution,
        OptimizationDirection, RuleEvaluation, RunPhaseTimingsV1, ScoreCategoryId, ScoreLevelId,
        ScoreLevelValue, ScoreVector, VerificationContextV1, VerificationReport, blake3_hex,
    };
    use eutheto_types::{
        DurationMillis, PackId, Rfc3339Timestamp, RuleId, ScenarioId, SolutionId, SolveRunId,
    };
    use std::collections::BTreeMap;

    fn hash(label: &str) -> String {
        blake3_hex(label.as_bytes())
    }

    fn entity(name: &str) -> Result<DomainEntityRef, Box<dyn std::error::Error>> {
        Ok(DomainEntityRef {
            kind: DomainEntityKindId::new("tests.person")?,
            id: DomainEntityId::new(format!("tests.{name}"))?,
        })
    }

    fn assignment(name: &str, value: i64) -> Result<DomainAssignment, Box<dyn std::error::Error>> {
        Ok(DomainAssignment {
            id: DomainAssignmentId::new(format!("tests.{name}"))?,
            entity: entity(name)?,
            value: crate::AssignmentValue::Integer(value),
            evidence: Vec::new(),
        })
    }

    fn rule(
        suffix: u8,
        entity_name: &str,
        message: &str,
    ) -> Result<RuleEvaluation, Box<dyn std::error::Error>> {
        Ok(RuleEvaluation {
            rule_id: format!("01890a5d-ac96-7b64-9f74-bbfcf30f9f{suffix:02x}").parse::<RuleId>()?,
            satisfied: true,
            affected_entities: vec![entity(entity_name)?],
            message_key: message.to_owned(),
            expected: BTreeMap::new(),
            observed: BTreeMap::new(),
            evidence: Vec::new(),
        })
    }

    struct AcceptedArgs<'a> {
        revision: u64,
        solution_suffix: u8,
        score: i64,
        assignments: Vec<DomainAssignment>,
        rules: Vec<RuleEvaluation>,
        category: Option<i64>,
        metrics: BTreeMap<MetricId, MetricValue>,
        scenario: &'a str,
        pack: &'a str,
        projection_version: u32,
    }

    fn accepted(args: AcceptedArgs<'_>) -> Result<AcceptedResult, Box<dyn std::error::Error>> {
        let scenario_id = args.scenario.parse::<ScenarioId>()?;
        let mut solution = NormalizedSolution {
            schema_version: NORMALIZED_SOLUTION_SCHEMA_VERSION,
            pack_id: PackId::new(args.pack)?,
            scenario_id,
            scenario_revision: args.revision,
            projection_version: args.projection_version,
            solution_id: format!(
                "01890a5d-ac96-7b64-9f74-bbfcf30f9f{:02x}",
                args.solution_suffix
            )
            .parse::<SolutionId>()?,
            assignments: args.assignments,
        };
        solution.canonicalize()?;
        let mut category_breakdown = BTreeMap::new();
        if let Some(value) = args.category {
            category_breakdown.insert(ScoreCategoryId::new("tests.category")?, value);
        }
        let score = ScoreVector {
            feasibility: 0,
            levels: vec![ScoreLevelValue {
                level_id: ScoreLevelId::new("tests.primary")?,
                value: args.score,
                direction: OptimizationDirection::Minimize,
                category_breakdown,
            }],
        };
        let context = VerificationContextV1::new(
            scenario_id,
            args.revision,
            hash(&format!("document-{}", args.revision)),
            hash(&format!("model-{}", args.revision)),
            solution.canonical_hash()?,
            hash(&format!("scope-{}", args.revision)),
        )?;
        let report =
            VerificationReport::new(&context, args.rules, score, Vec::new(), args.metrics)?;
        Ok(AcceptedResult::new(solution, report)?)
    }

    fn pair(
        base_score: i64,
        candidate_score: i64,
    ) -> Result<(AcceptedResult, AcceptedResult), Box<dyn std::error::Error>> {
        let scenario = "01890a5d-ac96-7b64-9f74-bbfcf30f9f80";
        let base = accepted(AcceptedArgs {
            revision: 1,
            solution_suffix: 0x10,
            score: base_score,
            assignments: vec![assignment("a", 1)?, assignment("b", 2)?],
            rules: vec![rule(1, "a", "tests.rule.base")?],
            category: Some(base_score),
            metrics: BTreeMap::from([
                (MetricId::new("tests.changed")?, MetricValue::Integer(1)),
                (MetricId::new("tests.removed")?, MetricValue::Integer(7)),
            ]),
            scenario,
            pack: "official.test",
            projection_version: 1,
        })?;
        let candidate = accepted(AcceptedArgs {
            revision: 2,
            solution_suffix: 0x11,
            score: candidate_score,
            assignments: vec![assignment("b", 3)?, assignment("c", 4)?],
            rules: vec![
                rule(1, "b", "tests.rule.changed")?,
                rule(2, "c", "tests.rule.added")?,
            ],
            category: None,
            metrics: BTreeMap::from([
                (MetricId::new("tests.added")?, MetricValue::Integer(8)),
                (MetricId::new("tests.changed")?, MetricValue::Integer(2)),
            ]),
            scenario,
            pack: "official.test",
            projection_version: 1,
        })?;
        Ok((base, candidate))
    }

    fn manifest(
        suffix: u8,
        accepted: &AcceptedResult,
        status: SolveStatus,
    ) -> Result<RunManifest, Box<dyn std::error::Error>> {
        let run_id =
            format!("01890a5d-ac96-7b64-9f74-bbfcf30f9f{suffix:02x}").parse::<SolveRunId>()?;
        Ok(RunManifest::new(
            run_id,
            hash(&format!("input-{suffix}")),
            RunTerminalOutcomeV1::Accepted {
                status,
                solution_id: accepted.solution.solution_id,
                accepted_result_checksum: accepted.checksum.clone(),
                verification_checksum: accepted.verification.checksum.clone(),
            },
            "2026-09-03T12:00:00Z".parse::<Rfc3339Timestamp>()?,
            "2026-09-03T12:00:01Z".parse::<Rfc3339Timestamp>()?,
            Some(DurationMillis::new(1_000)?),
            Some(DurationMillis::new(100)?),
            Some(DurationMillis::new(200)?),
            RunPhaseTimingsV1::default(),
            Vec::new(),
        )?)
    }

    #[test]
    fn compares_cross_revision_changes_and_absent_values() -> Result<(), Box<dyn std::error::Error>>
    {
        let (base, candidate) = pair(10, 5)?;
        let comparison = compare_accepted_results(&base, &candidate, None)?;
        assert_eq!(comparison.base.scenario_revision, 1);
        assert_eq!(comparison.candidate.scenario_revision, 2);
        assert_eq!(comparison.ordering, ComparisonOrdering::Better);
        assert_eq!(comparison.assignments.len(), 3);
        assert_eq!(comparison.rules.len(), 2);
        assert_eq!(comparison.score_levels[0].delta, -5);
        assert_eq!(comparison.score_levels[0].categories[0].before, Some(10));
        assert_eq!(comparison.score_levels[0].categories[0].after, None);
        assert_eq!(comparison.metrics.len(), 3);
        assert!(comparison.runs.is_none());
        assert!(comparison.locks.is_empty());
        assert_eq!(
            comparison.affected_entities,
            vec![entity("a")?, entity("b")?, entity("c")?]
        );
        comparison.validate()?;
        Ok(())
    }

    #[test]
    fn emits_locks_and_status_only_from_bound_context() -> Result<(), Box<dyn std::error::Error>> {
        let (base, candidate) = pair(10, 5)?;
        let base_manifest = manifest(0x20, &base, SolveStatus::Feasible)?;
        let candidate_manifest = manifest(0x21, &candidate, SolveStatus::Optimal)?;
        let locks = [ComparisonLockPair {
            assignment_id: DomainAssignmentId::new("tests.b")?,
            before: AssignmentLockStateV1::Locked {
                value: crate::AssignmentValue::Integer(2),
            },
            after: AssignmentLockStateV1::Locked {
                value: crate::AssignmentValue::Integer(3),
            },
        }];
        let context = ComparisonContext {
            locks: &locks,
            manifests: Some(ComparisonRunManifests {
                base: &base_manifest,
                candidate: &candidate_manifest,
            }),
        };
        let comparison = compare_accepted_results(&base, &candidate, Some(&context))?;
        assert!(!comparison.locks[0].preserved);
        let runs = comparison.runs.ok_or("missing run comparison")?;
        assert_eq!(runs.base.run_manifest_checksum, base_manifest.checksum);
        assert_eq!(
            runs.candidate.run_manifest_checksum,
            candidate_manifest.checksum
        );
        assert_eq!(
            runs.base.certainty,
            ExplanationCertainty::IndependentlyVerified
        );
        assert_eq!(runs.candidate.certainty, ExplanationCertainty::BackendProof);
        Ok(())
    }

    #[test]
    fn rejects_delta_overflow_and_incompatible_bindings() -> Result<(), Box<dyn std::error::Error>>
    {
        let (minimum, maximum) = pair(i64::MIN, i64::MAX)?;
        assert!(matches!(
            compare_accepted_results(&minimum, &maximum, None),
            Err(ComparisonError::ArithmeticOverflow)
        ));

        let scenario = "01890a5d-ac96-7b64-9f74-bbfcf30f9f81";
        let incompatible = accepted(AcceptedArgs {
            revision: 2,
            solution_suffix: 0x12,
            score: 5,
            assignments: Vec::new(),
            rules: Vec::new(),
            category: None,
            metrics: BTreeMap::new(),
            scenario,
            pack: "official.other",
            projection_version: 2,
        })?;
        assert!(matches!(
            compare_accepted_results(&minimum, &incompatible, None),
            Err(ComparisonError::Incompatible)
        ));
        assert!(matches!(
            compare_semantic_accepted_results(&minimum, &incompatible, None),
            Err(ComparisonError::Incompatible)
        ));
        Ok(())
    }

    #[test]
    fn semantic_comparison_allows_score_shape_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let (base, candidate) = pair(10, 5)?;
        let mut levels = candidate.verification.score.levels.clone();
        levels.push(ScoreLevelValue {
            level_id: ScoreLevelId::new("tests.secondary")?,
            value: 99,
            direction: OptimizationDirection::Minimize,
            category_breakdown: BTreeMap::new(),
        });
        let score = ScoreVector {
            feasibility: candidate.verification.score.feasibility,
            levels,
        };
        let context = VerificationContextV1::new(
            candidate.solution.scenario_id,
            candidate.solution.scenario_revision,
            candidate.verification.document_hash.clone(),
            candidate.verification.planning_model_hash.clone(),
            candidate.solution.canonical_hash()?,
            candidate.verification.verification_scope_checksum.clone(),
        )?;
        let report = VerificationReport::new(
            &context,
            candidate.verification.required_rule_results.clone(),
            score,
            candidate.verification.warnings.clone(),
            candidate.verification.metrics.clone(),
        )?;
        let candidate = AcceptedResult::new(candidate.solution.clone(), report)?;

        assert!(matches!(
            compare_accepted_results(&base, &candidate, None),
            Err(ComparisonError::Incompatible)
        ));
        let difference = compare_semantic_accepted_results(&base, &candidate, None)?;
        assert_eq!(difference.assignments.len(), 3);
        assert_eq!(difference.rules.len(), 2);
        assert_eq!(difference.metrics.len(), 3);
        let wire = serde_json::to_value(&difference)?;
        assert!(wire.get("baseScore").is_none());
        assert!(wire.get("candidateScore").is_none());
        assert!(wire.get("ordering").is_none());
        assert!(wire.get("locks").is_none());
        difference.validate()?;
        Ok(())
    }

    #[test]
    fn semantic_comparison_preserves_canonical_changes_and_checksum()
    -> Result<(), Box<dyn std::error::Error>> {
        let (base, candidate) = pair(10, 5)?;
        let difference = compare_semantic_accepted_results(&base, &candidate, None)?;
        assert!(matches!(
            difference.assignments[0],
            AssignmentComparisonV1::Removed { .. }
        ));
        assert_eq!(
            difference.affected_entities,
            vec![entity("a")?, entity("b")?, entity("c")?]
        );
        let encoded = serde_json::to_vec(&difference)?;
        assert_eq!(SemanticDifferenceV1::from_json(&encoded)?, difference);

        let mut tampered = difference.clone();
        tampered.checksum = hash("tampered");
        assert!(matches!(
            tampered.validate(),
            Err(DomainContractError::ChecksumMismatch)
        ));

        let mut noncanonical = difference;
        noncanonical.affected_entities.reverse();
        assert!(matches!(
            noncanonical.validate(),
            Err(DomainContractError::NonCanonicalExplanationCollection(
                "comparison affected entities"
            ))
        ));
        Ok(())
    }

    #[test]
    fn semantic_comparison_rejects_malformed_manifest_and_oversized_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let (base, candidate) = pair(10, 5)?;
        let mut base_manifest = manifest(0x30, &base, SolveStatus::Feasible)?;
        base_manifest.checksum = hash("tampered");
        let candidate_manifest = manifest(0x31, &candidate, SolveStatus::Optimal)?;
        let context = ComparisonContext {
            locks: &[],
            manifests: Some(ComparisonRunManifests {
                base: &base_manifest,
                candidate: &candidate_manifest,
            }),
        };
        assert!(matches!(
            compare_semantic_accepted_results(&base, &candidate, Some(&context)),
            Err(ComparisonError::InvalidManifest {
                side: ComparisonSide::Base,
                source: Some(DomainContractError::ChecksumMismatch),
            })
        ));

        let valid = compare_semantic_accepted_results(&base, &candidate, None)?;
        let mut oversized = valid.clone();
        oversized.affected_entities = (0..=crate::MAX_VERIFICATION_ENTITIES)
            .map(
                |index| -> Result<DomainEntityRef, Box<dyn std::error::Error>> {
                    Ok(DomainEntityRef {
                        kind: DomainEntityKindId::new("tests.person")?,
                        id: DomainEntityId::new(format!("tests.entity-{index}"))?,
                    })
                },
            )
            .collect::<Result<_, _>>()?;
        oversized.affected_entities.sort();
        assert!(matches!(
            oversized.validate(),
            Err(DomainContractError::LimitExceeded(
                "comparison affected entities"
            ))
        ));
        Ok(())
    }
}
