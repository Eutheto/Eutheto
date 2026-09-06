//! Bounded operation-local assignment-rule mathematics and original-domain evidence.
//! These contributions do not register a pack, accept a solution, or establish an authoritative score.

mod analysis;
mod budget;
mod compiler;
mod evaluation;
mod identity;
mod input;
mod intervals;
mod types;
pub use analysis::analyze_assignments;
pub use budget::{MAX_EXPANDED_INTERVALS, MAX_INSPECTED_PAIRS, MAX_SELECTED_PAIRS, MAX_WORK_STEPS};
pub use compiler::compile_assignment_rules;
pub use evaluation::evaluate_assignment_rules;
pub use types::*;

#[cfg(test)]
mod shared_tests;
