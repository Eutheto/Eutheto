use super::{PeopleImportRow, PeopleRowStatus};
use crate::people_csv::{
    mapping::map_row,
    types::{
        CsvError, CsvErrorCode, CsvRecord, FieldEdit, IdentityDecision, MAX_CSV_DATA_RECORDS,
        MAX_CSV_PREVIEW_BYTES, MAX_CSV_REJECTED_REPORT_BYTES, MAX_CSV_REJECTED_ROWS,
        PeopleCsvMapping, PersonField, PersonPatch, RejectedRow, RowDecision, RowRejectionCode,
    },
};
use crate::{
    commands,
    validation::{WorkforceSchemas, validate_document_with_schemas, validate_value_bounds},
};
use eutheto_domain_api::{
    ContractJsonLimits, DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, MAX_DOMAIN_BATCH_COMMANDS,
    ValidatedContractSchema, bounded_json_size,
};
use eutheto_types::{
    DomainCommandEnvelope, EntityId, PersonId, ScenarioDocument, collect_document_owned_uuids,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) struct RowReview<'a> {
    original: &'a ScenarioDocument,
    scratch: ScenarioDocument,
    mapping: &'a PeopleCsvMapping,
    decisions: BTreeMap<u32, IdentityDecision>,
    schemas: WorkforceSchemas,
    fields: BTreeMap<String, ValidatedContractSchema>,
    defaults: Value,
    external_ids: BTreeMap<&'a str, PersonId>,
    targets: BTreeMap<PersonId, usize>,
    external_targets: BTreeMap<String, usize>,
    rows: Vec<PeopleImportRow>,
    rejected: Vec<RejectedRow>,
    commands: BTreeMap<usize, (DomainCommandEnvelope, usize)>,
    command_bytes: usize,
}

impl<'a> RowReview<'a> {
    pub(super) fn new(
        original: &'a ScenarioDocument,
        mapping: &'a PeopleCsvMapping,
        decisions: &[RowDecision],
        schemas: WorkforceSchemas,
        fields: BTreeMap<String, ValidatedContractSchema>,
    ) -> Result<Self, CsvError> {
        let owned = collect_document_owned_uuids(original);
        for row in decisions {
            let invalid = match row.decision {
                IdentityDecision::Add { person_id } => owned.contains(&person_id.as_uuid()),
                IdentityDecision::Update { person_id } => {
                    original
                        .domain
                        .entities
                        .get(&EntityId::from_uuid(person_id.as_uuid()))
                        .and_then(|value| value.get("kind"))
                        .and_then(Value::as_str)
                        != Some("person")
                }
                IdentityDecision::Skip => false,
            };
            if invalid {
                return Err(CsvError::at(CsvErrorCode::InvalidDecision, row.record));
            }
        }
        let defaults = serde_json::to_value(&mapping.new_person_defaults)
            .map_err(|_| CsvError::source(CsvErrorCode::InvalidMapping))?;
        let external_ids = original
            .domain
            .entities
            .iter()
            .filter_map(|(id, value)| {
                if value.get("kind").and_then(Value::as_str) != Some("person") {
                    return None;
                }
                value
                    .get("externalId")
                    .and_then(Value::as_str)
                    .map(|external| (external, PersonId::from_uuid(id.as_uuid())))
            })
            .collect();
        Ok(Self {
            original,
            scratch: original.clone(),
            mapping,
            decisions: decisions
                .iter()
                .map(|row| (row.record, row.decision))
                .collect(),
            schemas,
            fields,
            defaults,
            external_ids,
            targets: BTreeMap::new(),
            external_targets: BTreeMap::new(),
            rows: Vec::new(),
            rejected: Vec::new(),
            commands: BTreeMap::new(),
            command_bytes: 2,
        })
    }

    pub(super) fn visit(&mut self, record: CsvRecord<'_>) -> Result<(), CsvError> {
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
        if self.decisions.get(&record.number) == Some(&IdentityDecision::Skip) {
            self.push_row(record.number, PeopleRowStatus::Skipped, None);
            return Ok(());
        }
        let number = record.number;
        let patch = match map_row(record, self.mapping) {
            Ok(patch) => patch,
            Err(code) => return self.reject(number, code),
        };
        if let Err(code) = self.validate_patch(&patch) {
            return self.reject(number, code);
        }
        let Some((person_id, add)) = self.choose_target(number, &patch)? else {
            let external = patch_external(&patch);
            let index = self.push_row(number, PeopleRowStatus::Unresolved, None);
            self.reserve_external(external, index)?;
            return Ok(());
        };
        let candidate = self.materialize(person_id, add, patch)?;
        if add && !candidate.get("name").is_some_and(Value::is_string) {
            return self.reject(number, RowRejectionCode::MissingName);
        }
        let (candidate, valid) = self.validate_candidate(person_id, candidate)?;
        if !valid {
            return self.reject(number, RowRejectionCode::InvalidPerson);
        }
        self.accept(number, person_id, add, candidate)
    }

    fn validate_patch(&self, patch: &PersonPatch) -> Result<(), RowRejectionCode> {
        for (field, edit) in &patch.fields {
            if let FieldEdit::Set(value) = edit {
                validate_value_bounds(value, "/peopleCsv/field")
                    .map_err(|_| RowRejectionCode::InvalidCell)?;
                self.fields
                    .get(field.key())
                    .ok_or(RowRejectionCode::InvalidCell)?
                    .validate(value, ContractJsonLimits::DEFAULT)
                    .map_err(|_| RowRejectionCode::InvalidCell)?;
                self.validate_references(*field, value)?;
            }
        }
        Ok(())
    }

    fn validate_references(
        &self,
        field: PersonField,
        value: &Value,
    ) -> Result<(), RowRejectionCode> {
        match field {
            PersonField::QualificationGrants => {
                for grant in value.as_array().ok_or(RowRejectionCode::InvalidCell)? {
                    self.reference(
                        grant
                            .get("qualificationId")
                            .ok_or(RowRejectionCode::InvalidCell)?,
                        "qualification",
                    )?;
                }
            }
            PersonField::EligibleAssignmentTypeIds | PersonField::TeamIds => {
                let kind = if field == PersonField::TeamIds {
                    "team"
                } else {
                    "assignmentType"
                };
                for id in value.as_array().ok_or(RowRejectionCode::InvalidCell)? {
                    self.reference(id, kind)?;
                }
            }
            PersonField::HomeLocationId => self.reference(value, "location")?,
            PersonField::WorkloadTarget => {
                self.reference(
                    value.get("bucketId").ok_or(RowRejectionCode::InvalidCell)?,
                    "workloadBucket",
                )?;
                self.reference(
                    value
                        .get("calendarId")
                        .ok_or(RowRejectionCode::InvalidCell)?,
                    "calendar",
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    fn reference(&self, value: &Value, kind: &str) -> Result<(), RowRejectionCode> {
        let id: EntityId = value
            .as_str()
            .ok_or(RowRejectionCode::InvalidReference)?
            .parse()
            .map_err(|_| RowRejectionCode::InvalidReference)?;
        if self
            .original
            .domain
            .entities
            .get(&id)
            .and_then(|entity| entity.get("kind"))
            .and_then(Value::as_str)
            == Some(kind)
        {
            Ok(())
        } else {
            Err(RowRejectionCode::InvalidReference)
        }
    }

    fn choose_target(
        &self,
        record: u32,
        patch: &PersonPatch,
    ) -> Result<Option<(PersonId, bool)>, CsvError> {
        let exact = patch_external(patch)
            .and_then(|external| self.external_ids.get(external))
            .copied();
        match self.decisions.get(&record).copied() {
            Some(IdentityDecision::Add { person_id }) if exact.is_none() => {
                Ok(Some((person_id, true)))
            }
            Some(IdentityDecision::Update { person_id })
                if exact.is_none_or(|id| id == person_id) =>
            {
                Ok(Some((person_id, false)))
            }
            Some(_) => Err(CsvError::at(CsvErrorCode::InvalidDecision, record)),
            None => Ok(exact.map(|id| (id, false))),
        }
    }

    fn materialize(
        &self,
        person_id: PersonId,
        add: bool,
        patch: PersonPatch,
    ) -> Result<Value, CsvError> {
        let mut candidate = if add {
            self.defaults.clone()
        } else {
            self.original
                .domain
                .entities
                .get(&EntityId::from_uuid(person_id.as_uuid()))
                .ok_or_else(|| CsvError::source(CsvErrorCode::InvalidDecision))?
                .clone()
        };
        let object = candidate
            .as_object_mut()
            .ok_or_else(|| CsvError::source(CsvErrorCode::InvalidCurrentDocument))?;
        if add {
            object.insert("kind".to_owned(), Value::String("person".to_owned()));
            object.insert("id".to_owned(), Value::String(person_id.to_string()));
        }
        for (field, edit) in patch.fields {
            match edit {
                FieldEdit::Set(value) => {
                    object.insert(field.key().to_owned(), value);
                }
                FieldEdit::Remove => {
                    object.remove(field.key());
                }
            }
        }
        Ok(candidate)
    }

    fn validate_candidate(
        &mut self,
        person_id: PersonId,
        candidate: Value,
    ) -> Result<(Value, bool), CsvError> {
        // Validate each row against the same original baseline. A malformed row
        // cannot reserve an identity or make valid peer rows fail. Aggregate and
        // ordered-prefix validity is checked by the real final command batch.
        let key = EntityId::from_uuid(person_id.as_uuid());
        let previous = self.scratch.domain.entities.insert(key, candidate);
        let valid = validate_document_with_schemas(&self.scratch, &self.schemas, None).is_ok();
        let candidate = match previous {
            Some(previous) => self.scratch.domain.entities.insert(key, previous),
            None => self.scratch.domain.entities.remove(&key),
        }
        .ok_or_else(|| CsvError::source(CsvErrorCode::InvalidBatch))?;
        Ok((candidate, valid))
    }

    fn accept(
        &mut self,
        record: u32,
        person_id: PersonId,
        add: bool,
        candidate: Value,
    ) -> Result<(), CsvError> {
        let unchanged = self
            .original
            .domain
            .entities
            .get(&EntityId::from_uuid(person_id.as_uuid()))
            == Some(&candidate);
        let status = if unchanged {
            PeopleRowStatus::Unchanged
        } else if add {
            PeopleRowStatus::Added
        } else {
            PeopleRowStatus::Updated
        };
        let index = self.push_row(record, status, Some(person_id));
        if let Some(&previous) = self.targets.get(&person_id) {
            self.conflict(previous)?;
            self.conflict(index)?;
        } else {
            self.targets.insert(person_id, index);
        }
        self.reserve_external(candidate.get("externalId").and_then(Value::as_str), index)?;
        if unchanged || self.rows[index].status == PeopleRowStatus::Conflict {
            return Ok(());
        }
        if self.commands.len() >= MAX_DOMAIN_BATCH_COMMANDS {
            return Err(CsvError::at(CsvErrorCode::MutationLimit, record));
        }
        let envelope = DomainCommandEnvelope {
            command_type: if add {
                commands::ADD_ENTITY
            } else {
                commands::UPDATE_ENTITY
            }
            .to_owned(),
            payload: Value::Object([("entity".to_owned(), candidate)].into_iter().collect()),
        };
        let size = bounded_json_size(&envelope, MAX_CSV_PREVIEW_BYTES)
            .map_err(|_| CsvError::at(CsvErrorCode::PreviewLimit, record))?
            .checked_add(1)
            .ok_or_else(|| CsvError::at(CsvErrorCode::PreviewLimit, record))?;
        self.command_bytes = self
            .command_bytes
            .checked_add(size)
            .filter(|bytes| *bytes <= MAX_CSV_PREVIEW_BYTES)
            .ok_or_else(|| CsvError::at(CsvErrorCode::PreviewLimit, record))?;
        self.commands.insert(index, (envelope, size));
        Ok(())
    }

    fn push_row(
        &mut self,
        record: u32,
        status: PeopleRowStatus,
        person_id: Option<PersonId>,
    ) -> usize {
        let index = self.rows.len();
        self.rows.push(PeopleImportRow {
            record,
            status,
            person_id,
            rejection: None,
        });
        index
    }

    fn reserve_external(&mut self, external: Option<&str>, index: usize) -> Result<(), CsvError> {
        if let Some(external) = external {
            if let Some(&previous) = self.external_targets.get(external) {
                self.conflict(previous)?;
                self.conflict(index)?;
            } else {
                self.external_targets.insert(external.to_owned(), index);
            }
        }
        Ok(())
    }

    fn conflict(&mut self, index: usize) -> Result<(), CsvError> {
        self.rows
            .get_mut(index)
            .ok_or_else(|| CsvError::source(CsvErrorCode::InvalidBatch))?
            .status = PeopleRowStatus::Conflict;
        if let Some((_, size)) = self.commands.remove(&index) {
            self.command_bytes = self
                .command_bytes
                .checked_sub(size)
                .ok_or_else(|| CsvError::source(CsvErrorCode::InvalidBatch))?;
        }
        Ok(())
    }

    fn reject(&mut self, record: u32, code: RowRejectionCode) -> Result<(), CsvError> {
        if self.rejected.len() >= MAX_CSV_REJECTED_ROWS {
            return Err(CsvError::at(CsvErrorCode::RejectedReportLimit, record));
        }
        self.rejected.push(RejectedRow { record, code });
        bounded_json_size(&self.rejected, MAX_CSV_REJECTED_REPORT_BYTES)
            .map_err(|_| CsvError::at(CsvErrorCode::RejectedReportLimit, record))?;
        self.rows.push(PeopleImportRow {
            record,
            status: PeopleRowStatus::Rejected,
            person_id: None,
            rejection: Some(code),
        });
        Ok(())
    }

    pub(super) fn finish(
        self,
    ) -> (
        Vec<PeopleImportRow>,
        Vec<RejectedRow>,
        Option<DomainBatchCommand>,
        bool,
    ) {
        let blocked = self.rows.iter().any(|row| {
            matches!(
                row.status,
                PeopleRowStatus::Unresolved | PeopleRowStatus::Conflict
            )
        });
        let batch = if blocked || self.commands.is_empty() {
            None
        } else {
            Some(DomainBatchCommand {
                schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
                pack_id: self.original.domain_pack.id.clone(),
                scenario_schema_version: self.original.domain_pack.schema_version,
                label: Some("Import people from CSV".to_owned()),
                // Index ordering is original logical-record order, not a semantic re-sort.
                commands: self
                    .commands
                    .into_values()
                    .map(|(command, _)| command)
                    .collect(),
            })
        };
        (self.rows, self.rejected, batch, blocked)
    }
}

fn patch_external(patch: &PersonPatch) -> Option<&str> {
    match patch.fields.get(&PersonField::ExternalId) {
        Some(FieldEdit::Set(Value::String(value))) => Some(value),
        _ => None,
    }
}
