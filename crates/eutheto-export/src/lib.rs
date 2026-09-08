//! Current-only portable scenario and full-library bundle export.
//!
//! This crate deliberately knows nothing about `SQLite`, credentials, providers,
//! or application services. Callers provide one already-consistent portable
//! snapshot and receive deterministic bytes or an atomically published file.

#[cfg(test)]
extern crate self as eutheto_export;
#[cfg(test)]
#[path = "../../../tests/support/portable_encode.rs"]
mod portable_encode;

use eutheto_types::{
    BundleId, CancellationToken, MAX_SCENARIO_DOCUMENT_BYTES, OperationControl,
    PORTABLE_LARGE_ASSET_BYTES_V1, PortableAsset, PortableDomainDocument, PortableJsonLimits,
    PortableProjectMetadata, Revision, Rfc3339Timestamp, SCENARIO_FORMAT_VERSION,
    SCENARIO_SNAPSHOT_SCHEMA_VERSION, ScenarioDocument, ScenarioFormat, ScenarioId,
    ScenarioMetadata, ScenarioSettings, ScenarioSnapshotV1, SemanticCapability,
    collect_scenario_owned_uuids, collect_self_declared_uuids, extract_asset_references,
    extract_result_dependency, extract_result_id, extract_scenario_references,
    validate_nonsecret_portable_json, validate_nonsecret_portable_json_bytes,
    validate_scenario_owned_uuid_uniqueness,
};
use image::{ImageFormat, ImageReader, Limits as ImageLimits};
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use tempfile::{Builder as TempFileBuilder, NamedTempFile};
use thiserror::Error;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const BUNDLE_FORMAT: &str = "eutheto-bundle";
pub const PORTABLE_SCENARIO_FORMAT: &str = "eutheto/scenario";
pub const CURRENT_BUNDLE_FORMAT_VERSION: u32 = 1;
pub const CURRENT_PORTABLE_SCHEMA_VERSION: u32 = 2;
pub const CHECKSUM_ALGORITHM: &str = "sha256";
pub const MANIFEST_PATH: &str = "manifest.json";
pub const OMITTED_ASSET_MEDIA_TYPE: &str = "eutheto/omitted-asset.v1";
pub const OMITTED_ASSET_FORMAT: &str = "eutheto/omitted-asset";
pub const BACKUP_SELECTION_EXTENSION: &str = "eutheto.backup-selection.v1";
pub const BACKUP_SELECTION_VERSION: u32 = 1;
pub const CHECKSUMS_PATH: &str = "checksums.json";

/// Global portable-v2 wire envelope. Domain records have exactly one pack-owned representation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableScenario {
    pub format: ScenarioFormat,
    pub schema_version: u32,
    pub revision: Revision,
    pub scenario_id: ScenarioId,
    pub metadata: ScenarioMetadata,
    pub settings: ScenarioSettings,
    pub domain: PortableDomainDocument,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PortableProjectMetadata>,
    pub required_capabilities: BTreeSet<SemanticCapability>,
    #[serde(default)]
    pub semantic_extensions: BTreeMap<String, Value>,
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
}

impl PortableScenario {
    /// Copies the host-owned shell without giving a pack authority over its identity or metadata.
    ///
    /// # Errors
    ///
    /// Rejects mismatched pack identity, envelope/payload versions, or malformed requirements.
    /// Callers separately validate the internal snapshot and the registered pack's support.
    pub fn from_snapshot(
        snapshot: &ScenarioSnapshotV1,
        domain: PortableDomainDocument,
    ) -> Result<Self, ExportError> {
        if domain.pack_id != snapshot.document.domain_pack.id
            || domain.schema_version == 0
            || domain.payload.get("schemaVersion").and_then(Value::as_u64)
                != Some(u64::from(domain.schema_version))
        {
            return Err(ExportError::InvalidModel(
                "encoded domain identity or schema version does not match its envelope".to_owned(),
            ));
        }
        for capability in &domain.required_capabilities {
            validate_namespace(&capability.id)?;
            if capability.version == 0 {
                return Err(ExportError::InvalidModel(
                    "domain capability version must be positive".to_owned(),
                ));
            }
        }
        validate_safe_value(&domain.payload, 0)?;
        Ok(Self {
            format: snapshot.format,
            schema_version: CURRENT_PORTABLE_SCHEMA_VERSION,
            revision: snapshot.revision,
            scenario_id: snapshot.document.scenario_id,
            metadata: snapshot.document.metadata.clone(),
            settings: snapshot.document.settings.clone(),
            domain,
            project: snapshot.project.clone(),
            required_capabilities: snapshot.required_capabilities.clone(),
            semantic_extensions: snapshot.semantic_extensions.clone(),
            extensions: snapshot.extensions.clone(),
        })
    }
}

/// One centralized set of limits shared by export and untrusted import.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortableLimits {
    pub max_archive_bytes: u64,
    pub max_total_uncompressed_bytes: u64,
    pub max_entry_bytes: u64,
    pub max_entries: usize,
    pub max_compression_ratio: u64,
    pub max_path_bytes: usize,
    pub max_json_bytes: u64,
    pub max_json_depth: usize,
    pub max_string_bytes: usize,
    pub max_collection_items: usize,
}

pub const PORTABLE_LIMITS: PortableLimits = PortableLimits {
    max_archive_bytes: 64 * 1024 * 1024,
    max_total_uncompressed_bytes: 64 * 1024 * 1024,
    max_entry_bytes: MAX_SCENARIO_DOCUMENT_BYTES,
    max_entries: 4_096,
    max_compression_ratio: 200,
    max_path_bytes: 240,
    max_json_bytes: MAX_SCENARIO_DOCUMENT_BYTES,
    max_json_depth: 128,
    max_string_bytes: 1024 * 1024,
    max_collection_items: 1_000_000,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BundleKind {
    ScenarioExport,
    FullBackup,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationMetadata {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleCounts {
    pub scenarios: u64,
    #[serde(default)]
    pub scenario_revisions: u64,
    pub results: u64,
    pub shared_records: u64,
    pub preferences: u64,
    pub assets: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrityDeclaration {
    pub algorithm: String,
    pub checksums_file: String,
}
/// Manifest declaration bound to one inert asset payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableAssetMetadata {
    pub media_type: String,
    pub redistribution_permitted: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OmittedAssetReason {
    ExcludeAll,
    AboveV1Threshold,
    ImportExcluded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OmittedAssetPlaceholder {
    pub format: String,
    pub version: u32,
    pub reason: OmittedAssetReason,
    pub original_media_type: String,
    pub original_size: u64,
    pub content_sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PortableBackupAssetSelection {
    All,
    ExcludeAll,
    V1Threshold,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackupSelectionScope {
    Scenario,
    Library,
}

/// Stable categories that are never portable, even in a full-library backup.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixedExclusion {
    LocalUndoAndAuditHistory,
    SqliteAndDatabaseInternals,
    CredentialsTokensAndKeychainReferences,
    DeviceLocalPathsAndWindowState,
    LogsCachesAndTemporaryData,
    RedistributionProhibitedProviderData,
    ExecutableContent,
}

impl FixedExclusion {
    pub const ALL: [Self; 7] = [
        Self::LocalUndoAndAuditHistory,
        Self::SqliteAndDatabaseInternals,
        Self::CredentialsTokensAndKeychainReferences,
        Self::DeviceLocalPathsAndWindowState,
        Self::LogsCachesAndTemporaryData,
        Self::RedistributionProhibitedProviderData,
        Self::ExecutableContent,
    ];
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupSelection {
    pub include_results: bool,
    pub asset_selection: PortableBackupAssetSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_bytes: Option<u64>,
    pub excluded_asset_count: u64,
    pub excluded_asset_ids: BTreeSet<String>,
    #[serde(default)]
    pub fixed_exclusions: BTreeSet<FixedExclusion>,
    pub scope: BackupSelectionScope,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleManifest {
    pub format: String,
    pub format_version: u32,
    pub schema_version: u32,
    pub bundle_id: BundleId,
    pub bundle_kind: BundleKind,
    pub created_at: String,
    pub application: ApplicationMetadata,
    pub title: String,
    pub counts: BundleCounts,
    pub required_capabilities: BTreeSet<SemanticCapability>,
    pub nonsemantic_extensions: BTreeSet<String>,
    pub integrity: IntegrityDeclaration,
    #[serde(default)]
    pub asset_metadata: BTreeMap<String, PortableAssetMetadata>,
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Checksums {
    pub algorithm: String,
    pub files: BTreeMap<String, String>,
}

fn validate_owned_identity_families<'a>(
    scenarios: impl IntoIterator<Item = &'a ScenarioSnapshotV1>,
) -> Result<(), ExportError> {
    let mut owners = BTreeMap::new();
    for scenario in scenarios {
        validate_scenario_owned_uuid_uniqueness(scenario)
            .map_err(|error| ExportError::InvalidModel(error.to_string()))?;
        let scenario_id = scenario.document.scenario_id;
        for identity in collect_scenario_owned_uuids(scenario) {
            if let Some(owner) = owners.insert(identity, scenario_id)
                && owner != scenario_id
            {
                return Err(ExportError::InvalidModel(format!(
                    "owned identity {identity} is declared by scenario families {owner} and {scenario_id}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_historical_revision_order(
    scenarios: &[ScenarioSnapshotV1],
    historical_revisions: &[ScenarioSnapshotV1],
) -> Result<(), ExportError> {
    let mut current = BTreeMap::new();
    for scenario in scenarios {
        if current
            .insert(scenario.document.scenario_id, scenario.revision)
            .is_some()
        {
            return Err(ExportError::InvalidModel(format!(
                "duplicate current scenario identity {}",
                scenario.document.scenario_id
            )));
        }
    }
    for historical in historical_revisions {
        let Some(current_revision) = current.get(&historical.document.scenario_id) else {
            return Err(ExportError::InvalidModel(format!(
                "historical revision references scenario {} absent from the bundle",
                historical.document.scenario_id
            )));
        };
        if historical.revision >= *current_revision {
            return Err(ExportError::InvalidModel(format!(
                "historical revision {} for scenario {} must be less than current revision {}",
                historical.revision.value(),
                historical.document.scenario_id,
                current_revision.value()
            )));
        }
    }
    Ok(())
}

/// A single immutable source snapshot for scenario export.
#[derive(Clone, Debug)]
pub struct ScenarioExportSnapshot {
    pub bundle_id: BundleId,
    pub created_at: String,
    pub application: ApplicationMetadata,
    pub title: String,
    pub scenario: ScenarioSnapshotV1,
    /// Exact historical revisions required by the selected retained results.
    pub scenario_revisions: Vec<ScenarioSnapshotV1>,
    pub sections: BackupSections,
    pub nonsemantic_extensions: BTreeSet<String>,
    pub manifest_extensions: BTreeMap<String, Value>,
}

/// Extra portable backup sections. JSON stays structured; assets remain inert bytes.
#[derive(Clone, Debug, Default)]
pub struct BackupSections {
    pub results: BTreeMap<String, Value>,
    pub shared_records: BTreeMap<String, Value>,
    pub preferences: BTreeMap<String, Value>,
    pub assets: BTreeMap<String, PortableAsset>,
}

/// One consistent full-library snapshot, captured by the application before export.
#[derive(Clone, Debug)]
pub struct FullBackupSnapshot {
    pub bundle_id: BundleId,
    pub created_at: String,
    pub application: ApplicationMetadata,
    pub title: String,
    pub scenarios: Vec<ScenarioSnapshotV1>,
    /// Exact non-current revisions required by retained results.
    pub scenario_revisions: Vec<ScenarioSnapshotV1>,
    pub sections: BackupSections,
    pub nonsemantic_extensions: BTreeSet<String>,
    pub manifest_extensions: BTreeMap<String, Value>,
}

/// A violation of the shared portable path, structure, size, or safety policy.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{0}")]
pub struct PortablePolicyViolation(pub String);

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("portable model is invalid: {0}")]
    InvalidModel(String),
    #[error("portable selection omits referenced scenario {0}")]
    MissingScenarioDependency(ScenarioId),
    #[error("portable JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("bundle ZIP operation failed: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("bundle filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("assembled bundle verification failed: {0}")]
    Verification(String),
    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("bundle publication was cancelled")]
    Cancelled,
}

/// Serialize as compact deterministic JSON. Struct field order is declared and
/// maps use ordered collections throughout the portable model.
///
/// # Errors
///
/// Returns [`ExportError::Json`] when `value` cannot be serialized as JSON.
pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, ExportError> {
    Ok(serde_json::to_vec(value)?)
}

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn supported_original_asset_media_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/png" | "image/jpeg" | "text/plain" | "text/plain; charset=utf-8"
    )
}

fn validate_omitted_asset_placeholder(
    placeholder: &OmittedAssetPlaceholder,
) -> Result<(), ExportError> {
    if placeholder.format != OMITTED_ASSET_FORMAT || placeholder.version != 1 {
        return Err(ExportError::InvalidModel(
            "omitted-asset placeholder has an unsupported format or version".to_owned(),
        ));
    }
    if !supported_original_asset_media_type(&placeholder.original_media_type) {
        return Err(ExportError::InvalidModel(
            "omitted-asset placeholder has an unsupported original media type".to_owned(),
        ));
    }
    if placeholder.reason == OmittedAssetReason::AboveV1Threshold
        && placeholder.original_size <= PORTABLE_LARGE_ASSET_BYTES_V1 as u64
    {
        return Err(ExportError::InvalidModel(
            "above-v1-threshold placeholder does not exceed the version-1 threshold".to_owned(),
        ));
    }
    if placeholder.content_sha256.len() != 64
        || !placeholder
            .content_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ExportError::InvalidModel(
            "omitted-asset placeholder contentSha256 must be 64 lowercase hexadecimal characters"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Constructs the exact inert reconnection record for intentionally omitted
/// original asset bytes.
///
/// # Errors
///
/// Returns [`ExportError::InvalidModel`] for an unsupported original media type
/// and propagates canonical JSON serialization failures.
pub fn omitted_asset_placeholder(
    original: &PortableAsset,
    reason: OmittedAssetReason,
) -> Result<PortableAsset, ExportError> {
    if !supported_original_asset_media_type(&original.media_type) {
        return Err(ExportError::InvalidModel(
            "cannot omit an asset with an unsupported original media type".to_owned(),
        ));
    }
    let placeholder = OmittedAssetPlaceholder {
        format: OMITTED_ASSET_FORMAT.to_owned(),
        version: 1,
        reason,
        original_media_type: original.media_type.clone(),
        original_size: u64::try_from(original.bytes.len())
            .map_err(|_| ExportError::InvalidModel("asset size overflow".to_owned()))?,
        content_sha256: sha256_hex(&original.bytes),
    };
    validate_omitted_asset_placeholder(&placeholder)?;
    Ok(PortableAsset {
        bytes: canonical_json(&placeholder)?,
        media_type: OMITTED_ASSET_MEDIA_TYPE.to_owned(),
        redistribution_permitted: true,
    })
}

fn parse_omitted_asset_bytes(bytes: &[u8]) -> Result<OmittedAssetPlaceholder, ExportError> {
    let placeholder: OmittedAssetPlaceholder = serde_json::from_slice(bytes).map_err(|error| {
        ExportError::InvalidModel(format!("invalid omitted-asset placeholder JSON: {error}"))
    })?;
    validate_omitted_asset_placeholder(&placeholder)?;
    if canonical_json(&placeholder)? != bytes {
        return Err(ExportError::InvalidModel(
            "omitted-asset placeholder must use exact canonical JSON".to_owned(),
        ));
    }
    Ok(placeholder)
}

/// Returns whether an asset declares the omitted-asset media type.
#[must_use]
pub fn is_omitted_asset(asset: &PortableAsset) -> bool {
    asset.media_type == OMITTED_ASSET_MEDIA_TYPE
}

/// Decodes and validates an exact canonical omitted-asset reconnection record.
///
/// # Errors
///
/// Returns [`ExportError::InvalidModel`] for malformed, noncanonical, or
/// unsupported placeholder bytes.
pub fn parse_omitted_asset_placeholder(
    asset: &PortableAsset,
) -> Result<Option<OmittedAssetPlaceholder>, ExportError> {
    if !is_omitted_asset(asset) {
        return Ok(None);
    }
    if !asset.redistribution_permitted {
        return Err(ExportError::InvalidModel(
            "omitted-asset placeholder must permit redistribution".to_owned(),
        ));
    }
    Ok(Some(parse_omitted_asset_bytes(&asset.bytes)?))
}

fn validate_backup_selection_shape(selection: &BackupSelection) -> Result<(), ExportError> {
    let excluded_count = u64::try_from(selection.excluded_asset_ids.len())
        .map_err(|_| ExportError::InvalidModel("excluded asset count overflow".to_owned()))?;
    if selection.excluded_asset_count != excluded_count {
        return Err(ExportError::InvalidModel(
            "backup selection excluded asset count does not match its IDs".to_owned(),
        ));
    }
    for asset_id in &selection.excluded_asset_ids {
        validate_segment(asset_id)?;
    }
    if selection.scope == BackupSelectionScope::Library
        && selection.fixed_exclusions != FixedExclusion::ALL.into_iter().collect()
    {
        return Err(ExportError::InvalidModel(
            "full-library backup selection must declare the complete fixed exclusion set"
                .to_owned(),
        ));
    }
    match selection.asset_selection {
        PortableBackupAssetSelection::All => {
            if selection.threshold_version.is_some() || selection.threshold_bytes.is_some() {
                return Err(ExportError::InvalidModel(
                    "all-assets selection cannot declare a threshold".to_owned(),
                ));
            }
        }
        PortableBackupAssetSelection::ExcludeAll => {
            if selection.threshold_version.is_some() || selection.threshold_bytes.is_some() {
                return Err(ExportError::InvalidModel(
                    "exclude-all asset selection cannot declare a threshold".to_owned(),
                ));
            }
        }
        PortableBackupAssetSelection::V1Threshold => {
            if selection.threshold_version != Some(BACKUP_SELECTION_VERSION)
                || selection.threshold_bytes != Some(PORTABLE_LARGE_ASSET_BYTES_V1 as u64)
            {
                return Err(ExportError::InvalidModel(
                    "v1-threshold asset selection must declare the version-1 threshold".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

/// Produces the strict manifest-extension value for one source selection.
///
/// # Errors
///
/// Returns [`ExportError::InvalidModel`] when the selection is inconsistent.
pub fn backup_selection_extension_value(selection: &BackupSelection) -> Result<Value, ExportError> {
    validate_backup_selection_shape(selection)?;
    Ok(serde_json::to_value(selection)?)
}

/// Parses and validates authoritative source-selection metadata from a manifest.
///
/// # Errors
///
/// Returns [`ExportError::InvalidModel`] when the extension schema, counts,
/// scope, or declared omitted-asset identities are inconsistent.
pub fn backup_selection_from_manifest(
    manifest: &BundleManifest,
) -> Result<Option<BackupSelection>, ExportError> {
    let Some(value) = manifest.extensions.get(BACKUP_SELECTION_EXTENSION) else {
        if manifest.bundle_kind == BundleKind::FullBackup {
            return Err(ExportError::InvalidModel(
                "full-library backup requires backup selection metadata".to_owned(),
            ));
        }
        if manifest
            .asset_metadata
            .values()
            .any(|metadata| metadata.media_type == OMITTED_ASSET_MEDIA_TYPE)
        {
            return Err(ExportError::InvalidModel(
                "omitted-asset placeholders require backup selection metadata".to_owned(),
            ));
        }
        return Ok(None);
    };
    let selection: BackupSelection = serde_json::from_value(value.clone()).map_err(|error| {
        ExportError::InvalidModel(format!("invalid {BACKUP_SELECTION_EXTENSION}: {error}"))
    })?;
    validate_backup_selection_shape(&selection)?;
    let expected_scope = match manifest.bundle_kind {
        BundleKind::ScenarioExport => BackupSelectionScope::Scenario,
        BundleKind::FullBackup => BackupSelectionScope::Library,
    };
    if selection.scope != expected_scope {
        return Err(ExportError::InvalidModel(
            "backup selection scope does not match bundle kind".to_owned(),
        ));
    }
    if !selection.include_results && manifest.counts.results != 0 {
        return Err(ExportError::InvalidModel(
            "backup selection excludes results but the bundle contains results".to_owned(),
        ));
    }
    let placeholder_ids = manifest
        .asset_metadata
        .iter()
        .filter_map(|(id, metadata)| {
            (metadata.media_type == OMITTED_ASSET_MEDIA_TYPE).then_some(id.clone())
        })
        .collect::<BTreeSet<_>>();
    if placeholder_ids != selection.excluded_asset_ids {
        return Err(ExportError::InvalidModel(
            "backup selection excluded asset IDs do not match omitted-asset placeholders"
                .to_owned(),
        ));
    }
    if selection.asset_selection == PortableBackupAssetSelection::ExcludeAll
        && manifest.asset_metadata.len() != placeholder_ids.len()
    {
        return Err(ExportError::InvalidModel(
            "exclude-all asset selection contains original asset bytes".to_owned(),
        ));
    }
    Ok(Some(selection))
}

/// Assemble a deterministic scenario-export archive from a consistent snapshot.
///
/// # Errors
///
/// Returns an error when the snapshot violates the portable-data policy, a
/// selected result lacks its exact scenario revision, or archive assembly fails.
pub fn assemble_scenario_export(
    snapshot: &ScenarioExportSnapshot,
    encode_domain: &impl Fn(&ScenarioDocument) -> Result<PortableDomainDocument, ExportError>,
) -> Result<Vec<u8>, ExportError> {
    let exported_scenario = &snapshot.scenario;
    validate_portable_scenario_inner(exported_scenario)?;
    validate_owned_identity_families(
        std::iter::once(exported_scenario).chain(&snapshot.scenario_revisions),
    )?;
    validate_historical_revision_order(
        std::slice::from_ref(exported_scenario),
        &snapshot.scenario_revisions,
    )?;
    validate_scenario_reference_closure(
        std::slice::from_ref(exported_scenario),
        &snapshot.sections,
    )?;
    validate_asset_reference_closure(
        std::iter::once(exported_scenario).chain(&snapshot.scenario_revisions),
        &snapshot.sections,
    )?;
    validate_global_identity_namespace(
        &snapshot.sections,
        std::iter::once(exported_scenario).chain(&snapshot.scenario_revisions),
    )?;
    validate_result_dependencies(
        &snapshot.sections.results,
        std::iter::once((
            exported_scenario.document.scenario_id,
            exported_scenario.revision,
        ))
        .chain(
            snapshot
                .scenario_revisions
                .iter()
                .map(|scenario| (scenario.document.scenario_id, scenario.revision)),
        ),
    )?;
    let scenario_id = exported_scenario.document.scenario_id.to_string();
    validate_segment(&scenario_id)?;
    let mut payloads = BTreeMap::new();
    let mut portable = PortableScenario::from_snapshot(
        exported_scenario,
        encode_domain(&exported_scenario.document)?,
    )?;
    portable.project = None;
    payloads.insert(
        format!("scenarios/{scenario_id}.json"),
        Cow::Owned(canonical_json(&portable)?),
    );
    let asset_metadata = append_sections(&mut payloads, &snapshot.sections)?;
    let mut capabilities = exported_scenario.required_capabilities.clone();
    capabilities.extend(portable.domain.required_capabilities);
    let mut extensions: BTreeSet<String> = exported_scenario.extensions.keys().cloned().collect();
    extensions.extend(exported_scenario.document.extensions.keys().cloned());
    extensions.extend(snapshot.nonsemantic_extensions.iter().cloned());
    append_scenario_revisions(
        &mut payloads,
        &snapshot.scenario_revisions,
        &mut capabilities,
        &mut extensions,
        encode_domain,
    )?;
    let manifest = manifest(ManifestInput {
        bundle_id: snapshot.bundle_id,
        bundle_kind: BundleKind::ScenarioExport,
        created_at: &snapshot.created_at,
        application: &snapshot.application,
        title: &snapshot.title,
        counts: section_counts(1, snapshot.scenario_revisions.len(), &snapshot.sections)?,
        required_capabilities: capabilities,
        nonsemantic_extensions: extensions,
        asset_metadata,
        extensions: &snapshot.manifest_extensions,
    });
    assemble(&manifest, payloads)
}

/// Assemble a deterministic full-library backup from a consistent snapshot.
///
/// # Errors
pub fn assemble_full_backup(
    snapshot: &FullBackupSnapshot,
    encode_domain: &impl Fn(&ScenarioDocument) -> Result<PortableDomainDocument, ExportError>,
) -> Result<Vec<u8>, ExportError> {
    validate_owned_identity_families(
        snapshot
            .scenarios
            .iter()
            .chain(&snapshot.scenario_revisions),
    )?;
    validate_historical_revision_order(&snapshot.scenarios, &snapshot.scenario_revisions)?;
    validate_scenario_reference_closure(&snapshot.scenarios, &snapshot.sections)?;
    validate_asset_reference_closure(
        snapshot
            .scenarios
            .iter()
            .chain(&snapshot.scenario_revisions),
        &snapshot.sections,
    )?;
    validate_global_identity_namespace(
        &snapshot.sections,
        snapshot
            .scenarios
            .iter()
            .chain(&snapshot.scenario_revisions),
    )?;
    validate_result_dependencies(
        &snapshot.sections.results,
        snapshot
            .scenarios
            .iter()
            .chain(&snapshot.scenario_revisions)
            .map(|scenario| (scenario.document.scenario_id, scenario.revision)),
    )?;
    let mut payloads = BTreeMap::new();
    let mut capabilities = BTreeSet::new();
    let mut nonsemantic_extensions = BTreeSet::new();
    for scenario in &snapshot.scenarios {
        validate_portable_scenario_inner(scenario)?;
        let mut portable =
            PortableScenario::from_snapshot(scenario, encode_domain(&scenario.document)?)?;
        portable
            .project
            .get_or_insert_with(PortableProjectMetadata::default);
        let id = scenario.document.scenario_id.to_string();
        validate_segment(&id)?;
        let path = format!("scenarios/{id}.json");
        if payloads
            .insert(path, Cow::Owned(canonical_json(&portable)?))
            .is_some()
        {
            return Err(ExportError::InvalidModel(format!(
                "duplicate scenario identity {id}"
            )));
        }
        capabilities.extend(portable.required_capabilities);
        capabilities.extend(portable.domain.required_capabilities);
        nonsemantic_extensions.extend(scenario.extensions.keys().cloned());
        nonsemantic_extensions.extend(scenario.document.extensions.keys().cloned());
    }
    nonsemantic_extensions.extend(snapshot.nonsemantic_extensions.iter().cloned());
    append_scenario_revisions(
        &mut payloads,
        &snapshot.scenario_revisions,
        &mut capabilities,
        &mut nonsemantic_extensions,
        encode_domain,
    )?;
    let asset_metadata = append_sections(&mut payloads, &snapshot.sections)?;
    let manifest = manifest(ManifestInput {
        bundle_id: snapshot.bundle_id,
        bundle_kind: BundleKind::FullBackup,
        created_at: &snapshot.created_at,
        application: &snapshot.application,
        title: &snapshot.title,
        counts: section_counts(
            snapshot.scenarios.len(),
            snapshot.scenario_revisions.len(),
            &snapshot.sections,
        )?,
        required_capabilities: capabilities,
        nonsemantic_extensions,
        asset_metadata,
        extensions: &snapshot.manifest_extensions,
    });
    assemble(&manifest, payloads)
}

struct ManifestInput<'a> {
    bundle_id: BundleId,
    bundle_kind: BundleKind,
    created_at: &'a str,
    application: &'a ApplicationMetadata,
    asset_metadata: BTreeMap<String, PortableAssetMetadata>,
    title: &'a str,
    counts: BundleCounts,
    required_capabilities: BTreeSet<SemanticCapability>,
    nonsemantic_extensions: BTreeSet<String>,
    extensions: &'a BTreeMap<String, Value>,
}

fn manifest(input: ManifestInput<'_>) -> BundleManifest {
    BundleManifest {
        format: BUNDLE_FORMAT.to_owned(),
        format_version: CURRENT_BUNDLE_FORMAT_VERSION,
        schema_version: CURRENT_PORTABLE_SCHEMA_VERSION,
        bundle_id: input.bundle_id,
        bundle_kind: input.bundle_kind,
        created_at: input.created_at.to_owned(),
        asset_metadata: input.asset_metadata,
        application: input.application.clone(),
        title: input.title.to_owned(),
        counts: input.counts,
        required_capabilities: input.required_capabilities,
        nonsemantic_extensions: input.nonsemantic_extensions,
        integrity: IntegrityDeclaration {
            algorithm: CHECKSUM_ALGORITHM.to_owned(),
            checksums_file: CHECKSUMS_PATH.to_owned(),
        },
        extensions: input.extensions.clone(),
    }
}

fn validate_scenario_reference_closure(
    scenarios: &[ScenarioSnapshotV1],
    sections: &BackupSections,
) -> Result<(), ExportError> {
    let represented = scenarios
        .iter()
        .map(|scenario| scenario.document.scenario_id)
        .collect::<BTreeSet<_>>();
    for scenario in scenarios {
        let value = serde_json::to_value(scenario)?;
        let references = extract_scenario_references(&value)
            .map_err(|error| ExportError::InvalidModel(error.to_string()))?;
        if let Some(missing) = references.difference(&represented).next() {
            return Err(ExportError::MissingScenarioDependency(*missing));
        }
    }
    for value in sections
        .shared_records
        .values()
        .chain(sections.preferences.values())
    {
        let references = extract_scenario_references(value)
            .map_err(|error| ExportError::InvalidModel(error.to_string()))?;
        if let Some(missing) = references.difference(&represented).next() {
            return Err(ExportError::MissingScenarioDependency(*missing));
        }
    }
    Ok(())
}

fn validate_asset_reference_closure<'a>(
    scenarios: impl IntoIterator<Item = &'a ScenarioSnapshotV1>,
    sections: &BackupSections,
) -> Result<(), ExportError> {
    let available = sections.assets.keys().cloned().collect::<BTreeSet<_>>();
    for scenario in scenarios {
        let value = serde_json::to_value(scenario)?;
        let references = extract_asset_references(&value)
            .map_err(|error| ExportError::InvalidModel(error.to_string()))?;
        if let Some(missing) = references.difference(&available).next() {
            return Err(ExportError::InvalidModel(format!(
                "portable selection omits referenced asset {missing}"
            )));
        }
    }
    for value in sections
        .results
        .values()
        .chain(sections.shared_records.values())
        .chain(sections.preferences.values())
    {
        let references = extract_asset_references(value)
            .map_err(|error| ExportError::InvalidModel(error.to_string()))?;
        if let Some(missing) = references.difference(&available).next() {
            return Err(ExportError::InvalidModel(format!(
                "portable selection omits referenced asset {missing}"
            )));
        }
    }
    Ok(())
}

fn append_json_section(
    payloads: &mut BTreeMap<String, Cow<'_, [u8]>>,
    directory: &str,
    values: &BTreeMap<String, Value>,
) -> Result<(), ExportError> {
    for (id, value) in values {
        validate_segment(id)?;
        validate_safe_value(value, 0)?;
        payloads.insert(
            format!("{directory}/{id}.json"),
            Cow::Owned(canonical_json(value)?),
        );
    }
    Ok(())
}
fn section_counts(
    scenario_count: usize,
    scenario_revision_count: usize,
    sections: &BackupSections,
) -> Result<BundleCounts, ExportError> {
    Ok(BundleCounts {
        scenarios: usize_to_u64(scenario_count)?,
        scenario_revisions: usize_to_u64(scenario_revision_count)?,
        results: usize_to_u64(sections.results.len())?,
        shared_records: usize_to_u64(sections.shared_records.len())?,
        preferences: usize_to_u64(sections.preferences.len())?,
        assets: usize_to_u64(sections.assets.len())?,
    })
}

fn append_scenario_revisions(
    payloads: &mut BTreeMap<String, Cow<'_, [u8]>>,
    scenario_revisions: &[ScenarioSnapshotV1],
    capabilities: &mut BTreeSet<SemanticCapability>,
    nonsemantic_extensions: &mut BTreeSet<String>,
    encode_domain: &impl Fn(&ScenarioDocument) -> Result<PortableDomainDocument, ExportError>,
) -> Result<(), ExportError> {
    let mut represented = BTreeSet::new();
    for scenario in scenario_revisions {
        if scenario.project.is_some() {
            return Err(ExportError::InvalidModel(
                "historical scenario revisions cannot contain project metadata".to_owned(),
            ));
        }
        validate_portable_scenario_inner(scenario)?;
        let identity = (scenario.document.scenario_id, scenario.revision);
        if !represented.insert(identity) {
            return Err(ExportError::InvalidModel(format!(
                "duplicate historical scenario revision {} at {}",
                identity.0,
                identity.1.value()
            )));
        }
        let name = format!("{}-{}.json", identity.0, identity.1.value());
        validate_segment(&name)?;
        let portable =
            PortableScenario::from_snapshot(scenario, encode_domain(&scenario.document)?)?;
        if payloads
            .insert(
                format!("scenario-revisions/{name}"),
                Cow::Owned(canonical_json(&portable)?),
            )
            .is_some()
        {
            return Err(ExportError::InvalidModel(
                "duplicate historical scenario revision path".to_owned(),
            ));
        }
        capabilities.extend(scenario.required_capabilities.iter().cloned());
        capabilities.extend(portable.domain.required_capabilities);
        nonsemantic_extensions.extend(scenario.extensions.keys().cloned());
        nonsemantic_extensions.extend(scenario.document.extensions.keys().cloned());
    }
    Ok(())
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum ExportIdentityOwner {
    ScenarioFamily(ScenarioId),
    Supplemental(&'static str, String),
}

fn register_export_identity(
    owners: &mut BTreeMap<Uuid, ExportIdentityOwner>,
    identity: Uuid,
    owner: ExportIdentityOwner,
) -> Result<(), ExportError> {
    if let Some(existing) = owners.get(&identity) {
        if existing == &owner {
            return Ok(());
        }
        return Err(ExportError::InvalidModel(format!(
            "owned identity {identity} is assigned to more than one portable record"
        )));
    }
    owners.insert(identity, owner);
    Ok(())
}

fn validate_global_identity_namespace<'a, I>(
    sections: &BackupSections,
    scenarios: I,
) -> Result<(), ExportError>
where
    I: IntoIterator<Item = &'a ScenarioSnapshotV1>,
{
    let mut owners = BTreeMap::new();
    for scenario in scenarios {
        let owner = ExportIdentityOwner::ScenarioFamily(scenario.document.scenario_id);
        for identity in collect_scenario_owned_uuids(scenario) {
            register_export_identity(&mut owners, identity, owner.clone())?;
        }
    }
    for (section, values) in [
        ("results", &sections.results),
        ("shared", &sections.shared_records),
        ("preferences", &sections.preferences),
    ] {
        for (key, value) in values {
            let owner = ExportIdentityOwner::Supplemental(section, key.clone());
            let key_identity = if section == "results" {
                Some(extract_result_id(value).map_err(|error| {
                    ExportError::InvalidModel(format!(
                        "retained result {key} has invalid identity: {error}"
                    ))
                })?)
            } else {
                Uuid::parse_str(key.split('.').next().unwrap_or(key)).ok()
            };
            if let Some(identity) = key_identity {
                register_export_identity(&mut owners, identity, owner.clone())?;
            }
            for identity in collect_self_declared_uuids(value) {
                register_export_identity(&mut owners, identity, owner.clone())?;
            }
        }
    }
    for key in sections.assets.keys() {
        if let Ok(identity) = Uuid::parse_str(key.split('.').next().unwrap_or(key)) {
            register_export_identity(
                &mut owners,
                identity,
                ExportIdentityOwner::Supplemental("assets", key.clone()),
            )?;
        }
    }
    Ok(())
}

fn validate_result_dependencies<I>(
    results: &BTreeMap<String, Value>,
    represented: I,
) -> Result<(), ExportError>
where
    I: IntoIterator<Item = (ScenarioId, Revision)>,
{
    let represented = represented.into_iter().collect::<BTreeSet<_>>();
    for (key, result) in results {
        let result_id = extract_result_id(result).map_err(|error| {
            ExportError::InvalidModel(format!(
                "retained result {key} has invalid identity: {error}"
            ))
        })?;
        if key != &result_id.to_string() {
            return Err(ExportError::InvalidModel(format!(
                "retained result key {key} does not match resultId {result_id}"
            )));
        }
        let dependency = extract_result_dependency(result).map_err(|error| {
            ExportError::InvalidModel(format!(
                "retained result {key} has invalid dependency: {error}"
            ))
        })?;
        if !represented.contains(&(dependency.scenario_id, dependency.scenario_revision)) {
            return Err(ExportError::InvalidModel(format!(
                "retained result {key} requires scenario {} at exact revision {}",
                dependency.scenario_id,
                dependency.scenario_revision.value()
            )));
        }
    }
    Ok(())
}

fn append_sections<'a>(
    payloads: &mut BTreeMap<String, Cow<'a, [u8]>>,
    sections: &'a BackupSections,
) -> Result<BTreeMap<String, PortableAssetMetadata>, ExportError> {
    append_json_section(payloads, "results", &sections.results)?;
    append_json_section(payloads, "shared", &sections.shared_records)?;
    append_json_section(payloads, "preferences", &sections.preferences)?;
    let mut metadata = BTreeMap::new();
    for (name, asset) in &sections.assets {
        validate_segment(name)?;
        validate_portable_asset(name, asset)?;
        payloads.insert(
            format!("assets/{name}"),
            Cow::Borrowed(asset.bytes.as_slice()),
        );
        metadata.insert(
            name.clone(),
            PortableAssetMetadata {
                media_type: asset.media_type.clone(),
                redistribution_permitted: asset.redistribution_permitted,
            },
        );
    }
    Ok(metadata)
}

/// Validates the frozen internal scenario snapshot independently of the portable wire version.
///
/// # Errors
///
/// Returns [`ExportError::InvalidModel`] for non-current envelopes, non-v7
/// identities, invalid namespaces, undeclared semantics, or unsafe JSON data.
pub fn validate_scenario_snapshot(scenario: &ScenarioSnapshotV1) -> Result<(), ExportError> {
    validate_portable_scenario_inner(scenario)?;
    if u64::try_from(canonical_json(scenario)?.len())
        .map_or(true, |size| size > MAX_SCENARIO_DOCUMENT_BYTES)
    {
        return Err(ExportError::InvalidModel(
            "scenario JSON exceeds the scenario byte limit".to_owned(),
        ));
    }
    Ok(())
}

fn validate_portable_scenario_inner(scenario: &ScenarioSnapshotV1) -> Result<(), ExportError> {
    if scenario.format != ScenarioFormat::EuthetoScenario {
        return Err(ExportError::InvalidModel(format!(
            "scenario format must be {PORTABLE_SCENARIO_FORMAT}"
        )));
    }
    if scenario.schema_version != SCENARIO_SNAPSHOT_SCHEMA_VERSION {
        return Err(ExportError::InvalidModel(format!(
            "internal snapshot must use schema version {SCENARIO_SNAPSHOT_SCHEMA_VERSION}"
        )));
    }
    if scenario.document.format_version != SCENARIO_FORMAT_VERSION {
        return Err(ExportError::InvalidModel(
            "scenario document is not at the current envelope version".to_owned(),
        ));
    }
    if scenario.document.scenario_id.as_uuid().get_version_num() != 7 {
        return Err(ExportError::InvalidModel(
            "scenario identity must be UUIDv7".to_owned(),
        ));
    }
    for id in scenario
        .document
        .domain
        .entities
        .keys()
        .map(|id| id.as_uuid())
        .chain(scenario.document.domain.rules.keys().map(|id| id.as_uuid()))
        .chain(
            scenario
                .document
                .domain
                .preferences
                .keys()
                .map(|id| id.as_uuid()),
        )
        .chain(
            scenario
                .document
                .domain
                .locked_assignments
                .keys()
                .map(|id| id.as_uuid()),
        )
    {
        if id.get_version_num() != 7 {
            return Err(ExportError::InvalidModel(
                "scenario-owned identity must be UUIDv7".to_owned(),
            ));
        }
    }
    for capability in &scenario.required_capabilities {
        validate_namespace(&capability.id)?;
        if capability.version == 0 {
            return Err(ExportError::InvalidModel(
                "semantic capability version must be positive".to_owned(),
            ));
        }
    }
    for namespace in scenario
        .extensions
        .keys()
        .chain(scenario.document.extensions.keys())
    {
        validate_namespace(namespace)?;
    }
    for namespace in scenario.semantic_extensions.keys() {
        if !scenario
            .required_capabilities
            .iter()
            .any(|capability| capability.id == *namespace)
        {
            return Err(ExportError::InvalidModel(format!(
                "semantic extension {namespace} lacks a required capability"
            )));
        }
    }
    let value = serde_json::to_value(scenario)?;
    validate_safe_value(&value, 0)
}

fn validate_safe_value(value: &Value, depth: usize) -> Result<(), ExportError> {
    validate_portable_json_value(value, &PORTABLE_LIMITS, depth)
        .map_err(|error| ExportError::InvalidModel(error.0))
}

/// Applies the shared bounded JSON and prohibited-data policy.
///
/// # Errors
///
/// Returns a policy violation for excessive nesting, strings or collections,
/// local paths, secret-bearing keys, or provider-restricted values.
pub fn validate_portable_json_value(
    value: &Value,
    limits: &PortableLimits,
    depth: usize,
) -> Result<(), PortablePolicyViolation> {
    if depth > limits.max_json_depth {
        return Err(PortablePolicyViolation(
            "JSON nesting limit exceeded".to_owned(),
        ));
    }
    validate_nonsecret_portable_json(
        value,
        &PortableJsonLimits {
            max_depth: limits.max_json_depth - depth,
            max_string_bytes: limits.max_string_bytes,
            max_collection_items: limits.max_collection_items,
        },
    )
    .map_err(|error| PortablePolicyViolation(error.0))
}

fn validate_namespace(namespace: &str) -> Result<(), ExportError> {
    if !eutheto_types::is_portable_namespace(namespace) {
        return Err(ExportError::InvalidModel(format!(
            "invalid namespaced portable identifier {namespace:?}"
        )));
    }
    Ok(())
}

fn validate_segment(segment: &str) -> Result<(), ExportError> {
    validate_portable_path(segment, PORTABLE_LIMITS.max_path_bytes)
        .map_err(|error| ExportError::InvalidModel(error.0))?;
    if segment.contains('/') {
        return Err(ExportError::InvalidModel(format!(
            "invalid portable path segment {segment:?}"
        )));
    }
    Ok(())
}

/// Validates one generated bundle-internal path using a single ASCII policy.
///
/// # Errors
///
/// Rejects non-ASCII or non-normal paths, Windows aliases and ADS syntax,
/// traversal, trailing dots/spaces, backslashes, drive paths, and UNC paths.
pub fn validate_portable_path(path: &str, max_bytes: usize) -> Result<(), PortablePolicyViolation> {
    if path.is_empty() || path.len() > max_bytes {
        return Err(PortablePolicyViolation("empty or too long path".to_owned()));
    }
    if !path.is_ascii() {
        return Err(PortablePolicyViolation(
            "non-ASCII portable path".to_owned(),
        ));
    }
    if path.starts_with('/') || path.starts_with("//") || path.contains('\\') {
        return Err(PortablePolicyViolation(
            "absolute, UNC, or backslash path".to_owned(),
        ));
    }
    for segment in path.split('/') {
        if segment.is_empty()
            || matches!(segment, "." | "..")
            || segment.ends_with(['.', ' '])
            || segment.contains(':')
            || segment
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte == 0x7f)
        {
            return Err(PortablePolicyViolation(format!(
                "invalid portable path segment {segment:?}"
            )));
        }
        let basename = segment
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let numbered_device = (basename.len() == 4)
            && (basename.starts_with("COM") || basename.starts_with("LPT"))
            && matches!(basename.as_bytes()[3], b'1'..=b'9');
        if matches!(basename.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$") || numbered_device
        {
            return Err(PortablePolicyViolation(format!(
                "Windows reserved portable path segment {segment:?}"
            )));
        }
    }
    Ok(())
}

/// Validates one declared asset against the bounded inert allowlist.
///
/// # Errors
///
/// Rejects missing redistribution permission, mismatched extension/media type,
/// malformed or oversized images, and non-UTF-8 or active plain text.
pub fn validate_portable_asset(name: &str, asset: &PortableAsset) -> Result<(), ExportError> {
    let metadata = PortableAssetMetadata {
        media_type: asset.media_type.clone(),
        redistribution_permitted: asset.redistribution_permitted,
    };
    validate_asset_bytes(name, &asset.bytes, &metadata)
        .map_err(|error| ExportError::InvalidModel(error.0))
}

fn has_exact_lowercase_suffix(name: &str, suffix: &[u8]) -> bool {
    name.as_bytes().strip_suffix(suffix).is_some()
}

fn validate_asset_bytes(
    name: &str,
    bytes: &[u8],
    metadata: &PortableAssetMetadata,
) -> Result<(), PortablePolicyViolation> {
    if !metadata.redistribution_permitted {
        return Err(PortablePolicyViolation(format!(
            "asset {name} is not permitted for redistribution"
        )));
    }
    reject_prohibited_portable_content(name, bytes)?;
    let lower = name.to_ascii_lowercase();
    match metadata.media_type.as_str() {
        OMITTED_ASSET_MEDIA_TYPE => parse_omitted_asset_bytes(bytes)
            .map(|_| ())
            .map_err(|error| PortablePolicyViolation(error.to_string())),
        "image/png" if has_exact_lowercase_suffix(&lower, b".png") => validate_png(name, bytes),
        "image/jpeg"
            if has_exact_lowercase_suffix(&lower, b".jpg")
                || has_exact_lowercase_suffix(&lower, b".jpeg") =>
        {
            validate_jpeg(name, bytes)
        }
        "text/plain" | "text/plain; charset=utf-8"
            if has_exact_lowercase_suffix(&lower, b".txt") =>
        {
            let text = std::str::from_utf8(bytes).map_err(|_| {
                PortablePolicyViolation(format!("plain-text asset {name} must be UTF-8"))
            })?;
            if text.contains('\0') {
                return Err(PortablePolicyViolation(format!(
                    "plain-text asset {name} contains binary data"
                )));
            }
            validate_nonsecret_portable_json(
                &Value::String(text.to_owned()),
                &PortableJsonLimits {
                    max_depth: 1,
                    max_string_bytes: PORTABLE_LIMITS.max_string_bytes,
                    max_collection_items: 1,
                },
            )
            .map_err(|error| PortablePolicyViolation(error.0))
        }
        _ => Err(PortablePolicyViolation(format!(
            "asset {name} extension and media type do not match the inert allowlist"
        ))),
    }
}
fn validate_png(name: &str, bytes: &[u8]) -> Result<(), PortablePolicyViolation> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || !png_has_exact_stream_end(bytes) {
        return Err(PortablePolicyViolation(format!(
            "asset {name} is not a complete PNG stream"
        )));
    }
    decode_image(name, bytes, ImageFormat::Png)
}

fn png_has_exact_stream_end(bytes: &[u8]) -> bool {
    let mut cursor = 8_usize;
    while cursor.checked_add(12).is_some_and(|end| end <= bytes.len()) {
        let length = u32::from_be_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]);
        let Ok(length) = usize::try_from(length) else {
            return false;
        };
        let Some(chunk_end) = cursor
            .checked_add(12)
            .and_then(|value| value.checked_add(length))
        else {
            return false;
        };
        if chunk_end > bytes.len() {
            return false;
        }
        let chunk_type = &bytes[cursor + 4..cursor + 8];
        if chunk_type == b"IEND" {
            return length == 0 && chunk_end == bytes.len();
        }
        cursor = chunk_end;
    }
    false
}

fn validate_jpeg(name: &str, bytes: &[u8]) -> Result<(), PortablePolicyViolation> {
    if !jpeg_has_exact_stream_end(bytes) {
        return Err(PortablePolicyViolation(format!(
            "asset {name} is not a complete JPEG stream"
        )));
    }
    decode_image(name, bytes, ImageFormat::Jpeg)
}

fn jpeg_has_exact_stream_end(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"\xff\xd8") {
        return false;
    }
    let mut cursor = 2_usize;
    loop {
        let marker_start = cursor;
        if bytes.get(cursor) != Some(&0xff) {
            return false;
        }
        while bytes.get(cursor) == Some(&0xff) {
            cursor = cursor.saturating_add(1);
        }
        let Some(&marker) = bytes.get(cursor) else {
            return false;
        };
        cursor = cursor.saturating_add(1);
        if marker == 0xd9 {
            return cursor == bytes.len();
        }
        if marker == 0x00 || marker == 0xd8 {
            return false;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let Some(length_bytes) = bytes.get(cursor..cursor.saturating_add(2)) else {
            return false;
        };
        let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
        if length < 2 {
            return false;
        }
        let Some(segment_end) = cursor.checked_add(length) else {
            return false;
        };
        if segment_end > bytes.len() {
            return false;
        }
        cursor = segment_end;
        if marker != 0xda {
            continue;
        }
        loop {
            let Some(relative) = bytes[cursor..].iter().position(|byte| *byte == 0xff) else {
                return false;
            };
            let scan_marker_start = cursor + relative;
            cursor = scan_marker_start.saturating_add(1);
            while bytes.get(cursor) == Some(&0xff) {
                cursor = cursor.saturating_add(1);
            }
            let Some(&scan_marker) = bytes.get(cursor) else {
                return false;
            };
            if scan_marker == 0x00 || (0xd0..=0xd7).contains(&scan_marker) {
                cursor = cursor.saturating_add(1);
                continue;
            }
            cursor = scan_marker_start;
            break;
        }
        if cursor <= marker_start {
            return false;
        }
    }
}

fn decode_image(
    name: &str,
    bytes: &[u8],
    format: ImageFormat,
) -> Result<(), PortablePolicyViolation> {
    const MAX_DIMENSION: u32 = 8_192;
    const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;
    if u64::try_from(bytes.len()).map_or(true, |size| size > PORTABLE_LIMITS.max_entry_bytes) {
        return Err(PortablePolicyViolation(format!(
            "asset {name} exceeds its encoded byte limit"
        )));
    }
    let mut limits = ImageLimits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| PortablePolicyViolation(format!("asset {name} is not a decodable image")))?;
    validate_image_dimensions(name, decoded.width(), decoded.height())
}

fn validate_image_dimensions(
    name: &str,
    width: u32,
    height: u32,
) -> Result<(), PortablePolicyViolation> {
    const MAX_DIMENSION: u32 = 8_192;
    const MAX_PIXELS: u64 = 16_777_216;
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(PortablePolicyViolation(format!(
            "asset {name} image dimensions exceed the inert asset limit"
        )));
    }
    Ok(())
}

/// Rejects executable, database, device, and nested-archive content by both
/// filename and magic bytes.
///
/// # Errors
///
/// Returns a policy violation when the entry is unsafe portable content.
pub fn reject_prohibited_portable_content(
    name: &str,
    bytes: &[u8],
) -> Result<(), PortablePolicyViolation> {
    let lower = name.to_ascii_lowercase();
    let forbidden_suffixes = [
        ".zip",
        ".eutheto",
        ".tar",
        ".tar.gz",
        ".tgz",
        ".gz",
        ".bz2",
        ".xz",
        ".7z",
        ".rar",
        ".sqlite",
        ".sqlite3",
        ".db",
        ".db-shm",
        ".db-wal",
        ".exe",
        ".dll",
        ".so",
        ".dylib",
        ".bat",
        ".cmd",
        ".com",
        ".ps1",
        ".sh",
        ".js",
        ".jar",
        ".msi",
        ".wasm",
        ".html",
        ".htm",
        ".svg",
        ".xml",
        ".xsl",
        ".xslt",
        ".mustache",
        ".hbs",
        ".jinja",
        ".tmpl",
        ".dotm",
        ".xlsm",
        ".docm",
        ".pptm",
    ];
    let executable_magic = [
        b"\xfe\xed\xfa\xce".as_slice(),
        b"\xfe\xed\xfa\xcf".as_slice(),
        b"\xce\xfa\xed\xfe".as_slice(),
        b"\xcf\xfa\xed\xfe".as_slice(),
        b"\xca\xfe\xba\xbe".as_slice(),
        b"\x00asm".as_slice(),
    ];
    let archive_magic = [
        b"PK\x03\x04".as_slice(),
        b"\x1f\x8b".as_slice(),
        b"BZh".as_slice(),
        b"\xfd7zXZ\x00".as_slice(),
        b"7z\xbc\xaf\x27\x1c".as_slice(),
        b"Rar!\x1a\x07".as_slice(),
    ];
    if forbidden_suffixes
        .iter()
        .any(|suffix| lower.ends_with(suffix))
        || bytes.starts_with(b"SQLite format 3\0")
        || bytes.starts_with(b"MZ")
        || bytes.starts_with(b"\x7fELF")
        || bytes.starts_with(b"#!")
        || executable_magic
            .iter()
            .any(|magic| bytes.starts_with(magic))
        || archive_magic.iter().any(|magic| bytes.starts_with(magic))
        || bytes.get(257..262) == Some(b"ustar".as_slice())
        || {
            let prefix = bytes.get(..bytes.len().min(256)).unwrap_or(bytes);
            let lower = String::from_utf8_lossy(prefix)
                .trim_start()
                .to_ascii_lowercase();
            lower.starts_with("<!doctype html")
                || lower.starts_with("<html")
                || lower.starts_with("<svg")
                || lower.contains("<script")
        }
    {
        return Err(PortablePolicyViolation(format!(
            "SQLite, executable, or nested archive asset {name} is prohibited"
        )));
    }
    Ok(())
}

/// Applies the shared payload path, count, size, JSON, and prohibited-content
/// policy to bundle entries other than the fixed manifest and checksums files.
///
/// # Errors
///
/// Returns the first structural or safety policy violation.
pub fn validate_portable_payloads<'a, I>(
    manifest: &BundleManifest,
    limits: &PortableLimits,
    payloads: I,
) -> Result<(), PortablePolicyViolation>
where
    I: IntoIterator<Item = (&'a str, &'a [u8])>,
{
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    let mut counts = [0_u64; 6];
    let mut entry_count = 0_usize;
    let mut omitted_assets = BTreeMap::new();
    for (path, bytes) in payloads {
        let (section_index, size) = validate_portable_payload(manifest, limits, path, bytes)?;
        if let Some(name) = path.strip_prefix("assets/")
            && manifest
                .asset_metadata
                .get(name)
                .is_some_and(|metadata| metadata.media_type == OMITTED_ASSET_MEDIA_TYPE)
        {
            let placeholder = parse_omitted_asset_bytes(bytes)
                .map_err(|error| PortablePolicyViolation(error.to_string()))?;
            omitted_assets.insert(name.to_owned(), placeholder);
        }
        if !seen.insert(path.to_ascii_lowercase()) {
            return Err(PortablePolicyViolation(format!(
                "duplicate or case-colliding portable path {path}"
            )));
        }
        entry_count = entry_count
            .checked_add(1)
            .ok_or_else(|| PortablePolicyViolation("entry count overflow".to_owned()))?;
        if entry_count > limits.max_entries.saturating_sub(2) {
            return Err(PortablePolicyViolation(
                "entry count limit exceeded".to_owned(),
            ));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| PortablePolicyViolation("total size overflow".to_owned()))?;
        if total > limits.max_total_uncompressed_bytes {
            return Err(PortablePolicyViolation(
                "total uncompressed size limit exceeded".to_owned(),
            ));
        }
        counts[section_index] = counts[section_index]
            .checked_add(1)
            .ok_or_else(|| PortablePolicyViolation("section count overflow".to_owned()))?;
    }
    let expected = [
        manifest.counts.scenarios,
        manifest.counts.scenario_revisions,
        manifest.counts.results,
        manifest.counts.shared_records,
        manifest.counts.preferences,
        manifest.counts.assets,
    ];
    if counts != expected {
        return Err(PortablePolicyViolation(
            "manifest counts do not match portable payloads".to_owned(),
        ));
    }
    if manifest.bundle_kind == BundleKind::ScenarioExport && counts[0] != 1 {
        return Err(PortablePolicyViolation(
            "scenario export must contain exactly one scenario".to_owned(),
        ));
    }
    validate_omitted_asset_selection(manifest, &omitted_assets)
        .map_err(|error| PortablePolicyViolation(error.to_string()))?;
    Ok(())
}

fn validate_omitted_asset_selection(
    manifest: &BundleManifest,
    omitted_assets: &BTreeMap<String, OmittedAssetPlaceholder>,
) -> Result<(), ExportError> {
    if backup_selection_from_manifest(manifest)?.is_none() && !omitted_assets.is_empty() {
        return Err(ExportError::InvalidModel(
            "omitted-asset payloads require backup selection metadata".to_owned(),
        ));
    }
    Ok(())
}

fn validate_portable_payload(
    manifest: &BundleManifest,
    limits: &PortableLimits,
    path: &str,
    bytes: &[u8],
) -> Result<(usize, u64), PortablePolicyViolation> {
    validate_portable_path(path, limits.max_path_bytes)?;
    let size = u64::try_from(bytes.len())
        .map_err(|_| PortablePolicyViolation(format!("entry {path} size overflow")))?;
    if size > limits.max_entry_bytes {
        return Err(PortablePolicyViolation(format!(
            "entry {path} exceeds its size limit"
        )));
    }
    reject_prohibited_portable_content(path, bytes)?;
    let (section, name) = path
        .split_once('/')
        .ok_or_else(|| PortablePolicyViolation(format!("undeclared portable entry {path}")))?;
    if name.is_empty() || name.contains('/') {
        return Err(PortablePolicyViolation(format!(
            "portable entry {path} must have exactly one section segment"
        )));
    }
    let (section_index, json) = match section {
        "scenarios" => (0, true),
        "scenario-revisions" => (1, true),
        "results" => (2, true),
        "shared" => (3, true),
        "preferences" => (4, true),
        "assets" => (5, false),
        _ => {
            return Err(PortablePolicyViolation(format!(
                "undeclared portable entry {path}"
            )));
        }
    };
    if json && name.as_bytes().strip_suffix(b".json").is_none() {
        return Err(PortablePolicyViolation(format!(
            "JSON section entry {path} must use .json"
        )));
    }
    if json {
        validate_json_payload(manifest, limits, path, bytes, section, size)?;
    } else {
        let metadata = manifest.asset_metadata.get(name).ok_or_else(|| {
            PortablePolicyViolation(format!("asset {name} lacks manifest metadata"))
        })?;
        validate_asset_bytes(name, bytes, metadata)?;
    }
    Ok((section_index, size))
}

struct ProjectPresence(bool);

impl<'de> Deserialize<'de> for ProjectPresence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ProjectPresenceVisitor;

        impl<'de> Visitor<'de> for ProjectPresenceVisitor {
            type Value = ProjectPresence;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a portable scenario envelope")
            }

            fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut present = false;
                while let Some(key) = object.next_key::<String>()? {
                    if key == "project" {
                        present = true;
                    }
                    object.next_value::<IgnoredAny>()?;
                }
                Ok(ProjectPresence(present))
            }
        }

        deserializer.deserialize_map(ProjectPresenceVisitor)
    }
}

fn project_presence(path: &str, bytes: &[u8]) -> Result<bool, PortablePolicyViolation> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let presence = ProjectPresence::deserialize(&mut deserializer)
        .map_err(|error| PortablePolicyViolation(format!("invalid JSON in {path}: {error}")))?;
    deserializer
        .end()
        .map_err(|error| PortablePolicyViolation(format!("invalid JSON in {path}: {error}")))?;
    Ok(presence.0)
}

fn validate_json_payload(
    manifest: &BundleManifest,
    limits: &PortableLimits,
    path: &str,
    bytes: &[u8],
    section: &str,
    size: u64,
) -> Result<(), PortablePolicyViolation> {
    if size > limits.max_json_bytes
        || (matches!(section, "scenarios" | "scenario-revisions")
            && size > MAX_SCENARIO_DOCUMENT_BYTES)
    {
        return Err(PortablePolicyViolation(format!(
            "JSON entry {path} exceeds its byte limit"
        )));
    }
    let json_limits = PortableJsonLimits {
        max_depth: limits.max_json_depth,
        max_string_bytes: limits.max_string_bytes,
        max_collection_items: limits.max_collection_items,
    };
    validate_nonsecret_portable_json_bytes(bytes, &json_limits)
        .map_err(|error| PortablePolicyViolation(format!("invalid JSON in {path}: {error}")))?;
    let has_project = project_presence(path, bytes)?;
    match (
        manifest.schema_version == CURRENT_PORTABLE_SCHEMA_VERSION,
        manifest.bundle_kind,
        section,
        has_project,
    ) {
        (true, BundleKind::ScenarioExport, "scenarios", true) => {
            return Err(PortablePolicyViolation(
                "scenario exports must omit project wrapper metadata".to_owned(),
            ));
        }
        (true, BundleKind::FullBackup, "scenarios", false) => {
            return Err(PortablePolicyViolation(
                "full backups must include project wrapper metadata".to_owned(),
            ));
        }
        (true, _, "scenario-revisions", true) => {
            return Err(PortablePolicyViolation(
                "historical scenario revisions must omit project wrapper metadata".to_owned(),
            ));
        }
        _ => {}
    }
    Ok(())
}

fn usize_to_u64(value: usize) -> Result<u64, ExportError> {
    u64::try_from(value)
        .map_err(|_| ExportError::InvalidModel("portable item count overflow".to_owned()))
}

fn validate_manifest_model(manifest: &BundleManifest) -> Result<(), ExportError> {
    if manifest.bundle_id.as_uuid().get_version_num() != 7 {
        return Err(ExportError::InvalidModel(
            "bundle identity must be UUIDv7".to_owned(),
        ));
    }
    if !manifest.created_at.ends_with('Z') || Rfc3339Timestamp::parse(&manifest.created_at).is_err()
    {
        return Err(ExportError::InvalidModel(
            "bundle creation time must be a valid UTC RFC 3339 timestamp".to_owned(),
        ));
    }
    if manifest.application.name.is_empty()
        || manifest.application.version.is_empty()
        || manifest.title.is_empty()
    {
        return Err(ExportError::InvalidModel(
            "application metadata and bundle title must be nonempty".to_owned(),
        ));
    }
    for capability in &manifest.required_capabilities {
        validate_namespace(&capability.id)?;
        if capability.version == 0 {
            return Err(ExportError::InvalidModel(
                "semantic capability version must be positive".to_owned(),
            ));
        }
    }
    for namespace in &manifest.nonsemantic_extensions {
        validate_namespace(namespace)?;
    }
    let declared_assets = u64::try_from(manifest.asset_metadata.len())
        .map_err(|_| ExportError::InvalidModel("asset metadata count overflow".to_owned()))?;
    if declared_assets != manifest.counts.assets {
        return Err(ExportError::InvalidModel(
            "manifest asset metadata does not match asset count".to_owned(),
        ));
    }
    for (name, metadata) in &manifest.asset_metadata {
        validate_segment(name)?;
        if !metadata.redistribution_permitted {
            return Err(ExportError::InvalidModel(format!(
                "asset {name} is not permitted for redistribution"
            )));
        }
        if !matches!(
            metadata.media_type.as_str(),
            "image/png"
                | "image/jpeg"
                | "text/plain"
                | "text/plain; charset=utf-8"
                | OMITTED_ASSET_MEDIA_TYPE
        ) {
            return Err(ExportError::InvalidModel(format!(
                "asset {name} has unsupported media type"
            )));
        }
    }
    backup_selection_from_manifest(manifest)?;
    validate_safe_value(&serde_json::to_value(manifest)?, 0)
}
fn assemble(
    manifest: &BundleManifest,
    mut payloads: BTreeMap<String, Cow<'_, [u8]>>,
) -> Result<Vec<u8>, ExportError> {
    validate_manifest_model(manifest)?;
    validate_portable_payloads(
        manifest,
        &PORTABLE_LIMITS,
        payloads
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_ref())),
    )
    .map_err(|error| ExportError::InvalidModel(error.0))?;
    let manifest_bytes = canonical_json(manifest)?;
    payloads.insert(MANIFEST_PATH.to_owned(), Cow::Owned(manifest_bytes));
    let files = payloads
        .iter()
        .map(|(path, bytes)| (path.clone(), sha256_hex(bytes.as_ref())))
        .collect();
    let checksums = Checksums {
        algorithm: CHECKSUM_ALGORITHM.to_owned(),
        files,
    };
    payloads.insert(
        CHECKSUMS_PATH.to_owned(),
        Cow::Owned(canonical_json(&checksums)?),
    );

    let total = payloads.values().try_fold(0_u64, |total, bytes| {
        let size = u64::try_from(bytes.len())
            .map_err(|_| ExportError::InvalidModel("portable entry size overflow".to_owned()))?;
        total
            .checked_add(size)
            .ok_or_else(|| ExportError::InvalidModel("portable total size overflow".to_owned()))
    })?;
    if total > PORTABLE_LIMITS.max_total_uncompressed_bytes {
        return Err(ExportError::InvalidModel(
            "assembled uncompressed content exceeds limit".to_owned(),
        ));
    }
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o600);
    for (path, bytes) in payloads {
        writer.start_file(path, options)?;
        writer.write_all(bytes.as_ref())?;
    }
    let bytes = writer.finish()?.into_inner();
    if u64::try_from(bytes.len()).map_or(true, |size| size > PORTABLE_LIMITS.max_archive_bytes) {
        return Err(ExportError::InvalidModel(
            "assembled archive exceeds limit".to_owned(),
        ));
    }
    verify_assembled_bundle(&bytes)?;
    Ok(bytes)
}

/// Reopens a produced archive and verifies every checksum declaration.
///
/// # Errors
///
/// Returns an error when `bytes` is not a readable ZIP archive, its checksum
/// declaration is missing or malformed, a declared payload cannot be read, or
/// any payload checksum differs from the declaration.
pub fn verify_assembled_bundle(bytes: &[u8]) -> Result<(), ExportError> {
    if u64::try_from(bytes.len()).map_or(true, |size| size > PORTABLE_LIMITS.max_archive_bytes) {
        return Err(ExportError::Verification(
            "archive exceeds compressed-byte limit".to_owned(),
        ));
    }
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor)?;
    if archive.len() > PORTABLE_LIMITS.max_entries {
        return Err(ExportError::Verification(
            "archive entry-count limit exceeded".to_owned(),
        ));
    }
    let checksums_bytes = read_zip_entry(&mut archive, CHECKSUMS_PATH)?;
    let checksums: Checksums = serde_json::from_slice(&checksums_bytes)?;
    if checksums.algorithm != CHECKSUM_ALGORITHM {
        return Err(ExportError::Verification(
            "unexpected checksum algorithm".to_owned(),
        ));
    }
    for (path, expected) in checksums.files {
        validate_portable_path(&path, PORTABLE_LIMITS.max_path_bytes)
            .map_err(|error| ExportError::Verification(error.0))?;
        let actual = sha256_zip_entry(&mut archive, &path)?;
        if actual != expected {
            return Err(ExportError::Verification(format!(
                "checksum mismatch for {path}"
            )));
        }
    }
    Ok(())
}

fn sha256_zip_entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    path: &str,
) -> Result<String, ExportError> {
    let mut file = archive.by_name(path)?;
    if file.size() > PORTABLE_LIMITS.max_entry_bytes {
        return Err(ExportError::Verification(format!(
            "entry {path} exceeds size limit"
        )));
    }
    let declared_size = file.size();
    let mut read_size = 0_u64;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        read_size =
            read_size
                .checked_add(u64::try_from(read).map_err(|_| {
                    ExportError::Verification(format!("entry {path} size overflow"))
                })?)
                .ok_or_else(|| ExportError::Verification(format!("entry {path} size overflow")))?;
        if read_size > PORTABLE_LIMITS.max_entry_bytes {
            return Err(ExportError::Verification(format!(
                "entry {path} exceeds size limit"
            )));
        }
        hasher.update(&buffer[..read]);
    }
    if read_size != declared_size {
        return Err(ExportError::Verification(format!(
            "entry {path} size differs from declaration"
        )));
    }
    Ok(hex::encode(hasher.finalize()))
}

fn read_zip_entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    path: &str,
) -> Result<Vec<u8>, ExportError> {
    let mut file = archive.by_name(path)?;
    if file.size() > PORTABLE_LIMITS.max_entry_bytes {
        return Err(ExportError::Verification(format!(
            "entry {path} exceeds size limit"
        )));
    }
    let declared_size = file.size();
    let mut bytes = Vec::new();
    file.by_ref()
        .take(PORTABLE_LIMITS.max_entry_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()) != Ok(declared_size) {
        return Err(ExportError::Verification(format!(
            "entry {path} size differs from declaration"
        )));
    }
    Ok(bytes)
}

/// A checked, owner-private temporary output awaiting its atomic no-clobber
/// publication commit point.
pub struct PreparedPublication {
    destination: PathBuf,
    temporary: NamedTempFile,
}

impl PreparedPublication {
    /// Atomically publishes the prepared exact bytes unless operation interruption was
    /// observed before the filesystem commit point.
    ///
    /// # Errors
    ///
    /// Returns a destination, cancellation, or filesystem publication error.
    pub fn publish_controlled(self, control: &OperationControl) -> Result<(), ExportError> {
        let mut cancelled = || control.check().is_err();
        publish_prepared_with_control(self, PublicationFailpoint::None, &mut cancelled)
    }
}

/// Writes, flushes, reopens, and verifies a bundle in an owner-private
/// temporary sibling without publishing it.
///
/// # Errors
///
/// Returns an error when `bytes` fails verification, cancellation is observed,
/// or temporary-file I/O fails.
pub fn prepare_bundle_atomic_cancellable(
    destination: &Path,
    bytes: &[u8],
    cancellation: &CancellationToken,
) -> Result<PreparedPublication, ExportError> {
    let mut cancelled = || cancellation.is_cancelled();
    prepare_bundle_atomic_with_control(
        destination,
        bytes,
        PublicationFailpoint::None,
        &mut cancelled,
    )
}

/// Stages canonical, bounded, nonsecret JSON without publishing it.
///
/// This checks portable JSON safety, not domain validity or accepted-result authority;
/// the application service must supply a semantically validated typed value.
///
/// # Errors
/// Rejects prohibited or oversized JSON, cancellation and temporary-file I/O failure.
pub fn prepare_json_atomic_controlled<T: Serialize>(
    destination: &Path,
    value: &T,
    control: &OperationControl,
) -> Result<PreparedPublication, ExportError> {
    let bytes = encode_json_controlled(value, control)?;
    let mut cancelled = || control.check().is_err();
    prepare_atomic_with_control(
        destination,
        &bytes,
        PublicationFailpoint::None,
        &mut cancelled,
    )
}

/// Encodes canonical, bounded, nonsecret JSON without granting domain acceptance authority.
///
/// # Errors
/// Rejects prohibited or oversized JSON, serialization failure and operation interruption.
pub fn encode_json_controlled<T: Serialize>(
    value: &T,
    control: &OperationControl,
) -> Result<Vec<u8>, ExportError> {
    let mut cancelled = || control.check().is_err();
    check_cancelled(&mut cancelled)?;
    let value = serde_json::to_value(value)?;
    validate_safe_value(&value, 0)?;
    let bytes = canonical_json(&value)?;
    if bytes.len() as u64 > PORTABLE_LIMITS.max_json_bytes {
        return Err(ExportError::InvalidModel(
            "JSON document byte limit exceeded".to_owned(),
        ));
    }
    check_cancelled(&mut cancelled)?;
    Ok(bytes)
}

/// Stages bounded inert UTF-8 text using the same checked no-clobber publication as JSON.
///
/// This is a byte-publication boundary, not a domain, acceptance, or privacy authority.
/// The application must validate the typed source and permitted export fields before encoding.
///
/// # Errors
/// Rejects excessive text, non-text controls, interruption, or staging I/O failure.
pub fn prepare_text_atomic_controlled(
    destination: &Path,
    text: &str,
    control: &OperationControl,
) -> Result<PreparedPublication, ExportError> {
    let mut cancelled = || control.check().is_err();
    check_cancelled(&mut cancelled)?;
    if text.len() as u64 > PORTABLE_LIMITS.max_json_bytes {
        return Err(ExportError::InvalidModel(
            "text document byte limit exceeded".to_owned(),
        ));
    }
    for (index, character) in text.chars().enumerate() {
        if index % 4096 == 0 {
            check_cancelled(&mut cancelled)?;
        }
        if character.is_control() && !matches!(character, '\t' | '\r' | '\n') {
            return Err(ExportError::InvalidModel(
                "text document contains non-text controls".to_owned(),
            ));
        }
    }
    prepare_atomic_with_control(
        destination,
        text.as_bytes(),
        PublicationFailpoint::None,
        &mut cancelled,
    )
}

/// Write to a random owner-private temporary sibling, flush and close it,
/// reopen and hash the exact bytes, then publish with an atomic no-clobber
/// primitive. Existing destinations are never overwritten.
///
/// # Errors
///
/// Returns an error when the destination exists, `bytes` fails verification,
/// temporary-file I/O or verification fails, or publication fails.
pub fn write_bundle_atomic(destination: &Path, bytes: &[u8]) -> Result<(), ExportError> {
    write_bundle_atomic_with_control(destination, bytes, PublicationFailpoint::None, || false)
}

/// Writes and atomically publishes a bundle unless cancellation is observed
/// before the no-clobber publication commit point.
///
/// # Errors
///
/// Returns [`ExportError::Cancelled`] when cancellation is observed before
/// publication, otherwise the same errors as [`write_bundle_atomic`].
pub fn write_bundle_atomic_cancellable(
    destination: &Path,
    bytes: &[u8],
    cancellation: &CancellationToken,
) -> Result<(), ExportError> {
    write_bundle_atomic_with_control(destination, bytes, PublicationFailpoint::None, || {
        cancellation.is_cancelled()
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicationFailpoint {
    None,
    #[cfg(test)]
    BeforePublish,
    #[cfg(test)]
    AfterPublish,
}

fn check_cancelled<F>(cancelled: &mut F) -> Result<(), ExportError>
where
    F: FnMut() -> bool,
{
    if cancelled() {
        Err(ExportError::Cancelled)
    } else {
        Ok(())
    }
}

fn prepare_bundle_atomic_with_control<F>(
    destination: &Path,
    bytes: &[u8],
    failpoint: PublicationFailpoint,
    cancelled: &mut F,
) -> Result<PreparedPublication, ExportError>
where
    F: FnMut() -> bool,
{
    check_cancelled(cancelled)?;
    verify_assembled_bundle(bytes)?;
    prepare_atomic_with_control(destination, bytes, failpoint, cancelled)
}

fn prepare_atomic_with_control<F>(
    destination: &Path,
    bytes: &[u8],
    failpoint: PublicationFailpoint,
    cancelled: &mut F,
) -> Result<PreparedPublication, ExportError>
where
    F: FnMut() -> bool,
{
    const WRITE_CHUNK_BYTES: usize = 64 * 1024;

    #[cfg(not(test))]
    let _ = failpoint;
    check_cancelled(cancelled)?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = TempFileBuilder::new()
        .prefix(".eutheto-publication-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    for chunk in bytes.chunks(WRITE_CHUNK_BYTES) {
        check_cancelled(cancelled)?;
        temporary.write_all(chunk)?;
    }
    check_cancelled(cancelled)?;
    temporary.flush()?;
    check_cancelled(cancelled)?;
    temporary.as_file().sync_all()?;

    let mut reopened = File::open(temporary.path())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; WRITE_CHUNK_BYTES].into_boxed_slice();
    loop {
        check_cancelled(cancelled)?;
        let read = reopened.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual: [u8; 32] = hasher.finalize().into();
    let expected: [u8; 32] = Sha256::digest(bytes).into();
    if actual != expected {
        return Err(ExportError::Verification(
            "temporary output differs after reopen".to_owned(),
        ));
    }
    drop(reopened);

    #[cfg(test)]
    if failpoint == PublicationFailpoint::BeforePublish {
        return Err(ExportError::Io(std::io::Error::other(
            "injected pre-publication failure",
        )));
    }

    Ok(PreparedPublication {
        destination: destination.to_path_buf(),
        temporary,
    })
}

fn publish_prepared_with_control<F>(
    prepared: PreparedPublication,
    failpoint: PublicationFailpoint,
    cancelled: &mut F,
) -> Result<(), ExportError>
where
    F: FnMut() -> bool,
{
    #[cfg(not(test))]
    let _ = failpoint;
    check_cancelled(cancelled)?;
    let parent = prepared
        .destination
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let published = match prepared.temporary.persist_noclobber(&prepared.destination) {
        Ok(published) => published,
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ExportError::DestinationExists(prepared.destination));
        }
        Err(error) => return Err(ExportError::Io(error.error)),
    };

    // Publication is the commit point: cancellation after this cannot turn a
    // successfully published output into an ambiguous failure.
    drop(published);
    #[cfg(test)]
    if failpoint == PublicationFailpoint::AfterPublish {
        return Ok(());
    }
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn write_bundle_atomic_with_control<F>(
    destination: &Path,
    bytes: &[u8],
    failpoint: PublicationFailpoint,
    mut cancelled: F,
) -> Result<(), ExportError>
where
    F: FnMut() -> bool,
{
    let prepared =
        prepare_bundle_atomic_with_control(destination, bytes, failpoint, &mut cancelled)?;
    publish_prepared_with_control(prepared, failpoint, &mut cancelled)
}

#[cfg(test)]
fn write_bundle_atomic_with_failpoint(
    destination: &Path,
    bytes: &[u8],
    failpoint: PublicationFailpoint,
) -> Result<(), ExportError> {
    write_bundle_atomic_with_control(destination, bytes, failpoint, || false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario_from_json(extension: Value) -> Result<ScenarioSnapshotV1, serde_json::Error> {
        let document: ScenarioDocument = serde_json::from_value(serde_json::json!({
            "format": "eutheto/scenario",
            "formatVersion": 1,
            "scenarioId": "018f1e2d-3c4b-7a69-8def-0123456789ab",
            "domainPack": {"id": "official.test", "schemaVersion": 1},
            "metadata": {
                "title": "Portable",
                "description": "",
                "createdAt": "2026-08-29T00:00:00Z",
                "updatedAt": "2026-08-29T00:00:00Z"
            },
            "settings": {
                "timeZone": "Etc/UTC",
                "locale": "en-US",
                "units": "metric",
                "horizon": {"start": "2026-08-29T00:00:00Z", "end": "2026-08-30T00:00:00Z"},
                "gapPolicy": "reject",
                "overlapPolicy": "earlier"
            },
            "domain": {"entities": {}, "rules": {}, "preferences": {}, "lockedAssignments": {}},
            "extensions": {}
        }))?;
        let mut scenario = ScenarioSnapshotV1::current(Revision::new(7), document, BTreeSet::new());
        scenario
            .extensions
            .insert("example.visual".to_owned(), extension);
        Ok(scenario)
    }

    fn self_declared(identity: Uuid) -> Value {
        Value::Object(serde_json::Map::from_iter([(
            identity.to_string(),
            serde_json::json!({"id": identity}),
        )]))
    }

    #[test]
    fn owned_uuid_uniqueness_rejects_root_reused_as_entity()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = scenario_from_json(Value::Null)?;
        let root = scenario.document.scenario_id.as_uuid();
        scenario
            .document
            .domain
            .entities
            .insert(root.to_string().parse()?, Value::Null);
        assert!(validate_scenario_owned_uuid_uniqueness(&scenario).is_err());
        Ok(())
    }

    #[test]
    fn owned_uuid_uniqueness_rejects_entity_reused_as_rule()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = scenario_from_json(Value::Null)?;
        let identity = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000001")?;
        scenario
            .document
            .domain
            .entities
            .insert(identity.to_string().parse()?, Value::Null);
        scenario
            .document
            .domain
            .rules
            .insert(identity.to_string().parse()?, Value::Null);
        assert!(validate_scenario_owned_uuid_uniqueness(&scenario).is_err());
        Ok(())
    }

    #[test]
    fn owned_uuid_uniqueness_rejects_typed_identity_reused_as_nested_definition()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = scenario_from_json(Value::Null)?;
        let identity = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000002")?;
        scenario
            .document
            .domain
            .entities
            .insert(identity.to_string().parse()?, Value::Null);
        scenario
            .extensions
            .insert("example.owned".to_owned(), self_declared(identity));
        assert!(validate_scenario_owned_uuid_uniqueness(&scenario).is_err());
        Ok(())
    }

    #[test]
    fn owned_uuid_uniqueness_rejects_duplicate_nested_definitions()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = scenario_from_json(Value::Null)?;
        let identity = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000003")?;
        scenario
            .document
            .extensions
            .insert("example.first".to_owned(), self_declared(identity));
        scenario
            .extensions
            .insert("example.second".to_owned(), self_declared(identity));
        assert!(validate_scenario_owned_uuid_uniqueness(&scenario).is_err());
        Ok(())
    }
    fn snapshot(scenario: ScenarioSnapshotV1) -> ScenarioExportSnapshot {
        ScenarioExportSnapshot {
            bundle_id: BundleId::from_uuid(Uuid::from_u128(
                0x018f_1e2d_3c4b_7a69_8def_1123_4567_89ab,
            )),
            created_at: "2026-08-29T00:00:00Z".to_owned(),
            application: ApplicationMetadata {
                name: "Eutheto".to_owned(),
                version: "0.1.0".to_owned(),
            },
            title: "Portable".to_owned(),
            scenario,
            scenario_revisions: Vec::new(),
            sections: BackupSections::default(),
            nonsemantic_extensions: BTreeSet::new(),
            manifest_extensions: BTreeMap::new(),
        }
    }

    fn full_backup(scenario: ScenarioSnapshotV1) -> FullBackupSnapshot {
        FullBackupSnapshot {
            bundle_id: BundleId::from_uuid(Uuid::from_u128(
                0x018f_1e2d_3c4b_7a69_8def_1123_4567_89ab,
            )),
            created_at: "2026-08-29T00:00:00Z".to_owned(),
            application: ApplicationMetadata {
                name: "Eutheto".to_owned(),
                version: "0.1.0".to_owned(),
            },
            title: "Portable".to_owned(),
            scenarios: vec![scenario],
            scenario_revisions: Vec::new(),
            sections: BackupSections::default(),
            nonsemantic_extensions: BTreeSet::new(),
            manifest_extensions: BTreeMap::from([(
                BACKUP_SELECTION_EXTENSION.to_owned(),
                serde_json::json!({
                    "includeResults": true,
                    "assetSelection": "all",
                    "excludedAssetCount": 0,
                    "excludedAssetIds": [],
                    "fixedExclusions": [
                        "local-undo-and-audit-history",
                        "sqlite-and-database-internals",
                        "credentials-tokens-and-keychain-references",
                        "device-local-paths-and-window-state",
                        "logs-caches-and-temporary-data",
                        "redistribution-prohibited-provider-data",
                        "executable-content"
                    ],
                    "scope": "library"
                }),
            )]),
        }
    }

    fn assert_invalid_model<T>(
        result: Result<T, ExportError>,
        expected: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match result {
            Err(ExportError::InvalidModel(message)) if message.contains(expected) => Ok(()),
            Err(ExportError::InvalidModel(message)) => Err(std::io::Error::other(format!(
                "expected {expected:?} in validation error, got {message:?}"
            ))
            .into()),
            Err(error) => Err(std::io::Error::other(format!(
                "expected invalid portable model, got {error}"
            ))
            .into()),
            Ok(_) => Err(std::io::Error::other("expected invalid portable model").into()),
        }
    }

    #[test]
    fn export_is_deterministic_and_canonically_ordered() -> Result<(), Box<dyn std::error::Error>> {
        let first = assemble_scenario_export(
            &snapshot(scenario_from_json(serde_json::json!({
                "zoom": 3,
                "palette": "high-contrast"
            }))?),
            &portable_encode::encode_fixture_domain,
        )?;
        let second = assemble_scenario_export(
            &snapshot(scenario_from_json(serde_json::json!({
                "palette": "high-contrast",
                "zoom": 3
            }))?),
            &portable_encode::encode_fixture_domain,
        )?;
        assert_eq!(first, second);
        verify_assembled_bundle(&first)?;

        let mut backup = full_backup(scenario_from_json(Value::Null)?);
        backup.sections.results.insert(
            "018f1e2d-3c4b-7a69-8def-012345678911".to_owned(),
            serde_json::json!({
                "resultId": "018f1e2d-3c4b-7a69-8def-012345678911",
                "scenarioId": "018f1e2d-3c4b-7a69-8def-0123456789ab",
                "scenarioRevision": 7
            }),
        );
        backup.sections.results.insert(
            "018f1e2d-3c4b-7a69-8def-012345678910".to_owned(),
            serde_json::json!({
                "resultId": "018f1e2d-3c4b-7a69-8def-012345678910",
                "scenarioId": "018f1e2d-3c4b-7a69-8def-0123456789ab",
                "scenarioRevision": 7
            }),
        );
        backup.sections.assets.insert(
            "zeta.txt".to_owned(),
            PortableAsset {
                bytes: b"zeta".to_vec(),
                media_type: "text/plain; charset=utf-8".to_owned(),
                redistribution_permitted: true,
            },
        );
        backup.sections.assets.insert(
            "alpha.txt".to_owned(),
            PortableAsset {
                bytes: b"alpha".to_vec(),
                media_type: "text/plain; charset=utf-8".to_owned(),
                redistribution_permitted: true,
            },
        );
        let ordered = assemble_full_backup(&backup, &portable_encode::encode_fixture_domain)?;
        let mut archive = ZipArchive::new(Cursor::new(ordered))?;
        let names = (0..archive.len())
            .map(|index| archive.by_index(index).map(|entry| entry.name().to_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            names,
            [
                "assets/alpha.txt",
                "assets/zeta.txt",
                CHECKSUMS_PATH,
                MANIFEST_PATH,
                "results/018f1e2d-3c4b-7a69-8def-012345678910.json",
                "results/018f1e2d-3c4b-7a69-8def-012345678911.json",
                "scenarios/018f1e2d-3c4b-7a69-8def-0123456789ab.json",
            ]
        );
        Ok(())
    }

    #[test]
    fn export_is_current_only() -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = scenario_from_json(Value::Null)?;
        scenario.schema_version = CURRENT_PORTABLE_SCHEMA_VERSION + 1;
        let error =
            assemble_scenario_export(&snapshot(scenario), &portable_encode::encode_fixture_domain);
        assert!(matches!(error, Err(ExportError::InvalidModel(_))));
        Ok(())
    }

    #[test]
    fn prohibited_json_fields_and_device_paths_are_structurally_rejected()
    -> Result<(), Box<dyn std::error::Error>> {
        let prohibited_fields = [
            (
                serde_json::json!({"credential": "secret"}),
                "prohibited field",
            ),
            (
                serde_json::json!({"oauthToken": "secret"}),
                "prohibited field",
            ),
            (serde_json::json!({"apiKey": "secret"}), "prohibited field"),
            (
                serde_json::json!({"providerApiKey": "secret"}),
                "prohibited field",
            ),
            (
                serde_json::json!({"providerClientSecret": "secret"}),
                "prohibited field",
            ),
            (
                serde_json::json!({"oauthCredentialId": "credential-id"}),
                "prohibited field",
            ),
            (
                serde_json::json!({"api-key-handle": "credential-handle"}),
                "prohibited field",
            ),
            (
                serde_json::json!({"databasePassword": "secret"}),
                "prohibited field",
            ),
            (
                serde_json::json!({"sqliteDatabase": "encoded"}),
                "prohibited field",
            ),
            (
                serde_json::json!({"providerAuthentication": {"value": "secret"}}),
                "prohibited field",
            ),
            (
                serde_json::json!({"auth": {"bearer": "secret"}}),
                "prohibited field",
            ),
            (
                serde_json::json!({"connectionString": "provider://credential"}),
                "prohibited field",
            ),
            (
                serde_json::json!({"authenticationReference": "credential-ref"}),
                "prohibited field",
            ),
            (
                serde_json::json!({"redistributionPermitted": false}),
                "provider-restricted",
            ),
        ];
        for (value, expected) in prohibited_fields {
            assert_invalid_model(
                assemble_scenario_export(
                    &snapshot(scenario_from_json(value)?),
                    &portable_encode::encode_fixture_domain,
                ),
                expected,
            )?;
        }
        let harmless = serde_json::json!({
            "authenticationStatus": "connected",
            "authenticationLabel": "Provider",
            "authenticationMethod": "browser",
            "grid": "grid",
            "tokenizer": "word-boundary",
            "monkey": "semantic label"
        });
        let _ = assemble_scenario_export(
            &snapshot(scenario_from_json(harmless)?),
            &portable_encode::encode_fixture_domain,
        )?;

        for path in [
            "/dev/disk0",
            "file:///home/user/private",
            r"C:\Users\user\private",
            "C:/Users/user/private",
            r"\\server\share\private",
        ] {
            assert_invalid_model(
                assemble_scenario_export(
                    &snapshot(scenario_from_json(Value::String(path.to_owned()))?),
                    &portable_encode::encode_fixture_domain,
                ),
                "filesystem path",
            )?;
        }
        Ok(())
    }
    #[test]
    fn scenario_exports_include_selected_sections_and_require_exact_result_revision()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut selected = snapshot(scenario_from_json(Value::Null)?);
        selected.sections.results.insert(
            "018f1e2d-3c4b-7a69-8def-012345678920".to_owned(),
            serde_json::json!({
                "resultId": "018f1e2d-3c4b-7a69-8def-012345678920",
                "scenarioId": "018f1e2d-3c4b-7a69-8def-0123456789ab",
                "scenarioRevision": 7,
                "payload": {"opaque": true}
            }),
        );
        selected
            .sections
            .shared_records
            .insert("shared".to_owned(), serde_json::json!({"safe": true}));
        selected.sections.assets.insert(
            "notes.txt".to_owned(),
            PortableAsset {
                bytes: b"portable notes".to_vec(),
                media_type: "text/plain; charset=utf-8".to_owned(),
                redistribution_permitted: true,
            },
        );
        let bytes = assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain)?;
        let mut archive = ZipArchive::new(Cursor::new(bytes))?;
        assert!(
            archive
                .by_name("results/018f1e2d-3c4b-7a69-8def-012345678920.json")
                .is_ok()
        );
        assert!(archive.by_name("shared/shared.json").is_ok());
        assert!(archive.by_name("assets/notes.txt").is_ok());

        let selected_result = selected
            .sections
            .results
            .get_mut("018f1e2d-3c4b-7a69-8def-012345678920")
            .ok_or_else(|| std::io::Error::other("missing selected result fixture"))?;
        selected_result["scenarioRevision"] = Value::from(6);
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "exact revision 6",
        )?;
        let mut historical = selected.scenario.clone();
        historical.revision = Revision::new(6);
        historical.project = None;
        selected.scenario_revisions.push(historical);
        let historical_bytes =
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain)?;
        verify_assembled_bundle(&historical_bytes)?;
        selected.scenario_revisions[0].revision = Revision::new(7);
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "must be less than current revision 7",
        )?;
        selected.scenario_revisions[0].revision = Revision::new(8);
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "must be less than current revision 7",
        )?;
        Ok(())
    }

    #[test]
    fn retained_result_key_requires_matching_typed_uuid() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut selected = snapshot(scenario_from_json(Value::Null)?);
        let scenario_id = selected.scenario.document.scenario_id;
        let key = "018f1e2d-3c4b-7a69-8def-012345678940";
        selected.sections.results.insert(
            key.to_owned(),
            serde_json::json!({
                "scenarioId": scenario_id,
                "scenarioRevision": 7
            }),
        );
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "missing resultId",
        )?;
        selected
            .sections
            .results
            .get_mut(key)
            .ok_or_else(|| std::io::Error::other("missing retained result identity fixture"))?["resultId"] =
            Value::String("018f1e2d-3c4b-7a69-8def-012345678941".to_owned());
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "does not match resultId",
        )?;
        selected.sections.results.clear();
        selected.sections.results.insert(
            scenario_id.to_string(),
            serde_json::json!({
                "resultId": scenario_id,
                "scenarioId": scenario_id,
                "scenarioRevision": 7
            }),
        );
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "assigned to more than one portable record",
        )?;
        Ok(())
    }

    #[test]
    fn export_rejects_uuid_ownership_shared_across_supplemental_sections()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut selected = snapshot(scenario_from_json(Value::Null)?);
        let scenario_id = selected.scenario.document.scenario_id.to_string();
        selected
            .sections
            .shared_records
            .insert(scenario_id, serde_json::json!({"safe": true}));
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "assigned to more than one portable record",
        )?;

        let mut selected = snapshot(scenario_from_json(Value::Null)?);
        let supplemental_id = "018f1e2d-3c4b-7a69-8def-012345678942";
        selected.sections.shared_records.insert(
            supplemental_id.to_owned(),
            serde_json::json!({"safe": true}),
        );
        selected.sections.assets.insert(
            format!("{supplemental_id}.txt"),
            PortableAsset {
                bytes: b"synthetic inert asset".to_vec(),
                media_type: "text/plain; charset=utf-8".to_owned(),
                redistribution_permitted: true,
            },
        );
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "assigned to more than one portable record",
        )
    }

    #[test]
    fn backup_rejects_owned_identity_shared_by_scenario_families()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut first = scenario_from_json(Value::Null)?;
        let shared_id: eutheto_types::EntityId = "018f1e2d-3c4b-7a69-8def-012345678940".parse()?;
        first
            .document
            .domain
            .entities
            .insert(shared_id, serde_json::json!({"id": shared_id}));
        let mut second = first.clone();
        second.document.scenario_id =
            ScenarioId::from_uuid(Uuid::parse_str("018f1e2d-3c4b-7a69-8def-0123456789ac")?);
        let mut backup = full_backup(first);
        backup.scenarios.push(second);
        assert_invalid_model(
            assemble_full_backup(&backup, &portable_encode::encode_fixture_domain),
            "declared by scenario families",
        )?;
        Ok(())
    }

    #[test]
    fn scenario_export_rejects_unrepresented_scenario_dependencies()
    -> Result<(), Box<dyn std::error::Error>> {
        let dependency =
            ScenarioId::from_uuid(Uuid::parse_str("018f1e2d-3c4b-7a69-8def-0123456789ac")?);
        let selected = snapshot(scenario_from_json(serde_json::json!({
            "scenarioId": dependency
        }))?);
        assert!(matches!(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            Err(ExportError::MissingScenarioDependency(found)) if found == dependency
        ));
        Ok(())
    }

    #[test]
    fn omitted_asset_placeholder_preserves_reference_closure_and_round_trips()
    -> Result<(), Box<dyn std::error::Error>> {
        let original = PortableAsset {
            bytes: b"portable notes".to_vec(),
            media_type: "text/plain; charset=utf-8".to_owned(),
            redistribution_permitted: true,
        };
        let placeholder = omitted_asset_placeholder(&original, OmittedAssetReason::ExcludeAll)?;
        let decoded = parse_omitted_asset_placeholder(&placeholder)?
            .ok_or_else(|| std::io::Error::other("placeholder was not recognized"))?;
        assert_eq!(decoded.original_media_type, original.media_type);
        assert_eq!(decoded.original_size, original.bytes.len() as u64);
        assert_eq!(decoded.content_sha256, sha256_hex(&original.bytes));

        let mut selected = snapshot(scenario_from_json(serde_json::json!({
            "asset": "notes.txt"
        }))?);
        selected
            .sections
            .assets
            .insert("notes.txt".to_owned(), placeholder);
        let selection = BackupSelection {
            include_results: false,
            asset_selection: PortableBackupAssetSelection::ExcludeAll,
            threshold_version: None,
            threshold_bytes: None,
            excluded_asset_count: 1,
            excluded_asset_ids: BTreeSet::from(["notes.txt".to_owned()]),
            fixed_exclusions: BTreeSet::new(),
            scope: BackupSelectionScope::Scenario,
        };
        selected.manifest_extensions.insert(
            BACKUP_SELECTION_EXTENSION.to_owned(),
            backup_selection_extension_value(&selection)?,
        );
        let bytes = assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain)?;
        verify_assembled_bundle(&bytes)?;
        let mut archive = ZipArchive::new(Cursor::new(bytes))?;
        let manifest: BundleManifest = serde_json::from_reader(archive.by_name(MANIFEST_PATH)?)?;
        assert_eq!(
            backup_selection_from_manifest(&manifest)?,
            Some(selection.clone())
        );
        let mut placeholder_bytes = Vec::new();
        archive
            .by_name("assets/notes.txt")?
            .read_to_end(&mut placeholder_bytes)?;
        let restored = PortableAsset {
            bytes: placeholder_bytes,
            media_type: manifest.asset_metadata["notes.txt"].media_type.clone(),
            redistribution_permitted: true,
        };
        assert_eq!(parse_omitted_asset_placeholder(&restored)?, Some(decoded));

        let inherited_selection = BackupSelection {
            include_results: false,
            asset_selection: PortableBackupAssetSelection::All,
            threshold_version: None,
            threshold_bytes: None,
            excluded_asset_count: 1,
            excluded_asset_ids: BTreeSet::from(["notes.txt".to_owned()]),
            fixed_exclusions: BTreeSet::new(),
            scope: BackupSelectionScope::Scenario,
        };
        selected.manifest_extensions.insert(
            BACKUP_SELECTION_EXTENSION.to_owned(),
            backup_selection_extension_value(&inherited_selection)?,
        );
        let inherited =
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain)?;
        verify_assembled_bundle(&inherited)?;
        Ok(())
    }

    #[test]
    fn full_backup_fixed_exclusions_round_trip_and_require_exact_known_set()
    -> Result<(), Box<dyn std::error::Error>> {
        let selection = BackupSelection {
            include_results: true,
            asset_selection: PortableBackupAssetSelection::All,
            threshold_version: None,
            threshold_bytes: None,
            excluded_asset_count: 0,
            excluded_asset_ids: BTreeSet::new(),
            fixed_exclusions: FixedExclusion::ALL.into_iter().collect(),
            scope: BackupSelectionScope::Library,
        };
        let mut backup = full_backup(scenario_from_json(Value::Null)?);
        backup.manifest_extensions.insert(
            BACKUP_SELECTION_EXTENSION.to_owned(),
            backup_selection_extension_value(&selection)?,
        );
        let bytes = assemble_full_backup(&backup, &portable_encode::encode_fixture_domain)?;
        let mut archive = ZipArchive::new(Cursor::new(bytes))?;
        let mut manifest: BundleManifest =
            serde_json::from_reader(archive.by_name(MANIFEST_PATH)?)?;
        assert_eq!(
            backup_selection_from_manifest(&manifest)?,
            Some(selection.clone())
        );
        let mut absent = manifest.clone();
        absent.extensions.remove(BACKUP_SELECTION_EXTENSION);
        assert!(backup_selection_from_manifest(&absent).is_err());

        manifest
            .extensions
            .get_mut(BACKUP_SELECTION_EXTENSION)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| std::io::Error::other("selection extension missing"))?
            .remove("fixedExclusions");
        assert!(backup_selection_from_manifest(&manifest).is_err());

        let mut unknown = serde_json::to_value(&selection)?;
        unknown["fixedExclusions"] = serde_json::json!(["future-unknown-exclusion"]);
        manifest
            .extensions
            .insert(BACKUP_SELECTION_EXTENSION.to_owned(), unknown);
        assert!(backup_selection_from_manifest(&manifest).is_err());
        Ok(())
    }

    #[test]
    fn malformed_or_inconsistent_omission_metadata_is_rejected()
    -> Result<(), Box<dyn std::error::Error>> {
        let original = PortableAsset {
            bytes: b"portable notes".to_vec(),
            media_type: "text/plain".to_owned(),
            redistribution_permitted: true,
        };
        let mut selected = snapshot(scenario_from_json(Value::Null)?);
        selected.sections.assets.insert(
            "notes.txt".to_owned(),
            omitted_asset_placeholder(&original, OmittedAssetReason::ExcludeAll)?,
        );
        selected.manifest_extensions.insert(
            BACKUP_SELECTION_EXTENSION.to_owned(),
            serde_json::json!({
                "includeResults": true,
                "assetSelection": "exclude-all",
                "excludedAssetCount": 0,
                "excludedAssetIds": ["notes.txt"],
                "scope": "scenario"
            }),
        );
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "excluded asset count",
        )?;

        let current_placeholder = selected
            .sections
            .assets
            .get("notes.txt")
            .ok_or_else(|| std::io::Error::other("missing placeholder fixture"))?;
        let mut invalid_threshold = parse_omitted_asset_placeholder(current_placeholder)?
            .ok_or_else(|| std::io::Error::other("placeholder fixture was not recognized"))?;
        invalid_threshold.reason = OmittedAssetReason::AboveV1Threshold;
        selected.sections.assets.insert(
            "notes.txt".to_owned(),
            PortableAsset {
                bytes: canonical_json(&invalid_threshold)?,
                media_type: OMITTED_ASSET_MEDIA_TYPE.to_owned(),
                redistribution_permitted: true,
            },
        );
        selected.manifest_extensions.insert(
            BACKUP_SELECTION_EXTENSION.to_owned(),
            backup_selection_extension_value(&BackupSelection {
                include_results: true,
                asset_selection: PortableBackupAssetSelection::V1Threshold,
                threshold_version: Some(1),
                threshold_bytes: Some(PORTABLE_LARGE_ASSET_BYTES_V1 as u64),
                excluded_asset_count: 1,
                excluded_asset_ids: BTreeSet::from(["notes.txt".to_owned()]),
                fixed_exclusions: BTreeSet::new(),
                scope: BackupSelectionScope::Scenario,
            })?,
        );
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "does not exceed the version-1 threshold",
        )?;
        selected.sections.assets.insert(
            "notes.txt".to_owned(),
            omitted_asset_placeholder(&original, OmittedAssetReason::ExcludeAll)?,
        );

        selected
            .sections
            .assets
            .get_mut("notes.txt")
            .ok_or_else(|| std::io::Error::other("missing placeholder fixture"))?
            .bytes
            .push(b'\n');
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "exact canonical JSON",
        )
    }

    #[test]
    fn export_rejects_dangling_declared_asset_references() -> Result<(), Box<dyn std::error::Error>>
    {
        let selected = snapshot(scenario_from_json(serde_json::json!({
            "asset": "missing.txt"
        }))?);
        assert_invalid_model(
            assemble_scenario_export(&selected, &portable_encode::encode_fixture_domain),
            "portable selection omits referenced asset missing.txt",
        )
    }

    #[test]
    fn prohibited_assets_are_structurally_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let scenario = scenario_from_json(Value::Null)?;
        let prohibited_assets = [
            (
                "restricted.txt",
                b"provider data".as_slice(),
                false,
                "not permitted for redistribution",
            ),
            (
                "database.bin",
                b"SQLite format 3\0payload".as_slice(),
                true,
                "SQLite, executable, or nested archive",
            ),
            (
                "database.sqlite",
                b"not a database".as_slice(),
                true,
                "SQLite, executable, or nested archive",
            ),
            (
                "program.bin",
                b"\x7fELFpayload".as_slice(),
                true,
                "SQLite, executable, or nested archive",
            ),
            (
                "program.wasm",
                b"renamed executable".as_slice(),
                true,
                "SQLite, executable, or nested archive",
            ),
            (
                "nested.bin",
                b"\x1f\x8bpayload".as_slice(),
                true,
                "SQLite, executable, or nested archive",
            ),
            (
                "nested.tar",
                b"renamed archive".as_slice(),
                true,
                "SQLite, executable, or nested archive",
            ),
            (
                "widget.svg",
                b"<svg><script>alert(1)</script></svg>".as_slice(),
                true,
                "SQLite, executable, or nested archive",
            ),
            (
                "report.html",
                b"<!doctype html><script>alert(1)</script>".as_slice(),
                true,
                "SQLite, executable, or nested archive",
            ),
        ];
        for (name, bytes, redistribution_permitted, expected) in prohibited_assets {
            let mut backup = full_backup(scenario.clone());
            backup.sections.assets.insert(
                name.to_owned(),
                PortableAsset {
                    bytes: bytes.to_vec(),
                    media_type: "text/plain".to_owned(),
                    redistribution_permitted,
                },
            );
            assert_invalid_model(
                assemble_full_backup(&backup, &portable_encode::encode_fixture_domain),
                expected,
            )?;
        }
        Ok(())
    }

    #[test]
    fn project_wrapper_is_kind_specific() -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = scenario_from_json(Value::Null)?;
        scenario.project = Some(PortableProjectMetadata {
            archived_at: Some(Rfc3339Timestamp::parse("2026-08-28T00:00:00Z")?),
        });
        let scenario_bundle = assemble_scenario_export(
            &snapshot(scenario.clone()),
            &portable_encode::encode_fixture_domain,
        )?;
        let mut archive = ZipArchive::new(Cursor::new(scenario_bundle))?;
        let path = format!("scenarios/{}.json", scenario.document.scenario_id);
        let scenario_value: Value = serde_json::from_reader(archive.by_name(&path)?)?;
        assert!(scenario_value.get("project").is_none());

        scenario.project = None;
        let backup_bundle = assemble_full_backup(
            &full_backup(scenario.clone()),
            &portable_encode::encode_fixture_domain,
        )?;
        let mut archive = ZipArchive::new(Cursor::new(backup_bundle))?;
        let backup_value: Value = serde_json::from_reader(archive.by_name(&path)?)?;
        assert_eq!(backup_value["project"], serde_json::json!({}));
        Ok(())
    }

    #[test]
    fn internal_paths_reject_unicode_and_windows_aliases() {
        for path in [
            "assets/résumé.txt",
            "assets/CON.txt",
            "assets/com1",
            "assets/report.txt:stream",
            "assets/trailing.",
            "assets/trailing ",
            r"assets\file.txt",
            "C:/file.txt",
        ] {
            assert!(validate_portable_path(path, PORTABLE_LIMITS.max_path_bytes).is_err());
        }
    }

    #[test]
    fn full_backup_represents_exact_historical_result_revision()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut current = scenario_from_json(Value::Null)?;
        current.revision = Revision::new(8);
        let mut historical = current.clone();
        historical.revision = Revision::new(7);
        historical.project = None;
        let scenario_id = current.document.scenario_id;
        let mut backup = full_backup(current);
        backup.scenario_revisions.push(historical);
        backup.sections.results.insert(
            "018f1e2d-3c4b-7a69-8def-012345678930".to_owned(),
            serde_json::json!({
                "resultId": "018f1e2d-3c4b-7a69-8def-012345678930",
                "scenarioId": scenario_id,
                "scenarioRevision": 7,
                "payload": {}
            }),
        );
        let bytes = assemble_full_backup(&backup, &portable_encode::encode_fixture_domain)?;
        verify_assembled_bundle(&bytes)?;
        let mut archive = ZipArchive::new(Cursor::new(bytes))?;
        assert!(
            archive
                .by_name(&format!("scenario-revisions/{scenario_id}-7.json"))
                .is_ok()
        );
        Ok(())
    }

    #[test]
    fn full_stream_image_decoding_rejects_corruption_and_polyglots()
    -> Result<(), Box<dyn std::error::Error>> {
        let image = image::DynamicImage::new_rgb8(1, 1);
        for (name, media_type, format) in [
            ("pixel.png", "image/png", ImageFormat::Png),
            ("pixel.jpg", "image/jpeg", ImageFormat::Jpeg),
        ] {
            let mut cursor = Cursor::new(Vec::new());
            image.write_to(&mut cursor, format)?;
            let valid = cursor.into_inner();
            validate_portable_asset(
                name,
                &PortableAsset {
                    bytes: valid.clone(),
                    media_type: media_type.to_owned(),
                    redistribution_permitted: true,
                },
            )?;

            let mut trailing = valid.clone();
            trailing.extend_from_slice(b"polyglot");
            assert!(
                validate_portable_asset(
                    name,
                    &PortableAsset {
                        bytes: trailing,
                        media_type: media_type.to_owned(),
                        redistribution_permitted: true,
                    },
                )
                .is_err()
            );

            let mut corrupt = valid;
            corrupt.truncate(corrupt.len().saturating_sub(1));
            assert!(
                validate_portable_asset(
                    name,
                    &PortableAsset {
                        bytes: corrupt,
                        media_type: media_type.to_owned(),
                        redistribution_permitted: true,
                    },
                )
                .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn text_publication_rejects_controls_and_honors_cancellation_before_commit()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("assignments.csv");
        let cancellation = CancellationToken::new();
        let control = OperationControl::Cancellation(cancellation.clone());
        assert!(matches!(
            prepare_text_atomic_controlled(&destination, "value,\u{1b}[31mred\n", &control),
            Err(ExportError::InvalidModel(_))
        ));
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);

        let text = "header,other\r\nrésumé,\tvalue\n";
        let staged = prepare_text_atomic_controlled(&destination, text, &control)?;
        assert!(!destination.exists());
        cancellation.cancel();
        assert!(matches!(
            staged.publish_controlled(&control),
            Err(ExportError::Cancelled)
        ));
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);

        let live = OperationControl::Cancellation(CancellationToken::new());
        prepare_text_atomic_controlled(&destination, text, &live)?.publish_controlled(&live)?;
        assert_eq!(std::fs::read(&destination)?, text.as_bytes());
        assert!(matches!(
            prepare_text_atomic_controlled(&destination, "replacement", &live)
                .and_then(|prepared| prepared.publish_controlled(&live)),
            Err(ExportError::DestinationExists(path)) if path == destination
        ));
        assert_eq!(std::fs::read(&destination)?, text.as_bytes());
        Ok(())
    }

    #[test]
    fn atomic_publication_never_clobbers_and_cleans_temporary_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let bundle = assemble_scenario_export(
            &snapshot(scenario_from_json(Value::Null)?),
            &portable_encode::encode_fixture_domain,
        )?;
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("portable.eutheto");
        std::fs::write(&destination, b"sentinel")?;
        assert!(matches!(
            write_bundle_atomic(&destination, &bundle),
            Err(ExportError::DestinationExists(path)) if path == destination
        ));
        assert_eq!(std::fs::read(&destination)?, b"sentinel");
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);

        std::fs::remove_file(&destination)?;
        write_bundle_atomic(&destination, &bundle)?;
        assert_eq!(std::fs::read(&destination)?, bundle);
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&destination)?.permissions().mode() & 0o777,
                0o600
            );
        }
        Ok(())
    }

    #[test]
    fn publication_failpoints_have_unambiguous_outcomes() -> Result<(), Box<dyn std::error::Error>>
    {
        let bundle = assemble_scenario_export(
            &snapshot(scenario_from_json(Value::Null)?),
            &portable_encode::encode_fixture_domain,
        )?;
        let directory = tempfile::tempdir()?;
        let before = directory.path().join("before.eutheto");
        assert!(
            write_bundle_atomic_with_failpoint(
                &before,
                &bundle,
                PublicationFailpoint::BeforePublish,
            )
            .is_err()
        );
        assert!(!before.exists());

        let after = directory.path().join("after.eutheto");
        write_bundle_atomic_with_failpoint(&after, &bundle, PublicationFailpoint::AfterPublish)?;
        assert_eq!(std::fs::read(after)?, bundle);
        Ok(())
    }

    #[test]
    fn cancellation_before_or_during_write_never_publishes_or_leaks_temporary_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut backup = full_backup(scenario_from_json(Value::Null)?);
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let mut state = 0x9e37_79b9_usize;
        let large_text = (0..160 * 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                alphabet[state % alphabet.len()]
            })
            .collect();
        backup.sections.assets.insert(
            "large.txt".to_owned(),
            PortableAsset {
                bytes: large_text,
                media_type: "text/plain".to_owned(),
                redistribution_permitted: true,
            },
        );
        let bundle = assemble_full_backup(&backup, &portable_encode::encode_fixture_domain)?;
        assert!(bundle.len() > 64 * 1024);
        let directory = tempfile::tempdir()?;

        let before = directory.path().join("before.eutheto");
        std::fs::write(&before, b"sentinel")?;
        let signal = CancellationToken::new();
        signal.cancel();
        assert!(matches!(
            write_bundle_atomic_cancellable(&before, &bundle, &signal),
            Err(ExportError::Cancelled)
        ));
        assert_eq!(std::fs::read(&before)?, b"sentinel");

        let during = directory.path().join("during.eutheto");
        std::fs::write(&during, b"sentinel")?;
        let mut checks = 0_u32;
        assert!(matches!(
            write_bundle_atomic_with_control(&during, &bundle, PublicationFailpoint::None, || {
                checks += 1;
                checks == 4
            },),
            Err(ExportError::Cancelled)
        ));
        assert_eq!(std::fs::read(&during)?, b"sentinel");
        assert_eq!(checks, 4);
        let retained = std::fs::read_dir(directory.path())?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<BTreeSet<_>, _>>()?;
        assert_eq!(retained, BTreeSet::from([before, during]));

        let normal = directory.path().join("normal.eutheto");
        write_bundle_atomic_cancellable(&normal, &bundle, &CancellationToken::new())?;
        assert_eq!(std::fs::read(normal)?, bundle);
        Ok(())
    }
}
