//! Bounded, native-only infeasibility enrichment.
//!
//! This module deliberately sits below ordinary routing and acceptance.  It owns one guarded
//! diagnostic model, awaits exactly the selected backend, and returns inert domain material.  The
//! caller owns the original infeasibility observation and the final manifest binding.

use eutheto_domain_api::{
    CompileContext, DomainCapability, DomainPack, DomainPackError, InfeasibilityCompileContext,
};
use eutheto_domain_ir::{
    AssumptionGroupId, ConflictGroupV1, ConflictUnavailableReason, CounterfactualConditionV1,
    DiagnosticInfeasibilityObservationV1, DiagnosticInfeasibilityProofV1,
    DiagnosticNativeProfileV1, DiagnosticSolveBindingV1, DiagnosticSubmittedAssumptionV1,
    ExplanationCapability, InfeasibilityEvidenceV1,
};
use eutheto_explain::{
    ConflictShrinker, ConflictTrialOutcome, MappedAssumptionGroups,
    map_assumption_literals_with_budget,
};
use eutheto_planning_ir::{
    Assumption, Literal, PlanningIrLimitsV1, PlanningPreparationBudget, PlanningPreparationError,
    PlanningProblem, PreparedPlanningModel,
};
use eutheto_solver_api::{
    BackendAssumptionCoreEvidence, BackendCandidate, BackendSolveOutcome, BackendTerminationReason,
    BoundedBackendOutput, BoundedOutputAdmissionError, CompatibilityLevel, OutcomeError,
    OutputError, ProgressSink, SolveRequest, SolverApiLimits, SolverBackend, validate_outcome,
};
use eutheto_types::{
    AppError, BackendSelection, DurationMillis, ExplanationMode, IdGenerator, OperationControl,
    OperationInterruption, ParentSolveBudget, ProtocolFailure, ScenarioDocument, SolveBudgetView,
    SolveOptions, WorkerThreadPolicy,
};
use eutheto_verify::{
    AcceptanceReviewer, CorrectnessAlarm, CorrectnessAlarmCategory, DiagnosticDecision,
    DiagnosticReviewerError, VerificationClock,
};
use std::collections::BTreeSet;
use std::mem::size_of;
use std::sync::Arc;
use std::time::Duration;

const ORTOOLS_BACKEND_ID: &str = "solver.ortools-cp-sat";
const DIAGNOSTIC_PHASE_LIMIT: Duration = Duration::from_secs(10);
const DIAGNOSTIC_DISPATCH_LIMIT_MILLISECONDS: u64 = 2_000;
const MAX_DIAGNOSTIC_GROUPS: usize = 512;
const SHRINK_THRESHOLD: usize = 16;
const MAX_SHRINK_TRIALS: u32 = 32;

/// Inputs for one optional infeasibility diagnostic phase.
///
/// The source model and document are immutable.  `backend` is already the selected backend from
/// the ordinary solve; this driver never performs ordinary routing or fallback.
pub(crate) struct InfeasibilityDiagnosticContext<'a> {
    pub(crate) pack: &'a dyn DomainPack,
    pub(crate) document: &'a ScenarioDocument,
    pub(crate) base_problem: &'a PlanningProblem,
    pub(crate) compile_context: &'a CompileContext,
    pub(crate) backend: &'a dyn SolverBackend,
    pub(crate) parent_budget: &'a ParentSolveBudget,
    pub(crate) verification_clock: &'a dyn VerificationClock,
    pub(crate) ids: &'a dyn IdGenerator,
    pub(crate) options: &'a SolveOptions,
    pub(crate) condition: Option<&'a CounterfactualConditionV1>,
}

/// Inert diagnostic material.  The caller binds it to the exact final run manifest.
// This bounded 392-byte inert outcome stays inline to avoid a heap allocation solely for enum
// layout at the authority handoff.
#[allow(clippy::large_enum_variant)]
pub(crate) enum InfeasibilityDiagnosticMaterial {
    Evidence {
        infeasibility: InfeasibilityEvidenceV1,
        proof: Option<DiagnosticInfeasibilityProofV1>,
    },
    Quarantined {
        alarm: CorrectnessAlarm,
    },
}

/// Runs the optional diagnostic phase without ordinary candidate acceptance.
///
/// No original observation is captured here and no final run evidence is constructed here.  The
/// caller binds the returned material to the original observation and exact final manifest.
// Keep source-scope admission, the selected native attempt, and its original-core proof/trial
// transitions together so cancellation and proof provenance cannot drift between stages.
#[allow(clippy::too_many_lines)]
pub(crate) async fn diagnose_infeasibility(
    context: InfeasibilityDiagnosticContext<'_>,
) -> Result<InfeasibilityDiagnosticMaterial, AppError> {
    if context.options.explanation_mode == ExplanationMode::None {
        return Ok(unavailable(ConflictUnavailableReason::NotRequested));
    }

    let mut clock = ClockGuard::default();
    let phase = context
        .parent_budget
        .phase_view()
        .capped_phase_view(DIAGNOSTIC_PHASE_LIMIT);
    let control = OperationControl::Solve(phase.clone());
    let mut preparation =
        PlanningPreparationBudget::new(control.clone(), context.compile_context.planning_limits);
    let diagnostic_started = observe_parent_elapsed(&context, &mut clock, &control)?;

    if let Some(material) = interruption_material(&control) {
        return Ok(material);
    }
    if !pack_advertises_diagnostics(context.pack) {
        return Ok(unavailable(
            ConflictUnavailableReason::UnsupportedDomainPack,
        ));
    }
    if !backend_advertises_native_diagnostics(context.backend) {
        return Ok(unavailable(
            ConflictUnavailableReason::UnsupportedDomainPack,
        ));
    }

    // This source classification is intentionally before any diagnostic compiler allocation.
    let source_scope = match context.pack.diagnostic_verification_scope(
        context.document,
        context.compile_context.scenario_revision,
        &control,
    ) {
        Ok(scope) => scope,
        Err(error) => return Ok(pack_error_material(&error)),
    };
    observe_parent_elapsed(&context, &mut clock, &control)?;
    if let Some(material) = interruption_material(&control) {
        return Ok(material);
    }
    let source_rules = source_scope.relaxable_required_rules();
    if source_rules.len() > MAX_DIAGNOSTIC_GROUPS {
        return Ok(unavailable(
            ConflictUnavailableReason::ResourceLimitExceeded,
        ));
    }

    let mut diagnostic_compile_context = context.compile_context.clone();
    diagnostic_compile_context.control = control.clone();
    let compile_context = InfeasibilityCompileContext {
        base_problem: context.base_problem,
        compile_context: &diagnostic_compile_context,
        condition: context.condition,
        budget: phase.phase_view(),
    };
    let compiled = match context
        .pack
        .compile_infeasibility(context.document, &compile_context)
    {
        Ok(problem) => problem,
        Err(error) => return Ok(pack_error_material(&error)),
    };
    observe_parent_elapsed(&context, &mut clock, &control)?;
    if let Some(material) = interruption_material(&control) {
        return Ok(material);
    }

    let mut problem = Arc::new(compiled);
    if let Err(reason) = validate_diagnostic_problem_shape(
        &mut problem,
        source_rules,
        context.compile_context.planning_limits,
        &mut preparation,
    ) {
        return Ok(unavailable(reason));
    }

    let options = diagnostic_options(context.options, context.backend);
    if options.validate().is_err() {
        return Ok(unavailable(ConflictUnavailableReason::DiagnosticFailed));
    }

    let initial = execute_attempt(
        &problem,
        &options,
        &phase,
        diagnostic_started,
        &context,
        &mut preparation,
        &mut clock,
    )
    .await?;
    let initial = match initial {
        AttemptResult::Quarantined { alarm } => {
            return Ok(InfeasibilityDiagnosticMaterial::Quarantined { alarm });
        }
        AttemptResult::Feasible => {
            // A fully checked original witness is handled as Quarantined above.  A partial
            // initial witness cannot be turned into an accepted result or a conflict proof.
            return Ok(unavailable(ConflictUnavailableReason::DiagnosticFailed));
        }
        AttemptResult::Unavailable { reason } => return Ok(unavailable(reason)),
        AttemptResult::Infeasible {
            observation,
            core,
            core_reservation,
            binding_reservation,
        } => InitialAttempt {
            observation,
            core,
            _core_reservation: core_reservation,
            _binding_reservation: binding_reservation,
        },
    };
    if initial.observation.binding.submitted_assumptions.is_empty() {
        let proof = DiagnosticInfeasibilityProofV1 {
            diagnostic_started_after_milliseconds: diagnostic_started,
            initial: initial.observation,
            initial_core: Vec::new(),
            last_proven_infeasible: None,
            attempted_trials: 0,
            max_trials: 0,
        };
        return Ok(InfeasibilityDiagnosticMaterial::Evidence {
            infeasibility: InfeasibilityEvidenceV1::Unavailable {
                reason: ConflictUnavailableReason::FoundationalInfeasibility,
            },
            proof: Some(proof),
        });
    }

    let Some(initial_core) = initial.core else {
        return Ok(unavailable(
            ConflictUnavailableReason::InvalidAssumptionCore,
        ));
    };
    if initial_core.is_empty() {
        return Ok(unavailable(
            ConflictUnavailableReason::InvalidAssumptionCore,
        ));
    }

    let max_trials = if initial_core.len() <= SHRINK_THRESHOLD {
        0
    } else {
        MAX_SHRINK_TRIALS
    };
    if max_trials == 0 {
        let Ok(infeasibility) = ConflictShrinker::new(initial_core.clone(), 0, phase.phase_view())
            .and_then(ConflictShrinker::into_result)
        else {
            return Ok(unavailable(ConflictUnavailableReason::DiagnosticFailed));
        };
        let proof = DiagnosticInfeasibilityProofV1 {
            diagnostic_started_after_milliseconds: diagnostic_started,
            initial: initial.observation,
            initial_core,
            last_proven_infeasible: None,
            attempted_trials: 0,
            max_trials: 0,
        };
        return Ok(InfeasibilityDiagnosticMaterial::Evidence {
            infeasibility,
            proof: Some(proof),
        });
    }
    let Ok(mut shrinker) =
        ConflictShrinker::new(initial_core.clone(), max_trials, phase.phase_view())
    else {
        return Ok(unavailable(ConflictUnavailableReason::DiagnosticFailed));
    };
    // Only the initial native core enters deletion trials. Non-core assumptions are parked by
    // moving their owned objects; no rule/template graph is cloned.
    let parked_bytes = u64::try_from(problem.assumptions.len())
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::try_from(size_of::<Assumption>()).unwrap_or(u64::MAX));
    if let Err(_error) = preparation.reserve_transient(parked_bytes) {
        return Ok(initial_core_material(
            initial_core,
            initial.observation,
            diagnostic_started,
            &phase,
        ));
    }
    if let Err(_error) = preparation.check() {
        return Ok(initial_core_material(
            initial_core,
            initial.observation,
            diagnostic_started,
            &phase,
        ));
    }
    let mut parked_assumptions = Vec::with_capacity(problem.assumptions.len());
    if retain_assumptions_for_groups(
        &mut problem,
        &initial_core,
        &mut parked_assumptions,
        &mut preparation,
    )
    .is_err()
    {
        return Ok(initial_core_material(
            initial_core,
            initial.observation,
            diagnostic_started,
            &phase,
        ));
    }

    let mut attempted_trials = 0_u32;
    let mut last_proven_infeasible = None;
    let mut last_proven_reservations = None;
    loop {
        let Some(removed_group_id) = (match shrinker.next_trial() {
            Ok(Some(candidate_groups)) => {
                attempted_trials = attempted_trials.saturating_add(1);
                removed_group_id_from_model(&problem, candidate_groups)
            }
            Ok(None) => break,
            Err(_) => {
                return Ok(initial_core_material(
                    initial_core,
                    initial.observation,
                    diagnostic_started,
                    &phase,
                ));
            }
        }) else {
            return Ok(initial_core_material(
                initial_core,
                initial.observation,
                diagnostic_started,
                &phase,
            ));
        };

        let Some((removed_index, removed_assumption)) =
            remove_assumption_for_group(&mut problem, &removed_group_id)
        else {
            return Ok(initial_core_material(
                initial_core,
                initial.observation,
                diagnostic_started,
                &phase,
            ));
        };

        let trial = execute_attempt(
            &problem,
            &options,
            &phase,
            diagnostic_started,
            &context,
            &mut preparation,
            &mut clock,
        )
        .await?;
        let trial = match trial {
            AttemptResult::Quarantined { alarm } => {
                return Ok(InfeasibilityDiagnosticMaterial::Quarantined { alarm });
            }
            AttemptResult::Infeasible {
                observation,
                core_reservation,
                binding_reservation,
                ..
            } => TrialResult::Infeasible {
                observation,
                core_reservation,
                binding_reservation,
            },
            AttemptResult::Feasible => TrialResult::Feasible,
            AttemptResult::Unavailable { reason } => TrialResult::Unavailable(reason),
        };

        match trial {
            TrialResult::Feasible => {
                if restore_assumption(&mut problem, removed_index, removed_assumption).is_err() {
                    return Ok(initial_core_material(
                        initial_core,
                        initial.observation,
                        diagnostic_started,
                        &phase,
                    ));
                }
                if shrinker
                    .complete_trial(ConflictTrialOutcome::ProvenFeasible)
                    .is_err()
                    || !model_matches_groups(&problem, shrinker.retained_groups())
                {
                    return Ok(initial_core_material(
                        initial_core,
                        initial.observation,
                        diagnostic_started,
                        &phase,
                    ));
                }
            }
            TrialResult::Infeasible {
                observation,
                core_reservation,
                binding_reservation,
            } => {
                if shrinker
                    .complete_trial(ConflictTrialOutcome::ProvenInfeasible)
                    .is_err()
                {
                    drop(observation);
                    let _ = preparation.release_transient(core_reservation);
                    let _ = preparation.release_transient(binding_reservation);
                    return Ok(initial_core_material(
                        initial_core,
                        initial.observation,
                        diagnostic_started,
                        &phase,
                    ));
                }
                if model_matches_groups(&problem, shrinker.retained_groups()) {
                    parked_assumptions.push(removed_assumption);
                    // Retain only the last admitted deletion proof; every earlier trial remains
                    // represented by truthful state-machine counts, not per-trial witness frames.
                    let previous =
                        last_proven_reservations.replace((core_reservation, binding_reservation));
                    let previous_observation = last_proven_infeasible.replace(observation);
                    drop(previous_observation);
                    if let Some((core_reservation, binding_reservation)) = previous {
                        let _ = preparation.release_transient(core_reservation);
                        let _ = preparation.release_transient(binding_reservation);
                    }
                } else {
                    if restore_assumption(&mut problem, removed_index, removed_assumption).is_err()
                        || !model_matches_groups(&problem, shrinker.retained_groups())
                    {
                        drop(observation);
                        let _ = preparation.release_transient(core_reservation);
                        let _ = preparation.release_transient(binding_reservation);
                        return Ok(initial_core_material(
                            initial_core,
                            initial.observation,
                            diagnostic_started,
                            &phase,
                        ));
                    }
                    drop(observation);
                    let _ = preparation.release_transient(core_reservation);
                    let _ = preparation.release_transient(binding_reservation);
                }
            }
            TrialResult::Unavailable(_reason) => {
                if restore_assumption(&mut problem, removed_index, removed_assumption).is_err() {
                    return Ok(initial_core_material(
                        initial_core,
                        initial.observation,
                        diagnostic_started,
                        &phase,
                    ));
                }
                if shrinker
                    .complete_trial(ConflictTrialOutcome::Inconclusive)
                    .is_err()
                    || !model_matches_groups(&problem, shrinker.retained_groups())
                {
                    return Ok(initial_core_material(
                        initial_core,
                        initial.observation,
                        diagnostic_started,
                        &phase,
                    ));
                }
            }
        }
    }

    let Ok(infeasibility) = shrinker.into_result() else {
        return Ok(initial_core_material(
            initial_core,
            initial.observation,
            diagnostic_started,
            &phase,
        ));
    };
    let proof = DiagnosticInfeasibilityProofV1 {
        diagnostic_started_after_milliseconds: diagnostic_started,
        initial: initial.observation,
        initial_core,
        last_proven_infeasible,
        attempted_trials,
        max_trials,
    };
    Ok(InfeasibilityDiagnosticMaterial::Evidence {
        infeasibility,
        proof: Some(proof),
    })
}

struct InitialAttempt {
    observation: DiagnosticInfeasibilityObservationV1,
    core: Option<Vec<ConflictGroupV1>>,
    _core_reservation: u64,
    _binding_reservation: u64,
}

enum TrialResult {
    Feasible,
    Infeasible {
        observation: DiagnosticInfeasibilityObservationV1,
        core_reservation: u64,
        binding_reservation: u64,
    },
    Unavailable(ConflictUnavailableReason),
}

enum AttemptResult {
    Infeasible {
        observation: DiagnosticInfeasibilityObservationV1,
        core: Option<Vec<ConflictGroupV1>>,
        core_reservation: u64,
        binding_reservation: u64,
    },
    Feasible,
    Unavailable {
        reason: ConflictUnavailableReason,
    },
    Quarantined {
        alarm: CorrectnessAlarm,
    },
}
fn discard_attempt_result(result: AttemptResult, preparation: &mut PlanningPreparationBudget) {
    if let AttemptResult::Infeasible {
        observation,
        core,
        core_reservation,
        binding_reservation,
    } = result
    {
        drop(observation);
        drop(core);
        let _ = preparation.release_transient(core_reservation);
        let _ = preparation.release_transient(binding_reservation);
    }
}

#[derive(Default)]
struct ClockGuard {
    last: Option<DurationMillis>,
}

impl ClockGuard {
    fn observe(&mut self, clock: &dyn VerificationClock) -> Result<DurationMillis, AppError> {
        let now = clock.now_milliseconds();
        if self.last.is_some_and(|last| now < last) {
            return Err(clock_error());
        }
        self.last = Some(now);
        Ok(now)
    }
}

#[derive(Clone)]
struct ReviewSummary {
    partial_verified: bool,
    interrupted: Option<OperationInterruption>,
    resource_limit: bool,
}

struct DiscardProgress;

impl ProgressSink for DiscardProgress {
    fn emit(&mut self, _event: eutheto_solver_api::SolveProgressEvent) -> Result<(), OutputError> {
        Ok(())
    }
}

// A single bounded attempt owns preparation, dispatch and release across trusted clock checks.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_attempt(
    problem: &Arc<PlanningProblem>,
    options: &SolveOptions,
    phase: &SolveBudgetView,
    diagnostic_started: DurationMillis,
    context: &InfeasibilityDiagnosticContext<'_>,
    preparation: &mut PlanningPreparationBudget,
    clock: &mut ClockGuard,
) -> Result<AttemptResult, AppError> {
    let control = OperationControl::Solve(phase.clone());
    if let Some(material) = interruption_material(&control) {
        return Ok(attempt_material(material));
    }
    let elapsed_before_dispatch = observe_parent_elapsed(context, clock, &control)?;
    if elapsed_before_dispatch < diagnostic_started {
        return Err(clock_error());
    }
    let prepared = match PreparedPlanningModel::prepare(
        Arc::clone(problem),
        context.compile_context.planning_limits,
        preparation,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            return Ok(AttemptResult::Unavailable {
                reason: match error {
                    eutheto_planning_ir::CanonicalError::Preparation(error) => {
                        preparation_reason(error)
                    }
                    _ => ConflictUnavailableReason::DiagnosticFailed,
                },
            });
        }
    };
    let compatibility = context.backend.compatibility(prepared.summary(), options);
    if compatibility.level == CompatibilityLevel::Unsupported {
        let _ = prepared.release(preparation);
        return Ok(AttemptResult::Unavailable {
            reason: ConflictUnavailableReason::UnsupportedDomainPack,
        });
    }
    let Ok(backend_limit) = DurationMillis::new(DIAGNOSTIC_DISPATCH_LIMIT_MILLISECONDS) else {
        let _ = prepared.release(preparation);
        return Err(clock_error());
    };
    let dispatch_phase = phase.phase_view().capped_phase_view(Duration::from_millis(
        DIAGNOSTIC_DISPATCH_LIMIT_MILLISECONDS,
    ));
    let dispatched_after = match observe_parent_elapsed(context, clock, &control) {
        Ok(dispatched_after) => dispatched_after,
        Err(error) => {
            let _ = prepared.release(preparation);
            return Err(error);
        }
    };
    let prepared_guard = prepared.clone();
    let Ok(request) = SolveRequest::new_prepared(
        context.backend.descriptor().id.clone(),
        &context.backend.descriptor().version,
        &context.backend.descriptor().adapter_version,
        prepared,
        options.clone(),
        dispatch_phase,
        Some(backend_limit),
        DurationMillis::ZERO,
    ) else {
        let _ = prepared_guard.release(preparation);
        return Ok(AttemptResult::Unavailable {
            reason: ConflictUnavailableReason::DiagnosticFailed,
        });
    };
    drop(prepared_guard);
    let result = execute_request_inner(
        &request,
        problem,
        diagnostic_started,
        context,
        preparation,
        clock,
        dispatched_after,
        &control,
    )
    .await;
    let release_result = request.release_prepared(preparation);
    complete_attempt_result(result, release_result, preparation)
}

fn complete_attempt_result(
    result: Result<AttemptResult, AppError>,
    release_result: Result<(), PlanningPreparationError>,
    preparation: &mut PlanningPreparationBudget,
) -> Result<AttemptResult, AppError> {
    match (result, release_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(result @ AttemptResult::Quarantined { .. }), Err(_)) => {
            // A complete contradictory witness is an irreversible correctness alarm. Cleanup or
            // accounting failure cannot erase it, though release_prepared was still attempted.
            Ok(result)
        }
        (Ok(result), Err(error)) => {
            discard_attempt_result(result, preparation);
            Ok(AttemptResult::Unavailable {
                reason: preparation_reason(error),
            })
        }
        (Err(error), _) => Err(error),
    }
}

// This is one request's ordered output/review/alarm/outcome/core-admission transaction; splitting
// it would obscure cleanup and irreversible quarantine precedence.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_request_inner(
    request: &SolveRequest,
    problem: &PlanningProblem,
    diagnostic_started: DurationMillis,
    context: &InfeasibilityDiagnosticContext<'_>,
    preparation: &mut PlanningPreparationBudget,
    clock: &mut ClockGuard,
    dispatched_after: DurationMillis,
    control: &OperationControl,
) -> Result<AttemptResult, AppError> {
    if request.dispatch_budget().backend_limit() == DurationMillis::ZERO {
        return Ok(AttemptResult::Unavailable {
            reason: ConflictUnavailableReason::BudgetExpired,
        });
    }
    if let Some(material) = interruption_material(&OperationControl::Solve(
        request.dispatch_budget().child_view(),
    )) {
        return Ok(attempt_material(material));
    }

    let mut progress = DiscardProgress;
    let mut output = match BoundedBackendOutput::new_with_preparation(
        problem,
        &mut progress,
        request.dispatch_budget(),
        SolverApiLimits::DEFAULT,
        preparation,
    ) {
        Ok(output) => output,
        Err(BoundedOutputAdmissionError::Preparation(error)) => {
            return Ok(AttemptResult::Unavailable {
                reason: preparation_reason(error),
            });
        }
        Err(BoundedOutputAdmissionError::Output(_)) => {
            return Ok(AttemptResult::Unavailable {
                reason: ConflictUnavailableReason::ResourceLimitExceeded,
            });
        }
    };
    let backend_result = context.backend.solve(request, &mut output).await;
    let (candidates, output_reservation) = output.into_candidates_with_reservation();
    if let Err(error) = observe_parent_elapsed(context, clock, control) {
        let _ = preparation.release_transient(output_reservation);
        return Err(error);
    }
    let trial_view = request.dispatch_budget().child_view();
    let review_result = review_candidates(
        problem,
        request,
        &candidates,
        context,
        &trial_view,
        preparation,
        clock,
    );
    let review = match review_result {
        Ok(review) => review,
        Err(error) => {
            let _ = preparation.release_transient(output_reservation);
            return Err(error);
        }
    };
    if let Some(alarm) = review.0 {
        let _ = preparation.release_transient(output_reservation);
        return Ok(AttemptResult::Quarantined { alarm });
    }
    let review_summary = review.1;

    let Ok(outcome) = backend_result else {
        let reason = if let Some(reason) = review_summary.interrupted {
            interruption_reason(reason)
        } else if review_summary.resource_limit {
            ConflictUnavailableReason::ResourceLimitExceeded
        } else {
            classify_control_or_failure(control)
        };
        let _ = preparation.release_transient(output_reservation);
        return Ok(AttemptResult::Unavailable { reason });
    };
    let validation = validate_outcome(request, &outcome, &candidates, SolverApiLimits::DEFAULT);
    if let Err(error) = preparation.release_transient(output_reservation) {
        return Ok(AttemptResult::Unavailable {
            reason: preparation_reason(error),
        });
    }
    if let Err(error) = validation {
        if let Some(reason) = control.check().err() {
            return Ok(AttemptResult::Unavailable {
                reason: interruption_reason(reason),
            });
        }
        return Ok(AttemptResult::Unavailable {
            reason: classify_outcome_error(error, &outcome),
        });
    }
    if is_feasible_termination(outcome.termination) && review_summary.partial_verified {
        let observed_after = observe_parent_elapsed(context, clock, control)?;
        if !timely_observation(
            diagnostic_started,
            dispatched_after,
            observed_after,
            request.dispatch_budget().backend_limit(),
            &trial_view,
            context.parent_budget,
            context.options.time_limit_milliseconds,
        ) {
            return Ok(AttemptResult::Unavailable {
                reason: classify_control_or_failure(&OperationControl::Solve(trial_view)),
            });
        }
        return Ok(AttemptResult::Feasible);
    }
    if let Some(reason) = review_summary.interrupted {
        return Ok(AttemptResult::Unavailable {
            reason: interruption_reason(reason),
        });
    }
    if review_summary.resource_limit {
        return Ok(AttemptResult::Unavailable {
            reason: ConflictUnavailableReason::ResourceLimitExceeded,
        });
    }
    if outcome.termination != BackendTerminationReason::InfeasibilityClaimed {
        return Ok(AttemptResult::Unavailable {
            reason: classify_terminal(&outcome, control),
        });
    }

    let child_control = OperationControl::Solve(request.dispatch_budget().child_view());
    let previous_control = preparation.replace_control(child_control);
    let core_result = prepare_infeasible_core(
        problem,
        request,
        &outcome,
        context.compile_context.planning_limits,
        context.backend,
        preparation,
    );
    preparation.restore_control(previous_control);
    let (binding, mapped_core, binding_reservation) = match core_result {
        Ok(result) => result,
        Err(reason) => {
            return Ok(AttemptResult::Unavailable { reason });
        }
    };
    let observed_after = match observe_parent_elapsed(context, clock, control) {
        Ok(observed_after) => observed_after,
        Err(error) => {
            let _ = preparation.release_transient(binding_reservation);
            if let Some(mapped_core) = mapped_core {
                let _ = mapped_core.release(preparation);
            }
            return Err(error);
        }
    };
    if observed_after < dispatched_after || observed_after < diagnostic_started {
        let _ = preparation.release_transient(binding_reservation);
        if let Some(mapped_core) = mapped_core {
            let _ = mapped_core.release(preparation);
        }
        return Err(clock_error());
    }
    if !timely_observation(
        diagnostic_started,
        dispatched_after,
        observed_after,
        binding.time_limit_milliseconds,
        &trial_view,
        context.parent_budget,
        context.options.time_limit_milliseconds,
    ) {
        let _ = preparation.release_transient(binding_reservation);
        if let Some(mapped_core) = mapped_core {
            let _ = mapped_core.release(preparation);
        }
        return Ok(AttemptResult::Unavailable {
            reason: classify_control_or_failure(&OperationControl::Solve(trial_view)),
        });
    }
    let (core, core_reservation) = match mapped_core {
        Some(mapped_core) => {
            let (groups, reservation) = mapped_core.into_parts();
            (Some(groups), reservation)
        }
        None => (None, 0),
    };
    Ok(AttemptResult::Infeasible {
        observation: DiagnosticInfeasibilityObservationV1 {
            binding,
            dispatched_after_milliseconds: dispatched_after,
            observed_after_milliseconds: observed_after,
        },
        core,
        core_reservation,
        binding_reservation,
    })
}
fn prepare_infeasible_core(
    problem: &PlanningProblem,
    request: &SolveRequest,
    outcome: &BackendSolveOutcome,
    limits: PlanningIrLimitsV1,
    backend: &dyn SolverBackend,
    preparation: &mut PlanningPreparationBudget,
) -> Result<
    (
        DiagnosticSolveBindingV1,
        Option<MappedAssumptionGroups>,
        u64,
    ),
    ConflictUnavailableReason,
> {
    let binding_scratch = u64::try_from(problem.assumptions.len())
        .unwrap_or(u64::MAX)
        .saturating_mul(256);
    preparation
        .reserve_work(
            u64::try_from(problem.assumptions.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(4),
        )
        .map_err(preparation_reason)?;
    preparation
        .reserve_transient(binding_scratch)
        .map_err(preparation_reason)?;
    let result = (|| {
        let binding = native_binding(backend, request, outcome)
            .map_err(|()| ConflictUnavailableReason::DiagnosticFailed)?;
        preparation.check().map_err(preparation_reason)?;
        if binding.submitted_assumptions.is_empty() {
            return Ok((binding, None, binding_scratch));
        }
        let literals = match &outcome.evidence.assumption_core {
            Some(BackendAssumptionCoreEvidence::Sufficient { literals }) => literals,
            Some(BackendAssumptionCoreEvidence::Unavailable { reason }) => {
                return Err(match reason {
                    eutheto_solver_api::BackendAssumptionCoreUnavailableReason::NotReturned => {
                        ConflictUnavailableReason::ConflictNotReturned
                    }
                    eutheto_solver_api::BackendAssumptionCoreUnavailableReason::InvalidCore => {
                        ConflictUnavailableReason::InvalidAssumptionCore
                    }
                });
            }
            None => return Err(ConflictUnavailableReason::ConflictNotReturned),
        };
        match validate_complete_core(problem, literals, preparation) {
            Ok(true) => {}
            Ok(false) => return Err(ConflictUnavailableReason::InvalidAssumptionCore),
            Err(error) => return Err(preparation_reason(error)),
        }
        let groups = map_assumption_literals_with_budget(problem, literals, limits, preparation)
            .map_err(|error| match error {
                eutheto_explain::AssumptionMappingError::Preparation(error) => {
                    preparation_reason(error)
                }
                _ => ConflictUnavailableReason::InvalidAssumptionCore,
            })?;
        if !valid_mapped_groups(groups.groups()) {
            if let Err(eutheto_explain::AssumptionMappingError::Preparation(error)) =
                groups.release(preparation)
            {
                return Err(preparation_reason(error));
            }
            return Err(ConflictUnavailableReason::InvalidAssumptionCore);
        }
        Ok((binding, Some(groups), binding_scratch))
    })();
    if result.is_err() {
        let _ = preparation.release_transient(binding_scratch);
    }
    result
}
// Keep per-candidate verification, clock observations, interruption checks, and alarm precedence
// in one auditable pipeline; splitting it risks changing which trusted observation admits a core.
#[allow(clippy::too_many_lines)]
fn review_candidates(
    problem: &PlanningProblem,
    request: &SolveRequest,
    candidates: &[BackendCandidate],
    context: &InfeasibilityDiagnosticContext<'_>,
    phase: &SolveBudgetView,
    preparation: &mut PlanningPreparationBudget,
    clock: &mut ClockGuard,
) -> Result<(Option<CorrectnessAlarm>, ReviewSummary), AppError> {
    let mut summary = ReviewSummary {
        partial_verified: false,
        interrupted: None,
        resource_limit: false,
    };
    if candidates.is_empty() {
        return Ok((None, summary));
    }
    let control = OperationControl::Solve(phase.clone());
    let Some(prepared) = request.prepared_model() else {
        return Ok((
            Some(CorrectnessAlarm {
                category: CorrectnessAlarmCategory::InvalidPlanningProblem,
                diagnostic_code: CorrectnessAlarmCategory::InvalidPlanningProblem
                    .code()
                    .to_owned(),
            }),
            summary,
        ));
    };
    let reviewer = match AcceptanceReviewer::new_diagnostic(
        context.pack,
        context.document,
        context.compile_context.scenario_revision,
        prepared,
        context.compile_context.objective_context.as_ref(),
        context.verification_clock,
        preparation,
        &control,
    ) {
        Ok(reviewer) => reviewer,
        Err(DiagnosticReviewerError::Alarm(alarm)) => return Ok((Some(alarm), summary)),
        Err(DiagnosticReviewerError::Preparation(error)) => {
            match error {
                PlanningPreparationError::Cancelled => {
                    summary.interrupted = Some(OperationInterruption::Cancelled);
                }
                PlanningPreparationError::DeadlineExceeded => {
                    summary.interrupted = Some(OperationInterruption::DeadlineExceeded);
                }
                PlanningPreparationError::WorkLimitExceeded
                | PlanningPreparationError::TransientLimitExceeded
                | PlanningPreparationError::SerializedLimitExceeded
                | PlanningPreparationError::ArithmeticOverflow
                | PlanningPreparationError::Serialization => summary.resource_limit = true,
            }
            return Ok((None, summary));
        }
    };
    let retained_scratch = u64::try_from(problem.assumptions.len())
        .unwrap_or(u64::MAX)
        .saturating_mul(192)
        .saturating_add(4096);
    if let Err(error) = preparation.reserve_work(
        u64::try_from(problem.assumptions.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(2),
    ) {
        let _ = reviewer.release_diagnostic(preparation);
        match error {
            PlanningPreparationError::Cancelled => {
                summary.interrupted = Some(OperationInterruption::Cancelled);
            }
            PlanningPreparationError::DeadlineExceeded => {
                summary.interrupted = Some(OperationInterruption::DeadlineExceeded);
            }
            _ => summary.resource_limit = true,
        }
        return Ok((None, summary));
    }
    if let Err(error) = preparation.reserve_transient(retained_scratch) {
        let _ = reviewer.release_diagnostic(preparation);
        match error {
            PlanningPreparationError::Cancelled => {
                summary.interrupted = Some(OperationInterruption::Cancelled);
            }
            PlanningPreparationError::DeadlineExceeded => {
                summary.interrupted = Some(OperationInterruption::DeadlineExceeded);
            }
            _ => summary.resource_limit = true,
        }
        return Ok((None, summary));
    }
    let mut retained_rules = Vec::with_capacity(problem.assumptions.len());
    for assumption in &problem.assumptions {
        if let Err(error) = preparation.check() {
            let _ = preparation.release_transient(retained_scratch);
            let _ = reviewer.release_diagnostic(preparation);
            match error {
                PlanningPreparationError::Cancelled => {
                    summary.interrupted = Some(OperationInterruption::Cancelled);
                }
                PlanningPreparationError::DeadlineExceeded => {
                    summary.interrupted = Some(OperationInterruption::DeadlineExceeded);
                }
                _ => summary.resource_limit = true,
            }
            return Ok((None, summary));
        }
        let Some(rule) = assumption.required_rules.first() else {
            let _ = preparation.release_transient(retained_scratch);
            let _ = reviewer.release_diagnostic(preparation);
            return Ok((
                Some(CorrectnessAlarm {
                    category: CorrectnessAlarmCategory::VerificationScopeFailed,
                    diagnostic_code: CorrectnessAlarmCategory::VerificationScopeFailed
                        .code()
                        .to_owned(),
                }),
                summary,
            ));
        };
        retained_rules.push(*rule);
    }
    retained_rules.sort();
    if retained_rules.windows(2).any(|pair| pair[0] == pair[1]) {
        let _ = preparation.release_transient(retained_scratch);
        let _ = reviewer.release_diagnostic(preparation);
        return Ok((
            Some(CorrectnessAlarm {
                category: CorrectnessAlarmCategory::VerificationScopeFailed,
                diagnostic_code: CorrectnessAlarmCategory::VerificationScopeFailed
                    .code()
                    .to_owned(),
            }),
            summary,
        ));
    }

    let result = (|| {
        for candidate in candidates {
            if let Some(material) = interruption_material(&control) {
                summary.interrupted = Some(material_interruption(&material));
                break;
            }
            observe_parent_elapsed(context, clock, &control)?;
            let Ok(solution_id) = eutheto_types::SolutionId::new(context.ids) else {
                return Ok((None, summary.clone()));
            };
            let decision = reviewer.review_diagnostic(
                candidate,
                solution_id,
                &retained_rules,
                context.condition,
                &control,
                preparation,
            );
            match decision {
                DiagnosticDecision::OriginalWitness { .. } => {
                    return Ok((
                        Some(CorrectnessAlarm {
                            category: CorrectnessAlarmCategory::InfeasibilityContradicted,
                            diagnostic_code: CorrectnessAlarmCategory::InfeasibilityContradicted
                                .code()
                                .to_owned(),
                        }),
                        summary.clone(),
                    ));
                }
                DiagnosticDecision::Verified { .. } => summary.partial_verified = true,
                DiagnosticDecision::Quarantined { alarm, .. } => {
                    return Ok((Some(alarm), summary.clone()));
                }
                DiagnosticDecision::Interrupted { reason, .. } => {
                    summary.interrupted = Some(reason);
                    break;
                }
                DiagnosticDecision::ResourceLimitExceeded { .. } => {
                    summary.resource_limit = true;
                    break;
                }
            }
            observe_parent_elapsed(context, clock, &control)?;
        }
        Ok((None, summary.clone()))
    })();
    let release_retained = preparation.release_transient(retained_scratch);
    let release_reviewer = reviewer.release_diagnostic(preparation);
    match result {
        Ok(value @ (Some(_), _)) => Ok(value),
        Ok((alarm, mut summary)) => {
            if release_retained.is_err() || release_reviewer.is_err() {
                summary.resource_limit = true;
            }
            Ok((alarm, summary))
        }
        Err(error) => {
            let _ = release_retained;
            let _ = release_reviewer;
            Err(error)
        }
    }
}

fn pack_advertises_diagnostics(pack: &dyn DomainPack) -> bool {
    let Ok(descriptor) = pack.descriptor() else {
        return false;
    };
    descriptor
        .capabilities
        .contains(&DomainCapability::InfeasibilityDiagnostics)
        && descriptor
            .explanation_capabilities
            .contains(&ExplanationCapability::Infeasibility)
}

fn backend_advertises_native_diagnostics(backend: &dyn SolverBackend) -> bool {
    let descriptor = backend.descriptor();
    let runtime = backend.runtime_identity();
    descriptor.id.as_str() == ORTOOLS_BACKEND_ID
        && matches!(
            descriptor.distribution,
            eutheto_solver_api::SolverDistribution::BundledWorker
        )
        && descriptor
            .capabilities
            .available()
            .iter()
            .any(|feature| feature.as_str() == "ir.assumptions")
        && descriptor
            .capabilities
            .available()
            .iter()
            .any(|feature| feature.as_str() == "solve.infeasibility-evidence")
        && backend.verified_worker_manifest_sha256().is_some()
        && runtime.backend_id() == &descriptor.id
        && runtime.backend_version() == descriptor.version
        && runtime.adapter_version() == descriptor.adapter_version
        && runtime.solver_version() == descriptor.version
        && !runtime.worker_version().is_empty()
        && !runtime.solver_version().is_empty()
        && runtime.protocol_major() != 0
}

fn diagnostic_options(options: &SolveOptions, backend: &dyn SolverBackend) -> SolveOptions {
    let mut options = options.clone();
    options.backend = BackendSelection::Specific(backend.descriptor().id.clone());
    options.worker_threads = WorkerThreadPolicy::Exact(1);
    options.stop_after_first_feasible = false;
    options.collect_intermediate_solutions = false;
    options
}

fn validate_diagnostic_problem_shape(
    problem: &mut Arc<PlanningProblem>,
    source_rules: &[eutheto_types::RuleId],
    _limits: PlanningIrLimitsV1,
    preparation: &mut PlanningPreparationBudget,
) -> Result<(), ConflictUnavailableReason> {
    let model = Arc::get_mut(problem).ok_or(ConflictUnavailableReason::DiagnosticFailed)?;
    preparation.check().map_err(preparation_reason)?;
    preparation
        .reserve_work(
            u64::try_from(source_rules.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(8),
        )
        .map_err(preparation_reason)?;
    let source_scratch = u64::try_from(source_rules.len())
        .unwrap_or(u64::MAX)
        .saturating_mul(256);
    preparation
        .reserve_transient(source_scratch)
        .map_err(preparation_reason)?;
    let result = (|| {
        // The diagnostic compiler returns an owned canonical model.  Do not normalize or copy the
        // graph again here; the first controlled attempt performs the complete generic validation.
        if model.assumptions.len() > MAX_DIAGNOSTIC_GROUPS {
            return Err(ConflictUnavailableReason::ResourceLimitExceeded);
        }
        if !model.objectives.levels.is_empty() || model.assumptions.len() != source_rules.len() {
            return Err(ConflictUnavailableReason::DiagnosticFailed);
        }
        let source: BTreeSet<_> = source_rules.iter().copied().collect();
        if source.len() != source_rules.len() {
            return Err(ConflictUnavailableReason::DiagnosticFailed);
        }
        let mut groups = BTreeSet::new();
        let mut rules = BTreeSet::new();
        let mut variables = BTreeSet::new();
        for assumption in &model.assumptions {
            preparation.reserve_work(1).map_err(preparation_reason)?;
            if assumption.required_rules.len() != 1
                || !groups.insert(assumption.id.clone())
                || !rules.insert(assumption.required_rules[0])
                || !variables.insert(assumption.literal.variable.clone())
                || !source.contains(&assumption.required_rules[0])
            {
                return Err(ConflictUnavailableReason::DiagnosticFailed);
            }
        }
        if rules != source {
            return Err(ConflictUnavailableReason::DiagnosticFailed);
        }
        Ok(())
    })();
    preparation
        .release_transient(source_scratch)
        .map_err(preparation_reason)?;
    result
}

fn retain_assumptions_for_groups(
    problem: &mut Arc<PlanningProblem>,
    groups: &[ConflictGroupV1],
    parked: &mut Vec<Assumption>,
    preparation: &mut PlanningPreparationBudget,
) -> Result<(), ConflictUnavailableReason> {
    let Some(model) = Arc::get_mut(problem) else {
        return Err(ConflictUnavailableReason::DiagnosticFailed);
    };
    let index_scratch = u64::try_from(groups.len())
        .unwrap_or(u64::MAX)
        .saturating_mul(64);
    preparation
        .reserve_work(u64::try_from(model.assumptions.len()).unwrap_or(u64::MAX))
        .map_err(preparation_reason)?;
    preparation
        .reserve_transient(index_scratch)
        .map_err(preparation_reason)?;
    let wanted: BTreeSet<&AssumptionGroupId> = groups.iter().map(|group| &group.group_id).collect();
    let result = (|| {
        let mut keep = 0;
        for index in 0..model.assumptions.len() {
            preparation.check().map_err(preparation_reason)?;
            if wanted.contains(&model.assumptions[index].id) {
                model.assumptions.swap(keep, index);
                keep += 1;
            }
        }
        while model.assumptions.len() > keep {
            if let Some(assumption) = model.assumptions.pop() {
                parked.push(assumption);
            }
        }
        model
            .assumptions
            .sort_unstable_by(|left, right| left.id.cmp(&right.id));
        update_assumption_capability(model);
        Ok(())
    })();
    drop(wanted);
    let release = preparation.release_transient(index_scratch);
    match (result, release) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (_, Err(error)) => Err(preparation_reason(error)),
    }
}

fn remove_assumption_for_group(
    problem: &mut Arc<PlanningProblem>,
    group_id: &AssumptionGroupId,
) -> Option<(usize, Assumption)> {
    let model = Arc::get_mut(problem)?;
    let index = model
        .assumptions
        .iter()
        .position(|assumption| &assumption.id == group_id)?;
    let assumption = model.assumptions.remove(index);
    update_assumption_capability(model);
    Some((index, assumption))
}

fn restore_assumption(
    problem: &mut Arc<PlanningProblem>,
    index: usize,
    assumption: Assumption,
) -> Result<(), ()> {
    let model = Arc::get_mut(problem).ok_or(())?;
    if index > model.assumptions.len() {
        return Err(());
    }
    model.assumptions.insert(index, assumption);
    update_assumption_capability(model);
    Ok(())
}

fn update_assumption_capability(model: &mut PlanningProblem) {
    if model.assumptions.is_empty() {
        model
            .declared_capabilities
            .remove(&eutheto_planning_ir::Capability::Assumptions);
    } else {
        model
            .declared_capabilities
            .insert(eutheto_planning_ir::Capability::Assumptions);
    }
}

fn model_matches_groups(problem: &PlanningProblem, groups: &[ConflictGroupV1]) -> bool {
    problem.assumptions.len() == groups.len()
        && problem
            .assumptions
            .iter()
            .zip(groups)
            .all(|(assumption, group)| {
                assumption.id == group.group_id && assumption.required_rules == group.required_rules
            })
}

fn removed_group_id_from_model(
    problem: &PlanningProblem,
    candidate: &[ConflictGroupV1],
) -> Option<AssumptionGroupId> {
    problem
        .assumptions
        .iter()
        .find(|assumption| {
            !candidate
                .iter()
                .any(|group| group.group_id == assumption.id)
        })
        .map(|assumption| assumption.id.clone())
}

fn validate_complete_core(
    problem: &PlanningProblem,
    literals: &[Literal],
    preparation: &mut PlanningPreparationBudget,
) -> Result<bool, PlanningPreparationError> {
    if literals.is_empty() || literals.len() > MAX_DIAGNOSTIC_GROUPS {
        return Ok(false);
    }
    preparation.reserve_work(
        u64::try_from(literals.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::try_from(problem.assumptions.len()).unwrap_or(u64::MAX)),
    )?;
    preparation.check()?;
    if !literals.windows(2).all(|pair| pair[0] < pair[1]) {
        return Ok(false);
    }
    for literal in literals {
        preparation.check()?;
        if !problem
            .assumptions
            .iter()
            .any(|assumption| assumption.literal == *literal)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn valid_mapped_groups(groups: &[ConflictGroupV1]) -> bool {
    !groups.is_empty()
        && groups.len() <= MAX_DIAGNOSTIC_GROUPS
        && groups.windows(2).all(|pair| pair[0] < pair[1])
        && groups.iter().all(|group| group.required_rules.len() == 1)
}

fn native_binding(
    backend: &dyn SolverBackend,
    request: &SolveRequest,
    outcome: &BackendSolveOutcome,
) -> Result<DiagnosticSolveBindingV1, ()> {
    let execution = outcome.evidence.execution.as_ref().ok_or(())?;
    let runtime = backend.runtime_identity();
    let descriptor = backend.descriptor();
    let reproducibility = &execution.reproducibility;
    if reproducibility.backend_version != descriptor.version
        || reproducibility.adapter_version != descriptor.adapter_version
        || reproducibility.worker_version != runtime.worker_version()
        || reproducibility.engine_version != runtime.solver_version()
        || reproducibility.protocol_major != runtime.protocol_major()
        || reproducibility.protocol_minor != runtime.protocol_minor()
        || &reproducibility.applied_options != request.options()
        || reproducibility.applied_parameters.worker_threads != 1
        || reproducibility
            .applied_parameters
            .emit_intermediate_solutions
        || reproducibility.applied_parameters.stop_after_first_feasible
    {
        return Err(());
    }
    let parameter_digest = reproducibility
        .applied_parameters_sha256
        .as_ref()
        .filter(|digest| is_sha256(digest))
        .ok_or(())?
        .clone();
    let manifest = backend.verified_worker_manifest_sha256().ok_or(())?;
    let mut submitted = request
        .problem()
        .assumptions
        .iter()
        .map(|assumption| {
            Ok(DiagnosticSubmittedAssumptionV1 {
                variable_id: assumption
                    .literal
                    .variable
                    .as_str()
                    .parse()
                    .map_err(|_| ())?,
                value: assumption.literal.positive,
                group_id: assumption.id.clone(),
                required_rule_id: *assumption.required_rules.first().ok_or(())?,
            })
        })
        .collect::<Result<Vec<_>, ()>>()?;
    submitted.sort_by(|left, right| left.variable_id.cmp(&right.variable_id));
    if submitted
        .windows(2)
        .any(|pair| pair[0].variable_id >= pair[1].variable_id)
    {
        return Err(());
    }
    let native_profile = if submitted.is_empty() {
        DiagnosticNativeProfileV1::ZeroAssumptionSatisfaction
    } else {
        DiagnosticNativeProfileV1::NonemptyAssumptionSatisfaction
    };
    let binding = DiagnosticSolveBindingV1 {
        model_hash: request.model_hash().to_owned(),
        routed_fingerprint: request.solve_fingerprint().to_owned(),
        submitted_assumptions: submitted,
        native_profile,
        parameter_digest,
        worker_manifest_sha256: crate::infeasibility::manifest_digest(manifest),
        time_limit_milliseconds: request.dispatch_budget().backend_limit(),
    };
    binding.validate().map_err(|_| ())?;
    Ok(binding)
}

#[allow(clippy::too_many_arguments)]
fn timely_observation(
    diagnostic_started: DurationMillis,
    dispatched_after: DurationMillis,
    observed_after: DurationMillis,
    dispatch_limit: DurationMillis,
    phase: &SolveBudgetView,
    parent: &ParentSolveBudget,
    parent_limit: DurationMillis,
) -> bool {
    let Some(dispatch_elapsed) = observed_after.value().checked_sub(dispatched_after.value())
    else {
        return false;
    };
    let Some(phase_elapsed) = observed_after
        .value()
        .checked_sub(diagnostic_started.value())
    else {
        return false;
    };
    dispatch_elapsed < dispatch_limit.value()
        && u128::from(phase_elapsed) < DIAGNOSTIC_PHASE_LIMIT.as_millis()
        && !phase.is_expired()
        && !parent.is_expired()
        && !parent.is_cancelled()
        && observed_after < parent_limit
}

fn is_feasible_termination(termination: BackendTerminationReason) -> bool {
    matches!(
        termination,
        BackendTerminationReason::CandidateFound | BackendTerminationReason::OptimalityClaimed
    )
}

fn classify_outcome_error(
    error: OutcomeError,
    outcome: &BackendSolveOutcome,
) -> ConflictUnavailableReason {
    if error == OutcomeError::InvalidAssumptionCoreEvidence {
        if outcome.evidence.assumption_core.is_none()
            && outcome.termination == BackendTerminationReason::InfeasibilityClaimed
        {
            ConflictUnavailableReason::ConflictNotReturned
        } else {
            ConflictUnavailableReason::InvalidAssumptionCore
        }
    } else {
        ConflictUnavailableReason::DiagnosticFailed
    }
}

fn classify_terminal(
    outcome: &BackendSolveOutcome,
    control: &OperationControl,
) -> ConflictUnavailableReason {
    match outcome.termination {
        BackendTerminationReason::Cancelled => ConflictUnavailableReason::Cancelled,
        BackendTerminationReason::TimeLimit => ConflictUnavailableReason::BudgetExpired,
        BackendTerminationReason::SolutionLimit => ConflictUnavailableReason::ConflictNotReturned,
        BackendTerminationReason::Unavailable | BackendTerminationReason::Failed => {
            classify_control_or_failure(control)
        }
        _ => ConflictUnavailableReason::DiagnosticFailed,
    }
}

fn classify_control_or_failure(control: &OperationControl) -> ConflictUnavailableReason {
    match control.check() {
        Err(OperationInterruption::Cancelled) => ConflictUnavailableReason::Cancelled,
        Err(OperationInterruption::DeadlineExceeded) => ConflictUnavailableReason::BudgetExpired,
        Ok(()) => ConflictUnavailableReason::DiagnosticFailed,
    }
}
fn preparation_reason(error: PlanningPreparationError) -> ConflictUnavailableReason {
    match error {
        PlanningPreparationError::Cancelled => ConflictUnavailableReason::Cancelled,
        PlanningPreparationError::DeadlineExceeded => ConflictUnavailableReason::BudgetExpired,
        PlanningPreparationError::WorkLimitExceeded
        | PlanningPreparationError::TransientLimitExceeded
        | PlanningPreparationError::SerializedLimitExceeded
        | PlanningPreparationError::ArithmeticOverflow => {
            ConflictUnavailableReason::ResourceLimitExceeded
        }
        PlanningPreparationError::Serialization => ConflictUnavailableReason::ResourceLimitExceeded,
    }
}

fn interruption_reason(reason: OperationInterruption) -> ConflictUnavailableReason {
    match reason {
        OperationInterruption::Cancelled => ConflictUnavailableReason::Cancelled,
        OperationInterruption::DeadlineExceeded => ConflictUnavailableReason::BudgetExpired,
    }
}

fn interruption_material(control: &OperationControl) -> Option<InfeasibilityDiagnosticMaterial> {
    control
        .check()
        .err()
        .map(|reason| unavailable(interruption_reason(reason)))
}

fn attempt_material(material: InfeasibilityDiagnosticMaterial) -> AttemptResult {
    match material {
        InfeasibilityDiagnosticMaterial::Evidence {
            infeasibility: InfeasibilityEvidenceV1::Unavailable { reason },
            ..
        } => AttemptResult::Unavailable { reason },
        InfeasibilityDiagnosticMaterial::Quarantined { alarm } => {
            AttemptResult::Quarantined { alarm }
        }
        InfeasibilityDiagnosticMaterial::Evidence { .. } => AttemptResult::Unavailable {
            reason: ConflictUnavailableReason::DiagnosticFailed,
        },
    }
}

fn material_interruption(material: &InfeasibilityDiagnosticMaterial) -> OperationInterruption {
    match material {
        InfeasibilityDiagnosticMaterial::Evidence {
            infeasibility: InfeasibilityEvidenceV1::Unavailable { reason },
            ..
        } => match reason {
            ConflictUnavailableReason::Cancelled => OperationInterruption::Cancelled,
            _ => OperationInterruption::DeadlineExceeded,
        },
        InfeasibilityDiagnosticMaterial::Quarantined { .. }
        | InfeasibilityDiagnosticMaterial::Evidence { .. } => {
            OperationInterruption::DeadlineExceeded
        }
    }
}

fn pack_error_material(error: &DomainPackError) -> InfeasibilityDiagnosticMaterial {
    let reason = match error {
        DomainPackError::UnsupportedDomainCapability(_)
        | DomainPackError::UnsupportedExplanationCapability(_) => {
            ConflictUnavailableReason::UnsupportedDomainPack
        }
        DomainPackError::Cancelled => ConflictUnavailableReason::Cancelled,
        DomainPackError::BudgetExpired => ConflictUnavailableReason::BudgetExpired,
        DomainPackError::ResourceLimitExceeded => ConflictUnavailableReason::ResourceLimitExceeded,
        _ => ConflictUnavailableReason::DiagnosticFailed,
    };
    unavailable(reason)
}

fn initial_core_material(
    initial_core: Vec<ConflictGroupV1>,
    observation: DiagnosticInfeasibilityObservationV1,
    diagnostic_started: DurationMillis,
    phase: &SolveBudgetView,
) -> InfeasibilityDiagnosticMaterial {
    let evidence = ConflictShrinker::new(initial_core.clone(), 0, phase.phase_view())
        .and_then(ConflictShrinker::into_result);
    let Ok(infeasibility) = evidence else {
        return unavailable(ConflictUnavailableReason::DiagnosticFailed);
    };
    InfeasibilityDiagnosticMaterial::Evidence {
        infeasibility,
        proof: Some(DiagnosticInfeasibilityProofV1 {
            diagnostic_started_after_milliseconds: diagnostic_started,
            initial: observation,
            initial_core,
            last_proven_infeasible: None,
            attempted_trials: 0,
            max_trials: 0,
        }),
    }
}

fn unavailable(reason: ConflictUnavailableReason) -> InfeasibilityDiagnosticMaterial {
    InfeasibilityDiagnosticMaterial::Evidence {
        infeasibility: InfeasibilityEvidenceV1::Unavailable { reason },
        proof: None,
    }
}

fn observe_parent_elapsed(
    context: &InfeasibilityDiagnosticContext<'_>,
    clock: &mut ClockGuard,
    _control: &OperationControl,
) -> Result<DurationMillis, AppError> {
    let _ = clock.observe(context.verification_clock)?;
    let elapsed = context
        .parent_budget
        .checked_elapsed()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .and_then(|elapsed| DurationMillis::new(elapsed).ok())
        .ok_or_else(clock_error)?;
    Ok(elapsed)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn clock_error() -> AppError {
    AppError::Protocol(ProtocolFailure {
        code: "solve.clock_invalid".to_owned(),
        message: "The operation timing cannot be represented safely.".to_owned(),
        retryable: false,
        diagnostic_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn late_observations_are_rejected_without_backdating() -> TestResult {
        let clock = eutheto_types::FixedMonotonicClock::default();
        let budget = ParentSolveBudget::new(
            DurationMillis::new(5_000)?,
            Arc::new(clock),
            eutheto_types::CancellationToken::new(),
        )?;
        let phase = budget.phase_view();
        assert!(!timely_observation(
            DurationMillis::new(0)?,
            DurationMillis::new(100)?,
            DurationMillis::new(2_100)?,
            DurationMillis::new(2_000)?,
            &phase,
            &budget,
            DurationMillis::new(5_000)?,
        ));
        Ok(())
    }

    #[test]
    fn empty_native_core_cannot_be_mapped() {
        assert!(!valid_mapped_groups(&[]));
    }

    #[test]
    fn preparation_failures_retain_typed_unavailable_reasons() {
        assert_eq!(
            preparation_reason(PlanningPreparationError::Cancelled),
            ConflictUnavailableReason::Cancelled
        );
        assert_eq!(
            preparation_reason(PlanningPreparationError::DeadlineExceeded),
            ConflictUnavailableReason::BudgetExpired
        );
        assert_eq!(
            preparation_reason(PlanningPreparationError::TransientLimitExceeded),
            ConflictUnavailableReason::ResourceLimitExceeded
        );
    }

    #[test]
    fn optional_stop_preserves_small_admitted_core_proof() -> TestResult {
        let group = ConflictGroupV1 {
            group_id: AssumptionGroupId::new("assumption.fixture")?,
            required_rules: vec!["01890a5d-ac96-7b64-9f74-bbfcf30f9f01".parse()?],
        };
        let hash = "a".repeat(64);
        let observation = DiagnosticInfeasibilityObservationV1 {
            binding: DiagnosticSolveBindingV1 {
                model_hash: hash.clone(),
                routed_fingerprint: hash.clone(),
                submitted_assumptions: Vec::new(),
                native_profile: DiagnosticNativeProfileV1::ZeroAssumptionSatisfaction,
                parameter_digest: hash.clone(),
                worker_manifest_sha256: hash,
                time_limit_milliseconds: DurationMillis::new(1)?,
            },
            dispatched_after_milliseconds: DurationMillis::ZERO,
            observed_after_milliseconds: DurationMillis::new(1)?,
        };
        let clock = eutheto_types::FixedMonotonicClock::default();
        let budget = ParentSolveBudget::new(
            DurationMillis::new(5_000)?,
            Arc::new(clock),
            eutheto_types::CancellationToken::new(),
        )?;
        let material = initial_core_material(
            vec![group.clone()],
            observation,
            DurationMillis::ZERO,
            &budget.phase_view(),
        );
        let InfeasibilityDiagnosticMaterial::Evidence {
            infeasibility:
                InfeasibilityEvidenceV1::Conflict {
                    groups, minimality, ..
                },
            proof: Some(proof),
        } = material
        else {
            return Err(std::io::Error::other(
                "initial core was not retained as sufficient evidence",
            )
            .into());
        };
        assert_eq!(groups, vec![group]);
        assert_eq!(
            minimality,
            eutheto_domain_ir::ConflictMinimality::Sufficient
        );
        assert_eq!(proof.initial_core.len(), 1);
        assert_eq!(proof.max_trials, 0);
        Ok(())
    }
}
