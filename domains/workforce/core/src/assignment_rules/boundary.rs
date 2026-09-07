//! Borrowed resource checks before allocating generic JSON/checksum operations.
//!
//! The serializer visits typed fields without constructing a Value tree or scanning an
//! unbounded string for JSON escapes. Semantic validation remains with the existing contracts.

use super::{
    AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleLimit,
    budget::{OperationBudget, add, count, within},
};
use eutheto_domain_api::ContractJsonLimits;
use serde::{Serialize, ser};
use std::fmt::{self, Write as _};

pub(super) const DOMAIN_LIMITS: ContractJsonLimits = ContractJsonLimits {
    max_serialized_bytes: eutheto_domain_ir::MAX_DOMAIN_CONTRACT_JSON_BYTES,
    max_depth: eutheto_domain_ir::MAX_DOMAIN_CONTRACT_JSON_DEPTH,
    max_string_bytes: eutheto_domain_ir::MAX_VERIFICATION_TEXT_BYTES,
    max_collection_items: eutheto_domain_ir::MAX_DOMAIN_CONTRACT_JSON_ITEMS,
};

#[derive(Clone, Copy)]
pub(super) struct Measured {
    pub bytes: u64,
    nodes: u64,
}

impl Measured {
    /// Charges bounded JSON scratch before generic constructors clone/serialize input.
    /// Like `OperationBudget`, this is cumulative logical accounting, not an RSS estimate.
    pub fn reserve_json(
        self,
        budget: &mut OperationBudget<'_>,
        copies: u64,
    ) -> Result<(), AssignmentRuleError> {
        let node_bytes =
            count(size_of::<serde_json::Value>() + size_of::<String>() + 4 * size_of::<usize>())?;
        let tree = add(self.bytes, multiply(self.nodes, node_bytes)?)?;
        budget.reserve(
            0,
            multiply(self.nodes, copies)?,
            multiply(add(tree, multiply(self.bytes, 2)?)?, copies)?,
        )
    }
}

pub(super) fn preflight<T: Serialize + ?Sized>(
    value: &T,
    budget: &mut OperationBudget<'_>,
    limits: ContractJsonLimits,
) -> Result<Measured, AssignmentRuleError> {
    let nodes = {
        let mut state = State {
            budget,
            limits,
            nodes: 0,
            string_bytes: 0,
        };
        value
            .serialize(Walker {
                state: &mut state,
                depth: 0,
                key: false,
            })
            .map_err(|error| error.0)?;
        state.nodes
    };
    let bytes = budget.measure(value)?;
    within(
        bytes,
        count(limits.max_serialized_bytes)?,
        AssignmentRuleLimit::Bytes,
    )?;
    Ok(Measured { bytes, nodes })
}

struct State<'a, 'token> {
    budget: &'a mut OperationBudget<'token>,
    limits: ContractJsonLimits,
    nodes: u64,
    string_bytes: u64,
}

impl State<'_, '_> {
    fn node(&mut self, depth: usize) -> Result<(), BoundsError> {
        self.budget.step()?;
        within(
            count(depth)?,
            count(self.limits.max_depth)?,
            AssignmentRuleLimit::PerRecord,
        )?;
        self.nodes = add(self.nodes, 1)?;
        within(
            self.nodes,
            count(self.limits.max_collection_items)?,
            AssignmentRuleLimit::References,
        )?;
        Ok(())
    }

    fn string(&mut self, length: usize) -> Result<(), BoundsError> {
        self.budget.step()?;
        within(
            count(length)?,
            count(self.limits.max_string_bytes)?,
            AssignmentRuleLimit::PerRecord,
        )?;
        self.string_bytes = add(self.string_bytes, count(length)?)?;
        // Raw bytes are a lower bound; exact escaped bytes are measured only after this walk.
        within(
            self.string_bytes,
            count(self.limits.max_serialized_bytes)?,
            AssignmentRuleLimit::Bytes,
        )?;
        Ok(())
    }
}

struct Walker<'a, 'b, 'token> {
    state: &'a mut State<'b, 'token>,
    depth: usize,
    key: bool,
}

impl<'a, 'b, 'token> Walker<'a, 'b, 'token> {
    fn scalar(&mut self) -> Result<(), BoundsError> {
        if self.key {
            self.state.budget.step()?;
            Ok(())
        } else {
            self.state.node(self.depth)
        }
    }

    fn container(self, length: Option<usize>) -> Result<Compound<'a, 'b, 'token>, BoundsError> {
        if self.key {
            return Err(invalid().into());
        }
        self.state.node(self.depth)?;
        if let Some(length) = length {
            within(
                count(length)?,
                count(self.state.limits.max_collection_items)?,
                AssignmentRuleLimit::References,
            )?;
        }
        Ok(Compound {
            state: self.state,
            depth: self.depth,
            items: 0,
        })
    }

    fn variant(self, name: &str) -> Result<Self, BoundsError> {
        if self.key {
            return Err(invalid().into());
        }
        self.state.node(self.depth)?;
        self.state.string(name.len())?;
        Ok(Self {
            state: self.state,
            depth: self.depth + 1,
            key: false,
        })
    }
}

macro_rules! scalar {
    ($($method:ident($value:ty)),* $(,)?) => {
        $(fn $method(mut self, _: $value) -> Result<(), BoundsError> { self.scalar() })*
    };
}

impl<'a, 'b, 'token> ser::Serializer for Walker<'a, 'b, 'token> {
    type Ok = ();
    type Error = BoundsError;
    type SerializeSeq = Compound<'a, 'b, 'token>;
    type SerializeTuple = Compound<'a, 'b, 'token>;
    type SerializeTupleStruct = Compound<'a, 'b, 'token>;
    type SerializeTupleVariant = Compound<'a, 'b, 'token>;
    type SerializeMap = Compound<'a, 'b, 'token>;
    type SerializeStruct = Compound<'a, 'b, 'token>;
    type SerializeStructVariant = Compound<'a, 'b, 'token>;

    scalar!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64)
    );

    fn serialize_char(self, value: char) -> Result<(), BoundsError> {
        self.serialize_str(value.encode_utf8(&mut [0; 4]))
    }

    fn serialize_str(mut self, value: &str) -> Result<(), BoundsError> {
        self.scalar()?;
        self.state.string(value.len())
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<(), BoundsError> {
        let mut sequence = self.container(Some(value.len()))?;
        for byte in value {
            sequence.item(byte)?;
        }
        Ok(())
    }

    fn serialize_none(mut self) -> Result<(), BoundsError> {
        self.scalar()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), BoundsError> {
        value.serialize(self)
    }
    fn serialize_unit(mut self) -> Result<(), BoundsError> {
        self.scalar()
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), BoundsError> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> Result<(), BoundsError> {
        self.serialize_str(variant)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), BoundsError> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), BoundsError> {
        value.serialize(self.variant(variant)?)
    }
    fn serialize_seq(self, length: Option<usize>) -> Result<Self::SerializeSeq, BoundsError> {
        self.container(length)
    }
    fn serialize_tuple(self, length: usize) -> Result<Self::SerializeTuple, BoundsError> {
        self.container(Some(length))
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleStruct, BoundsError> {
        self.container(Some(length))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleVariant, BoundsError> {
        self.variant(variant)?.container(Some(length))
    }
    fn serialize_map(self, length: Option<usize>) -> Result<Self::SerializeMap, BoundsError> {
        self.container(length)
    }
    fn serialize_struct(
        self,
        _: &'static str,
        length: usize,
    ) -> Result<Self::SerializeStruct, BoundsError> {
        self.container(Some(length))
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeStructVariant, BoundsError> {
        self.variant(variant)?.container(Some(length))
    }

    fn collect_str<T: fmt::Display + ?Sized>(mut self, value: &T) -> Result<(), BoundsError> {
        self.scalar()?;
        let mut output = DisplayCounter {
            state: self.state,
            length: 0,
            failure: None,
        };
        if write!(&mut output, "{value}").is_err() {
            return Err(output.failure.unwrap_or_else(|| invalid().into()));
        }
        Ok(())
    }
}

struct Compound<'a, 'b, 'token> {
    state: &'a mut State<'b, 'token>,
    depth: usize,
    items: u64,
}

impl Compound<'_, '_, '_> {
    fn item<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BoundsError> {
        self.items = add(self.items, 1)?;
        within(
            self.items,
            count(self.state.limits.max_collection_items)?,
            AssignmentRuleLimit::References,
        )?;
        value.serialize(Walker {
            state: self.state,
            depth: self.depth + 1,
            key: false,
        })
    }
    fn field<T: Serialize + ?Sized>(&mut self, key: &str, value: &T) -> Result<(), BoundsError> {
        self.state.string(key.len())?;
        self.item(value)
    }
}

macro_rules! sequence {
    ($($trait:ident::$method:ident),* $(,)?) => {
        $(impl ser::$trait for Compound<'_, '_, '_> {
            type Ok = ();
            type Error = BoundsError;
            fn $method<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BoundsError> {
                self.item(value)
            }
            fn end(self) -> Result<(), BoundsError> { Ok(()) }
        })*
    };
}
sequence!(
    SerializeSeq::serialize_element,
    SerializeTuple::serialize_element,
    SerializeTupleStruct::serialize_field,
    SerializeTupleVariant::serialize_field
);

impl ser::SerializeMap for Compound<'_, '_, '_> {
    type Ok = ();
    type Error = BoundsError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), BoundsError> {
        key.serialize(Walker {
            state: self.state,
            depth: self.depth + 1,
            key: true,
        })
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BoundsError> {
        self.item(value)
    }
    fn end(self) -> Result<(), BoundsError> {
        Ok(())
    }
}

macro_rules! structure {
    ($($trait:ident),* $(,)?) => {
        $(impl ser::$trait for Compound<'_, '_, '_> {
            type Ok = ();
            type Error = BoundsError;
            fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), BoundsError> {
                self.field(key, value)
            }
            fn end(self) -> Result<(), BoundsError> { Ok(()) }
        })*
    };
}
structure!(SerializeStruct, SerializeStructVariant);

struct DisplayCounter<'a, 'b, 'token> {
    state: &'a mut State<'b, 'token>,
    length: u64,
    failure: Option<BoundsError>,
}
impl fmt::Write for DisplayCounter<'_, '_, '_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let result = (|| -> Result<(), BoundsError> {
            self.length = add(self.length, count(value.len())?)?;
            within(
                self.length,
                count(self.state.limits.max_string_bytes)?,
                AssignmentRuleLimit::PerRecord,
            )?;
            self.state.string(value.len())
        })();
        result.map_err(|error| {
            self.failure = Some(error);
            fmt::Error
        })
    }
}

#[derive(Debug)]
struct BoundsError(AssignmentRuleError);
impl From<AssignmentRuleError> for BoundsError {
    fn from(value: AssignmentRuleError) -> Self {
        Self(value)
    }
}
impl fmt::Display for BoundsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
impl std::error::Error for BoundsError {}
impl ser::Error for BoundsError {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self(invalid())
    }
}
fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}
fn multiply(left: u64, right: u64) -> Result<u64, AssignmentRuleError> {
    left.checked_mul(right)
        .ok_or(AssignmentRuleError::InvalidConstruction(
            AssignmentConstructionIssue::ArithmeticOverflow,
        ))
}
