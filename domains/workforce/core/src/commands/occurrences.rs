use super::effect::{Changes, Effect};
use super::payload::{
    ADD_OCCURRENCE_IDENTITIES, AddOccurrenceIdentitiesPayload, DETACH_SHIFT, DetachShiftPayload,
    REATTACH_SHIFT, REMOVE_OCCURRENCE_IDENTITIES, RemoveOccurrenceIdentitiesPayload,
    TemplateOccurrenceIdentities,
};
use crate::ids::{ShiftId, ShiftTemplateId};
use crate::model::{OccurrenceIdentity, ShiftOrigin, WorkforceEntity};
use crate::validation::common::{Result, invalid, require};
use crate::validation::{MAX_DOCUMENT_OCCURRENCES, MAX_REFERENCE_ITEMS, MAX_TEMPLATE_OCCURRENCES};
use eutheto_domain_api::{DomainChange, DomainPackError, bounded_json_size};
use eutheto_types::{
    CancellationToken, DomainCommandEnvelope, MAX_SCENARIO_DOCUMENT_BYTES, ScenarioDocument,
};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn apply(
    document: &mut ScenarioDocument,
    envelope: &DomainCommandEnvelope,
    changes: &mut Changes,
    cancellation: Option<&CancellationToken>,
) -> Result<Effect> {
    super::check_cancellation(cancellation)?;
    match envelope.command_type.as_str() {
        ADD_OCCURRENCE_IDENTITIES => add(document, envelope, changes, cancellation),
        REMOVE_OCCURRENCE_IDENTITIES => remove(document, envelope, changes, cancellation),
        DETACH_SHIFT => detach(document, envelope, changes, cancellation),
        REATTACH_SHIFT => reattach(document, envelope, changes, cancellation),
        _ => Err(DomainPackError::UnknownCommand(
            envelope.command_type.clone(),
        )),
    }
}

fn decode<T: DeserializeOwned>(value: &Value) -> Result<T> {
    T::deserialize(value).map_err(|_| invalid("/payload", "invalid Workforce occurrence payload"))
}

fn ledger(
    document: &mut ScenarioDocument,
    template_id: ShiftTemplateId,
) -> Result<&mut Map<String, Value>> {
    let template = document
        .domain
        .entities
        .get_mut(&template_id.as_entity_id())
        .ok_or_else(|| invalid("/payload/templateId", "template does not exist"))?;
    require(
        template["kind"] == "shiftTemplate",
        "/payload/templateId",
        "target is not a shift template",
    )?;
    require(
        decode::<ShiftTemplateId>(&template["id"])? == template_id,
        "/payload/templateId",
        "template identity does not match its key",
    )?;
    template
        .get_mut("occurrenceIdentities")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid("occurrenceIdentities", "missing occurrence ledger"))
}

// Parse an addressed ledger once, preserving raw keys independently from entry UUID spelling.
fn index(
    ledger: &Map<String, Value>,
    cancellation: Option<&CancellationToken>,
) -> Result<BTreeMap<ShiftId, String>> {
    let mut keys = BTreeMap::new();
    for key in ledger.keys() {
        super::check_cancellation(cancellation)?;
        let id = key
            .parse::<ShiftId>()
            .map_err(|_| invalid("occurrenceIdentities", "invalid shift identity"))?;
        require(
            keys.insert(id, key.clone()).is_none(),
            "occurrenceIdentities",
            "duplicate shift identity",
        )?;
    }
    Ok(keys)
}

fn check_count(count: usize, limit: usize) -> Result {
    require(
        count > 0 && count <= limit,
        "/payload/templates",
        "occurrence target count is outside its bounds",
    )
}

fn check_entries(
    entries: &TemplateOccurrenceIdentities,
    cancellation: Option<&CancellationToken>,
) -> Result {
    check_count(
        entries.occurrence_identities.len(),
        MAX_TEMPLATE_OCCURRENCES,
    )?;
    for (id, occurrence) in &entries.occurrence_identities {
        super::check_cancellation(cancellation)?;
        require(
            *id == occurrence.id,
            "/payload/occurrenceIdentities",
            "occurrence identity does not match its key",
        )?;
    }
    Ok(())
}

fn change(
    changes: &mut Changes,
    command: &str,
    path: String,
    before: Value,
    after: Value,
) -> Result {
    changes.push(DomainChange {
        command_id: command.to_owned(),
        value: Value::Object(Map::from_iter([
            ("path".to_owned(), Value::String(path)),
            ("before".to_owned(), before),
            ("after".to_owned(), after),
        ])),
    })
}

fn entry_path(template_id: ShiftTemplateId, key: &str) -> String {
    // Keys have already been checked as UUIDs, so no JSON pointer escaping is needed.
    format!("/domain/entities/{template_id}/occurrenceIdentities/{key}")
}

fn finish(result: Value, command: &str, payload: Value) -> Result<Effect> {
    let inverse = DomainCommandEnvelope {
        command_type: command.to_owned(),
        payload,
    };
    bounded_json_size(&inverse, inverse_limit()?)?;
    Ok(Effect { result, inverse })
}

fn inverse_limit() -> Result<usize> {
    usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES)
        .map_err(|_| invalid("/inverse", "unsupported command byte limit"))
}

fn add(
    document: &mut ScenarioDocument,
    envelope: &DomainCommandEnvelope,
    changes: &mut Changes,
    cancellation: Option<&CancellationToken>,
) -> Result<Effect> {
    let payload: AddOccurrenceIdentitiesPayload = decode(&envelope.payload)?;
    check_count(payload.templates.len(), MAX_REFERENCE_ITEMS)?;
    let raw_templates = envelope.payload["templates"]
        .as_array()
        .ok_or_else(|| invalid("/payload/templates", "missing template targets"))?;
    let mut targets = BTreeSet::new();
    let mut total = 0;
    let mut inverse_templates = Vec::with_capacity(payload.templates.len());
    for (target, raw) in payload.templates.iter().zip(raw_templates) {
        super::check_cancellation(cancellation)?;
        require(
            targets.insert(target.template_id),
            "/payload/templates",
            "duplicate template target",
        )?;
        check_entries(target, cancellation)?;
        total += target.occurrence_identities.len();
        check_count(total, MAX_DOCUMENT_OCCURRENCES)?;
        let ledger = ledger(document, target.template_id)?;
        require(
            ledger.len() + target.occurrence_identities.len() <= MAX_TEMPLATE_OCCURRENCES,
            "occurrenceIdentities",
            "too many template occurrences",
        )?;
        let existing = index(ledger, cancellation)?;
        let raw_entries = raw["occurrenceIdentities"].as_object().ok_or_else(|| {
            invalid(
                "/payload/occurrenceIdentities",
                "missing occurrence entries",
            )
        })?;
        for (key, value) in raw_entries {
            super::check_cancellation(cancellation)?;
            let id: ShiftId = decode(&value["id"])?;
            require(
                !existing.contains_key(&id),
                "occurrenceIdentities",
                "occurrence already exists",
            )?;
            change(
                changes,
                ADD_OCCURRENCE_IDENTITIES,
                entry_path(target.template_id, key),
                Value::Null,
                value.clone(),
            )?;
            ledger.insert(key.clone(), value.clone());
        }
        inverse_templates.push(json!({"templateId":target.template_id,"shiftIds":target.occurrence_identities.keys().collect::<Vec<_>>()}));
    }
    finish(
        json!({"occurrenceCount":total,"templateCount":targets.len()}),
        REMOVE_OCCURRENCE_IDENTITIES,
        json!({"templates":inverse_templates}),
    )
}

fn remove(
    document: &mut ScenarioDocument,
    envelope: &DomainCommandEnvelope,
    changes: &mut Changes,
    cancellation: Option<&CancellationToken>,
) -> Result<Effect> {
    let payload: RemoveOccurrenceIdentitiesPayload = decode(&envelope.payload)?;
    check_count(payload.templates.len(), MAX_REFERENCE_ITEMS)?;
    let mut targets = BTreeSet::new();
    let mut total = 0;
    let mut inverse_templates = Vec::with_capacity(payload.templates.len());
    let mut remaining = inverse_limit()?;
    for target in &payload.templates {
        super::check_cancellation(cancellation)?;
        require(
            targets.insert(target.template_id),
            "/payload/templates",
            "duplicate template target",
        )?;
        check_count(target.shift_ids.len(), MAX_TEMPLATE_OCCURRENCES)?;
        total += target.shift_ids.len();
        check_count(total, MAX_DOCUMENT_OCCURRENCES)?;
        let ledger = ledger(document, target.template_id)?;
        let mut keys = index(ledger, cancellation)?;
        let mut removed = Map::new();
        for id in &target.shift_ids {
            super::check_cancellation(cancellation)?;
            // Taking the indexed key also rejects duplicate typed targets without rescanning.
            let key = keys.remove(id).ok_or_else(|| {
                invalid(
                    "/payload/shiftIds",
                    "missing or duplicate occurrence target",
                )
            })?;
            let value = ledger
                .remove(&key)
                .ok_or_else(|| invalid("occurrenceIdentities", "occurrence does not exist"))?;
            change(
                changes,
                REMOVE_OCCURRENCE_IDENTITIES,
                entry_path(target.template_id, &key),
                value.clone(),
                Value::Null,
            )?;
            removed.insert(key, value);
        }
        let inverse = json!({"templateId":target.template_id,"occurrenceIdentities":removed});
        // Charge each retained fragment once; finish and the batch caller check full framing.
        remaining -= bounded_json_size(&inverse, remaining)?;
        inverse_templates.push(inverse);
    }
    finish(
        json!({"occurrenceCount":total,"templateCount":targets.len()}),
        ADD_OCCURRENCE_IDENTITIES,
        json!({"templates":inverse_templates}),
    )
}

fn detach(
    document: &mut ScenarioDocument,
    envelope: &DomainCommandEnvelope,
    changes: &mut Changes,
    cancellation: Option<&CancellationToken>,
) -> Result<Effect> {
    let payload: DetachShiftPayload = decode(&envelope.payload)?;
    let WorkforceEntity::ShiftInstance(instance) = payload.instance else {
        return Err(invalid(
            "/payload/instance",
            "detach requires a shift instance",
        ));
    };
    let ShiftOrigin::Detached {
        template_id,
        occurrence_date,
    } = instance.origin
    else {
        return Err(invalid(
            "/payload/instance/origin",
            "detach requires a detached origin",
        ));
    };
    require(
        template_id == payload.template_id,
        "/payload/templateId",
        "detached template does not match target",
    )?;
    require(
        !document
            .domain
            .entities
            .contains_key(&instance.id.as_entity_id()),
        "/payload/instance/id",
        "stored identity already exists",
    )?;
    let ledger = ledger(document, template_id)?;
    let key = index(ledger, cancellation)?
        .remove(&instance.id)
        .ok_or_else(|| invalid("/payload/instance/id", "occurrence does not exist"))?;
    let original = ledger
        .get(&key)
        .ok_or_else(|| invalid("occurrenceIdentities", "occurrence does not exist"))?;
    let occurrence: OccurrenceIdentity = decode(original)?;
    require(
        occurrence.id == instance.id && occurrence.local_start_date == occurrence_date,
        "/payload/instance/origin",
        "detached origin does not match occurrence",
    )?;
    let original = ledger
        .remove(&key)
        .ok_or_else(|| invalid("occurrenceIdentities", "occurrence does not exist"))?;
    change(
        changes,
        DETACH_SHIFT,
        entry_path(template_id, &key),
        original.clone(),
        Value::Null,
    )?;
    let raw_instance = envelope
        .payload
        .get("instance")
        .ok_or_else(|| invalid("/payload/instance", "missing instance"))?;
    change(
        changes,
        DETACH_SHIFT,
        format!("/domain/entities/{}", instance.id),
        Value::Null,
        raw_instance.clone(),
    )?;
    document
        .domain
        .entities
        .insert(instance.id.as_entity_id(), raw_instance.clone());
    finish(
        json!({"templateId":template_id,"shiftId":instance.id,"occurrenceDate":occurrence_date,"owner":"stored"}),
        REATTACH_SHIFT,
        json!({"templateId":template_id,"occurrenceIdentities":{(key):original}}),
    )
}

fn reattach(
    document: &mut ScenarioDocument,
    envelope: &DomainCommandEnvelope,
    changes: &mut Changes,
    cancellation: Option<&CancellationToken>,
) -> Result<Effect> {
    let payload: TemplateOccurrenceIdentities = decode(&envelope.payload)?;
    check_entries(&payload, cancellation)?;
    require(
        payload.occurrence_identities.len() == 1,
        "/payload/occurrenceIdentities",
        "reattach requires exactly one occurrence",
    )?;
    let occurrence = payload
        .occurrence_identities
        .values()
        .next()
        .ok_or_else(|| invalid("/payload/occurrenceIdentities", "missing occurrence"))?;
    let original = document
        .domain
        .entities
        .get(&occurrence.id.as_entity_id())
        .ok_or_else(|| {
            invalid(
                "/payload/occurrenceIdentities",
                "stored shift does not exist",
            )
        })?;
    let WorkforceEntity::ShiftInstance(instance) = decode::<WorkforceEntity>(original)? else {
        return Err(invalid(
            "/payload/occurrenceIdentities",
            "stored target is not a shift instance",
        ));
    };
    require(
        instance.id == occurrence.id
            && instance.origin
                == ShiftOrigin::Detached {
                    template_id: payload.template_id,
                    occurrence_date: occurrence.local_start_date,
                },
        "/payload/occurrenceIdentities",
        "stored target has a different origin",
    )?;
    // Check replayable size before retaining the raw stored instance in its ordinary inverse.
    bounded_json_size(original, inverse_limit()?)?;
    let ledger = ledger(document, payload.template_id)?;
    require(
        ledger.len() < MAX_TEMPLATE_OCCURRENCES,
        "occurrenceIdentities",
        "too many template occurrences",
    )?;
    require(
        !index(ledger, cancellation)?.contains_key(&occurrence.id),
        "occurrenceIdentities",
        "occurrence already exists",
    )?;
    let (key, value) = envelope.payload["occurrenceIdentities"]
        .as_object()
        .and_then(|entries| entries.iter().next())
        .ok_or_else(|| invalid("/payload/occurrenceIdentities", "missing occurrence"))?;
    change(
        changes,
        REATTACH_SHIFT,
        entry_path(payload.template_id, key),
        Value::Null,
        value.clone(),
    )?;
    ledger.insert(key.clone(), value.clone());
    let original = document
        .domain
        .entities
        .remove(&occurrence.id.as_entity_id())
        .ok_or_else(|| {
            invalid(
                "/payload/occurrenceIdentities",
                "stored shift does not exist",
            )
        })?;
    change(
        changes,
        REATTACH_SHIFT,
        format!("/domain/entities/{}", occurrence.id),
        original.clone(),
        Value::Null,
    )?;
    finish(
        json!({"templateId":payload.template_id,"shiftId":occurrence.id,"occurrenceDate":occurrence.local_start_date,"owner":"template"}),
        DETACH_SHIFT,
        json!({"templateId":payload.template_id,"instance":original}),
    )
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn cancellation_rejects_each_atomic_command_without_changes()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let original = test_support::fixture()?;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        for command in [
            ADD_OCCURRENCE_IDENTITIES,
            REMOVE_OCCURRENCE_IDENTITIES,
            DETACH_SHIFT,
            REATTACH_SHIFT,
        ] {
            let mut working = original.clone();
            let mut changes = Changes::new(0);
            let envelope = DomainCommandEnvelope {
                command_type: command.to_owned(),
                payload: json!({}),
            };
            assert!(matches!(
                apply(&mut working, &envelope, &mut changes, Some(&cancellation)),
                Err(DomainPackError::Cancelled)
            ));
            assert_eq!(working, original);
            assert!(changes.into_records().is_empty());
        }
        Ok(())
    }
}
