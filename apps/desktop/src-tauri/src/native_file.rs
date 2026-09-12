use crate::NativeFileError;
use eutheto_types::CancellationToken;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;

const READ_CHUNK_BYTES: usize = 64 * 1024;

/// Captures bounded bytes from one securely checked regular-file handle.
///
/// Final symlinks/reparse points are rejected; ancestors are not constrained. Concurrent
/// in-place writes can affect the capture: this is not an atomic filesystem snapshot.
/// Cancellation is cooperative between reads, not an interruption of an OS file operation.
pub(crate) fn read_bounded_file(
    path: &Path,
    maximum_bytes: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, NativeFileError> {
    let (file, initial_length) = open_regular_file(path, maximum_bytes, cancellation)?;
    read_opened_file(file, initial_length, maximum_bytes, cancellation)
}

fn open_regular_file(
    path: &Path,
    maximum_bytes: usize,
    cancellation: &CancellationToken,
) -> Result<(File, usize), NativeFileError> {
    if cancellation.is_cancelled() {
        return Err(NativeFileError::Cancelled);
    }
    #[cfg(not(windows))]
    let selected_metadata =
        std::fs::symlink_metadata(path).map_err(|_| NativeFileError::Unreadable)?;
    #[cfg(not(windows))]
    if selected_metadata.file_type().is_symlink() || !selected_metadata.is_file() {
        return Err(NativeFileError::InvalidFileType);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT opens the final entry instead of its target.
        options.custom_flags(0x0020_0000);
    }
    if cancellation.is_cancelled() {
        return Err(NativeFileError::Cancelled);
    }
    let file = options
        .open(path)
        .map_err(|_| NativeFileError::Unreadable)?;
    let opened_metadata = file.metadata().map_err(|_| NativeFileError::Unreadable)?;
    if !opened_metadata.is_file() {
        return Err(NativeFileError::InvalidFileType);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if selected_metadata.dev() != opened_metadata.dev()
            || selected_metadata.ino() != opened_metadata.ino()
        {
            return Err(NativeFileError::Unreadable);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // The chooser supplies only a path. Inspect and read this same handle;
        // never resolve the selected name again after checking its attributes.
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if opened_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(NativeFileError::Unreadable);
        }
    }
    let initial_length =
        usize::try_from(opened_metadata.len()).map_err(|_| NativeFileError::TooLarge)?;
    if initial_length > maximum_bytes {
        return Err(NativeFileError::TooLarge);
    }
    Ok((file, initial_length))
}

fn read_opened_file(
    mut file: File,
    initial_length: usize,
    maximum_bytes: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, NativeFileError> {
    if cancellation.is_cancelled() {
        return Err(NativeFileError::Cancelled);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(initial_length)
        .map_err(|_| NativeFileError::Unreadable)?;
    loop {
        if cancellation.is_cancelled() {
            return Err(NativeFileError::Cancelled);
        }
        if bytes.len() == maximum_bytes || bytes.len() == bytes.capacity() {
            // Probe EOF before growing, and never append an overflow byte at the limit.
            let mut next = [0_u8; 1];
            let result = file.read(&mut next);
            if cancellation.is_cancelled() {
                return Err(NativeFileError::Cancelled);
            }
            match result {
                Ok(0) => return Ok(bytes),
                Ok(_) if bytes.len() == maximum_bytes => return Err(NativeFileError::TooLarge),
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(NativeFileError::Unreadable),
            }
            // Stable files reserve their actual metadata length once. A growing file uses
            // capped geometric growth, not Vec's potentially over-limit doubling policy.
            // Allocator overhead is not part of the logical source-byte custody limit.
            let next_chunk = bytes
                .len()
                .checked_add(READ_CHUNK_BYTES)
                .unwrap_or(maximum_bytes);
            let capacity = bytes
                .capacity()
                .checked_mul(2)
                .unwrap_or(maximum_bytes)
                .max(next_chunk)
                .min(maximum_bytes);
            bytes
                .try_reserve_exact(capacity - bytes.len())
                .map_err(|_| NativeFileError::Unreadable)?;
            bytes.push(next[0]);
            continue;
        }
        let start = bytes.len();
        let chunk_length = (maximum_bytes - start)
            .min(bytes.capacity() - start)
            .min(READ_CHUNK_BYTES);
        let end = start
            .checked_add(chunk_length)
            .ok_or(NativeFileError::TooLarge)?;
        bytes.resize(end, 0);
        let result = file.read(&mut bytes[start..end]);
        if cancellation.is_cancelled() {
            return Err(NativeFileError::Cancelled);
        }
        match result {
            Ok(count) => {
                bytes.truncate(start + count);
                if count == 0 {
                    return Ok(bytes);
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => bytes.truncate(start),
            Err(_) => return Err(NativeFileError::Unreadable),
        }
    }
}

#[cfg(test)]
#[path = "native_file_tests.rs"]
mod tests;
