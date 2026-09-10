//! Bounded Workforce compilation, projection and independent original-domain authority.
//! These APIs do not register a pack; accepted results still pass the generic acceptance reviewer.

pub(crate) mod analysis;
mod authority;
mod boundary;
pub(crate) mod budget;
mod compiler;
mod compiler_model;
mod compiler_rank;
mod evaluation;
mod evidence;
mod identity;
pub(crate) mod input;
pub(crate) mod intervals;
mod ir_cost;
mod projection;
mod sharing;
mod types;
pub use analysis::analyze_assignments;
pub(crate) use authority::{operation_error, validate_input_bounds};
pub use authority::{
    score_workforce_solution, verify_workforce_solution, workforce_verification_scope,
};
pub use budget::{MAX_EXPANDED_INTERVALS, MAX_INSPECTED_PAIRS, MAX_SELECTED_PAIRS, MAX_WORK_STEPS};
pub use compiler::compile_assignment_rules;
pub use compiler_model::compile_workforce;
pub use evaluation::evaluate_assignment_rules;
pub use evidence::render_workforce_evidence;
pub(crate) use evidence::validate_workforce_full;
pub use projection::{decode_workforce_assignment, project_workforce_candidate};
pub use sharing::build_workforce_share_result;
pub use types::*;

#[cfg(test)]
mod shared_tests;
