use super::types::{
    BlankPolicy, CsvError, CsvErrorCode, CsvRecord, FieldEdit, MAX_CSV_CELL_BYTES, MAX_CSV_COLUMNS,
    MAX_CSV_MAPPING_BYTES, PeopleCsvMapping, PersonField, PersonPatch, RowRejectionCode,
};
use crate::validation::{MAX_DISPLAY_BYTES, MAX_REFERENCE_ITEMS, MAX_TOKEN_BYTES};
use eutheto_domain_api::{ContractJsonLimits, bounded_json_size};
use eutheto_types::{
    EntityId, PortableJsonLimits, TypedUuidError, validate_nonsecret_portable_json_bytes,
};
use serde_json::Value;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// Checks caller-owned, fixed-depth policy before any clone or serialization.
pub(crate) fn validate_mapping(mapping: &PeopleCsvMapping) -> Result<(), CsvError> {
    let invalid = || CsvError::source(CsvErrorCode::InvalidMapping);
    let limit = || CsvError::source(CsvErrorCode::MappingLimit);
    if mapping.expected_columns == 0 || usize::from(mapping.expected_columns) > MAX_CSV_COLUMNS {
        return Err(invalid());
    }
    if mapping.columns.len() > 10 {
        return Err(limit());
    }
    let defaults = &mapping.new_person_defaults;
    if defaults.qualification_grants.len() > MAX_REFERENCE_ITEMS
        || defaults.eligible_assignment_type_ids.len() > MAX_REFERENCE_ITEMS
        || defaults.tags.len() > MAX_REFERENCE_ITEMS
        || defaults.team_ids.len() > MAX_REFERENCE_ITEMS
        || mapping.reference_mappings.len() > MAX_REFERENCE_ITEMS
    {
        return Err(limit());
    }
    // Length checks precede character scans and the bounded serializer, including for
    // programmatically constructed policy values that never passed a JSON decoder.
    for tag in &defaults.tags {
        if tag.len() > MAX_DISPLAY_BYTES {
            return Err(limit());
        }
    }
    if let Some(display) = &defaults.display
        && (display.color.as_ref().is_some_and(|color| color.len() > 7)
            || display
                .avatar_initials
                .as_ref()
                .is_some_and(|initials| initials.len() > 16))
    {
        return Err(limit());
    }
    let mut columns = [false; MAX_CSV_COLUMNS];
    let mut fields = [None; 10];
    for (position, column) in mapping.columns.iter().enumerate() {
        let index = usize::from(column.index);
        if column.index >= mapping.expected_columns
            || columns[index]
            || fields[..position].contains(&Some(column.field))
            || (column.blank == BlankPolicy::Clear
                && matches!(
                    column.field,
                    PersonField::Name | PersonField::ActiveRange | PersonField::WorkloadWeight
                ))
        {
            return Err(invalid());
        }
        columns[index] = true;
        fields[position] = Some(column.field);
    }
    for (token, id) in &mapping.reference_mappings {
        if token.len() > MAX_TOKEN_BYTES {
            return Err(limit());
        }
        if token.is_empty()
            || token.chars().any(char::is_control)
            || !matches!(
                token.parse::<EntityId>(),
                Err(TypedUuidError::InvalidUuid(_))
            )
            || id.as_uuid().get_version_num() != 7
        {
            return Err(invalid());
        }
    }
    bounded_json_size(mapping, MAX_CSV_MAPPING_BYTES).map_err(|_| limit())?;
    Ok(())
}

/// The caller validates mapping once; cells are borrowed and never modified.
pub(crate) fn map_row(
    record: CsvRecord<'_>,
    mapping: &PeopleCsvMapping,
) -> Result<PersonPatch, RowRejectionCode> {
    if record.cells.len() != usize::from(mapping.expected_columns) {
        return Err(RowRejectionCode::ColumnCount);
    }
    let mut fields = BTreeMap::new();
    for column in &mapping.columns {
        let cell = record
            .cells
            .get(usize::from(column.index))
            .ok_or(RowRejectionCode::ColumnCount)?;
        if cell.is_empty() {
            if column.blank == BlankPolicy::Clear {
                let edit = match column.field {
                    PersonField::ExternalId
                    | PersonField::HomeLocationId
                    | PersonField::WorkloadTarget => FieldEdit::Remove,
                    PersonField::QualificationGrants
                    | PersonField::EligibleAssignmentTypeIds
                    | PersonField::Tags
                    | PersonField::TeamIds => FieldEdit::Set(Value::Array(Vec::new())),
                    PersonField::Name | PersonField::ActiveRange | PersonField::WorkloadWeight => {
                        return Err(RowRejectionCode::InvalidCell);
                    }
                };
                fields.insert(column.field, edit);
            }
            continue;
        }
        if cell.len() > MAX_CSV_CELL_BYTES {
            return Err(RowRejectionCode::InvalidCell);
        }
        let value = match column.field {
            PersonField::Name | PersonField::ExternalId => Value::String((*cell).to_owned()),
            PersonField::HomeLocationId => Value::String(resolve_reference(cell, mapping)?),
            field => structured_cell(cell, field, mapping)?,
        };
        fields.insert(column.field, FieldEdit::Set(value));
    }
    Ok(PersonPatch { fields })
}

fn structured_cell(
    cell: &str,
    field: PersonField,
    mapping: &PeopleCsvMapping,
) -> Result<Value, RowRejectionCode> {
    let limits = PortableJsonLimits {
        max_depth: ContractJsonLimits::DEFAULT.max_depth,
        max_string_bytes: ContractJsonLimits::DEFAULT.max_string_bytes,
        max_collection_items: ContractJsonLimits::DEFAULT.max_collection_items,
    };
    // Streaming syntax/safety/depth/duplicate-key checks run before building a Value.
    validate_nonsecret_portable_json_bytes(cell.as_bytes(), &limits)
        .map_err(|_| RowRejectionCode::InvalidCell)?;
    let mut value: Value = serde_json::from_str(cell).map_err(|_| RowRejectionCode::InvalidCell)?;
    match field {
        PersonField::ActiveRange | PersonField::WorkloadWeight | PersonField::WorkloadTarget => {
            let object = value.as_object_mut().ok_or(RowRejectionCode::InvalidCell)?;
            if field == PersonField::WorkloadTarget {
                for key in ["bucketId", "calendarId"] {
                    resolve_value(
                        object.get_mut(key).ok_or(RowRejectionCode::InvalidCell)?,
                        mapping,
                    )?;
                }
            }
        }
        PersonField::QualificationGrants
        | PersonField::EligibleAssignmentTypeIds
        | PersonField::Tags
        | PersonField::TeamIds => {
            let items = value.as_array_mut().ok_or(RowRejectionCode::InvalidCell)?;
            if items.len() > MAX_REFERENCE_ITEMS {
                return Err(RowRejectionCode::InvalidCell);
            }
            for item in items {
                match field {
                    PersonField::QualificationGrants => {
                        let reference = item
                            .as_object_mut()
                            .and_then(|grant| grant.get_mut("qualificationId"))
                            .ok_or(RowRejectionCode::InvalidCell)?;
                        resolve_value(reference, mapping)?;
                    }
                    PersonField::EligibleAssignmentTypeIds | PersonField::TeamIds => {
                        resolve_value(item, mapping)?;
                    }
                    _ => {}
                }
            }
        }
        _ => return Err(RowRejectionCode::InvalidCell),
    }
    // Do not round-trip through a typed model: unknown keys, optional nulls and
    // alternate nested shapes must survive for generated candidate-schema rejection.
    Ok(value)
}

fn resolve_value(value: &mut Value, mapping: &PeopleCsvMapping) -> Result<(), RowRejectionCode> {
    let text = value.as_str().ok_or(RowRejectionCode::InvalidReference)?;
    if text.parse::<EntityId>().is_ok() {
        return Ok(()); // Keep direct UUID spelling, including hexadecimal letter case.
    }
    *value = Value::String(resolve_reference(text, mapping)?);
    Ok(())
}

fn resolve_reference(text: &str, mapping: &PeopleCsvMapping) -> Result<String, RowRejectionCode> {
    if text.parse::<EntityId>().is_ok() {
        return Ok(text.to_owned());
    }
    mapping
        .reference_mappings
        .get(text)
        .map(ToString::to_string)
        .ok_or(RowRejectionCode::InvalidReference)
}
