use super::{
    effect::{self, Changes, Collection, Effect, Operation},
    payload::{
        ADD_ENTITY, ADD_LOCK, ADD_OCCURRENCE_IDENTITIES, ADD_PREFERENCE, ADD_RULE, DETACH_SHIFT,
        EntityPayload, EntityTarget, LockPayload, LockTarget, PreferencePayload, PreferenceTarget,
        REATTACH_SHIFT, REMOVE_ENTITY, REMOVE_LOCK, REMOVE_OCCURRENCE_IDENTITIES,
        REMOVE_PREFERENCE, REMOVE_RULE, RulePayload, RuleTarget, UPDATE_ENTITY, UPDATE_LOCK,
        UPDATE_PREFERENCE, UPDATE_RULE,
    },
};
use crate::validation::common::{Result, invalid};
use eutheto_types::{DomainCommandEnvelope, OperationControl, ScenarioDocument};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub(super) fn apply_one(
    document: &mut ScenarioDocument,
    envelope: &DomainCommandEnvelope,
    changes: &mut Changes,
    control: Option<&OperationControl>,
) -> Result<Effect> {
    super::check_control(control)?;
    let domain = &mut document.domain;
    match envelope.command_type.as_str() {
        ADD_OCCURRENCE_IDENTITIES
        | REMOVE_OCCURRENCE_IDENTITIES
        | DETACH_SHIFT
        | REATTACH_SHIFT => super::occurrences::apply(document, envelope, changes, control),
        ADD_ENTITY | UPDATE_ENTITY => {
            let payload: EntityPayload = decode(&envelope.payload)?;
            effect::mutate(
                &mut domain.entities,
                payload.entity.id(),
                record_operation(envelope, &effect::ENTITIES)?,
                &effect::ENTITIES,
                changes,
            )
        }
        REMOVE_ENTITY => {
            let payload: EntityTarget = decode(&envelope.payload)?;
            effect::mutate(
                &mut domain.entities,
                payload.entity_id,
                Operation::Remove,
                &effect::ENTITIES,
                changes,
            )
        }
        ADD_RULE | UPDATE_RULE => {
            let payload: RulePayload = decode(&envelope.payload)?;
            effect::mutate(
                &mut domain.rules,
                payload.rule.header().0,
                record_operation(envelope, &effect::RULES)?,
                &effect::RULES,
                changes,
            )
        }
        REMOVE_RULE => {
            let payload: RuleTarget = decode(&envelope.payload)?;
            effect::mutate(
                &mut domain.rules,
                payload.rule_id,
                Operation::Remove,
                &effect::RULES,
                changes,
            )
        }
        ADD_PREFERENCE | UPDATE_PREFERENCE => {
            let payload: PreferencePayload = decode(&envelope.payload)?;
            effect::mutate(
                &mut domain.preferences,
                payload.preference.header().0,
                record_operation(envelope, &effect::PREFERENCES)?,
                &effect::PREFERENCES,
                changes,
            )
        }
        REMOVE_PREFERENCE => {
            let payload: PreferenceTarget = decode(&envelope.payload)?;
            effect::mutate(
                &mut domain.preferences,
                payload.preference_id,
                Operation::Remove,
                &effect::PREFERENCES,
                changes,
            )
        }
        ADD_LOCK | UPDATE_LOCK => {
            let payload: LockPayload = decode(&envelope.payload)?;
            effect::mutate(
                &mut domain.locked_assignments,
                payload.lock.id,
                record_operation(envelope, &effect::LOCKS)?,
                &effect::LOCKS,
                changes,
            )
        }
        REMOVE_LOCK => {
            let payload: LockTarget = decode(&envelope.payload)?;
            effect::mutate(
                &mut domain.locked_assignments,
                payload.assignment_id,
                Operation::Remove,
                &effect::LOCKS,
                changes,
            )
        }
        _ => Err(eutheto_domain_api::DomainPackError::UnknownCommand(
            envelope.command_type.clone(),
        )),
    }
}

fn decode<T: DeserializeOwned>(value: &Value) -> Result<T> {
    T::deserialize(value).map_err(|_| invalid("/payload", "invalid Workforce command payload"))
}

fn record_operation<'a>(
    envelope: &'a DomainCommandEnvelope,
    collection: &Collection,
) -> Result<Operation<'a>> {
    let value = envelope
        .payload
        .get(collection.record)
        .ok_or_else(|| invalid("/payload", "missing typed record"))?;
    Ok(if envelope.command_type == collection.add {
        Operation::Add(value)
    } else {
        Operation::Update(value)
    })
}
