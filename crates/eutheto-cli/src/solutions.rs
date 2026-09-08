//! File results are untrusted historical records until freshly verified against their source.

use super::{
    CliExitCode, ExportSolutionArgs, Outcome, OutputFormat, SafeCliError, SafeCliWarning,
    SolutionExportFormat, SolutionsArgs, SolutionsCommand, app_error, execute_solution_compare,
    execute_solution_explain, execute_solution_list, execute_solution_verify, files, open_app,
    scenario_files,
};
use eutheto_domain_ir::{DomainAssignmentId, PortableAcceptedResultV2};
use eutheto_export::PORTABLE_LIMITS;
use eutheto_types::{CancellationToken, OperationControl, Revision, ScenarioId, SolutionId};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) async fn execute(
    data_dir: Option<PathBuf>,
    args: SolutionsArgs,
    cancellation: &CancellationToken,
    output_format: OutputFormat,
    has_config: bool,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    match args.command {
        SolutionsCommand::List { scenario } => {
            let id = files::scenario_id_or_file(Path::new(&scenario))
                .map_err(|error| ("solutions.list", error))?
                .ok_or_else(|| {
                    (
                        "solutions.list",
                        SafeCliError::unavailable(
                            "solution.stored_list_required",
                            "Listing retained solutions requires a stored scenario identifier.",
                        ),
                    )
                })?;
            let app = open_app(data_dir, cancellation)
                .await
                .map_err(|error| ("solutions.list", error))?;
            execute_solution_list(&app, id).await
        }
        SolutionsCommand::Verify { scenario, solution } => {
            verify(data_dir, &scenario, &solution, cancellation, has_config)
                .await
                .map_err(|error| ("solutions.verify", error))
        }
        SolutionsCommand::Explain {
            scenario,
            solution,
            assignment,
        } => explain(
            data_dir,
            &scenario,
            &solution,
            &assignment,
            cancellation,
            has_config,
        )
        .await
        .map_err(|error| ("solutions.explain", error)),
        SolutionsCommand::Export(args) => {
            export(data_dir, args, cancellation, output_format, has_config)
                .await
                .map_err(|error| ("solutions.export", error))
        }
        SolutionsCommand::Compare {
            scenario,
            solution_a,
            solution_b,
        } => {
            let left = paired_ids(Path::new(&scenario), Path::new(&solution_a))
                .map_err(|error| ("solutions.compare", error))?;
            let right = paired_ids(Path::new(&scenario), Path::new(&solution_b))
                .map_err(|error| ("solutions.compare", error))?;
            if left.is_none() || right.is_none() {
                return Err((
                    "solutions.compare",
                    SafeCliError::unavailable(
                        "solution.file_comparison_unavailable",
                        "Comparison currently requires retained stored solutions.",
                    ),
                ));
            }
            let app = open_app(data_dir, cancellation)
                .await
                .map_err(|error| ("solutions.compare", error))?;
            execute_solution_compare(&app, &scenario, &solution_a, &solution_b).await
        }
    }
}

fn paired_ids(
    scenario: &Path,
    solution: &Path,
) -> Result<Option<(ScenarioId, SolutionId)>, SafeCliError> {
    match (
        files::scenario_id_or_file(scenario)?,
        files::solution_id_or_file(solution)?,
    ) {
        (Some(scenario), Some(solution)) => Ok(Some((scenario, solution))),
        (None, None) => Ok(None),
        _ => Err(SafeCliError::new(
            CliExitCode::Usage,
            "solution.mixed_input_modes",
            "Use two stored identifiers or two explicit files; mixed inputs are unavailable.",
        )),
    }
}

async fn load_result(
    path: &Path,
    control: &OperationControl,
) -> Result<PortableAcceptedResultV2, SafeCliError> {
    let bytes = files::read_controlled(
        path,
        PORTABLE_LIMITS.max_json_bytes,
        "solution.input_invalid",
        control,
    )
    .await?;
    let portable = PortableAcceptedResultV2::from_json(&bytes).map_err(|_| SafeCliError::storage(
        "solution.portable_invalid", "The accepted-result file is malformed, incompatible, oversized or internally inconsistent.",
    ))?;
    files::check_control(control)?;
    Ok(portable)
}

async fn verify(
    data_dir: Option<PathBuf>,
    scenario: &str,
    solution: &str,
    cancellation: &CancellationToken,
    has_config: bool,
) -> Result<Outcome, SafeCliError> {
    if let Some((scenario_id, solution_id)) = paired_ids(Path::new(scenario), Path::new(solution))?
    {
        let app = open_app(data_dir, cancellation).await?;
        return execute_solution_verify(&app, scenario_id, solution_id)
            .await
            .map_err(|(_, error)| error);
    }
    scenario_files::reject_config(has_config)?;
    let control = OperationControl::Cancellation(cancellation.clone());
    let service = scenario_files::service()?;
    let loaded = scenario_files::load(&service, Path::new(scenario), true, &control).await?;
    let snapshot = loaded.snapshot()?;
    let portable = load_result(Path::new(solution), &control).await?;
    let verification = service
        .verify_result(snapshot, &portable, &control)
        .map_err(app_error)?;
    Ok(Outcome::new("solutions.verify", "verified", json!({
        "resultId": portable.result_id, "scenarioRevision": snapshot.revision,
        "verification": verification, "historicalRunMetadata": "sourceProvidedUnverified",
    }), vec!["The supplied schedule and score were freshly independently verified. External solver history, optimality and timings remain unverified.".to_owned()]))
}

async fn explain(
    data_dir: Option<PathBuf>,
    scenario: &str,
    solution: &str,
    assignment: &str,
    cancellation: &CancellationToken,
    has_config: bool,
) -> Result<Outcome, SafeCliError> {
    let ids = paired_ids(Path::new(scenario), Path::new(solution))?;
    let assignment_id = assignment.parse::<DomainAssignmentId>().map_err(|_| {
        SafeCliError::validation(
            "solution.explanation_request_invalid",
            "The explanation request contains an invalid assignment identifier.",
        )
    })?;
    if let Some((scenario_id, solution_id)) = ids {
        let app = open_app(data_dir, cancellation).await?;
        return execute_solution_explain(&app, scenario_id, solution_id, assignment_id)
            .await
            .map_err(|(_, error)| error);
    }
    scenario_files::reject_config(has_config)?;
    let control = OperationControl::Cancellation(cancellation.clone());
    let service = scenario_files::service()?;
    let loaded = scenario_files::load(&service, Path::new(scenario), true, &control).await?;
    let portable = load_result(Path::new(solution), &control).await?;
    let explanation = service
        .explain_assignment(loaded.snapshot()?, &portable, &assignment_id, &control)
        .map_err(app_error)?;
    let human = explanation_human(&explanation.rendered)?;
    Ok(Outcome::new(
        "solutions.explain",
        "explained",
        json!({
            "resultId": portable.result_id, "explanation": explanation,
            "historicalRunMetadata": "sourceProvidedUnverified",
        }),
        human,
    )
    .with_warning(external_history_warning()))
}

async fn export(
    data_dir: Option<PathBuf>,
    args: ExportSolutionArgs,
    cancellation: &CancellationToken,
    output_format: OutputFormat,
    has_config: bool,
) -> Result<Outcome, SafeCliError> {
    let format = match args.format {
        SolutionExportFormat::Csv => "csv",
        SolutionExportFormat::Json => "json",
        _ => {
            return Err(SafeCliError::unavailable(
                "solution.export_format_unavailable",
                "This export format is unavailable; use JSON or assignment CSV.",
            ));
        }
    };
    if output_format == OutputFormat::Json && args.output.is_none() {
        return Err(SafeCliError::new(
            CliExitCode::Usage,
            "solution.export_output_required",
            "JSON result-envelope mode requires --output; raw data cannot share stdout with the envelope.",
        ));
    }
    let ids = paired_ids(&args.scenario, &args.solution)?;
    let control = OperationControl::Cancellation(cancellation.clone());
    let mut warnings = Vec::new();
    let (bytes, metadata) = if let Some((scenario_id, solution_id)) = ids {
        let app = open_app(data_dir, cancellation).await?;
        let exported = match args.format {
            SolutionExportFormat::Csv => {
                app.export_solution_assignments_csv(scenario_id, solution_id)
                    .await
            }
            SolutionExportFormat::Json => app.export_solution_json(scenario_id, solution_id).await,
            _ => return Err(super::unexpected_result()),
        }
        .map_err(app_error)?;
        warnings.extend(stale_warning(
            exported.scenario_revision,
            exported.current_revision,
        ));
        (
            exported.bytes,
            json!({
                "scenarioId": scenario_id, "resultId": solution_id,
                "scenarioRevision": exported.scenario_revision, "currentRevision": exported.current_revision,
                "stale": exported.scenario_revision != exported.current_revision,
                "format": format, "output": args.output,
            }),
        )
    } else {
        scenario_files::reject_config(has_config)?;
        let service = scenario_files::service()?;
        let loaded = scenario_files::load(&service, &args.scenario, true, &control).await?;
        let snapshot = loaded.snapshot()?;
        let portable = load_result(&args.solution, &control).await?;
        let bytes = match args.format {
            SolutionExportFormat::Csv => {
                service.export_assignments_csv(snapshot, &portable, &control)
            }
            SolutionExportFormat::Json => service.export_result_json(snapshot, &portable, &control),
            _ => return Err(super::unexpected_result()),
        }
        .map_err(app_error)?;
        warnings.push(external_history_warning());
        (
            bytes,
            json!({
                "scenarioId": snapshot.document.scenario_id, "resultId": portable.result_id,
                "scenarioRevision": snapshot.revision, "format": format, "output": args.output,
                "historicalRunMetadata": "sourceProvidedUnverified",
            }),
        )
    };
    let mut outcome = export_output(bytes, metadata, args.output.as_deref(), &control)?;
    outcome.warnings.extend(warnings);
    Ok(outcome)
}

fn export_output(
    bytes: Vec<u8>,
    metadata: Value,
    output: Option<&Path>,
    control: &OperationControl,
) -> Result<Outcome, SafeCliError> {
    let outcome = Outcome::new(
        "solutions.export",
        "exported",
        metadata,
        vec!["Published freshly verified result export.".to_owned()],
    );
    if let Some(output) = output {
        files::publish_text(output, &bytes, control)?;
        Ok(outcome)
    } else {
        files::check_control(control)?;
        Ok(outcome.with_raw(bytes))
    }
}

pub(super) fn explanation_human(
    rendered: &eutheto_domain_ir::EvidenceRenderResultV1,
) -> Result<Vec<String>, SafeCliError> {
    Ok(vec![
        serde_json::to_string_pretty(rendered).map_err(|_| super::serialization_error())?,
    ])
}

fn external_history_warning() -> SafeCliWarning {
    SafeCliWarning {
        code: "solution.external_history_unverified".to_owned(),
        message: "The schedule was freshly independently verified; source-provided solver history, optimality and timings remain unverified.".to_owned(),
        details: None,
    }
}

pub(super) fn stale_warning(solved: Revision, current: Revision) -> Option<SafeCliWarning> {
    (solved != current).then(|| SafeCliWarning {
        code: "solution.stale".to_owned(),
        message: format!(
            "This result belongs to revision {}; the current scenario is revision {}. It has not been applied to the current scenario.",
            solved.value(), current.value(),
        ),
        details: Some(json!({"scenarioRevision": solved, "currentRevision": current})),
    })
}
