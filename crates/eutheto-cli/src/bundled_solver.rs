//! The installed image and build-embedded digest are the only worker authority.

use super::{SafeCliError, files::check_control};
use eutheto_solver_api::SolverRegistry;
use eutheto_solver_ortools::{VerifiedWorkerArtifact, registry_with_ortools};
use eutheto_types::OperationControl;

const MANIFEST_SHA256: &str = env!("EUTHETO_ORTOOLS_MANIFEST_SHA256");

pub(super) async fn load(control: &OperationControl) -> Result<SolverRegistry, SafeCliError> {
    check_control(control)?;
    if MANIFEST_SHA256.is_empty() {
        return Err(SafeCliError::unavailable(
            "backend.worker_not_bundled",
            "This CLI was built without an approved worker. Use an approved packaged CLI build.",
        ));
    }
    let digest = decode_digest()?;
    let executable = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|_| unavailable())?;
    let parent = executable.parent().ok_or_else(unavailable)?;
    let worker = parent.join(if cfg!(windows) {
        "ortools-worker.exe"
    } else {
        "ortools-worker"
    });
    check_control(control)?;
    let artifact =
        VerifiedWorkerArtifact::verify_packaged(parent.join("solver/ortools"), worker, digest)
            .await;
    check_control(control)?;
    registry_with_ortools(artifact.map_err(|_| unavailable())?).map_err(|_| unavailable())
}

fn decode_digest() -> Result<[u8; 32], SafeCliError> {
    if MANIFEST_SHA256.len() != 64 {
        return Err(unavailable());
    }
    let mut digest = [0; 32];
    for (byte, pair) in digest
        .iter_mut()
        .zip(MANIFEST_SHA256.as_bytes().chunks_exact(2))
    {
        *byte = nibble(pair[0])? * 16 + nibble(pair[1])?;
    }
    Ok(digest)
}

fn nibble(byte: u8) -> Result<u8, SafeCliError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(unavailable()),
    }
}

fn unavailable() -> SafeCliError {
    SafeCliError::unavailable(
        "backend.worker_invalid",
        "The installed worker or its resources do not match this CLI build. Reinstall the approved package.",
    )
}
