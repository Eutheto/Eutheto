use super::{
    MAX_SHIFT_CSV_REPORT_BYTES, ShiftCsvDecision, ShiftCsvExcludedRow, ShiftCsvField,
    ShiftCsvIdentity, ShiftCsvMapping, ShiftCsvReview, ShiftCsvRowDecision, ShiftCsvRowIssue,
    ShiftCsvRowStatus, ShiftImportPreview, ShiftImportRow,
};
use crate::{
    commands,
    ids::ShiftId,
    model::{Coverage, QualificationMinimum, ShiftInstance, WorkforceDomainV1, WorkforceEntity},
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

const REVIEW_FORMAT: &str = "eutheto/workforce-shift-csv-review";

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

fn key(field: ShiftCsvField) -> &'static str {
    match field {
        ShiftCsvField::AssignmentTypeId => "assignmentTypeId",
        ShiftCsvField::LocationId => "locationId",
        ShiftCsvField::StartsAt => "startsAt",
        ShiftCsvField::EndsAt => "endsAt",
        ShiftCsvField::Coverage => "coverage",
        ShiftCsvField::Tags => "tags",
        ShiftCsvField::ReportingAttribution => "reportingAttribution",
    }
}

fn validate_policy(
    mapping: &ShiftCsvMapping,
    decisions: &[ShiftCsvRowDecision],
) -> Result<(), CsvError> {
    if mapping.columns.len() > 7 || mapping.reference_mappings.len() > MAX_REFERENCE_ITEMS {
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
                    ShiftCsvField::LocationId | ShiftCsvField::Tags
                ))
        {
            return Err(invalid(CsvErrorCode::InvalidMapping));
        }
    }
    for (token, id) in &mapping.reference_mappings {
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
                    ShiftCsvDecision::Add { shift_id } | ShiftCsvDecision::Update { shift_id } => {
                        shift_id.as_entity_id().as_uuid().get_version_num() != 7
                    }
                    ShiftCsvDecision::Skip {} => false,
                }
        })
    {
        return Err(invalid(CsvErrorCode::InvalidDecision));
    }
    Ok(())
}

fn stored(domain: &WorkforceDomainV1, id: ShiftId) -> Option<&ShiftInstance> {
    match domain.entities.get(&id.as_entity_id()) {
        Some(WorkforceEntity::ShiftInstance(value)) => Some(value),
        _ => None,
    }
}
fn identity(record: &CsvRecord<'_>, mapping: &ShiftCsvMapping) -> ShiftCsvIdentity {
    let token = mapping
        .identity_column
        .and_then(|column| record.cells.get(usize::from(column)))
        .copied()
        .unwrap_or("");
    if token.is_empty() {
        return ShiftCsvIdentity::Absent {};
    }
    if token.len() > MAX_CSV_INSPECTED_CELL_BYTES || token.chars().any(char::is_control) {
        return ShiftCsvIdentity::Invalid {};
    }
    match token.parse() {
        Ok(shift_id) => ShiftCsvIdentity::Supplied { shift_id },
        Err(_) => ShiftCsvIdentity::Invalid {},
    }
}

fn reference(
    value: &mut Value,
    kind: &str,
    mapping: &ShiftCsvMapping,
    document: &ScenarioDocument,
) -> Result<(), ShiftCsvRowIssue> {
    let text = value.as_str().ok_or(ShiftCsvRowIssue::InvalidReference)?;
    if text.len() > MAX_CSV_INSPECTED_CELL_BYTES || text.chars().any(char::is_control) {
        return Err(ShiftCsvRowIssue::InvalidReference);
    }
    let (id, alias) = match text.parse::<EntityId>() {
        Ok(id) => (id, false),
        Err(TypedUuidError::InvalidUuid(_)) => (
            mapping
                .reference_mappings
                .get(text)
                .copied()
                .ok_or(ShiftCsvRowIssue::InvalidReference)?,
            true,
        ),
        Err(_) => return Err(ShiftCsvRowIssue::InvalidReference),
    };
    if document
        .domain
        .entities
        .get(&id)
        .and_then(|raw| raw.get("kind"))
        .and_then(Value::as_str)
        != Some(kind)
    {
        return Err(ShiftCsvRowIssue::InvalidReference);
    }
    if alias {
        *value = Value::String(id.to_string());
    }
    Ok(())
}

// Edits retain submitted raw structured JSON. Candidate-schema validation, not typed
// reserialization, rejects unknown keys, explicit optional nulls and alternate shapes.
type Edits = BTreeMap<String, Option<Value>>;
fn mapped_edits(
    record: &CsvRecord<'_>,
    mapping: &ShiftCsvMapping,
    document: &ScenarioDocument,
    add: bool,
) -> Result<Edits, ShiftCsvRowIssue> {
    if record.cells.len() != usize::from(mapping.expected_columns) {
        return Err(ShiftCsvRowIssue::ColumnCount);
    }
    if add {
        for field in [
            ShiftCsvField::AssignmentTypeId,
            ShiftCsvField::StartsAt,
            ShiftCsvField::EndsAt,
            ShiftCsvField::Coverage,
            ShiftCsvField::ReportingAttribution,
        ] {
            if !mapping.columns.iter().any(|column| {
                column.field == field && !record.cells[usize::from(column.index)].is_empty()
            }) {
                return Err(ShiftCsvRowIssue::MissingRequiredField);
            }
        }
    }
    let mut edits = Edits::new();
    for column in &mapping.columns {
        let cell = record.cells[usize::from(column.index)];
        if cell.is_empty() {
            if column.blank == BlankPolicy::Clear {
                edits.insert(
                    key(column.field).to_owned(),
                    if column.field == ShiftCsvField::LocationId {
                        None
                    } else {
                        Some(json!([]))
                    },
                );
            }
            continue;
        }
        let mut value = match column.field {
            ShiftCsvField::AssignmentTypeId | ShiftCsvField::LocationId => {
                Value::String(cell.to_owned())
            }
            ShiftCsvField::StartsAt | ShiftCsvField::EndsAt => {
                let local = crate::model::parse_local_intent(cell)
                    .ok_or(ShiftCsvRowIssue::InvalidLocalTime)?;
                let resolved = resolve_local_time(
                    local,
                    &document.settings.time_zone,
                    document.settings.gap_policy,
                    document.settings.overlap_policy,
                )
                .map_err(|_| ShiftCsvRowIssue::UnresolvableLocalTime)?;
                serde_json::to_value(resolved).map_err(|_| ShiftCsvRowIssue::InvalidLocalTime)?
            }
            ShiftCsvField::ReportingAttribution => Value::String(cell.to_owned()),
            ShiftCsvField::Coverage | ShiftCsvField::Tags => {
                validate_nonsecret_portable_json_bytes(cell.as_bytes(), &json_limits())
                    .map_err(|_| ShiftCsvRowIssue::InvalidCell)?;
                serde_json::from_str(cell).map_err(|_| ShiftCsvRowIssue::InvalidCell)?
            }
        };
        match column.field {
            ShiftCsvField::AssignmentTypeId => {
                reference(&mut value, "assignmentType", mapping, document)?;
            }
            ShiftCsvField::LocationId => reference(&mut value, "location", mapping, document)?,
            ShiftCsvField::Coverage => {
                let minima = value
                    .get_mut("qualificationMinimums")
                    .and_then(Value::as_array_mut)
                    .ok_or(ShiftCsvRowIssue::InvalidCell)?;
                for minimum in minima {
                    let qualifications = minimum
                        .get_mut("qualifications")
                        .ok_or(ShiftCsvRowIssue::InvalidCell)?;
                    for name in ["allQualificationIds", "anyQualificationIds"] {
                        let ids = qualifications
                            .get_mut(name)
                            .and_then(Value::as_array_mut)
                            .ok_or(ShiftCsvRowIssue::InvalidCell)?;
                        for id in ids {
                            reference(id, "qualification", mapping, document)?;
                        }
                    }
                }
            }
            _ => {}
        }
        validate_value_bounds(&value, "/shiftCsv/field")
            .map_err(|_| ShiftCsvRowIssue::InvalidCell)?;
        edits.insert(key(column.field).to_owned(), Some(value));
    }
    Ok(edits)
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
fn minima_equal(left: &[QualificationMinimum], right: &[QualificationMinimum]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.minimum == right.minimum
                && left
                    .qualifications
                    .all_qualification_ids
                    .iter()
                    .collect::<BTreeSet<_>>()
                    == right
                        .qualifications
                        .all_qualification_ids
                        .iter()
                        .collect::<BTreeSet<_>>()
                && left
                    .qualifications
                    .any_qualification_ids
                    .iter()
                    .collect::<BTreeSet<_>>()
                    == right
                        .qualifications
                        .any_qualification_ids
                        .iter()
                        .collect::<BTreeSet<_>>()
        })
}
fn coverage_equal(left: &Coverage, right: &Coverage) -> bool {
    if left == right {
        return true;
    }
    match (left, right) {
        (
            Coverage::Exact {
                count: a,
                qualification_minimums: am,
            },
            Coverage::Exact {
                count: b,
                qualification_minimums: bm,
            },
        ) => a == b && minima_equal(am, bm),
        (
            Coverage::AtLeast {
                minimum: a,
                preferred_count: ap,
                maximum_count: ax,
                qualification_minimums: am,
            },
            Coverage::AtLeast {
                minimum: b,
                preferred_count: bp,
                maximum_count: bx,
                qualification_minimums: bm,
            },
        ) => a == b && ap == bp && ax == bx && minima_equal(am, bm),
        _ => false,
    }
}
fn field_equal(field: ShiftCsvField, before: &ShiftInstance, after: &ShiftInstance) -> bool {
    match field {
        ShiftCsvField::AssignmentTypeId => before.assignment_type_id == after.assignment_type_id,
        ShiftCsvField::LocationId => before.location_id == after.location_id,
        ShiftCsvField::StartsAt => before.starts_at == after.starts_at,
        ShiftCsvField::EndsAt => before.ends_at == after.ends_at,
        ShiftCsvField::Coverage => coverage_equal(&before.coverage, &after.coverage),
        ShiftCsvField::Tags => before.tags == after.tags,
        ShiftCsvField::ReportingAttribution => {
            before.reporting_attribution == after.reporting_attribution
        }
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
    mapping: &'a ShiftCsvMapping,
    decisions: &'a [ShiftCsvRowDecision],
    schemas: WorkforceSchemas,
    domain: WorkforceDomainV1,
    owned: BTreeSet<uuid::Uuid>,
    reserved_ids: &'a BTreeSet<uuid::Uuid>,
    cancellation: &'a CancellationToken,
    control: OperationControl,
    budget: Budget,
    rows: Vec<ShiftImportRow>,
    targets: BTreeMap<ShiftId, Vec<usize>>,
}

impl<'a> RowReview<'a> {
    fn new(
        document: &'a ScenarioDocument,
        mapping: &'a ShiftCsvMapping,
        decisions: &'a [ShiftCsvRowDecision],
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
        let mut row = ShiftImportRow {
            record: record.number,
            status: ShiftCsvRowStatus::Unresolved,
            identity,
            shift_id: None,
            before: None,
            after: None,
            issue: None,
        };
        if matches!(decision, Some(ShiftCsvDecision::Skip {})) {
            row.status = ShiftCsvRowStatus::Skipped;
            if let ShiftCsvIdentity::Supplied { shift_id } = identity {
                row.shift_id = Some(shift_id);
            }
        } else {
            let (target, add) = choose_target(&row, decision)?;
            row.shift_id = target;
            if !self.reserve_target(&mut row) {
                self.review_row(&record, &mut row, decision, add)?;
            }
        }
        self.rows.push(row);
        Ok(())
    }

    fn reserve_target(&mut self, row: &mut ShiftImportRow) -> bool {
        let Some(target) = row.shift_id else {
            return false;
        };
        let indices = self.targets.entry(target).or_default();
        let duplicate = !indices.is_empty();
        indices.push(self.rows.len());
        if duplicate {
            for &index in &indices[..indices.len() - 1] {
                block_row(&mut self.rows[index], ShiftCsvRowIssue::DuplicateTarget);
                self.rows[index].before = None;
                self.rows[index].after = None;
            }
            block_row(row, ShiftCsvRowIssue::DuplicateTarget);
        }
        duplicate
    }

    fn identity_issue(&self, row: &ShiftImportRow, add: bool) -> Option<ShiftCsvRowIssue> {
        match row.identity {
            ShiftCsvIdentity::Invalid {} => Some(ShiftCsvRowIssue::InvalidIdentity),
            _ => row
                .shift_id
                .filter(|id| {
                    let uuid = id.as_entity_id().as_uuid();
                    (self.owned.contains(&uuid) || self.reserved_ids.contains(&uuid))
                        && (add
                            || stored(&self.domain, *id).is_none()
                            || self.reserved_ids.contains(&uuid))
                })
                .map(|_| ShiftCsvRowIssue::ReservedIdentity),
        }
    }

    fn review_row(
        &mut self,
        record: &CsvRecord<'_>,
        row: &mut ShiftImportRow,
        decision: Option<ShiftCsvDecision>,
        add: bool,
    ) -> Result<(), CsvError> {
        if let Some(issue) = self.identity_issue(row, add) {
            block_row(row, issue);
            return Ok(());
        }
        let target = row.shift_id;
        if target.is_none() || (!add && target.is_some_and(|id| stored(&self.domain, id).is_none()))
        {
            if target.is_some() && decision.is_some() {
                return Err(CsvError::at(CsvErrorCode::InvalidDecision, record.number));
            }
            row.issue = Some(ShiftCsvRowIssue::UnresolvedIdentity);
            // Unlike Skip, an unresolved row still checks its mapped cells.
            if let Err(issue) = mapped_edits(record, self.mapping, self.document, false) {
                block_row(row, issue);
            }
            return Ok(());
        }
        let target = target.ok_or_else(|| invalid(CsvErrorCode::InvalidDecision))?;
        match mapped_edits(record, self.mapping, self.document, add) {
            Err(issue) => block_row(row, issue),
            Ok(edits) if !add && edits.is_empty() => {
                let original = self
                    .document
                    .domain
                    .entities
                    .get(&target.as_entity_id())
                    .ok_or_else(|| invalid(CsvErrorCode::InvalidCurrentDocument))?;
                self.budget.charge(self.budget.admit(original)?)?;
                row.before = Some(original.clone());
                row.status = ShiftCsvRowStatus::Unchanged;
            }
            Ok(edits) => self.review_candidate(row, target, add, edits)?,
        }
        Ok(())
    }

    fn review_candidate(
        &mut self,
        row: &mut ShiftImportRow,
        target: ShiftId,
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
            edits.insert("kind".to_owned(), Some(json!("shiftInstance")));
            edits.insert("id".to_owned(), Some(json!(target)));
            edits.insert("origin".to_owned(), Some(json!({"kind":"manual"})));
            edits
                .entry("tags".to_owned())
                .or_insert_with(|| Some(json!([])));
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
        row: &mut ShiftImportRow,
        target: ShiftId,
        candidate: &mut Value,
    ) -> Result<Option<ShiftInstance>, CsvError> {
        let candidate_valid = validate_value_bounds(candidate, "/shiftCsv/candidate").is_ok()
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
        let interval_invalid = matches!(&typed, Some(WorkforceEntity::ShiftInstance(shift))
            if shift.starts_at.instant >= shift.ends_at.instant);
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
                path: "/shiftCsv/candidate".to_owned(),
                message: "invalid shift".to_owned(),
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
                        ShiftCsvRowIssue::NonpositiveInterval
                    } else {
                        ShiftCsvRowIssue::InvalidShift
                    },
                );
                Ok(None)
            }
            Ok(()) => match typed {
                Some(WorkforceEntity::ShiftInstance(after)) => Ok(Some(after)),
                _ => Err(invalid(CsvErrorCode::InvalidBatch)),
            },
        }
    }

    fn accept_candidate(
        &mut self,
        row: &mut ShiftImportRow,
        target: ShiftId,
        add: bool,
        mut candidate: Value,
        after: &ShiftInstance,
    ) -> Result<(), CsvError> {
        let original = if add {
            None
        } else {
            self.document.domain.entities.get(&target.as_entity_id())
        };
        let original_object = original.and_then(Value::as_object);
        let mut edits = Edits::new();
        if let Some(before) = stored(&self.domain, target).filter(|_| !add) {
            for column in &self.mapping.columns {
                if !field_equal(column.field, before, after) {
                    edits.insert(
                        key(column.field).to_owned(),
                        candidate
                            .as_object_mut()
                            .and_then(|raw| raw.remove(key(column.field))),
                    );
                }
            }
        }
        let changed = add || !edits.is_empty();
        // Equivalent fields use the original spelling, order and omission evidence.
        let projected = Overlay {
            original: original_object,
            edits: &edits,
        };
        let before_bytes = original
            .map(|raw| self.budget.admit(raw))
            .transpose()?
            .unwrap_or(0);
        let after_bytes = if add {
            self.budget.admit(&candidate)?
        } else if changed {
            self.budget.admit(&projected)?
        } else {
            0
        };
        let added_candidate = if add {
            Some(candidate)
        } else {
            drop(candidate);
            None
        };
        // Reserve both row/command after trees and before projection BEFORE cloning.
        self.budget
            .charge(before_bytes + after_bytes * 2 + if changed { 128 } else { 0 })?;
        row.before = original.cloned();
        row.after = if add {
            added_candidate
        } else if changed {
            Some(materialize(original_object, edits))
        } else {
            None
        };
        row.status = if add {
            ShiftCsvRowStatus::Added
        } else if changed {
            ShiftCsvRowStatus::Updated
        } else {
            ShiftCsvRowStatus::Unchanged
        };
        Ok(())
    }

    fn excluded_rows(&mut self) -> Result<Vec<ShiftCsvExcludedRow>, CsvError> {
        let mut excluded_rows = Vec::new();
        let mut remaining = MAX_SHIFT_CSV_REPORT_BYTES - 2;
        for row in &self.rows {
            check(self.cancellation)?;
            if matches!(
                row.status,
                ShiftCsvRowStatus::Skipped
                    | ShiftCsvRowStatus::Blocked
                    | ShiftCsvRowStatus::Unresolved
            ) {
                let excluded = ShiftCsvExcludedRow {
                    record: row.record,
                    skipped: row.status == ShiftCsvRowStatus::Skipped,
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
    ) -> Result<ShiftImportPreview, CsvError> {
        let excluded_rows = self.excluded_rows()?;
        let mut blocked = self.rows.iter().any(|row| {
            matches!(
                row.status,
                ShiftCsvRowStatus::Blocked | ShiftCsvRowStatus::Unresolved
            )
        });
        let mut batch = proposed_batch(self.document, &self.rows, self.cancellation)?;
        let validation_issues =
            validate_proposal(self.document, &mut batch, &mut blocked, self.cancellation)?;
        check(self.cancellation)?;
        let review = if blocked {
            None
        } else {
            Some(ShiftCsvReview {
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
        let preview = ShiftImportPreview {
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
    row: &ShiftImportRow,
    decision: Option<ShiftCsvDecision>,
) -> Result<(Option<ShiftId>, bool), CsvError> {
    let supplied = match row.identity {
        ShiftCsvIdentity::Supplied { shift_id } => Some(shift_id),
        _ => None,
    };
    let (target, add) = match decision {
        Some(ShiftCsvDecision::Add { shift_id }) => (Some(shift_id), true),
        Some(ShiftCsvDecision::Update { shift_id }) => (Some(shift_id), false),
        Some(ShiftCsvDecision::Skip {}) => unreachable!(),
        None => (supplied, false),
    };
    if supplied.is_some() && target != supplied {
        return Err(CsvError::at(CsvErrorCode::InvalidDecision, row.record));
    }
    Ok((target, add))
}

fn block_row(row: &mut ShiftImportRow, issue: ShiftCsvRowIssue) {
    row.status = ShiftCsvRowStatus::Blocked;
    row.issue = Some(issue);
}

fn proposed_batch(
    document: &ScenarioDocument,
    rows: &[ShiftImportRow],
    cancellation: &CancellationToken,
) -> Result<Option<DomainBatchCommand>, CsvError> {
    let changes = rows
        .iter()
        .filter(|row| {
            matches!(
                row.status,
                ShiftCsvRowStatus::Added | ShiftCsvRowStatus::Updated
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
                ShiftCsvRowStatus::Blocked | ShiftCsvRowStatus::Unresolved
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
                command_type: if row.status == ShiftCsvRowStatus::Added {
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
        label: Some("Import stored shifts from CSV".to_owned()),
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
                    message: "The complete proposed shift changes violate scenario structure, references or capacity.".to_owned(),
                    field_path: Some("/shiftCsv/proposedState".to_owned()), resource: None,
                });
            }
            Err(DomainPackError::Cancelled) => return Err(invalid(CsvErrorCode::Cancelled)),
            Err(_) => return Err(invalid(CsvErrorCode::InvalidBatch)),
        }
    }
    Ok(issues)
}

/// Reviews only stored manual/detached shifts against native scenario time policy.
///
/// # Errors
/// Rejects unsafe/bounded source, policy, decisions, current state or cancellation.
pub fn preview_shift_csv<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    mapping: &ShiftCsvMapping,
    decisions: &[ShiftCsvRowDecision],
    reserved_ids: &BTreeSet<uuid::Uuid>,
    cancellation: &CancellationToken,
) -> Result<ShiftImportPreview, CsvError> {
    let mut review = RowReview::new(document, mapping, decisions, reserved_ids, cancellation)?;
    let source = review.scan(input)?;
    review.finish(source, revision)
}

fn validated_review_value(review: &ShiftCsvReview) -> Result<Value, CsvError> {
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
    validate_value_bounds(&value, "/shiftCsvReview")
        .map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    Ok(value)
}
fn approval_digest(review: &ShiftCsvReview) -> Result<String, CsvError> {
    #[derive(Serialize)]
    struct Approval<'a> {
        domain: &'static str,
        version: u32,
        review: &'a Value,
    }
    hash(&Approval {
        domain: "eutheto/workforce-shift-csv-approval",
        version: 1,
        review: &validated_review_value(review)?,
    })
}

/// Decodes only an exact, nonsecret, bounded review representation.
///
/// # Errors
/// Rejects malformed, unsupported, unsafe, oversized or alternate representations.
pub fn decode_shift_csv_review(bytes: &[u8]) -> Result<ShiftCsvReview, CsvError> {
    if bytes.len() > MAX_CSV_REVIEW_BYTES {
        return Err(invalid(CsvErrorCode::ReviewLimit));
    }
    validate_nonsecret_portable_json_bytes(bytes, &json_limits())
        .map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    let review: ShiftCsvReview =
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
pub fn rebuild_shift_csv_review<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    review: &ShiftCsvReview,
    approved_digest: &str,
    reserved_ids: &BTreeSet<uuid::Uuid>,
    cancellation: &CancellationToken,
) -> Result<ShiftImportPreview, CsvError> {
    check(cancellation)?;
    if approval_digest(review)? != approved_digest
        || review.scenario_id != document.scenario_id
        || review.revision != revision
        || review.scenario_blake3 != hash(&(document, reserved_ids))?
    {
        return Err(invalid(CsvErrorCode::StaleReview));
    }
    let preview = preview_shift_csv(
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
