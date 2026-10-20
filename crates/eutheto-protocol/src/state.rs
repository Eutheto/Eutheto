use std::collections::BTreeSet;
use std::fmt;

use prost::bytes::Bytes;
use semver::Version;
use sha2::{Digest, Sha256};

use crate::limits::{
    APPLIED_PARAMETERS_DOMAIN, ProtocolPolicy, SUFFICIENT_ASSUMPTIONS_ENABLED, checked_in_policy,
};
use crate::stderr::is_unsafe_control;
use crate::wire::handshake_response::Outcome;
use crate::wire::{
    Capability, Finished, HandshakeError, HandshakeErrorCode, HandshakeRequest, HandshakeSuccess,
    ParentFrame, Progress, ProgressKind, ProjectedCandidate, SolveRequest, TerminationReason,
    WorkerErrorCode, WorkerFrame, WorkerSolveStatus, parent_frame, worker_frame,
};
use crate::{ProtocolFault, StateFault};

#[derive(Clone, PartialEq, Eq)]
pub struct HandshakeExpectations {
    protocol_major: u32,
    protocol_minor: u32,
    worker_identity: String,
    worker_version: String,
    backend_id: String,
    ortools_version: String,
    adapter_version: String,
    manifest_sha256: [u8; 32],
    required_capabilities: BTreeSet<i32>,
    expected_advertised_capabilities: BTreeSet<i32>,
}

impl fmt::Debug for HandshakeExpectations {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HandshakeExpectations")
            .field("protocol_major", &self.protocol_major)
            .field("protocol_minor", &self.protocol_minor)
            .field(
                "required_capability_count",
                &self.required_capabilities.len(),
            )
            .field(
                "advertised_capability_count",
                &self.expected_advertised_capabilities.len(),
            )
            .finish_non_exhaustive()
    }
}

impl HandshakeExpectations {
    /// Builds validated expectations for one worker handshake.
    ///
    /// # Errors
    ///
    /// Returns a policy or state fault for invalid versions, identities,
    /// duplicate capabilities, or a required capability that is not expected.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        protocol_major: u32,
        protocol_minor: u32,
        worker_identity: impl Into<String>,
        worker_version: impl Into<String>,
        backend_id: impl Into<String>,
        ortools_version: impl Into<String>,
        adapter_version: impl Into<String>,
        manifest_sha256: [u8; 32],
        required_capabilities: impl IntoIterator<Item = Capability>,
        expected_advertised_capabilities: impl IntoIterator<Item = Capability>,
    ) -> Result<Self, ProtocolFault> {
        let policy = checked_in_policy()?;
        if protocol_major != policy.protocol_major() || protocol_minor != policy.protocol_minor() {
            return Err(StateFault::ProtocolVersion {
                expected_major: policy.protocol_major(),
                expected_minor: policy.protocol_minor(),
                actual_major: protocol_major,
                actual_minor: protocol_minor,
            }
            .into());
        }
        let worker_identity = worker_identity.into();
        let worker_version = worker_version.into();
        let backend_id = backend_id.into();
        let ortools_version = ortools_version.into();
        let adapter_version = adapter_version.into();
        validate_expected_text(
            policy,
            "eutheto.worker.v1.HandshakeSuccess.worker_identity",
            "worker_identity",
            &worker_identity,
        )?;
        validate_expected_text(
            policy,
            "eutheto.worker.v1.HandshakeSuccess.worker_version",
            "worker_version",
            &worker_version,
        )?;
        validate_expected_text(
            policy,
            "eutheto.worker.v1.HandshakeSuccess.backend_id",
            "backend_id",
            &backend_id,
        )?;
        validate_expected_text(
            policy,
            "eutheto.worker.v1.HandshakeSuccess.ortools_version",
            "ortools_version",
            &ortools_version,
        )?;
        validate_expected_text(
            policy,
            "eutheto.worker.v1.HandshakeSuccess.adapter_version",
            "adapter_version",
            &adapter_version,
        )?;
        validate_semver("worker_version", &worker_version)?;
        validate_semver("ortools_version", &ortools_version)?;
        validate_semver("adapter_version", &adapter_version)?;

        let mut required = BTreeSet::new();
        for capability in required_capabilities {
            let capability = capability as i32;
            if capability == Capability::Unspecified as i32 {
                return Err(StateFault::InvalidCapability.into());
            }
            if !SUFFICIENT_ASSUMPTIONS_ENABLED
                && capability == Capability::SufficientAssumptions as i32
            {
                return Err(StateFault::CapabilityDisabled(capability).into());
            }
            if !required.insert(capability) {
                return Err(StateFault::DuplicateCapability(capability).into());
            }
        }
        let mut advertised = BTreeSet::new();
        for capability in expected_advertised_capabilities {
            let capability = capability as i32;
            if capability == Capability::Unspecified as i32 {
                return Err(StateFault::InvalidCapability.into());
            }
            if !SUFFICIENT_ASSUMPTIONS_ENABLED
                && capability == Capability::SufficientAssumptions as i32
            {
                return Err(StateFault::CapabilityDisabled(capability).into());
            }
            if !advertised.insert(capability) {
                return Err(StateFault::DuplicateCapability(capability).into());
            }
        }
        if !required.is_subset(&advertised) {
            return Err(StateFault::InvalidHandshake("expected_advertised_capabilities").into());
        }
        Ok(Self {
            protocol_major,
            protocol_minor,
            worker_identity,
            worker_version,
            backend_id,
            ortools_version,
            adapter_version,
            manifest_sha256,
            required_capabilities: required,
            expected_advertised_capabilities: advertised,
        })
    }

    /// Builds expectations using the checked-in protocol version.
    ///
    /// # Errors
    ///
    /// Returns the same policy and state faults as [`Self::new`].
    #[allow(clippy::too_many_arguments)]
    pub fn checked_in(
        worker_identity: impl Into<String>,
        worker_version: impl Into<String>,
        backend_id: impl Into<String>,
        ortools_version: impl Into<String>,
        adapter_version: impl Into<String>,
        manifest_sha256: [u8; 32],
        required_capabilities: impl IntoIterator<Item = Capability>,
        expected_advertised_capabilities: impl IntoIterator<Item = Capability>,
    ) -> Result<Self, ProtocolFault> {
        let policy = checked_in_policy()?;
        Self::new(
            policy.protocol_major(),
            policy.protocol_minor(),
            worker_identity,
            worker_version,
            backend_id,
            ortools_version,
            adapter_version,
            manifest_sha256,
            required_capabilities,
            expected_advertised_capabilities,
        )
    }

    pub fn required_capabilities(&self) -> impl Iterator<Item = i32> + '_ {
        self.required_capabilities.iter().copied()
    }

    pub fn expected_advertised_capabilities(&self) -> impl Iterator<Item = i32> + '_ {
        self.expected_advertised_capabilities.iter().copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentPhase {
    HandshakeRequest,
    HandshakeResponse,
    Rejected,
    RejectedEof,
    RejectedComplete,
    SolveRequest,
    Started,
    Running,
    Terminal,
    Eof,
    Complete,
    Failed,
}

/// Validated protocol payloads remain untrusted solver evidence.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkerObservation {
    HandshakeAccepted,
    HandshakeRejected,
    Started(StartedObservation),
    Progress(Progress),
    Incumbent(crate::wire::Incumbent),
    Terminal,
}

/// Correlated started evidence detached from the decoded frame allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedObservation {
    pub request_id: String,
    pub model_fingerprint: [u8; 32],
}

/// Fully expanded solve parameters hashed by both protocol endpoints.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NormalizedAppliedParameters {
    pub wall_time_millis: u64,
    pub worker_threads: u32,
    pub random_seed: i32,
    pub stop_after_first_feasible: bool,
    pub emit_intermediate_solutions: bool,
    pub log_search_progress: bool,
    pub deterministic_test_profile: bool,
}

/// Expands optional applied values and validates cross-field backend limits.
///
/// # Errors
///
/// Returns a state fault for missing or invalid parameters and resource limits.
pub fn normalize_applied_parameters(
    solve: &SolveRequest,
) -> Result<NormalizedAppliedParameters, ProtocolFault> {
    let policy = checked_in_policy()?;
    let parameters = solve
        .parameters
        .as_ref()
        .ok_or(StateFault::InvalidSolveRequest("parameters"))?;
    let resource_limits = solve
        .resource_limits
        .as_ref()
        .ok_or(StateFault::InvalidSolveRequest("resource_limits"))?;
    if resource_limits.wall_time_millis == 0 {
        return Err(StateFault::InvalidSolveRequest("resource_limits.wall_time_millis").into());
    }
    if resource_limits.worker_threads == 0
        || resource_limits.worker_threads > policy.max_worker_threads()
    {
        return Err(StateFault::InvalidSolveRequest("resource_limits.worker_threads").into());
    }
    if resource_limits.memory_bytes == Some(0) {
        return Err(StateFault::InvalidSolveRequest("resource_limits.memory_bytes").into());
    }
    let normalized = NormalizedAppliedParameters {
        wall_time_millis: resource_limits.wall_time_millis,
        worker_threads: resource_limits.worker_threads,
        random_seed: parameters.random_seed.unwrap_or(1),
        stop_after_first_feasible: parameters.stop_after_first_feasible.unwrap_or(false),
        emit_intermediate_solutions: parameters.emit_intermediate_solutions.unwrap_or(false),
        log_search_progress: parameters.log_search_progress.unwrap_or(false),
        deterministic_test_profile: parameters.deterministic_test_profile.unwrap_or(false),
    };
    if normalized.deterministic_test_profile
        && (normalized.worker_threads != 1 || normalized.random_seed != 1)
    {
        return Err(
            StateFault::InvalidSolveRequest("parameters.deterministic_test_profile").into(),
        );
    }
    Ok(normalized)
}

/// Returns the fixed 56-byte, domain-separated applied-parameter preimage.
#[must_use]
pub fn applied_parameters_preimage(parameters: &NormalizedAppliedParameters) -> [u8; 56] {
    let mut preimage = [0u8; 56];
    preimage[..36].copy_from_slice(APPLIED_PARAMETERS_DOMAIN);
    preimage[36..44].copy_from_slice(&parameters.wall_time_millis.to_be_bytes());
    preimage[44..48].copy_from_slice(&parameters.worker_threads.to_be_bytes());
    preimage[48..52].copy_from_slice(&parameters.random_seed.to_be_bytes());
    preimage[52] = u8::from(parameters.stop_after_first_feasible);
    preimage[53] = u8::from(parameters.emit_intermediate_solutions);
    preimage[54] = u8::from(parameters.log_search_progress);
    preimage[55] = u8::from(parameters.deterministic_test_profile);
    preimage
}

/// Hashes normalized applied values using the protocol's SHA-256 contract.
#[must_use]
pub fn applied_parameters_sha256(parameters: &NormalizedAppliedParameters) -> [u8; 32] {
    Sha256::digest(applied_parameters_preimage(parameters)).into()
}

/// A validated worker handshake rejection after clean EOF and exit code zero.
#[derive(PartialEq, Eq)]
pub struct HandshakeRejectionEvidence {
    error: HandshakeError,
}

impl HandshakeRejectionEvidence {
    #[must_use]
    pub fn error(&self) -> &HandshakeError {
        &self.error
    }
}

impl fmt::Debug for HandshakeRejectionEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HandshakeRejectionEvidence")
    }
}

/// The exact terminal worker frame after protocol-consistent EOF and exit.
///
/// This remains untrusted candidate evidence. It conveys no feasibility,
/// objective, model-validity, or domain-validity claim.
#[derive(PartialEq)]
pub struct TerminalEvidence {
    frame: WorkerFrame,
}

impl TerminalEvidence {
    #[must_use]
    pub fn frame(&self) -> &WorkerFrame {
        &self.frame
    }
}

impl fmt::Debug for TerminalEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.frame.body {
            Some(worker_frame::Body::Finished(_)) => "finished",
            Some(worker_frame::Body::Error(_)) => "error",
            _ => "invalid",
        };
        formatter
            .debug_struct("TerminalEvidence")
            .field("kind", &kind)
            .finish()
    }
}

/// Evidence that became available only after clean EOF and exit code zero.
#[derive(PartialEq)]
pub enum CompletedSession {
    Solve(Box<TerminalEvidence>),
    HandshakeRejected(HandshakeRejectionEvidence),
}

impl fmt::Debug for CompletedSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Solve(_) => "CompletedSession::Solve",
            Self::HandshakeRejected(_) => "CompletedSession::HandshakeRejected",
        })
    }
}

#[derive(Debug)]
enum PhaseState {
    HandshakeRequest,
    HandshakeResponse,
    Rejected(HandshakeRejectionEvidence),
    RejectedEof(HandshakeRejectionEvidence),
    RejectedComplete(HandshakeRejectionEvidence),
    SolveRequest,
    Started {
        request_id: String,
        model_fingerprint: [u8; 32],
        requested_projections: Vec<u64>,
        applied_parameters: NormalizedAppliedParameters,
    },
    Running {
        request_id: String,
        model_fingerprint: [u8; 32],
        requested_projections: Vec<u64>,
        applied_parameters: NormalizedAppliedParameters,
    },
    Terminal(TerminalEvidence),
    Eof(TerminalEvidence),
    Complete(TerminalEvidence),
    Failed,
}

pub struct ParentProtocol {
    expectations: HandshakeExpectations,
    negotiated_capabilities: BTreeSet<i32>,
    phase: PhaseState,
}

impl fmt::Debug for ParentProtocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParentProtocol")
            .field("phase", &self.phase())
            .finish()
    }
}

impl ParentProtocol {
    #[must_use]
    pub fn new(expectations: HandshakeExpectations) -> Self {
        Self {
            expectations,
            negotiated_capabilities: BTreeSet::new(),
            phase: PhaseState::HandshakeRequest,
        }
    }

    #[must_use]
    pub fn phase(&self) -> ParentPhase {
        match &self.phase {
            PhaseState::HandshakeRequest => ParentPhase::HandshakeRequest,
            PhaseState::HandshakeResponse => ParentPhase::HandshakeResponse,
            PhaseState::Rejected(_) => ParentPhase::Rejected,
            PhaseState::RejectedEof(_) => ParentPhase::RejectedEof,
            PhaseState::RejectedComplete(_) => ParentPhase::RejectedComplete,
            PhaseState::SolveRequest => ParentPhase::SolveRequest,
            PhaseState::Started { .. } => ParentPhase::Started,
            PhaseState::Running { .. } => ParentPhase::Running,
            PhaseState::Terminal(_) => ParentPhase::Terminal,
            PhaseState::Eof(_) => ParentPhase::Eof,
            PhaseState::Complete(_) => ParentPhase::Complete,
            PhaseState::Failed => ParentPhase::Failed,
        }
    }

    /// Applies one parent-originated frame to the session state.
    ///
    /// # Errors
    ///
    /// Returns a state or policy fault for a missing, invalid, or out-of-order
    /// handshake or solve request.
    pub fn on_parent_frame(&mut self, frame: &ParentFrame) -> Result<(), ProtocolFault> {
        let result = self.on_parent_frame_inner(frame);
        self.absorb_failure(result)
    }

    /// Tightens the validated worker wall-time cap before the solve frame is written.
    ///
    /// This is valid only after a solve request entered [`ParentPhase::Started`]. It changes no
    /// model, projection, or solver setting and keeps terminal parameter-hash validation aligned
    /// with the exact request written to the worker.
    ///
    /// # Errors
    ///
    /// Returns a state fault when called out of order or when the new cap is zero or greater than
    /// the already validated cap.
    pub fn tighten_started_wall_time_millis(
        &mut self,
        wall_time_millis: u64,
    ) -> Result<(), ProtocolFault> {
        let state = self.state_name();
        let result = match &mut self.phase {
            PhaseState::Started {
                applied_parameters, ..
            } if wall_time_millis > 0
                && wall_time_millis <= applied_parameters.wall_time_millis =>
            {
                applied_parameters.wall_time_millis = wall_time_millis;
                Ok(())
            }
            PhaseState::Started { .. } => {
                Err(StateFault::InvalidSolveRequest("resource_limits.wall_time_millis").into())
            }
            _ => Err(StateFault::UnexpectedParent {
                state,
                message: "wall-time-tightening",
            }
            .into()),
        };
        self.absorb_failure(result)
    }

    fn on_parent_frame_inner(&mut self, frame: &ParentFrame) -> Result<(), ProtocolFault> {
        let body = frame.body.as_ref().ok_or(StateFault::MissingParentBody)?;
        match (&self.phase, body) {
            (PhaseState::HandshakeRequest, parent_frame::Body::HandshakeRequest(handshake)) => {
                self.validate_handshake_request(handshake)?;
                self.phase = PhaseState::HandshakeResponse;
                Ok(())
            }
            (PhaseState::SolveRequest, parent_frame::Body::SolveRequest(solve)) => {
                let validated = validate_solve_request(solve, &self.negotiated_capabilities)?;
                self.phase = PhaseState::Started {
                    request_id: solve.request_id.clone(),
                    model_fingerprint: validated.model_fingerprint,
                    requested_projections: validated.requested_projections,
                    applied_parameters: validated.applied_parameters,
                };
                Ok(())
            }
            _ => Err(StateFault::UnexpectedParent {
                state: self.state_name(),
                message: parent_body_name(body),
            }
            .into()),
        }
    }

    /// Applies one owned worker frame to the session state.
    ///
    /// # Errors
    ///
    /// Returns a state or policy fault for malformed semantic evidence,
    /// handshake mismatch, stale identifiers, or an illegal transition.
    #[allow(clippy::too_many_lines)]
    pub fn on_worker_frame(
        &mut self,
        frame: WorkerFrame,
    ) -> Result<WorkerObservation, ProtocolFault> {
        let result = self.on_worker_frame_inner(frame);
        self.absorb_failure(result)
    }

    #[allow(clippy::too_many_lines)]
    fn on_worker_frame_inner(
        &mut self,
        mut frame: WorkerFrame,
    ) -> Result<WorkerObservation, ProtocolFault> {
        let body = frame.body.as_ref().ok_or(StateFault::MissingWorkerBody)?;
        match (&self.phase, body) {
            (PhaseState::HandshakeResponse, worker_frame::Body::HandshakeResponse(response)) => {
                let outcome = response
                    .outcome
                    .as_ref()
                    .ok_or(StateFault::MissingWorkerBody)?;
                match outcome {
                    Outcome::Success(success) => {
                        self.negotiated_capabilities = self.validate_handshake_success(success)?;
                        self.phase = PhaseState::SolveRequest;
                        Ok(WorkerObservation::HandshakeAccepted)
                    }
                    Outcome::Error(error) => {
                        let code = HandshakeErrorCode::try_from(error.code)
                            .map_err(|_| StateFault::InvalidWorkerField("handshake_error.code"))?;
                        if code == HandshakeErrorCode::Unspecified {
                            return Err(
                                StateFault::InvalidWorkerField("handshake_error.code").into()
                            );
                        }
                        validate_safe_diagnostic(&error.message, "handshake_error.message")?;
                        self.phase = PhaseState::Rejected(HandshakeRejectionEvidence {
                            error: error.clone(),
                        });
                        Ok(WorkerObservation::HandshakeRejected)
                    }
                }
            }
            (
                PhaseState::Started {
                    request_id,
                    model_fingerprint,
                    requested_projections: _,
                    applied_parameters: _,
                },
                worker_frame::Body::Started(started),
            ) => {
                validate_request_id(request_id, &started.request_id)?;
                if started.model_fingerprint.as_ref() != model_fingerprint.as_slice() {
                    return Err(StateFault::ModelFingerprint.into());
                }
                let phase = std::mem::replace(&mut self.phase, PhaseState::HandshakeRequest);
                match phase {
                    PhaseState::Started {
                        request_id,
                        model_fingerprint,
                        requested_projections,
                        applied_parameters,
                    } => {
                        self.phase = PhaseState::Running {
                            request_id,
                            model_fingerprint,
                            requested_projections,
                            applied_parameters,
                        };
                        take_started_observation(&mut frame)
                    }
                    phase => {
                        self.phase = phase;
                        Err(StateFault::UnexpectedWorker {
                            state: self.state_name(),
                            message: "started",
                        }
                        .into())
                    }
                }
            }
            (PhaseState::Running { request_id, .. }, worker_frame::Body::Progress(progress)) => {
                validate_request_id(request_id, &progress.request_id)?;
                validate_progress(progress, &self.negotiated_capabilities)?;
                take_progress_observation(&mut frame)
            }
            (
                PhaseState::Running {
                    request_id,
                    requested_projections,
                    ..
                },
                worker_frame::Body::Incumbent(incumbent),
            ) => {
                validate_request_id(request_id, &incumbent.request_id)?;
                require_negotiated_capability(
                    &self.negotiated_capabilities,
                    Capability::IntermediateSolutions,
                    "incumbent",
                )?;
                require_negotiated_capability(
                    &self.negotiated_capabilities,
                    Capability::SolutionProjection,
                    "incumbent.candidate",
                )?;
                if !incumbent.best_bound_values.is_empty() {
                    require_negotiated_capability(
                        &self.negotiated_capabilities,
                        Capability::ObjectiveBounds,
                        "incumbent.best_bound_values",
                    )?;
                }
                if incumbent.deterministic_time.is_some() {
                    require_negotiated_capability(
                        &self.negotiated_capabilities,
                        Capability::DeterministicTime,
                        "incumbent.deterministic_time",
                    )?;
                }
                validate_finite(&incumbent.objective_values, "incumbent.objective_values")?;
                validate_finite(&incumbent.best_bound_values, "incumbent.best_bound_values")?;
                validate_optional_nonnegative_finite(
                    incumbent.wall_time_seconds,
                    "incumbent.wall_time_seconds",
                )?;
                validate_optional_nonnegative_finite(
                    incumbent.deterministic_time,
                    "incumbent.deterministic_time",
                )?;
                let candidate = incumbent
                    .candidate
                    .as_ref()
                    .ok_or(StateFault::InvalidWorkerField("incumbent.candidate"))?;
                validate_candidate(candidate, requested_projections)?;
                take_incumbent_observation(&mut frame)
            }
            (
                PhaseState::Running {
                    request_id,
                    model_fingerprint,
                    requested_projections,
                    applied_parameters,
                },
                worker_frame::Body::Finished(finished),
            ) => {
                validate_request_id(request_id, &finished.request_id)?;
                let expected_applied_parameters_sha256 =
                    applied_parameters_sha256(applied_parameters);
                validate_finished(
                    finished,
                    model_fingerprint,
                    requested_projections,
                    &expected_applied_parameters_sha256,
                    &self.negotiated_capabilities,
                )?;
                compact_finished_frame(&mut frame);
                self.phase = PhaseState::Terminal(TerminalEvidence { frame });
                Ok(WorkerObservation::Terminal)
            }
            (PhaseState::Running { request_id, .. }, worker_frame::Body::Error(error)) => {
                validate_request_id(request_id, &error.request_id)?;
                let code = WorkerErrorCode::try_from(error.code)
                    .map_err(|_| StateFault::InvalidWorkerField("error.code"))?;
                if code == WorkerErrorCode::Unspecified {
                    return Err(StateFault::InvalidWorkerField("error.code").into());
                }
                validate_safe_diagnostic(&error.message, "error.message")?;
                self.phase = PhaseState::Terminal(TerminalEvidence { frame });
                Ok(WorkerObservation::Terminal)
            }
            _ => Err(StateFault::UnexpectedWorker {
                state: self.state_name(),
                message: worker_body_name(body),
            }
            .into()),
        }
    }

    /// Records clean stdout EOF after the terminal frame.
    ///
    /// # Errors
    ///
    /// Returns a state fault when EOF arrives before a terminal frame or is
    /// observed more than once.
    pub fn on_eof(&mut self) -> Result<(), ProtocolFault> {
        let result = self.on_eof_inner();
        self.absorb_failure(result)
    }

    fn on_eof_inner(&mut self) -> Result<(), ProtocolFault> {
        let phase = std::mem::replace(&mut self.phase, PhaseState::HandshakeRequest);
        match phase {
            PhaseState::Terminal(evidence) => {
                self.phase = PhaseState::Eof(evidence);
                Ok(())
            }
            PhaseState::Rejected(evidence) => {
                self.phase = PhaseState::RejectedEof(evidence);
                Ok(())
            }
            phase @ (PhaseState::HandshakeRequest
            | PhaseState::HandshakeResponse
            | PhaseState::SolveRequest
            | PhaseState::Started { .. }
            | PhaseState::Running { .. }
            | PhaseState::Failed) => {
                self.phase = phase;
                Err(StateFault::MissingTerminal.into())
            }
            phase @ (PhaseState::Eof(_)
            | PhaseState::Complete(_)
            | PhaseState::RejectedEof(_)
            | PhaseState::RejectedComplete(_)) => {
                let state = phase_state_name(&phase);
                self.phase = phase;
                Err(StateFault::UnexpectedWorker {
                    state,
                    message: "EOF",
                }
                .into())
            }
        }
    }

    /// Corroborates the terminal frame and EOF with the worker exit code.
    ///
    /// # Errors
    ///
    /// Returns a state fault for exit before EOF, nonzero exit after a terminal
    /// frame, missing terminal evidence, or duplicate exit observation.
    pub fn on_exit(&mut self, code: i32) -> Result<(), ProtocolFault> {
        let result = self.on_exit_inner(code);
        self.absorb_failure(result)
    }

    fn on_exit_inner(&mut self, code: i32) -> Result<(), ProtocolFault> {
        let phase = std::mem::replace(&mut self.phase, PhaseState::HandshakeRequest);
        match phase {
            PhaseState::Eof(evidence) if code == 0 => {
                self.phase = PhaseState::Complete(evidence);
                Ok(())
            }
            PhaseState::RejectedEof(evidence) if code == 0 => {
                self.phase = PhaseState::RejectedComplete(evidence);
                Ok(())
            }
            phase @ (PhaseState::Eof(_) | PhaseState::RejectedEof(_)) => {
                self.phase = phase;
                Err(StateFault::ContradictoryExit(code).into())
            }
            phase @ (PhaseState::Terminal(_) | PhaseState::Rejected(_)) => {
                self.phase = phase;
                Err(StateFault::ExitBeforeEof.into())
            }
            phase @ (PhaseState::Complete(_) | PhaseState::RejectedComplete(_)) => {
                self.phase = phase;
                Err(StateFault::DuplicateExit.into())
            }
            phase @ (PhaseState::HandshakeRequest
            | PhaseState::HandshakeResponse
            | PhaseState::SolveRequest
            | PhaseState::Started { .. }
            | PhaseState::Running { .. }
            | PhaseState::Failed) => {
                self.phase = phase;
                Err(StateFault::MissingTerminal.into())
            }
        }
    }

    fn absorb_failure<T>(&mut self, result: Result<T, ProtocolFault>) -> Result<T, ProtocolFault> {
        if result.is_err() {
            self.phase = PhaseState::Failed;
        }
        result
    }

    #[must_use]
    pub fn completion(&self) -> Option<&TerminalEvidence> {
        match &self.phase {
            PhaseState::Complete(evidence) => Some(evidence),
            _ => None,
        }
    }

    #[must_use]
    pub fn handshake_rejection(&self) -> Option<&HandshakeRejectionEvidence> {
        match &self.phase {
            PhaseState::RejectedComplete(evidence) => Some(evidence),
            _ => None,
        }
    }

    /// Consumes a fully corroborated session and returns its completed evidence.
    ///
    /// Evidence is unavailable until both clean stdout EOF and exit code zero
    /// have been observed. Every incomplete or failed phase returns a state
    /// fault instead of exposing provisional terminal data.
    ///
    /// # Errors
    ///
    /// Returns a completion-unavailable state fault unless the phase is
    /// `Complete` or `RejectedComplete`.
    pub fn into_completion(self) -> Result<CompletedSession, ProtocolFault> {
        match self.phase {
            PhaseState::Complete(evidence) => Ok(CompletedSession::Solve(Box::new(evidence))),
            PhaseState::RejectedComplete(evidence) => {
                Ok(CompletedSession::HandshakeRejected(evidence))
            }
            phase => Err(StateFault::CompletionUnavailable {
                state: phase_state_name(&phase),
            }
            .into()),
        }
    }

    fn validate_handshake_request(
        &self,
        handshake: &HandshakeRequest,
    ) -> Result<(), ProtocolFault> {
        self.validate_requested_protocol_version(
            handshake.protocol_major,
            handshake.protocol_minor,
        )?;
        let policy = checked_in_policy()?;
        validate_expected_text(
            policy,
            "eutheto.worker.v1.HandshakeRequest.core_version",
            "core_version",
            &handshake.core_version,
        )?;
        validate_semver("core_version", &handshake.core_version)?;
        if handshake.expected_backend_id.as_str() != self.expectations.backend_id.as_str() {
            return Err(StateFault::InvalidHandshake("backend_id").into());
        }
        if handshake.expected_manifest_sha256.len() != 32
            || handshake.expected_manifest_sha256.as_ref()
                != self.expectations.manifest_sha256.as_slice()
        {
            return Err(StateFault::InvalidHandshake("expected_manifest_sha256").into());
        }
        let required = validate_capability_values(&handshake.required_capabilities)?;
        if required != self.expectations.required_capabilities {
            return Err(StateFault::InvalidHandshake("required_capabilities").into());
        }
        Ok(())
    }

    fn validate_handshake_success(
        &self,
        success: &HandshakeSuccess,
    ) -> Result<BTreeSet<i32>, ProtocolFault> {
        self.validate_worker_protocol_version(success.protocol_major, success.protocol_minor)?;
        let policy = checked_in_policy()?;
        for (key, field, value) in [
            (
                "eutheto.worker.v1.HandshakeSuccess.worker_identity",
                "worker_identity",
                success.worker_identity.as_str(),
            ),
            (
                "eutheto.worker.v1.HandshakeSuccess.worker_version",
                "worker_version",
                success.worker_version.as_str(),
            ),
            (
                "eutheto.worker.v1.HandshakeSuccess.backend_id",
                "backend_id",
                success.backend_id.as_str(),
            ),
            (
                "eutheto.worker.v1.HandshakeSuccess.ortools_version",
                "ortools_version",
                success.ortools_version.as_str(),
            ),
            (
                "eutheto.worker.v1.HandshakeSuccess.adapter_version",
                "adapter_version",
                success.adapter_version.as_str(),
            ),
        ] {
            validate_expected_text(policy, key, field, value)?;
        }
        validate_semver("worker_version", &success.worker_version)?;
        validate_semver("ortools_version", &success.ortools_version)?;
        validate_semver("adapter_version", &success.adapter_version)?;
        if success.worker_identity.as_str() != self.expectations.worker_identity.as_str()
            || success.backend_id.as_str() != self.expectations.backend_id.as_str()
        {
            return Err(StateFault::BackendIdentity.into());
        }
        for (field, actual, expected) in [
            (
                "worker_version",
                success.worker_version.as_str(),
                self.expectations.worker_version.as_str(),
            ),
            (
                "ortools_version",
                success.ortools_version.as_str(),
                self.expectations.ortools_version.as_str(),
            ),
            (
                "adapter_version",
                success.adapter_version.as_str(),
                self.expectations.adapter_version.as_str(),
            ),
        ] {
            if actual != expected {
                return Err(StateFault::VersionMismatch(field).into());
            }
        }
        if success.manifest_sha256.as_ref() != self.expectations.manifest_sha256.as_slice() {
            return Err(StateFault::ManifestSha256.into());
        }
        let capabilities = validate_capability_values(&success.capabilities)?;
        if capabilities != self.expectations.expected_advertised_capabilities {
            return Err(StateFault::InvalidHandshake("advertised_capabilities").into());
        }
        Ok(capabilities)
    }

    fn validate_requested_protocol_version(
        &self,
        major: u32,
        minor: u32,
    ) -> Result<(), ProtocolFault> {
        if major != self.expectations.protocol_major || minor != self.expectations.protocol_minor {
            return Err(StateFault::ProtocolVersion {
                expected_major: self.expectations.protocol_major,
                expected_minor: self.expectations.protocol_minor,
                actual_major: major,
                actual_minor: minor,
            }
            .into());
        }
        Ok(())
    }

    fn validate_worker_protocol_version(
        &self,
        major: u32,
        minor: u32,
    ) -> Result<(), ProtocolFault> {
        if major != self.expectations.protocol_major || minor != self.expectations.protocol_minor {
            return Err(StateFault::ProtocolVersion {
                expected_major: self.expectations.protocol_major,
                expected_minor: self.expectations.protocol_minor,
                actual_major: major,
                actual_minor: minor,
            }
            .into());
        }
        Ok(())
    }

    fn state_name(&self) -> &'static str {
        phase_state_name(&self.phase)
    }
}

fn compact_finished_frame(frame: &mut WorkerFrame) {
    if let Some(worker_frame::Body::Finished(finished)) = &mut frame.body {
        finished.model_fingerprint = Bytes::copy_from_slice(&finished.model_fingerprint);
        finished.applied_parameters_sha256 =
            Bytes::copy_from_slice(&finished.applied_parameters_sha256);
    }
}

fn take_started_observation(frame: &mut WorkerFrame) -> Result<WorkerObservation, ProtocolFault> {
    let Some(worker_frame::Body::Started(started)) = frame.body.take() else {
        return Err(StateFault::MissingWorkerBody.into());
    };
    let model_fingerprint = started
        .model_fingerprint
        .as_ref()
        .try_into()
        .map_err(|_| StateFault::ModelFingerprint)?;
    Ok(WorkerObservation::Started(StartedObservation {
        request_id: started.request_id,
        model_fingerprint,
    }))
}

fn take_progress_observation(frame: &mut WorkerFrame) -> Result<WorkerObservation, ProtocolFault> {
    let Some(worker_frame::Body::Progress(progress)) = frame.body.take() else {
        return Err(StateFault::MissingWorkerBody.into());
    };
    Ok(WorkerObservation::Progress(progress))
}

fn take_incumbent_observation(frame: &mut WorkerFrame) -> Result<WorkerObservation, ProtocolFault> {
    let Some(worker_frame::Body::Incumbent(incumbent)) = frame.body.take() else {
        return Err(StateFault::MissingWorkerBody.into());
    };
    Ok(WorkerObservation::Incumbent(incumbent))
}

fn phase_state_name(phase: &PhaseState) -> &'static str {
    match phase {
        PhaseState::HandshakeRequest => "handshake-request",
        PhaseState::HandshakeResponse => "handshake-response",
        PhaseState::Rejected(_) => "rejected",
        PhaseState::RejectedEof(_) => "rejected-eof",
        PhaseState::RejectedComplete(_) => "rejected-complete",
        PhaseState::SolveRequest => "solve-request",
        PhaseState::Started { .. } => "started",
        PhaseState::Running { .. } => "running",
        PhaseState::Terminal(_) => "terminal",
        PhaseState::Eof(_) => "eof",
        PhaseState::Complete(_) => "complete",
        PhaseState::Failed => "failed",
    }
}
fn validate_expected_text(
    policy: &ProtocolPolicy,
    policy_key: &str,
    field: &'static str,
    value: &str,
) -> Result<(), ProtocolFault> {
    let cap = policy
        .field_limit(policy_key)
        .max_bytes()
        .unwrap_or(policy.max_string_bytes())
        .min(policy.max_string_bytes());
    if value.is_empty() || value.len() > cap {
        return Err(StateFault::InvalidHandshake(field).into());
    }
    Ok(())
}

fn validate_safe_diagnostic(value: &str, field: &'static str) -> Result<(), ProtocolFault> {
    if value.is_empty() || value.chars().any(is_unsafe_control) {
        return Err(StateFault::InvalidWorkerField(field).into());
    }
    Ok(())
}

fn validate_capability_values(values: &[i32]) -> Result<BTreeSet<i32>, ProtocolFault> {
    let mut capabilities = BTreeSet::new();
    for value in values {
        let capability = Capability::try_from(*value).map_err(|_| StateFault::InvalidCapability)?;
        if capability == Capability::Unspecified {
            return Err(StateFault::InvalidCapability.into());
        }
        if !SUFFICIENT_ASSUMPTIONS_ENABLED && capability == Capability::SufficientAssumptions {
            return Err(StateFault::CapabilityDisabled(*value).into());
        }
        if !capabilities.insert(*value) {
            return Err(StateFault::DuplicateCapability(*value).into());
        }
    }
    Ok(capabilities)
}

struct ValidatedSolveRequest {
    requested_projections: Vec<u64>,
    applied_parameters: NormalizedAppliedParameters,
    model_fingerprint: [u8; 32],
}

fn validate_solve_request(
    solve: &SolveRequest,
    negotiated_capabilities: &BTreeSet<i32>,
) -> Result<ValidatedSolveRequest, ProtocolFault> {
    require_negotiated_capability(negotiated_capabilities, Capability::CpSat, "solve_request")?;
    let policy = checked_in_policy()?;
    let request_cap = policy
        .field_limit("eutheto.worker.v1.SolveRequest.request_id")
        .max_bytes()
        .unwrap_or(policy.max_string_bytes())
        .min(policy.max_string_bytes());
    if solve.request_id.is_empty() || solve.request_id.len() > request_cap {
        return Err(StateFault::InvalidSolveRequest("request_id").into());
    }
    let model_cap = policy
        .field_limit("eutheto.worker.v1.SolveRequest.cp_model_proto")
        .max_bytes()
        .unwrap_or(policy.frame_cap(crate::limits::FrameClass::SolveRequest));
    if solve.cp_model_proto.len() > model_cap {
        return Err(StateFault::InvalidSolveRequest("cp_model_proto").into());
    }
    let normalized_parameters = normalize_applied_parameters(solve)?;
    let parameters = solve
        .parameters
        .as_ref()
        .ok_or(StateFault::InvalidSolveRequest("parameters"))?;
    if parameters.emit_intermediate_solutions == Some(true) {
        require_negotiated_capability(
            negotiated_capabilities,
            Capability::IntermediateSolutions,
            "solve_request.parameters.emit_intermediate_solutions",
        )?;
    }
    if parameters.log_search_progress == Some(true) {
        require_negotiated_capability(
            negotiated_capabilities,
            Capability::Progress,
            "solve_request.parameters.log_search_progress",
        )?;
    }
    let expected_model_fingerprint: [u8; 32] = Sha256::digest(&solve.cp_model_proto).into();
    if solve.model_fingerprint.as_ref() != expected_model_fingerprint.as_slice() {
        return Err(StateFault::InvalidSolveRequest("model_fingerprint").into());
    }
    let projection_cap = policy
        .field_limit("eutheto.worker.v1.SolveRequest.projections")
        .max_count()
        .unwrap_or(policy.max_repeated_field_items())
        .min(policy.max_repeated_field_items());
    if solve.projections.len() > projection_cap {
        return Err(StateFault::InvalidSolveRequest("projections").into());
    }
    if !solve.projections.is_empty() {
        require_negotiated_capability(
            negotiated_capabilities,
            Capability::SolutionProjection,
            "solve_request.projections",
        )?;
    }
    let mut requested = Vec::with_capacity(solve.projections.len());
    for projection in &solve.projections {
        if projection.cp_sat_variable_index < 0 {
            return Err(
                StateFault::InvalidSolveRequest("projections.cp_sat_variable_index").into(),
            );
        }
        requested.push(projection.projection_id);
    }
    requested.sort_unstable();
    if let Some(duplicate) = requested.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(StateFault::DuplicateProjection(duplicate[0]).into());
    }
    Ok(ValidatedSolveRequest {
        requested_projections: requested,
        model_fingerprint: expected_model_fingerprint,
        applied_parameters: normalized_parameters,
    })
}

fn require_negotiated_capability(
    negotiated_capabilities: &BTreeSet<i32>,
    capability: Capability,
    field: &'static str,
) -> Result<(), ProtocolFault> {
    let capability = capability as i32;
    if negotiated_capabilities.contains(&capability) {
        Ok(())
    } else {
        Err(StateFault::UnnegotiatedCapability { field, capability }.into())
    }
}

fn validate_progress(
    progress: &Progress,
    negotiated_capabilities: &BTreeSet<i32>,
) -> Result<(), ProtocolFault> {
    require_negotiated_capability(negotiated_capabilities, Capability::Progress, "progress")?;
    if !progress.best_bound_values.is_empty() {
        require_negotiated_capability(
            negotiated_capabilities,
            Capability::ObjectiveBounds,
            "progress.best_bound_values",
        )?;
    }
    if progress.deterministic_time.is_some() {
        require_negotiated_capability(
            negotiated_capabilities,
            Capability::DeterministicTime,
            "progress.deterministic_time",
        )?;
    }
    let kind = ProgressKind::try_from(progress.kind)
        .map_err(|_| StateFault::InvalidWorkerField("progress.kind"))?;
    if kind == ProgressKind::Unspecified {
        return Err(StateFault::InvalidWorkerField("progress.kind").into());
    }
    validate_finite(&progress.objective_values, "progress.objective_values")?;
    validate_finite(&progress.best_bound_values, "progress.best_bound_values")?;
    validate_optional_nonnegative_finite(progress.wall_time_seconds, "progress.wall_time_seconds")?;
    validate_optional_nonnegative_finite(progress.deterministic_time, "progress.deterministic_time")
}

fn validate_finished(
    finished: &Finished,
    model_fingerprint: &[u8],
    requested_projections: &[u64],
    expected_applied_parameters_sha256: &[u8; 32],
    negotiated_capabilities: &BTreeSet<i32>,
) -> Result<(), ProtocolFault> {
    if !(0..=4).contains(&finished.raw_cp_sat_status) {
        return Err(StateFault::InvalidWorkerField("finished.raw_cp_sat_status").into());
    }
    let status = WorkerSolveStatus::try_from(finished.status)
        .map_err(|_| StateFault::InvalidWorkerField("finished.status"))?;
    if status == WorkerSolveStatus::Unspecified {
        return Err(StateFault::InvalidWorkerField("finished.status").into());
    }
    let termination = TerminationReason::try_from(finished.termination_reason)
        .map_err(|_| StateFault::InvalidWorkerField("finished.termination_reason"))?;
    if termination == TerminationReason::Unspecified {
        return Err(StateFault::InvalidWorkerField("finished.termination_reason").into());
    }
    if !finished_outcome_is_coherent(
        finished.raw_cp_sat_status,
        status,
        termination,
        finished.final_candidate.is_some(),
    ) {
        return Err(StateFault::InvalidWorkerField("finished.outcome").into());
    }
    if finished.model_fingerprint.as_ref() != model_fingerprint {
        return Err(StateFault::ModelFingerprint.into());
    }
    if finished.applied_parameters_sha256.as_ref() != expected_applied_parameters_sha256 {
        return Err(StateFault::InvalidWorkerField("finished.applied_parameters_sha256").into());
    }
    validate_finite(&finished.objective_values, "finished.objective_values")?;
    validate_finite(&finished.best_bound_values, "finished.best_bound_values")?;
    if !finished.best_bound_values.is_empty() {
        require_negotiated_capability(
            negotiated_capabilities,
            Capability::ObjectiveBounds,
            "finished.best_bound_values",
        )?;
    }
    if finished.deterministic_time.is_some() {
        require_negotiated_capability(
            negotiated_capabilities,
            Capability::DeterministicTime,
            "finished.deterministic_time",
        )?;
    }
    if finished.conflicts.is_some()
        || finished.branches.is_some()
        || finished.binary_propagations.is_some()
        || finished.integer_propagations.is_some()
    {
        require_negotiated_capability(
            negotiated_capabilities,
            Capability::SolutionStats,
            "finished.solution_stats",
        )?;
    }
    for (value, field) in [
        (finished.wall_time_seconds, "finished.wall_time_seconds"),
        (finished.user_time_seconds, "finished.user_time_seconds"),
        (finished.deterministic_time, "finished.deterministic_time"),
    ] {
        validate_optional_nonnegative_finite(value, field)?;
    }
    if let Some(candidate) = &finished.final_candidate {
        require_negotiated_capability(
            negotiated_capabilities,
            Capability::SolutionProjection,
            "finished.final_candidate",
        )?;
        validate_candidate(candidate, requested_projections)?;
    }
    if !finished.sufficient_assumptions.is_empty() {
        if !SUFFICIENT_ASSUMPTIONS_ENABLED {
            return Err(
                StateFault::CapabilityDisabled(Capability::SufficientAssumptions as i32).into(),
            );
        }
        require_negotiated_capability(
            negotiated_capabilities,
            Capability::SufficientAssumptions,
            "finished.sufficient_assumptions",
        )?;
        if finished.raw_cp_sat_status != 3
            || status != WorkerSolveStatus::Infeasible
            || termination != TerminationReason::Infeasible
            || finished.final_candidate.is_some()
        {
            return Err(StateFault::InvalidWorkerField("finished.sufficient_assumptions").into());
        }
    }
    Ok(())
}

fn finished_outcome_is_coherent(
    raw_status: i32,
    status: WorkerSolveStatus,
    termination: TerminationReason,
    has_candidate: bool,
) -> bool {
    match raw_status {
        4 => {
            status == WorkerSolveStatus::Optimal
                && termination == TerminationReason::Optimal
                && has_candidate
        }
        3 => {
            status == WorkerSolveStatus::Infeasible
                && termination == TerminationReason::Infeasible
                && !has_candidate
        }
        2 => {
            status == WorkerSolveStatus::Feasible
                && matches!(
                    termination,
                    TerminationReason::TimeLimit
                        | TerminationReason::SolutionLimit
                        | TerminationReason::MemoryLimit
                        | TerminationReason::Cancelled
                )
                && has_candidate
        }
        1 => {
            status == WorkerSolveStatus::InvalidModel
                && termination == TerminationReason::InvalidModel
                && !has_candidate
        }
        0 => {
            (status == WorkerSolveStatus::NoSolution
                && matches!(
                    termination,
                    TerminationReason::TimeLimit
                        | TerminationReason::MemoryLimit
                        | TerminationReason::Unknown
                )
                && !has_candidate)
                || (status == WorkerSolveStatus::Cancelled
                    && termination == TerminationReason::Cancelled)
                || (status == WorkerSolveStatus::Failed
                    && termination == TerminationReason::InternalError
                    && !has_candidate)
        }
        _ => false,
    }
}

fn validate_candidate(
    candidate: &ProjectedCandidate,
    requested_projections: &[u64],
) -> Result<(), ProtocolFault> {
    let mut observed = Vec::with_capacity(candidate.values.len());
    observed.extend(candidate.values.iter().map(|value| value.projection_id));
    observed.sort_unstable();
    if let Some(duplicate) = observed.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(StateFault::DuplicateProjection(duplicate[0]).into());
    }
    if let Some(unrequested) = observed
        .iter()
        .find(|projection| requested_projections.binary_search(projection).is_err())
    {
        return Err(StateFault::UnrequestedProjection(*unrequested).into());
    }
    if let Some(missing) = requested_projections
        .iter()
        .find(|projection| observed.binary_search(projection).is_err())
    {
        return Err(StateFault::MissingProjection(*missing).into());
    }
    Ok(())
}

fn validate_finite(values: &[f64], field: &'static str) -> Result<(), ProtocolFault> {
    if values.iter().any(|value| !value.is_finite()) {
        return Err(StateFault::InvalidWorkerField(field).into());
    }
    Ok(())
}

fn validate_optional_nonnegative_finite(
    value: Option<f64>,
    field: &'static str,
) -> Result<(), ProtocolFault> {
    if value.is_some_and(|value| !value.is_finite() || value < 0.0) {
        return Err(StateFault::InvalidWorkerField(field).into());
    }
    Ok(())
}

fn validate_request_id(expected: &str, actual: &str) -> Result<(), ProtocolFault> {
    if expected != actual {
        return Err(StateFault::RequestId.into());
    }
    Ok(())
}

fn validate_semver(field: &'static str, value: &str) -> Result<(), ProtocolFault> {
    Version::parse(value)
        .map(|_| ())
        .map_err(|_| StateFault::InvalidVersion { field }.into())
}

fn parent_body_name(body: &parent_frame::Body) -> &'static str {
    match body {
        parent_frame::Body::HandshakeRequest(_) => "handshake-request",
        parent_frame::Body::SolveRequest(_) => "solve-request",
    }
}

fn worker_body_name(body: &worker_frame::Body) -> &'static str {
    match body {
        worker_frame::Body::HandshakeResponse(_) => "handshake-response",
        worker_frame::Body::Started(_) => "started",
        worker_frame::Body::Progress(_) => "progress",
        worker_frame::Body::Incumbent(_) => "incumbent",
        worker_frame::Body::Finished(_) => "finished",
        worker_frame::Body::Error(_) => "error",
    }
}
