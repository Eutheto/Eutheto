use anyhow::{Context, Result, bail};
use eutheto_domain_api::{
    CommandDescriptor, ContractJsonLimits, DomainUiManifest, SetupQueryDescriptor,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

const PACK_SOURCE: &str = include_str!("../../schemas/domain-packs/official-test.contract.json");
const WORKFORCE_SOURCE: &str = include_str!("../../schemas/domain-packs/workforce.contract.json");
const MATRIX_SOURCE: &str = include_str!("../../schemas/solver-support-matrix.json");

const GENERATED_TYPESCRIPT_PACK: &str = "apps/desktop/src/api/generated-domain-pack-contracts.ts";
const GENERATED_GLOBAL_PORTABLE_FIXTURE: &str =
    "tests/migration/fixtures/portable_v2_scenario.json";
const PRETTIER_VERSION: &str = "3.9.6";
const MAX_EXPANDED_SCHEMA_NODES: usize = 100_000;
const MAX_PACK_CONTRACT_BYTES: usize = 16 * 1024 * 1024;

struct PackSource {
    path: &'static str,
    contents: &'static str,
    id: &'static str,
    rust_prefix: &'static str,
    typescript_prefix: &'static str,
    rust: &'static str,
    command_schemas: &'static str,
    query_schemas: &'static str,
    internal_schema: &'static str,
    portable_schema: &'static str,
    share_schema: Option<&'static str>,
    ai_tools: &'static str,
    ui_manifest: &'static str,
    docs: &'static str,
}

const PACK_SOURCES: [PackSource; 2] = [
    PackSource {
        path: "schemas/domain-packs/official-test.contract.json",
        contents: PACK_SOURCE,
        id: "official.test",
        rust_prefix: "OFFICIAL_TEST",
        typescript_prefix: "OfficialTest",
        rust: "crates/eutheto-command/src/generated_official_test_pack_contract.rs",
        command_schemas: "schemas/generated/official-test.command-schemas.json",
        query_schemas: "schemas/generated/official-test.setup-query-schemas.json",
        internal_schema: "schemas/generated/official-test.internal.schema.json",
        portable_schema: "schemas/generated/official-test.portable.schema.json",
        share_schema: Some("schemas/generated/official-test.share-result.schema.json"),
        ai_tools: "xtask/generated/official-test-ai-tools.json",
        ui_manifest: "xtask/generated/official-test-ui-manifest.json",
        docs: "docs/generated/official-test-pack-contract.md",
    },
    PackSource {
        path: "schemas/domain-packs/workforce.contract.json",
        contents: WORKFORCE_SOURCE,
        id: "official.workforce",
        rust_prefix: "WORKFORCE",
        typescript_prefix: "Workforce",
        rust: "domains/workforce/core/src/generated_workforce_pack_contract.rs",
        command_schemas: "schemas/generated/workforce.command-schemas.json",
        query_schemas: "schemas/generated/workforce.setup-query-schemas.json",
        internal_schema: "schemas/generated/workforce.internal.schema.json",
        portable_schema: "schemas/generated/workforce.portable.schema.json",
        share_schema: Some("schemas/generated/workforce.share-result.schema.json"),
        ai_tools: "xtask/generated/workforce-ai-tools.json",
        ui_manifest: "xtask/generated/workforce-ui-manifest.json",
        docs: "docs/generated/workforce-pack-contract.md",
    },
];
const GENERATED_RUST_MATRIX: &str = "crates/eutheto-solver-api/src/generated_support_matrix.rs";
const GENERATED_MATRIX_DOCS: &str = "docs/generated/solver-support-matrix.md";

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackContract {
    schema_version: u32,
    pack: PackDescriptor,
    #[serde(rename = "$defs", default, skip_serializing)]
    definitions: BTreeMap<String, Value>,
    commands: Vec<CommandDescriptor>,
    setup_queries: Vec<SetupQueryDescriptor>,
    internal_schema: Value,
    portable_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    share_result_schema: Option<Value>,
    ai_tools: Vec<SourceAiTool>,
    ui_manifest: DomainUiManifest,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackDescriptor {
    id: String,
    pack_version: String,
    latest_schema_version: u32,
    portable_schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    share_result_schema_version: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceAiTool {
    command_id: String,
    name: String,
    description: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SupportMatrix {
    schema_version: u32,
    planning_ir_schema_version: u32,
    features: Vec<SupportFeature>,
    registered_backends: Vec<RegisteredBackend>,
    deferred_candidate_gates: Vec<DeferredCandidateGate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SupportFeature {
    id: String,
    category: String,
    gate: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredBackend {
    id: String,
    version: String,
    adapter_version: String,
    support: BTreeMap<String, SourceSupportCell>,
}

#[derive(Debug, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "support",
    deny_unknown_fields
)]
enum SourceSupportCell {
    Supported {
        fixture_id: String,
    },
    Degraded {
        restriction_id: String,
        reason: String,
        remediation: String,
        fixture_id: String,
    },
    Unsupported {
        reason: String,
        remediation: String,
        fixture_id: String,
    },
}

impl SourceSupportCell {
    fn is_complete(&self) -> bool {
        match self {
            Self::Supported { fixture_id } => !fixture_id.is_empty(),
            Self::Degraded {
                restriction_id,
                reason,
                remediation,
                fixture_id,
            } => {
                !restriction_id.is_empty()
                    && !reason.is_empty()
                    && !remediation.is_empty()
                    && !fixture_id.is_empty()
            }
            Self::Unsupported {
                reason,
                remediation,
                fixture_id,
            } => !reason.is_empty() && !remediation.is_empty() && !fixture_id.is_empty(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeferredCandidateGate {
    backend_id: String,
    candidate_version: String,
    claim_status: String,
    owning_phase: u32,
}

pub fn generated_files(repo_root: &Path) -> Result<Vec<(&'static str, String)>> {
    let matrix = parse_matrix()?;
    validate_matrix(&matrix)?;
    let mut files = Vec::new();
    let mut typescript = String::from(
        "// SPDX-License-Identifier: Apache-2.0\n// @generated by `cargo xtask generate`; do not edit.\n",
    );
    for source in &PACK_SOURCES {
        let mut pack = parse_pack(source)?;
        let types = render_typescript_types(&pack, source)?;
        expand_pack_schemas(&mut pack)?;
        validate_pack(&pack, source)?;
        files.extend([
            (source.rust, render_rust_pack(&pack, source)?),
            (source.command_schemas, render_command_schemas(&pack)?),
            (source.query_schemas, render_query_schemas(&pack)?),
            (
                source.internal_schema,
                render_schema(&pack.internal_schema, source.internal_schema)?,
            ),
            (
                source.portable_schema,
                render_schema(&pack.portable_schema, source.portable_schema)?,
            ),
            (source.ai_tools, render_ai_tools(&pack, source)?),
            (source.ui_manifest, render_ui_manifest(&pack, source)?),
            (source.docs, render_pack_docs(&pack, source)?),
        ]);
        if let (Some(path), Some(schema)) = (source.share_schema, &pack.share_result_schema) {
            files.push((path, render_schema(schema, path)?));
        }
        if source.id == "official.test" {
            files.push((
                GENERATED_GLOBAL_PORTABLE_FIXTURE,
                render_global_portable_fixture(&pack)?,
            ));
        }
        typescript.push_str(&types);
        typescript.push_str(&render_typescript_constants(&pack, source)?);
    }
    files.extend([
        (
            GENERATED_TYPESCRIPT_PACK,
            format_typescript(
                repo_root,
                "src/api/generated-domain-pack-contracts.ts",
                &typescript,
            )?,
        ),
        (GENERATED_RUST_MATRIX, render_rust_matrix(&matrix)),
        (GENERATED_MATRIX_DOCS, render_matrix_docs(&matrix)?),
    ]);
    Ok(files)
}

/// Returns obsolete products only within this generator's declared filename prefixes.
pub(crate) fn unexpected_files(
    repo_root: &Path,
    expected: &[crate::protocol_generate::GeneratedOutput],
) -> Result<Vec<String>> {
    const OWNED_PREFIXES: &[(&str, &[&str])] = &[
        ("schemas/generated", &["official-test.", "workforce."]),
        ("xtask/generated", &["official-test-", "workforce-"]),
        (
            "docs/generated",
            &["official-test-pack-contract", "workforce-pack-contract"],
        ),
        (
            "crates/eutheto-command/src",
            &["generated_official_test_pack_contract"],
        ),
        (
            "domains/workforce/core/src",
            &["generated_workforce_pack_contract"],
        ),
        ("apps/desktop/src/api", &["generated-domain-pack-contracts"]),
    ];
    let expected = expected
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<BTreeSet<_>>();
    let mut unexpected = Vec::new();
    for (directory, prefixes) in OWNED_PREFIXES {
        let entries = match std::fs::read_dir(repo_root.join(directory)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("failed to inspect pack generated inventory"),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !prefixes.iter().any(|prefix| name.starts_with(prefix)) {
                continue;
            }
            if !entry.file_type()?.is_file() {
                bail!("pack generated inventory cannot contain matching nonregular entries")
            }
            let relative = format!("{directory}/{name}");
            if !expected.contains(relative.as_str()) {
                unexpected.push(relative);
            }
        }
    }
    unexpected.sort();
    Ok(unexpected)
}

fn render_global_portable_fixture(pack: &PackContract) -> Result<String> {
    let legacy: Value = serde_json::from_str(include_str!(
        "../../tests/migration/fixtures/portable_v1_scenario.json"
    ))?;
    let snapshot: eutheto_types::ScenarioSnapshotV1 = serde_json::from_value(
        legacy
            .get("input")
            .context("legacy portable fixture has no input")?
            .clone(),
    )?;
    let domain = eutheto_types::PortableDomainDocument {
        pack_id: snapshot.document.domain_pack.id.clone(),
        schema_version: pack.pack.portable_schema_version,
        payload: json!({
            "schemaVersion": pack.pack.portable_schema_version,
            "entities": snapshot.document.domain.entities,
            "rules": snapshot.document.domain.rules,
            "preferences": snapshot.document.domain.preferences,
            "lockedAssignments": snapshot.document.domain.locked_assignments,
            "extensions": snapshot.document.extensions,
        }),
        required_capabilities: [eutheto_types::SemanticCapability {
            id: "official.test.portable".to_owned(),
            version: 2,
        }]
        .into_iter()
        .collect(),
    };
    let wire = eutheto_export::PortableScenario::from_snapshot(&snapshot, domain)?;
    let requirements = wire
        .required_capabilities
        .union(&wire.domain.required_capabilities)
        .collect::<Vec<_>>();
    let value = json!({
        "$comment": "@generated by cargo xtask generate; do not edit by hand.",
        "format": "eutheto.test/portable-scenario-fixture",
        "schemaVersion": 1,
        "version": 1,
        "id": "018f47f2-e880-7000-8000-000000000102",
        "purpose": "Preserve the genuine v1 fixture's meaning through the current pack-owned wire.",
        "provenance": {"kind": "deterministic-synthetic", "license": "Apache-2.0"},
        "input": wire,
        "bundleDeclaration": {
            "requiredCapabilities": requirements,
            "nonsemanticExtensions": snapshot.document.extensions.keys().collect::<Vec<_>>(),
        },
        "expectedOutcome": {
            "status": "accepted",
            "portableSchemaVersion": eutheto_export::CURRENT_PORTABLE_SCHEMA_VERSION,
            "revision": snapshot.revision,
            "preservedNonsemanticExtensions": snapshot.document.extensions.keys().collect::<Vec<_>>(),
            "authoritativeMutationDuringInspection": false,
        },
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn parse_pack(source: &PackSource) -> Result<PackContract> {
    if source.contents.len() > MAX_PACK_CONTRACT_BYTES {
        bail!("pack contract source exceeds its byte limit")
    }
    serde_json::from_str(source.contents)
        .with_context(|| format!("invalid {} contract source", source.id))
}

fn expand_pack_schemas(pack: &mut PackContract) -> Result<()> {
    for name in pack.definitions.keys() {
        if !valid_identifier(name) {
            bail!("invalid local schema definition name")
        }
    }
    let mut expander = SchemaExpander {
        definitions: &pack.definitions,
        stack: Vec::new(),
        used: BTreeSet::new(),
        nodes: 0,
        remaining_bytes: MAX_PACK_CONTRACT_BYTES,
    };
    pack.internal_schema = expander.expand(&pack.internal_schema, 0)?;
    pack.portable_schema = expander.expand(&pack.portable_schema, 0)?;
    if let Some(schema) = &mut pack.share_result_schema {
        *schema = expander.expand(schema, 0)?;
    }
    for command in &mut pack.commands {
        command.payload_schema = expander.expand(&command.payload_schema, 0)?;
        command.result_schema = expander.expand(&command.result_schema, 0)?;
        command.change_schema = expander.expand(&command.change_schema, 0)?;
    }
    for query in &mut pack.setup_queries {
        query.parameter_schema = expander.expand(&query.parameter_schema, 0)?;
        query.result_schema = expander.expand(&query.result_schema, 0)?;
    }
    if expander.used.len() != pack.definitions.len() {
        bail!("local schema definitions must be reachable from emitted schemas")
    }
    eutheto_domain_api::bounded_json_size(pack, MAX_PACK_CONTRACT_BYTES)
        .context("expanded pack contract exceeds its byte limit")?;
    Ok(())
}

struct SchemaExpander<'a> {
    definitions: &'a BTreeMap<String, Value>,
    stack: Vec<&'a str>,
    used: BTreeSet<&'a str>,
    nodes: usize,
    remaining_bytes: usize,
}

impl SchemaExpander<'_> {
    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(bytes)
            .context("schema expansion exceeds its cumulative byte limit")?;
        Ok(())
    }

    fn charge_json(&mut self, value: &impl Serialize) -> Result<()> {
        let bytes = eutheto_domain_api::bounded_json_size(value, self.remaining_bytes)
            .context("schema expansion exceeds its cumulative byte limit")?;
        self.charge(bytes)
    }

    fn expand(&mut self, schema: &Value, depth: usize) -> Result<Value> {
        if depth > 32 || self.stack.len() > 32 || self.nodes == MAX_EXPANDED_SCHEMA_NODES {
            bail!("schema expansion exceeds its depth or cumulative node limit")
        }
        self.nodes += 1;
        let object = schema.as_object().context("schema must be an object")?;
        if let Some(reference) = object.get("$ref") {
            if object.len() != 1 {
                bail!("local schema references cannot have siblings")
            }
            let name = reference
                .as_str()
                .and_then(|value| value.strip_prefix("#/$defs/"))
                .filter(|name| valid_identifier(name))
                .context("only exact #/$defs/Name references are supported")?;
            let definitions = self.definitions;
            let (name, target) = definitions
                .get_key_value(name)
                .context("local schema reference is unresolved")?;
            if self.stack.contains(&name.as_str()) {
                bail!("local schema references contain a cycle")
            }
            self.used.insert(name);
            self.stack.push(name);
            let expanded = self.expand(target, depth);
            self.stack.pop();
            return expanded;
        }
        self.charge(2)?;
        let mut output = Map::new();
        for (key, value) in object {
            self.charge_json(key)?;
            self.charge(2)?;
            let expanded = match key.as_str() {
                "$defs" => bail!("schema definitions are permitted only at the source root"),
                "properties" => {
                    self.charge(2)?;
                    let properties = value.as_object().context("properties must be an object")?;
                    let mut expanded = Map::new();
                    for (name, child) in properties {
                        self.charge_json(name)?;
                        self.charge(2)?;
                        expanded.insert(name.clone(), self.expand(child, depth + 1)?);
                    }
                    Value::Object(expanded)
                }
                "items" => self.expand(value, depth + 1)?,
                "additionalProperties" if value.is_object() => self.expand(value, depth + 1)?,
                "oneOf" => {
                    self.charge(2)?;
                    let options = value.as_array().context("oneOf must be an array")?;
                    let mut expanded = Vec::with_capacity(options.len());
                    for child in options {
                        self.charge(1)?;
                        expanded.push(self.expand(child, depth + 1)?);
                    }
                    Value::Array(expanded)
                }
                _ => {
                    self.charge_json(value)?;
                    value.clone()
                }
            };
            output.insert(key.clone(), expanded);
        }
        Ok(Value::Object(output))
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn parse_matrix() -> Result<SupportMatrix> {
    serde_json::from_str(MATRIX_SOURCE).context("invalid solver support matrix source")
}

fn validate_pack(pack: &PackContract, source: &PackSource) -> Result<()> {
    if pack.schema_version != 4 {
        bail!("unsupported pack contract source version")
    }
    if pack.pack.id != source.id
        || pack.pack.latest_schema_version == 0
        || pack.pack.portable_schema_version == 0
        || pack.commands.is_empty()
    {
        bail!("pack descriptor versions, identity or commands are invalid")
    }
    semver::Version::parse(&pack.pack.pack_version)
        .context("packVersion is not semantic versioning")?;
    match (
        source.share_schema,
        pack.pack.share_result_schema_version,
        &pack.share_result_schema,
    ) {
        (Some(_), Some(version), Some(schema)) if version > 0 => {
            require_strict_object_schema(schema, "shareResultSchema")?;
            eutheto_domain_api::validate_contract_schema(schema)?;
        }
        (None, None, None) => {}
        _ => bail!("Share Result products do not match this pack's implementation stage"),
    }
    validate_commands(pack, source)?;
    validate_setup_queries(pack, source)?;
    let mut ai_names = BTreeSet::new();
    let mut ai_commands = BTreeSet::new();
    let mut prior_ai_command: Option<&str> = None;
    let mut prior_ai_name: Option<&str> = None;
    for tool in &pack.ai_tools {
        if !valid_identifier(&tool.name)
            || tool.description.trim().is_empty()
            || !ai_names.insert(&tool.name)
            || !ai_commands.insert(tool.command_id.as_str())
            || prior_ai_command.is_some_and(|prior| prior >= tool.command_id.as_str())
            || prior_ai_name.is_some_and(|prior| prior >= tool.name.as_str())
            || !pack
                .commands
                .iter()
                .any(|command| command.id == tool.command_id)
        {
            bail!("AI tools require sorted unique command references, safe names and descriptions")
        }
        prior_ai_command = Some(&tool.command_id);
        prior_ai_name = Some(&tool.name);
    }
    if source.id == "official.test" && ai_commands.len() != pack.commands.len() {
        bail!("official.test AI tools must cover every command exactly once")
    }
    pack.ui_manifest.validate()?;
    let ui = &pack.ui_manifest;
    if ui.setup_steps.is_empty()
        || ui.entity_kinds.is_empty()
        || ui.rule_kinds.is_empty()
        || ui.goal_kinds.is_empty()
    {
        bail!("implemented editor metadata groups must be nonempty")
    }
    let later_groups = [
        ui.score_kinds.is_empty(),
        ui.provenance_kinds.is_empty(),
        ui.result_views.is_empty(),
        ui.importers.is_empty(),
        ui.exporters.is_empty(),
    ];
    if source.id == "official.test" && later_groups.contains(&true) {
        bail!("official.test requires its complete implemented UI manifest")
    }
    if source.id == "official.workforce" && later_groups.contains(&false) {
        bail!("Workforce cannot publish unimplemented score, result or transfer metadata")
    }
    if !eutheto_types::ScenarioDomain::has_schema_shape(&pack.internal_schema) {
        bail!("internalSchema must describe the four ScenarioDomain maps")
    }
    require_strict_object_schema(&pack.internal_schema, "internalSchema")?;
    require_strict_object_schema(&pack.portable_schema, "portableSchema")?;
    eutheto_domain_api::validate_contract_schema(&pack.internal_schema)?;
    eutheto_domain_api::validate_contract_schema(&pack.portable_schema)?;
    Ok(())
}

fn validate_commands(pack: &PackContract, source: &PackSource) -> Result<()> {
    let namespace = format!("{}.", source.id);
    let mut prior_id: Option<&str> = None;
    for command in &pack.commands {
        let suffix = command
            .id
            .strip_prefix(&namespace)
            .filter(|value| valid_identifier(value))
            .context("command is outside its namespace or cannot produce a safe identifier")?;
        if suffix.bytes().any(|byte| byte.is_ascii_uppercase()) {
            bail!("command suffixes must use lowercase identifiers")
        }
        if prior_id.is_some_and(|prior| prior >= command.id.as_str()) {
            bail!("commands must be uniquely sorted by id")
        }
        prior_id = Some(&command.id);
        for schema in [
            &command.payload_schema,
            &command.result_schema,
            &command.change_schema,
        ] {
            require_strict_object_schema(schema, &command.id)?;
            eutheto_domain_api::validate_contract_schema(schema)?;
        }
        if command.valid_examples.is_empty() || command.invalid_examples.is_empty() {
            bail!("command {} requires valid and invalid examples", command.id)
        }
        if command.title.key.is_empty()
            || command.title.default_text.is_empty()
            || command.description.key.is_empty()
            || command.description.default_text.is_empty()
        {
            bail!("command {} has incomplete metadata", command.id)
        }
        for example in &command.valid_examples {
            eutheto_domain_api::validate_contract_value(
                &command.payload_schema,
                example,
                ContractJsonLimits::DEFAULT,
            )
            .with_context(|| format!("{} has a schema-invalid valid example", command.id))?;
        }
        for example in &command.invalid_examples {
            if eutheto_domain_api::validate_contract_value(
                &command.payload_schema,
                example,
                ContractJsonLimits::DEFAULT,
            )
            .is_ok()
            {
                bail!("{} has a schema-valid invalid example", command.id)
            }
        }
    }
    Ok(())
}

fn validate_setup_queries(pack: &PackContract, source: &PackSource) -> Result<()> {
    let mut prior_id: Option<&str> = None;
    let mut identifiers = BTreeSet::new();
    for suffix in [
        "PACK_ID",
        "PACK_VERSION",
        "PACK_CONTRACT_JSON",
        "COMMAND_IDS",
        "SETUP_QUERY_IDS",
    ] {
        identifiers.insert(format!("{}_{suffix}", source.rust_prefix));
    }
    for command in &pack.commands {
        if !identifiers.insert(command_suffix(&command.id, source)?.to_ascii_uppercase()) {
            bail!("generated command and metadata identifiers collide")
        }
    }
    for query in &pack.setup_queries {
        let suffix = query_suffix(&query.id, source)?;
        if suffix.bytes().any(|byte| byte.is_ascii_uppercase())
            || !identifiers.insert(format!("SETUP_{}_QUERY_ID", suffix.to_ascii_uppercase()))
            || prior_id.is_some_and(|prior| prior >= query.id.as_str())
        {
            bail!("setup queries require lowercase identifiers uniquely sorted by id")
        }
        prior_id = Some(&query.id);
        if pack
            .ui_manifest
            .result_views
            .iter()
            .any(|view| view.id == query.id)
        {
            bail!("setup queries cannot be accepted-result views")
        }
        require_strict_object_schema(&query.parameter_schema, &query.id)?;
        require_strict_object_schema(&query.result_schema, &query.id)?;
        query.validate()?;
    }
    Ok(())
}

fn require_strict_object_schema(schema: &Value, owner: &str) -> Result<()> {
    let object = schema
        .as_object()
        .with_context(|| format!("{owner} schema must be an object"))?;
    if object.get("type").and_then(Value::as_str) != Some("object")
        || object.get("additionalProperties").and_then(Value::as_bool) != Some(false)
    {
        bail!("{owner} schema must be a strict object")
    }
    Ok(())
}

fn validate_matrix(matrix: &SupportMatrix) -> Result<()> {
    if matrix.schema_version != 1 || matrix.planning_ir_schema_version != 2 {
        bail!("unsupported support-matrix or planning-IR schema version")
    }
    let mut prior_feature: Option<&str> = None;
    for feature in &matrix.features {
        if prior_feature.is_some_and(|prior| prior >= feature.id.as_str()) {
            bail!("support-matrix features must be uniquely sorted by id")
        }
        if feature.gate != "unconditional" {
            bail!("support matrix may contain only enabled unconditional features")
        }
        prior_feature = Some(&feature.id);
    }
    let feature_ids = matrix
        .features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut prior_backend: Option<&str> = None;
    for backend in &matrix.registered_backends {
        if prior_backend.is_some_and(|prior| prior >= backend.id.as_str()) {
            bail!("registered backends must be uniquely sorted by id")
        }
        if backend.id.is_empty() || backend.version.is_empty() || backend.adapter_version.is_empty()
        {
            bail!("registered backend descriptors must be complete")
        }
        if backend
            .support
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != feature_ids
        {
            bail!(
                "backend {} must declare every feature exactly once",
                backend.id
            )
        }
        if backend.support.values().any(|cell| !cell.is_complete()) {
            bail!("backend {} has an incomplete support cell", backend.id)
        }
        prior_backend = Some(&backend.id);
    }
    let registered_backend_ids = matrix
        .registered_backends
        .iter()
        .map(|backend| backend.id.as_str())
        .collect::<BTreeSet<_>>();
    for candidate in &matrix.deferred_candidate_gates {
        if candidate.claim_status != "unclaimed" {
            bail!(
                "deferred candidate {} must remain unclaimed",
                candidate.backend_id
            )
        }
        if registered_backend_ids.contains(candidate.backend_id.as_str()) {
            bail!(
                "registered backend {} cannot remain a deferred candidate",
                candidate.backend_id
            )
        }
    }
    Ok(())
}

fn render_rust_pack(pack: &PackContract, source: &PackSource) -> Result<String> {
    let canonical = pretty_json(pack)?;
    let source_hash = blake3::hash(source.contents.as_bytes()).to_hex();
    let prefix = source.rust_prefix;
    let mut output = format!(
        "// SPDX-License-Identifier: Apache-2.0\n// @generated by `cargo xtask generate` from {}; do not edit.\n// source-blake3: {source_hash}\n\npub const {prefix}_PACK_ID: &str = {:?};\npub const {prefix}_PACK_VERSION: &str = {:?};\npub const {prefix}_PACK_CONTRACT_JSON: &str = {canonical:?};\n",
        source.path, pack.pack.id, pack.pack.pack_version,
    );
    for command in &pack.commands {
        let suffix = command_suffix(&command.id, source)?.to_ascii_uppercase();
        writeln!(output, "pub const {suffix}: &str = {:?};", command.id)?;
    }
    if let [command] = pack.commands.as_slice() {
        writeln!(
            output,
            "pub const {prefix}_COMMAND_IDS: &[&str] = &[{:?}];",
            command.id
        )?;
    } else {
        writeln!(output, "pub const {prefix}_COMMAND_IDS: &[&str] = &[")?;
        for command in &pack.commands {
            writeln!(output, "    {:?},", command.id)?;
        }
        output.push_str("];\n");
    }
    if pack.setup_queries.is_empty() {
        writeln!(output, "pub const {prefix}_SETUP_QUERY_IDS: &[&str] = &[];")?;
    } else {
        writeln!(output, "pub const {prefix}_SETUP_QUERY_IDS: &[&str] = &[")?;
        for query in &pack.setup_queries {
            writeln!(output, "    {:?},", query.id)?;
        }
        output.push_str("];\n");
    }
    for query in &pack.setup_queries {
        let suffix = query_suffix(&query.id, source)?.to_ascii_uppercase();
        let declaration = format!("pub const SETUP_{suffix}_QUERY_ID: &str =");
        let value = format!("{:?};", query.id);
        // Match rustfmt's default 100-column constant layout.
        if declaration.len() + 1 + value.len() > 100 {
            writeln!(output, "{declaration}\n    {value}")?;
        } else {
            writeln!(output, "{declaration} {value}")?;
        }
    }
    Ok(output)
}

fn command_suffix<'a>(id: &'a str, source: &PackSource) -> Result<&'a str> {
    id.strip_prefix(source.id)
        .and_then(|suffix| suffix.strip_prefix('.'))
        .filter(|suffix| valid_identifier(suffix))
        .context("command cannot produce a safe generated identifier")
}

fn query_suffix<'a>(id: &'a str, source: &PackSource) -> Result<&'a str> {
    id.strip_prefix("eutheto.setup.")
        .or_else(|| id.strip_prefix(source.id)?.strip_prefix(".setup."))
        .filter(|suffix| valid_identifier(suffix))
        .context("setup query is outside its namespace or cannot produce a safe identifier")
}

fn render_typescript_types(pack: &PackContract, source: &PackSource) -> Result<String> {
    let prefix = source.typescript_prefix;
    let source_hash = blake3::hash(source.contents.as_bytes()).to_hex();
    let mut output = format!(
        "\n// source: {}; source-blake3: {source_hash}\n",
        source.path
    );
    let mut names = BTreeSet::new();
    for (name, schema) in &pack.definitions {
        if !valid_identifier(name) || !names.insert(name.clone()) {
            bail!("schema definition cannot produce a unique TypeScript identifier")
        }
        writeln!(
            output,
            "export type {prefix}{name} = {};",
            typescript_schema(schema, source, 0)?
        )?;
    }
    for command in &pack.commands {
        let suffix = command_suffix(&command.id, source)?;
        let mut name = String::new();
        for word in suffix.split('_') {
            let mut characters = word.chars();
            let first = characters
                .next()
                .context("command contains an empty identifier segment")?;
            name.push(first.to_ascii_uppercase());
            name.extend(characters);
        }
        name.push_str("Payload");
        if !names.insert(name.clone()) {
            bail!("command payload TypeScript identifiers collide")
        }
        let payload = typescript_schema(&command.payload_schema, source, 0)?;
        if source.id == "official.test" && command.id == "official.test.configure_entity" {
            writeln!(output, "export interface {prefix}{name} {payload}")?;
        } else {
            writeln!(output, "export type {prefix}{name} = {payload};")?;
        }
    }
    if pack.setup_queries.is_empty() {
        return Ok(output);
    }
    for (name, result) in [("SetupQueryParameters", false), ("SetupQueryResults", true)] {
        if !names.insert(name.to_owned()) {
            bail!("setup query TypeScript identifiers collide")
        }
        writeln!(output, "export interface {prefix}{name} {{")?;
        for query in &pack.setup_queries {
            let schema = if result {
                &query.result_schema
            } else {
                &query.parameter_schema
            };
            writeln!(
                output,
                "{}: {};",
                serde_json::to_string(&query.id)?,
                typescript_schema(schema, source, 0)?
            )?;
        }
        output.push_str("}\n");
    }
    render_typescript_setup_guards(pack, source, &mut output)?;
    Ok(output)
}

fn typescript_schema(schema: &Value, source: &PackSource, depth: usize) -> Result<String> {
    if depth > 32 {
        bail!("TypeScript schema rendering exceeds its depth limit")
    }
    let object = schema
        .as_object()
        .context("TypeScript schema must be an object")?;
    if let Some(reference) = object.get("$ref") {
        let name = reference
            .as_str()
            .and_then(|value| value.strip_prefix("#/$defs/"))
            .filter(|name| valid_identifier(name))
            .context("TypeScript schema requires an exact local reference")?;
        if object.len() != 1 {
            bail!("TypeScript schema references cannot have siblings")
        }
        return Ok(format!("{}{name}", source.typescript_prefix));
    }
    if let Some(value) = object.get("const") {
        if !value.is_null() && !value.is_boolean() && !value.is_number() && !value.is_string() {
            bail!("TypeScript contract constants must be scalar values")
        }
        return Ok(serde_json::to_string(value)?);
    }
    if let Some(options) = object.get("oneOf") {
        if object.contains_key("properties")
            || object.contains_key("required")
            || object.contains_key("additionalProperties")
        {
            bail!("TypeScript unions cannot contain sibling object-shape constraints")
        }
        return options
            .as_array()
            .context("TypeScript oneOf must be an array")?
            .iter()
            .map(|option| typescript_schema(option, source, depth + 1))
            .collect::<Result<Vec<_>>>()
            .map(|options| options.join(" | "));
    }
    match object.get("type").and_then(Value::as_str) {
        None if object.is_empty() => Ok("unknown".to_owned()),
        Some("string") => Ok("string".to_owned()),
        Some("integer" | "number") => Ok("number".to_owned()),
        Some("boolean") => Ok("boolean".to_owned()),
        Some("null") => Ok("null".to_owned()),
        Some("array") => Ok(format!(
            "ReadonlyArray<{}>",
            typescript_schema(
                object.get("items").context("array schema requires items")?,
                source,
                depth + 1,
            )?
        )),
        Some("object") => {
            let properties = object.get("properties").and_then(Value::as_object);
            let required = object.get("required").and_then(Value::as_array);
            let mut output = String::from("{\n");
            if let Some(properties) = properties {
                for (name, child) in properties {
                    let optional = if required.is_some_and(|required| {
                        required.iter().any(|value| value.as_str() == Some(name))
                    }) {
                        ""
                    } else {
                        "?"
                    };
                    writeln!(
                        output,
                        "readonly {}{optional}: {};",
                        serde_json::to_string(name)?,
                        typescript_schema(child, source, depth + 1)?
                    )?;
                }
            }
            output.push('}');
            let additional = match object.get("additionalProperties") {
                Some(Value::Bool(false)) => None,
                Some(value) if value.is_object() => {
                    Some(typescript_schema(value, source, depth + 1)?)
                }
                None | Some(Value::Bool(true)) => Some("unknown".to_owned()),
                _ => bail!("invalid additionalProperties in TypeScript schema"),
            };
            if properties.is_none_or(Map::is_empty) {
                return Ok(format!(
                    "Readonly<Record<string, {}>>",
                    additional.as_deref().unwrap_or("never")
                ));
            }
            if let Some(additional) = additional {
                write!(output, " & Readonly<Record<string, {additional}>>")?;
            }
            Ok(output)
        }
        _ => bail!("schema cannot produce a supported TypeScript structural type"),
    }
}

fn collect_guard_refs<'a>(schema: &'a Value, references: &mut BTreeSet<&'a str>) -> Result<()> {
    match schema {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref") {
                references.insert(
                    reference
                        .as_str()
                        .and_then(|value| value.strip_prefix("#/$defs/"))
                        .context("wire guards require local schema references")?,
                );
            }
            for child in object.values() {
                collect_guard_refs(child, references)?;
            }
        }
        Value::Array(children) => {
            for child in children {
                collect_guard_refs(child, references)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn render_typescript_setup_guards(
    pack: &PackContract,
    source: &PackSource,
    output: &mut String,
) -> Result<()> {
    let prefix = source.typescript_prefix;
    for reserved in [
        "WireObject",
        "WireString",
        "SetupViewId",
        "SetupQueryResult",
    ] {
        if pack.definitions.contains_key(reserved) {
            bail!("schema definition conflicts with a wire guard helper");
        }
    }
    let mut references = BTreeSet::new();
    for query in &pack.setup_queries {
        collect_guard_refs(&query.result_schema, &mut references)?;
    }
    let mut expanded = BTreeSet::new();
    loop {
        let next = references.difference(&expanded).next().copied();
        let Some(name) = next else { break };
        let schema = pack
            .definitions
            .get(name)
            .context("wire guard references an absent definition")?;
        collect_guard_refs(schema, &mut references)?;
        expanded.insert(name);
    }
    // Static predicates use the existing restricted schema vocabulary, not a runtime interpreter.
    // Typed setup DTO IDs are canonical; raw CSV domain values use a separate schema guard.
    output.push_str(&r#"
const __PREFIX__WireUuid = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
function is__PREFIX__WireObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const prototype: unknown = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}
function check__PREFIX__WireProperties(
  value: Record<string, unknown>, check: (key: string, child: unknown) => boolean,
): boolean {
  for (const key of Object.keys(value)) {
    if (!check(key, value[key])) return false;
  }
  return true;
}
function is__PREFIX__WireString(value: unknown, minimum: number, maximum: number): value is string {
  if (typeof value !== "string" || value.length < minimum || value.length > maximum * 2) return false;
  let count = 0;
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const low = value.charCodeAt(index + 1);
      if (!(low >= 0xdc00 && low <= 0xdfff)) return false;
      index += 1;
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      return false;
    }
    count += 1;
    if (count > maximum) return false;
  }
  return count >= minimum;
}
"#.replace("__PREFIX__", prefix));
    for name in references {
        let schema = pack
            .definitions
            .get(name)
            .context("wire guard definition disappeared")?;
        writeln!(
            output,
            "function is{prefix}{name}(value: unknown): value is {prefix}{name} {{ return {}; }}",
            typescript_guard(schema, source, "value", 0, None)?
        )?;
    }
    if prefix == "Workforce" {
        render_typescript_csv_person_guard(pack, source, output)?;
    }
    writeln!(
        output,
        "const {prefix}SetupResultGuards: {{ readonly [K in keyof {prefix}SetupQueryResults]: (value: unknown) => value is {prefix}SetupQueryResults[K] }} = {{"
    )?;
    for query in &pack.setup_queries {
        writeln!(
            output,
            "{}: (value: unknown): value is {prefix}SetupQueryResults[{}] => {},",
            serde_json::to_string(&query.id)?,
            serde_json::to_string(&query.id)?,
            typescript_guard(&query.result_schema, source, "value", 0, None)?
        )?;
    }
    writeln!(
        output,
        "}};\nexport function is{prefix}SetupViewId(value: unknown): value is keyof {prefix}SetupQueryResults {{ return typeof value === \"string\" && Object.hasOwn({prefix}SetupResultGuards, value); }}"
    )?;
    writeln!(
        output,
        "export function is{prefix}SetupQueryResult<K extends keyof {prefix}SetupQueryResults>(query: K, value: unknown): value is {prefix}SetupQueryResults[K] {{ return Object.hasOwn({prefix}SetupResultGuards, query) && {prefix}SetupResultGuards[query](value); }}"
    )?;
    Ok(())
}

fn render_typescript_csv_person_guard(
    pack: &PackContract,
    source: &PackSource,
    output: &mut String,
) -> Result<()> {
    // Raw domain Value payloads preserve schema-valid UUID case. Expand this one
    // schema so its UUID policy cannot widen canonical typed setup DTO guards.
    let mut expander = SchemaExpander {
        definitions: &pack.definitions,
        stack: Vec::new(),
        used: BTreeSet::new(),
        nodes: 0,
        remaining_bytes: MAX_PACK_CONTRACT_BYTES,
    };
    let person = expander.expand(
        pack.definitions
            .get("Person")
            .context("CSV person schema missing")?,
        0,
    )?;
    output.push_str("const WorkforceCsvUuid = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;\n");
    writeln!(
        output,
        "export function isWorkforceCsvPerson(value: unknown): value is WorkforcePerson {{ return {}; }}",
        typescript_guard(&person, source, "value", 0, Some("WorkforceCsvUuid"))?
    )?;
    Ok(())
}

fn typescript_guard(
    schema: &Value,
    source: &PackSource,
    value: &str,
    depth: usize,
    uuid_pattern: Option<&str>,
) -> Result<String> {
    if depth > 32 {
        bail!("wire guard rendering exceeds its depth limit");
    }
    let object = schema
        .as_object()
        .context("wire guard schema must be an object")?;
    if let Some(reference) = object.get("$ref") {
        if uuid_pattern.is_some() {
            bail!("schema-value UUID policy requires expanded references");
        }
        let name = reference
            .as_str()
            .and_then(|reference| reference.strip_prefix("#/$defs/"))
            .filter(|name| valid_identifier(name))
            .context("invalid wire guard reference")?;
        if object.len() != 1 {
            bail!("wire guard references cannot have siblings");
        }
        return Ok(format!("is{}{name}({value})", source.typescript_prefix));
    }
    let mut clauses = Vec::new();
    if let Some(expected) = object.get("const") {
        clauses.push(format!("{value} === {}", serde_json::to_string(expected)?));
    }
    if let Some(options) = object.get("oneOf").and_then(Value::as_array) {
        let alternatives = options
            .iter()
            .map(|option| {
                typescript_guard(option, source, value, depth + 1, uuid_pattern)
                    .map(|guard| format!("Number({guard})"))
            })
            .collect::<Result<Vec<_>>>()?;
        clauses.push(format!("({}) === 1", alternatives.join(" + ")));
    }
    match object.get("type").and_then(Value::as_str) {
        Some("string") => clauses.push(typescript_guard_string(
            object,
            source,
            value,
            uuid_pattern,
        )?),
        Some("integer") => {
            clauses.push(format!(
                "typeof {value} === \"number\" && Number.isSafeInteger({value})"
            ));
            for (bound, operator) in [("minimum", ">="), ("maximum", "<=")] {
                if let Some(limit) = object.get(bound) {
                    let limit = limit.as_i64().context("wire integer bound must be exact")?;
                    if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&limit) {
                        bail!("wire integer requires an explicit lossless string contract");
                    }
                    clauses.push(format!("{value} {operator} {limit}"));
                }
            }
        }
        Some("boolean") => clauses.push(format!("typeof {value} === \"boolean\"")),
        Some("null") => clauses.push(format!("{value} === null")),
        Some("array") => {
            let minimum = object.get("minItems").and_then(Value::as_u64).unwrap_or(0);
            let maximum = object
                .get("maxItems")
                .and_then(Value::as_u64)
                .unwrap_or(1_000_000);
            let item = format!("item{depth}");
            let child = typescript_guard(
                object.get("items").context("wire array requires items")?,
                source,
                &item,
                depth + 1,
                uuid_pattern,
            )?;
            clauses.push(format!("Array.isArray({value}) && {value}.length >= {minimum} && {value}.length <= {maximum} && {value}.every(({item}: unknown) => {child})"));
        }
        Some("object") => clauses.push(typescript_guard_object(
            object,
            source,
            value,
            depth,
            uuid_pattern,
        )?),
        None => {}
        _ => bail!("unsupported wire guard schema type"),
    }
    // {} is intentionally opaque. The enclosing IPC reader already checks the entire JSON budget.
    Ok(if clauses.is_empty() {
        "true".to_owned()
    } else {
        format!("({})", clauses.join(" && "))
    })
}

fn typescript_guard_string(
    schema: &Map<String, Value>,
    source: &PackSource,
    value: &str,
    uuid_pattern: Option<&str>,
) -> Result<String> {
    let prefix = source.typescript_prefix;
    let minimum = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0);
    let maximum = schema
        .get("maxLength")
        .and_then(Value::as_u64)
        .unwrap_or(1_048_576);
    let mut clauses = vec![format!(
        "is{prefix}WireString({value}, {minimum}, {maximum})"
    )];
    if let Some(format) = schema.get("format").and_then(Value::as_str) {
        clauses.push(match format {
            "uuid" => match uuid_pattern {
                Some(pattern) => format!("{pattern}.test({value})"),
                None => format!("{prefix}WireUuid.test({value})"),
            },
            "scenario-change-path" => {
                format!("({value} === \"/settings\" || {value}.startsWith(\"/domain/\"))")
            }
            _ => bail!("unsupported wire string format"),
        });
    }
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        let prefix = pattern
            .strip_prefix('^')
            .context("wire schema pattern requires a literal prefix")?;
        clauses.push(format!(
            "{value}.startsWith({})",
            serde_json::to_string(prefix)?
        ));
    }
    Ok(format!("({})", clauses.join(" && ")))
}

fn typescript_guard_object(
    schema: &Map<String, Value>,
    source: &PackSource,
    value: &str,
    depth: usize,
    uuid_pattern: Option<&str>,
) -> Result<String> {
    let mut clauses = vec![format!("is{}WireObject({value})", source.typescript_prefix)];
    let properties = schema.get("properties").and_then(Value::as_object);
    let required = schema.get("required").and_then(Value::as_array);
    if let Some(required) = required {
        for name in required {
            let name = name
                .as_str()
                .context("required wire property must be a string")?;
            if properties.is_none_or(|properties| !properties.contains_key(name)) {
                bail!("required wire property must have a declared schema");
            }
        }
    }
    if let Some(properties) = properties {
        // Test constant tags before traversing payloads of alternatives that cannot match.
        let ordered = properties
            .iter()
            .filter(|(_, child)| child.get("const").is_some())
            .chain(
                properties
                    .iter()
                    .filter(|(_, child)| child.get("const").is_none()),
            );
        for (name, child) in ordered {
            let key = serde_json::to_string(name)?;
            let member = format!("{value}[{key}]");
            let guard = typescript_guard(child, source, &member, depth + 1, uuid_pattern)?;
            if required
                .is_some_and(|required| required.iter().any(|item| item.as_str() == Some(name)))
            {
                clauses.push(if guard == "true" {
                    format!("Object.hasOwn({value}, {key})")
                } else {
                    format!("Object.hasOwn({value}, {key}) && {guard}")
                });
            } else if guard != "true" {
                clauses.push(format!("(!Object.hasOwn({value}, {key}) || {guard})"));
            }
        }
    }
    let key = format!("key{depth}");
    let mut alternatives = properties
        .into_iter()
        .flat_map(Map::keys)
        .map(|name| serde_json::to_string(name).map(|name| format!("{key} === {name}")))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match schema.get("additionalProperties") {
        Some(Value::Bool(false)) => {}
        Some(additional) if additional.is_object() => {
            let member = format!("member{depth}");
            let guard = typescript_guard(additional, source, &member, depth + 1, uuid_pattern)?;
            if guard == "true" {
                return Ok(format!("({})", clauses.join(" && ")));
            }
            let key = if alternatives.is_empty() {
                format!("_key{depth}")
            } else {
                key
            };
            alternatives.push(guard);
            clauses.push(format!(
                "check{}WireProperties({value}, ({key}, {member}) => {})",
                source.typescript_prefix,
                alternatives.join(" || ")
            ));
            return Ok(format!("({})", clauses.join(" && ")));
        }
        None | Some(Value::Bool(true)) => return Ok(format!("({})", clauses.join(" && "))),
        _ => bail!("invalid additionalProperties in wire guard schema"),
    }
    let keys_guard = if alternatives.is_empty() {
        "false".to_owned()
    } else {
        alternatives.join(" || ")
    };
    clauses.push(format!(
        "Object.keys({value}).every(({key}) => {keys_guard})"
    ));
    Ok(format!("({})", clauses.join(" && ")))
}

fn render_typescript_constants(pack: &PackContract, source: &PackSource) -> Result<String> {
    let prefix = source.rust_prefix;
    let mut output = format!(
        "\nexport const {prefix}_PACK_ID = {} as const;\n",
        serde_json::to_string(&pack.pack.id)?
    );
    for command in &pack.commands {
        let suffix = command_suffix(&command.id, source)?.to_ascii_uppercase();
        writeln!(
            output,
            "export const {prefix}_{suffix}_COMMAND_ID = {} as const;",
            serde_json::to_string(&command.id)?
        )?;
    }
    for query in &pack.setup_queries {
        let suffix = query_suffix(&query.id, source)?.to_ascii_uppercase();
        writeln!(
            output,
            "export const {prefix}_SETUP_{suffix}_QUERY_ID = {} as const;",
            serde_json::to_string(&query.id)?
        )?;
    }
    writeln!(
        output,
        "export const {prefix}_PACK_CONTRACT_JSON = {};",
        serde_json::to_string(&pretty_json(pack)?)?
    )?;
    Ok(output)
}

pub(crate) fn format_typescript(
    repo_root: &Path,
    relative_path: &str,
    contents: &str,
) -> Result<String> {
    let desktop = repo_root.join("apps/desktop");
    let prettier = || {
        let mut command = Command::new(if cfg!(windows) { "pnpm.cmd" } else { "pnpm" });
        command.current_dir(&desktop).args(["exec", "prettier"]);
        command
    };
    let version = prettier()
        .arg("--version")
        .output()
        .context("failed to start Prettier")?;
    if !version.status.success()
        || String::from_utf8_lossy(&version.stdout).trim() != PRETTIER_VERSION
    {
        bail!("generation requires installed Prettier {PRETTIER_VERSION}; run `just install`")
    }
    let info = prettier().args(["--file-info", relative_path]).output()?;
    let info_json: Value =
        serde_json::from_slice(&info.stdout).context("Prettier did not return file information")?;
    if !info.status.success()
        || info_json.get("ignored").and_then(Value::as_bool) != Some(false)
        || info_json.get("inferredParser").and_then(Value::as_str) != Some("typescript")
    {
        bail!("the generated TypeScript product cannot be ignored or use another parser")
    }
    let config = std::fs::read(desktop.join(".prettierrc.json"))?;
    let config_hash = blake3::hash(&config).to_hex();
    let provenance =
        format!("\n// formatter: prettier@{PRETTIER_VERSION}; config-blake3: {config_hash}\n");
    let mut child = prettier()
        .args([
            "--config",
            ".prettierrc.json",
            "--no-editorconfig",
            "--stdin-filepath",
            relative_path,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start the pinned TypeScript formatter")?;
    let written = {
        let mut input = child
            .stdin
            .take()
            .context("formatter stdin is unavailable")?;
        input
            .write_all(contents.as_bytes())
            .and_then(|()| input.write_all(provenance.as_bytes()))
    };
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "Prettier failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
    written.context("failed to send the generated product to Prettier")?;
    String::from_utf8(output.stdout).context("Prettier returned non-UTF-8 output")
}

fn render_command_schemas(pack: &PackContract) -> Result<String> {
    let mut commands = Map::new();
    for command in &pack.commands {
        commands.insert(
            command.id.clone(),
            json!({
                "payload": command.payload_schema,
                "result": command.result_schema,
                "change": command.change_schema,
                "validExamples": command.valid_examples,
                "invalidExamples": command.invalid_examples,
                "risk": command.risk,
                "reversibility": command.reversibility,
                "aiGroupingAllowed": command.ai_grouping_allowed,
            }),
        );
    }
    pretty_json(&json!({
        "schemaVersion": pack.schema_version,
        "packId": pack.pack.id,
        "commands": commands,
    }))
}

fn render_query_schemas(pack: &PackContract) -> Result<String> {
    let queries: BTreeMap<_, _> = pack
        .setup_queries
        .iter()
        .map(|query| {
            (
                &query.id,
                json!({
                    "sources": query.sources,
                    "supportsContinuation": query.supports_continuation,
                    "parameters": query.parameter_schema,
                    "result": query.result_schema,
                    "validExamples": query.valid_examples,
                    "invalidExamples": query.invalid_examples,
                }),
            )
        })
        .collect();
    pretty_json(&json!({
        "schemaVersion": pack.schema_version,
        "packId": pack.pack.id,
        "setupQueries": queries,
    }))
}

fn render_ai_tools(pack: &PackContract, source: &PackSource) -> Result<String> {
    pretty_json(&json!({
        "schemaVersion": pack.schema_version,
        "$comment": format!("Generated by cargo xtask generate from {}; do not edit.", source.path),
        "sourceBlake3": blake3::hash(source.contents.as_bytes()).to_hex().to_string(),
        "packId": pack.pack.id,
        "tools": pack.ai_tools,
    }))
}

fn render_ui_manifest(pack: &PackContract, source: &PackSource) -> Result<String> {
    pretty_json(&json!({
        "schemaVersion": pack.schema_version,
        "$comment": format!("Generated by cargo xtask generate from {}; do not edit.", source.path),
        "sourceBlake3": blake3::hash(source.contents.as_bytes()).to_hex().to_string(),
        "packId": pack.pack.id,
        "manifest": pack.ui_manifest,
    }))
}

fn render_schema(schema: &Value, owner: &str) -> Result<String> {
    eutheto_domain_api::validate_contract_schema(schema)
        .with_context(|| format!("invalid {owner}"))?;
    pretty_json(schema)
}

fn pretty_json(value: &impl Serialize) -> Result<String> {
    let mut output =
        serde_json::to_string_pretty(value).context("failed to render generated JSON")?;
    output.push('\n');
    Ok(output)
}

fn render_pack_docs(pack: &PackContract, source: &PackSource) -> Result<String> {
    let source_hash = blake3::hash(source.contents.as_bytes()).to_hex();
    let mut output = format!(
        "<!-- SPDX-License-Identifier: Apache-2.0 -->\n<!-- @generated by `cargo xtask generate` from `{}`; do not edit. -->\n<!-- source-blake3: {source_hash} -->\n\n# `{}` generated contract\n\nPack version `{}`, domain schema `{}`, portable schema `{}`.",
        source.path,
        pack.pack.id,
        pack.pack.pack_version,
        pack.pack.latest_schema_version,
        pack.pack.portable_schema_version,
    );
    if let Some(version) = pack.pack.share_result_schema_version {
        write!(output, " Share Result schema `{version}`.")?;
    } else {
        output.push_str(
            " Share Result is absent. This source describes an unregistered pack's implemented data and command surface, not solver, verifier, score, result or transfer support.",
        );
    }
    output.push_str("\n\n| Command | Risk | Reversibility | AI grouping |\n|---|---|---|---|\n");
    for command in &pack.commands {
        writeln!(
            output,
            "| `{}` | `{}` | `{}` | `{}` |",
            command.id,
            serde_json::to_string(&command.risk)?,
            serde_json::to_string(&command.reversibility)?,
            command.ai_grouping_allowed
        )?;
    }
    if !pack.setup_queries.is_empty() {
        output.push_str("\n\n## Read-only setup queries\n\nThese projections are not mutation commands or accepted-result views.\n\n| Query | Description |\n|---|---|\n");
        for query in &pack.setup_queries {
            writeln!(
                output,
                "| `{}` | {} |",
                query.id, query.description.default_text
            )?;
        }
    }
    Ok(output)
}

fn render_rust_matrix(matrix: &SupportMatrix) -> String {
    let source_hash = blake3::hash(MATRIX_SOURCE.as_bytes()).to_hex();
    let mut output = format!(
        "// SPDX-License-Identifier: Apache-2.0\n// @generated by `cargo xtask generate` from schemas/solver-support-matrix.json; do not edit.\n// source-blake3: {source_hash}\n\npub const SUPPORT_MATRIX_SCHEMA_VERSION: u32 = {};\npub const SUPPORT_MATRIX_IR_SCHEMA_VERSION: u32 = {};\npub const SUPPORT_FEATURES: &[(&str, &str, &str)] = &[\n",
        matrix.schema_version, matrix.planning_ir_schema_version
    );
    for feature in &matrix.features {
        let _ = writeln!(
            output,
            "    ({:?}, {:?}, {:?}),",
            feature.id, feature.category, feature.gate
        );
    }
    output.push_str("];\n");
    if matrix.registered_backends.is_empty() {
        output.push_str("pub const PRODUCTION_BACKENDS: &[(&str, &str, &str)] = &[];\n");
    } else if let [backend] = matrix.registered_backends.as_slice() {
        let _ = writeln!(
            output,
            "pub const PRODUCTION_BACKENDS: &[(&str, &str, &str)] =\n    &[({:?}, {:?}, {:?})];",
            backend.id, backend.version, backend.adapter_version
        );
    } else {
        output.push_str("pub const PRODUCTION_BACKENDS: &[(&str, &str, &str)] = &[\n");
        for backend in &matrix.registered_backends {
            let _ = writeln!(
                output,
                "    ({:?}, {:?}, {:?}),",
                backend.id, backend.version, backend.adapter_version
            );
        }
        output.push_str("];\n");
    }
    output.push_str(
        "pub(crate) const PRODUCTION_SUPPORT_CELLS: &[(&str, &str, &str, &str, &str, &str, &str)] = &[\n",
    );
    for backend in &matrix.registered_backends {
        for (feature_id, cell) in &backend.support {
            let (level, restriction_id, reason, remediation, fixture_id) =
                generated_cell_fields(cell);
            let _ = writeln!(
                output,
                "    (\n        {:?},\n        {:?},\n        {:?},\n        {:?},\n        {:?},\n        {:?},\n        {:?},\n    ),",
                backend.id, feature_id, level, restriction_id, reason, remediation, fixture_id
            );
        }
    }
    output.push_str("];\n");
    if matrix.deferred_candidate_gates.is_empty() {
        output.push_str("pub const DEFERRED_BACKEND_CANDIDATES: &[(&str, &str, u32)] = &[];\n");
    } else if let [candidate] = matrix.deferred_candidate_gates.as_slice() {
        let _ = writeln!(
            output,
            "pub const DEFERRED_BACKEND_CANDIDATES: &[(&str, &str, u32)] = &[({:?}, {:?}, {})];",
            candidate.backend_id, candidate.candidate_version, candidate.owning_phase
        );
    } else {
        output.push_str("pub const DEFERRED_BACKEND_CANDIDATES: &[(&str, &str, u32)] = &[\n");
        for candidate in &matrix.deferred_candidate_gates {
            let _ = writeln!(
                output,
                "    ({:?}, {:?}, {}),",
                candidate.backend_id, candidate.candidate_version, candidate.owning_phase
            );
        }
        output.push_str("];\n");
    }
    output
}

fn generated_cell_fields(cell: &SourceSupportCell) -> (&str, &str, &str, &str, &str) {
    match cell {
        SourceSupportCell::Supported { fixture_id } => {
            ("supported", "", "", "", fixture_id.as_str())
        }
        SourceSupportCell::Degraded {
            restriction_id,
            reason,
            remediation,
            fixture_id,
        } => ("degraded", restriction_id, reason, remediation, fixture_id),
        SourceSupportCell::Unsupported {
            reason,
            remediation,
            fixture_id,
        } => ("unsupported", "", reason, remediation, fixture_id),
    }
}

fn render_matrix_docs(matrix: &SupportMatrix) -> Result<String> {
    let source_hash = blake3::hash(MATRIX_SOURCE.as_bytes()).to_hex();
    let mut output = format!(
        "<!-- SPDX-License-Identifier: Apache-2.0 -->\n<!-- @generated by `cargo xtask generate` from `schemas/solver-support-matrix.json`; do not edit. -->\n<!-- source-blake3: {source_hash} -->\n\n# Solver support matrix\n\nThis generated view lists every production backend/version/adapter and its exact tested claim for each planning feature. Fake backends remain test-only and are not production columns.\n\n"
    );
    output.push_str("| Feature | Category | Gate");
    for backend in &matrix.registered_backends {
        let _ = write!(
            output,
            " | `{}` `{}` / adapter `{}`",
            backend.id, backend.version, backend.adapter_version
        );
    }
    output.push_str(" |\n|---|---|---");
    for _ in &matrix.registered_backends {
        output.push_str("|---");
    }
    output.push_str("|\n");
    for feature in &matrix.features {
        let _ = write!(
            output,
            "| `{}` | `{}` | `{}`",
            feature.id, feature.category, feature.gate
        );
        for backend in &matrix.registered_backends {
            let cell = backend.support.get(&feature.id).with_context(|| {
                format!(
                    "validated backend {} is missing feature {}",
                    backend.id, feature.id
                )
            })?;
            let _ = write!(output, " | {}", render_doc_cell(cell));
        }
        output.push_str(" |\n");
    }
    output.push_str("\n## Deferred candidates\n\n| Backend | Candidate version | Claim | Owning phase |\n|---|---|---|---|\n");
    for candidate in &matrix.deferred_candidate_gates {
        let _ = writeln!(
            output,
            "| `{}` | `{}` | `{}` | `{}` |",
            candidate.backend_id,
            candidate.candidate_version,
            candidate.claim_status,
            candidate.owning_phase
        );
    }
    Ok(output)
}

fn render_doc_cell(cell: &SourceSupportCell) -> String {
    let rendered = match cell {
        SourceSupportCell::Supported { fixture_id } => {
            format!("supported; fixture `{fixture_id}`")
        }
        SourceSupportCell::Degraded {
            restriction_id,
            reason,
            remediation,
            fixture_id,
        } => format!(
            "restricted `{restriction_id}`: {reason} Remediation: {remediation} Fixture: `{fixture_id}`"
        ),
        SourceSupportCell::Unsupported {
            reason,
            remediation,
            fixture_id,
        } => format!("unsupported: {reason} Remediation: {remediation} Fixture: `{fixture_id}`"),
    };
    rendered.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde_json::json;

    use super::{PACK_SOURCES, expand_pack_schemas, parse_pack, render_schema, validate_pack};

    #[test]
    fn duplicate_ai_tool_coverage_is_rejected() -> Result<()> {
        let mut pack = parse_pack(&PACK_SOURCES[0])?;
        pack.ai_tools.push(pack.ai_tools[0].clone());
        assert!(validate_pack(&pack, &PACK_SOURCES[0]).is_err());
        Ok(())
    }

    #[test]
    fn internal_schema_cannot_omit_a_host_domain_map() -> Result<()> {
        let mut pack = parse_pack(&PACK_SOURCES[0])?;
        pack.internal_schema["properties"]
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("schema properties must be an object"))?
            .remove("lockedAssignments");
        pack.internal_schema["required"]
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("schema required must be an array"))?
            .retain(|field| field.as_str() != Some("lockedAssignments"));
        assert!(validate_pack(&pack, &PACK_SOURCES[0]).is_err());
        Ok(())
    }

    #[test]
    fn rendered_schemas_are_consumable_by_the_runtime_evaluator() -> Result<()> {
        for source in &PACK_SOURCES {
            let mut pack = parse_pack(source)?;
            expand_pack_schemas(&mut pack)?;
            validate_pack(&pack, source)?;
            for schema in [&pack.internal_schema, &pack.portable_schema]
                .into_iter()
                .chain(pack.share_result_schema.as_ref())
            {
                let rendered = render_schema(schema, "pack schema")?;
                let parsed = serde_json::from_str(&rendered)?;
                eutheto_domain_api::validate_contract_schema(&parsed)?;
            }
        }
        Ok(())
    }

    #[test]
    fn local_references_reject_ambiguous_external_cyclic_and_unused_definitions() -> Result<()> {
        for (definitions, schema) in [
            (json!({}), json!({"$ref":"#/$defs/Missing"})),
            (json!({}), json!({"$ref":"https://example.invalid/schema"})),
            (
                json!({"Leaf":{"type":"string"}}),
                json!({"$ref":"#/$defs/Leaf","type":"string"}),
            ),
            (
                json!({"Loop":{"$ref":"#/$defs/Loop"}}),
                json!({"$ref":"#/$defs/Loop"}),
            ),
            (
                json!({"Leaf":{"type":"string"}}),
                json!({"$ref":"#/$defs/Leaf/properties"}),
            ),
            (
                json!({"Unused":{"type":"string"}}),
                json!({"type":"string"}),
            ),
        ] {
            let mut pack = parse_pack(&PACK_SOURCES[0])?;
            pack.definitions = serde_json::from_value(definitions)?;
            pack.internal_schema = schema;
            assert!(expand_pack_schemas(&mut pack).is_err());
        }
        Ok(())
    }

    #[test]
    fn repeated_references_cannot_amplify_past_node_or_byte_bounds() -> Result<()> {
        for (levels, leaf) in [
            (17, json!({"type":"string"})),
            (7, json!({"const":"x".repeat(512 * 1024)})),
        ] {
            let mut pack = parse_pack(&PACK_SOURCES[0])?;
            pack.definitions.insert("Level0".to_owned(), leaf);
            for level in 1..=levels {
                let previous = format!("#/$defs/Level{}", level - 1);
                pack.definitions.insert(
                    format!("Level{level}"),
                    json!({"oneOf":[{"$ref":previous},{"$ref":previous}]}),
                );
            }
            pack.internal_schema = json!({"$ref":format!("#/$defs/Level{levels}")});
            assert!(expand_pack_schemas(&mut pack).is_err());
        }
        Ok(())
    }

    #[test]
    fn inventory_finds_stale_pack_products_without_owning_unrelated_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("schemas/generated");
        std::fs::create_dir_all(&directory)?;
        for name in [
            "workforce.internal.schema.json",
            "workforce.share-result.schema.json",
            "unrelated.json",
        ] {
            std::fs::write(directory.join(name), "{}")?;
        }
        let expected = vec![(
            "schemas/generated/workforce.internal.schema.json".to_owned(),
            b"{}".to_vec(),
        )];
        assert_eq!(
            super::unexpected_files(root.path(), &expected)?,
            ["schemas/generated/workforce.share-result.schema.json"]
        );
        assert_eq!(std::fs::read(directory.join("unrelated.json"))?, b"{}");
        Ok(())
    }

    #[test]
    fn query_constants_cannot_collide_with_command_constants() -> Result<()> {
        let source = &PACK_SOURCES[1];
        let mut pack = parse_pack(source)?;
        expand_pack_schemas(&mut pack)?;
        super::validate_setup_queries(&pack, source)?;
        let mut command = pack.commands[0].clone();
        command.id = "official.workforce.setup_command_changes_query_id".to_owned();
        pack.commands.push(command);
        assert!(super::validate_setup_queries(&pack, source).is_err());
        Ok(())
    }
}
