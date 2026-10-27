// SPDX-License-Identifier: Apache-2.0
//! Owned, bounded execution of the app-bundled offline PDF renderer.
use super::{join_error, operation_interrupted, validation_error};
use eutheto_export::EncodedRecipientHtml;
use eutheto_types::{
    AppError, MAX_RECIPIENT_PDF_BYTES, MAX_RECIPIENT_PDF_HEADER_BYTES, MAX_RECIPIENT_REPORT_BYTES,
    OperationControl, RECIPIENT_PDF_PROTOCOL_VERSION, RECIPIENT_PDF_REQUEST_MAGIC,
    RECIPIENT_PDF_RESPONSE_MAGIC, RecipientPdfRendererV1, RecipientPdfRequestV1,
    RecipientPdfResponseV1,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, process::Stdio};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PdfManifest {
    schema_version: u32,
    worker_sha256: String,
    renderer: RecipientPdfRendererV1,
}

/// Rendered bytes retain the exact disclosure model used by the HTML preview.
pub struct EncodedRecipientPdf {
    pub(super) html: EncodedRecipientHtml,
    pub(super) bytes: Vec<u8>,
    pub(super) pdf_sha256: String,
    pub(super) renderer: RecipientPdfRendererV1,
}

impl EncodedRecipientPdf {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    #[must_use]
    pub fn model_sha256(&self) -> &str {
        self.html.model_sha256()
    }
    #[must_use]
    pub fn pdf_sha256(&self) -> &str {
        &self.pdf_sha256
    }
    #[must_use]
    pub fn renderer(&self) -> &RecipientPdfRendererV1 {
        &self.renderer
    }
    #[must_use]
    pub fn html(&self) -> &EncodedRecipientHtml {
        &self.html
    }
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>() + self.html.retained_bytes() - size_of::<EncodedRecipientHtml>()
            + self.bytes.capacity()
            + self.pdf_sha256.capacity()
            + self.renderer.typst_version.capacity()
            + self.renderer.template_sha256.capacity()
            + self.renderer.fonts_sha256.capacity()
    }
}

fn renderer_error() -> AppError {
    validation_error(
        "recipient.pdf_renderer_failed",
        "/format",
        "The bundled PDF renderer could not produce a verified report.",
    )
}

/// Render an already encoded disclosure model with the fixed app-owned renderer.
/// The owned task settles its child even when a caller drops its observation future.
///
/// # Errors
/// Refuses unadmitted artifacts, malformed output, failed children, and interruption.
pub async fn render_recipient_pdf(
    html: EncodedRecipientHtml,
    control: OperationControl,
) -> Result<EncodedRecipientPdf, AppError> {
    tokio::spawn(async move {
        if !matches!(&control, OperationControl::Solve(view) if view.snapshot().remaining_milliseconds.value() <= 30_000) {
            return Err(renderer_error());
        }
        control.check().map_err(operation_interrupted)?;
        let (directory, path, manifest) = admit_worker(&control).await?;
        let (bytes, pdf_sha256) = supervise(&path, &manifest.renderer, html.canonical_model_bytes(), &control).await?;
        drop(directory);
        Ok(EncodedRecipientPdf { html, bytes, pdf_sha256, renderer: manifest.renderer })
    }).await.map_err(join_error)?
}

async fn admit_worker(
    control: &OperationControl,
) -> Result<(tempfile::TempDir, PathBuf, PdfManifest), AppError> {
    let executable = std::env::current_exe().map_err(|_| renderer_error())?;
    let directory = executable.parent().ok_or_else(renderer_error)?;
    let path = directory.join(if cfg!(windows) {
        "eutheto-pdf-worker.exe"
    } else {
        "eutheto-pdf-worker"
    });
    let manifest_bytes = include_bytes!(concat!(env!("OUT_DIR"), "/recipient-pdf-manifest.json"));
    if manifest_bytes.is_empty() {
        return Err(renderer_error());
    }
    let manifest: PdfManifest =
        serde_json::from_slice(manifest_bytes).map_err(|_| renderer_error())?;
    if manifest.schema_version != 1 || manifest.worker_sha256.len() != 64 {
        return Err(renderer_error());
    }
    let mut builder = tempfile::Builder::new();
    builder.prefix("eutheto-pdf-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let directory = builder.tempdir().map_err(|_| renderer_error())?;
    let staged = directory
        .path()
        .join(path.file_name().ok_or_else(renderer_error)?);
    super::manual::controlled_wait(
        stage_worker(&path, &staged, &manifest.worker_sha256),
        control,
    )
    .await??;
    Ok((directory, staged, manifest))
}

// Copy and hash the same bytes into private custody; spawning the original mutable
// installation path after hashing would admit different bytes during an update.
async fn stage_worker(
    source: &std::path::Path,
    destination: &std::path::Path,
    expected: &str,
) -> Result<(), AppError> {
    let before = tokio::fs::symlink_metadata(source)
        .await
        .map_err(|_| renderer_error())?;
    if !regular_direct_file(&before) {
        return Err(renderer_error());
    }
    let (mut file, metadata) = open_worker_source(source).await?;
    let mut output = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .await
        .map_err(|_| renderer_error())?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut copied = 0_u64;
    loop {
        let count = file.read(&mut buffer).await.map_err(|_| renderer_error())?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > metadata.len() {
            return Err(renderer_error());
        }
        hash.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .await
            .map_err(|_| renderer_error())?;
    }
    if copied != metadata.len()
        || hash
            .finalize()
            .iter()
            .flat_map(|byte| [byte >> 4, byte & 0x0f])
            .map(|nibble| char::from(b"0123456789abcdef"[usize::from(nibble)]))
            .collect::<String>()
            != expected
    {
        return Err(renderer_error());
    }
    output.flush().await.map_err(|_| renderer_error())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        output
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|_| renderer_error())?;
    }
    drop(output.into_std().await);
    Ok(())
}

async fn open_worker_source(
    source: &std::path::Path,
) -> Result<(tokio::fs::File, std::fs::Metadata), AppError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    let file = options.open(source).await.map_err(|_| renderer_error())?;
    let metadata = file.metadata().await.map_err(|_| renderer_error())?;
    if !regular_direct_file(&metadata) || metadata.len() > 512 * 1024 * 1024 {
        return Err(renderer_error());
    }
    Ok((file, metadata))
}

fn regular_direct_file(metadata: &std::fs::Metadata) -> bool {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        } // FILE_ATTRIBUTE_REPARSE_POINT
    }
    true
}

async fn read_bounded<R: AsyncRead + Unpin>(
    reader: R,
    maximum: usize,
) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::new();
    reader
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| renderer_error())?;
    if bytes.len() > maximum {
        return Err(renderer_error());
    }
    Ok(bytes)
}

async fn response<R: AsyncRead + Unpin>(
    mut stdout: R,
    expected: &RecipientPdfRendererV1,
    model_sha256: &str,
) -> Result<(Vec<u8>, String), AppError> {
    let mut magic = [0; 8];
    stdout
        .read_exact(&mut magic)
        .await
        .map_err(|_| renderer_error())?;
    if &magic != RECIPIENT_PDF_RESPONSE_MAGIC {
        return Err(renderer_error());
    }
    let length = stdout.read_u32().await.map_err(|_| renderer_error())? as usize;
    if length == 0 || length > MAX_RECIPIENT_PDF_HEADER_BYTES {
        return Err(renderer_error());
    }
    let mut header = vec![0; length];
    stdout
        .read_exact(&mut header)
        .await
        .map_err(|_| renderer_error())?;
    let header: RecipientPdfResponseV1 =
        serde_json::from_slice(&header).map_err(|_| renderer_error())?;
    if header.schema_version != RECIPIENT_PDF_PROTOCOL_VERSION
        || header.model_sha256 != model_sha256
        || &header.renderer != expected
        || header.pdf_bytes == 0
        || header.pdf_bytes as usize > MAX_RECIPIENT_PDF_BYTES
    {
        return Err(renderer_error());
    }
    let mut bytes = vec![0; header.pdf_bytes as usize];
    stdout
        .read_exact(&mut bytes)
        .await
        .map_err(|_| renderer_error())?;
    let mut trailing = [0];
    if stdout
        .read(&mut trailing)
        .await
        .map_err(|_| renderer_error())?
        != 0
    {
        return Err(renderer_error());
    }
    let digest = eutheto_export::sha256_hex(&bytes);
    if digest != header.pdf_sha256 || !bytes.starts_with(b"%PDF-") {
        return Err(renderer_error());
    }
    Ok((bytes, digest))
}

async fn supervise(
    path: &std::path::Path,
    renderer: &RecipientPdfRendererV1,
    model: &[u8],
    control: &OperationControl,
) -> Result<(Vec<u8>, String), AppError> {
    control.check().map_err(operation_interrupted)?;
    if model.len() > MAX_RECIPIENT_REPORT_BYTES {
        return Err(renderer_error());
    }
    let model_sha256 = eutheto_export::sha256_hex(model);
    let header = serde_json::to_vec(&RecipientPdfRequestV1 {
        schema_version: RECIPIENT_PDF_PROTOCOL_VERSION,
        model_bytes: u32::try_from(model.len()).map_err(|_| renderer_error())?,
        model_sha256: model_sha256.clone(),
    })
    .map_err(|_| renderer_error())?;
    let mut command = tokio::process::Command::new(path);
    command
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .current_dir(path.parent().ok_or_else(renderer_error)?)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = loop {
        control.check().map_err(operation_interrupted)?;
        match command.spawn() {
            Ok(child) => break child,
            // A concurrent fork can retain the closed staging writer until exec.
            // Retry only kernel admission: no worker image has started yet.
            #[cfg(target_os = "linux")]
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(_) => return Err(renderer_error()),
        }
    };
    let mut stdin = child.stdin.take().ok_or_else(renderer_error)?;
    let stdout = child.stdout.take().ok_or_else(renderer_error)?;
    let stderr = child.stderr.take().ok_or_else(renderer_error)?;
    let work = async {
        let send = async {
            stdin
                .write_all(RECIPIENT_PDF_REQUEST_MAGIC)
                .await
                .map_err(|_| renderer_error())?;
            stdin
                .write_u32(u32::try_from(header.len()).map_err(|_| renderer_error())?)
                .await
                .map_err(|_| renderer_error())?;
            stdin
                .write_all(&header)
                .await
                .map_err(|_| renderer_error())?;
            stdin.write_all(model).await.map_err(|_| renderer_error())?;
            stdin.flush().await.map_err(|_| renderer_error())
        };
        let ((), result, _) = tokio::try_join!(
            send,
            response(stdout, renderer, &model_sha256),
            read_bounded(stderr, 4096)
        )?;
        if !child.wait().await.map_err(|_| renderer_error())?.success() {
            return Err(renderer_error());
        }
        Ok(result)
    };
    let result = super::manual::controlled_wait(work, control)
        .await
        .and_then(std::convert::identity);
    if result.is_err() {
        // No renderer descendants are permitted. Kill and reap before releasing the lifeline.
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    drop(stdin);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> RecipientPdfRendererV1 {
        RecipientPdfRendererV1 {
            version: 1,
            typst_version: "test".to_owned(),
            template_sha256: "a".repeat(64),
            fonts_sha256: "b".repeat(64),
        }
    }

    fn frame(
        header: &RecipientPdfResponseV1,
        bytes: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let json = serde_json::to_vec(header)?;
        let mut wire = RECIPIENT_PDF_RESPONSE_MAGIC.to_vec();
        wire.extend(u32::try_from(json.len())?.to_be_bytes());
        wire.extend(json);
        wire.extend(bytes);
        Ok(wire)
    }

    #[tokio::test]
    async fn accepts_only_exact_bounded_identity_and_pdf_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = b"%PDF-1.7\nfixture";
        let model = "c".repeat(64);
        let mut header = RecipientPdfResponseV1 {
            schema_version: 1,
            model_sha256: model.clone(),
            pdf_bytes: u32::try_from(bytes.len())?,
            pdf_sha256: eutheto_export::sha256_hex(bytes),
            renderer: identity(),
        };
        let wire = frame(&header, bytes)?;
        assert_eq!(
            response(wire.as_slice(), &identity(), &model)
                .await
                .map_err(|error| format!("{error:?}"))?
                .0,
            bytes
        );
        let mut extra = wire.clone();
        extra.push(0);
        assert!(
            response(extra.as_slice(), &identity(), &model)
                .await
                .is_err()
        );
        assert!(
            response(&wire[..wire.len() - 1], &identity(), &model)
                .await
                .is_err()
        );
        assert!(
            response(wire.as_slice(), &identity(), "different model")
                .await
                .is_err()
        );
        header.renderer.version = 2;
        assert!(
            response(frame(&header, bytes)?.as_slice(), &identity(), &model)
                .await
                .is_err()
        );
        header.renderer = identity();
        header.pdf_sha256 = "d".repeat(64);
        assert!(
            response(frame(&header, bytes)?.as_slice(), &identity(), &model)
                .await
                .is_err()
        );
        header.pdf_bytes = u32::try_from(MAX_RECIPIENT_PDF_BYTES + 1)?;
        assert!(
            response(frame(&header, &[])?.as_slice(), &identity(), &model)
                .await
                .is_err()
        );
        let mut oversized = RECIPIENT_PDF_RESPONSE_MAGIC.to_vec();
        oversized.extend(u32::try_from(MAX_RECIPIENT_PDF_HEADER_BYTES + 1)?.to_be_bytes());
        assert!(
            response(oversized.as_slice(), &identity(), &model)
                .await
                .is_err()
        );
        Ok(())
    }

    #[cfg(unix)]
    fn shell_program(name: &str) -> Result<String, Box<dyn std::error::Error>> {
        let paths = std::env::var_os("PATH").ok_or("missing test PATH")?;
        let path = std::env::split_paths(&paths)
            .map(|directory| directory.join(name))
            .find(|path| path.is_file())
            .ok_or("missing fixture utility")?;
        Ok(format!(
            "'{}'",
            path.to_str()
                .ok_or("non-UTF8 fixture utility")?
                .replace('\'', "'\\''")
        ))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn opened_handle_refuses_links_and_special_files_without_blocking()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("regular");
        std::fs::write(&file, b"fixture")?;
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&file, &link)?;
        assert!(open_worker_source(&link).await.is_err());
        let fifo = directory.path().join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()?
                .success()
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), open_worker_source(&fifo))
                .await?
                .is_err()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn valid_frame_still_requires_successful_worker_exit()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::fmt::Write as _;
        use std::os::unix::fs::PermissionsExt;
        let bytes = b"%PDF-1.7\nfixture";
        let model = b"{}";
        let header = RecipientPdfResponseV1 {
            schema_version: 1,
            model_sha256: eutheto_export::sha256_hex(model),
            pdf_bytes: u32::try_from(bytes.len())?,
            pdf_sha256: eutheto_export::sha256_hex(bytes),
            renderer: identity(),
        };
        let request_len = 12
            + serde_json::to_vec(&RecipientPdfRequestV1 {
                schema_version: 1,
                model_bytes: 2,
                model_sha256: eutheto_export::sha256_hex(model),
            })?
            .len()
            + model.len();
        let wire = frame(&header, bytes)?;
        let mut octal = String::new();
        for byte in &wire {
            write!(octal, "\\{byte:03o}")?;
        }
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("worker");
        let control = OperationControl::Cancellation(eutheto_types::CancellationToken::new());
        let dd = shell_program("dd")?;
        for exit in [0, 7] {
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\n{dd} bs=1 count={request_len} of=/dev/null 2>/dev/null\nprintf '{octal}'\nexit {exit}\n"
                ),
            )?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            let staged = directory.path().join(format!("staged-{exit}"));
            let digest = eutheto_export::sha256_hex(&std::fs::read(&path)?);
            stage_worker(&path, &staged, &digest)
                .await
                .map_err(|error| format!("{error:?}"))?;
            std::fs::write(&path, b"#!/bin/sh\nexit 99\n")?;
            let result = supervise(&staged, &identity(), model, &control).await;
            if result.is_ok() != (exit == 0) {
                let retained = directory.keep();
                return Err(format!(
                    "worker fixture failed: {result:?}; fixture: {}",
                    retained.display()
                )
                .into());
            }
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn dropped_observer_keeps_child_owned_until_cancel_and_reap()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("worker");
        let pid_path = directory.path().join("worker.pid");
        let sleep = shell_program("sleep")?;
        std::fs::write(
            &path,
            format!("#!/bin/sh\nprintf '%s' \"$$\" > \"$0.pid\"\nexec {sleep} 60\n"),
        )?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        let token = eutheto_types::CancellationToken::new();
        let control = OperationControl::Cancellation(token.clone());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let observer = tokio::spawn(async move {
            let outcome = supervise(&path, &identity(), b"{}", &control).await;
            let _ = sender.send(outcome);
        });
        drop(observer);
        let started = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Ok(value) = tokio::fs::read_to_string(&pid_path).await
                    && let Ok(pid) = value.parse::<u32>()
                {
                    break pid;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        token.cancel();
        let pid = started?;
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(3), receiver).await??;
        assert_eq!(
            outcome,
            Err(operation_interrupted(
                eutheto_types::OperationInterruption::Cancelled
            ))
        );
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        Ok(())
    }
}
