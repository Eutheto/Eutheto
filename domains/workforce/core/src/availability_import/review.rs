use super::{
    AvailabilityCsvDecision, AvailabilityCsvExcludedRow, AvailabilityCsvField,
    AvailabilityCsvIdentity, AvailabilityCsvMapping, AvailabilityCsvReview,
    AvailabilityCsvRowDecision, AvailabilityCsvRowIssue, AvailabilityCsvRowStatus,
    AvailabilityImportPreview, AvailabilityImportRow, MAX_AVAILABILITY_CSV_REPORT_BYTES,
};
use crate::{
    commands,
    ids::AvailabilityId,
    model::{Availability, TimeWindow, WorkforceDomainV1, WorkforceEntity},
    people_csv::{
        BlankPolicy, CSV_LIMITS_VERSION, CSV_PARSER_VERSION, CsvError, CsvErrorCode, CsvRecord,
        CsvSource, MAX_CSV_COLUMNS, MAX_CSV_DATA_RECORDS, MAX_CSV_DECISION_BYTES,
        MAX_CSV_DECISIONS, MAX_CSV_INSPECTED_CELL_BYTES, MAX_CSV_MAPPING_BYTES,
        MAX_CSV_PREVIEW_BYTES, MAX_CSV_REVIEW_BYTES, MAX_CSV_SOURCE_BYTES, PeopleImportDisposition,
        hash, scan_csv, valid_digest, valid_identity_alias,
    },
    validation::{
        MAX_REFERENCE_ITEMS, WorkforceSchemas, validate_document_with_schemas,
        validate_value_bounds,
    },
};
use eutheto_domain_api::{
    ContractJsonLimits, DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainPackError,
    MAX_DOMAIN_BATCH_COMMANDS, bounded_json_size,
};
use eutheto_types::{
    CancellationToken, DomainCommandEnvelope, EntityId, OperationControl, PortableJsonLimits,
    Revision, ScenarioDocument, TypedUuidError, ValidationIssue, resolve_local_time,
    validate_nonsecret_portable_json_bytes, visit_document_identities,
};
use serde::{Deserialize, Serialize, ser::SerializeMap};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

const REVIEW_FORMAT: &str = "eutheto/workforce-availability-csv-review";

fn invalid(code: CsvErrorCode) -> CsvError {
    CsvError::source(code)
}
fn check(cancellation: &CancellationToken) -> Result<(), CsvError> {
    if cancellation.is_cancelled() {
        Err(invalid(CsvErrorCode::Cancelled))
    } else {
        Ok(())
    }
}
fn document_error(error: &DomainPackError) -> CsvError {
    invalid(if matches!(error, DomainPackError::Cancelled) {
        CsvErrorCode::Cancelled
    } else {
        CsvErrorCode::InvalidCurrentDocument
    })
}
fn json_limits() -> PortableJsonLimits {
    PortableJsonLimits {
        max_depth: ContractJsonLimits::DEFAULT.max_depth,
        max_string_bytes: ContractJsonLimits::DEFAULT.max_string_bytes,
        max_collection_items: ContractJsonLimits::DEFAULT.max_collection_items,
    }
}

fn key(field: AvailabilityCsvField) -> &'static str {
    match field {
        AvailabilityCsvField::PersonId => "personId",
        AvailabilityCsvField::AvailabilityKind => "availabilityKind",
        AvailabilityCsvField::StartsAt
        | AvailabilityCsvField::EndsAt
        | AvailabilityCsvField::WeeklyWindows => "timeWindow",
        AvailabilityCsvField::EffectiveStartDate
        | AvailabilityCsvField::EffectiveEndDateExclusive => "effectiveRange",
        AvailabilityCsvField::AssignmentTypeIds => "assignmentTypeIds",
        AvailabilityCsvField::LocationIds => "locationIds",
        AvailabilityCsvField::Source => "source",
        AvailabilityCsvField::Note => "note",
    }
}

fn validate_policy(
    mapping: &AvailabilityCsvMapping,
    decisions: &[AvailabilityCsvRowDecision],
) -> Result<(), CsvError> {
    if mapping.columns.len() > 11
        || [
            &mapping.person_mappings,
            &mapping.assignment_type_mappings,
            &mapping.location_mappings,
        ]
        .iter()
        .any(|map| map.len() > MAX_REFERENCE_ITEMS)
    {
        return Err(invalid(CsvErrorCode::MappingLimit));
    }
    if mapping.expected_columns == 0 || usize::from(mapping.expected_columns) > MAX_CSV_COLUMNS {
        return Err(invalid(CsvErrorCode::InvalidMapping));
    }
    let mut columns = BTreeSet::new();
    let mut fields = BTreeSet::new();
    if let Some(index) = mapping.identity_column {
        if index >= mapping.expected_columns {
            return Err(invalid(CsvErrorCode::InvalidMapping));
        }
        columns.insert(index);
    }
    for column in &mapping.columns {
        if column.index >= mapping.expected_columns
            || !columns.insert(column.index)
            || !fields.insert(column.field)
            || (column.blank == BlankPolicy::Clear
                && !matches!(
                    column.field,
                    AvailabilityCsvField::AssignmentTypeIds
                        | AvailabilityCsvField::LocationIds
                        | AvailabilityCsvField::Source
                        | AvailabilityCsvField::Note
                ))
        {
            return Err(invalid(CsvErrorCode::InvalidMapping));
        }
    }
    for (token, id) in mapping
        .person_mappings
        .iter()
        .chain(&mapping.assignment_type_mappings)
        .chain(&mapping.location_mappings)
    {
        if token.len() > MAX_CSV_INSPECTED_CELL_BYTES {
            return Err(invalid(CsvErrorCode::MappingLimit));
        }
        if !valid_identity_alias(token) || id.as_uuid().get_version_num() != 7 {
            return Err(invalid(CsvErrorCode::InvalidMapping));
        }
    }
    bounded_json_size(mapping, MAX_CSV_MAPPING_BYTES)
        .map_err(|_| invalid(CsvErrorCode::MappingLimit))?;
    if decisions.len() > MAX_CSV_DECISIONS {
        return Err(invalid(CsvErrorCode::DecisionLimit));
    }
    bounded_json_size(decisions, MAX_CSV_DECISION_BYTES)
        .map_err(|_| invalid(CsvErrorCode::DecisionLimit))?;
    if decisions
        .windows(2)
        .any(|pair| pair[0].record >= pair[1].record)
        || decisions.iter().any(|row| {
            row.record <= u32::from(mapping.has_header)
                || row.record > MAX_CSV_DATA_RECORDS + u32::from(mapping.has_header)
                || match row.decision {
                    AvailabilityCsvDecision::Add { availability_id }
                    | AvailabilityCsvDecision::Update { availability_id } => {
                        availability_id.as_entity_id().as_uuid().get_version_num() != 7
                    }
                    AvailabilityCsvDecision::Skip {} => false,
                }
        })
    {
        return Err(invalid(CsvErrorCode::InvalidDecision));
    }
    Ok(())
}

fn stored(domain: &WorkforceDomainV1, id: AvailabilityId) -> Option<&Availability> {
    match domain.entities.get(&id.as_entity_id()) {
        Some(WorkforceEntity::Availability(value)) => Some(value),
        _ => None,
    }
}
fn identity(record: &CsvRecord<'_>, mapping: &AvailabilityCsvMapping) -> AvailabilityCsvIdentity {
    let token = mapping
        .identity_column
        .and_then(|column| record.cells.get(usize::from(column)))
        .copied()
        .unwrap_or("");
    if token.is_empty() {
        return AvailabilityCsvIdentity::Absent {};
    }
    if token.len() > MAX_CSV_INSPECTED_CELL_BYTES || token.chars().any(char::is_control) {
        return AvailabilityCsvIdentity::Invalid {};
    }
    match token.parse() {
        Ok(availability_id) => AvailabilityCsvIdentity::Supplied { availability_id },
        Err(_) => AvailabilityCsvIdentity::Invalid {},
    }
}

fn reference(
    value: &mut Value,
    kind: &str,
    mapping: &BTreeMap<String, EntityId>,
    document: &ScenarioDocument,
) -> Result<EntityId, AvailabilityCsvRowIssue> {
    let text = value
        .as_str()
        .ok_or(AvailabilityCsvRowIssue::InvalidReference)?;
    if text.len() > MAX_CSV_INSPECTED_CELL_BYTES || text.chars().any(char::is_control) {
        return Err(AvailabilityCsvRowIssue::InvalidReference);
    }
    let (id, alias) = match text.parse::<EntityId>() {
        Ok(id) => (id, false),
        Err(TypedUuidError::InvalidUuid(_)) => (
            mapping
                .get(text)
                .copied()
                .ok_or(AvailabilityCsvRowIssue::InvalidReference)?,
            true,
        ),
        Err(_) => return Err(AvailabilityCsvRowIssue::InvalidReference),
    };
    if document
        .domain
        .entities
        .get(&id)
        .and_then(|raw| raw.get("kind"))
        .and_then(Value::as_str)
        != Some(kind)
    {
        return Err(AvailabilityCsvRowIssue::InvalidReference);
    }
    if alias {
        *value = Value::String(id.to_string());
    }
    Ok(id)
}

// Edits retain submitted raw structured JSON. Candidate-schema validation, not typed
// reserialization, rejects unknown keys, explicit optional nulls and alternate shapes.
type Edits = BTreeMap<String, Option<Value>>;
struct MappedEdits {
    edits: Edits,
    starts: Option<eutheto_types::ResolvedLocalTime>,
    ends: Option<eutheto_types::ResolvedLocalTime>,
}

fn parse_json(cell: &str) -> Result<Value, AvailabilityCsvRowIssue> {
    validate_nonsecret_portable_json_bytes(cell.as_bytes(), &json_limits())
        .map_err(|_| AvailabilityCsvRowIssue::InvalidCell)?;
    serde_json::from_str(cell).map_err(|_| AvailabilityCsvRowIssue::InvalidCell)
}

fn filter_references(
    value: &mut Value,
    kind: &str,
    mappings: &BTreeMap<String, EntityId>,
    document: &ScenarioDocument,
) -> Result<(), AvailabilityCsvRowIssue> {
    let values = value
        .as_array_mut()
        .ok_or(AvailabilityCsvRowIssue::InvalidCell)?;
    if values.is_empty() {
        return Err(AvailabilityCsvRowIssue::InvalidCell);
    }
    let mut seen = BTreeSet::new();
    for value in values {
        let id = reference(value, kind, mappings, document)?;
        if !seen.insert(id) {
            return Err(AvailabilityCsvRowIssue::InvalidReference);
        }
    }
    Ok(())
}

fn local_endpoint(
    cell: &str,
    document: &ScenarioDocument,
) -> Result<eutheto_types::ResolvedLocalTime, AvailabilityCsvRowIssue> {
    let local =
        crate::model::parse_local_intent(cell).ok_or(AvailabilityCsvRowIssue::InvalidLocalTime)?;
    resolve_local_time(
        local,
        &document.settings.time_zone,
        document.settings.gap_policy,
        document.settings.overlap_policy,
    )
    .map_err(|_| AvailabilityCsvRowIssue::UnresolvableLocalTime)
}

fn ordinary_cell(
    field: AvailabilityCsvField,
    cell: &str,
    mapping: &AvailabilityCsvMapping,
    document: &ScenarioDocument,
) -> Result<Value, AvailabilityCsvRowIssue> {
    let mut value = if matches!(
        field,
        AvailabilityCsvField::AssignmentTypeIds | AvailabilityCsvField::LocationIds
    ) {
        parse_json(cell)?
    } else {
        json!(cell)
    };
    match field {
        AvailabilityCsvField::PersonId => {
            reference(&mut value, "person", &mapping.person_mappings, document)?;
        }
        AvailabilityCsvField::AssignmentTypeIds => filter_references(
            &mut value,
            "assignmentType",
            &mapping.assignment_type_mappings,
            document,
        )?,
        AvailabilityCsvField::LocationIds => {
            filter_references(&mut value, "location", &mapping.location_mappings, document)?;
        }
        _ => {}
    }
    validate_value_bounds(&value, "/availabilityCsv/field")
        .map_err(|_| AvailabilityCsvRowIssue::InvalidCell)?;
    Ok(value)
}

fn required_cells(
    record: &CsvRecord<'_>,
    mapping: &AvailabilityCsvMapping,
) -> Result<(), AvailabilityCsvRowIssue> {
    for field in [
        AvailabilityCsvField::PersonId,
        AvailabilityCsvField::AvailabilityKind,
        AvailabilityCsvField::EffectiveStartDate,
        AvailabilityCsvField::EffectiveEndDateExclusive,
    ] {
        if !mapping.columns.iter().any(|column| {
            column.field == field && !record.cells[usize::from(column.index)].is_empty()
        }) {
            return Err(AvailabilityCsvRowIssue::MissingRequiredField);
        }
    }
    Ok(())
}

fn mapped_edits(
    record: &CsvRecord<'_>,
    mapping: &AvailabilityCsvMapping,
    document: &ScenarioDocument,
    original: Option<&Map<String, Value>>,
    add: bool,
) -> Result<MappedEdits, AvailabilityCsvRowIssue> {
    if record.cells.len() != usize::from(mapping.expected_columns) {
        return Err(AvailabilityCsvRowIssue::ColumnCount);
    }
    if add {
        required_cells(record, mapping)?;
    }
    let mut mapped = MappedEdits {
        edits: Edits::new(),
        starts: None,
        ends: None,
    };
    let mut dates = Map::new();
    let mut weekly = None;
    for column in &mapping.columns {
        let cell = record.cells[usize::from(column.index)];
        if cell.is_empty() {
            if column.blank == BlankPolicy::Clear {
                mapped.edits.insert(
                    key(column.field).to_owned(),
                    if matches!(
                        column.field,
                        AvailabilityCsvField::Source | AvailabilityCsvField::Note
                    ) {
                        Some(json!(""))
                    } else {
                        None
                    },
                );
            }
            continue;
        }
        match column.field {
            AvailabilityCsvField::StartsAt => mapped.starts = Some(local_endpoint(cell, document)?),
            AvailabilityCsvField::EndsAt => mapped.ends = Some(local_endpoint(cell, document)?),
            AvailabilityCsvField::WeeklyWindows => weekly = Some(parse_json(cell)?),
            AvailabilityCsvField::EffectiveStartDate => {
                dates.insert("startDate".to_owned(), json!(cell));
            }
            AvailabilityCsvField::EffectiveEndDateExclusive => {
                dates.insert("endDateExclusive".to_owned(), json!(cell));
            }
            field => {
                mapped.edits.insert(
                    key(field).to_owned(),
                    Some(ordinary_cell(field, cell, mapping, document)?),
                );
            }
        }
    }
    if !dates.is_empty() {
        let mut range = original
            .and_then(|raw| raw.get("effectiveRange"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        range.extend(dates);
        mapped
            .edits
            .insert("effectiveRange".to_owned(), Some(Value::Object(range)));
    }
    window_edits(&mut mapped, weekly, original, add)?;
    Ok(mapped)
}

fn window_edits(
    mapped: &mut MappedEdits,
    weekly: Option<Value>,
    original: Option<&Map<String, Value>>,
    add: bool,
) -> Result<(), AvailabilityCsvRowIssue> {
    let instant_edit = mapped.starts.is_some() || mapped.ends.is_some();
    if let Some(windows) = weekly {
        if instant_edit {
            return Err(AvailabilityCsvRowIssue::ConflictingTimeWindow);
        }
        mapped.edits.insert(
            "timeWindow".to_owned(),
            Some(json!({"kind":"weekly", "windows":windows})),
        );
    } else if instant_edit {
        let existing = original
            .and_then(|raw| raw.get("timeWindow"))
            .filter(|window| window["kind"] == "instant")
            .and_then(Value::as_object);
        if existing.is_none() && (mapped.starts.is_none() || mapped.ends.is_none()) {
            return Err(AvailabilityCsvRowIssue::MissingRequiredField);
        }
        let mut window = existing
            .cloned()
            .unwrap_or_else(|| Map::from_iter([("kind".to_owned(), json!("instant"))]));
        if let Some(resolved) = &mapped.starts {
            window.insert("startsAt".to_owned(), json!(resolved.instant));
        }
        if let Some(resolved) = &mapped.ends {
            window.insert("endsAt".to_owned(), json!(resolved.instant));
        }
        mapped
            .edits
            .insert("timeWindow".to_owned(), Some(Value::Object(window)));
    } else if add {
        return Err(AvailabilityCsvRowIssue::MissingRequiredField);
    }
    Ok(())
}

// Serialize a borrowed candidate for admission BEFORE cloning any old/projection tree.
struct Overlay<'a> {
    original: Option<&'a Map<String, Value>>,
    edits: &'a Edits,
}
impl Serialize for Overlay<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if let Some(original) = self.original {
            for (key, value) in original {
                if !self.edits.contains_key(key) {
                    map.serialize_entry(key, value)?;
                }
            }
        }
        for (key, value) in self.edits {
            if let Some(value) = value {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}
fn materialize(original: Option<&Map<String, Value>>, edits: Edits) -> Value {
    let mut object = Map::new();
    if let Some(original) = original {
        for (key, value) in original {
            if !edits.contains_key(key) {
                object.insert(key.clone(), value.clone());
            }
        }
    }
    for (key, value) in edits {
        if let Some(value) = value {
            object.insert(key, value);
        }
    }
    Value::Object(object)
}
fn set_equal<T: Ord>(left: Option<&[T]>, right: Option<&[T]>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left == right
                || (left.len() == right.len()
                    && left.iter().collect::<BTreeSet<_>>()
                        == right.iter().collect::<BTreeSet<_>>())
        }
        _ => false,
    }
}
fn window_equal(left: &TimeWindow, right: &TimeWindow) -> bool {
    match (left, right) {
        (
            TimeWindow::Instant {
                starts_at: a,
                ends_at: b,
            },
            TimeWindow::Instant {
                starts_at: c,
                ends_at: d,
            },
        ) => a == c && b == d,
        (TimeWindow::Weekly { windows: a }, TimeWindow::Weekly { windows: b }) => {
            a.len() == b.len()
                && a.iter().zip(b).all(|(a, b)| {
                    a.start_time == b.start_time
                        && a.end_time == b.end_time
                        && a.end_day_offset == b.end_day_offset
                        && a.weekdays.len() == b.weekdays.len()
                        && a.weekdays.iter().all(|day| b.weekdays.contains(day))
                })
        }
        _ => false,
    }
}
fn field_equal(field: AvailabilityCsvField, before: &Availability, after: &Availability) -> bool {
    match field {
        AvailabilityCsvField::PersonId => before.person_id == after.person_id,
        AvailabilityCsvField::AvailabilityKind => {
            before.availability_kind == after.availability_kind
        }
        AvailabilityCsvField::StartsAt
        | AvailabilityCsvField::EndsAt
        | AvailabilityCsvField::WeeklyWindows => {
            window_equal(&before.time_window, &after.time_window)
        }
        AvailabilityCsvField::EffectiveStartDate
        | AvailabilityCsvField::EffectiveEndDateExclusive => {
            before.effective_range == after.effective_range
        }
        AvailabilityCsvField::AssignmentTypeIds => set_equal(
            before.assignment_type_ids.as_deref(),
            after.assignment_type_ids.as_deref(),
        ),
        AvailabilityCsvField::LocationIds => set_equal(
            before.location_ids.as_deref(),
            after.location_ids.as_deref(),
        ),
        AvailabilityCsvField::Source => before.source == after.source,
        AvailabilityCsvField::Note => before.note == after.note,
    }
}

type Preservation<'a> = BTreeMap<(&'static str, &'static str), &'a Value>;

fn preservation<'a>(
    original: Option<&'a Value>,
    before: Option<&Availability>,
    after: &Availability,
) -> Preservation<'a> {
    let mut preserved = Preservation::new();
    let (Some(original), Some(before)) = (original, before) else {
        return preserved;
    };
    for field in [
        AvailabilityCsvField::PersonId,
        AvailabilityCsvField::AvailabilityKind,
        AvailabilityCsvField::WeeklyWindows,
        AvailabilityCsvField::EffectiveStartDate,
        AvailabilityCsvField::AssignmentTypeIds,
        AvailabilityCsvField::LocationIds,
        AvailabilityCsvField::Source,
        AvailabilityCsvField::Note,
    ] {
        if field_equal(field, before, after)
            && let Some(value) = original.get(key(field))
        {
            preserved.insert((key(field), ""), value);
        }
    }
    for (name, equivalent) in [
        (
            "startDate",
            before.effective_range.start_date == after.effective_range.start_date,
        ),
        (
            "endDateExclusive",
            before.effective_range.end_date_exclusive == after.effective_range.end_date_exclusive,
        ),
    ] {
        if equivalent
            && !preserved.contains_key(&("effectiveRange", ""))
            && let Some(value) = original["effectiveRange"].get(name)
        {
            preserved.insert(("effectiveRange", name), value);
        }
    }
    if let (
        TimeWindow::Instant {
            starts_at: a,
            ends_at: b,
        },
        TimeWindow::Instant {
            starts_at: c,
            ends_at: d,
        },
    ) = (&before.time_window, &after.time_window)
    {
        for (name, equivalent) in [("startsAt", a == c), ("endsAt", b == d)] {
            if equivalent
                && !preserved.contains_key(&("timeWindow", ""))
                && let Some(value) = original["timeWindow"].get(name)
            {
                preserved.insert(("timeWindow", name), value);
            }
        }
    }
    preserved
}

struct Projection<'a> {
    candidate: &'a Value,
    preserved: &'a Preservation<'a>,
}
struct NestedProjection<'a> {
    root: &'a str,
    candidate: &'a Map<String, Value>,
    preserved: &'a Preservation<'a>,
}
impl Serialize for NestedProjection<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.candidate.len()))?;
        for (name, value) in self.candidate {
            map.serialize_entry(
                name,
                self.preserved
                    .get(&(self.root, name.as_str()))
                    .copied()
                    .unwrap_or(value),
            )?;
        }
        map.end()
    }
}
impl Serialize for Projection<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some(candidate) = self.candidate.as_object() else {
            return self.candidate.serialize(serializer);
        };
        let mut map = serializer.serialize_map(Some(candidate.len()))?;
        for (name, value) in candidate {
            if let Some(original) = self.preserved.get(&(name.as_str(), "")) {
                map.serialize_entry(name, original)?;
            } else if matches!(name.as_str(), "timeWindow" | "effectiveRange") {
                if let Some(candidate) = value.as_object() {
                    map.serialize_entry(
                        name,
                        &NestedProjection {
                            root: name,
                            candidate,
                            preserved: self.preserved,
                        },
                    )?;
                } else {
                    map.serialize_entry(name, value)?;
                }
            } else {
                map.serialize_entry(name, value)?;
            }
        }
        map.end()
    }
}

struct Budget {
    remaining: usize,
}
impl Budget {
    fn admit<T: Serialize + ?Sized>(&self, value: &T) -> Result<usize, CsvError> {
        bounded_json_size(value, self.remaining).map_err(|_| invalid(CsvErrorCode::PreviewLimit))
    }
    fn charge(&mut self, bytes: usize) -> Result<(), CsvError> {
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or_else(|| invalid(CsvErrorCode::PreviewLimit))?;
        Ok(())
    }
}

struct RowReview<'a> {
    document: &'a ScenarioDocument,
    scratch: ScenarioDocument,
    mapping: &'a AvailabilityCsvMapping,
    decisions: &'a [AvailabilityCsvRowDecision],
    schemas: WorkforceSchemas,
    domain: WorkforceDomainV1,
    owned: BTreeSet<uuid::Uuid>,
    reserved_ids: &'a BTreeSet<uuid::Uuid>,
    cancellation: &'a CancellationToken,
    control: OperationControl,
    budget: Budget,
    rows: Vec<AvailabilityImportRow>,
    targets: BTreeMap<AvailabilityId, Vec<usize>>,
}

impl<'a> RowReview<'a> {
    fn new(
        document: &'a ScenarioDocument,
        mapping: &'a AvailabilityCsvMapping,
        decisions: &'a [AvailabilityCsvRowDecision],
        reserved_ids: &'a BTreeSet<uuid::Uuid>,
        cancellation: &'a CancellationToken,
    ) -> Result<Self, CsvError> {
        check(cancellation)?;
        validate_policy(mapping, decisions)?;
        let control = OperationControl::Cancellation(cancellation.clone());
        let schemas = WorkforceSchemas::load().map_err(|error| document_error(&error))?;
        let domain = validate_document_with_schemas(document, &schemas, Some(&control))
            .map_err(|error| document_error(&error))?;
        let mut owned = BTreeSet::new();
        visit_document_identities(document, document.extensions.values(), &mut |id| {
            check(cancellation)?;
            owned.insert(id);
            Ok(())
        })?;
        check(cancellation)?;
        let mut budget = Budget {
            remaining: MAX_CSV_PREVIEW_BYTES,
        };
        budget.charge(4096 + budget.admit(mapping)? + budget.admit(decisions)?)?;
        Ok(Self {
            document,
            // Ordinary validation bounds this reusable native-only scratch snapshot.
            scratch: document.clone(),
            mapping,
            decisions,
            schemas,
            domain,
            owned,
            reserved_ids,
            cancellation,
            control,
            budget,
            rows: Vec::new(),
            targets: BTreeMap::new(),
        })
    }

    fn scan<R: Read + ?Sized>(&mut self, input: &mut R) -> Result<CsvSource, CsvError> {
        let source = scan_csv(input, self.mapping.dialect, self.cancellation, |record| {
            self.visit(record)
        })?;
        if source.logical_records < u32::from(self.mapping.has_header) {
            return Err(invalid(CsvErrorCode::HeaderMismatch));
        }
        if self
            .decisions
            .last()
            .is_some_and(|row| row.record > source.logical_records)
        {
            return Err(invalid(CsvErrorCode::InvalidDecision));
        }
        Ok(source)
    }

    fn visit(&mut self, record: CsvRecord<'_>) -> Result<(), CsvError> {
        check(self.cancellation)?;
        if self.mapping.has_header && record.number == 1 {
            return if record.cells.len() == usize::from(self.mapping.expected_columns) {
                Ok(())
            } else {
                Err(CsvError::at(CsvErrorCode::HeaderMismatch, record.number))
            };
        }
        if record.number - u32::from(self.mapping.has_header) > MAX_CSV_DATA_RECORDS {
            return Err(CsvError::at(CsvErrorCode::DataRecordLimit, record.number));
        }
        self.budget.charge(256)?; // Every row fact is admitted before retention.
        let identity = identity(&record, self.mapping);
        let decision = self
            .decisions
            .binary_search_by_key(&record.number, |row| row.record)
            .ok()
            .map(|index| self.decisions[index].decision);
        let mut row = AvailabilityImportRow {
            record: record.number,
            status: AvailabilityCsvRowStatus::Unresolved,
            identity,
            availability_id: None,
            before: None,
            after: None,
            resolved_starts_at: None,
            resolved_ends_at: None,
            issue: None,
        };
        if matches!(decision, Some(AvailabilityCsvDecision::Skip {})) {
            row.status = AvailabilityCsvRowStatus::Skipped;
            if let AvailabilityCsvIdentity::Supplied { availability_id } = identity {
                row.availability_id = Some(availability_id);
            }
        } else {
            let (target, add) = choose_target(&row, decision)?;
            row.availability_id = target;
            if !self.reserve_target(&mut row) {
                self.review_row(&record, &mut row, decision, add)?;
            }
        }
        self.rows.push(row);
        Ok(())
    }

    fn reserve_target(&mut self, row: &mut AvailabilityImportRow) -> bool {
        let Some(target) = row.availability_id else {
            return false;
        };
        let indices = self.targets.entry(target).or_default();
        let duplicate = !indices.is_empty();
        indices.push(self.rows.len());
        if duplicate {
            for &index in &indices[..indices.len() - 1] {
                block_row(
                    &mut self.rows[index],
                    AvailabilityCsvRowIssue::DuplicateTarget,
                );
                self.rows[index].before = None;
                self.rows[index].after = None;
            }
            block_row(row, AvailabilityCsvRowIssue::DuplicateTarget);
        }
        duplicate
    }

    fn identity_issue(
        &self,
        row: &AvailabilityImportRow,
        add: bool,
    ) -> Option<AvailabilityCsvRowIssue> {
        match row.identity {
            AvailabilityCsvIdentity::Invalid {} => Some(AvailabilityCsvRowIssue::InvalidIdentity),
            _ => row
                .availability_id
                .filter(|id| {
                    let uuid = id.as_entity_id().as_uuid();
                    (self.owned.contains(&uuid) || self.reserved_ids.contains(&uuid))
                        && (add
                            || stored(&self.domain, *id).is_none()
                            || self.reserved_ids.contains(&uuid))
                })
                .map(|_| AvailabilityCsvRowIssue::ReservedIdentity),
        }
    }

    fn review_row(
        &mut self,
        record: &CsvRecord<'_>,
        row: &mut AvailabilityImportRow,
        decision: Option<AvailabilityCsvDecision>,
        add: bool,
    ) -> Result<(), CsvError> {
        if let Some(issue) = self.identity_issue(row, add) {
            block_row(row, issue);
            return Ok(());
        }
        let target = row.availability_id;
        if target.is_none() || (!add && target.is_some_and(|id| stored(&self.domain, id).is_none()))
        {
            if target.is_some() && decision.is_some() {
                return Err(CsvError::at(CsvErrorCode::InvalidDecision, record.number));
            }
            row.issue = Some(AvailabilityCsvRowIssue::UnresolvedIdentity);
            // Unlike Skip, an unresolved row still checks its mapped cells.
            if let Err(issue) = mapped_edits(record, self.mapping, self.document, None, false) {
                block_row(row, issue);
            }
            return Ok(());
        }
        let target = target.ok_or_else(|| invalid(CsvErrorCode::InvalidDecision))?;
        let original = if add {
            None
        } else {
            self.document
                .domain
                .entities
                .get(&target.as_entity_id())
                .and_then(Value::as_object)
        };
        // Only bounded nested window/date leaves can be copied by mapping.
        if let Some(original) = original {
            self.budget.admit(&original.get("timeWindow"))?;
            self.budget.admit(&original.get("effectiveRange"))?;
        }
        match mapped_edits(record, self.mapping, self.document, original, add) {
            Err(issue) => block_row(row, issue),
            Ok(mapped) if !add && mapped.edits.is_empty() => {
                let original = self
                    .document
                    .domain
                    .entities
                    .get(&target.as_entity_id())
                    .ok_or_else(|| invalid(CsvErrorCode::InvalidCurrentDocument))?;
                self.budget.charge(self.budget.admit(original)?)?;
                row.before = Some(original.clone());
                row.status = AvailabilityCsvRowStatus::Unchanged;
            }
            Ok(mapped) => {
                let facts_bytes = self.budget.admit(&(&mapped.starts, &mapped.ends))?;
                self.budget.charge(facts_bytes)?;
                row.resolved_starts_at = mapped.starts;
                row.resolved_ends_at = mapped.ends;
                self.review_candidate(row, target, add, mapped.edits)?;
            }
        }
        Ok(())
    }

    fn review_candidate(
        &mut self,
        row: &mut AvailabilityImportRow,
        target: AvailabilityId,
        add: bool,
        mut edits: Edits,
    ) -> Result<(), CsvError> {
        let original = if add {
            None
        } else {
            self.document
                .domain
                .entities
                .get(&target.as_entity_id())
                .and_then(Value::as_object)
        };
        if add {
            edits.insert("kind".to_owned(), Some(json!("availability")));
            edits.insert("id".to_owned(), Some(json!(target)));
            edits
                .entry("source".to_owned())
                .or_insert_with(|| Some(json!("")));
            edits
                .entry("note".to_owned())
                .or_insert_with(|| Some(json!("")));
        }
        // Admit the borrowed raw candidate before cloning its retained old fields.
        self.budget.admit(&Overlay {
            original,
            edits: &edits,
        })?;
        let mut candidate = materialize(original, edits);
        if let Some(after) = self.validate_candidate(row, target, &mut candidate)? {
            self.accept_candidate(row, target, add, candidate, &after)?;
        }
        Ok(())
    }

    fn validate_candidate(
        &mut self,
        row: &mut AvailabilityImportRow,
        target: AvailabilityId,
        candidate: &mut Value,
    ) -> Result<Option<Availability>, CsvError> {
        let candidate_valid = validate_value_bounds(candidate, "/availabilityCsv/candidate")
            .is_ok()
            && self
                .schemas
                .entities
                .validate(candidate, ContractJsonLimits::DEFAULT)
                .is_ok();
        let typed: Option<WorkforceEntity> = if candidate_valid {
            Deserialize::deserialize(&*candidate).ok()
        } else {
            None
        };
        let interval_invalid = matches!(&typed, Some(WorkforceEntity::Availability(availability))
            if matches!(&availability.time_window, TimeWindow::Instant { starts_at, ends_at } if starts_at >= ends_at));
        let previous = self
            .scratch
            .domain
            .entities
            .insert(target.as_entity_id(), std::mem::take(candidate));
        let validation = if candidate_valid && !interval_invalid {
            validate_document_with_schemas(&self.scratch, &self.schemas, Some(&self.control))
                .map(|_| ())
        } else {
            Err(DomainPackError::InvalidPayload {
                path: "/availabilityCsv/candidate".to_owned(),
                message: "invalid availability".to_owned(),
            })
        };
        *candidate = self
            .scratch
            .domain
            .entities
            .remove(&target.as_entity_id())
            .ok_or_else(|| invalid(CsvErrorCode::InvalidCurrentDocument))?;
        if let Some(previous) = previous {
            self.scratch
                .domain
                .entities
                .insert(target.as_entity_id(), previous);
        }
        match validation {
            Err(DomainPackError::Cancelled) => Err(invalid(CsvErrorCode::Cancelled)),
            Err(_) => {
                block_row(
                    row,
                    if interval_invalid {
                        AvailabilityCsvRowIssue::NonpositiveInterval
                    } else {
                        AvailabilityCsvRowIssue::InvalidAvailability
                    },
                );
                Ok(None)
            }
            Ok(()) => match typed {
                Some(WorkforceEntity::Availability(after)) => Ok(Some(after)),
                _ => Err(invalid(CsvErrorCode::InvalidBatch)),
            },
        }
    }

    fn accept_candidate(
        &mut self,
        row: &mut AvailabilityImportRow,
        target: AvailabilityId,
        add: bool,
        mut candidate: Value,
        after: &Availability,
    ) -> Result<(), CsvError> {
        let original = if add {
            None
        } else {
            self.document.domain.entities.get(&target.as_entity_id())
        };
        let before = stored(&self.domain, target).filter(|_| !add);
        let changed = add
            || before.is_some_and(|before| {
                self.mapping
                    .columns
                    .iter()
                    .any(|column| !field_equal(column.field, before, after))
            });
        let preserved = preservation(original, before, after);
        let projected = Projection {
            candidate: &candidate,
            preserved: &preserved,
        };
        let before_bytes = original
            .map(|raw| self.budget.admit(raw))
            .transpose()?
            .unwrap_or(0);
        let after_bytes = if changed {
            self.budget.admit(&projected)?
        } else {
            0
        };
        // Admit every retained row tree and command copy before cloning.
        self.budget
            .charge(before_bytes + after_bytes * 2 + if changed { 128 } else { 0 })?;
        row.before = original.cloned();
        row.after = if changed {
            for ((root, leaf), raw) in preserved {
                let destination = if leaf.is_empty() {
                    candidate.get_mut(root)
                } else {
                    candidate
                        .get_mut(root)
                        .and_then(|value| value.get_mut(leaf))
                };
                let destination = destination.ok_or_else(|| invalid(CsvErrorCode::InvalidBatch))?;
                if destination != raw {
                    destination.clone_from(raw);
                }
            }
            Some(candidate)
        } else {
            None
        };
        row.status = if add {
            AvailabilityCsvRowStatus::Added
        } else if changed {
            AvailabilityCsvRowStatus::Updated
        } else {
            AvailabilityCsvRowStatus::Unchanged
        };
        Ok(())
    }

    fn excluded_rows(&mut self) -> Result<Vec<AvailabilityCsvExcludedRow>, CsvError> {
        let mut excluded_rows = Vec::new();
        let mut remaining = MAX_AVAILABILITY_CSV_REPORT_BYTES - 2;
        for row in &self.rows {
            check(self.cancellation)?;
            if matches!(
                row.status,
                AvailabilityCsvRowStatus::Skipped
                    | AvailabilityCsvRowStatus::Blocked
                    | AvailabilityCsvRowStatus::Unresolved
            ) {
                let excluded = AvailabilityCsvExcludedRow {
                    record: row.record,
                    skipped: row.status == AvailabilityCsvRowStatus::Skipped,
                    issue: row.issue,
                };
                let bytes = bounded_json_size(&excluded, remaining)
                    .map_err(|_| invalid(CsvErrorCode::RejectedReportLimit))?
                    + usize::from(!excluded_rows.is_empty());
                remaining = remaining
                    .checked_sub(bytes)
                    .ok_or_else(|| invalid(CsvErrorCode::RejectedReportLimit))?;
                self.budget.charge(bytes)?;
                excluded_rows.push(excluded);
            }
        }
        Ok(excluded_rows)
    }

    fn finish(
        mut self,
        source: CsvSource,
        revision: Revision,
    ) -> Result<AvailabilityImportPreview, CsvError> {
        let excluded_rows = self.excluded_rows()?;
        let mut blocked = self.rows.iter().any(|row| {
            matches!(
                row.status,
                AvailabilityCsvRowStatus::Blocked | AvailabilityCsvRowStatus::Unresolved
            )
        });
        let mut batch = proposed_batch(self.document, &self.rows, self.cancellation)?;
        let validation_issues =
            validate_proposal(self.document, &mut batch, &mut blocked, self.cancellation)?;
        check(self.cancellation)?;
        let review = if blocked {
            None
        } else {
            Some(AvailabilityCsvReview {
                format: REVIEW_FORMAT.to_owned(),
                schema_version: 1,
                scenario_id: self.document.scenario_id,
                revision,
                scenario_blake3: hash(&(self.document, self.reserved_ids))?,
                source: source.clone(),
                parser_version: CSV_PARSER_VERSION.to_owned(),
                limits_version: CSV_LIMITS_VERSION,
                mapping: self.mapping.clone(),
                decisions: self.decisions.to_vec(),
                rows_blake3: hash(&self.rows)?,
                changes_blake3: hash(&batch)?,
                excluded_blake3: hash(&excluded_rows)?,
                validation_blake3: hash(&validation_issues)?,
            })
        };
        let approval_digest = review.as_ref().map(approval_digest).transpose()?;
        let preview = AvailabilityImportPreview {
            schema_version: 1,
            disposition: if blocked {
                PeopleImportDisposition::Blocked
            } else if batch.is_some() {
                PeopleImportDisposition::Reviewable
            } else {
                PeopleImportDisposition::NoChanges
            },
            source,
            rows: self.rows,
            excluded_rows,
            validation_issues,
            batch,
            review,
            approval_digest,
        };
        bounded_json_size(&preview, MAX_CSV_PREVIEW_BYTES)
            .map_err(|_| invalid(CsvErrorCode::PreviewLimit))?;
        check(self.cancellation)?;
        Ok(preview)
    }
}

fn choose_target(
    row: &AvailabilityImportRow,
    decision: Option<AvailabilityCsvDecision>,
) -> Result<(Option<AvailabilityId>, bool), CsvError> {
    let supplied = match row.identity {
        AvailabilityCsvIdentity::Supplied { availability_id } => Some(availability_id),
        _ => None,
    };
    let (target, add) = match decision {
        Some(AvailabilityCsvDecision::Add { availability_id }) => (Some(availability_id), true),
        Some(AvailabilityCsvDecision::Update { availability_id }) => (Some(availability_id), false),
        Some(AvailabilityCsvDecision::Skip {}) => unreachable!(),
        None => (supplied, false),
    };
    if supplied.is_some() && target != supplied {
        return Err(CsvError::at(CsvErrorCode::InvalidDecision, row.record));
    }
    Ok((target, add))
}

fn block_row(row: &mut AvailabilityImportRow, issue: AvailabilityCsvRowIssue) {
    row.status = AvailabilityCsvRowStatus::Blocked;
    row.issue = Some(issue);
}

fn proposed_batch(
    document: &ScenarioDocument,
    rows: &[AvailabilityImportRow],
    cancellation: &CancellationToken,
) -> Result<Option<DomainBatchCommand>, CsvError> {
    let changes = rows
        .iter()
        .filter(|row| {
            matches!(
                row.status,
                AvailabilityCsvRowStatus::Added | AvailabilityCsvRowStatus::Updated
            )
        })
        .count();
    if changes > MAX_DOMAIN_BATCH_COMMANDS {
        return Err(invalid(CsvErrorCode::MutationLimit));
    }
    if changes == 0
        || rows.iter().any(|row| {
            matches!(
                row.status,
                AvailabilityCsvRowStatus::Blocked | AvailabilityCsvRowStatus::Unresolved
            )
        })
    {
        return Ok(None);
    }
    let mut envelopes = Vec::with_capacity(changes);
    for row in rows {
        check(cancellation)?;
        if let Some(after) = &row.after {
            // Tree bytes were charged by accept_candidate before this command clone.
            envelopes.push(DomainCommandEnvelope {
                command_type: if row.status == AvailabilityCsvRowStatus::Added {
                    commands::ADD_ENTITY
                } else {
                    commands::UPDATE_ENTITY
                }
                .to_owned(),
                payload: json!({"entity":after}),
            });
        }
    }
    let proposal = DomainBatchCommand {
        schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: document.domain_pack.id.clone(),
        scenario_schema_version: document.domain_pack.schema_version,
        label: Some("Import stored availability records from CSV".to_owned()),
        commands: envelopes,
    };
    proposal
        .validate_bounds()
        .map_err(|_| invalid(CsvErrorCode::InvalidBatch))?;
    Ok(Some(proposal))
}

fn validate_proposal(
    document: &ScenarioDocument,
    batch: &mut Option<DomainBatchCommand>,
    blocked: &mut bool,
    cancellation: &CancellationToken,
) -> Result<Vec<ValidationIssue>, CsvError> {
    let mut issues = Vec::new();
    if let Some(proposal) = batch {
        match commands::apply_batch_cancellable(document, proposal, cancellation) {
            Ok(_) => {}
            Err(DomainPackError::InvalidPayload { .. }) => {
                *blocked = true;
                *batch = None;
                issues.push(ValidationIssue {
                    code: "invalidProposedState".to_owned(),
                    severity: eutheto_types::ValidationSeverity::Error,
                    message: "The complete proposed availability changes violate scenario structure, references or capacity.".to_owned(),
                    field_path: Some("/availabilityCsv/proposedState".to_owned()), resource: None,
                });
            }
            Err(DomainPackError::Cancelled) => return Err(invalid(CsvErrorCode::Cancelled)),
            Err(_) => return Err(invalid(CsvErrorCode::InvalidBatch)),
        }
    }
    Ok(issues)
}

/// Reviews availability and time off against native scenario time policy.
///
/// # Errors
/// Rejects unsafe/bounded source, policy, decisions, current state or cancellation.
pub fn preview_availability_csv<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    mapping: &AvailabilityCsvMapping,
    decisions: &[AvailabilityCsvRowDecision],
    reserved_ids: &BTreeSet<uuid::Uuid>,
    cancellation: &CancellationToken,
) -> Result<AvailabilityImportPreview, CsvError> {
    let mut review = RowReview::new(document, mapping, decisions, reserved_ids, cancellation)?;
    let source = review.scan(input)?;
    review.finish(source, revision)
}

fn validated_review_value(review: &AvailabilityCsvReview) -> Result<Value, CsvError> {
    if review.format != REVIEW_FORMAT
        || review.schema_version != 1
        || review.parser_version != CSV_PARSER_VERSION
        || review.limits_version != CSV_LIMITS_VERSION
    {
        return Err(invalid(CsvErrorCode::UnsupportedVersion));
    }
    validate_policy(&review.mapping, &review.decisions)?;
    if review.scenario_id.as_uuid().get_version_num() != 7
        || [
            &review.scenario_blake3,
            &review.source.blake3,
            &review.rows_blake3,
            &review.changes_blake3,
            &review.excluded_blake3,
            &review.validation_blake3,
        ]
        .into_iter()
        .any(|digest| !valid_digest(digest))
        || usize::try_from(review.source.raw_bytes)
            .map_or(true, |bytes| bytes > MAX_CSV_SOURCE_BYTES)
        || review.source.logical_records
            > MAX_CSV_DATA_RECORDS + u32::from(review.mapping.has_header)
        || review.source.logical_records < u32::from(review.mapping.has_header)
        || review
            .decisions
            .last()
            .is_some_and(|row| row.record > review.source.logical_records)
    {
        return Err(invalid(CsvErrorCode::InvalidReview));
    }
    bounded_json_size(review, MAX_CSV_REVIEW_BYTES)
        .map_err(|_| invalid(CsvErrorCode::ReviewLimit))?;
    let value = serde_json::to_value(review).map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    validate_value_bounds(&value, "/availabilityCsvReview")
        .map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    Ok(value)
}
fn approval_digest(review: &AvailabilityCsvReview) -> Result<String, CsvError> {
    #[derive(Serialize)]
    struct Approval<'a> {
        domain: &'static str,
        version: u32,
        review: &'a Value,
    }
    hash(&Approval {
        domain: "eutheto/workforce-availability-csv-approval",
        version: 1,
        review: &validated_review_value(review)?,
    })
}

/// Decodes only an exact, nonsecret, bounded review representation.
///
/// # Errors
/// Rejects malformed, unsupported, unsafe, oversized or alternate representations.
pub fn decode_availability_csv_review(bytes: &[u8]) -> Result<AvailabilityCsvReview, CsvError> {
    if bytes.len() > MAX_CSV_REVIEW_BYTES {
        return Err(invalid(CsvErrorCode::ReviewLimit));
    }
    validate_nonsecret_portable_json_bytes(bytes, &json_limits())
        .map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    let review: AvailabilityCsvReview =
        Deserialize::deserialize(&raw).map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    if validated_review_value(&review)? != raw {
        return Err(invalid(CsvErrorCode::InvalidReview));
    }
    Ok(review)
}

/// Rebuilds the entire approved interpretation; callers own revision-checked commit.
///
/// # Errors
/// Rejects changed source, revision, snapshot, wrapper census, policy or approval.
pub fn rebuild_availability_csv_review<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    review: &AvailabilityCsvReview,
    approved_digest: &str,
    reserved_ids: &BTreeSet<uuid::Uuid>,
    cancellation: &CancellationToken,
) -> Result<AvailabilityImportPreview, CsvError> {
    check(cancellation)?;
    if approval_digest(review)? != approved_digest
        || review.scenario_id != document.scenario_id
        || review.revision != revision
        || review.scenario_blake3 != hash(&(document, reserved_ids))?
    {
        return Err(invalid(CsvErrorCode::StaleReview));
    }
    let preview = preview_availability_csv(
        document,
        revision,
        input,
        &review.mapping,
        &review.decisions,
        reserved_ids,
        cancellation,
    )?;
    if preview.review.as_ref() != Some(review)
        || preview.approval_digest.as_deref() != Some(approved_digest)
    {
        return Err(invalid(CsvErrorCode::StaleReview));
    }
    check(cancellation)?;
    Ok(preview)
}
