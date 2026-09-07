//! Workforce domain values, compilation, projection and independent original-domain authority.
//!
//! The real [`WorkforcePack`] is not registered in production. Host lifecycle integration and
//! production registration remain separate phase gates.

pub mod assignment_rules;
pub mod commands;
pub mod generated_workforce_pack_contract;
pub mod ids;
pub mod model;
mod pack;
pub mod people_csv;
pub mod portable;
pub mod temporal;
pub mod validation;

pub use pack::WorkforcePack;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;
