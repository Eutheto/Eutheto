//! Structural validation, separate from feasibility and backend support.

pub(crate) mod common;
mod context;
mod entities;
mod ingress;
mod preferences;
mod rules;
mod schemas;
mod scope;
mod score;
mod time;
mod workload;

pub use score::{
    MAX_DISPLAY_BYTES, MAX_PENALTY_BREAKPOINTS, MAX_POLICY_DEFINITIONS, MAX_REFERENCE_ITEMS,
    MAX_TOKEN_BYTES, MAX_WEIGHT, validate_score_policy_shape,
};

pub use context::{MAX_DOCUMENT_OCCURRENCES, MAX_TEMPLATE_OCCURRENCES};
pub use ingress::{decode_document, validate_document};
pub(crate) use ingress::{validate_document_with_schemas, validate_value_bounds};
pub(crate) use schemas::WorkforceSchemas;
