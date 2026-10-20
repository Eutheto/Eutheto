//! Borrowed accepted sources for Workforce CSV/calendar projections.
//!
//! This is not acceptance authority: callers must independently verify the complete original
//! source before encoding. Checkpoints retain their real input/result without a terminal manifest.

use crate::generated_workforce_pack_contract::WORKFORCE_PACK_ID;
use eutheto_domain_ir::{
    AcceptedResult, PortableAcceptedResult, RunInput, RunTerminalOutcomeV1, VerifiedCheckpoint,
};
use eutheto_types::{ScenarioDocument, SolveStatus};

/// An immutable accepted terminal result or a verified, nonterminal checkpoint.
#[derive(Clone, Copy, Debug)]
pub enum WorkforceExportSource<'a> {
    /// A genuine portable result with its terminal acceptance outcome.
    Terminal(&'a PortableAcceptedResult),
    /// A still-running accepted checkpoint, never a claim of terminality or optimality.
    Checkpoint(&'a VerifiedCheckpoint),
}

impl<'a> WorkforceExportSource<'a> {
    pub(crate) fn run_input(self) -> &'a RunInput {
        match self {
            Self::Terminal(portable) => &portable.run_input,
            Self::Checkpoint(checkpoint) => &checkpoint.run_input,
        }
    }

    pub(crate) fn accepted_result(self) -> &'a AcceptedResult {
        match self {
            Self::Terminal(portable) => &portable.accepted_result,
            Self::Checkpoint(checkpoint) => &checkpoint.accepted_result,
        }
    }

    /// Checks projection bindings and returns the existing truthful calendar status label.
    pub(crate) fn validate_binding(self, document: &ScenarioDocument) -> Result<&'static str, ()> {
        let input = self.run_input();
        let accepted = self.accepted_result();
        if input.scenario_id != document.scenario_id
            || accepted.solution.scenario_id != document.scenario_id
            || accepted.solution.pack_id.as_str() != WORKFORCE_PACK_ID
            || input.scenario_timezone != document.settings.time_zone
            || input.scenario_revision != accepted.solution.scenario_revision
        {
            return Err(());
        }
        match self {
            Self::Terminal(portable) => {
                if portable.scenario_id != document.scenario_id
                    || portable.result_id != accepted.solution.solution_id
                    || portable.scenario_revision != input.scenario_revision
                {
                    return Err(());
                }
                let RunTerminalOutcomeV1::Accepted {
                    status,
                    solution_id,
                    accepted_result_checksum,
                    verification_checksum,
                } = &portable.run_manifest.outcome
                else {
                    return Err(());
                };
                if *solution_id != portable.result_id
                    || accepted_result_checksum != &accepted.checksum
                    || verification_checksum != &accepted.verification.checksum
                {
                    return Err(());
                }
                match status {
                    // Independent verification proves feasibility, not historical optimality.
                    SolveStatus::Optimal => Ok("accepted"),
                    SolveStatus::Feasible => Ok("feasible"),
                    _ => Err(()),
                }
            }
            Self::Checkpoint(checkpoint) => {
                checkpoint.validate().map_err(|_| ())?;
                Ok("feasible")
            }
        }
    }
}
