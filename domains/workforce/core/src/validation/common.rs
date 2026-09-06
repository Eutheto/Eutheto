use super::{MAX_DISPLAY_BYTES, MAX_REFERENCE_ITEMS, MAX_TOKEN_BYTES, MAX_WEIGHT};
use eutheto_domain_api::DomainPackError;
use std::{collections::BTreeSet, fmt::Display};

pub(crate) type Result<T = ()> = std::result::Result<T, DomainPackError>;

pub(crate) fn invalid(path: &str, message: &str) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

pub(crate) fn require(condition: bool, path: &str, message: &str) -> Result {
    if condition {
        Ok(())
    } else {
        Err(invalid(path, message))
    }
}

pub(super) fn record<T>(result: Result<T>, map: &str, id: impl Display) -> Result<T> {
    result.map_err(|error| match error {
        DomainPackError::InvalidPayload { path, message } => DomainPackError::InvalidPayload {
            path: format!("domain.{map}.{id}.{path}"),
            message,
        },
        other => other,
    })
}

pub(super) fn text(value: &str, path: &str) -> Result {
    require(
        value.len() <= MAX_DISPLAY_BYTES
            && !value.trim().is_empty()
            && !value.chars().any(char::is_control),
        path,
        "expected bounded nonempty display text",
    )
}

pub(super) fn note(value: &str, path: &str) -> Result {
    require(
        value.len() <= 4096
            && !value
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r')),
        path,
        "text exceeds its bound or contains binary controls",
    )
}

pub(super) fn token(value: &str, path: &str) -> Result {
    require(
        !value.is_empty()
            && value.len() <= MAX_TOKEN_BYTES
            && value.as_bytes()[0].is_ascii_lowercase()
            && value.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            }),
        path,
        "expected a bounded lowercase ASCII semantic key",
    )
}

pub(super) fn weight(value: u32, path: &str) -> Result {
    require(
        value > 0 && value <= MAX_WEIGHT,
        path,
        "weight must be positive and within its bound",
    )
}

pub(super) fn bounded<T>(values: &[T], active: bool, path: &str) -> Result {
    require(
        values.len() <= MAX_REFERENCE_ITEMS && (!active || !values.is_empty()),
        path,
        "selection exceeds its bound or is empty while active",
    )
}

pub(super) fn unique<T: Ord>(values: &[T], active: bool, path: &str) -> Result {
    bounded(values, active, path)?;
    let mut seen = BTreeSet::new();
    require(
        values.iter().all(|value| seen.insert(value)),
        path,
        "duplicate selection value",
    )
}

pub(super) fn tags(values: &[String], path: &str) -> Result {
    bounded(values, false, path)?;
    for value in values {
        text(value, path)?;
    }
    unique(values, false, path)
}
