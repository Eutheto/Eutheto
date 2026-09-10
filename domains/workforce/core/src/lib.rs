//! Workforce domain values, compilation, projection and independent original-domain authority.
//!
//! The application explicitly registers the complete [`WorkforcePack`]. This crate owns no
//! persistence, approval custody or client lifecycle.

pub mod assignment_rules;
pub mod assignments_csv;
pub mod commands;
pub mod generated_workforce_pack_contract;
pub mod ids;
pub mod model;
mod pack;
pub mod people_csv;
pub mod portable;
pub mod setup;
pub mod temporal;
pub mod validation;

pub use pack::WorkforcePack;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;
