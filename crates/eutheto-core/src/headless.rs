use super::{
    application_portable_migrations, assignment_evidence, available_pack, command_store_error,
    decode_portable_domain, domain_interruption, encode_portable_domain, ensure_supported_document,
    explanation_render_error, export_error, import_error, inspect_application_bundle,
    operation_interrupted, project_initialization_error, reverify_portable_solution,
    solution_contract_error, store_error, unsupported_project_pack, validated_static_pack_registry,
    validated_static_registries, validation_error,
};
use eutheto_command::{apply_command_with_registry, validate_document_shape};
use eutheto_domain_api::{DomainPack, DomainPackRegistry, DomainValidationReport};
use eutheto_domain_ir::{
    DomainAssignmentId, EvidenceRenderRequestV1, ExplanationEvidencePayloadV1,
    ExplanationEvidenceV1, ExplanationResultV1, PortableAcceptedResultV2, VerificationReport,
};
use eutheto_export::{BundleKind, PortableScenario};
use eutheto_import::{
    InspectedBundle, InspectedScenario, InspectionPolicy, inspect_scenario,
    validate_standalone_scenario,
};
use eutheto_solver_api::SolverRegistry;
use eutheto_types::{
    AppError, CancellationToken, Clock, CommandEnvelope, CommandResult, DomainPackRef, IdGenerator,
    MonotonicClock, OperationControl, Revision, ScenarioDocument, ScenarioDomain, ScenarioId,
    ScenarioMetadata, ScenarioSettings, ScenarioSnapshotV1, ValidationSeverity,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[path = "headless_solve.rs"]
mod solve;
pub use solve::*;

/// Database-independent scenario operations. All identity, time and pack authority is injected.
#[derive(Clone)]
pub struct HeadlessService {
    pub(super) packs: Arc<DomainPackRegistry>,
    pub(super) solvers: Arc<SolverRegistry>,
    pub(super) monotonic_clock: Arc<dyn MonotonicClock>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) ids: Arc<dyn IdGenerator>,
}

pub struct AppliedScenarioSnapshot {
    pub snapshot: ScenarioSnapshotV1,
    pub result: CommandResult,
}

impl HeadlessService {
    /// Uses the same validated compiled pack registry as the durable application.
    ///
    /// # Errors
    /// Returns a safe startup error if compiled pack metadata is inconsistent.
    pub fn new(
        clock: Arc<dyn Clock>,
        monotonic_clock: Arc<dyn MonotonicClock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Result<Self, AppError> {
        let (packs, solvers) = validated_static_registries()?;
        Ok(Self {
            packs,
            solvers,
            monotonic_clock,
            clock,
            ids,
        })
    }

    /// Uses caller-assembled verified backend registrations without discovering a worker path.
    ///
    /// # Errors
    /// Returns the same compiled pack startup error as [`Self::new`].
    pub fn with_solver_registry(
        clock: Arc<dyn Clock>,
        monotonic_clock: Arc<dyn MonotonicClock>,
        ids: Arc<dyn IdGenerator>,
        solvers: SolverRegistry,
    ) -> Result<Self, AppError> {
        Ok(Self {
            packs: Arc::new(validated_static_pack_registry()?),
            solvers: Arc::new(solvers),
            monotonic_clock,
            clock,
            ids,
        })
    }

    /// Creates a new independent scenario without opening a library or allocating database rows.
    ///
    /// # Errors
    /// Rejects invalid settings/pack requests, identity failure and interrupted pack initialization.
    pub fn create_scenario(
        &self,
        title: String,
        description: String,
        domain_pack: DomainPackRef,
        settings: ScenarioSettings,
        control: &OperationControl,
    ) -> Result<ScenarioSnapshotV1, AppError> {
        control.check().map_err(operation_interrupted)?;
        let pack = creation_pack(&title, &domain_pack, &self.packs)?;
        let scenario_id =
            ScenarioId::new(self.ids.as_ref()).map_err(|_| solution_contract_error())?;
        let now = self.clock.now();
        let shell = ScenarioDocument::new(
            scenario_id,
            domain_pack,
            ScenarioMetadata {
                title,
                description,
                created_at: now,
                updated_at: now,
            },
            settings,
            ScenarioDomain::default(),
            BTreeMap::new(),
        );
        let document = initialize_document(&shell, pack, control)?;
        let snapshot =
            ScenarioSnapshotV1::current(Revision::INITIAL, document, BTreeSet::default());
        validate_standalone_scenario(&snapshot, &InspectionPolicy::default())
            .map_err(|error| import_error(&error))?;
        Ok(snapshot)
    }

    /// Decodes checked portable JSON under explicit operation control.
    ///
    /// # Errors
    /// Returns sanitized ingress, pack conversion or interruption errors.
    pub fn decode_scenario(
        &self,
        bytes: &[u8],
        control: &OperationControl,
    ) -> Result<InspectedScenario, AppError> {
        control.check().map_err(operation_interrupted)?;
        let migrations =
            application_portable_migrations(&self.packs).map_err(|error| import_error(&error))?;
        let inspected =
            inspect_scenario(bytes, &InspectionPolicy::default(), &migrations, &|wire| {
                decode_portable_domain(wire, &self.packs)
            })
            .map_err(|error| import_error(&error))?;
        control.check().map_err(operation_interrupted)?;
        Ok(inspected)
    }

    /// Inspects one read-only scenario export while retaining all supplemental payload custody.
    ///
    /// # Errors
    /// Rejects full backups, ambiguous scenario selection, unsafe bundles and interrupted work.
    pub fn decode_scenario_bundle(
        &self,
        bytes: &[u8],
        control: &OperationControl,
    ) -> Result<InspectedBundle, AppError> {
        control.check().map_err(operation_interrupted)?;
        let inspected =
            inspect_application_bundle(bytes, &self.packs).map_err(|error| import_error(&error))?;
        if inspected.manifest.bundle_kind != BundleKind::ScenarioExport
            || inspected.scenarios.len() != 1
        {
            return Err(validation_error(
                "scenario.single_export_required",
                "/bundle",
                "A single-scenario export is required; library backups are not standalone solve inputs.",
            ));
        }
        control.check().map_err(operation_interrupted)?;
        Ok(inspected)
    }

    /// Encodes a current standalone snapshot through its registered pack, preserving the wrapper.
    ///
    /// # Errors
    /// Rejects unsupported domains, invalid dependency closure, transformed bounds and interruption.
    pub fn encode_scenario(
        &self,
        snapshot: &ScenarioSnapshotV1,
        control: &OperationControl,
    ) -> Result<Vec<u8>, AppError> {
        control.check().map_err(operation_interrupted)?;
        ensure_supported_document(&snapshot.document, &self.packs).map_err(store_error)?;
        validate_standalone_scenario(snapshot, &InspectionPolicy::default())
            .map_err(|error| import_error(&error))?;
        let domain = encode_portable_domain(&snapshot.document, &self.packs)
            .map_err(|error| export_error(&error))?;
        let wire = PortableScenario::from_snapshot(snapshot, domain)
            .map_err(|error| export_error(&error))?;
        let bytes = eutheto_export::canonical_json(&wire).map_err(|error| export_error(&error))?;
        if u64::try_from(bytes.len()).map_or(true, |length| {
            length > eutheto_export::PORTABLE_LIMITS.max_json_bytes
        }) {
            return Err(solution_contract_error());
        }
        control.check().map_err(operation_interrupted)?;
        Ok(bytes)
    }

    /// Applies one typed command/batch to an owned snapshot, without journaling or publication.
    ///
    /// # Errors
    /// Rejects stale revisions, unsupported domains, invalid commands and cancellation atomically.
    pub fn apply_scenario(
        &self,
        mut snapshot: ScenarioSnapshotV1,
        envelope: &CommandEnvelope,
        cancellation: &CancellationToken,
    ) -> Result<AppliedScenarioSnapshot, AppError> {
        let control = OperationControl::Cancellation(cancellation.clone());
        control.check().map_err(operation_interrupted)?;
        validate_standalone_scenario(&snapshot, &InspectionPolicy::default())
            .map_err(|error| import_error(&error))?;
        ensure_supported_document(&snapshot.document, &self.packs).map_err(store_error)?;
        let mut applied = apply_command_with_registry(
            &snapshot.document,
            snapshot.revision,
            envelope,
            &self.packs,
            cancellation,
        )
        .map_err(|error| store_error(command_store_error(&error)))?;
        applied.document.metadata.updated_at = self.clock.now();
        snapshot.document = applied.document;
        snapshot.revision = applied.result.new_revision;
        validate_standalone_scenario(&snapshot, &InspectionPolicy::default())
            .map_err(|error| import_error(&error))?;
        control.check().map_err(operation_interrupted)?;
        Ok(AppliedScenarioSnapshot {
            snapshot,
            result: applied.result,
        })
    }

    /// Computes full readiness without interpreting contradiction findings as compilation failure.
    ///
    /// # Errors
    /// Rejects unsupported/malformed documents and interrupted validation.
    pub fn validate_full(
        &self,
        snapshot: &ScenarioSnapshotV1,
        control: &OperationControl,
    ) -> Result<DomainValidationReport, AppError> {
        control.check().map_err(operation_interrupted)?;
        eutheto_export::validate_scenario_snapshot(snapshot)
            .map_err(|_| project_initialization_error())?;
        ensure_supported_document(&snapshot.document, &self.packs).map_err(store_error)?;
        let pack = self
            .packs
            .require(&snapshot.document.domain_pack.id)
            .map_err(|_| project_initialization_error())?;
        let report = pack
            .validate_full(&snapshot.document, control)
            .map_err(|error| {
                domain_interruption(&error).unwrap_or_else(project_initialization_error)
            })?;
        control.check().map_err(operation_interrupted)?;
        Ok(report)
    }

    /// Freshly verifies an external accepted record against the exact supplied revision.
    ///
    /// This proves current original-domain feasibility and score only; source backend proof,
    /// historical timing and arbitrary evidence-map values remain inert archival metadata.
    ///
    /// # Errors
    /// Rejects malformed records, revision/document mismatches, forged score/report and interruption.
    pub fn verify_result(
        &self,
        snapshot: &ScenarioSnapshotV1,
        portable: &PortableAcceptedResultV2,
        control: &OperationControl,
    ) -> Result<VerificationReport, AppError> {
        eutheto_export::validate_scenario_snapshot(snapshot)
            .map_err(|_| solution_contract_error())?;
        reverify_portable_solution(
            &snapshot.document,
            snapshot.revision.value(),
            portable,
            &self.packs,
            control,
        )
    }

    /// Renders assignment explanation only after fresh verification and pack evidence validation.
    ///
    /// # Errors
    /// Rejects unverified records, unknown assignments, unsupported evidence and interruption.
    pub fn explain_assignment(
        &self,
        snapshot: &ScenarioSnapshotV1,
        portable: &PortableAcceptedResultV2,
        assignment_id: &DomainAssignmentId,
        control: &OperationControl,
    ) -> Result<ExplanationResultV1, AppError> {
        self.verify_result(snapshot, portable, control)?;
        let assignment = assignment_evidence(&portable.accepted_result, assignment_id)?;
        let evidence =
            ExplanationEvidenceV1::new(ExplanationEvidencePayloadV1::Assignment { assignment })
                .map_err(|_| solution_contract_error())?;
        let request = EvidenceRenderRequestV1::new(evidence.clone())
            .map_err(|_| solution_contract_error())?;
        let pack = available_pack(&snapshot.document, &self.packs)
            .ok_or_else(|| unsupported_project_pack(&snapshot.document.domain_pack))?;
        let rendered = pack
            .render_evidence(&snapshot.document, &request)
            .map_err(|error| explanation_render_error(&error))?;
        control.check().map_err(operation_interrupted)?;
        ExplanationResultV1::new(evidence, rendered).map_err(|_| solution_contract_error())
    }
}

pub(super) fn creation_pack<'a>(
    title: &str,
    domain_pack: &DomainPackRef,
    registry: &'a DomainPackRegistry,
) -> Result<&'a dyn DomainPack, AppError> {
    if title.trim().is_empty() {
        return Err(validation_error(
            "project.title_required",
            "/title",
            "Project title must not be empty.",
        ));
    }
    let descriptor = registry
        .descriptors()
        .find(|descriptor| descriptor.id == domain_pack.id)
        .ok_or_else(|| unsupported_project_pack(domain_pack))?;
    if domain_pack.schema_version != descriptor.scenario_versions.latest {
        return Err(unsupported_project_pack(domain_pack));
    }
    registry
        .require(&domain_pack.id)
        .map_err(|_| project_initialization_error())
}

pub(super) fn initialize_document(
    shell: &ScenarioDocument,
    pack: &dyn DomainPack,
    control: &OperationControl,
) -> Result<ScenarioDocument, AppError> {
    control.check().map_err(operation_interrupted)?;
    let document = pack
        .new_document(shell.clone())
        .map_err(|_| project_initialization_error())?;
    if document.format != shell.format
        || document.format_version != shell.format_version
        || document.scenario_id != shell.scenario_id
        || document.domain_pack != shell.domain_pack
        || document.metadata != shell.metadata
        || document.settings != shell.settings
        || document.extensions != shell.extensions
    {
        return Err(project_initialization_error());
    }
    validate_document_shape(&document).map_err(|_| project_initialization_error())?;
    if pack
        .validate_full(&document, control)
        .map_err(|error| domain_interruption(&error).unwrap_or_else(project_initialization_error))?
        .issues
        .iter()
        .any(|issue| issue.severity == ValidationSeverity::Error)
    {
        return Err(project_initialization_error());
    }
    control.check().map_err(operation_interrupted)?;
    Ok(document)
}
