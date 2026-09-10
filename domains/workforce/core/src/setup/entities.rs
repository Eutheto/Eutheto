use super::contracts::{
    EntityDetailParametersV1, EntityPageParametersV1, EntitySummaryV1, ORDINARY_DATA_BYTES,
    QUERY_STRING_BYTES, WorkforceEntityKindV1, WorkforcePositionV1, WorkforceSetupViewDataV1,
};
use super::paging::{PageBuilder, ProjectionBudget, Result, invalid};
use crate::model::WorkforceEntity;
use crate::validation::{MAX_DISPLAY_BYTES, WorkforceSchemas};
use eutheto_domain_api::{ContractJsonLimits, SetupViewContext};
use eutheto_types::{EntityId, ScenarioDocument};
use serde::Deserialize;
use serde_json::Value;

pub(super) fn page(
    document: &ScenarioDocument,
    parameters: &EntityPageParametersV1,
    position: Option<WorkforcePositionV1>,
    context: SetupViewContext,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    if parameters.search.len() > QUERY_STRING_BYTES
        || (!has_name(parameters.entity_kind) && !parameters.search.is_empty())
    {
        return Err(invalid(
            "/query/parameters/search",
            "search is invalid for this entity kind",
        ));
    }
    let previous = match position {
        None => None,
        Some(WorkforcePositionV1::Entity { entity_id }) => Some(entity_id),
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "entity position required",
            ));
        }
    };
    if previous.is_some_and(|id| !document.domain.entities.contains_key(&id)) {
        return Err(invalid(
            "/query/continuation/position",
            "continued entity is absent",
        ));
    }
    let search = parameters.search.to_lowercase();
    let mut page = PageBuilder::new(parameters.limit, ORDINARY_DATA_BYTES)?;
    for (&entity_id, record) in &document.domain.entities {
        budget.visit()?;
        let (kind, name) = header(entity_id, record)?;
        let matches = kind == parameters.entity_kind
            && (search.is_empty()
                || name.is_some_and(|name| name.to_lowercase().contains(&search)));
        if previous == Some(entity_id) && !matches {
            return Err(invalid(
                "/query/continuation/position",
                "continued entity does not match the query",
            ));
        }
        if matches {
            page.observe(previous.is_none_or(|previous| entity_id > previous), || {
                Ok((
                    EntitySummaryV1 {
                        entity_id,
                        kind,
                        name: name.map(str::to_owned),
                    },
                    WorkforcePositionV1::Entity { entity_id },
                ))
            })?;
        }
    }
    page.finish(
        document.scenario_id,
        context,
        WorkforceSetupViewDataV1::EntityPage,
    )
}

pub(super) fn detail(
    document: &ScenarioDocument,
    parameters: &EntityDetailParametersV1,
    position: Option<WorkforcePositionV1>,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    if position.is_some() {
        return Err(invalid(
            "/query/continuation",
            "entity detail does not accept continuation",
        ));
    }
    budget.visit()?;
    let record = document
        .domain
        .entities
        .get(&parameters.entity_id)
        .ok_or_else(|| invalid("/query/parameters/entityId", "entity is absent"))?;
    let (kind, _) = header(parameters.entity_id, record)?;
    if kind != parameters.entity_kind {
        return Err(invalid(
            "/query/parameters/entityKind",
            "entity kind does not match",
        ));
    }
    WorkforceSchemas::load()?
        .entities
        .validate(record, ContractJsonLimits::DEFAULT)?;
    let entity = WorkforceEntity::deserialize(record)
        .map_err(|_| invalid("/domain/entities", "entity has an invalid typed record"))?;
    Ok(WorkforceSetupViewDataV1::EntityDetail(Box::new(entity)))
}

/// Inspect only structural identity/name facts; this is deliberately not full validation
/// or a claim that references, schedules, rules, or feasibility are valid.
pub(super) fn header(
    id: EntityId,
    record: &Value,
) -> Result<(WorkforceEntityKindV1, Option<&str>)> {
    let object = record
        .as_object()
        .ok_or_else(|| invalid("/domain/entities", "entity must be an object"))?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("/domain/entities", "entity kind is invalid"))?;
    let kind = WorkforceEntityKindV1::deserialize(serde::de::value::StrDeserializer::<
        serde_json::Error,
    >::new(kind))
    .map_err(|_| invalid("/domain/entities", "entity kind is unknown"))?;
    let actual_id = EntityId::deserialize(
        object
            .get("id")
            .ok_or_else(|| invalid("/domain/entities", "entity identity is absent"))?,
    )
    .map_err(|_| invalid("/domain/entities", "entity identity is invalid"))?;
    if actual_id != id {
        return Err(invalid(
            "/domain/entities",
            "entity identity does not match its map key",
        ));
    }
    let name = if has_name(kind) {
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("/domain/entities", "entity name is invalid"))?;
        if name.len() > MAX_DISPLAY_BYTES {
            return Err(invalid("/domain/entities", "entity name exceeds its bound"));
        }
        Some(name)
    } else {
        None
    };
    Ok((kind, name))
}

const fn has_name(kind: WorkforceEntityKindV1) -> bool {
    matches!(
        kind,
        WorkforceEntityKindV1::Person
            | WorkforceEntityKindV1::Qualification
            | WorkforceEntityKindV1::Team
            | WorkforceEntityKindV1::Location
            | WorkforceEntityKindV1::WorkloadBucket
            | WorkforceEntityKindV1::Calendar
            | WorkforceEntityKindV1::AssignmentType
            | WorkforceEntityKindV1::ShiftTemplate
    )
}
