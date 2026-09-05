//! Typed stored values and explicit temporal intent.
//!
//! These shapes do not imply solve readiness or implemented rule evaluation.
// Fieldless tagged variants must be empty structs: Serde's tagged unit variants
// discard unknown parameters despite deny_unknown_fields.

mod domain;
mod entities;
mod preferences;
mod rules;
mod scope;
mod scoring;
mod time;

pub use domain::*;
pub use entities::*;
pub use preferences::*;
pub use rules::*;
pub use scope::*;
pub use scoring::*;
pub use time::*;

/// Missing fields use `None`; a present field must contain a real value, not null.
fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
