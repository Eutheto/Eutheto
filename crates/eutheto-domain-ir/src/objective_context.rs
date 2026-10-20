//! Recorded objective selection and compact immutable baseline bindings.
//!
//! These records bind replay content, not local accepted-result or manual-admission authority.
//! Native preparation authenticates that authority; packs validate exact snapshot content.
//! Captured originals are streamed into a canonical-byte-limited tree during deserialization.

use crate::{
    AcceptedResultRefV1, DomainContractError, DomainEntityRef, ObjectiveProfileId, PackId,
    ScenarioId, SolveRunId, validate_hash, validate_revision,
};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Latest recorded objective-context schema; accepted-source writers retain versions one/two.
pub const RUN_OBJECTIVE_CONTEXT_SCHEMA_VERSION: u32 = 3;
/// Maximum canonical JSON bytes in a run-only captured original baseline.
pub const MAX_CAPTURED_BASELINE_BYTES: usize = 512 * 1024;
/// Explicit ordinary authored semantics, distinct from historical absent context.
pub const AUTHORED_OBJECTIVE_PROFILE_ID: &str = "eutheto.objective.authored";

/// Operation whose objective semantics are recorded, not inferred from effort mode.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunObjectiveOperation {
    /// Optimize the authored source under the selected profile.
    Optimize,
    /// Minimize exact pair changes before the selected profile's remaining objectives.
    Repair,
}

/// Exact original accepted-source identity and immutable original pair content.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunBaselineOriginal {
    /// Original base record in the run snapshot.
    pub entity: DomainEntityRef,
    /// Pack-owned canonical original pair-content hash.
    pub pair_content_hash: String,
    /// Canonical accepted result from which the original base was captured.
    pub accepted_result: AcceptedResultRefV1,
    /// Original accepted source scenario, not a display or copied identity.
    pub source_scenario_id: ScenarioId,
    /// Original accepted source revision; need not equal the later repair revision.
    pub source_revision: u64,
    /// Original accepted source document hash.
    pub source_document_hash: String,
    /// Original accepted solve run.
    pub source_run_id: SolveRunId,
    /// Exact original run-input checksum.
    pub source_run_input_checksum: String,
    /// Exact original terminal-manifest checksum.
    pub source_run_manifest_checksum: String,
}

impl RunBaselineOriginal {
    /// Checks the original reference's shape and scenario binding, not local acceptance authority.
    ///
    /// # Errors
    /// Rejects a foreign scenario, invalid revision or malformed checksum.
    pub fn validate_for(&self, scenario_id: ScenarioId) -> Result<(), DomainContractError> {
        if self.source_scenario_id != scenario_id {
            return Err(DomainContractError::InvalidObjectiveContext(
                "original scenario",
            ));
        }
        validate_revision(self.source_revision)?;
        self.accepted_result.validate()?;
        for value in [
            &self.pair_content_hash,
            &self.source_document_hash,
            &self.source_run_input_checksum,
            &self.source_run_manifest_checksum,
        ] {
            validate_hash(value)?;
        }
        Ok(())
    }
}

/// Selected pair record; explicit empty manual content remains a manual selection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    deny_unknown_fields
)]
pub enum RunBaselineSelection {
    /// Use the original `BaseSchedule`'s complete pair content.
    OriginalAccepted {},
    /// Use the latest admitted complete manual record retained in this run snapshot.
    ManualSelection {
        /// Exact selected manual entity, distinct from the original base.
        entity: DomainEntityRef,
        /// Checksum of the complete frozen manual record.
        record_checksum: String,
        /// Immutable document on which full manual admission occurred.
        admission_source_scenario_id: ScenarioId,
        /// Admission revision; not the repair run's current source revision.
        admission_source_revision: u64,
        /// Exact admission-source document hash.
        admission_source_document_hash: String,
    },
}

/// Selection identity with complete pairs in the source snapshot or an explicit run-only capture.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunBaselineBinding {
    /// Immutable original accepted ancestry and pair content.
    pub original: RunBaselineOriginal,
    /// Exact original or manual selection, with no fallback between them.
    pub selection: RunBaselineSelection,
    /// Canonical complete selected pair-content hash, including an explicitly empty set.
    pub selected_pair_content_hash: String,
    /// Closed pack-owned original record, never native acceptance or manual-history authority.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_captured_original"
    )]
    pub captured_original: Option<Arc<serde_json::Value>>,
}

impl RunBaselineBinding {
    /// Validates structural integrity, not local provenance or current Required feasibility.
    ///
    /// # Errors
    /// Rejects invalid hashes/revisions, foreign source identities or contradictory selection.
    pub fn validate_for(&self, scenario_id: ScenarioId) -> Result<(), DomainContractError> {
        let original = &self.original;
        original.validate_for(scenario_id)?;
        validate_hash(&self.selected_pair_content_hash)?;
        if let Some(captured) = &self.captured_original {
            if !matches!(self.selection, RunBaselineSelection::OriginalAccepted {}) {
                return Err(DomainContractError::InvalidObjectiveContext(
                    "captured manual selection",
                ));
            }
            validate_captured_baseline_size(captured.as_ref())?;
        }
        match &self.selection {
            RunBaselineSelection::OriginalAccepted {} => {
                if self.selected_pair_content_hash != original.pair_content_hash {
                    return Err(DomainContractError::InvalidObjectiveContext(
                        "original pair content",
                    ));
                }
            }
            RunBaselineSelection::ManualSelection {
                entity,
                record_checksum,
                admission_source_scenario_id,
                admission_source_revision,
                admission_source_document_hash,
            } => {
                if *admission_source_scenario_id != scenario_id || entity == &original.entity {
                    return Err(DomainContractError::InvalidObjectiveContext(
                        "manual identity",
                    ));
                }
                validate_revision(*admission_source_revision)?;
                validate_hash(record_checksum)?;
                validate_hash(admission_source_document_hash)?;
            }
        }
        Ok(())
    }
}

/// Unverified external pair selection in the immutable run snapshot, never accepted ancestry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunExternalBaselineBinding {
    /// Pack-owned external starting-plan record in the exact run snapshot.
    pub entity: DomainEntityRef,
    /// Canonical complete current pair content, not the original import fingerprint.
    pub selected_pair_content_hash: String,
}

/// Closed objective context carried identically by request semantics and immutable run input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunObjectiveContext {
    /// One for in-snapshot accepted sources, two for captured originals, three for external plans.
    pub schema_version: u32,
    /// Pack owning profile interpretation and source-content validation.
    pub pack_id: PackId,
    /// Explicit recognized pack-owned profile; never inferred from a free policy label.
    pub profile_id: ObjectiveProfileId,
    /// Recorded profile semantic version, currently one.
    pub semantic_version: u32,
    /// Optimize or Repair, separate from solve effort settings.
    pub operation: RunObjectiveOperation,
    /// Optional non-null immutable selection binding, boxed to keep absent-baseline run states small.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::deserialize_present"
    )]
    pub baseline: Option<Box<RunBaselineBinding>>,
    /// Explicit unverified source, mutually exclusive with accepted ancestry.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::deserialize_present"
    )]
    pub external_baseline: Option<Box<RunExternalBaselineBinding>>,
}

impl RunObjectiveContext {
    /// Creates the explicit current ordinary authored context without a repair baseline.
    ///
    /// # Errors
    /// Returns an identifier error if the built-in profile identifier is invalid.
    pub fn authored(pack_id: PackId) -> Result<Self, DomainContractError> {
        Ok(Self {
            schema_version: 1,
            pack_id,
            profile_id: ObjectiveProfileId::new(AUTHORED_OBJECTIVE_PROFILE_ID)
                .map_err(|_| DomainContractError::InvalidObjectiveContext("authored profile"))?,
            semantic_version: 1,
            operation: RunObjectiveOperation::Optimize,
            baseline: None,
            external_baseline: None,
        })
    }

    /// Reports selected pair content without conferring accepted-source authority.
    #[must_use]
    pub const fn has_baseline(&self) -> bool {
        self.baseline.is_some() || self.external_baseline.is_some()
    }

    /// Validates shared bindings; the owning pack separately admits recognized profile semantics.
    ///
    /// # Errors
    /// Rejects unknown context/semantic versions, pack mismatch or missing repair baseline.
    pub fn validate_for(
        &self,
        pack_id: &PackId,
        scenario_id: ScenarioId,
    ) -> Result<(), DomainContractError> {
        if !matches!(
            self.schema_version,
            1 | 2 | RUN_OBJECTIVE_CONTEXT_SCHEMA_VERSION
        ) {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        let captured = self
            .baseline
            .as_ref()
            .is_some_and(|baseline| baseline.captured_original.is_some());
        if (self.schema_version == 2) != captured {
            return Err(DomainContractError::InvalidObjectiveContext(
                "captured baseline version",
            ));
        }
        if (self.schema_version == 3) != self.external_baseline.is_some()
            || (self.baseline.is_some() && self.external_baseline.is_some())
        {
            return Err(DomainContractError::InvalidObjectiveContext(
                "external baseline version or source",
            ));
        }
        if self.semantic_version != 1 {
            return Err(DomainContractError::InvalidObjectiveContext(
                "semantic version",
            ));
        }
        if &self.pack_id != pack_id {
            return Err(DomainContractError::InvalidObjectiveContext(
                "pack identity",
            ));
        }
        if self.profile_id.as_str() == AUTHORED_OBJECTIVE_PROFILE_ID
            && (self.operation != RunObjectiveOperation::Optimize || self.has_baseline())
        {
            return Err(DomainContractError::InvalidObjectiveContext(
                "authored operation",
            ));
        }
        if self.operation == RunObjectiveOperation::Repair && !self.has_baseline() {
            return Err(DomainContractError::InvalidObjectiveContext(
                "repair baseline",
            ));
        }
        if let Some(baseline) = &self.baseline {
            baseline.validate_for(scenario_id)?;
        }
        if let Some(baseline) = &self.external_baseline {
            validate_hash(&baseline.selected_pair_content_hash)?;
        }
        Ok(())
    }
}

// Count the representation serde_json will actually emit, not input whitespace or
// unescaped string lengths. Charge each child before retaining it in its parent;
// never reserve collections from an untrusted deserializer's size_hint.
fn deserialize_captured_original<'de, D>(
    deserializer: D,
) -> Result<Option<Arc<serde_json::Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut budget = CapturedBaselineByteLimit(MAX_CAPTURED_BASELINE_BYTES);
    let value = CapturedValueSeed {
        budget: &mut budget,
        prefix_bytes: 0,
    }
    .deserialize(deserializer)?;
    if value.is_null() {
        return Err(de::Error::custom("captured original must not be null"));
    }
    Ok(Some(Arc::new(value)))
}

struct CapturedValueSeed<'a> {
    budget: &'a mut CapturedBaselineByteLimit,
    prefix_bytes: usize,
}

impl<'de> DeserializeSeed<'de> for CapturedValueSeed<'_> {
    type Value = serde_json::Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        self.budget
            .consume(self.prefix_bytes)
            .map_err(de::Error::custom)?;
        deserializer.deserialize_any(self)
    }
}

impl CapturedValueSeed<'_> {
    fn number<E: de::Error>(self, value: serde_json::Number) -> Result<serde_json::Value, E> {
        self.budget.charge::<E>(&value)?;
        Ok(serde_json::Value::Number(value))
    }
}

impl<'de> Visitor<'de> for CapturedValueSeed<'_> {
    type Value = serde_json::Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("bounded JSON without duplicate object keys")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.budget.charge::<E>(&serde_json::Value::Null)?;
        Ok(serde_json::Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.visit_unit()
    }

    fn visit_some<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        self.budget.charge::<E>(&value)?;
        Ok(serde_json::Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        self.number(value.into())
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        self.number(value.into())
    }

    fn visit_i128<E: de::Error>(self, value: i128) -> Result<Self::Value, E> {
        self.number(serde_json::Number::deserialize(
            de::value::I128Deserializer::<E>::new(value),
        )?)
    }

    fn visit_u128<E: de::Error>(self, value: u128) -> Result<Self::Value, E> {
        self.number(serde_json::Number::deserialize(
            de::value::U128Deserializer::<E>::new(value),
        )?)
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        self.number(
            serde_json::Number::from_f64(value)
                .ok_or_else(|| E::custom("non-finite JSON number"))?,
        )
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.budget.charge::<E>(&value)?;
        Ok(serde_json::Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        self.budget.charge::<E>(&value)?;
        Ok(serde_json::Value::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        self.budget.consume(2).map_err(de::Error::custom)?;
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(CapturedValueSeed {
            budget: &mut *self.budget,
            prefix_bytes: usize::from(!values.is_empty()),
        })? {
            values.push(value);
        }
        Ok(serde_json::Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Self::Value, A::Error> {
        self.budget.consume(2).map_err(de::Error::custom)?;
        let mut values = serde_json::Map::new();
        while let Some(key) = object.next_key_seed(CapturedValueSeed {
            budget: &mut *self.budget,
            // Reserve the colon and, after the first entry, the comma.
            prefix_bytes: 1 + usize::from(!values.is_empty()),
        })? {
            let serde_json::Value::String(key) = key else {
                return Err(de::Error::custom("JSON object key must be a string"));
            };
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate object key"));
            }
            let value = object.next_value_seed(CapturedValueSeed {
                budget: &mut *self.budget,
                prefix_bytes: 0,
            })?;
            values.insert(key, value);
        }
        Ok(serde_json::Value::Object(values))
    }
}

/// Checks the capture's canonical JSON byte ceiling before retaining its wire representation.
///
/// This is a size check only; the owning pack validates its closed record and pair content.
///
/// # Errors
/// Rejects serialization failure or a capture larger than [`MAX_CAPTURED_BASELINE_BYTES`].
pub fn validate_captured_baseline_size(value: &impl Serialize) -> Result<(), DomainContractError> {
    serde_json::to_writer(
        CapturedBaselineByteLimit(MAX_CAPTURED_BASELINE_BYTES),
        value,
    )
    .map_err(|_| DomainContractError::LimitExceeded("captured baseline JSON bytes"))
}

struct CapturedBaselineByteLimit(usize);

impl CapturedBaselineByteLimit {
    fn consume(&mut self, bytes: usize) -> std::io::Result<()> {
        self.0 = self
            .0
            .checked_sub(bytes)
            .ok_or_else(|| std::io::Error::other("captured baseline JSON bytes"))?;
        Ok(())
    }

    fn charge<E: de::Error>(&mut self, value: &impl Serialize) -> Result<(), E> {
        serde_json::to_writer(self, value).map_err(E::custom)
    }
}

impl std::io::Write for CapturedBaselineByteLimit {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.consume(bytes.len())?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
