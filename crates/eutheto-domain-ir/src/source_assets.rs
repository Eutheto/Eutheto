//! Native-only source-asset metadata. Never include this record in desktop or diagnostic exports.

use crate::{
    DomainContractError, RunInput, checksum_without_field, validate_hash, validate_revision,
};
use eutheto_types::{ScenarioId, ScenarioSnapshotId, SolveRunId};
use serde::{Deserialize, Serialize};

/// Existing portable per-entry byte ceiling for one opaque asset.
pub const MAX_RUN_SOURCE_ASSET_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
/// Existing portable entry limit, applied to complete referenced source metadata.
pub const MAX_RUN_SOURCE_ASSETS: usize = 4_096;
/// Maximum encoded source-asset binding record retained by one run.
pub const MAX_RUN_SOURCE_ASSET_BINDING_BYTES: usize = 2 * 1024 * 1024;
/// Existing portable path byte ceiling, not a filesystem grant.
pub const MAX_RUN_SOURCE_ASSET_KEY_BYTES: usize = 240;
/// Existing portable aggregate byte ceiling for referenced opaque assets.
pub const MAX_RUN_SOURCE_ASSET_CONTENT_BYTES: u64 = 64 * 1024 * 1024;

/// Metadata of exactly one referenced global asset at immutable source capture.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSourceAssetRefV1 {
    /// Exact inert library reference key; never interpreted as a host path.
    pub reference: String,
    /// SHA256 of captured opaque bytes, not a document BLAKE3 hash.
    pub sha256: String,
    /// Captured exact media type.
    pub media_type: String,
    /// Captured licensing/privacy restriction, including false values in native custody.
    pub redistribution_permitted: bool,
    /// Captured byte length, bounded before reading a `SQLite` blob.
    pub byte_length: u64,
}

/// Why complete source metadata could not be captured without failing the solve.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunSourceAssetUnavailableReason {
    /// A declared reference did not resolve to an asset row.
    MissingAsset,
    /// Full-wrapper declarations or retained asset-row metadata were malformed.
    InvalidSource,
    /// Referenced entries, bytes or encoded metadata exceeded portable/native limits.
    ResourceLimitExceeded,
}

/// Complete source capture or a source-bound unavailable outcome, never a partial asset set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type", deny_unknown_fields)]
pub enum RunSourceAssetCaptureV1 {
    /// Explicit empty assets proves that the complete captured wrapper had no references.
    Available {
        /// Canonical ascending reference keys, with at most the portable entry limit.
        #[serde(deserialize_with = "deserialize_bounded_asset_refs")]
        assets: Vec<RunSourceAssetRefV1>,
    },
    /// Copy is unavailable; this does not change solve/verification admission.
    Unavailable {
        /// Exact complete-capture failure category.
        reason: RunSourceAssetUnavailableReason,
    },
}

/// Input-side custody inherited verbatim by derived runs from their base run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSourceAssetBindingsV1 {
    /// Current native record version, exactly one.
    pub schema_version: u32,
    /// Original capture run; for derived runs this remains the base run identity.
    pub origin_run_id: SolveRunId,
    /// Checksum of the original capture input, not a derived condition-modified input.
    pub origin_run_input_checksum: String,
    /// Exact source identity.
    pub scenario_id: ScenarioId,
    /// Exact retained source revision.
    pub scenario_revision: u64,
    /// Immutable snapshot identity inherited by derived runs.
    pub snapshot_id: ScenarioSnapshotId,
    /// Exact canonical `ScenarioDocument` BLAKE3 hash.
    pub source_document_hash: String,
    /// BLAKE3 of the COMPLETE canonical `ScenarioSnapshotV1`, including wrapper extensions.
    pub source_wrapper_hash: String,
    /// Complete asset metadata or an explicit source-bound unavailable state.
    pub capture: RunSourceAssetCaptureV1,
    /// Canonical BLAKE3 checksum excluding this field.
    pub checksum: String,
}

impl RunSourceAssetBindingsV1 {
    /// Captures source identity around a complete native metadata outcome.
    ///
    /// # Errors
    /// Rejects invalid input, keys, hashes, metadata or encoded bounds.
    pub fn new(
        input: &RunInput,
        source_wrapper_hash: String,
        capture: RunSourceAssetCaptureV1,
    ) -> Result<Self, DomainContractError> {
        input.validate()?;
        let mut value = Self {
            schema_version: 1,
            origin_run_id: input.run_id,
            origin_run_input_checksum: input.checksum.clone(),
            scenario_id: input.scenario_id,
            scenario_revision: input.scenario_revision,
            snapshot_id: input.snapshot_id,
            source_document_hash: input.snapshot_document_hash.clone(),
            source_wrapper_hash,
            capture,
            checksum: String::new(),
        };
        value.validate_payload()?;
        value.checksum = checksum_without_field(&value, "checksum")?;
        Ok(value)
    }

    /// Parses bounded native custody, deliberately allowing nonredistributable metadata.
    /// This is not a portable export and contains neither credentials nor asset bytes.
    ///
    /// # Errors
    /// Rejects oversized/malformed/unknown records before publication.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        if bytes.len() > MAX_RUN_SOURCE_ASSET_BINDING_BYTES {
            return Err(DomainContractError::LimitExceeded(
                "run source asset binding bytes",
            ));
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|error| DomainContractError::MalformedJson(error.to_string()))?;
        value.validate()?;
        Ok(value)
    }

    /// Validates bounded complete metadata and its checksum.
    ///
    /// # Errors
    /// Rejects noncanonical keys, invalid metadata, or a stale checksum.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    /// Checks capture against its ORIGINAL base input and exact historical full wrapper.
    /// Derived-run callers additionally cross-check their base custody and shared snapshot.
    ///
    /// # Errors
    /// Rejects substitution of input, source revision, snapshot or wrapper meaning.
    pub fn validate_source(
        &self,
        origin: &RunInput,
        wrapper_hash: &str,
    ) -> Result<(), DomainContractError> {
        self.validate()?;
        origin.validate()?;
        if self.origin_run_id != origin.run_id
            || self.origin_run_input_checksum != origin.checksum
            || self.scenario_id != origin.scenario_id
            || self.scenario_revision != origin.scenario_revision
            || self.snapshot_id != origin.snapshot_id
            || self.source_document_hash != origin.snapshot_document_hash
            || self.source_wrapper_hash != wrapper_hash
        {
            return Err(DomainContractError::InvalidExplanationContract(
                "source asset custody binding",
            ));
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        if self.schema_version != 1 {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        validate_revision(self.scenario_revision)?;
        for hash in [
            &self.origin_run_input_checksum,
            &self.source_document_hash,
            &self.source_wrapper_hash,
        ] {
            validate_hash(hash)?;
        }
        if let RunSourceAssetCaptureV1::Available { assets } = &self.capture {
            if assets.len() > MAX_RUN_SOURCE_ASSETS
                || assets
                    .windows(2)
                    .any(|pair| pair[0].reference >= pair[1].reference)
            {
                return Err(DomainContractError::InvalidExplanationContract(
                    "source asset reference ordering",
                ));
            }
            let mut total_bytes = 0_u64;
            for asset in assets {
                validate_hash(&asset.sha256)?;
                if asset.reference.is_empty()
                    || asset.reference.len() > MAX_RUN_SOURCE_ASSET_KEY_BYTES
                    || asset
                        .reference
                        .bytes()
                        .any(|byte| byte.is_ascii_control() || matches!(byte, b'\\' | b':'))
                    || asset
                        .reference
                        .split('/')
                        .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
                    || asset.media_type.is_empty()
                    || asset.media_type.len() > 512
                    || !asset.media_type.is_ascii()
                    || asset.media_type.bytes().any(|byte| byte.is_ascii_control())
                {
                    return Err(DomainContractError::InvalidExplanationContract(
                        "source asset metadata",
                    ));
                }
                if asset.byte_length > MAX_RUN_SOURCE_ASSET_ENTRY_BYTES {
                    return Err(DomainContractError::LimitExceeded(
                        "source asset entry bytes",
                    ));
                }
                total_bytes = total_bytes
                    .checked_add(asset.byte_length)
                    .filter(|bytes| *bytes <= MAX_RUN_SOURCE_ASSET_CONTENT_BYTES)
                    .ok_or(DomainContractError::LimitExceeded(
                        "source referenced asset bytes",
                    ))?;
            }
        }
        let mut count = EncodedAssetBytes {
            bytes: if self.checksum.is_empty() { 64 } else { 0 },
        };
        serde_json::to_writer(&mut count, self)
            .map_err(|_| DomainContractError::LimitExceeded("run source asset binding bytes"))?;
        Ok(())
    }
}

fn deserialize_bounded_asset_refs<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<RunSourceAssetRefV1>, D::Error> {
    struct Assets;
    impl<'de> serde::de::Visitor<'de> for Assets {
        type Value = Vec<RunSourceAssetRefV1>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a bounded complete source asset array")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut assets = Vec::new();
            while assets.len() < MAX_RUN_SOURCE_ASSETS {
                let Some(asset) = sequence.next_element()? else {
                    return Ok(assets);
                };
                assets.push(asset);
            }
            if sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("source asset entry limit"));
            }
            Ok(assets)
        }
    }
    deserializer.deserialize_seq(Assets)
}

struct EncodedAssetBytes {
    bytes: usize,
}
impl std::io::Write for EncodedAssetBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|count| *count <= MAX_RUN_SOURCE_ASSET_BINDING_BYTES)
            .ok_or_else(|| std::io::Error::other("run source asset binding bytes"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
