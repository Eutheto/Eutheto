//! Versioned nonsecret application-settings documents and reviewed import receipts.

use crate::{RequestId, Revision, Rfc3339Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Standalone document and desktop settings-operation schema version.
pub const APPLICATION_SETTINGS_SCHEMA_VERSION: u32 = 1;
/// Maximum source document and compact request/response size.
pub const MAX_APPLICATION_SETTINGS_BYTES: usize = 64 * 1024;

/// Identifies a standalone settings document, not a scenario or library backup.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ApplicationSettingsFormat {
    #[serde(rename = "eutheto/application-settings")]
    EuthetoApplicationSettings,
}

/// Complete portable setting entry; timestamps participate in change detection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationSettingEntryV1 {
    pub value: Value,
    pub updated_at: Rfc3339Timestamp,
}

/// Complete replacement of the supported nonsecret settings only.
///
/// Absent keys mean removal after review. Application services validate the
/// version, key/value allowlist and portable policy before retaining authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NonsecretSettingsDocumentV1 {
    pub format: ApplicationSettingsFormat,
    pub schema_version: u32,
    pub settings: BTreeMap<String, ApplicationSettingEntryV1>,
}

/// One reviewed addition, update or removal, ordered by setting key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsChangeV1 {
    pub key: String,
    pub before: Option<ApplicationSettingEntryV1>,
    pub after: Option<ApplicationSettingEntryV1>,
}

/// Bounded review data, not authority to supply replacement values on apply.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsImportPreviewDtoV1 {
    pub schema_version: u32,
    pub preview_id: RequestId,
    pub source_sha256: String,
    pub approval_sha256: String,
    pub library_revision: Revision,
    pub changes: Vec<SettingsChangeV1>,
}

/// Approval of an immutable native/core-owned settings preview.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsImportApplyRequestV1 {
    pub schema_version: u32,
    pub request_id: RequestId,
    pub preview_id: RequestId,
    pub approval_sha256: String,
    pub expected_library_revision: Revision,
}

/// Returned after the owned transaction settles, including an identical no-op.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsImportApplyDtoV1 {
    pub schema_version: u32,
    pub preview_id: RequestId,
    pub library_revision: Revision,
    pub changed: bool,
}
