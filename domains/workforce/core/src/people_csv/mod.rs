//! People CSV contracts for bounded, explicit review outside application authority.

mod mapping;
mod parsing;
mod review;
mod types;
use eutheto_types::{EntityId, TypedUuidError};
use serde::Serialize;
use std::{collections::BTreeMap, str::FromStr};

pub use parsing::{detect_people_csv, sample_people_csv_record};
pub(crate) use parsing::{sample_record, scan_csv};
pub use review::*;
pub(crate) use types::CsvRecord;
pub use types::*;
/// Compact typed serialization with ordered maps matches existing Workforce hashes.
/// Callers preflight recursive Values and their complete serialized byte budget.
pub(super) fn hash<T: Serialize + ?Sized>(value: &T) -> Result<String, CsvError> {
    let mut hasher = blake3::Hasher::new();
    serde_json::to_writer(&mut hasher, value)
        .map_err(|_| CsvError::source(CsvErrorCode::InvalidReview))?;
    Ok(hasher.finalize().to_hex().to_string())
}

pub(super) fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn valid_identity_alias(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_CSV_INSPECTED_CELL_BYTES
        && !token.chars().any(char::is_control)
        && matches!(
            token.parse::<EntityId>(),
            Err(TypedUuidError::InvalidUuid(_))
        )
}

/// Parsed identities cannot be redirected by an alias, including invalid UUID versions.
pub(crate) fn resolve_identity<T: Copy + FromStr<Err = TypedUuidError>, E>(
    token: &str,
    aliases: &BTreeMap<String, T>,
    exists: impl FnOnce(T) -> bool,
    invalid_token: E,
    missing: E,
) -> (Option<T>, Option<E>) {
    if token.is_empty()
        || token.len() > MAX_CSV_INSPECTED_CELL_BYTES
        || token.chars().any(char::is_control)
    {
        return (None, Some(invalid_token));
    }
    let id = match token.parse::<T>() {
        Ok(id) => Some(id),
        Err(TypedUuidError::InvalidUuid(_)) => aliases.get(token).copied(),
        Err(_) => return (None, Some(invalid_token)),
    };
    match id {
        Some(id) if exists(id) => (Some(id), None),
        _ => (None, Some(missing)),
    }
}
