//! Instance-owned gates around real pack behavior, shared by setup and accepted-view races.
use eutheto_domain_api::{
    CompileContext, CounterfactualCompileContext, DomainBatchCommand, DomainCatalog,
    DomainMutation, DomainPack, DomainPackDescriptor, DomainPackError, DomainSettingsMutation,
    DomainShareResult, DomainValidationReport, DomainViewInput, DomainViewOutput,
    PortableImportContext, ShareResultOptions,
};
use eutheto_domain_ir::{
    AcceptedResult, CounterfactualConditionV1, EvidenceRenderRequestV1, EvidenceRenderResultV1,
    NormalizedSolution, ScoreVector, VerificationContextV1, VerificationReport, VerificationScope,
};
use eutheto_planning_ir::{CandidateValues, PlanningProblem};
use eutheto_types::{
    CancellationToken, OperationControl, PortableDomainDocument, ScenarioDocument,
    ScenarioSettings, SolutionId,
};
use serde_json::Value;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use tokio::sync::Notify;

#[repr(u8)]
#[derive(Clone, Copy)]
pub enum PauseAt {
    Settings,
    FullValidation,
    View,
}

#[derive(Default)]
pub struct PreparationGate {
    pub armed: AtomicBool,
    pub pause_at: AtomicU8,
    pub full_report: Mutex<Option<DomainValidationReport>>,
    pub entered: Notify,
    released: Mutex<bool>,
    wake: Condvar,
}

impl PreparationGate {
    fn pause(&self, at: PauseAt) -> Result<(), DomainPackError> {
        if self.pause_at.load(Ordering::SeqCst) != at as u8
            || !self.armed.swap(false, Ordering::SeqCst)
        {
            return Ok(());
        }
        let mut released = self
            .released
            .lock()
            .map_err(|_| DomainPackError::Cancelled)?;
        self.entered.notify_one();
        while !*released {
            released = self
                .wake
                .wait(released)
                .map_err(|_| DomainPackError::Cancelled)?;
        }
        Ok(())
    }
    pub fn release(&self) {
        if let Ok(mut released) = self.released.lock() {
            *released = true;
            self.wake.notify_all();
        }
    }
}

pub struct ReleaseOnDrop(pub Arc<PreparationGate>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

pub struct ControlledPack<P> {
    pub pack: P,
    pub gate: Arc<PreparationGate>,
}
macro_rules! forward {
    ($name:ident($($argument:ident: $kind:ty),*) -> $result:ty) => {
        fn $name(&self, $($argument: $kind),*) -> $result { self.pack.$name($($argument),*) }
    };
}
impl<P: DomainPack> DomainPack for ControlledPack<P> {
    forward!(descriptor() -> Result<DomainPackDescriptor, DomainPackError>);
    forward!(catalog() -> Result<DomainCatalog, DomainPackError>);
    forward!(new_document(shell: ScenarioDocument) -> Result<ScenarioDocument, DomainPackError>);
    forward!(migrate_document(document: ScenarioDocument) -> Result<ScenarioDocument, DomainPackError>);
    forward!(validate_fast(document: &ScenarioDocument) -> DomainValidationReport);
    fn validate_full(
        &self,
        document: &ScenarioDocument,
        control: &OperationControl,
    ) -> Result<DomainValidationReport, DomainPackError> {
        self.gate.pause(PauseAt::FullValidation)?;
        if let Some(report) = self
            .gate
            .full_report
            .lock()
            .map_err(|_| DomainPackError::Cancelled)?
            .take()
        {
            return Ok(report);
        }
        self.pack.validate_full(document, control)
    }
    forward!(apply_batch(document: &ScenarioDocument, batch: &DomainBatchCommand, cancellation: &CancellationToken) -> Result<DomainMutation, DomainPackError>);
    fn reconcile_settings(
        &self,
        original: &ScenarioDocument,
        settings: &ScenarioSettings,
        restoration: Option<&Value>,
        control: &OperationControl,
    ) -> Result<DomainSettingsMutation, DomainPackError> {
        self.gate.pause(PauseAt::Settings)?;
        self.pack
            .reconcile_settings(original, settings, restoration, control)
    }
    forward!(compile(document: &ScenarioDocument, context: &CompileContext) -> Result<PlanningProblem, DomainPackError>);
    forward!(project(problem: &PlanningProblem, candidate: &CandidateValues, solution_id: SolutionId, control: &OperationControl) -> Result<NormalizedSolution, DomainPackError>);
    forward!(verification_scope(document: &ScenarioDocument, scenario_revision: u64, control: &OperationControl) -> Result<VerificationScope, DomainPackError>);
    forward!(verify(document: &ScenarioDocument, solution: &NormalizedSolution, context: &VerificationContextV1, authoritative_score: &ScoreVector, control: &OperationControl) -> Result<VerificationReport, DomainPackError>);
    forward!(score(document: &ScenarioDocument, solution: &NormalizedSolution, control: &OperationControl) -> Result<ScoreVector, DomainPackError>);
    forward!(export_portable(document: &ScenarioDocument) -> Result<PortableDomainDocument, DomainPackError>);
    forward!(migrate_portable_step(document: PortableDomainDocument) -> Result<PortableDomainDocument, DomainPackError>);
    forward!(import_portable(document: &PortableDomainDocument, context: &PortableImportContext) -> Result<ScenarioDocument, DomainPackError>);
    forward!(build_share_result(document: &ScenarioDocument, accepted: &AcceptedResult, options: ShareResultOptions) -> Result<DomainShareResult, DomainPackError>);
    fn build_view(
        &self,
        input: DomainViewInput<'_>,
        control: &OperationControl,
    ) -> Result<DomainViewOutput, DomainPackError> {
        self.gate.pause(PauseAt::View)?;
        self.pack.build_view(input, control)
    }
    forward!(render_evidence(document: &ScenarioDocument, request: &EvidenceRenderRequestV1) -> Result<EvidenceRenderResultV1, DomainPackError>);
    forward!(compile_counterfactual(document: &ScenarioDocument, condition: &CounterfactualConditionV1, context: &CounterfactualCompileContext<'_>) -> Result<PlanningProblem, DomainPackError>);
}
