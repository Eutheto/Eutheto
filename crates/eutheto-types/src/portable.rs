//! Shared portable-data safety, identity, asset, and reference contracts.

use crate::{PackId, Revision, Rfc3339Timestamp, ScenarioDocument, ScenarioFormat, ScenarioId};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use uuid::Uuid;
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticCapability {
    pub id: String,
    pub version: u32,
}

/// Versioned pack-owned portable payload.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableDomainDocument {
    pub pack_id: PackId,
    pub schema_version: u32,
    pub required_capabilities: BTreeSet<SemanticCapability>,
    pub payload: Value,
}

/// Frozen persisted snapshot representation, independent of the portable wire schema.
pub const SCENARIO_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Optional project-wrapper metadata present only in full-library backups.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableProjectMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<Rfc3339Timestamp>,
}

/// Byte-compatible internal snapshot retained by persistence and import staging.
///
/// This frozen representation also decodes genuine global portable-v1 input. It is not the
/// current portable wire envelope, and its marker must not follow portable schema upgrades.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioSnapshotV1 {
    pub format: ScenarioFormat,
    pub schema_version: u32,
    pub revision: Revision,
    pub document: ScenarioDocument,
    /// Wrapper metadata supplied for library backup and restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PortableProjectMetadata>,
    /// Semantic capabilities required to interpret this scenario.
    #[serde(default)]
    pub required_capabilities: BTreeSet<SemanticCapability>,
    /// Known semantic payloads. Each key must have a required capability.
    #[serde(default)]
    pub semantic_extensions: BTreeMap<String, Value>,
    /// Namespaced nonsemantic values preserved without interpretation.
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
}

impl ScenarioSnapshotV1 {
    #[must_use]
    pub fn current(
        revision: Revision,
        document: ScenarioDocument,
        required_capabilities: BTreeSet<SemanticCapability>,
    ) -> Self {
        Self {
            format: ScenarioFormat::EuthetoScenario,
            schema_version: SCENARIO_SNAPSHOT_SCHEMA_VERSION,
            revision,
            document,
            required_capabilities,
            project: None,
            semantic_extensions: BTreeMap::new(),
            extensions: BTreeMap::new(),
        }
    }
}

/// Decodes record maps without silently replacing equivalent typed identities.
///
/// Distinct JSON keys can decode to the same UUID, for example through letter-case aliases.
/// Reject duplicates before reading their values rather than accepting `BTreeMap`'s overwrite.
///
/// # Errors
///
/// Returns a deserialization error for invalid keys/values or repeated typed identities.
pub fn deserialize_unique_id_map<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    struct UniqueIdMap<K, V>(std::marker::PhantomData<(K, V)>);

    impl<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>> Visitor<'de> for UniqueIdMap<K, V> {
        type Value = BTreeMap<K, V>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("records with unique typed identities")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut records = BTreeMap::new();
            while let Some(id) = map.next_key::<K>()? {
                let std::collections::btree_map::Entry::Vacant(entry) = records.entry(id) else {
                    return Err(de::Error::custom("duplicate typed record identity"));
                };
                entry.insert(map.next_value::<V>()?);
            }
            Ok(records)
        }
    }

    deserializer.deserialize_map(UniqueIdMap::<K, V>(std::marker::PhantomData))
}

/// Whether a portable capability or nonsemantic extension has the established namespace shape.
#[must_use]
pub fn is_portable_namespace(namespace: &str) -> bool {
    namespace.contains('.')
        && namespace.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

/// Limits applied by the shared recursive JSON safety policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortableJsonLimits {
    pub max_depth: usize,
    pub max_string_bytes: usize,
    pub max_collection_items: usize,
}

/// A nonsecret/nonportable JSON policy violation safe to surface to callers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortableJsonViolation(pub String);

impl fmt::Display for PortableJsonViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PortableJsonViolation {}

/// Applies the one recursive nonsecret/nonportable JSON policy shared by
/// command application, import, and export.
///
/// Credential-bearing field matching tokenizes camel, snake, kebab, and
/// namespaced boundaries, while an explicit narrow allowlist preserves known
/// nonsecret identifiers without generic substring matching.
///
/// # Errors
///
/// Returns a violation for excessive nesting/size, credential-bearing fields,
/// host paths and database/device locations, or executable/template content.
pub fn validate_nonsecret_portable_json(
    value: &Value,
    limits: &PortableJsonLimits,
) -> Result<(), PortableJsonViolation> {
    validate_json_at(
        value,
        &mut StreamingJsonBudget {
            limits: *limits,
            nodes: 0,
            items: 0,
            string_bytes: 0,
        },
        0,
    )
}

struct StreamingJsonBudget {
    limits: PortableJsonLimits,
    nodes: usize,
    items: usize,
    string_bytes: usize,
}

impl StreamingJsonBudget {
    fn node<E: de::Error>(&mut self, depth: usize) -> Result<(), E> {
        if depth > self.limits.max_depth {
            return Err(E::custom("portable JSON nesting limit exceeded"));
        }
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or_else(|| E::custom("portable JSON node count overflow"))?;
        if self.nodes > self.limits.max_collection_items {
            return Err(E::custom("portable JSON aggregate node limit exceeded"));
        }
        Ok(())
    }

    fn item<E: de::Error>(&mut self, container_items: usize) -> Result<(), E> {
        if container_items >= self.limits.max_collection_items {
            return Err(E::custom("portable JSON container item limit exceeded"));
        }
        self.items = self
            .items
            .checked_add(1)
            .ok_or_else(|| E::custom("portable JSON item count overflow"))?;
        if self.items > self.limits.max_collection_items {
            return Err(E::custom("portable JSON aggregate item limit exceeded"));
        }
        Ok(())
    }

    fn string<E: de::Error>(&mut self, value: &str, key: bool) -> Result<(), E> {
        if value.len() > self.limits.max_string_bytes {
            return Err(E::custom(if key {
                "portable JSON object key limit exceeded"
            } else {
                "portable JSON string limit exceeded"
            }));
        }
        self.string_bytes = self
            .string_bytes
            .checked_add(value.len())
            .ok_or_else(|| E::custom("portable JSON string byte count overflow"))?;
        Ok(())
    }
}

struct StreamingJsonSeed<'a> {
    budget: &'a mut StreamingJsonBudget,
    depth: usize,
    reject_false: bool,
}

impl<'de> DeserializeSeed<'de> for StreamingJsonSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        self.budget.node::<D::Error>(self.depth)?;
        deserializer.deserialize_any(StreamingJsonVisitor {
            budget: self.budget,
            depth: self.depth,
            reject_false: self.reject_false,
        })
    }
}

struct StreamingJsonVisitor<'a> {
    budget: &'a mut StreamingJsonBudget,
    depth: usize,
    reject_false: bool,
}

impl<'de> Visitor<'de> for StreamingJsonVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded duplicate-free portable JSON")
    }

    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_none<E>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<(), E> {
        if self.reject_false && !value {
            return Err(E::custom(
                "provider-restricted content cannot be serialized",
            ));
        }
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        self.budget.string::<E>(value, false)?;
        validate_string(value, &self.budget.limits).map_err(E::custom)
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<(), E> {
        self.visit_str(&value)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        let depth = self
            .depth
            .checked_add(1)
            .ok_or_else(|| <A::Error as de::Error>::custom("portable JSON nesting overflow"))?;
        let budget = self.budget;
        let mut count = 0_usize;
        while sequence
            .next_element_seed(StreamingJsonSeed {
                budget: &mut *budget,
                depth,
                reject_false: false,
            })?
            .is_some()
        {
            budget.item::<A::Error>(count)?;
            count += 1;
        }
        Ok(())
    }

    fn visit_map<A>(self, mut object: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        let depth = self
            .depth
            .checked_add(1)
            .ok_or_else(|| <A::Error as de::Error>::custom("portable JSON nesting overflow"))?;
        let budget = self.budget;
        let mut keys = BTreeSet::new();
        while let Some(key) = object.next_key::<String>()? {
            budget.string::<A::Error>(&key, true)?;
            if keys.contains(&key) {
                return Err(de::Error::custom("duplicate portable JSON object key"));
            }
            budget.item::<A::Error>(keys.len())?;
            keys.insert(key.clone());
            if is_prohibited_field(&key) {
                return Err(de::Error::custom("prohibited portable JSON field"));
            }
            let reject_false = normalize_field(&key) == "redistributionpermitted";
            object.next_value_seed(StreamingJsonSeed {
                budget: &mut *budget,
                depth,
                reject_false,
            })?;
        }
        Ok(())
    }
}

/// Streams JSON syntax and the shared nonsecret portable policy without
/// constructing a semantic [`Value`] tree.
///
/// Duplicate keys, depth, aggregate and per-container items/nodes, strings,
/// keys, host-specific content, credential fields, and restricted content are
/// rejected while traversing.
///
/// # Errors
///
/// Returns a safe policy violation for malformed or over-budget JSON.
pub fn validate_nonsecret_portable_json_bytes(
    bytes: &[u8],
    limits: &PortableJsonLimits,
) -> Result<(), PortableJsonViolation> {
    let mut budget = StreamingJsonBudget {
        limits: *limits,
        nodes: 0,
        items: 0,
        string_bytes: 0,
    };
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    StreamingJsonSeed {
        budget: &mut budget,
        depth: 0,
        reject_false: false,
    }
    .deserialize(&mut deserializer)
    .map_err(|error| PortableJsonViolation(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| PortableJsonViolation(error.to_string()))
}

fn validate_json_at(
    value: &Value,
    budget: &mut StreamingJsonBudget,
    depth: usize,
) -> Result<(), PortableJsonViolation> {
    budget
        .node::<serde_json::Error>(depth)
        .map_err(|error| PortableJsonViolation(error.to_string()))?;
    match value {
        Value::String(text) => {
            budget
                .string::<serde_json::Error>(text, false)
                .map_err(|error| PortableJsonViolation(error.to_string()))?;
            validate_string(text, &budget.limits)
        }
        Value::Array(values) => {
            for (index, item) in values.iter().enumerate() {
                budget
                    .item::<serde_json::Error>(index)
                    .map_err(|error| PortableJsonViolation(error.to_string()))?;
                validate_json_at(item, budget, depth.saturating_add(1))?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for (index, (key, item)) in values.iter().enumerate() {
                budget
                    .string::<serde_json::Error>(key, true)
                    .map_err(|error| PortableJsonViolation(error.to_string()))?;
                budget
                    .item::<serde_json::Error>(index)
                    .map_err(|error| PortableJsonViolation(error.to_string()))?;
                let normalized = normalize_field(key);
                if is_prohibited_field(key) {
                    return Err(PortableJsonViolation(format!(
                        "prohibited field {key} cannot be serialized"
                    )));
                }
                if normalized == "redistributionpermitted" && item == &Value::Bool(false) {
                    return Err(PortableJsonViolation(
                        "provider-restricted content cannot be serialized".to_owned(),
                    ));
                }
                validate_json_at(item, budget, depth.saturating_add(1))?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

fn validate_string(text: &str, limits: &PortableJsonLimits) -> Result<(), PortableJsonViolation> {
    if text.len() > limits.max_string_bytes {
        return Err(PortableJsonViolation("string limit exceeded".to_owned()));
    }
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let absolute_path = lower.starts_with('/')
        || lower.starts_with("file://")
        || lower.starts_with("\\\\")
        || (bytes.len() > 2 && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/'));
    let database_path = looks_like_database_path(&lower);
    let device_path = is_windows_device_name(&lower) || lower.starts_with("device://");
    if absolute_path || database_path || device_path {
        return Err(PortableJsonViolation(
            "device-specific, database, or local filesystem path is not portable".to_owned(),
        ));
    }
    let template = (lower.contains("{{") && lower.contains("}}"))
        || (lower.contains("{%") && lower.contains("%}"))
        || (lower.contains("<%") && lower.contains("%>"));
    let active = lower.starts_with("#!")
        || lower.contains("<script")
        || lower.contains("javascript:")
        || lower.contains("<svg");
    if template || active || looks_like_private_key_pem(&lower) || looks_like_auth_material(&lower)
    {
        return Err(PortableJsonViolation(
            "executable, credential, or template-like content is not portable".to_owned(),
        ));
    }
    Ok(())
}

fn looks_like_private_key_pem(text: &str) -> bool {
    text.lines().any(|line| {
        let Some(marker) = line.trim().strip_prefix("-----begin ") else {
            return false;
        };
        let Some(label) = marker.strip_suffix("-----") else {
            return false;
        };
        label.trim_end().ends_with("private key")
    })
}

fn looks_like_auth_material(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        let credential = line
            .strip_prefix("authorization:")
            .or_else(|| line.strip_prefix("proxy-authorization:"))
            .map_or(line, str::trim_start);
        ["bearer ", "basic ", "digest ", "aws4-hmac-sha256 "]
            .iter()
            .any(|scheme| {
                credential
                    .strip_prefix(scheme)
                    .is_some_and(|value| !value.trim().is_empty())
            })
            || line.starts_with("cookie:")
            || line.starts_with("set-cookie:")
            || line.starts_with("x-api-key:")
    })
}

fn normalize_field(field: &str) -> String {
    field
        .chars()
        .filter(|character| !matches!(character, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_prohibited_field(field: &str) -> bool {
    let normalized = normalize_field(field);
    if matches!(
        normalized.as_str(),
        "secretary"
            | "tokenizedlabel"
            | "passwordpolicy"
            | "passwordpolicyname"
            | "vaultedceiling"
            | "externalcredentiallabel"
            | "authenticationstatus"
            | "authenticationlabel"
            | "authenticationmethod"
            // Domain recurrence reference, not executable template content.
            | "templateid"
    ) {
        return false;
    }
    let tokens = field_tokens(field);
    if tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "auth"
                | "authentication"
                | "authenticator"
                | "authorization"
                | "bearer"
                | "secret"
                | "secrets"
                | "token"
                | "tokens"
                | "credential"
                | "credentials"
                | "password"
                | "passphrase"
                | "cookie"
                | "keychain"
                | "vault"
                | "keystore"
                | "dsn"
                | "template"
                | "script"
                | "executable"
        )
    }) {
        return true;
    }
    if tokens.windows(2).any(|pair| {
        matches!(
            (pair[0].as_str(), pair[1].as_str()),
            (
                "api" | "access" | "private" | "signing" | "encryption",
                "key"
            ) | ("database", "path" | "file")
                | ("sqlite", "database" | "path")
                | ("connection", "string")
                | ("device", "path")
                | ("command", "line")
        )
    }) {
        return true;
    }
    compact_prohibited_field(&normalized)
}

fn field_tokens(field: &str) -> Vec<String> {
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

fn compact_prohibited_field(normalized: &str) -> bool {
    const BASES: [&str; 36] = [
        "auth",
        "authentication",
        "authenticator",
        "authorization",
        "authorizationheader",
        "bearer",
        "secret",
        "secrets",
        "token",
        "tokens",
        "credential",
        "credentials",
        "password",
        "passphrase",
        "apikey",
        "accesskey",
        "clientsecret",
        "privatekey",
        "privatekeypem",
        "secretkey",
        "signingkey",
        "encryptionkey",
        "cookie",
        "sessioncookie",
        "keychain",
        "vault",
        "keystore",
        "connectionstring",
        "dsn",
        "databasepath",
        "databasefile",
        "sqlitedatabase",
        "sqlitepath",
        "devicepath",
        "executablepath",
        "commandline",
    ];
    const SUFFIXES: [&str; 6] = ["id", "raw", "ref", "reference", "handle", "item"];
    if BASES.iter().any(|base| normalized.ends_with(base)) {
        return true;
    }
    SUFFIXES.iter().any(|suffix| {
        normalized
            .strip_suffix(suffix)
            .is_some_and(|stem| BASES.iter().any(|base| stem.ends_with(base)))
    })
}

fn looks_like_database_path(text: &str) -> bool {
    const DATABASE_SUFFIXES: [&[u8]; 4] = [b".db", b".db3", b".sqlite", b".sqlite3"];
    let candidate = text.split(['?', '#']).next().unwrap_or(text).as_bytes();
    DATABASE_SUFFIXES
        .iter()
        .any(|&suffix| candidate.strip_suffix(suffix).is_some())
}

fn is_windows_device_name(text: &str) -> bool {
    let leaf = text.rsplit(['/', '\\']).next().unwrap_or(text);
    let stem = leaf.split('.').next().unwrap_or(leaf);
    matches!(stem, "con" | "prn" | "aux" | "nul")
        || stem
            .strip_prefix("com")
            .or_else(|| stem.strip_prefix("lpt"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

/// Supplemental portable section namespace.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupplementalSectionKind {
    Results,
    SharedRecords,
    Preferences,
    Assets,
}

impl SupplementalSectionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Results => "results",
            Self::SharedRecords => "shared_records",
            Self::Preferences => "preferences",
            Self::Assets => "assets",
        }
    }
}

impl fmt::Display for SupplementalSectionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stable identity of one supplemental portable record.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupplementalIdentity {
    pub section: SupplementalSectionKind,
    pub key: String,
}

/// Version-1 threshold for the explicit "exclude large assets" backup option.
///
/// This stays below the smallest permitted asset payload ceiling (the
/// one-mebibyte plain-text string limit), so every supported asset kind has a
/// nonempty valid range that the option can exclude.
pub const PORTABLE_LARGE_ASSET_BYTES_V1: usize = 512 * 1024;
/// Inert portable asset plus the exact declaration that authorizes re-export.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableAsset {
    pub bytes: Vec<u8>,
    pub media_type: String,
    pub redistribution_permitted: bool,
}

/// One retained-result dependency on an exact scenario revision.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ScenarioRevisionReference {
    pub scenario_id: ScenarioId,
    pub scenario_revision: Revision,
}

/// Error produced by declared scenario-reference extraction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortableReferenceError(pub String);

impl fmt::Display for PortableReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PortableReferenceError {}

/// Extracts UUIDs from exact `scenarioId`/`scenarioIds` positions recursively.
/// Arbitrary UUID-shaped prose and external values are ignored.
///
/// # Errors
///
/// Returns an error when a declared scenario-reference position is malformed.
pub fn extract_scenario_references(
    value: &Value,
) -> Result<BTreeSet<ScenarioId>, PortableReferenceError> {
    let mut references = BTreeSet::new();
    collect_scenario_references(value, &mut references)?;
    Ok(references)
}

fn collect_scenario_references(
    value: &Value,
    references: &mut BTreeSet<ScenarioId>,
) -> Result<(), PortableReferenceError> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_scenario_references(value, references)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                match normalize_field(key).as_str() {
                    "scenarioid" => {
                        references.insert(parse_scenario_id(value, key)?);
                    }
                    "scenarioids" => {
                        let ids = value.as_array().ok_or_else(|| {
                            PortableReferenceError(format!("declared field {key} must be an array"))
                        })?;
                        for id in ids {
                            references.insert(parse_scenario_id(id, key)?);
                        }
                    }
                    _ => collect_scenario_references(value, references)?,
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

/// Extracts exact declared inert-asset references without inspecting prose.
///
/// # Errors
///
/// Returns an error when a declared scalar/list asset-reference field is malformed.
pub fn extract_asset_references(value: &Value) -> Result<BTreeSet<String>, PortableReferenceError> {
    let mut references = BTreeSet::new();
    collect_asset_references(value, &mut references)?;
    Ok(references)
}

fn collect_asset_references(
    value: &Value,
    references: &mut BTreeSet<String>,
) -> Result<(), PortableReferenceError> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_asset_references(value, references)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                let normalized = normalize_field(key);
                if matches!(
                    normalized.as_str(),
                    "asset" | "assetid" | "assetkey" | "assetpath"
                ) {
                    let reference = value.as_str().ok_or_else(|| {
                        PortableReferenceError(format!(
                            "declared asset reference {key} must be a string"
                        ))
                    })?;
                    references.insert(reference.to_owned());
                } else if matches!(
                    normalized.as_str(),
                    "assets" | "assetids" | "assetkeys" | "assetpaths"
                ) {
                    let items = value.as_array().ok_or_else(|| {
                        PortableReferenceError(format!(
                            "declared asset reference list {key} must be an array"
                        ))
                    })?;
                    for item in items {
                        let reference = item.as_str().ok_or_else(|| {
                            PortableReferenceError(format!(
                                "declared asset reference list {key} must contain strings"
                            ))
                        })?;
                        references.insert(reference.to_owned());
                    }
                } else {
                    collect_asset_references(value, references)?;
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn parse_scenario_id(value: &Value, field: &str) -> Result<ScenarioId, PortableReferenceError> {
    let text = value.as_str().ok_or_else(|| {
        PortableReferenceError(format!("declared field {field} must be a UUID string"))
    })?;
    let id = Uuid::parse_str(text).map_err(|_| {
        PortableReferenceError(format!("declared field {field} must be a UUID string"))
    })?;
    if id.get_version_num() != 7 {
        return Err(PortableReferenceError(format!(
            "declared field {field} must be a UUIDv7 string"
        )));
    }
    Ok(ScenarioId::from_uuid(id))
}

/// Extracts the required UUID `resultId` from a retained-result wrapper.
///
/// # Errors
///
/// Returns an error when the wrapper is not an object or `resultId` is
/// missing, ambiguous, malformed, or not a `UUIDv7`.
pub fn extract_result_id(value: &Value) -> Result<Uuid, PortableReferenceError> {
    let object = value.as_object().ok_or_else(|| {
        PortableReferenceError("retained result must be a JSON object".to_owned())
    })?;
    let (key, value) = unique_normalized_field(object, "resultid", "resultId")?;
    let text = value
        .as_str()
        .ok_or_else(|| PortableReferenceError(format!("retained result {key} must be a string")))?;
    let id = Uuid::parse_str(text)
        .map_err(|_| PortableReferenceError(format!("retained result {key} must be a UUID")))?;
    if id.get_version_num() != 7 {
        return Err(PortableReferenceError(format!(
            "retained result {key} must be a UUIDv7"
        )));
    }
    Ok(id)
}

/// Extracts the required exact `(scenarioId, scenarioRevision)` dependency from
/// a retained-result wrapper. Result-domain payload remains otherwise opaque.
///
/// # Errors
///
/// Returns an error when the wrapper is not an object or either required field
/// is missing or malformed.
pub fn extract_result_dependency(
    value: &Value,
) -> Result<ScenarioRevisionReference, PortableReferenceError> {
    let object = value.as_object().ok_or_else(|| {
        PortableReferenceError("retained result must be a JSON object".to_owned())
    })?;
    let (scenario_key, scenario_value) =
        unique_normalized_field(object, "scenarioid", "scenarioId")?;
    let scenario_id = parse_scenario_id(scenario_value, scenario_key)?;
    let (_, revision_value) =
        unique_normalized_field(object, "scenariorevision", "scenarioRevision")?;
    let revision = revision_value.as_u64().ok_or_else(|| {
        PortableReferenceError("retained result scenarioRevision must be an integer".to_owned())
    })?;
    Ok(ScenarioRevisionReference {
        scenario_id,
        scenario_revision: Revision::new(revision),
    })
}

fn unique_normalized_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    normalized: &str,
    display: &str,
) -> Result<(&'a str, &'a Value), PortableReferenceError> {
    let mut matches = object
        .iter()
        .filter(|(key, _)| normalize_field(key) == normalized);
    let (key, value) = matches
        .next()
        .ok_or_else(|| PortableReferenceError(format!("retained result is missing {display}")))?;
    if matches.next().is_some() {
        return Err(PortableReferenceError(format!(
            "retained result contains ambiguous {display} fields"
        )));
    }
    Ok((key.as_str(), value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LIMITS: PortableJsonLimits = PortableJsonLimits {
        max_depth: 16,
        max_string_bytes: 1024,
        max_collection_items: 1024,
    };

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    struct TypedRecord {
        minutes: u32,
    }

    #[derive(Debug, Deserialize)]
    struct TypedRecords {
        #[serde(deserialize_with = "deserialize_unique_id_map")]
        records: BTreeMap<crate::EntityId, TypedRecord>,
    }

    #[test]
    fn unique_id_map_decodes_typed_records_without_overwriting_aliases()
    -> Result<(), Box<dyn std::error::Error>> {
        let lower = "018f47f2-e880-7000-8000-0000000000ab";
        let upper = lower.to_uppercase();
        let input = format!(r#"{{"records":{{"{lower}":{{"minutes":30}}}}}}"#);
        let decoded: TypedRecords = serde_json::from_str(&input)?;
        assert_eq!(
            decoded.records,
            BTreeMap::from([(lower.parse()?, TypedRecord { minutes: 30 })]),
        );
        let duplicate =
            format!(r#"{{"records":{{"{lower}":{{"minutes":30}},"{upper}":{{"minutes":90}}}}}}"#);
        assert!(serde_json::from_str::<TypedRecords>(&duplicate).is_err());
        Ok(())
    }

    #[test]
    fn unique_id_map_rejects_normalized_duplicate_before_decoding_its_value()
    -> Result<(), Box<dyn std::error::Error>> {
        let lower = "018f47f2-e880-7000-8000-0000000000ab";
        let upper = lower.to_uppercase();
        let duplicate_prefix = format!(r#"{{"records":{{"{lower}":{{"minutes":30}},"{upper}":"#);
        let input = format!(r#"{duplicate_prefix}"not a typed record"}}}}"#);
        let error = serde_json::from_str::<TypedRecords>(&input)
            .err()
            .ok_or("a repeated normalized UUID must reject before decoding its value")?;
        assert!(
            error.column() <= duplicate_prefix.len(),
            "the duplicate value must remain unread",
        );
        Ok(())
    }

    #[test]
    fn value_and_streaming_validation_enforce_the_same_aggregate_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        let limits = PortableJsonLimits {
            max_collection_items: 128,
            ..LIMITS
        };
        let boundary = json!([vec![0; 62], vec![0; 63]]);
        validate_nonsecret_portable_json(&boundary, &limits)?;
        validate_nonsecret_portable_json_bytes(&serde_json::to_vec(&boundary)?, &limits)?;
        let excessive = json!([vec![0; 63], vec![0; 63]]);
        assert!(validate_nonsecret_portable_json(&excessive, &limits).is_err());
        assert!(
            validate_nonsecret_portable_json_bytes(&serde_json::to_vec(&excessive)?, &limits)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn exact_credential_fields_are_rejected_without_substring_matching()
    -> Result<(), PortableJsonViolation> {
        for key in [
            "secret",
            "raw_secret",
            "clientSecret",
            "private_key_pem",
            "authorization-header",
            "credentialRef",
            "keychain_ref",
            "vaultReference",
            "token-reference",
            "providerApiKey",
            "providerClientSecret",
            "oauthCredentialId",
            "apiKeyId",
            "provider-api-key-reference",
            "providerSecretHandle",
            "oauth_token_item",
            "api_key_ref",
            "passwordRef",
            "auth",
            "authenticationRef",
            "authenticatorHandle",
            "authorizationHeader",
            "bearer",
            "connectionString",
            "providerDsn",
            "providerAuthentication.value",
        ] {
            let mut object = serde_json::Map::new();
            object.insert(key.to_owned(), Value::String("sentinel".to_owned()));
            let value = Value::Object(object);
            assert!(validate_nonsecret_portable_json(&value, &LIMITS).is_err());
        }
        validate_nonsecret_portable_json(
            &json!({
                "secretary": "safe",
                "tokenizedLabel": "safe",
                "passwordPolicyName": "safe",
                "vaultedCeiling": "safe",
                "externalCredentialLabel": "safe",
                "authenticationStatus": "connected",
                "authenticationLabel": "Provider",
                "authenticationMethod": "browser",
                "grid": "safe",
                "tokenizer": "safe",
                "monkey": "safe"
            }),
            &LIMITS,
        )?;
        for value in [
            json!({"providerAuthentication": {"value": "sentinel"}}),
            json!({"auth": {"bearer": "sentinel"}}),
        ] {
            assert!(validate_nonsecret_portable_json(&value, &LIMITS).is_err());
        }
        Ok(())
    }

    #[test]
    fn domain_template_references_keep_all_content_and_executable_field_checks()
    -> Result<(), Box<dyn std::error::Error>> {
        let reference = json!({"templateId": "018f7b40-a000-7000-8000-000000000006"});
        validate_nonsecret_portable_json(&reference, &LIMITS)?;
        validate_nonsecret_portable_json_bytes(&serde_json::to_vec(&reference)?, &LIMITS)?;
        for prohibited in [
            json!({"template": "executable source"}),
            json!({"scriptId": "018f7b40-a000-7000-8000-000000000006"}),
            json!({"templateId": "{{ executable_expression }}"}),
            json!({"templateId": {"script": "executable source"}}),
        ] {
            assert!(validate_nonsecret_portable_json(&prohibited, &LIMITS).is_err());
            assert!(
                validate_nonsecret_portable_json_bytes(&serde_json::to_vec(&prohibited)?, &LIMITS)
                    .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn credential_shaped_values_are_rejected_independently_of_field_names()
    -> Result<(), PortableJsonViolation> {
        for value in [
            json!({"headers": ["Authorization: Bearer synthetic-bearer-sentinel"]}),
            json!({"headers": ["Proxy-Authorization: Basic c3ludGhldGljOnNlbnRpbmVs"]}),
            json!({"headers": ["Digest username=\"synthetic\", response=\"sentinel\""]}),
            json!({"headers": ["AWS4-HMAC-SHA256 Credential=synthetic/sentinel"]}),
            json!({"headers": ["Cookie: session=synthetic-sentinel"]}),
            json!({"headers": ["Set-Cookie: session=synthetic-sentinel"]}),
            json!({"headers": ["X-API-Key: synthetic-sentinel"]}),
            json!({"pem": "-----BEGIN RSA PRIVATE KEY-----\nsynthetic\n-----END RSA PRIVATE KEY-----"}),
            json!({"pem": "-----BEGIN EC PRIVATE KEY-----\nsynthetic\n-----END EC PRIVATE KEY-----"}),
            json!({"pem": "-----BEGIN OPENSSH PRIVATE KEY-----\nsynthetic\n-----END OPENSSH PRIVATE KEY-----"}),
            json!({"pem": "-----BEGIN ENCRYPTED PRIVATE KEY-----\nsynthetic\n-----END ENCRYPTED PRIVATE KEY-----"}),
        ] {
            assert!(validate_nonsecret_portable_json(&value, &LIMITS).is_err());
        }
        validate_nonsecret_portable_json(
            &json!({
                "note": "The bearer of this certificate may proceed.",
                "certificate": "-----BEGIN CERTIFICATE-----\nsynthetic\n-----END CERTIFICATE-----"
            }),
            &LIMITS,
        )
    }

    #[test]
    fn retained_result_identity_is_required_uuid_v7() -> Result<(), PortableReferenceError> {
        let expected = Uuid::parse_str("018f1e2d-3c4b-7a69-8def-0123456789ab")
            .map_err(|error| PortableReferenceError(error.to_string()))?;
        assert_eq!(extract_result_id(&json!({"resultId": expected}))?, expected);
        assert!(extract_result_id(&json!({})).is_err());
        assert!(extract_result_id(&json!({"resultId": Uuid::nil()})).is_err());
        Ok(())
    }

    #[test]
    fn references_ignore_uuid_shaped_prose() -> Result<(), PortableReferenceError> {
        let owned = "018f1e2d-3c4b-7a69-8def-0123456789ab";
        let external = "018f1e2d-3c4b-7a69-8def-1123456789ab";
        let references = extract_scenario_references(&json!({
            "scenarioId": owned,
            "note": external,
            "externalUuid": external
        }))?;
        assert_eq!(references.len(), 1);
        assert_eq!(
            references.iter().next().map(ToString::to_string),
            Some(owned.to_owned())
        );
        Ok(())
    }
}
