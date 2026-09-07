//! Stable, implementation-agnostic contracts for compiled-in domain packs.
//!
//! This crate owns no persistence, solver, plugin loader, UI renderer, or acceptance policy.
//! Packs consume immutable scenario data and return inert data contracts.

mod schema;

pub use schema::{
    ContractJsonLimits, ValidatedContractSchema, bounded_json_size, validate_contract_schema,
    validate_contract_value,
};

use eutheto_domain_ir::{
    AcceptedResult, CounterfactualConditionV1, EvidenceRenderRequestV1, EvidenceRenderResultV1,
    ExplanationCapability, NormalizedSolution, ScoreVector, VerificationContextV1,
    VerificationReport, VerificationScope,
};
use eutheto_planning_ir::{CandidateValues, PlanningIrLimitsV1, PlanningProblem};
use eutheto_types::{
    CancellationToken, DomainCommandEnvelope, MAX_SCENARIO_DOCUMENT_BYTES, PackId,
    PortableDomainDocument, ScenarioDocument, SemanticCapability, SolutionId, SolveBudgetView,
    ValidationIssue,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use thiserror::Error;

/// Current generic domain-pack command-batch schema.
pub const DOMAIN_BATCH_SCHEMA_VERSION: u32 = 1;
/// Largest number of commands accepted in one pack batch.
pub const MAX_DOMAIN_BATCH_COMMANDS: usize = 1_000;
/// Maximum change records emitted by one atomic mutation.
pub const MAX_DOMAIN_MUTATION_CHANGES: usize = 85_536;
/// Maximum compact JSON bytes in an atomic mutation's change records.
pub const MAX_DOMAIN_MUTATION_CHANGE_BYTES: usize = 64 * 1024 * 1024;
/// Maximum compact JSON bytes in an atomic mutation's result values.
pub const MAX_DOMAIN_MUTATION_RESULT_BYTES: usize = 16 * 1024 * 1024;

/// Localizable data. `default_text` is safe fallback text, not identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalizedText {
    pub key: String,
    pub default_text: String,
}

/// License/distribution data shown without loading a concrete pack type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LicenseMetadata {
    pub spdx_expression: String,
    pub attribution: String,
}

/// Stable domain capability declarations.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DomainCapability {
    Commands,
    Compilation,
    Projection,
    Verification,
    Scoring,
    PortableData,
    ShareResult,
    UiManifest,
    AiTools,
}

/// Current and sequentially migratable versions within one schema namespace.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaVersionDescriptor {
    pub latest: u32,
    pub migratable_from: BTreeSet<u32>,
}

impl SchemaVersionDescriptor {
    /// Whether the version can be opened directly or migrated sequentially.
    #[must_use]
    pub fn supports(&self, version: u32) -> bool {
        version == self.latest || self.migratable_from.contains(&version)
    }

    fn has_complete_migration_paths(&self) -> bool {
        self.latest != 0
            && self.migratable_from.iter().all(|version| {
                version.checked_add(1).is_some_and(|next| {
                    next == self.latest
                        || (next < self.latest && self.migratable_from.contains(&next))
                })
            })
    }
}

/// Deterministic data-only identity and compatibility description.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainPackDescriptor {
    pub id: PackId,
    pub display_name: LocalizedText,
    pub description: LocalizedText,
    pub pack_version: Version,
    pub scenario_versions: SchemaVersionDescriptor,
    pub icon_id: String,
    pub capabilities: BTreeSet<DomainCapability>,
    /// Canonical explanation kinds this pack can render. `Counterfactual` also gates temporary
    /// condition compilation.
    pub explanation_capabilities: BTreeSet<ExplanationCapability>,
    pub portable_versions: SchemaVersionDescriptor,
    /// Supported requirements across the declared portable versions. Each payload declares
    /// only the capabilities its own version actually requires.
    pub portable_capabilities: BTreeSet<SemanticCapability>,
    pub share_result_schema_version: u32,
    pub documentation_url: Option<String>,
    pub license: LicenseMetadata,
    /// True only for the synthetic conformance pack; applications must not present it as a real
    /// official domain.
    pub synthetic_test_only: bool,
}

/// Command risk classification exposed to UI and AI proposal review.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandRisk {
    OrdinaryMutation,
    DestructiveMutation,
}

/// How a command supplies undo information.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandReversibility {
    InverseCommand,
    Irreversible,
}

/// One generated pack command contract.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandDescriptor {
    pub id: String,
    pub title: LocalizedText,
    pub description: LocalizedText,
    pub risk: CommandRisk,
    pub reversibility: CommandReversibility,
    pub ai_grouping_allowed: bool,
    pub payload_schema: Value,
    pub result_schema: Value,
    pub change_schema: Value,
    pub valid_examples: Vec<Value>,
    pub invalid_examples: Vec<Value>,
}

/// Descriptor shared by setup, entities, rules, goals, provenance, and result views.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KindDescriptor {
    pub id: String,
    pub title: LocalizedText,
    pub description: LocalizedText,
}

/// Score category and direction metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreDescriptor {
    pub id: String,
    pub title: LocalizedText,
    pub minimize: bool,
}

/// Data-only import/export discovery entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferDescriptor {
    pub id: String,
    pub title: LocalizedText,
    pub schema_version: u32,
}

/// UI discovery data. It does not describe a schema-generated whole application.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainUiManifest {
    pub setup_steps: Vec<KindDescriptor>,
    pub entity_kinds: Vec<KindDescriptor>,
    pub rule_kinds: Vec<KindDescriptor>,
    pub goal_kinds: Vec<KindDescriptor>,
    pub score_kinds: Vec<ScoreDescriptor>,
    pub provenance_kinds: Vec<KindDescriptor>,
    pub result_views: Vec<KindDescriptor>,
    pub importers: Vec<TransferDescriptor>,
    pub exporters: Vec<TransferDescriptor>,
}

/// Typed AI tool discovery data; invocation still produces a normal reviewed command.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiToolDescriptor {
    pub command_id: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub valid_examples: Vec<Value>,
}

/// Complete deterministic catalog derived from one pack contract source.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainCatalog {
    pub pack_id: PackId,
    pub scenario_schema_version: u32,
    /// Pack-owned four-map document schema; versioned by `scenario_schema_version`.
    pub internal_schema: Value,
    pub portable_schema: Value,
    pub share_result_schema: Value,
    pub commands: Vec<CommandDescriptor>,
    pub ai_tools: Vec<AiToolDescriptor>,
    pub ui: DomainUiManifest,
}

impl DomainCatalog {
    /// Validates catalog completeness, uniqueness, schema vocabulary, examples, and AI links.
    ///
    /// # Errors
    /// Returns a stable contract error for the first inconsistency.
    pub fn validate(&self) -> Result<(), DomainPackError> {
        validate_schema_complete(&self.internal_schema, "internalSchema")?;
        if !eutheto_types::ScenarioDomain::has_schema_shape(&self.internal_schema) {
            return Err(DomainPackError::CatalogMismatch(
                "internalSchema must describe the four ScenarioDomain maps".to_owned(),
            ));
        }
        validate_schema_complete(&self.portable_schema, "portableSchema")?;
        validate_schema_complete(&self.share_result_schema, "shareResultSchema")?;
        unique_ids(
            self.commands.iter().map(|item| item.id.as_str()),
            "commands",
        )?;
        for command in &self.commands {
            validate_id(&command.id, "command")?;
            validate_localized(&command.title, "command title")?;
            validate_localized(&command.description, "command description")?;
            validate_schema_complete(&command.payload_schema, "command payload")?;
            validate_schema_complete(&command.result_schema, "command result")?;
            validate_schema_complete(&command.change_schema, "command change")?;
            if command.valid_examples.is_empty() || command.invalid_examples.is_empty() {
                return Err(DomainPackError::MissingMetadata(format!(
                    "command {} examples",
                    command.id
                )));
            }
            for example in &command.valid_examples {
                validate_contract_value(
                    &command.payload_schema,
                    example,
                    ContractJsonLimits::DEFAULT,
                )?;
            }
            if command.invalid_examples.iter().any(|example| {
                validate_contract_value(
                    &command.payload_schema,
                    example,
                    ContractJsonLimits::DEFAULT,
                )
                .is_ok()
            }) {
                return Err(DomainPackError::CatalogMismatch(format!(
                    "command {} has an accepted invalid example",
                    command.id
                )));
            }
        }
        unique_ids(
            self.ai_tools.iter().map(|item| item.name.as_str()),
            "AI tools",
        )?;
        for tool in &self.ai_tools {
            if tool.name.is_empty() || tool.description.is_empty() {
                return Err(DomainPackError::MissingMetadata("AI tool".to_owned()));
            }
            let command = self
                .commands
                .iter()
                .find(|command| command.id == tool.command_id)
                .ok_or_else(|| {
                    DomainPackError::CatalogMismatch(format!(
                        "AI tool {} references unknown command {}",
                        tool.name, tool.command_id
                    ))
                })?;
            if tool.input_schema != command.payload_schema
                || tool.valid_examples != command.valid_examples
            {
                return Err(DomainPackError::CatalogMismatch(format!(
                    "AI tool {} drifted from command {}",
                    tool.name, command.id
                )));
            }
        }
        self.ui.validate()?;
        Ok(())
    }

    /// Finds one exact namespaced command.
    #[must_use]
    pub fn command(&self, id: &str) -> Option<&CommandDescriptor> {
        self.commands.iter().find(|command| command.id == id)
    }
}

impl DomainUiManifest {
    /// Validates identities, ordering and localized metadata without claiming pack registration.
    ///
    /// Empty groups are permitted here; the caller enforces its implementation-stage contract.
    ///
    /// # Errors
    ///
    /// Returns the first invalid identity, ordering, text or transfer version.
    pub fn validate(&self) -> Result<(), DomainPackError> {
        validate_kind_group(&self.setup_steps, "setup steps")?;
        validate_kind_group(&self.entity_kinds, "entity kinds")?;
        validate_kind_group(&self.rule_kinds, "rule kinds")?;
        validate_kind_group(&self.goal_kinds, "goal kinds")?;
        validate_kind_group(&self.provenance_kinds, "provenance kinds")?;
        validate_kind_group(&self.result_views, "result views")?;
        unique_ids(
            self.score_kinds.iter().map(|item| item.id.as_str()),
            "score kinds",
        )?;
        for score in &self.score_kinds {
            validate_id(&score.id, "score")?;
            validate_localized(&score.title, "score title")?;
        }
        validate_transfers(&self.importers, "importers")?;
        validate_transfers(&self.exporters, "exporters")?;
        Ok(())
    }
}

/// Explicit pure compilation inputs. No ambient clock, locale, seed, or resource state is read.
#[derive(Clone, Debug)]
pub struct CompileContext {
    pub scenario_revision: u64,
    pub semantic_metadata: BTreeMap<String, String>,
    pub cancellation: CancellationToken,
    pub planning_limits: PlanningIrLimitsV1,
}

/// Immutable inputs for one bounded temporary counterfactual compilation.
///
/// This context is deliberately nonserializable. `budget` is a shared view of the caller's
/// original deadline and cancellation state; packs must not replace it with a fresh deadline.
#[derive(Clone)]
pub struct CounterfactualCompileContext<'a> {
    pub base_problem: &'a PlanningProblem,
    pub compile_context: &'a CompileContext,
    pub budget: SolveBudgetView,
}

/// Pack validation output in deterministic issue order.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainValidationReport {
    pub issues: Vec<ValidationIssue>,
}

/// Generic atomic batch envelope. Payload meaning remains pack-owned and typed at the pack seam.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainBatchCommand {
    pub schema_version: u32,
    pub pack_id: PackId,
    pub scenario_schema_version: u32,
    pub label: Option<String>,
    pub commands: Vec<DomainCommandEnvelope>,
}

impl DomainBatchCommand {
    /// Validates generic envelope version, size, count, label, and command identities before a
    /// pack decodes any typed payload.
    ///
    /// # Errors
    ///
    /// Returns an error when a version is unsupported, the batch or label exceeds its bound,
    /// the command list is empty, a command identity is invalid, or serialization fails.
    pub fn validate_bounds(&self) -> Result<(), DomainPackError> {
        self.validate_header()?;
        self.validate_encoded_size()
            .map_err(|_| DomainPackError::InvalidPayload {
                path: "/".to_owned(),
                message: "batch serialized size exceeds scenario limit".to_owned(),
            })
    }

    /// Validates a generated inverse, distinguishing a size limit from a malformed envelope.
    ///
    /// # Errors
    ///
    /// Returns [`DomainPackError::BatchInverseTooLarge`] only when a structurally valid
    /// inverse exceeds the existing pack-batch byte ceiling.
    pub fn validate_inverse_bounds(&self) -> Result<(), DomainPackError> {
        self.validate_header()?;
        self.validate_encoded_size()
            .map_err(|_| DomainPackError::BatchInverseTooLarge)
    }

    fn validate_header(&self) -> Result<(), DomainPackError> {
        if self.schema_version != DOMAIN_BATCH_SCHEMA_VERSION {
            return Err(DomainPackError::UnsupportedVersion(self.schema_version));
        }
        if self.scenario_schema_version == 0 {
            return Err(DomainPackError::UnsupportedVersion(0));
        }
        if self.commands.is_empty() || self.commands.len() > MAX_DOMAIN_BATCH_COMMANDS {
            return Err(DomainPackError::InvalidPayload {
                path: "/commands".to_owned(),
                message: format!("batch must contain 1..={MAX_DOMAIN_BATCH_COMMANDS} commands"),
            });
        }
        if self.label.as_ref().is_some_and(|label| label.len() > 1024) {
            return Err(DomainPackError::InvalidPayload {
                path: "/label".to_owned(),
                message: "batch label exceeds 1024 bytes".to_owned(),
            });
        }
        for command in &self.commands {
            validate_id(&command.command_type, "command")?;
        }
        Ok(())
    }

    fn validate_encoded_size(&self) -> Result<(), DomainPackError> {
        let limit = usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES).map_err(|error| {
            DomainPackError::InvalidPayload {
                path: "/".to_owned(),
                message: error.to_string(),
            }
        })?;
        bounded_json_size(self, limit).map(|_| ())
    }
}

/// One inert change attributed to an input command and checked against its change schema.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainChange {
    /// Zero-based input position; records are ordered by nondecreasing command index.
    pub command_index: u32,
    pub value: Value,
}

/// Atomic pure mutation with a complete reverse-order inverse batch.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainMutation {
    pub document: ScenarioDocument,
    pub results: Vec<Value>,
    pub changes: Vec<DomainChange>,
    pub inverse: DomainBatchCommand,
}

/// Import supplies an explicit scenario shell; packs cannot obtain persistence or host defaults.
#[derive(Clone, Debug)]
pub struct PortableImportContext {
    pub scenario_shell: ScenarioDocument,
}

/// Explicit privacy choices for an inert Share Result contribution.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareResultOptions {
    pub include_evidence_references: bool,
}

/// Pack-owned inert Share Result payload.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainShareResult {
    pub pack_id: PackId,
    pub schema_version: u32,
    pub payload: Value,
}

/// Data-only domain view. Values are inert JSON; pre-rendered HTML is prohibited by shared limits.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainView {
    pub view_id: String,
    pub data: Value,
}

/// Object-safe contract implemented by compiled-in packs.
pub trait DomainPack: Send + Sync {
    /// Returns this pack's descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error if the pack cannot produce its descriptor.
    fn descriptor(&self) -> Result<DomainPackDescriptor, DomainPackError>;

    /// Returns this pack's catalog.
    ///
    /// # Errors
    ///
    /// Returns an error if the pack cannot produce its catalog.
    fn catalog(&self) -> Result<DomainCatalog, DomainPackError>;

    /// Initializes a scenario document from the supplied shell.
    ///
    /// # Errors
    ///
    /// Returns an error if the shell is invalid or the pack cannot initialize the document.
    fn new_document(&self, shell: ScenarioDocument) -> Result<ScenarioDocument, DomainPackError>;

    /// Migrates a scenario document to the pack's current schema version.
    ///
    /// # Errors
    ///
    /// Returns an error if the document is invalid or its version cannot be migrated.
    fn migrate_document(
        &self,
        document: ScenarioDocument,
    ) -> Result<ScenarioDocument, DomainPackError>;

    fn validate_fast(&self, document: &ScenarioDocument) -> DomainValidationReport;
    fn validate_full(&self, document: &ScenarioDocument) -> DomainValidationReport;

    /// Applies an atomic command batch to a document.
    ///
    /// Every prefix must satisfy the pack's mutation validity rules. Host-owned document
    /// fields are immutable. Return one result and inverse per input command, ordered
    /// changes attributed by input index, and inverses in reverse execution order.
    /// Check cancellation between prefixes; failure leaves the caller's document unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid/unsupported input, bounded output failure or cancellation.
    fn apply_batch(
        &self,
        document: &ScenarioDocument,
        batch: &DomainBatchCommand,
        cancellation: &CancellationToken,
    ) -> Result<DomainMutation, DomainPackError>;

    /// Compiles a scenario document into a planning problem.
    ///
    /// # Errors
    ///
    /// Returns an error if the document or context is invalid, compilation fails, or the
    /// operation is cancelled.
    fn compile(
        &self,
        document: &ScenarioDocument,
        context: &CompileContext,
    ) -> Result<PlanningProblem, DomainPackError>;

    /// Projects candidate values into a normalized domain solution.
    ///
    /// # Errors
    ///
    /// Returns an error if the planning problem or candidate values violate the pack contract.
    fn project(
        &self,
        problem: &PlanningProblem,
        candidate: &CandidateValues,
        solution_id: SolutionId,
    ) -> Result<NormalizedSolution, DomainPackError>;

    /// Declares the required-rule identities and semantic bindings for a scenario revision.
    ///
    /// # Errors
    ///
    /// Returns an error if the document is invalid or its verification scope cannot be built.
    fn verification_scope(
        &self,
        document: &ScenarioDocument,
        scenario_revision: u64,
    ) -> Result<VerificationScope, DomainPackError>;

    /// Verifies required rules against a normalized solution and embeds the separately
    /// recomputed authoritative score without deriving rule outcomes from it.
    ///
    /// # Errors
    ///
    /// Returns an error if either input is invalid or the pack cannot perform verification.
    fn verify(
        &self,
        document: &ScenarioDocument,
        solution: &NormalizedSolution,
        context: &VerificationContextV1,
        authoritative_score: &ScoreVector,
    ) -> Result<VerificationReport, DomainPackError>;

    /// Computes the pack-defined score vector for a solution.
    ///
    /// # Errors
    ///
    /// Returns an error if either input is invalid or scoring violates the pack contract.
    fn score(
        &self,
        document: &ScenarioDocument,
        solution: &NormalizedSolution,
    ) -> Result<ScoreVector, DomainPackError>;

    /// Exports a scenario document as the pack's current portable representation.
    ///
    /// # Errors
    ///
    /// Returns an error if the document is invalid or cannot be represented portably.
    fn export_portable(
        &self,
        document: &ScenarioDocument,
    ) -> Result<PortableDomainDocument, DomainPackError>;

    /// Migrates a historical portable document by exactly one schema version.
    ///
    /// # Errors
    ///
    /// Returns an error if the document is invalid or its next version cannot be produced.
    /// The returned envelope and payload must agree on the next version; callers own the
    /// ordered migration loop and record one provenance entry for each successful step.
    fn migrate_portable_step(
        &self,
        document: PortableDomainDocument,
    ) -> Result<PortableDomainDocument, DomainPackError>;

    /// Imports a current portable document into the supplied scenario context.
    ///
    /// # Errors
    ///
    /// Returns an error if the portable document or import context is invalid or incompatible.
    fn import_portable(
        &self,
        document: &PortableDomainDocument,
        context: &PortableImportContext,
    ) -> Result<ScenarioDocument, DomainPackError>;

    /// Builds the pack-owned contribution to an inert share result.
    ///
    /// # Errors
    ///
    /// Returns an error if the document, accepted result, or requested options are invalid.
    fn build_share_result(
        &self,
        document: &ScenarioDocument,
        accepted: &AcceptedResult,
        options: ShareResultOptions,
    ) -> Result<DomainShareResult, DomainPackError>;

    /// Builds the requested data-only domain view.
    ///
    /// # Errors
    ///
    /// Returns an error if the document, optional solution, or view identifier is invalid.
    fn build_view(
        &self,
        document: &ScenarioDocument,
        solution: Option<&NormalizedSolution>,
        view_id: &str,
    ) -> Result<DomainView, DomainPackError>;

    /// Renders validated typed evidence as inert localization messages.
    ///
    /// The request kind must be declared in `descriptor().explanation_capabilities`. Implementors
    /// must validate the document and request and must return
    /// [`DomainPackError::UnsupportedExplanationCapability`] when the capability is absent.
    ///
    /// # Errors
    ///
    /// Returns an error if the document or evidence request is invalid or unsupported.
    fn render_evidence(
        &self,
        document: &ScenarioDocument,
        request: &EvidenceRenderRequestV1,
    ) -> Result<EvidenceRenderResultV1, DomainPackError>;

    /// Recompiles a validated temporary condition against an exact baseline model.
    ///
    /// `Counterfactual` must be declared in `descriptor().explanation_capabilities`. Implementors
    /// must observe the shared budget before and during work, preserve baseline semantics except
    /// for additive condition metadata, provenance, and constraints, and reject a recompiled
    /// baseline whose canonical hash differs from `context.base_problem`.
    ///
    /// # Errors
    ///
    /// Returns [`DomainPackError::Cancelled`] or [`DomainPackError::BudgetExpired`] when observed,
    /// [`DomainPackError::UnsupportedExplanationCapability`] when unsupported, or another typed
    /// pack error when the document, condition, baseline, or derived model is invalid.
    fn compile_counterfactual(
        &self,
        document: &ScenarioDocument,
        condition: &CounterfactualConditionV1,
        context: &CounterfactualCompileContext<'_>,
    ) -> Result<PlanningProblem, DomainPackError>;
}

/// A borrowed command whose catalog schemas were checked during registry construction.
#[derive(Clone, Copy)]
pub struct RegisteredCommand<'a> {
    descriptor: &'a CommandDescriptor,
}

impl RegisteredCommand<'_> {
    /// Checks an input without rechecking or copying the registered schema.
    ///
    /// # Errors
    /// Returns a contract error for unsafe, oversized or schema-invalid data.
    pub fn validate_payload(&self, value: &Value) -> Result<(), DomainPackError> {
        schema::validate_value_with_schema(
            &self.descriptor.payload_schema,
            value,
            ContractJsonLimits::DEFAULT,
        )
    }

    /// Checks the corresponding command result.
    ///
    /// # Errors
    /// Returns a contract error for unsafe, oversized or schema-invalid data.
    pub fn validate_result(&self, value: &Value) -> Result<(), DomainPackError> {
        schema::validate_value_with_schema(
            &self.descriptor.result_schema,
            value,
            ContractJsonLimits::DEFAULT,
        )
    }

    /// Checks one change value attributed to this input command.
    ///
    /// # Errors
    /// Returns a contract error for unsafe, oversized or schema-invalid data.
    pub fn validate_change(&self, value: &Value) -> Result<(), DomainPackError> {
        schema::validate_value_with_schema(
            &self.descriptor.change_schema,
            value,
            ContractJsonLimits::DEFAULT,
        )
    }
}

struct Registration {
    descriptor: DomainPackDescriptor,
    catalog: DomainCatalog,
    pack: Arc<dyn DomainPack>,
}

/// Validated deterministic compiled-in registry.
pub struct DomainPackRegistry {
    registrations: BTreeMap<PackId, Registration>,
}

impl DomainPackRegistry {
    #[must_use]
    pub fn builder() -> DomainPackRegistryBuilder {
        DomainPackRegistryBuilder { packs: Vec::new() }
    }

    /// Descriptors ordered by stable pack ID.
    #[must_use]
    pub fn descriptors(&self) -> impl ExactSizeIterator<Item = &DomainPackDescriptor> {
        self.registrations.values().map(|entry| &entry.descriptor)
    }

    /// Returns the registered implementation, or an exact unavailable error.
    ///
    /// # Errors
    ///
    /// Returns [`DomainPackError::PackUnavailable`] if no pack is registered under `id`.
    pub fn require(&self, id: &PackId) -> Result<&dyn DomainPack, DomainPackError> {
        self.registrations
            .get(id)
            .map(|entry| entry.pack.as_ref())
            .ok_or_else(|| DomainPackError::PackUnavailable(id.to_string()))
    }

    #[must_use]
    pub fn catalog(&self, id: &PackId) -> Option<&DomainCatalog> {
        self.registrations.get(id).map(|entry| &entry.catalog)
    }

    /// Resolves an exact command in the current registered pack catalog.
    ///
    /// # Errors
    /// Rejects malformed identities, unavailable packs and unknown commands.
    pub fn command(
        &self,
        pack_id: &PackId,
        command_id: &str,
    ) -> Result<RegisteredCommand<'_>, DomainPackError> {
        validate_id(command_id, "command")?;
        let catalog = self
            .catalog(pack_id)
            .ok_or_else(|| DomainPackError::PackUnavailable(pack_id.to_string()))?;
        let index = catalog
            .commands
            .binary_search_by(|command| command.id.as_str().cmp(command_id))
            .map_err(|_| DomainPackError::UnknownCommand(command_id.to_owned()))?;
        Ok(RegisteredCommand {
            descriptor: &catalog.commands[index],
        })
    }
}

/// Builder preserves duplicates until validation so no registration is silently replaced.
pub struct DomainPackRegistryBuilder {
    packs: Vec<Arc<dyn DomainPack>>,
}

impl DomainPackRegistryBuilder {
    #[must_use]
    pub fn register<P>(mut self, pack: P) -> Self
    where
        P: DomainPack + 'static,
    {
        self.packs.push(Arc::new(pack));
        self
    }

    /// Validates all descriptors/catalogs and rejects duplicate identities.
    ///
    /// # Errors
    ///
    /// Returns an error if a pack cannot supply its metadata, a descriptor or catalog is invalid,
    /// their declarations disagree, or two packs have the same identity.
    pub fn build(self) -> Result<DomainPackRegistry, DomainPackError> {
        let mut registrations = BTreeMap::new();
        for pack in self.packs {
            let descriptor = pack.descriptor()?;
            descriptor.validate()?;
            let catalog = pack.catalog()?;
            catalog.validate()?;
            if catalog.pack_id != descriptor.id
                || catalog.scenario_schema_version != descriptor.scenario_versions.latest
                || schema_version(&catalog.portable_schema)
                    != Some(u64::from(descriptor.portable_versions.latest))
                || schema_version(&catalog.share_result_schema)
                    != Some(u64::from(descriptor.share_result_schema_version))
                || (descriptor
                    .capabilities
                    .contains(&DomainCapability::Commands)
                    && catalog.commands.is_empty())
                || (descriptor.capabilities.contains(&DomainCapability::AiTools)
                    && catalog.ai_tools.is_empty())
                || (descriptor
                    .capabilities
                    .contains(&DomainCapability::UiManifest)
                    && catalog.ui.result_views.is_empty())
            {
                return Err(DomainPackError::CatalogMismatch(descriptor.id.to_string()));
            }
            let id = descriptor.id.clone();
            if registrations.contains_key(&id) {
                return Err(DomainPackError::DuplicatePack(id.to_string()));
            }
            registrations.insert(
                id,
                Registration {
                    descriptor,
                    catalog,
                    pack,
                },
            );
        }
        Ok(DomainPackRegistry { registrations })
    }
}

impl DomainPackDescriptor {
    /// Validates all metadata and compatibility declarations.
    ///
    /// # Errors
    ///
    /// Returns an error if required metadata is missing, a version declaration is invalid, or
    /// declared capabilities are inconsistent.
    pub fn validate(&self) -> Result<(), DomainPackError> {
        validate_localized(&self.display_name, "pack display name")?;
        validate_localized(&self.description, "pack description")?;
        if self.pack_version.major == 0
            && self.pack_version.minor == 0
            && self.pack_version.patch == 0
        {
            return Err(DomainPackError::InvalidVersion(
                "pack version 0.0.0".to_owned(),
            ));
        }
        if !self.scenario_versions.has_complete_migration_paths()
            || self.scenario_versions.migratable_from.contains(&0)
            || !self.portable_versions.has_complete_migration_paths()
            || self.share_result_schema_version == 0
        {
            return Err(DomainPackError::InvalidVersion(self.id.to_string()));
        }
        for capability in &self.portable_capabilities {
            validate_id(&capability.id, "portable capability")?;
            if capability.version == 0 {
                return Err(DomainPackError::InvalidVersion(capability.id.clone()));
            }
        }
        if self.icon_id.is_empty()
            || self.capabilities.is_empty()
            || self.license.spdx_expression.is_empty()
            || self.license.attribution.is_empty()
        {
            return Err(DomainPackError::MissingMetadata(self.id.to_string()));
        }
        if self.capabilities.contains(&DomainCapability::PortableData)
            && self.portable_capabilities.is_empty()
        {
            return Err(DomainPackError::CatalogMismatch(format!(
                "{} portable capabilities",
                self.id
            )));
        }
        Ok(())
    }
}

/// Stable domain-pack boundary failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DomainPackError {
    #[error("duplicate domain pack {0}")]
    DuplicatePack(String),
    #[error("domain pack {0} is unavailable")]
    PackUnavailable(String),
    #[error("missing domain-pack metadata: {0}")]
    MissingMetadata(String),
    #[error("invalid domain-pack version: {0}")]
    InvalidVersion(String),
    #[error("domain catalog mismatch: {0}")]
    CatalogMismatch(String),
    #[error("unsupported scenario or payload version {0}")]
    UnsupportedVersion(u32),
    #[error("unknown command {0}")]
    UnknownCommand(String),
    #[error("invalid domain payload at {path}: {message}")]
    InvalidPayload { path: String, message: String },
    #[error("domain batch inverse exceeds its byte bound")]
    BatchInverseTooLarge,
    #[error("domain mutation output exceeds its bound")]
    MutationOutputLimit,
    #[error("domain operation was cancelled")]
    Cancelled,
    #[error("domain operation budget expired")]
    BudgetExpired,
    #[error("unsupported explanation capability {0:?}")]
    UnsupportedExplanationCapability(ExplanationCapability),
    #[error("domain contract violation: {0}")]
    Contract(String),
}

fn schema_version(schema: &Value) -> Option<u64> {
    schema
        .get("properties")?
        .get("schemaVersion")?
        .get("const")?
        .as_u64()
}

fn validate_schema_complete(schema: &Value, name: &str) -> Result<(), DomainPackError> {
    validate_contract_schema(schema).map_err(|error| DomainPackError::InvalidPayload {
        path: name.to_owned(),
        message: error.to_string(),
    })
}

fn validate_localized(value: &LocalizedText, name: &str) -> Result<(), DomainPackError> {
    if value.key.is_empty() || value.default_text.is_empty() {
        Err(DomainPackError::MissingMetadata(name.to_owned()))
    } else {
        Ok(())
    }
}

fn validate_id(value: &str, kind: &str) -> Result<(), DomainPackError> {
    let valid = value.len() <= 160
        && value.split('.').count() >= 2
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-')
                })
        });
    if valid {
        Ok(())
    } else {
        Err(DomainPackError::MissingMetadata(format!("{kind} id")))
    }
}

fn unique_ids<'a>(
    values: impl Iterator<Item = &'a str>,
    kind: &str,
) -> Result<(), DomainPackError> {
    let mut seen = BTreeSet::new();
    let mut previous = None;
    for value in values {
        if !seen.insert(value) {
            return Err(DomainPackError::CatalogMismatch(format!(
                "duplicate {kind} id {value}"
            )));
        }
        if previous.is_some_and(|prior| prior >= value) {
            return Err(DomainPackError::CatalogMismatch(format!(
                "{kind} are not in stable ascending ID order"
            )));
        }
        previous = Some(value);
    }
    Ok(())
}

fn validate_kind_group(values: &[KindDescriptor], name: &str) -> Result<(), DomainPackError> {
    unique_ids(values.iter().map(|item| item.id.as_str()), name)?;
    for value in values {
        validate_id(&value.id, name)?;
        validate_localized(&value.title, name)?;
        validate_localized(&value.description, name)?;
    }
    Ok(())
}

fn validate_transfers(values: &[TransferDescriptor], name: &str) -> Result<(), DomainPackError> {
    unique_ids(values.iter().map(|item| item.id.as_str()), name)?;
    for value in values {
        validate_id(&value.id, name)?;
        validate_localized(&value.title, name)?;
        if value.schema_version == 0 {
            return Err(DomainPackError::InvalidVersion(value.id.clone()));
        }
    }
    Ok(())
}
