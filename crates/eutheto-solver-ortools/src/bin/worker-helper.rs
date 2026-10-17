#![forbid(unsafe_code)]

use std::env;
use std::error::Error;
use std::io::{self, Read, Write};
use std::process;
use std::thread;
use std::time::Duration;

use eutheto_protocol::frame::{decode_parent_frame, encode_frame, read_frame};
use eutheto_protocol::wire::handshake_response::Outcome;
use eutheto_protocol::wire::{
    Finished, HandshakeError, HandshakeErrorCode, HandshakeResponse, HandshakeSuccess, Incumbent,
    Progress, ProgressKind, ProjectedCandidate, ProjectedValue, Started, TerminationReason,
    WorkerError, WorkerErrorCode, WorkerFrame, WorkerSolveStatus, parent_frame, worker_frame,
};
use eutheto_protocol::{
    FrameClass, applied_parameters_sha256, checked_in_policy, normalize_applied_parameters,
};

fn main() {
    if env::args_os().len() != 1 {
        descendant();
    }
    if let Err(error) = worker() {
        let _ = writeln!(io::stderr(), "test helper failure: {error}");
        process::exit(91);
    }
}

fn descendant() -> ! {
    #[cfg(unix)]
    {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            process::abort()
        };
        runtime.block_on(async {
            let Ok(mut terminate) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            else {
                process::abort()
            };
            {
                let mut ready = io::stdout().lock();
                if ready.write_all(b"R").and_then(|()| ready.flush()).is_err() {
                    process::abort()
                }
            }
            loop {
                let _ = terminate.recv().await;
            }
        })
    }
    #[cfg(windows)]
    loop {
        thread::sleep(Duration::from_mins(1));
    }
}

#[allow(clippy::too_many_lines)]
fn worker() -> Result<(), Box<dyn Error>> {
    let policy = checked_in_policy()?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let payload =
        read_frame(&mut input, FrameClass::Handshake, policy)?.ok_or("missing handshake")?;
    let handshake = decode_parent_frame(payload, FrameClass::Handshake)?;
    let parent_frame::Body::HandshakeRequest(handshake) = handshake.body.ok_or("missing body")?
    else {
        return Err("wrong first frame".into());
    };

    match handshake.core_version.as_str() {
        "0.1.1" => {
            write_worker(
                &mut output,
                &WorkerFrame {
                    body: Some(worker_frame::Body::HandshakeResponse(HandshakeResponse {
                        outcome: Some(Outcome::Error(HandshakeError {
                            code: HandshakeErrorCode::UnsupportedProtocolMinor as i32,
                            message: "test rejection".to_owned(),
                            supported_protocol_major: Some(policy.protocol_major()),
                            supported_protocol_minor: Some(policy.protocol_minor()),
                        })),
                    })),
                },
            )?;
            return Ok(());
        }
        "0.1.2" => {
            thread::sleep(Duration::from_mins(1));
            return Ok(());
        }
        _ => {}
    }

    let production_contract = handshake.required_capabilities
        == [
            eutheto_protocol::wire::Capability::CpSat as i32,
            eutheto_protocol::wire::Capability::DeterministicTime as i32,
            eutheto_protocol::wire::Capability::IntermediateSolutions as i32,
            eutheto_protocol::wire::Capability::ObjectiveBounds as i32,
            eutheto_protocol::wire::Capability::Progress as i32,
            eutheto_protocol::wire::Capability::SolutionProjection as i32,
            eutheto_protocol::wire::Capability::SolutionStats as i32,
            eutheto_protocol::wire::Capability::SufficientAssumptions as i32,
        ];
    write_worker(
        &mut output,
        &WorkerFrame {
            body: Some(worker_frame::Body::HandshakeResponse(HandshakeResponse {
                outcome: Some(Outcome::Success(HandshakeSuccess {
                    protocol_major: handshake.protocol_major,
                    protocol_minor: handshake.protocol_minor,
                    worker_identity: if production_contract {
                        "eutheto-ortools-worker".to_owned()
                    } else {
                        "eutheto-test-worker".to_owned()
                    },
                    worker_version: "0.1.0".to_owned(),
                    backend_id: handshake.expected_backend_id,
                    ortools_version: "9.15.6755".to_owned(),
                    adapter_version: "0.1.0".to_owned(),
                    manifest_sha256: handshake.expected_manifest_sha256,
                    capabilities: handshake.required_capabilities,
                })),
            })),
        },
    )?;
    if handshake.core_version == "0.1.3" {
        process::exit(7);
    }

    let payload =
        read_frame(&mut input, FrameClass::SolveRequest, policy)?.ok_or("missing solve request")?;
    let solve = decode_parent_frame(payload, FrameClass::SolveRequest)?;
    let parent_frame::Body::SolveRequest(solve) = solve.body.ok_or("missing solve body")? else {
        return Err("wrong second frame".into());
    };
    if read_frame(&mut input, FrameClass::SolveRequest, policy)?.is_some() {
        return Err("duplicate parent frame".into());
    }

    if handshake.core_version == "0.1.4" {
        io::stderr().write_all(&vec![b'x'; policy.max_stderr_bytes() + 4096])?;
    }
    if matches!(
        handshake.core_version.as_str(),
        "0.1.8" | "0.2.1" | "0.2.2" | "0.2.3"
    ) {
        let executable = env::current_exe()?;
        let mut child = process::Command::new(executable)
            .arg("descendant-ignore-sigterm")
            .arg(&handshake.core_version)
            .stdin(process::Stdio::null())
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::null())
            .spawn()?;
        let mut descendant_stdout = child.stdout.take().ok_or("missing descendant readiness")?;
        let mut readiness = [0u8; 1];
        descendant_stdout.read_exact(&mut readiness)?;
        if readiness != *b"R" {
            return Err("invalid descendant readiness".into());
        }
        writeln!(io::stderr(), "descendant={}", child.id())?;
        if handshake.core_version != "0.2.1" {
            thread::sleep(Duration::from_mins(1));
            return Ok(());
        }
    }

    write_worker(
        &mut output,
        &WorkerFrame {
            body: Some(worker_frame::Body::Started(Started {
                request_id: solve.request_id.clone(),
                model_fingerprint: solve.model_fingerprint.clone(),
            })),
        },
    )?;
    if handshake.core_version == "0.1.5" {
        return Ok(());
    }
    if handshake.core_version == "0.1.7" {
        let cap = policy.frame_cap(FrameClass::WorkerEvent);
        output.write_all(&u32::try_from(cap + 1)?.to_be_bytes())?;
        output.flush()?;
        return Ok(());
    }
    if handshake.core_version == "0.1.9" {
        for _ in 0..=policy.events_per_second() {
            write_worker(
                &mut output,
                &WorkerFrame {
                    body: Some(worker_frame::Body::Progress(Progress {
                        request_id: solve.request_id.clone(),
                        kind: ProgressKind::Search as i32,
                        ..Progress::default()
                    })),
                },
            )?;
        }
        return Ok(());
    }

    if production_contract
        && solve.parameters.as_ref().is_some_and(|parameters| {
            parameters.random_seed == Some(2)
                && parameters.emit_intermediate_solutions == Some(true)
        })
    {
        write_worker(
            &mut output,
            &WorkerFrame {
                body: Some(worker_frame::Body::Incumbent(Incumbent {
                    request_id: solve.request_id.clone(),
                    candidate: Some(ProjectedCandidate {
                        values: solve
                            .projections
                            .iter()
                            .map(|projection| ProjectedValue {
                                projection_id: projection.projection_id,
                                value: 0,
                            })
                            .collect(),
                    }),
                    ..Incumbent::default()
                })),
            },
        )?;
        write_worker(
            &mut output,
            &WorkerFrame {
                body: Some(worker_frame::Body::Error(WorkerError {
                    request_id: solve.request_id,
                    code: WorkerErrorCode::Internal as i32,
                    message: "test internal failure".to_owned(),
                    retryable: false,
                })),
            },
        )?;
        return Ok(());
    }

    if production_contract {
        let applied_parameters_sha256 =
            applied_parameters_sha256(&normalize_applied_parameters(&solve)?);
        let terminal = WorkerFrame {
            body: Some(worker_frame::Body::Finished(Finished {
                request_id: solve.request_id,
                raw_cp_sat_status: 4,
                status: WorkerSolveStatus::Optimal as i32,
                termination_reason: TerminationReason::Optimal as i32,
                final_candidate: Some(ProjectedCandidate {
                    values: solve
                        .projections
                        .iter()
                        .map(|projection| ProjectedValue {
                            projection_id: projection.projection_id,
                            value: 0,
                        })
                        .collect(),
                }),
                model_fingerprint: solve.model_fingerprint,
                applied_parameters_sha256: applied_parameters_sha256.to_vec().into(),
                ..Finished::default()
            })),
        };
        write_worker(&mut output, &terminal)?;
        return Ok(());
    }

    let terminal = WorkerFrame {
        body: Some(worker_frame::Body::Error(WorkerError {
            request_id: solve.request_id,
            code: WorkerErrorCode::UnsupportedModel as i32,
            message: sanitation_observation(),
            retryable: false,
        })),
    };
    write_worker(&mut output, &terminal)?;
    if handshake.core_version == "0.2.4" {
        thread::sleep(Duration::from_millis(200));
    }
    if handshake.core_version == "0.1.6" {
        write_worker(&mut output, &terminal)?;
    }
    if handshake.core_version == "0.2.0" {
        process::exit(7);
    }
    Ok(())
}

fn sanitation_observation() -> String {
    let environment = env::vars_os().collect::<Vec<_>>();
    let allowed = ["LANG", "LC_ALL", "TZ", "TMPDIR", "TEMP", "TMP"];
    let clean = environment
        .iter()
        .all(|(name, _)| allowed.iter().any(|allowed| name == allowed));
    let cwd = env::current_dir().ok();
    let cwd_private = cwd
        .as_ref()
        .and_then(|cwd| {
            cwd.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .is_some_and(|name| name.starts_with("eutheto-worker-"));
    let deterministic = env::var_os("LANG").as_deref() == Some(std::ffi::OsStr::new("C"))
        && env::var_os("LC_ALL").as_deref() == Some(std::ffi::OsStr::new("C"))
        && env::var_os("TZ").as_deref() == Some(std::ffi::OsStr::new("UTC"));
    #[cfg(unix)]
    let private_temp = {
        use std::os::unix::fs::PermissionsExt;
        cwd.as_ref().is_some_and(|cwd| {
            let cwd_mode = cwd
                .metadata()
                .ok()
                .map(|metadata| metadata.permissions().mode() & 0o777);
            let executable_mode = env::current_exe()
                .ok()
                .and_then(|path| path.metadata().ok())
                .map(|metadata| metadata.permissions().mode() & 0o777);
            env::var_os("TMPDIR").as_deref() == Some(cwd.as_os_str())
                && env::var_os("TEMP").is_none()
                && env::var_os("TMP").is_none()
                && cwd_mode == Some(0o700)
                && executable_mode == Some(0o700)
        })
    };
    #[cfg(windows)]
    let private_temp = cwd.as_ref().is_some_and(|cwd| {
        env::var_os("TEMP").as_deref() == Some(cwd.as_os_str())
            && env::var_os("TMP").as_deref() == Some(cwd.as_os_str())
            && env::var_os("TMPDIR").is_none()
    });
    if clean && deterministic && cwd_private && private_temp {
        "sanitized test process".to_owned()
    } else {
        "unsanitized test process".to_owned()
    }
}

fn write_worker(output: &mut impl Write, frame: &WorkerFrame) -> Result<(), Box<dyn Error>> {
    let bytes = encode_frame(frame, frame_class(frame), checked_in_policy()?)?;
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}

fn frame_class(frame: &WorkerFrame) -> FrameClass {
    match &frame.body {
        Some(worker_frame::Body::HandshakeResponse(_)) => FrameClass::Handshake,
        _ => FrameClass::WorkerEvent,
    }
}
