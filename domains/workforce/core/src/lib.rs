//! Workforce domain values, compilation, projection and independent original-domain authority.
//!
//! The application explicitly registers the complete [`WorkforcePack`]. This crate owns no
//! persistence, approval custody or client lifecycle.

pub mod assignment_import;
pub mod assignment_rules;
pub mod assignments_csv;
pub mod assignments_ics;
pub mod availability_import;
pub mod commands;
pub mod eligibility_import;
pub mod export_source;
pub mod generated_workforce_pack_contract;
pub mod ids;
pub mod model;
mod pack;
pub mod people_csv;
pub mod portable;
mod result_view;
pub mod setup;
pub mod shift_import;
pub mod summary_csv;
pub mod temporal;
pub mod validation;

pub use pack::WorkforcePack;
pub use result_view::recipient_report;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;
