//! Explicitly granted, bounded inputs; no application-directory or database fallback.

use super::{CliExitCode, SafeCliError, native_file};
use eutheto_types::{OperationControl, OperationInterruption};
use std::io;
use std::path::Path;
use tokio::io::AsyncReadExt;

pub(super) fn check_control(control: &OperationControl) -> Result<(), SafeCliError> {
    control.check().map_err(|interruption| match interruption {
        OperationInterruption::Cancelled => SafeCliError::cancelled(),
        OperationInterruption::DeadlineExceeded => SafeCliError::new(
            CliExitCode::NoVerifiedSolution,
            "operation.deadline_exceeded",
            "The operation reached its deadline before completion.",
        ),
    })
}

pub(super) async fn read_controlled(
    path: &Path,
    limit: u64,
    code: &'static str,
    control: &OperationControl,
) -> Result<Vec<u8>, SafeCliError> {
    check_control(control)?;
    let file = native_file::open_regular(path).map_err(|_| {
        SafeCliError::storage(
            "storage.read_failed",
            "The input could not be opened as a regular file.",
        )
    })?;
    check_control(control)?;
    let length = file
        .metadata()
        .map_err(|_| {
            SafeCliError::storage(
                "storage.read_failed",
                "The opened input could not be inspected.",
            )
        })?
        .len();
    if length > limit {
        return Err(too_large(code));
    }
    let limit = usize::try_from(limit).map_err(|_| too_large(code))?;
    let capacity = usize::try_from(length).map_err(|_| too_large(code))?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut file = tokio::fs::File::from_std(file);
    let mut buffer = [0_u8; 8192];
    loop {
        check_control(control)?;
        let available = (limit - bytes.len()).min(buffer.len() - 1) + 1;
        let read = match file.read(&mut buffer[..available]).await {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                return Err(SafeCliError::storage(
                    "storage.read_failed",
                    "The opened input could not be read.",
                ));
            }
        };
        check_control(control)?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|length| length > limit)
        {
            return Err(too_large(code));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn too_large(code: &'static str) -> SafeCliError {
    SafeCliError::storage(code, "The input exceeds the supported byte limit.")
}

pub(super) fn scenario_id_or_file(
    input: &Path,
) -> Result<Option<eutheto_types::ScenarioId>, SafeCliError> {
    if explicit_file(input) {
        return Ok(None);
    }
    super::scenario_id(input.to_str().unwrap_or("")).map(Some)
}

pub(super) fn solution_id_or_file(
    input: &Path,
) -> Result<Option<eutheto_types::SolutionId>, SafeCliError> {
    if explicit_file(input) {
        return Ok(None);
    }
    super::solution_id(input.to_str().unwrap_or("")).map(Some)
}

fn explicit_file(input: &Path) -> bool {
    input.is_absolute()
        || input == Path::new(".")
        || input == Path::new("..")
        || input.extension().is_some()
        || input
            .to_str()
            .is_some_and(|text| text.contains(['/', '\\']))
}

pub(super) fn publish_text(
    destination: &Path,
    bytes: &[u8],
    control: &OperationControl,
) -> Result<(), SafeCliError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        SafeCliError::storage(
            "file.text_invalid",
            "The encoded output is not valid UTF-8 text.",
        )
    })?;
    let prepared = eutheto_export::prepare_text_atomic_controlled(destination, text, control)
        .map_err(|error| publication_error(&error, control))?;
    prepared
        .publish_controlled(control)
        .map_err(|error| publication_error(&error, control))
}

fn publication_error(
    error: &eutheto_export::ExportError,
    control: &OperationControl,
) -> SafeCliError {
    if let Err(interruption) = check_control(control) {
        return interruption;
    }
    if matches!(error, eutheto_export::ExportError::DestinationExists(_)) {
        SafeCliError::storage(
            "file.destination_exists",
            "The destination already exists and was not overwritten.",
        )
    } else {
        SafeCliError::storage(
            "file.publication_failed",
            "The requested output could not be published.",
        )
    }
}
