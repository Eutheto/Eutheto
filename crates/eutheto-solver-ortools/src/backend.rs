use std::sync::Arc;
use std::time::{Duration, Instant};

use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_protocol::wire::{
    Capability, Progress, ProgressKind, ResourceLimits, SolveParameters,
    SolveRequest as WorkerSolveRequest, worker_frame,
};
use eutheto_protocol::{
    CompletedSession, MAX_ORTOOLS_WORKER_THREADS, WorkerObservation, checked_in_policy,
};
use eutheto_solver_api::{
    BACKEND_STATISTIC_MAX_V1, BackendAppliedParameterEvidence, BackendAssumptionCoreEvidence,
    BackendAssumptionCoreUnavailableReason, BackendError, BackendExecutionEvidence,
    BackendModelCountEvidence, BackendOutputSink, BackendReproducibilityEvidence,
    BackendRuntimeIdentity, BackendRuntimeIdentityError, BackendSolveFuture, BackendSolveOutcome,
    BackendTerminationEvidence, BackendTerminationReason, BackendTimingEvidence,
    BackendWorkerStatistics, CapabilityMatrix, CompatibilityLevel, CompatibilityReport,
    OutputError, RegistryError, SolverBackend, SolverDescriptor, SolverRegistry, preflight,
};
use eutheto_types::{DurationMillis, ReproducibilityMode, SolveOptions, WorkerThreadPolicy};
use prost::bytes::Bytes;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    AdapterEvidenceRecorder, CandidateProofClaim, NormalizedWorkerTerminal,
    ORTOOLS_ADAPTER_VERSION, ORTOOLS_VERSION, OrToolsDescriptorError, SafeStopReason,
    SessionCompletion, SessionFailure, SessionRequest, SupervisorError, TranslatedCpSatModel,
    VerifiedWorkerArtifact, WorkerIdentity, WorkerLimitKind, backend_started_progress,
    bound_progress, candidate_progress, candidate_submission, duration_millis, normalize_terminal,
    ortools_compatibility, ortools_descriptor, supervise_with_observer, translate_supported_model,
};

// Measured completion margin stays inside the unchanged outer backend allocation.
const WORKER_COMPLETION_RESERVE: Duration = Duration::from_millis(100);

const WORKER_IDENTITY: &str = "eutheto-ortools-worker";
const WORKER_BACKEND_ID: &str = "ortools-cp-sat";
const WORKER_VERSION: &str = "0.1.0";

const WORKER_CAPABILITIES: [Capability; 8] = [
    Capability::CpSat,
    Capability::DeterministicTime,
    Capability::IntermediateSolutions,
    Capability::ObjectiveBounds,
    Capability::Progress,
    Capability::SolutionProjection,
    Capability::SolutionStats,
    Capability::SufficientAssumptions,
];

// Later worker incumbents are coalesced; the first usable candidate and terminal
// candidate are the only two distinct submissions for one solve.

/// The production OR-Tools adapter bound to one manifest-verified worker runtime closure.
pub struct OrToolsBackend {
    descriptor: SolverDescriptor,
    runtime_identity: BackendRuntimeIdentity,
    matrix: CapabilityMatrix,
    artifact: VerifiedWorkerArtifact,
}

fn ortools_runtime_identity(
    descriptor: &SolverDescriptor,
) -> Result<BackendRuntimeIdentity, OrToolsBackendBuildError> {
    let policy = checked_in_policy()?;
    BackendRuntimeIdentity::new(
        descriptor.id.clone(),
        ORTOOLS_VERSION.to_owned(),
        ORTOOLS_ADAPTER_VERSION.to_owned(),
        WORKER_VERSION.to_owned(),
        ORTOOLS_VERSION.to_owned(),
        policy.protocol_major(),
        policy.protocol_minor(),
    )
    .map_err(OrToolsBackendBuildError::from)
}

impl OrToolsBackend {
    /// Constructs the adapter only when its descriptor matches the generated production matrix.
    ///
    /// # Errors
    ///
    /// Returns an error when the reviewed descriptor, runtime identity, protocol policy, or
    /// generated support matrix is inconsistent.
    pub fn new(
        matrix: CapabilityMatrix,
        artifact: VerifiedWorkerArtifact,
    ) -> Result<Self, OrToolsBackendBuildError> {
        let descriptor = ortools_descriptor()?;
        matrix.validate_descriptor(&descriptor)?;
        let runtime_identity = ortools_runtime_identity(&descriptor)?;
        Ok(Self {
            descriptor,
            runtime_identity,
            matrix,
            artifact,
        })
    }

    fn worker_identity(&self) -> WorkerIdentity {
        WorkerIdentity {
            worker_identity: WORKER_IDENTITY.to_owned(),
            worker_version: self.runtime_identity.worker_version().to_owned(),
            backend_id: WORKER_BACKEND_ID.to_owned(),
            ortools_version: self.runtime_identity.solver_version().to_owned(),
            adapter_version: self.runtime_identity.adapter_version().to_owned(),
            required_capabilities: WORKER_CAPABILITIES.to_vec(),
            advertised_capabilities: WORKER_CAPABILITIES.to_vec(),
        }
    }

    fn admit_request(
        &self,
        request: &eutheto_solver_api::SolveRequest,
    ) -> Result<Option<BackendSolveOutcome>, BackendError> {
        preflight(&self.matrix, &self.descriptor, request).map_err(|_| {
            safe_backend_error(
                "solver.ortools.preflight",
                "The routed request failed the shared solver preflight contract.",
            )
        })?;
        let expected_search = request
            .dispatch_budget()
            .backend_limit()
            .value()
            .saturating_sub(self.completion_reserve().value());
        if request.dispatch_budget().search_limit().value() != expected_search {
            return Err(safe_backend_error(
                "solver.ortools.completion_reserve",
                "The routed search allocation does not match the native backend completion reserve.",
            ));
        }
        if request
            .dispatch_budget()
            .remaining_search_duration()
            .is_zero()
        {
            return Ok(Some(search_exhausted_outcome(request)));
        }
        let compatibility = self.compatibility(request.summary(), request.options());
        if compatibility.level == CompatibilityLevel::Unsupported {
            return Err(safe_backend_error(
                "solver.ortools.unsupported",
                "The routed request is outside the reviewed OR-Tools capability surface.",
            ));
        }

        Ok(None)
    }

    async fn solve_request(
        &self,
        request: &eutheto_solver_api::SolveRequest,
        output: &mut dyn BackendOutputSink,
    ) -> Result<BackendSolveOutcome, BackendError> {
        if let Some(outcome) = self.admit_request(request)? {
            return Ok(outcome);
        }

        let translation_started = Instant::now();
        let translated = translate_supported_model(request.problem(), PlanningIrLimitsV1::DEFAULT)
            .map_err(|_| {
                safe_backend_error("solver.ortools.translation", "The validated planning model could not be translated by the reviewed OR-Tools adapter.")
            })?;
        let wall_time_millis = u64::try_from(
            request
                .dispatch_budget()
                .remaining_search_duration()
                .as_millis(),
        )
        .unwrap_or(u64::MAX);
        if wall_time_millis == 0 {
            return Ok(search_exhausted_outcome(request));
        }
        let (worker_request, request_evidence) =
            build_worker_request(request, &translated, wall_time_millis)?;
        let translation_milliseconds = duration_millis(translation_started.elapsed());
        let mut recorder = AdapterEvidenceRecorder::new(
            &translated,
            request.dispatch_budget().remaining_at_dispatch(),
            request.dispatch_budget().backend_limit(),
        );
        let collect_intermediate = request.options().collect_intermediate_solutions;
        let mut retained_intermediate = false;

        let session = SessionRequest {
            executable: self.artifact.executable(),
            identity: self.worker_identity(),
            core_version: env!("CARGO_PKG_VERSION").to_owned(),
            solve: worker_request,
        };
        let completion = supervise_with_observer(
            session,
            request.dispatch_budget().child_view(),
            |observation| {
                observe_worker(
                    observation,
                    &translated,
                    collect_intermediate,
                    request.dispatch_budget().elapsed_milliseconds(),
                    output,
                    &mut recorder,
                    &mut retained_intermediate,
                )
            },
        )
        .await;

        // A Failed/Unavailable outcome cannot legally carry candidates. Return an error so
        // the router still reviews the first independently observed incumbent rather than
        // rejecting the whole attempt as contradictory terminal evidence.
        let termination = match &completion {
            Ok(completion) => normalized_termination(normalize_terminal(&completion.evidence)),
            Err(failure) => failure_termination(failure),
        };
        if retained_intermediate
            && matches!(
                termination,
                BackendTerminationReason::Failed | BackendTerminationReason::Unavailable
            )
        {
            return Err(safe_backend_error(
                "solver.ortools.session_after_candidate",
                "The OR-Tools worker failed after reporting a candidate.",
            ));
        }

        let execution_evidence = self.execution_evidence(
            request,
            &translated,
            &request_evidence,
            translation_milliseconds,
            &completion,
        )?;
        if let Ok(completion) = &completion {
            process_terminal_candidate(completion, &translated, request, output, &mut recorder)?;
            if termination == BackendTerminationReason::InfeasibilityClaimed
                && let Some(core) = assumption_core_for_terminal(request, &translated, completion)
            {
                recorder.record_assumption_core(core);
            }
        }
        let mut evidence = recorder
            .finish(request.dispatch_budget().elapsed_milliseconds())
            .map_err(|_| {
                safe_backend_error(
                    "solver.ortools.evidence",
                    "OR-Tools adapter evidence violated its parent-measured ordering contract.",
                )
            })?;
        evidence.execution = Some(execution_evidence);
        Ok(BackendSolveOutcome {
            backend_id: self.descriptor.id.clone(),
            model_hash: request.model_hash().to_owned(),
            solve_fingerprint: request.solve_fingerprint().to_owned(),
            termination,
            evidence,
        })
    }

    fn execution_evidence(
        &self,
        request: &eutheto_solver_api::SolveRequest,
        translated: &TranslatedCpSatModel,
        request_evidence: &WorkerRequestEvidence,
        translation_milliseconds: DurationMillis,
        session: &Result<SessionCompletion, SessionFailure>,
    ) -> Result<BackendExecutionEvidence, BackendError> {
        let runtime_identity = self.runtime_identity();
        let session_timings = match session {
            Ok(completion) => Some(&completion.timings),
            Err(failure) => failure.timings.as_ref(),
        };
        let translation_milliseconds = session_timings
            .and_then(|value| value.request_dispatch_milliseconds)
            .map_or(translation_milliseconds, |request_dispatch| {
                DurationMillis::new(
                    translation_milliseconds
                        .value()
                        .saturating_add(request_dispatch.value()),
                )
                .unwrap_or(DurationMillis::MAX)
            });
        let timings = BackendTimingEvidence {
            translation_serialization_milliseconds: translation_milliseconds,
            worker_startup_milliseconds: session_timings.map(|value| value.startup_milliseconds),
            handshake_milliseconds: session_timings.map(|value| value.handshake_milliseconds),
            solver_milliseconds: session_timings.and_then(|value| value.solver_milliseconds),
            protocol_decode_milliseconds: session_timings
                .map(|value| value.protocol_decode_milliseconds),
        };
        let (worker_statistics, applied_parameters_sha256) =
            terminal_execution_evidence(session.as_ref().ok())?;
        let mut applied_parameters = request_evidence.applied_parameters.clone();
        applied_parameters.wall_time_milliseconds =
            session_timings.and_then(|value| value.dispatched_wall_time_milliseconds);
        Ok(BackendExecutionEvidence {
            timings,
            model_counts: BackendModelCountEvidence {
                planning_variable_count: request.summary().variable_count,
                planning_constraint_count: request.summary().constraint_count,
                translated_variable_count: translated.translated_variable_count(),
                translated_constraint_count: translated.translated_constraint_count(),
            },
            worker_statistics,
            reproducibility: BackendReproducibilityEvidence {
                backend_version: runtime_identity.backend_version().to_owned(),
                adapter_version: runtime_identity.adapter_version().to_owned(),
                worker_version: runtime_identity.worker_version().to_owned(),
                engine_version: runtime_identity.solver_version().to_owned(),
                protocol_major: runtime_identity.protocol_major(),
                protocol_minor: runtime_identity.protocol_minor(),
                applied_options: request.options().clone(),
                applied_parameters,
                model_fingerprint_sha256: request_evidence.model_fingerprint_sha256.clone(),
                applied_parameters_sha256,
            },
        })
    }
}

impl SolverBackend for OrToolsBackend {
    fn completion_reserve(&self) -> DurationMillis {
        duration_millis(WORKER_COMPLETION_RESERVE)
    }

    fn descriptor(&self) -> &SolverDescriptor {
        &self.descriptor
    }

    fn runtime_identity(&self) -> &BackendRuntimeIdentity {
        &self.runtime_identity
    }

    fn verified_worker_manifest_sha256(&self) -> Option<&[u8; 32]> {
        Some(self.artifact.manifest_sha256())
    }

    fn compatibility(
        &self,
        problem: &eutheto_planning_ir::PlanningProblemSummary,
        options: &SolveOptions,
    ) -> CompatibilityReport {
        ortools_compatibility(&self.matrix, problem, options).unwrap_or_else(|_| {
            CompatibilityReport {
                level: CompatibilityLevel::Unsupported,
                unsupported_features: Vec::new(),
                warnings: Vec::new(),
                estimated_translation_cost: None,
            }
        })
    }

    fn solve<'a>(
        &'a self,
        request: &'a eutheto_solver_api::SolveRequest,
        output: &'a mut dyn BackendOutputSink,
    ) -> BackendSolveFuture<'a> {
        Box::pin(self.solve_request(request, output))
    }
}

/// Builds the exact production registry around one already manifest-verified bundled worker.
///
/// # Errors
///
/// Returns an error when generated metadata, the OR-Tools descriptor, or registry invariants fail.
pub fn registry_with_ortools(
    artifact: VerifiedWorkerArtifact,
) -> Result<SolverRegistry, OrToolsBackendBuildError> {
    let matrix = CapabilityMatrix::generated()?;
    let backend = Arc::new(OrToolsBackend::new(matrix.clone(), artifact)?);
    let backend: Arc<dyn SolverBackend> = backend;
    Ok(SolverRegistry::new(matrix, [backend])?)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkerRequestEvidence {
    applied_parameters: BackendAppliedParameterEvidence,
    model_fingerprint_sha256: String,
}

// An exhausted search allocation establishes no native terminal or runtime evidence.
fn search_exhausted_outcome(request: &eutheto_solver_api::SolveRequest) -> BackendSolveOutcome {
    BackendSolveOutcome {
        backend_id: request.backend_id().clone(),
        model_hash: request.model_hash().to_owned(),
        solve_fingerprint: request.solve_fingerprint().to_owned(),
        termination: BackendTerminationReason::TimeLimit,
        evidence: BackendTerminationEvidence {
            remaining_at_dispatch_milliseconds: request.dispatch_budget().remaining_at_dispatch(),
            backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
            elapsed_milliseconds: request.dispatch_budget().elapsed_milliseconds(),
            first_incumbent_milliseconds: None,
            objective: None,
            best_bound_values: None,
            assumption_core: None,
            evidence_refs: Vec::new(),
            execution: None,
        },
    }
}

fn build_worker_request(
    request: &eutheto_solver_api::SolveRequest,
    translated: &TranslatedCpSatModel,
    wall_time_millis: u64,
) -> Result<(WorkerSolveRequest, WorkerRequestEvidence), BackendError> {
    let random_seed = i32::try_from(request.options().random_seed).map_err(|_| {
        safe_backend_error(
            "solver.ortools.random_seed",
            "The random seed is outside the OR-Tools signed 32-bit parameter range.",
        )
    })?;
    let deterministic = request.options().reproducibility == ReproducibilityMode::Deterministic;
    let log_search_progress = !translated.objective_plan().levels.is_empty();
    let worker_threads = match request.options().worker_threads {
        WorkerThreadPolicy::Exact(count) => u32::from(count),
        WorkerThreadPolicy::Auto => std::thread::available_parallelism()
            .map_or(1, |count| u32::try_from(count.get()).unwrap_or(u32::MAX))
            .clamp(1, MAX_ORTOOLS_WORKER_THREADS),
    };
    let model_fingerprint: [u8; 32] = Sha256::digest(translated.cp_model_proto()).into();
    let worker_request = WorkerSolveRequest {
        request_id: request.solve_fingerprint().to_owned(),
        cp_model_proto: Bytes::copy_from_slice(translated.cp_model_proto()),
        parameters: Some(SolveParameters {
            random_seed: Some(random_seed),
            stop_after_first_feasible: Some(request.options().stop_after_first_feasible),
            emit_intermediate_solutions: Some(request.options().collect_intermediate_solutions),
            log_search_progress: Some(log_search_progress),
            deterministic_test_profile: Some(deterministic),
        }),
        projections: translated.worker_projection_requests().to_vec(),
        resource_limits: Some(ResourceLimits {
            wall_time_millis,
            memory_bytes: None,
            worker_threads,
        }),
        model_fingerprint: Bytes::copy_from_slice(&model_fingerprint),
    };
    Ok((
        worker_request,
        WorkerRequestEvidence {
            applied_parameters: BackendAppliedParameterEvidence {
                wall_time_milliseconds: None,
                memory_limit_bytes: None,
                worker_threads,
                random_seed,
                stop_after_first_feasible: request.options().stop_after_first_feasible,
                emit_intermediate_solutions: request.options().collect_intermediate_solutions,
                log_search_progress,
                deterministic_test_profile: deterministic,
            },
            model_fingerprint_sha256: encode_hex(&model_fingerprint),
        },
    ))
}

fn terminal_execution_evidence(
    completion: Option<&SessionCompletion>,
) -> Result<(Option<BackendWorkerStatistics>, Option<String>), BackendError> {
    let Some(SessionCompletion {
        evidence: CompletedSession::Solve(terminal),
        ..
    }) = completion
    else {
        return Ok((None, None));
    };
    let Some(worker_frame::Body::Finished(finished)) = &terminal.frame().body else {
        return Ok((None, None));
    };
    if finished.applied_parameters_sha256.len() != 32 {
        return Err(safe_backend_error(
            "solver.ortools.parameters_hash",
            "OR-Tools returned an invalid applied-parameter digest.",
        ));
    }
    Ok((
        Some(BackendWorkerStatistics {
            wall_time_milliseconds: seconds_to_millis(finished.wall_time_seconds)?,
            user_time_milliseconds: seconds_to_millis(finished.user_time_seconds)?,
            deterministic_time_milliseconds: seconds_to_millis(finished.deterministic_time)?,
            conflicts: checked_worker_counter(finished.conflicts)?,
            branches: checked_worker_counter(finished.branches)?,
            binary_propagations: checked_worker_counter(finished.binary_propagations)?,
            integer_propagations: checked_worker_counter(finished.integer_propagations)?,
        }),
        Some(encode_hex(&finished.applied_parameters_sha256)),
    ))
}

fn seconds_to_millis(value: Option<f64>) -> Result<Option<DurationMillis>, BackendError> {
    value
        .map(|seconds| {
            let duration = Duration::try_from_secs_f64(seconds).map_err(|_| {
                safe_backend_error(
                    "solver.ortools.worker_timing",
                    "OR-Tools returned an invalid native timing value.",
                )
            })?;
            let milliseconds = u64::try_from(duration.as_millis()).map_err(|_| {
                safe_backend_error(
                    "solver.ortools.worker_timing",
                    "OR-Tools returned an out-of-range native timing value.",
                )
            })?;
            DurationMillis::new(milliseconds).map_err(|_| {
                safe_backend_error(
                    "solver.ortools.worker_timing",
                    "OR-Tools returned an out-of-range native timing value.",
                )
            })
        })
        .transpose()
}

fn checked_worker_counter(value: Option<u64>) -> Result<Option<u64>, BackendError> {
    if value.is_some_and(|count| count > BACKEND_STATISTIC_MAX_V1) {
        return Err(safe_backend_error(
            "solver.ortools.worker_statistics",
            "OR-Tools returned an out-of-range native search counter.",
        ));
    }
    Ok(value)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn emit_optional_progress(
    output: &mut dyn BackendOutputSink,
    event: eutheto_solver_api::SolveProgressEvent,
) -> Result<(), OutputError> {
    match output.emit_progress(event) {
        Ok(()) | Err(OutputError::ProgressLimitExceeded) => Ok(()),
        Err(error) => Err(error),
    }
}

fn observe_worker(
    observation: WorkerObservation,
    translated: &TranslatedCpSatModel,
    collect_intermediate: bool,
    elapsed: DurationMillis,
    output: &mut dyn BackendOutputSink,
    recorder: &mut AdapterEvidenceRecorder,
    retained_intermediate: &mut bool,
) -> Result<(), SupervisorError> {
    match observation {
        WorkerObservation::Started(_) => output
            .emit_progress(backend_started_progress().map_err(|_| SupervisorError::AdapterOutput)?)
            .map_err(|_| SupervisorError::AdapterOutput),
        WorkerObservation::Progress(progress) => {
            if let Some(event) = bound_progress(translated, &progress, elapsed)
                .map_err(|_| SupervisorError::AdapterOutput)?
            {
                let emit =
                    if let eutheto_solver_api::SolveProgressEvent::BoundImproved(bound) = &event {
                        recorder
                            .record_bound(bound)
                            .map_err(|_| SupervisorError::AdapterOutput)?
                    } else {
                        true
                    };
                if emit {
                    emit_optional_progress(output, event)
                        .map_err(|_| SupervisorError::AdapterOutput)?;
                }
            }
            Ok(())
        }
        WorkerObservation::Incumbent(incumbent) => {
            if !collect_intermediate {
                return Err(SupervisorError::AdapterOutput);
            }
            if *retained_intermediate {
                return Ok(());
            }
            let Some(candidate) = incumbent.candidate.as_ref() else {
                return Err(SupervisorError::AdapterOutput);
            };
            let submission = candidate_submission(translated, candidate, elapsed)
                .map_err(|_| SupervisorError::AdapterOutput)?;
            let mut staged_recorder = recorder.clone();
            staged_recorder
                .record_submission(&submission)
                .map_err(|_| SupervisorError::AdapterOutput)?;
            let accepted = output
                .submit_candidate(submission)
                .map_err(|_| SupervisorError::AdapterOutput)?;
            emit_optional_progress(output, candidate_progress(&accepted))
                .map_err(|_| SupervisorError::AdapterOutput)?;
            *retained_intermediate = true;
            *recorder = staged_recorder;
            Ok(())
        }
        WorkerObservation::Terminal => Ok(()),
        WorkerObservation::HandshakeAccepted | WorkerObservation::HandshakeRejected => {
            Err(SupervisorError::AdapterOutput)
        }
    }
}

fn process_terminal_candidate(
    completion: &SessionCompletion,
    translated: &TranslatedCpSatModel,
    request: &eutheto_solver_api::SolveRequest,
    output: &mut dyn BackendOutputSink,
    recorder: &mut AdapterEvidenceRecorder,
) -> Result<(), BackendError> {
    let CompletedSession::Solve(terminal) = &completion.evidence else {
        return Ok(());
    };
    let Some(worker_frame::Body::Finished(finished)) = &terminal.frame().body else {
        return Ok(());
    };
    let elapsed = request.dispatch_budget().elapsed_milliseconds();
    let mut staged_recorder = recorder.clone();
    let mut terminal_bound = None;
    if !finished.best_bound_values.is_empty() {
        let progress = Progress {
            kind: ProgressKind::BoundImproved as i32,
            best_bound_values: finished.best_bound_values.clone(),
            ..Progress::default()
        };
        if let Some(event) = bound_progress(translated, &progress, elapsed).map_err(|_| {
            safe_backend_error(
                "solver.ortools.bound",
                "OR-Tools returned invalid terminal bound evidence.",
            )
        })? {
            let emit = if let eutheto_solver_api::SolveProgressEvent::BoundImproved(bound) = &event
            {
                staged_recorder.record_bound(bound).map_err(|_| {
                    safe_backend_error(
                        "solver.ortools.bound",
                        "OR-Tools terminal bound evidence contradicted the translated objective.",
                    )
                })?
            } else {
                true
            };
            if emit {
                terminal_bound = Some(event);
            }
        }
    }
    let terminal_candidate =
        if let Some(candidate) = &finished.final_candidate {
            let submission = candidate_submission(translated, candidate, elapsed).map_err(|_| {
            safe_backend_error(
                "solver.ortools.candidate",
                "OR-Tools returned a projected candidate outside the translated model contract.",
            )
        })?;
            staged_recorder.record_submission(&submission).map_err(|_| {
            safe_backend_error(
                "solver.ortools.evidence",
                "OR-Tools candidate evidence contradicted the translated objective contract.",
            )
        })?;
            Some(submission)
        } else {
            None
        };

    if let Some(event) = terminal_bound {
        emit_optional_progress(output, event)?;
    }
    if let Some(submission) = terminal_candidate {
        let accepted = output.submit_candidate(submission)?;
        emit_optional_progress(output, candidate_progress(&accepted))?;
    }
    *recorder = staged_recorder;
    Ok(())
}

fn assumption_core_for_terminal(
    request: &eutheto_solver_api::SolveRequest,
    translated: &TranslatedCpSatModel,
    completion: &SessionCompletion,
) -> Option<BackendAssumptionCoreEvidence> {
    let problem = request.problem();
    if problem.assumptions.is_empty()
        || !problem.objectives.levels.is_empty()
        || request.options().worker_threads != WorkerThreadPolicy::Exact(1)
    {
        return None;
    }
    let CompletedSession::Solve(terminal) = &completion.evidence else {
        return Some(BackendAssumptionCoreEvidence::Unavailable {
            reason: BackendAssumptionCoreUnavailableReason::NotReturned,
        });
    };
    let Some(worker_frame::Body::Finished(finished)) = &terminal.frame().body else {
        return Some(BackendAssumptionCoreEvidence::Unavailable {
            reason: BackendAssumptionCoreUnavailableReason::NotReturned,
        });
    };
    Some(map_assumption_core(
        &finished.sufficient_assumptions,
        translated,
        problem.assumptions.len(),
    ))
}
fn map_assumption_core(
    raw_core: &[i32],
    translated: &TranslatedCpSatModel,
    submitted_assumption_count: usize,
) -> BackendAssumptionCoreEvidence {
    if raw_core.is_empty() {
        return BackendAssumptionCoreEvidence::Unavailable {
            reason: BackendAssumptionCoreUnavailableReason::NotReturned,
        };
    }
    // Bound the untrusted worker vector before allocating the owned source-literal buffer.
    if raw_core.len() > submitted_assumption_count {
        return BackendAssumptionCoreEvidence::Unavailable {
            reason: BackendAssumptionCoreUnavailableReason::InvalidCore,
        };
    }
    // Native membership is checked allocation-free, before any source literal is cloned.
    if raw_core
        .iter()
        .any(|native_literal| translated.assumption(*native_literal).is_none())
    {
        return BackendAssumptionCoreEvidence::Unavailable {
            reason: BackendAssumptionCoreUnavailableReason::InvalidCore,
        };
    }

    let mut literals = Vec::with_capacity(raw_core.len());
    for native_literal in raw_core {
        // The complete native set was checked above before any source-literal clone is allocated.
        let Some(assumption) = translated.assumption(*native_literal) else {
            return BackendAssumptionCoreEvidence::Unavailable {
                reason: BackendAssumptionCoreUnavailableReason::InvalidCore,
            };
        };
        literals.push(assumption.literal.clone());
    }
    literals.sort_unstable();
    if !literals.windows(2).all(|pair| pair[0] < pair[1]) {
        return BackendAssumptionCoreEvidence::Unavailable {
            reason: BackendAssumptionCoreUnavailableReason::InvalidCore,
        };
    }
    BackendAssumptionCoreEvidence::Sufficient { literals }
}

const fn normalized_termination(terminal: NormalizedWorkerTerminal) -> BackendTerminationReason {
    match terminal {
        NormalizedWorkerTerminal::Candidate {
            proof: CandidateProofClaim::Optimal,
            ..
        } => BackendTerminationReason::OptimalityClaimed,
        NormalizedWorkerTerminal::Candidate { .. } => BackendTerminationReason::CandidateFound,
        NormalizedWorkerTerminal::Infeasible => BackendTerminationReason::InfeasibilityClaimed,
        NormalizedWorkerTerminal::NoSolutionWithinLimit(WorkerLimitKind::Solution) => {
            BackendTerminationReason::SolutionLimit
        }
        NormalizedWorkerTerminal::NoSolutionWithinLimit(_) => BackendTerminationReason::TimeLimit,
        NormalizedWorkerTerminal::Cancelled { .. } => BackendTerminationReason::Cancelled,
        NormalizedWorkerTerminal::AdapterDefect(_) => BackendTerminationReason::InvalidModel,
        NormalizedWorkerTerminal::BackendUnavailable(_) => BackendTerminationReason::Unavailable,
        NormalizedWorkerTerminal::BackendFailed { .. } => BackendTerminationReason::Failed,
    }
}

const fn failure_termination(failure: &SessionFailure) -> BackendTerminationReason {
    match failure.stop_reason {
        SafeStopReason::Cancelled => BackendTerminationReason::Cancelled,
        SafeStopReason::DeadlineExceeded => BackendTerminationReason::TimeLimit,
        SafeStopReason::HandshakeRejected | SafeStopReason::Io => {
            BackendTerminationReason::Unavailable
        }
        SafeStopReason::Completed
        | SafeStopReason::ProtocolViolation
        | SafeStopReason::OutputLimit
        | SafeStopReason::WorkerExit => BackendTerminationReason::Failed,
    }
}

fn safe_backend_error(code: &'static str, message: &'static str) -> BackendError {
    match BackendError::new(code, message) {
        Ok(error) => error,
        Err(error) => BackendError::from(error),
    }
}

/// Failure to construct the exact production OR-Tools backend registry.
#[derive(Debug, Error)]
pub enum OrToolsBackendBuildError {
    #[error(transparent)]
    Descriptor(#[from] OrToolsDescriptorError),
    #[error(transparent)]
    RuntimeIdentity(#[from] BackendRuntimeIdentityError),
    #[error(transparent)]
    Protocol(#[from] eutheto_protocol::ProtocolFault),
    #[error(transparent)]
    Matrix(#[from] eutheto_solver_api::SupportMatrixError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

#[cfg(test)]
mod tests {
    use eutheto_domain_ir::{
        AssumptionGroupId, DomainAssignmentId, DomainEntityId, DomainEntityKindId, DomainEntityRef,
    };
    use eutheto_planning_ir::{
        Assumption, BoolVariable, BoolVariableId, Capability, CompilerId, Literal, ObjectivePlan,
        PLANNING_IR_SCHEMA_VERSION, PROJECTION_SCHEMA_VERSION, PlanningMetadata, PlanningProblem,
        ProjectionExpression, ProjectionId, ProvenanceId, ProvenanceRecord, ProvenanceSourceKind,
        SolutionProjection, Variable,
    };
    use eutheto_protocol::wire::{Incumbent, ProjectedCandidate, ProjectedValue};
    use eutheto_solver_api::{
        BackendCandidate, BackendOutputSink, CandidateSubmission, OutputError, SolveProgressEvent,
    };
    use eutheto_types::{PackId, ScenarioId};
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default)]
    struct RecordingOutput {
        candidates: Vec<BackendCandidate>,
        progress: Vec<SolveProgressEvent>,
        reject_incumbent_progress: bool,
    }

    impl BackendOutputSink for RecordingOutput {
        fn emit_progress(&mut self, event: SolveProgressEvent) -> Result<(), OutputError> {
            if self.reject_incumbent_progress
                && matches!(event, SolveProgressEvent::IncumbentFound(_))
            {
                return Err(OutputError::ProgressLimitExceeded);
            }
            self.progress.push(event);
            Ok(())
        }

        fn submit_candidate(
            &mut self,
            candidate: CandidateSubmission,
        ) -> Result<BackendCandidate, OutputError> {
            let accepted = BackendCandidate {
                sequence: u32::try_from(self.candidates.len() + 1)
                    .map_err(|_| OutputError::CandidateLimitExceeded)?,
                values: candidate.values,
                observed_after_milliseconds: candidate.observed_after_milliseconds,
                objective: candidate.objective,
                evidence_refs: candidate.evidence_refs,
            };
            self.candidates.push(accepted.clone());
            Ok(accepted)
        }
    }

    fn translated_fixture() -> Result<TranslatedCpSatModel, Box<dyn std::error::Error>> {
        let provenance = ProvenanceId::new("backend.test.provenance")?;
        let variable = BoolVariableId::new("backend.test.selected")?;
        let problem = PlanningProblem {
            schema_version: PLANNING_IR_SCHEMA_VERSION,
            variables: vec![Variable::Boolean(BoolVariable {
                id: variable.clone(),
                provenance: provenance.clone(),
            })],
            constraints: Vec::new(),
            objectives: ObjectivePlan::default(),
            assumptions: vec![Assumption {
                id: AssumptionGroupId::new("backend.test.assumption")?,
                literal: Literal::positive(variable.clone()),
                required_rules: vec!["01890a5d-ac96-7b64-9f74-bbfcf30f9f01".parse()?],
                provenance: provenance.clone(),
            }],
            projections: vec![SolutionProjection {
                id: ProjectionId::new("backend.test.projection")?,
                assignment_id: DomainAssignmentId::new("backend.test.assignment")?,
                entity: DomainEntityRef {
                    kind: DomainEntityKindId::new("backend.test.entity")?,
                    id: DomainEntityId::new("backend.test.entity.one")?,
                },
                required: true,
                expression: ProjectionExpression::Boolean(variable),
                provenance: provenance.clone(),
            }],
            provenance: vec![ProvenanceRecord {
                id: provenance,
                source_kind: ProvenanceSourceKind::RequiredRule,
                source_id: "backend.test".to_owned(),
                entity_refs: Vec::new(),
                message_key: "backend.test".to_owned(),
                parameters: BTreeMap::default(),
                parent: None,
            }],
            metadata: PlanningMetadata {
                pack_id: PackId::new("official.synthetic")?,
                scenario_id: "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse::<ScenarioId>()?,
                scenario_revision: 1,
                projection_version: PROJECTION_SCHEMA_VERSION,
                compiler_id: CompilerId::new("compiler.backend.test")?,
                compiler_version: "1.0.0".to_owned(),
                compile_metadata: BTreeMap::default(),
                display_text: BTreeMap::default(),
            },
            declared_capabilities: [Capability::BooleanProjection, Capability::Assumptions]
                .into_iter()
                .collect(),
            split_authorization: None,
        };
        Ok(translate_supported_model(
            &problem,
            PlanningIrLimitsV1::DEFAULT,
        )?)
    }

    fn incumbent(value: i64) -> WorkerObservation {
        WorkerObservation::Incumbent(Incumbent {
            candidate: Some(ProjectedCandidate {
                values: vec![ProjectedValue {
                    projection_id: 0,
                    value,
                }],
            }),
            ..Incumbent::default()
        })
    }

    #[test]
    fn assumption_core_mapping_is_complete_and_signed_without_partial_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let translated = translated_fixture()?;
        assert!(matches!(
            map_assumption_core(&[0], &translated, 1),
            BackendAssumptionCoreEvidence::Sufficient { literals }
                if literals == vec![Literal::positive(BoolVariableId::new("backend.test.selected")?)]
        ));
        assert_eq!(
            map_assumption_core(&[0, 0], &translated, 2),
            BackendAssumptionCoreEvidence::Unavailable {
                reason: BackendAssumptionCoreUnavailableReason::InvalidCore,
            }
        );
        for raw_core in [&[0, 1][..], &[1][..], &[-1][..]] {
            assert_eq!(
                map_assumption_core(raw_core, &translated, 1),
                BackendAssumptionCoreEvidence::Unavailable {
                    reason: BackendAssumptionCoreUnavailableReason::InvalidCore,
                }
            );
        }
        assert_eq!(
            map_assumption_core(&[], &translated, 1),
            BackendAssumptionCoreEvidence::Unavailable {
                reason: BackendAssumptionCoreUnavailableReason::NotReturned,
            }
        );
        Ok(())
    }

    #[test]
    fn first_valid_intermediate_candidate_is_parent_timed_and_bounded()
    -> Result<(), Box<dyn std::error::Error>> {
        let translated = translated_fixture()?;
        let mut recorder = AdapterEvidenceRecorder::new(
            &translated,
            DurationMillis::new(100)?,
            DurationMillis::new(100)?,
        );
        let mut output = RecordingOutput::default();
        let mut state = false;

        observe_worker(
            incumbent(0),
            &translated,
            true,
            DurationMillis::new(7)?,
            &mut output,
            &mut recorder,
            &mut state,
        )?;
        observe_worker(
            incumbent(1),
            &translated,
            true,
            DurationMillis::new(11)?,
            &mut output,
            &mut recorder,
            &mut state,
        )?;

        assert_eq!(output.candidates.len(), 1);
        assert_eq!(
            output.candidates[0].observed_after_milliseconds,
            DurationMillis::new(7)?
        );
        assert_eq!(output.progress.len(), 1);
        assert_eq!(
            recorder
                .finish(DurationMillis::new(20)?)?
                .first_incumbent_milliseconds,
            Some(DurationMillis::new(7)?)
        );
        Ok(())
    }

    #[test]
    fn incumbent_progress_limit_does_not_drop_retained_candidate()
    -> Result<(), Box<dyn std::error::Error>> {
        let translated = translated_fixture()?;
        let mut recorder = AdapterEvidenceRecorder::new(
            &translated,
            DurationMillis::new(100)?,
            DurationMillis::new(100)?,
        );
        let mut output = RecordingOutput {
            reject_incumbent_progress: true,
            ..RecordingOutput::default()
        };
        let mut state = false;

        observe_worker(
            incumbent(0),
            &translated,
            true,
            DurationMillis::new(7)?,
            &mut output,
            &mut recorder,
            &mut state,
        )?;

        assert_eq!(output.candidates.len(), 1);
        assert!(output.progress.is_empty());
        Ok(())
    }

    #[test]
    fn unrequested_intermediate_candidate_keeps_legacy_rejection()
    -> Result<(), Box<dyn std::error::Error>> {
        let translated = translated_fixture()?;
        let mut recorder = AdapterEvidenceRecorder::new(
            &translated,
            DurationMillis::new(100)?,
            DurationMillis::new(100)?,
        );
        let mut output = RecordingOutput::default();
        let mut state = false;

        assert!(matches!(
            observe_worker(
                incumbent(0),
                &translated,
                false,
                DurationMillis::new(7)?,
                &mut output,
                &mut recorder,
                &mut state,
            ),
            Err(SupervisorError::AdapterOutput)
        ));
        assert!(output.candidates.is_empty());
        Ok(())
    }

    #[test]
    fn production_runtime_identity_uses_exact_reviewed_versions()
    -> Result<(), Box<dyn std::error::Error>> {
        let descriptor = ortools_descriptor()?;
        let identity = ortools_runtime_identity(&descriptor)?;
        let policy = checked_in_policy()?;

        assert_eq!(identity.backend_id(), &descriptor.id);
        assert_eq!(identity.backend_version(), ORTOOLS_VERSION);
        assert_eq!(identity.adapter_version(), ORTOOLS_ADAPTER_VERSION);
        assert_eq!(identity.worker_version(), WORKER_VERSION);
        assert_eq!(identity.solver_version(), ORTOOLS_VERSION);
        assert_eq!(identity.protocol_major(), policy.protocol_major());
        assert_eq!(identity.protocol_minor(), policy.protocol_minor());
        Ok(())
    }
}
