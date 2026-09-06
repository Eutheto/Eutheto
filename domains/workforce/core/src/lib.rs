//! Workforce domain values, independent of application and solver authority.
//!
//! This crate is not registered as a production domain pack. Registration requires
//! the complete Workforce compiler, independent verifier, and result contracts.

pub mod assignment_rules;
pub mod commands;
pub mod generated_workforce_pack_contract;
pub mod ids;
pub mod model;
pub mod people_csv;
pub mod portable;
pub mod temporal;
pub mod validation;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;
