//! Native-authenticated source rule relaxation and exact question authoring.
//! Persistence, proof custody, source assets and the copy transaction remain host-owned.

use super::{ADD_RULE, UPDATE_RULE};
use crate::{
    assignment_rules::{
        budget::OperationBudget, decode_workforce_assignment_id, input::AssignmentInput,
        operation_error, validate_input_bounds, workforce_diagnostic_verification_scope,
    },
    ids::ShiftId,
    model::{AssignmentPair, PersonSelection, RequiredStrength, Scope, WorkforceRule},
};
use eutheto_domain_api::{
    DOMAIN_BATCH_SCHEMA_VERSION, DiagnosticCopyContext, DomainBatchCommand, DomainPackError,
};
use eutheto_domain_ir::{AssignmentValue, CounterfactualConditionPayloadV1, DomainAssignmentId};
use eutheto_types::{DomainCommandEnvelope, EntityId, PersonId, RuleId, ScenarioDocument};
use serde_json::{Value, json};
use uuid::Uuid;

// Preserve source authentication, raw-rule intent, and exact condition pins in one native batch.
#[allow(clippy::too_many_lines)]
pub(crate) fn prepare_diagnostic_copy_changes(
    source: &ScenarioDocument,
    context: &DiagnosticCopyContext<'_>,
) -> Result<DomainBatchCommand, DomainPackError> {
    context.control.check()?;
    context
        .group
        .validate()
        .map_err(|_| invalid("/group", "Invalid source conflict group."))?;
    if context.group.required_rules.len() != 1
        || source.domain_pack != context.destination.domain_pack
        || context.id_remap.get(&source.scenario_id.as_uuid())
            != Some(&context.destination.scenario_id.as_uuid())
        || source.scenario_id == context.destination.scenario_id
    {
        return Err(invalid(
            "/source",
            "The diagnostic copy does not match its immutable source.",
        ));
    }
    let classified =
        workforce_diagnostic_verification_scope(source, context.source_revision, context.control)?;
    let selected = context.group.required_rules[0];
    if classified
        .relaxable_required_rules()
        .binary_search(&selected)
        .is_err()
    {
        return Err(invalid(
            "/group",
            "Only an active authored Required rule can be relaxed.",
        ));
    }
    validate_input_bounds(context.destination, Some(context.control))?;
    let copied_rule = RuleId::from_uuid(mapped(selected.as_uuid(), context)?);
    let raw = context
        .destination
        .domain
        .rules
        .get(&copied_rule)
        .ok_or_else(|| invalid("/group", "The copied source rule is missing."))?;
    if raw.get("active").and_then(Value::as_bool) != Some(true)
        || raw
            .get("id")
            .and_then(Value::as_str)
            .and_then(|text| text.parse::<RuleId>().ok())
            != Some(copied_rule)
    {
        return Err(invalid(
            "/group",
            "The copied rule no longer matches its active source.",
        ));
    }
    // Deactivation preserves every unrelated raw field, including absent/default fields.
    let mut deactivated = raw.clone();
    deactivated
        .as_object_mut()
        .ok_or_else(|| invalid("/group", "Invalid copied rule."))?
        .insert("active".to_owned(), Value::Bool(false));
    let mut commands = Vec::with_capacity(if context.condition.is_some() { 3 } else { 1 });
    commands.push(DomainCommandEnvelope {
        command_type: UPDATE_RULE.to_owned(),
        payload: json!({"rule": deactivated}),
    });
    if let Some(condition) = context.condition {
        condition
            .validate()
            .map_err(|_| invalid("/condition", "Invalid immutable diagnostic question."))?;
        let mut budget = OperationBudget::evaluation(Some(context.control));
        let original =
            AssignmentInput::new(source, &mut budget).map_err(|error| operation_error(&error))?;
        let destination = AssignmentInput::new(context.destination, &mut budget)
            .map_err(|error| operation_error(&error))?;
        match &condition.condition {
            CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id,
                value,
            }
            | CounterfactualConditionPayloadV1::ForbidAssignmentValue {
                assignment_id,
                value,
            } => {
                let AssignmentValue::Boolean(value) = value else {
                    return Err(invalid(
                        "/condition",
                        "The Workforce question must name a Boolean assignment.",
                    ));
                };
                let assigned = if matches!(
                    condition.condition,
                    CounterfactualConditionPayloadV1::ForceAssignmentValue { .. }
                ) {
                    *value
                } else {
                    !*value
                };
                let pair = remapped_pair(assignment_id, &original, &destination, context)?;
                commands.push(pin(pair, assigned, context)?);
            }
            CounterfactualConditionPayloadV1::ReplaceAssignment {
                target_assignment_id,
                replaced_assignment_id,
            } => {
                let target = decode_workforce_assignment_id(target_assignment_id)?;
                let replaced = decode_workforce_assignment_id(replaced_assignment_id)?;
                if target.shift_id != replaced.shift_id || target.person_id == replaced.person_id {
                    return Err(invalid(
                        "/condition",
                        "Replacement must name distinct people on the same exact shift.",
                    ));
                }
                commands.push(pin(
                    remapped_pair(target_assignment_id, &original, &destination, context)?,
                    true,
                    context,
                )?);
                commands.push(pin(
                    remapped_pair(replaced_assignment_id, &original, &destination, context)?,
                    false,
                    context,
                )?);
            }
        }
    }
    let batch = DomainBatchCommand {
        schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: source.domain_pack.id.clone(),
        scenario_schema_version: source.domain_pack.schema_version,
        label: Some("Relax one Required rule in a copy".to_owned()),
        commands,
    };
    batch.validate_bounds()?;
    context.control.check()?;
    Ok(batch)
}

fn remapped_pair(
    id: &DomainAssignmentId,
    source: &AssignmentInput,
    destination: &AssignmentInput,
    context: &DiagnosticCopyContext<'_>,
) -> Result<AssignmentPair, DomainPackError> {
    context.control.check()?;
    let pair = decode_workforce_assignment_id(id)?;
    if source.person(pair.person_id).is_none() || source.shift(pair.shift_id).is_none() {
        return Err(invalid(
            "/condition",
            "The question target is missing from its source.",
        ));
    }
    let remapped = AssignmentPair {
        person_id: PersonId::from_uuid(mapped(pair.person_id.as_uuid(), context)?),
        shift_id: ShiftId::try_from(EntityId::from_uuid(mapped(
            pair.shift_id.as_entity_id().as_uuid(),
            context,
        )?))
        .map_err(|_| invalid("/condition", "Invalid copied shift identity."))?,
    };
    if destination.person(remapped.person_id).is_none()
        || destination.shift(remapped.shift_id).is_none()
    {
        return Err(invalid(
            "/condition",
            "The question target is missing from the copied scenario.",
        ));
    }
    Ok(remapped)
}

fn pin(
    pair: AssignmentPair,
    assigned: bool,
    context: &DiagnosticCopyContext<'_>,
) -> Result<DomainCommandEnvelope, DomainPackError> {
    context.control.check()?;
    let id = RuleId::new(context.ids).map_err(|_| {
        invalid(
            "/condition",
            "Could not allocate a native question rule identity.",
        )
    })?;
    let rule = WorkforceRule::FixedAssignment {
        id,
        active: true,
        strength: RequiredStrength::Required,
        scope: Scope {
            people: PersonSelection::Selected {
                person_ids: vec![pair.person_id],
            },
            team_ids: None,
            assignment_type_ids: None,
            categories: None,
            weekdays: None,
            location_ids: None,
        },
        person_id: pair.person_id,
        shift_id: pair.shift_id,
        assigned,
    };
    let payload = serde_json::to_value(super::RulePayload { rule })
        .map_err(|_| invalid("/condition", "Could not encode the exact question rule."))?;
    Ok(DomainCommandEnvelope {
        command_type: ADD_RULE.to_owned(),
        payload,
    })
}

fn mapped(id: Uuid, context: &DiagnosticCopyContext<'_>) -> Result<Uuid, DomainPackError> {
    context
        .id_remap
        .get(&id)
        .copied()
        .filter(|mapped| mapped.get_version_num() == 7 && *mapped != id)
        .ok_or_else(|| {
            invalid(
                "/source",
                "An authenticated copied identity is missing or invalid.",
            )
        })
}

fn invalid(path: &str, message: &str) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}
