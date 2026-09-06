use crate::validation::common::{Result, invalid, require};
use eutheto_domain_api::{DomainChange, bounded_json_size};
use eutheto_types::DomainCommandEnvelope;
use serde::Serialize;
use serde_json::{Map, Value};
use std::{collections::BTreeMap, fmt::Display};

use super::payload::{
    ADD_ENTITY, ADD_LOCK, ADD_PREFERENCE, ADD_RULE, REMOVE_ENTITY, REMOVE_LOCK, REMOVE_PREFERENCE,
    REMOVE_RULE, UPDATE_ENTITY, UPDATE_LOCK, UPDATE_PREFERENCE, UPDATE_RULE,
};

pub(super) struct Collection {
    pub map: &'static str,
    pub record: &'static str,
    pub target: &'static str,
    pub add: &'static str,
    pub update: &'static str,
    pub remove: &'static str,
}

pub(super) const ENTITIES: Collection = Collection {
    map: "entities",
    record: "entity",
    target: "entityId",
    add: ADD_ENTITY,
    update: UPDATE_ENTITY,
    remove: REMOVE_ENTITY,
};
pub(super) const RULES: Collection = Collection {
    map: "rules",
    record: "rule",
    target: "ruleId",
    add: ADD_RULE,
    update: UPDATE_RULE,
    remove: REMOVE_RULE,
};
pub(super) const PREFERENCES: Collection = Collection {
    map: "preferences",
    record: "preference",
    target: "preferenceId",
    add: ADD_PREFERENCE,
    update: UPDATE_PREFERENCE,
    remove: REMOVE_PREFERENCE,
};
pub(super) const LOCKS: Collection = Collection {
    map: "lockedAssignments",
    record: "lock",
    target: "assignmentId",
    add: ADD_LOCK,
    update: UPDATE_LOCK,
    remove: REMOVE_LOCK,
};

#[derive(Clone, Copy)]
pub(super) enum Operation<'a> {
    Add(&'a Value),
    Update(&'a Value),
    Remove,
}

pub(super) struct Effect {
    pub result: Value,
    pub inverse: DomainCommandEnvelope,
}

const MAX_CHANGES: usize = 85_536;
const MAX_CHANGE_BYTES: usize = 64 * 1024 * 1024;

/// One batch-owned buffer. Compound commands cannot amplify small payloads without bounds.
pub(super) struct Changes {
    records: Vec<DomainChange>,
    bytes: usize,
}

impl Changes {
    pub fn new(capacity: usize) -> Self {
        Self { records: Vec::with_capacity(capacity), bytes: 2 }
    }

    pub fn push(&mut self, change: DomainChange) -> Result {
        require(self.records.len() < MAX_CHANGES, "/changes", "too many command changes")?;
        let separator = usize::from(!self.records.is_empty());
        let remaining = MAX_CHANGE_BYTES.checked_sub(self.bytes + separator)
            .ok_or_else(|| invalid("/changes", "command changes exceed byte limit"))?;
        let bytes = bounded_json_size(&change, remaining)?;
        self.bytes += separator + bytes;
        self.records.push(change);
        Ok(())
    }

    pub fn into_records(self) -> Vec<DomainChange> {
        self.records
    }
}

/// Edits only the addressed raw record. The caller validates typed payload and resulting state.
/// Original JSON spelling is retained in the ordinary inverse, never an undo-only bypass.
pub(super) fn mutate<K: Copy + Ord + Display + Serialize>(
    records: &mut BTreeMap<K, Value>,
    id: K,
    operation: Operation<'_>,
    collection: &Collection,
    changes: &mut Changes,
) -> Result<Effect> {
    let path = format!("/domain/{}/{id}", collection.map);
    let previous = records.get(&id);
    let (command_id, inverse_type, inverse_payload) = match operation {
        Operation::Add(_) => {
            require(previous.is_none(), &path, "record already exists")?;
            (collection.add, collection.remove, target(collection, id)?)
        }
        Operation::Update(value) => {
            let previous = previous.ok_or_else(|| invalid(&path, "record does not exist"))?;
            require(
                previous.get("kind") == value.get("kind"),
                &path,
                "record kind cannot change",
            )?;
            (
                collection.update,
                collection.update,
                field(collection.record, previous.clone()),
            )
        }
        Operation::Remove => {
            let previous = previous.ok_or_else(|| invalid(&path, "record does not exist"))?;
            (
                collection.remove,
                collection.add,
                field(collection.record, previous.clone()),
            )
        }
    };
    let (before, after) = match operation {
        Operation::Add(value) | Operation::Update(value) => {
            (records.insert(id, value.clone()), Some(value.clone()))
        }
        Operation::Remove => (records.remove(&id), None),
    };
    let change = Value::Object(Map::from_iter([
        ("path".to_owned(), Value::String(path)),
        ("before".to_owned(), before.unwrap_or(Value::Null)),
        ("after".to_owned(), after.unwrap_or(Value::Null)),
    ]));
    changes.push(DomainChange {
        command_id: command_id.to_owned(),
        value: change,
    })?;
    Ok(Effect {
        result: target(collection, id)?,
        inverse: DomainCommandEnvelope {
            command_type: inverse_type.to_owned(),
            payload: inverse_payload,
        },
    })
}

fn target(collection: &Collection, id: impl Serialize) -> Result<Value> {
    let value = serde_json::to_value(id)
        .map_err(|_| invalid("command", "identity serialization failed"))?;
    Ok(field(collection.target, value))
}

fn field(name: &str, value: Value) -> Value {
    Value::Object(Map::from_iter([(name.to_owned(), value)]))
}
