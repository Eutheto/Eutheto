// SPDX-License-Identifier: Apache-2.0
//! One request per disposable process; the template World denies external resolution.

mod world;

use eutheto_types::{
    MAX_RECIPIENT_PDF_BYTES, MAX_RECIPIENT_PDF_HEADER_BYTES, MAX_RECIPIENT_REPORT_BYTES,
    RECIPIENT_PDF_PROTOCOL_VERSION, RECIPIENT_PDF_REQUEST_MAGIC, RECIPIENT_PDF_RESPONSE_MAGIC,
    RecipientPdfRendererV1, RecipientPdfRequestV1, RecipientPdfResponseV1, RecipientReportV1,
};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("{code}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    if std::env::args_os().len() != 1 {
        return Err("PDF_UNEXPECTED_ARGUMENT".into());
    }
    let (request, model) = read_request(&mut io::stdin().lock())?;
    // Stdin remains the parent's lifeline while parsing, compiling and writing output.
    // Any byte, EOF or I/O error cancels; there is no command language to parse.
    std::thread::Builder::new()
        .name("pdf-parent-lifeline".into())
        .spawn(|| {
            let _ = io::stdin().lock().read_exact(&mut [0_u8; 1]);
            std::process::exit(3);
        })
        .map_err(|_| "PDF_LIFELINE_UNAVAILABLE")?;

    let (pdf, renderer) = render(&model)?;
    let response = RecipientPdfResponseV1 {
        schema_version: RECIPIENT_PDF_PROTOCOL_VERSION,
        model_sha256: request.model_sha256,
        pdf_bytes: u32::try_from(pdf.len()).map_err(|_| "PDF_OUTPUT_LIMIT")?,
        pdf_sha256: sha256(&pdf),
        renderer,
    };
    write_response(&mut io::stdout().lock(), &response, &pdf)
}

fn read_request(input: &mut impl Read) -> Result<(RecipientPdfRequestV1, Vec<u8>), String> {
    let mut magic = [0_u8; 8];
    input.read_exact(&mut magic).map_err(|_| "PDF_REQUEST_IO")?;
    if &magic != RECIPIENT_PDF_REQUEST_MAGIC {
        return Err("PDF_REQUEST_MAGIC".into());
    }
    let mut encoded_length = [0_u8; 4];
    input
        .read_exact(&mut encoded_length)
        .map_err(|_| "PDF_REQUEST_IO")?;
    let header_length =
        usize::try_from(u32::from_be_bytes(encoded_length)).map_err(|_| "PDF_HEADER_LIMIT")?;
    if header_length == 0 || header_length > MAX_RECIPIENT_PDF_HEADER_BYTES {
        return Err("PDF_HEADER_LIMIT".into());
    }
    let mut header = vec![0; header_length];
    input
        .read_exact(&mut header)
        .map_err(|_| "PDF_REQUEST_IO")?;
    let request: RecipientPdfRequestV1 =
        serde_json::from_slice(&header).map_err(|_| "PDF_REQUEST_HEADER")?;
    let length = usize::try_from(request.model_bytes).map_err(|_| "PDF_MODEL_LIMIT")?;
    if request.schema_version != RECIPIENT_PDF_PROTOCOL_VERSION
        || length == 0
        || length > MAX_RECIPIENT_REPORT_BYTES
        || !valid_digest(&request.model_sha256)
    {
        return Err("PDF_REQUEST_HEADER".into());
    }
    let mut model = vec![0; length];
    input.read_exact(&mut model).map_err(|_| "PDF_REQUEST_IO")?;
    if sha256(&model) != request.model_sha256 {
        return Err("PDF_MODEL_HASH".into());
    }
    Ok((request, model))
}

fn render(model: &[u8]) -> Result<(Vec<u8>, RecipientPdfRendererV1), String> {
    let report: RecipientReportV1 =
        serde_json::from_slice(model).map_err(|_| "PDF_INVALID_MODEL")?;
    report.validate().map_err(|_| "PDF_INVALID_MODEL")?;
    let canonical = serde_json::to_vec(&report).map_err(|_| "PDF_INVALID_MODEL")?;
    if canonical != model {
        return Err("PDF_NONCANONICAL_MODEL".into());
    }
    let world = world::ReportWorld::new(canonical.clone())?;
    // Canonical serde JSON emits non-ASCII scalars directly, including authored labels.
    // Refuse unsupported characters instead of replacing them with invisible/missing glyphs.
    let text = std::str::from_utf8(&canonical).map_err(|_| "PDF_INVALID_MODEL")?;
    for character in text.chars().chain(world::TEMPLATE.chars()) {
        if !world.supports(character) {
            return Err(format!(
                "PDF_UNSUPPORTED_GLYPH U+{:04X}",
                u32::from(character)
            ));
        }
    }
    drop(report);
    drop(canonical);
    let compiled = typst::compile(&world);
    let document = compiled.output.map_err(|_| "PDF_COMPILE_FAILED")?;
    let pdf = typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default())
        .map_err(|_| "PDF_ENCODING_FAILED")?;
    if pdf.len() > MAX_RECIPIENT_PDF_BYTES || !pdf.starts_with(b"%PDF-") {
        return Err("PDF_OUTPUT_LIMIT".into());
    }
    Ok((pdf, renderer_identity()))
}

fn renderer_identity() -> RecipientPdfRendererV1 {
    let mut fonts = Sha256::new();
    for bytes in world::FONT_BYTES {
        fonts.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
        fonts.update(bytes);
    }
    RecipientPdfRendererV1 {
        version: 1,
        typst_version: "0.15.1".into(),
        template_sha256: sha256(world::TEMPLATE.as_bytes()),
        fonts_sha256: hex::encode(fonts.finalize()),
    }
}

fn write_response(
    output: &mut impl Write,
    response: &RecipientPdfResponseV1,
    pdf: &[u8],
) -> Result<(), String> {
    let header = serde_json::to_vec(response).map_err(|_| "PDF_RESPONSE_HEADER")?;
    if header.len() > MAX_RECIPIENT_PDF_HEADER_BYTES {
        return Err("PDF_RESPONSE_HEADER".into());
    }
    let length = u32::try_from(header.len()).map_err(|_| "PDF_RESPONSE_HEADER")?;
    output
        .write_all(RECIPIENT_PDF_RESPONSE_MAGIC)
        .and_then(|()| output.write_all(&length.to_be_bytes()))
        .and_then(|()| output.write_all(&header))
        .and_then(|()| output.write_all(pdf))
        .and_then(|()| output.flush())
        .map_err(|_| "PDF_RESPONSE_IO".into())
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representative_model_compiles_and_renders() -> Result<(), Box<dyn std::error::Error>> {
        let report: RecipientReportV1 =
            serde_json::from_str(include_str!("../tests/fixtures/report.json"))?;
        report.validate()?;
        let model = serde_json::to_vec(&report)?;
        let world = world::ReportWorld::new(model.clone())?;
        let compiled = typst::compile(&world);
        let document = compiled.output.map_err(|errors| format!("{errors:?}"))?;
        let pdf = typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default())
            .map_err(|errors| format!("{errors:?}"))?;
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.len() < MAX_RECIPIENT_PDF_BYTES);
        let (_, identity) = render(&model)?;
        assert_eq!(identity, renderer_identity());
        Ok(())
    }

    #[test]
    fn request_bounds_are_checked_before_payload_reads() -> Result<(), Box<dyn std::error::Error>> {
        for (version, length, digest) in [
            (2, 1, "0".repeat(64)),
            (1, u32::MAX, "0".repeat(64)),
            (1, 0, "0".repeat(64)),
            (1, 1, "A".repeat(64)),
        ] {
            let header = serde_json::to_vec(&RecipientPdfRequestV1 {
                schema_version: version,
                model_bytes: length,
                model_sha256: digest,
            })?;
            let mut frame = RECIPIENT_PDF_REQUEST_MAGIC.to_vec();
            frame.extend(u32::try_from(header.len())?.to_be_bytes());
            frame.extend(header);
            assert_eq!(
                read_request(&mut frame.as_slice()),
                Err("PDF_REQUEST_HEADER".into())
            );
        }
        let mut frame = RECIPIENT_PDF_REQUEST_MAGIC.to_vec();
        frame.extend(u32::MAX.to_be_bytes());
        assert_eq!(
            read_request(&mut frame.as_slice()),
            Err("PDF_HEADER_LIMIT".into())
        );
        Ok(())
    }

    #[test]
    fn request_hash_binds_exact_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let header = serde_json::to_vec(&RecipientPdfRequestV1 {
            schema_version: 1,
            model_bytes: 2,
            model_sha256: sha256(b"{}"),
        })?;
        let mut frame = RECIPIENT_PDF_REQUEST_MAGIC.to_vec();
        frame.extend(u32::try_from(header.len())?.to_be_bytes());
        frame.extend(header);
        frame.extend(b"[]");
        assert_eq!(
            read_request(&mut frame.as_slice()),
            Err("PDF_MODEL_HASH".into())
        );
        Ok(())
    }
}
