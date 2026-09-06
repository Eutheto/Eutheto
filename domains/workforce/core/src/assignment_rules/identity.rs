use super::{
    AssignmentConstructionIssue, AssignmentRuleError,
    budget::{OperationBudget, add, count},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, btree_map::Entry},
    io::{self, Write},
};

const CONTEXT: &[u8] = b"eutheto/workforce/planning-id/v1\0";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum IdentityKind {
    Boolean,
    Integer,
    ObjectiveTerm,
    Projection,
    Constraint,
    Provenance,
}

impl IdentityKind {
    fn tag(self) -> &'static str {
        match self {
            Self::Boolean => "bool",
            Self::Integer => "int",
            Self::ObjectiveTerm => "objective",
            Self::Projection => "projection",
            Self::Constraint => "constraint",
            Self::Provenance => "provenance",
        }
    }
}

/// Collision evidence is local to one construction. Keys are typed canonical source values;
/// callers normalize semantic sets and intern reusable definition keys before calling.
#[derive(Default)]
pub(super) struct PlanningIdentities {
    keys: BTreeMap<(IdentityKind, [u8; 32]), Vec<u8>>,
}

impl PlanningIdentities {
    pub fn derive<T: Serialize + ?Sized>(
        &mut self,
        kind: IdentityKind,
        key: &T,
        budget: &mut OperationBudget<'_>,
    ) -> Result<String, AssignmentRuleError> {
        budget.step()?;
        let length = budget.measure(key)?;
        let id_length = count("official.workforce.".len() + kind.tag().len() + 1 + 64)?;
        budget.reserve(1, 1, add(add(length, 33)?, id_length)?)?;
        let capacity = usize::try_from(length).map_err(|_| invalid())?;
        let mut writer = ExactKeyWriter {
            bytes: Vec::with_capacity(capacity),
            capacity,
        };
        serde_json::to_writer(&mut writer, key).map_err(|_| invalid())?;
        if writer.bytes.len() != capacity {
            return Err(invalid());
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(CONTEXT);
        hasher.update(kind.tag().as_bytes());
        hasher.update(&[0]);
        hasher.update(&writer.bytes);
        let digest = *hasher.finalize().as_bytes();
        self.record(kind, digest, writer.bytes)?;
        budget.check()?;
        Ok(format!(
            "official.workforce.{}.{}",
            kind.tag(),
            blake3::Hash::from_bytes(digest).to_hex()
        ))
    }

    fn record(
        &mut self,
        kind: IdentityKind,
        digest: [u8; 32],
        key: Vec<u8>,
    ) -> Result<(), AssignmentRuleError> {
        match self.keys.entry((kind, digest)) {
            Entry::Vacant(entry) => {
                entry.insert(key);
            }
            Entry::Occupied(entry) if entry.get() == &key => {}
            Entry::Occupied(_) => {
                return Err(AssignmentRuleError::InvalidConstruction(
                    AssignmentConstructionIssue::IdentityCollision,
                ));
            }
        }
        Ok(())
    }
}

/// The counting pass precedes allocation; this pass cannot exceed the measured capacity even
/// if an internal caller accidentally supplies a serializer with changing output.
struct ExactKeyWriter {
    bytes: Vec<u8>,
    capacity: usize,
}

impl Write for ExactKeyWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() > self.capacity - self.bytes.len() {
            return Err(io::Error::other("non-deterministic Workforce identity key"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ShiftId;
    use eutheto_planning_ir::{BoolVariableId, PlanningIrLimitsV1};
    use eutheto_types::PersonId;

    #[test]
    fn planning_identity_recipe_vectors() -> Result<(), Box<dyn std::error::Error>> {
        let person: PersonId = "018f7b40-a000-7000-8000-000000000001".parse()?;
        let shift: ShiftId = "018f7b40-a000-7000-8000-000000000007".parse()?;
        let key = ("assignment", person, shift);
        let mut identities = PlanningIdentities::default();
        let mut budget = OperationBudget::analysis(None, PlanningIrLimitsV1::DEFAULT);
        for (kind, expected) in [
            (
                IdentityKind::Boolean,
                "official.workforce.bool.c27a5c8cd327d4c3568c13d260ef1feea7dd7979effcdd6839d95a35578a98ce",
            ),
            (
                IdentityKind::Constraint,
                "official.workforce.constraint.6d640a8e9a635b6847d0dcd71b2fbaff64575c673e19cb658382c864a71d095f",
            ),
            (
                IdentityKind::Provenance,
                "official.workforce.provenance.04144255fdd2a8bd30608d07ef07ab4c0942f8b005f26736e5e860dff5692d16",
            ),
        ] {
            let actual = identities.derive(kind, &key, &mut budget)?;
            assert_eq!(actual, expected);
            assert_eq!(identities.derive(kind, &key, &mut budget)?, actual);
        }
        let large_key: Vec<_> = (0_u32..10_000).collect();
        let id = identities.derive(IdentityKind::Boolean, &("large", large_key), &mut budget)?;
        BoolVariableId::new(id)?;
        Ok(())
    }

    #[test]
    fn a_digest_collision_never_replaces_a_different_semantic_key()
    -> Result<(), AssignmentRuleError> {
        let mut identities = PlanningIdentities::default();
        identities.record(IdentityKind::Boolean, [0; 32], b"first".to_vec())?;
        assert_eq!(
            identities.record(IdentityKind::Boolean, [0; 32], b"different".to_vec()),
            Err(AssignmentRuleError::InvalidConstruction(
                AssignmentConstructionIssue::IdentityCollision
            )),
        );
        Ok(())
    }
}
