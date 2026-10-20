//! Solver-independent domain result contracts.
//!
//! A [`NormalizedSolution`] contains projected domain assignments only. It records the
//! immutable scenario revision and projection contract version, but deliberately contains
//! no backend feasibility or objective claim. [`AcceptedResult::new`] is a data-integrity
//! gate over a separately produced [`VerificationReport`]; it is not a verifier,
//! orchestrator, or source of production authority.

use eutheto_types::{
    BackendId, BackendSelection, CounterfactualJobId, DurationMillis, IanaTimeZone, PackId,
    PortableJsonLimits, REVISION_MAX_V1, RequestId, Rfc3339Timestamp, RuleId, ScenarioId,
    ScenarioSnapshotId, SolutionId, SolveOptions, SolveRunId, SolveStatus,
    validate_nonsecret_portable_json_bytes,
};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

mod comparison;
mod explanation;
mod infeasibility;
mod native_observations;
mod objective_context;
mod source_assets;

pub use comparison::*;
pub use explanation::*;
pub use infeasibility::*;
pub use native_observations::*;
pub use objective_context::*;
pub use source_assets::*;

// Only newly introduced optional fields use this: absent is None, present null is invalid.
fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Current normalized-solution wire schema.
pub const NORMALIZED_SOLUTION_SCHEMA_VERSION: u32 = 1;
/// Current verification-scope wire schema.
pub const VERIFICATION_SCOPE_SCHEMA_VERSION: u32 = 1;
/// Current verification-context wire schema.
pub const VERIFICATION_CONTEXT_SCHEMA_VERSION: u32 = 1;
/// Current independent-verification report wire schema.
pub const VERIFICATION_REPORT_SCHEMA_VERSION: u32 = 2;
/// Current accepted-result wire schema.
pub const ACCEPTED_RESULT_SCHEMA_VERSION: u32 = 2;
/// Current canonical run-request semantic preimage schema.
pub const RUN_REQUEST_SEMANTICS_SCHEMA_VERSION: u32 = 2;
/// Current immutable run-input wire schema.
pub const RUN_INPUT_SCHEMA_VERSION: u32 = 2;
/// Current terminal run-manifest wire schema.
pub const RUN_MANIFEST_SCHEMA_VERSION: u32 = 2;
/// Current portable accepted-result wrapper wire schema.
pub const PORTABLE_ACCEPTED_RESULT_SCHEMA_VERSION: u32 = 3;
/// Local independently verified checkpoint while its original solve may still run.
pub const VERIFIED_CHECKPOINT_SCHEMA_VERSION: u32 = 2;
/// Maximum UTF-8 bytes in one persisted component version string.
pub const MAX_RUN_VERSION_BYTES: usize = 64;
/// Maximum UTF-8 bytes in one persisted diagnostic code.
pub const MAX_RUN_DIAGNOSTIC_CODE_BYTES: usize = 96;
/// Maximum evidence records in one portable accepted result.
pub const MAX_PORTABLE_RESULT_EVIDENCE_RECORDS: usize = 100_000;
/// Maximum nesting depth accepted at a portable domain-contract ingress.
pub const MAX_DOMAIN_CONTRACT_JSON_DEPTH: usize = 64;
/// Maximum aggregate nodes/items accepted at a portable domain-contract ingress.
pub const MAX_DOMAIN_CONTRACT_JSON_ITEMS: usize = 1_000_000;
/// Maximum aggregate JSON bytes accepted or minted by a domain contract.
pub const MAX_DOMAIN_CONTRACT_JSON_BYTES: usize = 16 * 1024 * 1024;
/// Maximum ordered levels in one authoritative score vector.
pub const MAX_SCORE_LEVELS: usize = 16;
/// Maximum explanatory category totals in one score level.
pub const MAX_SCORE_CATEGORIES_PER_LEVEL: usize = 1_024;
/// Maximum required rules or evaluations in one verification contract.
pub const MAX_VERIFICATION_RULES: usize = 100_000;
/// Maximum warnings in one verification report.
pub const MAX_VERIFICATION_WARNINGS: usize = 10_000;
/// Maximum metrics in one verification report.
pub const MAX_VERIFICATION_METRICS: usize = 1_024;
/// Maximum facts in one typed fact map.
pub const MAX_VERIFICATION_FACTS: usize = 64;
/// Maximum affected entities in one verification record.
pub const MAX_VERIFICATION_ENTITIES: usize = 256;
/// Maximum evidence references in one rule evaluation.
pub const MAX_VERIFICATION_EVIDENCE: usize = 64;
/// Maximum UTF-8 bytes in one verification text value.
pub const MAX_VERIFICATION_TEXT_BYTES: usize = 4_096;
/// Longest domain IR stable identifier, in UTF-8 bytes.
pub const MAX_DOMAIN_ID_BYTES: usize = 160;

/// Invalid stable domain identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DomainIdError;

impl fmt::Display for DomainIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("identifier must be at most 160 bytes and contain two or more lowercase ASCII namespace segments using letters, digits, '_' or '-'")
    }
}

impl std::error::Error for DomainIdError {}

fn valid_id(value: &str) -> bool {
    value.len() <= MAX_DOMAIN_ID_BYTES
        && value.split('.').count() >= 2
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_'
                        || byte == b'-'
                })
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

macro_rules! domain_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Validates and creates an identifier.
            ///
            /// # Errors
            /// Returns [`DomainIdError`] if `value` is not canonical.
            pub fn new(value: impl Into<String>) -> Result<Self, DomainIdError> {
                let value = value.into();
                if valid_id(&value) {
                    Ok(Self(value))
                } else {
                    Err(DomainIdError)
                }
            }

            /// Returns the canonical identifier.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Returns backing allocation capacity, excluding this inline identifier.
            #[must_use]
            pub fn heap_capacity(&self) -> usize {
                self.0.capacity()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = DomainIdError;

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
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

domain_id!(
    DomainAssignmentId,
    "Stable identity of one projected domain assignment."
);
domain_id!(DomainEntityId, "Stable identity of a domain entity.");
domain_id!(
    ObjectiveProfileId,
    "Stable identity of a pack-owned recorded objective profile."
);
domain_id!(
    DomainEntityKindId,
    "Stable identity of a domain entity kind."
);
domain_id!(
    DomainEvidenceId,
    "Stable identity of optional backend or verifier evidence."
);
domain_id!(
    AssumptionGroupId,
    "Stable identity of one explainable planning-assumption group."
);
domain_id!(
    DiagnosticAssumptionVariableId,
    "Canonical Boolean planning-variable identity retained only in diagnostic proof custody."
);
domain_id!(
    ScoreLevelId,
    "Stable identity of a lexicographic score level."
);
domain_id!(ScoreCategoryId, "Stable identity of a score category.");
domain_id!(
    VerificationIssueId,
    "Stable identity of a verification issue."
);
domain_id!(
    VerificationFactId,
    "Stable identity of a typed verification fact or message parameter."
);
domain_id!(
    MetricId,
    "Stable identity of a verifier-owned domain metric."
);
domain_id!(
    VerificationWarningId,
    "Stable identity of a non-acceptance-blocking verification warning."
);

/// Stable typed reference to an entity. Display text is never identity.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainEntityRef {
    /// Entity kind.
    pub kind: DomainEntityKindId,
    /// Entity identity within the kind.
    pub id: DomainEntityId,
}

impl DomainEntityRef {
    /// Returns whether `value` is exactly a valid typed entity reference.
    ///
    /// Recognizes only the closed wire shape; it establishes neither locality nor authority.
    #[must_use]
    pub fn matches_json(value: &serde_json::Value) -> bool {
        let Some(object) = value.as_object() else {
            return false;
        };
        object.len() == 2
            && object
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .is_some_and(valid_id)
            && object
                .get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(valid_id)
    }
}

/// A present half-open interval `[start, end)`.
///
/// Duration must be non-negative and `start + duration == end` in checked `i64`
/// arithmetic. A zero-duration interval occupies no time.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignedInterval {
    /// Inclusive start instant.
    pub start: i64,
    /// Non-negative duration.
    pub duration: i64,
    /// Exclusive end instant.
    pub end: i64,
}

impl AssignedInterval {
    /// Creates a coherent interval.
    ///
    /// # Errors
    /// Returns [`DomainContractError::InvalidInterval`] for a negative duration,
    /// arithmetic overflow, or an incoherent end.
    pub fn new(start: i64, duration: i64, end: i64) -> Result<Self, DomainContractError> {
        if duration < 0 || start.checked_add(duration) != Some(end) {
            Err(DomainContractError::InvalidInterval)
        } else {
            Ok(Self {
                start,
                duration,
                end,
            })
        }
    }

    /// Validates the interval invariant.
    ///
    /// # Errors
    /// Returns [`DomainContractError::InvalidInterval`] when incoherent.
    pub fn validate(self) -> Result<(), DomainContractError> {
        Self::new(self.start, self.duration, self.end).map(|_| ())
    }
}

/// Typed projected value. `Absent` is distinct from `false`, zero, and a zero-duration interval.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    tag = "type",
    content = "value",
    deny_unknown_fields
)]
pub enum AssignmentValue {
    /// Boolean assignment.
    Boolean(bool),
    /// Signed integer assignment.
    Integer(i64),
    /// Present interval assignment.
    Interval(AssignedInterval),
    /// Explicit absence of an optional projected value.
    Absent,
}

/// One stable normalized assignment and optional non-authoritative evidence references.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainAssignment {
    /// Assignment identity.
    pub id: DomainAssignmentId,
    /// Entity represented by the assignment.
    pub entity: DomainEntityRef,
    /// Typed value.
    pub value: AssignmentValue,
    /// Stable evidence references. These do not establish correctness.
    pub evidence: Vec<DomainEvidenceId>,
}

/// Versioned, backend-independent projection result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedSolution {
    /// Must equal [`NORMALIZED_SOLUTION_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Domain pack that owns the projection contract.
    pub pack_id: PackId,
    /// Immutable scenario identity.
    pub scenario_id: ScenarioId,
    /// Immutable revision solved.
    pub scenario_revision: u64,
    /// Domain-defined projection contract version.
    pub projection_version: u32,
    /// Stable solution identity.
    pub solution_id: SolutionId,
    /// Assignments in ascending assignment-ID order.
    pub assignments: Vec<DomainAssignment>,
}

impl NormalizedSolution {
    /// Strictly parses and validates a normalized solution.
    ///
    /// # Errors
    /// Rejects malformed JSON, unknown fields, unknown versions, duplicate or noncanonical
    /// assignment/evidence ordering, and incoherent intervals.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates schema and canonical ordering.
    ///
    /// # Errors
    /// Returns a typed contract error for the first invalid invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.schema_version != NORMALIZED_SOLUTION_SCHEMA_VERSION {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        validate_revision(self.scenario_revision)?;
        let mut prior: Option<&DomainAssignmentId> = None;
        for assignment in &self.assignments {
            if prior.is_some_and(|id| id >= &assignment.id) {
                return Err(DomainContractError::NonCanonicalAssignments);
            }
            prior = Some(&assignment.id);
            if let AssignmentValue::Interval(interval) = assignment.value {
                interval.validate()?;
            }
            if !strictly_sorted_unique(&assignment.evidence) {
                return Err(DomainContractError::NonCanonicalEvidence);
            }
        }
        Ok(())
    }

    /// Sorts assignments/evidence and rejects duplicate assignment IDs.
    ///
    /// # Errors
    /// Returns [`DomainContractError::DuplicateAssignment`] for duplicate IDs or an invalid
    /// interval error.
    pub fn canonicalize(&mut self) -> Result<(), DomainContractError> {
        self.assignments
            .sort_by(|left, right| left.id.cmp(&right.id));
        for pair in self.assignments.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(DomainContractError::DuplicateAssignment(pair[0].id.clone()));
            }
        }
        for assignment in &mut self.assignments {
            assignment.evidence.sort();
            assignment.evidence.dedup();
        }
        self.validate()
    }

    /// Computes the lowercase BLAKE3 hash of the canonical V1 solution.
    ///
    /// # Errors
    /// Rejects an invalid solution or an unexpected serialization failure.
    pub fn canonical_hash(&self) -> Result<String, DomainContractError> {
        self.validate()?;
        canonical_hash(self)
    }
}

fn strictly_sorted_unique<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

/// Computes a lowercase BLAKE3 digest for caller-defined canonical bytes.
#[must_use]
pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn canonical_hash(value: &impl Serialize) -> Result<String, DomainContractError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| DomainContractError::CanonicalSerialization(error.to_string()))?;
    if bytes.len() > MAX_DOMAIN_CONTRACT_JSON_BYTES {
        return Err(DomainContractError::LimitExceeded(
            "domain contract JSON bytes",
        ));
    }
    Ok(blake3_hex(&bytes))
}

fn ensure_domain_contract_size(value: &impl Serialize) -> Result<(), DomainContractError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| DomainContractError::CanonicalSerialization(error.to_string()))?;
    if bytes.len() > MAX_DOMAIN_CONTRACT_JSON_BYTES {
        Err(DomainContractError::LimitExceeded(
            "domain contract JSON bytes",
        ))
    } else {
        Ok(())
    }
}

fn ensure_portable_domain_contract_size(value: &impl Serialize) -> Result<(), DomainContractError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| DomainContractError::CanonicalSerialization(error.to_string()))?;
    validate_portable_domain_json(&bytes)
}

fn parse_domain_json<'de, T: Deserialize<'de>>(bytes: &'de [u8]) -> Result<T, DomainContractError> {
    if bytes.len() > MAX_DOMAIN_CONTRACT_JSON_BYTES {
        return Err(DomainContractError::LimitExceeded(
            "domain contract JSON bytes",
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| DomainContractError::MalformedJson(error.to_string()))
}

fn parse_portable_domain_json<'de, T: Deserialize<'de>>(
    bytes: &'de [u8],
) -> Result<T, DomainContractError> {
    validate_portable_domain_json(bytes)?;
    serde_json::from_slice(bytes)
        .map_err(|error| DomainContractError::MalformedJson(error.to_string()))
}

fn validate_portable_domain_json(bytes: &[u8]) -> Result<(), DomainContractError> {
    if bytes.len() > MAX_DOMAIN_CONTRACT_JSON_BYTES {
        return Err(DomainContractError::LimitExceeded(
            "domain contract JSON bytes",
        ));
    }
    validate_nonsecret_portable_json_bytes(
        bytes,
        &PortableJsonLimits {
            max_depth: MAX_DOMAIN_CONTRACT_JSON_DEPTH,
            max_string_bytes: MAX_VERIFICATION_TEXT_BYTES,
            max_collection_items: MAX_DOMAIN_CONTRACT_JSON_ITEMS,
        },
    )
    .map_err(|error| DomainContractError::PortableJsonViolation(error.to_string()))
}

fn checksum_without_field(
    value: &impl Serialize,
    field: &'static str,
) -> Result<String, DomainContractError> {
    let mut canonical = serde_json::to_value(value)
        .map_err(|error| DomainContractError::CanonicalSerialization(error.to_string()))?;
    let complete_bytes = serde_json::to_vec(&canonical)
        .map_err(|error| DomainContractError::CanonicalSerialization(error.to_string()))?;
    if complete_bytes.len() > MAX_DOMAIN_CONTRACT_JSON_BYTES {
        return Err(DomainContractError::LimitExceeded(
            "domain contract JSON bytes",
        ));
    }
    canonical
        .as_object_mut()
        .and_then(|object| object.remove(field))
        .ok_or(DomainContractError::CanonicalSerialization(
            "checksum field is missing".to_owned(),
        ))?;
    canonical_hash(&canonical)
}

fn valid_blake3(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_revision(value: u64) -> Result<(), DomainContractError> {
    if value > REVISION_MAX_V1 {
        Err(DomainContractError::LimitExceeded("scenario revision"))
    } else {
        Ok(())
    }
}

fn validate_hash(value: &str) -> Result<(), DomainContractError> {
    if valid_blake3(value) {
        Ok(())
    } else {
        Err(DomainContractError::InvalidBlake3)
    }
}

fn validate_message_key(value: &str) -> Result<(), DomainContractError> {
    if valid_id(value) {
        Ok(())
    } else {
        Err(DomainContractError::InvalidMessageKey)
    }
}

/// Direction of one authoritative score level.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OptimizationDirection {
    /// Smaller values are better.
    Minimize,
    /// Larger values are better.
    Maximize,
}

/// One ordered score level with a deterministic category breakdown.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreLevelValue {
    /// Stable level identity.
    pub level_id: ScoreLevelId,
    /// Authoritative value.
    pub value: i64,
    /// Comparison direction.
    pub direction: OptimizationDirection,
    /// Deterministic optional explanatory totals. Every present category must belong to this
    /// planning objective level, but categories may be omitted and totals need not sum to
    /// `value` unless the pack's own contract requires that relationship.
    pub category_breakdown: BTreeMap<ScoreCategoryId, i64>,
}

/// Ordered lexicographic authoritative score.
///
/// Feasibility is compared first and minimized; accepted results require it to be zero.
/// Level vector position defines precedence and identities/directions must match before values
/// can be compared.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreVector {
    /// Zero means independently verified feasible; positive values are violations.
    pub feasibility: i64,
    /// Highest precedence first.
    pub levels: Vec<ScoreLevelValue>,
}

impl ScoreVector {
    /// Lexicographically compares two scores.
    ///
    /// [`Ordering::Less`] means `self` is better. Feasibility is minimized, then each level
    /// follows its declared direction.
    ///
    /// # Errors
    /// Rejects an invalid current shape or level identity/direction mismatch.
    pub fn compare(&self, other: &Self) -> Result<Ordering, DomainContractError> {
        self.validate_current_shape()?;
        other.validate_current_shape()?;
        if self.levels.len() != other.levels.len() {
            return Err(DomainContractError::ScoreShapeMismatch);
        }
        let mut ordering = self.feasibility.cmp(&other.feasibility);
        for (left, right) in self.levels.iter().zip(&other.levels) {
            if left.level_id != right.level_id || left.direction != right.direction {
                return Err(DomainContractError::ScoreShapeMismatch);
            }
            if ordering == Ordering::Equal {
                ordering = match left.direction {
                    OptimizationDirection::Minimize => left.value.cmp(&right.value),
                    OptimizationDirection::Maximize => right.value.cmp(&left.value),
                };
            }
        }
        Ok(ordering)
    }

    /// Validates the released score identity and feasibility shape.
    ///
    /// # Errors
    /// Rejects negative feasibility or duplicate level IDs.
    pub fn validate_shape(&self) -> Result<(), DomainContractError> {
        if self.feasibility < 0 {
            return Err(DomainContractError::NegativeFeasibility);
        }
        let mut ids = BTreeSet::new();
        if self.levels.iter().any(|level| !ids.insert(&level.level_id)) {
            return Err(DomainContractError::DuplicateScoreLevel);
        }
        Ok(())
    }

    /// Validates the bounded current score shape.
    ///
    /// # Errors
    /// Rejects an invalid released shape or too many levels or category totals.
    pub fn validate_current_shape(&self) -> Result<(), DomainContractError> {
        self.validate_shape()?;
        if self.levels.len() > MAX_SCORE_LEVELS {
            return Err(DomainContractError::LimitExceeded("score levels"));
        }
        if self
            .levels
            .iter()
            .any(|level| level.category_breakdown.len() > MAX_SCORE_CATEGORIES_PER_LEVEL)
        {
            return Err(DomainContractError::LimitExceeded("score categories"));
        }
        Ok(())
    }
}

/// Typed value in verifier facts and localizable message parameters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    tag = "type",
    content = "value",
    deny_unknown_fields
)]
pub enum VerificationValue {
    /// Boolean fact.
    Boolean(bool),
    /// Signed integer fact.
    Integer(i64),
    /// Non-empty bounded UTF-8 text.
    Text(String),
    /// Stable affected entity.
    Entity(DomainEntityRef),
}

impl VerificationValue {
    fn validate(&self) -> Result<(), DomainContractError> {
        if let Self::Text(value) = self
            && (value.is_empty() || value.len() > MAX_VERIFICATION_TEXT_BYTES)
        {
            return Err(DomainContractError::InvalidVerificationText);
        }
        Ok(())
    }
}

/// Typed verifier-owned metric value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    tag = "type",
    content = "value",
    deny_unknown_fields
)]
pub enum MetricValue {
    /// Signed integer metric.
    Integer(i64),
    /// Exact rational metric; the denominator must be non-zero.
    Ratio {
        /// Signed numerator.
        numerator: i64,
        /// Positive denominator.
        denominator: u64,
    },
}

impl MetricValue {
    fn validate(&self) -> Result<(), DomainContractError> {
        if matches!(self, Self::Ratio { denominator: 0, .. }) {
            return Err(DomainContractError::InvalidMetricRatio);
        }
        Ok(())
    }
}

/// Semantic identity of one required rule in a verification scope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequiredRuleBinding {
    /// Stable scenario rule identity.
    pub rule_id: RuleId,
    /// Lowercase BLAKE3 hash of the rule/entity meaning evaluated by the verifier.
    pub semantic_hash: String,
}

/// Canonical V1 set of required rules for one immutable scenario revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationScope {
    /// Must equal [`VERIFICATION_SCOPE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Immutable scenario identity.
    pub scenario_id: ScenarioId,
    /// Immutable scenario revision.
    pub scenario_revision: u64,
    /// Required rules in ascending rule-ID order.
    pub required_rules: Vec<RequiredRuleBinding>,
    /// Lowercase BLAKE3 checksum of this scope excluding this field.
    pub checksum: String,
}

impl VerificationScope {
    /// Creates a canonical scope and computes its checksum.
    ///
    /// # Errors
    /// Rejects duplicate rules, malformed semantic hashes, or the configured rule bound.
    pub fn new(
        scenario_id: ScenarioId,
        scenario_revision: u64,
        mut required_rules: Vec<RequiredRuleBinding>,
    ) -> Result<Self, DomainContractError> {
        validate_revision(scenario_revision)?;
        if required_rules.len() > MAX_VERIFICATION_RULES {
            return Err(DomainContractError::LimitExceeded("required rules"));
        }
        required_rules.sort_by_key(|binding| binding.rule_id);
        for pair in required_rules.windows(2) {
            if pair[0].rule_id == pair[1].rule_id {
                return Err(DomainContractError::DuplicateRequiredRule(pair[0].rule_id));
            }
        }
        for binding in &required_rules {
            validate_hash(&binding.semantic_hash)?;
        }
        let mut scope = Self {
            schema_version: VERIFICATION_SCOPE_SCHEMA_VERSION,
            scenario_id,
            scenario_revision,
            required_rules,
            checksum: String::new(),
        };
        scope.checksum = checksum_without_field(&scope, "checksum")?;
        ensure_domain_contract_size(&scope)?;
        Ok(scope)
    }

    /// Strictly parses and validates a verification scope.
    ///
    /// # Errors
    /// Rejects malformed JSON and any scope invariant violation.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates version, bounds, canonical rule bindings, and checksum.
    ///
    /// # Errors
    /// Returns the first violated contract invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.schema_version != VERIFICATION_SCOPE_SCHEMA_VERSION {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        validate_revision(self.scenario_revision)?;
        if self.required_rules.len() > MAX_VERIFICATION_RULES {
            return Err(DomainContractError::LimitExceeded("required rules"));
        }
        if !strictly_sorted_unique_by(&self.required_rules, |binding| &binding.rule_id) {
            return Err(DomainContractError::NonCanonicalRequiredRules);
        }
        for binding in &self.required_rules {
            validate_hash(&binding.semantic_hash)?;
        }
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }
}

/// V1 immutable inputs and canonical bindings supplied to a domain verifier.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationContextV1 {
    /// Must equal [`VERIFICATION_CONTEXT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Immutable scenario identity.
    pub scenario_id: ScenarioId,
    /// Exact scenario revision being evaluated.
    pub evaluated_revision: u64,
    /// Lowercase BLAKE3 hash of the normalized scenario document.
    pub document_hash: String,
    /// Lowercase BLAKE3 hash of the canonical planning model.
    pub planning_model_hash: String,
    /// Lowercase BLAKE3 hash of the canonical normalized solution.
    pub normalized_solution_hash: String,
    /// Checksum of the exact required-rule scope.
    pub verification_scope_checksum: String,
}

impl VerificationContextV1 {
    /// Creates and validates a verifier context.
    ///
    /// # Errors
    /// Rejects any malformed lowercase BLAKE3 binding.
    pub fn new(
        scenario_id: ScenarioId,
        evaluated_revision: u64,
        document_hash: String,
        planning_model_hash: String,
        normalized_solution_hash: String,
        verification_scope_checksum: String,
    ) -> Result<Self, DomainContractError> {
        let value = Self {
            schema_version: VERIFICATION_CONTEXT_SCHEMA_VERSION,
            scenario_id,
            evaluated_revision,
            document_hash,
            planning_model_hash,
            normalized_solution_hash,
            verification_scope_checksum,
        };
        value.validate()?;
        Ok(value)
    }

    /// Strictly parses and validates a verification context.
    ///
    /// # Errors
    /// Rejects malformed JSON and any context invariant violation.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates the context version and all hash bindings.
    ///
    /// # Errors
    /// Returns the first violated contract invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.schema_version != VERIFICATION_CONTEXT_SCHEMA_VERSION {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        validate_revision(self.evaluated_revision)?;
        for binding in [
            &self.document_hash,
            &self.planning_model_hash,
            &self.normalized_solution_hash,
            &self.verification_scope_checksum,
        ] {
            validate_hash(binding)?;
        }
        Ok(())
    }
}

/// Independent result for one required domain rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleEvaluation {
    /// Stable required-rule identity.
    pub rule_id: RuleId,
    /// Whether the required rule is satisfied.
    pub satisfied: bool,
    /// Related entities in ascending canonical order.
    pub affected_entities: Vec<DomainEntityRef>,
    /// Stable localization key.
    pub message_key: String,
    /// Typed expected facts keyed by stable fact identity.
    pub expected: BTreeMap<VerificationFactId, VerificationValue>,
    /// Typed observed facts keyed by stable fact identity.
    pub observed: BTreeMap<VerificationFactId, VerificationValue>,
    /// Evidence references in ascending canonical order.
    pub evidence: Vec<DomainEvidenceId>,
}

impl RuleEvaluation {
    fn canonicalize(&mut self) -> Result<(), DomainContractError> {
        self.affected_entities.sort();
        if !strictly_sorted_unique(&self.affected_entities) && self.affected_entities.len() > 1 {
            return Err(DomainContractError::DuplicateAffectedEntity);
        }
        self.evidence.sort();
        if !strictly_sorted_unique(&self.evidence) && self.evidence.len() > 1 {
            return Err(DomainContractError::DuplicateVerificationEvidence);
        }
        self.validate()
    }

    /// Validates bounds, canonical collections, message identity, and typed facts.
    ///
    /// # Errors
    /// Returns the first violated record invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_message_key(&self.message_key)?;
        if self.affected_entities.len() > MAX_VERIFICATION_ENTITIES {
            return Err(DomainContractError::LimitExceeded("affected entities"));
        }
        if !strictly_sorted_unique(&self.affected_entities) {
            return Err(DomainContractError::NonCanonicalAffectedEntities);
        }
        if self.evidence.len() > MAX_VERIFICATION_EVIDENCE {
            return Err(DomainContractError::LimitExceeded("verification evidence"));
        }
        if !strictly_sorted_unique(&self.evidence) {
            return Err(DomainContractError::NonCanonicalVerificationEvidence);
        }
        validate_fact_map(&self.expected)?;
        validate_fact_map(&self.observed)
    }
}

/// Non-blocking, typed verifier warning.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationWarning {
    /// Stable warning identity.
    pub id: VerificationWarningId,
    /// Stable localization key.
    pub message_key: String,
    /// Related entities in ascending canonical order.
    pub affected_entities: Vec<DomainEntityRef>,
    /// Typed warning facts keyed by stable fact identity.
    pub facts: BTreeMap<VerificationFactId, VerificationValue>,
}

impl VerificationWarning {
    fn canonicalize(&mut self) -> Result<(), DomainContractError> {
        self.affected_entities.sort();
        if !strictly_sorted_unique(&self.affected_entities) && self.affected_entities.len() > 1 {
            return Err(DomainContractError::DuplicateAffectedEntity);
        }
        self.validate()
    }

    /// Validates bounds, canonical entities, message identity, and typed facts.
    ///
    /// # Errors
    /// Returns the first violated record invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_message_key(&self.message_key)?;
        if self.affected_entities.len() > MAX_VERIFICATION_ENTITIES {
            return Err(DomainContractError::LimitExceeded("affected entities"));
        }
        if !strictly_sorted_unique(&self.affected_entities) {
            return Err(DomainContractError::NonCanonicalAffectedEntities);
        }
        validate_fact_map(&self.facts)
    }
}

fn validate_fact_map(
    facts: &BTreeMap<VerificationFactId, VerificationValue>,
) -> Result<(), DomainContractError> {
    if facts.len() > MAX_VERIFICATION_FACTS {
        return Err(DomainContractError::LimitExceeded("verification facts"));
    }
    for value in facts.values() {
        value.validate()?;
    }
    Ok(())
}

fn validate_metrics(metrics: &BTreeMap<MetricId, MetricValue>) -> Result<(), DomainContractError> {
    if metrics.len() > MAX_VERIFICATION_METRICS {
        return Err(DomainContractError::LimitExceeded("verification metrics"));
    }
    for value in metrics.values() {
        value.validate()?;
    }
    Ok(())
}

/// Current V2 independent-verification report.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationReport {
    /// Must equal [`VERIFICATION_REPORT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// True exactly when every required-rule evaluation is satisfied.
    pub accepted: bool,
    /// Immutable scenario identity.
    pub scenario_id: ScenarioId,
    /// Exact scenario revision evaluated.
    pub evaluated_revision: u64,
    /// Scenario document hash copied from the verifier context.
    pub document_hash: String,
    /// Planning model hash copied from the verifier context.
    pub planning_model_hash: String,
    /// Normalized solution hash copied from the verifier context.
    pub normalized_solution_hash: String,
    /// Required-rule scope checksum copied from the verifier context.
    pub verification_scope_checksum: String,
    /// Every required-rule evaluation in ascending rule-ID order.
    pub required_rule_results: Vec<RuleEvaluation>,
    /// Authoritatively recomputed score.
    pub score: ScoreVector,
    /// Non-blocking warnings in ascending warning-ID order.
    pub warnings: Vec<VerificationWarning>,
    /// Typed verifier metrics.
    pub metrics: BTreeMap<MetricId, MetricValue>,
    /// Lowercase BLAKE3 checksum of this report excluding this field.
    pub checksum: String,
}

impl VerificationReport {
    /// Creates a canonical V2 report and computes acceptance and checksum.
    ///
    /// # Errors
    /// Rejects invalid bindings, duplicate records, bounds, facts, metrics, or score shape.
    pub fn new(
        context: &VerificationContextV1,
        mut required_rule_results: Vec<RuleEvaluation>,
        score: ScoreVector,
        mut warnings: Vec<VerificationWarning>,
        metrics: BTreeMap<MetricId, MetricValue>,
    ) -> Result<Self, DomainContractError> {
        context.validate()?;
        if required_rule_results.len() > MAX_VERIFICATION_RULES {
            return Err(DomainContractError::LimitExceeded("rule evaluations"));
        }
        for evaluation in &mut required_rule_results {
            evaluation.canonicalize()?;
        }
        required_rule_results.sort_by_key(|evaluation| evaluation.rule_id);
        for pair in required_rule_results.windows(2) {
            if pair[0].rule_id == pair[1].rule_id {
                return Err(DomainContractError::DuplicateRuleEvaluation(
                    pair[0].rule_id,
                ));
            }
        }
        if warnings.len() > MAX_VERIFICATION_WARNINGS {
            return Err(DomainContractError::LimitExceeded("verification warnings"));
        }
        for warning in &mut warnings {
            warning.canonicalize()?;
        }
        warnings.sort_by(|left, right| left.id.cmp(&right.id));
        for pair in warnings.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(DomainContractError::DuplicateVerificationWarning);
            }
        }
        score.validate_current_shape()?;
        validate_metrics(&metrics)?;
        let accepted = required_rule_results.iter().all(|result| result.satisfied);
        let mut report = Self {
            schema_version: VERIFICATION_REPORT_SCHEMA_VERSION,
            accepted,
            scenario_id: context.scenario_id,
            evaluated_revision: context.evaluated_revision,
            document_hash: context.document_hash.clone(),
            planning_model_hash: context.planning_model_hash.clone(),
            normalized_solution_hash: context.normalized_solution_hash.clone(),
            verification_scope_checksum: context.verification_scope_checksum.clone(),
            required_rule_results,
            score,
            warnings,
            metrics,
            checksum: String::new(),
        };
        report.checksum = checksum_without_field(&report, "checksum")?;
        ensure_domain_contract_size(&report)?;
        Ok(report)
    }

    /// Strictly parses and validates a V2 report.
    ///
    /// # Errors
    /// Rejects malformed JSON and any report invariant violation.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates version, bindings, canonical content, acceptance, and checksum.
    ///
    /// # Errors
    /// Returns the first violated contract invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.schema_version != VERIFICATION_REPORT_SCHEMA_VERSION {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        validate_revision(self.evaluated_revision)?;
        for binding in [
            &self.document_hash,
            &self.planning_model_hash,
            &self.normalized_solution_hash,
            &self.verification_scope_checksum,
            &self.checksum,
        ] {
            validate_hash(binding)?;
        }
        if self.required_rule_results.len() > MAX_VERIFICATION_RULES {
            return Err(DomainContractError::LimitExceeded("rule evaluations"));
        }
        if !strictly_sorted_unique_by(&self.required_rule_results, |result| &result.rule_id) {
            return Err(DomainContractError::NonCanonicalRuleEvaluations);
        }
        for evaluation in &self.required_rule_results {
            evaluation.validate()?;
        }
        self.score.validate_current_shape()?;
        if self.warnings.len() > MAX_VERIFICATION_WARNINGS {
            return Err(DomainContractError::LimitExceeded("verification warnings"));
        }
        if !strictly_sorted_unique_by(&self.warnings, |warning| &warning.id) {
            return Err(DomainContractError::NonCanonicalVerificationWarnings);
        }
        for warning in &self.warnings {
            warning.validate()?;
        }
        validate_metrics(&self.metrics)?;
        let accepted = self
            .required_rule_results
            .iter()
            .all(|result| result.satisfied);
        if self.accepted != accepted {
            return Err(DomainContractError::InconsistentVerification);
        }
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }
}

fn strictly_sorted_unique_by<T, K: Ord>(values: &[T], key: impl Fn(&T) -> &K) -> bool {
    values.windows(2).all(|pair| key(&pair[0]) < key(&pair[1]))
}

/// Current V2 validated pairing of a normalized solution and independent report.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptedResult {
    /// Must equal [`ACCEPTED_RESULT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Projected candidate.
    pub solution: NormalizedSolution,
    /// Independent V2 report bound to this exact solution.
    pub verification: VerificationReport,
    /// Lowercase BLAKE3 checksum of this result excluding this field.
    pub checksum: String,
}

impl AcceptedResult {
    /// Validates and binds an accepted result, then computes its checksum.
    ///
    /// # Errors
    /// Rejects invalid inputs, a scenario/revision/hash mismatch, or a non-accepted report.
    pub fn new(
        solution: NormalizedSolution,
        verification: VerificationReport,
    ) -> Result<Self, DomainContractError> {
        validate_result_binding(&solution, &verification)?;
        let mut result = Self {
            schema_version: ACCEPTED_RESULT_SCHEMA_VERSION,
            solution,
            verification,
            checksum: String::new(),
        };
        result.checksum = checksum_without_field(&result, "checksum")?;
        ensure_domain_contract_size(&result)?;
        Ok(result)
    }

    /// Strictly parses and validates a V2 accepted result.
    ///
    /// # Errors
    /// Rejects malformed JSON and any accepted-result invariant violation.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates version, solution/report binding, and result checksum.
    ///
    /// # Errors
    /// Returns the first violated contract invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.schema_version != ACCEPTED_RESULT_SCHEMA_VERSION {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        validate_result_binding(&self.solution, &self.verification)?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }
}

fn validate_result_binding(
    solution: &NormalizedSolution,
    verification: &VerificationReport,
) -> Result<(), DomainContractError> {
    solution.validate()?;
    verification.validate()?;
    if solution.scenario_id != verification.scenario_id
        || solution.scenario_revision != verification.evaluated_revision
        || solution.canonical_hash()? != verification.normalized_solution_hash
    {
        return Err(DomainContractError::VerificationBindingMismatch);
    }
    if !verification.accepted || verification.score.feasibility != 0 {
        return Err(DomainContractError::NotVerifiedFeasible);
    }
    Ok(())
}

/// Version-aware canonical semantic preimage for a solver request hash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunRequestSemantics {
    /// Recorded preimage version: legacy one or current two.
    pub schema_version: u32,
    /// Immutable scenario identity.
    pub scenario_id: ScenarioId,
    /// Exact scenario revision requested.
    pub scenario_revision: u64,
    /// Domain pack used to compile the request.
    pub pack_id: PackId,
    /// Domain-pack schema version.
    pub pack_schema_version: u32,
    /// Planning IR schema version.
    pub planning_ir_schema_version: u32,
    /// Domain compiler version.
    pub compiler_version: String,
    /// Application version.
    pub application_version: String,
    /// Selected backend identity.
    pub backend_id: BackendId,
    /// Selected backend version.
    pub backend_version: String,
    /// Backend adapter version.
    pub adapter_version: String,
    /// Isolated worker version.
    pub worker_version: String,
    /// Solver engine version.
    pub solver_version: String,
    /// Worker protocol major version.
    pub protocol_major: u32,
    /// Worker protocol minor version.
    pub protocol_minor: u32,
    /// Lowercase BLAKE3 hash of the canonical planning model.
    pub model_hash: String,
    /// Lowercase BLAKE3 hash of the canonical objective policy.
    pub objective_policy_hash: String,
    /// Exact canonical solve options.
    pub solve_options: SolveOptions,
    /// Scenario time zone governing local-time interpretation.
    pub scenario_timezone: IanaTimeZone,
    /// Optional lowercase BLAKE3 hash of the temporary-condition set.
    pub temporary_condition_hash: Option<String>,
    /// Recorded current semantics; absent only for exact historical version-one replay.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub objective_context: Option<RunObjectiveContext>,
}

impl RunRequestSemantics {
    /// Validates the complete request-hash semantic preimage.
    ///
    /// # Errors
    /// Rejects invalid revisions, hashes, versions, options, or backend selection bindings.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        validate_recorded_objective_context(
            self.schema_version,
            self.objective_context.as_ref(),
            &self.pack_id,
            self.scenario_id,
        )?;
        validate_revision(self.scenario_revision)?;
        for value in [&self.model_hash, &self.objective_policy_hash] {
            validate_hash(value)?;
        }
        if let Some(value) = &self.temporary_condition_hash {
            validate_hash(value)?;
        }
        self.solve_options
            .validate()
            .map_err(|_| DomainContractError::InvalidSolveOptions)?;
        if let BackendSelection::Specific(backend_id) = &self.solve_options.backend
            && backend_id != &self.backend_id
        {
            return Err(DomainContractError::RunInputBackendMismatch);
        }
        for (field, value) in [
            ("compiler version", self.compiler_version.as_str()),
            ("application version", self.application_version.as_str()),
            ("backend version", self.backend_version.as_str()),
            ("adapter version", self.adapter_version.as_str()),
            ("worker version", self.worker_version.as_str()),
            ("solver version", self.solver_version.as_str()),
        ] {
            validate_run_version(field, value)?;
        }
        if self.pack_schema_version == 0 {
            return Err(DomainContractError::ZeroVersion("pack schema version"));
        }
        if self.planning_ir_schema_version == 0 {
            return Err(DomainContractError::ZeroVersion(
                "planning IR schema version",
            ));
        }
        if self.protocol_major == 0 {
            return Err(DomainContractError::ZeroVersion("protocol major"));
        }
        Ok(())
    }

    /// Computes the lowercase BLAKE3 hash of this canonical semantic preimage.
    ///
    /// # Errors
    /// Rejects an invalid semantic preimage or an oversized canonical serialization.
    pub fn canonical_hash(&self) -> Result<String, DomainContractError> {
        self.validate()?;
        canonical_hash(self)
    }
}

/// Immutable, solver-neutral inputs that identify one solve run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunInput {
    /// Recorded input version: legacy one or current two.
    pub schema_version: u32,
    /// Stable identity of the run.
    pub run_id: SolveRunId,
    /// Request correlation identity.
    pub request_id: RequestId,
    /// Lowercase BLAKE3 hash of [`RunRequestSemantics`].
    pub request_hash: String,
    /// Immutable scenario identity.
    pub scenario_id: ScenarioId,
    /// Exact scenario revision captured for the run.
    pub scenario_revision: u64,
    /// Stable identity of the immutable scenario snapshot.
    pub snapshot_id: ScenarioSnapshotId,
    /// Lowercase BLAKE3 hash of the canonical snapshot document.
    pub snapshot_document_hash: String,
    /// Time at which the immutable snapshot was created.
    pub snapshot_created_at: Rfc3339Timestamp,
    /// Domain pack used to compile the snapshot.
    pub pack_id: PackId,
    /// Domain-pack schema version.
    pub pack_schema_version: u32,
    /// Planning IR schema version.
    pub planning_ir_schema_version: u32,
    /// Domain compiler version.
    pub compiler_version: String,
    /// Application version.
    pub application_version: String,
    /// Selected backend identity.
    pub backend_id: BackendId,
    /// Selected backend version.
    pub backend_version: String,
    /// Backend adapter version.
    pub adapter_version: String,
    /// Isolated worker version.
    pub worker_version: String,
    /// Solver engine version.
    pub solver_version: String,
    /// Worker protocol major version.
    pub protocol_major: u32,
    /// Worker protocol minor version.
    pub protocol_minor: u32,
    /// Lowercase BLAKE3 hash of the canonical planning model.
    pub model_hash: String,
    /// Lowercase BLAKE3 hash of the canonical objective policy.
    pub objective_policy_hash: String,
    /// Exact canonical solve options used by the run.
    pub solve_options: SolveOptions,
    /// Scenario time zone governing local-time interpretation.
    pub scenario_timezone: IanaTimeZone,
    /// Optional lowercase BLAKE3 hash of the temporary-condition set.
    pub temporary_condition_hash: Option<String>,
    /// Exact recorded objective semantics, never reconstructed from score or display labels.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub objective_context: Option<RunObjectiveContext>,
    /// Lowercase BLAKE3 checksum of this input excluding this field.
    pub checksum: String,
}

impl RunInput {
    /// Creates one immutable run-input record, derives its request hash, and computes its checksum.
    ///
    /// # Errors
    /// Rejects invalid request semantics, snapshot hashes, or oversized canonical serialization.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: SolveRunId,
        request_id: RequestId,
        scenario_id: ScenarioId,
        scenario_revision: u64,
        snapshot_id: ScenarioSnapshotId,
        snapshot_document_hash: String,
        snapshot_created_at: Rfc3339Timestamp,
        pack_id: PackId,
        pack_schema_version: u32,
        planning_ir_schema_version: u32,
        compiler_version: String,
        application_version: String,
        backend_id: BackendId,
        backend_version: String,
        adapter_version: String,
        worker_version: String,
        solver_version: String,
        protocol_major: u32,
        protocol_minor: u32,
        model_hash: String,
        objective_policy_hash: String,
        solve_options: SolveOptions,
        scenario_timezone: IanaTimeZone,
        temporary_condition_hash: Option<String>,
        objective_context: Option<RunObjectiveContext>,
    ) -> Result<Self, DomainContractError> {
        let mut input = Self {
            schema_version: if objective_context.is_some() {
                RUN_INPUT_SCHEMA_VERSION
            } else {
                1
            },
            run_id,
            request_id,
            request_hash: String::new(),
            scenario_id,
            scenario_revision,
            snapshot_id,
            snapshot_document_hash,
            snapshot_created_at,
            pack_id,
            pack_schema_version,
            planning_ir_schema_version,
            compiler_version,
            application_version,
            backend_id,
            backend_version,
            adapter_version,
            worker_version,
            solver_version,
            protocol_major,
            protocol_minor,
            model_hash,
            objective_policy_hash,
            solve_options,
            scenario_timezone,
            temporary_condition_hash,
            objective_context,
            checksum: String::new(),
        };
        input.request_hash = input.request_semantics().canonical_hash()?;
        input.validate_payload()?;
        input.checksum = checksum_without_field(&input, "checksum")?;
        ensure_domain_contract_size(&input)?;
        Ok(input)
    }

    /// Returns the canonical semantic preimage bound by `request_hash`.
    #[must_use]
    pub fn request_semantics(&self) -> RunRequestSemantics {
        RunRequestSemantics {
            schema_version: self.schema_version,
            scenario_id: self.scenario_id,
            scenario_revision: self.scenario_revision,
            pack_id: self.pack_id.clone(),
            pack_schema_version: self.pack_schema_version,
            planning_ir_schema_version: self.planning_ir_schema_version,
            compiler_version: self.compiler_version.clone(),
            application_version: self.application_version.clone(),
            backend_id: self.backend_id.clone(),
            backend_version: self.backend_version.clone(),
            adapter_version: self.adapter_version.clone(),
            worker_version: self.worker_version.clone(),
            solver_version: self.solver_version.clone(),
            protocol_major: self.protocol_major,
            protocol_minor: self.protocol_minor,
            model_hash: self.model_hash.clone(),
            objective_policy_hash: self.objective_policy_hash.clone(),
            solve_options: self.solve_options.clone(),
            scenario_timezone: self.scenario_timezone.clone(),
            temporary_condition_hash: self.temporary_condition_hash.clone(),
            objective_context: self.objective_context.clone(),
        }
    }

    /// Strictly parses and validates a retained version-one or version-two run input.
    ///
    /// # Errors
    /// Rejects malformed JSON and every run-input invariant violation.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates the complete run-input contract and checksum.
    ///
    /// # Errors
    /// Returns the first violated input invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        validate_recorded_objective_context(
            self.schema_version,
            self.objective_context.as_ref(),
            &self.pack_id,
            self.scenario_id,
        )?;
        validate_hash(&self.snapshot_document_hash)?;
        validate_hash(&self.request_hash)?;
        if self.request_semantics().canonical_hash()? != self.request_hash {
            return Err(DomainContractError::RequestHashMismatch);
        }
        Ok(())
    }
}

fn validate_recorded_objective_context(
    version: u32,
    context: Option<&RunObjectiveContext>,
    pack_id: &PackId,
    scenario_id: ScenarioId,
) -> Result<(), DomainContractError> {
    match (version, context) {
        (1, None) => Ok(()),
        (2, Some(context)) => context.validate_for(pack_id, scenario_id),
        (1 | 2, _) => Err(DomainContractError::InvalidObjectiveContext(
            "recorded presence",
        )),
        _ => Err(DomainContractError::UnsupportedVersion(version)),
    }
}

fn validate_run_version(field: &'static str, value: &str) -> Result<(), DomainContractError> {
    if !value.is_empty()
        && value.len() <= MAX_RUN_VERSION_BYTES
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
    {
        Ok(())
    } else {
        Err(DomainContractError::InvalidRunVersion(field))
    }
}

/// Optional bounded phase durations retained for one run.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunPhaseTimingsV1 {
    /// Scenario compilation duration.
    pub compile_milliseconds: Option<DurationMillis>,
    /// Backend execution duration.
    pub backend_milliseconds: Option<DurationMillis>,
    /// Candidate projection duration.
    pub projection_milliseconds: Option<DurationMillis>,
    /// Structural validation duration.
    pub structural_validation_milliseconds: Option<DurationMillis>,
    /// Authoritative score recomputation duration.
    pub score_recomputation_milliseconds: Option<DurationMillis>,
    /// Required-rule verification duration.
    pub required_rule_verification_milliseconds: Option<DurationMillis>,
    /// Deterministic evidence materialization performed immediately before atomic persistence.
    ///
    /// The database commit is deliberately excluded because this immutable, self-checksummed
    /// manifest must be constructed before the one transaction that persists it.
    pub evidence_persistence_milliseconds: Option<DurationMillis>,
    /// Optional explanation duration.
    pub optional_explanation_milliseconds: Option<DurationMillis>,
}

/// Solver-neutral terminal classification for one run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    deny_unknown_fields
)]
pub enum RunTerminalOutcomeV1 {
    /// A candidate passed independent acceptance.
    Accepted {
        /// Normalized terminal solve status.
        status: SolveStatus,
        /// Accepted normalized solution identity.
        solution_id: SolutionId,
        /// Checksum of the accepted-result contract.
        accepted_result_checksum: String,
        /// Checksum of the independent verification report.
        verification_checksum: String,
    },
    /// The run terminated without an accepted result.
    NoResult {
        /// Normalized terminal solve status.
        status: SolveStatus,
    },
    /// Independent verification raised a correctness alarm.
    VerificationAlarm {
        /// Stable safe diagnostic code without display text.
        diagnostic_code: String,
    },
    /// A legacy or incomplete run was interrupted before a terminal result was recorded.
    Interrupted,
}

impl RunTerminalOutcomeV1 {
    /// Validates status partitioning and bounded retained bindings.
    ///
    /// # Errors
    /// Rejects an invalid status, checksum, or verification diagnostic code.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        match self {
            Self::Accepted {
                status,
                accepted_result_checksum,
                verification_checksum,
                ..
            } => {
                if !matches!(status, SolveStatus::Optimal | SolveStatus::Feasible) {
                    return Err(DomainContractError::InvalidRunOutcomeStatus);
                }
                validate_hash(accepted_result_checksum)?;
                validate_hash(verification_checksum)
            }
            Self::NoResult { status } => {
                if matches!(
                    status,
                    SolveStatus::Infeasible
                        | SolveStatus::Unbounded
                        | SolveStatus::NoSolutionWithinLimit
                        | SolveStatus::Cancelled
                        | SolveStatus::InvalidModel
                        | SolveStatus::BackendUnavailable
                        | SolveStatus::BackendFailed
                ) {
                    Ok(())
                } else {
                    Err(DomainContractError::InvalidRunOutcomeStatus)
                }
            }
            Self::VerificationAlarm { diagnostic_code } => {
                if diagnostic_code.len() <= MAX_RUN_DIAGNOSTIC_CODE_BYTES
                    && valid_id(diagnostic_code)
                {
                    Ok(())
                } else {
                    Err(DomainContractError::InvalidRunDiagnosticCode)
                }
            }
            Self::Interrupted => Ok(()),
        }
    }
}

/// Exact clock-observation failure marker retained only on a quarantine manifest.
///
/// This is intentionally closed: a clock failure does not itself establish a
/// verification alarm, and producers must attach it only after retaining an
/// independently established non-clock alarm.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum RunClockFailureV1 {
    /// The final wall/elapsed observation could not be trusted.
    UnreliableObservation {},
}

/// Terminal, checksummed record for one immutable run input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunManifest {
    /// Writers emit [`RUN_MANIFEST_SCHEMA_VERSION`]; readers also accept legacy version 1.
    pub schema_version: u32,
    /// Stable identity of the terminal run.
    pub run_id: SolveRunId,
    /// Checksum of the exact [`RunInput`] used by the run.
    pub run_input_checksum: String,
    /// Solver-neutral terminal outcome.
    pub outcome: RunTerminalOutcomeV1,
    /// Optional marker for a trusted alarm whose terminal clock observation was unreliable.
    ///
    /// The marker is omitted for every normally measured manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_failure: Option<RunClockFailureV1>,
    /// Wall-clock time at which execution began.
    pub started_at: Rfc3339Timestamp,
    /// Wall-clock time at which terminal finalization completed.
    pub finished_at: Rfc3339Timestamp,
    /// Parent-measured end-to-end elapsed duration, absent only for interrupted recovery.
    pub elapsed_milliseconds: Option<DurationMillis>,
    /// Elapsed time to the first backend incumbent, if one was observed.
    pub first_incumbent_milliseconds: Option<DurationMillis>,
    /// Elapsed time to the first independently verified feasible candidate.
    pub first_verified_feasible_milliseconds: Option<DurationMillis>,
    /// Optional bounded phase durations.
    pub phase_timings: RunPhaseTimingsV1,
    /// Canonical verifier warnings in ascending warning-ID order.
    pub verification_warnings: Vec<VerificationWarning>,
    /// Descriptive admitted router/native claims, distinct from this application's terminal outcome.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub native_observations: Option<RunNativeObservations>,
    /// Lowercase BLAKE3 checksum of this manifest excluding this field.
    pub checksum: String,
}

impl RunManifest {
    /// Creates a canonical terminal manifest and computes its checksum.
    ///
    /// # Errors
    /// Rejects an invalid outcome, timing invariant, warning set, or run-input checksum.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: SolveRunId,
        run_input_checksum: String,
        outcome: RunTerminalOutcomeV1,
        started_at: Rfc3339Timestamp,
        finished_at: Rfc3339Timestamp,
        elapsed_milliseconds: Option<DurationMillis>,
        first_incumbent_milliseconds: Option<DurationMillis>,
        first_verified_feasible_milliseconds: Option<DurationMillis>,
        phase_timings: RunPhaseTimingsV1,
        verification_warnings: Vec<VerificationWarning>,
    ) -> Result<Self, DomainContractError> {
        Self::new_with_clock_failure(
            run_id,
            run_input_checksum,
            outcome,
            started_at,
            finished_at,
            elapsed_milliseconds,
            first_incumbent_milliseconds,
            first_verified_feasible_milliseconds,
            phase_timings,
            verification_warnings,
            None,
        )
    }

    /// Creates an all-unmeasured verification alarm for an unreliable terminal clock observation.
    ///
    /// The supplied wall-clock observations are retained exactly, including a regressed
    /// `finished_at`; no duration or phase timing is inferred or substituted.
    ///
    /// # Errors
    /// Rejects an invalid run identity, diagnostic code, or checksum payload.
    pub fn new_clock_failed_alarm(
        run_id: SolveRunId,
        run_input_checksum: String,
        diagnostic_code: String,
        started_at: Rfc3339Timestamp,
        finished_at: Rfc3339Timestamp,
    ) -> Result<Self, DomainContractError> {
        Self::new_with_clock_failure(
            run_id,
            run_input_checksum,
            RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code },
            started_at,
            finished_at,
            None,
            None,
            None,
            RunPhaseTimingsV1::default(),
            Vec::new(),
            Some(RunClockFailureV1::UnreliableObservation {}),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_clock_failure(
        run_id: SolveRunId,
        run_input_checksum: String,
        outcome: RunTerminalOutcomeV1,
        started_at: Rfc3339Timestamp,
        finished_at: Rfc3339Timestamp,
        elapsed_milliseconds: Option<DurationMillis>,
        first_incumbent_milliseconds: Option<DurationMillis>,
        first_verified_feasible_milliseconds: Option<DurationMillis>,
        phase_timings: RunPhaseTimingsV1,
        mut verification_warnings: Vec<VerificationWarning>,
        clock_failure: Option<RunClockFailureV1>,
    ) -> Result<Self, DomainContractError> {
        if verification_warnings.len() > MAX_VERIFICATION_WARNINGS {
            return Err(DomainContractError::LimitExceeded("verification warnings"));
        }
        for warning in &mut verification_warnings {
            warning.canonicalize()?;
        }
        verification_warnings.sort_by(|left, right| left.id.cmp(&right.id));
        for pair in verification_warnings.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(DomainContractError::DuplicateVerificationWarning);
            }
        }
        let mut manifest = Self {
            schema_version: RUN_MANIFEST_SCHEMA_VERSION,
            run_id,
            run_input_checksum,
            outcome,
            clock_failure,
            started_at,
            finished_at,
            elapsed_milliseconds,
            first_incumbent_milliseconds,
            first_verified_feasible_milliseconds,
            phase_timings,
            verification_warnings,
            native_observations: None,
            checksum: String::new(),
        };
        manifest.validate_payload()?;
        manifest.checksum = checksum_without_field(&manifest, "checksum")?;
        ensure_domain_contract_size(&manifest)?;
        Ok(manifest)
    }

    /// Packages compact claims without changing application outcome, timings or authority.
    /// Exact input binding is checked by every containing run envelope and durable admission.
    ///
    /// # Errors
    /// Rejects invalid native observations or an incompatible manifest version.
    pub fn with_native_observations(
        mut self,
        observations: Option<RunNativeObservations>,
    ) -> Result<Self, DomainContractError> {
        self.native_observations = observations;
        self.validate_payload()?;
        self.checksum = checksum_without_field(&self, "checksum")?;
        ensure_domain_contract_size(&self)?;
        Ok(self)
    }

    /// Checks native claims against the exact input after the caller validates the manifest itself.
    ///
    /// # Errors
    /// Rejects manifest/input identity, model/options/backend/runtime substitution.
    pub fn validate_native_input_binding(
        &self,
        input: &RunInput,
    ) -> Result<(), DomainContractError> {
        if self.run_id != input.run_id || self.run_input_checksum != input.checksum {
            return Err(DomainContractError::InvalidNativeObservations(
                "manifest input binding",
            ));
        }
        if let Some(observations) = &self.native_observations {
            observations.validate_input_bindings(input)?;
        }
        Ok(())
    }

    /// Strictly parses and validates a supported legacy or current terminal run manifest.
    ///
    /// # Errors
    /// Rejects malformed JSON and every manifest invariant violation.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates the complete manifest contract and checksum.
    ///
    /// # Errors
    /// Returns the first violated manifest invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        if !matches!(self.schema_version, 1 | RUN_MANIFEST_SCHEMA_VERSION) {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        if self.schema_version == 1 && self.native_observations.is_some() {
            return Err(DomainContractError::InvalidNativeObservations(
                "legacy manifest field",
            ));
        }
        if let Some(observations) = &self.native_observations {
            observations.validate()?;
        }
        validate_hash(&self.run_input_checksum)?;
        self.outcome.validate()?;
        let phase_timings = [
            self.phase_timings.compile_milliseconds,
            self.phase_timings.backend_milliseconds,
            self.phase_timings.projection_milliseconds,
            self.phase_timings.structural_validation_milliseconds,
            self.phase_timings.score_recomputation_milliseconds,
            self.phase_timings.required_rule_verification_milliseconds,
            self.phase_timings.evidence_persistence_milliseconds,
            self.phase_timings.optional_explanation_milliseconds,
        ];
        let interrupted = matches!(&self.outcome, RunTerminalOutcomeV1::Interrupted);
        let clock_failed_alarm = self.clock_failure.is_some();
        if clock_failed_alarm
            && (!matches!(
                &self.outcome,
                RunTerminalOutcomeV1::VerificationAlarm { .. }
            ) || self.elapsed_milliseconds.is_some()
                || self.first_incumbent_milliseconds.is_some()
                || self.first_verified_feasible_milliseconds.is_some()
                || phase_timings.iter().any(Option::is_some)
                || !self.verification_warnings.is_empty())
        {
            return Err(DomainContractError::InvalidRunTiming);
        }
        if !clock_failed_alarm {
            if interrupted {
                if self.elapsed_milliseconds.is_some()
                    || self.first_incumbent_milliseconds.is_some()
                    || self.first_verified_feasible_milliseconds.is_some()
                    || phase_timings.iter().any(Option::is_some)
                    || !self.verification_warnings.is_empty()
                {
                    return Err(DomainContractError::InvalidRunTiming);
                }
            } else if self.elapsed_milliseconds.is_none() {
                return Err(DomainContractError::InvalidRunTiming);
            }
        }
        let milestone_after_elapsed = self.elapsed_milliseconds.is_some_and(|elapsed| {
            [
                self.first_incumbent_milliseconds,
                self.first_verified_feasible_milliseconds,
            ]
            .into_iter()
            .flatten()
            .any(|timing| timing > elapsed)
        });
        let incumbent_after_verified = matches!(
            (
                self.first_incumbent_milliseconds,
                self.first_verified_feasible_milliseconds,
            ),
            (Some(incumbent), Some(verified)) if incumbent > verified
        );
        let phase_total = phase_timings
            .into_iter()
            .flatten()
            .try_fold(0_u64, |total, timing| total.checked_add(timing.value()));
        let phase_total_after_elapsed = self
            .elapsed_milliseconds
            .is_some_and(|elapsed| phase_total.is_none_or(|total| total > elapsed.value()));
        if !clock_failed_alarm
            && (self.finished_at < self.started_at
                || milestone_after_elapsed
                || incumbent_after_verified
                || phase_total_after_elapsed)
        {
            return Err(DomainContractError::InvalidRunTiming);
        }
        if self.verification_warnings.len() > MAX_VERIFICATION_WARNINGS {
            return Err(DomainContractError::LimitExceeded("verification warnings"));
        }
        if !strictly_sorted_unique_by(&self.verification_warnings, |warning| &warning.id) {
            return Err(DomainContractError::NonCanonicalVerificationWarnings);
        }
        for warning in &self.verification_warnings {
            warning.validate()?;
        }
        let accepted = matches!(self.outcome, RunTerminalOutcomeV1::Accepted { .. });
        if accepted != self.first_verified_feasible_milliseconds.is_some() {
            return Err(DomainContractError::InvalidRunTiming);
        }
        Ok(())
    }
}

/// Version-aware portable wrapper for one terminal independently accepted result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableAcceptedResult {
    /// Legacy wrapper two or current wrapper three.
    pub schema_version: u32,
    /// Stable result identity, equal to the accepted solution identity.
    pub result_id: SolutionId,
    /// Immutable scenario identity exposed for generic portable extractors.
    pub scenario_id: ScenarioId,
    /// Exact scenario revision exposed for generic portable extractors.
    pub scenario_revision: u64,
    /// Terminal immutable run input.
    pub run_input: RunInput,
    /// Terminal run manifest.
    pub run_manifest: RunManifest,
    /// Current accepted solution and independent verification report.
    pub accepted_result: AcceptedResult,
    /// Compact typed evidence keyed by stable evidence identity.
    pub evidence: BTreeMap<DomainEvidenceId, VerificationValue>,
    /// Lowercase BLAKE3 checksum of the complete wrapper excluding this field.
    pub checksum: String,
}

impl PortableAcceptedResult {
    /// Creates a portable wrapper and derives its exposed identities from the accepted result.
    ///
    /// # Errors
    /// Rejects invalid nested contracts, evidence bounds, or any cross-contract binding mismatch.
    pub fn new(
        run_input: RunInput,
        run_manifest: RunManifest,
        accepted_result: AcceptedResult,
        evidence: BTreeMap<DomainEvidenceId, VerificationValue>,
    ) -> Result<Self, DomainContractError> {
        let mut result = Self {
            schema_version: PORTABLE_ACCEPTED_RESULT_SCHEMA_VERSION,
            result_id: accepted_result.solution.solution_id,
            scenario_id: accepted_result.solution.scenario_id,
            scenario_revision: accepted_result.solution.scenario_revision,
            run_input,
            run_manifest,
            accepted_result,
            evidence,
            checksum: String::new(),
        };
        result.validate_payload()?;
        result.checksum = checksum_without_field(&result, "checksum")?;
        ensure_domain_contract_size(&result)?;
        Ok(result)
    }

    /// Strictly parses and validates the structure and integrity of a supported portable accepted result.
    ///
    /// This check never grants local acceptance authority; callers must independently establish
    /// that the restored scenario and local trust boundary permit accepting the retained result.
    ///
    /// # Errors
    /// Rejects unsafe or malformed JSON and every nested or cross-contract invariant violation.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_portable_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates nested contracts, evidence, and every immutable binding.
    ///
    /// # Errors
    /// Returns the first violated portable-result invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        if !matches!(self.schema_version, 2 | 3) {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        if self.schema_version == 2
            && (self.run_input.schema_version != 1 || self.run_manifest.schema_version != 1)
        {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "legacy nested versions",
            ));
        }
        validate_revision(self.scenario_revision)?;
        self.run_input.validate()?;
        self.run_manifest.validate()?;
        self.accepted_result.validate()?;
        validate_result_evidence(&self.accepted_result, &self.evidence)?;
        self.validate_bindings()?;
        self.validate_accepted_outcome()
    }

    fn validate_bindings(&self) -> Result<(), DomainContractError> {
        if self.result_id != self.accepted_result.solution.solution_id {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "result ID",
            ));
        }
        if self.scenario_id != self.run_input.scenario_id
            || self.scenario_id != self.accepted_result.solution.scenario_id
        {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "scenario ID",
            ));
        }
        if self.scenario_revision != self.run_input.scenario_revision
            || self.scenario_revision != self.accepted_result.solution.scenario_revision
        {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "scenario revision",
            ));
        }
        if self.run_manifest.run_id != self.run_input.run_id {
            return Err(DomainContractError::PortableResultBindingMismatch("run ID"));
        }
        if self.run_manifest.run_input_checksum != self.run_input.checksum {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "run-input checksum",
            ));
        }
        if self.run_input.snapshot_created_at > self.run_manifest.started_at {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "snapshot creation time",
            ));
        }
        if self.run_manifest.verification_warnings != self.accepted_result.verification.warnings {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "verification warnings",
            ));
        }
        if self.run_input.pack_id != self.accepted_result.solution.pack_id {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "pack ID",
            ));
        }
        if self.run_input.snapshot_document_hash != self.accepted_result.verification.document_hash
        {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "snapshot document hash",
            ));
        }
        if self.run_input.model_hash != self.accepted_result.verification.planning_model_hash {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "planning model hash",
            ));
        }
        if let Some(observations) = &self.run_manifest.native_observations {
            observations.validate_input_bindings(&self.run_input)?;
        }
        Ok(())
    }

    fn validate_accepted_outcome(&self) -> Result<(), DomainContractError> {
        match &self.run_manifest.outcome {
            RunTerminalOutcomeV1::Accepted {
                solution_id,
                accepted_result_checksum,
                verification_checksum,
                ..
            } if *solution_id == self.accepted_result.solution.solution_id
                && accepted_result_checksum == &self.accepted_result.checksum
                && verification_checksum == &self.accepted_result.verification.checksum =>
            {
                Ok(())
            }
            RunTerminalOutcomeV1::Accepted { .. } => Err(
                DomainContractError::PortableResultBindingMismatch("accepted outcome"),
            ),
            RunTerminalOutcomeV1::NoResult { .. }
            | RunTerminalOutcomeV1::VerificationAlarm { .. }
            | RunTerminalOutcomeV1::Interrupted => Err(
                DomainContractError::PortableResultBindingMismatch("terminal outcome"),
            ),
        }
    }
}

fn validate_result_evidence(
    accepted_result: &AcceptedResult,
    evidence: &BTreeMap<DomainEvidenceId, VerificationValue>,
) -> Result<(), DomainContractError> {
    if evidence.len() > MAX_PORTABLE_RESULT_EVIDENCE_RECORDS {
        return Err(DomainContractError::LimitExceeded(
            "portable result evidence",
        ));
    }
    for value in evidence.values() {
        value.validate()?;
    }
    let referenced_evidence: BTreeSet<&DomainEvidenceId> = accepted_result
        .solution
        .assignments
        .iter()
        .flat_map(|assignment| &assignment.evidence)
        .chain(
            accepted_result
                .verification
                .required_rule_results
                .iter()
                .flat_map(|evaluation| &evaluation.evidence),
        )
        .collect();
    if evidence
        .keys()
        .any(|evidence_id| !referenced_evidence.contains(evidence_id))
    {
        return Err(DomainContractError::PortableResultBindingMismatch(
            "evidence reference",
        ));
    }
    Ok(())
}

/// Independently accepted first candidate before its original solve has terminated.
///
/// This local authority is not a terminal run manifest or a portable accepted result. Its
/// immutable input, exact domain result, and first-verification milestone survive a later
/// backend failure or interrupted run without inventing proof.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedCheckpoint {
    pub schema_version: u32,
    pub run_input: RunInput,
    pub observed_after_milliseconds: DurationMillis,
    pub verified_after_milliseconds: DurationMillis,
    pub accepted_at: Rfc3339Timestamp,
    pub accepted_result: AcceptedResult,
    pub evidence: BTreeMap<DomainEvidenceId, VerificationValue>,
    pub checksum: String,
}

impl VerifiedCheckpoint {
    /// Binds one first verified candidate to its immutable, still-running input.
    ///
    /// # Errors
    /// Rejects a nonaccepted result, extraneous evidence, mismatched immutable bindings,
    /// regressing timing, or an excessive checkpoint.
    pub fn new(
        run_input: RunInput,
        observed_after_milliseconds: DurationMillis,
        verified_after_milliseconds: DurationMillis,
        accepted_at: Rfc3339Timestamp,
        accepted_result: AcceptedResult,
        evidence: BTreeMap<DomainEvidenceId, VerificationValue>,
    ) -> Result<Self, DomainContractError> {
        let mut checkpoint = Self {
            schema_version: VERIFIED_CHECKPOINT_SCHEMA_VERSION,
            run_input,
            observed_after_milliseconds,
            verified_after_milliseconds,
            accepted_at,
            accepted_result,
            evidence,
            checksum: String::new(),
        };
        checkpoint.validate_payload()?;
        checkpoint.checksum = checksum_without_field(&checkpoint, "checksum")?;
        ensure_domain_contract_size(&checkpoint)?;
        Ok(checkpoint)
    }

    /// Parses and verifies one bounded local checkpoint record.
    ///
    /// # Errors
    /// Rejects malformed, oversized, newer-version or mismatched authority.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let checkpoint: Self = parse_portable_domain_json(bytes)?;
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    /// Verifies the checkpoint and all nested checksums without claiming backend proof.
    ///
    /// # Errors
    /// Rejects an invalid version, binding, evidence, timestamp or checksum.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        self.validate_payload()?;
        validate_hash(&self.checksum)?;
        if checksum_without_field(self, "checksum")? != self.checksum {
            return Err(DomainContractError::ChecksumMismatch);
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainContractError> {
        if !matches!(self.schema_version, 1 | 2) {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        if self.schema_version == 1 && self.run_input.schema_version != 1 {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "legacy checkpoint input",
            ));
        }
        self.run_input.validate()?;
        self.accepted_result.validate()?;
        validate_result_evidence(&self.accepted_result, &self.evidence)?;
        if self.observed_after_milliseconds > self.verified_after_milliseconds
            || self.verified_after_milliseconds
                > self.run_input.solve_options.time_limit_milliseconds
            || self.accepted_at < self.run_input.snapshot_created_at
        {
            return Err(DomainContractError::InvalidRunTiming);
        }
        if self.accepted_result.solution.scenario_id != self.run_input.scenario_id
            || self.accepted_result.solution.scenario_revision != self.run_input.scenario_revision
            || self.accepted_result.solution.pack_id != self.run_input.pack_id
            || self.accepted_result.verification.document_hash
                != self.run_input.snapshot_document_hash
            || self.accepted_result.verification.planning_model_hash != self.run_input.model_hash
        {
            return Err(DomainContractError::PortableResultBindingMismatch(
                "verified checkpoint input",
            ));
        }
        Ok(())
    }
}

/// Inert V1 verification severity retained only for legacy decoding.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LegacyVerificationSeverityV1 {
    /// Legacy acceptance-blocking issue.
    Error,
    /// Legacy non-blocking warning.
    Warning,
}

/// Inert exact V1 issue shape retained only for legacy decoding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyVerificationIssueV1 {
    /// Legacy issue identity.
    pub id: VerificationIssueId,
    /// Legacy severity.
    pub severity: LegacyVerificationSeverityV1,
    /// Legacy message key.
    pub message_key: String,
    /// Legacy related entities.
    pub entities: Vec<DomainEntityRef>,
    /// Legacy evidence references.
    pub evidence: Vec<DomainEvidenceId>,
}

/// Inert exact V1 report shape retained only for legacy decoding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyVerificationReportV1 {
    /// Legacy schema version.
    pub schema_version: u32,
    /// Legacy scenario revision.
    pub scenario_revision: u64,
    /// Legacy feasibility flag.
    pub feasible: bool,
    /// Legacy issues.
    pub issues: Vec<LegacyVerificationIssueV1>,
    /// Legacy optional score.
    pub score: Option<ScoreVector>,
}

/// Inert exact V1 accepted-result shape retained only for legacy decoding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyAcceptedResultV1 {
    /// Legacy schema version.
    pub schema_version: u32,
    /// Legacy projected candidate.
    pub solution: NormalizedSolution,
    /// Legacy verification report.
    pub verification: LegacyVerificationReportV1,
}

impl LegacyVerificationReportV1 {
    /// Strictly parses and validates the inert V1 report shape.
    ///
    /// # Errors
    /// Rejects malformed JSON, any version other than V1, noncanonical issues, or inconsistent
    /// legacy feasibility/score data.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates the released V1 report contract without granting current acceptance authority.
    ///
    /// # Errors
    /// Returns the first violated legacy invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.schema_version != 1 {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        if !strictly_sorted_unique_by(&self.issues, |issue| &issue.id) {
            return Err(DomainContractError::NonCanonicalRuleEvaluations);
        }
        let has_error = self
            .issues
            .iter()
            .any(|issue| issue.severity == LegacyVerificationSeverityV1::Error);
        if self.feasible == has_error {
            return Err(DomainContractError::InconsistentVerification);
        }
        if let Some(score) = &self.score {
            score.validate_shape()?;
            if self.feasible != (score.feasibility == 0) {
                return Err(DomainContractError::InconsistentVerification);
            }
        }
        Ok(())
    }
}

impl LegacyAcceptedResultV1 {
    /// Strictly parses and validates the inert V1 accepted-result shape.
    ///
    /// # Errors
    /// Rejects malformed JSON, any version other than V1, or invalid legacy bindings.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DomainContractError> {
        let value: Self = parse_domain_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates released V1 data without converting it to a current accepted result.
    ///
    /// # Errors
    /// Returns the first violated legacy invariant.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.schema_version != 1 {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        self.solution.validate()?;
        self.verification.validate()?;
        if self.solution.scenario_revision != self.verification.scenario_revision
            || !self.verification.feasible
            || self
                .verification
                .score
                .as_ref()
                .is_none_or(|score| score.feasibility != 0)
        {
            return Err(DomainContractError::NotVerifiedFeasible);
        }
        Ok(())
    }
}

/// Domain result contract validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomainContractError {
    /// Serialized schema version is unsupported.
    UnsupportedVersion(u32),
    /// JSON cannot be parsed strictly.
    MalformedJson(String),
    /// Canonical JSON serialization unexpectedly failed.
    CanonicalSerialization(String),
    /// Present interval is incoherent or overflows.
    InvalidInterval,
    /// Assignments are not strictly ID-sorted.
    NonCanonicalAssignments,
    /// Duplicate assignment identity.
    DuplicateAssignment(DomainAssignmentId),
    /// Evidence references are not strictly sorted and unique.
    NonCanonicalEvidence,
    /// Score feasibility is negative.
    NegativeFeasibility,
    /// Score level identity occurs more than once.
    DuplicateScoreLevel,
    /// Compared score vectors do not have identical ordered identities/directions.
    ScoreShapeMismatch,
    /// A BLAKE3 hash is not exactly 64 lowercase hexadecimal characters.
    InvalidBlake3,
    /// A deterministic checksum does not match the canonical payload.
    ChecksumMismatch,
    /// Portable JSON violates the shared nonsecret or structural ingress policy.
    PortableJsonViolation(String),
    /// Canonical solve options contain a zero optional limit or exact worker count.
    InvalidSolveOptions,
    /// An explicitly selected solve-option backend differs from the retained backend identity.
    RunInputBackendMismatch,
    /// Recorded objective context violates its version, profile, or baseline binding.
    InvalidObjectiveContext(&'static str),
    /// Compact native observations violate their closed shape or exact input binding.
    InvalidNativeObservations(&'static str),
    /// The retained request hash does not match its canonical semantic preimage.
    RequestHashMismatch,
    /// A persisted component version is empty, unsafe, or exceeds its byte bound.
    InvalidRunVersion(&'static str),
    /// A required schema or protocol major version is zero.
    ZeroVersion(&'static str),
    /// A retained verification-alarm code is not a safe bounded lowercase dotted code.
    InvalidRunDiagnosticCode,
    /// A terminal status is not valid for its run-outcome variant.
    InvalidRunOutcomeStatus,
    /// Verified-feasible timing presence disagrees with the terminal outcome.
    InvalidRunTiming,
    /// A portable accepted-result field does not match its nested immutable contract.
    PortableResultBindingMismatch(&'static str),
    /// A configured verification collection bound was exceeded.
    LimitExceeded(&'static str),
    /// A stable localization message key is invalid.
    InvalidMessageKey,
    /// A verification text value is empty or exceeds its byte bound.
    InvalidVerificationText,
    /// A metric ratio has a zero denominator.
    InvalidMetricRatio,
    /// Required rule bindings are not strictly sorted and unique.
    NonCanonicalRequiredRules,
    /// Required rule identity occurs more than once.
    DuplicateRequiredRule(RuleId),
    /// Required-rule evaluations are not strictly sorted and unique.
    NonCanonicalRuleEvaluations,
    /// Required-rule identity occurs more than once in a report.
    DuplicateRuleEvaluation(RuleId),
    /// Affected entities are not strictly sorted and unique.
    NonCanonicalAffectedEntities,
    /// An affected entity occurs more than once.
    DuplicateAffectedEntity,
    /// Verification evidence is not strictly sorted and unique.
    NonCanonicalVerificationEvidence,
    /// A verification evidence reference occurs more than once.
    DuplicateVerificationEvidence,
    /// Warnings are not strictly sorted and unique.
    NonCanonicalVerificationWarnings,
    /// Warning identity occurs more than once.
    DuplicateVerificationWarning,
    /// Acceptance does not match complete required-rule satisfaction.
    InconsistentVerification,
    /// The report does not bind to the exact scenario, revision, and normalized solution.
    VerificationBindingMismatch,
    /// Verification did not establish acceptance with zero feasibility.
    NotVerifiedFeasible,
    /// A versioned explanation or counterfactual record violates its semantic contract.
    InvalidExplanationContract(&'static str),
    /// A bounded explanation collection is not in canonical strict order.
    NonCanonicalExplanationCollection(&'static str),
}

impl fmt::Display for DomainContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "domain IR contract error: {self:?}")
    }
}

impl std::error::Error for DomainContractError {}
