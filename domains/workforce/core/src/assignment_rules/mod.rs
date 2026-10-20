//! Bounded Workforce compilation, projection and independent original-domain authority.
//! These APIs do not register a pack; accepted results still pass the generic acceptance reviewer.

mod alternative_outcomes;
pub(crate) mod analysis;
mod authority;
pub(crate) mod baseline;
mod boundary;
pub(crate) mod budget;
mod comparison;
mod compiler;
pub(crate) mod compiler_diagnostic;
mod compiler_model;
mod compiler_rank;
mod compiler_weekend;
mod counterfactual;
mod evaluation;
mod evidence;
mod identity;
pub(crate) mod input;
pub(crate) mod intervals;
mod ir_cost;
mod preference;
mod preference_consecutive;
mod profile;
mod projection;
mod reporting;
mod sharing;
mod types;
mod weekends;
pub(crate) fn validate_workforce_objective_snapshot(
    domain: &crate::model::WorkforceDomainV1,
    captured: Option<&crate::model::BaseSchedule>,
    scenario_id: eutheto_types::ScenarioId,
    objective_context: Option<&eutheto_domain_ir::RunObjectiveContext>,
    budget: &mut budget::OperationBudget<'_>,
) -> Result<(), eutheto_domain_api::DomainPackError> {
    baseline::selected_baseline_pairs(domain, captured, scenario_id, objective_context, budget)
        .map(|_| ())
}

pub use alternative_outcomes::WorkforceAlternativeOutcomes;
pub use analysis::analyze_assignments;
pub(crate) use authority::{
    check_objective_context, operation_error, validate_input_bounds,
    workforce_diagnostic_verification_scope,
};
pub use authority::{
    evaluate_workforce_required_selection, score_workforce_solution, verify_workforce_solution,
    workforce_verification_scope,
};
pub use baseline::{
    clear_workforce_manual_after_base_change, normalize_workforce_manual_selection_copy,
    workforce_baseline_pairs, workforce_baseline_reference, workforce_external_baseline_reference,
    workforce_manual_selection_checksum, workforce_pair_content_hash,
};
pub use budget::WorkforceAdmissionBudget;
pub use budget::{MAX_EXPANDED_INTERVALS, MAX_INSPECTED_PAIRS, MAX_SELECTED_PAIRS, MAX_WORK_STEPS};
pub use comparison::workforce_comparison_enrichment;
pub use compiler::compile_assignment_rules;
pub(crate) use compiler_diagnostic::compile_infeasibility;
pub use compiler_model::compile_workforce;
pub(crate) use counterfactual::compile_counterfactual;
pub use evaluation::evaluate_assignment_rules;
pub(crate) use evaluation::evaluate_assignment_rules_with_health;
pub use evidence::render_workforce_evidence;
pub(crate) use evidence::render_workforce_repair;
pub(crate) use evidence::validate_workforce_full;
pub(crate) use identity::WORKFORCE_PROJECTION_VERSION;
pub(crate) use projection::decode_workforce_assignment_id;
pub use projection::{decode_workforce_assignment, project_workforce_candidate};
pub use sharing::build_workforce_share_result;
pub(crate) use sharing::recipient_source;
pub use types::*;

#[cfg(test)]
mod shared_tests;
