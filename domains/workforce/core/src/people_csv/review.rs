mod binding;
mod rows;

use super::{
    mapping::validate_mapping,
    parsing::scan_csv,
    types::{
        CSV_LIMITS_VERSION, CSV_PARSER_VERSION, ColumnMapping, CsvError, CsvErrorCode, CsvSource,
        MAX_CSV_PREVIEW_BYTES, PeopleCsvMapping, RejectedRow, RowDecision, RowRejectionCode,
    },
};
use crate::{
    commands,
    validation::{WorkforceSchemas, validate_document_with_schemas},
};
use eutheto_domain_api::{DomainBatchCommand, DomainPackError, bounded_json_size};
use eutheto_types::{CancellationToken, Revision, ScenarioDocument, ScenarioId};
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
/// Fatal input/policy/resource failures return an error. Unresolved identities and
/// duplicate targets return a blocked preview without a batch or approval artifact.
///
/// # Errors
///
/// Rejects invalid current state, source/policy limits, invalid decisions or an
/// invalid aggregate batch. Cancellation never returns an approved batch.
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
    validate_document_with_schemas(document, &schemas)
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
    let (outcomes, rejected_rows, batch, blocked) = rows.finish();
    let disposition = if blocked {
        PeopleImportDisposition::Blocked
    } else if batch.is_some() {
        PeopleImportDisposition::Reviewable
    } else {
        PeopleImportDisposition::NoChanges
    };
    if let Some(batch) = &batch {
        commands::apply_batch_cancellable(document, batch, cancellation).map_err(|error| {
            CsvError::source(if error == DomainPackError::Cancelled {
                CsvErrorCode::Cancelled
            } else {
                CsvErrorCode::InvalidBatch
            })
        })?;
    }
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
        batch,
        review,
        approval_digest,
    };
    bounded_json_size(&preview, MAX_CSV_PREVIEW_BYTES)
        .map_err(|_| CsvError::source(CsvErrorCode::PreviewLimit))?;
    check_cancelled(cancellation)?;
    Ok(preview)
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
