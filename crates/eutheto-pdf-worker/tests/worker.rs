// SPDX-License-Identifier: Apache-2.0

use eutheto_types::{
    MAX_RECIPIENT_PDF_BYTES, MAX_RECIPIENT_PDF_HEADER_BYTES, RECIPIENT_PDF_REQUEST_MAGIC,
    RECIPIENT_PDF_RESPONSE_MAGIC, RecipientPdfRequestV1, RecipientPdfResponseV1, RecipientReportV1,
};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn start(model: &[u8]) -> TestResult<Child> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_eutheto-pdf-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let header = serde_json::to_vec(&RecipientPdfRequestV1 {
        schema_version: 1,
        model_bytes: u32::try_from(model.len())?,
        model_sha256: hex::encode(Sha256::digest(model)),
    })?;
    let input = child.stdin.as_mut().ok_or("missing child stdin")?;
    input.write_all(RECIPIENT_PDF_REQUEST_MAGIC)?;
    input.write_all(&u32::try_from(header.len())?.to_be_bytes())?;
    input.write_all(&header)?;
    input.write_all(model)?;
    input.flush()?;
    Ok(child)
}

fn fixture() -> TestResult<Vec<u8>> {
    let report: RecipientReportV1 = serde_json::from_str(include_str!("fixtures/report.json"))?;
    report.validate()?;
    Ok(serde_json::to_vec(&report)?)
}

fn wait(child: &mut Child, timeout: Duration) -> TestResult<ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if start.elapsed() > timeout {
            child.kill()?;
            child.wait()?;
            return Err("PDF helper did not exit within test deadline".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn complete_pdf_exits_with_lifeline_open_and_binds_response() -> TestResult {
    let model = fixture()?;
    let mut child = start(&model)?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take((MAX_RECIPIENT_PDF_BYTES + MAX_RECIPIENT_PDF_HEADER_BYTES + 13) as u64)
            .read_to_end(&mut output)
            .map(|_| output)
    });
    let status = wait(&mut child, Duration::from_mins(1))?;
    let output = reader.join().map_err(|_| "stdout reader panicked")??;
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .ok_or("missing child stderr")?
        .take(4096)
        .read_to_string(&mut stderr)?;
    assert!(status.success(), "worker error: {stderr}");
    assert!(child.stdin.is_some());
    assert_eq!(
        output.get(..8),
        Some(RECIPIENT_PDF_RESPONSE_MAGIC.as_slice())
    );
    let encoded_length: [u8; 4] = output
        .get(8..12)
        .ok_or("missing header length")?
        .try_into()?;
    let length = usize::try_from(u32::from_be_bytes(encoded_length))?;
    assert!(length <= MAX_RECIPIENT_PDF_HEADER_BYTES);
    let response: RecipientPdfResponseV1 = serde_json::from_slice(
        output
            .get(12..12 + length)
            .ok_or("missing response header")?,
    )?;
    let pdf = output.get(12 + length..).ok_or("missing PDF")?;
    assert_eq!(response.schema_version, 1);
    assert_eq!(response.renderer.version, 1);
    assert_eq!(response.renderer.typst_version, "0.15.1");
    assert_eq!(response.model_sha256, hex::encode(Sha256::digest(&model)));
    assert_eq!(response.pdf_sha256, hex::encode(Sha256::digest(pdf)));
    assert_eq!(usize::try_from(response.pdf_bytes)?, pdf.len());
    assert!(pdf.starts_with(b"%PDF-"));
    Ok(())
}

#[test]
fn parent_eof_and_cancel_byte_terminate_active_helper() -> TestResult {
    for cancel_byte in [false, true] {
        let model = fixture()?;
        let mut child = start(&model)?;
        // The representative report is actively compiling with its lifeline retained.
        std::thread::sleep(Duration::from_millis(20));
        assert!(child.try_wait()?.is_none());
        if cancel_byte {
            child
                .stdin
                .as_mut()
                .ok_or("missing stdin")?
                .write_all(&[0])?;
        } else {
            drop(child.stdin.take());
        }
        let status = wait(&mut child, Duration::from_secs(3))?;
        assert_eq!(status.code(), Some(3));
    }
    Ok(())
}

#[test]
fn unsupported_glyph_refuses_instead_of_omitting_content() -> TestResult {
    let mut report: RecipientReportV1 = serde_json::from_slice(&fixture()?)?;
    report.title = eutheto_types::RecipientLabelV1::Authored("Unsupported 中".into());
    let mut child = start(&serde_json::to_vec(&report)?)?;
    assert_eq!(wait(&mut child, Duration::from_secs(5))?.code(), Some(2));
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .ok_or("missing child stderr")?
        .read_to_string(&mut stderr)?;
    assert_eq!(stderr, "PDF_UNSUPPORTED_GLYPH U+4E2D\n");
    Ok(())
}
