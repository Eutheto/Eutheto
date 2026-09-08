//! One finite local solve: the core owns capture, acceptance, deadlines and durable finalization.

use super::{
    CliExitCode, Outcome, SafeCliError, SafeCliWarning, SolveArgs, SolveModeArg, app_dependencies,
    app_error, bundled_solver, files, operation_request_id, scenario_files, unexpected_result,
};
use eutheto_core::{
    AppQuery, AppQueryResult, EuthetoApp, HeadlessCompilationReport, HeadlessService,
    SolvePreparationFailure, StoredSolveFailure, StoredSolveRequest,
};
use eutheto_domain_ir::{
    ConflictUnavailableReason, InfeasibilityEvidenceV1, RunInputV1, RunManifestV1,
    RunTerminalOutcomeV1,
};
use eutheto_export::PORTABLE_LIMITS;
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_solver_api::ProgressSink;
use eutheto_types::{
    BackendSelection, CancellationToken, DurationMillis, ExplanationMode, OperationControl,
    PreservationPolicy, ReproducibilityMode, ResourceLimits, ScenarioId, SolveMode, SolveOptions,
    SolveStatus, SystemClock, SystemIdGenerator, SystemMonotonicClock, WorkerThreadPolicy,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) async fn execute(
    data_dir: Option<PathBuf>,
    mut args: SolveArgs,
    cancellation: &CancellationToken,
    has_config: bool,
    progress: &mut dyn ProgressSink,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    if args.command.is_some() || args.repair_from.is_some() {
        return Err((
            "solve",
            SafeCliError::unavailable(
                "solve.operation_unavailable",
                "Service-mode cancellation and repair are unavailable; use a finite local solve.",
            ),
        ));
    }
    scenario_files::reject_config(has_config).map_err(|error| ("solve", error))?;
    let input = args.input.take().ok_or_else(|| {
        (
            "solve",
            usage(
                "solve.input_required",
                "A scenario file or stored identifier is required.",
            ),
        )
    })?;
    let stored_id = files::scenario_id_or_file(&input).map_err(|error| ("solve", error))?;
    if stored_id.is_none() && args.output.is_none() {
        return Err((
            "solve",
            usage(
                "solve.output_required",
                "File solves require --output to retain the accepted result.",
            ),
        ));
    }
    let options = options(&args).map_err(|error| ("solve", error))?;
    let startup_control = OperationControl::Cancellation(cancellation.clone());
    let registry = bundled_solver::load(&startup_control)
        .await
        .map_err(|error| ("solve", error))?;
    let result = if let Some(scenario_id) = stored_id {
        let dependencies =
            app_dependencies(data_dir, cancellation).map_err(|error| ("solve", error))?;
        let app = EuthetoApp::open_with_solver_registry(dependencies, registry)
            .await
            .map_err(|error| ("solve", app_error(error)))?;
        stored(&app, scenario_id, options, &args, cancellation, progress).await
    } else {
        let service = HeadlessService::with_solver_registry(
            Arc::new(SystemClock),
            Arc::new(SystemMonotonicClock::new()),
            Arc::new(SystemIdGenerator),
            registry,
        )
        .map_err(|error| ("solve", app_error(error)))?;
        file(&service, &input, options, &args, cancellation, progress).await
    };
    result.map_err(|error| ("solve", error))
}

async fn file(
    service: &HeadlessService,
    input: &Path,
    options: SolveOptions,
    args: &SolveArgs,
    cancellation: &CancellationToken,
    progress: &mut dyn ProgressSink,
) -> Result<Outcome, SafeCliError> {
    let operation = service
        .begin_solve(operation_request_id()?, options, cancellation.clone())
        .map_err(app_error)?;
    let control = operation.control();
    let prepared = {
        let bytes = files::read_controlled(
            input,
            PORTABLE_LIMITS.max_archive_bytes,
            "scenario.input_invalid",
            &control,
        )
        .await?;
        if bytes.starts_with(b"PK\x03\x04") {
            operation.prepare_bundle(&bytes)
        } else {
            let inspected = service
                .decode_scenario(&bytes, &control)
                .map_err(app_error)?;
            operation.prepare(inspected.scenario)
        }
    }
    .map_err(|failure| preparation_error(*failure))?;
    let solved = prepared.execute(progress).await.map_err(app_error)?;
    // These observations are not emitted as accepted until the typed publication succeeds.
    let result = terminal(solved.run_input(), solved.manifest());
    let diagnostics = args.include_diagnostics.as_ref().map(|_| {
        diagnostics(
            solved.run_input(),
            solved.manifest(),
            Some(&solved.compilation),
            true,
        )
    });
    if solved.accepted_result().is_some() {
        let output = args.output.as_deref().ok_or_else(unexpected_result)?;
        let publication = solved
            .prepare_publication(output)
            .map_err(app_error)?
            .ok_or_else(unexpected_result)?;
        publication.publish().map_err(app_error)?;
    }
    finish_diagnostics(
        result,
        args.include_diagnostics.as_deref(),
        diagnostics.as_ref(),
        cancellation,
    )
}

async fn stored(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    options: SolveOptions,
    args: &SolveArgs,
    cancellation: &CancellationToken,
    progress: &mut dyn ProgressSink,
) -> Result<Outcome, SafeCliError> {
    let request = StoredSolveRequest {
        request_id: operation_request_id()?,
        scenario_id,
        expected_revision: None,
        options,
    };
    let mut solved = Box::pin(app.solve_stored(request, progress))
        .await
        .map_err(stored_failure)?;
    let state = solved.state();
    let manifest = state.manifest.as_ref().ok_or_else(|| {
        SafeCliError::new(
            CliExitCode::Conflict,
            "solve.already_running",
            "This request already owns an unfinished solve run.",
        )
    })?;
    let mut result = terminal(&state.input, manifest);
    let diagnostics = args.include_diagnostics.as_ref().map(|_| {
        diagnostics(
            &state.input,
            manifest,
            solved.report.as_ref().map(|report| &report.compilation),
            false,
        )
    });
    if let Ok(outcome) = &mut result {
        if let Some(error) = solved.warning.take() {
            outcome.warnings.push(warning(
                "solve.result_reload_failed",
                "The run committed, but retained result material could not be reloaded.",
                &app_error(error),
            ));
        }
        if let Some(output) = &args.output
            && let Some(publication) = solved.publish_result(output, cancellation)
            && let Err(error) = publication.publication
        {
            outcome.warnings.push(warning("solve.output_publication_failed", "The accepted database result committed, but its requested output could not be published. Do not repeat the solve to recover the result.", &app_error(error)));
        }
        observe_current_revision(app, scenario_id, outcome).await;
    }
    finish_diagnostics(
        result,
        args.include_diagnostics.as_deref(),
        diagnostics.as_ref(),
        cancellation,
    )
}

async fn observe_current_revision(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    outcome: &mut Outcome,
) {
    match app.query(AppQuery::ProjectMetadata(scenario_id)).await {
        Ok(AppQueryResult::Project(project)) => {
            let current_revision = json!(project.revision);
            let stale = outcome.result["scenarioRevision"] != current_revision;
            outcome.result["currentRevision"] = current_revision;
            outcome.result["stale"] = json!(stale);
            if stale {
                outcome.human.push(format!(
                    "Stale: this run used revision {}; the current scenario is revision {}.",
                    outcome.result["scenarioRevision"],
                    project.revision.value(),
                ));
            }
        }
        observed => {
            let cause = observed.err().map_or_else(unexpected_result, app_error);
            outcome.result["currentRevision"] = Value::Null;
            outcome.result["stale"] = Value::Null;
            outcome
                .human
                .push("Current revision unavailable; result freshness is unknown.".to_owned());
            outcome.warnings.push(warning("solve.current_revision_unavailable", "The run committed; its relationship to the current revision could not be observed.", &cause));
        }
    }
}

fn options(args: &SolveArgs) -> Result<SolveOptions, SafeCliError> {
    let threads = args.threads.unwrap_or(1);
    if threads == 0 {
        return Err(usage(
            "solve.threads_invalid",
            "The exact thread count must be positive.",
        ));
    }
    let seed = args.seed.unwrap_or(1);
    let backend = match args.backend.as_deref() {
        None | Some("auto") => BackendSelection::Auto,
        Some(id) => BackendSelection::Specific(id.parse().map_err(|_| {
            usage(
                "solve.backend_invalid",
                "The backend identifier is invalid.",
            )
        })?),
    };
    let collection_limit = u32::try_from(PORTABLE_LIMITS.max_collection_items)
        .map_err(|_| super::serialization_error())?;
    Ok(SolveOptions {
        backend,
        mode: match args.mode.unwrap_or(SolveModeArg::Balanced) {
            SolveModeArg::Quick => SolveMode::Quick,
            SolveModeArg::Balanced => SolveMode::Balanced,
            SolveModeArg::Deep => SolveMode::Deep,
        },
        time_limit_milliseconds: duration(args.max_time.as_deref().unwrap_or("30s"))?,
        memory_limit_bytes: None,
        worker_threads: WorkerThreadPolicy::Exact(threads),
        random_seed: seed,
        solution_limit: None,
        stop_after_first_feasible: args.first_feasible,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::None,
        preserve_existing: PreservationPolicy::None,
        reproducibility: if threads == 1 && seed == 1 {
            ReproducibilityMode::Deterministic
        } else {
            ReproducibilityMode::Performance
        },
        resource_limits: ResourceLimits {
            max_entities: collection_limit,
            max_rules: collection_limit,
            max_variables: PlanningIrLimitsV1::DEFAULT.max_variables,
            max_constraints: PlanningIrLimitsV1::DEFAULT.max_constraints,
        },
    })
}

fn duration(text: &str) -> Result<DurationMillis, SafeCliError> {
    let invalid = || {
        usage(
            "solve.duration_invalid",
            "Use a positive integer duration with ms, s, m or h, within the supported millisecond range.",
        )
    };
    let (digits, unit) = [("ms", 1_u64), ("s", 1_000), ("m", 60_000), ("h", 3_600_000)]
        .into_iter()
        .find_map(|(suffix, unit)| text.strip_suffix(suffix).map(|digits| (digits, unit)))
        .ok_or_else(invalid)?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let value = digits
        .parse::<u64>()
        .ok()
        .and_then(|value| value.checked_mul(unit))
        .filter(|value| *value > 0)
        .ok_or_else(invalid)?;
    DurationMillis::new(value).map_err(|_| invalid())
}

fn terminal(input: &RunInputV1, manifest: &RunManifestV1) -> Result<Outcome, SafeCliError> {
    let (status, result_id) = match &manifest.outcome {
        RunTerminalOutcomeV1::Accepted {
            status,
            solution_id,
            ..
        } => (*status, Some(*solution_id)),
        RunTerminalOutcomeV1::NoResult { status } => (*status, None),
        RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code } => {
            return Err(terminal_error(
                input,
                CliExitCode::Verification,
                diagnostic_code,
                "Independent verification rejected the candidate; no accepted result is available.",
            ));
        }
        RunTerminalOutcomeV1::Interrupted => {
            return Err(terminal_error(
                input,
                CliExitCode::Storage,
                "solve.interrupted",
                "The solve was interrupted without confirmed finalization.",
            ));
        }
    };
    let (label, exit, message) = terminal_disposition(input, status)?;
    let mut human = vec![message.to_owned()];
    if let Some(result_id) = result_id {
        human.push(format!(
            "Result {result_id}; scenario {} revision {}.",
            input.scenario_id, input.scenario_revision
        ));
    }
    Ok(Outcome::new("solve", label, json!({
        "runId": input.run_id, "scenarioId": input.scenario_id,
        "scenarioRevision": input.scenario_revision, "backendId": input.backend_id,
        "status": status, "resultId": result_id,
        "independentlyVerified": result_id.is_some(),
        "infeasibilityEvidence": (status == SolveStatus::Infeasible).then_some(
            InfeasibilityEvidenceV1::Unavailable { reason: ConflictUnavailableReason::ConflictNotReturned }
        ),
    }), human).with_exit(exit))
}

fn terminal_disposition(
    input: &RunInputV1,
    status: SolveStatus,
) -> Result<(&'static str, CliExitCode, &'static str), SafeCliError> {
    Ok(match status {
        SolveStatus::Optimal => (
            "optimal",
            CliExitCode::Success,
            "Optimal plan independently verified and committed.",
        ),
        SolveStatus::Feasible => (
            "feasible",
            CliExitCode::Success,
            "Feasible plan independently verified and committed; optimality is not claimed.",
        ),
        SolveStatus::Infeasible => (
            "infeasible",
            CliExitCode::Infeasible,
            "The configured Required rules are infeasible. Conflict evidence is unavailable (conflictNotReturned).",
        ),
        SolveStatus::NoSolutionWithinLimit => (
            "noSolutionWithinLimit",
            CliExitCode::NoVerifiedSolution,
            "No verified solution was obtained within the limit; infeasibility is not proven.",
        ),
        SolveStatus::Cancelled => {
            return Err(terminal_error(
                input,
                CliExitCode::Cancelled,
                "operation.cancelled",
                "The solve was cancelled without an accepted result.",
            ));
        }
        SolveStatus::InvalidModel => {
            return Err(terminal_error(
                input,
                CliExitCode::Validation,
                "solve.invalid_model",
                "The scenario could not be compiled into a supported valid model.",
            ));
        }
        SolveStatus::BackendUnavailable => {
            return Err(terminal_error(
                input,
                CliExitCode::Unavailable,
                "solve.backend_unavailable",
                "The selected backend is unavailable.",
            ));
        }
        SolveStatus::BackendFailed => {
            return Err(terminal_error(
                input,
                CliExitCode::Unavailable,
                "solve.backend_failed",
                "The backend failed without an accepted result.",
            ));
        }
        SolveStatus::Unbounded => {
            return Err(terminal_error(
                input,
                CliExitCode::Unavailable,
                "solve.unbounded_outcome_unsupported",
                "The backend reported an unsupported unbounded outcome, not infeasibility.",
            ));
        }
    })
}

fn terminal_error(
    input: &RunInputV1,
    exit: CliExitCode,
    code: &str,
    message: &str,
) -> SafeCliError {
    let mut error = SafeCliError::new(exit, code, message);
    error.details = Some(
        json!({"runId": input.run_id, "scenarioId": input.scenario_id, "scenarioRevision": input.scenario_revision}),
    );
    error
}

fn preparation_error(failure: SolvePreparationFailure) -> SafeCliError {
    let mut error = app_error(failure.error);
    error.exit = match failure.status {
        SolveStatus::Cancelled => CliExitCode::Cancelled,
        SolveStatus::NoSolutionWithinLimit => CliExitCode::NoVerifiedSolution,
        SolveStatus::BackendUnavailable | SolveStatus::BackendFailed => CliExitCode::Unavailable,
        SolveStatus::InvalidModel if error.exit == CliExitCode::Application => {
            CliExitCode::Validation
        }
        _ => error.exit,
    };
    error.details = Some(
        json!({"solveStatus": failure.status, "cause": error.details.take(), "validation": failure.validation, "routing": failure.routing}),
    );
    error
}

fn stored_failure(failure: StoredSolveFailure) -> SafeCliError {
    match failure {
        StoredSolveFailure::BeforeStart(error) => app_error(error),
        StoredSolveFailure::Preparation(failure) => preparation_error(*failure),
        StoredSolveFailure::AfterStart {
            run_id,
            error,
            cleanup,
        } => {
            let mut error = app_error(error);
            let cleanup_error = cleanup.err().map(app_error);
            if cleanup_error.is_some() {
                error.exit = CliExitCode::Storage;
            }
            error.details = Some(
                json!({"runId": run_id, "cause": error.details.take(), "terminalizationConfirmed": cleanup_error.is_none(), "cleanupCauseCode": cleanup_error.map(|error| error.code.clone())}),
            );
            error
        }
        StoredSolveFailure::FinalizationUnconfirmed {
            run_id,
            result_id,
            error,
        } => {
            let cause = app_error(error);
            let mut error = SafeCliError::storage(
                "solve.finalization_unconfirmed",
                "The database finalization outcome is unconfirmed. Inspect retained state; do not assume rollback or repeat the solve blindly.",
            );
            error.details =
                Some(json!({"runId": run_id, "resultId": result_id, "causeCode": cause.code}));
            error
        }
    }
}

fn diagnostics(
    input: &RunInputV1,
    manifest: &RunManifestV1,
    compilation: Option<&HeadlessCompilationReport>,
    before_publication: bool,
) -> Value {
    json!({
        "apiVersion": super::API_VERSION, "command": "solve.diagnostics", "ok": true,
        "status": "captured", "diagnosticId": Value::Null, "warnings": [],
        "result": {
            "runId": input.run_id, "scenarioRevision": input.scenario_revision,
            "backendId": input.backend_id, "backendVersion": input.backend_version,
            "observedBeforeArtifactPreparation": before_publication,
            "outcome": manifest.outcome, "observedElapsedMilliseconds": manifest.elapsed_milliseconds,
            "phaseTimings": manifest.phase_timings,
            "model": compilation.map(|report| json!({
                "variableCount": report.summary.variable_count,
                "constraintCount": report.summary.constraint_count,
                "objectiveTermCount": report.summary.objective_term_count,
            })),
        },
    })
}

fn finish_diagnostics(
    mut result: Result<Outcome, SafeCliError>,
    destination: Option<&Path>,
    metadata: Option<&Value>,
    cancellation: &CancellationToken,
) -> Result<Outcome, SafeCliError> {
    if let (Some(destination), Some(metadata)) = (destination, metadata) {
        let control = OperationControl::Cancellation(cancellation.clone());
        let publication = serde_json::to_vec(metadata)
            .map_err(|_| super::serialization_error())
            .and_then(|bytes| files::publish_text(destination, &bytes, &control));
        if let Err(cause) = publication {
            let warning = warning(
                "solve.diagnostics_publication_failed",
                "The terminal solve outcome is unchanged, but requested diagnostics could not be published.",
                &cause,
            );
            match &mut result {
                Ok(outcome) => outcome.warnings.push(warning),
                Err(error) => {
                    error.human_details.push(warning.message.clone());
                    error.details =
                        Some(json!({"cause": error.details.take(), "diagnosticsWarning": warning}));
                }
            }
        }
    }
    result
}

fn warning(code: &str, message: &str, cause: &SafeCliError) -> SafeCliWarning {
    SafeCliWarning {
        code: code.to_owned(),
        message: message.to_owned(),
        details: Some(json!({"causeCode": cause.code})),
    }
}

fn usage(code: &str, message: &str) -> SafeCliError {
    SafeCliError::new(CliExitCode::Usage, code, message)
}
