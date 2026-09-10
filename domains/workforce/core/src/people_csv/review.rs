mod binding;
mod rows;

use super::{
    mapping::validate_mapping,
    parsing::scan_csv,
    types::{
        CSV_LIMITS_VERSION, CSV_PARSER_VERSION, ColumnMapping, CsvError, CsvErrorCode, CsvSource,
        MAX_CSV_PREVIEW_BYTES, MAX_CSV_VALIDATION_BYTES, MAX_CSV_VALIDATION_ISSUES,
        PeopleCsvMapping, RejectedRow, RowDecision, RowRejectionCode,
    },
};
use crate::{
    commands,
    validation::{WorkforceSchemas, validate_document_with_schemas},
};
use eutheto_domain_api::{DomainBatchCommand, DomainPackError, bounded_json_size};
use eutheto_types::{
    CancellationToken, Revision, ScenarioDocument, ScenarioId, ValidationIssue, ValidationSeverity,
};
use serde::{Deserialize, Serialize};
use std::io::Read;

const REVIEW_FORMAT: &str = "eutheto/workforce-people-csv-review";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PeopleImportDisposition {
    Reviewable,
    Blocked,
    NoChanges,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PeopleRowStatus {
    Added,
    Updated,
    Unchanged,
    Skipped,
    Rejected,
    Unresolved,
    Conflict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeopleImportRow {
    pub record: u32,
    pub status: PeopleRowStatus,
    pub person_id: Option<eutheto_types::PersonId>,
    pub rejection: Option<RowRejectionCode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeopleCsvReview {
    pub format: String,
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub scenario_blake3: String,
    pub source: CsvSource,
    pub parser_version: String,
    pub limits_version: u32,
    pub mapping: PeopleCsvMapping,
    pub decisions: Vec<RowDecision>,
    pub changes_blake3: String,
    pub rejected_blake3: String,
    pub validation_blake3: String,
}

/// Contains proposed ordinary commands only. Durable apply/approval custody is host-owned.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeopleImportPreview {
    pub schema_version: u32,
    pub disposition: PeopleImportDisposition,
    pub source: CsvSource,
    pub columns: Vec<ColumnMapping>,
    pub rows: Vec<PeopleImportRow>,
    pub rejected_rows: Vec<RejectedRow>,
    /// Safe bounded findings, ordered by rejected record followed by proposed-state failure.
    pub validation_issues: Vec<ValidationIssue>,
    pub batch: Option<DomainBatchCommand>,
    pub review: Option<PeopleCsvReview>,
    /// Supplied independently of the review artifact when rebuilding an approved import.
    pub approval_digest: Option<String>,
}

pub(super) fn check_cancelled(cancellation: &CancellationToken) -> Result<(), CsvError> {
    if cancellation.is_cancelled() {
        Err(CsvError::source(CsvErrorCode::Cancelled))
    } else {
        Ok(())
    }
}

/// Streams and validates a people import without changing the supplied scenario.
///
/// Fatal input/policy/resource failures return an error. Unresolved identities,
/// duplicate targets and invalid proposed state return a blocked preview without authority.
///
/// # Errors
///
/// Rejects invalid current state, source/policy/report limits, invalid decisions and
/// internal command failures. Cancellation never returns an approved batch.
pub fn preview_people_csv<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    mapping: &PeopleCsvMapping,
    decisions: &[RowDecision],
    cancellation: &CancellationToken,
) -> Result<PeopleImportPreview, CsvError> {
    check_cancelled(cancellation)?;
    validate_mapping(mapping)?;
    binding::validate_decisions(decisions, mapping.has_header)?;
    let (schemas, fields) = WorkforceSchemas::load_with_person_fields()
        .map_err(|_| CsvError::source(CsvErrorCode::InvalidCurrentDocument))?;
    validate_document_with_schemas(document, &schemas, None)
        .map_err(|_| CsvError::source(CsvErrorCode::InvalidCurrentDocument))?;
    check_cancelled(cancellation)?;
    let mut rows = rows::RowReview::new(document, mapping, decisions, schemas, fields)?;
    let source = scan_csv(input, mapping.dialect, cancellation, |record| {
        check_cancelled(cancellation)?;
        rows.visit(record)?;
        check_cancelled(cancellation)
    })?;
    if decisions
        .iter()
        .any(|decision| decision.record > source.logical_records)
    {
        return Err(CsvError::source(CsvErrorCode::InvalidDecision));
    }
    let (outcomes, rejected_rows, mut batch, mut blocked) = rows.finish();
    let mut validation_issues: Vec<_> = rejected_rows.iter().copied().map(rejected_issue).collect();
    if let Some(proposed) = &batch {
        // Input envelope limits are operation failures, not invalid proposed state.
        proposed
            .validate_bounds()
            .map_err(|_| CsvError::source(CsvErrorCode::InvalidBatch))?;
        match validate_proposed_state(document, proposed, cancellation) {
            Ok(_) => {}
            Err(DomainPackError::InvalidPayload { .. }) => {
                validation_issues.push(ValidationIssue {
                    code: "invalidProposedState".to_owned(),
                    severity: ValidationSeverity::Error,
                    message:
                        "The proposed import violates scenario structure, references or capacity."
                            .to_owned(),
                    field_path: Some("/peopleCsv/proposedState".to_owned()),
                    resource: None,
                });
                blocked = true;
                batch = None;
            }
            Err(DomainPackError::Cancelled) => {
                return Err(CsvError::source(CsvErrorCode::Cancelled));
            }
            Err(_) => return Err(CsvError::source(CsvErrorCode::InvalidBatch)),
        }
    }
    if validation_issues.len() > MAX_CSV_VALIDATION_ISSUES {
        return Err(CsvError::source(CsvErrorCode::ValidationReportLimit));
    }
    bounded_json_size(&validation_issues, MAX_CSV_VALIDATION_BYTES)
        .map_err(|_| CsvError::source(CsvErrorCode::ValidationReportLimit))?;
    let disposition = if blocked {
        PeopleImportDisposition::Blocked
    } else if batch.is_some() {
        PeopleImportDisposition::Reviewable
    } else {
        PeopleImportDisposition::NoChanges
    };
    check_cancelled(cancellation)?;
    let review = if blocked {
        None
    } else {
        Some(PeopleCsvReview {
            format: REVIEW_FORMAT.to_owned(),
            schema_version: 1,
            scenario_id: document.scenario_id,
            revision,
            scenario_blake3: binding::hash(document)?,
            source: source.clone(),
            parser_version: CSV_PARSER_VERSION.to_owned(),
            limits_version: CSV_LIMITS_VERSION,
            mapping: mapping.clone(),
            decisions: decisions.to_vec(),
            changes_blake3: binding::hash(&batch)?,
            rejected_blake3: binding::hash(&rejected_rows)?,
            validation_blake3: binding::hash(&validation_issues)?,
        })
    };
    let approval_digest = review.as_ref().map(binding::approval_digest).transpose()?;
    let preview = PeopleImportPreview {
        schema_version: 1,
        disposition,
        source,
        columns: mapping.columns.clone(),
        rows: outcomes,
        rejected_rows,
        validation_issues,
        batch,
        review,
        approval_digest,
    };
    bounded_json_size(&preview, MAX_CSV_PREVIEW_BYTES)
        .map_err(|_| CsvError::source(CsvErrorCode::PreviewLimit))?;
    check_cancelled(cancellation)?;
    Ok(preview)
}

fn validate_proposed_state(
    document: &ScenarioDocument,
    batch: &DomainBatchCommand,
    cancellation: &CancellationToken,
) -> Result<ScenarioDocument, DomainPackError> {
    if cancellation.is_cancelled() {
        return Err(DomainPackError::Cancelled);
    }
    match commands::apply_batch_cancellable(document, batch, cancellation) {
        Ok(mutation) => Ok(mutation.document),
        Err(DomainPackError::BatchInverseTooLarge) if batch.commands.len() > 1 => {
            // The forward preview is already bounded. Only inverse amplification needs
            // smaller private applications; retain the original batch for review binding.
            let (left, right) = batch.commands.split_at(batch.commands.len() / 2);
            let mut part = DomainBatchCommand {
                schema_version: batch.schema_version,
                pack_id: batch.pack_id.clone(),
                scenario_schema_version: batch.scenario_schema_version,
                label: batch.label.clone(),
                commands: left.to_vec(),
            };
            let working = validate_proposed_state(document, &part, cancellation)?;
            part.commands = right.to_vec();
            validate_proposed_state(&working, &part, cancellation)
        }
        Err(error) => Err(error),
    }
}

fn rejected_issue(row: RejectedRow) -> ValidationIssue {
    let code = match row.code {
        RowRejectionCode::ColumnCount => "columnCount",
        RowRejectionCode::InvalidCell => "invalidCell",
        RowRejectionCode::InvalidReference => "invalidReference",
        RowRejectionCode::MissingName => "missingName",
        RowRejectionCode::InvalidPerson => "invalidPerson",
    };
    ValidationIssue {
        code: code.to_owned(),
        severity: ValidationSeverity::Warning,
        message: "The row was rejected and is excluded from the proposed import.".to_owned(),
        field_path: Some(format!("/peopleCsv/records/{}", row.record)),
        resource: None,
    }
}

/// Recomputes an approved review against fresh bytes and the actual current document.
/// The approved digest must come from the caller's prior explicit review, not the artifact.
///
/// # Errors
///
/// Rejects unsupported/malformed review data, altered approval bindings, stale
/// scenario/source state, fatal preview failures and cancellation.
pub fn rebuild_review<R: Read + ?Sized>(
    document: &ScenarioDocument,
    revision: Revision,
    input: &mut R,
    review: &PeopleCsvReview,
    approved_digest: &str,
    cancellation: &CancellationToken,
) -> Result<PeopleImportPreview, CsvError> {
    check_cancelled(cancellation)?;
    if binding::approval_digest(review)? != approved_digest
        || document.scenario_id != review.scenario_id
        || revision != review.revision
    {
        return Err(CsvError::source(CsvErrorCode::StaleReview));
    }
    let preview = preview_people_csv(
        document,
        revision,
        input,
        &review.mapping,
        &review.decisions,
        cancellation,
    )?;
    if preview.review.as_ref() != Some(review)
        || preview.approval_digest.as_deref() != Some(approved_digest)
    {
        return Err(CsvError::source(CsvErrorCode::StaleReview));
    }
    check_cancelled(cancellation)?;
    Ok(preview)
}

/// Decodes only bounded review data in the representation produced by this version.
/// JSON whitespace and object-member order do not affect this comparison.
///
/// # Errors
///
/// Rejects oversized/deep/unsafe JSON, unknown fields/versions, duplicate keys,
/// and alternate typed representations that differ from the emitted contract.
pub fn decode_people_csv_review(bytes: &[u8]) -> Result<PeopleCsvReview, CsvError> {
    binding::decode_review(bytes)
}
