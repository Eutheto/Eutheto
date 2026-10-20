// SPDX-License-Identifier: Apache-2.0
//! Bounded PDF helper wire. Hashes identify bytes, never native acceptance.

use serde::{Deserialize, Serialize};

pub const RECIPIENT_PDF_PROTOCOL_VERSION: u32 = 1;
pub const RECIPIENT_PDF_REQUEST_MAGIC: &[u8; 8] = b"EUTPDFQ1";
pub const RECIPIENT_PDF_RESPONSE_MAGIC: &[u8; 8] = b"EUTPDFR1";
pub const MAX_RECIPIENT_PDF_HEADER_BYTES: usize = 4096;
pub const MAX_RECIPIENT_PDF_BYTES: usize = 32 * 1024 * 1024;

/// Follows request magic and a big-endian u32 JSON-header length. Exact canonical
/// report bytes follow this header; stdin then remains open as the parent lifeline.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientPdfRequestV1 {
    pub schema_version: u32,
    pub model_bytes: u32,
    pub model_sha256: String,
}

/// Renderer identity must match the approved helper manifest, not merely itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientPdfRendererV1 {
    pub version: u32,
    pub typst_version: String,
    pub template_sha256: String,
    pub fonts_sha256: String,
}

/// Follows response magic and a big-endian u32 JSON-header length, then `pdf_bytes`
/// raw PDF bytes. The parent rejects missing, oversized, extra, or mismatched data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientPdfResponseV1 {
    pub schema_version: u32,
    pub model_sha256: String,
    pub pdf_bytes: u32,
    pub pdf_sha256: String,
    pub renderer: RecipientPdfRendererV1,
}
