use crate::{
    assignment_rules::validate_input_bounds,
    ids::ShiftId,
    model::WorkforceEntity,
    validation::{
        MAX_REFERENCE_ITEMS, WorkforceSchemas,
        common::{Result, invalid, require},
        validate_document_with_schemas, validate_resolved_time,
    },
};
use eutheto_domain_api::{DomainPackError, DomainSettingsMutation, bounded_json_size};
use eutheto_types::{
    MAX_SCENARIO_DOCUMENT_BYTES, OperationControl, ResolvedLocalTime, ScenarioDocument,
    ScenarioSettings,
};
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Restoration<'a> {
    schema_version: u32,
    #[serde(borrow)]
    times: Vec<RestoredTimes<'a>>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoredTimes<'a> {
    shift_id: ShiftId,
    #[serde(borrow)]
    starts_at: RawTime<'a>,
    #[serde(borrow)]
    ends_at: RawTime<'a>,
}

// Borrow exact spelling; typed timestamps would canonicalize an inverse on serialization.
#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawTime<'a> {
    instant: &'a str,
    local: &'a str,
    offset_seconds: i32,
}

impl RawTime<'_> {
    fn resolved(&self) -> Result<ResolvedLocalTime> {
        Ok(ResolvedLocalTime {
            instant: self.instant.parse().map_err(|_| restoration_error())?,
            local: self.local.parse().map_err(|_| restoration_error())?,
            offset_seconds: self.offset_seconds,
        })
    }
}

pub(crate) fn reconcile_settings(
    original: &ScenarioDocument,
    settings: &ScenarioSettings,
    restoration: Option<&Value>,
    control: &OperationControl,
) -> Result<DomainSettingsMutation> {
    control.check()?;
    validate_input_bounds(original, Some(control))?;
    let schemas = WorkforceSchemas::load()?;
    let domain = validate_document_with_schemas(original, &schemas, Some(control))?;
    control.check()?;
    let restoration = decode_restoration(restoration, control)?;
    if let Some(restoration) = &restoration {
        for entry in &restoration.times {
            control.check()?;
            require(
                matches!(
                    domain.entities.get(&entry.shift_id.as_entity_id()),
                    Some(WorkforceEntity::ShiftInstance(_))
                ),
                "restoration.times.shiftId",
                "restoration requires an existing stored shift instance",
            )?;
        }
    }
    let zone = TimeZone::get(settings.time_zone.as_str())
        .map_err(|_| invalid("settings.timeZone", "invalid scenario time zone"))?;
    let mut working = original.clone();
    working.settings = settings.clone();
    let mut inverse = Restoration {
        schema_version: 1,
        times: Vec::new(),
    };
    for (id, entity) in &domain.entities {
        control.check()?;
        let WorkforceEntity::ShiftInstance(instance) = entity else {
            continue;
        };
        let supplied = restoration.as_ref().and_then(|restoration| {
            restoration
                .times
                .binary_search_by_key(&instance.id, |entry| entry.shift_id)
                .ok()
                .map(|index| &restoration.times[index])
        });
        let record = working
            .domain
            .entities
            .get_mut(id)
            .and_then(Value::as_object_mut)
            .ok_or_else(restoration_error)?;
        let start_changed = reconcile_endpoint(
            record.get_mut("startsAt").ok_or_else(restoration_error)?,
            &instance.starts_at,
            supplied.map(|entry| &entry.starts_at),
            settings,
            &zone,
        )?;
        let end_changed = reconcile_endpoint(
            record.get_mut("endsAt").ok_or_else(restoration_error)?,
            &instance.ends_at,
            supplied.map(|entry| &entry.ends_at),
            settings,
            &zone,
        )?;
        if start_changed || end_changed {
            require(
                inverse.times.len() < MAX_REFERENCE_ITEMS,
                "restoration.times",
                "too many restored shifts",
            )?;
            let original_record = original
                .domain
                .entities
                .get(id)
                .ok_or_else(restoration_error)?;
            inverse.times.push(RestoredTimes {
                shift_id: instance.id,
                starts_at: RawTime::deserialize(&original_record["startsAt"])
                    .map_err(|_| restoration_error())?,
                ends_at: RawTime::deserialize(&original_record["endsAt"])
                    .map_err(|_| restoration_error())?,
            });
        }
    }
    control.check()?;
    validate_document_with_schemas(&working, &schemas, Some(control))?;
    let inverse_payload = if inverse.times.is_empty() {
        None
    } else {
        let limit = usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES)
            .map_err(|_| DomainPackError::BatchInverseTooLarge)?;
        bounded_json_size(&inverse, limit).map_err(|_| DomainPackError::BatchInverseTooLarge)?;
        Some(serde_json::to_value(inverse).map_err(|_| restoration_error())?)
    };
    control.check()?;
    Ok(DomainSettingsMutation {
        domain: working.domain,
        inverse_payload,
    })
}

fn decode_restoration<'a>(
    value: Option<&'a Value>,
    control: &OperationControl,
) -> Result<Option<Restoration<'a>>> {
    let Some(value) = value else { return Ok(None) };
    validate_input_bounds(value, Some(control))?;
    let times = value
        .get("times")
        .and_then(Value::as_array)
        .ok_or_else(restoration_error)?;
    require(
        times.len() <= MAX_REFERENCE_ITEMS,
        "restoration.times",
        "too many restored shifts",
    )?;
    let restoration = Restoration::deserialize(value).map_err(|_| restoration_error())?;
    if restoration.schema_version != 1 {
        return Err(DomainPackError::UnsupportedVersion(
            restoration.schema_version,
        ));
    }
    require(
        restoration
            .times
            .windows(2)
            .all(|pair| pair[0].shift_id < pair[1].shift_id),
        "restoration.times.shiftId",
        "restored shift identities must be unique and sorted",
    )?;
    control.check()?;
    Ok(Some(restoration))
}

fn reconcile_endpoint(
    raw: &mut Value,
    original: &ResolvedLocalTime,
    supplied: Option<&RawTime<'_>>,
    settings: &ScenarioSettings,
    zone: &TimeZone,
) -> Result<bool> {
    if let Some(supplied) = supplied {
        let resolved = supplied.resolved()?;
        require(
            resolved.instant == original.instant,
            "restoration.times",
            "restoration cannot move a stored instant",
        )?;
        validate_resolved_time(&resolved, settings, zone)?;
        if RawTime::deserialize(&*raw).map_err(|_| restoration_error())? == *supplied {
            return Ok(false);
        }
        *raw = serde_json::to_value(supplied).map_err(|_| restoration_error())?;
        return Ok(true);
    }
    if validate_resolved_time(original, settings, zone).is_ok() {
        return Ok(false);
    }
    let actual = original.instant.as_timestamp().to_zoned(zone.clone());
    let raw = raw.as_object_mut().ok_or_else(restoration_error)?;
    raw.insert(
        "local".to_owned(),
        Value::String(actual.datetime().to_string()),
    );
    raw.insert(
        "offsetSeconds".to_owned(),
        Value::from(actual.offset().seconds()),
    );
    Ok(true)
}

fn restoration_error() -> DomainPackError {
    invalid("restoration", "invalid Workforce settings restoration")
}
