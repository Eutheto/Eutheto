use super::{
    EligibilityCsvMapping, EligibilityCsvReview, EligibilityExcludedRow, EligibilityImportPreview,
    EligibilityImportRow, EligibilityRowIssue, EligibilityRowStatus,
    MAX_ELIGIBILITY_CSV_REPORT_BYTES,
};
use crate::{
    commands,
    ids::AssignmentTypeId,
    model::{Person, WorkforceDomainV1, WorkforceEntity},
    people_csv::{
        CSV_LIMITS_VERSION, CSV_PARSER_VERSION, CsvError, CsvErrorCode, CsvRecord, CsvSource,
        MAX_CSV_COLUMNS, MAX_CSV_DATA_RECORDS, MAX_CSV_DECISION_BYTES, MAX_CSV_DECISIONS,
        MAX_CSV_MAPPING_BYTES, MAX_CSV_PREVIEW_BYTES, MAX_CSV_REVIEW_BYTES, MAX_CSV_SOURCE_BYTES,
        PeopleImportDisposition, hash, resolve_identity, scan_csv, valid_digest,
        valid_identity_alias,
    },
    validation::{WorkforceSchemas, validate_document_with_schemas, validate_value_bounds},
};
use eutheto_domain_api::{
    ContractJsonLimits, DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainPackError,
    MAX_DOMAIN_BATCH_COMMANDS, bounded_json_size,
};
use eutheto_types::{
    CancellationToken, DomainCommandEnvelope, EntityId, OperationControl, PersonId,
    PortableJsonLimits, Revision, ScenarioDocument, ValidationIssue, ValidationSeverity,
    validate_nonsecret_portable_json_bytes,
};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, io::Read};

const REVIEW_FORMAT: &str = "eutheto/workforce-eligibility-csv-review";

pub(super) fn invalid(code: CsvErrorCode) -> CsvError {
    CsvError::source(code)
}

pub(super) fn check(cancellation: &CancellationToken) -> Result<(), CsvError> {
    if cancellation.is_cancelled() {
        Err(invalid(CsvErrorCode::Cancelled))
    } else {
        Ok(())
    }
}

fn validate_policy(mapping: &EligibilityCsvMapping, skipped: &[u32]) -> Result<(), CsvError> {
    bounded_json_size(mapping, MAX_CSV_MAPPING_BYTES)
        .map_err(|_| invalid(CsvErrorCode::MappingLimit))?;
    let columns = [
        mapping.person_column,
        mapping.assignment_type_column,
        mapping.eligible_column,
    ];
    if mapping.expected_columns == 0
        || usize::from(mapping.expected_columns) > MAX_CSV_COLUMNS
        || columns
            .iter()
            .any(|column| *column >= mapping.expected_columns)
        || columns[0] == columns[1]
        || columns[0] == columns[2]
        || columns[1] == columns[2]
        || mapping.person_mappings.len() > MAX_CSV_DECISIONS
        || mapping.assignment_type_mappings.len() > MAX_CSV_DECISIONS
        || mapping
            .person_mappings
            .values()
            .any(|id| id.as_uuid().get_version_num() != 7)
        || mapping
            .assignment_type_mappings
            .values()
            .any(|id| id.as_entity_id().as_uuid().get_version_num() != 7)
        || mapping
            .person_mappings
            .keys()
            .chain(mapping.assignment_type_mappings.keys())
            .any(|token| !valid_identity_alias(token))
    {
        return Err(invalid(CsvErrorCode::InvalidMapping));
    }
    if skipped.len() > MAX_CSV_DECISIONS {
        return Err(invalid(CsvErrorCode::DecisionLimit));
    }
    bounded_json_size(skipped, MAX_CSV_DECISION_BYTES)
        .map_err(|_| invalid(CsvErrorCode::DecisionLimit))?;
    if skipped.windows(2).any(|pair| pair[0] >= pair[1])
        || skipped.iter().any(|record| {
            *record <= u32::from(mapping.has_header)
                || *record > MAX_CSV_DATA_RECORDS + u32::from(mapping.has_header)
        })
    {
        return Err(invalid(CsvErrorCode::InvalidDecision));
    }
    Ok(())
}

pub(super) fn person(domain: &WorkforceDomainV1, id: PersonId) -> Option<&Person> {
    match domain.entities.get(&EntityId::from_uuid(id.as_uuid())) {
        Some(WorkforceEntity::Person(person)) => Some(person),
        _ => None,
    }
}

fn map_record(
    record: &CsvRecord<'_>,
    mapping: &EligibilityCsvMapping,
    domain: &WorkforceDomainV1,
) -> Result<Option<EligibilityImportRow>, CsvError> {
    if mapping.has_header && record.number == 1 {
        if record.cells.len() != usize::from(mapping.expected_columns) {
            return Err(CsvError::at(CsvErrorCode::HeaderMismatch, record.number));
        }
        return Ok(None);
    }
    if record.number - u32::from(mapping.has_header) > MAX_CSV_DATA_RECORDS {
        return Err(CsvError::at(CsvErrorCode::DataRecordLimit, record.number));
    }
    let (person_id, assignment_type_id, after, issue) =
        if record.cells.len() == usize::from(mapping.expected_columns) {
            let (person_id, person_issue) = resolve_identity(
                record.cells[usize::from(mapping.person_column)],
                &mapping.person_mappings,
                |id| person(domain, id).is_some(),
                EligibilityRowIssue::InvalidToken,
                EligibilityRowIssue::UnresolvedPerson,
            );
            let (type_id, type_issue) = resolve_identity(
                record.cells[usize::from(mapping.assignment_type_column)],
                &mapping.assignment_type_mappings,
                |id| {
                    matches!(
                        domain.entities.get(&id.as_entity_id()),
                        Some(WorkforceEntity::AssignmentType(_))
                    )
                },
                EligibilityRowIssue::InvalidToken,
                EligibilityRowIssue::UnresolvedAssignmentType,
            );
            let desired = record.cells[usize::from(mapping.eligible_column)]
                .parse::<bool>()
                .ok();
            let issue = person_issue.or(type_issue).or_else(|| {
                desired
                    .is_none()
                    .then_some(EligibilityRowIssue::InvalidMembership)
            });
            (person_id, type_id, desired, issue)
        } else {
            (None, None, None, Some(EligibilityRowIssue::ColumnCount))
        };
    Ok(Some(EligibilityImportRow {
        record: record.number,
        status: if issue.is_some() {
            EligibilityRowStatus::Blocked
        } else {
            EligibilityRowStatus::Unchanged
        },
        person_id,
        assignment_type_id,
        before: None,
        after,
        issue,
    }))
}

#[derive(Default)]
pub(super) struct PairFacts {
    pub(super) before: bool,
    pub(super) desired: Option<bool>,
    first_ordinal: u32,
    total: usize,
    included: usize,
}
pub(super) type Memberships = BTreeMap<PersonId, BTreeMap<AssignmentTypeId, PairFacts>>;

pub(super) fn include_pair(
    memberships: &mut Memberships,
    person_id: PersonId,
    type_id: AssignmentTypeId,
    desired: Option<bool>,
    ordinal: u32,
    omitted: bool,
) {
    let facts = memberships
        .entry(person_id)
        .or_default()
        .entry(type_id)
        .or_default();
    facts.total += 1;
    if !omitted {
        facts.included += 1;
        if facts.included == 1 {
            facts.desired = desired;
            facts.first_ordinal = ordinal;
        }
    }
}

pub(super) fn scan_before(
    memberships: &mut Memberships,
    domain: &WorkforceDomainV1,
    cancellation: &CancellationToken,
) -> Result<(), CsvError> {
    // Visit each affected person's stored list once, not once per source cell.
    for (id, choices) in memberships {
        check(cancellation)?;
        let original =
            person(domain, *id).ok_or_else(|| invalid(CsvErrorCode::InvalidCurrentDocument))?;
        for type_id in &original.eligible_assignment_type_ids {
            check(cancellation)?;
            if let Some(facts) = choices.get_mut(type_id) {
                facts.before = true;
            }
        }
    }
    Ok(())
}

pub(super) fn resolve_pair_facts(
    facts: &PairFacts,
    status: &mut EligibilityRowStatus,
    issue: &mut Option<EligibilityRowIssue>,
    before: &mut Option<bool>,
    after: &mut Option<bool>,
    preserve_blank: bool,
) {
    *before = Some(facts.before);
    if *status == EligibilityRowStatus::Skipped {
        if facts.total > 1 {
            *issue = Some(EligibilityRowIssue::DuplicatePair);
        } else if preserve_blank && issue.is_none() && after.is_none() {
            *after = *before;
        }
    } else if facts.included > 1 {
        *status = EligibilityRowStatus::Blocked;
        *issue = Some(EligibilityRowIssue::DuplicatePair);
    } else if issue.is_none() {
        if preserve_blank && after.is_none() {
            *after = *before;
        }
        *status = if before == after {
            EligibilityRowStatus::Unchanged
        } else {
            EligibilityRowStatus::Changed
        };
    }
}

fn scan_rows<R: Read + ?Sized>(
    input: &mut R,
    mapping: &EligibilityCsvMapping,
    skipped: &[u32],
    domain: &WorkforceDomainV1,
    cancellation: &CancellationToken,
) -> Result<(CsvSource, Vec<EligibilityImportRow>, Memberships), CsvError> {
    let mut rows = Vec::new();
    let mut memberships = Memberships::new();
    let source = scan_csv(input, mapping.dialect, cancellation, |record| {
        if let Some(mut row) = map_record(&record, mapping, domain)? {
            let omitted = skipped.binary_search(&row.record).is_ok();
            if omitted {
                row.status = EligibilityRowStatus::Skipped;
            }
            if let (Some(person_id), Some(type_id)) = (row.person_id, row.assignment_type_id) {
                include_pair(
                    &mut memberships,
                    person_id,
                    type_id,
                    row.after,
                    row.record,
                    omitted,
                );
            }
            rows.push(row);
        }
        Ok(())
    })?;
    if source.logical_records < u32::from(mapping.has_header) {
        return Err(invalid(CsvErrorCode::HeaderMismatch));
    }
    if skipped
        .last()
        .is_some_and(|record| *record > source.logical_records)
    {
        return Err(invalid(CsvErrorCode::InvalidDecision));
    }
    scan_before(&mut memberships, domain, cancellation)?;
    for row in &mut rows {
        check(cancellation)?;
        if let (Some(person_id), Some(type_id)) = (row.person_id, row.assignment_type_id) {
            let facts = &memberships[&person_id][&type_id];
            resolve_pair_facts(
                facts,
                &mut row.status,
                &mut row.issue,
                &mut row.before,
                &mut row.after,
                false,
            );
        }
    }
    Ok((source, rows, memberships))
}

fn changed(facts: &PairFacts) -> bool {
    facts.desired.is_some_and(|desired| desired != facts.before)
}

fn updated_person(
    document: &ScenarioDocument,
    original: &Person,
    choices: &BTreeMap<AssignmentTypeId, PairFacts>,
) -> Result<Value, CsvError> {
    let mut raw = document
        .domain
        .entities
        .get(&EntityId::from_uuid(original.id.as_uuid()))
        .ok_or_else(|| invalid(CsvErrorCode::InvalidCurrentDocument))?
        .clone();
    let members = raw
        .get_mut("eligibleAssignmentTypeIds")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid(CsvErrorCode::InvalidCurrentDocument))?;
    let mut index = 0;
    members.retain(|_| {
        let type_id = original.eligible_assignment_type_ids[index];
        index += 1;
        choices
            .get(&type_id)
            .is_none_or(|facts| facts.desired != Some(false))
    });
    let mut additions: Vec<_> = choices
        .iter()
        .filter(|(_, facts)| !facts.before && facts.desired == Some(true))
        .map(|(id, facts)| (facts.first_ordinal, id))
        .collect();
    additions.sort_unstable_by_key(|(record, _)| *record);
    for (_, id) in additions {
        members.push(Value::String(id.to_string()));
    }
    Ok(raw)
}

pub(super) fn proposed_batch(
    document: &ScenarioDocument,
    domain: &WorkforceDomainV1,
    memberships: &Memberships,
    cancellation: &CancellationToken,
    label: &str,
) -> Result<Option<DomainBatchCommand>, CsvError> {
    let changed_people = memberships
        .values()
        .filter(|choices| choices.values().any(changed))
        .count();
    if changed_people > MAX_DOMAIN_BATCH_COMMANDS {
        return Err(invalid(CsvErrorCode::InvalidBatch));
    }
    if changed_people == 0 {
        return Ok(None);
    }
    let mut commands = Vec::with_capacity(changed_people);
    let mut remaining = MAX_CSV_PREVIEW_BYTES;
    for (id, choices) in memberships {
        check(cancellation)?;
        if !choices.values().any(changed) {
            continue;
        }
        let original =
            person(domain, *id).ok_or_else(|| invalid(CsvErrorCode::InvalidCurrentDocument))?;
        let entity = updated_person(document, original, choices)?;
        let command = DomainCommandEnvelope {
            command_type: commands::UPDATE_ENTITY.to_owned(),
            payload: Value::Object([("entity".to_owned(), entity)].into_iter().collect()),
        };
        let bytes = bounded_json_size(&command, remaining)
            .map_err(|_| invalid(CsvErrorCode::PreviewLimit))?;
        remaining = remaining
            .checked_sub(bytes)
            .ok_or_else(|| invalid(CsvErrorCode::PreviewLimit))?;
        commands.push(command);
    }
    let batch = DomainBatchCommand {
        schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: document.domain_pack.id.clone(),
        scenario_schema_version: document.domain_pack.schema_version,
        label: Some(label.to_owned()),
        commands,
    };
    batch
        .validate_bounds()
        .map_err(|_| invalid(CsvErrorCode::InvalidBatch))?;
    Ok(Some(batch))
}

pub(super) fn valid_proposal(
    document: &ScenarioDocument,
    proposal: &DomainBatchCommand,
    cancellation: &CancellationToken,
) -> Result<bool, CsvError> {
    match commands::apply_batch_cancellable(document, proposal, cancellation) {
        Ok(_) => Ok(true),
        Err(DomainPackError::InvalidPayload { .. }) => Ok(false),
        Err(error) => Err(invalid(if matches!(error, DomainPackError::Cancelled) {
            CsvErrorCode::Cancelled
        } else {
            CsvErrorCode::InvalidBatch
        })),
    }
}

pub(super) fn finding(field_path: &str) -> ValidationIssue {
    ValidationIssue { code: "invalidProposedState".to_owned(), severity: ValidationSeverity::Error, message: "The complete proposed eligibility changes violate scenario structure, references or capacity.".to_owned(), field_path: Some(field_path.to_owned()), resource: None }
}

/// Reviews sparse configured memberships against an immutable current snapshot.
///
/// # Errors
/// Rejects invalid source, mapping, skip decisions, current state, bounds and cancellation.
pub fn preview_eligibility_csv<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    mapping: &EligibilityCsvMapping,
    skipped_records: &[u32],
    cancellation: &CancellationToken,
) -> Result<EligibilityImportPreview, CsvError> {
    check(cancellation)?;
    validate_policy(mapping, skipped_records)?;
    let control = OperationControl::Cancellation(cancellation.clone());
    let schemas =
        WorkforceSchemas::load().map_err(|_| invalid(CsvErrorCode::InvalidCurrentDocument))?;
    let domain =
        validate_document_with_schemas(document, &schemas, Some(&control)).map_err(|error| {
            invalid(if matches!(error, DomainPackError::Cancelled) {
                CsvErrorCode::Cancelled
            } else {
                CsvErrorCode::InvalidCurrentDocument
            })
        })?;
    let (source, rows, memberships) =
        scan_rows(input, mapping, skipped_records, &domain, cancellation)?;
    let excluded_rows: Vec<_> = rows
        .iter()
        .filter(|row| {
            matches!(
                row.status,
                EligibilityRowStatus::Blocked | EligibilityRowStatus::Skipped
            )
        })
        .map(|row| EligibilityExcludedRow {
            record: row.record,
            skipped: row.status == EligibilityRowStatus::Skipped,
            issue: row.issue,
        })
        .collect();
    bounded_json_size(&excluded_rows, MAX_ELIGIBILITY_CSV_REPORT_BYTES)
        .map_err(|_| invalid(CsvErrorCode::PreviewLimit))?;
    let mut blocked = rows
        .iter()
        .any(|row| row.status == EligibilityRowStatus::Blocked);
    let mut validation_issues = Vec::new();
    let mut batch = if blocked {
        None
    } else {
        proposed_batch(
            document,
            &domain,
            &memberships,
            cancellation,
            "Import configured eligibility from CSV",
        )?
    };
    if let Some(proposal) = &batch
        && !valid_proposal(document, proposal, cancellation)?
    {
        blocked = true;
        batch = None;
        validation_issues.push(finding("/eligibilityCsv/proposedState"));
    }
    let disposition = if blocked {
        PeopleImportDisposition::Blocked
    } else if batch.is_some() {
        PeopleImportDisposition::Reviewable
    } else {
        PeopleImportDisposition::NoChanges
    };
    check(cancellation)?;
    let review = if blocked {
        None
    } else {
        Some(EligibilityCsvReview {
            format: REVIEW_FORMAT.to_owned(),
            schema_version: 1,
            scenario_id: document.scenario_id,
            revision,
            scenario_blake3: hash(document)?,
            source: source.clone(),
            parser_version: CSV_PARSER_VERSION.to_owned(),
            limits_version: CSV_LIMITS_VERSION,
            mapping: mapping.clone(),
            skipped_records: skipped_records.to_vec(),
            rows_blake3: hash(&rows)?,
            changes_blake3: hash(&batch)?,
            excluded_blake3: hash(&excluded_rows)?,
            validation_blake3: hash(&validation_issues)?,
        })
    };
    let approval_digest = review.as_ref().map(approval_digest).transpose()?;
    let preview = EligibilityImportPreview {
        schema_version: 1,
        disposition,
        source,
        rows,
        excluded_rows,
        validation_issues,
        batch,
        review,
        approval_digest,
    };
    bounded_json_size(&preview, MAX_CSV_PREVIEW_BYTES)
        .map_err(|_| invalid(CsvErrorCode::PreviewLimit))?;
    check(cancellation).map(|()| preview)
}

fn validated_review_value(review: &EligibilityCsvReview) -> Result<Value, CsvError> {
    if review.format != REVIEW_FORMAT
        || review.schema_version != 1
        || review.parser_version != CSV_PARSER_VERSION
        || review.limits_version != CSV_LIMITS_VERSION
    {
        return Err(invalid(CsvErrorCode::UnsupportedVersion));
    }
    validate_policy(&review.mapping, &review.skipped_records)?;
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
            .skipped_records
            .last()
            .is_some_and(|record| *record > review.source.logical_records)
    {
        return Err(invalid(CsvErrorCode::InvalidReview));
    }
    bounded_json_size(review, MAX_CSV_REVIEW_BYTES)
        .map_err(|_| invalid(CsvErrorCode::ReviewLimit))?;
    let value = serde_json::to_value(review).map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    validate_value_bounds(&value, "/eligibilityCsvReview")
        .map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    Ok(value)
}

fn approval_digest(review: &EligibilityCsvReview) -> Result<String, CsvError> {
    #[derive(Serialize)]
    struct Approval<'a> {
        domain: &'static str,
        version: u32,
        review: &'a Value,
    }
    let value = validated_review_value(review)?;
    hash(&Approval {
        domain: "eutheto/workforce-eligibility-csv-approval",
        version: 1,
        review: &value,
    })
}

/// Decodes an exact bounded review; a public digest is never native custody.
///
/// # Errors
/// Rejects unsupported, malformed, unsafe, oversized or alternate representations.
pub fn decode_eligibility_csv_review(bytes: &[u8]) -> Result<EligibilityCsvReview, CsvError> {
    if bytes.len() > MAX_CSV_REVIEW_BYTES {
        return Err(invalid(CsvErrorCode::ReviewLimit));
    }
    let limits = PortableJsonLimits {
        max_depth: ContractJsonLimits::DEFAULT.max_depth,
        max_string_bytes: ContractJsonLimits::DEFAULT.max_string_bytes,
        max_collection_items: ContractJsonLimits::DEFAULT.max_collection_items,
    };
    validate_nonsecret_portable_json_bytes(bytes, &limits)
        .map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    let review: EligibilityCsvReview =
        serde::Deserialize::deserialize(&raw).map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    if validated_review_value(&review)? != raw {
        return Err(invalid(CsvErrorCode::InvalidReview));
    }
    Ok(review)
}

/// Rebuilds the approved complete meaning; the caller owns revision-checked atomic commit.
///
/// # Errors
/// Rejects changed source, snapshot, policy, revision, approval, limits or cancellation.
pub fn rebuild_eligibility_csv_review<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    review: &EligibilityCsvReview,
    approved_digest: &str,
    cancellation: &CancellationToken,
) -> Result<EligibilityImportPreview, CsvError> {
    check(cancellation)?;
    if approval_digest(review)? != approved_digest
        || review.scenario_id != document.scenario_id
        || review.revision != revision
    {
        return Err(invalid(CsvErrorCode::StaleReview));
    }
    let preview = preview_eligibility_csv(
        document,
        revision,
        input,
        &review.mapping,
        &review.skipped_records,
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
