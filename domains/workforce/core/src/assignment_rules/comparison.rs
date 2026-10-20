use super::{
    AssignmentRuleError, MAX_SELECTED_PAIRS,
    authority::{operation_error, validate_pair},
    boundary::{DOMAIN_LIMITS, preflight},
    budget::{OperationBudget, count},
    identity::{ASSIGNMENT_KIND, RANK_CATEGORY, RANK_LEVEL, WORKFORCE_PROJECTION_VERSION},
    input::AssignmentInput,
    preference::{self, PreferencePlan, PreferenceVariant, WorkloadTargetKind},
    profile::{self, PrefixTier},
    projection::{decode_workforce_assignment, decode_workforce_assignment_id},
};
use crate::{
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    ids::{AssignmentTypeId, LocationId, ShiftId, TeamId},
    model::{AssignmentPair, Coverage, LockState, WorkforceEntity},
    temporal::ResolvedShiftOrigin,
};
use eutheto_domain_api::DomainPackError;
use eutheto_domain_ir::{
    ACCEPTED_RESULT_SCHEMA_VERSION, AssignmentProtectionComparisonV1, AssignmentProtectionStateV1,
    ComparisonEnrichmentV2, ComparisonEntityObservationV1, ComparisonScoringContext,
    DomainAssignmentId, DomainEntityId, DomainEntityKindId, DomainEntityRef, DomainEvidenceId,
    MAX_EXPLANATION_RECORDS, NORMALIZED_SOLUTION_SCHEMA_VERSION,
    PORTABLE_ACCEPTED_RESULT_SCHEMA_VERSION, PortableAcceptedResult, PreparedComparisonV2,
    RUN_INPUT_SCHEMA_VERSION, RUN_MANIFEST_SCHEMA_VERSION, RUN_NATIVE_OBSERVATIONS_SCHEMA_VERSION,
    RUN_OBJECTIVE_CONTEXT_SCHEMA_VERSION, RunObjectiveContext, VERIFICATION_REPORT_SCHEMA_VERSION,
};
use eutheto_planning_ir::{PLANNING_IR_SCHEMA_VERSION, PlanningIrLimitsV1};
use eutheto_types::{EntityId, OperationControl, PersonId, ScenarioDocument, ScenarioId};
use serde::Serialize;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
};

const SCORING_CONTEXT_PRODUCER: &str = "official.workforce.comparison.scoring-context.v1";
const SCORING_CONTEXT_DOMAIN: &[u8] = b"eutheto/workforce/comparison-scoring-context/v1\0";

/// Derives Workforce-owned comparison facts from two exact accepted snapshots.
///
/// The comparison caller has independently admitted both exact accepted sources. This helper
/// projects Workforce protection, party and source-content scoring facts only. It does not
/// approve, select, or repair a result.
///
/// # Errors
/// Rejects invalid source bindings, unsupported Workforce policy, exceeded bounds, or interruption.
pub fn workforce_comparison_enrichment<'a>(
    base_document: &ScenarioDocument,
    base: &'a PortableAcceptedResult,
    candidate_document: &ScenarioDocument,
    candidate: &'a PortableAcceptedResult,
    prepared: &PreparedComparisonV2<'_>,
    control: Option<&OperationControl>,
) -> Result<ComparisonEnrichmentV2<'a>, DomainPackError> {
    let mut budget = OperationBudget::evaluation(control);
    validate_portable(base, &mut budget)?;
    validate_portable(candidate, &mut budget)?;

    let base_input = AssignmentInput::new(base_document, &mut budget)
        .map_err(|error| operation_error(&error))?
        .with_objective_context(
            base.run_input.objective_context.as_ref(),
            base_document.scenario_id,
            &mut budget,
        )?;
    let candidate_input = AssignmentInput::new(candidate_document, &mut budget)
        .map_err(|error| operation_error(&error))?
        .with_objective_context(
            candidate.run_input.objective_context.as_ref(),
            candidate_document.scenario_id,
            &mut budget,
        )?;
    validate_source_binding(base, base_document, &base_input)?;
    validate_source_binding(candidate, candidate_document, &candidate_input)?;
    let base_selected = selected_pairs(base, &base_input, &mut budget)?;
    let candidate_selected = selected_pairs(candidate, &candidate_input, &mut budget)?;
    enrich_prepared(
        base_document,
        base,
        &base_input,
        &base_selected,
        candidate_document,
        candidate,
        &candidate_input,
        &candidate_selected,
        prepared,
        &mut budget,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn enrich_prepared<'a>(
    base_document: &ScenarioDocument,
    base: &'a PortableAcceptedResult,
    base_input: &AssignmentInput,
    base_selected: &BTreeSet<AssignmentPair>,
    candidate_document: &ScenarioDocument,
    candidate: &'a PortableAcceptedResult,
    candidate_input: &AssignmentInput,
    candidate_selected: &BTreeSet<AssignmentPair>,
    prepared: &PreparedComparisonV2<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<ComparisonEnrichmentV2<'a>, DomainPackError> {
    let protections = protections(
        base_input,
        candidate_input,
        base_selected,
        candidate_selected,
        budget,
    )?;
    let affected_parties =
        affected_parties(base_input, candidate_input, prepared, &protections, budget)?;
    let scoring_contexts = if has_current_scoring_context(base, base_document)
        && has_current_scoring_context(candidate, candidate_document)
    {
        let base_hash = scoring_semantic_hash(
            base_document,
            base_input,
            base.run_input.objective_context.as_ref(),
            budget,
        )?;
        let candidate_hash = scoring_semantic_hash(
            candidate_document,
            candidate_input,
            candidate.run_input.objective_context.as_ref(),
            budget,
        )?;
        let producer_id = DomainEvidenceId::new(SCORING_CONTEXT_PRODUCER)
            .map_err(|_| contract("comparison.producer"))?;
        budget
            .reserve(0, 2, 256)
            .map_err(|error| operation_error(&error))?;
        Some((
            ComparisonScoringContext {
                source: base,
                producer_id: producer_id.clone(),
                semantic_hash: base_hash,
            },
            ComparisonScoringContext {
                source: candidate,
                producer_id,
                semantic_hash: candidate_hash,
            },
        ))
    } else {
        None
    };
    Ok(ComparisonEnrichmentV2 {
        protections,
        affected_parties,
        scoring_contexts,
    })
}
/// Validates portable custody after charging bounded checksum scratch.
pub(super) fn validate_portable(
    source: &PortableAcceptedResult,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    budget.check().map_err(|error| operation_error(&error))?;
    if !matches!(source.schema_version, 2 | 3) {
        return Err(DomainPackError::UnsupportedVersion(source.schema_version));
    }
    if !matches!(
        source.run_input.schema_version,
        1 | RUN_INPUT_SCHEMA_VERSION
    ) {
        return Err(DomainPackError::UnsupportedVersion(
            source.run_input.schema_version,
        ));
    }
    if !matches!(
        source.run_manifest.schema_version,
        1 | RUN_MANIFEST_SCHEMA_VERSION
    ) {
        return Err(DomainPackError::UnsupportedVersion(
            source.run_manifest.schema_version,
        ));
    }
    if source.accepted_result.schema_version != ACCEPTED_RESULT_SCHEMA_VERSION {
        return Err(DomainPackError::UnsupportedVersion(
            source.accepted_result.schema_version,
        ));
    }
    if source.accepted_result.solution.schema_version != NORMALIZED_SOLUTION_SCHEMA_VERSION {
        return Err(DomainPackError::UnsupportedVersion(
            source.accepted_result.solution.schema_version,
        ));
    }
    if source.accepted_result.verification.schema_version != VERIFICATION_REPORT_SCHEMA_VERSION {
        return Err(DomainPackError::UnsupportedVersion(
            source.accepted_result.verification.schema_version,
        ));
    }
    if source
        .run_input
        .objective_context
        .as_ref()
        .is_some_and(|context| {
            !matches!(
                context.schema_version,
                1 | 2 | RUN_OBJECTIVE_CONTEXT_SCHEMA_VERSION
            )
        })
    {
        return Err(DomainPackError::UnsupportedVersion(
            source
                .run_input
                .objective_context
                .as_ref()
                .map_or(0, |context| context.schema_version),
        ));
    }
    if source
        .run_manifest
        .native_observations
        .as_ref()
        .is_some_and(|observations| {
            observations.schema_version != RUN_NATIVE_OBSERVATIONS_SCHEMA_VERSION
        })
    {
        return Err(DomainPackError::UnsupportedVersion(
            source
                .run_manifest
                .native_observations
                .as_ref()
                .map_or(0, |observations| observations.schema_version),
        ));
    }
    if source.run_input.pack_id.as_str() != WORKFORCE_PACK_ID {
        return Err(contract("comparison.pack"));
    }

    reserve_portable_checksum_scratch(&source.run_input, budget)?;
    if let Some(observations) = &source.run_manifest.native_observations {
        reserve_portable_checksum_scratch(observations, budget)?;
    }
    reserve_portable_checksum_scratch(&source.run_manifest, budget)?;
    reserve_portable_checksum_scratch(&source.accepted_result.solution, budget)?;
    reserve_portable_checksum_scratch(&source.accepted_result.verification, budget)?;
    reserve_portable_checksum_scratch(&source.accepted_result, budget)?;
    reserve_portable_checksum_scratch(source, budget)?;

    source.validate().map_err(|error| match error {
        eutheto_domain_ir::DomainContractError::UnsupportedVersion(version) => {
            DomainPackError::UnsupportedVersion(version)
        }
        _ => contract("comparison.portable-result"),
    })?;
    budget.check().map_err(|error| operation_error(&error))
}

fn reserve_portable_checksum_scratch<T: Serialize + ?Sized>(
    value: &T,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    preflight(value, budget, DOMAIN_LIMITS)
        .map_err(|error| operation_error(&error))?
        .reserve_json(budget, 1)
        .map_err(|error| operation_error(&error))
}

fn has_current_scoring_context(
    source: &PortableAcceptedResult,
    document: &ScenarioDocument,
) -> bool {
    let context = source.run_input.objective_context.as_ref();
    matches!(
        profile::ProfileKind::parse(context, document.scenario_id),
        Ok(Some(_))
    ) && source.schema_version == PORTABLE_ACCEPTED_RESULT_SCHEMA_VERSION
        && source.run_input.schema_version == RUN_INPUT_SCHEMA_VERSION
        && source.run_input.pack_schema_version == document.domain_pack.schema_version
        && source.run_input.pack_schema_version == 1
        && source.run_input.planning_ir_schema_version == PLANNING_IR_SCHEMA_VERSION
        && source.run_input.compiler_version == env!("CARGO_PKG_VERSION")
        && source.run_manifest.schema_version == RUN_MANIFEST_SCHEMA_VERSION
        && source.accepted_result.schema_version == ACCEPTED_RESULT_SCHEMA_VERSION
        && source.accepted_result.solution.schema_version == NORMALIZED_SOLUTION_SCHEMA_VERSION
        && source.accepted_result.solution.projection_version == WORKFORCE_PROJECTION_VERSION
        && source.accepted_result.verification.schema_version == VERIFICATION_REPORT_SCHEMA_VERSION
}

pub(super) fn validate_source_binding(
    source: &PortableAcceptedResult,
    document: &ScenarioDocument,
    input: &AssignmentInput,
) -> Result<(), DomainPackError> {
    if document.domain_pack.id.as_str() != WORKFORCE_PACK_ID
        || source.run_input.pack_id != document.domain_pack.id
        || source.run_input.pack_schema_version != document.domain_pack.schema_version
        || source.run_input.scenario_id != document.scenario_id
        || source.accepted_result.solution.pack_id != document.domain_pack.id
        || source.accepted_result.solution.scenario_id != document.scenario_id
        || source.accepted_result.verification.scenario_id != document.scenario_id
        || source.accepted_result.verification.evaluated_revision
            != source.run_input.scenario_revision
        || source.run_input.snapshot_document_hash != input.source_document_hash
        || source.accepted_result.verification.document_hash != input.source_document_hash
    {
        return Err(contract("comparison.source-binding"));
    }
    Ok(())
}

pub(super) fn selected_pairs(
    source: &PortableAcceptedResult,
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
) -> Result<BTreeSet<AssignmentPair>, DomainPackError> {
    let mut selected = BTreeSet::new();
    for assignment in &source.accepted_result.solution.assignments {
        budget.step().map_err(|error| operation_error(&error))?;
        let (pair, is_selected) = decode_workforce_assignment(assignment)?;
        validate_pair(input, pair)?;
        if !is_selected {
            continue;
        }
        if count(selected.len()).map_err(|error| operation_error(&error))? >= MAX_SELECTED_PAIRS {
            return Err(DomainPackError::ResourceLimitExceeded);
        }
        budget
            .reserve(1, 1, 32)
            .map_err(|error| operation_error(&error))?;
        if !selected.insert(pair) {
            return Err(contract("comparison.duplicate-selection"));
        }
    }
    Ok(selected)
}
fn protections(
    base: &AssignmentInput,
    candidate: &AssignmentInput,
    base_selected: &BTreeSet<AssignmentPair>,
    candidate_selected: &BTreeSet<AssignmentPair>,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<AssignmentProtectionComparisonV1>, DomainPackError> {
    let mut union = BTreeMap::<AssignmentPair, [Option<AssignmentProtectionStateV1>; 2]>::new();
    for (side, input) in [(0, base), (1, candidate)] {
        for lock in input.domain.locked_assignments.values() {
            budget.step().map_err(|error| operation_error(&error))?;
            let pair = AssignmentPair {
                person_id: lock.person_id,
                shift_id: lock.shift_id,
            };
            let state = protection_state(lock.state);
            if !union.contains_key(&pair) {
                if union.len() >= MAX_EXPLANATION_RECORDS {
                    return Err(DomainPackError::ResourceLimitExceeded);
                }
                budget
                    .reserve(1, 1, 64)
                    .map_err(|error| operation_error(&error))?;
            }
            let states = union.entry(pair).or_insert([None, None]);
            if states[side].replace(state).is_some() {
                return Err(contract("comparison.duplicate-protection"));
            }
        }
    }

    let mut result = Vec::new();
    budget
        .reserve(
            0,
            count(union.len()).map_err(|error| operation_error(&error))?,
            count(
                union
                    .len()
                    .checked_mul(size_of::<AssignmentProtectionComparisonV1>())
                    .ok_or(DomainPackError::ResourceLimitExceeded)?,
            )
            .map_err(|error| operation_error(&error))?,
        )
        .map_err(|error| operation_error(&error))?;
    result.reserve(union.len());
    for (pair, [before, after]) in union {
        budget.step().map_err(|error| operation_error(&error))?;
        let before_selected = base_selected.contains(&pair);
        let after_selected = candidate_selected.contains(&pair);
        let protection = AssignmentProtectionComparisonV1::new(
            assignment_id(pair)?,
            before,
            after,
            before_selected,
            after_selected,
        )
        .map_err(|_| contract("comparison.protection"))?;
        result.push(protection);
    }
    Ok(result)
}

fn protection_state(state: LockState) -> AssignmentProtectionStateV1 {
    match state {
        LockState::Hard {} => AssignmentProtectionStateV1::Hard,
        LockState::Soft { stability_weight } => AssignmentProtectionStateV1::Soft {
            weight: stability_weight,
        },
        LockState::Unlocked {} => AssignmentProtectionStateV1::Unlocked,
    }
}

pub(super) fn assignment_id(pair: AssignmentPair) -> Result<DomainAssignmentId, DomainPackError> {
    DomainAssignmentId::new(format!(
        "{ASSIGNMENT_KIND}.{}.{}",
        pair.person_id, pair.shift_id
    ))
    .map_err(|_| contract("comparison.assignment-id"))
}

fn affected_parties(
    base: &AssignmentInput,
    candidate: &AssignmentInput,
    prepared: &PreparedComparisonV2<'_>,
    protections: &[AssignmentProtectionComparisonV1],
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<ComparisonEntityObservationV1>, DomainPackError> {
    let generic = prepared.affected_entities();
    let mut observations = BTreeMap::<DomainEntityRef, [Option<String>; 2]>::new();
    let mut additional_entities = 0_usize;
    for entity in generic {
        add_source_label(
            base,
            entity,
            0,
            generic,
            &mut additional_entities,
            &mut observations,
            budget,
        )?;
        add_source_label(
            candidate,
            entity,
            1,
            generic,
            &mut additional_entities,
            &mut observations,
            budget,
        )?;
    }

    let mut pairs = BTreeSet::new();
    for delta in prepared.assignments() {
        budget.step().map_err(|error| operation_error(&error))?;
        let pair = decode_workforce_assignment_id(delta.assignment_id())?;
        insert_pair(&mut pairs, pair, budget)?;
    }
    for protection in protections {
        budget.step().map_err(|error| operation_error(&error))?;
        if protection.before != protection.after
            || protection.before_selected != protection.after_selected
        {
            let pair = decode_workforce_assignment_id(&protection.assignment_id)?;
            insert_pair(&mut pairs, pair, budget)?;
        }
    }
    for pair in pairs {
        project_pair_side(
            base,
            pair,
            0,
            generic,
            &mut additional_entities,
            &mut observations,
            budget,
        )?;
        project_pair_side(
            candidate,
            pair,
            1,
            generic,
            &mut additional_entities,
            &mut observations,
            budget,
        )?;
    }

    resolve_source_labels(base, candidate, &mut observations, budget)?;

    let mut result = Vec::new();
    budget
        .reserve(
            0,
            count(observations.len()).map_err(|error| operation_error(&error))?,
            count(
                observations
                    .len()
                    .checked_mul(size_of::<ComparisonEntityObservationV1>())
                    .ok_or(DomainPackError::ResourceLimitExceeded)?,
            )
            .map_err(|error| operation_error(&error))?,
        )
        .map_err(|error| operation_error(&error))?;
    result.reserve(observations.len());
    for (entity, [before_label, after_label]) in observations {
        budget.step().map_err(|error| operation_error(&error))?;
        if before_label.is_some() || after_label.is_some() {
            result.push(ComparisonEntityObservationV1 {
                entity,
                before_label,
                after_label,
            });
        }
    }
    Ok(result)
}

fn resolve_source_labels(
    base: &AssignmentInput,
    candidate: &AssignmentInput,
    observations: &mut BTreeMap<DomainEntityRef, [Option<String>; 2]>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    for (entity, labels) in observations {
        for (input, side) in [(base, 0), (candidate, 1)] {
            budget.step().map_err(|error| operation_error(&error))?;
            // Populated sides already passed source-label conflict checks before the union froze.
            if labels[side].is_some() {
                continue;
            }
            if let Some(label) = source_label(input, entity) {
                if label.is_empty() || label.len() > eutheto_domain_ir::MAX_VERIFICATION_TEXT_BYTES
                {
                    return Err(contract("comparison.entity-label"));
                }
                budget
                    .reserve(
                        0,
                        1,
                        count(label.len() + 8).map_err(|error| operation_error(&error))?,
                    )
                    .map_err(|error| operation_error(&error))?;
                labels[side] = Some(label.into_owned());
            }
        }
    }
    Ok(())
}

fn insert_pair(
    pairs: &mut BTreeSet<AssignmentPair>,
    pair: AssignmentPair,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    if pairs.contains(&pair) {
        return Ok(());
    }
    if pairs.len() >= MAX_EXPLANATION_RECORDS {
        return Err(DomainPackError::ResourceLimitExceeded);
    }
    budget
        .reserve(1, 1, 32)
        .map_err(|error| operation_error(&error))?;
    pairs.insert(pair);
    Ok(())
}

fn project_pair_side(
    input: &AssignmentInput,
    pair: AssignmentPair,
    side: usize,
    generic: &BTreeSet<DomainEntityRef>,
    additional_entities: &mut usize,
    observations: &mut BTreeMap<DomainEntityRef, [Option<String>; 2]>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    if let Some(person) = input.person(pair.person_id) {
        add_label(
            entity_ref("person", EntityId::from_uuid(pair.person_id.as_uuid()))?,
            side,
            Cow::Borrowed(&person.name),
            generic,
            additional_entities,
            observations,
            budget,
        )?;
        for team_id in &person.team_ids {
            budget.step().map_err(|error| operation_error(&error))?;
            if let Some(WorkforceEntity::Team(team)) =
                input.domain.entities.get(&team_id.as_entity_id())
            {
                add_label(
                    entity_ref("team", team.id.as_entity_id())?,
                    side,
                    Cow::Borrowed(&team.name),
                    generic,
                    additional_entities,
                    observations,
                    budget,
                )?;
            }
        }
        if let Some(location_id) = person.home_location_id {
            add_location_label(
                input,
                location_id,
                side,
                generic,
                additional_entities,
                observations,
                budget,
            )?;
        }
    }
    if let Some(shift) = input.shift(pair.shift_id) {
        add_label(
            entity_ref("shift", pair.shift_id.as_entity_id())?,
            side,
            shift_label(input, shift.origin, shift.reporting_date),
            generic,
            additional_entities,
            observations,
            budget,
        )?;
        let metadata = input
            .metadata(shift)
            .map_err(|error| operation_error(&error))?;
        add_label(
            entity_ref(
                "assignment_type",
                metadata.assignment_type.id.as_entity_id(),
            )?,
            side,
            Cow::Borrowed(&metadata.assignment_type.name),
            generic,
            additional_entities,
            observations,
            budget,
        )?;
        if let Some(location_id) = metadata.location_id {
            add_location_label(
                input,
                location_id,
                side,
                generic,
                additional_entities,
                observations,
                budget,
            )?;
        }
    }
    Ok(())
}

fn shift_label(
    input: &AssignmentInput,
    origin: ResolvedShiftOrigin,
    reporting_date: jiff::civil::Date,
) -> Cow<'static, str> {
    let (template_id, occurrence_date) = match origin {
        ResolvedShiftOrigin::Generated {
            template_id,
            occurrence_date,
        }
        | ResolvedShiftOrigin::Detached {
            template_id,
            occurrence_date,
        } => (Some(template_id), occurrence_date),
        ResolvedShiftOrigin::Manual => (None, reporting_date),
    };
    match template_id.and_then(|id| input.domain.entities.get(&id.as_entity_id())) {
        Some(WorkforceEntity::ShiftTemplate(template)) => {
            Cow::Owned(format!("{} — {occurrence_date}", template.name))
        }
        _ => Cow::Owned(format!("Shift — {occurrence_date}")),
    }
}

fn add_location_label(
    input: &AssignmentInput,
    location_id: LocationId,
    side: usize,
    generic: &BTreeSet<DomainEntityRef>,
    additional_entities: &mut usize,
    observations: &mut BTreeMap<DomainEntityRef, [Option<String>; 2]>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    if let Some(WorkforceEntity::Location(location)) =
        input.domain.entities.get(&location_id.as_entity_id())
    {
        add_label(
            entity_ref("location", location.id.as_entity_id())?,
            side,
            Cow::Borrowed(&location.name),
            generic,
            additional_entities,
            observations,
            budget,
        )?;
    }
    Ok(())
}

fn add_source_label(
    input: &AssignmentInput,
    entity: &DomainEntityRef,
    side: usize,
    generic: &BTreeSet<DomainEntityRef>,
    additional_entities: &mut usize,
    observations: &mut BTreeMap<DomainEntityRef, [Option<String>; 2]>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    if let Some(label) = source_label(input, entity) {
        add_label(
            entity.clone(),
            side,
            label,
            generic,
            additional_entities,
            observations,
            budget,
        )?;
    }
    Ok(())
}

fn source_label<'a>(input: &'a AssignmentInput, entity: &DomainEntityRef) -> Option<Cow<'a, str>> {
    let id = entity.id.as_str().strip_prefix("official.workforce.")?;
    match entity.kind.as_str() {
        "official.workforce.person" => id
            .parse::<PersonId>()
            .ok()
            .and_then(|id| input.person(id))
            .map(|person| Cow::Borrowed(person.name.as_str())),
        "official.workforce.shift" => id
            .parse::<ShiftId>()
            .ok()
            .and_then(|id| input.shift(id))
            .map(|shift| shift_label(input, shift.origin, shift.reporting_date)),
        "official.workforce.team" => id
            .parse::<TeamId>()
            .ok()
            .and_then(|id| input.domain.entities.get(&id.as_entity_id()))
            .and_then(|entity| match entity {
                WorkforceEntity::Team(team) => Some(Cow::Borrowed(team.name.as_str())),
                _ => None,
            }),
        "official.workforce.location" => id
            .parse::<LocationId>()
            .ok()
            .and_then(|id| input.domain.entities.get(&id.as_entity_id()))
            .and_then(|entity| match entity {
                WorkforceEntity::Location(location) => Some(Cow::Borrowed(location.name.as_str())),
                _ => None,
            }),
        "official.workforce.assignment_type" => id
            .parse::<AssignmentTypeId>()
            .ok()
            .and_then(|id| input.domain.entities.get(&id.as_entity_id()))
            .and_then(|entity| match entity {
                WorkforceEntity::AssignmentType(assignment_type) => {
                    Some(Cow::Borrowed(assignment_type.name.as_str()))
                }
                _ => None,
            }),
        _ => None,
    }
}

fn add_label(
    entity: DomainEntityRef,
    side: usize,
    label: Cow<'_, str>,
    generic: &BTreeSet<DomainEntityRef>,
    additional_entities: &mut usize,
    observations: &mut BTreeMap<DomainEntityRef, [Option<String>; 2]>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    if side > 1 || label.is_empty() || label.len() > eutheto_domain_ir::MAX_VERIFICATION_TEXT_BYTES
    {
        return Err(contract("comparison.entity-label"));
    }
    if observations
        .get(&entity)
        .and_then(|labels| labels[side].as_deref())
        .is_some_and(|existing| existing != label.as_ref())
    {
        return Err(contract("comparison.entity-label-conflict"));
    }
    let new_entity = !observations.contains_key(&entity);
    if new_entity {
        if !generic.contains(&entity) {
            if generic.len().saturating_add(*additional_entities) >= MAX_EXPLANATION_RECORDS {
                return Err(DomainPackError::ResourceLimitExceeded);
            }
            *additional_entities += 1;
        }
        budget
            .reserve(
                1,
                2,
                count(entity.kind.as_str().len() + entity.id.as_str().len() + label.len() + 96)
                    .map_err(|error| operation_error(&error))?,
            )
            .map_err(|error| operation_error(&error))?;
    } else if observations
        .get(&entity)
        .is_some_and(|labels| labels[side].is_some())
    {
        return Ok(());
    } else {
        budget
            .reserve(
                0,
                1,
                count(label.len() + 8).map_err(|error| operation_error(&error))?,
            )
            .map_err(|error| operation_error(&error))?;
    }
    let labels = observations.entry(entity).or_insert([None, None]);
    if labels[side].is_none() {
        labels[side] = Some(label.into_owned());
    }
    Ok(())
}

fn entity_ref(kind: &str, id: EntityId) -> Result<DomainEntityRef, DomainPackError> {
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new(format!("official.workforce.{kind}"))
            .map_err(|_| contract("comparison.entity-kind"))?,
        id: DomainEntityId::new(format!("official.workforce.{id}"))
            .map_err(|_| contract("comparison.entity-id"))?,
    })
}

fn scoring_semantic_hash(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    objective_context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<String, DomainPackError> {
    let plan = preference::prepare(
        document,
        input,
        objective_context,
        PlanningIrLimitsV1::DEFAULT,
        budget,
    )
    .map_err(|error| operation_error(&error))?;
    let shifts = sorted_shift_ids(input, budget)?;
    budget
        .reserve(
            0,
            0,
            count(SCORING_CONTEXT_DOMAIN.len()).map_err(|error| operation_error(&error))?,
        )
        .map_err(|error| operation_error(&error))?;
    let mut hash = SemanticHasher::new();
    match objective_context {
        None => hash.record(&("objective-context", "absent"), budget)?,
        Some(context) => hash.record(
            &(
                "objective-context",
                "authored",
                // Context v2 changes baseline storage, not objective meaning; keep v1 hashes.
                1_u32,
                context.pack_id.as_str(),
                context.profile_id.as_str(),
                context.semantic_version,
                context.operation,
            ),
            budget,
        )?,
    }
    hash_score_shape(&mut hash, &plan, budget)?;
    hash_rank_population(&mut hash, input, &shifts, budget)?;
    hash_prefix_costs(
        &mut hash,
        input,
        document.scenario_id,
        &plan,
        objective_context,
        budget,
    )?;
    hash_pair_preferences(&mut hash, document, input, &shifts, &plan, budget)?;
    hash_targets(&mut hash, &plan, budget)?;
    hash_groups(&mut hash, input, &plan, &document.settings, budget)?;
    hash_coverage(&mut hash, input, &shifts, &plan, &document.settings, budget)?;
    hash_workloads(&mut hash, input, &plan, budget)?;
    hash_consecutives(&mut hash, input, &plan, &document.settings, budget)?;
    hash.finish(budget)
}

fn sorted_shift_ids(
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<ShiftId>, DomainPackError> {
    budget
        .reserve(
            0,
            count(input.shifts.len()).map_err(|error| operation_error(&error))?,
            count(
                input
                    .shifts
                    .len()
                    .checked_mul(size_of::<ShiftId>())
                    .ok_or(DomainPackError::ResourceLimitExceeded)?,
            )
            .map_err(|error| operation_error(&error))?,
        )
        .map_err(|error| operation_error(&error))?;
    let mut shifts: Vec<_> = input.shifts.iter().map(|shift| shift.id).collect();
    budget
        .sort_work(shifts.len())
        .map_err(|error| operation_error(&error))?;
    shifts.sort_unstable();
    Ok(shifts)
}

fn hash_score_shape(
    hash: &mut SemanticHasher,
    plan: &PreferencePlan<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    hash.record(&("score-shape", "feasibility", "minimize"), budget)?;
    for (index, tier) in plan.layout.prefix.iter().enumerate() {
        budget.step().map_err(|error| operation_error(&error))?;
        hash.record(
            &(
                "objective",
                index,
                tier.id(),
                tier.category(),
                "minimize",
                match tier {
                    PrefixTier::PairChange => "assignment-pair count",
                    PrefixTier::SoftLocks => "soft-lock weight",
                },
            ),
            budget,
        )?;
    }
    let offset = plan.layout.prefix.len();
    for (index, level) in plan.layout.levels.iter().enumerate() {
        budget.step().map_err(|error| operation_error(&error))?;
        hash.record(
            &(
                "objective",
                offset + index,
                &level.key,
                &level.id,
                "minimize",
            ),
            budget,
        )?;
    }
    hash.record(
        &(
            "objective",
            offset + plan.layout.levels.len(),
            RANK_LEVEL,
            RANK_CATEGORY,
            "minimize",
            "stable-assignment rank sum",
        ),
        budget,
    )
}

fn hash_rank_population(
    hash: &mut SemanticHasher,
    input: &AssignmentInput,
    shifts: &[ShiftId],
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    hash.record(
        &("rank-population", input.people.len(), shifts.len()),
        budget,
    )?;
    for (index, person) in input.people.iter().enumerate() {
        budget.step().map_err(|error| operation_error(&error))?;
        hash.record(&("rank-person", index, person), budget)?;
    }
    for (index, shift) in shifts.iter().enumerate() {
        budget.step().map_err(|error| operation_error(&error))?;
        hash.record(&("rank-shift", index, shift), budget)?;
    }
    Ok(())
}

fn hash_prefix_costs(
    hash: &mut SemanticHasher,
    input: &AssignmentInput,
    scenario_id: ScenarioId,
    plan: &PreferencePlan<'_>,
    objective_context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    if plan
        .layout
        .prefix
        .iter()
        .any(|tier| matches!(tier, PrefixTier::PairChange))
    {
        let pairs = profile::sorted_baseline_pairs(input, scenario_id, objective_context, budget)?
            .ok_or_else(|| contract("comparison.baseline-required"))?;
        hash.record(&("pair-change-baseline", pairs.len()), budget)?;
        for index in 0..pairs.len() {
            budget.step().map_err(|error| operation_error(&error))?;
            hash.record(&("pair-change-reference", pairs.pair(index)), budget)?;
        }
    }
    if plan
        .layout
        .prefix
        .iter()
        .any(|tier| matches!(tier, PrefixTier::SoftLocks))
    {
        let mut soft = Vec::new();
        for lock in input.domain.locked_assignments.values() {
            budget.step().map_err(|error| operation_error(&error))?;
            let LockState::Soft { stability_weight } = lock.state else {
                continue;
            };
            budget
                .reserve(1, 1, 40)
                .map_err(|error| operation_error(&error))?;
            soft.push((
                AssignmentPair {
                    person_id: lock.person_id,
                    shift_id: lock.shift_id,
                },
                stability_weight,
            ));
        }
        budget
            .sort_work(soft.len())
            .map_err(|error| operation_error(&error))?;
        soft.sort_unstable();
        hash.record(&("soft-lock-costs", soft.len()), budget)?;
        for (pair, weight) in soft {
            budget.step().map_err(|error| operation_error(&error))?;
            hash.record(&("soft-lock-cost", pair, weight), budget)?;
        }
    }
    Ok(())
}

fn hash_pair_preferences(
    hash: &mut SemanticHasher,
    document: &ScenarioDocument,
    input: &AssignmentInput,
    shift_ids: &[ShiftId],
    plan: &PreferencePlan<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    for entry in &plan.entries {
        budget.step().map_err(|error| operation_error(&error))?;
        hash.record(
            &(
                "pair-preference",
                preference::category(entry.variant),
                entry.level,
                entry.coefficient,
                entry.direction,
            ),
            budget,
        )?;
        for person in &input.people {
            budget.step().map_err(|error| operation_error(&error))?;
            for shift_id in shift_ids {
                budget.step().map_err(|error| operation_error(&error))?;
                let pair = AssignmentPair {
                    person_id: *person,
                    shift_id: *shift_id,
                };
                if !preference::matches(input, *entry, pair, &document.settings, budget)
                    .map_err(|error| operation_error(&error))?
                {
                    continue;
                }
                let base_member = if matches!(entry.variant, PreferenceVariant::BaseStability) {
                    preference::base_contains(*entry, pair, budget)
                        .map_err(|error| operation_error(&error))?
                } else {
                    false
                };
                let feasible =
                    preference::statically_feasible(input, pair, &document.settings, budget)
                        .map_err(|error| operation_error(&error))?;
                if !feasible && !base_member {
                    continue;
                }
                let absent_cost = preference::pair_penalty(*entry, false, base_member)
                    .map_err(|error| operation_error(&error))?;
                let present_cost = if feasible {
                    Some(
                        preference::pair_penalty(*entry, true, base_member)
                            .map_err(|error| operation_error(&error))?,
                    )
                } else {
                    None
                };
                hash.record(
                    &(
                        "pair-opportunity",
                        pair,
                        feasible,
                        base_member,
                        absent_cost,
                        present_cost,
                    ),
                    budget,
                )?;
            }
        }
    }
    Ok(())
}

fn hash_targets(
    hash: &mut SemanticHasher,
    plan: &PreferencePlan<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    for target in &plan.targets {
        hash.record(
            &(
                "assignment-target",
                target.level,
                target.coefficient,
                target.target_count,
                target.periods.len(),
            ),
            budget,
        )?;
        for period in &target.periods {
            budget.step().map_err(|error| operation_error(&error))?;
            hash.record(
                &(
                    "assignment-target-period",
                    &period.person,
                    &period.period,
                    period.pairs.len(),
                ),
                budget,
            )?;
            for pair in &period.pairs {
                budget.step().map_err(|error| operation_error(&error))?;
                hash.record(&("assignment-target-pair", pair), budget)?;
            }
        }
    }
    for target in &plan.weekend_targets {
        hash.record(
            &(
                "weekend-target",
                target.level,
                target.coefficient,
                target.target_count,
                target.people.len(),
            ),
            budget,
        )?;
        for person in &target.people {
            budget.step().map_err(|error| operation_error(&error))?;
            hash.record(
                &(
                    "weekend-target-person",
                    person.person,
                    person.weekends.len(),
                ),
                budget,
            )?;
            for weekend in &person.weekends {
                budget.step().map_err(|error| operation_error(&error))?;
                hash.record(&("weekend", weekend.date, weekend.pairs.len()), budget)?;
                for pair in &weekend.pairs {
                    budget.step().map_err(|error| operation_error(&error))?;
                    hash.record(&("weekend-pair", pair), budget)?;
                }
            }
        }
    }
    Ok(())
}

fn hash_groups(
    hash: &mut SemanticHasher,
    input: &AssignmentInput,
    plan: &PreferencePlan<'_>,
    settings: &eutheto_types::ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    for group in &plan.groups {
        let mut opportunities = preference::group_opportunities(input, group, settings, budget)
            .map_err(|error| operation_error(&error))?;
        budget
            .sort_work(opportunities.len())
            .map_err(|error| operation_error(&error))?;
        opportunities.sort_unstable();
        hash.record(
            &(
                "group-objective",
                group.level,
                group.coefficient,
                group.direction,
                group.mode,
                opportunities.len(),
            ),
            budget,
        )?;
        for pair in &opportunities {
            budget.step().map_err(|error| operation_error(&error))?;
            hash.record(&("group-opportunity", pair), budget)?;
        }
        for left in 0..opportunities.len() {
            budget.step().map_err(|error| operation_error(&error))?;
            for right in left + 1..opportunities.len() {
                budget.step().map_err(|error| operation_error(&error))?;
                let first = opportunities[left];
                let second = opportunities[right];
                if first.person_id == second.person_id {
                    continue;
                }
                let relation = preference::relation_matches(input, first, second, group.mode)
                    .map_err(|error| operation_error(&error))?;
                hash.record(&("group-relation", first, second, relation), budget)?;
            }
        }
    }
    Ok(())
}

fn hash_coverage(
    hash: &mut SemanticHasher,
    input: &AssignmentInput,
    shift_ids: &[ShiftId],
    plan: &PreferencePlan<'_>,
    settings: &eutheto_types::ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    for coverage in &plan.coverage {
        let mut effective_count = 0usize;
        for requirement_id in coverage.coverage_requirement_ids {
            budget.step().map_err(|error| operation_error(&error))?;
            if preferred_coverage_count(input, requirement_id.as_entity_id())?.is_some() {
                effective_count = effective_count
                    .checked_add(1)
                    .ok_or(DomainPackError::ResourceLimitExceeded)?;
            }
        }
        hash.record(
            &(
                "preferred-coverage",
                coverage.level,
                coverage.coefficient,
                effective_count,
            ),
            budget,
        )?;
        for requirement_id in coverage.coverage_requirement_ids {
            budget.step().map_err(|error| operation_error(&error))?;
            hash_coverage_requirement(
                hash,
                input,
                shift_ids,
                coverage,
                requirement_id.as_entity_id(),
                settings,
                budget,
            )?;
        }
    }
    Ok(())
}

fn hash_coverage_requirement(
    hash: &mut SemanticHasher,
    input: &AssignmentInput,
    shift_ids: &[ShiftId],
    coverage: &preference::CoveragePlan<'_>,
    requirement_id: EntityId,
    settings: &eutheto_types::ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    let Some(preferred_count) = preferred_coverage_count(input, requirement_id)? else {
        return Ok(());
    };
    let Some(WorkforceEntity::CoverageRequirement(requirement)) =
        input.domain.entities.get(&requirement_id)
    else {
        return Err(contract("comparison.coverage-requirement"));
    };
    hash.record(
        &("preferred-coverage-requirement", "active", preferred_count),
        budget,
    )?;
    for shift_id in shift_ids {
        budget.step().map_err(|error| operation_error(&error))?;
        let shift = input
            .shift(*shift_id)
            .ok_or_else(|| contract("comparison.shift"))?;
        let metadata = input
            .metadata(shift)
            .map_err(|error| operation_error(&error))?;
        if !preference::requirement_shift_matches(&requirement.scope, shift, &metadata, budget)
            .map_err(|error| operation_error(&error))?
            || !crate::assignment_rules::analysis::support::shift_scope(
                coverage.scope,
                shift,
                &metadata,
                budget,
            )
            .map_err(|error| operation_error(&error))?
        {
            continue;
        }
        hash.record(
            &("preferred-coverage-shift", shift.id, preferred_count),
            budget,
        )?;
        for person_id in &input.people {
            budget.step().map_err(|error| operation_error(&error))?;
            let person = input
                .person(*person_id)
                .ok_or_else(|| contract("comparison.person"))?;
            if !crate::assignment_rules::analysis::support::person_scope(
                coverage.scope,
                person,
                budget,
            )
            .map_err(|error| operation_error(&error))?
            {
                continue;
            }
            let pair = AssignmentPair {
                person_id: *person_id,
                shift_id: shift.id,
            };
            let feasible = preference::statically_feasible(input, pair, settings, budget)
                .map_err(|error| operation_error(&error))?;
            hash.record(&("preferred-coverage-person", pair, feasible), budget)?;
        }
    }
    Ok(())
}

fn preferred_coverage_count(
    input: &AssignmentInput,
    requirement_id: EntityId,
) -> Result<Option<u64>, DomainPackError> {
    let Some(WorkforceEntity::CoverageRequirement(requirement)) =
        input.domain.entities.get(&requirement_id)
    else {
        return Err(contract("comparison.coverage-requirement"));
    };
    if !requirement.active {
        return Ok(None);
    }
    let preferred_count = match &requirement.coverage {
        Coverage::AtLeast {
            preferred_count: Some(value),
            ..
        } => u64::from(*value),
        Coverage::AtLeast {
            preferred_count: None,
            ..
        }
        | Coverage::Exact { .. } => return Err(contract("comparison.coverage-shape")),
    };
    Ok((preferred_count > 0).then_some(preferred_count))
}

fn workload_target_kind_name(kind: WorkloadTargetKind) -> &'static str {
    match kind {
        WorkloadTargetKind::Explicit => "explicit",
        WorkloadTargetKind::PersonTargets => "person-targets",
        WorkloadTargetKind::WeightedEqualShare => "weighted-equal-share",
        WorkloadTargetKind::EqualShare => "equal-share",
    }
}

fn hash_workloads(
    hash: &mut SemanticHasher,
    input: &AssignmentInput,
    plan: &PreferencePlan<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    for workload in &plan.workloads {
        let Some(WorkforceEntity::WorkloadBucket(bucket)) = input
            .domain
            .entities
            .get(&workload.policy.bucket_id.as_entity_id())
        else {
            return Err(contract("comparison.workload-bucket"));
        };
        hash.record(
            &(
                "workload-objective",
                workload.level,
                workload.coefficient,
                workload.nanoseconds_per_unit,
                workload.units_per_target_unit,
                workload.score_denominator,
                workload.policy.membership,
                bucket.measurement,
                bucket.overlapping_contribution,
                &workload.policy.penalty,
                workload_target_kind_name(workload.target.kind),
                workload.target.denominator,
                workload.target.targets.len(),
                workload.target.weights.len(),
                workload.target.peers.len(),
                workload.periods.len(),
            ),
            budget,
        )?;
        for (person, target) in &workload.target.targets {
            budget.step().map_err(|error| operation_error(&error))?;
            hash.record(&("workload-target", person, target), budget)?;
        }
        for (person, weight) in &workload.target.weights {
            budget.step().map_err(|error| operation_error(&error))?;
            hash.record(&("workload-weight", person, weight), budget)?;
        }
        for person in &workload.target.peers {
            budget.step().map_err(|error| operation_error(&error))?;
            hash.record(&("workload-peer", person), budget)?;
        }
        for period in &workload.periods {
            budget.step().map_err(|error| operation_error(&error))?;
            hash.record(
                &("workload-period", &period.period, period.peers.len()),
                budget,
            )?;
            for peer in &period.peers {
                budget.step().map_err(|error| operation_error(&error))?;
                hash.record(
                    &("workload-peer-period", peer.person, peer.pairs.len()),
                    budget,
                )?;
                for pair in &peer.pairs {
                    budget.step().map_err(|error| operation_error(&error))?;
                    let shift = input
                        .shift(pair.shift_id)
                        .ok_or_else(|| contract("comparison.workload-shift"))?;
                    if bucket.overlapping_contribution
                        == crate::model::OverlappingContribution::Union
                    {
                        let interval = preference::workload_interval(
                            shift,
                            period.period,
                            workload.policy.membership,
                        );
                        hash.record(&("workload-union-interval", pair, interval), budget)?;
                    } else {
                        let amount = preference::workload_amount(
                            shift,
                            period.period,
                            workload.policy.membership,
                            bucket,
                        )
                        .map_err(|error| operation_error(&error))?;
                        hash.record(&("workload-source-amount", pair, amount), budget)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn hash_consecutives(
    hash: &mut SemanticHasher,
    input: &AssignmentInput,
    plan: &PreferencePlan<'_>,
    settings: &eutheto_types::ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    for consecutive in &plan.consecutives {
        hash.record(
            &(
                "consecutive-objective",
                consecutive.level,
                consecutive.coefficient,
                consecutive.threshold,
                consecutive.weekend,
                consecutive.windows.len(),
            ),
            budget,
        )?;
        for window in &consecutive.windows {
            budget.step().map_err(|error| operation_error(&error))?;
            hash.record(
                &("consecutive-window", window.person, window.days.len()),
                budget,
            )?;
            for day in &window.days {
                budget.step().map_err(|error| operation_error(&error))?;
                hash.record(&("consecutive-day", day.date, day.pairs.len()), budget)?;
                for pair in &day.pairs {
                    budget.step().map_err(|error| operation_error(&error))?;
                    let feasible = preference::statically_feasible(input, *pair, settings, budget)
                        .map_err(|error| operation_error(&error))?;
                    hash.record(&("consecutive-pair", pair, feasible), budget)?;
                }
            }
        }
    }
    Ok(())
}

struct SemanticHasher(blake3::Hasher);

impl SemanticHasher {
    fn new() -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SCORING_CONTEXT_DOMAIN);
        Self(hasher)
    }

    fn record<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), DomainPackError> {
        budget.step().map_err(|error| operation_error(&error))?;
        budget
            .reserve(0, 0, 2)
            .map_err(|error| operation_error(&error))?;
        self.0.update(b"J");
        let mut writer = FingerprintWriter {
            hasher: &mut self.0,
            budget,
            failure: None,
        };
        let serialized = serde_json::to_writer(&mut writer, value);
        if let Some(error) = writer.failure.take() {
            return Err(operation_error(&error));
        }
        serialized.map_err(|_| contract("comparison.fingerprint-serialization"))?;
        writer.hasher.update(b"\0");
        Ok(())
    }

    fn finish(self, budget: &mut OperationBudget<'_>) -> Result<String, DomainPackError> {
        budget
            .reserve(0, 1, 64)
            .map_err(|error| operation_error(&error))?;
        budget.check().map_err(|error| operation_error(&error))?;
        Ok(self.0.finalize().to_hex().to_string())
    }
}

struct FingerprintWriter<'a, 'b> {
    hasher: &'a mut blake3::Hasher,
    budget: &'a mut OperationBudget<'b>,
    failure: Option<AssignmentRuleError>,
}

impl Write for FingerprintWriter<'_, '_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match count(buffer.len()).and_then(|amount| self.budget.steps(amount)) {
            Ok(()) => {
                self.hasher.update(buffer);
                Ok(buffer.len())
            }
            Err(error) => {
                self.failure = Some(error);
                Err(io::Error::other(
                    "bounded Workforce fingerprint interrupted",
                ))
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn contract(code: &'static str) -> DomainPackError {
    DomainPackError::Contract(format!("official.workforce.{code}"))
}

#[cfg(test)]
#[path = "comparison_tests.rs"]
mod tests;
