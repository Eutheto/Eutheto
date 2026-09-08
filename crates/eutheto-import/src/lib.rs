//! Bounded inspection, preview, collision planning, and typed staging for
//! untrusted portable bundles.
//!
//! No function in this crate opens a database or commits application state.
//! The final [`StagedImport`] is an inert input for one later store transaction.

#[cfg(test)]
extern crate self as eutheto_import;
#[cfg(test)]
#[path = "../../../tests/support/portable_decode.rs"]
mod portable_decode;
#[cfg(test)]
#[path = "../../../tests/support/portable_encode.rs"]
mod portable_encode;
#[cfg(test)]
#[path = "../../../tests/support/portable_migrate.rs"]
mod portable_migrate;

use eutheto_command::validate_document_shape;
use eutheto_export::{
    ApplicationMetadata, BUNDLE_FORMAT, BackupSelection, BundleKind, BundleManifest,
    CHECKSUM_ALGORITHM, CHECKSUMS_PATH, CURRENT_BUNDLE_FORMAT_VERSION,
    CURRENT_PORTABLE_SCHEMA_VERSION, Checksums, MANIFEST_PATH, OmittedAssetPlaceholder,
    OmittedAssetReason, PORTABLE_LIMITS, PortableBackupAssetSelection, PortableLimits,
    PortableScenario, backup_selection_from_manifest, canonical_json, omitted_asset_placeholder,
    parse_omitted_asset_placeholder, reject_prohibited_portable_content, sha256_hex,
    validate_portable_json_value, validate_portable_path, validate_portable_payloads,
    validate_scenario_snapshot,
};
use eutheto_types::{
    BundleId, PackId, PortableAsset, Revision, Rfc3339Timestamp, SCENARIO_SNAPSHOT_SCHEMA_VERSION,
    ScenarioDocument, ScenarioFormat, ScenarioId, ScenarioSnapshotV1, SemanticCapability,
    SupplementalIdentity, SupplementalSectionKind, ValidationReport, collect_scenario_owned_uuids,
    collect_self_declared_uuids, extract_asset_references, extract_result_dependency,
    extract_result_id, extract_scenario_references, validate_scenario_owned_uuid_uniqueness,
};
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Cursor, Read, Write};
use std::mem::size_of;
use std::path::Path;
use std::sync::Arc;
use tempfile::{Builder as TempFileBuilder, NamedTempFile};
use thiserror::Error;
use uuid::Uuid;
use zip::ZipArchive;

#[derive(Clone, Debug)]
pub struct InspectionPolicy {
    /// Caller limits may tighten, but never raise, the fixed portable ceilings.
    pub limits: PortableLimits,
    /// Highest supported version for each semantic capability identifier.
    pub supported_capabilities: BTreeMap<String, u32>,
}

impl Default for InspectionPolicy {
    fn default() -> Self {
        Self {
            limits: PORTABLE_LIMITS,
            supported_capabilities: BTreeMap::new(),
        }
    }
}

impl InspectionPolicy {
    fn hardened(&self) -> Self {
        let mut limits = self.limits;
        limits.max_archive_bytes = limits
            .max_archive_bytes
            .min(PORTABLE_LIMITS.max_archive_bytes);
        limits.max_total_uncompressed_bytes = limits
            .max_total_uncompressed_bytes
            .min(PORTABLE_LIMITS.max_total_uncompressed_bytes);
        limits.max_entry_bytes = limits.max_entry_bytes.min(PORTABLE_LIMITS.max_entry_bytes);
        limits.max_entries = limits.max_entries.min(PORTABLE_LIMITS.max_entries);
        limits.max_compression_ratio = limits
            .max_compression_ratio
            .min(PORTABLE_LIMITS.max_compression_ratio);
        limits.max_path_bytes = limits.max_path_bytes.min(PORTABLE_LIMITS.max_path_bytes);
        limits.max_json_bytes = limits.max_json_bytes.min(PORTABLE_LIMITS.max_json_bytes);
        limits.max_json_depth = limits.max_json_depth.min(PORTABLE_LIMITS.max_json_depth);
        limits.max_string_bytes = limits
            .max_string_bytes
            .min(PORTABLE_LIMITS.max_string_bytes);
        limits.max_collection_items = limits
            .max_collection_items
            .min(PORTABLE_LIMITS.max_collection_items);
        Self {
            limits,
            supported_capabilities: self.supported_capabilities.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogicalBundle {
    pub manifest: Value,
    pub entries: BTreeMap<String, Vec<u8>>,
}

#[derive(Clone, Copy)]
pub struct OuterMigrationStep {
    pub from_version: u32,
    pub to_version: u32,
    pub name: &'static str,
    pub migrate: fn(LogicalBundle) -> Result<LogicalBundle, MigrationFailure>,
}

#[derive(Clone)]
pub struct PortableMigrationStep {
    pub from_version: u32,
    pub to_version: u32,
    pub name: &'static str,
    pub migrate:
        Arc<dyn Fn(Value) -> Result<PortableMigrationOutcome, MigrationFailure> + Send + Sync>,
}

/// A trusted global migration declares exactly the requirements it introduces.
pub struct PortableMigrationOutcome {
    pub value: Value,
    pub introduced_requirements: BTreeSet<SemanticCapability>,
    pub applied_migrations: Vec<AppliedMigration>,
}

/// Pack-owned decoding cannot replace the host's snapshot or wrapper fields.
pub struct DecodedPortableDomain {
    pub document: ScenarioDocument,
    pub required_capabilities: BTreeSet<SemanticCapability>,
    pub applied_migrations: Vec<AppliedMigration>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackMigrationVersionSpace {
    Internal,
    Portable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationSubject {
    pub pack_id: PackId,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
}

#[derive(Clone, Debug, Error)]
pub enum MigrationFailure {
    #[error("{0}")]
    Invalid(String),
    #[error("domain validation failed")]
    Validation(ValidationReport),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedMigration {
    pub registry: MigrationRegistryKind,
    pub name: String,
    pub from_version: u32,
    pub to_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_space: Option<PackMigrationVersionSpace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<MigrationSubject>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MigrationRegistryKind {
    Outer,
    Portable,
}

#[derive(Clone, Default)]
pub struct MigrationRegistries {
    outer: Vec<OuterMigrationStep>,
    portable: Vec<PortableMigrationStep>,
}

impl MigrationRegistries {
    #[must_use]
    pub fn current_only() -> Self {
        Self::default()
    }

    /// Builds migration registries after validating that each version step is
    /// unique and advances exactly one version.
    ///
    /// # Errors
    ///
    /// Returns [`ImportError::MigrationRegistry`] if either registry contains
    /// duplicate starting versions or a non-sequential migration step.
    pub fn new(
        mut outer: Vec<OuterMigrationStep>,
        mut portable: Vec<PortableMigrationStep>,
    ) -> Result<Self, ImportError> {
        outer.sort_by_key(|step| step.from_version);
        portable.sort_by_key(|step| step.from_version);
        validate_outer_steps(&outer)?;
        validate_portable_steps(&portable)?;
        Ok(Self { outer, portable })
    }

    fn migrate_outer(
        &self,
        mut version: u32,
        mut bundle: LogicalBundle,
        applied: &mut Vec<AppliedMigration>,
    ) -> Result<LogicalBundle, ImportError> {
        while version < CURRENT_BUNDLE_FORMAT_VERSION {
            let step = self
                .outer
                .iter()
                .find(|candidate| candidate.from_version == version)
                .ok_or(ImportError::UnsupportedOlderVersion {
                    space: VersionSpace::BundleFormat,
                    found: version,
                    current: CURRENT_BUNDLE_FORMAT_VERSION,
                })?;
            if step.to_version != version.saturating_add(1) {
                return Err(ImportError::MigrationRegistry(
                    "outer migrations must be sequential".to_owned(),
                ));
            }
            bundle = (step.migrate)(bundle).map_err(ImportError::Migration)?;
            applied.push(AppliedMigration {
                registry: MigrationRegistryKind::Outer,
                name: step.name.to_owned(),
                from_version: step.from_version,
                to_version: step.to_version,
                version_space: None,
                subject: None,
            });
            version = step.to_version;
        }
        Ok(bundle)
    }

    fn migrate_portable(
        &self,
        mut version: u32,
        mut value: Value,
        path: &str,
        limits: &PortableLimits,
        applied: &mut Vec<AppliedMigration>,
    ) -> Result<Value, ImportError> {
        while version < CURRENT_PORTABLE_SCHEMA_VERSION {
            let step = self
                .portable
                .iter()
                .find(|candidate| candidate.from_version == version)
                .ok_or(ImportError::UnsupportedOlderVersion {
                    space: VersionSpace::PortableSchema,
                    found: version,
                    current: CURRENT_PORTABLE_SCHEMA_VERSION,
                })?;
            let before = scenario_requirements(&value, version, path)?;
            let outcome = (step.migrate)(value).map_err(ImportError::Migration)?;
            value = outcome.value;
            if required_u32(&value, "schemaVersion", path)? != step.to_version {
                return Err(ImportError::Migration(MigrationFailure::Invalid(
                    "portable migration did not produce its declared next version".to_owned(),
                )));
            }
            validate_json_limits(path, &value, limits, 0)?;
            if u64::try_from(
                canonical_json(&value)
                    .map_err(|error| {
                        ImportError::Migration(MigrationFailure::Invalid(error.to_string()))
                    })?
                    .len(),
            )
            .map_or(true, |size| size > limits.max_json_bytes)
            {
                return Err(ImportError::JsonLimit {
                    path: path.to_owned(),
                    limit: "document bytes",
                });
            }
            let after = scenario_requirements(&value, step.to_version, path)?;
            let introduced = after.difference(&before).cloned().collect::<BTreeSet<_>>();
            if introduced != outcome.introduced_requirements {
                return Err(ImportError::Migration(MigrationFailure::Invalid(
                    "portable migration requirement changes are not exactly declared".to_owned(),
                )));
            }
            applied.extend(outcome.applied_migrations);
            version = step.to_version;
        }
        Ok(value)
    }

    fn record_global_portable_migrations(
        &self,
        mut version: u32,
        applied: &mut Vec<AppliedMigration>,
    ) {
        while let Some(step) = self
            .portable
            .iter()
            .find(|step| step.from_version == version)
        {
            if version >= CURRENT_PORTABLE_SCHEMA_VERSION {
                break;
            }
            applied.push(AppliedMigration {
                registry: MigrationRegistryKind::Portable,
                name: step.name.to_owned(),
                from_version: step.from_version,
                to_version: step.to_version,
                version_space: None,
                subject: None,
            });
            version = step.to_version;
        }
    }

    fn supports_outer_version(&self, mut version: u32) -> bool {
        if version > CURRENT_BUNDLE_FORMAT_VERSION {
            return false;
        }
        while version < CURRENT_BUNDLE_FORMAT_VERSION {
            let Some(step) = self
                .outer
                .iter()
                .find(|candidate| candidate.from_version == version)
            else {
                return false;
            };
            version = step.to_version;
        }
        true
    }

    fn supports_portable_version(&self, mut version: u32) -> bool {
        if version > CURRENT_PORTABLE_SCHEMA_VERSION {
            return false;
        }
        while version < CURRENT_PORTABLE_SCHEMA_VERSION {
            let Some(step) = self
                .portable
                .iter()
                .find(|candidate| candidate.from_version == version)
            else {
                return false;
            };
            version = step.to_version;
        }
        true
    }
}

fn validate_outer_steps(steps: &[OuterMigrationStep]) -> Result<(), ImportError> {
    let mut versions = BTreeSet::new();
    for step in steps {
        if step.to_version != step.from_version.saturating_add(1)
            || !versions.insert(step.from_version)
        {
            return Err(ImportError::MigrationRegistry(
                "outer registry has a duplicate or non-sequential step".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_portable_steps(steps: &[PortableMigrationStep]) -> Result<(), ImportError> {
    let mut versions = BTreeSet::new();
    for step in steps {
        if step.to_version != step.from_version.saturating_add(1)
            || !versions.insert(step.from_version)
        {
            return Err(ImportError::MigrationRegistry(
                "portable registry has a duplicate or non-sequential step".to_owned(),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionSpace {
    BundleFormat,
    PortableSchema,
}

impl fmt::Display for VersionSpace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BundleFormat => formatter.write_str("bundle format"),
            Self::PortableSchema => formatter.write_str("portable schema"),
        }
    }
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("archive exceeds the compressed-byte limit")]
    ArchiveTooLarge,
    #[error("archive has too many entries")]
    TooManyEntries,
    #[error("archive entry {path} exceeds its size limit")]
    EntryTooLarge { path: String },
    #[error("archive exceeds the total uncompressed-byte limit")]
    TotalSizeExceeded,
    #[error("archive entry {path} exceeds the compression-ratio limit")]
    CompressionRatio { path: String },
    #[error("unsafe archive path {path:?}: {reason}")]
    UnsafePath { path: String, reason: &'static str },
    #[error("archive contains duplicate normalized path {0}")]
    DuplicatePath(String),
    #[error("invalid ZIP32 structure: {0}")]
    InvalidZipStructure(&'static str),
    #[error("unsupported ZIP feature: {0}")]
    UnsupportedZipFeature(&'static str),
    #[error("archive contains Unicode case-colliding path {0}")]
    CaseCollision(String),
    #[error("archive entry {path} is a link, device, or other non-regular file")]
    NonRegularEntry { path: String },
    #[error("archive entry {path} is executable or a nested archive")]
    ProhibitedContent { path: String },
    #[error("archive is missing required entry {0}")]
    MissingEntry(String),
    #[error("archive contains undeclared entry {0}")]
    UndeclaredEntry(String),
    #[error("checksums do not declare archive entry {0}")]
    MissingChecksum(String),
    #[error("checksum mismatch for {0}")]
    ChecksumMismatch(String),
    #[error("invalid checksum declaration: {0}")]
    InvalidChecksums(String),
    #[error("invalid JSON in {path}: {message}")]
    InvalidJson { path: String, message: String },
    #[error("JSON limit exceeded in {path}: {limit}")]
    JsonLimit { path: String, limit: &'static str },
    #[error("unsupported newer {space} version {found}; current is {current}")]
    UnsupportedNewerVersion {
        space: VersionSpace,
        found: u32,
        current: u32,
    },
    #[error("unsupported historical {space} version {found}; current is {current}")]
    UnsupportedOlderVersion {
        space: VersionSpace,
        found: u32,
        current: u32,
    },
    #[error("unsupported bundle kind")]
    UnsupportedKind,
    #[error("unsupported required semantic capability")]
    UnsupportedCapability { id: String, version: u32 },
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid portable scenario {path}: {message}")]
    InvalidScenario { path: String, message: String },
    #[error("manifest count does not match archive contents for {0}")]
    CountMismatch(&'static str),
    #[error("portable migration registry is invalid: {0}")]
    MigrationRegistry(String),
    #[error("portable migration failed: {0}")]
    Migration(MigrationFailure),
    #[error("ZIP parsing failed: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("archive I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("preview is stale")]
    StalePreview,
    #[error("collision for scenario {0} has no explicit resolution")]
    UnresolvedCollision(ScenarioId),
    #[error("collision plan names scenario {0} that is not a collision in the preview")]
    UnknownCollision(ScenarioId),
    #[error("collision for supplemental record {0:?} has no explicit resolution")]
    UnresolvedSupplementalCollision(SupplementalIdentity),
    #[error("collision plan names supplemental record {0:?} that is not in the preview")]
    UnknownSupplementalCollision(SupplementalIdentity),
    #[error("scenario {scenario_id} revision cannot advance beyond the version-1 bound")]
    RevisionOverflow { scenario_id: ScenarioId },
    #[error("scenario {scenario_id} has conflicting records at revision {revision:?}")]
    ConflictingScenarioRevision {
        scenario_id: ScenarioId,
        revision: Revision,
    },
    #[error("ID remapping failed: {0}")]
    Remap(String),
    #[error("backup restore staging is invalid: {0}")]
    InvalidRestore(String),
}

#[derive(Clone, Debug)]
pub struct InspectedBundle {
    pub file_sha256: String,
    pub manifest: BundleManifest,
    pub checksums: Checksums,
    pub scenarios: Vec<ScenarioSnapshotV1>,
    pub scenario_revisions: Vec<ScenarioSnapshotV1>,
    pub additional_entries: BTreeMap<String, Vec<u8>>,
    pub applied_migrations: Vec<AppliedMigration>,
    pub original_format_version: u32,
    pub original_schema_version: u32,
    /// Conservative checked charge for all representation retained by a preview.
    pub retained_memory_bytes: usize,
}

/// Allowlisted metadata read without accepting, migrating, or reconstructing a bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreservedBundleMetadata {
    pub file_sha256: String,
    pub format: String,
    pub format_version: u32,
    pub portable_schema_version: u32,
    pub bundle_kind: Option<String>,
    pub title: Option<String>,
    pub required_capabilities: Vec<PreservedCapabilityMetadata>,
    pub scenarios: Vec<PreservedScenarioMetadata>,
}

/// One semantic capability advertised by an unopened bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreservedCapabilityMetadata {
    pub id: String,
    pub version: u32,
}

/// Allowlisted scenario identity and pack metadata from an unopened bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreservedScenarioMetadata {
    pub path: String,
    pub scenario_id: Option<String>,
    pub pack_id: Option<String>,
    pub internal_pack_schema_version: Option<u32>,
    pub portable_pack_schema_version: Option<u32>,
}

/// Bounded original archive bytes retained only for exact re-export.
///
/// This type is not an imported bundle, does not expose parsed domain payloads,
#[derive(Clone, Debug)]
pub struct UnopenedBundle {
    metadata: PreservedBundleMetadata,
    exact_bytes: Box<[u8]>,
    retained_memory_bytes: usize,
}

impl UnopenedBundle {
    /// Returns the allowlisted, non-authoritative envelope metadata.
    #[must_use]
    pub const fn metadata(&self) -> &PreservedBundleMetadata {
        &self.metadata
    }

    /// Returns the exact original bytes. Callers must not reconstruct the bundle.
    #[must_use]
    pub const fn exact_bytes(&self) -> &[u8] {
        &self.exact_bytes
    }

    /// Returns the conservative checked charge for all retained bytes and metadata.
    #[must_use]
    pub const fn retained_memory_bytes(&self) -> usize {
        self.retained_memory_bytes
    }

    /// Transfers the exact original bytes to an explicit re-export path.
    #[must_use]
    pub fn into_exact_bytes(self) -> Box<[u8]> {
        self.exact_bytes
    }
}

const ZIP_EOCD_SIGNATURE: u32 = 0x0605_4b50;
const ZIP64_EOCD_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const ZIP_CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const ZIP_EOCD_LEN: usize = 22;
const ZIP_CENTRAL_HEADER_LEN: usize = 46;
const ZIP_MAX_COMMENT_LEN: usize = u16::MAX as usize;

fn zip_u16(bytes: &[u8], offset: usize) -> Result<u16, ImportError> {
    let end = offset
        .checked_add(2)
        .ok_or(ImportError::InvalidZipStructure("ZIP offset overflow"))?;
    let field = bytes
        .get(offset..end)
        .ok_or(ImportError::InvalidZipStructure("truncated ZIP record"))?;
    Ok(u16::from_le_bytes([field[0], field[1]]))
}

fn zip_u32(bytes: &[u8], offset: usize) -> Result<u32, ImportError> {
    let end = offset
        .checked_add(4)
        .ok_or(ImportError::InvalidZipStructure("ZIP offset overflow"))?;
    let field = bytes
        .get(offset..end)
        .ok_or(ImportError::InvalidZipStructure("truncated ZIP record"))?;
    Ok(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

fn has_zip_signature(bytes: &[u8], offset: usize, signature: u32) -> bool {
    let Some(end) = offset.checked_add(4) else {
        return false;
    };
    bytes.get(offset..end) == Some(signature.to_le_bytes().as_slice())
}

fn find_zip32_eocd(bytes: &[u8]) -> Result<usize, ImportError> {
    let search_window = ZIP_EOCD_LEN
        .checked_add(ZIP_MAX_COMMENT_LEN)
        .ok_or(ImportError::InvalidZipStructure("ZIP offset overflow"))?;
    let search_start = bytes.len().saturating_sub(search_window);
    let search_end = bytes.len().saturating_sub(4);
    let mut found = None;
    let mut saw_signature = false;

    for offset in search_start..=search_end {
        if !has_zip_signature(bytes, offset, ZIP_EOCD_SIGNATURE) {
            continue;
        }
        saw_signature = true;
        let Some(fixed_end) = offset.checked_add(ZIP_EOCD_LEN) else {
            return Err(ImportError::InvalidZipStructure("ZIP offset overflow"));
        };
        if fixed_end > bytes.len() {
            continue;
        }
        let comment_len = usize::from(zip_u16(bytes, offset + 20)?);
        let record_end = fixed_end
            .checked_add(comment_len)
            .ok_or(ImportError::InvalidZipStructure("ZIP offset overflow"))?;
        if record_end != bytes.len() {
            continue;
        }
        if found.replace(offset).is_some() {
            return Err(ImportError::InvalidZipStructure(
                "ambiguous end-of-central-directory records",
            ));
        }
    }

    found.ok_or(ImportError::InvalidZipStructure(if saw_signature {
        "truncated or inconsistent end-of-central-directory record"
    } else {
        "missing end-of-central-directory record"
    }))
}

fn validate_central_extra_fields(bytes: &[u8]) -> Result<(), ImportError> {
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let header_end = offset
            .checked_add(4)
            .ok_or(ImportError::InvalidZipStructure(
                "central-directory extra-field offset overflow",
            ))?;
        if header_end > bytes.len() {
            return Err(ImportError::InvalidZipStructure(
                "truncated central-directory extra field",
            ));
        }
        let tag = zip_u16(bytes, offset)?;
        let value_len = usize::from(zip_u16(bytes, offset + 2)?);
        let value_end =
            header_end
                .checked_add(value_len)
                .ok_or(ImportError::InvalidZipStructure(
                    "central-directory extra-field offset overflow",
                ))?;
        if value_end > bytes.len() {
            return Err(ImportError::InvalidZipStructure(
                "truncated central-directory extra field",
            ));
        }
        if tag == 0x0001 {
            return Err(ImportError::UnsupportedZipFeature("ZIP64 archive"));
        }
        if tag == 0x9901 {
            return Err(ImportError::UnsupportedZipFeature(
                "encrypted archive entry",
            ));
        }
        offset = value_end;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Zip32CentralDirectory {
    offset: usize,
    end: usize,
    entry_count: usize,
}

fn zip32_central_directory(
    bytes: &[u8],
    max_entries: usize,
) -> Result<Zip32CentralDirectory, ImportError> {
    let eocd_offset = find_zip32_eocd(bytes)?;
    if eocd_offset >= 20 && has_zip_signature(bytes, eocd_offset - 20, ZIP64_EOCD_LOCATOR_SIGNATURE)
    {
        return Err(ImportError::UnsupportedZipFeature("ZIP64 archive"));
    }

    let disk_number = zip_u16(bytes, eocd_offset + 4)?;
    let central_disk = zip_u16(bytes, eocd_offset + 6)?;
    let entries_on_disk = zip_u16(bytes, eocd_offset + 8)?;
    let entry_count = zip_u16(bytes, eocd_offset + 10)?;
    let central_size = zip_u32(bytes, eocd_offset + 12)?;
    let central_offset = zip_u32(bytes, eocd_offset + 16)?;
    if disk_number == u16::MAX
        || central_disk == u16::MAX
        || entries_on_disk == u16::MAX
        || entry_count == u16::MAX
        || central_size == u32::MAX
        || central_offset == u32::MAX
    {
        return Err(ImportError::UnsupportedZipFeature("ZIP64 archive"));
    }
    if disk_number != 0 || central_disk != 0 || entries_on_disk != entry_count {
        return Err(ImportError::UnsupportedZipFeature("multi-disk ZIP archive"));
    }

    let entry_count = usize::from(entry_count);
    if entry_count > max_entries {
        return Err(ImportError::TooManyEntries);
    }
    let central_offset = usize::try_from(central_offset)
        .map_err(|_| ImportError::InvalidZipStructure("central-directory offset overflow"))?;
    let central_size = usize::try_from(central_size)
        .map_err(|_| ImportError::InvalidZipStructure("central-directory size overflow"))?;
    let central_end =
        central_offset
            .checked_add(central_size)
            .ok_or(ImportError::InvalidZipStructure(
                "central-directory offset overflow",
            ))?;
    if central_end != eocd_offset {
        return Err(ImportError::InvalidZipStructure(
            "central-directory offset or size does not match EOCD",
        ));
    }
    Ok(Zip32CentralDirectory {
        offset: central_offset,
        end: central_end,
        entry_count,
    })
}

fn zip32_central_record(
    bytes: &[u8],
    cursor: usize,
    central_end: usize,
) -> Result<(&[u8], usize), ImportError> {
    let fixed_end =
        cursor
            .checked_add(ZIP_CENTRAL_HEADER_LEN)
            .ok_or(ImportError::InvalidZipStructure(
                "central-directory record offset overflow",
            ))?;
    if fixed_end > central_end {
        return Err(ImportError::InvalidZipStructure(
            "truncated central-directory record",
        ));
    }
    if !has_zip_signature(bytes, cursor, ZIP_CENTRAL_SIGNATURE) {
        return Err(ImportError::InvalidZipStructure(
            "invalid central-directory record signature",
        ));
    }

    let flags = zip_u16(bytes, cursor + 8)?;
    let compressed_size = zip_u32(bytes, cursor + 20)?;
    let uncompressed_size = zip_u32(bytes, cursor + 24)?;
    let name_len = usize::from(zip_u16(bytes, cursor + 28)?);
    let extra_len = usize::from(zip_u16(bytes, cursor + 30)?);
    let comment_len = usize::from(zip_u16(bytes, cursor + 32)?);
    let disk_start = zip_u16(bytes, cursor + 34)?;
    let local_offset = zip_u32(bytes, cursor + 42)?;
    if compressed_size == u32::MAX
        || uncompressed_size == u32::MAX
        || disk_start == u16::MAX
        || local_offset == u32::MAX
    {
        return Err(ImportError::UnsupportedZipFeature("ZIP64 archive"));
    }
    if disk_start != 0 {
        return Err(ImportError::UnsupportedZipFeature("multi-disk ZIP archive"));
    }
    if flags & ((1 << 0) | (1 << 6)) != 0 {
        return Err(ImportError::UnsupportedZipFeature(
            "encrypted archive entry",
        ));
    }

    let name_end = fixed_end
        .checked_add(name_len)
        .ok_or(ImportError::InvalidZipStructure(
            "central-directory name offset overflow",
        ))?;
    let extra_end = name_end
        .checked_add(extra_len)
        .ok_or(ImportError::InvalidZipStructure(
            "central-directory extra-field offset overflow",
        ))?;
    let record_end = extra_end
        .checked_add(comment_len)
        .ok_or(ImportError::InvalidZipStructure(
            "central-directory comment offset overflow",
        ))?;
    if record_end > central_end {
        return Err(ImportError::InvalidZipStructure(
            "truncated central-directory variable fields",
        ));
    }
    let name = bytes
        .get(fixed_end..name_end)
        .ok_or(ImportError::InvalidZipStructure(
            "truncated central-directory name",
        ))?;
    let extra = bytes
        .get(name_end..extra_end)
        .ok_or(ImportError::InvalidZipStructure(
            "truncated central-directory extra fields",
        ))?;
    validate_central_extra_fields(extra)?;
    Ok((name, record_end))
}

fn zip32_central_names(
    bytes: &[u8],
    directory: Zip32CentralDirectory,
) -> Result<Vec<&[u8]>, ImportError> {
    let mut cursor = directory.offset;
    let mut raw_names = Vec::new();
    for _ in 0..directory.entry_count {
        let (name, record_end) = zip32_central_record(bytes, cursor, directory.end)?;
        raw_names.push(name);
        cursor = record_end;
    }
    if cursor != directory.end {
        return Err(ImportError::InvalidZipStructure(
            "central-directory entry count or size does not match EOCD",
        ));
    }
    Ok(raw_names)
}

fn validate_zip32_names(raw_names: &[&[u8]], max_path_bytes: usize) -> Result<(), ImportError> {
    let mut unique_raw_names = BTreeSet::new();
    let mut unique_normalized_names = BTreeSet::new();
    for &raw_name in raw_names {
        let name = std::str::from_utf8(raw_name).map_err(|_| ImportError::UnsafePath {
            path: String::from_utf8_lossy(raw_name).into_owned(),
            reason: "path is not valid UTF-8",
        })?;
        if !unique_raw_names.insert(raw_name) {
            return Err(ImportError::DuplicatePath(name.to_owned()));
        }
        let normalized = normalize_path(name, max_path_bytes)?;
        if !unique_normalized_names.insert(normalized.clone()) {
            return Err(ImportError::DuplicatePath(normalized));
        }
    }
    Ok(())
}

fn validate_zip32_central_directory(
    bytes: &[u8],
    max_entries: usize,
    max_path_bytes: usize,
) -> Result<(), ImportError> {
    let directory = zip32_central_directory(bytes, max_entries)?;
    let raw_names = zip32_central_names(bytes, directory)?;
    validate_zip32_names(&raw_names, max_path_bytes)
}

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

fn validate_archive_metadata(bytes: &[u8], policy: &InspectionPolicy) -> Result<(), ImportError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    if archive.len() > policy.limits.max_entries {
        return Err(ImportError::TooManyEntries);
    }
    let mut paths = BTreeSet::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let raw_name = entry.name_raw();
        let name = std::str::from_utf8(raw_name).map_err(|_| ImportError::UnsafePath {
            path: String::from_utf8_lossy(raw_name).into_owned(),
            reason: "path is not valid UTF-8",
        })?;
        normalize_path(name, policy.limits.max_path_bytes)?;
        if entry.is_dir() {
            return Err(ImportError::NonRegularEntry {
                path: name.to_owned(),
            });
        }
        validate_regular_mode(entry.unix_mode(), name)?;
        let folded = name.to_ascii_lowercase();
        if !paths.insert(folded) {
            return Err(ImportError::DuplicatePath(name.to_owned()));
        }
        let size = entry.size();
        if size > policy.limits.max_entry_bytes {
            return Err(ImportError::EntryTooLarge {
                path: name.to_owned(),
            });
        }
        total = total
            .checked_add(size)
            .ok_or(ImportError::TotalSizeExceeded)?;
        if total > policy.limits.max_total_uncompressed_bytes {
            return Err(ImportError::TotalSizeExceeded);
        }
        let compressed = entry.compressed_size();
        if size > 0
            && (compressed == 0 || size / compressed.max(1) > policy.limits.max_compression_ratio)
        {
            return Err(ImportError::CompressionRatio {
                path: name.to_owned(),
            });
        }
    }
    Ok(())
}

// All archive passes share the same actual-output bound, including checksum-only inspection.
fn consume_bounded_zip_entry(
    reader: &mut impl Read,
    buffer: &mut [u8],
    path: &str,
    declared_size: u64,
    max_entry_bytes: u64,
    remaining_total: &mut u64,
    mut consume: impl FnMut(&[u8]),
) -> Result<(), ImportError> {
    if declared_size > max_entry_bytes {
        return Err(ImportError::EntryTooLarge {
            path: path.to_owned(),
        });
    }
    if declared_size > *remaining_total {
        return Err(ImportError::TotalSizeExceeded);
    }
    let mut count = 0_u64;
    loop {
        let remaining = declared_size
            .saturating_sub(count)
            .min(max_entry_bytes.saturating_sub(count))
            .min(*remaining_total);
        let request = usize::try_from(remaining.saturating_add(1))
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = reader.read(&mut buffer[..request])?;
        if read == 0 {
            break;
        }
        let amount = u64::try_from(read).map_err(|_| ImportError::TotalSizeExceeded)?;
        if amount > remaining {
            return Err(if amount > *remaining_total {
                ImportError::TotalSizeExceeded
            } else {
                ImportError::EntryTooLarge {
                    path: path.to_owned(),
                }
            });
        }
        count = count
            .checked_add(amount)
            .ok_or(ImportError::TotalSizeExceeded)?;
        *remaining_total = remaining_total
            .checked_sub(amount)
            .ok_or(ImportError::TotalSizeExceeded)?;
        consume(&buffer[..read]);
    }
    if count != declared_size {
        return Err(ImportError::EntryTooLarge {
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn read_fixed_entry(bytes: &[u8], path: &str, max_bytes: u64) -> Result<Vec<u8>, ImportError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut entry = archive
        .by_name(path)
        .map_err(|_| ImportError::MissingEntry(path.to_owned()))?;
    let declared_size = entry.size();
    let mut content = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut remaining = max_bytes;
    consume_bounded_zip_entry(
        &mut entry,
        &mut buffer,
        path,
        declared_size,
        max_bytes,
        &mut remaining,
        |chunk| content.extend_from_slice(chunk),
    )?;
    Ok(content)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestEnvelopeMetadata {
    format: String,
    format_version: u32,
    schema_version: u32,
    #[serde(default)]
    bundle_kind: Option<String>,
    #[serde(default)]
    title: Option<String>,
    integrity: ManifestIntegrityMetadata,
    #[serde(default)]
    required_capabilities: Vec<CapabilityMetadataWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestIntegrityMetadata {
    algorithm: String,
    checksums_file: String,
}

#[derive(Deserialize)]
struct CapabilityMetadataWire {
    id: String,
    version: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScenarioEnvelopeMetadata {
    #[serde(default)]
    schema_version: Option<u32>,
    #[serde(default)]
    scenario_id: Option<String>,
    #[serde(default)]
    domain: Option<PortableDomainMetadata>,
    #[serde(default)]
    document: Option<ScenarioDocumentMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortableDomainMetadata {
    #[serde(default)]
    pack_id: Option<String>,
    #[serde(default)]
    schema_version: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScenarioDocumentMetadata {
    #[serde(default)]
    scenario_id: Option<String>,
    #[serde(default, rename = "domain")]
    _domain: Option<IgnoredAny>,
    #[serde(default)]
    domain_pack: Option<ScenarioPackMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScenarioPackMetadata {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    schema_version: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScenarioRequirementMetadata {
    schema_version: u32,
    #[serde(default)]
    required_capabilities: BTreeSet<SemanticCapability>,
    // This is a JSON object projection, not a set encoded as a JSON array.
    #[expect(clippy::zero_sized_map_values)]
    #[serde(default)]
    semantic_extensions: BTreeMap<String, IgnoredAny>,
    #[serde(default)]
    domain: Option<DomainRequirementMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DomainRequirementMetadata {
    required_capabilities: BTreeSet<SemanticCapability>,
}

fn parse_metadata_json<T: DeserializeOwned>(
    path: &str,
    reader: impl Read,
) -> Result<T, ImportError> {
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let metadata = T::deserialize(&mut deserializer).map_err(|error| ImportError::InvalidJson {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    deserializer
        .end()
        .map_err(|error| ImportError::InvalidJson {
            path: path.to_owned(),
            message: error.to_string(),
        })?;
    Ok(metadata)
}

fn manifest_envelope_metadata(
    bytes: &[u8],
    limits: &PortableLimits,
) -> Result<ManifestEnvelopeMetadata, ImportError> {
    let metadata: ManifestEnvelopeMetadata =
        parse_metadata_json(MANIFEST_PATH, Cursor::new(bytes))?;
    if metadata.format.len() > limits.max_string_bytes
        || metadata
            .bundle_kind
            .as_ref()
            .is_some_and(|value| value.len() > limits.max_string_bytes)
        || metadata
            .title
            .as_ref()
            .is_some_and(|value| value.len() > limits.max_string_bytes)
        || metadata.required_capabilities.len() > limits.max_collection_items
    {
        return Err(ImportError::JsonLimit {
            path: MANIFEST_PATH.to_owned(),
            limit: "metadata",
        });
    }
    for capability in &metadata.required_capabilities {
        if capability.id.len() > limits.max_string_bytes {
            return Err(ImportError::JsonLimit {
                path: MANIFEST_PATH.to_owned(),
                limit: "string bytes",
            });
        }
        if capability.version == 0 || !valid_namespace(&capability.id) {
            return Err(ImportError::InvalidManifest(
                "a required capability declaration is invalid".to_owned(),
            ));
        }
    }
    if metadata.format != BUNDLE_FORMAT {
        return Err(ImportError::InvalidManifest(format!(
            "format must be {BUNDLE_FORMAT}"
        )));
    }
    if metadata.integrity.algorithm != CHECKSUM_ALGORITHM
        || metadata.integrity.checksums_file != CHECKSUMS_PATH
    {
        return Err(ImportError::InvalidManifest(
            "integrity declaration is not the fixed SHA-256 checksums file".to_owned(),
        ));
    }
    Ok(metadata)
}

fn stream_validate_checksums(
    bytes: &[u8],
    checksums: &Checksums,
    policy: &InspectionPolicy,
) -> Result<(), ImportError> {
    if checksums.algorithm != CHECKSUM_ALGORITHM {
        return Err(ImportError::InvalidChecksums(
            "only SHA-256 is supported".to_owned(),
        ));
    }
    if checksums.files.contains_key(CHECKSUMS_PATH) {
        return Err(ImportError::InvalidChecksums(
            "checksums.json cannot checksum itself".to_owned(),
        ));
    }
    if checksums.files.len() > policy.limits.max_entries {
        return Err(ImportError::TooManyEntries);
    }
    for (path, expected) in &checksums.files {
        normalize_path(path, policy.limits.max_path_bytes)?;
        if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ImportError::InvalidChecksums(
                "a SHA-256 declaration is invalid".to_owned(),
            ));
        }
    }

    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut seen = BTreeSet::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut remaining_total = policy.limits.max_total_uncompressed_bytes;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let path = std::str::from_utf8(entry.name_raw())
            .map_err(|_| ImportError::UnsafePath {
                path: String::new(),
                reason: "path is not valid UTF-8",
            })?
            .to_owned();
        let expected = if path == CHECKSUMS_PATH {
            None
        } else {
            Some(
                checksums
                    .files
                    .get(&path)
                    .ok_or_else(|| ImportError::MissingChecksum(path.clone()))?,
            )
        };
        let mut hasher = Sha256::new();
        let mut prefix = Vec::with_capacity(262);
        let declared_size = entry.size();
        consume_bounded_zip_entry(
            &mut entry,
            &mut buffer,
            &path,
            declared_size,
            policy.limits.max_entry_bytes,
            &mut remaining_total,
            |chunk| {
                hasher.update(chunk);
                if prefix.len() < 262 {
                    let remaining = 262 - prefix.len();
                    prefix.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                }
            },
        )?;
        reject_prohibited_content(&path, &prefix)?;
        if let Some(expected) = expected {
            let actual = format!("{:x}", hasher.finalize());
            if actual != expected.to_ascii_lowercase() {
                return Err(ImportError::ChecksumMismatch(path.clone()));
            }
            seen.insert(path);
        }
    }
    for path in checksums.files.keys() {
        if !seen.contains(path) {
            return Err(ImportError::UndeclaredEntry(path.clone()));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestPreflight {
    format: String,
    format_version: u32,
    schema_version: u32,
    #[serde(default)]
    required_capabilities: BTreeSet<SemanticCapability>,
}

fn preflight_manifest(
    bytes: &[u8],
    policy: &InspectionPolicy,
    registries: &MigrationRegistries,
) -> Result<(Value, u32, u32, usize), ImportError> {
    let manifest_bytes = read_fixed_entry(bytes, MANIFEST_PATH, MAX_MANIFEST_BYTES)?;
    let mut manifest_limits = policy.limits;
    manifest_limits.max_json_bytes = MAX_MANIFEST_BYTES;
    let parsed = parse_strict_json_charged(MANIFEST_PATH, &manifest_bytes, &manifest_limits)?;
    let value = parsed.value;
    let preflight: ManifestPreflight = serde_json::from_value(value.clone()).map_err(|error| {
        ImportError::InvalidManifest(format!("manifest preflight failed: {error}"))
    })?;
    if preflight.format != BUNDLE_FORMAT {
        return Err(ImportError::InvalidManifest(format!(
            "format must be {BUNDLE_FORMAT}"
        )));
    }
    if preflight.format_version > CURRENT_BUNDLE_FORMAT_VERSION {
        return Err(ImportError::UnsupportedNewerVersion {
            space: VersionSpace::BundleFormat,
            found: preflight.format_version,
            current: CURRENT_BUNDLE_FORMAT_VERSION,
        });
    }
    if !registries.supports_outer_version(preflight.format_version) {
        return Err(ImportError::UnsupportedOlderVersion {
            space: VersionSpace::BundleFormat,
            found: preflight.format_version,
            current: CURRENT_BUNDLE_FORMAT_VERSION,
        });
    }
    if preflight.schema_version > CURRENT_PORTABLE_SCHEMA_VERSION {
        return Err(ImportError::UnsupportedNewerVersion {
            space: VersionSpace::PortableSchema,
            found: preflight.schema_version,
            current: CURRENT_PORTABLE_SCHEMA_VERSION,
        });
    }
    if !registries.supports_portable_version(preflight.schema_version) {
        return Err(ImportError::UnsupportedOlderVersion {
            space: VersionSpace::PortableSchema,
            found: preflight.schema_version,
            current: CURRENT_PORTABLE_SCHEMA_VERSION,
        });
    }
    for capability in &preflight.required_capabilities {
        if capability.version == 0 || !valid_namespace(&capability.id) {
            return Err(ImportError::InvalidManifest(
                "a required capability declaration is invalid".to_owned(),
            ));
        }
    }
    Ok((
        value,
        preflight.format_version,
        preflight.schema_version,
        parsed.representation_bytes,
    ))
}

fn read_archive_entries(
    bytes: &[u8],
    policy: &InspectionPolicy,
) -> Result<BTreeMap<String, Vec<u8>>, ImportError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    if archive.len() > policy.limits.max_entries {
        return Err(ImportError::TooManyEntries);
    }

    let mut entries = BTreeMap::new();
    let mut remaining_total = policy.limits.max_total_uncompressed_bytes;
    let mut buffer = [0_u8; 16 * 1024];
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let raw_name = entry.name_raw();
        let name = std::str::from_utf8(raw_name)
            .map_err(|_| ImportError::UnsafePath {
                path: String::from_utf8_lossy(raw_name).into_owned(),
                reason: "path is not valid UTF-8",
            })?
            .to_owned();
        let normalized = normalize_path(&name, policy.limits.max_path_bytes)?;
        let declared_size = entry.size();
        let mut content = Vec::new();
        consume_bounded_zip_entry(
            &mut entry,
            &mut buffer,
            &normalized,
            declared_size,
            policy.limits.max_entry_bytes,
            &mut remaining_total,
            |chunk| content.extend_from_slice(chunk),
        )?;
        reject_prohibited_content(&normalized, &content)?;
        if entries.insert(normalized.clone(), content).is_some() {
            return Err(ImportError::DuplicatePath(normalized));
        }
    }
    Ok(entries)
}

struct InspectedLogicalEntries {
    scenarios: Vec<ScenarioSnapshotV1>,
    scenario_revisions: Vec<ScenarioSnapshotV1>,
    additional_entries: BTreeMap<String, Vec<u8>>,
    retained_memory_bytes: usize,
}

fn conservative_parsed_memory_charge(
    path: &str,
    _raw_bytes: usize,
    traversal_charge: usize,
) -> Result<usize, ImportError> {
    // The traversal charge includes every Value node, decoded string/key byte,
    // collection item, and 96 bytes per item for BTree links, String headers,
    // allocator metadata, and collection spare capacity. A fixed typed-envelope
    // allowance covers the bounded non-Value fields after from_value conversion.
    traversal_charge
        .checked_add(4_096)
        .ok_or_else(|| ImportError::JsonLimit {
            path: path.to_owned(),
            limit: "representation bytes",
        })
}

fn add_retained_memory(
    path: &str,
    total: &mut usize,
    amount: usize,
    limit: u64,
) -> Result<(), ImportError> {
    let next = total
        .checked_add(amount)
        .ok_or_else(|| ImportError::JsonLimit {
            path: path.to_owned(),
            limit: "representation bytes",
        })?;
    if u64::try_from(next).map_or(true, |next| next > limit) {
        return Err(ImportError::JsonLimit {
            path: path.to_owned(),
            limit: "representation bytes",
        });
    }
    *total = next;
    Ok(())
}

/// Conservative checked allocation charge for a retained JSON value.
///
/// Returns `None` on size overflow; callers must reject rather than undercharge.
#[must_use]
pub fn retained_value_memory_charge(value: &Value) -> Option<usize> {
    let mut charge = size_of::<Value>();
    match value {
        Value::String(string) => charge.checked_add(string.capacity()),
        Value::Array(values) => {
            charge = charge.checked_add(values.len().checked_mul(96)?)?;
            for value in values {
                charge = charge.checked_add(retained_value_memory_charge(value)?)?;
            }
            charge.checked_add(
                values
                    .capacity()
                    .saturating_sub(values.len())
                    .checked_mul(size_of::<Value>())?,
            )
        }
        Value::Object(values) => {
            charge = charge.checked_add(values.len().checked_mul(96)?)?;
            for (key, value) in values {
                charge = charge
                    .checked_add(key.capacity())?
                    .checked_add(retained_value_memory_charge(value)?)?;
            }
            Some(charge)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Some(charge),
    }
}

fn declared_capabilities(
    value: Option<&Value>,
    path: &str,
) -> Result<BTreeSet<SemanticCapability>, ImportError> {
    let capabilities: BTreeSet<SemanticCapability> = value
        .map_or_else(
            || Ok(BTreeSet::new()),
            |value| serde_json::from_value(value.clone()),
        )
        .map_err(|error| ImportError::InvalidScenario {
            path: path.to_owned(),
            message: error.to_string(),
        })?;
    if capabilities
        .iter()
        .any(|capability| capability.version == 0 || !valid_namespace(&capability.id))
    {
        return Err(ImportError::InvalidScenario {
            path: path.to_owned(),
            message: "invalid semantic capability declaration".to_owned(),
        });
    }
    Ok(capabilities)
}

fn scenario_requirements(
    value: &Value,
    version: u32,
    path: &str,
) -> Result<BTreeSet<SemanticCapability>, ImportError> {
    let mut requirements = declared_capabilities(value.get("requiredCapabilities"), path)?;
    if version >= 2 {
        requirements.extend(declared_capabilities(
            value
                .get("domain")
                .and_then(|domain| domain.get("requiredCapabilities")),
            path,
        )?);
    }
    Ok(requirements)
}

/// Check the source declarations before any trusted migration can introduce requirements.
fn validate_source_requirements(
    entries: &BTreeMap<String, Vec<u8>>,
    manifest: &BundleManifest,
    policy: &InspectionPolicy,
) -> Result<(), ImportError> {
    let mut requirements = BTreeSet::new();
    for (path, content) in entries {
        if !path.starts_with("scenarios/") && !path.starts_with("scenario-revisions/") {
            continue;
        }
        let metadata: ScenarioRequirementMetadata =
            parse_metadata_json(path, Cursor::new(content))?;
        if metadata.schema_version != manifest.schema_version {
            return Err(ImportError::InvalidScenario {
                path: path.clone(),
                message: "scenario schema version does not match the source manifest".to_owned(),
            });
        }
        requirements.extend(validate_source_scenario_requirements(
            metadata, path, policy,
        )?);
    }
    if requirements != manifest.required_capabilities {
        return Err(ImportError::InvalidManifest(
            "required capabilities are not the exact union of source scenario declarations"
                .to_owned(),
        ));
    }
    Ok(())
}

fn validate_source_scenario_requirements(
    metadata: ScenarioRequirementMetadata,
    path: &str,
    policy: &InspectionPolicy,
) -> Result<BTreeSet<SemanticCapability>, ImportError> {
    let mut requirements = metadata.required_capabilities;
    for capability in &requirements {
        require_capability(capability, policy)?;
    }
    for namespace in metadata.semantic_extensions.keys() {
        if !requirements
            .iter()
            .any(|capability| capability.id == *namespace)
        {
            return Err(ImportError::UnsupportedCapability {
                id: namespace.clone(),
                version: 0,
            });
        }
    }
    if metadata.schema_version >= 2 {
        let domain = metadata
            .domain
            .ok_or_else(|| ImportError::InvalidScenario {
                path: path.to_owned(),
                message: "portable domain requirements are missing".to_owned(),
            })?;
        requirements.extend(domain.required_capabilities);
    }
    Ok(requirements)
}

/// Checked standalone scenario data, without bundle or database authority.
pub struct InspectedScenario {
    pub scenario: ScenarioSnapshotV1,
    pub applied_migrations: Vec<AppliedMigration>,
    pub original_schema_version: u32,
}

/// Strictly decodes one portable JSON scenario, including supported historical migrations.
///
/// Bare JSON has no asset custody and may reference only its own scenario identity.
/// Nonsemantic namespace maps declare themselves; bundle manifest rules remain bundle-only.
///
/// # Errors
/// Rejects unsupported versions/capabilities, malformed or oversized data and missing dependencies.
pub fn inspect_scenario(
    bytes: &[u8],
    policy: &InspectionPolicy,
    registries: &MigrationRegistries,
    decode_domain: &impl Fn(&PortableScenario) -> Result<DecodedPortableDomain, MigrationFailure>,
) -> Result<InspectedScenario, ImportError> {
    let policy = policy.hardened();
    let mut applied_migrations = Vec::new();
    let mut retained_memory_bytes = 0;
    let (scenario, _, original_schema_version) = inspect_scenario_entry(
        "scenario.json",
        bytes,
        &policy,
        registries,
        &mut applied_migrations,
        &mut retained_memory_bytes,
        decode_domain,
    )?;
    let mut global_migrations = Vec::new();
    registries.record_global_portable_migrations(original_schema_version, &mut global_migrations);
    global_migrations.append(&mut applied_migrations);
    let applied_migrations = global_migrations;
    let migration_metadata = serde_json::to_value(&applied_migrations)
        .map_err(|error| ImportError::Migration(MigrationFailure::Invalid(error.to_string())))?;
    validate_json_limits("scenario.json", &migration_metadata, &policy.limits, 0)?;
    let metadata_charge = retained_value_memory_charge(&migration_metadata).ok_or_else(|| {
        ImportError::JsonLimit {
            path: "scenario.json".to_owned(),
            limit: "representation bytes",
        }
    })?;
    add_retained_memory(
        "scenario.json",
        &mut retained_memory_bytes,
        metadata_charge,
        policy.limits.max_total_uncompressed_bytes,
    )?;
    validate_standalone_scenario(&scenario, &policy)?;
    Ok(InspectedScenario {
        scenario,
        applied_migrations,
        original_schema_version,
    })
}

/// Validates current standalone snapshot bounds, identity and dependency closure.
///
/// # Errors
/// Rejects malformed snapshots, unsupported host semantics, external scenario/asset references
/// and resource-limit violations. Domain support is checked by the registered pack separately.
pub fn validate_standalone_scenario(
    scenario: &ScenarioSnapshotV1,
    policy: &InspectionPolicy,
) -> Result<(), ImportError> {
    let policy = policy.hardened();
    let charge = decoded_snapshot_memory_charge("scenario.json", scenario, &policy.limits)?;
    add_retained_memory(
        "scenario.json",
        &mut 0,
        charge,
        policy.limits.max_total_uncompressed_bytes,
    )?;
    validate_decoded_scenario("scenario.json", scenario, &policy)?;
    validate_bundle_references(std::slice::from_ref(scenario), &[], &BTreeMap::new())?;
    Ok(())
}

fn inspect_scenario_entry(
    path: &str,
    content: &[u8],
    policy: &InspectionPolicy,
    registries: &MigrationRegistries,
    applied_migrations: &mut Vec<AppliedMigration>,
    retained_memory_bytes: &mut usize,
    decode_domain: &impl Fn(&PortableScenario) -> Result<DecodedPortableDomain, MigrationFailure>,
) -> Result<(ScenarioSnapshotV1, BTreeSet<SemanticCapability>, u32), ImportError> {
    let parsed = parse_strict_json_charged(path, content, &policy.limits)?;
    let parsed_memory_bytes =
        conservative_parsed_memory_charge(path, content.len(), parsed.representation_bytes)?;
    add_retained_memory(
        path,
        retained_memory_bytes,
        parsed_memory_bytes,
        policy.limits.max_total_uncompressed_bytes,
    )?;
    let value = parsed.value;
    let version = required_u32(&value, "schemaVersion", path)?;
    if version > CURRENT_PORTABLE_SCHEMA_VERSION {
        return Err(ImportError::UnsupportedNewerVersion {
            space: VersionSpace::PortableSchema,
            found: version,
            current: CURRENT_PORTABLE_SCHEMA_VERSION,
        });
    }
    let metadata = ScenarioRequirementMetadata::deserialize(&value).map_err(|error| {
        ImportError::InvalidScenario {
            path: path.to_owned(),
            message: error.to_string(),
        }
    })?;
    validate_source_scenario_requirements(metadata, path, policy)?;
    let current =
        registries.migrate_portable(version, value, path, &policy.limits, applied_migrations)?;
    let migrated_memory_bytes = conservative_parsed_memory_charge(
        path,
        content.len(),
        retained_value_memory_charge(&current).ok_or_else(|| ImportError::JsonLimit {
            path: path.to_owned(),
            limit: "representation bytes",
        })?,
    )?;
    add_retained_memory(
        path,
        retained_memory_bytes,
        migrated_memory_bytes.saturating_sub(parsed_memory_bytes),
        policy.limits.max_total_uncompressed_bytes,
    )?;
    let wire = serde_json::from_value(current).map_err(|error| ImportError::InvalidScenario {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    let (scenario, requirements) =
        decode_portable_snapshot(wire, path, decode_domain, applied_migrations)?;
    let decoded_memory_bytes = decoded_snapshot_memory_charge(path, &scenario, &policy.limits)?;
    add_retained_memory(
        path,
        retained_memory_bytes,
        decoded_memory_bytes.saturating_sub(parsed_memory_bytes.max(migrated_memory_bytes)),
        policy.limits.max_total_uncompressed_bytes,
    )?;
    Ok((scenario, requirements, version))
}

fn decode_portable_snapshot(
    wire: PortableScenario,
    path: &str,
    decode_domain: &impl Fn(&PortableScenario) -> Result<DecodedPortableDomain, MigrationFailure>,
    applied: &mut Vec<AppliedMigration>,
) -> Result<(ScenarioSnapshotV1, BTreeSet<SemanticCapability>), ImportError> {
    if wire.format != ScenarioFormat::EuthetoScenario
        || wire.schema_version != CURRENT_PORTABLE_SCHEMA_VERSION
        || wire
            .domain
            .payload
            .get("schemaVersion")
            .and_then(Value::as_u64)
            != Some(u64::from(wire.domain.schema_version))
    {
        return Err(ImportError::InvalidScenario {
            path: path.to_owned(),
            message: "invalid portable envelope or mismatched domain payload version".to_owned(),
        });
    }
    let decoded = decode_domain(&wire).map_err(ImportError::Migration)?;
    if decoded.document.scenario_id != wire.scenario_id
        || decoded.document.format != wire.format
        || decoded.document.metadata != wire.metadata
        || decoded.document.settings != wire.settings
        || decoded.document.domain_pack.id != wire.domain.pack_id
    {
        return Err(ImportError::Migration(MigrationFailure::Invalid(
            "domain decoding changed host-owned scenario fields".to_owned(),
        )));
    }
    if decoded.applied_migrations.is_empty()
        && decoded.required_capabilities != wire.domain.required_capabilities
    {
        return Err(ImportError::Migration(MigrationFailure::Invalid(
            "domain decoding changed requirements without a migration".to_owned(),
        )));
    }
    applied.extend(decoded.applied_migrations);
    let mut requirements = wire.required_capabilities.clone();
    requirements.extend(decoded.required_capabilities);
    Ok((
        ScenarioSnapshotV1 {
            format: wire.format,
            schema_version: SCENARIO_SNAPSHOT_SCHEMA_VERSION,
            revision: wire.revision,
            document: decoded.document,
            project: wire.project,
            required_capabilities: wire.required_capabilities,
            semantic_extensions: wire.semantic_extensions,
            extensions: wire.extensions,
        },
        requirements,
    ))
}

fn decoded_snapshot_memory_charge(
    path: &str,
    scenario: &ScenarioSnapshotV1,
    limits: &PortableLimits,
) -> Result<usize, ImportError> {
    let value = serde_json::to_value(scenario).map_err(|error| ImportError::InvalidScenario {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    validate_json_limits(path, &value, limits, 0)?;
    let encoded_bytes = canonical_json(&value)
        .map_err(|error| ImportError::Migration(MigrationFailure::Invalid(error.to_string())))?
        .len();
    if u64::try_from(encoded_bytes).map_or(true, |size| size > limits.max_json_bytes) {
        return Err(ImportError::JsonLimit {
            path: path.to_owned(),
            limit: "document bytes",
        });
    }
    conservative_parsed_memory_charge(
        path,
        encoded_bytes,
        retained_value_memory_charge(&value).ok_or_else(|| ImportError::JsonLimit {
            path: path.to_owned(),
            limit: "representation bytes",
        })?,
    )
}

fn inspect_logical_entries(
    entries: BTreeMap<String, Vec<u8>>,
    manifest: &mut BundleManifest,
    policy: &InspectionPolicy,
    registries: &MigrationRegistries,
    applied_migrations: &mut Vec<AppliedMigration>,
    initial_retained_memory_bytes: usize,
    decode_domain: &impl Fn(&PortableScenario) -> Result<DecodedPortableDomain, MigrationFailure>,
) -> Result<InspectedLogicalEntries, ImportError> {
    let mut scenarios = Vec::new();
    let mut scenario_revisions = Vec::new();
    let mut additional_entries = BTreeMap::new();
    let mut retained_memory_bytes = initial_retained_memory_bytes;
    let mut effective_requirements = BTreeSet::new();
    for (path, content) in entries {
        if path.starts_with("scenarios/") || path.starts_with("scenario-revisions/") {
            let historical = path.starts_with("scenario-revisions/");
            let (scenario, requirements, _) = inspect_scenario_entry(
                &path,
                &content,
                policy,
                registries,
                applied_migrations,
                &mut retained_memory_bytes,
                decode_domain,
            )?;
            manifest
                .required_capabilities
                .extend(requirements.iter().cloned());
            effective_requirements.extend(requirements);
            if historical {
                validate_historical_scenario(&path, &scenario, manifest, policy)?;
                scenario_revisions.push(scenario);
            } else {
                validate_scenario(&path, &scenario, manifest, policy)?;
                scenarios.push(scenario);
            }
        } else {
            if path.as_bytes().strip_suffix(b".json").is_some() {
                let _validated = parse_strict_json(&path, &content, &policy.limits)?;
            }
            let raw_memory_bytes = content
                .capacity()
                .checked_add(path.capacity())
                .and_then(|charge| charge.checked_add(256))
                .ok_or_else(|| ImportError::JsonLimit {
                    path: path.clone(),
                    limit: "representation bytes",
                })?;
            add_retained_memory(
                &path,
                &mut retained_memory_bytes,
                raw_memory_bytes,
                policy.limits.max_total_uncompressed_bytes,
            )?;
            additional_entries.insert(path, content);
        }
    }
    manifest.required_capabilities = effective_requirements;
    scenarios.sort_by_key(|scenario| scenario.document.scenario_id);
    scenario_revisions.sort_by_key(|scenario| (scenario.document.scenario_id, scenario.revision));
    Ok(InspectedLogicalEntries {
        scenarios,
        scenario_revisions,
        additional_entries,
        retained_memory_bytes,
    })
}
fn parse_and_validate_checksums(
    entries: &BTreeMap<String, Vec<u8>>,
    limits: &PortableLimits,
) -> Result<(Checksums, usize), ImportError> {
    let checksums_bytes = entries
        .get(CHECKSUMS_PATH)
        .ok_or_else(|| ImportError::MissingEntry(CHECKSUMS_PATH.to_owned()))?;
    let parsed_checksums = parse_strict_json_charged(CHECKSUMS_PATH, checksums_bytes, limits)?;
    let checksums_memory_bytes = conservative_parsed_memory_charge(
        CHECKSUMS_PATH,
        checksums_bytes.len(),
        parsed_checksums.representation_bytes,
    )?;
    let checksums_value = parsed_checksums.value;
    let checksums: Checksums = serde_json::from_value(checksums_value)
        .map_err(|error| ImportError::InvalidChecksums(error.to_string()))?;
    validate_checksums(&checksums, entries)?;
    Ok((checksums, checksums_memory_bytes))
}

struct ValidatedOuterBundle {
    manifest: BundleManifest,
    entries: BTreeMap<String, Vec<u8>>,
    manifest_memory_bytes: usize,
}

fn migrate_and_validate_outer_bundle(
    logical: LogicalBundle,
    original_format_version: u32,
    original_schema_version: u32,
    policy: &InspectionPolicy,
    registries: &MigrationRegistries,
    applied_migrations: &mut Vec<AppliedMigration>,
) -> Result<ValidatedOuterBundle, ImportError> {
    let LogicalBundle {
        manifest: manifest_value,
        mut entries,
    } = registries.migrate_outer(original_format_version, logical, applied_migrations)?;
    let manifest_memory_bytes = conservative_parsed_memory_charge(
        MANIFEST_PATH,
        0,
        retained_value_memory_charge(&manifest_value).ok_or_else(|| ImportError::JsonLimit {
            path: MANIFEST_PATH.to_owned(),
            limit: "representation bytes",
        })?,
    )?;
    let manifest: BundleManifest = serde_json::from_value(manifest_value)
        .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
    validate_manifest(&manifest, policy, registries)?;
    if manifest.schema_version != original_schema_version {
        return Err(ImportError::Migration(MigrationFailure::Invalid(
            "outer migration changed the global portable version".to_owned(),
        )));
    }
    validate_counts(&manifest, &entries)?;
    entries.remove(MANIFEST_PATH);
    entries.remove(CHECKSUMS_PATH);
    validate_portable_payloads(
        &manifest,
        &policy.limits,
        entries
            .iter()
            .map(|(path, content)| (path.as_str(), content.as_slice())),
    )
    .map_err(|error| ImportError::InvalidManifest(error.0))?;
    validate_source_requirements(&entries, &manifest, policy)?;
    Ok(ValidatedOuterBundle {
        manifest,
        entries,
        manifest_memory_bytes,
    })
}

/// Inspect bytes completely under limits. Nothing is extracted or made live.
///
/// # Errors
///
/// Returns [`ImportError`] when the input violates an archive, path, size,
/// checksum, schema, migration, or semantic-capability requirement, or when
/// ZIP or entry I/O fails.
pub fn inspect_bundle(
    bytes: &[u8],
    policy: &InspectionPolicy,
    registries: &MigrationRegistries,
    decode_domain: &impl Fn(&PortableScenario) -> Result<DecodedPortableDomain, MigrationFailure>,
) -> Result<InspectedBundle, ImportError> {
    let effective_policy = policy.hardened();
    let policy = &effective_policy;
    let archive_size = u64::try_from(bytes.len()).map_err(|_| ImportError::ArchiveTooLarge)?;
    if archive_size > policy.limits.max_archive_bytes {
        return Err(ImportError::ArchiveTooLarge);
    }
    validate_zip32_central_directory(
        bytes,
        policy.limits.max_entries,
        policy.limits.max_path_bytes,
    )?;
    validate_archive_metadata(bytes, policy)?;
    let (manifest_value, original_format_version, original_schema_version, manifest_memory_bytes) =
        preflight_manifest(bytes, policy, registries)?;

    let entries = read_archive_entries(bytes, policy)?;
    let (checksums, checksums_memory_bytes) =
        parse_and_validate_checksums(&entries, &policy.limits)?;

    let mut applied_migrations = Vec::new();
    let ValidatedOuterBundle {
        mut manifest,
        entries,
        manifest_memory_bytes: migrated_manifest_memory_bytes,
    } = migrate_and_validate_outer_bundle(
        LogicalBundle {
            manifest: manifest_value,
            entries,
        },
        original_format_version,
        original_schema_version,
        policy,
        registries,
        &mut applied_migrations,
    )?;
    let mut initial_retained_memory_bytes = 0_usize;
    add_retained_memory(
        MANIFEST_PATH,
        &mut initial_retained_memory_bytes,
        manifest_memory_bytes.max(migrated_manifest_memory_bytes),
        policy.limits.max_total_uncompressed_bytes,
    )?;
    add_retained_memory(
        CHECKSUMS_PATH,
        &mut initial_retained_memory_bytes,
        checksums_memory_bytes,
        policy.limits.max_total_uncompressed_bytes,
    )?;
    registries.record_global_portable_migrations(original_schema_version, &mut applied_migrations);
    let InspectedLogicalEntries {
        scenarios,
        scenario_revisions,
        additional_entries,
        retained_memory_bytes: mut logical_memory_bytes,
    } = inspect_logical_entries(
        entries,
        &mut manifest,
        policy,
        registries,
        &mut applied_migrations,
        initial_retained_memory_bytes,
        decode_domain,
    )?;
    validate_bundle_references(&scenarios, &scenario_revisions, &additional_entries)?;
    manifest.format_version = CURRENT_BUNDLE_FORMAT_VERSION;
    manifest.schema_version = CURRENT_PORTABLE_SCHEMA_VERSION;
    let effective_metadata = serde_json::to_value((&manifest, &applied_migrations))
        .map_err(|error| ImportError::Migration(MigrationFailure::Invalid(error.to_string())))?;
    validate_json_limits(MANIFEST_PATH, &effective_metadata[1], &policy.limits, 0)?;
    let metadata_charge = conservative_parsed_memory_charge(
        MANIFEST_PATH,
        0,
        retained_value_memory_charge(&effective_metadata).ok_or_else(|| {
            ImportError::JsonLimit {
                path: MANIFEST_PATH.to_owned(),
                limit: "representation bytes",
            }
        })?,
    )?;
    add_retained_memory(
        MANIFEST_PATH,
        &mut logical_memory_bytes,
        metadata_charge.saturating_sub(manifest_memory_bytes.max(migrated_manifest_memory_bytes)),
        policy.limits.max_total_uncompressed_bytes,
    )?;
    drop(effective_metadata);

    Ok(InspectedBundle {
        file_sha256: sha256_hex(bytes),
        manifest,
        checksums,
        scenarios,
        scenario_revisions,
        additional_entries,
        applied_migrations,
        original_format_version,
        original_schema_version,
        retained_memory_bytes: logical_memory_bytes,
    })
}

fn preserved_metadata_memory_charge(
    metadata: &PreservedBundleMetadata,
) -> Result<usize, ImportError> {
    let mut charge = size_of::<PreservedBundleMetadata>();
    for string in [
        Some(&metadata.file_sha256),
        Some(&metadata.format),
        metadata.bundle_kind.as_ref(),
        metadata.title.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        charge = charge
            .checked_add(string.capacity())
            .ok_or_else(|| ImportError::JsonLimit {
                path: MANIFEST_PATH.to_owned(),
                limit: "representation bytes",
            })?;
    }
    charge = charge
        .checked_add(
            metadata
                .required_capabilities
                .capacity()
                .checked_mul(size_of::<PreservedCapabilityMetadata>())
                .ok_or_else(|| ImportError::JsonLimit {
                    path: MANIFEST_PATH.to_owned(),
                    limit: "representation bytes",
                })?,
        )
        .and_then(|value| {
            value.checked_add(
                metadata
                    .scenarios
                    .capacity()
                    .checked_mul(size_of::<PreservedScenarioMetadata>())?,
            )
        })
        .ok_or_else(|| ImportError::JsonLimit {
            path: MANIFEST_PATH.to_owned(),
            limit: "representation bytes",
        })?;
    for capability in &metadata.required_capabilities {
        charge = charge
            .checked_add(capability.id.capacity())
            .ok_or_else(|| ImportError::JsonLimit {
                path: MANIFEST_PATH.to_owned(),
                limit: "representation bytes",
            })?;
    }
    for scenario in &metadata.scenarios {
        for string in [
            Some(&scenario.path),
            scenario.scenario_id.as_ref(),
            scenario.pack_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            charge =
                charge
                    .checked_add(string.capacity())
                    .ok_or_else(|| ImportError::JsonLimit {
                        path: scenario.path.clone(),
                        limit: "representation bytes",
                    })?;
        }
    }
    Ok(charge)
}

/// Performs a no-copy safety, checksum, and allowlisted metadata preflight.
///
/// Scenario domain payloads and all non-allowlisted fields are streamed through
/// `IgnoredAny`; they are never represented as [`Value`] or domain DTOs.
///
/// # Errors
///
/// Returns [`ImportError`] unless the archive and its fixed checksum contract
/// are valid and its allowlisted envelope metadata can be read within bounds.
pub fn preflight_bundle_metadata(
    bytes: &[u8],
    policy: &InspectionPolicy,
) -> Result<PreservedBundleMetadata, ImportError> {
    let effective_policy = policy.hardened();
    let policy = &effective_policy;
    let archive_size = u64::try_from(bytes.len()).map_err(|_| ImportError::ArchiveTooLarge)?;
    if archive_size > policy.limits.max_archive_bytes {
        return Err(ImportError::ArchiveTooLarge);
    }
    validate_zip32_central_directory(
        bytes,
        policy.limits.max_entries,
        policy.limits.max_path_bytes,
    )?;
    validate_archive_metadata(bytes, policy)?;

    let checksums_bytes = read_fixed_entry(bytes, CHECKSUMS_PATH, policy.limits.max_json_bytes)?;
    let checksums_value = parse_strict_json(CHECKSUMS_PATH, &checksums_bytes, &policy.limits)?;
    let checksums: Checksums = serde_json::from_value(checksums_value)
        .map_err(|error| ImportError::InvalidChecksums(error.to_string()))?;
    stream_validate_checksums(bytes, &checksums, policy)?;

    let manifest_bytes = read_fixed_entry(bytes, MANIFEST_PATH, MAX_MANIFEST_BYTES)?;
    let manifest = manifest_envelope_metadata(&manifest_bytes, &policy.limits)?;
    let mut scenarios = Vec::new();
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let path = std::str::from_utf8(entry.name_raw())
            .map_err(|_| ImportError::UnsafePath {
                path: String::new(),
                reason: "path is not valid UTF-8",
            })?
            .to_owned();
        // Archive paths are canonical: an uppercase extension is not a JSON entry.
        if !(path.starts_with("scenarios/") || path.starts_with("scenario-revisions/"))
            || path.as_bytes().strip_suffix(b".json").is_none()
        {
            continue;
        }
        if entry.size() > policy.limits.max_json_bytes {
            return Err(ImportError::JsonLimit {
                path,
                limit: "document bytes",
            });
        }
        let envelope: ScenarioEnvelopeMetadata = parse_metadata_json(&path, entry.by_ref())?;
        let (scenario_id, pack_id, internal_version, portable_version) =
            match envelope.schema_version {
                Some(0 | 1) => {
                    let document = envelope.document;
                    let scenario_id = document.as_ref().and_then(|doc| doc.scenario_id.clone());
                    let pack = document.and_then(|doc| doc.domain_pack);
                    let version = pack.as_ref().and_then(|pack| pack.schema_version);
                    (scenario_id, pack.and_then(|pack| pack.id), version, None)
                }
                Some(_) => {
                    let version = envelope
                        .domain
                        .as_ref()
                        .and_then(|domain| domain.schema_version);
                    (
                        envelope.scenario_id,
                        envelope.domain.and_then(|domain| domain.pack_id),
                        None,
                        version,
                    )
                }
                None => (None, None, None, None),
            };
        let scenario_id = scenario_id.filter(|value| value.len() <= policy.limits.max_string_bytes);
        let pack_id = pack_id
            .filter(|value| value.len() <= policy.limits.max_string_bytes)
            .and_then(|id| id.parse::<PackId>().ok())
            .map(|id| id.to_string());
        scenarios.push(PreservedScenarioMetadata {
            path,
            scenario_id,
            pack_id,
            internal_pack_schema_version: internal_version,
            portable_pack_schema_version: portable_version,
        });
    }
    scenarios.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(PreservedBundleMetadata {
        file_sha256: sha256_hex(bytes),
        format: manifest.format,
        format_version: manifest.format_version,
        portable_schema_version: manifest.schema_version,
        bundle_kind: manifest.bundle_kind,
        title: manifest.title,
        required_capabilities: manifest
            .required_capabilities
            .into_iter()
            .map(|capability| PreservedCapabilityMetadata {
                id: capability.id,
                version: capability.version,
            })
            .collect(),
        scenarios,
    })
}

/// Inspect only archive safety, fixed checksums, and allowlisted envelope metadata.
///
/// The returned archive remains unopened application data: no migration,
/// semantic-capability acceptance, pack lookup, scenario validation, or import
/// staging occurs. Exact original bytes are retained so an unavailable or newer
/// pack can be re-exported without reconstruction.
///
/// # Errors
///
/// Returns [`ImportError`] unless the archive is bounded, structurally safe,
/// uses the fixed checksum contract, and all declared entry bytes match it.
pub fn inspect_unopened_bundle_for_exact_reexport(
    bytes: &[u8],
    policy: &InspectionPolicy,
) -> Result<UnopenedBundle, ImportError> {
    let metadata = preflight_bundle_metadata(bytes, policy)?;
    let retained_memory_bytes = bytes
        .len()
        .checked_add(preserved_metadata_memory_charge(&metadata)?)
        .ok_or_else(|| ImportError::JsonLimit {
            path: "bundle".to_owned(),
            limit: "representation bytes",
        })?;
    Ok(UnopenedBundle {
        metadata,
        exact_bytes: bytes.to_vec().into_boxed_slice(),
        retained_memory_bytes,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum BundleIdentityOwner {
    ScenarioFamily(ScenarioId),
    Supplemental(SupplementalIdentity),
}

fn register_bundle_owned_uuid(
    owners: &mut BTreeMap<Uuid, BundleIdentityOwner>,
    identity: Uuid,
    owner: BundleIdentityOwner,
) -> Result<(), ImportError> {
    if let Some(existing) = owners.get(&identity) {
        if existing == &owner {
            return Ok(());
        }
        return Err(ImportError::InvalidManifest(format!(
            "owned identity {identity} is declared by both {existing:?} and {owner:?}"
        )));
    }
    owners.insert(identity, owner);
    Ok(())
}

fn validate_bundle_references(
    scenarios: &[ScenarioSnapshotV1],
    scenario_revisions: &[ScenarioSnapshotV1],
    entries: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ImportError> {
    for scenario in scenarios.iter().chain(scenario_revisions) {
        validate_scenario_owned_uuid_uniqueness(scenario).map_err(|error| {
            ImportError::InvalidManifest(
                eutheto_export::ExportError::InvalidModel(error.to_string()).to_string(),
            )
        })?;
    }
    validate_owned_identity_families(scenarios.iter().chain(scenario_revisions))?;
    let mut identity_owners = BTreeMap::new();
    for scenario in scenarios.iter().chain(scenario_revisions) {
        let owner = BundleIdentityOwner::ScenarioFamily(scenario.document.scenario_id);
        for identity in collect_scenario_owned_uuids(scenario) {
            register_bundle_owned_uuid(&mut identity_owners, identity, owner.clone())?;
        }
    }
    let mut current_revisions = BTreeMap::new();
    for scenario in scenarios {
        if current_revisions
            .insert(scenario.document.scenario_id, scenario.revision)
            .is_some()
        {
            return Err(ImportError::InvalidManifest(format!(
                "current scenario identity {} is duplicated",
                scenario.document.scenario_id
            )));
        }
    }
    let scenario_ids = current_revisions.keys().copied().collect::<BTreeSet<_>>();
    for historical in scenario_revisions {
        let identity = (historical.document.scenario_id, historical.revision);
        let Some(current_revision) = current_revisions.get(&identity.0) else {
            return Err(ImportError::InvalidManifest(format!(
                "historical revision references scenario {} absent from the bundle",
                identity.0
            )));
        };
        if historical.revision >= *current_revision {
            return Err(ImportError::InvalidManifest(format!(
                "historical revision {} for scenario {} must be less than current revision {}",
                historical.revision.value(),
                historical.document.scenario_id,
                current_revision.value()
            )));
        }
    }
    let represented = scenarios
        .iter()
        .chain(scenario_revisions)
        .map(|scenario| (scenario.document.scenario_id, scenario.revision))
        .collect::<BTreeSet<_>>();
    if represented.len() != scenarios.len().saturating_add(scenario_revisions.len()) {
        return Err(ImportError::InvalidManifest(
            "scenario revision identities are duplicated".to_owned(),
        ));
    }
    let available_assets = entries
        .keys()
        .filter_map(|path| path.strip_prefix("assets/"))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    for scenario in scenarios.iter().chain(scenario_revisions) {
        let value = serde_json::to_value(scenario)
            .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
        validate_asset_references(&value, &available_assets, "scenario")?;
        let references = extract_scenario_references(&value)
            .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
        if !references.is_subset(&scenario_ids) {
            return Err(ImportError::InvalidManifest(
                "scenario references an identity absent from the bundle".to_owned(),
            ));
        }
    }
    validate_supplemental_bundle_references(
        entries,
        &available_assets,
        &scenario_ids,
        &represented,
        &mut identity_owners,
    )
}

fn validate_supplemental_bundle_references(
    entries: &BTreeMap<String, Vec<u8>>,
    available_assets: &BTreeSet<String>,
    scenario_ids: &BTreeSet<ScenarioId>,
    represented: &BTreeSet<(ScenarioId, Revision)>,
    identity_owners: &mut BTreeMap<Uuid, BundleIdentityOwner>,
) -> Result<(), ImportError> {
    for (path, bytes) in entries {
        let Some((section, key)) = path.split_once('/') else {
            continue;
        };
        let section_kind = match section {
            "results" => SupplementalSectionKind::Results,
            "shared" => SupplementalSectionKind::SharedRecords,
            "preferences" => SupplementalSectionKind::Preferences,
            "assets" => SupplementalSectionKind::Assets,
            _ => continue,
        };
        if section_kind == SupplementalSectionKind::Assets {
            register_supplemental_uuid_stem(identity_owners, section_kind, key)?;
            continue;
        }
        let value = parse_strict_json(path, bytes, &PORTABLE_LIMITS)?;
        validate_asset_references(&value, available_assets, path)?;
        if section_kind == SupplementalSectionKind::Results {
            validate_result_bundle_reference(path, &value, represented, identity_owners)?;
            register_supplemental_value_definitions(identity_owners, section_kind, key, &value)?;
            continue;
        }
        register_supplemental_uuid_stem(identity_owners, section_kind, key)?;
        register_supplemental_value_definitions(identity_owners, section_kind, key, &value)?;
        let references = extract_scenario_references(&value).map_err(|error| {
            ImportError::InvalidManifest(format!("{path} has invalid scenario reference: {error}"))
        })?;
        if let Some(missing) = references.difference(scenario_ids).next() {
            return Err(ImportError::InvalidManifest(format!(
                "{path} references scenario {missing} absent from the bundle"
            )));
        }
    }
    Ok(())
}

fn register_supplemental_uuid_stem(
    identity_owners: &mut BTreeMap<Uuid, BundleIdentityOwner>,
    section: SupplementalSectionKind,
    key: &str,
) -> Result<(), ImportError> {
    let identity = SupplementalIdentity {
        section,
        key: key.to_owned(),
    };
    if let Some(uuid) = supplemental_identity_uuid(&identity) {
        register_bundle_owned_uuid(
            identity_owners,
            uuid,
            BundleIdentityOwner::Supplemental(identity),
        )?;
    }
    Ok(())
}

fn register_supplemental_value_definitions(
    identity_owners: &mut BTreeMap<Uuid, BundleIdentityOwner>,
    section: SupplementalSectionKind,
    key: &str,
    value: &Value,
) -> Result<(), ImportError> {
    let owner = BundleIdentityOwner::Supplemental(SupplementalIdentity {
        section,
        key: key.to_owned(),
    });
    for identity in collect_self_declared_uuids(value) {
        register_bundle_owned_uuid(identity_owners, identity, owner.clone())?;
    }
    Ok(())
}

fn validate_result_bundle_reference(
    path: &str,
    value: &Value,
    represented: &BTreeSet<(ScenarioId, Revision)>,
    identity_owners: &mut BTreeMap<Uuid, BundleIdentityOwner>,
) -> Result<(), ImportError> {
    let result_id = extract_result_id(value).map_err(|error| {
        ImportError::InvalidManifest(format!("{path} has invalid result identity: {error}"))
    })?;
    let identity = SupplementalIdentity {
        section: SupplementalSectionKind::Results,
        key: format!("{result_id}.json"),
    };
    register_bundle_owned_uuid(
        identity_owners,
        result_id,
        BundleIdentityOwner::Supplemental(identity),
    )?;
    let canonical_path = format!("results/{result_id}.json");
    if path != canonical_path {
        return Err(ImportError::InvalidManifest(format!(
            "{path} does not match canonical result identity {canonical_path}"
        )));
    }
    let dependency = extract_result_dependency(value).map_err(|error| {
        ImportError::InvalidManifest(format!("{path} has invalid result dependency: {error}"))
    })?;
    if !represented.contains(&(dependency.scenario_id, dependency.scenario_revision)) {
        return Err(ImportError::InvalidManifest(format!(
            "{path} requires scenario {} at exact revision {}",
            dependency.scenario_id,
            dependency.scenario_revision.value()
        )));
    }
    Ok(())
}
fn validate_owned_identity_families<'a>(
    scenarios: impl IntoIterator<Item = &'a ScenarioSnapshotV1>,
) -> Result<(), ImportError> {
    let mut owners = BTreeMap::new();
    for scenario in scenarios {
        let scenario_id = scenario.document.scenario_id;
        for identity in collect_scenario_owned_uuids(scenario) {
            if let Some(owner) = owners.insert(identity, scenario_id)
                && owner != scenario_id
            {
                return Err(ImportError::InvalidManifest(format!(
                    "owned identity {identity} is declared by scenario families {owner} and {scenario_id}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_asset_references(
    value: &Value,
    available: &BTreeSet<String>,
    context: &str,
) -> Result<(), ImportError> {
    let references = extract_asset_references(value)
        .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
    if let Some(missing) = references.difference(available).next() {
        return Err(ImportError::InvalidManifest(format!(
            "{context} references absent asset {missing}"
        )));
    }
    Ok(())
}

fn validate_checksums(
    checksums: &Checksums,
    entries: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ImportError> {
    if checksums.algorithm != CHECKSUM_ALGORITHM {
        return Err(ImportError::InvalidChecksums(
            "only SHA-256 is supported".to_owned(),
        ));
    }
    if checksums.files.contains_key(CHECKSUMS_PATH) {
        return Err(ImportError::InvalidChecksums(
            "checksums.json cannot checksum itself".to_owned(),
        ));
    }
    for (path, content) in entries {
        if path == CHECKSUMS_PATH {
            continue;
        }
        let expected = checksums
            .files
            .get(path)
            .ok_or_else(|| ImportError::MissingChecksum(path.clone()))?;
        if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ImportError::InvalidChecksums(format!(
                "invalid SHA-256 for {path}"
            )));
        }
        if sha256_hex(content) != expected.to_ascii_lowercase() {
            return Err(ImportError::ChecksumMismatch(path.clone()));
        }
    }
    for path in checksums.files.keys() {
        if !entries.contains_key(path) {
            return Err(ImportError::UndeclaredEntry(path.clone()));
        }
    }
    Ok(())
}

fn validate_manifest(
    manifest: &BundleManifest,
    policy: &InspectionPolicy,
    registries: &MigrationRegistries,
) -> Result<(), ImportError> {
    if manifest.format != BUNDLE_FORMAT {
        return Err(ImportError::InvalidManifest(format!(
            "format must be {BUNDLE_FORMAT}"
        )));
    }
    if manifest.bundle_id.as_uuid().get_version_num() != 7 {
        return Err(ImportError::InvalidManifest(
            "bundle identity must be UUIDv7".to_owned(),
        ));
    }
    if !manifest.created_at.ends_with('Z') || Rfc3339Timestamp::parse(&manifest.created_at).is_err()
    {
        return Err(ImportError::InvalidManifest(
            "creation time must be a valid UTC RFC 3339 timestamp".to_owned(),
        ));
    }
    if manifest.application.name.is_empty()
        || manifest.application.version.is_empty()
        || manifest.title.is_empty()
    {
        return Err(ImportError::InvalidManifest(
            "application metadata and bundle title must be nonempty".to_owned(),
        ));
    }
    validate_portable_json_value(
        &serde_json::to_value(manifest)
            .map_err(|error| ImportError::InvalidManifest(error.to_string()))?,
        &policy.limits,
        0,
    )
    .map_err(|error| ImportError::InvalidManifest(error.0))?;
    if manifest.format_version != CURRENT_BUNDLE_FORMAT_VERSION {
        return Err(ImportError::InvalidManifest(
            "outer migration did not produce the current format".to_owned(),
        ));
    }
    if manifest.schema_version > CURRENT_PORTABLE_SCHEMA_VERSION {
        return Err(ImportError::UnsupportedNewerVersion {
            space: VersionSpace::PortableSchema,
            found: manifest.schema_version,
            current: CURRENT_PORTABLE_SCHEMA_VERSION,
        });
    }
    if !registries.supports_portable_version(manifest.schema_version) {
        return Err(ImportError::UnsupportedOlderVersion {
            space: VersionSpace::PortableSchema,
            found: manifest.schema_version,
            current: CURRENT_PORTABLE_SCHEMA_VERSION,
        });
    }
    if !matches!(
        manifest.bundle_kind,
        BundleKind::ScenarioExport | BundleKind::FullBackup
    ) {
        return Err(ImportError::UnsupportedKind);
    }
    if manifest.integrity.algorithm != CHECKSUM_ALGORITHM
        || manifest.integrity.checksums_file != CHECKSUMS_PATH
    {
        return Err(ImportError::InvalidManifest(
            "integrity declaration is not the fixed SHA-256 checksums file".to_owned(),
        ));
    }
    for capability in &manifest.required_capabilities {
        if capability.version == 0 || !valid_namespace(&capability.id) {
            return Err(ImportError::InvalidManifest(
                "a required capability declaration is invalid".to_owned(),
            ));
        }
    }
    for namespace in &manifest.nonsemantic_extensions {
        if !valid_namespace(namespace) {
            return Err(ImportError::InvalidManifest(
                "a nonsemantic extension namespace is invalid".to_owned(),
            ));
        }
    }
    Ok(())
}

fn valid_namespace(namespace: &str) -> bool {
    namespace.contains('.')
        && namespace.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

fn require_capability(
    capability: &SemanticCapability,
    policy: &InspectionPolicy,
) -> Result<(), ImportError> {
    if capability.version == 0 || !valid_namespace(&capability.id) {
        return Err(ImportError::InvalidManifest(
            "a required capability declaration is invalid".to_owned(),
        ));
    }
    if policy
        .supported_capabilities
        .get(&capability.id)
        .is_none_or(|supported| *supported < capability.version)
    {
        return Err(ImportError::UnsupportedCapability {
            id: capability.id.clone(),
            version: capability.version,
        });
    }
    Ok(())
}

fn validate_scenario(
    path: &str,
    scenario: &ScenarioSnapshotV1,
    manifest: &BundleManifest,
    policy: &InspectionPolicy,
) -> Result<(), ImportError> {
    if scenario.format != ScenarioFormat::EuthetoScenario
        || scenario.schema_version != SCENARIO_SNAPSHOT_SCHEMA_VERSION
    {
        return Err(ImportError::InvalidScenario {
            path: path.to_owned(),
            message: "migration did not produce the current portable scenario".to_owned(),
        });
    }
    if (manifest.bundle_kind == BundleKind::ScenarioExport && scenario.project.is_some())
        || (manifest.bundle_kind == BundleKind::FullBackup && scenario.project.is_none())
    {
        return Err(ImportError::InvalidScenario {
            path: path.to_owned(),
            message: "project wrapper metadata does not match bundle kind".to_owned(),
        });
    }
    let expected_path = format!("scenarios/{}.json", scenario.document.scenario_id);
    if path != expected_path {
        return Err(ImportError::InvalidScenario {
            path: path.to_owned(),
            message: "scenario filename does not match its stable identity".to_owned(),
        });
    }
    validate_scenario_contents(path, scenario, manifest, policy)
}

fn validate_historical_scenario(
    path: &str,
    scenario: &ScenarioSnapshotV1,
    manifest: &BundleManifest,
    policy: &InspectionPolicy,
) -> Result<(), ImportError> {
    if scenario.project.is_some() {
        return Err(ImportError::InvalidScenario {
            path: path.to_owned(),
            message: "historical scenario revisions cannot contain project metadata".to_owned(),
        });
    }
    let expected_path = format!(
        "scenario-revisions/{}-{}.json",
        scenario.document.scenario_id,
        scenario.revision.value()
    );
    if path != expected_path {
        return Err(ImportError::InvalidScenario {
            path: path.to_owned(),
            message: "historical revision filename does not match its identity and revision"
                .to_owned(),
        });
    }
    validate_scenario_contents(path, scenario, manifest, policy)
}

fn validate_scenario_contents(
    path: &str,
    scenario: &ScenarioSnapshotV1,
    manifest: &BundleManifest,
    policy: &InspectionPolicy,
) -> Result<(), ImportError> {
    validate_decoded_scenario(path, scenario, policy)?;
    for capability in &scenario.required_capabilities {
        if !manifest.required_capabilities.contains(capability) {
            return Err(ImportError::InvalidScenario {
                path: path.to_owned(),
                message: format!(
                    "required capability {} is absent from the manifest",
                    capability.id
                ),
            });
        }
    }
    for namespace in scenario
        .extensions
        .keys()
        .chain(scenario.document.extensions.keys())
    {
        if !manifest.nonsemantic_extensions.contains(namespace) {
            return Err(ImportError::InvalidScenario {
                path: path.to_owned(),
                message: format!("nonsemantic extension {namespace} is undeclared"),
            });
        }
    }
    Ok(())
}

fn validate_decoded_scenario(
    path: &str,
    scenario: &ScenarioSnapshotV1,
    policy: &InspectionPolicy,
) -> Result<(), ImportError> {
    if scenario.format != ScenarioFormat::EuthetoScenario
        || scenario.schema_version != SCENARIO_SNAPSHOT_SCHEMA_VERSION
    {
        return Err(ImportError::InvalidScenario {
            path: path.to_owned(),
            message: "migration did not produce the current portable scenario".to_owned(),
        });
    }
    for capability in &scenario.required_capabilities {
        require_capability(capability, policy)?;
    }
    validate_scenario_snapshot(scenario).map_err(|error| ImportError::InvalidScenario {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    validate_document_shape(&scenario.document).map_err(|error| ImportError::InvalidScenario {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    for namespace in scenario.semantic_extensions.keys() {
        if !scenario
            .required_capabilities
            .iter()
            .any(|capability| capability.id == *namespace)
        {
            return Err(ImportError::UnsupportedCapability {
                id: namespace.clone(),
                version: 0,
            });
        }
    }
    Ok(())
}

fn validate_counts(
    manifest: &BundleManifest,
    entries: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ImportError> {
    let count = |prefix: &str| -> Result<u64, ImportError> {
        u64::try_from(
            entries
                .keys()
                .filter(|path| path.starts_with(prefix))
                .count(),
        )
        .map_err(|_| ImportError::TotalSizeExceeded)
    };
    if count("scenarios/")? != manifest.counts.scenarios {
        return Err(ImportError::CountMismatch("scenarios"));
    }
    if count("scenario-revisions/")? != manifest.counts.scenario_revisions {
        return Err(ImportError::CountMismatch("scenario revisions"));
    }
    if count("results/")? != manifest.counts.results {
        return Err(ImportError::CountMismatch("results"));
    }
    if count("shared/")? != manifest.counts.shared_records {
        return Err(ImportError::CountMismatch("shared records"));
    }
    if count("preferences/")? != manifest.counts.preferences {
        return Err(ImportError::CountMismatch("preferences"));
    }
    if count("assets/")? != manifest.counts.assets {
        return Err(ImportError::CountMismatch("assets"));
    }
    for path in entries.keys() {
        if path == MANIFEST_PATH || path == CHECKSUMS_PATH {
            continue;
        }
        if ![
            "scenarios/",
            "scenario-revisions/",
            "results/",
            "shared/",
            "preferences/",
            "assets/",
        ]
        .iter()
        .any(|prefix| path.starts_with(prefix))
        {
            return Err(ImportError::UndeclaredEntry(path.clone()));
        }
    }
    if manifest.bundle_kind == BundleKind::ScenarioExport && manifest.counts.scenarios != 1 {
        return Err(ImportError::InvalidManifest(
            "scenario export must contain exactly one scenario".to_owned(),
        ));
    }
    Ok(())
}

fn normalize_path(path: &str, max_bytes: usize) -> Result<String, ImportError> {
    validate_portable_path(path, max_bytes).map_err(|error| ImportError::UnsafePath {
        path: path.to_owned(),
        reason: match error.0.as_str() {
            "non-ASCII portable path" => "portable paths must be ASCII",
            _ => "path violates portable normalization policy",
        },
    })?;
    Ok(path.to_owned())
}

fn validate_regular_mode(mode: Option<u32>, path: &str) -> Result<(), ImportError> {
    if let Some(mode) = mode {
        let file_type = mode & 0o170_000;
        if file_type != 0 && file_type != 0o100_000 {
            return Err(ImportError::NonRegularEntry {
                path: path.to_owned(),
            });
        }
        if mode & 0o111 != 0 {
            return Err(ImportError::ProhibitedContent {
                path: path.to_owned(),
            });
        }
    }
    Ok(())
}

fn reject_prohibited_content(path: &str, bytes: &[u8]) -> Result<(), ImportError> {
    reject_prohibited_portable_content(path, bytes).map_err(|_| ImportError::ProhibitedContent {
        path: path.to_owned(),
    })
}

fn required_u32(value: &Value, field: &str, path: &str) -> Result<u32, ImportError> {
    let number = value
        .as_object()
        .and_then(|object| object.get(field))
        .and_then(Value::as_u64)
        .ok_or_else(|| ImportError::InvalidJson {
            path: path.to_owned(),
            message: format!("missing unsigned integer field {field}"),
        })?;
    u32::try_from(number).map_err(|_| ImportError::InvalidJson {
        path: path.to_owned(),
        message: format!("field {field} exceeds u32"),
    })
}

const JSON_LIMIT_MARKER: &str = "__eutheto_json_limit__:";

struct ParsedJson {
    value: Value,
    representation_bytes: usize,
}

struct JsonBudget {
    limits: PortableLimits,
    nodes: usize,
    items: usize,
    string_bytes: usize,
    representation_bytes: usize,
    max_representation_bytes: usize,
}

impl JsonBudget {
    fn new(limits: &PortableLimits) -> Result<Self, &'static str> {
        let json_bytes =
            usize::try_from(limits.max_json_bytes).map_err(|_| "representation bytes")?;
        let max_representation_bytes = json_bytes
            .checked_mul(64)
            .and_then(|value| value.checked_add(4_096))
            .ok_or("representation bytes")?;
        Ok(Self {
            limits: *limits,
            nodes: 0,
            items: 0,
            string_bytes: 0,
            representation_bytes: 0,
            max_representation_bytes,
        })
    }

    fn add_representation<E: de::Error>(&mut self, bytes: usize) -> Result<(), E> {
        self.representation_bytes = self
            .representation_bytes
            .checked_add(bytes)
            .ok_or_else(|| E::custom(format_args!("{JSON_LIMIT_MARKER}representation bytes")))?;
        if self.representation_bytes > self.max_representation_bytes {
            return Err(E::custom(format_args!(
                "{JSON_LIMIT_MARKER}representation bytes"
            )));
        }
        Ok(())
    }

    fn enter_node<E: de::Error>(&mut self, depth: usize) -> Result<(), E> {
        if depth > self.limits.max_json_depth {
            return Err(E::custom(format_args!("{JSON_LIMIT_MARKER}nesting depth")));
        }
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or_else(|| E::custom(format_args!("{JSON_LIMIT_MARKER}aggregate nodes")))?;
        if self.nodes > self.limits.max_collection_items {
            return Err(E::custom(format_args!(
                "{JSON_LIMIT_MARKER}aggregate nodes"
            )));
        }
        self.add_representation::<E>(size_of::<Value>())
    }

    fn add_item<E: de::Error>(&mut self, container_items: usize) -> Result<(), E> {
        if container_items >= self.limits.max_collection_items {
            return Err(E::custom(format_args!(
                "{JSON_LIMIT_MARKER}container items"
            )));
        }
        self.items = self
            .items
            .checked_add(1)
            .ok_or_else(|| E::custom(format_args!("{JSON_LIMIT_MARKER}aggregate items")))?;
        if self.items > self.limits.max_collection_items {
            return Err(E::custom(format_args!(
                "{JSON_LIMIT_MARKER}aggregate items"
            )));
        }
        // A deliberately conservative allowance for map links, allocator metadata,
        // and collection spare capacity. Preview accounting reuses the resulting
        // checked representation charge rather than compact JSON length.
        self.add_representation::<E>(96)
    }

    fn add_string<E: de::Error>(&mut self, bytes: usize, key: bool) -> Result<(), E> {
        if bytes > self.limits.max_string_bytes {
            let limit = if key {
                "object key bytes"
            } else {
                "string bytes"
            };
            return Err(E::custom(format_args!("{JSON_LIMIT_MARKER}{limit}")));
        }
        self.string_bytes = self
            .string_bytes
            .checked_add(bytes)
            .ok_or_else(|| E::custom(format_args!("{JSON_LIMIT_MARKER}aggregate string bytes")))?;
        if u64::try_from(self.string_bytes).map_or(true, |total| total > self.limits.max_json_bytes)
        {
            return Err(E::custom(format_args!(
                "{JSON_LIMIT_MARKER}aggregate string bytes"
            )));
        }
        self.add_representation::<E>(bytes)
    }

    fn visit_value(&mut self, value: &Value, depth: usize) -> Result<(), serde_json::Error> {
        self.enter_node::<serde_json::Error>(depth)?;
        match value {
            Value::String(value) => self.add_string::<serde_json::Error>(value.len(), false)?,
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    self.add_item::<serde_json::Error>(index)?;
                    self.visit_value(value, depth.saturating_add(1))?;
                }
            }
            Value::Object(values) => {
                for (index, (key, value)) in values.iter().enumerate() {
                    self.add_string::<serde_json::Error>(key.len(), true)?;
                    self.add_item::<serde_json::Error>(index)?;
                    self.visit_value(value, depth.saturating_add(1))?;
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
        Ok(())
    }
}

fn json_limit_from_error(path: &str, message: &str) -> Option<ImportError> {
    let marker = message.find(JSON_LIMIT_MARKER)?;
    let limit = &message[marker + JSON_LIMIT_MARKER.len()..];
    let limit = limit.split(" at line").next().unwrap_or(limit);
    let limit = match limit {
        "nesting depth" => "nesting depth",
        "aggregate nodes" => "aggregate nodes",
        "container items" => "container items",
        "aggregate items" => "aggregate items",
        "string bytes" => "string bytes",
        "object key bytes" => "object key bytes",
        "aggregate string bytes" => "aggregate string bytes",
        "representation bytes" => "representation bytes",
        _ => return None,
    };
    Some(ImportError::JsonLimit {
        path: path.to_owned(),
        limit,
    })
}

fn parse_strict_json_charged(
    path: &str,
    bytes: &[u8],
    limits: &PortableLimits,
) -> Result<ParsedJson, ImportError> {
    if u64::try_from(bytes.len()).map_or(true, |size| size > limits.max_json_bytes) {
        return Err(ImportError::JsonLimit {
            path: path.to_owned(),
            limit: "document bytes",
        });
    }
    let mut budget = JsonBudget::new(limits).map_err(|limit| ImportError::JsonLimit {
        path: path.to_owned(),
        limit,
    })?;
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValueSeed {
        budget: &mut budget,
        depth: 0,
    }
    .deserialize(&mut deserializer)
    .map_err(|error| {
        let message = error.to_string();
        json_limit_from_error(path, &message).unwrap_or_else(|| ImportError::InvalidJson {
            path: path.to_owned(),
            message,
        })
    })?;
    deserializer
        .end()
        .map_err(|error| ImportError::InvalidJson {
            path: path.to_owned(),
            message: error.to_string(),
        })?;
    validate_json_limits(path, &value, limits, 0)?;
    validate_portable_json_value(&value, limits, 0).map_err(|error| ImportError::InvalidJson {
        path: path.to_owned(),
        message: error.0,
    })?;
    Ok(ParsedJson {
        value,
        representation_bytes: budget.representation_bytes,
    })
}

fn parse_strict_json(
    path: &str,
    bytes: &[u8],
    limits: &PortableLimits,
) -> Result<Value, ImportError> {
    parse_strict_json_charged(path, bytes, limits).map(|parsed| parsed.value)
}

fn validate_json_limits(
    path: &str,
    value: &Value,
    limits: &PortableLimits,
    depth: usize,
) -> Result<(), ImportError> {
    let mut budget = JsonBudget::new(limits).map_err(|limit| ImportError::JsonLimit {
        path: path.to_owned(),
        limit,
    })?;
    budget.visit_value(value, depth).map_err(|error| {
        let message = error.to_string();
        json_limit_from_error(path, &message).unwrap_or_else(|| ImportError::InvalidJson {
            path: path.to_owned(),
            message,
        })
    })
}

struct StrictValueSeed<'a> {
    budget: &'a mut JsonBudget,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for StrictValueSeed<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        self.budget.enter_node::<D::Error>(self.depth)?;
        deserializer.deserialize_any(StrictValueVisitor {
            budget: self.budget,
            depth: self.depth,
        })
    }
}

struct StrictValueVisitor<'a> {
    budget: &'a mut JsonBudget,
    depth: usize,
}

impl<'de> Visitor<'de> for StrictValueVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.budget.add_string::<E>(value.len(), false)?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.budget.add_string::<E>(value.len(), false)?;
        Ok(Value::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let depth = self.depth.checked_add(1).ok_or_else(|| {
            <A::Error as de::Error>::custom(format_args!("{JSON_LIMIT_MARKER}nesting depth"))
        })?;
        let budget = self.budget;
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(StrictValueSeed {
            budget: &mut *budget,
            depth,
        })? {
            budget.add_item::<A::Error>(values.len())?;
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let depth = self.depth.checked_add(1).ok_or_else(|| {
            <A::Error as de::Error>::custom(format_args!("{JSON_LIMIT_MARKER}nesting depth"))
        })?;
        let budget = self.budget;
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            budget.add_string::<A::Error>(key.len(), true)?;
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate object key"));
            }
            budget.add_item::<A::Error>(values.len())?;
            let value = object.next_value_seed(StrictValueSeed {
                budget: &mut *budget,
                depth,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreMode {
    ImportScenario,
    AddBackup,
    ReplaceLibrary,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportOptions {
    pub restore_mode: RestoreMode,
    pub include_results: bool,
    pub include_assets: bool,
}

#[derive(Clone, Debug)]
pub struct LocalLibrarySnapshot {
    pub revision: Revision,
    pub scenario_ids: BTreeSet<ScenarioId>,
    /// Highest revision ever committed for each scenario identity, including tombstones.
    pub scenario_revision_high_water: BTreeMap<ScenarioId, Revision>,
    /// Durable ownership of every scenario-family UUID, including removed IDs.
    pub identity_owners: BTreeMap<Uuid, ScenarioId>,
    /// Every local occupied UUID, encoded canonically for collision allocation.
    pub occupied_uuids: BTreeSet<String>,
    pub scenarios: Vec<LocalScenarioSnapshot>,
    pub supplemental_identities: BTreeSet<SupplementalIdentity>,
    /// Durable UUID ownership for each currently stored supplemental record.
    pub supplemental_identity_owners: BTreeMap<Uuid, SupplementalIdentity>,
    pub settings: BTreeMap<String, Value>,
}

/// Redaction-safe local project metadata displayed before replace-library removal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalScenarioSnapshot {
    pub scenario_id: ScenarioId,
    pub title: String,
    pub revision: Revision,
    pub archived: bool,
    /// Scenario-family UUID ownership used to authorize safe replacement.
    pub owned_uuids: BTreeSet<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewBinding {
    pub file_sha256: String,
    pub options_sha256: String,
    pub local_library_revision: Revision,
    pub format_version: u32,
    pub schema_version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScenarioPreview {
    pub scenario_id: ScenarioId,
    pub title: String,
    pub source_revision: Revision,
    pub same_identity_revision: Revision,
    pub same_identity_revision_warning: Option<String>,
    pub collides: bool,
}

#[derive(Clone, Debug)]
pub struct ImportPreview {
    pub binding: PreviewBinding,
    pub bundle_id: BundleId,
    pub bundle_kind: BundleKind,
    pub title: String,
    pub created_at: String,
    pub source_application: ApplicationMetadata,
    pub source_format_version: u32,
    pub source_schema_version: u32,
    pub counts: eutheto_export::BundleCounts,
    pub required_capabilities: BTreeSet<SemanticCapability>,
    pub preserved_extensions: BTreeSet<String>,
    pub source_backup_selection: Option<BackupSelection>,
    pub omitted_assets: BTreeMap<String, OmittedAssetPlaceholder>,
    pub included_sections: BTreeSet<String>,
    pub excluded_sections: BTreeSet<String>,
    pub scenarios: Vec<ScenarioPreview>,
    pub supplemental_collisions: Vec<SupplementalIdentity>,
    pub removed_scenarios: Vec<LocalScenarioSnapshot>,
    pub removed_supplemental: Vec<SupplementalIdentity>,
    pub applied_migrations: Vec<AppliedMigration>,
    pub settings_changed: Vec<String>,
    pub settings_removed: Vec<String>,
}

fn same_identity_revision(
    scenario: &ScenarioSnapshotV1,
    local: &LocalLibrarySnapshot,
) -> Result<(Revision, Option<String>), ImportError> {
    let scenario_id = scenario.document.scenario_id;
    let floor = local
        .scenario_revision_high_water
        .get(&scenario_id)
        .copied()
        .into_iter()
        .chain(
            local
                .scenarios
                .iter()
                .filter(|current| current.scenario_id == scenario_id)
                .map(|current| current.revision),
        )
        .max();
    let Some(floor) = floor else {
        return Ok((scenario.revision, None));
    };
    if scenario.revision > floor {
        return Ok((scenario.revision, None));
    }
    let same_identity_revision = floor
        .checked_next()
        .map_err(|_| ImportError::RevisionOverflow { scenario_id })?;
    Ok((
        same_identity_revision,
        Some(format!(
            "source revision {} will advance to {} because local history reached revision {}",
            scenario.revision.value(),
            same_identity_revision.value(),
            floor.value()
        )),
    ))
}

/// Builds the deterministic preview bound to the inspected file, selected
/// options, and current local-library revision.
///
/// # Errors
///
/// Returns [`ImportError::InvalidManifest`] if the import options cannot be
/// serialized canonically for the preview binding.
pub fn build_preview(
    inspected: &InspectedBundle,
    options: &ImportOptions,
    local: &LocalLibrarySnapshot,
) -> Result<ImportPreview, ImportError> {
    validate_replace_library_identity_ownership(inspected, options, local)?;
    let options_bytes =
        canonical_json(options).map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
    let scenarios = build_scenario_previews(inspected, options, local)?;
    let mut included_sections = BTreeSet::from([
        "scenarios".to_owned(),
        "shared-records".to_owned(),
        "preferences".to_owned(),
    ]);
    let mut excluded_sections = BTreeSet::new();
    let source_backup_selection = backup_selection_from_manifest(&inspected.manifest)
        .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
    let source_includes_results = source_backup_selection
        .as_ref()
        .is_none_or(|selection| selection.include_results);
    if options.include_results && source_includes_results {
        included_sections.insert("results".to_owned());
    } else {
        excluded_sections.insert("results".to_owned());
    }
    if options.include_assets {
        match source_backup_selection
            .as_ref()
            .map(|selection| selection.asset_selection)
        {
            Some(PortableBackupAssetSelection::ExcludeAll) => {
                excluded_sections.insert("assets".to_owned());
            }
            Some(PortableBackupAssetSelection::V1Threshold) => {
                included_sections.insert("assets".to_owned());
                excluded_sections.insert("assets-above-v1-threshold".to_owned());
            }
            Some(PortableBackupAssetSelection::All) | None => {
                included_sections.insert("assets".to_owned());
            }
        }
    } else {
        excluded_sections.insert("assets".to_owned());
    }
    let removed_scenarios = if options.restore_mode == RestoreMode::ReplaceLibrary {
        local.scenarios.clone()
    } else {
        Vec::new()
    };
    let removed_supplemental = if options.restore_mode == RestoreMode::ReplaceLibrary {
        local.supplemental_identities.iter().cloned().collect()
    } else {
        Vec::new()
    };
    Ok(ImportPreview {
        binding: PreviewBinding {
            file_sha256: inspected.file_sha256.clone(),
            options_sha256: sha256_hex(&options_bytes),
            local_library_revision: local.revision,
            format_version: inspected.manifest.format_version,
            schema_version: inspected.manifest.schema_version,
        },
        bundle_id: inspected.manifest.bundle_id,
        bundle_kind: inspected.manifest.bundle_kind,
        title: inspected.manifest.title.clone(),
        created_at: inspected.manifest.created_at.clone(),
        source_application: inspected.manifest.application.clone(),
        source_format_version: inspected.original_format_version,
        source_schema_version: inspected.original_schema_version,
        counts: inspected.manifest.counts.clone(),
        required_capabilities: inspected.manifest.required_capabilities.clone(),
        preserved_extensions: inspected.manifest.nonsemantic_extensions.clone(),
        source_backup_selection,
        omitted_assets: build_omitted_asset_preview(inspected, options)?,
        included_sections,
        excluded_sections,
        scenarios,
        removed_scenarios,
        supplemental_collisions: build_supplemental_collision_preview(inspected, options, local)?,
        applied_migrations: inspected.applied_migrations.clone(),
        removed_supplemental,
        settings_changed: Vec::new(),
        settings_removed: Vec::new(),
    })
}

fn build_scenario_previews(
    inspected: &InspectedBundle,
    options: &ImportOptions,
    local: &LocalLibrarySnapshot,
) -> Result<Vec<ScenarioPreview>, ImportError> {
    inspected
        .scenarios
        .iter()
        .map(|scenario| {
            let (same_identity_revision, same_identity_revision_warning) =
                same_identity_revision(scenario, local)?;
            Ok(ScenarioPreview {
                scenario_id: scenario.document.scenario_id,
                title: scenario.document.metadata.title.clone(),
                source_revision: scenario.revision,
                same_identity_revision,
                same_identity_revision_warning,
                collides: options.restore_mode != RestoreMode::ReplaceLibrary
                    && (scenario_has_local_collision(inspected, scenario, local)
                        || (options.include_results
                            && scenario_has_unsafe_local_result_collision(
                                inspected,
                                scenario.document.scenario_id,
                                local,
                            )?)),
            })
        })
        .collect()
}

fn build_supplemental_collision_preview(
    inspected: &InspectedBundle,
    options: &ImportOptions,
    local: &LocalLibrarySnapshot,
) -> Result<Vec<SupplementalIdentity>, ImportError> {
    let selected = supplemental_owned_uuids(inspected, options)?;
    let mut collisions = selected
        .keys()
        .filter(|identity| local.supplemental_identities.contains(*identity))
        .cloned()
        .collect::<BTreeSet<_>>();
    if options.restore_mode != RestoreMode::ReplaceLibrary {
        let local_occupied = local
            .occupied_uuids
            .iter()
            .filter_map(|identity| Uuid::parse_str(identity).ok())
            .collect::<BTreeSet<_>>();
        for (identity, owned) in &selected {
            if !owned.is_disjoint(&local_occupied) {
                collisions.insert(identity.clone());
            }
        }
    }
    Ok(collisions.into_iter().collect())
}

fn build_omitted_asset_preview(
    inspected: &InspectedBundle,
    options: &ImportOptions,
) -> Result<BTreeMap<String, OmittedAssetPlaceholder>, ImportError> {
    let mut omitted_assets = BTreeMap::new();
    for (asset_id, metadata) in &inspected.manifest.asset_metadata {
        let path = format!("assets/{asset_id}");
        let bytes = inspected
            .additional_entries
            .get(&path)
            .ok_or_else(|| ImportError::InvalidManifest(format!("asset {asset_id} is missing")))?;
        let asset = PortableAsset {
            bytes: bytes.clone(),
            media_type: metadata.media_type.clone(),
            redistribution_permitted: metadata.redistribution_permitted,
        };
        if let Some(placeholder) = parse_omitted_asset_placeholder(&asset)
            .map_err(|error| ImportError::InvalidManifest(error.to_string()))?
        {
            omitted_assets.insert(asset_id.clone(), placeholder);
        } else if !options.include_assets {
            let placeholder_asset =
                omitted_asset_placeholder(&asset, OmittedAssetReason::ImportExcluded)
                    .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
            let placeholder = parse_omitted_asset_placeholder(&placeholder_asset)
                .map_err(|error| ImportError::InvalidManifest(error.to_string()))?
                .ok_or_else(|| {
                    ImportError::InvalidManifest(format!(
                        "asset {asset_id} could not form an omission placeholder"
                    ))
                })?;
            omitted_assets.insert(asset_id.clone(), placeholder);
        }
    }
    Ok(omitted_assets)
}

/// Verifies that a preview still matches its file, options, local revision, and
/// the currently supported format versions.
///
/// # Errors
///
/// Returns [`ImportError::InvalidManifest`] if options serialization fails, or
/// [`ImportError::StalePreview`] if any binding value differs.
pub fn validate_preview_binding(
    binding: &PreviewBinding,
    file_sha256: &str,
    options: &ImportOptions,
    local_library_revision: Revision,
) -> Result<(), ImportError> {
    let options_bytes =
        canonical_json(options).map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
    if binding.file_sha256 != file_sha256
        || binding.options_sha256 != sha256_hex(&options_bytes)
        || binding.local_library_revision != local_library_revision
        || binding.format_version != CURRENT_BUNDLE_FORMAT_VERSION
        || binding.schema_version != CURRENT_PORTABLE_SCHEMA_VERSION
    {
        return Err(ImportError::StalePreview);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CollisionAction {
    CreateCopy,
    Replace,
    Skip,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupplementalCollisionAction {
    Replace,
    Skip,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupplementalCollisionChoice {
    pub section: SupplementalSectionKind,
    pub key: String,
    pub action: SupplementalCollisionAction,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CollisionPlan {
    pub scenarios: BTreeMap<ScenarioId, CollisionAction>,
    pub supplemental: BTreeMap<SupplementalIdentity, SupplementalCollisionAction>,
}

impl Serialize for CollisionPlan {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire<'a> {
            scenarios: &'a BTreeMap<ScenarioId, CollisionAction>,
            supplemental_choices: Vec<SupplementalCollisionChoice>,
        }
        let supplemental_choices = self
            .supplemental
            .iter()
            .map(|(identity, action)| SupplementalCollisionChoice {
                section: identity.section,
                key: identity.key.clone(),
                action: *action,
            })
            .collect();
        Wire {
            scenarios: &self.scenarios,
            supplemental_choices,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CollisionPlan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            scenarios: BTreeMap<ScenarioId, CollisionAction>,
            #[serde(default)]
            supplemental_choices: Vec<SupplementalCollisionChoice>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let mut supplemental = BTreeMap::new();
        for choice in wire.supplemental_choices {
            let identity = SupplementalIdentity {
                section: choice.section,
                key: choice.key,
            };
            if supplemental.insert(identity, choice.action).is_some() {
                return Err(de::Error::custom(
                    "supplemental collision choices contain a duplicate identity",
                ));
            }
        }
        Ok(Self {
            scenarios: wire.scenarios,
            supplemental,
        })
    }
}
/// Computes the stable UI review token for one preview binding and collision plan.
///
/// This domain-separated token binds the reviewed file/options/library revision
/// and exact plan. It is not safety-backup failure evidence.
///
/// # Errors
///
/// Returns [`ImportError::InvalidManifest`] if canonical serialization fails.
pub fn portable_review_token(
    binding: &PreviewBinding,
    plan: &CollisionPlan,
) -> Result<String, ImportError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ReviewTokenPayload<'a> {
        domain: &'static str,
        version: u32,
        binding: &'a PreviewBinding,
        collision_plan: &'a CollisionPlan,
    }
    let bytes = canonical_json(&ReviewTokenPayload {
        domain: "eutheto/portable-review-token",
        version: 1,
        binding,
        collision_plan: plan,
    })
    .map_err(|error| ImportError::InvalidManifest(error.to_string()))?;
    Ok(sha256_hex(&bytes))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagedDisposition {
    Create,
    CreateCopy,
    Replace,
}

#[derive(Clone, Debug)]
pub struct StagedScenario {
    pub original_id: ScenarioId,
    /// Immutable revision of the inspected source record before remapping or
    /// high-water advancement.
    pub source_revision: Revision,
    pub disposition: StagedDisposition,
    pub scenario: ScenarioSnapshotV1,
    /// Complete old-to-new UUID mapping applied to scenario-owned domain data.
    pub id_remap: BTreeMap<Uuid, Uuid>,
}

#[derive(Clone, Debug)]
pub struct ImportProvenance {
    pub source_bundle_id: BundleId,
    pub source_application: ApplicationMetadata,
    pub source_created_at: Rfc3339Timestamp,
    pub original_format_version: u32,
    pub original_schema_version: u32,
    pub source_file_sha256: String,
    pub applied_migrations: Vec<AppliedMigration>,
}

/// Fully validated input for exactly one later application/store transaction.
#[derive(Clone, Debug)]
pub struct StagedImport {
    pub binding: PreviewBinding,
    pub mode: RestoreMode,
    pub scenarios: Vec<StagedScenario>,
    pub scenario_revisions: Vec<ScenarioSnapshotV1>,
    pub results: BTreeMap<String, Vec<u8>>,
    pub shared_records: BTreeMap<String, Vec<u8>>,
    pub preferences: BTreeMap<String, Vec<u8>>,
    pub manifest_extensions: BTreeMap<String, Value>,
    pub nonsemantic_extensions: BTreeSet<String>,
    pub assets: BTreeMap<String, PortableAsset>,
    /// Exact non-replace-mode collisions authorized for replacement.
    pub supplemental_replacements: BTreeSet<SupplementalIdentity>,
    pub provenance: ImportProvenance,
}

/// Evidence required before a later store transaction may replace a library.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SafetyBackupEvidence {
    /// Add mode does not need a pre-restore backup.
    NotRequired,
    /// The application created, reopened, and verified a portable safety backup.
    Verified { bundle_sha256: String },
    /// Creation failed and the application supplied a receipt-bound proof.
    FailedWithStrongConfirmation { proof: String },
}

#[derive(Clone, Debug)]
pub struct RestoreAuthorization {
    pub destructive_action_confirmed: bool,
    pub safety_backup: SafetyBackupEvidence,
    /// Prospective token used only to bind the first replace attempt's failure receipt.
    pub prospective_failure_receipt_token: Option<String>,
    /// Canonical collision-plan hash, required after core normalizes replace authorization.
    pub collision_plan_sha256: Option<String>,
}

/// Typed backup-restore input for one later all-or-nothing store operation.
#[derive(Clone, Debug)]
pub struct StagedBackupRestore {
    pub import: StagedImport,
    /// Exact current scenarios removed by replace mode; empty for add mode.
    pub remove_scenario_ids: BTreeSet<ScenarioId>,
    pub authorization: RestoreAuthorization,
}

/// Stages scenarios and selected supplemental sections for a later atomic
/// store transaction.
///
/// # Errors
///
/// Returns [`ImportError`] if the preview binding is stale, the collision plan
/// is incomplete or names an unknown scenario, or deterministic ID remapping
/// fails.
pub fn stage_import(
    inspected: &InspectedBundle,
    preview: &ImportPreview,
    options: &ImportOptions,
    local: &LocalLibrarySnapshot,
    plan: &CollisionPlan,
) -> Result<StagedImport, ImportError> {
    validate_preview_binding(
        &preview.binding,
        &inspected.file_sha256,
        options,
        local.revision,
    )?;
    validate_collision_plan(preview, options.restore_mode, plan)?;
    let scenario_staging = stage_scenarios(inspected, options, local, plan)?;
    let mut split = split_sections(inspected, options)?;
    split.results = stage_json_section(
        SupplementalSectionKind::Results,
        split.results,
        plan,
        &scenario_staging.skipped_scenarios,
        &scenario_staging.bundle_mapping,
    )?;
    let retained_result_dependencies = staged_result_dependencies(&split.results)?;
    let scenario_revisions = stage_scenario_revisions(
        inspected,
        &retained_result_dependencies,
        &scenario_staging.bundle_mapping,
        &scenario_staging.scenarios,
    )?;
    let (split, supplemental_replacements) = stage_supplemental_sections(
        inspected,
        options,
        plan,
        local,
        &scenario_staging,
        &scenario_revisions,
        split,
    )?;
    let source_created_at = Rfc3339Timestamp::parse(&inspected.manifest.created_at)
        .map_err(|_| ImportError::InvalidManifest("bundle createdAt is invalid".to_owned()))?;
    Ok(StagedImport {
        binding: preview.binding.clone(),
        mode: options.restore_mode,
        scenarios: scenario_staging.scenarios,
        scenario_revisions,
        results: split.results,
        shared_records: split.shared_records,
        preferences: split.preferences,
        assets: split.assets,
        supplemental_replacements,
        manifest_extensions: inspected.manifest.extensions.clone(),
        nonsemantic_extensions: inspected.manifest.nonsemantic_extensions.clone(),
        provenance: ImportProvenance {
            source_bundle_id: inspected.manifest.bundle_id,
            source_application: inspected.manifest.application.clone(),
            source_created_at,
            original_format_version: inspected.original_format_version,
            original_schema_version: inspected.original_schema_version,
            source_file_sha256: inspected.file_sha256.clone(),
            applied_migrations: inspected.applied_migrations.clone(),
        },
    })
}

fn validate_collision_plan(
    preview: &ImportPreview,
    restore_mode: RestoreMode,
    plan: &CollisionPlan,
) -> Result<(), ImportError> {
    if restore_mode == RestoreMode::ReplaceLibrary {
        if !plan.scenarios.is_empty() || !plan.supplemental.is_empty() {
            return Err(ImportError::InvalidRestore(
                "replace-library collision plan must be empty".to_owned(),
            ));
        }
        return Ok(());
    }

    let preview_collisions = preview
        .scenarios
        .iter()
        .filter(|scenario| scenario.collides)
        .map(|scenario| scenario.scenario_id)
        .collect::<BTreeSet<_>>();
    for scenario_id in plan.scenarios.keys() {
        if !preview_collisions.contains(scenario_id) {
            return Err(ImportError::UnknownCollision(*scenario_id));
        }
    }
    for scenario_id in &preview_collisions {
        if !plan.scenarios.contains_key(scenario_id) {
            return Err(ImportError::UnresolvedCollision(*scenario_id));
        }
    }

    let preview_supplemental = preview
        .supplemental_collisions
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    for identity in plan.supplemental.keys() {
        if !preview_supplemental.contains(identity) {
            return Err(ImportError::UnknownSupplementalCollision(identity.clone()));
        }
    }
    for identity in &preview_supplemental {
        if !plan.supplemental.contains_key(identity) {
            return Err(ImportError::UnresolvedSupplementalCollision(
                identity.clone(),
            ));
        }
    }
    Ok(())
}

struct ScenarioStaging {
    scenarios: Vec<StagedScenario>,
    skipped_scenarios: BTreeSet<ScenarioId>,
    bundle_mapping: BTreeMap<Uuid, Uuid>,
}

fn occupied_import_identities(
    inspected: &InspectedBundle,
    options: &ImportOptions,
    local: &LocalLibrarySnapshot,
) -> Result<BTreeSet<Uuid>, ImportError> {
    let mut occupied = local
        .occupied_uuids
        .iter()
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect::<BTreeSet<_>>();
    for scenario in inspected
        .scenarios
        .iter()
        .chain(&inspected.scenario_revisions)
    {
        occupied.extend(collect_scenario_owned_uuids(scenario));
    }
    let imported_supplemental = supplemental_identities(inspected, options)?;
    for identity in local
        .supplemental_identities
        .iter()
        .chain(&imported_supplemental)
    {
        if identity.section != SupplementalSectionKind::Results
            && let Some(id) = supplemental_identity_uuid(identity)
        {
            occupied.insert(id);
        }
    }
    if options.include_results {
        for (path, bytes) in &inspected.additional_entries {
            if path.starts_with("results/") {
                occupied.insert(result_id_from_bytes(path, bytes)?);
            }
        }
    }
    Ok(occupied)
}

fn stage_scenarios(
    inspected: &InspectedBundle,
    options: &ImportOptions,
    local: &LocalLibrarySnapshot,
    plan: &CollisionPlan,
) -> Result<ScenarioStaging, ImportError> {
    let mut occupied = occupied_import_identities(inspected, options, local)?;
    validate_local_supplemental_uuid_collisions(inspected, options, local, plan)?;

    let mut copied_ids = BTreeSet::new();
    let mut skipped_scenarios = BTreeSet::new();
    for scenario in &inspected.scenarios {
        let scenario_id = scenario.document.scenario_id;
        let action = plan.scenarios.get(&scenario_id).copied();
        let collides = scenario_has_local_collision(inspected, scenario, local);
        if options.restore_mode == RestoreMode::ReplaceLibrary
            && collides
            && !scenario_replace_is_safe(inspected, scenario, local)
        {
            return Err(ImportError::Remap(format!(
                "scenario {scenario_id} reuses a durable identity outside its local family"
            )));
        }
        if options.restore_mode != RestoreMode::ReplaceLibrary && collides && action.is_none() {
            return Err(ImportError::UnresolvedCollision(scenario_id));
        }
        if options.restore_mode != RestoreMode::ReplaceLibrary
            && collides
            && action == Some(CollisionAction::Replace)
            && !scenario_replace_is_safe(inspected, scenario, local)
        {
            return Err(ImportError::Remap(format!(
                "scenario {scenario_id} collides with identities outside its local family and must be copied or skipped"
            )));
        }
        match action {
            Some(CollisionAction::CreateCopy) => {
                collect_scenario_family_uuids(inspected, scenario, &mut copied_ids);
            }
            Some(CollisionAction::Skip) => {
                skipped_scenarios.insert(scenario_id);
            }
            Some(CollisionAction::Replace) | None => {}
        }
    }
    collect_copied_result_ids(inspected, options, plan, &mut copied_ids)?;
    let bundle_mapping = allocate_identity_mapping(
        &copied_ids,
        inspected.manifest.bundle_id,
        &inspected.file_sha256,
        &mut occupied,
    )?;

    let mut scenarios = Vec::new();
    for scenario in &inspected.scenarios {
        let original_id = scenario.document.scenario_id;
        let action = plan.scenarios.get(&original_id).copied();
        if action == Some(CollisionAction::Skip) {
            continue;
        }
        let copied = action == Some(CollisionAction::CreateCopy);
        let mut rewritten = rewrite_scenario_for_plan(scenario, copied, &bundle_mapping)?;
        if !copied {
            rewritten.revision = same_identity_revision(scenario, local)?.0;
        }
        let id_remap = if copied {
            let mut owned = BTreeSet::new();
            collect_scenario_family_uuids(inspected, scenario, &mut owned);
            owned
                .into_iter()
                .filter_map(|old| bundle_mapping.get(&old).copied().map(|new| (old, new)))
                .collect()
        } else {
            BTreeMap::new()
        };
        scenarios.push(StagedScenario {
            original_id,
            source_revision: scenario.revision,
            disposition: match action {
                Some(CollisionAction::CreateCopy) => StagedDisposition::CreateCopy,
                Some(CollisionAction::Replace) => StagedDisposition::Replace,
                Some(CollisionAction::Skip) => continue,
                None if local.scenario_ids.contains(&original_id) => StagedDisposition::Replace,
                None => StagedDisposition::Create,
            },
            scenario: rewritten,
            id_remap,
        });
    }
    Ok(ScenarioStaging {
        scenarios,
        skipped_scenarios,
        bundle_mapping,
    })
}
fn collect_copied_result_ids(
    inspected: &InspectedBundle,
    options: &ImportOptions,
    plan: &CollisionPlan,
    copied_ids: &mut BTreeSet<Uuid>,
) -> Result<(), ImportError> {
    if !options.include_results {
        return Ok(());
    }
    for (path, bytes) in &inspected.additional_entries {
        let Some(identity) = supplemental_identity(path, bytes, options)?
            .filter(|identity| identity.section == SupplementalSectionKind::Results)
        else {
            continue;
        };
        if plan.supplemental.get(&identity) == Some(&SupplementalCollisionAction::Skip) {
            continue;
        }
        let value = parse_strict_json(path, bytes, &PORTABLE_LIMITS)?;
        let dependency = extract_result_dependency(&value).map_err(|error| {
            ImportError::Remap(format!("{path} has invalid dependency: {error}"))
        })?;
        if plan.scenarios.get(&dependency.scenario_id) == Some(&CollisionAction::CreateCopy) {
            copied_ids.insert(extract_result_id(&value).map_err(|error| {
                ImportError::Remap(format!("{path} has invalid result identity: {error}"))
            })?);
        }
    }
    Ok(())
}

fn validate_local_supplemental_uuid_collisions(
    inspected: &InspectedBundle,
    options: &ImportOptions,
    local: &LocalLibrarySnapshot,
    plan: &CollisionPlan,
) -> Result<(), ImportError> {
    let local_occupied = local
        .occupied_uuids
        .iter()
        .filter_map(|identity| Uuid::parse_str(identity).ok())
        .collect::<BTreeSet<_>>();
    let durable_scenario_roots = local
        .scenario_revision_high_water
        .keys()
        .map(|scenario_id| scenario_id.as_uuid())
        .collect::<BTreeSet<_>>();
    for (identity, owned) in supplemental_owned_uuids(inspected, options)? {
        if plan.supplemental.get(&identity) == Some(&SupplementalCollisionAction::Skip) {
            continue;
        }
        let key_uuid = supplemental_identity_uuid(&identity);
        let (copied_result, skipped_result) =
            if identity.section == SupplementalSectionKind::Results {
                let path = format!("results/{}", identity.key);
                let bytes = inspected.additional_entries.get(&path).ok_or_else(|| {
                    ImportError::Remap(format!("retained result {path} is missing"))
                })?;
                let value = parse_strict_json(&path, bytes, &PORTABLE_LIMITS)?;
                let dependency = extract_result_dependency(&value)
                    .map_err(|error| ImportError::Remap(format!("{path}: {error}")))?;
                match plan.scenarios.get(&dependency.scenario_id) {
                    Some(CollisionAction::CreateCopy) => (true, false),
                    Some(CollisionAction::Skip) => (false, true),
                    Some(CollisionAction::Replace) | None => (false, false),
                }
            } else {
                (false, false)
            };
        if skipped_result {
            continue;
        }
        for uuid in owned {
            if durable_scenario_roots.contains(&uuid) && !(key_uuid == Some(uuid) && copied_result)
            {
                return Err(ImportError::Remap(format!(
                    "supplemental identity {uuid} overlaps a durable scenario identity"
                )));
            }
            if options.restore_mode == RestoreMode::ReplaceLibrary {
                continue;
            }
            if !local_occupied.contains(&uuid) {
                continue;
            }
            let replaces_same_supplemental = !copied_result
                && plan.supplemental.get(&identity) == Some(&SupplementalCollisionAction::Replace)
                && (local.supplemental_identity_owners.get(&uuid) == Some(&identity)
                    || (key_uuid == Some(uuid)
                        && local.supplemental_identities.contains(&identity)));
            if replaces_same_supplemental || (key_uuid == Some(uuid) && copied_result) {
                continue;
            }
            return Err(ImportError::Remap(format!(
                "supplemental identity {uuid} overlaps a local identity and must be skipped"
            )));
        }
    }
    Ok(())
}

fn scenario_has_unsafe_local_result_collision(
    inspected: &InspectedBundle,
    scenario_id: ScenarioId,
    local: &LocalLibrarySnapshot,
) -> Result<bool, ImportError> {
    let local_occupied = local
        .occupied_uuids
        .iter()
        .filter_map(|identity| Uuid::parse_str(identity).ok())
        .collect::<BTreeSet<_>>();
    for (path, bytes) in &inspected.additional_entries {
        if !path.starts_with("results/") {
            continue;
        }
        let value = parse_strict_json(path, bytes, &PORTABLE_LIMITS)?;
        let dependency = extract_result_dependency(&value)
            .map_err(|error| ImportError::Remap(format!("{path}: {error}")))?;
        if dependency.scenario_id != scenario_id {
            continue;
        }
        let result_id = extract_result_id(&value)
            .map_err(|error| ImportError::Remap(format!("{path}: {error}")))?;
        let identity = SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key: format!("{result_id}.json"),
        };
        if local_occupied.contains(&result_id) && !local.supplemental_identities.contains(&identity)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn allocate_identity_mapping(
    copied_ids: &BTreeSet<Uuid>,
    bundle_id: BundleId,
    file_sha256: &str,
    occupied: &mut BTreeSet<Uuid>,
) -> Result<BTreeMap<Uuid, Uuid>, ImportError> {
    let mut mapping = BTreeMap::new();
    for &old in copied_ids {
        let mut salt = 0_u32;
        let new = loop {
            let candidate = deterministic_uuid_v7(bundle_id, file_sha256, old, salt);
            if !copied_ids.contains(&candidate) && occupied.insert(candidate) {
                break candidate;
            }
            salt = salt
                .checked_add(1)
                .ok_or_else(|| ImportError::Remap("ID derivation exhausted".to_owned()))?;
        };
        mapping.insert(old, new);
    }
    Ok(mapping)
}

fn stage_supplemental_sections(
    inspected: &InspectedBundle,
    options: &ImportOptions,
    plan: &CollisionPlan,
    local: &LocalLibrarySnapshot,
    scenario_staging: &ScenarioStaging,
    scenario_revisions: &[ScenarioSnapshotV1],
    mut split: SplitSections,
) -> Result<(SplitSections, BTreeSet<SupplementalIdentity>), ImportError> {
    split.shared_records = stage_json_section(
        SupplementalSectionKind::SharedRecords,
        split.shared_records,
        plan,
        &scenario_staging.skipped_scenarios,
        &scenario_staging.bundle_mapping,
    )?;
    split.preferences = stage_json_section(
        SupplementalSectionKind::Preferences,
        split.preferences,
        plan,
        &scenario_staging.skipped_scenarios,
        &scenario_staging.bundle_mapping,
    )?;
    split.assets = stage_assets(split.assets, plan);
    let StagedReferences {
        represented,
        referenced_assets,
    } = staged_references(&scenario_staging.scenarios, scenario_revisions, &split)?;
    if inspected.manifest.bundle_kind == BundleKind::ScenarioExport {
        split
            .assets
            .retain(|key, _| referenced_assets.contains(key));
    }
    let mut available_assets = split.assets.keys().cloned().collect::<BTreeSet<_>>();
    if options.restore_mode != RestoreMode::ReplaceLibrary {
        available_assets.extend(
            plan.supplemental
                .iter()
                .filter(|(identity, action)| {
                    identity.section == SupplementalSectionKind::Assets
                        && **action == SupplementalCollisionAction::Skip
                        && local.supplemental_identities.contains(identity)
                })
                .map(|(identity, _)| identity.key.clone()),
        );
    }
    if let Some(missing) = referenced_assets.difference(&available_assets).next() {
        return Err(ImportError::InvalidManifest(format!(
            "retained import records reference absent asset {missing}"
        )));
    }
    let represented_scenario_ids = represented
        .iter()
        .map(|(scenario_id, _)| *scenario_id)
        .collect::<BTreeSet<_>>();
    validate_staged_references(
        [&split.shared_records, &split.preferences],
        &represented_scenario_ids,
    )?;
    validate_staged_result_dependencies(&split.results, &represented)?;
    let supplemental_replacements = staged_supplemental_identities(&split)?
        .into_iter()
        .filter(|identity| {
            plan.supplemental.get(identity) == Some(&SupplementalCollisionAction::Replace)
        })
        .collect();
    Ok((split, supplemental_replacements))
}

struct StagedReferences {
    represented: BTreeSet<(ScenarioId, Revision)>,
    referenced_assets: BTreeSet<String>,
}

fn staged_references(
    staged_scenarios: &[StagedScenario],
    staged_scenario_revisions: &[ScenarioSnapshotV1],
    split: &SplitSections,
) -> Result<StagedReferences, ImportError> {
    let represented = staged_scenarios
        .iter()
        .map(|staged| {
            (
                staged.scenario.document.scenario_id,
                staged.scenario.revision,
            )
        })
        .chain(
            staged_scenario_revisions
                .iter()
                .map(|scenario| (scenario.document.scenario_id, scenario.revision)),
        )
        .collect::<BTreeSet<_>>();
    let mut referenced_assets = BTreeSet::new();
    for scenario in staged_scenarios
        .iter()
        .map(|staged| &staged.scenario)
        .chain(staged_scenario_revisions)
    {
        let value = serde_json::to_value(scenario)
            .map_err(|error| ImportError::Remap(error.to_string()))?;
        referenced_assets.extend(
            extract_asset_references(&value)
                .map_err(|error| ImportError::InvalidManifest(error.to_string()))?,
        );
    }
    for (key, bytes) in split
        .results
        .iter()
        .chain(&split.shared_records)
        .chain(&split.preferences)
    {
        let value = parse_strict_json(key, bytes, &PORTABLE_LIMITS)?;
        referenced_assets.extend(
            extract_asset_references(&value)
                .map_err(|error| ImportError::InvalidManifest(error.to_string()))?,
        );
    }
    Ok(StagedReferences {
        represented,
        referenced_assets,
    })
}

fn stage_assets(
    assets: BTreeMap<String, PortableAsset>,
    plan: &CollisionPlan,
) -> BTreeMap<String, PortableAsset> {
    assets
        .into_iter()
        .filter(|(key, _)| {
            plan.supplemental.get(&SupplementalIdentity {
                section: SupplementalSectionKind::Assets,
                key: key.clone(),
            }) != Some(&SupplementalCollisionAction::Skip)
        })
        .collect()
}

fn stage_json_section(
    section: SupplementalSectionKind,
    values: BTreeMap<String, Vec<u8>>,
    plan: &CollisionPlan,
    skipped_scenarios: &BTreeSet<ScenarioId>,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<BTreeMap<String, Vec<u8>>, ImportError> {
    let mut staged = BTreeMap::new();
    for (key, bytes) in values {
        let mut value = parse_strict_json(&format!("{section}/{key}"), &bytes, &PORTABLE_LIMITS)?;
        let result_id = if section == SupplementalSectionKind::Results {
            Some(extract_result_id(&value).map_err(|error| {
                ImportError::Remap(format!("{section}/{key} has invalid identity: {error}"))
            })?)
        } else {
            None
        };
        let identity = SupplementalIdentity {
            section,
            key: result_id.map_or_else(|| key.clone(), |id| format!("{id}.json")),
        };
        if plan.supplemental.get(&identity) == Some(&SupplementalCollisionAction::Skip) {
            continue;
        }
        let result_scenario_id = if section == SupplementalSectionKind::Results {
            Some(
                extract_result_dependency(&value)
                    .map_err(|error| {
                        ImportError::Remap(format!(
                            "{section}/{key} has invalid dependency: {error}"
                        ))
                    })?
                    .scenario_id,
            )
        } else {
            None
        };
        let references = if let Some(scenario_id) = result_scenario_id {
            BTreeSet::from([scenario_id])
        } else {
            extract_scenario_references(&value).map_err(|error| {
                ImportError::Remap(format!("{section}/{key} has invalid reference: {error}"))
            })?
        };
        if !references.is_disjoint(skipped_scenarios) {
            continue;
        }
        let copied_scenario = result_scenario_id
            .is_some_and(|scenario_id| mapping.contains_key(&scenario_id.as_uuid()));
        let copy_identity =
            copied_scenario && result_id.is_some_and(|id| mapping.contains_key(&id));
        let rewritten = !mapping.is_empty();
        if rewritten {
            rewrite_declared_references(&mut value, mapping)?;
        }
        reject_stale_declared_references(&value, mapping)?;
        let staged_key = if copy_identity {
            let old = result_id.ok_or_else(|| {
                ImportError::Remap(format!(
                    "supplemental record {section}/{key} has no typed result identity"
                ))
            })?;
            remap_supplemental_key(&identity, old, mapping)?
        } else {
            key
        };
        if section == SupplementalSectionKind::Results {
            validate_result_identity(&value, &staged_key)?;
        }
        let bytes = if rewritten {
            serde_json::to_vec(&value).map_err(|error| ImportError::Remap(error.to_string()))?
        } else {
            bytes
        };
        if staged.insert(staged_key, bytes).is_some() {
            return Err(ImportError::Remap(
                "supplemental copy produced a duplicate identity".to_owned(),
            ));
        }
    }
    Ok(staged)
}

fn validate_result_identity(value: &Value, staged_key: &str) -> Result<(), ImportError> {
    let result_id = extract_result_id(value).map_err(|error| {
        ImportError::Remap(format!("invalid retained result identity: {error}"))
    })?;
    let staged_identity = staged_key.strip_suffix(".json").ok_or_else(|| {
        ImportError::Remap("retained result key must use the .json suffix".to_owned())
    })?;
    if result_id.to_string() != staged_identity {
        return Err(ImportError::Remap(
            "retained resultId does not match its staged key".to_owned(),
        ));
    }
    Ok(())
}
fn staged_result_dependencies(
    results: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeSet<(ScenarioId, Revision)>, ImportError> {
    results
        .iter()
        .map(|(key, bytes)| {
            let value = parse_strict_json(&format!("results/{key}"), bytes, &PORTABLE_LIMITS)?;
            let dependency = extract_result_dependency(&value).map_err(|error| {
                ImportError::Remap(format!("results/{key} has invalid dependency: {error}"))
            })?;
            Ok((dependency.scenario_id, dependency.scenario_revision))
        })
        .collect()
}

fn validate_staged_result_dependencies(
    results: &BTreeMap<String, Vec<u8>>,
    represented: &BTreeSet<(ScenarioId, Revision)>,
) -> Result<(), ImportError> {
    for (key, bytes) in results {
        let value = parse_strict_json(&format!("results/{key}"), bytes, &PORTABLE_LIMITS)?;
        let dependency = extract_result_dependency(&value).map_err(|error| {
            ImportError::Remap(format!("results/{key} has invalid dependency: {error}"))
        })?;
        if !represented.contains(&(dependency.scenario_id, dependency.scenario_revision)) {
            return Err(ImportError::Remap(format!(
                "results/{key} requires scenario {} at exact revision {}",
                dependency.scenario_id,
                dependency.scenario_revision.value()
            )));
        }
    }
    Ok(())
}
fn validate_staged_references<'a, I>(
    sections: I,
    represented: &BTreeSet<ScenarioId>,
) -> Result<(), ImportError>
where
    I: IntoIterator<Item = &'a BTreeMap<String, Vec<u8>>>,
{
    for section in sections {
        for (key, bytes) in section {
            let value = parse_strict_json(key, bytes, &PORTABLE_LIMITS)?;
            let references = extract_scenario_references(&value)
                .map_err(|error| ImportError::Remap(error.to_string()))?;
            if let Some(missing) = references.difference(represented).next() {
                return Err(ImportError::Remap(format!(
                    "supplemental record {key} references unstaged scenario {missing}"
                )));
            }
        }
    }
    Ok(())
}

fn staged_supplemental_identities(
    split: &SplitSections,
) -> Result<BTreeSet<SupplementalIdentity>, ImportError> {
    let mut identities = BTreeSet::new();
    for (key, bytes) in &split.results {
        let result_id = result_id_from_bytes(&format!("results/{key}"), bytes)?;
        identities.insert(SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key: format!("{result_id}.json"),
        });
    }
    for (section, keys) in [
        (
            SupplementalSectionKind::SharedRecords,
            split.shared_records.keys(),
        ),
        (
            SupplementalSectionKind::Preferences,
            split.preferences.keys(),
        ),
    ] {
        identities.extend(
            keys.cloned()
                .map(|key| SupplementalIdentity { section, key }),
        );
    }
    identities.extend(
        split
            .assets
            .keys()
            .cloned()
            .map(|key| SupplementalIdentity {
                section: SupplementalSectionKind::Assets,
                key,
            }),
    );
    Ok(identities)
}

/// Stages an authorized full-backup restore for a later atomic store operation.
///
/// # Errors
///
/// Returns [`ImportError::InvalidRestore`] when the bundle, restore mode, or
/// authorization evidence is invalid, and propagates any error from
/// [`stage_import`].
pub fn stage_backup_restore(
    inspected: &InspectedBundle,
    preview: &ImportPreview,
    options: &ImportOptions,
    local: &LocalLibrarySnapshot,
    plan: &CollisionPlan,
    authorization: RestoreAuthorization,
) -> Result<StagedBackupRestore, ImportError> {
    if inspected.manifest.bundle_kind != BundleKind::FullBackup {
        return Err(ImportError::InvalidRestore(
            "restore requires a full-backup bundle".to_owned(),
        ));
    }
    match options.restore_mode {
        RestoreMode::ImportScenario => {
            return Err(ImportError::InvalidRestore(
                "restore mode must be add-backup or replace-library".to_owned(),
            ));
        }
        RestoreMode::AddBackup => {
            if authorization.safety_backup != SafetyBackupEvidence::NotRequired
                || authorization.prospective_failure_receipt_token.is_some()
                || authorization.collision_plan_sha256.is_some()
            {
                return Err(ImportError::InvalidRestore(
                    "add mode must not carry replace authorization evidence".to_owned(),
                ));
            }
        }
        RestoreMode::ReplaceLibrary => {
            if !authorization.destructive_action_confirmed {
                return Err(ImportError::InvalidRestore(
                    "replace mode requires explicit destructive confirmation".to_owned(),
                ));
            }
            if authorization.collision_plan_sha256.is_none() {
                return Err(ImportError::InvalidRestore(
                    "replace mode requires a collision-plan hash".to_owned(),
                ));
            }
            let invalid_evidence = match &authorization.safety_backup {
                SafetyBackupEvidence::NotRequired => {
                    authorization.prospective_failure_receipt_token.is_none()
                }
                SafetyBackupEvidence::Verified { .. }
                | SafetyBackupEvidence::FailedWithStrongConfirmation { .. } => {
                    authorization.prospective_failure_receipt_token.is_some()
                }
            };
            if invalid_evidence {
                return Err(ImportError::InvalidRestore(
                    "replace backup evidence does not match its receipt-token phase".to_owned(),
                ));
            }
        }
    }
    let import = stage_import(inspected, preview, options, local, plan)?;
    let remove_scenario_ids = if options.restore_mode == RestoreMode::ReplaceLibrary {
        local.scenario_ids.clone()
    } else {
        BTreeSet::new()
    };
    Ok(StagedBackupRestore {
        import,
        remove_scenario_ids,
        authorization,
    })
}
#[derive(Default)]
struct SplitSections {
    results: BTreeMap<String, Vec<u8>>,
    shared_records: BTreeMap<String, Vec<u8>>,
    preferences: BTreeMap<String, Vec<u8>>,
    assets: BTreeMap<String, PortableAsset>,
}

fn supplemental_identities(
    inspected: &InspectedBundle,
    options: &ImportOptions,
) -> Result<BTreeSet<SupplementalIdentity>, ImportError> {
    let mut identities = BTreeSet::new();
    for (path, bytes) in &inspected.additional_entries {
        if let Some(identity) = supplemental_identity(path, bytes, options)? {
            identities.insert(identity);
        }
    }
    Ok(identities)
}

fn supplemental_identity(
    path: &str,
    bytes: &[u8],
    options: &ImportOptions,
) -> Result<Option<SupplementalIdentity>, ImportError> {
    let (section, key) = if path.starts_with("results/") {
        if !options.include_results {
            return Ok(None);
        }
        let result_id = result_id_from_bytes(path, bytes)?;
        (
            SupplementalSectionKind::Results,
            format!("{result_id}.json"),
        )
    } else if let Some(key) = path.strip_prefix("shared/") {
        (SupplementalSectionKind::SharedRecords, key.to_owned())
    } else if let Some(key) = path.strip_prefix("preferences/") {
        (SupplementalSectionKind::Preferences, key.to_owned())
    } else if let Some(key) = path.strip_prefix("assets/") {
        // Excluding bytes still stages a reconnectable placeholder at this key.
        (SupplementalSectionKind::Assets, key.to_owned())
    } else {
        return Ok(None);
    };
    Ok(Some(SupplementalIdentity { section, key }))
}

fn result_id_from_bytes(path: &str, bytes: &[u8]) -> Result<Uuid, ImportError> {
    let value = parse_strict_json(path, bytes, &PORTABLE_LIMITS)?;
    extract_result_id(&value)
        .map_err(|error| ImportError::Remap(format!("{path} has invalid result identity: {error}")))
}

fn supplemental_identity_uuid(identity: &SupplementalIdentity) -> Option<Uuid> {
    let stem = identity.key.split('.').next().unwrap_or(&identity.key);
    Uuid::parse_str(stem).ok()
}

fn supplemental_owned_uuids(
    inspected: &InspectedBundle,
    options: &ImportOptions,
) -> Result<BTreeMap<SupplementalIdentity, BTreeSet<Uuid>>, ImportError> {
    let mut owners = BTreeMap::new();
    for (path, bytes) in &inspected.additional_entries {
        let Some(identity) = supplemental_identity(path, bytes, options)? else {
            continue;
        };
        let mut owned = supplemental_identity_uuid(&identity)
            .into_iter()
            .collect::<BTreeSet<_>>();
        if identity.section != SupplementalSectionKind::Assets {
            let value = parse_strict_json(path, bytes, &PORTABLE_LIMITS)?;
            owned.extend(collect_self_declared_uuids(&value));
        }
        if owners.insert(identity, owned).is_some() {
            return Err(ImportError::InvalidManifest(
                "supplemental identity is duplicated".to_owned(),
            ));
        }
    }
    Ok(owners)
}

fn remap_supplemental_key(
    identity: &SupplementalIdentity,
    old: Uuid,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<String, ImportError> {
    let new = mapping
        .get(&old)
        .ok_or_else(|| ImportError::Remap(format!("supplemental identity {old} was not mapped")))?;
    let suffix = identity
        .key
        .strip_prefix(&old.to_string())
        .unwrap_or_default();
    Ok(format!("{new}{suffix}"))
}

fn split_sections(
    inspected: &InspectedBundle,
    options: &ImportOptions,
) -> Result<SplitSections, ImportError> {
    let mut split = SplitSections::default();
    for (path, bytes) in &inspected.additional_entries {
        if let Some(name) = path.strip_prefix("results/") {
            if options.include_results {
                split.results.insert(name.to_owned(), bytes.clone());
            }
        } else if let Some(name) = path.strip_prefix("shared/") {
            split.shared_records.insert(name.to_owned(), bytes.clone());
        } else if let Some(name) = path.strip_prefix("preferences/") {
            split.preferences.insert(name.to_owned(), bytes.clone());
        } else if let Some(name) = path.strip_prefix("assets/") {
            let metadata = inspected.manifest.asset_metadata.get(name).ok_or_else(|| {
                ImportError::InvalidManifest(format!("asset {name} lacks manifest metadata"))
            })?;
            let source = PortableAsset {
                bytes: bytes.clone(),
                media_type: metadata.media_type.clone(),
                redistribution_permitted: metadata.redistribution_permitted,
            };
            let asset = if options.include_assets
                || parse_omitted_asset_placeholder(&source)
                    .map_err(|error| ImportError::InvalidManifest(error.to_string()))?
                    .is_some()
            {
                source
            } else {
                omitted_asset_placeholder(&source, OmittedAssetReason::ImportExcluded)
                    .map_err(|error| ImportError::InvalidManifest(error.to_string()))?
            };
            split.assets.insert(name.to_owned(), asset);
        }
    }
    Ok(split)
}

fn rewrite_scenario_for_plan(
    scenario: &ScenarioSnapshotV1,
    copied: bool,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<ScenarioSnapshotV1, ImportError> {
    let mut domain = serde_json::to_value(&scenario.document.domain)
        .map_err(|error| ImportError::Remap(error.to_string()))?;
    let mut semantic_extensions = serde_json::to_value(&scenario.semantic_extensions)
        .map_err(|error| ImportError::Remap(error.to_string()))?;
    let mut document_extensions = serde_json::to_value(&scenario.document.extensions)
        .map_err(|error| ImportError::Remap(error.to_string()))?;
    let mut wrapper_extensions = serde_json::to_value(&scenario.extensions)
        .map_err(|error| ImportError::Remap(error.to_string()))?;
    if copied {
        let owned = collect_scenario_owned_uuids(scenario);
        let owned_mapping = owned
            .into_iter()
            .filter_map(|old| mapping.get(&old).copied().map(|new| (old, new)))
            .collect::<BTreeMap<_, _>>();
        rewrite_domain_definitions(&mut domain, &owned_mapping)?;
        rewrite_self_declared_definitions(&mut domain, &owned_mapping)?;
        rewrite_self_declared_definitions(&mut semantic_extensions, &owned_mapping)?;
        rewrite_self_declared_definitions(&mut document_extensions, &owned_mapping)?;
        rewrite_self_declared_definitions(&mut wrapper_extensions, &owned_mapping)?;
    }
    rewrite_declared_references(&mut domain, mapping)?;
    rewrite_declared_references(&mut semantic_extensions, mapping)?;
    rewrite_declared_references(&mut document_extensions, mapping)?;
    rewrite_declared_references(&mut wrapper_extensions, mapping)?;
    reject_stale_declared_references(&domain, mapping)?;
    reject_stale_declared_references(&semantic_extensions, mapping)?;
    reject_stale_declared_references(&document_extensions, mapping)?;
    reject_stale_declared_references(&wrapper_extensions, mapping)?;

    let mut rewritten = scenario.clone();
    if copied {
        let new_scenario_id = mapping
            .get(&scenario.document.scenario_id.as_uuid())
            .copied()
            .ok_or_else(|| ImportError::Remap("scenario ID was not mapped".to_owned()))?;
        rewritten.document.scenario_id = ScenarioId::from_uuid(new_scenario_id);
    }
    rewritten.document.domain =
        serde_json::from_value(domain).map_err(|error| ImportError::Remap(error.to_string()))?;
    rewritten.semantic_extensions = serde_json::from_value(semantic_extensions)
        .map_err(|error| ImportError::Remap(error.to_string()))?;
    rewritten.document.extensions = serde_json::from_value(document_extensions)
        .map_err(|error| ImportError::Remap(error.to_string()))?;
    rewritten.extensions = serde_json::from_value(wrapper_extensions)
        .map_err(|error| ImportError::Remap(error.to_string()))?;
    validate_scenario_snapshot(&rewritten)
        .map_err(|error| ImportError::Remap(error.to_string()))?;
    validate_document_shape(&rewritten.document)
        .map_err(|error| ImportError::Remap(error.to_string()))?;
    Ok(rewritten)
}

fn insert_scenario_revision(
    revisions: &mut BTreeMap<(ScenarioId, Revision), ScenarioSnapshotV1>,
    candidate: ScenarioSnapshotV1,
) -> Result<(), ImportError> {
    let key = (candidate.document.scenario_id, candidate.revision);
    if let Some(existing) = revisions.get(&key) {
        if existing != &candidate {
            return Err(ImportError::ConflictingScenarioRevision {
                scenario_id: key.0,
                revision: key.1,
            });
        }
        return Ok(());
    }
    revisions.insert(key, candidate);
    Ok(())
}

fn stage_scenario_revisions(
    inspected: &InspectedBundle,
    retained_result_dependencies: &BTreeSet<(ScenarioId, Revision)>,
    mapping: &BTreeMap<Uuid, Uuid>,
    staged_scenarios: &[StagedScenario],
) -> Result<Vec<ScenarioSnapshotV1>, ImportError> {
    let mut revisions = BTreeMap::new();
    for scenario in &inspected.scenario_revisions {
        let rewritten = rewrite_scenario_for_plan(
            scenario,
            mapping.contains_key(&scenario.document.scenario_id.as_uuid()),
            mapping,
        )?;
        let dependency = (rewritten.document.scenario_id, rewritten.revision);
        if retained_result_dependencies.contains(&dependency) {
            insert_scenario_revision(&mut revisions, rewritten)?;
        }
    }
    for source in &inspected.scenarios {
        let Some(staged) = staged_scenarios
            .iter()
            .find(|staged| staged.original_id == source.document.scenario_id)
        else {
            continue;
        };
        if staged.scenario.revision == source.revision {
            continue;
        }
        let mut historical = rewrite_scenario_for_plan(source, false, mapping)?;
        let dependency = (historical.document.scenario_id, historical.revision);
        if !retained_result_dependencies.contains(&dependency) {
            continue;
        }
        historical.project = None;
        insert_scenario_revision(&mut revisions, historical)?;
    }
    Ok(revisions.into_values().collect())
}

fn collect_scenario_family_uuids(
    inspected: &InspectedBundle,
    scenario: &ScenarioSnapshotV1,
    ids: &mut BTreeSet<Uuid>,
) {
    ids.extend(collect_scenario_owned_uuids(scenario));
    let scenario_id = scenario.document.scenario_id;
    for revision in &inspected.scenario_revisions {
        if revision.document.scenario_id == scenario_id {
            ids.extend(collect_scenario_owned_uuids(revision));
        }
    }
}
fn validate_replace_library_identity_ownership(
    inspected: &InspectedBundle,
    options: &ImportOptions,
    local: &LocalLibrarySnapshot,
) -> Result<(), ImportError> {
    if options.restore_mode != RestoreMode::ReplaceLibrary {
        return Ok(());
    }
    let mut durable_owners = local.identity_owners.clone();
    for scenario_id in local.scenario_revision_high_water.keys().copied() {
        durable_owners
            .entry(scenario_id.as_uuid())
            .or_insert(scenario_id);
    }
    for scenario in &inspected.scenarios {
        let scenario_id = scenario.document.scenario_id;
        let mut owned = BTreeSet::new();
        collect_scenario_family_uuids(inspected, scenario, &mut owned);
        for identity in owned {
            if durable_owners
                .get(&identity)
                .is_some_and(|owner| *owner != scenario_id)
            {
                return Err(ImportError::InvalidRestore(format!(
                    "scenario {scenario_id} identity {identity} is durably owned by another scenario family"
                )));
            }
        }
    }
    for (supplemental, owned) in supplemental_owned_uuids(inspected, options)? {
        if let Some(identity) = owned
            .into_iter()
            .find(|identity| durable_owners.contains_key(identity))
        {
            return Err(ImportError::InvalidRestore(format!(
                "supplemental record {:?}/{} identity {identity} overlaps a durable scenario family",
                supplemental.section, supplemental.key
            )));
        }
    }
    Ok(())
}

fn scenario_has_local_collision(
    inspected: &InspectedBundle,
    scenario: &ScenarioSnapshotV1,
    local: &LocalLibrarySnapshot,
) -> bool {
    if local.scenario_ids.contains(&scenario.document.scenario_id) {
        return true;
    }
    let mut owned = BTreeSet::new();
    collect_scenario_family_uuids(inspected, scenario, &mut owned);
    let root = scenario.document.scenario_id.as_uuid();
    let root_is_tombstone_only = local
        .scenario_revision_high_water
        .contains_key(&scenario.document.scenario_id)
        && !local
            .scenarios
            .iter()
            .any(|current| current.owned_uuids.contains(&root))
        && !local
            .supplemental_identities
            .iter()
            .filter_map(supplemental_identity_uuid)
            .any(|identity| identity == root);
    if root_is_tombstone_only {
        owned.remove(&root);
    }
    if !local.scenario_ids.contains(&scenario.document.scenario_id) {
        owned.retain(|identity| {
            local.identity_owners.get(identity) != Some(&scenario.document.scenario_id)
        });
    }
    local
        .occupied_uuids
        .iter()
        .filter_map(|identity| Uuid::parse_str(identity).ok())
        .any(|identity| owned.contains(&identity))
}
fn scenario_replace_is_safe(
    inspected: &InspectedBundle,
    scenario: &ScenarioSnapshotV1,
    local: &LocalLibrarySnapshot,
) -> bool {
    let scenario_id = scenario.document.scenario_id;
    let mut imported_owned = BTreeSet::new();
    collect_scenario_family_uuids(inspected, scenario, &mut imported_owned);
    let occupied = local
        .occupied_uuids
        .iter()
        .filter_map(|identity| Uuid::parse_str(identity).ok())
        .collect::<BTreeSet<_>>();
    imported_owned.intersection(&occupied).all(|identity| {
        if local.identity_owners.get(identity) == Some(&scenario_id) {
            return true;
        }
        if *identity == scenario_id.as_uuid() && local.scenario_ids.contains(&scenario_id) {
            return true;
        }
        let owners = local
            .scenarios
            .iter()
            .filter(|local_scenario| local_scenario.owned_uuids.contains(identity))
            .map(|local_scenario| local_scenario.scenario_id)
            .collect::<BTreeSet<_>>();
        owners == BTreeSet::from([scenario_id])
    })
}

fn rewrite_self_declared_definitions(
    value: &mut Value,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<(), ImportError> {
    match value {
        Value::Array(values) => {
            for value in values {
                rewrite_self_declared_definitions(value, mapping)?;
            }
        }
        Value::Object(values) => {
            let taken = std::mem::take(values);
            for (key, mut value) in taken {
                rewrite_self_declared_definitions(&mut value, mapping)?;
                let rewritten_key = eutheto_types::self_declared_uuid(&key, &value)
                    .and_then(|old| mapping.get(&old))
                    .map_or_else(
                        || key.clone(),
                        |new| {
                            if let Some(id) = value.get_mut("id") {
                                *id = Value::String(new.to_string());
                            }
                            new.to_string()
                        },
                    );
                if values.insert(rewritten_key.clone(), value).is_some() {
                    return Err(ImportError::Remap(format!(
                        "definition rewrite produced duplicate key {rewritten_key}"
                    )));
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn rewrite_domain_definitions(
    domain: &mut Value,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<(), ImportError> {
    let object = domain
        .as_object_mut()
        .ok_or_else(|| ImportError::Remap("scenario domain is not an object".to_owned()))?;
    for section in ["entities", "rules", "preferences", "lockedAssignments"] {
        let Some(records) = object.get_mut(section).and_then(Value::as_object_mut) else {
            return Err(ImportError::Remap(format!(
                "scenario domain section {section} is not an object"
            )));
        };
        let taken = std::mem::take(records);
        for (key, mut value) in taken {
            let old = Uuid::parse_str(&key)
                .map_err(|_| ImportError::Remap(format!("definition key {key} is not a UUID")))?;
            let new = mapping.get(&old).ok_or_else(|| {
                ImportError::Remap(format!("definition key {key} was not mapped"))
            })?;
            if let Some(record_id) = value.get_mut("id")
                && record_id.as_str() == Some(key.as_str())
            {
                *record_id = Value::String(new.to_string());
            }
            records.insert(new.to_string(), value);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeclaredReferenceKind {
    Scalar,
    List,
    External,
}

fn rewrite_declared_references(
    value: &mut Value,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<(), ImportError> {
    match value {
        Value::Array(values) => {
            for value in values {
                rewrite_declared_references(value, mapping)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                match declared_reference_kind(key) {
                    Some(DeclaredReferenceKind::Scalar) => {
                        rewrite_uuid_string(value, mapping, key)?;
                    }
                    Some(DeclaredReferenceKind::List) => {
                        let items = value.as_array_mut().ok_or_else(|| {
                            ImportError::Remap(format!(
                                "declared internal reference list {key} must be an array"
                            ))
                        })?;
                        for item in items {
                            rewrite_uuid_string(item, mapping, key)?;
                        }
                    }
                    Some(DeclaredReferenceKind::External) => {}
                    None => rewrite_declared_references(value, mapping)?,
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn declared_reference_kind(key: &str) -> Option<DeclaredReferenceKind> {
    let normalized = key
        .chars()
        .filter(|character| !matches!(character, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if matches!(
        normalized.as_str(),
        "externalid"
            | "externalids"
            | "providerexternalid"
            | "providerexternalids"
            | "sourceexternalid"
            | "sourceexternalids"
            | "legacyexternalid"
            | "legacyexternalids"
    ) {
        return Some(DeclaredReferenceKind::External);
    }
    match reference_field_tokens(key).last().map(String::as_str) {
        Some("id") => Some(DeclaredReferenceKind::Scalar),
        Some("ids") => Some(DeclaredReferenceKind::List),
        _ => None,
    }
}

fn reference_field_tokens(field: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut previous = None;
    let mut characters = field.chars().peekable();
    while let Some(character) = characters.next() {
        if !character.is_ascii_alphanumeric() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            previous = None;
            continue;
        }
        let boundary = !token.is_empty()
            && character.is_ascii_uppercase()
            && previous.is_some_and(|previous: char| {
                previous.is_ascii_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_ascii_uppercase()
                        && characters.peek().is_some_and(char::is_ascii_lowercase))
            });
        if boundary {
            tokens.push(std::mem::take(&mut token));
        }
        token.push(character.to_ascii_lowercase());
        previous = Some(character);
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn reject_stale_declared_references(
    value: &Value,
    mapping: &BTreeMap<Uuid, Uuid>,
) -> Result<(), ImportError> {
    match value {
        Value::Array(values) => {
            for value in values {
                reject_stale_declared_references(value, mapping)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                match declared_reference_kind(key) {
                    Some(DeclaredReferenceKind::Scalar) => {
                        reject_stale_uuid(value, mapping, key)?;
                    }
                    Some(DeclaredReferenceKind::List) => {
                        let items = value.as_array().ok_or_else(|| {
                            ImportError::Remap(format!(
                                "declared internal reference list {key} must be an array"
                            ))
                        })?;
                        for item in items {
                            reject_stale_uuid(item, mapping, key)?;
                        }
                    }
                    Some(DeclaredReferenceKind::External) => {}
                    None => reject_stale_declared_references(value, mapping)?,
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn reject_stale_uuid(
    value: &Value,
    mapping: &BTreeMap<Uuid, Uuid>,
    key: &str,
) -> Result<(), ImportError> {
    let text = value.as_str().ok_or_else(|| {
        ImportError::Remap(format!(
            "declared internal reference {key} must be a string"
        ))
    })?;
    let Ok(identity) = Uuid::parse_str(text) else {
        return Ok(());
    };
    if mapping.contains_key(&identity) {
        return Err(ImportError::Remap(format!(
            "declared internal reference still targets copied identity {identity}"
        )));
    }
    Ok(())
}

fn rewrite_uuid_string(
    value: &mut Value,
    mapping: &BTreeMap<Uuid, Uuid>,
    key: &str,
) -> Result<(), ImportError> {
    let text = value.as_str().ok_or_else(|| {
        ImportError::Remap(format!(
            "declared internal reference {key} must be a string"
        ))
    })?;
    if let Ok(old) = Uuid::parse_str(text)
        && let Some(new) = mapping.get(&old)
    {
        *value = Value::String(new.to_string());
    }
    Ok(())
}

fn deterministic_uuid_v7(bundle_id: BundleId, file_sha256: &str, old: Uuid, salt: u32) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(b"eutheto/create-copy/v1");
    hash.update(bundle_id.as_uuid().as_bytes());
    hash.update(file_sha256.as_bytes());
    hash.update(old.as_bytes());
    hash.update(salt.to_be_bytes());
    let digest = hash.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// Owner-private, non-extracted copy of original input bytes. The OS-random
/// temporary file is deleted on every return path when this guard drops.
#[derive(Debug)]
pub struct PrivateStagingFile {
    file: NamedTempFile,
}

impl PrivateStagingFile {
    /// Creates and durably writes a private staging copy of `bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`ImportError::ArchiveTooLarge`] at the fixed compressed-byte
    /// limit, or [`ImportError::Io`] for creation, writing, flushing, or syncing.
    pub fn create(bytes: &[u8]) -> Result<Self, ImportError> {
        if u64::try_from(bytes.len()).map_or(true, |size| size > PORTABLE_LIMITS.max_archive_bytes)
        {
            return Err(ImportError::ArchiveTooLarge);
        }
        let mut file = TempFileBuilder::new()
            .prefix("eutheto-import-")
            .suffix(".staging")
            .tempfile()?;
        file.write_all(bytes)?;
        file.flush()?;
        file.as_file().sync_all()?;
        Ok(Self { file })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.file.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_export::{
        ApplicationMetadata, BackupSections, BackupSelectionScope, FixedExclusion,
        FullBackupSnapshot, OmittedAssetReason, PortableBackupAssetSelection,
        ScenarioExportSnapshot, assemble_full_backup, assemble_scenario_export,
        backup_selection_extension_value, omitted_asset_placeholder,
    };
    use eutheto_types::{PackId, ScenarioDocument};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    #[test]
    fn portable_review_token_is_stable_and_binds_preview_and_plan()
    -> Result<(), Box<dyn std::error::Error>> {
        let binding = PreviewBinding {
            file_sha256: "file".repeat(16),
            options_sha256: "options".repeat(10),
            local_library_revision: Revision::new(4),
            format_version: CURRENT_BUNDLE_FORMAT_VERSION,
            schema_version: CURRENT_PORTABLE_SCHEMA_VERSION,
        };
        let scenario_id: ScenarioId = "018f1e2d-3c4b-7a69-8def-0123456789ab".parse()?;
        let plan = CollisionPlan::default();
        let stable = portable_review_token(&binding, &plan)?;
        assert_eq!(portable_review_token(&binding, &plan)?, stable);
        let mut changed_binding = binding.clone();
        changed_binding.local_library_revision = Revision::new(5);
        assert_ne!(portable_review_token(&changed_binding, &plan)?, stable);
        let changed_plan = CollisionPlan {
            scenarios: BTreeMap::from([(scenario_id, CollisionAction::Replace)]),
            supplemental: BTreeMap::new(),
        };
        assert_ne!(portable_review_token(&binding, &changed_plan)?, stable);
        let removed_action = serde_json::json!({
            "scenarios": {},
            "supplementalChoices": [{
                "section": "assets",
                "key": "asset.txt",
                "action": "create-copy"
            }]
        });
        assert!(serde_json::from_value::<CollisionPlan>(removed_action).is_err());
        Ok(())
    }

    fn portable_scenario() -> Result<ScenarioSnapshotV1, Box<dyn std::error::Error>> {
        let document: ScenarioDocument = serde_json::from_value(serde_json::json!({
            "format": "eutheto/scenario",
            "formatVersion": 1,
            "scenarioId": "018f1e2d-3c4b-7a69-8def-0123456789ab",
            "domainPack": {"id": "official.test", "schemaVersion": 1},
            "metadata": {"title": "Portable", "description": "", "createdAt": "2026-08-29T00:00:00Z", "updatedAt": "2026-08-29T00:00:00Z"},
            "settings": {
                "timeZone": "Etc/UTC", "locale": "en-US", "units": "metric",
                "horizon": {"start": "2026-08-29T00:00:00Z", "end": "2026-08-30T00:00:00Z"},
                "gapPolicy": "reject", "overlapPolicy": "earlier"
            },
            "domain": {
                "entities": {"018f1e2d-3c4b-7a69-8def-100000000001": {
                    "id": "018f1e2d-3c4b-7a69-8def-100000000001",
                    "managerId": "018f1e2d-3c4b-7a69-8def-100000000002",
                    "participantId": "018f1e2d-3c4b-7a69-8def-100000000001",
                    "locationId": "018f1e2d-3c4b-7a69-8def-100000000002",
                    "vehicleIds": [
                        "018f1e2d-3c4b-7a69-8def-100000000001",
                        "018f1e2d-3c4b-7a69-8def-100000000002"
                    ],
                    "externalId": "018f1e2d-3c4b-7a69-8def-100000000001",
                    "grid": "018f1e2d-3c4b-7a69-8def-100000000002",
                    "note": "External reference 018f1e2d-3c4b-7a69-8def-900000000001 must remain",
                    "externalUuid": "018f1e2d-3c4b-7a69-8def-900000000001"
                }},
                "rules": {"018f1e2d-3c4b-7a69-8def-100000000002": {
                    "id": "018f1e2d-3c4b-7a69-8def-100000000002",
                    "personId": "018f1e2d-3c4b-7a69-8def-100000000001",
                    "activityId": "018f1e2d-3c4b-7a69-8def-100000000001"
                }},
                "preferences": {}, "lockedAssignments": {}
            },
            "extensions": {}
        }))?;
        let mut scenario = ScenarioSnapshotV1::current(Revision::new(7), document, BTreeSet::new());
        scenario
            .extensions
            .insert("example.visual".to_owned(), serde_json::json!({"zoom": 2}));
        Ok(scenario)
    }

    #[test]
    fn historical_revisions_must_precede_current_and_can_retain_exact_results()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut current = portable_scenario()?;
        current.revision = Revision::new(5);
        let mut historical = current.clone();
        historical.project = None;
        let no_entries = BTreeMap::new();

        historical.revision = Revision::new(5);
        assert!(matches!(
            validate_bundle_references(
                std::slice::from_ref(&current),
                std::slice::from_ref(&historical),
                &no_entries,
            ),
            Err(ImportError::InvalidManifest(message))
                if message.contains("must be less than current revision 5")
        ));

        historical.revision = Revision::new(6);
        assert!(matches!(
            validate_bundle_references(
                std::slice::from_ref(&current),
                std::slice::from_ref(&historical),
                &no_entries,
            ),
            Err(ImportError::InvalidManifest(message))
                if message.contains("must be less than current revision 5")
        ));

        historical.revision = Revision::new(4);
        let result_id = "018f1e2d-3c4b-7a69-8def-012345678920";
        let entries = BTreeMap::from([(
            format!("results/{result_id}.json"),
            serde_json::to_vec(&serde_json::json!({
                "resultId": result_id,
                "scenarioId": current.document.scenario_id,
                "scenarioRevision": 4,
                "payload": {}
            }))?,
        )]);
        validate_bundle_references(
            std::slice::from_ref(&current),
            std::slice::from_ref(&historical),
            &entries,
        )?;
        Ok(())
    }

    #[test]
    fn supplemental_uuid_ownership_is_global_across_bundle_sections()
    -> Result<(), Box<dyn std::error::Error>> {
        let scenario = portable_scenario()?;
        let entity_id = "018f1e2d-3c4b-7a69-8def-100000000001";
        let entity_collision = BTreeMap::from([(
            format!("shared/{entity_id}.json"),
            serde_json::to_vec(&serde_json::json!({}))?,
        )]);
        assert!(matches!(
            validate_bundle_references(
                std::slice::from_ref(&scenario),
                &[],
                &entity_collision
            ),
            Err(ImportError::InvalidManifest(message))
                if message.contains("owned identity")
        ));

        let result_id = "018f1e2d-3c4b-7a69-8def-200000000001";
        let result_collision = BTreeMap::from([
            (
                format!("results/{result_id}.json"),
                serde_json::to_vec(&serde_json::json!({
                    "resultId": result_id,
                    "scenarioId": scenario.document.scenario_id,
                    "scenarioRevision": scenario.revision.value(),
                    "payload": {}
                }))?,
            ),
            (
                format!("preferences/{result_id}.json"),
                serde_json::to_vec(&serde_json::json!({}))?,
            ),
        ]);
        assert!(
            validate_bundle_references(std::slice::from_ref(&scenario), &[], &result_collision)
                .is_err()
        );

        let shared_asset_id = "018f1e2d-3c4b-7a69-8def-200000000002";
        let shared_asset_collision = BTreeMap::from([
            (
                format!("shared/{shared_asset_id}.json"),
                serde_json::to_vec(&serde_json::json!({}))?,
            ),
            (
                format!("assets/{shared_asset_id}.txt"),
                b"synthetic inert asset".to_vec(),
            ),
        ]);
        assert!(
            validate_bundle_references(
                std::slice::from_ref(&scenario),
                &[],
                &shared_asset_collision
            )
            .is_err()
        );
        let same_record_id = "018f1e2d-3c4b-7a69-8def-200000000003";
        let same_record = BTreeMap::from([(
            format!("shared/{same_record_id}.json"),
            serde_json::to_vec(&serde_json::json!({
                "definitions": {
                    same_record_id: {"id": same_record_id}
                }
            }))?,
        )]);
        validate_bundle_references(std::slice::from_ref(&scenario), &[], &same_record)?;
        Ok(())
    }

    fn library_selection_extensions(
        include_results: bool,
    ) -> Result<BTreeMap<String, Value>, Box<dyn std::error::Error>> {
        let selection = BackupSelection {
            include_results,
            asset_selection: PortableBackupAssetSelection::All,
            threshold_version: None,
            threshold_bytes: None,
            excluded_asset_count: 0,
            excluded_asset_ids: BTreeSet::new(),
            fixed_exclusions: FixedExclusion::ALL.into_iter().collect(),
            scope: BackupSelectionScope::Library,
        };
        Ok(BTreeMap::from([(
            eutheto_export::BACKUP_SELECTION_EXTENSION.to_owned(),
            backup_selection_extension_value(&selection)?,
        )]))
    }

    fn valid_bundle() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(assemble_scenario_export(
            &ScenarioExportSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0001,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Portable".to_owned(),
                scenario: portable_scenario()?,
                scenario_revisions: Vec::new(),
                sections: BackupSections::default(),
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: BTreeMap::new(),
            },
            &portable_encode::encode_fixture_domain,
        )?)
    }

    fn inspect(bytes: &[u8]) -> Result<InspectedBundle, ImportError> {
        inspect_bundle(
            bytes,
            &InspectionPolicy::default(),
            &MigrationRegistries::current_only(),
            &portable_decode::decode_fixture_domain,
        )
    }

    #[test]
    fn current_and_historical_bundle_payloads_require_scenario_reference_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut current = portable_scenario()?;
        current.revision = Revision::new(2);
        current.extensions.insert(
            "example.link".to_owned(),
            serde_json::json!({"scenarioId": current.document.scenario_id}),
        );
        let mut historical = current.clone();
        historical.revision = Revision::new(1);
        let bytes = assemble_scenario_export(
            &ScenarioExportSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0001,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Reference closure".to_owned(),
                scenario: current,
                scenario_revisions: vec![historical],
                sections: BackupSections::default(),
                nonsemantic_extensions: BTreeSet::from(["example.link".to_owned()]),
                manifest_extensions: BTreeMap::new(),
            },
            &portable_encode::encode_fixture_domain,
        )?;
        let valid = inspect(&bytes)?;
        assert_eq!(valid.scenario_revisions.len(), 1);
        let entries = read_archive_entries(&bytes, &InspectionPolicy::default())?;
        for prefix in ["scenarios/", "scenario-revisions/"] {
            let path = entries
                .keys()
                .find(|path| path.starts_with(prefix))
                .ok_or("missing scenario entry")?;
            let mut changed = entries.clone();
            let mut value: Value = serde_json::from_slice(&changed[path])?;
            value["extensions"]["example.link"]["scenarioId"] =
                Value::String("018f1e2d-3c4b-7a69-8def-400000000099".to_owned());
            let payload = canonical_json(&value)?;
            let mut checksums: Checksums = serde_json::from_slice(&changed[CHECKSUMS_PATH])?;
            checksums.files.insert(path.clone(), sha256_hex(&payload));
            changed.insert(path.clone(), payload);
            changed.insert(CHECKSUMS_PATH.to_owned(), canonical_json(&checksums)?);
            let archive_entries = changed
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
                .collect::<Vec<_>>();
            let malformed = raw_zip(&archive_entries, CompressionMethod::Stored)?;
            assert!(
                matches!(inspect(&malformed), Err(ImportError::InvalidManifest(_))),
                "{prefix} admitted an absent scenario reference"
            );
        }
        Ok(())
    }

    #[test]
    fn standalone_scenario_migrates_v1_and_preserves_nonsemantic_namespaces()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = portable_scenario()?;
        source.extensions.insert(
            "example.wrapper".to_owned(),
            serde_json::json!({"note": "kept"}),
        );
        source.document.extensions.insert(
            "example.document".to_owned(),
            serde_json::json!({"note": "also kept"}),
        );
        let migrations = MigrationRegistries::new(
            Vec::new(),
            vec![PortableMigrationStep {
                from_version: 1,
                to_version: 2,
                name: "fixture-v1-to-v2",
                migrate: Arc::new(portable_migrate::migrate_fixture_v1),
            }],
        )?;
        let inspected = inspect_scenario(
            &canonical_json(&source)?,
            &InspectionPolicy::default(),
            &migrations,
            &portable_decode::decode_fixture_domain,
        )?;
        assert_eq!(inspected.scenario, source);
        assert_eq!(inspected.original_schema_version, 1);
        assert!(inspected.applied_migrations.iter().any(|migration| {
            migration.registry == MigrationRegistryKind::Portable
                && migration.from_version == 1
                && migration.to_version == 2
        }));
        Ok(())
    }

    #[test]
    fn standalone_scenario_rejects_external_dependencies_and_undeclared_semantics()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = portable_scenario()?;
        let mut wire = PortableScenario::from_snapshot(
            &source,
            portable_encode::encode_fixture_domain(&source.document)?,
        )?;
        wire.extensions.insert(
            "example.reference".to_owned(),
            serde_json::json!({"scenarioId": "018f1e2d-3c4b-7a69-8def-200000000002"}),
        );
        assert!(matches!(
            inspect_scenario(
                &canonical_json(&wire)?,
                &InspectionPolicy::default(),
                &MigrationRegistries::current_only(),
                &portable_decode::decode_fixture_domain,
            ),
            Err(ImportError::InvalidManifest(_))
        ));
        wire.extensions.clear();
        wire.semantic_extensions
            .insert("example.unknown".to_owned(), serde_json::json!({}));
        assert!(matches!(
            inspect_scenario(
                &canonical_json(&wire)?,
                &InspectionPolicy::default(),
                &MigrationRegistries::current_only(),
                &portable_decode::decode_fixture_domain,
            ),
            Err(ImportError::UnsupportedCapability { .. })
        ));
        Ok(())
    }

    #[test]
    fn standalone_scenario_rejects_duplicate_keys_before_typed_decode()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = portable_scenario()?;
        let wire = PortableScenario::from_snapshot(
            &source,
            portable_encode::encode_fixture_domain(&source.document)?,
        )?;
        let json = String::from_utf8(canonical_json(&wire)?)?;
        let duplicate = json.replacen('{', "{\"schemaVersion\":2,", 1);
        assert!(matches!(
            inspect_scenario(
                duplicate.as_bytes(),
                &InspectionPolicy::default(),
                &MigrationRegistries::current_only(),
                &portable_decode::decode_fixture_domain,
            ),
            Err(ImportError::InvalidJson { .. })
        ));
        Ok(())
    }

    #[test]
    fn decoded_snapshots_cannot_exceed_the_source_aggregate_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = valid_bundle()?;
        let mut policy = InspectionPolicy::default();
        policy.limits.max_collection_items = 256;
        assert_eq!(
            inspect_bundle(
                &bytes,
                &policy,
                &MigrationRegistries::current_only(),
                &portable_decode::decode_fixture_domain
            )?
            .scenarios,
            vec![portable_scenario()?],
        );
        let expanded = inspect_bundle(
            &bytes,
            &policy,
            &MigrationRegistries::current_only(),
            &|wire| {
                let mut decoded = portable_decode::decode_fixture_domain(wire)?;
                let entity = decoded
                    .document
                    .domain
                    .entities
                    .values_mut()
                    .next()
                    .ok_or_else(|| {
                        MigrationFailure::Invalid("fixture entity is missing".to_owned())
                    })?;
                entity["expanded"] = serde_json::json!([vec![0; 160], vec![0; 160]]);
                Ok(decoded)
            },
        );
        assert!(matches!(
            expanded,
            Err(ImportError::JsonLimit {
                limit: "aggregate nodes",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn migration_outputs_cannot_exceed_the_source_aggregate_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = portable_scenario()?;
        let mut manifest = inspect(&valid_bundle()?)?.manifest;
        manifest.schema_version = 1;
        manifest.required_capabilities = source.required_capabilities.clone();
        let path = format!("scenarios/{}.json", source.document.scenario_id);
        let bytes = rebuilt_bundle(&manifest, &path, &canonical_json(&source)?)?;
        let migrations = MigrationRegistries::new(
            Vec::new(),
            vec![PortableMigrationStep {
                from_version: 1,
                to_version: 2,
                name: "expanding-fixture",
                migrate: Arc::new(|value| {
                    let mut outcome = portable_migrate::migrate_fixture_v1(value)?;
                    let entity = outcome.value["domain"]["payload"]["entities"]
                        .as_object_mut()
                        .and_then(|entities| entities.values_mut().next())
                        .ok_or_else(|| {
                            MigrationFailure::Invalid("fixture entity is missing".to_owned())
                        })?;
                    entity["expanded"] = serde_json::json!([vec![0; 160], vec![0; 160]]);
                    Ok(outcome)
                }),
            }],
        )?;
        let mut policy = InspectionPolicy::default();
        policy.limits.max_collection_items = 256;
        assert!(matches!(
            inspect_bundle(
                &bytes,
                &policy,
                &migrations,
                &portable_decode::decode_fixture_domain
            ),
            Err(ImportError::JsonLimit {
                limit: "aggregate nodes",
                ..
            }),
        ));
        Ok(())
    }

    #[test]
    fn an_entry_version_cannot_be_laundered_through_the_global_migration()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = portable_scenario()?;
        let mut manifest = inspect(&valid_bundle()?)?.manifest;
        // Requirements match the actual v1 source; only the manifest's v2 version lies.
        manifest.required_capabilities = source.required_capabilities.clone();
        let path = format!("scenarios/{}.json", source.document.scenario_id);
        let bytes = rebuilt_bundle(&manifest, &path, &canonical_json(&source)?)?;
        let migrations = MigrationRegistries::new(
            Vec::new(),
            vec![PortableMigrationStep {
                from_version: 1,
                to_version: 2,
                name: "fixture-v1-to-v2",
                migrate: Arc::new(portable_migrate::migrate_fixture_v1),
            }],
        )?;
        assert!(matches!(
            inspect_bundle(
                &bytes,
                &InspectionPolicy::default(),
                &migrations,
                &portable_decode::decode_fixture_domain
            ),
            Err(ImportError::InvalidScenario { .. }),
        ));
        Ok(())
    }

    #[test]
    fn source_capabilities_are_exact_and_pack_support_cannot_authorize_the_host()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = portable_scenario()?;
        let manifest = inspect(&valid_bundle()?)?.manifest;
        let path = format!("scenarios/{}.json", source.document.scenario_id);
        let wire = PortableScenario::from_snapshot(
            &source,
            portable_encode::encode_fixture_domain(&source.document)?,
        )?;
        let mut extra = manifest.clone();
        extra.required_capabilities.insert(SemanticCapability {
            id: "example.extra".to_owned(),
            version: 1,
        });
        assert!(matches!(
            inspect(&rebuilt_bundle(&extra, &path, &canonical_json(&wire)?)?),
            Err(ImportError::InvalidManifest(_)),
        ));
        let mut undeclared = wire.clone();
        undeclared.domain.required_capabilities.clear();
        assert!(matches!(
            inspect(&rebuilt_bundle(
                &manifest,
                &path,
                &canonical_json(&undeclared)?
            )?),
            Err(ImportError::InvalidManifest(_)),
        ));
        let mut wrong_scope = wire;
        wrong_scope.required_capabilities = wrong_scope.domain.required_capabilities.clone();
        assert!(matches!(
            inspect(&rebuilt_bundle(
                &manifest,
                &path,
                &canonical_json(&wrong_scope)?
            )?),
            Err(ImportError::UnsupportedCapability { .. }),
        ));
        Ok(())
    }

    #[test]
    fn domain_decoder_cannot_replace_host_identity() -> Result<(), Box<dyn std::error::Error>> {
        let replacement = "018f1e2d-3c4b-7a69-8def-0123456789ac".parse()?;
        let result = inspect_bundle(
            &valid_bundle()?,
            &InspectionPolicy::default(),
            &MigrationRegistries::current_only(),
            &|wire| {
                let mut decoded = portable_decode::decode_fixture_domain(wire)?;
                decoded.document.scenario_id = replacement;
                Ok(decoded)
            },
        );
        assert!(matches!(result, Err(ImportError::Migration(_))));
        Ok(())
    }

    #[test]
    fn unopened_bundle_preserves_exact_bytes_without_import_claim()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = valid_bundle()?;
        let unopened =
            inspect_unopened_bundle_for_exact_reexport(&bytes, &InspectionPolicy::default())?;
        assert_eq!(unopened.exact_bytes(), bytes);
        assert_eq!(unopened.metadata().format, BUNDLE_FORMAT);
        assert_eq!(unopened.metadata().title.as_deref(), Some("Portable"));
        assert_eq!(unopened.metadata().scenarios.len(), 1);
        assert_eq!(
            unopened.metadata().scenarios[0].pack_id.as_deref(),
            Some("official.test")
        );
        assert_eq!(unopened.into_exact_bytes().as_ref(), bytes);
        Ok(())
    }

    #[test]
    fn unopened_bundle_preserves_newer_semantics_but_normal_import_rejects_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let policy = InspectionPolicy::default();
        let mut entries = read_archive_entries(&valid_bundle()?, &policy)?;
        let manifest_bytes = entries
            .get(MANIFEST_PATH)
            .ok_or("fixture manifest is missing")?;
        let mut manifest: Value = serde_json::from_slice(manifest_bytes)?;
        let manifest_object = manifest
            .as_object_mut()
            .ok_or("manifest is not an object")?;
        manifest_object.insert(
            "schemaVersion".to_owned(),
            Value::from(CURRENT_PORTABLE_SCHEMA_VERSION.saturating_add(1)),
        );
        manifest_object.insert(
            "requiredCapabilities".to_owned(),
            serde_json::json!([{"id": "future.semantic", "version": 1}]),
        );
        let updated_manifest = canonical_json(&manifest)?;
        entries.insert(MANIFEST_PATH.to_owned(), updated_manifest.clone());

        let checksums_bytes = entries
            .get(CHECKSUMS_PATH)
            .ok_or("fixture checksums are missing")?;
        let mut checksums: Checksums = serde_json::from_slice(checksums_bytes)?;
        checksums
            .files
            .insert(MANIFEST_PATH.to_owned(), sha256_hex(&updated_manifest));
        entries.insert(CHECKSUMS_PATH.to_owned(), canonical_json(&checksums)?);
        let archive_entries = entries
            .iter()
            .map(|(path, content)| (path.as_str(), content.as_slice()))
            .collect::<Vec<_>>();
        let newer = raw_zip(&archive_entries, CompressionMethod::Deflated)?;

        assert!(matches!(
            inspect(&newer),
            Err(ImportError::UnsupportedNewerVersion { .. })
        ));
        let unopened =
            inspect_unopened_bundle_for_exact_reexport(&newer, &InspectionPolicy::default())?;
        assert_eq!(unopened.exact_bytes(), newer);
        assert_eq!(
            unopened.metadata().portable_schema_version,
            CURRENT_PORTABLE_SCHEMA_VERSION.saturating_add(1)
        );
        assert_eq!(
            unopened.metadata().required_capabilities,
            vec![PreservedCapabilityMetadata {
                id: "future.semantic".to_owned(),
                version: 1,
            }]
        );
        Ok(())
    }

    fn supplemental_bundle() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let scenario = portable_scenario()?;
        let scenario_id = scenario.document.scenario_id;
        let mut sections = BackupSections::default();
        sections.results.insert(
            "018f1e2d-3c4b-7a69-8def-100000000007".to_owned(),
            serde_json::json!({
                "resultId": "018f1e2d-3c4b-7a69-8def-100000000007",
                "scenarioId": scenario_id,
                "scenarioRevision": 7,
                "payload": {
                    "note": "external 018f1e2d-3c4b-7a69-8def-900000000001",
                    "definitions": {
                        "018f1e2d-3c4b-7a69-8def-100000000009": {
                            "id": "018f1e2d-3c4b-7a69-8def-100000000009"
                        }
                    }
                }
            }),
        );
        sections.shared_records.insert(
            "shared".to_owned(),
            serde_json::json!({
                "scenarioId": scenario_id,
                "safe": true,
                "definitions": {
                    "018f1e2d-3c4b-7a69-8def-100000000008": {
                        "id": "018f1e2d-3c4b-7a69-8def-100000000008"
                    }
                }
            }),
        );
        sections.assets.insert(
            "notes.txt".to_owned(),
            PortableAsset {
                bytes: b"safe notes".to_vec(),
                media_type: "text/plain; charset=utf-8".to_owned(),
                redistribution_permitted: true,
            },
        );
        Ok(assemble_scenario_export(
            &ScenarioExportSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0002,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Supplemental".to_owned(),
                scenario,
                scenario_revisions: Vec::new(),
                sections,
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: BTreeMap::new(),
            },
            &portable_encode::encode_fixture_domain,
        )?)
    }

    fn asset_bundle() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut scenario = portable_scenario()?;
        scenario.extensions.insert(
            "example.visual".to_owned(),
            serde_json::json!({"asset": "notes.txt"}),
        );
        let mut sections = BackupSections::default();
        sections.assets.insert(
            "notes.txt".to_owned(),
            PortableAsset {
                bytes: b"safe notes".to_vec(),
                media_type: "text/plain; charset=utf-8".to_owned(),
                redistribution_permitted: true,
            },
        );
        Ok(assemble_scenario_export(
            &ScenarioExportSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0011,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Asset-bearing".to_owned(),
                scenario,
                scenario_revisions: Vec::new(),
                sections,
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: BTreeMap::new(),
            },
            &portable_encode::encode_fixture_domain,
        )?)
    }

    fn revisioned_result_backup(
        source_revision: Revision,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut scenario = portable_scenario()?;
        scenario.revision = source_revision;
        let scenario_id = scenario.document.scenario_id;
        let mut sections = BackupSections::default();
        sections.results.insert(
            "018f1e2d-3c4b-7a69-8def-100000000099".to_owned(),
            serde_json::json!({
                "resultId": "018f1e2d-3c4b-7a69-8def-100000000099",
                "scenarioId": scenario_id,
                "scenarioRevision": source_revision
            }),
        );
        let scenario_revisions = if source_revision > Revision::INITIAL {
            let mut historical = scenario.clone();
            historical.revision = Revision::INITIAL;
            historical.project = None;
            vec![historical]
        } else {
            Vec::new()
        };
        Ok(assemble_full_backup(
            &FullBackupSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0013,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Revisioned result".to_owned(),
                scenarios: vec![scenario],
                scenario_revisions,
                sections,
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: library_selection_extensions(true)?,
            },
            &portable_encode::encode_fixture_domain,
        )?)
    }

    #[test]
    // One sequential flow is necessary to prove the same identity cannot regress across states.
    #[allow(clippy::too_many_lines)]
    fn scenario_revision_high_water_prevents_aba_and_preserves_result_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let inspected = inspect(&revisioned_result_backup(Revision::new(2))?)?;
        let scenario_id = inspected.scenarios[0].document.scenario_id;
        let options = ImportOptions {
            restore_mode: RestoreMode::ReplaceLibrary,
            include_results: true,
            include_assets: true,
        };
        let current_local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::new(9),
            scenario_ids: BTreeSet::from([scenario_id]),
            scenario_revision_high_water: BTreeMap::from([(scenario_id, Revision::new(5))]),
            occupied_uuids: BTreeSet::from([scenario_id.to_string()]),
            scenarios: vec![LocalScenarioSnapshot {
                scenario_id,
                title: "Current local".to_owned(),
                revision: Revision::new(5),
                archived: false,
                owned_uuids: BTreeSet::from([scenario_id.as_uuid()]),
            }],
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let preview = build_preview(&inspected, &options, &current_local)?;
        assert_eq!(preview.scenarios[0].source_revision, Revision::new(2));
        assert_eq!(
            preview.scenarios[0].same_identity_revision,
            Revision::new(6)
        );
        assert!(
            preview.scenarios[0]
                .same_identity_revision_warning
                .is_some()
        );
        let staged = stage_import(
            &inspected,
            &preview,
            &options,
            &current_local,
            &CollisionPlan::default(),
        )?;
        assert_eq!(staged.scenarios[0].disposition, StagedDisposition::Replace);
        assert_eq!(staged.scenarios[0].source_revision, Revision::new(2));
        assert_eq!(staged.scenarios[0].scenario.revision, Revision::new(6));
        assert!(staged.scenario_revisions.iter().any(|historical| {
            historical.document.scenario_id == scenario_id
                && historical.revision == Revision::new(2)
        }));
        assert_eq!(staged.scenario_revisions.len(), 1);
        let result: Value = serde_json::from_slice(
            staged
                .results
                .first_key_value()
                .ok_or_else(|| std::io::Error::other("retained result missing"))?
                .1,
        )?;
        assert_eq!(
            extract_result_dependency(&result)?.scenario_revision,
            Revision::new(2)
        );
        let without_results = ImportOptions {
            include_results: false,
            ..options.clone()
        };
        let without_results_preview = build_preview(&inspected, &without_results, &current_local)?;
        let without_results_staged = stage_import(
            &inspected,
            &without_results_preview,
            &without_results,
            &current_local,
            &CollisionPlan::default(),
        )?;
        assert!(without_results_staged.results.is_empty());
        assert!(without_results_staged.scenario_revisions.is_empty());

        let result_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000099")?;
        let result_identity = SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key: format!("{result_id}.json"),
        };
        let mut skip_local = current_local.clone();
        skip_local.occupied_uuids.insert(result_id.to_string());
        skip_local
            .supplemental_identities
            .insert(result_identity.clone());
        let skip_options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            ..options.clone()
        };
        let skip_preview = build_preview(&inspected, &skip_options, &skip_local)?;
        let skipped_result = stage_import(
            &inspected,
            &skip_preview,
            &skip_options,
            &skip_local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::Replace)]),
                supplemental: BTreeMap::from([(
                    result_identity,
                    SupplementalCollisionAction::Skip,
                )]),
            },
        )?;
        assert!(skipped_result.results.is_empty());
        assert!(skipped_result.scenario_revisions.is_empty());

        let mut fresh = current_local.clone();
        fresh.scenario_ids.clear();
        fresh.scenario_revision_high_water.clear();
        fresh.occupied_uuids.clear();
        fresh.scenarios.clear();
        let fresh_preview = build_preview(&inspected, &options, &fresh)?;
        assert_eq!(
            fresh_preview.scenarios[0].same_identity_revision,
            Revision::new(2)
        );
        let fresh_staged = stage_import(
            &inspected,
            &fresh_preview,
            &options,
            &fresh,
            &CollisionPlan::default(),
        )?;
        assert_eq!(
            fresh_staged.scenarios[0].disposition,
            StagedDisposition::Create
        );
        assert_eq!(
            fresh_staged.scenarios[0].scenario.revision,
            Revision::new(2)
        );
        assert!(fresh_staged.scenario_revisions.is_empty());

        let mut tombstoned = fresh.clone();
        tombstoned
            .scenario_revision_high_water
            .insert(scenario_id, Revision::new(5));
        tombstoned.occupied_uuids.insert(scenario_id.to_string());
        for identity in collect_scenario_owned_uuids(&inspected.scenarios[0]) {
            tombstoned.occupied_uuids.insert(identity.to_string());
            tombstoned.identity_owners.insert(identity, scenario_id);
        }
        let tombstone_preview = build_preview(&inspected, &options, &tombstoned)?;
        assert_eq!(
            tombstone_preview.scenarios[0].same_identity_revision,
            Revision::new(6)
        );
        let resurrected = stage_import(
            &inspected,
            &tombstone_preview,
            &options,
            &tombstoned,
            &CollisionPlan::default(),
        )?;
        assert_eq!(
            resurrected.scenarios[0].disposition,
            StagedDisposition::Create
        );
        assert_eq!(resurrected.scenarios[0].scenario.revision, Revision::new(6));

        let other_id: ScenarioId = "018f1e2d-3c4b-7a69-8def-0123456789ac".parse()?;
        let mut nested_owner = tombstoned.clone();
        nested_owner
            .identity_owners
            .insert(scenario_id.as_uuid(), other_id);
        nested_owner.scenario_ids.insert(other_id);
        nested_owner.scenarios.push(LocalScenarioSnapshot {
            scenario_id: other_id,
            title: "Other live owner".to_owned(),
            revision: Revision::INITIAL,
            archived: false,
            owned_uuids: BTreeSet::from([other_id.as_uuid(), scenario_id.as_uuid()]),
        });
        let nested_options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            ..options.clone()
        };
        let nested_preview = build_preview(&inspected, &nested_options, &nested_owner)?;
        assert!(nested_preview.scenarios[0].collides);
        assert!(matches!(
            stage_import(
                &inspected,
                &nested_preview,
                &nested_options,
                &nested_owner,
                &CollisionPlan::default(),
            ),
            Err(ImportError::UnresolvedCollision(id)) if id == scenario_id
        ));

        let newer = inspect(&revisioned_result_backup(Revision::new(8))?)?;
        let newer_preview = build_preview(&newer, &options, &current_local)?;
        assert_eq!(
            newer_preview.scenarios[0].same_identity_revision,
            Revision::new(8)
        );
        let newer_staged = stage_import(
            &newer,
            &newer_preview,
            &options,
            &current_local,
            &CollisionPlan::default(),
        )?;
        assert_eq!(
            newer_staged.scenarios[0].scenario.revision,
            Revision::new(8)
        );

        let mut maximum = tombstoned.clone();
        maximum.scenario_revision_high_water.insert(
            scenario_id,
            Revision::try_new(eutheto_types::REVISION_MAX_V1)?,
        );
        assert!(matches!(
            build_preview(&inspected, &options, &maximum),
            Err(ImportError::RevisionOverflow { scenario_id: found }) if found == scenario_id
        ));

        let mut conflicting = inspect(&revisioned_result_backup(Revision::new(2))?)?;
        let mut conflicting_history = conflicting.scenarios[0].clone();
        conflicting_history.project = None;
        conflicting_history.document.metadata.title = "Conflicting source history".to_owned();
        conflicting.scenario_revisions.push(conflicting_history);
        let conflicting_preview = build_preview(&conflicting, &options, &current_local)?;
        assert!(matches!(
            stage_import(
                &conflicting,
                &conflicting_preview,
                &options,
                &current_local,
                &CollisionPlan::default(),
            ),
            Err(ImportError::ConflictingScenarioRevision {
                scenario_id: found,
                revision
            }) if found == scenario_id && revision == Revision::new(2)
        ));
        Ok(())
    }

    #[test]
    fn replace_library_preflights_durable_identity_owners() -> Result<(), Box<dyn std::error::Error>>
    {
        let inspected = inspect(&valid_bundle()?)?;
        let incoming_id = inspected.scenarios[0].document.scenario_id;
        let other_id: ScenarioId = "018f1e2d-3c4b-7a69-8def-0123456789ac".parse()?;
        let conflicting_identity = collect_scenario_owned_uuids(&inspected.scenarios[0])
            .into_iter()
            .find(|identity| *identity != incoming_id.as_uuid())
            .ok_or_else(|| std::io::Error::other("scenario-owned test identity missing"))?;
        let options = ImportOptions {
            restore_mode: RestoreMode::ReplaceLibrary,
            include_results: true,
            include_assets: true,
        };
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::from([(conflicting_identity, other_id)]),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([conflicting_identity.to_string()]),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        assert!(matches!(
            build_preview(&inspected, &options, &local),
            Err(ImportError::InvalidRestore(message))
                if message.contains("durably owned by another scenario family")
        ));

        let supplemental = inspect(&supplemental_bundle()?)?;
        let supplemental_identity = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000008")?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::from([(supplemental_identity, other_id)]),
            occupied_uuids: BTreeSet::from([supplemental_identity.to_string()]),
            ..local
        };
        assert!(matches!(
            build_preview(&supplemental, &options, &local),
            Err(ImportError::InvalidRestore(message))
                if message.contains("overlaps a durable scenario family")
        ));

        let removable_owner = SupplementalIdentity {
            section: SupplementalSectionKind::SharedRecords,
            key: "old.json".to_owned(),
        };
        let removable = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::from([(
                supplemental_identity,
                removable_owner.clone(),
            )]),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([supplemental_identity.to_string()]),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::from([removable_owner]),
            settings: BTreeMap::new(),
        };
        build_preview(&supplemental, &options, &removable)?;
        validate_local_supplemental_uuid_collisions(
            &supplemental,
            &options,
            &removable,
            &CollisionPlan::default(),
        )?;
        Ok(())
    }

    fn migrate_outer_v0(mut bundle: LogicalBundle) -> Result<LogicalBundle, MigrationFailure> {
        let manifest = bundle
            .manifest
            .as_object_mut()
            .ok_or_else(|| MigrationFailure::Invalid("manifest must be an object".to_owned()))?;
        manifest.insert(
            "formatVersion".to_owned(),
            Value::from(CURRENT_BUNDLE_FORMAT_VERSION),
        );
        Ok(bundle)
    }

    fn migrate_portable_v0(mut value: Value) -> Result<PortableMigrationOutcome, MigrationFailure> {
        let scenario = value
            .as_object_mut()
            .ok_or_else(|| MigrationFailure::Invalid("scenario must be an object".to_owned()))?;
        scenario.insert("schemaVersion".to_owned(), Value::from(1));
        Ok(PortableMigrationOutcome {
            value,
            introduced_requirements: BTreeSet::new(),
            applied_migrations: Vec::new(),
        })
    }

    fn raw_zip(
        entries: &[(&str, &[u8])],
        compression: CompressionMethod,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(compression)
            .unix_permissions(0o600);
        for (path, bytes) in entries {
            writer.start_file(*path, options)?;
            writer.write_all(bytes)?;
        }
        Ok(writer.finish()?.into_inner())
    }

    #[test]
    fn forged_deflate_size_is_rejected_before_crc_or_payload_validation()
    -> Result<(), Box<dyn std::error::Error>> {
        let policy = InspectionPolicy::default();
        let entries = read_archive_entries(&valid_bundle()?, &policy)?;
        let path = entries
            .keys()
            .find(|path| path.starts_with("scenarios/"))
            .ok_or("missing scenario fixture")?;
        let inflated = vec![b' '; 64 * 1024];
        let mut payloads = vec![(path.as_str(), inflated.as_slice())];
        payloads.extend(
            entries
                .iter()
                .filter(|(name, _)| *name != path)
                .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
        );
        let mut bytes = raw_zip(&payloads, CompressionMethod::Deflated)?;
        let central = zip32_central_directory(&bytes, policy.limits.max_entries)?.offset;
        let bad_crc = (zip_u32(&bytes, 14)? ^ 1).to_le_bytes();
        bytes[14..18].copy_from_slice(&bad_crc);
        bytes[22..26].copy_from_slice(&1_u32.to_le_bytes());
        bytes[central + 16..central + 20].copy_from_slice(&bad_crc);
        bytes[central + 24..central + 28].copy_from_slice(&1_u32.to_le_bytes());

        let semantic = inspect_bundle(
            &bytes,
            &policy,
            &MigrationRegistries::current_only(),
            &portable_decode::decode_fixture_domain,
        )
        .map(|_| ());
        let unopened = inspect_unopened_bundle_for_exact_reexport(&bytes, &policy).map(|_| ());
        for result in [semantic, unopened] {
            assert!(
                matches!(&result, Err(ImportError::EntryTooLarge { path: rejected }) if rejected == path),
                "{result:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn bounded_entry_read_stops_at_one_overflow_byte() {
        let mut reader = Cursor::new(b"oversized decompressed content");
        let mut buffer = [0_u8; 16 * 1024];
        let mut remaining = 128;
        let mut retained = Vec::new();
        let result = consume_bounded_zip_entry(
            &mut reader,
            &mut buffer,
            "scenario.json",
            5,
            128,
            &mut remaining,
            |chunk| retained.extend_from_slice(chunk),
        );
        assert!(matches!(result, Err(ImportError::EntryTooLarge { .. })));
        assert_eq!(reader.position(), 6);
        assert!(retained.len() <= 5);
    }

    fn raw_stored_zip_allowing_duplicate_paths(
        entries: &[(&str, &[u8])],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        const UTF8_NAME: u16 = 1 << 11;
        const VERSION_NEEDED: u16 = 20;
        const UNIX_VERSION_MADE_BY: u16 = (3 << 8) | VERSION_NEEDED;
        const DOS_DATE_1980_01_01: u16 = (1 << 5) | 1;

        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = u32::MAX;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
                }
            }
            !crc
        }

        let mut local_entries = Vec::new();
        let mut central_directory = Vec::new();
        for &(path, content) in entries {
            let path = path.as_bytes();
            let path_len = u16::try_from(path.len())?;
            let content_len = u32::try_from(content.len())?;
            let local_offset = u32::try_from(local_entries.len())?;
            let crc = crc32(content);

            local_entries.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
            local_entries.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
            local_entries.extend_from_slice(&UTF8_NAME.to_le_bytes());
            local_entries.extend_from_slice(&0_u16.to_le_bytes());
            local_entries.extend_from_slice(&0_u16.to_le_bytes());
            local_entries.extend_from_slice(&DOS_DATE_1980_01_01.to_le_bytes());
            local_entries.extend_from_slice(&crc.to_le_bytes());
            local_entries.extend_from_slice(&content_len.to_le_bytes());
            local_entries.extend_from_slice(&content_len.to_le_bytes());
            local_entries.extend_from_slice(&path_len.to_le_bytes());
            local_entries.extend_from_slice(&0_u16.to_le_bytes());
            local_entries.extend_from_slice(path);
            local_entries.extend_from_slice(content);

            central_directory.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
            central_directory.extend_from_slice(&UNIX_VERSION_MADE_BY.to_le_bytes());
            central_directory.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
            central_directory.extend_from_slice(&UTF8_NAME.to_le_bytes());
            central_directory.extend_from_slice(&0_u16.to_le_bytes());
            central_directory.extend_from_slice(&0_u16.to_le_bytes());
            central_directory.extend_from_slice(&DOS_DATE_1980_01_01.to_le_bytes());
            central_directory.extend_from_slice(&crc.to_le_bytes());
            central_directory.extend_from_slice(&content_len.to_le_bytes());
            central_directory.extend_from_slice(&content_len.to_le_bytes());
            central_directory.extend_from_slice(&path_len.to_le_bytes());
            central_directory.extend_from_slice(&0_u16.to_le_bytes());
            central_directory.extend_from_slice(&0_u16.to_le_bytes());
            central_directory.extend_from_slice(&0_u16.to_le_bytes());
            central_directory.extend_from_slice(&0_u16.to_le_bytes());
            central_directory.extend_from_slice(&(0o100_600_u32 << 16).to_le_bytes());
            central_directory.extend_from_slice(&local_offset.to_le_bytes());
            central_directory.extend_from_slice(path);
        }

        let entry_count = u16::try_from(entries.len())?;
        let central_offset = u32::try_from(local_entries.len())?;
        let central_size = u32::try_from(central_directory.len())?;
        local_entries.extend_from_slice(&central_directory);
        local_entries.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
        local_entries.extend_from_slice(&0_u16.to_le_bytes());
        local_entries.extend_from_slice(&0_u16.to_le_bytes());
        local_entries.extend_from_slice(&entry_count.to_le_bytes());
        local_entries.extend_from_slice(&entry_count.to_le_bytes());
        local_entries.extend_from_slice(&central_size.to_le_bytes());
        local_entries.extend_from_slice(&central_offset.to_le_bytes());
        local_entries.extend_from_slice(&0_u16.to_le_bytes());
        Ok(local_entries)
    }

    #[test]
    fn deterministic_export_import_preserves_extension() -> Result<(), Box<dyn std::error::Error>> {
        let bundle = valid_bundle()?;
        let inspected = inspect(&bundle)?;
        assert_eq!(inspected.scenarios[0].revision, Revision::new(7));
        assert_eq!(
            inspected.scenarios[0].document.domain_pack.id,
            PackId::new("official.test")?
        );
        assert_eq!(inspected.scenarios.len(), 1);
        assert_eq!(
            inspected.scenarios[0].extensions["example.visual"]["zoom"],
            2
        );
        Ok(())
    }

    #[test]
    fn full_backup_export_is_accepted_with_project_wrapper()
    -> Result<(), Box<dyn std::error::Error>> {
        let scenario = portable_scenario()?;
        let bundle = assemble_full_backup(
            &FullBackupSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0002,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Backup".to_owned(),
                scenarios: vec![scenario],
                scenario_revisions: Vec::new(),
                sections: BackupSections::default(),
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: library_selection_extensions(true)?,
            },
            &portable_encode::encode_fixture_domain,
        )?;
        let inspected = inspect(&bundle)?;
        assert_eq!(inspected.manifest.bundle_kind, BundleKind::FullBackup);
        assert!(inspected.scenarios[0].project.is_some());
        Ok(())
    }

    #[test]
    fn rejects_unsafe_duplicate_and_case_colliding_paths() -> Result<(), Box<dyn std::error::Error>>
    {
        for entries in [
            vec![("../manifest.json", b"{}".as_slice())],
            vec![
                ("Manifest.json", b"{}".as_slice()),
                ("manifest.json", b"{}".as_slice()),
            ],
            vec![
                ("MÄNIFEST.json", b"{}".as_slice()),
                ("Mänifest.json", b"{}".as_slice()),
            ],
            vec![("assets/CON.txt", b"x".as_slice())],
            vec![("assets/report.txt:stream", b"x".as_slice())],
            vec![("assets/trailing.", b"x".as_slice())],
            vec![(r"assets\backslash.txt", b"x".as_slice())],
        ] {
            let bytes = raw_zip(&entries, CompressionMethod::Stored)?;
            assert!(inspect(&bytes).is_err());
        }

        let duplicate = raw_stored_zip_allowing_duplicate_paths(&[
            (MANIFEST_PATH, b"{}".as_slice()),
            (MANIFEST_PATH, b"{}".as_slice()),
        ])?;
        assert!(matches!(
            inspect_bundle(
                &duplicate,
                &InspectionPolicy::default(),
                &MigrationRegistries::current_only(),
                &portable_decode::decode_fixture_domain,
            ),
            Err(ImportError::DuplicatePath(path)) if path == MANIFEST_PATH
        ));
        Ok(())
    }

    #[test]
    fn rejects_checksum_size_and_ratio_failures() -> Result<(), Box<dyn std::error::Error>> {
        let valid = inspect(&valid_bundle()?)?;
        let manifest_bytes = canonical_json(&valid.manifest)?;
        let scenario_path = format!("scenarios/{}.json", valid.scenarios[0].document.scenario_id);
        let scenario_bytes = canonical_json(&valid.scenarios[0])?;
        let bad_checksums = canonical_json(&Checksums {
            algorithm: CHECKSUM_ALGORITHM.to_owned(),
            files: BTreeMap::from([
                (MANIFEST_PATH.to_owned(), sha256_hex(&manifest_bytes)),
                (scenario_path.clone(), "0".repeat(64)),
            ]),
        })?;
        let bad_checksum = raw_zip(
            &[
                (MANIFEST_PATH, &manifest_bytes),
                (CHECKSUMS_PATH, &bad_checksums),
                (&scenario_path, &scenario_bytes),
            ],
            CompressionMethod::Stored,
        )?;
        assert!(matches!(
            inspect(&bad_checksum),
            Err(ImportError::ChecksumMismatch(_))
        ));

        let large = raw_zip(&[("large.json", &[0_u8; 1024])], CompressionMethod::Stored)?;
        let mut size_policy = InspectionPolicy::default();
        size_policy.limits.max_entry_bytes = 100;
        assert!(matches!(
            inspect_bundle(
                &large,
                &size_policy,
                &MigrationRegistries::current_only(),
                &portable_decode::decode_fixture_domain
            ),
            Err(ImportError::EntryTooLarge { .. })
        ));

        let compressible = vec![b'a'; 32_768];
        let compressed = raw_zip(
            &[("large.json", &compressible)],
            CompressionMethod::Deflated,
        )?;
        let mut ratio_policy = InspectionPolicy::default();
        ratio_policy.limits.max_compression_ratio = 2;
        assert!(matches!(
            inspect_bundle(
                &compressed,
                &ratio_policy,
                &MigrationRegistries::current_only(),
                &portable_decode::decode_fixture_domain,
            ),
            Err(ImportError::CompressionRatio { .. })
        ));
        Ok(())
    }

    #[test]
    fn unknown_newer_and_semantic_capability_are_rejected() -> Result<(), Box<dyn std::error::Error>>
    {
        let bundle = valid_bundle()?;
        let inspected = inspect(&bundle)?;
        let mut manifest = inspected.manifest.clone();
        manifest.format_version = CURRENT_BUNDLE_FORMAT_VERSION + 1;
        let scenario_path = format!(
            "scenarios/{}.json",
            inspected.scenarios[0].document.scenario_id
        );
        let wire = PortableScenario::from_snapshot(
            &inspected.scenarios[0],
            portable_encode::encode_fixture_domain(&inspected.scenarios[0].document)?,
        )?;
        let scenario_bytes = canonical_json(&wire)?;
        let newer = rebuilt_bundle(&manifest, &scenario_path, &scenario_bytes)?;
        assert!(matches!(
            inspect(&newer),
            Err(ImportError::UnsupportedNewerVersion { .. })
        ));

        let capability = SemanticCapability {
            id: "official.unknown.rule".to_owned(),
            version: 1,
        };
        let mut semantic_manifest = inspected.manifest;
        semantic_manifest
            .required_capabilities
            .insert(capability.clone());
        let mut semantic_scenario = wire;
        semantic_scenario
            .required_capabilities
            .insert(capability.clone());
        let semantic_scenario_bytes = canonical_json(&semantic_scenario)?;
        let semantic =
            rebuilt_bundle(&semantic_manifest, &scenario_path, &semantic_scenario_bytes)?;
        assert!(matches!(
            inspect(&semantic),
            Err(ImportError::UnsupportedCapability { .. })
        ));

        let mut policy = InspectionPolicy::default();
        policy
            .supported_capabilities
            .insert(capability.id.clone(), capability.version);
        let supported = inspect_bundle(
            &semantic,
            &policy,
            &MigrationRegistries::current_only(),
            &portable_decode::decode_fixture_domain,
        )?;
        assert_eq!(
            supported.scenarios[0].required_capabilities,
            BTreeSet::from([capability.clone()])
        );

        semantic_manifest.required_capabilities.remove(&capability);
        let undeclared =
            rebuilt_bundle(&semantic_manifest, &scenario_path, &semantic_scenario_bytes)?;
        assert!(matches!(
            inspect_bundle(
                &undeclared,
                &policy,
                &MigrationRegistries::current_only(),
                &portable_decode::decode_fixture_domain
            ),
            Err(ImportError::InvalidManifest(_))
        ));
        Ok(())
    }

    fn rebuilt_bundle(
        manifest: &BundleManifest,
        scenario_path: &str,
        scenario_bytes: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let manifest_bytes = canonical_json(manifest)?;
        let checksums = Checksums {
            algorithm: CHECKSUM_ALGORITHM.to_owned(),
            files: BTreeMap::from([
                (MANIFEST_PATH.to_owned(), sha256_hex(&manifest_bytes)),
                (scenario_path.to_owned(), sha256_hex(scenario_bytes)),
            ]),
        };
        let checksum_bytes = canonical_json(&checksums)?;
        raw_zip(
            &[
                (CHECKSUMS_PATH, &checksum_bytes),
                (MANIFEST_PATH, &manifest_bytes),
                (scenario_path, scenario_bytes),
            ],
            CompressionMethod::Stored,
        )
    }

    #[test]
    fn registered_older_versions_migrate_sequentially() -> Result<(), Box<dyn std::error::Error>> {
        let current = inspect(&valid_bundle()?)?;
        let scenario_path = format!(
            "scenarios/{}.json",
            current.scenarios[0].document.scenario_id
        );
        let mut old_scenario = serde_json::to_value(&current.scenarios[0])?;
        old_scenario["schemaVersion"] = Value::from(0);
        let old_scenario_bytes = canonical_json(&old_scenario)?;
        let mut old_manifest = current.manifest;
        old_manifest.format_version = 0;
        old_manifest.schema_version = 0;
        old_manifest.required_capabilities = current.scenarios[0].required_capabilities.clone();
        let old_bundle = rebuilt_bundle(&old_manifest, &scenario_path, &old_scenario_bytes)?;
        let registries = MigrationRegistries::new(
            vec![OuterMigrationStep {
                from_version: 0,
                to_version: CURRENT_BUNDLE_FORMAT_VERSION,
                name: "outer-v0-to-v1",
                migrate: migrate_outer_v0,
            }],
            vec![
                PortableMigrationStep {
                    from_version: 0,
                    to_version: 1,
                    name: "portable-v0-to-v1",
                    migrate: Arc::new(migrate_portable_v0),
                },
                PortableMigrationStep {
                    from_version: 1,
                    to_version: 2,
                    name: "portable-v1-to-v2",
                    migrate: Arc::new(portable_migrate::migrate_fixture_v1),
                },
            ],
        )?;
        let migrated = inspect_bundle(
            &old_bundle,
            &InspectionPolicy::default(),
            &registries,
            &portable_decode::decode_fixture_domain,
        )?;
        assert_eq!(migrated.original_format_version, 0);
        assert_eq!(migrated.original_schema_version, 0);
        assert_eq!(
            migrated.manifest.schema_version,
            CURRENT_PORTABLE_SCHEMA_VERSION
        );
        assert_eq!(migrated.scenarios, current.scenarios);
        assert_eq!(
            migrated
                .applied_migrations
                .iter()
                .map(|step| (step.registry, step.from_version, step.to_version))
                .collect::<Vec<_>>(),
            vec![
                (MigrationRegistryKind::Outer, 0, 1),
                (MigrationRegistryKind::Portable, 0, 1),
                (MigrationRegistryKind::Portable, 1, 2),
            ],
        );
        Ok(())
    }
    #[test]
    fn importer_applies_the_exporter_prohibited_data_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        let current = inspect(&valid_bundle()?)?;
        let scenario_path = format!(
            "scenarios/{}.json",
            current.scenarios[0].document.scenario_id
        );
        let wire = PortableScenario::from_snapshot(
            &current.scenarios[0],
            portable_encode::encode_fixture_domain(&current.scenarios[0].document)?,
        )?;
        for value in [
            serde_json::json!({"oauthToken": "secret"}),
            serde_json::json!({"providerAuthentication": {"value": "secret"}}),
            serde_json::json!({"auth": {"bearer": "secret"}}),
            serde_json::json!({"connectionString": "provider://credential"}),
            serde_json::json!({"headers": ["Authorization: Bearer synthetic-sentinel"]}),
            serde_json::json!({"pem": "-----BEGIN OPENSSH PRIVATE KEY-----\nsynthetic\n-----END OPENSSH PRIVATE KEY-----"}),
        ] {
            let mut prohibited = wire.clone();
            prohibited
                .extensions
                .insert("example.visual".to_owned(), value);
            let bytes = rebuilt_bundle(
                &current.manifest,
                &scenario_path,
                &canonical_json(&prohibited)?,
            )?;
            let rejection = inspect(&bytes);
            assert!(
                matches!(&rejection, Err(ImportError::InvalidManifest(_))),
                "{rejection:?}",
            );
        }
        let mut harmless = wire;
        harmless.extensions.insert(
            "example.visual".to_owned(),
            serde_json::json!({
                "authenticationStatus": "connected",
                "authenticationLabel": "Provider",
                "authenticationMethod": "browser",
                "grid": "grid",
                "tokenizer": "word-boundary",
                "monkey": "semantic label"
            }),
        );
        let bytes = rebuilt_bundle(
            &current.manifest,
            &scenario_path,
            &canonical_json(&harmless)?,
        )?;
        inspect(&bytes)?;
        Ok(())
    }

    #[test]
    fn preview_discloses_source_exclusions_and_stages_omission_reconnection_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = portable_scenario()?;
        scenario.extensions.insert(
            "example.visual".to_owned(),
            serde_json::json!({"asset": "notes.txt"}),
        );
        let original = PortableAsset {
            bytes: b"portable notes".to_vec(),
            media_type: "text/plain; charset=utf-8".to_owned(),
            redistribution_permitted: true,
        };
        let mut sections = BackupSections::default();
        sections.assets.insert(
            "notes.txt".to_owned(),
            omitted_asset_placeholder(&original, OmittedAssetReason::ExcludeAll)?,
        );
        let source_selection = BackupSelection {
            include_results: false,
            asset_selection: PortableBackupAssetSelection::ExcludeAll,
            threshold_version: None,
            threshold_bytes: None,
            excluded_asset_count: 1,
            excluded_asset_ids: BTreeSet::from(["notes.txt".to_owned()]),
            fixed_exclusions: BTreeSet::new(),
            scope: BackupSelectionScope::Scenario,
        };
        let bytes = assemble_scenario_export(
            &ScenarioExportSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0010,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Omitted assets".to_owned(),
                scenario,
                scenario_revisions: Vec::new(),
                sections,
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: BTreeMap::from([(
                    eutheto_export::BACKUP_SELECTION_EXTENSION.to_owned(),
                    backup_selection_extension_value(&source_selection)?,
                )]),
            },
            &portable_encode::encode_fixture_domain,
        )?;
        let inspected = inspect(&bytes)?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::new(),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        assert_eq!(
            preview.source_backup_selection,
            Some(source_selection.clone())
        );
        assert!(preview.excluded_sections.contains("results"));
        assert!(preview.excluded_sections.contains("assets"));
        let omitted = preview
            .omitted_assets
            .get("notes.txt")
            .ok_or_else(|| std::io::Error::other("missing omitted-asset disclosure"))?;
        assert_eq!(omitted.original_size, original.bytes.len() as u64);
        assert_eq!(omitted.content_sha256, sha256_hex(&original.bytes));

        let staged = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan::default(),
        )?;
        let staged_placeholder = staged
            .assets
            .get("notes.txt")
            .ok_or_else(|| std::io::Error::other("omitted asset was not staged"))?;
        assert_eq!(
            staged_placeholder.media_type,
            eutheto_export::OMITTED_ASSET_MEDIA_TYPE
        );
        assert_eq!(
            parse_omitted_asset_placeholder(staged_placeholder)?,
            Some(omitted.clone())
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn restore_excluded_asset_becomes_reexportable_reconnection_placeholder()
    -> Result<(), Box<dyn std::error::Error>> {
        let inspected = inspect(&asset_bundle()?)?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::new(),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: false,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        assert!(preview.excluded_sections.contains("assets"));
        let disclosed = preview
            .omitted_assets
            .get("notes.txt")
            .ok_or_else(|| std::io::Error::other("restore exclusion was not disclosed"))?;
        assert_eq!(disclosed.reason, OmittedAssetReason::ImportExcluded);
        let staged = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan::default(),
        )?;
        let placeholder = staged
            .assets
            .get("notes.txt")
            .ok_or_else(|| std::io::Error::other("restore exclusion was not staged"))?;
        assert_ne!(placeholder.bytes, b"safe notes");
        assert_eq!(
            parse_omitted_asset_placeholder(placeholder)?,
            Some(disclosed.clone())
        );

        let asset_identity = SupplementalIdentity {
            section: SupplementalSectionKind::Assets,
            key: "notes.txt".to_owned(),
        };
        let mut colliding_local = local.clone();
        colliding_local
            .supplemental_identities
            .insert(asset_identity.clone());
        let colliding_preview = build_preview(&inspected, &options, &colliding_local)?;
        assert!(
            colliding_preview
                .supplemental_collisions
                .contains(&asset_identity)
        );
        assert!(matches!(
            stage_import(
                &inspected,
                &colliding_preview,
                &options,
                &colliding_local,
                &CollisionPlan::default(),
            ),
            Err(ImportError::UnresolvedSupplementalCollision(identity))
                if identity == asset_identity
        ));
        let skipped = stage_import(
            &inspected,
            &colliding_preview,
            &options,
            &colliding_local,
            &CollisionPlan {
                scenarios: BTreeMap::new(),
                supplemental: BTreeMap::from([(asset_identity, SupplementalCollisionAction::Skip)]),
            },
        )?;
        assert!(skipped.assets.is_empty());
        let sections = BackupSections {
            assets: staged.assets.clone(),

            ..BackupSections::default()
        };
        let selection = BackupSelection {
            include_results: false,
            asset_selection: PortableBackupAssetSelection::All,
            threshold_version: None,
            threshold_bytes: None,
            excluded_asset_count: 1,
            excluded_asset_ids: BTreeSet::from(["notes.txt".to_owned()]),
            fixed_exclusions: BTreeSet::new(),
            scope: BackupSelectionScope::Scenario,
        };
        let reexported = assemble_scenario_export(
            &ScenarioExportSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0012,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Re-exported omission".to_owned(),
                scenario: staged.scenarios[0].scenario.clone(),
                scenario_revisions: Vec::new(),
                sections,
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: BTreeMap::from([(
                    eutheto_export::BACKUP_SELECTION_EXTENSION.to_owned(),
                    backup_selection_extension_value(&selection)?,
                )]),
            },
            &portable_encode::encode_fixture_domain,
        )?;
        eutheto_export::verify_assembled_bundle(&reexported)?;
        Ok(())
    }

    #[test]
    fn skipped_asset_reference_requires_exact_retained_local_asset()
    -> Result<(), Box<dyn std::error::Error>> {
        let inspected = inspect(&asset_bundle()?)?;
        let scenario_id = inspected.scenarios[0].document.scenario_id;
        let asset_identity = SupplementalIdentity {
            section: SupplementalSectionKind::Assets,
            key: "notes.txt".to_owned(),
        };
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::from([scenario_id]),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([scenario_id.to_string()]),
            scenarios: vec![LocalScenarioSnapshot {
                scenario_id,
                title: "Local".to_owned(),
                revision: Revision::new(3),
                archived: false,
                owned_uuids: BTreeSet::from([scenario_id.as_uuid()]),
            }],
            supplemental_identities: BTreeSet::from([asset_identity.clone()]),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        let plan = CollisionPlan {
            scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
            supplemental: BTreeMap::from([(
                asset_identity.clone(),
                SupplementalCollisionAction::Skip,
            )]),
        };
        let staged = stage_import(&inspected, &preview, &options, &local, &plan)?;
        assert!(staged.assets.is_empty());

        let mut missing_local = local.clone();
        missing_local.supplemental_identities.clear();
        assert!(stage_import(&inspected, &preview, &options, &missing_local, &plan).is_err());

        let replace_options = ImportOptions {
            restore_mode: RestoreMode::ReplaceLibrary,
            include_results: true,
            include_assets: true,
        };
        let replace_preview = build_preview(&inspected, &replace_options, &local)?;
        assert!(
            stage_import(
                &inspected,
                &replace_preview,
                &replace_options,
                &local,
                &plan,
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn skipped_scenario_prunes_all_unique_real_and_placeholder_assets()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = portable_scenario()?;
        let scenario_id = scenario.document.scenario_id;
        scenario.extensions.insert(
            "example.visual".to_owned(),
            serde_json::json!({"asset": "real.txt", "assetIds": ["omitted.txt"]}),
        );
        let real = PortableAsset {
            bytes: b"real bytes".to_vec(),
            media_type: "text/plain".to_owned(),
            redistribution_permitted: true,
        };
        let omitted_source = PortableAsset {
            bytes: b"omitted bytes".to_vec(),
            media_type: "text/plain".to_owned(),
            redistribution_permitted: true,
        };
        let mut sections = BackupSections::default();
        sections.assets.insert("real.txt".to_owned(), real);
        sections.assets.insert(
            "omitted.txt".to_owned(),
            omitted_asset_placeholder(&omitted_source, OmittedAssetReason::ImportExcluded)?,
        );
        let selection = BackupSelection {
            include_results: false,
            asset_selection: PortableBackupAssetSelection::All,
            threshold_version: None,
            threshold_bytes: None,
            excluded_asset_count: 1,
            excluded_asset_ids: BTreeSet::from(["omitted.txt".to_owned()]),
            fixed_exclusions: BTreeSet::new(),
            scope: BackupSelectionScope::Scenario,
        };
        let bytes = assemble_scenario_export(
            &ScenarioExportSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0014,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Skipped assets".to_owned(),
                scenario,
                scenario_revisions: Vec::new(),
                sections,
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: BTreeMap::from([(
                    eutheto_export::BACKUP_SELECTION_EXTENSION.to_owned(),
                    backup_selection_extension_value(&selection)?,
                )]),
            },
            &portable_encode::encode_fixture_domain,
        )?;
        let inspected = inspect(&bytes)?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::from([scenario_id]),
            scenario_revision_high_water: BTreeMap::from([(scenario_id, Revision::new(7))]),
            occupied_uuids: BTreeSet::from([scenario_id.to_string()]),
            scenarios: vec![LocalScenarioSnapshot {
                scenario_id,
                title: "Existing scenario".to_owned(),
                revision: Revision::new(7),
                archived: false,
                owned_uuids: BTreeSet::from([scenario_id.as_uuid()]),
            }],
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        let staged = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::Skip)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        assert!(staged.scenarios.is_empty());
        assert!(staged.scenario_revisions.is_empty());
        assert!(staged.assets.is_empty());
        Ok(())
    }

    #[test]
    fn partial_full_backup_skip_preserves_all_selected_library_assets()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut first = portable_scenario()?;
        first.document.domain.entities.clear();
        first.document.domain.rules.clear();
        first.document.domain.preferences.clear();
        first.document.domain.locked_assignments.clear();
        first.extensions.insert(
            "example.visual".to_owned(),
            serde_json::json!({"asset": "first.txt"}),
        );
        let first_id = first.document.scenario_id;
        let mut second = first.clone();
        second.document.scenario_id = "018f1e2d-3c4b-7a69-8def-0123456789ac".parse()?;
        second.document.metadata.title = "Second".to_owned();
        second.extensions.insert(
            "example.visual".to_owned(),
            serde_json::json!({"asset": "second.txt"}),
        );
        let second_id = second.document.scenario_id;
        let mut sections = BackupSections::default();
        for key in ["first.txt", "second.txt"] {
            sections.assets.insert(
                key.to_owned(),
                PortableAsset {
                    bytes: key.as_bytes().to_vec(),
                    media_type: "text/plain".to_owned(),
                    redistribution_permitted: true,
                },
            );
        }
        let bytes = assemble_full_backup(
            &FullBackupSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0015,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Partial assets".to_owned(),
                scenarios: vec![first, second],
                scenario_revisions: Vec::new(),
                sections,
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: library_selection_extensions(true)?,
            },
            &portable_encode::encode_fixture_domain,
        )?;
        let inspected = inspect(&bytes)?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::from([first_id]),
            scenario_revision_high_water: BTreeMap::from([(first_id, Revision::new(7))]),
            occupied_uuids: BTreeSet::from([first_id.to_string()]),
            scenarios: vec![LocalScenarioSnapshot {
                scenario_id: first_id,
                title: "Existing first scenario".to_owned(),
                revision: Revision::new(7),
                archived: false,
                owned_uuids: BTreeSet::from([first_id.as_uuid()]),
            }],
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::AddBackup,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        let staged = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(first_id, CollisionAction::Skip)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        assert_eq!(staged.scenarios.len(), 1);
        assert_eq!(staged.scenarios[0].original_id, second_id);
        assert_eq!(
            staged.assets.keys().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from(["first.txt".to_owned(), "second.txt".to_owned()])
        );
        Ok(())
    }

    #[test]
    fn fresh_full_restore_preserves_unreferenced_real_and_placeholder_assets()
    -> Result<(), Box<dyn std::error::Error>> {
        let real = PortableAsset {
            bytes: b"global library asset".to_vec(),
            media_type: "text/plain".to_owned(),
            redistribution_permitted: true,
        };
        let placeholder = omitted_asset_placeholder(&real, OmittedAssetReason::ExcludeAll)?;
        let mut sections = BackupSections::default();
        sections
            .assets
            .insert("global.txt".to_owned(), real.clone());
        sections
            .assets
            .insert("global-omitted.txt".to_owned(), placeholder.clone());
        let selection = BackupSelection {
            include_results: false,
            asset_selection: PortableBackupAssetSelection::All,
            threshold_version: None,
            threshold_bytes: None,
            excluded_asset_count: 1,
            excluded_asset_ids: BTreeSet::from(["global-omitted.txt".to_owned()]),
            fixed_exclusions: FixedExclusion::ALL.into_iter().collect(),
            scope: BackupSelectionScope::Library,
        };
        let bytes = assemble_full_backup(
            &FullBackupSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0016,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Global assets".to_owned(),
                scenarios: vec![portable_scenario()?],
                scenario_revisions: Vec::new(),
                sections,
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: BTreeMap::from([(
                    eutheto_export::BACKUP_SELECTION_EXTENSION.to_owned(),
                    backup_selection_extension_value(&selection)?,
                )]),
            },
            &portable_encode::encode_fixture_domain,
        )?;
        let inspected = inspect(&bytes)?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::new(),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let included_options = ImportOptions {
            restore_mode: RestoreMode::AddBackup,
            include_results: true,
            include_assets: true,
        };
        let included_preview = build_preview(&inspected, &included_options, &local)?;
        let included = stage_import(
            &inspected,
            &included_preview,
            &included_options,
            &local,
            &CollisionPlan::default(),
        )?;
        assert_eq!(included.assets.get("global.txt"), Some(&real));
        assert_eq!(
            included.assets.get("global-omitted.txt"),
            Some(&placeholder)
        );

        let excluded_options = ImportOptions {
            include_assets: false,
            ..included_options
        };
        let excluded_preview = build_preview(&inspected, &excluded_options, &local)?;
        let excluded = stage_import(
            &inspected,
            &excluded_preview,
            &excluded_options,
            &local,
            &CollisionPlan::default(),
        )?;
        let excluded_real = excluded
            .assets
            .get("global.txt")
            .ok_or_else(|| std::io::Error::other("global asset placeholder missing"))?;
        assert_eq!(
            parse_omitted_asset_placeholder(excluded_real)?.map(|value| value.reason),
            Some(OmittedAssetReason::ImportExcluded)
        );
        assert_eq!(
            excluded.assets.get("global-omitted.txt"),
            Some(&placeholder)
        );
        Ok(())
    }
    #[test]
    fn replace_preview_binds_exact_local_removal_metadata() -> Result<(), Box<dyn std::error::Error>>
    {
        let inspected = inspect(&valid_bundle()?)?;
        let local_id: ScenarioId = "018f1e2d-3c4b-7a69-8def-0123456789ac".parse()?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::new(9),
            scenario_ids: BTreeSet::from([local_id]),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([local_id.to_string()]),
            scenarios: vec![LocalScenarioSnapshot {
                scenario_id: local_id,
                title: "Archived local project".to_owned(),
                revision: Revision::new(4),
                owned_uuids: BTreeSet::from([local_id.as_uuid()]),
                archived: true,
            }],
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let replace_options = ImportOptions {
            restore_mode: RestoreMode::ReplaceLibrary,
            include_results: false,
            include_assets: false,
        };
        let preview = build_preview(&inspected, &replace_options, &local)?;
        assert_eq!(preview.binding.local_library_revision, Revision::new(9));
        assert_eq!(preview.removed_scenarios, local.scenarios);
        assert!(preview.included_sections.contains("scenarios"));
        assert!(preview.excluded_sections.contains("results"));
        let incoming_id = inspected.scenarios[0].document.scenario_id;
        let scenario_choice = CollisionPlan {
            scenarios: BTreeMap::from([(incoming_id, CollisionAction::Replace)]),
            supplemental: BTreeMap::new(),
        };
        assert!(matches!(
            stage_import(
                &inspected,
                &preview,
                &replace_options,
                &local,
                &scenario_choice,
            ),
            Err(ImportError::InvalidRestore(message))
                if message == "replace-library collision plan must be empty"
        ));
        let supplemental_choice = CollisionPlan {
            scenarios: BTreeMap::new(),
            supplemental: BTreeMap::from([(
                SupplementalIdentity {
                    section: SupplementalSectionKind::Assets,
                    key: "extra.txt".to_owned(),
                },
                SupplementalCollisionAction::Skip,
            )]),
        };
        assert!(matches!(
            stage_import(
                &inspected,
                &preview,
                &replace_options,
                &local,
                &supplemental_choice,
            ),
            Err(ImportError::InvalidRestore(message))
                if message == "replace-library collision plan must be empty"
        ));
        let staged = stage_import(
            &inspected,
            &preview,
            &replace_options,
            &local,
            &CollisionPlan::default(),
        )?;
        assert_eq!(staged.scenarios.len(), preview.scenarios.len());

        let import_options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: false,
            include_assets: false,
        };
        let import_preview = build_preview(&inspected, &import_options, &local)?;
        assert!(!import_preview.scenarios[0].collides);
        assert!(matches!(
            stage_import(
                &inspected,
                &import_preview,
                &import_options,
                &local,
                &scenario_choice,
            ),
            Err(ImportError::UnknownCollision(found)) if found == incoming_id
        ));
        Ok(())
    }

    fn assert_complete_copy_remap(
        copy: &StagedImport,
        inspected: &InspectedBundle,
        original: ScenarioId,
    ) -> Result<(ScenarioId, BTreeSet<String>), Box<dyn std::error::Error>> {
        assert_eq!(
            copy.provenance.source_created_at.to_string(),
            inspected.manifest.created_at
        );
        assert_eq!(copy.scenarios[0].disposition, StagedDisposition::CreateCopy);
        assert_eq!(
            copy.scenarios[0].source_revision,
            inspected.scenarios[0].revision
        );
        assert_ne!(copy.scenarios[0].scenario.document.scenario_id, original);
        assert!(copy.scenarios[0].id_remap.len() >= 3);
        let old_person = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000001")?;
        let old_rule = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000002")?;
        let new_person = copy.scenarios[0].id_remap[&old_person].to_string();
        let new_rule = copy.scenarios[0].id_remap[&old_rule].to_string();
        let copy_domain = serde_json::to_value(&copy.scenarios[0].scenario.document.domain)?;
        let entity = &copy_domain["entities"][new_person.as_str()];
        assert_eq!(entity["participantId"], new_person);
        assert_eq!(entity["locationId"], new_rule);
        assert_eq!(
            entity["vehicleIds"],
            serde_json::json!([new_person, new_rule])
        );
        assert_eq!(entity["externalId"], old_person.to_string());
        assert_eq!(entity["grid"], old_rule.to_string());
        assert_eq!(
            copy_domain["rules"][new_rule.as_str()]["activityId"],
            new_person
        );
        let copy_json = serde_json::to_string(&copy_domain)?;
        assert!(copy_json.contains("018f1e2d-3c4b-7a69-8def-900000000001"));
        Ok((
            copy.scenarios[0].scenario.document.scenario_id,
            copy.scenarios[0]
                .id_remap
                .values()
                .map(ToString::to_string)
                .collect(),
        ))
    }

    #[test]
    fn collision_copy_replace_and_skip_are_complete() -> Result<(), Box<dyn std::error::Error>> {
        let inspected = inspect(&valid_bundle()?)?;
        let original = inspected.scenarios[0].document.scenario_id;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::new(4),
            scenario_ids: BTreeSet::from([original]),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([original.to_string()]),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;

        let copy = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(original, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        let (first_copy_id, first_copy_owned_ids) =
            assert_complete_copy_remap(&copy, &inspected, original)?;
        let occupied_local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: local.revision,
            scenario_ids: BTreeSet::from([original, first_copy_id]),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: first_copy_owned_ids,
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let occupied_preview = build_preview(&inspected, &options, &occupied_local)?;
        let second_copy = stage_import(
            &inspected,
            &occupied_preview,
            &options,
            &occupied_local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(original, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        assert_ne!(
            second_copy.scenarios[0].scenario.document.scenario_id,
            first_copy_id
        );
        assert!(copy.scenarios[0].id_remap.values().all(|id| {
            !second_copy.scenarios[0]
                .id_remap
                .values()
                .any(|other| other == id)
        }));

        let replace = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(original, CollisionAction::Replace)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        assert_eq!(replace.scenarios[0].disposition, StagedDisposition::Replace);
        let skip = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(original, CollisionAction::Skip)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        assert!(skip.scenarios.is_empty());
        Ok(())
    }

    #[test]
    // Keeping both extension layers in one fixture proves they share one identity mapping.
    #[allow(clippy::too_many_lines)]
    fn create_copy_remaps_declared_references_in_both_extension_layers()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = portable_scenario()?;
        let scenario_id = scenario.document.scenario_id;
        let person_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000001")?;
        let rule_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000002")?;
        let document_extension_owned = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000003")?;
        let wrapper_extension_owned = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000004")?;
        let unknown_extension_key = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-900000000003")?;
        let mut document_references = serde_json::json!({
            "managerId": person_id,
            "vehicle_ids": [rule_id],
            "externalId": person_id,
            "opaque": rule_id,
            "note": format!("keep external UUID {person_id}")
        });
        document_references["definitions"] = Value::Object(Map::from_iter([(
            document_extension_owned.to_string().to_ascii_uppercase(),
            serde_json::json!({"id": document_extension_owned, "managerId": person_id}),
        )]));
        document_references["unknownDefinitions"] = Value::Object(Map::from_iter([(
            unknown_extension_key.to_string(),
            serde_json::json!({"label": "not self-declared"}),
        )]));
        let mut wrapper_references = document_references.clone();
        wrapper_references["definitions"] = Value::Object(Map::from_iter([(
            wrapper_extension_owned.to_string(),
            serde_json::json!({"id": wrapper_extension_owned, "managerId": person_id}),
        )]));
        scenario
            .document
            .extensions
            .insert("example.document-refs".to_owned(), document_references);
        scenario
            .extensions
            .insert("example.wrapper-refs".to_owned(), wrapper_references);
        let bytes = assemble_scenario_export(
            &ScenarioExportSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0017,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Extension references".to_owned(),
                scenario,
                scenario_revisions: Vec::new(),
                sections: BackupSections::default(),
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: BTreeMap::new(),
            },
            &portable_encode::encode_fixture_domain,
        )?;
        let inspected = inspect(&bytes)?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::from([scenario_id]),
            scenario_revision_high_water: BTreeMap::from([(scenario_id, Revision::new(7))]),
            occupied_uuids: BTreeSet::from([scenario_id.to_string()]),
            scenarios: vec![LocalScenarioSnapshot {
                scenario_id,
                title: "Existing scenario".to_owned(),
                revision: Revision::new(7),
                archived: false,
                owned_uuids: BTreeSet::from([scenario_id.as_uuid(), person_id, rule_id]),
            }],
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        let copied = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        let remapped_person = copied.scenarios[0].id_remap[&person_id].to_string();
        let remapped_rule = copied.scenarios[0].id_remap[&rule_id].to_string();
        let remapped_document_owned =
            copied.scenarios[0].id_remap[&document_extension_owned].to_string();
        let remapped_wrapper_owned =
            copied.scenarios[0].id_remap[&wrapper_extension_owned].to_string();
        assert!(
            !copied.scenarios[0]
                .id_remap
                .contains_key(&unknown_extension_key)
        );
        for (extension, remapped_extension_owned) in [
            (
                &copied.scenarios[0].scenario.document.extensions["example.document-refs"],
                &remapped_document_owned,
            ),
            (
                &copied.scenarios[0].scenario.extensions["example.wrapper-refs"],
                &remapped_wrapper_owned,
            ),
        ] {
            assert_eq!(extension["managerId"], remapped_person);
            assert_eq!(extension["vehicle_ids"], serde_json::json!([remapped_rule]));
            assert_eq!(extension["externalId"], person_id.to_string());
            assert_eq!(extension["opaque"], rule_id.to_string());
            assert_eq!(extension["note"], format!("keep external UUID {person_id}"));
            assert_eq!(
                extension["definitions"][remapped_extension_owned.as_str()]["id"],
                remapped_extension_owned.as_str()
            );
            assert!(
                extension["unknownDefinitions"]
                    .get(unknown_extension_key.to_string().as_str())
                    .is_some()
            );
        }
        Ok(())
    }
    #[test]
    fn copy_mapping_includes_historical_only_owned_definitions()
    -> Result<(), Box<dyn std::error::Error>> {
        let current = portable_scenario()?;
        let scenario_id = current.document.scenario_id;
        let mut historical = current.clone();
        historical.revision = Revision::new(6);
        let historical_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000009")?;
        historical.document.domain.entities.insert(
            historical_id.to_string().parse()?,
            serde_json::json!({"id": historical_id, "participantId": historical_id}),
        );
        let mut sections = BackupSections::default();
        sections.results.insert(
            "018f1e2d-3c4b-7a69-8def-100000000099".to_owned(),
            serde_json::json!({
                "resultId": "018f1e2d-3c4b-7a69-8def-100000000099",
                "scenarioId": scenario_id,
                "scenarioRevision": 6
            }),
        );
        let bundle = assemble_scenario_export(
            &ScenarioExportSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0004,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Historical copy".to_owned(),
                scenario: current,
                scenario_revisions: vec![historical],
                sections,
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: BTreeMap::new(),
            },
            &portable_encode::encode_fixture_domain,
        )?;
        let inspected = inspect(&bundle)?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::from([scenario_id]),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([scenario_id.to_string()]),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        let staged = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        let remapped = staged.scenarios[0].id_remap[&historical_id].to_string();
        assert!(
            staged.scenario_revisions[0]
                .document
                .domain
                .entities
                .keys()
                .any(|identity| identity.to_string() == remapped)
        );
        Ok(())
    }

    #[test]
    // One cohesive cross-scenario graph is required to prove definitions stay local while all
    // declared references in the synthesized historical revision follow the copied family.
    #[allow(clippy::too_many_lines)]
    fn synthesized_source_history_rewrites_references_to_copied_family()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut inspected = inspect(&revisioned_result_backup(Revision::new(7))?)?;
        let mut source = inspected.scenarios[0].clone();
        let source_id = source.document.scenario_id;
        let source_entity = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000001")?;
        let source_rule = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000002")?;
        let copied_id = Uuid::parse_str("01951e2d-3c4b-7a69-8def-100000000202")?;
        let copied_entity = Uuid::parse_str("01951e2d-3c4b-7a69-8def-100000000212")?;
        let copied_rule = Uuid::parse_str("01951e2d-3c4b-7a69-8def-100000000222")?;
        let source_to_copy = BTreeMap::from([
            (source_id.as_uuid(), copied_id),
            (source_entity, copied_entity),
            (source_rule, copied_rule),
        ]);
        let copied = rewrite_scenario_for_plan(&source, true, &source_to_copy)?;
        let source_entity_key = source_entity.to_string().parse()?;
        let source_record = source
            .document
            .domain
            .entities
            .get_mut(&source_entity_key)
            .ok_or_else(|| std::io::Error::other("source entity missing"))?;
        source_record["scenarioId"] = Value::String(copied_id.to_string());
        source_record["entityId"] = Value::String(copied_entity.to_string());
        source_record["participantId"] = Value::String(copied_entity.to_string());
        inspected.scenarios = vec![source, copied];

        let copied_scenario_id = ScenarioId::from_uuid(copied_id);
        let source_owned = collect_scenario_owned_uuids(&inspected.scenarios[0]);
        let copied_owned = collect_scenario_owned_uuids(&inspected.scenarios[1]);
        let occupied_uuids = source_owned
            .iter()
            .chain(&copied_owned)
            .map(ToString::to_string)
            .collect();
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::new(9),
            scenario_ids: BTreeSet::from([source_id, copied_scenario_id]),
            scenario_revision_high_water: BTreeMap::from([
                (source_id, Revision::new(8)),
                (copied_scenario_id, Revision::new(7)),
            ]),
            occupied_uuids,
            scenarios: vec![
                LocalScenarioSnapshot {
                    scenario_id: source_id,
                    title: "Source".to_owned(),
                    revision: Revision::new(8),
                    archived: false,
                    owned_uuids: source_owned,
                },
                LocalScenarioSnapshot {
                    scenario_id: copied_scenario_id,
                    title: "Copied source".to_owned(),
                    revision: Revision::new(7),
                    archived: false,
                    owned_uuids: copied_owned,
                },
            ],
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        let staged = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([
                    (source_id, CollisionAction::Replace),
                    (copied_scenario_id, CollisionAction::CreateCopy),
                ]),
                supplemental: BTreeMap::new(),
            },
        )?;
        let copied_stage = staged
            .scenarios
            .iter()
            .find(|scenario| scenario.original_id == copied_scenario_id)
            .ok_or_else(|| std::io::Error::other("copied scenario missing"))?;
        let remapped_scenario = copied_stage.id_remap[&copied_id];
        let remapped_entity = copied_stage.id_remap[&copied_entity];
        let historical = staged
            .scenario_revisions
            .iter()
            .find(|scenario| {
                scenario.document.scenario_id == source_id && scenario.revision == Revision::new(7)
            })
            .ok_or_else(|| std::io::Error::other("synthesized source history missing"))?;
        assert!(
            historical
                .document
                .domain
                .entities
                .contains_key(&source_entity_key)
        );
        assert!(
            historical
                .document
                .domain
                .rules
                .contains_key(&source_rule.to_string().parse()?)
        );
        let historical_record = &historical.document.domain.entities[&source_entity_key];
        assert_eq!(
            historical_record["scenarioId"],
            remapped_scenario.to_string()
        );
        assert_eq!(historical_record["entityId"], remapped_entity.to_string());
        assert_eq!(
            historical_record["participantId"],
            remapped_entity.to_string()
        );
        Ok(())
    }

    #[test]
    fn nested_owned_id_collision_on_new_scenario_requires_copy_or_skip()
    -> Result<(), Box<dyn std::error::Error>> {
        let inspected = inspect(&valid_bundle()?)?;
        let scenario_id = inspected.scenarios[0].document.scenario_id;
        let nested_id = "018f1e2d-3c4b-7a69-8def-100000000001";
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([nested_id.to_owned()]),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        assert!(preview.scenarios[0].collides);
        assert!(matches!(
            stage_import(
                &inspected,
                &preview,
                &options,
                &local,
                &CollisionPlan::default(),
            ),
            Err(ImportError::UnresolvedCollision(found)) if found == scenario_id
        ));
        assert!(
            stage_import(
                &inspected,
                &preview,
                &options,
                &local,
                &CollisionPlan {
                    scenarios: BTreeMap::from([(scenario_id, CollisionAction::Replace)]),
                    supplemental: BTreeMap::new(),
                },
            )
            .is_err()
        );
        let copied = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        assert_eq!(
            copied.scenarios[0].disposition,
            StagedDisposition::CreateCopy
        );
        Ok(())
    }

    #[test]
    fn incoming_scenario_id_colliding_with_local_result_requires_copy()
    -> Result<(), Box<dyn std::error::Error>> {
        let inspected = inspect(&valid_bundle()?)?;
        let scenario_id = inspected.scenarios[0].document.scenario_id;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([scenario_id.to_string()]),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::from([SupplementalIdentity {
                section: SupplementalSectionKind::Results,
                key: format!("{scenario_id}.json"),
            }]),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        assert!(preview.scenarios[0].collides);
        assert!(
            stage_import(
                &inspected,
                &preview,
                &options,
                &local,
                &CollisionPlan {
                    scenarios: BTreeMap::from([(scenario_id, CollisionAction::Replace)]),
                    supplemental: BTreeMap::new(),
                },
            )
            .is_err()
        );
        let copied = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        assert_eq!(
            copied.scenarios[0].disposition,
            StagedDisposition::CreateCopy
        );
        assert_ne!(
            copied.scenarios[0].scenario.document.scenario_id,
            scenario_id
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn result_collision_offers_scenario_copy_and_remaps_result_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let inspected = inspect(&supplemental_bundle()?)?;
        let scenario_id = inspected.scenarios[0].document.scenario_id;
        let result_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000007")?;
        let result_identity = SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key: format!("{result_id}.json"),
        };
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::from([(
                ScenarioId::from_uuid(result_id),
                Revision::new(3),
            )]),
            occupied_uuids: BTreeSet::from([result_id.to_string()]),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        assert!(preview.scenarios[0].collides);
        assert!(preview.supplemental_collisions.contains(&result_identity));
        assert!(
            stage_import(
                &inspected,
                &preview,
                &options,
                &local,
                &CollisionPlan {
                    scenarios: BTreeMap::from([(scenario_id, CollisionAction::Replace)]),
                    supplemental: BTreeMap::from([(
                        result_identity.clone(),
                        SupplementalCollisionAction::Replace,
                    )]),
                },
            )
            .is_err()
        );

        let copied = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::from([(
                    result_identity.clone(),
                    SupplementalCollisionAction::Replace,
                )]),
            },
        )?;
        let (staged_key, staged_bytes) = copied
            .results
            .first_key_value()
            .ok_or_else(|| std::io::Error::other("copied result was not staged"))?;
        let staged_result: Value = serde_json::from_slice(staged_bytes)?;
        let staged_result_id = extract_result_id(&staged_result)?;
        assert_ne!(staged_result_id, result_id);
        assert_eq!(staged_key, &format!("{staged_result_id}.json"));
        assert_eq!(
            staged_key.strip_suffix(".json"),
            staged_result["resultId"].as_str()
        );
        assert_eq!(
            staged_result["scenarioId"],
            copied.scenarios[0]
                .scenario
                .document
                .scenario_id
                .to_string()
        );

        let nested_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000009")?;
        let mut nested_local = local.clone();
        nested_local.scenario_revision_high_water.clear();
        nested_local.scenario_ids.insert(scenario_id);
        nested_local
            .identity_owners
            .insert(scenario_id.as_uuid(), scenario_id);
        nested_local.occupied_uuids.insert(scenario_id.to_string());
        nested_local.scenarios.push(LocalScenarioSnapshot {
            scenario_id,
            title: "Original scenario".to_owned(),
            revision: Revision::new(3),
            archived: false,
            owned_uuids: BTreeSet::from([scenario_id.as_uuid()]),
        });
        nested_local.occupied_uuids.insert(nested_id.to_string());
        nested_local
            .supplemental_identities
            .insert(result_identity.clone());
        nested_local
            .supplemental_identity_owners
            .insert(result_id, result_identity.clone());
        nested_local
            .supplemental_identity_owners
            .insert(nested_id, result_identity.clone());
        let nested_preview = build_preview(&inspected, &options, &nested_local)?;
        let rejected = stage_import(
            &inspected,
            &nested_preview,
            &options,
            &nested_local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::from([(
                    result_identity.clone(),
                    SupplementalCollisionAction::Replace,
                )]),
            },
        );
        assert!(
            matches!(
                &rejected,
                Err(ImportError::Remap(message)) if message.contains("must be skipped")
            ),
            "{rejected:?}"
        );
        let skipped_result = stage_import(
            &inspected,
            &nested_preview,
            &options,
            &nested_local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::from([(
                    result_identity,
                    SupplementalCollisionAction::Skip,
                )]),
            },
        )?;
        assert!(skipped_result.results.is_empty());
        Ok(())
    }

    #[test]
    fn supplemental_owned_identity_collision_is_explicitly_skip_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let inspected = inspect(&supplemental_bundle()?)?;
        let owned_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000008")?;
        let identity = SupplementalIdentity {
            section: SupplementalSectionKind::SharedRecords,
            key: "shared.json".to_owned(),
        };
        let other_id: ScenarioId = "018f1e2d-3c4b-7a69-8def-0123456789ac".parse()?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::from([(owned_id, other_id)]),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([owned_id.to_string()]),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        assert!(preview.supplemental_collisions.contains(&identity));
        assert!(matches!(
            stage_import(
                &inspected,
                &preview,
                &options,
                &local,
                &CollisionPlan {
                    scenarios: BTreeMap::new(),
                    supplemental: BTreeMap::from([(
                        identity.clone(),
                        SupplementalCollisionAction::Replace,
                    )]),
                },
            ),
            Err(ImportError::Remap(message)) if message.contains("must be skipped")
        ));
        let staged = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::new(),
                supplemental: BTreeMap::from([(
                    identity.clone(),
                    SupplementalCollisionAction::Skip,
                )]),
            },
        )?;
        assert!(staged.shared_records.is_empty());

        let mut same_record = local.clone();
        same_record.identity_owners.clear();
        same_record.supplemental_identities.insert(identity.clone());
        same_record
            .supplemental_identity_owners
            .insert(owned_id, identity.clone());
        let same_record_preview = build_preview(&inspected, &options, &same_record)?;
        let replaced = stage_import(
            &inspected,
            &same_record_preview,
            &options,
            &same_record,
            &CollisionPlan {
                scenarios: BTreeMap::new(),
                supplemental: BTreeMap::from([(identity, SupplementalCollisionAction::Replace)]),
            },
        )?;
        assert!(replaced.shared_records.contains_key("shared.json"));
        Ok(())
    }

    #[test]
    fn replace_refuses_identity_owned_by_another_local_family()
    -> Result<(), Box<dyn std::error::Error>> {
        let inspected = inspect(&valid_bundle()?)?;
        let scenario_id = inspected.scenarios[0].document.scenario_id;
        let nested_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000001")?;
        let other_id: ScenarioId = "018f1e2d-3c4b-7a69-8def-0123456789ac".parse()?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::from([scenario_id, other_id]),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([
                scenario_id.to_string(),
                other_id.to_string(),
                nested_id.to_string(),
            ]),
            scenarios: vec![
                LocalScenarioSnapshot {
                    scenario_id,
                    title: "Matching family".to_owned(),
                    revision: Revision::new(7),
                    archived: false,
                    owned_uuids: BTreeSet::from([scenario_id.as_uuid()]),
                },
                LocalScenarioSnapshot {
                    scenario_id: other_id,
                    title: "Other owner".to_owned(),
                    revision: Revision::new(3),
                    archived: false,
                    owned_uuids: BTreeSet::from([other_id.as_uuid(), nested_id]),
                },
            ],
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        assert!(
            stage_import(
                &inspected,
                &preview,
                &options,
                &local,
                &CollisionPlan {
                    scenarios: BTreeMap::from([(scenario_id, CollisionAction::Replace)]),
                    supplemental: BTreeMap::new(),
                },
            )
            .is_err()
        );
        let copied = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        assert_eq!(
            copied.scenarios[0].disposition,
            StagedDisposition::CreateCopy
        );
        Ok(())
    }

    #[test]
    fn incoming_families_cannot_share_owned_identity() -> Result<(), Box<dyn std::error::Error>> {
        let mut first = portable_scenario()?;
        first.document.domain.entities.clear();
        first.document.domain.rules.clear();
        first.document.domain.preferences.clear();
        first.document.domain.locked_assignments.clear();
        let extension_owned = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000010")?;
        first.document.extensions.insert(
            "example.owned".to_owned(),
            Value::Object(Map::from_iter([(
                extension_owned.to_string(),
                serde_json::json!({"id": extension_owned}),
            )])),
        );
        let mut second = first.clone();
        second.document.scenario_id = "018f1e2d-3c4b-7a69-8def-0123456789ac".parse()?;
        assert!(validate_owned_identity_families([&first, &second]).is_err());
        Ok(())
    }

    #[test]
    fn inspection_requires_canonical_unique_typed_result_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let scenario = portable_scenario()?;
        let scenario_id = scenario.document.scenario_id;
        let result_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000008")?;
        let canonical_path = format!("results/{result_id}.json");
        let missing = canonical_json(&serde_json::json!({
            "scenarioId": scenario_id,
            "scenarioRevision": 7
        }))?;
        assert!(
            validate_bundle_references(
                std::slice::from_ref(&scenario),
                &[],
                &BTreeMap::from([(canonical_path.clone(), missing)]),
            )
            .is_err()
        );
        let payload = canonical_json(&serde_json::json!({
            "resultId": result_id,
            "scenarioId": scenario_id,
            "scenarioRevision": 7
        }))?;
        assert!(
            validate_bundle_references(
                std::slice::from_ref(&scenario),
                &[],
                &BTreeMap::from([("results/alias.json".to_owned(), payload.clone())]),
            )
            .is_err()
        );
        assert!(
            validate_bundle_references(
                std::slice::from_ref(&scenario),
                &[],
                &BTreeMap::from([
                    (canonical_path, payload.clone()),
                    ("results/duplicate.json".to_owned(), payload),
                ]),
            )
            .is_err()
        );

        let overlapping_path = format!("results/{scenario_id}.json");
        let overlapping_payload = canonical_json(&serde_json::json!({
            "resultId": scenario_id,
            "scenarioId": scenario_id,
            "scenarioRevision": 7
        }))?;
        assert!(
            validate_bundle_references(
                std::slice::from_ref(&scenario),
                &[],
                &BTreeMap::from([(overlapping_path, overlapping_payload)]),
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    // The result key, wrapper, payload, and scenario graph must be asserted as one mapping.
    #[allow(clippy::too_many_lines)]
    fn copied_result_key_identity_and_internal_references_remap_together()
    -> Result<(), Box<dyn std::error::Error>> {
        let scenario = portable_scenario()?;
        let scenario_id = scenario.document.scenario_id;
        let result_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000008")?;
        let old_person = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000001")?;
        let old_rule = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000002")?;
        let mut sections = BackupSections::default();
        sections.results.insert(
            result_id.to_string(),
            serde_json::json!({
                "resultId": result_id,
                "scenarioId": scenario_id,
                "scenarioRevision": 7,
                "payload": {
                    "entityId": old_person,
                    "assignmentId": old_rule,
                    "assetId": "notes.txt",
                    "externalId": old_person,
                    "description": format!("original {old_person}")
                }
            }),
        );
        sections.assets.insert(
            "notes.txt".to_owned(),
            PortableAsset {
                bytes: b"portable result notes".to_vec(),
                media_type: "text/plain; charset=utf-8".to_owned(),
                redistribution_permitted: true,
            },
        );
        let bundle = assemble_scenario_export(
            &ScenarioExportSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0005,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Result copy".to_owned(),
                scenario,
                scenario_revisions: Vec::new(),
                sections,
                nonsemantic_extensions: BTreeSet::new(),
                manifest_extensions: BTreeMap::new(),
            },
            &portable_encode::encode_fixture_domain,
        )?;
        let inspected = inspect(&bundle)?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::from([scenario_id]),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([scenario_id.to_string()]),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        let staged = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::new(),
            },
        )?;
        let (result_key, result_bytes) = staged
            .results
            .first_key_value()
            .ok_or_else(|| std::io::Error::other("copied result fixture was not staged"))?;
        let result: Value = serde_json::from_slice(result_bytes)?;
        let staged_result_id = result_key.split('.').next().unwrap_or(result_key);
        assert_eq!(result["resultId"], staged_result_id);
        assert_eq!(
            result["scenarioId"],
            staged.scenarios[0]
                .scenario
                .document
                .scenario_id
                .to_string()
        );
        assert_eq!(
            result["payload"]["entityId"],
            staged.scenarios[0].id_remap[&old_person].to_string()
        );
        assert_eq!(
            result["payload"]["assignmentId"],
            staged.scenarios[0].id_remap[&old_rule].to_string()
        );
        assert_eq!(result["payload"]["assetId"], "notes.txt");
        assert_eq!(result["payload"]["externalId"], old_person.to_string());
        assert_eq!(
            result["payload"]["description"],
            format!("original {old_person}")
        );
        Ok(())
    }

    fn assert_incomplete_supplemental_plan_is_rejected(
        inspected: &InspectedBundle,
        preview: &ImportPreview,
        options: &ImportOptions,
        local: &LocalLibrarySnapshot,
        result_identity: &SupplementalIdentity,
        shared_identity: &SupplementalIdentity,
    ) {
        assert_eq!(
            preview.supplemental_collisions,
            vec![result_identity.clone(), shared_identity.clone()]
        );
        let original = inspected.scenarios[0].document.scenario_id;
        let incomplete = stage_import(
            inspected,
            preview,
            options,
            local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(original, CollisionAction::CreateCopy)]),
                supplemental: BTreeMap::from([(
                    result_identity.clone(),
                    SupplementalCollisionAction::Replace,
                )]),
            },
        );
        assert!(matches!(
            incomplete,
            Err(ImportError::UnresolvedSupplementalCollision(identity))
                if identity == *shared_identity
        ));
    }

    #[test]
    fn supplemental_collisions_are_explicit_and_follow_scenario_copy_or_skip()
    -> Result<(), Box<dyn std::error::Error>> {
        let inspected = inspect(&supplemental_bundle()?)?;
        let original = inspected.scenarios[0].document.scenario_id;
        let result_identity = SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key: "018f1e2d-3c4b-7a69-8def-100000000007.json".to_owned(),
        };
        let shared_identity = SupplementalIdentity {
            section: SupplementalSectionKind::SharedRecords,
            key: "shared.json".to_owned(),
        };
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::new(11),
            scenario_ids: BTreeSet::from([original]),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::from([original.to_string()]),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::from([
                result_identity.clone(),
                shared_identity.clone(),
            ]),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        assert_incomplete_supplemental_plan_is_rejected(
            &inspected,
            &preview,
            &options,
            &local,
            &result_identity,
            &shared_identity,
        );

        let replacements = BTreeMap::from([
            (
                result_identity.clone(),
                SupplementalCollisionAction::Replace,
            ),
            (
                shared_identity.clone(),
                SupplementalCollisionAction::Replace,
            ),
        ]);
        let copied = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(original, CollisionAction::CreateCopy)]),
                supplemental: replacements,
            },
        )?;
        let copied_id = copied.scenarios[0].scenario.document.scenario_id;
        let (result_key, staged_result) = copied
            .results
            .first_key_value()
            .ok_or_else(|| std::io::Error::other("missing staged result fixture"))?;
        let result: Value = serde_json::from_slice(staged_result)?;
        assert_eq!(result["scenarioId"], Value::String(copied_id.to_string()));
        assert_eq!(
            result["resultId"],
            result_key.strip_suffix(".json").unwrap_or(result_key)
        );
        assert_eq!(
            result["payload"]["note"],
            "external 018f1e2d-3c4b-7a69-8def-900000000001"
        );
        assert_eq!(
            copied.supplemental_replacements,
            BTreeSet::from([shared_identity.clone()])
        );

        let skipped = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan {
                scenarios: BTreeMap::from([(original, CollisionAction::Skip)]),
                supplemental: BTreeMap::from([
                    (result_identity, SupplementalCollisionAction::Skip),
                    (shared_identity, SupplementalCollisionAction::Skip),
                ]),
            },
        )?;
        assert!(skipped.scenarios.is_empty());
        assert!(skipped.results.is_empty());
        assert!(skipped.shared_records.is_empty());
        assert!(skipped.supplemental_replacements.is_empty());
        assert!(skipped.assets.is_empty());
        Ok(())
    }

    #[test]
    fn preview_staleness_and_staging_cleanup_are_enforced() -> Result<(), Box<dyn std::error::Error>>
    {
        let bytes = valid_bundle()?;
        let inspected = inspect(&bytes)?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::new(2),
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::new(),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::ImportScenario,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        assert!(matches!(
            validate_preview_binding(
                &preview.binding,
                &inspected.file_sha256,
                &options,
                Revision::new(3),
            ),
            Err(ImportError::StalePreview)
        ));

        let path = {
            let staging = PrivateStagingFile::create(&bytes)?;
            let path = staging.path().to_path_buf();
            assert!(path.exists());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&path)?.permissions().mode() & 0o777,
                    0o600
                );
            }
            path
        };
        assert!(!path.exists());
        Ok(())
    }
    #[test]
    fn copy_remap_preserves_project_wrapper_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = portable_scenario()?;
        scenario.project = Some(eutheto_types::PortableProjectMetadata {
            archived_at: Some(Rfc3339Timestamp::parse("2026-08-28T00:00:00Z")?),
        });
        let expected = scenario.project.clone();
        let mut occupied = BTreeSet::new();
        let copied_ids = collect_scenario_owned_uuids(&scenario);
        let mapping = allocate_identity_mapping(
            &copied_ids,
            BundleId::from_uuid(Uuid::from_u128(0x018f_1e2d_3c4b_7a69_8def_2000_0000_0001)),
            "fixture-sha256",
            &mut occupied,
        )?;
        let copy = rewrite_scenario_for_plan(&scenario, true, &mapping)?;
        assert_eq!(copy.project, expected);
        Ok(())
    }

    #[test]
    fn semantic_extension_definitions_and_structural_id_fields_remap_together()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut scenario = portable_scenario()?;
        scenario.required_capabilities.insert(SemanticCapability {
            id: "example.graph".to_owned(),
            version: 1,
        });
        let extension_id = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000003")?;
        let old_person = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-100000000001")?;
        scenario.semantic_extensions.insert(
            "example.graph".to_owned(),
            serde_json::json!({
                "records": {
                    (extension_id.to_string()): {
                        "id": extension_id,
                        "participantId": old_person,
                        "externalId": old_person,
                        "description": format!("original {old_person}")
                    }
                }
            }),
        );
        let copied_ids = collect_scenario_owned_uuids(&scenario);
        assert!(copied_ids.contains(&extension_id));
        let mapping = allocate_identity_mapping(
            &copied_ids,
            BundleId::from_uuid(Uuid::from_u128(0x018f_1e2d_3c4b_7a69_8def_2000_0000_0001)),
            "semantic-extension-fixture",
            &mut BTreeSet::new(),
        )?;
        let copy = rewrite_scenario_for_plan(&scenario, true, &mapping)?;
        let new_extension = mapping[&extension_id].to_string();
        let new_person = mapping[&old_person].to_string();
        let record = &copy.semantic_extensions["example.graph"]["records"][new_extension.as_str()];
        assert_eq!(record["id"], new_extension);
        assert_eq!(record["participantId"], new_person);
        assert_eq!(record["externalId"], old_person.to_string());
        assert_eq!(record["description"], format!("original {old_person}"));
        Ok(())
    }

    #[test]
    fn malformed_structural_reference_shapes_are_rejected() -> Result<(), Box<dyn std::error::Error>>
    {
        let mapping = BTreeMap::new();
        for mut malformed in [
            serde_json::json!({"participantId": 7}),
            serde_json::json!({"vehicleIds": "not-an-array"}),
            serde_json::json!({"activity_ids": [7]}),
        ] {
            assert!(rewrite_declared_references(&mut malformed, &mapping).is_err());
        }
        let mut safe = serde_json::json!({
            "grid": 7,
            "externalId": "external-provider-value",
            "description": "participantId is prose"
        });
        rewrite_declared_references(&mut safe, &mapping)?;
        Ok(())
    }

    #[test]
    fn manifest_extensions_and_nonsemantic_declarations_survive_staging()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut manifest_extensions = library_selection_extensions(true)?;
        manifest_extensions.insert(
            "example.bundle".to_owned(),
            serde_json::json!({"display": "compact"}),
        );
        let bundle = assemble_full_backup(
            &FullBackupSnapshot {
                bundle_id: BundleId::from_uuid(Uuid::from_u128(
                    0x018f_1e2d_3c4b_7a69_8def_2000_0000_0003,
                )),
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                application: ApplicationMetadata {
                    name: "Eutheto".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                title: "Extensions".to_owned(),
                scenarios: vec![portable_scenario()?],
                scenario_revisions: Vec::new(),
                sections: BackupSections::default(),
                nonsemantic_extensions: BTreeSet::from(["example.unused".to_owned()]),
                manifest_extensions: manifest_extensions.clone(),
            },
            &portable_encode::encode_fixture_domain,
        )?;
        let inspected = inspect(&bundle)?;
        let local = LocalLibrarySnapshot {
            supplemental_identity_owners: BTreeMap::new(),
            identity_owners: BTreeMap::new(),
            revision: Revision::INITIAL,
            scenario_ids: BTreeSet::new(),
            scenario_revision_high_water: BTreeMap::new(),
            occupied_uuids: BTreeSet::new(),
            scenarios: Vec::new(),
            supplemental_identities: BTreeSet::new(),
            settings: BTreeMap::new(),
        };
        let options = ImportOptions {
            restore_mode: RestoreMode::AddBackup,
            include_results: true,
            include_assets: true,
        };
        let preview = build_preview(&inspected, &options, &local)?;
        let staged = stage_import(
            &inspected,
            &preview,
            &options,
            &local,
            &CollisionPlan::default(),
        )?;
        assert_eq!(staged.manifest_extensions, manifest_extensions);
        assert!(staged.nonsemantic_extensions.contains("example.unused"));
        Ok(())
    }
    #[test]
    fn strict_parser_rejects_aggregate_tiny_nodes_during_traversal() {
        let mut limits = PORTABLE_LIMITS;
        limits.max_collection_items = 8;
        let result = parse_strict_json("aggregate.json", b"[[0,0],[0,0],[0,0]]", &limits);
        assert!(
            matches!(
                result,
                Err(ImportError::JsonLimit {
                    limit: "aggregate nodes",
                    ..
                })
            ),
            "aggregate node budget must reject nested tiny values"
        );
    }
}
