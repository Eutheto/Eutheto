//! Strongly typed identifiers used by serialized application contracts.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

/// Failure returned by an identifier source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdGenerationError {
    /// The deterministic source has no identifiers remaining.
    Exhausted,
    /// The source returned a UUID from a version other than version 7.
    NotVersion7,
}

impl fmt::Display for IdGenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Exhausted => "identifier source is exhausted",
            Self::NotVersion7 => "identifier source returned a UUID that is not version 7",
        })
    }
}

impl std::error::Error for IdGenerationError {}

/// Parse failure for a UUIDv7-backed typed identifier.
#[derive(Debug)]
pub enum TypedUuidError {
    /// The text is not a UUID.
    InvalidUuid(uuid::Error),
    /// The UUID is valid but is not version 7.
    NotVersion7,
}

impl fmt::Display for TypedUuidError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUuid(error) => error.fmt(formatter),
            Self::NotVersion7 => formatter.write_str("identifier must be a UUIDv7"),
        }
    }
}

impl std::error::Error for TypedUuidError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidUuid(error) => Some(error),
            Self::NotVersion7 => None,
        }
    }
}

/// Source of `UUIDv7` values used by typed identifier constructors.
pub trait IdGenerator: Send + Sync {
    /// Returns the next `UUIDv7` value.
    ///
    /// # Errors
    ///
    /// Returns [`IdGenerationError`] when the source is exhausted or violates
    /// the `UUIDv7` contract.
    fn next_uuid(&self) -> Result<Uuid, IdGenerationError>;
}

/// Identifier source backed by the operating system clock and randomness.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemIdGenerator;

impl IdGenerator for SystemIdGenerator {
    fn next_uuid(&self) -> Result<Uuid, IdGenerationError> {
        Ok(Uuid::now_v7())
    }
}

/// Deterministic, finite identifier source for tests and repeatable operations.
#[derive(Debug)]
pub struct FixedIdGenerator {
    ids: Box<[Uuid]>,
    cursor: AtomicUsize,
}

impl FixedIdGenerator {
    /// Creates a source that yields `ids` in order and then reports exhaustion.
    #[must_use]
    pub fn new(ids: impl IntoIterator<Item = Uuid>) -> Self {
        Self {
            ids: ids.into_iter().collect(),
            cursor: AtomicUsize::new(0),
        }
    }

    /// Creates a source containing one identifier.
    #[must_use]
    pub fn single(id: Uuid) -> Self {
        Self::new([id])
    }
}

impl IdGenerator for FixedIdGenerator {
    fn next_uuid(&self) -> Result<Uuid, IdGenerationError> {
        let index = self.cursor.fetch_add(1, Ordering::Relaxed);
        let id = self
            .ids
            .get(index)
            .copied()
            .ok_or(IdGenerationError::Exhausted)?;
        if id.get_version_num() == 7 {
            Ok(id)
        } else {
            Err(IdGenerationError::NotVersion7)
        }
    }
}

macro_rules! define_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Creates an identifier from an already validated UUID.
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// Generates an identifier using the supplied source.
            ///
            /// # Errors
            ///
            /// Returns [`IdGenerationError`] when the source cannot provide a `UUIDv7`.
            pub fn new(generator: &(impl IdGenerator + ?Sized)) -> Result<Self, IdGenerationError> {
                generator.next_uuid().and_then(|value| {
                    if value.get_version_num() == 7 {
                        Ok(Self(value))
                    } else {
                        Err(IdGenerationError::NotVersion7)
                    }
                })
            }

            /// Returns the wrapped UUID.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = TypedUuidError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let parsed = Uuid::parse_str(value).map_err(TypedUuidError::InvalidUuid)?;
                if parsed.get_version_num() == 7 {
                    Ok(Self(parsed))
                } else {
                    Err(TypedUuidError::NotVersion7)
                }
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(serde::de::Error::custom)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

define_id!(ScenarioId, "Stable identity of a scenario.");
define_id!(EntityId, "Stable identity of a domain entity.");
define_id!(PersonId, "Stable identity of a person.");
define_id!(RuleId, "Stable identity of a rule or preference.");
define_id!(
    AssignmentId,
    "Stable identity of an assignment or assignment lock."
);
define_id!(SolveRunId, "Stable identity of a solver run.");
define_id!(
    ScenarioSnapshotId,
    "Stable identity of an immutable scenario snapshot."
);
define_id!(
    CounterfactualJobId,
    "Stable identity of a counterfactual solve job."
);
define_id!(SolutionId, "Stable identity of a normalized solution.");
define_id!(CommandId, "Stable identity of a command journal entry.");
define_id!(
    RequestId,
    "Stable identity used to correlate an API request."
);
define_id!(
    OperationId,
    "Native-minted identity of one window-owned application operation."
);
/// Error returned for an invalid namespaced identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NamespacedIdError;

impl fmt::Display for NamespacedIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "identifier must contain at least two non-empty lowercase ASCII namespace segments",
        )
    }
}

impl std::error::Error for NamespacedIdError {}

fn validate_namespaced_id(value: &str) -> bool {
    value.len() <= 128
        && value.split('.').count() >= 2
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment.len() <= 63
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && segment
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && segment
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}

macro_rules! define_namespaced_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Validates and creates a namespaced identifier.
            ///
            /// # Errors
            ///
            /// Returns [`NamespacedIdError`] when `value` is not canonical namespaced ASCII.
            pub fn new(value: &str) -> Result<Self, NamespacedIdError> {
                if validate_namespaced_id(value) {
                    Ok(Self(value.to_owned()))
                } else {
                    Err(NamespacedIdError)
                }
            }

            /// Returns the canonical namespaced identifier.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = NamespacedIdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(&value).map_err(serde::de::Error::custom)
            }
        }
    };
}

define_namespaced_id!(PackId, "Stable namespaced identity of a domain pack.");
define_namespaced_id!(BackendId, "Stable namespaced identity of a solver backend.");
define_id!(BundleId, "Stable identity of a portable bundle.");

#[cfg(test)]
mod tests {
    use super::{
        CounterfactualJobId, FixedIdGenerator, IdGenerator, PackId, ScenarioId, ScenarioSnapshotId,
    };
    use uuid::Uuid;

    #[test]
    fn fixed_ids_are_deterministic_and_exhaustible() -> Result<(), Box<dyn std::error::Error>> {
        let first = Uuid::parse_str("018f47f2-e880-7000-8000-000000000001")?;
        let second = Uuid::parse_str("018f47f2-e880-7000-8000-000000000002")?;
        let generator = FixedIdGenerator::new([first, second]);

        assert_eq!(ScenarioId::new(&generator)?.as_uuid(), first);
        assert_eq!(generator.next_uuid()?, second);
        assert!(generator.next_uuid().is_err());
        Ok(())
    }

    #[test]
    fn serialized_typed_ids_reject_non_v7_uuids() {
        let result = serde_json::from_str::<ScenarioId>("\"550e8400-e29b-41d4-a716-446655440000\"");
        assert!(result.is_err());
    }

    #[test]
    fn persistence_ids_are_strict_uuid_v7_types() -> Result<(), Box<dyn std::error::Error>> {
        let value = "018f47f2-e880-7000-8000-000000000003";
        assert_eq!(value.parse::<ScenarioSnapshotId>()?.to_string(), value);
        assert_eq!(value.parse::<CounterfactualJobId>()?.to_string(), value);
        assert!(
            "550e8400-e29b-41d4-a716-446655440000"
                .parse::<ScenarioSnapshotId>()
                .is_err()
        );
        assert!(
            "550e8400-e29b-41d4-a716-446655440000"
                .parse::<CounterfactualJobId>()
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn namespaced_pack_ids_are_canonical() -> Result<(), Box<dyn std::error::Error>> {
        let id = PackId::new("official.test")?;
        assert_eq!(id.as_str(), "official.test");
        assert_eq!(serde_json::to_string(&id)?, "\"official.test\"");
        assert!(PackId::new("Official.Test").is_err());
        assert!(PackId::new("unnamespaced").is_err());
        Ok(())
    }
}
