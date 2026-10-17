use super::{
    AssignmentCsvMapping, AssignmentCsvReview, AssignmentExcludedRow, AssignmentImportChanges,
    AssignmentImportError, AssignmentImportPreview, AssignmentImportRow, AssignmentImportTarget,
    AssignmentRowIssue, AssignmentRowStatus, MAX_ASSIGNMENT_CSV_REPORT_BYTES,
    source::{ImportRecord, check, scan_source},
};
use crate::{
    assignment_rules::baseline::workforce_pair_content_hash,
    commands,
    ids::ShiftId,
    model::{
        AssignmentPair, ExternalStartingPlan, ExternalStartingPlanImport, WorkforceDomainV1,
        WorkforceEntity,
    },
    people_csv::{
        CSV_LIMITS_VERSION, CSV_PARSER_VERSION, CsvError, CsvErrorCode, MAX_CSV_COLUMNS,
        MAX_CSV_DATA_RECORDS, MAX_CSV_DECISION_BYTES, MAX_CSV_DECISIONS, MAX_CSV_MAPPING_BYTES,
        MAX_CSV_PREVIEW_BYTES, MAX_CSV_REVIEW_BYTES, MAX_CSV_SOURCE_BYTES, PeopleImportDisposition,
        hash, resolve_identity, valid_digest, valid_identity_alias,
    },
    validation::{
        MAX_DISPLAY_BYTES, WorkforceSchemas, validate_document_with_schemas, validate_value_bounds,
    },
};
use eutheto_domain_api::{
    ContractJsonLimits, DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainPackError,
    bounded_json_size,
};
use eutheto_types::{
    CancellationToken, DomainCommandEnvelope, EntityId, OperationControl, PersonId,
    PortableJsonLimits, Revision, ScenarioDocument, ValidationIssue, ValidationSeverity,
    collect_document_owned_uuids, validate_nonsecret_portable_json_bytes,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

const REVIEW_FORMAT: &str = "eutheto/workforce-assignment-csv-review";
const CANONICAL_PARSER_VERSION: &str = "eutheto-assignments-csv-1";

fn invalid(code: CsvErrorCode) -> CsvError {
    CsvError::source(code)
}

fn parser_version(mapping: &AssignmentCsvMapping) -> &'static str {
    match mapping {
        AssignmentCsvMapping::CanonicalV1 => CANONICAL_PARSER_VERSION,
        AssignmentCsvMapping::Mapped { .. } => CSV_PARSER_VERSION,
    }
}

fn validate_policy(
    mapping: &AssignmentCsvMapping,
    target: &AssignmentImportTarget,
    skipped: &[u32],
) -> Result<(), CsvError> {
    bounded_json_size(mapping, MAX_CSV_MAPPING_BYTES)
        .map_err(|_| invalid(CsvErrorCode::MappingLimit))?;
    if target.id().as_uuid().get_version_num() != 7
        || target.name().len() > MAX_DISPLAY_BYTES
        || target.name().trim().is_empty()
        || target.name().chars().any(char::is_control)
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
            *record <= mapping.framing_records()
                || *record > MAX_CSV_DATA_RECORDS + mapping.framing_records()
        })
    {
        return Err(invalid(CsvErrorCode::InvalidDecision));
    }
    if let AssignmentCsvMapping::Mapped {
        expected_columns,
        person_column,
        shift_column,
        person_mappings,
        shift_mappings,
        ..
    } = mapping
    {
        if *expected_columns == 0
            || usize::from(*expected_columns) > MAX_CSV_COLUMNS
            || *person_column >= *expected_columns
            || *shift_column >= *expected_columns
            || person_column == shift_column
            || person_mappings.len() > MAX_CSV_DECISIONS
            || shift_mappings.len() > MAX_CSV_DECISIONS
            || person_mappings
                .values()
                .any(|id| id.as_uuid().get_version_num() != 7)
        {
            return Err(invalid(CsvErrorCode::InvalidMapping));
        }
        for token in person_mappings.keys().chain(shift_mappings.keys()) {
            if !valid_identity_alias(token) {
                return Err(invalid(CsvErrorCode::InvalidMapping));
            }
        }
    }
    Ok(())
}

fn known_person(domain: &WorkforceDomainV1, id: PersonId) -> bool {
    matches!(
        domain.entities.get(&EntityId::from_uuid(id.as_uuid())),
        Some(WorkforceEntity::Person(_))
    )
}

fn row_pair(row: &AssignmentImportRow) -> Option<AssignmentPair> {
    Some(AssignmentPair {
        person_id: row.person_id?,
        shift_id: row.shift_id?,
    })
}

fn map_record(
    record: &ImportRecord<'_>,
    mapping: &AssignmentCsvMapping,
    domain: &WorkforceDomainV1,
    shifts: &BTreeSet<ShiftId>,
) -> Result<Option<AssignmentImportRow>, CsvError> {
    let (number, person_id, shift_id, issue) = match record {
        ImportRecord::Canonical { number, pair, .. } => {
            let person = known_person(domain, pair.person_id).then_some(pair.person_id);
            let shift = shifts.contains(&pair.shift_id).then_some(pair.shift_id);
            let issue = if person.is_none() {
                Some(AssignmentRowIssue::UnresolvedPerson)
            } else if shift.is_none() {
                Some(AssignmentRowIssue::UnresolvedShift)
            } else {
                None
            };
            (*number, person, shift, issue)
        }
        ImportRecord::Mapped(record) => {
            let AssignmentCsvMapping::Mapped {
                has_header,
                expected_columns,
                person_column,
                shift_column,
                person_mappings,
                shift_mappings,
                ..
            } = mapping
            else {
                return Err(invalid(CsvErrorCode::InvalidMapping));
            };
            if *has_header && record.number == 1 {
                if record.cells.len() != usize::from(*expected_columns) {
                    return Err(CsvError::at(CsvErrorCode::HeaderMismatch, record.number));
                }
                return Ok(None);
            }
            if record.number - u32::from(*has_header) > MAX_CSV_DATA_RECORDS {
                return Err(CsvError::at(CsvErrorCode::DataRecordLimit, record.number));
            }
            if record.cells.len() == usize::from(*expected_columns) {
                let (person, person_issue) = resolve_identity(
                    record.cells[usize::from(*person_column)],
                    person_mappings,
                    |id| known_person(domain, id),
                    AssignmentRowIssue::InvalidToken,
                    AssignmentRowIssue::UnresolvedPerson,
                );
                let (shift, shift_issue) = resolve_identity(
                    record.cells[usize::from(*shift_column)],
                    shift_mappings,
                    |id| shifts.contains(&id),
                    AssignmentRowIssue::InvalidToken,
                    AssignmentRowIssue::UnresolvedShift,
                );
                (record.number, person, shift, person_issue.or(shift_issue))
            } else {
                (
                    record.number,
                    None,
                    None,
                    Some(AssignmentRowIssue::ColumnCount),
                )
            }
        }
    };
    Ok(Some(AssignmentImportRow {
        record: number,
        status: if issue.is_some() {
            AssignmentRowStatus::Blocked
        } else {
            AssignmentRowStatus::Included
        },
        person_id,
        shift_id,
        issue,
    }))
}

fn domain_error(error: &DomainPackError) -> CsvError {
    invalid(if matches!(error, DomainPackError::Cancelled) {
        CsvErrorCode::Cancelled
    } else {
        CsvErrorCode::InvalidBatch
    })
}

fn finding(code: &str, message: &str) -> ValidationIssue {
    ValidationIssue {
        code: code.to_owned(),
        severity: ValidationSeverity::Error,
        message: message.to_owned(),
        field_path: Some("/assignmentCsv/proposedState".to_owned()),
        resource: None,
    }
}

/// Reviews the complete input and one whole external-plan replacement without mutating state.
/// Invalid or duplicate unskipped rows block authority; infeasibility does not confer or remove authority.
///
/// # Errors
/// Rejects malformed/bounded input, invalid mapping/target/decisions, invalid current state,
/// resource limits and cancellation. Report and approval cover every original logical row.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn preview_assignment_csv<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    mapping: &AssignmentCsvMapping,
    target: &AssignmentImportTarget,
    skipped_records: &[u32],
    allow_empty: bool,
    cancellation: &CancellationToken,
) -> Result<AssignmentImportPreview, AssignmentImportError> {
    check(cancellation)?;
    validate_policy(mapping, target, skipped_records)?;
    let control = OperationControl::Cancellation(cancellation.clone());
    let schemas =
        WorkforceSchemas::load().map_err(|_| invalid(CsvErrorCode::InvalidCurrentDocument))?;
    let domain =
        validate_document_with_schemas(document, &schemas, Some(&control)).map_err(|error| {
            if matches!(error, DomainPackError::Cancelled) {
                invalid(CsvErrorCode::Cancelled)
            } else {
                invalid(CsvErrorCode::InvalidCurrentDocument)
            }
        })?;
    let previous = match target {
        AssignmentImportTarget::Create { id, .. } => {
            if collect_document_owned_uuids(document).contains(&id.as_uuid()) {
                return Err(invalid(CsvErrorCode::InvalidMapping).into());
            }
            None
        }
        AssignmentImportTarget::Replace { id, .. } => match domain.entities.get(id) {
            Some(WorkforceEntity::ExternalStartingPlan(plan)) => Some(plan),
            _ => return Err(invalid(CsvErrorCode::InvalidMapping).into()),
        },
    };
    // Membership is structural, not whole-horizon generation or candidate eligibility.
    let mut shifts = BTreeSet::new();
    for entity in domain.entities.values() {
        check(cancellation)?;
        match entity {
            WorkforceEntity::ShiftInstance(shift) => {
                shifts.insert(shift.id);
            }
            WorkforceEntity::ShiftTemplate(template) => {
                for id in template.occurrence_identities.keys() {
                    check(cancellation)?;
                    shifts.insert(*id);
                }
            }
            _ => {}
        }
    }
    let mut rows = Vec::new();
    let mut pair_counts = BTreeMap::<AssignmentPair, (usize, usize)>::new();
    let source = scan_source(input, mapping.read_format(), cancellation, |record| {
        if let Some(mut row) = map_record(&record, mapping, &domain, &shifts)? {
            let skipped = skipped_records.binary_search(&row.record).is_ok();
            if skipped {
                row.status = AssignmentRowStatus::Skipped;
            }
            if let Some(pair) = row_pair(&row) {
                // The scanner admits at most 10,000 data rows before this counter changes.
                let counts = pair_counts.entry(pair).or_default();
                counts.0 += 1;
                counts.1 += usize::from(!skipped);
            }
            rows.push(row);
        }
        Ok(())
    })?;
    if source.logical_records < mapping.framing_records() {
        return Err(invalid(CsvErrorCode::HeaderMismatch).into());
    }
    if skipped_records
        .last()
        .is_some_and(|record| *record > source.logical_records)
    {
        return Err(invalid(CsvErrorCode::InvalidDecision).into());
    }
    for row in &mut rows {
        check(cancellation)?;
        if let Some(pair) = row_pair(row) {
            let (total, included) = pair_counts[&pair];
            if row.status == AssignmentRowStatus::Skipped && total > 1 {
                row.issue = Some(AssignmentRowIssue::DuplicatePair);
            } else if row.status != AssignmentRowStatus::Skipped && included > 1 {
                row.status = AssignmentRowStatus::Blocked;
                row.issue = Some(AssignmentRowIssue::DuplicatePair);
            }
        }
    }
    let excluded_rows: Vec<_> = rows
        .iter()
        .filter(|row| row.status != AssignmentRowStatus::Included)
        .map(|row| AssignmentExcludedRow {
            record: row.record,
            skipped: row.status == AssignmentRowStatus::Skipped,
            issue: row.issue,
        })
        .collect();
    bounded_json_size(&excluded_rows, MAX_ASSIGNMENT_CSV_REPORT_BYTES)
        .map_err(|_| invalid(CsvErrorCode::RejectedReportLimit))?;
    let mut blocked = rows
        .iter()
        .any(|row| row.status == AssignmentRowStatus::Blocked);
    let mut validation_issues = Vec::new();
    let mut batch = None;
    let mut changes = None;
    if !blocked {
        let selected: BTreeSet<_> = rows
            .iter()
            .filter(|row| row.status == AssignmentRowStatus::Included)
            .filter_map(row_pair)
            .collect();
        if selected.is_empty() && !allow_empty {
            blocked = true;
            validation_issues.push(finding(
                "emptyApprovalRequired",
                "Explicit approval is required to create or replace a plan with no pairs.",
            ));
        }
        let assignments: Vec<_> = selected.iter().copied().collect();
        let original_import = ExternalStartingPlanImport {
            source_blake3: source.blake3.clone(),
            pair_content_hash: workforce_pair_content_hash(&assignments, Some(&control))
                .map_err(|error| domain_error(&error))?,
        };
        let old_pairs: BTreeSet<_> = previous
            .into_iter()
            .flat_map(|plan| plan.assignments.iter().copied())
            .collect();
        changes = Some(AssignmentImportChanges {
            previous_name: previous.map(|plan| plan.name.clone()),
            before_pair_count: old_pairs.len(),
            after_pair_count: selected.len(),
            added_pairs: selected.difference(&old_pairs).copied().collect(),
            removed_pairs: old_pairs.difference(&selected).copied().collect(),
            provenance_changed: previous.is_none_or(|plan| plan.original_import != original_import),
        });
        let proposed = ExternalStartingPlan {
            schema_version: 1,
            id: target.id(),
            name: target.name().to_owned(),
            assignments,
            original_import,
        };
        if !blocked && previous != Some(&proposed) {
            let entity = serde_json::to_value(WorkforceEntity::ExternalStartingPlan(proposed))
                .map_err(|_| invalid(CsvErrorCode::InvalidBatch))?;
            let command = DomainCommandEnvelope {
                command_type: if previous.is_some() {
                    commands::UPDATE_ENTITY
                } else {
                    commands::ADD_ENTITY
                }
                .to_owned(),
                payload: Value::Object([("entity".to_owned(), entity)].into_iter().collect()),
            };
            let proposed_batch = DomainBatchCommand {
                schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
                pack_id: document.domain_pack.id.clone(),
                scenario_schema_version: document.domain_pack.schema_version,
                label: Some("Import unverified starting plan from CSV".to_owned()),
                commands: vec![command],
            };
            proposed_batch
                .validate_bounds()
                .map_err(|_| invalid(CsvErrorCode::InvalidBatch))?;
            match commands::apply_batch_cancellable(document, &proposed_batch, cancellation) {
                Ok(_) => batch = Some(proposed_batch),
                Err(DomainPackError::InvalidPayload { .. }) => {
                    blocked = true;
                    validation_issues.push(finding("invalidProposedState", "The complete proposed plan violates scenario structure, references or capacity."));
                }
                Err(error) => return Err(domain_error(&error).into()),
            }
        }
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
        Some(AssignmentCsvReview {
            format: REVIEW_FORMAT.to_owned(),
            schema_version: 1,
            scenario_id: document.scenario_id,
            revision,
            scenario_blake3: hash(document)?,
            source: source.clone(),
            parser_version: parser_version(mapping).to_owned(),
            limits_version: CSV_LIMITS_VERSION,
            mapping: mapping.clone(),
            target: target.clone(),
            skipped_records: skipped_records.to_vec(),
            allow_empty,
            rows_blake3: hash(&rows)?,
            changes_blake3: hash(&batch)?,
            excluded_blake3: hash(&excluded_rows)?,
            validation_blake3: hash(&validation_issues)?,
        })
    };
    let approval_digest = review.as_ref().map(approval_digest).transpose()?;
    let preview = AssignmentImportPreview {
        schema_version: 1,
        disposition,
        source,
        rows,
        excluded_rows,
        changes,
        validation_issues,
        batch,
        review,
        approval_digest,
    };
    bounded_json_size(&preview, MAX_CSV_PREVIEW_BYTES)
        .map_err(|_| invalid(CsvErrorCode::PreviewLimit))?;
    check(cancellation)?;
    Ok(preview)
}

fn validated_review_value(review: &AssignmentCsvReview) -> Result<Value, CsvError> {
    if review.format != REVIEW_FORMAT
        || review.schema_version != 1
        || review.parser_version != parser_version(&review.mapping)
        || review.limits_version != CSV_LIMITS_VERSION
    {
        return Err(invalid(CsvErrorCode::UnsupportedVersion));
    }
    validate_policy(&review.mapping, &review.target, &review.skipped_records)?;
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
        .any(|value| !valid_digest(value))
        || usize::try_from(review.source.raw_bytes)
            .map_or(true, |bytes| bytes > MAX_CSV_SOURCE_BYTES)
        || review.source.logical_records > MAX_CSV_DATA_RECORDS + review.mapping.framing_records()
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
    validate_value_bounds(&value, "/assignmentCsvReview")
        .map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    Ok(value)
}

fn approval_digest(review: &AssignmentCsvReview) -> Result<String, CsvError> {
    #[derive(Serialize)]
    struct Approval<'a> {
        domain: &'static str,
        version: u32,
        review: &'a Value,
    }
    let value = validated_review_value(review)?;
    hash(&Approval {
        domain: "eutheto/workforce-assignment-csv-approval",
        version: 1,
        review: &value,
    })
}

/// Decodes a bounded exact review representation; public hashes are not native custody.
///
/// # Errors
/// Rejects unsupported versions, malformed/unsafe JSON and alternate typed representations.
pub fn decode_assignment_csv_review(
    bytes: &[u8],
) -> Result<AssignmentCsvReview, AssignmentImportError> {
    if bytes.len() > MAX_CSV_REVIEW_BYTES {
        return Err(invalid(CsvErrorCode::ReviewLimit).into());
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
    let review: AssignmentCsvReview =
        serde::Deserialize::deserialize(&raw).map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    if validated_review_value(&review)? != raw {
        return Err(invalid(CsvErrorCode::InvalidReview).into());
    }
    Ok(review)
}

/// Rebuilds complete approved meaning from the current document and native source bytes.
///
/// # Errors
/// Rejects changed source, target, policy, revision, approval, unsupported review data,
/// invalid proposals and cancellation. The caller still owns the atomic commit boundary.
pub fn rebuild_assignment_csv_review<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    review: &AssignmentCsvReview,
    approved_digest: &str,
    cancellation: &CancellationToken,
) -> Result<AssignmentImportPreview, AssignmentImportError> {
    check(cancellation)?;
    if approval_digest(review)? != approved_digest
        || review.scenario_id != document.scenario_id
        || review.revision != revision
    {
        return Err(invalid(CsvErrorCode::StaleReview).into());
    }
    let preview = preview_assignment_csv(
        document,
        revision,
        input,
        &review.mapping,
        &review.target,
        &review.skipped_records,
        review.allow_empty,
        cancellation,
    )?;
    if preview.review.as_ref() != Some(review)
        || preview.approval_digest.as_deref() != Some(approved_digest)
    {
        return Err(invalid(CsvErrorCode::StaleReview).into());
    }
    check(cancellation)?;
    Ok(preview)
}
