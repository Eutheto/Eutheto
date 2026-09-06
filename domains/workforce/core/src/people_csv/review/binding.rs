use super::{PeopleCsvReview, REVIEW_FORMAT};
use crate::people_csv::{
    mapping::validate_mapping,
    types::{
        CSV_LIMITS_VERSION, CSV_PARSER_VERSION, CsvError, CsvErrorCode, IdentityDecision,
        MAX_CSV_DATA_RECORDS, MAX_CSV_DECISION_BYTES, MAX_CSV_DECISIONS, MAX_CSV_LOGICAL_RECORDS,
        MAX_CSV_REVIEW_BYTES, MAX_CSV_SOURCE_BYTES, RowDecision,
    },
};
use crate::validation::validate_value_bounds;
use eutheto_domain_api::{ContractJsonLimits, bounded_json_size};
use eutheto_types::{PortableJsonLimits, validate_nonsecret_portable_json_bytes};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

/// Compact typed serialization with ordered maps matches existing Workforce hashes.
/// Callers preflight recursive Values and their complete serialized byte budget.
pub(super) fn hash<T: Serialize + ?Sized>(value: &T) -> Result<String, CsvError> {
    let mut hasher = blake3::Hasher::new();
    serde_json::to_writer(&mut hasher, value)
        .map_err(|_| CsvError::source(CsvErrorCode::InvalidReview))?;
    Ok(hasher.finalize().to_hex().to_string())
}

pub(super) fn validate_decisions(
    decisions: &[RowDecision],
    has_header: bool,
) -> Result<(), CsvError> {
    if decisions.len() > MAX_CSV_DECISIONS {
        return Err(CsvError::source(CsvErrorCode::DecisionLimit));
    }
    bounded_json_size(decisions, MAX_CSV_DECISION_BYTES)
        .map_err(|_| CsvError::source(CsvErrorCode::DecisionLimit))?;
    let mut records = BTreeSet::new();
    for decision in decisions {
        if decision.record == 0
            || decision.record > MAX_CSV_LOGICAL_RECORDS
            || (has_header && decision.record == 1)
            || !records.insert(decision.record)
        {
            return Err(CsvError::at(CsvErrorCode::InvalidDecision, decision.record));
        }
        if let IdentityDecision::Add { person_id } | IdentityDecision::Update { person_id } =
            decision.decision
            && person_id.as_uuid().get_version_num() != 7
        {
            return Err(CsvError::at(CsvErrorCode::InvalidDecision, decision.record));
        }
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validated_review_value(review: &PeopleCsvReview) -> Result<Value, CsvError> {
    if review.format != REVIEW_FORMAT
        || review.schema_version != 1
        || review.parser_version != CSV_PARSER_VERSION
        || review.limits_version != CSV_LIMITS_VERSION
    {
        return Err(CsvError::source(CsvErrorCode::UnsupportedVersion));
    }
    if review.scenario_id.as_uuid().get_version_num() != 7
        || !valid_digest(&review.scenario_blake3)
        || !valid_digest(&review.source.blake3)
        || !valid_digest(&review.changes_blake3)
        || !valid_digest(&review.rejected_blake3)
        || usize::try_from(review.source.raw_bytes)
            .map_or(true, |bytes| bytes > MAX_CSV_SOURCE_BYTES)
        || review.source.logical_records > MAX_CSV_LOGICAL_RECORDS
        || review
            .source
            .logical_records
            .saturating_sub(u32::from(review.mapping.has_header))
            > MAX_CSV_DATA_RECORDS
    {
        return Err(CsvError::source(CsvErrorCode::InvalidReview));
    }
    validate_mapping(&review.mapping)?;
    validate_decisions(&review.decisions, review.mapping.has_header)?;
    if review
        .decisions
        .iter()
        .any(|decision| decision.record > review.source.logical_records)
    {
        return Err(CsvError::source(CsvErrorCode::InvalidDecision));
    }
    bounded_json_size(review, MAX_CSV_REVIEW_BYTES)
        .map_err(|_| CsvError::source(CsvErrorCode::ReviewLimit))?;
    // Fixed-depth, count/string/byte-bounded types make this materialization safe.
    // Use the same portable safety/aggregate limits as the byte decoder, so a
    // producer can never approve an artifact that its own decoder will reject.
    let value =
        serde_json::to_value(review).map_err(|_| CsvError::source(CsvErrorCode::InvalidReview))?;
    validate_value_bounds(&value, "/peopleCsvReview")
        .map_err(|_| CsvError::source(CsvErrorCode::InvalidReview))?;
    Ok(value)
}

pub(super) fn approval_digest(review: &PeopleCsvReview) -> Result<String, CsvError> {
    #[derive(Serialize)]
    struct Approval<'a> {
        domain: &'static str,
        version: u32,
        review: &'a Value,
    }
    let value = validated_review_value(review)?;
    hash(&Approval {
        domain: "eutheto/workforce-people-csv-approval",
        version: 1,
        review: &value,
    })
}

pub(super) fn decode_review(bytes: &[u8]) -> Result<PeopleCsvReview, CsvError> {
    if bytes.len() > MAX_CSV_REVIEW_BYTES {
        return Err(CsvError::source(CsvErrorCode::ReviewLimit));
    }
    let limits = PortableJsonLimits {
        max_depth: ContractJsonLimits::DEFAULT.max_depth,
        max_string_bytes: ContractJsonLimits::DEFAULT.max_string_bytes,
        max_collection_items: ContractJsonLimits::DEFAULT.max_collection_items,
    };
    validate_nonsecret_portable_json_bytes(bytes, &limits)
        .map_err(|_| CsvError::source(CsvErrorCode::InvalidReview))?;
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|_| CsvError::source(CsvErrorCode::InvalidReview))?;
    let review: PeopleCsvReview = serde::Deserialize::deserialize(&raw)
        .map_err(|_| CsvError::source(CsvErrorCode::InvalidReview))?;
    if validated_review_value(&review)? != raw {
        return Err(CsvError::source(CsvErrorCode::InvalidReview));
    }
    Ok(review)
}
