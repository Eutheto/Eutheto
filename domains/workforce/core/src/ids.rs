//! Pack-owned identities retain the host's `UUIDv7` string representation.

use eutheto_types::{EntityId, IdGenerationError, IdGenerator, TypedUuidError};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

macro_rules! define_entity_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(EntityId);

        impl $name {
            /// Generates an identity through the caller-owned `UUIDv7` source.
            ///
            /// # Errors
            ///
            /// Returns the source failure or rejects a non-`UUIDv7` value.
            pub fn new(generator: &(impl IdGenerator + ?Sized)) -> Result<Self, IdGenerationError> {
                EntityId::new(generator).map(Self)
            }

            /// Returns the identity used by the host's domain entity map.
            #[must_use]
            pub const fn as_entity_id(self) -> EntityId {
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
                value.parse().map(Self)
            }
        }

        impl TryFrom<EntityId> for $name {
            type Error = TypedUuidError;

            fn try_from(value: EntityId) -> Result<Self, Self::Error> {
                if value.as_uuid().get_version_num() == 7 {
                    Ok(Self(value))
                } else {
                    Err(TypedUuidError::NotVersion7)
                }
            }
        }

        impl From<$name> for EntityId {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

define_entity_id!(
    QualificationId,
    "Stable identity of a qualification definition."
);
define_entity_id!(LocationId, "Stable identity of a work location.");
define_entity_id!(AssignmentTypeId, "Stable identity of an assignment type.");
define_entity_id!(WorkCalendarId, "Stable identity of a work calendar.");
define_entity_id!(
    ShiftTemplateId,
    "Stable identity of a recurring shift template."
);
define_entity_id!(
    ShiftId,
    "Stable identity of a generated, detached, or manual shift."
);
define_entity_id!(AvailabilityId, "Stable identity of an availability record.");
define_entity_id!(
    CoverageRequirementId,
    "Stable identity of a scoped coverage requirement."
);
define_entity_id!(TeamId, "Stable identity of a team.");
define_entity_id!(
    WorkloadBucketId,
    "Stable identity of a workload measurement bucket."
);
define_entity_id!(
    WorkloadPolicyId,
    "Stable identity of a workload balance policy."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_host_identity_cannot_bypass_uuidv7_validation() -> Result<(), Box<dyn std::error::Error>>
    {
        let raw = EntityId::from_uuid("00000000-0000-4000-8000-000000000001".parse()?);
        assert!(matches!(
            ShiftId::try_from(raw),
            Err(TypedUuidError::NotVersion7)
        ));
        Ok(())
    }
}
