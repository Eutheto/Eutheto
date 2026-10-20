use prost::{Message, bytes::Bytes};
use sha2::{Digest, Sha256};

use eutheto_protocol::strict::{WireSchema, validate_checked_in};
use eutheto_protocol::wire::handshake_response::Outcome;
use eutheto_protocol::wire::{
    Capability, Finished, HandshakeError, HandshakeErrorCode, HandshakeRequest, HandshakeResponse,
    HandshakeSuccess, Incumbent, ParentFrame, Progress, ProgressKind, ProjectedCandidate,
    ProjectedValue, ProjectionRequest, ResourceLimits, SolveParameters, SolveRequest, Started,
    TerminationReason, WorkerError, WorkerErrorCode, WorkerFrame, WorkerSolveStatus, parent_frame,
    worker_frame,
};
use eutheto_protocol::{
    CompletedSession, FrameClass, HandshakeExpectations, NormalizedAppliedParameters, ParentPhase,
    ParentProtocol, ProtocolFault, ProtocolPolicy, StateFault, WorkerObservation,
    applied_parameters_sha256, checked_in_policy, normalize_applied_parameters,
};

const PARENT_MESSAGE: &str = "eutheto.worker.v1.ParentFrame";
const MANIFEST: [u8; 32] = [0x5a; 32];
const MODEL_BYTES: [u8; 6] = [0x12, 0x04, 0x12, 0x02, 0x00, 0x00];
const MODEL: [u8; 32] = [
    0x94, 0x17, 0x6f, 0x9b, 0x2c, 0xe2, 0xf9, 0xaf, 0x84, 0x32, 0x21, 0x5d, 0x87, 0x7f, 0x15, 0x03,
    0x13, 0x23, 0x85, 0xa4, 0xb1, 0x0f, 0xfd, 0x3f, 0x22, 0xd9, 0x85, 0x1d, 0xb2, 0x18, 0xac, 0x27,
];

fn expectations() -> Result<HandshakeExpectations, ProtocolFault> {
    let mut advertised = vec![
        Capability::CpSat,
        Capability::IntermediateSolutions,
        Capability::Progress,
        Capability::SolutionProjection,
        Capability::ObjectiveBounds,
        Capability::SolutionStats,
        Capability::DeterministicTime,
    ];
    if eutheto_protocol::limits::SUFFICIENT_ASSUMPTIONS_ENABLED {
        advertised.insert(6, Capability::SufficientAssumptions);
    }
    HandshakeExpectations::checked_in(
        "eutheto-ortools-worker",
        "0.1.0",
        "ortools-cp-sat",
        "9.15.6755",
        "0.1.0",
        MANIFEST,
        [Capability::CpSat, Capability::SolutionProjection],
        advertised,
    )
}

fn handshake_request() -> Result<ParentFrame, ProtocolFault> {
    let policy = checked_in_policy()?;
    Ok(ParentFrame {
        body: Some(parent_frame::Body::HandshakeRequest(HandshakeRequest {
            protocol_major: policy.protocol_major(),
            protocol_minor: policy.protocol_minor(),
            core_version: "0.1.0".to_owned(),
            expected_backend_id: "ortools-cp-sat".to_owned(),
            expected_manifest_sha256: Bytes::copy_from_slice(&MANIFEST),
            required_capabilities: vec![
                Capability::CpSat as i32,
                Capability::SolutionProjection as i32,
            ],
        })),
    })
}

fn handshake_success() -> Result<WorkerFrame, ProtocolFault> {
    let policy = checked_in_policy()?;
    let mut capabilities = vec![
        Capability::CpSat as i32,
        Capability::IntermediateSolutions as i32,
        Capability::Progress as i32,
        Capability::SolutionProjection as i32,
        Capability::ObjectiveBounds as i32,
        Capability::SolutionStats as i32,
        Capability::DeterministicTime as i32,
    ];
    if eutheto_protocol::limits::SUFFICIENT_ASSUMPTIONS_ENABLED {
        capabilities.insert(6, Capability::SufficientAssumptions as i32);
    }
    Ok(WorkerFrame {
        body: Some(worker_frame::Body::HandshakeResponse(HandshakeResponse {
            outcome: Some(Outcome::Success(HandshakeSuccess {
                protocol_major: policy.protocol_major(),
                protocol_minor: policy.protocol_minor(),
                worker_identity: "eutheto-ortools-worker".to_owned(),
                worker_version: "0.1.0".to_owned(),
                backend_id: "ortools-cp-sat".to_owned(),
                ortools_version: "9.15.6755".to_owned(),
                adapter_version: "0.1.0".to_owned(),
                manifest_sha256: Bytes::copy_from_slice(&MANIFEST),
                capabilities,
            })),
        })),
    })
}

fn solve_request() -> ParentFrame {
    ParentFrame {
        body: Some(parent_frame::Body::SolveRequest(SolveRequest {
            request_id: "request-1".to_owned(),
            cp_model_proto: Bytes::copy_from_slice(&MODEL_BYTES),
            parameters: Some(SolveParameters {
                emit_intermediate_solutions: Some(true),
                deterministic_test_profile: Some(true),
                ..SolveParameters::default()
            }),
            projections: vec![ProjectionRequest {
                projection_id: 7,
                cp_sat_variable_index: 0,
            }],
            resource_limits: Some(ResourceLimits {
                wall_time_millis: 1_000,
                memory_bytes: None,
                worker_threads: 1,
            }),
            model_fingerprint: Bytes::copy_from_slice(&MODEL),
        })),
    }
}

fn started(request_id: &str) -> WorkerFrame {
    WorkerFrame {
        body: Some(worker_frame::Body::Started(Started {
            request_id: request_id.to_owned(),
            model_fingerprint: Bytes::copy_from_slice(&MODEL),
        })),
    }
}

fn progress(request_id: &str) -> WorkerFrame {
    WorkerFrame {
        body: Some(worker_frame::Body::Progress(Progress {
            request_id: request_id.to_owned(),
            kind: ProgressKind::Search as i32,
            ..Progress::default()
        })),
    }
}

fn incumbent(request_id: &str) -> WorkerFrame {
    WorkerFrame {
        body: Some(worker_frame::Body::Incumbent(Incumbent {
            request_id: request_id.to_owned(),
            candidate: Some(ProjectedCandidate {
                values: vec![ProjectedValue {
                    projection_id: 7,
                    value: 42,
                }],
            }),
            ..Incumbent::default()
        })),
    }
}

fn finished(request_id: &str) -> WorkerFrame {
    WorkerFrame {
        body: Some(worker_frame::Body::Finished(Finished {
            request_id: request_id.to_owned(),
            raw_cp_sat_status: 2,
            status: WorkerSolveStatus::Feasible as i32,
            termination_reason: TerminationReason::TimeLimit as i32,
            final_candidate: Some(ProjectedCandidate {
                values: vec![ProjectedValue {
                    projection_id: 7,
                    value: 42,
                }],
            }),
            model_fingerprint: Bytes::copy_from_slice(&MODEL),
            applied_parameters_sha256: Bytes::copy_from_slice(&applied_parameters_sha256(
                &NormalizedAppliedParameters {
                    wall_time_millis: 1_000,
                    worker_threads: 1,
                    random_seed: 1,
                    stop_after_first_feasible: false,
                    emit_intermediate_solutions: true,
                    log_search_progress: false,
                    deterministic_test_profile: true,
                },
            )),
            ..Finished::default()
        })),
    }
}

fn solve_ready_protocol() -> Result<ParentProtocol, ProtocolFault> {
    let mut protocol = ParentProtocol::new(expectations()?);
    protocol.on_parent_frame(&handshake_request()?)?;
    protocol.on_worker_frame(handshake_success()?)?;
    Ok(protocol)
}

fn running_protocol_with_capabilities(
    required: &[Capability],
    advertised: &[Capability],
    mutate_solve: impl FnOnce(&mut SolveRequest),
) -> Result<(ParentProtocol, [u8; 32]), ProtocolFault> {
    let expectations = HandshakeExpectations::checked_in(
        "eutheto-ortools-worker",
        "0.1.0",
        "ortools-cp-sat",
        "9.15.6755",
        "0.1.0",
        MANIFEST,
        required.iter().copied(),
        advertised.iter().copied(),
    )?;
    let mut protocol = ParentProtocol::new(expectations);
    let mut request = handshake_request()?;
    if let Some(parent_frame::Body::HandshakeRequest(request)) = &mut request.body {
        request.required_capabilities = required
            .iter()
            .map(|capability| *capability as i32)
            .collect();
    }
    protocol.on_parent_frame(&request)?;
    let mut success = handshake_success()?;
    if let Some(worker_frame::Body::HandshakeResponse(response)) = &mut success.body
        && let Some(Outcome::Success(success)) = &mut response.outcome
    {
        success.capabilities = advertised
            .iter()
            .map(|capability| *capability as i32)
            .collect();
    }
    protocol.on_worker_frame(success)?;
    let mut solve = solve_request();
    let normalized = match &mut solve.body {
        Some(parent_frame::Body::SolveRequest(solve)) => {
            mutate_solve(solve);
            normalize_applied_parameters(solve)?
        }
        _ => return Err(StateFault::MissingParentBody.into()),
    };
    let hash = applied_parameters_sha256(&normalized);
    protocol.on_parent_frame(&solve)?;
    protocol.on_worker_frame(started("request-1"))?;
    Ok((protocol, hash))
}

fn running_protocol() -> Result<ParentProtocol, ProtocolFault> {
    let mut protocol = solve_ready_protocol()?;
    protocol.on_parent_frame(&solve_request())?;
    protocol.on_worker_frame(started("request-1"))?;
    Ok(protocol)
}

fn assert_running_rejects(frame: WorkerFrame) -> Result<(), ProtocolFault> {
    let mut protocol = running_protocol()?;

    assert!(protocol.on_worker_frame(frame).is_err());
    assert_eq!(protocol.phase(), ParentPhase::Failed);
    Ok(())
}
#[test]
fn terminal_evidence_compacts_borrowed_bytes() -> Result<(), ProtocolFault> {
    let mut protocol = running_protocol()?;
    let frame = finished("request-1");
    let (fingerprint_pointer, parameters_pointer) = match &frame.body {
        Some(worker_frame::Body::Finished(finished)) => (
            finished.model_fingerprint.as_ptr() as usize,
            finished.applied_parameters_sha256.as_ptr() as usize,
        ),
        _ => return Err(StateFault::MissingWorkerBody.into()),
    };
    protocol.on_worker_frame(frame)?;
    protocol.on_eof()?;
    protocol.on_exit(0)?;
    let stored = protocol
        .completion()
        .and_then(|evidence| evidence.frame().body.as_ref())
        .ok_or(StateFault::MissingTerminal)?;
    let worker_frame::Body::Finished(stored) = stored else {
        return Err(StateFault::MissingTerminal.into());
    };
    assert_ne!(
        stored.model_fingerprint.as_ptr() as usize,
        fingerprint_pointer
    );
    assert_ne!(
        stored.applied_parameters_sha256.as_ptr() as usize,
        parameters_pointer
    );
    Ok(())
}

#[test]
fn consuming_completion_requires_eof_and_zero_exit() -> Result<(), ProtocolFault> {
    let mut premature = running_protocol()?;
    premature.on_worker_frame(finished("request-1"))?;
    assert!(matches!(
        premature.into_completion(),
        Err(ProtocolFault::State(StateFault::CompletionUnavailable {
            state: "terminal"
        }))
    ));

    let mut complete = running_protocol()?;
    complete.on_worker_frame(finished("request-1"))?;
    complete.on_eof()?;
    complete.on_exit(0)?;
    assert!(matches!(
        complete.into_completion()?,
        CompletedSession::Solve(_)
    ));
    Ok(())
}

#[test]
fn protocol_debug_omits_identity_hashes_messages_and_model_evidence() -> Result<(), ProtocolFault> {
    let expectations_debug = format!("{:?}", expectations()?);
    assert!(!expectations_debug.contains("eutheto-ortools-worker"));
    assert!(!expectations_debug.contains("5a5a5a"));

    let mut protocol = running_protocol()?;
    protocol.on_worker_frame(WorkerFrame {
        body: Some(worker_frame::Body::Error(WorkerError {
            request_id: "request-1".to_owned(),
            code: WorkerErrorCode::Internal as i32,
            message: "representative-secret-message".to_owned(),
            retryable: false,
        })),
    })?;
    protocol.on_eof()?;
    protocol.on_exit(0)?;
    let parent_debug = format!("{protocol:?}");
    assert_eq!(parent_debug, "ParentProtocol { phase: Complete }");
    let completion_debug = format!("{:?}", protocol.into_completion()?);
    assert!(!completion_debug.contains("representative-secret-message"));
    assert!(!completion_debug.contains("request-1"));
    assert_eq!(completion_debug, "CompletedSession::Solve");
    Ok(())
}

#[test]
fn every_legal_parent_transition_reaches_consistent_completion() -> Result<(), ProtocolFault> {
    let mut protocol = ParentProtocol::new(expectations()?);
    assert_eq!(protocol.phase(), ParentPhase::HandshakeRequest);
    protocol.on_parent_frame(&handshake_request()?)?;
    assert_eq!(protocol.phase(), ParentPhase::HandshakeResponse);
    assert_eq!(
        protocol.on_worker_frame(handshake_success()?)?,
        WorkerObservation::HandshakeAccepted
    );
    protocol.on_parent_frame(&solve_request())?;
    assert_eq!(protocol.phase(), ParentPhase::Started);
    let WorkerObservation::Started(started) = protocol.on_worker_frame(started("request-1"))?
    else {
        return Err(StateFault::MissingWorkerBody.into());
    };
    assert_eq!(started.request_id, "request-1");
    assert_eq!(started.model_fingerprint, MODEL);
    let WorkerObservation::Progress(progress) = protocol.on_worker_frame(progress("request-1"))?
    else {
        return Err(StateFault::MissingWorkerBody.into());
    };
    assert_eq!(progress.kind, ProgressKind::Search as i32);
    let WorkerObservation::Incumbent(incumbent) =
        protocol.on_worker_frame(incumbent("request-1"))?
    else {
        return Err(StateFault::MissingWorkerBody.into());
    };
    assert_eq!(incumbent.request_id, "request-1");
    let terminal = finished("request-1");
    assert_eq!(
        protocol.on_worker_frame(terminal)?,
        WorkerObservation::Terminal
    );
    assert_eq!(protocol.phase(), ParentPhase::Terminal);
    protocol.on_eof()?;
    assert_eq!(protocol.phase(), ParentPhase::Eof);
    protocol.on_exit(0)?;
    assert_eq!(protocol.phase(), ParentPhase::Complete);
    assert!(matches!(
        protocol
            .completion()
            .map(|value| value.frame().body.as_ref()),
        Some(Some(worker_frame::Body::Finished(_)))
    ));
    Ok(())
}

#[test]
fn tightened_wall_time_becomes_the_terminal_parameter_contract() -> Result<(), ProtocolFault> {
    let mut protocol = solve_ready_protocol()?;
    protocol.on_parent_frame(&solve_request())?;
    protocol.tighten_started_wall_time_millis(500)?;
    protocol.on_worker_frame(started("request-1"))?;

    let mut terminal = finished("request-1");
    let Some(worker_frame::Body::Finished(finished)) = &mut terminal.body else {
        return Err(StateFault::MissingWorkerBody.into());
    };
    finished.applied_parameters_sha256 =
        Bytes::copy_from_slice(&applied_parameters_sha256(&NormalizedAppliedParameters {
            wall_time_millis: 500,
            worker_threads: 1,
            random_seed: 1,
            stop_after_first_feasible: false,
            emit_intermediate_solutions: true,
            log_search_progress: false,
            deterministic_test_profile: true,
        }));

    assert_eq!(
        protocol.on_worker_frame(terminal)?,
        WorkerObservation::Terminal
    );
    Ok(())
}

#[test]
fn wall_time_tightening_rejects_zero_and_increases() -> Result<(), ProtocolFault> {
    let mut zero = solve_ready_protocol()?;
    zero.on_parent_frame(&solve_request())?;
    assert!(zero.tighten_started_wall_time_millis(0).is_err());
    assert_eq!(zero.phase(), ParentPhase::Failed);

    let mut increase = solve_ready_protocol()?;
    increase.on_parent_frame(&solve_request())?;
    assert!(increase.tighten_started_wall_time_millis(1_001).is_err());
    assert_eq!(increase.phase(), ParentPhase::Failed);
    Ok(())
}

#[test]
fn stale_repeated_and_out_of_order_frames_are_absorbing() -> Result<(), ProtocolFault> {
    let mut before_started = solve_ready_protocol()?;
    before_started.on_parent_frame(&solve_request())?;
    assert!(
        before_started
            .on_worker_frame(progress("request-1"))
            .is_err()
    );
    assert_eq!(before_started.phase(), ParentPhase::Failed);
    assert!(
        before_started
            .on_worker_frame(started("request-1"))
            .is_err()
    );
    assert!(before_started.completion().is_none());

    let mut wrong_started_state = solve_ready_protocol()?;
    wrong_started_state.on_parent_frame(&solve_request())?;
    let mut wrong_fingerprint = started("request-1");
    if let Some(worker_frame::Body::Started(started)) = &mut wrong_fingerprint.body {
        started.model_fingerprint = Bytes::from_static(&[0; 32]);
    }
    assert!(matches!(
        wrong_started_state.on_worker_frame(wrong_fingerprint),
        Err(ProtocolFault::State(StateFault::ModelFingerprint))
    ));
    assert_eq!(wrong_started_state.phase(), ParentPhase::Failed);

    let mut stale = running_protocol()?;
    assert!(matches!(
        stale.on_worker_frame(progress("stale")),
        Err(ProtocolFault::State(StateFault::RequestId))
    ));
    assert_eq!(stale.phase(), ParentPhase::Failed);
    assert!(stale.on_worker_frame(progress("request-1")).is_err());
    assert!(stale.on_worker_frame(finished("request-1")).is_err());
    assert!(stale.completion().is_none());

    let mut repeated_terminal = running_protocol()?;
    repeated_terminal.on_worker_frame(finished("request-1"))?;
    assert!(
        repeated_terminal
            .on_worker_frame(finished("request-1"))
            .is_err()
    );
    assert_eq!(repeated_terminal.phase(), ParentPhase::Failed);
    Ok(())
}

fn assert_solve_rejected(frame: &ParentFrame) -> Result<(), ProtocolFault> {
    let mut protocol = solve_ready_protocol()?;
    assert!(protocol.on_parent_frame(frame).is_err());
    assert_eq!(protocol.phase(), ParentPhase::Failed);
    Ok(())
}

#[test]
fn solve_request_semantics_are_bounded_and_complete() -> Result<(), ProtocolFault> {
    let valid = solve_request();

    let mut missing_parameters = valid.clone();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut missing_parameters.body {
        solve.parameters = None;
    }
    assert_solve_rejected(&missing_parameters)?;

    let mut missing_limits = valid.clone();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut missing_limits.body {
        solve.resource_limits = None;
    }
    assert_solve_rejected(&missing_limits)?;

    let mut zero_budget = valid.clone();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut zero_budget.body
        && let Some(limits) = &mut solve.resource_limits
    {
        limits.wall_time_millis = 0;
    }
    assert_solve_rejected(&zero_budget)?;

    let mut too_many_threads = valid.clone();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut too_many_threads.body
        && let Some(limits) = &mut solve.resource_limits
    {
        limits.worker_threads = 10_001;
    }
    assert_solve_rejected(&too_many_threads)?;

    let mut deterministic_conflict = valid.clone();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut deterministic_conflict.body
        && let Some(parameters) = &mut solve.parameters
    {
        parameters.random_seed = Some(2);
    }
    assert_solve_rejected(&deterministic_conflict)?;

    let mut negative_index = valid.clone();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut negative_index.body {
        solve.projections[0].cp_sat_variable_index = -1;
    }
    assert_solve_rejected(&negative_index)?;

    let mut duplicate_projection = valid.clone();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut duplicate_projection.body {
        solve.projections.push(solve.projections[0]);
    }
    assert_solve_rejected(&duplicate_projection)?;

    let mut wrong_fingerprint = valid.clone();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut wrong_fingerprint.body {
        solve.model_fingerprint = Bytes::from_static(&[0; 32]);
    }
    assert_solve_rejected(&wrong_fingerprint)?;

    let mut changed_model = valid.clone();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut changed_model.body {
        let mut changed = MODEL_BYTES;
        changed[0] ^= 1;
        solve.cp_model_proto = Bytes::copy_from_slice(&changed);
    }
    assert_solve_rejected(&changed_model)?;

    let mut maximum_threads = valid.clone();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut maximum_threads.body
        && let (Some(parameters), Some(limits)) =
            (&mut solve.parameters, &mut solve.resource_limits)
    {
        parameters.deterministic_test_profile = Some(false);
        limits.worker_threads = 10_000;
    }
    let mut maximum_protocol = solve_ready_protocol()?;
    maximum_protocol.on_parent_frame(&maximum_threads)?;

    let mut empty_model = valid;
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut empty_model.body {
        solve.cp_model_proto.clear();
        solve.model_fingerprint = Bytes::copy_from_slice(&Sha256::digest([]));
    }
    let mut empty_protocol = solve_ready_protocol()?;
    empty_protocol.on_parent_frame(&empty_model)?;
    assert_eq!(empty_protocol.phase(), ParentPhase::Started);
    Ok(())
}

#[test]
fn applied_parameter_hash_is_canonical_and_field_complete() -> Result<(), ProtocolFault> {
    let golden = NormalizedAppliedParameters {
        wall_time_millis: 100,
        worker_threads: 1,
        random_seed: 1,
        stop_after_first_feasible: false,
        emit_intermediate_solutions: true,
        log_search_progress: false,
        deterministic_test_profile: true,
    };
    assert_eq!(
        applied_parameters_sha256(&golden),
        [
            0x58, 0xa0, 0x2d, 0x86, 0xa3, 0xb5, 0x7c, 0x8a, 0x13, 0x28, 0x65, 0xae, 0x6f, 0xb5,
            0x26, 0x0c, 0x4c, 0xbe, 0xf5, 0xab, 0x24, 0x3b, 0xcb, 0x93, 0x7d, 0x16, 0x76, 0x4b,
            0xfa, 0x9e, 0xc4, 0xf1,
        ]
    );
    let mut without_memory = solve_request();
    let mut with_memory = solve_request();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut with_memory.body
        && let Some(limits) = &mut solve.resource_limits
    {
        limits.memory_bytes = Some(1024);
    }
    let without_memory = match &mut without_memory.body {
        Some(parent_frame::Body::SolveRequest(solve)) => normalize_applied_parameters(solve)?,
        _ => return Err(StateFault::MissingParentBody.into()),
    };
    let with_memory = match &mut with_memory.body {
        Some(parent_frame::Body::SolveRequest(solve)) => normalize_applied_parameters(solve)?,
        _ => return Err(StateFault::MissingParentBody.into()),
    };
    assert_eq!(
        applied_parameters_sha256(&without_memory),
        applied_parameters_sha256(&with_memory)
    );

    let mut omitted = solve_request();
    let mut explicit = solve_request();
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut omitted.body
        && let Some(parameters) = &mut solve.parameters
    {
        parameters.random_seed = None;
        parameters.stop_after_first_feasible = None;
        parameters.log_search_progress = None;
    }
    if let Some(parent_frame::Body::SolveRequest(solve)) = &mut explicit.body
        && let Some(parameters) = &mut solve.parameters
    {
        parameters.random_seed = Some(1);
        parameters.stop_after_first_feasible = Some(false);
        parameters.log_search_progress = Some(false);
    }
    let omitted = match &omitted.body {
        Some(parent_frame::Body::SolveRequest(solve)) => normalize_applied_parameters(solve)?,
        _ => return Err(StateFault::MissingParentBody.into()),
    };
    let explicit = match &explicit.body {
        Some(parent_frame::Body::SolveRequest(solve)) => normalize_applied_parameters(solve)?,
        _ => return Err(StateFault::MissingParentBody.into()),
    };
    assert_eq!(omitted, explicit);

    let baseline = applied_parameters_sha256(&golden);
    for changed in [
        NormalizedAppliedParameters {
            wall_time_millis: 101,
            ..golden
        },
        NormalizedAppliedParameters {
            worker_threads: 2,
            deterministic_test_profile: false,
            ..golden
        },
        NormalizedAppliedParameters {
            random_seed: 2,
            deterministic_test_profile: false,
            ..golden
        },
        NormalizedAppliedParameters {
            stop_after_first_feasible: true,
            ..golden
        },
        NormalizedAppliedParameters {
            emit_intermediate_solutions: false,
            ..golden
        },
        NormalizedAppliedParameters {
            log_search_progress: true,
            ..golden
        },
        NormalizedAppliedParameters {
            deterministic_test_profile: false,
            ..golden
        },
    ] {
        assert_ne!(applied_parameters_sha256(&changed), baseline);
    }
    Ok(())
}

#[test]
fn worker_event_semantics_reject_invalid_numbers_and_candidates() -> Result<(), ProtocolFault> {
    assert_running_rejects(WorkerFrame {
        body: Some(worker_frame::Body::Progress(Progress {
            request_id: "request-1".to_owned(),
            ..Progress::default()
        })),
    })?;

    let mut nonfinite_objective = progress("request-1");
    if let Some(worker_frame::Body::Progress(progress)) = &mut nonfinite_objective.body {
        progress.objective_values.push(f64::NAN);
    }
    assert_running_rejects(nonfinite_objective)?;

    let mut infinite_time = progress("request-1");
    if let Some(worker_frame::Body::Progress(progress)) = &mut infinite_time.body {
        progress.wall_time_seconds = Some(f64::INFINITY);
    }
    assert_running_rejects(infinite_time)?;

    assert_running_rejects(WorkerFrame {
        body: Some(worker_frame::Body::Incumbent(Incumbent {
            request_id: "request-1".to_owned(),
            ..Incumbent::default()
        })),
    })?;

    let mut unrequested = incumbent("request-1");
    if let Some(worker_frame::Body::Incumbent(incumbent)) = &mut unrequested.body
        && let Some(candidate) = &mut incumbent.candidate
    {
        candidate.values[0].projection_id = 8;
    }
    assert_running_rejects(unrequested)?;

    let mut duplicate = incumbent("request-1");
    if let Some(worker_frame::Body::Incumbent(incumbent)) = &mut duplicate.body
        && let Some(candidate) = &mut incumbent.candidate
    {
        candidate.values.push(candidate.values[0]);
    }
    assert_running_rejects(duplicate)?;
    let mut incomplete = incumbent("request-1");
    if let Some(worker_frame::Body::Incumbent(incumbent)) = &mut incomplete.body
        && let Some(candidate) = &mut incumbent.candidate
    {
        candidate.values.clear();
    }
    assert_running_rejects(incomplete)?;
    Ok(())
}

#[test]
fn negative_objectives_and_bounds_are_valid_solver_evidence() -> Result<(), ProtocolFault> {
    let mut protocol = running_protocol()?;
    let mut signed_progress = progress("request-1");
    if let Some(worker_frame::Body::Progress(progress)) = &mut signed_progress.body {
        progress.objective_values.push(-10.0);
        progress.best_bound_values.push(-11.0);
    }
    let WorkerObservation::Progress(signed_progress) = protocol.on_worker_frame(signed_progress)?
    else {
        return Err(StateFault::MissingWorkerBody.into());
    };
    assert_eq!(signed_progress.objective_values, [-10.0]);
    assert_eq!(signed_progress.best_bound_values, [-11.0]);

    let mut signed_finished = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut signed_finished.body {
        finished.objective_values.push(-10.0);
        finished.best_bound_values.push(-11.0);
    }
    assert_eq!(
        protocol.on_worker_frame(signed_finished)?,
        WorkerObservation::Terminal
    );
    Ok(())
}

#[test]
fn worker_fields_require_negotiated_capabilities() -> Result<(), ProtocolFault> {
    let quiet = |solve: &mut SolveRequest| {
        if let Some(parameters) = &mut solve.parameters {
            parameters.emit_intermediate_solutions = Some(false);
            parameters.log_search_progress = Some(false);
            parameters.deterministic_test_profile = Some(false);
        }
    };
    let minimal = [Capability::CpSat, Capability::SolutionProjection];
    let (mut no_progress, _) = running_protocol_with_capabilities(&minimal, &minimal, quiet)?;
    assert!(no_progress.on_worker_frame(progress("request-1")).is_err());
    assert_eq!(no_progress.phase(), ParentPhase::Failed);

    let progress_caps = [
        Capability::CpSat,
        Capability::Progress,
        Capability::SolutionProjection,
    ];
    let (mut no_intermediate, _) =
        running_protocol_with_capabilities(&minimal, &progress_caps, quiet)?;
    assert!(
        no_intermediate
            .on_worker_frame(incumbent("request-1"))
            .is_err()
    );

    let (mut no_bounds, _) = running_protocol_with_capabilities(&minimal, &progress_caps, quiet)?;
    let mut bounded = progress("request-1");
    if let Some(worker_frame::Body::Progress(progress)) = &mut bounded.body {
        progress.best_bound_values.push(1.0);
    }
    assert!(no_bounds.on_worker_frame(bounded).is_err());

    let (mut no_deterministic_time, _) =
        running_protocol_with_capabilities(&minimal, &progress_caps, quiet)?;
    let mut deterministic = progress("request-1");
    if let Some(worker_frame::Body::Progress(progress)) = &mut deterministic.body {
        progress.deterministic_time = Some(1.0);
    }
    assert!(
        no_deterministic_time
            .on_worker_frame(deterministic)
            .is_err()
    );

    let no_stats_caps = [
        Capability::CpSat,
        Capability::IntermediateSolutions,
        Capability::Progress,
        Capability::SolutionProjection,
        Capability::ObjectiveBounds,
        Capability::DeterministicTime,
    ];
    let (mut no_stats, expected_hash) =
        running_protocol_with_capabilities(&minimal, &no_stats_caps, quiet)?;
    let mut with_stats = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut with_stats.body {
        finished.applied_parameters_sha256 = Bytes::copy_from_slice(&expected_hash);
        finished.conflicts = Some(1);
    }
    assert!(no_stats.on_worker_frame(with_stats).is_err());

    let (mut no_projection, expected_hash) =
        running_protocol_with_capabilities(&[Capability::CpSat], &[Capability::CpSat], |solve| {
            quiet(solve);
            solve.projections.clear();
        })?;
    let mut with_candidate = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut with_candidate.body {
        finished.applied_parameters_sha256 = Bytes::copy_from_slice(&expected_hash);
    }
    assert!(no_projection.on_worker_frame(with_candidate).is_err());
    Ok(())
}

#[test]
fn unknown_event_enums_fail_semantically_and_absorb() -> Result<(), ProtocolFault> {
    let mut unknown_progress = progress("request-1");
    if let Some(worker_frame::Body::Progress(progress)) = &mut unknown_progress.body {
        progress.kind = 99;
    }
    validate_checked_in(
        "eutheto.worker.v1.WorkerFrame",
        &unknown_progress.encode_to_vec(),
    )?;
    assert_running_rejects(unknown_progress)?;

    let mut unknown_status = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut unknown_status.body {
        finished.status = 99;
    }
    validate_checked_in(
        "eutheto.worker.v1.WorkerFrame",
        &unknown_status.encode_to_vec(),
    )?;
    assert_running_rejects(unknown_status)
}

#[test]
fn finished_raw_and_normalized_outcomes_are_coherent() -> Result<(), ProtocolFault> {
    let cases = [
        (
            4,
            WorkerSolveStatus::Optimal,
            TerminationReason::Optimal,
            true,
            true,
        ),
        (
            3,
            WorkerSolveStatus::Infeasible,
            TerminationReason::Infeasible,
            false,
            true,
        ),
        (
            2,
            WorkerSolveStatus::Feasible,
            TerminationReason::TimeLimit,
            true,
            true,
        ),
        (
            1,
            WorkerSolveStatus::InvalidModel,
            TerminationReason::InvalidModel,
            false,
            true,
        ),
        (
            0,
            WorkerSolveStatus::NoSolution,
            TerminationReason::TimeLimit,
            false,
            true,
        ),
        (
            0,
            WorkerSolveStatus::Cancelled,
            TerminationReason::Cancelled,
            true,
            true,
        ),
        (
            0,
            WorkerSolveStatus::Failed,
            TerminationReason::InternalError,
            false,
            true,
        ),
        (
            2,
            WorkerSolveStatus::Optimal,
            TerminationReason::TimeLimit,
            true,
            false,
        ),
        (
            2,
            WorkerSolveStatus::Feasible,
            TerminationReason::TimeLimit,
            false,
            false,
        ),
        (
            0,
            WorkerSolveStatus::NoSolution,
            TerminationReason::TimeLimit,
            true,
            false,
        ),
        (
            4,
            WorkerSolveStatus::Optimal,
            TerminationReason::Optimal,
            false,
            false,
        ),
    ];

    for (raw, status, termination, has_candidate, expected_valid) in cases {
        let mut frame = finished("request-1");
        if let Some(worker_frame::Body::Finished(finished)) = &mut frame.body {
            finished.raw_cp_sat_status = raw;
            finished.status = status as i32;
            finished.termination_reason = termination as i32;
            if !has_candidate {
                finished.final_candidate = None;
            }
        }
        let mut protocol = running_protocol()?;
        assert_eq!(protocol.on_worker_frame(frame).is_ok(), expected_valid);
    }
    Ok(())
}

#[test]
fn terminal_semantics_reject_partial_or_contradictory_evidence() -> Result<(), ProtocolFault> {
    assert_running_rejects(WorkerFrame {
        body: Some(worker_frame::Body::Error(WorkerError {
            request_id: "request-1".to_owned(),
            ..WorkerError::default()
        })),
    })?;
    assert_running_rejects(WorkerFrame {
        body: Some(worker_frame::Body::Error(WorkerError {
            request_id: "request-1".to_owned(),
            code: WorkerErrorCode::Internal as i32,
            message: String::new(),
            retryable: false,
        })),
    })?;

    assert_running_rejects(WorkerFrame {
        body: Some(worker_frame::Body::Error(WorkerError {
            request_id: "request-1".to_owned(),
            code: WorkerErrorCode::Internal as i32,
            message: "unsafe\u{202e}diagnostic".to_owned(),
            retryable: false,
        })),
    })?;

    assert_running_rejects(WorkerFrame {
        body: Some(worker_frame::Body::Finished(Finished {
            request_id: "request-1".to_owned(),
            model_fingerprint: Bytes::copy_from_slice(&MODEL),
            applied_parameters_sha256: Bytes::copy_from_slice(&applied_parameters_sha256(
                &NormalizedAppliedParameters {
                    wall_time_millis: 1_000,
                    worker_threads: 1,
                    random_seed: 1,
                    stop_after_first_feasible: false,
                    emit_intermediate_solutions: true,
                    log_search_progress: false,
                    deterministic_test_profile: true,
                },
            )),
            ..Finished::default()
        })),
    })?;

    let mut invalid_raw_status = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut invalid_raw_status.body {
        finished.raw_cp_sat_status = 5;
    }
    assert_running_rejects(invalid_raw_status)?;

    let mut wrong_fingerprint = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut wrong_fingerprint.body {
        finished.model_fingerprint = Bytes::from_static(&[0; 32]);
    }
    assert_running_rejects(wrong_fingerprint)?;

    let mut short_parameter_hash = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut short_parameter_hash.body {
        finished.applied_parameters_sha256 = Bytes::from_static(&[0; 31]);
    }
    assert_running_rejects(short_parameter_hash)?;
    let mut mismatched_parameter_hash = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut mismatched_parameter_hash.body {
        finished.applied_parameters_sha256 = Bytes::from_static(&[0; 32]);
    }
    assert_running_rejects(mismatched_parameter_hash)?;

    let mut incomplete_final_candidate = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut incomplete_final_candidate.body
        && let Some(candidate) = &mut finished.final_candidate
    {
        candidate.values.clear();
    }
    assert_running_rejects(incomplete_final_candidate)?;

    let mut negative_time = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut negative_time.body {
        finished.deterministic_time = Some(-1.0);
    }
    assert_running_rejects(negative_time)?;

    Ok(())
}

#[test]
fn assumption_core_requires_negotiated_infeasible_terminal() -> Result<(), ProtocolFault> {
    let required = [Capability::CpSat, Capability::SufficientAssumptions];
    let advertised = [
        Capability::CpSat,
        Capability::IntermediateSolutions,
        Capability::Progress,
        Capability::SolutionProjection,
        Capability::ObjectiveBounds,
        Capability::SolutionStats,
        Capability::SufficientAssumptions,
        Capability::DeterministicTime,
    ];
    let mut core_frame = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut core_frame.body {
        finished.raw_cp_sat_status = 3;
        finished.status = WorkerSolveStatus::Infeasible as i32;
        finished.termination_reason = TerminationReason::Infeasible as i32;
        finished.final_candidate = None;
        finished.sufficient_assumptions.push(1);
    }
    let without_core = advertised
        .iter()
        .copied()
        .filter(|capability| *capability != Capability::SufficientAssumptions)
        .collect::<Vec<_>>();
    let (mut unnegotiated, _) =
        running_protocol_with_capabilities(&[Capability::CpSat], &without_core, |_| {})?;
    assert!(unnegotiated.on_worker_frame(core_frame.clone()).is_err());
    assert_eq!(unnegotiated.phase(), ParentPhase::Failed);

    let (mut valid, _) = running_protocol_with_capabilities(&required, &advertised, |_| {})?;
    assert_eq!(
        valid.on_worker_frame(core_frame.clone())?,
        WorkerObservation::Terminal
    );
    valid.on_eof()?;
    valid.on_exit(0)?;
    assert_eq!(valid.phase(), ParentPhase::Complete);

    let (mut wrong_status, _) = running_protocol_with_capabilities(&required, &advertised, |_| {})?;
    let mut wrong_status_frame = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut wrong_status_frame.body {
        finished.sufficient_assumptions.push(1);
    }
    assert!(wrong_status.on_worker_frame(wrong_status_frame).is_err());
    assert_eq!(wrong_status.phase(), ParentPhase::Failed);

    let (mut wrong_candidate, _) =
        running_protocol_with_capabilities(&required, &advertised, |_| {})?;
    let mut wrong_candidate_frame = finished("request-1");
    if let Some(worker_frame::Body::Finished(finished)) = &mut wrong_candidate_frame.body {
        finished.raw_cp_sat_status = 3;
        finished.status = WorkerSolveStatus::Infeasible as i32;
        finished.termination_reason = TerminationReason::Infeasible as i32;
        finished.sufficient_assumptions.push(1);
    }
    assert!(
        wrong_candidate
            .on_worker_frame(wrong_candidate_frame)
            .is_err()
    );
    assert_eq!(wrong_candidate.phase(), ParentPhase::Failed);
    Ok(())
}

#[test]
fn worker_error_is_the_alternative_solve_terminal() -> Result<(), ProtocolFault> {
    let mut protocol = running_protocol()?;
    let terminal = WorkerFrame {
        body: Some(worker_frame::Body::Error(WorkerError {
            request_id: "request-1".to_owned(),
            code: WorkerErrorCode::Internal as i32,
            message: "bounded failure".to_owned(),
            retryable: false,
        })),
    };
    assert_eq!(
        protocol.on_worker_frame(terminal)?,
        WorkerObservation::Terminal
    );
    protocol.on_eof()?;
    protocol.on_exit(0)?;
    assert!(matches!(
        protocol
            .completion()
            .map(|value| value.frame().body.as_ref()),
        Some(Some(worker_frame::Body::Error(_)))
    ));
    Ok(())
}

#[test]
fn terminal_eof_and_exit_must_all_agree() -> Result<(), ProtocolFault> {
    let mut missing = running_protocol()?;
    assert!(matches!(
        missing.on_eof(),
        Err(ProtocolFault::State(StateFault::MissingTerminal))
    ));
    assert_eq!(missing.phase(), ParentPhase::Failed);
    assert!(missing.on_worker_frame(finished("request-1")).is_err());
    assert!(missing.on_eof().is_err());
    assert!(missing.on_exit(0).is_err());
    assert!(missing.completion().is_none());

    let mut no_eof = running_protocol()?;
    no_eof.on_worker_frame(finished("request-1"))?;
    assert!(matches!(
        no_eof.on_exit(0),
        Err(ProtocolFault::State(StateFault::ExitBeforeEof))
    ));
    assert_eq!(no_eof.phase(), ParentPhase::Failed);
    assert!(no_eof.on_eof().is_err());
    assert!(no_eof.on_exit(0).is_err());

    let mut contradictory = running_protocol()?;
    contradictory.on_worker_frame(finished("request-1"))?;
    contradictory.on_eof()?;
    assert!(matches!(
        contradictory.on_exit(70),
        Err(ProtocolFault::State(StateFault::ContradictoryExit(70)))
    ));
    assert_eq!(contradictory.phase(), ParentPhase::Failed);
    assert!(contradictory.on_exit(0).is_err());
    assert!(contradictory.completion().is_none());

    let mut duplicate = running_protocol()?;
    duplicate.on_worker_frame(finished("request-1"))?;
    duplicate.on_eof()?;
    duplicate.on_exit(0)?;
    assert!(matches!(
        duplicate.on_exit(0),
        Err(ProtocolFault::State(StateFault::DuplicateExit))
    ));
    assert_eq!(duplicate.phase(), ParentPhase::Failed);
    Ok(())
}

fn assert_handshake_rejected(
    mutate: impl FnOnce(&mut HandshakeSuccess),
) -> Result<(), ProtocolFault> {
    let mut state = ParentProtocol::new(expectations()?);
    state.on_parent_frame(&handshake_request()?)?;
    let mut response = handshake_success()?;
    let success = match &mut response.body {
        Some(worker_frame::Body::HandshakeResponse(response)) => match &mut response.outcome {
            Some(Outcome::Success(success)) => success,
            _ => return Err(StateFault::MissingWorkerBody.into()),
        },
        _ => return Err(StateFault::MissingWorkerBody.into()),
    };
    mutate(success);
    assert!(state.on_worker_frame(response).is_err());
    Ok(())
}

#[test]
fn handshake_mismatches_fail_before_solve() -> Result<(), ProtocolFault> {
    let mut wrong_request = handshake_request()?;
    if let Some(parent_frame::Body::HandshakeRequest(request)) = &mut wrong_request.body {
        request.core_version = "not-semver".to_owned();
    }
    let mut protocol = ParentProtocol::new(expectations()?);
    assert!(matches!(
        protocol.on_parent_frame(&wrong_request),
        Err(ProtocolFault::State(StateFault::InvalidVersion {
            field: "core_version"
        }))
    ));
    for expected_manifest_sha256 in [Bytes::from_static(&[0; 31]), Bytes::from_static(&[0; 32])] {
        let mut invalid_manifest = handshake_request()?;
        if let Some(parent_frame::Body::HandshakeRequest(request)) = &mut invalid_manifest.body {
            request.expected_manifest_sha256 = expected_manifest_sha256;
        }
        let mut manifest_state = ParentProtocol::new(expectations()?);
        assert!(matches!(
            manifest_state.on_parent_frame(&invalid_manifest),
            Err(ProtocolFault::State(StateFault::InvalidHandshake(
                "expected_manifest_sha256"
            )))
        ));
        assert_eq!(manifest_state.phase(), ParentPhase::Failed);
    }

    assert_handshake_rejected(|success| {
        success.protocol_minor = success.protocol_minor.saturating_add(1);
    })?;
    assert_handshake_rejected(|success| success.worker_version = "0.1.1".to_owned())?;
    assert_handshake_rejected(|success| success.worker_identity = "other-worker".to_owned())?;
    assert_handshake_rejected(|success| success.ortools_version = "9.15.6756".to_owned())?;
    assert_handshake_rejected(|success| success.adapter_version = "0.1.1".to_owned())?;
    assert_handshake_rejected(|success| success.backend_id = "other".to_owned())?;
    assert_handshake_rejected(|success| {
        success.manifest_sha256 = Bytes::from_static(&[0; 32]);
    })?;
    assert_handshake_rejected(|success| success.capabilities.clear())?;
    Ok(())
}

#[test]
fn protocol_minor_must_exactly_match_v1_1() {
    assert!(
        HandshakeExpectations::new(
            1,
            1,
            "eutheto-ortools-worker",
            "0.1.0",
            "ortools-cp-sat",
            "9.15.6755",
            "0.1.0",
            MANIFEST,
            [Capability::CpSat, Capability::SolutionProjection],
            [Capability::CpSat, Capability::SolutionProjection],
        )
        .is_ok()
    );
    for minor in [0, 2] {
        assert!(
            HandshakeExpectations::new(
                1,
                minor,
                "eutheto-ortools-worker",
                "0.1.0",
                "ortools-cp-sat",
                "9.15.6755",
                "0.1.0",
                MANIFEST,
                [Capability::CpSat, Capability::SolutionProjection],
                [Capability::CpSat, Capability::SolutionProjection],
            )
            .is_err()
        );
    }
}

#[test]
fn handshake_rejection_requires_clean_eof_and_zero_exit() -> Result<(), ProtocolFault> {
    let rejection = || WorkerFrame {
        body: Some(worker_frame::Body::HandshakeResponse(HandshakeResponse {
            outcome: Some(Outcome::Error(HandshakeError {
                code: HandshakeErrorCode::UnsupportedProtocolMinor as i32,
                message: "minor dialect is unsupported".to_owned(),
                supported_protocol_major: Some(1),
                supported_protocol_minor: Some(0),
            })),
        })),
    };

    let mut protocol = ParentProtocol::new(expectations()?);
    protocol.on_parent_frame(&handshake_request()?)?;
    assert_eq!(
        protocol.on_worker_frame(rejection())?,
        WorkerObservation::HandshakeRejected
    );
    assert_eq!(protocol.phase(), ParentPhase::Rejected);
    assert!(protocol.handshake_rejection().is_none());
    protocol.on_eof()?;
    assert_eq!(protocol.phase(), ParentPhase::RejectedEof);
    protocol.on_exit(0)?;
    assert_eq!(protocol.phase(), ParentPhase::RejectedComplete);
    let evidence = protocol
        .handshake_rejection()
        .ok_or(StateFault::MissingTerminal)?;
    assert_eq!(
        evidence.error().code,
        HandshakeErrorCode::UnsupportedProtocolMinor as i32
    );
    assert_eq!(evidence.error().message, "minor dialect is unsupported");
    assert_eq!(evidence.error().supported_protocol_major, Some(1));
    assert_eq!(evidence.error().supported_protocol_minor, Some(0));
    let rejection_debug = format!("{evidence:?}");
    assert_eq!(rejection_debug, "HandshakeRejectionEvidence");
    assert!(!rejection_debug.contains("minor dialect is unsupported"));
    assert!(matches!(
        protocol.into_completion()?,
        CompletedSession::HandshakeRejected(_)
    ));

    let mut nonzero = ParentProtocol::new(expectations()?);
    nonzero.on_parent_frame(&handshake_request()?)?;
    nonzero.on_worker_frame(rejection())?;
    nonzero.on_eof()?;
    assert!(nonzero.on_exit(2).is_err());
    assert_eq!(nonzero.phase(), ParentPhase::Failed);
    assert!(nonzero.on_exit(0).is_err());

    let mut trailing = ParentProtocol::new(expectations()?);
    trailing.on_parent_frame(&handshake_request()?)?;
    trailing.on_worker_frame(rejection())?;
    assert!(trailing.on_worker_frame(handshake_success()?).is_err());
    assert_eq!(trailing.phase(), ParentPhase::Failed);

    let mut early_exit = ParentProtocol::new(expectations()?);
    early_exit.on_parent_frame(&handshake_request()?)?;
    early_exit.on_worker_frame(rejection())?;
    assert!(early_exit.on_exit(0).is_err());
    assert_eq!(early_exit.phase(), ParentPhase::Failed);
    let mut duplicate_eof = ParentProtocol::new(expectations()?);
    duplicate_eof.on_parent_frame(&handshake_request()?)?;
    duplicate_eof.on_worker_frame(rejection())?;
    duplicate_eof.on_eof()?;
    assert!(duplicate_eof.on_eof().is_err());
    assert_eq!(duplicate_eof.phase(), ParentPhase::Failed);

    let mut duplicate_exit = ParentProtocol::new(expectations()?);
    duplicate_exit.on_parent_frame(&handshake_request()?)?;
    duplicate_exit.on_worker_frame(rejection())?;
    duplicate_exit.on_eof()?;
    duplicate_exit.on_exit(0)?;
    assert!(duplicate_exit.on_exit(0).is_err());
    assert_eq!(duplicate_exit.phase(), ParentPhase::Failed);

    Ok(())
}
#[test]
fn duplicate_capabilities_and_unspecified_handshake_errors_are_rejected()
-> Result<(), ProtocolFault> {
    assert!(
        HandshakeExpectations::checked_in(
            "eutheto-ortools-worker",
            "0.1.0",
            "ortools-cp-sat",
            "9.15.6755",
            "0.1.0",
            MANIFEST,
            [Capability::CpSat, Capability::CpSat],
            [Capability::CpSat],
        )
        .is_err()
    );
    assert!(
        HandshakeExpectations::checked_in(
            "x".repeat(300),
            "0.1.0",
            "ortools-cp-sat",
            "9.15.6755",
            "0.1.0",
            MANIFEST,
            [Capability::CpSat],
            [Capability::CpSat],
        )
        .is_err()
    );
    let mut duplicate_request = handshake_request()?;
    if let Some(parent_frame::Body::HandshakeRequest(request)) = &mut duplicate_request.body {
        request.required_capabilities.push(Capability::CpSat as i32);
    }
    let mut duplicate_request_state = ParentProtocol::new(expectations()?);
    assert!(
        duplicate_request_state
            .on_parent_frame(&duplicate_request)
            .is_err()
    );

    assert_handshake_rejected(|success| {
        success.capabilities.push(Capability::CpSat as i32);
    })?;
    assert_handshake_rejected(|success| {
        success.capabilities.push(Capability::Progress as i32);
    })?;

    let mut state = ParentProtocol::new(expectations()?);
    state.on_parent_frame(&handshake_request()?)?;
    let response = WorkerFrame {
        body: Some(worker_frame::Body::HandshakeResponse(HandshakeResponse {
            outcome: Some(Outcome::Error(HandshakeError::default())),
        })),
    };
    assert!(matches!(
        state.on_worker_frame(response),
        Err(ProtocolFault::State(StateFault::InvalidWorkerField(
            "handshake_error.code"
        )))
    ));
    let mut unsafe_error_state = ParentProtocol::new(expectations()?);
    unsafe_error_state.on_parent_frame(&handshake_request()?)?;
    let unsafe_error = WorkerFrame {
        body: Some(worker_frame::Body::HandshakeResponse(HandshakeResponse {
            outcome: Some(Outcome::Error(HandshakeError {
                code: HandshakeErrorCode::UnsupportedProtocolMajor as i32,
                message: "unsafe\u{1b}diagnostic".to_owned(),
                ..HandshakeError::default()
            })),
        })),
    };
    assert!(unsafe_error_state.on_worker_frame(unsafe_error).is_err());
    Ok(())
}

#[test]
fn descriptor_driven_validator_accepts_canonical_generated_wire() -> Result<(), ProtocolFault> {
    let bytes = handshake_request()?.encode_to_vec();
    validate_checked_in(PARENT_MESSAGE, &bytes)
}

#[test]
fn additive_unknown_fields_decode_and_continue() -> Result<(), ProtocolFault> {
    let request = handshake_request()?;
    let Some(parent_frame::Body::HandshakeRequest(handshake)) = request.body else {
        return Err(StateFault::MissingParentBody.into());
    };
    let mut nested = handshake.encode_to_vec();
    nested.extend_from_slice(&[0xa0, 0x06, 0x01, 0xaa, 0x06, 0x03, b'a', b'b', b'c']);
    let nested_length = u8::try_from(nested.len())
        .map_err(|_| ProtocolFault::Policy("test handshake exceeds one-byte length".to_owned()))?;
    let mut bytes = vec![0x0a, nested_length];
    bytes.extend_from_slice(&nested);
    validate_checked_in(PARENT_MESSAGE, &bytes)?;
    let decoded = ParentFrame::decode(bytes.as_slice()).map_err(|error| ProtocolFault::Decode {
        message: PARENT_MESSAGE,
        reason: error.to_string(),
    })?;
    let mut protocol = ParentProtocol::new(expectations()?);
    protocol.on_parent_frame(&decoded)?;
    assert_eq!(protocol.phase(), ParentPhase::HandshakeResponse);
    Ok(())
}

#[test]
fn adversarial_tags_oneofs_wire_types_and_lengths_are_rejected() {
    let cases: &[&[u8]] = &[
        &[0x08, 0x01],
        &[0x8a, 0x00, 0x00],
        &[0x80],
        &[0x0a, 0x02, 0x00],
        &[0x0a, 0x02, 0x08, 0x80],
        &[0x0a, 0x80, 0x00],
        &[
            0x0a, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
        ],
        &[0x0a, 0x03, 0x1a, 0x01, 0xff],
        &[0x0a, 0x04, 0x08, 0x01, 0x08, 0x01],
        &[0x0a, 0x00, 0x12, 0x00],
    ];
    for bytes in cases {
        assert!(validate_checked_in(PARENT_MESSAGE, bytes).is_err());
    }
}

#[test]
fn unknown_enums_and_repeated_count_overages_are_rejected() -> Result<(), ProtocolFault> {
    let mut unknown = handshake_request()?;
    if let Some(parent_frame::Body::HandshakeRequest(request)) = &mut unknown.body {
        request.required_capabilities = vec![99];
    }
    let unknown_capability = unknown.encode_to_vec();
    validate_checked_in(PARENT_MESSAGE, &unknown_capability)?;
    let decoded = ParentFrame::decode(unknown_capability.as_slice()).map_err(|error| {
        ProtocolFault::Decode {
            message: PARENT_MESSAGE,
            reason: error.to_string(),
        }
    })?;
    let mut state = ParentProtocol::new(expectations()?);
    assert!(matches!(
        state.on_parent_frame(&decoded),
        Err(ProtocolFault::State(StateFault::InvalidCapability))
    ));
    assert_eq!(state.phase(), ParentPhase::Failed);
    let truncated_to_known_enum = [0x0a, 0x07, 0x2a, 0x05, 0x81, 0x80, 0x80, 0x80, 0x10];
    assert!(validate_checked_in(PARENT_MESSAGE, &truncated_to_known_enum).is_err());

    let source = eutheto_protocol::limits::POLICY_JSON.replacen(
        "\"eutheto.worker.v1.HandshakeRequest.required_capabilities\": {\n      \"max_count\": 64",
        "\"eutheto.worker.v1.HandshakeRequest.required_capabilities\": {\n      \"max_count\": 1",
        1,
    );
    let policy = ProtocolPolicy::parse(&source)?;
    let schema = WireSchema::decode(eutheto_protocol::limits::DESCRIPTOR_BYTES, &policy)?;
    let repeated = [0x0a, 0x04, 0x2a, 0x02, 0x01, 0x02];
    assert!(schema.validate(PARENT_MESSAGE, &repeated, &policy).is_err());
    Ok(())
}

#[test]
fn message_specific_byte_caps_and_depth_are_enforced_before_decode() -> Result<(), ProtocolFault> {
    let frame = handshake_request()?.encode_to_vec();
    let byte_source = eutheto_protocol::limits::POLICY_JSON.replacen(
        "\"eutheto.worker.v1.HandshakeRequest.core_version\": {\n      \"max_bytes\": 64",
        "\"eutheto.worker.v1.HandshakeRequest.core_version\": {\n      \"max_bytes\": 1",
        1,
    );
    let byte_policy = ProtocolPolicy::parse(&byte_source)?;
    let byte_schema = WireSchema::decode(eutheto_protocol::limits::DESCRIPTOR_BYTES, &byte_policy)?;
    assert!(
        byte_schema
            .validate(PARENT_MESSAGE, &frame, &byte_policy)
            .is_err()
    );

    let depth_source = eutheto_protocol::limits::POLICY_JSON.replacen(
        "\"max_nesting_depth\": 8",
        "\"max_nesting_depth\": 1",
        1,
    );
    let depth_policy = ProtocolPolicy::parse(&depth_source)?;
    let depth_schema =
        WireSchema::decode(eutheto_protocol::limits::DESCRIPTOR_BYTES, &depth_policy)?;
    assert!(
        depth_schema
            .validate(PARENT_MESSAGE, &frame, &depth_policy)
            .is_err()
    );
    Ok(())
}

#[test]
fn frame_encoding_uses_exact_big_endian_length() -> Result<(), ProtocolFault> {
    let frame = handshake_request()?;
    let encoded =
        eutheto_protocol::frame::encode_frame(&frame, FrameClass::Handshake, checked_in_policy()?)?;
    let payload_len = u32::from_be_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
    assert_eq!(usize::try_from(payload_len).ok(), Some(encoded.len() - 4));
    Ok(())
}
