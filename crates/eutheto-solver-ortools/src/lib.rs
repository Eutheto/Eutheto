#![forbid(unsafe_code)]

mod artifact;
mod backend;
mod candidate;
mod compatibility;
mod descriptor;
mod evidence;
mod progress;
mod status;
mod translation;

#[allow(clippy::all, clippy::pedantic)]
mod cp_sat {
    include!("generated/operations_research.sat.rs");
}

pub use artifact::{BundledWorkerArtifactError, VerifiedWorkerArtifact};
pub use backend::{OrToolsBackend, OrToolsBackendBuildError, registry_with_ortools};
pub use candidate::{CandidateDecodeError, DecodedCandidate, decode_projected_candidate};
pub use compatibility::ortools_compatibility;
pub use descriptor::{
    ORTOOLS_ADAPTER_VERSION, ORTOOLS_BACKEND_ID, ORTOOLS_VERSION, OrToolsDescriptorError,
    ortools_descriptor,
};
pub use evidence::{AdapterEvidenceError, AdapterEvidenceRecorder};
pub use progress::{
    OrToolsProgressError, backend_started_progress, bound_progress, candidate_progress,
    candidate_submission,
};
pub use status::{
    AdapterDefectKind, CandidateProofClaim, CandidateStopCause, NormalizedWorkerTerminal,
    WorkerFailureKind, WorkerLimitKind, WorkerUnavailableKind, normalize_terminal,
};
pub use translation::{
    CandidateProjectionVariable, TranslatedCpSatModel, TranslationError, VariableTranslation,
    translate_supported_model, translate_variable_domains,
};

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use eutheto_protocol::frame::{decode_worker_frame, encode_frame, write_solve_request_frame_async};
use eutheto_protocol::wire::{
    Capability, HandshakeRequest, ParentFrame, SolveRequest, WorkerFrame, parent_frame,
    worker_frame,
};
use eutheto_protocol::{
    BoundedStderr, CompletedSession, FrameClass, FrameFault, HandshakeExpectations, ParentProtocol,
    ProtocolFault, ProtocolPolicy, WorkerObservation, checked_in_policy,
};
use eutheto_types::{DurationMillis, SolveBudgetView};
#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use prost::bytes::Bytes;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

/// The fixed interval allowed for graceful Unix process-group termination.
pub const TERMINATION_GRACE: Duration = Duration::from_millis(250);
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(10);
const HASH_BUFFER_BYTES: usize = 64 * 1024;
const MAX_STAGED_FILE_BYTES: u64 = 128 * 1024 * 1024;

/// An executable whose exact bytes and manifest correlation were supplied by a
/// trusted manifest-validation boundary.
#[derive(Clone)]
pub struct VerifiedExecutable {
    path: PathBuf,
    executable_sha256: [u8; 32],
    executable_len: u64,
    manifest_sha256: [u8; 32],
    staged_relative_path: PathBuf,
    runtime_files: Vec<VerifiedRuntimeFile>,
}

#[derive(Clone)]
struct VerifiedRuntimeFile {
    path: PathBuf,
    sha256: [u8; 32],
    len: u64,
    staged_relative_path: PathBuf,
}

impl VerifiedExecutable {
    /// Checks path shape and executable bytes without following a final symlink.
    ///
    /// # Errors
    ///
    /// Returns a safe identity error when the path is not absolute, is a
    /// symlink or non-regular file, cannot be read, or hashes differently.
    pub async fn verify(
        path: impl Into<PathBuf>,
        executable_sha256: [u8; 32],
        manifest_sha256: [u8; 32],
    ) -> Result<Self, ExecutableIdentityError> {
        let path = path.into();
        let executable_len = verify_path_and_digest(&path, &executable_sha256).await?;
        #[cfg(unix)]
        let staged_relative_path = PathBuf::from("worker");
        #[cfg(windows)]
        let staged_relative_path = PathBuf::from("worker.exe");
        Ok(Self {
            path,
            executable_sha256,
            executable_len,
            manifest_sha256,
            staged_relative_path,
            runtime_files: Vec::new(),
        })
    }

    /// Returns the caller-supplied manifest digest used for handshake correlation.
    #[must_use]
    pub const fn manifest_sha256(&self) -> &[u8; 32] {
        &self.manifest_sha256
    }
}

impl fmt::Debug for VerifiedExecutable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VerifiedExecutable { identity: [redacted] }")
    }
}

/// Safe failures while checking an executable identity.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ExecutableIdentityError {
    #[error("worker executable path is not absolute")]
    NotAbsolute,
    #[error("worker executable path is a symbolic link")]
    SymbolicLink,
    #[error("worker executable is not a regular file")]
    NotRegularFile,
    #[error("worker executable identity could not be read: {0:?}")]
    Io(io::ErrorKind),
    #[error("worker artifact contains an invalid relative path")]
    InvalidRelativePath,
    #[error("worker executable digest does not match its approved identity")]
    DigestMismatch,
    #[error("worker artifact file exceeds the staging byte limit")]
    FileTooLarge,
    #[error("worker artifact file size changed after verification")]
    SizeMismatch,
}

/// Expected immutable identity and capability surface for one worker binary.
#[derive(Clone, Debug)]
pub struct WorkerIdentity {
    pub worker_identity: String,
    pub worker_version: String,
    pub backend_id: String,
    pub ortools_version: String,
    pub adapter_version: String,
    pub required_capabilities: Vec<Capability>,
    pub advertised_capabilities: Vec<Capability>,
}

/// Complete inputs for one fresh worker process and one solve session.
#[derive(Clone)]
pub struct SessionRequest {
    pub executable: VerifiedExecutable,
    pub identity: WorkerIdentity,
    pub core_version: String,
    pub solve: SolveRequest,
}

impl fmt::Debug for SessionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionRequest")
            .field("executable", &"[redacted]")
            .field(
                "required_capability_count",
                &self.identity.required_capabilities.len(),
            )
            .field("solve", &"[redacted]")
            .finish_non_exhaustive()
    }
}

/// Why a session stopped, without filesystem identity or worker diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafeStopReason {
    Completed,
    HandshakeRejected,
    Cancelled,
    DeadlineExceeded,
    ProtocolViolation,
    OutputLimit,
    Io,
    WorkerExit,
}

/// Parent-measured spans for a worker session that reached protocol execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerSessionTimings {
    pub startup_milliseconds: DurationMillis,
    pub handshake_milliseconds: DurationMillis,
    pub request_dispatch_milliseconds: Option<DurationMillis>,
    pub solver_milliseconds: Option<DurationMillis>,
    pub protocol_decode_milliseconds: DurationMillis,
    pub dispatched_wall_time_milliseconds: Option<DurationMillis>,
}

/// A fully corroborated normal protocol outcome and bounded sanitized stderr.
///
/// Solve terminal evidence remains untrusted: this type conveys no candidate
/// feasibility, model validity, or domain validity.
#[derive(Debug)]
pub struct SessionCompletion {
    pub evidence: CompletedSession,
    pub stderr: BoundedStderr,
    pub stop_reason: SafeStopReason,
    pub timings: WorkerSessionTimings,
}

/// A failed session after process-tree termination and reaping was attempted.
#[derive(Debug)]
pub struct SessionFailure {
    pub error: SupervisorError,
    pub stderr: BoundedStderr,
    pub stop_reason: SafeStopReason,
    /// Partial phase timings when the worker process reached protocol execution.
    pub timings: Option<WorkerSessionTimings>,
}

impl fmt::Display for SessionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for SessionFailure {}

/// Safe supervisor diagnostics. Paths, digests, and stderr are deliberately not
/// included in its `Display` representation.
#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error(transparent)]
    Executable(#[from] ExecutableIdentityError),
    #[error(transparent)]
    Protocol(#[from] ProtocolFault),
    #[error("worker process operation failed during {operation}: {kind:?}")]
    Io {
        operation: &'static str,
        kind: io::ErrorKind,
    },
    #[error("worker stdout exceeded the checked-in frame count limit")]
    FrameCountLimit,
    #[error("worker stdout exceeded the checked-in event count limit")]
    EventCountLimit,
    #[error("worker stdout exceeded the checked-in session byte limit")]
    SessionByteLimit,
    #[error("worker progress/incumbent rate exceeded the checked-in limit")]
    EventRateLimit,
    #[error("adapter rejected validated worker output")]
    AdapterOutput,
    #[error("solve was explicitly cancelled")]
    Cancelled,
    #[error("the original parent solve deadline elapsed")]
    DeadlineExceeded,
    #[error("worker exited without a portable exit code")]
    MissingExitCode,
    #[error("stderr drain task did not complete")]
    StderrDrain,
}

impl SupervisorError {
    fn stop_reason(&self) -> SafeStopReason {
        match self {
            Self::Cancelled => SafeStopReason::Cancelled,
            Self::DeadlineExceeded => SafeStopReason::DeadlineExceeded,
            Self::FrameCountLimit
            | Self::EventCountLimit
            | Self::SessionByteLimit
            | Self::EventRateLimit
            | Self::AdapterOutput
            | Self::Protocol(ProtocolFault::Frame(FrameFault::Oversized { .. })) => {
                SafeStopReason::OutputLimit
            }
            Self::Protocol(_) => SafeStopReason::ProtocolViolation,
            Self::MissingExitCode => SafeStopReason::WorkerExit,
            Self::Executable(_) | Self::Io { .. } | Self::StderrDrain => SafeStopReason::Io,
        }
    }

    fn graceful_stop(&self) -> bool {
        matches!(self, Self::Cancelled | Self::DeadlineExceeded)
    }
}

/// Launches a fresh no-argument worker and supervises exactly one protocol session.
///
/// The supplied budget remains tied to its original parent deadline across
/// executable revalidation, launch, handshake, request streaming, solve, EOF,
/// and process exit.
///
/// # Errors
///
/// Returns a safe failure after terminating and reaping the worker tree when
/// executable validation, process I/O, protocol validation, cancellation, or
/// the parent deadline prevents a corroborated completion.
#[allow(clippy::too_many_lines)]
pub async fn supervise(
    request: SessionRequest,
    budget: SolveBudgetView,
) -> Result<SessionCompletion, SessionFailure> {
    supervise_with_observer(request, budget, |_| Ok(())).await
}

/// Supervises one worker session and synchronously delivers each protocol-validated observation.
///
/// Returning an error from the observer terminates and reaps the worker before returning a safe
/// adapter-output failure.
///
/// # Errors
///
/// Returns the same failures as [`supervise`], plus observer rejection.
#[allow(clippy::too_many_lines)]
pub(crate) async fn supervise_with_observer<F>(
    request: SessionRequest,
    budget: SolveBudgetView,
    mut observer: F,
) -> Result<SessionCompletion, SessionFailure>
where
    F: FnMut(WorkerObservation) -> Result<(), SupervisorError> + Send,
{
    let remaining_at_entry = budget.remaining_duration();
    let startup_started = Instant::now();
    let policy = match checked_in_policy() {
        Ok(policy) => policy,
        Err(error) => return Err(prelaunch_failure(error.into())),
    };
    if let Err(error) = budget_checkpoint(&budget) {
        return Err(prelaunch_failure(error));
    }

    let expectations = match expectations(&request) {
        Ok(value) => value,
        Err(error) => return Err(prelaunch_failure(error.into())),
    };
    let handshake = handshake_frame(&request, policy);
    let directory = match private_worker_tempdir() {
        Ok(value) => value,
        Err(error) => {
            return Err(prelaunch_failure(io_error(
                "creating private worker directory",
                &error,
            )));
        }
    };
    let staged_executable = match budgeted(
        &budget,
        stage_verified_executable(&request.executable, directory.path()),
    )
    .await
    {
        Ok(path) => path,
        Err(BudgetedError::Cancelled) => {
            return Err(prelaunch_failure(SupervisorError::Cancelled));
        }
        Err(BudgetedError::DeadlineExceeded) => {
            return Err(prelaunch_failure(SupervisorError::DeadlineExceeded));
        }
        Err(BudgetedError::Inner(error)) => {
            return Err(prelaunch_failure(error.into()));
        }
    };
    if let Err(error) = budget_checkpoint(&budget) {
        return Err(prelaunch_failure(error));
    }
    let mut command = worker_command(
        &staged_executable,
        directory.path(),
        !request.executable.runtime_files.is_empty(),
    );
    let child = match command.spawn() {
        Ok(value) => value,
        Err(error) => {
            return Err(prelaunch_failure(io_error("spawning worker", &error)));
        }
    };
    let mut process = ProcessGuard::new(child, directory);
    if let Err(error) = budget_checkpoint(&budget) {
        return cleanup_failure(process, None, error, None).await;
    }
    let Some(stdin) = process.child_mut().stdin().take() else {
        return cleanup_failure(
            process,
            None,
            SupervisorError::Io {
                operation: "taking worker stdin",
                kind: io::ErrorKind::BrokenPipe,
            },
            None,
        )
        .await;
    };
    let Some(stdout) = process.child_mut().stdout().take() else {
        return cleanup_failure(
            process,
            None,
            SupervisorError::Io {
                operation: "taking worker stdout",
                kind: io::ErrorKind::BrokenPipe,
            },
            None,
        )
        .await;
    };
    let Some(stderr) = process.child_mut().stderr().take() else {
        return cleanup_failure(
            process,
            None,
            SupervisorError::Io {
                operation: "taking worker stderr",
                kind: io::ErrorKind::BrokenPipe,
            },
            None,
        )
        .await;
    };
    let stderr_task = tokio::spawn(drain_stderr(stderr, policy.max_stderr_bytes()));

    let startup_milliseconds = duration_millis(startup_started.elapsed());
    let execution = execute_session(
        process.child_mut(),
        WorkerPipes { stdin, stdout },
        &budget,
        policy,
        ProtocolSession {
            remaining_at_entry,
            expectations,
            handshake,
            solve: &request.solve,
        },
        &mut observer,
        startup_milliseconds,
    )
    .await;
    match execution {
        Ok(execution) => {
            process.kill_remaining_tree().await;
            let stderr = match budgeted(&budget, finish_stderr(stderr_task)).await {
                Ok(stderr) => stderr,
                Err(BudgetedError::Cancelled) => {
                    return cleanup_failure(
                        process,
                        None,
                        SupervisorError::Cancelled,
                        Some(execution.timings),
                    )
                    .await;
                }
                Err(BudgetedError::DeadlineExceeded) => {
                    return cleanup_failure(
                        process,
                        None,
                        SupervisorError::DeadlineExceeded,
                        Some(execution.timings),
                    )
                    .await;
                }
                Err(BudgetedError::Inner(error)) => {
                    return cleanup_failure(process, None, error, Some(execution.timings)).await;
                }
            };
            process.finish();
            let stop_reason = match execution.evidence {
                CompletedSession::Solve(_) => SafeStopReason::Completed,
                CompletedSession::HandshakeRejected(_) => SafeStopReason::HandshakeRejected,
            };
            Ok(SessionCompletion {
                evidence: execution.evidence,
                stderr,
                stop_reason,
                timings: execution.timings,
            })
        }
        Err(failure) => {
            cleanup_failure(
                process,
                Some(stderr_task),
                failure.error,
                Some(failure.timings),
            )
            .await
        }
    }
}

fn prelaunch_failure(error: SupervisorError) -> SessionFailure {
    let stop_reason = error.stop_reason();
    SessionFailure {
        error,
        stderr: empty_stderr(),
        stop_reason,
        timings: None,
    }
}

struct WorkerPipes {
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

struct ProtocolSession<'a> {
    remaining_at_entry: Duration,
    expectations: HandshakeExpectations,
    handshake: ParentFrame,
    solve: &'a SolveRequest,
}

struct ExecuteSessionResult {
    evidence: CompletedSession,
    timings: WorkerSessionTimings,
}

struct ExecuteSessionFailure {
    error: SupervisorError,
    timings: WorkerSessionTimings,
}

struct SessionTimingRecorder {
    handshake_started: Instant,
    handshake_milliseconds: Option<DurationMillis>,
    request_dispatch_started: Option<Instant>,
    request_dispatch_milliseconds: Option<DurationMillis>,
    solver_started: Option<Instant>,
    solver_milliseconds: Option<DurationMillis>,
    protocol_decode: Duration,
    dispatched_wall_time_milliseconds: Option<DurationMillis>,
}

impl SessionTimingRecorder {
    fn new() -> Self {
        Self {
            handshake_started: Instant::now(),
            handshake_milliseconds: None,
            request_dispatch_started: None,
            request_dispatch_milliseconds: None,
            solver_started: None,
            solver_milliseconds: None,
            protocol_decode: Duration::ZERO,
            dispatched_wall_time_milliseconds: None,
        }
    }

    fn record_handshake_complete(&mut self) {
        self.handshake_milliseconds = Some(duration_millis(self.handshake_started.elapsed()));
    }

    fn begin_request_dispatch(&mut self, dispatched_wall_time_milliseconds: DurationMillis) {
        self.dispatched_wall_time_milliseconds = Some(dispatched_wall_time_milliseconds);
        self.request_dispatch_started = Some(Instant::now());
    }

    fn record_request_dispatched(&mut self) {
        if self.request_dispatch_milliseconds.is_none() {
            self.request_dispatch_milliseconds = self
                .request_dispatch_started
                .map(|started| duration_millis(started.elapsed()));
        }
    }

    fn record_worker_started(&mut self) {
        if self.solver_started.is_none() {
            self.solver_started = Some(Instant::now());
        }
    }

    fn record_protocol_decode(&mut self, elapsed: Duration) {
        self.protocol_decode = self.protocol_decode.saturating_add(elapsed);
    }

    fn record_terminal(&mut self) {
        if self.solver_milliseconds.is_none() {
            self.solver_milliseconds = self
                .solver_started
                .map(|started| duration_millis(started.elapsed()));
        }
    }

    fn finish(&self, startup_milliseconds: DurationMillis) -> WorkerSessionTimings {
        WorkerSessionTimings {
            startup_milliseconds,
            handshake_milliseconds: self
                .handshake_milliseconds
                .unwrap_or_else(|| duration_millis(self.handshake_started.elapsed())),
            request_dispatch_milliseconds: self.request_dispatch_milliseconds.or_else(|| {
                self.request_dispatch_started
                    .map(|started| duration_millis(started.elapsed()))
            }),
            solver_milliseconds: self.solver_milliseconds.or_else(|| {
                self.solver_started
                    .map(|started| duration_millis(started.elapsed()))
            }),
            protocol_decode_milliseconds: duration_millis(self.protocol_decode),
            dispatched_wall_time_milliseconds: self.dispatched_wall_time_milliseconds,
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn execute_session<F>(
    child: &mut Box<dyn ChildWrapper>,
    pipes: WorkerPipes,
    budget: &SolveBudgetView,
    policy: &ProtocolPolicy,
    session: ProtocolSession<'_>,
    observer: &mut F,
    startup_milliseconds: DurationMillis,
) -> Result<ExecuteSessionResult, ExecuteSessionFailure>
where
    F: FnMut(WorkerObservation) -> Result<(), SupervisorError> + Send,
{
    let ProtocolSession {
        remaining_at_entry,
        expectations,
        handshake,
        solve,
    } = session;
    let WorkerPipes {
        mut stdin,
        mut stdout,
    } = pipes;
    let mut timings = SessionTimingRecorder::new();
    let result: Result<CompletedSession, SupervisorError> = async {
        let mut protocol = ParentProtocol::new(expectations);
        protocol.on_parent_frame(&handshake)?;
        let handshake_bytes = encode_frame(&handshake, FrameClass::Handshake, policy)?;
        budgeted(budget, stdin.write_all(&handshake_bytes))
            .await
            .map_err(|error| map_budgeted_io(error, "writing worker handshake"))?;

        let mut accounting = OutputAccounting::default();
        let Some((first_frame, first_bytes, first_decode)) = read_worker_frame(
            &mut stdout,
            FrameClass::Handshake,
            policy,
            budget,
            accounting.session_bytes,
            accounting.frames,
        )
        .await?
        else {
            protocol.on_eof()?;
            return Err(ProtocolFault::from(eutheto_protocol::StateFault::MissingTerminal).into());
        };
        timings.record_protocol_decode(first_decode);
        accounting.record_frame(first_bytes, false, policy)?;
        let observation = protocol.on_worker_frame(first_frame)?;
        timings.record_handshake_complete();
        match observation {
            WorkerObservation::HandshakeAccepted => {
                let mut parent_solve = ParentFrame {
                    body: Some(parent_frame::Body::SolveRequest(solve.clone())),
                };
                protocol.on_parent_frame(&parent_solve)?;
                let Some(parent_frame::Body::SolveRequest(dispatch_solve)) = &mut parent_solve.body
                else {
                    unreachable!("the solve frame was constructed immediately above");
                };
                let wall_time_millis =
                    finalize_solve_for_dispatch(dispatch_solve, budget, remaining_at_entry)?;
                protocol.tighten_started_wall_time_millis(wall_time_millis)?;
                timings.begin_request_dispatch(duration_millis(Duration::from_millis(
                    wall_time_millis,
                )));
                budgeted(
                    budget,
                    write_solve_request_frame_async(&mut stdin, dispatch_solve, policy),
                )
                .await
                .map_err(map_budgeted_protocol)?;
                budgeted(budget, stdin.shutdown())
                    .await
                    .map_err(|error| map_budgeted_io(error, "closing worker stdin"))?;
                drop(stdin);
                timings.record_request_dispatched();
            }
            WorkerObservation::HandshakeRejected => drop(stdin),
            _ => {
                return Err(
                    ProtocolFault::from(eutheto_protocol::StateFault::MissingTerminal).into(),
                );
            }
        }

        loop {
            if let Some((frame, bytes, decode_elapsed)) = read_worker_frame(
                &mut stdout,
                FrameClass::WorkerEvent,
                policy,
                budget,
                accounting.session_bytes,
                accounting.frames,
            )
            .await?
            {
                timings.record_protocol_decode(decode_elapsed);
                let intermediate = matches!(
                    &frame.body,
                    Some(worker_frame::Body::Progress(_) | worker_frame::Body::Incumbent(_))
                );
                accounting.record_frame(bytes, intermediate, policy)?;
                if intermediate {
                    accounting.record_intermediate_rate(Instant::now(), policy)?;
                }
                let observation = protocol.on_worker_frame(frame)?;
                if matches!(&observation, WorkerObservation::Started(_)) {
                    timings.record_worker_started();
                }
                if matches!(&observation, WorkerObservation::Terminal) {
                    timings.record_terminal();
                }
                observer(observation)?;
            } else {
                protocol.on_eof()?;
                break;
            }
        }

        let status = budgeted(budget, child.wait())
            .await
            .map_err(|error| map_budgeted_io(error, "waiting for worker exit"))?;
        let code = portable_exit_code(status)?;
        protocol.on_exit(code)?;
        protocol.into_completion().map_err(SupervisorError::from)
    }
    .await;
    let timings = timings.finish(startup_milliseconds);
    match result {
        Ok(evidence) => Ok(ExecuteSessionResult { evidence, timings }),
        Err(error) => Err(ExecuteSessionFailure { error, timings }),
    }
}

fn duration_millis(duration: Duration) -> DurationMillis {
    let milliseconds = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    DurationMillis::new(milliseconds).unwrap_or(DurationMillis::MAX)
}

#[derive(Default)]
struct OutputAccounting {
    frames: usize,
    events: usize,
    session_bytes: usize,
    rate_window: VecDeque<Instant>,
}

impl OutputAccounting {
    fn record_frame(
        &mut self,
        bytes: usize,
        intermediate: bool,
        policy: &ProtocolPolicy,
    ) -> Result<(), SupervisorError> {
        self.frames = self
            .frames
            .checked_add(1)
            .ok_or(SupervisorError::FrameCountLimit)?;
        if self.frames > policy.frames_per_session() {
            return Err(SupervisorError::FrameCountLimit);
        }
        self.session_bytes = self
            .session_bytes
            .checked_add(bytes)
            .ok_or(SupervisorError::SessionByteLimit)?;
        if self.session_bytes > policy.total_session_bytes() {
            return Err(SupervisorError::SessionByteLimit);
        }
        if intermediate {
            self.events = self
                .events
                .checked_add(1)
                .ok_or(SupervisorError::EventCountLimit)?;
            if self.events > policy.events_per_session() {
                return Err(SupervisorError::EventCountLimit);
            }
        }
        Ok(())
    }

    fn record_intermediate_rate(
        &mut self,
        now: Instant,
        policy: &ProtocolPolicy,
    ) -> Result<(), SupervisorError> {
        let cutoff = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
        while self
            .rate_window
            .front()
            .is_some_and(|instant| *instant <= cutoff)
        {
            self.rate_window.pop_front();
        }
        if self.rate_window.len() >= policy.events_per_second() {
            return Err(SupervisorError::EventRateLimit);
        }
        self.rate_window.push_back(now);
        Ok(())
    }
}

async fn read_worker_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    class: FrameClass,
    policy: &ProtocolPolicy,
    budget: &SolveBudgetView,
    accounted_bytes: usize,
    accounted_frames: usize,
) -> Result<Option<(WorkerFrame, usize, Duration)>, SupervisorError> {
    let mut prefix = [0u8; 4];
    let first = budgeted(budget, reader.read(&mut prefix[..1]))
        .await
        .map_err(|error| map_budgeted_io(error, "reading worker frame prefix"))?;
    if first == 0 {
        return Ok(None);
    }
    let prefix_remainder = read_exact_count(
        reader,
        &mut prefix[1..],
        budget,
        "reading worker frame prefix",
    )
    .await?;
    if prefix_remainder != 3 {
        return Err(ProtocolFault::from(FrameFault::TruncatedPrefix {
            received: 1 + prefix_remainder,
        })
        .into());
    }
    let length = usize::try_from(u32::from_be_bytes(prefix))
        .map_err(|_| ProtocolFault::from(FrameFault::LengthOverflow))?;
    if length == 0 {
        return Err(ProtocolFault::from(FrameFault::ZeroLength).into());
    }
    let cap = policy.frame_cap(class);
    if length > cap {
        return Err(ProtocolFault::from(FrameFault::Oversized {
            class: class.policy_name(),
            length,
            cap,
        })
        .into());
    }
    if accounted_frames >= policy.frames_per_session() {
        return Err(SupervisorError::FrameCountLimit);
    }
    let total_bytes = length
        .checked_add(4)
        .ok_or_else(|| ProtocolFault::from(FrameFault::LengthOverflow))?;
    let projected_session_bytes = accounted_bytes
        .checked_add(total_bytes)
        .ok_or(SupervisorError::SessionByteLimit)?;
    if projected_session_bytes > policy.total_session_bytes() {
        return Err(SupervisorError::SessionByteLimit);
    }
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(length)
        .map_err(|_| ProtocolFault::from(FrameFault::Allocation))?;
    payload.resize(length, 0);
    let received =
        read_exact_count(reader, &mut payload, budget, "reading worker frame payload").await?;
    if received != length {
        return Err(ProtocolFault::from(FrameFault::TruncatedPayload {
            declared: length,
            received,
        })
        .into());
    }
    let decode_started = Instant::now();
    let frame = decode_worker_frame(Bytes::from(payload), class)?;
    Ok(Some((frame, total_bytes, decode_started.elapsed())))
}

async fn read_exact_count<R: AsyncRead + Unpin>(
    reader: &mut R,
    mut output: &mut [u8],
    budget: &SolveBudgetView,
    operation: &'static str,
) -> Result<usize, SupervisorError> {
    let expected = output.len();
    while !output.is_empty() {
        let read = budgeted(budget, reader.read(output))
            .await
            .map_err(|error| map_budgeted_io(error, operation))?;
        if read == 0 {
            break;
        }
        output = &mut output[read..];
    }
    Ok(expected - output.len())
}

fn expectations(request: &SessionRequest) -> Result<HandshakeExpectations, ProtocolFault> {
    HandshakeExpectations::checked_in(
        request.identity.worker_identity.clone(),
        request.identity.worker_version.clone(),
        request.identity.backend_id.clone(),
        request.identity.ortools_version.clone(),
        request.identity.adapter_version.clone(),
        request.executable.manifest_sha256,
        request.identity.required_capabilities.iter().copied(),
        request.identity.advertised_capabilities.iter().copied(),
    )
}

fn handshake_frame(request: &SessionRequest, policy: &ProtocolPolicy) -> ParentFrame {
    ParentFrame {
        body: Some(parent_frame::Body::HandshakeRequest(HandshakeRequest {
            protocol_major: policy.protocol_major(),
            protocol_minor: policy.protocol_minor(),
            core_version: request.core_version.clone(),
            expected_backend_id: request.identity.backend_id.clone(),
            expected_manifest_sha256: Bytes::copy_from_slice(&request.executable.manifest_sha256),
            required_capabilities: request
                .identity
                .required_capabilities
                .iter()
                .map(|capability| *capability as i32)
                .collect(),
        })),
    }
}

fn worker_command(path: &Path, cwd: &Path, has_runtime_files: bool) -> CommandWrap {
    let mut command = CommandWrap::with_new(path.as_os_str(), |command| {
        command
            .env_clear()
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        command.env("TMPDIR", cwd);
        #[cfg(windows)]
        command.env("TEMP", cwd).env("TMP", cwd);
        if has_runtime_files {
            #[cfg(target_os = "linux")]
            command.env("LD_LIBRARY_PATH", cwd.join("lib"));
            #[cfg(target_os = "macos")]
            command.env("DYLD_LIBRARY_PATH", cwd.join("lib"));
        }
    });
    command.wrap(KillOnDrop);
    #[cfg(unix)]
    command.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(JobObject);
    command
}

fn private_worker_tempdir() -> io::Result<TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("eutheto-worker-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    // Windows relies on the current user's temporary-directory ACL. The child
    // receives only this already-selected path after its environment is cleared.
    builder.tempdir()
}

async fn stage_verified_executable(
    executable: &VerifiedExecutable,
    directory: &Path,
) -> Result<PathBuf, ExecutableIdentityError> {
    for runtime in &executable.runtime_files {
        stage_verified_file(
            &runtime.path,
            &runtime.sha256,
            runtime.len,
            &directory.join(&runtime.staged_relative_path),
            false,
        )
        .await?;
    }
    let staged_path = directory.join(&executable.staged_relative_path);
    stage_verified_file(
        &executable.path,
        &executable.executable_sha256,
        executable.executable_len,
        &staged_path,
        true,
    )
    .await?;
    Ok(staged_path)
}

async fn stage_verified_file(
    source_path: &Path,
    expected_sha256: &[u8; 32],
    expected_len: u64,
    staged_path: &Path,
    executable: bool,
) -> Result<(), ExecutableIdentityError> {
    let metadata = tokio::fs::symlink_metadata(source_path)
        .await
        .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    if is_link_like(&metadata) {
        return Err(ExecutableIdentityError::SymbolicLink);
    }
    if !metadata.is_file() {
        return Err(ExecutableIdentityError::NotRegularFile);
    }
    if metadata.len() != expected_len {
        return Err(ExecutableIdentityError::SizeMismatch);
    }
    let mut source_options = tokio::fs::OpenOptions::new();
    source_options.read(true);
    add_no_follow_flags(&mut source_options);
    let source = source_options
        .open(source_path)
        .await
        .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    let open_metadata = source
        .metadata()
        .await
        .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    if !open_metadata.is_file() {
        return Err(ExecutableIdentityError::NotRegularFile);
    }
    if is_link_like(&open_metadata) {
        return Err(ExecutableIdentityError::SymbolicLink);
    }
    if open_metadata.len() != expected_len {
        return Err(ExecutableIdentityError::SizeMismatch);
    }
    let parent = staged_path
        .parent()
        .ok_or(ExecutableIdentityError::InvalidRelativePath)?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    let mut staged = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(staged_path)
        .await
        .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    let mut source = source.take(expected_len.saturating_add(1));
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
    let mut copied = 0_u64;
    loop {
        let read = source
            .read(&mut buffer)
            .await
            .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(u64::try_from(read).map_err(|_| ExecutableIdentityError::SizeMismatch)?)
            .ok_or(ExecutableIdentityError::SizeMismatch)?;
        if copied > expected_len {
            return Err(ExecutableIdentityError::SizeMismatch);
        }
        digest.update(&buffer[..read]);
        staged
            .write_all(&buffer[..read])
            .await
            .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    }
    if copied != expected_len {
        return Err(ExecutableIdentityError::SizeMismatch);
    }
    staged
        .flush()
        .await
        .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    let actual: [u8; 32] = digest.finalize().into();
    if actual != *expected_sha256 {
        return Err(ExecutableIdentityError::DigestMismatch);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o700 } else { 0o600 };
        staged
            .set_permissions(std::fs::Permissions::from_mode(mode))
            .await
            .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    drop(staged);
    Ok(())
}

async fn verify_path_and_digest(
    path: &Path,
    expected: &[u8; 32],
) -> Result<u64, ExecutableIdentityError> {
    if !path.is_absolute() {
        return Err(ExecutableIdentityError::NotAbsolute);
    }
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    if is_link_like(&metadata) {
        return Err(ExecutableIdentityError::SymbolicLink);
    }
    if !metadata.is_file() {
        return Err(ExecutableIdentityError::NotRegularFile);
    }
    let len = metadata.len();
    if len > MAX_STAGED_FILE_BYTES {
        return Err(ExecutableIdentityError::FileTooLarge);
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    add_no_follow_flags(&mut options);
    let file = options
        .open(path)
        .await
        .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    let open_metadata = file
        .metadata()
        .await
        .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
    if is_link_like(&open_metadata) {
        return Err(ExecutableIdentityError::SymbolicLink);
    }
    if !open_metadata.is_file() {
        return Err(ExecutableIdentityError::NotRegularFile);
    }
    if open_metadata.len() != len {
        return Err(ExecutableIdentityError::SizeMismatch);
    }
    let mut file = file.take(len.saturating_add(1));
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
    let mut read_total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| ExecutableIdentityError::Io(error.kind()))?;
        if read == 0 {
            break;
        }
        read_total = read_total
            .checked_add(u64::try_from(read).map_err(|_| ExecutableIdentityError::SizeMismatch)?)
            .ok_or(ExecutableIdentityError::SizeMismatch)?;
        if read_total > len {
            return Err(ExecutableIdentityError::SizeMismatch);
        }
        digest.update(&buffer[..read]);
    }
    if read_total != len {
        return Err(ExecutableIdentityError::SizeMismatch);
    }
    let actual: [u8; 32] = digest.finalize().into();
    if actual != *expected {
        return Err(ExecutableIdentityError::DigestMismatch);
    }
    Ok(len)
}

fn add_no_follow_flags(options: &mut tokio::fs::OpenOptions) {
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = options;
}

fn is_link_like(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

async fn drain_stderr(
    mut stderr: tokio::process::ChildStderr,
    max_bytes: usize,
) -> Result<BoundedStderr, SupervisorError> {
    let mut bounded = BoundedStderr::new(max_bytes)?;
    let mut buffer = [0u8; 4096];
    loop {
        let read = stderr
            .read(&mut buffer)
            .await
            .map_err(|error| io_error("draining worker stderr", &error))?;
        if read == 0 {
            return Ok(bounded);
        }
        bounded.push(&buffer[..read]);
    }
}

async fn finish_stderr(
    task: JoinHandle<Result<BoundedStderr, SupervisorError>>,
) -> Result<BoundedStderr, SupervisorError> {
    task.await.map_err(|_| SupervisorError::StderrDrain)?
}

fn empty_stderr() -> BoundedStderr {
    match BoundedStderr::new(eutheto_protocol::STDERR_TRUNCATION_MARKER.len()) {
        Ok(stderr) => stderr,
        Err(_) => std::process::abort(),
    }
}

struct ProcessGuard {
    child: Option<Box<dyn ChildWrapper>>,
    directory: Option<TempDir>,
}

impl ProcessGuard {
    fn new(child: Box<dyn ChildWrapper>, directory: TempDir) -> Self {
        Self {
            child: Some(child),
            directory: Some(directory),
        }
    }

    fn child_mut(&mut self) -> &mut Box<dyn ChildWrapper> {
        match &mut self.child {
            Some(child) => child,
            None => std::process::abort(),
        }
    }

    fn finish(mut self) {
        drop(self.child.take());
        drop(self.directory.take());
    }

    async fn kill_remaining_tree(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }

    async fn stop_and_reap(&mut self, graceful: bool) {
        if let Some(child) = &mut self.child {
            stop_and_reap(child, graceful).await;
        }
        drop(self.child.take());
        drop(self.directory.take());
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let directory = self.directory.take();
        let _ = child.start_kill();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn(async move {
                let _ = child.wait().await;
                drop(directory);
            }));
        }
    }
}

async fn cleanup_failure(
    mut process: ProcessGuard,
    stderr_task: Option<JoinHandle<Result<BoundedStderr, SupervisorError>>>,
    error: SupervisorError,
    timings: Option<WorkerSessionTimings>,
) -> Result<SessionCompletion, SessionFailure> {
    process.stop_and_reap(error.graceful_stop()).await;
    let stderr = match stderr_task {
        Some(task) => match tokio::time::timeout(TERMINATION_GRACE, finish_stderr(task)).await {
            Ok(Ok(stderr)) => stderr,
            _ => empty_stderr(),
        },
        None => empty_stderr(),
    };
    process.finish();
    let stop_reason = error.stop_reason();
    Err(SessionFailure {
        error,
        stderr,
        stop_reason,
        timings,
    })
}

async fn stop_and_reap(child: &mut Box<dyn ChildWrapper>, graceful: bool) {
    #[cfg(unix)]
    if graceful {
        const SIGTERM: i32 = 15;
        let _ = child.signal(SIGTERM);
        tokio::time::sleep(TERMINATION_GRACE).await;
    }
    #[cfg(windows)]
    let _ = graceful;
    let _ = child.start_kill();
    let _ = child.wait().await;
}

fn portable_exit_code(status: ExitStatus) -> Result<i32, SupervisorError> {
    status.code().ok_or(SupervisorError::MissingExitCode)
}

fn io_error(operation: &'static str, error: &io::Error) -> SupervisorError {
    SupervisorError::Io {
        operation,
        kind: error.kind(),
    }
}

#[derive(Debug)]
enum BudgetedError<E> {
    Cancelled,
    DeadlineExceeded,
    Inner(E),
}

fn budget_checkpoint(budget: &SolveBudgetView) -> Result<(), SupervisorError> {
    let snapshot = budget.snapshot();
    if snapshot.cancelled {
        Err(SupervisorError::Cancelled)
    } else if snapshot.expired {
        Err(SupervisorError::DeadlineExceeded)
    } else {
        Ok(())
    }
}

fn finalize_solve_for_dispatch(
    solve: &mut SolveRequest,
    budget: &SolveBudgetView,
    remaining_at_entry: Duration,
) -> Result<u64, SupervisorError> {
    let Some(resource_limits) = solve.resource_limits.as_mut() else {
        return Err(
            ProtocolFault::from(eutheto_protocol::StateFault::InvalidSolveRequest(
                "resource_limits",
            ))
            .into(),
        );
    };
    if resource_limits.wall_time_millis == 0 {
        return Err(
            ProtocolFault::from(eutheto_protocol::StateFault::InvalidSolveRequest(
                "resource_limits.wall_time_millis",
            ))
            .into(),
        );
    }

    let snapshot = budget.snapshot();
    if snapshot.cancelled {
        return Err(SupervisorError::Cancelled);
    }
    let remaining_now = budget.remaining_duration();
    if snapshot.expired || remaining_now.is_zero() {
        return Err(SupervisorError::DeadlineExceeded);
    }
    let elapsed_before_dispatch = remaining_at_entry.saturating_sub(remaining_now);
    let remaining_worker_limit = Duration::from_millis(resource_limits.wall_time_millis)
        .saturating_sub(elapsed_before_dispatch)
        .min(remaining_now);
    let wall_time_millis = match u64::try_from(remaining_worker_limit.as_millis()) {
        Ok(value) if value > 0 => value,
        Ok(_) => return Err(SupervisorError::DeadlineExceeded),
        Err(_) => {
            return Err(ProtocolFault::Policy(
                "worker dispatch budget exceeds the protocol duration range".to_owned(),
            )
            .into());
        }
    };
    resource_limits.wall_time_millis = wall_time_millis;
    Ok(wall_time_millis)
}

async fn budgeted<F, T, E>(budget: &SolveBudgetView, future: F) -> Result<T, BudgetedError<E>>
where
    F: Future<Output = Result<T, E>>,
{
    tokio::pin!(future);
    loop {
        let snapshot = budget.snapshot();
        if snapshot.cancelled {
            return Err(BudgetedError::Cancelled);
        }
        if snapshot.expired {
            return Err(BudgetedError::DeadlineExceeded);
        }
        let pause = CANCELLATION_POLL_INTERVAL.min(budget.remaining_duration());
        tokio::select! {
            output = &mut future => {
                let snapshot = budget.snapshot();
                if snapshot.cancelled {
                    return Err(BudgetedError::Cancelled);
                }
                if snapshot.expired {
                    return Err(BudgetedError::DeadlineExceeded);
                }
                return output.map_err(BudgetedError::Inner);
            }
            () = tokio::time::sleep(pause) => {}
        }
    }
}

fn map_budgeted_protocol(error: BudgetedError<ProtocolFault>) -> SupervisorError {
    match error {
        BudgetedError::Cancelled => SupervisorError::Cancelled,
        BudgetedError::DeadlineExceeded => SupervisorError::DeadlineExceeded,
        BudgetedError::Inner(error) => SupervisorError::Protocol(error),
    }
}

fn map_budgeted_io(error: BudgetedError<io::Error>, operation: &'static str) -> SupervisorError {
    match error {
        BudgetedError::Cancelled => SupervisorError::Cancelled,
        BudgetedError::DeadlineExceeded => SupervisorError::DeadlineExceeded,
        BudgetedError::Inner(error) => io_error(operation, &error),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;
    use std::time::Duration;

    use super::{
        BudgetedError, OutputAccounting, SupervisorError, VerifiedExecutable, budgeted,
        finalize_solve_for_dispatch, private_worker_tempdir, stage_verified_executable,
        worker_command,
    };
    use eutheto_protocol::checked_in_policy;
    use eutheto_protocol::wire::{ResourceLimits, SolveRequest as WorkerSolveRequest};
    use eutheto_types::{
        CancellationToken, DurationMillis, FixedMonotonicClock, ParentSolveBudget,
    };
    use sha2::{Digest, Sha256};

    #[test]
    fn session_accepts_exact_intermediate_event_and_total_frame_caps()
    -> Result<(), eutheto_protocol::ProtocolFault> {
        let policy = checked_in_policy()?;
        assert_eq!(policy.events_per_session(), 4096);
        assert_eq!(policy.frames_per_session(), 4099);
        let mut accounting = OutputAccounting::default();
        accounting
            .record_frame(1, false, policy)
            .map_err(supervisor_fault)?;
        accounting
            .record_frame(1, false, policy)
            .map_err(supervisor_fault)?;
        for _ in 0..policy.events_per_session() {
            accounting
                .record_frame(1, true, policy)
                .map_err(supervisor_fault)?;
        }
        accounting
            .record_frame(1, false, policy)
            .map_err(supervisor_fault)?;
        assert_eq!(accounting.events, policy.events_per_session());
        assert_eq!(accounting.frames, policy.frames_per_session());
        Ok(())
    }

    #[test]
    fn session_rejects_one_intermediate_event_over_cap()
    -> Result<(), eutheto_protocol::ProtocolFault> {
        let policy = checked_in_policy()?;
        let mut accounting = OutputAccounting::default();
        accounting
            .record_frame(1, false, policy)
            .map_err(supervisor_fault)?;
        accounting
            .record_frame(1, false, policy)
            .map_err(supervisor_fault)?;
        for _ in 0..policy.events_per_session() {
            accounting
                .record_frame(1, true, policy)
                .map_err(supervisor_fault)?;
        }
        assert!(matches!(
            accounting.record_frame(1, true, policy),
            Err(SupervisorError::EventCountLimit)
        ));
        Ok(())
    }

    #[test]
    fn session_byte_cap_rejects_before_unbounded_output_accounting()
    -> Result<(), eutheto_protocol::ProtocolFault> {
        let policy = checked_in_policy()?;
        let mut accounting = OutputAccounting::default();
        assert!(matches!(
            accounting.record_frame(policy.total_session_bytes() + 1, false, policy),
            Err(SupervisorError::SessionByteLimit)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn ready_output_cannot_cross_the_deadline_boundary() -> Result<(), Box<dyn Error>> {
        let clock = FixedMonotonicClock::default();
        let parent = ParentSolveBudget::new(
            DurationMillis::new(1)?,
            Arc::new(clock.clone()),
            CancellationToken::new(),
        )?;
        let result = budgeted(&parent.phase_view(), async {
            clock.advance(Duration::from_millis(1))?;
            Ok::<_, eutheto_types::FixedMonotonicClockError>(())
        })
        .await;
        assert!(matches!(result, Err(BudgetedError::DeadlineExceeded)));
        Ok(())
    }

    #[tokio::test]
    async fn ready_output_cannot_cross_cancellation_boundary() -> Result<(), Box<dyn Error>> {
        let clock = FixedMonotonicClock::default();
        let cancellation = CancellationToken::new();
        let parent = ParentSolveBudget::new(
            DurationMillis::new(1)?,
            Arc::new(clock.clone()),
            cancellation.clone(),
        )?;
        let result = budgeted(&parent.phase_view(), async {
            clock.advance(Duration::from_millis(1))?;
            cancellation.cancel();
            Ok::<_, eutheto_types::FixedMonotonicClockError>(())
        })
        .await;
        assert!(matches!(result, Err(BudgetedError::Cancelled)));
        Ok(())
    }

    #[test]
    fn dispatch_uses_smaller_of_backend_cap_and_current_parent_budget() -> Result<(), Box<dyn Error>>
    {
        let clock = FixedMonotonicClock::default();
        let parent = ParentSolveBudget::new(
            DurationMillis::new(1_000)?,
            Arc::new(clock.clone()),
            CancellationToken::new(),
        )?;
        clock.advance(Duration::from_millis(250))?;

        let mut solve = WorkerSolveRequest {
            resource_limits: Some(ResourceLimits {
                wall_time_millis: 900,
                memory_bytes: None,
                worker_threads: 1,
            }),
            ..WorkerSolveRequest::default()
        };
        let wall_time_millis =
            finalize_solve_for_dispatch(&mut solve, &parent.phase_view(), Duration::from_secs(1))?;
        assert_eq!(wall_time_millis, 650);
        assert_eq!(
            solve
                .resource_limits
                .as_ref()
                .ok_or("missing dispatch resource limits")?
                .wall_time_millis,
            650
        );

        solve
            .resource_limits
            .as_mut()
            .ok_or("missing mutable resource limits")?
            .wall_time_millis = 500;
        let wall_time_millis =
            finalize_solve_for_dispatch(&mut solve, &parent.phase_view(), Duration::from_secs(1))?;
        assert_eq!(wall_time_millis, 250);
        Ok(())
    }

    #[test]
    fn submillisecond_dispatch_budget_stops_before_zero_limit_frame() -> Result<(), Box<dyn Error>>
    {
        let clock = FixedMonotonicClock::default();
        let parent = ParentSolveBudget::new(
            DurationMillis::new(1)?,
            Arc::new(clock.clone()),
            CancellationToken::new(),
        )?;
        clock.advance(Duration::from_micros(999))?;
        let mut solve = WorkerSolveRequest {
            resource_limits: Some(ResourceLimits {
                wall_time_millis: 1,
                memory_bytes: None,
                worker_threads: 1,
            }),
            ..WorkerSolveRequest::default()
        };
        assert!(matches!(
            finalize_solve_for_dispatch(&mut solve, &parent.phase_view(), Duration::from_millis(1),),
            Err(SupervisorError::DeadlineExceeded)
        ));
        Ok(())
    }

    #[test]
    fn zero_original_worker_limit_remains_a_protocol_fault() -> Result<(), Box<dyn Error>> {
        let clock = FixedMonotonicClock::default();
        let parent = ParentSolveBudget::new(
            DurationMillis::new(1_000)?,
            Arc::new(clock),
            CancellationToken::new(),
        )?;
        let mut solve = WorkerSolveRequest {
            resource_limits: Some(ResourceLimits {
                wall_time_millis: 0,
                memory_bytes: None,
                worker_threads: 1,
            }),
            ..WorkerSolveRequest::default()
        };
        assert!(matches!(
            finalize_solve_for_dispatch(&mut solve, &parent.phase_view(), Duration::from_secs(1),),
            Err(SupervisorError::Protocol(
                eutheto_protocol::ProtocolFault::State(
                    eutheto_protocol::StateFault::InvalidSolveRequest(
                        "resource_limits.wall_time_millis"
                    )
                )
            ))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn staged_copy_survives_original_replacement_and_has_private_modes()
    -> Result<(), Box<dyn Error>> {
        let source_directory = tempfile::tempdir()?;
        let source = source_directory.path().join("approved-worker");
        let approved = b"approved executable bytes";
        tokio::fs::write(&source, approved).await?;
        let executable = VerifiedExecutable {
            path: source.clone(),
            executable_sha256: Sha256::digest(approved).into(),
            executable_len: u64::try_from(approved.len())?,
            manifest_sha256: [0x5a; 32],
            staged_relative_path: std::path::PathBuf::from("worker"),
            runtime_files: Vec::new(),
        };
        let private_directory = private_worker_tempdir()?;
        let staged = stage_verified_executable(&executable, private_directory.path()).await?;
        tokio::fs::write(&source, b"hostile replacement").await?;
        assert_eq!(tokio::fs::read(&staged).await?, approved);
        let command = worker_command(&staged, private_directory.path(), false);
        assert_eq!(command.command().as_std().get_program(), staged.as_os_str());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let directory_mode = private_directory.path().metadata()?.permissions().mode() & 0o777;
            let executable_mode = staged.metadata()?.permissions().mode() & 0o777;
            assert_eq!(directory_mode, 0o700);
            assert_eq!(executable_mode, 0o700);
        }
        Ok(())
    }

    #[allow(clippy::needless_pass_by_value)]
    fn supervisor_fault(error: SupervisorError) -> eutheto_protocol::ProtocolFault {
        eutheto_protocol::ProtocolFault::Policy(error.to_string())
    }
}
