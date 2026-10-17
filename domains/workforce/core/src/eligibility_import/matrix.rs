use super::{
    ELIGIBILITY_MATRIX_LIMITS_VERSION, ELIGIBILITY_MATRIX_PARSER_VERSION, EligibilityMatrixCell,
    EligibilityMatrixCellAddress, EligibilityMatrixDetection, EligibilityMatrixExcludedCell,
    EligibilityMatrixMapping, EligibilityMatrixPreview, EligibilityMatrixReview,
    EligibilityRowIssue, EligibilityRowStatus, MAX_ELIGIBILITY_MATRIX_CELLS,
    MAX_ELIGIBILITY_MATRIX_REPORT_BYTES, MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES,
    review::{
        Memberships, check, finding, include_pair, invalid, person, proposed_batch,
        resolve_pair_facts, scan_before, valid_proposal,
    },
};
use crate::{
    model::{WorkforceDomainV1, WorkforceEntity},
    people_csv::{
        CsvDialect, CsvError, CsvErrorCode, CsvRecord, CsvSampleRecord, CsvSource, MAX_CSV_COLUMNS,
        MAX_CSV_DECISION_BYTES, MAX_CSV_DECISIONS, MAX_CSV_DETECTION_BYTES,
        MAX_CSV_INSPECTED_CELL_BYTES, MAX_CSV_MAPPING_BYTES, MAX_CSV_PREVIEW_BYTES,
        MAX_CSV_REVIEW_BYTES, MAX_CSV_SAMPLE_CELL_BYTES, PeopleImportDisposition, hash,
        resolve_identity, sample_record, scan_csv, valid_digest, valid_identity_alias,
    },
    validation::{WorkforceSchemas, validate_document_with_schemas, validate_value_bounds},
};
use eutheto_domain_api::{ContractJsonLimits, DomainPackError, bounded_json_size};
use eutheto_types::{
    CancellationToken, OperationControl, PortableJsonLimits, Revision, ScenarioDocument,
    validate_nonsecret_portable_json_bytes,
};
use serde::Serialize;
use serde_json::Value;
use std::io::Read;

const REVIEW_FORMAT: &str = "eutheto/workforce-eligibility-matrix-review";

fn validate_policy(
    mapping: &EligibilityMatrixMapping,
    skipped: &[EligibilityMatrixCellAddress],
) -> Result<(), CsvError> {
    bounded_json_size(mapping, MAX_CSV_MAPPING_BYTES)
        .map_err(|_| invalid(CsvErrorCode::MappingLimit))?;
    if mapping.person_mappings.len() > MAX_CSV_DECISIONS
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
        || skipped.iter().any(|cell| {
            cell.record < 2
                || cell.record > MAX_ELIGIBILITY_MATRIX_CELLS + 1
                || cell.column == 0
                || usize::from(cell.column) >= MAX_CSV_COLUMNS
        })
    {
        return Err(invalid(CsvErrorCode::InvalidDecision));
    }
    Ok(())
}

fn validate_skips(
    skipped: &[EligibilityMatrixCellAddress],
    columns: u16,
    records: u32,
) -> Result<(), CsvError> {
    if skipped
        .iter()
        .any(|cell| cell.column >= columns || cell.record > records)
    {
        return Err(invalid(CsvErrorCode::InvalidDecision));
    }
    Ok(())
}

/// The shared scanner owns decoding; this layer owns the complete rectangle census.
fn scan_matrix<R: Read + ?Sized>(
    input: &mut R,
    cancellation: &CancellationToken,
    mut visit: impl FnMut(CsvRecord<'_>) -> Result<(), CsvError>,
) -> Result<(CsvSource, u16, u32), CsvError> {
    let mut columns = 0;
    let mut cells = 0u32;
    let mut capped = input.take(MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES as u64 + 1);
    let source = scan_csv(&mut capped, CsvDialect::Tab, cancellation, |record| {
        check(cancellation)?;
        if record.number == 1 {
            if !(2..=MAX_CSV_COLUMNS).contains(&record.cells.len())
                || !matches!(record.cells[0], "" | "personId")
            {
                return Err(CsvError::at(CsvErrorCode::MatrixShape, record.number));
            }
            columns = u16::try_from(record.cells.len())
                .map_err(|_| CsvError::at(CsvErrorCode::MatrixShape, record.number))?;
        } else {
            if record.cells.len() != usize::from(columns) {
                return Err(CsvError::at(CsvErrorCode::MatrixShape, record.number));
            }
            cells = cells
                .checked_add(u32::from(columns - 1))
                .filter(|count| *count <= MAX_ELIGIBILITY_MATRIX_CELLS)
                .ok_or_else(|| CsvError::at(CsvErrorCode::MatrixCellLimit, record.number))?;
        }
        visit(record)
    })?;
    if source.raw_bytes > MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES as u64 {
        return Err(invalid(CsvErrorCode::SourceByteLimit));
    }
    if columns == 0 {
        return Err(invalid(CsvErrorCode::MatrixShape));
    }
    check(cancellation)?;
    Ok((source, columns, cells))
}

/// Detects a tab-delimited eligibility rectangle, including unsampled suffixes.
///
/// # Errors
/// Rejects invalid shape, source, census, bounds, I/O or cancellation.
pub fn detect_eligibility_matrix<R: Read + ?Sized>(
    input: &mut R,
    cancellation: &CancellationToken,
) -> Result<EligibilityMatrixDetection, CsvError> {
    let mut samples = Vec::with_capacity(2);
    let (source, columns, cells) = scan_matrix(input, cancellation, |record| {
        if samples.len() < 2 {
            samples.push(sample_record(&record, MAX_CSV_SAMPLE_CELL_BYTES));
        }
        Ok(())
    })?;
    let detection = EligibilityMatrixDetection {
        schema_version: 1,
        source,
        columns,
        cells,
        samples,
    };
    bounded_json_size(&detection, MAX_CSV_DETECTION_BYTES)
        .map_err(|_| invalid(CsvErrorCode::DetectionLimit))?;
    check(cancellation)?;
    Ok(detection)
}

/// Inspects an inert logical-record sample after validating the complete rectangle.
///
/// # Errors
/// Rejects invalid shape, source, census, bounds, I/O or cancellation.
pub fn sample_eligibility_matrix_record<R: Read + ?Sized>(
    input: &mut R,
    record_number: u32,
    cancellation: &CancellationToken,
) -> Result<Option<CsvSampleRecord>, CsvError> {
    let mut sample = None;
    scan_matrix(input, cancellation, |record| {
        if record.number == record_number {
            sample = Some(sample_record(&record, MAX_CSV_INSPECTED_CELL_BYTES));
        }
        Ok(())
    })?;
    check(cancellation)?;
    Ok(sample)
}

type MatrixRows = (CsvSource, u16, u32, Vec<EligibilityMatrixCell>, Memberships);

fn scan_rows<R: Read + ?Sized>(
    input: &mut R,
    mapping: &EligibilityMatrixMapping,
    skipped_cells: &[EligibilityMatrixCellAddress],
    domain: &WorkforceDomainV1,
    cancellation: &CancellationToken,
) -> Result<MatrixRows, CsvError> {
    let mut rows = Vec::new();
    let mut types = Vec::new();
    let mut memberships = Memberships::new();
    let (source, columns, cells) = scan_matrix(input, cancellation, |record| {
        if record.number == 1 {
            for token in &record.cells[1..] {
                check(cancellation)?;
                types.push(resolve_identity(
                    token,
                    &mapping.assignment_type_mappings,
                    |id| {
                        matches!(
                            domain.entities.get(&id.as_entity_id()),
                            Some(WorkforceEntity::AssignmentType(_))
                        )
                    },
                    EligibilityRowIssue::InvalidToken,
                    EligibilityRowIssue::UnresolvedAssignmentType,
                ));
            }
            return Ok(());
        }
        let (person_id, person_issue) = resolve_identity(
            record.cells[0],
            &mapping.person_mappings,
            |id| person(domain, id).is_some(),
            EligibilityRowIssue::InvalidToken,
            EligibilityRowIssue::UnresolvedPerson,
        );
        for (index, token) in record.cells[1..].iter().enumerate() {
            check(cancellation)?;
            let (assignment_type_id, type_issue) = types[index];
            let (desired, membership_issue) = match *token {
                "true" => (Some(true), None),
                "false" => (Some(false), None),
                "" => (None, None),
                _ => (None, Some(EligibilityRowIssue::InvalidMembership)),
            };
            let cell = EligibilityMatrixCellAddress {
                record: record.number,
                column: u16::try_from(index + 1)
                    .map_err(|_| CsvError::at(CsvErrorCode::MatrixShape, record.number))?,
            };
            let omitted = skipped_cells.binary_search(&cell).is_ok();
            let issue = person_issue.or(type_issue).or(membership_issue);
            if let (Some(person_id), Some(type_id)) = (person_id, assignment_type_id) {
                // Unique even when a row contains several types; skipped duplicates
                // cannot steal the first included source position.
                let ordinal = u32::try_from(rows.len())
                    .map_err(|_| CsvError::at(CsvErrorCode::MatrixCellLimit, record.number))?;
                include_pair(
                    &mut memberships,
                    person_id,
                    type_id,
                    desired,
                    ordinal,
                    omitted,
                );
            }
            rows.push(EligibilityMatrixCell {
                cell,
                status: if omitted {
                    EligibilityRowStatus::Skipped
                } else if issue.is_some() {
                    EligibilityRowStatus::Blocked
                } else {
                    EligibilityRowStatus::Unchanged
                },
                person_id,
                assignment_type_id,
                before: None,
                after: desired,
                issue,
            });
        }
        Ok(())
    })?;
    validate_skips(skipped_cells, columns, source.logical_records)?;
    scan_before(&mut memberships, domain, cancellation)?;
    for row in &mut rows {
        check(cancellation)?;
        if let (Some(person_id), Some(type_id)) = (row.person_id, row.assignment_type_id) {
            resolve_pair_facts(
                &memberships[&person_id][&type_id],
                &mut row.status,
                &mut row.issue,
                &mut row.before,
                &mut row.after,
                true,
            );
        }
    }
    Ok((source, columns, cells, rows, memberships))
}

/// Reviews configured memberships without inferring computed assignment feasibility.
///
/// # Errors
/// Rejects invalid source, policy, current document, bounds or cancellation.
pub fn preview_eligibility_matrix<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    mapping: &EligibilityMatrixMapping,
    skipped_cells: &[EligibilityMatrixCellAddress],
    cancellation: &CancellationToken,
) -> Result<EligibilityMatrixPreview, CsvError> {
    check(cancellation)?;
    validate_policy(mapping, skipped_cells)?;
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
    let (source, columns, cells, rows, memberships) =
        scan_rows(input, mapping, skipped_cells, &domain, cancellation)?;
    let excluded_cells: Vec<_> = rows
        .iter()
        .filter(|row| {
            matches!(
                row.status,
                EligibilityRowStatus::Blocked | EligibilityRowStatus::Skipped
            )
        })
        .map(|row| EligibilityMatrixExcludedCell {
            cell: row.cell,
            skipped: row.status == EligibilityRowStatus::Skipped,
            issue: row.issue,
        })
        .collect();
    bounded_json_size(&excluded_cells, MAX_ELIGIBILITY_MATRIX_REPORT_BYTES)
        .map_err(|_| invalid(CsvErrorCode::PreviewLimit))?;
    let mut blocked = rows
        .iter()
        .any(|row| row.status == EligibilityRowStatus::Blocked);
    let mut batch = if blocked {
        None
    } else {
        proposed_batch(
            document,
            &domain,
            &memberships,
            cancellation,
            "Import configured eligibility from pasted matrix",
        )?
    };
    let mut validation_issues = Vec::new();
    if let Some(proposal) = &batch
        && !valid_proposal(document, proposal, cancellation)?
    {
        blocked = true;
        batch = None;
        validation_issues.push(finding("/eligibilityMatrix/proposedState"));
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
        Some(EligibilityMatrixReview {
            format: REVIEW_FORMAT.to_owned(),
            schema_version: 1,
            scenario_id: document.scenario_id,
            revision,
            scenario_blake3: hash(document)?,
            source: source.clone(),
            parser_version: ELIGIBILITY_MATRIX_PARSER_VERSION.to_owned(),
            limits_version: ELIGIBILITY_MATRIX_LIMITS_VERSION,
            columns,
            cells,
            mapping: mapping.clone(),
            skipped_cells: skipped_cells.to_vec(),
            rows_blake3: hash(&rows)?,
            changes_blake3: hash(&batch)?,
            excluded_blake3: hash(&excluded_cells)?,
            validation_blake3: hash(&validation_issues)?,
        })
    };
    let approval_digest = review.as_ref().map(approval_digest).transpose()?;
    let preview = EligibilityMatrixPreview {
        schema_version: 1,
        disposition,
        source,
        columns,
        cells,
        rows,
        excluded_cells,
        validation_issues,
        batch,
        review,
        approval_digest,
    };
    bounded_json_size(&preview, MAX_CSV_PREVIEW_BYTES)
        .map_err(|_| invalid(CsvErrorCode::PreviewLimit))?;
    check(cancellation).map(|()| preview)
}

fn validated_review_value(review: &EligibilityMatrixReview) -> Result<Value, CsvError> {
    if review.format != REVIEW_FORMAT
        || review.schema_version != 1
        || review.parser_version != ELIGIBILITY_MATRIX_PARSER_VERSION
        || review.limits_version != ELIGIBILITY_MATRIX_LIMITS_VERSION
    {
        return Err(invalid(CsvErrorCode::UnsupportedVersion));
    }
    validate_policy(&review.mapping, &review.skipped_cells)?;
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
        || review.source.raw_bytes == 0
        || review.source.raw_bytes > MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES as u64
        || !(2..=MAX_CSV_COLUMNS).contains(&usize::from(review.columns))
        || review.source.logical_records == 0
        || review
            .source
            .logical_records
            .checked_sub(1)
            .and_then(|records| records.checked_mul(u32::from(review.columns - 1)))
            != Some(review.cells)
        || review.cells > MAX_ELIGIBILITY_MATRIX_CELLS
    {
        return Err(invalid(CsvErrorCode::InvalidReview));
    }
    validate_skips(
        &review.skipped_cells,
        review.columns,
        review.source.logical_records,
    )?;
    bounded_json_size(review, MAX_CSV_REVIEW_BYTES)
        .map_err(|_| invalid(CsvErrorCode::ReviewLimit))?;
    let value = serde_json::to_value(review).map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    validate_value_bounds(&value, "/eligibilityMatrixReview")
        .map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    Ok(value)
}

fn approval_digest(review: &EligibilityMatrixReview) -> Result<String, CsvError> {
    #[derive(Serialize)]
    struct Approval<'a> {
        domain: &'static str,
        version: u32,
        review: &'a Value,
    }
    let value = validated_review_value(review)?;
    hash(&Approval {
        domain: "eutheto/workforce-eligibility-matrix-approval",
        version: 1,
        review: &value,
    })
}

/// Decodes the exact bounded matrix review representation; never grants native custody.
///
/// # Errors
/// Rejects unsupported versions, alternate representations and unsafe or unbounded policies.
pub fn decode_eligibility_matrix_review(bytes: &[u8]) -> Result<EligibilityMatrixReview, CsvError> {
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
    let review: EligibilityMatrixReview =
        serde::Deserialize::deserialize(&raw).map_err(|_| invalid(CsvErrorCode::InvalidReview))?;
    if validated_review_value(&review)? != raw {
        return Err(invalid(CsvErrorCode::InvalidReview));
    }
    Ok(review)
}

/// Rebuilds the approved complete source, shape, policy and current-state meaning.
///
/// # Errors
/// Rejects changed source, snapshot, revision, approval, policy, versions or cancellation.
pub fn rebuild_eligibility_matrix_review<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    review: &EligibilityMatrixReview,
    approved_digest: &str,
    cancellation: &CancellationToken,
) -> Result<EligibilityMatrixPreview, CsvError> {
    check(cancellation)?;
    if approval_digest(review)? != approved_digest
        || review.scenario_id != document.scenario_id
        || review.revision != revision
    {
        return Err(invalid(CsvErrorCode::StaleReview));
    }
    let preview = preview_eligibility_matrix(
        document,
        revision,
        input,
        &review.mapping,
        &review.skipped_cells,
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
