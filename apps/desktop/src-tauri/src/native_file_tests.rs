use super::*;
use std::error::Error;
use std::io::Write;

type TestResult = Result<(), Box<dyn Error>>;

fn boxed(error: NativeFileError) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

#[test]
fn regular_file_at_exact_limit_is_captured_across_chunks() -> TestResult {
    let directory = tempfile::tempdir()?;
    // The primitive has no filename-extension policy.
    let path = directory.path().join("selected.data");
    let expected: Vec<u8> = (0_u8..=255).cycle().take(READ_CHUNK_BYTES + 17).collect();
    std::fs::write(&path, &expected)?;
    assert_eq!(
        read_bounded_file(&path, expected.len(), &CancellationToken::new()),
        Ok(expected)
    );
    Ok(())
}

#[test]
fn regular_file_one_byte_over_limit_is_rejected() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("selected.csv");
    std::fs::write(&path, b"12345")?;
    assert_eq!(
        read_bounded_file(&path, 4, &CancellationToken::new()),
        Err(NativeFileError::TooLarge)
    );
    Ok(())
}

#[test]
fn empty_file_and_zero_byte_limit_are_supported() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("selected.csv");
    std::fs::write(&path, b"")?;
    assert_eq!(
        read_bounded_file(&path, 0, &CancellationToken::new()),
        Ok(Vec::new())
    );
    assert_eq!(
        read_bounded_file(&path, 16 * 1024 * 1024, &CancellationToken::new()),
        Ok(Vec::new())
    );
    std::fs::write(&path, b"x")?;
    assert_eq!(
        read_bounded_file(&path, 0, &CancellationToken::new()),
        Err(NativeFileError::TooLarge)
    );
    Ok(())
}

#[test]
fn pre_cancelled_acquisition_does_not_resolve_the_path() -> TestResult {
    let directory = tempfile::tempdir()?;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        read_bounded_file(&directory.path().join("missing.csv"), 32, &cancellation),
        Err(NativeFileError::Cancelled)
    );
    Ok(())
}

#[test]
fn cancellation_after_acquisition_prevents_capture() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("selected.csv");
    std::fs::write(&path, b"original")?;
    let cancellation = CancellationToken::new();
    let (file, initial_length) = open_regular_file(&path, 32, &cancellation).map_err(boxed)?;
    cancellation.cancel();
    assert_eq!(
        read_opened_file(file, initial_length, 32, &cancellation),
        Err(NativeFileError::Cancelled)
    );
    Ok(())
}

#[test]
fn directory_is_not_read_as_a_source() -> TestResult {
    let directory = tempfile::tempdir()?;
    // Windows may reject a directory at open, before same-handle metadata inspection.
    assert!(matches!(
        read_bounded_file(directory.path(), 32, &CancellationToken::new()),
        Err(NativeFileError::InvalidFileType | NativeFileError::Unreadable)
    ));
    Ok(())
}

#[test]
fn growth_after_metadata_is_read_but_cannot_bypass_the_byte_limit() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("selected.csv");
    std::fs::write(&path, b"ab")?;
    let cancellation = CancellationToken::new();
    let (file, initial_length) = open_regular_file(&path, 4, &cancellation).map_err(boxed)?;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(b"cd")?;
    assert_eq!(
        read_opened_file(file, initial_length, 4, &cancellation),
        Ok(b"abcd".to_vec())
    );

    let (file, initial_length) = open_regular_file(&path, 4, &cancellation).map_err(boxed)?;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(b"e")?;
    assert_eq!(
        read_opened_file(file, initial_length, 4, &cancellation),
        Err(NativeFileError::TooLarge)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn final_symlink_and_unix_socket_are_rejected() -> TestResult {
    use std::os::unix::{fs::symlink, net::UnixListener};

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("selected.csv");
    std::fs::write(&path, b"original")?;
    let link = directory.path().join("link.csv");
    symlink(&path, &link)?;
    assert_eq!(
        read_bounded_file(&link, 32, &CancellationToken::new()),
        Err(NativeFileError::InvalidFileType)
    );
    let socket = directory.path().join("socket.csv");
    let _listener = UnixListener::bind(&socket)?;
    assert_eq!(
        read_bounded_file(&socket, 32, &CancellationToken::new()),
        Err(NativeFileError::InvalidFileType)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn replacing_selected_path_after_secure_open_does_not_redirect_capture() -> TestResult {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir()?;
    let selected = directory.path().join("selected.csv");
    let moved = directory.path().join("moved.csv");
    let replacement = directory.path().join("replacement.csv");
    std::fs::write(&selected, b"original")?;
    std::fs::write(&replacement, b"replacement")?;
    let cancellation = CancellationToken::new();
    let (file, initial_length) = open_regular_file(&selected, 32, &cancellation).map_err(boxed)?;
    // Replace the selected directory entry deterministically after the actual secure
    // acquisition; the production read stage receives only that acquired File.
    std::fs::rename(&selected, &moved)?;
    symlink(&replacement, &selected)?;
    let captured = read_opened_file(file, initial_length, 32, &cancellation).map_err(boxed)?;
    assert_eq!(captured, b"original");
    std::fs::write(&moved, b"changed after capture")?;
    assert_eq!(captured, b"original");
    assert_eq!(
        read_bounded_file(&selected, 32, &cancellation),
        Err(NativeFileError::InvalidFileType)
    );
    Ok(())
}

#[cfg(windows)]
#[test]
#[ignore = "Requires Windows Developer Mode or symlink-creation privilege; run explicitly on Windows"]
fn windows_final_file_reparse_point_is_rejected() -> TestResult {
    use std::os::windows::fs::symlink_file;

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("selected.csv");
    std::fs::write(&path, b"original")?;
    let link = directory.path().join("link.csv");
    symlink_file(&path, &link)?;
    assert!(matches!(
        read_bounded_file(&link, 32, &CancellationToken::new()),
        Err(NativeFileError::InvalidFileType | NativeFileError::Unreadable)
    ));
    Ok(())
}
