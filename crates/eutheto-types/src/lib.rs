//! Stable serialized value types shared by Eutheto application boundaries.

mod application_settings;
mod budget;
mod contracts;
mod identity;
mod ids;
mod portable;
mod values;

pub use application_settings::*;
pub use budget::*;
pub use contracts::*;
pub use identity::*;
pub use ids::*;
pub use portable::*;
pub use values::*;

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// The schema version of [`FoundationStatus`].
pub const FOUNDATION_STATUS_SCHEMA_VERSION: u32 = 1;

/// Capability exposed by the application shell.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    /// Repository/tooling foundation retained for wire compatibility.
    #[serde(rename = "phase_00_foundation")]
    Phase00Foundation,
    /// Phase-01 core application shell and persistence contracts.
    #[serde(rename = "phase_01_core")]
    Phase01Core,
}

impl CapabilityState {
    /// Returns the stable serialized name of this capability state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Phase00Foundation => "phase_00_foundation",
            Self::Phase01Core => "phase_01_core",
        }
    }
}

impl fmt::Display for CapabilityState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Versioned application-shell capability status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundationStatus {
    /// Version of this serialized DTO contract.
    pub schema_version: u32,
    /// Capability currently provided by the application shell.
    pub capability: CapabilityState,
}

impl FoundationStatus {
    /// Creates the retained Phase-00 status for compatibility with older shells.
    #[must_use]
    pub const fn phase_00() -> Self {
        Self {
            schema_version: FOUNDATION_STATUS_SCHEMA_VERSION,
            capability: CapabilityState::Phase00Foundation,
        }
    }

    /// Creates the current Phase-01 core capability status.
    #[must_use]
    pub const fn phase_01() -> Self {
        Self {
            schema_version: FOUNDATION_STATUS_SCHEMA_VERSION,
            capability: CapabilityState::Phase01Core,
        }
    }
}

impl<'de> Deserialize<'de> for FoundationStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct SerializedFoundationStatus {
            schema_version: u32,
            capability: CapabilityState,
        }

        let serialized = SerializedFoundationStatus::deserialize(deserializer)?;
        if serialized.schema_version != FOUNDATION_STATUS_SCHEMA_VERSION {
            return Err(serde::de::Error::custom(format_args!(
                "unsupported foundation status schema version {}; expected {}",
                serialized.schema_version, FOUNDATION_STATUS_SCHEMA_VERSION
            )));
        }

        Ok(Self {
            schema_version: serialized.schema_version,
            capability: serialized.capability,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CapabilityState, FOUNDATION_STATUS_SCHEMA_VERSION, FoundationStatus};

    #[test]
    fn phase_01_status_has_stable_json() -> Result<(), serde_json::Error> {
        let json = serde_json::to_string(&FoundationStatus::phase_01())?;
        assert_eq!(json, r#"{"schemaVersion":1,"capability":"phase_01_core"}"#);
        Ok(())
    }

    #[test]
    fn retained_phase_00_status_round_trips() -> Result<(), serde_json::Error> {
        let json = r#"{"schemaVersion":1,"capability":"phase_00_foundation"}"#;
        let status: FoundationStatus = serde_json::from_str(json)?;
        assert_eq!(status.schema_version, FOUNDATION_STATUS_SCHEMA_VERSION);
        assert_eq!(status.capability, CapabilityState::Phase00Foundation);
        Ok(())
    }

    #[test]
    fn foundation_status_rejects_unknown_schema_version() {
        let json = r#"{"schemaVersion":2,"capability":"phase_01_core"}"#;
        assert!(serde_json::from_str::<FoundationStatus>(json).is_err());
    }
}
