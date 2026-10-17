//! Retained source allocations pinned by recipient publication handles, not process RSS.

use super::{operation_interrupted, resource_limit_error};
use eutheto_domain_ir::{
    AcceptedResultRefV1, DomainEntityRef, MAX_DOMAIN_ID_BYTES, MAX_RUN_DIAGNOSTIC_CODE_BYTES,
    MAX_RUN_VERSION_BYTES, MAX_SCORE_LEVELS, RunBaselineBinding, RunBaselineOriginal,
    RunBaselineSelection, RunExternalBaselineBinding, RunInput, RunNativeAttempt,
    RunNativeAttemptTermination, RunNativeObjective, RunNativeObservations, RunNativeOutcome,
    RunNativeRuntime, RunObjectiveContext,
};
use eutheto_types::{AppError, BackendSelection, OperationControl, SolveOptions};
use serde_json::Value;
use std::alloc::Layout;
use std::mem::size_of;

// These feature changes alter owned heap shape and require updating this walk.
const _: () = assert!(!std::mem::needs_drop::<serde_json::Number>());
const _: () = assert!(
    size_of::<serde_json::Map<String, Value>>()
        == size_of::<std::collections::BTreeMap<String, Value>>()
);

pub(crate) struct SourceBytes<'a> {
    bytes: usize,
    control: &'a OperationControl,
}

impl<'a> SourceBytes<'a> {
    pub(crate) fn new(control: &'a OperationControl) -> Result<Self, AppError> {
        control.check().map_err(operation_interrupted)?;
        Ok(Self { bytes: 0, control })
    }

    pub(crate) fn add(&mut self, bytes: usize) -> Result<(), AppError> {
        self.control.check().map_err(operation_interrupted)?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(resource_limit_error)?;
        Ok(())
    }

    pub(crate) fn strings<const N: usize>(&mut self, values: [&String; N]) -> Result<(), AppError> {
        for value in values {
            self.add(value.capacity())?;
        }
        Ok(())
    }

    pub(crate) fn vector<T>(&mut self, values: &[T], capacity: usize) -> Result<(), AppError> {
        debug_assert!(capacity >= values.len());
        self.add(
            capacity
                .checked_mul(size_of::<T>())
                .ok_or_else(resource_limit_error)?,
        )
    }

    pub(crate) fn arc<T>(&mut self) -> Result<(), AppError> {
        // Rust 1.97.1 alloc::sync::ArcInner is repr(C): two Atomic<usize>s then T.
        let (layout, _) = Layout::new::<[usize; 2]>()
            .extend(Layout::new::<T>())
            .map_err(|_| resource_limit_error())?;
        self.add(layout.pad_to_align().size())
    }

    pub(crate) fn finish(self) -> usize {
        self.bytes
    }

    pub(crate) fn entity(&mut self, entity: &DomainEntityRef) -> Result<(), AppError> {
        let DomainEntityRef { kind, id } = entity;
        self.add(kind.heap_capacity())?;
        self.add(id.heap_capacity())
    }

    pub(crate) fn accepted(&mut self, accepted: &AcceptedResultRefV1) -> Result<(), AppError> {
        let AcceptedResultRefV1 {
            solution_id: _,
            result_checksum,
        } = accepted;
        self.strings([result_checksum])
    }

    pub(crate) fn input(&mut self, input: &RunInput) -> Result<(), AppError> {
        let RunInput {
            schema_version: _,
            run_id: _,
            request_id: _,
            request_hash,
            scenario_id: _,
            scenario_revision: _,
            snapshot_id: _,
            snapshot_document_hash,
            snapshot_created_at: _,
            pack_id,
            pack_schema_version: _,
            planning_ir_schema_version: _,
            compiler_version,
            application_version,
            backend_id,
            backend_version,
            adapter_version,
            worker_version,
            solver_version,
            protocol_major: _,
            protocol_minor: _,
            model_hash,
            objective_policy_hash,
            solve_options,
            scenario_timezone,
            temporary_condition_hash,
            objective_context,
            checksum,
        } = input;
        self.strings([
            request_hash,
            snapshot_document_hash,
            compiler_version,
            application_version,
            backend_version,
            adapter_version,
            worker_version,
            solver_version,
            model_hash,
            objective_policy_hash,
            checksum,
        ])?;
        // Root namespaced IDs/time zones are immutable Strings constructed from &str.to_owned().
        self.add(pack_id.as_str().len())?;
        self.add(backend_id.as_str().len())?;
        self.add(scenario_timezone.as_str().len())?;
        self.options(solve_options)?;
        if let Some(hash) = temporary_condition_hash {
            self.strings([hash])?;
        }
        if let Some(context) = objective_context {
            self.context(context)?;
        }
        Ok(())
    }

    fn options(&mut self, options: &SolveOptions) -> Result<(), AppError> {
        let SolveOptions {
            backend,
            mode: _,
            time_limit_milliseconds: _,
            memory_limit_bytes: _,
            worker_threads: _,
            random_seed: _,
            solution_limit: _,
            stop_after_first_feasible: _,
            collect_intermediate_solutions: _,
            explanation_mode: _,
            preserve_existing: _,
            reproducibility: _,
            resource_limits: _,
        } = options;
        match backend {
            BackendSelection::Auto => Ok(()),
            BackendSelection::Specific(id) => self.add(id.as_str().len()),
        }
    }

    fn context(&mut self, context: &RunObjectiveContext) -> Result<(), AppError> {
        let RunObjectiveContext {
            schema_version: _,
            pack_id,
            profile_id,
            semantic_version: _,
            operation: _,
            baseline,
            external_baseline,
        } = context;
        self.add(pack_id.as_str().len())?;
        self.add(profile_id.heap_capacity())?;
        if let Some(baseline) = baseline {
            self.add(size_of::<RunBaselineBinding>())?;
            let RunBaselineBinding {
                original,
                selection,
                selected_pair_content_hash,
                captured_original,
            } = baseline.as_ref();
            let RunBaselineOriginal {
                entity,
                pair_content_hash,
                accepted_result,
                source_scenario_id: _,
                source_revision: _,
                source_document_hash,
                source_run_id: _,
                source_run_input_checksum,
                source_run_manifest_checksum,
            } = original;
            self.entity(entity)?;
            self.accepted(accepted_result)?;
            self.strings([
                pair_content_hash,
                source_document_hash,
                source_run_input_checksum,
                source_run_manifest_checksum,
                selected_pair_content_hash,
            ])?;
            match selection {
                RunBaselineSelection::OriginalAccepted {} => {}
                RunBaselineSelection::ManualSelection {
                    entity,
                    record_checksum,
                    admission_source_scenario_id: _,
                    admission_source_revision: _,
                    admission_source_document_hash,
                } => {
                    self.entity(entity)?;
                    self.strings([record_checksum, admission_source_document_hash])?;
                }
            }
            if let Some(original) = captured_original {
                self.arc::<Value>()?;
                self.json(original, 0)?;
            }
        }
        if let Some(external) = external_baseline {
            self.add(size_of::<RunExternalBaselineBinding>())?;
            let RunExternalBaselineBinding {
                entity,
                selected_pair_content_hash,
            } = external.as_ref();
            self.entity(entity)?;
            self.strings([selected_pair_content_hash])?;
        }
        Ok(())
    }

    fn json(&mut self, value: &Value, depth: usize) -> Result<(), AppError> {
        // Refuse excessive nesting rather than risking the accounting walk's native stack.
        if depth > 128 {
            return Err(resource_limit_error());
        }
        self.add(0)?;
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
            Value::String(value) => self.strings([value])?,
            Value::Array(values) => {
                self.vector(values, values.capacity())?;
                for value in values {
                    self.json(value, depth + 1)?;
                }
            }
            Value::Object(values) => {
                // serde_json 1.0.151 without preserve_order uses BTreeMap. Pinned Rust 1.97.1
                // alloc/collections/btree/node.rs has B=6: 11 K/V slots, 12 child pointers,
                // a parent pointer and two u16s. Four usize words overbound the header/padding.
                // Every nonempty live node owns at least one key; an empty retained root may
                // still exist after deletion. Thus len+1 full internal nodes bounds allocation.
                // This intentionally overcharges spare BTree slots, never JSON serialized bytes.
                let node = 11_usize
                    .checked_mul(size_of::<String>() + size_of::<Value>())
                    .and_then(|bytes| bytes.checked_add(16 * size_of::<usize>()))
                    .ok_or_else(resource_limit_error)?;
                self.add(
                    values
                        .len()
                        .checked_add(1)
                        .and_then(|nodes| nodes.checked_mul(node))
                        .ok_or_else(resource_limit_error)?,
                )?;
                for (key, value) in values {
                    self.strings([key])?;
                    self.json(value, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn pending_observations(&mut self) -> Result<(), AppError> {
        // An ordinary verified checkpoint can be published inside candidate review, before
        // SolverRouter::execute returns and native_observations::capture installs its result.
        // capture collects at most MAX_SCORE_LEVELS fresh IDs and clones attempt/runtime
        // Strings and numeric Vecs, dropping their incoming spare capacity. Pinned Vec
        // collection's minimum/doubling growth fits twice the maximum objective count.
        // The observation Arc embeds its optional attempt/outcome/runtime structs.
        self.arc::<RunNativeObservations>()?;
        let slots = MAX_SCORE_LEVELS
            .checked_mul(2)
            .ok_or_else(resource_limit_error)?;
        for (count, capacity) in [
            (slots, size_of::<RunNativeObjective>()),
            (MAX_SCORE_LEVELS, MAX_DOMAIN_ID_BYTES),
            (slots, size_of::<i64>()), // Two cloned objective/bound vectors, each <= max levels.
            (7, 64),  // Three root hashes, fingerprint, manifest and two runtime digests.
            (1, 128), // Root BackendId constructor's maximum namespaced ID length.
            (4, MAX_RUN_VERSION_BYTES), // Backend, adapter, worker and engine versions.
            (2, MAX_RUN_DIAGNOSTIC_CODE_BYTES), // Termination and backend-failure codes.
        ] {
            self.add(
                count
                    .checked_mul(capacity)
                    .ok_or_else(resource_limit_error)?,
            )?;
        }
        Ok(())
    }

    pub(crate) fn observations(
        &mut self,
        observations: &RunNativeObservations,
    ) -> Result<(), AppError> {
        self.arc::<RunNativeObservations>()?;
        let RunNativeObservations {
            schema_version: _,
            model_hash,
            objective_policy_hash,
            solve_options_hash,
            objectives,
            terminal_status: _,
            terminal_reason: _,
            attempt,
        } = observations;
        self.strings([model_hash, objective_policy_hash, solve_options_hash])?;
        self.vector(objectives, objectives.capacity())?;
        for objective in objectives {
            let RunNativeObjective {
                level_id,
                direction: _,
                lower_bound: _,
                upper_bound: _,
            } = objective;
            self.add(level_id.heap_capacity())?;
        }
        if let Some(attempt) = attempt {
            let RunNativeAttempt {
                backend_id,
                backend_version,
                adapter_version,
                solve_fingerprint,
                invocation_index: _,
                remaining_at_dispatch_milliseconds: _,
                backend_limit_milliseconds: _,
                termination,
                backend_failure_code,
                approved_worker_manifest_sha256,
                outcome,
            } = attempt;
            self.add(backend_id.as_str().len())?;
            self.strings([backend_version, adapter_version, solve_fingerprint])?;
            match termination {
                RunNativeAttemptTermination::PreflightRejected { code }
                | RunNativeAttemptTermination::BackendError { code }
                | RunNativeAttemptTermination::OutcomeContractFailure { code }
                | RunNativeAttemptTermination::VerificationQuarantined {
                    diagnostic_code: code,
                } => self.strings([code])?,
                RunNativeAttemptTermination::BackendOutcome(_)
                | RunNativeAttemptTermination::CandidateAwaitingVerification
                | RunNativeAttemptTermination::CandidateVerified
                | RunNativeAttemptTermination::ReviewCancelled
                | RunNativeAttemptTermination::ReviewDeadlineExceeded
                | RunNativeAttemptTermination::ReviewResourceLimitExceeded
                | RunNativeAttemptTermination::ParentCancelled
                | RunNativeAttemptTermination::ParentDeadlineExceeded => {}
            }
            for code in [backend_failure_code, approved_worker_manifest_sha256]
                .into_iter()
                .flatten()
            {
                self.strings([code])?;
            }
            if let Some(outcome) = outcome {
                let RunNativeOutcome {
                    reason: _,
                    objective_values,
                    best_bound_values,
                    runtime,
                } = outcome;
                for values in [objective_values, best_bound_values].into_iter().flatten() {
                    self.vector(values, values.capacity())?;
                }
                if let Some(runtime) = runtime {
                    let RunNativeRuntime {
                        worker_version,
                        engine_version,
                        protocol_major: _,
                        protocol_minor: _,
                        model_fingerprint_sha256,
                        applied_parameters_sha256,
                    } = runtime;
                    self.strings([
                        worker_version,
                        engine_version,
                        model_fingerprint_sha256,
                        applied_parameters_sha256,
                    ])?;
                }
            }
        }
        Ok(())
    }
}
